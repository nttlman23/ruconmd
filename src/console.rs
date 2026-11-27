//! Встроенная консоль: оболочка в псевдотерминале, разбор её вывода, перевод
//! клавиш в байты для неё. Отрисовки здесь нет.

use std::io::{self, Read, Write};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::key::{Chord, Key, Mods};

/// Столько строк ушедшего вверх вывода можно прокрутить назад.
const SCROLLBACK: usize = 2000;

/// Оболочка по умолчанию.
pub fn default_shell() -> String {
    if cfg!(windows) {
        let pwsh = std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("pwsh.exe").is_file())
        });
        if pwsh { "pwsh.exe" } else { "powershell.exe" }.to_owned()
    } else {
        std::env::var("SHELL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "/bin/sh".to_owned())
    }
}

/// Оболочки, из которых можно выбрать в настройках. Пустая строка: по умолчанию.
pub fn known_shells() -> Vec<String> {
    let mut shells = vec![String::new()];
    if cfg!(windows) {
        shells.extend(["pwsh.exe", "powershell.exe", "cmd.exe"].map(String::from));
    } else {
        for name in ["bash", "zsh", "fish", "sh"] {
            let found = ["/bin", "/usr/bin", "/usr/local/bin"]
                .iter()
                .map(|dir| Path::new(dir).join(name))
                .find(|path| path.is_file());
            if let Some(path) = found {
                shells.push(path.to_string_lossy().into_owned());
            }
        }
    }
    shells
}

/// Команда оболочки: существующий путь целиком, иначе слова через пробел.
fn command(shell: &str) -> Option<CommandBuilder> {
    let shell = shell.trim();
    if shell.is_empty() {
        return None;
    }
    if Path::new(shell).is_file() {
        return Some(CommandBuilder::new(shell));
    }
    let mut words = shell.split_whitespace();
    let mut command = CommandBuilder::new(words.next()?);
    command.args(words);
    Some(command)
}

/// Ошибки псевдотерминала приходят своим типом.
fn failed(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn size(rows: u16, cols: u16) -> (NonZeroU16, NonZeroU16) {
    let one = NonZeroU16::MIN;
    (
        NonZeroU16::new(rows).unwrap_or(one),
        NonZeroU16::new(cols).unwrap_or(one),
    )
}

pub struct Console {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    parser: Arc<Mutex<vt100::Parser>>,
    /// Читающий поток ещё получает вывод.
    reading: Arc<AtomicBool>,
    rows: u16,
    cols: u16,
    /// Что запущено, для заголовка.
    pub title: String,
}

impl Console {
    /// Запускает оболочку. `wake` вызывается после каждого прочитанного вывода.
    pub fn start(
        shell: &str,
        dir: &Path,
        rows: u16,
        cols: u16,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        let (rows, cols) = (rows.max(1), cols.max(1));
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(failed)?;
        let wanted = if shell.trim().is_empty() {
            default_shell()
        } else {
            shell.to_owned()
        };
        let mut command = command(&wanted).ok_or_else(|| io::Error::other("no shell"))?;
        command.cwd(dir);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(command).map_err(failed)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(failed)?;
        let writer = pair.master.take_writer().map_err(failed)?;
        let (r, c) = size(rows, cols);
        let parser = Arc::new(Mutex::new(vt100::Parser::new(r, c, SCROLLBACK)));
        let reading = Arc::new(AtomicBool::new(true));
        let (shared, alive) = (Arc::clone(&parser), Arc::clone(&reading));
        std::thread::spawn(move || {
            let mut chunk = [0; 16 * 1024];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut parser = shared.lock().unwrap_or_else(PoisonError::into_inner);
                        parser.process(&chunk[..n]);
                    }
                }
                wake();
            }
            alive.store(false, Ordering::SeqCst);
            wake();
        });
        Ok(Self {
            master: pair.master,
            writer,
            child,
            parser,
            reading,
            rows,
            cols,
            title: wanted,
        })
    }

    pub fn screen(&self) -> MutexGuard<'_, vt100::Parser> {
        self.parser.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Оболочка завершилась.
    pub fn finished(&mut self) -> bool {
        let exited = self.child.try_wait().ok().flatten().is_some();
        exited || !self.reading.load(Ordering::SeqCst)
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(1), cols.max(1));
        if (rows, cols) == (self.rows, self.cols) {
            return;
        }
        (self.rows, self.cols) = (rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        let (r, c) = size(rows, cols);
        self.screen().set_size(r, c);
    }

    pub fn write(&mut self, bytes: &[u8]) {
        // Ввод всегда возвращает к последнему выводу.
        self.screen().screen_mut().set_scrollback(0);
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn key(&mut self, chord: Chord) {
        let application = self.screen().screen().application_cursor();
        if let Some(bytes) = encode(chord, application) {
            self.write(&bytes);
        }
    }

    pub fn paste(&mut self, text: &str) {
        let bracketed = self.screen().screen().bracketed_paste();
        // Переводы строк вставки как нажатия Enter.
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bytes = match bracketed {
            true => format!("\x1b[200~{text}\x1b[201~"),
            false => text,
        };
        self.write(bytes.as_bytes());
    }

    /// Прокручивает назад к ушедшему вверх выводу.
    pub fn scroll(&mut self, delta: isize) {
        let mut parser = self.screen();
        let screen = parser.screen_mut();
        let back = screen.scrollback().saturating_add_signed(-delta);
        screen.set_scrollback(back);
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Каталог для консоли: каталог файла, а без него показанный или текущий.
pub fn start_dir(file: Option<&Path>, shown: Option<&Path>) -> PathBuf {
    let of_file = file
        .and_then(|f| std::path::absolute(f).ok())
        .and_then(|f| f.parent().map(Path::to_owned));
    of_file
        .filter(|d| d.is_dir())
        .or_else(|| shown.map(Path::to_owned).filter(|d| d.is_dir()))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Номер модификаторов в последовательностях xterm: 1 + Shift + 2·Alt + 4·Ctrl.
fn modifier(mods: Mods) -> u8 {
    1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl)
}

/// Байты, которые терминал xterm посылает на это нажатие.
pub fn encode(chord: Chord, application: bool) -> Option<Vec<u8>> {
    let mods = chord.mods;
    let plain = mods.is_none();
    let m = modifier(mods);
    let cursor = |letter: char| match (plain, application) {
        (true, true) => format!("\x1bO{letter}"),
        (true, false) => format!("\x1b[{letter}"),
        (false, _) => format!("\x1b[1;{m}{letter}"),
    };
    let tilde = |number: u8| match plain {
        true => format!("\x1b[{number}~"),
        false => format!("\x1b[{number};{m}~"),
    };
    let alt = |bytes: &[u8]| {
        let mut out = Vec::new();
        if mods.alt {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        out
    };
    let text = match chord.key {
        Key::Char(c) if mods.ctrl => {
            let control = match c {
                'a'..='z' => c as u8 - b'a' + 1,
                '@' | ' ' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '/' | '7' => 0x1f,
                '8' | '?' => 0x7f,
                _ => return None,
            };
            return Some(alt(&[control]));
        }
        Key::Char(c) => return Some(alt(c.encode_utf8(&mut [0; 4]).as_bytes())),
        Key::Enter => return Some(alt(b"\r")),
        Key::Tab if mods.shift => "\x1b[Z".to_owned(),
        Key::Tab => return Some(alt(b"\t")),
        Key::Backspace if mods.ctrl => return Some(alt(b"\x08")),
        Key::Backspace => return Some(alt(b"\x7f")),
        Key::Esc => return Some(alt(b"\x1b")),
        Key::Up => cursor('A'),
        Key::Down => cursor('B'),
        Key::Right => cursor('C'),
        Key::Left => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F(n @ 1..=4) => {
            let letter = (b'P' + n - 1) as char;
            match plain {
                true => format!("\x1bO{letter}"),
                false => format!("\x1b[1;{m}{letter}"),
            }
        }
        Key::F(n) => {
            let number = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => return None,
            };
            tilde(number)
        }
    };
    Some(text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    fn encoded(chord: &str, application: bool) -> String {
        let chord: Chord = chord.parse().unwrap();
        String::from_utf8(encode(chord, application).unwrap()).unwrap()
    }

    #[test]
    fn keys_are_sent_as_xterm_sends_them() {
        for (chord, bytes) in [
            ("enter", "\r"),
            ("tab", "\t"),
            ("shift+tab", "\x1b[Z"),
            ("backspace", "\x7f"),
            ("esc", "\x1b"),
            ("up", "\x1b[A"),
            ("ctrl+left", "\x1b[1;5D"),
            ("shift+end", "\x1b[1;2F"),
            ("delete", "\x1b[3~"),
            ("ctrl+pagedown", "\x1b[6;5~"),
            ("f1", "\x1bOP"),
            ("shift+f4", "\x1b[1;2S"),
            ("f5", "\x1b[15~"),
            ("f12", "\x1b[24~"),
            ("ctrl+c", "\x03"),
            ("ctrl+d", "\x04"),
            ("alt+b", "\x1bb"),
            ("ctrl+alt+x", "\x1b\x18"),
        ] {
            assert_eq!(encoded(chord, false), bytes, "{chord}");
        }
        let ctrl = Mods {
            ctrl: true,
            ..Mods::NONE
        };
        let escape = encode(Chord::new(Key::Char('['), ctrl), false);
        assert_eq!(escape.unwrap(), b"\x1b");
        assert_eq!(encoded("up", true), "\x1bOA");
        assert_eq!(encoded("ctrl+up", true), "\x1b[1;5A");
        let typed = encode(Chord::plain(Key::Char('ж')), false).unwrap();
        assert_eq!(typed, "ж".as_bytes());
        // Ctrl с кириллицей приходит латинской буквой по месту клавиши.
        let russian = Chord::shortcut(
            'с',
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );
        assert_eq!(encode(russian, false).unwrap(), b"\x03");
        assert_eq!(encode(Chord::plain(Key::F(13)), false), None);
    }

    #[test]
    fn shell_commands_are_read() {
        assert!(command("").is_none() && command("   ").is_none());
        let words = command("bash -l").unwrap();
        let argv: Vec<_> = words
            .get_argv()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv, ["bash", "-l"]);
        let dir = tempfile::tempdir().unwrap();
        let spaced = dir.path().join("my shell");
        std::fs::write(&spaced, "").unwrap();
        let whole = command(spaced.to_str().unwrap()).unwrap();
        assert_eq!(whole.get_argv().len(), 1);
        assert!(!default_shell().is_empty());
        assert_eq!(known_shells()[0], "");
    }

    #[test]
    fn start_dir_is_the_files_dir() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        assert_eq!(start_dir(Some(&file), None), dir.path());
        let shown = dir.path().join("shown");
        std::fs::create_dir(&shown).unwrap();
        let gone = dir.path().join("none").join("a.md");
        assert_eq!(start_dir(Some(&gone), Some(&shown)), shown);
        assert_eq!(start_dir(None, None), std::env::current_dir().unwrap());
    }

    #[cfg(unix)]
    fn wait(console: &Console, what: &str) {
        let start = Instant::now();
        while !console.screen().screen().contents().contains(what) {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "{what}: {}",
                console.screen().screen().contents()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    #[test]
    fn shell_runs_in_the_given_dir_and_exits() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("здесь.md"), "").unwrap();
        let woken = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&woken);
        let mut console = Console::start("/bin/sh", dir.path(), 10, 40, move || {
            flag.store(true, Ordering::SeqCst);
        })
        .unwrap();
        console.write(b"ls; echo $TERM; echo \xd0\xb3\xd0\xbe\xd1\x82\xd0\xbe\xd0\xb2\xd0\xbe\r");
        wait(&console, "готово");
        let shown = console.screen().screen().contents();
        assert!(
            shown.contains("здесь.md") && shown.contains("xterm-256color"),
            "{shown}"
        );
        assert!(woken.load(Ordering::SeqCst));
        console.resize(5, 20);
        console.write(b"stty size\r");
        wait(&console, "5 20");
        console.paste("echo один\necho два\n");
        wait(&console, "два");
        // Прокрутка назад и возврат при вводе.
        for i in 0..30 {
            console.write(format!("echo строка{i}\r").as_bytes());
        }
        wait(&console, "строка29");
        console.scroll(-10);
        assert!(console.screen().screen().scrollback() > 0);
        console.scroll(100);
        assert_eq!(console.screen().screen().scrollback(), 0);
        console.scroll(-10);
        console.key("enter".parse().unwrap());
        assert_eq!(console.screen().screen().scrollback(), 0);
        assert!(!console.finished());
        console.write(b"exit\r");
        let start = Instant::now();
        while !console.finished() {
            assert!(start.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    #[test]
    fn missing_shell_is_an_error_or_ends_at_once() {
        let dir = tempfile::tempdir().unwrap();
        match Console::start("/nonexistent/shell", dir.path(), 5, 20, || {}) {
            Err(_) => {}
            Ok(mut console) => {
                let start = Instant::now();
                while !console.finished() {
                    assert!(start.elapsed() < Duration::from_secs(20));
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
}
