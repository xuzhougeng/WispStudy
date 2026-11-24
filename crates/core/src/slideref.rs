//! A PowerPoint file given as the reference for slides to be made (.pptx,
//! .potx): how a talk of the person's looks and how it is told.
//!
//! One reading, two uses. For whoever writes the new deck, a person or a
//! model: what each slide says (its title, its texts by level, its notes),
//! which layouts the slides stand on, the theme's fonts and colours. For
//! `pptx::write_on`: the parts of the file a new deck keeps (the masters,
//! the layouts, the themes and what they show: pictures, fonts), without
//! its slides, and where a title and points stand in the three layouts the
//! new slides use (`Stage`).
//!
//! The layouts are chosen as the file uses them: the content's is the one
//! most of its slides with a title and a text stand on, the first slide's
//! is the title page's when it is one, the section header of the same
//! master opens a part. A file whose slides are boxes drawn on blank
//! layouts has none in use: the layouts are then chosen by their kind, and
//! `Reference::drawn` says that the new deck will not look like its slides.
//!
//! The XML is read as it is written (`docx`'s readers), with the prefixes
//! every program gives it: `p:` and `a:`.

use crate::docx::{Node, Zip, attr, beside, children, element_end, inside, node, open_tag, rels_of, unescape, without};
use crate::manuscript::xml_escape as escape;
use crate::pptx::{Frame, Holder, Place, Stage};
use anyhow::{Result, anyhow, bail};

const NOT_PPTX: &str = "这不是 PowerPoint 的 .pptx 文件（旧的 .ppt 和加了密码的文件读不了）。";
const MAIN_TYPE: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
/// What a presentation leads to that a new deck keeps; its slides and what only they need are left out.
const KEPT: [&str; 7] = ["slideMaster", "notesMaster", "handoutMaster", "theme", "presProps", "tableStyles", "font"];
/// What a presentation says about its slides or about parts that are not kept.
const DROPPED: [&str; 7] = ["p:sldIdLst", "p:extLst", "p:custShowLst", "p:custDataLst", "p:smartTags", "p:photoAlbum", "p:modifyVerifier"];
/// The room a text keeps free in its box when nothing says otherwise: left, right, top, bottom, in points.
const INSETS: [f32; 4] = [7.2, 7.2, 3.6, 3.6];
/// For reading the placeholders of a slide, whose colours are not asked about.
const NO_PALETTE: Palette<'static> = Palette { scheme: "", map: "" };

// ---- the file's parts

struct Rel {
    id: String,
    kind: String,
    target: String,
    external: bool,
}

impl Rel {
    fn is(&self, kind: &str) -> bool {
        self.kind.rsplit('/').next() == Some(kind)
    }
}

fn relations(xml: &str) -> Vec<Rel> {
    let (mut out, mut at) = (vec![], 0);
    while let Some(found) = xml[at..].find("<Relationship ") {
        let Some((Node::Open(_, tag) | Node::Empty(_, tag), end)) = node(xml, at + found) else { break };
        out.push(Rel { id: attr(tag, "Id").unwrap_or_default(), kind: attr(tag, "Type").unwrap_or_default(), target: attr(tag, "Target").unwrap_or_default(), external: attr(tag, "TargetMode").is_some_and(|mode| mode.eq_ignore_ascii_case("External")) });
        at = end;
    }
    out
}

/// Where the first element of this name at or after `from` starts and ends.
fn find(xml: &str, name: &str, from: usize) -> Option<(usize, usize)> {
    let open = format!("<{name}");
    let mut from = from;
    while let Some(found) = xml.get(from..)?.find(&open) {
        let at = from + found;
        if xml[at + open.len()..].starts_with(|c: char| c.is_whitespace() || c == '/' || c == '>') { return Some((at, element_end(xml, at))) }
        from = at + open.len();
    }
    None
}

/// The first element of this name in `xml`, whole.
fn first<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    find(xml, name, 0).map(|(start, end)| &xml[start..end])
}

/// Every element of this name in `xml` that is not inside another of them.
fn every<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let (mut out, mut at) = (vec![], 0);
    while let Some((start, end)) = find(xml, name, at) {
        out.push(&xml[start..end]);
        at = end.max(start + 1);
    }
    out
}

/// A length of the file (914400 to an inch) in points.
fn points(tag: &str, name: &str) -> Option<f32> {
    attr(tag, name).and_then(|value| value.trim().parse::<f64>().ok()).map(|emu| (emu / 12700.0) as f32)
}

struct Package {
    zip: Zip,
    /// The presentation's part: `ppt/presentation.xml`.
    main: String,
}

impl Package {
    fn open(bytes: &[u8]) -> Result<Package> {
        let zip = Zip::read(bytes).map_err(|_| anyhow!(NOT_PPTX))?;
        let root = zip.text("_rels/.rels").map_err(|_| anyhow!(NOT_PPTX))?;
        let main = relations(&root).into_iter().find(|rel| rel.is("officeDocument")).map_or("ppt/presentation.xml".to_string(), |rel| beside("", &rel.target));
        let package = Package { zip, main };
        if !package.main.contains('/') || !package.text(&package.main).contains("<p:presentation") { bail!(NOT_PPTX) }
        Ok(package)
    }

    fn text(&self, part: &str) -> String {
        self.zip.text(part).unwrap_or_default()
    }

    fn relations(&self, part: &str) -> Vec<Rel> {
        relations(&self.text(&rels_of(part)))
    }

    /// The part a relation of `part` of this kind leads to.
    fn led(&self, part: &str, kind: &str) -> Option<String> {
        self.relations(part).into_iter().find(|rel| rel.is(kind) && !rel.external).map(|rel| beside(part, &rel.target))
    }
}

// ---- shapes

struct Shape<'a> {
    /// `p:sp`, `p:pic`, `p:graphicFrame`.
    name: &'a str,
    xml: &'a str,
}

/// The shapes of a tree, those in groups among them, in the order they are drawn.
fn shapes<'a>(tree: &'a str, out: &mut Vec<Shape<'a>>) {
    for (name, xml) in children(inside(tree)) {
        match name {
            "p:sp" | "p:pic" | "p:graphicFrame" => out.push(Shape { name, xml }),
            "p:grpSp" => shapes(xml, out),
            // Something newer programs draw one way and older ones another: one of the two.
            "mc:AlternateContent" => {
                if let Some(way) = first(xml, "mc:Fallback").or_else(|| first(xml, "mc:Choice")) { shapes(way, out) }
            }
            _ => {}
        }
    }
}

fn frame(shape: &str) -> Option<Frame> {
    let place = first(first(shape, "p:spPr")?, "a:xfrm")?;
    let (from, size) = (open_tag(first(place, "a:off")?), open_tag(first(place, "a:ext")?));
    let frame = Frame { x: points(from, "x")?, y: points(from, "y")?, w: points(size, "cx")?, h: points(size, "cy")? };
    (frame.w > 1.0 && frame.h > 1.0).then_some(frame)
}

/// The colour a list of levels gives the letters of the first one.
fn first_colour(levels: &str, palette: &Palette) -> Option<String> {
    let letters = first(first(levels, "a:lvl1pPr")?, "a:defRPr")?;
    colour(inside(first(letters, "a:solidFill")?), &|name| palette.get(name))
}

/// The size of the letters a list of levels gives the first one.
fn first_size(levels: &str) -> Option<f32> {
    let letters = open_tag(first(first(levels, "a:lvl1pPr")?, "a:defRPr")?);
    attr(letters, "sz").and_then(|size| size.parse::<f32>().ok()).map(|size| size / 100.0).filter(|size| *size >= 1.0)
}

/// A placeholder of a slide, a layout or a master.
#[derive(Clone, Debug, Default)]
struct Held {
    /// `title`, `ctrTitle`, `subTitle`, `body`, `obj`, `sldNum`, `ftr`, `dt`…; empty for one that takes anything.
    kind: String,
    /// `<p:ph …/>` as a slide that fills it says it.
    tag: String,
    frame: Option<Frame>,
    size: Option<f32>,
    /// The colour of its letters, `RRGGBB`, where it says one.
    colour: Option<String>,
    /// Left, right, top, bottom, where it says them.
    insets: [Option<f32>; 4],
    vertical: bool,
}

impl Held {
    fn titles(&self) -> bool {
        matches!(self.kind.as_str(), "title" | "ctrTitle")
    }

    /// A text under a title goes in it.
    fn takes_text(&self) -> bool {
        matches!(self.kind.as_str(), "" | "body" | "obj") && !self.vertical
    }
}

fn held(shape: &str, palette: &Palette) -> Option<Held> {
    let head = children(inside(shape)).into_iter().find(|(name, _)| !name.is_empty())?.1;
    let tag = open_tag(first(head, "p:ph")?);
    let mut said = String::from("<p:ph");
    for name in ["type", "orient", "sz", "idx"] {
        if let Some(value) = attr(tag, name) { said.push_str(&format!(" {name}=\"{}\"", escape(&value))) }
    }
    said.push_str("/>");
    let text = first(shape, "p:txBody");
    let room = text.and_then(|text| first(text, "a:bodyPr")).map(open_tag).unwrap_or("");
    Some(Held {
        kind: attr(tag, "type").unwrap_or_default(),
        tag: said,
        frame: frame(shape),
        size: text.and_then(|text| first(text, "a:lstStyle")).and_then(first_size),
        colour: text.and_then(|text| first(text, "a:lstStyle")).and_then(|levels| first_colour(levels, palette)),
        insets: ["lIns", "rIns", "tIns", "bIns"].map(|name| points(room, name)),
        vertical: attr(tag, "orient").is_some_and(|way| way == "vert") || attr(room, "vert").is_some_and(|way| way != "horz"),
    })
}

/// The placeholders of a layout or a master.
fn holders(xml: &str, palette: &Palette) -> Vec<Held> {
    let mut all = vec![];
    if let Some(tree) = first(xml, "p:spTree") { shapes(tree, &mut all) }
    all.iter().filter_map(|shape| held(shape.xml, palette)).collect()
}

/// The words of a paragraph, a cell or anything else that holds runs of text.
fn words(xml: &str) -> String {
    let (mut out, mut at, mut inside) = (String::new(), 0, false);
    while let Some((piece, end)) = node(xml, at) {
        match piece {
            Node::Open("a:t", _) => inside = true,
            Node::Empty("a:br", _) => out.push(' '),
            Node::Text(text) if inside => out.push_str(&unescape(text)),
            Node::Close => inside = false,
            _ => {}
        }
        at = end;
    }
    // A line broken inside a paragraph is written as a vertical tab by some programs.
    out.replace("_x000B_", " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The paragraphs of a shape's text that say something: their level and their words.
fn paragraphs(shape: &str) -> Vec<(usize, String)> {
    let Some(text) = first(shape, "p:txBody") else { return vec![] };
    every(text, "a:p").into_iter().filter_map(|paragraph| {
        let level = first(paragraph, "a:pPr").and_then(|look| attr(open_tag(look), "lvl")).and_then(|level| level.parse::<usize>().ok()).unwrap_or(0);
        Some((level.min(8), words(paragraph))).filter(|(_, text)| !text.is_empty())
    }).collect()
}

// ---- what the file says

/// A slide of the reference.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// What its title placeholder says; empty for a slide without one.
    pub title: String,
    /// Its other texts in the order they were drawn: the level of each (0 the outermost) and its words. A table is a line a row, its cells set apart by ` | `.
    pub lines: Vec<(usize, String)>,
    pub notes: String,
    pub pictures: usize,
    /// The layout it stands on: its place in `Reference::layouts`.
    pub layout: Option<usize>,
}

/// A layout of the reference.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    part: String,
    /// As PowerPoint lists it: 「标题和内容」, `Title and Content`.
    pub name: String,
    /// `title`, `obj`, `secHead`, `blank`…; empty for one of the file's own making.
    pub kind: String,
    /// How many of the file's slides stand on it.
    pub used: usize,
    master: usize,
    held: Vec<Held>,
    /// A background of its own, where it has one.
    ground: Option<Ground>,
}

struct Master {
    held: Vec<Held>,
    /// The sizes and the colours the master gives a title and the first level of a text.
    title: (Option<f32>, Option<String>),
    body: (Option<f32>, Option<String>),
    margins: [f32; 2],
    ground: Ground,
    /// The colour of text that nothing else gives one.
    text: String,
    theme: Theme,
}

/// What slides are drawn on: one colour (`RRGGBB`), or something no one colour says.
#[derive(Clone, Debug, PartialEq)]
enum Ground {
    Plain(String),
    Picture,
    Gradient,
}

/// The colours of a theme, by the names a master gives them.
struct Palette<'a> {
    /// The theme's `a:clrScheme`.
    scheme: &'a str,
    /// The master's `<p:clrMap …>`: which of the theme's colours `tx1`, `bg1` and so on are.
    map: &'a str,
}

impl Palette<'_> {
    fn get(&self, name: &str) -> Option<String> {
        let named = attr(self.map, name).unwrap_or_else(|| name.to_string());
        first(self.scheme, &format!("a:{named}")).and_then(|one| colour(inside(one), &|_| None))
    }
}

/// The background a master or a layout gives its slides, where it says one.
fn ground(xml: &str, palette: &Palette) -> Option<Ground> {
    let fill = first(xml, "p:bg")?;
    if fill.contains("<a:blip") { return Some(Ground::Picture) }
    if fill.contains("<a:gradFill") || fill.contains("<a:pattFill") { return Some(Ground::Gradient) }
    colour(inside(fill), &|name| palette.get(name)).map(Ground::Plain)
}

/// Between two colours (`RRGGBB`): `part` of the way from the first to the second.
fn mix(from: &str, to: &str, part: f32) -> String {
    let channel = |colour: &str, at: usize| colour.get(at..at + 2).and_then(|hex| u8::from_str_radix(hex, 16).ok()).unwrap_or(0) as f32;
    (0..3).map(|n| format!("{:02X}", (channel(from, n * 2) * (1.0 - part) + channel(to, n * 2) * part).round() as u8)).collect()
}

/// What a master's theme says, for whoever describes the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Theme {
    pub name: String,
    /// The fonts of headings and of text.
    pub heading: String,
    pub body: String,
    /// The font named for Chinese, when one is.
    pub chinese: String,
    /// `#RRGGBB`; empty when it could not be read.
    pub text: String,
    /// `#RRGGBB`, or `picture` / `gradient` for a background that is one.
    pub background: String,
    pub accent: String,
}

pub struct Reference {
    package: Package,
    /// The slide's size in points.
    pub wide: f32,
    pub high: f32,
    pub slides: Vec<Shown>,
    pub layouts: Vec<Layout>,
    pub theme: Theme,
    /// The layouts the new slides stand on: the first slide's, a part opening's, every other's.
    pub cover: usize,
    pub opening: usize,
    pub content: usize,
    /// Most of the file's slides are drawn as boxes of their own, not in the placeholders of their layouts: a deck on
    /// these layouts will not look like them.
    pub drawn: bool,
    stage: Stage,
    kept: Vec<String>,
    notes_master: Option<String>,
}

/// A colour element (`a:srgbClr`, `a:sysClr`, `a:schemeClr`) among `xml`'s, as `RRGGBB`. A scheme colour is looked up by `scheme`.
fn colour(xml: &str, scheme: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let hex = |value: String| (value.len() == 6 && value.chars().all(|c| c.is_ascii_hexdigit())).then(|| value.to_uppercase());
    let (mut at, mut found) = (0, None);
    while let Some((piece, end)) = node(xml, at) {
        if let Node::Open(name, tag) | Node::Empty(name, tag) = piece {
            found = match name {
                "a:srgbClr" => attr(tag, "val").and_then(hex),
                "a:sysClr" => attr(tag, "lastClr").and_then(hex),
                "a:schemeClr" => attr(tag, "val").and_then(|name| scheme(&name)),
                _ => None,
            };
            if found.is_some() { break }
        }
        at = end;
    }
    found
}

fn theme(xml: &str, palette: &Palette, ground: &Ground) -> Theme {
    let hex = |colour: Option<String>| colour.map(|hex| format!("#{hex}")).unwrap_or_default();
    let face = |fonts: &str, of: &str| first(xml, fonts).and_then(|fonts| first(fonts, of)).and_then(|font| attr(open_tag(font), "typeface")).unwrap_or_default();
    let chinese = first(xml, "a:minorFont").and_then(|fonts| every(fonts, "a:font").into_iter().find(|font| attr(open_tag(font), "script").is_some_and(|script| script == "Hans"))).and_then(|font| attr(open_tag(font), "typeface"));
    Theme {
        name: first(xml, "a:theme").and_then(|theme| attr(open_tag(theme), "name")).unwrap_or_default(),
        heading: face("a:majorFont", "a:latin"),
        body: face("a:minorFont", "a:latin"),
        chinese: Some(face("a:minorFont", "a:ea")).filter(|face| !face.is_empty()).or(chinese).unwrap_or_default(),
        text: hex(palette.get("tx1")),
        background: match ground {
            Ground::Plain(colour) => format!("#{colour}"),
            Ground::Picture => "picture".into(),
            Ground::Gradient => "gradient".into(),
        },
        accent: hex(palette.get("accent1")),
    }
}

/// The sizes a title may have in a placeholder whose layout gives it `size`: that one, and smaller ones for a longer title.
fn title_sizes(size: f32) -> Vec<f32> {
    let mut sizes: Vec<f32> = [1.0, 0.87, 0.75, 0.62, 0.5, 0.4].iter().map(|part| (size * part).round()).filter(|one| *one >= 18.0).collect();
    sizes.dedup();
    if sizes.is_empty() { sizes.push(size.max(8.0)) }
    sizes
}

/// The sizes points may have where the layout gives them `size`: not larger than reads well on a slide of text, down by twos.
fn text_sizes(size: f32, most: f32, least: f32) -> Vec<f32> {
    let from = size.round().clamp(least + 2.0, most);
    (0..6).map(|n| from - 2.0 * n as f32).filter(|one| *one >= least).collect()
}

impl Reference {
    pub fn read(bytes: &[u8]) -> Result<Reference> {
        let package = Package::open(bytes)?;
        let presentation = package.text(&package.main);
        let listed = package.relations(&package.main);
        let part_of = |id: &str| listed.iter().find(|rel| rel.id == id && !rel.external).map(|rel| beside(&package.main, &rel.target));
        let size = first(&presentation, "p:sldSz").map(open_tag).unwrap_or("");
        let (wide, high) = (points(size, "cx").filter(|wide| *wide > 10.0).unwrap_or(960.0), points(size, "cy").filter(|high| *high > 10.0).unwrap_or(540.0));

        // The masters in the presentation's order, each with its layouts in its own.
        let (mut masters, mut layouts) = (vec![], vec![]);
        for listed in first(&presentation, "p:sldMasterIdLst").map(|list| every(list, "p:sldMasterId")).unwrap_or_default() {
            let Some(part) = attr(open_tag(listed), "r:id").and_then(|id| part_of(&id)) else { continue };
            let xml = package.text(&part);
            if xml.is_empty() { continue }
            let levels = first(&xml, "p:bodyStyle").unwrap_or("");
            let margin = |level: &str, or: f32| first(levels, level).and_then(|level| points(open_tag(level), "marL")).filter(|margin| (0.0..=200.0).contains(margin)).unwrap_or(or);
            let look = package.led(&part, "theme").map(|theme| package.text(&theme)).unwrap_or_default();
            let palette = Palette { scheme: first(&look, "a:clrScheme").unwrap_or(""), map: first(&xml, "p:clrMap").map(open_tag).unwrap_or("") };
            let own = ground(&xml, &palette).unwrap_or_else(|| Ground::Plain(palette.get("bg1").unwrap_or_else(|| "FFFFFF".into())));
            for rel in package.relations(&part).into_iter().filter(|rel| rel.is("slideLayout") && !rel.external) {
                let layout = beside(&part, &rel.target);
                let xml = package.text(&layout);
                if xml.is_empty() { continue }
                layouts.push(Layout { name: first(&xml, "p:cSld").and_then(|slide| attr(open_tag(slide), "name")).unwrap_or_default(), kind: attr(open_tag(first(&xml, "p:sldLayout").unwrap_or("")), "type").unwrap_or_default(), used: 0, master: masters.len(), held: holders(&xml, &palette), ground: ground(&xml, &palette), part: layout });
            }
            let heading = first(&xml, "p:titleStyle").unwrap_or("");
            masters.push(Master {
                held: holders(&xml, &palette),
                title: (first_size(heading), first_colour(heading, &palette)),
                body: (first_size(levels), first_colour(levels, &palette)),
                margins: [margin("a:lvl1pPr", 27.0), margin("a:lvl2pPr", 58.5)],
                text: palette.get("tx1").unwrap_or_else(|| "000000".into()),
                theme: theme(&look, &palette, &own),
                ground: own,
            });
        }
        if masters.is_empty() || layouts.is_empty() { bail!("这个 PowerPoint 文件里没有可用的母版和版式，做不了参考。") }

        // The slides in the order they are shown.
        let mut slides = vec![];
        let (mut filled, mut said) = (0, 0);
        for listed in first(&presentation, "p:sldIdLst").map(|list| every(list, "p:sldId")).unwrap_or_default() {
            let Some(part) = attr(open_tag(listed), "r:id").and_then(|id| part_of(&id)) else { continue };
            let xml = package.text(&part);
            let mut shown = Shown { layout: package.led(&part, "slideLayout").and_then(|layout| layouts.iter().position(|one| one.part == layout)), ..Default::default() };
            let mut all = vec![];
            if let Some(tree) = first(&xml, "p:spTree") { shapes(tree, &mut all) }
            for shape in all {
                match shape.name {
                    "p:pic" => shown.pictures += 1,
                    "p:graphicFrame" => shown.lines.extend(every(shape.xml, "a:tr").into_iter().map(|row| (0, every(row, "a:tc").into_iter().map(words).collect::<Vec<_>>().join(" | "))).filter(|(_, row)| row.chars().any(char::is_alphanumeric))),
                    _ => {
                        let holder = held(shape.xml, &NO_PALETTE);
                        // What every slide repeats says nothing of this one.
                        if holder.as_ref().is_some_and(|holder| matches!(holder.kind.as_str(), "sldNum" | "ftr" | "dt" | "hdr")) { continue }
                        let said = paragraphs(shape.xml);
                        if holder.is_some_and(|holder| holder.titles()) && shown.title.is_empty() {
                            shown.title = said.into_iter().map(|(_, text)| text).collect::<Vec<_>>().join(" ");
                        } else {
                            shown.lines.extend(said);
                        }
                    }
                }
            }
            shown.notes = package.led(&part, "notesSlide").map(|notes| package.text(&notes)).map(|xml| {
                let mut all = vec![];
                if let Some(tree) = first(&xml, "p:spTree") { shapes(tree, &mut all) }
                all.iter().filter(|shape| held(shape.xml, &NO_PALETTE).is_some_and(|holder| holder.kind == "body")).flat_map(|shape| paragraphs(shape.xml)).map(|(_, text)| text).collect::<Vec<_>>().join("\n")
            }).unwrap_or_default();
            if let Some(at) = shown.layout { layouts[at].used += 1 }
            if !shown.title.is_empty() { filled += 1 }
            if !shown.title.is_empty() || !shown.lines.is_empty() { said += 1 }
            slides.push(shown);
        }

        // The layout of the slides under a title: the one most of the file's slides stand on, of those that have a place for a text.
        let fit = |layout: &Layout| -> Option<usize> {
            if layout.kind.starts_with("vert") || matches!(layout.kind.as_str(), "title" | "secHead" | "blank" | "titleOnly") { return None }
            let texts = layout.held.iter().filter(|holder| holder.takes_text()).count();
            if texts == 0 { return None }
            Some(match (layout.kind.as_str(), texts) {
                ("obj", _) => 0,
                ("tx", _) => 1,
                (_, 1) => 2,
                _ => 3,
            })
        };
        let content = (0..layouts.len()).filter_map(|at| fit(&layouts[at]).map(|rank| (at, rank))).min_by_key(|(at, rank)| (std::cmp::Reverse(layouts[*at].used), *rank, *at)).map(|(at, _)| at);
        // A file without one: the first layout, with the master's places.
        let content = content.unwrap_or(0);
        let home = layouts[content].master;
        let titles = |layout: &Layout| layout.kind == "title" || layout.held.iter().any(|holder| holder.kind == "ctrTitle");
        let cover = slides.first().and_then(|slide| slide.layout).filter(|at| titles(&layouts[*at]))
            .or_else(|| (0..layouts.len()).find(|at| layouts[*at].master == home && titles(&layouts[*at])))
            .or_else(|| (0..layouts.len()).find(|at| titles(&layouts[*at])))
            .unwrap_or(content);
        let opening = (0..layouts.len()).find(|at| layouts[*at].master == home && layouts[*at].kind == "secHead").unwrap_or(cover);

        let master = &masters[home];
        let scale = (wide / 960.0, high / 540.0);
        let scaled = |x: f32, y: f32, w: f32, h: f32| Frame { x: x * scale.0, y: y * scale.1, w: w * scale.0, h: h * scale.1 };
        let from_master = |kind: &str| masters[home].held.iter().find(|holder| holder.kind == kind);
        // A place of a layout: the layout's own placeholder, standing where it says, else where the master's of its kind does.
        let title_of = |at: usize, or: Frame, size: f32| -> Holder {
            let layout = &layouts[at];
            let own = layout.held.iter().find(|holder| holder.titles());
            let above = masters[layout.master].held.iter().find(|holder| holder.kind == "title");
            Holder {
                tag: own.or(above).map_or("<p:ph type=\"title\"/>".to_string(), |holder| holder.tag.clone()),
                frame: own.and_then(|holder| holder.frame).or(above.and_then(|holder| holder.frame)).unwrap_or(or),
                sizes: title_sizes(own.and_then(|holder| holder.size).or(above.and_then(|holder| holder.size)).or(masters[layout.master].title.0).unwrap_or(size)),
            }
        };
        // The colour a title has on a layout: small text of the slide's own beside it is drawn in it.
        let title_colour = |at: usize| -> String {
            let layout = &layouts[at];
            let (own, above) = (layout.held.iter().find(|holder| holder.titles()), masters[layout.master].held.iter().find(|holder| holder.kind == "title"));
            own.and_then(|holder| holder.colour.clone()).or(above.and_then(|holder| holder.colour.clone())).or(masters[layout.master].title.1.clone()).unwrap_or_else(|| masters[layout.master].text.clone())
        };
        let plain = |colour: &str| format!("<a:srgbClr val=\"{colour}\"/>");
        let foot_of = |at: usize, kind: &str| layouts[at].held.iter().find(|holder| holder.kind == kind).map(|holder| holder.tag.clone());
        let place = |at: usize, title: Holder, under: Option<Holder>, soft: String| Place { layout: layouts[at].part.clone(), title, under, number: foot_of(at, "sldNum"), foot: foot_of(at, "ftr"), soft };
        let under_cover = layouts[cover].held.iter().find(|holder| holder.kind == "subTitle").and_then(|holder| holder.frame.map(|frame| Holder { tag: holder.tag.clone(), frame, sizes: text_sizes(holder.size.unwrap_or(24.0), 32.0, 12.0) }));
        // The text of a slide: the largest of the layout's places for one.
        let above = from_master("body");
        let area = |holder: &Held| holder.frame.or(above.and_then(|above| above.frame)).map_or(0.0, |frame| frame.w * frame.h);
        let text = layouts[content].held.iter().filter(|holder| holder.takes_text()).max_by(|a, b| area(a).total_cmp(&area(b)));
        let insets: Vec<f32> = (0..4).map(|n| text.and_then(|holder| holder.insets[n]).or(above.and_then(|above| above.insets[n])).unwrap_or(INSETS[n])).collect();
        let under_content = Holder {
            tag: text.or(above).map_or("<p:ph type=\"body\" idx=\"1\"/>".to_string(), |holder| holder.tag.clone()),
            frame: text.and_then(|holder| holder.frame).or(above.and_then(|above| above.frame)).unwrap_or(scaled(56.0, 122.0, 848.0, 358.0)),
            sizes: text_sizes(text.and_then(|holder| holder.size).or(above.and_then(|above| above.size)).or(master.body.0).unwrap_or(24.0), 28.0, 14.0),
        };
        // Small text under a picture and at the foot: the colour of the slide's text, part of the way to the background's where that is one colour.
        let ink = text.and_then(|holder| holder.colour.clone()).or(above.and_then(|above| above.colour.clone())).or(master.body.1.clone()).unwrap_or_else(|| master.text.clone());
        let soft = match layouts[content].ground.as_ref().unwrap_or(&master.ground) {
            Ground::Plain(ground) => mix(&ink, ground, 0.35),
            _ => ink,
        };
        let stage = Stage {
            wide,
            high,
            cover: place(cover, title_of(cover, scaled(56.0, 150.0, 848.0, 150.0), 40.0), under_cover, plain(&title_colour(cover))),
            opening: place(opening, title_of(opening, scaled(56.0, 190.0, 848.0, 120.0), 36.0), None, plain(&title_colour(opening))),
            content: place(content, title_of(content, scaled(56.0, 40.0, 848.0, 64.0), 30.0), Some(under_content), plain(&soft)),
            pad: (insets[0] + insets[1], insets[2] + insets[3]),
            margins: master.margins,
        };

        // What a new deck keeps: everything the presentation leads to but its slides, and what that leads to.
        let mut kept: Vec<String> = vec![];
        let mut queue: Vec<String> = listed.iter().filter(|rel| !rel.external && KEPT.iter().any(|kind| rel.is(kind))).map(|rel| beside(&package.main, &rel.target)).collect();
        while let Some(part) = queue.pop() {
            if kept.contains(&part) || !package.zip.has(&part) { continue }
            queue.extend(package.relations(&part).into_iter().filter(|rel| !rel.external && !rel.is("slide")).map(|rel| beside(&part, &rel.target)));
            kept.push(part);
        }
        kept.sort();
        let notes_master = listed.iter().find(|rel| rel.is("notesMaster") && !rel.external).map(|rel| beside(&package.main, &rel.target)).filter(|part| kept.contains(part));
        let theme = masters[home].theme.clone();
        let drawn = said >= 3 && filled * 2 < said;
        Ok(Reference { package, wide, high, slides, layouts, theme, cover, opening, content, drawn, stage, kept, notes_master })
    }

    pub(crate) fn stage(&self) -> &Stage {
        &self.stage
    }

    /// The presentation's part.
    pub(crate) fn main(&self) -> &str {
        &self.package.main
    }

    /// The folder the presentation is in, under which its slides are: `ppt`.
    pub(crate) fn folder(&self) -> &str {
        self.package.main.rsplit_once('/').map_or("", |(folder, _)| folder)
    }

    /// The reference's own master of notes, when it has one.
    pub(crate) fn notes_master(&self) -> Option<&str> {
        self.notes_master.as_deref()
    }

    /// The parts a new deck keeps, each with its relations: (name, content).
    pub(crate) fn kept(&self) -> Result<Vec<(String, Vec<u8>)>> {
        let mut out = vec![];
        for part in &self.kept {
            for name in [part.clone(), rels_of(part)] {
                if let Some(bytes) = self.package.zip.get(&name).map_err(|_| anyhow!(NOT_PPTX))? { out.push((name, bytes)) }
            }
        }
        Ok(out)
    }

    /// The presentation for a deck of this many slides (`slides/slide<n>.xml` beside it), and its relations: what it
    /// said of its own slides is gone. `notes` is a master of notes the deck brings along.
    pub(crate) fn presentation(&self, slides: usize, notes: Option<&str>) -> Result<(String, String)> {
        let main = &self.package.main;
        let mut xml = self.package.text(main);
        for name in DROPPED { xml = without(&xml, name) }
        let all = self.package.relations(main);
        let stays = |rel: &Rel| KEPT.iter().any(|kind| rel.is(kind)) && (rel.external || self.kept.contains(&beside(main, &rel.target)));
        // A list that names a part that is not there any more names nothing.
        for (list, kind) in [("p:notesMasterIdLst", "notesMaster"), ("p:handoutMasterIdLst", "handoutMaster")] {
            if !all.iter().any(|rel| rel.is(kind) && stays(rel)) { xml = without(&xml, list) }
        }
        let mut last = all.iter().filter_map(|rel| rel.id.trim_start_matches(|c: char| !c.is_ascii_digit()).parse::<usize>().ok()).max().unwrap_or(0);
        let mut next = || {
            last += 1;
            format!("rId{last}")
        };
        let relation = |id: &str, kind: &str, target: &str| format!("<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"/>", escape(id), escape(kind), escape(target));
        let base = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        let mut listed: String = all.iter().filter(|rel| stays(rel)).map(|rel| relation(&rel.id, &rel.kind, &rel.target)).collect();
        // The lists of a presentation come in a set order: masters, the notes' master, the handout's, then the slides.
        let after = |xml: &str, names: &[&str]| names.iter().filter_map(|name| find(xml, name, 0)).map(|(_, end)| end).max();
        if let Some(part) = notes {
            let id = next();
            listed.push_str(&relation(&id, &format!("{base}/notesMaster"), &format!("/{part}")));
            let at = after(&xml, &["p:sldMasterIdLst"]).ok_or_else(|| anyhow!(NOT_PPTX))?;
            xml.insert_str(at, &format!("<p:notesMasterIdLst><p:notesMasterId r:id=\"{id}\"/></p:notesMasterIdLst>"));
        }
        let mut ids = String::new();
        for n in 0..slides {
            let id = next();
            listed.push_str(&relation(&id, &format!("{base}/slide"), &format!("slides/slide{}.xml", n + 1)));
            ids.push_str(&format!("<p:sldId id=\"{}\" r:id=\"{id}\"/>", 256 + n));
        }
        let at = after(&xml, &["p:sldMasterIdLst", "p:notesMasterIdLst", "p:handoutMasterIdLst"]).ok_or_else(|| anyhow!(NOT_PPTX))?;
        xml.insert_str(at, &format!("<p:sldIdLst>{ids}</p:sldIdLst>"));
        Ok((xml, format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{listed}</Relationships>")))
    }

    /// What kind of content each part of the new file is: the reference's word for the parts kept, and `added` (part, kind).
    pub(crate) fn content_types(&self, added: &[(String, String)]) -> Result<String> {
        let xml = self.package.text("[Content_Types].xml");
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">");
        let mut endings: Vec<String> = vec![];
        for tag in every(&xml, "Default").into_iter().map(open_tag) {
            let (Some(ending), Some(kind)) = (attr(tag, "Extension"), attr(tag, "ContentType")) else { continue };
            out.push_str(&format!("<Default Extension=\"{}\" ContentType=\"{}\"/>", escape(&ending), escape(&kind)));
            endings.push(ending.to_lowercase());
        }
        for (ending, kind) in [("rels", "application/vnd.openxmlformats-package.relationships+xml"), ("xml", "application/xml"), ("png", "image/png"), ("jpg", "image/jpeg")] {
            if !endings.iter().any(|known| known == ending) { out.push_str(&format!("<Default Extension=\"{ending}\" ContentType=\"{kind}\"/>")) }
        }
        out.push_str(&format!("<Override PartName=\"/{}\" ContentType=\"{MAIN_TYPE}\"/>", escape(&self.package.main)));
        for tag in every(&xml, "Override").into_iter().map(open_tag) {
            let (Some(part), Some(kind)) = (attr(tag, "PartName"), attr(tag, "ContentType")) else { continue };
            if self.kept.iter().any(|kept| kept == part.trim_start_matches('/')) { out.push_str(&format!("<Override PartName=\"{}\" ContentType=\"{}\"/>", escape(&part), escape(&kind))) }
        }
        for (part, kind) in added { out.push_str(&format!("<Override PartName=\"/{}\" ContentType=\"{}\"/>", escape(part), escape(kind))) }
        out.push_str("</Types>");
        Ok(out)
    }
}

/// What the assistant is told with a question that came with PowerPoint files: where they are and what they are for.
pub fn note(paths: &[String]) -> String {
    let files: Vec<String> = paths.iter().map(|path| format!("- {path}")).collect();
    format!("用户随这个问题附上了 PowerPoint 文件（本机路径）：\n{}\n这是做幻灯片时的参考，不是文献，也不会收进文献库。先用 read_slides 读它：每页的标题和要点（它怎样分节、每页写多少、用什么语气）、用到的版式和主题。之后用 make_slides 做幻灯片时，把它的路径填进 reference：新文件就建在它的母版和版式上（背景、配色、字体、标志都和它一样），内容仍由你按文献来写；用户要沿用它的讲法时，按它的分节和详略来排页。参考文件本身不会被改动，它的幻灯片也不会出现在新文件里。这次对话里之后再做幻灯片，除非用户另说，都沿用这份参考。用户只交来文件、没说做什么时，读完后用几句话说清它是怎样一份幻灯片（多少页、怎样分节、什么版式），再问用户要照它做哪篇文献或哪些内容的幻灯片。", files.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pptx::{Deck, Picture, Point, Slide, write, write_on};

    const SPACES: &str = "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"";
    const BASE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    const GROUP: &str = "<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>";

    fn related(of: &[(&str, &str, &str)]) -> String {
        let all: String = of.iter().map(|(id, kind, target)| format!("<Relationship Id=\"{id}\" Type=\"{BASE}/{kind}\" Target=\"{target}\"/>")).collect();
        format!("<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{all}</Relationships>")
    }

    /// A shape with text: in a placeholder (`<p:ph …/>`) or a box of its own, at a place in points or where its layout has it.
    fn shape(holder: &str, at: Option<[f32; 4]>, room: &str, levels: &str, paragraphs: &str) -> String {
        let place = at.map(|[x, y, w, h]| format!("<a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>", (x * 12700.0) as i64, (y * 12700.0) as i64, (w * 12700.0) as i64, (h * 12700.0) as i64)).unwrap_or_default();
        format!("<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"x\"/><p:cNvSpPr/><p:nvPr>{holder}</p:nvPr></p:nvSpPr><p:spPr>{place}</p:spPr><p:txBody><a:bodyPr{room}/><a:lstStyle>{levels}</a:lstStyle>{paragraphs}</p:txBody></p:sp>")
    }

    fn said(level: usize, text: &str) -> String {
        format!("<a:p><a:pPr lvl=\"{level}\"/><a:r><a:rPr lang=\"zh-CN\"/><a:t>{text}</a:t></a:r></a:p>")
    }

    fn picture(relation: &str) -> String {
        format!("<p:pic><p:nvPicPr><p:cNvPr id=\"9\" name=\"p\"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed=\"{relation}\"/></p:blipFill><p:spPr/></p:pic>")
    }

    /// A lab's deck as PowerPoint writes one: a dark master with a mark, four layouts, a title page, a part and two
    /// slides of points, and around them what a new deck must not keep (sections, a custom show, the views, a
    /// printer's settings, the count of slides). `drawn` writes the slides' words as boxes of their own.
    fn lab(drawn: bool) -> Vec<u8> {
        let mut zip = Zip::default();
        let mut types = String::from("<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"bin\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.printerSettings\"/>");
        let mut part = |zip: &mut Zip, name: &str, kind: &str, xml: String| {
            types.push_str(&format!("<Override PartName=\"/{name}\" ContentType=\"application/vnd.openxmlformats-officedocument.{kind}+xml\"/>"));
            zip.set(name, xml.as_bytes());
        };
        let colours: String = [("dk1", "141E30"), ("lt1", "F2F2F2"), ("dk2", "1F497D"), ("lt2", "EEECE1"), ("accent1", "E0A830")].iter().map(|(name, colour)| format!("<a:{name}><a:srgbClr val=\"{colour}\"/></a:{name}>")).collect();
        part(&mut zip, "ppt/theme/theme1.xml", "theme", format!("<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"课题组\"><a:themeElements><a:clrScheme name=\"x\">{colours}</a:clrScheme><a:fontScheme name=\"x\"><a:majorFont><a:latin typeface=\"Georgia\"/><a:ea typeface=\"\"/><a:font script=\"Hans\" typeface=\"黑体\"/></a:majorFont><a:minorFont><a:latin typeface=\"Verdana\"/><a:ea typeface=\"\"/><a:font script=\"Hans\" typeface=\"宋体\"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"));
        let master = [
            picture("rId6"),
            shape("<p:ph type=\"title\"/>", Some([36.0, 20.0, 648.0, 90.0]), "", "", &said(0, "Click to edit")),
            shape("<p:ph type=\"body\" idx=\"1\"/>", Some([36.0, 126.0, 648.0, 356.0]), " lIns=\"127000\" rIns=\"127000\" tIns=\"63500\" bIns=\"63500\"", "", &said(0, "Click to edit")),
            shape("<p:ph type=\"ftr\" sz=\"quarter\" idx=\"3\"/>", Some([246.0, 500.0, 228.0, 28.0]), "", "", ""),
            shape("<p:ph type=\"sldNum\" sz=\"quarter\" idx=\"4\"/>", Some([516.0, 500.0, 168.0, 28.0]), "", "", ""),
        ].concat();
        part(&mut zip, "ppt/slideMasters/slideMaster1.xml", "presentationml.slideMaster", format!("<p:sldMaster {SPACES}><p:cSld><p:bg><p:bgPr><a:solidFill><a:schemeClr val=\"bg1\"/></a:solidFill></p:bgPr></p:bg><p:spTree>{GROUP}{master}</p:spTree></p:cSld><p:clrMap bg1=\"dk1\" tx1=\"lt1\" bg2=\"dk2\" tx2=\"lt2\" accent1=\"accent1\"/><p:sldLayoutIdLst><p:sldLayoutId id=\"2147483649\" r:id=\"rId1\"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr algn=\"ctr\"><a:defRPr sz=\"4000\"><a:solidFill><a:srgbClr val=\"E0A830\"/></a:solidFill></a:defRPr></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr marL=\"342900\" indent=\"-342900\"><a:defRPr sz=\"3200\"><a:solidFill><a:schemeClr val=\"tx1\"/></a:solidFill></a:defRPr></a:lvl1pPr><a:lvl2pPr marL=\"742950\" indent=\"-285750\"><a:defRPr sz=\"2800\"/></a:lvl2pPr></p:bodyStyle></p:txStyles></p:sldMaster>"));
        zip.set("ppt/slideMasters/_rels/slideMaster1.xml.rels", related(&[("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"), ("rId2", "slideLayout", "../slideLayouts/slideLayout2.xml"), ("rId3", "slideLayout", "../slideLayouts/slideLayout3.xml"), ("rId4", "slideLayout", "../slideLayouts/slideLayout4.xml"), ("rId5", "theme", "../theme/theme1.xml"), ("rId6", "image", "../media/image1.png")]).as_bytes());
        let foot = [shape("<p:ph type=\"ftr\" sz=\"quarter\" idx=\"11\"/>", None, "", "", ""), shape("<p:ph type=\"sldNum\" sz=\"quarter\" idx=\"12\"/>", None, "", "", "")].concat();
        let layouts = [
            ("title", "标题幻灯片", [shape("<p:ph type=\"ctrTitle\"/>", Some([54.0, 168.0, 612.0, 116.0]), "", "<a:lvl1pPr><a:defRPr sz=\"5400\"><a:solidFill><a:srgbClr val=\"FFFFFF\"/></a:solidFill></a:defRPr></a:lvl1pPr>", ""), shape("<p:ph type=\"subTitle\" idx=\"1\"/>", Some([108.0, 306.0, 504.0, 138.0]), "", "<a:lvl1pPr><a:defRPr sz=\"2000\"/></a:lvl1pPr>", "")].concat()),
            ("obj", "标题和内容", [shape("<p:ph type=\"title\"/>", None, "", "", ""), shape("<p:ph idx=\"1\"/>", None, "", "", ""), foot.clone()].concat()),
            ("secHead", "节标题", [shape("<p:ph type=\"title\"/>", Some([57.0, 347.0, 612.0, 107.0]), "", "<a:lvl1pPr><a:defRPr sz=\"3600\"/></a:lvl1pPr>", ""), shape("<p:ph type=\"body\" idx=\"1\"/>", Some([57.0, 229.0, 612.0, 118.0]), "", "", "")].concat()),
            ("blank", "空白", foot),
        ];
        for (n, (kind, name, held)) in layouts.iter().enumerate() {
            part(&mut zip, &format!("ppt/slideLayouts/slideLayout{}.xml", n + 1), "presentationml.slideLayout", format!("<p:sldLayout {SPACES} type=\"{kind}\"><p:cSld name=\"{name}\"><p:spTree>{GROUP}{held}</p:spTree></p:cSld></p:sldLayout>"));
            zip.set(&format!("ppt/slideLayouts/_rels/slideLayout{}.xml.rels", n + 1), related(&[("rId1", "slideMaster", "../slideMasters/slideMaster1.xml")]).as_bytes());
        }
        // The words of a slide: in the layout's placeholders, or as boxes of the slide's own on the blank layout.
        let held = |holder: &str| if drawn { String::new() } else { holder.to_string() };
        let points = [said(0, "做了什么 &amp; 怎么做"), said(1, "数据在哪"), said(0, "下一步")].concat();
        let slides = [
            (1, [shape(&held("<p:ph type=\"ctrTitle\"/>"), None, "", "", &said(0, "课题组周报")), shape(&held("<p:ph type=\"subTitle\" idx=\"1\"/>"), None, "", "", &said(0, "李四 · 第 12 周"))].concat()),
            (3, shape(&held("<p:ph type=\"title\"/>"), None, "", "", &said(0, "进展"))),
            (2, [shape(&held("<p:ph type=\"title\"/>"), None, "", "", &said(0, "第一项")), shape(&held("<p:ph idx=\"1\"/>"), None, "", "", &points), picture("rId2"), shape("<p:ph type=\"sldNum\" sz=\"quarter\" idx=\"12\"/>", None, "", "", &said(0, "3"))].concat()),
            (2, [shape(&held("<p:ph type=\"title\"/>"), None, "", "", &said(0, "第二项")), shape(&held("<p:ph idx=\"1\"/>"), None, "", "", &points), "<p:graphicFrame><a:graphic><a:graphicData><a:tbl><a:tr><a:tc><a:txBody><a:p><a:r><a:t>组</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:p><a:r><a:t>细胞数</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>".to_string()].concat()),
        ];
        for (n, (layout, shapes)) in slides.iter().enumerate() {
            let layout = if drawn { 4 } else { *layout };
            part(&mut zip, &format!("ppt/slides/slide{}.xml", n + 1), "presentationml.slide", format!("<p:sld {SPACES}><p:cSld><p:spTree>{GROUP}{shapes}</p:spTree></p:cSld></p:sld>"));
            let mut leads = vec![("rId1".to_string(), "slideLayout", format!("../slideLayouts/slideLayout{layout}.xml"))];
            if n == 2 { leads.extend([("rId2".to_string(), "image", "../media/photo.png".to_string()), ("rId3".to_string(), "notesSlide", "../notesSlides/notesSlide3.xml".to_string())]) }
            zip.set(&format!("ppt/slides/_rels/slide{}.xml.rels", n + 1), related(&leads.iter().map(|(id, kind, target)| (id.as_str(), *kind, target.as_str())).collect::<Vec<_>>()).as_bytes());
        }
        part(&mut zip, "ppt/notesSlides/notesSlide3.xml", "presentationml.notesSlide", format!("<p:notes {SPACES}><p:cSld><p:spTree>{GROUP}{}{}</p:spTree></p:cSld></p:notes>", shape("<p:ph type=\"sldImg\"/>", None, "", "", ""), shape("<p:ph type=\"body\" idx=\"1\"/>", None, "", "", &[said(0, "先讲背景。"), said(0, "再讲数据。")].concat())));
        part(&mut zip, "ppt/presProps.xml", "presentationml.presProps", format!("<p:presentationPr {SPACES}/>"));
        part(&mut zip, "ppt/viewProps.xml", "presentationml.viewProps", format!("<p:viewPr {SPACES}/>"));
        part(&mut zip, "docProps/app.xml", "extended-properties", "<Properties><Slides>4</Slides></Properties>".into());
        zip.set("ppt/media/image1.png", b"the lab's mark");
        zip.set("ppt/media/photo.png", b"a photograph of one slide");
        zip.set("ppt/printerSettings/printerSettings1.bin", b"a printer");
        // Saved as a template: the same parts under another word for the whole.
        types.push_str("<Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.template.main+xml\"/></Types>");
        zip.set("[Content_Types].xml", types.as_bytes());
        zip.set("_rels/.rels", related(&[("rId1", "officeDocument", "ppt/presentation.xml"), ("rId2", "extended-properties", "docProps/app.xml")]).as_bytes());
        zip.set("ppt/presentation.xml", format!("<p:presentation {SPACES} xmlns:p14=\"http://schemas.microsoft.com/office/powerpoint/2010/main\"><p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId1\"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id=\"256\" r:id=\"rId10\"/><p:sldId id=\"257\" r:id=\"rId11\"/><p:sldId id=\"258\" r:id=\"rId12\"/><p:sldId id=\"259\" r:id=\"rId13\"/></p:sldIdLst><p:sldSz cx=\"9144000\" cy=\"6858000\" type=\"screen4x3\"/><p:notesSz cx=\"6858000\" cy=\"9144000\"/><p:custShowLst><p:custShow name=\"短版\" id=\"0\"><p:sldLst><p:sld r:id=\"rId10\"/></p:sldLst></p:custShow></p:custShowLst><p:defaultTextStyle/><p:extLst><p:ext uri=\"{{521415D9-36F7-43E2-AB2F-B90AF26B5E84}}\"><p14:sectionLst><p14:section name=\"进展\" id=\"{{A}}\"><p14:sldIdLst><p14:sldId id=\"257\"/></p14:sldIdLst></p14:section></p14:sectionLst></p:ext></p:extLst></p:presentation>").as_bytes());
        zip.set("ppt/_rels/presentation.xml.rels", related(&[("rId1", "slideMaster", "slideMasters/slideMaster1.xml"), ("rId2", "theme", "theme/theme1.xml"), ("rId3", "presProps", "presProps.xml"), ("rId4", "viewProps", "viewProps.xml"), ("rId5", "printerSettings", "printerSettings/printerSettings1.bin"), ("rId10", "slide", "slides/slide1.xml"), ("rId11", "slide", "slides/slide2.xml"), ("rId12", "slide", "slides/slide3.xml"), ("rId13", "slide", "slides/slide4.xml")]).as_bytes());
        zip.write()
    }

    /// Every tag of a part closes the one opened last.
    fn well_formed(xml: &str) -> bool {
        let (mut open, mut at) = (vec![], 0);
        while let Some((piece, end)) = node(xml, at) {
            match piece {
                Node::Open(name, _) => open.push(name),
                Node::Close if open.pop().is_none_or(|name| !xml[at..end].starts_with(&format!("</{name}"))) => return false,
                _ => {}
            }
            at = end;
        }
        open.is_empty()
    }

    fn talk() -> Deck {
        let cell = Picture { bytes: b"a picture of cells".to_vec(), kind: "png", width: 1200, height: 900 };
        Deck {
            title: "根尖干细胞微环境的单细胞图谱".into(),
            subtitle: "Zhang et al., 2021 · Plant Cell".into(),
            date: "2026-11-23T08:00:00.000Z".into(),
            slides: vec![
                Slide { title: "背景".into(), ..Default::default() },
                Slide { title: "WOX5 维持静止中心".into(), points: ["**WOX5** 只在静止中心表达", "  敲除后干细胞分化 & 根变短"].iter().map(|line| Point::read(line)).collect(), source: "Zhang et al., 2021 · p. 3".into(), notes: "先讲表达。".into(), ..Default::default() },
                Slide { title: "单细胞图谱".into(), points: vec![Point::read("共 12 个细胞群")], picture: Some(cell), caption: "Fig. 1 | UMAP of root tip cells.".into(), ..Default::default() },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_reference_is_read_for_what_it_says_and_how_it_looks() {
        let reference = Reference::read(&lab(false)).unwrap();
        assert_eq!((reference.wide, reference.high), (720.0, 540.0));
        assert_eq!(reference.slides.iter().map(|slide| slide.title.as_str()).collect::<Vec<_>>(), ["课题组周报", "进展", "第一项", "第二项"]);
        assert_eq!(reference.slides[0].lines, [(0, "李四 · 第 12 周".to_string())]);
        // Levels are kept, what every slide repeats (its number) is not among a slide's words, a table is a line a row.
        assert_eq!(reference.slides[2].lines, [(0, "做了什么 & 怎么做".to_string()), (1, "数据在哪".into()), (0, "下一步".into())]);
        assert_eq!((reference.slides[2].pictures, reference.slides[2].notes.as_str()), (1, "先讲背景。\n再讲数据。"));
        assert_eq!(reference.slides[3].lines.last().unwrap(), &(0, "组 | 细胞数".to_string()));
        assert_eq!(reference.layouts.iter().map(|layout| (layout.name.as_str(), layout.used)).collect::<Vec<_>>(), [("标题幻灯片", 1), ("标题和内容", 2), ("节标题", 1), ("空白", 0)]);
        // The layouts are the ones the file's own slides stand on.
        assert_eq!((reference.cover, reference.opening, reference.content), (0, 2, 1));
        assert!(!reference.drawn);
        // The master says which of the theme's colours text and background are.
        assert_eq!(reference.theme, Theme { name: "课题组".into(), heading: "Georgia".into(), body: "Verdana".into(), chinese: "宋体".into(), text: "#F2F2F2".into(), background: "#141E30".into(), accent: "#E0A830".into() });

        let stage = reference.stage();
        // A layout that says no place for a title or a text has the master's; the sizes begin at what the layout or the master gives.
        assert_eq!(stage.content.title, Holder { tag: "<p:ph type=\"title\"/>".into(), frame: Frame { x: 36.0, y: 20.0, w: 648.0, h: 90.0 }, sizes: vec![40.0, 35.0, 30.0, 25.0, 20.0] });
        let under = stage.content.under.as_ref().unwrap();
        assert_eq!((under.tag.as_str(), under.frame, under.sizes.as_slice()), ("<p:ph idx=\"1\"/>", Frame { x: 36.0, y: 126.0, w: 648.0, h: 356.0 }, [28.0, 26.0, 24.0, 22.0, 20.0, 18.0].as_slice()));
        assert_eq!((stage.content.number.as_deref(), stage.content.foot.as_deref()), (Some("<p:ph type=\"sldNum\" sz=\"quarter\" idx=\"12\"/>"), Some("<p:ph type=\"ftr\" sz=\"quarter\" idx=\"11\"/>")));
        assert_eq!((stage.cover.title.tag.as_str(), stage.cover.title.sizes[0], stage.cover.under.as_ref().unwrap().tag.as_str()), ("<p:ph type=\"ctrTitle\"/>", 54.0, "<p:ph type=\"subTitle\" idx=\"1\"/>"));
        assert_eq!((stage.opening.layout.as_str(), stage.opening.title.frame.y, stage.opening.title.sizes[0], stage.opening.number.is_none()), ("ppt/slideLayouts/slideLayout3.xml", 347.0, 36.0, true));
        // The room the master's text keeps free, how far its levels stand in, and small text between the text's colour and the background's.
        assert_eq!((stage.pad, stage.margins), ((20.0, 10.0), [27.0, 58.5]));
        assert_eq!(stage.content.soft, format!("<a:srgbClr val=\"{}\"/>", mix("F2F2F2", "141E30", 0.35)));
        assert_eq!(stage.cover.soft, "<a:srgbClr val=\"FFFFFF\"/>");

        // Boxes drawn on blank layouts: nothing of the file's own use says which layouts to take, so their kind does.
        let drawn = Reference::read(&lab(true)).unwrap();
        assert!(drawn.drawn && drawn.slides.iter().all(|slide| slide.title.is_empty()) && drawn.slides[2].lines.len() == 4);
        assert_eq!((drawn.cover, drawn.opening, drawn.content), (0, 2, 1));
    }

    #[test]
    fn a_deck_stands_on_the_masters_and_layouts_of_a_reference() {
        let reference = Reference::read(&lab(false)).unwrap();
        let written = write_on(&talk(), &reference).unwrap();
        assert_eq!((written.pages, written.continued.len()), (4, 0));
        let zip = Zip::read(&written.bytes).unwrap();
        let names: Vec<&str> = zip.names().collect();
        // The masters, the layouts, the theme and the mark are the reference's; its slides and what only they needed are gone.
        for kept in ["ppt/slideMasters/slideMaster1.xml", "ppt/slideMasters/_rels/slideMaster1.xml.rels", "ppt/slideLayouts/slideLayout4.xml", "ppt/theme/theme1.xml", "ppt/media/image1.png", "ppt/presProps.xml"] { assert!(names.contains(&kept), "{kept}") }
        for gone in ["ppt/media/photo.png", "ppt/viewProps.xml", "ppt/printerSettings/printerSettings1.bin", "docProps/app.xml", "ppt/slides/slide5.xml", "ppt/notesSlides/notesSlide4.xml"] { assert!(!names.contains(&gone), "{gone}") }
        assert_eq!(zip.get("ppt/media/image1.png").unwrap().unwrap(), b"the lab's mark");
        let every: String = names.iter().filter(|name| name.ends_with(".xml") || name.ends_with(".rels")).map(|name| zip.text(name).unwrap()).collect();
        for word in ["课题组周报", "第一项", "数据在哪", "先讲背景", "sectionLst", "custShow"] { assert!(!every.contains(word), "{word}") }
        for name in names.iter().filter(|name| name.ends_with(".xml") || name.ends_with(".rels")) { assert!(well_formed(&zip.text(name).unwrap()), "{name}") }

        // The presentation lists the new slides after its masters and the notes' master the deck brought along, each by a relation that leads to a part.
        let presentation = zip.text("ppt/presentation.xml").unwrap();
        let order: Vec<usize> = ["<p:sldMasterIdLst>", "<p:notesMasterIdLst>", "<p:sldIdLst>", "<p:sldSz", "<p:defaultTextStyle"].iter().map(|tag| presentation.find(tag).unwrap_or_else(|| panic!("{tag}"))).collect();
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(every_one(&presentation, "p:sldId").len(), 4);
        let listed = relations(&zip.text("ppt/_rels/presentation.xml.rels").unwrap());
        assert!(!listed.iter().any(|rel| rel.is("viewProps") || rel.is("printerSettings")));
        let mut ids: Vec<&str> = listed.iter().map(|rel| rel.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), listed.len());
        for (at, _) in presentation.match_indices("r:id=\"") {
            let id = presentation[at + 6..].split('"').next().unwrap();
            let rel = listed.iter().find(|rel| rel.id == id).unwrap_or_else(|| panic!("{id}"));
            assert!(names.contains(&beside("ppt/presentation.xml", &rel.target).as_str()), "{}", rel.target);
        }
        // Every relation of every part leads to a part of the file.
        for name in names.iter().filter(|name| name.ends_with(".rels")) {
            let part = name.replace("_rels/", "").trim_end_matches(".rels").to_string();
            for rel in relations(&zip.text(name).unwrap()) { assert!(names.contains(&beside(&part, &rel.target).as_str()), "{name}: {}", rel.target) }
        }
        // Every part has its kind of content said, and nothing is said of a part that is not there.
        let types = zip.text("[Content_Types].xml").unwrap();
        assert!(types.contains(&format!("<Override PartName=\"/ppt/presentation.xml\" ContentType=\"{MAIN_TYPE}\"/>")) && !types.contains("template.main"));
        for name in names.iter().filter(|name| name.ends_with(".xml") && !name.starts_with('[')) { assert!(types.contains(&format!("PartName=\"/{name}\"")), "{name}") }
        for tag in every_one(&types, "Override") { assert!(names.contains(&attr(open_tag(tag), "PartName").unwrap().trim_start_matches('/')), "{tag}") }

        // The first slide fills the title layout's two places, at the largest size the title fits them.
        let cover = zip.text("ppt/slides/slide1.xml").unwrap();
        assert!(zip.text("ppt/slides/_rels/slide1.xml.rels").unwrap().contains("../slideLayouts/slideLayout1.xml"));
        assert!(cover.contains("<p:ph type=\"ctrTitle\"/>") && cover.contains("<p:ph type=\"subTitle\" idx=\"1\"/>") && !cover.contains("slidenum"));
        // 54 points is the layout's size for a title: this one takes two lines there, and two lines stay inside the place from 41 down.
        assert_eq!(cover.match_indices("sz=\"").map(|(at, _)| cover[at + 4..].split('"').next().unwrap()).collect::<Vec<_>>(), ["4100", "2000"]);
        // How the words look is the layout's: no colour, no font, no mark of a point, no place across the line is written.
        let opening = zip.text("ppt/slides/slide2.xml").unwrap();
        assert!(zip.text("ppt/slides/_rels/slide2.xml.rels").unwrap().contains("../slideLayouts/slideLayout3.xml") && opening.contains("背景") && !opening.contains("Rule"));
        let points = zip.text("ppt/slides/slide3.xml").unwrap();
        assert!(zip.text("ppt/slides/_rels/slide3.xml.rels").unwrap().contains("../slideLayouts/slideLayout2.xml"));
        assert!(points.contains("<p:ph idx=\"1\"/>") && points.contains("<a:pPr lvl=\"1\">") && points.contains("<a:rPr lang=\"zh-CN\" sz=\"2800\" b=\"1\" dirty=\"0\"></a:rPr><a:t>WOX5</a:t>"));
        assert!(!points.contains("solidFill") && !points.contains("buChar") && !points.contains("algn=") && !points.contains("latin"));
        // The line at the foot and the number stand in the layout's own places for them.
        assert!(points.contains("<p:ph type=\"ftr\" sz=\"quarter\" idx=\"11\"/></p:nvPr></p:nvSpPr><p:spPr></p:spPr>") && points.contains("Zhang et al., 2021 · p. 3") && points.contains("<p:ph type=\"sldNum\" sz=\"quarter\" idx=\"12\"/>") && points.contains("type=\"slidenum\""));
        // A picture and its caption are the slide's own: beside the points inside the layout's place for text, the caption in a colour that reads on the background.
        let shown = zip.text("ppt/slides/slide4.xml").unwrap();
        assert!(shown.contains("<p:pic>") && shown.contains(&format!("<a:srgbClr val=\"{}\"/>", mix("F2F2F2", "141E30", 0.35))) && shown.contains("Fig. 1 | UMAP"));
        // Its file has a name the reference's mark does not.
        assert!(zip.text("ppt/slides/_rels/slide4.xml.rels").unwrap().contains("../media/image2.png") && zip.get("ppt/media/image2.png").unwrap().unwrap() == b"a picture of cells");
        // The notes hang on a master of notes with a theme of its own, under names that were free.
        assert!(zip.text("ppt/notesSlides/notesSlide3.xml").unwrap().contains("先讲表达。") && zip.text("ppt/notesSlides/_rels/notesSlide3.xml.rels").unwrap().contains("../notesMasters/notesMaster1.xml"));
        assert!(zip.text("ppt/notesMasters/_rels/notesMaster1.xml.rels").unwrap().contains("../theme/theme2.xml") && names.contains(&"ppt/theme/theme2.xml"));

        // What was written is a reference in its turn, and reads as the deck: nothing of the first reference's slides came along.
        let again = Reference::read(&written.bytes).unwrap();
        assert_eq!(again.slides.iter().map(|slide| slide.title.as_str()).collect::<Vec<_>>(), ["根尖干细胞微环境的单细胞图谱", "背景", "WOX5 维持静止中心", "单细胞图谱"]);
        assert_eq!(again.slides[2].lines, [(0, "WOX5 只在静止中心表达".to_string()), (1, "敲除后干细胞分化 & 根变短".into())]);
        assert_eq!((again.cover, again.opening, again.content, again.notes_master()), (0, 2, 1, Some("ppt/notesMasters/notesMaster1.xml")));
        // A deck without notes on a reference without a master for them brings none.
        let quiet = write_on(&Deck { slides: vec![Slide { title: "只有这一页".into(), points: vec![Point::read("一点")], ..Default::default() }], ..Default::default() }, &reference).unwrap();
        let zip = Zip::read(&quiet.bytes).unwrap();
        assert!(!zip.names().any(|name| name.contains("notes")) && !zip.text("ppt/presentation.xml").unwrap().contains("notesMasterIdLst") && quiet.pages == 1);
    }

    fn every_one<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
        every(xml, name)
    }

    #[test]
    fn a_deck_of_the_apps_own_is_a_reference_too_and_what_is_no_deck_is_refused() {
        // One master with one layout: all three kinds of slide stand on it.
        let own = write(&talk());
        let reference = Reference::read(&own.bytes).unwrap();
        assert_eq!((reference.wide, reference.layouts.len(), reference.cover, reference.opening, reference.content), (960.0, 1, 0, 0, 0));
        assert_eq!(reference.slides[2].title, "WOX5 维持静止中心");
        assert_eq!((reference.theme.heading.as_str(), reference.theme.text.as_str(), reference.theme.background.as_str()), ("Arial", "#1F2328", "#FFFFFF"));
        let written = write_on(&talk(), &reference).unwrap();
        let zip = Zip::read(&written.bytes).unwrap();
        assert!(zip.names().filter(|name| name.ends_with(".xml") || name.ends_with(".rels")).all(|name| well_formed(&zip.text(name).unwrap())));
        // Its own master of notes is used, not a second one.
        assert_eq!(zip.names().filter(|name| name.starts_with("ppt/notesMasters/") && name.ends_with(".xml")).count(), 1);
        assert_eq!(Reference::read(&written.bytes).unwrap().slides.len(), written.pages);

        for not in [b"not a zip at all".to_vec(), crate::docx::tests::file("<w:p/>", ""), { let mut zip = Zip::default(); zip.set("_rels/.rels", related(&[("rId1", "officeDocument", "ppt/presentation.xml")]).as_bytes()); zip.write() }] {
            assert!(Reference::read(&not).err().unwrap().to_string().contains("不是 PowerPoint"));
        }
        // A presentation without a master is none a deck can stand on.
        let mut bare = Zip::default();
        bare.set("_rels/.rels", related(&[("rId1", "officeDocument", "ppt/presentation.xml")]).as_bytes());
        bare.set("ppt/presentation.xml", format!("<p:presentation {SPACES}/>").as_bytes());
        assert!(Reference::read(&bare.write()).err().unwrap().to_string().contains("母版"));
    }

    #[test]
    fn a_long_title_takes_a_smaller_size_that_stays_in_its_place() {
        let reference = Reference::read(&lab(false)).unwrap();
        let long = "一个很长很长的标题，一行放不下，两行也未必放得下，所以要看它最后落在哪个字号上才算数";
        let written = write_on(&Deck { title: long.into(), slides: vec![Slide { title: long.into(), points: vec![Point::read("一点")], ..Default::default() }], ..Default::default() }, &reference).unwrap();
        let zip = Zip::read(&written.bytes).unwrap();
        let size = |name: &str| -> f32 { let xml = zip.text(name).unwrap(); let at = xml.find("sz=\"").unwrap(); xml[at + 4..].split('"').next().unwrap().parse::<f32>().unwrap() / 100.0 };
        // In the title page's place (612 by 116 points) and in the master's for a slide's title (648 by 90).
        let (cover, title) = (size("ppt/slides/slide1.xml"), size("ppt/slides/slide2.xml"));
        assert!((22.0..54.0).contains(&cover) && (18.0..40.0).contains(&title), "{cover} {title}");
    }
}
