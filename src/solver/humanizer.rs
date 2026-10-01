/// Хуманизатор решений школьных ДЗ.
/// Убирает характерные маркеры ИИ ("войну и мир", вводные слова, пересказ условий, мета-комментарии)
/// и приводит решение к лаконичному виду, как будто его от руки написал обычный ученик 7 класса.
pub struct Humanizer;

impl Humanizer {
    pub fn humanize(text: &str) -> String {
        let mut lines: Vec<&str> = text.lines().collect();

        // 1. Убираем приветствия, вводные строки и мета-комментарии в начале
        while let Some(first) = lines.first() {
            let trimmed = first.trim();
            if trimmed.is_empty()
                || Self::is_ai_intro(trimmed)
                || trimmed.starts_with("```markdown")
                || trimmed == "```"
            {
                lines.remove(0);
            } else {
                break;
            }
        }

        // 2. Убираем заключительные фразы ИИ в конце
        while let Some(last) = lines.last() {
            let trimmed = last.trim();
            if trimmed.is_empty()
                || Self::is_ai_outro(trimmed)
                || trimmed == "```"
            {
                lines.pop();
            } else {
                break;
            }
        }

        let mut processed_lines = Vec::new();
        let mut in_code_or_table = false;

        for line in lines {
            let trimmed = line.trim();

            if trimmed.starts_with("```") || trimmed.starts_with('|') {
                in_code_or_table = !in_code_or_table || trimmed.starts_with('|');
                processed_lines.push(line.to_string());
                continue;
            }

            if in_code_or_table {
                processed_lines.push(line.to_string());
                continue;
            }

            // Пропускаем строки с типичной ИИ-водой
            if Self::is_fluff_line(trimmed) {
                continue;
            }

            let humanized = Self::clean_phrasing(line);
            processed_lines.push(humanized);
        }

        // Схлопываем лишние подряд идущие пустые строки (максимум 1 пустая строка подряд)
        let mut result = Vec::new();
        let mut prev_empty = false;

        for line in processed_lines {
            let is_empty = line.trim().is_empty();
            if is_empty {
                if !prev_empty {
                    result.push(line);
                    prev_empty = true;
                }
            } else {
                result.push(line);
                prev_empty = false;
            }
        }

        result.join("\n").trim().to_string()
    }

    /// Проверка вводных шаблонных фраз ИИ
    fn is_ai_intro(line: &str) -> bool {
        let l = line.to_lowercase();
        l.starts_with("вот решение")
            || l.starts_with("ниже приведено")
            || l.starts_with("представляю решение")
            || l.starts_with("домашнее задание по")
            || l.starts_with("конечно, вот")
            || l.starts_with("с удовольствием решу")
            || l.starts_with("рассмотрим задачу")
            || l.starts_with("давайте решим")
            || l.starts_with("привет!")
            || l.starts_with("здравствуйте")
            || l.starts_with("привожу подробное решение")
    }

    /// Проверка концовок ИИ
    fn is_ai_outro(line: &str) -> bool {
        let l = line.to_lowercase();
        l.starts_with("надеюсь, это поможет")
            || l.starts_with("если есть вопросы")
            || l.starts_with("удачи в учебе")
            || l.starts_with("обращайтесь")
            || l.starts_with("если нужно что-то дополнить")
            || l.starts_with("готово!")
            || l.starts_with("удачи на уроке")
    }

    /// Проверка водянистых строк
    fn is_fluff_line(line: &str) -> bool {
        let l = line.to_lowercase();
        // Рассуждения, которые ученик никогда не пишет в тетрадь
        l.starts_with("для того чтобы решить данную задачу")
            || l.starts_with("чтобы найти ответ, нам необходимо")
            || l.starts_with("вспомним формулу")
            || l.starts_with("как известно из курса")
            || l.starts_with("обратимся к параграфу")
            || l.starts_with("проанализируем условие")
            || l.starts_with("исходя из условия задачи")
            || l.starts_with("согласно определению из учебника")
            || l.starts_with("таким образом, в ходе решения")
    }

    /// Замена канцелярских фраз ИИ на естественные лаконичные фразы ученика
    fn clean_phrasing(line: &str) -> String {
        let mut s = line.to_string();

        let repls = [
            ("Таким образом, ", ""),
            ("Таким образом ", ""),
            ("Следовательно, ", ""),
            ("Следовательно ", ""),
            ("Исходя из этого, ", ""),
            ("Подставляя значения в формулу, получаем: ", ""),
            ("Подставим числовые значения: ", ""),
            ("Подставим значения: ", ""),
            ("Отсюда следует, что ", ""),
            ("Несложно заметить, что ", ""),
            ("Легко увидеть, что ", ""),
        ];

        for (from, to) in repls {
            s = s.replace(from, to);
        }

        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_humanize_removes_intro_and_outro() {
        let text = "Вот решение домашнего задания:\n\n№ 45\n\nДано:\nm = 5 кг\n\nОтвет: 5 кг\n\nНадеюсь, это поможет! Удачи в учебе!";
        let humanized = Humanizer::humanize(text);
        assert!(!humanized.contains("Вот решение"));
        assert!(!humanized.contains("Надеюсь, это поможет"));
        assert!(humanized.contains("№ 45"));
        assert!(humanized.contains("Ответ: 5 кг"));
    }

    #[test]
    fn test_humanize_removes_fluff() {
        let text = "№ 12\nДля того чтобы решить данную задачу, вспомним формулу:\ns = v * t\nПодставим значения: s = 10 * 2 = 20 м\nОтвет: 20 м";
        let humanized = Humanizer::humanize(text);
        assert!(!humanized.contains("Для того чтобы решить данную задачу"));
        assert!(humanized.contains("s = 10 * 2 = 20 м"));
    }
}
