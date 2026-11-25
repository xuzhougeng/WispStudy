//! 调整: the few details of a citation style most often asked to be otherwise, read from a CSL style and
//! written back into it. How a numbered style sets its number in the text (in brackets, raised), from how
//! many authors on a citation or an entry of the list says "et al." and how many it names first, and what
//! stands after an initial.
//!
//! A change is a change to the style's own XML, attribute by attribute, the rest of the file as it was
//! written: no second way of saying what a style is. What comes of it is a CSL file like any other, which the
//! person keeps beside the built-in ones (`keep`, in `styles/` of the data folder) and Zotero reads as well.
//!
//! A style says these things in more than one place (IEEE's brackets are on a group around the number, its
//! "et al." on the name itself), so `read` looks where the common styles have them and `knobs` then offers
//! only what shows: a detail whose change leaves the style's sample as it was is not offered. Anything else
//! about a style is for the assistant, which changes the same file (`read_cite_style`, `try_cite_style`,
//! `save_cite_style`).

use crate::cite;
use crate::csl::{self, Listed};
use crate::docx::{Node, node, unescape};
use crate::manuscript::xml_escape;
use anyhow::Result;
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

/// The namespace a CSL file names on its `<style>`; Zotero takes no file without it.
const NAMESPACE: &str = "http://purl.org/net/xbiblio/csl";

/// The comment that names the style a copy was made from (`copied`), by its key.
const MARK: &str = "<!-- Wisp Study: adjusted from ";

/// What stands around the number of a numbered style in the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brackets {
    /// `[1]`
    Square,
    /// `(1)`
    Round,
    /// `1`, which a style then raises.
    Bare,
}

impl Brackets {
    fn pair(self) -> Option<(&'static str, &'static str)> {
        match self {
            Brackets::Square => Some(("[", "]")),
            Brackets::Round => Some(("(", ")")),
            Brackets::Bare => None,
        }
    }

    fn of(prefix: Option<&str>, suffix: Option<&str>) -> Option<Brackets> {
        match (prefix, suffix) {
            (Some("["), Some("]")) => Some(Brackets::Square),
            (Some("("), Some(")")) => Some(Brackets::Round),
            _ => None,
        }
    }
}

/// When a list of authors is cut short: with `min` of them or more, the first `first` and "et al.". A `min`
/// of 0 is never.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cut {
    pub min: u32,
    pub first: u32,
}

/// The details of a style that can be changed here. None: the style has no such thing (a number, in a style
/// that cites by author and year) or says it in a way not known here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Knobs {
    /// A numbered style: what stands around the number in the text.
    pub brackets: Option<Brackets>,
    /// And whether the number is raised.
    pub raised: Option<bool>,
    /// A style that cites by author and year: when a citation says "et al.".
    pub cited: Option<Cut>,
    /// When an entry of the list does.
    pub listed: Option<Cut>,
    /// What stands after an initial in the list: `. ` (W. F.), `.` (W.F.), a space (W F) or nothing (WF).
    pub initials: Option<String>,
}

// ---- the XML as it is written

/// A tag that opens an element: the element's name, the tag as written, where it starts, and the tag of the
/// element it stands in (its place among the tags).
struct Tag<'a> {
    name: &'a str,
    text: &'a str,
    at: usize,
    parent: Option<usize>,
}

fn tags(xml: &str) -> Vec<Tag<'_>> {
    let (mut out, mut open, mut at): (Vec<Tag>, Vec<usize>, usize) = (vec![], vec![], 0);
    while let Some((piece, end)) = node(xml, at) {
        match piece {
            Node::Open(name, text) => {
                out.push(Tag { name, text, at, parent: open.last().copied() });
                open.push(out.len() - 1);
            }
            Node::Empty(name, text) => out.push(Tag { name, text, at, parent: open.last().copied() }),
            Node::Close => {
                open.pop();
            }
            Node::Text(_) => {}
        }
        at = end;
    }
    out
}

/// The attributes of a tag as written: each one's name, where it stands in the tag (from its name to the
/// quote that closes its value) and its value as written.
fn attributes(tag: &str) -> Vec<(&str, Range<usize>, &str)> {
    let bytes = tag.as_bytes();
    let mut out = vec![];
    // Past `<` and the element's name.
    let mut at = tag.find(|c: char| c.is_whitespace() || c == '/' || c == '>').unwrap_or(tag.len());
    loop {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() { at += 1 }
        let start = at;
        while at < bytes.len() && !bytes[at].is_ascii_whitespace() && !matches!(bytes[at], b'=' | b'/' | b'>') { at += 1 }
        if at == start { return out }
        let name = &tag[start..at];
        while at < bytes.len() && bytes[at].is_ascii_whitespace() { at += 1 }
        if bytes.get(at) != Some(&b'=') { continue }
        at += 1;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() { at += 1 }
        let Some(quote) = bytes.get(at).copied().filter(|b| matches!(b, b'"' | b'\'')) else { return out };
        let Some(end) = tag[at + 1..].find(quote as char).map(|end| at + 1 + end) else { return out };
        out.push((name, start..end + 1, &tag[at + 1..end]));
        at = end + 1;
    }
}

/// What a tag says for an attribute.
fn said(tag: &str, name: &str) -> Option<String> {
    attributes(tag).into_iter().find(|(said, ..)| *said == name).map(|(_, _, value)| unescape(value))
}

/// A tag as written, with this attribute saying `value`, or without it.
fn with(tag: &str, name: &str, value: Option<&str>) -> String {
    let found = attributes(tag).into_iter().find(|(said, ..)| *said == name).map(|(_, range, _)| range);
    let wanted = value.map(|value| format!("{name}=\"{}\"", xml_escape(value)));
    match (found, wanted) {
        (Some(range), Some(wanted)) => format!("{}{wanted}{}", &tag[..range.start], &tag[range.end..]),
        // With the space that stood before it.
        (Some(range), None) => format!("{}{}", tag[..range.start].trim_end(), &tag[range.end..]),
        (None, Some(wanted)) => {
            let end = tag.len() - if tag.ends_with("/>") { 2 } else { 1 };
            format!("{} {wanted}{}", tag[..end].trim_end(), &tag[end..])
        }
        (None, None) => tag.to_string(),
    }
}

/// `xml` with some of its tags written anew: each by its place among `tags`.
fn spliced(xml: &str, tags: &[Tag], changed: &BTreeMap<usize, String>) -> String {
    let (mut out, mut at) = (String::with_capacity(xml.len() + 128), 0);
    for (i, tag) in changed {
        out.push_str(&xml[at..tags[*i].at]);
        out.push_str(tag);
        at = tags[*i].at + tags[*i].text.len();
    }
    out.push_str(&xml[at..]);
    out
}

// ---- where a style says these things

/// The tags of a style that say what `Knobs` holds.
struct Places {
    root: Option<usize>,
    citation: Option<usize>,
    bibliography: Option<usize>,
    /// A numbered style: the layout of its citation, the groups around the number and the number, outermost first.
    number: Vec<usize>,
    /// The names as a style writes them (`<name>` in `<names>`): those of the list first, which are not the
    /// short form a citation in the text uses.
    names: Vec<usize>,
}

impl Places {
    fn of(tags: &[Tag]) -> Places {
        let root = tags.iter().position(|tag| tag.name == "style" && tag.parent.is_none());
        let child = |name: &str| tags.iter().position(|tag| tag.name == name && tag.parent.is_some() && tag.parent == root);
        let (citation, bibliography) = (child("citation"), child("bibliography"));
        // The number as the citation writes it, and what stands around it up to the citation's layout.
        let mut number = vec![];
        // Only a number the citation's own layout holds: one written through a macro, or only in the list, is not known here.
        let cited = |mut i: usize| {
            while let Some(parent) = tags[i].parent {
                if Some(parent) == citation { return true }
                i = parent;
            }
            false
        };
        let mut at = tags.iter().enumerate().position(|(i, tag)| tag.name == "text" && said(tag.text, "variable").as_deref() == Some("citation-number") && cited(i));
        while let Some(i) = at {
            number.push(i);
            if tags[i].name == "layout" { break }
            at = tags[i].parent;
        }
        if number.last().is_none_or(|&i| tags[i].name != "layout") { number.clear() }
        number.reverse();
        let mut names: Vec<usize> = tags.iter().enumerate().filter(|(_, tag)| tag.name == "name" && tag.parent.is_some_and(|parent| tags[parent].name == "names")).map(|(i, _)| i).collect();
        names.sort_by_key(|&i| said(tags[i].text, "form").as_deref() == Some("short"));
        Places { root, citation, bibliography, number, names }
    }
}

/// When the tag cuts a list of names short, if it says.
fn cut_of(tag: &str) -> Option<Cut> {
    let min = said(tag, "et-al-min")?.trim().parse().ok()?;
    Some(Cut { min, first: said(tag, "et-al-use-first").and_then(|first| first.trim().parse().ok()).unwrap_or(1) })
}

/// The details of a style as its XML says them, wherever the common styles say them. `knobs` is this, without
/// what does not show.
pub fn read(xml: &str) -> Knobs {
    let tags = tags(xml);
    let places = Places::of(&tags);
    let text = |i: usize| tags[i].text;
    let long = || places.names.iter().copied().filter(|&i| said(text(i), "form").as_deref() != Some("short"));
    let mut knobs = Knobs::default();
    if !places.number.is_empty() {
        knobs.brackets = Some(places.number.iter().find_map(|&i| Brackets::of(said(text(i), "prefix").as_deref(), said(text(i), "suffix").as_deref())).unwrap_or(Brackets::Bare));
        knobs.raised = Some(places.number.iter().any(|&i| said(text(i), "vertical-align").as_deref() == Some("sup")));
    } else if let Some(citation) = places.citation {
        knobs.cited = Some(cut_of(text(citation)).or(places.root.and_then(|root| cut_of(text(root)))).unwrap_or_default());
    }
    if let Some(bibliography) = places.bibliography {
        knobs.listed = Some([bibliography].into_iter().chain(long()).chain(places.root).find_map(|i| cut_of(text(i))).unwrap_or_default());
        knobs.initials = long().chain([bibliography]).chain(places.root).find_map(|i| said(text(i), "initialize-with"));
    }
    knobs
}

/// `xml` with the details of `wanted` that it does not have yet: each written where the style says it, and
/// nothing else of the file touched. A detail that is none, or as the style has it, changes nothing.
pub fn write(xml: &str, wanted: &Knobs) -> String {
    let now = read(xml);
    let tags = tags(xml);
    let places = Places::of(&tags);
    let mut changed: BTreeMap<usize, String> = BTreeMap::new();
    let mut set = |i: usize, name: &str, value: Option<&str>| {
        let tag = with(changed.get(&i).map_or(tags[i].text, String::as_str), name, value);
        changed.insert(i, tag);
    };
    let has = |i: usize, name: &str| said(tags[i].text, name);
    if let (Some(brackets), Some(was), Some(&layout)) = (wanted.brackets, now.brackets, places.number.first()) && brackets != was {
        // The brackets go from wherever they are (IEEE's on a group around each number) to the layout, around the whole citation.
        for &i in &places.number {
            // The layout's own are written over where they stand.
            if (i != layout || brackets == Brackets::Bare) && Brackets::of(has(i, "prefix").as_deref(), has(i, "suffix").as_deref()).is_some() {
                set(i, "prefix", None);
                set(i, "suffix", None);
            }
        }
        if let Some((open, close)) = brackets.pair() {
            set(layout, "prefix", Some(open));
            set(layout, "suffix", Some(close));
        }
    }
    if let (Some(raised), Some(was), Some(&layout)) = (wanted.raised, now.raised, places.number.first()) && raised != was {
        for &i in &places.number {
            if has(i, "vertical-align").as_deref() == Some("sup") { set(i, "vertical-align", None) }
        }
        if raised { set(layout, "vertical-align", Some("sup")) }
    }
    let mut cut = |i: usize, cut: Cut| {
        let (min, first) = (cut.min.to_string(), cut.first.clamp(1, cut.min.max(2) - 1).to_string());
        set(i, "et-al-min", (cut.min > 0).then_some(min.as_str()));
        set(i, "et-al-use-first", (cut.min > 0).then_some(first.as_str()));
        // A later citation of the same work is cut as the first is.
        set(i, "et-al-subsequent-min", None);
        set(i, "et-al-subsequent-use-first", None);
    };
    if let (Some(cited), Some(was), Some(citation)) = (wanted.cited, now.cited, places.citation) && cited != was { cut(citation, cited) }
    if let (Some(listed), Some(was), Some(bibliography)) = (wanted.listed, now.listed, places.bibliography) && listed != was {
        cut(bibliography, listed);
        // A name that says for itself when it is cut (IEEE) follows the list from now on.
        for &i in &places.names {
            if has(i, "form").as_deref() != Some("short") && has(i, "et-al-min").is_some() {
                set(i, "et-al-min", None);
                set(i, "et-al-use-first", None);
            }
        }
    }
    if let (Some(initials), Some(was)) = (wanted.initials.as_deref(), now.initials.as_deref()) && initials != was {
        // Everywhere the style says it: its names, the list, the citation and the style as a whole.
        for i in places.names.iter().copied().chain(places.bibliography).chain(places.citation).chain(places.root) {
            if has(i, "initialize-with").is_some() { set(i, "initialize-with", Some(initials)) }
        }
    }
    spliced(xml, &tags, &changed)
}

/// How a style reads on the works of `cite::sample`.
fn sample(xml: &str, root: &Path) -> Result<cite::Sample> {
    Ok(cite::sample(&cite::Style::Csl(Arc::new(csl::parse(xml, root)?))))
}

/// What of a style can be changed here: the details `read` finds, without those whose change leaves the
/// style's sample as it was (said in a place not known here, or of no effect: what follows an initial, in a
/// style that writes given names in full). Fails for a file that is no style to cite with.
pub fn knobs(xml: &str, root: &Path) -> Result<Knobs> {
    let found = read(xml);
    let was = sample(xml, root)?;
    let shows = |other: Knobs| sample(&write(xml, &other), root).is_ok_and(|now| now != was);
    let other_cut = |cut: Cut| if cut == (Cut { min: 2, first: 1 }) { Cut { min: 3, first: 1 } } else { Cut { min: 2, first: 1 } };
    Ok(Knobs {
        brackets: found.brackets.filter(|was| shows(Knobs { brackets: Some(if *was == Brackets::Square { Brackets::Round } else { Brackets::Square }), ..Default::default() })),
        raised: found.raised.filter(|was| shows(Knobs { raised: Some(!was), ..Default::default() })),
        cited: found.cited.filter(|was| shows(Knobs { cited: Some(other_cut(*was)), ..Default::default() })),
        listed: found.listed.filter(|was| shows(Knobs { listed: Some(other_cut(*was)), ..Default::default() })),
        initials: found.initials.clone().filter(|was| shows(Knobs { initials: Some(if was.is_empty() { ". " } else { "" }.to_string()), ..Default::default() })),
    })
}

// ---- a changed style, kept

/// A style written out says the namespace of CSL on its `<style>`, as a file of the style repository does
/// (the built-in ones are written out without it).
pub(crate) fn in_namespace(xml: &str) -> String {
    let tags = tags(xml);
    let Some(root) = tags.iter().position(|tag| tag.name == "style" && tag.parent.is_none()) else { return xml.to_string() };
    if said(tags[root].text, "xmlns").is_some() { return xml.to_string() }
    spliced(xml, &tags, &BTreeMap::from([(root, with(tags[root].text, "xmlns", Some(NAMESPACE)))]))
}

/// `xml` with what the first element of this name in its `<info>` holds replaced.
fn with_info(xml: &str, name: &str, content: &str) -> String {
    let tags = tags(xml);
    let Some(tag) = tags.iter().find(|tag| tag.name == name && !tag.text.ends_with("/>") && tag.parent.is_some_and(|parent| tags[parent].name == "info")) else { return xml.to_string() };
    let start = tag.at + tag.text.len();
    let Some(end) = xml[start..].find("</").map(|end| start + end) else { return xml.to_string() };
    format!("{}{}{}", &xml[..start], xml_escape(content), &xml[end..])
}

/// A style under another title.
pub fn titled(xml: &str, title: &str) -> String {
    with_info(xml, "title", title)
}

/// A style's title, as its file gives it.
pub fn title_of(xml: &str) -> Option<String> {
    let tags = tags(xml);
    let tag = tags.iter().find(|tag| tag.name == "title" && tag.parent.is_some_and(|parent| tags[parent].name == "info"))?;
    let start = tag.at + tag.text.len();
    Some(unescape(xml[start..].split("</").next()?.trim())).filter(|title| !title.is_empty())
}

/// The key of the style a copy was made from (`copied`); none for any other file.
pub fn base_of(xml: &str) -> Option<String> {
    let start = xml.find(MARK)? + MARK.len();
    Some(xml[start..].split("-->").next()?.trim().to_string()).filter(|key| !key.is_empty())
}

/// A style as a style of the person's own made from another: under an `id` and a `title` of its own, the
/// style it was made from (`base`, a key) named in a comment at its head and by the link that called it
/// itself, which now says what it follows.
fn copied(xml: &str, base: &str, id: &str, title: &str) -> String {
    let xml = with_info(&with_info(&in_namespace(xml), "id", id), "title", title);
    let tags = tags(&xml);
    let links: BTreeMap<usize, String> = tags.iter().enumerate().filter(|(_, tag)| tag.name == "link" && said(tag.text, "rel").as_deref() == Some("self")).map(|(i, tag)| (i, with(tag.text, "rel", Some("template")))).collect();
    let xml = spliced(&xml, &tags, &links);
    // A key is a name and a file name: nothing in it ends a comment.
    let mark = if base.contains("--") { String::new() } else { format!("{MARK}{base} -->\n") };
    match xml.strip_prefix("<?xml").and_then(|rest| rest.find("?>")) {
        Some(end) => format!("{}\n{mark}{}", &xml[..end + 7], xml[end + 7..].trim_start()),
        None => format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n{mark}{xml}"),
    }
}

/// The copy the person already has of `base` that reads as `knobs` do, if any: kept once, not once more.
pub fn kept(root: &Path, base: &str, knobs: &Knobs) -> Option<Listed> {
    csl::added(root).into_iter().filter(|style| style.key.starts_with("file:")).find(|style| csl::xml_of(&style.key, root).is_ok_and(|xml| base_of(&xml).as_deref() == Some(base) && read(&xml) == *knobs))
}

/// Keep a changed style (`write`) in the data folder's styles. One of the person's own (`key` is `file:…`)
/// is written over, what it said before kept in `styles/removed/` (`csl::save`), under `title` if one is
/// given. Any other is left as it is and kept as a new file named after it, with `title` and the style it
/// was made from (`copied`).
pub fn keep(root: &Path, key: &str, xml: &str, title: Option<&str>) -> Result<Listed> {
    if let Some(file) = key.strip_prefix("file:") {
        return csl::save(root, file, &title.map_or_else(|| xml.to_string(), |title| titled(xml, title)));
    }
    let named = key.split_once(':').map_or(key, |(_, name)| name).trim_end_matches(".csl");
    let stem: String = named.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let stem = format!("{}-adjusted", stem.trim_matches('-'));
    let folder = root.join(csl::FOLDER);
    let free = (1..).map(|n| if n == 1 { stem.clone() } else { format!("{stem}-{n}") }).find(|name| !folder.join(format!("{name}.csl")).exists()).unwrap_or(stem);
    let title = title.map(str::to_string).or_else(|| title_of(xml)).unwrap_or_else(|| free.clone());
    csl::save(root, &free, &copied(xml, key, &format!("urn:wisp-study:style:{free}"), &title))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> std::path::PathBuf {
        let folder = std::env::temp_dir().join(format!("wisp-adjust-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn builtin(name: &str) -> String {
        csl::xml_of(&format!("csl:{name}"), Path::new("/nowhere")).unwrap()
    }

    fn shown(xml: &str) -> cite::Sample {
        sample(xml, Path::new("/nowhere")).unwrap()
    }

    #[test]
    fn a_tag_is_written_anew_only_where_it_changes() {
        assert_eq!(with("<layout delimiter=\",\">", "prefix", Some("[")), "<layout delimiter=\",\" prefix=\"[\">");
        assert_eq!(with("<layout vertical-align=\"sup\" delimiter=\",\">", "vertical-align", None), "<layout delimiter=\",\">");
        assert_eq!(with("<name initialize-with='. ' and=\"text\"/>", "initialize-with", Some("")), "<name initialize-with=\"\" and=\"text\"/>");
        assert_eq!(with("<text variable=\"citation-number\"/>", "prefix", Some("<&\"")), "<text variable=\"citation-number\" prefix=\"&lt;&amp;&quot;\"/>");
        assert_eq!(with("<layout>", "prefix", None), "<layout>");
        // A name inside another's, or in a value, is not that attribute.
        assert_eq!(said("<style initialize-with-hyphen=\"false\" initialize-with=\". \">", "initialize-with").as_deref(), Some(". "));
        assert_eq!(said("<text value=\"a prefix=&quot;x&quot;\" suffix=\")\"/>", "prefix"), None);
        assert_eq!(said("<text value=\"a &amp; b\"/>", "value").as_deref(), Some("a & b"));
    }

    #[test]
    fn the_common_styles_are_read_where_they_say_it() {
        let nature = read(&builtin("nature"));
        assert_eq!((nature.brackets, nature.raised, nature.cited), (Some(Brackets::Bare), Some(true), None));
        assert_eq!((nature.listed, nature.initials.as_deref()), (Some(Cut { min: 6, first: 1 }), Some(". ")));
        let vancouver = read(&builtin("vancouver"));
        assert_eq!((vancouver.brackets, vancouver.raised, vancouver.listed, vancouver.initials.as_deref()), (Some(Brackets::Round), Some(false), Some(Cut { min: 7, first: 6 }), Some("")));
        // Brackets on a group around the number, "et al." on the name itself.
        let ieee = read(&builtin("ieee"));
        assert_eq!((ieee.brackets, ieee.raised, ieee.listed), (Some(Brackets::Square), Some(false), Some(Cut { min: 7, first: 1 })));
        // Raised and in brackets at once.
        let gb = read(&builtin("gb-7714-2015-numeric"));
        assert_eq!((gb.brackets, gb.raised, gb.listed, gb.initials.as_deref()), (Some(Brackets::Square), Some(true), Some(Cut { min: 4, first: 3 }), Some(" ")));
        let apa = read(&builtin("apa"));
        assert_eq!((apa.brackets, apa.raised, apa.cited, apa.listed), (None, None, Some(Cut { min: 3, first: 1 }), Some(Cut { min: 21, first: 19 })));
        // A style that names every author.
        assert_eq!(read(&builtin("american-chemical-society")).listed, Some(Cut::default()));
    }

    #[test]
    fn a_number_changes_its_brackets_and_its_height() {
        let nature = builtin("nature");
        assert!(shown(&nature).sentence.looks.iter().any(|(_, look)| look.sup));
        let square = write(&nature, &Knobs { brackets: Some(Brackets::Square), raised: Some(false), ..Default::default() });
        let sample = shown(&square);
        assert_eq!(sample.sentence.text, "Root hairs take up most of the water [1], as later work confirmed [2,3].");
        assert!(!sample.sentence.looks.iter().any(|(_, look)| look.sup));
        // The list is as it was.
        assert_eq!(sample.list, shown(&nature).list);
        // IEEE's brackets leave the group they were on: one pair around the whole citation.
        let round = write(&builtin("ieee"), &Knobs { brackets: Some(Brackets::Round), ..Default::default() });
        assert_eq!(shown(&builtin("ieee")).sentence.text, "Root hairs take up most of the water [1], as later work confirmed [2], [3].");
        assert_eq!(shown(&round).sentence.text, "Root hairs take up most of the water (1), as later work confirmed (2, 3).");
        // GB/T's raised `[1]` comes down to the line and keeps its brackets.
        let gb = write(&builtin("gb-7714-2015-numeric"), &Knobs { raised: Some(false), ..Default::default() });
        let sample = shown(&gb);
        assert!(sample.sentence.text.contains("[1]") && !sample.sentence.looks.iter().any(|(_, look)| look.sup), "{}", sample.sentence.text);
    }

    #[test]
    fn authors_are_cut_where_asked() {
        // In the list: Nature names one of seven, then three, then all.
        let nature = builtin("nature");
        let first = |xml: &str| shown(xml).list[0].text.clone();
        assert!(first(&nature).starts_with("1. Zhang, W. et al. A single-cell"), "{}", first(&nature));
        let three = write(&nature, &Knobs { listed: Some(Cut { min: 4, first: 3 }), ..Default::default() });
        assert!(first(&three).starts_with("1. Zhang, W., Li, Q., Wang, F., et al. A single-cell"), "{}", first(&three));
        let all = write(&nature, &Knobs { listed: Some(Cut::default()), ..Default::default() });
        assert!(first(&all).contains("Tanaka, H. & Garcia, M. A single-cell") && !first(&all).contains("et al"), "{}", first(&all));
        assert_eq!(read(&all).listed, Some(Cut::default()));
        // IEEE says it on the name, which follows the list once the list says it.
        let ieee = builtin("ieee");
        assert!(first(&ieee).starts_with("[1] W. Zhang et al., "), "{}", first(&ieee));
        let ieee = write(&ieee, &Knobs { listed: Some(Cut { min: 8, first: 3 }), ..Default::default() });
        assert!(first(&ieee).contains("H. Tanaka, and M. Garcia") && !first(&ieee).contains("et al"), "{}", first(&ieee));
        assert_eq!(read(&ieee).listed, Some(Cut { min: 8, first: 3 }));
        // APA ends a list cut short with its last author, as it does its own.
        let apa = builtin("apa");
        let long = |xml: &str| shown(xml).list[1].text.clone();
        let short = write(&apa, &Knobs { listed: Some(Cut { min: 4, first: 3 }), ..Default::default() });
        assert!(long(&short).starts_with("Zhang, W., Li, Q., Wang, F., … Garcia, M. (2024)."), "{}", long(&short));
        // In the text: APA writes two authors out, until asked not to. The list is as it was.
        assert!(shown(&apa).sentence.text.contains("(Smith & Jones, 2019;"), "{}", shown(&apa).sentence.text);
        let two = write(&apa, &Knobs { cited: Some(Cut { min: 2, first: 1 }), ..Default::default() });
        assert!(shown(&two).sentence.text.contains("(Smith et al., 2019;"), "{}", shown(&two).sentence.text);
        assert_eq!(shown(&two).list, shown(&apa).list);
        // A number of names to name first that is no fewer than the number that cuts is brought under it.
        assert_eq!(read(&write(&nature, &Knobs { listed: Some(Cut { min: 3, first: 9 }), ..Default::default() })).listed, Some(Cut { min: 3, first: 2 }));
    }

    #[test]
    fn initials_are_followed_by_what_is_asked() {
        // The second work's first author has two given names, where the four ways differ.
        let nature = builtin("nature");
        let second = |xml: &str, initials: &str| shown(&write(xml, &Knobs { initials: Some(initials.into()), ..Default::default() })).list[1].text.clone();
        assert!(shown(&nature).list[1].text.starts_with("2. Smith, J. A. & Jones, M. Auxin"));
        assert!(second(&nature, ".").starts_with("2. Smith, J.A. & Jones, M. Auxin"), "{}", second(&nature, "."));
        assert!(second(&nature, " ").starts_with("2. Smith, J A & Jones, M. Auxin"), "{}", second(&nature, " "));
        assert!(second(&nature, "").starts_with("2. Smith, JA & Jones, M. Auxin"), "{}", second(&nature, ""));
        // GB/T says it on the style as a whole.
        let gb = builtin("gb-7714-2015-numeric");
        assert!(shown(&gb).list[1].text.starts_with("[2] SMITH J A, JONES M. Auxin"), "{}", shown(&gb).list[1].text);
        assert!(second(&gb, ". ").starts_with("[2] SMITH J. A., JONES M. Auxin"), "{}", second(&gb, ". "));
        assert_eq!(read(&write(&gb, &Knobs { initials: Some(String::new()), ..Default::default() })).initials.as_deref(), Some(""));
    }

    #[test]
    fn nothing_asked_changes_nothing() {
        for name in ["nature", "ieee", "apa", "gb-7714-2015-numeric"] {
            let xml = builtin(name);
            assert_eq!(write(&xml, &Knobs::default()), xml);
            assert_eq!(write(&xml, &read(&xml)), xml, "{name}");
        }
    }

    /// Every style offered first: what is offered of it shows, is read back as written, leaves the other
    /// details as they were, and is still a style to cite with.
    #[test]
    fn what_is_offered_of_the_common_styles_takes() {
        let root = Path::new("/nowhere");
        for style in csl::common() {
            let xml = csl::xml_of(&style.key, root).unwrap();
            let offered = knobs(&xml, root).unwrap_or_else(|err| panic!("{}: {err:#}", style.key));
            let was = read(&xml);
            let mut tried = vec![];
            if let Some(brackets) = offered.brackets { tried.push(Knobs { brackets: Some(if brackets == Brackets::Round { Brackets::Square } else { Brackets::Round }), ..Default::default() }) }
            if let Some(raised) = offered.raised { tried.push(Knobs { raised: Some(!raised), ..Default::default() }) }
            if offered.cited.is_some() { tried.push(Knobs { cited: Some(Cut { min: 4, first: 1 }), ..Default::default() }) }
            if offered.listed.is_some() { tried.push(Knobs { listed: Some(Cut { min: 5, first: 2 }), ..Default::default() }) }
            if let Some(initials) = &offered.initials { tried.push(Knobs { initials: Some(if initials.contains('.') { " " } else { "." }.to_string()), ..Default::default() }) }
            // A numbered style offers its number, one by author and year its "et al."; both offer the list's.
            assert!(offered.brackets.is_some() != offered.cited.is_some(), "{}: {offered:?}", style.key);
            assert!(offered.listed.is_some(), "{}", style.key);
            for wanted in tried {
                let changed = write(&xml, &wanted);
                let now = read(&changed);
                let expected = Knobs { brackets: wanted.brackets.or(was.brackets), raised: wanted.raised.or(was.raised), cited: wanted.cited.or(was.cited), listed: wanted.listed.or(was.listed), initials: wanted.initials.clone().or(was.initials.clone()) };
                assert_eq!(now, expected, "{} with {wanted:?}", style.key);
                assert_ne!(sample(&changed, root).unwrap_or_else(|err| panic!("{} with {wanted:?}: {err:#}", style.key)), sample(&xml, root).unwrap(), "{} with {wanted:?}", style.key);
            }
        }
    }

    #[test]
    fn what_does_not_show_is_not_offered() {
        let root = Path::new("/nowhere");
        // Chicago writes given names in full: what follows an initial changes nothing a reader sees.
        let chicago = builtin("chicago-author-date");
        assert!(read(&chicago).initials.is_some());
        assert_eq!(knobs(&chicago, root).unwrap().initials, None);
        assert!(knobs(&builtin("nature"), root).unwrap().initials.is_some());
        assert!(knobs("<style>", root).is_err());
    }

    #[test]
    fn a_file_written_by_hand_keeps_what_it_says() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" class="in-text" version="1.0">
  <!-- a style of mine -->
  <info>
    <title>Mine &amp; yours</title>
    <id>http://example.org/mine</id>
    <link href="http://example.org/mine" rel="self"/>
    <updated>2024-01-01T00:00:00+00:00</updated>
  </info>
  <citation collapse='citation-number'>
    <layout prefix='(' suffix=')' delimiter=', '>
      <text variable="citation-number"/>
    </layout>
  </citation>
  <bibliography et-al-min="4" et-al-use-first="3">
    <layout>
      <names variable="author"><name initialize-with="." name-as-sort-order="all"/></names>
      <text variable="title" prefix=" "/>
    </layout>
  </bibliography>
</style>"#;
        let was = read(xml);
        assert_eq!((was.brackets, was.raised, was.listed, was.initials.as_deref()), (Some(Brackets::Round), Some(false), Some(Cut { min: 4, first: 3 }), Some(".")));
        let changed = write(xml, &Knobs { brackets: Some(Brackets::Square), raised: Some(true), listed: Some(Cut::default()), ..Default::default() });
        // Only the two tags are written anew; the comment, the quotes and the lines of the rest stay.
        assert!(changed.contains("<layout prefix=\"[\" suffix=\"]\" delimiter=', ' vertical-align=\"sup\">"), "{changed}");
        assert!(changed.contains("<bibliography>") && changed.contains("<!-- a style of mine -->") && changed.contains("<citation collapse='citation-number'>"), "{changed}");
        assert_eq!(changed.lines().count(), xml.lines().count());
        assert_eq!(title_of(&changed).as_deref(), Some("Mine & yours"));
        assert!(shown(&changed).sentence.text.contains("[1]"));
    }

    #[test]
    fn a_changed_style_is_kept_as_a_file_of_the_persons_own() {
        let root = folder("keep");
        let nature = builtin("nature");
        let wanted = Knobs { brackets: Some(Brackets::Square), raised: Some(false), ..Default::default() };
        let changed = write(&nature, &wanted);
        let kept_as = keep(&root, "csl:nature", &changed, Some("Nature（方括号）")).unwrap();
        assert_eq!((kept_as.key.as_str(), kept_as.name.as_str()), ("file:nature-adjusted.csl", "Nature（方括号）"));
        let xml = csl::xml_of(&kept_as.key, &root).unwrap();
        // A file Zotero takes: the namespace, an id of its own, and the style it follows.
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!-- Wisp Study: adjusted from csl:nature -->\n<style "), "{}", &xml[..200]);
        assert!(xml.contains("xmlns=\"http://purl.org/net/xbiblio/csl\"") && xml.contains("<id>urn:wisp-study:style:nature-adjusted</id>"));
        assert!(xml.contains("<link href=\"http://www.zotero.org/styles/nature\" rel=\"template\"/>") && !xml.contains("rel=\"self\""));
        assert_eq!(base_of(&xml).as_deref(), Some("csl:nature"));
        assert_eq!(base_of(&nature), None);
        let style = csl::load(&kept_as.key, &root).unwrap();
        assert!(style.numbered());
        assert_eq!(read(&xml), read(&changed));
        // The same change again is the file there is; another is a file beside it.
        assert_eq!(kept(&root, "csl:nature", &read(&changed)).map(|style| style.key).as_deref(), Some("file:nature-adjusted.csl"));
        assert_eq!(kept(&root, "csl:vancouver", &read(&changed)), None);
        let round = write(&nature, &Knobs { brackets: Some(Brackets::Round), raised: Some(false), ..Default::default() });
        assert_eq!(kept(&root, "csl:nature", &read(&round)), None);
        assert_eq!(keep(&root, "csl:nature", &round, Some("Nature（圆括号）")).unwrap().key, "file:nature-adjusted-2.csl");
        // One of the person's own is written over, under a title that says what it is now.
        let again = write(&xml, &Knobs { listed: Some(Cut { min: 4, first: 3 }), ..Default::default() });
        let kept_as = keep(&root, "file:nature-adjusted.csl", &again, Some("Nature（方括号，列前 3 位作者）")).unwrap();
        assert_eq!((kept_as.key.as_str(), kept_as.name.as_str()), ("file:nature-adjusted.csl", "Nature（方括号，列前 3 位作者）"));
        assert_eq!(base_of(&csl::xml_of(&kept_as.key, &root).unwrap()).as_deref(), Some("csl:nature"));
        assert_eq!(std::fs::read_dir(root.join("styles/removed")).unwrap().count(), 1);
        // A style of Zotero's or a file with dots in its name gives a file name that can be used.
        assert_eq!(keep(&root, "zotero:My Journal 2.csl", &changed, None).unwrap().key, "file:my-journal-2-adjusted.csl");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_built_in_style_written_out_says_its_namespace() {
        let xml = builtin("nature");
        assert_eq!(said(tags(&xml)[0].text, "xmlns").as_deref(), Some(NAMESPACE), "{}", &xml[..160]);
        // One that says it already is left alone.
        assert_eq!(in_namespace(&xml), xml);
        assert!(csl::parse(&xml, Path::new("/nowhere")).is_ok());
    }
}
