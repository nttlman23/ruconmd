//! Таблицы: подбор ширины колонок и рамки.

use pulldown_cmark::Alignment;

use super::Span;
use super::inline::Inline;
use crate::layout;
use crate::style::{Mark, Tone};

/// Длиннее этого слово может быть разорвано, чтобы таблица поместилась.
const MAX_WORD: usize = 16;
/// В колонку помещается хотя бы один широкий символ.
const MIN_COLUMN: usize = 2;

struct Row {
    cells: Vec<Inline>,
    src: usize,
}

pub(super) struct Table {
    alignments: Vec<Alignment>,
    rows: Vec<Row>,
}

impl Table {
    pub fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            alignments,
            rows: Vec::new(),
        }
    }

    pub fn start_row(&mut self, src: usize) {
        self.rows.push(Row {
            cells: Vec::new(),
            src,
        });
    }

    pub fn push_cell(&mut self, mut cell: Inline) {
        let header = self.rows.len() == 1;
        if let Some(row) = self.rows.last_mut() {
            if header {
                cell.set_bold();
            }
            row.cells.push(cell);
        }
    }

    pub fn lay_out(&self, available: usize, tab: usize) -> Vec<(Vec<Span>, usize)> {
        let widths = self.widths(available, tab);
        let mut out = Vec::new();
        let Some(first) = self.rows.first() else {
            return out;
        };
        let wrapped: Vec<Vec<Vec<Vec<Span>>>> = self
            .rows
            .iter()
            .map(|row| {
                (0..widths.len())
                    .map(|c| match row.cells.get(c) {
                        Some(cell) => {
                            let lines = cell.wrap(widths[c], tab);
                            lines.into_iter().map(|(spans, _)| spans).collect()
                        }
                        None => Vec::new(),
                    })
                    .collect()
            })
            .collect();
        // Когда ячейки многострочные, строки таблицы разделяются линиями.
        let tall = wrapped.iter().skip(1).flatten().any(|cell| cell.len() > 1);

        out.push((vec![rule(&widths, "┌", "┬", "┐")], first.src));
        for (index, (row, cells)) in self.rows.iter().zip(&wrapped).enumerate() {
            let src = row.cells.iter().find_map(Inline::src).unwrap_or(row.src);
            if index == 1 || (index > 1 && tall) {
                out.push((vec![rule(&widths, "├", "┼", "┤")], src));
            }
            let height = cells.iter().map(Vec::len).max().unwrap_or(0).max(1);
            for line in 0..height {
                let mut spans = Vec::new();
                for (c, width) in widths.iter().enumerate() {
                    let border = if c == 0 { "│ " } else { " │ " };
                    spans.push(Span::new(border, Mark::tone(Tone::Dim)));
                    let text = cells[c].get(line).map_or(&[][..], Vec::as_slice);
                    let used: usize = text.iter().map(|s| layout::width(&s.text, tab)).sum();
                    let free = width.saturating_sub(used);
                    let left = match self.alignments[c] {
                        Alignment::Right => free,
                        Alignment::Center => free / 2,
                        Alignment::Left | Alignment::None => 0,
                    };
                    push_space(&mut spans, left);
                    spans.extend_from_slice(text);
                    push_space(&mut spans, free - left);
                }
                spans.push(Span::new(" │", Mark::tone(Tone::Dim)));
                out.push((spans, src));
            }
        }
        let last = out.last().map_or(first.src, |(_, src)| *src);
        out.push((vec![rule(&widths, "└", "┴", "┘")], last));
        out
    }

    /// Ширина колонок, чтобы таблица вместе с рамками поместилась в `available`.
    fn widths(&self, available: usize, tab: usize) -> Vec<usize> {
        let columns = self.alignments.len();
        let measure = |c: usize, f: &dyn Fn(&Inline) -> usize| {
            let cells = self.rows.iter().filter_map(|r| r.cells.get(c));
            cells.map(f).max().unwrap_or(0).max(1)
        };
        let natural: Vec<usize> = (0..columns)
            .map(|c| measure(c, &|cell| cell.width(tab)))
            .collect();
        let room = available.saturating_sub(3 * columns + 1);
        if natural.iter().sum::<usize>() <= room {
            return natural;
        }
        let words: Vec<usize> = (0..columns)
            .map(|c| measure(c, &|cell| cell.longest_word(tab).min(MAX_WORD)))
            .collect();
        let least: &[usize] = if words.iter().sum::<usize>() <= room {
            &words
        } else {
            &[]
        };
        // Широкие колонки ограничиваются общим пределом, узкие не трогаются.
        let with_cap = |cap: usize| -> Vec<usize> {
            (0..columns)
                .map(|c| natural[c].min(cap.max(least.get(c).copied().unwrap_or(MIN_COLUMN))))
                .collect()
        };
        let (mut low, mut high) = (MIN_COLUMN, room.max(MIN_COLUMN));
        while low < high {
            let middle = (low + high).div_ceil(2);
            if with_cap(middle).iter().sum::<usize>() <= room {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        with_cap(low)
    }
}

fn push_space(spans: &mut Vec<Span>, width: usize) {
    if width > 0 {
        spans.push(Span::new(" ".repeat(width), Mark::TEXT));
    }
}

fn rule(widths: &[usize], left: &str, middle: &str, right: &str) -> Span {
    let parts: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
    let text = format!("{left}{}{right}", parts.join(middle));
    Span::new(text, Mark::tone(Tone::Dim))
}
