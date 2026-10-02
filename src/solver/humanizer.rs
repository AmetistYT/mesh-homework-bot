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

    /// Очистка текста от любых Unicode-эмодзи
    pub fn strip_emojis(text: &str) -> String {
        text.chars()
            .filter(|&c| {
                !matches!(c,
                    '\u{1F600}'..='\u{1F64F}' | // Emoticons
                    '\u{1F300}'..='\u{1F5FF}' | // Misc Symbols and Pictographs
                    '\u{1F680}'..='\u{1F6FF}' | // Transport and Map
                    '\u{1F700}'..='\u{1F77F}' | // Alchemical Symbols
                    '\u{1F780}'..='\u{1F7FF}' | // Geometric Shapes Extended
                    '\u{1F800}'..='\u{1F8FF}' | // Supplemental Arrows-C
                    '\u{1F900}'..='\u{1F9FF}' | // Supplemental Symbols and Pictographs
                    '\u{1FA00}'..='\u{1FA6F}' | // Chess Symbols
                    '\u{1FA70}'..='\u{1FAFF}' | // Symbols and Pictographs Extended-A
                    '\u{2600}'..='\u{26FF}'   | // Misc symbols
                    '\u{2700}'..='\u{27BF}'   | // Dingbats
                    '\u{FE00}'..='\u{FE0F}'   | // Variation Selectors
                    '\u{1F1E6}'..='\u{1F1FF}'   // Regional indicator symbols
                )
            })
            .collect()
    }

    /// Специальная очистка ответов в AI-чате: убирает приветствия, концовки и эмодзи,
    /// сохраняя блоки действий модерации telegram_action
    pub fn clean_chat_reply(text: &str) -> String {
        let no_emojis = Self::strip_emojis(text);
        let mut lines: Vec<&str> = no_emojis.lines().collect();

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
            if trimmed.is_empty() || Self::is_ai_outro(trimmed) {
                lines.pop();
            } else {
                break;
            }
        }

        let result = lines.join("\n").trim().to_string();
        if result.is_empty() {
            no_emojis.trim().to_string()
        } else {
            result
        }
    }

    /// Проверка вводных шаблонных фраз ИИ
    fn is_ai_intro(line: &str) -> bool {
        let l = line.to_lowercase();
        l.starts_with("вот решение")
            || l.starts_with("ниже приведено")
            || l.starts_with("представляю решение")
            || l.starts_with("домашнее задание по")
            || l.starts_with("конечно, вот")
            || l.starts_with("конечно!")
            || l.starts_with("конечно,")
            || l.starts_with("с удовольствием решу")
            || l.starts_with("с радостью помогу")
            || l.starts_with("рассмотрим задачу")
            || l.starts_with("давайте решим")
            || l.starts_with("давайте разберемся")
            || l.starts_with("давай разберемся")
            || l.starts_with("привет!")
            || l.starts_with("привет,")
            || l.starts_with("привет ")
            || l.starts_with("здравствуйте")
            || l.starts_with("добрый день")
            || l.starts_with("доброе утро")
            || l.starts_with("добрый вечер")
            || l.starts_with("привожу подробное решение")
            || l.starts_with("вот ответ:")
            || l.starts_with("вот ответ на")
            || l.starts_with("по твоему запросу")
            || l.starts_with("по вашему запросу")
            || l.starts_with("отвечаю на вопрос")
            || l.starts_with("касательно твоего вопроса")
            || l.starts_with("что касается твоего вопроса")
            || l.starts_with("что касается вашего вопроса")
    }

    /// Проверка концовок ИИ
    fn is_ai_outro(line: &str) -> bool {
        let l = line.to_lowercase();
        l.starts_with("надеюсь, это поможет")
            || l.starts_with("надеюсь, это помогло")
            || l.starts_with("надеюсь, помог")
            || l.starts_with("если есть вопросы")
            || l.starts_with("если будут вопросы")
            || l.starts_with("если появятся вопросы")
            || l.starts_with("если возникнут вопросы")
            || l.starts_with("удачи в учебе")
            || l.starts_with("удачи на уроке")
            || l.starts_with("удачи!")
            || l.starts_with("успехов!")
            || l.starts_with("обращайтесь")
            || l.starts_with("обращайся")
            || l.starts_with("всегда рад помочь")
            || l.starts_with("всегда готов помочь")
            || l.starts_with("хорошего дня")
            || l.starts_with("если нужно что-то дополнить")
            || l.starts_with("готово!")
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
