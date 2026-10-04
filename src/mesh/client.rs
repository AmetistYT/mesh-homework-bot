use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{NaiveDate, Utc};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::db::Database;
use super::cdz_client::CdzAssignmentInfo;
use super::models::{
    EventCalendarItem, EventCalendarResponse, HomeworkItem, HomeworksShortResponse, ProfileResponse,
};


const MES_API_BASE: &str = "https://school.mos.ru/api/family/mobile/v1";

#[derive(Clone)]
pub struct MeshClient {
    client: reqwest::Client,
    jwt_token: Arc<RwLock<String>>,
    refresh_token: Arc<RwLock<Option<String>>>,
    student_id: i64,
    cookies: Arc<RwLock<String>>,
    person_id: Arc<RwLock<Option<String>>>,
    db: Option<Database>,
}

impl MeshClient {
    pub fn new(
        jwt_token: String,
        refresh_token: Option<String>,
        student_id: i64,
        initial_cookies: String,
        db: Option<Database>,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()?;

        let initial_person_id = Self::extract_person_id_from_jwt(&jwt_token);

        Ok(Self {
            client,
            jwt_token: Arc::new(RwLock::new(jwt_token)),
            refresh_token: Arc::new(RwLock::new(refresh_token)),
            student_id,
            cookies: Arc::new(RwLock::new(initial_cookies)),
            person_id: Arc::new(RwLock::new(initial_person_id)),
            db,
        })
    }

    pub async fn get_token(&self) -> String {
        self.jwt_token.read().await.clone()
    }

    pub async fn get_refresh_token(&self) -> Option<String> {
        self.refresh_token.read().await.clone()
    }

    pub fn student_id(&self) -> i64 {
        self.student_id
    }

    pub async fn get_cookies(&self) -> String {
        self.cookies.read().await.clone()
    }


    pub async fn set_refresh_token(&self, new_rt: String) {
        let mut guard = self.refresh_token.write().await;
        *guard = Some(new_rt);
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
        if !Self::check_jwt_expired(&token) {
            return false;
        }
        drop(token);

        // Если токен в памяти истек, проверяем, нет ли свежего токена в БД
        if let Some(ref db) = self.db {
            if let Ok(Some(db_token)) = db.get_setting("mesh_token") {
                if !Self::check_jwt_expired(&db_token) {
                    info!("Обнаружен валидный токен в БД, обновляем кэш клиента...");
                    self.set_token(db_token).await;
                    if let Ok(Some(db_rt)) = db.get_setting("mesh_refresh_token") {
                        if !db_rt.is_empty() {
                            self.set_refresh_token(db_rt).await;
                        }
                    }
                    return false;
                }
            }
        }
        true
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

    pub fn clean_mesh_error_message(status: reqwest::StatusCode, raw_body: &str) -> String {
        let trimmed = raw_body.trim();
        if trimmed.is_empty() {
            return format!("{}", status);
        }
        if trimmed.starts_with("<!DOCTYPE") || trimmed.starts_with("<html") || trimmed.contains("<body") {
            if status.is_server_error() {
                format!("{} (МЭШ временно недоступен или технические работы)", status)
            } else {
                format!("{} (сервер вернул HTML вместо данных)", status)
            }
        } else if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(msg) = val.get("message").and_then(|m| m.as_str()) {
                format!("{}: {}", status, msg)
            } else if let Some(err) = val.get("error").and_then(|e| e.as_str()) {
                format!("{}: {}", status, err)
            } else {
                let s = val.to_string();
                if s.len() > 150 {
                    format!("{} (детали ошибки сокращены)", status)
                } else {
                    format!("{}: {}", status, s)
                }
            }
        } else if trimmed.len() > 200 {
            format!("{} (некорректный ответ сервера)", status)
        } else {
            format!("{}: {}", status, trimmed)
        }
    }

    pub async fn get_profile(&self) -> Result<ProfileResponse> {
        if self.is_token_expired().await {
            let _ = self.try_refresh().await;
        }

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
            if status.as_u16() == 401 {
                warn!("Получен 401 Unauthorized от МЭШ /profile. Пробуем автообновление токена...");
                if let Ok(Some(new_token)) = self.try_refresh().await {
                    let retry_resp = self
                        .client
                        .get(&url)
                        .headers(self.build_headers(&new_token))
                        .send()
                        .await
                        .context("Ошибка повторного запроса к МЭШ /profile")?;

                    if retry_resp.status().is_success() {
                        let profile = retry_resp.json::<ProfileResponse>().await?;
                        return Ok(profile);
                    }
                }
            }
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Ошибка получения профиля: {}", Self::clean_mesh_error_message(status, &body)));
        }

        let profile = resp.json::<ProfileResponse>().await?;
        Ok(profile)
    }

    pub async fn get_homeworks(
        &self,
        from_date: NaiveDate,
        to_date: NaiveDate,
    ) -> Result<Vec<HomeworkItem>> {
        if self.is_token_expired().await {
            let _ = self.try_refresh().await;
        }

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
                warn!("Получен 401 Unauthorized от МЭШ API. Пробуем автообновление токена...");
                if let Ok(Some(new_token)) = self.try_refresh().await {
                    info!("Токен успешно обновлен, повторяем запрос ДЗ");
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
            return Err(anyhow!("МЭШ API вернул ошибку {}", Self::clean_mesh_error_message(status, &body)));
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
            if status.as_u16() == 401 {
                if let Ok(Some(new_token)) = self.try_refresh().await {
                    let retry = self
                        .client
                        .post(&url)
                        .headers(self.build_headers(&new_token))
                        .send()
                        .await?;
                    if retry.status().is_success() {
                        return Ok(());
                    }
                }
            }
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Не удалось отметить ДЗ выполненным: {}", Self::clean_mesh_error_message(status, &text)));
        }

        Ok(())
    }

    pub async fn get_events(
        &self,
        from_date: NaiveDate,
        to_date: NaiveDate,
    ) -> Result<Vec<EventCalendarItem>> {
        if self.is_token_expired().await {
            let _ = self.try_refresh().await;
        }

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
                warn!("Получен 401 от EventCalendar API. Пробуем автообновление токена...");
                if let Ok(Some(new_token)) = self.try_refresh().await {
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
            return Err(anyhow!("МЭШ EventCalendar API вернул ошибку {}", Self::clean_mesh_error_message(status, &body)));
        }

        let body: EventCalendarResponse = resp.json().await?;
        Ok(body.response.unwrap_or_default())
    }

    /// Поиск цифровых домашних заданий (ЦДЗ) за заданный диапазон дат
    pub async fn fetch_cdz_assignments(&self, from_date: NaiveDate, to_date: NaiveDate) -> Result<Vec<CdzAssignmentInfo>> {
        let events = self.get_events(from_date, to_date).await?;
        let token = self.get_token().await;
        let mut results = Vec::new();

        for ev in events {
            let ev_id = match ev.id {
                Some(id) => id,
                None => continue,
            };

            if ev.homework.is_none() {
                continue;
            }

            let detail_url = format!("{}/lesson_schedule_items/{}", MES_API_BASE, ev_id);
            let resp = self.client.get(&detail_url)
                .headers(self.build_headers(&token))
                .query(&[("student_id", self.student_id.to_string())])
                .send()
                .await;

            if let Ok(r) = resp {
                if r.status().is_success() {
                    if let Ok(json_val) = r.json::<serde_json::Value>().await {
                        if let Some(lesson_hws) = json_val["lesson_homeworks"].as_array() {
                            for hw in lesson_hws {
                                let hw_entry_id = hw["homework_entry_id"].as_i64()
                                    .or_else(|| hw["homework_entry_student_id"].as_i64())
                                    .unwrap_or(0);
                                let hw_text = hw["homework"].as_str().unwrap_or_default().to_string();
                                let is_done = hw["is_done"].as_bool().unwrap_or(false);

                                if let Some(materials) = hw["additional_materials"].as_array() {
                                    for mat in materials {
                                        let mat_type = mat["type"].as_str().unwrap_or_default().to_lowercase();
                                        let mat_type_name = mat["type_name"].as_str().unwrap_or_default().to_lowercase();
                                        let mat_sub_type = mat["sub_type"].as_str().unwrap_or_default().to_lowercase();
                                        let uuid = mat["uuid"].as_str().unwrap_or_default();
                                        let mat_title = mat["title"].as_str().unwrap_or_default();
                                        let mat_title_lower = mat_title.to_lowercase();

                                        if uuid.is_empty() {
                                            continue;
                                        }

                                        // Считаем ЦДЗ ТОЛЬКО материалы с явной пометкой 'тест' в МЭШ:
                                        // тип test_spec_binding/test, type_name "тест", sub_type "test" или "тест" в названии материала.
                                        let is_explicit_test = mat_type == "test_spec_binding"
                                            || mat_type.contains("test")
                                            || mat_type_name.contains("тест")
                                            || mat_sub_type.contains("test")
                                            || mat_title_lower.contains("тест")
                                            || mat_title_lower.contains("тестирован");

                                        if is_explicit_test {
                                            results.push(CdzAssignmentInfo {
                                                homework_entry_id: hw_entry_id,
                                                material_uuid: uuid.to_string(),
                                                material_title: mat_title.to_string(),
                                                subject_name: ev.subject_name.clone().unwrap_or_else(|| "Неизвестный предмет".to_string()),
                                                description: hw_text.clone(),
                                                date: ev.start_at.clone().unwrap_or_default(),
                                                is_done,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        results.dedup_by(|a, b| a.homework_entry_id == b.homework_entry_id && a.material_uuid == b.material_uuid);
        Ok(results)
    }


    /// Обновление пары токенов через официальный эндпоинт МЭШ /v3/token/refresh
    pub async fn refresh_token_v3(&self) -> Result<Option<String>> {
        let rt_opt = {
            let guard = self.refresh_token.read().await;
            guard.clone()
        };

        let rt = match rt_opt {
            Some(ref t) if !t.trim().is_empty() => t.clone(),
            _ => {
                if let Some(ref db) = self.db {
                    if let Ok(Some(db_rt)) = db.get_setting("mesh_refresh_token") {
                        if !db_rt.trim().is_empty() {
                            db_rt
                        } else {
                            let cookies = self.cookies.read().await;
                            Self::extract_refresh_token_from_cookie(&cookies).unwrap_or_default()
                        }
                    } else {
                        let cookies = self.cookies.read().await;
                        Self::extract_refresh_token_from_cookie(&cookies).unwrap_or_default()
                    }
                } else {
                    let cookies = self.cookies.read().await;
                    Self::extract_refresh_token_from_cookie(&cookies).unwrap_or_default()
                }
            }
        };

        if rt.trim().is_empty() {
            warn!("Refresh token не найден для вызова /v3/token/refresh");
            return Ok(None);
        }

        info!("Попытка автообновления токена через /v3/token/refresh...");
        let url = "https://school.mos.ru/v3/token/refresh";
        let resp = self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(reqwest::header::USER_AGENT, "okhttp/4.12.0")
            .body(format!("refresh_token={}", rt.trim()))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!("Ошибка /v3/token/refresh ({}): {}", status, body);
            return Ok(None);
        }

        let body: Value = resp.json().await?;
        let new_access = body.get("access_token").and_then(|v| v.as_str()).map(|s| s.to_string());
        let new_refresh = body.get("refresh_token").and_then(|v| v.as_str()).map(|s| s.to_string());

        if let Some(access) = new_access {
            self.set_token(access.clone()).await;
            if let Some(ref rtok) = new_refresh {
                self.set_refresh_token(rtok.clone()).await;
            }
            self.save_tokens(&access, new_refresh.as_deref().unwrap_or(&rt)).await;
            info!("МЭШ JWT токен успешно обновлен навсегда через /v3/token/refresh!");
            return Ok(Some(access));
        }

        Ok(None)
    }

    /// Автообновление токена через сохраненные сессионные куки СУДИР/mos.ru (логика mesh_expressive)
    pub async fn refresh_from_sso(&self) -> Result<Option<String>> {
        info!("Попытка автообновления токена через СУДИР SSO (refresh_mesh_sso.py)...");
        let script_paths = [
            "scripts/refresh_mesh_sso.py",
            "/root/projects/mesh-homework-bot/scripts/refresh_mesh_sso.py",
            "/home/ametist1337/projects/mesh-homework-bot/scripts/refresh_mesh_sso.py",
        ];

        let mut script_path = None;
        for p in &script_paths {
            if std::path::Path::new(p).exists() {
                script_path = Some(*p);
                break;
            }
        }

        let script = match script_path {
            Some(p) => p,
            None => {
                warn!("Скрипт refresh_mesh_sso.py не найден");
                return Ok(None);
            }
        };

        let output = tokio::process::Command::new("python3")
            .arg(script)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("refresh_mesh_sso.py завершился с ошибкой: {}", stderr);
            return Ok(None);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Ok(val) = serde_json::from_str::<Value>(&stdout) {
            if val.get("status").and_then(|s| s.as_str()) == Some("ok") {
                if let Some(new_tok) = val.get("token").and_then(|t| t.as_str()) {
                    let tok_str = new_tok.to_string();
                    self.set_token(tok_str.clone()).await;
                    if let Some(ref db) = self.db {
                        let _ = db.set_setting("mesh_token", &tok_str);
                    }
                    if let Some(rt) = val.get("refresh_token").and_then(|t| t.as_str()) {
                        self.set_refresh_token(rt.to_string()).await;
                        if let Some(ref db) = self.db {
                            let _ = db.set_setting("mesh_refresh_token", rt);
                        }
                    }
                    info!("МЭШ токен успешно обновлен навсегда через СУДИР SSO!");
                    return Ok(Some(tok_str));
                }
            }
        }

        Ok(None)
    }

    /// Единый метод обновления токена МЭШ со всеми резервными цепочками
    pub async fn try_refresh(&self) -> Result<Option<String>> {
        // 0. Приоритет: проверяем, нет ли уже свежего действующего токена в БД
        if let Some(ref db) = self.db {
            if let Ok(Some(db_token)) = db.get_setting("mesh_token") {
                let current_token = self.get_token().await;
                if !Self::check_jwt_expired(&db_token) && db_token != current_token {
                    info!("Свежий МЭШ токен получен напрямую из БД!");
                    self.set_token(db_token.clone()).await;
                    if let Ok(Some(db_rt)) = db.get_setting("mesh_refresh_token") {
                        if !db_rt.is_empty() {
                            self.set_refresh_token(db_rt).await;
                        }
                    }
                    return Ok(Some(db_token));
                }
            }
        }

        // 1. Приоритет: SSO автообновление через сессионные куки СУДИР/mos.ru
        if let Ok(Some(tok)) = self.refresh_from_sso().await {
            return Ok(Some(tok));
        }

        // 2. Резерв: официальный refresh токена АУПД /v3/token/refresh
        if let Ok(Some(tok)) = self.refresh_token_v3().await {
            return Ok(Some(tok));
        }

        // 3. Резерв: прямой sudir flow через reqwest
        if let Ok(Some(tok)) = self.try_refresh_sudir().await {
            return Ok(Some(tok));
        }

        warn!("Все методы автообновления токена не дали результата");
        Ok(None)
    }

    /// Превентивное автообновление, если до истечения токена осталось менее 30 минут
    pub async fn check_and_proactive_refresh(&self) -> Result<bool> {
        let token = self.jwt_token.read().await;
        if Self::check_jwt_expires_within(&token, 1800) {
            drop(token);
            info!("МЭШ токен истекает менее чем через 30 минут. Превентивное автообновление...");
            let res = self.try_refresh().await?;
            return Ok(res.is_some());
        }
        Ok(false)
    }

    pub fn check_jwt_expires_within(token: &str, seconds: i64) -> bool {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() < 2 {
            return true;
        }

        let payload_part = parts[1];
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
                return current_time >= (exp - seconds);
            }
        }

        true
    }

    pub async fn save_tokens(&self, access_token: &str, refresh_token: &str) {
        if let Some(ref db) = self.db {
            let _ = db.set_setting("mesh_token", access_token);
            let _ = db.set_setting("mesh_refresh_token", refresh_token);
        }
        Self::update_env_file("MESH_JWT_TOKEN", access_token);
        Self::update_env_file("MESH_REFRESH_TOKEN", refresh_token);
    }

    pub fn update_env_file(key: &str, value: &str) {
        let paths = [
            ".env",
            "/root/projects/mesh-homework-bot/.env",
            "/home/ametist1337/projects/mesh-homework-bot/.env",
        ];
        for p in &paths {
            let path = std::path::Path::new(p);
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(path) {
                    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
                    let prefix = format!("{}=", key);
                    let mut found = false;
                    for l in &mut lines {
                        if l.starts_with(&prefix) {
                            *l = format!("{}{}", prefix, value);
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        lines.push(format!("{}{}", prefix, value));
                    }
                    let _ = std::fs::write(path, lines.join("\n") + "\n");
                }
            }
        }
    }

    pub fn extract_refresh_token_from_cookie(cookie_str: &str) -> Option<String> {
        for part in cookie_str.split(';') {
            let p = part.trim();
            if let Some(val) = p.strip_prefix("aupd_refresh_token=") {
                let clean = val.split(';').next().unwrap_or(val).trim();
                if clean.len() > 20 {
                    return Some(clean.to_string());
                }
            }
        }
        None
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
    #[ignore]
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

        let client = MeshClient::new(token, None, student_id, String::new(), None).unwrap();
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
    }

    #[tokio::test]
    #[ignore]
    async fn test_mesh_fetch_cdz() {
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

        let client = MeshClient::new(token, None, student_id, String::new(), None).unwrap();
        let today = chrono::Local::now().date_naive();
        let cdz = client
            .fetch_cdz_assignments(
                today - chrono::Duration::days(14),
                today + chrono::Duration::days(14),
            )
            .await;
        assert!(cdz.is_ok(), "Ошибка запроса ЦДЗ: {:?}", cdz.err());
        let list = cdz.unwrap();
        println!("Найдено ЦДЗ: {}", list.len());
        for item in &list {
            println!("  - [{}] {} (материал: '{}') id: {}", item.subject_name, item.description, item.material_title, item.homework_entry_id);
        }
    }
}
