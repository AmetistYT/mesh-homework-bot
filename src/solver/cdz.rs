use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::mesh::cdz_client::CdzTask;
use crate::solver::SolverRunner;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolvedCdzTask {
    pub task_id: i64,
    pub task_type_label: String,
    pub question_text: String,
    pub explanation: String,
    pub answer_summary: String,
    pub solution_text: String,
    pub payload: serde_json::Value,
}

pub struct CdzSolver;

impl CdzSolver {
    pub fn get_task_type_label(answer_type: &str) -> String {
        match answer_type {
            "answer/single" => "Одиночный выбор".to_string(),
            "answer/multiple" => "Множественный выбор".to_string(),
            "answer/match" => "Сопоставление".to_string(),
            "answer/groups" => "Группировка".to_string(),
            "answer/string" => "Текстовый ответ".to_string(),
            "answer/number" => "Числовой расчет".to_string(),
            _ => "Тестовое задание".to_string(),
        }
    }

    /// Решает одно задание ЦДЗ через SolverRunner (gemini-3.8-flash-medium)
    pub async fn solve_task(
        runner: &SolverRunner,
        task: &CdzTask,
        subject: &str,
        task_num: usize,
        total_tasks: usize,
    ) -> Result<SolvedCdzTask> {
        let question_text = task
            .question_elements
            .iter()
            .filter_map(|q| q.text.as_ref())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");

        let answer_type = task.answer.answer_type.as_str();
        let task_type_label = Self::get_task_type_label(answer_type);

        info!(
            "Решение задания #{}/{} (ID: {}, тип: {}) по предмету '{}'...",
            task_num, total_tasks, task.id, answer_type, subject
        );

        let prompt = Self::build_prompt(subject, task_num, total_tasks, &question_text, answer_type, &task.answer.options);

        // Таймаут 75 секунд на решение одного задания, чтобы не подвешивать весь процесс
        let ai_response = match tokio::time::timeout(
            std::time::Duration::from_secs(75),
            runner.solve(&prompt)
        ).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => return Err(anyhow!("Сбой вызова модели для задания ЦДЗ: {}", e)),
            Err(_) => return Err(anyhow!("Превышен таймаут решения задания (75 сек)")),
        };

        let (payload, explanation, summary, solution_text) = Self::parse_ai_response(&ai_response, answer_type, task)?;

        Ok(SolvedCdzTask {
            task_id: task.id,
            task_type_label,
            question_text,
            explanation,
            answer_summary: summary,
            solution_text,
            payload,
        })
    }

    fn build_prompt(
        subject: &str,
        task_num: usize,
        total_tasks: usize,
        question: &str,
        answer_type: &str,
        options: &[crate::mesh::cdz_client::CdzOption],
    ) -> String {
        let mut p = String::new();
        p.push_str(&format!(
            "Ты эксперт по школьным предметам и решаешь официальное цифровое домашнее задание (ЦДЗ МЭШ) по предмету '{}'.\n",
            subject
        ));
        p.push_str(&format!("Номер задания: {} из {}.\n\n", task_num, total_tasks));
        p.push_str("ТЕКСТ ЗАДАНИЯ:\n");
        p.push_str(question);
        p.push_str("\n\n");

        match answer_type {
            "answer/single" => {
                p.push_str("ТИП ЗАДАНИЯ: Одиночный выбор (выбрать один правильный вариант).\nВАРИАНТЫ ОТВЕТА:\n");
                for opt in options {
                    let text = opt.text.as_deref().unwrap_or("");
                    p.push_str(&format!("- ID: \"{}\" | Текст: \"{}\"\n", opt.id, text));
                }
                p.push_str("\nИНСТРУКЦИЯ:\n");
                p.push_str("Выбери ровно один правильный вариант ответа.\n");
                p.push_str("Ответь строго в формате JSON в блоке ```json ... ```:\n");
                p.push_str("```json\n");
                p.push_str("{\n");
                p.push_str("  \"selected_id\": \"<ID выбранного варианта>\",\n");
                p.push_str("  \"solution\": \"Пошаговый ход решения и разбор задания на русском языке\",\n");
                p.push_str("  \"explanation\": \"Правило, цитата из текста или краткое обоснование\"\n");
                p.push_str("}\n");
                p.push_str("```\n");
            }
            "answer/multiple" => {
                p.push_str("ТИП ЗАДАНИЯ: Множественный выбор (выбрать все верные варианты).\nВАРИАНТЫ ОТВЕТА:\n");
                for opt in options {
                    let text = opt.text.as_deref().unwrap_or("");
                    p.push_str(&format!("- ID: \"{}\" | Текст: \"{}\"\n", opt.id, text));
                }
                p.push_str("\nИНСТРУКЦИЯ:\n");
                p.push_str("Выбери все верные варианты ответа.\n");
                p.push_str("Ответь строго в формате JSON в блоке ```json ... ```:\n");
                p.push_str("```json\n");
                p.push_str("{\n");
                p.push_str("  \"selected_ids\": [\"<ID 1>\", \"<ID 2>\"],\n");
                p.push_str("  \"solution\": \"Пошаговый ход решения и анализ каждого варианта на русском языке\",\n");
                p.push_str("  \"explanation\": \"Правила или цитаты для подтверждения верных вариантов\"\n");
                p.push_str("}\n");
                p.push_str("```\n");
            }
            "answer/match" => {
                p.push_str("ТИП ЗАДАНИЯ: Установление соответствий (сопоставить элементы левого столбца с правым).\n");
                let sources: Vec<_> = options.iter().filter(|o| o.option_type.as_deref() == Some("option_type/match/source")).collect();
                let targets: Vec<_> = options.iter().filter(|o| o.option_type.as_deref() == Some("option_type/match/target")).collect();

                p.push_str("ЛЕВЫЙ СТОЛБЕЦ (Исходные элементы / Source):\n");
                for s in &sources {
                    p.push_str(&format!("- Source ID: \"{}\" | Текст: \"{}\"\n", s.id, s.text.as_deref().unwrap_or("")));
                }
                p.push_str("\nПРАВЫЙ СТОЛБЕЦ (Целевые элементы / Target):\n");
                for t in &targets {
                    p.push_str(&format!("- Target ID: \"{}\" | Текст: \"{}\"\n", t.id, t.text.as_deref().unwrap_or("")));
                }
                p.push_str("\nИНСТРУКЦИЯ:\n");
                p.push_str("Сопоставь каждый Source ID с соответствующим Target ID.\n");
                p.push_str("Ответь строго в формате JSON в блоке ```json ... ```:\n");
                p.push_str("```json\n");
                p.push_str("{\n");
                p.push_str("  \"matches\": {\n");
                p.push_str("    \"<Source ID 1>\": [\"<Target ID>\"]\n");
                p.push_str("  },\n");
                p.push_str("  \"solution\": \"Пошаговый разбор соответствий и сопоставления на русском языке\",\n");
                p.push_str("  \"explanation\": \"Обоснование каждого сопоставления\"\n");
                p.push_str("}\n");
                p.push_str("```\n");
            }
            "answer/groups" => {
                p.push_str("ТИП ЗАДАНИЯ: Распределение элементов по группам (например, True/False или категории).\n");
                let groups: Vec<_> = options.iter().filter(|o| o.option_type.as_deref() == Some("option_type/group")).collect();
                let items: Vec<_> = options.iter().filter(|o| o.option_type.as_deref() != Some("option_type/group")).collect();

                p.push_str("ГРУППЫ (Категории):\n");
                for g in &groups {
                    p.push_str(&format!("- Group ID: \"{}\" | Название: \"{}\"\n", g.id, g.text.as_deref().unwrap_or("")));
                }
                p.push_str("\nЭЛЕМЕНТЫ ДЛЯ РАСПРЕДЕЛЕНИЯ:\n");
                for it in &items {
                    p.push_str(&format!("- Item ID: \"{}\" | Текст: \"{}\"\n", it.id, it.text.as_deref().unwrap_or("")));
                }
                p.push_str("\nИНСТРУКЦИЯ:\n");
                p.push_str("Распредели Item ID по соответствующим Group ID.\n");
                p.push_str("Ответь строго в формате JSON в блоке ```json ... ```:\n");
                p.push_str("```json\n");
                p.push_str("{\n");
                p.push_str("  \"groups\": [\n");
                p.push_str("    {\n");
                p.push_str("      \"group_id\": \"<Group ID>\",\n");
                p.push_str("      \"options_ids\": [\"<Item ID 1>\", \"<Item ID 2>\"]\n");
                p.push_str("    }\n");
                p.push_str("  ],\n");
                p.push_str("  \"solution\": \"Пошаговый анализ распределения элементов по группам на русском языке\",\n");
                p.push_str("  \"explanation\": \"Обоснование распределения\"\n");
                p.push_str("}\n");
                p.push_str("```\n");
            }
            "answer/string" => {
                p.push_str("ТИП ЗАДАНИЯ: Ввод текстового ответа.\n");
                p.push_str("ИНСТРУКЦИЯ:\n");
                p.push_str("Сформулируй точный текстовый ответ (слово или словосочетание).\n");
                p.push_str("Ответь строго в формате JSON в блоке ```json ... ```:\n");
                p.push_str("```json\n");
                p.push_str("{\n");
                p.push_str("  \"answer_string\": \"<текст ответа>\",\n");
                p.push_str("  \"solution\": \"Пошаговый ход решения и вывод формулировки ответа на русском языке\",\n");
                p.push_str("  \"explanation\": \"Грамматическое или смысловое обоснование\"\n");
                p.push_str("}\n");
                p.push_str("```\n");
            }
            "answer/number" => {
                p.push_str("ТИП ЗАДАНИЯ: Ввод числового ответа.\n");
                p.push_str("ИНСТРУКЦИЯ:\n");
                p.push_str("Рассчитай точное числовое значение.\n");
                p.push_str("Ответь строго в формате JSON в блоке ```json ... ```:\n");
                p.push_str("```json\n");
                p.push_str("{\n");
                p.push_str("  \"answer_number\": <число или числовая строка>,\n");
                p.push_str("  \"solution\": \"Пошаговый расчет и вычисления на русском языке\",\n");
                p.push_str("  \"explanation\": \"Формулы и пояснения к расчету\"\n");
                p.push_str("}\n");
                p.push_str("```\n");
            }
            _ => {
                p.push_str(&format!("ТИП ЗАДАНИЯ: {}\n", answer_type));
                for opt in options {
                    p.push_str(&format!("- ID: \"{}\" | Текст: \"{}\"\n", opt.id, opt.text.as_deref().unwrap_or("")));
                }
                p.push_str("Ответь строго в формате JSON в блоке ```json ... ``` с полями ответа, \"solution\" и \"explanation\".\n");
            }
        }

        p.push_str("\nНе используй эмодзи. Отвечай исключительно по существу.\n");
        p
    }

    fn extract_json_str(text: &str) -> Option<String> {
        let re_block = Regex::new(r"```(?:json)?\s*([\s\S]*?)\s*```").ok();
        if let Some(re) = re_block {
            for caps in re.captures_iter(text) {
                if let Some(m) = caps.get(1) {
                    let candidate = m.as_str().trim();
                    if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                        return Some(candidate.to_string());
                    }
                }
            }
        }
        if let Some(start) = text.find('{') {
            if let Some(end) = text.rfind('}') {
                if end > start {
                    let candidate = &text[start..=end];
                    if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                        return Some(candidate.to_string());
                    }
                }
            }
        }
        None
    }

    fn parse_ai_response(
        response_text: &str,
        answer_type: &str,
        task: &CdzTask,
    ) -> Result<(serde_json::Value, String, String, String)> {
        let json_str = Self::extract_json_str(response_text)
            .ok_or_else(|| anyhow!("В ответе нейросети не найден блок JSON"))?;

        let parsed: serde_json::Value = serde_json::from_str(&json_str)
            .context(format!("Ошибка парсинга JSON нейросети: {}", json_str))?;

        let explanation = parsed["explanation"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Решение получено автоматически".to_string());

        let solution = parsed["solution"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                format!("Пошаговый анализ задания и выбор правильного ответа в соответствии с условиями.\n{}", explanation)
            });

        match answer_type {
            "answer/single" => {
                let selected_id = parsed["selected_id"]
                    .as_str()
                    .or_else(|| parsed["id"].as_str())
                    .or_else(|| parsed["option_id"].as_str())
                    .or_else(|| parsed["answer"].as_str())
                    .map(|s| s.to_string())
                    .or_else(|| parsed["selected_id"].as_i64().map(|n| n.to_string()))
                    .or_else(|| parsed["id"].as_i64().map(|n| n.to_string()))
                    .unwrap_or_else(|| {
                        task.answer.options.first().map(|o| o.id.clone()).unwrap_or_default()
                    });

                let opt_match = task.answer.options.iter()
                    .find(|o| o.id == selected_id || o.text.as_deref().unwrap_or("").eq_ignore_ascii_case(&selected_id))
                    .or_else(|| task.answer.options.first());

                let final_id = opt_match.map(|o| o.id.as_str()).unwrap_or(&selected_id);
                let opt_text = opt_match.and_then(|o| o.text.as_ref()).map(|s| s.as_str()).unwrap_or(final_id);

                let payload = serde_json::json!({
                    "@answer_type": "answer/single",
                    "id": final_id
                });
                let summary = format!("Выбран вариант: {}", opt_text);
                Ok((payload, explanation, summary, solution))
            }
            "answer/multiple" => {
                let mut ids = Vec::new();
                if let Some(arr) = parsed["selected_ids"].as_array().or_else(|| parsed["ids"].as_array()) {
                    for v in arr {
                        if let Some(s) = v.as_str() {
                            ids.push(s.to_string());
                        } else if let Some(n) = v.as_i64() {
                            ids.push(n.to_string());
                        }
                    }
                }
                if ids.is_empty() {
                    if let Some(first) = task.answer.options.first() {
                        ids.push(first.id.clone());
                    }
                }
                let names = ids.iter().map(|id| {
                    task.answer.options.iter()
                        .find(|o| &o.id == id || o.text.as_deref().unwrap_or("").eq_ignore_ascii_case(id))
                        .and_then(|o| o.text.as_ref())
                        .map(|s| s.as_str())
                        .unwrap_or(id.as_str())
                }).collect::<Vec<_>>().join(", ");

                let payload = serde_json::json!({
                    "@answer_type": "answer/multiple",
                    "ids": ids
                });
                let summary = format!("Выбраны варианты: {}", names);
                Ok((payload, explanation, summary, solution))
            }
            "answer/match" => {
                let mut matches_map = serde_json::Map::new();
                let mut summary_parts = Vec::new();

                if let Some(obj) = parsed["matches"].as_object().or_else(|| parsed["match"].as_object()) {
                    for (src_id, tgt_val) in obj {
                        let src_text = task.answer.options.iter()
                            .find(|o| &o.id == src_id || o.text.as_deref().unwrap_or("").eq_ignore_ascii_case(src_id))
                            .and_then(|o| o.text.as_ref())
                            .map(|s| s.as_str())
                            .unwrap_or(src_id.as_str());

                        let tgt_id = tgt_val.as_array()
                            .and_then(|a| a.first())
                            .and_then(|v| v.as_str())
                            .or_else(|| tgt_val.as_str())
                            .unwrap_or("");

                        let tgt_text = task.answer.options.iter()
                            .find(|o| o.id == tgt_id || o.text.as_deref().unwrap_or("").eq_ignore_ascii_case(tgt_id))
                            .and_then(|o| o.text.as_ref())
                            .map(|s| s.as_str())
                            .unwrap_or(tgt_id);

                        matches_map.insert(src_id.clone(), serde_json::json!([tgt_id]));
                        summary_parts.push(format!("{} -> {}", src_text, tgt_text));
                    }
                } else if let Some(arr) = parsed["matches"].as_array() {
                    for item in arr {
                        let s_id = item["source_id"].as_str().or_else(|| item["src"].as_str()).unwrap_or_default();
                        let t_id = item["target_id"].as_str().or_else(|| item["tgt"].as_str()).unwrap_or_default();
                        if !s_id.is_empty() {
                            matches_map.insert(s_id.to_string(), serde_json::json!([t_id]));
                            summary_parts.push(format!("{} -> {}", s_id, t_id));
                        }
                    }
                }

                let payload = serde_json::json!({
                    "@answer_type": "answer/match",
                    "match": matches_map
                });
                let summary = if summary_parts.is_empty() {
                    "Сопоставления сформированы".to_string()
                } else {
                    format!("Сопоставления:\n{}", summary_parts.join("\n"))
                };
                Ok((payload, explanation, summary, solution))
            }
            "answer/groups" => {
                let mut groups_arr = Vec::new();
                let mut summary_parts = Vec::new();

                if let Some(arr) = parsed["groups"].as_array() {
                    for grp in arr {
                        let grp_id = grp["group_id"].as_str().unwrap_or_default();
                        let grp_text = task.answer.options.iter()
                            .find(|o| o.id == grp_id || o.text.as_deref().unwrap_or("").trim().eq_ignore_ascii_case(grp_id.trim()))
                            .and_then(|o| o.text.as_ref())
                            .map(|s| s.as_str())
                            .unwrap_or(grp_id);

                        let opt_ids = grp["options_ids"].as_array();
                        let mut item_names = Vec::new();
                        let mut item_ids_clean = Vec::new();
                        if let Some(items) = opt_ids {
                            for item in items {
                                if let Some(iid) = item.as_str() {
                                    item_ids_clean.push(serde_json::json!(iid));
                                    let iname = task.answer.options.iter()
                                        .find(|o| o.id == iid || o.text.as_deref().unwrap_or("").trim().eq_ignore_ascii_case(iid.trim()))
                                        .and_then(|o| o.text.as_ref())
                                        .map(|s| s.as_str())
                                        .unwrap_or(iid);
                                    item_names.push(iname);
                                }
                            }
                        }
                        groups_arr.push(serde_json::json!({
                            "group_id": grp_id,
                            "options_ids": item_ids_clean
                        }));
                        summary_parts.push(format!("[{}]: {}", grp_text, item_names.join("; ")));
                    }
                } else if let Some(obj) = parsed["groups"].as_object() {
                    for (grp_key, items_val) in obj {
                        let opt_match = task.answer.options.iter()
                            .find(|o| &o.id == grp_key || o.text.as_deref().unwrap_or("").trim().eq_ignore_ascii_case(grp_key.trim()));
                        let grp_id = opt_match.map(|o| o.id.as_str()).unwrap_or(grp_key);
                        let grp_text = opt_match.and_then(|o| o.text.as_ref()).map(|s| s.as_str()).unwrap_or(grp_key);

                        let mut item_ids_clean = Vec::new();
                        let mut item_names = Vec::new();
                        if let Some(items) = items_val.as_array() {
                            for item in items {
                                if let Some(iid) = item.as_str() {
                                    let it_match = task.answer.options.iter()
                                        .find(|o| o.id == iid || o.text.as_deref().unwrap_or("").trim().eq_ignore_ascii_case(iid.trim()));
                                    let final_iid = it_match.map(|o| o.id.as_str()).unwrap_or(iid);
                                    let iname = it_match.and_then(|o| o.text.as_ref()).map(|s| s.as_str()).unwrap_or(final_iid);
                                    item_ids_clean.push(serde_json::json!(final_iid));
                                    item_names.push(iname);
                                }
                            }
                        }
                        groups_arr.push(serde_json::json!({
                            "group_id": grp_id,
                            "options_ids": item_ids_clean
                        }));
                        summary_parts.push(format!("[{}]: {}", grp_text, item_names.join("; ")));
                    }
                }

                let payload = serde_json::json!({
                    "@answer_type": "answer/groups",
                    "groups": groups_arr
                });
                let summary = if summary_parts.is_empty() {
                    "Распределение по группам сформировано".to_string()
                } else {
                    format!("Распределение по группам:\n{}", summary_parts.join("\n"))
                };
                Ok((payload, explanation, summary, solution))
            }
            "answer/string" => {
                let s = parsed["answer_string"]
                    .as_str()
                    .or_else(|| parsed["answer"].as_str())
                    .or_else(|| parsed["text"].as_str())
                    .unwrap_or("");

                let payload = serde_json::json!({
                    "@answer_type": "answer/string",
                    "string": [s.trim()]
                });
                let summary = format!("Введенный ответ: {}", s.trim());
                Ok((payload, explanation, summary, solution))
            }
            "answer/number" => {
                let num_val = if let Some(n) = parsed["answer_number"].as_f64().or_else(|| parsed["number"].as_f64()) {
                    serde_json::json!([n])
                } else if let Some(s) = parsed["answer_number"].as_str().or_else(|| parsed["number"].as_str()).or_else(|| parsed["answer"].as_str()) {
                    serde_json::json!([s.trim()])
                } else {
                    serde_json::json!([0])
                };

                let payload = serde_json::json!({
                    "@answer_type": "answer/number",
                    "number": num_val
                });
                let summary = format!("Числовой ответ: {}", payload["number"]);
                Ok((payload, explanation, summary, solution))
            }
            _ => {
                let payload = serde_json::json!({
                    "@answer_type": answer_type
                });
                let summary = format!("Ответ сформирован для типа {}", answer_type);
                Ok((payload, explanation, summary, solution))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::cdz_client::{CdzOption, CdzQuestionElement, CdzTask, CdzTaskAnswer};

    #[test]
    fn test_parse_single_choice() {
        let task = CdzTask {
            id: 101,
            task_order: Some(0),
            question_elements: vec![CdzQuestionElement {
                text: Some("Выберите правильный вариант".to_string()),
                element_type: None,
            }],
            answer: CdzTaskAnswer {
                answer_type: "answer/single".to_string(),
                options: vec![
                    CdzOption {
                        id: "opt-1".to_string(),
                        text: Some("Вариант А".to_string()),
                        option_type: None,
                    },
                    CdzOption {
                        id: "opt-2".to_string(),
                        text: Some("Вариант Б".to_string()),
                        option_type: None,
                    },
                ],
            },
            user_answer: None,
        };

        let ai_response = "```json\n{\n  \"selected_id\": \"opt-1\",\n  \"solution\": \"Проверяем варианты и выбираем А\",\n  \"explanation\": \"Правильный выбор по контексту\"\n}\n```";
        let (payload, expl, summary, sol) = CdzSolver::parse_ai_response(ai_response, "answer/single", &task).unwrap();

        assert_eq!(payload["@answer_type"], "answer/single");
        assert_eq!(payload["id"], "opt-1");
        assert_eq!(expl, "Правильный выбор по контексту");
        assert_eq!(sol, "Проверяем варианты и выбираем А");
        assert!(summary.contains("Вариант А"));
    }

    #[test]
    fn test_parse_match() {
        let task = CdzTask {
            id: 102,
            task_order: Some(1),
            question_elements: vec![],
            answer: CdzTaskAnswer {
                answer_type: "answer/match".to_string(),
                options: vec![
                    CdzOption {
                        id: "src-1".to_string(),
                        text: Some("Hello".to_string()),
                        option_type: Some("option_type/match/source".to_string()),
                    },
                    CdzOption {
                        id: "tgt-1".to_string(),
                        text: Some("Привет".to_string()),
                        option_type: Some("option_type/match/target".to_string()),
                    },
                ],
            },
            user_answer: None,
        };

        let ai_response = "```json\n{\n  \"matches\": {\n    \"src-1\": [\"tgt-1\"]\n  },\n  \"solution\": \"Сопоставляем английское слово с русским переводом\",\n  \"explanation\": \"Перевод\"\n}\n```";
        let (payload, _expl, summary, sol) = CdzSolver::parse_ai_response(ai_response, "answer/match", &task).unwrap();

        assert_eq!(payload["@answer_type"], "answer/match");
        assert_eq!(payload["match"]["src-1"][0], "tgt-1");
        assert_eq!(sol, "Сопоставляем английское слово с русским переводом");
        assert!(summary.contains("Hello -> Привет"));
    }

    #[test]
    fn test_parse_groups() {
        let task = CdzTask {
            id: 103,
            task_order: Some(2),
            question_elements: vec![],
            answer: CdzTaskAnswer {
                answer_type: "answer/groups".to_string(),
                options: vec![
                    CdzOption {
                        id: "grp-t".to_string(),
                        text: Some("True".to_string()),
                        option_type: Some("option_type/group".to_string()),
                    },
                    CdzOption {
                        id: "grp-f".to_string(),
                        text: Some("False".to_string()),
                        option_type: Some("option_type/group".to_string()),
                    },
                    CdzOption {
                        id: "item-1".to_string(),
                        text: Some("Земля круглая".to_string()),
                        option_type: Some("option_type/default".to_string()),
                    },
                ],
            },
            user_answer: None,
        };

        let ai_response = "```json\n{\n  \"groups\": [\n    {\n      \"group_id\": \"grp-t\",\n      \"options_ids\": [\"item-1\"]\n    }\n  ],\n  \"solution\": \"Оцениваем истинность утверждения\",\n  \"explanation\": \"Факт\"\n}\n```";
        let (payload, _expl, summary, sol) = CdzSolver::parse_ai_response(ai_response, "answer/groups", &task).unwrap();

        assert_eq!(payload["@answer_type"], "answer/groups");
        assert_eq!(payload["groups"][0]["group_id"], "grp-t");
        assert_eq!(payload["groups"][0]["options_ids"][0], "item-1");
        assert_eq!(sol, "Оцениваем истинность утверждения");
        assert!(summary.contains("[True]: Земля круглая"));
    }
}

