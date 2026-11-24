//! Раскладка строки текста по экранным строкам.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Знак на месте символов, которые нельзя вывести в терминал как есть.
pub const REPLACEMENT: &str = "\u{fffd}";

/// Экранная строка: диапазон символов и байтов внутри строки текста.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub start: usize,
    pub end: usize,
    pub byte_start: usize,
    pub byte_end: usize,
}

/// Графема с положением внутри экранной строки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph<'a> {
    pub text: &'a str,
    /// Смещение в символах от начала строки текста.
    pub offset: usize,
    pub chars: usize,
    pub column: usize,
    pub width: usize,
}

fn width_at(grapheme: &str, column: usize, tab: usize) -> usize {
    if grapheme == "\t" {
        return tab - column % tab;
    }
    if needs_replacement(grapheme) {
        return 1;
    }
    grapheme.width()
}

/// Управляющие и невидимые символы заменяются при выводе видимым знаком.
fn needs_replacement(grapheme: &str) -> bool {
    grapheme.contains(char::is_control) || grapheme.width() == 0
}

impl Glyph<'_> {
    /// Что вывести на экран. Табуляция выводится пробелами.
    pub fn symbol(&self) -> &str {
        if self.text == "\t" {
            " "
        } else if needs_replacement(self.text) {
            REPLACEMENT
        } else {
            self.text
        }
    }
}

/// Делит строку на экранные. `width`: `None` без переноса.
///
/// Пробел на границе остаётся в строке и может занять одну ячейку за `width`.
pub fn rows(line: &str, width: Option<usize>, tab: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let (mut start, mut byte_start) = (0, 0);
    let (mut offset, mut column) = (0, 0);
    // Последнее место переноса и ширина текста после него.
    let mut break_at = None;
    let mut since_break = 0;
    for (byte, g) in line.grapheme_indices(true) {
        let mut w = width_at(g, column, tab);
        if let Some(width) = width {
            let hangs = g == " " && column == width;
            if column + w > width && column > 0 && !hangs {
                let (end, byte_end) = break_at.unwrap_or((offset, byte));
                rows.push(Row {
                    start,
                    end,
                    byte_start,
                    byte_end,
                });
                column = if break_at.is_some() { since_break } else { 0 };
                (start, byte_start) = (end, byte_end);
                break_at = None;
                w = width_at(g, column, tab);
                // Остаток слова вместе с новой графемой может занять всю строку.
                if column + w > width && column > 0 {
                    rows.push(Row {
                        start,
                        end: offset,
                        byte_start,
                        byte_end: byte,
                    });
                    (start, byte_start, column) = (offset, byte, 0);
                    w = width_at(g, 0, tab);
                }
            }
        }
        column += w;
        offset += g.chars().count();
        since_break += w;
        if g == " " || g == "\t" {
            break_at = Some((offset, byte + g.len()));
            since_break = 0;
        }
    }
    rows.push(Row {
        start,
        end: offset,
        byte_start,
        byte_end: line.len(),
    });
    rows
}

/// Графемы экранной строки.
pub fn glyphs(line: &str, row: Row, tab: usize) -> impl Iterator<Item = Glyph<'_>> {
    let mut offset = row.start;
    let mut column = 0;
    line[row.byte_start..row.byte_end]
        .graphemes(true)
        .map(move |text| {
            let glyph = Glyph {
                text,
                offset,
                chars: text.chars().count(),
                column,
                width: width_at(text, column, tab),
            };
            offset += glyph.chars;
            column += glyph.width;
            glyph
        })
}

/// Ширина строки без переноса.
pub fn width(line: &str, tab: usize) -> usize {
    let mut column = 0;
    for g in line.graphemes(true) {
        column += width_at(g, column, tab);
    }
    column
}

/// Номер экранной строки, в которой стоит курсор со смещением `offset`.
pub fn row_of(rows: &[Row], offset: usize) -> usize {
    rows.partition_point(|r| r.start <= offset)
        .saturating_sub(1)
}

/// Экранная колонка символа со смещением `offset`.
pub fn column_of(line: &str, row: Row, offset: usize, tab: usize) -> usize {
    glyphs(line, row, tab)
        .take_while(|g| g.offset < offset)
        .last()
        .map_or(0, |g| g.column + g.width)
}

/// Смещение символа в колонке. За концом: конец строки текста, а в переносимой
/// строке последняя графема, иначе курсор оказался бы на следующей строке.
pub fn offset_at(line: &str, rows: &[Row], index: usize, column: usize, tab: usize) -> usize {
    let row = rows[index];
    let mut last = row.start;
    for g in glyphs(line, row, tab) {
        if column < g.column + g.width {
            return g.offset;
        }
        last = g.offset;
    }
    if index + 1 == rows.len() {
        row.end
    } else {
        last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(line: &str, width: usize) -> Vec<&str> {
        rows(line, Some(width), 4)
            .iter()
            .map(|r| &line[r.byte_start..r.byte_end])
            .collect()
    }

    #[test]
    fn wraps_at_words() {
        assert_eq!(split("один два три", 8), ["один два ", "три"]);
        assert_eq!(split("один два три", 7), ["один ", "два три"]);
        assert_eq!(split("", 5), [""]);
        assert_eq!(split("abc", 3), ["abc"]);
    }

    #[test]
    fn long_words_are_cut() {
        assert_eq!(split("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(split("a bcdefgh", 3), ["a ", "bcd", "efg", "h"]);
    }

    #[test]
    fn wide_and_narrow_windows() {
        assert_eq!(split("日本語", 4), ["日本", "語"]);
        assert_eq!(split("日本", 1), ["日", "本"]);
    }

    #[test]
    fn rows_cover_the_line_without_gaps_and_fit() {
        let line = "слово\tи  ещё   оченьдлинноеслово 日本語 e\u{301}👨‍👩‍👧 конец\u{200b}\u{7}";
        for width in 1..40 {
            let rows = rows(line, Some(width), 4);
            let mut next = (0, 0);
            for row in &rows {
                assert_eq!((row.start, row.byte_start), next);
                next = (row.end, row.byte_end);
                let used = glyphs(line, *row, 4)
                    .last()
                    .map_or(0, |g| g.column + g.width);
                assert!(used <= width.max(2) + 2, "width {width}: {used}");
                if width >= 4 {
                    assert!(used <= width + 1, "width {width}: {used}");
                }
            }
            assert_eq!(next, (line.chars().count(), line.len()));
        }
    }

    #[test]
    fn tabs_align_to_stops() {
        let line = "a\tb";
        let row = rows(line, None, 4)[0];
        assert_eq!(column_of(line, row, 2, 4), 4);
        assert_eq!(column_of(line, row, 3, 4), 5);
    }

    #[test]
    fn unsafe_symbols_are_replaced() {
        let line = "a\u{1b}\u{200b}\u{202e}b";
        let row = rows(line, None, 4)[0];
        let shown: String = glyphs(line, row, 4)
            .map(|g| g.symbol().to_owned())
            .collect();
        assert!(
            !shown.contains(['\u{1b}', '\u{200b}', '\u{202e}']),
            "{shown:?}"
        );
        assert!(shown.starts_with('a') && shown.ends_with('b'));
    }

    #[test]
    fn clicks_map_to_graphemes() {
        let line = "日本 конец";
        let rows = rows(line, Some(5), 4);
        assert_eq!(rows.len(), 2);
        assert_eq!(offset_at(line, &rows, 0, 1, 4), 0);
        assert_eq!(offset_at(line, &rows, 0, 2, 4), 1);
        assert_eq!(offset_at(line, &rows, 0, 99, 4), 2);
        assert_eq!(offset_at(line, &rows, 1, 99, 4), 8);
        assert_eq!(row_of(&rows, 2), 0);
        assert_eq!(row_of(&rows, 3), 1);
    }
}
