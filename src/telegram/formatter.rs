/// Модуль форматирования текста для Telegram с поддержкой жирного шрифта (HTML parse_mode).
/// Конвертирует заголовки (#, ##, ###), жирный текст (**текст**), инлайн-код (`код`) и блоки кода.

pub fn markdown_to_telegram_html(input: &str) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = input.lines().collect();
    let mut in_code_block = false;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // Блоки кода ``` ... ```
        if trimmed.starts_with("```") {
            if !in_code_block {
                in_code_block = true;
                out.push_str("<pre>");
            } else {
                in_code_block = false;
                out.push_str("</pre>");
                if i + 1 < lines.len() {
                    out.push('\n');
                }
            }
            continue;
        }

        if in_code_block {
            let escaped = escape_html(line);
            out.push_str(&escaped);
            if i + 1 < lines.len() {
                out.push('\n');
            }
            continue;
        }

        // Заголовки: # ..., ## ..., ### ..., #### ...
        if trimmed.starts_with('#') {
            let hash_count = trimmed.chars().take_while(|&c| c == '#').count();
            if hash_count > 0 && trimmed[hash_count..].starts_with(' ') {
                let header_text = trimmed[hash_count..].trim();
                let escaped = escape_html(header_text);
                let formatted = format_inlines(&escaped);
                out.push_str(&format!("<b>{}</b>", formatted));
                if i + 1 < lines.len() {
                    out.push('\n');
                }
                continue;
            }
        }

        let escaped = escape_html(line);
        let formatted = format_inlines(&escaped);
        out.push_str(&formatted);
        if i + 1 < lines.len() {
            out.push('\n');
        }
    }

    if in_code_block {
        out.push_str("</pre>");
    }

    out
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn format_inlines(s: &str) -> String {
    let mut res = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '`' {
            let mut code = String::new();
            let mut closed = false;
            while let Some(nc) = chars.next() {
                if nc == '`' {
                    closed = true;
                    break;
                }
                code.push(nc);
            }
            if closed {
                res.push_str("<code>");
                res.push_str(&code);
                res.push_str("</code>");
            } else {
                res.push('`');
                res.push_str(&code);
            }
        } else if c == '*' && chars.peek() == Some(&'*') {
            chars.next(); // пропускаем второй '*'
            let mut bold = String::new();
            let mut closed = false;
            while let Some(nc) = chars.next() {
                if nc == '*' && chars.peek() == Some(&'*') {
                    chars.next(); // пропускаем второй '*'
                    closed = true;
                    break;
                }
                bold.push(nc);
            }
            if closed {
                res.push_str("<b>");
                res.push_str(&bold);
                res.push_str("</b>");
            } else {
                res.push_str("**");
                res.push_str(&bold);
            }
        } else {
            res.push(c);
        }
    }

    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_to_telegram_html() {
        let md = "### Заголовок урока\n**Дано:** масса 5 кг\nФормула: `F = m * g`\nНекоторый <текст> & знаки";
        let html = markdown_to_telegram_html(md);
        assert_eq!(
            html,
            "<b>Заголовок урока</b>\n<b>Дано:</b> масса 5 кг\nФормула: <code>F = m * g</code>\nНекоторый &lt;текст&gt; &amp; знаки"
        );
    }
}
