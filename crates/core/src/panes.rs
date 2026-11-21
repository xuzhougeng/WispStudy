//! 分栏 (#194): the reading area as columns on an eight-unit grid, each a pane with one use: a paper,
//! the translation of the paper beside it, the conversation, the list of papers. A layout is what the
//! person wished for (`4,2,2`: a paper on half the width, its translation and the conversation on a
//! quarter each); `arrange` says what a window of a given width shows of it. Plain arithmetic in logical
//! pixels, measured from the area's left edge; the window draws whatever this gives.
//!
//! The frame stays and the content gives way (docs/dev/design.md): a pane is not squeezed below the width
//! it can be read at. Where the window is too narrow for a layout, its panes give way in an order fixed
//! here: the list is put away; the translations are drawn inside their papers (the original over its
//! translation), all of them at once so that they look alike; the conversation floats over a paper; a
//! paper shares the pane of the one before it. A pane holds one thing besides its own: a paper with a
//! second paper inside it has no room left for its translation. Columns only, never rows: each pane is
//! one face from the top of the window to its foot.

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
    /// about, the PDF beside the conversation and the conversation beside it (`library.rs`).
    pub fn min_width(self) -> f32 {
        match self {
            Pane::List => 200.,
            Pane::Paper | Pane::Translation => 360.,
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

/// A mode's columns for `papers` papers (one at least), in their order. Each paper's share stays what it
/// is with one paper, the papers sharing it: 对照译文 is 4·4 with one paper and 2·2·2·2 with two.
pub fn columns(mode: Mode, papers: usize) -> Vec<Column> {
    let papers = papers.max(1) as u32;
    let paper = |units: u32| -> Vec<Column> {
        let mut out = vec![column(Pane::Paper, units)];
        if mode.translates() { out.push(column(Pane::Translation, units)) }
        out
    };
    // The share of all papers and their translations on the grid, the rest the list's and the conversation's.
    let (list, chat, share) = match mode {
        Mode::Read | Mode::Translate => (0, 0, UNITS),
        Mode::Ask => (2, 3, 3),
        Mode::Study => (0, 2, 6),
    };
    // Each paper's units, its translation's alike: the share split among them, on a finer grid where it does not divide.
    let parts = papers * if mode.translates() { 2 } else { 1 };
    let (scale, each) = if share % parts == 0 { (1, share / parts) } else { (parts, share) };
    let mut out = vec![];
    if list > 0 { out.push(column(Pane::List, list * scale)) }
    if mode == Mode::Ask { out.push(column(Pane::Chat, chat * scale)) }
    for _ in 0..papers { out.extend(paper(each)) }
    if mode == Mode::Study { out.push(column(Pane::Chat, chat * scale)) }
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
    /// Inside column `n`'s pane: a translation under its original, a paper in the other half of the one before it.
    Inside(usize),
    /// Floating over column `n`'s pane: the conversation.
    Over(usize),
    /// Not drawn: the list, which comes back when the window is wider; a translation with no room in its paper's pane.
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
    loop {
        let shown: Vec<usize> = (0..columns.len()).filter(|&i| put[i].is_none()).collect();
        let room = width - gap * shown.len().saturating_sub(1) as f32;
        let least: f32 = shown.iter().map(|&i| columns[i].pane.min_width()).sum();
        if least <= room || !give_way(columns, &mut put) { break }
    }
    let shown: Vec<usize> = (0..columns.len()).filter(|&i| put[i].is_none()).collect();
    let room = (width - gap * shown.len().saturating_sub(1) as f32).max(0.);
    let widths = share(&shown.iter().map(|&i| (columns[i].units, columns[i].pane.min_width())).collect::<Vec<_>>(), room);
    let mut x = 0.;
    for (&i, &w) in shown.iter().zip(&widths) {
        put[i] = Some(Placement::Shown { x, width: w });
        x += w + gap;
    }
    put.into_iter().map(|placement| placement.unwrap_or(Placement::Away)).collect()
}

/// One step of giving way, the next in the order the module's comment gives; false when nothing is left to give.
fn give_way(columns: &[Column], put: &mut [Option<Placement>]) -> bool {
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
            put[i] = Some(host.map_or(Placement::Away, Placement::Inside));
        }
        return true;
    }
    let papers = shown(put, Pane::Paper);
    if let Some(&over) = papers.first() && let Some(chat) = shown(put, Pane::Chat).first().copied() {
        put[chat] = Some(Placement::Over(over));
        return true;
    }
    // The rightmost paper goes into the pane of the paper before it, which gives up its translation for it.
    if papers.len() > 1 {
        let (host, guest) = (papers[papers.len() - 2], papers[papers.len() - 1]);
        // (The conversation floats over the first paper, never over the one that goes.)
        for p in put.iter_mut() {
            if matches!(*p, Some(Placement::Inside(n)) if n == host || n == guest) { *p = Some(Placement::Away) }
        }
        put[guest] = Some(Placement::Inside(host));
        return true;
    }
    false
}

/// Whether the pane of column `n` holds another column inside it.
fn holds(put: &[Option<Placement>], n: usize) -> bool {
    put.contains(&Some(Placement::Inside(n)))
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
        assert_eq!(needs(four, 1.), 1443.);
    }

    #[test]
    fn a_pane_is_not_squeezed_below_its_least_width() {
        // 提问 at 900: the paper's share would be 337, so it keeps 360 and the other two share the rest by their units.
        let ask = preset("ask").unwrap().columns;
        assert_eq!(widths(&arrange(ask, 900., 0.)), vec![216., 324., 360.]);
        assert_eq!(widths(&arrange(ask, 1200., 0.)), vec![300., 450., 450.]);
    }

    #[test]
    fn panes_give_way_in_order() {
        let ask = preset("ask").unwrap().columns;
        // Too narrow for the list: it is put away first.
        assert_eq!(arrange(ask, 700., 0.), vec![Placement::Away, Placement::Shown { x: 0., width: 340. }, Placement::Shown { x: 340., width: 360. }]);
        // Then the conversation floats over the paper.
        assert_eq!(arrange(ask, 500., 0.), vec![Placement::Away, Placement::Over(2), Placement::Shown { x: 0., width: 500. }]);
        // 3,3,2 at 1000: the translation goes into its paper's pane, the conversation stays beside it.
        let study = preset("study").unwrap().columns;
        assert_eq!(arrange(study, 1000., 0.), vec![Placement::Shown { x: 0., width: 600. }, Placement::Inside(0), Placement::Shown { x: 600., width: 400. }]);
        // A lone paper narrower than its least width has the whole width: there is nothing left to give.
        assert_eq!(arrange(preset("read").unwrap().columns, 300., 0.), vec![Placement::Shown { x: 0., width: 300. }]);
    }

    #[test]
    fn translations_give_way_together() {
        // 2,2,2,2 at 1440 with a gap: both translations go under their originals, not one of them.
        let four = preset("compare-translate").unwrap().columns;
        assert_eq!(arrange(four, 1440., 1.), vec![Placement::Shown { x: 0., width: 719. }, Placement::Inside(0), Placement::Shown { x: 720., width: 720. }, Placement::Inside(2)]);
        // Narrower still, the second paper shares the first one's pane, which gives up both translations for it.
        assert_eq!(arrange(four, 600., 1.), vec![Placement::Shown { x: 0., width: 600. }, Placement::Away, Placement::Inside(0), Placement::Away]);
        // Two papers side by side become two halves of one pane.
        assert_eq!(arrange(preset("compare").unwrap().columns, 600., 0.), vec![Placement::Shown { x: 0., width: 600. }, Placement::Inside(0)]);
    }

    #[test]
    fn the_conversation_floats_over_the_first_paper() {
        let columns = parse("paper:3,paper:3,chat:2").unwrap();
        assert_eq!(widths(&arrange(&columns, 1100., 0.)), vec![390., 390., 320.]);
        // The two papers still fit side by side once the conversation floats.
        assert_eq!(arrange(&columns, 750., 0.), vec![Placement::Shown { x: 0., width: 375. }, Placement::Shown { x: 375., width: 375. }, Placement::Over(0)]);
        assert_eq!(arrange(&columns, 700., 0.), vec![Placement::Shown { x: 0., width: 700. }, Placement::Inside(0), Placement::Over(0)]);
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
        assert_eq!(grid(&columns(Mode::Translate, 2)).as_deref(), Some("2·2·2·2"));
        assert_eq!(grid(&columns(Mode::Read, 2)).as_deref(), Some("4·4"));
        assert_eq!(grid(&columns(Mode::Read, 4)).as_deref(), Some("2·2·2·2"));
        assert_eq!(grid(&columns(Mode::Study, 1)).as_deref(), Some("3·3·2"));
        assert_eq!(grid(&columns(Mode::Ask, 1)).as_deref(), Some("2·3·3"));
        // Three papers of 提问 share its 3 of 8; two do not divide them, and a finer grid keeps the shares.
        assert_eq!(grid(&columns(Mode::Ask, 3)).as_deref(), Some("2·3·1·1·1"));
        let ask = columns(Mode::Ask, 2);
        assert_eq!(ask.iter().map(|c| c.units).collect::<Vec<_>>(), vec![4, 6, 3, 3]);
        assert_eq!(grid(&ask), None);
        let study = columns(Mode::Study, 2);
        assert_eq!(study.iter().map(|c| (c.pane, c.units)).collect::<Vec<_>>(), vec![(Pane::Paper, 6), (Pane::Translation, 6), (Pane::Paper, 6), (Pane::Translation, 6), (Pane::Chat, 8)]);
        // Each translation next to its paper and as wide.
        for papers in 1..=4 { for mode in Mode::ALL { let c = columns(mode, papers); for i in 0..c.len() { if c[i].pane == Pane::Translation { assert_eq!(c[source(&c, i).unwrap()].units, c[i].units) } } } }
        assert_eq!(columns(Mode::Read, 0), columns(Mode::Read, 1));
    }

    #[test]
    fn a_width_that_is_no_number_is_none() {
        let placed = arrange(preset("ask").unwrap().columns, f32::NAN, f32::INFINITY);
        assert_eq!(placed, vec![Placement::Away, Placement::Over(2), Placement::Shown { x: 0., width: 0. }]);
    }
}
