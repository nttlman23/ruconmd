//! Приведение событий терминала к вводу программы.
//!
//! Учитываются особенности платформ:
//! - Windows сообщает и нажатия, и отпускания; символ, набранный Alt-кодом,
//!   приходит только отпусканием;
//! - в Windows нет события вставки, она приходит потоком нажатий;
//! - при русской раскладке Ctrl+Ы приходит как `ы` с флагом Ctrl.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};

use crate::key::{Chord, Key, Mods, latin_by_position};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Unix,
    Windows,
}

impl Platform {
    pub const CURRENT: Platform = if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Unix
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Key(Chord),
    /// `continued`: продолжение предыдущей вставки, для отмены одним шагом.
    Paste {
        text: String,
        continued: bool,
    },
    Mouse(MouseEvent),
    Resize,
    /// Окно терминала получило или потеряло фокус.
    Focus(bool),
}

/// Минимальная длина потока символов, который считается вставкой.
const PASTE_MIN_RUN: usize = 6;
/// То же, если внутри потока есть Enter или Tab.
const PASTE_MIN_RUN_WITH_BREAK: usize = 3;
/// Поток, начавшийся не позже этого срока после вставки, её продолжает.
const PASTE_CONTINUE: Duration = Duration::from_millis(30);

pub struct InputDecoder {
    platform: Platform,
    /// Нажатые символьные клавиши, только Windows.
    held: HashSet<char>,
    last_paste: Option<Instant>,
}

impl InputDecoder {
    pub fn new(platform: Platform) -> Self {
        Self {
            platform,
            held: HashSet::new(),
            last_paste: None,
        }
    }

    /// Превращает пачку событий, прочитанных подряд, в ввод программы.
    pub fn decode(&mut self, events: &[Event], now: Instant) -> Vec<Input> {
        let mut out = Vec::new();
        let mut run = Vec::new();
        for event in events {
            match event {
                Event::Key(key) => {
                    let Some(chord) = self.key(key) else { continue };
                    if self.platform == Platform::Windows && text_like(chord).is_some() {
                        run.push(chord);
                        continue;
                    }
                    self.flush(&mut run, &mut out, now);
                    out.push(Input::Key(chord));
                }
                other => {
                    self.flush(&mut run, &mut out, now);
                    match other {
                        Event::Paste(text) => out.push(Input::Paste {
                            text: text.replace("\r\n", "\n").replace('\r', "\n"),
                            continued: false,
                        }),
                        Event::Mouse(mouse) => out.push(Input::Mouse(*mouse)),
                        Event::Resize(..) => out.push(Input::Resize),
                        Event::FocusGained => out.push(Input::Focus(true)),
                        Event::FocusLost => out.push(Input::Focus(false)),
                        _ => {}
                    }
                }
            }
        }
        self.flush(&mut run, &mut out, now);
        out
    }

    fn key(&mut self, event: &KeyEvent) -> Option<Chord> {
        if event.kind == KeyEventKind::Release {
            return self.released(event);
        }
        if self.platform == Platform::Windows
            && let KeyCode::Char(c) = event.code
        {
            self.held.insert(fold(c));
        }
        normalize(event, self.platform)
    }

    /// Отпускание клавиши. Символ без парного нажатия введён Alt-кодом.
    fn released(&mut self, event: &KeyEvent) -> Option<Chord> {
        let KeyCode::Char(c) = event.code else {
            return None;
        };
        if self.held.remove(&fold(c)) {
            return None;
        }
        // Символ мог смениться, если Shift отпущен раньше клавиши: снимаем любую.
        if let Some(any) = self.held.iter().next().copied() {
            self.held.remove(&any);
            return None;
        }
        let alt_code = self.platform == Platform::Windows && !c.is_ascii();
        alt_code.then(|| Chord::plain(Key::Char(c)))
    }

    fn flush(&mut self, run: &mut Vec<Chord>, out: &mut Vec<Input>, now: Instant) {
        if run.is_empty() {
            return;
        }
        let continued = out.is_empty()
            && self
                .last_paste
                .is_some_and(|t| now.saturating_duration_since(t) < PASTE_CONTINUE);
        if continued || looks_pasted(run) {
            let text = run.iter().filter_map(|c| text_like(*c)).collect();
            out.push(Input::Paste { text, continued });
            self.last_paste = Some(now);
        } else {
            out.extend(run.iter().copied().map(Input::Key));
        }
        run.clear();
    }
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Символ, которым клавиша была бы в составе вставки.
pub fn text_like(chord: Chord) -> Option<char> {
    match chord.key {
        Key::Enter if chord.mods.is_none() => Some('\n'),
        Key::Tab if chord.mods.is_none() => Some('\t'),
        _ => chord.text(),
    }
}

fn looks_pasted(run: &[Chord]) -> bool {
    // Удержание Enter или Tab само по себе вставкой не считается.
    if !run.iter().any(|c| c.text().is_some()) {
        return false;
    }
    let (_, head) = run.split_last().expect("run is not empty");
    let break_inside = head.iter().any(|c| c.text().is_none());
    run.len() >= PASTE_MIN_RUN || (break_inside && run.len() >= PASTE_MIN_RUN_WITH_BREAK)
}

/// Приводит событие клавиши к сочетанию. `None`: клавиша программе не нужна.
pub fn normalize(event: &KeyEvent, platform: Platform) -> Option<Chord> {
    let mut mods = Mods {
        ctrl: event.modifiers.contains(KeyModifiers::CONTROL),
        alt: event.modifiers.contains(KeyModifiers::ALT),
        shift: event.modifiers.contains(KeyModifiers::SHIFT),
    };
    if event
        .modifiers
        .intersects(KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META)
    {
        return None;
    }
    let key = match event.code {
        KeyCode::Char(c) => return Some(normalize_char(c, mods, platform)),
        KeyCode::BackTab => {
            mods.shift = true;
            Key::Tab
        }
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Esc => Key::Esc,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    };
    Some(Chord::new(key, mods))
}

fn normalize_char(c: char, mods: Mods, platform: Platform) -> Chord {
    // AltGr в Windows приходит как Ctrl+Alt и означает ввод символа.
    let alt_gr = platform == Platform::Windows && mods.ctrl && mods.alt;
    if alt_gr || (!mods.ctrl && !mods.alt) {
        return Chord::plain(Key::Char(c));
    }
    // Часть терминалов передаёт Backspace как Ctrl+H.
    if mods
        == (Mods {
            ctrl: true,
            ..Mods::NONE
        })
        && latin_by_position(c) == 'h'
    {
        return Chord::plain(Key::Backspace);
    }
    Chord::shortcut(c, mods)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn ch(c: char) -> Event {
        press(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn release(c: char) -> Event {
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char(c),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ))
    }

    fn typed(text: &str) -> Vec<Event> {
        text.chars()
            .map(|c| match c {
                '\n' => press(KeyCode::Enter, KeyModifiers::NONE),
                '\t' => press(KeyCode::Tab, KeyModifiers::NONE),
                c => ch(c),
            })
            .collect()
    }

    fn keys(text: &str) -> Vec<Input> {
        text.chars()
            .map(|c| Input::Key(Chord::plain(Key::Char(c))))
            .collect()
    }

    fn paste(text: &str, continued: bool) -> Vec<Input> {
        vec![Input::Paste {
            text: text.into(),
            continued,
        }]
    }

    fn decode(platform: Platform, events: &[Event]) -> Vec<Input> {
        InputDecoder::new(platform).decode(events, Instant::now())
    }

    fn chord(code: KeyCode, modifiers: KeyModifiers, platform: Platform) -> String {
        normalize(&KeyEvent::new(code, modifiers), platform)
            .unwrap()
            .to_string()
    }

    #[test]
    fn shortcuts_ignore_layout_and_caps_lock() {
        let ctrl = KeyModifiers::CONTROL;
        for c in ['s', 'S', 'ы', 'Ы'] {
            for p in [Platform::Unix, Platform::Windows] {
                assert_eq!(chord(KeyCode::Char(c), ctrl, p), "Ctrl+S", "{c}");
            }
        }
        assert_eq!(
            chord(KeyCode::Char('ш'), KeyModifiers::ALT, Platform::Unix),
            "Alt+I"
        );
    }

    #[test]
    fn plain_text_keeps_layout_and_case() {
        let input = decode(Platform::Unix, &[ch('ы'), ch('Ы')]);
        assert_eq!(input, keys("ыЫ"));
        let shifted = press(KeyCode::Char('Ф'), KeyModifiers::SHIFT);
        assert_eq!(decode(Platform::Unix, &[shifted]), keys("Ф"));
    }

    #[test]
    fn alt_gr_types_text_on_windows_only() {
        let alt_gr = KeyModifiers::CONTROL | KeyModifiers::ALT;
        assert_eq!(chord(KeyCode::Char('€'), alt_gr, Platform::Windows), "€");
        assert_eq!(
            chord(KeyCode::Char('e'), alt_gr, Platform::Unix),
            "Ctrl+Alt+E"
        );
    }

    #[test]
    fn ctrl_h_and_back_tab() {
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(chord(KeyCode::Char('h'), ctrl, Platform::Unix), "Backspace");
        for mods in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            assert_eq!(chord(KeyCode::BackTab, mods, Platform::Unix), "Shift+Tab");
        }
    }

    #[test]
    fn super_combinations_are_ignored() {
        let e = press(KeyCode::Char('s'), KeyModifiers::SUPER);
        assert!(decode(Platform::Unix, &[e]).is_empty());
    }

    #[test]
    fn windows_releases_do_not_duplicate_text() {
        let events = [
            ch('Ф'),
            release('ф'),
            ch('!'),
            ch('b'),
            release('1'),
            release('b'),
        ];
        let got = decode(Platform::Windows, &events);
        assert_eq!(got, keys("Ф!b"));
    }

    #[test]
    fn windows_alt_code_arrives_as_release() {
        assert_eq!(decode(Platform::Windows, &[release('№')]), keys("№"));
        assert!(decode(Platform::Windows, &[release('a')]).is_empty());
        assert!(decode(Platform::Unix, &[release('№')]).is_empty());
    }

    #[test]
    fn windows_fast_stream_is_a_paste() {
        let got = decode(Platform::Windows, &typed("- один\n- два\n"));
        assert_eq!(got, paste("- один\n- два\n", false));
        assert_eq!(
            decode(Platform::Windows, &typed("a\nb")),
            paste("a\nb", false)
        );
    }

    #[test]
    fn windows_paste_with_releases_is_whole() {
        let mut events = Vec::new();
        for c in "привет мир".chars() {
            events.extend([ch(c), release(c)]);
        }
        let got = decode(Platform::Windows, &events);
        assert_eq!(got, paste("привет мир", false));
    }

    #[test]
    fn windows_typing_is_not_a_paste() {
        assert_eq!(decode(Platform::Windows, &typed("abc")), keys("abc"));
        let enter = Input::Key(Chord::plain(Key::Enter));
        let got = decode(Platform::Windows, &typed("ab\n"));
        assert_eq!(got.last(), Some(&enter));
        let held = decode(Platform::Windows, &typed("\n\n\n\n\n\n\n\n"));
        assert_eq!(held, vec![enter; 8]);
    }

    #[test]
    fn windows_paste_continues_across_batches() {
        let mut d = InputDecoder::new(Platform::Windows);
        let t = Instant::now();
        assert_eq!(
            d.decode(&typed("длинный текст"), t),
            paste("длинный текст", false)
        );
        let soon = t + Duration::from_millis(10);
        assert_eq!(d.decode(&typed("\nx"), soon), paste("\nx", true));
        let later = soon + Duration::from_millis(200);
        assert_eq!(d.decode(&typed("y"), later), keys("y"));
    }

    #[test]
    fn unix_uses_bracketed_paste_only() {
        assert_eq!(decode(Platform::Unix, &typed("abcdefgh")), keys("abcdefgh"));
        let event = Event::Paste("a\r\nb\rc".into());
        assert_eq!(decode(Platform::Unix, &[event]), paste("a\nb\nc", false));
    }

    #[test]
    fn focus_is_reported() {
        for platform in [Platform::Unix, Platform::Windows] {
            let got = decode(platform, &[Event::FocusLost, Event::FocusGained]);
            assert_eq!(got, [Input::Focus(false), Input::Focus(true)]);
        }
    }

    #[test]
    fn shortcut_splits_a_stream() {
        let mut events = typed("abcdefgh");
        events.push(press(KeyCode::Char('ы'), KeyModifiers::CONTROL));
        let got = decode(Platform::Windows, &events);
        assert_eq!(got.len(), 2);
        assert!(matches!(&got[1], Input::Key(c) if c.to_string() == "Ctrl+S"));
    }
}
