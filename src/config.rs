//! Файл конфигурации.
//!
//! Ошибка в одном параметре не отменяет остальные: вместо него берётся значение
//! по умолчанию, а пользователю показывается предупреждение.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use toml::{Table, Value};

use crate::command::Command;
use crate::graphics::Wanted;
use crate::i18n::Lang;
use crate::key::Chord;
use crate::keymap::{BindError, Keymap};
use crate::style::{Depth, Theme, parse_color};
use crate::view::Settings;

#[derive(Debug, Clone)]
pub struct Config {
    /// `None`: по локали системы.
    pub lang: Option<Lang>,
    pub theme: Theme,
    pub editor: Settings,
    /// Наибольшая ширина просмотра, 0: вся ширина окна.
    pub preview_width: usize,
    pub images: Wanted,
    pub backup: bool,
    /// Через сколько после правки сохранять файл.
    pub autosave: Option<Duration>,
    pub keymap: Keymap,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            lang: None,
            theme: Theme::DARK,
            editor: Settings::default(),
            preview_width: 100,
            images: Wanted::Auto,
            backup: true,
            autosave: None,
            keymap: Keymap::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    Unreadable(String),
    Syntax(String),
    Unknown,
    BadValue,
    BadKey(String),
    /// Клавиша уже назначена другой команде из конфига.
    Taken(&'static str),
    /// Команда осталась без клавиш, возвращены клавиши по умолчанию.
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// Параметр или путь к файлу.
    pub place: String,
    pub problem: Problem,
}

/// Читает конфиг. Отсутствие файла по пути по умолчанию не ошибка.
pub fn load(explicit: Option<&Path>) -> (Config, Vec<Warning>) {
    let Some(path) = explicit.map(Path::to_owned).or_else(default_path) else {
        return (Config::default(), Vec::new());
    };
    match fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == io::ErrorKind::NotFound && explicit.is_none() => {
            (Config::default(), Vec::new())
        }
        Err(e) => {
            let warning = Warning {
                place: path.display().to_string(),
                problem: Problem::Unreadable(e.to_string()),
            };
            (Config::default(), vec![warning])
        }
    }
}

/// Абсолютный путь из переменной окружения. Относительные XDG велит не учитывать.
fn env_dir(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os(name)?);
    path.is_absolute().then_some(path)
}

pub fn default_path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        env_dir("APPDATA")
    } else {
        env_dir("XDG_CONFIG_HOME").or_else(|| Some(env_dir("HOME")?.join(".config")))
    };
    Some(base?.join("ruconmd").join("config.toml"))
}

/// Каталог для файлов восстановления.
pub fn state_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        env_dir("LOCALAPPDATA")
    } else {
        env_dir("XDG_STATE_HOME").or_else(|| Some(env_dir("HOME")?.join(".local/state")))
    };
    Some(base?.join("ruconmd").join("recovery"))
}

pub fn parse(text: &str) -> (Config, Vec<Warning>) {
    let mut reader = Reader {
        config: Config::default(),
        warnings: Vec::new(),
    };
    match text.trim_start_matches('\u{feff}').parse::<Table>() {
        Ok(table) => reader.root(&table),
        Err(e) => {
            let line = e.span().map_or(1, |s| {
                let before = text.get(..s.start).unwrap_or_default();
                before.matches('\n').count() + 1
            });
            let message = e.message().lines().next().unwrap_or_default().to_owned();
            reader.warn(format!("config.toml:{line}"), Problem::Syntax(message));
        }
    }
    (reader.config, reader.warnings)
}

struct Reader {
    config: Config,
    warnings: Vec<Warning>,
}

impl Reader {
    fn warn(&mut self, place: impl Into<String>, problem: Problem) {
        self.warnings.push(Warning {
            place: place.into(),
            problem,
        });
    }

    /// Записывает значение или предупреждает, что оно не подходит.
    fn set<T>(&mut self, place: &str, value: Option<T>, put: impl FnOnce(&mut Config, T)) {
        match value {
            Some(value) => put(&mut self.config, value),
            None => self.warn(place, Problem::BadValue),
        }
    }

    fn root(&mut self, table: &Table) {
        for (key, value) in table {
            match (key.as_str(), value) {
                ("language", v) => {
                    let lang = match v.as_str() {
                        Some("auto") => Some(None),
                        Some("ru") => Some(Some(Lang::Ru)),
                        Some("en") => Some(Some(Lang::En)),
                        _ => None,
                    };
                    self.set(key, lang, |c, l| c.lang = l);
                }
                ("theme", _) | ("themes", Value::Table(_)) => {}
                ("editor", Value::Table(t)) => self.editor(t),
                ("preview", Value::Table(t)) => self.preview(t),
                ("save", Value::Table(t)) => self.save(t),
                ("keys", Value::Table(t)) => self.keys(t),
                ("editor" | "preview" | "save" | "keys" | "themes", _) => {
                    self.warn(key, Problem::BadValue);
                }
                _ => self.warn(key, Problem::Unknown),
            }
        }
        self.theme(table);
    }

    fn editor(&mut self, table: &Table) {
        for (key, v) in table {
            let place = format!("editor.{key}");
            match key.as_str() {
                "tab_width" => {
                    self.set(&place, number(v, 1, 16), |c, n| c.editor.tab_width = n);
                }
                "wrap" => self.set(&place, v.as_bool(), |c, b| c.editor.wrap = b),
                "line_numbers" => {
                    self.set(&place, v.as_bool(), |c, b| c.editor.line_numbers = b);
                }
                _ => self.warn(place, Problem::Unknown),
            }
        }
    }

    fn preview(&mut self, table: &Table) {
        for (key, v) in table {
            let place = format!("preview.{key}");
            match key.as_str() {
                "max_width" => {
                    let width = number(v, 0, 1000).filter(|n| *n == 0 || *n >= 20);
                    self.set(&place, width, |c, n| c.preview_width = n);
                }
                "images" => {
                    let wanted = match v {
                        Value::Boolean(true) => Some(Wanted::Auto),
                        Value::Boolean(false) => Some(Wanted::Off),
                        other => other.as_str().and_then(Wanted::from_name),
                    };
                    self.set(&place, wanted, |c, w| c.images = w);
                }
                _ => self.warn(place, Problem::Unknown),
            }
        }
    }

    fn save(&mut self, table: &Table) {
        let mut autosave = false;
        let mut delay = 5;
        for (key, v) in table {
            let place = format!("save.{key}");
            match key.as_str() {
                "backup" => self.set(&place, v.as_bool(), |c, b| c.backup = b),
                "autosave" => self.set(&place, v.as_bool(), |_, b| autosave = b),
                "autosave_delay_sec" => {
                    self.set(&place, number(v, 1, 3600), |_, n| delay = n as u64);
                }
                _ => self.warn(place, Problem::Unknown),
            }
        }
        self.config.autosave = autosave.then(|| Duration::from_secs(delay));
    }

    fn keys(&mut self, table: &Table) {
        let mut keymap = Keymap::default();
        let mut wanted = Vec::new();
        for (name, value) in table {
            let place = format!("keys.{name}");
            let Some(command) = Command::from_name(name) else {
                self.warn(place, Problem::Unknown);
                continue;
            };
            let Some(list) = value.as_array() else {
                self.warn(place, Problem::BadValue);
                continue;
            };
            keymap.unbind(command);
            for item in list {
                match item.as_str().map(str::parse::<Chord>) {
                    Some(Ok(chord)) => wanted.push((place.clone(), command, chord)),
                    Some(Err(e)) => self.warn(&place, Problem::BadKey(e.to_string())),
                    None => self.warn(&place, Problem::BadValue),
                }
            }
        }
        // Клавиши из конфига отбираются у команд по умолчанию, но не друг у друга.
        let mut given: Vec<(Chord, Command)> = Vec::new();
        for (place, command, chord) in wanted {
            let taken = given.iter().find(|(c, other)| {
                *c == chord && *other != command && keymap.overlap(*other, command)
            });
            if let Some((_, other)) = taken {
                self.warn(place, Problem::Taken(other.name()));
                continue;
            }
            match keymap.bind(chord, command) {
                Ok(()) => given.push((chord, command)),
                Err(BindError::NeededForTyping) => {
                    self.warn(
                        place,
                        Problem::BadKey(format!("{chord}: needed for typing")),
                    );
                }
            }
        }
        // Без этих команд из программы не выйти и режим не сменить.
        for command in [Command::ToggleMode, Command::Quit] {
            if keymap.chords(command).is_empty() {
                keymap.restore_defaults(command);
                self.warn(format!("keys.{}", command.name()), Problem::Required);
            }
        }
        self.config.keymap = keymap;
    }

    fn theme(&mut self, root: &Table) {
        let name = match root.get("theme") {
            None => return self.check_themes(root),
            Some(v) => v.as_str(),
        };
        let custom = root.get("themes").and_then(Value::as_table);
        match name {
            Some("dark") => self.config.theme = Theme::DARK,
            Some("light") => self.config.theme = Theme::LIGHT,
            Some(name) => match custom.and_then(|t| t.get(name)).and_then(Value::as_table) {
                Some(table) => self.config.theme = custom_theme(name, table).0,
                None => self.warn("theme", Problem::BadValue),
            },
            None => self.warn("theme", Problem::BadValue),
        }
        self.check_themes(root);
    }

    /// Предупреждает об ошибках во всех темах, а не только в выбранной.
    fn check_themes(&mut self, root: &Table) {
        let Some(themes) = root.get("themes").and_then(Value::as_table) else {
            return;
        };
        for (name, value) in themes {
            match value.as_table() {
                Some(table) => self.warnings.extend(custom_theme(name, table).1),
                None => self.warn(format!("themes.{name}"), Problem::BadValue),
            }
        }
    }
}

fn custom_theme(name: &str, table: &Table) -> (Theme, Vec<Warning>) {
    let mut warnings = Vec::new();
    let mut warn = |key: &str, problem| {
        warnings.push(Warning {
            place: format!("themes.{name}.{key}"),
            problem,
        });
    };
    let mut theme = match table.get("base").map(Value::as_str) {
        None | Some(Some("dark")) => Theme::DARK,
        Some(Some("light")) => Theme::LIGHT,
        Some(_) => {
            warn("base", Problem::BadValue);
            Theme::DARK
        }
    };
    for (key, value) in table.iter().filter(|(k, _)| *k != "base") {
        let color = match value {
            Value::Integer(i) => u8::try_from(*i).ok().map(ratatui::style::Color::Indexed),
            other => other.as_str().and_then(parse_color),
        };
        match color {
            Some(color) if theme.set(key, color) => {}
            Some(_) => warn(key, Problem::Unknown),
            None => warn(key, Problem::BadValue),
        }
    }
    (theme, warnings)
}

fn number(value: &Value, min: usize, max: usize) -> Option<usize> {
    let n = usize::try_from(value.as_integer()?).ok()?;
    (min..=max).contains(&n).then_some(n)
}

impl Config {
    pub fn fitted(mut self, depth: Depth) -> Self {
        self.theme = self.theme.fitted(depth);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Mode;

    const EXAMPLE: &str = include_str!("../config.example.toml");

    fn problems(text: &str) -> Vec<(String, Problem)> {
        let (_, warnings) = parse(text);
        warnings.into_iter().map(|w| (w.place, w.problem)).collect()
    }

    fn lookup(config: &Config, chord: &str) -> Option<Command> {
        config.keymap.lookup(Mode::Edit, chord.parse().unwrap())
    }

    #[test]
    fn example_is_valid_and_lists_the_defaults() {
        let (config, warnings) = parse(EXAMPLE);
        assert_eq!(warnings, []);
        let default = Config::default();
        assert_eq!(config.theme, default.theme);
        assert_eq!(config.editor, default.editor);
        assert_eq!(config.preview_width, default.preview_width);
        assert_eq!((config.backup, config.autosave), (true, None));
        for command in Command::ALL {
            assert!(
                EXAMPLE.contains(&format!("\n{} = [", command.name())),
                "{}",
                command.name()
            );
            assert_eq!(
                config.keymap.chords(*command),
                default.keymap.chords(*command)
            );
        }
    }

    #[test]
    fn values_are_applied() {
        let (c, warnings) = parse(
            "language = \"ru\"\ntheme = \"light\"\n[editor]\ntab_width = 2\nwrap = false\n\
             line_numbers = false\n[preview]\nmax_width = 0\nimages = false\n\
             [save]\nbackup = false\nautosave = true\nautosave_delay_sec = 30\n",
        );
        assert_eq!(warnings, []);
        assert_eq!(c.lang, Some(Lang::Ru));
        assert_eq!(c.theme, Theme::LIGHT);
        assert_eq!(
            (c.editor.tab_width, c.editor.wrap, c.editor.line_numbers),
            (2, false, false)
        );
        assert_eq!(
            (c.preview_width, c.images, c.backup),
            (0, Wanted::Off, false)
        );
        assert_eq!(c.autosave, Some(Duration::from_secs(30)));
    }

    #[test]
    fn image_protocols() {
        let images = |v: &str| parse(&format!("[preview]\nimages = {v}\n")).0.images;
        assert_eq!(images("true"), Wanted::Auto);
        assert_eq!(Some(images("\"sixel\"")), Wanted::from_name("sixel"));
        assert_eq!(problems("[preview]\nimages = \"png\"\n").len(), 1);
    }

    #[test]
    fn one_bad_value_does_not_cancel_the_rest() {
        let text = "language = \"de\"\ntypo = 1\n[editor]\ntab_width = 0\nwrap = false\n\
                    nope = 1\n[preview]\nmax_width = 5\n[save]\nbackup = \"yes\"\n";
        let (c, _) = parse(text);
        assert!(!c.editor.wrap);
        assert_eq!(
            (c.editor.tab_width, c.backup, c.preview_width),
            (4, true, 100)
        );
        let mut got = problems(text);
        got.sort_by(|a, b| a.0.cmp(&b.0));
        let want = [
            ("editor.nope", Problem::Unknown),
            ("editor.tab_width", Problem::BadValue),
            ("language", Problem::BadValue),
            ("preview.max_width", Problem::BadValue),
            ("save.backup", Problem::BadValue),
            ("typo", Problem::Unknown),
        ];
        assert_eq!(got, want.map(|(p, e)| (p.to_owned(), e)));
    }

    #[test]
    fn syntax_error_names_the_line() {
        let got = problems("theme = \"dark\"\n[editor\n");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "config.toml:2");
        assert!(matches!(got[0].1, Problem::Syntax(_)));
        assert_eq!(problems("editor = 5").len(), 1);
    }

    #[test]
    fn keys_are_replaced_and_taken_from_defaults() {
        let (c, warnings) = parse("[keys]\nsave = [\"ctrl+e\", \"f9\"]\nundo = []\n");
        assert_eq!(warnings, []);
        assert_eq!(lookup(&c, "ctrl+e"), Some(Command::Save));
        assert_eq!(lookup(&c, "f9"), Some(Command::Save));
        assert_eq!(lookup(&c, "ctrl+s"), None);
        assert_eq!(lookup(&c, "ctrl+z"), None);
        assert_eq!(lookup(&c, "f5"), Some(Command::ToggleMode));
    }

    #[test]
    fn toggle_hotkey_can_be_changed() {
        let (c, _) = parse("[keys]\ntoggle_mode = [\"alt+v\"]\n");
        assert_eq!(lookup(&c, "alt+м"), Some(Command::ToggleMode));
        assert_eq!(lookup(&c, "f5"), None);
    }

    #[test]
    fn bad_keys_are_reported() {
        let got = problems("[keys]\nsave = [\"ctrl+m\", \"q\", 5]\nfind = \"f3\"\nnope = []\n");
        assert_eq!(got.len(), 5, "{got:?}");
        let got = problems("[keys]\nsave = [\"f9\"]\nquit = [\"f9\", \"ctrl+q\"]\n");
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0].1, Problem::Taken(_)));
        for key in ["enter", "tab", "space", "up", "backspace"] {
            let got = problems(&format!("[keys]\nsave = [\"{key}\"]\n"));
            assert!(matches!(got[0].1, Problem::BadKey(_)), "{key}: {got:?}");
        }
        assert_eq!(
            problems("[keys]\nopen_item = [\"space\"]\ntoggle_checkbox = [\"enter\"]\n"),
            []
        );
        assert_eq!(
            problems("[keys]\nbold = [\"f9\"]\nback = [\"f9\"]\n"),
            [],
            "other modes"
        );
    }

    #[test]
    fn quit_and_toggle_cannot_be_lost() {
        let text = "[keys]\nquit = []\nsave = [\"f5\", \"ctrl+e\"]\n";
        let (c, warnings) = parse(text);
        assert_eq!(warnings.len(), 2);
        assert!(warnings.iter().all(|w| w.problem == Problem::Required));
        assert_eq!(lookup(&c, "ctrl+q"), Some(Command::Quit));
        assert!(!c.keymap.chords(Command::ToggleMode).is_empty());
    }

    #[test]
    fn themes() {
        let text =
            "theme = \"my\"\n[themes.my]\nbase = \"light\"\nheading = \"#7aa2f7\"\nlink = 33\n";
        let (c, warnings) = parse(text);
        assert_eq!(warnings, []);
        let mut want = Theme::LIGHT;
        want.set("heading", parse_color("#7aa2f7").unwrap());
        want.set("link", ratatui::style::Color::Indexed(33));
        assert_eq!(c.theme, want);

        let got =
            problems("theme = \"no\"\n[themes.x]\nbase = \"b\"\nzzz = \"red\"\nlink = \"?\"\n");
        assert_eq!(got.len(), 4, "{got:?}");
    }
}
