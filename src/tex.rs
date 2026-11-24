//! Формулы LaTeX в Unicode.
//!
//! Поддерживается распространённая часть языка. Если в формуле встретилось
//! что-то ещё, результата нет и показывается исходный текст: лучше он, чем
//! формула с потерянным смыслом.

use unicode_width::UnicodeWidthStr;

const MAX_LEN: usize = 4000;
const MAX_DEPTH: usize = 40;

/// Строки формулы или `None`, если её нельзя передать в Unicode.
pub fn convert(source: &str) -> Option<Vec<String>> {
    if source.len() > MAX_LEN {
        return None;
    }
    let tokens = tokenize(source)?;
    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
        depth: 0,
    };
    let block = parser.sequence(&[])?;
    if parser.pos != tokens.len() {
        return None;
    }
    let lines: Vec<String> = block
        .lines
        .iter()
        .map(|l| l.trim_end().to_owned())
        .collect();
    lines.iter().any(|l| !l.is_empty()).then_some(lines)
}

/// То же для формулы внутри строки текста.
pub fn convert_inline(source: &str) -> Option<String> {
    let mut lines = convert(source)?;
    (lines.len() == 1).then(|| lines.remove(0).trim().to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Command(String),
    Char(char),
    Open,
    Close,
    Sup,
    Sub,
    Amp,
    Break,
}

fn tokenize(source: &str) -> Option<Vec<Token>> {
    let mut out = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(match c {
            '\\' => match chars.next()? {
                '\\' => Token::Break,
                c if c.is_ascii_alphabetic() => {
                    let mut name = String::from(c);
                    while let Some(c) = chars.next_if(char::is_ascii_alphabetic) {
                        name.push(c);
                    }
                    Token::Command(name)
                }
                c => Token::Command(c.to_string()),
            },
            '{' => Token::Open,
            '}' => Token::Close,
            '^' => Token::Sup,
            '_' => Token::Sub,
            '&' => Token::Amp,
            '%' | '#' => return None,
            c if c.is_whitespace() => Token::Char(' '),
            c if c.is_control() => return None,
            c => Token::Char(c),
        });
    }
    Some(out)
}

/// Прямоугольник текста. Строки одной ширины, `base` стоит на линии формулы.
#[derive(Debug, Clone)]
struct Block {
    lines: Vec<String>,
    base: usize,
}

impl Block {
    fn text(text: impl Into<String>) -> Self {
        Self {
            lines: vec![text.into()],
            base: 0,
        }
    }

    fn single(&self) -> Option<&str> {
        match &self.lines[..] {
            [line] => Some(line),
            _ => None,
        }
    }

    fn width(&self) -> usize {
        self.lines.iter().map(|l| l.width()).max().unwrap_or(0)
    }

    fn append(&mut self, other: &Block) {
        let above = self.base.max(other.base);
        let below = (self.lines.len() - self.base).max(other.lines.len() - other.base);
        let (left, right) = (self.width(), other.width());
        let row = |b: &Block, width: usize, y: usize| {
            let line = (y + b.base).checked_sub(above).and_then(|i| b.lines.get(i));
            let line = line.map_or("", String::as_str);
            format!("{line}{}", " ".repeat(width - line.width()))
        };
        self.lines = (0..above + below)
            .map(|y| row(self, left, y) + &row(other, right, y))
            .collect();
        self.base = above;
    }
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    depth: usize,
}

/// Что завершает последовательность.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Close,
    Bracket,
    Cell,
    Right,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        token
    }

    fn skip_spaces(&mut self) {
        while self.peek() == Some(&Token::Char(' ')) {
            self.pos += 1;
        }
    }

    fn at_stop(&self, stops: &[Stop]) -> bool {
        match self.peek() {
            Some(Token::Close) => stops.contains(&Stop::Close),
            Some(Token::Char(']')) => stops.contains(&Stop::Bracket),
            Some(Token::Amp | Token::Break) => stops.contains(&Stop::Cell),
            Some(Token::Command(c)) if c == "end" => stops.contains(&Stop::Cell),
            Some(Token::Command(c)) if c == "right" => stops.contains(&Stop::Right),
            _ => false,
        }
    }

    fn sequence(&mut self, stops: &[Stop]) -> Option<Block> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return None;
        }
        let mut out = Block::text("");
        // Второй показатель подряд в LaTeX недопустим.
        let (mut has_sup, mut has_sub) = (false, false);
        // После знака суммы или имени функции нужен пробел.
        let mut operator = None;
        while self.pos < self.tokens.len() && !self.at_stop(stops) {
            match self.peek()? {
                Token::Sup if std::mem::replace(&mut has_sup, true) => return None,
                Token::Sub if std::mem::replace(&mut has_sub, true) => return None,
                Token::Sup | Token::Sub | Token::Char(' ') => {}
                token => {
                    (has_sup, has_sub) = (false, false);
                    let bracket = matches!(token, Token::Char('(' | '['));
                    let next = match token {
                        Token::Command(c) if FUNCTIONS.contains(&c.as_str()) => Some(true),
                        Token::Command(c) if OPERATORS.contains(&c.as_str()) => Some(false),
                        _ => None,
                    };
                    let separate = match operator.take() {
                        Some(function) => !(function && bracket),
                        None => next == Some(true) && ends_with_letter(&out),
                    };
                    if separate && !out.lines[out.base].ends_with(' ') {
                        push(&mut out, " ");
                    }
                    operator = next;
                }
            }
            self.element(&mut out)?;
        }
        self.depth -= 1;
        Some(out)
    }

    /// Аргумент команды: группа в скобках или один знак.
    fn argument(&mut self) -> Option<Block> {
        self.skip_spaces();
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return None;
        }
        let block = match self.next()? {
            Token::Open => {
                let block = self.sequence(&[Stop::Close])?;
                (self.next()? == Token::Close).then_some(block)?
            }
            Token::Char(c) => Block::text(letter(c)),
            Token::Command(name) => {
                let mut block = Block::text("");
                self.command(&name, &mut block)?;
                block
            }
            _ => return None,
        };
        self.depth -= 1;
        Some(block)
    }

    fn flat_argument(&mut self) -> Option<String> {
        Some(self.argument()?.single()?.trim().to_owned())
    }

    /// Текст аргумента без разбора, для `\text` и имён окружений.
    fn raw_argument(&mut self) -> Option<String> {
        self.skip_spaces();
        if self.next()? != Token::Open {
            return None;
        }
        let mut out = String::new();
        loop {
            match self.next()? {
                Token::Close => return Some(out),
                Token::Char(c) => out.push(c),
                Token::Command(c) if c == " " || c == "," => out.push(' '),
                Token::Command(c) if matches!(c.as_str(), "_" | "&" | "%" | "$" | "#") => {
                    out.push_str(&c);
                }
                _ => return None,
            }
        }
    }

    fn element(&mut self, out: &mut Block) -> Option<()> {
        match self.next()? {
            Token::Char(' ') => {}
            Token::Char('\'') => push(out, "′"),
            Token::Char('~') => push(out, " "),
            Token::Char(c) if is_relation(c) => spaced(out, &c.to_string()),
            Token::Char(c @ ('+' | '-')) => {
                let sign = if c == '-' { "−" } else { "+" };
                // Знак числа пробелами не окружается.
                let unary = out.single().is_none_or(|l| {
                    let before = l.trim_end().chars().next_back();
                    before.is_none_or(|c| "([{|,=<>≤≥≈≠+−·×→∈∑∫".contains(c))
                });
                if unary {
                    push(out, sign)
                } else {
                    spaced(out, sign)
                }
            }
            Token::Char(',') => push(out, ", "),
            Token::Char(c) => push(out, &letter(c)),
            Token::Open => {
                let block = self.sequence(&[Stop::Close])?;
                (self.next()? == Token::Close).then_some(())?;
                out.append(&block);
            }
            Token::Sup => {
                let text = self.flat_argument()?;
                script(out, &text, '^', superscript)?;
            }
            Token::Sub => {
                let text = self.flat_argument()?;
                script(out, &text, '_', subscript)?;
            }
            Token::Command(name) => self.command(&name, out)?,
            // Строки многострочной формулы без окружения.
            Token::Break => {
                let rest = self.sequence(&[])?;
                let width = out.width().max(rest.width());
                out.lines.extend(rest.lines);
                for line in &mut out.lines {
                    let pad = width - line.width();
                    line.push_str(&" ".repeat(pad));
                }
            }
            Token::Close | Token::Amp => return None,
        }
        Some(())
    }

    fn command(&mut self, name: &str, out: &mut Block) -> Option<()> {
        if let Some(symbol) = symbol(name) {
            match symbol {
                Symbol::Plain(s) => push(out, s),
                Symbol::Spaced(s) => spaced(out, s),
            }
            return Some(());
        }
        if FUNCTIONS.contains(&name) {
            push(out, name);
            return Some(());
        }
        match name {
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let (top, bottom) = (self.flat_argument()?, self.flat_argument()?);
                push(out, &format!("{}/{}", wrap_sum(&top), wrap_factor(&bottom)));
            }
            "binom" | "dbinom" | "tbinom" => {
                let (top, bottom) = (self.flat_argument()?, self.flat_argument()?);
                push(out, &format!("C({top}, {bottom})"));
            }
            "sqrt" => {
                self.skip_spaces();
                let mut root = "√".to_owned();
                if self.peek() == Some(&Token::Char('[')) {
                    self.pos += 1;
                    let index = self.sequence(&[Stop::Bracket])?;
                    (self.next()? == Token::Char(']')).then_some(())?;
                    root = match index.single()?.trim() {
                        "2" => "√".into(),
                        "3" => "∛".into(),
                        "4" => "∜".into(),
                        other => format!("{}√", scripted(other, superscript)?),
                    };
                }
                let text = self.flat_argument()?;
                if is_atom(&text) {
                    push(out, &format!("{root}{text}"));
                } else {
                    push(out, &format!("{root}({text})"));
                }
            }
            "text" | "textrm" | "textbf" | "textit" | "mbox" | "operatorname" => {
                let text = self.raw_argument()?;
                push(out, &text);
            }
            "mathrm" | "mathbf" | "mathit" | "mathsf" | "boldsymbol" | "bm" | "displaystyle"
            | "textstyle" | "limits" | "nolimits" => {
                if name.starts_with("math") || name.starts_with('b') {
                    let block = self.argument()?;
                    out.append(&block);
                }
            }
            "mathbb" | "mathcal" | "mathscr" | "mathfrak" => {
                let text = self.raw_argument()?;
                let table = match name {
                    "mathbb" => double_struck,
                    "mathfrak" => fraktur,
                    _ => script_letter,
                };
                let converted: Option<String> = text.chars().map(table).collect();
                push(out, &converted?);
            }
            "hat" | "widehat" => self.accent(out, '\u{302}')?,
            "bar" => self.accent(out, '\u{304}')?,
            "vec" => self.accent(out, '\u{20d7}')?,
            "dot" => self.accent(out, '\u{307}')?,
            "ddot" => self.accent(out, '\u{308}')?,
            "tilde" | "widetilde" => self.accent(out, '\u{303}')?,
            "overline" => {
                let text = self.flat_argument()?;
                let lined: String = text.chars().flat_map(|c| [c, '\u{305}']).collect();
                push(out, &lined);
            }
            "left" => {
                let open = self.delimiter()?;
                let inner = self.sequence(&[Stop::Right])?;
                (self.next()? == Token::Command("right".into())).then_some(())?;
                let close = self.delimiter()?;
                out.append(&fence(&open, inner, &close));
            }
            "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl" | "Bigr" => {
                let text = self.delimiter()?;
                push(out, &text);
            }
            "not" => {
                let text = self.flat_argument()?;
                let negated = match text.as_str() {
                    "=" => "≠",
                    "∈" => "∉",
                    "⊂" => "⊄",
                    "≡" => "≢",
                    "∃" => "∄",
                    _ => return None,
                };
                spaced(out, negated);
            }
            "pmod" => {
                let text = self.flat_argument()?;
                push(out, &format!(" (mod {text})"));
            }
            "begin" => {
                let block = self.environment()?;
                out.append(&block);
            }
            _ => return None,
        }
        Some(())
    }

    /// Знак над одной буквой.
    fn accent(&mut self, out: &mut Block, mark: char) -> Option<()> {
        let text = self.flat_argument()?;
        if text.chars().count() != 1 {
            return None;
        }
        push(out, &format!("{text}{mark}"));
        Some(())
    }

    fn delimiter(&mut self) -> Option<String> {
        self.skip_spaces();
        match self.next()? {
            Token::Char('.') => Some(String::new()),
            Token::Char(c) => Some(c.to_string()),
            Token::Command(name) => match symbol(&name)? {
                Symbol::Plain(s) | Symbol::Spaced(s) => Some(s.to_owned()),
            },
            _ => None,
        }
    }

    fn environment(&mut self) -> Option<Block> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return None;
        }
        let name = self.raw_argument()?;
        if name == "array" {
            // Описание колонок не нужно.
            self.raw_argument()?;
        }
        let mut rows: Vec<Vec<Block>> = vec![Vec::new()];
        loop {
            let cell = self.sequence(&[Stop::Cell])?;
            rows.last_mut()?.push(cell);
            match self.next()? {
                Token::Amp => {}
                Token::Break => rows.push(Vec::new()),
                Token::Command(_) => break,
                _ => return None,
            }
        }
        if self.raw_argument()? != name {
            return None;
        }
        self.depth -= 1;
        // Перевод строки перед концом окружения пустой строки не создаёт.
        let blank = |row: &Vec<Block>| row.iter().all(|c| c.width() == 0);
        if rows.len() > 1 && rows.last().is_some_and(blank) {
            rows.pop();
        }
        let (open, close, gap) = match name.trim_end_matches('*') {
            "aligned" | "align" | "split" | "gather" | "gathered" | "equation" | "eqnarray" => {
                ("", "", "")
            }
            "matrix" | "array" | "smallmatrix" => ("", "", "  "),
            "pmatrix" => ("(", ")", "  "),
            "bmatrix" => ("[", "]", "  "),
            "Bmatrix" => ("{", "}", "  "),
            "vmatrix" => ("|", "|", "  "),
            "Vmatrix" => ("‖", "‖", "  "),
            "cases" => ("{", "", "  "),
            _ => return None,
        };
        let align_right = matches!(
            name.trim_end_matches('*'),
            "aligned" | "align" | "split" | "eqnarray"
        );
        Some(fence(open, grid(&rows, gap, align_right)?, close))
    }
}

/// Таблица ячеек, каждая в одну строку.
fn grid(rows: &[Vec<Block>], gap: &str, align_right: bool) -> Option<Block> {
    let columns = rows.iter().map(Vec::len).max()?;
    let mut widths = vec![0; columns];
    for row in rows {
        for (c, cell) in row.iter().enumerate() {
            widths[c] = widths[c].max(cell.single()?.trim().width());
        }
    }
    let mut lines = Vec::new();
    for row in rows {
        let mut line = String::new();
        for (c, width) in widths.iter().enumerate() {
            let cell = row.get(c).and_then(Block::single).unwrap_or("");
            // Пробел перед знаком отношения в начале ячейки нужен.
            let text = cell.trim();
            let pad = " ".repeat(width - text.width());
            if c > 0 {
                let relation = align_right && c % 2 == 1;
                line.push_str(if relation { " " } else { gap });
            }
            if align_right && c % 2 == 0 {
                line += &pad;
                line += text;
            } else {
                line += text;
                line += &pad;
            }
        }
        lines.push(line);
    }
    let base = (lines.len() - 1) / 2;
    Some(Block { lines, base })
}

/// Окружает блок скобками по его высоте.
fn fence(open: &str, inner: Block, close: &str) -> Block {
    let height = inner.lines.len();
    let side = |mark: &str| -> Option<Block> {
        if mark.is_empty() {
            return None;
        }
        let parts = match mark {
            "(" => ("⎛", "⎜", "⎝"),
            ")" => ("⎞", "⎟", "⎠"),
            "[" => ("⎡", "⎢", "⎣"),
            "]" => ("⎤", "⎥", "⎦"),
            "{" => ("⎧", "⎪", "⎩"),
            "}" => ("⎫", "⎪", "⎭"),
            other => (other, other, other),
        };
        let lines = (0..height).map(|y| match y {
            _ if height == 1 => mark,
            0 => parts.0,
            y if y + 1 == height => parts.2,
            y if y == inner.base && height > 2 && mark == "{" => "⎨",
            y if y == inner.base && height > 2 && mark == "}" => "⎬",
            _ => parts.1,
        });
        Some(Block {
            lines: lines.map(str::to_owned).collect(),
            base: inner.base,
        })
    };
    let mut out = side(open).unwrap_or(Block {
        lines: vec![String::new(); height],
        base: inner.base,
    });
    let right = side(close);
    out.append(&inner);
    if let Some(right) = right {
        out.append(&right);
    }
    out
}

fn ends_with_letter(block: &Block) -> bool {
    block.lines[block.base].ends_with(char::is_alphanumeric)
}

fn push(out: &mut Block, text: &str) {
    out.append(&Block::text(text));
}

/// Знак с пробелами по сторонам, без лишних в начале.
fn spaced(out: &mut Block, sign: &str) {
    let base = &out.lines[out.base];
    let before = if base.is_empty() || base.ends_with(' ') {
        ""
    } else {
        " "
    };
    push(out, &format!("{before}{sign} "));
}

fn letter(c: char) -> String {
    match c {
        '-' => "−".into(),
        '*' => "∗".into(),
        '\'' => "′".into(),
        c => c.to_string(),
    }
}

fn is_relation(c: char) -> bool {
    matches!(c, '=' | '<' | '>' | '≤' | '≥' | '≠' | '≈' | '∈' | '→')
}

/// Не нуждается в скобках под корнем и в знаменателе.
fn is_atom(text: &str) -> bool {
    let mut chars = text.chars();
    let single = chars.next().is_some() && chars.all(|c| c.width_is_zero());
    single
        || text.chars().all(|c| c.is_ascii_digit() || c == '.')
        || text
            .chars()
            .all(|c| c.is_alphabetic() && c.is_lowercase() && !c.is_ascii())
        || (text.starts_with('(') && text.ends_with(')') && text.matches('(').count() == 1)
}

trait ZeroWidth {
    fn width_is_zero(self) -> bool;
}

impl ZeroWidth for char {
    fn width_is_zero(self) -> bool {
        unicode_width::UnicodeWidthChar::width(self) == Some(0)
    }
}

/// Числитель: скобки нужны только сумме, отношению и дроби.
fn wrap_sum(text: &str) -> String {
    let compound = text.contains([' ', '/', '+', '−']);
    if compound && !is_atom(text) {
        format!("({text})")
    } else {
        text.to_owned()
    }
}

/// Знаменатель: скобки нужны всему, кроме одного знака или числа.
fn wrap_factor(text: &str) -> String {
    if is_atom(text) {
        text.to_owned()
    } else {
        format!("({text})")
    }
}

fn scripted(text: &str, table: fn(char) -> Option<char>) -> Option<String> {
    text.chars().filter(|c| *c != ' ').map(table).collect()
}

/// Индекс или степень. Чего нет в Unicode, пишется через знак и скобки.
fn script(out: &mut Block, text: &str, sign: char, table: fn(char) -> Option<char>) -> Option<()> {
    if text.is_empty() {
        return Some(());
    }
    if text == "′" || text.chars().all(|c| c == '′') {
        push(out, text);
    } else if let Some(converted) = scripted(text, table) {
        push(out, &converted);
    } else if text.chars().count() == 1 {
        push(out, &format!("{sign}{text}"));
    } else {
        push(out, &format!("{sign}({text})"));
    }
    Some(())
}

fn lookup(c: char, from: &str, to: &str) -> Option<char> {
    from.chars()
        .zip(to.chars())
        .find_map(|(a, b)| (a == c).then_some(b))
}

fn superscript(c: char) -> Option<char> {
    lookup(
        c,
        "0123456789+−=()abcdefghijklmnoprstuvwxyzABDEGHIJKLMNOPRTUVWαβγδθ∗",
        "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖʳˢᵗᵘᵛʷˣʸᶻᴬᴮᴰᴱᴳᴴᴵᴶᴷᴸᴹᴺᴼᴾᴿᵀᵁⱽᵂᵅᵝᵞᵟᶿ*",
    )
}

fn subscript(c: char) -> Option<char> {
    lookup(
        c,
        "0123456789+−=()aehijklmnoprstuvxβγρφχ",
        "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐₑₕᵢⱼₖₗₘₙₒₚᵣₛₜᵤᵥₓᵦᵧᵨᵩᵪ",
    )
}

/// Буква математического алфавита, начинающегося с `start`.
fn styled(c: char, start: u32, exceptions: &[(char, char)]) -> Option<char> {
    if let Some((_, styled)) = exceptions.iter().find(|(plain, _)| *plain == c) {
        return Some(*styled);
    }
    match c {
        'A'..='Z' => char::from_u32(start + (c as u32 - 'A' as u32)),
        'a'..='z' => char::from_u32(start + 26 + (c as u32 - 'a' as u32)),
        _ => None,
    }
}

fn double_struck(c: char) -> Option<char> {
    let exceptions = [
        ('C', 'ℂ'),
        ('H', 'ℍ'),
        ('N', 'ℕ'),
        ('P', 'ℙ'),
        ('Q', 'ℚ'),
        ('R', 'ℝ'),
        ('Z', 'ℤ'),
        ('1', '𝟙'),
    ];
    styled(c, 0x1d538, &exceptions)
}

fn script_letter(c: char) -> Option<char> {
    let exceptions = [
        ('B', 'ℬ'),
        ('E', 'ℰ'),
        ('F', 'ℱ'),
        ('H', 'ℋ'),
        ('I', 'ℐ'),
        ('L', 'ℒ'),
        ('M', 'ℳ'),
        ('R', 'ℛ'),
        ('e', 'ℯ'),
        ('g', 'ℊ'),
        ('o', 'ℴ'),
    ];
    styled(c, 0x1d49c, &exceptions)
}

fn fraktur(c: char) -> Option<char> {
    let exceptions = [('C', 'ℭ'), ('H', 'ℌ'), ('I', 'ℑ'), ('R', 'ℜ'), ('Z', 'ℨ')];
    styled(c, 0x1d504, &exceptions)
}

const FUNCTIONS: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh", "tanh",
    "ln", "log", "lg", "exp", "lim", "limsup", "liminf", "max", "min", "sup", "inf", "arg", "det",
    "dim", "ker", "deg", "gcd", "Pr", "hom", "mod", "tg", "ctg", "arctg",
];

const OPERATORS: &[&str] = &[
    "sum", "prod", "coprod", "int", "iint", "iiint", "oint", "bigcup", "bigcap",
];

enum Symbol {
    Plain(&'static str),
    Spaced(&'static str),
}

fn symbol(name: &str) -> Option<Symbol> {
    let plain = |s| Some(Symbol::Plain(s));
    let spaced = |s| Some(Symbol::Spaced(s));
    match name {
        "alpha" => plain("α"),
        "beta" => plain("β"),
        "gamma" => plain("γ"),
        "delta" => plain("δ"),
        "epsilon" => plain("ϵ"),
        "varepsilon" => plain("ε"),
        "zeta" => plain("ζ"),
        "eta" => plain("η"),
        "theta" => plain("θ"),
        "vartheta" => plain("ϑ"),
        "iota" => plain("ι"),
        "kappa" => plain("κ"),
        "lambda" => plain("λ"),
        "mu" => plain("μ"),
        "nu" => plain("ν"),
        "xi" => plain("ξ"),
        "pi" => plain("π"),
        "rho" => plain("ρ"),
        "varrho" => plain("ϱ"),
        "sigma" => plain("σ"),
        "varsigma" => plain("ς"),
        "tau" => plain("τ"),
        "upsilon" => plain("υ"),
        "phi" => plain("ϕ"),
        "varphi" => plain("φ"),
        "chi" => plain("χ"),
        "psi" => plain("ψ"),
        "omega" => plain("ω"),
        "Gamma" => plain("Γ"),
        "Delta" => plain("Δ"),
        "Theta" => plain("Θ"),
        "Lambda" => plain("Λ"),
        "Xi" => plain("Ξ"),
        "Pi" => plain("Π"),
        "Sigma" => plain("Σ"),
        "Upsilon" => plain("Υ"),
        "Phi" => plain("Φ"),
        "Psi" => plain("Ψ"),
        "Omega" => plain("Ω"),
        "sum" => plain("∑"),
        "prod" => plain("∏"),
        "coprod" => plain("∐"),
        "int" => plain("∫"),
        "iint" => plain("∬"),
        "iiint" => plain("∭"),
        "oint" => plain("∮"),
        "bigcup" => plain("⋃"),
        "bigcap" => plain("⋂"),
        "infty" => plain("∞"),
        "partial" => plain("∂"),
        "nabla" => plain("∇"),
        "forall" => plain("∀"),
        "exists" => plain("∃"),
        "nexists" => plain("∄"),
        "emptyset" | "varnothing" => plain("∅"),
        "hbar" => plain("ℏ"),
        "ell" => plain("ℓ"),
        "Re" => plain("ℜ"),
        "Im" => plain("ℑ"),
        "aleph" => plain("ℵ"),
        "angle" => plain("∠"),
        "triangle" => plain("△"),
        "degree" => plain("°"),
        "prime" => plain("′"),
        "neg" | "lnot" => plain("¬"),
        "dots" | "ldots" => plain("…"),
        "cdots" => plain("⋯"),
        "vdots" => plain("⋮"),
        "ddots" => plain("⋱"),
        "langle" => plain("⟨"),
        "rangle" => plain("⟩"),
        "lfloor" => plain("⌊"),
        "rfloor" => plain("⌋"),
        "lceil" => plain("⌈"),
        "rceil" => plain("⌉"),
        "lbrace" | "{" => plain("{"),
        "rbrace" | "}" => plain("}"),
        "lvert" | "rvert" | "vert" => plain("|"),
        "lVert" | "rVert" | "Vert" | "|" => plain("‖"),
        "backslash" => plain("\\"),
        "%" => plain("%"),
        "$" => plain("$"),
        "#" => plain("#"),
        "&" => plain("&"),
        "_" => plain("_"),
        "," | ";" | ":" | " " | ">" => plain(" "),
        "!" => plain(""),
        "quad" => plain("  "),
        "qquad" => plain("    "),
        "top" => plain("⊤"),
        "bot" => plain("⊥"),
        "star" => plain("⋆"),
        "dagger" => plain("†"),
        "circ" => plain("∘"),
        "cdot" => spaced("·"),
        "times" => spaced("×"),
        "div" => spaced("÷"),
        "pm" => spaced("±"),
        "mp" => spaced("∓"),
        "ast" => spaced("∗"),
        "oplus" => spaced("⊕"),
        "ominus" => spaced("⊖"),
        "otimes" => spaced("⊗"),
        "odot" => spaced("⊙"),
        "cup" => spaced("∪"),
        "cap" => spaced("∩"),
        "setminus" => spaced("∖"),
        "land" | "wedge" => spaced("∧"),
        "lor" | "vee" => spaced("∨"),
        "le" | "leq" | "leqslant" => spaced("≤"),
        "ge" | "geq" | "geqslant" => spaced("≥"),
        "ne" | "neq" => spaced("≠"),
        "lt" => spaced("<"),
        "gt" => spaced(">"),
        "ll" => spaced("≪"),
        "gg" => spaced("≫"),
        "approx" => spaced("≈"),
        "equiv" => spaced("≡"),
        "sim" => spaced("∼"),
        "simeq" => spaced("≃"),
        "cong" => spaced("≅"),
        "propto" => spaced("∝"),
        "in" => spaced("∈"),
        "notin" => spaced("∉"),
        "ni" => spaced("∋"),
        "subset" => spaced("⊂"),
        "subseteq" => spaced("⊆"),
        "supset" => spaced("⊃"),
        "supseteq" => spaced("⊇"),
        "perp" => spaced("⟂"),
        "parallel" => spaced("∥"),
        "mid" => spaced("∣"),
        "vdash" => spaced("⊢"),
        "models" => spaced("⊨"),
        "to" | "rightarrow" => spaced("→"),
        "leftarrow" | "gets" => spaced("←"),
        "leftrightarrow" => spaced("↔"),
        "Rightarrow" | "implies" => spaced("⇒"),
        "Leftarrow" => spaced("⇐"),
        "Leftrightarrow" | "iff" => spaced("⇔"),
        "longrightarrow" => spaced("⟶"),
        "mapsto" => spaced("↦"),
        "uparrow" => spaced("↑"),
        "downarrow" => spaced("↓"),
        "colon" => spaced(":"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(source: &str) -> String {
        convert_inline(source).unwrap_or_else(|| panic!("not converted: {source}"))
    }

    #[test]
    fn common_formulas() {
        for (source, want) in [
            ("E = mc^2", "E = mc²"),
            ("x_1 + x_2", "x₁ + x₂"),
            ("a^{n+1}", "aⁿ⁺¹"),
            ("x_{i}^{2}", "xᵢ²"),
            ("-1", "−1"),
            ("x=-1", "x = −1"),
            ("\\alpha+\\beta", "α + β"),
            ("\\frac{1}{2}", "1/2"),
            ("\\frac12", "1/2"),
            ("\\frac{a+b}{c}", "(a + b)/c"),
            ("\\frac{a}{bc}", "a/(bc)"),
            ("\\sqrt{2}", "√2"),
            ("\\sqrt{x+1}", "√(x + 1)"),
            ("\\sqrt[3]{x}", "∛x"),
            ("\\sqrt[n]{x}", "ⁿ√x"),
            ("\\sum_{i=1}^{n} i", "∑ᵢ₌₁ⁿ i"),
            ("\\int_0^\\infty f(x)\\,dx", "∫₀^∞ f(x) dx"),
            ("\\sin^2 x + \\cos^2 x = 1", "sin² x + cos² x = 1"),
            ("\\lim_{x \\to 0} f", "lim_(x → 0) f"),
            ("a \\le b \\ne c", "a ≤ b ≠ c"),
            ("x \\in \\mathbb{R}", "x ∈ ℝ"),
            ("\\vec{v}", "v\u{20d7}"),
            ("f'(x)", "f′(x)"),
            ("\\left( \\frac{a}{b} \\right)^2", "(a/b)²"),
            ("\\text{если } x > 0", "если x > 0"),
            ("\\mathcal{L}", "ℒ"),
            ("e^{i\\pi} + 1 = 0", "e^(iπ) + 1 = 0"),
            ("\\{1, 2\\}", "{1, 2}"),
            ("A \\Rightarrow B", "A ⇒ B"),
            ("n \\log n", "n log n"),
            ("x^q", "x^q"),
            ("\\not= ", "≠"),
        ] {
            assert_eq!(line(source), want, "{source}");
        }
    }

    #[test]
    fn matrices_cases_and_aligned() {
        let got = convert("A = \\begin{pmatrix} 1 & 2 \\\\ 33 & 4 \\end{pmatrix}").unwrap();
        assert_eq!(got, ["A = ⎛1   2⎞", "    ⎝33  4⎠"]);
        let got =
            convert("|x| = \\begin{cases} x & x \\ge 0 \\\\ -x & x < 0 \\end{cases}").unwrap();
        assert_eq!(got, ["|x| = ⎧x   x ≥ 0", "      ⎩−x  x < 0"]);
        let got = convert("\\begin{aligned} a &= b \\\\ cc &= d \\\\ \\end{aligned}").unwrap();
        assert_eq!(got, [" a = b", "cc = d"]);
        let got = convert("a = 1 \\\\ b = 2").unwrap();
        assert_eq!(got, ["a = 1", "b = 2"]);
        assert!(convert_inline("\\begin{matrix} 1 \\\\ 2 \\end{matrix}").is_none());
    }

    #[test]
    fn unknown_things_are_not_guessed() {
        for source in [
            "\\unknowncommand{x}",
            "\\frac{1}",
            "{x",
            "x}",
            "a & b",
            "\\begin{tikzpicture} \\end{tikzpicture}",
            "\\begin{matrix} 1",
            "\\left( x",
            "\\hat{abc}",
            "\\",
            "",
            "  ",
            "x^",
            "x^x^x",
            "x_a^b_c",
            "\\def\\x{1}",
            "\u{1b}[31m",
        ] {
            assert_eq!(convert(source), None, "{source}");
        }
    }

    #[test]
    fn nesting_is_limited() {
        for source in [
            "{".repeat(1900) + &"}".repeat(1900),
            "\\sqrt".repeat(700) + "x",
            "x^".repeat(1500) + "x",
            "\\frac".repeat(700),
            "\\\\".repeat(1500),
            "\\left(".repeat(600),
        ] {
            assert_eq!(convert(&source), None);
        }
    }

    #[test]
    fn rows_have_no_control_characters() {
        let source = "\\begin{bmatrix} a & \\text{б} \\\\ \\frac{x}{y} & \\sqrt{z} \\end{bmatrix}";
        for line in convert(source).unwrap() {
            assert!(!line.contains(char::is_control));
        }
    }
}
