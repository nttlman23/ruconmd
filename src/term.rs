//! Состояние терминала и чтение событий.

use std::io::{self, Stdout, stdout};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};

use crate::input::{Input, InputDecoder, Platform};

static ENHANCED: AtomicBool = AtomicBool::new(false);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK: Once = Once::new();

/// Терминал в режиме программы. Исходное состояние возвращается при
/// уничтожении значения и при панике.
pub struct Terminal {
    inner: ratatui::Terminal<CrosstermBackend<Stdout>>,
}

impl Terminal {
    pub fn enter(mouse: bool) -> io::Result<Self> {
        PANIC_HOOK.call_once(|| {
            let default = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                restore();
                default(info);
            }));
        });
        ACTIVE.store(true, Ordering::SeqCst);
        Self::setup(mouse).inspect_err(|_| restore())
    }

    fn setup(mouse: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
        if mouse {
            execute!(stdout(), EnableMouseCapture)?;
        }
        if Platform::CURRENT == Platform::Unix {
            execute!(stdout(), EnableBracketedPaste)?;
            // С этим протоколом терминал передаёт Ctrl + кириллица.
            if supports_keyboard_enhancement().unwrap_or(false) {
                execute!(
                    stdout(),
                    PushKeyboardEnhancementFlags(
                        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    )
                )?;
                ENHANCED.store(true, Ordering::SeqCst);
            }
        }
        let inner = ratatui::Terminal::new(CrosstermBackend::new(stdout()))?;
        Ok(Self { inner })
    }

    pub fn draw(&mut self, render: impl FnOnce(&mut Frame)) -> io::Result<()> {
        self.inner.draw(render).map(|_| ())
    }

    /// Поддерживает ли терминал расширенный протокол клавиатуры.
    pub fn enhanced_keyboard() -> bool {
        ENHANCED.load(Ordering::SeqCst)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        restore();
    }
}

/// Ошибки не важны: терминал мог уже исчезнуть.
fn restore() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    if ENHANCED.load(Ordering::SeqCst) {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    if Platform::CURRENT == Platform::Unix {
        let _ = execute!(stdout(), DisableBracketedPaste);
    }
    let _ = execute!(stdout(), DisableMouseCapture);
    let _ = execute!(
        stdout(),
        LeaveAlternateScreen,
        ratatui::crossterm::cursor::Show
    );
    let _ = disable_raw_mode();
}

/// События, прочитанные подряд, и их толкование.
pub struct Batch {
    pub raw: Vec<Event>,
    pub inputs: Vec<Input>,
}

pub struct EventReader {
    decoder: InputDecoder,
}

/// Ограничение пачки, чтобы большая вставка не останавливала отрисовку.
const MAX_BATCH: usize = 8192;
/// Сколько ждать следующую клавишу, когда поток похож на вставку.
const PASTE_GAP: Duration = Duration::from_millis(8);
/// Столько символов подряд уже похоже на вставку.
const PASTE_SUSPECT: usize = 3;

impl EventReader {
    #[expect(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            decoder: InputDecoder::new(Platform::CURRENT),
        }
    }

    /// Ждёт событие и забирает всё, что уже пришло следом.
    /// `None`: за `timeout` ничего не пришло.
    pub fn read(&mut self, timeout: Option<Duration>) -> io::Result<Option<Batch>> {
        if let Some(timeout) = timeout
            && !event::poll(timeout)?
        {
            return Ok(None);
        }
        let mut raw = vec![event::read()?];
        let mut typed = 0;
        while raw.len() < MAX_BATCH {
            if let Some(Event::Key(key)) = raw.last()
                && key.is_press()
                && matches!(key.code, KeyCode::Char(_) | KeyCode::Enter | KeyCode::Tab)
            {
                typed += 1;
            }
            let wait = if Platform::CURRENT == Platform::Windows && typed >= PASTE_SUSPECT {
                PASTE_GAP
            } else {
                Duration::ZERO
            };
            if !event::poll(wait)? {
                break;
            }
            raw.push(event::read()?);
        }
        let inputs = self.decoder.decode(&raw, Instant::now());
        Ok(Some(Batch { raw, inputs }))
    }
}
