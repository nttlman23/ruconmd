//! Состояние программы и реакция на ввод. Отрисовки и терминала здесь нет.

mod find;
mod markup;
mod navigate;
mod protect;

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::buffer::{Buffer, Eol};
use crate::clipboard::Clipboard;
use crate::command::{Command, Mode};
use crate::config::{Config, Warning};
use crate::dialog::{Action, Ask, Dialog, Field, Location, Menu, Outcome, SaveJob, Then};
use crate::encoding::{self, Enc, Format, Unencodable};
use crate::file::{self, Disk, ReadError};
use crate::graphics::{Graphics, Wanted};
use crate::i18n::Lang;
use crate::input::Input;
use crate::key::{Chord, Key};
use crate::keymap::Keymap;
use crate::links::Opener;
use crate::mdline::States;
use crate::pane::{Pane, Shown};
use crate::recovery::{Orphan, Recovery};
use crate::search::{Query, Search};
use crate::style::Theme;
use crate::view::{Place, Viewport};

const WHEEL_LINES: isize = 3;
/// Время на подсветку кода за одну отрисовку.
const HIGHLIGHT_BUDGET: Duration = Duration::from_millis(15);

/// Прямоугольник текста на экране, без номеров строк.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextArea {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

pub struct Options {
    pub path: Option<PathBuf>,
    pub mode: Mode,
    pub lang: Lang,
    pub encoding: Option<Enc>,
    /// Номер строки с единицы.
    pub line: Option<usize>,
    pub config: Config,
    pub warnings: Vec<Warning>,
    /// Где хранить копии для восстановления. `None`: не хранить.
    pub state_dir: Option<PathBuf>,
}

pub struct App {
    pub path: Option<PathBuf>,
    pub buffer: Buffer,
    pub view: Viewport,
    pub pane: Pane,
    /// Состояния строк для подсветки разметки.
    pub states: States,
    pub mode: Mode,
    pub lang: Lang,
    pub theme: Theme,
    pub keymap: Keymap,
    pub message: Option<String>,
    pub dialog: Option<Dialog>,
    pub help: bool,
    pub should_quit: bool,
    /// Кодировка для следующей записи.
    pub format: Format,
    /// Кодировка файла на диске.
    saved_format: Format,
    /// Файл прочитан с потерями, запись в него требует подтверждения.
    lossy: bool,
    /// Файл, каким программа видела его последний раз. `None`: его не было.
    disk: Option<Disk>,
    /// На диске не то, от чего идут правки.
    disk_differs: bool,
    next_check: Instant,
    state_dir: Option<PathBuf>,
    recovery: Option<Recovery>,
    recovery_failed: bool,
    orphan: Option<Orphan>,
    backup: bool,
    backed_up: HashSet<PathBuf>,
    autosave: Option<Duration>,
    autosave_state: Option<protect::Autosave>,
    preview_width: usize,
    pub wanted_images: Wanted,
    clipboard: Box<dyn Clipboard>,
    opener: Box<dyn Opener>,
    /// Поиск, совпадения которого подсвечены.
    pub search: Option<Search>,
    query: Query,
    replace_with: String,
    /// Пустое совпадение, на котором стоит курсор.
    last_empty_hit: Option<usize>,
    /// Откуда приходили по ссылкам.
    history: Vec<Location>,
    area: TextArea,
    /// Колонка, к которой стремится курсор при движении вверх и вниз.
    goal: Option<usize>,
    /// Показать курсор при следующей отрисовке.
    follow: bool,
    dragging: bool,
}

struct Loaded {
    text: String,
    format: Format,
    lossy: bool,
    large: bool,
    disk: Option<Disk>,
}

fn load(path: &Path, forced: Option<Enc>, lang: Lang) -> io::Result<Loaded> {
    let s = lang.strings();
    let fail = |reason: String| io::Error::other(format!("{}: {reason}", path.display()));
    match file::read_with_meta(path) {
        Ok(Some((bytes, meta))) => {
            let decoded = encoding::decode(&bytes, forced);
            Ok(Loaded {
                text: decoded.text,
                format: decoded.format,
                lossy: decoded.lossy,
                large: bytes.len() > file::LARGE,
                disk: Some(Disk::of(&meta, &bytes)),
            })
        }
        Ok(None) => Ok(Loaded {
            text: String::new(),
            format: forced.map_or(Format::UTF8, Format::of),
            lossy: false,
            large: false,
            disk: None,
        }),
        Err(ReadError::Directory) => Err(fail(s.is_directory.into())),
        Err(ReadError::NotRegular) => Err(fail(s.not_regular.into())),
        Err(ReadError::Io(e)) => Err(fail(e.to_string())),
    }
}

impl App {
    pub fn open(
        options: Options,
        clipboard: Box<dyn Clipboard>,
        opener: Box<dyn Opener>,
    ) -> io::Result<Self> {
        let lang = options.lang;
        let loaded = match &options.path {
            Some(path) => load(path, options.encoding, lang)?,
            None => Loaded {
                text: String::new(),
                format: options.encoding.map_or(Format::UTF8, Format::of),
                lossy: false,
                large: false,
                disk: None,
            },
        };
        let config = options.config;
        let path = options.path;
        let state_dir = options.state_dir;
        let recovery = state_dir
            .clone()
            .map(|dir| Recovery::new(dir, path.as_deref()));
        let mut app = Self {
            path,
            buffer: Buffer::new(""),
            view: Viewport::new(config.editor),
            pane: Pane::default(),
            states: States::default(),
            mode: options.mode,
            lang,
            theme: config.theme,
            keymap: config.keymap,
            message: None,
            dialog: None,
            help: false,
            should_quit: false,
            format: Format::UTF8,
            saved_format: Format::UTF8,
            lossy: false,
            disk: None,
            disk_differs: false,
            next_check: Instant::now(),
            state_dir,
            recovery,
            recovery_failed: false,
            orphan: None,
            backup: config.backup,
            backed_up: HashSet::new(),
            autosave: config.autosave,
            autosave_state: None,
            preview_width: config.preview_width,
            wanted_images: config.images,
            clipboard,
            opener,
            search: None,
            query: Query::default(),
            replace_with: String::new(),
            last_empty_hit: None,
            history: Vec::new(),
            area: TextArea::default(),
            goal: None,
            follow: true,
            dragging: false,
        };
        app.pane.reset(app.path.clone());
        app.set_content(loaded);
        if let Some(line) = options.line {
            app.goto_line(line);
        }
        if app.mode == Mode::View {
            app.enter_view();
        }
        if let Some(first) = options.warnings.first() {
            let more = options.warnings.len() - 1;
            app.message = Some(lang.config_warning(first, more));
        }
        app.offer_recovery();
        Ok(app)
    }

    fn set_content(&mut self, loaded: Loaded) {
        self.buffer = Buffer::new(&loaded.text);
        self.goal = None;
        self.follow = true;
        self.view.clamp(&self.buffer);
        self.accept_file(&loaded);
    }

    /// Запоминает, каким прочитан файл.
    fn accept_file(&mut self, loaded: &Loaded) {
        let s = self.lang.strings();
        self.format = loaded.format;
        self.saved_format = loaded.format;
        self.lossy = loaded.lossy;
        self.disk = loaded.disk;
        self.disk_differs = false;
        if loaded.lossy {
            self.message = Some(s.lossy_warning.into());
        } else if loaded.large {
            self.message = Some(s.large_warning.into());
        }
    }

    pub fn is_modified(&self) -> bool {
        self.buffer.is_modified() || self.format != self.saved_format || self.disk_differs
    }

    /// Строка и колонка курсора с единицы.
    pub fn cursor_line_column(&self) -> (usize, usize) {
        let cursor = self.buffer.cursor();
        let line = self.buffer.line_of(cursor);
        (line + 1, cursor - self.buffer.line_start(line) + 1)
    }

    /// Вызывается перед отрисовкой: область текста и положение курсора в ней.
    pub fn prepare(&mut self, area: TextArea) {
        self.area = area;
        let (width, height) = (area.width as usize, area.height as usize);
        if let Some(line) = self.buffer.take_changed_line() {
            self.states.invalidate(line);
        }
        self.view.resize(&self.buffer, width, height);
        if self.follow && self.mode == Mode::Edit {
            self.view.reveal(&self.buffer, self.buffer.cursor());
        } else {
            self.view.clamp(&self.buffer);
        }
    }

    /// Вызывается перед отрисовкой просмотра.
    pub fn prepare_preview(&mut self, width: usize, height: usize) {
        let width = match self.preview_width {
            0 => width,
            limit => width.min(limit),
        };
        let tab = self.view.settings.tab_width;
        self.pane.prepare(&self.buffer, width, height, tab);
        self.pane.set_search(self.search.as_ref());
        self.pane.highlight(Instant::now() + HIGHLIGHT_BUDGET);
        self.pane.load_images();
    }

    pub fn set_graphics(&mut self, graphics: Option<Graphics>) {
        self.pane.set_graphics(graphics);
    }

    /// Картинки, которые будут на экране. Под окнами они не рисуются.
    pub fn images_on_screen(&self) -> Vec<Shown> {
        let covered = self.help || matches!(self.dialog, Some(Dialog::Menu(_)));
        if self.mode != Mode::View || covered || self.pane.graphics().is_none() {
            return Vec::new();
        }
        let mut shown = self.pane.shown_images();
        shown.retain(|s| self.pane.picture(s.image).is_some());
        shown
    }

    /// Нужна отрисовка, даже если пользователь ничего не нажимает.
    pub fn is_busy(&self) -> bool {
        self.mode == Mode::View && self.pane.is_busy()
    }

    // --- Ввод ---

    pub fn handle(&mut self, input: Input) {
        match input {
            Input::Key(chord) => self.key(chord),
            Input::Paste { text, continued } => self.pasted(&text, continued),
            Input::Mouse(mouse) => self.mouse(mouse),
            Input::Resize => {}
        }
    }

    fn pasted(&mut self, text: &str, continued: bool) {
        self.help = false;
        match &mut self.dialog {
            Some(Dialog::Field(field)) => field.insert(text),
            Some(Dialog::Menu(_) | Dialog::Replace(_)) => {}
            None if self.mode == Mode::Edit => {
                self.message = None;
                self.buffer.paste(text, continued);
                self.edited();
            }
            None => {}
        }
    }

    fn key(&mut self, chord: Chord) {
        self.message = None;
        if let Some(Dialog::Replace(_)) = &self.dialog {
            return self.replace_key(chord);
        }
        if self.find_option(chord) {
            return;
        }
        if let Some(dialog) = &mut self.dialog {
            let outcome = dialog.key(chord);
            return self.dialog_outcome(outcome);
        }
        let command = self.keymap.lookup(self.mode, chord);
        if self.help && command != Some(Command::Quit) {
            self.help = false;
            return;
        }
        match (command, self.mode) {
            (Some(command), _) => self.run(command),
            (None, Mode::Edit) => self.edit_key(chord),
            (None, Mode::View) => self.view_key(chord),
        }
    }

    fn view_key(&mut self, chord: Chord) {
        let page = self.page();
        match chord.key {
            Key::Up => self.pane.scroll(-1),
            Key::Down => self.pane.scroll(1),
            Key::PageUp => self.pane.scroll(-page),
            Key::PageDown => self.pane.scroll(page),
            Key::Home => self.pane.scroll_to_start(),
            Key::End => self.pane.scroll_to_end(),
            Key::Esc => {
                self.pane.clear_selection();
                self.search = None;
            }
            _ => {}
        }
    }

    fn edit_key(&mut self, chord: Chord) {
        let mods = chord.mods;
        if mods.alt {
            return;
        }
        let (select, ctrl) = (mods.shift, mods.ctrl);
        let cursor = self.buffer.cursor();
        match chord.key {
            Key::Char(c) if mods.is_none() => {
                self.buffer.type_text(c.encode_utf8(&mut [0; 4]));
                self.edited();
            }
            Key::Enter => {
                self.enter();
                self.edited();
            }
            Key::Tab if !ctrl => {
                self.tab(mods.shift);
                self.edited();
            }
            Key::Backspace => {
                if ctrl {
                    self.buffer.delete_word_back();
                } else {
                    self.buffer.backspace();
                }
                self.edited();
            }
            Key::Delete => {
                if ctrl {
                    self.buffer.delete_word();
                } else {
                    self.buffer.delete();
                }
                self.edited();
            }
            Key::Left => {
                let to = match self.buffer.selection() {
                    Some(range) if !select && !ctrl => range.start,
                    _ if ctrl => self.buffer.prev_word(cursor),
                    _ => self.buffer.prev_grapheme(cursor),
                };
                self.move_to(to, select);
            }
            Key::Right => {
                let to = match self.buffer.selection() {
                    Some(range) if !select && !ctrl => range.end,
                    _ if ctrl => self.buffer.next_word(cursor),
                    _ => self.buffer.next_grapheme(cursor),
                };
                self.move_to(to, select);
            }
            Key::Up => self.move_rows(-1, select),
            Key::Down => self.move_rows(1, select),
            Key::PageUp => self.move_page(-self.page(), select),
            Key::PageDown => self.move_page(self.page(), select),
            Key::Home if ctrl => self.move_to(0, select),
            Key::End if ctrl => self.move_to(self.buffer.len(), select),
            Key::Home => {
                let to = self.row_start(cursor);
                self.move_to(to, select);
            }
            Key::End => {
                let to = self.row_end(cursor);
                self.move_to(to, select);
            }
            Key::Esc => {
                self.buffer.clear_selection();
                self.search = None;
            }
            _ => {}
        }
    }

    fn mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.wheel(-WHEEL_LINES),
            MouseEventKind::ScrollDown => self.wheel(WHEEL_LINES),
            MouseEventKind::Down(MouseButton::Left) if self.mode == Mode::View => {
                let y = mouse.row.checked_sub(self.area.y);
                if let Some(y) = y.filter(|y| *y < self.area.height)
                    && self.dialog.is_none()
                    && !self.help
                    && self.pane.select_at(mouse.column as usize, y as usize)
                {
                    self.message = None;
                    self.activate(false);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let a = self.area;
                let inside = (a.y..a.y + a.height).contains(&mouse.row);
                if self.dialog.is_none() && !self.help && inside {
                    self.message = None;
                    self.dragging = true;
                    let extend = mouse.modifiers.contains(KeyModifiers::SHIFT);
                    self.move_to(self.under(mouse), extend);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.move_to(self.under(mouse), true);
            }
            MouseEventKind::Up(MouseButton::Left) => self.dragging = false,
            _ => {}
        }
    }

    fn under(&self, mouse: MouseEvent) -> usize {
        let x = mouse.column as isize - self.area.x as isize;
        let y = mouse.row as isize - self.area.y as isize;
        self.view.hit(&self.buffer, x, y)
    }

    fn wheel(&mut self, delta: isize) {
        if self.dialog.is_some() || self.help {
            return;
        }
        match self.mode {
            Mode::View => self.pane.scroll(delta),
            Mode::Edit => {
                self.view.scroll(&self.buffer, delta);
                self.follow = false;
            }
        }
    }

    // --- Курсор ---

    fn page(&self) -> isize {
        self.view.height.saturating_sub(1).max(1) as isize
    }

    fn move_to(&mut self, pos: usize, select: bool) {
        self.buffer.move_to(pos, select);
        self.goal = None;
        self.follow = true;
    }

    fn edited(&mut self) {
        self.goal = None;
        self.follow = true;
    }

    fn move_rows(&mut self, delta: isize, select: bool) {
        let (place, column) = self.view.locate(&self.buffer, self.buffer.cursor());
        let goal = self.goal.unwrap_or(column);
        let target = self.view.advance(&self.buffer, place, delta);
        // На первой и последней строке движение ведёт к краю документа.
        let pos = match target == place {
            true if delta < 0 => 0,
            true => self.buffer.len(),
            false => self.view.position(&self.buffer, target, goal),
        };
        self.move_to(pos, select);
        self.goal = Some(goal);
    }

    fn move_page(&mut self, delta: isize, select: bool) {
        self.view.scroll(&self.buffer, delta);
        self.move_rows(delta, select);
    }

    /// Начало экранной строки, а с него начало строки текста.
    fn row_start(&self, pos: usize) -> usize {
        let (place, _) = self.view.locate(&self.buffer, pos);
        let start = self.view.position(&self.buffer, place, 0);
        if start == pos {
            self.buffer.line_start(place.line)
        } else {
            start
        }
    }

    /// Конец экранной строки, а с него конец строки текста.
    fn row_end(&self, pos: usize) -> usize {
        let (place, _) = self.view.locate(&self.buffer, pos);
        let end = self.view.position(&self.buffer, place, usize::MAX);
        if end == pos {
            self.buffer.line_end(place.line)
        } else {
            end
        }
    }

    fn goto_line(&mut self, line: usize) {
        let line = line.clamp(1, self.buffer.line_count()) - 1;
        self.move_to(self.buffer.line_start(line), false);
        self.view.top = Place { line, row: 0 };
        let back = self.view.height as isize / 2;
        self.view.scroll(&self.buffer, -back);
        if self.mode == Mode::View {
            self.enter_view();
        }
    }

    // --- Команды ---

    fn run(&mut self, command: Command) {
        let s = self.lang.strings();
        match command {
            Command::ToggleMode => self.toggle_mode(),
            Command::Help => self.help = true,
            Command::Quit if self.is_modified() => self.ask_unsaved(Then::Quit),
            Command::Quit => self.should_quit = true,
            Command::Save => self.save(Then::Stay),
            Command::SaveAs => self.ask_path(Then::Stay),
            Command::Undo => {
                if self.buffer.undo() {
                    self.edited();
                } else {
                    self.message = Some(s.nothing_to_undo.into());
                }
            }
            Command::Redo => {
                if self.buffer.redo() {
                    self.edited();
                } else {
                    self.message = Some(s.nothing_to_redo.into());
                }
            }
            Command::Copy if self.mode == Mode::Edit => {
                let text = self.buffer.slice(self.copy_range());
                self.clipboard.set(&text);
                self.message = Some(s.copied.into());
            }
            Command::Cut => {
                let range = self.copy_range();
                self.clipboard.set(&self.buffer.slice(range.clone()));
                self.buffer.delete_range(range);
                self.edited();
            }
            Command::Paste => match self.clipboard.get() {
                Some(text) if !text.is_empty() => {
                    self.buffer.paste(&text, false);
                    self.edited();
                }
                _ => self.message = Some(s.clipboard_empty.into()),
            },
            Command::SelectAll if self.mode == Mode::Edit => self.buffer.select_all(),
            Command::GotoLine => {
                let field = Field::new(Ask::GotoLine, s.ask_line, String::new());
                self.dialog = Some(Dialog::Field(field));
            }
            Command::Encoding => self.ask_encoding(),
            Command::Find => self.ask_find(Ask::Find),
            Command::FindNext => self.find(true),
            Command::FindPrev => self.find(false),
            Command::Replace => self.ask_find(Ask::ReplaceFind),
            Command::Outline => self.ask_outline(),
            Command::Bold | Command::Italic | Command::Link => {
                match command {
                    Command::Bold => self.emphasis(2),
                    Command::Italic => self.emphasis(1),
                    _ => self.insert_link(),
                }
                self.edited();
            }
            Command::NextItem => self.select_item(true),
            Command::PrevItem => self.select_item(false),
            Command::OpenItem => self.activate(false),
            Command::ToggleCheckbox => self.activate(true),
            Command::Back => self.go_back(),
            // В просмотре копировать и выделять нечего.
            Command::Copy | Command::SelectAll => {}
        }
    }

    /// Выделение, а без него вся строка вместе с переводом.
    fn copy_range(&self) -> std::ops::Range<usize> {
        self.buffer.selection().unwrap_or_else(|| {
            let line = self.buffer.line_of(self.buffer.cursor());
            self.buffer.line_start(line)..self.buffer.line_start(line + 1)
        })
    }

    fn toggle_mode(&mut self) {
        self.mode = self.mode.toggled();
        match self.mode {
            Mode::View => self.enter_view(),
            Mode::Edit => self.leave_view(),
        }
    }

    /// Просмотр открывается на том же блоке и на той же высоте экрана.
    fn enter_view(&mut self) {
        let cursor = self.buffer.cursor();
        let (pos, row) = if self.view.is_visible(&self.buffer, cursor) {
            let (place, _) = self.view.locate(&self.buffer, cursor);
            (cursor, self.view.rows_between(&self.buffer, place))
        } else {
            (self.view.position(&self.buffer, self.view.top, 0), 0)
        };
        self.pane.show(self.buffer.byte_of(pos), row);
    }

    /// Без прокрутки всё остаётся как было, после неё редактор следует за просмотром.
    fn leave_view(&mut self) {
        self.follow = false;
        if !self.pane.is_scrolled() {
            return;
        }
        let focus = self.pane.focus();
        let src = focus.unwrap_or_else(|| self.pane.source_at_top());
        let pos = self.buffer.pos_of_byte(src);
        self.view.top = self.view.locate(&self.buffer, pos).0;
        self.view.clamp(&self.buffer);
        if focus.is_some() || !self.view.is_visible(&self.buffer, self.buffer.cursor()) {
            self.move_to(pos, false);
            self.follow = false;
        }
    }

    // --- Запросы ---

    fn menu(&mut self, title: &str, items: Vec<(String, Action)>) {
        self.dialog = Some(Dialog::Menu(Menu::new(title, items)));
    }

    fn ask_unsaved(&mut self, then: Then) {
        let s = self.lang.strings();
        self.menu(
            s.ask_quit,
            vec![
                (s.save.into(), Action::SaveThen(then.clone())),
                (s.dont_save.into(), Action::Discard(then)),
                (s.cancel.into(), Action::Cancel),
            ],
        );
    }

    fn ask_path(&mut self, then: Then) {
        let path = self.path.as_ref();
        let text = path.map_or(String::new(), |p| p.display().to_string());
        let title = self.lang.strings().ask_path;
        let field = Field::new(Ask::SavePath(then), title, text);
        self.dialog = Some(Dialog::Field(field));
    }

    fn ask_encoding(&mut self) {
        let s = self.lang.strings();
        let mut items = Vec::new();
        for (eol, name) in [(Eol::Lf, "LF"), (Eol::Crlf, "CRLF")] {
            items.push((format!("{} {name}", s.line_ends), Action::SetEol(eol)));
        }
        let mut formats: Vec<Format> = Enc::ALL.into_iter().map(Format::of).collect();
        let utf8_bom = Format {
            enc: Enc::Utf8,
            bom: true,
        };
        formats.insert(1, utf8_bom);
        for format in formats {
            items.push((format!("{} {format}", s.save_in), Action::SetFormat(format)));
        }
        if self.path.is_some() {
            for enc in Enc::ALL {
                items.push((format!("{} {}", s.read_as, enc.name()), Action::Reload(enc)));
            }
        }
        self.menu(self.lang.command(Command::Encoding), items);
    }

    fn dialog_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Open => {}
            Outcome::Close => self.dialog = None,
            Outcome::Submit(ask, text) => {
                self.dialog = None;
                self.submit(ask, text);
            }
            Outcome::Act(action) => {
                self.dialog = None;
                self.act(action);
            }
        }
    }

    fn submit(&mut self, ask: Ask, text: String) {
        let trimmed = text.trim();
        match ask {
            Ask::Find => self.submit_find(text),
            Ask::ReplaceFind => self.submit_replace_find(text),
            Ask::ReplaceWith => self.submit_replace_with(text),
            Ask::GotoLine => match trimmed.parse() {
                Ok(line) => self.goto_line(line),
                Err(_) => self.message = Some(self.lang.strings().bad_number.into()),
            },
            Ask::SavePath(_) if trimmed.is_empty() => {}
            Ask::SavePath(then) => self.write(SaveJob {
                path: PathBuf::from(trimmed),
                format: self.format,
                then,
                overwrite: false,
                allow_lossy: false,
                ignore_disk: false,
            }),
        }
    }

    fn act(&mut self, action: Action) {
        let s = self.lang.strings();
        match action {
            Action::Cancel => {}
            Action::Save(job) => self.write(job),
            Action::SaveThen(then) => self.save(then),
            Action::Discard(then) => self.proceed(then),
            Action::GotoSrc(src) => self.goto_src(src),
            Action::Restore => self.restore(),
            Action::DropCopy => self.drop_copy(),
            Action::TakeDisk => self.take_disk(),
            Action::SaveAs => self.ask_path(Then::Stay),
            Action::Reload(_) if self.is_modified() => {
                self.message = Some(s.save_first.into());
            }
            Action::Reload(enc) => {
                let Some(path) = self.path.clone() else {
                    return;
                };
                match load(&path, Some(enc), self.lang) {
                    Ok(loaded) => self.set_content(loaded),
                    Err(e) => self.message = Some(e.to_string()),
                }
            }
            Action::SetFormat(format) => self.format = format,
            Action::SetEol(eol) => {
                self.buffer.convert_eol(eol);
                self.edited();
            }
        }
    }

    // --- Запись ---

    /// Продолжает то, ради чего спрашивали о сохранении.
    fn proceed(&mut self, then: Then) {
        match then {
            Then::Stay => {}
            Then::Quit => self.should_quit = true,
            Then::Go(location) => self.arrive(location),
        }
    }

    fn save(&mut self, then: Then) {
        match self.path.clone() {
            Some(path) => self.write(SaveJob {
                path,
                format: self.format,
                then,
                overwrite: true,
                allow_lossy: false,
                ignore_disk: false,
            }),
            None => self.ask_path(then),
        }
    }

    fn write(&mut self, job: SaveJob) {
        let s = self.lang.strings();
        let same = self.path.as_ref() == Some(&job.path);
        if !same && !job.overwrite && job.path.symlink_metadata().is_ok() {
            let title = format!("{}: {}", job.path.display(), s.ask_overwrite);
            let yes = SaveJob {
                overwrite: true,
                ..job
            };
            let items = vec![
                (s.cancel.into(), Action::Cancel),
                (s.overwrite.into(), Action::Save(yes)),
            ];
            return self.menu(&title, items);
        }
        if same && self.lossy && !job.allow_lossy {
            let yes = SaveJob {
                allow_lossy: true,
                ..job
            };
            let items = vec![
                (s.cancel.into(), Action::Cancel),
                (s.save_anyway.into(), Action::Save(yes)),
            ];
            return self.menu(s.ask_lossy, items);
        }
        if same && !job.ignore_disk && self.ask_about_disk(&job) {
            return;
        }
        let bytes = match encoding::encode(&self.buffer.text(), job.format) {
            Ok(bytes) => bytes,
            Err(Unencodable(c)) => {
                let name = job.format.enc.name();
                let title = format!("{name}: {} «{c}» U+{:04X}", s.ask_unencodable, c as u32);
                let utf8 = SaveJob {
                    format: Format::UTF8,
                    ..job
                };
                let items = vec![
                    (s.save_utf8.into(), Action::Save(utf8)),
                    (s.cancel.into(), Action::Cancel),
                ];
                return self.menu(&title, items);
            }
        };
        let backup = self.back_up(&job.path, &bytes);
        match file::write(&job.path, &bytes) {
            Ok(()) => {
                let mut message = format!("{}: {}", s.saved, job.path.display());
                if let Err(e) = backup {
                    message = format!("{message} ({}: {e})", s.no_backup);
                }
                self.message = Some(message);
                self.after_write(&job.path, &bytes);
                self.format = job.format;
                self.saved_format = job.format;
                self.lossy = false;
                self.buffer.mark_saved();
                self.proceed(job.then);
            }
            Err(e) => {
                self.message = Some(format!("{} {}: {e}", s.not_saved, job.path.display()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::Local;
    use std::fs;

    /// Вместо системного приложения записывает адреса.
    #[derive(Default)]
    struct Opened;

    thread_local! {
        static URLS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    impl Opener for Opened {
        fn open(&mut self, url: &str) -> io::Result<()> {
            URLS.with_borrow_mut(|u| u.push(url.to_owned()));
            Ok(())
        }
    }

    fn open(path: Option<PathBuf>, encoding: Option<Enc>) -> App {
        let options = Options {
            path,
            mode: Mode::Edit,
            lang: Lang::En,
            encoding,
            line: None,
            config: Config::default(),
            warnings: Vec::new(),
            state_dir: None,
        };
        let mut app =
            App::open(options, Box::<Local>::default(), Box::<Opened>::default()).unwrap();
        draw(&mut app);
        app
    }

    fn with_text(text: &str) -> App {
        let mut app = open(None, None);
        app.buffer = Buffer::new(text);
        draw(&mut app);
        app
    }

    fn draw(app: &mut App) {
        app.prepare(TextArea {
            x: 4,
            y: 1,
            width: 41,
            height: 10,
        });
        if app.mode == Mode::View {
            app.prepare_preview(44, 10);
        }
    }

    fn press(app: &mut App, chords: &str) {
        for chord in chords.split(' ') {
            app.handle(Input::Key(chord.parse().unwrap()));
            draw(app);
        }
    }

    fn type_in(app: &mut App, text: &str) {
        for c in text.chars() {
            let key = if c == '\n' { Key::Enter } else { Key::Char(c) };
            app.handle(Input::Key(Chord::plain(key)));
            draw(app);
        }
    }

    fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
        app.handle(Input::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }));
        draw(app);
    }

    fn menu_title(app: &App) -> &str {
        match &app.dialog {
            Some(Dialog::Menu(m)) => &m.title,
            other => panic!("expected a menu, got {other:?}"),
        }
    }

    const DOWN: MouseEventKind = MouseEventKind::Down(MouseButton::Left);
    const DRAG: MouseEventKind = MouseEventKind::Drag(MouseButton::Left);
    const UP: MouseEventKind = MouseEventKind::Up(MouseButton::Left);

    #[test]
    fn edit_save_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        let mut app = open(Some(path.clone()), None);
        type_in(&mut app, "# Заметка\nтекст");
        assert!(app.is_modified());
        press(&mut app, "ctrl+ы");
        assert!(!app.is_modified());
        let eol = Eol::PLATFORM.as_str();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("# Заметка{eol}текст")
        );
        press(&mut app, "ctrl+q");
        assert!(app.should_quit);
    }

    #[test]
    fn untouched_files_are_saved_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        for text in [
            "",
            "текст",
            "а\nб\r\nв\r\n",
            "а\rб\u{2028}в\u{0}\n\n",
            "\r\n",
        ] {
            for enc in Enc::ALL {
                let Ok(bytes) = encoding::encode(text, Format::of(enc)) else {
                    continue;
                };
                fs::write(&path, &bytes).unwrap();
                let mut app = open(Some(path.clone()), Some(enc));
                press(&mut app, "ctrl+s");
                assert_eq!(fs::read(&path).unwrap(), bytes, "{text:?} {}", enc.name());
                type_in(&mut app, "x");
                press(&mut app, "ctrl+z ctrl+s");
                assert_eq!(fs::read(&path).unwrap(), bytes);
            }
        }
    }

    #[test]
    fn edits_keep_encoding_bom_and_line_ends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        let cp1251 = Format::of(Enc::Cp1251);
        let original = encoding::encode("Обычный текст\r\nзаметки\r\n", cp1251).unwrap();
        fs::write(&path, original).unwrap();
        let mut app = open(Some(path.clone()), None);
        assert_eq!(app.format, cp1251);
        type_in(&mut app, "Ж\n");
        press(&mut app, "ctrl+s");
        let expected = encoding::encode("Ж\r\nОбычный текст\r\nзаметки\r\n", cp1251).unwrap();
        assert_eq!(fs::read(&path).unwrap(), expected);
    }

    #[test]
    fn unencodable_text_is_never_replaced_silently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        let original = encoding::encode("Обычный текст\n", Format::of(Enc::Cp866)).unwrap();
        fs::write(&path, &original).unwrap();
        let mut app = open(Some(path.clone()), None);
        type_in(&mut app, "😀");
        press(&mut app, "ctrl+s");
        assert!(menu_title(&app).contains("CP866"));
        press(&mut app, "esc");
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(app.is_modified());
        press(&mut app, "ctrl+s enter");
        assert_eq!(fs::read_to_string(&path).unwrap(), "😀Обычный текст\n");
        assert_eq!(app.format, Format::UTF8);
        assert!(!app.is_modified());
    }

    #[test]
    fn damaged_file_needs_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        fs::write(&path, b"abc\xff").unwrap();
        let mut app = open(Some(path.clone()), Some(Enc::Utf8));
        assert!(app.message.is_some());
        press(&mut app, "ctrl+s enter");
        assert_eq!(
            fs::read(&path).unwrap(),
            b"abc\xff",
            "cancel is the default"
        );
        press(&mut app, "ctrl+s down enter");
        assert_eq!(fs::read(&path).unwrap(), "abc\u{fffd}".as_bytes());
    }

    #[test]
    fn quit_asks_about_unsaved_text() {
        let mut app = with_text("");
        type_in(&mut app, "x");
        press(&mut app, "ctrl+q");
        assert!(!app.should_quit);
        press(&mut app, "esc ctrl+q down down enter");
        assert!(!app.should_quit && app.dialog.is_none());
        press(&mut app, "ctrl+q down enter");
        assert!(app.should_quit);
    }

    #[test]
    fn quit_with_saving_an_unnamed_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("новый.md");
        let mut app = with_text("");
        type_in(&mut app, "x");
        press(&mut app, "ctrl+q enter");
        assert!(matches!(app.dialog, Some(Dialog::Field(_))));
        app.handle(Input::Paste {
            text: path.display().to_string(),
            continued: false,
        });
        press(&mut app, "enter");
        assert!(app.should_quit);
        assert_eq!(fs::read_to_string(&path).unwrap(), "x");
    }

    #[test]
    fn failed_save_does_not_quit_or_lose_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = open(Some(dir.path().join("none/a.md")), None);
        type_in(&mut app, "x");
        press(&mut app, "ctrl+q enter");
        assert!(!app.should_quit && app.is_modified());
        assert!(app.message.is_some());
        assert_eq!(app.buffer.text(), "x");
    }

    #[test]
    fn save_as_asks_before_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        fs::write(&path, "old").unwrap();
        let mut app = with_text("new");
        press(&mut app, "f12");
        app.handle(Input::Paste {
            text: path.display().to_string(),
            continued: false,
        });
        press(&mut app, "enter enter");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "old",
            "cancel is the default"
        );
        assert!(app.path.is_none());
        press(&mut app, "f12");
        app.handle(Input::Paste {
            text: path.display().to_string(),
            continued: false,
        });
        press(&mut app, "enter down enter");
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(app.path.as_ref(), Some(&path));
    }

    #[test]
    fn encoding_menu_rereads_and_converts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        fs::write(&path, "Привет\n").unwrap();
        let mut app = open(Some(path.clone()), None);
        app.act(Action::Reload(Enc::Cp1251));
        assert_ne!(app.buffer.text(), "Привет\n");
        app.act(Action::Reload(Enc::Utf8));
        app.act(Action::SetFormat(Format::of(Enc::Cp866)));
        assert!(app.is_modified());
        app.act(Action::Reload(Enc::Utf8));
        assert_eq!(
            app.format.enc,
            Enc::Cp866,
            "unsaved changes block rereading"
        );
        app.act(Action::SetEol(Eol::Crlf));
        press(&mut app, "ctrl+s");
        let expected = encoding::encode("Привет\r\n", Format::of(Enc::Cp866)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), expected);
        press(&mut app, "f8 up enter");
        assert_eq!(app.format.enc, Enc::Koi8R);
    }

    #[test]
    fn clipboard_copies_selection_or_line() {
        let mut app = with_text("один\nдва\n");
        press(&mut app, "ctrl+c ctrl+v");
        assert_eq!(app.buffer.text(), "один\nодин\nдва\n");
        press(&mut app, "shift+right shift+right ctrl+x ctrl+end ctrl+v");
        assert_eq!(app.buffer.text(), "один\nин\nдва\nод");
        press(&mut app, "ctrl+z ctrl+z ctrl+z");
        assert_eq!(app.buffer.text(), "один\nдва\n");
    }

    #[test]
    fn vertical_movement_keeps_the_column() {
        let mut app = with_text("длинная строка\nx\nдлинная строка\n");
        press(&mut app, "end down down");
        assert_eq!(app.cursor_line_column(), (3, 15));
        press(&mut app, "down down");
        assert_eq!(app.buffer.cursor(), app.buffer.len());
        press(&mut app, "ctrl+home up");
        assert_eq!(app.buffer.cursor(), 0);
    }

    #[test]
    fn home_and_end_work_by_screen_rows_first() {
        let mut app = with_text(&"слово ".repeat(20));
        press(&mut app, "down end");
        let first = app.buffer.cursor();
        press(&mut app, "end");
        assert_eq!(app.buffer.cursor(), app.buffer.len());
        assert!(first < app.buffer.len());
        press(&mut app, "home");
        assert!(app.buffer.cursor() > 0);
        press(&mut app, "home");
        assert_eq!(app.buffer.cursor(), 0);
    }

    #[test]
    fn arrows_collapse_the_selection() {
        let mut app = with_text("abcd");
        press(&mut app, "shift+right shift+right left");
        assert_eq!((app.buffer.cursor(), app.buffer.selection()), (0, None));
        press(&mut app, "shift+right shift+right right");
        assert_eq!(app.buffer.cursor(), 2);
        press(&mut app, "ctrl+a delete");
        assert!(app.buffer.is_empty());
    }

    #[test]
    fn mouse_places_the_cursor_selects_and_scrolls() {
        let text: String = (0..100).map(|i| format!("строка {i}\n")).collect();
        let mut app = with_text(&text);
        mouse(&mut app, DOWN, 4 + 3, 1 + 2);
        assert_eq!(app.cursor_line_column(), (3, 4));
        mouse(&mut app, DRAG, 4 + 1, 1 + 3);
        mouse(&mut app, UP, 4 + 1, 1 + 3);
        assert_eq!(app.buffer.selected_text().unwrap(), "ока 2\nс");
        mouse(&mut app, MouseEventKind::Moved, 30, 8);
        mouse(&mut app, MouseEventKind::ScrollDown, 5, 5);
        assert_eq!(app.view.top.line, 3);
        mouse(&mut app, MouseEventKind::ScrollDown, 5, 5);
        assert_eq!(
            app.view.top.line, 6,
            "wheel is not pulled back to the cursor"
        );
        type_in(&mut app, "x");
        assert!(app.view.is_visible(&app.buffer, app.buffer.cursor()));
    }

    #[test]
    fn dragging_past_the_edge_scrolls() {
        let mut app = with_text(&"x\n".repeat(100));
        mouse(&mut app, DOWN, 4, 1);
        for _ in 0..5 {
            mouse(&mut app, DRAG, 4, 11);
        }
        assert_eq!(app.view.top.line, 5);
    }

    #[test]
    fn mode_toggle_keeps_position_and_cursor() {
        let mut app = with_text(&"x\n\n".repeat(100));
        press(&mut app, "pagedown pagedown");
        let (top, cursor) = (app.view.top, app.buffer.cursor());
        press(&mut app, "f5 ctrl+у");
        assert_eq!((app.view.top, app.buffer.cursor()), (top, cursor));
    }

    #[test]
    fn scrolled_preview_moves_the_editor() {
        let text: String = (0..100).map(|i| format!("## Глава {i}\n\n")).collect();
        let mut app = with_text(&text);
        press(&mut app, "f5 end f5");
        let (line, _) = app.cursor_line_column();
        assert!(line > 190, "{line}");
        assert!(app.view.is_visible(&app.buffer, app.buffer.cursor()));
    }

    #[test]
    fn preview_opens_at_the_cursor_row() {
        let text: String = (0..100).map(|i| format!("абзац {i}\n\n")).collect();
        let mut app = with_text(&text);
        press(&mut app, "ctrl+g");
        type_in(&mut app, "101\n");
        let (place, _) = app.view.locate(&app.buffer, app.buffer.cursor());
        let row = app.view.rows_between(&app.buffer, place);
        press(&mut app, "f5");
        let line = &app.pane.visible()[row];
        assert_eq!(line.spans[0].text, "абзац 50");
        press(&mut app, "f5");
        assert_eq!(app.cursor_line_column().0, 101);
    }

    #[test]
    fn goto_line_works_in_preview() {
        let text: String = (0..100).map(|i| format!("абзац {i}\n\n")).collect();
        let mut app = with_text(&text);
        press(&mut app, "f5 ctrl+g");
        type_in(&mut app, "101\n");
        let shown = app.pane.visible().iter().flat_map(|l| &l.spans);
        assert!(shown.into_iter().any(|s| s.text == "абзац 50"));
    }

    #[test]
    fn view_mode_does_not_change_text() {
        let mut app = with_text("abc\n");
        press(&mut app, "f5");
        type_in(&mut app, "x\n y");
        press(&mut app, "backspace delete ctrl+x ctrl+z ctrl+v");
        app.handle(Input::Paste {
            text: "zzz".into(),
            continued: false,
        });
        assert_eq!(app.buffer.text(), "abc\n");
        assert!(!app.is_modified());
    }

    #[test]
    fn goto_line() {
        let mut app = with_text(&"x\n".repeat(100));
        press(&mut app, "ctrl+g");
        type_in(&mut app, "50\n");
        assert_eq!(app.cursor_line_column().0, 50);
        press(&mut app, "ctrl+g");
        type_in(&mut app, "9999\n");
        assert_eq!(app.cursor_line_column().0, 101);
        press(&mut app, "ctrl+g");
        type_in(&mut app, "abc\n");
        assert!(app.message.is_some());
    }

    #[test]
    fn tab_aligns_with_spaces() {
        let mut app = with_text("");
        type_in(&mut app, "ab");
        press(&mut app, "tab");
        assert_eq!(app.buffer.text(), "ab  ");
    }

    #[test]
    fn help_closes_on_any_key() {
        let mut app = with_text("");
        press(&mut app, "f1 f5");
        assert!(!app.help);
        assert_eq!(app.mode, Mode::Edit);
    }

    // --- Помощь при вводе ---

    #[test]
    fn enter_continues_and_finishes_lists() {
        let mut app = with_text("");
        type_in(&mut app, "1. один\nдва\n\nтекст");
        assert_eq!(app.buffer.text(), "1. один\n2. два\nтекст");
        let mut app = with_text("");
        type_in(&mut app, "> - [x] а\nб");
        assert_eq!(app.buffer.text(), "> - [x] а\n> - [ ] б");
        press(&mut app, "ctrl+z");
        assert_eq!(app.buffer.text(), "> - [x] а\n> - [ ] ");
        press(&mut app, "ctrl+z");
        assert_eq!(app.buffer.text(), "> - [x] а");
    }

    #[test]
    fn enter_does_not_help_in_code_and_paste_is_verbatim() {
        let mut app = with_text("```\n- a\n```\n");
        press(&mut app, "down end enter");
        assert_eq!(app.buffer.text(), "```\n- a\n\n```\n");
        let mut app = with_text("- a");
        press(&mut app, "end");
        app.handle(Input::Paste {
            text: "\n- b\n".into(),
            continued: false,
        });
        assert_eq!(app.buffer.text(), "- a\n- b\n");
    }

    #[test]
    fn enter_keeps_crlf() {
        let mut app = with_text("- a\r\n- b\r\n");
        press(&mut app, "end enter");
        assert_eq!(app.buffer.text(), "- a\r\n- \r\n- b\r\n");
        press(&mut app, "enter");
        assert_eq!(app.buffer.text(), "- a\r\n\r\n- b\r\n");
    }

    #[test]
    fn tab_nests_list_items_and_selected_lines() {
        let mut app = with_text("- a\n- b\n1. c\n");
        press(&mut app, "down end tab");
        assert_eq!(app.buffer.text(), "- a\n  - b\n1. c\n");
        assert_eq!(app.cursor_line_column(), (2, 6));
        press(&mut app, "shift+tab shift+tab");
        assert_eq!(app.buffer.text(), "- a\n- b\n1. c\n");
        press(&mut app, "ctrl+a tab");
        assert_eq!(app.buffer.text(), "  - a\n  - b\n   1. c\n");
        press(&mut app, "ctrl+z");
        assert_eq!(app.buffer.text(), "- a\n- b\n1. c\n");
    }

    #[test]
    fn bold_and_italic_toggle() {
        let mut app = with_text("слово тут");
        press(&mut app, "ctrl+shift+right ctrl+b");
        assert_eq!(app.buffer.text(), "**слово** тут");
        press(&mut app, "alt+i");
        assert_eq!(app.buffer.text(), "***слово*** тут");
        press(&mut app, "ctrl+b");
        assert_eq!(app.buffer.text(), "*слово* тут");
        press(&mut app, "alt+i");
        assert_eq!(app.buffer.text(), "слово тут");
        press(&mut app, "ctrl+z ctrl+z ctrl+z ctrl+z");
        assert!(!app.buffer.is_modified());
    }

    #[test]
    fn bold_without_selection_and_inside_one() {
        let mut app = with_text("");
        press(&mut app, "ctrl+b");
        type_in(&mut app, "x");
        assert_eq!(app.buffer.text(), "**x**");
        let mut app = with_text("");
        press(&mut app, "ctrl+b ctrl+b");
        assert_eq!(app.buffer.text(), "");
        let mut app = with_text("**x**");
        press(&mut app, "ctrl+a ctrl+b");
        assert_eq!(app.buffer.text(), "x");
        let mut app = with_text("   ");
        press(&mut app, "ctrl+a ctrl+b");
        assert_eq!(app.buffer.text(), "   ");
    }

    #[test]
    fn link_insertion() {
        let mut app = with_text("имя");
        press(&mut app, "ctrl+a ctrl+k");
        type_in(&mut app, "url");
        assert_eq!(app.buffer.text(), "[имя](url)");
        let mut app = with_text("https://a.ru");
        press(&mut app, "ctrl+a ctrl+k");
        type_in(&mut app, "имя");
        assert_eq!(app.buffer.text(), "[имя](https://a.ru)");
    }

    // --- Поиск и замена ---

    fn search_for(app: &mut App, key: &str, text: &str) {
        press(app, key);
        press(app, "home");
        for _ in 0..40 {
            press(app, "delete");
        }
        type_in(app, text);
        press(app, "enter");
    }

    #[test]
    fn find_goes_around_in_both_directions() {
        let mut app = with_text("Кот и кот\nКОТ");
        search_for(&mut app, "ctrl+f", "кот");
        assert_eq!(app.buffer.selection(), Some(0..3));
        assert_eq!(app.message.as_deref(), Some("1 of 3"));
        press(&mut app, "f3 f3");
        assert_eq!(app.buffer.selection(), Some(10..13));
        press(&mut app, "f3");
        assert_eq!(app.message.as_deref(), Some("1 of 3 (wrapped)"));
        press(&mut app, "shift+f3");
        assert_eq!(app.message.as_deref(), Some("3 of 3 (wrapped)"));
        press(&mut app, "ctrl+f alt+c enter");
        assert_eq!(app.message.as_deref(), Some("1 of 1 (wrapped)"));
    }

    #[test]
    fn find_options_work_in_russian_layout() {
        let mut app = with_text("a1 a2");
        press(&mut app, "ctrl+f alt+к");
        type_in(&mut app, "a\\d");
        press(&mut app, "enter");
        assert_eq!(app.message.as_deref(), Some("1 of 2"));
        search_for(&mut app, "ctrl+f", "(");
        assert!(matches!(&app.dialog, Some(Dialog::Field(f)) if f.title.contains("unclosed")));
    }

    #[test]
    fn empty_matches_do_not_stop_the_search() {
        let mut app = with_text("a\nb\nc");
        press(&mut app, "ctrl+f alt+r");
        type_in(&mut app, "^");
        press(&mut app, "enter");
        assert_eq!(app.cursor_line_column().0, 1);
        press(&mut app, "f3");
        assert_eq!(app.cursor_line_column().0, 2);
        press(&mut app, "f3 f3");
        assert_eq!(app.cursor_line_column().0, 1);
    }

    #[test]
    fn replace_one_skip_and_rest() {
        let mut app = with_text("a a a a a");
        press(&mut app, "right right");
        search_for(&mut app, "ctrl+r", "a");
        type_in(&mut app, "aa");
        press(&mut app, "enter enter space");
        assert_eq!(app.buffer.text(), "a aa a a a");
        app.handle(Input::Key(Chord::plain(Key::Char('ф'))));
        // Пропущенное не тронуто, первое заменено на втором круге, вставленное нет.
        assert_eq!(app.buffer.text(), "aa aa a aa aa");
        assert_eq!(app.message.as_deref(), Some("Replaced: 4"));
        press(&mut app, "ctrl+z");
        assert_eq!(app.buffer.text(), "a aa a a a");
    }

    #[test]
    fn replace_stops_after_one_circle() {
        let mut app = with_text("x x x");
        press(&mut app, "right right");
        search_for(&mut app, "ctrl+r", "x");
        type_in(&mut app, "xx");
        press(&mut app, "enter enter enter enter");
        assert_eq!(app.buffer.text(), "xx xx xx");
        assert!(app.dialog.is_none());
        assert_eq!(app.message.as_deref(), Some("Replaced: 3"));
    }

    #[test]
    fn replace_with_groups_empty_matches_and_crlf() {
        let mut app = with_text("a=1\r\nb=2\r\n");
        press(&mut app, "ctrl+r alt+r");
        type_in(&mut app, "(\\w)=(\\d)");
        press(&mut app, "enter");
        type_in(&mut app, "$2\\n$1");
        press(&mut app, "enter");
        app.handle(Input::Key(Chord::plain(Key::Char('A'))));
        assert_eq!(app.buffer.text(), "1\r\na\r\n2\r\nb\r\n");

        let mut app = with_text("a\nb");
        press(&mut app, "ctrl+r alt+r");
        type_in(&mut app, "^");
        press(&mut app, "enter");
        type_in(&mut app, "> ");
        press(&mut app, "enter enter enter");
        assert_eq!(app.buffer.text(), "> a\n> b");
        assert!(app.dialog.is_none());
    }

    #[test]
    fn replace_reports_nothing_found() {
        let mut app = with_text("a");
        search_for(&mut app, "ctrl+r", "z");
        press(&mut app, "enter");
        assert_eq!(app.message.as_deref(), Some("Not found"));
        assert!(app.dialog.is_none() && !app.is_modified());
    }

    #[test]
    fn preview_search_finds_rendered_text_across_wrapping() {
        let text = format!(
            "{}**найди** меня тут\n\n{}",
            "слово ".repeat(12),
            "x\n\n".repeat(50)
        );
        let mut app = with_text(&text);
        press(&mut app, "f5");
        search_for(&mut app, "ctrl+f", "слово найди меня");
        assert_eq!(app.message.as_deref(), Some("1 of 1"));
        assert!((0..10).any(|row| !app.pane.hits(row).is_empty()));
        search_for(&mut app, "ctrl+f", "**");
        assert_eq!(app.message.as_deref(), Some("Not found"));
        press(&mut app, "esc");
        assert!(app.search.is_none());
    }

    #[test]
    fn preview_search_scrolls_to_matches_and_code() {
        let text = format!("{}```\nцель\n```\n", "x\n\n".repeat(50));
        let mut app = with_text(&text);
        press(&mut app, "f5");
        search_for(&mut app, "ctrl+f", "цель");
        assert!((0..10).any(|row| app.pane.hits(row).iter().any(|(_, current)| *current)));
    }

    // --- Просмотр: выбор, ссылки, оглавление ---

    #[test]
    fn checkbox_is_toggled_from_preview() {
        let text = format!("{}- [ ] дело\r\n- [X] ещё\r\n", "x\r\n\r\n".repeat(30));
        let mut app = with_text(&text);
        press(&mut app, "f5 end tab space");
        assert!(app.buffer.text().contains("- [x] дело\r\n"));
        let top = app.pane.source_at_top();
        press(&mut app, "tab space");
        assert!(app.buffer.text().ends_with("- [ ] ещё\r\n"));
        assert_eq!(app.pane.source_at_top(), top, "the screen does not move");
        press(&mut app, "space");
        assert!(app.buffer.text().ends_with("- [x] ещё\r\n"));
        press(&mut app, "f5 ctrl+z ctrl+z ctrl+z");
        assert_eq!(app.buffer.text(), text);
    }

    #[test]
    fn mouse_toggles_a_checkbox_in_preview() {
        let mut app = with_text("текст\n\n- [ ] дело\n");
        press(&mut app, "f5");
        mouse(&mut app, DOWN, 10, 1);
        assert!(!app.is_modified());
        mouse(&mut app, DOWN, 1, 3);
        assert_eq!(app.buffer.text(), "текст\n\n- [x] дело\n");
    }

    #[test]
    fn web_links_go_to_the_system_and_others_do_not() {
        let mut app = with_text("[a](https://a.ru) [b](file:///etc/passwd) [c](run.sh)");
        press(&mut app, "f5 tab enter");
        URLS.with_borrow(|u| assert_eq!(*u, ["https://a.ru"]));
        press(&mut app, "tab enter");
        press(&mut app, "tab enter");
        assert!(app.message.as_deref().unwrap().contains("not opened"));
        URLS.with_borrow(|u| assert_eq!(u.len(), 1));
        press(&mut app, "space");
        assert!(!app.is_modified());
    }

    #[test]
    fn anchors_and_back() {
        let chapters: String = (0..40)
            .map(|i| format!("## Глава {i}\n\nтекст\n\n"))
            .collect();
        let mut app = with_text(&format!("[туда](#глава-30) [нет](#нет)\n\n{chapters}"));
        press(&mut app, "f5");
        let before = app.pane.source_at_top();
        press(&mut app, "tab enter");
        assert_eq!(app.pane.visible()[0].spans[0].text, "Глава 30");
        press(&mut app, "alt+left");
        assert_eq!(app.pane.source_at_top(), before);
        press(&mut app, "alt+left");
        assert_eq!(app.message.as_deref(), Some("Nowhere to go back"));
        press(&mut app, "tab tab enter");
        assert_eq!(app.message.as_deref(), Some("No such section"));
        press(&mut app, "tab enter f5");
        assert!(
            app.buffer
                .line(app.cursor_line_column().0 - 1)
                .contains("30")
        );
    }

    #[test]
    fn links_between_files() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.md"), dir.path().join("b.md"));
        fs::write(&a, "[b](b.md#два) [нет](none.md)").unwrap();
        fs::write(&b, format!("# Один\n\n{}## Два\n", "x\n\n".repeat(30))).unwrap();
        let mut app = open(Some(a.clone()), None);
        press(&mut app, "f5 tab enter");
        assert_eq!(app.path.as_ref(), Some(&b));
        assert!(
            app.pane
                .visible()
                .iter()
                .any(|l| l.spans.iter().any(|s| s.text == "Два"))
        );
        press(&mut app, "alt+left");
        assert_eq!(app.path.as_ref(), Some(&a));
        press(&mut app, "tab tab enter");
        assert!(app.message.as_deref().unwrap().contains("no such file"));
        assert!(!dir.path().join("none.md").exists());
        assert_eq!(app.path.as_ref(), Some(&a));
    }

    #[test]
    fn leaving_a_modified_file_asks_first() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.md"), dir.path().join("b.md"));
        fs::write(&a, "[b](b.md)").unwrap();
        fs::write(&b, "текст").unwrap();
        let mut app = open(Some(a.clone()), None);
        press(&mut app, "ctrl+end");
        type_in(&mut app, "!");
        press(&mut app, "f5 tab enter");
        assert_eq!(app.path.as_ref(), Some(&a));
        press(&mut app, "esc");
        assert!(app.is_modified());
        press(&mut app, "enter enter");
        assert_eq!(app.path.as_ref(), Some(&b));
        assert_eq!(fs::read_to_string(&a).unwrap(), "[b](b.md)!");
        press(&mut app, "f5");
        type_in(&mut app, "z");
        press(&mut app, "f5 alt+left down enter");
        assert_eq!(app.path.as_ref(), Some(&a));
        assert_eq!(fs::read_to_string(&b).unwrap(), "текст", "discarded");
    }

    #[test]
    fn outline_works_in_both_modes() {
        let chapters: String = (0..40)
            .map(|i| format!("## Глава {i}\n\nтекст\n\n"))
            .collect();
        let mut app = with_text(&chapters);
        press(&mut app, "ctrl+t pagedown enter");
        assert_eq!(app.cursor_line_column().0, 41);
        press(&mut app, "ctrl+t down enter");
        assert_eq!(
            app.cursor_line_column().0,
            45,
            "opens at the current heading"
        );
        press(&mut app, "f5 ctrl+t end enter f5");
        assert_eq!(app.cursor_line_column().0, 157);
        let mut app = with_text("текст");
        press(&mut app, "ctrl+t");
        assert!(app.dialog.is_none() && app.message.is_some());
    }

    // --- Защита данных ---

    struct Session {
        dir: tempfile::TempDir,
        path: PathBuf,
        now: Instant,
        config: Config,
    }

    impl Session {
        fn new(text: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("a.md");
            fs::write(&path, text).unwrap();
            Self {
                dir,
                path,
                now: Instant::now(),
                config: Config::default(),
            }
        }

        fn open(&self) -> App {
            let options = Options {
                path: Some(self.path.clone()),
                mode: Mode::Edit,
                lang: Lang::En,
                encoding: None,
                line: None,
                config: self.config.clone(),
                warnings: Vec::new(),
                state_dir: Some(self.state()),
            };
            let mut app =
                App::open(options, Box::<Local>::default(), Box::<Opened>::default()).unwrap();
            draw(&mut app);
            app
        }

        fn state(&self) -> PathBuf {
            self.dir.path().join("state")
        }

        fn copies(&self) -> usize {
            fs::read_dir(self.state()).map_or(0, Iterator::count)
        }

        /// Проходит столько секунд, по одной.
        fn pass(&mut self, app: &mut App, seconds: u64) {
            for _ in 0..seconds {
                self.now += Duration::from_secs(1);
                app.tick(self.now);
                draw(app);
            }
        }

        fn disk(&self) -> String {
            fs::read_to_string(&self.path).unwrap()
        }
    }

    #[test]
    fn lost_session_is_restored() {
        let mut s = Session::new("старое\n");
        let mut app = s.open();
        type_in(&mut app, "правка ");
        s.pass(&mut app, 8);
        // Фоновая запись могла ещё не закончиться.
        app.emergency_save().unwrap();
        drop(app);

        let mut app = s.open();
        assert!(menu_title(&app).contains("Unsaved changes found"));
        assert!(!menu_title(&app).contains("changed since"));
        press(&mut app, "enter");
        assert_eq!(app.buffer.text(), "правка старое\n");
        assert!(app.is_modified());
        assert_eq!(s.disk(), "старое\n", "restoring does not write the file");
        assert_eq!(s.copies(), 2, "own copy and lock, the old ones are gone");
        press(&mut app, "ctrl+z");
        assert_eq!(app.buffer.text(), "старое\n");
        press(&mut app, "ctrl+y ctrl+s");
        s.pass(&mut app, 1);
        assert_eq!(s.disk(), "правка старое\n");
        assert_eq!(s.copies(), 0);
        app.finish();
        assert!(s.open().dialog.is_none());
    }

    #[test]
    fn postponed_and_dropped_copies() {
        let s = Session::new("старое\n");
        let mut app = s.open();
        type_in(&mut app, "x");
        app.emergency_save().unwrap();
        drop(app);
        fs::write(&s.path, "чужое\n").unwrap();
        let mut app = s.open();
        assert!(menu_title(&app).contains("changed since"));
        press(&mut app, "down enter");
        assert_eq!(app.buffer.text(), "чужое\n");
        app.finish();
        drop(app);
        let mut app = s.open();
        assert!(app.dialog.is_some(), "offered again");
        press(&mut app, "down down enter");
        assert_eq!(s.copies(), 0);
        assert_eq!(app.buffer.text(), "чужое\n");
    }

    #[test]
    fn restored_text_keeps_encoding_and_line_ends() {
        let s = Session::new("");
        let cp866 = Format::of(Enc::Cp866);
        fs::write(
            &s.path,
            encoding::encode("Обычный текст\r\nзаметки\r\n", cp866).unwrap(),
        )
        .unwrap();
        let mut app = s.open();
        type_in(&mut app, "Ж");
        app.emergency_save().unwrap();
        drop(app);
        let mut app = s.open();
        press(&mut app, "enter ctrl+s");
        let want = encoding::encode("ЖОбычный текст\r\nзаметки\r\n", cp866).unwrap();
        assert_eq!(fs::read(&s.path).unwrap(), want);
    }

    #[test]
    fn two_windows_do_not_steal_each_others_copies() {
        let mut s = Session::new("текст\n");
        let mut first = s.open();
        type_in(&mut first, "x");
        first.emergency_save().unwrap();
        let mut second = s.open();
        assert!(second.dialog.is_none());
        assert!(
            second
                .message
                .as_deref()
                .unwrap()
                .contains("Another window")
        );
        second.finish();
        assert_eq!(s.copies(), 2, "the copy of the first window is intact");
        press(&mut first, "ctrl+z");
        s.pass(&mut first, 1);
        assert_eq!(s.copies(), 0);
    }

    #[test]
    fn foreign_change_is_reported_and_can_be_taken() {
        let mut s = Session::new("а\nб\nв\n");
        let mut app = s.open();
        press(&mut app, "down");
        type_in(&mut app, "моё ");
        fs::write(&s.path, "чужое\nб\r\nв\n").unwrap();
        s.pass(&mut app, 3);
        assert!(menu_title(&app).contains("another program"));
        press(&mut app, "down enter");
        assert_eq!(app.buffer.text(), "чужое\nб\r\nв\n");
        assert!(!app.is_modified());
        assert_eq!(app.cursor_line_column().0, 2);
        press(&mut app, "ctrl+z");
        assert_eq!(app.buffer.text(), "а\nмоё б\nв\n", "reload can be undone");
        s.pass(&mut app, 5);
        assert!(app.dialog.is_none());
    }

    #[test]
    fn foreign_change_is_not_overwritten_silently() {
        let mut s = Session::new("а\n");
        let mut app = s.open();
        type_in(&mut app, "моё ");
        fs::write(&s.path, "чужое, длиннее\n").unwrap();
        press(&mut app, "ctrl+s");
        assert!(menu_title(&app).contains("another program"));
        press(&mut app, "enter");
        assert_eq!(s.disk(), "чужое, длиннее\n", "cancel is the default");
        s.pass(&mut app, 3);
        press(&mut app, "esc");
        assert!(app.is_modified(), "own version is kept as unsaved");
        s.pass(&mut app, 5);
        assert!(app.dialog.is_none(), "asked once");
        press(&mut app, "ctrl+s");
        assert_eq!(s.disk(), "моё а\n");
        let backup = s.dir.path().join("a.md.bak");
        assert_eq!(fs::read_to_string(backup).unwrap(), "чужое, длиннее\n");
    }

    #[test]
    fn own_saves_touching_and_deleting() {
        let mut s = Session::new("а\n");
        let mut app = s.open();
        for _ in 0..3 {
            type_in(&mut app, "x");
            press(&mut app, "ctrl+s");
            s.pass(&mut app, 3);
            assert!(app.dialog.is_none(), "own save is not a foreign change");
        }
        fs::write(&s.path, s.disk()).unwrap();
        s.pass(&mut app, 3);
        assert!(app.dialog.is_none(), "same content is not a change");
        fs::remove_file(&s.path).unwrap();
        s.pass(&mut app, 3);
        assert!(app.is_modified() && app.message.as_deref().unwrap().contains("deleted"));
        press(&mut app, "ctrl+s");
        assert_eq!(s.disk(), "xxxа\n");
    }

    #[test]
    fn backup_is_made_once_per_session() {
        let mut s = Session::new("первое");
        let backup = s.dir.path().join("a.md.bak");
        let mut app = s.open();
        press(&mut app, "ctrl+s");
        assert!(!backup.exists(), "nothing changed, nothing to back up");
        type_in(&mut app, "1");
        press(&mut app, "ctrl+s");
        type_in(&mut app, "2");
        press(&mut app, "ctrl+s");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "первое");
        app.finish();
        let mut app = s.open();
        type_in(&mut app, "3");
        press(&mut app, "ctrl+s");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "12первое");
        fs::remove_file(&backup).unwrap();
        s.config.backup = false;
        let mut app = s.open();
        type_in(&mut app, "4");
        press(&mut app, "ctrl+s");
        assert!(!backup.exists());
    }

    #[test]
    fn failed_backup_is_reported_but_text_is_saved() {
        let s = Session::new("а");
        fs::create_dir(s.dir.path().join("a.md.bak")).unwrap();
        let mut app = s.open();
        type_in(&mut app, "x");
        press(&mut app, "ctrl+s");
        assert_eq!(s.disk(), "xа");
        assert!(app.message.as_deref().unwrap().contains("no .bak copy"));
    }

    #[cfg(unix)]
    #[test]
    fn backup_is_private_and_does_not_follow_links() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
        let s = Session::new("тайна");
        fs::set_permissions(&s.path, fs::Permissions::from_mode(0o600)).unwrap();
        let (backup, victim) = (s.dir.path().join("a.md.bak"), s.dir.path().join("victim"));
        fs::write(&victim, "не трогать").unwrap();
        symlink(&victim, &backup).unwrap();
        let mut app = s.open();
        type_in(&mut app, "x");
        press(&mut app, "ctrl+s");
        assert_eq!(fs::read_to_string(&victim).unwrap(), "не трогать");
        let meta = fs::symlink_metadata(&backup).unwrap();
        assert!(meta.is_file());
        assert_eq!(meta.mode() & 0o777, 0o600);
    }

    #[test]
    fn autosave_waits_for_a_pause() {
        let mut s = Session::new("а");
        s.config.autosave = Some(Duration::from_secs(3));
        let mut app = s.open();
        type_in(&mut app, "1");
        s.pass(&mut app, 3);
        type_in(&mut app, "2");
        s.pass(&mut app, 3);
        assert_eq!(s.disk(), "а");
        s.pass(&mut app, 2);
        assert_eq!(s.disk(), "12а");
        assert!(!app.is_modified() && app.message.is_none());
        press(&mut app, "ctrl+z");
        assert_eq!(app.buffer.text(), "а", "undo works after autosave");
    }

    #[test]
    fn autosave_stops_on_conflict_and_bad_encoding() {
        let mut s = Session::new("а");
        s.config.autosave = Some(Duration::from_secs(3));
        let mut app = s.open();
        type_in(&mut app, "x");
        fs::write(&s.path, "чужое, длиннее").unwrap();
        s.pass(&mut app, 4);
        press(&mut app, "esc");
        s.pass(&mut app, 10);
        assert_eq!(s.disk(), "чужое, длиннее");
        press(&mut app, "ctrl+s");
        assert_eq!(s.disk(), "xа");

        app.act(Action::SetFormat(Format::of(Enc::Cp866)));
        type_in(&mut app, "😀");
        s.pass(&mut app, 6);
        assert!(app.dialog.is_none() && app.is_modified());
        assert!(app.message.as_deref().unwrap().contains("Autosave"));
        assert_eq!(s.disk(), "xа");
    }

    #[test]
    fn unnamed_text_is_restored_too() {
        let dir = tempfile::tempdir().unwrap();
        let start = || {
            let options = Options {
                path: None,
                mode: Mode::Edit,
                lang: Lang::En,
                encoding: None,
                line: None,
                config: Config::default(),
                warnings: Vec::new(),
                state_dir: Some(dir.path().into()),
            };
            App::open(options, Box::<Local>::default(), Box::<Opened>::default()).unwrap()
        };
        let mut app = start();
        type_in(&mut app, "x");
        app.emergency_save().unwrap();
        drop(app);
        let mut app = start();
        press(&mut app, "enter");
        assert_eq!(app.buffer.text(), "x");
    }

    #[test]
    fn config_is_applied() {
        let mut s = Session::new("a\tb");
        let text = "theme = \"light\"\n[editor]\ntab_width = 8\n[keys]\ntoggle_mode = [\"f9\"]\n";
        s.config = crate::config::parse(text).0;
        let mut app = s.open();
        assert_eq!(app.view.settings.tab_width, 8);
        assert_eq!(app.theme, Theme::LIGHT);
        press(&mut app, "f5");
        assert_eq!(app.mode, Mode::Edit);
        press(&mut app, "f9");
        assert_eq!(app.mode, Mode::View);
    }

    #[test]
    fn directory_is_an_error() {
        let options = Options {
            path: Some(std::env::temp_dir()),
            mode: Mode::Edit,
            lang: Lang::En,
            encoding: None,
            line: None,
            config: Config::default(),
            warnings: Vec::new(),
            state_dir: None,
        };
        assert!(App::open(options, Box::<Local>::default(), Box::<Opened>::default()).is_err());
    }
}
