//! Командная строка и основной цикл.

use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

use crate::app::{App, Options};
use crate::clipboard;
use crate::command::Mode;
use crate::encoding::Enc;
use crate::i18n::Lang;
use crate::term::{EventReader, Terminal};
use crate::{code, diag, links, ui};

/// Как часто обновлять экран, пока идёт фоновая работа.
const BUSY_REDRAW: Duration = Duration::from_millis(20);

#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Args {
    /// File to open
    pub file: Option<PathBuf>,
    /// Start in preview mode
    #[arg(long)]
    pub view: bool,
    /// Go to this line
    #[arg(long, value_name = "N")]
    pub line: Option<usize>,
    /// File encoding (default: detect)
    #[arg(long, value_name = "NAME")]
    pub encoding: Option<Enc>,
    /// Interface language (default: system locale)
    #[arg(long, value_enum)]
    pub lang: Option<Lang>,
    /// Show what the terminal sends for each key and print the log on exit
    #[arg(long, conflicts_with_all = ["file", "view", "line", "encoding"])]
    pub keys: bool,
}

pub fn run() -> io::Result<()> {
    let args = Args::parse();
    let lang = args.lang.unwrap_or_else(Lang::from_system);
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(lang.strings().not_a_terminal));
    }
    if args.keys {
        return diag::run(lang);
    }

    let options = Options {
        path: args.file,
        mode: if args.view { Mode::View } else { Mode::Edit },
        lang,
        encoding: args.encoding,
        line: args.line,
    };
    let mut app = App::open(
        options,
        Box::<clipboard::System>::default(),
        Box::new(links::System),
    )?;
    std::thread::spawn(code::warm_up);
    let mut terminal = Terminal::enter(true)?;
    let mut reader = EventReader::new();
    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        let timeout = app.is_busy().then_some(BUSY_REDRAW);
        if let Some(batch) = reader.read(timeout)? {
            for input in batch.inputs {
                app.handle(input);
            }
        }
    }
    Ok(())
}
