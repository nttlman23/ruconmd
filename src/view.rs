//! Видимая область текста: прокрутка, перенос, попадание мыши.

use std::cell::RefCell;
use std::rc::Rc;

use crate::buffer::Buffer;
use crate::layout::{self, Row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub tab_width: usize,
    pub wrap: bool,
    pub line_numbers: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_width: 4,
            wrap: true,
            line_numbers: true,
        }
    }
}

/// Место в документе: строка текста и экранная строка внутри неё.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Place {
    pub line: usize,
    pub row: usize,
}

/// Строка текста, разложенная по экранным.
#[derive(Debug)]
pub struct LaidOut {
    pub text: String,
    pub rows: Vec<Row>,
}

/// Строки короче раскладываются быстро и в кэш не попадают.
const CACHE_MIN_BYTES: usize = 4096;
const CACHE_LINES: usize = 4;

#[derive(Debug, PartialEq, Eq)]
struct CacheKey {
    revision: u64,
    line: usize,
    width: usize,
    settings: Settings,
}

#[derive(Debug)]
pub struct Viewport {
    cache: RefCell<Vec<(CacheKey, Rc<LaidOut>)>>,
    pub settings: Settings,
    pub top: Place,
    /// Горизонтальная прокрутка, только без переноса.
    pub left: usize,
    pub width: usize,
    pub height: usize,
}

impl Viewport {
    pub fn new(settings: Settings) -> Self {
        Self {
            cache: RefCell::default(),
            settings,
            top: Place::default(),
            left: 0,
            width: 1,
            height: 1,
        }
    }

    /// Последняя колонка оставлена для курсора в конце строки.
    fn wrap_width(&self) -> Option<usize> {
        self.settings
            .wrap
            .then(|| self.width.saturating_sub(1).max(1))
    }

    pub fn lay_out(&self, buffer: &Buffer, line: usize) -> Rc<LaidOut> {
        let key = CacheKey {
            revision: buffer.revision(),
            line,
            width: self.width,
            settings: self.settings,
        };
        let mut cache = self.cache.borrow_mut();
        if let Some((_, laid)) = cache.iter().find(|(k, _)| *k == key) {
            return Rc::clone(laid);
        }
        let text = buffer.line(line);
        let rows = layout::rows(&text, self.wrap_width(), self.settings.tab_width);
        let laid = Rc::new(LaidOut { text, rows });
        if laid.text.len() >= CACHE_MIN_BYTES {
            if cache.len() == CACHE_LINES {
                cache.remove(0);
            }
            cache.push((key, Rc::clone(&laid)));
        }
        laid
    }

    fn place_of(&self, buffer: &Buffer, pos: usize) -> (Place, Rc<LaidOut>, usize) {
        let line = buffer.line_of(pos);
        let laid = self.lay_out(buffer, line);
        let offset = pos - buffer.line_start(line);
        let row = layout::row_of(&laid.rows, offset);
        (Place { line, row }, laid, offset)
    }

    /// Место и экранная колонка позиции.
    pub fn locate(&self, buffer: &Buffer, pos: usize) -> (Place, usize) {
        let (place, laid, offset) = self.place_of(buffer, pos);
        let row = laid.rows[place.row];
        let column = layout::column_of(&laid.text, row, offset, self.settings.tab_width);
        (place, column)
    }

    /// Позиция в документе по месту и экранной колонке.
    pub fn position(&self, buffer: &Buffer, place: Place, column: usize) -> usize {
        let laid = self.lay_out(buffer, place.line);
        let row = place.row.min(laid.rows.len() - 1);
        let tab = self.settings.tab_width;
        buffer.line_start(place.line) + layout::offset_at(&laid.text, &laid.rows, row, column, tab)
    }

    /// Место на `delta` экранных строк ниже или выше, в пределах документа.
    pub fn advance(&self, buffer: &Buffer, from: Place, delta: isize) -> Place {
        let mut place = from;
        let mut left = delta.unsigned_abs();
        if delta >= 0 {
            let mut count = self.lay_out(buffer, place.line).rows.len();
            while left > 0 {
                let here = count - 1 - place.row.min(count - 1);
                if left <= here {
                    place.row += left;
                    break;
                }
                if place.line + 1 >= buffer.line_count() {
                    place.row = count - 1;
                    break;
                }
                left -= here + 1;
                place = Place {
                    line: place.line + 1,
                    row: 0,
                };
                count = self.lay_out(buffer, place.line).rows.len();
            }
        } else {
            while left > 0 {
                if left <= place.row {
                    place.row -= left;
                    break;
                }
                if place.line == 0 {
                    place.row = 0;
                    break;
                }
                left -= place.row + 1;
                place.line -= 1;
                place.row = self.lay_out(buffer, place.line).rows.len() - 1;
            }
        }
        place
    }

    /// Самое нижнее положение верха: последняя строка документа внизу экрана.
    fn lowest_top(&self, buffer: &Buffer) -> Place {
        let line = buffer.line_count() - 1;
        let row = self.lay_out(buffer, line).rows.len() - 1;
        let back = self.height.saturating_sub(1) as isize;
        self.advance(buffer, Place { line, row }, -back)
    }

    pub fn scroll(&mut self, buffer: &Buffer, delta: isize) {
        self.top = self.advance(buffer, self.top, delta);
        self.clamp(buffer);
    }

    pub fn scroll_to_start(&mut self) {
        self.top = Place::default();
    }

    pub fn scroll_to_end(&mut self, buffer: &Buffer) {
        self.top = self.lowest_top(buffer);
    }

    /// Возвращает верх в пределы документа после правок и смены размера.
    pub fn clamp(&mut self, buffer: &Buffer) {
        self.top.line = self.top.line.min(buffer.line_count() - 1);
        let rows = self.lay_out(buffer, self.top.line).rows.len();
        self.top.row = self.top.row.min(rows - 1);
        self.top = self.top.min(self.lowest_top(buffer));
    }

    pub fn resize(&mut self, buffer: &Buffer, width: usize, height: usize) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.width, self.height) {
            return;
        }
        // Верхней остаётся та же часть текста, а не та же экранная строка.
        let laid = self.lay_out(buffer, self.top.line.min(buffer.line_count() - 1));
        let offset = laid.rows[self.top.row.min(laid.rows.len() - 1)].start;
        (self.width, self.height) = (width, height);
        self.top.line = self.top.line.min(buffer.line_count() - 1);
        let laid = self.lay_out(buffer, self.top.line);
        self.top.row = layout::row_of(&laid.rows, offset);
        self.clamp(buffer);
    }

    /// Прокручивает ровно настолько, чтобы позиция стала видна.
    pub fn reveal(&mut self, buffer: &Buffer, pos: usize) {
        self.clamp(buffer);
        let (place, column) = self.locate(buffer, pos);
        if place < self.top {
            self.top = place;
        } else {
            let back = self.height.saturating_sub(1) as isize;
            self.top = self.top.max(self.advance(buffer, place, -back));
        }
        if self.settings.wrap {
            self.left = 0;
        } else if column < self.left {
            self.left = column;
        } else if column >= self.left + self.width {
            self.left = column + 1 - self.width;
        }
    }

    /// Сколько экранных строк от верха до места, не больше высоты экрана.
    pub fn rows_between(&self, buffer: &Buffer, place: Place) -> usize {
        let mut at = self.top;
        for rows in 0..self.height {
            if at >= place {
                return rows;
            }
            at = self.advance(buffer, at, 1);
        }
        self.height
    }

    pub fn is_visible(&self, buffer: &Buffer, pos: usize) -> bool {
        let (place, _) = self.locate(buffer, pos);
        let last = self.advance(buffer, self.top, self.height.saturating_sub(1) as isize);
        (self.top..=last).contains(&place)
    }

    /// Позиция под ячейкой области. Координаты могут выходить за её край.
    pub fn hit(&self, buffer: &Buffer, x: isize, y: isize) -> usize {
        let place = self.advance(buffer, self.top, y);
        self.position(buffer, place, self.left + x.max(0) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(buffer: &Buffer, width: usize, height: usize) -> Viewport {
        let mut v = Viewport::new(Settings::default());
        v.resize(buffer, width, height);
        v
    }

    #[test]
    fn moves_through_wrapped_lines() {
        let b = Buffer::new("один два три\nx\n");
        let v = view(&b, 9, 5);
        let start = Place::default();
        assert_eq!(v.advance(&b, start, 1), Place { line: 0, row: 1 });
        assert_eq!(v.advance(&b, start, 2), Place { line: 1, row: 0 });
        assert_eq!(v.advance(&b, start, 99), Place { line: 2, row: 0 });
        assert_eq!(
            v.advance(&b, Place { line: 1, row: 0 }, -1),
            Place { line: 0, row: 1 }
        );
        assert_eq!(v.advance(&b, Place { line: 1, row: 0 }, -9), start);
    }

    #[test]
    fn long_line_cache_follows_edits_and_width() {
        let mut b = Buffer::new(&"слово ".repeat(2000));
        let mut v = view(&b, 31, 5);
        let count = |v: &Viewport, b: &Buffer| v.lay_out(b, 0).rows.len();
        let before = count(&v, &b);
        b.paste(&"ещё ".repeat(500), false);
        assert!(count(&v, &b) > before);
        b.undo();
        assert_eq!(count(&v, &b), before);
        v.resize(&b, 61, 5);
        assert!(count(&v, &b) < before);
        v.settings.wrap = false;
        assert_eq!(count(&v, &b), 1);
    }

    #[test]
    fn scrolling_stops_at_the_last_screen() {
        let b = Buffer::new(&"x\n".repeat(99));
        let mut v = view(&b, 20, 10);
        v.scroll(&b, 1000);
        assert_eq!(v.top.line, 90);
        v.scroll(&b, -1000);
        assert_eq!(v.top, Place::default());
    }

    #[test]
    fn reveal_is_minimal() {
        let b = Buffer::new(&"x\n".repeat(99));
        let mut v = view(&b, 20, 10);
        v.reveal(&b, b.line_start(5));
        assert_eq!(v.top.line, 0);
        v.reveal(&b, b.line_start(10));
        assert_eq!(v.top.line, 1);
        v.reveal(&b, 0);
        assert_eq!(v.top.line, 0);
    }

    #[test]
    fn every_position_is_visible_after_reveal() {
        let b = Buffer::new("слово \tи ещё оченьдлинноеслово 日本語\n\nконец текста\n");
        for (width, height) in [(1, 1), (2, 1), (5, 2), (9, 3), (80, 5)] {
            let mut v = view(&b, width, height);
            for pos in (0..=b.len()).chain((0..=b.len()).rev()) {
                v.reveal(&b, pos);
                assert!(v.is_visible(&b, pos), "{width}x{height} {pos}");
            }
        }
    }

    #[test]
    fn position_is_the_inverse_of_locate() {
        let b = Buffer::new("слово \tи ещё оченьдлинноеслово 日本語\n\nконец\r\n");
        for wrap in [true, false] {
            let mut v = view(&b, 9, 4);
            v.settings.wrap = wrap;
            let mut pos = 0;
            while pos < b.len() {
                let (place, column) = v.locate(&b, pos);
                assert_eq!(v.position(&b, place, column), pos);
                pos = b.next_grapheme(pos);
            }
        }
    }

    #[test]
    fn horizontal_scroll_without_wrap() {
        let b = Buffer::new(&"x".repeat(100));
        let mut v = view(&b, 10, 2);
        v.settings.wrap = false;
        v.reveal(&b, 50);
        assert_eq!(v.left, 41);
        v.reveal(&b, 3);
        assert_eq!(v.left, 3);
    }

    #[test]
    fn resize_keeps_the_same_text_on_top() {
        let b = Buffer::new(&"слово ".repeat(200));
        let mut v = view(&b, 31, 5);
        v.scroll(&b, 20);
        let top = v.lay_out(&b, 0).rows[v.top.row].start;
        v.resize(&b, 61, 5);
        let rows = &v.lay_out(&b, 0).rows;
        let row = rows[v.top.row];
        assert!(row.start <= top && top < row.end);
    }
}
