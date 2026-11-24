//! Состояние программы и реакция на ввод. Отрисовки и терминала здесь нет.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::buffer::{Buffer, Eol};
use crate::clipboard::Clipboard;
use crate::command::{Command, Mode};
use crate::dialog::{Action, Ask, Dialog, Field, Menu, Outcome, SaveJob};
use crate::encoding::{self, Enc, Format, Unencodable};
use crate::file::{self, ReadError};
use crate::i18n::Lang;
use crate::input::Input;
use crate::key::{Chord, Key};
use crate::keymap::Keymap;
use crate::mdline::States;
use crate::pane::Pane;
use crate::view::{Place, Settings, Viewport};

const WHEEL_LINES: isize = 3;
/// Время на подсветку кода за одну отрисовку.
const HIGHLIGHT_BUDGET: Duration = Duration::from_millis(15);
/// Шире этого текст просмотра не растягивается.
const PREVIEW_WIDTH: usize = 100;

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
    clipboard: Box<dyn Clipboard>,
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
}

fn load(path: &Path, forced: Option<Enc>, lang: Lang) -> io::Result<Loaded> {
    let s = lang.strings();
    let fail = |reason: String| io::Error::other(format!("{}: {reason}", path.display()));
    match file::read(path) {
        Ok(Some(bytes)) => {
            let decoded = encoding::decode(&bytes, forced);
            Ok(Loaded {
                text: decoded.text,
                format: decoded.format,
                lossy: decoded.lossy,
                large: bytes.len() > file::LARGE,
            })
        }
        Ok(None) => Ok(Loaded {
            text: String::new(),
            format: forced.map_or(Format::UTF8, Format::of),
            lossy: false,
            large: false,
        }),
        Err(ReadError::Directory) => Err(fail(s.is_directory.into())),
        Err(ReadError::NotRegular) => Err(fail(s.not_regular.into())),
        Err(ReadError::Io(e)) => Err(fail(e.to_string())),
    }
}

impl App {
    pub fn open(options: Options, clipboard: Box<dyn Clipboard>) -> io::Result<Self> {
        let lang = options.lang;
        let loaded = match &options.path {
            Some(path) => load(path, options.encoding, lang)?,
            None => Loaded {
                text: String::new(),
                format: options.encoding.map_or(Format::UTF8, Format::of),
                lossy: false,
                large: false,
            },
        };
        let mut app = Self {
            path: options.path,
            buffer: Buffer::new(""),
            view: Viewport::new(Settings::default()),
            pane: Pane::default(),
            states: States::default(),
            mode: options.mode,
            lang,
            keymap: Keymap::default(),
            message: None,
            dialog: None,
            help: false,
            should_quit: false,
            format: Format::UTF8,
            saved_format: Format::UTF8,
            lossy: false,
            clipboard,
            area: TextArea::default(),
            goal: None,
            follow: true,
            dragging: false,
        };
        app.set_content(loaded);
        if let Some(line) = options.line {
            app.goto_line(line);
        }
        if app.mode == Mode::View {
            app.enter_view();
        }
        Ok(app)
    }

    fn set_content(&mut self, loaded: Loaded) {
        let s = self.lang.strings();
        self.buffer = Buffer::new(&loaded.text);
        self.format = loaded.format;
        self.saved_format = loaded.format;
        self.lossy = loaded.lossy;
        self.goal = None;
        self.follow = true;
        self.view.clamp(&self.buffer);
        if loaded.lossy {
            self.message = Some(s.lossy_warning.into());
        } else if loaded.large {
            self.message = Some(s.large_warning.into());
        }
    }

    pub fn is_modified(&self) -> bool {
        self.buffer.is_modified() || self.format != self.saved_format
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
        let width = width.min(PREVIEW_WIDTH);
        let tab = self.view.settings.tab_width;
        self.pane.prepare(&self.buffer, width, height, tab);
        self.pane.highlight(Instant::now() + HIGHLIGHT_BUDGET);
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
            Some(Dialog::Menu(_)) => {}
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
                self.buffer.type_text("\n");
                self.edited();
            }
            Key::Tab if mods.is_none() => {
                let tab = self.view.settings.tab_width;
                let (_, column) = self.cursor_line_column();
                self.buffer.type_text(&" ".repeat(tab - (column - 1) % tab));
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
            Key::Esc => self.buffer.clear_selection(),
            _ => {}
        }
    }

    fn mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.wheel(-WHEEL_LINES),
            MouseEventKind::ScrollDown => self.wheel(WHEEL_LINES),
            MouseEventKind::Down(MouseButton::Left) => {
                let a = self.area;
                let inside = (a.y..a.y + a.height).contains(&mouse.row);
                if self.mode == Mode::Edit && self.dialog.is_none() && !self.help && inside {
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
            Command::Quit if self.is_modified() => self.ask_quit(),
            Command::Quit => self.should_quit = true,
            Command::Save => self.save(false),
            Command::SaveAs => self.ask_path(false),
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
            other => {
                let name = self.lang.command(other);
                self.message = Some(format!("{name}: {}", s.not_implemented));
            }
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
        let pos = self.buffer.pos_of_byte(self.pane.source_at_top());
        self.view.top = self.view.locate(&self.buffer, pos).0;
        self.view.clamp(&self.buffer);
        if !self.view.is_visible(&self.buffer, self.buffer.cursor()) {
            self.move_to(pos, false);
            self.follow = false;
        }
    }

    // --- Запросы ---

    fn menu(&mut self, title: &str, items: Vec<(String, Action)>) {
        self.dialog = Some(Dialog::Menu(Menu::new(title, items)));
    }

    fn ask_quit(&mut self) {
        let s = self.lang.strings();
        self.menu(
            s.ask_quit,
            vec![
                (s.save.into(), Action::SaveAndQuit),
                (s.dont_save.into(), Action::QuitWithoutSaving),
                (s.cancel.into(), Action::Cancel),
            ],
        );
    }

    fn ask_path(&mut self, quit: bool) {
        let path = self.path.as_ref();
        let text = path.map_or(String::new(), |p| p.display().to_string());
        let title = self.lang.strings().ask_path;
        let field = Field::new(Ask::SavePath { quit }, title, text);
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
                self.submit(ask, text.trim());
            }
            Outcome::Act(action) => {
                self.dialog = None;
                self.act(action);
            }
        }
    }

    fn submit(&mut self, ask: Ask, text: &str) {
        match ask {
            Ask::GotoLine => match text.parse() {
                Ok(line) => self.goto_line(line),
                Err(_) => self.message = Some(self.lang.strings().bad_number.into()),
            },
            Ask::SavePath { .. } if text.is_empty() => {}
            Ask::SavePath { quit } => self.write(SaveJob {
                path: PathBuf::from(text),
                format: self.format,
                quit,
                overwrite: false,
                allow_lossy: false,
            }),
        }
    }

    fn act(&mut self, action: Action) {
        let s = self.lang.strings();
        match action {
            Action::Cancel => {}
            Action::Save(job) => self.write(job),
            Action::SaveAndQuit => self.save(true),
            Action::QuitWithoutSaving => self.should_quit = true,
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

    fn save(&mut self, quit: bool) {
        match self.path.clone() {
            Some(path) => self.write(SaveJob {
                path,
                format: self.format,
                quit,
                overwrite: true,
                allow_lossy: false,
            }),
            None => self.ask_path(quit),
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
        match file::write(&job.path, &bytes) {
            Ok(()) => {
                self.message = Some(format!("{}: {}", s.saved, job.path.display()));
                self.path = Some(job.path);
                self.format = job.format;
                self.saved_format = job.format;
                self.lossy = false;
                self.buffer.mark_saved();
                self.should_quit = job.quit;
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

    fn open(path: Option<PathBuf>, encoding: Option<Enc>) -> App {
        let options = Options {
            path,
            mode: Mode::Edit,
            lang: Lang::En,
            encoding,
            line: None,
        };
        let mut app = App::open(options, Box::<Local>::default()).unwrap();
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

    #[test]
    fn directory_is_an_error() {
        let options = Options {
            path: Some(std::env::temp_dir()),
            mode: Mode::Edit,
            lang: Lang::En,
            encoding: None,
            line: None,
        };
        assert!(App::open(options, Box::<Local>::default()).is_err());
    }
}
