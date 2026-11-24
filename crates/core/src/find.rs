//! Words looked for across a whole library: in the page text of every file, in the reading marks (the
//! text marked and what was written on it), in a paper's notes, its title and its abstract. Exact, as
//! the find bar of a PDF is (`fulltext::find_text`): the whole phrase, without case, across line breaks
//! and words hyphenated at a line's end, inside a run of Chinese. Read only. The window's 全库查找 and
//! the tool `search_fulltext` both end here.

use crate::annotation::{Annotation, Kind};
use crate::fulltext::{bare, find_text};
use crate::model::{Paper, PaperFile};
use crate::store::Store;
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

/// Where in a paper words were found, in the order a paper's places are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Place {
    Title,
    Abstract,
    Notes,
    /// A reading mark: a highlight or an underline, a box, a sticky note, a text box, a piece marked to cut.
    Mark,
    /// The page text of a file: the full text or an attachment.
    Text,
}

impl Place {
    /// As the tool names it.
    pub fn key(self) -> &'static str {
        match self { Self::Title => "title", Self::Abstract => "abstract", Self::Notes => "notes", Self::Mark => "mark", Self::Text => "fulltext" }
    }
}

/// The mark words were found on.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkHit {
    pub id: String,
    pub kind: Kind,
    /// The lines of the text it marks, or its frame (`Annotation::boxes`).
    pub boxes: Vec<[f32; 4]>,
    /// In what was written on it (its note, a sticky note's or a text box's words), not in the text it marks.
    pub written: bool,
}

/// One place the words stand.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub paper_id: String,
    pub place: Place,
    /// The file (`resources.id`) of a page's text or of a mark; empty for the record and the notes.
    pub file: String,
    /// The 1-based page of that file; 0 for the record and the notes.
    pub page: usize,
    /// The sentence the words stand in, on one line: the text's own words, only its spacing made one space,
    /// so it can be looked for on the page as a quotation is (`fulltext::find_quote`).
    pub passage: String,
    /// The words found, a byte range of `passage`.
    pub found: Range<usize>,
    /// The sentence went on before / after the passage: a long one is cut round the words.
    pub cut: (bool, bool),
    /// Where the words stand in the text they were found in (the page, the abstract, the notes, the mark's text).
    pub at: Range<usize>,
    pub mark: Option<MarkHit>,
}

impl Hit {
    /// The passage to show on one line with the words in view: at most `before` characters ahead of them, and
    /// `…` where it was cut. The range is the words' in what is returned.
    pub fn shown(&self, before: usize) -> (String, Range<usize>) {
        let head = &self.passage[..self.found.start];
        let skip = head.chars().count().saturating_sub(before);
        let (mut start, mut cut) = (head.char_indices().nth(skip).map_or(0, |(at, _)| at), self.cut.0);
        if skip > 0 {
            // At the start of a word, where there is one near.
            if let Some(space) = head[start..].find(' ').filter(|space| *space < 16) { start += space + 1 }
            cut = true;
        }
        let lead = if cut { "…" } else { "" };
        let text = format!("{lead}{}{}", &self.passage[start..], if self.cut.1 { "…" } else { "" });
        let shift = lead.len() as isize - start as isize;
        let found = (self.found.start as isize + shift) as usize..(self.found.end as isize + shift) as usize;
        (text, found)
    }
}

/// What a search came to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    /// The places kept, the papers with most first, each paper's in the order of `Place`.
    pub hits: Vec<Hit>,
    /// How many places each paper has, those not kept counted too.
    pub counts: BTreeMap<String, usize>,
    pub total: usize,
    /// Papers whose full text was looked through.
    pub searched: usize,
    /// Papers with a full-text file that gave no text: what they say was not looked through.
    pub textless: Vec<String>,
    /// Papers with a full-text file not read yet: not looked through either.
    pub unread: Vec<String>,
    /// Stopped before the end: a newer search was asked for.
    pub stopped: bool,
}

/// Which page texts a search looks through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Texts {
    /// Every file's: the full texts and the attachments.
    All,
    /// The attachments' only (the tool looks through the full texts in its own way).
    Attachments,
}

/// A search going on: places are handed to it, and `finish` sorts them.
pub struct Search {
    wanted: String,
    per_paper: usize,
    kept: BTreeMap<String, usize>,
    found: Found,
}

impl Search {
    /// `None` when the query has nothing to look for (no letter, no digit). At most `per_paper` places of a paper are kept.
    pub fn new(query: &str, per_paper: usize) -> Option<Self> {
        if bare(query).is_empty() { return None }
        Some(Self { wanted: query.to_string(), per_paper, kept: BTreeMap::new(), found: Found::default() })
    }

    fn look(&mut self, paper: &str, place: Place, file: &str, page: usize, text: &str, mark: Option<&MarkHit>) {
        for (n, found) in find_text(text, &self.wanted).into_iter().enumerate() {
            // A mark is one place however often its words come back in it.
            if n > 0 && mark.is_some() { break }
            self.found.total += 1;
            *self.found.counts.entry(paper.to_string()).or_default() += 1;
            let kept = self.kept.entry(paper.to_string()).or_default();
            if *kept >= self.per_paper { continue }
            *kept += 1;
            let (passage, at, cut) = passage(text, found.clone());
            self.found.hits.push(Hit { paper_id: paper.to_string(), place, file: file.to_string(), page, passage, found: at, cut, at: found, mark: mark.cloned() });
        }
    }

    /// A paper's title, abstract and notes.
    pub fn record(&mut self, paper: &Paper) {
        self.look(&paper.id, Place::Title, "", 0, &paper.title, None);
        self.look(&paper.id, Place::Abstract, "", 0, &paper.abstract_text, None);
        self.look(&paper.id, Place::Notes, "", 0, &paper.notes, None);
    }

    /// A reading mark: the text it marks, and what was written on it.
    pub fn mark(&mut self, mark: &Annotation) {
        for (text, written) in [(&mark.quote, false), (&mark.note, true)] {
            let hit = MarkHit { id: mark.id.clone(), kind: mark.kind, boxes: mark.boxes.clone(), written };
            self.look(&mark.paper_id, Place::Mark, &mark.resource_id, mark.page, text, Some(&hit));
        }
    }

    /// The pages of a file.
    pub fn pages(&mut self, file: &PaperFile, pages: &[String]) {
        for (n, page) in pages.iter().enumerate() { self.look(&file.paper_id, Place::Text, &file.id, n + 1, page, None) }
    }

    /// The places sorted: the papers with most first (of as many, in `order`), each paper's by `Place`, then as they came.
    pub fn finish(mut self, order: &[&str]) -> Found {
        let rank: BTreeMap<&str, usize> = order.iter().enumerate().map(|(n, id)| (*id, n)).collect();
        let counts = &self.found.counts;
        self.found.hits.sort_by_key(|hit| (std::cmp::Reverse(counts.get(&hit.paper_id).copied().unwrap_or(0)), rank.get(hit.paper_id.as_str()).copied().unwrap_or(usize::MAX), hit.paper_id.clone(), hit.place));
        self.found
    }
}

/// Look for words through a library: the records of its papers, their marks, and the page texts `texts` says,
/// `per_paper` places of a paper kept at most. `stop` is asked between files; when it says so, what was found so far
/// comes back with `stopped`. `only` keeps to one paper. Reads only.
pub fn in_library(store: &Store, query: &str, only: Option<&str>, texts: Texts, per_paper: usize, stop: &dyn Fn() -> bool) -> Result<Found> {
    let Some(mut search) = Search::new(query, per_paper) else { return Ok(Found::default()) };
    let papers: Vec<Paper> = store.list_papers()?.into_iter().filter(|p| only.is_none_or(|id| p.id == id)).collect();
    for paper in &papers { search.record(paper) }
    for mark in store.pdf_annotations(only.unwrap_or(""), "")? { search.mark(&mark) }
    let known: BTreeSet<&str> = papers.iter().map(|p| p.id.as_str()).collect();
    let mut read: BTreeSet<String> = BTreeSet::new();
    let mut stopped = false;
    store.each_text(|file, pages| {
        if stop() {
            stopped = true;
            return false;
        }
        if !known.contains(file.paper_id.as_str()) { return true }
        if file.kind == "fulltext" { read.insert(file.paper_id.clone()); }
        if texts == Texts::All || file.kind != "fulltext" { search.pages(&file, &pages) }
        true
    })?;
    let order: Vec<&str> = papers.iter().map(|p| p.id.as_str()).collect();
    let mut found = search.finish(&order);
    found.stopped = stopped;
    found.searched = read.len();
    for paper in papers.iter().filter(|p| !read.contains(&p.id)) {
        match paper.fulltext.as_str() {
            "textless" => found.textless.push(paper.id.clone()),
            "stored" => found.unread.push(paper.id.clone()),
            _ => {}
        }
    }
    Ok(found)
}

/// How far a passage reaches round the words before it is cut, in characters.
const BEFORE: usize = 160;
const AFTER: usize = 240;
/// A sentence shorter than this (in compared characters) is too short to be found again on its page: the text round it is taken.
const SHORTEST: usize = 16;

/// The sentence of `text` the words at `found` stand in, on one line, the words' range in it, and whether it was cut.
fn passage(text: &str, found: Range<usize>) -> (String, Range<usize>, (bool, bool)) {
    let (mut start, mut end, mut cut) = bounds(text, &found, true);
    if bare(&text[start..end]).chars().count() < SHORTEST { (start, end, cut) = bounds(text, &found, false) }
    // The spacing made one space, the words' place followed through it.
    let (mut out, mut at) = (String::with_capacity(end - start), found.clone());
    let mut space = false;
    for (i, c) in text[start..end].char_indices() {
        let i = start + i;
        if i == found.start { at.start = out.len() + usize::from(space && !out.is_empty()) }
        if i == found.end { at.end = out.len() }
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() { out.push(' ') }
        space = false;
        out.push(c);
    }
    if found.end >= end { at.end = out.len() }
    // The end of the words never stands on the space that follows them.
    if at.end > at.start && out[..at.end].ends_with(' ') { at.end -= 1 }
    (out, at, cut)
}

/// Where the sentence round `found` starts and ends in `text`, and whether it was cut short there. `sentences`
/// false takes the text round the words whatever the sentences.
fn bounds(text: &str, found: &Range<usize>, sentences: bool) -> (usize, usize, (bool, bool)) {
    let ends = |c: char, next: Option<char>| sentences && (matches!(c, '。' | '！' | '？' | '；') || matches!(c, '.' | '!' | '?') && next.is_none_or(char::is_whitespace));
    let (reach_back, reach_on) = if sentences { (BEFORE, AFTER) } else { (BEFORE / 2, AFTER / 2) };
    // Back from the words to the end of the sentence before.
    let (mut start, mut cut_start, mut next, mut space) = (0, false, text[found.start..].chars().next(), None);
    for (n, (i, c)) in text[..found.start].char_indices().rev().enumerate() {
        if ends(c, next) {
            start = i + c.len_utf8();
            break;
        }
        if c.is_whitespace() { space = Some(i + c.len_utf8()) }
        if n >= reach_back {
            (start, cut_start) = (space.unwrap_or(i), true);
            break;
        }
        next = Some(c);
    }
    // On from the words to the end of their sentence.
    let rest = &text[found.end..];
    let (mut end, mut cut_end, mut space) = (text.len(), false, None);
    let mut chars = rest.char_indices().peekable();
    let mut n = 0;
    while let Some((i, c)) = chars.next() {
        if ends(c, chars.peek().map(|(_, c)| *c)) {
            end = found.end + i + c.len_utf8();
            break;
        }
        if c.is_whitespace() { space = Some(found.end + i) }
        if n >= reach_on {
            (end, cut_end) = (space.unwrap_or(found.end + i), true);
            break;
        }
        n += 1;
    }
    (start, end, (cut_start, cut_end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(text: &str, query: &str) -> Vec<(String, String, (bool, bool))> {
        find_text(text, query).into_iter().map(|found| {
            let (passage, at, cut) = passage(text, found);
            (passage.clone(), passage[at].to_string(), cut)
        }).collect()
    }

    #[test]
    fn the_sentence_round_the_words() {
        let page = "Roots grow.  Libraries were prepared\nwith 10x Genom-\nics Chromium. The first run failed.";
        // Over a line break and a word hyphenated at the line's end, without case; the sentence is the page's own words.
        assert_eq!(hits(page, "10X genomics"), [("Libraries were prepared with 10x Genom- ics Chromium.".to_string(), "10x Genom- ics".to_string(), (false, false))]);
        // Every place, apart; none in an empty query or one of marks only.
        assert_eq!(hits(page, "r").len(), page.matches(['r', 'R']).count());
        assert!(hits(page, "").is_empty() && hits(page, " - ").is_empty() && hits(page, "absent").is_empty());
        // Chinese has no spaces: a word is found inside a run of it, and its sentence ends at 。.
        let chinese = "根尖分生组织维持干细胞。生长素在根尖形成浓度梯度，决定细胞的命运与分化方向。";
        assert_eq!(hits(chinese, "浓度梯度"), [("生长素在根尖形成浓度梯度，决定细胞的命运与分化方向。".to_string(), "浓度梯度".to_string(), (false, false))]);
        // A sentence too short to be found again on its page: the text round it.
        assert_eq!(hits("Fig. 2. Auxin. Roots respond to it in hours.", "auxin")[0].0, "Fig. 2. Auxin. Roots respond to it in hours.");
        // A long sentence is cut at a word, and says so.
        let long = format!("{} needle {}", "word ".repeat(80), "more ".repeat(80));
        let (passage, words, cut) = hits(&long, "needle").remove(0);
        assert_eq!((words.as_str(), cut), ("needle", (true, true)));
        assert!(passage.starts_with("word") && passage.ends_with("more") && passage.chars().count() < 420, "{passage}");
    }

    #[test]
    fn shown_with_the_words_in_view() {
        let long = format!("{}the needle here.", "word ".repeat(60));
        let mut search = Search::new("needle", 10).unwrap();
        search.look("P1", Place::Text, "R1", 3, &long, None);
        let hit = &search.found.hits[0];
        let (text, at) = hit.shown(20);
        assert_eq!(&text[at.clone()], "needle");
        assert!(text.starts_with('…') && text[..at.start].chars().count() <= 21, "{text}");
        let (whole, at) = Hit { passage: "A needle.".into(), found: 2..8, cut: (false, false), ..hit.clone() }.shown(20);
        assert_eq!((whole.as_str(), &whole[at]), ("A needle.", "needle"));
    }

    #[test]
    fn every_place_of_a_library() {
        let dir = std::env::temp_dir().join(format!("wisp-find-{}", crate::model::new_id("T")));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = Store::in_memory(&dir).unwrap();
        let first = store.add_paper(Paper { title: "Auxin gradients in the root".into(), abstract_text: "We map auxin.".into(), ..Default::default() }).unwrap();
        let second = store.add_paper(Paper { title: "Leaves".into(), notes: "Compare with the AUXIN maxima paper.".into(), ..Default::default() }).unwrap();
        let other = store.add_paper(Paper { title: "Nothing".into(), ..Default::default() }).unwrap();
        let text = dir.join("a.pdf");
        std::fs::write(&text, "Auxin moves.").unwrap();
        let file = store.import_file(&first.id, &text, "fulltext").unwrap();
        store.save_fulltext_pages(&file.id, &Ok(vec!["The root tip.".into(), "Polar transport of auxin\nsets the maximum.".into()])).unwrap();
        let unread = dir.join("b.md");
        std::fs::write(&unread, "x").unwrap();
        store.import_file(&other.id, &unread, "fulltext").unwrap();
        store.add_pdf_annotation(&first.id, &file.id, 2, Kind::Highlight, "Polar transport of auxin", &[[1., 2., 3., 4.]]).unwrap();
        let leaf = store.import_file(&second.id, &text, "fulltext").unwrap();
        let note = store.add_pdf_shape(&second.id, &leaf.id, 1, Kind::Note, [5., 5., 5., 5.], "auxin again?").unwrap();
        let found = in_library(&store, "auxin", None, Texts::All, 50, &|| false).unwrap();
        let places: Vec<(&str, Place, usize)> = found.hits.iter().map(|h| (h.paper_id.as_str(), h.place, h.page)).collect();
        assert_eq!(places, [(first.id.as_str(), Place::Title, 0), (first.id.as_str(), Place::Abstract, 0), (first.id.as_str(), Place::Mark, 2), (first.id.as_str(), Place::Text, 2), (second.id.as_str(), Place::Notes, 0), (second.id.as_str(), Place::Mark, 1)]);
        assert_eq!((found.total, found.counts[&first.id], found.searched, found.unread.len(), found.stopped), (6, 4, 1, 2, false));
        assert!(found.unread.contains(&other.id) && found.textless.is_empty());
        let written = found.hits.iter().find(|h| h.mark.as_ref().is_some_and(|m| m.id == note.id)).unwrap();
        assert!(written.mark.as_ref().unwrap().written && written.file == leaf.id && written.passage == "auxin again?");
        assert_eq!(found.hits[3].passage, "Polar transport of auxin sets the maximum.");
        // Kept to a number a paper; counted all the same. One paper only; the attachments' texts only; stopped.
        let few = in_library(&store, "auxin", None, Texts::All, 1, &|| false).unwrap();
        assert_eq!((few.hits.len(), few.total), (2, 6));
        assert!(in_library(&store, "auxin", Some(&second.id), Texts::All, 50, &|| false).unwrap().hits.iter().all(|h| h.paper_id == second.id));
        assert!(in_library(&store, "auxin", None, Texts::Attachments, 50, &|| false).unwrap().hits.iter().all(|h| h.place != Place::Text));
        assert!(in_library(&store, "auxin", None, Texts::All, 50, &|| true).unwrap().stopped);
        assert_eq!(in_library(&store, " … ", None, Texts::All, 50, &|| false).unwrap(), Found::default());
        std::fs::remove_dir_all(dir).ok();
    }
}
