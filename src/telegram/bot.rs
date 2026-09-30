use chrono::{Datelike, Timelike};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::db::Database;
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
}

impl BotHandler {
    pub fn new(
        tg: TelegramClient,
        mesh: MeshClient,
        db: Database,
        admin_ids: Vec<i64>,
        event_tx: mpsc::Sender<BotCommandEvent>,
    ) -> Self {
        Self {
            tg,
            mesh,
            db,
            admin_ids,
            event_tx,
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
            _ => {}
        }
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

