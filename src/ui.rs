//! Отрисовка.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::app::App;
use crate::command::Command;

const BAR: Style = Style::new().add_modifier(Modifier::REVERSED);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    app.set_page(body.height as usize);

    let name = match &app.path {
        Some(p) => p.display().to_string(),
        None => app.lang.strings().untitled.into(),
    };
    frame.render_widget(
        Paragraph::new(format!(" {}", printable(&name))).style(BAR),
        header,
    );

    let lines: Vec<Line> = app
        .lines
        .iter()
        .skip(app.scroll)
        .take(body.height as usize)
        .map(|l| Line::from(printable(l)))
        .collect();
    frame.render_widget(Paragraph::new(lines), body);

    frame.render_widget(Paragraph::new(footer_text(app)).style(BAR), footer);
    if app.help {
        draw_help(frame, app, body);
    }
}

fn footer_text(app: &App) -> String {
    let mode = app.lang.mode(app.mode);
    if let Some(message) = &app.message {
        return format!(" {mode} | {message}");
    }
    let s = app.lang.strings();
    let line = (app.scroll + 1).min(app.lines.len().max(1));
    let keys = |c| app.keymap.chords(c).join("/");
    format!(
        " {mode} | {line}/{} | {}: {} | {}: {} | {}: {}",
        app.lines.len().max(1),
        keys(Command::ToggleMode),
        app.lang.mode(app.mode.toggled()).to_lowercase(),
        keys(Command::Help),
        s.help,
        keys(Command::Quit),
        s.quit,
    )
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let rows: Vec<Line> = Command::ALL
        .iter()
        .map(|c| {
            let keys = app.keymap.chords(*c).join(", ");
            Line::from(format!(" {keys:<14} {}", app.lang.command(*c)))
        })
        .collect();
    let width = 40.min(area.width);
    let height = (rows.len() as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let block = Block::bordered().title(app.lang.strings().help_title);
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(rows).block(block), popup);
}

/// Управляющие символы не должны попадать в терминал, табуляция раскрывается.
pub fn printable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\t' => out.push_str("    "),
            c if c.is_control() => out.push('\u{fffd}'),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Mode;
    use crate::i18n::Lang;
    use crate::input::Input;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn shows_mode_text_and_hints() {
        let mut app = App::with_text(None, "# Привет\nмир\n", Mode::Edit, Lang::Ru);
        let rows = screen(&mut app, 80, 6);
        assert!(rows[0].starts_with(" без имени"));
        assert!(rows[1].starts_with("# Привет"));
        assert!(rows[5].contains("ПРАВКА") && rows[5].contains("F5/Ctrl+E"));
        app.handle(Input::Key("f5".parse().unwrap()));
        assert!(screen(&mut app, 80, 6)[5].contains("ПРОСМОТР"));
    }

    #[test]
    fn survives_tiny_windows() {
        let mut app = App::with_text(None, "текст\n", Mode::Edit, Lang::Ru);
        app.help = true;
        for (w, h) in [(1, 1), (2, 2), (5, 3), (10, 4)] {
            screen(&mut app, w, h);
        }
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        assert_eq!(printable("a\x1b[31m\tb"), "a\u{fffd}[31m    b");
    }
}
