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
                } => self.solve_ssh_chat(host, user, *port, agy_bin, prompt, model).await,
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
                "agy завершился с ошибкой:\nstdout: {}\nstderr: {}",
                stdout,
                stderr
            ));
        }

        let solution = String::from_utf8_lossy(&output.stdout).to_string();
        let ansi_cleaned = Self::strip_ansi(&solution);
        let latex_cleaned = crate::solver::latex::clean_latex(&ansi_cleaned);
        let chat_cleaned = crate::solver::humanizer::Humanizer::clean_chat_reply(&latex_cleaned);
        Ok(chat_cleaned)
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
                "Удаленный agy по SSH завершился с ошибкой:\nstdout: {}\nstderr: {}",
                stdout,
                stderr
            ));
        }

        let solution = String::from_utf8_lossy(&output.stdout).to_string();
        let ansi_cleaned = Self::strip_ansi(&solution);
        let latex_cleaned = crate::solver::latex::clean_latex(&ansi_cleaned);
        let chat_cleaned = crate::solver::humanizer::Humanizer::clean_chat_reply(&latex_cleaned);
        Ok(chat_cleaned)
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
