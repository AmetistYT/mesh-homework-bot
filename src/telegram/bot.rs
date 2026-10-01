use chrono::{Datelike, Timelike};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::db::Database;
use crate::mesh::models::EventCalendarItem;
use crate::mesh::MeshClient;
use crate::telegram::client::TelegramClient;

#[derive(Debug, Clone)]
pub enum TargetDay {
    Today,
    Tomorrow,
    Specific(chrono::NaiveDate),
}

pub enum BotCommandEvent {
    SolveForDay {
        chat_id: String,
        target: TargetDay,
    },
    ForceCheck,
}

pub struct BotHandler {
    tg: TelegramClient,
    mesh: MeshClient,
    db: Database,
    admin_ids: Vec<i64>,
    event_tx: mpsc::Sender<BotCommandEvent>,
    solver: crate::solver::SolverRunner,
}

impl BotHandler {
    pub fn new(
        tg: TelegramClient,
        mesh: MeshClient,
        db: Database,
        admin_ids: Vec<i64>,
        event_tx: mpsc::Sender<BotCommandEvent>,
        solver: crate::solver::SolverRunner,
    ) -> Self {
        Self {
            tg,
            mesh,
            db,
            admin_ids,
            event_tx,
            solver,
        }
    }

    pub async fn run_polling(&self) {
        let mut offset: Option<i64> = None;
        info!("Запущен long polling для Telegram бота...");

        loop {
            match self.tg.get_updates(offset, 25).await {
                Ok(updates) => {
                    for u in updates {
                        offset = Some(u.update_id + 1);

                        if let Some(msg) = u.message {
                            if let Some(text) = msg.text {
                                let from = msg.from.as_ref();
                                let from_id = from.map(|f| f.id).unwrap_or(0);
                                let from_username = from.and_then(|f| f.username.as_deref());
                                let chat_id = msg.chat.id.to_string();

                                let is_ametist = from_username.map_or(false, |u| {
                                    u.trim_start_matches('@').eq_ignore_ascii_case("ametist1337")
                                });
                                let is_admin = is_ametist
                                    || (!self.admin_ids.is_empty() && self.admin_ids.contains(&from_id));

                                if text.starts_with('/') {
                                    self.handle_command(&chat_id, &text, from_username, from_id, is_admin).await;
                                } else if text.starts_with('!') {
                                    self.handle_chat_query(&chat_id, &text).await;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Ошибка polling обновлений Telegram: {}", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                }
            }
        }
    }

    async fn handle_command(
        &self,
        chat_id: &str,
        text: &str,
        from_username: Option<&str>,
        from_id: i64,
        is_admin: bool,
    ) {
        let parts: Vec<&str> = text.split_whitespace().collect();
        let cmd = parts[0].to_lowercase();

        let is_ametist = from_username.map_or(false, |u| {
            u.trim_start_matches('@').eq_ignore_ascii_case("ametist1337")
        });

        match cmd.as_str() {
            "/start" | "/help" | "/start@ew1484_bot" | "/help@ew1484_bot" => {
                let msg = "Бот автоматического решения домашних заданий МЭШ.\n\n\
                Доступные команды:\n\
                ! <вопрос> - задать вопрос нейросети (объяснить ДЗ, разобрать тему, доступно всем)\n\
                /dz - решение ДЗ на завтра (только @ametist1337)\n\
                /dz <день недели> - решение на любой день (пн, вт, ср, чт, пт, сб, вс)\n\
                /dz сегодня - решение заданий на сегодня\n\
                /status - текущее состояние сервиса и статистика\n\
                /check - принудительная проверка заданий (админ)\n\
                /token <новый_jwt> - обновить JWT токен авторизации МЭШ\n\
                /help - справка по командам";
                let _ = self.tg.send_message(chat_id, msg).await;
            }
            "/dz" | "/дз" | "/hw" | "/dz@ew1484_bot" | "/дз@ew1484_bot" | "/hw@ew1484_bot" => {
                if !is_ametist && !is_admin {
                    warn!(
                        "Отклонен вызов команды {} от пользователя {:?} (id: {})",
                        cmd, from_username, from_id
                    );
                    let _ = self
                        .tg
                        .send_message(chat_id, "Команда /dz доступна только @ametist1337.")
                        .await;
                    return;
                }
                let today = chrono::Local::now().date_naive();
                let is_force = parts.iter().any(|&p| {
                    let low = p.to_lowercase();
                    low == "force" || low == "перерешай" || low == "заново" || low == "сброс" || low == "reset"
                });

                // Ищем аргумент дня, игнорируя команду и флаги сброса
                let day_arg = parts.iter().skip(1).find(|&&p| {
                    let low = p.to_lowercase();
                    low != "force" && low != "перерешай" && low != "заново" && low != "сброс" && low != "reset"
                });

                let (target, target_name) = if let Some(arg) = day_arg {
                    parse_target_day_input(arg, today)
                } else {
                    (TargetDay::Tomorrow, "завтра".to_string())
                };

                if is_force {
                    let target_date = match &target {
                        TargetDay::Today => today,
                        TargetDay::Tomorrow => today + chrono::Duration::days(1),
                        TargetDay::Specific(d) => *d,
                    };
                    let date_str = target_date.format("%Y-%m-%d").to_string();
                    let _ = self.db.clear_cache_for_date(&date_str);
                    let _ = self.tg.send_message(chat_id, &format!("Сброшен кэш для {}. Перерешиваю с нуля...", target_name)).await;
                } else {
                    let _ = self.tg.send_message(chat_id, &format!("Ищу задания на {} и запускаю решение...", target_name)).await;
                }

                let _ = self.event_tx.send(BotCommandEvent::SolveForDay {
                    chat_id: chat_id.to_string(),
                    target,
                }).await;
            }
            "/status" | "/status@ew1484_bot" => {
                let is_expired = self.mesh.is_token_expired().await;
                let stats = self.db.get_stats().unwrap_or(crate::db::DbStats {
                    total: 0,
                    sent: 0,
                    failed: 0,
                    pending: 0,
                });

                let status_msg = format!(
                    "СТАТУС СЕРВИСА:\n\
                    МЭШ токен: {}\n\
                    Всего заданий в БД: {}\n\
                    Успешно отправлено решений: {}\n\
                    В процессе / ожидает: {}\n\
                    Ошибок решения: {}",
                    if is_expired { "ИСТЕК (требуется /token)" } else { "АКТИВЕН" },
                    stats.total,
                    stats.sent,
                    stats.pending,
                    stats.failed
                );
                let _ = self.tg.send_message(chat_id, &status_msg).await;
            }
            "/check" | "/check@ew1484_bot" => {
                if !is_admin {
                    let _ = self.tg.send_message(chat_id, "Команда доступна только администратору.").await;
                    return;
                }
                let _ = self.tg.send_message(chat_id, "Запуск внеочередной проверки заданий в МЭШ...").await;
                let _ = self.event_tx.send(BotCommandEvent::ForceCheck).await;
            }
            "/token" | "/token@ew1484_bot" => {
                if !is_admin {
                    let _ = self.tg.send_message(chat_id, "Команда доступна только администратору.").await;
                    return;
                }
                if parts.len() < 2 {
                    let _ = self.tg.send_message(chat_id, "Использование: /token <новый_jwt_токен>").await;
                    return;
                }

                let new_token = parts[1].trim().to_string();
                if MeshClient::check_jwt_expired(&new_token) {
                    let _ = self.tg.send_message(chat_id, "Ошибка: переданный токен невалиден или уже истек.").await;
                    return;
                }

                self.mesh.set_token(new_token.clone()).await;
                let _ = self.db.set_setting("mesh_token", &new_token);
                let _ = self.tg.send_message(chat_id, "МЭШ токен успешно обновлен и сохранен в базе данных.").await;
            }
            "/schedule" | "/schedule@ew1484_bot" | "/расписание" | "/расписание@ew1484_bot" | "/lessons" | "/звонки" => {
                let today = chrono::Local::now().date_naive();
                let day_arg = parts.iter().skip(1).next().copied();
                let (target, _name) = if let Some(arg) = day_arg {
                    parse_target_day_input(arg, today)
                } else {
                    let hour = chrono::Local::now().hour();
                    if hour >= 15 {
                        (TargetDay::Tomorrow, "завтра".to_string())
                    } else {
                        (TargetDay::Today, "сегодня".to_string())
                    }
                };

                let target_date = match target {
                    TargetDay::Today => today,
                    TargetDay::Tomorrow => today + chrono::Duration::days(1),
                    TargetDay::Specific(d) => d,
                };

                let mesh_client = self.mesh.clone();
                let tg_client = self.tg.clone();
                let chat_id_str = chat_id.to_string();

                tokio::spawn(async move {
                    match mesh_client.get_events(target_date, target_date).await {
                        Ok(events) => {
                            let text = format_day_schedule_telegram(target_date, &events);
                            let _ = tg_client.send_message(&chat_id_str, &text).await;
                        }
                        Err(e) => {
                            let _ = tg_client.send_message(&chat_id_str, &format!("Ошибка получения расписания: {}", e)).await;
                        }
                    }
                });
            }
            _ => {}
        }
    }

    async fn handle_chat_query(&self, chat_id: &str, raw_text: &str) {
        let query = raw_text.trim_start_matches('!').trim();
        if query.is_empty() {
            return;
        }

        info!("Получен запрос к AI чату: '{}'", query);

        let today = chrono::Local::now().date_naive();
        let from_date = today - chrono::Duration::days(14);
        let to_date = today + chrono::Duration::days(14);

        let mesh_client = self.mesh.clone();
        // Параллельно запрашиваем живые домашние задания и события календаря (расписание с таймингами)
        let (live_hws_res, events_res) = tokio::join!(
            mesh_client.get_homeworks(from_date, to_date),
            mesh_client.get_events(from_date, to_date)
        );

        let live_hws = live_hws_res.unwrap_or_default();
        let events = events_res.unwrap_or_default();

        let schedule_context = format_schedule_context(&events);

        let mut mesh_context = String::new();
        if !live_hws.is_empty() {
            mesh_context.push_str(&format!(
                "АКТУАЛЬНЫЕ ДОМАШНИЕ ЗАДАНИЯ ИЗ МЭШ (СЕГОДНЯ: {}, диапазон {} — {}):\n",
                today.format("%d.%m.%Y"),
                from_date.format("%d.%m.%Y"),
                to_date.format("%d.%m.%Y")
            ));
            for hw in &live_hws {
                let sub = hw.subject_name.as_deref().unwrap_or("Предмет");
                let date = hw.date.as_deref().or(hw.lesson_date_time.as_deref()).unwrap_or("—");
                let desc = hw.description.as_deref().unwrap_or("").trim();
                let is_done_str = if hw.is_done.unwrap_or(false) { " (выполнено)" } else { "" };
                mesh_context.push_str(&format!("• [{}] {}{}: {}\n", date, sub, is_done_str, desc));
            }
            mesh_context.push('\n');
        }

        // 2. Получаем готовые решения из локальной базы данных
        let recent_solutions = self.db.get_recent_solutions(15).unwrap_or_default();
        let mut solutions_context = String::new();
        if !recent_solutions.is_empty() {
            solutions_context.push_str("БАЗА ГОТОВЫХ РЕШЕНИЙ ДОМАШНИХ ЗАДАНИЙ:\n");
            for (sub, date, desc, sol) in recent_solutions {
                solutions_context.push_str(&format!("--- ПРЕДМЕТ: {} (Срок: {}) ---\n", sub, date));
                if !desc.trim().is_empty() {
                    solutions_context.push_str(&format!("Задание: {}\n", desc.trim()));
                }
                let short_sol: String = sol.chars().take(900).collect();
                solutions_context.push_str(&format!("Решение:\n{}\n\n", short_sol));
            }
        }

        let prompt = format!(
            "Ты — дружелюбный, умный школьный помощник и репетитор для учеников 7 класса.\n\
             Тебе задали вопрос в Telegram-чате класса.\n\n\
             {}\n\
             {}\n\
             {}\n\
             ВОПРОС ПОЛЬЗОВАТЕЛЯ:\n\
             \"\"\"\n{}\n\"\"\"\n\n\
             ПРАВИЛА ОТВЕТА:\n\
             1. В твоем распоряжении официальные данные МЭШ: точное расписание уроков со звонками (таймингами), номерами кабинетов и учителями, а также домашние задания и база готовых решений.\n\
             2. Отвечай прямо и точно на вопрос пользователя (расписание уроков, звонки, время начала/окончания уроков, перемены, кабинеты, домашние задания, объяснения тем и решений).\n\
             3. Если пользователь спрашивает про конкретный день недели, сегодня, завтра или даты — сопоставь с текущей датой ({}) и расписанием МЭШ.\n\
             4. Форматируй ответ в стиле Telegram Markdown/HTML: используй *жирный шрифт*, `моноширинный текст`, списки.\n\
             5. ЗАПРЕЩЕНО использовать LaTeX ($$, $, \\frac, \\circ). Пиши обычным текстом, градусы знаком ° или °C.\n\
             6. Не используй смайлики и эмодзи.\n\
             7. Отвечай кратко, емко, без лишней воды и длинных вступлений.",
            schedule_context,
            mesh_context,
            solutions_context,
            query,
            today.format("%d.%m.%Y (%A)")
        );

        let tg_client = self.tg.clone();
        let solver = self.solver.clone();
        let chat_id_str = chat_id.to_string();

        tokio::spawn(async move {
            match solver.ask_chat(&prompt, "gemini-3.8-flash-low").await {
                Ok(reply) => {
                    let _ = tg_client.send_message(&chat_id_str, &reply).await;
                }
                Err(e) => {
                    warn!("Ошибка генерации ответа в AI чате: {}", e);
                    let _ = tg_client.send_message(&chat_id_str, &format!("Не удалось получить ответ: {}", e)).await;
                }
            }
        });
    }
}

pub fn parse_target_day_input(input: &str, today: chrono::NaiveDate) -> (TargetDay, String) {
    let s = input.trim().to_lowercase();
    match s.as_str() {
        "сегодня" | "today" => (TargetDay::Today, "сегодня".to_string()),
        "завтра" | "tomorrow" => (TargetDay::Tomorrow, "завтра".to_string()),
        "послезавтра" => {
            let d = today + chrono::Duration::days(2);
            (TargetDay::Specific(d), format!("послезавтра ({})", d.format("%d.%m")))
        }
        _ => {
            let weekday = match s.as_str() {
                "пн" | "понедельник" | "mon" | "monday" => Some((chrono::Weekday::Mon, "понедельник")),
                "вт" | "вторник" | "tue" | "tuesday" => Some((chrono::Weekday::Tue, "вторник")),
                "ср" | "среда" | "среду" | "wed" | "wednesday" => Some((chrono::Weekday::Wed, "среду")),
                "чт" | "четверг" | "thu" | "thursday" => Some((chrono::Weekday::Thu, "четверг")),
                "пт" | "пятница" | "пятницу" | "fri" | "friday" => Some((chrono::Weekday::Fri, "пятницу")),
                "сб" | "суббота" | "субботу" | "sat" | "saturday" => Some((chrono::Weekday::Sat, "субботу")),
                "вс" | "воскресенье" | "sun" | "sunday" => Some((chrono::Weekday::Sun, "воскресенье")),
                _ => None,
            };

            if let Some((target_wd, name)) = weekday {
                let current_wd = today.weekday();
                let diff = (target_wd.num_days_from_monday() as i64 + 7 - current_wd.num_days_from_monday() as i64) % 7;
                let days_ahead = if diff == 0 {
                    let hour = chrono::Local::now().hour();
                    if hour >= 14 { 7 } else { 0 }
                } else {
                    diff
                };
                let target_date = today + chrono::Duration::days(days_ahead);
                return (
                    TargetDay::Specific(target_date),
                    format!("{} ({})", name, target_date.format("%d.%m")),
                );
            }

            if let Ok(d) = chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
                return (TargetDay::Specific(d), d.format("%d.%m.%Y").to_string());
            }
            if let Ok(d) = chrono::NaiveDate::parse_from_str(&s, "%d.%m.%Y") {
                return (TargetDay::Specific(d), d.format("%d.%m.%Y").to_string());
            }
            if let Ok(d) = chrono::NaiveDate::parse_from_str(&s, "%d.%m") {
                let year = today.year();
                if let Some(with_year) = d.with_year(year) {
                    return (TargetDay::Specific(with_year), with_year.format("%d.%m").to_string());
                }
            }

            // По умолчанию - завтра
            (TargetDay::Tomorrow, "завтра".to_string())
        }
    }
}

pub fn weekday_to_ru(wd: chrono::Weekday) -> &'static str {
    match wd {
        chrono::Weekday::Mon => "Понедельник",
        chrono::Weekday::Tue => "Вторник",
        chrono::Weekday::Wed => "Среда",
        chrono::Weekday::Thu => "Четверг",
        chrono::Weekday::Fri => "Пятница",
        chrono::Weekday::Sat => "Суббота",
        chrono::Weekday::Sun => "Воскресенье",
    }
}

pub fn format_schedule_context(events: &[EventCalendarItem]) -> String {
    if events.is_empty() {
        return String::new();
    }

    use std::collections::BTreeMap;
    let mut by_date: BTreeMap<String, Vec<&EventCalendarItem>> = BTreeMap::new();

    for ev in events {
        if let Some(ref start) = ev.start_at {
            if start.len() >= 10 {
                let date_str = &start[..10];
                by_date.entry(date_str.to_string()).or_default().push(ev);
            }
        }
    }

    let mut out = String::new();
    out.push_str("РАСПИСАНИЕ И ТАЙМИНГИ УРОКОВ (ЗВОНКИ, КАБИНЕТЫ, УЧИТЕЛЯ):\n");

    for (date_str, mut day_events) in by_date {
        day_events.sort_by_key(|e| e.start_at.as_deref().unwrap_or(""));

        let day_title = if let Ok(d) = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
            format!("{} ({})", d.format("%d.%m.%Y"), weekday_to_ru(d.weekday()))
        } else {
            date_str.clone()
        };

        out.push_str(&format!("• {}:\n", day_title));

        for (idx, ev) in day_events.iter().enumerate() {
            let num = idx + 1;
            let start_time = ev
                .start_at
                .as_deref()
                .and_then(|s| if s.len() >= 16 { Some(&s[11..16]) } else { None })
                .unwrap_or("??:??");
            let end_time = ev
                .finish_at
                .as_deref()
                .and_then(|s| if s.len() >= 16 { Some(&s[11..16]) } else { None })
                .unwrap_or("??:??");
            let subject = ev.subject_name.as_deref().unwrap_or("Урок");

            let mut details = Vec::new();
            if let Some(ref room) = ev.room_number {
                if !room.trim().is_empty() {
                    details.push(format!("каб. {}", room.trim()));
                }
            } else if let Some(ref room) = ev.room_name {
                if !room.trim().is_empty() {
                    details.push(format!("каб. {}", room.trim()));
                }
            }

            if let Some(ref teacher) = ev.teacher.as_ref().or(ev.author.as_ref()) {
                let mut t_name = String::new();
                if let Some(ref ln) = teacher.last_name {
                    t_name.push_str(ln);
                }
                if let Some(ref fn_) = teacher.first_name {
                    if let Some(ch) = fn_.chars().next() {
                        t_name.push_str(&format!(" {}.", ch));
                    }
                }
                if let Some(ref mn) = teacher.middle_name {
                    if let Some(ch) = mn.chars().next() {
                        t_name.push_str(&format!("{}.", ch));
                    }
                }
                if !t_name.trim().is_empty() {
                    details.push(format!("учитель: {}", t_name.trim()));
                }
            }

            if ev.cancelled.unwrap_or(false) {
                details.push("ОТМЕНЕН".to_string());
            } else if ev.replaced.unwrap_or(false) {
                details.push("ЗАМЕНА".to_string());
            }

            if let Some(ref marks) = ev.marks {
                let marks_str: Vec<String> = marks.iter().filter_map(|m| m.value.clone()).collect();
                if !marks_str.is_empty() {
                    details.push(format!("оценка: {}", marks_str.join(", ")));
                }
            }

            let details_str = if details.is_empty() {
                String::new()
            } else {
                format!(" ({})", details.join(", "))
            };

            out.push_str(&format!(
                "  {}. {} - {}: {}{}\n",
                num, start_time, end_time, subject, details_str
            ));
        }
        out.push('\n');
    }

    out
}

pub fn format_day_schedule_telegram(date: chrono::NaiveDate, events: &[EventCalendarItem]) -> String {
    let date_str = date.format("%Y-%m-%d").to_string();
    let mut day_events: Vec<&EventCalendarItem> = events
        .iter()
        .filter(|e| e.start_at.as_deref().map(|s| s.starts_with(&date_str)).unwrap_or(false))
        .collect();

    if day_events.is_empty() {
        return format!(
            "На {} ({}) уроков в расписании нет.",
            date.format("%d.%m.%Y"),
            weekday_to_ru(date.weekday())
        );
    }

    day_events.sort_by_key(|e| e.start_at.as_deref().unwrap_or(""));

    let mut out = format!(
        "РАСПИСАНИЕ НА {} ({}):\n\n",
        date.format("%d.%m.%Y"),
        weekday_to_ru(date.weekday()).to_uppercase()
    );

    for (idx, ev) in day_events.iter().enumerate() {
        let num = idx + 1;
        let start_time = ev
            .start_at
            .as_deref()
            .and_then(|s| if s.len() >= 16 { Some(&s[11..16]) } else { None })
            .unwrap_or("??:??");
        let end_time = ev
            .finish_at
            .as_deref()
            .and_then(|s| if s.len() >= 16 { Some(&s[11..16]) } else { None })
            .unwrap_or("??:??");
        let subject = ev.subject_name.as_deref().unwrap_or("Урок");

        let room = ev
            .room_number
            .as_deref()
            .or(ev.room_name.as_deref())
            .map(|r| format!(" • каб. {}", r.trim()))
            .unwrap_or_default();

        let status = if ev.cancelled.unwrap_or(false) {
            " [ОТМЕНЕН]"
        } else if ev.replaced.unwrap_or(false) {
            " [ЗАМЕНА]"
        } else {
            ""
        };

        out.push_str(&format!(
            "{}. `{} - {}` *{}*{}{}\n",
            num, start_time, end_time, subject, room, status
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_schedule_context() {
        let events = vec![
            EventCalendarItem {
                id: Some(1),
                start_at: Some("2026-10-02T08:30:00+03:00".to_string()),
                finish_at: Some("2026-10-02T09:10:00+03:00".to_string()),
                cancelled: Some(false),
                replaced: Some(false),
                is_missed_lesson: Some(false),
                room_name: None,
                room_number: Some("16".to_string()),
                subject_id: Some(101),
                subject_name: Some("Физика".to_string()),
                teacher: None,
                author: None,
                homework: None,
                marks: None,
            },
            EventCalendarItem {
                id: Some(2),
                start_at: Some("2026-10-02T09:30:00+03:00".to_string()),
                finish_at: Some("2026-10-02T10:10:00+03:00".to_string()),
                cancelled: Some(false),
                replaced: Some(false),
                is_missed_lesson: Some(false),
                room_name: None,
                room_number: Some("109".to_string()),
                subject_id: Some(102),
                subject_name: Some("Алгебра".to_string()),
                teacher: None,
                author: None,
                homework: None,
                marks: None,
            },
        ];

        let text = format_schedule_context(&events);
        assert!(text.contains("08:30 - 09:10: Физика (каб. 16)"));
        assert!(text.contains("09:30 - 10:10: Алгебра (каб. 109)"));

        let day_text = format_day_schedule_telegram(chrono::NaiveDate::from_ymd_opt(2026, 10, 2).unwrap(), &events);
        assert!(day_text.contains("РАСПИСАНИЕ НА 02.10.2026"));
        assert!(day_text.contains("08:30 - 09:10"));
        assert!(day_text.contains("Физика"));
    }
}

