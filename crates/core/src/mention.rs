//! @ in the assistant's question box: `/` says what to do, `@` says which thing is meant. A mention only points at
//! something the library already has; it never changes what a conversation is about and is never a yes to a tool.
//!
//! The text is the truth: a mention is a token in the box's plain text, nothing beside it. A paper is written as a
//! manuscript cites it, `[@Zhang2024]` (its key, `cite::Index`); anything else as `[@kind:value]`: a library on the
//! shelf `[@library:<id>]`, a tag `[@tag:<name>]`, a section of a manuscript `[@section:<id>]`, a figure or table of
//! a paper `[@figure:<paper>/<file>/<number>]` (`figure_value`). A value holds what a
//! token cannot (a blank, a bracket) percent-encoded (`token`, `decode`). A token whose kind is not known, or whose
//! key, id or tag nothing has, stays the text it is. So Undo, 撤回, history and copying need nothing of their own.
//!
//! What the model gets (`resolve`, `note`): the token written as it reads naturally (`[P12]（Zhang 等 2024）`,
//! 标签「X」（N 篇）, 稿件「T」的「节名」一节, `[P1 p.5] 的 Fig. 3`), and a note after the question naming each thing once,
//! with what it needs to read it. A tag is a group named, a section a place named: neither changes what the
//! conversation reads. A figure goes with its caption, its page and, cut out of its page, its picture.

use crate::figref::{self, Key};
use crate::libraries::LibraryInfo;
use crate::manuscript::{Manuscript, Section};
use crate::model::{Paper, author_names};
use std::collections::HashMap;
use std::ops::Range;

/// The kind of `[@library:<id>]`: a library on the shelf, named in the home page's assistant.
pub const LIBRARY: &str = "library";
/// The kind of `[@tag:<name>]`: the papers of the library that have a tag.
pub const TAG: &str = "tag";
/// The kind of `[@section:<id>]`: a section of one of the library's manuscripts.
pub const SECTION: &str = "section";
/// The kind of `[@figure:<paper>/<file>/<number>]`: a figure or table of a paper, in one of its PDFs (`figure_value`).
pub const FIGURE: &str = "figure";
/// The kinds a token other than a paper's may name. A kind not here leaves its token as written.
pub const KINDS: [&str; 4] = [LIBRARY, TAG, SECTION, FIGURE];
/// How far back from the caret an `@` is looked for, in characters: what is typed after it to find a paper.
pub const QUERY_MAX: usize = 40;
/// Rows the popup offers.
pub const OFFERED: usize = 8;
/// How many rows `@` alone gives at least to each kind that has any (`share`).
pub const FLOOR: usize = 2;
/// What the note says of a paper without its full text: a question beyond the abstract is answered as resting on it.
const ABSTRACT_ONLY: &str = "问到摘要之外的细节（方法、实验、数据、图表）时，明说这篇没有可读的全文、回答只依据摘要，摘要里没有的不要推测。";
/// How much of a figure's caption the note gives.
const LEGEND_MAX: usize = 1200;
/// How much of an abstract the note gives.
const ABSTRACT_MAX: usize = 800;
/// The papers of a tag the note names by label; the rest are a tool call away.
const TAG_LISTED: usize = 10;
/// A section's text up to this many characters goes with the note; a longer one is a tool call away.
const SECTION_TEXT_MAX: usize = 1500;
/// Authors the note names before 等.
const AUTHORS_MAX: usize = 6;

/// What a token names, as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Named<'a> {
    /// A paper, by its citation key.
    Paper(&'a str),
    /// `[@kind:value]`.
    Other { kind: &'a str, value: &'a str },
}

/// A token in a text, and where it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token<'a> {
    pub range: Range<usize>,
    pub named: Named<'a>,
}

/// How a paper is mentioned: as a manuscript cites it.
pub fn paper_token(key: &str) -> String {
    crate::cite::written(key)
}

/// How anything else is mentioned: its value with what a token cannot hold encoded (a tag's name may have any).
pub fn token(kind: &str, value: &str) -> String {
    format!("[@{kind}:{}]", encode(value))
}

/// What a token's value cannot hold as written: what ends it (`tokens`), and `%`, which encodes.
fn unwritten(c: char) -> bool {
    matches!(c, ']' | '[' | ';' | ',' | '@' | '%') || c.is_whitespace()
}

/// A value with each character a token cannot hold written as `%` and the hex of its bytes.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if unwritten(c) {
            for byte in c.to_string().bytes() { out.push_str(&format!("%{byte:02X}")) }
        } else {
            out.push(c);
        }
    }
    out
}

/// A token's value as it was before `encode`; a `%` not followed by two hex digits is itself.
pub fn decode(value: &str) -> String {
    if !value.contains('%') { return value.to_string() }
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|b| (*b as char).to_digit(16));
        match (bytes[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(high), Some(low)) => {
                out.push((high * 16 + low) as u8);
                i += 3;
            }
            (byte, ..) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every token written in `text`, in order: `[@` and a run without blanks, brackets, `;` or `,`, closed by `]`.
/// `kind:` before the value is a lower-case word; a key like Pandoc's `doe:2020` reads so too, and is tried as a key when no kind fits.
pub fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut out = vec![];
    let mut from = 0;
    while let Some(at) = text[from..].find("[@").map(|i| from + i) {
        let start = at + 2;
        let rest = &text[start..];
        match rest.find(|c: char| c == ']' || c == '[' || c == ';' || c == ',' || c == '@' || c.is_whitespace()) {
            Some(end) if end > 0 && end <= 200 && rest[end..].starts_with(']') => {
                let inner = &rest[..end];
                let named = match inner.split_once(':') {
                    Some((kind, value)) if !kind.is_empty() && !value.is_empty() && kind.chars().all(|c| c.is_ascii_lowercase()) => Named::Other { kind, value },
                    _ => Named::Paper(inner),
                };
                out.push(Token { range: at..start + end + 1, named });
                from = start + end + 1;
            }
            _ => from = start,
        }
    }
    out
}

/// `text` with its tokens taken out: what the person wrote in their own words (`ai::reply_language`).
pub fn without(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for token in tokens(text) {
        out.push_str(&text[at..token.range.start]);
        out.push(' ');
        at = token.range.end;
    }
    out.push_str(&text[at..]);
    out
}

/// `text` with its tokens written as a person reads them where nothing better is known (a conversation's title
/// listed where the library is not at hand): a paper by its key, `@Zhang2024`, a tag `#name`, a section `§`, a
/// figure by its number (`Fig. 3`), a library by its id.
pub fn readable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for token in tokens(text) {
        out.push_str(&text[at..token.range.start]);
        match token.named {
            Named::Paper(key) => out.push_str(&format!("@{key}")),
            Named::Other { kind: TAG, value } => out.push_str(&format!("#{}", decode(value))),
            Named::Other { kind: SECTION, .. } => out.push('§'),
            Named::Other { kind: FIGURE, value } => out.push_str(&figure_named(&decode(value)).map_or_else(|| format!("@{}", decode(value)), |named| key_written(named.key))),
            Named::Other { value, .. } => out.push_str(&format!("@{}", decode(value))),
        }
        at = token.range.end;
    }
    out.push_str(&text[at..]);
    out
}

/// The `@` being typed before the caret: from the `@` to the caret, while what follows it is a query (no line break,
/// no bracket, at most `QUERY_MAX` characters). An `@` right after an ASCII letter or digit is an address (`a@b.com`)
/// and one after `[` is a token's: neither is a mention being typed. A Chinese character before it is fine.
pub fn typed(text: &str, caret: usize) -> Option<Range<usize>> {
    if caret > text.len() || !text.is_char_boundary(caret) { return None }
    let before = &text[..caret];
    for (n, (i, c)) in before.char_indices().rev().enumerate() {
        if c == '\n' || c == '[' || c == ']' { return None }
        if c == '@' {
            let prev = before[..i].chars().next_back();
            if prev.is_some_and(|p| p.is_ascii_alphanumeric() || p == '[' || p == '@') { return None }
            return Some(i..caret);
        }
        if n >= QUERY_MAX { return None }
    }
    None
}

/// A paper as a mention is drawn: the first author's family name, 等 / et al. when there are more, and the year; a
/// paper without authors by the start of its title.
pub fn short_name(paper: &Paper, english: bool) -> String {
    let names = author_names(&paper.authors);
    let who = match names.first() {
        Some((family, _)) if names.len() > 1 => if english { format!("{family} et al.") } else { format!("{family} 等") },
        Some((family, _)) => family.clone(),
        None => cut(&paper.title, 16),
    };
    match paper.year {
        Some(year) if !who.is_empty() => format!("{who} {year}"),
        Some(year) => year.to_string(),
        None => who,
    }
}

/// The first author's family name and the year, for the right of a row: what tells two papers of one title apart.
pub fn byline(paper: &Paper) -> String {
    let family = author_names(&paper.authors).into_iter().next().map(|(family, _)| family).unwrap_or_default();
    [family, paper.year.map(|y| y.to_string()).unwrap_or_default()].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ")
}

/// `text` on one line, at most `chars` characters, `…` where it was cut.
pub fn cut(text: &str, chars: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= chars { return text }
    let mut out: String = text.chars().take(chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The key of every paper of a library and the paper of every key, as `cite::Index` gives them: made once when the
/// library is read (`Library::reload`), so that a keystroke and a frame only look up.
#[derive(Clone, Debug, Default)]
pub struct Keys {
    key_of: HashMap<String, String>,
    /// By the key in lower case: keys are unique ignoring case.
    by_key: HashMap<String, String>,
}

impl Keys {
    pub fn new(index: &crate::cite::Index, papers: &[Paper]) -> Keys {
        let mut keys = Keys::default();
        for paper in papers {
            let Some(key) = index.key(&paper.id) else { continue };
            keys.by_key.insert(key.to_lowercase(), paper.id.clone());
            keys.key_of.insert(paper.id.clone(), key.to_string());
        }
        keys
    }

    pub fn key(&self, paper_id: &str) -> Option<&str> {
        self.key_of.get(paper_id).map(String::as_str)
    }

    /// The paper a key names; a key like `doe:2020` is read whole.
    pub fn paper(&self, named: &Named) -> Option<&str> {
        let key = match named {
            Named::Paper(key) => key.to_string(),
            Named::Other { kind, value } if !KINDS.contains(kind) => format!("{kind}:{value}"),
            Named::Other { .. } => return None,
        };
        self.by_key.get(&key.to_lowercase()).map(String::as_str)
    }
}

/// A paper the popup may offer: with its key and its label in the conversation (`P3`), empty when it has none yet.
pub struct Candidate<'a> {
    pub paper: &'a Paper,
    pub key: &'a str,
    pub label: &'a str,
}

/// The papers every word of `query` is somewhere in: the title, a family name of its authors, the year, the key, or
/// the label in the conversation (`P3`, whole). Those whose name (a family name, the key, the title or the label)
/// begins with the first word come first; otherwise the order given. At most `limit`.
pub fn matching<'a>(query: &str, candidates: impl IntoIterator<Item = Candidate<'a>>, limit: usize) -> Vec<&'a Paper> {
    matching_ranked(query, candidates, limit).into_iter().map(|(_, paper)| paper).collect()
}

/// `matching` with each paper's rank: 0 when its name begins with the query, 1 when the query is only in it.
pub fn matching_ranked<'a>(query: &str, candidates: impl IntoIterator<Item = Candidate<'a>>, limit: usize) -> Vec<(u8, &'a Paper)> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() { return vec![] }
    let mut found: Vec<(u8, &Paper)> = candidates
        .into_iter()
        .filter_map(|candidate| {
            let paper = candidate.paper;
            let title = paper.title.to_lowercase();
            let families: Vec<String> = author_names(&paper.authors).into_iter().map(|(family, _)| family.to_lowercase()).collect();
            let year = paper.year.map(|y| y.to_string()).unwrap_or_default();
            let (key, label) = (candidate.key.to_lowercase(), candidate.label.to_lowercase());
            let holds = |word: &str| title.contains(word) || families.iter().any(|family| family.contains(word)) || (!year.is_empty() && year.starts_with(word)) || (!key.is_empty() && key.contains(word)) || (!label.is_empty() && label == word);
            if !words.iter().all(|word| holds(word)) { return None }
            let first = words[0].as_str();
            let named = families.iter().any(|family| family.starts_with(first)) || key.starts_with(first) || title.starts_with(first) || label == first;
            Some((if named { 0 } else { 1 }, paper))
        })
        .collect();
    found.sort_by_key(|(rank, _)| *rank);
    found.truncate(limit);
    found
}

/// The libraries every word of `query` is in, by name or topic; those whose name begins with it first.
pub fn matching_libraries<'a>(query: &str, libraries: impl IntoIterator<Item = &'a LibraryInfo>, limit: usize) -> Vec<&'a LibraryInfo> {
    matching_libraries_ranked(query, libraries, limit).into_iter().map(|(_, info)| info).collect()
}

/// `matching_libraries` with each library's rank, as `matching_ranked`.
pub fn matching_libraries_ranked<'a>(query: &str, libraries: impl IntoIterator<Item = &'a LibraryInfo>, limit: usize) -> Vec<(u8, &'a LibraryInfo)> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() { return vec![] }
    let mut found: Vec<(u8, &LibraryInfo)> = libraries
        .into_iter()
        .filter_map(|info| {
            let (name, topic) = (info.name.to_lowercase(), info.topic.to_lowercase());
            if !words.iter().all(|word| name.contains(word.as_str()) || topic.contains(word.as_str())) { return None }
            Some((if name.starts_with(words[0].as_str()) { 0 } else { 1 }, info))
        })
        .collect();
    found.sort_by_key(|(rank, _)| *rank);
    found.truncate(limit);
    found
}

/// The tags (each with how many papers have it) every word of `query` is in; those that begin with it first, then by
/// the order given (the most used first). With its rank, as `matching_ranked`.
pub fn matching_tags<'a>(query: &str, tags: impl IntoIterator<Item = &'a (String, usize)>, limit: usize) -> Vec<(u8, &'a (String, usize))> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() { return vec![] }
    let mut found: Vec<(u8, &(String, usize))> = tags
        .into_iter()
        .filter_map(|tag| {
            let name = tag.0.to_lowercase();
            if !words.iter().all(|word| name.contains(word.as_str())) { return None }
            Some((if name.starts_with(words[0].as_str()) { 0 } else { 1 }, tag))
        })
        .collect();
    found.sort_by_key(|(rank, _)| *rank);
    found.truncate(limit);
    found
}

/// The sections every word of `query` is in, by the section's name or its manuscript's title; those whose name
/// begins with it first, then in the order given. With its rank, as `matching_ranked`.
pub fn matching_sections<'a>(query: &str, sections: impl IntoIterator<Item = (&'a Manuscript, &'a Section)>, limit: usize) -> Vec<(u8, (&'a Manuscript, &'a Section))> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() { return vec![] }
    let mut found: Vec<(u8, (&Manuscript, &Section))> = sections
        .into_iter()
        .filter_map(|(manuscript, section)| {
            let (name, title) = (section.name.to_lowercase(), manuscript.title.to_lowercase());
            if !words.iter().all(|word| name.contains(word.as_str()) || title.contains(word.as_str())) { return None }
            Some((if name.starts_with(words[0].as_str()) { 0 } else { 1 }, (manuscript, section)))
        })
        .collect();
    found.sort_by_key(|(rank, _)| *rank);
    found.truncate(limit);
    found
}

/// How many of `total` rows each kind gets when `@` alone lists them, the kinds in the order they are listed and
/// `have` how many rows each could give: every kind that has rows at least `FLOOR` of them (fewer when it has
/// fewer, or when the room is too small for all), what is left to the kinds in their order.
pub fn share(have: &[usize], total: usize) -> Vec<usize> {
    let mut given: Vec<usize> = vec![0; have.len()];
    let mut left = total;
    for (n, &rows) in have.iter().enumerate() {
        let floor = rows.min(FLOOR).min(left);
        given[n] = floor;
        left -= floor;
    }
    for (n, &rows) in have.iter().enumerate() {
        let more = (rows - given[n]).min(left);
        given[n] += more;
        left -= more;
    }
    given
}

/// A figure as a token names it: the paper, the file of the paper it is in (its id), and its number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FigureName {
    pub paper: String,
    pub file: String,
    pub key: Key,
}

/// How a figure's number is written in its token: `fig3`, `figS2` (the supplement's), `figE1` (Extended Data),
/// `table2`, `scheme1`. The same however the paper writes it.
pub fn key_code(key: Key) -> String {
    let kind = match key.kind {
        figref::Kind::Figure => "fig",
        figref::Kind::Table => "table",
        figref::Kind::Scheme => "scheme",
    };
    let series = match key.series {
        figref::Series::Main => "",
        figref::Series::Supplementary => "S",
        figref::Series::Extended => "E",
    };
    format!("{kind}{series}{}", key.number)
}

/// The number `key_code` wrote.
pub fn key_of_code(code: &str) -> Option<Key> {
    let (kind, rest) = [("fig", figref::Kind::Figure), ("table", figref::Kind::Table), ("scheme", figref::Kind::Scheme)].into_iter().find_map(|(word, kind)| code.strip_prefix(word).map(|rest| (kind, rest)))?;
    let (series, digits) = match rest.as_bytes().first()? {
        b'S' => (figref::Series::Supplementary, &rest[1..]),
        b'E' => (figref::Series::Extended, &rest[1..]),
        _ => (figref::Series::Main, rest),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) { return None }
    Some(Key { kind, series, number: digits.parse().ok()? })
}

/// A figure's number as a person writes it where the paper's own words are not at hand: `Fig. 3`, `Fig. S2`,
/// `Extended Data Fig. 1`, `Table 2`.
pub fn key_written(key: Key) -> String {
    let kind = match key.kind {
        figref::Kind::Figure => "Fig.",
        figref::Kind::Table => "Table",
        figref::Kind::Scheme => "Scheme",
    };
    match key.series {
        figref::Series::Main => format!("{kind} {}", key.number),
        figref::Series::Supplementary => format!("{kind} S{}", key.number),
        figref::Series::Extended => format!("Extended Data {kind} {}", key.number),
    }
}

/// The value of a figure's token: `<paper id>/<file id>/<number>` (`key_code`), what stays the same across
/// restarts. `token(FIGURE, …)` encodes what an id may hold that a token cannot.
pub fn figure_value(paper: &str, file: &str, key: Key) -> String {
    format!("{paper}/{file}/{}", key_code(key))
}

/// What a figure's (decoded) value names; `None` when it is not one.
pub fn figure_named(value: &str) -> Option<FigureName> {
    let (rest, code) = value.rsplit_once('/')?;
    let (paper, file) = rest.split_once('/')?;
    if paper.is_empty() || file.is_empty() { return None }
    Some(FigureName { paper: paper.to_string(), file: file.to_string(), key: key_of_code(code)? })
}

/// The id a figure's picture is kept under with a question: the same for the same figure, so that a question asked
/// again, or taken back into the box, does not carry it twice.
pub fn figure_picture_id(value: &str) -> String {
    // FNV-1a: stable across builds, unlike the standard hasher.
    let hash = value.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3));
    format!("IF{hash:016x}")
}

/// A figure the popup may offer: its number, as its caption writes it, and the caption's words.
pub struct FigureCandidate<'a> {
    pub key: Key,
    pub label: &'a str,
    pub legend: &'a str,
}

/// The number a query writes, typed loosely: `fig3`, `Fig. 3`, `figure 3b`, `图3`, `table 2`, `tab2`, `表2`, `figS2`.
fn typed_key(query: &str) -> Option<Key> {
    let query = query.trim();
    // `tab` is not a word `figref` reads.
    let lower = query.to_lowercase();
    let query = match lower.strip_prefix("tab") {
        Some(rest) if !rest.starts_with("le") => format!("table{rest}"),
        _ => lower,
    };
    // Typed together, `figs2` is Fig. S2, not the plural of a figure.
    for word in ["figs", "tables"] {
        if let Some(rest) = query.strip_prefix(word) && rest.starts_with(|c: char| c.is_ascii_digit()) { return figref::key_of(&format!("{} S{rest}", &word[..word.len() - 1])) }
    }
    figref::key_of(&query)
}

/// The kind a query names without a number yet: `fig`, `figure`, `tab`, `table`, `图`, `表`.
fn typed_kind(query: &str) -> Option<figref::Kind> {
    let query = query.trim().trim_end_matches('.').to_lowercase();
    if query.is_empty() { return None }
    if "figure".starts_with(&query) || "图".starts_with(&query) { return Some(figref::Kind::Figure) }
    if "table".starts_with(&query) || query == "表" { return Some(figref::Kind::Table) }
    None
}

/// The figures a query names, in the order given (the order they stand in their files): those whose number it
/// writes, however loosely (`typed_key`), or whose kind it begins to write (`fig`, `表`), first; then those whose
/// caption holds every word of it. With each one's place in `figures` and its rank, as `matching_ranked`.
pub fn matching_figures<'a>(query: &str, figures: impl IntoIterator<Item = FigureCandidate<'a>>, limit: usize) -> Vec<(u8, usize)> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() { return vec![] }
    let (key, kind) = (typed_key(query), typed_kind(query));
    let mut found: Vec<(u8, usize)> = figures
        .into_iter()
        .enumerate()
        .filter_map(|(n, figure)| {
            if key.is_some_and(|key| key == figure.key) || key.is_none() && kind.is_some_and(|kind| kind == figure.key.kind) { return Some((0, n)) }
            let text = format!("{} {}", figure.label, figure.legend).to_lowercase();
            words.iter().all(|word| text.contains(word.as_str())).then_some((1, n))
        })
        .collect();
    found.sort_by_key(|(rank, _)| *rank);
    found.truncate(limit);
    found
}

/// A figure found for the model: where it is (the paper's label in the conversation, the attachment it is in,
/// the 1-based page), its number as the paper writes it, its caption, and its picture: the place it has among the
/// question's pictures (1-based), or why there is none.
#[derive(Clone, Debug, PartialEq)]
pub struct FigureSeen {
    pub label: String,
    pub attachment: Option<usize>,
    pub page: usize,
    pub key: String,
    pub legend: String,
    pub picture: Result<usize, String>,
}

impl FigureSeen {
    /// The page as an answer cites it: `[P1 p.5]`, `[P1 S1 p.2]` in an attachment.
    pub fn cited(&self) -> String {
        match self.attachment {
            Some(n) => format!("[{} S{n} p.{}]", self.label, self.page),
            None => format!("[{} p.{}]", self.label, self.page),
        }
    }
}

/// What a token was found to name, for the model.
#[derive(Clone, Debug)]
pub enum Thing<'a> {
    /// A paper and the label the conversation cites it by.
    Paper { paper: &'a Paper, label: String },
    Library(&'a LibraryInfo),
    /// A tag, and every paper that has it with its label in the conversation (empty when it has none).
    Tag { name: String, papers: Vec<(String, &'a Paper)> },
    /// A section and the manuscript it is in.
    Section { manuscript: &'a Manuscript, section: &'a Section },
    /// A figure or table of a paper.
    Figure(FigureSeen),
}

impl Thing<'_> {
    fn same(&self, other: &Thing) -> bool {
        match (self, other) {
            (Thing::Paper { paper: a, .. }, Thing::Paper { paper: b, .. }) => a.id == b.id,
            (Thing::Library(a), Thing::Library(b)) => a.id == b.id,
            (Thing::Tag { name: a, .. }, Thing::Tag { name: b, .. }) => a == b,
            (Thing::Section { section: a, .. }, Thing::Section { section: b, .. }) => a.id == b.id,
            (Thing::Figure(a), Thing::Figure(b)) => a.cited() == b.cited() && a.key == b.key,
            _ => false,
        }
    }

    /// The thing as the question names it to the model, in the token's place.
    pub fn written(&self, english: bool) -> String {
        match self {
            Thing::Paper { paper, label } => format!("[{label}]（{}）", short_name(paper, english)),
            Thing::Library(info) => format!("文献库「{}」", info.name),
            Thing::Tag { name, papers } => format!("标签「{name}」（{} 篇）", papers.len()),
            Thing::Section { manuscript, section } => format!("稿件「{}」的「{}」一节", manuscript.shown_title(), section.name.trim()),
            Thing::Figure(figure) => format!("{} 的 {}", figure.cited(), figure.key),
        }
    }
}

/// `text` with each token `find` knows written as the model reads it (`Thing::written`), and what they name, each once,
/// in the order first named. A token `find` does not know stays as written.
pub fn resolve<'a>(text: &str, find: impl Fn(&Named) -> Option<Thing<'a>>, english: bool) -> (String, Vec<Thing<'a>>) {
    let mut out = String::with_capacity(text.len());
    let mut named: Vec<Thing<'a>> = vec![];
    let mut at = 0;
    for token in tokens(text) {
        let Some(thing) = find(&token.named) else { continue };
        out.push_str(&text[at..token.range.start]);
        out.push_str(&thing.written(english));
        at = token.range.end;
        if !named.iter().any(|known| known.same(&thing)) { named.push(thing) }
    }
    out.push_str(&text[at..]);
    (out, named)
}

/// What goes after a question that names things with @: each of them, with what it takes to read it. Empty without any.
pub fn note(named: &[Thing]) -> String {
    if named.is_empty() { return String::new() }
    let mut out = String::from("【用 @ 点名的内容】用户在问题里用 @ 点名了下面这些；问题里说到它们时，指的就是这些：\n");
    for thing in named {
        match thing {
            Thing::Paper { paper, label } => {
                out.push_str(&format!("- [{label}] {}\n", paper.title.split_whitespace().collect::<Vec<_>>().join(" ")));
                let names: Vec<&str> = paper.authors.split(';').map(str::trim).filter(|name| !name.is_empty()).collect();
                let authors = if names.len() > AUTHORS_MAX { format!("{} 等 {} 人", names[..AUTHORS_MAX].join("; "), names.len()) } else { names.join("; ") };
                let facts: Vec<String> = [("作者", authors), ("年份", paper.year.map(|y| y.to_string()).unwrap_or_default()), ("期刊", paper.journal.trim().to_string()), ("DOI", paper.doi.trim().to_string())]
                    .into_iter()
                    .filter(|(_, value)| !value.is_empty())
                    .map(|(name, value)| format!("{name}：{value}"))
                    .collect();
                if !facts.is_empty() { out.push_str(&format!("  {}\n", facts.join("；"))) }
                let summary = cut(&paper.abstract_text, ABSTRACT_MAX);
                if !summary.is_empty() { out.push_str(&format!("  摘要：{summary}\n")) }
                out.push_str(&format!(
                    "  全文：{}\n",
                    match paper.fulltext.as_str() {
                        "stored" => format!("已保存，用 read_fulltext 读（paper 填 {label}），search_fulltext 在其中找原文。问到方法、实验、数据、图表这些摘要之外的细节时，先读全文里相关的部分再回答，不要只凭摘要作答。"),
                        "textless" => format!("存了 PDF，但没有读出文字，只有上面的记录和摘要。{ABSTRACT_ONLY}"),
                        "linked" => format!("只有全文的链接，没有保存全文，只有上面的记录和摘要。{ABSTRACT_ONLY}"),
                        _ => format!("没有保存全文，只有上面的记录和摘要。{ABSTRACT_ONLY}"),
                    }
                ));
            }
            Thing::Library(info) => {
                let topic: String = info.topic.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect();
                out.push_str(&format!("- 文献库「{}」（ID {}）：{} 篇{}。工具带上 library（名称或 ID）就是读这个文献库。\n", info.name, info.id, info.paper_count, if topic.is_empty() { String::new() } else { format!("。主题：{topic}") }));
            }
            Thing::Tag { name, papers } => {
                let listed: Vec<String> = papers.iter().take(TAG_LISTED).map(|(label, paper)| if label.is_empty() { short_name(paper, false) } else { format!("[{label}] {}", short_name(paper, false)) }).collect();
                let quoted = serde_json::to_string(name).unwrap_or_default();
                let rest = if papers.len() > TAG_LISTED { format!("其余 {} 篇用 list_papers 列出（tag 填 {quoted}）", papers.len() - TAG_LISTED) } else { format!("list_papers（tag 填 {quoted}）列出它们的记录") };
                out.push_str(&format!("- 标签「{name}」：文献库里有 {} 篇文献带这个标签{}：{}。{rest}。标签只是点名这一组文献，不改变这次对话读哪些文献。\n", papers.len(), if papers.len() > TAG_LISTED { format!("，前 {TAG_LISTED} 篇") } else { String::new() }, listed.join("；")));
            }
            Thing::Section { manuscript, section } => {
                let kind = crate::model::label(&crate::structure::SECTIONS, &section.kind);
                let body = section.body.trim();
                let length = body.chars().count();
                out.push_str(&format!("- 稿件「{}」（ID {}）的「{}」一节（ID {}，属于{kind}）：", manuscript.shown_title(), manuscript.id, section.name.trim(), section.id));
                if body.is_empty() {
                    out.push_str("还没有写内容。\n");
                } else if length <= SECTION_TEXT_MAX {
                    out.push_str(&format!("共 {length} 字，现在的正文：\n<<<\n{body}\n>>>\n"));
                } else {
                    out.push_str(&format!("共 {length} 字，较长，用 read_manuscript 读（manuscript 填 {}，section 填 {}）。\n", manuscript.id, section.id));
                }
            }
            Thing::Figure(figure) => {
                let place = match figure.attachment {
                    Some(n) => format!("附件 S{n} 第 {} 页", figure.page),
                    None => format!("正文第 {} 页", figure.page),
                };
                out.push_str(&format!("- {} 的 {}：在 [{}] 的{place}，引用时写 {}。\n", figure.cited(), figure.key, figure.label, figure.cited()));
                let legend = cut(&figure.legend, LEGEND_MAX);
                if !legend.is_empty() { out.push_str(&format!("  图注：{} {legend}\n", figure.key)) }
                match &figure.picture {
                    Ok(n) => out.push_str(&format!("  这张图（连同图注）从 PDF 页面上截成图片，附在这个问题里（第 {n} 张图片）：回答关于它的问题时看这张图片，图里看不清的就说看不清，不要猜。\n")),
                    Err(why) => out.push_str(&format!("  没能截出这张图的图片（{why}），只有上面的图注和页码：回答时说明没有看到图本身。\n")),
                }
            }
        }
    }
    out.push_str("点名只说明问的是哪一个，不是让你改动它们，也不表示用户同意了任何改动。");
    if named.iter().any(|thing| matches!(thing, Thing::Section { .. })) { out.push_str("点到稿件的一节，是说问题说的是这一节，不是要你改它：用户没有要求改，就不要用 edit_section 改它。") }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paper(id: &str, title: &str, authors: &str, year: Option<i32>) -> Paper {
        Paper { id: id.into(), title: title.into(), authors: authors.into(), year, added_at: id.into(), ..Default::default() }
    }

    #[test]
    fn tokens_are_read_from_the_text() {
        let text = "比较 [@Zhang2024] 和 [@Li2019b]，在 [@library:L7] 里找；[@ broken] [@a b] [@Zhang2024, p. 5] [@Doe:2020]";
        let found: Vec<(&str, Named)> = tokens(text).into_iter().map(|token| (&text[token.range.clone()], token.named)).collect();
        assert_eq!(found, vec![("[@Zhang2024]", Named::Paper("Zhang2024")), ("[@Li2019b]", Named::Paper("Li2019b")), ("[@library:L7]", Named::Other { kind: "library", value: "L7" }), ("[@Doe:2020]", Named::Paper("Doe:2020"))]);
        assert_eq!(without("[@Zhang2024] what does it say"), "  what does it say");
        assert_eq!(readable("比较 [@Zhang2024] 和 [@library:L7]"), "比较 @Zhang2024 和 @L7");
        assert_eq!(paper_token("Zhang2024"), "[@Zhang2024]");
        assert_eq!(token(LIBRARY, "L7"), "[@library:L7]");
    }

    #[test]
    fn an_at_is_a_mention_being_typed_only_where_it_starts_a_word() {
        assert_eq!(typed("看看 @zh", "看看 @zh".len()), Some("看看 ".len().."看看 @zh".len()));
        // A Chinese character right before it is fine; an address is not a mention.
        assert_eq!(typed("比较@li", "比较@li".len()), Some("比较".len().."比较@li".len()));
        assert_eq!(typed("mail a@b.com", 12), None);
        assert_eq!(typed("@", 1), Some(0..1));
        // Spaces are part of the query; a line break, a token or a long run ends it.
        assert_eq!(typed("@root stem", 10), Some(0..10));
        assert_eq!(typed("@root\nstem", 10), None);
        assert_eq!(typed("[@Zhang2024] and", 16), None);
        assert_eq!(typed(&format!("@{}", "a".repeat(QUERY_MAX + 5)), QUERY_MAX + 6), None);
        // Only the text before the caret counts.
        assert_eq!(typed("@zh then", 3), Some(0..3));
    }

    #[test]
    fn a_paper_is_drawn_by_its_first_author_and_year() {
        assert_eq!(short_name(&paper("1", "Root stem cells", "Zhang, Wei; Li, Na", Some(2024)), false), "Zhang 等 2024");
        assert_eq!(short_name(&paper("1", "Root stem cells", "Wei Zhang; Na Li", Some(2024)), true), "Zhang et al. 2024");
        assert_eq!(short_name(&paper("1", "Root stem cells", "Zhang W", None), false), "Zhang");
        assert_eq!(short_name(&paper("1", "A very long title about the roots of plants", "", Some(2020)), false), "A very long tit… 2020");
        assert_eq!(byline(&paper("1", "T", "Zhang, Wei; Li, Na", Some(2024))), "Zhang 2024");
    }

    #[test]
    fn papers_are_found_by_every_word_names_first() {
        let papers = [paper("1", "Stem cells of the root", "Zhang, Wei; Li, Na", Some(2024)), paper("2", "Zhang's method revisited", "Wang, Fang", Some(2019)), paper("3", "拟南芥根干细胞", "Li, Na", Some(2021))];
        let index = crate::cite::Index::new(&papers, [], &Default::default());
        let keys = Keys::new(&index, &papers);
        let labels = ["P1", "P2", "P3"];
        let found = |query: &str| matching(query, papers.iter().zip(labels).map(|(paper, label)| Candidate { paper, key: keys.key(&paper.id).unwrap_or(""), label }), OFFERED).into_iter().map(|p| p.id.as_str()).collect::<Vec<_>>();
        // The author named Zhang before the title that holds the word.
        assert_eq!(found("zhang"), ["1", "2"]);
        assert_eq!(found("li 2021"), ["3"]);
        assert_eq!(found("根干细胞"), ["3"]);
        assert_eq!(found("p2"), ["2"]);
        assert_eq!(found("Wang2019"), ["2"]);
        assert!(found("nothing here").is_empty());
        assert_eq!(keys.paper(&Named::Paper("zhang2024")), Some("1"));
        assert_eq!(keys.paper(&Named::Other { kind: LIBRARY, value: "1" }), None);
    }

    #[test]
    fn libraries_are_found_by_name_and_topic() {
        let shelf = [LibraryInfo { id: "a".into(), name: "根发育".into(), topic: "root development".into(), ..Default::default() }, LibraryInfo { id: "b".into(), name: "Root hairs".into(), ..Default::default() }];
        let found: Vec<&str> = matching_libraries("root", &shelf, OFFERED).into_iter().map(|info| info.id.as_str()).collect();
        assert_eq!(found, ["b", "a"]);
    }

    #[test]
    fn the_model_reads_each_named_thing_once_with_a_note() {
        let mut stored = paper("1", "Stem cells of the root", "Zhang, Wei; Li, Na", Some(2024));
        stored.fulltext = "stored".into();
        stored.abstract_text = "We find the root.".into();
        stored.journal = "Nature".into();
        let shelf = LibraryInfo { id: "L7".into(), name: "根发育".into(), topic: "roots".into(), paper_count: 12, ..Default::default() };
        let find = |named: &Named| match named {
            Named::Paper("Zhang2024") => Some(Thing::Paper { paper: &stored, label: "P12".into() }),
            Named::Other { kind: LIBRARY, value: "L7" } => Some(Thing::Library(&shelf)),
            _ => None,
        };
        let (text, named) = resolve("/compare [@Zhang2024] and [@Zhang2024] in [@library:L7] [@Nobody2000]", find, false);
        assert_eq!(text, "/compare [P12]（Zhang 等 2024） and [P12]（Zhang 等 2024） in 文献库「根发育」 [@Nobody2000]");
        assert_eq!(named.len(), 2);
        let note = note(&named);
        assert!(note.contains("[P12] Stem cells of the root"));
        assert!(note.contains("期刊：Nature"));
        assert!(note.contains("摘要：We find the root."));
        assert!(note.contains("read_fulltext") && note.contains("先读全文里相关的部分再回答"), "{note}");
        let mut bare = stored.clone();
        bare.fulltext = "linked".into();
        assert!(super::note(&[Thing::Paper { paper: &bare, label: "P1".into() }]).contains("回答只依据摘要"));
        assert!(note.contains("文献库「根发育」（ID L7）：12 篇。主题：roots"));
        assert!(super::note(&[]).is_empty());
    }

    #[test]
    fn a_tag_of_any_name_is_one_token() {
        for name in ["root hairs", "a]b", "[x] y;z,@w", "100%", "根 发育", "全角　空格"] {
            let written = token(TAG, name);
            let found = tokens(&written);
            assert_eq!(found.len(), 1, "{written}");
            assert_eq!(found[0].range, 0..written.len());
            let Named::Other { kind, value } = found[0].named else { panic!("{written}") };
            assert_eq!((kind, decode(value).as_str()), (TAG, name));
        }
        assert_eq!(token(TAG, "root hairs"), "[@tag:root%20hairs]");
        assert_eq!(token(TAG, "a]b"), "[@tag:a%5Db]");
        // Plain values are as they are; a stray `%` decodes to itself.
        assert_eq!((token(SECTION, "S12"), decode("50%"), decode("%zz")), ("[@section:S12]".to_string(), "50%".to_string(), "%zz".to_string()));
        assert_eq!(readable(&format!("看 {} 和 {}", token(TAG, "root hairs"), token(SECTION, "S1"))), "看 #root hairs 和 §");
    }

    #[test]
    fn tags_and_sections_are_found_by_their_names() {
        let tags = [("soil".to_string(), 3), ("root hairs".to_string(), 2), ("hairy roots".to_string(), 1)];
        let found: Vec<(u8, &str)> = matching_tags("root", &tags, OFFERED).into_iter().map(|(rank, (name, _))| (rank, name.as_str())).collect();
        assert_eq!(found, [(0, "root hairs"), (1, "hairy roots")]);
        let manuscript = Manuscript { id: "M1".into(), title: "根的综述".into(), sections: vec![Section { id: "S1".into(), name: "引言".into(), ..Default::default() }, Section { id: "S2".into(), name: "根毛的引言".into(), ..Default::default() }], ..Default::default() };
        let found: Vec<(u8, &str)> = matching_sections("引言", manuscript.sections.iter().map(|section| (&manuscript, section)), OFFERED).into_iter().map(|(rank, (_, section))| (rank, section.id.as_str())).collect();
        assert_eq!(found, [(0, "S1"), (1, "S2")]);
        assert_eq!(matching_sections("综述", manuscript.sections.iter().map(|section| (&manuscript, section)), OFFERED).len(), 2);
    }

    #[test]
    fn every_kind_with_rows_gets_two_of_the_eight() {
        assert_eq!(share(&[20, 5, 4], OFFERED), [4, 2, 2]);
        assert_eq!(share(&[20, 1, 0], OFFERED), [7, 1, 0]);
        assert_eq!(share(&[3, 20], OFFERED), [3, 5]);
        assert_eq!(share(&[0, 0], OFFERED), [0, 0]);
        assert_eq!(share(&[9, 9, 9, 9, 9], 8), [2, 2, 2, 2, 0]);
    }

    #[test]
    fn a_figure_is_named_by_its_paper_file_and_number() {
        use figref::{Kind, Series};
        let key = |kind, series, number| Key { kind, series, number };
        for (k, code, written) in [(key(Kind::Figure, Series::Main, 3), "fig3", "Fig. 3"), (key(Kind::Figure, Series::Supplementary, 12), "figS12", "Fig. S12"), (key(Kind::Table, Series::Main, 2), "table2", "Table 2"), (key(Kind::Figure, Series::Extended, 1), "figE1", "Extended Data Fig. 1"), (key(Kind::Scheme, Series::Main, 4), "scheme4", "Scheme 4")] {
            assert_eq!((key_code(k).as_str(), key_of_code(code), key_written(k).as_str()), (code, Some(k), written));
        }
        assert_eq!((key_of_code("fig"), key_of_code("figX3"), key_of_code("photo3")), (None, None, None));
        // Ids of any kind, a blank or a slash in the file's id among them, make one token and come back as they were.
        let value = figure_value("P-17-ab", "R 9/x", key(Kind::Figure, Series::Main, 3));
        let written = token(FIGURE, &value);
        let found = tokens(&written);
        assert_eq!((found.len(), found[0].range.clone()), (1, 0..written.len()));
        let Named::Other { kind, value: encoded } = found[0].named else { panic!("{written}") };
        assert_eq!(kind, FIGURE);
        assert_eq!(figure_named(&decode(encoded)), Some(FigureName { paper: "P-17-ab".into(), file: "R 9/x".into(), key: key(Kind::Figure, Series::Main, 3) }));
        assert_eq!((figure_named("P1/fig3"), figure_named("P1/R1/photo")), (None, None));
        assert_eq!(readable(&format!("{written} 的 C 组")), "Fig. 3 的 C 组");
        // The same figure, the same picture's id; another, another.
        assert_eq!(figure_picture_id(&value), figure_picture_id(&value));
        assert_ne!(figure_picture_id(&value), figure_picture_id(&figure_value("P-17-ab", "R 9/x", key(Kind::Figure, Series::Main, 4))));
        // A paper's key that looks like a kind is not taken for a figure.
        assert_eq!(Keys::default().paper(&Named::Other { kind: FIGURE, value: "x" }), None);
    }

    #[test]
    fn figures_are_found_by_their_number_typed_loosely_and_by_their_caption() {
        use figref::{Kind, Series};
        let key = |kind, series, number| Key { kind, series, number };
        let figures = [(key(Kind::Figure, Series::Main, 1), "Fig. 1", "Overview of the root atlas."), (key(Kind::Figure, Series::Main, 3), "Figure 3", "Trajectories of stem cells in the root tip."), (key(Kind::Table, Series::Main, 2), "Table 2", "Marker genes per cluster."), (key(Kind::Figure, Series::Supplementary, 2), "Fig. S2", "Quality control of the nuclei."), (key(Kind::Figure, Series::Main, 2), "图 2", "根尖细胞的聚类")];
        let found = |query: &str| matching_figures(query, figures.iter().map(|(key, label, legend)| FigureCandidate { key: *key, label, legend }), OFFERED).into_iter().map(|(rank, n)| (rank, figures[n].1)).collect::<Vec<_>>();
        for query in ["fig3", "Fig. 3", "fig 3", "figure 3b", "FIG3"] { assert_eq!(found(query), [(0, "Figure 3")], "{query}") }
        for query in ["table 2", "tab2", "表2", "Table2"] { assert_eq!(found(query), [(0, "Table 2")], "{query}") }
        assert_eq!(found("图2"), [(0, "图 2")]);
        assert_eq!(found("figs2"), [(0, "Fig. S2")]);
        // A kind begun: all of that kind, in their order.
        assert_eq!(found("fig").len(), 4);
        assert_eq!(found("表"), [(0, "Table 2")]);
        // Words of the caption.
        assert_eq!(found("stem cells"), [(1, "Figure 3")]);
        assert_eq!(found("聚类"), [(1, "图 2")]);
        assert_eq!(found("root"), [(1, "Fig. 1"), (1, "Figure 3")]);
        assert!(found("nothing like it").is_empty());
    }

    #[test]
    fn a_figure_goes_to_the_model_with_its_page_caption_and_picture() {
        let shown = FigureSeen { label: "P1".into(), attachment: None, page: 5, key: "Fig. 3".into(), legend: "Trajectories of stem cells.".into(), picture: Ok(2) };
        let apart = FigureSeen { label: "P1".into(), attachment: Some(1), page: 2, key: "Fig. S2".into(), legend: String::new(), picture: Err("文件已经不在了".into()) };
        let find = |named: &Named| match named {
            Named::Other { kind: FIGURE, value: "a" } => Some(Thing::Figure(shown.clone())),
            Named::Other { kind: FIGURE, value: "b" } => Some(Thing::Figure(apart.clone())),
            _ => None,
        };
        let (text, named) = resolve("[@figure:a] 的 C 组和 [@figure:b] 说明了什么？[@figure:a]", find, false);
        assert_eq!(text, "[P1 p.5] 的 Fig. 3 的 C 组和 [P1 S1 p.2] 的 Fig. S2 说明了什么？[P1 p.5] 的 Fig. 3");
        assert_eq!(named.len(), 2);
        let note = note(&named);
        assert!(note.contains("- [P1 p.5] 的 Fig. 3：在 [P1] 的正文第 5 页，引用时写 [P1 p.5]。"), "{note}");
        assert!(note.contains("图注：Fig. 3 Trajectories of stem cells."), "{note}");
        assert!(note.contains("附在这个问题里（第 2 张图片）"), "{note}");
        assert!(note.contains("附件 S1 第 2 页") && note.contains("没能截出这张图的图片（文件已经不在了）"), "{note}");
    }

    #[test]
    fn a_tag_and_a_section_are_told_with_what_reads_them() {
        let papers: Vec<Paper> = (1..=12).map(|n| paper(&n.to_string(), "T", &format!("Zhang{n}, W; Li, N"), Some(2000 + n))).collect();
        let short = Section { id: "S1".into(), name: "引言".into(), kind: "introduction".into(), body: "根尖的干细胞。".into(), ..Default::default() };
        let long = Section { id: "S2".into(), name: "讨论".into(), kind: "discussion".into(), body: "长".repeat(SECTION_TEXT_MAX + 1), ..Default::default() };
        let manuscript = Manuscript { id: "M1".into(), title: "根的综述".into(), sections: vec![short.clone(), long.clone()], ..Default::default() };
        let tag = Thing::Tag { name: "root hairs".into(), papers: papers.iter().enumerate().map(|(n, paper)| (format!("P{}", n + 1), paper)).collect() };
        let find = |named: &Named| match named {
            Named::Other { kind: TAG, value } if decode(value) == "root hairs" => Some(tag.clone()),
            Named::Other { kind: SECTION, value: "S1" } => Some(Thing::Section { manuscript: &manuscript, section: &manuscript.sections[0] }),
            Named::Other { kind: SECTION, value: "S2" } => Some(Thing::Section { manuscript: &manuscript, section: &manuscript.sections[1] }),
            _ => None,
        };
        let asked = format!("{} 和 {} {} 讲了什么 {}", token(TAG, "root hairs"), token(SECTION, "S1"), token(SECTION, "S2"), token(TAG, "gone"));
        let (text, named) = resolve(&asked, find, false);
        assert_eq!(text, "标签「root hairs」（12 篇） 和 稿件「根的综述」的「引言」一节 稿件「根的综述」的「讨论」一节 讲了什么 [@tag:gone]");
        let note = note(&named);
        assert!(note.contains("12 篇文献带这个标签，前 10 篇：[P1] Zhang1 等 2001；"), "{note}");
        assert!(note.contains("[P10] Zhang10 等 2010。其余 2 篇用 list_papers 列出（tag 填 \"root hairs\"）"), "{note}");
        assert!(!note.contains("[P11]"));
        assert!(note.contains("稿件「根的综述」（ID M1）的「引言」一节（ID S1，属于导言）：共 7 字，现在的正文：\n<<<\n根尖的干细胞。\n>>>"), "{note}");
        assert!(note.contains("用 read_manuscript 读（manuscript 填 M1，section 填 S2）"), "{note}");
        assert!(note.contains("不要用 edit_section 改它"));
    }
}
