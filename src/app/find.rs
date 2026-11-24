//! Поиск и замена.

use std::ops::Range;

use super::{App, Mode};
use crate::dialog::{Ask, Dialog, Field, Replacing};
use crate::key::{Chord, Key, Mods, latin_by_position};
use crate::search::{Query, Search};

impl App {
    /// Открывает запрос текста: выделенного, а без выделения прежнего.
    pub(super) fn ask_find(&mut self, ask: Ask) {
        let selected = self
            .buffer
            .selected_text()
            .filter(|t| self.mode == Mode::Edit && !t.contains('\n') && !self.query.regex);
        let text = selected.unwrap_or_else(|| self.query.text.clone());
        self.dialog = Some(Dialog::Field(Field::new(ask, "", text)));
        self.update_find_title();
    }

    /// Alt+C и Alt+R в запросе поиска.
    pub(super) fn find_option(&mut self, chord: Chord) -> bool {
        let alt = Mods {
            alt: true,
            ..Mods::NONE
        };
        let Some(Dialog::Field(field)) = &self.dialog else {
            return false;
        };
        if !matches!(field.ask, Ask::Find | Ask::ReplaceFind) || chord.mods != alt {
            return false;
        }
        match chord.key {
            Key::Char('c') => self.query.case_sensitive = !self.query.case_sensitive,
            Key::Char('r') => self.query.regex = !self.query.regex,
            _ => return false,
        }
        self.update_find_title();
        true
    }

    fn update_find_title(&mut self) {
        let s = self.lang.strings();
        let flag = |on: bool| if on { '+' } else { '-' };
        let (case, regex) = (flag(self.query.case_sensitive), flag(self.query.regex));
        if let Some(Dialog::Field(field)) = &mut self.dialog {
            let name = match field.ask {
                Ask::ReplaceFind => s.replace_what,
                _ => s.find,
            };
            field.title = format!(
                "{name} [Alt+C {} {case}] [Alt+R regex {regex}]",
                s.match_case
            );
        }
    }

    /// Принимает запрос. При ошибке в выражении запрос остаётся открытым.
    fn set_query(&mut self, ask: Ask, text: String) -> bool {
        let query = Query {
            text,
            ..self.query.clone()
        };
        match Search::new(query.clone()) {
            Ok(search) => {
                self.query = query;
                self.search = Some(search);
                true
            }
            Err(error) => {
                self.ask_find(ask);
                if let Some(Dialog::Field(field)) = &mut self.dialog {
                    field.text = query.text;
                    field.cursor = field.text.len();
                    field.title = format!("{}: {error}", self.lang.strings().bad_regex);
                }
                false
            }
        }
    }

    pub(super) fn submit_find(&mut self, text: String) {
        if text.is_empty() {
            self.search = None;
        } else if self.set_query(Ask::Find, text) {
            self.find(true);
        }
    }

    pub(super) fn submit_replace_find(&mut self, text: String) {
        if !text.is_empty() && self.set_query(Ask::ReplaceFind, text) {
            let title = self.lang.strings().replace_with;
            let field = Field::new(Ask::ReplaceWith, title, self.replace_with.clone());
            self.dialog = Some(Dialog::Field(field));
        }
    }

    /// Переходит к следующему или предыдущему совпадению по кругу.
    pub(super) fn find(&mut self, forward: bool) {
        if self.search.is_none() {
            return self.ask_find(Ask::Find);
        }
        let found = match self.mode {
            Mode::Edit => self.find_in_text(forward),
            Mode::View if !self.pane.is_ready() => {
                self.message = Some(self.lang.strings().preparing.into());
                return;
            }
            Mode::View => {
                self.pane.set_search(self.search.as_ref());
                self.pane.find(forward)
            }
        };
        let s = self.lang.strings();
        self.message = Some(match found {
            None => s.not_found.into(),
            Some((number, total, wrapped)) => {
                let note = if wrapped { s.wrapped } else { "" };
                format!("{number} {} {total}{note}", s.of)
            }
        });
    }

    fn find_in_text(&mut self, forward: bool) -> Option<(usize, usize, bool)> {
        let search = self.search.as_ref()?;
        let text = self.buffer.text();
        let cursor = self.buffer.cursor();
        let selection = self.buffer.selection().unwrap_or(cursor..cursor);
        let (hit, wrapped) = if forward {
            let from = self.buffer.byte_of(selection.end);
            let mut next = search.next(&text, from);
            // Пустое совпадение под курсором уже найдено прошлый раз.
            if next == Some(from..from) && self.last_empty_hit == Some(from) {
                next = following(&text, from).and_then(|from| search.next(&text, from));
            }
            match next {
                Some(hit) => (hit, false),
                None => (search.next(&text, 0)?, true),
            }
        } else {
            let before = self.buffer.byte_of(selection.start);
            match search.previous(&text, before) {
                Some(hit) => (hit, false),
                None => (search.previous(&text, text.len() + 1)?, true),
            }
        };
        let (mut number, mut total) = (0, 0);
        for m in search.matches(&text) {
            total += 1;
            number += usize::from(m.start <= hit.start);
        }
        self.select_bytes(&hit);
        Some((number, total, wrapped))
    }

    fn select_bytes(&mut self, hit: &Range<usize>) {
        self.last_empty_hit = hit.is_empty().then_some(hit.start);
        let start = self.buffer.pos_of_byte(hit.start);
        let end = self.buffer.pos_of_byte(hit.end);
        self.move_to(start, false);
        self.move_to(end, true);
    }

    // --- Замена ---

    pub(super) fn submit_replace_with(&mut self, with: String) {
        let Some(search) = &self.search else { return };
        self.replace_with = with.clone();
        let text = self.buffer.text();
        let cursor = self.buffer.cursor();
        let from = self.buffer.selection().map_or(cursor, |r| r.start);
        let start = self.buffer.byte_of(from);
        let mut state = Replacing {
            with,
            start,
            wrapped: false,
            hit: 0..0,
            was_empty_hit: false,
            done: 0,
        };
        match search.next(&text, start) {
            Some(hit) => state.hit = hit,
            None => match search.next(&text, 0) {
                Some(hit) => {
                    state.hit = hit;
                    state.wrapped = true;
                }
                None => {
                    self.message = Some(self.lang.strings().not_found.into());
                    return;
                }
            },
        }
        self.select_bytes(&state.hit.clone());
        self.dialog = Some(Dialog::Replace(state));
    }

    pub(super) fn replace_key(&mut self, chord: Chord) {
        let Some(Dialog::Replace(mut state)) = self.dialog.take() else {
            return;
        };
        let plain = chord.mods.is_none() || chord.mods.shift;
        match chord.key {
            Key::Enter => self.replace_one(&mut state),
            Key::Char(' ') | Key::Tab | Key::Down => {
                let (from, empty) = (state.hit.end, state.hit.is_empty());
                self.next_replacement(state, from, empty);
                return;
            }
            Key::Char(c) if plain && latin_by_position(c) == 'a' => {
                self.replace_rest(&mut state);
                return self.finish_replacing(state.done);
            }
            Key::Esc => return self.finish_replacing(state.done),
            _ => {
                self.dialog = Some(Dialog::Replace(state));
                return;
            }
        }
        let (from, empty) = (state.hit.end, state.hit.is_empty());
        // Пустое совпадение после вставки надо пройти, даже если вставлен текст.
        let was_empty = std::mem::take(&mut state.was_empty_hit) || empty;
        self.next_replacement(state, from, was_empty);
    }

    fn replace_one(&mut self, state: &mut Replacing) {
        let Some(search) = &self.search else { return };
        let text = self.buffer.text();
        let Some(new) = search.replacement(&text, &state.hit, &state.with) else {
            return;
        };
        state.was_empty_hit = state.hit.is_empty();
        let before = text.len();
        let start = self.buffer.pos_of_byte(state.hit.start);
        let end = self.buffer.pos_of_byte(state.hit.end);
        self.buffer.edit(start..end, &new, None);
        self.edited();
        let after = self.buffer.byte_len();
        if state.wrapped {
            state.start = (state.start + after).saturating_sub(before);
        }
        state.hit.end = (state.hit.end + after).saturating_sub(before);
        state.done += 1;
    }

    fn next_replacement(&mut self, mut state: Replacing, from: usize, skip_empty: bool) {
        let Some(search) = &self.search else { return };
        let text = self.buffer.text();
        let mut hit = search.next(&text, from);
        if skip_empty && hit == Some(from..from) {
            hit = following(&text, from).and_then(|from| search.next(&text, from));
        }
        if hit.is_none() && !state.wrapped {
            state.wrapped = true;
            hit = search.next(&text, 0);
        }
        match hit {
            // Вернулись туда, откуда начали.
            Some(hit) if !state.wrapped || hit.start < state.start => {
                self.select_bytes(&hit);
                state.hit = hit;
                self.dialog = Some(Dialog::Replace(state));
            }
            _ => self.finish_replacing(state.done),
        }
    }

    /// Заменяет текущее совпадение и все, до которых ещё не дошли.
    fn replace_rest(&mut self, state: &mut Replacing) {
        let Some(search) = &self.search else { return };
        let text = self.buffer.text();
        let (current, start, wrapped) = (state.hit.start, state.start, state.wrapped);
        let accept = |hit: &Range<usize>| {
            if wrapped {
                (current..start).contains(&hit.start)
            } else {
                hit.start >= current || hit.start < start
            }
        };
        if let Some((range, new, count)) = search.replace_all(&text, &state.with, accept) {
            let start = self.buffer.pos_of_byte(range.start);
            let end = self.buffer.pos_of_byte(range.end);
            self.buffer.edit(start..end, &new, None);
            self.edited();
            state.done += count;
        }
    }

    fn finish_replacing(&mut self, done: usize) {
        self.dialog = None;
        let name = self.lang.strings().replaced;
        self.message = Some(format!("{name}: {done}"));
    }
}

/// Начало символа после байта.
fn following(text: &str, byte: usize) -> Option<usize> {
    text[byte..].chars().next().map(|c| byte + c.len_utf8())
}
