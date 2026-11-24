//! Командная строка и основной цикл.

use std::io::{self, IsTerminal};
use std::path::PathBuf;

use clap::Parser;

use crate::app::App;
use crate::command::Mode;
use crate::i18n::Lang;
use crate::term::{EventReader, Terminal};
use crate::{diag, ui};

#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Args {
    /// File to open
    pub file: Option<PathBuf>,
    /// Start in preview mode
    #[arg(long)]
    pub view: bool,
    /// Interface language (default: system locale)
    #[arg(long, value_enum)]
    pub lang: Option<Lang>,
    /// Show what the terminal sends for each key and print the log on exit
    #[arg(long, conflicts_with_all = ["file", "view"])]
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

    let mode = if args.view { Mode::View } else { Mode::Edit };
    let mut app = App::open(args.file, mode, lang)?;
    let mut terminal = Terminal::enter(true)?;
    let mut reader = EventReader::new();
    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        for input in reader.read()?.inputs {
            app.handle(input);
        }
    }
    Ok(())
}
