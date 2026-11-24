//! Тексты интерфейса.

use crate::command::{Command, Mode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Lang {
    Ru,
    En,
}

pub struct Strings {
    pub not_a_terminal: &'static str,
    pub untitled: &'static str,
    pub not_implemented: &'static str,
    pub text_input_later: &'static str,
    pub is_directory: &'static str,
    pub help_title: &'static str,
    pub quit: &'static str,
    pub help: &'static str,
    pub diag_title: &'static str,
    pub diag_hint: &'static str,
    pub diag_unbound: &'static str,
    pub diag_text: &'static str,
    pub diag_paste: &'static str,
    pub diag_continued: &'static str,
}

const RU: Strings = Strings {
    not_a_terminal: "нужен интерактивный терминал",
    untitled: "без имени",
    not_implemented: "пока не реализовано",
    text_input_later: "Ввод текста появится на этапе 2",
    is_directory: "это каталог, а не файл",
    help_title: " Клавиши ",
    quit: "выход",
    help: "справка",
    diag_title: " Проверка клавиш ",
    diag_hint: "Нажимайте клавиши и вставляйте текст. Esc дважды: выход и печать журнала",
    diag_unbound: "не назначено",
    diag_text: "текст",
    diag_paste: "вставка, символов",
    diag_continued: "продолжение",
};

const EN: Strings = Strings {
    not_a_terminal: "an interactive terminal is required",
    untitled: "untitled",
    not_implemented: "not implemented yet",
    text_input_later: "Text input comes in stage 2",
    is_directory: "is a directory, not a file",
    help_title: " Keys ",
    quit: "quit",
    help: "help",
    diag_title: " Key check ",
    diag_hint: "Press keys and paste text. Esc twice: exit and print the log",
    diag_unbound: "unbound",
    diag_text: "text",
    diag_paste: "paste, characters",
    diag_continued: "continued",
};

impl Lang {
    pub fn from_system() -> Self {
        Self::from_locale(sys_locale::get_locale().as_deref())
    }

    fn from_locale(locale: Option<&str>) -> Self {
        match locale {
            Some(l) if l.to_lowercase().starts_with("ru") => Self::Ru,
            _ => Self::En,
        }
    }

    pub fn strings(self) -> &'static Strings {
        match self {
            Self::Ru => &RU,
            Self::En => &EN,
        }
    }

    pub fn mode(self, mode: Mode) -> &'static str {
        match (self, mode) {
            (Self::Ru, Mode::Edit) => "ПРАВКА",
            (Self::Ru, Mode::View) => "ПРОСМОТР",
            (Self::En, Mode::Edit) => "EDIT",
            (Self::En, Mode::View) => "VIEW",
        }
    }

    pub fn command(self, command: Command) -> &'static str {
        let (ru, en) = match command {
            Command::ToggleMode => ("Правка / просмотр", "Edit / view"),
            Command::Save => ("Сохранить", "Save"),
            Command::SaveAs => ("Сохранить как", "Save as"),
            Command::Quit => ("Выход", "Quit"),
            Command::Undo => ("Отменить", "Undo"),
            Command::Redo => ("Повторить", "Redo"),
            Command::Copy => ("Копировать", "Copy"),
            Command::Cut => ("Вырезать", "Cut"),
            Command::Paste => ("Вставить", "Paste"),
            Command::SelectAll => ("Выделить всё", "Select all"),
            Command::Find => ("Найти", "Find"),
            Command::FindNext => ("Найти далее", "Find next"),
            Command::FindPrev => ("Найти ранее", "Find previous"),
            Command::Replace => ("Заменить", "Replace"),
            Command::GotoLine => ("К строке", "Go to line"),
            Command::Outline => ("Оглавление", "Outline"),
            Command::Bold => ("Жирный", "Bold"),
            Command::Italic => ("Курсив", "Italic"),
            Command::Link => ("Ссылка", "Link"),
            Command::Encoding => ("Кодировка", "Encoding"),
            Command::Help => ("Справка", "Help"),
            Command::NextItem => ("Следующая ссылка", "Next link"),
            Command::PrevItem => ("Предыдущая ссылка", "Previous link"),
            Command::OpenItem => ("Открыть ссылку", "Open link"),
            Command::ToggleCheckbox => ("Отметить задачу", "Toggle task"),
            Command::Back => ("Назад", "Back"),
        };
        match self {
            Self::Ru => ru,
            Self::En => en,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_follows_locale() {
        assert_eq!(Lang::from_locale(Some("ru-RU")), Lang::Ru);
        assert_eq!(Lang::from_locale(Some("ru_RU.UTF-8")), Lang::Ru);
        assert_eq!(Lang::from_locale(Some("en-US")), Lang::En);
        assert_eq!(Lang::from_locale(None), Lang::En);
    }
}
