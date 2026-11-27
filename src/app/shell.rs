//! Консоль внизу экрана: открытие, ввод, мышь, граница с документом.

use std::sync::Arc;
use std::time::Duration;

use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use super::{App, TextArea};
use crate::command::Command;
use crate::console::{self, Console};
use crate::key::Chord;

/// Строк прокрутки консоли на один шаг колеса.
const WHEEL_LINES: isize = 3;
/// Доля консоли, проценты.
pub const HEIGHT_MIN: u16 = 10;
pub const HEIGHT_MAX: u16 = 90;

/// Чем будить основной цикл, когда консоль что-то вывела.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Место консоли: граница с документом и сама консоль.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Split {
    /// Вся рабочая область, документ и консоль вместе.
    pub whole: TextArea,
    pub divider: u16,
    pub console: TextArea,
}

impl App {
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    pub fn has_console(&self) -> bool {
        self.console.is_some()
    }

    pub fn console(&self) -> Option<&Console> {
        self.console.as_ref()
    }

    pub fn console_focused(&self) -> bool {
        self.console.is_some() && self.console_focus
    }

    /// Доля консоли в рабочей области, проценты.
    pub fn console_height(&self) -> u16 {
        self.values.console_height
    }

    /// F7: открыть консоль, перейти в неё, вернуться к документу.
    pub(super) fn toggle_console(&mut self) {
        if self.console.is_some() {
            self.console_focus = !self.console_focus;
            if self.console_focus {
                self.leave_files();
            }
            return;
        }
        let shown = self.new_file_dir();
        let dir = console::start_dir(self.path.as_deref(), shown.as_deref());
        let size = self.split.console;
        let (rows, cols) = (size.height.max(10), size.width.max(40));
        let waker = self.waker.clone();
        let wake = move || {
            if let Some(waker) = &waker {
                waker();
            }
        };
        match Console::start(&self.values.console_shell, &dir, rows, cols, wake) {
            Ok(console) => {
                self.console = Some(console);
                self.console_focus = true;
                self.leave_files();
            }
            Err(e) => {
                let s = self.lang.strings();
                self.message = Some(format!("{}: {e}", s.console_failed));
            }
        }
    }

    pub(super) fn close_console(&mut self) {
        self.console = None;
        self.console_focus = false;
        self.split_drag = None;
    }

    /// Клавиша, пока ввод в консоли. Себе оставляются только клавиши консоли.
    pub(super) fn console_key(&mut self, chord: Chord, command: Option<Command>) {
        match command {
            Some(Command::Console) => self.console_focus = false,
            Some(Command::CloseConsole) => self.close_console(),
            _ => {
                if let Some(console) = &mut self.console {
                    console.key(chord);
                }
            }
        }
    }

    pub(super) fn console_paste(&mut self, text: &str) {
        if let Some(console) = &mut self.console {
            console.paste(text);
        }
    }

    /// Закрывает консоль, оболочка которой завершилась.
    pub(super) fn watch_console(&mut self) {
        if self.console.as_mut().is_some_and(Console::finished) {
            self.close_console();
            self.message = Some(self.lang.strings().console_closed.into());
        }
    }

    /// Вызывается перед отрисовкой: где консоль. Без места: её не видно.
    pub fn prepare_console(&mut self, split: Option<Split>) {
        self.split = split.unwrap_or_default();
        if let (Some(console), Some(split)) = (&mut self.console, split) {
            console.resize(split.console.height, split.console.width);
        }
    }

    /// Мышь над консолью или её границей. `true`: событие её.
    pub(super) fn console_mouse(&mut self, mouse: MouseEvent) -> bool {
        if self.console.is_none() || self.dialog.is_some() || self.help {
            return false;
        }
        let split = self.split;
        let c = split.console;
        let in_console = (c.y..c.y + c.height).contains(&mouse.row)
            && (c.x..c.x + c.width).contains(&mouse.column);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if mouse.row == split.divider => {
                self.split_drag = Some(self.values.clone());
                true
            }
            MouseEventKind::Drag(MouseButton::Left) if self.split_drag.is_some() => {
                let whole = split.whole;
                let below = (whole.y + whole.height).saturating_sub(mouse.row);
                let share = u32::from(below) * 100 / u32::from(whole.height.max(1));
                let share = (share as u16).clamp(HEIGHT_MIN, HEIGHT_MAX);
                self.values.console_height = share;
                true
            }
            MouseEventKind::Up(MouseButton::Left) if self.split_drag.is_some() => {
                let before = self.split_drag.take().expect("checked above");
                if before != self.values
                    && let Err(text) = self.store_settings(&before)
                {
                    self.message = Some(text);
                }
                true
            }
            MouseEventKind::Down(MouseButton::Left) if in_console => {
                self.console_focus = true;
                self.leave_files();
                true
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if in_console => {
                let up = mouse.kind == MouseEventKind::ScrollUp;
                if let Some(console) = &mut self.console {
                    console.scroll(if up { -WHEEL_LINES } else { WHEEL_LINES });
                }
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // Нажатие в документе возвращает ввод ему.
                self.console_focus = false;
                false
            }
            _ => false,
        }
    }
}

/// Сколько ждать вывода консоли между отрисовками, когда она открыта.
pub const CONSOLE_SLEEP: Duration = Duration::from_millis(50);
