//! Правки с учётом разметки: Enter, отступы, жирный, курсив, ссылка.

use super::App;
use crate::mdline::State;
use crate::typing::{self, Enter};

impl App {
    pub(super) fn enter(&mut self) {
        let cursor = self.buffer.cursor();
        if self.buffer.selection().is_some() {
            return self.buffer.type_text("\n");
        }
        let line = self.buffer.line_of(cursor);
        let start = self.buffer.line_start(line);
        let text = self.buffer.line(line);
        let column = self
            .buffer
            .slice(start..cursor.min(self.buffer.line_end(line)));
        if let Some(changed) = self.buffer.take_changed_line() {
            self.states.invalidate(changed);
        }
        let in_code = self.states.at(&self.buffer, line) != State::Normal;
        match typing::enter(&text, column.len(), in_code) {
            Enter::Continue(lead) => self.buffer.type_text(&format!("\n{lead}")),
            Enter::Finish(keep) => {
                let from = start + text[..keep].chars().count();
                self.buffer.delete_range(from..self.buffer.line_end(line));
            }
        }
    }

    /// Строки, которых касается выделение.
    fn touched_lines(&self) -> std::ops::Range<usize> {
        let cursor = self.buffer.cursor();
        let range = self.buffer.selection().unwrap_or(cursor..cursor);
        let first = self.buffer.line_of(range.start);
        let mut last = self.buffer.line_of(range.end);
        // Выделение до начала строки саму строку не захватывает.
        if last > first && self.buffer.line_start(last) == range.end {
            last -= 1;
        }
        first..last + 1
    }

    pub(super) fn tab(&mut self, back: bool) {
        let tab = self.view.settings.tab_width;
        let lines = self.touched_lines();
        let in_list = typing::list_step(&self.buffer.line(lines.start)).is_some();
        if !back && lines.len() == 1 && !in_list {
            let cursor = self
                .buffer
                .selection()
                .map_or(self.buffer.cursor(), |r| r.start);
            let column = cursor - self.buffer.line_start(lines.start);
            return self.buffer.type_text(&" ".repeat(tab - column % tab));
        }
        self.buffer.edit_lines(lines, |line| {
            let step = typing::list_step(line).unwrap_or(tab);
            if back {
                typing::outdent(line, step)
            } else {
                typing::indent(line, step)
            }
        });
    }

    /// Добавляет или снимает звёздочки вокруг выделения.
    pub(super) fn emphasis(&mut self, len: usize) {
        let mark = "*".repeat(len);
        let cursor = self.buffer.cursor();
        let Some(mut range) = self.buffer.selection() else {
            let stars = self.stars_around(cursor, cursor);
            if stars == len {
                return self.buffer.edit(cursor - len..cursor + len, "", None);
            }
            let place = Some((None, cursor + len));
            return self.buffer.edit(cursor..cursor, &mark.repeat(2), place);
        };
        // Звёздочки рядом с пробелом не работают.
        let text = self.buffer.slice(range.clone());
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        range.start += text.len_chars() - text.trim_start().len_chars();
        range.end = range.start + trimmed.len_chars();

        let outside = self.stars_around(range.start, range.end);
        let inside = inner_stars(trimmed);
        let (start, end) = (range.start, range.end);
        if typing::is_wrapped(outside, len) {
            let place = Some((Some(start - len), end - len));
            self.buffer.edit(start - len..end + len, trimmed, place);
        } else if typing::is_wrapped(inside, len) && trimmed.len_chars() > 2 * len {
            let inner = &trimmed[len..trimmed.len() - len];
            let place = Some((Some(start), end - 2 * len));
            self.buffer.edit(range, inner, place);
        } else {
            let place = Some((Some(start + len), end + len));
            self.buffer
                .edit(range, &format!("{mark}{trimmed}{mark}"), place);
        }
    }

    /// Сколько звёздочек одновременно слева и справа.
    fn stars_around(&self, start: usize, end: usize) -> usize {
        let left = self.buffer.slice(start.saturating_sub(4)..start);
        let right = self.buffer.slice(end..(end + 4).min(self.buffer.len()));
        let left = left.chars().rev().take_while(|c| *c == '*').count();
        left.min(right.chars().take_while(|c| *c == '*').count())
    }

    pub(super) fn insert_link(&mut self) {
        let cursor = self.buffer.cursor();
        let range = self.buffer.selection().unwrap_or(cursor..cursor);
        let text = self.buffer.slice(range.clone());
        let start = range.start;
        let (new, at) = if typing::is_url(&text) {
            (format!("[]({text})"), start + 1)
        } else if text.is_empty() {
            ("[]()".to_owned(), start + 1)
        } else {
            (format!("[{text}]()"), start + text.len_chars() + 3)
        };
        self.buffer.edit(range, &new, Some((None, at)));
    }
}

fn inner_stars(text: &str) -> usize {
    let left = text.chars().take_while(|c| *c == '*').count();
    left.min(text.chars().rev().take_while(|c| *c == '*').count())
}

trait LenChars {
    fn len_chars(&self) -> usize;
}

impl LenChars for str {
    fn len_chars(&self) -> usize {
        self.chars().count()
    }
}
