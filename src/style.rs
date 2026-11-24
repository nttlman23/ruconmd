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

    /// Стиль встроенной тёмной темы.
    pub fn style(self) -> Style {
        let mut style = match self.tone {
            Tone::Text => Style::new(),
            Tone::Dim | Tone::Comment => Style::new().fg(Color::DarkGray),
            Tone::Heading => Style::new().fg(Color::Cyan),
            Tone::Marker | Tone::Keyword => Style::new().fg(Color::Magenta),
            Tone::Quote => Style::new().fg(Color::Yellow),
            Tone::Link | Tone::Function => Style::new().fg(Color::LightBlue),
            Tone::Html => Style::new().fg(Color::DarkGray),
            Tone::Code | Tone::String => Style::new().fg(Color::Green),
            Tone::Number => Style::new().fg(Color::Yellow),
            Tone::Type => Style::new().fg(Color::Cyan),
        };
        for (on, modifier) in [
            (self.bold, Modifier::BOLD),
            (self.italic, Modifier::ITALIC),
            (self.strike, Modifier::CROSSED_OUT),
            (self.underline, Modifier::UNDERLINED),
        ] {
            if on {
                style = style.add_modifier(modifier);
            }
        }
        style
    }
}
