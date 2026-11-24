//! Чтение и запись файлов.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Файлы крупнее открываются с предупреждением.
pub const LARGE: usize = 10 * 1024 * 1024;

#[derive(Debug)]
pub enum ReadError {
    Directory,
    /// Канал, устройство и тому подобное: чтение могло бы не закончиться.
    NotRegular,
    Io(io::Error),
}

/// Содержимое файла или `None`, если его ещё нет.
pub fn read(path: &Path) -> Result<Option<Vec<u8>>, ReadError> {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        // Битая символическая ссылка тоже «не существует», файл создастся по ней.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ReadError::Io(e)),
    };
    if meta.is_dir() {
        return Err(ReadError::Directory);
    }
    if !meta.is_file() {
        return Err(ReadError::NotRegular);
    }
    fs::read(path).map(Some).map_err(ReadError::Io)
}

/// Записывает файл так, чтобы сбой не оставил его наполовину записанным.
///
/// Новое содержимое пишется рядом и переименовывается. Если так нельзя сохранить
/// владельца или жёсткие ссылки либо создать файл в каталоге, запись идёт на место.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = resolve(path)?;
    match fs::metadata(&target) {
        Ok(meta) => {
            if !meta.is_file() {
                return Err(io::Error::other("not a regular file"));
            }
            // Переименование обошло бы запрет записи в сам файл.
            OpenOptions::new().write(true).open(&target)?;
            if !can_replace(&meta) {
                return write_in_place(&target, bytes);
            }
            match replace(&target, bytes, Some(&meta)) {
                // Только отказ в доступе: при нехватке места запись на место
                // обрезала бы исходный файл.
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    write_in_place(&target, bytes)
                }
                result => result,
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => replace(&target, bytes, None),
        Err(e) => Err(e),
    }
}

/// Проходит по символическим ссылкам, в том числе к ещё не созданному файлу.
fn resolve(path: &Path) -> io::Result<PathBuf> {
    const MAX_LINKS: usize = 40;
    let mut target = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        match fs::read_link(&target) {
            Ok(link) => target = parent(&target).join(link),
            Err(_) => return Ok(target),
        }
    }
    Err(io::Error::other("too many symbolic links"))
}

fn parent(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

#[cfg(unix)]
fn can_replace(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.nlink() == 1
}

#[cfg(not(unix))]
fn can_replace(_: &fs::Metadata) -> bool {
    true
}

fn replace(target: &Path, bytes: &[u8], old: Option<&fs::Metadata>) -> io::Result<()> {
    let (mut file, temp) = create_temp(target)?;
    let result = (|| {
        file.write_all(bytes)?;
        if let Some(old) = old {
            copy_attributes(&file, old)?;
        }
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
        return result;
    }
    #[cfg(unix)]
    if let Ok(dir) = File::open(parent(target)) {
        let _ = dir.sync_all();
    }
    result
}

fn create_temp(target: &Path) -> io::Result<(File, PathBuf)> {
    if target.file_name().is_none() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no file name"));
    }
    for attempt in 0..16 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        // Имя не зависит от исходного: вместе они могли бы превысить предел длины.
        let name = format!(".ruconmd-{}-{nanos}-{attempt}.tmp", std::process::id());
        let temp = target.with_file_name(name);
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => return Ok((file, temp)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("cannot create a temporary file"))
}

#[cfg(unix)]
fn copy_attributes(file: &File, old: &fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, fchown};
    let new = file.metadata()?;
    if (new.uid(), new.gid()) != (old.uid(), old.gid()) {
        fchown(file, Some(old.uid()), Some(old.gid()))?;
    }
    file.set_permissions(old.permissions())
}

#[cfg(not(unix))]
fn copy_attributes(_: &File, _: &fs::Metadata) -> io::Result<()> {
    Ok(())
}

fn write_in_place(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).truncate(true).open(target)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn entries(dir: &Path) -> usize {
        fs::read_dir(dir).unwrap().count()
    }

    #[test]
    fn creates_and_overwrites_without_leftovers() {
        let dir = dir();
        let path = dir.path().join("файл.md");
        assert!(read(&path).unwrap().is_none());
        write(&path, b"one").unwrap();
        write(&path, b"2").unwrap();
        assert_eq!(read(&path).unwrap().unwrap(), b"2");
        assert_eq!(entries(dir.path()), 1);
    }

    #[test]
    fn refuses_directories() {
        let dir = dir();
        assert!(matches!(read(dir.path()), Err(ReadError::Directory)));
        assert!(write(dir.path(), b"x").is_err());
        assert!(write(&dir.path().join("none/file.md"), b"x").is_err());
    }

    #[test]
    fn read_only_file_is_not_replaced() {
        let dir = dir();
        let path = dir.path().join("a.md");
        fs::write(&path, "old").unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&path, perms).unwrap();
        // Суперпользователю запрет не помеха.
        if OpenOptions::new().write(true).open(&path).is_ok() {
            return;
        }
        assert!(write(&path, b"new").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(entries(dir.path()), 1);
        #[cfg(windows)]
        {
            let mut perms = fs::metadata(&path).unwrap().permissions();
            #[expect(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            fs::set_permissions(&path, perms).unwrap();
        }
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

        #[test]
        fn keeps_permissions() {
            let dir = dir();
            let path = dir.path().join("a.md");
            fs::write(&path, "old").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            write(&path, b"new").unwrap();
            assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o640);
        }

        #[test]
        fn keeps_symlinks_and_writes_through_dangling_ones() {
            let dir = dir();
            let real = dir.path().join("real.md");
            let link = dir.path().join("link.md");
            symlink("real.md", &link).unwrap();
            for text in ["created", "changed"] {
                write(&link, text.as_bytes()).unwrap();
                assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
                assert_eq!(fs::read_to_string(&real).unwrap(), text);
            }
        }

        #[test]
        fn keeps_hard_links() {
            let dir = dir();
            let (a, b) = (dir.path().join("a.md"), dir.path().join("b.md"));
            fs::write(&a, "a long old text").unwrap();
            fs::hard_link(&a, &b).unwrap();
            write(&a, b"new").unwrap();
            assert_eq!(fs::read(&b).unwrap(), b"new");
        }

        #[test]
        fn longest_names_can_be_saved() {
            let dir = dir();
            let path = dir.path().join("ж".repeat(127));
            write(&path, b"old").unwrap();
            write(&path, b"new").unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"new");
        }

        #[test]
        fn symlink_loop_is_an_error() {
            let dir = dir();
            let link = dir.path().join("loop.md");
            symlink("loop.md", &link).unwrap();
            assert!(write(&link, b"x").is_err());
        }

        #[test]
        fn fifo_is_refused() {
            assert!(matches!(
                read(Path::new("/dev/null")),
                Err(ReadError::NotRegular)
            ));
        }
    }
}
