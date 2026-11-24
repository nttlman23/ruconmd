//! Картинки: умеет ли терминал их показывать, загрузка и кодирование.

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use ratatui::layout::Size;
use ratatui_image::FontSize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::sliced::SlicedProtocol;

/// Выше этого картинка уменьшается, чтобы помещаться на обычный экран.
const MAX_ROWS: u32 = 30;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_SIDE: u32 = 16_384;
const MAX_MEMORY: u64 = 512 * 1024 * 1024;

/// Параметр `preview.images`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    Off,
    Auto,
    Force(ProtocolType),
}

impl Wanted {
    /// Имена протоколов в конфиге.
    pub const NAMES: [&'static str; 3] = ["kitty", "sixel", "iterm2"];

    /// Имя выбранного протокола.
    pub fn name(self) -> Option<&'static str> {
        Self::NAMES
            .into_iter()
            .find(|name| Self::from_name(name) == Some(self))
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "kitty" => Some(Self::Force(ProtocolType::Kitty)),
            "sixel" => Some(Self::Force(ProtocolType::Sixel)),
            "iterm2" => Some(Self::Force(ProtocolType::Iterm2)),
            _ => None,
        }
    }
}

/// Способ показа картинок в этом терминале.
#[derive(Clone)]
pub struct Graphics {
    picker: Picker,
}

impl Graphics {
    pub fn new(protocol: ProtocolType, cell_width: u16, cell_height: u16) -> Self {
        // Другого способа задать размер ячейки библиотека не даёт: её собственный
        // опрос терминала может оставить поток, читающий ввод.
        #[expect(deprecated)]
        let mut picker = Picker::from_fontsize(FontSize::new(cell_width, cell_height));
        picker.set_protocol_type(protocol);
        Self { picker }
    }

    pub fn protocol(&self) -> ProtocolType {
        self.picker.protocol_type()
    }

    /// Остаются ли на экране следы картинки, если поверх неё ничего не нарисовать.
    pub fn leaves_traces(&self) -> bool {
        self.protocol() != ProtocolType::Kitty
    }

    /// Размер картинки в ячейках. Маленькие не растягиваются.
    pub fn cells(&self, (width, height): (u32, u32), available: usize) -> (u16, u16) {
        let font = self.picker.font_size();
        let (cell_w, cell_h) = (u32::from(font.width).max(1), u32::from(font.height).max(1));
        let (width, height) = (f64::from(width.max(1)), f64::from(height.max(1)));
        let room_w = f64::from(u32::try_from(available.max(1)).unwrap_or(1) * cell_w);
        let room_h = f64::from(MAX_ROWS * cell_h);
        let scale = (room_w / width).min(room_h / height).min(1.0);
        let in_cells =
            |pixels: f64, cell: u32| ((pixels * scale) / f64::from(cell)).ceil().max(1.0);
        let columns = in_cells(width, cell_w).min(available.max(1) as f64);
        let rows = in_cells(height, cell_h).min(f64::from(MAX_ROWS));
        (columns as u16, rows as u16)
    }

    /// Загружает и кодирует картинку. Долго, вызывается в отдельном потоке.
    pub fn encode(&self, path: &Path, columns: u16, rows: u16) -> Option<SlicedProtocol> {
        let image = load(path)?;
        SlicedProtocol::new(&self.picker, image, Some(Size::new(columns, rows))).ok()
    }
}

/// Открывает файл, если это разрешённая картинка разумного размера.
fn open(path: &Path) -> Option<ImageReader<BufReader<File>>> {
    // Каналы и устройства не читаются.
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    let mut file = BufReader::new(File::open(path).ok()?);
    let mut head = [0; 32];
    let read = file.by_ref().take(32).read(&mut head).ok()?;
    let format = image::guess_format(&head[..read]).ok()?;
    let allowed = [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Gif,
        ImageFormat::WebP,
    ];
    if !allowed.contains(&format) {
        return None;
    }
    file.rewind().ok()?;
    let mut reader = ImageReader::with_format(file, format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_MEMORY);
    reader.limits(limits);
    Some(reader)
}

/// Размер картинки в точках с учётом поворота, без чтения содержимого.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    let mut decoder = open(path)?.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let turned = decoder.orientation().is_ok_and(|o| {
        use image::metadata::Orientation::*;
        matches!(o, Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH)
    });
    Some(if turned {
        (height, width)
    } else {
        (width, height)
    })
}

fn load(path: &Path) -> Option<DynamicImage> {
    let mut decoder = open(path)?.into_decoder().ok()?;
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    Some(image)
}

/// Картинки документа: где лежат и как показываются.
#[derive(Clone)]
pub struct Images {
    pub graphics: Graphics,
    pub document: Option<PathBuf>,
}

/// Сведения из окружения, нужные для выбора протокола.
#[derive(Debug, Default, Clone)]
pub struct Env {
    pub term: String,
    pub term_program: String,
    pub in_multiplexer: bool,
    pub konsole: bool,
    pub wezterm: bool,
}

impl Env {
    pub fn from_system() -> Self {
        let var = |name| std::env::var(name).unwrap_or_default();
        let term = var("TERM");
        Self {
            in_multiplexer: !var("TMUX").is_empty()
                || !var("STY").is_empty()
                || term.starts_with("tmux")
                || term.starts_with("screen")
                || var("TERM_PROGRAM") == "tmux",
            konsole: !var("KONSOLE_VERSION").is_empty(),
            wezterm: !var("WEZTERM_EXECUTABLE").is_empty(),
            term_program: var("TERM_PROGRAM"),
            term,
        }
    }
}

/// Ответы терминала на вопросы о графике.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Answers {
    pub kitty: bool,
    pub sixel: bool,
    /// Ширина и высота ячейки в точках.
    pub cell: Option<(u16, u16)>,
    /// Терминал ответил на последний вопрос, ждать больше нечего.
    pub complete: bool,
}

/// Вопросы: графика Kitty, размер ячейки и свойства терминала. На последний
/// отвечают все терминалы.
pub const QUESTIONS: &str = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[c";

pub fn parse_answers(input: &str) -> Answers {
    let mut answers = Answers {
        kitty: input.contains("\x1b_Gi=31;OK"),
        ..Answers::default()
    };
    for part in input.split("\x1b[").skip(1) {
        let Some(end) = part.find(|c: char| c.is_ascii_alphabetic()) else {
            continue;
        };
        let (body, kind) = (&part[..end], &part[end..=end]);
        match kind {
            "c" if body.starts_with('?') => {
                answers.complete = true;
                // Первое число: уровень терминала, дальше его возможности.
                answers.sixel = body[1..].split(';').skip(1).any(|n| n == "4");
            }
            "t" => {
                let numbers: Vec<u16> = body.split(';').filter_map(|n| n.parse().ok()).collect();
                if let [6, height, width] = numbers[..]
                    && (1..=200).contains(&width)
                    && (1..=200).contains(&height)
                {
                    answers.cell = Some((width, height));
                }
            }
            _ => {}
        }
    }
    answers
}

/// Имеет ли смысл спрашивать терминал.
pub fn worth_asking(wanted: Wanted, env: &Env) -> bool {
    // Мультиплексор искажает картинки, а обход этого меняет его настройки.
    wanted == Wanted::Auto
        && !env.in_multiplexer
        && !matches!(env.term.as_str(), "" | "dumb" | "linux")
}

pub fn choose(wanted: Wanted, env: &Env, answers: Answers) -> Option<Graphics> {
    if env.in_multiplexer {
        return None;
    }
    let protocol = match wanted {
        Wanted::Off => return None,
        Wanted::Force(protocol) => {
            // Windows Terminal считает ячейку Sixel равной 10 на 20 точек.
            let (width, height) = answers.cell.unwrap_or((10, 20));
            return Some(Graphics::new(protocol, width, height));
        }
        Wanted::Auto => {
            let iterm = ["iTerm", "WezTerm", "mintty", "rio"]
                .iter()
                .any(|name| env.term_program.contains(name));
            // У WezTerm и Konsole неполная поддержка Kitty, у Konsole ещё и Sixel.
            let reliable = !env.konsole && !env.wezterm;
            if answers.kitty && reliable && !iterm {
                ProtocolType::Kitty
            } else if iterm || env.wezterm {
                ProtocolType::Iterm2
            } else if answers.sixel && reliable {
                ProtocolType::Sixel
            } else {
                return None;
            }
        }
    };
    let (width, height) = answers.cell?;
    Some(Graphics::new(protocol, width, height))
}

/// Спрашивает терминал и ждёт ответа не дольше срока. Терминал уже в raw mode,
/// а поток чтения ввода ещё не запущен.
#[cfg(unix)]
pub fn ask() -> Answers {
    use std::io::Write;
    use std::time::{Duration, Instant};

    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use rustix::stdio::{stdin, stdout};

    const LIMIT: Duration = Duration::from_secs(1);

    let mut out = std::io::stdout();
    if out.write_all(QUESTIONS.as_bytes()).is_err() || out.flush().is_err() {
        return Answers::default();
    }
    let deadline = Instant::now() + LIMIT;
    let mut input = Vec::new();
    let mut answers = Answers::default();
    while !answers.complete {
        let left = deadline.saturating_duration_since(Instant::now());
        let timeout = Timespec {
            tv_sec: left.as_secs() as _,
            tv_nsec: left.subsec_nanos() as _,
        };
        let mut fds = [PollFd::from_borrowed_fd(stdin(), PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(n) if n > 0 => {}
            Err(rustix::io::Errno::INTR) if !left.is_zero() => continue,
            _ => break,
        }
        let mut chunk = [0; 256];
        match rustix::io::read(stdin(), &mut chunk) {
            Ok(n) if n > 0 => input.extend_from_slice(&chunk[..n]),
            _ => break,
        }
        answers = parse_answers(&String::from_utf8_lossy(&input));
    }
    answers.cell = answers.cell.or_else(|| {
        let size = rustix::termios::tcgetwinsize(stdout()).ok()?;
        let width = size.ws_xpixel.checked_div(size.ws_col)?;
        let height = size.ws_ypixel.checked_div(size.ws_row)?;
        (width > 0 && height > 0).then_some((width, height))
    });
    answers
}

#[cfg(not(unix))]
pub fn ask() -> Answers {
    Answers::default()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn kitty() -> Graphics {
        Graphics::new(ProtocolType::Kitty, 10, 20)
    }

    pub fn picture(path: &Path, width: u32, height: u32) {
        DynamicImage::new_rgb8(width, height).save(path).unwrap();
    }

    #[test]
    fn answers_are_parsed() {
        let all = "\x1b_Gi=31;OK\x1b\\\x1b[6;20;10t\x1b[?62;4;22c";
        let want = Answers {
            kitty: true,
            sixel: true,
            cell: Some((10, 20)),
            complete: true,
        };
        assert_eq!(parse_answers(all), want);
        let plain = parse_answers("\x1b[?64;1;22c");
        assert!(plain.complete && !plain.sixel && !plain.kitty && plain.cell.is_none());
        // Первое число 4 означает уровень, а не Sixel.
        assert!(!parse_answers("\x1b[?4;6c").sixel);
        for partial in [
            "",
            "\x1b",
            "\x1b[",
            "\x1b[?62;4",
            "\x1b[6;20",
            "\x1b_Gi=31;",
            "я\x1b[я",
        ] {
            assert!(!parse_answers(partial).complete, "{partial:?}");
        }
        assert_eq!(parse_answers("\x1b[6;0;0t").cell, None);
        assert_eq!(parse_answers("\x1b[6;9999;9999t").cell, None);
    }

    #[test]
    fn protocol_choice() {
        let full = parse_answers("\x1b_Gi=31;OK\x1b\\\x1b[6;20;10t\x1b[?62;4c");
        let sixel = Answers {
            kitty: false,
            ..full
        };
        let pick = |wanted, env: &Env, a| choose(wanted, env, a).map(|g| g.protocol());
        let env = Env {
            term: "xterm".into(),
            ..Env::default()
        };
        assert_eq!(pick(Wanted::Auto, &env, full), Some(ProtocolType::Kitty));
        assert_eq!(pick(Wanted::Auto, &env, sixel), Some(ProtocolType::Sixel));
        assert_eq!(pick(Wanted::Auto, &env, Answers::default()), None);
        assert_eq!(pick(Wanted::Off, &env, full), None);
        assert_eq!(
            pick(Wanted::Auto, &env, Answers { cell: None, ..full }),
            None
        );
        let forced = Wanted::from_name("sixel").unwrap();
        assert_eq!(
            pick(forced, &env, Answers::default()),
            Some(ProtocolType::Sixel)
        );

        let wezterm = Env {
            wezterm: true,
            ..env.clone()
        };
        assert_eq!(
            pick(Wanted::Auto, &wezterm, full),
            Some(ProtocolType::Iterm2)
        );
        let konsole = Env {
            konsole: true,
            ..env.clone()
        };
        assert_eq!(pick(Wanted::Auto, &konsole, full), None);
        let tmux = Env {
            in_multiplexer: true,
            ..env.clone()
        };
        assert_eq!(pick(forced, &tmux, full), None);
        assert!(worth_asking(Wanted::Auto, &env));
        assert!(!worth_asking(Wanted::Auto, &tmux) && !worth_asking(Wanted::Off, &env));
        assert!(!worth_asking(Wanted::Auto, &Env::default()));
    }

    #[test]
    fn size_in_cells() {
        let g = kitty();
        assert_eq!(g.cells((100, 100), 80), (10, 5));
        assert_eq!(g.cells((1, 1), 80), (1, 1));
        assert_eq!(g.cells((1600, 400), 80), (80, 10), "fits the width");
        assert_eq!(g.cells((100, 6000), 80), (1, 30), "fits the height");
        assert_eq!(g.cells((10_000, 1), 80), (80, 1));
        assert_eq!(g.cells((100, 100), 0), (1, 1));
    }

    #[test]
    fn only_real_pictures_are_opened() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.png", "a.jpg", "a.gif", "a.webp"] {
            let path = dir.path().join(name);
            picture(&path, 30, 40);
            assert_eq!(dimensions(&path), Some((30, 40)), "{name}");
            for protocol in [
                ProtocolType::Kitty,
                ProtocolType::Sixel,
                ProtocolType::Iterm2,
            ] {
                let g = Graphics::new(protocol, 10, 20);
                let encoded = g.encode(&path, 3, 2).unwrap();
                assert_eq!((encoded.size().width, encoded.size().height), (3, 2));
            }
        }
        // Содержимое важнее расширения.
        let renamed = dir.path().join("b.txt");
        std::fs::copy(dir.path().join("a.png"), &renamed).unwrap();
        assert!(dimensions(&renamed).is_some());
        let fake = dir.path().join("fake.png");
        std::fs::write(&fake, "<svg onload=alert(1)>").unwrap();
        assert_eq!(dimensions(&fake), None);
        let bytes = std::fs::read(dir.path().join("a.png")).unwrap();
        std::fs::write(&fake, &bytes[..40]).unwrap();
        assert!(kitty().encode(&fake, 3, 2).is_none(), "truncated");
        assert_eq!(dimensions(dir.path()), None);
        assert_eq!(dimensions(&dir.path().join("none.png")), None);
        #[cfg(unix)]
        assert_eq!(dimensions(Path::new("/dev/zero")), None);
    }

    #[test]
    fn huge_pictures_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        picture(&path, MAX_SIDE + 1, 1);
        assert_eq!(dimensions(&path), None);
        assert!(kitty().encode(&path, 3, 2).is_none());
    }
}
