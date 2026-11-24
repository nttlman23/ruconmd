//! Куда ведёт ссылка и чем её открыть.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Раздел текущего документа.
    Anchor(String),
    /// Адрес для системного приложения.
    Web(String),
    /// Файл Markdown и раздел в нём.
    File(PathBuf, Option<String>),
    /// Такие ссылки не открываются.
    Unsupported,
}

/// Разбирает ссылку из документа. Относительные пути считаются от его каталога.
pub fn parse(url: &str, document: Option<&Path>) -> Target {
    let url = url.trim();
    if url.is_empty() || url.contains(char::is_control) {
        return Target::Unsupported;
    }
    if let Some(anchor) = url.strip_prefix('#') {
        return Target::Anchor(decode(anchor));
    }
    if let Some(scheme) = scheme(url) {
        return match scheme.as_str() {
            "http" | "https" | "mailto" => Target::Web(url.to_owned()),
            _ => Target::Unsupported,
        };
    }
    // Сетевой путь заставил бы Windows обратиться к чужому серверу.
    if url.starts_with("//") || url.starts_with("\\\\") {
        return Target::Unsupported;
    }
    let (path, anchor) = match url.split_once('#') {
        Some((path, anchor)) => (path, Some(decode(anchor))),
        None => (url, None),
    };
    let path = decode(path);
    if path.starts_with("//") || path.contains("\\\\") || path.contains(char::is_control) {
        return Target::Unsupported;
    }
    let path = PathBuf::from(path);
    let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    if !matches!(extension.as_deref(), Some("md" | "markdown")) {
        return Target::Unsupported;
    }
    let base = document.and_then(Path::parent).unwrap_or(Path::new(""));
    Target::File(base.join(path), anchor)
}

/// Схема адреса. Одна буква перед двоеточием: диск Windows, а не схема.
fn scheme(url: &str) -> Option<String> {
    let (scheme, _) = url.split_once(':')?;
    let valid = scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    valid.then(|| scheme.to_ascii_lowercase())
}

/// Раскрывает `%XX`. Если получается не UTF-8, текст остаётся как был.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = text.get(i + 1..i + 3).filter(|_| bytes[i] == b'%');
        match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

/// Имя раздела для ссылок, по правилам GitHub.
pub fn slug(heading: &str) -> String {
    heading
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

pub trait Opener {
    fn open(&mut self, url: &str) -> io::Result<()>;
}

/// Открывает адрес программой, назначенной в системе. Оболочка не участвует.
pub struct System;

impl Opener for System {
    fn open(&mut self, url: &str) -> io::Result<()> {
        let mut command = if cfg!(windows) {
            let mut c = Command::new("rundll32.exe");
            c.arg("url.dll,FileProtocolHandler");
            c
        } else {
            Command::new("xdg-open")
        };
        command
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn()?;
        // Завершившийся процесс надо дождаться, иначе он останется зомби.
        std::thread::spawn(move || child.wait());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(url: &str) -> Target {
        parse(url, Some(Path::new("notes/a.md")))
    }

    #[test]
    fn web_and_anchors() {
        for url in ["http://a.ru", "HTTPS://a.ru/x#y", "mailto:a@b.ru"] {
            assert_eq!(parsed(url), Target::Web(url.into()));
        }
        assert_eq!(parsed("#глава"), Target::Anchor("глава".into()));
        assert_eq!(parsed("#%D0%B0"), Target::Anchor("а".into()));
    }

    #[test]
    fn markdown_files_are_relative_to_the_document() {
        let file = |p: &str| Target::File(p.into(), None);
        assert_eq!(parsed("b.md"), file("notes/b.md"));
        assert_eq!(parsed("../b%20c.MD"), file("notes/../b c.MD"));
        assert_eq!(parsed("/x/b.markdown"), file("/x/b.markdown"));
        assert_eq!(
            parsed("b.md#раздел"),
            Target::File("notes/b.md".into(), Some("раздел".into()))
        );
        assert_eq!(parse("b.md", None), file("b.md"));
    }

    #[test]
    fn dangerous_links_are_refused() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ssh://host",
            "ftp://a.ru",
            "run.exe",
            "script.sh",
            "",
            "//server/share/a.md",
            "\\\\server\\share\\a.md",
            "%5C%5Cserver%5Ca.md",
            "/%2Fserver/a.md",
            "http://a.ru/\u{1b}[31m",
            "a%00.md",
            "vbscript:x.md",
        ] {
            assert_eq!(parsed(url), Target::Unsupported, "{url}");
        }
    }

    #[test]
    fn bad_escapes_are_kept() {
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%zz%ff"), "%zz%ff");
        assert_eq!(decode("%d"), "%d");
        assert_eq!(decode("%я1"), "%я1");
    }

    #[test]
    fn slugs_follow_github() {
        assert_eq!(slug("Привет, Мир!"), "привет-мир");
        assert_eq!(slug("a_b - c"), "a_b---c");
        assert_eq!(slug("1. Раздел `код`"), "1-раздел-код");
    }
}
