use chrono::{Datelike, Timelike};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::db::Database;
use crate::mesh::models::EventCalendarItem;
use crate::mesh::MeshClient;
use crate::telegram::client::TelegramClient;
use crate::telegram::moderation::ModerationManager;
use crate::solver::VisualRenderer;

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
        reply_to_message_id: Option<i64>,
    },
    ForceCheck,
}

#[derive(Clone)]
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

        // Запуск фонового воркера восстановления прав модерации
        ModerationManager::start_restoration_worker(self.tg.clone(), self.db.clone());

        loop {
            match self.tg.get_updates(offset, 25).await {
                Ok(updates) => {
                    for u in updates {
                        offset = Some(u.update_id + 1);

                        if let Some(msg) = u.message {
                            let message_id = msg.message_id;
                            let chat_id = msg.chat.id.to_string();
                            let from = msg.from.as_ref();
                            let from_id = from.map(|f| f.id).unwrap_or(0);
                            let from_username = from.and_then(|f| f.username.as_deref());

                            let from_name = match from {
                                Some(f) => {
                                    if let Some(ref ln) = f.last_name {
                                        format!("{} {}", f.first_name, ln)
                                    } else {
                                        f.first_name.clone()
                                    }
                                }
                                None => "Пользователь".to_string(),
                            };

                            if let Some(f) = from {
                                let _ = self.db.save_seen_user(f.id, f.username.as_deref(), &f.first_name, f.last_name.as_deref(), None);
                            }

                            let reply_user = msg.reply_to_message.as_ref().and_then(|r| {
                                r.from.as_ref().map(|u| {
                                    let _ = self.db.save_seen_user(u.id, u.username.as_deref(), &u.first_name, u.last_name.as_deref(), None);
                                    (u.id, u.username.clone().map(|un| format!("@{}", un)), Some(u.first_name.clone()))
                                })
                            });

                            let (reply_to_msg_id, reply_to_uid, reply_to_text) = match msg.reply_to_message.as_ref() {
                                Some(r) => {
                                    let r_uid = r.from.as_ref().map(|u| u.id);
                                    let r_text = r.display_text();
                                    let clean_r = if r_text.trim().is_empty() { None } else { Some(r_text) };
                                    (Some(r.message_id), r_uid, clean_r)
                                }
                                None => (None, None, None),
                            };

                            let display_content = msg.display_text();
                            let raw_text = msg.text.as_deref().or(msg.caption.as_deref()).unwrap_or("").trim();

                            // Сохраняем любое входящее сообщение чата (текст, стикер, медиа) в БД для контекста диалога и анализа споров
                            if !display_content.trim().is_empty() {
                                let _ = self.db.save_chat_message(
                                    &chat_id,
                                    message_id,
                                    from_id,
                                    from_username,
                                    &from_name,
                                    &display_content,
                                    reply_to_msg_id,
                                    reply_to_uid,
                                    reply_to_text.as_deref(),
                                    msg.date,
                                );
                            }

                            let is_ametist = from_username.map_or(false, |u| {
                                u.trim_start_matches('@').eq_ignore_ascii_case("ametist1337")
                            });
                            let is_admin = is_ametist
                                || (!self.admin_ids.is_empty() && self.admin_ids.contains(&from_id));

                            let is_reply_to_bot = msg.reply_to_message.as_ref().map_or(false, |r| {
                                r.from.as_ref().map_or(false, |u| {
                                    u.is_bot
                                        || u.username.as_deref().map_or(false, |un| un.eq_ignore_ascii_case("ew1484_bot"))
                                })
                            });

                            let raw_lower = raw_text.to_lowercase();
                            let mentions_bot = raw_lower.contains("@ew1484_bot")
                                || raw_lower.starts_with("бот,")
                                || raw_lower.starts_with("бот ");

                            let is_dispute_natural = is_dispute_query(raw_text) && (
                                raw_lower.contains("сумарайз")
                                    || raw_lower.contains("суммариз")
                                    || raw_lower.contains("саммари")
                                    || raw_lower.contains("разбери")
                                    || raw_lower.contains("разбор")
                                    || raw_lower.contains("что за спор")
                                    || raw_lower.contains("из-за чего спор")
                                    || raw_lower.contains("из за чего спор")
                                    || raw_lower.contains("кто прав")
                                    || raw_lower.contains("поясни за спор")
                                    || is_reply_to_bot
                                    || mentions_bot
                            );

                            let photo_file_id = msg.get_largest_photo_file_id().or_else(|| {
                                msg.reply_to_message.as_ref().and_then(|r| r.get_largest_photo_file_id())
                            });
                            let has_photo = photo_file_id.is_some();
                            let is_private = msg.chat.chat_type == "private";

                            if is_direct_moderation_command(raw_text) {
                                self.handle_direct_moderation(&chat_id, message_id, from_id, raw_text, is_ametist, is_admin, reply_user).await;
                            } else if raw_text.starts_with('/') {
                                self.handle_command(&chat_id, message_id, raw_text, from_username, from_id, is_admin).await;
                            } else if is_dispute_natural {
                                self.handle_dispute_summary(&chat_id, message_id).await;
                            } else if raw_text.starts_with('!') || is_reply_to_bot || mentions_bot || is_private || (has_photo && is_reply_to_bot) {
                                let query_text = if raw_text.starts_with('!') {
                                    raw_text.to_string()
                                } else if let Some(stripped) = raw_text.strip_prefix("@ew1484_bot") {
                                    format!("!{}", stripped.trim())
                                } else if let Some(stripped) = raw_text.strip_prefix("бот,") {
                                    format!("!{}", stripped.trim())
                                } else if let Some(stripped) = raw_text.strip_prefix("бот ") {
                                    format!("!{}", stripped.trim())
                                } else if !raw_text.is_empty() {
                                    format!("!{}", raw_text)
                                } else if has_photo {
                                    "!Что на прикрепленном изображении?".to_string()
                                } else {
                                    format!("!{}", raw_text)
                                };
                                self.handle_chat_query(&chat_id, &query_text, message_id, is_ametist, is_admin, reply_user, photo_file_id).await;
                            }
                        } else if let Some(emsg) = u.edited_message {
                            let display_content = emsg.display_text();
                            if !display_content.trim().is_empty() {
                                let (reply_to_msg_id, reply_to_uid, reply_to_text) = match emsg.reply_to_message.as_ref() {
                                    Some(r) => {
                                        let r_uid = r.from.as_ref().map(|u| u.id);
                                        let r_text = r.display_text();
                                        let clean_r = if r_text.trim().is_empty() { None } else { Some(r_text) };
                                        (Some(r.message_id), r_uid, clean_r)
                                    }
                                    None => (None, None, None),
                                };
                                let from = emsg.from.as_ref();
                                let from_id = from.map(|f| f.id).unwrap_or(0);
                                let from_username = from.and_then(|f| f.username.as_deref());
                                let from_name = match from {
                                    Some(f) => {
                                        if let Some(ref ln) = f.last_name {
                                            format!("{} {}", f.first_name, ln)
                                        } else {
                                            f.first_name.clone()
                                        }
                                    }
                                    None => "Пользователь".to_string(),
                                };
                                let chat_id = emsg.chat.id.to_string();
                                let _ = self.db.save_chat_message(
                                    &chat_id,
                                    emsg.message_id,
                                    from_id,
                                    from_username,
                                    &from_name,
                                    &display_content,
                                    reply_to_msg_id,
                                    reply_to_uid,
                                    reply_to_text.as_deref(),
                                    emsg.date,
                                );
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
        message_id: i64,
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
                /спор, !спор - детальный разбор последнего конфликта или спора в чате\n\
                !мут <время> (в ответ на сообщение) - замутить нарушителя\n\
                /dz - решение ДЗ на завтра (только @ametist1337)\n\
                /dz <день недели> - решение на любой день (пн, вт, ср, чт, пт, сб, вс)\n\
                /dz сегодня - решение заданий на сегодня\n\
                /status - текущее состояние сервиса и статистика\n\
                /sync - синхронизация переписки через Telethon (админ)\n\
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

                let _ = self.tg.set_message_reaction(chat_id, message_id, Some("👀")).await;
                let _ = self.event_tx.send(BotCommandEvent::SolveForDay {
                    chat_id: chat_id.to_string(),
                    target,
                    reply_to_message_id: Some(message_id),
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
            "/sync" | "/sync@ew1484_bot" | "/синхронизация" => {
                if !is_admin && !is_ametist {
                    let _ = self.tg.send_message(chat_id, "Команда доступна только администратору.").await;
                    return;
                }
                let _ = self.tg.set_message_reaction(chat_id, message_id, Some("👀")).await;
                let _ = self.tg.send_message(chat_id, "Запускаю синхронизацию истории сообщений чата через MTProto Telethon...").await;
                let cid = chat_id.to_string();
                let tg_cl = self.tg.clone();
                tokio::spawn(async move {
                    let out = tokio::process::Command::new("python3")
                        .arg("scripts/sync_chat_history.py")
                        .output()
                        .await;
                    match out {
                        Ok(o) if o.status.success() => {
                            let text = String::from_utf8_lossy(&o.stdout);
                            let clean = text.lines().last().unwrap_or("Синхронизация завершена.");
                            let _ = tg_cl.send_message(&cid, &format!("Синхронизация успешно завершена:\n{}", clean)).await;
                            let _ = tg_cl.set_message_reaction(&cid, message_id, Some("👍")).await;
                        }
                        Ok(o) => {
                            let err = String::from_utf8_lossy(&o.stderr);
                            let _ = tg_cl.send_message(&cid, &format!("Ошибка синхронизации: {}", err)).await;
                            let _ = tg_cl.set_message_reaction(&cid, message_id, None).await;
                        }
                        Err(e) => {
                            let _ = tg_cl.send_message(&cid, &format!("Не удалось вызвать скрипт синхронизации: {}", e)).await;
                            let _ = tg_cl.set_message_reaction(&cid, message_id, None).await;
                        }
                    }
                });
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
            "/спор" | "/dispute" | "/спор@ew1484_bot" | "/dispute@ew1484_bot" => {
                self.handle_dispute_summary(chat_id, message_id).await;
            }
            _ => {}
        }
    }

    async fn handle_direct_moderation(
        &self,
        chat_id: &str,
        message_id: i64,
        from_id: i64,
        text: &str,
        is_ametist: bool,
        is_admin: bool,
        reply_user: Option<(i64, Option<String>, Option<String>)>,
    ) {
        let mut has_permission = is_ametist || is_admin;
        if !has_permission {
            // Проверяем, является ли автор администратором Telegram группы
            if let Ok(member) = self.tg.get_chat_member(chat_id, from_id).await {
                if member.status == "creator" || member.status == "administrator" {
                    has_permission = true;
                }
            }
        }

        if !has_permission {
            let _ = self
                .tg
                .send_message_reply(chat_id, "У вас нет прав для управления модерацией.", Some(message_id))
                .await;
            return;
        }

        let _ = self.tg.set_message_reaction(chat_id, message_id, Some("👀")).await;

        let trimmed = text.trim();
        let lower = trimmed.to_lowercase();
        let first_word = lower.split_whitespace().next().unwrap_or("");

        let is_unmute = first_word.contains("размут") || first_word.contains("unmute");
        let is_ban = first_word.contains("бан") || first_word.contains("ban");

        // 1. Определение целевого пользователя: при ответе на сообщение (reply) цель ВСЕГДА автор сообщения
        let target = if let Some((uid, ref un, ref fn_)) = reply_user {
            (uid, un.clone(), fn_.clone().unwrap_or_else(|| "пользователь".to_string()))
        } else {
            // Если reply нет, ищем @username среди аргументов текста
            let mut found = None;
            for part in trimmed.split_whitespace().skip(1) {
                if part.starts_with('@') {
                    if let Ok((uid, un)) =
                        ModerationManager::resolve_target_user(&self.tg, &self.db, chat_id, Some(part), None).await
                    {
                        found = Some((uid, un, part.to_string()));
                        break;
                    }
                }
            }
            // Если @username не найден, ищем явный числовой ID пользователя (длиннее 5 цифр и не являющийся временем)
            if found.is_none() {
                let words: Vec<&str> = trimmed.split_whitespace().collect();
                for (idx, &part) in words.iter().enumerate().skip(1) {
                    if let Ok(num_id) = part.parse::<i64>() {
                        let next_is_unit = words.get(idx + 1).map_or(false, |next| {
                            let n_low = next.to_lowercase();
                            n_low.starts_with("мин")
                                || n_low.starts_with("час")
                                || n_low.starts_with("сек")
                                || n_low.starts_with("дн")
                                || n_low.starts_with("day")
                                || n_low.starts_with("min")
                                || n_low.starts_with("hour")
                                || n_low.starts_with("sec")
                        });
                        if !next_is_unit && num_id > 100_000 {
                            if let Ok((uid, un)) =
                                ModerationManager::resolve_target_user(&self.tg, &self.db, chat_id, Some(part), None).await
                            {
                                found = Some((uid, un, part.to_string()));
                                break;
                            }
                        }
                    }
                }
            }
            match found {
                Some(t) => t,
                None => {
                    let _ = self
                        .tg
                        .send_message_reply(
                            chat_id,
                            "Ответьте на сообщение нарушителя (reply) или укажите @username / ID.",
                            Some(message_id),
                        )
                        .await;
                    return;
                }
            }
        };

        let target_uid = target.0;
        let target_mention = target.1.as_deref().unwrap_or(&target.2);

        // Защита: нельзя замутить или забанить самого бота
        if target.1.as_deref().map_or(false, |u| u.trim_start_matches('@').eq_ignore_ascii_case("ew1484_bot")) {
            let _ = self.tg.send_message_reply(chat_id, "Невозможно применить ограничения к самому боту.", Some(message_id)).await;
            return;
        }

        if is_unmute {
            let action = crate::telegram::moderation::TelegramActionRequest {
                action: "restore".to_string(),
                target: Some(target_uid.to_string()),
                duration_seconds: None,
                permissions: None,
                restore_previous_after: false,
                ..Default::default()
            };
            match ModerationManager::execute_action(&self.tg, &self.db, chat_id, &action, reply_user, true).await {
                Ok(msg) => {
                    let text = if msg.is_empty() {
                        format!("Ограничения с {} сняты.", target_mention)
                    } else {
                        msg
                    };
                    let _ = self.tg.send_message_reply(chat_id, &text, Some(message_id)).await;
                    let _ = self.tg.set_message_reaction(chat_id, message_id, Some("👍")).await;
                }
                Err(e) => {
                    let _ = self.tg.set_message_reaction(chat_id, message_id, None).await;
                    let _ = self
                        .tg
                        .send_message_reply(chat_id, &format!("Ошибка снятия ограничений: {}", e), Some(message_id))
                        .await;
                }
            }
            return;
        }

        if is_ban {
            let duration = Self::parse_duration_seconds(trimmed);
            let action = crate::telegram::moderation::TelegramActionRequest {
                action: "ban".to_string(),
                target: Some(target_uid.to_string()),
                duration_seconds: duration,
                permissions: None,
                restore_previous_after: false,
                ..Default::default()
            };
            match ModerationManager::execute_action(&self.tg, &self.db, chat_id, &action, reply_user, true).await {
                Ok(msg) => {
                    let text = if msg.is_empty() {
                        format!("Пользователь {} заблокирован.", target_mention)
                    } else {
                        msg
                    };
                    let _ = self.tg.send_message_reply(chat_id, &text, Some(message_id)).await;
                    let _ = self.tg.set_message_reaction(chat_id, message_id, Some("👍")).await;
                }
                Err(e) => {
                    let _ = self.tg.set_message_reaction(chat_id, message_id, None).await;
                    let _ = self
                        .tg
                        .send_message_reply(chat_id, &format!("Ошибка блокировки: {}", e), Some(message_id))
                        .await;
                }
            }
            return;
        }

        // Mute: ограничение на отправку сообщений и медиа
        let is_perm = lower.contains("навсегда") || lower.contains("perm") || lower.contains("бессрочно");
        let (duration_secs, duration_human, restore_prev) = if is_perm {
            (None, "бессрочно".to_string(), false)
        } else {
            let secs = Self::parse_duration_seconds(trimmed).unwrap_or(900); // 15 минут по умолчанию
            (Some(secs), format!("на {}", Self::format_duration_human(secs)), true)
        };

        let perms = crate::telegram::client::ChatPermissions {
            can_send_messages: Some(false),
            can_send_audios: Some(false),
            can_send_documents: Some(false),
            can_send_photos: Some(false),
            can_send_videos: Some(false),
            can_send_video_notes: Some(false),
            can_send_voice_notes: Some(false),
            can_send_polls: Some(false),
            can_send_other_messages: Some(false),
            can_add_web_page_previews: Some(false),
            can_change_info: Some(false),
            can_invite_users: Some(false),
            can_pin_messages: Some(false),
            can_manage_topics: Some(false),
        };

        let action = crate::telegram::moderation::TelegramActionRequest {
            action: "restrict".to_string(),
            target: Some(target_uid.to_string()),
            duration_seconds: duration_secs,
            permissions: Some(perms),
            restore_previous_after: restore_prev,
            ..Default::default()
        };

        match ModerationManager::execute_action(&self.tg, &self.db, chat_id, &action, reply_user, true).await {
            Ok(_) => {
                let confirmation = format!(
                    "Пользователь {} замучен {}.",
                    target_mention, duration_human
                );
                let _ = self.tg.send_message_reply(chat_id, &confirmation, Some(message_id)).await;
                let _ = self.tg.set_message_reaction(chat_id, message_id, Some("👍")).await;
            }
            Err(e) => {
                let _ = self.tg.set_message_reaction(chat_id, message_id, None).await;
                let _ = self
                    .tg
                    .send_message_reply(
                        chat_id,
                        &format!("Ошибка применения мута: {}", e),
                        Some(message_id),
                    )
                    .await;
            }
        }
    }

    async fn handle_dispute_summary(&self, chat_id: &str, message_id: i64) {
        let history = match self.db.get_recent_dispute_context(chat_id, 600) {
            Ok(h) => h,
            Err(e) => {
                let _ = self
                    .tg
                    .send_message_reply(
                        chat_id,
                        &format!("Ошибка загрузки истории сообщений для анализа спора: {}", e),
                        Some(message_id),
                    )
                    .await;
                return;
            }
        };

        if history.len() < 3 {
            let _ = self
                .tg
                .send_message_reply(
                    chat_id,
                    "В базе пока недостаточно сообщений для анализа спора.",
                    Some(message_id),
                )
                .await;
            return;
        }

        let title_map = self.db.get_user_titles_map().unwrap_or_default();
        let mut formatted_history = String::new();
        for msg in &history {
            let un_str = msg.username.as_deref().map(|u| format!(" (@{})", u)).unwrap_or_default();
            let title_info = title_map.get(&msg.user_id).map(|t| format!(" [приписка: {}]", t)).unwrap_or_default();
            let reply_info = if let Some(r_id) = msg.reply_to_message_id {
                let r_user = msg.reply_to_user_id.map(|id| format!(" user:{}", id)).unwrap_or_default();
                format!(" [в ответ на #{}{}]", r_id, r_user)
            } else {
                String::new()
            };
            formatted_history.push_str(&format!(
                "ID #{}: {}{}{}{}: {}\n",
                msg.message_id, msg.full_name, title_info, un_str, reply_info, msg.text
            ));
        }

        let prompt = format!(
            "Ты — предельно лаконичный арбитр и аналитик переписки Telegram-чата.\n\
             ГЛАВНЫЙ ПРИНЦИП: МАКСИМАЛЬНАЯ КРАТКОСТЬ, СУТЬ И НОЛЬ ВОДЫ.\n\
             Перед тобой хронологическая история последних сообщений чата (от старых к новым).\n\
             Твоя задача: найти последний спор или конфликт между участниками, кратко разобрать его и назвать причину.\n\n\
             ПЛАН РАЗБОРА (по 1 краткому предложению на пункт, строго по делу):\n\
             1. Участники спора: имена, приписки и @username.\n\
             2. Первопричина (триггер): конкретное сообщение или повод, из-за которого начался спор.\n\
             3. Суть спора: в чем разногласие.\n\
             4. Тезисы сторон: по одной строке позиции участников.\n\
             5. Итог: чем закончился или продолжается ли.\n\n\
             ПРАВИЛА:\n\
             - Отвечай строго на русском языке.\n\
             - КАТЕГОРИЧЕСКИ ЗАПРЕЩЕНЫ любые смайлики и эмодзи.\n\
             - КАТЕГОРИЧЕСКИ ЗАПРЕЩЕНЫ вводные слова, расшаркивания и длинные абзацы. Сразу суть.\n\
             - Если в истории сообщений нет открытого спора, напиши: 'В недавней переписке споров не зафиксировано.' и одним предложением тему обсуждения.\n\
             - Форматируй ответ в стиле Telegram Markdown (жирный шрифт, списки).\n\n\
             ИСТОРИЯ ПЕРЕПИСКИ В ЧАТЕ:\n\
             \"\"\"\n{}\n\"\"\"",
            formatted_history
        );

        let solver = self.solver.clone();
        let tg = self.tg.clone();
        let db = self.db.clone();
        let chat_id_str = chat_id.to_string();

        let _ = self.tg.set_message_reaction(chat_id, message_id, Some("👀")).await;
        let _ = self.tg.send_chat_action(chat_id, "typing").await;

        tokio::spawn(async move {
            let tg_typing = tg.clone();
            let cid_typing = chat_id_str.clone();
            let typing_task = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;
                    let _ = tg_typing.send_chat_action(&cid_typing, "typing").await;
                }
            });

            let res = solver.ask_chat(&prompt, "gemini-3.8-flash-medium", Some(&chat_id_str)).await;
            typing_task.abort();

            match res {
                Ok(reply) => {
                    let clean = reply.replace("```telegram_action", "").trim().to_string();
                    let sent_id = tg.send_message_reply(&chat_id_str, &clean, Some(message_id)).await.ok();
                    let _ = tg.set_message_reaction(&chat_id_str, message_id, Some("👍")).await;
                    if let Some(sid) = sent_id {
                        let _ = db.save_chat_message(
                            &chat_id_str,
                            sid,
                            0,
                            Some("ew1484_bot"),
                            "МЭШ Бот Помощник",
                            &clean,
                            Some(message_id),
                            None,
                            None,
                            chrono::Utc::now().timestamp(),
                        );
                    }
                }
                Err(e) => {
                    let _ = tg.set_message_reaction(&chat_id_str, message_id, None).await;
                    let _ = tg
                        .send_message_reply(
                            &chat_id_str,
                            &format!("Не удалось разобрать спор: {}", e),
                            Some(message_id),
                        )
                        .await;
                }
            }
        });
    }

    pub fn parse_duration_seconds(input: &str) -> Option<u64> {
        let lower = input.to_lowercase();
        if lower.contains("навсегда") || lower.contains("perm") || lower.contains("бессрочно") {
            return None;
        }

        let words: Vec<&str> = lower.split_whitespace().collect();
        for i in 0..words.len() {
            let w = words[i];
            if let Some(pos) = w.find(|c: char| !c.is_ascii_digit()) {
                if pos > 0 {
                    let num_part = &w[..pos];
                    let unit_part = &w[pos..];
                    if let Ok(num) = num_part.parse::<u64>() {
                        if unit_part.starts_with('м') || unit_part.starts_with('m') {
                            return Some(num * 60);
                        } else if unit_part.starts_with('ч') || unit_part.starts_with('h') {
                            return Some(num * 3600);
                        } else if unit_part.starts_with('д') || unit_part.starts_with('d') {
                            return Some(num * 86400);
                        } else if unit_part.starts_with('с') || unit_part.starts_with('s') {
                            return Some(num);
                        }
                    }
                }
            }

            if let Ok(num) = w.parse::<u64>() {
                if i + 1 < words.len() {
                    let next = words[i + 1];
                    if next.starts_with("мин") || next.starts_with("min") || next.starts_with('м') || next.starts_with('m') {
                        return Some(num * 60);
                    } else if next.starts_with("час") || next.starts_with("hour") || next.starts_with('ч') || next.starts_with('h') {
                        return Some(num * 3600);
                    } else if next.starts_with("ден") || next.starts_with("дн") || next.starts_with("day") || next.starts_with('д') || next.starts_with('d') {
                        return Some(num * 86400);
                    } else if next.starts_with("сек") || next.starts_with("sec") || next.starts_with('с') || next.starts_with('s') {
                        return Some(num);
                    }
                } else if num <= 10_000 {
                    return Some(num * 60);
                }
            }
        }

        None
    }

    pub fn format_duration_human(secs: u64) -> String {
        if secs >= 86400 {
            let days = secs / 86400;
            format!("{} дн.", days)
        } else if secs >= 3600 {
            let hours = secs / 3600;
            let mins = (secs % 3600) / 60;
            if mins > 0 {
                format!("{} ч. {} мин.", hours, mins)
            } else {
                format!("{} ч.", hours)
            }
        } else if secs >= 60 {
            let mins = secs / 60;
            let rem_s = secs % 60;
            if rem_s > 0 {
                format!("{} мин. {} сек.", mins, rem_s)
            } else {
                format!("{} мин.", mins)
            }
        } else {
            format!("{} сек.", secs)
        }
    }

    /// Синхронизация администраторов и титулов (приписок) чата с сохранением в базу данных
    pub async fn sync_chat_administrators_and_titles(&self, chat_id: &str) {
        if let Ok(admins) = self.tg.get_chat_administrators(chat_id).await {
            info!("Синхронизация участников и титулов чата {}: получено {} администраторов", chat_id, admins.len());
            for a in admins {
                let title = a.custom_title.as_deref().map(|t| t.trim()).filter(|t| !t.is_empty());
                let _ = self.db.save_seen_user(
                    a.user.id,
                    a.user.username.as_deref(),
                    &a.user.first_name,
                    a.user.last_name.as_deref(),
                    title,
                );
            }
        }
    }

    /// Построение блока привязки участников группы (ID, приписки / кастомные титулы, юзернеймы, имена) для промпта
    pub async fn build_group_roster_context(&self, chat_id: &str) -> (String, std::collections::HashMap<i64, String>) {
        // 1. Синхронизируем через Telegram API
        self.sync_chat_administrators_and_titles(chat_id).await;

        // 2. Получаем карту титулов из базы
        let mut titles_map = self.db.get_user_titles_map().unwrap_or_default();

        // 3. Эталонный реестр участников группы класса
        let known_roster = [
            (8193800816i64, Some("Ametist1337"), "Ametist1337", "семён", "administrator (Владелец / доверенный администратор)"),
            (8026023851i64, None, "Оп оп сприподнизовское движение", "имран", "administrator"),
            (8603102299i64, Some("asas19902"), "игорь", "игорь", "administrator"),
            (8478374870i64, Some("oracle_edits"), "zenty | bskt | #online #sf🪬", "демир", "administrator"),
            (7575524932i64, Some("Grigosha200"), "Grigosha 14/12", "тммофей", "administrator (Тимофей)"),
            (7466301796i64, Some("Trlcly"), "Tricly", "андрей", "administrator"),
            (7679243922i64, Some("sdayzx"), "wᴀlly🎰", "айдина", "administrator"),
            (7838916464i64, None, ".", "ассоль", "administrator"),
            (8050447344i64, Some("snova_obognal"), "agg✌🏿", "агния", "administrator"),
            (7076530728i64, None, "Alen", "ален", "administrator"),
            (6274940965i64, None, "PEGA", "арсений", "administrator"),
            (7339405598i64, Some("DeShek67"), "Варя", "варя", "administrator"),
            (8732018819i64, Some("bWRmfTID"), "💋", "лиза", "administrator"),
            (1665877840i64, None, "васька", "василя", "administrator"),
            (5393284763i64, Some("gussiavaffe"), "V", "варя", "administrator"),
            (5278759499i64, Some("maksimpolyakov1234"), "Максим", "максим", "administrator"),
            (8469803740i64, Some("AHzkto67"), "Eudh", "асыл", "administrator"),
            (6731822991i64, Some("yeo3ange"), "🅴 чęлķāçтâя", "алина", "administrator"),
            (8566127992i64, Some("zxcdota2kitty"), "жыртракс мод", "лох ебать", "creator (Создатель группы)"),
            (8127794104i64, Some("EW1484_bot"), "автогдзешка", "", "bot (Ты сам)"),
        ];

        for (uid, un_opt, fn_, title, _role) in &known_roster {
            if !title.is_empty() {
                titles_map.entry(*uid).or_insert_with(|| title.to_string());
                let _ = self.db.save_seen_user(*uid, *un_opt, fn_, None, Some(title));
            }
        }

        let mut roster_text = String::from("СПИСОК И ПРИВЯЗКА УЧАСТНИКОВ ГРУППЫ (ID, ПРИПИСКИ / ТИТУЛЫ, ЮЗЕРНЕЙМЫ И ИМЕНА):\n\
        Каждый участник группы имеет уникальный Telegram ID, отображаемое имя (ник), юзернейм и кастомный титул (официальную приписку администратора в чате).\n\
        Приписка/титул — это настоящее имя или статус человека в коллективе. Когда в чате называют человека по имени (например, Имран, Игорь, Демир, Тимофей, Семён, Асыл, Варя, Алина, Арсений, Лиза, Василя, Ассоль, Агния, Ален, Андрей, Айдина, Максим), используй эту таблицу:\n");

        for (uid, un_opt, fn_, title, role) in &known_roster {
            let un_str = un_opt.map(|u| format!("@{}", u)).unwrap_or_else(|| "нет".to_string());
            let title_str = if title.is_empty() { "нет".to_string() } else { format!("\"{}\"", title) };
            roster_text.push_str(&format!("• ID: {} | Приписка (титул): {} | Имя: {} | Username: {} | Роль: {}\n", uid, title_str, fn_, un_str, role));
        }

        // Если в базе есть другие пользователи с титулами, добавляем их
        if let Ok(known_users) = self.db.get_all_known_users_in_chat(chat_id) {
            for (uid, un_opt, full_name) in known_users {
                if !known_roster.iter().any(|r| r.0 == uid) {
                    let title = titles_map.get(&uid).map(|t| format!("\"{}\"", t)).unwrap_or_else(|| "нет".to_string());
                    let un_str = un_opt.map(|u| format!("@{}", u)).unwrap_or_else(|| "нет".to_string());
                    roster_text.push_str(&format!("• ID: {} | Приписка (титул): {} | Имя: {} | Username: {}\n", uid, title, full_name, un_str));
                }
            }
        }

        roster_text.push('\n');
        (roster_text, titles_map)
    }

    async fn handle_chat_query(
        &self,
        chat_id: &str,
        raw_text: &str,
        message_id: i64,
        is_ametist: bool,
        is_admin: bool,
        reply_user: Option<(i64, Option<String>, Option<String>)>,
        photo_file_id: Option<String>,
    ) {
        let mut query = raw_text.trim_start_matches('!').trim().to_string();
        if query.is_empty() {
            if photo_file_id.is_some() {
                query = "Что изображено на прикрепленном изображении? Разбери задачу или ответь по картинке.".to_string();
            } else {
                return;
            }
        }

        info!("Получен запрос к AI чату (msg_id: {}): '{}'", message_id, query);

        let _ = self.tg.set_message_reaction(chat_id, message_id, Some("👀")).await;
        let _ = self.tg.send_chat_action(chat_id, "typing").await;

        // Проверяем, не является ли запрос просьбой разобрать спор в чате
        if is_dispute_query(&query) {
            self.handle_dispute_summary(chat_id, message_id).await;
            return;
        }

        // Скачиваем прикрепленное фото из Telegram во временный файл, если есть
        let mut downloaded_photo: Option<String> = None;
        if let Some(ref fid) = photo_file_id {
            let clean_chat_id = chat_id.replace('-', "m");
            let temp_path = format!(
                "/tmp/tg_photo_{}_{}_{}.jpg",
                clean_chat_id,
                message_id,
                chrono::Utc::now().timestamp_millis()
            );
            match self.tg.download_file_to_path(fid, &temp_path).await {
                Ok(_) => {
                    info!("Успешно скачано прикрепленное фото в {}", temp_path);
                    downloaded_photo = Some(temp_path);
                }
                Err(e) => {
                    warn!("Не удалось скачать фото из Telegram (file_id: {}): {}", fid, e);
                }
            }
        }

        let user_query_formatted = if let Some(ref p) = downloaded_photo {
            format!("*прикреплено: {}*\n{}", p, query)
        } else {
            query.clone()
        };

        let is_school = {
            let low = query.to_lowercase();
            low.contains("дз") || low.contains("дом") || low.contains("урок")
            || low.contains("расписан") || low.contains("задан") || low.contains("звонк")
            || low.contains("кабинет") || low.contains("школ") || low.contains("предмет")
            || low.contains("понедельник") || low.contains("вторник") || low.contains("сред")
            || low.contains("четверг") || low.contains("пятниц") || low.contains("суббот")
            || low.contains("завтра") || low.contains("сегодня") || low.contains("послезавтра")
            || low.contains("учител")
        };

        let today = chrono::Local::now().date_naive();
        let from_date = today - chrono::Duration::days(7);
        let to_date = today + chrono::Duration::days(7);

        let (schedule_context, mesh_context, solutions_context) = if is_school {
            let mesh_client = self.mesh.clone();
            let (live_hws_res, events_res) = tokio::join!(
                mesh_client.get_homeworks(from_date, to_date),
                mesh_client.get_events(from_date, to_date)
            );

            let live_hws = live_hws_res.unwrap_or_default();
            let events = events_res.unwrap_or_default();
            let sched = format_schedule_context(&events);

            let mut mc = String::new();
            if !live_hws.is_empty() {
                mc.push_str(&format!(
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
                    mc.push_str(&format!("• [{}] {}{}: {}\n", date, sub, is_done_str, desc));
                }
                mc.push('\n');
            }

            let recent_solutions = self.db.get_recent_solutions(5).unwrap_or_default();
            let mut sc = String::new();
            if !recent_solutions.is_empty() {
                sc.push_str("БАЗА ГОТОВЫХ РЕШЕНИЙ ДОМАШНИХ ЗАДАНИЙ:\n");
                for (sub, date, desc, sol) in recent_solutions {
                    sc.push_str(&format!("--- ПРЕДМЕТ: {} (Срок: {}) ---\n", sub, date));
                    if !desc.trim().is_empty() {
                        sc.push_str(&format!("Задание: {}\n", desc.trim()));
                    }
                    let short_sol: String = sol.chars().take(400).collect();
                    sc.push_str(&format!("Решение:\n{}\n\n", short_sol));
                }
            }
            (sched, mc, sc)
        } else {
            (String::new(), String::new(), String::new())
        };

        // Загружаем привязку участников группы и карту титулов
        let (roster_context, title_map) = self.build_group_roster_context(chat_id).await;

        // Загружаем недавний контекст истории диалога (40 последних сообщений для мгновенного ответа)
        let chat_history = self.db.get_chat_history(chat_id, 40, 30_000).unwrap_or_default();
        let mut chat_history_context = String::new();
        if !chat_history.is_empty() {
            chat_history_context.push_str("ИСТОРИЯ ПЕРЕПИСКИ В ЧАТЕ (недавний контекст диалога):\n");
            for m in &chat_history {
                if m.message_id == message_id {
                    continue;
                }
                let un = m.username.as_deref().map(|u| format!(" (@{})", u)).unwrap_or_default();
                let title_hint = title_map.get(&m.user_id).map(|t| format!(" [приписка: {}]", t)).unwrap_or_default();
                let r_hint = if let Some(r_id) = m.reply_to_message_id {
                    format!(" [в ответ на #{}]", r_id)
                } else {
                    String::new()
                };
                chat_history_context.push_str(&format!("[#{}] {}{}{}{} (id: {}): {}\n", m.message_id, m.full_name, title_hint, un, r_hint, m.user_id, m.text));
            }
            chat_history_context.push('\n');
        }

        let is_auth = is_ametist || is_admin;
        let mod_instructions = if is_auth {
            let target_hint = if let Some((uid, ref un, ref fn_)) = reply_user {
                format!(
                    "Команда вызвана в ответ (reply) на сообщение участника: id={}, username={:?}, first_name={:?}.\n\
                     Если действие направлено на него, обязательно пиши \"target\": \"reply\".\n",
                    uid, un, fn_
                )
            } else {
                String::new()
            };

            format!(
                "ПРАВА УПРАВЛЕНИЯ ЧАТОМ И МОДЕРАЦИИ (ТОЛЬКО ДЛЯ @ametist1337):\n\
                 Запрос поступил от владельца/администратора (@ametist1337).\n\
                 {}\
                 Ты имеешь право мутить, банить, менять любые разрешения (сообщения, музыка/аудио, медиа, стикеры и др.), назначать администраторов или восстанавливать права участников чата.\n\
                 Если пользователь @ametist1337 просит замутить, ограничить, забанить, выдать админку, снять права или вернуть права (например: '!верни голос для Имран', '!@user на 5 минут убрать все разрешения кроме музыки, потом вернуть все прежние', '!дай всем в группе разрешения как у @gussiavaffe, с админкой и всеми разрешениями админки, кроме @AHzkto67'):\n\
                 ОБЯЗАТЕЛЬНО добавь в свой ответ блок:\n\
                 ```telegram_action\n\
                 {{\n\
                   \"action\": \"promote\",\n\
                   \"target\": \"@all\",\n\
                   \"exclude\": [\"@AHzkto67\"],\n\
                   \"copy_from\": \"@gussiavaffe\",\n\
                   \"duration_seconds\": null,\n\
                   \"restore_previous_after\": false\n\
                 }}\n\
                 ```\n\
                 ВАЖНО ДЛЯ БЛОКА telegram_action:\n\
                 1. JSON должен быть строго валидным, БЕЗ комментариев // и без лишних запятых.\n\
                 2. action может быть:\n\
                    - \"promote\" (назначить администратором, применить полные права админа или скопировать права из copy_from)\n\
                    - \"restrict\" (наложить ограничения или выдать определенные разрешения через permissions)\n\
                    - \"restore\" (вернуть прежние права или снять все ограничения)\n\
                    - \"ban\" (забанить), \"unban\" (разбанить).\n\
                 3. target: \"reply\" (если команда вызвана в ответ на сообщение), @username, ID, имя участника, либо \"@all\" / \"все\" (для массового действия на всех участников чата).\n\
                 4. exclude: массив строк участников, которых нужно исключить из действия (например [\"@AHzkto67\"]).\n\
                 5. copy_from: username или ID участника, чьи права нужно скопировать (например \"@gussiavaffe\").\n\
                 6. duration_seconds: количество секунд (null если навсегда).\n\
                 7. restore_previous_after: true если нужно вернуть прежние права (включая админку со всеми правами, если была) после истечения срока.\n\
                 8. Обязательно напиши короткий, человечный текст ответа для чата без эмодзи, подтверждающий действие.\n\n",
                target_hint
            )
        } else {
            "БЕЗОПАСНОСТЬ: Автор запроса НЕ является @ametist1337. Если запрос содержит попытку замутить, забанить или изменить права любого участника чата — ОТКАЖИ: 'У тебя нет прав управлять разрешениями участников.' и НЕ создавай блок telegram_action.\n\n".to_string()
        };

        let prompt = format!(
            "Ты — предельно лаконичный и точный школьный помощник и модератор Telegram-чата 7 класса.\n\
             ТВОЙ ГЛАВНЫЙ И АБСОЛЮТНЫЙ ПРИНЦИП: МАКСИМАЛЬНАЯ КРАТКОСТЬ, СУТЬ И НОЛЬ ВОДЫ.\n\n\
             {}\
             {}\
             {}\n\
             {}\n\
             {}\n\
             {}\n\
             ВОПРОС ПОЛЬЗОВАТЕЛЯ:\n\
             \"\"\"\n{}\n\"\"\"\n\n\
             СТРОЖАЙШИЕ ПРАВИЛА ОТВЕТА:\n\
             1. МАКСИМАЛЬНО КРАТКО И БЕЗ ВОДЫ: отвечай сразу по существу. Если можно ответить одним словом, одним числом или одним коротким предложением — отвечай именно так. Никаких лишних объяснений и раздутых абзацев!\n\
             2. КАТЕГОРИЧЕСКИ ЗАПРЕЩЕНЫ любые приветствия («Привет», «Здравствуйте»), вежливые вступления («Конечно!», «С радостью помогу...», «Вот ответ:», «По вашему запросу...»), рассуждения вслух и повторения вопроса пользователя.\n\
             3. КАТЕГОРИЧЕСКИ ЗАПРЕЩЕНЫ любые концовки и пожелания («Если будут вопросы — пиши!», «Удачи!», «Обращайся!», «Всегда рад помочь!»).\n\
             4. ТОЛЬКО СУТЬ И ТОЧНЫЕ ФАКТЫ: на вопрос об уроке/расписании пиши только время, предмет и кабинет. На вопрос о ДЗ — только суть задания. На вопрос об участнике — только приписку, юзернейм/ID и факт.\n\
             5. КАТЕГОРИЧЕСКИ ЗАПРЕЩЕНЫ любые эмодзи и смайлики.\n\
             6. ЗАПРЕЩЕНО использовать LaTeX ($$, $, \\frac, \\circ). Пиши обычным текстом, градусы знаком ° или °C.\n\
             7. Форматируй ответ в стиле Telegram Markdown: *жирный шрифт*, `моноширинный текст`, краткие списки.\n\
             8. Учитывай контекст предыдущего диалога: помни, о чем шла речь ранее в переписке, не начинай каждый раз с чистого листа.\n\
             9. Если спрашивают про дни недели или даты — сопоставь с текущей датой ({}) и расписанием МЭШ.\n\
             10. Используй список участников и их приписки: понимай, к кому обращаются по имени (Имран, Игорь, Асыл, Варя, Семён, Демир, Тимофей, Алина и др.).\n\
             11. СТРОГИЙ РЕЖИМ РАБОТЫ И ИНСТРУМЕНТЫ:\n\
                 - КАТЕГОРИЧЕСКИ ЗАПРЕЩЕНО вызывать любые консольные инструменты, bash, run_command, git, ls, grep или читать файлы проекта для обычных вопросов! На любые обычные сообщения, общение, шутки, мемы, школьные вопросы, приветствия и реакцию на фото — ТЫ ОБЯЗАН СРАЗУ ВЫДАТЬ ПРЯМОЙ ЛАКОНИЧНЫЙ ТЕКСТ ОТВЕТА БЕЗ ЕДИНОГО ТУЛА!\n\
                 - Использовать терминал и Python разрешено ИСКЛЮЧИТЕЛЬНО для глубоких аналитических запросов ('топ сообщений', 'статистика', 'облако слов', 'график', 'sql'):\n\
                   • `python3 scripts/bot_api.py top 20` (выводит красивый рейтинг участников по сообщениям)\n\
                   • `python3 scripts/bot_api.py stats` (общая статистика чата)\n\
                   • `python3 scripts/bot_api.py sync` (скачивает свежую историю переписки через Telethon)\n\
                   • `python3 scripts/generate_swear_visuals.py --send` (строит график и облако слов ненормативной лексики по всей базе и отправляет в чат)\n\
                   • `python3 scripts/bot_api.py sql \"SELECT ...\"` (прямые SQL-запросы к chat_messages).\n\
                 - Переменные окружения: `$TELEGRAM_BOT_TOKEN`, `$TELEGRAM_CHAT_ID`, `$DB_PATH`.\n\
             12. РАБОТА С ИЗОБРАЖЕНИЯМИ:\n\
                 - Если в вопросе пользователя есть строка `*прикреплено: /путь/к/файлу.jpg*`, значит к запросу прикреплена картинка. Файл доступен на диске сервера: ты можешь проанализировать его, использовать скрипты на Python (например, PIL/tesseract) или консольные утилиты.\n\
                 - Если ты хочешь отправить в чат сгенерированное изображение/график/чертеж: сохрани картинку в файл (например, в `/tmp/plot.png`) и укажи в своем ответе: `*прикреплено: /tmp/plot.png*` (бот автоматически отправит фото в чат и удалит временный файл), либо отправь напрямую вызовом `python3 scripts/bot_api.py send_photo /tmp/plot.png`.\n\
             13. Если выполняется модерация владельцем (@ametist1337) — вердикт в 1 короткую фразу плюс блок ```telegram_action```.",
            roster_context,
            mod_instructions,
            chat_history_context,
            schedule_context,
            mesh_context,
            solutions_context,
            user_query_formatted,
            today.format("%d.%m.%Y (%A)")
        );

        let requires_tools = {
            let q_lower = query.to_lowercase();
            q_lower.contains("топ") || q_lower.contains("статистик") || q_lower.contains("скрипт")
            || q_lower.contains("график") || q_lower.contains("облако") || q_lower.contains("sql")
            || q_lower.contains("посчитай") || q_lower.contains("выгрузи") || q_lower.contains("пропарси")
            || q_lower.contains("баз") || q_lower.contains("мат")
        };

        let target_model = if requires_tools {
            "gemini-3.8-flash-medium"
        } else {
            "gemini-3.8-flash-low"
        };

        let tg_client = self.tg.clone();
        let solver = self.solver.clone();
        let db_cl = self.db.clone();
        let chat_id_str = chat_id.to_string();
        let query_clone = query.to_string();
        let temp_photo_cleanup = downloaded_photo.clone();

        tokio::spawn(async move {
            let tg_typing = tg_client.clone();
            let cid_typing = chat_id_str.clone();
            let typing_task = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;
                    let _ = tg_typing.send_chat_action(&cid_typing, "typing").await;
                }
            });

            let res = solver.ask_chat(&prompt, target_model, Some(&chat_id_str)).await;
            typing_task.abort();

            // Удаляем временный файл входящего фото после ответа модели
            if let Some(ref p) = temp_photo_cleanup {
                let _ = tokio::fs::remove_file(p).await;
            }

            match res {
                Ok(reply) => {
                    let (action_opt, raw_clean_reply) = ModerationManager::extract_action(&reply);
                    let (clean_reply, outgoing_photo_opt) = {
                        let (cleaned, direct_photo) = extract_attached_photo(&raw_clean_reply);
                        if direct_photo.is_some() {
                            (cleaned, direct_photo)
                        } else {
                            let (visual_clean, visual_img) = VisualRenderer::process_visuals_and_strip(&cleaned, None);
                            if let Some(v_path) = visual_img {
                                (visual_clean, Some(v_path.to_string_lossy().to_string()))
                            } else {
                                (cleaned, None)
                            }
                        }
                    };

                    if let Some(action) = action_opt {
                        match ModerationManager::execute_action(
                            &tg_client,
                            &db_cl,
                            &chat_id_str,
                            &action,
                            reply_user.clone(),
                            is_auth,
                        ).await {
                            Ok(exec_msg) => {
                                let final_msg = if clean_reply.trim().is_empty() {
                                    exec_msg
                                } else {
                                    clean_reply
                                };
                                let sent_id = tg_client.send_message_reply(&chat_id_str, &final_msg, Some(message_id)).await.ok();
                                let _ = tg_client.set_message_reaction(&chat_id_str, message_id, Some("👍")).await;
                                if let Some(sid) = sent_id {
                                    let _ = db_cl.save_chat_message(
                                        &chat_id_str,
                                        sid,
                                        0,
                                        Some("ew1484_bot"),
                                        "МЭШ Бот Помощник",
                                        &final_msg,
                                        Some(message_id),
                                        reply_user.as_ref().map(|u| u.0),
                                        Some(&query_clone),
                                        chrono::Utc::now().timestamp(),
                                    );
                                }
                            }
                            Err(e) => {
                                let _ = tg_client.set_message_reaction(&chat_id_str, message_id, None).await;
                                let err_msg = format!("Ошибка выполнения модерации: {}", e);
                                let _ = tg_client.send_message_reply(&chat_id_str, &err_msg, Some(message_id)).await;
                            }
                        }
                    } else {
                        let final_msg = if clean_reply.trim().is_empty() {
                            if outgoing_photo_opt.is_some() {
                                String::new()
                            } else {
                                "Команда обработана, но ответ пустой.".to_string()
                            }
                        } else {
                            clean_reply
                        };

                        let mut sent_via_photo = false;
                        if let Some(ref photo_path) = outgoing_photo_opt {
                            if std::path::Path::new(photo_path).exists() {
                                let cap = if final_msg.is_empty() { None } else { Some(final_msg.as_str()) };
                                match tg_client.send_photo(&chat_id_str, photo_path, cap, Some(message_id)).await {
                                    Ok(pid) => {
                                        sent_via_photo = true;
                                        let _ = tokio::fs::remove_file(photo_path).await;
                                        let _ = tg_client.set_message_reaction(&chat_id_str, message_id, Some("👍")).await;
                                        let _ = db_cl.save_chat_message(
                                            &chat_id_str,
                                            pid,
                                            0,
                                            Some("ew1484_bot"),
                                            "МЭШ Бот Помощник",
                                            if final_msg.is_empty() { "[Фото]" } else { &final_msg },
                                            Some(message_id),
                                            reply_user.as_ref().map(|u| u.0),
                                            Some(&query_clone),
                                            chrono::Utc::now().timestamp(),
                                        );
                                    }
                                    Err(e) => {
                                        warn!("Не удалось отправить прикрепленное фото в ответ: {}", e);
                                        let _ = tokio::fs::remove_file(photo_path).await;
                                    }
                                }
                            }
                        }

                        if !sent_via_photo {
                            let sent_id = tg_client.send_message_reply(&chat_id_str, &final_msg, Some(message_id)).await.ok();
                            let _ = tg_client.set_message_reaction(&chat_id_str, message_id, Some("👍")).await;
                            if let Some(sid) = sent_id {
                                let _ = db_cl.save_chat_message(
                                    &chat_id_str,
                                    sid,
                                    0,
                                    Some("ew1484_bot"),
                                    "МЭШ Бот Помощник",
                                    &final_msg,
                                    Some(message_id),
                                    reply_user.as_ref().map(|u| u.0),
                                    Some(&query_clone),
                                    chrono::Utc::now().timestamp(),
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tg_client.set_message_reaction(&chat_id_str, message_id, None).await;
                    warn!("Ошибка генерации ответа в AI чате: {:#}", e);
                    let _ = tg_client
                        .send_message_reply(
                            &chat_id_str,
                            &format!("Не удалось получить ответ: {:#}", e),
                            Some(message_id),
                        )
                        .await;
                }
            }
        });
    }
}

pub fn extract_attached_photo(text: &str) -> (String, Option<String>) {
    let prefix = "*прикреплено:";
    let suffix = "*";
    if let Some(start_idx) = text.find(prefix) {
        let after = &text[start_idx + prefix.len()..];
        if let Some(end_idx) = after.find(suffix) {
            let path = after[..end_idx].trim().to_string();
            let mut cleaned = text.to_string();
            let full_len = prefix.len() + end_idx + suffix.len();
            cleaned.replace_range(start_idx..start_idx + full_len, "");
            return (cleaned.trim().to_string(), Some(path));
        }
    }

    let prefix_plain = "прикреплено:";
    if let Some(start_idx) = text.to_lowercase().find(prefix_plain) {
        let after = &text[start_idx + prefix_plain.len()..];
        let end_idx = after.find('\n').unwrap_or(after.len());
        let path = after[..end_idx].trim().trim_matches('*').trim().to_string();
        if path.starts_with('/') && (std::path::Path::new(&path).exists() || path.ends_with(".png") || path.ends_with(".jpg")) {
            let mut cleaned = text.to_string();
            let full_len = prefix_plain.len() + end_idx;
            cleaned.replace_range(start_idx..start_idx + full_len, "");
            return (cleaned.trim().to_string(), Some(path));
        }
    }

    (text.to_string(), None)
}

pub fn is_direct_moderation_command(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    let cmd = lower.split_whitespace().next().unwrap_or("");
    let prefixes = [
        "!мут", "/мут", "!mute", "/mute",
        "!размут", "/размут", "!unmute", "/unmute",
        "!бан", "/бан", "!ban", "/ban",
    ];

    for prefix in &prefixes {
        if cmd == *prefix {
            return true;
        }
        if let Some(rest) = cmd.strip_prefix(prefix) {
            if let Some(first_char) = rest.chars().next() {
                if !first_char.is_alphabetic() {
                    return true;
                }
            }
        }
    }
    false
}

pub fn is_dispute_query(query: &str) -> bool {
    let lower = query.to_lowercase();
    lower.contains("спор")
        || lower.contains("споре")
        || lower.contains("спорили")
        || lower.contains("спорят")
        || lower.contains("конфликт")
        || lower.contains("ругань")
        || lower.contains("срач")
        || lower.contains("диспут")
        || lower.contains("суммариз")
        || lower.contains("сумарайз")
        || lower.contains("саммари")
        || lower.contains("разбор переписки")
        || lower.contains("что произошло в чате")
        || lower.contains("из-за чего спорили")
        || lower.contains("из за чего спор")
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

    #[test]
    fn test_direct_moderation_commands() {
        assert!(is_direct_moderation_command("!мут 15 минут"));
        assert!(is_direct_moderation_command("!мут 2 часа"));
        assert!(is_direct_moderation_command("!мут 1 день"));
        assert!(is_direct_moderation_command("!мут 30с"));
        assert!(is_direct_moderation_command("!мут10м"));
        assert!(is_direct_moderation_command("!мут"));
        assert!(is_direct_moderation_command("!бан"));
        assert!(is_direct_moderation_command("/бан"));
        assert!(is_direct_moderation_command("!размут"));
        assert!(is_direct_moderation_command("!unmute"));
        assert!(!is_direct_moderation_command("просто текст"));
        assert!(!is_direct_moderation_command("!спор"));
        assert!(!is_direct_moderation_command("!банк"));
        assert!(!is_direct_moderation_command("!банан"));
        assert!(!is_direct_moderation_command("!мутант"));
        assert!(!is_direct_moderation_command("!мутить воду"));
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(BotHandler::parse_duration_seconds("15 минут"), Some(900));
        assert_eq!(BotHandler::parse_duration_seconds("15 мин"), Some(900));
        assert_eq!(BotHandler::parse_duration_seconds("15m"), Some(900));
        assert_eq!(BotHandler::parse_duration_seconds("15м"), Some(900));
        assert_eq!(BotHandler::parse_duration_seconds("2 часа"), Some(7200));
        assert_eq!(BotHandler::parse_duration_seconds("2ч"), Some(7200));
        assert_eq!(BotHandler::parse_duration_seconds("2h"), Some(7200));
        assert_eq!(BotHandler::parse_duration_seconds("1 день"), Some(86400));
        assert_eq!(BotHandler::parse_duration_seconds("1д"), Some(86400));
        assert_eq!(BotHandler::parse_duration_seconds("1d"), Some(86400));
        assert_eq!(BotHandler::parse_duration_seconds("45 сек"), Some(45));
        assert_eq!(BotHandler::parse_duration_seconds("45с"), Some(45));
        assert_eq!(BotHandler::parse_duration_seconds("15"), Some(900));
        assert_eq!(BotHandler::parse_duration_seconds("123456789"), None); // Telegram ID не должен считаться длительностью
        assert_eq!(BotHandler::parse_duration_seconds("навсегда"), None);
        assert_eq!(BotHandler::parse_duration_seconds("непонятно что"), None);
    }

    #[test]
    fn test_format_duration_human() {
        assert_eq!(BotHandler::format_duration_human(900), "15 мин.");
        assert_eq!(BotHandler::format_duration_human(7200), "2 ч.");
        assert_eq!(BotHandler::format_duration_human(86400), "1 дн.");
        assert_eq!(BotHandler::format_duration_human(45), "45 сек.");
        assert_eq!(BotHandler::format_duration_human(3660), "1 ч. 1 мин.");
    }

    #[test]
    fn test_is_dispute_query() {
        assert!(is_dispute_query("!спор"));
        assert!(is_dispute_query("/спор"));
        assert!(is_dispute_query("разбери спор в чате"));
        assert!(is_dispute_query("суммаризируй последний спор"));
        assert!(is_dispute_query("сумарайз последнего спора в чате"));
        assert!(is_dispute_query("сделай саммари спора"));
        assert!(is_dispute_query("кто прав в этом споре?"));
        assert!(is_dispute_query("из-за чего спорили"));
        assert!(!is_dispute_query("Привет, как решить задачу 5?"));
        assert!(!is_dispute_query("что задали на завтра"));
    }

    #[test]
    fn test_message_display_text() {
        use crate::telegram::client::TelegramMessage;
        let mut msg = TelegramMessage {
            message_id: 1,
            from: None,
            chat: crate::telegram::client::TelegramChat { id: 1, title: None, username: None, chat_type: "group".to_string() },
            text: None,
            caption: None,
            date: 0,
            reply_to_message: None,
            photo: None,
            sticker: Some(serde_json::json!({})),
            voice: None,
            audio: None,
            document: None,
            video: None,
            video_note: None,
        };
        assert_eq!(msg.display_text(), "[Стикер]");

        msg.sticker = None;
        msg.photo = Some(serde_json::json!({}));
        assert_eq!(msg.display_text(), "[Фото]");

        msg.caption = Some("Посмотри на это".to_string());
        assert_eq!(msg.display_text(), "Посмотри на это");

        msg.text = Some("Обычный текст".to_string());
        assert_eq!(msg.display_text(), "Обычный текст");
    }

    #[test]
    fn test_extract_attached_photo() {
        let (text, photo) = extract_attached_photo("Вот график функции:\n*прикреплено: /tmp/plot_1.png*");
        assert_eq!(photo, Some("/tmp/plot_1.png".to_string()));
        assert_eq!(text, "Вот график функции:");

        let (text2, photo2) = extract_attached_photo("*прикреплено: /tmp/img.jpg*\nРешение задания готово.");
        assert_eq!(photo2, Some("/tmp/img.jpg".to_string()));
        assert_eq!(text2, "Решение задания готово.");

        let (text3, photo3) = extract_attached_photo("Обычный ответ без картинки");
        assert_eq!(photo3, None);
        assert_eq!(text3, "Обычный ответ без картинки");
    }
}


