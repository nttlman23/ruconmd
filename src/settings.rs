//! Настройки, которые меняются в программе, и запись их в конфиг.

use std::fs;
use std::io;
use std::path::Path;

use toml_edit::{DocumentMut, Value};

use crate::app::shell::{HEIGHT_MAX, HEIGHT_MIN};
use crate::config::{Config, FILES_WIDTH, MAX_TAB, MIN_WIDTH};
use crate::console;
use crate::file;
use crate::graphics::Wanted;
use crate::i18n::Lang;

/// Образец со всеми параметрами, из него создаётся конфиг.
pub const EXAMPLE: &str = include_str!("../config.example.toml");

const BOM: char = '\u{feff}';
/// Значения задержки автосохранения, секунды.
const DELAYS: [u64; 10] = [1, 2, 3, 5, 10, 15, 30, 60, 300, 600];
const WIDTH_STEP: usize = 10;
const WIDEST: usize = 200;
const FILES_STEP: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Values {
    pub lang: Option<Lang>,
    pub theme: String,
    pub tab_width: usize,
    pub wrap: bool,
    pub line_numbers: bool,
    pub preview_width: usize,
    pub images: Wanted,
    pub backup: bool,
    pub autosave: bool,
    pub autosave_delay: u64,
    pub files_all: bool,
    pub files_width: u16,
    /// Пустая строка: оболочка по умолчанию.
    pub console_shell: String,
    pub console_height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Language,
    Theme,
    TabWidth,
    Wrap,
    LineNumbers,
    PreviewWidth,
    Images,
    Backup,
    Autosave,
    AutosaveDelay,
    FilesAll,
    FilesWidth,
    ConsoleShell,
    ConsoleHeight,
    /// Открыть сам файл: в нём клавиши и свои темы.
    OpenFile,
}

impl Item {
    pub const ALL: [Item; 15] = [
        Item::Language,
        Item::Theme,
        Item::TabWidth,
        Item::Wrap,
        Item::LineNumbers,
        Item::PreviewWidth,
        Item::Images,
        Item::Backup,
        Item::Autosave,
        Item::AutosaveDelay,
        Item::FilesAll,
        Item::FilesWidth,
        Item::ConsoleShell,
        Item::ConsoleHeight,
        Item::OpenFile,
    ];
}

/// Соседнее значение списка, по кругу. Чужое значение заменяется ближайшим большим.
fn neighbour<T: PartialOrd + Copy>(list: &[T], current: T, forward: bool) -> T {
    let exact = list.iter().position(|v| *v == current);
    let larger = list.iter().position(|v| *v > current);
    let index = match (exact, larger) {
        (Some(i), _) if forward => (i + 1) % list.len(),
        (Some(i), _) => (i + list.len() - 1) % list.len(),
        (None, Some(i)) if forward => i,
        (None, Some(i)) => (i + list.len() - 1) % list.len(),
        (None, None) if forward => 0,
        (None, None) => list.len() - 1,
    };
    list[index]
}

/// То же для значений без порядка.
fn turn<T: PartialEq + Clone>(list: &[T], current: &T, forward: bool) -> T {
    let step = if forward { 1 } else { list.len() - 1 };
    let index = list.iter().position(|v| v == current);
    list[index.map_or(0, |i| (i + step) % list.len())].clone()
}

impl Values {
    pub fn of(config: &Config) -> Self {
        let delay = config.autosave.map(|d| d.as_secs());
        Self {
            lang: config.lang,
            theme: config.theme_name.clone(),
            tab_width: config.editor.tab_width,
            wrap: config.editor.wrap,
            line_numbers: config.editor.line_numbers,
            preview_width: config.preview_width,
            images: config.images,
            backup: config.backup,
            autosave: config.autosave.is_some(),
            autosave_delay: delay.unwrap_or(config.autosave_delay),
            files_all: config.files_all,
            files_width: config.files_width,
            console_shell: config.console_shell.clone(),
            console_height: config.console_height,
        }
    }

    /// Меняет параметр на следующее или предыдущее значение.
    pub fn step(&mut self, item: Item, forward: bool, themes: &[String]) {
        match item {
            Item::Language => {
                let all = [None, Some(Lang::Ru), Some(Lang::En)];
                self.lang = turn(&all, &self.lang, forward);
            }
            Item::Theme => self.theme = turn(themes, &self.theme, forward),
            Item::TabWidth => {
                let all: Vec<usize> = (1..=MAX_TAB).collect();
                self.tab_width = neighbour(&all, self.tab_width, forward);
            }
            Item::Wrap => self.wrap = !self.wrap,
            Item::LineNumbers => self.line_numbers = !self.line_numbers,
            Item::PreviewWidth => {
                let mut all = vec![0];
                all.extend((MIN_WIDTH..=WIDEST).step_by(WIDTH_STEP));
                self.preview_width = neighbour(&all, self.preview_width, forward);
            }
            Item::Images => {
                let mut all = vec![Wanted::Auto, Wanted::Off];
                all.extend(Wanted::NAMES.iter().filter_map(|n| Wanted::from_name(n)));
                self.images = turn(&all, &self.images, forward);
            }
            Item::Backup => self.backup = !self.backup,
            Item::Autosave => self.autosave = !self.autosave,
            Item::AutosaveDelay => {
                self.autosave_delay = neighbour(&DELAYS, self.autosave_delay, forward);
            }
            Item::FilesAll => self.files_all = !self.files_all,
            Item::FilesWidth => {
                let all: Vec<u16> = FILES_WIDTH.step_by(FILES_STEP as usize).collect();
                self.files_width = neighbour(&all, self.files_width, forward);
            }
            Item::ConsoleShell => {
                let mut all = console::known_shells();
                if !all.contains(&self.console_shell) {
                    all.push(self.console_shell.clone());
                }
                self.console_shell = turn(&all, &self.console_shell, forward);
            }
            Item::ConsoleHeight => {
                let all: Vec<u16> = (HEIGHT_MIN..=HEIGHT_MAX).step_by(10).collect();
                self.console_height = neighbour(&all, self.console_height, forward);
            }
            Item::OpenFile => {}
        }
    }

    /// Параметры под именами из конфига.
    fn named(&self) -> Vec<(Option<&'static str>, &'static str, Value)> {
        let number = |n: usize| Value::from(i64::try_from(n).unwrap_or(i64::MAX));
        let lang = match self.lang {
            None => "auto",
            Some(Lang::Ru) => "ru",
            Some(Lang::En) => "en",
        };
        let images = match self.images {
            Wanted::Auto => Value::from(true),
            Wanted::Off => Value::from(false),
            forced => Value::from(forced.name().unwrap_or("kitty")),
        };
        vec![
            (None, "language", lang.into()),
            (None, "theme", self.theme.as_str().into()),
            (Some("editor"), "tab_width", number(self.tab_width)),
            (Some("editor"), "wrap", self.wrap.into()),
            (Some("editor"), "line_numbers", self.line_numbers.into()),
            (Some("preview"), "max_width", number(self.preview_width)),
            (Some("preview"), "images", images),
            (Some("save"), "backup", self.backup.into()),
            (Some("save"), "autosave", self.autosave.into()),
            (
                Some("save"),
                "autosave_delay_sec",
                number(self.autosave_delay as usize),
            ),
            (Some("files"), "all", self.files_all.into()),
            (
                Some("files"),
                "width",
                number(usize::from(self.files_width)),
            ),
            (Some("console"), "shell", self.console_shell.as_str().into()),
            (
                Some("console"),
                "height",
                number(usize::from(self.console_height)),
            ),
        ]
    }
}

#[derive(Debug)]
pub enum StoreError {
    /// В конфиге ошибка, править его вслепую нельзя.
    Damaged,
    Io(io::Error),
}

/// Записывает в конфиг параметры, которые отличаются от `before`. Остальное в нём,
/// включая комментарии, не меняется.
pub fn store(path: &Path, before: &Values, after: &Values) -> Result<(), StoreError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(StoreError::Io(e)),
    };
    let new = patched(&text, before, after).ok_or(StoreError::Damaged)?;
    create(path, &new).map_err(StoreError::Io)
}

/// Создаёт или заменяет файл вместе с каталогами до него.
pub fn create(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }
    file::write(path, text.as_bytes())
}

fn patched(text: &str, before: &Values, after: &Values) -> Option<String> {
    let (bom, body) = match text.strip_prefix(BOM) {
        Some(body) => (true, body),
        None => (false, text),
    };
    let mut document: DocumentMut = body.parse().ok()?;
    let pairs = before.named().into_iter().zip(after.named());
    for ((_, _, old), (table, key, new)) in pairs {
        if old.to_string() == new.to_string() {
            continue;
        }
        let place: &mut dyn toml_edit::TableLike = match table {
            None => document.as_table_mut(),
            Some(name) => document
                .entry(name)
                .or_insert_with(toml_edit::table)
                .as_table_like_mut()?,
        };
        match place.get_mut(key).and_then(toml_edit::Item::as_value_mut) {
            Some(value) => {
                // Пробелы и комментарий вокруг значения остаются.
                let decor = value.decor().clone();
                *value = new;
                *value.decor_mut() = decor;
            }
            None => {
                place.insert(key, toml_edit::Item::Value(new));
            }
        }
    }
    let mut out = document.to_string();
    if body.contains("\r\n") {
        out = out.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    if bom {
        out.insert(0, BOM);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    fn defaults() -> Values {
        Values::of(&Config::default())
    }

    fn themes() -> Vec<String> {
        vec!["dark".into(), "light".into(), "my".into()]
    }

    /// Значения, в которых всё отличается от исходных.
    fn other() -> Values {
        let mut values = defaults();
        for item in Item::ALL {
            values.step(item, true, &themes());
        }
        values
    }

    #[test]
    fn every_change_is_read_back() {
        let text = patched("[themes.my]\nbase = \"light\"\n", &defaults(), &other()).unwrap();
        let (config, warnings) = config::parse(&text);
        assert_eq!(warnings, [], "{text}");
        assert_eq!(Values::of(&config), other());
        let all = defaults().named().into_iter().zip(other().named());
        for ((_, key, old), (_, _, new)) in all {
            assert_ne!(old.to_string(), new.to_string(), "{key}");
        }
        let back = patched(&text, &other(), &defaults()).unwrap();
        assert_eq!(Values::of(&config::parse(&back).0), defaults());
    }

    #[test]
    fn values_turn_around_and_stay_valid() {
        for item in Item::ALL {
            let (mut values, mut seen) = (defaults(), 0);
            loop {
                values.step(item, true, &themes());
                seen += 1;
                let text = patched("[themes.my]\n", &defaults(), &values).unwrap();
                assert_eq!(config::parse(&text).1, [], "{item:?}: {text}");
                if values == defaults() {
                    break;
                }
                assert!(seen < 100, "{item:?} does not come back");
            }
            for _ in 0..seen {
                values.step(item, false, &themes());
            }
            assert_eq!(values, defaults(), "{item:?}");
        }
    }

    #[test]
    fn unusual_values_move_to_the_nearest() {
        assert_eq!(neighbour(&[0, 20, 30], 25, true), 30);
        assert_eq!(neighbour(&[0, 20, 30], 25, false), 20);
        assert_eq!(neighbour(&[0, 20, 30], 99, true), 0);
        assert_eq!(neighbour(&[0, 20, 30], 99, false), 30);
        assert_eq!(turn(&["a", "b"], &"gone", true), "a");
    }

    #[test]
    fn the_rest_of_the_file_is_kept() {
        let text = "# моё\ntheme = \"dark\"   # тёмная\n\n[keys]\nsave = [\"f2\"] # клавиша\n\
                    [editor]\nwrap   =  true # перенос\nunknown = 1\n";
        let mut after = defaults();
        after.wrap = false;
        after.theme = "light".into();
        after.backup = false;
        let new = patched(text, &defaults(), &after).unwrap();
        let expected = text
            .replace("\"dark\"", "\"light\"")
            .replace("true", "false")
            + "\n[save]\nbackup = false\n";
        assert_eq!(new, expected);
        assert_eq!(patched(text, &after, &after).unwrap(), text);
    }

    #[test]
    fn line_ends_bom_and_inline_tables() {
        let mut after = defaults();
        after.wrap = false;
        after.backup = false;
        let text = "\u{feff}editor = { wrap = true }\r\n[save]\r\nbackup = true\r\n";
        let new = patched(text, &defaults(), &after).unwrap();
        assert_eq!(new, text.replace("true", "false"));
        let empty = patched("", &defaults(), &after).unwrap();
        assert!(!config::parse(&empty).0.editor.wrap);
        assert!(!config::parse(&empty).0.backup);
    }

    #[test]
    fn damaged_config_is_not_touched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for bad in ["[editor\n", "editor = 5\n"] {
            fs::write(&path, bad).unwrap();
            let result = store(&path, &defaults(), &other());
            assert!(matches!(result, Err(StoreError::Damaged)));
            assert_eq!(fs::read_to_string(&path).unwrap(), bad);
        }
        fs::write(&path, b"\xff").unwrap();
        let result = store(&path, &defaults(), &other());
        assert!(matches!(result, Err(StoreError::Io(_))));
    }

    #[test]
    fn file_and_folders_are_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/config.toml");
        let mut after = defaults();
        after.tab_width = 2;
        store(&path, &defaults(), &after).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.trim(), "[editor]\ntab_width = 2");
    }
}
