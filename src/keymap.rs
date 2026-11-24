//! Соответствие сочетаний клавиш командам.

use std::collections::HashMap;

use crate::command::{Command, Mode};
use crate::key::Chord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Scope {
    Any,
    Only(Mode),
}

#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: HashMap<(Scope, Chord), Command>,
}

/// Клавиши по умолчанию, раздел 5 спецификации. Порядок важен для подсказок.
const DEFAULTS: &[(Command, &[&str])] = &[
    (Command::ToggleMode, &["f5", "ctrl+e"]),
    (Command::Save, &["ctrl+s", "f2"]),
    (Command::SaveAs, &["f12"]),
    (Command::Quit, &["ctrl+q", "f10"]),
    (Command::Undo, &["ctrl+z"]),
    (Command::Redo, &["ctrl+y"]),
    (Command::Copy, &["ctrl+c"]),
    (Command::Cut, &["ctrl+x"]),
    (Command::Paste, &["ctrl+v"]),
    (Command::SelectAll, &["ctrl+a"]),
    (Command::Find, &["ctrl+f"]),
    (Command::FindNext, &["f3"]),
    (Command::FindPrev, &["shift+f3"]),
    (Command::Replace, &["ctrl+r"]),
    (Command::GotoLine, &["ctrl+g"]),
    (Command::Outline, &["ctrl+t"]),
    (Command::Bold, &["ctrl+b"]),
    (Command::Italic, &["alt+i"]),
    (Command::Link, &["ctrl+k"]),
    (Command::Encoding, &["f8"]),
    (Command::Help, &["f1"]),
    (Command::NextItem, &["tab"]),
    (Command::PrevItem, &["shift+tab"]),
    (Command::OpenItem, &["enter"]),
    (Command::ToggleCheckbox, &["space"]),
    (Command::Back, &["alt+left"]),
];

fn scope_of(command: Command) -> Scope {
    use Command::*;
    match command {
        NextItem | PrevItem | OpenItem | ToggleCheckbox | Back => Scope::Only(Mode::View),
        Undo | Redo | Cut | Paste | Replace | Bold | Italic | Link => Scope::Only(Mode::Edit),
        _ => Scope::Any,
    }
}

impl Default for Keymap {
    fn default() -> Self {
        let mut map = Self {
            bindings: HashMap::new(),
        };
        for (command, chords) in DEFAULTS {
            for chord in *chords {
                let chord = chord.parse().expect("default bindings are valid");
                map.bind(chord, *command);
            }
        }
        map
    }
}

impl Keymap {
    /// Назначает сочетание команде. Возвращает команду, у которой оно отобрано.
    pub fn bind(&mut self, chord: Chord, command: Command) -> Option<Command> {
        self.bindings.insert((scope_of(command), chord), command)
    }

    pub fn lookup(&self, mode: Mode, chord: Chord) -> Option<Command> {
        self.bindings
            .get(&(Scope::Only(mode), chord))
            .or_else(|| self.bindings.get(&(Scope::Any, chord)))
            .copied()
    }

    /// Сочетания команды: F-клавиши первыми, затем по алфавиту.
    pub fn chords(&self, command: Command) -> Vec<String> {
        let mut chords: Vec<String> = self
            .bindings
            .iter()
            .filter(|(_, c)| **c == command)
            .map(|((_, chord), _)| chord.to_string())
            .collect();
        chords.sort_by_key(|s| (!s.starts_with('F'), s.clone()));
        chords
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::{Key, Mods};

    #[test]
    fn defaults_cover_every_command_without_conflicts() {
        let total: usize = DEFAULTS.iter().map(|(_, c)| c.len()).sum();
        let map = Keymap::default();
        assert_eq!(map.bindings.len(), total, "duplicate default binding");
        for command in Command::ALL {
            assert!(!map.chords(*command).is_empty(), "{}", command.name());
            assert_eq!(Command::from_name(command.name()), Some(*command));
        }
    }

    #[test]
    fn russian_layout_reaches_the_same_command() {
        let map = Keymap::default();
        let ctrl = Mods {
            ctrl: true,
            ..Mods::NONE
        };
        let chord = Chord::shortcut('у', ctrl);
        assert_eq!(map.lookup(Mode::Edit, chord), Some(Command::ToggleMode));
        assert_eq!(map.lookup(Mode::View, chord), Some(Command::ToggleMode));
    }

    #[test]
    fn view_keys_do_not_steal_typing() {
        let map = Keymap::default();
        let space = Chord::plain(Key::Char(' '));
        assert_eq!(map.lookup(Mode::Edit, space), None);
        assert_eq!(map.lookup(Mode::View, space), Some(Command::ToggleCheckbox));
        assert_eq!(map.lookup(Mode::Edit, Chord::plain(Key::Enter)), None);
    }

    #[test]
    fn toggle_lists_function_key_first() {
        assert_eq!(
            Keymap::default().chords(Command::ToggleMode),
            ["F5", "Ctrl+E"]
        );
    }
}
