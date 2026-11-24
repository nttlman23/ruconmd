//! Файлы восстановления: копии несохранённых правок на случай сбоя.
//!
//! Имя файла состоит из отпечатка пути документа и номера сеанса. Рядом лежит
//! заблокированный файл: пока блокировка держится, сеанс жив, и его копию никто
//! не предлагает восстановить.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ropey::Rope;

use crate::encoding::{Enc, Format};
use crate::file;

/// Наибольшая потеря правок при сбое.
pub const INTERVAL: Duration = Duration::from_secs(5);

const MAGIC: &str = "ruconmd-recovery 1";

/// Отпечаток FNV-1a: одинаков во всех версиях программы.
pub fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Отпечаток документа: один и тот же для всех путей к файлу.
fn document_id(path: Option<&Path>) -> u64 {
    let Some(path) = path else { return 0 };
    let real = path.canonicalize().or_else(|_| {
        // Файла ещё нет, но каталог уже может быть.
        let full = std::path::absolute(path)?;
        match (full.parent(), full.file_name()) {
            (Some(dir), Some(name)) => Ok(dir.canonicalize()?.join(name)),
            _ => Ok(full),
        }
    });
    let real = real.unwrap_or_else(|_: io::Error| path.to_owned());
    fingerprint(real.as_os_str().as_encoded_bytes())
}

/// Что известно о тексте, кроме него самого.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub format: Format,
    /// Отпечаток файла на диске, от которого идут правки.
    pub base: Option<u64>,
}

fn encode(meta: Meta, text: &str) -> String {
    let base = meta.base.map_or("-".into(), |b| format!("{b:016x}"));
    let (enc, bom) = (meta.format.enc.name(), u8::from(meta.format.bom));
    format!("{MAGIC} {enc} {bom} {base}\n{text}")
}

fn decode(content: &str) -> Option<(Meta, String)> {
    let (header, text) = content.split_once('\n')?;
    let mut fields = header.strip_prefix(MAGIC)?.split_whitespace();
    let enc: Enc = fields.next()?.parse().ok()?;
    let bom = fields.next()? == "1";
    let base = match fields.next()? {
        "-" => None,
        hex => Some(u64::from_str_radix(hex, 16).ok()?),
    };
    let format = Format { enc, bom };
    Some((Meta { format, base }, text.to_owned()))
}

/// Копия, оставшаяся от прерванного сеанса.
#[derive(Debug)]
pub struct Orphan {
    file: PathBuf,
    pub saved: SystemTime,
    pub meta: Meta,
    pub text: String,
}

impl Orphan {
    pub fn remove(&self) {
        let _ = fs::remove_file(&self.file);
        let _ = fs::remove_file(self.file.with_extension("lock"));
    }
}

#[derive(Debug, Default)]
pub struct Search {
    /// Самая свежая брошенная копия.
    pub orphan: Option<Orphan>,
    /// Этот же документ с несохранёнными правками открыт в другом окне.
    pub open_elsewhere: bool,
}

/// Ищет копии документа, оставленные другими сеансами.
pub fn search(dir: &Path, document: Option<&Path>) -> Search {
    let mut found = Search::default();
    let prefix = format!("{:016x}-", document_id(document));
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let file = entry.path();
        let name = entry.file_name();
        let matches = name.to_str().is_some_and(|n| n.starts_with(&prefix));
        if !matches || file.extension().is_none_or(|e| e != "rec") {
            continue;
        }
        if is_locked(&file.with_extension("lock")) {
            found.open_elsewhere = true;
            continue;
        }
        let saved = entry.metadata().and_then(|m| m.modified());
        let saved = saved.unwrap_or(UNIX_EPOCH);
        if found.orphan.as_ref().is_some_and(|o| o.saved >= saved) {
            continue;
        }
        let content = fs::read_to_string(&file).ok();
        if let Some((meta, text)) = content.as_deref().and_then(decode) {
            found.orphan = Some(Orphan {
                file,
                saved,
                meta,
                text,
            });
        }
    }
    found
}

fn is_locked(path: &Path) -> bool {
    match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => matches!(file.try_lock(), Err(TryLockError::WouldBlock)),
        Err(_) => false,
    }
}

/// Копия текущего сеанса.
pub struct Recovery {
    dir: PathBuf,
    file: PathBuf,
    /// Держит блокировку, пока копия нужна.
    lock: Option<File>,
    job: Option<JoinHandle<io::Result<()>>>,
    /// Правка текста, которая уже записана.
    written: Option<u64>,
    /// С какого времени есть незаписанные правки.
    pending: Option<Instant>,
}

impl Recovery {
    pub fn new(dir: PathBuf, document: Option<&Path>) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let name = format!(
            "{:016x}-{}-{nanos:x}.rec",
            document_id(document),
            std::process::id()
        );
        Self {
            file: dir.join(name),
            dir,
            lock: None,
            job: None,
            written: None,
            pending: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.file
    }

    /// Переходит к другому документу. Копия прежнего удаляется.
    pub fn retarget(&mut self, document: Option<&Path>) {
        self.clear();
        *self = Self::new(self.dir.clone(), document);
    }

    /// Сообщает о состоянии текста. Записывает копию, когда пора.
    pub fn update(
        &mut self,
        now: Instant,
        revision: u64,
        modified: bool,
        meta: Meta,
        rope: impl FnOnce() -> Rope,
    ) -> io::Result<()> {
        if !modified {
            self.clear();
            return Ok(());
        }
        let result = self.finish(false);
        if self.written == Some(revision) {
            self.pending = None;
            return result;
        }
        let since = *self.pending.get_or_insert(now);
        if result.is_ok() && self.job.is_none() && now.duration_since(since) >= INTERVAL {
            self.start(revision, meta, rope())?;
        }
        result
    }

    /// Когда понадобится следующая запись.
    pub fn due(&self) -> Option<Instant> {
        Some(self.pending? + INTERVAL)
    }

    fn start(&mut self, revision: u64, meta: Meta, rope: Rope) -> io::Result<()> {
        self.prepare()?;
        let file = self.file.clone();
        self.written = Some(revision);
        self.pending = None;
        self.job = Some(std::thread::spawn(move || {
            file::write_exact(&file, encode(meta, &rope.to_string()).as_bytes(), None)
        }));
        Ok(())
    }

    /// Записывает копию немедленно.
    pub fn write_now(&mut self, revision: u64, meta: Meta, rope: Rope) -> io::Result<()> {
        let _ = self.finish(true);
        if self.written == Some(revision) && self.file.exists() {
            return Ok(());
        }
        self.start(revision, meta, rope)?;
        self.finish(true)
    }

    fn prepare(&mut self) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700))?;
        }
        if self.lock.is_none() {
            let lock = File::create(self.file.with_extension("lock"))?;
            // Без блокировок копия просто будет считаться брошенной.
            let _ = lock.try_lock();
            self.lock = Some(lock);
        }
        Ok(())
    }

    /// Забирает итог фоновой записи.
    fn finish(&mut self, wait: bool) -> io::Result<()> {
        if !self.job.as_ref().is_some_and(|j| wait || j.is_finished()) {
            return Ok(());
        }
        let Some(job) = self.job.take() else {
            return Ok(());
        };
        let result = match job.join() {
            Ok(result) => result,
            Err(_) => Err(io::Error::other("recovery thread panicked")),
        };
        if result.is_err() {
            // Попробуем ещё раз через обычный срок.
            self.written = None;
            self.pending = Some(Instant::now());
        }
        result
    }

    /// Удаляет копию: правки сохранены или больше не нужны.
    pub fn clear(&mut self) {
        let _ = self.finish(true);
        self.pending = None;
        if self.written.take().is_some() || self.lock.is_some() {
            let _ = fs::remove_file(&self.file);
            // Windows не удаляет открытый файл.
            self.lock = None;
            let _ = fs::remove_file(self.file.with_extension("lock"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: Meta = Meta {
        format: Format {
            enc: Enc::Cp1251,
            bom: false,
        },
        base: Some(0xabc),
    };

    fn rope(text: &str) -> Rope {
        Rope::from_str(text)
    }

    /// Сеанс, прерванный сбоем: блокировка снята, файлы остались.
    fn crash(mut r: Recovery) {
        r.lock = None;
    }

    #[test]
    fn content_round_trips() {
        for text in ["", "текст\r\nещё\n", "ruconmd-recovery 1\n"] {
            for base in [None, Some(0), Some(u64::MAX)] {
                let meta = Meta { base, ..META };
                assert_eq!(decode(&encode(meta, text)), Some((meta, text.to_owned())));
            }
        }
        for bad in ["", "мусор\nтекст", "ruconmd-recovery 2 UTF-8 0 -\n", MAGIC] {
            assert_eq!(decode(bad), None);
        }
    }

    #[test]
    fn crashed_session_is_found_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("a.md");
        let state = dir.path().join("state");
        let mut r = Recovery::new(state.clone(), Some(&doc));
        r.write_now(1, META, rope("правки")).unwrap();
        assert!(
            search(&state, Some(&doc)).orphan.is_none(),
            "session is alive"
        );
        assert!(search(&state, Some(&doc)).open_elsewhere);
        crash(r);
        let found = search(&state, Some(&doc));
        let orphan = found.orphan.unwrap();
        assert!(!found.open_elsewhere);
        assert_eq!((orphan.text.as_str(), orphan.meta), ("правки", META));
        assert!(
            search(&state, Some(&dir.path().join("b.md")))
                .orphan
                .is_none()
        );
        assert!(search(&state, None).orphan.is_none());
        orphan.remove();
        assert_eq!(fs::read_dir(&state).unwrap().count(), 0);
    }

    #[test]
    fn newest_copy_wins_and_damaged_ones_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let mut old = Recovery::new(dir.path().into(), None);
        old.write_now(1, META, rope("старое")).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let mut new = Recovery::new(dir.path().into(), None);
        new.write_now(1, META, rope("новое")).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let mut bad = Recovery::new(dir.path().into(), None);
        bad.write_now(1, META, rope("")).unwrap();
        fs::write(&bad.file, b"\xff\xfe").unwrap();
        crash(old);
        crash(new);
        crash(bad);
        assert_eq!(search(dir.path(), None).orphan.unwrap().text, "новое");
    }

    #[test]
    fn same_document_by_different_paths() {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("a.md");
        let other = dir.path().join(".").join("a.md");
        assert_eq!(document_id(Some(&doc)), document_id(Some(&other)));
        fs::write(&doc, "").unwrap();
        assert_eq!(document_id(Some(&doc)), document_id(Some(&other)));
        assert_ne!(document_id(Some(&doc)), document_id(None));
    }

    #[test]
    fn writes_on_schedule_and_clears_after_saving() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = Recovery::new(dir.path().into(), None);
        let start = Instant::now();
        let text = || rope("x");
        r.update(start, 1, true, META, text).unwrap();
        assert_eq!(r.due(), Some(start + INTERVAL));
        // Новые правки не откладывают запись.
        let later = start + INTERVAL - Duration::from_millis(1);
        r.update(later, 2, true, META, text).unwrap();
        r.finish(true).unwrap();
        assert!(!r.file.exists());
        r.update(start + INTERVAL, 3, true, META, text).unwrap();
        r.finish(true).unwrap();
        assert!(r.file.exists());
        r.update(start + INTERVAL * 3, 3, true, META, text).unwrap();
        assert_eq!(r.due(), None, "already written");
        r.update(start + INTERVAL * 3, 3, false, META, text)
            .unwrap();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn retarget_removes_the_old_copy() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = Recovery::new(dir.path().into(), None);
        r.write_now(1, META, rope("x")).unwrap();
        r.retarget(Some(Path::new("other.md")));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn copies_are_private() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("a/b");
        let mut r = Recovery::new(state.clone(), None);
        r.write_now(1, META, rope("x")).unwrap();
        assert_eq!(fs::metadata(&state).unwrap().mode() & 0o777, 0o700);
    }

    #[test]
    fn unwritable_place_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("file");
        fs::write(&blocked, "").unwrap();
        let mut r = Recovery::new(blocked.join("state"), None);
        assert!(r.write_now(1, META, rope("x")).is_err());
        r.clear();
    }
}
