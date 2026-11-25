//! Проверки собранной программы в псевдотерминале.
//!
//! Общие тесты идут и в Linux, и в Windows (ConPTY). Экран восстанавливает эмулятор
//! терминала, поэтому проверяется то, что увидел бы пользователь.

#![cfg_attr(not(unix), allow(dead_code))]

use std::fs;
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

const ROWS: u16 = 24;
const COLS: u16 = 100;
const PATIENCE: Duration = Duration::from_secs(20);

const ENTER: &str = "\r";
const ESC: &str = "\x1b";
const TAB: &str = "\t";
const F1: &str = "\x1bOP";
const F4: &str = "\x1bOS";
const F5: &str = "\x1b[15~";
const F6: &str = "\x1b[17~";
const F9: &str = "\x1b[20~";
const DOWN: &str = "\x1b[B";
const RIGHT: &str = "\x1b[C";
const CTRL_A: &str = "\x01";
const CTRL_C: &str = "\x03";
const CTRL_E: &str = "\x05";
const CTRL_F: &str = "\x06";
const CTRL_Q: &str = "\x11";
const CTRL_R: &str = "\x12";
const CTRL_S: &str = "\x13";
const CTRL_V: &str = "\x16";
const CTRL_Z: &str = "\x1a";

fn size(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n).expect("size is not zero")
}

/// Что умеет изображаемый терминал.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Terminal {
    Plain,
    /// Расширенный протокол клавиатуры и графика Kitty.
    Kitty,
    Sixel,
    /// Не отвечает ни на что.
    Silent,
}

struct Seen {
    parser: vt100::Parser,
    raw: Vec<u8>,
}

/// Отдельные каталоги настроек и состояния, чтобы не трогать настоящие.
struct Home {
    dir: tempfile::TempDir,
    terminal: Terminal,
    env: Vec<(&'static str, &'static str)>,
}

impl Home {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            terminal: Terminal::Plain,
            env: Vec::new(),
        }
    }

    fn on(terminal: Terminal) -> Self {
        Self {
            terminal,
            ..Self::new()
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn write(&self, name: &str, text: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, text).unwrap();
        path
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap()
    }

    fn config(&self, text: &str) {
        let dir = self.path("config/ruconmd");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), text).unwrap();
    }

    /// Файлы восстановления.
    fn copies(&self) -> usize {
        let dir = self.path("state/ruconmd/recovery");
        fs::read_dir(dir).map_or(0, Iterator::count)
    }

    /// Есть ли готовая копия. Число файлов этого не скажет: пока копия пишется,
    /// рядом с блокировкой лежит временный файл.
    fn has_copy(&self) -> bool {
        let Ok(entries) = fs::read_dir(self.path("state/ruconmd/recovery")) else {
            return false;
        };
        entries
            .flatten()
            .any(|e| e.path().extension().is_some_and(|e| e == "rec"))
    }

    fn start(&self, args: &[&str]) -> Session {
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_ruconmd"));
        command.args(["--lang", "ru"]);
        command.args(args);
        self.spawn(command)
    }

    fn spawn(&self, mut command: CommandBuilder) -> Session {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        command.cwd(self.dir.path());
        for name in [
            "TMUX",
            "STY",
            "TERM_PROGRAM",
            "KONSOLE_VERSION",
            "WEZTERM_EXECUTABLE",
        ] {
            command.env_remove(name);
        }
        // Настоящий буфер обмена тесты не трогают.
        for name in [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "WAYLAND_SOCKET",
            "XDG_RUNTIME_DIR",
        ] {
            command.env_remove(name);
        }
        command.env("TERM", "xterm-256color");
        for name in ["XDG_CONFIG_HOME", "APPDATA"] {
            command.env(name, self.path("config"));
        }
        for name in ["XDG_STATE_HOME", "LOCALAPPDATA"] {
            command.env(name, self.path("state"));
        }
        for (name, value) in &self.env {
            command.env(name, value);
        }
        let child = pty.slave.spawn_command(command).unwrap();
        drop(pty.slave);

        let writer = Arc::new(Mutex::new(pty.master.take_writer().unwrap()));
        let seen = Arc::new(Mutex::new(Seen {
            parser: vt100::Parser::new(size(ROWS), size(COLS), 0),
            raw: Vec::new(),
        }));
        let detach = Arc::new(AtomicBool::new(false));
        let mut reader = pty.master.try_clone_reader().unwrap();
        let thread = {
            let (writer, seen) = (Arc::clone(&writer), Arc::clone(&seen));
            let (detach, terminal) = (Arc::clone(&detach), self.terminal);
            thread::spawn(move || {
                let mut answers = Answers::new(terminal);
                let mut chunk = [0; 8192];
                while let Ok(n) = reader.read(&mut chunk) {
                    if n == 0 || detach.load(Ordering::SeqCst) {
                        break;
                    }
                    let mut seen = seen.lock().unwrap();
                    seen.raw.extend_from_slice(&chunk[..n]);
                    seen.parser.process(&chunk[..n]);
                    let reply = answers.next(&seen.raw);
                    drop(seen);
                    if !reply.is_empty() {
                        let mut writer = writer.lock().unwrap();
                        let _ = writer.write_all(reply.as_bytes());
                        let _ = writer.flush();
                    }
                }
            })
        };
        Session {
            child,
            master: Some(pty.master),
            writer: Some(writer),
            seen,
            detach,
            thread: Some(thread),
        }
    }

    /// Открывает файл и ждёт, когда программа появится на экране.
    fn open(&self, args: &[&str]) -> Session {
        let session = self.start(args);
        session.wait_for("F1: справка");
        session
    }
}

/// Ответы терминала на запросы программы.
struct Answers {
    terminal: Terminal,
    attributes: usize,
    cursor: usize,
}

impl Answers {
    fn new(terminal: Terminal) -> Self {
        Self {
            terminal,
            attributes: 0,
            cursor: 0,
        }
    }

    fn next(&mut self, raw: &[u8]) -> String {
        let count = |what: &[u8]| raw.windows(what.len()).filter(|w| *w == what).count();
        let mut reply = String::new();
        // ConPTY при запуске спрашивает позицию курсора и ждёт ответа.
        while self.cursor < count(b"\x1b[6n") {
            self.cursor += 1;
            reply.push_str("\x1b[1;1R");
        }
        if self.terminal == Terminal::Silent {
            return reply;
        }
        while self.attributes < count(b"\x1b[c") {
            self.attributes += 1;
            let graphics = self.attributes > 1;
            reply.push_str(match (self.terminal, graphics) {
                (Terminal::Kitty, false) => "\x1b[?0u\x1b[?62c",
                (Terminal::Kitty, true) => "\x1b_Gi=31;OK\x1b\\\x1b[6;20;10t\x1b[?62;22c",
                (Terminal::Sixel, true) => "\x1b[6;20;10t\x1b[?62;4;22c",
                _ => "\x1b[?62;22c",
            });
        }
        reply
    }
}

struct Session {
    child: Box<dyn Child + Send + Sync>,
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    seen: Arc<Mutex<Seen>>,
    detach: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Session {
    fn send(&self, text: &str) {
        let mut writer = self.writer.as_ref().unwrap().lock().unwrap();
        writer.write_all(text.as_bytes()).unwrap();
        writer.flush().unwrap();
        drop(writer);
        // В Windows быстрый поток символов считается вставкой.
        thread::sleep(Duration::from_millis(60));
    }

    /// Набирает текст как человек, по одной клавише.
    fn type_text(&self, text: &str) {
        for c in text.chars() {
            self.send(c.encode_utf8(&mut [0; 4]));
        }
    }

    fn screen(&self) -> String {
        self.seen.lock().unwrap().parser.screen().contents()
    }

    fn raw(&self) -> Vec<u8> {
        self.seen.lock().unwrap().raw.clone()
    }

    fn raw_len(&self) -> usize {
        self.seen.lock().unwrap().raw.len()
    }

    fn contains(&self, from: usize, what: &str) -> bool {
        let raw = self.raw();
        raw[from..]
            .windows(what.len())
            .any(|w| w == what.as_bytes())
    }

    fn wait(&self, what: &str, ready: impl Fn(&Self) -> bool) {
        let start = Instant::now();
        while !ready(self) {
            assert!(
                start.elapsed() < PATIENCE,
                "не дождались: {what}\n--- экран ---\n{}",
                self.screen()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for(&self, text: &str) {
        self.wait(text, |s| s.screen().contains(text));
    }

    fn wait_gone(&self, text: &str) {
        self.wait(&format!("исчезнет {text}"), |s| {
            !s.screen().contains(text)
        });
    }

    fn wait_file(&self, path: &Path, text: &str) {
        self.wait(&format!("в файле {text:?}"), |_| {
            fs::read_to_string(path).is_ok_and(|t| t == text)
        });
    }

    /// Ждёт завершения, возвращает код.
    fn code(&mut self) -> u32 {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.exit_code();
            }
            if start.elapsed() > PATIENCE {
                let _ = self.child.kill();
                panic!("программа не завершилась\n{}", self.screen());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn quit(&mut self) {
        self.send(CTRL_Q);
        assert_eq!(self.code(), 0);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

// --- Общие проверки ---

#[test]
fn text_is_typed_saved_and_reopened() {
    let home = Home::new();
    let path = home.path("новый.md");
    let mut s = home.open(&[arg(&path)]);
    s.type_text("# Привет");
    s.wait_for("# Привет");
    s.send(CTRL_S);
    s.wait_file(&path, "# Привет");
    s.quit();
    let mut s = home.open(&[arg(&path)]);
    s.wait_for("# Привет");
    s.quit();
    assert_eq!(home.copies(), 0);
}

#[test]
fn both_hotkeys_switch_modes() {
    let home = Home::new();
    let path = home.write("a.md", "# Глава\n\n- [x] **дело**\n");
    let mut s = home.open(&[arg(&path)]);
    s.wait_for("ПРАВКА");
    s.send(F5);
    s.wait_for("ПРОСМОТР");
    s.wait_for("☑ дело");
    assert!(!s.screen().contains("**"));
    s.send(CTRL_E);
    s.wait_for("ПРАВКА");
    s.wait_for("**дело**");
    s.quit();
}

#[test]
fn quitting_with_changes_asks_first() {
    let home = Home::new();
    let path = home.write("a.md", "старое");
    let mut s = home.open(&[arg(&path)]);
    s.type_text("x");
    s.wait_for("xстарое");
    s.send(CTRL_Q);
    s.wait_for("Не сохранять");
    s.send(ESC);
    s.wait_gone("Не сохранять");
    s.send(CTRL_Q);
    s.wait_for("Не сохранять");
    s.send(DOWN);
    s.send(ENTER);
    assert_eq!(s.code(), 0);
    assert_eq!(home.read("a.md"), "старое");
    assert_eq!(home.copies(), 0);
}

#[test]
fn undo_and_internal_clipboard() {
    let home = Home::new();
    let path = home.write("a.md", "кот");
    let mut s = home.open(&[arg(&path)]);
    s.send(CTRL_A);
    s.type_text("x");
    s.wait_for("1 x");
    s.send(CTRL_Z);
    s.wait_for("1 кот");
    if cfg!(unix) {
        s.send(CTRL_A);
        s.send(CTRL_C);
        s.send("\x1b[F");
        s.send(CTRL_V);
        s.wait_for("1 коткот");
        s.send(CTRL_Z);
        s.wait_gone("коткот");
    }
    s.send(CTRL_S);
    s.quit();
    assert_eq!(home.read("a.md"), "кот");
}

#[test]
fn search_and_replace() {
    let home = Home::new();
    let path = home.write("a.md", "кот Кот\nпёс\n");
    let mut s = home.open(&[arg(&path)]);
    s.send(CTRL_F);
    s.type_text("кот");
    s.send(ENTER);
    s.wait_for("1 из 2");
    s.send(CTRL_R);
    s.wait_for("Заменить [");
    s.send(ENTER);
    s.wait_for("Заменить на");
    s.type_text("кит");
    s.send(ENTER);
    s.wait_for("Пробел: пропустить");
    s.type_text("ф");
    s.wait_for("Заменено: 2");
    s.send(CTRL_S);
    s.wait_file(&path, "кит кит\nпёс\n");
    s.quit();
}

#[test]
fn lists_continue_while_typing() {
    let home = Home::new();
    let path = home.write("a.md", "\n");
    let mut s = home.open(&[arg(&path)]);
    s.type_text("- раз");
    s.send(ENTER);
    s.type_text("два");
    s.wait_for("- два");
    s.send(CTRL_S);
    s.wait_file(&path, "- раз\n- два\n");
    s.quit();
}

#[test]
fn encoding_and_line_ends_are_kept() {
    let home = Home::new();
    // «Обычный текст\r\n» в CP1251.
    let original = b"\xce\xe1\xfb\xf7\xed\xfb\xe9 \xf2\xe5\xea\xf1\xf2\r\n";
    let path = home.write("a.md", original);
    let mut s = home.open(&[arg(&path)]);
    s.wait_for("Обычный текст");
    s.wait_for("CP1251 | CRLF");
    s.type_text("Ж");
    s.send(CTRL_S);
    let want = [b"\xc6".as_slice(), original].concat();
    s.wait("сохранение", |_| fs::read(&path).unwrap() == want);
    s.quit();
    assert_eq!(fs::read(home.path("a.md.bak")).unwrap(), original);
}

#[test]
fn checkbox_and_link_in_preview() {
    let home = Home::new();
    let path = home.write("a.md", "[файл](b.md)\n\n- [ ] дело\n");
    home.write("b.md", "# Второй\n");
    let mut s = home.open(&["--view", arg(&path)]);
    s.send(TAB);
    s.wait_for("Enter: b.md");
    s.send(ENTER);
    s.wait_for("Второй");
    s.send("\x1b[1;3D");
    s.wait_for("☐ дело");
    s.send(TAB);
    s.send(TAB);
    s.send(" ");
    s.wait_for("☑ дело");
    s.send(CTRL_S);
    s.wait_file(&path, "[файл](b.md)\n\n- [x] дело\n");
    s.quit();
}

#[test]
fn foreign_change_is_noticed() {
    let home = Home::new();
    let path = home.write("a.md", "моё\n");
    let mut s = home.open(&[arg(&path)]);
    fs::write(&path, "чужое, длиннее\n").unwrap();
    s.wait_for("Файл изменён другой программой");
    s.send(DOWN);
    s.send(ENTER);
    s.wait_for("чужое, длиннее");
    s.quit();
}

#[test]
fn killed_session_is_restored() {
    let home = Home::new();
    let path = home.write("a.md", "старое\n");
    let mut s = home.open(&[arg(&path)]);
    s.type_text("правка ");
    s.wait("копия записана", |_| home.has_copy());
    s.child.kill().unwrap();
    s.child.wait().unwrap();
    drop(s);
    let mut s = home.start(&[arg(&path)]);
    s.wait_for("Найдены несохранённые правки");
    s.send(ENTER);
    s.wait_for("правка старое");
    assert_eq!(home.read("a.md"), "старое\n");
    s.send(CTRL_S);
    s.wait_file(&path, "правка старое\n");
    s.quit();
    assert_eq!(home.copies(), 0);
}

#[test]
fn directory_is_opened_and_a_file_is_chosen() {
    let home = Home::new();
    let dir = home.path("docs");
    fs::create_dir_all(dir.join("глубже")).unwrap();
    fs::write(dir.join("a.md"), "первый\n").unwrap();
    fs::write(dir.join("глубже").join("b.md"), "второй\n").unwrap();
    let mut s = home.start(&[arg(&dir)]);
    s.wait_for("ФАЙЛЫ");
    s.wait_for("глубже");
    s.send(ENTER);
    s.wait_for("b.md");
    s.send(DOWN);
    s.send(ENTER);
    s.wait_for("второй");
    s.type_text("ещё ");
    s.wait_for("ещё второй");
    // Другой файл: сначала вопрос о правках.
    s.send(F4);
    s.wait_for("ФАЙЛЫ");
    s.send(DOWN);
    s.send(ENTER);
    s.wait_for("Есть несохранённые правки");
    s.send(ENTER);
    s.wait_for("первый");
    assert_eq!(
        fs::read_to_string(dir.join("глубже").join("b.md")).unwrap(),
        "ещё второй\n"
    );
    s.send(F4);
    s.send(F4);
    s.wait_gone("глубже");
    s.quit();
}

#[test]
fn settings_are_changed_in_the_program() {
    let home = Home::new();
    home.config("# моё\n[editor]\nline_numbers = true # номера\n");
    let path = home.write("a.md", "строка\n");
    let mut s = home.open(&[arg(&path)]);
    s.wait_for("1 строка");
    s.send(F9);
    s.wait_for("Номера строк");
    for _ in 0..4 {
        s.send(DOWN);
    }
    s.send(RIGHT);
    s.send(ENTER);
    s.wait_for("Настройки сохранены");
    s.wait_gone("1 строка");
    assert_eq!(
        home.read("config/ruconmd/config.toml"),
        "# моё\n[editor]\nline_numbers = false # номера\n"
    );
    s.quit();
    let mut s = home.open(&[arg(&path)]);
    assert!(!s.screen().contains("1 строка"));
    s.quit();
}

#[test]
fn line_numbers_are_toggled_and_remembered() {
    let home = Home::new();
    let path = home.write("a.md", "строка\n");
    let mut s = home.open(&[arg(&path)]);
    s.wait_for("1 строка");
    s.send(F6);
    s.wait_for("Номера строк: нет");
    s.wait_gone("1 строка");
    s.quit();
    let mut s = home.open(&[arg(&path)]);
    s.wait_for("строка");
    assert!(!s.screen().contains("1 строка"));
    s.send(F6);
    s.wait_for("1 строка");
    s.quit();
    let mut s = home.open(&[arg(&path)]);
    s.wait_for("1 строка");
    s.quit();
}

#[test]
fn config_changes_keys_theme_and_saving() {
    let home = Home::new();
    home.config(
        "theme = \"light\"\ntypo = 1\n[keys]\ntoggle_mode = [\"f9\"]\n\
         [save]\nautosave = true\nautosave_delay_sec = 1\n",
    );
    let path = home.write("a.md", "старое\n");
    let mut s = home.start(&[arg(&path)]);
    s.wait_for("Конфиг: typo: неизвестный параметр");
    s.send(F9);
    s.wait_for("ПРОСМОТР");
    s.send(F9);
    s.wait_for("ПРАВКА");
    s.type_text("x");
    s.wait_file(&path, "xстарое\n");
    s.quit();
}

#[test]
fn help_and_unknown_options() {
    let run = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ruconmd"))
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    };
    let (code, text) = run(&["--help"]);
    assert_eq!(code, Some(0));
    for option in [
        "--view",
        "--line",
        "--encoding",
        "--lang",
        "--config",
        "--keys",
    ] {
        assert!(text.contains(option), "{option}");
    }
    assert_eq!(
        run(&["--version"]).1.trim(),
        format!("ruconmd {}", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(run(&["--encoding", "latin1", "x.md"]).0, Some(2));
    assert_eq!(run(&["x.md"]).0, Some(1), "not a terminal");
}

// --- Windows ---

/// Вставка приходит потоком нажатий и не должна запускать помощь при вводе.
#[cfg(windows)]
#[test]
fn windows_paste_is_verbatim() {
    let home = Home::new();
    let path = home.write("a.md", "\n");
    let mut s = home.open(&[arg(&path)]);
    s.send("- раз\r- два\r- три");
    s.wait_for("- три");
    s.send(CTRL_S);
    s.wait_file(&path, "- раз\n- два\n- три\n");
    s.send(CTRL_Z);
    s.wait_gone("- раз");
    s.send(CTRL_S);
    s.wait_file(&path, "\n");
    s.quit();
}

// --- Unix ---

#[cfg(unix)]
mod unix {
    use super::*;

    fn signal(s: &Session, name: &str) {
        let pid = s.child.process_id().unwrap().to_string();
        let status = std::process::Command::new("kill")
            .args([name, &pid])
            .status();
        assert!(status.unwrap().success());
    }

    #[test]
    fn paste_is_verbatim_and_one_undo_step() {
        let home = Home::new();
        let path = home.write("a.md", "\n");
        let mut s = home.open(&[arg(&path)]);
        s.send("\x1b[200~- раз\r- два\x1b[201~");
        s.wait_for("- два");
        s.send(CTRL_S);
        s.wait_file(&path, "- раз\n- два\n");
        s.send(CTRL_Z);
        s.send(CTRL_S);
        s.wait_file(&path, "\n");
        s.quit();
    }

    #[test]
    fn russian_layout_shortcuts() {
        let home = Home::on(Terminal::Kitty);
        let path = home.write("a.md", "текст\n");
        let mut s = home.open(&[arg(&path)]);
        // Ctrl+У, Ctrl+Ы и Ctrl+Й в протоколе Kitty.
        s.send("\x1b[1091;5u");
        s.wait_for("ПРОСМОТР");
        s.send("\x1b[1091;5u");
        s.wait_for("ПРАВКА");
        s.type_text("ж");
        s.send("\x1b[1099;5u");
        s.wait_file(&path, "жтекст\n");
        s.send("\x1b[1081;5u");
        assert_eq!(s.code(), 0);
    }

    #[test]
    fn mouse_selects_text() {
        let home = Home::new();
        let path = home.write("a.md", "раз два\n");
        let mut s = home.open(&[arg(&path)]);
        // Номер строки занимает две колонки.
        s.send("\x1b[<0;3;2M\x1b[<32;7;2M\x1b[<0;7;2m");
        s.send("\x7f");
        s.send(CTRL_S);
        s.wait_file(&path, "два\n");
        s.quit();
    }

    #[test]
    fn file_is_reloaded_while_the_window_is_in_the_background() {
        let home = Home::new();
        let path = home.write("a.md", "первое\n");
        let mut s = home.open(&[arg(&path)]);
        s.send("\x1b[O");
        fs::write(&path, "чужое второе\n").unwrap();
        s.wait_for("чужое второе");
        s.wait_for("перечитан");
        s.send("\x1b[I");
        s.type_text("моё ");
        s.send("\x1b[O");
        fs::write(&path, "чужое третье\n").unwrap();
        s.wait_for("есть и ваши правки");
        s.wait_for("моё чужое второе");
        s.send(DOWN);
        s.send(ENTER);
        s.wait_for("чужое третье");
        s.quit();
        assert_eq!(home.read("a.md"), "чужое третье\n");
    }

    #[test]
    fn terminal_is_restored_on_exit() {
        let home = Home::on(Terminal::Kitty);
        let path = home.write("a.md", "текст\n");
        let mut s = home.open(&[arg(&path)]);
        for on in ["\x1b[?1049h", "\x1b[?2004h", "\x1b[?1004h", "\x1b[>1u"] {
            assert!(s.contains(0, on), "{on:?}");
        }
        s.quit();
        for off in [
            "\x1b[?1049l",
            "\x1b[?2004l",
            "\x1b[?1004l",
            "\x1b[<1u",
            "\x1b[?25h",
        ] {
            s.wait(off, |s| s.contains(0, off));
        }
        let echo = s.master.as_ref().unwrap().get_termios().unwrap();
        assert!(format!("{:?}", echo.local_flags).contains("ECHO"));
    }

    #[test]
    fn signals_keep_unsaved_text() {
        for name in ["-TERM", "-HUP", "-INT"] {
            let home = Home::new();
            let path = home.write("a.md", "старое\n");
            let mut s = home.open(&[arg(&path)]);
            s.type_text("x");
            s.wait_for("xстарое");
            signal(&s, name);
            s.code();
            drop(s);
            let mut s = home.start(&[arg(&path)]);
            s.wait_for("Найдены несохранённые правки");
            s.send(ENTER);
            s.wait_for("xстарое");
            s.send(CTRL_S);
            s.wait_file(&path, "xстарое\n");
            s.quit();
        }
    }

    /// Терминал исчез вместе с окном: программа не должна зависнуть.
    #[test]
    fn closed_terminal_keeps_unsaved_text() {
        let home = Home::new();
        let path = home.write("a.md", "старое\n");
        let mut s = home.open(&[arg(&path)]);
        s.type_text("x");
        s.wait_for("xстарое");
        // Читающий поток держит терминал открытым, пока не прочтёт что-нибудь.
        s.detach.store(true, Ordering::SeqCst);
        s.send("y");
        s.thread.take().unwrap().join().unwrap();
        s.writer = None;
        s.master = None;
        s.code();
        drop(s);
        let mut s = home.start(&[arg(&path)]);
        s.wait_for("Найдены несохранённые правки");
        s.send(ENTER);
        // Прежний текст виден и под вопросом, ждать надо восстановленный.
        s.wait_for("xyстарое");
        s.send(CTRL_S);
        s.wait("сохранение", |_| {
            home.read("a.md").starts_with("xy")
        });
        s.quit();
    }

    #[test]
    fn silent_terminal_loses_no_keys() {
        let home = Home::on(Terminal::Silent);
        let path = home.path("a.md");
        let mut s = home.open(&[arg(&path)]);
        s.type_text("привет");
        s.send(CTRL_S);
        s.wait_file(&path, "привет");
        s.quit();
    }

    #[test]
    fn key_log_does_not_contain_pasted_text() {
        let home = Home::new();
        let mut s = home.start(&["--keys"]);
        s.wait_for("Проверка клавиш");
        s.send(CTRL_S);
        s.send("\x1b[200~секрет\x1b[201~");
        s.wait_for("вставка, символов 6");
        s.send(ESC);
        s.send(ESC);
        assert_eq!(s.code(), 0);
        s.wait("журнал", |s| s.contains(0, "Ctrl+S -> Сохранить"));
        assert!(!s.contains(0, "секрет"));
    }

    #[test]
    fn files_stay_private_and_links_stay_links() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
        let home = Home::new();
        let real = home.write("real.md", "тайна\n");
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        symlink("real.md", home.path("a.md")).unwrap();
        let path = home.path("a.md");
        let mut s = home.open(&[arg(&path)]);
        s.type_text("x");
        s.send(CTRL_S);
        s.wait_file(&real, "xтайна\n");
        s.quit();
        assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        assert_eq!(fs::metadata(&real).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(home.path("a.md.bak")).unwrap().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn web_link_goes_to_the_system_without_a_shell() {
        use std::os::unix::fs::PermissionsExt;
        let mut home = Home::new();
        let bin = home.path("bin");
        fs::create_dir(&bin).unwrap();
        let opener = bin.join("xdg-open");
        let log = home.path("opened");
        fs::write(&opener, "#!/bin/sh\nprintf '%s' \"$1\" > \"$OPENED_LOG\"\n").unwrap();
        fs::set_permissions(&opener, fs::Permissions::from_mode(0o755)).unwrap();
        let search = format!("{}:/usr/bin:/bin", bin.display());
        home.env.push(("PATH", Box::leak(search.into_boxed_str())));
        home.env.push((
            "OPENED_LOG",
            Box::leak(arg(&log).to_owned().into_boxed_str()),
        ));
        let path = home.write("a.md", "[a](https://example.com/?a=1&b=$(id))\n");
        let mut s = home.open(&["--view", arg(&path)]);
        s.send(TAB);
        s.wait_for("Enter: https://example.com");
        s.send(ENTER);
        s.wait_file(&log, "https://example.com/?a=1&b=$(id)");
        s.quit();
    }

    // --- Картинки ---

    const PICTURE: &str =
        "# Картинка\n\n![Подпись](a.png)\n\n![сеть](https://example.com/x.png)\n\n";

    fn with_picture(home: &Home) -> PathBuf {
        image::DynamicImage::new_rgb8(200, 120)
            .save(home.path("a.png"))
            .unwrap();
        home.write("a.md", format!("{PICTURE}{}", "строка\n\n".repeat(60)))
    }

    #[test]
    fn kitty_picture_is_sent_once() {
        let home = Home::on(Terminal::Kitty);
        let path = with_picture(&home);
        let mut s = home.open(&["--view", arg(&path)]);
        s.wait("картинка", |s| {
            s.contains(0, "U=1") && s.contains(0, "\u{10eeee}")
        });
        s.wait_for("▣ сеть (https://example.com/x.png)");
        let sent = |s: &Session| s.raw().windows(3).filter(|w| *w == b"a=T").count();
        let before = sent(&s);
        s.send("\x1b[6~");
        s.send("\x1b[5~");
        s.wait_for("Подпись");
        assert_eq!(sent(&s), before);
        s.quit();
    }

    #[test]
    fn sixel_picture_leaves_no_traces() {
        let home = Home::on(Terminal::Sixel);
        let path = with_picture(&home);
        let mut s = home.open(&["--view", arg(&path)]);
        s.wait("картинка", |s| s.contains(0, "\x1bP"));
        let from = s.raw_len();
        s.send(DOWN);
        s.wait("перерисовка", |s| {
            s.contains(from, "\x1b[2J") && s.contains(from, "\x1bP")
        });
        let from = s.raw_len();
        s.send(F1);
        s.wait("очистка под справкой", |s| {
            s.contains(from, "\x1b[2J")
        });
        s.send(ESC);
        assert!(
            !s.contains(0, "\x1b[6n"),
            "cursor queries would block input"
        );
        s.quit();
    }

    #[test]
    fn pictures_are_text_without_graphics() {
        let home = Home::new();
        let path = with_picture(&home);
        let mut s = home.open(&["--view", arg(&path)]);
        s.wait_for("▣ Подпись (a.png)");
        assert!(!s.contains(0, "\x1bP") && !s.contains(0, "\u{10eeee}"));
        s.quit();

        let mut home = Home::on(Terminal::Kitty);
        home.env.push(("TMUX", "x"));
        let path = with_picture(&home);
        let mut s = home.open(&["--view", arg(&path)]);
        s.wait_for("▣ Подпись (a.png)");
        assert!(!s.contains(0, "a=q"), "tmux is not asked");
        s.quit();
    }
}

const PANIC_RESULT: &str = "RUCONMD_TEST_PANIC_RESULT";

/// Не самостоятельная проверка: её запускает в псевдотерминале следующая.
#[cfg(unix)]
#[test]
#[ignore = "runs inside a pseudoterminal"]
fn panic_in_another_thread_child() {
    use ratatui::crossterm::terminal::is_raw_mode_enabled;

    let Some(result) = std::env::var_os(PANIC_RESULT) else {
        return;
    };
    let mut terminal = ruconmd::term::Terminal::enter(false).unwrap();
    terminal.draw(|_| {}).unwrap();
    assert!(thread::spawn(|| panic!("broken picture")).join().is_err());
    let raw = is_raw_mode_enabled().unwrap();
    terminal.draw(|_| {}).unwrap();
    print!("[after panic]");
    std::io::stdout().flush().unwrap();
    drop(terminal);
    fs::write(result, format!("{raw} {}", is_raw_mode_enabled().unwrap())).unwrap();
}

#[cfg(unix)]
#[test]
fn panic_in_another_thread_leaves_the_screen_alone() {
    let home = Home::new();
    let result = home.path("result");
    let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
    command.args([
        "--exact",
        "panic_in_another_thread_child",
        "--ignored",
        "--nocapture",
    ]);
    command.env(PANIC_RESULT, &result);
    let mut session = home.spawn(command);
    assert_eq!(session.code(), 0);
    session.wait_file(&result, "true false");
    session.wait("сообщение о панике", |s| {
        s.contains(0, "broken picture")
    });
    let raw = session.raw();
    let find = |what: &[u8]| raw.windows(what.len()).position(|w| w == what).unwrap();
    let (marker, leave) = (find(b"[after panic]"), find(b"\x1b[?1049l"));
    let message = find(b"broken picture");
    assert!(marker < leave, "the screen was left because of the panic");
    assert!(leave < message, "the message came over the screen");
}
