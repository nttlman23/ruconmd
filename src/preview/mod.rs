//! Построение просмотра: Markdown в строки экрана заданной ширины.

mod inline;
mod table;

use std::collections::HashMap;
use std::ops::Range;

use pulldown_cmark::{
    BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use crate::layout;
use crate::links;
use crate::style::{Mark, Tone};
use inline::Inline;
use table::Table;

/// Меньше этого содержимое не сужается, лишнее обрезается краем экрана.
const MIN_CONTENT: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub mark: Mark,
    /// Номер в [`Doc::items`].
    pub item: Option<u32>,
}

impl Span {
    fn new(text: impl Into<String>, mark: Mark) -> Self {
        Self {
            text: text.into(),
            mark,
            item: None,
        }
    }
}

/// Как строка экрана продолжает предыдущую.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Join {
    /// Новая строка текста.
    New,
    /// Перенос по пробелу.
    Space,
    /// Разрыв внутри слова.
    Glue,
}

/// То, что можно выбрать в просмотре.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Link(String),
    /// Чекбокс: место `[ ]` в исходном тексте и отметка.
    Task {
        src: usize,
        done: bool,
    },
}

/// Часть строки кода. Подсветка добавляется при отрисовке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeRef {
    pub block: u32,
    pub line: u32,
    pub bytes: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Отступы и содержимое.
    pub spans: Vec<Span>,
    /// Сколько первых фрагментов относится к отступам.
    pub prefix: usize,
    pub join: Join,
    pub code: Option<CodeRef>,
    /// Байт исходного текста, которому соответствует начало строки.
    pub src: usize,
}

#[derive(Debug, Default)]
pub struct CodeBlock {
    pub lang: String,
    pub lines: Vec<String>,
}

/// Готовый просмотр.
#[derive(Debug, Default)]
pub struct Doc {
    pub lines: Vec<Line>,
    pub items: Vec<Item>,
    /// Выбираемое в порядке показа: номер и первая строка.
    pub order: Vec<(u32, usize)>,
    pub code: Vec<CodeBlock>,
}

impl Doc {
    pub fn build(text: &str, width: usize, tab: usize) -> Self {
        let mut builder = Builder::new(width.max(1), tab);
        for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
            builder.event(event, range);
        }
        builder.finish()
    }

    /// Код, который показывает строка.
    pub fn code_text(&self, line: &Line) -> Option<&str> {
        let code = line.code.as_ref()?;
        let block = &self.code[code.block as usize];
        Some(&block.lines[code.line as usize][code.bytes.clone()])
    }

    /// Строка просмотра, показывающая байт исходного текста.
    pub fn line_at(&self, src: usize) -> usize {
        let mut line = self
            .lines
            .partition_point(|l| l.src <= src)
            .saturating_sub(1);
        // Блок начинается с первой непустой строки с тем же местом.
        while line > 0
            && self.lines[line - 1].src == self.lines[line].src
            && !self.lines[line - 1].is_blank()
        {
            line -= 1;
        }
        // Пустая строка перед блоком относится к его началу.
        if line + 1 < self.lines.len() && self.lines[line].is_blank() {
            line += 1;
        }
        line
    }
}

impl Line {
    fn is_blank(&self) -> bool {
        self.code.is_none() && self.spans.iter().all(|s| s.text.trim().is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: usize,
    pub text: String,
    pub src: usize,
    /// Имя для ссылок, повторы с номером.
    pub slug: String,
}

/// Заголовки документа по порядку.
pub fn headings(text: &str) -> Vec<Heading> {
    let mut out: Vec<Heading> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut open: Option<Heading> = None;
    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                open = Some(Heading {
                    level: level as usize,
                    text: String::new(),
                    src: range.start,
                    slug: String::new(),
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(mut heading) = open.take() {
                    let slug = links::slug(&heading.text);
                    let count = seen.entry(slug.clone()).or_insert(0);
                    heading.slug = match *count {
                        0 => slug,
                        n => format!("{slug}-{n}"),
                    };
                    *count += 1;
                    out.push(heading);
                }
            }
            Event::Text(t) | Event::Code(t) | Event::InlineMath(t) => {
                if let Some(heading) = &mut open {
                    heading.text.push_str(&t);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(heading) = &mut open {
                    heading.text.push(' ');
                }
            }
            _ => {}
        }
    }
    out
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
        | Options::ENABLE_MATH
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
}

/// Отступ вложенного блока: маркер в первой строке, выравнивание в остальных.
struct Prefix {
    first: String,
    rest: String,
    mark: Mark,
    width: usize,
    used: bool,
    /// Чекбокс в первой строке.
    item: Option<u32>,
}

struct List {
    next: Option<u64>,
    /// Между пунктами есть пустые строки.
    loose: bool,
}

struct Builder {
    doc: Doc,
    width: usize,
    tab: usize,
    prefixes: Vec<Prefix>,
    prefix_width: usize,
    lists: Vec<List>,
    inline: Inline,
    marks: Vec<Mark>,
    item: Option<u32>,
    images: Vec<String>,
    need_blank: bool,
    table: Option<Table>,
    code: Option<CodeBlock>,
    /// Текст, который выводится как есть: HTML, метаданные, содержимое кода.
    raw: Option<String>,
    heading: Option<HeadingLevel>,
    /// Начало текущего блока.
    src: usize,
}

impl Builder {
    fn new(width: usize, tab: usize) -> Self {
        Self {
            doc: Doc::default(),
            width,
            tab,
            prefixes: Vec::new(),
            prefix_width: 0,
            lists: Vec::new(),
            inline: Inline::default(),
            marks: Vec::new(),
            item: None,
            images: Vec::new(),
            need_blank: false,
            table: None,
            code: None,
            raw: None,
            heading: None,
            src: 0,
        }
    }

    fn finish(mut self) -> Doc {
        self.flush();
        // Поиск места в документе требует неубывающих позиций.
        let mut last = 0;
        for line in &mut self.doc.lines {
            last = line.src.max(last);
            line.src = last;
        }
        let mut seen = vec![false; self.doc.items.len()];
        for (number, line) in self.doc.lines.iter().enumerate() {
            for item in line.spans.iter().filter_map(|s| s.item) {
                if !std::mem::replace(&mut seen[item as usize], true) {
                    self.doc.order.push((item, number));
                }
            }
        }
        self.doc
    }

    // --- События ---

    fn event(&mut self, event: Event, range: Range<usize>) {
        match event {
            Event::Start(tag) => self.start(tag, range.start),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match &mut self.raw {
                Some(raw) => raw.push_str(&text),
                None => self.push(&text, Mark::TEXT, range.start),
            },
            Event::Code(text) => self.push(&text, Mark::tone(Tone::Code), range.start),
            Event::InlineMath(text) => {
                self.push(&format!("${text}$"), Mark::tone(Tone::Code), range.start);
            }
            Event::DisplayMath(text) => {
                let text = text.replace(['\r', '\n'], " ");
                self.push(&format!("$${text}$$"), Mark::tone(Tone::Code), range.start);
            }
            Event::Html(text) => match &mut self.raw {
                Some(raw) => raw.push_str(&text),
                None => self.push(&text, Mark::tone(Tone::Html), range.start),
            },
            Event::InlineHtml(text) => {
                let text = text.replace(['\r', '\n'], " ");
                self.push(&text, Mark::tone(Tone::Html), range.start);
            }
            Event::FootnoteReference(label) => {
                self.push(&format!("[{label}]"), Mark::tone(Tone::Link), range.start);
            }
            Event::SoftBreak => self.push(" ", Mark::TEXT, range.start),
            Event::HardBreak => self.push("\n", Mark::TEXT, range.start),
            Event::Rule => {
                self.begin(range.start);
                let rule = "─".repeat(self.available());
                self.emit(vec![Span::new(rule, Mark::tone(Tone::Dim))], range.start);
                self.need_blank = true;
            }
            Event::TaskListMarker(done) => self.task(done, range.start),
        }
    }

    fn start(&mut self, tag: Tag, src: usize) {
        match tag {
            Tag::Paragraph => {
                if let Some(list) = self.lists.last_mut() {
                    list.loose = true;
                }
                self.begin(src);
            }
            Tag::Heading { level, .. } => {
                self.begin(src);
                self.heading = Some(level);
                self.marks.push(heading_mark(level));
            }
            Tag::BlockQuote(kind) => {
                self.begin(src);
                self.push_prefix("▌ ".into(), "▌ ".into(), Mark::tone(Tone::Quote));
                if let Some(kind) = kind {
                    let mark = Mark::tone(Tone::Quote).bold();
                    self.emit(vec![Span::new(alert_name(kind), mark)], src);
                }
            }
            Tag::CodeBlock(kind) => {
                self.begin(src);
                let info = match &kind {
                    CodeBlockKind::Fenced(info) => info,
                    CodeBlockKind::Indented => "",
                };
                let lang = info.split([' ', ',', '{', '\t']).next().unwrap_or_default();
                self.code = Some(CodeBlock {
                    lang: lang.to_owned(),
                    lines: Vec::new(),
                });
                self.raw = Some(String::new());
            }
            Tag::HtmlBlock | Tag::MetadataBlock(_) => {
                self.begin(src);
                self.raw = Some(String::new());
            }
            Tag::List(next) => {
                self.begin(src);
                self.lists.push(List { next, loose: false });
            }
            Tag::Item => {
                self.begin(src);
                let marker = self.marker();
                let rest = " ".repeat(layout::width(&marker, self.tab));
                self.push_prefix(marker, rest, Mark::tone(Tone::Marker));
            }
            Tag::FootnoteDefinition(label) => {
                self.begin(src);
                let marker = format!("[{label}] ");
                self.push_prefix(marker, "    ".into(), Mark::tone(Tone::Link));
                // Абзацы сноски не относятся к списку вокруг неё.
                self.lists.push(List {
                    next: None,
                    loose: false,
                });
            }
            Tag::Table(alignments) => {
                self.begin(src);
                self.table = Some(Table::new(alignments));
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.start_row(src);
                }
            }
            Tag::TableCell => self.inline = Inline::default(),
            Tag::Emphasis => self.marks.push(Mark::TEXT.italic()),
            Tag::Strong => self.marks.push(Mark::TEXT.bold()),
            Tag::Strikethrough => self.marks.push(Mark::TEXT.strike()),
            Tag::Link { dest_url, .. } => {
                self.item = Some(self.add_item(Item::Link(dest_url.into_string())));
                self.marks.push(Mark::tone(Tone::Link).underline());
            }
            Tag::Image { dest_url, .. } => {
                self.marks.push(Mark::tone(Tone::Link));
                self.push("▣ ", Mark::TEXT, src);
                self.images.push(dest_url.into_string());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                self.need_blank = true;
            }
            TagEnd::Heading(level) => {
                self.flush();
                self.marks.pop();
                self.heading = None;
                let rule = match level {
                    HeadingLevel::H1 => "═",
                    HeadingLevel::H2 => "─",
                    _ => "",
                };
                if !rule.is_empty() {
                    let rule = rule.repeat(self.available());
                    self.emit(vec![Span::new(rule, Mark::tone(Tone::Heading))], self.src);
                }
                self.need_blank = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.pop_prefix();
                self.need_blank = true;
            }
            TagEnd::CodeBlock => {
                self.emit_code();
                self.need_blank = true;
            }
            TagEnd::HtmlBlock | TagEnd::MetadataBlock(_) => {
                self.emit_raw();
                self.need_blank = true;
            }
            TagEnd::List(_) => {
                self.lists.pop();
                self.need_blank = self.lists.last().is_none_or(|l| l.loose);
            }
            TagEnd::Item => {
                self.end_marked_block();
                self.need_blank = self.lists.last().is_some_and(|l| l.loose);
            }
            TagEnd::FootnoteDefinition => {
                self.end_marked_block();
                self.lists.pop();
                self.need_blank = true;
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    for (spans, src) in table.lay_out(self.available(), self.tab) {
                        self.emit(spans, src);
                    }
                }
                self.need_blank = true;
            }
            TagEnd::TableCell => {
                let mut cell = std::mem::take(&mut self.inline);
                cell.find_links(&mut self.doc.items);
                if let Some(table) = &mut self.table {
                    table.push_cell(cell);
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.marks.pop();
            }
            TagEnd::Link => {
                self.marks.pop();
                self.item = None;
            }
            TagEnd::Image => {
                if let Some(dest) = self.images.pop() {
                    // Адрес нужен, пока сама картинка не показана.
                    self.push(&format!(" ({dest})"), Mark::tone(Tone::Dim), self.src);
                }
                self.marks.pop();
            }
            _ => {}
        }
    }

    // --- Строчное содержимое ---

    fn push(&mut self, text: &str, mark: Mark, src: usize) {
        let outer = self.marks.iter().fold(Mark::TEXT, |a, b| a.with(*b));
        self.inline.push(text, outer.with(mark), self.item, src);
    }

    fn add_item(&mut self, item: Item) -> u32 {
        self.doc.items.push(item);
        self.doc.items.len() as u32 - 1
    }

    /// Выводит накопленный абзац.
    fn flush(&mut self) {
        if self.inline.is_empty() {
            return;
        }
        let mut inline = std::mem::take(&mut self.inline);
        inline.find_links(&mut self.doc.items);
        for (spans, src, join) in inline.wrap(self.available(), self.tab) {
            self.emit_joined(spans, src, join);
        }
    }

    // --- Блоки ---

    /// Начинает блок: завершает предыдущий текст и отделяет пустой строкой.
    fn begin(&mut self, src: usize) {
        self.flush();
        self.src = src;
        if std::mem::take(&mut self.need_blank) {
            let mut spans = Vec::new();
            let mut used = 0;
            for p in &self.prefixes {
                if used >= self.width {
                    break;
                }
                used += p.width;
                spans.push(Span::new(p.rest.clone(), p.mark));
            }
            self.doc.lines.push(Line {
                prefix: spans.len(),
                spans,
                join: Join::New,
                code: None,
                src,
            });
        }
    }

    fn available(&self) -> usize {
        self.width
            .saturating_sub(self.prefix_width)
            .max(MIN_CONTENT)
    }

    fn push_prefix(&mut self, first: String, rest: String, mark: Mark) {
        let width = layout::width(&first, self.tab);
        self.prefix_width += width;
        self.prefixes.push(Prefix {
            first,
            rest,
            mark,
            width,
            used: false,
            item: None,
        });
    }

    fn pop_prefix(&mut self) {
        if let Some(p) = self.prefixes.pop() {
            self.prefix_width -= p.width;
        }
    }

    /// Завершает пункт списка или сноску. Пустой пункт показывает один маркер.
    fn end_marked_block(&mut self) {
        self.flush();
        if self.prefixes.last().is_some_and(|p| !p.used) {
            self.emit(Vec::new(), self.src);
        }
        self.pop_prefix();
    }

    fn marker(&mut self) -> String {
        const BULLETS: [&str; 3] = ["• ", "◦ ", "▪ "];
        let depth = self.lists.len().saturating_sub(1);
        match self.lists.last_mut().and_then(|l| l.next.as_mut()) {
            Some(number) => {
                let marker = format!("{number}. ");
                *number = number.saturating_add(1);
                marker
            }
            None => BULLETS[depth % BULLETS.len()].into(),
        }
    }

    fn task(&mut self, done: bool, src: usize) {
        let box_ = if done { "☑ " } else { "☐ " };
        let numbered = self.lists.last().is_some_and(|l| l.next.is_some());
        if self.prefixes.last().is_none_or(|p| p.used) {
            return;
        }
        let item = self.add_item(Item::Task { src, done });
        let Some(p) = self.prefixes.last_mut() else {
            return;
        };
        p.item = Some(item);
        if numbered {
            p.first.push_str(box_);
        } else {
            p.first = box_.into();
        }
        let width = layout::width(&p.first, self.tab);
        p.rest = " ".repeat(width);
        self.prefix_width = self.prefix_width - p.width + width;
        p.width = width;
    }

    /// Отступы текущей строки. Глубже ширины экрана они не нужны.
    fn prefix_spans(&mut self) -> Vec<Span> {
        let mut spans = Vec::new();
        let mut used = 0;
        for p in &mut self.prefixes {
            if used >= self.width {
                break;
            }
            used += p.width;
            let text = if p.used { &p.rest } else { &p.first };
            let mut span = Span::new(text.clone(), p.mark);
            if !p.used {
                span.item = p.item;
            }
            p.used = true;
            spans.push(span);
        }
        spans
    }

    fn emit(&mut self, spans: Vec<Span>, src: usize) {
        self.emit_joined(spans, src, Join::New);
    }

    fn emit_joined(&mut self, spans: Vec<Span>, src: usize, join: Join) {
        let mut all = self.prefix_spans();
        let prefix = all.len();
        all.extend(spans);
        self.doc.lines.push(Line {
            spans: all,
            prefix,
            join,
            code: None,
            src,
        });
    }

    fn emit_raw(&mut self) {
        let raw = self.raw.take().unwrap_or_default();
        let mut inline = Inline::default();
        let mut src = self.src;
        for line in raw.trim_end().split_inclusive('\n') {
            let text = line.trim_end_matches(['\r', '\n']);
            inline.push(text, Mark::tone(Tone::Html), None, src);
            src += line.len();
            let lines = std::mem::take(&mut inline).wrap(self.available(), self.tab);
            for (spans, src, join) in lines {
                self.emit_joined(spans, src, join);
            }
        }
    }

    fn emit_code(&mut self) {
        let raw = self.raw.take().unwrap_or_default();
        let mut block = self.code.take().unwrap_or_default();
        let dim = Mark::tone(Tone::Dim);
        let top = if block.lang.is_empty() {
            "┌─".into()
        } else {
            format!("┌─ {}", block.lang)
        };
        self.emit(vec![Span::new(top, dim)], self.src);

        let index = self.doc.code.len() as u32;
        let width = self.available().saturating_sub(2).max(MIN_CONTENT);
        // Точное место строк в исходнике из-за отступов неизвестно, хватает оценки.
        let mut src = self.src;
        for (number, line) in raw.lines().enumerate() {
            src += line.len() + 1;
            let mut join = Join::New;
            for row in layout::rows(line, Some(width), self.tab) {
                let mut spans = self.prefix_spans();
                spans.push(Span::new("│ ", dim));
                self.doc.lines.push(Line {
                    prefix: spans.len(),
                    spans,
                    join: std::mem::replace(&mut join, Join::Glue),
                    code: Some(CodeRef {
                        block: index,
                        line: number as u32,
                        bytes: row.byte_start..row.byte_end,
                    }),
                    src,
                });
            }
        }
        self.emit(vec![Span::new("└─", dim)], src);
        block.lines = raw.lines().map(str::to_owned).collect();
        self.doc.code.push(block);
    }
}

fn heading_mark(level: HeadingLevel) -> Mark {
    let mark = Mark::tone(Tone::Heading);
    match level {
        HeadingLevel::H1 | HeadingLevel::H2 | HeadingLevel::H3 => mark.bold(),
        HeadingLevel::H4 => mark,
        _ => mark.italic(),
    }
}

fn alert_name(kind: BlockQuoteKind) -> &'static str {
    match kind {
        BlockQuoteKind::Note => "Note",
        BlockQuoteKind::Tip => "Tip",
        BlockQuoteKind::Important => "Important",
        BlockQuoteKind::Warning => "Warning",
        BlockQuoteKind::Caution => "Caution",
    }
}

#[cfg(test)]
mod tests;
