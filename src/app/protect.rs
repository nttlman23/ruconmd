//! Защита данных: копии для восстановления, чужие изменения, `.bak`, автосохранение.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use super::{App, load};
use crate::buffer::Buffer;
use crate::dialog::{Action, SaveJob, Then};
use crate::encoding;
use crate::file::{self, Change, Disk};
use crate::recovery::{self, Meta, Orphan};

/// Как часто проверять файл на диске.
const DISK_INTERVAL: Duration = Duration::from_secs(2);
/// Наибольшее время между проверками, в том числе сигналов завершения.
const MAX_SLEEP: Duration = Duration::from_secs(1);
const BUSY_SLEEP: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Copy)]
pub(super) struct Autosave {
    revision: u64,
    since: Instant,
    /// Эту правку сохранить не удалось, повторять незачем.
    failed: bool,
}

impl App {
    /// Фоновая работа. Вызывается в каждом обороте основного цикла.
    pub fn tick(&mut self, now: Instant) {
        self.check_disk(now);
        self.run_autosave(now);
        self.update_recovery(now);
        self.refresh_files(now);
    }

    /// Через сколько вызвать [`App::tick`], если ввода нет.
    pub fn sleep(&self) -> Duration {
        if self.is_busy() {
            BUSY_SLEEP
        } else {
            MAX_SLEEP
        }
    }

    /// Обычное завершение: копия для восстановления больше не нужна.
    pub fn finish(&mut self) {
        if let Some(r) = &mut self.recovery {
            r.clear();
        }
    }

    /// Аварийное завершение. Возвращает путь к копии несохранённых правок.
    pub fn emergency_save(&mut self) -> Option<PathBuf> {
        if !self.is_modified() {
            self.finish();
            return None;
        }
        let (meta, revision, rope) = (self.meta(), self.buffer.revision(), self.buffer.rope());
        let r = self.recovery.as_mut()?;
        r.write_now(revision, meta, rope).ok()?;
        Some(r.path().to_owned())
    }

    // --- Восстановление ---

    fn meta(&self) -> Meta {
        Meta {
            format: self.format,
            base: self.disk.map(|d| d.hash),
        }
    }

    fn update_recovery(&mut self, now: Instant) {
        let (meta, modified) = (self.meta(), self.is_modified());
        let Some(r) = &mut self.recovery else { return };
        let buffer = &self.buffer;
        let result = r.update(now, buffer.revision(), modified, meta, || buffer.rope());
        match result {
            Ok(()) => {}
            Err(e) if !self.recovery_failed => {
                self.recovery_failed = true;
                let text = self.lang.strings().no_recovery;
                self.message = Some(format!("{text}: {e}"));
            }
            Err(_) => {}
        }
    }

    /// Предлагает правки, оставшиеся от прерванного сеанса.
    pub(super) fn offer_recovery(&mut self) {
        let Some(dir) = &self.state_dir else { return };
        let found = recovery::search(dir, self.path.as_deref());
        let s = self.lang.strings();
        if found.open_elsewhere {
            self.message = Some(s.open_elsewhere.into());
        }
        let Some(orphan) = found.orphan else { return };
        if orphan.text == self.buffer.text() {
            return orphan.remove();
        }
        let mut title = format!("{}: {}", s.found_recovery, self.age(&orphan));
        if orphan.meta.base != self.disk.map(|d| d.hash) {
            title = format!("{title}; {}", s.changed_since);
        }
        self.orphan = Some(orphan);
        self.menu(
            &title,
            vec![
                (s.restore.into(), Action::Restore),
                (s.not_now.into(), Action::Cancel),
                (s.drop_copy.into(), Action::DropCopy),
            ],
        );
    }

    fn age(&self, orphan: &Orphan) -> String {
        let age = SystemTime::now().duration_since(orphan.saved);
        let minutes = age.map_or(0, |d| d.as_secs() / 60);
        let s = self.lang.strings();
        match minutes {
            0..=59 => format!("{minutes} {}", s.minutes_ago),
            60..=1439 => format!("{} {}", minutes / 60, s.hours_ago),
            _ => format!("{} {}", minutes / 1440, s.days_ago),
        }
    }

    pub(super) fn restore(&mut self) {
        let Some(orphan) = self.orphan.take() else {
            return;
        };
        self.buffer.replace_all(&orphan.text);
        self.format = orphan.meta.format;
        self.edited();
        let s = self.lang.strings();
        // Прежняя копия удаляется, только когда правки записаны в свою.
        match self.emergency_save() {
            Some(_) => {
                orphan.remove();
                self.message = Some(s.restored.into());
            }
            None => self.message = Some(s.no_recovery.into()),
        }
    }

    pub(super) fn drop_copy(&mut self) {
        if let Some(orphan) = self.orphan.take() {
            orphan.remove();
        }
    }

    // --- Изменения на диске ---

    fn check_disk(&mut self, now: Instant) {
        if now < self.next_check || self.dialog.is_some() || self.help {
            return;
        }
        self.next_check = now + DISK_INTERVAL;
        let Some(path) = &self.path else { return };
        let s = self.lang.strings();
        match Disk::check(path, self.disk) {
            Change::None => {}
            Change::Touched(disk) => self.disk = Some(disk),
            // Без фокуса и своих правок спрашивать не о чем.
            Change::Content(_) if !self.in_focus && !self.has_own_changes() => {
                let follow = self.follow;
                self.message = None;
                self.take_disk();
                // Прокрученное не возвращается к курсору.
                self.follow = follow;
                if self.message.is_none() {
                    self.message = Some(s.reloaded.into());
                }
            }
            Change::Content(disk) => {
                // Если закрыть вопрос без ответа, останется своя версия.
                self.disk = Some(disk);
                self.disk_differs = true;
                let title = if self.buffer.is_modified() {
                    s.changed_on_both_sides
                } else {
                    s.changed_on_disk
                };
                self.menu(
                    title,
                    vec![
                        (s.keep_mine.into(), Action::Cancel),
                        (s.reread.into(), Action::TakeDisk),
                        (s.save_as.into(), Action::SaveAs),
                    ],
                );
            }
            Change::Gone => {
                self.disk = None;
                self.disk_differs = true;
                self.message = Some(s.gone_from_disk.into());
            }
        }
    }

    /// Есть ли что терять при перечитывании. Удалённый и вновь созданный файл сам по
    /// себе не правка: так его сохраняют некоторые программы.
    fn has_own_changes(&self) -> bool {
        let kept = self.disk_differs && self.disk.is_some();
        self.buffer.is_modified() || self.format != self.saved_format || kept
    }

    /// Изменения, сделанные до возвращения фокуса, ещё считаются фоновыми.
    pub(super) fn focus_changed(&mut self, focus: bool) {
        self.dragging = false;
        if focus && !self.in_focus {
            let now = Instant::now();
            self.next_check = now;
            self.check_disk(now);
        }
        self.in_focus = focus;
    }

    /// Перечитывает файл. Прежний текст можно вернуть отменой.
    pub(super) fn take_disk(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        match load(&path, self.forced, self.lang) {
            Ok(loaded) => {
                self.buffer.reload(&loaded.text);
                self.buffer.mark_saved();
                self.accept_file(&loaded);
                self.edited();
            }
            Err(e) => self.message = Some(e.to_string()),
        }
    }

    /// Не записывает поверх чужих изменений без разрешения. `true`: спросили.
    pub(super) fn ask_about_disk(&mut self, job: &SaveJob) -> bool {
        match Disk::check(&job.path, self.disk) {
            Change::Content(_) => {}
            Change::Touched(disk) => {
                self.disk = Some(disk);
                return false;
            }
            Change::None | Change::Gone => return false,
        }
        let s = self.lang.strings();
        let yes = SaveJob {
            ignore_disk: true,
            ..job.clone()
        };
        self.menu(
            s.changed_on_disk,
            vec![
                (s.cancel.into(), Action::Cancel),
                (s.overwrite.into(), Action::Save(yes)),
                (s.reread.into(), Action::TakeDisk),
                (s.save_as.into(), Action::SaveAs),
            ],
        );
        true
    }

    // --- Резервная копия ---

    /// Копирует файл в `.bak` перед первой записью за сеанс.
    pub(super) fn back_up(&mut self, path: &Path, new: &[u8]) -> Result<(), String> {
        let Ok(real) = path.canonicalize() else {
            return Ok(());
        };
        if !self.backup || self.backed_up.contains(&real) {
            return Ok(());
        }
        let Ok(Some((old, meta))) = file::read_with_meta(path) else {
            return Ok(());
        };
        if old == new {
            return Ok(());
        }
        let mut name = OsString::from(path);
        name.push(".bak");
        file::write_exact(Path::new(&name), &old, Some((path, &meta)))
            .map_err(|e| e.to_string())?;
        self.backed_up.insert(real);
        Ok(())
    }

    // --- Автосохранение ---

    fn run_autosave(&mut self, now: Instant) {
        let Some(delay) = self.autosave else { return };
        if !self.is_modified() {
            self.autosave_state = None;
            return;
        }
        let revision = self.buffer.revision();
        let state = match self.autosave_state {
            Some(state) if state.revision == revision => state,
            _ => {
                self.autosave_state = Some(Autosave {
                    revision,
                    since: now,
                    failed: false,
                });
                return;
            }
        };
        let busy = self.dialog.is_some() || self.help || self.disk_differs || self.lossy;
        if busy || state.failed || now.duration_since(state.since) < delay {
            return;
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        // Чужие изменения покажет очередная проверка диска.
        if matches!(Disk::check(&path, self.disk), Change::Content(_)) {
            return;
        }
        let message = self.message.take();
        if encoding::encode(&self.buffer.text(), self.format).is_err() {
            self.message = Some(self.lang.strings().autosave_encoding.into());
        } else {
            self.write(SaveJob {
                path,
                format: self.format,
                then: Then::Stay,
                overwrite: true,
                allow_lossy: false,
                ignore_disk: false,
            });
        }
        if !self.is_modified() {
            self.message = message;
        } else if let Some(state) = &mut self.autosave_state {
            state.failed = true;
        }
    }

    pub(super) fn after_write(&mut self, path: &Path, bytes: &[u8]) {
        let renamed = self.path.as_deref() != Some(path);
        self.path = Some(path.to_owned());
        if renamed {
            // Пути картинок считаются от нового места.
            self.pane.reset(Some(path.to_owned()));
        }
        self.disk = Disk::after_write(path, bytes);
        self.disk_differs = false;
        if renamed && let Some(r) = &mut self.recovery {
            r.retarget(Some(path));
        }
    }

    /// Меняет документ на другой файл.
    pub(super) fn open_other(&mut self, path: PathBuf, loaded: super::Loaded) {
        if let Some(r) = &mut self.recovery {
            r.retarget(Some(&path));
        }
        self.path = Some(path);
        self.forced = None;
        self.buffer = Buffer::new("");
        self.view.scroll_to_start();
        self.set_content(loaded);
        self.offer_recovery();
    }
}
