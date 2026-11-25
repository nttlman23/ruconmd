//! Отрисовка.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph, Widget};
use ratatui_image::sliced::{SignedPosition, SlicedImage};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::browse::PanelPlace;
use crate::app::{App, TextArea};
use crate::buffer::Eol;
use crate::command::{Command, Mode};
use crate::dialog::{Dialog, Field, Menu};
use crate::files::Kind;
use crate::layout;
use crate::mdline;
use crate::pane::MARGIN;
use crate::preview::Item;
use crate::settings::Item as Setting;
use crate::style::{Mark, Tone};

const BAR: Style = Style::new().add_modifier(Modifier::REVERSED);
const SELECTED: Style = Style::new().add_modifier(Modifier::REVERSED);
/// Уже этого номера строк не показываются.
const MIN_TEXT_WIDTH: u16 = 20;
/// Совпадения поиска.
const MATCH: Style = Style::new().fg(Color::Black).bg(Color::Yellow);
const CURRENT_MATCH: Style = Style::new().fg(Color::Black).bg(Color::LightGreen);
/// В более длинных строках редактора совпадения не подсвечиваются.
const MATCH_MAX_LINE: usize = 64 * 1024;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let screen = frame.area();
    frame.buffer_mut().set_style(screen, app.theme.base());

    let work = body;
    let place = app.panel_place(work.width);
    let (panel, body) = match place {
        PanelPlace::Hidden => (None, work),
        PanelPlace::Full => (Some(work), work),
        PanelPlace::Side(width) => {
            let [panel, body] =
                Layout::horizontal([Constraint::Length(width), Constraint::Min(0)]).areas(work);
            (Some(panel), body)
        }
    };
    let covered = place == PanelPlace::Full;
    let panel_area = panel.map(|p| TextArea {
        x: p.x,
        y: p.y,
        width: p.width,
        height: p.height,
    });
    app.prepare_files(panel_area, body.x, covered);

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
    if app.mode == Mode::View {
        app.prepare_preview(
            body.width.saturating_sub(MARGIN) as usize,
            body.height as usize,
        );
    }

    frame.render_widget(
        Paragraph::new(header_text(app, header.width)).style(BAR),
        header,
    );
    if let Some(panel) = panel {
        draw_files(frame, app, panel);
    }
    if body.width > 0 && body.height > 0 && !covered {
        match app.mode {
            Mode::Edit => draw_text(frame, app, area, gutter),
            Mode::View => draw_preview(frame, app, body),
        }
    }
    match &app.dialog {
        Some(Dialog::Field(field)) => draw_field(frame, field, footer),
        Some(Dialog::Replace(_)) => {
            let text = format!(" {}", app.lang.strings().replace_hint);
            frame.render_widget(Paragraph::new(text).style(BAR), footer);
        }
        _ => frame.render_widget(Paragraph::new(footer_text(app)).style(BAR), footer),
    }
    match &app.dialog {
        Some(Dialog::Menu(menu)) => draw_menu(frame, menu, work, app.theme.base()),
        Some(Dialog::Settings(p)) => draw_settings(frame, app, p.selected, work),
        _ if app.help => draw_help(frame, app, work),
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
    format!("{mark}{}", tail(&name, room))
}

/// Конец строки, если целиком она не помещается.
fn tail(text: &str, room: usize) -> String {
    if text.width() <= room {
        return text.to_owned();
    }
    let mut tail = text;
    while tail.width() + 1 > room && !tail.is_empty() {
        let mut graphemes = tail.graphemes(true);
        graphemes.next();
        tail = graphemes.as_str();
    }
    format!("…{tail}")
}

/// Начало строки, если целиком она не помещается.
fn head(text: &str, room: usize) -> String {
    if text.width() <= room {
        return text.to_owned();
    }
    let mut head = text;
    while head.width() + 1 > room && !head.is_empty() {
        let last = head.graphemes(true).next_back().map_or(0, str::len);
        head = &head[..head.len() - last];
    }
    format!("{head}…")
}

fn draw_files(frame: &mut Frame, app: &App, area: Rect) {
    let Some(browser) = &app.browser else { return };
    if area.width < 2 || area.height == 0 {
        return;
    }
    let s = app.lang.strings();
    let focused = app.files_focused();
    let dim = app.theme.style(Mark::tone(Tone::Dim));
    let heading = app.theme.style(Mark::tone(Tone::Heading).bold());
    let border = area.x + area.width - 1;
    let room = area.width as usize - 1;
    let current = app.path.as_deref();
    let current = current.and_then(|p| std::path::absolute(p).ok());
    let buf = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        buf.set_string(border, y, "│", dim);
    }
    let root = printable(&browser.tree.root().display().to_string());
    buf.set_string(area.x, area.y, tail(&root, room), heading);
    for (number, (row, selected)) in browser.tree.visible().enumerate() {
        let y = area.y + 1 + number as u16;
        if y >= area.y + area.height {
            break;
        }
        let indent = "  ".repeat(row.depth.min(room / 4));
        let (sign, name) = match row.kind {
            Kind::Up => ("", "..".to_owned()),
            Kind::Dir { open: true } => ("▾ ", printable(&row.name)),
            Kind::Dir { open: false } => ("▸ ", printable(&row.name)),
            Kind::File => ("  ", printable(&row.name)),
            Kind::Unreadable => ("  ", format!("({})", s.files_unreadable)),
            Kind::Cut => ("  ", format!("({})", s.files_cut)),
        };
        let mut style = match row.kind {
            Kind::Unreadable | Kind::Cut => dim,
            Kind::Dir { .. } | Kind::Up => app.theme.style(Mark::tone(Tone::Link)),
            Kind::File if Some(&row.path) == current.as_ref() => heading,
            Kind::File => app.theme.style(Mark::TEXT),
        };
        if selected && focused {
            style = style.patch(SELECTED);
        }
        let text = head(&format!(" {indent}{sign}{name}"), room);
        buf.set_string(area.x, y, &text, style);
        let end = area.x + text.width() as u16;
        for x in end..border {
            buf[(x, y)].set_style(style);
        }
    }
}

fn draw_settings(frame: &mut Frame, app: &App, selected: usize, area: Rect) {
    let s = app.lang.strings();
    // У параметра с клавишей она видна рядом.
    let title = |item: Setting| {
        let name = app.lang.setting(item);
        match item {
            Setting::LineNumbers => match first_key(app, Command::LineNumbers) {
                key if key.is_empty() => name.to_owned(),
                key => format!("{name} ({key})"),
            },
            _ => name.to_owned(),
        }
    };
    let name_width = |item: &Setting| title(*item).width();
    let names = Setting::ALL.iter().map(name_width).max().unwrap_or(0);
    const VALUE: usize = 16;
    let width = names + VALUE + 7;
    let rect = popup(area, width, Setting::ALL.len() + 1);
    let room = (rect.width as usize).saturating_sub(4);
    // На низком экране видна часть списка вокруг выбранного.
    let rows = rect.height.saturating_sub(3).max(1) as usize;
    let first = selected
        .saturating_sub(rows / 2)
        .min(Setting::ALL.len().saturating_sub(rows));
    let mut lines: Vec<Line> = Setting::ALL
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .map(|(index, item)| {
            let name = title(*item);
            let value = printable(&app.lang.setting_value(*item, &app.values));
            let value = head(&value, VALUE);
            let gap = " ".repeat(names.saturating_sub(name.width()) + 2);
            let style = if index == selected {
                SELECTED
            } else {
                Style::new()
            };
            Line::styled(format!(" {name}{gap}{value} "), style)
        })
        .collect();
    let path = app.config_path.as_ref();
    let path = path.map_or(String::new(), |p| printable(&p.display().to_string()));
    let dim = app.theme.style(Mark::tone(Tone::Dim));
    lines.push(Line::styled(format!(" {}", tail(&path, room)), dim));
    let block = Block::bordered()
        .title(format!(" {} ", s.settings_title))
        .title_bottom(format!(" {} ", s.settings_hint));
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).style(app.theme.base()).block(block),
        rect,
    );
}

fn draw_text(frame: &mut Frame, app: &mut App, area: TextArea, gutter: u16) {
    let theme = &app.theme;
    let view = &app.view;
    let tab = view.settings.tab_width;
    let selection = app.buffer.selection().unwrap_or(0..0);
    let cursor = app.buffer.cursor();

    let mut y = area.y;
    let bottom = area.y + area.height;
    let right = area.x + area.width;
    let (mut line, mut first) = (view.top.line, view.top.row);
    while y < bottom && line < app.buffer.line_count() {
        let laid = view.lay_out(&app.buffer, line);
        let start = app.buffer.line_start(line);
        let marks = mdline::marks(app.states.at(&app.buffer, line), &laid.text);
        let hits: Vec<_> = match &app.search {
            Some(search) if laid.text.len() <= MATCH_MAX_LINE => {
                search.matches(&laid.text).collect()
            }
            _ => Vec::new(),
        };
        for (index, row) in laid.rows.iter().enumerate().skip(first) {
            if y >= bottom {
                break;
            }
            let buf = frame.buffer_mut();
            if index == 0 && gutter > 0 {
                let number = format!("{:>1$}", line + 1, gutter as usize - 1);
                buf.set_string(
                    area.x - gutter,
                    y,
                    number,
                    theme.style(Mark::tone(Tone::Dim)),
                );
            }
            let mut end_column = 0;
            let mut cursor_x = None;
            let mut byte = row.byte_start;
            for g in layout::glyphs(&laid.text, *row, tab) {
                let pos = start + g.offset;
                if pos == cursor {
                    cursor_x = Some(g.column);
                }
                end_column = g.column + g.width;
                let mut style = theme.style(marks.get(byte).copied().unwrap_or_default());
                if hits.iter().any(|h| h.contains(&byte)) {
                    style = MATCH;
                }
                byte += g.text.len();
                if selection.contains(&pos) {
                    style = style.patch(SELECTED);
                }
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
                && app.dialog.is_none()
                && !app.help
                && !app.files_focused()
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

fn draw_preview(frame: &mut Frame, app: &App, area: Rect) {
    let tab = app.view.settings.tab_width;
    let right = area.x + area.width;
    let buf = frame.buffer_mut();
    let mut put = |x: &mut u16, y: u16, text: &str, style: &dyn Fn(usize) -> Style| {
        let row = layout::rows(text, None, tab)[0];
        let mut byte = 0;
        for g in layout::glyphs(text, row, tab) {
            let style = style(byte);
            byte += g.text.len();
            if *x as usize + g.width > right as usize {
                *x = right;
                return;
            }
            buf[(*x, y)].set_symbol(g.symbol()).set_style(style);
            for extra in 1..g.width as u16 {
                buf[(*x + extra, y)].set_symbol(" ").set_style(style);
            }
            *x += g.width as u16;
        }
    };
    let selected = app.pane.selected_id();
    let on_screen = app.images_on_screen();
    for (row, line) in app.pane.visible().iter().enumerate() {
        let y = area.y + row as u16;
        let mut x = area.x + MARGIN;
        let mut offset = 0;
        let hits = app.pane.hits(row);
        let found = |style: Style, byte: usize| match hits.iter().find(|(h, _)| h.contains(&byte)) {
            Some((_, true)) => CURRENT_MATCH,
            Some((_, false)) => MATCH,
            None => style,
        };
        for span in &line.spans {
            let mut style = app.theme.style(span.mark);
            if span.item.is_some() && span.item == selected {
                style = style.patch(SELECTED);
            }
            put(&mut x, y, &span.text, &|byte| found(style, offset + byte));
            offset += span.text.len();
        }
        // Место картинки, которой ещё нет.
        if let Some(part) = line.image
            && part.row == 0
            && !on_screen.iter().any(|s| s.image == part.image)
        {
            let (alt, failed) = app.pane.image_note(part.image);
            let note = if failed { "✗" } else { "…" };
            let style = app.theme.style(Mark::tone(Tone::Dim));
            put(&mut x, y, &format!("▣ {alt} {note}"), &|_| style);
        }
        if let Some(text) = app.pane.code_text(line) {
            put(&mut x, y, text, &|byte| {
                found(
                    app.theme.style(app.pane.code_mark(line, byte)),
                    offset + byte,
                )
            });
        }
    }
    for place in on_screen {
        if let Some(picture) = app.pane.picture(place.image) {
            let x = place.x.min(area.width);
            let target = Rect {
                x: area.x + x,
                width: area.width - x,
                ..area
            };
            let position = SignedPosition { x: 0, y: place.y };
            SlicedImage::new(picture, position).render(target, buf);
        }
    }
}

fn footer_text(app: &App) -> String {
    let mode = app.lang.mode(app.mode);
    if let Some(message) = &app.message {
        return format!(" {mode} | {}", printable(message));
    }
    let s = app.lang.strings();
    if app.files_focused() {
        return format!(" {}", s.files_hint);
    }
    if app.mode == Mode::View && !app.pane.is_ready() {
        return format!(" {mode} | {}", app.preview_note());
    }
    if app.mode == Mode::View {
        match app.pane.selected_item() {
            Some(Item::Link(url)) => {
                return format!(" {mode} | Enter: {}", printable(url));
            }
            Some(Item::Task { .. }) => {
                let key = app.keymap.chords(Command::ToggleCheckbox).join("/");
                let name = app.lang.command(Command::ToggleCheckbox);
                return format!(" {mode} | {key}: {name}");
            }
            None => {}
        }
    }
    let place = match app.mode {
        Mode::Edit => {
            let (line, column) = app.cursor_line_column();
            format!("{line}:{column}")
        }
        Mode::View => {
            let pos = app.buffer.pos_of_byte(app.pane.source_at_top());
            format!(
                "{}/{}",
                app.buffer.line_of(pos) + 1,
                app.buffer.line_count()
            )
        }
    };
    let eol = match (app.buffer.ends.mixed, app.buffer.ends.eol) {
        (true, _) => "LF+CRLF",
        (_, Eol::Lf) => "LF",
        (_, Eol::Crlf) => "CRLF",
    };
    // Сначала главное: на узком экране конец не виден.
    let mut text = format!(" {mode} | {place} | {} | {eol}", app.format);
    let toggled = app.lang.mode(app.mode.toggled()).to_lowercase();
    let command = |c| app.lang.command(c).to_lowercase();
    for (keys, name) in [
        (first_key(app, Command::ToggleMode), toggled),
        (first_key(app, Command::Help), s.help.to_owned()),
        (
            app.keymap.chords(Command::Quit).join("/"),
            s.quit.to_owned(),
        ),
        (
            first_key(app, Command::Settings),
            command(Command::Settings),
        ),
        (first_key(app, Command::Files), command(Command::Files)),
    ] {
        if !keys.is_empty() {
            text += &format!(" | {keys}: {name}");
        }
    }
    text
}

fn first_key(app: &App, command: Command) -> String {
    app.keymap
        .chords(command)
        .into_iter()
        .next()
        .unwrap_or_default()
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
    let height = (rows.min(u16::MAX as usize - 2) as u16 + 2).min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn draw_menu(frame: &mut Frame, menu: &Menu, area: Rect, base: Style) {
    let title = format!(" {} ", printable(&menu.title));
    let widest = menu.items.iter().map(|(name, _)| name.width()).max();
    let width = widest.unwrap_or(0).max(title.width()) + 4;
    let rect = popup(area, width, menu.items.len());
    // В оглавлении большого файла тысячи строк, рисуются только видимые.
    let rows = rect.height.saturating_sub(2) as usize;
    let first = menu
        .selected
        .saturating_sub(rows / 2)
        .min(menu.items.len().saturating_sub(rows));
    let lines: Vec<Line> = menu
        .items
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .map(|(index, (name, _))| {
            let style = if index == menu.selected {
                SELECTED
            } else {
                Style::new()
            };
            Line::styled(format!(" {} ", printable(name)), style)
        })
        .collect();
    let list = Paragraph::new(lines)
        .style(base)
        .block(Block::bordered().title(title));
    frame.render_widget(Clear, rect);
    frame.render_widget(list, rect);
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    const COLUMN: usize = 38;
    let rows: Vec<String> = Command::ALL
        .iter()
        .map(|c| {
            let keys = app.keymap.chords(*c).join(", ");
            format!(" {keys:<14} {}", app.lang.command(*c))
        })
        .collect();
    // Не помещается по высоте: вторая колонка, если хватает ширины.
    let high = rows.len() + 2 <= area.height as usize;
    let wide = area.width as usize >= 2 * COLUMN + 2;
    let (columns, count) = match high || !wide {
        true => (1, rows.len()),
        false => (2, rows.len().div_ceil(2)),
    };
    let rows: Vec<Line> = (0..count)
        .map(|y| {
            let mut line = rows[y].clone();
            if let Some(second) = rows.get(y + count) {
                let gap = COLUMN.saturating_sub(line.width());
                line = format!("{line}{}{second}", " ".repeat(gap));
            }
            Line::from(line)
        })
        .collect();
    let rect = popup(area, columns * COLUMN + 2, rows.len());
    let block = Block::bordered().title(app.lang.strings().help_title);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(rows).style(app.theme.base()).block(block),
        rect,
    );
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
            dir: None,
            mode: Mode::Edit,
            lang: Lang::Ru,
            encoding: None,
            line: None,
            config: crate::config::Config::default(),
            config_path: None,
            warnings: Vec::new(),
            state_dir: None,
        };
        let opener = Box::new(crate::links::System);
        let mut app = App::open(options, Box::<Local>::default(), opener).unwrap();
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
        assert!(rows[5].contains("F5: просмотр"));
        assert!(rows[5].contains("F9: настройки"));
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
    fn preview_shows_rendered_markdown() {
        let mut app = app("# Привет\n\n- [x] **дело**\n\n```rust\nlet x = 1;\n```\n");
        press(&mut app, "f5");
        let t = terminal(&mut app, 40, 12);
        let buffer = t.backend().buffer();
        let row = |y: u16| (0..40).map(|x| buffer[(x, y)].symbol()).collect::<String>();
        assert!(row(1).starts_with(" Привет"));
        assert!(row(4).starts_with(" ☑ дело"));
        assert!(buffer[(3, 4)].modifier.contains(Modifier::BOLD));
        assert!(row(7).starts_with(" │ let x = 1;"));
        assert_eq!(buffer[(3, 7)].fg, Color::Magenta);
        assert_eq!(buffer[(11, 7)].fg, Color::Yellow);
    }

    #[test]
    fn editor_highlights_markup() {
        let mut app = app("# Глава\nтекст `код`\n");
        let t = terminal(&mut app, 40, 5);
        let buffer = t.backend().buffer();
        assert_eq!(buffer[(2, 1)].fg, Color::Cyan);
        assert_eq!(buffer[(2, 2)].fg, Color::Reset);
        assert_eq!(buffer[(9, 2)].fg, Color::Green);
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
            press(&mut app, "f5");
            screen(&mut app, w, h);
            press(&mut app, "f5");
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

    #[test]
    fn panel_shows_files_beside_the_text() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("каталог")).unwrap();
        std::fs::write(dir.path().join("файл с очень длинным именем.md"), "").unwrap();
        std::fs::write(dir.path().join("\u{1b}[31m.md"), "").unwrap_or(());
        let mut app = app("# Привет\n");
        app.config_path = None;
        press(&mut app, "f4");
        app.browser.as_mut().unwrap().tree = crate::files::Tree::new(dir.path(), false);
        let rows = screen(&mut app, 100, 12);
        let shown = rows.join("\n");
        assert!(rows[2].starts_with(" .."), "{shown}");
        assert!(rows[3].starts_with(" ▸ каталог"), "{shown}");
        assert!(shown.contains("файл с очень длинным") && shown.contains('…'));
        assert!(!shown.contains('\u{1b}'));
        assert!(
            rows[1]
                .chars()
                .skip(29)
                .collect::<String>()
                .starts_with("│1 # Привет")
        );
        assert!(rows[11].contains("ФАЙЛЫ"));
        // Узкий экран: панель вместо текста, без неё текст целиком.
        let rows = screen(&mut app, 40, 12);
        assert!(!rows.join("").contains("Привет"));
        press(&mut app, "esc");
        let rows = screen(&mut app, 40, 12);
        assert!(rows[1].starts_with("1 # Привет"));
        for (width, height) in [(1, 1), (2, 2), (3, 5), (17, 3), (60, 2)] {
            screen(&mut app, width, height);
            press(&mut app, "f4");
            screen(&mut app, width, height);
            press(&mut app, "esc");
        }
    }

    #[test]
    fn settings_show_values_and_the_path() {
        let mut app = app("текст\n");
        app.config_path = Some("/home/кто-то/.config/ruconmd/config.toml".into());
        press(&mut app, "f9");
        let shown = screen(&mut app, 100, 24).join("\n");
        for text in [
            "Настройки",
            "Язык",
            "авто",
            "Тема",
            "dark",
            "Панель: ширина",
            "Номера строк (F6)",
            "Открыть файл настроек",
            "config.toml",
            "Enter: сохранить",
        ] {
            assert!(shown.contains(text), "{text}: {shown}");
        }
        press(&mut app, "down");
        press(&mut app, "right");
        assert!(screen(&mut app, 100, 24).join("").contains("light"));
        // На низком экране выбранное видно.
        press(&mut app, "end");
        let shown = screen(&mut app, 100, 8).join("\n");
        assert!(shown.contains("Открыть файл настроек"), "{shown}");
        for (width, height) in [(1, 1), (5, 3), (30, 4)] {
            screen(&mut app, width, height);
        }
    }

    #[test]
    fn help_fits_a_usual_terminal() {
        let mut app = app("");
        press(&mut app, "f1");
        let shown = screen(&mut app, 80, 24).join("\n");
        for command in Command::ALL {
            assert!(
                shown.contains(app.lang.command(*command)),
                "{command:?}: {shown}"
            );
        }
    }
}
