//! Текстовый буфер: правки, курсор, выделение, отмена.
//!
//! Позиции измеряются в символах (`char`) от начала документа. Переводы строк
//! хранятся как в файле, поэтому нетронутый текст сохраняется побайтно.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    Lf,
    Crlf,
}

impl Eol {
    pub const PLATFORM: Eol = if cfg!(windows) { Eol::Crlf } else { Eol::Lf };

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }
}

/// Переводы строк документа: чем разделять новые строки и все ли одинаковы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineEnds {
    pub eol: Eol,
    pub mixed: bool,
}

impl LineEnds {
    pub fn detect(text: &str) -> Self {
        let total = text.bytes().filter(|b| *b == b'\n').count();
        let crlf = text.matches("\r\n").count();
        let lf = total - crlf;
        let eol = match () {
            _ if total == 0 => Eol::PLATFORM,
            _ if crlf > lf => Eol::Crlf,
            _ => Eol::Lf,
        };
        Self {
            eol,
            mixed: crlf > 0 && lf > 0,
        }
    }
}

/// Вид правки, от него зависит объединение в один шаг отмены.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Type,
    Backspace,
    Delete,
    Paste,
    Other,
}

#[derive(Debug)]
struct Edit {
    at: usize,
    removed: String,
    inserted: String,
}

#[derive(Debug, Clone, Copy)]
struct Selection {
    cursor: usize,
    anchor: Option<usize>,
}

/// Один шаг отмены.
#[derive(Debug)]
struct Step {
    id: u64,
    kind: Kind,
    edit: Edit,
    before: Selection,
    after: Selection,
    ends: Option<(LineEnds, LineEnds)>,
}

pub struct Buffer {
    rope: Rope,
    revision: u64,
    /// Верхняя строка, изменённая с прошлого вопроса.
    changed_line: Option<usize>,
    cursor: usize,
    anchor: Option<usize>,
    pub ends: LineEnds,
    undo: Vec<Step>,
    redo: Vec<Step>,
    next_id: u64,
    saved_id: u64,
    /// Следующая правка начинает новый шаг.
    sealed: bool,
}

fn next_revision() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn head(text: &str, limit: usize) -> &str {
    let text = match text.strip_suffix('\n') {
        Some(line) => line.strip_suffix('\r').unwrap_or(line),
        None => text,
    };
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            revision: next_revision(),
            changed_line: Some(0),
            cursor: 0,
            anchor: None,
            ends: LineEnds::detect(text),
            undo: Vec::new(),
            redo: Vec::new(),
            next_id: 1,
            saved_id: 0,
            sealed: true,
        }
    }

    // --- Чтение ---

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn len(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Меняется при каждом изменении текста, у разных буферов не совпадает.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn changed(&mut self, at: usize) {
        self.revision = next_revision();
        let line = self.rope.char_to_line(at);
        self.changed_line = Some(self.changed_line.map_or(line, |l| l.min(line)));
    }

    pub fn take_changed_line(&mut self) -> Option<usize> {
        self.changed_line.take()
    }

    /// Дешёвая копия текста для другого потока.
    pub fn rope(&self) -> Rope {
        self.rope.clone()
    }

    pub fn byte_len(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn byte_of(&self, pos: usize) -> usize {
        self.rope.char_to_byte(pos)
    }

    /// Позиция символа, содержащего байт.
    pub fn pos_of_byte(&self, byte: usize) -> usize {
        self.rope.byte_to_char(byte.min(self.byte_len()))
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_char(line.min(self.line_count()))
    }

    /// Конец строки без перевода строки.
    pub fn line_end(&self, line: usize) -> usize {
        let start = self.line_start(line);
        let mut end = self.line_start(line + 1);
        if end > start && self.rope.char(end - 1) == '\n' {
            end -= 1;
            if end > start && self.rope.char(end - 1) == '\r' {
                end -= 1;
            }
        }
        end
    }

    /// Текст строки без перевода строки.
    pub fn line(&self, line: usize) -> String {
        self.slice(self.line_start(line)..self.line_end(line))
    }

    /// Начала строк подряд, каждое не длиннее `limit` байт, без перевода строки.
    pub fn line_heads(&self, from: usize, limit: usize) -> impl Iterator<Item = Cow<'_, str>> {
        self.rope
            .lines_at(from)
            .map(move |line| match line.as_str() {
                Some(text) => Cow::Borrowed(head(text, limit)),
                // Строка лежит в нескольких кусках дерева.
                None => {
                    let mut text = String::new();
                    for chunk in line.chunks() {
                        text.push_str(chunk);
                        if text.len() >= limit {
                            break;
                        }
                    }
                    Cow::Owned(head(&text, limit).to_owned())
                }
            })
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        self.rope.slice(range).to_string()
    }

    pub fn line_of(&self, pos: usize) -> usize {
        self.rope.char_to_line(pos)
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Выделенный диапазон, если он не пуст.
    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub fn selected_text(&self) -> Option<String> {
        self.selection().map(|r| self.slice(r))
    }

    pub fn is_modified(&self) -> bool {
        self.top_id() != self.saved_id
    }

    /// Запоминает текущее состояние как сохранённое.
    pub fn mark_saved(&mut self) {
        self.saved_id = self.top_id();
        self.sealed = true;
    }

    fn top_id(&self) -> u64 {
        self.undo.last().map_or(0, |s| s.id)
    }

    // --- Курсор ---

    /// Ставит курсор. `select`: продолжить выделение от прежнего места.
    pub fn move_to(&mut self, pos: usize, select: bool) {
        if !select {
            self.anchor = None;
        } else if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        self.cursor = pos.min(self.len());
        self.sealed = true;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.len();
        self.sealed = true;
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    pub fn next_grapheme(&self, pos: usize) -> usize {
        const LOOK: usize = 64;
        let end = (pos + LOOK).min(self.len());
        let text = self.slice(pos..end);
        pos + text.graphemes(true).next().map_or(0, char_count)
    }

    pub fn prev_grapheme(&self, pos: usize) -> usize {
        const LOOK: usize = 256;
        let text = self.slice(pos.saturating_sub(LOOK)..pos);
        pos - text.graphemes(true).next_back().map_or(0, char_count)
    }

    pub fn next_word(&self, pos: usize) -> usize {
        let mut chars = self.rope.chars_at(pos).peekable();
        let Some(first) = chars.peek().map(|c| class(*c)) else {
            return pos;
        };
        let mut end = pos;
        while chars
            .next_if(|c| first != Class::Space && class(*c) == first)
            .is_some()
        {
            end += 1;
        }
        while chars.next_if(|c| class(*c) == Class::Space).is_some() {
            end += 1;
        }
        end
    }

    pub fn prev_word(&self, pos: usize) -> usize {
        let mut chars = self.rope.chars_at(pos).reversed().peekable();
        let mut start = pos;
        while chars.next_if(|c| class(*c) == Class::Space).is_some() {
            start -= 1;
        }
        let Some(kind) = chars.peek().map(|c| class(*c)) else {
            return start;
        };
        while chars.next_if(|c| class(*c) == kind).is_some() {
            start -= 1;
        }
        start
    }

    // --- Правки ---

    /// Ввод с клавиатуры.
    pub fn type_text(&mut self, text: &str) {
        let kind = if text.contains('\n') {
            Kind::Other
        } else {
            Kind::Type
        };
        self.insert(text, kind, false);
    }

    /// Вставка. `continued`: продолжение предыдущей, отменяется вместе с ней.
    pub fn paste(&mut self, text: &str, continued: bool) {
        self.insert(text, Kind::Paste, continued);
    }

    fn with_document_eol(&self, text: &str) -> String {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        match self.ends.eol {
            Eol::Lf => text,
            Eol::Crlf => text.replace('\n', "\r\n"),
        }
    }

    fn insert(&mut self, text: &str, kind: Kind, continued: bool) {
        let text = self.with_document_eol(text);
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        self.replace(range, text, kind, continued);
    }

    /// Удаляет выделение, а без него диапазон до `to`.
    fn delete_to(&mut self, to: usize, kind: Kind) {
        if let Some(range) = self.selection() {
            self.replace(range, String::new(), Kind::Other, false);
        } else if to != self.cursor {
            let range = to.min(self.cursor)..to.max(self.cursor);
            self.replace(range, String::new(), kind, false);
        }
    }

    pub fn backspace(&mut self) {
        self.delete_to(self.prev_grapheme(self.cursor), Kind::Backspace);
    }

    pub fn delete(&mut self) {
        self.delete_to(self.next_grapheme(self.cursor), Kind::Delete);
    }

    pub fn delete_word_back(&mut self) {
        self.delete_to(self.prev_word(self.cursor), Kind::Other);
    }

    pub fn delete_word(&mut self) {
        self.delete_to(self.next_word(self.cursor), Kind::Other);
    }

    pub fn delete_range(&mut self, range: Range<usize>) {
        if !range.is_empty() {
            self.replace(range, String::new(), Kind::Other, false);
        }
    }

    /// Заменяет диапазон одним шагом отмены. Текст вставляется как есть: в нём
    /// бывают куски документа, переводы строк которых менять нельзя. Место курсора
    /// и начала выделения задаётся в новом тексте, без него курсор встаёт за
    /// вставленным.
    pub fn edit(&mut self, range: Range<usize>, text: &str, place: Option<(Option<usize>, usize)>) {
        self.replace(range, text.to_owned(), Kind::Other, false);
        if let Some((anchor, cursor)) = place {
            self.anchor = anchor.map(|a| a.min(self.len()));
            self.cursor = cursor.min(self.len());
            let after = self.state();
            self.undo.last_mut().expect("replace added a step").after = after;
        }
        self.sealed = true;
    }

    /// Меняет строки одним шагом. Курсор и выделение сдвигаются вместе с текстом.
    pub fn edit_lines(&mut self, lines: Range<usize>, change: impl Fn(&str) -> String) {
        let first = self.line_start(lines.start);
        let last = self.line_end(lines.end - 1);
        let mut out = String::new();
        // Начало строки, её новое начало и сдвиг текста в ней.
        let mut shifts = Vec::new();
        let mut written = 0;
        for line in lines {
            let start = self.line_start(line);
            let old = self.line(line);
            let new = change(&old);
            let shift = char_count(&new) as isize - char_count(&old) as isize;
            shifts.push((start, first + written, shift));
            let end = self.line_end(line);
            let eol = if end < last {
                self.slice(end..self.line_start(line + 1))
            } else {
                String::new()
            };
            written += char_count(&new) + char_count(&eol);
            out.push_str(&new);
            out.push_str(&eol);
        }
        if out == self.slice(first..last) {
            return;
        }
        let grown = written as isize - (last - first) as isize;
        let moved = |pos: usize| match shifts.iter().rev().find(|(start, ..)| *start <= pos) {
            None => pos,
            Some(_) if pos > last => pos.saturating_add_signed(grown),
            Some((start, new, shift)) => new + (pos - start).saturating_add_signed(*shift),
        };
        let place = (self.anchor.map(moved), moved(self.cursor));
        // Переводы строк взяты из документа, приводить их не надо.
        self.replace(first..last, out, Kind::Other, false);
        (self.anchor, self.cursor) = place;
        let after = self.state();
        self.undo.last_mut().expect("replace added a step").after = after;
        self.sealed = true;
    }

    /// Приводит все переводы строк к одному виду, отменяется одним шагом.
    pub fn convert_eol(&mut self, eol: Eol) {
        let old = self.text();
        let unix = old.replace("\r\n", "\n");
        let new = match eol {
            Eol::Lf => unix,
            Eol::Crlf => unix.replace('\n', "\r\n"),
        };
        let ends = LineEnds { eol, mixed: false };
        let previous = self.ends;
        if new == old {
            self.ends = ends;
            return;
        }
        self.replace_all(&new);
        self.ends = ends;
        self.undo.last_mut().expect("replace added a step").ends = Some((previous, ends));
    }

    /// Заменяет весь текст как есть, одним шагом отмены. Курсор остаётся на месте.
    pub fn replace_all(&mut self, text: &str) {
        if *text == self.rope {
            return;
        }
        let line = self.line_of(self.cursor);
        let column = self.cursor.min(self.line_end(line)) - self.line_start(line);
        let previous = self.ends;
        self.replace(0..self.len(), text.to_owned(), Kind::Other, false);
        self.ends = LineEnds::detect(text);
        let line = line.min(self.line_count() - 1);
        self.cursor = (self.line_start(line) + column).min(self.line_end(line));
        self.sealed = true;
        let after = self.state();
        let ends = self.ends;
        let step = self.undo.last_mut().expect("replace added a step");
        step.after = after;
        step.ends = Some((previous, ends));
    }

    fn state(&self) -> Selection {
        Selection {
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    fn restore(&mut self, state: Selection) {
        self.cursor = state.cursor;
        self.anchor = state.anchor;
    }

    fn replace(&mut self, range: Range<usize>, inserted: String, kind: Kind, continued: bool) {
        let before = self.state();
        let edit = Edit {
            at: range.start,
            removed: self.slice(range.clone()),
            inserted,
        };
        self.rope.remove(range);
        self.rope.insert(edit.at, &edit.inserted);
        self.changed(edit.at);
        self.cursor = edit.at + char_count(&edit.inserted);
        self.anchor = None;
        self.redo.clear();

        let after = self.state();
        if let Some(edit) = self.merge(edit, kind, continued) {
            let id = self.next_id;
            self.next_id += 1;
            self.undo.push(Step {
                id,
                kind,
                edit,
                before,
                after,
                ends: None,
            });
        } else {
            self.undo.last_mut().expect("merged into a step").after = after;
        }
        self.sealed = false;
    }

    /// Объединяет правку с последним шагом или возвращает её обратно.
    fn merge(&mut self, edit: Edit, kind: Kind, continued: bool) -> Option<Edit> {
        if self.sealed || (kind == Kind::Paste && !continued) {
            return Some(edit);
        }
        let Some(last) = self.undo.last_mut().filter(|s| s.kind == kind) else {
            return Some(edit);
        };
        let prev = &mut last.edit;
        match kind {
            Kind::Type | Kind::Paste if edit.removed.is_empty() => {
                if prev.at + char_count(&prev.inserted) != edit.at {
                    return Some(edit);
                }
                // Набор делится на шаги по словам.
                let ends_word = prev.inserted.ends_with(char::is_whitespace);
                let starts_word = !edit.inserted.starts_with(char::is_whitespace);
                if kind == Kind::Type && ends_word && starts_word {
                    return Some(edit);
                }
                prev.inserted.push_str(&edit.inserted);
            }
            Kind::Backspace if edit.at + char_count(&edit.removed) == prev.at => {
                prev.at = edit.at;
                prev.removed.insert_str(0, &edit.removed);
            }
            Kind::Delete if edit.at == prev.at => prev.removed.push_str(&edit.removed),
            _ => return Some(edit),
        }
        None
    }

    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else {
            return false;
        };
        let Edit {
            at,
            removed,
            inserted,
        } = &step.edit;
        self.rope.remove(*at..*at + char_count(inserted));
        self.rope.insert(*at, removed);
        self.changed(step.edit.at);
        self.restore(step.before);
        if let Some((before, _)) = step.ends {
            self.ends = before;
        }
        self.redo.push(step);
        self.sealed = true;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else {
            return false;
        };
        let Edit {
            at,
            removed,
            inserted,
        } = &step.edit;
        self.rope.remove(*at..*at + char_count(removed));
        self.rope.insert(*at, inserted);
        self.changed(step.edit.at);
        self.restore(step.after);
        if let Some((_, after)) = step.ends {
            self.ends = after;
        }
        self.undo.push(step);
        self.sealed = true;
        true
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Other,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' || c.width() == Some(0) {
        // Нулевая ширина: комбинируемые знаки, от буквы их не отрываем.
        Class::Word
    } else {
        Class::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(b: &mut Buffer, text: &str) {
        for c in text.chars() {
            b.type_text(&c.to_string());
        }
    }

    #[test]
    fn typing_is_undone_by_words() {
        let mut b = Buffer::new("");
        typed(&mut b, "один два");
        b.undo();
        assert_eq!(b.text(), "один ");
        b.undo();
        assert_eq!(b.text(), "");
        assert!(!b.undo());
        b.redo();
        b.redo();
        assert_eq!(b.text(), "один два");
        assert_eq!(b.cursor(), 8);
        assert!(!b.redo());
    }

    #[test]
    fn moving_the_cursor_starts_a_new_step() {
        let mut b = Buffer::new("");
        typed(&mut b, "ab");
        b.move_to(1, false);
        b.move_to(2, false);
        typed(&mut b, "cd");
        b.undo();
        assert_eq!(b.text(), "ab");
    }

    #[test]
    fn backspace_and_delete_runs_are_single_steps() {
        let mut b = Buffer::new("привет мир");
        b.move_to(6, false);
        b.backspace();
        b.backspace();
        assert_eq!(b.text(), "прив мир");
        b.undo();
        assert_eq!((b.text().as_str(), b.cursor()), ("привет мир", 6));
        b.move_to(0, false);
        b.delete();
        b.delete();
        b.undo();
        assert_eq!(b.text(), "привет мир");
    }

    #[test]
    fn paste_replaces_selection_and_continues_as_one_step() {
        let mut b = Buffer::new("abc");
        b.ends.eol = Eol::Lf;
        b.move_to(1, false);
        b.move_to(2, true);
        b.paste("X\r\nY", false);
        b.paste("Z", true);
        assert_eq!(b.text(), "aX\nYZc");
        b.undo();
        assert_eq!(b.text(), "aXc".replace('X', "b"));
        assert_eq!(b.selection(), Some(1..2));
    }

    #[test]
    fn separate_pastes_are_separate_steps() {
        let mut b = Buffer::new("");
        b.paste("a", false);
        b.paste("b", false);
        b.undo();
        assert_eq!(b.text(), "a");
    }

    #[test]
    fn modified_flag_follows_undo_and_branches() {
        let mut b = Buffer::new("");
        assert!(!b.is_modified());
        typed(&mut b, "a");
        b.mark_saved();
        typed(&mut b, "b");
        assert!(
            b.is_modified(),
            "typing after save must not merge into saved step"
        );
        b.undo();
        assert!(!b.is_modified());
        b.undo();
        assert!(b.is_modified());
        typed(&mut b, "a");
        assert!(b.is_modified(), "a new branch is not the saved state");
    }

    #[test]
    fn graphemes_move_and_delete_whole() {
        let mut b = Buffer::new("e\u{301}👨‍👩‍👧x");
        let first = b.next_grapheme(0);
        assert_eq!(first, 2);
        let second = b.next_grapheme(first);
        assert_eq!(b.prev_grapheme(second), first);
        b.move_to(second, false);
        b.backspace();
        assert_eq!(b.text(), "e\u{301}x");
    }

    #[test]
    fn crlf_is_one_step_for_cursor_and_deletion() {
        let mut b = Buffer::new("a\r\nb");
        assert_eq!(b.line_end(0), 1);
        assert_eq!(b.next_grapheme(1), 3);
        b.move_to(3, false);
        b.backspace();
        assert_eq!(b.text(), "ab");
        b.undo();
        b.move_to(1, false);
        typed(&mut b, "\n");
        assert_eq!(b.text(), "a\r\n\r\nb");
    }

    #[test]
    fn only_lf_and_crlf_split_lines() {
        let b = Buffer::new("a\u{2028}b\rc\u{b}d\ne");
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line(0), "a\u{2028}b\rc\u{b}d");
        assert_eq!(b.line(1), "e");
        assert_eq!(Buffer::new("a\r").line(0), "a\r");
    }

    #[test]
    fn line_heads_match_lines() {
        let text = format!("a\r\n{}\n\nж{}\r\n", "б".repeat(5000), "я".repeat(300));
        let b = Buffer::new(&text);
        let heads: Vec<_> = b.line_heads(0, 101).collect();
        assert_eq!(heads.len(), b.line_count());
        for (i, h) in heads.iter().enumerate() {
            assert!(h.len() <= 101 && b.line(i).starts_with(&**h));
        }
        assert_eq!(heads[0], "a");
        assert_eq!(b.line_heads(3, 2).next().unwrap(), "ж");
    }

    #[test]
    fn words() {
        let b = Buffer::new("слово, e\u{301}x  конец");
        assert_eq!(b.next_word(0), 5);
        assert_eq!(b.next_word(5), 7);
        assert_eq!(b.next_word(7), 12);
        assert_eq!(b.prev_word(12), 7);
        assert_eq!(b.prev_word(0), 0);
        assert_eq!(b.next_word(b.len()), b.len());
    }

    #[test]
    fn eol_conversion_is_undoable_and_keeps_position() {
        let mut b = Buffer::new("a\nbc\r\nd");
        assert!(b.ends.mixed);
        b.move_to(3, false);
        b.convert_eol(Eol::Crlf);
        assert_eq!(b.text(), "a\r\nbc\r\nd");
        assert_eq!(b.cursor(), 4);
        assert!(!b.ends.mixed);
        b.undo();
        assert_eq!(b.text(), "a\nbc\r\nd");
        assert!(b.ends.mixed);
    }

    #[test]
    fn replacing_everything_keeps_bytes_and_place() {
        let mut b = Buffer::new("a\nbcd\n");
        b.move_to(4, false);
        b.replace_all("x\r\nyz\nw");
        assert_eq!(b.text(), "x\r\nyz\nw");
        assert_eq!((b.line_of(b.cursor()), b.ends.mixed), (1, true));
        b.undo();
        assert_eq!((b.text().as_str(), b.cursor()), ("a\nbcd\n", 4));
        assert!(!b.ends.mixed && !b.is_modified());
        b.replace_all("");
        assert_eq!(b.cursor(), 0);
    }

    #[test]
    fn empty_operations_do_nothing() {
        let mut b = Buffer::new("");
        b.backspace();
        b.delete();
        b.delete_word();
        b.delete_word_back();
        assert!(!b.is_modified());
        assert_eq!(b.line(0), "");
    }

    #[test]
    fn edit_places_the_cursor_and_is_one_step() {
        let mut b = Buffer::new("один два");
        b.move_to(4, false);
        b.edit(0..4, "**один**", Some((Some(2), 6)));
        assert_eq!(b.selected_text().unwrap(), "один");
        b.undo();
        assert_eq!((b.text().as_str(), b.cursor()), ("один два", 4));
        b.redo();
        assert_eq!(b.selected_text().unwrap(), "один");
    }

    #[test]
    fn edit_lines_moves_the_selection_with_text() {
        let mut b = Buffer::new("x\r\naa\nbb\ncc");
        b.move_to(4, false);
        b.move_to(8, true);
        assert_eq!(b.selected_text().unwrap(), "a\nbb");
        b.edit_lines(1..3, |l| format!("  {l}"));
        assert_eq!(b.text(), "x\r\n  aa\n  bb\ncc");
        assert_eq!(b.selected_text().unwrap(), "a\n  bb");
        b.edit_lines(1..3, |l| l.trim_start().to_owned());
        assert_eq!(b.selected_text().unwrap(), "a\nbb");
        b.undo();
        b.undo();
        assert_eq!(b.text(), "x\r\naa\nbb\ncc");
        assert!(!b.is_modified());
        b.edit_lines(0..1, str::to_owned);
        assert!(!b.is_modified(), "no change, no step");
    }

    /// Случайные правки: полная отмена возвращает исходный текст, полный повтор конечный.
    #[test]
    fn random_edits_round_trip_through_undo() {
        let start = "# Текст\r\nвторая строка\nтретья 👨‍👩‍👧 e\u{301}\n";
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut rnd = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for _ in 0..200 {
            let mut b = Buffer::new(start);
            for _ in 0..60 {
                match rnd(11) {
                    0 => b.type_text("ж"),
                    1 => b.type_text("\n"),
                    2 => b.type_text(" "),
                    3 => b.paste("x\ny", false),
                    4 => b.paste("z", true),
                    5 => b.backspace(),
                    6 => b.delete(),
                    7 => b.delete_word_back(),
                    8 => b.convert_eol(if rnd(2) == 0 { Eol::Lf } else { Eol::Crlf }),
                    9 => b.move_to(b.next_grapheme(b.cursor()), rnd(2) == 0),
                    _ => b.move_to(b.prev_grapheme(b.cursor()), rnd(2) == 0),
                }
                assert!(b.cursor() <= b.len());
            }
            let end = b.text();
            while b.undo() {}
            assert_eq!(b.text(), start);
            assert!(!b.is_modified());
            while b.redo() {}
            assert_eq!(b.text(), end);
        }
    }
}
