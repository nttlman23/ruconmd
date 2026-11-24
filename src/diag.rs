//! `--keys`: показывает, что передаёт терминал и как это понято.

use std::io;
use std::time::Duration;

use ratatui::crossterm::event::{Event, KeyCode};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};

use crate::command::Mode;
use crate::i18n::Lang;
use crate::input::Input;
use crate::key::{Chord, Key};
use crate::keymap::Keymap;
use crate::term::{Events, Terminal};
use crate::ui::printable;

pub fn run(lang: Lang, keymap: &Keymap) -> io::Result<()> {
    let mut log = Vec::new();
    {
        // Мышь не захватывается: правая кнопка во многих терминалах вставляет текст.
        let mut terminal = Terminal::enter(false)?;
        let events = Events::start();
        log.push(format!(
            "ruconmd {} {} enhanced_keyboard={}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            Terminal::enhanced_keyboard()
        ));
        let mut escapes = 0;
        while escapes < 2 {
            terminal.draw(|frame| {
                let s = lang.strings();
                let [hint, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                    .areas(frame.area());
                frame.render_widget(Paragraph::new(s.diag_hint), hint);
                let rows = body.height.saturating_sub(2) as usize;
                let tail = log.len().saturating_sub(rows);
                let lines: Vec<Line> = log[tail..].iter().map(|l| Line::from(l.as_str())).collect();
                let block = Block::bordered().title(s.diag_title);
                frame.render_widget(Paragraph::new(lines).block(block), body);
            })?;
            let Some(batch) = events.next(Duration::from_secs(1))? else {
                continue;
            };
            log.extend(batch.raw.iter().filter_map(describe_raw));
            for input in &batch.inputs {
                if *input == Input::Key(Chord::plain(Key::Esc)) {
                    escapes += 1;
                } else {
                    escapes = 0;
                }
                log.extend(describe(input, keymap, lang));
            }
        }
    }
    for line in log {
        println!("{line}");
    }
    Ok(())
}

/// Содержимое вставки не записывается, журнал могут переслать.
fn describe_raw(event: &Event) -> Option<String> {
    match event {
        Event::Key(k) => {
            let code = match k.code {
                KeyCode::Char(c) => format!("{c:?} U+{:04X}", c as u32),
                other => format!("{other:?}"),
            };
            Some(format!("  {:?} {code} {:?}", k.kind, k.modifiers))
        }
        Event::Paste(text) => Some(format!("  Paste, {} bytes", text.len())),
        _ => None,
    }
}

fn describe(input: &Input, keymap: &Keymap, lang: Lang) -> Option<String> {
    let s = lang.strings();
    match input {
        Input::Key(chord) => {
            let command = keymap.lookup(Mode::Edit, *chord).or_else(|| {
                keymap
                    .lookup(Mode::View, *chord)
                    .filter(|_| chord.text().is_none())
            });
            let meaning = match command {
                Some(c) => lang.command(c),
                None if chord.text().is_some() => s.diag_text,
                None => s.diag_unbound,
            };
            Some(printable(&format!("{chord} -> {meaning}")))
        }
        Input::Paste { text, continued } => {
            let note = if *continued {
                format!(", {}", s.diag_continued)
            } else {
                String::new()
            };
            let lines = text.lines().count();
            Some(format!(
                "{} {}, \\n: {lines}{note}",
                s.diag_paste,
                text.chars().count()
            ))
        }
        _ => None,
    }
}
