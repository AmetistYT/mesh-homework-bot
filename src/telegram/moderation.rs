use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{error, info, warn};

use crate::db::Database;
use crate::telegram::client::{ChatAdminRights, ChatPermissions, TelegramClient};

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TelegramActionRequest {
    pub action: String, // "restrict", "ban", "unban", "restore"
    pub target: Option<String>, // "@username", "123456", "reply"
    pub duration_seconds: Option<u64>,
    pub permissions: Option<ChatPermissions>,
    #[serde(default)]
    pub restore_previous_after: bool,
}

pub struct ModerationManager;

impl ModerationManager {
    /// Извлечение блока telegram_action из ответа модели и очистка текста для пользователя
    pub fn extract_action(text: &str) -> (Option<TelegramActionRequest>, String) {
        let action_tag = "```telegram_action";
        let json_tag = "```json";

        let find_block = |tag: &str| -> Option<(usize, usize, String)> {
            if let Some(start_idx) = text.find(tag) {
                let content_start = start_idx + tag.len();
                if let Some(end_offset) = text[content_start..].find("```") {
                    let end_idx = content_start + end_offset + 3;
                    let json_str = text[content_start..content_start + end_offset].trim().to_string();
                    return Some((start_idx, end_idx, json_str));
                }
            }
            None
        };

        if let Some((start, end, json_str)) = find_block(action_tag) {
            if let Ok(action) = serde_json::from_str::<TelegramActionRequest>(&json_str) {
                let mut clean_text = String::new();
                clean_text.push_str(&text[..start]);
                clean_text.push_str(&text[end..]);
                return (Some(action), clean_text.trim().to_string());
            }
        }

        // Поиск в generic ```json если там есть "action":
        if let Some((start, end, json_str)) = find_block(json_tag) {
            if json_str.contains("\"action\"") {
                if let Ok(action) = serde_json::from_str::<TelegramActionRequest>(&json_str) {
                    let mut clean_text = String::new();
                    clean_text.push_str(&text[..start]);
                    clean_text.push_str(&text[end..]);
                    return (Some(action), clean_text.trim().to_string());
                }
            }
        }

        (None, text.to_string())
    }

    /// Определение целевого user_id по username, reply или ID
    pub async fn resolve_target_user(
        tg: &TelegramClient,
        db: &Database,
        chat_id: &str,
        target_str: Option<&str>,
        reply_user: Option<(i64, Option<String>)>,
    ) -> Result<(i64, Option<String>)> {
        let t = target_str.unwrap_or("").trim();

        // 1. Если явно указан reply или target пустой при наличии reply
        if (t.is_empty() || t.eq_ignore_ascii_case("reply")) && reply_user.is_some() {
            return Ok(reply_user.unwrap());
        }

        // 2. Если передан числовой ID
        if let Ok(num_id) = t.parse::<i64>() {
            return Ok((num_id, None));
        }

        // 3. Если передан @username
        if t.starts_with('@') || (!t.is_empty() && !t.contains(' ')) {
            let clean_name = t.trim_start_matches('@').to_lowercase();

            // Проверяем локальную базу виденных пользователей
            if let Ok(Some(uid)) = db.find_user_by_username(&clean_name) {
                return Ok((uid, Some(format!("@{}", clean_name))));
            }

            // Проверяем список администраторов чата
            if let Ok(admins) = tg.get_chat_administrators(chat_id).await {
                for a in admins {
                    if let Some(ref un) = a.user.username {
                        if un.eq_ignore_ascii_case(&clean_name) {
                            // Сохраняем в БД на будущее
                            let _ = db.save_seen_user(a.user.id, Some(un), &a.user.first_name, None);
                            return Ok((a.user.id, Some(format!("@{}", un))));
                        }
                    }
                }
            }

            // Если был reply и имя совпадает
            if let Some((rid, ref run)) = reply_user {
                if let Some(rname) = run {
                    if rname.trim_start_matches('@').eq_ignore_ascii_case(&clean_name) {
                        return Ok((rid, Some(format!("@{}", clean_name))));
                    }
                }
            }
        }

        // 4. Если ничего не подошло, но есть reply
        if let Some(ru) = reply_user {
            return Ok(ru);
        }

        Err(anyhow!(
            "Не удалось найти пользователя '{}'. Ответьте на его сообщение (Reply) или укажите точный @username.",
            t
        ))
    }

    /// Выполнение действия модерации
    pub async fn execute_action(
        tg: &TelegramClient,
        db: &Database,
        chat_id: &str,
        action: &TelegramActionRequest,
        reply_user: Option<(i64, Option<String>)>,
        is_authorized: bool,
    ) -> Result<String> {
        if !is_authorized {
            return Ok("Отказано: управление правами и разрешениями участников доступно только @ametist1337.".to_string());
        }

        let (user_id, username_opt) = Self::resolve_target_user(
            tg,
            db,
            chat_id,
            action.target.as_deref(),
            reply_user,
        ).await?;

        let user_display = username_opt
            .clone()
            .unwrap_or_else(|| format!("ID: {}", user_id));

        info!(
            "Выполнение модерации над {} (id: {}) в чате {}: действие={}",
            user_display, user_id, chat_id, action.action
        );

        match action.action.to_lowercase().as_str() {
            "restore" => {
                Self::restore_user_permissions(tg, db, chat_id, user_id, username_opt.as_deref()).await
            }
            "unban" => {
                tg.unban_chat_member(chat_id, user_id).await?;
                Ok(format!("Пользователь {} разблокирован.", user_display))
            }
            "ban" => {
                let until = action.duration_seconds.filter(|&s| s > 0).map(|s| Utc::now().timestamp() + s as i64);
                tg.ban_chat_member(chat_id, user_id, until).await?;
                if let Some(sec) = action.duration_seconds.filter(|&s| s > 0) {
                    Ok(format!("Пользователь {} заблокирован на {} сек.", user_display, sec))
                } else {
                    Ok(format!("Пользователь {} заблокирован навсегда.", user_display))
                }
            }
            "restrict" => {
                let member_info = tg.get_chat_member(chat_id, user_id).await
                    .context("Не удалось получить информацию об участнике от Telegram")?;

                if member_info.status == "creator" {
                    return Err(anyhow!("Невозможно изменить разрешения создателя (владельца) группы."));
                }

                let is_admin = member_info.status == "administrator";

                // 1. Сохраняем исходные права в БД, если требуется восстановление
                let restore_at = if action.restore_previous_after {
                    action.duration_seconds.filter(|&s| s > 0).map(|s| Utc::now().timestamp() + s as i64)
                } else {
                    None
                };

                let admin_rights_json = if is_admin {
                    let rights = ChatAdminRights {
                        is_anonymous: member_info.is_anonymous,
                        can_manage_chat: member_info.can_manage_chat,
                        can_delete_messages: member_info.can_delete_messages,
                        can_manage_video_chats: member_info.can_manage_video_chats,
                        can_restrict_members: member_info.can_restrict_members,
                        can_promote_members: member_info.can_promote_members,
                        can_change_info: member_info.can_change_info,
                        can_invite_users: member_info.can_invite_users,
                        can_post_messages: member_info.can_post_messages,
                        can_edit_messages: member_info.can_edit_messages,
                        can_pin_messages: member_info.can_pin_messages,
                        can_post_stories: member_info.can_post_stories,
                        can_edit_stories: member_info.can_edit_stories,
                        can_delete_stories: member_info.can_delete_stories,
                        can_manage_topics: member_info.can_manage_topics,
                    };
                    Some(serde_json::to_string(&rights)?)
                } else {
                    None
                };

                let permissions_json = if member_info.status == "restricted" {
                    let perms = ChatPermissions {
                        can_send_messages: member_info.can_send_messages,
                        can_send_audios: member_info.can_send_audios,
                        can_send_documents: member_info.can_send_documents,
                        can_send_photos: member_info.can_send_photos,
                        can_send_videos: member_info.can_send_videos,
                        can_send_video_notes: member_info.can_send_video_notes,
                        can_send_voice_notes: member_info.can_send_voice_notes,
                        can_send_polls: member_info.can_send_polls,
                        can_send_other_messages: member_info.can_send_other_messages,
                        can_add_web_page_previews: member_info.can_add_web_page_previews,
                        can_change_info: member_info.can_change_info,
                        can_invite_users: member_info.can_invite_users,
                        can_pin_messages: member_info.can_pin_messages,
                        can_manage_topics: member_info.can_manage_topics,
                    };
                    Some(serde_json::to_string(&perms)?)
                } else {
                    None
                };

                let _ = db.save_permission_backup(
                    chat_id,
                    user_id,
                    username_opt.as_deref(),
                    &member_info.status,
                    member_info.custom_title.as_deref(),
                    admin_rights_json.as_deref(),
                    permissions_json.as_deref(),
                    restore_at,
                )?;

                // 2. Если участник был администратором, сначала снимаем права админа (иначе Telegram не даст ограничить)
                if is_admin {
                    let strip_rights = ChatAdminRights {
                        is_anonymous: Some(false),
                        can_manage_chat: Some(false),
                        can_delete_messages: Some(false),
                        can_manage_video_chats: Some(false),
                        can_restrict_members: Some(false),
                        can_promote_members: Some(false),
                        can_change_info: Some(false),
                        can_invite_users: Some(false),
                        can_post_messages: Some(false),
                        can_edit_messages: Some(false),
                        can_pin_messages: Some(false),
                        can_post_stories: Some(false),
                        can_edit_stories: Some(false),
                        can_delete_stories: Some(false),
                        can_manage_topics: Some(false),
                    };

                    if let Err(e) = tg.promote_chat_member(chat_id, user_id, &strip_rights).await {
                        warn!("Не удалось снять права админа с {}: {}", user_display, e);
                        return Err(anyhow!(
                            "Не удалось снять права администратора с {}. Убедитесь, что у бота включено право 'Назначение администраторов' и он выше в иерархии.",
                            user_display
                        ));
                    }
                }

                // 3. Применяем ограничения
                let empty_perms = ChatPermissions::default();
                let perms_to_apply = action.permissions.as_ref().unwrap_or(&empty_perms);
                let until = action.duration_seconds.filter(|&s| s > 0).map(|s| Utc::now().timestamp() + s as i64);

                tg.restrict_chat_member(chat_id, user_id, perms_to_apply, until).await
                    .context("Ошибка вызова restrictChatMember в Telegram")?;

                // 4. Если задано авто-восстановление, запускаем таймер
                if let Some(sec) = action.duration_seconds.filter(|&s| s > 0 && action.restore_previous_after) {
                    let tg_cl = tg.clone();
                    let db_cl = db.clone();
                    let cid = chat_id.to_string();
                    let uopt = username_opt.clone();

                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(sec)).await;
                        info!("Сработал таймер восстановления прав для {} (id: {})", cid, user_id);
                        if let Ok(msg) = Self::restore_user_permissions(&tg_cl, &db_cl, &cid, user_id, uopt.as_deref()).await {
                            let _ = tg_cl.send_message(&cid, &msg).await;
                        }
                    });

                    Ok(format!(
                        "Разрешения для {} успешно изменены на {} сек. Прежний статус ({}) сохранен и будет восстановлен автоматически.",
                        user_display, sec, if is_admin { "администратор" } else { "участник" }
                    ))
                } else {
                    Ok(format!(
                        "Разрешения для {} успешно изменены.",
                        user_display
                    ))
                }
            }
            _ => Err(anyhow!("Неизвестное действие: {}", action.action)),
        }
    }

    /// Восстановление исходных прав участника из сохраненного бэкапа
    pub async fn restore_user_permissions(
        tg: &TelegramClient,
        db: &Database,
        chat_id: &str,
        user_id: i64,
        username_hint: Option<&str>,
    ) -> Result<String> {
        let backup = db.get_latest_active_backup(chat_id, user_id)?
            .ok_or_else(|| anyhow!("Для пользователя {} не найдено сохраненных резервных копий прав.", user_id))?;

        let user_display = username_hint
            .or(backup.username.as_deref())
            .map(|u| format!("@{}", u.trim_start_matches('@')))
            .unwrap_or_else(|| format!("ID: {}", user_id));

        info!(
            "Восстановление прав для {} (id: {}): исходный статус = {}",
            user_display, user_id, backup.original_status
        );

        if backup.original_status == "administrator" {
            // 1. Снимаем любые наложенные ограничения
            let _ = tg.unban_chat_member(chat_id, user_id).await;

            // 2. Восстанавливаем права администратора
            if let Some(ref rights_json) = backup.admin_rights_json {
                if let Ok(rights) = serde_json::from_str::<ChatAdminRights>(rights_json) {
                    tg.promote_chat_member(chat_id, user_id, &rights).await
                        .context("Ошибка восстановления прав администратора через promoteChatMember")?;
                }
            }

            // 3. Восстанавливаем кастомный титул, если был
            if let Some(ref title) = backup.custom_title {
                if !title.trim().is_empty() {
                    let _ = tg.set_chat_admin_custom_title(chat_id, user_id, title.trim()).await;
                }
            }

            let _ = db.mark_backup_restored(backup.id);
            Ok(format!("Все права администратора для {} успешно восстановлены в полном объеме.", user_display))
        } else if backup.original_status == "restricted" {
            if let Some(ref perms_json) = backup.permissions_json {
                if let Ok(perms) = serde_json::from_str::<ChatPermissions>(perms_json) {
                    tg.restrict_chat_member(chat_id, user_id, &perms, None).await?;
                }
            } else {
                let _ = tg.unban_chat_member(chat_id, user_id).await;
            }

            let _ = db.mark_backup_restored(backup.id);
            Ok(format!("Прежние индивидуальные разрешения для {} успешно восстановлены.", user_display))
        } else {
            // Обычный участник: снимаем ограничения
            tg.unban_chat_member(chat_id, user_id).await?;
            let _ = db.mark_backup_restored(backup.id);
            Ok(format!("Ограничения с {} сняты. Возвращены стандартные права участника.", user_display))
        }
    }

    /// Фоновый цикл проверки просроченных ограничений для надежности при перезапусках
    pub fn start_restoration_worker(tg: TelegramClient, db: Database) {
        tokio::spawn(async move {
            info!("Запущен фоновый воркер восстановления прав модерации");
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;

                if let Ok(pending) = db.get_pending_restorations() {
                    for record in pending {
                        info!(
                            "Фоновое восстановление прав по таймеру: chat={}, user={}",
                            record.chat_id, record.user_id
                        );
                        match Self::restore_user_permissions(
                            &tg,
                            &db,
                            &record.chat_id,
                            record.user_id,
                            record.username.as_deref(),
                        ).await {
                            Ok(msg) => {
                                let _ = tg.send_message(&record.chat_id, &msg).await;
                            }
                            Err(e) => {
                                error!(
                                    "Ошибка при фоновом восстановлении прав пользователя {}: {}",
                                    record.user_id, e
                                );
                                let _ = db.mark_backup_restored(record.id);
                            }
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_action() {
        let sample = "Привет!\n```telegram_action\n{\n  \"action\": \"restrict\",\n  \"target\": \"@testuser\",\n  \"duration_seconds\": 300,\n  \"permissions\": {\n    \"can_send_messages\": false,\n    \"can_send_audios\": true\n  },\n  \"restore_previous_after\": true\n}\n```\nПользователь ограничен на 5 минут.";
        let (action, clean) = ModerationManager::extract_action(sample);
        assert!(action.is_some());
        let a = action.unwrap();
        assert_eq!(a.action, "restrict");
        assert_eq!(a.target.as_deref(), Some("@testuser"));
        assert_eq!(a.duration_seconds, Some(300));
        assert!(a.restore_previous_after);
        assert_eq!(a.permissions.as_ref().unwrap().can_send_messages, Some(false));
        assert_eq!(a.permissions.as_ref().unwrap().can_send_audios, Some(true));
        assert_eq!(clean, "Привет!\n\nПользователь ограничен на 5 минут.");
    }

    #[test]
    fn test_extract_action_no_block() {
        let sample = "Просто обычный ответ бота.";
        let (action, clean) = ModerationManager::extract_action(sample);
        assert!(action.is_none());
        assert_eq!(clean, sample);
    }
}
