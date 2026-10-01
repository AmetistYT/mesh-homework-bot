use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SubjectExportItem {
    pub id: String,
    pub name: String,
    pub is_cdz: bool,
    pub preview_text: String,
    pub task_text: String,
    pub solution_markdown: String,
    pub is_open: bool,
}

pub struct HtmlExporter;

impl HtmlExporter {
    pub fn render_html(date_title: &str, subjects: &[SubjectExportItem]) -> String {
        let template = include_str!("../../templates/homework_template.html");
        let now_time = chrono::Local::now().format("%H:%M").to_string();
        let items_count = subjects.len().to_string();

        let mut cards_html = String::new();

        for s in subjects {
            let open_attr = if s.is_open { "open" } else { "" };
            let cdz_badge = if s.is_cdz {
                r#"<span class="badge-cdz">ЦДЗ</span>"#
            } else {
                ""
            };

            let task_box = if !s.task_text.trim().is_empty() {
                format!(
                    r#"<div class="task-box">
          <div class="task-label">Задание учителя</div>
          <div class="task-text">{}</div>
        </div>"#,
                    html_escape(&s.task_text)
                )
            } else {
                String::new()
            };

            let solution_html = markdown_to_html(&s.solution_markdown);

            let card = format!(
                r#"    <details class="subject-card" id="card-{id}" {open_attr}>
      <summary class="subject-summary">
        <div class="subject-meta">
          <div class="subject-header-row">
            <span class="subject-title">{name}</span>
            {cdz_badge}
          </div>
          <div class="subject-preview">{preview}</div>
        </div>
        <svg class="chevron" viewBox="0 0 24 24">
          <polyline points="6 9 12 15 18 9"></polyline>
        </svg>
      </summary>

      <div class="subject-content">
        {task_box}

        <div class="solution-body" id="solution-{id}">
          {solution_html}
        </div>

        <div class="card-footer">
          <button class="btn-action" onclick="copySolution('{id}', this)">
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
              <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
            </svg>
            <span>Копировать</span>
          </button>
        </div>
      </div>
    </details>
"#,
                id = s.id,
                name = html_escape(&s.name),
                cdz_badge = cdz_badge,
                preview = html_escape(&s.preview_text),
                open_attr = open_attr,
                task_box = task_box,
                solution_html = solution_html
            );
            cards_html.push_str(&card);
        }

        let mut output = template
            .replace("{{date_title}}", date_title)
            .replace("{{items_count}}", &items_count)
            .replace("{{generated_time}}", &now_time);

        // Заменяем блок шаблона
        if let Some(start_idx) = output.find("{{#subjects}}") {
            if let Some(end_marker) = output.find("{{/subjects}}") {
                let end_idx = end_marker + "{{/subjects}}".len();
                output.replace_range(start_idx..end_idx, &cards_html);
            }
        }

        output
    }

    pub fn save_export_file<P: AsRef<Path>>(
        output_dir: P,
        date_str: &str,
        html_content: &str,
    ) -> Result<PathBuf> {
        let dir = output_dir.as_ref();
        std::fs::create_dir_all(dir)?;

        let filename = format!("ДЗ_{}.html", date_str.replace('-', "_"));
        let target_path = dir.join(&filename);
        std::fs::write(&target_path, html_content)?;
        Ok(target_path)
    }
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn markdown_to_html(md: &str) -> String {
    let cleaned = crate::solver::latex::clean_latex(md);
    let mut html = String::new();
    let mut in_table = false;
    let mut table_rows: Vec<Vec<String>> = Vec::new();

    let lines: Vec<&str> = cleaned.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        // Проверка на строку таблицы
        if line.starts_with('|') && line.ends_with('|') {
            // Строка-разделитель |---|---|
            if line.chars().all(|c| c == '|' || c == '-' || c == ':' || c.is_whitespace()) {
                i += 1;
                continue;
            }

            let cols: Vec<String> = line
                .split('|')
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_string())
                .collect();

            if !cols.is_empty() {
                in_table = true;
                table_rows.push(cols);
                i += 1;
                continue;
            }
        } else if in_table {
            // Завершаем таблицу
            html.push_str(&render_table_html(&table_rows));
            table_rows.clear();
            in_table = false;
        }

        // Проверка на блок SVG (inline <svg> или внутри ```xml / ```svg)
        if line.starts_with("<svg") || line.starts_with("```xml\n<svg") || line.starts_with("```svg") {
            let mut svg_buf = String::new();
            let inside_fence = line.starts_with("```");
            if inside_fence {
                if let Some(pos) = line.find("<svg") {
                    svg_buf.push_str(&line[pos..]);
                    svg_buf.push('\n');
                }
            } else {
                svg_buf.push_str(line);
                svg_buf.push('\n');
            }

            if !svg_buf.contains("</svg>") {
                i += 1;
                while i < lines.len() {
                    let cur = lines[i];
                    let cur_trimmed = cur.trim();
                    if cur_trimmed == "```" {
                        break;
                    }
                    svg_buf.push_str(cur);
                    svg_buf.push('\n');
                    if cur_trimmed.contains("</svg>") {
                        if inside_fence && i + 1 < lines.len() && lines[i + 1].trim() == "```" {
                            i += 1;
                        }
                        break;
                    }
                    i += 1;
                }
            }

            // Извлекаем чистый <svg>...</svg>
            if let Some(start) = svg_buf.find("<svg") {
                if let Some(end) = svg_buf[start..].find("</svg>") {
                    let pure_svg = &svg_buf[start..start + end + 6];
                    html.push_str(&format!(r#"<div class="svg-container">{}</div>"#, pure_svg));
                    html.push('\n');
                    i += 1;
                    continue;
                }
            }
        }

        if line.is_empty() {
            i += 1;
            continue;
        }

        // Заголовки
        if let Some(h3) = line.strip_prefix("### ") {
            html.push_str(&format!("<h3>{}</h3>\n", format_inline(h3)));
            i += 1;
            continue;
        }
        if let Some(h2) = line.strip_prefix("## ") {
            html.push_str(&format!("<h3>{}</h3>\n", format_inline(h2)));
            i += 1;
            continue;
        }
        if let Some(h1) = line.strip_prefix("# ") {
            html.push_str(&format!("<h3>{}</h3>\n", format_inline(h1)));
            i += 1;
            continue;
        }

        // Разделители
        if line == "---" || line == "***" || line == "___" {
            html.push_str("<hr>\n");
            i += 1;
            continue;
        }

        // Блок «Ответ:»
        if line.starts_with("Ответ:") || line.starts_with("**Ответ:**") || line.starts_with("ОТВЕТ:") {
            html.push_str(&format!(
                r#"<div class="answer-box"><strong>Ответ:</strong> {}</div>"#,
                format_inline(line.trim_start_matches("**Ответ:**").trim_start_matches("Ответ:").trim_start_matches("ОТВЕТ:").trim())
            ));
            i += 1;
            continue;
        }

        // Обычные абзацы
        html.push_str(&format!("<p>{}</p>\n", format_inline(line)));
        i += 1;
    }

    if in_table {
        html.push_str(&render_table_html(&table_rows));
    }

    html
}

fn render_table_html(rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let mut out = String::from("<div class=\"table-container\"><table>\n");
    // Первая строка - шапка
    out.push_str("<thead><tr>\n");
    for col in &rows[0] {
        out.push_str(&format!("<th>{}</th>", format_inline(col)));
    }
    out.push_str("</tr></thead>\n<tbody>\n");

    for row in rows.iter().skip(1) {
        out.push_str("<tr>\n");
        for col in row {
            out.push_str(&format!("<td>{}</td>", format_inline(col)));
        }
        out.push_str("</tr>\n");
    }

    out.push_str("</tbody></table></div>\n");
    out
}

fn format_inline(text: &str) -> String {
    // 1. Код `code`
    let mut res = String::new();
    let parts: Vec<&str> = text.split('`').collect();
    for (idx, p) in parts.iter().enumerate() {
        if idx % 2 == 1 {
            res.push_str(&format!("<code>{}</code>", html_escape(p)));
        } else {
            // 2. Жирный **text**
            let bold_parts: Vec<&str> = p.split("**").collect();
            for (b_idx, bp) in bold_parts.iter().enumerate() {
                if b_idx % 2 == 1 {
                    res.push_str(&format!("<strong>{}</strong>", html_escape(bp)));
                } else {
                    res.push_str(&html_escape(bp));
                }
            }
        }
    }
    res
}
