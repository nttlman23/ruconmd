//! Тексты интерфейса.

use crate::command::{Command, Mode};
use crate::config::{Problem, Warning};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Lang {
    Ru,
    En,
}

pub struct Strings {
    pub not_a_terminal: &'static str,
    pub untitled: &'static str,
    pub not_regular: &'static str,
    pub lossy_warning: &'static str,
    pub large_warning: &'static str,
    pub nothing_to_undo: &'static str,
    pub nothing_to_redo: &'static str,
    pub copied: &'static str,
    pub clipboard_empty: &'static str,
    pub ask_line: &'static str,
    pub bad_number: &'static str,
    pub ask_path: &'static str,
    pub ask_quit: &'static str,
    pub save: &'static str,
    pub dont_save: &'static str,
    pub cancel: &'static str,
    pub ask_overwrite: &'static str,
    pub overwrite: &'static str,
    pub ask_lossy: &'static str,
    pub save_anyway: &'static str,
    pub ask_unencodable: &'static str,
    pub save_utf8: &'static str,
    pub save_first: &'static str,
    pub saved: &'static str,
    pub not_saved: &'static str,
    pub line_ends: &'static str,
    pub save_in: &'static str,
    pub read_as: &'static str,
    pub preparing: &'static str,
    pub preview_failed: &'static str,
    pub no_recovery: &'static str,
    pub open_elsewhere: &'static str,
    pub found_recovery: &'static str,
    pub changed_since: &'static str,
    pub restore: &'static str,
    pub not_now: &'static str,
    pub drop_copy: &'static str,
    pub minutes_ago: &'static str,
    pub hours_ago: &'static str,
    pub days_ago: &'static str,
    pub restored: &'static str,
    pub changed_on_disk: &'static str,
    pub keep_mine: &'static str,
    pub reread: &'static str,
    pub save_as: &'static str,
    pub gone_from_disk: &'static str,
    pub no_backup: &'static str,
    pub autosave_encoding: &'static str,
    pub crashed: &'static str,
    pub outline: &'static str,
    pub no_headings: &'static str,
    pub no_links: &'static str,
    pub select_first: &'static str,
    pub opening: &'static str,
    pub cannot_open: &'static str,
    pub unsupported_link: &'static str,
    pub no_history: &'static str,
    pub no_such_file: &'static str,
    pub no_such_section: &'static str,
    pub find: &'static str,
    pub replace_what: &'static str,
    pub replace_with: &'static str,
    pub match_case: &'static str,
    pub bad_regex: &'static str,
    pub not_found: &'static str,
    pub of: &'static str,
    pub wrapped: &'static str,
    pub replaced: &'static str,
    pub replace_hint: &'static str,
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
    not_regular: "это не обычный файл",
    lossy_warning: "Файл прочитан с потерями: повреждён или выбрана не та кодировка (F8)",
    large_warning: "Файл больше 10 МБ, возможны задержки",
    nothing_to_undo: "Отменять нечего",
    nothing_to_redo: "Повторять нечего",
    copied: "Скопировано",
    clipboard_empty: "Буфер обмена пуст",
    ask_line: "Номер строки",
    bad_number: "Нужно число",
    ask_path: "Имя файла",
    ask_quit: "Есть несохранённые правки",
    save: "Сохранить",
    dont_save: "Не сохранять",
    cancel: "Отмена",
    ask_overwrite: "файл уже есть",
    overwrite: "Заменить",
    ask_lossy: "Файл прочитан с потерями, запись изменит его содержимое",
    save_anyway: "Всё равно сохранить",
    ask_unencodable: "нет символа",
    save_utf8: "Сохранить в UTF-8",
    save_first: "Сначала сохраните правки",
    saved: "Сохранено",
    not_saved: "Не сохранено",
    line_ends: "Переводы строк",
    save_in: "Сохранять в",
    read_as: "Прочитать как",
    preparing: "Готовлю просмотр…",
    preview_failed: "Просмотр построить не удалось",
    no_recovery: "Копия для восстановления не записана",
    open_elsewhere: "В другом окне есть несохранённые правки этого файла",
    found_recovery: "Найдены несохранённые правки",
    changed_since: "файл с тех пор изменился",
    restore: "Восстановить",
    not_now: "Не сейчас",
    drop_copy: "Удалить копию",
    minutes_ago: "мин назад",
    hours_ago: "ч назад",
    days_ago: "дн назад",
    restored: "Правки восстановлены, файл ещё не сохранён",
    changed_on_disk: "Файл изменён другой программой",
    keep_mine: "Оставить мою версию",
    reread: "Перечитать файл",
    save_as: "Сохранить как",
    gone_from_disk: "Файл удалён с диска",
    no_backup: "копия .bak не создана",
    autosave_encoding: "Автосохранение: есть символы вне кодировки файла",
    crashed: "Несохранённые правки записаны, программа предложит их при открытии файла",
    outline: "Оглавление",
    no_headings: "В документе нет заголовков",
    no_links: "Ссылок и задач нет",
    select_first: "Выберите ссылку или задачу клавишей Tab",
    opening: "Открываю",
    cannot_open: "Не удалось открыть",
    unsupported_link: "Такие ссылки не открываются",
    no_history: "Возвращаться некуда",
    no_such_file: "файла нет",
    no_such_section: "Такого раздела нет",
    find: "Найти",
    replace_what: "Заменить",
    replace_with: "Заменить на",
    match_case: "регистр",
    bad_regex: "Ошибка в выражении",
    not_found: "Не найдено",
    of: "из",
    wrapped: " (по кругу)",
    replaced: "Заменено",
    replace_hint: "Enter: заменить | Пробел: пропустить | A: все оставшиеся | Esc: закончить",
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
    not_regular: "is not a regular file",
    lossy_warning: "File was read with losses: damaged or wrong encoding (F8)",
    large_warning: "File is larger than 10 MB, expect delays",
    nothing_to_undo: "Nothing to undo",
    nothing_to_redo: "Nothing to redo",
    copied: "Copied",
    clipboard_empty: "Clipboard is empty",
    ask_line: "Line number",
    bad_number: "A number is required",
    ask_path: "File name",
    ask_quit: "There are unsaved changes",
    save: "Save",
    dont_save: "Don't save",
    cancel: "Cancel",
    ask_overwrite: "file already exists",
    overwrite: "Overwrite",
    ask_lossy: "File was read with losses, saving will alter it",
    save_anyway: "Save anyway",
    ask_unencodable: "cannot represent",
    save_utf8: "Save as UTF-8",
    save_first: "Save your changes first",
    saved: "Saved",
    not_saved: "Not saved",
    line_ends: "Line endings",
    save_in: "Save in",
    read_as: "Reread as",
    preparing: "Preparing preview…",
    preview_failed: "Cannot build the preview",
    no_recovery: "Recovery copy is not written",
    open_elsewhere: "Another window has unsaved changes to this file",
    found_recovery: "Unsaved changes found",
    changed_since: "the file has changed since",
    restore: "Restore",
    not_now: "Not now",
    drop_copy: "Delete the copy",
    minutes_ago: "min ago",
    hours_ago: "h ago",
    days_ago: "d ago",
    restored: "Changes restored, the file is not saved yet",
    changed_on_disk: "The file was changed by another program",
    keep_mine: "Keep my version",
    reread: "Reload the file",
    save_as: "Save as",
    gone_from_disk: "The file was deleted from disk",
    no_backup: "no .bak copy",
    autosave_encoding: "Autosave: some characters do not fit the file encoding",
    crashed: "Unsaved changes are kept and will be offered when the file is opened",
    outline: "Outline",
    no_headings: "The document has no headings",
    no_links: "No links or tasks",
    select_first: "Select a link or a task with Tab",
    opening: "Opening",
    cannot_open: "Cannot open",
    unsupported_link: "This kind of link is not opened",
    no_history: "Nowhere to go back",
    no_such_file: "no such file",
    no_such_section: "No such section",
    find: "Find",
    replace_what: "Replace",
    replace_with: "Replace with",
    match_case: "case",
    bad_regex: "Bad expression",
    not_found: "Not found",
    of: "of",
    wrapped: " (wrapped)",
    replaced: "Replaced",
    replace_hint: "Enter: replace | Space: skip | A: all remaining | Esc: finish",
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

    pub fn config_problem(self, warning: &Warning) -> String {
        let ru = self == Self::Ru;
        let pick = |a: &str, b: &str| if ru { a.to_owned() } else { b.to_owned() };
        let problem = match &warning.problem {
            Problem::Unreadable(e) | Problem::Syntax(e) | Problem::BadKey(e) => e.clone(),
            Problem::Unknown => pick("неизвестный параметр", "unknown setting"),
            Problem::BadValue => pick("недопустимое значение", "invalid value"),
            Problem::Taken(other) => {
                format!(
                    "{} {other}",
                    pick("клавиша уже у команды", "key already used by")
                )
            }
            Problem::Required => pick(
                "команде нужна клавиша, возвращены клавиши по умолчанию",
                "a key is required, defaults restored",
            ),
        };
        format!("{}: {problem}", warning.place)
    }

    /// Первое предупреждение о конфиге и число остальных.
    pub fn config_warning(self, first: &Warning, more: usize) -> String {
        let name = if self == Self::Ru {
            "Конфиг"
        } else {
            "Config"
        };
        let rest = if more > 0 {
            format!(" (+{more})")
        } else {
            String::new()
        };
        format!("{name}: {}{rest}", self.config_problem(first))
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
