//! Состояние терминала и чтение событий.

use std::io::{self, Stdout, Write, stderr, stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Mutex, Once, PoisonError};
use std::thread::ThreadId;
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
/// Поток, который рисует. Паника в остальных программу не завершает.
static OWNER: Mutex<Option<ThreadId>> = Mutex::new(None);
/// Сообщения о панике в остальных потоках, выводятся по выходе.
static REPORTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
const MAX_REPORTS: usize = 10;

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
                if ACTIVE.load(Ordering::SeqCst) && !is_owner() {
                    // Вывод сейчас испортил бы экран.
                    report(info.to_string());
                    return;
                }
                restore();
                default(info);
            }));
        });
        *OWNER.lock().unwrap_or_else(PoisonError::into_inner) = Some(std::thread::current().id());
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

    /// Стирает экран, следующая отрисовка выведет его целиком.
    pub fn clear(&mut self) -> io::Result<()> {
        // `clear()` спросил бы позицию курсора, а ответ перехватил бы поток ввода.
        let area = self.inner.size()?.into();
        self.inner.resize(area)
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
        let mut reports = REPORTS.lock().unwrap_or_else(PoisonError::into_inner);
        for report in reports.drain(..) {
            let _ = writeln!(stderr(), "ruconmd: {report}");
        }
    }
}

fn is_owner() -> bool {
    let owner = *OWNER.lock().unwrap_or_else(PoisonError::into_inner);
    owner == Some(std::thread::current().id())
}

fn report(text: String) {
    let mut reports = REPORTS.lock().unwrap_or_else(PoisonError::into_inner);
    if reports.len() < MAX_REPORTS {
        reports.push(text);
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

struct EventReader {
    decoder: InputDecoder,
}

/// Ограничение пачки, чтобы большая вставка не останавливала отрисовку.
const MAX_BATCH: usize = 8192;
/// Сколько ждать следующую клавишу, когда поток похож на вставку.
const PASTE_GAP: Duration = Duration::from_millis(8);
/// Столько символов подряд уже похоже на вставку.
const PASTE_SUSPECT: usize = 3;

impl EventReader {
    fn new() -> Self {
        Self {
            decoder: InputDecoder::new(Platform::CURRENT),
        }
    }

    /// Ждёт событие и забирает всё, что уже пришло следом.
    fn read(&mut self) -> io::Result<Batch> {
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
        Ok(Batch { raw, inputs })
    }
}

/// Ввод, который читает отдельный поток.
///
/// Если терминал исчез, `crossterm` может не вернуться из чтения никогда. Основной
/// цикл при этом продолжает работать и успевает записать несохранённые правки.
pub struct Events {
    receiver: Receiver<io::Result<Batch>>,
}

impl Events {
    /// Запускается после [`Terminal::enter`]: тот сам читает ответы терминала.
    pub fn start() -> Self {
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            let mut reader = EventReader::new();
            loop {
                let batch = reader.read();
                let failed = batch.is_err();
                if sender.send(batch).is_err() || failed {
                    return;
                }
            }
        });
        Self { receiver }
    }

    /// Ждёт ввод не дольше `timeout`.
    pub fn next(&self, timeout: Duration) -> io::Result<Option<Batch>> {
        match self.receiver.recv_timeout(timeout) {
            Ok(batch) => batch.map(Some),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(io::Error::other("input thread stopped")),
        }
    }
}
