use chrono::{Datelike, Duration, Local, NaiveDate, Timelike};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::db::Database;
use crate::mesh::MeshClient;
use crate::solver::{PromptBuilder, SolverRunner};
use crate::telegram::{BotCommandEvent, TargetDay, TelegramClient};

pub struct Engine {
    mesh: MeshClient,
    tg: TelegramClient,
    db: Database,
    prompt_builder: PromptBuilder,
    solver: SolverRunner,
    channel_id: String,
    poll_interval_secs: u64,
    days_ahead: i64,
    days_back: i64,
    auto_mark_done: bool,
}

impl Engine {
    pub fn new(
        mesh: MeshClient,
        tg: TelegramClient,
        db: Database,
        prompt_builder: PromptBuilder,
        solver: SolverRunner,
        channel_id: String,
        poll_interval_secs: u64,
        days_ahead: i64,
        days_back: i64,
        auto_mark_done: bool,
    ) -> Self {
        Self {
            mesh,
            tg,
            db,
            prompt_builder,
            solver,
            channel_id,
            poll_interval_secs,
            days_ahead,
            days_back,
            auto_mark_done,
        }
    }

    pub fn format_russian_date(date_str: &str) -> String {
        if let Ok(d) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            let weekday = match d.weekday() {
                chrono::Weekday::Mon => "понедельник",
                chrono::Weekday::Tue => "вторник",
                chrono::Weekday::Wed => "среда",
                chrono::Weekday::Thu => "четверг",
                chrono::Weekday::Fri => "пятница",
                chrono::Weekday::Sat => "суббота",
                chrono::Weekday::Sun => "воскресенье",
            };
            let month = match d.month() {
                1 => "янв",
                2 => "фев",
                3 => "мар",
                4 => "апр",
                5 => "май",
                6 => "июн",
                7 => "июл",
                8 => "авг",
                9 => "сент",
                10 => "окт",
                11 => "нояб",
                12 => "дек",
                _ => "",
            };
            format!("{} {} {}", weekday, d.day(), month)
        } else {
            date_str.to_string()
        }
    }

    pub async fn run_loop(&self, mut event_rx: mpsc::Receiver<BotCommandEvent>) {
        info!("Главный рабочий цикл запущен. Режим: решение по /dz или в 15:00 ежедневно.");
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
        let mut last_auto_run_date: Option<NaiveDate> = None;

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.unpin_expired_messages().await;
                    let _ = self.mesh.check_and_proactive_refresh().await;

                    let now = Local::now();
                    let today = now.date_naive();
                    // Автоматический запуск ровно в 15:00 каждый день по Москве
                    if now.hour() == 15 && now.minute() == 0 {
                        if last_auto_run_date != Some(today) {
                            last_auto_run_date = Some(today);
                            info!("Ежедневный автозапуск в 15:00: решение ДЗ на завтра!");
                            self.solve_and_publish_day(TargetDay::Tomorrow, &self.channel_id).await;
                        }
                    }
                }
                Some(event) = event_rx.recv() => {
                    match event {
                        BotCommandEvent::SolveForDay { chat_id, target } => {
                            info!("Получена команда решения ДЗ для {:?}", target);
                            self.solve_and_publish_day(target, &chat_id).await;
                        }
                        BotCommandEvent::ForceCheck => {
                            info!("Получена команда принудительной проверки заданий");
                            self.solve_and_publish_day(TargetDay::Tomorrow, &self.channel_id).await;
                        }
                    }
                }
            }
        }
    }

    async fn unpin_expired_messages(&self) {
        match self.db.get_expired_pins(24) {
            Ok(expired) => {
                for (hw_id, msg_id) in expired {
                    info!(
                        "Открепляю сообщение {} для задания {} (истекло 24 часа)...",
                        msg_id, hw_id
                    );
                    let target_chat = &self.channel_id;
                    if let Err(e) = self.tg.unpin_chat_message(target_chat, msg_id).await {
                        warn!("Ошибка при откреплении сообщения {}: {}", msg_id, e);
                    }
                    let _ = self.db.mark_unpinned(hw_id);
                }
            }
            Err(e) => {
                warn!("Ошибка получения списка истекших закрепов: {}", e);
            }
        }
    }

    pub async fn solve_and_publish_day(&self, target: TargetDay, chat_id: &str) {
        info!("=== Начало обработки ДЗ для {:?} ===", target);

        self.unpin_expired_messages().await;

        if self.mesh.is_token_expired().await {
            warn!("МЭШ токен истек. Пробуем обновить через СУДИР...");
            let _ = self.mesh.try_refresh_sudir().await;
        }

        let today = Local::now().date_naive();
        let mut target_date = match target {
            TargetDay::Today => today,
            TargetDay::Tomorrow => today + Duration::days(1),
            TargetDay::Specific(d) => d,
        };

        // Запрашиваем задания на целевую дату
        let mut items = match self.mesh.get_homeworks(target_date, target_date).await {
            Ok(hws) => hws,
            Err(e) => {
                error!("Ошибка получения ДЗ из МЭШ: {}", e);
                let _ = self
                    .tg
                    .send_message(chat_id, &format!("Ошибка получения данных из МЭШ: {}", e))
                    .await;
                return;
            }
        };

        // Если на целевой день пусто (например, выходные), ищем ближайший следующий учебный день
        if items.is_empty() {
            for offset in 1..=3 {
                let candidate = target_date + Duration::days(offset);
                if let Ok(cand_items) = self.mesh.get_homeworks(candidate, candidate).await {
                    if !cand_items.is_empty() {
                        items = cand_items;
                        target_date = candidate;
                        break;
                    }
                }
            }

            if items.is_empty() {
                let date_str = target_date.format("%Y-%m-%d").to_string();
                let pretty_date = Self::format_russian_date(&date_str);
                let msg = format!("{}:\nВ электронном дневнике МЭШ заданий нет.", pretty_date);
                let _ = self.tg.send_message(chat_id, &msg).await;
                return;
            }
        }

        let date_str = target_date.format("%Y-%m-%d").to_string();
        let pretty_date = Self::format_russian_date(&date_str);

        let target_chat = if chat_id.is_empty() {
            &self.channel_id
        } else {
            chat_id
        };

        // 1. Формируем верхний список всех заданий на день для подписи к файлу
        let mut full_tasks_summary = format!("{}:\n", pretty_date);
        for item in &items {
            let sub = item.subject_name.as_deref().unwrap_or("Предмет");
            let desc = item.description.as_deref().unwrap_or("");
            if !desc.trim().is_empty() {
                let clean_desc = desc.replace('\n', " ");
                full_tasks_summary.push_str(&format!("- {}: {}\n", sub, clean_desc.trim()));
            }
        }

        // Подготавливаем статусы для динамического сообщения прогресса
        #[derive(Clone, PartialEq)]
        enum TaskProgressStatus {
            Pending,
            Solving,
            Done,
            Cached,
            Electronic,
            Failed,
        }

        let mut progress_list: Vec<(String, String, TaskProgressStatus)> = Vec::new();
        for item in &items {
            let sub = item.subject_name.as_deref().unwrap_or("Предмет").to_string();
            let desc = item.description.as_deref().unwrap_or("").to_string();
            if desc.trim().is_empty() {
                continue;
            }
            let is_rus = sub.to_lowercase().contains("русск");
            let is_cdz = desc.to_lowercase().contains("цдз") || desc.to_lowercase().contains("электронн") || is_rus;
            let initial_status = if is_cdz {
                TaskProgressStatus::Electronic
            } else if self.db.get_homework_solution(item.homework_entry_student_id).ok().flatten().is_some() {
                TaskProgressStatus::Cached
            } else {
                TaskProgressStatus::Pending
            };
            progress_list.push((sub, desc, initial_status));
        }

        // Вспомогательная функция формирования текста прогресса
        let build_progress_text = |p_list: &[(String, String, TaskProgressStatus)], current_date: &str| -> String {
            let mut text = format!("Выполняю домашние задания на {}:\n\n", current_date);
            for (sub, _desc, status) in p_list {
                let status_str = match status {
                    TaskProgressStatus::Pending => "[В очереди]",
                    TaskProgressStatus::Solving => "[Выполняется...]",
                    TaskProgressStatus::Done => "[Готово]",
                    TaskProgressStatus::Cached => "[Готово (из базы)]",
                    TaskProgressStatus::Electronic => "[ЦДЗ / Электронно]",
                    TaskProgressStatus::Failed => "[Ошибка]",
                };
                text.push_str(&format!("• {} — {}\n", sub, status_str));
            }
            text
        };

        // Отправляем начальное сообщение с прогрессом
        let mut progress_msg_id: Option<i64> = None;
        if !progress_list.is_empty() {
            let initial_progress_text = build_progress_text(&progress_list, &pretty_date);
            if let Ok(msg_id) = self.tg.send_message(target_chat, &initial_progress_text).await {
                progress_msg_id = Some(msg_id);
            }
        }

        // 2. Решаем каждый предмет по очереди и обновляем прогресс
        let mut solutions: Vec<(String, String)> = Vec::new();

        for item in &items {
            let hw_id = item.homework_entry_student_id;
            let subject = item.subject_name.as_deref().unwrap_or("Предмет");
            let desc = item.description.as_deref().unwrap_or("");

            if desc.trim().is_empty() {
                continue;
            }

            // Русский язык исключен из решения (ЦДЗ)
            if subject.to_lowercase().contains("русск") {
                info!("Предмет '{}' исключен из решения (ЦДЗ / сводка)", subject);
                continue;
            }

            // Проверяем кэш базы данных
            if let Ok(Some(cached_sol)) = self.db.get_homework_solution(hw_id) {
                info!("Для задания {} ({}) найдено готовое решение в базе данных", hw_id, subject);
                let cleaned_cached = crate::solver::latex::clean_latex(&cached_sol);
                solutions.push((subject.to_string(), cleaned_cached));

                if let Some(pos) = progress_list.iter().position(|(s, _, _)| s == subject) {
                    progress_list[pos].2 = TaskProgressStatus::Cached;
                    if let Some(msg_id) = progress_msg_id {
                        let _ = self.tg.edit_message_text(target_chat, msg_id, &build_progress_text(&progress_list, &pretty_date)).await;
                    }
                }
                continue;
            }

            // Обновляем статус: в процессе решения
            if let Some(pos) = progress_list.iter().position(|(s, _, _)| s == subject) {
                progress_list[pos].2 = TaskProgressStatus::Solving;
                if let Some(msg_id) = progress_msg_id {
                    let _ = self.tg.edit_message_text(target_chat, msg_id, &build_progress_text(&progress_list, &pretty_date)).await;
                }
            }

            // Собираем вложения
            let mut att_info = String::new();
            if let Some(ref atts) = item.attachments {
                for a in atts {
                    if let Some(ref name) = a.file_name.as_ref().or(a.name.as_ref()) {
                        let link = a.link.as_ref().or(a.url.as_ref()).map(|s| s.as_str()).unwrap_or("");
                        att_info.push_str(&format!("- Файл: {} ({})\n", name, link));
                    }
                }
            }
            let att_opt = if att_info.is_empty() { None } else { Some(att_info.as_str()) };

            let prompt = self.prompt_builder.build_prompt(subject, &date_str, desc, att_opt);

            let _ = self.db.insert_pending_homework(hw_id, subject, &date_str, desc);
            let _ = self.db.update_solution(hw_id, "", "solving");

            info!("Запуск Antigravity solver для {} (hw_id: {})...", subject, hw_id);
            match self.solver.solve(&prompt).await {
                Ok(sol) => {
                    info!("Задание {} успешно решено agy", hw_id);
                    let cleaned_sol = crate::solver::latex::clean_latex(&sol);
                    let _ = self.db.update_solution(hw_id, &cleaned_sol, "sent");
                    solutions.push((subject.to_string(), cleaned_sol));

                    if let Some(pos) = progress_list.iter().position(|(s, _, _)| s == subject) {
                        progress_list[pos].2 = TaskProgressStatus::Done;
                        if let Some(msg_id) = progress_msg_id {
                            let _ = self.tg.edit_message_text(target_chat, msg_id, &build_progress_text(&progress_list, &pretty_date)).await;
                        }
                    }
                }
                Err(e) => {
                    error!("Ошибка решения задания {} через agy: {}", hw_id, e);
                    let _ = self.db.update_solution(hw_id, &format!("Ошибка solver: {}", e), "failed");
                    solutions.push((subject.to_string(), format!("Не удалось получить решение: {}", e)));

                    if let Some(pos) = progress_list.iter().position(|(s, _, _)| s == subject) {
                        progress_list[pos].2 = TaskProgressStatus::Failed;
                        if let Some(msg_id) = progress_msg_id {
                            let _ = self.tg.edit_message_text(target_chat, msg_id, &build_progress_text(&progress_list, &pretty_date)).await;
                        }
                    }
                }
            }
        }

        // Финальное обновление текста прогресса
        if let Some(msg_id) = progress_msg_id {
            let mut final_progress_text = build_progress_text(&progress_list, &pretty_date);
            final_progress_text.push_str("\nВсе задания обработаны! Файл решений прикреплен ниже.");
            let _ = self.tg.edit_message_text(target_chat, msg_id, &final_progress_text).await;
        }

        // 3. Запекаем решения всех предметов в интерактивный HTML файл
        let mut export_items: Vec<crate::solver::SubjectExportItem> = Vec::new();
        for (idx, item) in items.iter().enumerate() {
            let hw_id = item.homework_entry_student_id;
            let subject = item.subject_name.as_deref().unwrap_or("Предмет");
            let desc = item.description.as_deref().unwrap_or("");
            let is_rus = subject.to_lowercase().contains("русск");
            let is_cdz = desc.to_lowercase().contains("цдз")
                || desc.to_lowercase().contains("электронн")
                || is_rus;

            let solution_md = if let Some((_, sol)) = solutions.iter().find(|(s, _)| s == subject) {
                sol.clone()
            } else if let Ok(Some(cached)) = self.db.get_homework_solution(hw_id) {
                cached
            } else if is_cdz {
                "Задание выполняется в электронном виде на портале МЭШ (ЦДЗ).".to_string()
            } else {
                "Задание устное или не требует отдельного письменного решения.".to_string()
            };

            let preview = if is_cdz {
                "ЦДЗ на платформе МЭШ".to_string()
            } else if let Some(first_line) = solution_md.lines().find(|l| !l.trim().is_empty()) {
                let clean_line = first_line.replace('#', "").replace('*', "");
                clean_line.trim().chars().take(60).collect::<String>()
            } else {
                "Решение внутри".to_string()
            };

            export_items.push(crate::solver::SubjectExportItem {
                id: format!("sub_{}", idx + 1),
                name: subject.to_string(),
                is_cdz,
                preview_text: preview,
                task_text: desc.to_string(),
                solution_markdown: solution_md,
                is_open: idx == 0,
            });
        }

        let html_content = crate::solver::HtmlExporter::render_html(&pretty_date, &export_items);
        match crate::solver::HtmlExporter::save_export_file("data/exports", &date_str, &html_content) {
            Ok(export_path) => {
                // Подпись к файлу содержит все домашние задания на день
                let doc_caption = full_tasks_summary.clone();
                match self.tg.send_document(target_chat, &export_path, Some(&doc_caption)).await {
                    Ok(doc_msg_id) => {
                        info!("HTML-файл с решениями отправлен в Telegram (msg_id: {})", doc_msg_id);
                        // ЗАКРЕПЛЯЕМ ИМЕННО ФАЙЛ С ПОДПИСЬЮ
                        if let Err(e) = self.tg.pin_chat_message(target_chat, doc_msg_id).await {
                            warn!("Не удалось закрепить файл с решениями {}: {}", doc_msg_id, e);
                        } else {
                            info!("Файл с решениями (msg_id: {}) успешно закреплен в чате", doc_msg_id);
                        }
                        for item in &items {
                            let _ = self.db.mark_sent_and_pinned(item.homework_entry_student_id, doc_msg_id);
                        }
                    }
                    Err(e) => {
                        warn!("Не удалось отправить HTML-документ в Telegram: {}", e);
                    }
                }
            }
            Err(e) => {
                error!("Ошибка сохранения HTML-экспорта: {}", e);
            }
        }

        info!("=== Обработка ДЗ завершена ===");
    }
}
