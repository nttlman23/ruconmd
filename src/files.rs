//! Дерево файлов для панели: чтение каталогов, раскрытие, выбор. Терминала здесь нет.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Столько записей каталога читается и столько показывается.
const MAX_SCAN: usize = 50_000;
const MAX_SHOWN: usize = 5_000;
const MARKDOWN: [&str; 4] = ["md", "markdown", "mdown", "mkd"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Каталог выше корня.
    Up,
    Dir {
        open: bool,
    },
    File,
    /// Каталог прочитать нельзя.
    Unreadable,
    /// Показаны не все файлы.
    Cut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub kind: Kind,
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    path: PathBuf,
    dir: bool,
}

#[derive(Debug)]
struct Listing {
    /// Время каталога до чтения: по нему видно, что пора перечитать.
    modified: Option<SystemTime>,
    entries: Vec<Entry>,
    readable: bool,
    cut: bool,
}

fn modified(dir: &Path) -> Option<SystemTime> {
    fs::metadata(dir).and_then(|m| m.modified()).ok()
}

fn extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_string_lossy().to_lowercase())
}

/// Markdown ли это, судя по имени файла.
pub fn is_markdown(path: &Path) -> bool {
    extension(path).is_some_and(|e| MARKDOWN.contains(&e.as_str()))
}

fn is_text(path: &Path) -> bool {
    is_markdown(path) || extension(path).is_some_and(|e| e == "txt")
}

#[cfg(windows)]
fn is_hidden(entry: &fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 2;
    entry
        .metadata()
        .is_ok_and(|m| m.file_attributes() & HIDDEN != 0)
}

#[cfg(not(windows))]
fn is_hidden(_: &fs::DirEntry) -> bool {
    false
}

impl Listing {
    fn read(dir: &Path, all: bool) -> Self {
        let mut listing = Self {
            modified: modified(dir),
            entries: Vec::new(),
            readable: true,
            cut: false,
        };
        let Ok(found) = fs::read_dir(dir) else {
            listing.readable = false;
            return listing;
        };
        for (number, entry) in found.flatten().enumerate() {
            if number == MAX_SCAN {
                listing.cut = true;
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !all && (name.starts_with('.') || is_hidden(&entry)) {
                continue;
            }
            let path = entry.path();
            let Ok(mut kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                // Битые ссылки не показываются.
                let Ok(meta) = fs::metadata(&path) else {
                    continue;
                };
                kind = meta.file_type();
            }
            let dir = kind.is_dir();
            // Каналы и устройства не открываются.
            if dir || (kind.is_file() && (all || is_text(&path))) {
                listing.entries.push(Entry { name, path, dir });
            }
        }
        listing
            .entries
            .sort_by_cached_key(|e| (!e.dir, e.name.to_lowercase(), e.name.clone()));
        if listing.entries.len() > MAX_SHOWN {
            listing.entries.truncate(MAX_SHOWN);
            listing.cut = true;
        }
        listing
    }
}

#[derive(Debug)]
pub struct Tree {
    root: PathBuf,
    /// Показывать скрытое и файлы любых типов.
    all: bool,
    open: HashSet<PathBuf>,
    listings: HashMap<PathBuf, Listing>,
    rows: Vec<Row>,
    selected: usize,
    top: usize,
    height: usize,
}

impl Tree {
    pub fn new(root: &Path, all: bool) -> Self {
        let mut tree = Self {
            // Без этого у `.` не было бы каталога выше.
            root: std::path::absolute(root).unwrap_or_else(|_| root.to_owned()),
            all,
            open: HashSet::new(),
            listings: HashMap::new(),
            rows: Vec::new(),
            selected: 0,
            top: 0,
            height: 1,
        };
        tree.rebuild();
        tree.select_first();
        tree
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn set_all(&mut self, all: bool) {
        if self.all != all {
            self.all = all;
            self.reread();
        }
    }

    // --- Построение ---

    fn rebuild(&mut self) {
        let before = self.rows.get(self.selected).map(|r| r.path.clone());
        let mut rows = Vec::new();
        if let Some(parent) = self.root.parent() {
            rows.push(Row {
                path: parent.to_owned(),
                name: "..".into(),
                depth: 0,
                kind: Kind::Up,
            });
        }
        let root = self.root.clone();
        self.add(&root, 0, &mut rows);
        self.rows = rows;
        // Выбранным остаётся тот же файл, а если его нет, то же место.
        let same = before.and_then(|path| self.rows.iter().position(|r| r.path == path));
        self.select(same.unwrap_or(self.selected));
    }

    fn add(&mut self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        let all = self.all;
        let listing = self
            .listings
            .entry(dir.to_owned())
            .or_insert_with(|| Listing::read(dir, all));
        let (readable, cut) = (listing.readable, listing.cut);
        let found: Vec<Row> = listing
            .entries
            .iter()
            .map(|e| Row {
                path: e.path.clone(),
                name: e.name.clone(),
                depth,
                kind: match e.dir {
                    true => Kind::Dir {
                        open: self.open.contains(&e.path),
                    },
                    false => Kind::File,
                },
            })
            .collect();
        for row in found {
            let inside = (row.kind == Kind::Dir { open: true }).then(|| row.path.clone());
            rows.push(row);
            if let Some(path) = inside {
                self.add(&path, depth + 1, rows);
            }
        }
        for (shown, kind) in [(!readable, Kind::Unreadable), (cut, Kind::Cut)] {
            if shown {
                rows.push(Row {
                    path: PathBuf::new(),
                    name: String::new(),
                    depth,
                    kind,
                });
            }
        }
    }

    /// Перечитывает всё, что раскрыто.
    pub fn reread(&mut self) {
        self.listings.clear();
        self.rebuild();
    }

    /// Перечитывает каталоги, которые изменились.
    pub fn refresh(&mut self) {
        let before = self.listings.len();
        self.listings
            .retain(|dir, listing| modified(dir) == listing.modified);
        if self.listings.len() != before {
            self.rebuild();
        }
    }

    // --- Выбор ---

    fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
        self.follow();
    }

    /// Первое, что есть в самом каталоге, а не `..`.
    fn select_first(&mut self) {
        self.select(usize::from(
            self.rows.len() > 1 && self.rows[0].kind == Kind::Up,
        ));
    }

    fn follow(&mut self) {
        let lowest = (self.selected + 1).saturating_sub(self.height);
        self.top = self.top.clamp(lowest, self.selected);
        self.clamp();
    }

    fn clamp(&mut self) {
        self.top = self.top.min(self.rows.len().saturating_sub(self.height));
    }

    pub fn set_height(&mut self, height: usize) {
        if self.height != height.max(1) {
            self.height = height.max(1);
            self.follow();
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        self.select(self.selected.saturating_add_signed(delta));
    }

    pub fn page(&self) -> isize {
        self.height.saturating_sub(1).max(1) as isize
    }

    pub fn scroll(&mut self, delta: isize) {
        self.top = self.top.saturating_add_signed(delta);
        self.clamp();
    }

    /// Строки на экране и признак выбранной.
    pub fn visible(&self) -> impl Iterator<Item = (&Row, bool)> {
        let rows = self.rows.iter().enumerate().skip(self.top);
        rows.take(self.height)
            .map(|(index, row)| (row, index == self.selected))
    }

    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    /// Выбирает строку экрана. `false`: там пусто.
    pub fn click(&mut self, y: usize) -> bool {
        let index = self.top + y;
        if index >= self.rows.len() || y >= self.height {
            return false;
        }
        self.selected = index;
        true
    }

    /// Следующее имя на эту букву, по кругу.
    pub fn jump(&mut self, letter: char) {
        let wanted: String = letter.to_lowercase().collect();
        let count = self.rows.len();
        let next = (1..=count).map(|step| (self.selected + step) % count);
        let found = next
            .into_iter()
            .find(|i| self.rows[*i].name.to_lowercase().starts_with(&wanted));
        if let Some(index) = found {
            self.select(index);
        }
    }

    /// Раскрывает или сворачивает каталог. Возвращает выбранный файл.
    pub fn activate(&mut self) -> Option<PathBuf> {
        let row = self.selected()?.clone();
        match row.kind {
            Kind::File => return Some(row.path),
            Kind::Up => self.go_up(),
            Kind::Dir { open } => self.set_open(&row.path, !open),
            Kind::Unreadable | Kind::Cut => {}
        }
        None
    }

    fn set_open(&mut self, dir: &Path, open: bool) {
        if open {
            self.open.insert(dir.to_owned());
        } else {
            self.open.remove(dir);
        }
        // Раскрытое вновь перечитывается.
        self.listings.remove(dir);
        self.rebuild();
    }

    /// Раскрывает каталог, а в раскрытом переходит внутрь.
    pub fn right(&mut self) {
        match self.selected().map(|r| (r.kind, r.path.clone())) {
            Some((Kind::Dir { open: false }, path)) => self.set_open(&path, true),
            Some((Kind::Dir { open: true }, _)) => {
                let depth = self.rows[self.selected].depth;
                let inside = self.rows.get(self.selected + 1);
                if inside.is_some_and(|r| r.depth > depth) {
                    self.move_by(1);
                }
            }
            _ => {}
        }
    }

    /// Сворачивает каталог, а из файла переходит к его каталогу.
    pub fn left(&mut self) {
        let Some(row) = self.selected().cloned() else {
            return;
        };
        if row.kind == (Kind::Dir { open: true }) {
            return self.set_open(&row.path, false);
        }
        let above = self.rows[..self.selected].iter();
        if let Some(index) = above
            .rev()
            .position(|r| r.depth < row.depth)
            .map(|back| self.selected - 1 - back)
        {
            self.select(index);
        }
    }

    /// Делает корнем каталог выше. Прежний корень остаётся раскрытым и выбранным.
    pub fn go_up(&mut self) {
        let Some(parent) = self.root.parent().map(Path::to_owned) else {
            return;
        };
        let old = std::mem::replace(&mut self.root, parent);
        self.open.insert(old.clone());
        self.rebuild();
        self.select_path(&old);
    }

    fn select_path(&mut self, path: &Path) -> bool {
        let index = self.rows.iter().position(|r| r.path == path);
        index.map(|i| self.select(i)).is_some()
    }

    /// Показывает файл, раскрывая каталоги до него, если он внутри корня.
    pub fn reveal(&mut self, file: &Path) {
        let Ok(inside) = file.strip_prefix(&self.root) else {
            return;
        };
        let mut dir = self.root.clone();
        let parents = inside.parent().map(|p| p.components().collect::<Vec<_>>());
        for part in parents.unwrap_or_default() {
            dir.push(part);
            self.open.insert(dir.clone());
        }
        self.rebuild();
        self.select_path(file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in ["b.md", "A.MD", "я.txt", "run.sh", ".hidden.md", "pic.png"] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        fs::create_dir_all(dir.path().join("Заметки/старые")).unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("Заметки/в.md"), "").unwrap();
        fs::write(dir.path().join("Заметки/старые/г.md"), "").unwrap();
        dir
    }

    fn tree(dir: &tempfile::TempDir) -> Tree {
        let mut tree = Tree::new(dir.path(), false);
        tree.set_height(50);
        tree
    }

    fn names(tree: &Tree) -> Vec<String> {
        tree.rows
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
            .collect()
    }

    fn chosen(tree: &Tree) -> &str {
        &tree.selected().unwrap().name
    }

    #[test]
    fn directories_first_and_only_documents() {
        let dir = sample();
        let mut t = tree(&dir);
        assert_eq!(names(&t), ["..", "Заметки", "A.MD", "b.md", "я.txt"]);
        assert_eq!(chosen(&t), "Заметки");
        t.set_all(true);
        assert_eq!(
            names(&t),
            [
                "..",
                ".git",
                "Заметки",
                ".hidden.md",
                "A.MD",
                "b.md",
                "pic.png",
                "run.sh",
                "я.txt"
            ]
        );
        assert_eq!(chosen(&t), "Заметки", "the same row stays selected");
    }

    #[test]
    fn opening_and_closing() {
        let dir = sample();
        let mut t = tree(&dir);
        assert_eq!(t.activate(), None);
        assert_eq!(
            names(&t)[..5],
            ["..", "Заметки", "  старые", "  в.md", "A.MD"]
        );
        t.right();
        assert_eq!(chosen(&t), "старые");
        t.right();
        t.right();
        assert_eq!(chosen(&t), "г.md");
        let expected = dir.path().join("Заметки").join("старые").join("г.md");
        assert_eq!(t.activate(), Some(expected));
        t.right();
        assert_eq!(chosen(&t), "г.md");
        t.left();
        assert_eq!(chosen(&t), "старые");
        t.left();
        assert_eq!(chosen(&t), "старые");
        t.left();
        assert_eq!(chosen(&t), "Заметки");
        t.left();
        assert_eq!(names(&t).len(), 5);
        // Раскрытое внутри помнится.
        t.right();
        assert!(!names(&t).contains(&"    г.md".to_owned()));
    }

    #[test]
    fn empty_directory_opens_without_moving() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("пусто")).unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        let mut t = tree(&dir);
        t.right();
        t.right();
        assert_eq!(chosen(&t), "пусто");
    }

    #[test]
    fn going_up_keeps_the_old_root_in_view() {
        let dir = sample();
        let mut t = Tree::new(&dir.path().join("Заметки"), false);
        t.set_height(50);
        assert_eq!(names(&t), ["..", "старые", "в.md"]);
        t.go_up();
        assert_eq!(t.root(), dir.path());
        assert_eq!(chosen(&t), "Заметки");
        assert!(names(&t).contains(&"  в.md".to_owned()));
        // У корня файловой системы выше ничего нет.
        for _ in 0..50 {
            t.go_up();
        }
        assert!(t.root().parent().is_none());
        assert!(t.rows.iter().all(|r| r.kind != Kind::Up));
    }

    #[test]
    fn changes_on_disk_are_seen() {
        let dir = sample();
        let mut t = tree(&dir);
        t.jump('b');
        assert_eq!(chosen(&t), "b.md");
        // Время каталога должно заметно измениться.
        std::thread::sleep(std::time::Duration::from_millis(30));
        fs::write(dir.path().join("a0.md"), "").unwrap();
        t.refresh();
        assert!(names(&t).contains(&"a0.md".to_owned()));
        assert_eq!(chosen(&t), "b.md");
        fs::remove_file(dir.path().join("b.md")).unwrap();
        t.reread();
        assert_eq!(chosen(&t), "я.txt", "the same place");
        fs::remove_dir_all(dir.path().join("Заметки")).unwrap();
        fs::remove_file(dir.path().join("я.txt")).unwrap();
        t.reread();
        assert_eq!(chosen(&t), "a0.md");
    }

    #[test]
    fn unreadable_directory_is_marked() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = Tree::new(&dir.path().join("none"), false);
        assert_eq!(t.rows.last().unwrap().kind, Kind::Unreadable);
        t.move_by(5);
        assert_eq!(t.activate(), None);
        t.left();
        t.right();
        t.jump('x');
    }

    #[test]
    fn too_many_files_are_cut() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..MAX_SHOWN + 5 {
            fs::write(dir.path().join(format!("{i:05}.md")), "").unwrap();
        }
        let t = tree(&dir);
        assert_eq!(t.rows.len(), MAX_SHOWN + 2);
        assert_eq!(t.rows.last().unwrap().kind, Kind::Cut);
    }

    #[test]
    fn markdown_is_told_by_the_name() {
        for name in ["a.md", "A.MD", "дир.txt/б.markdown", "c.mdown", "d.mkd"] {
            assert!(is_markdown(Path::new(name)), "{name}");
        }
        for name in [
            "a.txt", "README", ".md", "a.md.bak", "a.mdx", "md", "a.toml",
        ] {
            assert!(!is_markdown(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn letters_jump_in_a_circle() {
        let dir = sample();
        let mut t = tree(&dir);
        t.jump('Я');
        assert_eq!(chosen(&t), "я.txt");
        t.jump('a');
        assert_eq!(chosen(&t), "A.MD");
        t.jump('ъ');
        assert_eq!(chosen(&t), "A.MD");
    }

    #[test]
    fn scrolling_follows_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..30 {
            fs::write(dir.path().join(format!("{i:02}.md")), "").unwrap();
        }
        let mut t = tree(&dir);
        t.set_height(10);
        t.move_by(15);
        assert!(t.visible().any(|(_, selected)| selected));
        t.move_by(isize::MAX);
        assert_eq!(t.visible().last().unwrap().0.name, "29.md");
        t.scroll(-100);
        assert_eq!(t.visible().next().unwrap().0.name, "..");
        assert!(t.click(2));
        assert_eq!(chosen(&t), "01.md");
        assert!(!t.click(10));
        t.move_by(isize::MIN);
        assert_eq!(chosen(&t), "..");
        t.set_height(1);
        assert_eq!(t.visible().count(), 1);
    }

    #[test]
    fn reveal_opens_the_way_to_a_file() {
        let dir = sample();
        let mut t = tree(&dir);
        t.reveal(&dir.path().join("Заметки").join("старые").join("г.md"));
        assert_eq!(chosen(&t), "г.md");
        t.reveal(Path::new("/elsewhere/x.md"));
        assert_eq!(chosen(&t), "г.md");
    }

    #[cfg(unix)]
    #[test]
    fn links_and_special_files() {
        use std::os::unix::fs::symlink;
        let dir = sample();
        let path = |n: &str| dir.path().join(n);
        symlink(path("Заметки"), path("link")).unwrap();
        symlink(path("none"), path("broken.md")).unwrap();
        symlink("/dev/null", path("device.md")).unwrap();
        symlink(dir.path(), path("loop")).unwrap();
        let mut t = tree(&dir);
        assert_eq!(
            names(&t),
            ["..", "link", "loop", "Заметки", "A.MD", "b.md", "я.txt"]
        );
        t.jump('l');
        t.activate();
        t.activate();
        assert_eq!(names(&t).len(), 7);
    }
}
