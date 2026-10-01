use std::path::PathBuf;
use std::process::Command;
use tracing::{info, warn};

pub struct VisualRenderer;

impl VisualRenderer {
    pub fn try_extract_and_render(text: &str, subject_title: Option<&str>) -> Option<PathBuf> {
        // 1. Попытка найти и исполнить SVG код
        if let Some(svg_content) = Self::extract_svg(text) {
            info!("Обнаружен SVG чертеж в ответе, запускаю рендеринг...");
            if let Some(png_path) = Self::render_svg_to_png(&svg_content) {
                return Some(png_path);
            }
        }

        // 2. Попытка найти и исполнить Python matplotlib код
        if let Some(py_content) = Self::extract_python_plot(text) {
            info!("Обнаружен Python-скрипт построения графика, запускаю рендеринг...");
            if let Some(png_path) = Self::execute_python_plot(&py_content) {
                return Some(png_path);
            }
        }

        // 3. Попытка найти Markdown таблицу и отрендерить ее на черном фоне
        if Self::contains_markdown_table(text) {
            info!("Обнаружена Markdown-таблица, запускаю рендеринг на черном фоне...");
            if let Some(png_path) = Self::render_markdown_table(text, subject_title) {
                return Some(png_path);
            }
        }

        None
    }

    fn extract_svg(text: &str) -> Option<String> {
        let start_tag = "<svg";
        let end_tag = "</svg>";

        if let Some(start_idx) = text.find(start_tag) {
            if let Some(end_idx) = text[start_idx..].find(end_tag) {
                let full_end = start_idx + end_idx + end_tag.len();
                return Some(text[start_idx..full_end].to_string());
            }
        }

        None
    }

    fn render_svg_to_png(svg: &str) -> Option<PathBuf> {
        let timestamp = chrono::Local::now().timestamp_nanos_opt().unwrap_or(0);
        let svg_path = format!("/tmp/hw_svg_{}.svg", timestamp);
        let png_path = format!("/tmp/hw_svg_{}.png", timestamp);

        if std::fs::write(&svg_path, svg).is_err() {
            return None;
        }

        let res = Command::new("resvg")
            .arg(&svg_path)
            .arg(&png_path)
            .output();

        if let Ok(out) = res {
            if out.status.success() && std::path::Path::new(&png_path).exists() {
                let _ = std::fs::remove_file(&svg_path);
                return Some(PathBuf::from(png_path));
            }
        }

        let res2 = Command::new("rsvg-convert")
            .arg(&svg_path)
            .arg("-o")
            .arg(&png_path)
            .output();

        if let Ok(out) = res2 {
            if out.status.success() && std::path::Path::new(&png_path).exists() {
                let _ = std::fs::remove_file(&svg_path);
                return Some(PathBuf::from(png_path));
            }
        }

        let _ = std::fs::remove_file(&svg_path);
        None
    }

    fn extract_python_plot(text: &str) -> Option<String> {
        let mut in_block = false;
        let mut block_lines = Vec::new();
        let mut is_plot_script = false;

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("```python") || trimmed.starts_with("```py") {
                in_block = true;
                block_lines.clear();
                is_plot_script = false;
                continue;
            } else if trimmed == "```" && in_block {
                in_block = false;
                if is_plot_script {
                    return Some(block_lines.join("\n"));
                }
            } else if in_block {
                if line.contains("matplotlib") || line.contains("plt.") || line.contains("savefig") {
                    is_plot_script = true;
                }
                block_lines.push(line);
            }
        }

        None
    }

    fn execute_python_plot(py_code: &str) -> Option<PathBuf> {
        let timestamp = chrono::Local::now().timestamp_nanos_opt().unwrap_or(0);
        let py_path = format!("/tmp/hw_plot_{}.py", timestamp);
        let png_path = format!("/tmp/hw_plot_{}.png", timestamp);

        let mut full_code = String::new();
        full_code.push_str("import matplotlib\nmatplotlib.use('Agg')\nimport matplotlib.pyplot as plt\n");

        for line in py_code.lines() {
            if line.contains("plt.show()") {
                full_code.push_str(&format!("plt.savefig('{}', bbox_inches='tight')\n", png_path));
            } else if line.contains("plt.savefig(") {
                full_code.push_str(&format!("plt.savefig('{}', bbox_inches='tight')\n", png_path));
            } else {
                full_code.push_str(line);
                full_code.push('\n');
            }
        }

        full_code.push_str(&format!("\nif not plt.get_fignums(): pass\nelse: plt.savefig('{}', bbox_inches='tight')\n", png_path));

        if std::fs::write(&py_path, full_code).is_err() {
            return None;
        }

        let output = Command::new("python3").arg(&py_path).output();
        let _ = std::fs::remove_file(&py_path);

        if let Ok(out) = output {
            if out.status.success() && std::path::Path::new(&png_path).exists() {
                let metadata = std::fs::metadata(&png_path).ok();
                if metadata.map(|m| m.len() > 0).unwrap_or(false) {
                    info!("График успешно сгенерирован в {}", png_path);
                    return Some(PathBuf::from(png_path));
                }
            } else {
                warn!(
                    "Ошибка исполнения Python-скрипта графика: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        }

        None
    }

    fn contains_markdown_table(text: &str) -> bool {
        let mut pipe_lines = 0;
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with('|') && t.ends_with('|') {
                pipe_lines += 1;
                if pipe_lines >= 3 {
                    return true;
                }
            }
        }
        false
    }

    fn render_markdown_table(text: &str, title: Option<&str>) -> Option<PathBuf> {
        let timestamp = chrono::Local::now().timestamp_nanos_opt().unwrap_or(0);
        let txt_path = format!("/tmp/table_{}.txt", timestamp);
        let png_path = format!("/tmp/table_{}.png", timestamp);

        if std::fs::write(&txt_path, text).is_err() {
            return None;
        }

        let mut cmd = Command::new("python3");
        cmd.arg("scripts/render_table.py")
           .arg(&txt_path)
           .arg(&png_path);

        if let Some(t) = title {
            cmd.arg(t);
        }

        let output = cmd.output();
        let _ = std::fs::remove_file(&txt_path);

        if let Ok(out) = output {
            if out.status.success() && std::path::Path::new(&png_path).exists() {
                info!("Таблица успешно отрендерена на черном фоне: {}", png_path);
                return Some(PathBuf::from(png_path));
            } else {
                warn!(
                    "Ошибка рендеринга таблицы: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        }

        None
    }

    /// Извлекает визуальные элементы (таблицы, графики, SVG), рендерит их в файлы изображений,
    /// и возвращает очищенный от таблиц/графиков текст для Telegram + путь к сгенерированному фото.
    pub fn process_visuals_and_strip(text: &str, subject_title: Option<&str>) -> (String, Option<PathBuf>) {
        let img_path = Self::try_extract_and_render(text, subject_title);
        let text_without_visuals = Self::strip_visuals_from_text(text);
        (text_without_visuals, img_path)
    }

    /// Полное удаление таблиц Markdown, блоков SVG и Python-скриптов из текста сообщения.
    pub fn strip_visuals_from_text(text: &str) -> String {
        let mut without_svg = text.to_string();
        while let Some(start_idx) = without_svg.find("<svg") {
            if let Some(end_idx) = without_svg[start_idx..].find("</svg>") {
                let full_end = start_idx + end_idx + "</svg>".len();
                without_svg.replace_range(start_idx..full_end, "");
            } else {
                break;
            }
        }

        // Удаляем блоки ```python ... plt ... ```
        let mut clean_lines = Vec::new();
        let mut in_py_block = false;
        let mut py_block_lines = Vec::new();

        for line in without_svg.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("```python") || trimmed.starts_with("```py") {
                in_py_block = true;
                py_block_lines.clear();
                continue;
            } else if trimmed == "```" && in_py_block {
                in_py_block = false;
                let full_py = py_block_lines.join("\n");
                if !full_py.contains("plt.") && !full_py.contains("matplotlib") {
                    // Если это был не график, возвращаем код
                    clean_lines.push("```python");
                    clean_lines.extend(py_block_lines.drain(..));
                    clean_lines.push("```");
                }
                py_block_lines.clear();
                continue;
            } else if in_py_block {
                py_block_lines.push(line);
                continue;
            }

            clean_lines.push(line);
        }

        // Удаляем строки Markdown-таблиц (| ... |)
        let mut final_lines: Vec<&str> = Vec::new();
        let mut last_was_empty = false;

        for line in clean_lines {
            let trimmed = line.trim();
            let is_table_row = trimmed.starts_with('|') && trimmed.ends_with('|');

            if is_table_row {
                continue;
            }

            // Устраняем избыточные пустые строки после вырезания таблиц
            if trimmed.is_empty() {
                if !last_was_empty {
                    final_lines.push("");
                    last_was_empty = true;
                }
            } else {
                final_lines.push(line);
                last_was_empty = false;
            }
        }

        final_lines.join("\n").trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_markdown_tables() {
        let text = "Параграф 15.\n\n| Колонка 1 | Колонка 2 |\n|---|---|\n| А | Б |\n\nОтвет: готово.";
        let stripped = VisualRenderer::strip_visuals_from_text(text);
        assert_eq!(stripped, "Параграф 15.\n\nОтвет: готово.");
    }
}
