use anyhow::{anyhow, Context, Result};
use std::time::Duration;
use tokio::process::Command;
use tracing::info;

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
    model: String,
}

impl SolverRunner {
    pub fn new<P: AsRef<Path>>(mode: SolverMode, timeout_secs: u64, working_dir: P, model: String) -> Self {
        Self {
            mode,
            timeout_secs,
            working_dir: working_dir.as_ref().to_path_buf(),
            model,
        }
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

    pub async fn ask_chat(&self, prompt: &str, model: &str) -> Result<String> {
        info!("Запрос к нейросети через agy (чат, модель: {})...", model);

        let run_fut = async {
            match &self.mode {
                SolverMode::Local { agy_bin } => self.solve_local_chat(agy_bin, prompt, model).await,
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
                "Превышен таймаут ожидания ответа нейросети ({} сек)",
                self.timeout_secs
            )),
        }
    }

    async fn solve_local_chat(&self, agy_bin: &str, prompt: &str, model: &str) -> Result<String> {
        let bin_path = if std::path::Path::new(agy_bin).is_absolute() && std::path::Path::new(agy_bin).exists() {
            agy_bin.to_string()
        } else if std::path::Path::new("/usr/local/bin/agy").exists() {
            "/usr/local/bin/agy".to_string()
        } else if std::path::Path::new("/usr/bin/agy").exists() {
            "/usr/bin/agy".to_string()
        } else {
            agy_bin.to_string()
        };

        let mut cmd = Command::new(&bin_path);
        cmd.arg("--dangerously-skip-permissions")
            .arg("--model")
            .arg(model)
            .arg("-p")
            .arg(prompt)
            .env("LANG", "en_US.UTF-8")
            .env("LC_ALL", "en_US.UTF-8")
            .current_dir(&self.working_dir)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);

        let output = cmd.output()
            .await
            .context(format!("Не удалось запустить локальный agy бинарник '{}'", bin_path))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(anyhow!(
                "agy завершился с ошибкой:\nstdout: {}\nstderr: {}",
                stdout,
                stderr
            ));
        }

        let solution = String::from_utf8_lossy(&output.stdout).to_string();
        let ansi_cleaned = Self::strip_ansi(&solution);
        let latex_cleaned = crate::solver::latex::clean_latex(&ansi_cleaned);
        Ok(latex_cleaned)
    }

    fn strip_ansi(raw: &str) -> String {
        let mut cleaned = String::new();
        let mut in_escape = false;

        for c in raw.chars() {
            if c == '\x1b' {
                in_escape = true;
            } else if in_escape {
                if c == 'm' || c == 'K' || c == 'H' || c == 'J' {
                    in_escape = false;
                }
            } else {
                cleaned.push(c);
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

        let mut cmd = Command::new(&bin_path);
        cmd.arg("--dangerously-skip-permissions")
            .arg("--model")
            .arg(model)
            .arg("-p")
            .arg(prompt)
            .env("LANG", "en_US.UTF-8")
            .env("LC_ALL", "en_US.UTF-8")
            .current_dir(&self.working_dir)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);

        let output = cmd.output()
            .await
            .context(format!("Не удалось запустить локальный agy бинарник '{}' (cwd: '{}')", bin_path, self.working_dir.display()))?;

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
        // Экранируем одинарные кавычки в промпте для безопасного bash -c
        let escaped_prompt = prompt.replace('\'', "'\\''");
        let remote_cmd = format!("cd {} && LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 {} --dangerously-skip-permissions --model {} -p '{}'", self.working_dir.display(), agy_bin, model, escaped_prompt);

        let mut cmd = Command::new("ssh");
        cmd.arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg("-p")
            .arg(port.to_string())
            .arg(format!("{}@{}", user, host))
            .arg(&remote_cmd)
            .kill_on_drop(true);

        let output = cmd.output()
            .await
            .context("Не удалось запустить команду через ssh")?;

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

    fn clean_solution(raw: &str) -> String {
        // Удаляем ANSI escape sequences если они есть
        let mut cleaned = String::new();
        let mut in_escape = false;

        for c in raw.chars() {
            if c == '\x1b' {
                in_escape = true;
            } else if in_escape {
                if c == 'm' || c == 'K' || c == 'H' || c == 'J' {
                    in_escape = false;
                }
            } else {
                cleaned.push(c);
            }
        }

        let ansi_cleaned = cleaned.trim().to_string();
        let latex_cleaned = crate::solver::latex::clean_latex(&ansi_cleaned);
        crate::solver::humanizer::Humanizer::humanize(&latex_cleaned)
    }
}
