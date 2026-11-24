//! Текст абзаца с оформлением и его перенос по ширине.

use std::ops::Range;

use linkify::{LinkFinder, LinkKind};

use super::Span;
use crate::layout;
use crate::style::{Mark, Tone};

#[derive(Debug, Clone, Copy)]
struct Run {
    start: usize,
    mark: Mark,
    link: Option<u32>,
    src: usize,
}

/// Накопленный текст. `\n` означает обязательный перенос.
#[derive(Debug, Default)]
pub(super) struct Inline {
    text: String,
    runs: Vec<Run>,
}

impl Inline {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn push(&mut self, text: &str, mark: Mark, link: Option<u32>, src: usize) {
        if text.is_empty() {
            return;
        }
        self.runs.push(Run {
            start: self.text.len(),
            mark,
            link,
            src,
        });
        self.text.push_str(text);
    }

    pub fn set_bold(&mut self) {
        for run in &mut self.runs {
            run.mark = run.mark.bold();
        }
    }

    pub fn src(&self) -> Option<usize> {
        self.runs.first().map(|r| r.src)
    }

    fn run_end(&self, index: usize) -> usize {
        self.runs
            .get(index + 1)
            .map_or(self.text.len(), |r| r.start)
    }

    /// Первый фрагмент, пересекающий байт.
    fn run_at(&self, byte: usize) -> usize {
        self.runs
            .partition_point(|r| r.start <= byte)
            .saturating_sub(1)
    }

    /// Находит адреса без разметки: http, https, ftp, www и почту.
    pub fn find_links(&mut self, links: &mut Vec<String>) {
        if !self.text.contains("://") && !self.text.contains("www.") && !self.text.contains('@') {
            return;
        }
        let mut finder = LinkFinder::new();
        finder.url_must_have_scheme(false);
        let mut found: Vec<(Range<usize>, String)> = Vec::new();
        for link in finder.links(&self.text) {
            let url = link.as_str();
            let lower = url.to_ascii_lowercase();
            let target = match link.kind() {
                LinkKind::Email => format!("mailto:{url}"),
                _ if lower.starts_with("www.") => format!("http://{url}"),
                _ if ["http://", "https://", "ftp://"]
                    .iter()
                    .any(|s| lower.starts_with(s)) =>
                {
                    url.to_owned()
                }
                _ => continue,
            };
            let first = self.run_at(link.start());
            let plain = (first..self.runs.len())
                .take_while(|i| self.runs[*i].start < link.end())
                .all(|i| self.runs[i].link.is_none() && self.runs[i].mark.tone != Tone::Code);
            if plain {
                found.push((link.start()..link.end(), target));
            }
        }
        // С конца, чтобы номера фрагментов слева не сдвигались.
        for (range, target) in found.into_iter().rev() {
            links.push(target);
            self.set_link(range, links.len() as u32 - 1);
        }
    }

    fn set_link(&mut self, range: Range<usize>, link: u32) {
        self.split(range.end);
        self.split(range.start);
        for run in &mut self.runs {
            if range.contains(&run.start) {
                run.link = Some(link);
                run.mark = run.mark.with(Mark::tone(Tone::Link).underline());
            }
        }
    }

    fn split(&mut self, byte: usize) {
        if byte >= self.text.len() {
            return;
        }
        let index = self.run_at(byte);
        let run = self.runs[index];
        if run.start != byte {
            let second = Run {
                start: byte,
                src: run.src + (byte - run.start),
                ..run
            };
            self.runs.insert(index + 1, second);
        }
    }

    /// Ширина без переноса.
    pub fn width(&self, tab: usize) -> usize {
        let lines = self.text.split('\n');
        lines.map(|l| layout::width(l, tab)).max().unwrap_or(0)
    }

    /// Ширина самого длинного слова.
    pub fn longest_word(&self, tab: usize) -> usize {
        let words = self.text.split([' ', '\n', '\t']);
        words.map(|w| layout::width(w, tab)).max().unwrap_or(0)
    }

    /// Строки не шире `width` и место каждой в исходном тексте.
    pub fn wrap(&self, width: usize, tab: usize) -> Vec<(Vec<Span>, usize)> {
        let mut out = Vec::new();
        let mut base = 0;
        for segment in self.text.split('\n') {
            for row in layout::rows(segment, Some(width), tab) {
                let text = segment[row.byte_start..row.byte_end].trim_end_matches(' ');
                let start = base + row.byte_start;
                out.push(self.spans(start..start + text.len()));
            }
            base += segment.len() + 1;
        }
        out
    }

    fn spans(&self, range: Range<usize>) -> (Vec<Span>, usize) {
        let first = self.run_at(range.start);
        let src = self
            .runs
            .get(first)
            .map_or(0, |r| r.src + range.start.saturating_sub(r.start));
        let mut spans: Vec<Span> = Vec::new();
        let mut index = first;
        while index < self.runs.len() && self.runs[index].start < range.end {
            let run = self.runs[index];
            let text = &self.text[run.start.max(range.start)..self.run_end(index).min(range.end)];
            match spans.last_mut() {
                Some(s) if s.mark == run.mark && s.link == run.link => s.text.push_str(text),
                _ if text.is_empty() => {}
                _ => spans.push(Span {
                    text: text.to_owned(),
                    mark: run.mark,
                    link: run.link,
                }),
            }
            index += 1;
        }
        (spans, src)
    }
}
