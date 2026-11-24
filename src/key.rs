//! Собственное представление клавиш, не зависящее от терминальной библиотеки.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Insert,
    Esc,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: false,
    };

    pub fn is_none(self) -> bool {
        self == Self::NONE
    }
}

/// Нажатие с модификаторами.
///
/// Символ без Ctrl и Alt хранится как введён. С Ctrl или Alt это латинская
/// строчная буква, см. [`Chord::shortcut`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: Key,
    pub mods: Mods,
}

impl Chord {
    pub fn new(key: Key, mods: Mods) -> Self {
        Self { key, mods }
    }

    pub fn plain(key: Key) -> Self {
        Self::new(key, Mods::NONE)
    }

    /// Сочетание Ctrl/Alt + символ, приведённое к латинской раскладке.
    pub fn shortcut(c: char, mods: Mods) -> Self {
        Self::new(Key::Char(latin_by_position(c)), mods)
    }

    /// Символ, который следует вставить в текст.
    pub fn text(self) -> Option<char> {
        match self.key {
            Key::Char(c) if self.mods.is_none() => Some(c),
            _ => None,
        }
    }
}

/// Латинский символ на той же клавише (ЙЦУКЕН -> QWERTY), строчный.
pub fn latin_by_position(c: char) -> char {
    const CYRILLIC: &str = "йцукенгшщзхъфывапролджэячсмитьбюёіїєў";
    const LATIN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.`s]'o";
    let lower = c.to_lowercase().next().unwrap_or(c);
    CYRILLIC
        .chars()
        .zip(LATIN.chars())
        .find_map(|(cyr, lat)| (cyr == lower).then_some(lat))
        .unwrap_or(lower)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseChordError {
    Empty,
    UnknownKey(String),
    UnknownModifier(String),
    /// Терминал может не отличить сочетание от другой клавиши.
    Ambiguous(String),
    /// Обычный символ без Ctrl или Alt нужен для ввода текста.
    PlainText(String),
}

impl fmt::Display for ParseChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty key"),
            Self::UnknownKey(k) => write!(f, "unknown key: {k}"),
            Self::UnknownModifier(m) => write!(f, "unknown modifier: {m}"),
            Self::Ambiguous(k) => write!(f, "{k} is indistinguishable from another key"),
            Self::PlainText(k) => write!(f, "{k} is a text key, add ctrl or alt"),
        }
    }
}

impl std::error::Error for ParseChordError {}

impl FromStr for Chord {
    type Err = ParseChordError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let (name, modifiers) = parts.split_last().ok_or(ParseChordError::Empty)?;
        let mut mods = Mods::NONE;
        for m in modifiers {
            match m.to_lowercase().as_str() {
                "ctrl" => mods.ctrl = true,
                "alt" => mods.alt = true,
                "shift" => mods.shift = true,
                _ => return Err(ParseChordError::UnknownModifier((*m).into())),
            }
        }
        let lower = name.to_lowercase();
        let key = match lower.as_str() {
            "" => return Err(ParseChordError::Empty),
            "enter" => Key::Enter,
            "tab" => Key::Tab,
            "backspace" => Key::Backspace,
            "delete" => Key::Delete,
            "insert" => Key::Insert,
            "esc" => Key::Esc,
            "left" => Key::Left,
            "right" => Key::Right,
            "up" => Key::Up,
            "down" => Key::Down,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "space" => Key::Char(' '),
            other => {
                let mut chars = other.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Key::Char(latin_by_position(c)),
                    _ => other
                        .strip_prefix('f')
                        .and_then(|n| n.parse().ok())
                        .filter(|n| (1..=12).contains(n))
                        .map(Key::F)
                        .ok_or_else(|| ParseChordError::UnknownKey((*name).into()))?,
                }
            }
        };
        if let Key::Char(c) = key {
            if !mods.ctrl && !mods.alt && c != ' ' {
                return Err(ParseChordError::PlainText(s.into()));
            }
            // Ctrl+M = Enter, Ctrl+I = Tab, Ctrl+H = Backspace, Ctrl+[ = Esc.
            if mods.ctrl && matches!(c, 'm' | 'i' | 'h' | '[') {
                return Err(ParseChordError::Ambiguous(s.into()));
            }
        }
        Ok(Chord::new(key, mods))
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.mods.alt {
            f.write_str("Alt+")?;
        }
        if self.mods.shift {
            f.write_str("Shift+")?;
        }
        match self.key {
            Key::Char(' ') => f.write_str("Space"),
            Key::Char(c) if self.mods.is_none() => write!(f, "{c}"),
            Key::Char(c) => write!(f, "{}", c.to_uppercase()),
            Key::F(n) => write!(f, "F{n}"),
            Key::Delete => f.write_str("Del"),
            Key::Insert => f.write_str("Ins"),
            Key::PageUp => f.write_str("PgUp"),
            Key::PageDown => f.write_str("PgDn"),
            key => write!(f, "{key:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_table_is_aligned() {
        for (cyr, lat) in [
            ('й', 'q'),
            ('ы', 's'),
            ('у', 'e'),
            ('я', 'z'),
            ('с', 'c'),
            ('м', 'v'),
            ('ю', '.'),
            ('ё', '`'),
            ('і', 's'),
            ('ў', 'o'),
        ] {
            assert_eq!(latin_by_position(cyr), lat, "{cyr}");
        }
        assert_eq!(latin_by_position('Ы'), 's');
        assert_eq!(latin_by_position('S'), 's');
        assert_eq!(latin_by_position('7'), '7');
    }

    #[test]
    fn parses_and_prints() {
        for (src, shown) in [
            ("ctrl+e", "Ctrl+E"),
            ("F5", "F5"),
            ("shift+f3", "Shift+F3"),
            ("alt+left", "Alt+Left"),
            ("Ctrl+Ы", "Ctrl+S"),
            ("space", "Space"),
        ] {
            assert_eq!(src.parse::<Chord>().unwrap().to_string(), shown);
        }
    }

    #[test]
    fn rejects_bad_bindings() {
        use ParseChordError::*;
        for s in ["ctrl+m", "ctrl+i", "ctrl+h", "ctrl+ь"] {
            assert_eq!(s.parse::<Chord>(), Err(Ambiguous(s.into())), "{s}");
        }
        assert_eq!("a".parse::<Chord>(), Err(PlainText("a".into())));
        assert_eq!("".parse::<Chord>(), Err(Empty));
        assert_eq!("ctrl+".parse::<Chord>(), Err(Empty));
        assert_eq!("f13".parse::<Chord>(), Err(UnknownKey("f13".into())));
        assert_eq!(
            "hyper+a".parse::<Chord>(),
            Err(UnknownModifier("hyper".into()))
        );
    }
}
