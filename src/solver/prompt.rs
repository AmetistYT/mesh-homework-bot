use std::path::{Path, PathBuf};

pub struct PromptBuilder {
    textbooks_dir: PathBuf,
    rules_dir: PathBuf,
}

impl PromptBuilder {
    pub fn new<P: AsRef<Path>, R: AsRef<Path>>(textbooks_dir: P, rules_dir: R) -> Self {
        Self {
            textbooks_dir: textbooks_dir.as_ref().to_path_buf(),
            rules_dir: rules_dir.as_ref().to_path_buf(),
        }
    }

    /// Поиск наиболее подходящих учебников для предмета
    pub fn find_textbooks_for_subject(&self, subject: &str) -> Vec<PathBuf> {
        let sub = subject.to_lowercase();
        let mut matches = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&self.textbooks_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|ext| ext == "pdf") {
                    let filename = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase();

                    let is_match = match sub.as_str() {
                        s if s.contains("алгебр") || s.contains("матем") => {
                            filename.contains("алгебра") || filename.contains("макарычев")
                        }
                        s if s.contains("геометр") => {
                            filename.contains("геометрия") || filename.contains("атанасян")
                        }
                        s if s.contains("физик") => {
                            filename.contains("физика")
                                || filename.contains("перышкин")
                                || filename.contains("лукашик")
                        }
                        s if s.contains("русск") => {
                            filename.contains("русский")
                                || filename.contains("ладыженская")
                                || filename.contains("богданова")
                        }
                        s if s.contains("литератур") => {
                            filename.contains("литература") || filename.contains("коровина")
                        }
                        s if s.contains("англ") => {
                            filename.contains("starlight") || filename.contains("английский")
                        }
                        s if s.contains("биолог") => {
                            filename.contains("биология") || filename.contains("пасечник")
                        }
                        s if s.contains("географ") => {
                            filename.contains("география") || filename.contains("душина")
                        }
                        s if s.contains("истор") => {
                            filename.contains("история") || filename.contains("мединский")
                        }
                        s if s.contains("хим") => {
                            filename.contains("химия") || filename.contains("габриелян")
                        }
                        _ => false,
                    };

                    if is_match {
                        matches.push(path);
                    }
                }
            }
        }

        matches
    }

    /// Загрузка правил оформления по предмету (или общих правил)
    pub fn load_subject_rules(&self, subject: &str) -> Option<String> {
        let sub = subject.to_lowercase();
        let rule_filename = match sub.as_str() {
            s if s.contains("алгебр") || s.contains("матем") => "algebra.md",
            s if s.contains("геометр") => "geometry.md",
            s if s.contains("физик") => "physics.md",
            s if s.contains("русск") => "russian.md",
            s if s.contains("литератур") => "literature.md",
            s if s.contains("англ") => "english.md",
            s if s.contains("биолог") => "biology.md",
            s if s.contains("географ") => "geography.md",
            s if s.contains("истор") => "history.md",
            s if s.contains("хим") => "chemistry.md",
            _ => "general.md",
        };

        let path = self.rules_dir.join(rule_filename);
        if path.exists() {
            std::fs::read_to_string(path).ok()
        } else {
            let gen_path = self.rules_dir.join("general.md");
            if gen_path.exists() {
                std::fs::read_to_string(gen_path).ok()
            } else {
                None
            }
        }
    }

    /// Построение итогового промпта для agy
    pub fn build_prompt(
        &self,
        subject: &str,
        date: &str,
        description: &str,
        attachments_info: Option<&str>,
    ) -> String {
        let textbooks = self.find_textbooks_for_subject(subject);
        let rules = self.load_subject_rules(subject);
        let sub_lower = subject.to_lowercase();

        let mut p = String::new();
        p.push_str("Ты — эксперт-репетитор для ученика 7 класса.\n");
        p.push_str("Твоя задача: решить домашнее задание из электронного дневника МЭШ.\n\n");

        p.push_str(&format!("ПРЕДМЕТ: {}\n", subject));
        p.push_str(&format!("СРОК СДАЧИ: {}\n", date));
        p.push_str(&format!("ТЕКСТ ЗАДАНИЯ УЧИТЕЛЯ:\n\"\"\"\n{}\n\"\"\"\n\n", description.trim()));

        // Специальные подсказки по предметам
        if sub_lower.contains("физик") {
            p.push_str("ВАЖНОЕ РАЗЪЯСНЕНИЕ СОКРАЩЕНИЙ ПО ФИЗИКЕ:\n");
            p.push_str("- Буква 'П' (например, 'П.14,15', 'П14', 'П 14') означает параграфы учебника Перышкина. Если к ним не указаны упражнения, напиши краткий конспект ключевых определений и формул этих параграфов (1-2 предложения).\n");
            p.push_str("- Буква 'Л' (например, 'Л.98,99', 'Л98', 'Л 98') означает задачи из 'Сборника задач по физике 7-9 класс' Лукашика. Найди условие номеров в PDF-файле сборника и реши их.\n");
            p.push_str("- Оформление задач по физике СТРОГО ЧЕРЕЗ 'Дано', 'СИ', 'Найти', 'Решение' и 'Ответ'.\n\n");
        } else if sub_lower.contains("алгебр") || sub_lower.contains("матем") {
            p.push_str("ВАЖНОЕ ТРЕБОВАНИЕ К ОФОРМЛЕНИЮ АЛГЕБРЫ:\n");
            p.push_str("- СТРОГО НЕ ПИСАТЬ 'Дано:'! В алгебре 'Дано' писать категорически запрещено. Решай напрямую от исходного выражения/уравнения к преобразованиям и ответу.\n");
            p.push_str("- ЕСЛИ В ЗАДАНИИ ТРЕБУЕТСЯ ПОСТРОИТЬ ГРАФИК, ЧЕРТЕЖ ИЛИ ЧИСЛОВУЮ ПРЯМУЮ:\n");
            p.push_str("  Обязательно включи в решение блок Python-кода с библиотекой matplotlib (сохраняющий график в 'plot.png') ИЛИ блок чистого SVG-кода (внутри тегов ```xml <svg>...</svg>```). Сервис автоматически преобразует его в изображение и отправит в группу.\n\n");
        } else if sub_lower.contains("геометр") {
            p.push_str("ВАЖНОЕ ТРЕБОВАНИЕ К ОФОРМЛЕНИЮ ГЕОМЕТРИИ:\n");
            p.push_str("- Оформление СТРОГО ЧЕРЕЗ 'Дано:', 'Найти:' (или 'Доказать:'), 'Решение:' (или 'Доказательство:'), 'Ответ:'.\n");
            p.push_str("- Ссылайся на признаки равенства треугольников, теоремы и аксиомы.\n");
            p.push_str("- При необходимости чертежа можно также привести блок SVG или Python для построения фигуры.\n\n");
        } else if sub_lower.contains("англ") {
            p.push_str("ВАЖНОЕ ПОЯСНЕНИЕ ПО АНГЛИЙСКОМУ ЯЗЫКУ:\n");
            p.push_str("- Основной учебник — 'Starlight_7_Student_Book_Звездный_английский_Баранова.pdf' и 'Starlight_7_Workbook_Звездный_английский_Рабочая_тетрадь.pdf'.\n");
            p.push_str("- Если в задании сказано 'выучить диалог стр. X №Y' — найди этот диалог в учебнике Starlight 7, приведи его полный английский текст и перевод на русский язык.\n\n");
        } else if sub_lower.contains("истор") {
            p.push_str("ВАЖНОЕ ПОЯСНЕНИЕ ПО ИСТОРИИ:\n");
            p.push_str("- Основной и актуальный учебник: 'История_Нового_времени_7_класс_Мединский_Чубарьян_2025.pdf'.\n");
            p.push_str("- Используй строго это издание 2025 года для поиска тем, параграфов и пунктов (например, в издании 2025 года § 6 — это 'Реформация и Контрреформация', пункт 4 — 'Кальвин и кальвинизм', пункт 5 — 'Борьба за души верующих').\n\n");
        }

        if let Some(att) = attachments_info {
            p.push_str(&format!("ПРИКРЕПЛЕННЫЕ МАТЕРИАЛЫ / ВЛОЖЕНИЯ:\n{}\n\n", att));
        }

        if !textbooks.is_empty() {
            p.push_str("ДОСТУПНЫЕ УЧЕБНИКИ В ТВОЕЙ ПАПКЕ:\n");
            for tb in textbooks {
                if let Some(name) = tb.file_name() {
                    p.push_str(&format!("- {}\n", name.to_string_lossy()));
                }
            }
            p.push('\n');
        }

        if let Some(r) = rules {
            p.push_str("ДОПОЛНИТЕЛЬНЫЕ ПРАВИЛА И ТРЕБОВАНИЯ УЧИТЕЛЯ К ОФОРМЛЕНИЮ:\n");
            p.push_str(&r);
            p.push_str("\n\n");
        }

        p.push_str("СТРОГИЕ ПРАВИЛА ВЫПОЛНЕНИЯ (СТИЛЬ И ХУМАНИЗАЦИЯ):\n");
        p.push_str("1. СТИЛЬ РЕШЕНИЯ — ОБЫЧНЫЙ СЕМИКЛАССНИК: Пиши так, как реальный ученик пишет синей ручкой в своей тетради. Простой, живой, естественный язык. НИКАКОЙ «войны и мира», высокопарности, заумных книжных оборотов и канцелярщины!\n");
        p.push_str("2. НОЛЬ ВОДЫ: Категорически запрещены любые приветствия, предисловия («Вот решение:», «Рассмотрим задачу...»), пересказы условия задачи, рассуждения вслух («Для того чтобы найти...», «Вспомним формулу...») и прощания («Надеюсь, помог!»).\n");
        p.push_str("3. СРАЗУ К СУТИ: Сразу пиши номер задания, краткое оформление (Дано/Решение для физики и геометрии, либо сразу пример и преобразования для алгебры), действия в 1-2 строки и финальный «Ответ: ...».\n");
        p.push_str("4. КОНСПЕКТЫ И ПАРАГРАФЫ: Если задан параграф или вопросы к нему — отвечай максимально емко, строго по фактам (1-3 коротких предложения на пункт), без многостраничных эссе и «воды».\n");
        p.push_str("5. Твоя текущая папка содержит файлы учебников. КАТЕГОРИЧЕСКИ ЗАПРЕЩЕНО исследовать посторонние репозитории, проектные файлы бота или базы данных SQLite!\n");
        p.push_str("6. КАТЕГОРИЧЕСКИ ЗАПРЕЩЕНО искать ответы в интернете, парсить ГДЗ или делать запросы к поисковикам.\n");
        p.push_str("7. Все учебники — это отсканированные страницы в PDF. Если требуется посмотреть условия номеров, быстро отрендери нужные страницы: `pdftoppm -png -r 150 -f <start> -l <end> <имя_файла>.pdf /tmp/page` и открой через `view_file /tmp/page-<номер>.png`.\n");
        p.push_str("8. КАТЕГОРИЧЕСКИ ЗАПРЕЩЕНО запускать фоновые задачи (background tasks) или завершать ход словами 'ожидаю выполнения команды'. Сразу выдай окончательное решение в этом же ответе.\n");
        p.push_str("9. РИСОВАНИЕ И ЧЕРТЕЖИ ЧЕРЕЗ SVG: Везде, где требуется чертеж (геометрия: треугольники, углы, параллельные прямые), график функций (алгебра: числовая прямая, парабола, прямая) или схема (физика/география) — ОБЯЗАТЕЛЬНО рисуй аккуратный векторный SVG прямо в коде (внутри тегов ```xml <svg ...>...</svg>``` или напрямую `<svg ...>...</svg>`). Используй светлые линии (stroke=\"#ececf2\", stroke-width=\"2\"), четкие подписи вершин и осей (<text fill=\"#9c9cae\">) и viewBox, чтобы он красиво отображался в интерактивном шаблоне.\n");
        p.push_str("10. Таблицы оформляй стандартным синтаксисом Markdown (| колонка 1 | колонка 2 |) прямо в теле ответа.\n");
        p.push_str("11. ЗАПРЕЩЕНО использовать LaTeX ($$, $, \\circ, \\text, \\frac, \\degree). Пиши формулы обычным текстом, а градусы знаком ° или °C.\n");
        p.push_str("12. Не используй смайлики и эмодзи.\n");

        p
    }
}
