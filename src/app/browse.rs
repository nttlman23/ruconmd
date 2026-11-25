//! Панель файлов: показ, клавиши, мышь, открытие выбранного.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{App, TextArea};
use crate::command::Command;
use crate::dialog::{Location, Spot};
use crate::files::Tree;
use crate::key::{Chord, Key};

/// Как часто искать изменения в показанных каталогах.
const REFRESH: Duration = Duration::from_secs(2);
/// Меньше этого тексту рядом с панелью не оставляется.
const MIN_TEXT: u16 = 30;

pub struct Browser {
    pub tree: Tree,
    pub shown: bool,
    /// Клавиши получает панель, а не текст.
    focused: bool,
    next_refresh: Instant,
    /// Где панель на экране.
    area: TextArea,
}

/// Место панели в рабочей области.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelPlace {
    Hidden,
    /// Слева от текста, столько колонок.
    Side(u16),
    /// Вместо текста: экран слишком узкий.
    Full,
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_owned())
}

impl App {
    /// Показывает каталог, с которым запущена программа.
    pub(super) fn browse(&mut self, dir: &Path) {
        self.browser = Some(Browser {
            tree: Tree::new(dir, self.values.files_all),
            shown: true,
            focused: true,
            next_refresh: Instant::now() + REFRESH,
            area: TextArea::default(),
        });
    }

    pub fn files_focused(&self) -> bool {
        let browser = self.browser.as_ref();
        browser.is_some_and(|b| b.shown && b.focused)
    }

    pub(super) fn leave_files(&mut self) {
        if let Some(b) = &mut self.browser {
            b.focused = false;
        }
    }

    /// Открывает панель, переходит в неё, а из неё закрывает.
    pub(super) fn toggle_files(&mut self) {
        if self.files_focused() {
            if let Some(b) = &mut self.browser {
                (b.shown, b.focused) = (false, false);
            }
            return;
        }
        let file = self.path.as_deref().map(absolute);
        let opened = !self.browser.as_ref().is_some_and(|b| b.shown);
        if self.browser.is_none() {
            let dir = file.as_deref().and_then(Path::parent);
            let dir = dir.map_or_else(|| absolute(Path::new(".")), Path::to_owned);
            self.browse(&dir);
        }
        if let Some(b) = &mut self.browser {
            (b.shown, b.focused) = (true, true);
            b.tree.reread();
            if opened && let Some(file) = file {
                b.tree.reveal(&file);
            }
        }
    }

    /// Каталог, в котором пользователь создаёт новые файлы.
    pub(super) fn new_file_dir(&self) -> Option<PathBuf> {
        let browser = self.browser.as_ref().filter(|b| b.shown)?;
        Some(browser.tree.root().to_owned())
    }

    pub fn panel_place(&self, width: u16) -> PanelPlace {
        match &self.browser {
            Some(b) if b.shown => {
                let panel = self.values.files_width;
                if width >= panel + MIN_TEXT {
                    PanelPlace::Side(panel)
                } else if b.focused {
                    PanelPlace::Full
                } else {
                    PanelPlace::Hidden
                }
            }
            _ => PanelPlace::Hidden,
        }
    }

    /// Вызывается перед отрисовкой: где панель (без места: её нет) и где
    /// начинается текст.
    pub fn prepare_files(&mut self, area: Option<TextArea>, origin: u16, covered: bool) {
        self.origin = origin;
        self.covered = covered;
        if let Some(b) = &mut self.browser {
            b.area = area.unwrap_or_default();
            // Первая строка: имя корня.
            b.tree.set_height(b.area.height.saturating_sub(1) as usize);
        }
    }

    pub(super) fn refresh_files(&mut self, now: Instant) {
        if let Some(b) = &mut self.browser
            && b.shown
            && now >= b.next_refresh
        {
            b.next_refresh = now + REFRESH;
            b.tree.refresh();
        }
    }

    /// Клавиша, пока выбрана панель.
    pub(super) fn files_key(&mut self, chord: Chord, command: Option<Command>) {
        let Some(b) = &mut self.browser else { return };
        let tree = &mut b.tree;
        if chord.mods.is_none() {
            let page = tree.page();
            match chord.key {
                Key::Up => return tree.move_by(-1),
                Key::Down => return tree.move_by(1),
                Key::PageUp => return tree.move_by(-page),
                Key::PageDown => return tree.move_by(page),
                Key::Home => return tree.move_by(isize::MIN),
                Key::End => return tree.move_by(isize::MAX),
                Key::Left => return tree.left(),
                Key::Right => return tree.right(),
                Key::Backspace => return tree.go_up(),
                Key::Enter | Key::Char(' ') => return self.activate_file(),
                Key::Esc | Key::Tab => return self.leave_files(),
                Key::Char(c) => return tree.jump(c),
                _ => {}
            }
        }
        match command {
            Some(
                command @ (Command::ToggleMode
                | Command::Save
                | Command::SaveAs
                | Command::Quit
                | Command::Help
                | Command::Settings
                | Command::LineNumbers
                | Command::Files),
            ) => self.run(command),
            // Это про текст: сначала переходим к нему.
            Some(
                command @ (Command::Find
                | Command::FindNext
                | Command::FindPrev
                | Command::Outline
                | Command::GotoLine
                | Command::Encoding
                | Command::Back),
            ) => {
                self.leave_files();
                self.run(command);
            }
            _ => {}
        }
    }

    fn activate_file(&mut self) {
        let Some(b) = &mut self.browser else { return };
        let Some(path) = b.tree.activate() else {
            return;
        };
        if self.is_here(&Some(path.clone())) {
            return self.leave_files();
        }
        self.go(Location {
            path: Some(path),
            spot: Spot::Start,
            back: false,
        });
    }

    /// Нажатие мыши. `true`: оно пришлось на панель.
    pub(super) fn files_click(&mut self, column: u16, row: u16) -> bool {
        let Some(y) = self.files_row(column, row) else {
            return false;
        };
        let Some(b) = &mut self.browser else {
            return false;
        };
        b.focused = true;
        // Первая строка: имя корня.
        if y > 0 && b.tree.click(y - 1) {
            self.message = None;
            self.activate_file();
        }
        true
    }

    pub(super) fn files_wheel(&mut self, column: u16, row: u16, delta: isize) -> bool {
        if self.files_row(column, row).is_none() {
            return false;
        }
        if let Some(b) = &mut self.browser {
            b.tree.scroll(delta);
        }
        true
    }

    /// Строка панели под ячейкой экрана.
    fn files_row(&self, column: u16, row: u16) -> Option<usize> {
        let b = self.browser.as_ref().filter(|b| b.shown)?;
        let a = b.area;
        let inside = (a.x..a.x + a.width).contains(&column) && (a.y..a.y + a.height).contains(&row);
        inside.then(|| (row - a.y) as usize)
    }
}
