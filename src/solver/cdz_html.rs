use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CdzExportTask {
    pub task_num: usize,
    pub task_id: i64,
    pub task_type_label: String,
    pub question_text: String,
    pub answer_summary: String,
    pub solution_text: String,
    pub explanation: String,
}

pub struct CdzHtmlExporter;

impl CdzHtmlExporter {
    pub fn render_html(
        subject_name: &str,
        material_title: &str,
        homework_id: i64,
        date_title: &str,
        status_badge: &str,
        is_submitted: bool,
        tasks: &[CdzExportTask],
    ) -> String {
        let template = include_str!("../../templates/cdz_template.html");
        let now_time = chrono::Local::now().format("%H:%M").to_string();
        let tasks_count = tasks.len().to_string();

        let mut cards_html = String::new();

        for t in tasks {
            let question_html = crate::solver::html_export::markdown_to_html(&t.question_text);
            let answer_html = crate::solver::html_export::html_escape(&t.answer_summary);
            let solution_html = crate::solver::html_export::markdown_to_html(&t.solution_text);
            let explanation_html = crate::solver::html_export::markdown_to_html(&t.explanation);

            let card = format!(
                r#"    <div class="task-card" id="task-card-{task_num}">
      <div class="task-card-header">
        <div class="task-title-group">
          <span class="task-number">Задание {task_num}</span>
          <span class="task-type-badge">{type_label}</span>
        </div>
        <span class="task-id-badge">ID: {task_id}</span>
      </div>

      <!-- ИЗНАЧАЛЬНО ОТКРЫТО: ЗАДАНИЕ -->
      <div class="question-box">
        <div class="section-label">Задание</div>
        <div class="question-content">{question_html}</div>
      </div>

      <!-- ИЗНАЧАЛЬНО ОТКРЫТО: ОТВЕТ -->
      <div class="answer-box">
        <div class="answer-header">
          <span class="answer-label">Итоговый ответ:</span>
          <button class="btn-copy-mini" onclick="copyText('answer-val-{task_num}', this)">Копировать</button>
        </div>
        <div class="answer-value" id="answer-val-{task_num}">{answer_html}</div>
      </div>

      <!-- ОТДЕЛЬНО РАЗВОРАЧИВАЕМЫЙ ХОД РЕШЕНИЯ -->
      <details class="cdz-detail-block" id="sol-detail-{task_num}">
        <summary class="cdz-detail-summary">
          <div class="summary-left">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>
            <span>Ход решения</span>
          </div>
          <svg class="chevron" viewBox="0 0 24 24"><polyline points="6 9 12 15 18 9"/></svg>
        </summary>
        <div class="cdz-detail-content">{solution_html}</div>
      </details>

      <!-- ОТДЕЛЬНО РАЗВОРАЧИВАЕМОЕ ОБОСНОВАНИЕ -->
      <details class="cdz-detail-block" id="exp-detail-{task_num}">
        <summary class="cdz-detail-summary">
          <div class="summary-left">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor"><circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>
            <span>Обоснование и правила</span>
          </div>
          <svg class="chevron" viewBox="0 0 24 24"><polyline points="6 9 12 15 18 9"/></svg>
        </summary>
        <div class="cdz-detail-content">{explanation_html}</div>
      </details>
    </div>
"#,
                task_num = t.task_num,
                task_id = t.task_id,
                type_label = crate::solver::html_export::html_escape(&t.task_type_label),
                question_html = question_html,
                answer_html = answer_html,
                solution_html = solution_html,
                explanation_html = explanation_html,
            );
            cards_html.push_str(&card);
        }

        let status_class = if is_submitted { "submitted" } else { "draft" };

        let mut output = template
            .replace("{{subject_name}}", &crate::solver::html_export::html_escape(subject_name))
            .replace("{{material_title}}", &crate::solver::html_export::html_escape(material_title))
            .replace("{{homework_id}}", &homework_id.to_string())
            .replace("{{date_title}}", &crate::solver::html_export::html_escape(date_title))
            .replace("{{status_badge}}", &crate::solver::html_export::html_escape(status_badge))
            .replace("{{status_class}}", status_class)
            .replace("{{tasks_count}}", &tasks_count)
            .replace("{{generated_time}}", &now_time);

        output = output.replace("{{#tasks}}", &cards_html);
        output
    }

    pub fn save_export_file<P: AsRef<Path>>(
        output_dir: P,
        subject_name: &str,
        homework_id: i64,
        html_content: &str,
    ) -> Result<PathBuf> {
        let dir = output_dir.as_ref();
        std::fs::create_dir_all(dir)?;

        let safe_sub = subject_name
            .replace(['/', '\\', '?', '%', '*', ':', '|', '"', '<', '>', '.', ' '], "_");
        let filename = format!("ЦДЗ_{}_{}.html", safe_sub, homework_id);
        let target_path = dir.join(&filename);
        std::fs::write(&target_path, html_content)?;
        Ok(target_path)
    }
}
