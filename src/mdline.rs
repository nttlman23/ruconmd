//! Подсветка разметки в редакторе, построчно.
//!
//! Между строками передаётся только состояние блока кода и метаданных, поэтому
//! после правки пересчитываются лишь строки ниже неё до края экрана.
//! Выделение, начатое на одной строке и законченное на другой, не подсвечивается.

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

use crate::buffer::Buffer;
use crate::style::{Mark, Tone};

/// Более длинные строки не подсвечиваются.
const MAX_LINE: usize = 8 * 1024;
/// Столько байт достаточно, чтобы узнать границу блока.
const HEAD: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Normal,
    Fence { mark: u8, len: usize, quoted: bool },
    Metadata,
}

/// Состояния начала строк, вычисленные до некоторой строки.
#[derive(Debug, Default)]
pub struct States {
    known: Vec<State>,
}

impl States {
    /// Строка изменилась: состояния после неё неизвестны.
    pub fn invalidate(&mut self, line: usize) {
        self.known.truncate(line + 1);
    }

    pub fn at(&mut self, buffer: &Buffer, line: usize) -> State {
        if self.known.is_empty() {
            self.known.push(State::Normal);
        }
        if self.known.len() <= line {
            let first = self.known.len() - 1;
            for (index, head) in (first..line).zip(buffer.line_heads(first, HEAD)) {
                self.known.push(next(self.known[index], &head, index));
            }
        }
        self.known[line]
    }
}

fn strip_quotes(line: &str) -> (&str, bool) {
    let mut rest = line.trim_start();
    let mut quoted = false;
    while let Some(inner) = rest.strip_prefix('>') {
        quoted = true;
        rest = inner.trim_start();
    }
    (rest, quoted)
}

/// Символ и длина ограждения в начале строки.
fn fence(text: &str) -> Option<(u8, usize)> {
    let mark = *text.as_bytes().first()?;
    if mark != b'`' && mark != b'~' {
        return None;
    }
    let len = text.bytes().take_while(|b| *b == mark).count();
    (len >= 3).then_some((mark, len))
}

fn opening(line: &str) -> Option<State> {
    let (mut text, quoted) = strip_quotes(line);
    if let Some(len) = list_marker(text) {
        text = text[len..].trim_start();
    }
    let (mark, len) = fence(text)?;
    // Обратные апострофы в подписи означают код в строке, а не блок.
    let inline = mark == b'`' && text[len..].contains('`');
    (!inline).then_some(State::Fence { mark, len, quoted })
}

fn next(state: State, line: &str, index: usize) -> State {
    match state {
        State::Normal if index == 0 && line.trim_end() == "---" => State::Metadata,
        State::Normal => opening(line).unwrap_or(State::Normal),
        State::Metadata if matches!(line.trim_end(), "---" | "...") => State::Normal,
        State::Metadata => State::Metadata,
        State::Fence { mark, len, quoted } => {
            let (text, is_quoted) = strip_quotes(line);
            if quoted && !is_quoted {
                // Цитата кончилась, вместе с ней и блок.
                return next(State::Normal, line, index);
            }
            let closing = fence(text)
                .is_some_and(|(m, l)| m == mark && l >= len && text[l..].trim().is_empty());
            if closing { State::Normal } else { state }
        }
    }
}

/// Длина маркера списка вместе с пробелом.
fn list_marker(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let len = match bytes.first()? {
        b'-' | b'*' | b'+' => 1,
        b'0'..=b'9' => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            if digits > 9 || !matches!(bytes.get(digits), Some(b'.' | b')')) {
                return None;
            }
            digits + 1
        }
        _ => return None,
    };
    match bytes.get(len) {
        Some(b' ' | b'\t') => Some(len + 1),
        None => Some(len),
        _ => None,
    }
}

fn is_rule(text: &str) -> bool {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && ["-", "*", "_"]
            .iter()
            .any(|m| compact.replace(m, "").is_empty())
}

fn is_table_rule(text: &str) -> bool {
    text.contains('|') && text.contains('-') && text.trim().chars().all(|c| "|-: \t".contains(c))
}

/// Оформление каждого байта строки. Пусто: строка без подсветки.
pub fn marks(state: State, line: &str) -> Vec<Mark> {
    if line.len() > MAX_LINE || line.is_empty() {
        return Vec::new();
    }
    let mut out = vec![Mark::TEXT; line.len()];
    match state {
        State::Metadata => paint(&mut out, 0..line.len(), Mark::tone(Tone::Html)),
        State::Fence { quoted, .. } if !quoted || strip_quotes(line).1 => {
            let closing = next(state, line, usize::MAX) == State::Normal;
            let tone = if closing { Tone::Marker } else { Tone::Code };
            paint(&mut out, 0..line.len(), Mark::tone(tone));
        }
        _ if opening(line).is_some() => paint(&mut out, 0..line.len(), Mark::tone(Tone::Marker)),
        _ => paint_text(&mut out, line),
    }
    out
}

fn paint(out: &mut [Mark], range: Range<usize>, mark: Mark) {
    for m in &mut out[range] {
        *m = m.with(mark);
    }
}

fn paint_text(out: &mut [Mark], line: &str) {
    let end = line.len();
    let rest_at = |rest: &str| end - rest.len();

    let mut rest = line.trim_start();
    while let Some(inner) = rest.strip_prefix('>') {
        paint(out, rest_at(rest)..rest_at(inner), Mark::tone(Tone::Quote));
        rest = inner.trim_start();
    }
    if is_rule(rest) || is_table_rule(rest) {
        return paint(out, rest_at(rest)..end, Mark::tone(Tone::Marker));
    }
    while let Some(len) = list_marker(rest) {
        paint(
            out,
            rest_at(rest)..rest_at(rest) + len,
            Mark::tone(Tone::Marker),
        );
        rest = rest[len..].trim_start();
    }
    for task in ["[ ] ", "[x] ", "[X] "] {
        if let Some(inner) = rest.strip_prefix(task) {
            paint(out, rest_at(rest)..rest_at(inner), Mark::tone(Tone::Marker));
            rest = inner;
        }
    }
    let hashes = rest.bytes().take_while(|b| *b == b'#').count();
    let after = rest.as_bytes().get(hashes);
    if (1..=6).contains(&hashes) && matches!(after, None | Some(b' ' | b'\t')) {
        paint(out, rest_at(rest)..end, Mark::tone(Tone::Heading).bold());
        rest = rest[hashes..].trim_start();
    }
    paint_inline(out, rest_at(rest), rest);
}

fn paint_inline(out: &mut [Mark], base: usize, text: &str) {
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_MATH;
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        let mark = match event {
            Event::Start(Tag::Strong) => Mark::TEXT.bold(),
            Event::Start(Tag::Emphasis) => Mark::TEXT.italic(),
            Event::Start(Tag::Strikethrough) => Mark::TEXT.strike(),
            Event::Start(Tag::Link { .. } | Tag::Image { .. }) => Mark::tone(Tone::Link),
            Event::Code(_) | Event::InlineMath(_) | Event::DisplayMath(_) => Mark::tone(Tone::Code),
            Event::InlineHtml(_) | Event::Html(_) => Mark::tone(Tone::Html),
            _ => continue,
        };
        paint(out, base + range.start..base + range.end, mark);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: &str, word: &str) -> Mark {
        marks(State::Normal, line)[line.find(word).unwrap()]
    }

    fn states(text: &str) -> Vec<State> {
        let buffer = Buffer::new(text);
        let mut s = States::default();
        (0..buffer.line_count()).map(|i| s.at(&buffer, i)).collect()
    }

    fn code_lines(text: &str) -> Vec<usize> {
        let is_code = |s: &State| *s != State::Normal;
        let all = states(text);
        (0..all.len()).filter(|i| is_code(&all[*i])).collect()
    }

    #[test]
    fn line_markup() {
        assert_eq!(at("# Заголовок", "Заг").tone, Tone::Heading);
        assert_eq!(at("#тег", "тег").tone, Tone::Text);
        assert_eq!(at("> цитата", ">").tone, Tone::Quote);
        assert_eq!(at("> - [x] дело", "-").tone, Tone::Marker);
        assert_eq!(at("> - [x] дело", "[x]").tone, Tone::Marker);
        assert_eq!(at("> - [x] дело", "дело").tone, Tone::Text);
        assert_eq!(at("12. пункт", "12.").tone, Tone::Marker);
        assert_eq!(at("- - -", "-").tone, Tone::Marker);
        assert_eq!(at("|---|:-:|", "|").tone, Tone::Marker);
        assert_eq!(at("-5 градусов", "-5").tone, Tone::Text);
    }

    #[test]
    fn inline_markup() {
        let line = "текст **жирный *оба*** `код` [имя](url) ~~нет~~ <b> \\*a\\*";
        assert!(at(line, "жирный").bold && !at(line, "жирный").italic);
        assert!(at(line, "оба").bold && at(line, "оба").italic);
        assert_eq!(at(line, "код").tone, Tone::Code);
        assert_eq!(at(line, "имя").tone, Tone::Link);
        assert!(at(line, "нет").strike);
        assert_eq!(at(line, "<b>").tone, Tone::Html);
        assert_eq!(at(line, "a\\*"), Mark::TEXT);
        assert!(at("## Глава **x**", "x").bold);
        assert_eq!(at("- пункт `код`", "код").tone, Tone::Code);
    }

    #[test]
    fn fences() {
        assert_eq!(code_lines("a\n```rust\nx\n```\nb\n"), [2, 3]);
        assert_eq!(code_lines("~~~\n```\n~~~~\nb"), [1, 2]);
        assert_eq!(code_lines("````\n```\n````\nb"), [1, 2]);
        assert_eq!(code_lines("- ```\n  x\n  ```\nb"), [1, 2]);
        assert_eq!(code_lines("> ```\n> x\nb"), [1, 2]);
        assert_eq!(code_lines("```a``` b\nc"), [0usize; 0]);
        assert_eq!(code_lines("---\ntitle: x\n---\nb"), [1, 2]);
    }

    #[test]
    fn states_follow_edits() {
        let mut buffer = Buffer::new("a\n```\nx\n```\nb");
        let mut s = States::default();
        assert_eq!(s.at(&buffer, 4), State::Normal);
        buffer.take_changed_line();
        buffer.move_to(2, false);
        buffer.delete_word();
        s.invalidate(buffer.take_changed_line().unwrap());
        assert_ne!(s.at(&buffer, buffer.line_count() - 1), State::Normal);
        buffer.undo();
        s.invalidate(buffer.take_changed_line().unwrap());
        assert_eq!(s.at(&buffer, 4), State::Normal);
    }

    #[test]
    fn odd_lines_do_not_panic() {
        for line in [
            "",
            ">",
            ">>> ",
            "-",
            "1.",
            "[ ",
            "######",
            "# ",
            "`",
            "**",
            "日本 **語",
            "\u{7}",
        ] {
            for state in [State::Normal, State::Metadata] {
                assert_eq!(marks(state, line).len(), line.len());
            }
        }
        assert!(marks(State::Normal, &"x".repeat(MAX_LINE + 1)).is_empty());
    }
}
