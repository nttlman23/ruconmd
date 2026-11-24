//! Поиск и замена в тексте.

use std::ops::Range;

use regex::{NoExpand, Regex, RegexBuilder, Replacer};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub text: String,
    pub case_sensitive: bool,
    pub regex: bool,
}

#[derive(Debug, Clone)]
pub struct Search {
    pub query: Query,
    regex: Regex,
}

impl Search {
    pub fn new(query: Query) -> Result<Self, String> {
        let pattern = if query.regex {
            query.text.clone()
        } else {
            regex::escape(&query.text)
        };
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!query.case_sensitive)
            .multi_line(true)
            .crlf(true)
            .build()
            .map_err(|e| brief(&e))?;
        Ok(Self { query, regex })
    }

    pub fn matches<'a>(&'a self, text: &'a str) -> impl Iterator<Item = Range<usize>> + 'a {
        self.regex.find_iter(text).map(|m| m.range())
    }

    /// Первое совпадение, начинающееся не раньше байта `from`.
    pub fn next(&self, text: &str, from: usize) -> Option<Range<usize>> {
        self.regex.find_at(text, from).map(|m| m.range())
    }

    /// Последнее совпадение, начинающееся раньше байта `before`.
    pub fn previous(&self, text: &str, before: usize) -> Option<Range<usize>> {
        self.matches(text).take_while(|m| m.start < before).last()
    }

    /// Чем заменить совпадение. В режиме regex доступны группы `$1`, `${имя}`
    /// и обозначения `\n`, `\t`.
    pub fn replacement(&self, text: &str, hit: &Range<usize>, with: &str) -> Option<String> {
        let captures = self.regex.captures_at(text, hit.start)?;
        if captures.get(0)?.range() != *hit {
            return None;
        }
        let mut out = String::new();
        if self.query.regex {
            captures.expand(&unescape(with), &mut out);
        } else {
            NoExpand(with).replace_append(&captures, &mut out);
        }
        Some(out)
    }

    /// Заменяет совпадения, прошедшие отбор, одной правкой: диапазон, новый текст
    /// и число замен.
    pub fn replace_all(
        &self,
        text: &str,
        with: &str,
        accept: impl Fn(&Range<usize>) -> bool,
    ) -> Option<(Range<usize>, String, usize)> {
        let with = if self.query.regex {
            unescape(with)
        } else {
            with.to_owned()
        };
        let mut out = String::new();
        let mut span: Option<Range<usize>> = None;
        let mut count = 0;
        for captures in self.regex.captures_iter(text) {
            let hit = captures.get(0)?.range();
            if !accept(&hit) {
                continue;
            }
            let span = span.get_or_insert(hit.start..hit.start);
            out.push_str(&text[span.end..hit.start]);
            if self.query.regex {
                captures.expand(&with, &mut out);
            } else {
                out.push_str(&with);
            }
            span.end = hit.end;
            count += 1;
        }
        span.map(|span| (span, out, count))
    }
}

/// Последняя строка сообщения об ошибке, без рисунка выражения.
fn brief(error: &regex::Error) -> String {
    let text = error.to_string();
    let line = text.lines().last().unwrap_or_default();
    line.trim_start_matches("error: ").to_owned()
}

fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some(other) => out.extend(['\\', other]),
                None => out.push('\\'),
            },
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(text: &str, case_sensitive: bool, regex: bool) -> Search {
        Search::new(Query {
            text: text.into(),
            case_sensitive,
            regex,
        })
        .unwrap()
    }

    #[test]
    fn case_works_for_cyrillic() {
        let text = "Слово слово СЛОВО";
        assert_eq!(search("слово", false, false).matches(text).count(), 3);
        assert_eq!(search("слово", true, false).matches(text).count(), 1);
    }

    #[test]
    fn literal_text_is_not_a_pattern() {
        assert_eq!(search("a.c*", false, false).matches("abc a.c*").count(), 1);
        assert_eq!(search("a.c", false, true).matches("abc a.c").count(), 2);
    }

    #[test]
    fn directions() {
        let s = search("a", false, false);
        assert_eq!(s.next("a-a-a", 1), Some(2..3));
        assert_eq!(s.previous("a-a-a", 4), Some(2..3));
        assert_eq!(s.previous("a-a-a", 0), None);
    }

    #[test]
    fn line_anchors_understand_crlf() {
        let s = search("b$", false, true);
        assert_eq!(s.matches("ab\r\ncb\r\n").count(), 2);
    }

    #[test]
    fn replacements() {
        let s = search(r"(\w+)@(\w+)", false, true);
        let hit = s.next("x a@b", 0).unwrap();
        assert_eq!(s.replacement("x a@b", &hit, "$2\\n$1").unwrap(), "b\na");
        let s = search("a", false, false);
        assert_eq!(s.replacement("a", &(0..1), "$1\\n").unwrap(), "$1\\n");
        let (span, out, count) = s.replace_all("xaxaxa", "aa", |r| r.start > 1).unwrap();
        assert_eq!((span, out.as_str(), count), (3..6, "aaxaa", 2));
        assert!(s.replace_all("zzz", "", |_| true).is_none());
    }

    #[test]
    fn bad_pattern_is_explained() {
        let query = Query {
            text: "(".into(),
            regex: true,
            ..Query::default()
        };
        let error = Search::new(query).unwrap_err();
        assert!(error.contains("unclosed group"), "{error}");
    }
}
