use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{error, info, warn};

use crate::db::Database;
use crate::telegram::client::{ChatAdminRights, ChatPermissions, TelegramClient};

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TelegramActionRequest {
    pub action: String, // "restrict", "ban", "unban", "restore", "promote", "admin"
    pub target: Option<String>, // "@username", "123456", "reply", "@all", "все"
    pub targets: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub copy_from: Option<String>,
    pub duration_seconds: Option<u64>,
    pub permissions: Option<ChatPermissions>,
    pub admin_rights: Option<ChatAdminRights>,
    pub custom_title: Option<String>,
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

    pub fn parse_target_and_exclusions(
        target_raw: &str,
        exclude_raw: Option<&[String]>,
    ) -> (String, Vec<String>) {
        let mut exclusions = Vec::new();
        if let Some(exs) = exclude_raw {
            for e in exs {
                let clean = e.trim_start_matches('@').trim().to_lowercase();
                if !clean.is_empty() && !exclusions.contains(&clean) {
                    exclusions.push(clean);
                }
            }
        }

        let lower = target_raw.to_lowercase();
        let mut main_target = target_raw.trim().to_string();

        let kw_info = if let Some(pos) = lower.find("кроме") {
            Some((pos, "кроме".len()))
        } else if let Some(pos) = lower.find("except") {
            Some((pos, "except".len()))
        } else {
            None
        };

        if let Some((pos, len)) = kw_info {
            let char_offset = lower[..pos].chars().count();
            let kw_chars = lower[pos..pos + len].chars().count();
            let mut byte_start = None;
            let mut byte_end = None;
            for (c_idx, (b_idx, _)) in target_raw.char_indices().enumerate() {
                if c_idx == char_offset {
                    byte_start = Some(b_idx);
                }
                if c_idx == char_offset + kw_chars {
                    byte_end = Some(b_idx);
                }
            }
            if let Some(b_start) = byte_start {
                let target_part = &target_raw[..b_start];
                main_target = target_part.trim().trim_end_matches(',').trim().to_string();
                let after_except = match byte_end {
                    Some(b_end) => &target_raw[b_end..],
                    None => "",
                };
                for token in after_except.split(|c: char| c == ',' || c.is_whitespace()) {
                    let clean = token.trim().trim_start_matches('@').trim_end_matches(',').trim().to_lowercase();
                    if !clean.is_empty() && !exclusions.contains(&clean) {
                        exclusions.push(clean);
                    }
                }
            }
        }

        (main_target, exclusions)
    }

    pub fn is_bulk_target(t: &str) -> bool {
        let s = t.trim_start_matches('@').trim().to_lowercase();
        s == "all"
            || s == "все"
            || s == "всем"
            || s == "всей группе"
            || s == "все участники"
            || s == "всем участникам"
            || s == "каждому"
            || s == "everybody"
            || s == "everyone"
            || s.starts_with("все ")
            || s.starts_with("всем ")
            || s.starts_with("all ")
    }

    /// Определение целевого user_id по username, reply, имени или ID
    pub async fn resolve_target_user(
        tg: &TelegramClient,
        db: &Database,
        chat_id: &str,
        target_str: Option<&str>,
        reply_user: Option<(i64, Option<String>, Option<String>)>, // id, username, first_name
    ) -> Result<(i64, Option<String>)> {
        let bot_id = tg.bot_id();
        let reply_user = if let Some(ru) = reply_user {
            if ru.0 == bot_id {
                None
            } else {
                Some(ru)
            }
        } else {
            None
        };

        let t = target_str.unwrap_or("").trim();
        let t_clean = t.trim_start_matches('@').trim();
        let t_lower = t_clean.to_lowercase();

        // Проверяем, не пытается ли явно указать бота
        if t_lower == "ew1484_bot" || t_clean == bot_id.to_string() {
            return Err(anyhow!("Невозможно применить действие модерации к самому боту."));
        }

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

        if is_reply_keyword {
            if let Some((uid, un, _)) = reply_user {
                if uid == bot_id {
                    return Err(anyhow!("Команда вызвана в ответ на сообщение бота. Укажите конкретного участника для модерации."));
                }
                return Ok((uid, un));
            }
        }

        // 2. Если передан числовой ID (например "123456789")
        if let Ok(num_id) = t_clean.parse::<i64>() {
            if num_id == bot_id {
                return Err(anyhow!("Невозможно применить действие модерации к самому боту."));
            }
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
                if ru_id == bot_id {
                    return Err(anyhow!("Невозможно применить действие модерации к самому боту."));
                }
                return Ok((ru_id, ru_un.clone()));
            }
        }

        // 4. Поиск по локальной базе данных seen_users (по username, first_name, last_name)
        if !t_clean.is_empty() {
            if let Ok(Some((uid, un))) = db.find_user_by_name_or_username(t_clean) {
                if uid == bot_id {
                    return Err(anyhow!("Невозможно применить действие модерации к самому боту."));
                }
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
                    let matches_title = a.custom_title.as_deref().map_or(false, |ct| {
                        let ct_low = ct.trim().to_lowercase();
                        !ct_low.is_empty() && (ct_low == t_lower || ct_low.contains(&t_lower) || t_lower.contains(&ct_low))
                    });

                    if matches_un || matches_fn || matches_ln || matches_title {
                        if a.user.id == bot_id {
                            return Err(anyhow!("Невозможно применить действие модерации к самому боту."));
                        }
                        let _ = db.save_seen_user(
                            a.user.id,
                            a.user.username.as_deref(),
                            &a.user.first_name,
                            a.user.last_name.as_deref(),
                            a.custom_title.as_deref(),
                        );
                        let u_display = a.user.username.map(|u| format!("@{}", u.trim_start_matches('@')));
                        return Ok((a.user.id, u_display));
                    }
                }
            }
        }

        // 6. Если ничего не нашли по имени, но команда была вызвана в ответ (reply) - используем автора сообщения
        if let Some((ru_id, ru_un, _)) = reply_user {
            if ru_id == bot_id {
                return Err(anyhow!("Команда вызвана в ответ на сообщение бота. Укажите конкретного участника для модерации."));
            }
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

        // 1. Проверяем массовое действие (@all, все, exclude)
        let target_raw = action.target.as_deref().unwrap_or("").trim();
        let (main_target, mut exclusions) = Self::parse_target_and_exclusions(target_raw, action.exclude.as_deref());

        if let Some(ref add_ex) = action.exclude {
            for e in add_ex {
                let cl = e.trim_start_matches('@').trim().to_lowercase();
                if !cl.is_empty() && !exclusions.contains(&cl) {
                    exclusions.push(cl);
                }
            }
        }

        let is_bulk = Self::is_bulk_target(&main_target)
            || action.targets.as_ref().map_or(false, |ts| ts.len() > 1);

        if is_bulk {
            return Self::execute_bulk_action(tg, db, chat_id, action, &exclusions).await;
        }

        let resolved_target_str = if main_target.is_empty() {
            action.target.as_deref()
        } else {
            Some(main_target.as_str())
        };

        let (user_id, username_opt) = Self::resolve_target_user(
            tg,
            db,
            chat_id,
            resolved_target_str,
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
            "promote" | "admin" | "grant_admin" => {
                let mut rights = action.admin_rights.clone();
                let mut title = action.custom_title.clone();

                if let Some(ref cf) = action.copy_from {
                    if let Ok((src_uid, _)) = Self::resolve_target_user(tg, db, chat_id, Some(cf), None).await {
                        if let Ok(src_member) = tg.get_chat_member(chat_id, src_uid).await {
                            if src_member.status == "administrator" || src_member.status == "creator" {
                                rights = Some(src_member.to_admin_rights());
                                if title.is_none() {
                                    title = src_member.custom_title;
                                }
                            }
                        }
                    }
                }

                let final_rights = rights.unwrap_or_else(ChatAdminRights::all_rights);
                tg.promote_chat_member(chat_id, user_id, &final_rights).await
                    .context("Ошибка вызова promoteChatMember (проверьте, есть ли у бота право 'Назначение администраторов')")?;

                if let Some(ref t) = title {
                    if !t.trim().is_empty() {
                        let _ = tg.set_chat_admin_custom_title(chat_id, user_id, t.trim()).await;
                    }
                }

                Ok(format!("Пользователь {} успешно назначен администратором.", user_display))
            }
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

    /// Массовое выполнение действий модерации (@all, все, список участников с исключениями)
    pub async fn execute_bulk_action(
        tg: &TelegramClient,
        db: &Database,
        chat_id: &str,
        action: &TelegramActionRequest,
        exclusions: &[String],
    ) -> Result<String> {
        let bot_id = tg.bot_id();
        info!("Выполнение массового действия модерации '{}' в чате {}", action.action, chat_id);

        // 1. Собираем всех известных участников чата
        let mut candidates: Vec<(i64, Option<String>, String)> = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        if let Ok(known_users) = db.get_all_known_users_in_chat(chat_id) {
            for (uid, un, fn_) in known_users {
                if seen_ids.insert(uid) {
                    candidates.push((uid, un, fn_));
                }
            }
        }

        // 2. Добавляем администраторов чата
        let admins = tg.get_chat_administrators(chat_id).await.unwrap_or_default();
        let mut creator_id = None;
        for a in &admins {
            if a.status == "creator" {
                creator_id = Some(a.user.id);
            }
            if seen_ids.insert(a.user.id) {
                candidates.push((
                    a.user.id,
                    a.user.username.clone(),
                    a.user.first_name.clone(),
                ));
            }
        }

        // 3. Если указаны конкретные цели в action.targets
        if let Some(ref target_list) = action.targets {
            for t in target_list {
                if let Ok((uid, un)) = Self::resolve_target_user(tg, db, chat_id, Some(t), None).await {
                    if seen_ids.insert(uid) {
                        candidates.push((uid, un, t.clone()));
                    }
                }
            }
        }

        // 4. Если источник прав - другой пользователь (copy_from)
        let mut copy_admin_rights = None;
        let mut copy_title = None;
        let mut copy_perms = None;

        if let Some(ref cf) = action.copy_from {
            if let Ok((src_uid, _)) = Self::resolve_target_user(tg, db, chat_id, Some(cf), None).await {
                if let Ok(src_member) = tg.get_chat_member(chat_id, src_uid).await {
                    if src_member.status == "administrator" || src_member.status == "creator" {
                        copy_admin_rights = Some(src_member.to_admin_rights());
                        copy_title = src_member.custom_title.clone();
                    } else if src_member.status == "restricted" {
                        copy_perms = Some(ChatPermissions {
                            can_send_messages: src_member.can_send_messages,
                            can_send_audios: src_member.can_send_audios,
                            can_send_documents: src_member.can_send_documents,
                            can_send_photos: src_member.can_send_photos,
                            can_send_videos: src_member.can_send_videos,
                            can_send_video_notes: src_member.can_send_video_notes,
                            can_send_voice_notes: src_member.can_send_voice_notes,
                            can_send_polls: src_member.can_send_polls,
                            can_send_other_messages: src_member.can_send_other_messages,
                            can_add_web_page_previews: src_member.can_add_web_page_previews,
                            can_change_info: src_member.can_change_info,
                            can_invite_users: src_member.can_invite_users,
                            can_pin_messages: src_member.can_pin_messages,
                            can_manage_topics: src_member.can_manage_topics,
                        });
                    }
                }
            }
        }

        let is_user_excluded = |uid: i64, un: Option<&str>, fn_name: &str| -> bool {
            if uid == bot_id {
                return true;
            }
            if Some(uid) == creator_id {
                return true;
            }
            let un_clean = un.map(|u| u.trim_start_matches('@').to_lowercase()).unwrap_or_default();
            let fn_low = fn_name.to_lowercase();

            for ex in exclusions {
                let ex_clean = ex.trim_start_matches('@').trim().to_lowercase();
                if ex_clean.is_empty() {
                    continue;
                }
                if let Ok(ex_id) = ex_clean.parse::<i64>() {
                    if ex_id == uid {
                        return true;
                    }
                }
                if !un_clean.is_empty() && un_clean == ex_clean {
                    return true;
                }
                if !fn_low.is_empty() && (fn_low == ex_clean || fn_low.contains(&ex_clean) || ex_clean.contains(&fn_low)) {
                    return true;
                }
            }
            false
        };

        let mut success_count = 0;
        let mut fail_count = 0;
        let mut excluded_names = Vec::new();
        let action_type = action.action.to_lowercase();

        for (uid, un, fn_name) in candidates {
            let user_display = un
                .as_ref()
                .map(|u| format!("@{}", u.trim_start_matches('@')))
                .unwrap_or_else(|| {
                    if !fn_name.trim().is_empty() {
                        fn_name.clone()
                    } else {
                        format!("ID: {}", uid)
                    }
                });

            if is_user_excluded(uid, un.as_deref(), &fn_name) {
                if uid != bot_id && Some(uid) != creator_id {
                    excluded_names.push(user_display);
                }
                continue;
            }

            match action_type.as_str() {
                "promote" | "admin" | "grant_admin" => {
                    let rights = copy_admin_rights
                        .clone()
                        .or_else(|| action.admin_rights.clone())
                        .unwrap_or_else(ChatAdminRights::all_rights);

                    match tg.promote_chat_member(chat_id, uid, &rights).await {
                        Ok(_) => {
                            let title = copy_title.as_deref().or(action.custom_title.as_deref());
                            if let Some(t) = title {
                                if !t.trim().is_empty() {
                                    let _ = tg.set_chat_admin_custom_title(chat_id, uid, t.trim()).await;
                                }
                            }
                            success_count += 1;
                        }
                        Err(e) => {
                            warn!("Не удалось выдать права администратора пользователю {} ({}): {}", user_display, uid, e);
                            fail_count += 1;
                        }
                    }
                }
                "restrict" => {
                    let perms = copy_perms
                        .clone()
                        .or_else(|| action.permissions.clone())
                        .unwrap_or_default();
                    let until = action.duration_seconds.filter(|&s| s > 0).map(|s| {
                        let now = Utc::now().timestamp();
                        if s < 30 { now + 35 } else { now + s as i64 }
                    });

                    match tg.restrict_chat_member(chat_id, uid, &perms, until).await {
                        Ok(_) => {
                            success_count += 1;
                        }
                        Err(e) => {
                            warn!("Не удалось ограничить пользователя {} ({}): {}", user_display, uid, e);
                            fail_count += 1;
                        }
                    }
                }
                "restore" | "unban" => {
                    match Self::restore_user_permissions(tg, db, chat_id, uid, un.as_deref()).await {
                        Ok(_) => {
                            success_count += 1;
                        }
                        Err(e) => {
                            warn!("Не удалось восстановить пользователя {} ({}): {}", user_display, uid, e);
                            fail_count += 1;
                        }
                    }
                }
                _ => {}
            }
        }

        let mut res = match action_type.as_str() {
            "promote" | "admin" | "grant_admin" => {
                format!("Массовое назначение администраторов выполнено. Обновлено участников: {}.", success_count)
            }
            "restrict" => {
                format!("Массовое изменение разрешений выполнено. Обновлено участников: {}.", success_count)
            }
            "restore" | "unban" => {
                format!("Массовое восстановление прав выполнено. Обновлено участников: {}.", success_count)
            }
            _ => format!("Массовое действие выполнено для {} участников.", success_count),
        };

        if !excluded_names.is_empty() {
            res.push_str(&format!(" Исключены: {}.", excluded_names.join(", ")));
        }

        if fail_count > 0 {
            res.push_str(&format!(" Ошибок при обновлении: {} (проверьте права бота на назначение администраторов).", fail_count));
        }

        if success_count == 0 && fail_count == 0 {
            res.push_str(" В базе данных бота пока нет других известных участников этой группы (кроме создателя и исключенных). Чтобы бот мог управлять участниками, они должны написать хотя бы одно сообщение в группу.");
        }

        Ok(res)
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
    let targets = obj.get("targets").and_then(|v| {
        if let Some(arr) = v.as_array() {
            Some(arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        } else {
            None
        }
    });
    let exclude = obj.get("exclude").and_then(|v| {
        if let Some(arr) = v.as_array() {
            Some(arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        } else if let Some(s) = v.as_str() {
            Some(vec![s.to_string()])
        } else {
            None
        }
    });
    let copy_from = obj.get("copy_from").and_then(|v| v.as_str().map(|s| s.to_string()));
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
    let admin_rights = obj.get("admin_rights").and_then(|v| {
        serde_json::from_value::<ChatAdminRights>(v.clone()).ok()
    });
    let custom_title = obj.get("custom_title").and_then(|v| v.as_str().map(|s| s.to_string()));

    Some(TelegramActionRequest {
        action,
        target,
        targets,
        exclude,
        copy_from,
        duration_seconds,
        permissions,
        admin_rights,
        custom_title,
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
        let _ = db.save_seen_user(999999, Some("imran777"), "Имран", None, Some("имран"));
        let res3 = ModerationManager::resolve_target_user(&tg, &db, "-100", Some("Имран"), None).await;
        assert_eq!(res3.unwrap(), (999999, Some("@imran777".to_string())));
    }

    #[test]
    fn test_parse_target_and_exclusions() {
        let (main, ex) = ModerationManager::parse_target_and_exclusions("все, кроме @AHzkto67", None);
        assert_eq!(main, "все");
        assert_eq!(ex, vec!["ahzkto67".to_string()]);

        let (main2, ex2) = ModerationManager::parse_target_and_exclusions("@all except @user1, @user2", None);
        assert_eq!(main2, "@all");
        assert_eq!(ex2, vec!["user1".to_string(), "user2".to_string()]);

        let (main3, ex3) = ModerationManager::parse_target_and_exclusions("все", Some(&["@excluded_one".to_string()]));
        assert_eq!(main3, "все");
        assert_eq!(ex3, vec!["excluded_one".to_string()]);
    }

    #[test]
    fn test_is_bulk_target() {
        assert!(ModerationManager::is_bulk_target("@all"));
        assert!(ModerationManager::is_bulk_target("all"));
        assert!(ModerationManager::is_bulk_target("все"));
        assert!(ModerationManager::is_bulk_target("всем"));
        assert!(ModerationManager::is_bulk_target("каждому"));
        assert!(ModerationManager::is_bulk_target("всей группе"));
        assert!(!ModerationManager::is_bulk_target("@user123"));
        assert!(!ModerationManager::is_bulk_target("Имран"));
    }

    #[test]
    fn test_extract_promote_action() {
        let sample = "Готово!\n```telegram_action\n{\n  \"action\": \"promote\",\n  \"target\": \"@all\",\n  \"exclude\": [\"@AHzkto67\"],\n  \"copy_from\": \"@gussiavaffe\"\n}\n```\nВсе участники, кроме @AHzkto67, назначены администраторами с правами как у @gussiavaffe.";
        let (action, clean) = ModerationManager::extract_action(sample);
        assert!(action.is_some());
        let a = action.unwrap();
        assert_eq!(a.action, "promote");
        assert_eq!(a.target.as_deref(), Some("@all"));
        assert_eq!(a.exclude.as_ref().unwrap(), &vec!["@AHzkto67".to_string()]);
        assert_eq!(a.copy_from.as_deref(), Some("@gussiavaffe"));
        assert_eq!(clean, "Готово!\n\nВсе участники, кроме @AHzkto67, назначены администраторами с правами как у @gussiavaffe.");
    }
}
