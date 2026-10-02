mod config;
mod db;
mod engine;
mod mesh;
mod solver;
mod telegram;

use anyhow::Result;
use config::Config;
use db::Database;
use engine::Engine;
use mesh::MeshClient;
use solver::{PromptBuilder, SolverMode, SolverRunner};
use telegram::{BotHandler, TelegramClient};
use tokio::sync::mpsc;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("=== Запуск МЭШ Homework Solver Bot ===");

    let cfg = Config::from_env();
    info!("Конфигурация загружена. Student ID: {}", cfg.student_id);

    let db = Database::open(&cfg.db_path)?;
    info!("База данных инициализирована: {}", cfg.db_path.display());

    // Проверяем, есть ли сохраненный токен в БД, который новее
    let active_token = match db.get_setting("mesh_token") {
        Ok(Some(t)) if !MeshClient::check_jwt_expired(&t) => {
            info!("Используется действующий МЭШ токен из базы данных");
            t
        }
        _ => {
            info!("Используется МЭШ токен из конфигурации/окружения");
            cfg.mesh_token.clone()
        }
    };

    let active_refresh_token = match db.get_setting("mesh_refresh_token") {
        Ok(Some(rt)) if !rt.trim().is_empty() => Some(rt),
        _ => cfg.mesh_refresh_token.clone(),
    };

    let active_cookies = db
        .get_setting("mesh_cookies")
        .ok()
        .flatten()
        .unwrap_or_else(|| cfg.mesh_cookies.clone());

    let mesh_client = MeshClient::new(
        active_token,
        active_refresh_token,
        cfg.student_id,
        active_cookies,
        Some(db.clone()),
    )?;

    // Проверяем профиль в МЭШ
    match mesh_client.get_profile().await {
        Ok(prof) => {
            info!(
                "Авторизация в МЭШ подтверждена: {} {}",
                prof.last_name.unwrap_or_default(),
                prof.first_name.unwrap_or_default()
            );
        }
        Err(e) => {
            warn!("Предупреждение при проверке профиля МЭШ: {}. Сервис продолжит работу.", e);
        }
    }

    let tg_client = TelegramClient::new(cfg.tg_bot_token.clone());

    let prompt_builder = PromptBuilder::new(&cfg.textbooks_dir, &cfg.rules_dir);

    let solver_mode = if cfg.solver_mode == "remote_ssh" {
        info!("Режим работы solver: Remote SSH ({}:{})", cfg.ssh_host, cfg.ssh_port);
        SolverMode::RemoteSsh {
            host: cfg.ssh_host.clone(),
            user: cfg.ssh_user.clone(),
            port: cfg.ssh_port,
            agy_bin: cfg.agy_bin.clone(),
        }
    } else {
        info!("Режим работы solver: Local binary ({})", cfg.agy_bin);
        SolverMode::Local {
            agy_bin: cfg.agy_bin.clone(),
        }
    };

    info!("Модель solver (agy): {}", cfg.agy_model);
    let solver_runner = SolverRunner::new(
        solver_mode,
        cfg.solver_timeout_secs,
        &cfg.textbooks_dir,
        cfg.agy_model.clone(),
    );

    let (event_tx, event_rx) = mpsc::channel(32);

    let bot_handler = BotHandler::new(
        tg_client.clone(),
        mesh_client.clone(),
        db.clone(),
        cfg.admin_ids.clone(),
        event_tx,
        solver_runner.clone(),
    );

    let engine = Engine::new(
        mesh_client,
        tg_client,
        db,
        prompt_builder,
        solver_runner,
        cfg.tg_channel_id.clone(),
        cfg.poll_interval_secs,
        cfg.days_ahead,
        cfg.days_back,
        cfg.auto_mark_done,
    );

    // Запуск polling Telegram бота в фоновой задаче
    tokio::spawn(async move {
        bot_handler.run_polling().await;
    });

    // Запуск рабочего цикла движка
    engine.run_loop(event_rx).await;

    Ok(())
}
