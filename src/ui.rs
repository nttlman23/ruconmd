//! Отрисовка.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, List, ListState, Paragraph};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, TextArea};
use crate::buffer::Eol;
use crate::command::{Command, Mode};
use crate::dialog::{Dialog, Field, Menu};
use crate::layout;

const BAR: Style = Style::new().add_modifier(Modifier::REVERSED);
const SELECTED: Style = Style::new().add_modifier(Modifier::REVERSED);
const GUTTER: Style = Style::new().fg(Color::DarkGray);
/// Уже этого номера строк не показываются.
const MIN_TEXT_WIDTH: u16 = 20;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let digits = app.buffer.line_count().to_string().len() as u16;
    let mut gutter = if app.view.settings.line_numbers {
        digits + 1
    } else {
        0
    };
    if body.width < gutter + MIN_TEXT_WIDTH {
        gutter = 0;
    }
    let area = TextArea {
        x: body.x + gutter,
        y: body.y,
        width: body.width - gutter,
        height: body.height,
    };
    app.prepare(area);

    frame.render_widget(
        Paragraph::new(header_text(app, header.width)).style(BAR),
        header,
    );
    if area.width > 0 && area.height > 0 {
        draw_text(frame, app, area, gutter);
    }
    match &app.dialog {
        Some(Dialog::Field(field)) => draw_field(frame, field, footer),
        _ => frame.render_widget(Paragraph::new(footer_text(app)).style(BAR), footer),
    }
    match &app.dialog {
        Some(Dialog::Menu(menu)) => draw_menu(frame, menu, body),
        _ if app.help => draw_help(frame, app, body),
        _ => {}
    }
}

/// Признак изменений стоит первым, а у длинного пути виден конец: имя файла.
fn header_text(app: &App, width: u16) -> String {
    let name = match &app.path {
        Some(p) => printable(&p.display().to_string()),
        None => app.lang.strings().untitled.into(),
    };
    let mark = if app.is_modified() { '*' } else { ' ' };
    let room = (width as usize).saturating_sub(2);
    if name.width() <= room {
        return format!("{mark}{name}");
    }
    let mut tail = name.as_str();
    while tail.width() + 1 > room && !tail.is_empty() {
        let mut graphemes = tail.graphemes(true);
        graphemes.next();
        tail = graphemes.as_str();
    }
    format!("{mark}…{tail}")
}

fn draw_text(frame: &mut Frame, app: &App, area: TextArea, gutter: u16) {
    let view = &app.view;
    let tab = view.settings.tab_width;
    let edit = app.mode == Mode::Edit;
    let selection = app.buffer.selection().filter(|_| edit).unwrap_or(0..0);
    let cursor = app.buffer.cursor();

    let mut y = area.y;
    let bottom = area.y + area.height;
    let right = area.x + area.width;
    let (mut line, mut first) = (view.top.line, view.top.row);
    while y < bottom && line < app.buffer.line_count() {
        let laid = view.lay_out(&app.buffer, line);
        let start = app.buffer.line_start(line);
        for (index, row) in laid.rows.iter().enumerate().skip(first) {
            if y >= bottom {
                break;
            }
            let buf = frame.buffer_mut();
            if index == 0 && gutter > 0 {
                let number = format!("{:>1$}", line + 1, gutter as usize - 1);
                buf.set_string(area.x - gutter, y, number, GUTTER);
            }
            let mut end_column = 0;
            let mut cursor_x = None;
            for g in layout::glyphs(&laid.text, *row, tab) {
                let pos = start + g.offset;
                if pos == cursor {
                    cursor_x = Some(g.column);
                }
                end_column = g.column + g.width;
                let style = if selection.contains(&pos) {
                    SELECTED
                } else {
                    Style::new()
                };
                if g.column < view.left {
                    continue;
                }
                let x = area.x as usize + g.column - view.left;
                if x + g.width > right as usize {
                    continue;
                }
                buf[(x as u16, y)].set_symbol(g.symbol()).set_style(style);
                for extra in 1..g.width {
                    let cell = &mut buf[((x + extra) as u16, y)];
                    // Под широким символом ячейка пустая, под табуляцией пробел.
                    cell.set_symbol(" ").set_style(style);
                }
            }
            let last = index + 1 == laid.rows.len();
            let end = start + row.end;
            if last && cursor == end {
                cursor_x = Some(end_column);
            }
            // Выделенный перевод строки виден как одна ячейка.
            if last && selection.contains(&end) && end_column >= view.left {
                let x = area.x as usize + end_column - view.left;
                if x < right as usize {
                    buf[(x as u16, y)].set_style(SELECTED);
                }
            }
            if let Some(column) = cursor_x
                && edit
                && app.dialog.is_none()
                && !app.help
                && column >= view.left
            {
                let x = (area.x as usize + column - view.left).min(right as usize - 1);
                frame.set_cursor_position(Position::new(x as u16, y));
            }
            y += 1;
        }
        line += 1;
        first = 0;
    }
}

fn footer_text(app: &App) -> String {
    let mode = app.lang.mode(app.mode);
    if let Some(message) = &app.message {
        return format!(" {mode} | {}", printable(message));
    }
    let s = app.lang.strings();
    let place = match app.mode {
        Mode::Edit => {
            let (line, column) = app.cursor_line_column();
            format!("{line}:{column}")
        }
        Mode::View => format!("{}/{}", app.view.top.line + 1, app.buffer.line_count()),
    };
    let eol = match (app.buffer.ends.mixed, app.buffer.ends.eol) {
        (true, _) => "LF+CRLF",
        (_, Eol::Lf) => "LF",
        (_, Eol::Crlf) => "CRLF",
    };
    let keys = |c| app.keymap.chords(c).join("/");
    format!(
        " {mode} | {place} | {} | {eol} | {}: {} | {}: {} | {}: {}",
        app.format,
        keys(Command::ToggleMode),
        app.lang.mode(app.mode.toggled()).to_lowercase(),
        keys(Command::Help),
        s.help,
        keys(Command::Quit),
        s.quit,
    )
}

fn draw_field(frame: &mut Frame, field: &Field, area: Rect) {
    let label = format!(" {}: ", field.title);
    let before = label.width() + field.text[..field.cursor].width();
    // Длинный текст сдвигается, чтобы курсор оставался виден.
    let shift = (before + 1).saturating_sub(area.width as usize);
    let text = format!("{label}{}", field.text);
    let line = Paragraph::new(text).style(BAR).scroll((0, shift as u16));
    frame.render_widget(line, area);
    if area.width > 0 && area.height > 0 {
        let x = area.x + (before - shift).min(area.width as usize - 1) as u16;
        frame.set_cursor_position(Position::new(x, area.y));
    }
}

fn popup(area: Rect, width: usize, rows: usize) -> Rect {
    let width = (width.min(u16::MAX as usize) as u16).min(area.width);
    let height = (rows as u16 + 2).min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn draw_menu(frame: &mut Frame, menu: &Menu, area: Rect) {
    let title = format!(" {} ", printable(&menu.title));
    let widest = menu.items.iter().map(|(name, _)| name.width()).max();
    let width = widest.unwrap_or(0).max(title.width()) + 4;
    let rect = popup(area, width, menu.items.len());
    let items = menu.items.iter().map(|(name, _)| format!(" {name} "));
    let list = List::new(items)
        .block(Block::bordered().title(title))
        .highlight_style(SELECTED);
    let mut state = ListState::default().with_selected(Some(menu.selected));
    frame.render_widget(Clear, rect);
    frame.render_stateful_widget(list, rect, &mut state);
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let rows: Vec<Line> = Command::ALL
        .iter()
        .map(|c| {
            let keys = app.keymap.chords(*c).join(", ");
            Line::from(format!(" {keys:<14} {}", app.lang.command(*c)))
        })
        .collect();
    let rect = popup(area, 40, rows.len());
    let block = Block::bordered().title(app.lang.strings().help_title);
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(rows).block(block), rect);
}

/// Строка для вывода вне области текста: без управляющих символов.
pub fn printable(text: &str) -> String {
    let row = layout::rows(text, None, 4)[0];
    layout::glyphs(text, row, 4)
        .map(|g| if g.text == "\t" { " " } else { g.symbol() }.to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Options;
    use crate::buffer::Buffer;
    use crate::clipboard::Local;
    use crate::i18n::Lang;
    use crate::input::Input;
    use ratatui::Terminal;
    use ratatui::backend::{Backend, TestBackend};

    fn app(text: &str) -> App {
        let options = Options {
            path: None,
            mode: Mode::Edit,
            lang: Lang::Ru,
            encoding: None,
            line: None,
        };
        let mut app = App::open(options, Box::<Local>::default()).unwrap();
        app.buffer = Buffer::new(text);
        app
    }

    fn terminal(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal
    }

    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let terminal = terminal(app, width, height);
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    fn press(app: &mut App, key: &str) {
        app.handle(Input::Key(key.parse().unwrap()));
    }

    #[test]
    fn shows_mode_text_and_hints() {
        let mut app = app("# Привет\nмир\n");
        let rows = screen(&mut app, 100, 6);
        assert!(rows[0].starts_with(" без имени "));
        assert!(rows[1].starts_with("1 # Привет"));
        assert!(rows[5].contains("ПРАВКА | 1:1 | UTF-8 | LF"));
        assert!(rows[5].contains("F5/Ctrl+E"));
        press(&mut app, "f5");
        assert!(screen(&mut app, 100, 6)[5].contains("ПРОСМОТР"));
    }

    #[test]
    fn marks_unsaved_text() {
        let mut app = app("");
        press(&mut app, "enter");
        assert!(screen(&mut app, 40, 4)[0].starts_with("*без имени "));
    }

    #[test]
    fn long_path_keeps_the_mark_and_the_file_name() {
        let mut app = app("");
        app.path = Some("каталог/".repeat(20).into());
        app.path.as_mut().unwrap().push("заметка.md");
        press(&mut app, "enter");
        let row = &screen(&mut app, 40, 4)[0];
        assert!(
            row.starts_with("*…") && row.trim_end().ends_with("/заметка.md"),
            "{row}"
        );
    }

    #[test]
    fn wraps_long_lines_and_places_the_cursor() {
        let mut app = app("один два три четыре пять шесть");
        press(&mut app, "ctrl+end");
        let mut t = terminal(&mut app, 24, 6);
        let buffer = t.backend().buffer();
        let row = |y| (0..24).map(|x| buffer[(x, y)].symbol()).collect::<String>();
        assert!(row(1).starts_with("1 один два три четыре "));
        assert!(row(2).starts_with("  пять шесть"));
        assert_eq!(
            t.backend_mut().get_cursor_position().unwrap(),
            Position::new(12, 2)
        );
    }

    #[test]
    fn selection_is_highlighted() {
        let mut app = app("ab\ncd");
        terminal(&mut app, 30, 4);
        press(&mut app, "shift+down");
        let t = terminal(&mut app, 30, 4);
        let on = |x, y| {
            t.backend().buffer()[(x, y)]
                .modifier
                .contains(Modifier::REVERSED)
        };
        assert!(on(2, 1) && on(3, 1) && on(4, 1), "text and line break");
        assert!(!on(5, 1) && !on(2, 2));
    }

    #[test]
    fn survives_any_window_size() {
        let mut app = app("текст 日本語 e\u{301}\tконец\n\n");
        for (w, h) in (1..30).flat_map(|w| (1..6).map(move |h| (w, h))) {
            for setup in 0..4 {
                app.help = setup == 1;
                app.dialog = None;
                if setup == 2 {
                    press(&mut app, "f8");
                } else if setup == 3 {
                    press(&mut app, "f12");
                }
                screen(&mut app, w, h);
            }
            app.dialog = None;
            app.help = false;
            press(&mut app, "ctrl+end");
            screen(&mut app, w, h);
            press(&mut app, "ctrl+home");
        }
    }

    #[test]
    fn menu_and_field_are_shown() {
        let mut app = app("x");
        press(&mut app, "f8");
        let rows = screen(&mut app, 60, 24).join("\n");
        assert!(rows.contains("CP1251") && rows.contains("CRLF"), "{rows}");
        press(&mut app, "esc");
        press(&mut app, "f12");
        assert!(screen(&mut app, 60, 6)[5].contains("Имя файла"));
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        assert_eq!(printable("a\x1b[31m\u{202e}\tb"), "a\u{fffd}[31m\u{fffd} b");
        let mut app = app("a\x1b[31mb");
        assert!(screen(&mut app, 40, 4).iter().all(|r| !r.contains('\x1b')));
    }
}
