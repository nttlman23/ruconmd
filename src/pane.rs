//! Область просмотра: построение в фоне, прокрутка, связь с местом в тексте.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;

use ropey::Rope;

use crate::buffer::Buffer;
use crate::code::Highlighter;
use crate::preview::{Doc, Line};
use crate::style::Mark;

/// Тексты до этого размера строятся сразу, без мигания экрана.
const SYNC_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    revision: u64,
    width: usize,
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
    code: Highlighter,
    /// Подсветка видимого кода не закончена.
    unfinished: bool,
    top: usize,
    height: usize,
    anchor: Option<Anchor>,
    /// Пользователь прокручивал просмотр.
    scrolled: bool,
}

impl Default for Pane {
    fn default() -> Self {
        Self {
            doc: Doc::default(),
            shown: None,
            wanted: None,
            job: None,
            code: Highlighter::default(),
            unfinished: false,
            top: 0,
            height: 1,
            anchor: None,
            scrolled: false,
        }
    }
}

fn spawn(rope: Rope, width: usize, tab: usize) -> Receiver<Doc> {
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        // Получателя может уже не быть: режим сменили или программу закрыли.
        let _ = sender.send(Doc::build(&rope.to_string(), width, tab));
    });
    receiver
}

impl Pane {
    /// Показать место исходного текста на заданной строке экрана.
    pub fn show(&mut self, src: usize, row: usize) {
        self.anchor = Some(Anchor { src, row });
        self.scrolled = false;
        self.apply_anchor();
    }

    /// Приводит просмотр к тексту и размеру области.
    pub fn prepare(&mut self, buffer: &Buffer, width: usize, height: usize, tab: usize) {
        self.height = height.max(1);
        let key = Key {
            revision: buffer.revision(),
            width,
        };
        self.wanted = Some(key);
        if self.shown != Some(key) && self.job.is_none() {
            if buffer.byte_len() <= SYNC_BYTES {
                let doc = Doc::build(&buffer.text(), width, tab);
                self.accept(key, doc);
            } else {
                self.job = Some((key, spawn(buffer.rope(), width, tab)));
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
            Err(TryRecvError::Disconnected) => self.job = None,
        }
    }

    fn accept(&mut self, key: Key, doc: Doc) {
        // После смены ширины сверху остаётся тот же текст.
        if self.anchor.is_none() && self.shown.is_some() {
            let src = self.source_at_top();
            self.anchor = Some(Anchor { src, row: 0 });
        }
        self.doc = doc;
        self.shown = Some(key);
        self.code = Highlighter::default();
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

    /// Нужна повторная отрисовка без участия пользователя.
    pub fn is_busy(&self) -> bool {
        self.job.is_some() || self.unfinished
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
        self.scrolled |= self.top != before;
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

    /// Текст и оформление видимой части строки кода.
    pub fn code_text(&self, line: &Line) -> Option<&str> {
        let code = line.code.as_ref()?;
        let block = &self.doc.code[code.block as usize];
        Some(&block.lines[code.line as usize][code.bytes.clone()])
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
