use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{NaiveDate, Utc};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use super::models::{
    EventCalendarItem, EventCalendarResponse, HomeworkItem, HomeworksShortResponse, ProfileResponse,
};

const MES_API_BASE: &str = "https://school.mos.ru/api/family/mobile/v1";

#[derive(Clone)]
pub struct MeshClient {
    client: reqwest::Client,
    jwt_token: Arc<RwLock<String>>,
    student_id: i64,
    cookies: Arc<RwLock<String>>,
    person_id: Arc<RwLock<Option<String>>>,
}

impl MeshClient {
    pub fn new(jwt_token: String, student_id: i64, initial_cookies: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?;

        let initial_person_id = Self::extract_person_id_from_jwt(&jwt_token);

        Ok(Self {
            client,
            jwt_token: Arc::new(RwLock::new(jwt_token)),
            student_id,
            cookies: Arc::new(RwLock::new(initial_cookies)),
            person_id: Arc::new(RwLock::new(initial_person_id)),
        })
    }

    pub async fn get_token(&self) -> String {
        self.jwt_token.read().await.clone()
    }

    pub async fn set_token(&self, new_token: String) {
        if let Some(pid) = Self::extract_person_id_from_jwt(&new_token) {
            let mut guard = self.person_id.write().await;
            *guard = Some(pid);
        }
        let mut token = self.jwt_token.write().await;
        *token = new_token;
        info!("МЭШ токен успешно обновлен в памяти клиента");
    }

    pub fn extract_person_id_from_jwt(token: &str) -> Option<String> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() < 2 {
            return None;
        }

        let payload_part = parts[1];
        let padded = match payload_part.len() % 4 {
            2 => format!("{}==", payload_part),
            3 => format!("{}=", payload_part),
            _ => payload_part.to_string(),
        };

        let decoded = match URL_SAFE_NO_PAD.decode(padded.trim_end_matches('=')) {
            Ok(bytes) => bytes,
            Err(_) => return None,
        };

        if let Ok(val) = serde_json::from_slice::<Value>(&decoded) {
            if let Some(msh) = val.get("msh").and_then(|v| v.as_str()) {
                if !msh.trim().is_empty() {
                    return Some(msh.trim().to_string());
                }
            }
        }
        None
    }

    pub async fn get_or_fetch_person_id(&self) -> Result<String> {
        {
            let guard = self.person_id.read().await;
            if let Some(ref pid) = *guard {
                return Ok(pid.clone());
            }
        }

        let token = self.get_token().await;
        if let Some(pid) = Self::extract_person_id_from_jwt(&token) {
            let mut guard = self.person_id.write().await;
            *guard = Some(pid.clone());
            return Ok(pid);
        }

        let profile = self.get_profile().await?;
        if let Some(children) = profile.children {
            if let Some(first) = children.first() {
                if let Some(ref guid) = first.contingent_guid {
                    let mut guard = self.person_id.write().await;
                    *guard = Some(guid.clone());
                    return Ok(guid.clone());
                }
                if let Some(ref pid) = first.person_id {
                    let mut guard = self.person_id.write().await;
                    *guard = Some(pid.clone());
                    return Ok(pid.clone());
                }
            }
        }

        Err(anyhow!("Не удалось определить person_id / contingent_guid ученика"))
    }

    pub async fn set_cookies(&self, new_cookies: String) {
        let mut cookies = self.cookies.write().await;
        *cookies = new_cookies;
        info!("МЭШ cookies обновлены в памяти клиента");
    }

    pub async fn is_token_expired(&self) -> bool {
        let token = self.jwt_token.read().await;
        Self::check_jwt_expired(&token)
    }

    pub fn check_jwt_expired(token: &str) -> bool {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() < 2 {
            return true;
        }

        let payload_part = parts[1];
        // Добавляем padding при необходимости для base64
        let padded = match payload_part.len() % 4 {
            2 => format!("{}==", payload_part),
            3 => format!("{}=", payload_part),
            _ => payload_part.to_string(),
        };

        let decoded = match URL_SAFE_NO_PAD.decode(padded.trim_end_matches('=')) {
            Ok(bytes) => bytes,
            Err(_) => return true,
        };

        if let Ok(val) = serde_json::from_slice::<Value>(&decoded) {
            if let Some(exp) = val.get("exp").and_then(|e| e.as_i64()) {
                let current_time = Utc::now().timestamp();
                // Запас 60 секунд до истечения
                return current_time >= (exp - 60);
            }
        }

        true
    }

    fn build_headers(&self, token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let auth_val = format!("Bearer {}", token.trim().trim_start_matches("Bearer "));
        let raw_token = token.trim().trim_start_matches("Bearer ").trim();

        if let Ok(v) = HeaderValue::from_str(&auth_val) {
            headers.insert(AUTHORIZATION, v);
        }
        if let Ok(v) = HeaderValue::from_str(raw_token) {
            headers.insert("Auth-Token", v);
        }
        if let Ok(v) = HeaderValue::from_str(&self.student_id.to_string()) {
            headers.insert("Profile-Id", v);
        }
        headers.insert("x-mes-subsystem", HeaderValue::from_static("familymp"));
        headers.insert("client-type", HeaderValue::from_static("diary-mobile"));
        headers.insert(USER_AGENT, HeaderValue::from_static("okhttp/4.12.0"));
        headers
    }

    pub async fn get_profile(&self) -> Result<ProfileResponse> {
        let token = self.get_token().await;
        let url = format!("{}/profile", MES_API_BASE);

        let resp = self
            .client
            .get(&url)
            .headers(self.build_headers(&token))
            .send()
            .await
            .context("Ошибка отправки запроса к МЭШ /profile")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Ошибка получения профиля: {} - {}", status, body));
        }

        let profile = resp.json::<ProfileResponse>().await?;
        Ok(profile)
    }

    pub async fn get_homeworks(
        &self,
        from_date: NaiveDate,
        to_date: NaiveDate,
    ) -> Result<Vec<HomeworkItem>> {
        let token = self.get_token().await;
        let url = format!("{}/homeworks/short", MES_API_BASE);

        let resp = self
            .client
            .get(&url)
            .headers(self.build_headers(&token))
            .query(&[
                ("student_id", self.student_id.to_string()),
                ("from", from_date.format("%Y-%m-%d").to_string()),
                ("to", to_date.format("%Y-%m-%d").to_string()),
                ("sort_column", "date".to_string()),
                ("sort_direction", "asc".to_string()),
            ])
            .send()
            .await
            .context("Ошибка отправки запроса /homeworks/short")?;

        if !resp.status().is_success() {
            let status = resp.status();
            if status.as_u16() == 401 {
                tracing::warn!("Получен 401 Unauthorized от МЭШ API. Пробуем автообновление токена...");
                if let Ok(Some(new_token)) = self.try_refresh_sudir().await {
                    tracing::info!("Токен успешно обновлен через СУДИР, повторяем запрос ДЗ");
                    let retry_resp = self
                        .client
                        .get(&url)
                        .headers(self.build_headers(&new_token))
                        .query(&[
                            ("student_id", self.student_id.to_string()),
                            ("from", from_date.format("%Y-%m-%d").to_string()),
                            ("to", to_date.format("%Y-%m-%d").to_string()),
                            ("sort_column", "date".to_string()),
                            ("sort_direction", "asc".to_string()),
                        ])
                        .send()
                        .await
                        .context("Ошибка повторного запроса /homeworks/short")?;

                    if retry_resp.status().is_success() {
                        let body: HomeworksShortResponse = retry_resp.json().await?;
                        return Ok(body.payload.unwrap_or_default());
                    }
                }
            }
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("МЭШ API вернул ошибку {}: {}", status, body));
        }

        let body: HomeworksShortResponse = resp.json().await?;
        Ok(body.payload.unwrap_or_default())
    }

    pub async fn mark_homework_done(&self, homework_id: i64) -> Result<()> {
        let token = self.get_token().await;
        let url = format!("{}/homeworks/{}/done", MES_API_BASE, homework_id);

        let resp = self
            .client
            .post(&url)
            .headers(self.build_headers(&token))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Не удалось отметить ДЗ выполненным: {} - {}", status, text));
        }

        Ok(())
    }

    pub async fn get_events(
        &self,
        from_date: NaiveDate,
        to_date: NaiveDate,
    ) -> Result<Vec<EventCalendarItem>> {
        let token = self.get_token().await;
        let person_id = self.get_or_fetch_person_id().await?;
        let url = "https://school.mos.ru/api/eventcalendar/v1/api/events";

        let mut headers = self.build_headers(&token);
        headers.insert("x-mes-role", HeaderValue::from_static("student"));

        let resp = self
            .client
            .get(url)
            .headers(headers)
            .query(&[
                ("person_ids", person_id.as_str()),
                ("begin_date", &from_date.format("%Y-%m-%d").to_string()),
                ("end_date", &to_date.format("%Y-%m-%d").to_string()),
                ("expand", "homework,materials,marks"),
            ])
            .send()
            .await
            .context("Ошибка отправки запроса к МЭШ /api/events")?;

        if !resp.status().is_success() {
            let status = resp.status();
            if status.as_u16() == 401 {
                tracing::warn!("Получен 401 от EventCalendar API. Пробуем автообновление токена...");
                if let Ok(Some(new_token)) = self.try_refresh_sudir().await {
                    let mut retry_headers = self.build_headers(&new_token);
                    retry_headers.insert("x-mes-role", HeaderValue::from_static("student"));
                    let retry_resp = self
                        .client
                        .get(url)
                        .headers(retry_headers)
                        .query(&[
                            ("person_ids", person_id.as_str()),
                            ("begin_date", &from_date.format("%Y-%m-%d").to_string()),
                            ("end_date", &to_date.format("%Y-%m-%d").to_string()),
                            ("expand", "homework,materials,marks"),
                        ])
                        .send()
                        .await
                        .context("Ошибка повторного запроса к eventcalendar API")?;

                    if retry_resp.status().is_success() {
                        let body: EventCalendarResponse = retry_resp.json().await?;
                        return Ok(body.response.unwrap_or_default());
                    }
                }
            }
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("МЭШ EventCalendar API вернул ошибку {}: {}", status, body));
        }

        let body: EventCalendarResponse = resp.json().await?;
        Ok(body.response.unwrap_or_default())
    }

    /// Полноценное обновление сессии через school.mos.ru/v3/auth/sudir/login flow
    /// по логике mesh_expressive (TokenUtils + BackgroundTokenRefresher)
    pub async fn try_refresh_sudir(&self) -> Result<Option<String>> {
        let current_token = self.get_token().await;
        let mut cookies = self.cookies.read().await.clone();
        if cookies.trim().is_empty() {
            cookies = format!(
                "auth_token={}; aupd_token={}; aupd_current_role=2:1; profile_id={}",
                current_token, current_token, self.student_id
            );
        }

        info!("Попытка автоматического обновления токена через sudir/login flow");
        let login_url = "https://school.mos.ru/v3/auth/sudir/login";

        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Linux; Android 13; Pixel 7 Pro Build/TD1A.220804.031) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36",
            ),
        );
        if let Ok(c) = HeaderValue::from_str(&cookies) {
            headers.insert("Cookie", c);
        }

        let resp = self.client.get(login_url).headers(headers.clone()).send().await?;
        let mut all_cookies_str = cookies.clone();
        let mut new_token = None;

        for (k, v) in resp.headers() {
            if k == reqwest::header::SET_COOKIE {
                if let Ok(cookie_val) = v.to_str() {
                    all_cookies_str.push_str("; ");
                    all_cookies_str.push_str(cookie_val);

                    if let Some(t) = Self::extract_token_from_cookie(cookie_val) {
                        new_token = Some(t);
                    }
                }
            }
        }

        // Если токен не получен на первом шаге, проверяем HTML meta refresh
        let body = resp.text().await.unwrap_or_default();
        if new_token.is_none() {
            if let Some(idx) = body.find("url=") {
                let rest = &body[idx + 4..];
                let next_url = rest.split('"').next().unwrap_or("").replace("&amp;", "&");
                if next_url.starts_with("http") {
                    info!("Следование по meta-refresh к OAuth: {}", &next_url[..std::cmp::min(80, next_url.len())]);
                    let mut step_headers = HeaderMap::new();
                    step_headers.insert(
                        USER_AGENT,
                        HeaderValue::from_static(
                            "Mozilla/5.0 (Linux; Android 13; Pixel 7 Pro Build/TD1A.220804.031) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36",
                        ),
                    );
                    if let Ok(c) = HeaderValue::from_str(&all_cookies_str) {
                        step_headers.insert("Cookie", c);
                    }

                    if let Ok(step_resp) = self.client.get(&next_url).headers(step_headers).send().await {
                        for (k, v) in step_resp.headers() {
                            if k == reqwest::header::SET_COOKIE {
                                if let Ok(cookie_val) = v.to_str() {
                                    all_cookies_str.push_str("; ");
                                    all_cookies_str.push_str(cookie_val);

                                    if let Some(t) = Self::extract_token_from_cookie(cookie_val) {
                                        new_token = Some(t);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        self.set_cookies(all_cookies_str).await;

        if let Some(tok) = new_token {
            self.set_token(tok.clone()).await;
            info!("Успешно получен новый МЭШ токен через СУДИР!");
            return Ok(Some(tok));
        }

        Ok(None)
    }

    /// Извлечение валидного токена из cookies по алгоритму TokenUtils.kt
    pub fn extract_token_from_cookie(cookie_str: &str) -> Option<String> {
        let mut auth_token = None;
        let mut aupd_token = None;
        let mut mes_session = None;

        for part in cookie_str.split(';') {
            let p = part.trim();
            if let Some(val) = p.strip_prefix("auth_token=") {
                let clean = val.split(';').next().unwrap_or(val).trim();
                if Self::is_token_valid_and_not_expired(clean) {
                    auth_token = Some(clean.to_string());
                }
            } else if let Some(val) = p.strip_prefix("aupd_token=") {
                let clean = val.split(';').next().unwrap_or(val).trim();
                if Self::is_token_valid_and_not_expired(clean) {
                    aupd_token = Some(clean.to_string());
                }
            } else if let Some(val) = p.strip_prefix("mes_session=") {
                let clean = val.split(';').next().unwrap_or(val).trim();
                if Self::is_token_valid_and_not_expired(clean) {
                    mes_session = Some(clean.to_string());
                }
            }
        }

        auth_token.or(mes_session).or(aupd_token)
    }

    pub fn is_token_valid_and_not_expired(token: &str) -> bool {
        let t = token.trim();
        if t.len() <= 20 {
            return false;
        }
        !Self::check_jwt_expired(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mesh_get_events() {
        let _ = dotenvy::dotenv();
        let token = std::env::var("MESH_JWT_TOKEN").unwrap_or_default();
        let student_id: i64 = std::env::var("MESH_STUDENT_ID")
            .unwrap_or_else(|_| "17486681".to_string())
            .parse()
            .unwrap_or(17486681);
        if token.is_empty() {
            println!("Пропуск теста: MESH_JWT_TOKEN не задан");
            return;
        }

        let client = MeshClient::new(token, student_id, String::new()).unwrap();
        let today = chrono::Local::now().date_naive();
        let events = client
            .get_events(
                today - chrono::Duration::days(2),
                today + chrono::Duration::days(5),
            )
            .await;
        assert!(events.is_ok(), "Ошибка запроса событий: {:?}", events.err());
        let evs = events.unwrap();
        println!("Найдено событий: {}", evs.len());
        assert!(!evs.is_empty());
    }
}
