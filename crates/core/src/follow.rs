//! 翻译 (the translation mode of 写作): one manuscript that holds its text in several languages. The language
//! it is written in (`Manuscript::language`) is the primary, 主稿; each other is a version that follows it.
//!
//! Structure is the primary's alone: its outline, sections, the order of its paragraphs, its citations and
//! figures. A version keeps, for each unit of the primary (the title, a section's name, a block of a section as
//! `bilingual::blocks` cuts it), a `Kept`: the translation, the primary's text it was made from (`source`), the
//! text it now stands beside (`anchor`), and whether the person changed it by hand. Citations stay placeholders
//! (`[@1]`) in what is kept, numbered as the citations of the block it stands beside, so they read the same in
//! every language.
//!
//! After the primary changed, `align` finds each kept translation its unit again (the same text first, then a
//! close one near where it stood, then the one left in its place), and `settle` writes that down: a
//! translation whose unit is gone is dropped when a model made it (it is in the library's translations still),
//! and set aside (`gone`) when the person had changed it, never dropped. A unit then has one of four `State`s:
//! 未译, 已译, 手改过, 原文已改. One that a model made follows by itself (`wanted`); one changed by hand waits for
//! the person: 重译, 只带改动 (`carry_prompt`, shown as a comparison to take or leave), or 已核对.
//!
//! A version is read, copied and exported as a manuscript of its own (`rendered`), and can be made the primary
//! (`swap`): the pairing stays and its direction turns round.

use crate::bilingual::{self, Block, Kind};
use crate::manuscript::{self, Manuscript};
use crate::markdown;
use crate::store::Store;
use crate::time::now_iso;
use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};
use std::collections::HashMap;
use std::ops::Range;

/// The tables, made with the library's own (`store::SCHEMA`). An older build does not know them and reads the
/// rest of the database as before; its manuscripts are the primaries alone.
pub(crate) const SCHEMA: &str = r#"
-- A language a manuscript follows itself in, besides its own.
CREATE TABLE IF NOT EXISTS manuscript_versions (
    manuscript_id TEXT NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE,
    -- A code of `manuscript::LANGUAGES`.
    language TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (manuscript_id, language)
);
-- What a version keeps of one unit of its manuscript: the title, a section's name, a block of a section's text.
CREATE TABLE IF NOT EXISTS version_blocks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    manuscript_id TEXT NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE,
    language TEXT NOT NULL,
    -- The section it stands in; empty for the title.
    section_id TEXT NOT NULL DEFAULT '',
    -- title, name or text.
    part TEXT NOT NULL DEFAULT 'text',
    -- Its place among the manuscript's units when it was last found.
    position INTEGER NOT NULL DEFAULT 0,
    -- The primary's text it stands beside, and the one it was made from, citations as placeholders.
    anchor TEXT NOT NULL,
    source TEXT NOT NULL,
    -- The translation, citations as placeholders numbered as the anchor's.
    body TEXT NOT NULL,
    -- The anchor's citations as written, a JSON list: the first is [@1].
    cites TEXT NOT NULL DEFAULT '[]',
    edited INTEGER NOT NULL DEFAULT 0,
    -- When its unit left the primary: set aside, kept; empty while it stands.
    gone_at TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_version_blocks ON version_blocks(manuscript_id, language);
"#;

/// What a unit of a manuscript is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Part {
    Title,
    /// A section's name.
    Name,
    /// A block of a section's text.
    #[default]
    Text,
}

impl Part {
    pub fn key(self) -> &'static str {
        match self {
            Part::Title => "title",
            Part::Name => "name",
            Part::Text => "text",
        }
    }

    fn from_key(key: &str) -> Part {
        match key {
            "title" => Part::Title,
            "name" => Part::Name,
            _ => Part::Text,
        }
    }
}

/// A unit of the primary as it now stands: what a version keeps a translation of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// The section it stands in; empty for the title.
    pub section: String,
    pub part: Part,
    /// For a block, its place among the section's `bilingual::blocks`.
    pub block: usize,
    /// What is translated, citations as placeholders.
    pub sent: String,
    /// Its citations as written: the first is [@1].
    pub cites: Vec<String>,
}

/// What a version keeps of one unit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    /// 0 until it is saved.
    pub id: i64,
    pub section: String,
    pub part: Part,
    pub position: usize,
    /// The primary's text it stands beside.
    pub anchor: String,
    /// The primary's text it was made from: the anchor, unless the primary changed since.
    pub source: String,
    /// The translation, citations as placeholders numbered as the anchor's.
    pub text: String,
    /// The anchor's citations as written.
    pub cites: Vec<String>,
    /// The person changed it by hand: a model never writes over it.
    pub edited: bool,
    /// When its unit left the primary; empty while it stands.
    pub gone: String,
}

/// How a unit of a version stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// 未译: nothing kept for it.
    Untranslated,
    /// 已译: made from the primary as it stands, and not changed since.
    Translated,
    /// 手改过: changed by hand, from the primary as it stands.
    Edited,
    /// 原文已改: the primary changed since it was made.
    Changed,
}

impl State {
    /// The name the window says it by, a key of `locales/en.json`.
    pub fn label(self) -> &'static str {
        match self {
            State::Untranslated => "未译",
            State::Translated => "已译",
            State::Edited => "手改过",
            State::Changed => "原文已改",
        }
    }
}

/// The state of a unit, with what is kept of it (paired by `align` and `settle`).
pub fn state(kept: Option<&Kept>) -> State {
    match kept {
        None => State::Untranslated,
        Some(kept) if kept.source != kept.anchor => State::Changed,
        Some(kept) if kept.edited => State::Edited,
        Some(_) => State::Translated,
    }
}

/// The units of a manuscript, in the order read: the title, then each section's name and its blocks that say
/// something. A section whose name is the manuscript's own (`Section::bare`) has no name of its own to translate.
pub fn units(manuscript: &Manuscript) -> Vec<Unit> {
    let mut units = vec![];
    let named = |section: &str, part, text: &str| Unit { section: section.to_string(), part, block: 0, sent: text.trim().to_string(), cites: vec![] };
    if !manuscript.title.trim().is_empty() { units.push(named("", Part::Title, &manuscript.title)) }
    for section in &manuscript.sections {
        if !section.bare && !section.name.trim().is_empty() { units.push(named(&section.id, Part::Name, &section.name)) }
        for (b, block) in bilingual::blocks(&section.body).into_iter().enumerate() {
            if block.sent.is_empty() { continue }
            let cites = block.cites.iter().map(|range| section.body[range.clone()].to_string()).collect();
            units.push(Unit { section: section.id.clone(), part: Part::Text, block: b, sent: block.sent, cites });
        }
    }
    units
}

/// How alike two texts are, 0 to 1: the pairs of letters next to each other that they share (Dice), spaces and
/// citation placeholders left out, so a Chinese paragraph and an English one are measured alike.
pub fn closeness(a: &str, b: &str) -> f32 {
    let letters = |text: &str| -> Vec<char> { strip_placeholders(text).chars().filter(|c| !c.is_whitespace()).collect() };
    let (a, b) = (letters(a), letters(b));
    if a == b { return 1. }
    if a.len() < 2 || b.len() < 2 { return 0. }
    let mut grams: HashMap<(char, char), i32> = HashMap::new();
    for pair in a.windows(2) { *grams.entry((pair[0], pair[1])).or_default() += 1 }
    let mut shared = 0;
    for pair in b.windows(2) {
        if let Some(left) = grams.get_mut(&(pair[0], pair[1])).filter(|left| **left > 0) {
            *left -= 1;
            shared += 1;
        }
    }
    2. * shared as f32 / (a.len() + b.len() - 2) as f32
}

/// The text without its citation placeholders.
fn strip_placeholders(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("[@") {
        let after = &rest[at + 2..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        out.push_str(&rest[..at]);
        if digits > 0 && after[digits..].starts_with(']') { rest = &after[digits + 1..] } else {
            out.push_str("[@");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// The placeholders written in a text, in order: 1 for [@1].
pub fn placeholders(text: &str) -> Vec<usize> {
    let mut found = vec![];
    let mut rest = text;
    while let Some(at) = rest.find("[@") {
        let after = &rest[at + 2..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with(']') && let Ok(n) = after[..digits].parse() { found.push(n) }
        rest = after;
    }
    found
}

/// Whether a translation has the citations of what it translates, each in its place at least once and no other:
/// a translation without them would lose a citation when it is read, copied or made the primary.
pub fn cites_kept(sent: &str, text: &str) -> bool {
    let (mut wanted, mut have) = (placeholders(sent), placeholders(text));
    wanted.sort_unstable();
    wanted.dedup();
    have.sort_unstable();
    have.dedup();
    wanted == have
}

/// The keys a citation as written names (`[@Li2019; @Wang2020]` names Li2019 and Wang2020).
fn cite_keys(cite: &str) -> Vec<&str> {
    cite.split('@').skip(1).map(|piece| piece.split(|c: char| c.is_whitespace() || matches!(c, ';' | ',' | ']')).next().unwrap_or("")).filter(|key| !key.is_empty()).collect()
}

/// A translation's placeholders numbered anew: they were those of the citations `then`, and are now those of
/// `now`. A citation is found again as it is written, else by the keys it names, else at its place; one that is
/// in neither stays as it was (`cites_kept` then says so).
pub fn renumbered(text: &str, then: &[String], now: &[String]) -> String {
    if then == now { return text.to_string() }
    let mut taken = vec![false; now.len()];
    let mut to: HashMap<usize, usize> = HashMap::new();
    let mut pass = |same: &dyn Fn(&str, &str) -> bool, to: &mut HashMap<usize, usize>| {
        for (i, cite) in then.iter().enumerate() {
            if to.contains_key(&(i + 1)) { continue }
            if let Some(j) = now.iter().enumerate().position(|(j, other)| !taken[j] && same(cite, other)) {
                taken[j] = true;
                to.insert(i + 1, j + 1);
            }
        }
    };
    pass(&|a, b| a == b, &mut to);
    pass(&|a, b| { let keys = cite_keys(a); !keys.is_empty() && keys == cite_keys(b) }, &mut to);
    pass(&|_, _| true, &mut to);
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("[@") {
        let after = &rest[at + 2..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        match after[..digits].parse::<usize>().ok().filter(|_| after[digits..].starts_with(']')).and_then(|n| to.get(&n)) {
            Some(n) => {
                out.push_str(&rest[..at]);
                out.push_str(&format!("[@{n}]"));
                rest = &after[digits + 1..];
            }
            None => {
                out.push_str(&rest[..at + 2]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The least two texts must share to be taken for the same paragraph changed.
const CLOSE: f32 = 0.4;

/// Which kept translation stands beside each unit of the primary as it now stands: the place in `kept`, or none.
///
/// The title and the sections' names go by where they stand. A block goes by its text: the same text first
/// (nearest to where it stood when it is there twice), then a text close to it (`closeness`), the nearer to its
/// old place the better, then, between two blocks found, the one left in the same place in the same section
/// (a paragraph rewritten). What was set aside comes back only for its very text, a paragraph put back.
pub fn align(kept: &[Kept], units: &[Unit]) -> Vec<Option<usize>> {
    let mut pairs: Vec<Option<usize>> = vec![None; units.len()];
    let mut used = vec![false; kept.len()];
    let standing = |k: usize| kept[k].gone.is_empty();
    // The title and the names.
    for (u, unit) in units.iter().enumerate().filter(|(_, unit)| unit.part != Part::Text) {
        let found = (0..kept.len()).filter(|k| !used[*k] && kept[*k].part == unit.part && kept[*k].section == unit.section).min_by_key(|k| !standing(*k));
        if let Some(k) = found {
            used[k] = true;
            pairs[u] = Some(k);
        }
    }
    let texts: Vec<usize> = (0..kept.len()).filter(|k| kept[*k].part == Part::Text).collect();
    // The same text.
    let mut by_anchor: HashMap<&str, Vec<usize>> = HashMap::new();
    for &k in &texts { by_anchor.entry(kept[k].anchor.as_str()).or_default().push(k) }
    for (u, unit) in units.iter().enumerate().filter(|(_, unit)| unit.part == Part::Text) {
        let Some(list) = by_anchor.get(unit.sent.as_str()) else { continue };
        let best = list.iter().copied().filter(|k| !used[*k]).min_by_key(|k| (!standing(*k), kept[*k].section != unit.section, kept[*k].position.abs_diff(u)));
        if let Some(k) = best {
            used[k] = true;
            pairs[u] = Some(k);
        }
    }
    // A text close to it, near where it stood.
    let (n, m) = (units.len().max(1) as f32, kept.iter().map(|k| k.position + 1).max().unwrap_or(1) as f32);
    let mut close: Vec<(f32, usize, usize)> = vec![];
    for (u, unit) in units.iter().enumerate().filter(|(u, unit)| unit.part == Part::Text && pairs[*u].is_none()) {
        for &k in texts.iter().filter(|k| !used[**k] && standing(**k)) {
            let near = closeness(&kept[k].anchor, &unit.sent);
            if near < CLOSE { continue }
            let drift = (kept[k].position as f32 / m - u as f32 / n).abs();
            close.push((near - 0.25 * drift + if kept[k].section == unit.section { 0.05 } else { 0. }, u, k));
        }
    }
    close.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, u, k) in close {
        if pairs[u].is_some() || used[k] { continue }
        used[k] = true;
        pairs[u] = Some(k);
    }
    // The one left in its place: a run of blocks not found between two that were, and as many kept between theirs, in one section.
    let mut u = 0;
    while u < units.len() {
        if units[u].part != Part::Text || pairs[u].is_some() {
            u += 1;
            continue;
        }
        let section = units[u].section.clone();
        let start = u;
        while u < units.len() && units[u].part == Part::Text && pairs[u].is_none() && units[u].section == section { u += 1 }
        let before = (0..start).rev().find_map(|v| pairs[v].filter(|_| units[v].part == Part::Text).map(|k| kept[k].position));
        let after = (u..units.len()).find_map(|v| pairs[v].filter(|_| units[v].part == Part::Text).map(|k| kept[k].position));
        let mut between: Vec<usize> = texts.iter().copied().filter(|k| !used[*k] && standing(*k) && kept[*k].section == section && before.is_none_or(|p| kept[*k].position > p) && after.is_none_or(|p| kept[*k].position < p)).collect();
        if between.len() != u - start { continue }
        between.sort_by_key(|k| kept[*k].position);
        for (v, k) in (start..u).zip(between) {
            used[k] = true;
            pairs[v] = Some(k);
        }
    }
    pairs
}

/// A version after `settle`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settled {
    /// What is kept, in the order of the units it stands beside, those set aside last.
    pub kept: Vec<Kept>,
    /// For each unit, its place in `kept`.
    pub pairs: Vec<Option<usize>>,
    /// The saved ones no longer kept: made by a model for a unit that is gone.
    pub dropped: Vec<i64>,
    /// How many were set aside by this settling: changed by hand, for a unit that is gone.
    pub set_aside: usize,
    /// Whether anything is to be saved.
    pub changed: bool,
}

/// What `align` found, written into what is kept: each translation beside its unit (anchor, section, place, its
/// placeholders numbered as the unit's citations), and those whose unit is gone dropped or set aside.
pub fn settle(kept: Vec<Kept>, units: &[Unit], pairs: &[Option<usize>], now: &str) -> Settled {
    let mut settled = Settled { pairs: vec![None; units.len()], ..Default::default() };
    let mut slot: Vec<Option<usize>> = vec![None; kept.len()];
    for (u, k) in pairs.iter().enumerate() {
        if let Some(k) = *k { slot[k] = Some(u) }
    }
    let mut placed: Vec<(usize, Kept)> = vec![];
    let mut aside: Vec<Kept> = vec![];
    for (k, mut one) in kept.into_iter().enumerate() {
        match slot[k] {
            Some(u) => {
                let unit = &units[u];
                let text = renumbered(&one.text, &one.cites, &unit.cites);
                let was = (one.anchor.clone(), one.section.clone(), one.position, one.gone.clone(), one.text.clone());
                one.anchor = unit.sent.clone();
                one.section = unit.section.clone();
                one.position = u;
                one.gone.clear();
                // The translation of a citation the source no longer has stays as it was, numbered as the source it was made from.
                one.text = text;
                one.cites = unit.cites.clone();
                settled.changed |= was != (one.anchor.clone(), one.section.clone(), one.position, one.gone.clone(), one.text.clone());
                placed.push((u, one));
            }
            None if !one.gone.is_empty() => aside.push(one),
            None if one.edited => {
                one.gone = now.to_string();
                settled.set_aside += 1;
                settled.changed = true;
                aside.push(one);
            }
            None => {
                if one.id != 0 { settled.dropped.push(one.id) }
                settled.changed = true;
            }
        }
    }
    placed.sort_by_key(|(u, _)| *u);
    for (u, one) in placed {
        settled.pairs[u] = Some(settled.kept.len());
        settled.kept.push(one);
    }
    settled.kept.extend(aside);
    settled
}

/// The units a model translates by itself: those with nothing kept, and those whose kept translation it made
/// and whose source has changed. Each unit's place in `units`.
pub fn wanted(kept: &[Kept], pairs: &[Option<usize>]) -> Vec<usize> {
    pairs.iter().enumerate().filter(|(_, k)| match k { None => true, Some(k) => !kept[*k].edited && kept[*k].source != kept[*k].anchor }).map(|(u, _)| u).collect()
}

/// A translation made of the unit as it stands: kept, a model's, from the source as it is.
pub fn made(unit: &Unit, text: &str) -> Kept {
    Kept { id: 0, section: unit.section.clone(), part: unit.part, position: 0, anchor: unit.sent.clone(), source: unit.sent.clone(), text: text.trim().to_string(), cites: unit.cites.clone(), edited: false, gone: String::new() }
}

/// Where a block's words stand in its section's text: a picture's caption, else the whole block.
fn words_range(body: &str, block: &Block) -> Option<Range<usize>> {
    match block.kind {
        Kind::Kept => None,
        Kind::Words => Some(block.range.clone()),
        Kind::Picture => markdown::picture_line(&body[block.range.clone()]).map(|(caption, _)| block.range.start + caption.start..block.range.start + caption.end),
    }
}

/// A section's text with the blocks that have a translation (`translated`: the block's place, the translation)
/// written in its place, each citation as written. Beside it, each such block's place in the new text.
fn translated_body(body: &str, translated: &HashMap<usize, &str>) -> (String, Vec<(usize, Range<usize>)>) {
    let (mut out, mut at, mut placed) = (String::new(), 0, vec![]);
    for (b, block) in bilingual::blocks(body).iter().enumerate() {
        let (Some(text), Some(words)) = (translated.get(&b), words_range(body, block)) else { continue };
        out.push_str(&body[at..words.start]);
        let start = out.len() - (words.start - block.range.start);
        out.push_str(&bilingual::restored(text, body, block, &[]));
        placed.push((b, start..out.len() + (block.range.end - words.end)));
        at = words.end;
    }
    out.push_str(&body[at..]);
    (out, placed)
}

/// The version as a manuscript of its own, in its language: the title, each section's name and each block in
/// translation where it has one, as the primary has it where not (a manuscript to read, copy or export whole).
pub fn rendered(manuscript: &Manuscript, language: &str, kept: &[Kept], units: &[Unit], pairs: &[Option<usize>]) -> Manuscript {
    let text = |u: usize| pairs.get(u).copied().flatten().map(|k| kept[k].text.as_str()).filter(|text| !text.trim().is_empty());
    let mut out = Manuscript { language: language.to_string(), ..manuscript.clone() };
    for (u, unit) in units.iter().enumerate() {
        let Some(text) = text(u) else { continue };
        match unit.part {
            Part::Title => out.title = text.to_string(),
            Part::Name => if let Some(section) = out.sections.iter_mut().find(|s| s.id == unit.section) { section.name = text.to_string() },
            Part::Text => {}
        }
    }
    for section in &mut out.sections {
        let translated: HashMap<usize, &str> = units.iter().enumerate().filter(|(_, unit)| unit.part == Part::Text && unit.section == section.id).filter_map(|(u, unit)| Some((unit.block, text(u)?))).collect();
        if !translated.is_empty() { section.body = translated_body(&section.body, &translated).0 }
    }
    out
}

/// What keeps a version from being made the primary: how many of its units are 未译, 原文已改, or have lost a
/// citation of what they translate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unready {
    pub untranslated: usize,
    pub changed: usize,
    pub cites: usize,
}

/// The manuscript in a version's words, and what the old primary becomes: a version of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Swap {
    pub title: Option<String>,
    /// Each section's new name and text, by its id.
    pub sections: Vec<(String, String, String)>,
    /// The old primary as a version of the new one: its words, each changed by hand (the person wrote them).
    pub kept: Vec<Kept>,
}

/// What 设为主稿 would write: the version's words become the manuscript's and the primary's words a version
/// of them, paired as before. Refused while a unit is 未译 or 原文已改, or its translation lost a citation.
pub fn swap(manuscript: &Manuscript, kept: &[Kept], units: &[Unit], pairs: &[Option<usize>]) -> std::result::Result<Swap, Unready> {
    let mut unready = Unready::default();
    for (u, unit) in units.iter().enumerate() {
        let one = pairs[u].map(|k| &kept[k]);
        match state(one) {
            State::Untranslated => unready.untranslated += 1,
            State::Changed => unready.changed += 1,
            _ if !cites_kept(&unit.sent, &one.unwrap().text) => unready.cites += 1,
            _ => {}
        }
    }
    if unready != Unready::default() { return Err(unready) }
    let text = |u: usize| kept[pairs[u].unwrap()].text.as_str();
    let mut back: Vec<Kept> = vec![];
    let old = |unit: &Unit, now: &str, cites: Vec<String>, from: &str| Kept { id: 0, section: unit.section.clone(), part: unit.part, position: 0, anchor: now.to_string(), source: now.to_string(), text: from.to_string(), cites, edited: true, gone: String::new() };
    let mut title = None;
    for (u, unit) in units.iter().enumerate().filter(|(_, unit)| unit.part == Part::Title) {
        title = Some(text(u).to_string());
        back.push(old(unit, text(u), vec![], &unit.sent));
    }
    let mut sections = vec![];
    for section in &manuscript.sections {
        let mine: Vec<(usize, &Unit)> = units.iter().enumerate().filter(|(_, unit)| unit.section == section.id).collect();
        let name = mine.iter().find(|(_, unit)| unit.part == Part::Name).map_or(section.name.clone(), |(u, _)| text(*u).to_string());
        if let Some((u, unit)) = mine.iter().find(|(_, unit)| unit.part == Part::Name) { back.push(old(unit, text(*u), vec![], &unit.sent)) }
        let translated: HashMap<usize, &str> = mine.iter().filter(|(_, unit)| unit.part == Part::Text).map(|(u, unit)| (unit.block, text(*u))).collect();
        let (body, placed) = translated_body(&section.body, &translated);
        // Each block of the new text is paired with the old block it came from: the first that came of it, should a translation be two.
        let mut given = vec![false; placed.len()];
        for (b, block) in bilingual::blocks(&body).into_iter().enumerate() {
            if block.sent.is_empty() { continue }
            let Some(p) = placed.iter().position(|(_, range)| range.contains(&block.range.start)) else { continue };
            if std::mem::replace(&mut given[p], true) { continue }
            let Some((_, unit)) = mine.iter().find(|(_, unit)| unit.part == Part::Text && unit.block == placed[p].0) else { continue };
            let cites: Vec<String> = block.cites.iter().map(|range| body[range.clone()].to_string()).collect();
            // The old words' placeholders numbered as the new text's citations: those are the old ones, in the order the translation put them.
            let from = renumbered(&unit.sent, &unit.cites, &cites);
            back.push(Kept { section: section.id.clone(), position: b, ..old(unit, &block.sent, cites, &from) });
        }
        sections.push((section.id.clone(), name, body));
    }
    Ok(Swap { title, sections, kept: back })
}

/// The instructions 只带改动 is asked with: the primary changed under a translation the person made their own.
pub fn carry_prompt(code: &str) -> String {
    let language = if code == "zh" { "简体中文" } else { crate::model::label(&manuscript::LANGUAGES, code) };
    format!(
        "你是翻译助手。一段论文稿件的原文改过了，它的{language}译文是用户自己改定的。下面给你改前的原文、改后的原文和现在的译文：\
         只把原文的这处改动带进译文，其余部分一字不动，保留用户的措辞。\n\
         形如 [@1] 的记号是引用的占位，现在的译文里已经按改后的原文编号，原样留在对应的位置；Markdown 记号、公式、数字、基因名和物种学名原样。\n\
         只输出改好的整段译文，不加解释、不加引号。"
    )
}

/// What 只带改动 sends: the source then and now, and the translation as the person left it.
pub fn carry_question(before: &str, after: &str, translation: &str) -> String {
    format!("改前的原文：\n{}\n\n改后的原文：\n{}\n\n现在的译文：\n{}", before.trim(), after.trim(), translation.trim())
}

/// What the person wrote in a block, as it is kept: each citation of the block written as it is in the
/// primary becomes its placeholder again (the same citation twice, the next of its places each time).
pub fn placeheld(edited: &str, cites: &[String]) -> String {
    let mut out = String::new();
    let mut taken = vec![false; cites.len()];
    let mut at = 0;
    'scan: while at < edited.len() {
        if edited[at..].starts_with("[@") {
            // The longest citation written here.
            let mut best: Option<usize> = None;
            for (n, cite) in cites.iter().enumerate() {
                if !edited[at..].starts_with(cite.as_str()) { continue }
                let better = match best { None => true, Some(b) => cite.len() > cites[b].len() || (cite.len() == cites[b].len() && taken[b] && !taken[n]) };
                if better { best = Some(n) }
            }
            if let Some(n) = best {
                // The same citation twice in the block: the first of its places not yet given.
                let n = (0..cites.len()).find(|m| !taken[*m] && cites[*m] == cites[n]).unwrap_or(n);
                taken[n] = true;
                out.push_str(&format!("[@{}]", n + 1));
                at += cites[n].len();
                continue 'scan;
            }
        }
        let c = edited[at..].chars().next().unwrap();
        out.push(c);
        at += c.len_utf8();
    }
    out.trim().to_string()
}

/// A kept translation as the person edits it: each placeholder the citation as written.
pub fn as_written(text: &str, cites: &[String]) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("[@") {
        let after = &rest[at + 2..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        match after[..digits].parse::<usize>().ok().filter(|n| *n > 0 && after[digits..].starts_with(']')).and_then(|n| cites.get(n - 1)) {
            Some(cite) => {
                out.push_str(&rest[..at]);
                out.push_str(cite);
                rest = &after[digits + 1..];
            }
            None => {
                out.push_str(&rest[..at + 2]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn read_kept(r: &rusqlite::Row) -> rusqlite::Result<Kept> {
    let cites: String = r.get(8)?;
    Ok(Kept {
        id: r.get(0)?,
        section: r.get(1)?,
        part: Part::from_key(&r.get::<_, String>(2)?),
        position: r.get::<_, i64>(3)?.max(0) as usize,
        anchor: r.get(4)?,
        source: r.get(5)?,
        text: r.get(6)?,
        edited: r.get::<_, i64>(7)? != 0,
        cites: serde_json::from_str(&cites).unwrap_or_default(),
        gone: r.get(9)?,
    })
}

const KEPT_COLUMNS: &str = "id, section_id, part, position, anchor, source, body, edited, cites, gone_at";

impl Store {
    /// The languages each manuscript follows itself in, besides its own, in the order they were added.
    pub fn version_languages(&self) -> Result<HashMap<String, Vec<String>>> {
        let mut stmt = self.db.prepare("SELECT manuscript_id, language FROM manuscript_versions ORDER BY created_at, language")?;
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (manuscript, language) = row?;
            out.entry(manuscript).or_default().push(language);
        }
        Ok(out)
    }

    /// A language for a manuscript to follow itself in. What an earlier version of that language kept is its again.
    pub fn add_version(&mut self, manuscript_id: &str, language: &str) -> Result<()> {
        if !manuscript::is_language(language) { bail!("不认识这种语言：{language}") }
        let own: Option<String> = self.db.query_row("SELECT language FROM manuscripts WHERE id = ?1", [manuscript_id], |r| r.get(0)).optional()?;
        match own {
            None => bail!("没有这份稿件：{manuscript_id}"),
            Some(own) if own == language => bail!("稿件本身就是这种语言。"),
            Some(_) => {}
        }
        self.db.execute("INSERT OR IGNORE INTO manuscript_versions(manuscript_id, language, created_at) VALUES (?1, ?2, ?3)", params![manuscript_id, language, now_iso()])?;
        Ok(())
    }

    /// A version no longer followed. What it kept stays, for the language added again.
    pub fn remove_version(&mut self, manuscript_id: &str, language: &str) -> Result<()> {
        self.db.execute("DELETE FROM manuscript_versions WHERE manuscript_id = ?1 AND language = ?2", params![manuscript_id, language])?;
        Ok(())
    }

    /// What a version keeps, those standing in the order of their units, then those set aside.
    pub fn version_blocks(&self, manuscript_id: &str, language: &str) -> Result<Vec<Kept>> {
        let mut stmt = self.db.prepare(&format!("SELECT {KEPT_COLUMNS} FROM version_blocks WHERE manuscript_id = ?1 AND language = ?2 ORDER BY gone_at != '', position, id"))?;
        Ok(stmt.query_map(params![manuscript_id, language], read_kept)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn put_kept(db: &rusqlite::Connection, manuscript_id: &str, language: &str, kept: &mut Kept) -> Result<()> {
        let cites = serde_json::to_string(&kept.cites)?;
        if kept.id == 0 {
            db.execute("INSERT INTO version_blocks(manuscript_id, language, section_id, part, position, anchor, source, body, cites, edited, gone_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)", params![manuscript_id, language, kept.section, kept.part.key(), kept.position as i64, kept.anchor, kept.source, kept.text, cites, kept.edited as i64, kept.gone, now_iso()])?;
            kept.id = db.last_insert_rowid();
        } else {
            db.execute("UPDATE version_blocks SET section_id = ?3, part = ?4, position = ?5, anchor = ?6, source = ?7, body = ?8, cites = ?9, edited = ?10, gone_at = ?11, updated_at = ?12 WHERE id = ?13 AND manuscript_id = ?1 AND language = ?2", params![manuscript_id, language, kept.section, kept.part.key(), kept.position as i64, kept.anchor, kept.source, kept.text, cites, kept.edited as i64, kept.gone, now_iso(), kept.id])?;
        }
        Ok(())
    }

    /// One kept translation saved, a new one given its id.
    pub fn save_version_block(&mut self, manuscript_id: &str, language: &str, kept: &mut Kept) -> Result<()> {
        Self::put_kept(&self.db, manuscript_id, language, kept)
    }

    /// A version as `settle` left it: every kept translation saved, and those it dropped gone.
    pub fn save_version(&mut self, manuscript_id: &str, language: &str, kept: &mut [Kept], dropped: &[i64]) -> Result<()> {
        let tx = self.db.transaction()?;
        for id in dropped { tx.execute("DELETE FROM version_blocks WHERE id = ?1 AND manuscript_id = ?2 AND language = ?3", params![id, manuscript_id, language])?; }
        for one in kept.iter_mut() { Self::put_kept(&tx, manuscript_id, language, one)? }
        tx.commit()?;
        Ok(())
    }

    /// 设为主稿: the manuscript written in `language`, from `swap`, and what it was written in a version of it.
    /// Each section's text is kept as a snapshot first. What the new primary's version had set aside stays,
    /// under its language, as what a version of the old one had.
    pub fn swap_primary(&mut self, manuscript_id: &str, language: &str, swap: &mut Swap) -> Result<()> {
        if !manuscript::is_language(language) { bail!("不认识这种语言：{language}") }
        let Some(own) = self.db.query_row("SELECT language FROM manuscripts WHERE id = ?1", [manuscript_id], |r| r.get::<_, String>(0)).optional()? else { bail!("没有这份稿件：{manuscript_id}") };
        if own == language { bail!("这已经是主稿的语言。") }
        for (section, _, _) in &swap.sections { self.take_snapshot(section, "swap")?; }
        let now = now_iso();
        let tx = self.db.transaction()?;
        for (section, name, body) in &swap.sections {
            tx.execute("UPDATE manuscript_sections SET name = ?2, body = ?3, updated_at = ?4 WHERE id = ?1 AND manuscript_id = ?5", params![section, name, body, now, manuscript_id])?;
        }
        if let Some(title) = &swap.title { tx.execute("UPDATE manuscripts SET title = ?2 WHERE id = ?1", params![manuscript_id, title])?; }
        tx.execute("UPDATE manuscripts SET language = ?2, updated_at = ?3 WHERE id = ?1", params![manuscript_id, language, now])?;
        // The new primary's translations are its text now; the old primary's words are kept as its version.
        tx.execute("DELETE FROM version_blocks WHERE manuscript_id = ?1 AND language IN (?2, ?3) AND gone_at = ''", params![manuscript_id, language, own])?;
        tx.execute("DELETE FROM manuscript_versions WHERE manuscript_id = ?1 AND language = ?2", params![manuscript_id, language])?;
        tx.execute("INSERT OR IGNORE INTO manuscript_versions(manuscript_id, language, created_at) VALUES (?1, ?2, ?3)", params![manuscript_id, own, now])?;
        for one in swap.kept.iter_mut() {
            one.id = 0;
            Self::put_kept(&tx, manuscript_id, &own, one)?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::Section;

    fn section(id: &str, name: &str, body: &str) -> Section {
        Section { id: id.into(), name: name.into(), kind: "other".into(), body: body.into(), updated_at: String::new(), bare: false, level: 1 }
    }

    fn manuscript(sections: Vec<Section>) -> Manuscript {
        Manuscript { id: "M1".into(), title: "根的生长".into(), language: "zh".into(), sections, ..Default::default() }
    }

    const P1: &str = "根在紧实的土壤里长得很慢，这一点早有报道 [@Li2019]。";
    const P2: &str = "我们比较了三种土壤里主根的长度，每种二十株。";
    const P3: &str = "突变体的根毛更短，而且分布更稀疏。";
    const P4: &str = "这些结果说明根毛参与了对紧实土壤的应答。";

    /// A version made of every unit of the manuscript, each translation the source in brackets so it is told apart.
    fn translated(of: &Manuscript) -> (Vec<Unit>, Vec<Kept>) {
        let units = units(of);
        let mut kept: Vec<Kept> = units.iter().map(|unit| made(unit, &format!("<{}>", unit.sent))).collect();
        for (n, one) in kept.iter_mut().enumerate() { one.position = n }
        (units, kept)
    }

    /// The kept translation paired with each unit now, by what it was made from.
    fn found(kept: &[Kept], pairs: &[Option<usize>]) -> Vec<Option<String>> {
        pairs.iter().map(|k| k.map(|k| kept[k].source.clone())).collect()
    }

    #[test]
    fn a_manuscript_is_cut_into_units() {
        let m = manuscript(vec![section("S1", "引言", &format!("{P1}\n\n{P2}")), section("S2", "结果", "")]);
        let units = units(&m);
        let said: Vec<(Part, &str, &str)> = units.iter().map(|unit| (unit.part, unit.section.as_str(), unit.sent.as_str())).collect();
        assert_eq!(said, vec![(Part::Title, "", "根的生长"), (Part::Name, "S1", "引言"), (Part::Text, "S1", "根在紧实的土壤里长得很慢，这一点早有报道 [@1]。"), (Part::Text, "S1", P2), (Part::Name, "S2", "结果")]);
        assert_eq!(units[2].cites, vec!["[@Li2019]".to_string()]);
        assert_eq!(units[3].block, 1);
    }

    #[test]
    fn the_four_states() {
        let unit = Unit { section: "S1".into(), part: Part::Text, block: 0, sent: P2.into(), cites: vec![] };
        let mut one = made(&unit, "We compared…");
        assert_eq!(state(None), State::Untranslated);
        assert_eq!(state(Some(&one)), State::Translated);
        one.edited = true;
        assert_eq!(state(Some(&one)), State::Edited);
        one.anchor = P3.into();
        assert_eq!(state(Some(&one)), State::Changed);
        for s in [State::Untranslated, State::Translated, State::Edited, State::Changed] { assert!(!s.label().is_empty()) }
    }

    #[test]
    fn a_paragraph_edited_keeps_its_translation() {
        let before = manuscript(vec![section("S1", "引言", &format!("{P1}\n\n{P2}\n\n{P3}"))]);
        let (_, kept) = translated(&before);
        let after = manuscript(vec![section("S1", "引言", &format!("{P1}\n\n我们比较了四种土壤里主根的长度，每种三十株。\n\n{P3}"))]);
        let now = units(&after);
        let pairs = align(&kept, &now);
        assert_eq!(found(&kept, &pairs)[3].as_deref(), Some(P2));
        let settled = settle(kept, &now, &pairs, "t");
        let one = &settled.kept[settled.pairs[3].unwrap()];
        assert_eq!(state(Some(one)), State::Changed);
        // Made by a model: it follows by itself.
        assert_eq!(wanted(&settled.kept, &settled.pairs), vec![3]);
    }

    #[test]
    fn a_paragraph_rewritten_in_its_place_is_still_paired() {
        let before = manuscript(vec![section("S1", "引言", &format!("{P1}\n\n{P2}\n\n{P3}"))]);
        let (_, kept) = translated(&before);
        // Nothing alike is left of the middle one, but it stands where it stood.
        let after = manuscript(vec![section("S1", "引言", &format!("{P1}\n\n样本取自北方的三块试验田。\n\n{P3}"))]);
        let pairs = align(&kept, &units(&after));
        assert_eq!(found(&kept, &pairs)[3].as_deref(), Some(P2));
    }

    #[test]
    fn a_paragraph_moved_takes_its_translation_along() {
        let before = manuscript(vec![section("S1", "引言", &format!("{P1}\n\n{P2}")), section("S2", "结果", &format!("{P3}\n\n{P4}"))]);
        let (_, kept) = translated(&before);
        // P4 moved up into the first section, P1 moved down.
        let after = manuscript(vec![section("S1", "引言", &format!("{P4}\n\n{P2}")), section("S2", "结果", &format!("{P3}\n\n{P1}"))]);
        let now = units(&after);
        let pairs = align(&kept, &now);
        let sources = found(&kept, &pairs);
        assert_eq!(sources[2].as_deref(), Some(P4));
        assert_eq!(sources[6].as_deref(), Some(units(&before)[2].sent.as_str()));
        let settled = settle(kept, &now, &pairs, "t");
        assert!(settled.pairs.iter().all(Option::is_some));
        assert!(settled.kept.iter().all(|k| state(Some(k)) == State::Translated));
        // Each stands now in the section it was moved to.
        assert_eq!(settled.kept[settled.pairs[2].unwrap()].section, "S1");
        assert_eq!(settled.kept[settled.pairs[6].unwrap()].section, "S2");
    }

    #[test]
    fn a_paragraph_split_in_two_keeps_its_translation_with_the_larger_half() {
        let long = "我们比较了三种土壤里主根的长度，每种二十株，每隔两天量一次，一共量了三周";
        let before = manuscript(vec![section("S1", "方法", &format!("{P1}\n\n{long}。所有植株都种在同一间温室里。\n\n{P3}"))]);
        let (_, mut kept) = translated(&before);
        kept[3].edited = true;
        let after = manuscript(vec![section("S1", "方法", &format!("{P1}\n\n{long}。\n\n所有植株都种在同一间温室里。\n\n{P3}"))]);
        let now = units(&after);
        let pairs = align(&kept, &now);
        let settled = settle(kept, &now, &pairs, "t");
        // The hand-made translation is with the first half, its source changed; the second is to translate.
        let first = &settled.kept[settled.pairs[3].unwrap()];
        assert!(first.edited && state(Some(first)) == State::Changed);
        assert_eq!(settled.pairs[4], None);
        assert_eq!(state(settled.pairs[5].map(|k| &settled.kept[k])), State::Translated);
        assert_eq!(wanted(&settled.kept, &settled.pairs), vec![4]);
        assert_eq!(settled.set_aside, 0);
    }

    #[test]
    fn two_paragraphs_merged_set_aside_the_other_hand_made_translation() {
        let before = manuscript(vec![section("S1", "结果", &format!("{P2}\n\n{P3}\n\n{P4}"))]);
        let (_, mut kept) = translated(&before);
        kept[2].edited = true;
        kept[3].edited = true;
        let merged = format!("{}{}", P2, P3);
        let after = manuscript(vec![section("S1", "结果", &format!("{merged}\n\n{P4}"))]);
        let now = units(&after);
        let pairs = align(&kept, &now);
        let settled = settle(kept, &now, &pairs, "2026-11-28");
        assert_eq!(state(settled.pairs[2].map(|k| &settled.kept[k])), State::Changed);
        assert_eq!(settled.set_aside, 1);
        let aside: Vec<&Kept> = settled.kept.iter().filter(|k| !k.gone.is_empty()).collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(aside[0].gone, "2026-11-28");
        assert!(aside[0].text.starts_with('<'));
    }

    #[test]
    fn a_paragraph_deleted_drops_a_models_translation_and_sets_aside_a_hand_made_one() {
        let before = manuscript(vec![section("S1", "结果", &format!("{P2}\n\n{P3}\n\n{P4}"))]);
        let (_, mut kept) = translated(&before);
        for (n, one) in kept.iter_mut().enumerate() { one.id = n as i64 + 1 }
        let now = units(&manuscript(vec![section("S1", "结果", &format!("{P2}\n\n{P4}"))]));
        let pairs = align(&kept, &now);
        let settled = settle(kept.clone(), &now, &pairs, "t");
        assert_eq!((settled.dropped.clone(), settled.set_aside), (vec![4], 0));
        assert!(settled.pairs.iter().all(Option::is_some));
        kept[3].edited = true;
        let pairs = align(&kept, &now);
        let settled = settle(kept, &now, &pairs, "t");
        assert_eq!((settled.dropped.len(), settled.set_aside), (0, 1));
        // Put back as it was, the paragraph has its translation again.
        let back = units(&manuscript(vec![section("S1", "结果", &format!("{P2}\n\n{P3}\n\n{P4}"))]));
        let pairs = align(&settled.kept, &back);
        let again = settle(settled.kept, &back, &pairs, "t");
        assert!(again.kept.iter().all(|k| k.gone.is_empty()) && again.pairs.iter().all(Option::is_some));
        assert_eq!(state(Some(&again.kept[again.pairs[3].unwrap()])), State::Edited);
    }

    #[test]
    fn a_paragraph_inserted_is_to_translate() {
        let before = manuscript(vec![section("S1", "结果", &format!("{P2}\n\n{P4}"))]);
        let (_, kept) = translated(&before);
        let now = units(&manuscript(vec![section("S1", "结果", &format!("{P2}\n\n{P3}\n\n{P4}"))]));
        let pairs = align(&kept, &now);
        assert_eq!(pairs.iter().map(Option::is_some).collect::<Vec<_>>(), vec![true, true, true, false, true]);
        let settled = settle(kept, &now, &pairs, "t");
        assert_eq!(wanted(&settled.kept, &settled.pairs), vec![3]);
        assert!(settled.dropped.is_empty() && settled.set_aside == 0);
    }

    #[test]
    fn the_same_paragraph_twice_is_paired_by_place() {
        let before = manuscript(vec![section("S1", "结果", &format!("{P2}\n\n{P3}\n\n{P2}"))]);
        let (_, mut kept) = translated(&before);
        kept[4].text = "the second".into();
        let now = units(&manuscript(vec![section("S1", "结果", &format!("{P3}\n\n{P2}\n\n{P2}"))]));
        let pairs = align(&kept, &now);
        // The second P2 keeps the second translation, and the one moved up front its own.
        assert_eq!(kept[pairs[4].unwrap()].text, "the second");
        assert_eq!(kept[pairs[2].unwrap()].source, P3);
    }

    #[test]
    fn citations_are_numbered_as_the_paragraph_now_has_them() {
        let then = vec!["[@Li2019]".to_string(), "[@Wang2020 p.3]".to_string()];
        let now = vec!["[@Zhang2024]".to_string(), "[@Wang2020 p.4]".to_string(), "[@Li2019]".to_string()];
        // Li2019 is found as written, Wang2020 by its key; Zhang2024 is new.
        assert_eq!(renumbered("A [@1], B [@2].", &then, &now), "A [@3], B [@2].");
        assert_eq!(renumbered("A [@1] [@x]", &then, &then), "A [@1] [@x]");
        assert!(cites_kept("A [@1] B [@2]", "B [@2] A [@1] [@1]"));
        assert!(!cites_kept("A [@1] B [@2]", "A [@1] B"));
        assert!(!cites_kept("A [@1]", "A [@1] [@2]"));
        // Settling a version renumbers a translation kept by hand under a source that took a citation in front.
        let unit = |sent: &str, cites: Vec<String>| Unit { section: "S1".into(), part: Part::Text, block: 0, sent: sent.into(), cites };
        let mut one = made(&unit("甲 [@1]，乙 [@2]。", then.clone()), "A [@1], B [@2].");
        one.edited = true;
        let units = vec![unit("丙 [@1]，乙 [@2]，甲 [@3]。", now.clone())];
        let settled = settle(vec![one], &units, &[Some(0)], "t");
        assert_eq!(settled.kept[0].text, "A [@3], B [@2].");
        assert_eq!(settled.kept[0].cites, now);
    }

    #[test]
    fn what_is_typed_keeps_its_citations_as_placeholders() {
        let cites = vec!["[@Li2019]".to_string(), "[@Li2019; @Wang2020]".to_string(), "[@Li2019]".to_string()];
        let written = "Roots [@Li2019] grow [@Li2019; @Wang2020] slowly [@Li2019] [@Other].";
        assert_eq!(placeheld(written, &cites), "Roots [@1] grow [@2] slowly [@3] [@Other].");
        assert_eq!(as_written("Roots [@1] grow [@2] slowly [@3] [@9].", &cites), "Roots [@Li2019] grow [@Li2019; @Wang2020] slowly [@Li2019] [@9].");
    }

    #[test]
    fn closeness_is_measured_on_letters() {
        assert_eq!(closeness(P2, P2), 1.);
        assert!(closeness(P2, "我们比较了四种土壤里主根的长度，每种三十株。") > 0.6);
        assert!(closeness(P2, P4) < CLOSE);
        assert_eq!(closeness("甲 [@1]", "甲 [@2]"), 1.);
    }

    #[test]
    fn a_version_reads_as_a_manuscript_in_its_language() {
        let body = format!("{P1}\n\n```\ncode\n```\n\n![根毛 [@Li2019]](figures/G1.png)\n\n{P2}");
        let m = manuscript(vec![section("S1", "引言", &body)]);
        let (units, mut kept) = translated(&m);
        kept[2].text = "Roots grow slowly in compacted soil [@1].".into();
        kept[3].text = "Root hairs [@1]".into();
        // The last block is not translated yet: it reads as the primary has it.
        let pairs: Vec<Option<usize>> = (0..units.len()).map(|u| (u != 4).then_some(u)).collect();
        let version = rendered(&m, "en", &kept, &units, &pairs);
        assert_eq!((version.language.as_str(), version.title.as_str(), version.sections[0].name.as_str()), ("en", "<根的生长>", "<引言>"));
        assert_eq!(version.sections[0].body, format!("Roots grow slowly in compacted soil [@Li2019].\n\n```\ncode\n```\n\n![Root hairs [@Li2019]](figures/G1.png)\n\n{P2}"));
        assert!(manuscript::plain_text(&version).contains("Root hairs"));
    }

    #[test]
    fn a_version_is_made_the_primary_only_when_it_is_whole() {
        let m = manuscript(vec![section("S1", "引言", &format!("{P1}\n\n{P2}"))]);
        let (units, mut kept) = translated(&m);
        let mut pairs: Vec<Option<usize>> = (0..units.len()).map(Some).collect();
        kept[2].text = "Roots grow slowly [@1].".into();
        kept[3].text = "We compared".into();
        // One untranslated, one changed: refused, and said how many.
        pairs[3] = None;
        kept[1].anchor = "前言".into();
        assert_eq!(swap(&m, &kept, &units, &pairs), Err(Unready { untranslated: 1, changed: 1, cites: 0 }));
        pairs[3] = Some(3);
        kept[1].anchor = kept[1].source.clone();
        kept[2].text = "Roots grow slowly.".into();
        assert_eq!(swap(&m, &kept, &units, &pairs).unwrap_err().cites, 1);
        kept[2].text = "Roots grow slowly [@1].".into();
        let swapped = swap(&m, &kept, &units, &pairs).unwrap();
        assert_eq!(swapped.title.as_deref(), Some("<根的生长>"));
        assert_eq!(swapped.sections, vec![("S1".to_string(), "<引言>".to_string(), "Roots grow slowly [@Li2019].\n\nWe compared".to_string())]);
        // The old words are a version of the new ones, paired as before and kept as the person's.
        let new = Manuscript { title: "<根的生长>".into(), language: "en".into(), sections: vec![section("S1", "<引言>", &swapped.sections[0].2)], ..m.clone() };
        let now = super::units(&new);
        let pairs = align(&swapped.kept, &now);
        let back = rendered(&new, "zh", &swapped.kept, &now, &pairs);
        assert_eq!(back.title, m.title);
        assert_eq!(back.sections[0].body, m.sections[0].body);
        assert!(swapped.kept.iter().all(|k| k.edited && state(Some(k)) == State::Edited));
    }

    #[test]
    fn a_swap_carries_citations_the_translation_reordered() {
        let m = manuscript(vec![section("S1", "引言", "甲 [@A2020]，乙 [@B2021]。")]);
        let (units, mut kept) = translated(&m);
        kept[2].text = "B [@2], then A [@1].".into();
        let pairs: Vec<Option<usize>> = (0..units.len()).map(Some).collect();
        let swapped = swap(&m, &kept, &units, &pairs).unwrap();
        assert_eq!(swapped.sections[0].2, "B [@B2021], then A [@A2020].");
        let text = swapped.kept.iter().find(|k| k.part == Part::Text).unwrap();
        assert_eq!((text.anchor.as_str(), text.text.as_str()), ("B [@1], then A [@2].", "甲 [@2]，乙 [@1]。"));
    }

    #[test]
    fn a_version_is_kept_in_the_library() {
        let mut store = Store::in_memory(std::path::Path::new("/tmp/wisp-follow-test")).unwrap();
        let m = store.create_manuscript("根", "zh").unwrap();
        let section = m.sections[0].id.clone();
        store.save_section_body(&section, P2).unwrap();
        let m = store.manuscript(&m.id).unwrap().unwrap();
        assert!(store.add_version(&m.id, "zh").is_err() && store.add_version(&m.id, "xx").is_err());
        store.add_version(&m.id, "en").unwrap();
        store.add_version(&m.id, "en").unwrap();
        assert_eq!(store.version_languages().unwrap().get(&m.id), Some(&vec!["en".to_string()]));
        let units = units(&m);
        let mut kept: Vec<Kept> = units.iter().map(|unit| made(unit, &format!("<{}>", unit.sent))).collect();
        store.save_version(&m.id, "en", &mut kept, &[]).unwrap();
        assert!(kept.iter().all(|k| k.id > 0));
        kept[0].text = "Roots".into();
        kept[0].edited = true;
        store.save_version_block(&m.id, "en", &mut kept[0]).unwrap();
        let read = store.version_blocks(&m.id, "en").unwrap();
        assert_eq!(read.len(), kept.len());
        assert!(read[0].edited && read[0].text == "Roots");
        // Removed and added again, a version has what it kept.
        store.remove_version(&m.id, "en").unwrap();
        assert!(store.version_languages().unwrap().is_empty());
        store.add_version(&m.id, "en").unwrap();
        assert_eq!(store.version_blocks(&m.id, "en").unwrap().len(), kept.len());
        // Made the primary: the manuscript is in English, its Chinese a version of it, and the old text a snapshot.
        let pairs = align(&read, &units);
        let mut swapped = swap(&m, &read, &units, &pairs).unwrap();
        store.swap_primary(&m.id, "en", &mut swapped).unwrap();
        let now = store.manuscript(&m.id).unwrap().unwrap();
        assert_eq!((now.language.as_str(), now.title.as_str()), ("en", "Roots"));
        assert_eq!(store.version_languages().unwrap().get(&m.id), Some(&vec!["zh".to_string()]));
        assert!(store.version_blocks(&m.id, "en").unwrap().is_empty());
        assert_eq!(store.version_blocks(&m.id, "zh").unwrap().len(), kept.len());
        assert_eq!(store.snapshots(&section).unwrap()[0].body, P2);
    }
}
