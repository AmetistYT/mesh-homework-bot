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
    ) -> Result<i64> {
        let file_bytes = tokio::fs::read(&photo_path).await?;
        let filename = photo_path
            .as_ref()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let part = Part::bytes(file_bytes)
            .file_name(filename)
            .mime_str("image/png")?;

        let mut form = Form::new()
            .text("chat_id", chat_id.to_string())
            .part("photo", part);

        if let Some(c) = caption {
            form = form.text("caption", c.to_string());
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
            "only_if_banned": false
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

    fn split_message(text: &str, max_len: usize) -> Vec<String> {
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
                        let (part, rest) = sub.split_at(max_len);
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
