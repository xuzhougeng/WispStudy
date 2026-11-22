//! 分栏 (#194): the reading area as columns on an eight-unit grid, each a pane with one use: a paper,
//! the translation of the paper beside it, the conversation, the list of papers. A layout is what the
//! person wished for (`4,2,2`: a paper on half the width, its translation and the conversation on a
//! quarter each); `arrange` says what a window of a given width shows of it. Plain arithmetic in logical
//! pixels, measured from the area's left edge; the window draws whatever this gives.
//!
//! The frame stays and the content gives way (docs/dev/design.md): a pane is not squeezed below the width
//! it can be read at. Where the window is too narrow for a layout, its panes give way in an order fixed
//! here: the list is put away; the translations are drawn inside their papers (the original over its
//! translation), all of them at once so that they look alike; the conversation floats over a paper; the
//! last paper is put away with its translation, a click away (the window says how many are, and shows one
//! in the last pane it has room for). A paper is never squeezed into half of another's pane: half a page
//! high, neither is read. A translation that goes into its paper's pane brings its units with it: the pane
//! that now holds both keeps the width of both, and the conversation beside them is no wider for it.
//! Columns only, never rows: each pane is one face from the top of the window to its foot.
//!
//! With several papers on show the papers are what the window is for (`frame`): the list keeps the width
//! the person gave it while there is room and is the first to go, the conversation has its share or the
//! width it was dragged to, and neither takes what a paper needs to be read.

/// The grid a layout is written on: the units of its columns add up to this.
pub const UNITS: u32 = 8;
/// The most columns a layout has.
pub const MAX_COLUMNS: usize = 4;

/// What a pane is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    /// The papers of the library, a narrow column to pick from.
    List,
    /// One paper: its PDF, or its record when it has none.
    Paper,
    /// The paper next to it with each paragraph translated (`source`), as wide as that paper: the two are
    /// read at one scale, side by side.
    Translation,
    /// The conversation about the papers on show.
    Chat,
}

impl Pane {
    pub const ALL: [Pane; 4] = [Pane::List, Pane::Paper, Pane::Translation, Pane::Chat];

    /// The key a layout is written with.
    pub fn key(self) -> &'static str {
        match self {
            Pane::List => "list",
            Pane::Paper => "paper",
            Pane::Translation => "translation",
            Pane::Chat => "chat",
        }
    }

    pub fn from_key(key: &str) -> Option<Pane> {
        Pane::ALL.into_iter().find(|pane| pane.key() == key)
    }

    /// Its name in the window; translated where it is shown.
    pub fn label(self) -> &'static str {
        match self {
            Pane::List => "列表",
            Pane::Paper => "文献",
            Pane::Translation => "译文",
            Pane::Chat => "对话",
        }
    }

    /// The narrowest it is still read at: the window's own limits for the narrow list beside a paper asked
    /// about and the conversation beside a PDF (`library.rs`). A paper beside another is read at the width
    /// its pages fit: narrower than this, a two-column page is under three fifths of its size and its
    /// text is not read; three papers still stand side by side in a window 1360 wide.
    pub fn min_width(self) -> f32 {
        match self {
            Pane::List => 200.,
            Pane::Paper | Pane::Translation => 440.,
            Pane::Chat => 320.,
        }
    }
}

/// A column of a layout: its pane and how many units of the grid it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    pub pane: Pane,
    pub units: u32,
}

const fn column(pane: Pane, units: u32) -> Column {
    Column { pane, units }
}

/// A layout offered by name.
pub struct Preset {
    pub key: &'static str,
    /// Its name in the window; translated where it is shown.
    pub name: &'static str,
    pub columns: &'static [Column],
}

/// The layouts offered, the plainest first. `ask` is the window's layout for a paper asked about.
pub const PRESETS: [Preset; 6] = [
    Preset { key: "read", name: "阅读", columns: &[column(Pane::Paper, 8)] },
    Preset { key: "ask", name: "提问", columns: &[column(Pane::List, 2), column(Pane::Chat, 3), column(Pane::Paper, 3)] },
    Preset { key: "translate", name: "对照译文", columns: &[column(Pane::Paper, 4), column(Pane::Translation, 4)] },
    Preset { key: "compare", name: "两篇对照", columns: &[column(Pane::Paper, 4), column(Pane::Paper, 4)] },
    Preset { key: "study", name: "精读", columns: &[column(Pane::Paper, 3), column(Pane::Translation, 3), column(Pane::Chat, 2)] },
    Preset { key: "compare-translate", name: "两篇对照带译文", columns: &[column(Pane::Paper, 2), column(Pane::Translation, 2), column(Pane::Paper, 2), column(Pane::Translation, 2)] },
];

pub fn preset(key: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.key == key)
}

/// How the papers on show are read, whatever their number: each paper alone (阅读), each with its translation
/// (对照译文), the list and the conversation beside them (提问), each with its translation and the conversation
/// (精读). A mode's columns repeat each paper's (`columns`); with one paper they are the `PRESETS` of its key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Read,
    Translate,
    Ask,
    Study,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Read, Mode::Translate, Mode::Ask, Mode::Study];

    pub fn key(self) -> &'static str {
        match self {
            Mode::Read => "read",
            Mode::Translate => "translate",
            Mode::Ask => "ask",
            Mode::Study => "study",
        }
    }

    pub fn from_key(key: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|mode| mode.key() == key)
    }

    /// Its name in the window; translated where it is shown.
    pub fn label(self) -> &'static str {
        preset(self.key()).map_or("", |preset| preset.name)
    }

    /// Whether each paper has its translation beside it.
    pub fn translates(self) -> bool {
        matches!(self, Mode::Translate | Mode::Study)
    }
}

/// A mode's columns for `papers` papers (one at least), in their order: each paper's columns are what they
/// are with one paper, once for each paper. 提问 is 2·3·3 with one paper and 2·3·3·3 with two: a paper beside
/// another is given what it has alone, and it is the list and the conversation that have a smaller share of
/// the window (`frame`).
pub fn columns(mode: Mode, papers: usize) -> Vec<Column> {
    let one = preset(mode.key()).map_or(&[][..], |preset| preset.columns);
    let of_paper = |c: &Column| matches!(c.pane, Pane::Paper | Pane::Translation);
    // What stands before the papers stays before them, what stands after them after.
    let first = one.iter().position(of_paper).unwrap_or(one.len());
    let end = one.iter().rposition(of_paper).map_or(first, |last| last + 1);
    let mut out = one[..first].to_vec();
    for _ in 0..papers.max(1) { out.extend_from_slice(&one[first..end]) }
    out.extend_from_slice(&one[end..]);
    out
}

/// The units of columns on the grid of `UNITS`, as the layout's name says them (`2·2·2·2`); none when they are on a finer grid.
pub fn grid(columns: &[Column]) -> Option<String> {
    (columns.iter().map(|c| c.units).sum::<u32>() == UNITS).then(|| columns.iter().map(|c| c.units.to_string()).collect::<Vec<_>>().join("·"))
}

/// Why these columns are no layout, if they are not one: a sentence for the person.
pub fn refusal(columns: &[Column]) -> Option<String> {
    if columns.is_empty() { return Some("至少要有一栏。".into()) }
    if columns.len() > MAX_COLUMNS { return Some(format!("最多分 {MAX_COLUMNS} 栏。")) }
    if columns.iter().any(|c| c.units == 0) { return Some("每一栏至少占 1 格。".into()) }
    let sum: u32 = columns.iter().map(|c| c.units).sum();
    if sum != UNITS { return Some(format!("各栏加起来要正好 {UNITS} 格，现在是 {sum} 格。")) }
    for pane in [Pane::List, Pane::Chat] {
        if columns.iter().filter(|c| c.pane == pane).count() > 1 { return Some(format!("「{}」只能有一栏。", pane.label())) }
    }
    if !columns.iter().any(|c| c.pane == Pane::Paper) { return Some("至少要有一栏「文献」。".into()) }
    let mut translated = vec![];
    for (i, c) in columns.iter().enumerate().filter(|(_, c)| c.pane == Pane::Translation) {
        let Some(paper) = source(columns, i) else { return Some("「译文」要紧挨着它译的那栏「文献」。".into()) };
        if columns[paper].units != c.units { return Some("「译文」要和它译的那栏「文献」一样宽：两边的页面按同一个比例显示。".into()) }
        if translated.contains(&paper) { return Some("一栏「文献」只配一栏「译文」。".into()) }
        translated.push(paper);
    }
    None
}

/// Columns written as `paper:4,translation:2,chat:2`, as a layout is kept.
pub fn write(columns: &[Column]) -> String {
    columns.iter().map(|c| format!("{}:{}", c.pane.key(), c.units)).collect::<Vec<_>>().join(",")
}

/// The columns `write` wrote, if they make a layout; else why not.
pub fn parse(text: &str) -> Result<Vec<Column>, String> {
    let columns = text
        .split(',')
        .map(|part| {
            let (pane, units) = part.trim().split_once(':').ok_or_else(|| format!("没有读懂「{}」：每一栏写成「栏:格数」。", part.trim()))?;
            let pane = Pane::from_key(pane.trim()).ok_or_else(|| format!("没有「{}」这种栏。", pane.trim()))?;
            let units = units.trim().parse::<u32>().map_err(|_| format!("「{}」不是格数。", units.trim()))?;
            Ok(Column { pane, units })
        })
        .collect::<Result<Vec<_>, String>>()?;
    match refusal(&columns) {
        Some(why) => Err(why),
        None => Ok(columns),
    }
}

/// The paper a translation at `at` translates: the column at its left when that is a paper, else the one at its right.
pub fn source(columns: &[Column], at: usize) -> Option<usize> {
    [at.checked_sub(1), Some(at + 1)].into_iter().flatten().find(|&i| columns.get(i).is_some_and(|c| c.pane == Pane::Paper))
}

/// The width at which a layout shows every pane on its own: the narrowest width of each, and the gaps.
pub fn needs(columns: &[Column], gap: f32) -> f32 {
    columns.iter().map(|c| c.pane.min_width()).sum::<f32>() + gap * columns.len().saturating_sub(1) as f32
}

/// Where a column of a layout is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    /// A pane of its own: its left edge and its width.
    Shown { x: f32, width: f32 },
    /// Inside column `n`'s pane: a translation under its original.
    Inside(usize),
    /// Floating over column `n`'s pane: the conversation.
    Over(usize),
    /// Not drawn: the list, which comes back when the window is wider; a paper the window has no room for, and its
    /// translation with it; a translation with no room in its paper's pane.
    Away,
}

impl Placement {
    pub fn shown(self) -> bool {
        matches!(self, Placement::Shown { .. })
    }
}

/// What a window `width` wide shows of a layout, with `gap` between two panes: each column's place, in
/// the layout's order.
pub fn arrange(columns: &[Column], width: f32, gap: f32) -> Vec<Placement> {
    let width = if width.is_finite() { width.max(0.) } else { 0. };
    let gap = if gap.is_finite() { gap.max(0.) } else { 0. };
    let mut put: Vec<Option<Placement>> = vec![None; columns.len()];
    // What each pane shares the width by: its column's units, and those of a column that went inside it.
    let mut units: Vec<u32> = columns.iter().map(|c| c.units).collect();
    loop {
        let shown: Vec<usize> = (0..columns.len()).filter(|&i| put[i].is_none()).collect();
        let room = width - gap * shown.len().saturating_sub(1) as f32;
        let least: f32 = shown.iter().map(|&i| columns[i].pane.min_width()).sum();
        if least <= room || !give_way(columns, &mut put, &mut units) { break }
    }
    let shown: Vec<usize> = (0..columns.len()).filter(|&i| put[i].is_none()).collect();
    let room = (width - gap * shown.len().saturating_sub(1) as f32).max(0.);
    let widths = share(&shown.iter().map(|&i| (units[i], columns[i].pane.min_width())).collect::<Vec<_>>(), room);
    let mut x = 0.;
    for (&i, &w) in shown.iter().zip(&widths) {
        put[i] = Some(Placement::Shown { x, width: w });
        x += w + gap;
    }
    put.into_iter().map(|placement| placement.unwrap_or(Placement::Away)).collect()
}

/// One step of giving way, the next in the order the module's comment gives; false when nothing is left to give.
/// A translation that goes into its paper's pane adds its `units` to that one's.
fn give_way(columns: &[Column], put: &mut [Option<Placement>], units: &mut [u32]) -> bool {
    let shown = |put: &[Option<Placement>], pane: Pane| -> Vec<usize> { (0..columns.len()).filter(|&i| put[i].is_none() && columns[i].pane == pane).collect() };
    let lists = shown(put, Pane::List);
    if !lists.is_empty() {
        for i in lists { put[i] = Some(Placement::Away) }
        return true;
    }
    let translations = shown(put, Pane::Translation);
    if !translations.is_empty() {
        for i in translations {
            // Its paper's pane takes it, unless that pane holds something already (another translation of the same paper).
            let host = source(columns, i).filter(|&p| put[p].is_none() && !holds(put, p));
            if let Some(host) = host { units[host] += units[i] }
            put[i] = Some(host.map_or(Placement::Away, Placement::Inside));
        }
        return true;
    }
    let papers = shown(put, Pane::Paper);
    if let Some(&over) = papers.first() && let Some(chat) = shown(put, Pane::Chat).first().copied() {
        put[chat] = Some(Placement::Over(over));
        return true;
    }
    // The last paper is put away, and its translation with it.
    // (The conversation floats over the first paper, never over the one that goes.)
    if let Some(&gone) = papers.last().filter(|_| papers.len() > 1) {
        for p in put.iter_mut() {
            if *p == Some(Placement::Inside(gone)) { *p = Some(Placement::Away) }
        }
        put[gone] = Some(Placement::Away);
        return true;
    }
    false
}

/// Whether the pane of column `n` holds another column inside it.
fn holds(put: &[Option<Placement>], n: usize) -> bool {
    put.contains(&Some(Placement::Inside(n)))
}

/// The least the conversation has beside several papers while it is not dragged narrower: under this the line
/// that says what it reads is cut to a word.
const CHAT_AT_EASE: f32 = 360.;

/// What a window gives the list and the conversation around several papers, and what it leaves the papers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// The list's width; 0 when it is put away.
    pub list: f32,
    /// The conversation's width; 0 when the layout has none.
    pub chat: f32,
    /// What is left for the papers and their translations, which `arrange` lays out among themselves.
    pub papers: f32,
}

/// The frame of a window `width` wide around the papers of `columns` (a mode's, `columns`), with `gap`
/// between two panes. `list` is the width the list is kept at (0: this window shows none): it has that
/// while every pane beside it keeps its least width, less while it keeps its own, and is put away before
/// anything else gives. `chat` is the width the conversation was dragged to (0: no wish, it has its units'
/// share of what the list leaves): never so wide that a paper standing beside it could not be read.
pub fn frame(columns: &[Column], width: f32, gap: f32, list: f32, chat: f32) -> Frame {
    let width = if width.is_finite() { width.max(0.) } else { 0. };
    let gap = if gap.is_finite() { gap.max(0.) } else { 0. };
    let rest: Vec<Column> = columns.iter().filter(|c| c.pane != Pane::List).copied().collect();
    let least = |placed: &[Placement], columns: &[Column], pane: fn(Pane) -> bool| -> (f32, usize) {
        let shown: Vec<f32> = columns.iter().zip(placed).filter(|(c, p)| p.shown() && pane(c.pane)).map(|(c, _)| c.pane.min_width()).collect();
        (shown.iter().sum(), shown.len())
    };
    // The list: there while the window shows every pane with it, as wide as the others' least widths let it be.
    let list = if list > 0. {
        let with: Vec<Column> = std::iter::once(column(Pane::List, 2)).chain(rest.iter().copied()).collect();
        let placed = arrange(&with, width, gap);
        if placed[0].shown() {
            let (others, count) = least(&placed, &with, |pane| pane != Pane::List);
            list.min(width - others - gap * count as f32).max(Pane::List.min_width())
        } else {
            0.
        }
    } else {
        0.
    };
    let room = (width - if list > 0. { list + gap } else { 0. }).max(0.);
    let Some(at) = rest.iter().position(|c| c.pane == Pane::Chat) else { return Frame { list, chat: 0., papers: room } };
    let placed = arrange(&rest, room, gap);
    let (papers, count) = least(&placed, &rest, |pane| pane != Pane::Chat);
    let most = (room - papers - gap * count as f32).max(Pane::Chat.min_width());
    let chat = match placed[at] {
        _ if chat > 0. => chat.clamp(Pane::Chat.min_width(), most),
        // Its share, and not less than it is read at with ease while the papers can spare that.
        Placement::Shown { width, .. } => width.max(CHAT_AT_EASE).min(most),
        // No room for it beside the papers: it keeps its least width and the papers arrange themselves in the rest.
        _ => Pane::Chat.min_width().min(room),
    };
    Frame { list, chat, papers: (room - chat - gap).max(0.) }
}

/// The widths of columns `(units, least width)` sharing `room`: each its units' share, but none below its
/// least width while the room has that much; the others share what is left, still by their units. Whole
/// pixels, the last column taking what rounding leaves, so that the widths fill the room exactly.
fn share(columns: &[(u32, f32)], room: f32) -> Vec<f32> {
    if columns.is_empty() { return vec![] }
    let enough = columns.iter().map(|c| c.1).sum::<f32>() <= room;
    let mut fixed: Vec<Option<f32>> = vec![None; columns.len()];
    loop {
        let units: u32 = columns.iter().zip(&fixed).filter(|(_, f)| f.is_none()).map(|(c, _)| c.0).sum();
        let left = room - fixed.iter().flatten().sum::<f32>();
        let mut changed = false;
        if enough && units > 0 {
            for (i, (u, least)) in columns.iter().enumerate() {
                if fixed[i].is_none() && left * (*u as f32) / (units as f32) < *least {
                    fixed[i] = Some(*least);
                    changed = true;
                }
            }
        }
        if !changed { break }
    }
    let units: u32 = columns.iter().zip(&fixed).filter(|(_, f)| f.is_none()).map(|(c, _)| c.0).sum();
    let left = room - fixed.iter().flatten().sum::<f32>();
    let mut widths: Vec<f32> = columns.iter().zip(&fixed).map(|((u, _), f)| f.unwrap_or(if units > 0 { left * *u as f32 / units as f32 } else { 0. }).floor()).collect();
    let last = widths.len() - 1;
    widths[last] = (room - widths[..last].iter().sum::<f32>()).max(0.);
    widths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widths(placements: &[Placement]) -> Vec<f32> {
        placements.iter().filter_map(|p| match p { Placement::Shown { width, .. } => Some(*width), _ => None }).collect()
    }

    #[test]
    fn every_preset_is_a_layout() {
        for p in &PRESETS {
            assert_eq!(refusal(p.columns), None, "{}", p.key);
            assert_eq!(parse(&write(p.columns)).unwrap(), p.columns, "{}", p.key);
            assert!(preset(p.key).is_some_and(|found| found.name == p.name));
        }
        let mut keys: Vec<&str> = PRESETS.iter().map(|p| p.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), PRESETS.len());
    }

    #[test]
    fn a_layout_is_refused_with_a_reason() {
        assert!(parse("paper:4,chat:2").unwrap_err().contains("现在是 6 格"));
        assert!(parse("paper:4,shelf:4").unwrap_err().contains("shelf"));
        assert!(parse("paper:4,chat").unwrap_err().contains("栏:格数"));
        assert!(parse("paper:x,chat:8").unwrap_err().contains("格数"));
        assert!(parse("chat:4,chat:4").unwrap_err().contains("对话"));
        assert!(parse("chat:4,translation:4").unwrap_err().contains("文献"));
        assert!(parse("paper:0,chat:8").unwrap_err().contains("至少占 1 格"));
        assert!(parse("paper:2,paper:2,paper:2,paper:1,chat:1").unwrap_err().contains("最多分 4 栏"));
        assert_eq!(parse(" paper : 3 , translation:3,chat:2").unwrap(), preset("study").unwrap().columns);
    }

    #[test]
    fn a_translation_translates_the_paper_next_to_it() {
        let columns = parse("paper:2,translation:2,translation:2,paper:2").unwrap();
        assert_eq!((source(&columns, 1), source(&columns, 2)), (Some(0), Some(3)));
        assert_eq!(source(&parse("translation:4,paper:4").unwrap(), 0), Some(1));
        assert!(parse("paper:3,chat:2,translation:3").unwrap_err().contains("紧挨着"));
        assert!(parse("paper:4,translation:2,chat:2").unwrap_err().contains("一样宽"));
        assert!(parse("translation:3,paper:2,translation:3").unwrap_err().contains("一样宽"));
        assert!(parse("translation:2,paper:2,translation:2,chat:2").unwrap_err().contains("只配一栏"));
    }

    #[test]
    fn a_wide_window_shows_the_layout_as_written() {
        let study = preset("study").unwrap().columns;
        let placed = arrange(study, 2000., 0.);
        assert_eq!(placed, vec![Placement::Shown { x: 0., width: 750. }, Placement::Shown { x: 750., width: 750. }, Placement::Shown { x: 1500., width: 500. }]);
        // Gaps come out of the room, and the panes fill the rest to the last pixel.
        let four = preset("compare-translate").unwrap().columns;
        let placed = arrange(four, 2563., 1.);
        assert_eq!(widths(&placed), vec![640., 640., 640., 640.]);
        assert_eq!(placed[3], Placement::Shown { x: 1923., width: 640. });
        assert_eq!(needs(four, 1.), 1763.);
    }

    #[test]
    fn a_pane_is_not_squeezed_below_its_least_width() {
        // 提问 at 1000: the paper's share would be 375, so it keeps 440 and the other two share the rest by their units.
        let ask = preset("ask").unwrap().columns;
        assert_eq!(widths(&arrange(ask, 1000., 0.)), vec![224., 336., 440.]);
        assert_eq!(widths(&arrange(ask, 1200., 0.)), vec![300., 450., 450.]);
    }

    #[test]
    fn panes_give_way_in_order() {
        let ask = preset("ask").unwrap().columns;
        // Too narrow for the list: it is put away first.
        assert_eq!(arrange(ask, 800., 0.), vec![Placement::Away, Placement::Shown { x: 0., width: 360. }, Placement::Shown { x: 360., width: 440. }]);
        // Then the conversation floats over the paper.
        assert_eq!(arrange(ask, 500., 0.), vec![Placement::Away, Placement::Over(2), Placement::Shown { x: 0., width: 500. }]);
        // 3,3,2 at 1000: the translation goes into its paper's pane and brings its units, so the pane that holds
        // both keeps their six of eight and the conversation its two (its least width, here).
        let study = preset("study").unwrap().columns;
        assert_eq!(arrange(study, 1000., 0.), vec![Placement::Shown { x: 0., width: 680. }, Placement::Inside(0), Placement::Shown { x: 680., width: 320. }]);
        assert_eq!(arrange(study, 1190., 0.), vec![Placement::Shown { x: 0., width: 870. }, Placement::Inside(0), Placement::Shown { x: 870., width: 320. }]);
        // A lone paper narrower than its least width has the whole width: there is nothing left to give.
        assert_eq!(arrange(preset("read").unwrap().columns, 300., 0.), vec![Placement::Shown { x: 0., width: 300. }]);
    }

    #[test]
    fn translations_give_way_together() {
        // 2,2,2,2 at 1440 with a gap: both translations go under their originals, not one of them.
        let four = preset("compare-translate").unwrap().columns;
        assert_eq!(arrange(four, 1440., 1.), vec![Placement::Shown { x: 0., width: 719. }, Placement::Inside(0), Placement::Shown { x: 720., width: 720. }, Placement::Inside(2)]);
        // Narrower still, the second paper is put away with its translation; the first keeps its own under it.
        assert_eq!(arrange(four, 600., 1.), vec![Placement::Shown { x: 0., width: 600. }, Placement::Inside(0), Placement::Away, Placement::Away]);
        // Of two papers side by side with room for one, the second is put away: neither is halved.
        assert_eq!(arrange(preset("compare").unwrap().columns, 600., 0.), vec![Placement::Shown { x: 0., width: 600. }, Placement::Away]);
        // Of four with room for two, the last two.
        let read = columns(Mode::Read, 4);
        assert_eq!(arrange(&read, 1000., 1.).iter().map(|p| p.shown()).collect::<Vec<_>>(), vec![true, true, false, false]);
    }

    #[test]
    fn the_conversation_floats_over_the_first_paper() {
        let columns = parse("paper:3,paper:3,chat:2").unwrap();
        assert_eq!(widths(&arrange(&columns, 1300., 0.)), vec![487., 487., 326.]);
        // The two papers still fit side by side once the conversation floats.
        assert_eq!(arrange(&columns, 900., 0.), vec![Placement::Shown { x: 0., width: 450. }, Placement::Shown { x: 450., width: 450. }, Placement::Over(0)]);
        assert_eq!(arrange(&columns, 700., 0.), vec![Placement::Shown { x: 0., width: 700. }, Placement::Away, Placement::Over(0)]);
    }

    #[test]
    fn a_mode_repeats_each_papers_columns() {
        // One paper: the preset of the mode's key.
        for mode in Mode::ALL {
            assert_eq!(columns(mode, 1), preset(mode.key()).unwrap().columns, "{}", mode.key());
            assert_eq!(refusal(&columns(mode, 1)), None);
            assert!(!mode.label().is_empty());
            assert_eq!(Mode::from_key(mode.key()), Some(mode));
        }
        assert_eq!((grid(&columns(Mode::Study, 1)).as_deref(), grid(&columns(Mode::Ask, 1)).as_deref()), (Some("3·3·2"), Some("2·3·3")));
        // Each further paper has the columns the first has: its share is not split among them.
        let units = |mode: Mode, papers: usize| columns(mode, papers).iter().map(|c| (c.pane, c.units)).collect::<Vec<_>>();
        assert_eq!(units(Mode::Read, 3), vec![(Pane::Paper, 8); 3]);
        assert_eq!(units(Mode::Translate, 2), vec![(Pane::Paper, 4), (Pane::Translation, 4), (Pane::Paper, 4), (Pane::Translation, 4)]);
        assert_eq!(units(Mode::Ask, 3), vec![(Pane::List, 2), (Pane::Chat, 3), (Pane::Paper, 3), (Pane::Paper, 3), (Pane::Paper, 3)]);
        assert_eq!(units(Mode::Study, 2), vec![(Pane::Paper, 3), (Pane::Translation, 3), (Pane::Paper, 3), (Pane::Translation, 3), (Pane::Chat, 2)]);
        // On the grid of eight only with one paper.
        assert_eq!(grid(&columns(Mode::Read, 2)), None);
        // Each translation next to its paper and as wide.
        for papers in 1..=4 { for mode in Mode::ALL { let c = columns(mode, papers); for i in 0..c.len() { if c[i].pane == Pane::Translation { assert_eq!(c[source(&c, i).unwrap()].units, c[i].units) } } } }
        assert_eq!(columns(Mode::Read, 0), columns(Mode::Read, 1));
    }

    #[test]
    fn several_papers_have_the_window_before_the_list_and_the_conversation() {
        // 提问 with two papers at 1728: the list at the width it is kept at, the conversation a paper's share of the rest.
        let ask = columns(Mode::Ask, 2);
        assert_eq!(frame(&ask, 1728., 1., 280., 0.), Frame { list: 280., chat: 481., papers: 965. });
        // At 1360 the list is what goes, not a paper: both stand side by side, as wide as the conversation.
        assert_eq!(frame(&ask, 1360., 1., 280., 0.), Frame { list: 0., chat: 452., papers: 907. });
        // The list keeps less than it was given before it goes: 200 + 320 + 2 × 440 and the gaps are 1403.
        assert_eq!(frame(&ask, 1420., 1., 280., 0.).list, 217.);
        assert_eq!(frame(&ask, 1403., 1., 280., 0.).list, 200.);
        assert_eq!(frame(&ask, 1402., 1., 280., 0.).list, 0.);
        // A window that shows no list (reading, another page).
        assert_eq!(frame(&ask, 1728., 1., 0., 0.).list, 0.);
        // 精读 with two papers at 1728: the translations stand under their papers, which keep their six units
        // each against the conversation's two (the least it is read at with ease, here), not half the window.
        let study = columns(Mode::Study, 2);
        assert_eq!(frame(&study, 1728., 1., 0., 0.), Frame { list: 0., chat: 360., papers: 1367. });
        assert_eq!(frame(&study, 1210., 1., 0., 0.).chat, 328.);
        assert_eq!(frame(&study, 2560., 1., 0., 0.).chat, 368.);
        // Dragged, the conversation has the width it was given, as far as the papers beside it are still read.
        assert_eq!(frame(&study, 1728., 1., 0., 600.).chat, 600.);
        assert_eq!(frame(&study, 1728., 1., 0., 1200.).chat, 1728. - 2. * 440. - 2.);
        assert_eq!(frame(&study, 1728., 1., 0., 100.).chat, 320.);
        // Three papers at 1360 have no room beside the conversation: it keeps its least width, they share the rest.
        assert_eq!(frame(&columns(Mode::Ask, 3), 1360., 1., 280., 0.), Frame { list: 0., chat: 320., papers: 1039. });
        // No conversation in the layout: the papers have the window.
        assert_eq!(frame(&columns(Mode::Read, 2), 1360., 1., 0., 0.), Frame { list: 0., chat: 0., papers: 1360. });
    }

    #[test]
    fn a_width_that_is_no_number_is_none() {
        let placed = arrange(preset("ask").unwrap().columns, f32::NAN, f32::INFINITY);
        assert_eq!(placed, vec![Placement::Away, Placement::Over(2), Placement::Shown { x: 0., width: 0. }]);
    }
}
