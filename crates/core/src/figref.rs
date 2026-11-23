//! Figures as a paper's own text points at them (#229): where a page
//! speaks of "Fig. 3B" (`mentions`), where the file shows the figure of
//! that number (`shown`: its caption, and the piece of the page the figure
//! takes, found from what the page draws by the caption), and which of
//! several places a number means (`Index::find`). The reader makes a
//! number in the text a thing to click, and shows that piece beside it.
//!
//! A PDF does not say what a figure is: the caption is the one thing that
//! can be read, and the figure is taken to be what is drawn over it (under
//! it for a table). Where nothing is drawn there, the caption alone is
//! what is shown.

use crate::figure;
use crate::pdftext::{self, Line, Mark};

/// What a number counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Figure,
    Table,
    Scheme,
}

/// Which run of numbers it belongs to: the paper's own, its supplement's
/// ("Fig. S2", "Supplementary Table 1"), or Extended Data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Series {
    Main,
    Supplementary,
    Extended,
}

/// A figure or table as its number names it, however the number is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub kind: Kind,
    pub series: Series,
    pub number: u16,
}

/// The words a number comes after, the longer first: the kind, and whether it speaks of several.
const WORDS: [(&str, Kind, bool); 8] = [
    ("figures", Kind::Figure, true),
    ("figure", Kind::Figure, false),
    ("figs", Kind::Figure, true),
    ("fig", Kind::Figure, false),
    ("tables", Kind::Table, true),
    ("table", Kind::Table, false),
    ("schemes", Kind::Scheme, true),
    ("scheme", Kind::Scheme, false),
];

/// The words that put a number in the supplement's run, the longer first.
const SUPPLEMENT: [&str; 5] = ["supplementary", "supplemental", "supporting", "suppl", "supp"];

fn starts(text: &str, word: &str) -> bool {
    text.get(..word.len()).is_some_and(|start| start.eq_ignore_ascii_case(word))
}

/// The bytes of white space a text opens with.
fn blank(text: &str) -> usize {
    text.len() - text.trim_start().len()
}

/// The words before a number at the start of `rest` ("Fig.", "Supplementary Table", "图"): what they count,
/// in which run, whether they speak of several, and the bytes they take.
fn opener(rest: &str) -> Option<(Kind, Series, bool, usize)> {
    // In Chinese a list needs no plural: 图2、3.
    for (word, kind, series) in [("附图", Kind::Figure, Series::Supplementary), ("附表", Kind::Table, Series::Supplementary), ("图", Kind::Figure, Series::Main), ("表", Kind::Table, Series::Main)] {
        if rest.starts_with(word) { return Some((kind, series, true, word.len())) }
    }
    let (mut at, mut series) = (0, Series::Main);
    if starts(rest, "extended data") {
        (at, series) = ("extended data".len(), Series::Extended);
    } else if let Some(word) = SUPPLEMENT.iter().find(|word| starts(rest, word)) {
        (at, series) = (word.len(), Series::Supplementary);
        if rest[at..].starts_with('.') { at += 1 }
    }
    if at > 0 {
        let apart = blank(&rest[at..]);
        if apart == 0 && !rest[..at].ends_with('.') { return None }
        at += apart;
    }
    let (word, kind, plural) = WORDS.iter().find(|(word, ..)| starts(&rest[at..], word))?;
    at += word.len();
    // "Figured", "Tablet".
    if rest[at..].starts_with(char::is_alphabetic) { return None }
    if rest[at..].starts_with('.') { at += 1 }
    Some((*kind, series, *plural, at))
}

/// A number at the start of `rest`, with the letters of its panels: "3", "S12", "3B", "2a–c", "4A,B". Its
/// value, whether the S puts it in the supplement, and the bytes it takes.
fn number(rest: &str) -> Option<(u16, bool, usize)> {
    let supplement = rest.starts_with(['S', 's']) && rest[1..].starts_with(|c: char| c.is_ascii_digit());
    let mut at = supplement as usize;
    let digits = rest[at..].bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 3 { return None }
    let value = rest[at..at + digits].parse().ok()?;
    at += digits;
    // "Figure 3.5" is a book's numbering by chapter, not figure 3.
    if rest[at..].starts_with('.') && rest[at + 1..].starts_with(|c: char| c.is_ascii_digit()) { return None }
    // A panel is one letter that no word goes on from.
    let panel = |at: usize| rest[at..].starts_with(|c: char| c.is_ascii_alphabetic()) && !rest[at + 1..].starts_with(char::is_alphanumeric);
    if panel(at) {
        at += 1;
        while let Some(between) = rest[at..].chars().next().filter(|c| matches!(c, '–' | '—' | '-' | ',')) {
            if !panel(at + between.len_utf8()) { break }
            at += between.len_utf8() + 1;
        }
    }
    Some((value, supplement, at))
}

/// What stands between two numbers of one list, at the start of `rest`: the bytes up to the next number.
/// A comma lists only after a plural ("Figs. 2, 3"): "in Fig. 2, 30 cells" is one figure.
fn listed(rest: &str, plural: bool) -> Option<usize> {
    let before = blank(rest);
    let tail = &rest[before..];
    let (word, always) = [("and", true), ("&", true), ("to", true), ("–", true), ("—", true), ("-", true), ("~", true), ("～", true), ("至", true), ("和", true), ("及", true), ("与", true), (",", false), (";", false), ("，", false), ("、", false)].into_iter().find(|(word, _)| starts(tail, word))?;
    if !always && !plural { return None }
    let mut at = before + word.len();
    // "and" and "to" are words: "2 and 3", not "2 android".
    if word.is_ascii() && word.chars().all(char::is_alphabetic) && blank(&rest[at..]) == 0 { return None }
    at += blank(&rest[at..]);
    number(&rest[at..]).map(|_| at)
}

/// The figure a caption's number names: "Fig. 3", "Supplementary Figure S12", "图 3".
pub fn key_of(label: &str) -> Option<Key> {
    let label = label.trim_start();
    let (kind, series, _, taken) = opener(label)?;
    let rest = &label[taken..];
    let (number, supplement, _) = number(&rest[blank(rest)..])?;
    Some(Key { kind, series: if supplement && series == Series::Main { Series::Supplementary } else { series }, number })
}

/// A place where a page's text speaks of a figure: the marks that say so (the first and the last, as the
/// page's glyphs are numbered) and the figure meant. "Figs. 2 and 3" is two: `Figs. 2`, and `3`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mention {
    pub from: usize,
    pub to: usize,
    pub key: Key,
}

/// Every place a page's glyphs speak of a figure, a table or a scheme by its number, in reading order. A
/// caption's own number is among them: whoever resolves a mention leaves out the one that is its target.
pub fn mentions(marks: &[Mark]) -> Vec<Mention> {
    // The page as one text, a space wherever a line ends or the page sets two glyphs apart.
    let mut text = String::new();
    let mut starts = Vec::with_capacity(marks.len());
    for (at, mark) in marks.iter().enumerate() {
        if at > 0 && (pdftext::starts_line(&marks[at - 1], mark) || pdftext::spaced(&marks[at - 1], mark)) && !text.ends_with(' ') { text.push(' ') }
        starts.push(text.len());
        text.push_str(&mark.text);
    }
    let mark_of = |byte: usize| starts.partition_point(|start| *start <= byte).saturating_sub(1);
    let mut out = vec![];
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        let first = rest.chars().next().unwrap_or(' ');
        // A word of Latin letters starts where no letter comes before it: "config. 3" names no figure.
        let begins = !first.is_ascii() || !text[..at].chars().next_back().is_some_and(char::is_alphanumeric);
        if begins && let Some((kind, series, plural, taken)) = opener(rest) {
            let mut from = at;
            let mut end = at + taken + blank(&text[at + taken..]);
            let mut any = false;
            while let Some((number, supplement, taken)) = number(&text[end..]) {
                let series = if supplement && series == Series::Main { Series::Supplementary } else { series };
                out.push(Mention { from: mark_of(from), to: mark_of(end + taken - 1), key: Key { kind, series, number } });
                any = true;
                end += taken;
                let Some(between) = listed(&text[end..], plural) else { break };
                from = end + between;
                end = from;
            }
            if any {
                at = end;
                continue;
            }
        }
        at += first.len_utf8();
    }
    out
}

/// A figure or a table where a page shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub key: Key,
    /// The number as the page writes it ("Fig. 3"), and the legend after it.
    pub label: String,
    pub legend: String,
    /// The caption's lines, in the page's points from the top left.
    pub caption: [f32; 4],
    /// The piece of the page the figure itself takes, when what the page draws by the caption tells.
    pub figure: Option<[f32; 4]>,
    /// How surely this is the figure's caption, and not a line of running text that begins with its number.
    pub weight: u8,
}

/// The room left around a piece that is shown.
const PAD: f32 = 6.;

impl Shown {
    /// The piece of the page to look at: the figure with its caption, or the caption alone.
    pub fn piece(&self, page: (f32, f32)) -> [f32; 4] {
        let [x0, y0, x1, y1] = self.figure.map_or(self.caption, |figure| union(figure, self.caption));
        [(x0 - PAD).max(0.), (y0 - PAD).max(0.), (x1 + PAD).min(page.0), (y1 + PAD).min(page.1)]
    }
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

fn line_box(line: &Line) -> [f32; 4] {
    [line.x0, line.y - line.size * 0.8, line.x1, line.y + line.size * 0.25]
}

/// Whether two boxes share some of the page's width.
fn abreast(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2]
}

/// How far a figure may stand from its caption, and one part of it from the next, in points.
const FROM_CAPTION: f32 = 48.;
const BETWEEN_PARTS: f32 = 36.;
/// How far apart two parts of a figure may stand side by side: less than the gap between a page's columns.
const BESIDE: f32 = 24.;
/// A shape thinner than this is a rule: it belongs to a figure it touches, and starts none.
const RULE: f32 = 3.;
/// The least a figure is, each way.
const LEAST: f32 = 32.;
/// How far a table's first rule may stand under its caption, and one rule under the one before it.
const TABLE_TOP: f32 = 40.;
const TABLE_ROWS: f32 = 360.;

/// The shapes drawn on one side of a caption that make up its figure: those near it and abreast of it, then
/// those near them, never past `wall` (the caption of another figure) nor into what stands over another
/// caption beside this one. `over` looks above the caption.
fn gathered(caption: [f32; 4], solid: &[[f32; 4]], others: &[[f32; 4]], wall: f32, over: bool) -> Option<[f32; 4]> {
    // Seen from the caption: how far off a shape's near and far edges are, and how far off the wall is.
    let near = |shape: &[f32; 4]| if over { caption[1] - shape[3] } else { shape[1] - caption[3] };
    let far = |shape: &[f32; 4]| if over { caption[1] - shape[1] } else { shape[3] - caption[3] };
    let limit = if over { caption[1] - wall } else { wall - caption[3] };
    // A shape that is another caption's: abreast of that one and on its figure's side, and not abreast of this.
    let anothers = |shape: &[f32; 4]| !abreast(*shape, caption) && others.iter().any(|other| abreast(*shape, *other) && if over { shape[3] <= other[1] + 4. } else { shape[1] >= other[3] - 4. });
    let mut taken = vec![false; solid.len()];
    let mut region: Option<[f32; 4]> = None;
    // Each round takes what the last one brought within reach; a figure of many parts needs a few.
    for _ in 0..64 {
        let reached = region.map_or(0., |region| far(&region));
        let mut grown = region;
        for (at, shape) in solid.iter().enumerate() {
            if taken[at] || near(shape) < -4. || far(shape) > limit + 2. { continue }
            let joins = match region {
                None => abreast(*shape, caption) && near(shape) <= FROM_CAPTION,
                Some(region) => {
                    let level = shape[1] < region[3] && region[1] < shape[3];
                    let gap = (shape[0] - region[2]).max(region[0] - shape[2]);
                    abreast(*shape, union(region, caption)) && near(shape) - reached <= BETWEEN_PARTS || level && gap <= BESIDE && !anothers(shape)
                }
            };
            if !joins { continue }
            taken[at] = true;
            grown = Some(grown.map_or(*shape, |all| union(all, *shape)));
        }
        if grown == region && region.is_some() || grown.is_none() { break }
        region = grown;
    }
    region.filter(|[x0, y0, x1, y1]| x1 - x0 >= LEAST && y1 - y0 >= LEAST)
}

/// The piece of a page a figure takes, from its caption and what the page draws: `others` are the page's
/// other captions, `shapes` the boxes of its pictures and paths, `loose` its lines that belong to no caption.
fn region(kind: Kind, caption: [f32; 4], others: &[[f32; 4]], shapes: &[[f32; 4]], loose: &[[f32; 4]], page: (f32, f32)) -> Option<[f32; 4]> {
    // What fills the page is its ground, not a figure.
    let shapes: Vec<[f32; 4]> = shapes.iter().copied().filter(|[x0, y0, x1, y1]| !(x1 - x0 > page.0 * 0.8 && y1 - y0 > page.1 * 0.8)).collect();
    let thin = |[x0, y0, x1, y1]: &[f32; 4]| (x1 - x0).min(y1 - y0) < RULE;
    let solid: Vec<[f32; 4]> = shapes.iter().copied().filter(|shape| !thin(shape)).collect();
    // The nearest caption over this one and the nearest under it, abreast of it: no figure reaches past them.
    let above = others.iter().filter(|other| abreast(**other, caption) && other[3] <= caption[1]).map(|other| other[3]).fold(0., f32::max);
    let below = others.iter().filter(|other| abreast(**other, caption) && other[1] >= caption[3]).map(|other| other[1]).fold(page.1, f32::min);
    let mut found = match kind {
        // A table stands under its caption, between rules across it.
        Kind::Table => {
            let mut rules: Vec<[f32; 4]> = shapes.iter().copied().filter(|rule| rule[3] - rule[1] < RULE && rule[2] - rule[0] >= (caption[2] - caption[0]) * 0.4 && abreast(*rule, caption) && rule[1] >= caption[3] - 2. && rule[3] <= below).collect();
            rules.sort_by(|a, b| a[1].total_cmp(&b[1]));
            let mut table: Option<[f32; 4]> = None;
            for rule in rules {
                let (last, reach) = table.map_or((caption[3], TABLE_TOP), |table| (table[3], TABLE_ROWS));
                if rule[1] - last > reach { break }
                table = Some(table.map_or(rule, |table| union(table, rule)));
            }
            table.filter(|table| table[3] - table[1] >= LEAST / 2.).or_else(|| gathered(caption, &solid, others, below, false))?
        }
        // A figure stands over its caption; one whose caption leads it, under.
        Kind::Figure | Kind::Scheme => gathered(caption, &solid, others, above, true).or_else(|| gathered(caption, &solid, others, below, false))?,
    };
    // Rules that touch it are its axes and frames; short lines of text by it are its labels: the numbers
    // along an axis, a panel's letter, the names over a tree. A line as long as the figure is wide is the
    // next column's. Twice, since a label may stand by a rule that was only now taken in.
    let touching = |shape: &[f32; 4], reach: (f32, f32), of: [f32; 4]| shape[0] <= of[2] + reach.0 && shape[2] >= of[0] - reach.0 && shape[1] <= of[3] + reach.1 && shape[3] >= of[1] - reach.1;
    let width = found[2] - found[0];
    for _ in 0..2 {
        let core = found;
        for rule in shapes.iter().filter(|shape| thin(shape) && touching(shape, (6., 6.), core)) {
            if rule[1] >= above && rule[3] <= below { found = union(found, *rule) }
        }
        let core = found;
        for line in loose.iter().filter(|line| line[2] - line[0] <= width * 0.5 && touching(line, (12., 14.), core)) {
            if line[1] >= above && line[3] <= below { found = union(found, *line) }
        }
    }
    Some(found)
}

/// The figures and tables a page shows, in the order it draws their captions: `marks` are its glyphs,
/// `shapes` the boxes of what else it draws (`pdftext::page_drawn`), `page` its size in points.
pub fn shown(marks: &[Mark], shapes: &[[f32; 4]], page: (f32, f32)) -> Vec<Shown> {
    let lines = pdftext::lines(marks);
    let spans = figure::caption_spans(&lines);
    let rects: Vec<[f32; 4]> = spans.iter().map(|(start, end, _)| lines[*start..*end].iter().map(line_box).reduce(union).unwrap_or_default()).collect();
    let captioned = |at: usize| spans.iter().any(|(start, end, _)| (*start..*end).contains(&at));
    let loose: Vec<[f32; 4]> = lines.iter().enumerate().filter(|(at, _)| !captioned(*at)).map(|(_, line)| line_box(line)).collect();
    let mut out = vec![];
    for (at, (start, _, caption)) in spans.iter().enumerate() {
        let Some(key) = key_of(&caption.label) else { continue };
        let line = &lines[*start];
        let others: Vec<[f32; 4]> = rects.iter().enumerate().filter(|(other, _)| *other != at).map(|(_, rect)| *rect).collect();
        let figure = region(key.kind, rects[at], &others, shapes, &loose, page);
        // Running text that happens to begin a line with "Fig. 3. The…" looks like a caption too. A real one
        // is set apart from the line before it, opens in bold, says something, and has its figure by it.
        let apart = start.checked_sub(1).map(|before| &lines[before]).is_none_or(|before| {
            let leading = line.y - before.y;
            leading < 0. || leading > line.size.max(before.size) * 1.6 || before.x1 <= line.x0 || before.x0 >= line.x1 || (before.size - line.size).abs() > line.size * 0.06
        });
        let weight = 2 * (line.bold || line.bold_opening > 0) as u8 + apart as u8 + !caption.legend.is_empty() as u8 + 2 * figure.is_some() as u8;
        out.push(Shown { key, label: caption.label.clone(), legend: caption.legend.clone(), caption: rects[at], figure, weight });
    }
    out
}

/// What a page draws that no caption on it claims, when it is large enough to be a figure: the figure of a
/// caption on another page (a legend carried over to the next page, or the legends listed apart from the
/// figures, as a manuscript has them). `shown` is the page's own (`shown`).
pub fn unclaimed(shapes: &[[f32; 4]], shown: &[Shown], page: (f32, f32)) -> Option<[f32; 4]> {
    let claimed = |shape: &[f32; 4]| shown.iter().filter_map(|shown| shown.figure).any(|figure| shape[0] >= figure[0] - 4. && shape[1] >= figure[1] - 4. && shape[2] <= figure[2] + 4. && shape[3] <= figure[3] + 4.);
    shapes
        .iter()
        .copied()
        .filter(|shape| {
            let (wide, tall) = (shape[2] - shape[0], shape[3] - shape[1]);
            wide.min(tall) >= RULE && !(wide > page.0 * 0.8 && tall > page.1 * 0.8) && !claimed(shape)
        })
        .reduce(union)
        .filter(|[x0, y0, x1, y1]| x1 - x0 >= ALONE.0 && y1 - y0 >= ALONE.1)
}

/// The least a figure with no caption by it is, across and down: smaller is a journal's mark or an icon.
const ALONE: (f32, f32) = (120., 90.);
/// How far down its page a legend carried over from the figure's page may begin, as a share of the page.
const CARRIED: f32 = 0.2;

/// Where a file shows its figures: every page's `shown`, by 0-based page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Index {
    places: Vec<Place>,
    /// What each page draws that none of its captions claims (`unclaimed`), with the page's height.
    loose: Vec<(usize, [f32; 4])>,
    heights: Vec<(usize, f32)>,
}

/// A figure where the file shows it: the page of its caption (0-based) and, when the figure itself is
/// drawn on another page, that page and the piece of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub page: usize,
    pub shown: Shown,
    pub apart: Option<(usize, [f32; 4])>,
}

impl Place {
    /// What to look at, in reading order: the figure with its caption as one piece of a page, or, when
    /// they are on two pages, the figure and then the caption. `size` is a page's size in points.
    pub fn pieces(&self, size: impl Fn(usize) -> (f32, f32)) -> Vec<(usize, [f32; 4])> {
        let padded = |[x0, y0, x1, y1]: [f32; 4], page: (f32, f32)| [(x0 - PAD).max(0.), (y0 - PAD).max(0.), (x1 + PAD).min(page.0), (y1 + PAD).min(page.1)];
        match self.apart {
            Some((page, figure)) => vec![(page, padded(figure, size(page))), (self.page, padded(self.shown.caption, size(self.page)))],
            None => vec![(self.page, self.shown.piece(size(self.page)))],
        }
    }

    /// Whether the figure itself was found, not its caption alone.
    pub fn pictured(&self) -> bool {
        self.shown.figure.is_some() || self.apart.is_some()
    }
}

/// Whether a mention on a 0-based page stands within the figure it would lead to: the caption's own
/// number, words of its legend, or a label in the figure itself. It leads nowhere but to where it is.
pub fn own(marks: &[Mark], mention: &Mention, page: usize, place: &Place) -> bool {
    let Some(first) = marks.get(mention.from) else { return false };
    let (x, y) = ((first.x0 + first.x1) / 2., first.y - first.size * 0.3);
    let inside = |[x0, y0, x1, y1]: [f32; 4]| x >= x0 - 1. && x <= x1 + 1. && y >= y0 - 1. && y <= y1 + 1.;
    page == place.page && (inside(place.shown.caption) || place.shown.figure.is_some_and(inside)) || place.apart.is_some_and(|(at, figure)| at == page && inside(figure))
}

impl Index {
    /// A page's figures, what else it draws (`unclaimed`), and its height in points.
    pub fn add(&mut self, page: usize, shown: Vec<Shown>, loose: Option<[f32; 4]>, height: f32) {
        self.places.extend(shown.into_iter().map(|shown| Place { page, shown, apart: None }));
        self.loose.extend(loose.map(|loose| (page, loose)));
        self.heights.push((page, height));
    }

    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    pub fn places(&self) -> &[Place] {
        &self.places
    }

    /// Once every page is in: give the captions that have no figure by them the figures that have no
    /// caption. A legend that opens its page belongs to what the page before draws. Legends still alone
    /// are a list of them, and the figures follow on pages of their own, one a page, in the same order;
    /// they are matched only when there are as many of the one as of the other.
    pub fn settle(&mut self) {
        self.places.sort_by_key(|place| place.page);
        let loose = self.loose.clone();
        let on = |page: usize| loose.iter().find(|(at, _)| *at == page).map(|(_, piece)| *piece);
        let captioned: Vec<usize> = self.places.iter().map(|place| place.page).collect();
        let mut used: Vec<usize> = vec![];
        let alone = |place: &Place| place.shown.key.kind != Kind::Table && place.shown.figure.is_none() && place.apart.is_none();
        for at in 0..self.places.len() {
            let place = &self.places[at];
            let height = self.heights.iter().find(|(page, _)| *page == place.page).map_or(f32::MAX, |(_, height)| *height);
            // Only the first caption of its page can be carried over from the page before.
            let leads = !self.places[..at].iter().any(|other| other.page == place.page);
            if !alone(place) || !leads || place.shown.caption[1] > height * CARRIED || place.page == 0 { continue }
            let before = place.page - 1;
            if used.contains(&before) { continue }
            if let Some(figure) = on(before) {
                used.push(before);
                self.places[at].apart = Some((before, figure));
                self.places[at].shown.weight += 2;
            }
        }
        let legends: Vec<usize> = (0..self.places.len()).filter(|at| alone(&self.places[*at])).collect();
        let Some(last) = legends.iter().map(|at| self.places[*at].page).max() else { return };
        let figures: Vec<(usize, [f32; 4])> = loose.iter().copied().filter(|(page, _)| *page > last && !captioned.contains(page) && !used.contains(page)).collect();
        if figures.len() != legends.len() { return }
        for (at, figure) in legends.into_iter().zip(figures) {
            self.places[at].apart = Some(figure);
            self.places[at].shown.weight += 2;
        }
    }

    /// The place a number means: of the captions that name it, the surest; of equals, the first in the file.
    pub fn find(&self, key: Key) -> Option<&Place> {
        let mut best: Option<&Place> = None;
        for place in self.places.iter().filter(|place| place.shown.key == key) {
            if best.is_none_or(|best| place.shown.weight > best.shown.weight || place.shown.weight == best.shown.weight && place.page < best.page) { best = Some(place) }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines of text as a page's glyphs: one a character, 5 points wide, a line every 12 points from `top`.
    fn glyphs(top: f32, left: f32, lines: &[&str]) -> Vec<Mark> {
        let mut marks = vec![];
        for (row, line) in lines.iter().enumerate() {
            for (column, c) in line.chars().enumerate() {
                let x0 = left + column as f32 * 5.;
                marks.push(Mark { text: c.to_string(), x0, x1: x0 + 5., y: top + row as f32 * 12., size: 10., bold: false });
            }
        }
        marks
    }

    fn figure(number: u16) -> Key {
        Key { kind: Kind::Figure, series: Series::Main, number }
    }

    /// What a page's text names, as written and as meant.
    fn named(lines: &[&str]) -> Vec<(String, Key)> {
        let marks = glyphs(100., 50., lines);
        mentions(&marks).into_iter().map(|mention| (pdftext::marks_text(&marks[mention.from..=mention.to]), mention.key)).collect()
    }

    #[test]
    fn numbers_in_running_text() {
        let supplement = Key { kind: Kind::Figure, series: Series::Supplementary, number: 2 };
        let table = Key { kind: Kind::Table, series: Series::Main, number: 1 };
        assert_eq!(
            named(&["as shown in Fig. 3B and Figs. 2 and 4a–c (Supplementary Fig. S2, Table 1)."]),
            vec![("Fig. 3B".to_string(), figure(3)), ("Figs. 2".to_string(), figure(2)), ("4a–c".to_string(), figure(4)), ("Supplementary Fig. S2".to_string(), supplement), ("Table 1".to_string(), table)]
        );
        // Written without the word, the S alone says it is the supplement's; Extended Data is a run of its own.
        assert_eq!(named(&["(Fig.S2c; Extended Data Fig. 4)"]), vec![("Fig.S2c".to_string(), supplement), ("Extended Data Fig. 4".to_string(), Key { kind: Kind::Figure, series: Series::Extended, number: 4 })]);
        // A number broken off its word at the line's end is still that figure's.
        assert_eq!(named(&["the roots bend (Fig.", "3) within hours"]), vec![("Fig. 3".to_string(), figure(3))]);
        // A list after a plural; a comma after one figure lists nothing.
        assert_eq!(named(&["Figures 2, 3 and 5"]).iter().map(|(_, key)| key.number).collect::<Vec<_>>(), vec![2, 3, 5]);
        assert_eq!(named(&["in Fig. 2, 30 cells were counted"]), vec![("Fig. 2".to_string(), figure(2))]);
    }

    #[test]
    fn words_that_name_no_figure() {
        assert_eq!(named(&["see config. 3, the Tablet 2 and what was Figured 4"]), vec![]);
        // A book's numbering by chapter is not figure 3.
        assert_eq!(named(&["Figure 3.5 shows"]), vec![]);
        assert_eq!(named(&["Fig. A1 and Table of contents"]), vec![]);
    }

    #[test]
    fn numbers_in_chinese() {
        let table = |number| Key { kind: Kind::Table, series: Series::Main, number };
        assert_eq!(
            named(&["如图3所示，结果见表 2、3 和附图 1。"]),
            vec![("图3".to_string(), figure(3)), ("表 2".to_string(), table(2)), ("3".to_string(), table(3)), ("附图 1".to_string(), Key { kind: Kind::Figure, series: Series::Supplementary, number: 1 })]
        );
        assert_eq!(named(&["结果表明 3 个样本"]), vec![]);
    }

    #[test]
    fn a_caption_names_its_figure() {
        assert_eq!(key_of("Fig. 3"), Some(figure(3)));
        assert_eq!(key_of("FIGURE 12"), Some(figure(12)));
        assert_eq!(key_of("图 3"), Some(figure(3)));
        assert_eq!(key_of("Supplementary Figure S12"), Some(Key { kind: Kind::Figure, series: Series::Supplementary, number: 12 }));
        assert_eq!(key_of("Supplemental Table 2"), Some(Key { kind: Kind::Table, series: Series::Supplementary, number: 2 }));
        assert_eq!(key_of("Table S1"), Some(Key { kind: Kind::Table, series: Series::Supplementary, number: 1 }));
        assert_eq!(key_of("Extended Data Fig. 4"), Some(Key { kind: Kind::Figure, series: Series::Extended, number: 4 }));
        assert_eq!(key_of("Scheme 1"), Some(Key { kind: Kind::Scheme, series: Series::Main, number: 1 }));
        assert_eq!(key_of("Figure"), None);
    }

    const PAGE: (f32, f32) = (595., 842.);

    #[test]
    fn a_figure_is_what_is_drawn_over_its_caption() {
        // Text, a figure of two panels with an axis and its numbers, the caption, text again.
        let mut marks = glyphs(80., 60., &["Roots bend down within hours of being turned on their side."]);
        marks.extend(glyphs(318., 96., &["0", "", "", "", "", "", "", "", "", "", "10"]));
        marks.extend(glyphs(460., 60., &["Fig. 1 | Bending of the root.", "a, Control. b, Treated with auxin."]));
        marks.extend(glyphs(520., 60., &["The response was the same in every line that was looked at here."]));
        let shapes = [[110., 150., 290., 440.], [300., 150., 480., 440.], [108., 150., 108., 440.], [0., 0., 595., 842.], [60., 40., 535., 40.5]];
        let shown = shown(&marks, &shapes, PAGE);
        assert_eq!(shown.len(), 1);
        assert_eq!((shown[0].key, shown[0].label.as_str()), (figure(1), "Fig. 1"));
        let [x0, y0, x1, y1] = shown[0].figure.expect("the figure is found");
        // The two panels, the rule that is the axis, and the numbers along it; not the page's ground, the rule under its header, or the text.
        assert!((90. ..=108.).contains(&x0) && (y0 - 150.).abs() < 1. && (x1 - 480.).abs() < 1. && (y1 - 440.).abs() < 12., "{:?}", shown[0].figure);
        // What is looked at is the figure with its caption under it.
        let piece = shown[0].piece(PAGE);
        assert!(piece[1] < 150. && piece[3] > 472. && piece[3] < 500., "{piece:?}");
        assert_eq!(unclaimed(&shapes, &shown, PAGE), None);
    }

    #[test]
    fn a_figure_stops_at_the_caption_of_the_one_over_it() {
        let mut marks = glyphs(300., 60., &["Fig. 1 | The first."]);
        marks.extend(glyphs(560., 60., &["Fig. 2 | The second."]));
        let shapes = [[80., 100., 500., 280.], [80., 330., 500., 540.]];
        let shown = shown(&marks, &shapes, PAGE);
        assert_eq!(shown.iter().map(|shown| shown.figure.map(|figure| (figure[1], figure[3]))).collect::<Vec<_>>(), vec![Some((100., 280.)), Some((330., 540.))]);
    }

    #[test]
    fn a_table_is_what_stands_between_the_rules_under_its_caption() {
        let mut marks = glyphs(100., 60., &["Table 1. Primers used in this study"]);
        marks.extend(glyphs(125., 60., &["Gene      Forward        Reverse", "WOX5      ATGGC          TTAGC", "PIN2      ATGAT          TCAAA"]));
        let shapes = [[60., 110., 400., 110.5], [60., 130., 400., 130.5], [60., 160., 400., 160.5]];
        let shown = shown(&marks, &shapes, PAGE);
        let table = shown[0].figure.expect("the table is found");
        assert!((table[1] - 110.).abs() < 1. && (table[3] - 160.5).abs() < 1., "{table:?}");
        // A caption with nothing drawn by it is shown alone.
        let alone = super::shown(&glyphs(100., 60., &["Fig. 2 | Nothing here."]), &[], PAGE);
        assert_eq!(alone[0].figure, None);
        assert_eq!(alone[0].piece(PAGE), [54., 86., 60. + 22. * 5. + 6., 108.5]);
    }

    fn caption(number: u16, top: f32, figure: Option<[f32; 4]>) -> Shown {
        Shown { key: self::figure(number), label: format!("Fig. {number}"), legend: "A legend.".into(), caption: [60., top, 500., top + 12.], figure, weight: 2 + 2 * figure.is_some() as u8 }
    }

    #[test]
    fn a_legend_carried_over_takes_the_figure_of_the_page_before() {
        let mut index = Index::default();
        index.add(0, vec![], Some([50., 50., 540., 600.]), 782.);
        index.add(1, vec![caption(3, 51., None)], None, 782.);
        index.settle();
        let place = index.find(figure(3)).unwrap();
        assert_eq!((place.page, place.apart), (1, Some((0, [50., 50., 540., 600.]))));
        // The figure first, then its caption, each on its own page.
        assert_eq!(place.pieces(|_| (595., 782.)).iter().map(|(page, _)| *page).collect::<Vec<_>>(), vec![0, 1]);
        // A caption half way down its page was not carried over.
        let mut index = Index::default();
        index.add(0, vec![], Some([50., 50., 540., 600.]), 782.);
        index.add(1, vec![caption(3, 400., None)], None, 782.);
        index.add(2, vec![caption(4, 300., Some([60., 60., 500., 290.]))], None, 782.);
        index.settle();
        assert_eq!(index.find(figure(3)).unwrap().apart, None);
    }

    #[test]
    fn legends_listed_apart_take_the_figures_that_follow_in_order() {
        let pages = |figures: usize| {
            let mut index = Index::default();
            index.add(4, vec![caption(1, 100., None), caption(2, 480., None)], None, 842.);
            index.add(5, vec![caption(3, 300., None)], None, 842.);
            for page in 0..figures { index.add(8 + page, vec![], Some([72., 72., 523., 400. + page as f32]), 842.) }
            index.settle();
            index
        };
        let index = pages(3);
        assert_eq!([1, 2, 3].map(|number| index.find(figure(number)).unwrap().apart.map(|(page, _)| page)), [Some(8), Some(9), Some(10)]);
        assert!(index.find(figure(2)).unwrap().pictured());
        // One figure more or less than there are legends: which is whose is not known.
        let index = pages(2);
        assert_eq!([1, 2, 3].map(|number| index.find(figure(number)).unwrap().apart), [None, None, None]);
    }

    #[test]
    fn of_two_places_the_surer_is_meant_and_a_caption_does_not_lead_to_itself() {
        // Running text that begins a line with the number, and the caption itself with its figure.
        let mut index = Index::default();
        index.add(1, vec![Shown { weight: 1, ..caption(2, 300., None) }], None, 842.);
        index.add(3, vec![caption(2, 500., Some([60., 100., 500., 490.]))], None, 842.);
        index.settle();
        let place = index.find(figure(2)).unwrap();
        assert_eq!(place.page, 3);
        // On the figure's page: its caption's own number, and a word in the text over the figure.
        let mut marks = glyphs(510., 60., &["Fig. 2 | A legend."]);
        marks.extend(glyphs(60., 60., &["as Fig. 2 shows"]));
        let found = mentions(&marks);
        assert_eq!(found.iter().map(|mention| own(&marks, mention, 3, place)).collect::<Vec<_>>(), vec![true, false]);
        // The same words on another page lead to it.
        assert!(!own(&marks, &found[0], 0, place));
    }
}
