//! Обновление программы командой `ruconmd --update`: список выпусков на GitHub,
//! загрузка и замена исполняемого файла.
//!
//! Других обращений к сети в программе нет, сама она обновлений не ищет.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use semver::Version;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::file;
use crate::i18n::Lang;

const RELEASES: &str = "https://api.github.com/repos/nttlman23/ruconmd/releases?per_page=30";
const DOWNLOADS: &str = "https://github.com/nttlman23/ruconmd/releases/download";
const PAGE: &str = "https://github.com/nttlman23/ruconmd/releases";
const SUMS: &str = "SHA256SUMS";

const LIST_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_LIST: u64 = 4 * 1024 * 1024;
const MAX_SUMS: u64 = 64 * 1024;
const MAX_PROGRAM: u64 = 64 * 1024 * 1024;

/// Конец имени файла выпуска и начало его содержимого. `None`: для этой системы
/// готовых файлов нет.
const PLATFORM: Option<(&str, &[u8])> = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some(("linux-x86_64.bin", b"\x7fELF"))
} else if cfg!(all(windows, target_arch = "x86_64")) {
    Some(("windows-x86_64.exe", b"MZ"))
} else {
    None
};

/// Откуда берутся данные. В тестах сети нет.
trait Source {
    fn get(&self, url: &str, limit: u64, timeout: Duration) -> Result<Vec<u8>, String>;
}

struct Http;

impl Source for Http {
    fn get(&self, url: &str, limit: u64, timeout: Duration) -> Result<Vec<u8>, String> {
        let agent = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(5)
            .timeout_global(Some(timeout))
            .user_agent(concat!("ruconmd/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        // Имя сервера и прокси: из текста самой ошибки не видно, что не нашлось.
        let via = agent.config().proxy().map(|p| (p.host(), p.port()));
        let explain = |e: ureq::Error| explain(url, via, &e.to_string());
        let mut response = agent.get(url).call().map_err(explain)?;
        let body = response.body_mut().with_config().limit(limit);
        body.read_to_vec().map_err(explain)
    }
}

/// Текст ошибки с тем, к чему было обращение. Имя и пароль прокси не выводятся.
fn explain(url: &str, proxy: Option<(&str, u16)>, error: &str) -> String {
    let host = url.split('/').nth(2).unwrap_or(url);
    match proxy {
        Some((name, port)) => format!("{host} (proxy {name}:{port}): {error}"),
        None => format!("{host}: {error}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    Network(String),
    /// Вместо списка выпусков пришло что-то другое.
    BadList,
    /// Файл не тот, что описан в выпуске.
    Checksum,
    /// Файл программы заменить не удалось.
    Replace(String),
}

/// Обновляет работающую программу и сообщает о ходе дела.
pub fn run(lang: Lang) -> io::Result<()> {
    let s = lang.strings();
    let platform = PLATFORM.ok_or_else(|| io::Error::other(s.update_no_platform))?;
    let current = Version::parse(env!("CARGO_PKG_VERSION")).map_err(io::Error::other)?;
    let program = std::env::current_exe()?;
    let mut out = io::stdout();
    match update(&Http, &current, platform, &program, lang, &mut out) {
        Ok(()) => Ok(()),
        Err(failure) => {
            let reason = match failure {
                Failure::Network(e) => format!("{}: {e}", s.update_network),
                Failure::BadList => s.update_bad_list.to_owned(),
                Failure::Checksum => s.update_checksum.to_owned(),
                Failure::Replace(e) => format!("{}: {e}", s.update_replace),
            };
            Err(io::Error::other(format!("{reason}\n{PAGE}")))
        }
    }
}

fn update(
    source: &dyn Source,
    current: &Version,
    platform: (&str, &[u8]),
    program: &Path,
    lang: Lang,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let s = lang.strings();
    let list = source.get(RELEASES, MAX_LIST, LIST_TIMEOUT);
    let list = list.map_err(Failure::Network)?;
    let Some(version) = newest(&list, current, platform.0)? else {
        // Вывода может не быть, обновлению это не помеха.
        let _ = writeln!(out, "{}: {current}", s.update_none);
        return Ok(());
    };
    // Незачем загружать то, что некуда поставить.
    let probe = file::write_beside(program, b"").and_then(fs::remove_file);
    probe.map_err(|e| Failure::Replace(e.to_string()))?;
    let _ = writeln!(out, "{}: {current} -> {version}", s.update_loading);
    let bytes = download(source, &version, platform)?;
    replace(program, &bytes).map_err(|e| Failure::Replace(e.to_string()))?;
    let _ = writeln!(out, "{}: {version}", s.update_done);
    Ok(())
}

fn asset_name(version: &Version, platform: &str) -> String {
    format!("ruconmd-v{version}-{platform}")
}

/// Самый новый выпуск, на который можно перейти. Предварительные предлагаются
/// только тем, у кого уже предварительный.
fn newest(list: &[u8], current: &Version, platform: &str) -> Result<Option<Version>, Failure> {
    let list: Value = serde_json::from_slice(list).map_err(|_| Failure::BadList)?;
    let list = list.as_array().ok_or(Failure::BadList)?;
    let fits = |release: &Value| {
        let flag = |name: &str| release.get(name).and_then(Value::as_bool);
        let tag = release.get("tag_name")?.as_str()?;
        let version = Version::parse(tag.strip_prefix('v')?).ok()?;
        let early = flag("prerelease")? || !version.pre.is_empty();
        let names: Vec<&str> = release
            .get("assets")?
            .as_array()?
            .iter()
            .filter_map(|a| a.get("name")?.as_str())
            .collect();
        let complete = names.contains(&SUMS) && names.contains(&&*asset_name(&version, platform));
        let good = !flag("draft")?
            && version.build.is_empty()
            && version > *current
            && (!early || !current.pre.is_empty())
            && complete;
        good.then_some(version)
    };
    Ok(list.iter().filter_map(fits).max())
}

fn download(
    source: &dyn Source,
    version: &Version,
    (platform, magic): (&str, &[u8]),
) -> Result<Vec<u8>, Failure> {
    let name = asset_name(version, platform);
    let get = |name: &str, limit| {
        let url = format!("{DOWNLOADS}/v{version}/{name}");
        let got = source.get(&url, limit, DOWNLOAD_TIMEOUT);
        got.map_err(Failure::Network)
    };
    let sums = get(SUMS, MAX_SUMS)?;
    let expected = checksum(&sums, &name).ok_or(Failure::Checksum)?;
    let bytes = get(&name, MAX_PROGRAM)?;
    let actual: [u8; 32] = Sha256::digest(&bytes).into();
    if actual != expected || !bytes.starts_with(magic) {
        return Err(Failure::Checksum);
    }
    Ok(bytes)
}

/// Сумма файла из вывода `sha256sum`. Файл должен быть назван ровно один раз.
fn checksum(sums: &[u8], name: &str) -> Option<[u8; 32]> {
    let sums = std::str::from_utf8(sums).ok()?;
    let mut found = sums.lines().filter_map(|line| {
        let (sum, file) = line.split_once(' ')?;
        let file = file.strip_prefix([' ', '*']).unwrap_or(file);
        (file == name).then_some(sum)
    });
    let sum = found.next()?;
    if found.next().is_some() || sum.len() != 64 || !sum.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0; 32];
    for (byte, i) in out.iter_mut().zip((0..).step_by(2)) {
        *byte = u8::from_str_radix(sum.get(i..i + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Ставит новое содержимое программы на место прежнего.
fn replace(program: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = file::write_beside(program, bytes)?;
    let result = if cfg!(windows) {
        swap(program, &temp)
    } else {
        fs::rename(&temp, program)
    };
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

const MAX_OLD: usize = 16;

/// Куда отодвинуть прежнюю программу.
fn old_name(program: &Path, number: usize) -> PathBuf {
    let mut name = program.as_os_str().to_owned();
    name.push(format!(".old{number}"));
    name.into()
}

/// Windows не даёт перезаписать работающую программу, но даёт переименовать.
fn swap(program: &Path, temp: &Path) -> io::Result<()> {
    let free = (0..MAX_OLD).map(|n| old_name(program, n));
    let Some(old) = free.into_iter().find(|p| {
        let _ = fs::remove_file(p);
        !p.exists()
    }) else {
        return Err(io::Error::other("previous versions are still running"));
    };
    fs::rename(program, &old)?;
    fs::rename(temp, program).inspect_err(|_| {
        // Без программы пользователя не оставляем.
        let _ = fs::rename(&old, program);
    })
}

/// Убирает программы, отодвинутые при прошлых обновлениях.
pub fn remove_old() {
    if !cfg!(windows) {
        return;
    }
    let Ok(program) = std::env::current_exe() else {
        return;
    };
    for number in 0..MAX_OLD {
        // Прежняя версия может ещё работать, тогда в другой раз.
        let _ = fs::remove_file(old_name(&program, number));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    const PLATFORM: (&str, &[u8]) = ("linux-x86_64.bin", b"\x7fELF");
    const PROGRAM: &[u8] = b"\x7fELF new program";

    /// Вместо GitHub.
    #[derive(Default)]
    struct Fake {
        pages: HashMap<String, Vec<u8>>,
        asked: RefCell<Vec<String>>,
    }

    impl Source for Fake {
        fn get(&self, url: &str, limit: u64, _: Duration) -> Result<Vec<u8>, String> {
            self.asked.borrow_mut().push(url.to_owned());
            let page = self.pages.get(url).ok_or("404")?;
            if page.len() as u64 > limit {
                return Err("too big".into());
            }
            Ok(page.clone())
        }
    }

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn release(tag: &str, prerelease: bool) -> String {
        let name = format!("ruconmd-{tag}-linux-x86_64.bin");
        format!(
            r#"{{"tag_name":"{tag}","draft":false,"prerelease":{prerelease},
            "assets":[{{"name":"{name}"}},{{"name":"SHA256SUMS"}}]}}"#
        )
    }

    fn list(releases: &[String]) -> Vec<u8> {
        format!("[{}]", releases.join(",")).into_bytes()
    }

    fn hex(bytes: &[u8]) -> String {
        let sum = Sha256::digest(bytes);
        sum.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn url(name: &str) -> String {
        format!("{DOWNLOADS}/v0.2.0/{name}")
    }

    const NAME: &str = "ruconmd-v0.2.0-linux-x86_64.bin";

    /// Выпуск 0.2.0 с файлом и его суммой.
    fn github() -> Fake {
        let sums = format!("{}  other\n{}  {NAME}\n", hex(b"x"), hex(PROGRAM));
        let mut fake = Fake::default();
        let releases = list(&[release("v0.2.0", false)]);
        fake.pages.insert(RELEASES.into(), releases);
        fake.pages.insert(url(SUMS), sums.into());
        fake.pages.insert(url(NAME), PROGRAM.into());
        fake
    }

    /// Программа в своём каталоге.
    struct Installed {
        dir: tempfile::TempDir,
        program: PathBuf,
    }

    impl Installed {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let program = dir.path().join("ruconmd");
            fs::write(&program, "old program").unwrap();
            Self { dir, program }
        }

        fn update(&self, source: &Fake, current: &str) -> (Result<(), Failure>, String) {
            let mut out = Vec::new();
            let current = v(current);
            let result = update(
                source,
                &current,
                PLATFORM,
                &self.program,
                Lang::En,
                &mut out,
            );
            (result, String::from_utf8(out).unwrap())
        }

        fn files(&self) -> usize {
            fs::read_dir(self.dir.path()).unwrap().count()
        }
    }

    #[test]
    fn verified_program_replaces_the_old_one() {
        let (installed, source) = (Installed::new(), github());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&installed.program, fs::Permissions::from_mode(0o750)).unwrap();
        }
        let (result, out) = installed.update(&source, "0.1.0");
        assert_eq!(result, Ok(()));
        assert!(out.contains("0.1.0 -> 0.2.0"), "{out}");
        assert_eq!(fs::read(&installed.program).unwrap(), PROGRAM);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = fs::metadata(&installed.program).unwrap();
            assert_eq!(meta.mode() & 0o777, 0o750);
            assert_eq!(installed.files(), 1);
        }
        let asked = source.asked.borrow();
        assert_eq!(*asked, [RELEASES.to_owned(), url(SUMS), url(NAME)]);
        assert!(asked.iter().all(|u| u.starts_with("https://")));
    }

    #[test]
    fn nothing_is_downloaded_when_there_is_nothing_new() {
        let (installed, source) = (Installed::new(), github());
        let (result, out) = installed.update(&source, "0.2.0");
        assert_eq!(result, Ok(()));
        assert!(out.contains("0.2.0"));
        assert_eq!(source.asked.borrow().len(), 1);
        assert_eq!(fs::read(&installed.program).unwrap(), b"old program");
    }

    #[test]
    fn damaged_download_changes_nothing() {
        let html = b"<html>not a program".to_vec();
        let of_html = format!("{}  {NAME}\n", hex(&html));
        let mut cases = std::array::from_fn::<_, 6, _>(|_| github());
        cases[0].pages.insert(url(NAME), b"\x7fELF other".to_vec());
        cases[1].pages.insert(url(SUMS), Vec::new());
        // Сумма верна, но это не программа.
        cases[2].pages.insert(url(NAME), html);
        cases[2].pages.insert(url(SUMS), of_html.into());
        cases[3].pages.remove(&url(NAME));
        cases[4].pages.remove(RELEASES);
        cases[5].pages.insert(RELEASES.into(), b"{}".to_vec());
        for (number, source) in cases.iter().enumerate() {
            let installed = Installed::new();
            let expected = match number {
                0..3 => Failure::Checksum,
                5 => Failure::BadList,
                _ => Failure::Network("404".into()),
            };
            assert_eq!(installed.update(source, "0.1.0").0, Err(expected));
            assert_eq!(fs::read(&installed.program).unwrap(), b"old program");
            assert_eq!(installed.files(), 1);
        }
    }

    #[test]
    fn nothing_is_downloaded_when_it_cannot_be_installed() {
        let (installed, source) = (Installed::new(), github());
        fs::remove_file(&installed.program).unwrap();
        let result = installed.update(&source, "0.1.0").0;
        assert!(matches!(result, Err(Failure::Replace(_))));
        assert_eq!(source.asked.borrow().len(), 1);
        assert_eq!(installed.files(), 0);
    }

    #[test]
    fn network_errors_name_the_server_and_the_proxy() {
        let url = "https://api.github.com/repos/x?per_page=30";
        assert_eq!(explain(url, None, "oops"), "api.github.com: oops");
        assert_eq!(
            explain(url, Some(("10.0.0.1", 3128)), "oops"),
            "api.github.com (proxy 10.0.0.1:3128): oops"
        );
    }

    #[test]
    fn newest_suitable_release_is_chosen() {
        let all = list(&[
            release("v0.1.0", false),
            release("v0.3.0-beta.10", true),
            release("v0.3.0-beta.9", true),
            release("v0.2.0", false),
            release("v0.2.5", true),
        ]);
        let pick = |current| {
            let found = newest(&all, &v(current), PLATFORM.0).unwrap();
            found.map(|v| v.to_string())
        };
        assert_eq!(pick("0.1.0").as_deref(), Some("0.2.0"));
        assert_eq!(pick("0.1.0-beta.1").as_deref(), Some("0.3.0-beta.10"));
        assert_eq!(pick("0.3.0-beta.9").as_deref(), Some("0.3.0-beta.10"));
        assert_eq!(pick("0.3.0-beta.10"), None);
        assert_eq!(pick("0.2.0"), None);
        assert_eq!(pick("9.0.0-beta.1"), None);
        assert_eq!(newest(&all, &v("0.1.0"), "windows-x86_64.exe"), Ok(None));
    }

    #[test]
    fn strange_releases_are_ignored() {
        let good = release("v0.2.0", false);
        for bad in [
            good.replace("\"draft\":false", "\"draft\":true"),
            good.replace("SHA256SUMS", "other"),
            good.replace("\"v0.2.0\"", "\"0.2.0\""),
            good.replace("\"v0.2.0\"", "\"v0.2.0/../x\""),
            good.replace("v0.2.0", "v0.2.0+build"),
            good.replace("\"assets\"", "\"files\""),
            "null".into(),
        ] {
            assert_eq!(newest(&list(&[bad]), &v("0.1.0"), PLATFORM.0), Ok(None));
        }
        for bad in ["", "{}", "[", "{\"message\":\"rate limit\"}"] {
            let got = newest(bad.as_bytes(), &v("0.1.0"), PLATFORM.0);
            assert_eq!(got, Err(Failure::BadList));
        }
    }

    #[test]
    fn checksums_are_read_strictly() {
        let sum = hex(PROGRAM);
        let expected: [u8; 32] = Sha256::digest(PROGRAM).into();
        let read = |text: &str| checksum(text.as_bytes(), "a.bin");
        assert_eq!(read(&format!("{sum}  a.bin\n")), Some(expected));
        assert_eq!(read(&format!("{sum} *a.bin\r\n")), Some(expected));
        for bad in [
            format!("{sum}  b.bin"),
            format!("{sum}  a.bin.sig"),
            format!("{sum}  a.bin\n{sum}  a.bin"),
            format!("{}  a.bin", &sum[1..]),
            format!("{}  a.bin", "я".repeat(32)),
            format!("+{}  a.bin", &sum[1..]),
            "a.bin".into(),
        ] {
            assert_eq!(read(&bad), None, "{bad}");
        }
    }

    #[test]
    fn swap_moves_the_old_program_aside_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let (program, temp) = (dir.path().join("p.exe"), dir.path().join("new"));
        fs::write(&program, "one").unwrap();
        fs::write(&temp, "two").unwrap();
        // Занятое имя: каталог не удалить как файл.
        fs::create_dir(old_name(&program, 0)).unwrap();
        swap(&program, &temp).unwrap();
        assert_eq!(fs::read_to_string(&program).unwrap(), "two");
        assert_eq!(fs::read_to_string(old_name(&program, 1)).unwrap(), "one");
        assert!(swap(&program, &temp).is_err(), "nothing to put in place");
        assert_eq!(fs::read_to_string(&program).unwrap(), "two");
    }

    #[test]
    fn own_version_is_understood() {
        assert!(Version::parse(env!("CARGO_PKG_VERSION")).is_ok());
    }
}
