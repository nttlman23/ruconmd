//! Чтение и запись файлов.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::recovery::fingerprint;

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
    Ok(read_with_meta(path)?.map(|(bytes, _)| bytes))
}

pub fn read_with_meta(path: &Path) -> Result<Option<(Vec<u8>, fs::Metadata)>, ReadError> {
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
    let bytes = fs::read(path).map_err(ReadError::Io)?;
    Ok(Some((bytes, meta)))
}

/// Записывает файл так, чтобы сбой не оставил его наполовину записанным.
///
/// Новое содержимое пишется рядом и переименовывается. Если так нельзя сохранить
/// владельца, список доступа (ACL), расширенные атрибуты или жёсткие ссылки либо
/// создать файл в каталоге, запись идёт на место.
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

/// Записывает файл точно по этому пути: символическая ссылка заменяется, а не
/// проходится. Права берутся у файла `like`.
pub fn write_exact(
    path: &Path,
    bytes: &[u8],
    like: Option<(&Path, &fs::Metadata)>,
) -> io::Result<()> {
    if path.is_dir() {
        return Err(io::Error::other("is a directory"));
    }
    let (file, temp) = create_temp(path, true)?;
    fill(file, &temp, path, bytes, |file| match like {
        Some((source, meta)) => copy_rights(source, file, meta),
        None => Ok(()),
    })
}

/// Записывает файл, доступный только владельцу, через временный `temp`.
pub fn write_private(path: &Path, temp: &Path, bytes: &[u8]) -> io::Result<()> {
    match fs::remove_file(temp) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let file = new_file(temp, true)?;
    fill(file, temp, path, bytes, |_| Ok(()))
}

/// Готовит временный файл, записывает его и ставит на место `path`.
fn fill(
    mut file: File,
    temp: &Path,
    path: &Path,
    bytes: &[u8],
    prepare: impl FnOnce(&File) -> io::Result<()>,
) -> io::Result<()> {
    let result = (|| {
        // До записи: чужим содержимое не видно ни на миг.
        prepare(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

/// Состояние файла на диске, по которому видны чужие изменения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disk {
    modified: Option<SystemTime>,
    len: u64,
    pub hash: u64,
}

impl Disk {
    pub fn of(meta: &fs::Metadata, bytes: &[u8]) -> Self {
        Self {
            modified: meta.modified().ok(),
            len: meta.len(),
            hash: fingerprint(bytes),
        }
    }

    /// Состояние только что записанного файла.
    pub fn after_write(path: &Path, bytes: &[u8]) -> Option<Self> {
        Some(Self::of(&fs::metadata(path).ok()?, bytes))
    }

    /// Новое состояние, если файл изменился. Содержимое читается, только когда
    /// изменились время или размер.
    pub fn check(path: &Path, known: Option<Disk>) -> Change {
        let Ok(meta) = fs::metadata(path) else {
            return if known.is_some() {
                Change::Gone
            } else {
                Change::None
            };
        };
        if known.is_some_and(|k| k.modified == meta.modified().ok() && k.len == meta.len()) {
            return Change::None;
        }
        match read(path) {
            Ok(Some(bytes)) => {
                let now = Disk::of(&meta, &bytes);
                if known.is_some_and(|k| k.hash == now.hash) {
                    Change::Touched(now)
                } else {
                    Change::Content(now)
                }
            }
            _ => Change::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    None,
    /// Изменилось только время.
    Touched(Disk),
    Content(Disk),
    Gone,
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
    let (file, temp) = create_temp(target, old.is_some())?;
    fill(file, &temp, target, bytes, |file| match old {
        Some(old) => copy_attributes(target, file, old),
        None => Ok(()),
    })?;
    #[cfg(unix)]
    if let Ok(dir) = File::open(parent(target)) {
        let _ = dir.sync_all();
    }
    Ok(())
}

fn create_temp(target: &Path, private: bool) -> io::Result<(File, PathBuf)> {
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
        match new_file(&temp, private) {
            Ok(file) => return Ok((file, temp)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("cannot create a temporary file"))
}

fn new_file(path: &Path, private: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    options.open(path)
}

/// Права резервной копии. Владельца может не выйти сменить, для копии это не
/// беда. Но без группы и списка доступа те же биты открыли бы её другим людям,
/// тогда она остаётся доступной только владельцу.
#[cfg(unix)]
fn copy_rights(source: &Path, file: &File, meta: &fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, fchown};
    let (uid, gid) = (Some(meta.uid()), Some(meta.gid()));
    let new = file.metadata()?;
    let owned = (new.uid(), new.gid()) == (meta.uid(), meta.gid())
        || fchown(file, uid, gid).is_ok()
        || new.gid() == meta.gid()
        || fchown(file, None, gid).is_ok();
    if !owned || xattr::copy(source, file).is_err() {
        return Ok(());
    }
    // Копию не запускают, особые биты ей не нужны.
    file.set_permissions(fs::Permissions::from_mode(meta.mode() & 0o777))
}

#[cfg(not(unix))]
fn copy_rights(_: &Path, file: &File, meta: &fs::Metadata) -> io::Result<()> {
    file.set_permissions(meta.permissions())
}

#[cfg(unix)]
fn copy_attributes(source: &Path, file: &File, old: &fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, fchown};
    let new = file.metadata()?;
    if (new.uid(), new.gid()) != (old.uid(), old.gid()) {
        fchown(file, Some(old.uid()), Some(old.gid()))?;
    }
    // Раньше прав: при списке доступа биты группы означают его маску, без списка
    // они открыли бы файл группе.
    xattr::copy(source, file)?;
    file.set_permissions(old.permissions())
}

#[cfg(not(unix))]
fn copy_attributes(_: &Path, _: &File, _: &fs::Metadata) -> io::Result<()> {
    Ok(())
}

/// Расширенные атрибуты, в том числе списки доступа и метки SELinux.
#[cfg(target_os = "linux")]
mod xattr {
    use std::fs::File;
    use std::io;
    use std::path::Path;

    use rustix::fs::{XattrFlags, fgetxattr, fremovexattr, fsetxattr, getxattr, listxattr};
    use rustix::io::Errno;

    pub const ACL: &[u8] = b"system.posix_acl_access";
    /// Наибольший размер значения и списка имён в Linux.
    const MAX: usize = 64 * 1024;
    /// Ими ведает ядро, у нового содержимого они свои.
    const KERNEL: [&[u8]; 2] = [b"security.ima", b"security.evm"];

    /// Делает атрибуты `file` такими же, как у `source`. Если не вышло, это отказ
    /// в доступе, кроме нехватки места.
    pub fn copy(source: &Path, file: &File) -> io::Result<()> {
        copy_all(source, file).map_err(|e| match e {
            Errno::NOSPC | Errno::DQUOT => e.into(),
            e => io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("extended attributes: {}", io::Error::from(e)),
            ),
        })
    }

    fn copy_all(source: &Path, file: &File) -> rustix::io::Result<()> {
        let mut names = vec![0; MAX];
        let len = match listxattr(source, &mut names[..]) {
            Ok(len) => len,
            // Файловая система без атрибутов.
            Err(Errno::OPNOTSUPP | Errno::NOSYS) => return Ok(()),
            Err(e) => return Err(e),
        };
        let names = || names[..len].split(|b| *b == 0).filter(|n| !n.is_empty());
        let (mut wanted, mut present) = (vec![0; MAX], vec![0; MAX]);
        for name in names().filter(|n| !KERNEL.contains(n)) {
            let len = match getxattr(source, name, &mut wanted[..]) {
                Ok(len) => len,
                // Уже удалён.
                Err(Errno::NODATA) => continue,
                Err(e) => return Err(e),
            };
            let same = fgetxattr(file, name, &mut present[..])
                .is_ok_and(|n| present[..n] == wanted[..len]);
            if !same {
                fsetxattr(file, name, &wanted[..len], XattrFlags::empty())?;
            }
        }
        // Новый файл мог получить список доступа от каталога.
        if !names().any(|n| n == ACL) {
            match fremovexattr(file, ACL) {
                Ok(()) | Err(Errno::NODATA | Errno::OPNOTSUPP | Errno::NOSYS) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
mod xattr {
    pub fn copy(_: &std::path::Path, _: &std::fs::File) -> std::io::Result<()> {
        Ok(())
    }
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
    fn changes_on_disk_are_noticed() {
        let dir = dir();
        let path = dir.path().join("a.md");
        assert_eq!(Disk::check(&path, None), Change::None);
        fs::write(&path, "one").unwrap();
        let Change::Content(known) = Disk::check(&path, None) else {
            panic!("a new file is a change");
        };
        assert_eq!(Disk::check(&path, Some(known)), Change::None);
        fs::write(&path, "another").unwrap();
        let Change::Content(changed) = Disk::check(&path, Some(known)) else {
            panic!("content changed");
        };
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(matches!(
            Disk::check(&path, Some(changed)),
            Change::Touched(_)
        ));
        fs::remove_file(&path).unwrap();
        assert_eq!(Disk::check(&path, Some(changed)), Change::Gone);
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
        fn copy_keeps_the_group_and_its_rights() {
            let dir = dir();
            let (path, copy) = (dir.path().join("a.md"), dir.path().join("a.md.bak"));
            fs::write(&path, "text").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            let groups = std::process::Command::new("id").arg("-G").output().unwrap();
            let groups = String::from_utf8(groups.stdout).unwrap();
            for gid in groups.split_whitespace().map(|g| g.parse().unwrap()) {
                std::os::unix::fs::chown(&path, None, Some(gid)).unwrap();
                let meta = fs::metadata(&path).unwrap();
                write_exact(&copy, b"text", Some((&path, &meta))).unwrap();
                let made = fs::metadata(&copy).unwrap();
                assert_eq!((made.gid(), made.mode() & 0o7777), (gid, 0o640));
            }
        }

        #[test]
        fn copy_is_private_when_the_group_cannot_be_kept() {
            let dir = dir();
            let copy = dir.path().join("copy");
            // Чужой файл с доступом для группы и остальных.
            let source = Path::new("/etc/passwd");
            let meta = fs::metadata(source).unwrap();
            assert_ne!(meta.mode() & 0o044, 0);
            write_exact(&copy, b"text", Some((source, &meta))).unwrap();
            let made = fs::metadata(&copy).unwrap();
            // Суперпользователь и член группы могут её повторить.
            if made.gid() != meta.gid() {
                assert_eq!(made.mode() & 0o7777, 0o600);
            }
            assert_eq!(fs::read(&copy).unwrap(), b"text");
        }

        /// Список доступа: владелец и пользователь 12345 читают и пишут, группа нет.
        #[cfg(target_os = "linux")]
        fn acl() -> Vec<u8> {
            const ANYONE: u32 = u32::MAX;
            let entries = [
                (1u16, 6u16, ANYONE),
                (2, 6, 12345),
                (4, 0, ANYONE),
                (16, 6, ANYONE),
                (32, 0, ANYONE),
            ];
            let mut out = 2u32.to_le_bytes().to_vec();
            for (tag, perm, id) in entries {
                out.extend(tag.to_le_bytes());
                out.extend(perm.to_le_bytes());
                out.extend(id.to_le_bytes());
            }
            out
        }

        #[cfg(target_os = "linux")]
        fn set(path: &Path, name: &[u8], value: &[u8]) -> bool {
            use rustix::fs::{XattrFlags, setxattr};
            match setxattr(path, name, value, XattrFlags::empty()) {
                Ok(()) => true,
                // Файловая система так не умеет, проверять нечего.
                Err(rustix::io::Errno::OPNOTSUPP) => false,
                Err(e) => panic!("{e}"),
            }
        }

        #[cfg(target_os = "linux")]
        fn get(path: &Path, name: &[u8]) -> Option<Vec<u8>> {
            let mut value = vec![0; 1024];
            let len = rustix::fs::getxattr(path, name, &mut value[..]).ok()?;
            value.truncate(len);
            Some(value)
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn keeps_access_lists_and_attributes() {
            let dir = dir();
            let path = dir.path().join("a.md");
            fs::write(&path, "old").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            let inode = fs::metadata(&path).unwrap().ino();
            let with_acl = set(&path, xattr::ACL, &acl());
            let with_user = set(&path, b"user.note", b"value");
            write(&path, b"new").unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"new");
            let meta = fs::metadata(&path).unwrap();
            assert_ne!(meta.ino(), inode, "still replaced, not written in place");
            if with_acl {
                // Биты группы 6: маска, самой группе доступ закрыт.
                assert_eq!(meta.mode() & 0o777, 0o660);
                assert_eq!(get(&path, xattr::ACL), Some(acl()));
            }
            if with_user {
                assert_eq!(get(&path, b"user.note").unwrap(), b"value");
            }

            let copy = dir.path().join("a.md.bak");
            write_exact(&copy, b"old", Some((&path, &meta))).unwrap();
            if with_acl {
                assert_eq!(get(&copy, xattr::ACL), Some(acl()));
            }
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn access_list_of_the_directory_is_not_added() {
            let dir = dir();
            let path = dir.path().join("a.md");
            fs::write(&path, "old").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            if !set(dir.path(), b"system.posix_acl_default", &acl()) {
                return;
            }
            write(&path, b"new").unwrap();
            assert_eq!(get(&path, xattr::ACL), None);
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
