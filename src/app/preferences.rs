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
        let Some(path) = self.config_path.clone() else {
            self.message = Some(s.settings_no_file.into());
            return;
        };
        if p.before != self.values {
            self.message = Some(match self.store_settings(&p.before) {
                Ok(()) => {
                    let mut text = format!("{}: {}", s.settings_saved, path.display());
                    if p.before.images != self.values.images {
                        text = format!("{text} ({})", s.settings_images_later);
                    }
                    text
                }
                Err(text) => text,
            });
        }
        if Item::ALL[p.selected] == Item::OpenFile {
            self.open_config(path);
        }
    }

    /// Записывает в конфиг то, что отличается от `before`. Ошибка: текст для
    /// пользователя.
    pub(super) fn store_settings(&self, before: &Values) -> Result<(), String> {
        let s = self.lang.strings();
        let Some(path) = &self.config_path else {
            return Err(s.settings_no_file.into());
        };
        let failed = |reason: &str| format!("{} {}: {reason}", s.not_saved, path.display());
        settings::store(path, before, &self.values).map_err(|e| match e {
            StoreError::Damaged => failed(s.settings_damaged),
            StoreError::Io(e) => failed(&e.to_string()),
        })
    }

    /// Показывает или скрывает номера строк и запоминает выбор.
    pub(super) fn toggle_line_numbers(&mut self) {
        let before = self.values.clone();
        let mut values = before.clone();
        values.line_numbers = !values.line_numbers;
        self.apply(values);
        let name = self.lang.setting(Item::LineNumbers);
        let value = self.lang.setting_value(Item::LineNumbers, &self.values);
        self.message = Some(match self.store_settings(&before) {
            Ok(()) => format!("{name}: {value}"),
            // Действует до конца сеанса.
            Err(text) => format!("{name}: {value}. {text}"),
        });
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
