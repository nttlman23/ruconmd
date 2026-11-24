//! Подсветка кода в просмотре.
//!
//! Разбор медленный, поэтому выполняется по мере показа и с ограничением времени:
//! непоказанные строки не разбираются, недоделанные выводятся без цвета.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::preview::{CodeBlock, Doc};
use crate::style::Tone;

/// Более длинные строки и дальнейший текст блока не подсвечиваются.
const MAX_LINE: usize = 2000;

struct Syntaxes {
    set: SyntaxSet,
    /// Начала имён областей и их роли, от более точных.
    tones: Vec<(Scope, Tone)>,
}

const TONES: &[(&str, Tone)] = &[
    ("comment", Tone::Comment),
    ("string", Tone::String),
    ("constant", Tone::Number),
    ("markup.inserted", Tone::String),
    ("markup.deleted", Tone::Keyword),
    ("keyword", Tone::Keyword),
    ("storage", Tone::Keyword),
    ("entity.name.function", Tone::Function),
    ("support.function", Tone::Function),
    ("variable.function", Tone::Function),
    ("entity.name", Tone::Type),
    ("entity.other", Tone::Number),
    ("support", Tone::Type),
];

fn syntaxes() -> &'static Syntaxes {
    static SYNTAXES: OnceLock<Syntaxes> = OnceLock::new();
    SYNTAXES.get_or_init(|| Syntaxes {
        set: two_face::syntax::extra_newlines(),
        tones: TONES
            .iter()
            .map(|(name, tone)| (Scope::new(name).expect("scope names are valid"), *tone))
            .collect(),
    })
}

fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let lower = lang.to_lowercase();
    let name = match lower.as_str() {
        "" => return None,
        "csharp" => "cs",
        "shell" | "console" | "zsh" => "sh",
        "golang" => "go",
        "jsonc" | "json5" => "json",
        other => other,
    };
    syntaxes().set.find_syntax_by_token(name)
}

/// Роли участков строки: начало участка в байтах и роль до следующего.
type LineTones = Vec<(usize, Tone)>;

struct Progress {
    /// `None`: язык неизвестен или разбор остановлен.
    state: Option<(ParseState, ScopeStack)>,
    lines: Vec<LineTones>,
}

impl Progress {
    fn new(block: &CodeBlock) -> Self {
        let state = find_syntax(&block.lang).map(|s| (ParseState::new(s), ScopeStack::new()));
        Self {
            state,
            lines: Vec::new(),
        }
    }

    fn parse_next(&mut self, text: &str) {
        let Some((state, stack)) = &mut self.state else {
            return;
        };
        if text.len() > MAX_LINE {
            self.state = None;
            return;
        }
        let all = syntaxes();
        let Ok(ops) = state.parse_line(&format!("{text}\n"), &all.set) else {
            self.state = None;
            return;
        };
        let mut tones = vec![(0, tone_of(stack, &all.tones))];
        for (at, op) in ops {
            if stack.apply(&op).is_err() {
                self.state = None;
                return;
            }
            let tone = tone_of(stack, &all.tones);
            match tones.last_mut() {
                Some(last) if last.0 == at => last.1 = tone,
                _ => tones.push((at, tone)),
            }
        }
        self.lines.push(tones);
    }
}

fn tone_of(stack: &ScopeStack, tones: &[(Scope, Tone)]) -> Tone {
    let found = stack.as_slice().iter().rev().find_map(|scope| {
        let mut known = tones.iter();
        known.find(|(prefix, _)| prefix.is_prefix_of(*scope))
    });
    found.map_or(Tone::Text, |(_, tone)| *tone)
}

/// Состояние подсветки одного документа.
#[derive(Default)]
pub struct Highlighter {
    blocks: HashMap<u32, Progress>,
}

impl Highlighter {
    /// Подготавливает строку блока. `false`: до `deadline` не успели.
    pub fn prepare(&mut self, doc: &Doc, block: u32, line: u32, deadline: Instant) -> bool {
        let code = &doc.code[block as usize];
        let progress = self
            .blocks
            .entry(block)
            .or_insert_with(|| Progress::new(code));
        while progress.state.is_some() && progress.lines.len() <= line as usize {
            if Instant::now() >= deadline {
                return false;
            }
            progress.parse_next(&code.lines[progress.lines.len()]);
        }
        true
    }

    /// Роль байта строки кода.
    pub fn tone(&self, block: u32, line: u32, byte: usize) -> Tone {
        let progress = self.blocks.get(&block);
        let Some(tones) = progress.and_then(|p| p.lines.get(line as usize)) else {
            return Tone::Text;
        };
        let index = tones.partition_point(|(start, _)| *start <= byte);
        tones[index.saturating_sub(1)].1
    }
}

/// Загружает описания языков заранее, чтобы первый показ кода не задержался.
pub fn warm_up() {
    syntaxes();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tones(lang: &str, code: &str, line: u32, words: &[&str]) -> Vec<Tone> {
        let doc = Doc::build(&format!("```{lang}\n{code}\n```\n"), 80, 4);
        let mut h = Highlighter::default();
        let deadline = Instant::now() + Duration::from_secs(60);
        assert!(h.prepare(&doc, 0, line, deadline));
        let text = &doc.code[0].lines[line as usize];
        words
            .iter()
            .map(|w| h.tone(0, line, text.find(w).unwrap()))
            .collect()
    }

    #[test]
    fn colors_rust() {
        let code = "// комментарий\nlet x = \"строка\"; let n = 5;";
        assert_eq!(tones("rust", code, 0, &["коммент"]), [Tone::Comment]);
        let got = tones("rust", code, 1, &["let", "строка", "5"]);
        assert_eq!(got, [Tone::Keyword, Tone::String, Tone::Number]);
    }

    #[test]
    fn state_spans_lines() {
        let code = "/* начало\nвнутри\n*/ fn a() {}";
        assert_eq!(tones("rs", code, 1, &["внутри"]), [Tone::Comment]);
        assert_eq!(tones("rs", code, 2, &["fn"]), [Tone::Keyword]);
    }

    #[test]
    fn common_languages_are_known() {
        for lang in [
            "rust",
            "python",
            "toml",
            "ts",
            "yaml",
            "json",
            "bash",
            "shell",
            "dockerfile",
            "csharp",
            "kotlin",
            "go",
            "sql",
            "diff",
            "C++",
        ] {
            assert!(find_syntax(lang).is_some(), "{lang}");
        }
        assert!(find_syntax("нет-такого").is_none());
        assert!(find_syntax("").is_none());
    }

    #[test]
    fn unknown_language_and_long_lines_are_plain() {
        assert_eq!(tones("mermaid", "graph TD", 0, &["graph"]), [Tone::Text]);
        let long = format!("let a = 1;\n{}\nlet b = 2;", "x".repeat(MAX_LINE + 1));
        assert_eq!(tones("rust", &long, 2, &["let"]), [Tone::Text]);
        assert_eq!(tones("rust", &long, 0, &["let"]), [Tone::Keyword]);
    }

    #[test]
    fn stops_at_the_deadline_and_continues_later() {
        let doc = Doc::build("```rust\nlet a = 1;\nlet b = 2;\n```\n", 80, 4);
        let mut h = Highlighter::default();
        assert!(!h.prepare(&doc, 0, 1, Instant::now()));
        assert_eq!(h.tone(0, 1, 0), Tone::Text);
        assert!(h.prepare(&doc, 0, 1, Instant::now() + Duration::from_secs(60)));
        assert_eq!(h.tone(0, 1, 0), Tone::Keyword);
    }
}
