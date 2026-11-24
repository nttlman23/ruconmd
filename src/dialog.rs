//! Запросы к пользователю: строка ввода и список вариантов.

use std::ops::Range;
use std::path::PathBuf;

use crate::buffer::Eol;
use crate::encoding::{Enc, Format};
use crate::key::{Chord, Key};

/// На сколько пунктов сдвигают меню PgUp и PgDn.
const PAGE: usize = 10;

/// Место, куда ведёт переход.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// `None`: документ без имени.
    pub path: Option<PathBuf>,
    pub spot: Spot,
    /// Возврат по истории, а не новый переход.
    pub back: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spot {
    Start,
    Anchor(String),
    Src(usize),
}

/// Что сделать после сохранения или отказа от него.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    Stay,
    Quit,
    Go(Location),
}

/// Ход замены. Места измеряются в байтах текста.
#[derive(Debug, PartialEq, Eq)]
pub struct Replacing {
    pub with: String,
    /// Откуда начали: второй круг останавливается здесь.
    pub start: usize,
    pub wrapped: bool,
    pub hit: Range<usize>,
    /// Последнее заменённое совпадение было пустым.
    pub was_empty_hit: bool,
    pub done: usize,
}

/// Запись файла, которую можно продолжить после ответа пользователя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveJob {
    pub path: PathBuf,
    pub format: Format,
    pub then: Then,
    /// Пользователь согласился записать поверх другого файла.
    pub overwrite: bool,
    /// Пользователь согласился потерять нечитаемые байты исходного файла.
    pub allow_lossy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Cancel,
    Save(SaveJob),
    SaveThen(Then),
    Discard(Then),
    /// Показать место исходного текста.
    GotoSrc(usize),
    Reload(Enc),
    SetFormat(Format),
    SetEol(Eol),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    SavePath(Then),
    GotoLine,
    Find,
    ReplaceFind,
    ReplaceWith,
}

#[derive(Debug)]
pub struct Field {
    pub ask: Ask,
    pub title: String,
    pub text: String,
    /// Курсор, смещение в байтах.
    pub cursor: usize,
}

#[derive(Debug)]
pub struct Menu {
    pub title: String,
    pub items: Vec<(String, Action)>,
    pub selected: usize,
}

#[derive(Debug)]
pub enum Dialog {
    Field(Field),
    Menu(Menu),
    Replace(Replacing),
}

/// Что получилось после нажатия.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Open,
    Close,
    Submit(Ask, String),
    Act(Action),
}

impl Field {
    pub fn new(ask: Ask, title: &str, text: String) -> Self {
        Self {
            ask,
            title: title.to_owned(),
            cursor: text.len(),
            text,
        }
    }

    pub fn insert(&mut self, text: &str) {
        let line = text.lines().next().unwrap_or_default();
        let clean: String = line.chars().filter(|c| !c.is_control()).collect();
        self.text.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
    }

    fn prev(&self) -> usize {
        let before = self.text[..self.cursor].char_indices().next_back();
        before.map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        let after = self.text[self.cursor..].chars().next();
        self.cursor + after.map_or(0, char::len_utf8)
    }

    fn key(&mut self, chord: Chord) -> Outcome {
        match chord.key {
            Key::Enter => return Outcome::Submit(self.ask.clone(), self.text.clone()),
            Key::Backspace => {
                let start = self.prev();
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            Key::Delete => {
                let end = self.next();
                self.text.replace_range(self.cursor..end, "");
            }
            Key::Left => self.cursor = self.prev(),
            Key::Right => self.cursor = self.next(),
            Key::Home => self.cursor = 0,
            Key::End => self.cursor = self.text.len(),
            _ => {
                if let Some(c) = chord.text() {
                    self.insert(c.encode_utf8(&mut [0; 4]));
                }
            }
        }
        Outcome::Open
    }
}

impl Menu {
    pub fn new(title: &str, items: Vec<(String, Action)>) -> Self {
        Self {
            title: title.to_owned(),
            items,
            selected: 0,
        }
    }

    fn key(&mut self, chord: Chord) -> Outcome {
        let last = self.items.len() - 1;
        match chord.key {
            Key::Up => self.selected = self.selected.checked_sub(1).unwrap_or(last),
            Key::Down | Key::Tab => self.selected = (self.selected + 1) % self.items.len(),
            Key::Home => self.selected = 0,
            Key::End => self.selected = last,
            Key::PageUp => self.selected = self.selected.saturating_sub(PAGE),
            Key::PageDown => self.selected = (self.selected + PAGE).min(last),
            Key::Enter => return Outcome::Act(self.items[self.selected].1.clone()),
            _ => {}
        }
        Outcome::Open
    }
}

impl Dialog {
    pub fn key(&mut self, chord: Chord) -> Outcome {
        if chord.key == Key::Esc {
            return Outcome::Close;
        }
        match self {
            Dialog::Field(field) => field.key(chord),
            Dialog::Menu(menu) => menu.key(chord),
            // Клавиши замены обрабатывает приложение.
            Dialog::Replace(_) => Outcome::Open,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(d: &mut Dialog, key: &str) -> Outcome {
        d.key(key.parse().unwrap())
    }

    #[test]
    fn field_edits_unicode_text() {
        let mut d = Dialog::Field(Field::new(Ask::GotoLine, "", "аб".into()));
        press(&mut d, "left");
        d.key(Chord::plain(Key::Char('ж')));
        press(&mut d, "home");
        press(&mut d, "delete");
        press(&mut d, "backspace");
        let got = press(&mut d, "enter");
        assert_eq!(got, Outcome::Submit(Ask::GotoLine, "жб".into()));
        assert_eq!(press(&mut d, "esc"), Outcome::Close);
    }

    #[test]
    fn field_takes_only_the_first_pasted_line() {
        let mut f = Field::new(Ask::GotoLine, "", String::new());
        f.insert("a\u{1b}b\nc");
        assert_eq!(f.text, "ab");
    }

    #[test]
    fn menu_wraps_around() {
        let items = vec![
            ("a".into(), Action::SetEol(Eol::Lf)),
            ("b".into(), Action::Cancel),
        ];
        let mut d = Dialog::Menu(Menu::new("", items));
        press(&mut d, "up");
        assert_eq!(press(&mut d, "enter"), Outcome::Act(Action::Cancel));
        press(&mut d, "down");
        assert_eq!(
            press(&mut d, "enter"),
            Outcome::Act(Action::SetEol(Eol::Lf))
        );
    }
}
