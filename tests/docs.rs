//! Ссылки в документации ведут на существующие файлы и разделы.

use std::fs;
use std::path::{Path, PathBuf};

use ruconmd::links::{self, Target};
use ruconmd::preview::{self, Doc, Item};

fn documents(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() && name != "target" && !name.starts_with('.') {
            documents(&path, out);
        } else if name.ends_with(".md") {
            out.push(path);
        }
    }
}

fn has_section(file: &Path, anchor: &str) -> bool {
    let text = fs::read_to_string(file).unwrap();
    preview::headings(&text).iter().any(|h| h.slug == anchor)
}

#[test]
fn links_in_documentation_are_valid() {
    let mut files = Vec::new();
    documents(Path::new(env!("CARGO_MANIFEST_DIR")), &mut files);
    assert!(files.len() >= 5, "{files:?}");
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for item in Doc::build(&text, 100, 4, None).items {
            let Item::Link(url) = item else { continue };
            let place = format!("{}: {url}", file.display());
            match links::parse(&url, Some(&file)) {
                Target::Anchor(anchor) => assert!(has_section(&file, &anchor), "{place}"),
                Target::File(path, anchor) => {
                    assert!(path.is_file(), "{place}");
                    if let Some(anchor) = anchor {
                        assert!(has_section(&path, &anchor), "{place}");
                    }
                }
                Target::Web(_) => {}
                // Не Markdown: файл всё равно должен быть. У образца есть и
                // нарочно запрещённая ссылка.
                Target::Unsupported if url.starts_with("file:") => {}
                Target::Unsupported => {
                    let path = links::local_path(&url, Some(&file));
                    assert!(path.is_some_and(|p| p.exists()), "{place}");
                }
            }
        }
    }
}
