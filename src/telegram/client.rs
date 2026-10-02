use anyhow::{anyhow, Context, Result};
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use tracing::info;

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
    pub edited_message: Option<TelegramMessage>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    pub from: Option<TelegramUser>,
    pub chat: TelegramChat,
    pub text: Option<String>,
    pub caption: Option<String>,
    pub date: i64,
    pub reply_to_message: Option<Box<TelegramMessage>>,
    pub photo: Option<serde_json::Value>,
    pub sticker: Option<serde_json::Value>,
    pub voice: Option<serde_json::Value>,
    pub audio: Option<serde_json::Value>,
    pub document: Option<serde_json::Value>,
    pub video: Option<serde_json::Value>,
    pub video_note: Option<serde_json::Value>,
}

impl TelegramMessage {
    pub fn display_text(&self) -> String {
        if let Some(ref t) = self.text {
            if !t.trim().is_empty() {
                return t.clone();
            }
        }
        if let Some(ref c) = self.caption {
            if !c.trim().is_empty() {
                return c.clone();
            }
        }
        if self.sticker.is_some() {
            return "[Стикер]".to_string();
        }
        if self.voice.is_some() {
            return "[Голосовое сообщение]".to_string();
        }
        if self.video_note.is_some() {
            return "[Кружочек/видеосообщение]".to_string();
        }
        if self.photo.is_some() {
            return "[Фото]".to_string();
        }
        if self.video.is_some() {
            return "[Видео]".to_string();
        }
        if self.document.is_some() {
            return "[Документ]".to_string();
        }
        if self.audio.is_some() {
            return "[Аудио]".to_string();
        }
        String::new()
    }

    pub fn get_largest_photo_file_id(&self) -> Option<String> {
        if let Some(ref p) = self.photo {
            if let Some(arr) = p.as_array() {
                let mut best_id = None;
                let mut max_metric: i64 = -1;
                for item in arr {
                    if let Some(fid) = item.get("file_id").and_then(|v| v.as_str()) {
                        let size = item.get("file_size").and_then(|v| v.as_i64())
                            .or_else(|| item.get("width").and_then(|v| v.as_i64()))
                            .unwrap_or(0);
                        if size >= max_metric {
                            max_metric = size;
                            best_id = Some(fid.to_string());
                        }
                    }
                }
                if best_id.is_some() {
                    return best_id;
                }
            }
        }
        None
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramUser {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub last_name: Option<String>,
    pub username: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
    pub title: Option<String>,
    pub username: Option<String>,
    #[serde(rename = "type")]
    pub chat_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatPermissions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_send_messages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_send_audios: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_send_documents: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_send_photos: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_send_videos: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_send_video_notes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_send_voice_notes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_send_polls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_send_other_messages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_add_web_page_previews: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_change_info: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_invite_users: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_pin_messages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_manage_topics: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatAdminRights {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_anonymous: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_manage_chat: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_delete_messages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_manage_video_chats: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_restrict_members: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_promote_members: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_change_info: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_invite_users: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_post_messages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_edit_messages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_pin_messages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_post_stories: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_edit_stories: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_delete_stories: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_manage_topics: Option<bool>,
}

impl ChatAdminRights {
    pub fn all_rights() -> Self {
        Self {
            is_anonymous: Some(false),
            can_manage_chat: Some(true),
            can_delete_messages: Some(true),
            can_manage_video_chats: Some(true),
            can_restrict_members: Some(true),
            can_promote_members: Some(true),
            can_change_info: Some(true),
            can_invite_users: Some(true),
            can_post_messages: Some(true),
            can_edit_messages: Some(true),
            can_pin_messages: Some(true),
            can_post_stories: Some(true),
            can_edit_stories: Some(true),
            can_delete_stories: Some(true),
            can_manage_topics: Some(true),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMember {
    pub status: String,
    pub user: TelegramUser,
    pub custom_title: Option<String>,
    pub is_anonymous: Option<bool>,
    pub can_manage_chat: Option<bool>,
    pub can_delete_messages: Option<bool>,
    pub can_manage_video_chats: Option<bool>,
    pub can_restrict_members: Option<bool>,
    pub can_promote_members: Option<bool>,
    pub can_change_info: Option<bool>,
    pub can_invite_users: Option<bool>,
    pub can_post_messages: Option<bool>,
    pub can_edit_messages: Option<bool>,
    pub can_pin_messages: Option<bool>,
    pub can_post_stories: Option<bool>,
    pub can_edit_stories: Option<bool>,
    pub can_delete_stories: Option<bool>,
    pub can_manage_topics: Option<bool>,
    pub can_send_messages: Option<bool>,
    pub can_send_audios: Option<bool>,
    pub can_send_documents: Option<bool>,
    pub can_send_photos: Option<bool>,
    pub can_send_videos: Option<bool>,
    pub can_send_video_notes: Option<bool>,
    pub can_send_voice_notes: Option<bool>,
    pub can_send_polls: Option<bool>,
    pub can_send_other_messages: Option<bool>,
    pub can_add_web_page_previews: Option<bool>,
    pub until_date: Option<i64>,
}

impl ChatMember {
    pub fn to_admin_rights(&self) -> ChatAdminRights {
        ChatAdminRights {
            is_anonymous: self.is_anonymous,
            can_manage_chat: self.can_manage_chat,
            can_delete_messages: self.can_delete_messages,
            can_manage_video_chats: self.can_manage_video_chats,
            can_restrict_members: self.can_restrict_members,
            can_promote_members: self.can_promote_members,
            can_change_info: self.can_change_info,
            can_invite_users: self.can_invite_users,
            can_post_messages: self.can_post_messages,
            can_edit_messages: self.can_edit_messages,
            can_pin_messages: self.can_pin_messages,
            can_post_stories: self.can_post_stories,
            can_edit_stories: self.can_edit_stories,
            can_delete_stories: self.can_delete_stories,
            can_manage_topics: self.can_manage_topics,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct TelegramApiResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
}

#[derive(Clone)]
pub struct TelegramClient {
    client: Client,
    bot_token: String,
}

impl TelegramClient {
    pub fn new(bot_token: String) -> Self {
        Self {
            client: Client::new(),
            bot_token,
        }
    }

    pub fn bot_id(&self) -> i64 {
        self.bot_token
            .split(':')
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
    }

    fn api_url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{}", self.bot_token, method)
    }

    pub async fn send_message(&self, chat_id: &str, text: &str) -> Result<i64> {
        self.send_message_reply(chat_id, text, None).await
    }

    pub async fn send_message_reply(
        &self,
        chat_id: &str,
        text: &str,
        reply_to_message_id: Option<i64>,
    ) -> Result<i64> {
        let chunks = Self::split_message(text, 4000);
        let mut last_msg_id = 0;

        for (idx, chunk) in chunks.iter().enumerate() {
            let chunk_text = if chunks.len() > 1 {
                format!("[Часть {}/{}]\n{}", idx + 1, chunks.len(), chunk)
            } else {
                chunk.clone()
            };

            let html_text = crate::telegram::formatter::markdown_to_telegram_html(&chunk_text);
            let mut payload = json!({
                "chat_id": chat_id,
                "text": html_text,
                "parse_mode": "HTML",
                "disable_web_page_preview": true,
                "allow_sending_without_reply": true
            });

            let target_reply = if idx == 0 {
                reply_to_message_id
            } else if last_msg_id > 0 {
                Some(last_msg_id)
            } else {
                reply_to_message_id
            };

            if let Some(reply_id) = target_reply {
                payload["reply_to_message_id"] = json!(reply_id);
            }

            let resp = self
                .client
                .post(self.api_url("sendMessage"))
                .json(&payload)
                .send()
                .await
                .context("Ошибка выполнения HTTP запроса к Telegram API")?;

            let res: TelegramApiResponse<TelegramMessage> = resp.json().await?;
            if !res.ok {
                tracing::warn!(
                    "Ошибка отправки HTML в Telegram ({:?}), повторная отправка plain text...",
                    res.description
                );
                let mut plain_payload = json!({
                    "chat_id": chat_id,
                    "text": chunk_text,
                    "disable_web_page_preview": true,
                    "allow_sending_without_reply": true
                });
                if let Some(reply_id) = target_reply {
                    plain_payload["reply_to_message_id"] = json!(reply_id);
                }
                let plain_resp = self
                    .client
                    .post(self.api_url("sendMessage"))
                    .json(&plain_payload)
                    .send()
                    .await
                    .context("Ошибка повторной отправки plain text в Telegram")?;
                let plain_res: TelegramApiResponse<TelegramMessage> = plain_resp.json().await?;
                if !plain_res.ok {
                    return Err(anyhow!(
                        "Ошибка отправки сообщения в Telegram: {:?}",
                        plain_res.description
                    ));
                }
                if let Some(msg) = plain_res.result {
                    last_msg_id = msg.message_id;
                }
            } else if let Some(msg) = res.result {
                last_msg_id = msg.message_id;
            }
        }

        Ok(last_msg_id)
    }

    pub async fn edit_message_text(&self, chat_id: &str, message_id: i64, text: &str) -> Result<()> {
        let html_text = crate::telegram::formatter::markdown_to_telegram_html(text);
        let payload = json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": html_text,
            "parse_mode": "HTML",
            "disable_web_page_preview": true
        });

        let resp = self
            .client
            .post(self.api_url("editMessageText"))
            .json(&payload)
            .send()
            .await
            .context("Ошибка выполнения HTTP запроса к Telegram API (editMessageText)")?;

        let res: TelegramApiResponse<serde_json::Value> = resp.json().await?;
        if !res.ok {
            // Если ошибка в разметке HTML, пробуем plain text
            let plain_payload = json!({
                "chat_id": chat_id,
                "message_id": message_id,
                "text": text,
                "disable_web_page_preview": true
            });
            let _ = self
                .client
                .post(self.api_url("editMessageText"))
                .json(&plain_payload)
                .send()
                .await;
        }

        Ok(())
    }

    pub async fn send_photo<P: AsRef<Path>>(
        &self,
        chat_id: &str,
        photo_path: P,
        caption: Option<&str>,
        reply_to_message_id: Option<i64>,
    ) -> Result<i64> {
        let file_bytes = tokio::fs::read(&photo_path).await?;
        let filename = photo_path
            .as_ref()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let lower = filename.to_lowercase();
        let mime = if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            "image/jpeg"
        } else if lower.ends_with(".webp") {
            "image/webp"
        } else {
            "image/png"
        };

        let part = Part::bytes(file_bytes)
            .file_name(filename)
            .mime_str(mime)?;

        let mut form = Form::new()
            .text("chat_id", chat_id.to_string())
            .part("photo", part);

        if let Some(c) = caption {
            form = form.text("caption", c.to_string());
        }
        if let Some(r_id) = reply_to_message_id {
            form = form.text("reply_to_message_id", r_id.to_string());
        }

        let resp = self
            .client
            .post(self.api_url("sendPhoto"))
            .multipart(form)
            .send()
            .await?;

        let res: TelegramApiResponse<TelegramMessage> = resp.json().await?;
        if !res.ok {
            return Err(anyhow!(
                "Ошибка отправки фото в Telegram: {:?}",
                res.description
            ));
        }

        Ok(res.result.map(|m| m.message_id).unwrap_or(0))
    }

    pub async fn get_file_path(&self, file_id: &str) -> Result<String> {
        let payload = serde_json::json!({
            "file_id": file_id
        });
        let resp = self
            .client
            .post(self.api_url("getFile"))
            .json(&payload)
            .send()
            .await?;

        let res: TelegramApiResponse<serde_json::Value> = resp.json().await?;
        if !res.ok {
            return Err(anyhow!(
                "Ошибка вызова getFile в Telegram: {:?}",
                res.description
            ));
        }

        let file_path = res
            .result
            .and_then(|r| r.get("file_path").and_then(|v| v.as_str().map(|s| s.to_string())))
            .ok_or_else(|| anyhow!("В ответе getFile отсутствует file_path"))?;

        Ok(file_path)
    }

    pub async fn download_file_to_path<P: AsRef<Path>>(&self, file_id: &str, target_path: P) -> Result<()> {
        let file_path = self.get_file_path(file_id).await?;
        let download_url = format!("https://api.telegram.org/file/bot{}/{}", self.bot_token, file_path);
        let resp = self.client.get(&download_url).send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("Ошибка скачивания файла Telegram: HTTP {}", resp.status()));
        }
        let bytes = resp.bytes().await?;
        tokio::fs::write(&target_path, bytes).await?;
        Ok(())
    }

    pub async fn send_document<P: AsRef<Path>>(
        &self,
        chat_id: &str,
        doc_path: P,
        caption: Option<&str>,
    ) -> Result<i64> {
        let file_bytes = tokio::fs::read(&doc_path).await?;
        let filename = doc_path
            .as_ref()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let part = Part::bytes(file_bytes)
            .file_name(filename)
            .mime_str("text/html; charset=utf-8")?;

        let mut form = Form::new()
            .text("chat_id", chat_id.to_string())
            .part("document", part);

        if let Some(c) = caption {
            form = form.text("caption", c.to_string());
        }

        let resp = self
            .client
            .post(self.api_url("sendDocument"))
            .multipart(form)
            .send()
            .await?;

        let res: TelegramApiResponse<TelegramMessage> = resp.json().await?;
        if !res.ok {
            return Err(anyhow!(
                "Ошибка отправки документа в Telegram: {:?}",
                res.description
            ));
        }

        Ok(res.result.map(|m| m.message_id).unwrap_or(0))
    }

    pub async fn pin_chat_message(&self, chat_id: &str, message_id: i64) -> Result<()> {
        let payload = json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "disable_notification": false
        });

        let resp = self
            .client
            .post(self.api_url("pinChatMessage"))
            .json(&payload)
            .send()
            .await?;

        let res: TelegramApiResponse<bool> = resp.json().await?;
        if !res.ok {
            return Err(anyhow!(
                "Ошибка закрепления сообщения в Telegram: {:?}",
                res.description
            ));
        }

        info!("Сообщение {} закреплено в группе {}", message_id, chat_id);
        Ok(())
    }

    pub async fn unpin_chat_message(&self, chat_id: &str, message_id: i64) -> Result<()> {
        let payload = json!({
            "chat_id": chat_id,
            "message_id": message_id
        });

        let resp = self
            .client
            .post(self.api_url("unpinChatMessage"))
            .json(&payload)
            .send()
            .await?;

        let res: TelegramApiResponse<bool> = resp.json().await?;
        if !res.ok {
            return Err(anyhow!(
                "Ошибка открепления сообщения в Telegram: {:?}",
                res.description
            ));
        }

        info!("Сообщение {} откреплено в группе {}", message_id, chat_id);
        Ok(())
    }

    pub async fn get_chat_member(&self, chat_id: &str, user_id: i64) -> Result<ChatMember> {
        let resp = self
            .client
            .get(self.api_url("getChatMember"))
            .query(&[("chat_id", chat_id), ("user_id", &user_id.to_string())])
            .send()
            .await
            .context("Ошибка запроса getChatMember")?;

        let res: TelegramApiResponse<ChatMember> = resp.json().await?;
        if !res.ok {
            return Err(anyhow!(
                "Ошибка Telegram API (getChatMember): {:?}",
                res.description
            ));
        }

        res.result.ok_or_else(|| anyhow!("Пустой результат getChatMember"))
    }

    pub async fn get_chat_administrators(&self, chat_id: &str) -> Result<Vec<ChatMember>> {
        let resp = self
            .client
            .get(self.api_url("getChatAdministrators"))
            .query(&[("chat_id", chat_id)])
            .send()
            .await
            .context("Ошибка запроса getChatAdministrators")?;

        let res: TelegramApiResponse<Vec<ChatMember>> = resp.json().await?;
        if !res.ok {
            return Err(anyhow!(
                "Ошибка Telegram API (getChatAdministrators): {:?}",
                res.description
            ));
        }

        Ok(res.result.unwrap_or_default())
    }

    pub async fn restrict_chat_member(
        &self,
        chat_id: &str,
        user_id: i64,
        permissions: &ChatPermissions,
        until_date: Option<i64>,
    ) -> Result<()> {
        let mut payload = json!({
            "chat_id": chat_id,
            "user_id": user_id,
            "permissions": permissions,
            "use_independent_chat_permissions": true
        });

        if let Some(until) = until_date {
            if until > 0 {
                payload["until_date"] = json!(until);
            }
        }

        let resp = self
            .client
            .post(self.api_url("restrictChatMember"))
            .json(&payload)
            .send()
            .await
            .context("Ошибка выполнения HTTP запроса к Telegram API (restrictChatMember)")?;

        let res: TelegramApiResponse<bool> = resp.json().await?;
        if !res.ok {
            return Err(anyhow!(
                "Ошибка restrictChatMember: {:?}",
                res.description
            ));
        }

        Ok(())
    }

    pub async fn promote_chat_member(
        &self,
        chat_id: &str,
        user_id: i64,
        rights: &ChatAdminRights,
    ) -> Result<()> {
        let mut payload = serde_json::to_value(rights).unwrap_or_else(|_| json!({}));
        payload["chat_id"] = json!(chat_id);
        payload["user_id"] = json!(user_id);

        let resp = self
            .client
            .post(self.api_url("promoteChatMember"))
            .json(&payload)
            .send()
            .await
            .context("Ошибка выполнения HTTP запроса к Telegram API (promoteChatMember)")?;

        let res: TelegramApiResponse<bool> = resp.json().await?;
        if !res.ok {
            return Err(anyhow!(
                "Ошибка promoteChatMember: {:?}",
                res.description
            ));
        }

        Ok(())
    }

    pub async fn set_chat_admin_custom_title(
        &self,
        chat_id: &str,
        user_id: i64,
        custom_title: &str,
    ) -> Result<()> {
        let payload = json!({
            "chat_id": chat_id,
            "user_id": user_id,
            "custom_title": custom_title
        });

        let resp = self
            .client
            .post(self.api_url("setChatAdministratorCustomTitle"))
            .json(&payload)
            .send()
            .await
            .context("Ошибка запроса setChatAdministratorCustomTitle")?;

        let res: TelegramApiResponse<bool> = resp.json().await?;
        if !res.ok {
            return Err(anyhow!(
                "Ошибка setChatAdministratorCustomTitle: {:?}",
                res.description
            ));
        }

        Ok(())
    }

    pub async fn ban_chat_member(
        &self,
        chat_id: &str,
        user_id: i64,
        until_date: Option<i64>,
    ) -> Result<()> {
        let mut payload = json!({
            "chat_id": chat_id,
            "user_id": user_id
        });
        if let Some(until) = until_date {
            if until > 0 {
                payload["until_date"] = json!(until);
            }
        }

        let resp = self
            .client
            .post(self.api_url("banChatMember"))
            .json(&payload)
            .send()
            .await
            .context("Ошибка запроса banChatMember")?;

        let res: TelegramApiResponse<bool> = resp.json().await?;
        if !res.ok {
            return Err(anyhow!("Ошибка banChatMember: {:?}", res.description));
        }

        Ok(())
    }

    pub async fn unban_chat_member(&self, chat_id: &str, user_id: i64) -> Result<()> {
        let payload = json!({
            "chat_id": chat_id,
            "user_id": user_id,
            "only_if_banned": true
        });

        let resp = self
            .client
            .post(self.api_url("unbanChatMember"))
            .json(&payload)
            .send()
            .await
            .context("Ошибка запроса unbanChatMember")?;

        let res: TelegramApiResponse<bool> = resp.json().await?;
        if !res.ok {
            return Err(anyhow!("Ошибка unbanChatMember: {:?}", res.description));
        }

        Ok(())
    }

    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        timeout_secs: u64,
    ) -> Result<Vec<TelegramUpdate>> {
        let mut query = vec![("timeout", timeout_secs.to_string())];
        let offset_str = offset.map(|o| o.to_string());
        if let Some(ref o) = offset_str {
            query.push(("offset", o.clone()));
        }

        let resp = self
            .client
            .get(self.api_url("getUpdates"))
            .query(&query)
            .send()
            .await?;

        let res: TelegramApiResponse<Vec<TelegramUpdate>> = resp.json().await?;
        if !res.ok {
            return Err(anyhow!(
                "Ошибка getUpdates Telegram: {:?}",
                res.description
            ));
        }

        Ok(res.result.unwrap_or_default())
    }

    pub fn split_message(text: &str, max_len: usize) -> Vec<String> {
        if text.len() <= max_len {
            return vec![text.to_string()];
        }

        let mut chunks = Vec::new();
        let mut current = String::new();

        for line in text.lines() {
            if current.len() + line.len() + 1 > max_len {
                if !current.is_empty() {
                    chunks.push(current.trim().to_string());
                    current = String::new();
                }

                if line.len() > max_len {
                    let mut sub = line;
                    while sub.len() > max_len {
                        let mut split_pos = max_len;
                        while split_pos > 0 && !sub.is_char_boundary(split_pos) {
                            split_pos -= 1;
                        }
                        if split_pos == 0 {
                            split_pos = sub.chars().next().map(|c| c.len_utf8()).unwrap_or(sub.len());
                        }
                        let (part, rest) = sub.split_at(split_pos);
                        chunks.push(part.to_string());
                        sub = rest;
                    }
                    current.push_str(sub);
                    current.push('\n');
                } else {
                    current.push_str(line);
                    current.push('\n');
                }
            } else {
                current.push_str(line);
                current.push('\n');
            }
        }

        if !current.trim().is_empty() {
            chunks.push(current.trim().to_string());
        }

        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_message_utf8_boundary() {
        // Проверяем строку, в которой байт 4000 попадает ровно внутрь 2-байтового русского символа
        let text = format!("a{}", "б".repeat(3000));
        let chunks = TelegramClient::split_message(&text, 4000);
        assert!(chunks.len() >= 2);
        for chunk in chunks {
            assert!(chunk.len() <= 4000);
        }

        // Проверяем строку с эмодзи (4 байта на символ)
        let text_emoji = format!("xyz{}", "🔥".repeat(1500));
        let chunks_emoji = TelegramClient::split_message(&text_emoji, 4000);
        assert!(chunks_emoji.len() >= 2);
        for chunk in chunks_emoji {
            assert!(chunk.len() <= 4000);
        }
    }
}
