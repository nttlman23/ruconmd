//! Смысловое оформление текста и цвета темы.

use ratatui::style::{Color, Modifier, Style};

/// Роль фрагмента текста. Цвет роли задаёт тема.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Text,
    /// Служебное: рамки, адреса, подписи.
    Dim,
    Heading,
    Marker,
    Quote,
    Link,
    Html,
    Code,
    Keyword,
    String,
    Comment,
    Number,
    Function,
    Type,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mark {
    pub tone: Tone,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub underline: bool,
}

impl Mark {
    pub const TEXT: Mark = Mark::tone(Tone::Text);

    pub const fn tone(tone: Tone) -> Self {
        Self {
            tone,
            bold: false,
            italic: false,
            strike: false,
            underline: false,
        }
    }

    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub const fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub const fn strike(mut self) -> Self {
        self.strike = true;
        self
    }

    pub const fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    /// Накладывает вложенное оформление на внешнее.
    pub fn with(self, inner: Mark) -> Self {
        Self {
            tone: if inner.tone == Tone::Text {
                self.tone
            } else {
                inner.tone
            },
            bold: self.bold || inner.bold,
            italic: self.italic || inner.italic,
            strike: self.strike || inner.strike,
            underline: self.underline || inner.underline,
        }
    }
}

/// Роли, цвет которых задаётся в `[themes.*]`, под именами из конфига.
const TONES: [(&str, Tone); 14] = [
    ("text", Tone::Text),
    ("dim", Tone::Dim),
    ("heading", Tone::Heading),
    ("marker", Tone::Marker),
    ("quote", Tone::Quote),
    ("link", Tone::Link),
    ("html", Tone::Html),
    ("code", Tone::Code),
    ("keyword", Tone::Keyword),
    ("string", Tone::String),
    ("comment", Tone::Comment),
    ("number", Tone::Number),
    ("function", Tone::Function),
    ("type", Tone::Type),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    tones: [Color; TONES.len()],
    background: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Theme {
    /// Для тёмного фона. Фон и текст берутся у терминала.
    pub const DARK: Theme = Theme {
        tones: [
            Color::Reset,
            Color::DarkGray,
            Color::Cyan,
            Color::Magenta,
            Color::Yellow,
            Color::LightBlue,
            Color::DarkGray,
            Color::Green,
            Color::Magenta,
            Color::Green,
            Color::DarkGray,
            Color::Yellow,
            Color::LightBlue,
            Color::Cyan,
        ],
        background: Color::Reset,
    };

    /// Со своим светлым фоном, поэтому читается в любом терминале.
    pub const LIGHT: Theme = Theme {
        tones: [
            rgb(0x1a1a1a),
            rgb(0x6b7280),
            rgb(0x005f87),
            rgb(0x8700af),
            rgb(0x875f00),
            rgb(0x0055cc),
            rgb(0x6b7280),
            rgb(0x00661a),
            rgb(0x8700af),
            rgb(0x00661a),
            rgb(0x6b7280),
            rgb(0xa04000),
            rgb(0x0055cc),
            rgb(0x005f87),
        ],
        background: rgb(0xffffff),
    };

    /// Задаёт цвет роли. `false`: такой роли нет.
    pub fn set(&mut self, role: &str, color: Color) -> bool {
        if role == "background" {
            self.background = color;
            return true;
        }
        let Some(index) = TONES.iter().position(|(name, _)| *name == role) else {
            return false;
        };
        self.tones[index] = color;
        true
    }

    fn color(&self, tone: Tone) -> Color {
        let index = TONES.iter().position(|(_, t)| *t == tone);
        index.map_or(Color::Reset, |i| self.tones[i])
    }

    /// Приводит цвета к тому, что умеет терминал.
    pub fn fitted(mut self, depth: Depth) -> Self {
        for color in self.tones.iter_mut().chain([&mut self.background]) {
            *color = depth.fit(*color);
        }
        self
    }

    /// Стиль всего экрана.
    pub fn base(&self) -> Style {
        Style::new().fg(self.color(Tone::Text)).bg(self.background)
    }

    pub fn style(&self, mark: Mark) -> Style {
        let mut style = Style::new().fg(self.color(mark.tone));
        for (on, modifier) in [
            (mark.bold, Modifier::BOLD),
            (mark.italic, Modifier::ITALIC),
            (mark.strike, Modifier::CROSSED_OUT),
            (mark.underline, Modifier::UNDERLINED),
        ] {
            if on {
                style = style.add_modifier(modifier);
            }
        }
        style
    }
}

/// Цвет из конфига: `#rrggbb`, имя, номер 0–255 или `default`.
pub fn parse_color(text: &str) -> Option<Color> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("default") {
        return Some(Color::Reset);
    }
    if let Some(hex) = text.strip_prefix('#') {
        let valid = hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit());
        return valid
            .then(|| u32::from_str_radix(hex, 16).ok().map(rgb))
            .flatten();
    }
    text.parse().ok()
}

/// Сколько цветов показывает терминал.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    True,
    Indexed,
    Ansi,
}

impl Depth {
    pub fn from_system() -> Self {
        if cfg!(windows) {
            return Self::True;
        }
        let var = |name| std::env::var(name).unwrap_or_default();
        Self::from_env(&var("COLORTERM"), &var("TERM"))
    }

    fn from_env(colorterm: &str, term: &str) -> Self {
        if matches!(colorterm, "truecolor" | "24bit") || term.ends_with("-direct") {
            Self::True
        } else if term.contains("256color") {
            Self::Indexed
        } else {
            Self::Ansi
        }
    }

    fn fit(self, color: Color) -> Color {
        match (self, color) {
            (Self::Indexed, Color::Rgb(r, g, b)) => Color::Indexed(to_indexed(r, g, b)),
            (Self::Ansi, Color::Rgb(r, g, b)) => to_ansi(r, g, b),
            (Self::Ansi, Color::Indexed(i)) if i >= 16 => {
                let (r, g, b) = from_indexed(i);
                to_ansi(r, g, b)
            }
            _ => color,
        }
    }
}

const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn from_indexed(index: u8) -> (u8, u8, u8) {
    if index >= 232 {
        let gray = 8 + (index - 232) * 10;
        return (gray, gray, gray);
    }
    let i = index.saturating_sub(16) as usize;
    (CUBE[i / 36], CUBE[i / 6 % 6], CUBE[i % 6])
}

fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| u32::from(x.abs_diff(y)).pow(2);
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

fn to_indexed(r: u8, g: u8, b: u8) -> u8 {
    let nearest = (16..=255).min_by_key(|i| distance(from_indexed(*i), (r, g, b)));
    nearest.unwrap_or(15)
}

fn to_ansi(r: u8, g: u8, b: u8) -> Color {
    const ANSI: [(Color, (u8, u8, u8)); 16] = [
        (Color::Black, (0, 0, 0)),
        (Color::Red, (170, 0, 0)),
        (Color::Green, (0, 170, 0)),
        (Color::Yellow, (170, 85, 0)),
        (Color::Blue, (0, 0, 170)),
        (Color::Magenta, (170, 0, 170)),
        (Color::Cyan, (0, 170, 170)),
        (Color::Gray, (170, 170, 170)),
        (Color::DarkGray, (85, 85, 85)),
        (Color::LightRed, (255, 85, 85)),
        (Color::LightGreen, (85, 255, 85)),
        (Color::LightYellow, (255, 255, 85)),
        (Color::LightBlue, (85, 85, 255)),
        (Color::LightMagenta, (255, 85, 255)),
        (Color::LightCyan, (85, 255, 255)),
        (Color::White, (255, 255, 255)),
    ];
    let nearest = ANSI.iter().min_by_key(|(_, c)| distance(*c, (r, g, b)));
    nearest.map_or(Color::Reset, |(color, _)| *color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tone_has_its_own_color_slot() {
        let mut theme = Theme::DARK;
        for (i, (name, _)) in TONES.iter().enumerate() {
            assert!(theme.set(name, Color::Indexed(i as u8)));
        }
        for (i, (_, tone)) in TONES.iter().enumerate() {
            assert_eq!(theme.color(*tone), Color::Indexed(i as u8));
        }
        assert!(!theme.set("no-such-role", Color::Red));
    }

    #[test]
    fn colors_are_parsed() {
        assert_eq!(parse_color("#7aa2f7"), Some(Color::Rgb(0x7a, 0xa2, 0xf7)));
        assert_eq!(parse_color("red"), Some(Color::Red));
        assert_eq!(parse_color("208"), Some(Color::Indexed(208)));
        assert_eq!(parse_color("default"), Some(Color::Reset));
        for bad in ["#12", "#zzzzzz", "#яяя", "", "256", "нет"] {
            assert_eq!(parse_color(bad), None, "{bad}");
        }
    }

    #[test]
    fn colors_fit_the_terminal() {
        assert_eq!(Depth::from_env("truecolor", "xterm"), Depth::True);
        assert_eq!(Depth::from_env("", "xterm-256color"), Depth::Indexed);
        assert_eq!(Depth::from_env("", "linux"), Depth::Ansi);
        let white = Color::Rgb(255, 255, 255);
        assert_eq!(Depth::Indexed.fit(white), Color::Indexed(231));
        assert_eq!(Depth::Ansi.fit(white), Color::White);
        assert_eq!(Depth::Ansi.fit(Color::Indexed(196)), Color::Red);
        assert_eq!(Depth::Ansi.fit(Color::Cyan), Color::Cyan);
        let fitted = Theme::LIGHT.fitted(Depth::Ansi);
        assert_eq!(fitted.background, Color::White);
    }
}
