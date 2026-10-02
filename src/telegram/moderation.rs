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
    /// Извлечение блока telegram_action из ответа модели и надежная очистка текста для чата
    pub fn extract_action(text: &str) -> (Option<TelegramActionRequest>, String) {
        // 1. Поиск блока между тройными кавычками ```
        let mut cur_offset = 0;
        while let Some(start_rel) = text[cur_offset..].find("```") {
            let start_idx = cur_offset + start_rel;
            let after_open = start_idx + 3;
            if let Some(end_rel) = text[after_open..].find("```") {
                let end_idx = after_open + end_rel + 3;
                let block_content = &text[after_open..after_open + end_rel];

                // Проверяем, содержит ли блок команду действия модерации
                if block_content.contains("\"action\"") || block_content.contains("'action'") {
                    // Всегда вырезаем этот блок из текста, чтобы сырой JSON не попадал в Telegram чат
                    let mut clean_text = String::new();
                    clean_text.push_str(&text[..start_idx]);
                    clean_text.push_str(&text[end_idx..]);
                    let clean_text = clean_text.trim().to_string();

                    let sanitized = sanitize_action_json(block_content);
                    if let Ok(action) = serde_json::from_str::<TelegramActionRequest>(&sanitized) {
                        return (Some(action), clean_text);
                    } else if let Some(action) = parse_action_loosely(&sanitized) {
                        return (Some(action), clean_text);
                    } else {
                        warn!("Найден блок действия модерации, но не удалось распарсить JSON: {}", sanitized);
                        return (None, clean_text);
                    }
                }
                cur_offset = end_idx;
            } else {
                break;
            }
        }

        // 2. Если вдруг модель вернула JSON без кавычек ```, но с {"action": ...}
        if let Some(start_idx) = text.find("{\"action\"") {
            if let Some(end_offset) = text[start_idx..].rfind('}') {
                let end_idx = start_idx + end_offset + 1;
                let json_slice = &text[start_idx..end_idx];
                let sanitized = sanitize_action_json(json_slice);

                let mut clean_text = String::new();
                clean_text.push_str(&text[..start_idx]);
                clean_text.push_str(&text[end_idx..]);
                let clean_text = clean_text.trim().to_string();

                if let Ok(action) = serde_json::from_str::<TelegramActionRequest>(&sanitized) {
                    return (Some(action), clean_text);
                } else if let Some(action) = parse_action_loosely(&sanitized) {
                    return (Some(action), clean_text);
                }
            }
        }

        (None, text.to_string())
    }

    /// Определение целевого user_id по username, reply, имени или ID
    pub async fn resolve_target_user(
        tg: &TelegramClient,
        db: &Database,
        chat_id: &str,
        target_str: Option<&str>,
        reply_user: Option<(i64, Option<String>, Option<String>)>, // id, username, first_name
    ) -> Result<(i64, Option<String>)> {
        let t = target_str.unwrap_or("").trim();
        let t_clean = t.trim_start_matches('@').trim();
        let t_lower = t_clean.to_lowercase();

        // 1. Если явно указан "reply" или цель пустая / местоимение при наличии reply
        let is_reply_keyword = t_lower.is_empty()
            || t_lower == "reply"
            || t_lower == "он"
            || t_lower == "его"
            || t_lower == "ему"
            || t_lower == "её"
            || t_lower == "этот"
            || t_lower == "этого"
            || t_lower == "участник"
            || t_lower == "пользователь";

        if is_reply_keyword && reply_user.is_some() {
            let (uid, un, _) = reply_user.unwrap();
            return Ok((uid, un));
        }

        // 2. Если передан числовой ID (например "123456789")
        if let Ok(num_id) = t_clean.parse::<i64>() {
            let un = if let Some((ru_id, ref ru_un, _)) = reply_user {
                if ru_id == num_id {
                    ru_un.clone()
                } else {
                    None
                }
            } else {
                None
            };
            return Ok((num_id, un));
        }

        // 3. Если передан reply_user, проверяем совпадение с ним (по username или имени)
        if let Some((ru_id, ref ru_un, ref ru_fn)) = reply_user {
            let matches_un = ru_un.as_ref().map_or(false, |un| {
                un.trim_start_matches('@').eq_ignore_ascii_case(t_clean)
            });
            let matches_fn = ru_fn.as_ref().map_or(false, |fn_| {
                let fn_low = fn_.to_lowercase();
                fn_low == t_lower || fn_low.contains(&t_lower) || t_lower.contains(&fn_low)
            });
            if matches_un || matches_fn {
                return Ok((ru_id, ru_un.clone()));
            }
        }

        // 4. Поиск по локальной базе данных seen_users (по username, first_name, last_name)
        if !t_clean.is_empty() {
            if let Ok(Some((uid, un))) = db.find_user_by_name_or_username(t_clean) {
                return Ok((uid, un.map(|u| format!("@{}", u.trim_start_matches('@')))));
            }
        }

        // 5. Поиск среди администраторов чата (по username, first_name, last_name)
        if !t_clean.is_empty() {
            if let Ok(admins) = tg.get_chat_administrators(chat_id).await {
                for a in admins {
                    let matches_un = a.user.username.as_ref().map_or(false, |u| {
                        u.trim_start_matches('@').eq_ignore_ascii_case(t_clean)
                    });
                    let matches_fn = {
                        let fn_low = a.user.first_name.to_lowercase();
                        fn_low == t_lower || fn_low.contains(&t_lower) || t_lower.contains(&fn_low)
                    };
                    let matches_ln = a.user.last_name.as_ref().map_or(false, |ln| {
                        let ln_low = ln.to_lowercase();
                        ln_low == t_lower || ln_low.contains(&t_lower)
                    });

                    if matches_un || matches_fn || matches_ln {
                        let _ = db.save_seen_user(
                            a.user.id,
                            a.user.username.as_deref(),
                            &a.user.first_name,
                            a.user.last_name.as_deref(),
                        );
                        let u_display = a.user.username.map(|u| format!("@{}", u.trim_start_matches('@')));
                        return Ok((a.user.id, u_display));
                    }
                }
            }
        }

        // 6. Если ничего не нашли по имени, но команда была вызвана в ответ (reply) - используем автора сообщения
        if let Some((ru_id, ru_un, _)) = reply_user {
            return Ok((ru_id, ru_un));
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
        reply_user: Option<(i64, Option<String>, Option<String>)>,
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
                let until = action.duration_seconds.filter(|&s| s > 0).map(|s| {
                    let now = Utc::now().timestamp();
                    if s < 30 { now + 35 } else { now + s as i64 }
                });
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
                let until = action.duration_seconds.filter(|&s| s > 0).map(|s| {
                    let now = Utc::now().timestamp();
                    if s < 30 { now + 35 } else { now + s as i64 }
                });

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
        let backup_opt = db.get_latest_active_backup(chat_id, user_id)?;

        let user_display = username_hint
            .or(backup_opt.as_ref().and_then(|b| b.username.as_deref()))
            .map(|u| format!("@{}", u.trim_start_matches('@')))
            .unwrap_or_else(|| format!("ID: {}", user_id));

        if let Some(backup) = backup_opt {
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
        } else {
            // Если сохраненного бэкапа нет в базе данных: снимаем все ограничения через unbanChatMember
            info!(
                "Для {} (id: {}) не найдено сохраненных бэкапов прав в базе, снимаем ограничения",
                user_display, user_id
            );
            tg.unban_chat_member(chat_id, user_id).await?;
            Ok(format!("С пользователя {} сняты все ограничения. Возвращены стандартные права участника.", user_display))
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

/// Очистка сырой строки JSON от комментариев, языковых тегов и лишних запятых
fn sanitize_action_json(raw: &str) -> String {
    let mut s = raw.trim();
    // Пропускаем тег языка в первой строке (telegram_action, json, JSON и т.д.)
    if let Some(newline_pos) = s.find('\n') {
        let first_line = s[..newline_pos].trim().to_lowercase();
        if first_line.starts_with("telegram_action")
            || first_line.starts_with("json")
            || first_line.starts_with("action")
        {
            s = s[newline_pos + 1..].trim();
        }
    }

    // Если нет открывающей {, ищем её
    if let Some(brace_idx) = s.find('{') {
        s = &s[brace_idx..];
    }
    // Если есть закрывающая }, берем до последней }
    if let Some(last_brace) = s.rfind('}') {
        s = &s[..=last_brace];
    }

    // Удаляем однострочные комментарии //
    let mut lines = Vec::new();
    for line in s.lines() {
        let mut clean_line = line;
        if let Some(comment_idx) = line.find("//") {
            let prefix = &line[..comment_idx];
            let quotes_count = prefix.chars().filter(|&c| c == '"').count();
            if quotes_count % 2 == 0 {
                clean_line = prefix.trim_end();
            }
        }
        lines.push(clean_line);
    }
    let out = lines.join("\n");

    remove_trailing_commas(&out)
}

fn remove_trailing_commas(s: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_str = false;
    let mut last_comma_idx: Option<usize> = None;

    while i < len {
        let c = chars[i];
        if c == '"' && (i == 0 || chars[i - 1] != '\\') {
            in_str = !in_str;
            last_comma_idx = None;
            result.push(c);
            i += 1;
            continue;
        }

        if in_str {
            result.push(c);
            i += 1;
            continue;
        }

        if c == ',' {
            last_comma_idx = Some(result.len());
            result.push(c);
            i += 1;
            continue;
        }

        if c == '}' || c == ']' {
            if let Some(comma_pos) = last_comma_idx {
                result.remove(comma_pos);
                last_comma_idx = None;
            }
            result.push(c);
            i += 1;
            continue;
        }

        if !c.is_whitespace() {
            last_comma_idx = None;
        }

        result.push(c);
        i += 1;
    }

    result
}

/// Мягкий парсер на случай нестрогих типов в JSON
fn parse_action_loosely(json_str: &str) -> Option<TelegramActionRequest> {
    let val: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let obj = val.as_object()?;
    let action = obj.get("action")?.as_str()?.to_string();
    let target = obj.get("target").and_then(|v| {
        if v.is_null() {
            None
        } else if let Some(s) = v.as_str() {
            Some(s.to_string())
        } else if let Some(n) = v.as_i64() {
            Some(n.to_string())
        } else {
            None
        }
    });
    let duration_seconds = obj.get("duration_seconds").and_then(|v| {
        if v.is_null() {
            None
        } else if let Some(n) = v.as_u64() {
            Some(n)
        } else if let Some(s) = v.as_str() {
            s.parse::<u64>().ok()
        } else {
            None
        }
    });
    let restore_previous_after = obj
        .get("restore_previous_after")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let permissions = obj.get("permissions").and_then(|v| {
        serde_json::from_value::<ChatPermissions>(v.clone()).ok()
    });

    Some(TelegramActionRequest {
        action,
        target,
        duration_seconds,
        permissions,
        restore_previous_after,
    })
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
    fn test_extract_action_with_comments_and_json_tag() {
        let sample = "```JSON\n{\n  \"action\": \"restrict\", // комментарий\n  \"target\": \"Имран\",\n  \"duration_seconds\": null,\n  \"permissions\": {\n    \"can_send_messages\": true,\n  },\n  \"restore_previous_after\": false,\n}\n```\nРазмутил Имрана.";
        let (action, clean) = ModerationManager::extract_action(sample);
        assert!(action.is_some());
        let a = action.unwrap();
        assert_eq!(a.action, "restrict");
        assert_eq!(a.target.as_deref(), Some("Имран"));
        assert_eq!(a.duration_seconds, None);
        assert_eq!(clean, "Размутил Имрана.");
    }

    #[test]
    fn test_extract_action_corrupted_json_is_stripped() {
        let sample = "```json\n{\n  \"action\": \"restrict\", unclosed syntax\n```\nПользователь размучен.";
        let (action, clean) = ModerationManager::extract_action(sample);
        // JSON не валиден, но кодовый блок ОБЯЗАН быть вырезан из clean!
        assert!(action.is_none());
        assert_eq!(clean, "Пользователь размучен.");
    }

    #[test]
    fn test_extract_action_no_block() {
        let sample = "Просто обычный ответ бота.";
        let (action, clean) = ModerationManager::extract_action(sample);
        assert!(action.is_none());
        assert_eq!(clean, sample);
    }

    #[tokio::test]
    async fn test_resolve_target_user_fallback() {
        let tg = TelegramClient::new("fake_token".to_string());
        let db = Database::open(":memory:").unwrap();

        // 1. Команда с target="reply" и переданным reply_user
        let reply_user = Some((8026023851, Some("@imran_nick".to_string()), Some("Имран".to_string())));
        let res = ModerationManager::resolve_target_user(&tg, &db, "-100", Some("reply"), reply_user.clone()).await;
        assert_eq!(res.unwrap(), (8026023851, Some("@imran_nick".to_string())));

        // 2. Команда с target="Имран" и переданным reply_user без юзернейма
        let reply_no_un = Some((8026023851, None, Some("Имран".to_string())));
        let res2 = ModerationManager::resolve_target_user(&tg, &db, "-100", Some("Имран"), reply_no_un.clone()).await;
        assert_eq!(res2.unwrap(), (8026023851, None));

        // 3. Команда с target="Имран", но в reply_user другое имя, зато в БД есть Имран
        let _ = db.save_seen_user(999999, Some("imran777"), "Имран", None);
        let res3 = ModerationManager::resolve_target_user(&tg, &db, "-100", Some("Имран"), None).await;
        assert_eq!(res3.unwrap(), (999999, Some("@imran777".to_string())));
    }
}
