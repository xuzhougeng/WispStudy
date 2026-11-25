//! 教程: the tutorials of the project page (`site/tutorials`, `site/en/tutorials`), read into blocks the window
//! draws. The pages are built into the program, so their words are there without the network and are those of
//! this build; their screenshots stay on the project page (all of them come to some 80 MB) and are fetched the
//! first time they are shown, into `cache_folder`. The site's HTML is the one source: a page is not written
//! twice, and what the window cannot read of it (the site's header, footer and language switch) is left out.

use crate::markdown::{Marks, Rich};
use std::path::PathBuf;

/// The project page, where the screenshots are and where a link out of the tutorials goes.
pub const SITE: &str = "https://xuzhougeng.github.io/WispStudy/";

macro_rules! pages {
    ($($slug:literal),* $(,)?) => {
        &[$(($slug, include_str!(concat!("../../../site/tutorials/", $slug, ".html")), include_str!(concat!("../../../site/en/tutorials/", $slug, ".html")))),*]
    };
}

/// Every page of the tutorials, in both languages: the shelf (`index`) and one per tutorial. A page added to
/// the site is added here too (the test `every_page_of_the_site_is_built_in` says which is missing).
const PAGES: &[(&str, &str, &str)] = pages!(
    "index", "start", "home", "migrate", "pdf", "read", "mindmap", "compare", "listen", "slides", "ask",
    "rank", "extract", "review", "write", "revise", "compare-writing", "follow", "libraries",
);

/// The HTML of a page, in the language asked for.
pub fn page(slug: &str, english: bool) -> Option<&'static str> {
    PAGES.iter().find(|(name, ..)| *name == slug).map(|(_, zh, en)| if english { *en } else { *zh })
}

/// Where the tutorials of a language are on the project page.
pub fn base(english: bool) -> String {
    format!("{SITE}{}tutorials/", if english { "en/" } else { "" })
}

/// A page's address on the project page: what 在浏览器中打开 opens.
pub fn page_url(slug: Option<&str>, english: bool) -> String {
    match slug {
        Some(slug) if slug != "index" => format!("{}{slug}.html", base(english)),
        _ => base(english),
    }
}

/// Where a fetched screenshot is kept: under the cache folder, at its path on the project page.
pub fn cached(url: &str) -> Option<PathBuf> {
    let path = url.strip_prefix(SITE)?;
    if path.split('/').any(|part| part.is_empty() || part == ".." || part == ".") { return None }
    Some(crate::settings::cache_folder().join("tutorials").join(path))
}

// ---- What a page is read into

/// The shelf: what the tutorials are, and each one under the part of the work it belongs to.
#[derive(Clone, Debug, Default)]
pub struct Shelf {
    pub kicker: String,
    pub title: String,
    pub lead: Rich,
    pub facts: Vec<(String, Rich)>,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, Default)]
pub struct Part {
    /// 收集, 阅读, …
    pub name: String,
    pub entries: Vec<Entry>,
}

/// A tutorial as the shelf lists it.
#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub slug: String,
    /// 01, 02, …
    pub number: String,
    pub title: String,
    pub summary: Rich,
    /// 需要 …, 用时 …
    pub needs: Vec<(String, String)>,
}

impl Entry {
    /// The title before its colon: what a narrow list has room for (「从一个课题开始」).
    pub fn short_title(&self) -> &str {
        let cut = [self.title.find('：'), self.title.find(": ")].into_iter().flatten().min();
        cut.map_or(self.title.as_str(), |at| &self.title[..at]).trim()
    }
}

impl Shelf {
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.parts.iter().flat_map(|part| part.entries.iter())
    }

    pub fn entry(&self, slug: &str) -> Option<&Entry> {
        self.entries().find(|entry| entry.slug == slug)
    }

    /// The tutorials before and after one, in the shelf's order.
    pub fn neighbours(&self, slug: &str) -> (Option<&Entry>, Option<&Entry>) {
        let all: Vec<&Entry> = self.entries().collect();
        let Some(at) = all.iter().position(|entry| entry.slug == slug) else { return (None, None) };
        (at.checked_sub(1).map(|before| all[before]), all.get(at + 1).copied())
    }
}

/// One tutorial: its head, its numbered sections, and the note at its foot on how it was made.
#[derive(Clone, Debug, Default)]
pub struct Tutorial {
    pub kicker: String,
    pub title: String,
    pub lead: Rich,
    pub facts: Vec<(String, Rich)>,
    pub sections: Vec<Section>,
    pub colophon: Vec<Rich>,
}

#[derive(Clone, Debug, Default)]
pub struct Section {
    /// What a link names it by: `s1`, `recap`.
    pub id: String,
    /// 1, 2, …; empty for 回顾.
    pub number: String,
    pub title: String,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Rich),
    Heading(Rich),
    List { ordered: bool, items: Vec<Rich> },
    /// What was typed or answered, quoted: its label (主题, 提问) and its paragraphs.
    Quote { cite: String, paragraphs: Vec<Rich> },
    /// A screenshot: its address on the project page, its size in pixels, and what it shows.
    Figure { src: String, alt: String, width: f32, height: f32, caption: Rich },
    /// A command and what it printed.
    Code { caption: String, text: String },
    /// 提示 or 注意 (`warn`).
    Note { warn: bool, label: String, text: Rich },
    /// Rows of cells; the first `head` of them are the header.
    Table { caption: Rich, head: usize, rows: Vec<Vec<Rich>> },
}

/// Where a link in a tutorial leads.
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    /// Another page of the tutorials (`index` is the shelf), at a section of it or its top.
    Page(String, Option<String>),
    /// Anywhere else: opened in the browser.
    Web(String),
}

/// `href`, written on the page `from`, as where it leads.
pub fn link(href: &str, from: &str, english: bool) -> Link {
    if let Some(anchor) = href.strip_prefix('#') { return Link::Page(from.to_string(), Some(anchor.to_string())) }
    let url = resolve(&base(english), href);
    if let Some(rest) = url.strip_prefix(&base(english)) {
        let (path, anchor) = match rest.split_once('#') {
            Some((path, anchor)) => (path, Some(anchor.to_string())),
            None => (rest, None),
        };
        let slug = if path.is_empty() { "index" } else { path.strip_suffix(".html").unwrap_or(path) };
        if page(slug, english).is_some() { return Link::Page(slug.to_string(), anchor) }
    }
    Link::Web(url)
}

/// An address written on a page of `base` (a folder, ending in `/`), made whole: `../img/t/a.jpg` → `…/img/t/a.jpg`.
pub fn resolve(base: &str, href: &str) -> String {
    if href.contains("://") || href.starts_with("mailto:") { return href.to_string() }
    let Some(scheme_end) = base.find("://").map(|at| at + 3) else { return href.to_string() };
    let root_end = base[scheme_end..].find('/').map_or(base.len(), |at| scheme_end + at);
    let (root, path) = base.split_at(root_end);
    let mut parts: Vec<&str> = if href.starts_with('/') { vec![] } else { path.split('/').filter(|part| !part.is_empty()).collect() };
    // The base is a folder: a page's address is resolved against the folder it is in.
    if !path.ends_with('/') { parts.pop(); }
    let (href, tail) = match href.find(['#', '?']) {
        Some(at) => href.split_at(at),
        None => (href, ""),
    };
    for piece in href.split('/') {
        match piece {
            "" | "." => {}
            ".." => { parts.pop(); }
            piece => parts.push(piece),
        }
    }
    let folder = href.ends_with('/') && !parts.is_empty();
    format!("{root}/{}{}{tail}", parts.join("/"), if folder { "/" } else { "" })
}

// ---- Reading a page

/// The shelf of a language.
pub fn shelf(english: bool) -> Shelf {
    page("index", english).map(read_shelf).unwrap_or_default()
}

/// A tutorial of a language, if there is one by that name.
pub fn tutorial(slug: &str, english: bool) -> Option<Tutorial> {
    if slug == "index" { return None }
    page(slug, english).map(read_tutorial)
}

pub fn read_shelf(html: &str) -> Shelf {
    let nodes = parse(html);
    let Some(main) = find(&nodes, &|node| node.is("main")) else { return Shelf::default() };
    let (kicker, title, lead, facts) = head(main);
    let parts = all(main, &|node| node.is("section") && node.has_class("shelf-part"))
        .into_iter()
        .map(|part| {
            // 「一收集」: the number in <small> is dropped, the name stays.
            let name = part.child("h2").map(|h2| inline_text(h2.children(), &|node| !node.is("small"))).unwrap_or_default();
            let entries = all(part, &|node| node.is("a") && node.has_class("entry"))
                .into_iter()
                .map(|entry| Entry {
                    slug: entry.attr("href").unwrap_or_default().trim_end_matches(".html").to_string(),
                    number: find(entry.children(), &|node| node.is("span") && node.has_class("num")).map(Node::text).unwrap_or_default(),
                    title: find(entry.children(), &|node| node.is("h3")).map(Node::text).unwrap_or_default(),
                    summary: find(entry.children(), &|node| node.is("p")).map(|p| rich(p.children())).unwrap_or_default(),
                    needs: find(entry.children(), &|node| node.is("dl")).map(|dl| pairs(dl).into_iter().map(|(term, value)| (term, value.text)).collect()).unwrap_or_default(),
                })
                .collect();
            Part { name, entries }
        })
        .collect();
    Shelf { kicker, title, lead, facts, parts }
}

pub fn read_tutorial(html: &str) -> Tutorial {
    let nodes = parse(html);
    let Some(main) = find(&nodes, &|node| node.is("main")) else { return Tutorial::default() };
    let (kicker, title, lead, facts) = head(main);
    let article = find(main.children(), &|node| node.is("article"));
    let sections = article
        .map(|article| article.children().iter().filter(|node| node.is("section")).map(section).collect())
        .unwrap_or_default();
    let colophon = find(&nodes, &|node| node.is("div") && node.has_class("colophon"))
        .map(|div| div.children().iter().filter(|node| node.is("p")).map(|p| rich(p.children())).collect())
        .unwrap_or_default();
    Tutorial { kicker, title, lead, facts, sections, colophon }
}

/// The head of a page: the line over its title, the title, the lead and the facts under it.
fn head(main: &Node) -> (String, String, Rich, Vec<(String, Rich)>) {
    let Some(head) = find(main.children(), &|node| node.is("header") && node.has_class("doc-head")) else { return Default::default() };
    let kicker = find(head.children(), &|node| node.is("p") && node.has_class("kicker")).map(Node::text).unwrap_or_default();
    let title = find(head.children(), &|node| node.is("h1")).map(Node::text).unwrap_or_default();
    let lead = find(head.children(), &|node| node.is("p") && node.has_class("lead")).map(|p| rich(p.children())).unwrap_or_default();
    let facts = find(head.children(), &|node| node.is("dl")).map(pairs).unwrap_or_default();
    (kicker, title, lead, facts)
}

/// The terms and values of a `<dl>`, each pair in a `<div>` of its own or side by side.
fn pairs(dl: &Node) -> Vec<(String, Rich)> {
    let mut out = vec![];
    let mut term = None;
    let mut walk = |nodes: &[Node], out: &mut Vec<(String, Rich)>| {
        for node in nodes {
            if node.is("dt") { term = Some(node.text()) }
            if node.is("dd") { out.push((term.take().unwrap_or_default(), rich(node.children()))) }
        }
    };
    for node in dl.children() {
        if node.is("div") { walk(node.children(), &mut out) } else { walk(std::slice::from_ref(node), &mut out) }
    }
    out
}

fn section(node: &Node) -> Section {
    let id = node.attr("id").unwrap_or_default().to_string();
    let h2 = node.child("h2");
    let number = h2.and_then(|h2| h2.child("i")).map(Node::text).unwrap_or_default();
    let title = h2.map(|h2| inline_text(h2.children(), &|node| !node.is("i"))).unwrap_or_default();
    let blocks = blocks(node.children().iter().filter(|child| !child.is("h2")));
    Section { id, number, title, blocks }
}

fn blocks<'a>(nodes: impl Iterator<Item = &'a Node>) -> Vec<Block> {
    let mut out = vec![];
    for node in nodes {
        let Node::Element { name, children, .. } = node else {
            if let Node::Text(text) = node && !text.trim().is_empty() { out.push(Block::Paragraph(Rich::plain(text.trim()))) }
            continue;
        };
        match name.as_str() {
            "p" => {
                let text = rich(children);
                if !text.text.is_empty() { out.push(Block::Paragraph(text)) }
            }
            "h3" | "h4" => out.push(Block::Heading(rich(children))),
            "ul" | "ol" => out.push(Block::List { ordered: name == "ol", items: children.iter().filter(|child| child.is("li")).map(|li| rich(li.children())).collect() }),
            "blockquote" => {
                let cite = node.child("cite").map(Node::text).unwrap_or_default();
                let mut paragraphs: Vec<Rich> = children.iter().filter(|child| child.is("p")).map(|p| rich(p.children())).collect();
                if paragraphs.is_empty() { paragraphs.push(rich(&children.iter().filter(|child| !child.is("cite")).cloned().collect::<Vec<_>>())) }
                out.push(Block::Quote { cite, paragraphs });
            }
            "figure" if node.has_class("code") => out.push(Block::Code { caption: node.child("figcaption").map(Node::text).unwrap_or_default(), text: find(children, &|node| node.is("pre")).map(Node::raw_text).unwrap_or_default().trim_end().to_string() }),
            "figure" => {
                let Some(image) = find(children, &|node| node.is("img")) else { continue };
                let size = |name: &str| image.attr(name).and_then(|value| value.parse::<f32>().ok()).unwrap_or(0.);
                out.push(Block::Figure {
                    src: image.attr("src").unwrap_or_default().to_string(),
                    alt: image.attr("alt").unwrap_or_default().to_string(),
                    width: size("width"),
                    height: size("height"),
                    caption: node.child("figcaption").map(|caption| rich(caption.children())).unwrap_or_default(),
                });
            }
            "pre" => out.push(Block::Code { caption: String::new(), text: node.raw_text().trim_end().to_string() }),
            "aside" => {
                // 「<b>提示</b>…」: the label first, then what it says.
                let label = children.iter().find(|child| child.is("b") || child.is("strong")).map(Node::text).unwrap_or_default();
                let mut skipped = false;
                let rest: Vec<Node> = children.iter().filter(|child| {
                    if !skipped && (child.is("b") || child.is("strong")) { skipped = true; return false }
                    true
                }).cloned().collect();
                out.push(Block::Note { warn: node.has_class("warn"), label, text: rich(&rest) });
            }
            "table" => out.push(table(node)),
            "div" | "section" => out.extend(blocks(children.iter())),
            _ => {}
        }
    }
    out
}

fn table(node: &Node) -> Block {
    let caption = node.child("caption").map(|caption| rich(caption.children())).unwrap_or_default();
    let row = |tr: &Node| -> Vec<Rich> { tr.children().iter().filter(|cell| cell.is("th") || cell.is("td")).map(|cell| rich(cell.children())).collect() };
    let rows_of = |parent: &Node| -> Vec<Vec<Rich>> { parent.children().iter().filter(|child| child.is("tr")).map(row).collect() };
    let head: Vec<Vec<Rich>> = node.child("thead").map(rows_of).unwrap_or_default();
    let mut body: Vec<Vec<Rich>> = node.children().iter().filter(|child| child.is("tbody")).flat_map(rows_of).collect();
    body.extend(rows_of(node));
    Block::Table { caption, head: head.len(), rows: head.into_iter().chain(body).collect() }
}

/// Inline HTML as text with marks: bold, italics, code (and keys), links; a line break stays one.
fn rich(nodes: &[Node]) -> Rich {
    fn walk(nodes: &[Node], marks: &Marks, out: &mut Rich) {
        for node in nodes {
            match node {
                Node::Text(text) => {
                    let text = collapse(text);
                    // What follows a line break starts the line: the source's indent and newline are not a space.
                    let text = if out.text.is_empty() || out.text.ends_with('\n') { text.trim_start().to_string() } else { text };
                    out.push(&text, marks.clone());
                }
                Node::Element { name, children, attrs } => {
                    let mut inner = marks.clone();
                    match name.as_str() {
                        "br" => {
                            trim_end(out);
                            out.push("\n", marks.clone());
                            continue;
                        }
                        "b" | "strong" => inner.bold = true,
                        "i" | "em" => inner.italic = true,
                        "code" | "kbd" => inner.code = true,
                        "a" => inner.link = attrs.iter().find(|(key, _)| key == "href").map(|(_, value)| value.clone()),
                        "img" | "script" | "style" => continue,
                        _ => {}
                    }
                    walk(children, &inner, out);
                }
            }
        }
    }
    let mut out = Rich::default();
    walk(nodes, &Marks::default(), &mut out);
    trim_end(&mut out);
    out
}

/// Spaces at the end of a text with marks taken off, run by run.
fn trim_end(rich: &mut Rich) {
    while let Some((len, _)) = rich.runs.last_mut() {
        let kept = rich.text.trim_end_matches([' ', '\t']).len();
        let cut = rich.text.len() - kept;
        if cut == 0 { break }
        let taken = cut.min(*len);
        *len -= taken;
        rich.text.truncate(rich.text.len() - taken);
        if *len == 0 { rich.runs.pop(); }
    }
}

/// Runs of white space as one space, as a browser draws them.
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() && c != '\u{3000}' && c != '\u{a0}' {
            if !space { out.push(' ') }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out
}

/// The words of nodes, those `keep` says yes to, white space collapsed and trimmed.
fn inline_text(nodes: &[Node], keep: &dyn Fn(&Node) -> bool) -> String {
    let kept: Vec<Node> = nodes.iter().filter(|node| keep(node)).cloned().collect();
    rich(&kept).text.trim().to_string()
}

// ---- A small reader of HTML: enough for pages written by hand

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Element { name: String, attrs: Vec<(String, String)>, children: Vec<Node> },
    Text(String),
}

impl Node {
    fn is(&self, tag: &str) -> bool {
        matches!(self, Node::Element { name, .. } if name == tag)
    }

    fn attr(&self, key: &str) -> Option<&str> {
        match self {
            Node::Element { attrs, .. } => attrs.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str()),
            Node::Text(_) => None,
        }
    }

    fn children(&self) -> &[Node] {
        match self {
            Node::Element { children, .. } => children,
            Node::Text(_) => &[],
        }
    }

    fn has_class(&self, class: &str) -> bool {
        self.attr("class").is_some_and(|classes| classes.split_whitespace().any(|name| name == class))
    }

    fn child(&self, tag: &str) -> Option<&Node> {
        match self {
            Node::Element { children, .. } => children.iter().find(|child| child.is(tag)),
            Node::Text(_) => None,
        }
    }

    /// The words, white space collapsed and trimmed.
    fn text(&self) -> String {
        rich(std::slice::from_ref(self)).text.trim().to_string()
    }

    /// The words as written, white space and all (a `<pre>`).
    fn raw_text(&self) -> String {
        match self {
            Node::Text(text) => text.clone(),
            Node::Element { name, .. } if name == "br" => "\n".into(),
            Node::Element { children, .. } => children.iter().map(Node::raw_text).collect(),
        }
    }
}

/// The first node, at any depth, that `wanted` says yes to.
fn find<'a>(nodes: &'a [Node], wanted: &dyn Fn(&Node) -> bool) -> Option<&'a Node> {
    for node in nodes {
        if wanted(node) { return Some(node) }
        if let Node::Element { children, .. } = node && let Some(found) = find(children, wanted) { return Some(found) }
    }
    None
}

/// Every node under `node` that `wanted` says yes to, not looking inside those found.
fn all<'a>(node: &'a Node, wanted: &dyn Fn(&Node) -> bool) -> Vec<&'a Node> {
    fn walk<'a>(nodes: &'a [Node], wanted: &dyn Fn(&Node) -> bool, out: &mut Vec<&'a Node>) {
        for node in nodes {
            if wanted(node) {
                out.push(node);
            } else if let Node::Element { children, .. } = node {
                walk(children, wanted, out);
            }
        }
    }
    let mut out = vec![];
    if let Node::Element { children, .. } = node { walk(children, wanted, &mut out) }
    out
}

const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"];

/// HTML as a tree. Comments and the doctype are dropped, the contents of `<script>` and `<style>` too; a
/// closing tag with no open element of its name is ignored, and one that closes an outer element closes the
/// ones left open inside it.
fn parse(html: &str) -> Vec<Node> {
    // The open elements, each with its children so far; the bottom one is the document.
    let mut stack: Vec<(String, Vec<(String, String)>, Vec<Node>)> = vec![(String::new(), vec![], vec![])];
    let close = |stack: &mut Vec<(String, Vec<(String, String)>, Vec<Node>)>| {
        if let Some((name, attrs, children)) = stack.pop() && let Some(parent) = stack.last_mut() { parent.2.push(Node::Element { name, attrs, children }) }
    };
    let mut rest = html;
    while !rest.is_empty() {
        let Some(open) = rest.find('<') else {
            push_text(&mut stack, rest);
            break;
        };
        push_text(&mut stack, &rest[..open]);
        rest = &rest[open..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            rest = rest.find('>').map_or("", |end| &rest[end + 1..]);
            continue;
        }
        if let Some(after) = rest.strip_prefix("</") {
            let end = after.find('>').unwrap_or(after.len());
            let name = after[..end].trim().to_ascii_lowercase();
            rest = after.get(end + 1..).unwrap_or("");
            if let Some(depth) = stack.iter().rposition(|(open, ..)| *open == name).filter(|depth| *depth > 0) {
                while stack.len() > depth { close(&mut stack) }
            }
            continue;
        }
        let Some((name, attrs, self_closing, used)) = tag(rest) else {
            // A lone `<`: text.
            push_text(&mut stack, "<");
            rest = &rest[1..];
            continue;
        };
        rest = &rest[used..];
        if VOID.contains(&name.as_str()) || self_closing {
            if let Some(parent) = stack.last_mut() { parent.2.push(Node::Element { name, attrs, children: vec![] }) }
            continue;
        }
        if name == "script" || name == "style" {
            let end = format!("</{name}");
            rest = rest.find(&end).map_or("", |at| rest[at..].find('>').map_or("", |close| &rest[at + close + 1..]));
            continue;
        }
        stack.push((name, attrs, vec![]));
    }
    while stack.len() > 1 { close(&mut stack) }
    stack.pop().map(|(_, _, children)| children).unwrap_or_default()
}

fn push_text(stack: &mut [(String, Vec<(String, String)>, Vec<Node>)], text: &str) {
    if text.is_empty() { return }
    let Some(parent) = stack.last_mut() else { return };
    let text = decode(text);
    match parent.2.last_mut() {
        Some(Node::Text(last)) => last.push_str(&text),
        _ => parent.2.push(Node::Text(text)),
    }
}

/// An opening tag at the start of `text`: its name, attributes, whether it closes itself, and its length.
fn tag(text: &str) -> Option<(String, Vec<(String, String)>, bool, usize)> {
    let bytes = text.as_bytes();
    let mut at = 1;
    let start = at;
    while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'-') { at += 1 }
    if at == start { return None }
    let name = text[start..at].to_ascii_lowercase();
    let mut attrs = vec![];
    loop {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() { at += 1 }
        if at >= bytes.len() { return Some((name, attrs, false, at)) }
        match bytes[at] {
            b'>' => return Some((name, attrs, false, at + 1)),
            b'/' if bytes.get(at + 1) == Some(&b'>') => return Some((name, attrs, true, at + 2)),
            b'/' => {
                at += 1;
                continue;
            }
            _ => {}
        }
        let key_start = at;
        while at < bytes.len() && !bytes[at].is_ascii_whitespace() && !matches!(bytes[at], b'=' | b'>' | b'/') { at += 1 }
        let key = text[key_start..at].to_ascii_lowercase();
        while at < bytes.len() && bytes[at].is_ascii_whitespace() { at += 1 }
        let mut value = String::new();
        if bytes.get(at) == Some(&b'=') {
            at += 1;
            while at < bytes.len() && bytes[at].is_ascii_whitespace() { at += 1 }
            match bytes.get(at) {
                Some(&quote) if quote == b'"' || quote == b'\'' => {
                    let end = text[at + 1..].find(quote as char).map_or(text.len(), |end| at + 1 + end);
                    value = decode(&text[at + 1..end]);
                    at = (end + 1).min(text.len());
                }
                _ => {
                    let value_start = at;
                    while at < bytes.len() && !bytes[at].is_ascii_whitespace() && bytes[at] != b'>' { at += 1 }
                    value = decode(&text[value_start..at]);
                }
            }
        }
        if key.is_empty() {
            at += 1;
            continue;
        }
        attrs.push((key, value));
    }
}

/// Character references as the characters they stand for.
fn decode(text: &str) -> String {
    if !text.contains('&') { return text.to_string() }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest[1..].find(';').map(|end| end + 1).filter(|end| *end <= 12);
        let named = end.and_then(|end| {
            let name = &rest[1..end];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                "mdash" => Some('—'),
                "ndash" => Some('–'),
                "hellip" => Some('…'),
                "rarr" => Some('→'),
                "larr" => Some('←'),
                "times" => Some('×'),
                "middot" => Some('·'),
                _ => name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")).and_then(|hex| u32::from_str_radix(hex, 16).ok()).or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok())).and_then(char::from_u32),
            };
            c.map(|c| (c, end))
        });
        match named {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_of_the_site_is_built_in() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../site/tutorials");
        for file in std::fs::read_dir(&folder).unwrap() {
            let name = file.unwrap().file_name().to_string_lossy().to_string();
            let Some(slug) = name.strip_suffix(".html") else { continue };
            if slug.starts_with('_') { continue }
            assert!(page(slug, false).is_some(), "site/tutorials/{name} is not in tutorial::PAGES");
        }
    }

    #[test]
    fn the_shelf_lists_every_tutorial_once() {
        for english in [false, true] {
            let shelf = shelf(english);
            assert!(!shelf.title.is_empty() && !shelf.lead.text.is_empty() && shelf.facts.len() >= 2, "{english}");
            assert_eq!(shelf.parts.len(), 5, "{english}");
            let mut slugs: Vec<&str> = shelf.entries().map(|entry| entry.slug.as_str()).collect();
            assert_eq!(slugs.len(), PAGES.len() - 1, "{english}");
            assert!(shelf.entries().all(|entry| entry.number.len() == 2 && !entry.title.is_empty() && !entry.summary.text.is_empty() && !entry.needs.is_empty()), "{english}");
            slugs.sort();
            slugs.dedup();
            assert_eq!(slugs.len(), PAGES.len() - 1);
            assert!(shelf.parts.iter().all(|part| !part.name.is_empty() && !part.name.contains('一')), "{:?}", shelf.parts.iter().map(|part| &part.name).collect::<Vec<_>>());
        }
        let shelf = shelf(false);
        assert_eq!(shelf.parts[0].name, "收集");
        assert_eq!(shelf.entry("start").unwrap().short_title(), "从一个课题开始");
        assert_eq!(shelf.neighbours("start").0.map(|entry| entry.slug.as_str()), None);
        assert_eq!(shelf.neighbours("start").1.map(|entry| entry.slug.as_str()), Some("home"));
        assert_eq!(super::shelf(true).entry("start").unwrap().short_title(), "Start from a topic");
    }

    #[test]
    fn every_tutorial_reads_into_sections_and_its_links_lead_somewhere() {
        for english in [false, true] {
            for (slug, ..) in PAGES.iter().filter(|(slug, ..)| *slug != "index") {
                let tutorial = tutorial(slug, english).unwrap();
                assert!(!tutorial.title.is_empty() && !tutorial.kicker.is_empty() && !tutorial.lead.text.is_empty(), "{slug} {english}");
                assert!(tutorial.sections.len() >= 2, "{slug} {english}");
                assert!(!tutorial.colophon.is_empty(), "{slug} {english}");
                for section in &tutorial.sections {
                    assert!(!section.id.is_empty() && !section.title.is_empty() && !section.blocks.is_empty(), "{slug} {english} {}", section.id);
                    for block in &section.blocks {
                        let mut texts = vec![];
                        match block {
                            Block::Figure { src, width, height, caption, .. } => {
                                assert!(*width > 0. && *height > 0. && !caption.text.is_empty(), "{slug} {src}");
                                let url = resolve(&base(english), src);
                                assert!(url.starts_with(SITE) && url.contains("/img/") && cached(&url).is_some(), "{slug} {url}");
                            }
                            Block::Code { text, .. } => assert!(!text.is_empty(), "{slug}"),
                            Block::Paragraph(text) | Block::Heading(text) => texts.push(text),
                            Block::List { items, .. } => texts.extend(items),
                            Block::Note { text, label, .. } => {
                                assert!(!label.is_empty(), "{slug}");
                                texts.push(text);
                            }
                            Block::Quote { paragraphs, .. } => texts.extend(paragraphs),
                            Block::Table { rows, head, .. } => {
                                assert!(*head >= 1 && rows.len() > *head, "{slug}");
                                texts.extend(rows.iter().flatten());
                            }
                        }
                        // A link to another tutorial or a section names one that is there.
                        for text in texts {
                            for (_, marks) in text.spans() {
                                let Some(href) = &marks.link else { continue };
                                if let Link::Page(target, anchor) = link(href, slug, english) {
                                    if target == "index" { continue }
                                    let other = super::tutorial(&target, english).unwrap_or_else(|| panic!("{slug}: {href}"));
                                    if let Some(anchor) = anchor { assert!(other.sections.iter().any(|section| section.id == anchor), "{slug}: {href}") }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_section_of_a_page() {
        let html = r#"<main><header class="doc-head"><p class="kicker">教程 01　·　收集</p><h1>从一个课题开始</h1><p class="lead">先&nbsp;配置 <b>模型</b>。</p>
            <dl class="facts"><div><dt>需要</dt><dd>一个模型</dd></div></dl></header>
            <article class="prose"><section id="s1"><h2><i>1</i>安装，配置模型</h2>
            <p>点「设置」（<kbd>⌘</kbd> + <kbd>,</kbd>），见<a href="read.html#s2">教程 05</a>。</p>
            <ul><li><b>接口</b>：三选一。</li><li>B &amp; C</li></ul>
            <figure class="shot"><img src="../img/t/a.jpg" width="1372" height="867" alt="设置页"><figcaption><b>图 1</b>　模型。</figcaption></figure>
            <aside class="note warn"><b>注意</b>明文保存。</aside>
            <blockquote><cite>主题</cite><p>植物<br>
               单细胞</p></blockquote>
            <figure class="code"><figcaption>命令行</figcaption>
<pre><code><span class="p">$</span> wisp list   <span class="c"># 注释</span>
{ "a": 1 }</code></pre></figure>
            <div class="scroll"><table><caption><b>表 1</b>　命中。</caption><thead><tr><th>库</th><th>篇</th></tr></thead><tbody><tr><th scope="row">PubMed</th><td>1007</td></tr></tbody></table></div>
            </section></article></main>"#;
        let tutorial = read_tutorial(html);
        assert_eq!((tutorial.kicker.as_str(), tutorial.title.as_str(), tutorial.lead.text.as_str()), ("教程 01　·　收集", "从一个课题开始", "先\u{a0}配置 模型。"));
        assert_eq!(tutorial.facts[0].0, "需要");
        let section = &tutorial.sections[0];
        assert_eq!((section.id.as_str(), section.number.as_str(), section.title.as_str()), ("s1", "1", "安装，配置模型"));
        let Block::Paragraph(text) = &section.blocks[0] else { panic!("{:?}", section.blocks[0]) };
        assert_eq!(text.text, "点「设置」（⌘ + ,），见教程 05。");
        // Keys are set as code, the link keeps its address.
        assert!(text.spans().any(|(range, marks)| marks.code && &text.text[range] == "⌘"));
        assert!(text.spans().any(|(range, marks)| marks.link.as_deref() == Some("read.html#s2") && &text.text[range] == "教程 05"));
        assert_eq!(link("read.html#s2", "start", false), Link::Page("read".into(), Some("s2".into())));
        let Block::List { ordered: false, items } = &section.blocks[1] else { panic!() };
        assert_eq!(items.iter().map(|item| item.text.as_str()).collect::<Vec<_>>(), ["接口：三选一。", "B & C"]);
        assert_eq!(section.blocks[2], Block::Figure { src: "../img/t/a.jpg".into(), alt: "设置页".into(), width: 1372., height: 867., caption: { let mut caption = Rich::default(); caption.push("图 1", Marks { bold: true, ..Default::default() }); caption.push("　模型。", Marks::default()); caption } });
        let Block::Note { warn: true, label, text } = &section.blocks[3] else { panic!() };
        assert_eq!((label.as_str(), text.text.as_str()), ("注意", "明文保存。"));
        let Block::Quote { cite, paragraphs } = &section.blocks[4] else { panic!() };
        assert_eq!((cite.as_str(), paragraphs[0].text.as_str()), ("主题", "植物\n单细胞"));
        assert_eq!(section.blocks[5], Block::Code { caption: "命令行".into(), text: "$ wisp list   # 注释\n{ \"a\": 1 }".into() });
        let Block::Table { head: 1, rows, caption } = &section.blocks[6] else { panic!() };
        assert_eq!((caption.text.as_str(), rows.len(), rows[1][1].text.as_str()), ("表 1　命中。", 2, "1007"));
    }

    #[test]
    fn addresses_on_the_project_page() {
        let zh = base(false);
        assert_eq!(resolve(&zh, "../img/t/a.jpg"), format!("{SITE}img/t/a.jpg"));
        assert_eq!(resolve(&base(true), "../../img/en/t/a.jpg"), format!("{SITE}img/en/t/a.jpg"));
        assert_eq!(resolve(&zh, "../index.html#ideas"), format!("{SITE}index.html#ideas"));
        assert_eq!(resolve(&zh, "https://github.com/x"), "https://github.com/x");
        assert_eq!(link("index.html", "start", false), Link::Page("index".into(), None));
        assert_eq!(link("#s3", "start", false), Link::Page("start".into(), Some("s3".into())));
        assert_eq!(link("../index.html#download", "start", false), Link::Web(format!("{SITE}index.html#download")));
        assert_eq!(link("nothing.html", "start", false), Link::Web(format!("{zh}nothing.html")));
        assert_eq!(page_url(Some("read"), true), format!("{SITE}en/tutorials/read.html"));
        assert_eq!(page_url(None, false), zh);
        // Nothing outside the cache folder.
        assert!(cached(&format!("{SITE}img/../../etc/passwd")).is_none());
        assert!(cached("https://example.org/a.jpg").is_none());
        assert!(cached(&format!("{SITE}img/t/a.jpg")).unwrap().ends_with("tutorials/img/t/a.jpg"));
    }

    #[test]
    fn html_that_is_not_quite_closed() {
        let nodes = parse("<!doctype html><p>a <b>b</p><p>c<img src=x>d</p><script>if (a < b) {}</script>&#x4e2d;&amp;&bogus;");
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].text(), "a b");
        assert_eq!(nodes[1].text(), "cd");
        assert_eq!(nodes[2], Node::Text("中&&bogus;".into()));
    }
}
