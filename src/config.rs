use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub mesh_token: String,
    pub mesh_refresh_token: Option<String>,
    pub mesh_cookies: String,
    pub student_id: i64,
    pub tg_bot_token: String,
    pub tg_channel_id: String,
    pub admin_ids: Vec<i64>,
    pub solver_mode: String,
    pub ssh_host: String,
    pub ssh_user: String,
    pub ssh_port: u16,
    pub agy_bin: String,
    pub textbooks_dir: PathBuf,
    pub rules_dir: PathBuf,
    pub db_path: PathBuf,
    pub poll_interval_secs: u64,
    pub days_ahead: i64,
    pub days_back: i64,
    pub auto_mark_done: bool,
    pub solver_timeout_secs: u64,
    pub agy_model: String,
}

impl Config {
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv();

        let mesh_token = env::var("MESH_JWT_TOKEN").unwrap_or_default();
        let mesh_refresh_token = env::var("MESH_REFRESH_TOKEN").ok().filter(|s| !s.trim().is_empty());
        let mesh_cookies = env::var("MESH_COOKIES").unwrap_or_default();
        let student_id = env::var("MESH_STUDENT_ID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let tg_bot_token = env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default();
        let tg_channel_id = env::var("TELEGRAM_CHANNEL_ID").unwrap_or_default();

        let admin_ids: Vec<i64> = env::var("TELEGRAM_ADMIN_IDS")
            .unwrap_or_default()
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();

        let solver_mode = env::var("SOLVER_MODE").unwrap_or_else(|_| "local".to_string());
        let ssh_host = env::var("SSH_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let ssh_user = env::var("SSH_USER").unwrap_or_else(|_| "root".to_string());
        let ssh_port = env::var("SSH_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(22);

        let agy_bin = env::var("AGY_BIN").unwrap_or_else(|_| "agy".to_string());

        let textbooks_dir = env::var("TEXTBOOKS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("textbooks_7_grade"));

        let rules_dir = env::var("RULES_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("rules"));

        let db_path = env::var("DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("data/homeworks.db"));

        let poll_interval_secs = env::var("POLL_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300);

        let days_ahead = env::var("DAYS_AHEAD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);

        let days_back = env::var("DAYS_BACK")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2);

        let auto_mark_done = env::var("AUTO_MARK_DONE")
            .map(|s| s == "1" || s.to_lowercase() == "true")
            .unwrap_or(false);

        let solver_timeout_secs = env::var("SOLVER_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(600);

        let agy_model = env::var("AGY_MODEL").unwrap_or_else(|_| "gemini-3.8-flash-medium".to_string());

        Self {
            mesh_token,
            mesh_refresh_token,
            mesh_cookies,
            student_id,
            tg_bot_token,
            tg_channel_id,
            admin_ids,
            solver_mode,
            ssh_host,
            ssh_user,
            ssh_port,
            agy_bin,
            textbooks_dir,
            rules_dir,
            db_path,
            poll_interval_secs,
            days_ahead,
            days_back,
            auto_mark_done,
            solver_timeout_secs,
            agy_model,
        }
    }
}
