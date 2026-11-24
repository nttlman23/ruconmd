//! Помощь при вводе Markdown. Здесь только разбор строк, правит их `App`.

/// Начало строки: отступ, цитаты, маркер списка, чекбокс.
#[derive(Debug, PartialEq, Eq)]
struct Lead<'a> {
    /// Отступ и знаки цитаты.
    outer: &'a str,
    marker: Option<Marker<'a>>,
}

#[derive(Debug, PartialEq, Eq)]
struct Marker<'a> {
    kind: Kind<'a>,
    /// Пробелы после маркера.
    gap: &'a str,
    task: bool,
    len: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum Kind<'a> {
    Bullet(&'a str),
    Number(u64, &'a str),
}

fn lead(line: &str) -> Lead<'_> {
    let mut outer = 0;
    let space = |from: usize| line.len() - line[from..].trim_start_matches([' ', '\t']).len();
    loop {
        let next = space(outer);
        match line[next..].strip_prefix('>') {
            Some(_) => outer = next + 1,
            None => break,
        }
    }
    let start = space(outer);
    // Пробел после `>` относится к цитате, отступ пункта тоже.
    let (head, rest) = line.split_at(start);
    Lead {
        outer: head,
        marker: marker(rest),
    }
}

fn marker(rest: &str) -> Option<Marker<'_>> {
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let end = if digits > 0 { digits + 1 } else { 1 };
    let sign = rest.get(end - 1..end)?;
    let kind = match (digits, sign) {
        (0, "-" | "*" | "+") => Kind::Bullet(sign),
        (1..=9, "." | ")") => Kind::Number(rest[..digits].parse().ok()?, sign),
        _ => return None,
    };
    let after = &rest[end..];
    let gap = &after[..after.len() - after.trim_start_matches([' ', '\t']).len()];
    if gap.is_empty() {
        return None;
    }
    let mut len = end + gap.len();
    let task = ["[ ] ", "[x] ", "[X] "]
        .iter()
        .any(|t| rest[len..].starts_with(t));
    if task {
        len += 4;
    }
    Some(Marker {
        kind,
        gap,
        task,
        len,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Enter {
    /// Перевод строки и этот текст после него.
    Continue(String),
    /// Пустой пункт: оставить от строки столько байт.
    Finish(usize),
}

/// Что делает Enter, нажатый на байте `cursor` строки.
pub fn enter(line: &str, cursor: usize, in_code: bool) -> Enter {
    let indent = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
    let plain = || Enter::Continue(indent[..indent.len().min(cursor)].to_owned());
    if in_code {
        return plain();
    }
    let lead = lead(line);
    let quoted = lead.outer.contains('>');
    let outer = lead.outer.len();
    match &lead.marker {
        Some(m) => {
            let len = outer + m.len;
            if cursor < len {
                return plain();
            }
            if line[len..].trim().is_empty() {
                // Вместе с маркером уходит отступ, знаки цитаты остаются.
                let keep = lead.outer.rfind('>').map_or(0, |i| (i + 2).min(outer));
                return Enter::Finish(keep);
            }
            let sign = match m.kind {
                Kind::Bullet(sign) => sign.to_owned(),
                Kind::Number(n, sign) => format!("{}{sign}", n + 1),
            };
            let task = if m.task { "[ ] " } else { "" };
            Enter::Continue(format!("{}{sign}{}{task}", lead.outer, m.gap))
        }
        None if quoted && cursor >= outer => {
            if line[outer..].trim().is_empty() {
                // Убирается самый внутренний уровень цитаты.
                let keep = line[..lead.outer.rfind('>').unwrap_or(0)].trim_end().len();
                return Enter::Finish(keep);
            }
            Enter::Continue(lead.outer.to_owned())
        }
        None => plain(),
    }
}

/// На сколько пробелов сдвигается пункт списка, чтобы стать вложенным.
pub fn list_step(line: &str) -> Option<usize> {
    let lead = lead(line);
    let m = lead.marker?;
    // Сдвигать пункт внутри цитаты этим способом нельзя.
    if lead.outer.contains('>') {
        return None;
    }
    let task = if m.task { 4 } else { 0 };
    Some(m.len - task)
}

/// Строка со сдвигом вправо.
pub fn indent(line: &str, step: usize) -> String {
    if line.is_empty() {
        return String::new();
    }
    format!("{}{line}", " ".repeat(step))
}

/// Строка со сдвигом влево.
pub fn outdent(line: &str, step: usize) -> String {
    if let Some(rest) = line.strip_prefix('\t') {
        return rest.to_owned();
    }
    let spaces = line.bytes().take_while(|b| *b == b' ').count();
    line[spaces.min(step)..].to_owned()
}

/// Есть ли вокруг выделения оформление длиной `len` звёздочек.
/// Одна или три звёздочки означают курсив, две или три жирный.
pub fn is_wrapped(stars: usize, len: usize) -> bool {
    match len {
        1 => stars == 1 || stars == 3,
        _ => stars >= len,
    }
}

/// Похоже ли выделение на адрес: тогда ссылке не хватает текста.
pub fn is_url(text: &str) -> bool {
    let schemes = ["http://", "https://", "mailto:"];
    schemes.iter().any(|s| text.starts_with(s)) && !text.contains(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_end(line: &str) -> Enter {
        enter(line, line.len(), false)
    }

    fn go(text: &str) -> Enter {
        Enter::Continue(text.into())
    }

    #[test]
    fn enter_continues_lists_and_quotes() {
        assert_eq!(at_end("- пункт"), go("- "));
        assert_eq!(at_end("  * пункт"), go("  * "));
        assert_eq!(at_end("9. пункт"), go("10. "));
        assert_eq!(at_end("3) пункт"), go("4) "));
        assert_eq!(at_end("- [x] дело"), go("- [ ] "));
        assert_eq!(at_end("> цитата"), go("> "));
        assert_eq!(at_end("> > - a"), go("> > - "));
        assert_eq!(at_end(">без пробела"), go(">"));
        assert_eq!(at_end("текст"), go(""));
        assert_eq!(at_end("    отступ"), go("    "));
        assert_eq!(at_end("-не список"), go(""));
        assert_eq!(at_end("**жирный** текст"), go(""));
    }

    #[test]
    fn enter_finishes_empty_items() {
        assert_eq!(at_end("- "), Enter::Finish(0));
        assert_eq!(at_end("  1. "), Enter::Finish(0));
        assert_eq!(at_end("- [ ] "), Enter::Finish(0));
        assert_eq!(at_end("> - "), Enter::Finish(2));
        assert_eq!(at_end("> "), Enter::Finish(0));
        assert_eq!(at_end("> > "), Enter::Finish(1));
    }

    #[test]
    fn enter_inside_the_marker_or_code_is_plain() {
        assert_eq!(enter("- пункт", 0, false), go(""));
        assert_eq!(enter("  - пункт", 1, false), go(" "));
        assert_eq!(enter("- a", 3, true), go(""));
        assert_eq!(enter("    x", 5, true), go("    "));
    }

    #[test]
    fn shifting() {
        assert_eq!(list_step("- a"), Some(2));
        assert_eq!(list_step("  10. [ ] a"), Some(4));
        assert_eq!(list_step("текст"), None);
        assert_eq!(list_step("> - a"), None);
        assert_eq!(indent("a", 2), "  a");
        assert_eq!(indent("", 2), "");
        assert_eq!(outdent("   a", 2), " a");
        assert_eq!(outdent(" a", 4), "a");
        assert_eq!(outdent("\ta", 4), "a");
    }

    #[test]
    fn odd_lines() {
        for line in [
            "",
            ">",
            "1",
            "1.",
            "-",
            "- ",
            "> >",
            "é",
            "12345678901. x",
            "١. x",
        ] {
            for cursor in (0..=line.len()).filter(|i| line.is_char_boundary(*i)) {
                enter(line, cursor, false);
            }
            list_step(line);
        }
    }

    #[test]
    fn stars() {
        assert!(is_wrapped(1, 1) && is_wrapped(3, 1) && !is_wrapped(2, 1));
        assert!(is_wrapped(2, 2) && is_wrapped(3, 2) && !is_wrapped(1, 2));
        assert!(is_url("https://a.ru") && !is_url("https://a.ru b") && !is_url("текст"));
    }
}
