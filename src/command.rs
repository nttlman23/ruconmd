//! Команды программы. Имя команды используется в секции `[keys]` конфига.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Edit,
    View,
}

impl Mode {
    pub fn toggled(self) -> Self {
        match self {
            Self::Edit => Self::View,
            Self::View => Self::Edit,
        }
    }
}

macro_rules! commands {
    ($($variant:ident = $name:literal),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Command { $($variant),* }

        impl Command {
            pub const ALL: &[Command] = &[$(Command::$variant),*];

            pub fn name(self) -> &'static str {
                match self { $(Command::$variant => $name),* }
            }

            pub fn from_name(name: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|c| c.name() == name)
            }
        }
    };
}

commands! {
    ToggleMode = "toggle_mode",
    Save = "save",
    SaveAs = "save_as",
    Quit = "quit",
    Files = "files",
    Settings = "settings",
    Undo = "undo",
    Redo = "redo",
    Copy = "copy",
    Cut = "cut",
    Paste = "paste",
    SelectAll = "select_all",
    Find = "find",
    FindNext = "find_next",
    FindPrev = "find_prev",
    Replace = "replace",
    GotoLine = "goto_line",
    Outline = "outline",
    Bold = "bold",
    Italic = "italic",
    Link = "link",
    Encoding = "encoding",
    Help = "help",
    NextItem = "next_item",
    PrevItem = "prev_item",
    OpenItem = "open_item",
    ToggleCheckbox = "toggle_checkbox",
    Back = "back",
}
