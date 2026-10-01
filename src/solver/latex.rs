/// Очистка текста от LaTeX-синтаксиса ($$, $, \text, \circ, \frac, {, })
/// для красивого и чистого отображения в Telegram.
pub fn clean_latex(input: &str) -> String {
    let mut s = input.to_string();

    // 1. Удаляем блочные и строчные математические разделители: $$, $, \(, \), \[, \]
    s = s.replace("$$", "");
    s = s.replace("\\[", "");
    s = s.replace("\\]", "");
    s = s.replace("\\(", "");
    s = s.replace("\\)", "");

    // 2. Специфические составные шаблоны (температура, градусы, единицы измерения)
    s = s.replace("^{\\circ}\\text{C}", "°C");
    s = s.replace("^{\\circ}\\text{ C}", "°C");
    s = s.replace("^{\\circ} \\text{C}", "°C");
    s = s.replace("^{\\circ} \\text{ C}", "°C");
    s = s.replace("^\\circ\\text{C}", "°C");
    s = s.replace("^\\circ\\text{ C}", "°C");
    s = s.replace("^\\circ \\text{C}", "°C");
    s = s.replace("^\\circ \\text{ C}", "°C");
    s = s.replace("^{\\circ}C", "°C");
    s = s.replace("^{\\circ} C", "°C");
    s = s.replace("^\\circ C", "°C");
    s = s.replace("^\\circ", "°");
    s = s.replace("^{\\circ}", "°");
    s = s.replace("\\circ", "°");
    s = s.replace("\\degree", "°");
    s = s.replace("\\deg", "°");

    // LaTeX пробелы
    s = s.replace("\\quad", " ");
    s = s.replace("\\qquad", "  ");
    s = s.replace("\\,", " ");
    s = s.replace("\\;", " ");
    s = s.replace("\\:", " ");
    s = s.replace("\\!", "");

    // 3. Десятичные запятые в LaTeX: {,} -> ,
    s = s.replace("{,}", ",");
    s = s.replace("{, }", ", ");

    // 4. Дроби: \frac{числитель}{знаменатель} -> числитель / знаменатель
    while let Some(pos) = s.find("\\frac") {
        if let Some((num, den, full_len)) = parse_two_braced_args(&s[pos + 5..]) {
            let repl = format!("{} / {}", num.trim(), den.trim());
            s.replace_range(pos..pos + 5 + full_len, &repl);
        } else {
            s = s.replacen("\\frac", "", 1);
        }
    }

    // 5. Квадратные корни: \sqrt{выражение} -> √(выражение)
    while let Some(pos) = s.find("\\sqrt") {
        if let Some((arg, full_len)) = parse_one_braced_arg(&s[pos + 5..]) {
            let repl = format!("√({})", arg.trim());
            s.replace_range(pos..pos + 5 + full_len, &repl);
        } else {
            s = s.replacen("\\sqrt", "√", 1);
        }
    }

    // 6. Текстовые обертки: \text{...}, \mathrm{...}, \mathbf{...}, \textbf{...}, \textit{...}
    for tag in &[
        "\\text",
        "\\mathrm",
        "\\mathbf",
        "\\textbf",
        "\\textit",
        "\\underline",
        "\\displaystyle",
    ] {
        while let Some(pos) = s.find(tag) {
            let tag_len = tag.len();
            if let Some((arg, full_len)) = parse_one_braced_arg(&s[pos + tag_len..]) {
                let repl = arg.clone();
                s.replace_range(pos..pos + tag_len + full_len, &repl);
            } else {
                s = s.replacen(tag, "", 1);
            }
        }
    }

    // 7. Математические операторы и спецсимволы
    s = s.replace("\\cdot", "·");
    s = s.replace("\\times", "×");
    s = s.replace("\\div", ":");
    s = s.replace("\\approx", "≈");
    s = s.replace("\\pm", "±");
    s = s.replace("\\mp", "∓");
    s = s.replace("\\le", "≤");
    s = s.replace("\\leq", "≤");
    s = s.replace("\\ge", "≥");
    s = s.replace("\\geq", "≥");
    s = s.replace("\\ne", "≠");
    s = s.replace("\\neq", "≠");
    s = s.replace("\\equiv", "≡");
    s = s.replace("\\infty", "∞");
    s = s.replace("\\to", "→");
    s = s.replace("\\rightarrow", "→");
    s = s.replace("\\leftarrow", "←");
    s = s.replace("\\dots", "...");
    s = s.replace("\\ldots", "...");

    // 8. Греческие буквы (физика, математика)
    s = s.replace("\\rho", "ρ");
    s = s.replace("\\lambda", "λ");
    s = s.replace("\\alpha", "α");
    s = s.replace("\\beta", "β");
    s = s.replace("\\gamma", "γ");
    s = s.replace("\\Delta", "Δ");
    s = s.replace("\\delta", "δ");
    s = s.replace("\\mu", "μ");
    s = s.replace("\\nu", "ν");
    s = s.replace("\\omega", "ω");
    s = s.replace("\\Omega", "Ω");
    s = s.replace("\\tau", "τ");
    s = s.replace("\\pi", "π");
    s = s.replace("\\phi", "φ");
    s = s.replace("\\theta", "θ");
    s = s.replace("\\sigma", "σ");
    s = s.replace("\\Sigma", "Σ");

    // 9. Нижние индексы: _{...} -> _...
    while let Some(pos) = s.find("_{") {
        if let Some((arg, full_len)) = parse_one_braced_arg(&s[pos + 1..]) {
            let repl = format!("_{}", arg);
            s.replace_range(pos..pos + 1 + full_len, &repl);
        } else {
            break;
        }
    }

    // 10. Верхние индексы: ^{...} -> Юникод степени или ^...
    while let Some(pos) = s.find("^{") {
        if let Some((arg, full_len)) = parse_one_braced_arg(&s[pos + 1..]) {
            let unicode_pow = match arg.trim() {
                "0" => "⁰",
                "1" => "¹",
                "2" => "²",
                "3" => "³",
                "4" => "⁴",
                "5" => "⁵",
                "6" => "⁶",
                "7" => "⁷",
                "8" => "⁸",
                "9" => "⁹",
                "-1" => "⁻¹",
                "-2" => "⁻²",
                "-3" => "⁻³",
                _ => "",
            };
            let repl = if !unicode_pow.is_empty() {
                unicode_pow.to_string()
            } else {
                format!("^{}", arg)
            };
            s.replace_range(pos..pos + 1 + full_len, &repl);
        } else {
            break;
        }
    }

    // 11. Удаляем оставшиеся знаки '$' (инлайн-математика)
    s = s.replace('$', "");

    // 12. Удаляем оставшиеся фигурные скобки { и }, если они были частью синтаксиса
    let mut clean_chars = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            if chars.peek() == Some(&'}') {
                chars.next();
            }
        } else if c == '}' {
            // пропускаем закрывающую фигурную скобку
        } else {
            clean_chars.push(c);
        }
    }

    clean_chars
}

fn parse_one_braced_arg(s: &str) -> Option<(String, usize)> {
    let s_trimmed = s.trim_start();
    let lead_spaces = s.len() - s_trimmed.len();
    if !s_trimmed.starts_with('{') {
        return None;
    }
    let mut depth = 0;
    let mut end_idx = 0;
    for (i, c) in s_trimmed.char_indices() {
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                end_idx = i;
                break;
            }
        }
    }
    if depth == 0 && end_idx > 0 {
        let inside = s_trimmed[1..end_idx].to_string();
        Some((inside, lead_spaces + end_idx + 1))
    } else {
        None
    }
}

fn parse_two_braced_args(s: &str) -> Option<(String, String, usize)> {
    let (first, len1) = parse_one_braced_arg(s)?;
    let (second, len2) = parse_one_braced_arg(&s[len1..])?;
    Some((first, second, len1 + len2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_latex() {
        let raw = "Самый тёплый месяц: май ($+30{,}2^\\circ\\text{C}$, по школьному атласу: $+30^\\circ\\text{C}$).";
        let cleaned = clean_latex(raw);
        assert_eq!(
            cleaned,
            "Самый тёплый месяц: май (+30,2°C, по школьному атласу: +30°C)."
        );

        let formula = "$$A = T_{max} - T_{min}$$";
        assert_eq!(clean_latex(formula), "A = T_max - T_min");

        let frac = "\\rho = \\frac{m}{V}";
        assert_eq!(clean_latex(frac), "ρ = m / V");
    }
}
