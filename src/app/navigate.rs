//! Ссылки, оглавление, чекбоксы, переход между файлами.

use std::path::{Path, PathBuf};

use super::{App, Mode, load};
use crate::dialog::{Action, Dialog, Location, Menu, Spot, Then};
use crate::links::{self, Target};
use crate::preview::{self, Item};

impl App {
    /// Показывает место исходного текста в текущем режиме.
    pub(super) fn goto_src(&mut self, src: usize) {
        match self.mode {
            Mode::Edit => {
                let pos = self.buffer.pos_of_byte(src);
                self.move_to(pos, false);
                self.view.top = self.view.locate(&self.buffer, pos).0;
                self.view.clamp(&self.buffer);
            }
            Mode::View => self.pane.jump(src),
        }
    }

    pub(super) fn ask_outline(&mut self) {
        let s = self.lang.strings();
        // В обычном тексте заголовков нет.
        let plain = self.mode == Mode::View && self.pane.is_plain();
        let headings = match plain {
            true => Vec::new(),
            false => preview::headings(&self.buffer.text()),
        };
        if headings.is_empty() {
            self.message = Some(s.no_headings.into());
            return;
        }
        let here = match self.mode {
            Mode::Edit => self.buffer.byte_of(self.buffer.cursor()),
            Mode::View => self.pane.source_at_top(),
        };
        let current = headings
            .partition_point(|h| h.src <= here)
            .saturating_sub(1);
        let items = headings
            .into_iter()
            .map(|h| {
                let name = format!("{}{}", "  ".repeat(h.level - 1), h.text);
                (name, Action::GotoSrc(h.src))
            })
            .collect();
        let mut menu = Menu::new(s.outline, items);
        menu.selected = current;
        self.dialog = Some(Dialog::Menu(menu));
    }

    // --- Выбор в просмотре ---

    pub(super) fn select_item(&mut self, forward: bool) {
        if self.pane.is_ready() && !self.pane.select(forward) {
            self.message = Some(self.lang.strings().no_links.into());
        }
    }

    /// Enter, а для чекбокса и пробел.
    pub(super) fn activate(&mut self, only_task: bool) {
        match self.pane.selected_item().cloned() {
            Some(Item::Task { src, .. }) => self.toggle_task(src),
            Some(Item::Link(url)) if !only_task => self.follow_link(&url),
            _ => self.message = Some(self.lang.strings().select_first.into()),
        }
    }

    fn toggle_task(&mut self, src: usize) {
        if !self.pane.is_ready() {
            return;
        }
        // Между `[` и `]` всегда один символ.
        let pos = self.buffer.pos_of_byte(src + 1);
        let new = match self.buffer.slice(pos..pos + 1).as_str() {
            " " | "\t" => "x",
            "x" | "X" => " ",
            _ => return,
        };
        let anchor = self.buffer.selection().map(|r| {
            let cursor = self.buffer.cursor();
            if r.start == cursor { r.end } else { r.start }
        });
        let place = Some((anchor, self.buffer.cursor()));
        self.buffer.edit(pos..pos + 1, new, place);
    }

    fn follow_link(&mut self, url: &str) {
        let s = self.lang.strings();
        match links::parse(url, self.path.as_deref()) {
            Target::Anchor(anchor) => {
                let path = self.path.clone();
                self.arrive(Location {
                    path,
                    spot: Spot::Anchor(anchor),
                    back: false,
                });
            }
            Target::Web(url) => {
                self.message = Some(match self.opener.open(&url) {
                    Ok(()) => format!("{}: {url}", s.opening),
                    Err(e) => format!("{}: {e}", s.cannot_open),
                });
            }
            Target::File(path, anchor) => self.go(Location {
                path: Some(path),
                spot: anchor.map_or(Spot::Start, Spot::Anchor),
                back: false,
            }),
            Target::Unsupported => self.message = Some(s.unsupported_link.into()),
        }
    }

    pub(super) fn go_back(&mut self) {
        match self.history.last().cloned() {
            Some(location) => self.go(location),
            None => self.message = Some(self.lang.strings().no_history.into()),
        }
    }

    /// Переход, который может потребовать сохранить правки.
    pub(super) fn go(&mut self, location: Location) {
        if !self.is_here(&location.path) && self.is_modified() {
            self.ask_unsaved(Then::Go(location));
        } else {
            self.arrive(location);
        }
    }

    pub(super) fn is_here(&self, path: &Option<PathBuf>) -> bool {
        if *path == self.path {
            return true;
        }
        let real = |p: &Option<PathBuf>| p.as_deref().and_then(|p| Path::canonicalize(p).ok());
        real(path).is_some_and(|p| Some(p) == real(&self.path))
    }

    /// Переход к месту в этом или другом файле.
    pub(super) fn arrive(&mut self, location: Location) {
        let s = self.lang.strings();
        let here = Location {
            path: self.path.clone(),
            spot: Spot::Src(match self.mode {
                Mode::View => self.pane.source_at_top(),
                Mode::Edit => self.buffer.byte_of(self.buffer.cursor()),
            }),
            back: true,
        };
        let same = self.is_here(&location.path);
        if !same {
            let Some(path) = &location.path else { return };
            if !path.exists() {
                self.message = Some(format!("{}: {}", path.display(), s.no_such_file));
                return;
            }
            match load(path, None, self.lang) {
                Ok(loaded) => {
                    self.pane.reset(Some(path.clone()));
                    self.open_other(path.clone(), loaded);
                    self.leave_files();
                }
                Err(e) => {
                    self.message = Some(e.to_string());
                    return;
                }
            }
        }
        let src = match &location.spot {
            Spot::Start => Some(0),
            Spot::Src(src) => Some(*src),
            Spot::Anchor(anchor) => self.find_anchor(anchor),
        };
        match src {
            Some(src) => {
                self.goto_src(src);
                if location.back {
                    self.history.pop();
                } else if same || here.path.is_some() {
                    self.history.push(here);
                }
            }
            None => {
                self.message = Some(s.no_such_section.into());
                if !same {
                    self.goto_src(0);
                    self.history.push(here);
                }
            }
        }
    }

    fn find_anchor(&self, anchor: &str) -> Option<usize> {
        if anchor.is_empty() || anchor == "top" {
            return Some(0);
        }
        let wanted = anchor.to_lowercase();
        let headings = preview::headings(&self.buffer.text());
        headings
            .into_iter()
            .find(|h| h.slug == wanted)
            .map(|h| h.src)
    }
}
