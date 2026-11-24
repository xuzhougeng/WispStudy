//! A paper's mind map (思维导图): a tree of short notes the reader makes of a
//! paper, or has a model draft. A node may point at the passage it is drawn
//! from (`Anchor`), which the reader's PDF goes to. Here are the tree, every
//! change to it, its outline as text (what a model writes and what is copied
//! out) and where its nodes stand when it is drawn. The window draws it
//! (`src/mindmap.rs`) and the library keeps it (`Store::mind_map`).

use crate::ai::{self, HttpRequest, Profile};
use crate::model::Paper;
use serde::{Deserialize, Serialize};

/// How much of a paper's text a model is given to draft a map from, in characters.
const SOURCE: usize = 120_000;

/// What the model is told. `{language}` is the language the nodes are written in.
pub const PROMPT: &str = "你是帮研究者读论文的助手。把用户给的论文整理成一张思维导图的大纲，让人一眼看出这篇论文问了什么、怎么做的、发现了什么、意味着什么。\n\
要求：\n\
- 用{language}写节点；专有名词、基因名、方法名保留原文。\n\
- 第一行是「# 」加论文的短题目（不超过 20 个字）。其后是嵌套的 Markdown 列表，用「- 」开头，每深一层多缩进两个空格，最多三层。\n\
- 第一层 4 到 6 个分支，按这篇论文自己的思路来（例如：问题、方法、主要发现、局限、意义），不要照搬章节标题；每个分支下 2 到 5 个要点，要点下可以再有一层细节。全图 20 到 40 个节点。\n\
- 每个节点是一个短语或一句短话，不超过 24 个字，写具体的内容（对象、数字、结论），不要写「介绍了方法」这样的空话。\n\
- 只写论文里有的内容。有出处的要点，在节点后面用方括号标出页码和从原文逐字抄下来的一小段（8 到 25 个词，和原文一字不差，用原文的语言，不翻译、不用省略号）：`- 要点 [p.5 \"exact words from the paper\"]`。页码按正文里【第 N 页】的标记。拿不准原文就不标，不要编造。\n\
- 除了这份大纲，不要输出任何别的文字。";

/// The instructions for a map written in `language` (`ai::language_name`'s words: 中文, 英文).
pub fn instructions(language: &str) -> String {
    PROMPT.replace("{language}", language)
}

/// What the model is given: the paper's details and its pages, each under its number, as far as they fit.
pub fn source(paper: &Paper, pages: &[String]) -> String {
    let mut out = format!("题目：{}\n", paper.title.trim());
    if !paper.authors.trim().is_empty() { out.push_str(&format!("作者：{}\n", paper.authors.trim())) }
    if !paper.abstract_text.trim().is_empty() { out.push_str(&format!("\n摘要：\n{}\n", paper.abstract_text.trim())) }
    let mut left = SOURCE;
    for (n, page) in pages.iter().enumerate() {
        let page = page.trim();
        if page.is_empty() { continue }
        let cut: String = page.chars().take(left).collect();
        out.push_str(&format!("\n【第 {} 页】\n{cut}\n", n + 1));
        left = left.saturating_sub(cut.chars().count());
        if left == 0 {
            out.push_str("\n（正文到此截断）");
            break;
        }
    }
    out
}

/// The request for a draft of a paper's map.
pub fn request(profile: &Profile, language: &str, paper: &Paper, pages: &[String]) -> Result<HttpRequest, String> {
    if paper.abstract_text.trim().is_empty() && pages.iter().all(|page| page.trim().is_empty()) { return Err("这篇文献没有可以整理的文字：没有摘要，也读不出全文。".into()) }
    ai::plain_request(profile, &instructions(language), &source(paper, pages))
}

/// The map a model's reply draws, with the passages it names looked up on the pages of `file`: one that is
/// there word for word is kept with the page it is really on (the one named, when it is there too), the others
/// are let go and their nodes stay without one. A reply with no outline in it is an error to show.
pub fn read_reply(reply: &str, paper: &Paper, file: &str, pages: &[String]) -> Result<MindMap, String> {
    let mut map = MindMap::from_outline(reply, &paper.title);
    if map.root.children.is_empty() { return Err("模型的回答里没有读出大纲，请重试。".into()) }
    let nowhere = Paper::default();
    map.keep_found(|page, quote| {
        let named = page.checked_sub(1).and_then(|at| pages.get(at)).and_then(|text| ai::locate_quote(quote, &nowhere, Some(std::slice::from_ref(text)))).map(|_| page);
        let page = named.or_else(|| ai::locate_quote(quote, &nowhere, Some(pages)).flatten())?;
        Some(Anchor { file: file.to_string(), page, quote: quote.to_string() })
    });
    Ok(map)
}

/// Where a node is drawn from: a file of the paper (a resource id), a 1-based
/// page of it, and the words there. Kept only when the words are found on a
/// page (`MindMap::keep_found`): a quotation a model made up points nowhere.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    #[serde(default)]
    pub file: String,
    pub page: usize,
    #[serde(default)]
    pub quote: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
    /// Its children are not drawn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub folded: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

impl Node {
    fn find(&self, id: u64) -> Option<&Node> {
        if self.id == id { return Some(self) }
        self.children.iter().find_map(|child| child.find(id))
    }

    fn find_mut(&mut self, id: u64) -> Option<&mut Node> {
        if self.id == id { return Some(self) }
        self.children.iter_mut().find_map(|child| child.find_mut(id))
    }

    /// The node that has `id` among its children, and where.
    fn holder(&self, id: u64) -> Option<(&Node, usize)> {
        if let Some(at) = self.children.iter().position(|child| child.id == id) { return Some((self, at)) }
        self.children.iter().find_map(|child| child.holder(id))
    }

    fn count(&self) -> usize {
        1 + self.children.iter().map(Node::count).sum::<usize>()
    }

    fn each(&mut self, change: &mut impl FnMut(&mut Node)) {
        change(self);
        for child in &mut self.children { child.each(change) }
    }
}

/// A step from one node to another with the arrow keys, as the map is drawn: its root at the left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Up,
    Down,
    /// To the node it hangs on.
    Out,
    /// To its first child.
    In,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MindMap {
    pub root: Node,
    /// The id the next node made gets: ids are never used twice, so a node chosen stays the one chosen.
    #[serde(default)]
    next: u64,
}

impl MindMap {
    /// A map with nothing but its root: the paper's title, to be branched from.
    pub fn new(title: &str) -> MindMap {
        MindMap { root: Node { id: 1, text: title.trim().to_string(), ..Node::default() }, next: 2 }
    }

    pub fn node(&self, id: u64) -> Option<&Node> {
        self.root.find(id)
    }

    pub fn parent(&self, id: u64) -> Option<u64> {
        self.root.holder(id).map(|(parent, _)| parent.id)
    }

    /// How many nodes, the root among them.
    pub fn count(&self) -> usize {
        self.root.count()
    }

    fn fresh(&mut self) -> u64 {
        // A map read from an older body, or written by hand: never an id that is there.
        let mut most = 0;
        self.root.each(&mut |node| most = most.max(node.id));
        self.next = self.next.max(most + 1);
        self.next += 1;
        self.next - 1
    }

    /// A new last child of `parent`, which is unfolded to show it. Its id.
    pub fn add_child(&mut self, parent: u64, text: &str, anchor: Option<Anchor>) -> Option<u64> {
        self.root.find(parent)?;
        let id = self.fresh();
        let parent = self.root.find_mut(parent)?;
        parent.folded = false;
        parent.children.push(Node { id, text: text.trim().to_string(), anchor, ..Node::default() });
        Some(id)
    }

    /// A new node right after `id` under the same parent; after the root there is no such place, so it is the root's child.
    pub fn add_after(&mut self, id: u64, text: &str) -> Option<u64> {
        let Some((parent, at)) = self.root.holder(id).map(|(parent, at)| (parent.id, at)) else { return self.add_child(self.root.id, text, None) };
        let new = self.fresh();
        self.root.find_mut(parent)?.children.insert(at + 1, Node { id: new, text: text.trim().to_string(), ..Node::default() });
        Some(new)
    }

    /// Take a node and all under it out. The root stays. The node to choose next: the one before it, else the one
    /// after, else the one it hung on.
    pub fn remove(&mut self, id: u64) -> Option<u64> {
        let (parent, at) = self.root.holder(id).map(|(parent, at)| (parent.id, at))?;
        let parent = self.root.find_mut(parent)?;
        parent.children.remove(at);
        Some(if at > 0 { parent.children[at - 1].id } else { parent.children.first().map_or(parent.id, |next| next.id) })
    }

    /// Whether the words changed.
    pub fn set_text(&mut self, id: u64, text: &str) -> bool {
        let Some(node) = self.root.find_mut(id) else { return false };
        let text = text.trim();
        if node.text == text { return false }
        node.text = text.to_string();
        true
    }

    /// Fold a node's children away, or show them again. Whether anything changed: a node without children has nothing to fold.
    pub fn fold(&mut self, id: u64) -> bool {
        let Some(node) = self.root.find_mut(id).filter(|node| !node.children.is_empty()) else { return false };
        node.folded = !node.folded;
        true
    }

    /// A level deeper: the node becomes the last child of the one before it.
    pub fn indent(&mut self, id: u64) -> bool {
        let Some((parent, at)) = self.root.holder(id).map(|(parent, at)| (parent.id, at)).filter(|(_, at)| *at > 0) else { return false };
        let Some(parent) = self.root.find_mut(parent) else { return false };
        let node = parent.children.remove(at);
        let before = &mut parent.children[at - 1];
        before.folded = false;
        before.children.push(node);
        true
    }

    /// A level up: the node comes right after the one it hung on. A child of the root is as far up as a node goes.
    pub fn outdent(&mut self, id: u64) -> bool {
        let Some(parent) = self.parent(id) else { return false };
        let Some((grand, parent_at)) = self.root.holder(parent).map(|(grand, at)| (grand.id, at)) else { return false };
        let Some(at) = self.root.find(parent).and_then(|parent| parent.children.iter().position(|child| child.id == id)) else { return false };
        let Some(node) = self.root.find_mut(parent).map(|parent| parent.children.remove(at)) else { return false };
        let Some(grand) = self.root.find_mut(grand) else { return false };
        grand.children.insert(parent_at + 1, node);
        true
    }

    /// Before the node above it, or after the one below, under the same parent.
    pub fn shift(&mut self, id: u64, down: bool) -> bool {
        let Some((parent, at)) = self.root.holder(id).map(|(parent, at)| (parent.id, at)) else { return false };
        let Some(parent) = self.root.find_mut(parent) else { return false };
        let to = if down { at + 1 } else { at.wrapping_sub(1) };
        if to >= parent.children.len() { return false }
        parent.children.swap(at, to);
        true
    }

    /// The nodes drawn, from the top down as they stand (a node, then what hangs on it): those under a folded node are not.
    pub fn shown(&self) -> Vec<u64> {
        fn walk(node: &Node, out: &mut Vec<u64>) {
            out.push(node.id);
            if !node.folded { for child in &node.children { walk(child, out) } }
        }
        let mut out = vec![];
        walk(&self.root, &mut out);
        out
    }

    /// The node an arrow key leads to from `id`. Up and down stay among the nodes of the same parent while there
    /// is one that way, and go on to the node drawn above or below otherwise, as the eye does.
    pub fn step(&self, id: u64, step: Step) -> Option<u64> {
        match step {
            Step::Out => self.parent(id),
            Step::In => self.node(id).filter(|node| !node.folded).and_then(|node| node.children.first()).map(|child| child.id),
            Step::Up | Step::Down => {
                if let Some((parent, at)) = self.root.holder(id) {
                    let to = if step == Step::Down { at + 1 } else { at.wrapping_sub(1) };
                    if let Some(next) = parent.children.get(to) { return Some(next.id) }
                }
                let shown = self.shown();
                let at = shown.iter().position(|shown| *shown == id)?;
                shown.get(if step == Step::Down { at + 1 } else { at.checked_sub(1)? }).copied()
            }
        }
    }

    /// The map as an outline: the root as a heading, what hangs on it as a list, a node's passage after its words
    /// as `[p.5 "…"]`. What `from_outline` reads back, and what is copied out of the window.
    pub fn outline(&self) -> String {
        fn walk(node: &Node, depth: usize, out: &mut String) {
            let cited = node.anchor.as_ref().map(|a| if a.quote.is_empty() { format!(" [p.{}]", a.page) } else { format!(" [p.{} \"{}\"]", a.page, a.quote.replace('"', "'")) }).unwrap_or_default();
            out.push_str(&format!("{}- {}{}\n", "  ".repeat(depth), node.text.replace('\n', " "), cited));
            for child in &node.children { walk(child, depth + 1, out) }
        }
        let mut out = format!("# {}\n\n", self.root.text.replace('\n', " "));
        for child in &self.root.children { walk(child, 0, &mut out) }
        out
    }

    /// Read an outline as a model writes one: a heading for the root (`title` without one), then a nested list,
    /// deeper by its indentation; further headings are levels too. A passage named after a node's words
    /// (`[p.5 "…"]`) is taken as it is said: nothing is known yet of whether it is in the paper (`keep_found`).
    pub fn from_outline(text: &str, title: &str) -> MindMap {
        let mut map = MindMap::new(title);
        // The nodes a new line may hang on: how deep each is, and for a list item how far it is indented.
        let mut open: Vec<(u64, usize)> = vec![];
        // How deep the last heading stands (the root is 0), and the indentation of the list items seen under it.
        let (mut under, mut indents, mut rooted) = (0usize, Vec::<usize>::new(), false);
        for raw in text.lines() {
            let indent = raw.chars().take_while(|c| c.is_whitespace()).map(|c| if c == '\t' { 4 } else { 1 }).sum::<usize>();
            let line = raw.trim();
            if line.is_empty() || line.starts_with("```") { continue }
            let hashes = line.chars().take_while(|c| *c == '#').count();
            let heading = (hashes > 0 && line[hashes..].starts_with(' ')).then(|| &line[hashes..]);
            let (words, depth) = match (heading, item(line)) {
                (Some(heading), _) => {
                    indents.clear();
                    // The first heading of the first level names the map.
                    if hashes == 1 && !rooted {
                        rooted = true;
                        let (words, _) = cited(heading);
                        if !words.is_empty() { map.root.text = words }
                        continue;
                    }
                    under = (hashes - 1).max(1);
                    (heading, under)
                }
                (None, Some(words)) => {
                    while indents.last().is_some_and(|last| *last > indent) { indents.pop(); }
                    if indents.last() != Some(&indent) { indents.push(indent) }
                    (words, under + indents.len())
                }
                // A line of prose between the items is not a node.
                (None, None) => continue,
            };
            let (words, anchor) = cited(words);
            if words.is_empty() { continue }
            while open.last().is_some_and(|(_, at)| *at >= depth) { open.pop(); }
            let parent = open.last().map_or(map.root.id, |(id, _)| *id);
            if let Some(id) = map.add_child(parent, &words, anchor) { open.push((id, depth)) }
        }
        map
    }

    /// Keep the passages that are in the paper and let go of the others: `locate` is asked for each with the page
    /// it names (0 when it names none) and its words, and says where the words really stand.
    pub fn keep_found(&mut self, locate: impl Fn(usize, &str) -> Option<Anchor>) {
        self.root.each(&mut |node| node.anchor = node.anchor.take().and_then(|anchor| locate(anchor.page, &anchor.quote)));
    }
}

/// The words of a list item without its bullet or number; `None` for a line that is not one.
fn item(line: &str) -> Option<&str> {
    for bullet in ["- ", "* ", "+ ", "• ", "· "] {
        if let Some(rest) = line.strip_prefix(bullet) { return Some(rest.trim()) }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let rest = &line[digits..];
    if digits > 0 && digits <= 3 { return rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")).or_else(|| rest.strip_prefix("、")).map(str::trim) }
    None
}

/// A node's words apart from the passage named after them: `方法 [p.5 "we used"]` is the words and page 5 with
/// its quotation. Marks of emphasis around the words go.
fn cited(text: &str) -> (String, Option<Anchor>) {
    let text = text.trim();
    let clean = |words: &str| words.trim().trim_matches(|c| c == '*' || c == '_' || c == '`').trim().to_string();
    let Some(open) = text.rfind('[').filter(|_| text.ends_with(']')) else { return (clean(text), None) };
    let inside = &text[open + 1..text.len() - 1];
    // The quotation: what stands between the first and the last quotation mark.
    let marks = (inside.find(['"', '“', '「']), inside.rfind(['"', '”', '」']));
    let quote = match marks {
        (Some(a), Some(b)) if b > a => inside[a..b].chars().skip(1).collect::<String>().trim().to_string(),
        _ => String::new(),
    };
    // The page: the number after `p.`, `page` or `第`, looked for before the quotation (its words may hold such
    // a number too) and only where the mark begins a word (`Supp. 3` names no page). A paper's label (`P3`) is none.
    let head = inside[..marks.0.unwrap_or(inside.len())].to_lowercase();
    let page = ["p.", "page", "第"].iter().find_map(|mark| {
        head.rmatch_indices(mark).find_map(|(at, _)| {
            if head[..at].chars().next_back().is_some_and(char::is_alphabetic) { return None }
            let digits: String = head[at + mark.len()..].trim_start().chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<usize>().ok()
        })
    });
    if page.is_none() && quote.is_empty() { return (clean(text), None) }
    (clean(&text[..open]), Some(Anchor { file: String::new(), page: page.unwrap_or(0), quote }))
}

/// A node as it is drawn: its place and size, how deep it is (the root is 0), the node it hangs on (its place in
/// `Layout::nodes`), how many children it has, and whether they are folded away.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub id: u64,
    pub depth: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub parent: Option<usize>,
    pub children: usize,
    pub folded: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub nodes: Vec<Placed>,
    pub size: (f32, f32),
}

/// Where the nodes stand: the root at the left, each node's children in a column to its right (`gap.0` away),
/// one under the other (`gap.1` apart), the node in the middle of the height they take. `size` is how large a
/// node is drawn at its depth, which is the window's to say (it measures the words).
pub fn layout(map: &MindMap, size: &dyn Fn(&Node, usize) -> (f32, f32), gap: (f32, f32)) -> Layout {
    struct Sized<'a> {
        node: &'a Node,
        size: (f32, f32),
        /// The height the node and what hangs on it take.
        span: f32,
        children: Vec<Sized<'a>>,
    }
    fn measure<'a>(node: &'a Node, depth: usize, size: &dyn Fn(&Node, usize) -> (f32, f32), gap: f32) -> Sized<'a> {
        let own = size(node, depth);
        let children: Vec<Sized> = if node.folded { vec![] } else { node.children.iter().map(|child| measure(child, depth + 1, size, gap)).collect() };
        let below = children.iter().map(|child| child.span).sum::<f32>() + gap * children.len().saturating_sub(1) as f32;
        Sized { node, size: own, span: own.1.max(below), children }
    }
    fn put(sized: &Sized, depth: usize, x: f32, top: f32, parent: Option<usize>, gap: (f32, f32), out: &mut Layout) {
        let at = out.nodes.len();
        let (w, h) = sized.size;
        out.nodes.push(Placed { id: sized.node.id, depth, x, y: top + (sized.span - h) / 2., w, h, parent, children: sized.node.children.len(), folded: sized.node.folded });
        out.size = (out.size.0.max(x + w), out.size.1.max(top + sized.span));
        let below = sized.children.iter().map(|child| child.span).sum::<f32>() + gap.1 * sized.children.len().saturating_sub(1) as f32;
        let mut top = top + (sized.span - below) / 2.;
        for child in &sized.children {
            put(child, depth + 1, x + w + gap.0, top, Some(at), gap, out);
            top += child.span + gap.1;
        }
    }
    let mut out = Layout::default();
    put(&measure(&map.root, 0, size, gap.1), 0, 0., 0., None, gap, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (MindMap, [u64; 4]) {
        let mut map = MindMap::new("  A paper  ");
        let a = map.add_child(1, "方法", None).unwrap();
        let b = map.add_child(1, "结果", None).unwrap();
        let a1 = map.add_child(a, "测序", Some(Anchor { file: "r1".into(), page: 3, quote: "we sequenced".into() })).unwrap();
        let a2 = map.add_after(a1, "聚类").unwrap();
        (map, [a, b, a1, a2])
    }

    #[test]
    fn nodes_are_added_removed_and_keep_their_ids() {
        let (mut map, [a, b, a1, a2]) = sample();
        assert_eq!((map.root.text.as_str(), map.count(), map.parent(a2), map.parent(1)), ("A paper", 5, Some(a), None));
        // After the root there is no place: the node is its child.
        let c = map.add_after(1, "讨论").unwrap();
        assert_eq!(map.root.children.iter().map(|n| n.id).collect::<Vec<_>>(), vec![a, b, c]);
        // Taken out: the one before is chosen, else the one after, else the parent; the root stays.
        assert_eq!((map.remove(a2), map.remove(a1), map.remove(1)), (Some(a1), Some(a), None));
        assert_eq!(map.remove(b), Some(a));
        // An id is not used twice, in a map read back from its body either.
        let body = serde_json::to_string(&map).unwrap();
        let mut back: MindMap = serde_json::from_str(&body).unwrap();
        assert_eq!(back, map);
        let d = back.add_child(1, "新", None).unwrap();
        assert!(![1, a, b, c, a1, a2].contains(&d));
        let mut bare: MindMap = serde_json::from_str(r#"{"root":{"id":7,"text":"t","children":[{"id":9,"text":"x"}]}}"#).unwrap();
        assert_eq!(bare.add_child(7, "y", None), Some(10));
    }

    #[test]
    fn nodes_move_between_levels_and_among_their_siblings() {
        let (mut map, [a, b, a1, a2]) = sample();
        // Deeper: under the one before; the first of its parent has none before it.
        assert!(!map.indent(a1) && map.indent(a2));
        assert_eq!(map.parent(a2), Some(a1));
        // Up again: right after the node it hung on. A child of the root is as far up as it goes.
        assert!(map.outdent(a2) && !map.outdent(a));
        assert_eq!(map.node(a).unwrap().children.iter().map(|n| n.id).collect::<Vec<_>>(), vec![a1, a2]);
        assert!(map.outdent(a2));
        assert_eq!(map.root.children.iter().map(|n| n.id).collect::<Vec<_>>(), vec![a, a2, b]);
        assert!(map.shift(a2, true) && !map.shift(a2, true) && map.shift(a, true));
        assert_eq!(map.root.children.iter().map(|n| n.id).collect::<Vec<_>>(), vec![b, a, a2]);
        assert!(map.set_text(b, " 发现 ") && !map.set_text(b, "发现") && !map.set_text(99, "x"));
    }

    #[test]
    fn the_arrow_keys_follow_the_map_as_it_is_drawn() {
        let (mut map, [a, b, a1, a2]) = sample();
        assert_eq!(map.shown(), vec![1, a, a1, a2, b]);
        assert_eq!((map.step(1, Step::In), map.step(a, Step::In), map.step(a1, Step::Out), map.step(1, Step::Out)), (Some(a), Some(a1), Some(a), None));
        // Down among the children of one node, then on to what is drawn below.
        assert_eq!((map.step(a1, Step::Down), map.step(a2, Step::Down), map.step(a, Step::Down), map.step(b, Step::Down)), (Some(a2), Some(b), Some(b), None));
        assert_eq!((map.step(b, Step::Up), map.step(a1, Step::Up), map.step(1, Step::Up)), (Some(a), Some(a), None));
        // Folded: what hangs on it is not drawn, and not stepped into.
        assert!(map.fold(a) && !map.fold(b));
        assert_eq!((map.shown(), map.step(a, Step::In)), (vec![1, a, b], None));
        // A child added shows itself.
        map.add_child(a, "x", None);
        assert!(!map.node(a).unwrap().folded);
    }

    #[test]
    fn an_outline_is_written_and_read_back() {
        let (map, _) = sample();
        let outline = map.outline();
        assert_eq!(outline, "# A paper\n\n- 方法\n  - 测序 [p.3 \"we sequenced\"]\n  - 聚类\n- 结果\n");
        let back = MindMap::from_outline(&outline, "other");
        assert_eq!(back.outline(), outline);
        assert_eq!(back.root.children[0].children[0].anchor, Some(Anchor { file: String::new(), page: 3, quote: "we sequenced".into() }));
    }

    #[test]
    fn what_a_model_writes_is_read_as_an_outline() {
        let reply = "```markdown\n# **Single-cell atlas**\n\n## 背景\n- 问题：细胞类型不清 [P1 p.2 “cell types remain unclear”]\n    * 以往方法\n- 目标\n## 2. 方法\n1. 测序 [p.4]\n2) 聚类（见第 5 页）\n\n不是列表的一行\n### 细节\n- 深一层 [第7页 \"deep\"]\n```";
        let map = MindMap::from_outline(reply, "title");
        assert_eq!(map.outline(), "# Single-cell atlas\n\n- 背景\n  - 问题：细胞类型不清 [p.2 \"cell types remain unclear\"]\n    - 以往方法\n  - 目标\n- 2. 方法\n  - 测序 [p.4]\n  - 聚类（见第 5 页）\n  - 细节\n    - 深一层 [p.7 \"deep\"]\n");
        // No heading: the title given is the root, and the list hangs on it.
        let plain = MindMap::from_outline("- a\n  - b\n- c [\"only words, no page\"]", "The title");
        assert_eq!(plain.outline(), "# The title\n\n- a\n  - b\n- c [p.0 \"only words, no page\"]\n");
        // Square brackets that name no passage are words.
        assert_eq!(MindMap::from_outline("- CRISPR [2012]\n- see [ref]\n- more [Supp. 3]", "t").outline(), "# t\n\n- CRISPR [2012]\n- see [ref]\n- more [Supp. 3]\n");
    }

    #[test]
    fn passages_not_found_are_let_go() {
        let mut map = MindMap::from_outline("- a [p.2 \"found here\"]\n- b [p.9 \"made up\"]\n- c", "t");
        map.keep_found(|page, quote| (quote == "found here").then(|| Anchor { file: "r1".into(), page: page + 1, quote: quote.to_string() }));
        let anchors: Vec<Option<Anchor>> = map.root.children.iter().map(|n| n.anchor.clone()).collect();
        assert_eq!(anchors, vec![Some(Anchor { file: "r1".into(), page: 3, quote: "found here".into() }), None, None]);
    }

    #[test]
    fn a_draft_keeps_the_passages_that_are_on_the_pages() {
        let paper = Paper { title: "Atlas of the root".into(), abstract_text: "An abstract.".into(), ..Paper::default() };
        let pages = vec!["Introduction. Cell types remain unclear in the root tip.".to_string(), "Methods. We sequenced ten\nthousand nuclei from seedlings.".to_string()];
        let given = source(&paper, &pages);
        assert!(given.contains("题目：Atlas of the root") && given.contains("【第 1 页】\nIntroduction") && given.contains("【第 2 页】\nMethods"));
        assert!(instructions("中文").contains("用中文写节点"));
        // On the page named; on another page than the one named; made up; too short to be found anywhere.
        let reply = "# 根尖图谱\n- 问题 [p.1 \"Cell types remain unclear\"]\n- 方法 [p.1 \"We sequenced ten thousand nuclei\"]\n  - 编的 [p.2 \"twelve thousand protoplasts were used\"]\n- 短 [p.1 \"root\"]";
        let map = read_reply(reply, &paper, "r1", &pages).unwrap();
        let anchors: Vec<Option<usize>> = [0, 1, 2].iter().map(|n| map.root.children[*n].anchor.as_ref().map(|a| a.page)).collect();
        assert_eq!((map.root.text.as_str(), anchors), ("根尖图谱", vec![Some(1), Some(2), None]));
        assert_eq!((map.root.children[1].anchor.as_ref().unwrap().file.as_str(), &map.root.children[1].children[0].anchor), ("r1", &None));
        // Nothing to draw is said, not shown as an empty map.
        assert!(read_reply("抱歉，我无法处理。", &paper, "r1", &pages).is_err());
        // A paper with neither abstract nor text is not sent.
        assert!(source(&Paper::default(), &[]).starts_with("题目："));
    }

    #[test]
    fn nodes_stand_in_columns_centred_on_their_children() {
        let (mut map, [a, b, a1, a2]) = sample();
        let size = |node: &Node, _: usize| (node.text.chars().count() as f32 * 10., 20.);
        let drawn = layout(&map, &size, (30., 10.));
        let of = |id: u64| drawn.nodes.iter().find(|n| n.id == id).unwrap().clone();
        // Columns: a node's children start a gap past its right edge.
        assert_eq!((of(1).x, of(a).x, of(a1).x), (0., 70. + 30., 70. + 30. + 20. + 30.));
        // The two leaves under 方法 one under the other; 方法 in the middle of them; 结果 below; the root in the middle of all.
        assert_eq!((of(a1).y, of(a2).y, of(a).y, of(b).y), (0., 30., 15., 60.));
        assert_eq!((of(1).y, drawn.size), (30., (170., 80.)));
        assert_eq!((of(a1).parent, of(a).children, of(b).parent), (Some(1), 2, Some(0)));
        // Folded away, its children take no room.
        map.fold(a);
        let drawn = layout(&map, &size, (30., 10.));
        assert_eq!((drawn.nodes.len(), drawn.size, drawn.nodes[1].folded), (3, (120., 50.), true));
    }
}
