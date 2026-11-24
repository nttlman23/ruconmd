use super::*;

fn doc(text: &str, width: usize) -> Doc {
    Doc::build(text, width, 4)
}

fn shown(doc: &Doc) -> Vec<String> {
    doc.lines
        .iter()
        .map(|l| {
            let mut s: String = l.spans.iter().map(|s| s.text.as_str()).collect();
            if let Some(c) = &l.code {
                s += &doc.code[c.block as usize].lines[c.line as usize][c.bytes.clone()];
            }
            s.trim_end().to_owned()
        })
        .collect()
}

fn render(text: &str, width: usize) -> Vec<String> {
    shown(&doc(text, width))
}

fn find<'a>(doc: &'a Doc, text: &str) -> &'a Span {
    let mut spans = doc.lines.iter().flat_map(|l| &l.spans);
    spans
        .find(|s| s.text == text)
        .unwrap_or_else(|| panic!("no span {text:?}"))
}

#[test]
fn headings_and_paragraphs() {
    let got = render("# Один\nтекст\nдальше\n\n## Два\n### Три", 10);
    let want = [
        "Один",
        "══════════",
        "",
        "текст",
        "дальше",
        "",
        "Два",
        "──────────",
        "",
        "Три",
    ];
    assert_eq!(got, want);
}

#[test]
fn inline_marks() {
    let d = doc("**ж** *к* ~~з~~ `код` ***оба***", 80);
    assert!(find(&d, "ж").mark.bold);
    assert!(find(&d, "к").mark.italic);
    assert!(find(&d, "з").mark.strike);
    assert_eq!(find(&d, "код").mark.tone, Tone::Code);
    assert!(find(&d, "оба").mark.bold && find(&d, "оба").mark.italic);
}

#[test]
fn hard_breaks() {
    assert_eq!(render("один  \nдва\\\nтри", 80), ["один", "два", "три"]);
}

#[test]
fn lists() {
    let text = "- один\n- два\n  - внутри\n    - глубже\n- три\n\nтекст\n\n3. a\n4. b\n";
    let want = [
        "• один",
        "• два",
        "  ◦ внутри",
        "    ▪ глубже",
        "• три",
        "",
        "текст",
        "",
        "3. a",
        "4. b",
    ];
    assert_eq!(render(text, 80), want);
}

#[test]
fn loose_lists_and_item_blocks() {
    let text = "- один\n\n  второй абзац\n\n- два\n";
    assert_eq!(
        render(text, 80),
        ["• один", "", "  второй абзац", "", "• два"]
    );
    let text = "- код:\n  ```\n  x\n  ```\n- дальше\n";
    assert_eq!(
        render(text, 80),
        ["• код:", "  ┌─", "  │ x", "  └─", "• дальше"]
    );
}

#[test]
fn wrapped_items_are_aligned() {
    assert_eq!(render("10. один два три", 12), ["10. один два", "    три"]);
}

#[test]
fn tasks_and_empty_items() {
    let text = "- [ ] нет\n- [x] да\n-\n1. [ ] номер\n";
    let got = render(text, 80);
    assert_eq!(&got[..3], ["☐ нет", "☑ да", "•"]);
    assert_eq!(got.last().unwrap(), "1. ☐ номер");
}

#[test]
fn quotes_and_alerts() {
    let text = "> один\n>\n> > два\n\n> [!WARNING]\n> осторожно\n";
    let want = ["▌ один", "▌", "▌ ▌ два", "", "▌ Warning", "▌ осторожно"];
    assert_eq!(render(text, 80), want);
}

#[test]
fn code_blocks() {
    let got = render("```rust\nfn a() {}\n\n\tb\n```\n\n    отступ\n", 80);
    let want = [
        "┌─ rust",
        "│ fn a() {}",
        "│",
        "│ \tb",
        "└─",
        "",
        "┌─",
        "│ отступ",
        "└─",
    ];
    assert_eq!(got, want);
    assert_eq!(
        render("```\nabcdefgh\n```", 8),
        ["┌─", "│ abcdef", "│ gh", "└─"]
    );
}

#[test]
fn tables() {
    let text = "| имя | год |\n|:--|--:|\n| Анна | 5 |\n| Б | 1999 |\n";
    let want = [
        "┌──────┬──────┐",
        "│ имя  │  год │",
        "├──────┼──────┤",
        "│ Анна │    5 │",
        "│ Б    │ 1999 │",
        "└──────┴──────┘",
    ];
    assert_eq!(render(text, 80), want);
    assert!(find(&doc(text, 80), "имя").mark.bold);
}

#[test]
fn wide_tables_wrap_inside_cells() {
    let text = "| a | описание |\n|---|---|\n| 1 | один два три четыре |\n| 2 | пять |\n";
    let want = [
        "┌───┬──────────┐",
        "│ a │ описание │",
        "├───┼──────────┤",
        "│ 1 │ один два │",
        "│   │ три      │",
        "│   │ четыре   │",
        "├───┼──────────┤",
        "│ 2 │ пять     │",
        "└───┴──────────┘",
    ];
    assert_eq!(render(text, 16), want);
}

#[test]
fn table_lines_have_equal_width_and_fit() {
    let text = "| a | 日本語 | `код` e\u{301} |\n|:-:|--|--|\n| длинноеслово | x | |\n| y |\n";
    for width in 16..60 {
        let lines = render(text, width);
        let widths: Vec<usize> = lines.iter().map(|l| layout::width(l, 4)).collect();
        assert!(
            widths.iter().all(|w| *w == widths[0] && *w <= width),
            "{width}: {lines:#?}"
        );
    }
}

#[test]
fn links() {
    let d = doc(
        "[т](http://a.ru) <http://b.ru> https://c.ru/x_y_z, www.d.ru e@f.ru `http://g.ru` file.md",
        200,
    );
    let url = |t: &str| d.links[find(&d, t).link.unwrap() as usize].as_str();
    assert_eq!(url("т"), "http://a.ru");
    assert_eq!(url("http://b.ru"), "http://b.ru");
    assert_eq!(url("https://c.ru/x_y_z"), "https://c.ru/x_y_z");
    assert_eq!(url("www.d.ru"), "http://www.d.ru");
    assert_eq!(url("e@f.ru"), "mailto:e@f.ru");
    assert_eq!(find(&d, "http://g.ru").link, None);
    assert_eq!(d.links.len(), 5);
}

#[test]
fn link_survives_wrapping() {
    let d = doc("текст https://example.com/aaaaaaaaaaaaaaaaaaaa", 20);
    let linked = d
        .lines
        .iter()
        .flat_map(|l| &l.spans)
        .filter(|s| s.link.is_some());
    let text: String = linked.map(|s| s.text.as_str()).collect();
    assert_eq!(text, "https://example.com/aaaaaaaaaaaaaaaaaaaa");
}

#[test]
fn images_footnotes_html_math_and_metadata() {
    let text = "---\ntitle: x\n---\n\n![кот](cat.png) сноска[^1] <b>html</b> $x^2$\n\n[^1]: Пояснение\n\n<div>\n";
    let want = [
        "title: x",
        "",
        "▣ кот (cat.png) сноска[1] <b>html</b> $x^2$",
        "",
        "[1] Пояснение",
        "",
        "<div>",
    ];
    assert_eq!(render(text, 80), want);
}

#[test]
fn rule_and_empty_document() {
    assert_eq!(render("a\n\n---\n\nb", 5), ["a", "", "─────", "", "b"]);
    assert!(render("", 80).is_empty());
    assert!(render("\n\n", 80).is_empty());
}

#[test]
fn crlf_renders_the_same() {
    let text = "# A\n\nтекст\nещё\n\n- a\n- b\n\n```\nx\ny\n```\n\n| a |\n|---|\n| b |\n";
    assert_eq!(render(text, 40), render(&text.replace('\n', "\r\n"), 40));
}

#[test]
fn source_positions_are_monotonic_and_exact() {
    let text = "# Глава\n\nабзац один два три\n\n- пункт\n\n```\ncode\n```\n\nконец\n";
    let d = doc(text, 12);
    assert!(d.lines.is_sorted_by_key(|l| l.src));
    for word in ["Глава", "абзац", "три", "пункт", "конец"] {
        let line = d.line_at(text.find(word).unwrap());
        assert!(
            shown(&d)[line].contains(word),
            "{word}: {:?}",
            shown(&d)[line]
        );
    }
    assert_eq!(doc("", 10).line_at(5), 0);
}

#[test]
fn any_width_and_odd_input_do_not_panic() {
    let mut text = String::from("> - > 1. [ ] a **b\n\t\tx\n\n| a | b |\n|-|-|\n| 1 |\n\n");
    text += "e\u{301}\u{200b}\u{7}\t日本 [a](b \"c\") ![](x) [^n]\n\n[^n]: > - x\n";
    text += &"> ".repeat(300);
    text += "глубоко\n\n";
    text += &(0..60)
        .map(|i| format!("{}- x\n", "  ".repeat(i)))
        .collect::<String>();
    for width in 0..50 {
        let d = doc(&text, width);
        assert!(d.lines.is_sorted_by_key(|l| l.src));
        for line in &d.lines {
            assert!(line.spans.iter().all(|s| !s.text.contains('\n')));
        }
    }
    for end in (0..text.len())
        .step_by(3)
        .filter(|i| text.is_char_boundary(*i))
    {
        doc(&text[..end], 20);
    }
}
