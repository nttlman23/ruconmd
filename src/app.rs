//! Состояние программы и реакция на ввод. Отрисовки и терминала здесь нет.

use std::io;
use std::path::PathBuf;

use ratatui::crossterm::event::MouseEventKind;

use crate::command::{Command, Mode};
use crate::i18n::Lang;
use crate::input::Input;
use crate::key::{Chord, Key};
use crate::keymap::Keymap;

const WHEEL_LINES: isize = 3;

pub struct App {
    pub path: Option<PathBuf>,
    pub lines: Vec<String>,
    pub mode: Mode,
    pub lang: Lang,
    pub keymap: Keymap,
    /// Первая видимая строка.
    pub scroll: usize,
    /// Высота рабочей области, задаётся при отрисовке.
    pub page: usize,
    pub message: Option<String>,
    pub help: bool,
    pub should_quit: bool,
}

impl App {
    pub fn open(path: Option<PathBuf>, mode: Mode, lang: Lang) -> io::Result<Self> {
        let text = match &path {
            Some(p) if p.is_dir() => {
                let reason = lang.strings().is_directory;
                return Err(io::Error::other(format!("{}: {reason}", p.display())));
            }
            Some(p) => match std::fs::read(p) {
                // Кодировки появятся на этапе 2, запись до тех пор отключена.
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(io::Error::new(e.kind(), format!("{}: {e}", p.display()))),
            },
            None => String::new(),
        };
        Ok(Self::with_text(path, &text, mode, lang))
    }

    pub fn with_text(path: Option<PathBuf>, text: &str, mode: Mode, lang: Lang) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        Self {
            path,
            lines: text.lines().map(str::to_owned).collect(),
            mode,
            lang,
            keymap: Keymap::default(),
            scroll: 0,
            page: 1,
            message: None,
            help: false,
            should_quit: false,
        }
    }

    pub fn handle(&mut self, input: Input) {
        match input {
            Input::Key(chord) => self.key(chord),
            Input::Paste { .. } if self.mode == Mode::Edit => {
                self.message = Some(self.lang.strings().text_input_later.into());
            }
            Input::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => self.scroll_by(-WHEEL_LINES),
                MouseEventKind::ScrollDown => self.scroll_by(WHEEL_LINES),
                _ => {}
            },
            Input::Paste { .. } | Input::Resize => {}
        }
    }

    fn key(&mut self, chord: Chord) {
        self.message = None;
        let command = self.keymap.lookup(self.mode, chord);
        if self.help && command != Some(Command::Quit) {
            self.help = false;
            return;
        }
        if let Some(command) = command {
            return self.run(command);
        }
        if chord.mods.is_none() {
            let page = self.page.saturating_sub(1).max(1) as isize;
            match chord.key {
                Key::Up => return self.scroll_by(-1),
                Key::Down => return self.scroll_by(1),
                Key::PageUp => return self.scroll_by(-page),
                Key::PageDown => return self.scroll_by(page),
                Key::Home => return self.scroll = 0,
                Key::End => return self.scroll = self.max_scroll(),
                _ => {}
            }
        }
        if self.mode == Mode::Edit && chord.text().is_some() {
            self.message = Some(self.lang.strings().text_input_later.into());
        }
    }

    fn run(&mut self, command: Command) {
        match command {
            // Обе области показывают одни и те же строки, позиция общая.
            Command::ToggleMode => self.mode = self.mode.toggled(),
            Command::Quit => self.should_quit = true,
            Command::Help => self.help = true,
            other => {
                let name = self.lang.command(other);
                let note = self.lang.strings().not_implemented;
                self.message = Some(format!("{name}: {note}"));
            }
        }
    }

    fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(self.page)
    }

    fn scroll_by(&mut self, delta: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.max_scroll());
    }

    /// Вызывается отрисовкой: после изменения размера окна позиция остаётся верной.
    pub fn set_page(&mut self, rows: usize) {
        self.page = rows;
        self.scroll = self.scroll.min(self.max_scroll());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(lines: usize) -> App {
        let text: String = (0..lines).map(|i| format!("line {i}\n")).collect();
        let mut app = App::with_text(None, &text, Mode::Edit, Lang::En);
        app.set_page(10);
        app
    }

    fn press(app: &mut App, chord: &str) {
        app.handle(Input::Key(chord.parse().unwrap()));
    }

    #[test]
    fn both_hotkeys_toggle_and_keep_position() {
        let mut app = app(100);
        press(&mut app, "pagedown");
        let before = app.scroll;
        for key in ["f5", "ctrl+e", "ctrl+у"] {
            let mode = app.mode;
            press(&mut app, key);
            assert_eq!(app.mode, mode.toggled(), "{key}");
            assert_eq!(app.scroll, before);
        }
    }

    #[test]
    fn scroll_stays_in_range() {
        let mut app = app(25);
        press(&mut app, "end");
        press(&mut app, "pagedown");
        assert_eq!(app.scroll, 15);
        app.set_page(40);
        assert_eq!(app.scroll, 0);
        press(&mut app, "up");
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn help_closes_on_any_key_but_quit_still_works() {
        let mut app = app(1);
        press(&mut app, "f1");
        assert!(app.help);
        press(&mut app, "f5");
        assert!(!app.help);
        assert_eq!(app.mode, Mode::Edit);
        press(&mut app, "f1");
        press(&mut app, "ctrl+q");
        assert!(app.should_quit);
    }

    #[test]
    fn directory_is_an_error_and_missing_file_is_empty() {
        let dir = std::env::temp_dir();
        assert!(App::open(Some(dir.clone()), Mode::Edit, Lang::En).is_err());
        let missing = dir.join("ruconmd-no-such-file.md");
        assert!(
            App::open(Some(missing), Mode::Edit, Lang::En)
                .unwrap()
                .lines
                .is_empty()
        );
    }
}
