//! Область просмотра: построение в фоне, прокрутка, связь с местом в тексте.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;

use ratatui_image::sliced::SlicedProtocol;
use ropey::Rope;

use crate::buffer::Buffer;
use crate::code::Highlighter;
use crate::files::is_markdown;
use crate::graphics::{Graphics, Images};
use crate::layout;
use crate::preview::{Doc, Item, Join, Line};
use crate::search::{Query, Search};
use crate::style::Mark;

/// Поле слева от просмотра.
pub const MARGIN: u16 = 1;
/// Тексты до этого размера строятся сразу, без мигания экрана.
const SYNC_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    revision: u64,
    width: usize,
    tab: usize,
}

/// Место, которое надо показать: байт исходного текста и строка экрана.
#[derive(Debug, Clone, Copy)]
struct Anchor {
    src: usize,
    row: usize,
}

pub struct Pane {
    doc: Doc,
    shown: Option<Key>,
    wanted: Option<Key>,
    job: Option<(Key, Receiver<Doc>)>,
    /// Построить это не удалось, повторять незачем.
    failed: Option<Key>,
    code: Highlighter,
    /// Подсветка видимого кода не закончена.
    unfinished: bool,
    top: usize,
    height: usize,
    anchor: Option<Anchor>,
    /// Пользователь прокручивал просмотр.
    scrolled: bool,
    /// Выбранная ссылка или чекбокс, номер в [`Doc::order`].
    selected: Option<usize>,
    found: Option<Found>,
    /// Куда был переход, пока пользователь не прокрутил дальше.
    focus: Option<usize>,
    graphics: Option<Graphics>,
    /// Документ, от которого считаются пути картинок.
    document: Option<PathBuf>,
    pictures: HashMap<u32, Picture>,
}

/// Картинка на пути к экрану.
enum Picture {
    Loading(Receiver<Option<SlicedProtocol>>),
    Ready(Box<SlicedProtocol>),
    Failed,
}

/// Столько картинок хранится готовыми, невидимые сверх этого выгружаются.
const KEEP_PICTURES: usize = 16;

/// Видимая картинка: номер, отступ слева, строка экрана её начала (может быть
/// выше края) и сколько её строк видно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub image: u32,
    pub x: u16,
    pub y: i16,
    pub rows: u16,
}

/// Кусок текста поиска: откуда он в просмотре.
struct Piece {
    /// Начало в тексте поиска.
    start: usize,
    line: usize,
    /// Начало в строке просмотра, после отступов.
    offset: usize,
}

/// Результат поиска по просмотру.
struct Found {
    query: Query,
    pieces: Vec<Piece>,
    hits: Vec<Range<usize>>,
    current: Option<usize>,
}

impl Found {
    fn new(doc: &Doc, search: &Search) -> Self {
        let mut text = String::new();
        let mut pieces = Vec::with_capacity(doc.lines.len());
        for (number, line) in doc.lines.iter().enumerate() {
            text.push_str(match line.join {
                Join::New => "\n",
                Join::Space => " ",
                Join::Glue => "",
            });
            let (prefix, content) = line.spans.split_at(line.prefix);
            pieces.push(Piece {
                start: text.len(),
                line: number,
                offset: prefix.iter().map(|s| s.text.len()).sum(),
            });
            text.extend(content.iter().map(|s| s.text.as_str()));
            text.push_str(doc.code_text(line).unwrap_or_default());
        }
        Self {
            query: search.query.clone(),
            // Пустые совпадения показать нельзя.
            hits: search.matches(&text).filter(|h| !h.is_empty()).collect(),
            pieces,
            current: None,
        }
    }

    fn line_of(&self, hit: usize) -> usize {
        let start = self.hits[hit].start;
        let piece = self.pieces.partition_point(|p| p.start <= start);
        self.pieces[piece.saturating_sub(1)].line
    }

    /// Байты строки просмотра, попавшие в совпадения, и признак текущего.
    fn in_line(&self, line: usize) -> Vec<(Range<usize>, bool)> {
        let Some(piece) = self.pieces.get(line) else {
            return Vec::new();
        };
        let end = self.pieces.get(line + 1).map_or(usize::MAX, |p| p.start);
        let first = self.hits.partition_point(|h| h.end <= piece.start);
        let hits = self.hits[first..].iter().enumerate();
        hits.take_while(|(_, h)| h.start < end)
            .map(|(i, h)| {
                let from = h.start.max(piece.start) - piece.start + piece.offset;
                let to = h.end.min(end) - piece.start + piece.offset;
                (from..to, self.current == Some(first + i))
            })
            .collect()
    }
}

impl Default for Pane {
    fn default() -> Self {
        Self {
            doc: Doc::default(),
            shown: None,
            wanted: None,
            job: None,
            failed: None,
            code: Highlighter::default(),
            unfinished: false,
            top: 0,
            height: 1,
            anchor: None,
            scrolled: false,
            selected: None,
            found: None,
            focus: None,
            graphics: None,
            document: None,
            pictures: HashMap::new(),
        }
    }
}

/// Из чего строится просмотр, кроме самого текста.
#[derive(Clone)]
struct Source {
    width: usize,
    tab: usize,
    images: Option<Images>,
    /// Файл не Markdown.
    plain: bool,
}

impl Source {
    fn build(&self, text: &str) -> Doc {
        if self.plain {
            Doc::plain(text, self.width, self.tab)
        } else {
            Doc::build(text, self.width, self.tab, self.images.as_ref())
        }
    }
}

fn spawn(rope: Rope, source: Source) -> Receiver<Doc> {
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        // Получателя может уже не быть: режим сменили или программу закрыли.
        let _ = sender.send(source.build(&rope.to_string()));
    });
    receiver
}

impl Pane {
    /// Пустой просмотр для другого документа в том же терминале.
    pub fn reset(&mut self, document: Option<PathBuf>) {
        *self = Self {
            graphics: self.graphics.take(),
            document,
            ..Self::default()
        };
    }

    pub fn set_graphics(&mut self, graphics: Option<Graphics>) {
        self.graphics = graphics;
    }

    pub fn graphics(&self) -> Option<&Graphics> {
        self.graphics.as_ref()
    }

    /// Файл показывается обычным текстом. Безымянный документ считается Markdown.
    pub fn is_plain(&self) -> bool {
        self.document.as_deref().is_some_and(|d| !is_markdown(d))
    }

    fn images(&self) -> Option<Images> {
        Some(Images {
            graphics: self.graphics.clone()?,
            document: self.document.clone(),
        })
    }

    /// Видимые картинки сверху вниз.
    pub fn shown_images(&self) -> Vec<Shown> {
        let mut out: Vec<Shown> = Vec::new();
        for (y, line) in self.visible().iter().enumerate() {
            let Some(part) = line.image else { continue };
            match out.last_mut() {
                Some(last) if last.image == part.image => last.rows += 1,
                _ => {
                    let prefix: usize = line.spans.iter().map(|s| layout::width(&s.text, 4)).sum();
                    out.push(Shown {
                        image: part.image,
                        x: MARGIN + prefix as u16,
                        y: y as i16 - part.row as i16,
                        rows: 1,
                    });
                }
            }
        }
        out
    }

    /// Загружает видимые картинки и забирает готовые.
    pub fn load_images(&mut self) {
        let Some(graphics) = &self.graphics else {
            return;
        };
        let shown = self.shown_images();
        if self.pictures.len() > KEEP_PICTURES {
            self.pictures
                .retain(|id, _| shown.iter().any(|s| s.image == *id));
        }
        for place in &shown {
            let image = &self.doc.images[place.image as usize];
            self.pictures.entry(place.image).or_insert_with(|| {
                let (sender, receiver) = channel();
                let (graphics, image) = (graphics.clone(), image.clone());
                std::thread::spawn(move || {
                    let _ = sender.send(graphics.encode(&image.path, image.columns, image.rows));
                });
                Picture::Loading(receiver)
            });
        }
        for picture in self.pictures.values_mut() {
            if let Picture::Loading(receiver) = picture {
                match receiver.try_recv() {
                    Ok(Some(ready)) => *picture = Picture::Ready(Box::new(ready)),
                    // Поток мог и упасть на испорченном файле.
                    Ok(None) | Err(TryRecvError::Disconnected) => *picture = Picture::Failed,
                    Err(TryRecvError::Empty) => {}
                }
            }
        }
    }

    pub fn picture(&self, image: u32) -> Option<&SlicedProtocol> {
        match self.pictures.get(&image)? {
            Picture::Ready(ready) => Some(ready),
            Picture::Loading(_) | Picture::Failed => None,
        }
    }

    /// Подпись на месте картинки, пока её нет, и признак ошибки.
    pub fn image_note(&self, image: u32) -> (&str, bool) {
        let failed = matches!(self.pictures.get(&image), Some(Picture::Failed));
        (&self.doc.images[image as usize].alt, failed)
    }

    /// Показать место исходного текста на заданной строке экрана.
    pub fn show(&mut self, src: usize, row: usize) {
        self.anchor = Some(Anchor { src, row });
        self.scrolled = false;
        self.focus = None;
        self.apply_anchor();
    }

    /// Приводит просмотр к тексту и размеру области.
    pub fn prepare(&mut self, buffer: &Buffer, width: usize, height: usize, tab: usize) {
        self.height = height.max(1);
        let key = Key {
            revision: buffer.revision(),
            width,
            tab,
        };
        self.wanted = Some(key);
        if self.shown != Some(key) && self.failed != Some(key) && self.job.is_none() {
            let source = Source {
                width,
                tab,
                images: self.images(),
                plain: self.is_plain(),
            };
            if buffer.byte_len() <= SYNC_BYTES {
                let doc = source.build(&buffer.text());
                self.accept(key, doc);
            } else {
                self.job = Some((key, spawn(buffer.rope(), source)));
            }
        }
        self.poll();
        self.apply_anchor();
        self.clamp();
    }

    /// Забирает готовый результат.
    fn poll(&mut self) {
        let Some((key, receiver)) = &self.job else {
            return;
        };
        let key = *key;
        match receiver.try_recv() {
            Ok(doc) => {
                self.job = None;
                // Устаревший результат не нужен, следующий вызов начнёт заново.
                if self.wanted == Some(key) {
                    self.accept(key, doc);
                }
            }
            Err(TryRecvError::Empty) => {}
            // Поток построения упал.
            Err(TryRecvError::Disconnected) => {
                self.job = None;
                self.failed = Some(key);
            }
        }
    }

    fn accept(&mut self, key: Key, doc: Doc) {
        // После отметки чекбокса строки те же, ничего не должно сдвинуться.
        let same_shape = self.shown.is_some_and(|k| k.width == key.width)
            && self.doc.lines.len() == doc.lines.len()
            && self.doc.order.len() == doc.order.len();
        if !same_shape {
            self.selected = None;
        }
        self.found = None;
        // После смены ширины сверху остаётся тот же текст.
        if self.anchor.is_none() && self.shown.is_some() && !same_shape {
            let src = self.source_at_top();
            self.anchor = Some(Anchor { src, row: 0 });
        }
        // Те же картинки заново не загружаются.
        if self.doc.images != doc.images {
            self.pictures.clear();
        }
        self.doc = doc;
        self.shown = Some(key);
        self.code = Highlighter::default();
    }

    /// Переход к месту по желанию пользователя: редактор потом последует за ним.
    pub fn jump(&mut self, src: usize) {
        self.show(src, 0);
        self.scrolled = true;
        self.selected = None;
        self.focus = Some(src);
    }

    pub fn focus(&self) -> Option<usize> {
        self.focus
    }

    // --- Выбор ---

    /// Выбирает следующую или предыдущую ссылку. `false`: выбирать нечего.
    pub fn select(&mut self, forward: bool) -> bool {
        let count = self.doc.order.len();
        if count == 0 {
            return false;
        }
        let bottom = self.top + self.height;
        let visible = |line: usize| (self.top..bottom).contains(&line);
        let selected = self.selected.filter(|s| visible(self.doc.order[*s].1));
        let next = match (selected, forward) {
            (Some(s), true) => (s + 1) % count,
            (Some(s), false) => (s + count - 1) % count,
            // Начинаем с того, что на экране.
            (None, true) => self.doc.order.partition_point(|(_, l)| *l < self.top) % count,
            (None, false) => {
                let after = self.doc.order.partition_point(|(_, l)| *l < bottom);
                (after + count - 1) % count
            }
        };
        self.selected = Some(next);
        self.reveal(self.doc.order[next].1);
        true
    }

    /// Выбирает ссылку или чекбокс под ячейкой области.
    pub fn select_at(&mut self, x: usize, y: usize) -> bool {
        if !self.is_ready() {
            return false;
        }
        let Some(line) = self.visible().get(y) else {
            return false;
        };
        let mut column = MARGIN as usize;
        let mut item = None;
        for span in &line.spans {
            let end = column + layout::width(&span.text, 4);
            if (column..end).contains(&x) {
                item = span.item;
            }
            column = end;
        }
        let Some(item) = item else { return false };
        self.selected = self.doc.order.iter().position(|(i, _)| *i == item);
        self.selected.is_some()
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.selected_id().map(|id| &self.doc.items[id as usize])
    }

    /// Номер выбранного в [`Doc::items`].
    pub fn selected_id(&self) -> Option<u32> {
        let selected = self.selected.filter(|_| self.is_ready())?;
        self.doc.order.get(selected).map(|(id, _)| *id)
    }

    fn reveal(&mut self, line: usize) {
        if !(self.top..self.top + self.height).contains(&line) {
            self.set_top(line.saturating_sub(self.height / 2));
        }
    }

    // --- Поиск ---

    /// Обновляет совпадения, если изменился запрос или документ.
    pub fn set_search(&mut self, search: Option<&Search>) {
        match search {
            None => self.found = None,
            Some(search) => {
                let known = self.found.as_ref().map(|f| &f.query);
                if known != Some(&search.query) && self.is_ready() {
                    self.found = Some(Found::new(&self.doc, search));
                }
            }
        }
    }

    /// Переходит к совпадению: его номер, число всех и был ли переход по кругу.
    pub fn find(&mut self, forward: bool) -> Option<(usize, usize, bool)> {
        let found = self.found.as_ref()?;
        let count = found.hits.len();
        if count == 0 {
            return None;
        }
        let first_visible = || {
            let byte = found.pieces.get(self.top).map_or(0, |p| p.start);
            found.hits.partition_point(|h| h.start < byte)
        };
        let (index, wrapped) = match (found.current, forward) {
            (Some(c), true) => ((c + 1) % count, c + 1 == count),
            (Some(c), false) => ((c + count - 1) % count, c == 0),
            (None, true) => (first_visible() % count, first_visible() == count),
            (None, false) => ((first_visible() + count - 1) % count, first_visible() == 0),
        };
        let line = found.line_of(index);
        if let Some(found) = &mut self.found {
            found.current = Some(index);
        }
        self.reveal(line);
        Some((index + 1, count, wrapped))
    }

    /// Совпадения в видимой строке: байты и признак текущего.
    pub fn hits(&self, row: usize) -> Vec<(Range<usize>, bool)> {
        let found = self.found.as_ref();
        found.map_or(Vec::new(), |f| f.in_line(self.top + row))
    }

    fn apply_anchor(&mut self) {
        if self.is_ready()
            && let Some(anchor) = self.anchor.take()
        {
            self.top = self.doc.line_at(anchor.src).saturating_sub(anchor.row);
            self.clamp();
        }
    }

    /// Показан просмотр текущего текста.
    pub fn is_ready(&self) -> bool {
        self.shown.is_some() && self.shown == self.wanted
    }

    /// Просмотр текущего текста построить не удалось.
    pub fn is_failed(&self) -> bool {
        self.failed.is_some() && self.failed == self.wanted
    }

    /// Нужна повторная отрисовка без участия пользователя.
    pub fn is_busy(&self) -> bool {
        let loading = |p: &Picture| matches!(p, Picture::Loading(_));
        self.job.is_some() || self.unfinished || self.pictures.values().any(loading)
    }

    pub fn is_scrolled(&self) -> bool {
        self.scrolled && self.is_ready()
    }

    /// Байт исходного текста вверху экрана.
    pub fn source_at_top(&self) -> usize {
        self.doc.lines.get(self.top).map_or(0, |l| l.src)
    }

    fn lowest_top(&self) -> usize {
        self.doc.lines.len().saturating_sub(self.height)
    }

    fn clamp(&mut self) {
        self.top = self.top.min(self.lowest_top());
    }

    pub fn scroll(&mut self, delta: isize) {
        self.set_top(self.top.saturating_add_signed(delta));
    }

    pub fn scroll_to_start(&mut self) {
        self.set_top(0);
    }

    pub fn scroll_to_end(&mut self) {
        self.set_top(usize::MAX);
    }

    fn set_top(&mut self, top: usize) {
        let before = self.top;
        self.top = top;
        self.clamp();
        if self.top != before {
            self.scrolled = true;
            self.focus = None;
        }
    }

    pub fn visible(&self) -> &[Line] {
        let end = (self.top + self.height).min(self.doc.lines.len());
        &self.doc.lines[self.top.min(end)..end]
    }

    /// Подсвечивает видимый код, сколько успеет до `deadline`.
    pub fn highlight(&mut self, deadline: Instant) {
        let end = (self.top + self.height).min(self.doc.lines.len());
        self.unfinished = false;
        for line in self.top..end {
            if let Some(code) = &self.doc.lines[line].code
                && !self
                    .code
                    .prepare(&self.doc, code.block, code.line, deadline)
            {
                self.unfinished = true;
                break;
            }
        }
    }

    pub fn code_text(&self, line: &Line) -> Option<&str> {
        self.doc.code_text(line)
    }

    pub fn code_mark(&self, line: &Line, byte: usize) -> Mark {
        let Some(code) = &line.code else {
            return Mark::TEXT;
        };
        Mark::tone(
            self.code
                .tone(code.block, code.line, code.bytes.start + byte),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn wait(pane: &mut Pane, buffer: &Buffer, width: usize) {
        for _ in 0..2000 {
            pane.prepare(buffer, width, 10, 4);
            if pane.is_ready() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("preview is not ready");
    }

    fn paragraphs(n: usize) -> String {
        (0..n).map(|i| format!("абзац номер {i}\n\n")).collect()
    }

    fn top_text(pane: &Pane) -> String {
        pane.visible()[0]
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect()
    }

    #[test]
    fn small_text_is_ready_at_once() {
        let buffer = Buffer::new(&paragraphs(50));
        let mut pane = Pane::default();
        pane.prepare(&buffer, 40, 10, 4);
        assert!(pane.is_ready() && !pane.is_busy());
        assert_eq!(pane.visible().len(), 10);
    }

    #[test]
    fn large_text_is_built_in_background_at_the_right_place() {
        let text = paragraphs(20_000);
        let buffer = Buffer::new(&text);
        let mut pane = Pane::default();
        pane.show(text.find("номер 15000\n").unwrap(), 0);
        pane.prepare(&buffer, 40, 10, 4);
        assert!(!pane.is_ready() && pane.is_busy());
        wait(&mut pane, &buffer, 40);
        assert_eq!(top_text(&pane), "абзац номер 15000");
        assert!(!pane.is_busy() && !pane.is_scrolled());
    }

    #[test]
    fn stale_result_is_not_shown() {
        let mut buffer = Buffer::new(&paragraphs(20_000));
        let mut pane = Pane::default();
        pane.prepare(&buffer, 40, 10, 4);
        buffer.paste("# Новое\n\n", false);
        wait(&mut pane, &buffer, 40);
        assert_eq!(top_text(&pane), "Новое");
    }

    #[test]
    fn failed_build_is_not_repeated_until_the_text_changes() {
        let mut buffer = Buffer::new(&paragraphs(20_000));
        let mut pane = Pane::default();
        pane.prepare(&buffer, 40, 10, 4);
        // Упавший поток оставляет канал без отправителя.
        let (key, _) = pane.job.take().unwrap();
        pane.job = Some((key, channel().1));
        for _ in 0..3 {
            pane.prepare(&buffer, 40, 10, 4);
            assert!(pane.is_failed() && !pane.is_ready() && !pane.is_busy());
        }
        buffer.paste("# Новое\n\n", false);
        wait(&mut pane, &buffer, 40);
        assert!(!pane.is_failed());
        assert_eq!(top_text(&pane), "Новое");
    }

    #[test]
    fn width_change_keeps_the_same_text_on_top() {
        let text = format!("{}\n\n", "слово ".repeat(50)).repeat(30);
        let buffer = Buffer::new(&text);
        let mut pane = Pane::default();
        pane.prepare(&buffer, 30, 10, 4);
        pane.scroll(100);
        let before = pane.source_at_top();
        pane.prepare(&buffer, 60, 10, 4);
        let after = pane.source_at_top();
        assert!(after <= before && before - after < 130, "{before} {after}");
    }

    #[test]
    fn tab_width_change_rebuilds_the_preview() {
        let buffer = Buffer::new("```\n\t\t\tкод\n```\n");
        let mut pane = Pane::default();
        let mut shown = |tab| {
            pane.prepare(&buffer, 20, 10, tab);
            pane.doc.lines.clone()
        };
        let narrow = shown(2);
        assert_ne!(narrow, shown(8));
    }

    #[test]
    fn scrolling_stops_at_the_edges() {
        let buffer = Buffer::new(&paragraphs(20));
        let mut pane = Pane::default();
        pane.prepare(&buffer, 40, 10, 4);
        pane.scroll(-5);
        assert!(!pane.is_scrolled());
        pane.scroll_to_end();
        assert_eq!(pane.visible().len(), 10);
        assert!(pane.is_scrolled());
        pane.scroll(100);
        assert_eq!(pane.visible().len(), 10);
        pane.scroll_to_start();
        assert_eq!(top_text(&pane), "абзац номер 0");
    }
}
