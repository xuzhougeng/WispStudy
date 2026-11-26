//! Citations in a manuscript: how a paper of the library is cited, finding
//! the citations in a text, and the reference list and BibTeX they make.
//!
//! The written form is Pandoc's: `[@Zhang2024]`, several as
//! `[@Zhang2024; @Li2019b]`, and a locator may follow (`[@Zhang2024, p. 5]`).
//! Chosen because it is plain text that a person reads as "Zhang, 2024",
//! survives being typed, pasted and exported to Markdown unchanged, and is
//! one key per paper, so it resolves by lookup and never by guessing; and
//! because the exported Markdown with the BibTeX of the cited papers (same
//! keys, `bibtex`) is a manuscript Pandoc's citeproc formats in any journal
//! style. A form like `(Zhang et al., 2024)` reads better but names a paper
//! only as long as no other paper of the library has the same first author
//! and year; citations of that older form (`Paper::citation`, what 引用
//! inserted before) are still read, resolved when exactly one paper fits,
//! and can be rewritten into keys (`upgrades`).
//!
//! A key is the first author's family name and the year (`Zhang2024`), the
//! first two words of the title for a paper without authors
//! (`DeNovo2020`), `-nd` for one without a year. Two papers with the same key
//! are told apart by a letter: the paper added first keeps `Zhang2024`, the
//! next is `Zhang2024b`, so adding papers never changes a key already
//! given. A key a citation was written with is kept in the library
//! (`Store::keep_cite_key`), so it stays the paper's when its record is
//! corrected or another paper is removed. Keys are unique ignoring case.
//!
//! What cannot be resolved is reported, not dropped: a key no paper has
//! (`Unknown`), a key whose paper was deleted (`Removed`), a paper in the
//! trash (`Trashed`, still in the reference list), an old-style citation that
//! fits several papers (`Ambiguous`).

use crate::model::Paper;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;

/// How a paper is cited in the text.
pub fn written(key: &str) -> String {
    format!("[@{key}]")
}

/// A chat answer converted for the writing page. Each `(paper_id, key)` must
/// be kept with `Store::keep_cite_key` before saving `text` to a manuscript.
pub struct AiCitationConversion {
    pub text: String,
    pub keys: Vec<(String, String)>,
    /// The sentences the answer quoted for a citation of one paper: what that citation rests on (`passage::quoted`).
    pub quoted: Vec<Quoted>,
    /// The sentences quoted after a key that names no paper of the library (`[@Nobody1999 "…"]`), each with
    /// the key as written (`@Nobody1999`): out of the text as the others are, with no paper to look them up in.
    pub dropped: Vec<(String, String)>,
    /// The citations of the conversation's that stayed in the text as they were written, in its order.
    pub left: Vec<Left>,
}

/// A citation as a conversation writes one, by a label or with its sentence, that stayed as it was written:
/// the text has it, sentence and all.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Left {
    /// As it stands in the text: its brackets and what they hold, or, of a bracket that was not closed,
    /// from the bracket to the end of the sentence it quotes (`[P2 "A sentence."`, `[@Key "A sentence."`).
    pub written: String,
    /// The labels in it that are not one paper of the conversation's which the library has. None when every
    /// label is: then it is how the citation is written that was not read (a bracket left open, words that
    /// are neither a label nor a page nor the sentence).
    pub unknown: Vec<String>,
}

/// A sentence a chat citation quoted: `[P3 p.5 "…"]`, or `[@Key p.5 "…"]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Quoted {
    /// Where the citation stands in the converted text: its opening bracket.
    pub at: usize,
    pub paper_id: String,
    pub page: Option<usize>,
    pub quote: String,
}

/// Convert only complete, unambiguous chat citation groups. Unrecognised
/// text and groups with an unavailable source remain exactly as written,
/// and are handed back in `left`. The sentence a citation quotes
/// (`[P3 p.5 "…"]`) is left out of the text, the page kept (`p.0` is no
/// page): a manuscript cites, and the quotation was the conversation's way
/// of showing where. It is handed back beside the text (`quoted`), for the
/// caller to keep as the passage the citation rests on. With several papers
/// in the brackets (`[P1; P2 p.7 "…"]`) the sentence is the last one's,
/// the paper it was written after, as an answer's is (`ai::citation_spans`).
/// Conversion is not verification of the claim or page.
///
/// Nothing but such a sentence leaves the text. A sentence is what
/// `ai::bracketed` reads as one: it runs to the closing bracket. A model
/// that left the bracket of one citation open (`[P2 "One."。 words
/// [P1 "Two."]`) has written the first as text: it stays, with its sentence
/// and the words after it, and the second is the citation it is.
///
/// A model that has the manuscript's form in mind writes the key where the
/// label goes and the sentence after it all the same: `[@Kaelin2008 "…"]`,
/// `[@Keith2012 p.0 "…"]`. That is read as the label with its sentence is:
/// the citation stays as a manuscript writes it (`[@Kaelin2008]`,
/// `[@Keith2012, p. 5]`; `p.0` is no page), and the sentence is handed back
/// in `quoted`. Left as it was written it would be a paper's sentence in
/// the middle of the manuscript, with nothing kept of where it is from. The
/// sentence leaves the text whatever the key names: after a key no paper
/// has there is nowhere to look it up, so it is handed back in `dropped`,
/// and the citation stays for `find` to report as unknown. Only brackets
/// that begin with a key and end with the quotation are read so: a
/// manuscript's own `[@Key]` and `[see @Key, p. 5]` quote nothing.
pub fn convert_ai_citations(answer: &str, sources: &[crate::ai::Source], index: &Index<'_>) -> AiCitationConversion {
    let mut text = String::new();
    let mut keys: Vec<(String, String)> = Vec::new();
    let mut quoted = Vec::new();
    let mut dropped = Vec::new();
    let mut left = Vec::new();
    // The paper a label names, with its key: one paper of the conversation's, which the library has.
    let named = |label: &str| -> Option<(String, String)> {
        let mut matches = sources.iter().filter(|source| source.label == label);
        let source = matches.next()?;
        if matches.next().is_some() { return None }
        Some((source.paper_id.clone(), index.key(&source.paper_id)?.to_string()))
    };
    // Of the labels of a citation that stays as written, those that name no such paper.
    let unknown = |labels: Vec<&str>| -> Vec<String> {
        let mut unknown: Vec<String> = vec![];
        for label in labels {
            if named(label).is_none() && !unknown.iter().any(|one| one == label) { unknown.push(label.to_string()) }
        }
        unknown
    };
    let open = |stretch: &str| -> Vec<Left> { unclosed(stretch).into_iter().map(|(written, labels)| Left { written: written.to_string(), unknown: unknown(labels.iter().map(String::as_str).collect()) }).collect() };
    // Where the text taken so far ends, and where the last pair of brackets did.
    let (mut end, mut after) = (0, 0);
    let labelled = crate::ai::citation_spans(answer);
    for (range, head, quote) in crate::ai::bracketed(answer) {
        left.extend(open(&answer[after..range.start]));
        after = range.end;
        let Some((_, labels)) = labelled.iter().find(|(span, _)| *span == range) else {
            // `[@Key p.5 "…"]`: a key and the sentence a label would have.
            let Some((written, cited)) = (!quote.is_empty()).then(|| keyed_group(head)).flatten() else { continue };
            text.push_str(&answer[end..range.start]);
            end = range.end;
            // Of several papers the sentence is the last one's, as with labels.
            if let Some((key, page)) = cited.last() {
                // A paper's id stands for its key in what a tool is given (`compose::keyed`).
                match index.key(key).map(|_| key.to_string()).or_else(|| index.resolve(key).paper().map(str::to_string)) {
                    Some(id) => {
                        if let Some(key) = index.key(&id) && !keys.iter().any(|pair| pair == &(id.clone(), key.to_string())) { keys.push((id.clone(), key.to_string())) }
                        quoted.push(Quoted { at: text.len(), paper_id: id, page: *page, quote: quote.to_string() });
                    }
                    None => dropped.push((format!("@{key}"), quote.to_string())),
                }
            }
            text.push_str(&format!("[{written}]"));
            continue;
        };
        text.push_str(&answer[end..range.start]);
        end = range.end;
        // What stands before the sentence is labels and pages and nothing else, and every label names its paper.
        let resolved: Option<Vec<(String, String, Option<usize>)>> = parse_chat_group(head).and_then(|cited| cited.into_iter().map(|(label, page)| named(&label).map(|(id, key)| (id, key, page))).collect());
        let Some(resolved) = resolved else {
            text.push_str(&answer[range.clone()]);
            left.push(Left { written: answer[range].to_string(), unknown: unknown(labels.iter().map(|one| one.label.as_str()).collect()) });
            continue;
        };
        if let (false, Some((id, _, page))) = (quote.is_empty(), resolved.last()) { quoted.push(Quoted { at: text.len(), paper_id: id.clone(), page: *page, quote: quote.to_string() }) }
        let pieces: Vec<String> = resolved.iter().map(|(_, key, page)| match page {
            Some(page) => format!("@{key}, p. {page}"),
            None => format!("@{key}"),
        }).collect();
        text.push_str(&format!("[{}]", pieces.join("; ")));
        for (id, key, _) in resolved {
            if !keys.iter().any(|pair| pair == &(id.clone(), key.clone())) { keys.push((id, key)) }
        }
    }
    left.extend(open(&answer[after..]));
    text.push_str(&answer[end..]);
    AiCitationConversion { text, keys, quoted, dropped, left }
}

/// The citations that a stretch of text between two pairs of brackets
/// begins and does not end: a bracket with a label after it (`[P2`,
/// `[@P2`), or with a key and its sentence (`[@Key p.5 "…"`), and no
/// bracket that closes it where one belongs. Each as it is written, as far
/// as it reads as a citation, with its labels: to the end of the sentence
/// it quotes when labels and pages, or a key, are followed by one on the
/// same line, else the bracket and its first label alone. A key without a
/// sentence is the manuscript's own way of citing, and not looked at.
fn unclosed(text: &str) -> Vec<(&str, Vec<String>)> {
    let mut found = vec![];
    let mut from = 0;
    while let Some(open) = text[from..].find('[').map(|at| from + at) {
        from = open + 1;
        let rest = &text[from..];
        let quoting = rest.find(['"', '“', '「', '[', ']', '\n']).and_then(|at| {
            let cited = match parse_chat_group(&rest[..at]) {
                Some(cited) => cited,
                None => keyed_group(&rest[..at]).map(|_| vec![])?,
            };
            let closing = match rest[at..].chars().next()? {
                '"' => '"',
                '“' => '”',
                '「' => '」',
                _ => return None,
            };
            let start = at + closing.len_utf8();
            let end = start + rest[start..].find([closing, '\n'])?;
            rest[end..].starts_with(closing).then(|| (end + closing.len_utf8(), cited.into_iter().map(|(label, _)| label).collect::<Vec<_>>()))
        });
        let (len, labels) = match quoting {
            Some(read) => read,
            None => {
                let bare = rest.strip_prefix('@').unwrap_or(rest);
                let digits = bare.strip_prefix('P').map_or(0, |after| after.bytes().take_while(u8::is_ascii_digit).count());
                if digits == 0 { continue }
                // `[P2: …`, `[P2a`: not a label.
                if bare[1 + digits..].chars().next().is_some_and(|next| !next.is_whitespace() && ![']', ',', '，', ';', '、'].contains(&next)) { continue }
                (rest.len() - bare.len() + 1 + digits, vec![bare[..1 + digits].to_string()])
            }
        };
        from += len;
        found.push((&text[open..from], labels));
    }
    found
}

/// What stands before the sentence in `[@Key p.5 "…"]`, as a manuscript
/// writes it, and the keys with the page each names. A page and nothing
/// else after a key is written `, p. 5`, and `p.0` is no page; with
/// anything else there (`[@Key, Fig. 2 "…"]`) the words stay as they were
/// written, since nothing but the sentence is to leave the text. None when
/// the brackets do not begin with a key: `[see @Key, p. 5 "…"]` is a
/// person's own way of citing.
fn keyed_group(head: &str) -> Option<(String, Vec<(&str, Option<usize>)>)> {
    let head = head.trim();
    let starts = key_starts(head);
    if starts.first().map(|(at, _)| *at) != Some(0) { return None }
    let parts: Vec<(&str, &str)> = starts.iter().enumerate().map(|(n, (at, key))| (*key, &head[at + 1 + key.len()..starts.get(n + 1).map_or(head.len(), |(next, _)| *next)])).collect();
    let pages: Option<Vec<Option<usize>>> = parts.iter().map(|(_, locator)| page_alone(locator)).collect();
    Some(match pages {
        Some(pages) => {
            let written: Vec<String> = parts.iter().zip(&pages).map(|((key, _), page)| match page {
                Some(page) => format!("@{key}, p. {page}"),
                None => format!("@{key}"),
            }).collect();
            (written.join("; "), parts.iter().zip(pages).map(|((key, _), page)| (*key, page)).collect())
        }
        None => (head.to_string(), parts.iter().map(|(key, locator)| (*key, page_of(locator))).collect()),
    })
}

/// What follows a key when it is a page and nothing else (`p.5`, `, p. 5`,
/// `p5`) or nothing at all: the page, none for `p.0`.
fn page_alone(locator: &str) -> Option<Option<usize>> {
    let words: Vec<String> = locator.split(|c: char| c.is_whitespace() || [',', '，', ';', '；'].contains(&c)).filter(|word| !word.is_empty()).map(str::to_lowercase).collect();
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let digits = match words[..] {
        [] => return Some(None),
        ["p." | "p", digits] => digits,
        [word] => word.strip_prefix("p.").or_else(|| word.strip_prefix('p'))?,
        _ => return None,
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) { return None }
    Some(digits.parse::<usize>().ok().filter(|page| *page > 0))
}

/// What a citation group names before the sentence it quotes, or in all it
/// holds when it quotes none: labels, each with its page or without. The
/// deliberately narrow grammar has no free text, and no quotation but the
/// sentence the brackets end with (`ai::bracketed`), so converting a group
/// cannot silently drop a qualifier or source detail. `p.0` is no page, as
/// after a key (`page_alone`).
fn parse_chat_group(inner: &str) -> Option<Vec<(String, Option<usize>)>> {
    if inner.contains(['[', ']', '\n', '"', '“', '”', '「', '」']) { return None }
    let mut citations: Vec<(String, Option<usize>)> = Vec::new();
    let parse_page = |word: &str| -> Option<Option<usize>> {
        let digits = word.strip_prefix("p.").or_else(|| word.strip_prefix('p'))?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) { return None }
        let page = digits.parse::<usize>().ok()?;
        Some((page > 0).then_some(page))
    };
    for part in inner.split([',', '，', ';', '、']) {
        let mut words = part.split_whitespace();
        let first = words.next()?;
        // `[@P3 p.5]` is `[P3 p.5]` (`ai::citation_spans`).
        let first = first.strip_prefix('@').unwrap_or(first);
        if first.starts_with('P') && first.len() > 1 && first[1..].bytes().all(|b| b.is_ascii_digit()) {
            let page = match words.next() { Some(word) => parse_page(word)?, None => None };
            if words.next().is_some() { return None }
            citations.push((first.to_string(), page));
        } else {
            let page = parse_page(first)?;
            if words.next().is_some() { return None }
            let last = citations.last_mut()?;
            if last.1.is_some() { return None }
            last.1 = page;
        }
    }
    (!citations.is_empty()).then_some(citations)
}

/// Every paper of a library with its key: the open papers and those in the
/// trash, and the keys kept from earlier citations (key → paper id).
pub struct Index<'a> {
    papers: HashMap<&'a str, (&'a Paper, bool)>,
    key_of: HashMap<&'a str, String>,
    /// By the key in lower case: the paper id, which may be one no longer in the library.
    by_key: HashMap<String, String>,
}

impl<'a> Index<'a> {
    pub fn new(papers: impl IntoIterator<Item = &'a Paper>, trashed: impl IntoIterator<Item = &'a Paper>, kept: &BTreeMap<String, String>) -> Index<'a> {
        let mut all: HashMap<&str, (&Paper, bool)> = HashMap::new();
        for paper in papers { all.insert(paper.id.as_str(), (paper, false)); }
        for paper in trashed { all.insert(paper.id.as_str(), (paper, true)); }
        let mut by_key: HashMap<String, String> = HashMap::new();
        let mut key_of: HashMap<&str, String> = HashMap::new();
        for (key, id) in kept {
            if by_key.contains_key(&key.to_lowercase()) { continue }
            by_key.insert(key.to_lowercase(), id.clone());
            if let Some((paper, _)) = all.get(id.as_str()) { key_of.entry(paper.id.as_str()).or_insert_with(|| key.clone()); }
        }
        // The rest in the order they came into the library: a later paper never takes a key from an earlier one.
        let mut rest: Vec<&Paper> = all.values().map(|(paper, _)| *paper).filter(|paper| !key_of.contains_key(paper.id.as_str())).collect();
        rest.sort_by(|a, b| (a.added_at.as_str(), a.id.as_str()).cmp(&(b.added_at.as_str(), b.id.as_str())));
        for paper in rest {
            let base = base_key(paper);
            let key = (1..).map(|n| suffixed(&base, n)).find(|key| !by_key.contains_key(&key.to_lowercase())).unwrap();
            by_key.insert(key.to_lowercase(), paper.id.clone());
            key_of.insert(paper.id.as_str(), key);
        }
        Index { papers: all, key_of, by_key }
    }

    /// The key a paper is cited by.
    pub fn key(&self, paper_id: &str) -> Option<&str> {
        self.key_of.get(paper_id).map(String::as_str)
    }

    pub fn paper(&self, paper_id: &str) -> Option<&'a Paper> {
        self.papers.get(paper_id).map(|(paper, _)| *paper)
    }

    /// The paper a key is taken by, letters compared without their case: one of the library, or one a kept key
    /// still names though it is gone. None for a key that is free.
    pub fn holder(&self, key: &str) -> Option<&str> {
        self.by_key.get(&key.to_lowercase()).map(String::as_str)
    }

    fn resolve(&self, key: &str) -> Found {
        match self.by_key.get(&key.to_lowercase()) {
            None => Found::Unknown,
            Some(id) => match self.papers.get(id.as_str()) {
                None => Found::Removed,
                Some((_, true)) => Found::Trashed(id.clone()),
                Some((_, false)) => Found::Paper(id.clone()),
            },
        }
    }

    /// The papers an old-style citation can mean: `Zhang et al.`, `Zhang and Li`, `Zhang`, with the year.
    fn fitting(&self, who: &Who, year: i32) -> Vec<String> {
        let candidates: Vec<&Paper> = self.papers.values().map(|(paper, _)| *paper).filter(|paper| paper.year == Some(year)).collect();
        let strict: Vec<&Paper> = candidates.iter().copied().filter(|paper| who.fits(paper, true)).collect();
        let found = if strict.is_empty() { candidates.into_iter().filter(|paper| who.fits(paper, false)).collect() } else { strict };
        let mut ids: Vec<String> = found.into_iter().map(|paper| paper.id.clone()).collect();
        ids.sort_by_key(|id| self.key(id).unwrap_or("").to_string());
        ids
    }

    /// The papers whose author, title, key or year has every word of a query, best first: for citing a paper by typing part of it.
    pub fn matching(&self, query: &str, limit: usize) -> Vec<&'a Paper> {
        let words: Vec<String> = query.split_whitespace().map(plain).filter(|word| !word.is_empty()).collect();
        if words.is_empty() { return vec![] }
        let mut found: Vec<(u8, &Paper)> = self
            .papers
            .values()
            .filter(|(_, trashed)| !trashed)
            .filter_map(|(paper, _)| {
                let first = families(&paper.authors).into_iter().next().map(|name| plain(&name)).unwrap_or_default();
                let key = self.key(&paper.id).map(plain).unwrap_or_default();
                let haystack = format!("{} {} {} {key}", plain(&paper.authors), plain(&paper.title), paper.year.map(|y| y.to_string()).unwrap_or_default());
                if !words.iter().all(|word| haystack.contains(word.as_str())) { return None }
                // The first author or the key named first is what a person types who knows the paper.
                let rank = if !first.is_empty() && first.starts_with(words[0].as_str()) || key.starts_with(words[0].as_str()) { 0 } else { 1 };
                Some((rank, *paper))
            })
            .collect();
        found.sort_by(|(ra, a), (rb, b)| ra.cmp(rb).then(b.year.cmp(&a.year)).then(a.title.cmp(&b.title)));
        found.into_iter().take(limit).map(|(_, paper)| paper).collect()
    }
}

/// What a citation names.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    Paper(String),
    /// A paper in the trash: cited, listed, and pointed out.
    Trashed(String),
    /// A key a paper had that has since been deleted.
    Removed,
    /// No paper has this key, or fits this citation.
    Unknown,
    /// An old-style citation several papers fit, best first.
    Ambiguous(Vec<String>),
}

impl Found {
    pub fn paper(&self) -> Option<&str> {
        match self {
            Found::Paper(id) | Found::Trashed(id) => Some(id),
            _ => None,
        }
    }
}

/// One citation in a text.
#[derive(Clone, Debug, PartialEq)]
pub struct Cited {
    /// Where it is written: `@Zhang2024`, or `Zhang et al., 2024` in an old-style one.
    pub range: Range<usize>,
    pub written: String,
    /// The brackets or parentheses it stands in, with the others cited there.
    pub group: Range<usize>,
    /// Old style: `(Zhang et al., 2024)`, not a key.
    pub old: bool,
    pub found: Found,
}

/// The citations of a text in the order they are written: the keys in
/// brackets, and the old author-year form in parentheses.
pub fn find(text: &str, index: &Index) -> Vec<Cited> {
    let mut found = keyed(text, index);
    let taken: Vec<Range<usize>> = found.iter().map(|cited| cited.group.clone()).collect();
    found.extend(old_style(text, index).into_iter().filter(|cited| !taken.iter().any(|group| group.start < cited.group.end && cited.group.start < group.end)));
    found.sort_by_key(|cited| cited.range.start);
    found
}

/// `[… @key …; @key …]`: Pandoc's bracketed citations.
fn keyed(text: &str, index: &Index) -> Vec<Cited> {
    let mut out = vec![];
    let mut from = 0;
    while let Some(open) = text[from..].find('[').map(|at| from + at) {
        let Some(close) = text[open + 1..].find([']', '[', '\n']).map(|at| open + 1 + at) else { break };
        if !text[close..].starts_with(']') {
            from = close;
            continue;
        }
        let group = open..close + 1;
        for (at, key) in key_starts(&text[open + 1..close]) {
            let range = open + 1 + at..open + 2 + at + key.len();
            out.push(Cited { written: text[range.clone()].to_string(), range, group: group.clone(), old: false, found: index.resolve(key) });
        }
        from = close + 1;
    }
    out
}

/// Where the keys stand in what a pair of brackets holds, each `@` with the
/// key after it: at the beginning, or after a space, a `;` or a `-`.
fn key_starts(inner: &str) -> Vec<(usize, &str)> {
    let mut keys = vec![];
    let mut previous: Option<char> = None;
    for (at, c) in inner.char_indices() {
        if c == '@' && previous.is_none_or(|p| p.is_whitespace() || p == ';' || p == '-') {
            let key = key_at(&inner[at + 1..]);
            if !key.is_empty() { keys.push((at, key)) }
        }
        previous = Some(c);
    }
    keys
}

/// A key as Pandoc reads it: letters, digits and `_`, with punctuation only between them.
fn key_at(text: &str) -> &str {
    let mut end = 0;
    let mut chars = text.char_indices().peekable();
    if !chars.peek().is_some_and(|(_, c)| c.is_alphanumeric() || *c == '_') { return "" }
    while let Some((at, c)) = chars.next() {
        if c.is_alphanumeric() || c == '_' {
            end = at + c.len_utf8();
        } else if ":.#$%&-+?<>~/".contains(c) && chars.peek().is_some_and(|(_, next)| next.is_alphanumeric() || *next == '_') {
            continue;
        } else {
            break;
        }
    }
    &text[..end]
}

/// Who an old-style citation names.
struct Who {
    /// The words before `et al.`, `and`, or the comma: the first author's family name (possibly after "see").
    first: Vec<String>,
    second: Option<Vec<String>>,
    /// `et al.`: three authors or more.
    more: bool,
}

impl Who {
    fn read(text: &str) -> Option<Who> {
        let text = text.trim();
        let words = |text: &str| -> Vec<String> { text.split_whitespace().map(plain).filter(|word| !word.is_empty()).collect() };
        for end in ["et al.", "et al", "等"] {
            if let Some(first) = text.strip_suffix(end) { return Some(Who { first: words(first), second: None, more: true }).filter(|who| !who.first.is_empty()) }
        }
        for and in [" and ", " & ", "和", "与"] {
            if let Some((first, second)) = text.split_once(and) { return Some(Who { first: words(first), second: Some(words(second)), more: false }).filter(|who| !who.first.is_empty()) }
        }
        Some(Who { first: words(text), second: None, more: false }).filter(|who| !who.first.is_empty())
    }

    /// Strictly: the authors counted as the citation counts them (`Paper::citation`). Loosely: the first author only.
    fn fits(&self, paper: &Paper, strict: bool) -> bool {
        let names = families(&paper.authors);
        let ends = |words: &[String], name: &str| {
            let name: Vec<String> = name.split_whitespace().map(plain).filter(|w| !w.is_empty()).collect();
            !name.is_empty() && words.ends_with(&name)
        };
        if names.is_empty() {
            // `Paper::citation` cites a paper without authors by the first three words of its title.
            let title: Vec<String> = paper.title.split_whitespace().take(3).map(plain).filter(|w| !w.is_empty()).collect();
            return !title.is_empty() && self.first == title;
        }
        if !ends(&self.first, &names[0]) { return false }
        if let Some(second) = &self.second {
            return names.len() == 2 && second.join(" ") == names[1].split_whitespace().map(plain).collect::<Vec<_>>().join(" ");
        }
        !strict || if self.more { names.len() >= 3 } else { names.len() == 1 }
    }
}

/// `(Zhang et al., 2021; Li, 2019)`: the form `Paper::citation` writes, typed by hand or left by an older build.
fn old_style(text: &str, index: &Index) -> Vec<Cited> {
    let mut out = vec![];
    let mut from = 0;
    while let Some((open, width)) = text[from..].char_indices().find(|(_, c)| *c == '(' || *c == '（').map(|(at, c)| (from + at, c.len_utf8())) {
        let Some((close, close_width)) = text[open + width..].char_indices().find(|(_, c)| matches!(c, ')' | '）' | '(' | '（' | '\n')).map(|(at, c)| (open + width + at, c.len_utf8())) else { break };
        if !matches!(text[close..].chars().next(), Some(')' | '）')) {
            from = close;
            continue;
        }
        let group = open..close + close_width;
        let inner_start = open + width;
        let mut parts = vec![];
        let mut start = inner_start;
        for (at, c) in text[inner_start..close].char_indices().chain([(close - inner_start, ';')]) {
            if c != ';' && c != '；' { continue }
            let end = inner_start + at;
            parts.push(start..end);
            start = end + c.len_utf8();
        }
        let read: Option<Vec<(Range<usize>, Who, i32)>> = parts
            .into_iter()
            .map(|part| {
                let piece = &text[part.clone()];
                let (who, year) = piece.rsplit_once([',', '，'])?;
                let year = year.trim();
                let digits = year.trim_end_matches(|c: char| c.is_ascii_lowercase());
                if digits.len() != 4 || !digits.chars().all(|c| c.is_ascii_digit()) || year.len() > 5 { return None }
                if !who.trim().starts_with(char::is_alphabetic) || who.contains(|c: char| c.is_ascii_digit() || c == '=') { return None }
                let skip = piece.len() - piece.trim_start().len();
                let range = part.start + skip..part.start + piece.trim_end().len();
                Some((range, Who::read(who)?, digits.parse().ok()?))
            })
            .collect();
        for (range, who, year) in read.unwrap_or_default() {
            let found = match index.fitting(&who, year).as_slice() {
                [] => Found::Unknown,
                [one] => index.resolve(index.key(one).unwrap_or("")),
                many => Found::Ambiguous(many.to_vec()),
            };
            out.push(Cited { written: text[range.clone()].to_string(), range, group: group.clone(), old: true, found });
        }
        from = close + close_width;
    }
    out
}

/// The old-style citations that can be written as keys: each parenthesis
/// whose every part names one paper, with what replaces it. Last first, so
/// they can be applied in that order without moving the ones still to come.
pub fn upgrades(cited: &[Cited], index: &Index) -> Vec<(Range<usize>, String)> {
    let mut groups: Vec<(Range<usize>, Vec<&Cited>)> = vec![];
    for one in cited.iter().filter(|one| one.old) {
        match groups.last_mut() {
            Some((group, parts)) if *group == one.group => parts.push(one),
            _ => groups.push((one.group.clone(), vec![one])),
        }
    }
    let mut out: Vec<(Range<usize>, String)> = groups
        .into_iter()
        .filter_map(|(group, parts)| {
            let keys: Option<Vec<String>> = parts.iter().map(|part| part.found.paper().and_then(|id| index.key(id)).map(|key| format!("@{key}"))).collect();
            Some((group, format!("[{}]", keys?.join("; "))))
        })
        .collect();
    out.reverse();
    out
}

/// One entry of the reference list.
#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    pub key: String,
    pub paper_id: String,
    pub text: String,
    pub trashed: bool,
}

/// Each paper the citations name, once, in the order of an author-year
/// list: by the first author's family name (the title for a paper without
/// authors), then the year, then the key.
pub fn references(cited: &[Cited], index: &Index) -> Vec<Reference> {
    let mut seen = HashSet::new();
    let mut out: Vec<(String, Option<i32>, Reference)> = vec![];
    for one in cited {
        let Some(id) = one.found.paper() else { continue };
        if !seen.insert(id.to_string()) { continue }
        let (Some(paper), Some(key)) = (index.paper(id), index.key(id)) else { continue };
        let sort = families(&paper.authors).into_iter().next().unwrap_or_else(|| paper.title.clone());
        out.push((plain(&sort), paper.year, Reference { key: key.to_string(), paper_id: id.to_string(), text: reference(paper), trashed: matches!(one.found, Found::Trashed(_)) }));
    }
    out.sort_by(|a, b| (a.0.as_str(), a.1.is_none(), a.1, a.2.key.to_lowercase()).cmp(&(b.0.as_str(), b.1.is_none(), b.1, b.2.key.to_lowercase())));
    out.into_iter().map(|(_, _, reference)| reference).collect()
}

/// The list as plain text, one entry a line, for copying.
pub fn reference_list(references: &[Reference]) -> String {
    references.iter().map(|reference| reference.text.as_str()).collect::<Vec<_>>().join("\n")
}

/// The BibTeX of the cited papers, under the keys they are cited by.
pub fn bibtex(references: &[Reference], index: &Index) -> String {
    let entries: Vec<(&str, &Paper)> = references.iter().filter_map(|reference| Some((reference.key.as_str(), index.paper(&reference.paper_id)?))).collect();
    crate::export::bibtex_keyed(&entries)
}

/// One entry, from what the record has: `Zhang, W., & Li, Q. (2024). Title. Journal, 10(3), 357–362. https://doi.org/…`.
/// Volume, issue and pages are written when the record has them.
pub fn reference(paper: &Paper) -> String {
    reference_dated(paper, &paper.year.map_or("n.d.".to_string(), |year| year.to_string()))
}

/// `reference` with the year written as given: `2024a` when two entries would otherwise read alike.
fn reference_dated(paper: &Paper, year: &str) -> String {
    let names = authors_shown(&paper.authors);
    let authors = match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        many if many.len() > 20 => format!("{}, … {}", many[..19].join(", "), many[many.len() - 1]),
        [rest @ .., last] => format!("{}, & {last}", rest.join(", ")),
    };
    let title = paper.title.trim();
    let title = if title.ends_with(['.', '?', '!', '。', '？']) { title.to_string() } else { format!("{title}.") };
    let mut out = if authors.is_empty() { format!("{title} ({year}).") } else { format!("{authors} ({year}). {title}") };
    let journal = paper.journal.trim();
    if !journal.is_empty() {
        // `Journal, 10(3), 357–362.`: each part only when the record has it.
        let placed = paper.placed();
        if placed.is_empty() {
            out.push_str(&format!(" {}{}", journal, if journal.ends_with('.') { "" } else { "." }));
        } else {
            out.push_str(&format!(" {journal}, {placed}."));
        }
    } else if paper.publication_status == "preprint" {
        out.push_str(" Preprint.");
    }
    if !paper.doi.is_empty() {
        out.push_str(&format!(" https://doi.org/{}", paper.doi));
    } else if paper.url.starts_with("http") {
        out.push_str(&format!(" {}", paper.url));
    } else if !paper.pmid.is_empty() {
        out.push_str(&format!(" PMID: {}", paper.pmid));
    }
    out
}

// ---- what a citation says besides its key

/// What a citation writes after its key: the page it points to and the
/// sentence of the paper it quotes, as in `[@Xue2025 p.2 "a core set …"]`
/// or `[@Zhang2024, p. 5]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Said {
    pub page: Option<usize>,
    pub quote: Option<String>,
}

/// What follows a citation's key up to the next key of its group (`;`
/// outside a quotation) or the closing bracket. Nothing for an old-style one.
pub fn said(text: &str, cited: &Cited) -> Said {
    if cited.old || cited.range.end > text.len() || cited.group.end > text.len() { return Said::default() }
    let rest = &text[cited.range.end..cited.group.end.saturating_sub(1).max(cited.range.end)];
    let mut quoting: Option<char> = None;
    let mut end = rest.len();
    for (at, c) in rest.char_indices() {
        match quoting {
            Some(close) if c == close => quoting = None,
            Some(_) => {}
            None if c == '"' => quoting = Some('"'),
            None if c == '“' => quoting = Some('”'),
            None if c == '「' => quoting = Some('」'),
            None if c == ';' || c == '；' => {
                end = at;
                break;
            }
            None => {}
        }
    }
    let piece = &rest[..end];
    let open = piece.find(['"', '“', '「']);
    let quote = open.and_then(|at| {
        let first = piece[at..].chars().next()?;
        let close = match first { '“' => '”', '「' => '」', _ => '"' };
        let inner = &piece[at + first.len_utf8()..];
        let inner = inner.rfind(close).map_or(inner, |to| &inner[..to]).trim();
        (!inner.is_empty()).then(|| inner.to_string())
    });
    Said { page: page_of(&piece[..open.unwrap_or(piece.len())]), quote }
}

/// The first page a locator names: `p. 5`, `p.5`, `pp. 3–4`, `, p 12`.
fn page_of(locator: &str) -> Option<usize> {
    let words: Vec<&str> = locator.split([' ', ',', '，']).filter(|word| !word.is_empty()).collect();
    let digits = |word: &str| -> Option<usize> {
        let lead: String = word.chars().take_while(char::is_ascii_digit).collect();
        lead.parse().ok().filter(|page| *page > 0)
    };
    for (n, word) in words.iter().enumerate() {
        let word = word.to_lowercase();
        if ["p", "p.", "pp", "pp."].contains(&word.as_str()) { return words.get(n + 1).and_then(|next| digits(next)) }
        if let Some(rest) = word.strip_prefix("pp.").or_else(|| word.strip_prefix("p.")) { return digits(rest) }
        if let Some(rest) = word.strip_prefix('p') && rest.starts_with(|c: char| c.is_ascii_digit()) { return digits(rest) }
    }
    None
}

// ---- how a manuscript leaving the app cites

/// How the citations of a manuscript exported to Word are written. In the
/// app they stay keys (`[@Zhang2024]`), which find their paper by lookup;
/// a reader of the exported file sees a journal's form and a reference list.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Style {
    /// `(Zhang et al., 2024)`, the list by author and year.
    #[default]
    AuthorYear,
    /// `[1]`, numbered in the order first cited, the list in that order.
    Numeric,
    /// A journal's own form, as a CSL file defines it (`csl`).
    Csl(std::sync::Arc<crate::csl::Loaded>),
}

impl Style {
    /// The two simple styles as kept in the settings: `numeric`, else author-year. A CSL style is read by `csl::style`.
    pub fn from_key(key: &str) -> Style {
        if key == "numeric" { Style::Numeric } else { Style::AuthorYear }
    }

    pub fn key(&self) -> &str {
        match self {
            Style::AuthorYear => "author-year",
            Style::Numeric => "numeric",
            Style::Csl(style) => &style.key,
        }
    }
}

/// A text as a Word file sets it: what is in italics, bold or raised, and
/// which of its words are a citation of which paper (by the paper's id),
/// so the file can lead from a citation to its entry in the list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rich {
    pub text: String,
    pub looks: Vec<(Range<usize>, crate::docx::Look)>,
    pub links: Vec<(Range<usize>, String)>,
}

impl Rich {
    pub(crate) fn push(&mut self, text: &str) {
        self.text.push_str(text);
    }

    /// Text that cites a paper.
    fn push_linked(&mut self, text: &str, paper_id: &str) {
        let start = self.text.len();
        self.text.push_str(text);
        self.links.push((start..self.text.len(), paper_id.to_string()));
    }

    /// The same text for a place that draws nothing raised: what is raised is written in raised characters
    /// (`¹²³`, a range's dash as `⁻`), every look and link moved to where its words now are, and the raised
    /// ones no longer marked as such. A raised stretch with a character that has no raised form (the brackets
    /// of GB/T 7714's `[1]`) stays on the line whole, as a manuscript being written shows it (`labels`).
    pub fn raised_as_characters(&self) -> Rich {
        let writable = |range: &Range<usize>| self.text.get(range.clone()).is_some_and(|words| words.chars().all(|ch| ch.is_ascii_digit() || "-–−(), ".contains(ch)));
        let raised = |at: usize| self.looks.iter().any(|(range, look)| look.sup && range.contains(&at) && writable(range));
        let mut text = String::with_capacity(self.text.len());
        // Where each byte of the text as it was now stands; the last is its end.
        let mut now = vec![0; self.text.len() + 1];
        for (at, ch) in self.text.char_indices() {
            for place in &mut now[at..at + ch.len_utf8()] { *place = text.len() }
            text.push(match ch {
                _ if !raised(at) => ch,
                '0' => '⁰',
                '1' => '¹',
                '2' => '²',
                '3' => '³',
                '4'..='9' => char::from_u32(0x2070 + ch as u32 - '0' as u32).unwrap_or(ch),
                '-' | '–' | '−' => '⁻',
                '(' => '⁽',
                ')' => '⁾',
                _ => ch,
            });
        }
        now[self.text.len()] = text.len();
        let moved = |range: &Range<usize>| now.get(range.start).copied().unwrap_or(text.len())..now.get(range.end).copied().unwrap_or(text.len());
        let looks = self.looks.iter().map(|(range, look)| (moved(range), crate::docx::Look { sup: false, ..*look })).filter(|(_, look)| *look != crate::docx::Look::default()).collect();
        let links = self.links.iter().map(|(range, to)| (moved(range), to.clone())).collect();
        Rich { text, looks, links }
    }

    pub(crate) fn append(&mut self, other: &Rich) {
        let base = self.text.len();
        self.text.push_str(&other.text);
        self.looks.extend(other.looks.iter().map(|(range, look)| (base + range.start..base + range.end, *look)));
        self.links.extend(other.links.iter().map(|(range, to)| (base + range.start..base + range.end, to.clone())));
    }

    /// Without the blanks around it: what is set apart and what cites stays on its words.
    pub(crate) fn trimmed(self) -> Rich {
        let lead = self.text.len() - self.text.trim_start().len();
        let text = self.text.trim().to_string();
        let within = |range: Range<usize>| range.start.saturating_sub(lead).min(text.len())..range.end.saturating_sub(lead).min(text.len());
        let looks = self.looks.into_iter().map(|(range, look)| (within(range), look)).filter(|(range, _)| !range.is_empty()).collect();
        let links = self.links.into_iter().map(|(range, to)| (within(range), to)).filter(|(range, _)| !range.is_empty()).collect();
        Rich { text, looks, links }
    }
}

/// The citations of a manuscript written in a `Style`: what each cited
/// paper is called in the text and its entry in the reference list. Made
/// once from every section, so the numbers and the letters that tell two
/// papers apart run through the whole manuscript. Only a group whose every
/// key names a paper is rewritten; one with a key nothing has stays as
/// written, so a reader of the file sees that something is missing.
pub struct Formatter {
    style: Style,
    chinese: bool,
    /// Paper id → `Zhang et al., 2024a` (author-year) or its number.
    labels: HashMap<String, String>,
    numbers: HashMap<String, usize>,
    /// The reference list in the style's order: the paper, and its entry.
    list: Vec<(String, Rich)>,
    /// A CSL style: how each group of papers is written, by the papers it cites in the order written. A group
    /// cited again reads as it did the first time, also in a style that would shorten a later citation.
    groups: HashMap<Vec<String>, Rich>,
}

/// The papers a group cites, each once in the order written; none when a key of it names no paper.
fn cited_ids(group: &[Cited], known: impl Fn(&str) -> bool) -> Option<Vec<&str>> {
    let mut ids: Vec<&str> = group.iter().map(|one| one.found.paper().filter(|id| known(id))).collect::<Option<_>>()?;
    let mut seen = HashSet::new();
    ids.retain(|id| seen.insert(*id));
    Some(ids)
}

impl Formatter {
    pub fn new(texts: &[&str], index: &Index, style: &Style, language: &str) -> Formatter {
        let chinese = language == "zh";
        // Only the groups that will be written count: a paper cited beside a key nothing has gets no number and no entry from that group.
        let groups: Vec<Vec<Cited>> = texts.iter().flat_map(|text| written_groups(text, index)).filter(|group| group.iter().all(|one| one.found.paper().is_some_and(|id| index.paper(id).is_some()))).collect();
        let cited: Vec<Cited> = groups.iter().flatten().cloned().collect();
        let mut numbers: HashMap<String, usize> = HashMap::new();
        let mut order: Vec<String> = vec![];
        for one in &cited {
            let Some(id) = one.found.paper() else { continue };
            if index.paper(id).is_none() || numbers.contains_key(id) { continue }
            order.push(id.to_string());
            numbers.insert(id.to_string(), order.len());
        }
        let mut labels = HashMap::new();
        let mut written = HashMap::new();
        let plain = |text: String| Rich { text, ..Default::default() };
        let list = match style {
            Style::Numeric => order.iter().filter_map(|id| Some((id.clone(), plain(format!("[{}] {}", numbers[id], reference(index.paper(id)?)))))).collect(),
            Style::AuthorYear => {
                let listed = references(&cited, index);
                // Two papers that would read alike, `Zhang et al., 2024`, are 2024a and 2024b, in the list's order.
                let mut alike: HashMap<(String, String), Vec<String>> = HashMap::new();
                for one in &listed {
                    let Some(paper) = index.paper(&one.paper_id) else { continue };
                    alike.entry((who(paper), year_of(paper))).or_default().push(one.paper_id.clone());
                }
                let mut out = vec![];
                for one in &listed {
                    let Some(paper) = index.paper(&one.paper_id) else { continue };
                    let (name, year) = (who(paper), year_of(paper));
                    let same = &alike[&(name.clone(), year.clone())];
                    let year = match same.iter().position(|id| *id == one.paper_id) {
                        Some(n) if same.len() > 1 && n < 26 => format!("{year}{}", (b'a' + n as u8) as char),
                        _ => year,
                    };
                    labels.insert(one.paper_id.clone(), format!("{name}, {year}"));
                    out.push((one.paper_id.clone(), plain(reference_dated(paper, &year))));
                }
                out
            }
            Style::Csl(csl) => {
                // Every group as the processor is to see it: in the order written, the ones with a key nothing has left out.
                let asked: Vec<Vec<&str>> = groups.iter().filter_map(|group| cited_ids(group, |id| numbers.contains_key(id))).collect();
                let papers: Vec<Vec<&Paper>> = asked.iter().map(|ids| ids.iter().filter_map(|id| index.paper(id)).collect()).collect();
                let (cited, list) = crate::csl::render(csl, &papers, language);
                for (ids, rich) in asked.iter().zip(cited) { written.entry(ids.iter().map(|id| id.to_string()).collect()).or_insert(rich); }
                list
            }
        };
        Formatter { style: style.clone(), chinese, labels, numbers, list, groups: written }
    }

    /// How a group that cites these papers is written.
    fn written(&self, ids: &[&str]) -> Option<Rich> {
        let mut out = Rich::default();
        match &self.style {
            Style::Numeric => {
                let mut numbered: Vec<(usize, &str)> = ids.iter().map(|id| (self.numbers[*id], *id)).collect();
                numbered.sort_unstable();
                out.push("[");
                // `1, 3–5, 7`: three numbers in a row or more as a range, which names its two ends.
                let mut at = 0;
                while at < numbered.len() {
                    let mut end = at;
                    while end + 1 < numbered.len() && numbered[end + 1].0 == numbered[end].0 + 1 { end += 1 }
                    if at > 0 { out.push(", ") }
                    out.push_linked(&numbered[at].0.to_string(), numbered[at].1);
                    if end > at {
                        out.push(if end - at == 1 { ", " } else { "–" });
                        out.push_linked(&numbered[end].0.to_string(), numbered[end].1);
                    }
                    at = end + 1;
                }
                out.push("]");
            }
            Style::AuthorYear => {
                out.push(if self.chinese { "（" } else { "(" });
                for (n, id) in ids.iter().filter(|id| self.labels.contains_key(**id)).enumerate() {
                    if n > 0 { out.push(if self.chinese { "；" } else { "; " }) }
                    out.push_linked(&self.labels[*id], id);
                }
                out.push(if self.chinese { "）" } else { ")" });
            }
            Style::Csl(_) => return self.groups.get(&ids.iter().map(|id| id.to_string()).collect::<Vec<_>>()).cloned(),
        }
        Some(out)
    }

    /// `text` with each citation group that names papers written in the
    /// style, the rest as it was; with what the style sets apart, and
    /// which words cite which paper.
    pub fn rich(&self, text: &str, index: &Index) -> Rich {
        let (mut out, mut at) = (Rich::default(), 0);
        for group in written_groups(text, index) {
            let Some(first) = group.first() else { continue };
            let Some(written) = cited_ids(&group, |id| self.numbers.contains_key(id)).and_then(|ids| self.written(&ids)) else { continue };
            out.push(&text[at..first.group.start]);
            out.append(&written);
            at = first.group.end;
        }
        out.push(&text[at..]);
        out
    }

    /// How each group of `text` that is written for a reader reads, by where the group is written: what a
    /// manuscript being written shows in a group's place while the caret is not in it. A group a style raises
    /// (a numbered journal style) is put in brackets, as it stands on the line there. A group with a key
    /// nothing has is not among them: it stays as written.
    pub fn labels(&self, text: &str, index: &Index) -> Vec<(Range<usize>, String)> {
        written_groups(text, index)
            .into_iter()
            .filter_map(|group| {
                let place = group.first()?.group.clone();
                let rich = cited_ids(&group, |id| self.numbers.contains_key(id)).and_then(|ids| self.written(&ids))?;
                let raised = !rich.looks.is_empty() && rich.looks.iter().all(|(_, look)| look.sup);
                let words = rich.text.trim();
                Some((place, if raised { format!("[{words}]") } else { words.to_string() }))
            })
            .collect()
    }

    /// `rich` as words alone.
    pub fn apply(&self, text: &str, index: &Index) -> String {
        self.rich(text, index).text
    }

    /// The reference list in the style's order: each paper's id and its entry.
    pub fn references(&self) -> &[(String, Rich)] {
        &self.list
    }

    /// What the list is headed: in the manuscript's language.
    pub fn heading(&self) -> &'static str {
        if self.chinese { "参考文献" } else { "References" }
    }
}

/// A style shown on works of its own, before it is chosen: a sentence that cites them as a reader of the file
/// reads it, and their entries in the list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    pub sentence: Rich,
    pub list: Vec<Rich>,
}

/// `style` tried on three works that tell styles apart: one of seven authors, one of two, the first with two
/// given names, and a Chinese one of three (where styles differ in when they write `et al.` and in what
/// follows an initial), each with its volume, issue and pages; cited alone and as a group. The sentence is
/// in English: what is shown is the style, and the locale of a manuscript in Chinese changes only its `等`.
pub fn sample(style: &Style) -> Sample {
    let paper = |id: &str, authors: &str, year: i32, title: &str, journal: &str, placed: (&str, &str, &str)| Paper { id: id.into(), authors: authors.into(), year: Some(year), title: title.into(), journal: journal.into(), volume: placed.0.into(), issue: placed.1.into(), pages: placed.2.into(), added_at: id.into(), ..Default::default() };
    let papers = [
        paper("1", "Zhang, Wei; Li, Qiang; Wang, Fang; Smith, John; Müller, Anna; Tanaka, Hiroshi; Garcia, Maria", 2024, "A single-cell atlas of root development", "Nature Plants", ("10", "3", "357-362")),
        paper("2", "Smith, John A.; Jones, Mary", 2019, "Auxin transport shapes the root tip", "The Plant Cell", ("31", "4", "1021-1033")),
        paper("3", "王芳; 李强; 张伟", 2021, "水稻根系发育的单细胞转录组研究", "中国科学：生命科学", ("51", "8", "912-925")),
    ];
    let index = Index::new(&papers, [], &BTreeMap::new());
    let key = |id: &str| index.key(id).unwrap_or_default().to_string();
    let written = |gap: &str| format!("Root hairs take up most of the water{gap}[@{}], as later work confirmed{gap}[@{}; @{}].", key("1"), key("2"), key("3"));
    let text = written(" ");
    let format = Formatter::new(&[&text], &index, style, "en");
    let mut sentence = format.rich(&text, &index);
    // A raised number stands against the word it follows, as a person writing for such a journal sets it.
    if sentence.looks.iter().any(|(_, look)| look.sup) {
        let text = written("");
        sentence = Formatter::new(&[&text], &index, style, "en").rich(&text, &index);
    }
    Sample { sentence, list: format.references().iter().map(|(_, entry)| entry.clone()).collect() }
}

/// The stretch `within` of `text` as a reader reads it: each of `labels` (`Formatter::labels` of that text) that
/// lies in the stretch stands in its group's place, the rest is as written. A label that does not fit the text
/// (made from an earlier version of it) is passed over, so the words there stay as they are written.
pub fn read_as(text: &str, labels: &[(Range<usize>, String)], within: Range<usize>) -> String {
    let (mut out, mut at) = (String::new(), within.start);
    for (range, label) in labels {
        if range.start < at || range.end > within.end || !text.get(range.clone()).is_some_and(|written| written.starts_with('[') && written.ends_with(']')) { continue }
        let Some(before) = text.get(at..range.start) else { continue };
        out.push_str(before);
        out.push_str(label);
        at = range.end;
    }
    out.push_str(text.get(at..within.end).unwrap_or(""));
    out
}

/// The bracketed citations of a text, a group each, in order: old-style ones are left as they are written.
fn written_groups(text: &str, index: &Index) -> Vec<Vec<Cited>> {
    let mut groups: Vec<Vec<Cited>> = vec![];
    for one in find(text, index).into_iter().filter(|one| !one.old) {
        match groups.last_mut() {
            Some(group) if group[0].group == one.group => group.push(one),
            _ => groups.push(vec![one]),
        }
    }
    groups
}

/// Who a paper is in an author-year citation: `Zhang`, `Zhang & Li`, `Zhang et al.`; `张等` for a name in Chinese.
fn who(paper: &Paper) -> String {
    let names = families(&paper.authors);
    let chinese = names.first().is_some_and(|name| name.chars().any(crate::structure::is_cjk));
    match names.as_slice() {
        [] => paper.title.split_whitespace().take(3).collect::<Vec<_>>().join(" ").trim_end_matches(|c: char| !c.is_alphanumeric()).to_string(),
        [one] => one.clone(),
        [one, two] if chinese => format!("{one}和{two}"),
        [one, two] => format!("{one} & {two}"),
        [first, ..] if chinese => format!("{first}等"),
        [first, ..] => format!("{first} et al."),
    }
}

fn year_of(paper: &Paper) -> String {
    paper.year.map_or("n.d.".to_string(), |year| year.to_string())
}

/// `Zhang, Tian-Qi`, `Tian-Qi Zhang` or `Zhang TQ` → `Zhang, T.-Q.`; a name in one piece stays as it is.
/// The names are read together (`model::author_names`): how a list is written tells which word is the family name.
fn authors_shown(authors: &str) -> Vec<String> {
    crate::model::author_names(authors)
        .into_iter()
        .map(|(family, given)| {
            let initials: Vec<String> = given
                .split_whitespace()
                // `TQ` is two initials already; `Tian-Qi` gives one for each piece.
                .map(|part| if part.chars().count() <= 3 && part.chars().all(char::is_uppercase) { part.chars().map(|c| format!("{c}.")).collect::<Vec<_>>().join(" ") } else { part.split('-').filter_map(|piece| piece.chars().next()).map(|c| format!("{}.", c.to_uppercase())).collect::<Vec<_>>().join("-") })
                .filter(|part| !part.is_empty())
                .collect();
            if initials.is_empty() { family } else { format!("{family}, {}", initials.join(" ")) }
        })
        .collect()
}

/// The family names of `A; B; C`, the way `Paper::citation` reads them.
fn families(authors: &str) -> Vec<String> {
    crate::model::author_names(authors).into_iter().map(|(family, _)| family).collect()
}

const STOP_WORDS: [&str; 16] = ["a", "an", "the", "of", "on", "in", "and", "for", "to", "with", "by", "at", "from", "into", "is", "are"];

/// The key before a letter tells it apart: `Zhang2024`, `DeNovo2020`, `Zhang-nd`.
fn base_key(paper: &Paper) -> String {
    let word = |text: &str| -> String {
        let folded: String = text.chars().map(fold).collect();
        let mut out = String::new();
        for c in folded.chars() {
            if c.is_alphanumeric() {
                out.push(c);
            } else if c == '-' && !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
        }
        out.trim_end_matches('-').to_string()
    };
    let mut who = families(&paper.authors).first().map(|name| word(&name.replace(char::is_whitespace, ""))).unwrap_or_default();
    if who.is_empty() {
        who = paper
            .title
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty() && !STOP_WORDS.contains(&w.to_lowercase().as_str()))
            .take(2)
            .map(|w| {
                let w = word(w);
                let mut chars = w.chars();
                chars.next().map(|first| first.to_uppercase().chain(chars).collect::<String>()).unwrap_or_default()
            })
            .collect();
    }
    if who.is_empty() { who = "Anon".into() }
    if !who.starts_with(|c: char| c.is_alphanumeric()) { who.insert_str(0, "Anon") }
    match paper.year {
        Some(year) => format!("{who}{year}"),
        None => format!("{who}-nd"),
    }
}

/// The n-th key of a base, from 1: `Zhang2024`, `Zhang2024b`, … `Zhang2024z`, `Zhang2024-27`.
fn suffixed(base: &str, n: usize) -> String {
    let sep = if base.ends_with(|c: char| c.is_ascii_digit()) { "" } else { "-" };
    match n {
        1 => base.to_string(),
        2..=26 => format!("{base}{sep}{}", (b'a' + n as u8 - 1) as char),
        _ => format!("{base}-{n}"),
    }
}

/// Lower case, accents taken off, letters and digits only: how names and words are compared.
fn plain(text: &str) -> String {
    text.chars().map(fold).collect::<String>().to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// The accented Latin letters of names, written without the accent (`ü` → `u`, `ß` → `ss`).
fn fold(c: char) -> String {
    let lower = c.to_lowercase().next().unwrap_or(c);
    let plain = match lower {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => "i",
        'ł' | 'ľ' | 'ĺ' => "l",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'œ' => "oe",
        'ř' | 'ŕ' => "r",
        'ś' | 'š' | 'ş' | 'ș' => "s",
        'ß' => "ss",
        'ť' | 'ţ' | 'ț' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ž' | 'ż' => "z",
        _ => return c.to_string(),
    };
    if c.is_uppercase() {
        let mut chars = plain.chars();
        chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
    } else {
        plain.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paper(id: &str, authors: &str, year: Option<i32>, title: &str, added: &str) -> Paper {
        Paper { id: id.into(), authors: authors.into(), year, title: title.into(), added_at: added.into(), ..Default::default() }
    }

    fn library() -> Vec<Paper> {
        vec![
            paper("P1", "Zhang, Wei; Li, Qiang; Wang, Fang", Some(2024), "Single-cell atlas of roots", "2024-01-01"),
            paper("P2", "Zhang, Min; Chen, Yu; Liu, Bo", Some(2024), "WOX5 and the quiescent centre", "2024-02-01"),
            paper("P3", "", Some(2020), "De Novo Domestication: an alternative route", "2024-03-01"),
            paper("P4", "Jürgen Müller", None, "Auxin", "2024-04-01"),
            paper("P5", "Smith, Alice; Jones, Bob", Some(2019), "Two authors", "2024-05-01"),
        ]
    }

    #[test]
    fn converts_only_complete_chat_citations_and_preserves_locators() {
        let papers = library();
        let index = Index::new(&papers, [], &BTreeMap::new());
        let sources = vec![
            crate::ai::Source { label: "P1".into(), paper_id: "P1".into(), title: "One".into(), ..Default::default() },
            crate::ai::Source { label: "P2".into(), paper_id: "P2".into(), title: "Two".into(), ..Default::default() },
        ];
        let original = "测试 [P1 p.5] and [P1, p.6; P2 p.7] [P1, P9] [P1 \"quote\"] [P2 p.3 “a sentence. With a stop”] [P1 p.5 extra] [P1 p.0].";
        let converted = convert_ai_citations(original, &sources, &index);
        // `p.0` is no page, as after a key.
        assert_eq!(converted.text, "测试 [@Zhang2024, p. 5] and [@Zhang2024, p. 6; @Zhang2024b, p. 7] [P1, P9] [@Zhang2024] [@Zhang2024b, p. 3] [P1 p.5 extra] [@Zhang2024].");
        assert_eq!(converted.keys, vec![("P1".into(), "Zhang2024".into()), ("P2".into(), "Zhang2024b".into())]);
        // What was quoted is handed back with where its citation now stands.
        assert_eq!(converted.quoted.iter().map(|q| (&converted.text[q.at..q.at + 2], q.paper_id.as_str(), q.page, q.quote.as_str())).collect::<Vec<_>>(), [("[@", "P1", None, "quote"), ("[@", "P2", Some(3), "a sentence. With a stop")]);
        assert_eq!(&converted.text[converted.quoted[1].at..], "[@Zhang2024b, p. 3] [P1 p.5 extra] [@Zhang2024].");
        let references = find(&converted.text, &index);
        assert_eq!(references.iter().filter(|citation| citation.found.paper().is_some()).count(), 6);
        // What stayed as written is handed back: with the label that names no paper, or with none when it is the writing that was not read.
        assert_eq!(converted.left, [Left { written: "[P1, P9]".into(), unknown: vec!["P9".into()] }, Left { written: "[P1 p.5 extra]".into(), unknown: vec![] }]);

        // A label written with the manuscript's `@` before it is the same label, and its quotation is not left in the text.
        let hybrid = convert_ai_citations("One [@P1 p.5 \"a sentence\"] and two [@P1; @P2 p.7].", &sources, &index);
        assert_eq!(hybrid.text, "One [@Zhang2024, p. 5] and two [@Zhang2024; @Zhang2024b, p. 7].");
        assert_eq!(hybrid.quoted.iter().map(|q| (q.paper_id.as_str(), q.page, q.quote.as_str())).collect::<Vec<_>>(), [("P1", Some(5), "a sentence")]);

        let mut ambiguous = sources.clone();
        ambiguous.push(crate::ai::Source { label: "P1".into(), paper_id: "P2".into(), title: "duplicate".into(), ..Default::default() });
        let result = convert_ai_citations("[P1, P2] [P2]", &ambiguous, &index);
        assert_eq!(result.text, "[P1, P2] [@Zhang2024b]");
        assert_eq!(result.keys, vec![("P2".into(), "Zhang2024b".into())]);

        let removed = vec![crate::ai::Source { label: "P1".into(), paper_id: "gone".into(), title: "Gone".into(), ..Default::default() }];
        let result = convert_ai_citations("[P1 p.5]", &removed, &index);
        assert_eq!(result.text, "[P1 p.5]");
        assert!(result.keys.is_empty());
        assert_eq!(result.left, [Left { written: "[P1 p.5]".into(), unknown: vec!["P1".into()] }]);
    }

    #[test]
    fn a_bracket_left_open_takes_nothing_with_it() {
        let papers = library();
        let index = Index::new(&papers, [], &BTreeMap::new());
        let sources = vec![
            crate::ai::Source { label: "P1".into(), paper_id: "P1".into(), title: "One".into(), ..Default::default() },
            crate::ai::Source { label: "P2".into(), paper_id: "P2".into(), title: "Two".into(), ..Default::default() },
        ];
        let convert = |text: &str| convert_ai_citations(text, &sources, &index);
        // What a model sent (Kimi K3, 助手写): the first citation's bracket never closes. Read as one citation it took the
        // words between the two, and the second citation, out of the manuscript as its sentence.
        let (first, second) = ("HIF-1α and HIF-2α have distinct and sometimes opposing roles in tumour growth; HIF-2α drives clear cell renal carcinoma.", "The HIF-2α inhibitor belzutifan produced durable responses in renal cell carcinoma associated with VHL disease.");
        let (before, between, after) = ("具体看肾透明细胞癌，HIF-2α 的地位尤其突出。HIF-1α 与 HIF-2α 虽然是同源的两个亚基，但在肿瘤生长中的作用并不相同，有时甚至相互拮抗，其中驱动肾透明细胞癌的主要是 HIF-2α ", "。这一机制认识也直接转化成了治疗手段：HIF-2α 抑制剂 belzutifan 在 VHL 病相关的肾细胞癌中取得了持久的疗效 ", "，证明靶向 HIF-2α 是治疗这类肿瘤的一条可行途径。");
        let sent = format!("{before}[P2 \"{first}\"{between}[P1 \"{second}\"]{after}");
        let converted = convert(&sent);
        // The open bracket is the text it was written as, with its sentence; the citation after it is a citation.
        assert_eq!(converted.text, format!("{before}[P2 \"{first}\"{between}[@Zhang2024]{after}"));
        assert_eq!(converted.quoted.iter().map(|q| (&converted.text[q.at..q.at + 11], q.paper_id.as_str(), q.page, q.quote.as_str())).collect::<Vec<_>>(), [("[@Zhang2024", "P1", None, second)]);
        assert_eq!(converted.keys, vec![("P1".into(), "Zhang2024".into())]);
        // It is handed back as far as it reads as a citation, and its label is not said to be unknown.
        assert_eq!(converted.left, [Left { written: format!("[P2 \"{first}\""), unknown: vec![] }]);

        // Closed, each converts as it did.
        let closed = convert(&format!("{before}[P2 \"{first}\"]{between}[P1 \"{second}\"]{after} 另见 [P1]、[P2 p.5] 和 [P2 p.5 “With a page.”]。"));
        assert_eq!(closed.text, format!("{before}[@Zhang2024b]{between}[@Zhang2024]{after} 另见 [@Zhang2024]、[@Zhang2024b, p. 5] 和 [@Zhang2024b, p. 5]。"));
        assert_eq!(closed.quoted.iter().map(|q| (q.paper_id.as_str(), q.page, q.quote.as_str())).collect::<Vec<_>>(), [("P2", None, first), ("P1", None, second), ("P2", Some(5), "With a page.")]);
        assert!(closed.left.is_empty());

        // Other ways of not ending a citation: nothing after the label, words after the sentence, a sentence that does not
        // end, a label of no paper. All stay; only a sentence that runs to its closing bracket leaves the text.
        let open = "一 [P1 p.5 “Open.” 二 [P9 \"Nobody's.\" 三 [P1 \"Words after.\" and so on] 四 [P2 \"It never ends 五 [@P2, p.3 「With an at.」\n六 [@Zhang2024 p.5 \"After a key.\"。七 [@Zhang2024, p. 5 八 [P1";
        let left = convert(open);
        assert_eq!((left.text.as_str(), left.quoted.len(), left.keys.len()), (open, 0, 0));
        assert_eq!(left.left.iter().map(|one| (one.written.as_str(), one.unknown.join(" "))).collect::<Vec<_>>(), [
            ("[P1 p.5 “Open.”", String::new()),
            ("[P9 \"Nobody's.\"", "P9".to_string()),
            ("[P1 \"Words after.\" and so on]", String::new()),
            ("[P2", String::new()),
            ("[@P2, p.3 「With an at.」", String::new()),
            // After a key too, when it has a sentence: without one the bracket is the manuscript's own.
            ("[@Zhang2024 p.5 \"After a key.\"", String::new()),
            ("[P1", String::new()),
        ]);
        // A bracket that only looks like a label's is nobody's citation, and one the sentence holds is the sentence's.
        let other = convert("[P2: a note [P2a [P1 \"The [P2 allele] is lost.\"]");
        assert_eq!((other.text.as_str(), other.quoted[0].quote.as_str(), other.left.len()), ("[P2: a note [P2a [@Zhang2024]", "The [P2 allele] is lost.", 0));

        // A sentence with quotation marks of its own, of the kind it is quoted in, is still one sentence.
        let own = convert("[P1 p.5 \"He called it a \"sibling rivalry\" of the two.\"] [P2 “所谓“缺氧”反应。”]");
        assert_eq!(own.text, "[@Zhang2024, p. 5] [@Zhang2024b]");
        assert_eq!(own.quoted.iter().map(|q| q.quote.as_str()).collect::<Vec<_>>(), ["He called it a \"sibling rivalry\" of the two.", "所谓“缺氧”反应。"]);
    }

    #[test]
    fn page_nought_and_a_sentence_after_several_papers() {
        let papers = library();
        let index = Index::new(&papers, [], &BTreeMap::new());
        let sources = vec![
            crate::ai::Source { label: "P1".into(), paper_id: "P1".into(), title: "One".into(), ..Default::default() },
            crate::ai::Source { label: "P2".into(), paper_id: "P2".into(), title: "Two".into(), ..Default::default() },
        ];
        let quoted = |converted: &AiCitationConversion| converted.quoted.iter().map(|q| (converted.text[q.at..].chars().next(), q.paper_id.clone(), q.page, q.quote.clone())).collect::<Vec<_>>();
        // `p.0` is what a model writes for "no page": the citation has none, and its sentence is kept as any other.
        let nought = convert_ai_citations("一 [P1 p.0 \"From the abstract.\"]，二 [P2 p.0]，三 [P1, p.0; P2 p.3]。", &sources, &index);
        assert_eq!(nought.text, "一 [@Zhang2024]，二 [@Zhang2024b]，三 [@Zhang2024; @Zhang2024b, p. 3]。");
        assert_eq!(quoted(&nought), [(Some('['), "P1".to_string(), None, "From the abstract.".to_string())]);
        assert!(nought.left.is_empty());
        // Several papers and one sentence: it is the last one's, the paper it was written after, with that paper's page.
        let several = convert_ai_citations("一 [P1; P2 p.7 \"The second paper's.\"]，二 [P2 p.3、P1 “The first paper's.”]。", &sources, &index);
        assert_eq!(several.text, "一 [@Zhang2024; @Zhang2024b, p. 7]，二 [@Zhang2024b, p. 3; @Zhang2024]。");
        assert_eq!(quoted(&several), [(Some('['), "P2".to_string(), Some(7), "The second paper's.".to_string()), (Some('['), "P1".to_string(), None, "The first paper's.".to_string())]);
        // Each is found by its brackets and its paper where the passages are kept.
        let brought = crate::passage::quoted(&several.text, &find(&several.text, &index), &several.quoted);
        assert_eq!(brought.iter().map(|(paper, _, candidate)| (paper.as_str(), candidate.text.as_str(), candidate.page)).collect::<Vec<_>>(), [("P2", "The second paper's.", 7), ("P1", "The first paper's.", 0)]);
        // One label of the group names no paper: the group stays as written, sentence and all.
        let stray = convert_ai_citations("[P1; P9 \"Whose?\"]", &sources, &index);
        assert_eq!((stray.text.as_str(), stray.quoted.len()), ("[P1; P9 \"Whose?\"]", 0));
        assert_eq!(stray.left, [Left { written: "[P1; P9 \"Whose?\"]".into(), unknown: vec!["P9".into()] }]);
    }

    #[test]
    fn a_key_written_with_the_sentence_a_label_has() {
        let papers = library();
        let index = Index::new(&papers, [], &BTreeMap::new());
        let convert = |text: &str| convert_ai_citations(text, &[], &index);
        // As a model wrote them, with and without a page; `p.0` is no page, and a bracket inside the sentence does not end it.
        let converted = convert("一 [@Zhang2024 \"First [sic] sentence.\"]，二 [@Smith2019 p.0 \"Second.\"]，三 [@Zhang2024b p.5 “Third.”]，四 [@DeNovo2020, p. 7 「Fourth.」]，五 [@zhang2024 p12 \"Fifth.\" ]。");
        assert_eq!(converted.text, "一 [@Zhang2024]，二 [@Smith2019]，三 [@Zhang2024b, p. 5]，四 [@DeNovo2020, p. 7]，五 [@zhang2024, p. 12]。");
        assert_eq!(
            converted.quoted.iter().map(|q| (&converted.text[q.at..q.at + 2], q.paper_id.as_str(), q.page, q.quote.as_str())).collect::<Vec<_>>(),
            [("[@", "P1", None, "First [sic] sentence."), ("[@", "P5", None, "Second."), ("[@", "P2", Some(5), "Third."), ("[@", "P3", Some(7), "Fourth."), ("[@", "P1", Some(12), "Fifth.")]
        );
        assert_eq!(converted.keys, vec![("P1".into(), "Zhang2024".into()), ("P5".into(), "Smith2019".into()), ("P2".into(), "Zhang2024b".into()), ("P3".into(), "DeNovo2020".into())]);
        assert!(converted.dropped.is_empty());
        // Every one of them is a citation the manuscript reads, and none says a sentence any more.
        let found = find(&converted.text, &index);
        assert!(found.len() == 5 && found.iter().all(|one| one.found.paper().is_some() && said(&converted.text, one).quote.is_none()));

        // A key no paper has: the sentence does not stay in the text, and is handed back with the key it was written after.
        let unknown = convert("x [@Nobody1999 p.3 \"Nobody said this.\"] y");
        assert_eq!((unknown.text.as_str(), unknown.quoted.len(), unknown.keys.len()), ("x [@Nobody1999, p. 3] y", 0, 0));
        assert_eq!(unknown.dropped, [("@Nobody1999".to_string(), "Nobody said this.".to_string())]);
        assert_eq!(find(&unknown.text, &index)[0].found, Found::Unknown);

        // Several papers and one sentence: it is the last one's, as with labels, or handed back when that key is no paper's.
        let several = convert("[@Zhang2024; @Smith2019 p.2 \"The last one's.\"] [@Zhang2024; @Nobody1999 \"Nobody's.\"]");
        assert_eq!(several.text, "[@Zhang2024; @Smith2019, p. 2] [@Zhang2024; @Nobody1999]");
        assert_eq!(several.quoted.iter().map(|q| (&several.text[q.at..q.at + 2], q.paper_id.as_str(), q.page, q.quote.as_str())).collect::<Vec<_>>(), [("[@", "P5", Some(2), "The last one's.")]);
        assert_eq!((several.dropped.as_slice(), several.keys.as_slice()), ([("@Nobody1999".to_string(), "Nobody's.".to_string())].as_slice(), [("P5".to_string(), "Smith2019".to_string())].as_slice()));
        // Words after the key that are not a page stay as they were written; the sentence still leaves.
        let worded = convert("[@Zhang2024, Fig. 2 \"In the figure.\"] [@Smith2019 pp. 3–4 \"Over two pages.\"]");
        assert_eq!(worded.text, "[@Zhang2024, Fig. 2] [@Smith2019 pp. 3–4]");
        assert_eq!(worded.quoted.iter().map(|q| (q.paper_id.as_str(), q.page, q.quote.as_str())).collect::<Vec<_>>(), [("P1", None, "In the figure."), ("P5", Some(3), "Over two pages.")]);
        // A paper's id where the key goes, as a tool takes it.
        let other = [paper("paper-1700000000000-0a1b2c3d", "Jürgen Müller", None, "Auxin", "2024-04-01")];
        let by_id = convert_ai_citations("[@paper-1700000000000-0a1b2c3d \"By its id.\"]", &[], &Index::new(&other, [], &BTreeMap::new()));
        assert_eq!((by_id.text.as_str(), by_id.quoted[0].paper_id.as_str(), by_id.keys[0].1.as_str()), ("[@paper-1700000000000-0a1b2c3d]", "paper-1700000000000-0a1b2c3d", "Muller-nd"));

        // What a manuscript's own citations look like is not touched: no sentence, a sentence that is not the
        // end of the brackets, or words before the key.
        let own = "[@Zhang2024] [@Zhang2024, p. 5] [see @Zhang2024, p. 5] [@Zhang2024 \"a title\", p. 5] [see @Zhang2024 \"quoted\"] [-@Zhang2024 \"quoted\"] [\"quoted\" @Zhang2024] [@ \"no key\"] [note \"quoted\"]";
        let kept = convert(own);
        assert_eq!((kept.text.as_str(), kept.quoted.len(), kept.dropped.len(), kept.keys.len()), (own, 0, 0, 0));
    }

    #[test]
    fn keys_are_readable_and_unique() {
        let papers = library();
        let index = Index::new(&papers, [], &BTreeMap::new());
        let keys: Vec<&str> = ["P1", "P2", "P3", "P4", "P5"].iter().map(|id| index.key(id).unwrap()).collect();
        assert_eq!(keys, ["Zhang2024", "Zhang2024b", "DeNovo2020", "Muller-nd", "Smith2019"]);
        // Authors as Europe PMC writes them, the family name first: the key is the family name's, not the initials'.
        let pmc = [paper("E1", "Bono M; Marín-Sanz M; Uberegui E", Some(2026), "TaPYL", "2026-01-01"), paper("E2", "de Melo AV; Costa LC", Some(2026), "Iron", "2026-01-02")];
        let index = Index::new(&pmc, [], &BTreeMap::new());
        assert_eq!((index.key("E1"), index.key("E2")), (Some("Bono2026"), Some("deMelo2026")));
        // A key cited before stays the paper's, whatever it would be called now.
        let kept = BTreeMap::from([("M2026".to_string(), "E1".to_string())]);
        assert_eq!(Index::new(&pmc, [], &kept).key("E1"), Some("M2026"));
    }

    #[test]
    fn keys_hold_when_papers_come_and_go() {
        let mut papers = library();
        // A paper added later does not take the key of an earlier one.
        papers.push(paper("P0", "Zhang, Lei", Some(2024), "Later", "2025-01-01"));
        let index = Index::new(&papers, [], &BTreeMap::new());
        assert_eq!((index.key("P1"), index.key("P2"), index.key("P0")), (Some("Zhang2024"), Some("Zhang2024b"), Some("Zhang2024c")));
        // Cited keys are kept: deleting P1 leaves P2 its key, and a citation of P1 says it is gone.
        let kept: BTreeMap<String, String> = [("Zhang2024".to_string(), "P1".to_string()), ("Zhang2024b".to_string(), "P2".to_string())].into();
        let rest: Vec<Paper> = papers.iter().filter(|p| p.id != "P1").cloned().collect();
        let index = Index::new(&rest, [], &kept);
        assert_eq!(index.key("P2"), Some("Zhang2024b"));
        assert_eq!(index.key("P0"), Some("Zhang2024c"));
        let cited = find("as shown [@Zhang2024; @zhang2024B]", &index);
        assert_eq!(cited.iter().map(|c| c.found.clone()).collect::<Vec<_>>(), [Found::Removed, Found::Paper("P2".into())]);
        // A corrected record keeps the key it was cited by.
        let mut fixed = papers.clone();
        fixed[0].year = Some(2023);
        let index = Index::new(&fixed, [], &kept);
        assert_eq!(index.key("P1"), Some("Zhang2024"));
        assert_eq!(index.key("P0"), Some("Zhang2024c"));
    }

    #[test]
    fn finds_bracketed_keys() {
        let papers = library();
        let (open, trash) = papers.split_at(4);
        let index = Index::new(open, trash, &BTreeMap::new());
        let text = "Roots [@Zhang2024, p. 5; see @DeNovo2020]. Mail me@x.org [@Nobody1999] and [@Smith2019]. [P3 p.5] [@].";
        let cited = find(text, &index);
        let got: Vec<(&str, Found)> = cited.iter().map(|c| (c.written.as_str(), c.found.clone())).collect();
        assert_eq!(got, [("@Zhang2024", Found::Paper("P1".into())), ("@DeNovo2020", Found::Paper("P3".into())), ("@Nobody1999", Found::Unknown), ("@Smith2019", Found::Trashed("P5".into()))]);
        assert_eq!(&text[cited[0].group.clone()], "[@Zhang2024, p. 5; see @DeNovo2020]");
        assert_eq!(&text[cited[0].range.clone()], "@Zhang2024");
        // A sentence's full stop after a key is not part of it.
        assert_eq!(key_at("Zhang2024."), "Zhang2024");
        assert_eq!(key_at("a:b-c_d]"), "a:b-c_d");
    }

    #[test]
    fn reads_old_style_citations() {
        let papers = library();
        let index = Index::new(&papers, [], &BTreeMap::new());
        let text = "A (Zhang et al., 2024) B (Smith and Jones, 2019; De Novo Domestication:, 2020) C (Müller, 2021) D (n = 12, 2021) (March 2021) （Smith, 2019）";
        let cited = find(text, &index);
        let got: Vec<(&str, Found)> = cited.iter().map(|c| (c.written.as_str(), c.found.clone())).collect();
        assert_eq!(
            got,
            [
                ("Zhang et al., 2024", Found::Ambiguous(vec!["P1".into(), "P2".into()])),
                ("Smith and Jones, 2019", Found::Paper("P5".into())),
                ("De Novo Domestication:, 2020", Found::Paper("P3".into())),
                ("Müller, 2021", Found::Unknown),
                // Loosely: Smith is the first author of one paper of 2019, though not its only one.
                ("Smith, 2019", Found::Paper("P5".into())),
            ]
        );
        let upgrades = upgrades(&cited, &index);
        assert_eq!(upgrades.iter().map(|(_, to)| to.as_str()).collect::<Vec<_>>(), ["[@Smith2019]", "[@Smith2019; @DeNovo2020]"]);
        let mut fixed = text.to_string();
        for (range, to) in &upgrades { fixed.replace_range(range.clone(), to) }
        assert!(fixed.starts_with("A (Zhang et al., 2024) B [@Smith2019; @DeNovo2020] C (Müller, 2021)") && fixed.ends_with(" [@Smith2019]"), "{fixed}");
        // Read again, the keys are found and the old forms no longer.
        assert_eq!(find(&fixed, &index).iter().filter(|c| c.old).count(), 2);
    }

    #[test]
    fn reference_list() {
        let mut papers = library();
        papers[0].journal = "Nature Plants".into();
        papers[0].doi = "10.1000/x".into();
        papers[2].publication_status = "preprint".into();
        papers[4].pmid = "123".into();
        let index = Index::new(&papers, [], &BTreeMap::new());
        let cited = find("[@Zhang2024] [@Smith2019; @DeNovo2020] again [@Zhang2024] [@Muller-nd] [@Gone2000]", &index);
        let refs = references(&cited, &index);
        assert_eq!(refs.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), ["DeNovo2020", "Muller-nd", "Smith2019", "Zhang2024"]);
        assert_eq!(refs[0].text, "De Novo Domestication: an alternative route. (2020). Preprint.");
        assert_eq!(refs[1].text, "Müller, J. (n.d.). Auxin.");
        assert_eq!(refs[2].text, "Smith, A., & Jones, B. (2019). Two authors. PMID: 123");
        assert_eq!(refs[3].text, "Zhang, W., Li, Q., & Wang, F. (2024). Single-cell atlas of roots. Nature Plants. https://doi.org/10.1000/x");
        // Volume, issue and pages follow the journal, each when the record has it.
        let mut placed = papers[0].clone();
        (placed.volume, placed.issue, placed.pages) = ("10".into(), "3".into(), "357-362".into());
        assert_eq!(reference(&placed), "Zhang, W., Li, Q., & Wang, F. (2024). Single-cell atlas of roots. Nature Plants, 10(3), 357–362. https://doi.org/10.1000/x");
        placed.issue.clear();
        assert!(reference(&placed).contains(" Nature Plants, 10, 357–362. "));
        (placed.volume, placed.pages) = (String::new(), "e1234".into());
        assert!(reference(&placed).contains(" Nature Plants, e1234. "));
        let bib = bibtex(&refs, &index);
        assert!(bib.starts_with("@article{DeNovo2020,\n") && bib.contains("@article{Zhang2024,\n") && bib.matches("@article").count() == 4, "{bib}");
        assert_eq!(authors_shown("Tian-Qi Zhang; 张伟"), ["Zhang, T.-Q.", "张伟"]);
        // Europe PMC writes the family name first and initials after it.
        assert_eq!(authors_shown("Bono M; Marín-Sanz M; de Melo AV"), ["Bono, M.", "Marín-Sanz, M.", "de Melo, A. V."]);
        assert_eq!(authors_shown("Zhang, TQ; Harris, Charles R."), ["Zhang, T. Q.", "Harris, C. R."]);
    }

    #[test]
    fn picking_a_paper_by_typing() {
        let papers = library();
        let index = Index::new(&papers, [], &BTreeMap::new());
        let ids = |query: &str| index.matching(query, 5).iter().map(|p| p.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids("zhang wox5"), ["P2"]);
        assert_eq!(ids("muller"), ["P4"]);
        assert_eq!(ids("domestication"), ["P3"]);
        assert_eq!(ids("2019"), ["P5"]);
        assert!(ids("  ").is_empty());
        // The first author first, then the papers that only mention the word.
        assert_eq!(ids("smith"), ["P5"]);
        assert_eq!(ids("jones two"), ["P5"]);
    }

    #[test]
    fn what_a_citation_says_besides_its_key() {
        let papers = library();
        let index = Index::new(&papers, [], &BTreeMap::new());
        let text = "根系[@Zhang2024 p.2 \"a core set; of genes\"]，见 [@Smith2019, p. 5; @Zhang2024b “Another one.”] 和 [@DeNovo2020, pp. 3–4] [@Smith2019 p12] [@Smith2019]";
        let said: Vec<Said> = find(text, &index).iter().map(|one| said(text, one)).collect();
        let s = |page: Option<usize>, quote: Option<&str>| Said { page, quote: quote.map(str::to_string) };
        assert_eq!(said, [s(Some(2), Some("a core set; of genes")), s(Some(5), None), s(None, Some("Another one.")), s(Some(3), None), s(Some(12), None), s(None, None)]);
    }

    #[test]
    fn citations_written_for_a_reader() {
        let papers = library();
        let index = Index::new(&papers, [], &BTreeMap::new());
        let one = "Roots [@Zhang2024 p.2 \"quoted\"] and shoots [@Smith2019; @Zhang2024b] [@Gone2000].";
        let two = "Again [@DeNovo2020; @Zhang2024; @Smith2019; @Zhang2024b] (Zhang et al., 2024).";
        // Author and year: two papers that read alike get letters, the list by author.
        let format = Formatter::new(&[one, two], &index, &Style::AuthorYear, "en");
        assert_eq!(format.apply(one, &index), "Roots (Zhang et al., 2024a) and shoots (Smith & Jones, 2019; Zhang et al., 2024b) [@Gone2000].");
        assert_eq!(format.apply(two, &index), "Again (De Novo Domestication, 2020; Zhang et al., 2024a; Smith & Jones, 2019; Zhang et al., 2024b) (Zhang et al., 2024).");
        assert_eq!(format.references().len(), 4);
        assert!(format.references()[2].1.text.starts_with("Zhang, W., Li, Q., & Wang, F. (2024a). Single-cell"), "{:?}", format.references());
        assert!(format.references()[3].1.text.contains("(2024b). WOX5"));
        assert_eq!(format.heading(), "References");
        // Numbered in the order first cited, three in a row as a range.
        let format = Formatter::new(&[one, two], &index, &Style::Numeric, "zh");
        assert_eq!(format.apply(one, &index), "Roots [1] and shoots [2, 3] [@Gone2000].");
        assert_eq!(format.apply(two, &index), "Again [1–4] (Zhang et al., 2024).");
        assert!(format.references()[0].1.text.starts_with("[1] Zhang, W.") && format.references()[3].1.text.starts_with("[4] De Novo"));
        assert_eq!(format.heading(), "参考文献");
        let chinese = Formatter::new(&["差异[@Smith2019]。"], &index, &Style::AuthorYear, "zh");
        assert_eq!(chinese.apply("差异[@Smith2019]。", &index), "差异（Smith & Jones, 2019）。");
        // Which words cite which paper: a range by its two ends, an author-year group name by name.
        let linked = |rich: Rich| rich.links.iter().map(|(range, to)| (rich.text[range.clone()].to_string(), index.key(to).unwrap_or("").to_string())).collect::<Vec<_>>();
        let pair = |words: &str, key: &str| (words.to_string(), key.to_string());
        assert_eq!(linked(format.rich(two, &index)), [pair("1", "Zhang2024"), pair("4", "DeNovo2020")]);
        assert_eq!(linked(format.rich(one, &index)), [pair("1", "Zhang2024"), pair("2", "Smith2019"), pair("3", "Zhang2024b")]);
        assert_eq!(linked(chinese.rich("差异[@Smith2019]。", &index)), [pair("Smith & Jones, 2019", "Smith2019")]);
        // A journal's style: the same groups, as its CSL file says.
        let nature = Style::Csl(std::sync::Arc::new(crate::csl::load("csl:nature", std::path::Path::new("/nowhere")).unwrap()));
        let format = Formatter::new(&[one, two], &index, &nature, "en");
        let rich = format.rich(one, &index);
        assert_eq!(rich.text, "Roots 1 and shoots 2,3 [@Gone2000].");
        assert_eq!(linked(rich.clone()), [pair("1", "Zhang2024"), pair("2", "Smith2019"), pair("3", "Zhang2024b")]);
        assert!(rich.looks.iter().all(|(_, look)| look.sup) && !rich.looks.is_empty());
        assert_eq!(format.apply(two, &index), "Again 1–4 (Zhang et al., 2024).");
        assert_eq!(format.references().len(), 4);
        assert!(format.references()[0].1.text.starts_with("1. Zhang, W., Li, Q. & Wang, F."), "{:?}", format.references()[0]);
    }

    #[test]
    fn a_style_is_shown_on_works_of_its_own() {
        let shown = sample(&Style::AuthorYear);
        assert_eq!(shown.sentence.text, "Root hairs take up most of the water (Zhang et al., 2024), as later work confirmed (Smith & Jones, 2019; 王芳等, 2021).");
        assert_eq!(shown.list.len(), 3);
        let shown = sample(&Style::Numeric);
        assert_eq!(shown.sentence.text, "Root hairs take up most of the water [1], as later work confirmed [2, 3].");
        assert!(shown.list[0].text.starts_with("[1] Zhang") && shown.list[0].text.contains("Nature Plants"), "{}", shown.list[0].text);
        // A journal's style: raised numbers, and the journal in italics with where the paper is in it.
        let nature = sample(&Style::Csl(std::sync::Arc::new(crate::csl::load("csl:nature", std::path::Path::new("/nowhere")).unwrap())));
        assert_eq!(nature.sentence.text, "Root hairs take up most of the water1, as later work confirmed2,3.");
        assert!(nature.sentence.looks.iter().any(|(_, look)| look.sup));
        assert!(nature.list[0].text.starts_with("1. Zhang, W. et al. A single-cell atlas of root development.") && nature.list[0].text.contains("10") && nature.list[0].text.contains("357–362"), "{}", nature.list[0].text);
        assert!(nature.list[0].looks.iter().any(|(range, look)| look.italic && &nature.list[0].text[range.clone()] == "Nature Plants"));
        // Where nothing can be drawn raised, the numbers are raised characters, and what follows them keeps its looks and links.
        let drawn = nature.sentence.raised_as_characters();
        assert_eq!(drawn.text, "Root hairs take up most of the water¹, as later work confirmed²,³.");
        assert!(drawn.looks.is_empty());
        assert_eq!(drawn.links.iter().map(|(range, to)| (&drawn.text[range.clone()], to.as_str())).collect::<Vec<_>>(), [("¹", "1"), ("²", "2"), ("³", "3")]);
        let entry = nature.list[0].raised_as_characters();
        assert_eq!(entry, nature.list[0]);
        let mixed = Rich { text: "a 14–16 b".into(), looks: vec![(2..9, crate::docx::Look { sup: true, ..Default::default() }), (10..11, crate::docx::Look { italic: true, ..Default::default() })], links: vec![] }.raised_as_characters();
        assert_eq!((mixed.text.as_str(), &mixed.text[mixed.looks[0].0.clone()], mixed.looks.len()), ("a ¹⁴⁻¹⁶ b", "b", 1));
        let bracketed = Rich { text: "a[1,2]".into(), looks: vec![(1..6, crate::docx::Look { sup: true, ..Default::default() })], links: vec![] }.raised_as_characters();
        assert_eq!((bracketed.text.as_str(), bracketed.looks.len()), ("a[1,2]", 0));
        // Every style offered first can be shown.
        for one in crate::csl::common() {
            let shown = sample(&crate::csl::style(&one.key, std::path::Path::new("/nowhere")).unwrap());
            assert!(shown.list.len() == 3 && !shown.sentence.text.contains("[@"), "{}: {}", one.key, shown.sentence.text);
        }
    }

    #[test]
    fn what_a_manuscript_shows_in_a_citations_place() {
        let papers = library();
        let index = Index::new(&papers, [], &BTreeMap::new());
        let one = "Roots [@Zhang2024 p.2 \"quoted\"] and shoots [@Smith2019; @Zhang2024b] [@Gone2000].";
        let two = "Again [@DeNovo2020].";
        let placed = |format: &Formatter, text: &str| format.labels(text, &index).into_iter().map(|(range, label)| (text[range].to_string(), label)).collect::<Vec<_>>();
        let pair = |written: &str, label: &str| (written.to_string(), label.to_string());
        // The group whole, quotation and all; a key nothing has stays as it is written.
        let format = Formatter::new(&[one, two], &index, &Style::AuthorYear, "en");
        assert_eq!(placed(&format, one), [pair("[@Zhang2024 p.2 \"quoted\"]", "(Zhang et al., 2024a)"), pair("[@Smith2019; @Zhang2024b]", "(Smith & Jones, 2019; Zhang et al., 2024b)")]);
        // Numbered through the whole manuscript, not the part on show.
        let format = Formatter::new(&[one, two], &index, &Style::Numeric, "en");
        assert_eq!(placed(&format, two), [pair("[@DeNovo2020]", "[4]")]);
        // A raised number is put in brackets on the line.
        let nature = Style::Csl(std::sync::Arc::new(crate::csl::load("csl:nature", std::path::Path::new("/nowhere")).unwrap()));
        let format = Formatter::new(&[one, two], &index, &nature, "en");
        assert_eq!(placed(&format, one), [pair("[@Zhang2024 p.2 \"quoted\"]", "[1]"), pair("[@Smith2019; @Zhang2024b]", "[2,3]")]);
    }

    #[test]
    fn a_text_read_through_has_its_citations_as_a_reader_reads_them() {
        let papers = library();
        let index = Index::new(&papers, [], &BTreeMap::new());
        let text = "根 [@Zhang2024 p.2 \"quoted\"] 与茎 [@Smith2019; @Zhang2024b] [@Gone2000]。\n| 文献 | 说法 |\n|---|---|\n| [@Smith2019] | 茎 |";
        let format = Formatter::new(&[text], &index, &Style::AuthorYear, "en");
        let labels = format.labels(text, &index);
        // The whole text; a key nothing has stays as it is written.
        assert_eq!(read_as(text, &labels, 0..text.len()), "根 (Zhang et al., 2024a) 与茎 (Smith & Jones, 2019; Zhang et al., 2024b) [@Gone2000]。\n| 文献 | 说法 |\n|---|---|\n| (Smith & Jones, 2019) | 茎 |");
        // A stretch of it (what stands between two pictures): only the groups inside it.
        let from = text.find('|').unwrap();
        assert_eq!(read_as(text, &labels, from..text.len()), "| 文献 | 说法 |\n|---|---|\n| (Smith & Jones, 2019) | 茎 |");
        assert_eq!(read_as(text, &labels, 0..from - 1), "根 (Zhang et al., 2024a) 与茎 (Smith & Jones, 2019; Zhang et al., 2024b) [@Gone2000]。");
        // Labels of another text are passed over and nothing breaks: here they point into the middle of words.
        assert_eq!(read_as("根与茎都长得很快，叶子随后。", &labels, 0..42), "根与茎都长得很快，叶子随后。");
        assert_eq!(read_as("short", &labels, 0..5), "short");
    }
}
