use anyhow::{anyhow, Context, Result};
use std::time::Duration;
use tokio::process::Command;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub enum SolverMode {
    Local {
        agy_bin: String,
    },
    RemoteSsh {
        host: String,
        user: String,
        port: u16,
        agy_bin: String,
    },
}

use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct SolverRunner {
    mode: SolverMode,
    timeout_secs: u64,
    working_dir: PathBuf,
    project_dir: PathBuf,
    tg_bot_token: Option<String>,
    db_path: Option<PathBuf>,
    model: String,
}

impl SolverRunner {
    pub fn new<P: AsRef<Path>, Q: AsRef<Path>>(
        mode: SolverMode,
        timeout_secs: u64,
        working_dir: P,
        project_dir: Q,
        tg_bot_token: Option<String>,
        db_path: Option<PathBuf>,
        model: String,
    ) -> Self {
        Self {
            mode,
            timeout_secs,
            working_dir: working_dir.as_ref().to_path_buf(),
            project_dir: project_dir.as_ref().to_path_buf(),
            tg_bot_token,
            db_path,
            model,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub async fn solve(&self, prompt: &str) -> Result<String> {
        self.solve_with_model(prompt, &self.model).await
    }

    pub async fn solve_with_model(&self, prompt: &str, model: &str) -> Result<String> {
        info!("Запуск Antigravity solver (модель {}) для решения задачи...", model);

        let run_fut = async {
            match &self.mode {
                SolverMode::Local { agy_bin } => self.solve_local(agy_bin, prompt, model).await,
                SolverMode::RemoteSsh {
                    host,
                    user,
                    port,
                    agy_bin,
                } => self.solve_ssh(host, user, *port, agy_bin, prompt, model).await,
            }
        };

        match tokio::time::timeout(Duration::from_secs(self.timeout_secs), run_fut).await {
            Ok(res) => res,
            Err(_) => Err(anyhow!(
                "Превышен таймаут ожидания решения задачи ({} сек)",
                self.timeout_secs
            )),
        }
    }

    pub async fn ask_chat(
        &self,
        prompt: &str,
        model: &str,
        chat_id: Option<&str>,
        conversation_id: Option<&str>,
    ) -> Result<(String, Option<String>)> {
        info!("Запрос к нейросети через agy (чат, модель: {}, conv_id: {:?})...", model, conversation_id);

        let run_fut = async {
            match &self.mode {
                SolverMode::Local { agy_bin } => self.solve_local_chat(agy_bin, prompt, model, chat_id, conversation_id).await,
                SolverMode::RemoteSsh {
                    host,
                    user,
                    port,
                    agy_bin,
                } => self.solve_ssh_chat(host, user, *port, agy_bin, prompt, model, chat_id, conversation_id).await,
            }
        };

        match tokio::time::timeout(Duration::from_secs(self.timeout_secs), run_fut).await {
            Ok(res) => res,
            Err(_) => Err(anyhow!(
                "Превышен таймаут ожидания ответа нейросети ({} сек)",
                self.timeout_secs
            )),
        }
    }

    pub fn get_latest_local_agy_conversation_id() -> Option<String> {
        let home = std::env::var("HOME").ok()?;
        let db_path = std::path::Path::new(&home).join(".gemini/antigravity-cli/conversation_summaries.db");
        if !db_path.exists() {
            return None;
        }
        let conn = rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ).ok()?;
        let mut stmt = conn.prepare("SELECT conversation_id FROM conversation_summaries ORDER BY last_modified_time DESC LIMIT 1").ok()?;
        let mut rows = stmt.query([]).ok()?;
        if let Some(row) = rows.next().ok()? {
            let id: String = row.get(0).ok()?;
            return Some(id);
        }
        None
    }

    async fn solve_local_chat(
        &self,
        agy_bin: &str,
        prompt: &str,
        model: &str,
        chat_id: Option<&str>,
        conversation_id: Option<&str>,
    ) -> Result<(String, Option<String>)> {
        let bin_path = if std::path::Path::new(agy_bin).is_absolute() && std::path::Path::new(agy_bin).exists() {
            agy_bin.to_string()
        } else if std::path::Path::new("/usr/local/bin/agy").exists() {
            "/usr/local/bin/agy".to_string()
        } else if std::path::Path::new("/usr/bin/agy").exists() {
            "/usr/bin/agy".to_string()
        } else {
            agy_bin.to_string()
        };

        // Для чата и скриптов используем каталог проекта бота (где доступны scripts/, data/, cargo)
        let run_dir = if self.project_dir.exists() {
            self.project_dir.clone()
        } else if self.working_dir.exists() {
            self.working_dir.clone()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };

        let mut cmd = Command::new(&bin_path);
        cmd.arg("--dangerously-skip-permissions")
            .arg("--model")
            .arg(model)
            .env("LANG", "en_US.UTF-8")
            .env("LC_ALL", "en_US.UTF-8");

        if let Some(cid) = conversation_id {
            if !cid.trim().is_empty() {
                cmd.arg("--conversation").arg(cid.trim());
            }
        }

        if let Some(token) = &self.tg_bot_token {
            cmd.env("TELEGRAM_BOT_TOKEN", token);
        }
        if let Some(db_path) = &self.db_path {
            cmd.env("DB_PATH", db_path);
        }
        if let Some(cid) = chat_id {
            cmd.env("TELEGRAM_CHAT_ID", cid);
        }
        cmd.env("PYTHONPATH", &run_dir)
            .current_dir(&run_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn()
            .with_context(|| format!("Не удалось запустить локальный agy бинарник '{}' (cwd: '{}')", bin_path, run_dir.display()))?;

        if let Some(mut stdin) = child.stdin.take() {
            let prompt_bytes = prompt.as_bytes().to_vec();
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(&prompt_bytes).await;
                let _ = stdin.flush().await;
                drop(stdin);
            });
        }

        let output = child.wait_with_output()
            .await
            .context("Ошибка ожидания завершения agy процесса")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);

            if conversation_id.is_some() {
                warn!("agy завершился с ошибкой при продолжении беседы {:?}, перезапуск с нового диалога...", conversation_id);
                return Box::pin(self.solve_local_chat(agy_bin, prompt, model, chat_id, None)).await;
            }

            return Err(anyhow!(
                "agy завершился с ошибкой:\nstdout: {}\nstderr: {}",
                stdout,
                stderr
            ));
        }

        let solution = String::from_utf8_lossy(&output.stdout).to_string();
        let ansi_cleaned = Self::strip_ansi(&solution);
        let latex_cleaned = crate::solver::latex::clean_latex(&ansi_cleaned);
        let chat_cleaned = crate::solver::humanizer::Humanizer::clean_chat_reply(&latex_cleaned);

        let conv_out = if let Some(cid) = conversation_id {
            Some(cid.to_string())
        } else {
            Self::get_latest_local_agy_conversation_id()
        };

        Ok((chat_cleaned, conv_out))
    }

    pub fn strip_ansi(raw: &str) -> String {
        let mut cleaned = String::new();
        let chars: Vec<char> = raw.chars().collect();
        let len = chars.len();
        let mut i = 0;

        while i < len {
            if chars[i] == '\x1b' {
                i += 1;
                if i >= len {
                    break;
                }
                match chars[i] {
                    '[' => {
                        // CSI последовательность: \x1b[ ... [A-Za-z]
                        i += 1;
                        while i < len {
                            let c = chars[i];
                            if c.is_ascii_alphabetic() || c == '@' || c == '`' || c == '~' {
                                i += 1;
                                break;
                            } else if (c as u32) >= 0x20 && (c as u32) <= 0x3F {
                                i += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    ']' => {
                        // OSC последовательность: \x1b] ... (\x07 или \x1b\)
                        i += 1;
                        while i < len {
                            if chars[i] == '\x07' {
                                i += 1;
                                break;
                            } else if chars[i] == '\x1b' && i + 1 < len && chars[i + 1] == '\\' {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    '(' | ')' => {
                        i += 1;
                        if i < len {
                            i += 1;
                        }
                    }
                    c if (c as u32) >= 0x40 && (c as u32) <= 0x5F => {
                        i += 1;
                    }
                    _ => {
                        i += 1;
                    }
                }
            } else {
                cleaned.push(chars[i]);
                i += 1;
            }
        }
        cleaned.trim().to_string()
    }

    async fn solve_local(&self, agy_bin: &str, prompt: &str, model: &str) -> Result<String> {
        let bin_path = if std::path::Path::new(agy_bin).is_absolute() && std::path::Path::new(agy_bin).exists() {
            agy_bin.to_string()
        } else if std::path::Path::new("/usr/local/bin/agy").exists() {
            "/usr/local/bin/agy".to_string()
        } else if std::path::Path::new("/usr/bin/agy").exists() {
            "/usr/bin/agy".to_string()
        } else {
            agy_bin.to_string()
        };

        let run_dir = if self.working_dir.exists() {
            self.working_dir.clone()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };

        let mut cmd = Command::new(&bin_path);
        cmd.arg("--dangerously-skip-permissions")
            .arg("--model")
            .arg(model)
            .env("LANG", "en_US.UTF-8")
            .env("LC_ALL", "en_US.UTF-8")
            .current_dir(&run_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn()
            .with_context(|| format!("Не удалось запустить локальный agy бинарник '{}' (cwd: '{}')", bin_path, run_dir.display()))?;

        if let Some(mut stdin) = child.stdin.take() {
            let prompt_bytes = prompt.as_bytes().to_vec();
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(&prompt_bytes).await;
                let _ = stdin.flush().await;
                drop(stdin);
            });
        }

        let output = child.wait_with_output()
            .await
            .context("Ошибка ожидания завершения agy процесса")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(anyhow!(
                "agy завершился с ошибкой (код {:?}):\nstdout: {}\nstderr: {}",
                output.status.code(),
                stdout,
                stderr
            ));
        }

        let solution = String::from_utf8_lossy(&output.stdout).to_string();
        let cleaned = Self::clean_solution(&solution);

        if cleaned.is_empty() || cleaned.contains("no output produced") {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "agy не вернул текст решения: {}\nStderr: {}",
                cleaned,
                stderr
            ));
        }

        Ok(cleaned)
    }

    async fn solve_ssh(
        &self,
        host: &str,
        user: &str,
        port: u16,
        agy_bin: &str,
        prompt: &str,
        model: &str,
    ) -> Result<String> {
        let remote_cmd = format!(
            "cd {} && LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 {} --dangerously-skip-permissions --model {}",
            self.working_dir.display(),
            agy_bin,
            model
        );

        let mut cmd = Command::new("ssh");
        cmd.arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg("-p")
            .arg(port.to_string())
            .arg(format!("{}@{}", user, host))
            .arg(&remote_cmd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn()
            .context("Не удалось запустить команду через ssh")?;

        if let Some(mut stdin) = child.stdin.take() {
            let prompt_bytes = prompt.as_bytes().to_vec();
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(&prompt_bytes).await;
                let _ = stdin.flush().await;
                drop(stdin);
            });
        }

        let output = child.wait_with_output()
            .await
            .context("Ошибка ожидания ssh")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(anyhow!(
                "Удаленный agy по SSH завершился с ошибкой: {}\n{}",
                stdout,
                stderr
            ));
        }

        let solution = String::from_utf8_lossy(&output.stdout).to_string();
        let cleaned = Self::clean_solution(&solution);

        if cleaned.is_empty() || cleaned.contains("no output produced") {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "agy не вернул текст решения: {}\nStderr: {}",
                cleaned,
                stderr
            ));
        }

        Ok(cleaned)
    }

    async fn solve_ssh_chat(
        &self,
        host: &str,
        user: &str,
        port: u16,
        agy_bin: &str,
        prompt: &str,
        model: &str,
        chat_id: Option<&str>,
        conversation_id: Option<&str>,
    ) -> Result<(String, Option<String>)> {
        let run_dir = if self.project_dir.exists() {
            self.project_dir.clone()
        } else {
            self.working_dir.clone()
        };

        let mut env_vars = "LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8".to_string();
        if let Some(token) = &self.tg_bot_token {
            env_vars.push_str(&format!(" TELEGRAM_BOT_TOKEN='{}'", token.replace('\'', "'\\''")));
        }
        if let Some(db) = &self.db_path {
            env_vars.push_str(&format!(" DB_PATH='{}'", db.display()));
        }
        if let Some(cid) = chat_id {
            env_vars.push_str(&format!(" TELEGRAM_CHAT_ID='{}'", cid.replace('\'', "'\\''")));
        }
        env_vars.push_str(&format!(" PYTHONPATH='{}'", run_dir.display()));

        let conv_arg = if let Some(cid) = conversation_id {
            if !cid.trim().is_empty() {
                format!(" --conversation '{}'", cid.trim().replace('\'', "'\\''"))
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let remote_cmd = format!(
            "cd {} && {} {} --dangerously-skip-permissions --model {}{}",
            run_dir.display(),
            env_vars,
            agy_bin,
            model,
            conv_arg
        );

        let mut cmd = Command::new("ssh");
        cmd.arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg("-p")
            .arg(port.to_string())
            .arg(format!("{}@{}", user, host))
            .arg(&remote_cmd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn()
            .context("Не удалось запустить команду через ssh")?;

        if let Some(mut stdin) = child.stdin.take() {
            let prompt_bytes = prompt.as_bytes().to_vec();
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(&prompt_bytes).await;
                let _ = stdin.flush().await;
                drop(stdin);
            });
        }

        let output = child.wait_with_output()
            .await
            .context("Ошибка ожидания ssh")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(anyhow!(
                "Удаленный agy по SSH завершился с ошибкой:\nstdout: {}\nstderr: {}",
                stdout,
                stderr
            ));
        }

        let solution = String::from_utf8_lossy(&output.stdout).to_string();
        let ansi_cleaned = Self::strip_ansi(&solution);
        let latex_cleaned = crate::solver::latex::clean_latex(&ansi_cleaned);
        let chat_cleaned = crate::solver::humanizer::Humanizer::clean_chat_reply(&latex_cleaned);
        Ok((chat_cleaned, conversation_id.map(|s| s.to_string())))
    }

    fn clean_solution(raw: &str) -> String {
        let ansi_cleaned = Self::strip_ansi(raw);
        let latex_cleaned = crate::solver::latex::clean_latex(&ansi_cleaned);
        let humanized = crate::solver::humanizer::Humanizer::humanize(&latex_cleaned);
        crate::solver::humanizer::Humanizer::strip_emojis(&humanized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_colors_and_controls() {
        let sample = "\x1b[32mУспешно выполнено\x1b[0m";
        assert_eq!(SolverRunner::strip_ansi(sample), "Успешно выполнено");

        let sample_cursor = "\x1b[?25hПривет, мир!\x1b[?25l";
        assert_eq!(SolverRunner::strip_ansi(sample_cursor), "Привет, мир!");

        let sample_clear = "\x1b[2J\x1b[HТекст после очистки";
        assert_eq!(SolverRunner::strip_ansi(sample_clear), "Текст после очистки");

        let sample_osc = "\x1b]0;Title\x07Заголовок терминала";
        assert_eq!(SolverRunner::strip_ansi(sample_osc), "Заголовок терминала");

        let sample_cyrillic = "Простой русский текст без кодов";
        assert_eq!(SolverRunner::strip_ansi(sample_cyrillic), "Простой русский текст без кодов");
    }
}
