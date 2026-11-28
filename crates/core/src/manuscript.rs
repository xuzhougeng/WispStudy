//! 写作's manuscripts: any number per library, each with a title, the
//! language it is written in and an ordered list of sections. A section has
//! the person's own name and a `kind` (a key of `structure::SECTIONS`) that
//! decides which part of the library's papers its candidate sentences come
//! from. A section's text is kept as it is typed; snapshots of it are kept
//! on a pause or when the person leaves it, so an earlier version can be
//! looked at and restored. Export turns a manuscript into Markdown, plain
//! text or a Word file, its section names becoming headings.
//!
//! A manuscript can also start from a file (`from_word`, `from_markdown`,
//! `Store::import_manuscript`): its headings become the sections, and the
//! comments a Word file came with become the manuscript's (`comment.rs`).
//! The Word file itself is kept beside the library (`manuscripts/`), and an
//! export to Word is that file written again (`docx::rewrite`): with its
//! formatting, tables and pictures, the text as it now stands, and the
//! comments made here.

use crate::cite;
use crate::comment::{self, Comment};
use crate::docx;
use crate::headings;
use crate::markdown;
use crate::model::new_id;
use crate::store::Store;
use crate::structure::{self, SECTIONS};
use crate::time::now_iso;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// The tables, made with the library's own (`store::SCHEMA`). An older build
/// does not know them and reads the rest of the database as before.
pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS manuscripts (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    -- What it is written in: a short code of `LANGUAGES`.
    language TEXT NOT NULL DEFAULT 'en',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    -- Set when the person removed it: it is kept, only not shown.
    removed_at TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS manuscript_sections (
    id TEXT PRIMARY KEY NOT NULL,
    manuscript_id TEXT NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    name TEXT NOT NULL,
    -- A key of `structure::SECTIONS`: where its candidates come from.
    kind TEXT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_manuscript_sections ON manuscript_sections(manuscript_id, position);
-- Earlier versions of a section's text, newest last (`KEEP_SNAPSHOTS` at most).
CREATE TABLE IF NOT EXISTS section_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    section_id TEXT NOT NULL REFERENCES manuscript_sections(id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    -- pause, leave or restore: what made it be kept.
    why TEXT NOT NULL DEFAULT '',
    taken_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_section_snapshots ON section_snapshots(section_id, id);
-- The file a manuscript was started from.
CREATE TABLE IF NOT EXISTS manuscript_sources (
    manuscript_id TEXT PRIMARY KEY NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE,
    -- docx or markdown.
    kind TEXT NOT NULL,
    -- The file's name as it was taken in.
    name TEXT NOT NULL,
    -- Where the copy of a Word file is, relative to the library folder; empty when none is kept.
    path TEXT NOT NULL DEFAULT '',
    -- The manuscript's title is a paragraph of the file.
    titled INTEGER NOT NULL DEFAULT 0,
    imported_at TEXT NOT NULL
);
"#;

/// The folder of the library that holds the Word files manuscripts were started from.
pub const FOLDER: &str = "manuscripts";

/// The languages a manuscript can be written in: (code, name).
pub const LANGUAGES: [(&str, &str); 6] = [("en", "英文"), ("zh", "中文"), ("de", "德文"), ("fr", "法文"), ("es", "西班牙文"), ("ja", "日文")];

/// A new manuscript's language.
pub const DEFAULT_LANGUAGE: &str = "en";

/// A new manuscript's title until the person gives it one.
pub const UNTITLED: &str = "未命名稿件";

/// The title of the manuscript made of the drafts of builds before manuscripts.
pub const FROM_DRAFTS: &str = "早先的草稿";

/// How many snapshots a section keeps; the oldest go first.
pub const KEEP_SNAPSHOTS: usize = 40;

/// How long typing stops before the text is kept as a snapshot, in seconds.
pub const PAUSE_SECS: u64 = 30;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manuscript {
    pub id: String,
    pub title: String,
    /// A code of `LANGUAGES`.
    pub language: String,
    /// The key of the style its citations are written in (`csl::style`); empty, it follows the one the settings
    /// keep for manuscripts that chose none.
    pub cite_style: String,
    /// What every translation and new version of its words is to follow (American spelling, a journal's habits),
    /// as the person wrote it; the library's 术语表 is apart from it (`glossary`).
    pub requirement: String,
    pub created_at: String,
    pub updated_at: String,
    /// In the order they are written.
    pub sections: Vec<Section>,
}

/// How long a text is, as a writer counts it: a Chinese, Japanese or Korean letter is one, and so is every word
/// of a language that spaces them. The second is whether letters were counted, which is what to call the number
/// (字, else 词). A picture's line is its caption: the address of its file is not something written. It is the
/// one count of a manuscript's text: the outline's, the card's that asks before a section is written anew, and
/// a conversation's line about what was written.
pub fn text_length(text: &str) -> (usize, bool) {
    let count = |text: &str| {
        let letters = text.chars().filter(|c| crate::structure::is_cjk(*c)).count();
        let words = text.split(|c: char| c.is_whitespace() || crate::structure::is_cjk(c)).filter(|word| word.chars().any(char::is_alphanumeric)).count();
        (letters + words, letters > 0)
    };
    if !text.contains("![") { return count(text) }
    let said: Vec<&str> = text.lines().map(|line| crate::markdown::picture_line(line).map_or(line, |(caption, _)| &line[caption])).collect();
    count(&said.join("\n"))
}

/// That length said: 「1200 字」 for a text with Chinese in it, 「300 词」 for one without.
pub fn length_said(text: &str) -> String {
    match text_length(text) {
        (n, true) => format!("{n} 字"),
        (n, false) => format!("{n} 词"),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub id: String,
    pub name: String,
    /// A key of `structure::SECTIONS`.
    pub kind: String,
    pub body: String,
    pub updated_at: String,
    /// Text that stood under no heading in the file the manuscript came from: its name is the manuscript's own and is not written back.
    pub bare: bool,
    /// How deep in the outline it stands, as a heading's level does in Word: 1 is a section, 2 a subsection of
    /// the nearest section before it that is less deep (a method of 方法, a result of 结果), up to `MOST_LEVELS`.
    /// The outline is the sections in their order with their levels; what is under a section is the sections
    /// after it that are deeper (`under`), and a section moves, rises and sinks with them.
    pub level: u8,
}

impl Default for Section {
    fn default() -> Self {
        Section { id: String::new(), name: String::new(), kind: String::new(), body: String::new(), updated_at: String::new(), bare: false, level: 1 }
    }
}

/// How deep an outline goes: sections, their subsections, and the subsections of those.
pub const MOST_LEVELS: u8 = 3;

/// The levels of an outline made such as one can be: the first at the top, none deeper than one under the one
/// before it, none deeper than `MOST_LEVELS`. What a file, a model or a removed section left out of order.
pub fn fit_levels(levels: &mut [u8]) {
    let mut before = 0;
    for level in levels.iter_mut() {
        *level = (*level).clamp(1, MOST_LEVELS).min(before + 1);
        before = *level;
    }
}

/// The section at `at` with what is under it: it and the sections after it that are deeper, as places in `sections`.
pub fn under(sections: &[Section], at: usize) -> Range<usize> {
    let level = sections[at].level;
    let end = sections[at + 1..].iter().position(|section| section.level <= level).map_or(sections.len(), |n| at + 1 + n);
    at..end
}

/// The section the one at `at` is a subsection of: the nearest before it that is less deep.
pub fn parent(sections: &[Section], at: usize) -> Option<usize> {
    let level = sections[at].level;
    sections[..at].iter().rposition(|section| section.level < level)
}

/// The section beside the one at `at` under the same section and as deep, before it or after it: what it changes places with.
pub fn sibling(sections: &[Section], at: usize, after: bool) -> Option<usize> {
    let level = sections[at].level;
    if after {
        let next = under(sections, at).end;
        sections.get(next).filter(|section| section.level == level).map(|_| next)
    } else {
        let before = sections[..at].iter().rposition(|section| section.level <= level)?;
        (sections[before].level == level).then_some(before)
    }
}

/// Each section's number in the outline: 3, then 3.1 and 3.2 for the subsections of the third section.
pub fn numbers(sections: &[Section]) -> Vec<String> {
    let mut counts = [0usize; MOST_LEVELS as usize];
    sections
        .iter()
        .map(|section| {
            let depth = section.level.clamp(1, MOST_LEVELS) as usize;
            counts[depth - 1] += 1;
            counts[depth..].iter_mut().for_each(|count| *count = 0);
            // A level the outline skipped counts as the first of it, as Word numbers headings.
            counts[..depth].iter().map(|count| (*count).max(1).to_string()).collect::<Vec<_>>().join(".")
        })
        .collect()
}

/// An earlier version of a section's text.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: i64,
    pub section_id: String,
    pub body: String,
    /// pause, leave or restore.
    pub why: String,
    pub taken_at: String,
}

/// A section a manuscript is to have (`Store::arrange_sections`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arrange {
    /// The section it is now; empty for one to be made.
    pub id: String,
    pub name: String,
    /// A key of `structure::SECTIONS`.
    pub kind: String,
    /// How deep in the outline (`Section::level`).
    pub level: u8,
}

/// What arranging a manuscript's sections came to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arranged {
    /// The ids of the sections made.
    pub added: Vec<String>,
    /// The names of the unwritten sections that went.
    pub removed: Vec<String>,
    /// The names of the sections that were not asked for and stay, because something was written in them.
    pub kept: Vec<String>,
}

/// Where `Store::split_section` cuts a section, and what the paragraph at the cut becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    /// It opens a new section, as deep as the one cut, under no name of the file's.
    Paragraph,
    /// It names a new section as deep as the one cut.
    Heading,
    /// It names a subsection of the one cut.
    Subheading,
    /// It names a new section at this level of the outline (1 a section, 2 a subsection, 3 a part of that), no deeper
    /// than a subsection of the one cut: a heading typed with as many `#` (`finished_headings`).
    Level(u8),
}

/// What `Store::unsplit` puts back after headings typed in a section were made sections: the section cut as it
/// was and where the caret was in its text, the sections made, and the level every section of the manuscript had
/// (a heading less deep than the one cut takes the subsections after it under itself).
#[derive(Clone, Debug, PartialEq)]
pub struct Unsplit {
    pub was: Section,
    pub caret: usize,
    pub made: Vec<String>,
    pub levels: Vec<(String, u8)>,
}

/// Headings typed in a section made sections (`Store::split_at_headings`): the section the caret goes on in and
/// its byte in that section's text, and what takes it back.
#[derive(Clone, Debug, PartialEq)]
pub struct Typed {
    pub section: Section,
    pub caret: usize,
    pub back: Unsplit,
}

/// The headings a change to a section's text finished: lines of one to `MOST_LEVELS` `#` and a name whose line
/// break came with what the change put in (`put`, its bytes in `text`), as Enter after `## Methods` does or a
/// paste with such lines. A heading still being typed, one written earlier, one in a quote or in code and one
/// too long to name a section stay lines of the text. Each line's bytes and its level, in order.
pub fn finished_headings(text: &str, put: Range<usize>) -> Vec<(Range<usize>, u8)> {
    markdown::lines(text)
        .into_iter()
        .filter(|line| (1..=MOST_LEVELS).contains(&line.heading) && line.quote == 0 && !line.code)
        .filter(|line| put.contains(&line.range.end) && text[line.range.end..].starts_with('\n'))
        .filter(|line| heading_name(&text[line.range.clone()]).is_ok())
        .map(|line| (line.range, line.heading))
        .collect()
}

/// The name a paragraph set as a heading gives a section: its words, not its Markdown (`**Methods**` or
/// `## Methods` names it Methods), and short enough for a heading.
fn heading_name(paragraph: &str) -> Result<String> {
    // Read before it is trimmed: `## ` alone is an empty heading, not the words `##`.
    let name = markdown::plain(paragraph.trim_start()).trim().to_string();
    if name.is_empty() { bail!("光标所在的段落是空的。") }
    if structure::width(&name) > headings::LONGEST * 2 { bail!("这一段太长，不能作为标题。") }
    Ok(name)
}

impl Manuscript {
    pub fn section(&self, id: &str) -> Option<&Section> {
        self.sections.iter().find(|section| section.id == id)
    }

    /// The title, or what stands for it while there is none.
    pub fn shown_title(&self) -> &str {
        if self.title.trim().is_empty() { UNTITLED } else { self.title.trim() }
    }
}

/// The name a section of this kind starts with in a manuscript in this language.
pub fn default_name(kind: &str, language: &str) -> String {
    if language == "zh" { return crate::model::label(&SECTIONS, kind).to_string() }
    match kind {
        "abstract" => "Abstract",
        "introduction" => "Introduction",
        "methods" => "Methods",
        "results" => "Results",
        "discussion" => "Discussion",
        _ => "Section",
    }
    .to_string()
}

/// The sections a new manuscript starts with: IMRaD with its abstract.
pub fn imrad(language: &str) -> Vec<(String, &'static str)> {
    SECTIONS.iter().take(5).map(|(kind, _)| (default_name(kind, language), *kind)).collect()
}

pub fn is_kind(kind: &str) -> bool {
    SECTIONS.iter().any(|(key, _)| *key == kind)
}

pub fn is_language(code: &str) -> bool {
    LANGUAGES.iter().any(|(key, _)| *key == code)
}

/// Every place `query` stands in `text`, as byte ranges in order and apart, letters compared without their case.
/// What the search over a manuscript lights and goes to, section by section.
pub fn occurrences(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let wanted: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if query.trim().is_empty() { return vec![] }
    let letters: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = vec![];
    let mut at = 0;
    'start: while at < letters.len() {
        // Compared letter by letter: a letter whose lower case is longer (İ) is matched as that whole.
        let (mut i, mut w) = (at, 0);
        while w < wanted.len() {
            let Some((_, c)) = letters.get(i) else { break 'start };
            for lower in c.to_lowercase() {
                if wanted.get(w) != Some(&lower) { at += 1; continue 'start }
                w += 1;
            }
            i += 1;
        }
        let end = letters.get(i).map_or(text.len(), |(byte, _)| *byte);
        found.push(letters[at].0..end);
        at = i;
    }
    found
}

/// Whether a section's text is worth a snapshot, given the newest one kept:
/// only when it changed, and an empty section with no history has nothing to keep.
pub fn worth_keeping(latest: Option<&str>, body: &str) -> bool {
    match latest {
        Some(latest) => latest.trim_end() != body.trim_end(),
        None => !body.trim().is_empty(),
    }
}

// ---- export

/// Whether something is written in the section at `at` or under it: a section of its own subsections alone
/// (方法 with a method in each) is written with its heading.
pub fn written_under(sections: &[Section], at: usize) -> bool {
    sections[under(sections, at)].iter().any(|section| !section.body.trim().is_empty())
}

/// The sections an export writes, with their names trimmed and their texts: (level, name, text).
fn sections_with_text(manuscript: &Manuscript) -> impl Iterator<Item = (u8, &str, &str)> {
    let sections = &manuscript.sections;
    sections.iter().enumerate().filter(|(at, _)| written_under(sections, *at)).map(|(_, section)| (section.level, section.name.trim(), section.body.trim()))
}

/// The paragraphs of a text: each line that is not blank.
pub fn paragraphs(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|line| !line.is_empty())
}

/// A picture a section's text has on a line of its own (`markdown::picture_line`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PictureLine {
    /// The line, in bytes of the text.
    pub line: Range<usize>,
    /// Its caption as written, Markdown and citations included; empty when it has none.
    pub caption: String,
    pub address: String,
    /// Where the address is, in bytes of the text.
    pub place: Range<usize>,
}

/// The pictures of a section's text, in order. Fenced code is not looked into.
pub fn picture_lines(body: &str) -> Vec<PictureLine> {
    markdown::lines(body)
        .into_iter()
        .filter_map(|line| {
            let address = line.picture.clone()?;
            let (caption, _) = markdown::picture_line(&body[line.range.clone()])?;
            Some(PictureLine { caption: body[line.range.start + caption.start..line.range.start + caption.end].to_string(), address: body[address.clone()].to_string(), place: address, line: line.range })
        })
        .collect()
}

/// Whether an address names a picture of the figure library: a file right
/// under `figures/` in the library folder, and nothing that leads elsewhere.
pub fn library_picture(address: &str) -> bool {
    address.strip_prefix(&format!("{}/", crate::figure::FOLDER)).is_some_and(|file| !file.is_empty() && !file.starts_with('.') && !file.contains(['/', '\\', ':']))
}

/// The pictures of the figure library a manuscript shows, each once, in the order they stand.
pub fn pictures_used(manuscript: &Manuscript) -> Vec<String> {
    let mut used: Vec<String> = vec![];
    for picture in manuscript.sections.iter().flat_map(|section| picture_lines(&section.body)) {
        if library_picture(&picture.address) && !used.contains(&picture.address) { used.push(picture.address) }
    }
    used
}

/// The line that puts a picture into a section's text: its caption, on one
/// line and without what would end it early, and its address.
pub fn picture_markdown(caption: &str, address: &str) -> String {
    let caption = caption.split_whitespace().collect::<Vec<_>>().join(" ").replace("](", "] (");
    format!("![{caption}]({address})")
}

/// Where a picture's line goes into a text being written, and what is
/// written there: after the paragraph the caret is in, a blank line apart
/// from what stands before and after it. The byte the words go in at, the
/// words, and how far into them the picture's line begins.
pub fn place_picture(text: &str, caret: usize, line: &str) -> (usize, String, usize) {
    let caret = text.floor_char_boundary(caret.min(text.len()));
    let at = caret + text[caret..].find('\n').unwrap_or(text.len() - caret);
    let (before, after) = (&text[..at], &text[at..]);
    // Nothing there, or a blank line already: none is added.
    let lead = if before.trim().is_empty() || before.ends_with("\n\n") { "" } else if before.ends_with('\n') { "\n" } else { "\n\n" };
    let tail = if after.trim().is_empty() || after.starts_with("\n\n") { "" } else if after.starts_with('\n') { "\n" } else { "\n\n" };
    (at, format!("{lead}{line}{tail}"), lead.len())
}

/// Whether a line is a picture without a caption: it says nothing of its own.
fn bare_picture(body: &str, line: &markdown::Line) -> bool {
    line.picture.is_some() && markdown::picture_line(&body[line.range.clone()]).is_some_and(|(caption, _)| body[line.range.start + caption.start..line.range.start + caption.end].trim().is_empty())
}

/// How many lines of a section a table that starts at line `at` takes (`markdown::table_lines`); 0 when none
/// does. `written` are the lines as written. A line of fenced code, an item of a list, a heading and a quotation
/// open no table.
pub(crate) fn table_rows(lines: &[markdown::Line], written: &[&str], at: usize) -> usize {
    let line = &lines[at];
    if line.code || line.item.is_some() || line.heading > 0 || line.quote > 0 { 0 } else { markdown::table_lines(&written[at..]) }
}

/// A section's text as Markdown to stand under a heading of level
/// `under`: each line a paragraph of its own, as the editor has them, the
/// items of a list, the lines of fenced code and the rows of a table kept
/// together, and a heading written in the section set that many levels deeper.
pub fn section_markdown(body: &str, under: u8) -> String {
    let mut blocks: Vec<String> = vec![];
    // What the block being made is: a list, code, or neither.
    let (mut list, mut code) = (false, false);
    let lines = markdown::lines(body);
    let written: Vec<&str> = lines.iter().map(|line| &body[line.range.clone()]).collect();
    // The lines still to pass over: the rows of a table already taken whole.
    let mut taken = 0;
    for (at, line) in lines.iter().enumerate() {
        if taken > 0 {
            taken -= 1;
            continue;
        }
        let text = written[at];
        // A table is its lines one under the other: a blank line between two rows and it is no table to any reader.
        let rows = table_rows(&lines, &written, at);
        if rows > 0 {
            blocks.push(written[at..at + rows].iter().map(|row| row.trim()).collect::<Vec<_>>().join("\n"));
            (list, code, taken) = (false, false, rows - 1);
            continue;
        }
        if line.code {
            match blocks.last_mut().filter(|_| code) {
                Some(block) => {
                    block.push('\n');
                    block.push_str(text);
                }
                None => blocks.push(text.to_string()),
            }
            (list, code) = (false, true);
            continue;
        }
        code = false;
        if text.trim().is_empty() {
            list = false;
            continue;
        }
        if line.item.is_some() {
            match blocks.last_mut().filter(|_| list) {
                Some(block) => {
                    block.push('\n');
                    block.push_str(text.trim_end());
                }
                None => blocks.push(text.trim_end().to_string()),
            }
            list = true;
            continue;
        }
        list = false;
        let text = text.trim();
        if line.heading > 0 && line.quote == 0 {
            let level = (line.heading + under).min(6) as usize;
            blocks.push(format!("{} {}", "#".repeat(level), text.trim_start_matches('#').trim_start()));
        } else {
            blocks.push(text.to_string());
        }
    }
    blocks.join("\n\n")
}

/// The manuscript as Markdown: the title, then each section with text under its name.
pub fn markdown(manuscript: &Manuscript) -> String {
    let mut out = format!("# {}\n", manuscript.shown_title());
    for (level, name, body) in sections_with_text(manuscript) {
        // The title is the first level: a section is the second, a subsection the third.
        let depth = level + 1;
        out.push_str(&format!("\n{} {}\n", "#".repeat(depth as usize), if name.is_empty() { "Section" } else { name }));
        if !body.is_empty() { out.push_str(&format!("\n{}\n", section_markdown(body, depth))) }
    }
    out
}

/// The manuscript as plain text, for the clipboard: headings on lines of their own, without the Markdown.
pub fn plain_text(manuscript: &Manuscript) -> String {
    let mut out = format!("{}\n", manuscript.shown_title());
    for (_, name, body) in sections_with_text(manuscript) {
        out.push_str(&format!("\n{name}\n\n"));
        // A picture is its caption here; one without a caption says nothing.
        let said: Vec<String> = markdown::lines(body).iter().filter(|line| !bare_picture(body, line)).map(|line| markdown::plain_line(body, line).trim().to_string()).filter(|line| !line.is_empty()).collect();
        out.push_str(&said.join("\n\n"));
        out.push('\n');
    }
    out
}

/// A line of a section as a paragraph of a Word file: a heading written in
/// the section as a heading as many levels under the section's (of `level`)
/// as it has marks, the words with their bold, italic, strike-through and
/// code. Empty for a line that says nothing (blank, a fence, a rule).
fn word_paragraph(body: &str, line: &markdown::Line, level: u8) -> String {
    let mut runs = String::new();
    for piece in &line.pieces {
        let text = match piece.part {
            markdown::Part::Text => &body[piece.range.clone()],
            markdown::Part::Bullet => "• ",
            markdown::Part::Syntax => continue,
        };
        let marks = &piece.marks;
        let mut look = String::new();
        if marks.code { look.push_str(&format!("<w:rFonts w:ascii=\"{0}\" w:hAnsi=\"{0}\"/>", docx::MONO)) }
        if marks.bold { look.push_str("<w:b/>") }
        if marks.italic { look.push_str("<w:i/>") }
        if marks.strike { look.push_str("<w:strike/>") }
        if marks.link.is_some() { look.push_str("<w:u w:val=\"single\"/>") }
        let look = if look.is_empty() { look } else { format!("<w:rPr>{look}</w:rPr>") };
        runs.push_str(&format!("<w:r>{look}<w:t xml:space=\"preserve\">{}</w:t></w:r>", xml_escape(text)));
    }
    if runs.trim().is_empty() || body[line.range.clone()].trim().is_empty() { return String::new() }
    let style = match (line.heading, line.quote, line.code) {
        (1.., 0, _) => format!("Heading{}", (line.heading + level).min(6)),
        (_, 1.., _) => "Quote".into(),
        (_, _, true) => "Code".into(),
        _ => String::new(),
    };
    let style = if style.is_empty() { String::new() } else { format!("<w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>") };
    format!("<w:p>{style}{runs}</w:p>")
}

/// A text as a Word file sets it, as one of the paragraphs `docx::rewrite` is asked for.
fn set(rich: cite::Rich) -> docx::Wanted {
    docx::Wanted { text: rich.text, looks: rich.looks, links: rich.links.into_iter().map(|(range, to)| docx::Link { range, to }).collect(), ..Default::default() }
}

/// The lines of a table written in a section (`table_rows`) as the table of a Word file: a cell's bold, italics,
/// strike-through and code are its looks (a link is underlined, as in a paragraph), `<br>` starts a new paragraph
/// in it, and with `cites` its citations are written for a reader as a paragraph's are, each leading to its
/// paper's entry in the reference list.
fn word_table(written: &[&str], cites: Option<&Cites>) -> Option<docx::Table> {
    let (markdown::Block::Table { aligns, rows }, _) = markdown::pipe_table(written)? else { return None };
    let cell = |rich: &markdown::Rich| -> Vec<docx::Wanted> {
        let mut paras: Vec<cite::Rich> = vec![];
        for (range, marks) in rich.spans() {
            for (n, words) in rich.text[range].split('\n').enumerate() {
                if n > 0 || paras.is_empty() { paras.push(cite::Rich::default()) }
                let Some(para) = paras.last_mut() else { continue };
                let start = para.text.len();
                match cites {
                    Some(cites) => para.append(&cites.format.rich(words, cites.index)),
                    None => para.push(words),
                }
                let look = docx::Look { bold: marks.bold, italic: marks.italic, strike: marks.strike, mono: marks.code, underline: marks.link.is_some(), ..Default::default() };
                if look != docx::Look::default() && para.text.len() > start { para.looks.push((start..para.text.len(), look)) }
            }
        }
        paras.into_iter().map(|para| set(para.trimmed())).filter(|para| !para.text.is_empty()).collect()
    };
    Some(docx::Table { aligns, rows: rows.iter().map(|row| row.iter().map(cell).collect()).collect() })
}

/// The paragraph among these that says `words`, one in a cell of a table too, and where in it.
fn saying<'a>(paragraphs: &'a mut [docx::Wanted], words: &str) -> Option<(&'a mut docx::Wanted, Range<usize>)> {
    if words.is_empty() { return None }
    for one in paragraphs {
        if one.table.is_none() {
            if let Some(at) = one.text.find(words) { return Some((one, at..at + words.len())) }
            continue;
        }
        let found = one.table.as_mut().and_then(|table| table.paragraphs_mut().find_map(|cell| cell.text.find(words).map(|at| (cell, at))));
        if let Some((cell, at)) = found { return Some((cell, at..at + words.len())) }
    }
    None
}

/// A Word file's paragraphs say what is written in words alone: a line
/// that `markdown::plain` reads otherwise (`* p < 0.05`, `#3`) gets the
/// backslashes that keep its characters as they are.
fn escape_markdown(text: &str) -> String {
    if markdown::plain(text) == text { return text.to_string() }
    let mut out = String::with_capacity(text.len() + 8);
    let lead = text.len() - text.trim_start().len();
    out.push_str(&text[..lead]);
    let rest = &text[lead..];
    // What would open a block: `#`, `>`, a bullet, a number with its dot, a rule.
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let rest = if rest.starts_with(['#', '>', '-', '+', '*', '_', '`', '~']) {
        out.push('\\');
        out.push_str(&rest[..1]);
        &rest[1..]
    } else if digits > 0 && rest[digits..].starts_with(['.', ')']) {
        out.push_str(&rest[..digits]);
        out.push('\\');
        &rest[digits..]
    } else {
        rest
    };
    for c in rest.chars() {
        if matches!(c, '\\' | '*' | '_' | '`' | '~' | '[') { out.push('\\') }
        out.push(c);
    }
    // Never worse than the line as it was.
    if markdown::plain(&out) == text { out } else { text.to_string() }
}

pub(crate) fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Control characters other than tab are not allowed in XML 1.0.
            c if (c as u32) < 0x20 && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// The manuscript as a Word document (.docx): a title, a Heading 1 for
/// each section, a paragraph for each line, a table for the lines of one
/// (`word_table`). Written by hand, with the zip entries stored: Word reads
/// a docx without compression.
pub fn docx(manuscript: &Manuscript) -> Vec<u8> {
    document(manuscript, true)
}

/// `docx`, with its tables or without them: `Store::manuscript_word` leaves
/// them to `docx::rewrite`, which puts each in with its citations and its
/// comments (`wanted`).
fn document(manuscript: &Manuscript, tables: bool) -> Vec<u8> {
    let paragraph = |style: &str, text: &str| {
        let style = if style.is_empty() { String::new() } else { format!("<w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>") };
        format!("<w:p>{style}<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>", xml_escape(text))
    };
    let mut body = paragraph("Title", manuscript.shown_title());
    for (level, name, text) in sections_with_text(manuscript) {
        body.push_str(&paragraph(&format!("Heading{level}"), name));
        let lines = markdown::lines(text);
        let written: Vec<&str> = lines.iter().map(|line| &text[line.range.clone()]).collect();
        let mut at = 0;
        while at < lines.len() {
            let rows = table_rows(&lines, &written, at);
            if let Some(table) = word_table(&written[at..at + rows], None) {
                // Two tables that touch are one table to Word.
                if tables && body.ends_with("</w:tbl>") { body.push_str("<w:p/>") }
                if tables { body.push_str(&table.plain()) }
                at += rows;
                continue;
            }
            // A picture's line is its caption: the picture itself is put over it when the file is written (`wanted`).
            if !bare_picture(text, &lines[at]) { body.push_str(&word_paragraph(text, &lines[at], level)) }
            at += 1;
        }
    }
    // A document does not end in a table.
    if body.ends_with("</w:tbl>") { body.push_str("<w:p/>") }
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>"
    );
    let lang = match manuscript.language.as_str() {
        "zh" => "zh-CN",
        "en" | "" => "en-US",
        other => other,
    };
    let styles = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val=\"24\"/><w:lang w:val=\"{lang}\" w:eastAsia=\"zh-CN\"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"160\" w:line=\"360\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Title\"><w:name w:val=\"Title\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:spacing w:after=\"320\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"40\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before=\"360\" w:after=\"120\"/><w:outlineLvl w:val=\"0\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"30\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before=\"240\" w:after=\"120\"/><w:outlineLvl w:val=\"1\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"27\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading3\"><w:name w:val=\"heading 3\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before=\"200\" w:after=\"80\"/><w:outlineLvl w:val=\"2\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"25\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading4\"><w:name w:val=\"heading 4\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before=\"160\" w:after=\"80\"/><w:outlineLvl w:val=\"3\"/></w:pPr><w:rPr><w:b/><w:i/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading5\"><w:name w:val=\"heading 5\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before=\"120\" w:after=\"60\"/><w:outlineLvl w:val=\"4\"/></w:pPr><w:rPr><w:i/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading6\"><w:name w:val=\"heading 6\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before=\"120\" w:after=\"60\"/><w:outlineLvl w:val=\"5\"/></w:pPr><w:rPr><w:i/><w:color w:val=\"595959\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Quote\"><w:name w:val=\"Quote\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/><w:pPr><w:ind w:left=\"720\"/></w:pPr><w:rPr><w:i/><w:color w:val=\"595959\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Code\"><w:name w:val=\"Code\"/><w:basedOn w:val=\"Normal\"/><w:qFormat/><w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr><w:rPr><w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\"/><w:sz w:val=\"20\"/></w:rPr></w:style>\
</w:styles>"
    );
    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/><Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/></Types>";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>";
    let document_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>";
    stored_zip(&[("[Content_Types].xml", content_types.as_bytes()), ("_rels/.rels", rels.as_bytes()), ("word/document.xml", document.as_bytes()), ("word/_rels/document.xml.rels", document_rels.as_bytes()), ("word/styles.xml", styles.as_bytes())])
}

pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 { crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 } }
    }
    !crc
}

/// A zip archive of these files, stored without compression.
pub(crate) fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let (mut out, mut central) = (Vec::new(), Vec::new());
    let u16le = |v: &mut Vec<u8>, n: u16| v.extend_from_slice(&n.to_le_bytes());
    let u32le = |v: &mut Vec<u8>, n: u32| v.extend_from_slice(&n.to_le_bytes());
    for (name, data) in files {
        let (offset, crc, size) = (out.len() as u32, crc32(data), data.len() as u32);
        // Local header: version 2.0, UTF-8 names (bit 11), stored, 1980-01-01.
        u32le(&mut out, 0x0403_4b50);
        for n in [20, 0x0800, 0, 0, 0x21] { u16le(&mut out, n) }
        for n in [crc, size, size] { u32le(&mut out, n) }
        u16le(&mut out, name.len() as u16);
        u16le(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        u32le(&mut central, 0x0201_4b50);
        for n in [20, 20, 0x0800, 0, 0, 0x21] { u16le(&mut central, n) }
        for n in [crc, size, size] { u32le(&mut central, n) }
        for n in [name.len() as u16, 0, 0, 0, 0] { u16le(&mut central, n) }
        u32le(&mut central, 0);
        u32le(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let (start, length) = (out.len() as u32, central.len() as u32);
    out.extend_from_slice(&central);
    u32le(&mut out, 0x0605_4b50);
    for n in [0, 0, files.len() as u16, files.len() as u16] { u16le(&mut out, n) }
    u32le(&mut out, length);
    u32le(&mut out, start);
    u16le(&mut out, 0);
    out
}

// ---- starting from a file

/// What a file gives a manuscript.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub title: String,
    /// The title is a paragraph of the file, not the file's name.
    pub titled: bool,
    /// A code of `LANGUAGES`.
    pub language: String,
    pub sections: Vec<DraftSection>,
    pub comments: Vec<DraftComment>,
    /// What a Word file holds besides: kept in the file, not in the manuscript.
    pub tables: usize,
    pub pictures: usize,
    /// The Word file had tracked changes: it was read as if they were accepted.
    pub tracked: bool,
    /// A Word file without heading styles: the paragraphs that look like headings, for the person to confirm (`from_word_with`).
    pub headings: Vec<headings::Guess>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DraftSection {
    pub name: String,
    /// A key of `structure::SECTIONS`.
    pub kind: String,
    /// The text before the first heading: the name is not a heading of the file.
    pub bare: bool,
    pub paragraphs: Vec<String>,
    /// How deep in the outline (`Section::level`): the file's highest headings are 1.
    pub level: u8,
}

impl Default for DraftSection {
    fn default() -> Self {
        DraftSection { name: String::new(), kind: String::new(), bare: false, paragraphs: vec![], level: 1 }
    }
}

/// A comment the file came with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DraftComment {
    /// Which of `Draft::sections` it is in.
    pub section: usize,
    pub quote: String,
    pub author: String,
    pub date: String,
    pub body: String,
}

/// A paragraph of a file, as far as the outline goes.
struct Line {
    /// What it says; for a heading, its words alone.
    text: String,
    /// As it reads when it stays in a section's text (a Markdown heading keeps its marks there).
    kept: String,
    level: Option<u8>,
    title: bool,
    /// Lines of a Markdown file kept one under the other (`kept_together`): never the file's title.
    block: bool,
}

/// A title is short and is not a sentence.
const TITLE_WIDTH: usize = 150;

/// A heading told by its name alone (in a file without heading styles) is no longer than this.
const HEADING_WIDTH: usize = 60;

fn manuscript_language(text: &str) -> String {
    let code = structure::language(&[text.to_string()]);
    if is_language(code) { return code.to_string() }
    // Too short for the rules of a paper's full text: a text mostly in Chinese characters is Chinese.
    let (cjk, letters) = (text.chars().filter(|c| structure::is_cjk(*c)).count(), text.chars().filter(|c| c.is_alphabetic()).count());
    if code.is_empty() && cjk * 2 > letters { "zh".to_string() } else { DEFAULT_LANGUAGE.to_string() }
}

/// The paragraph that is the file's title: the one styled as one, else a short first line that is not a sentence.
fn title_of(lines: &[Line]) -> Option<usize> {
    let styled = lines.iter().position(|line| line.title);
    if styled.is_none() && lines.len() > 1 && lines[0].level.is_none() && !lines[0].block && structure::width(&lines[0].text) <= TITLE_WIDTH && !lines[0].text.ends_with(['。', '.', '；', ';', '：', ':', '，', ',']) { return Some(0) }
    styled
}

/// The manuscript a file's paragraphs make, and the section each of them
/// went to (none for the title). The file's highest headings are its
/// sections, the headings one and two levels under them subsections
/// (`MOST_LEVELS`), each of the kind its name says or else of the section
/// it is under; a heading deeper still stays in the text, a Markdown one
/// with as many marks as it is levels under its section. With `by_name`,
/// a file without headings is cut at the names every paper has.
fn outline(mut lines: Vec<Line>, file_name: &str, by_name: bool) -> (Draft, Vec<Option<usize>>) {
    // A file without heading styles: the headings every paper has are told by their names.
    if by_name && lines.iter().all(|line| line.level.is_none()) {
        for line in &mut lines {
            if structure::width(&line.text) <= HEADING_WIDTH && structure::canonical(&line.text).is_some() { line.level = Some(1) }
        }
    }
    let language = manuscript_language(&lines.iter().filter(|line| line.level.is_none()).map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n"));
    let stem = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let stem = stem.rsplit_once('.').map_or(stem, |(stem, _)| stem).trim();
    let title = title_of(&lines);
    let top = lines.iter().enumerate().filter(|(n, _)| Some(*n) != title).filter_map(|(_, line)| line.level).min();
    let mut draft = Draft { title: title.map_or(stem.to_string(), |n| lines[n].text.clone()), titled: title.is_some(), language, ..Default::default() };
    let chinese = draft.language == "zh";
    let mut places = vec![];
    for (n, line) in lines.iter().enumerate() {
        if Some(n) == title {
            places.push(None);
            continue;
        }
        // How far under the file's highest headings this one is: a section's heading is 0, a subsection's 1.
        let down = line.level.zip(top).map(|(level, top)| level.saturating_sub(top));
        if let Some(down) = down.filter(|down| *down < MOST_LEVELS) {
            let mut levels: Vec<u8> = draft.sections.iter().map(|section| section.level).chain([down + 1]).collect();
            fit_levels(&mut levels);
            let level = levels[levels.len() - 1];
            // A subsection whose name says no part is of the part of the section it is under: a method of 方法.
            let over = draft.sections.iter().rev().find(|section| section.level < level).map(|section| section.kind.clone());
            let kind = structure::canonical(&line.text).filter(|kind| is_kind(kind)).map(str::to_string).or(over).unwrap_or_else(|| "other".to_string());
            draft.sections.push(DraftSection { name: line.text.clone(), kind, bare: false, paragraphs: vec![], level });
        } else {
            if draft.sections.is_empty() {
                let name = match (top.is_some(), chinese) {
                    (true, true) => "开头",
                    (true, false) => "Front matter",
                    (false, true) => "正文",
                    (false, false) => "Text",
                };
                draft.sections.push(DraftSection { name: name.to_string(), kind: "other".to_string(), bare: true, ..Default::default() });
            }
            if let Some(section) = draft.sections.last_mut() {
                // A heading of a Markdown file deeper than the outline goes: its marks say how far under the section it is.
                let kept = match (line.level, top) {
                    (Some(level), Some(top)) if line.kept.starts_with('#') => format!("{} {}", "#".repeat(level.saturating_sub(top + section.level - 1).max(1) as usize), line.text),
                    _ => line.kept.clone(),
                };
                section.paragraphs.push(kept);
            }
        }
        places.push(Some(draft.sections.len() - 1));
    }
    if draft.sections.is_empty() { draft.sections.push(DraftSection { name: if chinese { "正文" } else { "Text" }.to_string(), kind: "other".to_string(), bare: true, ..Default::default() }) }
    (draft, places)
}

/// The manuscript a Word file makes: sections at its highest headings, the
/// comments it came with. A file without heading styles is cut at the
/// headings guessed by their look that are likely ones (`Draft::headings`
/// lists them all, for the person to choose with `from_word_with`).
pub fn from_word(read: &docx::Read, file_name: &str) -> Draft {
    from_word_with(read, file_name, None)
}

/// `from_word`, with the paragraphs of a file without heading styles that
/// start a section chosen by the person (by their place in
/// `read.paragraphs`). They stay plain paragraphs in the file: written
/// back, a section's name is that paragraph as it was.
pub fn from_word_with(read: &docx::Read, file_name: &str, chosen: Option<&[usize]>) -> Draft {
    // What a paragraph says is its words: Markdown's characters in it stay characters (`escape_markdown`).
    let mut lines: Vec<Line> = read.paragraphs.iter().map(|para| Line { text: para.text.clone(), kept: escape_markdown(&para.text), level: para.level, title: para.title, block: false }).collect();
    let mut guesses = vec![];
    if lines.iter().all(|line| line.level.is_none()) {
        let title = title_of(&lines);
        let looks: Vec<(&str, bool)> = read.paragraphs.iter().map(|para| (para.text.as_str(), para.bold)).collect();
        guesses = headings::guess(&looks, title);
        let starts: Vec<usize> = match chosen {
            Some(chosen) => chosen.to_vec(),
            None => guesses.iter().filter(|guess| guess.likely).map(|guess| guess.paragraph).collect(),
        };
        for n in starts {
            if Some(n) != title && let Some(line) = lines.get_mut(n) { line.level = Some(1) }
        }
    }
    let (mut draft, places) = outline(lines, file_name, false);
    draft.headings = guesses;
    (draft.tables, draft.pictures, draft.tracked) = (read.tables, read.pictures, read.tracked);
    draft.comments = read.comments.iter().filter(|comment| !comment.body.trim().is_empty()).map(|comment| DraftComment { section: comment.paragraph.and_then(|n| places[n]).unwrap_or(0), quote: comment.quote.clone(), author: comment.author.clone(), date: comment.date.clone(), body: comment.body.clone() }).collect();
    draft
}

/// The lines of a Markdown file from line `at` on that stay one under the other in a section's text, as one
/// paragraph of the draft, and how many they are; none for a line that is a paragraph by itself. They are what
/// `section_markdown` writes together, told by the same reading (`markdown::lines`, `table_rows`): the lines of
/// fenced code as written, the blank ones among them too; the rows of a table; the items of a list that stand
/// next to each other, each as far in as it is nested (what all of them are indented by is not kept).
fn kept_together(lines: &[markdown::Line], written: &[&str], at: usize) -> Option<(String, usize)> {
    let run = |is: fn(&markdown::Line) -> bool| lines[at..].iter().take_while(|line| is(line)).count();
    let code = run(|line| line.code);
    // A fence left open runs to the end of the file: the blank lines there are not the code's.
    if code > 0 { return Some((written[at..at + code].join("\n").trim_end().to_string(), code)) }
    let rows = table_rows(lines, written, at);
    if rows > 0 { return Some((written[at..at + rows].iter().map(|row| row.trim()).collect::<Vec<_>>().join("\n"), rows)) }
    let items = &written[at..at + run(|line| line.item.is_some())];
    let least = items.iter().map(|item| item.len() - item.trim_start_matches([' ', '\t']).len()).min()?;
    Some((items.iter().map(|item| item[least..].trim_end()).collect::<Vec<_>>().join("\n"), items.len()))
}

/// The manuscript a Markdown (or plain text) file makes. Lines of one
/// paragraph become one line, as the manuscript has them; a picture, a
/// quotation's line and a rule keep their own, and the rows of a table,
/// the items of a list and the lines of fenced code stay one under the
/// other (`kept_together`), so the section reads as the file did.
pub fn from_markdown(text: &str, file_name: &str) -> Draft {
    let text = text.replace("\r\n", "\n");
    let mut rest = text.trim_start_matches('\u{feff}');
    // A header of fields before the text: its `title` is the title.
    let mut named = None;
    if let Some(after) = rest.strip_prefix("---\n") && let Some(end) = after.find("\n---") {
        named = after[..end].lines().find_map(|line| line.strip_prefix("title:")).map(|title| title.trim().trim_matches(['"', '\'']).to_string()).filter(|title| !title.is_empty());
        rest = after[end + 4..].split_once('\n').map_or("", |(_, rest)| rest);
    }
    // The file as the editor reads a section: what is code, an item or a table is told there, not by a rule of this reader's own.
    let read = markdown::lines(rest);
    let written: Vec<&str> = read.iter().map(|line| &rest[line.range.clone()]).collect();
    let mut lines: Vec<Line> = vec![];
    let mut joining = false;
    // The lines still to pass over: those of a block already taken whole.
    let mut taken = 0;
    for (at, line) in written.iter().enumerate() {
        if taken > 0 {
            taken -= 1;
            continue;
        }
        if let Some((block, took)) = kept_together(&read, &written, at) {
            lines.push(Line { text: block.clone(), kept: block, level: None, title: false, block: true });
            (joining, taken) = (false, took - 1);
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            joining = false;
            continue;
        }
        let marks = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&marks) && trimmed[marks..].starts_with(' ') {
            lines.push(Line { text: trimmed[marks..].trim().trim_end_matches('#').trim().to_string(), kept: trimmed.to_string(), level: Some(marks as u8), title: false, block: false });
            joining = false;
            continue;
        }
        // What Pandoc is told of a picture after it (`{width=80%}`) is not the manuscript's: the line is the picture's alone.
        let trimmed = match trimmed.rfind("){") {
            Some(at) if trimmed.starts_with("![") && trimmed.ends_with('}') && markdown::picture_line(&trimmed[..=at]).is_some() => &trimmed[..=at],
            _ => trimmed,
        };
        let own = markdown::picture_line(trimmed).is_some() || trimmed.starts_with(['|', '>']) || read[at].rule;
        match lines.last_mut().filter(|_| joining && !own) {
            Some(last) => {
                // Chinese is written without spaces between the lines of a paragraph.
                let apart = !(last.text.ends_with(structure::is_cjk) && trimmed.starts_with(structure::is_cjk));
                if apart { last.text.push(' ') }
                last.text.push_str(trimmed);
                last.kept = last.text.clone();
            }
            None => lines.push(Line { text: trimmed.to_string(), kept: trimmed.to_string(), level: None, title: false, block: false }),
        }
        joining = !own;
    }
    // One `#` heading before all others is the title; the sections are the next level.
    let firsts: Vec<usize> = lines.iter().enumerate().filter(|(_, line)| line.level == Some(1)).map(|(n, _)| n).collect();
    if named.is_none() && firsts.len() == 1 && lines.iter().position(|line| line.level.is_some()) == Some(firsts[0]) {
        lines[firsts[0]].title = true;
        lines[firsts[0]].level = None;
    }
    // With a title from the header, no line is taken for one.
    let guard = named.is_some();
    if guard { lines.insert(0, Line { text: named.unwrap_or_default(), kept: String::new(), level: None, title: true, block: false }) }
    let (mut draft, _) = outline(lines, file_name, true);
    draft.titled = false;
    draft
}

// ---- the pictures a file shows from files of its own

/// Why a picture a file shows did not come along with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Left {
    /// The address is a link: nothing is fetched.
    Link,
    Missing,
    Unread,
    /// Not a picture that is kept here.
    Kind,
    Large,
}

impl Left {
    pub fn said(self) -> &'static str {
        match self {
            Left::Link => "是网上的图片，没有下载",
            Left::Missing => "没有找到这个文件",
            Left::Unread => "文件读不了",
            Left::Kind => "不是 PNG、JPEG 或 WebP 图片",
            Left::Large => "图片太大了",
        }
    }
}

/// A picture a file shows from a file of its own: `![caption](fig1.png)` in a Markdown file, the address read
/// from the folder the file is in.
#[derive(Clone, Debug, PartialEq)]
pub struct Beside {
    /// The address as the file writes it.
    pub address: String,
    /// The caption of the first line that shows it, in plain words.
    pub caption: String,
    /// The file meant, or why there is none to read.
    pub file: Result<PathBuf, Left>,
}

/// The file an address names for a file in `folder`: beside it, or where the address says in full. A link is
/// not followed, and nothing is read.
fn file_beside(address: &str, folder: &Path) -> Result<PathBuf, Left> {
    if address.contains("://") || address.starts_with("data:") { return Err(Left::Link) }
    let written = crate::webpage::percent_decoded(address);
    let path = folder.join(written.strip_prefix("./").unwrap_or(&written));
    if path.is_file() { Ok(path) } else { Err(Left::Missing) }
}

/// The pictures a draft shows, each address once and in the order they stand, with the files they name beside
/// the file the draft was read from. One that already is a figure of the library (`figures/G….png`, the file
/// not beside the draft) is found there by `Store::bring_pictures`.
pub fn pictures_beside(draft: &Draft, folder: &Path) -> Vec<Beside> {
    let mut found: Vec<Beside> = vec![];
    for picture in draft.sections.iter().flat_map(|section| &section.paragraphs).flat_map(|paragraph| picture_lines(paragraph)) {
        if found.iter().any(|known| known.address == picture.address) { continue }
        found.push(Beside { file: file_beside(&picture.address, folder), caption: markdown::plain(&picture.caption).trim().to_string(), address: picture.address });
    }
    found
}

/// A picture of a file, read: what `Store::bring_pictures` keeps.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub address: String,
    pub caption: String,
    /// The name of the file it is.
    pub name: String,
    pub bytes: Result<Vec<u8>, Left>,
}

/// The pictures a draft shows, read from beside its file. A PNG or a JPEG is kept as it is; `anew` writes any
/// other kind as one of those where it can (the window does, a tool does not), and a WebP it leaves is kept so.
pub fn read_pictures(draft: &Draft, folder: &Path, anew: &dyn Fn(&[u8]) -> Option<Vec<u8>>) -> Vec<Picture> {
    pictures_beside(draft, folder)
        .into_iter()
        .map(|beside| {
            let name = beside.file.as_ref().ok().and_then(|path| path.file_name()).map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
            let bytes = beside.file.and_then(|path| std::fs::read(path).map_err(|_| Left::Unread)).and_then(|bytes| match crate::figure::picture_kind(&bytes) {
                Some("png" | "jpg") => Ok(bytes),
                kind => anew(&bytes).or(kind.map(|_| bytes)).ok_or(Left::Kind),
            });
            Picture { address: beside.address, caption: beside.caption, name, bytes: bytes.and_then(|bytes| if bytes.len() > crate::figure::MAX_BYTES { Err(Left::Large) } else { Ok(bytes) }) }
        })
        .collect()
}

/// What became of the pictures a file shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Brought {
    /// Kept as new figures of the library.
    pub added: usize,
    /// The library held them already, to the byte.
    pub known: usize,
    /// Those that stay as the file writes them, and why.
    pub left: Vec<(String, Left)>,
}

impl Brought {
    /// How many of the file's pictures the manuscript shows from the library.
    pub fn shown(&self) -> usize {
        self.added + self.known
    }
}

/// A text with the addresses of its pictures taken out: what two readings of a file have alike when the
/// pictures of one were brought into the library since.
pub fn without_addresses(body: &str) -> String {
    let mut out = body.to_string();
    for picture in picture_lines(body).into_iter().rev() { out.replace_range(picture.place, "") }
    out
}

/// The file a manuscript was started from.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// `docx` or `markdown`.
    pub kind: String,
    pub name: String,
    /// The copy of a Word file, relative to the library folder.
    pub path: String,
    pub titled: bool,
    pub imported_at: String,
}

/// `2026-11-12T08:00:00Z`, as a Word file dates a comment or a change.
fn word_date(stamp: &str) -> String {
    match stamp.get(..19) {
        Some(seconds) if seconds.as_bytes().get(10) == Some(&b'T') => format!("{seconds}Z"),
        _ => now_iso().get(..19).map(|seconds| format!("{seconds}Z")).unwrap_or_default(),
    }
}

/// How a Word file cites: the manuscript's citations written for a reader (`cite::Formatter`), with the library they are found in.
pub struct Cites<'a> {
    pub format: &'a cite::Formatter,
    pub index: &'a cite::Index<'a>,
}

/// The manuscript's paragraphs as a Word file is to say them, each with the
/// comments made here on it. With a `source`, they are the paragraphs of
/// the file the manuscript came from; without, those `docx` writes.
///
/// With `cites` the citations are written for a reader: each one leads to
/// its paper's entry in the reference list, which stands after the last
/// section under a heading of the manuscript's language.
///
/// A picture's line (`picture_lines`) is its caption, with the picture
/// `pictures` reads at its address over it; one without a caption stands
/// under the paragraph before it. A picture that cannot be read leaves its
/// caption alone.
///
/// The lines of a table (`table_rows`) are no paragraphs but one table
/// (`docx::Wanted::table`, `word_table`), which neither file has yet: it
/// is put in after the paragraph before it.
pub fn wanted(manuscript: &Manuscript, source: Option<&Source>, comments: &[Comment], cites: Option<&Cites>, pictures: &dyn Fn(&str) -> Option<docx::Picture>) -> Vec<docx::Wanted> {
    let mut out: Vec<docx::Wanted> = vec![];
    // Where each section's paragraphs are in `out`: its heading, and its lines.
    let mut places: Vec<(&str, Option<usize>, Range<usize>)> = vec![];
    // Pictures without a caption met before any paragraph: they go over the first one.
    let mut waiting: Vec<docx::Picture> = vec![];
    let paragraph = |text: &str, role: docx::Role| docx::Wanted { text: text.to_string(), role, ..Default::default() };
    match source {
        Some(source) => {
            if source.titled && !manuscript.title.trim().is_empty() { out.push(paragraph(manuscript.title.trim(), docx::Role::Title)) }
        }
        None => out.push(paragraph(manuscript.shown_title(), docx::Role::Title)),
    }
    for (n, section) in manuscript.sections.iter().enumerate() {
        // A file made here leaves out the sections with nothing written in them or under them; one written again keeps every heading it had.
        if source.is_none() && !written_under(&manuscript.sections, n) { continue }
        let heading = (source.is_none() || !section.bare).then(|| {
            out.push(docx::Wanted { depth: section.level, ..paragraph(section.name.trim(), docx::Role::Heading) });
            out.len() - 1
        });
        let start = out.len();
        // What a paragraph says, as the editor draws it: a Word file has its own bold and headings.
        let body = section.body.as_str();
        let lines = markdown::lines(body);
        let written: Vec<&str> = lines.iter().map(|line| &body[line.range.clone()]).collect();
        // The lines still to pass over: the rows of a table already taken whole.
        let mut taken = 0;
        for (at, line) in lines.iter().enumerate() {
            if taken > 0 {
                taken -= 1;
                continue;
            }
            let rows = table_rows(&lines, &written, at);
            if let Some(table) = word_table(&written[at..at + rows], cites) {
                out.push(docx::Wanted { table: Some(table), above: std::mem::take(&mut waiting), ..Default::default() });
                taken = rows - 1;
                continue;
            }
            let said = markdown::plain_line(body, line);
            let said = said.trim();
            let picture = line.picture.as_ref().and_then(|address| pictures(&body[address.clone()]));
            if bare_picture(body, line) {
                // No caption to stand over: under the paragraph before it, or over the first one there will be.
                match (picture, out.last_mut()) {
                    (Some(picture), Some(last)) => last.below.push(picture),
                    (Some(picture), None) => waiting.push(picture),
                    (None, _) => {}
                }
                continue;
            }
            if said.is_empty() { continue }
            let mut made = match cites {
                Some(cites) => set(cites.format.rich(said, cites.index)),
                None => paragraph(said, docx::Role::Body),
            };
            made.above.append(&mut waiting);
            made.above.extend(picture);
            out.push(made);
        }
        places.push((section.id.as_str(), heading, start..out.len()));
    }
    for comment in comments.iter().filter(|comment| comment.origin != comment::FILE && comment.state != comment::DONE) {
        let said = comment.said();
        if said.is_empty() { continue }
        let Some((_, heading, lines)) = places.iter().find(|(id, _, _)| *id == comment.section_id) else { continue };
        // Its words, in the paragraph that has them; a comment whose words were written over since goes on the section's heading, with the words it was about.
        // The words are those of the file: their citations written as the paragraph's are.
        let quote = markdown::plain(&comment.quote);
        let quote = cites.map_or(quote.clone(), |cites| cites.format.apply(&quote, cites.index));
        let first = quote.lines().next().unwrap_or("").trim();
        let note = |range: Range<usize>, body: String| docx::Note { range, author: comment.author.clone(), date: word_date(&comment.created_at), body };
        // In a cell of a table as in a paragraph.
        if let Some((para, range)) = saying(&mut out[lines.clone()], first) {
            para.notes.push(note(range, said));
            continue;
        }
        if let Some(n) = heading.filter(|n| !first.is_empty() && out[*n].text.contains(first)) {
            out[n].notes.push(note(0..0, said));
            continue;
        }
        // A section that opens with a table and has no heading in the file: on the table's first words.
        let Some(n) = heading.or(lines.clone().next()) else { continue };
        let Some(para) = (match &mut out[n].table {
            Some(table) => table.paragraphs_mut().next(),
            None => Some(&mut out[n]),
        }) else { continue };
        para.notes.push(note(0..0, if first.is_empty() { said } else { format!("「{first}」\n{said}") }));
    }
    if let Some(cites) = cites.filter(|cites| !cites.format.references().is_empty()) {
        out.push(paragraph(cites.format.heading(), docx::Role::Heading));
        out.extend(cites.format.references().iter().map(|(paper, entry)| docx::Wanted { anchor: Some(paper.clone()), ..set(entry.clone()) }));
    }
    out
}

impl Store {
    /// A manuscript made of a file: its sections, the comments it came with,
    /// and for a Word file (`bytes`) a copy of the file itself, which an
    /// export to Word is written from. All of it or nothing.
    pub fn import_manuscript(&mut self, draft: &Draft, kind: &str, file_name: &str, bytes: Option<&[u8]>) -> Result<Manuscript> {
        if !is_language(&draft.language) { bail!("不认识这种语言：{}", draft.language) }
        if draft.sections.is_empty() || draft.sections.iter().any(|section| section.name.trim().is_empty()) { bail!("稿件需要有名称的章节") }
        let (id, now) = (new_id("M"), now_iso());
        let path = if bytes.is_some() { format!("{FOLDER}/{id}.docx") } else { String::new() };
        let (folder, file) = (self.root().join(FOLDER), self.root().join(path.replace('/', std::path::MAIN_SEPARATOR_STR)));
        let tx = self.db.transaction()?;
        tx.execute("INSERT INTO manuscripts(id, title, language, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)", params![id, draft.title.trim(), draft.language, now])?;
        let mut sections = vec![];
        for (position, section) in draft.sections.iter().enumerate() {
            let section_id = new_id("S");
            tx.execute("INSERT INTO manuscript_sections(id, manuscript_id, position, name, kind, body, updated_at, bare, level) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)", params![section_id, id, position as i64, section.name.trim(), if is_kind(&section.kind) { section.kind.as_str() } else { "other" }, section.paragraphs.join("\n\n"), now, section.bare, section.level])?;
            sections.push(section_id);
        }
        for (n, theirs) in draft.comments.iter().enumerate() {
            let Some(section) = sections.get(theirs.section) else { continue };
            let made = Comment { id: new_id("C"), manuscript_id: id.clone(), section_id: section.clone(), quote: theirs.quote.trim().to_string(), body: theirs.body.trim().to_string(), author: theirs.author.trim().to_string(), origin: comment::FILE.into(), state: comment::OPEN.into(), created_at: if theirs.date.is_empty() { format!("{now}{n:04}") } else { theirs.date.clone() }, ..Default::default() };
            comment::insert_comment(&tx, &made)?;
        }
        tx.execute("INSERT INTO manuscript_sources(manuscript_id, kind, name, path, titled, imported_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![id, kind, file_name, path, draft.titled, now])?;
        if let Some(bytes) = bytes {
            std::fs::create_dir_all(&folder).with_context(|| format!("无法创建稿件文件夹 {}", folder.display()))?;
            std::fs::write(&file, bytes).with_context(|| format!("无法保存 {}", file.display()))?;
        }
        if let Err(err) = tx.commit() {
            if bytes.is_some() { let _ = std::fs::remove_file(&file); }
            return Err(err.into());
        }
        self.manuscript(&id)?.ok_or_else(|| anyhow::anyhow!("新稿件保存后未找到"))
    }

    /// Keep the pictures a file shows (`read_pictures`) as figures of the library, the person's own (`UPLOADED`),
    /// and write the draft's lines to show them from there: `![caption](figures/G….png)`. One the library holds
    /// already, to the byte, is not kept twice. A picture that did not come leaves its line as the file has it.
    /// Done before `import_manuscript`; a figure kept stays one should that fail.
    pub fn bring_pictures(&mut self, draft: &mut Draft, pictures: Vec<Picture>) -> Result<Brought> {
        let mut brought = Brought::default();
        let mut now: Vec<(String, String)> = vec![];
        for picture in pictures {
            let bytes = match picture.bytes {
                Ok(bytes) => bytes,
                // A line the library's own export wrote, read where no picture lies beside it: the figure is here.
                Err(Left::Missing) if library_picture(&picture.address) && self.root().join(picture.address.replace('/', std::path::MAIN_SEPARATOR_STR)).is_file() => continue,
                Err(why) => {
                    brought.left.push((picture.address, why));
                    continue;
                }
            };
            let held = match self.figure_holding(&bytes)? {
                Some(held) => {
                    brought.known += 1;
                    held.local_path
                }
                None => {
                    let mut figure = crate::figure::Figure { origin: crate::figure::UPLOADED.into(), file_name: picture.name, legend: picture.caption, ..Default::default() };
                    self.add_figure(&mut figure, &bytes)?;
                    brought.added += 1;
                    figure.local_path
                }
            };
            now.push((picture.address, held));
        }
        for paragraph in draft.sections.iter_mut().flat_map(|section| &mut section.paragraphs) {
            for line in picture_lines(paragraph).into_iter().rev() {
                if let Some((_, held)) = now.iter().find(|(address, _)| *address == line.address) { paragraph.replace_range(line.place, held) }
            }
        }
        Ok(brought)
    }

    /// The file a manuscript was started from, when it was.
    pub fn manuscript_source(&self, manuscript_id: &str) -> Result<Option<Source>> {
        Ok(self.db.query_row("SELECT kind, name, path, titled, imported_at FROM manuscript_sources WHERE manuscript_id = ?1", [manuscript_id], |r| Ok(Source { kind: r.get(0)?, name: r.get(1)?, path: r.get(2)?, titled: r.get(3)?, imported_at: r.get(4)? })).optional()?)
    }

    /// Where the copy of a manuscript's Word file is.
    pub fn source_file(&self, source: &Source) -> PathBuf {
        self.root().join(source.path.replace('/', std::path::MAIN_SEPARATOR_STR))
    }

    /// The manuscript as a Word file with its comments. One that came from a
    /// Word file is that file written again, with `track` its changes as
    /// revisions by `author`; any other is a plain document made here. The
    /// second value says whether the file it came from was written again.
    ///
    /// Citations are written in `style` (`cite::Formatter`), and the papers
    /// they name are listed after the last section under a heading of the
    /// manuscript's language: the file is for a reader, who has no keys.
    /// Each citation is a field that jumps to its entry in that list.
    ///
    /// The pictures the manuscript shows (`pictures_used`) are put into the
    /// file; one whose file is gone, or is a WebP (which Word does not read),
    /// is named in `Rewritten::left_out` and leaves its caption alone.
    pub fn manuscript_word(&self, manuscript: &Manuscript, comments: &[Comment], track: bool, author: &str, style: cite::Style) -> Result<(docx::Rewritten, bool)> {
        let held: std::collections::HashMap<String, Option<docx::Picture>> = pictures_used(manuscript).into_iter().map(|address| {
            let read = std::fs::read(self.root().join(address.replace('/', std::path::MAIN_SEPARATOR_STR))).ok().and_then(|bytes| docx::Picture::read(bytes).ok());
            (address, read)
        }).collect();
        let pictures = |address: &str| held.get(address).cloned().flatten();
        let left_out: Vec<String> = pictures_used(manuscript).into_iter().filter(|address| pictures(address).is_none()).collect();
        let (papers, trash, kept) = (self.list_papers()?, self.list_trash()?, self.cite_keys()?);
        let index = cite::Index::new(&papers, trash.iter().map(|entry| &entry.paper), &kept);
        let bodies: Vec<&str> = manuscript.sections.iter().map(|section| section.body.as_str()).collect();
        let format = cite::Formatter::new(&bodies, &index, &style, &manuscript.language);
        let cites = Cites { format: &format, index: &index };
        let source = self.manuscript_source(&manuscript.id)?.filter(|source| source.kind == "docx" && !source.path.is_empty());
        if let Some(source) = &source && let Ok(bytes) = std::fs::read(self.source_file(source)) {
            let options = docx::Options { track, author: author.to_string(), date: word_date(&now_iso()) };
            return Ok((docx::Rewritten { left_out, ..docx::rewrite(&bytes, &wanted(manuscript, Some(source), comments, Some(&cites), &pictures), &options)? }, true));
        }
        // The document made here says what `wanted` says, the list included: what is written into it is the fields and the looks.
        let mut said = manuscript.clone();
        for section in &mut said.sections { section.body = format.apply(&section.body, &index) }
        if !format.references().is_empty() {
            let body = format.references().iter().map(|(_, entry)| escape_markdown(&entry.text)).collect::<Vec<_>>().join("\n\n");
            said.sections.push(Section { name: format.heading().to_string(), kind: "other".into(), body, ..Default::default() });
        }
        // Its tables are put in as it is written again: there they get their citations and comments as the paragraphs do.
        Ok((docx::Rewritten { left_out, ..docx::rewrite(&document(&said, false), &wanted(manuscript, None, comments, Some(&cites), &pictures), &docx::Options::default())? }, false))
    }
}

// ---- migration

/// The drafts of builds before manuscripts (one text per part, in the
/// `drafts` table) become the sections of one manuscript, and leave that
/// table. An older build opening the library afterwards finds no drafts,
/// and what it writes there is taken in as another manuscript next time.
pub(crate) fn migrate_drafts(db: &mut Connection) -> Result<()> {
    let drafts: Vec<(String, String)> = {
        let mut stmt = db.prepare("SELECT section, body FROM drafts")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?
    };
    if drafts.is_empty() { return Ok(()) }
    let tx = db.transaction()?;
    if drafts.iter().any(|(_, body)| !body.trim().is_empty()) {
        let body = |kind: &str| drafts.iter().find(|(section, _)| section == kind).map(|(_, body)| body.clone()).unwrap_or_default();
        let mut sections: Vec<(String, String, String)> = imrad(DEFAULT_LANGUAGE).into_iter().map(|(name, kind)| (name, kind.to_string(), body(kind))).collect();
        // Parts the five do not cover (`other`, or a key of a later build) are kept as sections of their own.
        for (section, text) in &drafts {
            if sections.iter().any(|(_, kind, _)| kind == section) || text.trim().is_empty() { continue }
            let kind = if is_kind(section) { section.clone() } else { "other".to_string() };
            sections.push((if section == "other" { "Other".into() } else { section.clone() }, kind, text.clone()));
        }
        let (id, now) = (new_id("M"), now_iso());
        tx.execute("INSERT INTO manuscripts(id, title, language, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)", params![id, FROM_DRAFTS, DEFAULT_LANGUAGE, now])?;
        for (position, (name, kind, body)) in sections.iter().enumerate() {
            tx.execute("INSERT INTO manuscript_sections(id, manuscript_id, position, name, kind, body, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![new_id("S"), id, position as i64, name, kind, body, now])?;
        }
    }
    tx.execute("DELETE FROM drafts", [])?;
    tx.commit()?;
    Ok(())
}

// ---- the store

/// A row of `manuscript_sections`, read from the columns `SECTION_COLUMNS` names in their order.
fn read_section(r: &rusqlite::Row) -> rusqlite::Result<Section> {
    Ok(Section { id: r.get(0)?, name: r.get(1)?, kind: r.get(2)?, body: r.get(3)?, updated_at: r.get(4)?, bare: r.get(5)?, level: r.get(6)? })
}

/// What `read_section` reads.
const SECTION_COLUMNS: &str = "id, name, kind, body, updated_at, bare, level";

/// Put a manuscript's levels in order (`fit_levels`) after a change that can leave a section deeper than the one before it allows.
fn fit_outline(db: &Connection, manuscript_id: &str) -> Result<()> {
    let rows: Vec<(String, u8)> = {
        let mut stmt = db.prepare("SELECT id, level FROM manuscript_sections WHERE manuscript_id = ?1 ORDER BY position, id")?;
        stmt.query_map([manuscript_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    let mut levels: Vec<u8> = rows.iter().map(|(_, level)| *level).collect();
    fit_levels(&mut levels);
    for ((id, was), now) in rows.iter().zip(levels) {
        if *was != now { db.execute("UPDATE manuscript_sections SET level = ?2 WHERE id = ?1", params![id, now])?; }
    }
    Ok(())
}

/// The sections of a manuscript in this order, by their ids.
fn put_in_order(db: &Connection, order: &[&str]) -> Result<()> {
    for (position, id) in order.iter().enumerate() { db.execute("UPDATE manuscript_sections SET position = ?2 WHERE id = ?1", params![id, position as i64])?; }
    Ok(())
}

impl Store {
    /// Create a manuscript from prepared sections and pin the citation keys
    /// in the same transaction. A failure leaves neither a partial manuscript
    /// nor newly reserved keys. `keys` holds (paper_id, citation_key).
    pub fn create_manuscript_from_sections(&mut self, title: &str, language: &str, sections: &[(String, String)], keys: &[(String, String)]) -> Result<Manuscript> {
        if !is_language(language) { bail!("不认识这种语言：{language}") }
        if sections.is_empty() || sections.iter().any(|(name, _)| name.trim().is_empty()) { bail!("稿件需要有名称的章节") }
        let (id, now) = (new_id("M"), now_iso());
        let tx = self.db.transaction()?;
        for (paper_id, key) in keys {
            if key.trim().is_empty() { bail!("引用键不能为空") }
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM papers WHERE id = ?1)", [paper_id], |r| r.get(0))?;
            if !exists { bail!("引用的文献已不存在：{paper_id}") }
            tx.execute("INSERT OR IGNORE INTO cite_keys(key, paper_id, made_at) VALUES (?1, ?2, ?3)", params![key, paper_id, now])?;
            let actual: String = tx.query_row("SELECT paper_id FROM cite_keys WHERE key = ?1", [key], |r| r.get(0))?;
            if &actual != paper_id { bail!("引用键与其他文献冲突：{key}") }
        }
        tx.execute("INSERT INTO manuscripts(id, title, language, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)", params![id, title.trim(), language, now])?;
        for (position, (name, body)) in sections.iter().enumerate() {
            tx.execute("INSERT INTO manuscript_sections(id, manuscript_id, position, name, kind, body, updated_at) VALUES (?1, ?2, ?3, ?4, 'other', ?5, ?6)", params![new_id("S"), id, position as i64, name.trim(), body, now])?;
        }
        tx.commit()?;
        self.manuscript(&id)?.ok_or_else(|| anyhow::anyhow!("新稿件保存后未找到"))
    }

    /// The manuscripts of the library that were not removed, oldest first, with their sections.
    pub fn manuscripts(&self) -> Result<Vec<Manuscript>> {
        let mut stmt = self.db.prepare("SELECT id, title, language, created_at, updated_at, cite_style, requirement FROM manuscripts WHERE removed_at = '' ORDER BY created_at, id")?;
        let mut list = stmt.query_map([], |r| Ok(Manuscript { id: r.get(0)?, title: r.get(1)?, language: r.get(2)?, created_at: r.get(3)?, updated_at: r.get(4)?, cite_style: r.get(5)?, requirement: r.get(6)?, sections: vec![] }))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for manuscript in &mut list { manuscript.sections = self.sections_of(&manuscript.id)? }
        Ok(list)
    }

    pub fn manuscript(&self, id: &str) -> Result<Option<Manuscript>> {
        let found = self.db.query_row("SELECT id, title, language, created_at, updated_at, cite_style, requirement FROM manuscripts WHERE id = ?1 AND removed_at = ''", [id], |r| Ok(Manuscript { id: r.get(0)?, title: r.get(1)?, language: r.get(2)?, created_at: r.get(3)?, updated_at: r.get(4)?, cite_style: r.get(5)?, requirement: r.get(6)?, sections: vec![] })).optional()?;
        let Some(mut manuscript) = found else { return Ok(None) };
        manuscript.sections = self.sections_of(id)?;
        Ok(Some(manuscript))
    }

    fn sections_of(&self, manuscript_id: &str) -> Result<Vec<Section>> {
        let mut stmt = self.db.prepare(&format!("SELECT {SECTION_COLUMNS} FROM manuscript_sections WHERE manuscript_id = ?1 ORDER BY position, id"))?;
        Ok(stmt.query_map([manuscript_id], read_section)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn section_row(&self, id: &str) -> Result<(String, Section)> {
        let found = self.db.query_row(&format!("SELECT {SECTION_COLUMNS}, manuscript_id FROM manuscript_sections WHERE id = ?1"), [id], |r| Ok((r.get(7)?, read_section(r)?))).optional()?;
        found.ok_or_else(|| anyhow::anyhow!("没有这一节：{id}"))
    }

    fn touch_manuscript(&self, id: &str) -> Result<()> {
        self.db.execute("UPDATE manuscripts SET updated_at = ?2 WHERE id = ?1", params![id, now_iso()])?;
        Ok(())
    }

    /// A new manuscript with the IMRaD sections, named in its language.
    pub fn create_manuscript(&mut self, title: &str, language: &str) -> Result<Manuscript> {
        if !is_language(language) { bail!("不认识这种语言：{language}") }
        let (id, now) = (new_id("M"), now_iso());
        let tx = self.db.transaction()?;
        tx.execute("INSERT INTO manuscripts(id, title, language, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)", params![id, title.trim(), language, now])?;
        for (position, (name, kind)) in imrad(language).into_iter().enumerate() {
            tx.execute("INSERT INTO manuscript_sections(id, manuscript_id, position, name, kind, body, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, '', ?6)", params![new_id("S"), id, position as i64, name, kind, now])?;
        }
        tx.commit()?;
        Ok(self.manuscript(&id)?.unwrap_or_default())
    }

    /// A new manuscript that is one piece of text under no heading (`Section::bare`): an article, a note, a post.
    /// What the home page's 稿件 starts, where nothing says the text is a paper with IMRaD parts; headings are
    /// made as it is written (`split_section`), or by an outline.
    pub fn create_piece(&mut self, title: &str, language: &str) -> Result<Manuscript> {
        if !is_language(language) { bail!("不认识这种语言：{language}") }
        let (id, now) = (new_id("M"), now_iso());
        let tx = self.db.transaction()?;
        tx.execute("INSERT INTO manuscripts(id, title, language, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)", params![id, title.trim(), language, now])?;
        tx.execute("INSERT INTO manuscript_sections(id, manuscript_id, position, name, kind, body, updated_at, bare) VALUES (?1, ?2, 0, ?3, 'other', '', ?4, 1)", params![new_id("S"), id, if language == "zh" { "正文" } else { "Text" }, now])?;
        tx.commit()?;
        self.manuscript(&id)?.ok_or_else(|| anyhow::anyhow!("新稿件保存后未找到"))
    }

    pub fn rename_manuscript(&mut self, id: &str, title: &str) -> Result<()> {
        self.db.execute("UPDATE manuscripts SET title = ?2, updated_at = ?3 WHERE id = ?1", params![id, title.trim(), now_iso()])?;
        Ok(())
    }

    pub fn set_manuscript_language(&mut self, id: &str, language: &str) -> Result<()> {
        if !is_language(language) { bail!("不认识这种语言：{language}") }
        self.db.execute("UPDATE manuscripts SET language = ?2, updated_at = ?3 WHERE id = ?1", params![id, language, now_iso()])?;
        Ok(())
    }

    /// The style a manuscript's citations are written in, by its key; empty, it follows the settings' again.
    /// The words stay as they are: only how they read changes, so it is no change to the manuscript's date.
    pub fn set_manuscript_cite_style(&mut self, id: &str, key: &str) -> Result<()> {
        self.db.execute("UPDATE manuscripts SET cite_style = ?2 WHERE id = ?1", params![id, key.trim()])?;
        Ok(())
    }

    /// What a manuscript's translations and new versions of its words are to follow. Kept as it is typed, so a
    /// space at its end stays until the person goes on; it is no change to the manuscript's words or date.
    pub fn set_manuscript_requirement(&mut self, id: &str, requirement: &str) -> Result<()> {
        self.db.execute("UPDATE manuscripts SET requirement = ?2 WHERE id = ?1", params![id, requirement])?;
        Ok(())
    }

    /// Whether a manuscript is still the empty one 写作 opens a library
    /// with: no title, the sections a new one starts with and not a word
    /// in them, and nothing kept of it (a version, a comment, a
    /// conversation, a file it came from).
    pub fn manuscript_blank(&self, id: &str) -> Result<bool> {
        let Some(manuscript) = self.manuscript(id)? else { return Ok(false) };
        let shape: Vec<(String, &str)> = manuscript.sections.iter().map(|section| (section.name.clone(), section.kind.as_str())).collect();
        let untouched = manuscript.title.is_empty() && manuscript.sections.iter().all(|section| section.body.trim().is_empty() && section.level == 1) && LANGUAGES.iter().any(|(code, _)| imrad(code) == shape);
        if !untouched { return Ok(false) }
        let kept = |sql: &str| -> Result<bool> { Ok(self.db.query_row(sql, [id], |_| Ok(())).optional()?.is_some()) };
        Ok(!(kept("SELECT 1 FROM section_snapshots WHERE section_id IN (SELECT id FROM manuscript_sections WHERE manuscript_id = ?1)")?
            || kept("SELECT 1 FROM manuscript_comments WHERE manuscript_id = ?1")?
            || kept("SELECT 1 FROM manuscript_sources WHERE manuscript_id = ?1")?
            || kept("SELECT 1 FROM conversations WHERE paper_id = ?1")?))
    }

    /// Put away the manuscripts that are still blank, except `keep`: the
    /// empty one 写作 made so that there was something to write in gives
    /// way to the first real one (a new one, a file, a review), and does
    /// not stay beside it as a stray 未命名稿件. The ids put away.
    pub fn retire_blank_manuscripts(&mut self, keep: &str) -> Result<Vec<String>> {
        let mut retired = vec![];
        for manuscript in self.manuscripts()? {
            if manuscript.id != keep && self.manuscript_blank(&manuscript.id)? {
                self.remove_manuscript(&manuscript.id)?;
                retired.push(manuscript.id);
            }
        }
        Ok(retired)
    }

    /// Put a manuscript away: it stays in the database, only no longer listed.
    pub fn remove_manuscript(&mut self, id: &str) -> Result<()> {
        self.db.execute("UPDATE manuscripts SET removed_at = ?2 WHERE id = ?1", params![id, now_iso()])?;
        Ok(())
    }

    /// A section added at the end of a manuscript.
    pub fn add_section(&mut self, manuscript_id: &str, name: &str, kind: &str) -> Result<Section> {
        if !is_kind(kind) { bail!("没有这种部分：{kind}") }
        if name.trim().is_empty() { bail!("这一节需要一个名称。") }
        let position: i64 = self.db.query_row("SELECT coalesce(max(position) + 1, 0) FROM manuscript_sections WHERE manuscript_id = ?1", [manuscript_id], |r| r.get(0))?;
        let id = new_id("S");
        self.db.execute("INSERT INTO manuscript_sections(id, manuscript_id, position, name, kind, body, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, '', ?6)", params![id, manuscript_id, position, name.trim(), kind, now_iso()])?;
        self.touch_manuscript(manuscript_id)?;
        Ok(self.section_row(&id)?.1)
    }

    /// A subsection added under a section, after what is under it already: a method under 方法, a result under 结果.
    pub fn add_subsection(&mut self, parent_id: &str, name: &str, kind: &str) -> Result<Section> {
        if !is_kind(kind) { bail!("没有这种部分：{kind}") }
        if name.trim().is_empty() { bail!("这一节需要一个名称。") }
        let (manuscript, parent) = self.section_row(parent_id)?;
        if parent.level >= MOST_LEVELS { bail!("大纲最多分 {MOST_LEVELS} 级，这一节下面不能再加小节。") }
        let sections = self.sections_of(&manuscript)?;
        let Some(at) = sections.iter().position(|section| section.id == parent_id) else { bail!("没有这一节：{parent_id}") };
        let id = new_id("S");
        let mut order: Vec<&str> = sections.iter().map(|section| section.id.as_str()).collect();
        order.insert(under(&sections, at).end, &id);
        let tx = self.db.transaction()?;
        tx.execute("INSERT INTO manuscript_sections(id, manuscript_id, position, name, kind, body, updated_at, level) VALUES (?1, ?2, 0, ?3, ?4, '', ?5, ?6)", params![id, manuscript, name.trim(), kind, now_iso(), parent.level + 1])?;
        put_in_order(&tx, &order)?;
        tx.commit()?;
        self.touch_manuscript(&manuscript)?;
        Ok(self.section_row(&id)?.1)
    }

    /// Give a section another name and kind.
    pub fn edit_section(&mut self, id: &str, name: &str, kind: &str) -> Result<()> {
        if !is_kind(kind) { bail!("没有这种部分：{kind}") }
        if name.trim().is_empty() { bail!("这一节需要一个名称。") }
        let (manuscript, _) = self.section_row(id)?;
        self.db.execute("UPDATE manuscript_sections SET name = ?2, kind = ?3 WHERE id = ?1", params![id, name.trim(), kind])?;
        self.touch_manuscript(&manuscript)
    }

    /// Give a manuscript these sections, in this order: one named by its id
    /// stays what it is under the name and kind given, the others are made.
    /// A section of the manuscript that is not among them goes when nothing
    /// was ever written in it (no text, no snapshot); one with text stays,
    /// after the others. All of it or nothing.
    pub fn arrange_sections(&mut self, manuscript_id: &str, wanted: &[Arrange]) -> Result<Arranged> {
        let existing = self.sections_of(manuscript_id)?;
        for one in wanted {
            if !is_kind(&one.kind) { bail!("没有这种部分：{}", one.kind) }
            if one.name.trim().is_empty() { bail!("这一节需要一个名称。") }
            if !one.id.is_empty() && !existing.iter().any(|section| section.id == one.id) { bail!("没有这一节：{}", one.id) }
        }
        let mut arranged = Arranged::default();
        let now = now_iso();
        let tx = self.db.transaction()?;
        let mut position = 0i64;
        for one in wanted {
            if one.id.is_empty() {
                let id = new_id("S");
                tx.execute("INSERT INTO manuscript_sections(id, manuscript_id, position, name, kind, body, updated_at, level) VALUES (?1, ?2, ?3, ?4, ?5, '', ?6, ?7)", params![id, manuscript_id, position, one.name.trim(), one.kind, now, one.level])?;
                arranged.added.push(id);
            } else {
                tx.execute("UPDATE manuscript_sections SET position = ?2, name = ?3, kind = ?4, level = ?5 WHERE id = ?1", params![one.id, position, one.name.trim(), one.kind, one.level])?;
            }
            position += 1;
        }
        for section in existing.iter().filter(|section| !wanted.iter().any(|one| one.id == section.id)) {
            let versions: i64 = tx.query_row("SELECT count(*) FROM section_snapshots WHERE section_id = ?1", [&section.id], |r| r.get(0))?;
            if section.body.trim().is_empty() && versions == 0 {
                tx.execute("DELETE FROM manuscript_sections WHERE id = ?1", [&section.id])?;
                arranged.removed.push(section.name.clone());
            } else {
                tx.execute("UPDATE manuscript_sections SET position = ?2 WHERE id = ?1", params![section.id, position])?;
                arranged.kept.push(section.name.clone());
                position += 1;
            }
        }
        fit_outline(&tx, manuscript_id)?;
        tx.commit()?;
        self.touch_manuscript(manuscript_id)?;
        Ok(arranged)
    }

    /// Move a section with what is under it by `by` places among the sections beside it (`sibling`; negative:
    /// towards the start). It stays under the section it is under: past the first or the last of them it stops.
    pub fn move_section(&mut self, id: &str, by: i64) -> Result<()> {
        let (manuscript, _) = self.section_row(id)?;
        let mut sections = self.sections_of(&manuscript)?;
        let Some(mut at) = sections.iter().position(|other| other.id == id) else { return Ok(()) };
        let mut moved = false;
        for _ in 0..by.unsigned_abs() {
            let Some(other) = sibling(&sections, at, by > 0) else { break };
            let (ours, theirs) = (under(&sections, at), under(&sections, other));
            // The two stand next to each other: the later one goes before the earlier.
            let (first, second) = if by > 0 { (ours, theirs) } else { (theirs, ours) };
            let later: Vec<Section> = sections.drain(second.clone()).collect();
            sections.splice(first.start..first.start, later);
            at = if by > 0 { first.start + second.len() } else { first.start };
            moved = true;
        }
        if !moved { return Ok(()) }
        let tx = self.db.transaction()?;
        put_in_order(&tx, &sections.iter().map(|section| section.id.as_str()).collect::<Vec<_>>())?;
        tx.commit()?;
        self.touch_manuscript(&manuscript)
    }

    /// A section one level deeper, a subsection of the section before it as deep as it was, or one level less
    /// deep; what is under it goes with it, and the order stays. Less deep, the sections after it that were
    /// beside it are under it then, as headings are in Word.
    pub fn indent_section(&mut self, id: &str, deeper: bool) -> Result<()> {
        let (manuscript, _) = self.section_row(id)?;
        let sections = self.sections_of(&manuscript)?;
        let Some(at) = sections.iter().position(|other| other.id == id) else { bail!("没有这一节：{id}") };
        let block = under(&sections, at);
        if deeper {
            if sibling(&sections, at, false).is_none() { bail!("前面没有同一级的节，这一节不能成为它的小节。") }
            if sections[block.clone()].iter().any(|section| section.level >= MOST_LEVELS) { bail!("大纲最多分 {MOST_LEVELS} 级。") }
        } else if sections[at].level <= 1 {
            bail!("这已经是最上一级。")
        }
        let tx = self.db.transaction()?;
        for section in &sections[block] { tx.execute("UPDATE manuscript_sections SET level = ?2 WHERE id = ?1", params![section.id, if deeper { section.level + 1 } else { section.level - 1 }])?; }
        fit_outline(&tx, &manuscript)?;
        tx.commit()?;
        self.touch_manuscript(&manuscript)
    }

    /// Remove a section with its snapshots. The window asks first when it has text. Its subsections stay where
    /// they are, a level less deep: they do not become the section's before it.
    pub fn remove_section(&mut self, id: &str) -> Result<()> {
        let (manuscript, _) = self.section_row(id)?;
        let sections = self.sections_of(&manuscript)?;
        let tx = self.db.transaction()?;
        if let Some(at) = sections.iter().position(|other| other.id == id) {
            for section in &sections[under(&sections, at)][1..] { tx.execute("UPDATE manuscript_sections SET level = ?2 WHERE id = ?1", params![section.id, section.level - 1])?; }
        }
        tx.execute("DELETE FROM manuscript_sections WHERE id = ?1", [id])?;
        fit_outline(&tx, &manuscript)?;
        tx.commit()?;
        self.touch_manuscript(&manuscript)
    }

    /// Cut a section in two at the paragraph `at` (its bytes in the section's
    /// text). Cut at a heading (`Cut::Heading`, `Cut::Subheading`) that
    /// paragraph is the new section's name, as a heading typed without a
    /// heading style is: written back to a Word file it stays the paragraph
    /// it was. Otherwise the paragraph opens the new section, whose name is
    /// the manuscript's own (`bare`) and goes into no file. The new section is
    /// as deep as the one cut, a level deeper for a subheading, as deep as a
    /// `Cut::Level` says but no deeper than a subsection of the one cut; it
    /// comes before the subsections of the one cut, which are then under it
    /// when it is not less deep than they are, as the text after the cut is.
    /// The comments on what moved go with it. The section's text is kept as a
    /// snapshot first. The new section; the heading of a `bare` section's
    /// first paragraph names that section instead when it is as deep, which
    /// leaves none empty behind.
    pub fn split_section(&mut self, id: &str, at: Range<usize>, cut: Cut) -> Result<Section> {
        let heading = cut != Cut::Paragraph;
        let (manuscript, section) = self.section_row(id)?;
        let body = section.body.as_str();
        if at.start > at.end || at.end > body.len() || !body.is_char_boundary(at.start) || !body.is_char_boundary(at.end) { bail!("这一段已经不在这一节里了。") }
        let before = body[..at.start].trim_end();
        let (name, after) = if heading {
            (heading_name(&body[at.clone()])?, body[at.end..].trim_start())
        } else {
            let after = body[at.start..].trim_start();
            if after.is_empty() { bail!("这里之后没有文字可以拆成新的一节。") }
            if before.is_empty() { bail!("这已经是这一节的开头。") }
            let first = markdown::plain(paragraphs(after).next().unwrap_or(""));
            let first = first.trim();
            let short: String = first.chars().take(16).collect();
            (if short.len() < first.len() { format!("{short}…") } else { short }, after)
        };
        let kind = if heading { structure::canonical(&name).filter(|kind| is_kind(kind)).unwrap_or(&section.kind).to_string() } else { section.kind.clone() };
        let level = match cut {
            Cut::Subheading => (section.level + 1).min(MOST_LEVELS),
            Cut::Level(level) => level.clamp(1, (section.level + 1).min(MOST_LEVELS)),
            Cut::Heading | Cut::Paragraph => section.level,
        };
        if heading && cut != Cut::Subheading && level == section.level && section.bare && before.is_empty() {
            self.take_snapshot(id, "split")?;
            self.db.execute("UPDATE manuscript_sections SET name = ?2, kind = ?3, body = ?4, bare = 0, updated_at = ?5 WHERE id = ?1", params![id, name, kind, after, now_iso()])?;
            self.touch_manuscript(&manuscript)?;
            return Ok(self.section_row(id)?.1);
        }
        let position: i64 = self.db.query_row("SELECT position FROM manuscript_sections WHERE id = ?1", [id], |r| r.get(0))?;
        let quotes: Vec<(String, String)> = {
            let mut stmt = self.db.prepare("SELECT id, quote FROM manuscript_comments WHERE section_id = ?1")?;
            stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
        };
        let claims: Vec<(String, String)> = {
            let mut stmt = self.db.prepare("SELECT id, claim FROM cite_passages WHERE section_id = ?1")?;
            stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
        };
        self.take_snapshot(id, "split")?;
        let (new, now) = (new_id("S"), now_iso());
        let tx = self.db.transaction()?;
        tx.execute("UPDATE manuscript_sections SET body = ?2, updated_at = ?3 WHERE id = ?1", params![id, before, now])?;
        tx.execute("UPDATE manuscript_sections SET position = position + 1 WHERE manuscript_id = ?1 AND position > ?2", params![manuscript, position])?;
        tx.execute("INSERT INTO manuscript_sections(id, manuscript_id, position, name, kind, body, updated_at, bare, level) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)", params![new, manuscript, position + 1, name, kind, after, now, !heading, level])?;
        // A comment goes where its words are now: one about the heading's words goes with the heading.
        for (comment, quote) in quotes {
            let stays = quote.trim().is_empty() || comment::locate(before, &quote, None).is_some();
            let goes = comment::locate(after, &quote, None).is_some() || (heading && name.contains(quote.trim()));
            if !stays && goes { tx.execute("UPDATE manuscript_comments SET section_id = ?2 WHERE id = ?1", params![comment, new])?; }
        }
        // A citation's passage goes where the sentence that cites is now, known by how it begins.
        for (passage, claim) in claims {
            let opening: String = claim.chars().take(24).collect();
            if !opening.is_empty() && !before.contains(&opening) && after.contains(&opening) { tx.execute("UPDATE cite_passages SET section_id = ?2 WHERE id = ?1", params![passage, new])?; }
        }
        fit_outline(&tx, &manuscript)?;
        tx.commit()?;
        self.touch_manuscript(&manuscript)?;
        Ok(self.section_row(&new)?.1)
    }

    /// Put a section back into the one before it, its name a paragraph
    /// again (the name of a `bare` section was never one, and goes), with
    /// its comments; the undoing of `split_section`. The section before,
    /// whose text is kept as a snapshot first.
    pub fn merge_section(&mut self, id: &str) -> Result<Section> {
        let (manuscript, section) = self.section_row(id)?;
        let order = self.sections_of(&manuscript)?;
        let Some(at) = order.iter().position(|other| other.id == id) else { bail!("没有这一节：{id}") };
        let Some(previous) = at.checked_sub(1).map(|n| &order[n]) else { bail!("这是第一节，前面没有可以并入的一节。") };
        let name = (!section.bare).then_some(section.name.trim());
        let body = [previous.body.trim(), name.unwrap_or(""), section.body.trim()].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n\n");
        let previous = previous.id.clone();
        self.take_snapshot(&previous, "merge")?;
        let tx = self.db.transaction()?;
        tx.execute("UPDATE manuscript_sections SET body = ?2, updated_at = ?3 WHERE id = ?1", params![previous, body, now_iso()])?;
        tx.execute("UPDATE manuscript_comments SET section_id = ?2 WHERE section_id = ?1", params![id, previous])?;
        tx.execute("UPDATE cite_passages SET section_id = ?2 WHERE section_id = ?1", params![id, previous])?;
        tx.execute("DELETE FROM manuscript_sections WHERE id = ?1", [id])?;
        fit_outline(&tx, &manuscript)?;
        tx.commit()?;
        self.touch_manuscript(&manuscript)?;
        Ok(self.section_row(&previous)?.1)
    }

    /// The section cut where a change to its text, already kept, finished headings (`finished_headings` of its
    /// text and of what the change put in, `put`): each a section at its level with the text after it, one after
    /// the other, as `split_section` makes them. `None` when the change finished none.
    pub fn split_at_headings(&mut self, id: &str, put: Range<usize>) -> Result<Option<Typed>> {
        let (manuscript, was) = self.section_row(id)?;
        if finished_headings(&was.body, put.clone()).is_empty() { return Ok(None) }
        let levels = self.sections_of(&manuscript)?.into_iter().map(|section| (section.id, section.level)).collect();
        let (mut section, caret, mut put, mut made) = (was.clone(), put.end, put, vec![]);
        while let Some((line, level)) = finished_headings(&section.body, put.clone()).into_iter().next() {
            let next = self.split_section(&section.id, line, Cut::Level(level))?;
            // What follows a heading is the end of the text cut: a place in it is as far from the end as it was.
            let gone = section.body.len() - next.body.len();
            put = put.start.saturating_sub(gone)..put.end.saturating_sub(gone);
            if next.id != section.id { made.push(next.id.clone()) }
            section = next;
        }
        Ok(Some(Typed { section, caret: put.end, back: Unsplit { was, caret, made, levels } }))
    }

    /// Take back headings typed in a section (`split_at_headings`): the sections made go, their comments and the
    /// passages of their citations back to the section cut, which has its text, name and kind again, and every
    /// section the level it had. The section cut's text is kept as a snapshot first.
    pub fn unsplit(&mut self, back: &Unsplit) -> Result<Section> {
        let (manuscript, _) = self.section_row(&back.was.id)?;
        self.take_snapshot(&back.was.id, "merge")?;
        let tx = self.db.transaction()?;
        for made in &back.made {
            tx.execute("UPDATE manuscript_comments SET section_id = ?2 WHERE section_id = ?1", params![made, back.was.id])?;
            tx.execute("UPDATE cite_passages SET section_id = ?2 WHERE section_id = ?1", params![made, back.was.id])?;
            tx.execute("DELETE FROM manuscript_sections WHERE id = ?1", [made])?;
        }
        let was = &back.was;
        tx.execute("UPDATE manuscript_sections SET name = ?2, kind = ?3, body = ?4, bare = ?5, updated_at = ?6 WHERE id = ?1", params![was.id, was.name, was.kind, was.body, was.bare, now_iso()])?;
        for (id, level) in &back.levels { tx.execute("UPDATE manuscript_sections SET level = ?2 WHERE id = ?1", params![id, level])?; }
        fit_outline(&tx, &manuscript)?;
        tx.commit()?;
        self.touch_manuscript(&manuscript)?;
        Ok(self.section_row(&was.id)?.1)
    }

    /// Keep what is typed in a section.
    pub fn save_section_body(&mut self, id: &str, body: &str) -> Result<()> {
        let now = now_iso();
        let changed = self.db.execute("UPDATE manuscript_sections SET body = ?2, updated_at = ?3 WHERE id = ?1", params![id, body, now])?;
        if changed == 0 { bail!("没有这一节：{id}") }
        // The manuscript written in is the one last worked on, which is the one a library opens with.
        self.db.execute("UPDATE manuscripts SET updated_at = ?2 WHERE id = (SELECT manuscript_id FROM manuscript_sections WHERE id = ?1)", params![id, now])?;
        Ok(())
    }

    /// Add text at the end of a section, after keeping what it held as a
    /// snapshot, and keep the citation keys the text uses with their papers
    /// (`keys`: paper and key). The text the section now has.
    pub fn append_to_section(&mut self, id: &str, text: &str, keys: &[(String, String)]) -> Result<String> {
        if text.trim().is_empty() { bail!("没有可以加进稿件的文字") }
        self.take_snapshot(id, "insert")?;
        let now = now_iso();
        let tx = self.db.transaction()?;
        let Some(body) = tx.query_row("SELECT body FROM manuscript_sections WHERE id = ?1", [id], |r| r.get::<_, String>(0)).optional()? else { bail!("没有这一节：{id}") };
        for (paper_id, key) in keys {
            tx.execute("INSERT OR IGNORE INTO cite_keys(key, paper_id, made_at) VALUES (?1, ?2, ?3)", params![key, paper_id, now])?;
            let actual: String = tx.query_row("SELECT paper_id FROM cite_keys WHERE key = ?1", [key], |r| r.get(0))?;
            if &actual != paper_id { bail!("引用键与其他文献冲突：{key}") }
        }
        let body = if body.trim().is_empty() { text.trim().to_string() } else { format!("{}\n\n{}", body.trim_end(), text.trim()) };
        tx.execute("UPDATE manuscript_sections SET body = ?2, updated_at = ?3 WHERE id = ?1", params![id, body, now])?;
        tx.execute("UPDATE manuscripts SET updated_at = ?2 WHERE id = (SELECT manuscript_id FROM manuscript_sections WHERE id = ?1)", params![id, now])?;
        tx.commit()?;
        Ok(body)
    }

    /// A section's snapshots, newest first.
    pub fn snapshots(&self, section_id: &str) -> Result<Vec<Snapshot>> {
        let mut stmt = self.db.prepare("SELECT id, section_id, body, why, taken_at FROM section_snapshots WHERE section_id = ?1 ORDER BY id DESC")?;
        Ok(stmt.query_map([section_id], |r| Ok(Snapshot { id: r.get(0)?, section_id: r.get(1)?, body: r.get(2)?, why: r.get(3)?, taken_at: r.get(4)? }))?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Keep the section's saved text as a snapshot when it is `worth_keeping`;
    /// the oldest beyond `KEEP_SNAPSHOTS` go. Whether one was taken.
    pub fn take_snapshot(&mut self, section_id: &str, why: &str) -> Result<bool> {
        let Some(body) = self.db.query_row("SELECT body FROM manuscript_sections WHERE id = ?1", [section_id], |r| r.get::<_, String>(0)).optional()? else { return Ok(false) };
        let latest: Option<String> = self.db.query_row("SELECT body FROM section_snapshots WHERE section_id = ?1 ORDER BY id DESC LIMIT 1", [section_id], |r| r.get(0)).optional()?;
        if !worth_keeping(latest.as_deref(), &body) { return Ok(false) }
        self.db.execute("INSERT INTO section_snapshots(section_id, body, why, taken_at) VALUES (?1, ?2, ?3, ?4)", params![section_id, body, why, now_iso()])?;
        self.db.execute("DELETE FROM section_snapshots WHERE section_id = ?1 AND id NOT IN (SELECT id FROM section_snapshots WHERE section_id = ?1 ORDER BY id DESC LIMIT ?2)", params![section_id, KEEP_SNAPSHOTS as i64])?;
        Ok(true)
    }

    /// Put a snapshot's text back into its section, after keeping the
    /// current text as a snapshot of its own, so the restoring can be undone.
    /// The text the section now has.
    pub fn restore_snapshot(&mut self, snapshot_id: i64) -> Result<(String, String)> {
        let found = self.db.query_row("SELECT section_id, body FROM section_snapshots WHERE id = ?1", [snapshot_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).optional()?;
        let Some((section, body)) = found else { bail!("这个版本已经不在了。") };
        self.take_snapshot(&section, "restore")?;
        self.save_section_body(&section, &body)?;
        Ok((section, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn store() -> Store {
        Store::in_memory(Path::new("/tmp/wisp-manuscript-test")).unwrap()
    }

    #[test]
    fn a_manuscript_keeps_the_style_it_cites_in() {
        let mut store = store();
        let made = store.create_manuscript("Roots", "en").unwrap();
        // None of its own to begin with: it follows the settings'.
        assert_eq!(made.cite_style, "");
        let before = store.manuscript(&made.id).unwrap().unwrap().updated_at;
        store.set_manuscript_cite_style(&made.id, " csl:nature ").unwrap();
        let now = store.manuscript(&made.id).unwrap().unwrap();
        assert_eq!((now.cite_style.as_str(), now.updated_at == before), ("csl:nature", true));
        assert_eq!(store.manuscripts().unwrap().iter().find(|m| m.id == made.id).unwrap().cite_style, "csl:nature");
        // Another manuscript of the library is left with its own.
        let other = store.create_manuscript("Shoots", "en").unwrap();
        assert_eq!(store.manuscript(&other.id).unwrap().unwrap().cite_style, "");
        store.set_manuscript_cite_style(&made.id, "").unwrap();
        // The standing requirement is kept apart from the words, and leaves the date as it is.
        store.set_manuscript_requirement(&made.id, "American spelling").unwrap();
        let kept = store.manuscript(&made.id).unwrap().unwrap();
        assert_eq!((kept.requirement.as_str(), kept.updated_at == now.updated_at), ("American spelling", true));
        assert_eq!(store.manuscript(&made.id).unwrap().unwrap().cite_style, "");
    }

    #[test]
    fn words_looked_for_in_a_section() {
        let text = "Cells grow. 细胞生长很快，cells 也会分裂；CELLS.";
        let found: Vec<&str> = occurrences(text, "cells").into_iter().map(|range| &text[range]).collect();
        assert_eq!(found, ["Cells", "cells", "CELLS"]);
        assert_eq!(occurrences(text, "生长").into_iter().map(|range| &text[range]).collect::<Vec<_>>(), ["生长"]);
        // Apart, not overlapping; nothing typed finds nothing.
        assert_eq!(occurrences("aaaa", "aa"), [0..2, 2..4]);
        assert!(occurrences(text, "  ").is_empty() && occurrences(text, "absent").is_empty() && occurrences("", "a").is_empty());
    }

    #[test]
    fn text_is_added_at_a_sections_end_with_its_keys() {
        let mut store = store();
        let paper = store.add_paper(crate::model::Paper { title: "Roots".into(), ..Default::default() }).unwrap();
        let other = store.add_paper(crate::model::Paper { title: "Leaves".into(), ..Default::default() }).unwrap();
        let made = store.create_manuscript("Draft", "en").unwrap();
        let section = made.sections[1].id.clone();
        assert_eq!(store.append_to_section(&section, " First [@Root2024]. \n", &[(paper.id.clone(), "Root2024".into())]).unwrap(), "First [@Root2024].");
        assert_eq!(store.append_to_section(&section, "Second.", &[]).unwrap(), "First [@Root2024].\n\nSecond.");
        assert_eq!(store.cite_keys().unwrap().get("Root2024"), Some(&paper.id));
        // What the section held before the second addition can be brought back.
        assert_eq!(store.snapshots(&section).unwrap()[0].body, "First [@Root2024].");
        // A key that is another paper's stops the whole addition.
        assert!(store.append_to_section(&section, "Third [@Root2024].", &[(other.id.clone(), "Root2024".into())]).is_err());
        assert_eq!(store.manuscript(&made.id).unwrap().unwrap().sections[1].body, "First [@Root2024].\n\nSecond.");
        assert!(store.append_to_section(&section, "  ", &[]).is_err());
        assert!(store.append_to_section("nowhere", "Text.", &[]).is_err());
    }

    #[test]
    fn imported_sections_and_citation_keys_commit_together() {
        let mut store = store();
        let paper = store.add_paper(crate::model::Paper { title: "Roots".into(), ..Default::default() }).unwrap();
        let other = store.add_paper(crate::model::Paper { title: "Leaves".into(), ..Default::default() }).unwrap();
        let sections = vec![("Introduction".into(), "Text [@Root2024, p. 5]".into()), ("Conclusion".into(), "More".into())];
        let bad = vec![(paper.id.clone(), "Root2024".into()), ("missing".into(), "Missing2024".into())];
        assert!(store.create_manuscript_from_sections("Review", "en", &sections, &bad).is_err());
        assert!(store.manuscripts().unwrap().is_empty());
        assert!(store.cite_keys().unwrap().is_empty(), "an earlier reserved key must roll back");

        let conflict = vec![(paper.id.clone(), "Root2024".into()), (other.id.clone(), "root2024".into())];
        assert!(store.create_manuscript_from_sections("Review", "en", &sections, &conflict).is_err());
        assert!(store.manuscripts().unwrap().is_empty() && store.cite_keys().unwrap().is_empty());

        let made = store.create_manuscript_from_sections("Review", "en", &sections, &[(paper.id.clone(), "Root2024".into())]).unwrap();
        assert_eq!(made.sections.iter().map(|s| (s.name.clone(), s.body.clone())).collect::<Vec<_>>(), sections);
        assert_eq!(store.cite_keys().unwrap().get("Root2024"), Some(&paper.id));
    }

    #[test]
    fn a_new_manuscript_has_imrad_in_its_language() {
        let mut store = store();
        let en = store.create_manuscript("", "en").unwrap();
        assert_eq!(en.sections.iter().map(|s| (s.name.as_str(), s.kind.as_str())).collect::<Vec<_>>(), [("Abstract", "abstract"), ("Introduction", "introduction"), ("Methods", "methods"), ("Results", "results"), ("Discussion", "discussion")]);
        assert_eq!(en.shown_title(), UNTITLED);
        let zh = store.create_manuscript("根系", "zh").unwrap();
        assert_eq!(zh.sections[1].name, "导言");
        assert!(store.create_manuscript("x", "klingon").is_err());
        assert_eq!(store.manuscripts().unwrap().len(), 2);
        store.remove_manuscript(&en.id).unwrap();
        assert_eq!(store.manuscripts().unwrap().iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["根系"]);
    }

    #[test]
    fn the_empty_manuscript_writing_starts_with_gives_way_to_a_real_one() {
        let mut store = store();
        // What opening 写作 makes in a library without a manuscript, and then a new one beside it.
        let placeholder = store.create_manuscript("", DEFAULT_LANGUAGE).unwrap();
        assert!(store.manuscript_blank(&placeholder.id).unwrap());
        let real = store.create_manuscript("", "zh").unwrap();
        assert_eq!(store.retire_blank_manuscripts(&real.id).unwrap(), std::slice::from_ref(&placeholder.id));
        assert_eq!(store.manuscripts().unwrap().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), [real.id.as_str()]);
        // One the person did anything with is not blank: a title, a word, a section of their own, a version kept, a conversation.
        let fresh = |store: &mut Store| store.create_manuscript("", "en").unwrap();
        let titled = fresh(&mut store);
        store.rename_manuscript(&titled.id, "Roots").unwrap();
        let written = fresh(&mut store);
        store.save_section_body(&written.sections[1].id, "Roots grow.").unwrap();
        let shaped = fresh(&mut store);
        store.add_section(&shaped.id, "Root hairs", "other").unwrap();
        let emptied = fresh(&mut store);
        store.save_section_body(&emptied.sections[0].id, "A whole abstract that was written and is worth keeping as a version of this section.").unwrap();
        store.take_snapshot(&emptied.sections[0].id, "leave").unwrap();
        store.save_section_body(&emptied.sections[0].id, "").unwrap();
        let asked = fresh(&mut store);
        store.db.execute("INSERT INTO conversations(id, paper_id, created_at, updated_at) VALUES ('C-1', ?1, '', '')", [&asked.id]).unwrap();
        for manuscript in [&titled, &written, &shaped, &emptied, &asked] { assert!(!store.manuscript_blank(&manuscript.id).unwrap(), "{}", manuscript.id) }
        let blank = fresh(&mut store);
        // Made in the same millisecond (Windows' clock is coarse), the two come in the order of their ids' random tail.
        let mut retired = store.retire_blank_manuscripts("").unwrap();
        let mut expected = vec![real.id.clone(), blank.id.clone()];
        retired.sort();
        expected.sort();
        assert_eq!(retired, expected);
        assert_eq!(store.manuscripts().unwrap().len(), 5);
    }

    #[test]
    fn sections_can_be_added_renamed_moved_and_removed() {
        let mut store = store();
        let review = store.create_manuscript("A review", "en").unwrap();
        for section in &review.sections[2..] { store.remove_section(&section.id).unwrap() }
        let roots = store.add_section(&review.id, "Root architecture", "other").unwrap();
        let hairs = store.add_section(&review.id, "Root hairs", "other").unwrap();
        assert!(store.add_section(&review.id, "x", "preface").is_err());
        assert!(store.add_section(&review.id, "  ", "other").is_err());
        store.move_section(&hairs.id, -1).unwrap();
        store.move_section(&hairs.id, -10).unwrap();
        store.edit_section(&roots.id, "Architecture", "results").unwrap();
        let names: Vec<(String, String)> = store.manuscript(&review.id).unwrap().unwrap().sections.into_iter().map(|s| (s.name, s.kind)).collect();
        assert_eq!(names, [("Root hairs".to_string(), "other".to_string()), ("Abstract".into(), "abstract".into()), ("Introduction".into(), "introduction".into()), ("Architecture".into(), "results".into())]);
    }

    fn outline_of(levels: &[u8]) -> Vec<Section> {
        levels.iter().enumerate().map(|(n, level)| Section { id: format!("S{n}"), level: *level, ..Default::default() }).collect()
    }

    #[test]
    fn an_outline_is_its_sections_with_their_levels() {
        let mut levels = [2, 3, 1, 3, 0, 5, 2];
        fit_levels(&mut levels);
        assert_eq!(levels, [1, 2, 1, 2, 1, 2, 2]);
        // 导言; 方法 with two methods, the second with a part of its own; 结果 with one result.
        let sections = outline_of(&[1, 1, 2, 2, 3, 1, 2]);
        assert_eq!(numbers(&sections), ["1", "2", "2.1", "2.2", "2.2.1", "3", "3.1"]);
        assert_eq!((under(&sections, 1), under(&sections, 3), under(&sections, 6)), (1..5, 3..5, 6..7));
        assert_eq!((parent(&sections, 4), parent(&sections, 2), parent(&sections, 0)), (Some(3), Some(1), None));
        assert_eq!((sibling(&sections, 1, true), sibling(&sections, 1, false), sibling(&sections, 2, false), sibling(&sections, 2, true), sibling(&sections, 3, true)), (Some(5), Some(0), None, Some(3), None));
        // A level skipped counts as the first of it.
        assert_eq!(numbers(&outline_of(&[1, 3])), ["1", "1.1.1"]);
    }

    #[test]
    fn subsections_move_rise_and_sink_with_what_is_under_them() {
        let mut store = store();
        let made = store.create_manuscript("", "en").unwrap();
        let id = |store: &Store, name: &str| store.manuscript(&made.id).unwrap().unwrap().sections.iter().find(|s| s.name == name).unwrap().id.clone();
        let shape = |store: &Store| store.manuscript(&made.id).unwrap().unwrap().sections.iter().map(|s| format!("{}{}", "-".repeat(s.level as usize - 1), s.name)).collect::<Vec<_>>().join(" ");
        let methods = id(&store, "Methods");
        let cells = store.add_subsection(&methods, "Cell culture", "methods").unwrap();
        let imaging = store.add_subsection(&methods, "Imaging", "methods").unwrap();
        let confocal = store.add_subsection(&imaging.id, "Confocal", "methods").unwrap();
        assert_eq!((cells.level, imaging.level, confocal.level), (2, 2, 3));
        assert!(store.add_subsection(&confocal.id, "Lenses", "methods").unwrap_err().to_string().contains("最多分 3 级"));
        let results = id(&store, "Results");
        let first = store.add_subsection(&results, "Result 1", "results").unwrap();
        assert_eq!(shape(&store), "Abstract Introduction Methods -Cell culture -Imaging --Confocal Results -Result 1 Discussion");
        // A method moves past the one beside it with what is under it, and no further than the first under 方法.
        store.move_section(&imaging.id, -5).unwrap();
        assert_eq!(shape(&store), "Abstract Introduction Methods -Imaging --Confocal -Cell culture Results -Result 1 Discussion");
        store.move_section(&first.id, 1).unwrap();
        assert_eq!(shape(&store), "Abstract Introduction Methods -Imaging --Confocal -Cell culture Results -Result 1 Discussion");
        // 方法 goes after 结果 with all its methods, and back.
        store.move_section(&methods, 1).unwrap();
        assert_eq!(shape(&store), "Abstract Introduction Results -Result 1 Methods -Imaging --Confocal -Cell culture Discussion");
        store.move_section(&methods, -1).unwrap();
        // Deeper: a part of the method before it; not past the third level, and not without a section beside it before.
        store.indent_section(&cells.id, true).unwrap();
        assert_eq!(shape(&store), "Abstract Introduction Methods -Imaging --Confocal --Cell culture Results -Result 1 Discussion");
        assert!(store.indent_section(&cells.id, true).unwrap_err().to_string().contains("最多分 3 级"));
        assert!(store.indent_section(&imaging.id, true).unwrap_err().to_string().contains("前面没有同一级"));
        assert!(store.indent_section(&methods, false).is_err());
        // Less deep with what is under it, the order as it was; and back under 方法.
        store.indent_section(&imaging.id, false).unwrap();
        assert_eq!(shape(&store), "Abstract Introduction Methods Imaging -Confocal -Cell culture Results -Result 1 Discussion");
        store.indent_section(&imaging.id, true).unwrap();
        // A section removed leaves its subsections where they are, a level up, and not under the section before it.
        store.remove_section(&imaging.id).unwrap();
        assert_eq!(shape(&store), "Abstract Introduction Methods -Confocal -Cell culture Results -Result 1 Discussion");
        store.remove_section(&methods).unwrap();
        assert_eq!(shape(&store), "Abstract Introduction Confocal Cell culture Results -Result 1 Discussion");
        // A paragraph made a subheading is a subsection of the section it was in, before its other subsections.
        store.save_section_body(&results, "Overview.\n\nRoot length\n\nRoots grew.").unwrap();
        let start = "Overview.\n\n".len();
        let length = store.split_section(&results, start..start + "Root length".len(), Cut::Subheading).unwrap();
        assert_eq!((length.name.as_str(), length.level, length.body.as_str()), ("Root length", 2, "Roots grew."));
        assert_eq!(shape(&store), "Abstract Introduction Confocal Cell culture Results -Root length -Result 1 Discussion");
        // Merged back, the result after it is under 结果 still.
        store.merge_section(&length.id).unwrap();
        assert_eq!(shape(&store), "Abstract Introduction Confocal Cell culture Results -Result 1 Discussion");
        // A manuscript somebody gave subsections is no longer the blank one 写作 starts with.
        let blank = store.create_manuscript("", "en").unwrap();
        assert!(store.manuscript_blank(&blank.id).unwrap());
        store.indent_section(&blank.sections[1].id, true).unwrap();
        assert!(!store.manuscript_blank(&blank.id).unwrap());
    }

    #[test]
    fn headings_typed_in_a_section_are_sections_of_their_level() {
        // Enter after a heading finishes it; one still being typed, one written earlier, one in a quote or in code,
        // `####` (deeper than an outline goes) and `#tag` stay lines of the text.
        let text = "Seedlings grew.\n## Plant material\n";
        assert_eq!(finished_headings(text, text.len() - 1..text.len()), [("Seedlings grew.\n".len()..text.len() - 1, 2)]);
        assert!(finished_headings("## Plant mat", 11..12).is_empty());
        let earlier = "## Plant material\n\nSeedlings grew.\n";
        assert!(finished_headings(earlier, earlier.len() - 1..earlier.len()).is_empty());
        for line in ["> ## Quoted\n", "```\n## Code\n", "#### Detail\n", "#tag\n", "## \n"] { assert!(finished_headings(line, line.len() - 1..line.len()).is_empty(), "{line}") }
        // A paste: each heading in it, in order.
        let pasted = "Opening.\n## Limits\nSmall sample.\n### Power\nToo few plants.";
        assert_eq!(finished_headings(pasted, 0..pasted.len()).into_iter().map(|(_, level)| level).collect::<Vec<_>>(), [2, 3]);

        let mut store = store();
        let made = store.create_manuscript("", "en").unwrap();
        let id = |store: &Store, name: &str| store.manuscript(&made.id).unwrap().unwrap().sections.iter().find(|s| s.name == name).unwrap().id.clone();
        let shape = |store: &Store| store.manuscript(&made.id).unwrap().unwrap().sections.iter().map(|s| format!("{}{}", "-".repeat(s.level as usize - 1), s.name)).collect::<Vec<_>>().join(" ");
        let methods = id(&store, "Methods");
        let cells = store.add_subsection(&methods, "Cell culture", "methods").unwrap();
        // `## Plant material` and Enter in 方法's text: a subsection with the text after it, before the other subsections,
        // the caret at its start; a comment goes with its words.
        let text = "Seedlings grew.\n## Plant material\nSeeds were sown.";
        store.save_section_body(&methods, text).unwrap();
        store.add_comment(&made.id, &methods, "sown", "When?", "Xu", comment::PERSON).unwrap();
        let enter = "Seedlings grew.\n## Plant material".len();
        assert!(store.split_at_headings(&methods, 0..enter).unwrap().is_none());
        let typed = store.split_at_headings(&methods, enter..enter + 1).unwrap().unwrap();
        assert_eq!((typed.section.name.as_str(), typed.section.level, typed.section.kind.as_str(), typed.section.body.as_str(), typed.caret), ("Plant material", 2, "methods", "Seeds were sown.", 0));
        assert_eq!(shape(&store), "Abstract Introduction Methods -Plant material -Cell culture Results Discussion");
        assert_eq!(store.comments(&made.id).unwrap()[0].section_id, typed.section.id);
        // Taken back: the text as it was typed, the comment where it was.
        assert_eq!(store.unsplit(&typed.back).unwrap().body, text);
        assert_eq!(shape(&store), "Abstract Introduction Methods -Cell culture Results Discussion");
        assert_eq!(store.comments(&made.id).unwrap()[0].section_id, methods);

        // `#` in a subsection: a section of the first level, which takes what follows under itself; taken back, the
        // part of a method after it is that method's again.
        let imaging = store.add_subsection(&methods, "Imaging", "methods").unwrap();
        store.add_subsection(&imaging.id, "Confocal", "methods").unwrap();
        let text = "Roots were imaged.\n# Growth\n";
        store.save_section_body(&imaging.id, text).unwrap();
        let typed = store.split_at_headings(&imaging.id, text.len() - 1..text.len()).unwrap().unwrap();
        assert_eq!(shape(&store), "Abstract Introduction Methods -Cell culture -Imaging Growth -Confocal Results Discussion");
        store.unsplit(&typed.back).unwrap();
        assert_eq!(shape(&store), "Abstract Introduction Methods -Cell culture -Imaging --Confocal Results Discussion");
        assert_eq!(store.section_row(&cells.id).unwrap().1.level, 2);

        // `###` with no subsection before it is no deeper than a subsection of the section it is typed in.
        let introduction = id(&store, "Introduction");
        let text = "Why.\n### Gap\n";
        store.save_section_body(&introduction, text).unwrap();
        let typed = store.split_at_headings(&introduction, text.len() - 1..text.len()).unwrap().unwrap();
        assert_eq!((typed.section.name.as_str(), typed.section.level), ("Gap", 2));
        store.unsplit(&typed.back).unwrap();

        // A paste with two headings: one section after the other, each with its text, the caret at the end of the paste.
        let discussion = id(&store, "Discussion");
        store.save_section_body(&discussion, pasted).unwrap();
        let typed = store.split_at_headings(&discussion, 0..pasted.len()).unwrap().unwrap();
        assert_eq!((typed.section.name.as_str(), typed.section.body.as_str(), typed.caret), ("Power", "Too few plants.", "Too few plants.".len()));
        assert_eq!(shape(&store), "Abstract Introduction Methods -Cell culture -Imaging --Confocal Results Discussion -Limits --Power");
        assert_eq!(store.section_row(&discussion).unwrap().1.body, "Opening.");
        assert_eq!(store.unsplit(&typed.back).unwrap().body, pasted);
        assert_eq!(shape(&store), "Abstract Introduction Methods -Cell culture -Imaging --Confocal Results Discussion");
    }

    #[test]
    fn snapshots_are_kept_only_on_change_bounded_and_restoring_is_undoable() {
        let mut store = store();
        let manuscript = store.create_manuscript("", "en").unwrap();
        let section = manuscript.sections[1].id.clone();
        assert!(!store.take_snapshot(&section, "pause").unwrap(), "nothing written, nothing kept");
        store.save_section_body(&section, "Roots grow.").unwrap();
        assert!(store.take_snapshot(&section, "pause").unwrap());
        assert!(!store.take_snapshot(&section, "leave").unwrap(), "unchanged");
        store.save_section_body(&section, "Roots grow down.").unwrap();
        assert!(store.take_snapshot(&section, "leave").unwrap());
        let first = store.snapshots(&section).unwrap().last().unwrap().clone();
        assert_eq!(first.body, "Roots grow.");
        store.save_section_body(&section, "Roots grow down. Typed after.").unwrap();
        let (restored_in, body) = store.restore_snapshot(first.id).unwrap();
        assert_eq!((restored_in.as_str(), body.as_str()), (section.as_str(), "Roots grow."));
        let newest = store.snapshots(&section).unwrap()[0].clone();
        assert_eq!((newest.body.as_str(), newest.why.as_str()), ("Roots grow down. Typed after.", "restore"));
        assert_eq!(store.manuscript(&manuscript.id).unwrap().unwrap().sections[1].body, "Roots grow.");
        for n in 0..KEEP_SNAPSHOTS + 5 {
            store.save_section_body(&section, &format!("version {n}")).unwrap();
            store.take_snapshot(&section, "pause").unwrap();
        }
        let kept = store.snapshots(&section).unwrap();
        assert_eq!(kept.len(), KEEP_SNAPSHOTS);
        assert_eq!(kept[0].body, format!("version {}", KEEP_SNAPSHOTS + 4));
        store.remove_section(&section).unwrap();
        assert!(store.snapshots(&section).unwrap().is_empty());
    }

    #[test]
    fn the_policy() {
        assert!(!worth_keeping(None, "  \n"));
        assert!(worth_keeping(None, "a"));
        assert!(!worth_keeping(Some("a\n"), "a"));
        assert!(worth_keeping(Some("a"), ""));
    }

    #[test]
    fn old_drafts_become_one_manuscript() {
        let mut store = store();
        store.db.execute_batch("INSERT INTO drafts(section, body, updated_at) VALUES ('introduction', 'Roots are long.', 'x'), ('discussion', '', 'x'), ('other', 'Notes.', 'x'), ('preface', 'Later key.', 'x')").unwrap();
        migrate_drafts(&mut store.db).unwrap();
        let list = store.manuscripts().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, FROM_DRAFTS);
        let sections: Vec<(&str, &str, &str)> = list[0].sections.iter().map(|s| (s.name.as_str(), s.kind.as_str(), s.body.as_str())).collect();
        assert_eq!(sections, [("Abstract", "abstract", ""), ("Introduction", "introduction", "Roots are long."), ("Methods", "methods", ""), ("Results", "results", ""), ("Discussion", "discussion", ""), ("Other", "other", "Notes."), ("preface", "other", "Later key.")]);
        let left: i64 = store.db.query_row("SELECT count(*) FROM drafts", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
        // Run again (every open does): nothing more.
        migrate_drafts(&mut store.db).unwrap();
        assert_eq!(store.manuscripts().unwrap().len(), 1);
        // Drafts that are all empty make no manuscript, and go.
        store.db.execute_batch("INSERT INTO drafts(section, body, updated_at) VALUES ('methods', ' ', 'x')").unwrap();
        migrate_drafts(&mut store.db).unwrap();
        assert_eq!(store.manuscripts().unwrap().len(), 1);
    }

    #[test]
    fn a_library_with_drafts_opens_with_them() {
        let dir = std::env::temp_dir().join(format!("wisp-manuscript-{}", new_id("T")));
        {
            let store = Store::open(&dir).unwrap();
            store.db.execute_batch("INSERT INTO drafts(section, body, updated_at) VALUES ('methods', 'We grew plants.', 'x')").unwrap();
        }
        let store = Store::open(&dir).unwrap();
        let list = store.manuscripts().unwrap();
        assert_eq!(list[0].sections[2].body, "We grew plants.");
        std::fs::remove_dir_all(&dir).ok();
    }

    fn sample() -> Manuscript {
        let section = |name: &str, body: &str| Section { name: name.into(), kind: "other".into(), body: body.into(), ..Default::default() };
        Manuscript { title: "Roots & <shoots>".into(), language: "en".into(), sections: vec![section("Introduction", "First line.\n\n\nSecond line.  \n"), section("Methods", "  "), section("Results", "Grew.")], ..Default::default() }
    }

    #[test]
    fn exports_put_names_as_headings_and_leave_out_empty_sections() {
        assert_eq!(markdown(&sample()), "# Roots & <shoots>\n\n## Introduction\n\nFirst line.\n\nSecond line.\n\n## Results\n\nGrew.\n");
        assert_eq!(plain_text(&sample()), "Roots & <shoots>\n\nIntroduction\n\nFirst line.\n\nSecond line.\n\nResults\n\nGrew.\n");
    }

    #[test]
    fn a_markdown_file_becomes_sections_at_its_headings() {
        let text = "---\ntitle: \"Roots and shoots\"\nauthor: x\n---\n\nA line before\nany heading.\n\n## Introduction\n\nRoots grow\ndown.\n根向下\n生长。\n\n- one\n- two\n\n### Hairs\n\nShort.\n\n## 方法\n\n```\ncode  line\n```\n";
        let draft = from_markdown(text, "notes/draft.md");
        assert_eq!((draft.title.as_str(), draft.language.as_str()), ("Roots and shoots", "en"));
        let shown: Vec<(&str, &str, bool, Vec<&str>)> = draft.sections.iter().map(|s| (s.name.as_str(), s.kind.as_str(), s.bare, s.paragraphs.iter().map(String::as_str).collect())).collect();
        assert_eq!(shown, [("Front matter", "other", true, vec!["A line before any heading."]), ("Introduction", "introduction", false, vec!["Roots grow down. 根向下生长。", "- one\n- two"]), ("Hairs", "introduction", false, vec!["Short."]), ("方法", "methods", false, vec!["```\ncode  line\n```"])]);
        assert_eq!(draft.sections.iter().map(|s| s.level).collect::<Vec<_>>(), [1, 1, 2, 1]);
        // One `#` heading before the rest is the title; without any heading the file's name is, and the text is one section.
        let draft = from_markdown("# 根的综述\n\n## 引言\n\n根向下生长，吸收水分。\n", "x.md");
        assert_eq!((draft.title.as_str(), draft.language.as_str(), draft.sections.len(), draft.sections[0].name.as_str(), draft.sections[0].kind.as_str()), ("根的综述", "zh", 1, "引言", "introduction"));
        let draft = from_markdown("只有一段话，没有标题，写得长一点才不会被当成题目。\n\n第二段。", "随手记.txt");
        assert_eq!((draft.title.as_str(), draft.sections.len(), draft.sections[0].name.as_str(), draft.sections[0].bare), ("随手记", 1, "正文", true));
        assert_eq!(from_markdown("", "empty.md").sections.len(), 1);
    }

    #[test]
    fn subsections_are_written_a_level_under_their_section_and_read_back_so() {
        let section = |name: &str, level: u8, body: &str| Section { name: name.into(), kind: "methods".into(), body: body.into(), level, ..Default::default() };
        // 方法 has no text of its own, a method none either but a part of it has: both are written with their headings.
        let manuscript = Manuscript { title: "T".into(), sections: vec![section("Methods", 1, ""), section("Cell culture", 2, "Cells grew.\n\n# Media\n\nDMEM."), section("Unwritten", 2, ""), section("Imaging", 2, ""), section("Confocal", 3, "Zeiss.")], ..Default::default() };
        let written = markdown(&manuscript);
        assert_eq!(written, "# T\n\n## Methods\n\n### Cell culture\n\nCells grew.\n\n#### Media\n\nDMEM.\n\n### Imaging\n\n#### Confocal\n\nZeiss.\n");
        let xml = String::from_utf8_lossy(&docx(&manuscript)).to_string();
        for (style, text) in [("Heading1", "Methods"), ("Heading2", "Cell culture"), ("Heading3", "Media"), ("Heading2", "Imaging"), ("Heading3", "Confocal")] {
            assert!(xml.contains(&format!("<w:pStyle w:val=\"{style}\"/></w:pPr><w:r><w:t xml:space=\"preserve\">{text}<")), "{style} {text}");
        }
        assert!(!xml.contains("Unwritten") && xml.contains("w:styleId=\"Heading6\""));
        // A file's headings one and two levels under its highest are subsections, of the part of the section they are
        // under; a heading deeper still stays in the text, with as many marks as it is levels under its section.
        let draft = from_markdown("# T\n\n## Methods\n\nIntro.\n\n### Cells\n\nGrew.\n\n#### Media\n\nDMEM.\n\n##### Brand\n\nGibco.\n\n## Results\n\n#### Roots\n\nLong.\n", "x.md");
        let shown: Vec<(&str, u8, &str, Vec<&str>)> = draft.sections.iter().map(|s| (s.name.as_str(), s.level, s.kind.as_str(), s.paragraphs.iter().map(String::as_str).collect())).collect();
        assert_eq!(shown, [("Methods", 1, "methods", vec!["Intro."]), ("Cells", 2, "methods", vec!["Grew."]), ("Media", 3, "methods", vec!["DMEM.", "# Brand", "Gibco."]), ("Results", 1, "results", vec![]), ("Roots", 2, "results", vec!["Long."])]);
        let mut store = store();
        let made = store.import_manuscript(&draft, "markdown", "x.md", None).unwrap();
        assert_eq!(numbers(&made.sections), ["1", "1.1", "1.1.1", "2", "2.1"]);
        // What was exported comes back as the same outline.
        let again = from_markdown(&written, "T.md");
        assert_eq!(again.sections.iter().map(|s| (s.name.as_str(), s.level)).collect::<Vec<_>>(), [("Methods", 1), ("Cell culture", 2), ("Media", 3), ("Imaging", 2), ("Confocal", 3)]);
    }

    #[test]
    fn a_markdown_file_keeps_its_tables_lists_and_code_together() {
        let text = "# Models\n\n## Results\n\nThe models\nare these:\n| Model | Year |\n| --- | --- |\n| LSTM | 1997 |\nAfter the table.\n\nModel | Year\n:-- | --:\nGRU | 2014\n\n  - one\n  - two\n      1. deeper\n  - three\nnot an item\n\n- apart\n\n````python\ndef f():\n    return 1\n\n```\n# no heading\n| a | b |\n|---|---|\n````\n***\nlast\nwords\n";
        let draft = from_markdown(text, "models.md");
        assert_eq!((draft.title.as_str(), draft.sections.len(), draft.sections[0].name.as_str()), ("Models", 1, "Results"));
        // The rows of a table, with or without the pipes at its edges, the items of a list as far in as each is
        // nested, and the lines of code, the blank one and what only looks like a heading or a table among them:
        // each one under the other, as the file has them. A rule is a line of its own, and nothing joins it.
        assert_eq!(draft.sections[0].paragraphs, ["The models are these:", "| Model | Year |\n| --- | --- |\n| LSTM | 1997 |", "After the table.", "Model | Year\n:-- | --:\nGRU | 2014", "- one\n- two\n    1. deeper\n- three", "not an item", "- apart", "````python\ndef f():\n    return 1\n\n```\n# no heading\n| a | b |\n|---|---|\n````", "***", "last words"]);
        // Kept as a manuscript, the section's text has a table where the file had one: before, each row was a paragraph of pipes.
        let mut store = store();
        let made = store.import_manuscript(&draft, "markdown", "models.md", None).unwrap();
        let blocks = markdown::parse(&made.sections[0].body);
        assert!(matches!(&blocks[1], markdown::Block::Table { rows, .. } if rows.len() == 2 && rows[1][0].text == "LSTM"), "{blocks:?}");
        assert!(matches!(&blocks[3], markdown::Block::Table { rows, .. } if rows.len() == 2 && rows[1][1].text == "2014"), "{blocks:?}");
        // A file that opens with a table, a list or code has no title in it; a fence left open takes the rest, without the blank lines at its end.
        let draft = from_markdown("| a | b |\n|---|---|\n\nShort words", "table.md");
        assert_eq!((draft.title.as_str(), &draft.sections[0].paragraphs[..]), ("table", &["| a | b |\n|---|---|".to_string(), "Short words".into()][..]));
        assert_eq!(from_markdown("Some words here.\n\n```\ncode\n\n## Not a section\n\n\n", "open.md").sections[0].paragraphs, ["Some words here.", "```\ncode\n\n## Not a section"]);
    }

    #[test]
    fn an_exported_manuscript_read_again_has_the_same_sections() {
        let section = |name: &str, body: &str| Section { name: name.into(), kind: "other".into(), body: body.into(), ..Default::default() };
        let bodies = [
            "Compared:\n\n| Model | Year |\n|---|:-:|\n| LSTM | 1997 |\n| GRU | **2014** |\n\nThat is all.",
            "- one\n- two\n  - nested\n    1. deeper\n\nBetween the lists.\n\n1. first\n2. second\n\n• a dot\n• another",
            "```python\ndef f():\n    return 1\n\n\nprint(f())\n```\n\n~~~\n| a | b |\n|---|---|\n- no item\n~~~\n\nAfter the code.",
        ];
        let manuscript = Manuscript { title: "Round trip".into(), language: "en".into(), sections: vec![section("Tables", bodies[0]), section("Lists", bodies[1]), section("Code", bodies[2])], ..Default::default() };
        let written = markdown(&manuscript);
        let draft = from_markdown(&written, "round.md");
        assert_eq!((draft.title.as_str(), draft.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>()), ("Round trip", vec!["Tables", "Lists", "Code"]));
        assert_eq!(draft.sections.iter().map(|s| s.paragraphs.join("\n\n")).collect::<Vec<_>>(), bodies);
        // Kept, the sections' texts are those, and written again the file is the same to the byte.
        let mut store = store();
        let made = store.import_manuscript(&draft, "markdown", "round.md", None).unwrap();
        assert_eq!(made.sections.iter().map(|s| s.body.as_str()).collect::<Vec<_>>(), bodies);
        assert_eq!(markdown(&made), written);
        // Lines written right under each other in the editor are paragraphs apart in the file and come back so, the table still whole.
        let manuscript = Manuscript { title: "T".into(), sections: vec![section("Results", "Compared:\n| a | b |\n|---|---|\n| 1 | 2 |\nThe end.")], ..Default::default() };
        assert_eq!(from_markdown(&markdown(&manuscript), "t.md").sections[0].paragraphs, ["Compared:", "| a | b |\n|---|---|\n| 1 | 2 |", "The end."]);
    }

    /// A Word file as this app writes one, with a comment put on it.
    fn word_file() -> Vec<u8> {
        let section = |name: &str, body: &str| Section { name: name.into(), kind: "other".into(), body: body.into(), ..Default::default() };
        let written = Manuscript { title: "Roots".into(), language: "en".into(), sections: vec![section("Introduction", "Roots grow down into the soil.\n\nThey take up water."), section("Methods", "We grew plants.")], ..Default::default() };
        let mut paragraphs = wanted(&written, None, &[], None, &|_| None);
        paragraphs[1 + 1].notes.push(docx::Note { range: 11..15, author: "Li Hua".into(), date: "2026-01-02T03:04:05Z".into(), body: "How far down?".into() });
        docx::rewrite(&docx(&written), &paragraphs, &docx::Options::default()).unwrap().bytes
    }

    #[test]
    fn a_subsection_written_into_a_word_file_looks_like_its_subsections() {
        let dir = std::env::temp_dir().join(format!("wisp-manuscript-{}", new_id("T")));
        let mut store = Store::open(&dir).unwrap();
        let section = |name: &str, level: u8, body: &str| Section { name: name.into(), kind: "other".into(), body: body.into(), level, ..Default::default() };
        let written = Manuscript { title: "Roots".into(), language: "en".into(), sections: vec![section("Methods", 1, "We grew plants."), section("Plant growth", 2, "In soil."), section("Results", 1, "Grew.")], ..Default::default() };
        let bytes = docx(&written);
        let draft = from_word(&docx::read(&bytes).unwrap(), "Roots.docx");
        assert_eq!(draft.sections.iter().map(|s| (s.name.as_str(), s.level)).collect::<Vec<_>>(), [("Methods", 1), ("Plant growth", 2), ("Results", 1)]);
        let made = store.import_manuscript(&draft, "docx", "Roots.docx", Some(&bytes)).unwrap();
        // A result added under 结果: the nearest heading before it is the section's, but it is set like the file's subsections.
        let length = store.add_subsection(&made.sections[2].id, "Root length", "results").unwrap();
        store.save_section_body(&length.id, "Longer.").unwrap();
        let now = store.manuscript(&made.id).unwrap().unwrap();
        let (out, from_file) = store.manuscript_word(&now, &[], false, "Xu", cite::Style::AuthorYear).unwrap();
        assert!(from_file);
        let read = docx::read(&out.bytes).unwrap();
        let level = |text: &str| read.paragraphs.iter().find(|p| p.text == text).and_then(|p| p.level);
        assert_eq!((level("Results"), level("Plant growth"), level("Root length")), (Some(1), Some(2), Some(2)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_word_file_is_taken_in_and_written_again_with_what_changed() {
        let dir = std::env::temp_dir().join(format!("wisp-manuscript-{}", new_id("T")));
        let mut store = Store::open(&dir).unwrap();
        let bytes = word_file();
        let draft = from_word(&docx::read(&bytes).unwrap(), "Roots v3.docx");
        assert_eq!((draft.title.as_str(), draft.titled, draft.sections.iter().map(|s| (s.name.as_str(), s.kind.as_str())).collect::<Vec<_>>()), ("Roots", true, vec![("Introduction", "introduction"), ("Methods", "methods")]));
        let made = store.import_manuscript(&draft, "docx", "Roots v3.docx", Some(&bytes)).unwrap();
        assert_eq!(made.sections[0].body, "Roots grow down into the soil.\n\nThey take up water.");
        let source = store.manuscript_source(&made.id).unwrap().unwrap();
        assert_eq!((source.kind.as_str(), source.name.as_str(), source.titled, std::fs::read(store.source_file(&source)).unwrap() == bytes), ("docx", "Roots v3.docx", true, true));
        let theirs = store.comments(&made.id).unwrap();
        assert_eq!(theirs.iter().map(|c| (c.origin.as_str(), c.author.as_str(), c.quote.as_str(), c.body.as_str(), c.section_id == made.sections[0].id)).collect::<Vec<_>>(), [("file", "Li Hua", "down", "How far down?", true)]);

        // Nothing changed: the file is written again as it was, and their comment is not put in twice.
        let (same, from_file) = store.manuscript_word(&made, &theirs, true, "Xu", cite::Style::AuthorYear).unwrap();
        assert!(from_file);
        assert_eq!((same.kept, same.changed, same.added, same.removed, same.comments), (6, 0, 0, 0, 0));

        // A paragraph changed, one put in, a section added, and comments of the person's and the assistant's.
        let (intro, methods) = (made.sections[0].id.clone(), made.sections[1].id.clone());
        store.save_section_body(&intro, "Roots grow two metres down into the soil.\n\nThey take up water.\n\nA new paragraph.").unwrap();
        store.add_section(&made.id, "Outlook", "discussion").unwrap();
        store.add_comment(&made.id, &intro, "take up water", "And salts?", "Xu", comment::PERSON).unwrap();
        let remark = store.add_remarks(&made.id, &methods, &[comment::Remark { quote: "We grew plants.".into(), body: "说明品种".into(), suggestion: "We grew Arabidopsis.".into() }]).unwrap();
        store.add_comment(&made.id, &methods, "words that are gone", "Lost its place.", "Xu", comment::PERSON).unwrap();
        let now = store.manuscript(&made.id).unwrap().unwrap();
        let (out, _) = store.manuscript_word(&now, &store.comments(&made.id).unwrap(), true, "Xu", cite::Style::AuthorYear).unwrap();
        assert_eq!((out.kept, out.changed, out.added, out.removed, out.comments), (5, 1, 2, 0, 3));
        let read = docx::read(&out.bytes).unwrap();
        assert_eq!(read.paragraphs.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["Roots", "Introduction", "Roots grow two metres down into the soil.", "They take up water.", "A new paragraph.", "Methods", "We grew plants.", "Outlook"]);
        let said: Vec<(&str, &str, &str)> = read.comments.iter().map(|c| (c.author.as_str(), c.quote.as_str(), c.body.as_str())).collect();
        assert_eq!(said, [("Li Hua", "down", "How far down?"), ("Xu", "take up water", "And salts?"), ("Xu", "Methods", "「words that are gone」\nLost its place."), (comment::ASSISTANT_NAME, "We grew plants.", "说明品种\n建议改为：We grew Arabidopsis.")]);
        assert!(read.tracked);

        // Its proposal taken, the comment stands on the new words.
        store.save_section_body(&methods, "We grew Arabidopsis.").unwrap();
        store.save_comment(&remark[0].taken()).unwrap();
        let now = store.manuscript(&made.id).unwrap().unwrap();
        let (out, _) = store.manuscript_word(&now, &store.comments(&made.id).unwrap(), false, "Xu", cite::Style::AuthorYear).unwrap();
        let read = docx::read(&out.bytes).unwrap();
        assert!(read.comments.iter().any(|c| c.quote == "We grew Arabidopsis." && c.body == "说明品种") && !read.tracked);

        // Without its file a manuscript is a plain document, comments included.
        std::fs::remove_file(store.source_file(&source)).unwrap();
        let (plain, from_file) = store.manuscript_word(&now, &store.comments(&made.id).unwrap(), true, "Xu", cite::Style::AuthorYear).unwrap();
        assert!(!from_file && docx::read(&plain.bytes).unwrap().comments.len() == 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_word_file_without_heading_styles_is_cut_at_the_names_every_paper_has() {
        let para = |text: &str| docx::ReadPara { text: text.into(), ..Default::default() };
        let read = docx::Read { paragraphs: vec![para("植物单细胞组学的现状"), para("张三，李四"), para("摘要"), para("单细胞技术正在改变植物学研究的方式，本文对此作一综述。"), para("1 引言"), para("植物器官由多种细胞类型组成。"), para("参考文献"), para("[1] Zhang 2021.")], comments: vec![docx::ReadComment { author: "Li".into(), body: "补充".into(), paragraph: Some(5), quote: "多种细胞类型".into(), ..Default::default() }, docx::ReadComment { body: "on a table".into(), ..Default::default() }], ..Default::default() };
        let draft = from_word(&read, "稿子.docx");
        assert_eq!((draft.title.as_str(), draft.titled, draft.language.as_str()), ("植物单细胞组学的现状", true, "zh"));
        assert_eq!(draft.sections.iter().map(|s| (s.name.as_str(), s.kind.as_str(), s.bare, s.paragraphs.len())).collect::<Vec<_>>(), [("开头", "other", true, 1), ("摘要", "abstract", false, 1), ("1 引言", "introduction", false, 1), ("参考文献", "other", false, 1)]);
        assert_eq!(draft.comments.iter().map(|c| (c.section, c.quote.as_str())).collect::<Vec<_>>(), [(2, "多种细胞类型"), (0, "")]);
    }

    /// A Word file typed without heading styles: its headings are plain paragraphs, one of them in bold.
    fn styleless_word_file() -> Vec<u8> {
        let body = ["单细胞转录组测序技术能够在单个细胞水平上解析基因表达，为研究植物发育提供了新的视角。", "单细胞转录组测序技术的发展（图1）", "自2009年首次报道以来，单细胞转录组测序技术经历了快速的发展，通量不断提高，成本不断下降。", "原生质体的制备", "植物细胞有细胞壁，需要先酶解制备原生质体，这一步骤可能改变部分基因的表达。", "展望", "随着空间转录组等技术的发展，单细胞水平的研究将更加深入。"].join("\n\n");
        let written = Manuscript { title: "单细胞转录组测序技术在植物发育生物学上的应用及进展".into(), language: "zh".into(), sections: vec![Section { name: "摘要".into(), kind: "abstract".into(), body, ..Default::default() }], ..Default::default() };
        let mut zip = docx::Zip::read(&docx(&written)).unwrap();
        let xml = zip.text("word/document.xml").unwrap().replace("<w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr>", "").replace("<w:pPr><w:pStyle w:val=\"Title\"/></w:pPr>", "").replace("<w:r><w:t xml:space=\"preserve\">原生质体的制备", "<w:r><w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">原生质体的制备");
        zip.set("word/document.xml", xml.as_bytes());
        zip.write()
    }

    #[test]
    fn a_word_file_without_heading_styles_is_cut_where_the_person_chose_and_written_back_unchanged() {
        let bytes = styleless_word_file();
        let read = docx::read(&bytes).unwrap();
        assert!(read.paragraphs.iter().all(|p| p.level.is_none() && !p.title));
        assert_eq!(read.paragraphs.iter().filter(|p| p.bold).map(|p| p.text.as_str()).collect::<Vec<_>>(), ["原生质体的制备"]);
        // The guesses, all likely here; taken as they are, each starts a section.
        let draft = from_word(&read, "综述.docx");
        let guessed: Vec<(usize, &str, headings::Sign, bool)> = draft.headings.iter().map(|g| (g.paragraph, g.text.as_str(), g.sign, g.likely)).collect();
        assert_eq!(guessed, [(1, "摘要", headings::Sign::Named, true), (3, "单细胞转录组测序技术的发展（图1）", headings::Sign::Figure, true), (5, "原生质体的制备", headings::Sign::Bold, true), (7, "展望", headings::Sign::Named, true)]);
        assert_eq!((draft.title.as_str(), draft.titled), ("单细胞转录组测序技术在植物发育生物学上的应用及进展", true));
        assert_eq!(draft.sections.iter().map(|s| (s.name.as_str(), s.kind.as_str(), s.paragraphs.len())).collect::<Vec<_>>(), [("摘要", "abstract", 1), ("单细胞转录组测序技术的发展（图1）", "other", 1), ("原生质体的制备", "other", 1), ("展望", "discussion", 1)]);
        // The person keeps two of them: the others stay paragraphs of their section.
        let draft = from_word_with(&read, "综述.docx", Some(&[1, 3]));
        assert_eq!(draft.sections.iter().map(|s| (s.name.as_str(), s.paragraphs.len())).collect::<Vec<_>>(), [("摘要", 1), ("单细胞转录组测序技术的发展（图1）", 5)]);
        // None at all: one section, as before.
        assert_eq!(from_word_with(&read, "综述.docx", Some(&[])).sections.iter().map(|s| (s.name.as_str(), s.bare)).collect::<Vec<_>>(), [("正文", true)]);

        let dir = std::env::temp_dir().join(format!("wisp-manuscript-{}", new_id("T")));
        let mut store = Store::open(&dir).unwrap();
        let made = store.import_manuscript(&draft, "docx", "综述.docx", Some(&bytes)).unwrap();
        let unchanged = |store: &Store| {
            let now = store.manuscript(&made.id).unwrap().unwrap();
            let (out, from_file) = store.manuscript_word(&now, &store.comments(&made.id).unwrap(), true, "Xu", cite::Style::AuthorYear).unwrap();
            assert!(from_file);
            // Every paragraph as it was: no heading style put on the headings guessed, nothing tracked.
            assert_eq!((out.kept, out.changed, out.added, out.removed), (9, 0, 0, 0));
            let read = docx::read(&out.bytes).unwrap();
            assert!(read.paragraphs.iter().all(|p| p.level.is_none()) && !read.tracked);
            assert_eq!(read.paragraphs.iter().map(|p| p.text.clone()).collect::<Vec<_>>(), docx::read(&bytes).unwrap().paragraphs.iter().map(|p| p.text.clone()).collect::<Vec<_>>());
        };
        unchanged(&store);

        // In the editor: 设为标题 on the paragraph in bold, with a comment on words after it.
        let section = made.sections[1].clone();
        store.add_comment(&made.id, &section.id, "酶解", "哪种酶？", "Xu", comment::PERSON).unwrap();
        store.add_comment(&made.id, &section.id, "快速的发展", "多快？", "Xu", comment::PERSON).unwrap();
        let start = section.body.find("原生质体的制备").unwrap();
        let split = store.split_section(&section.id, start..start + "原生质体的制备".len(), Cut::Heading).unwrap();
        assert_eq!((split.name.as_str(), split.kind.as_str(), split.bare), ("原生质体的制备", "other", false));
        let now = store.manuscript(&made.id).unwrap().unwrap();
        assert_eq!(now.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["摘要", "单细胞转录组测序技术的发展（图1）", "原生质体的制备"]);
        assert!(now.sections[1].body.ends_with("成本不断下降。") && split.body.starts_with("植物细胞有细胞壁"));
        let placed: Vec<(String, bool)> = store.comments(&made.id).unwrap().into_iter().map(|c| (c.quote, c.section_id == split.id)).collect();
        assert_eq!(placed, [("酶解".to_string(), true), ("快速的发展".to_string(), false)]);
        assert_eq!(store.snapshots(&section.id).unwrap()[0].why, "split");
        unchanged(&store);

        // Put back, and cut again without a heading: the new section's name goes into no file.
        let merged = store.merge_section(&split.id).unwrap();
        assert_eq!(merged.body, section.body);
        assert!(store.comments(&made.id).unwrap().iter().all(|c| c.section_id == section.id));
        unchanged(&store);
        let start = merged.body.find("植物细胞").unwrap();
        let cut = store.split_section(&merged.id, start..start + 3, Cut::Paragraph).unwrap();
        assert_eq!((cut.name.as_str(), cut.bare, cut.body.as_str()), ("植物细胞有细胞壁，需要先酶解制备…", true, "植物细胞有细胞壁，需要先酶解制备原生质体，这一步骤可能改变部分基因的表达。\n\n展望\n\n随着空间转录组等技术的发展，单细胞水平的研究将更加深入。"));
        unchanged(&store);
        assert!(store.split_section(&made.sections[0].id, 0..0, Cut::Paragraph).is_err() && store.merge_section(&made.sections[0].id).is_err());

        // Taken in without a section: the first paragraph set as a heading names the one there is.
        let whole = store.import_manuscript(&from_word_with(&read, "综述.docx", Some(&[])), "docx", "综述.docx", Some(&bytes)).unwrap();
        let named = store.split_section(&whole.sections[0].id, 0.."摘要".len(), Cut::Heading).unwrap();
        assert_eq!((named.id == whole.sections[0].id, named.name.as_str(), named.kind.as_str(), named.bare, named.body.starts_with("单细胞转录组测序技术能够")), (true, "摘要", "abstract", false, true));
        let now = store.manuscript(&whole.id).unwrap().unwrap();
        let (out, _) = store.manuscript_word(&now, &[], false, "Xu", cite::Style::AuthorYear).unwrap();
        assert_eq!((out.kept, out.changed, out.added, out.removed), (9, 0, 0, 0));
        // A paragraph written in Markdown names the section by its words.
        let typed = store.create_manuscript("Typed", "en").unwrap();
        let first = &typed.sections[0];
        store.save_section_body(&first.id, "Opening words.\n\n**Methods**\n\nWe grew roots.").unwrap();
        let start = "Opening words.\n\n".len();
        let methods = store.split_section(&first.id, start..start + "**Methods**".len(), Cut::Heading).unwrap();
        assert_eq!((methods.name.as_str(), methods.kind.as_str(), methods.body.as_str()), ("Methods", "methods", "We grew roots."));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_docx_is_a_zip_with_the_text_escaped() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let bytes = docx(&sample());
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("word/document.xml") && text.contains("[Content_Types].xml"));
        assert!(text.contains("Roots &amp; &lt;shoots&gt;"));
        assert!(text.contains("<w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t xml:space=\"preserve\">Results"));
        assert!(!text.contains("Methods"));
        // The end of central directory says five entries.
        let end = &bytes[bytes.len() - 22..];
        assert_eq!(&end[..4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([end[10], end[11]]), 5);
    }

    #[test]
    fn markdown_written_in_a_section_is_exported_as_markdown_and_as_formatting() {
        let section = Section { name: "Results".into(), kind: "results".into(), body: "# Roots\nThey **grew** fast.\n- one\n- two\n\n```\nlet x = 1;\n\nlet y = 2;\n```\n---".into(), ..Default::default() };
        let manuscript = Manuscript { title: "T".into(), sections: vec![section], ..Default::default() };
        // The heading goes under the section's; a list and code keep their lines together.
        assert_eq!(markdown(&manuscript), "# T\n\n## Results\n\n### Roots\n\nThey **grew** fast.\n\n- one\n- two\n\n```\nlet x = 1;\n\nlet y = 2;\n```\n\n---\n");
        assert_eq!(plain_text(&manuscript), "T\n\nResults\n\nRoots\n\nThey grew fast.\n\n• one\n\n• two\n\n```\n\nlet x = 1;\n\nlet y = 2;\n\n```\n");
        let text = String::from_utf8_lossy(&docx(&manuscript)).to_string();
        assert!(text.contains("<w:p><w:pPr><w:pStyle w:val=\"Heading2\"/></w:pPr><w:r><w:t xml:space=\"preserve\">Roots</w:t></w:r></w:p>"));
        assert!(text.contains("<w:r><w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">grew</w:t></w:r>"));
        assert!(!text.contains("**") && !text.contains("---"));
    }

    #[test]
    fn a_table_written_in_a_section_keeps_its_rows_together() {
        // What an assistant writes into a section: a line, a table, a line. Every other line is a paragraph of its own.
        let body = "对照如下：\n| 维度 | 顶刊 | 子刊 |\n|---|---|:-:|\n| 数据 | 自造 | 二次挖掘 |\n| 验证 | **有** | 无 |\n一句话：数据决定。\n另一段。";
        let written = section_markdown(body, 2);
        assert_eq!(written, "对照如下：\n\n| 维度 | 顶刊 | 子刊 |\n|---|---|:-:|\n| 数据 | 自造 | 二次挖掘 |\n| 验证 | **有** | 无 |\n\n一句话：数据决定。\n\n另一段。");
        // Read back, it is a table: before, each row was a paragraph of pipes.
        let blocks = markdown::parse(&written);
        assert!(matches!(&blocks[1], markdown::Block::Table { rows, aligns } if rows.len() == 3 && aligns.len() == 3 && rows[2][1].text == "有"), "{blocks:?}");
        assert_eq!(blocks.len(), 4);
        // A line with a pipe and no line of dashes under it is a paragraph; so is a table's line inside fenced code, and a list's item.
        assert_eq!(section_markdown("a | b\nc | d", 2), "a | b\n\nc | d");
        assert_eq!(section_markdown("```\n| a | b |\n|---|---|\n```", 2), "```\n| a | b |\n|---|---|\n```");
        assert_eq!(section_markdown("- a | b\n- --- | ---", 2), "- a | b\n- --- | ---");
        // The exported file has it the same way.
        let manuscript = Manuscript { title: "T".into(), sections: vec![Section { name: "讨论".into(), kind: "discussion".into(), body: body.into(), ..Default::default() }], ..Default::default() };
        assert!(markdown(&manuscript).contains("\n\n| 维度 | 顶刊 | 子刊 |\n|---|---|:-:|\n| 数据 | 自造 | 二次挖掘 |\n| 验证 | **有** | 无 |\n\n"));
    }

    #[test]
    fn a_word_file_cites_for_a_reader_and_lists_the_references() {
        let mut store = Store::in_memory(Path::new(".")).unwrap();
        let paper = store.add_paper(crate::model::Paper { title: "Single-cell atlas of roots".into(), authors: "Zhang, Wei; Li, Qiang; Wang, Fang".into(), year: Some(2024), journal: "Nature Plants".into(), ..Default::default() }).unwrap();
        let made = store.create_manuscript("Roots", "en").unwrap();
        store.save_section_body(&made.sections[1].id, "Roots differ [@Zhang2024 p.2 \"a core set of genes\"] and [@Nobody2001].").unwrap();
        store.keep_cite_key(&paper.id, "Zhang2024").unwrap();
        let now = store.manuscript(&made.id).unwrap().unwrap();
        let said = |style| -> Vec<String> {
            let (out, _) = store.manuscript_word(&now, &[], false, "Xu", style).unwrap();
            docx::read(&out.bytes).unwrap().paragraphs.into_iter().map(|p| p.text).collect()
        };
        assert_eq!(said(cite::Style::AuthorYear), ["Roots", "Introduction", "Roots differ (Zhang et al., 2024) and [@Nobody2001].", "References", "Zhang, W., Li, Q., & Wang, F. (2024). Single-cell atlas of roots. Nature Plants."]);
        assert_eq!(said(cite::Style::Numeric)[2..], ["Roots differ [1] and [@Nobody2001].", "References", "[1] Zhang, W., Li, Q., & Wang, F. (2024). Single-cell atlas of roots. Nature Plants."]);
        // Each citation is a field that jumps to the bookmark on its paper's entry; a journal's style sets its own looks.
        let xml = |style: cite::Style| docx::Zip::read(&store.manuscript_word(&now, &[], false, "Xu", style).unwrap().0.bytes).unwrap().text("word/document.xml").unwrap();
        let numbered = xml(cite::Style::Numeric);
        assert!(numbered.contains(r#"<w:instrText xml:space="preserve"> HYPERLINK \l "_WPREF_1" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t xml:space="preserve">1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve">] and"#), "{numbered}");
        assert!(numbered.contains(r#"<w:bookmarkStart w:id="1" w:name="_WPREF_1"/><w:r><w:t xml:space="preserve">[1] Zhang, W."#) && numbered.contains(r#"<w:bookmarkEnd w:id="1"/></w:p>"#), "{numbered}");
        let nature = cite::Style::Csl(std::sync::Arc::new(crate::csl::load("csl:nature", std::path::Path::new("/nowhere")).unwrap()));
        assert_eq!(said(nature.clone())[2..], ["Roots differ 1 and [@Nobody2001].", "References", "1. Zhang, W., Li, Q. & Wang, F. Single-cell atlas of roots. Nature Plants (2024)."]);
        let set = xml(nature);
        assert!(set.contains(r#"<w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:t xml:space="preserve">1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#), "{set}");
        assert!(set.contains(r#"<w:r><w:rPr><w:i/></w:rPr><w:t xml:space="preserve">Nature Plants</w:t></w:r>"#), "{set}");
        // The manuscript itself keeps its keys.
        assert!(store.manuscript(&made.id).unwrap().unwrap().sections[1].body.contains("[@Zhang2024 p.2"));
    }

    #[test]
    fn a_table_written_in_a_section_is_a_table_of_the_word_file() {
        let mut store = Store::in_memory(Path::new(".")).unwrap();
        let paper = store.add_paper(crate::model::Paper { title: "Single-cell atlas of roots".into(), authors: "Zhang, Wei; Li, Qiang; Wang, Fang".into(), year: Some(2024), journal: "Nature Plants".into(), ..Default::default() }).unwrap();
        let made = store.create_manuscript("Roots", "en").unwrap();
        let body = "Compared:\n| Tissue | Finding | n |\n|:--|:-:|--:|\n| **Root** | *deep* [@Zhang2024] | 12 |\n| Shoot | light<br>`fast` | |\nAfter the table.";
        store.save_section_body(&made.sections[1].id, body).unwrap();
        store.keep_cite_key(&paper.id, "Zhang2024").unwrap();
        store.add_comment(&made.id, &made.sections[1].id, "light", "How much?", "Xu", comment::PERSON).unwrap();
        let now = store.manuscript(&made.id).unwrap().unwrap();
        let notes = store.comments(&made.id).unwrap();
        let (out, _) = store.manuscript_word(&now, &notes, false, "Xu", cite::Style::AuthorYear).unwrap();
        // No paragraph of pipes: the lines are one table between the paragraphs around them, and the paper a cell cites is listed.
        let read = docx::read(&out.bytes).unwrap();
        assert_eq!(read.paragraphs.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["Roots", "Introduction", "Compared:", "After the table.", "References", "Zhang, W., Li, Q., & Wang, F. (2024). Single-cell atlas of roots. Nature Plants."]);
        assert_eq!((read.tables, out.tables, out.comments), (1, 1, 1));
        let xml = docx::Zip::read(&out.bytes).unwrap().text("word/document.xml").unwrap();
        let (start, end) = (xml.find("<w:tbl>").unwrap(), xml.find("</w:tbl>").unwrap());
        assert!(xml.find("Compared:").unwrap() < start && end < xml.find("After the table.").unwrap() && !xml.contains('|'), "{xml}");
        let table = &xml[start..end];
        assert_eq!((table.matches("<w:tr>").count(), table.matches("<w:tc>").count(), table.matches("<w:gridCol ").count()), (3, 9, 3));
        // The header is set apart and said again on every page; a column is set to the side its line of dashes names.
        let cell = |side: &str, look: &str, text: &str| format!("<w:jc w:val=\"{side}\"/></w:pPr><w:r>{look}<w:t xml:space=\"preserve\">{text}</w:t></w:r>");
        assert!(table.starts_with("<w:tbl><w:tblPr><w:tblW w:w=\"9026\" w:type=\"dxa\"/><w:tblBorders><w:top w:val=\"single\""), "{table}");
        assert_eq!(table.matches("<w:tr><w:trPr><w:tblHeader/></w:trPr>").count(), 1);
        for (side, text) in [("left", "Tissue"), ("center", "Finding"), ("right", "n")] { assert!(table.contains(&cell(side, "<w:rPr><w:b/></w:rPr>", text)), "{text}: {table}") }
        assert!(table.contains(&cell("left", "<w:rPr><w:b/></w:rPr>", "Root")) && table.contains(&cell("center", "<w:rPr><w:i/></w:rPr>", "deep")) && table.contains(&cell("right", "", "12")), "{table}");
        // A citation in a cell is written as one in a paragraph is: for a reader, and leading to its entry.
        assert!(table.contains(r#"<w:instrText xml:space="preserve"> HYPERLINK \l "_WPREF_1" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t xml:space="preserve">Zhang et al., 2024</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve">)</w:t></w:r></w:p></w:tc>"#), "{table}");
        // `<br>` is a paragraph of its own in the cell, code is set as code, and a comment stands on a cell's words.
        assert!(table.contains("<w:commentRangeStart w:id=\"1\"/><w:r><w:t xml:space=\"preserve\">light</w:t></w:r><w:commentRangeEnd w:id=\"1\"/><w:r><w:commentReference w:id=\"1\"/></w:r></w:p><w:p>") && table.contains(&cell("center", &format!("<w:rPr><w:rFonts w:ascii=\"{0}\" w:hAnsi=\"{0}\"/></w:rPr>", docx::MONO), "fast")), "{table}");
        // A cell without words still holds a paragraph, and the columns are as wide as the page's text.
        assert!(table.ends_with("<w:jc w:val=\"right\"/></w:pPr></w:p></w:tc></w:tr>"), "{table}");
        let wide: u32 = table.match_indices("<w:gridCol w:w=\"").map(|(at, _)| table[at + 16..].split('"').next().unwrap().parse::<u32>().unwrap()).sum();
        assert_eq!(wide, 9026);
        // A journal's style raises its number in a cell too.
        let nature = cite::Style::Csl(std::sync::Arc::new(crate::csl::load("csl:nature", std::path::Path::new("/nowhere")).unwrap()));
        let raised = docx::Zip::read(&store.manuscript_word(&now, &[], false, "Xu", nature).unwrap().0.bytes).unwrap().text("word/document.xml").unwrap();
        assert!(raised[raised.find("<w:tbl>").unwrap()..raised.find("</w:tbl>").unwrap()].contains(r#"<w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:t xml:space="preserve">1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#), "{raised}");
        // The document made without a library says the table too, its citations as written; two tables stay two, and none ends the document.
        let alone = String::from_utf8_lossy(&docx(&Manuscript { title: "T".into(), sections: vec![Section { name: "Results".into(), body: format!("{body}\n| a | b |\n|---|---|\n\n| c | d |\n|---|---|"), ..Default::default() }], ..Default::default() })).to_string();
        assert!(alone.matches("<w:tbl>").count() == 3 && alone.contains("[@Zhang2024]") && alone.contains("</w:tbl><w:p/><w:tbl>") && alone.contains("</w:tbl><w:p/><w:sectPr>") && !alone.contains("| a"), "{alone}");
    }

    #[test]
    fn a_table_written_into_a_manuscript_from_word_goes_into_its_file() {
        let dir = std::env::temp_dir().join(format!("wisp-manuscript-{}", new_id("T")));
        let mut store = Store::open(&dir).unwrap();
        // A file with a table of its own between two paragraphs, its text in a font of its own.
        let font = "<w:rPr><w:rFonts w:ascii=\"Georgia\"/></w:rPr>";
        let theirs = "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>theirs</w:t></w:r></w:p></w:tc></w:tr></w:tbl>";
        let paras = [
            "<w:p><w:pPr><w:pStyle w:val=\"a3\"/></w:pPr><w:r><w:t>Roots</w:t></w:r></w:p>".to_string(),
            "<w:p><w:pPr><w:pStyle w:val=\"1\"/></w:pPr><w:r><w:t>Introduction</w:t></w:r></w:p>".to_string(),
            format!("<w:p><w:pPr><w:ind w:firstLine=\"420\"/></w:pPr><w:r>{font}<w:t xml:space=\"preserve\">Roots grow </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>down</w:t></w:r><w:r>{font}<w:t>.</w:t></w:r></w:p>"),
            format!("<w:p><w:r>{font}<w:t>They take up water.</w:t></w:r></w:p>"),
            "<w:p><w:pPr><w:pStyle w:val=\"1\"/></w:pPr><w:r><w:t>Methods</w:t></w:r></w:p>".to_string(),
            format!("<w:p><w:r>{font}<w:t>We grew plants.</w:t></w:r></w:p>"),
        ];
        let bytes = docx::tests::file(&format!("{}{}{}{theirs}{}{}{}", paras[0], paras[1], paras[2], paras[3], paras[4], paras[5]), "");
        let made = store.import_manuscript(&from_word(&docx::read(&bytes).unwrap(), "Roots.docx"), "docx", "Roots.docx", Some(&bytes)).unwrap();
        assert_eq!(made.sections.iter().map(|s| s.body.as_str()).collect::<Vec<_>>(), ["Roots grow down.\n\nThey take up water.", "We grew plants."]);
        // A table written after the first paragraph, another at the end of the last section, and a comment on a cell's words.
        store.save_section_body(&made.sections[0].id, "Roots grow down.\n\n| Depth | Share |\n|---|--:|\n| *deep* soil | 12 |\n\nThey take up water.").unwrap();
        store.save_section_body(&made.sections[1].id, "We grew plants.\n| Pot | Plants |\n|---|---|\n| 1 | 4 |").unwrap();
        store.add_comment(&made.id, &made.sections[0].id, "soil", "Which soil?", "Xu", comment::PERSON).unwrap();
        let now = store.manuscript(&made.id).unwrap().unwrap();
        let (out, from_file) = store.manuscript_word(&now, &store.comments(&made.id).unwrap(), false, "Xu", cite::Style::AuthorYear).unwrap();
        assert!(from_file);
        // Every paragraph of the file is as it was, to the byte, and so is its table.
        assert_eq!((out.kept, out.changed, out.added, out.removed, out.tables, out.comments), (6, 0, 0, 0, 2, 1));
        let body = docx::tests::body_of(&out.bytes);
        let mut at = 0;
        for piece in [paras[0].as_str(), &paras[1], &paras[2], "<w:tbl><w:tblPr>", "</w:tbl><w:p/>", theirs, &paras[3], &paras[4], &paras[5], "<w:tbl><w:tblPr>"] {
            at += body[at..].find(piece).unwrap_or_else(|| panic!("{piece} after {at}: {body}")) + piece.len();
        }
        // The table at the document's end has a paragraph after it; a table of the file and one made here that touch are kept apart.
        assert!(body.ends_with("</w:tbl><w:p/>") && body.matches("<w:p/>").count() == 2, "{body}");
        let read = docx::read(&out.bytes).unwrap();
        assert_eq!((read.tables, read.paragraphs.iter().map(|p| p.text.as_str()).collect::<Vec<_>>()), (3, vec!["Roots", "Introduction", "Roots grow down.", "They take up water.", "Methods", "We grew plants."]));
        // A cell's words are set like the text around the table, without that text's first-line indent, and the comment is on them.
        let table = &body[body.find("<w:tbl><w:tblPr>").unwrap()..body.find("</w:tbl>").unwrap()];
        assert!(table.contains("<w:r><w:rPr><w:rFonts w:ascii=\"Georgia\"/><w:b/></w:rPr><w:t xml:space=\"preserve\">Depth</w:t></w:r>") && table.contains("<w:r><w:rPr><w:rFonts w:ascii=\"Georgia\"/><w:i/></w:rPr><w:t xml:space=\"preserve\">deep</w:t></w:r>"), "{table}");
        assert!(table.contains("w:firstLine=\"0\"") && table.contains("<w:commentRangeStart w:id=\"1\"/><w:r><w:rPr><w:rFonts w:ascii=\"Georgia\"/></w:rPr><w:t xml:space=\"preserve\">soil</w:t></w:r><w:commentRangeEnd w:id=\"1\"/>"), "{table}");
        assert_eq!(read.comments.iter().map(|c| (c.author.as_str(), c.body.as_str())).collect::<Vec<_>>(), [("Xu", "Which soil?")]);

        // As tracked changes, each row is one put in, and so are the words in its cells.
        let (tracked, _) = store.manuscript_word(&now, &[], true, "Xu", cite::Style::AuthorYear).unwrap();
        let body = docx::tests::body_of(&tracked.bytes);
        assert_eq!((body.matches("<w:tr><w:trPr>").count(), body.matches("<w:trPr><w:tblHeader/><w:ins w:id=").count(), body.matches("<w:trPr><w:ins w:id=").count()), (4, 2, 2), "{body}");
        assert!(body.contains("<w:ins w:id=\"7\" w:author=\"Xu\"") && body.contains(&paras[2]) && body.contains(theirs) && docx::read(&tracked.bytes).unwrap().tracked, "{body}");
        // The manuscript taken in again from the file written has its tables in the file, not in its text.
        let again = from_word(&docx::read(&out.bytes).unwrap(), "Roots.docx");
        assert_eq!(again.sections.iter().map(|s| s.paragraphs.join("\n\n")).collect::<Vec<_>>(), ["Roots grow down.\n\nThey take up water.", "We grew plants."]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_markdown_file_brings_its_pictures_into_the_figure_library() {
        let root = std::env::temp_dir().join(format!("wisp-manuscript-beside-{}", new_id("T")));
        let (library, project) = (root.join("library"), root.join("project"));
        std::fs::create_dir_all(&library).unwrap();
        std::fs::create_dir_all(project.join("figures")).unwrap();
        // A PNG's first bytes, saying how wide it is.
        let png = |width: u32| {
            let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
            png.extend(width.to_be_bytes());
            png.extend(100u32.to_be_bytes());
            png.extend([8, 2, 0, 0, 0]);
            png
        };
        std::fs::write(project.join("figures/fig1.png"), png(300)).unwrap();
        std::fs::write(project.join("figures/fig 2.png"), png(400)).unwrap();
        std::fs::write(project.join("figures/fig3.svg"), "<svg/>").unwrap();
        let text = "# Salt and roots\n\n## Results\n\nRoots are shorter under salt.\n![Root length under **salt**](figures/fig1.png)\n**Figure 1.** Root length.\n\n![](./figures/fig%202.png){width=80%}\n\n![again](figures/fig1.png)\n\n![vector](figures/fig3.svg)\n\n![gone](figures/none.png)\n\n![web](https://example.org/a.png)\n";
        let mut draft = from_markdown(text, "draft.md");
        // A picture is a paragraph of its own whatever stands right over and under it, and without what Pandoc is told of it.
        assert_eq!((draft.sections[0].name.as_str(), &draft.sections[0].paragraphs[..4]), ("Results", &["Roots are shorter under salt.".to_string(), "![Root length under **salt**](figures/fig1.png)".into(), "**Figure 1.** Root length.".into(), "![](./figures/fig%202.png)".into()][..]));
        let pictures = read_pictures(&draft, &project, &|_| None);
        let read: Vec<(&str, &str, Result<usize, Left>)> = pictures.iter().map(|picture| (picture.address.as_str(), picture.name.as_str(), picture.bytes.as_ref().map(Vec::len).map_err(|why| *why))).collect();
        assert_eq!(read, [("figures/fig1.png", "fig1.png", Ok(29)), ("./figures/fig%202.png", "fig 2.png", Ok(29)), ("figures/fig3.svg", "fig3.svg", Err(Left::Kind)), ("figures/none.png", "", Err(Left::Missing)), ("https://example.org/a.png", "", Err(Left::Link))]);
        // A kind that is not kept comes where the caller writes it anew.
        assert_eq!(read_pictures(&draft, &project, &|bytes| (bytes == b"<svg/>").then(|| png(500)))[2].bytes, Ok(png(500)));

        let mut store = Store::in_memory(&library).unwrap();
        let brought = store.bring_pictures(&mut draft, pictures).unwrap();
        assert_eq!((brought.added, brought.known, brought.shown(), brought.left.clone()), (2, 0, 2, vec![("figures/fig3.svg".to_string(), Left::Kind), ("figures/none.png".to_string(), Left::Missing), ("https://example.org/a.png".to_string(), Left::Link)]));
        let figures = store.list_figures().unwrap();
        let (first, second) = (figures.iter().find(|figure| figure.file_name == "fig1.png").unwrap(), figures.iter().find(|figure| figure.file_name == "fig 2.png").unwrap());
        assert!(first.uploaded() && first.legend == "Root length under salt" && (first.width, first.height) == (300, 100) && second.legend.is_empty(), "{figures:?}");
        assert_eq!(std::fs::read(store.figure_path(second)).unwrap(), png(400));
        // The lines show them from the library, captions as written; what did not come stays as the file has it.
        let body = draft.sections[0].paragraphs.join("\n\n");
        assert!(body.contains(&format!("![Root length under **salt**]({})", first.local_path)), "{body}");
        assert_eq!(picture_lines(&body).into_iter().map(|picture| picture.address).collect::<Vec<_>>(), [first.local_path.as_str(), second.local_path.as_str(), first.local_path.as_str(), "figures/fig3.svg", "figures/none.png", "https://example.org/a.png"]);
        let made = store.import_manuscript(&draft, "markdown", "draft.md", None).unwrap();
        assert_eq!(pictures_used(&made)[..2], [first.local_path.clone(), second.local_path.clone()]);

        // Read again, the file is the manuscript but for the addresses, and its pictures are the ones held.
        let mut again = from_markdown(text, "draft.md");
        assert_eq!(without_addresses(&made.sections[0].body), without_addresses(&again.sections[0].paragraphs.join("\n\n")));
        assert_ne!(made.sections[0].body, again.sections[0].paragraphs.join("\n\n"));
        let pictures = read_pictures(&again, &project, &|_| None);
        let brought = store.bring_pictures(&mut again, pictures).unwrap();
        assert_eq!((brought.added, brought.known, store.list_figures().unwrap().len()), (0, 2, 2));
        assert_eq!(again.sections[0].paragraphs, draft.sections[0].paragraphs);
        // A line the library's own export wrote, its picture not beside the file: the figure is here, and nothing is said of it.
        let mut own = from_markdown(&format!("## Results\n\n![kept]({})\n", first.local_path), "export.md");
        let pictures = read_pictures(&own, &root, &|_| None);
        assert_eq!(pictures[0].bytes, Err(Left::Missing));
        assert_eq!(store.bring_pictures(&mut own, pictures).unwrap(), Brought::default());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn pictures_stand_in_the_manuscript_and_in_its_word_file() {
        let root = std::env::temp_dir().join(format!("wisp-manuscript-pictures-{}", new_id("T")));
        std::fs::create_dir_all(&root).unwrap();
        let mut store = Store::in_memory(&root).unwrap();
        let paper = store.add_paper(crate::model::Paper { title: "Single-cell atlas of roots".into(), authors: "Zhang, Wei".into(), year: Some(2024), ..Default::default() }).unwrap();
        store.keep_cite_key(&paper.id, "Zhang2024").unwrap();
        // A PNG's first bytes, saying it is 2000 by 1000: wider than a page.
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
        png.extend(2000u32.to_be_bytes());
        png.extend(1000u32.to_be_bytes());
        png.extend([8, 2, 0, 0, 0]);
        let mut own = crate::figure::Figure { origin: crate::figure::UPLOADED.into(), file_name: "fig1.png".into(), ..Default::default() };
        store.add_figure(&mut own, &png).unwrap();
        let mut second = crate::figure::Figure { origin: crate::figure::UPLOADED.into(), ..Default::default() };
        store.add_figure(&mut second, &png).unwrap();

        let line = picture_markdown("Root  length\nunder salt [@Zhang2024].", &own.local_path);
        assert_eq!(line, format!("![Root length under salt [@Zhang2024].]({})", own.local_path));
        assert_eq!(picture_markdown("a](b", "figures/x.png"), "![a] (b](figures/x.png)");
        assert!(library_picture(&own.local_path) && !library_picture("figures/../library.json") && !library_picture("papers/a.png") && !library_picture("figures/") && !library_picture("https://example.org/a.png"));

        // The line goes after the paragraph at the caret, a blank line apart from its neighbours.
        let put = |text: &str, caret: usize| {
            let (at, words, _) = place_picture(text, caret, "![](p.png)");
            format!("{}{words}{}", &text[..at], &text[at..])
        };
        assert_eq!(put("", 0), "![](p.png)");
        assert_eq!(put("One. Two.\n\nThree.", 3), "One. Two.\n\n![](p.png)\n\nThree.");
        assert_eq!(put("One.\nTwo.", 9), "One.\nTwo.\n\n![](p.png)");
        assert_eq!(put("One.\n\n\n\nTwo.", 5), "One.\n\n![](p.png)\n\n\nTwo.");
        assert_eq!(put("根尖。\n", 99), "根尖。\n\n![](p.png)");
        assert_eq!(place_picture("One.", 2, "![](p.png)").2, 2);

        let made = store.create_manuscript("Roots", "en").unwrap();
        let body = format!("Roots grow.\n\n{line}\n\n![]({})\n\n![Gone.](figures/G-gone.png)\n\n```\n![code](figures/x.png)\n```\n\nThey take up water.", second.local_path);
        store.save_section_body(&made.sections[1].id, &body).unwrap();
        let now = store.manuscript(&made.id).unwrap().unwrap();
        let read = picture_lines(&body);
        assert_eq!(read.iter().map(|picture| (picture.caption.as_str(), picture.address.as_str())).collect::<Vec<_>>(), vec![("Root length under salt [@Zhang2024].", own.local_path.as_str()), ("", second.local_path.as_str()), ("Gone.", "figures/G-gone.png")]);
        assert_eq!(&body[read[0].line.clone()], line);
        assert_eq!(pictures_used(&now), vec![own.local_path.clone(), second.local_path.clone(), "figures/G-gone.png".to_string()]);
        // Copied as text, a picture is its caption; one without says nothing. The Markdown keeps the lines as written.
        assert!(plain_text(&now).contains("Roots grow.\n\nRoot length under salt [@Zhang2024].\n\nGone.\n\n```"));
        assert!(markdown(&now).contains(&format!("{line}\n\n![]({})", second.local_path)));

        let (out, _) = store.manuscript_word(&now, &[], false, "Xu", cite::Style::AuthorYear).unwrap();
        assert_eq!((out.pictures, out.left_out.clone()), (2, vec!["figures/G-gone.png".to_string()]));
        // The caption is a paragraph, cited as the rest; the picture without one, and the address, say nothing.
        let said: Vec<String> = docx::read(&out.bytes).unwrap().paragraphs.into_iter().map(|p| p.text).collect();
        assert_eq!(said[..5], ["Roots", "Introduction", "Roots grow.", "Root length under salt (Zhang, 2024).", "Gone."]);
        assert!(!said.iter().any(|text| text.contains("figures/G")) && said[5..8] == ["```", "![code](figures/x.png)", "```"], "{said:?}");
        let zip = docx::Zip::read(&out.bytes).unwrap();
        let xml = zip.text("word/document.xml").unwrap();
        // Each picture is a paragraph of its own: the first over its caption, the second under it, scaled to the page.
        let (first, caption, second_at) = (xml.find("r:embed=\"rIdWisp1\"").unwrap(), xml.find("Root length under salt").unwrap(), xml.find("r:embed=\"rIdWisp2\"").unwrap());
        assert!(first < caption && caption < second_at && second_at < xml.find("Gone.").unwrap(), "{xml}");
        assert!(xml.contains("<wp:extent cx=\"5486400\" cy=\"2743200\"/>"), "{xml}");
        assert_eq!((zip.get("word/media/wisp1.png").unwrap(), zip.has("word/media/wisp2.png")), (Some(png.clone()), true));
        assert!(zip.text("word/_rels/document.xml.rels").unwrap().contains("Id=\"rIdWisp2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/wisp2.png\""));
        assert!(zip.text("[Content_Types].xml").unwrap().contains("<Default Extension=\"png\" ContentType=\"image/png\"/>"));

        // A manuscript that came from a Word file gets the pictures put into that file.
        let draft = from_word(&docx::read(&word_file()).unwrap(), "roots.docx");
        let taken = store.import_manuscript(&draft, "docx", "roots.docx", Some(&word_file())).unwrap();
        store.save_section_body(&taken.sections[0].id, &format!("Roots grow down into the soil.\n\n![Our roots.]({})\n\nThey take up water.", own.local_path)).unwrap();
        let (out, from_file) = store.manuscript_word(&store.manuscript(&taken.id).unwrap().unwrap(), &[], false, "Xu", cite::Style::AuthorYear).unwrap();
        assert!(from_file && (out.pictures, out.added, out.changed, out.removed) == (1, 1, 0, 0) && out.left_out.is_empty());
        let xml = docx::Zip::read(&out.bytes).unwrap().text("word/document.xml").unwrap();
        assert!(xml.find("into the soil").unwrap() < xml.find("<w:drawing>").unwrap() && xml.find("<w:drawing>").unwrap() < xml.find("Our roots.").unwrap() && xml.find("Our roots.").unwrap() < xml.find("They take up water.").unwrap(), "{xml}");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_word_paragraph_keeps_its_characters() {
        for said in ["Plain words.", "* p < 0.05 for all", "#3 was the best", "1. Not a list in Word", "a_b and *c* and `d` [x](https://e.org)", "---", "> not quoted"] {
            let kept = escape_markdown(said);
            assert_eq!(markdown::plain(&kept), said, "{kept}");
        }
        assert_eq!(escape_markdown("Plain words."), "Plain words.");
    }
}
