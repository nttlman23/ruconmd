//! Командная строка и основной цикл.

use std::io::{self, IsTerminal, Write};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;

use crate::app::{App, Options};
use crate::clipboard;
use crate::command::Mode;
use crate::encoding::Enc;
use crate::graphics::Graphics;
use crate::i18n::Lang;
use crate::style::Depth;
use crate::term::{Events, Terminal};
use crate::{code, config, diag, graphics, links, ui};

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
    /// Configuration file
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Show what the terminal sends for each key and print the log on exit
    #[arg(long, conflicts_with_all = ["file", "view", "line", "encoding"])]
    pub keys: bool,
}

pub fn run() -> io::Result<()> {
    let args = Args::parse();
    let (config, warnings) = config::load(args.config.as_deref());
    let config = config.fitted(Depth::from_system());
    let lang = args.lang.or(config.lang).unwrap_or_else(Lang::from_system);
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(lang.strings().not_a_terminal));
    }
    if args.keys {
        return diag::run(lang, &config.keymap);
    }

    let options = Options {
        path: args.file,
        mode: if args.view { Mode::View } else { Mode::Edit },
        lang,
        encoding: args.encoding,
        line: args.line,
        config,
        warnings: warnings.clone(),
        state_dir: config::state_dir(),
    };
    let mut app = App::open(
        options,
        Box::<clipboard::System>::default(),
        Box::new(links::System),
    )?;
    std::thread::spawn(code::warm_up);
    let stop = stop_signal()?;
    let result = guard(&mut app, |app| event_loop(app, &stop));

    let mut stderr = io::stderr();
    for warning in &warnings {
        let _ = writeln!(stderr, "ruconmd: {}", lang.config_problem(warning));
    }
    result
}

fn event_loop(app: &mut App, stop: &AtomicBool) -> io::Result<End> {
    let mut terminal = Terminal::enter(true)?;
    let env = graphics::Env::from_system();
    let answers = if graphics::worth_asking(app.wanted_images, &env) {
        graphics::ask()
    } else {
        graphics::Answers::default()
    };
    app.set_graphics(graphics::choose(app.wanted_images, &env, answers));
    let events = Events::start();
    let mut drawn = Vec::new();
    while !app.should_quit {
        app.tick(Instant::now());
        terminal.draw(|frame| ui::draw(frame, app))?;
        // Прежнее место картинки само не стирается.
        let traces = app.pane.graphics().is_some_and(Graphics::leaves_traces);
        if traces && drawn != app.images_on_screen() && !drawn.is_empty() {
            terminal.clear()?;
            terminal.draw(|frame| ui::draw(frame, app))?;
        }
        drawn = app.images_on_screen();
        // Всё, что уже накопилось, обрабатывается до следующей отрисовки.
        let mut timeout = app.sleep();
        while let Some(batch) = events.next(timeout)? {
            for input in batch.inputs {
                app.handle(input);
            }
            if app.should_quit {
                break;
            }
            timeout = Duration::ZERO;
        }
        if stop.load(Ordering::SeqCst) {
            return Ok(End::Interrupted);
        }
    }
    Ok(End::Quit)
}

#[derive(Debug, PartialEq, Eq)]
enum End {
    /// Пользователь вышел сам, правки сохранены или отброшены.
    Quit,
    Interrupted,
}

/// Выполняет основной цикл. При сбое, закрытии окна и панике несохранённые
/// правки записываются для восстановления.
fn guard(app: &mut App, body: impl FnOnce(&mut App) -> io::Result<End>) -> io::Result<()> {
    let outcome = catch_unwind(AssertUnwindSafe(|| body(app)));
    if matches!(outcome, Ok(Ok(End::Quit))) {
        app.finish();
        return Ok(());
    }
    if let Some(path) = app.emergency_save() {
        let text = app.lang.strings().crashed;
        let _ = writeln!(io::stderr(), "ruconmd: {text}: {}", path.display());
    }
    match outcome {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(panic) => resume_unwind(panic),
    }
}

/// Признак того, что систему просят завершить программу.
fn stop_signal() -> io::Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGINT,
    ] {
        signal_hook::flag::register(signal, Arc::clone(&stop))?;
    }
    Ok(stop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::Local;
    use crate::config::Config;
    use crate::input::Input;
    use crate::key::{Chord, Key};
    use crate::recovery;

    fn app(dir: &std::path::Path) -> App {
        let options = Options {
            path: Some(dir.join("a.md")),
            mode: Mode::Edit,
            lang: Lang::En,
            encoding: None,
            line: None,
            config: Config::default(),
            warnings: Vec::new(),
            state_dir: Some(dir.join("state")),
        };
        let mut app = App::open(options, Box::<Local>::default(), Box::new(links::System)).unwrap();
        app.handle(Input::Key(Chord::plain(Key::Char('x'))));
        app
    }

    fn kept(dir: &std::path::Path, app: App) -> Option<String> {
        drop(app);
        let found = recovery::search(&dir.join("state"), Some(&dir.join("a.md")));
        found.orphan.map(|o| o.text)
    }

    #[test]
    fn panic_keeps_unsaved_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        let result = catch_unwind(AssertUnwindSafe(|| {
            guard(&mut app, |_| panic!("test panic"))
        }));
        assert!(result.is_err(), "the panic goes on");
        assert_eq!(kept(dir.path(), app).as_deref(), Some("x"));
    }

    #[test]
    fn errors_and_signals_keep_unsaved_text() {
        for error in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = app(dir.path());
            let result = guard(&mut app, |_| match error {
                true => Err(io::Error::other("terminal is gone")),
                false => Ok(End::Interrupted),
            });
            assert_eq!(result.is_err(), error);
            assert_eq!(kept(dir.path(), app).as_deref(), Some("x"));
        }
    }

    #[test]
    fn normal_exit_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        app.emergency_save().unwrap();
        guard(&mut app, |_| Ok(End::Quit)).unwrap();
        assert_eq!(kept(dir.path(), app), None);
        assert_eq!(
            std::fs::read_dir(dir.path().join("state")).unwrap().count(),
            0
        );
    }
}
