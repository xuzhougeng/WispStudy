//! The Markdown of a model's answer → blocks to draw: headings, paragraphs,
//! lists (nested by indentation, with checkboxes), fenced code, quotes,
//! rules and pipe tables; inline, code spans, emphasis, strike-through and
//! links. The rules follow quiet.md's Markdown reader (github.com/s1dashu/quiet-md,
//! MIT). What does not close (an answer is drawn while it streams) stays as
//! written, and a citation like `[P3 p.5 "…"]` is ordinary text.

use std::ops::Range;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<String>,
}

/// Text with marks: `runs` are (byte length, marks), cover `text` exactly,
/// and neighbours differ.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rich {
    pub text: String,
    pub runs: Vec<(usize, Marks)>,
}

impl Rich {
    pub fn plain(text: &str) -> Rich {
        let mut rich = Rich::default();
        rich.push(text, Marks::default());
        rich
    }

    pub fn push(&mut self, text: &str, marks: Marks) {
        if text.is_empty() { return }
        self.text.push_str(text);
        match self.runs.last_mut() {
            Some((len, last)) if *last == marks => *len += text.len(),
            _ => self.runs.push((text.len(), marks)),
        }
    }

    /// Each run as its byte range in `text`.
    pub fn spans(&self) -> impl Iterator<Item = (Range<usize>, &Marks)> {
        let mut at = 0;
        self.runs.iter().map(move |(len, marks)| {
            at += len;
            (at - len..at, marks)
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    Bullet,
    Number(u32),
    Todo(bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// Lines that followed each other without a blank one, joined by `\n`.
    Paragraph(Rich),
    Heading(u8, Rich),
    Item { marker: Marker, text: Rich, children: Vec<Block> },
    /// `closed` is whether its closing fence was read: an answer still being written ends in one that is not.
    Code { lang: String, text: String, closed: bool },
    Quote(Vec<Block>),
    Rule,
    /// The first row is the header; every row has one cell per column.
    Table { aligns: Vec<Align>, rows: Vec<Vec<Rich>> },
}

pub fn parse(text: &str) -> Vec<Block> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
    blocks(&lines)
}

/// A place in a text drawn from its Markdown: which of its pieces (in the order `texts` gives them), and the byte in it.
pub type Place = (usize, usize);

/// The pieces of a text as they are drawn from its Markdown, in order: what a `Place` counts. One for a paragraph,
/// a heading, an item's own line, a table cell, a block of code.
pub fn texts(text: &str) -> Vec<String> {
    fn walk(blocks: &[Block], out: &mut Vec<String>) {
        for block in blocks {
            match block {
                Block::Paragraph(text) | Block::Heading(_, text) => out.push(text.text.clone()),
                Block::Item { text, children, .. } => {
                    out.push(text.text.clone());
                    walk(children, out);
                }
                Block::Code { text, .. } => out.push(text.clone()),
                Block::Quote(inner) => walk(inner, out),
                Block::Rule => {}
                Block::Table { rows, .. } => out.extend(rows.iter().flatten().map(|cell| cell.text.clone())),
            }
        }
    }
    let mut out = vec![];
    walk(&parse(text), &mut out);
    out
}

/// The words drawn between two places of a text: the pieces a line apart.
pub fn text_between(text: &str, from: Place, to: Place) -> String {
    let pieces = texts(text);
    let mut out = vec![];
    for (at, piece) in pieces.iter().enumerate().take(to.0 + 1).skip(from.0) {
        let cut = |byte: usize| (0..=byte.min(piece.len())).rev().find(|byte| piece.is_char_boundary(*byte)).unwrap_or(0);
        let (start, end) = (if at == from.0 { cut(from.1) } else { 0 }, if at == to.0 { cut(to.1) } else { piece.len() });
        if start < end { out.push(piece[start..end].to_string()) }
    }
    out.join("\n")
}

fn indent_of(line: &str) -> usize {
    let mut n = 0;
    for c in line.chars() {
        match c {
            ' ' => n += 1,
            '\t' => n += 4 - n % 4,
            _ => break,
        }
    }
    n
}

/// `line` without its first `n` columns of indentation.
fn dedent(line: &str, n: usize) -> &str {
    let mut col = 0;
    for (i, c) in line.char_indices() {
        if col >= n { return &line[i..] }
        match c {
            ' ' => col += 1,
            '\t' => col += 4 - col % 4,
            _ => return &line[i..],
        }
    }
    ""
}

/// A list item line: (marker, content column, content).
fn list_item(line: &str) -> Option<(Marker, usize, &str)> {
    let rest = line.trim_start_matches([' ', '\t']);
    let (marker, len) = if let Some(c) = rest.chars().next().filter(|c| matches!(c, '-' | '*' | '+' | '•')) {
        (Marker::Bullet, c.len_utf8())
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 9 { return None }
        let dot = rest[digits..].chars().next().filter(|c| matches!(c, '.' | ')'))?;
        (Marker::Number(rest[..digits].parse().ok()?), digits + dot.len_utf8())
    };
    let content = rest[len..].strip_prefix([' ', '\t'])?;
    // `***` and `- - -` are rules, not items.
    if rule(rest) { return None }
    Some((marker, indent_of(line) + len + 1, content))
}

fn rule(line: &str) -> bool {
    let marks: Vec<char> = line.trim().chars().filter(|c| *c != ' ').collect();
    marks.len() >= 3 && matches!(marks[0], '-' | '*' | '_') && marks.iter().all(|c| *c == marks[0])
}

fn fence(line: &str) -> Option<(char, usize, &str)> {
    let t = line.trim_start();
    let c = t.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let n = t.chars().take_while(|&x| x == c).count();
    (n >= 3).then(|| (c, n, t[n..].trim()))
}

fn todo(s: &str) -> Option<(bool, &str)> {
    [("[ ] ", false), ("[x] ", true), ("[X] ", true)].iter().find_map(|(prefix, done)| s.strip_prefix(prefix).map(|rest| (*done, rest)))
}

fn quote_line(s: &str) -> Option<&str> {
    let rest = s.strip_prefix('>')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

fn blocks(lines: &[&str]) -> Vec<Block> {
    let mut out: Vec<Block> = vec![];
    // Lines of the paragraph being read.
    let mut para: Vec<&str> = vec![];
    fn close(para: &mut Vec<&str>, out: &mut Vec<Block>) {
        if !para.is_empty() { out.push(Block::Paragraph(inline(&para.join("\n")))) }
        para.clear();
    }
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            close(&mut para, &mut out);
            i += 1;
            continue;
        }
        let ind = indent_of(line);
        let body = line.trim();
        // Fenced code, closed or (still streaming) running to the end.
        if let Some((mark, n, info)) = fence(line) {
            close(&mut para, &mut out);
            let mut code = vec![];
            let mut j = i + 1;
            while j < lines.len() {
                if fence(lines[j]).is_some_and(|(mark2, n2, rest)| mark2 == mark && n2 >= n && rest.is_empty()) { break }
                code.push(dedent(lines[j], ind));
                j += 1;
            }
            out.push(Block::Code { lang: info.split_whitespace().next().unwrap_or("").to_string(), text: code.join("\n"), closed: j < lines.len() });
            i = j + 1;
            continue;
        }
        if let Some((marker, col, content)) = list_item(line) {
            close(&mut para, &mut out);
            // The item's own lines: anything indented past its marker.
            let mut sub: Vec<&str> = vec![];
            let mut j = i + 1;
            while j < lines.len() {
                let l = lines[j];
                if l.trim().is_empty() {
                    // A blank line ends the item unless indented content follows.
                    if lines.get(j + 1).is_some_and(|next| !next.trim().is_empty() && indent_of(next) > ind) {
                        j += 1;
                        continue;
                    }
                    break;
                }
                if indent_of(l) <= ind { break }
                sub.push(dedent(l, col.min(indent_of(l))));
                j += 1;
            }
            let (marker, text) = match (todo(content), marker) {
                (Some((done, rest)), _) => (Marker::Todo(done), rest),
                // Items all written `1.` count on from the one before.
                (None, Marker::Number(1)) => match out.last() {
                    Some(Block::Item { marker: Marker::Number(before), .. }) => (Marker::Number(before + 1), content),
                    _ => (marker, content),
                },
                (None, marker) => (marker, content),
            };
            out.push(Block::Item { marker, text: inline(text.trim()), children: blocks(&sub) });
            i = j;
            continue;
        }
        let hashes = body.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && body[hashes..].starts_with([' ', '\t']) {
            close(&mut para, &mut out);
            out.push(Block::Heading(hashes as u8, inline(body[hashes..].trim().trim_end_matches('#').trim_end())));
            i += 1;
            continue;
        }
        if rule(body) {
            close(&mut para, &mut out);
            out.push(Block::Rule);
            i += 1;
            continue;
        }
        if quote_line(body).is_some() {
            close(&mut para, &mut out);
            let mut sub = vec![];
            while let Some(rest) = lines.get(i).and_then(|l| quote_line(l.trim())) {
                sub.push(rest);
                i += 1;
            }
            out.push(Block::Quote(blocks(&sub)));
            continue;
        }
        if let Some((table, used)) = pipe_table(&lines[i..]) {
            close(&mut para, &mut out);
            out.push(table);
            i += used;
            continue;
        }
        para.push(line.trim_end());
        i += 1;
    }
    close(&mut para, &mut out);
    out
}

fn table_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').filter(|x| !x.ends_with('\\')).unwrap_or(t);
    let mut cells = vec![];
    let mut cell = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cell.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cell).trim().to_string()),
            _ => cell.push(c),
        }
    }
    cells.push(cell.trim().to_string());
    cells
}

/// A pipe table at the start of `lines`: the table, and the lines it took.
pub fn pipe_table(lines: &[&str]) -> Option<(Block, usize)> {
    let (head, sep) = (lines.first()?, lines.get(1)?);
    if !head.contains('|') || !sep.contains('-') { return None }
    let seps = table_cells(sep);
    if !seps.iter().all(|cell| !cell.trim_matches(':').is_empty() && cell.trim_matches(':').chars().all(|c| c == '-')) { return None }
    let header = table_cells(head);
    if header.len() != seps.len() { return None }
    let aligns = seps.iter().map(|cell| match (cell.starts_with(':'), cell.ends_with(':')) {
        (true, true) => Align::Center,
        (false, true) => Align::Right,
        _ => Align::Left,
    });
    let mut used = 2;
    let mut rows = vec![header];
    while let Some(line) = lines.get(used).filter(|l| !l.trim().is_empty() && l.contains('|')) {
        rows.push(table_cells(line));
        used += 1;
    }
    let columns = seps.len();
    // `<br>` is how a cell holds two lines.
    let rows = rows.iter().map(|row| (0..columns).map(|c| inline(&row.get(c).map_or(String::new(), |cell| cell.replace("<br>", "\n").replace("<br/>", "\n")))).collect()).collect();
    Some((Block::Table { aligns: aligns.collect(), rows }, used))
}

/// How many of `lines` a pipe table at their start takes: its header, the line of dashes under it and its rows;
/// 0 when they do not start with one. For whoever reads a text line by line and must keep a table's lines together.
pub fn table_lines(lines: &[&str]) -> usize {
    pipe_table(lines).map_or(0, |(_, used)| used)
}

/// How wide a text is at its longest line, in letters: a letter of Chinese counts as two of Latin.
pub fn width(text: &str) -> usize {
    text.lines().map(|line| line.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum::<usize>()).max().unwrap_or(0)
}

/// The share of a table's width each of its columns gets, adding up to 1: by the widest of its cells (`width`
/// of each, row by row), so a column of labels is narrow and one of sentences wide. No column is counted
/// shorter than `SHORT` or longer than `LONG`: a label stays on one line beside a long sentence, and one very
/// long cell does not take the table. The window draws a table so, and a Word file has it so.
pub fn column_shares(rows: &[Vec<usize>]) -> Vec<f32> {
    const SHORT: usize = 4;
    const LONG: usize = 30;
    // The room a cell's padding takes, in letters: what keeps a narrow column from being all padding.
    const PADDING: usize = 3;
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns).map(|c| rows.iter().filter_map(|row| row.get(c)).copied().max().unwrap_or(0).clamp(SHORT, LONG) + PADDING).collect();
    let all: usize = widths.iter().sum();
    widths.iter().map(|width| *width as f32 / all as f32).collect()
}

fn is_boundary(c: Option<char>) -> bool {
    c.is_none_or(|c| c.is_whitespace() || c.is_ascii_punctuation() || !c.is_ascii())
}

fn link_ok(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ["http://", "https://", "mailto:"].iter().any(|scheme| lower.starts_with(scheme))
}

/// Inline Markdown: escapes, code spans, links, bare URLs and emphasis.
/// The text of the pieces `pieces` finds, without their syntax.
pub fn inline(s: &str) -> Rich {
    let mut out = Rich::default();
    let mut pieces = vec![];
    pieces_of(s, 0, &Marks::default(), &mut pieces, &mut vec![]);
    for piece in pieces.into_iter().filter(|piece| piece.part != Part::Syntax) {
        out.push(&s[piece.range], piece.marks);
    }
    out
}

/// What a piece of a written line is: words, or the Markdown around them
/// (`**`, `# `, a link's address), or a list's bullet (`-`, drawn as a dot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Text,
    Syntax,
    Bullet,
}

/// A stretch of a written text, in bytes of the whole text, with its marks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub range: Range<usize>,
    pub marks: Marks,
    pub part: Part,
    /// A Pandoc citation, `[@Zhang2024, p. 5]`.
    pub cite: bool,
}

/// One line of a text being written, as an editor draws it: what kind of
/// line it is, and its pieces, which cover it exactly and in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    /// In bytes of the text, without the line break.
    pub range: Range<usize>,
    /// 1 to 6 for `#` to `######`, else 0.
    pub heading: u8,
    /// How many `>` it opens with.
    pub quote: u8,
    pub item: Option<Marker>,
    /// `---`, `***`, `___`.
    pub rule: bool,
    /// A line of fenced code, or a fence.
    pub code: bool,
    /// The line is a picture, `![caption](figures/G1.png)`: where its address is, in bytes of the text.
    /// Its words are the caption's; one without a caption says nothing.
    pub picture: Option<Range<usize>>,
    pub pieces: Vec<Piece>,
}

/// A text line by line, the way an editor that keeps the Markdown as
/// written and draws it formatted reads it. Each line stands alone but for
/// fenced code, which runs to its closing fence (or the end).
pub fn lines(text: &str) -> Vec<Line> {
    lines_in(text, &mut Read::default())
}

/// What `lines` reads beside the lines, for changing the text (`emphasize`): the spans of emphasis, and
/// where each line's inline text is (`None` for code or a fence; a rule's is the whole of it).
#[derive(Default)]
struct Read {
    spans: Vec<Span>,
    inline: Vec<Option<Range<usize>>>,
}

/// A span of emphasis as the reader closed it: its opening and closing marks, in bytes of the text.
/// One mark is italic, two bold, three both.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Span {
    open: Range<usize>,
    close: Range<usize>,
}

impl Span {
    /// Whether it is bold (`k` 2) or italic (`k` 1).
    fn has(&self, k: usize) -> bool {
        self.open.len() == k || self.open.len() == 3
    }

    fn inner(&self) -> Range<usize> {
        self.open.end..self.close.start
    }
}

fn lines_in(text: &str, read: &mut Read) -> Vec<Line> {
    let mut out = vec![];
    // The fence that is open: its character and length.
    let mut fenced: Option<(char, usize)> = None;
    let mut start = 0;
    for raw in text.split('\n') {
        let range = start..start + raw.len();
        start = range.end + 1;
        let mut line = Line { range: range.clone(), ..Line::default() };
        let piece = |range: Range<usize>, part: Part| Piece { range, marks: Marks::default(), part, cite: false };
        if let Some((mark, n)) = fenced {
            line.code = true;
            if fence(raw).is_some_and(|(mark2, n2, rest)| mark2 == mark && n2 >= n && rest.is_empty()) {
                fenced = None;
                line.pieces.push(piece(range, Part::Syntax));
            } else if !raw.is_empty() {
                line.pieces.push(Piece { marks: Marks { code: true, ..Marks::default() }, ..piece(range, Part::Text) });
            }
            out.push(line);
            read.inline.push(None);
            continue;
        }
        if let Some((mark, n, _)) = fence(raw) {
            fenced = Some((mark, n));
            line.code = true;
            line.pieces.push(piece(range, Part::Syntax));
            out.push(line);
            read.inline.push(None);
            continue;
        }
        let mut at = 0;
        let syntax = |line: &mut Line, from: usize, to: usize, part: Part| {
            if to > from { line.pieces.push(piece(range.start + from..range.start + to, part)) }
        };
        // A picture has the line to itself: its caption is the line's words.
        if let Some((caption, address)) = picture_line(raw) {
            line.picture = Some(range.start + address.start..range.start + address.end);
            // Two pieces of syntax even around no caption: between them is where one is written.
            syntax(&mut line, 0, caption.start, Part::Syntax);
            pieces_of(&raw[caption.clone()], range.start + caption.start, &Marks::default(), &mut line.pieces, &mut read.spans);
            syntax(&mut line, caption.end, raw.len(), Part::Syntax);
            out.push(line);
            read.inline.push(Some(range.start + caption.start..range.start + caption.end));
            continue;
        }
        // Indentation stays as written: it is how far a list is nested.
        let indent = raw.len() - raw.trim_start_matches([' ', '\t']).len();
        if indent > 0 && indent < raw.len() {
            line.pieces.push(piece(range.start..range.start + indent, Part::Text));
            at = indent;
        }
        while raw[at..].starts_with('>') {
            let to = at + 1 + usize::from(raw[at + 1..].starts_with(' '));
            syntax(&mut line, at, to, Part::Syntax);
            line.quote += 1;
            at = to;
        }
        let lead = at;
        let rest = &raw[at..];
        let hashes = rest.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && rest[hashes..].starts_with([' ', '\t']) {
            let to = at + hashes + (rest[hashes..].len() - rest[hashes..].trim_start_matches([' ', '\t']).len());
            syntax(&mut line, at, to, Part::Syntax);
            line.heading = hashes as u8;
            at = to;
        } else if rule(rest) {
            syntax(&mut line, at, raw.len(), Part::Syntax);
            line.rule = true;
            at = raw.len();
        } else if let Some((marker, _, content)) = list_item(rest) {
            let mark = rest.len() - content.len();
            match marker {
                Marker::Bullet => {
                    let bullet = rest.chars().next().map_or(1, char::len_utf8);
                    // The space after the bullet stays, between the dot and the words.
                    syntax(&mut line, at, at + bullet, Part::Bullet);
                    syntax(&mut line, at + bullet, at + mark, Part::Text);
                }
                _ => syntax(&mut line, at, at + mark, Part::Text),
            }
            line.item = Some(match todo(content) {
                Some((done, _)) => Marker::Todo(done),
                None => marker,
            });
            at += mark;
        }
        pieces_of(&raw[at..], range.start + at, &Marks::default(), &mut line.pieces, &mut read.spans);
        // A task's box is not among its words. A rule has none, but the caret may be in it: `****` is also an empty pair of bold.
        let words = match line.item {
            Some(Marker::Todo(_)) => raw.len() - todo(&raw[at..]).map_or(raw.len() - at, |(_, after)| after.len()),
            _ if line.rule => lead,
            _ => at,
        };
        read.inline.push(Some(range.start + words..range.end));
        out.push(line);
    }
    out
}

/// A text without its Markdown: what `lines` reads as words, each line as
/// it is drawn (a bullet as `•`, a rule as nothing).
pub fn plain(text: &str) -> String {
    lines(text).iter().map(|line| plain_line(text, line)).collect::<Vec<_>>().join("\n")
}

/// One line of `text` without its Markdown, as `plain` says it.
pub fn plain_line(text: &str, line: &Line) -> String {
    line.pieces
        .iter()
        .map(|piece| match piece.part {
            Part::Text => &text[piece.range.clone()],
            Part::Bullet => "•",
            // Code is kept whole, fences and all.
            Part::Syntax if line.code && !line.rule => &text[piece.range.clone()],
            Part::Syntax => "",
        })
        .collect()
}

/// A line that is a picture and nothing else, `![caption](address)` or
/// `![caption](address "title")`: where the caption and the address are in
/// it, in bytes. The caption may hold brackets of its own (a citation); the
/// address has no blank and no bracket.
pub fn picture_line(line: &str) -> Option<(Range<usize>, Range<usize>)> {
    let lead = line.len() - line.trim_start().len();
    let body = line.trim();
    if !body.starts_with("![") || !body.ends_with(')') { return None }
    let mid = body.rfind("](")?;
    let caption = &body[2..mid.max(2)];
    if mid < 2 || caption.contains("](") { return None }
    let inner = &body[mid + 2..body.len() - 1];
    let written = inner.split_whitespace().next()?;
    let address = written.trim_matches(['<', '>']);
    if address.is_empty() || address.contains(['(', ')', '[', ']']) { return None }
    // What follows the address is its title, in quotes, or nothing.
    let after = inner[inner.find(written)? + written.len()..].trim();
    if !after.is_empty() && !(after.len() >= 2 && after.starts_with(['"', '\'']) && after.ends_with(['"', '\''])) { return None }
    let start = lead + mid + 2 + inner.find(address)?;
    Some((lead + 2..lead + mid, start..start + address.len()))
}

/// A change an editor makes to its text: `text` in place of `range`, then `select` (bytes of the changed text)
/// selected. With `range` empty and no `text`, only the caret moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
    pub select: Range<usize>,
}

/// Bold (`strong`) or italic for `sel` of `text`, as ⌘B and ⌘I do in an editor that keeps the Markdown as written.
/// Words that all have it lose it: the span around them is taken away, or cut where only part of it was selected;
/// otherwise the words get it, a span partly selected taken in whole and the spans of the kind inside folded into it.
/// The marks go around the words, never around the spaces at the ends, and each line of a selection gets its own.
/// With nothing selected the caret, before a span's closing mark, steps out of it, so what is typed next is plain;
/// right after a span or before its opening mark it steps in; inside one the span loses it; anywhere else an empty
/// pair is put in with the caret between, and taken away again by the next press. `None`: nothing to do (code).
pub fn emphasize(text: &str, sel: Range<usize>, strong: bool) -> Option<Edit> {
    let k = if strong { 2 } else { 1 };
    let mut read = Read::default();
    let lines = lines_in(text, &mut read);
    // The spans of the kind that no span of the kind holds.
    let kind: Vec<&Span> = read.spans.iter().filter(|span| span.has(k)).collect();
    let outer: Vec<&Span> = kind.iter().copied().filter(|span| !kind.iter().any(|o| o != span && o.open.start <= span.open.start && span.close.end <= o.close.end)).collect();
    let mark = |n: usize| "*".repeat(n);
    let moved = |at: usize| Some(Edit { range: at..at, text: String::new(), select: at..at });
    if sel.is_empty() {
        let at = sel.start;
        let (_, region) = lines.iter().zip(&read.inline).find(|(line, _)| line.range.start <= at && at <= line.range.end)?;
        let region = region.clone().filter(|region| region.start <= at && at <= region.end)?;
        let bytes = text.as_bytes();
        let stars = |range: Range<usize>| bytes[range].iter().all(|b| matches!(b, b'*' | b'_'));
        // An empty pair of the kind, as a press put in (`**|**`, or `***|***`, bold and italic): taken away. A span's
        // marks are never one.
        let before = text[region.start..at].bytes().rev().take_while(|b| *b == b'*').count();
        let after = text[at..region.end].bytes().take_while(|b| *b == b'*').count();
        let marking = read.spans.iter().any(|span| (span.open.start < at + after && at - before < span.open.end) || (span.close.start < at + after && at - before < span.close.end));
        if before == after && (before == k || before == 3) && !marking { return Some(Edit { range: at - k..at + k, text: String::new(), select: at - k..at - k }) }
        // Before the closing mark of a span of the kind, or of spans that close in the same run: out past its own.
        if let Some(span) = kind.iter().find(|span| span.inner().start < at && at <= span.close.start && stars(at..span.close.start)) {
            return moved(span.close.end - span.open.len() + k);
        }
        // Right after one, or before its opening mark: in, past the marks of the kind.
        if let Some(span) = kind.iter().find(|span| span.close.end == at) { return moved(span.close.end - k) }
        if let Some(span) = kind.iter().find(|span| span.open.start == at) { return moved(span.open.start + k) }
        // Inside one: it loses the mark.
        if let Some(span) = outer.iter().find(|span| span.open.end <= at && at <= span.close.start) {
            let edits = vec![(span.open.start..span.open.start + k, String::new()), (span.close.end - k..span.close.end, String::new())];
            return applied(text, edits, at..at);
        }
        return Some(Edit { range: at..at, text: mark(2 * k), select: at + k..at + k });
    }
    // The selection on each line it reaches: never cutting a mark, a citation or code, and without the spaces at its ends.
    let mut parts = vec![];
    for (line, region) in lines.iter().zip(&read.inline) {
        let Some(region) = region.as_ref().filter(|_| !line.rule) else { continue };
        let (mut s, mut e) = (sel.start.max(region.start), sel.end.min(region.end));
        if s >= e { continue }
        let whole = |p: &Piece| p.part == Part::Syntax || p.cite || p.marks.code;
        for (n, p) in line.pieces.iter().enumerate() {
            if whole(p) && p.range.start < s && s < p.range.end { s = p.range.start }
            if whole(p) && p.range.start < e && e < p.range.end { e = p.range.end }
            // Code is taken with its backticks.
            if p.marks.code && p.range.start == s && n > 0 && line.pieces[n - 1].part == Part::Syntax && line.pieces[n - 1].range.end == s { s = line.pieces[n - 1].range.start }
            if p.marks.code && p.range.end == e && line.pieces.get(n + 1).is_some_and(|next| next.part == Part::Syntax && next.range.start == e) { e = line.pieces[n + 1].range.end }
        }
        s += text[s..e].len() - text[s..e].trim_start().len();
        e -= text[s..e].len() - text[s..e].trim_end().len();
        if s < e { parts.push((line, s, e)) }
    }
    let (first, last) = (parts.first()?.1, parts.last()?.2);
    let has = |p: &Piece| if strong { p.marks.bold } else { p.marks.italic };
    let all = parts.iter().all(|(line, s, e)| line.pieces.iter().filter(|p| p.part == Part::Text && p.range.start < *e && p.range.end > *s).all(|p| has(p) || text[p.range.start.max(*s)..p.range.end.min(*e)].trim().is_empty()));
    let mut edits = vec![];
    if all {
        for &(_, s, e) in &parts {
            for span in outer.iter().filter(|span| span.inner().start < e && span.inner().end > s) {
                let ch = &text[span.open.start..span.open.start + 1];
                let (n, inner) = (span.open.len(), span.inner());
                let (full, rest) = (ch.repeat(n), ch.repeat(n - k));
                let (a, b) = (s.max(inner.start), e.min(inner.end));
                // What stays of the span on each side keeps all its marks; the words taken out keep the other kind's.
                let left = inner.start + text[inner.start..a].trim_end().len();
                let right = b + (text[b..inner.end].len() - text[b..inner.end].trim_start().len());
                if left > inner.start {
                    edits.push((left..left, full.clone()));
                    edits.push((a..a, rest.clone()));
                } else {
                    edits.push((span.open.clone(), rest.clone()));
                }
                if right < inner.end {
                    edits.push((b..b, rest));
                    edits.push((right..right, full));
                } else {
                    edits.push((span.close.clone(), rest));
                }
            }
        }
        return applied(text, edits, first..last);
    }
    let (mut from, mut to) = (first, last);
    for &(_, s0, e0) in &parts {
        let (mut s, mut e) = (s0, e0);
        // A span of the kind partly selected is taken in whole.
        loop {
            let (s1, e1) = (s, e);
            for span in &kind {
                if span.open.start < s && s < span.close.end { s = span.open.start }
                if span.open.start < e && e < span.close.end { e = span.close.end }
            }
            if (s, e) == (s1, e1) { break }
        }
        (from, to) = (from.min(s), to.max(e));
        edits.push((s..s, mark(k)));
        for span in kind.iter().filter(|span| s <= span.open.start && span.close.end <= e) {
            edits.push((span.open.start..span.open.start + k, String::new()));
            edits.push((span.close.end - k..span.close.end, String::new()));
        }
        edits.push((e..e, mark(k)));
    }
    applied(text, edits, from..to)
}

/// `edits` (in bytes of `text`, none overlapping another) made as one change; `select` is where the selection
/// was, its start kept after what is put in where it stands and its end before.
fn applied(text: &str, mut edits: Vec<(Range<usize>, String)>, select: Range<usize>) -> Option<Edit> {
    edits.retain(|(range, put)| !(range.is_empty() && put.is_empty()));
    edits.sort_by_key(|(range, _)| range.start);
    let from = edits.iter().map(|(range, _)| range.start).min()?;
    let to = edits.iter().map(|(range, _)| range.end).max()?;
    let mut out = String::new();
    let mut at = from;
    for (range, put) in &edits {
        if range.start < at { return None }
        out.push_str(&text[at..range.start]);
        out.push_str(put);
        at = range.end;
    }
    out.push_str(&text[at..to]);
    let shift = |x: usize, after: bool| {
        let mut delta = 0isize;
        for (range, put) in &edits {
            if range.end < x || (range.end == x && range.start < x) {
                delta += put.len() as isize - range.len() as isize;
            } else if range.is_empty() && range.start == x {
                if after { delta += put.len() as isize }
            } else if range.start < x && x < range.end {
                return (range.start as isize + delta) as usize + if after { put.len() } else { 0 };
            }
        }
        (x as isize + delta) as usize
    };
    let (start, end) = (shift(select.start, true), shift(select.end, false));
    Some(Edit { range: from..to, text: out, select: start..end.max(start) })
}

/// What Enter goes on with in a line of a list or a quote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Continued {
    /// What the next line opens with: the indentation, the quote, the next bullet or number.
    pub prefix: String,
    /// The line is its marker alone: Enter ends the list instead.
    pub empty: bool,
    /// How many bytes of the line its marker takes, indentation and quote included.
    pub marker: usize,
}

/// How the line after `line` opens when Enter is pressed at its end, if it
/// is an item or a quote: `- ` after `- a`, `3. ` after `2. b`, `- [ ] ` after a task.
pub fn continued(line: &str) -> Option<Continued> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let mut at = indent;
    while line[at..].starts_with('>') {
        at += 1 + usize::from(line[at + 1..].starts_with(' '));
    }
    let quoted = at > indent;
    let rest = &line[at..];
    let prefix = |marker: &str| format!("{}{}", &line[..at], marker);
    if let Some((marker, _, content)) = list_item(rest) {
        let mark = rest.len() - content.len();
        let (next, content) = match (marker, todo(content)) {
            (_, Some((_, after))) => (prefix(&format!("{}[ ] ", &rest[..mark])), after),
            (Marker::Number(n), None) => {
                let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                (prefix(&format!("{}{} ", n + 1, &rest[digits..digits + 1])), content)
            }
            _ => (prefix(&rest[..mark]), content),
        };
        return Some(Continued { prefix: next, empty: content.trim().is_empty(), marker: at + rest.len() - content.len() });
    }
    quoted.then(|| Continued { prefix: prefix(""), empty: rest.trim().is_empty(), marker: at })
}

/// The pieces of inline Markdown `s`, which starts at byte `base` of the
/// whole text, with `marks` on all of them; pushed onto `out`.
fn pieces_of(s: &str, base: usize, marks: &Marks, out: &mut Vec<Piece>, spans: &mut Vec<Span>) {
    let push = |out: &mut Vec<Piece>, range: Range<usize>, marks: Marks, part: Part, cite: bool| {
        if range.is_empty() { return }
        let range = base + range.start..base + range.end;
        // Words beside words with the same marks are one piece; a citation is always one of its own, so two written together stay two.
        if let Some(last) = out.last_mut()
            && part == Part::Text
            && last.part == Part::Text
            && last.range.end == range.start
            && last.marks == marks
            && !cite
            && !last.cite
        {
            return last.range.end = range.end;
        }
        out.push(Piece { range, marks, part, cite });
    };
    let text = |out: &mut Vec<Piece>, range: Range<usize>| push(out, range, marks.clone(), Part::Text, false);
    let syntax = |out: &mut Vec<Piece>, range: Range<usize>| push(out, range, marks.clone(), Part::Syntax, false);
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next().unwrap();
        let prev = s[..i].chars().next_back();
        // `\*`, `\_` …: the character itself.
        if c == '\\' && let Some(next) = rest[1..].chars().next().filter(char::is_ascii_punctuation) {
            syntax(out, i..i + 1);
            text(out, i + 1..i + 1 + next.len_utf8());
            i += 1 + next.len_utf8();
            continue;
        }
        if c == '`' {
            let n = rest.bytes().take_while(|&b| b == b'`').count();
            let delim = &rest[..n];
            if let Some(end) = find_run(&rest[n..], delim, b'`') {
                let inner = &rest[n..n + end];
                // One space inside each end is the code span's, not the code's.
                let pad = usize::from(inner.len() > 2 && inner.starts_with(' ') && inner.ends_with(' '));
                syntax(out, i..i + n + pad);
                push(out, i + n + pad..i + n + end - pad, Marks { code: true, ..marks.clone() }, Part::Text, false);
                syntax(out, i + n + end - pad..i + n + end + n);
                i += n + end + n;
                continue;
            }
            text(out, i..i + n);
            i += n;
            continue;
        }
        if c == '[' && let Some((label, url, used)) = link_at(rest) {
            let linked = Marks { link: Some(url.to_string()), ..marks.clone() };
            if label.is_empty() {
                // `[](url)` shows the address.
                let from = url.as_ptr() as usize - rest.as_ptr() as usize;
                syntax(out, i..i + from);
                push(out, i + from..i + from + url.len(), linked, Part::Text, false);
                syntax(out, i + from + url.len()..i + used);
            } else {
                syntax(out, i..i + 1);
                pieces_of(label, base + i + 1, &linked, out, spans);
                syntax(out, i + 1 + label.len()..i + used);
            }
            i += used;
            continue;
        }
        // `[@Key]`, `[@A; @B, p. 5]`, `[-@Key]`: a citation, as it is written.
        if c == '[' && (rest.starts_with("[@") || rest.starts_with("[-@")) && let Some(end) = rest.find(']').filter(|end| !rest[1..*end].contains('[')) {
            push(out, i..i + end + 1, marks.clone(), Part::Text, true);
            i += end + 1;
            continue;
        }
        // A bare URL is a link to itself.
        if c == 'h' && !prev.is_some_and(|p| p.is_alphanumeric()) && (rest.starts_with("https://") || rest.starts_with("http://")) {
            let end = rest.find(|c: char| c.is_whitespace() || "<>\"'`".contains(c) || !c.is_ascii()).unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(|c: char| ".,;:!?)]}*_~".contains(c));
            if url.split_once("://").is_some_and(|(_, host)| host.len() > 3 && host.contains('.')) {
                push(out, i..i + url.len(), Marks { link: Some(url.to_string()), ..marks.clone() }, Part::Text, false);
                i += url.len();
                continue;
            }
        }
        if matches!(c, '*' | '_' | '~') {
            let run = rest.bytes().take_while(|&b| b == c as u8).count();
            // Three that no three close open two spans, one inside the other: `***a* b**` is bold around an
            // italic, `***a** b*` italic around a bold.
            let tries: &[usize] = if run == 3 && c != '~' { &[3, 2, 1] } else { &[run] };
            // `snake_case` is not emphasis: `_` counts only at the edge of a word.
            let bounded = c == '_';
            let mut done = false;
            for &n in tries {
                let set: Option<fn(&mut Marks)> = match (c, n) {
                    ('*' | '_', 3) => Some(|m| (m.bold, m.italic) = (true, true)),
                    ('*' | '_', 2) => Some(|m| m.bold = true),
                    ('*' | '_', 1) => Some(|m| m.italic = true),
                    ('~', 2) => Some(|m| m.strike = true),
                    _ => None,
                };
                if let Some(set) = set
                    && (!bounded || is_boundary(prev))
                    && let Some(end) = find_close(&rest[n..], c, n)
                {
                    let inner = &rest[n..n + end];
                    let after = rest[n + end + n..].chars().next();
                    if !inner.starts_with(char::is_whitespace) && !inner.ends_with(char::is_whitespace) && (!bounded || is_boundary(after)) {
                        let mut inside = marks.clone();
                        set(&mut inside);
                        syntax(out, i..i + n);
                        pieces_of(inner, base + i + n, &inside, out, spans);
                        syntax(out, i + n + end..i + n + end + n);
                        if c != '~' { spans.push(Span { open: base + i..base + i + n, close: base + i + n + end..base + i + n + end + n }) }
                        i += n + end + n;
                        done = true;
                        break;
                    }
                }
            }
            if !done {
                text(out, i..i + run);
                i += run;
            }
            continue;
        }
        text(out, i..i + c.len_utf8());
        i += c.len_utf8();
    }
}

/// Where in `s` a run of `ch` exactly as long as `delim` stands (not part of
/// a longer run, and not at the very start: what it closes is not empty).
fn find_run(s: &str, delim: &str, ch: u8) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut from = 0;
    while let Some(found) = s[from..].find(delim) {
        let at = from + found;
        if at > 0 && bytes[at - 1] != ch && bytes.get(at + delim.len()) != Some(&ch) { return Some(at) }
        from = at + delim.len().max(1);
        while bytes.get(from) == Some(&ch) { from += 1 }
    }
    None
}

/// Where in `s` the emphasis opened by `n` of `mark` closes. Runs between
/// that open and close spans of their own are stepped over, so the `***` at
/// the end of `**a *b***` closes the inner span with its first star and the
/// outer one with the other two.
fn find_close(s: &str, mark: char, n: usize) -> Option<usize> {
    // The lengths of the runs opened inside and not yet closed.
    let mut open: Vec<usize> = vec![];
    let mut at = 0;
    while let Some(found) = s[at..].find(mark) {
        let start = at + found;
        let len = s[start..].chars().take_while(|c| *c == mark).count();
        at = start + len;
        let can_close = s[..start].chars().next_back().is_some_and(|c| !c.is_whitespace());
        let can_open = s[at..].chars().next().is_some_and(|c| !c.is_whitespace());
        let (mut left, mut pos) = (len, start);
        if can_close {
            while let Some(&inner) = open.last().filter(|inner| **inner <= left) {
                open.pop();
                left -= inner;
                pos += inner;
            }
            if open.is_empty() && left == n { return Some(pos) }
            // A longer run closes with as many as opened, the rest opening anew (`*a***b**`), as CommonMark reads it:
            // not where the run could open as well and the two runs come to a multiple of three (`*a**b**c*`).
            if open.is_empty() && left > n && (!can_open || !(n + left).is_multiple_of(3)) { return Some(pos) }
        }
        if left > 0 && can_open { open.push(left) }
    }
    None
}

/// `[label](url "title")` at the start of `s`: (label, url, bytes used).
fn link_at(s: &str) -> Option<(&str, &str, usize)> {
    let mut depth = 0;
    let mut close = None;
    for (at, byte) in s.bytes().enumerate() {
        match byte {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(at);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let inner = s[close + 1..].strip_prefix('(')?;
    let end = inner.find(')')?;
    let url = inner[..end].split_whitespace().next().unwrap_or("").trim_matches(['<', '>']);
    link_ok(url).then_some((&s[1..close], url, close + 2 + end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold() -> Marks {
        Marks { bold: true, ..Marks::default() }
    }

    #[test]
    fn inline_marks() {
        let rich = inline("a **b** `c*d` ~~e~~ *f*");
        assert_eq!(rich.text, "a b c*d e f");
        let marks: Vec<(&str, &Marks)> = rich.spans().map(|(range, marks)| (&rich.text[range], marks)).collect();
        assert_eq!(marks[1], ("b", &bold()));
        assert_eq!(marks[3], ("c*d", &Marks { code: true, ..Marks::default() }));
        assert_eq!(marks[5], ("e", &Marks { strike: true, ..Marks::default() }));
        assert_eq!(marks[7], ("f", &Marks { italic: true, ..Marks::default() }));
        // A span inside a span, both closed by one run of stars.
        let nested = inline("**寄生植物 *Phtheirospermum japonicum*** 和 *a **b** c*");
        assert_eq!(nested.text, "寄生植物 Phtheirospermum japonicum 和 a b c");
        assert_eq!(nested.runs[0], ("寄生植物 ".len(), bold()));
        assert_eq!(nested.runs[1], ("Phtheirospermum japonicum".len(), Marks { bold: true, italic: true, ..Marks::default() }));
        assert_eq!(nested.runs[4].1, Marks { bold: true, italic: true, ..Marks::default() });
        // Chinese text right against the stars, as models write it.
        assert_eq!(inline("扩展到**多种粮食作物**，以及").runs[1], ("多种粮食作物".len(), bold()));
    }

    #[test]
    fn what_does_not_close_stays_as_written() {
        // An answer is drawn while it streams.
        assert_eq!(inline("扩展到**多种粮食").text, "扩展到**多种粮食");
        assert_eq!(inline("a `b").text, "a `b");
        assert_eq!(inline("2 * 3 * 4 and snake_case_name, ~5000 cells").runs.len(), 1);
    }

    #[test]
    fn citations_are_not_links() {
        let rich = inline("见 [P3 p.5 \"a sentence\"] 和 [P148][P296]，以及 [原文](https://doi.org/10.1/x) https://example.org/a.");
        assert!(rich.text.starts_with("见 [P3 p.5 \"a sentence\"] 和 [P148][P296]，以及 原文 https://example.org/a."));
        let links: Vec<(&str, &str)> = rich.spans().filter_map(|(range, marks)| marks.link.as_deref().map(|url| (&rich.text[range], url))).collect();
        assert_eq!(links, [("原文", "https://doi.org/10.1/x"), ("https://example.org/a", "https://example.org/a")]);
    }

    #[test]
    fn blocks_of_an_answer() {
        let parsed = parse("## 覆盖范围\n\n第一行\n第二行\n\n- **根**：拟南芥\n  - 侧根\n- 叶\n\n1. 一\n1. 二\n\n---\n> 引文\n\n```python\nprint(1)\n```\n- [x] 完成");
        assert_eq!(parsed[0], Block::Heading(2, Rich::plain("覆盖范围")));
        assert_eq!(parsed[1], Block::Paragraph(Rich::plain("第一行\n第二行")));
        let Block::Item { marker: Marker::Bullet, text, children } = &parsed[2] else { panic!("{:?}", parsed[2]) };
        assert_eq!((text.text.as_str(), &text.runs[0].1), ("根：拟南芥", &bold()));
        assert_eq!(children[..], [Block::Item { marker: Marker::Bullet, text: Rich::plain("侧根"), children: vec![] }]);
        assert!(matches!(&parsed[4], Block::Item { marker: Marker::Number(1), .. }));
        assert!(matches!(&parsed[5], Block::Item { marker: Marker::Number(2), .. }));
        assert_eq!(parsed[6], Block::Rule);
        assert_eq!(parsed[7], Block::Quote(vec![Block::Paragraph(Rich::plain("引文"))]));
        assert_eq!(parsed[8], Block::Code { lang: "python".into(), text: "print(1)".into(), closed: true });
        assert!(matches!(&parsed[9], Block::Item { marker: Marker::Todo(true), .. }));
        assert_eq!(parsed.len(), 10);
        // A fence an answer has not come to the end of yet.
        assert_eq!(parse("```mermaid\ngraph LR\n  A --> B"), [Block::Code { lang: "mermaid".into(), text: "graph LR\n  A --> B".into(), closed: false }]);
    }

    #[test]
    fn pipe_tables() {
        let parsed = parse("| 比较维度 | scRNA-seq | snRNA-seq |\n|---|:---:|---:|\n| **RNA 信息范围** | 整细胞 [P260] | 核内 |\n| 材料 | 需酶解 |\n后面的话");
        let Block::Table { aligns, rows } = &parsed[0] else { panic!("{:?}", parsed[0]) };
        assert_eq!(aligns[..], [Align::Left, Align::Center, Align::Right]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1][0], Rich { text: "RNA 信息范围".into(), runs: vec![("RNA 信息范围".len(), bold())] });
        assert_eq!(rows[1][1].text, "整细胞 [P260]");
        // A short row is filled up.
        assert_eq!(rows[2][2], Rich::default());
        assert_eq!(parsed[1], Block::Paragraph(Rich::plain("后面的话")));
        // Before its second line has arrived, a table is still a paragraph.
        assert!(matches!(parse("| a | b |")[0], Block::Paragraph(_)));
    }

    /// Each piece of a line as (its text, part, bold, cite).
    fn drawn(text: &str) -> Vec<Vec<(&str, Part, bool, bool)>> {
        lines(text).iter().map(|line| line.pieces.iter().map(|piece| (&text[piece.range.clone()], piece.part, piece.marks.bold, piece.cite)).collect()).collect()
    }

    #[test]
    fn lines_keep_where_their_syntax_is() {
        let text = "# 引言\nRoots **grow** [@Zhang2024, p. 5].\n- 一\n  2. 二\n> 引文\n---";
        let read = lines(text);
        assert_eq!((read[0].heading, read[2].item, read[3].item, read[4].quote, read[5].rule), (1, Some(Marker::Bullet), Some(Marker::Number(2)), 1, true));
        let pieces = drawn(text);
        assert_eq!(pieces[0], [("# ", Part::Syntax, false, false), ("引言", Part::Text, false, false)]);
        assert_eq!(pieces[1], [("Roots ", Part::Text, false, false), ("**", Part::Syntax, false, false), ("grow", Part::Text, true, false), ("**", Part::Syntax, false, false), (" ", Part::Text, false, false), ("[@Zhang2024, p. 5]", Part::Text, false, true), (".", Part::Text, false, false)]);
        assert_eq!(pieces[2], [("-", Part::Bullet, false, false), (" 一", Part::Text, false, false)]);
        assert_eq!(pieces[3], [("  ", Part::Text, false, false), ("2. 二", Part::Text, false, false)]);
        assert_eq!(pieces[4][0], ("> ", Part::Syntax, false, false));
        // The pieces cover each line exactly, and the ranges are the text's.
        for line in &read {
            let covered: String = line.pieces.iter().map(|piece| &text[piece.range.clone()]).collect();
            assert_eq!(covered, &text[line.range.clone()]);
        }
        // `#tag` and a lone `#` are not headings; `snake_case` is no emphasis.
        assert_eq!(lines("#tag\n#\nsnake_case_name").iter().map(|line| line.heading).collect::<Vec<_>>(), [0, 0, 0]);
    }

    #[test]
    fn links_code_and_fences() {
        let text = "see [原文](https://doi.org/10.1/x) and `a*b`\n```\nlet x = *1*;\n```\nafter";
        let pieces = drawn(text);
        assert_eq!(pieces[0].iter().filter(|piece| piece.1 == Part::Text).map(|piece| piece.0).collect::<String>(), "see 原文 and a*b");
        let read = lines(text);
        assert!(read[1].code && read[2].code && read[3].code && !read[4].code);
        // Inside a fence nothing is emphasis.
        assert_eq!(pieces[2], [("let x = *1*;", Part::Text, false, false)]);
        assert_eq!(plain(text), "see 原文 and a*b\n```\nlet x = *1*;\n```\nafter");
        assert_eq!(plain("## A **b**\n- c\n***"), "A b\n• c\n");
    }

    #[test]
    fn enter_goes_on_with_a_list() {
        let next = |line: &str| continued(line).map(|c| (c.prefix, c.empty, c.marker));
        assert_eq!(next("- roots"), Some(("- ".into(), false, 2)));
        assert_eq!(next("  * a"), Some(("  * ".into(), false, 4)));
        assert_eq!(next("9. nine"), Some(("10. ".into(), false, 3)));
        assert_eq!(next("1) a"), Some(("2) ".into(), false, 3)));
        assert_eq!(next("- [x] done"), Some(("- [ ] ".into(), false, 6)));
        assert_eq!(next("> quoted"), Some(("> ".into(), false, 2)));
        assert_eq!(next("> - a"), Some(("> - ".into(), false, 4)));
        // A marker alone ends the list.
        assert_eq!(next("- "), Some(("- ".into(), true, 2)));
        assert_eq!(next("3. "), Some(("4. ".into(), true, 3)));
        assert_eq!(next("> "), Some(("> ".into(), true, 2)));
        assert_eq!(next("Plain words."), None);
        assert_eq!(next("---"), None);
        assert_eq!(next("-"), None);
    }

    #[test]
    fn a_picture_on_a_line_of_its_own() {
        let read = |line: &str| picture_line(line).map(|(caption, address)| (line[caption].to_string(), line[address].to_string()));
        let picture = |caption: &str, address: &str| Some((caption.to_string(), address.to_string()));
        assert_eq!(read("![Root length under salt.](figures/G-1.png)"), picture("Root length under salt.", "figures/G-1.png"));
        assert_eq!(read("  ![](figures/G-1.png)  "), picture("", "figures/G-1.png"));
        // A citation in the caption, as written and as a Word file says it; a title after the address.
        assert_eq!(read("![WOX5 marks the centre [@Zhang2021].](figures/G-2.jpg)"), picture("WOX5 marks the centre [@Zhang2021].", "figures/G-2.jpg"));
        assert_eq!(read("![a (Zhang et al., 2021) [1]](<figures/G-2.jpg> \"Fig. 1\")"), picture("a (Zhang et al., 2021) [1]", "figures/G-2.jpg"));
        // Words beside it, two on a line, a link, no address: not a picture's line.
        for line in ["See ![a](figures/x.png)", "![a](figures/x.png) and more.", "![a](x.png) ![b](y.png)", "[a](figures/x.png)", "![a]()", "![a](x.png and more)", "![a](x(1).png)"] {
            assert_eq!(read(line), None, "{line}");
        }

        let text = "Before.\n![Root *length*.](figures/G-1.png)\n![](figures/G-2.png)\n```\n![code](x.png)\n```";
        let read = lines(text);
        assert_eq!(read.iter().map(|line| line.picture.clone().map(|address| &text[address])).collect::<Vec<_>>(), vec![None, Some("figures/G-1.png"), Some("figures/G-2.png"), None, None, None]);
        // The caption is the line's words, set as it says; a picture without one says nothing.
        assert_eq!(plain(text), "Before.\nRoot length.\n\n```\n![code](x.png)\n```");
        assert_eq!(read[2].pieces.iter().map(|piece| (&text[piece.range.clone()], piece.part)).collect::<Vec<_>>(), vec![("![", Part::Syntax), ("](figures/G-2.png)", Part::Syntax)]);
        assert!(read[1].pieces.iter().any(|piece| piece.marks.italic && &text[piece.range.clone()] == "length"));
        // Every byte of the line is in one piece, in order.
        for line in &read[1..3] {
            assert_eq!(line.pieces.iter().map(|piece| &text[piece.range.clone()]).collect::<String>(), &text[line.range.clone()]);
        }
    }

    /// `before` with its selection between ⟨ and ⟩ (a caret is ‸), after ⌘B (`strong`) or ⌘I: the text and the
    /// selection marked the same way; unchanged when there is nothing to do.
    fn pressed(before: &str, strong: bool) -> String {
        let caret = before.find('‸');
        let text = before.replace(['⟨', '⟩', '‸'], "");
        let sel = match caret {
            Some(at) => at..at,
            None => before.find('⟨').unwrap()..before.find('⟩').unwrap() - '⟨'.len_utf8(),
        };
        let Some(edit) = emphasize(&text, sel, strong) else { return before.to_string() };
        let mut out = text.clone();
        out.replace_range(edit.range, &edit.text);
        let (a, b) = (edit.select.start, edit.select.end);
        if a == b { format!("{}‸{}", &out[..a], &out[a..]) } else { format!("{}⟨{}⟩{}", &out[..a], &out[a..b], &out[b..]) }
    }

    #[test]
    fn emphasis_of_a_selection() {
        let bold = |before: &str| pressed(before, true);
        let italic = |before: &str| pressed(before, false);
        // Around the words, not the spaces at the ends; and back again, the marks selected or not.
        assert_eq!(bold("a⟨ word ⟩b"), "a **⟨word⟩** b");
        assert_eq!(bold("a **⟨word⟩** b"), "a ⟨word⟩ b");
        assert_eq!(bold("a ⟨**word**⟩ b"), "a ⟨word⟩ b");
        assert_eq!(italic("⟨拟南芥⟩的根"), "*⟨拟南芥⟩*的根");
        // Part of a span loses it where it was selected, the rest keeps it.
        assert_eq!(bold("**foo ⟨bar⟩ baz**"), "**foo** ⟨bar⟩ **baz**");
        assert_eq!(bold("**⟨foo⟩ bar**"), "⟨foo⟩ **bar**");
        assert_eq!(bold("**甲⟨乙⟩**"), "**甲**⟨乙⟩");
        assert_eq!(italic("***foo ⟨bar⟩***"), "***foo*** **⟨bar⟩**");
        // A span partly selected is taken in whole, and spans inside are folded into the new one.
        assert_eq!(bold("⟨a **b⟩ c**"), "**⟨a b c⟩**");
        assert_eq!(bold("⟨a **b** c⟩"), "**⟨a b c⟩**");
        assert_eq!(bold("⟨a ***b*** c⟩"), "**⟨a *b* c⟩**");
        // Beside the other kind's marks, which the reader then reads as one inside the other.
        assert_eq!(bold("⟨*a* b⟩"), "**⟨*a* b⟩**");
        assert_eq!(bold("*拟南芥*⟨的根⟩"), "*拟南芥***⟨的根⟩**");
        assert_eq!(italic("**⟨word⟩**"), "***⟨word⟩***");
        // Never cutting a citation or code; never into a heading's marks, a list's or a task's box; each line its own.
        assert_eq!(bold("see [@Zh⟨ang2024] now⟩"), "see **⟨[@Zhang2024] now⟩**");
        assert_eq!(bold("a `c⟨o⟩de` b"), "a **⟨`code`⟩** b");
        assert_eq!(bold("⟨# Title⟩"), "# **⟨Title⟩**");
        assert_eq!(bold("⟨one\n\n- two\n- [ ] three⟩"), "**⟨one**\n\n- **two**\n- [ ] **three⟩**");
        assert_eq!(bold("⟨one\n\n- **two**⟩"), "**⟨one**\n\n- **two⟩**");
        assert_eq!(bold("⟨**one**\n\n- **two**⟩"), "⟨one\n\n- two⟩");
        // Code is left as it is.
        assert_eq!(bold("```\n⟨code⟩\n```"), "```\n⟨code⟩\n```");
        // What each change reads as.
        for (text, words) in [("**foo** bar **baz**", "foo bar baz"), ("*拟南芥***的根**", "拟南芥的根"), ("***foo*** **bar**", "foo bar")] {
            assert_eq!(inline(text).text, words, "{text}");
        }
    }

    #[test]
    fn emphasis_at_the_caret() {
        let bold = |before: &str| pressed(before, true);
        let italic = |before: &str| pressed(before, false);
        // An empty pair to type in, taken away by the next press; bold and italic together.
        assert_eq!(bold("a ‸ b"), "a **‸** b");
        assert_eq!(bold("a **‸** b"), "a ‸ b");
        assert_eq!(italic("**‸**"), "***‸***");
        assert_eq!(italic("***‸***"), "**‸**");
        // On a line of its own, where `****` reads as a rule, too; a rule selected is left alone.
        assert_eq!(bold("‸"), "**‸**");
        assert_eq!(bold("**‸**"), "‸");
        assert_eq!(bold("a\n⟨---⟩\nb"), "a\n⟨---⟩\nb");
        // Before the closing mark the caret steps out, so the next words are plain; and back in.
        assert_eq!(bold("**ab‸**"), "**ab**‸");
        assert_eq!(bold("**ab**‸"), "**ab‸**");
        assert_eq!(bold("‸**ab**"), "**‸ab**");
        assert_eq!(bold("**a *b‸***"), "**a *b***‸");
        assert_eq!(italic("***x‸***"), "***x*‸**");
        assert_eq!(bold("***x‸***"), "***x**‸*");
        // The closing mark of one span and the opening of the next are no empty pair.
        assert_eq!(bold("**a** ‸**b**"), "**a** **‸b**");
        assert_eq!(bold("*a*‸*b*"), "*a***‸***b*");
        // Inside a span it loses the mark.
        assert_eq!(bold("x **a‸b** y"), "x a‸b y");
        assert_eq!(italic("***a‸b***"), "**a‸b**");
        // Code has none.
        assert_eq!(bold("```\nco‸de\n```"), "```\nco‸de\n```");
        // What is typed after stepping out of italic inside bold is still bold.
        let runs = inline("***x*y**").runs;
        assert_eq!(runs, vec![(1, Marks { bold: true, italic: true, ..Marks::default() }), (1, Marks { bold: true, ..Marks::default() })]);
        let runs = inline("***x**y*").runs;
        assert_eq!(runs, vec![(1, Marks { bold: true, italic: true, ..Marks::default() }), (1, Marks { italic: true, ..Marks::default() })]);
    }

    #[test]
    fn emphasis_runs_read_as_commonmark() {
        let marks = |text: &str| inline(text).spans().map(|(range, marks)| (inline(text).text[range].to_string(), marks.bold, marks.italic)).collect::<Vec<_>>();
        let s = |t: &str| t.to_string();
        assert_eq!(marks("***a* b**"), [(s("a"), true, true), (s(" b"), true, false)]);
        assert_eq!(marks("***a** b*"), [(s("a"), true, true), (s(" b"), false, true)]);
        assert_eq!(marks("*a***b**"), [(s("a"), false, true), (s("b"), true, false)]);
        // A run that could open as well, two runs coming to three: inside, as before.
        assert_eq!(marks("*甲**乙**丙*"), [(s("甲"), false, true), (s("乙"), true, true), (s("丙"), false, true)]);
        assert_eq!(marks("*a**"), [(s("a"), false, true), (s("*"), false, false)]);
    }
}
