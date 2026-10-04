use anyhow::{anyhow, Context, Result};
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdzAssignmentInfo {
    pub homework_entry_id: i64,
    pub material_uuid: String,
    pub material_title: String,
    pub subject_name: String,
    pub description: String,
    pub date: String,
    pub is_done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdzLaunchParams {
    pub actor: String,
    pub activity_id: String,
    pub registration: String,
    pub context: String,
    pub challenge_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdzSession {
    pub access_token_eom: String,
    pub profile_id: i64,
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdzOption {
    pub id: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default, rename = "type")]
    pub option_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdzQuestionElement {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default, rename = "type")]
    pub element_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdzTaskAnswer {
    #[serde(rename = "type")]
    pub answer_type: String,
    #[serde(default)]
    pub options: Vec<CdzOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdzTask {
    pub id: i64,
    pub task_order: Option<usize>,
    #[serde(default)]
    pub question_elements: Vec<CdzQuestionElement>,
    pub answer: CdzTaskAnswer,
    #[serde(default)]
    pub user_answer: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdzTestGroup {
    pub test_group_num: Option<usize>,
    #[serde(default)]
    pub challenge_tasks: Vec<CdzTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdzAttempt {
    pub challenge_attempt_id: i64,
    pub challenge_id: String,
    pub test_groups: Vec<CdzTestGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdzResult {
    pub attempt_id: i64,
    pub score: Option<f64>,
    pub max_score: Option<f64>,
    pub percent_complete: Option<f64>,
    pub raw: Option<serde_json::Value>,
}

#[derive(Clone)]
pub struct CdzClient {
    client: reqwest::Client,
    cookies_jar: Arc<reqwest::cookie::Jar>,
    diary_token: String,
    student_id: i64,
    raw_cookies_json: String,
}

impl CdzClient {
    pub fn new(cookies_json: &str, diary_token: &str, student_id: i64) -> Result<Self> {
        let jar = Arc::new(reqwest::cookie::Jar::default());

        if let Ok(cookies_list) = serde_json::from_str::<Vec<serde_json::Value>>(cookies_json) {
            let base_url = "https://school.mos.ru".parse::<reqwest::Url>().unwrap();
            let uchebnik_url = "https://uchebnik.mos.ru".parse::<reqwest::Url>().unwrap();

            for ck in cookies_list {
                if let (Some(name), Some(value)) = (ck["name"].as_str(), ck["value"].as_str()) {
                    if name.starts_with("_ym") || name.starts_with("fp_") {
                        continue;
                    }
                    let domain = ck["domain"].as_str().unwrap_or(".mos.ru");
                    let path = ck["path"].as_str().unwrap_or("/");
                    let cookie_str = format!("{}={}; Domain={}; Path={}", name, value, domain, path);

                    if domain.contains("uchebnik") {
                        jar.add_cookie_str(&cookie_str, &uchebnik_url);
                    } else if domain.contains("login") {
                        if let Ok(login_url) = "https://login.mos.ru".parse::<reqwest::Url>() {
                            jar.add_cookie_str(&cookie_str, &login_url);
                        }
                    } else {
                        jar.add_cookie_str(&cookie_str, &base_url);
                    }
                }
            }
        }

        // Гарантируем наличие токенов авторизации для school.mos.ru и uchebnik.mos.ru
        for host in &["https://school.mos.ru", "https://uchebnik.mos.ru"] {
            if let Ok(u) = host.parse::<reqwest::Url>() {
                jar.add_cookie_str(&format!("auth_token={}; Domain=.mos.ru; Path=/", diary_token), &u);
                jar.add_cookie_str(&format!("aupd_token={}; Domain=.mos.ru; Path=/", diary_token), &u);
            }
        }

        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"),
        );

        let client = reqwest::Client::builder()
            .cookie_provider(jar.clone())
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()?;

        Ok(Self {
            client,
            cookies_jar: jar,
            diary_token: diary_token.to_string(),
            student_id,
            raw_cookies_json: cookies_json.to_string(),
        })
    }

    /// Шаг 1-2: Получение launcher-ссылки из дневника и трансформация в подписанный xAPI URL плеера тестов
    pub async fn resolve_launcher_params(&self, homework_entry_id: i64, material_uuid: &str) -> Result<CdzLaunchParams> {
        info!("Запрос launcher-ссылки для ДЗ #{} (материал: {})...", homework_entry_id, material_uuid);

        let launch_req_url = format!(
            "https://school.mos.ru/api/ej/partners/v1/homeworks/launch?homework_entry_id={}&material_id={}",
            homework_entry_id, material_uuid
        );

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", self.diary_token))?);
        headers.insert("Auth-Token", HeaderValue::from_str(&self.diary_token)?);
        headers.insert("Profile-Id", HeaderValue::from_str(&self.student_id.to_string())?);
        headers.insert("x-mes-subsystem", HeaderValue::from_static("familymp"));
        headers.insert("client-type", HeaderValue::from_static("diary-mobile"));

        let resp = self.client.get(&launch_req_url).headers(headers).send().await
            .context("Сетевая ошибка при запросе launcher ссылки дневника")?;

        let status = resp.status();
        let loc_header = resp.headers().get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let body = resp.text().await.unwrap_or_default().trim().to_string();
        let launch_v1_launch = if body.starts_with("http") {
            body
        } else if let Some(loc) = loc_header {
            loc
        } else {
            body
        };

        if launch_v1_launch.is_empty() || !launch_v1_launch.starts_with("http") {
            return Err(anyhow!("Сервер дневника не вернул ссылку лаунчера для ДЗ #{} (статус {}, ответ: '{}')", homework_entry_id, status, launch_v1_launch));
        }

        info!("Получена ссылка лаунчера: {}", &launch_v1_launch[..std::cmp::min(100, launch_v1_launch.len())]);

        // Шаг 2: замена /v1/launch на /v1/url и получение итогового подписанного URL плеера
        let launch_v1_url = launch_v1_launch.replace("/v1/launch", "/v1/url");
        let resp2 = self.client.get(&launch_v1_url).send().await
            .context("Ошибка вызова /v1/url лаунчера")?;

        let resp2_status = resp2.status();
        let player_url_str = resp2.text().await.unwrap_or_default().trim().to_string();
        if !player_url_str.contains("activityId=") {
            return Err(anyhow!("Ответ /v1/url не содержит activityId (статус {}): {}", resp2_status, player_url_str));
        }

        info!("Получен подписанный xAPI URL плеера тестов: {}", &player_url_str[..std::cmp::min(100, player_url_str.len())]);

        let parsed_url = reqwest::Url::parse(&player_url_str)
            .context("Не удалось распарсить URL плеера тестов")?;

        let mut actor = String::new();
        let mut activity_id = String::new();
        let mut registration = String::new();
        let mut context = String::new();

        for (k, v) in parsed_url.query_pairs() {
            match k.as_ref() {
                "actor" => actor = v.to_string(),
                "activityId" => activity_id = v.to_string(),
                "registration" => registration = v.to_string(),
                "context" => context = v.to_string(),
                _ => {}
            }
        }

        // Извлекаем challenge_id из activity_id (например https://uchebnik.mos.ru/exam/challenge/142006653)
        let re_challenge = Regex::new(r"/exam/challenge/(\d+)")?;
        let challenge_id = re_challenge
            .captures(&activity_id)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| anyhow!("Не удалось извлечь challenge_id из activityId: {}", activity_id))?;

        Ok(CdzLaunchParams {
            actor,
            activity_id,
            registration,
            context,
            challenge_id,
        })
    }

    /// Шаг 3: OAuth авторизация в библиотеке uchebnik.mos.ru для получения accessTokenEom
    pub async fn authenticate_library(&self) -> Result<CdzSession> {
        info!("Выполнение OAuth handshake для uchebnik.mos.ru...");

        let state = "cdz-auth-session";
        let auth_url = format!(
            "https://school.mos.ru/v1/oauth/authorize?client_id=library&redirect_uri=https%3A%2F%2Fuchebnik.mos.ru%2Flauncher%2Fauthenticate&response_mode=query&response_type=code&scope=email+phone+profile+oic_given_name+oic_family_name+oic_middle_name+oic_birthdate&state={}",
            state
        );

        let r_code = self.client.get(&auth_url).send().await
            .context("Ошибка при вызове OAuth authorize")?;
        let loc_header = r_code
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let html_text = r_code.text().await.unwrap_or_default();

        let re_code = Regex::new(r"code=([a-f0-9-]+)")?;
        let code = re_code
            .captures(&loc_header)
            .or_else(|| re_code.captures(&html_text))
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| anyhow!("Не удалось извлечь одноразовый OAuth код из ответа authorize. Location: '{}', HTML: '{}'", loc_header, html_text))?;

        info!("Извлечен одноразовый OAuth код: {}", code);

        let callback_url = format!(
            "https://uchebnik.mos.ru/acl/api/session/v2/callback?code={}&state={}&rgn=77",
            code, state
        );

        let cb_resp = self.client.get(&callback_url).send().await
            .context("Ошибка при вызове OAuth callback на uchebnik.mos.ru")?;

        let session_json: serde_json::Value = cb_resp.json().await
            .context("Не удалось распарсить JSON сессии библиотеки")?;

        let access_token_eom = session_json["accessTokenEom"]
            .as_str()
            .ok_or_else(|| anyhow!("В ответе callback отсутствует accessTokenEom: {:?}", session_json))?
            .to_string();

        let profile_id = session_json["profileId"]
            .as_i64()
            .unwrap_or(3276799);

        let user_id = session_json["userId"]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| session_json["userId"].as_i64().map(|n| n.to_string()))
            .unwrap_or_else(|| "3163720".to_string());

        info!("Успешная авторизация в библиотеке. EOM profileId: {}, userId: {}", profile_id, user_id);

        Ok(CdzSession {
            access_token_eom,
            profile_id,
            user_id,
        })
    }

    fn test_headers(&self, session: &CdzSession) -> Result<HeaderMap> {
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", session.access_token_eom))?,
        );
        h.insert("Profile-Id", HeaderValue::from_str(&session.profile_id.to_string())?);
        h.insert("User-Id", HeaderValue::from_str(&session.user_id)?);
        let profile_val = serde_json::json!({
            "id": session.profile_id,
            "type": "student"
        });
        h.insert("Profile", HeaderValue::from_str(&profile_val.to_string())?);
        h.insert("X-Mes-Subsystem", HeaderValue::from_static("testw"));
        h.insert(ACCEPT, HeaderValue::from_static("application/json"));
        Ok(h)
    }

    /// Шаг 4: Запуск попытки прохождения теста
    pub async fn start_attempt(&self, session: &CdzSession, params: &CdzLaunchParams) -> Result<CdzAttempt> {
        info!("Запуск попытки тестирования challenge_id={}...", params.challenge_id);

        let start_url = format!(
            "https://uchebnik.mos.ru/webtests/exam/rest/secure/challenge/{}/start-attempt",
            params.challenge_id
        );

        let body = serde_json::json!({
            "actorParam": params.actor,
            "registrationParam": params.registration,
            "activityIdParam": params.activity_id,
            "resultOverviewUrl": format!("https://uchebnik.mos.ru/exam/challenge/{}/attempt/{{attemptId}}", params.challenge_id),
            "resultManagementUrl": format!("https://uchebnik.mos.ru/exam/challenge/{}/context/{{assignmentContextId}}/attempt/{{attemptId}}", params.challenge_id),
            "context": params.context,
            "contextCorrupted": false
        });

        let resp = self.client.post(&start_url)
            .headers(self.test_headers(session)?)
            .json(&body)
            .send()
            .await
            .context("Сетевой сбой при вызове start-attempt")?;

        let status = resp.status();
        let resp_text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(anyhow!("Ошибка запуска попытки тестирования (HTTP {}): {}", status, resp_text));
        }

        let resp_json: serde_json::Value = serde_json::from_str(&resp_text)
            .context("Не удалось распарсить JSON ответа start-attempt")?;

        let attempt_id = resp_json["challenge_attempt_id"]
            .as_i64()
            .or_else(|| resp_json["id"].as_i64())
            .ok_or_else(|| anyhow!("Ответ start-attempt не содержит challenge_attempt_id: {}", resp_text))?;

        let raw_groups = resp_json["challenge_test_groups"].clone();
        let test_groups: Vec<CdzTestGroup> = serde_json::from_value(raw_groups)
            .unwrap_or_default();

        info!("Попытка успешно запущена! Attempt ID: {}, групп заданий: {}", attempt_id, test_groups.len());

        Ok(CdzAttempt {
            challenge_attempt_id: attempt_id,
            challenge_id: params.challenge_id.clone(),
            test_groups,
        })
    }

    /// Шаг 5: Отправка ответа на конкретное задание через multipart/form-data
    pub async fn submit_task_answer(
        &self,
        session: &CdzSession,
        task_id: i64,
        attempt_id: i64,
        answer_payload: serde_json::Value,
    ) -> Result<()> {
        let answer_url = "https://uchebnik.mos.ru/webtests/exam/rest/secure/challenge/task/answer";

        let body_json = serde_json::json!({
            "challenge_task_id": task_id,
            "challenge_attempt_id": attempt_id,
            "answer": answer_payload
        });

        let json_str = serde_json::to_string(&body_json)?;

        let part = Part::text(json_str)
            .file_name("blob")
            .mime_str("application/json")?;

        let form = Form::new().part("request", part);

        let mut headers = self.test_headers(session)?;
        headers.remove(CONTENT_TYPE);

        let resp = self.client.post(answer_url)
            .headers(headers)
            .multipart(form)
            .send()
            .await
            .context("Ошибка при отправке ответа на задание")?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(anyhow!("Сервер отклонил ответ на задачу #{} (HTTP {}): {}", task_id, status, text));
        }

        info!("Ответ на задачу #{} успешно сохранен на сервере МЭШ (HTTP 200)", task_id);
        Ok(())
    }

    /// Шаг 6: Завершение попытки тестирования
    pub async fn complete_attempt(&self, session: &CdzSession, challenge_id: &str) -> Result<()> {
        info!("Завершение попытки тестирования challenge_id={}...", challenge_id);

        let complete_url = format!(
            "https://uchebnik.mos.ru/webtests/exam/rest/secure/challenge/{}/complete_attempt",
            challenge_id
        );

        let body = serde_json::json!({
            "challenge_id": challenge_id
        });

        let resp = self.client.post(&complete_url)
            .headers(self.test_headers(session)?)
            .json(&body)
            .send()
            .await
            .context("Ошибка запроса complete_attempt")?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(anyhow!("Ошибка при завершении попытки (HTTP {}): {}", status, text));
        }

        info!("Попытка challenge_id={} успешно завершена!", challenge_id);
        Ok(())
    }

    /// Шаг 7: Получение итогового протокола результатов тестирования
    pub async fn fetch_result(&self, session: &CdzSession, challenge_id: &str, attempt_id: i64) -> Result<CdzResult> {
        let result_url = format!(
            "https://uchebnik.mos.ru/webtests/exam/rest/secure/challenge/{}/attempt/{}/result",
            challenge_id, attempt_id
        );

        let resp = self.client.get(&result_url)
            .headers(self.test_headers(session)?)
            .send()
            .await
            .context("Ошибка запроса результатов тестирования")?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(anyhow!("Не удалось получить результат тестирования (HTTP {}): {}", status, text));
        }

        let json_val: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_default();

        let score = json_val["score"].as_f64();
        let max_score = json_val["max_score"].as_f64();
        let percent_complete = json_val["percent_complete"].as_f64();

        Ok(CdzResult {
            attempt_id,
            score,
            max_score,
            percent_complete,
            raw: Some(json_val),
        })
    }
}
