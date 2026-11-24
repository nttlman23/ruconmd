//! Диалог настроек: изменения видны сразу, в конфиг пишутся по Enter.

use std::path::PathBuf;
use std::time::Duration;

use super::App;
use crate::dialog::{Dialog, Location, Preferences, Spot};
use crate::i18n::Lang;
use crate::key::{Chord, Key};
use crate::settings::{self, Item, StoreError, Values};

impl App {
    pub(super) fn ask_settings(&mut self) {
        self.dialog = Some(Dialog::Settings(Preferences {
            selected: 0,
            before: self.values.clone(),
        }));
    }

    pub(super) fn settings_key(&mut self, chord: Chord) {
        let Some(Dialog::Settings(p)) = &mut self.dialog else {
            return;
        };
        let last = Item::ALL.len() - 1;
        match chord.key {
            Key::Up => p.selected = p.selected.checked_sub(1).unwrap_or(last),
            Key::Down | Key::Tab => p.selected = (p.selected + 1) % Item::ALL.len(),
            Key::Home | Key::PageUp => p.selected = 0,
            Key::End | Key::PageDown => p.selected = last,
            Key::Left => self.change_setting(false),
            Key::Right | Key::Char(' ') => self.change_setting(true),
            Key::Enter => self.accept_settings(),
            Key::Esc => {
                let before = p.before.clone();
                self.dialog = None;
                self.apply(before);
            }
            _ => {}
        }
    }

    fn change_setting(&mut self, forward: bool) {
        let Some(Dialog::Settings(p)) = &self.dialog else {
            return;
        };
        let names: Vec<String> = self.themes.iter().map(|(name, _)| name.clone()).collect();
        let mut values = self.values.clone();
        values.step(Item::ALL[p.selected], forward, &names);
        self.apply(values);
    }

    /// Вводит настройки в действие.
    fn apply(&mut self, values: Values) {
        let old = std::mem::replace(&mut self.values, values);
        let new = &self.values;
        // Язык мог быть задан в командной строке, он меняется только явно.
        if new.lang != old.lang {
            self.lang = new.lang.unwrap_or_else(Lang::from_system);
        }
        if let Some((_, theme)) = self.themes.iter().find(|(name, _)| *name == new.theme) {
            self.theme = theme.clone();
        }
        let editor = &mut self.view.settings;
        editor.tab_width = new.tab_width;
        editor.wrap = new.wrap;
        editor.line_numbers = new.line_numbers;
        if new.wrap {
            self.view.left = 0;
        }
        self.preview_width = new.preview_width;
        self.backup = new.backup;
        let delay = Duration::from_secs(new.autosave_delay);
        self.autosave = new.autosave.then_some(delay);
        let all = new.files_all;
        if let Some(b) = &mut self.browser {
            b.tree.set_all(all);
        }
    }

    fn accept_settings(&mut self) {
        let Some(Dialog::Settings(p)) = self.dialog.take() else {
            return;
        };
        let s = self.lang.strings();
        let open = Item::ALL[p.selected] == Item::OpenFile;
        let Some(path) = self.config_path.clone() else {
            self.message = Some(s.settings_no_file.into());
            return;
        };
        if p.before != self.values {
            let text = match settings::store(&path, &p.before, &self.values) {
                Ok(()) => {
                    let mut text = format!("{}: {}", s.settings_saved, path.display());
                    if p.before.images != self.values.images {
                        text = format!("{text} ({})", s.settings_images_later);
                    }
                    Ok(text)
                }
                Err(StoreError::Damaged) => Err(s.settings_damaged.to_owned()),
                Err(StoreError::Io(e)) => Err(e.to_string()),
            };
            self.message = Some(match &text {
                Ok(text) => text.clone(),
                Err(e) => format!("{} {}: {e}", s.not_saved, path.display()),
            });
        }
        if open {
            self.open_config(path);
        }
    }

    /// Открывает файл настроек, а если его нет, создаёт из образца.
    fn open_config(&mut self, path: PathBuf) {
        if !path.exists()
            && let Err(e) = settings::create(&path, settings::EXAMPLE)
        {
            let s = self.lang.strings();
            self.message = Some(format!("{} {}: {e}", s.not_saved, path.display()));
            return;
        }
        self.go(Location {
            path: Some(path),
            spot: Spot::Start,
            back: false,
        });
    }
}
