use anyhow::{anyhow, Context, Result};
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::Deserialize;
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
    pub date: i64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramUser {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub username: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
    pub title: Option<String>,
    pub username: Option<String>,
    #[serde(rename = "type")]
    pub chat_type: String,
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
