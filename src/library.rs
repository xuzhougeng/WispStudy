//! An open library: the sidebar (collect, views, tags), the paper table with
//! its search and filters, and the trash.

use crate::i18n::{label, t};
use crate::app::{Cmd, ColumnDrag, Edge, FilterKey, Library, MenuKind, Page, Tab, Translated, Workspace};
use crate::settings::SettingsPage;
use crate::theme::{self, Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::ops::Range;
use wisp_core::filter::{self, View};
use wisp_core::model::*;
use wisp_core::time;

const SIDEBAR_W: f32 = 224.;
/// How narrow and how wide the sidebar is dragged. Pulled in past half its narrowest, it is put away.
const SIDEBAR_MIN: f32 = 176.;
const SIDEBAR_MAX: f32 = 420.;
pub const DETAIL_W: f32 = 448.;
/// The narrowest the detail panel is dragged to, and what the list beside it keeps.
const DETAIL_MIN: f32 = 320.;
const MAIN_MIN: f32 = 260.;

/// The narrowest a paper's conversation is dragged to beside its PDF, and what the PDF keeps.
const SPLIT_CHAT_MIN: f32 = 320.;
const SPLIT_PDF_MIN: f32 = 360.;

/// How wide the conversation is in a panel `width` wide, when the reader asked for `own` (0 or less: no wish).
fn split_chat_width(own: f32, width: f32) -> f32 {
    let wished = if own > 0. { own } else { (width * 0.45).clamp(360., 520.) };
    wished.min(width - SPLIT_PDF_MIN).max(SPLIT_CHAT_MIN)
}

/// How far the conversation reaches with the pointer at `x` on the edge between it and the PDF, in a panel
/// from `left` to `right`: from the panel's left edge to the pointer, or from the pointer to its right edge
/// when the PDF is first. The same limits (`split_chat_width`) hold in both orders.
fn split_chat_reach(x: f32, left: f32, right: f32, pdf_first: bool) -> f32 {
    if pdf_first { right - x } else { x - left }
}

/// The list of papers beside a paper that is asked about: the width it comes with, and how narrow and how wide it is dragged.
const ASK_LIST_W: f32 = 280.;
const ASK_LIST_MIN: f32 = 200.;
const ASK_LIST_MAX: f32 = 420.;

/// How wide that list is in a window `viewport` wide, when the reader asked for `own` (0 or less: no wish).
/// The conversation and the PDF keep `detail::SPLIT_MIN` between them; a window without room for the list has none.
fn ask_list_width(own: f32, viewport: f32) -> f32 {
    let room = viewport - crate::detail::SPLIT_MIN;
    if room < ASK_LIST_MIN { return 0. }
    (if own > 0. { own } else { ASK_LIST_W }).clamp(ASK_LIST_MIN, ASK_LIST_MAX).min(room)
}

fn view_label(view: View) -> &'static str {
    VIEWS.iter().find(|(_, _, _, other)| *other == view).map_or(t("文献"), |(_, _, label, _)| t(label))
}

/// The views of a library, as the sidebar lists them: element id, icon, name. Each is the same list, narrowed.
/// What waits for a decision is not one of them: that is 筛选, a row of the group below, and the 收录 filter.
pub(crate) const VIEWS: [(&str, &str, &str, View); 5] = [
    ("view-all", "library", "全部文献", View::All),
    ("view-recent", "history", "最近添加", View::Recent),
    ("view-unread", "eye", "未读", View::Unread),
    ("view-starred", "star", "收藏", View::Starred),
    ("view-nopdf", "file-text", "缺 PDF", View::NoPdf),
];

/// The columns of the paper table a right click on their names can put away, by the key they are kept under
/// (`Settings::columns_hidden`, `Settings::column_widths`) and in the order they stand in. A custom field's
/// column is `field:<id>`, on show while the field is `in_table`.
pub const COLUMNS: [(&str, &str); 7] = [("journal", "期刊"), ("impact", "影响因子"), ("zone", "分区"), ("year", "年份"), ("fulltext", "全文"), ("reading", "阅读"), ("screening", "收录")];
/// How narrow and how wide a column can be dragged, and what the title keeps.
const COLUMN_MIN: f32 = 44.;
const COLUMN_MAX: f32 = 640.;
const TITLE_MIN: f32 = 300.;

/// A column of the paper table: fixed width, or the flexible title column.
struct Column {
    label: String,
    width: Option<f32>,
    /// The key its width is kept under.
    key: String,
    /// Narrower than it comes: what its cells show may be cut.
    narrowed: bool,
    sort: Option<&'static str>,
    /// What the cells show.
    kind: ColumnKind,
}

#[derive(Clone)]
enum ColumnKind {
    Star,
    Title,
    Journal,
    /// The journal's impact factor and its zone in the partition `Settings::journal_zone` names (`journals`).
    Impact,
    Zone,
    Year,
    Field(String),
    Fulltext,
    Reading,
    Screening,
}

impl ColumnKind {
    /// A short value or a chip: its name and its cells stand centred, so the two line up whatever the chip's padding.
    fn centred(&self) -> bool {
        matches!(self, ColumnKind::Impact | ColumnKind::Zone | ColumnKind::Year | ColumnKind::Fulltext | ColumnKind::Reading | ColumnKind::Screening)
    }

    fn key(&self) -> String {
        match self {
            ColumnKind::Star => "star".into(),
            ColumnKind::Title => "title".into(),
            ColumnKind::Journal => "journal".into(),
            ColumnKind::Impact => "impact".into(),
            ColumnKind::Zone => "zone".into(),
            ColumnKind::Year => "year".into(),
            ColumnKind::Field(id) => format!("field:{id}"),
            ColumnKind::Fulltext => "fulltext".into(),
            ColumnKind::Reading => "reading".into(),
            ColumnKind::Screening => "screening".into(),
        }
    }
}

/// The widths of the two columns at an edge that was taken at `drag.x` and is now at `x`: what one gains the
/// other gives, the title (no width of its own) down to what it can spare, a column between `COLUMN_MIN` and `COLUMN_MAX`.
fn dragged(drag: &ColumnDrag, x: f32) -> (Option<f32>, f32) {
    let right = drag.right.1;
    let (give, take) = match &drag.left {
        Some((_, left)) => ((left - COLUMN_MIN).max(0.), (COLUMN_MAX - left).max(0.)),
        None => (drag.spare.max(0.), f32::MAX),
    };
    let by = (x - drag.x).clamp(-give.min((COLUMN_MAX - right).max(0.)), take.min((right - COLUMN_MIN).max(0.)));
    (drag.left.as_ref().map(|(_, left)| left + by), right - by)
}

/// The sort of the 分区 column: by the partition it shows.
pub(crate) fn zone_sort(settings: &wisp_core::settings::Settings) -> &'static str {
    match settings.journal_zone.as_str() {
        "xr" => "xr",
        "jcr" => "jcr",
        _ => "cas",
    }
}

/// The width a sortable column has by itself: `own`, or what its name in the header needs with the arrow that
/// marks the sort beside it, when that is more. The name is measured roughly (a Chinese character is as wide
/// as the 12px type is high, a Latin letter a little over half of that, at `scale` of that size), with the
/// cell's padding (8 each side), the gap and the arrow (3 + 12) and a little to spare. Without this the arrow
/// took the name's room: sorted by 影响因子 the header read 「影响...」.
pub(crate) fn sortable_width(label: &str, own: f32, scale: f32) -> f32 {
    let name: f32 = label.chars().map(|c| if c.is_ascii() { 6.5 } else { 12. }).sum::<f32>() * scale;
    own.max((name + 16. + 15. + 4.).ceil())
}

/// A sort as the list is to be in: one by a partition (`cas`, `xr`, `jcr`) is by the partition the 分区 column
/// shows now, in the same direction, whichever it named when it was chosen or kept with the library; any
/// other sort as it is. The column's header marks the sort by its own key, so the two never disagree.
pub(crate) fn sort_in_zone(sort: &str, settings: &wisp_core::settings::Settings) -> String {
    let (key, desc) = filter::sort_parts(sort);
    if ["cas", "xr", "jcr"].contains(&key) { format!("{}-{}", zone_sort(settings), if desc { "desc" } else { "asc" }) } else { sort.to_string() }
}

/// 近 N 年 as the filter by year offers it, and the impact factors a journal is asked to reach.
pub(crate) const YEARS_BACK: [i32; 3] = [3, 5, 10];
pub(crate) const IMPACTS: [f64; 4] = [3., 5., 10., 20.];

/// A span as `Filters::year` and `Filters::impact` keep it, in its two ends; a value alone is both.
fn ends(value: &str) -> (&str, &str) {
    value.split_once('-').unwrap_or((value, value))
}

/// A filter by year as its chip and its menu say it: `2024`, 近 3 年, `2020–2026`, 2020 年起, 2010 年及以前.
pub(crate) fn year_label(value: &str) -> String {
    if let Some(count) = value.strip_prefix("last:") { return tf!("近 {count} 年", "Last {count} years") }
    match ends(value) {
        ("", "") => String::new(),
        (low, "") => tf!("{low} 年起", "Since {low}"),
        ("", high) => tf!("{high} 年及以前", "Up to {high}"),
        (low, high) if low == high => low.to_string(),
        (low, high) => format!("{low}–{high}"),
    }
}

/// A filter by impact factor as its chip and its menu say it: `≥ 5`, `≤ 3`, `5–10`.
pub(crate) fn impact_label(value: &str) -> String {
    match ends(value) {
        ("", "") => String::new(),
        (low, "") => format!("≥ {low}"),
        ("", high) => format!("≤ {high}"),
        (low, high) if low == high => low.to_string(),
        (low, high) => format!("{low}–{high}"),
    }
}

/// A filter by zone as its chip and its menu say it: `中科院 1区`, `中科院 1–2区`, `JCR Q1–Q2`. The partition is
/// named each time: the same words under another one are other papers.
pub(crate) fn zone_label(value: &str) -> String {
    let Some((which, last)) = filter::zone_parts(value) else { return value.to_string() };
    let zones = match (which, last) {
        ("jcr", 1) => "Q1".to_string(),
        ("jcr", last) => format!("Q1–Q{last}"),
        (_, 1) => tf!("{}区", "Zone {}", 1),
        (_, last) => tf!("1–{last}区", "Zones 1–{last}"),
    };
    format!("{} {zones}", label(&wisp_core::journals::ZONES, which))
}

/// What 自定义区间… sets a filter to from its two boxes, either left empty (both: the filter is cleared), or why not.
/// The ends typed the wrong way round are taken the right way.
pub(crate) fn range_value(key: &FilterKey, low: &str, high: &str) -> Result<String, String> {
    fn ordered<T: PartialOrd + std::fmt::Display>(low: Option<T>, high: Option<T>) -> String {
        match (low, high) {
            (Some(low), Some(high)) if low > high => filter::span_value(Some(high), Some(low)),
            (low, high) => filter::span_value(low, high),
        }
    }
    let end = |text: &str, read: &dyn Fn(&str) -> bool| text.is_empty() || read(text);
    if *key == FilterKey::Impact {
        let fine = |text: &str| text.parse::<f64>().is_ok_and(|n| n.is_finite() && n >= 0.);
        if !end(low, &fine) || !end(high, &fine) { return Err(t("影响因子应为不小于 0 的数字，例如 5 或 9.5。").into()) }
        Ok(ordered(low.parse::<f64>().ok(), high.parse::<f64>().ok()))
    } else {
        let fine = |text: &str| text.parse::<i32>().is_ok_and(|year| (1000..=9999).contains(&year));
        if !end(low, &fine) || !end(high, &fine) { return Err(t("年份应为四位数字，例如 2020。").into()) }
        Ok(ordered(low.parse::<i32>().ok(), high.parse::<i32>().ok()))
    }
}

/// A journal's zone in one partition as the window writes it: `1区 Top`, `Zone 1 Top`, `Q2`.
pub(crate) fn zone_text(j: &wisp_core::journals::Journal, which: &str) -> Option<String> {
    let zone = |z: Option<u8>, top: bool| z.map(|z| format!("{}{}", tf!("{}区", "Zone {}", z), if top { " Top" } else { "" }));
    match which {
        "jcr" => Some(j.quartile.clone()).filter(|q| !q.is_empty()),
        "xr" => zone(j.xr, j.xr_top),
        _ => zone(j.cas, j.cas_top),
    }
}

/// Every metric of a journal in one line: `IF 18.1 · JCR Q1 · 中科院 1区 Top · 新锐 1区 Top`.
pub(crate) fn journal_line(j: &wisp_core::journals::Journal) -> String {
    let mut parts = vec![];
    if let Some(impact) = j.impact_text() { parts.push(format!("IF {impact}")) }
    if !j.quartile.is_empty() { parts.push(format!("JCR {}", j.quartile)) }
    for (which, name) in [("cas", t("中科院")), ("xr", t("新锐"))] {
        if let Some(zone) = zone_text(j, which) { parts.push(format!("{name} {zone}")) }
    }
    parts.join(" · ")
}

/// What a row of the paper table says in full, over a row that had to cut it.
struct PaperTip {
    title: String,
    meta: String,
    tags: Vec<String>,
}

impl Render for PaperTip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            // Blocks, not a flex column: a title that wraps is given its second line's height.
            .w(px(440.))
            .px(px(10.))
            .py(px(7.))
            .rounded(px(6.))
            .border_1()
            .border_color(Theme::line())
            .bg(Theme::bg_body())
            .text_size(text_px(12.))
            .line_height(text_px(18.))
            .text_color(Theme::text_secondary())
            .child(div().text_size(text_px(13.)).line_height(text_px(19.)).text_color(Theme::text()).child(self.title.clone()))
            .when(!self.meta.is_empty(), |d| d.child(div().mt(px(3.)).child(self.meta.clone())))
            .when(!self.tags.is_empty(), |d| d.child(div().mt(px(3.)).flex().flex_wrap().gap_x(px(6.)).text_color(Theme::accent()).children(self.tags.iter().map(|tag| format!("#{tag}")))))
    }
}

impl Workspace {
    /// The sidebar's width for this window: none while it is put away, else
    /// the width the reader dragged it to, or the one it comes with.
    pub fn sidebar_width(&self, window: &Window) -> f32 {
        // A conversation not yet made a library has no navigation: the window is the conversation (`draft.rs`).
        if self.settings.sidebar_hidden || self.in_draft() { return 0. }
        let own = if self.settings.sidebar_width > 0. { self.settings.sidebar_width } else { SIDEBAR_W };
        own.min(Self::sidebar_max(window)).max(SIDEBAR_MIN)
    }

    fn sidebar_max(window: &Window) -> f32 {
        SIDEBAR_MAX.min(f32::from(window.viewport_size().width) * 0.4)
    }

    /// The sidebar's edge is being dragged: the sidebar reaches from the
    /// window's left edge to the pointer, and is put away once the pointer
    /// is well inside its narrowest width (back out, and it is there again).
    pub fn drag_sidebar(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.resizing != Some(Edge::Sidebar) { return }
        if event.pressed_button != Some(MouseButton::Left) { return self.end_drag(cx) }
        let x = f32::from(event.position.x);
        let hidden = x < SIDEBAR_MIN / 2.;
        let width = x.min(Self::sidebar_max(window)).max(SIDEBAR_MIN);
        if hidden == self.settings.sidebar_hidden && (hidden || (width - self.settings.sidebar_width).abs() < 0.5) { return }
        self.settings.sidebar_hidden = hidden;
        if !hidden { self.settings.sidebar_width = width }
        cx.notify();
    }

    /// A paper is asked about with its PDF wanted beside the conversation:
    /// the window is laid out for that. The sidebar is away, the papers are
    /// a narrow list at the left (`ask_list_width`), and the conversation
    /// and the PDF have the rest. Nothing is kept for it: it follows from
    /// the tab and `Library::split`, so 信息, the PDF put away, Esc
    /// (`Cmd::LeaveAsk`) and closing the paper all end it.
    pub fn asking(&self, window: &Window) -> bool {
        let Some(lib) = &self.lib else { return false };
        // A conversation not yet made a library keeps its conversation in view: a paper's own stays in its panel.
        !lib.info.draft && lib.selected.is_some() && lib.tab == Tab::Ask && lib.split && !lib.reading && !matches!(lib.page, Page::Trash | Page::Sources | Page::Guide) && f32::from(window.viewport_size().width) >= crate::detail::SPLIT_MIN
    }

    /// The width of the list of papers beside a paper that is asked about: none on a page whose
    /// list is not the papers', nor in a window too narrow for three columns (the paper then has it all).
    pub fn ask_list_width(&self, window: &Window) -> f32 {
        if !self.asking(window) || self.lib.as_ref().is_none_or(|lib| lib.page != Page::Papers) { return 0. }
        // With papers kept beside, the list is the first to give them its room.
        if let Some(frame) = self.pane_frame(window) { return frame.list }
        ask_list_width(self.settings.ask_list_width, f32::from(window.viewport_size().width))
    }

    /// With papers kept beside the one on show and the paper's conversation with them (#194): what the
    /// window gives the list and the conversation (`panes::frame`). The papers are what such a window is
    /// for: the list has the width the reader keeps it at only while each paper beside it can still be read,
    /// and is put away before a paper goes under another; the conversation has a paper's share of the
    /// rest, or the width it was dragged to here (`Library::pane_chat`). None with one paper on show, whose
    /// widths are the reader's habit (`Settings::chat_width`, `ask_list_width`).
    pub fn pane_frame(&self, window: &Window) -> Option<wisp_core::panes::Frame> {
        use wisp_core::panes::{self, Mode};
        let lib = self.lib.as_ref().filter(|lib| !lib.pins.is_empty())?;
        let mode = lib.layout_mode().filter(|mode| matches!(mode, Mode::Ask | Mode::Study))?;
        // A conversation that floats over the papers takes no width of theirs.
        let mode = if self.settings.floating.on { if mode.translates() { Mode::Translate } else { Mode::Read } } else { mode };
        let asking = self.asking(window);
        let width = if asking { f32::from(window.viewport_size().width) } else { self.detail_width(window) };
        let list = if !asking || lib.page != Page::Papers { 0. } else if self.settings.ask_list_width > 0. { self.settings.ask_list_width.clamp(ASK_LIST_MIN, ASK_LIST_MAX) } else { ASK_LIST_W };
        let mut columns = panes::columns(mode, 1 + lib.pins.len());
        // A translation that stands on the pages themselves takes no column.
        if self.settings.facing_in_place { columns.retain(|c| c.pane != panes::Pane::Translation) }
        Some(panes::frame(&columns, width, 1., list, lib.pane_chat))
    }

    /// How tall a paper's row is: in the table, or in the narrow list beside a paper that is asked
    /// about, where a title has two lines. 设置 › 外观: the compact rows still fit the title and its line of details.
    pub fn row_height(&self, narrow: bool) -> f32 {
        f32::from(text_px(match (narrow, self.settings.density.as_str()) {
            (_, "compact") => 44.,
            (false, "comfortable") => 72.,
            (true, "comfortable") => 84.,
            (false, _) => 56.,
            (true, _) => 68.,
        }))
    }

    /// The detail panel's width for this window: the width the reader
    /// dragged it to, or the one it comes with; most of the window while it
    /// is expanded. The list beside it keeps `MAIN_MIN` where the window allows.
    pub fn detail_width(&self, window: &Window) -> f32 {
        let Some(lib) = &self.lib else { return 0. };
        if lib.selected.is_none() || matches!(lib.page, Page::Trash | Page::Sources | Page::Guide) { return 0. }
        let viewport = f32::from(window.viewport_size().width);
        // Reading mode: the paper has the window.
        if lib.reading { return viewport }
        // Asked about: what the narrow list leaves.
        if self.asking(window) { return viewport - self.ask_list_width(window) }
        let own = if self.settings.detail_width > 0. { self.settings.detail_width } else { DETAIL_W };
        let sidebar = self.sidebar_width(window);
        let main_min = if lib.page == Page::Papers { MAIN_MIN } else { 460. };
        if lib.wide { (viewport * 0.62).max(DETAIL_W).min(viewport - sidebar - 80.) } else { own.min(viewport - sidebar - main_min).max(DETAIL_MIN) }
    }

    /// The edge between the list and the detail panel is being dragged:
    /// the panel reaches from the pointer to the window's right edge.
    pub fn drag_detail(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.resizing != Some(Edge::Detail) { return }
        // Let go outside the window: the button is up by the time the pointer is back.
        if event.pressed_button != Some(MouseButton::Left) { return self.end_drag(cx) }
        let viewport = f32::from(window.viewport_size().width);
        // Beside a paper that is asked about, the edge is the narrow list's: it reaches from the window's left to the pointer.
        if self.asking(window) {
            let width = ask_list_width(f32::from(event.position.x).max(ASK_LIST_MIN), viewport);
            if (width - self.settings.ask_list_width).abs() < 0.5 { return }
            self.settings.ask_list_width = width;
            return cx.notify();
        }
        let width = (viewport - f32::from(event.position.x)).min(viewport - self.sidebar_width(window) - MAIN_MIN).max(DETAIL_MIN);
        if (width - self.settings.detail_width).abs() < 0.5 { return }
        self.settings.detail_width = width;
        if let Some(lib) = self.lib.as_mut() { lib.wide = false }
        cx.notify();
    }

    /// The conversation's width beside the PDF in a detail panel `width` wide: the one the reader
    /// dragged it to, or a good share of the panel; the PDF keeps `SPLIT_PDF_MIN`.
    /// With papers kept beside, what they leave it (`pane_frame`).
    pub fn chat_width(&self, width: f32, window: &Window) -> f32 {
        if let Some(frame) = self.pane_frame(window) { return frame.chat.min(width) }
        split_chat_width(self.settings.chat_width, width)
    }

    /// The edge between a paper's conversation and its PDF is being dragged:
    /// the conversation reaches from the panel's left edge to the pointer, or,
    /// with the PDF at its left (`Settings::pdf_first`), from the pointer to the panel's right edge.
    pub fn drag_split(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.resizing != Some(Edge::Split) { return }
        if event.pressed_button != Some(MouseButton::Left) { return self.end_drag(cx) }
        let panel = self.detail_width(window);
        let right = f32::from(window.viewport_size().width);
        // With papers kept beside, the width is this arrangement's, not the reader's habit for one paper.
        if self.pane_frame(window).is_some() {
            let reach = split_chat_reach(f32::from(event.position.x), right - panel, right, self.settings.pdf_first).max(1.);
            if let Some(lib) = self.lib.as_mut() { lib.pane_chat = reach }
            let kept = self.pane_frame(window).map_or(0., |frame| frame.chat);
            if let Some(lib) = self.lib.as_mut() { lib.pane_chat = kept }
            return cx.notify();
        }
        let width = split_chat_width(split_chat_reach(f32::from(event.position.x), right - panel, right, self.settings.pdf_first), panel);
        if (width - self.settings.chat_width).abs() < 0.5 { return }
        self.settings.chat_width = width;
        cx.notify();
    }

    /// The edge was let go: the width is kept for the next time.
    pub fn end_drag(&mut self, cx: &mut Context<Self>) {
        self.column_drag = None;
        if self.resizing.take().is_none() { return }
        self.save_settings(cx);
        cx.notify();
    }

    /// The band between the halves of a split PDF is being dragged (分屏 自由): it follows the pointer, each half keeping some room.
    pub fn drag_halves(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.resizing != Some(Edge::Halves) { return }
        if event.pressed_button != Some(MouseButton::Left) { return self.end_drag(cx) }
        let Some(lib) = self.lib.as_mut() else { return self.end_drag(cx) };
        let Some((from, share, room)) = lib.halves_drag else { return self.end_drag(cx) };
        let stacked = lib.beside.as_ref().is_some_and(|beside| beside.split == crate::pdf::Split::Stacked);
        let along = if stacked { f32::from(event.position.y) } else { f32::from(event.position.x) };
        let share = crate::pdf::dragged_share(share, along - from, room);
        if (share - lib.split_share).abs() * room < 0.5 { return }
        lib.split_share = share;
        cx.notify();
    }

    /// The edge between two columns of the paper table is being dragged: it follows the pointer.
    pub fn drag_column(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.resizing != Some(Edge::Column) { return }
        if event.pressed_button != Some(MouseButton::Left) { return self.end_drag(cx) }
        let Some(drag) = &self.column_drag else { return self.end_drag(cx) };
        let (left, right) = dragged(drag, f32::from(event.position.x));
        let widths = &mut self.settings.column_widths;
        if let (Some((key, _)), Some(left)) = (&drag.left, left) { widths.insert(key.clone(), left); }
        widths.insert(drag.right.0.clone(), right);
        cx.notify();
    }

    pub fn render_library(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let detail_width = self.detail_width(window);
        // Reading mode: nothing but the paper.
        if detail_width > 0. && self.lib.as_ref().is_some_and(|l| l.reading) { return div().size_full().child(self.render_detail(detail_width, window, cx)).into_any_element() }
        // Into or out of the layout for asking, the rows change their height: the list finds its paper again.
        let asking = self.asking(window);
        if let Some(lib) = self.lib.as_mut().filter(|lib| lib.ask_drawn != asking) {
            lib.ask_drawn = asking;
            if let Some(at) = lib.selected.as_ref().and_then(|id| lib.shown.iter().position(|&i| lib.papers[i].id == *id)) { lib.table_scroll.scroll_to_item(at, ScrollStrategy::Center) }
        }
        // A paper asked about: the papers as a narrow list, then the conversation and the PDF.
        if asking {
            let list_width = self.ask_list_width(window);
            let list = (list_width > 0.).then(|| self.render_ask_list(list_width, window, cx));
            return div().flex().size_full().children(list).child(self.render_detail(detail_width, window, cx)).into_any_element();
        }
        let sidebar_width = self.sidebar_width(window);
        let main_width = f32::from(window.viewport_size().width) - sidebar_width - detail_width;
        let page = self.lib.as_ref().map_or(Page::Papers, |l| l.page);
        // What the manuscript cites is read by the outline and the page alike: worked out once, before either is drawn.
        if page == Page::Write || self.in_draft() { self.cite_refresh(cx) }
        let main = match page {
            _ if self.in_draft() => self.render_draft(main_width, window, cx),
            Page::Papers => self.render_papers(main_width, window, cx),
            Page::Ask => self.render_chat(crate::app::Talk::Library, window, cx),
            Page::Trash => self.render_trash(cx),
            Page::Sources => self.render_searches(cx),
            Page::Guide => self.render_guide(window, cx),
            Page::Screen => self.render_screen(main_width, window, cx),
            Page::Write => self.render_write(main_width, window, cx),
            Page::Figures => self.render_figures(main_width, window, cx),
            Page::Extract => self.render_extract(cx),
        };
        let shown = sidebar_width > 0.;
        // The sidebar's right edge: drag it for a wider or narrower sidebar, click it twice to put the sidebar away.
        let dragging = self.resizing == Some(Edge::Sidebar);
        let edge = div()
            .id("sidebar-edge")
            .group("sidebar-edge")
            .absolute()
            .top_0()
            .left(px(-4.))
            .w(px(9.))
            .h_full()
            .flex()
            .justify_center()
            .cursor_col_resize()
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if event.click_count >= 2 {
                    this.resizing = None;
                    this.settings.sidebar_hidden = true;
                    this.save_settings(cx);
                } else {
                    this.resizing = Some(Edge::Sidebar);
                }
                cx.notify();
            }))
            // A tap is over before the layer that follows the pointer is drawn: the edge hears the end of it itself.
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
            .child(div().w(px(2.)).h_full().when(dragging, |d| d.bg(Theme::accent())).when(!dragging, |d| d.group_hover("sidebar-edge", |s| s.bg(Theme::accent().opacity(0.5)))));
        // With the sidebar put away its two top buttons sit over the list, where they were.
        let strip = if shown { ui::title_strip() } else { self.render_corner(true, false, cx).mt(px(Self::corner_top())).pl(px(10. + ui::traffic_lights_width())) };
        div()
            .flex()
            .size_full()
            .when(shown, |d| d.child(self.render_sidebar(sidebar_width, cx)))
            .child(div().relative().flex().flex_col().flex_1().min_w_0().h_full().bg(Theme::bg_body()).when(shown, |d| d.border_l_1().border_color(Theme::line())).child(strip).children(self.render_batch_translation(cx)).child(div().flex().flex_col().flex_1().min_h_0().child(main)).when(shown || dragging, |d| d.child(edge)))
            .when(detail_width > 0., |d| d.child(self.render_detail(detail_width, window, cx)))
            .into_any_element()
    }

    /// What is left free above the top-left buttons. Where the window draws its own title bar there is
    /// nothing: the buttons sit in a strip of its height, level with macOS's traffic lights beside them.
    pub(crate) fn corner_top() -> f32 {
        if ui::custom_titlebar() { 0. } else { 10. }
    }

    /// The window's top-left corner in a library: the button that puts the
    /// sidebar away or brings it back, and the way back, which goes up one
    /// level and no further: from a manuscript to its library, from the library
    /// to all of them, and from a paper asked about (`asking`) to the library around it.
    /// Leaving a manuscript changes the page too, not only the sidebar (`Cmd::LeaveWrite`).
    /// It is the only way back in the sidebar, so the topmost
    /// arrow is never the longer jump. A narrow sidebar keeps the arrow and drops its words.
    pub(crate) fn render_corner(&self, roomy: bool, asking: bool, cx: &mut Context<Self>) -> Div {
        // A conversation not yet made a library is one level under the home page: no sidebar to bring back, one way back.
        let draft = self.in_draft();
        let (id, label, cmd) = if draft { ("back-home", t("首页"), Cmd::GoHome) } else if self.outline_shown() { ("back-library", t("文献库"), Cmd::LeaveWrite) } else if asking { ("back-library", t("文献库"), Cmd::LeaveAsk) } else { ("back-home", t("所有文献库"), Cmd::GoHome) };
        ui::title_strip()
            .when(!ui::custom_titlebar(), |d| d.h(text_px(28.)))
            .pl(px(ui::traffic_lights_width()))
            .when(!draft, |d| d.child(ui::ghost_button("toggle-sidebar", Some("panel-left"), None).occlude().on_click(self.on(Cmd::ToggleSidebar, cx))))
            // The arrow one level up takes this one's place under the pointer: the second click of a double click is not a second step back.
            .child(ui::ghost_button(id, Some("arrow-left"), roomy.then(|| label.into())).occlude().on_click(cx.listener(move |this, event: &ClickEvent, window, cx| if event.click_count() <= 1 { this.run(cmd.clone(), window, cx) })))
    }

    /// Each background job stays compact; its dialog owns the live details.
    fn render_import(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let run = self.lib.as_ref()?.import.as_ref()?;
        Some(ui::progress_entry("import-progress", run.title(), run.rows.len(), run.total, run.running(), &ui::import_segments(run))
            .on_click(self.on(Cmd::ShowImport, cx)).into_any_element())
    }

    fn render_fetching(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let run = self.lib.as_ref()?.fetching.as_ref()?;
        Some(ui::progress_entry("fetch-progress", if run.running() { t("查找全文") } else { run.status() }, run.rows.len(), run.total, run.running(), &ui::fetch_segments(run))
            .on_click(self.on(Cmd::ShowFetch, cx)).into_any_element())
    }

    fn render_sidebar(&mut self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        // While writing, the tags and counts are of no use: the manuscript's outline takes their place.
        if self.outline_shown() { return self.render_outline(width, cx) }
        let lib = self.lib.as_ref().unwrap();
        let nav = |id: &'static str, icon: &'static str, label: &'static str, count: Option<usize>, active: bool, cmd: Cmd, this: &Self, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(8.))
                .h(text_px(30.))
                .px(px(8.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(active, |d| d.bg(Theme::accent_soft()).text_color(Theme::accent()).font_weight(FontWeight::MEDIUM))
                .when(!active, |d| d.hover(|s| s.bg(Theme::hover())))
                .on_click(this.on(cmd, cx))
                .child(ui::icon(icon, 16., if active { Theme::accent() } else { Theme::text_secondary() }))
                .child(div().flex_1().min_w_0().truncate().child(label))
                .when_some(count, |d, n| d.child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(n.to_string())))
        };
        let on_papers = lib.page == Page::Papers;
        // The numbers follow the search and the filters set over the list (`Library::counts`): each view says
        // how many papers it would show, each tag how many of those on show hold it.
        let filtered = lib.filters.active() > 0 || !lib.filters.search.trim().is_empty();
        let mut view_rows: Vec<AnyElement> = VIEWS
            .into_iter()
            .map(|(id, icon, label, view)| {
                // 收藏 holds the starred papers and the figure library: both count, and either page is its own.
                // The figures are not narrowed by the list's filters, so under them only the papers count.
                let collected = if view == View::Starred && !filtered { lib.gallery.items.len() } else { 0 };
                let active = (on_papers && lib.filters.view == view) || (view == View::Starred && lib.page == Page::Figures);
                nav(id, icon, t(label), Some(lib.counts.view(view) + collected).filter(|n| *n > 0), active, Cmd::SetView(view), self, cx).into_any_element()
            })
            .collect();
        // 播客: the playlist of the podcasts kept in the library, once there is one; lit while one plays.
        let podcasts = crate::app::podcast_list(lib).len();
        if podcasts > 0 {
            let playing = lib.aloud.as_ref().is_some_and(|r| r.kind == crate::read_aloud::Kind::Podcast);
            view_rows.push(
                div()
                    .id("view-podcasts")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(text_px(30.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(Theme::hover()))
                    .when(playing, |d| d.text_color(Theme::accent()))
                    .on_click(self.on_menu_beside(MenuKind::Podcasts, cx))
                    .child(ui::icon("headphones", 16., if playing { Theme::accent() } else { Theme::text_secondary() }))
                    .child(div().flex_1().min_w_0().truncate().child(t("播客")))
                    .child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(podcasts.to_string()))
                    .into_any_element(),
            );
        }
        let tags = filter::tag_counts(&lib.papers);
        let tag_rows: Vec<AnyElement> = tags
            .iter()
            .take(30)
            .enumerate()
            .map(|(i, (tag, _))| {
                // Listed in the order of the whole library, so the rows stay put while the search is typed.
                // Chosen (all must be held) is lit; excluded is struck through, quiet, and a click lets it go.
                // Its number is then how many papers it hides.
                let count = lib.counts.tag(tag);
                let active = on_papers && lib.filters.chosen(tag);
                let excluded = on_papers && lib.filters.excluded(tag);
                let clicked = tag.clone();
                div()
                    .id(("tag", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(text_px(28.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(active, |d| d.bg(Theme::accent_soft()).text_color(Theme::accent()))
                    .when(!active, |d| d.hover(|s| s.bg(Theme::hover())))
                    .when(excluded, |d| d.text_color(Theme::text_tertiary()))
                    // Alone a click chooses just this tag; ⌘ / Ctrl adds it to those chosen, ⌥ / Alt excludes it.
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        let held = ev.modifiers();
                        this.run(if held.alt { Cmd::ExcludeTag(clicked.clone()) } else { Cmd::ClickTag(clicked.clone(), held.secondary()) }, window, cx)
                    }))
                    .on_mouse_down(MouseButton::Right, self.on_context(MenuKind::Tag(tag.clone()), cx))
                    .child(if excluded { div().w(px(6.)).h(px(1.5)).flex_none().rounded_full().bg(Theme::text_tertiary()) } else { div().size(px(6.)).flex_none().rounded_full().bg(if active { Theme::accent() } else { Theme::text_placeholder() }) })
                    .child(div().flex_1().min_w_0().truncate().when(excluded, |d| d.line_through()).child(tag.clone()))
                    .child(div().text_size(text_px(12.)).text_color(if count == 0 || excluded { Theme::text_placeholder() } else { Theme::text_tertiary() }).child(count.to_string()))
                    .into_any_element()
            })
            .collect();
        let tags_set = on_papers && !(lib.filters.tags.is_empty() && lib.filters.not_tags.is_empty());
        let trash_count = lib.trash.len();
        div()
            .id("library-sidebar")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            .h_full()
            .overflow_y_scroll()
            .px(px(10.))
            .pb(px(10.))
            .pt(px(Self::corner_top()))
            .gap(px(10.))
            .child(self.render_corner(width >= ui::traffic_lights_width() + f32::from(text_px(150.)), false, cx).when(ui::custom_titlebar(), |d| d.mb(px(4.))))
            .child(
                div()
                    .id("library-head")
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(Theme::hover()))
                    .on_click(self.on(Cmd::EditLibrary(lib.info.id.clone()), cx))
                    .child(div().flex().items_center().gap(px(8.)).child(div().size(px(10.)).flex_none().rounded(px(3.)).bg(Theme::accent())).child(div().flex_1().min_w_0().truncate().text_size(text_px(15.)).font_weight(FontWeight::SEMIBOLD).child(lib.info.name.clone())))
            )
            .child(ui::primary_button("add-paper", t("添加文献")).w_full().on_click(self.on_menu(MenuKind::Add, cx)))
            .child(div().flex().flex_col().gap(px(1.)).children(view_rows))
            // The assistant, then a review's path in its order: 检索, 筛选, 提取 (the number is how many proposals wait), 写作.
            .child(div().flex().flex_col().gap(px(1.)).child(nav("page-ask", "sparkle", t("助手"), None, lib.page == Page::Ask, Cmd::SetPage(Page::Ask), self, cx)).child(nav("page-sources", "search", t("检索与导入"), (!lib.searches.is_empty()).then_some(lib.searches.len()), matches!(lib.page, Page::Sources | Page::Guide), Cmd::SetPage(Page::Sources), self, cx)).child(nav("page-screen", "list-todo", t("筛选"), Some(lib.papers.iter().filter(|p| p.screening == "pending").count()).filter(|n| *n > 0), lib.page == Page::Screen, Cmd::SetPage(Page::Screen), self, cx)).child(nav("page-extract", "scan-text", t("提取"), Some(lib.extract_rows().len()).filter(|n| *n > 0), lib.page == Page::Extract, Cmd::SetPage(Page::Extract), self, cx)).child(nav("page-write", "pen-line", t("写作"), None, lib.page == Page::Write, Cmd::SetPage(Page::Write), self, cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(80.))
                    .gap(px(2.))
                    // The caption says on hover what the keys held with a click do; while tags are chosen or excluded, 清除 lets them all go.
                    .child(
                        div()
                            .id("tags-head")
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(8.))
                            .py(px(4.))
                            .tooltip(ui::tip(tf!("{}多选 · {}排除", "{} to add · {} to exclude", crate::context::held_click(false), crate::context::held_click(true))))
                            .child(ui::caption(t("标签")))
                            .when(tags_set, |d| d.child(div().id("tags-clear").text_size(text_px(11.)).text_color(Theme::text_tertiary()).cursor_pointer().hover(|s| s.text_color(Theme::text_secondary())).on_click(self.on(Cmd::SetFilter(FilterKey::Tag, String::new()), cx)).child(t("清除")))),
                    )
                    .child(div().id("tags").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().gap(px(1.)).children(tag_rows).when(tags.is_empty(), |d| d.child(div().px(px(8.)).text_size(text_px(12.)).text_color(Theme::text_placeholder()).child(t("编辑文献时添加标签")))))
            )
            .children(self.render_import(cx))
            .when_some(lib.zotero.as_ref(), |d, run| d.child(
                ui::progress_entry("zotero-progress", if run.running { t("导入 Zotero") } else if !run.error.is_empty() { t("Zotero 导入失败") } else { t("Zotero 导入完成") }, run.rows.len(), run.total, run.running, &ui::zotero_segments(run))
                    .on_click(self.on(Cmd::ShowZotero, cx))
            ))
            .children(self.render_fetching(cx))
            .children(self.render_extract_progress(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .pt(px(6.))
                    .border_t_1()
                    .border_color(Theme::line())
                    // Under the line: what is managed, not where the work is done.
                    .child(nav("manage-fields", "list", t("自定义词条"), Some(lib.fields.len()).filter(|n| *n > 0), false, Cmd::ManageFields, self, cx))
                    .child(nav("page-trash", "bin", t("回收站"), (trash_count > 0).then_some(trash_count), lib.page == Page::Trash, Cmd::SetPage(Page::Trash), self, cx))
                    .child(nav("open-settings", "settings-gear", t("设置"), None, false, Cmd::OpenSettings(SettingsPage::General), self, cx)),
            )
            .into_any_element()
    }

    /// How far down the table's column names start: the search row and the filters over them, as
    /// `render_papers` lays them out (14 + the search box, 10 + a filter chip + 10). The paper beside
    /// the table gives its title this much, so its tabs stand level with the column names.
    pub(crate) fn papers_head() -> Pixels {
        px(14. + 20.) + text_px(34.) + text_px(26.)
    }

    /// The columns of the table in a list `width` wide: the star and the title, then those the reader left on
    /// show, at the widths the reader gave them, as far as the title keeps `TITLE_MIN`.
    fn columns(settings: &wisp_core::settings::Settings, lib: &Library, width: f32) -> Vec<Column> {
        Self::fitted(settings, lib, width).0
    }

    /// Those columns, and the names of the ones the reader left on show that the width has no room for:
    /// the row of names says how many they are, so a column does not go without a word.
    fn fitted(settings: &wisp_core::settings::Settings, lib: &Library, width: f32) -> (Vec<Column>, Vec<String>) {
        let column = |label: &str, own: f32, sort: Option<&'static str>, kind: ColumnKind| {
            let key = kind.key();
            let width = settings.column_widths.get(&key).map_or(own, |w| w.clamp(COLUMN_MIN, COLUMN_MAX));
            Column { label: label.to_string(), width: Some(width), key, narrowed: width < own, sort, kind }
        };
        let mut columns = vec![Column { label: String::new(), width: Some(34.), key: "star".into(), narrowed: false, sort: None, kind: ColumnKind::Star }, Column { label: t("文献").into(), width: None, key: "title".into(), narrowed: false, sort: Some("title"), kind: ColumnKind::Title }];
        // Optional columns join while there is room, in this order.
        let mut room = width - 34. - TITLE_MIN - 32.;
        let english = crate::i18n::is_english();
        let mut optional = vec![column(t("阅读"), if english { 112. } else { 76. }, None, ColumnKind::Reading), column(t("年份"), 60., Some("year"), ColumnKind::Year), column(t("全文"), if english { 144. } else { 84. }, None, ColumnKind::Fulltext)];
        optional.extend(lib.fields.iter().filter(|f| f.in_table).map(|f| column(&f.name, 132., None, ColumnKind::Field(f.id.clone()))));
        optional.push(column(t("期刊"), 150., Some("journal"), ColumnKind::Journal));
        // A column the list can be sorted by has room for its name and the arrow beside it.
        let scale = f32::from(text_px(12.)) / 12.;
        optional.push(column(t("影响因子"), sortable_width(t("影响因子"), 76., scale), Some("impact"), ColumnKind::Impact));
        optional.push(column(t("分区"), sortable_width(t("分区"), 76., scale), Some(zone_sort(settings)), ColumnKind::Zone));
        optional.push(column(t("收录"), if english { 176. } else { 84. }, None, ColumnKind::Screening));
        optional.retain(|c| !settings.columns_hidden.contains(&c.key));
        let mut chosen = vec![];
        let mut left_out = vec![];
        for c in optional {
            let w = c.width.unwrap_or(0.);
            if room >= w {
                room -= w;
                chosen.push(c);
            } else {
                left_out.push(c);
            }
        }
        // Shown in reading order rather than the order they were admitted in.
        let rank = |c: &Column| match c.kind {
            ColumnKind::Journal => 0,
            ColumnKind::Impact => 1,
            ColumnKind::Zone => 2,
            ColumnKind::Year => 3,
            ColumnKind::Field(_) => 4,
            ColumnKind::Fulltext => 5,
            ColumnKind::Reading => 6,
            ColumnKind::Screening => 7,
            _ => 0,
        };
        chosen.sort_by_key(rank);
        left_out.sort_by_key(rank);
        columns.extend(chosen);
        (columns, left_out.into_iter().map(|c| c.label).collect())
    }

    /// What is left for the title's words in a row `width` wide: the row's padding, the other columns and the cell's padding off.
    fn title_room(columns: &[Column], width: f32) -> f32 {
        width - 16. - columns.iter().filter_map(|c| c.width).sum::<f32>() - 16.
    }

    fn render_papers(&mut self, width: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let f = &lib.filters;
        // The filters: each a name, and what it is set to.
        let mut filters: Vec<(ElementId, String, String, FilterKey)> = vec![
            ("f-year".into(), t("年份").into(), year_label(&f.year), FilterKey::Year),
            ("f-impact".into(), t("影响因子").into(), impact_label(&f.impact), FilterKey::Impact),
            ("f-zone".into(), t("分区").into(), zone_label(&f.zone), FilterKey::Zone),
            ("f-reading".into(), t("阅读").into(), label(&READINGS, &f.reading).to_string(), FilterKey::Reading),
            ("f-screening".into(), t("收录").into(), label(&SCREENINGS, &f.screening).to_string(), FilterKey::Screening),
            ("f-fulltext".into(), t("全文").into(), label(&FULLTEXTS, &f.fulltext).to_string(), FilterKey::Fulltext),
            ("f-tag".into(), t("标签").into(), f.tags.iter().cloned().chain(f.not_tags.iter().map(|tag| format!("−{tag}"))).collect::<Vec<_>>().join(", "), FilterKey::Tag),
        ];
        for (i, field) in lib.fields.iter().enumerate() {
            if field.kind == FieldKind::Text { continue }
            filters.push((("f-field", i).into(), field.name.clone(), f.fields.get(&field.id).cloned().unwrap_or_default(), FilterKey::Field(field.id.clone())));
        }
        filters.push(("f-source".into(), t("来源").into(), f.source.clone(), FilterKey::Source));
        filters.push(("f-preprint".into(), t("预印本").into(), match f.preprint.as_str() { "yes" => t("只看").into(), "no" => t("不含").into(), _ => String::new() }, FilterKey::Preprint));
        // A chip says the name alone when unset, "name: value" when set.
        let said = |name: &str, value: &str| if value.is_empty() { name.to_string() } else { format!("{name}：{value}") };
        let chip = |(id, name, value, key): &(ElementId, String, String, FilterKey), this: &Self, cx: &mut Context<Self>| {
            let set = !value.is_empty();
            div()
                .id(id.clone())
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .h(text_px(26.))
                .px(px(9.))
                .rounded(px(13.))
                .border_1()
                .cursor_pointer()
                .text_size(text_px(12.))
                .when(set, |d| d.bg(Theme::accent_soft()).border_color(Theme::accent_soft()).text_color(Theme::accent()))
                .when(!set, |d| d.border_color(Theme::line_strong()).text_color(Theme::text_secondary()).hover(|s| s.bg(Theme::hover())))
                .on_click(this.on_menu(MenuKind::Filter(key.clone()), cx))
                .child(said(name, value))
                .child(ui::icon("chevron-down", 12., if set { Theme::accent() } else { Theme::text_tertiary() }))
                .into_any_element()
        };
        let filtered = f.active() > 0 || !f.search.trim().is_empty();
        // A list too narrow for the buttons' words keeps their icons, and says the words on hover.
        let roomy = width >= f32::from(text_px(620.));
        lib.search.update(cx, |input, _| input.set_placeholder(if width >= f32::from(text_px(440.)) { "搜索题目、作者、DOI、标签、词条、笔记…" } else { "搜索…" }));

        // The filters stay one row. Where they do not all fit, the row is folded: those that are set come first,
        // then as many as there is room for and how many more there are; under the pointer a card shows them all.
        const GAP: f32 = 6.;
        let face = theme::ui_font();
        let wide = |text: &str| {
            let run = TextRun { len: text.len(), font: face.clone(), color: Theme::text(), background_color: None, underline: None, strikethrough: None };
            f32::from(window.text_system().shape_line(text.to_string().into(), text_px(12.), &[run], None).width)
        };
        // A chip: its words, the chevron after a gap, the padding and the border.
        let chip_width = |(_, name, value, _): &(ElementId, String, String, FilterKey)| wide(&said(name, value)) + 4. + 12. + 18. + 2.;
        let clear = |this: &Self, cx: &mut Context<Self>| ui::ghost_button("f-clear", Some("x"), Some(t("清空筛选").into())).h(text_px(26.)).on_click(this.on(Cmd::ClearFilters, cx)).into_any_element();
        let room = width - 34.;
        let all = filters.iter().map(chip_width).sum::<f32>() + GAP * (filters.len() - 1) as f32 + if filtered { GAP + wide(t("清空筛选")) + 37. } else { 0. };
        let mut row: Vec<AnyElement> = vec![];
        let mut hidden = 0;
        if all <= room {
            for filter in &filters { row.push(chip(filter, self, cx)) }
            if filtered { row.push(clear(self, cx)) }
        } else {
            filters.sort_by_key(|(_, _, value, _)| value.is_empty());
            // Room is kept for the count of the rest, and for the x that clears.
            let mut used = wide("+9") + 16. + if filtered { GAP + 28. } else { 0. };
            for (i, filter) in filters.iter().enumerate() {
                used += GAP + chip_width(filter);
                if i > 0 && used > room { break }
                row.push(chip(filter, self, cx));
            }
            hidden = filters.len() - row.len();
            if hidden > 0 {
                let set = filters[row.len()..].iter().any(|(_, _, value, _)| !value.is_empty());
                row.push(div().flex().flex_none().items_center().h(text_px(26.)).px(px(8.)).rounded(px(13.)).text_size(text_px(12.)).bg(if set { Theme::accent_soft() } else { Theme::bg_sunken() }).text_color(if set { Theme::accent() } else { Theme::text_secondary() }).child(format!("+{hidden}")).into_any_element());
            }
            if filtered { row.push(ui::ghost_button("f-clear", Some("x"), None).h(text_px(26.)).tooltip(ui::tip(t("清空筛选"))).on_click(self.on(Cmd::ClearFilters, cx)).into_any_element()) }
        }
        self.filters_drawn = hidden > 0;
        // The card lies over the row, its first chips where the row's are, so a press meant for one of those lands on it.
        // It stays under a filter's choices, and comes after a moment: a pointer on its way across does not raise it.
        let card = (hidden > 0 && (self.filters_over != [false; 2] || self.menu.as_ref().is_some_and(|menu| menu.filter))).then(|| {
            let mut chips: Vec<AnyElement> = vec![];
            for filter in &filters { chips.push(chip(filter, self, cx)) }
            if filtered { chips.push(clear(self, cx)) }
            div()
                .id("filters-card")
                .absolute()
                .top(px(14. + 2.) + text_px(34.))
                .left(px(8.))
                .right(px(8.))
                .occlude()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(GAP))
                .p(px(7.))
                .rounded(px(10.))
                .border_1()
                .border_color(Theme::line())
                .bg(Theme::bg_body())
                .shadow(theme::shadow_panel())
                .on_hover(cx.listener(|this, over: &bool, _, cx| {
                    this.filters_over[1] = *over;
                    cx.notify();
                }))
                .children(chips)
                .with_animation("filters-card", Animation::new(std::time::Duration::from_millis(240)), |card, shown| card.opacity(((shown - 0.4) / 0.6).max(0.)))
        });

        let (columns, left_out) = Self::fitted(&self.settings, lib, width);
        // The columns there is no room for, said at the end of the title's name: the title's cell is the one that gives, so nothing moves.
        let more = (!left_out.is_empty()).then(|| {
            let (count, names) = (left_out.len(), left_out.join(if crate::i18n::is_english() { ", " } else { "、" }));
            div()
                .id("th-more")
                .flex()
                .flex_none()
                .items_center()
                .h(text_px(20.))
                .px(px(7.))
                .rounded(px(10.))
                .bg(Theme::bg_body())
                .border_1()
                .border_color(Theme::line())
                .text_size(text_px(11.))
                .text_color(Theme::text_secondary())
                .cursor_pointer()
                .hover(|s| s.text_color(Theme::text()))
                .occlude()
                .child(tf!("+{count} 列", "+{count} columns"))
                .tooltip(ui::tip(tf!("这个宽度放不下 {count} 列：{names}。拉宽窗口或收起侧栏就会出现；点这里选择显示哪些列。", "No room at this width for {count} columns: {names}. Widen the window or put the sidebar away to see them; click to choose the columns.")))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_menu(MenuKind::Columns, event.position, cx);
                }))
        });
        let mut more = more;
        let (sort_key, sort_desc) = filter::sort_parts(&f.sort);
        let spare = (Self::title_room(&columns, width) - TITLE_MIN).max(0.);
        let header: Vec<AnyElement> = columns
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let active = c.sort == Some(sort_key);
                // The edge at a column's left is held and moved for other widths: the column gives to the one before it or
                // takes from it (the title has what the others leave). Clicked twice, the column has its own width again.
                let edge = (i >= 2).then(|| {
                    let left = columns[i - 1].width.map(|w| (columns[i - 1].key.clone(), w));
                    let right = (c.key.clone(), c.width.unwrap_or(0.));
                    let held = self.resizing == Some(Edge::Column) && self.column_drag.as_ref().is_some_and(|drag| drag.right.0 == c.key);
                    let group = SharedString::from(format!("th-edge-{i}"));
                    div()
                        .id(("th-edge", i))
                        .group(group.clone())
                        .absolute()
                        .top_0()
                        .left(px(-5.))
                        .w(px(9.))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_col_resize()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if event.click_count >= 2 {
                                this.resizing = None;
                                this.column_drag = None;
                                this.settings.column_widths.remove(&right.0);
                                this.save_settings(cx);
                            } else {
                                this.resizing = Some(Edge::Column);
                                this.column_drag = Some(ColumnDrag { x: f32::from(event.position.x), left: left.clone(), right: right.clone(), spare });
                            }
                            cx.notify();
                        }))
                        // A tap is over before the layer that follows the pointer is drawn: the edge hears the end of it itself.
                        .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
                        .child(div().w(px(1.)).h(text_px(14.)).when(held, |d| d.w(px(2.)).h_full().bg(Theme::accent())).when(!held, |d| d.bg(Theme::line_strong()).group_hover(group, |s| s.bg(Theme::accent()))))
                });
                div()
                    .id(("th", i))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .h_full()
                    .px(px(8.))
                    .map(|d| match c.width {
                        Some(w) => d.w(px(w)).flex_none(),
                        None => d.flex_1().min_w_0(),
                    })
                    .when(c.kind.centred(), |d| d.justify_center())
                    .when_some(c.sort, |d, key| d.cursor_pointer().hover(|s| s.text_color(Theme::text())).on_click(self.on(Cmd::SetSort(filter::toggle_sort(&f.sort, key)), cx)))
                    .when(active, |d| d.text_color(Theme::text()))
                    .child(div().min_w_0().truncate().child(c.label.clone()))
                    .children(edge)
                    .when(active, |d| d.child(ui::icon("chevron-down", 12., Theme::text()).when(!sort_desc, |s| s.with_transformation(Transformation::rotate(percentage(0.5))))))
                    .when_some(if matches!(c.kind, ColumnKind::Title) { more.take() } else { None }, |d, more| d.child(div().flex_1()).child(more))
                    .into_any_element()
            })
            .collect();

        let total = filter::view_count(&lib.papers, View::All, 0);
        let shown = lib.shown.len();
        // Of the papers on show, those without a PDF that have a number to be looked up by.
        let lookable = if lib.filters.view == View::NoPdf { lib.shown.iter().filter(|&&i| wisp_core::openaccess::lookable(&lib.papers[i])).count() } else { 0 };
        let body = if shown == 0 {
            if lib.papers.is_empty() {
                ui::empty_state("library", t("这个文献库还是空的"), t("点击左侧「添加文献」：粘贴 DOI 或 PMID、导入 BibTeX / RIS，或者直接把 PDF 或整个文件夹拖进窗口（子文件夹的名字会成为标签）。")).into_any_element()
            } else {
                ui::empty_state("search", t("没有匹配的文献"), t("调整搜索词或筛选条件。")).child(ui::secondary_button("empty-clear", t("清空筛选")).on_click(self.on(Cmd::ClearFilters, cx))).into_any_element()
            }
        } else {
            // A bar at the right edge shows where in a long list the rows on show are, and is dragged to go elsewhere.
            div().relative().flex().flex_col().flex_1().min_h_0()
                .child(uniform_list("papers", shown, cx.processor(move |this, range: Range<usize>, window, cx| this.render_rows(range, width, window, cx))).track_scroll(lib.table_scroll.clone()).flex_1())
                .child(crate::scrollbar::scrollbar("papers-bar", lib.table_scroll.clone()))
                .into_any_element()
        };
        let marked = lib.marked.len();
        let status = if marked > 0 {
            tf!("已选 {marked} 篇 · 右键批量操作 · Esc 取消", "{marked} selected · Right-click for batch actions · Esc to cancel")
        } else if let Some(run) = lib.import.as_ref().filter(|run| run.running()) {
            if run.ids == 0 { tf!("正在导入 PDF，已完成 {} / {}", "Importing PDFs, {} / {} complete", run.rows.len(), run.total) } else { tf!("正在导入，已完成 {} / {}", "Importing, {} / {} complete", run.rows.len(), run.total) }
        } else if !lib.extracting.is_empty() {
            tf!("正在提取 {} 篇全文的文字，用于提问…", "Extracting full text from {} papers for chat…", lib.extracting.len())
        } else {
            tf!("↑↓ 切换文献 · Shift / {} 点选多篇 · 右键更多操作 · {} 搜索", "↑↓ Switch paper · Shift / {} to select multiple · Right-click for more · {} Search", if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" }, crate::shortcuts::pretty(&crate::shortcuts::key_of("search", &self.settings.shortcuts)))
        };
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            // The search row and the filters keep one height however narrow the list is (`papers_head`): the buttons
            // drop their words, the filters fold. The paper beside the list is laid out by that height.
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .pt(px(14.))
                    // 收藏: the starred papers here, the figure library beside them.
                    .when(lib.filters.view == View::Starred, |d| d.child(self.render_collection_switch(cx)))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w(px(80.))
                            .overflow_hidden()
                            .items_center()
                            .gap(px(8.))
                            .h(text_px(34.))
                            .px(px(10.))
                            .rounded(px(8.))
                            .bg(Theme::bg_sunken())
                            .border_1()
                            .border_color(if lib.search.focus_handle(cx).is_focused(window) { Theme::accent() } else { Theme::bg_sunken() })
                            .child(ui::icon("search", 15., Theme::text_tertiary()))
                            .child(lib.search.clone()),
                    )
                    .when(roomy, |d| {
                        d.child(ui::secondary_button("sort", t("排序")).child(ui::icon("sort-down", 14., Theme::text_secondary())).on_click(self.on_menu(MenuKind::Sort, cx)))
                            .child(ui::secondary_button("list-translation", if lib.show_translation { t("显示原文") } else { t("显示译文") }).on_click(self.on(Cmd::ToggleTranslation, cx)))
                            .child(ui::secondary_button("export", t("导出")).child(ui::icon("export", 14., Theme::text_secondary())).on_click(self.on_menu(MenuKind::Export, cx)))
                    })
                    .when(!roomy, |d| {
                        d.child(ui::tool_button("sort", "sort-down", t("排序")).on_click(self.on_menu(MenuKind::Sort, cx)))
                            // Lit while the translations are on show.
                            .child(ui::tool_button("list-translation", "languages", if lib.show_translation { t("显示原文") } else { t("显示译文") }).when(lib.show_translation, |d| d.bg(Theme::accent_soft()).border_color(Theme::accent_soft()).hover(|s| s.bg(Theme::accent_soft()))).on_click(self.on(Cmd::ToggleTranslation, cx)))
                            .child(ui::tool_button("export", "export", t("导出")).on_click(self.on_menu(MenuKind::Export, cx)))
                    }),
            )
            .child(div().id("filters").flex().flex_none().items_center().gap(px(GAP)).px(px(16.)).py(px(10.)).overflow_hidden().when(hidden > 0, |d| {
                d.on_hover(cx.listener(|this, over: &bool, _, cx| {
                    this.filters_over[0] = *over;
                    cx.notify();
                }))
            }).children(row))
            .child(div().flex().flex_none().h(text_px(32.)).px(px(8.)).border_t_1().border_b_1().border_color(Theme::line()).bg(Theme::bg_sunken()).text_size(text_px(12.)).text_color(Theme::text_tertiary())
                // A right click on the names: which columns are on show.
                .on_mouse_down(MouseButton::Right, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_menu(MenuKind::Columns, event.position, cx);
                }))
                .children(header))
            // The papers without a PDF: the open ones among them can be fetched in one go.
            .when(lib.filters.view == View::NoPdf && lookable > 0 && lib.fetching.as_ref().is_none_or(|run| !run.running()), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(10.))
                        .px(px(16.))
                        .py(px(8.))
                        .border_b_1()
                        .border_color(Theme::line())
                        .bg(Theme::accent_soft())
                        .child(div().flex_1().min_w_0().text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_secondary()).child(tf!("这些文献还没有 PDF。可以替你找开放获取的全文（arXiv、开放获取期刊、作者存档的版本），下载后附到各自的条目上；要付费或登录的不会去取。查找时会把 DOI 或 PMID 发给 OpenAlex、Europe PMC 和出版商。{}", "Find open access full text from arXiv, journals and author repositories, then attach it to each paper. Paid or login-only files are skipped. DOI or PMID will be sent to OpenAlex, Europe PMC and publishers.{}", if lookable < shown { tf!("另有 {} 篇没有 DOI、PMID 或 arXiv 编号，查不了。", "Another {} papers have no DOI, PMID or arXiv ID and cannot be searched.", shown - lookable) } else { String::new() })))
                        .child(ui::primary_button("find-pdfs", tf!("为 {lookable} 篇查找开放全文", "Find open full text for {lookable} papers")).h(text_px(30.)).flex_none().on_click(self.on(Cmd::FindPdfs(None), cx))),
                )
            })
            .child(body)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .h(text_px(30.))
                    .px(px(16.))
                    .border_t_1()
                    .border_color(Theme::line())
                    .text_size(text_px(12.))
                    .text_color(Theme::text_tertiary())
                    .gap(px(12.))
                    .child(div().flex_none().child(if filtered || shown != total { tf!("显示 {shown} / {total} 篇", "Showing {shown} / {total} papers") } else { tf!("共 {total} 篇", "{total} papers in total") }))
                    .child(div().min_w_0().truncate().child(status)),
            )
            // Last, so it is drawn over the column names and the list.
            .children(card)
            .into_any_element()
    }

    fn render_rows(&mut self, range: Range<usize>, width: f32, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(lib) = &self.lib else { return vec![] };
        let columns = Self::columns(&self.settings, lib, width);
        let title_room = Self::title_room(&columns, width);
        // How wide words come out, to know which a cell cuts: those are said in full under the pointer.
        let style = window.text_style();
        let (base, small) = (style.font_size.to_pixels(window.rem_size()), text_px(12.));
        let wide = |text: &str, size: Pixels, weight: FontWeight| {
            let text = text.replace('\n', " ");
            let run = TextRun { len: text.len(), font: Font { weight, ..style.font() }, color: Theme::text(), background_color: None, underline: None, strikethrough: None };
            f32::from(window.text_system().shape_line(text.into(), size, &[run], None).width)
        };
        let row_height = self.row_height(false);
        let muted = |text: &str| div().text_color(Theme::text_placeholder()).child(text.to_string());
        range
            .filter_map(|row| {
                let p = lib.papers.get(*lib.shown.get(row)?)?;
                let selected = lib.selected.as_deref() == Some(p.id.as_str()) || lib.marked.contains(&p.id);
                // The paper a right-click menu is about.
                let pointed = self.menu.as_ref().is_some_and(|menu| menu.paper.as_deref() == Some(p.id.as_str()));
                let cells: Vec<AnyElement> = columns
                    .iter()
                    .map(|c| {
                        let cell = div().flex().items_center().h_full().overflow_hidden().px(px(8.)).map(|d| match c.width {
                            Some(w) => d.w(px(w)).flex_none(),
                            None => d.flex_1().min_w_0(),
                        }).when(c.kind.centred(), |d| d.justify_center());
                        // A cell that cuts what it shows says it in full under the pointer.
                        let room = c.width.unwrap_or(0.) - 16.;
                        let tipped = |cell: Div, tip: Option<String>| match tip {
                            Some(tip) => cell.id(ElementId::NamedInteger(c.key.clone().into(), row as u64)).tooltip(ui::tip(tip)).into_any_element(),
                            None => cell.into_any_element(),
                        };
                        let chip = |cell: Div, key: &str, name: &str| tipped(cell.child(ui::status_chip(key, name)), c.narrowed.then(|| name.to_string()));
                        match &c.kind {
                            ColumnKind::Star => cell
                                .px(px(4.))
                                .child(
                                    div()
                                        .id(("star", row))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(26.))
                                        .rounded(px(6.))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(Theme::hover()))
                                        .on_click({
                                            let toggle = self.on(Cmd::ToggleStar(p.id.clone()), cx);
                                            move |ev, window, cx| {
                                                cx.stop_propagation();
                                                toggle(ev, window, cx);
                                            }
                                        })
                                        .child(ui::icon("star", 15., if p.starred { Theme::star() } else { Theme::text_placeholder() })),
                                )
                                .into_any_element(),
                            ColumnKind::Title => {
                                let title = self.listed_title(lib, p);
                                let mut meta = vec![];
                                let authors = crate::i18n::short_authors(p);
                                if !authors.is_empty() { meta.push(authors) }
                                if let Some(year) = p.year { meta.push(year.to_string()) }
                                if !p.journal.is_empty() { meta.push(p.journal.clone()) }
                                let meta = meta.join(" · ");
                                let weight = if p.reading == "unread" { FontWeight::MEDIUM } else { FontWeight::NORMAL };
                                let preprint = p.publication_status == "preprint";
                                let beside = p.tags.iter().take(2).map(|tag| 6. + wide(&format!("#{tag}"), small, FontWeight::NORMAL)).sum::<f32>() + if preprint { 6. + wide(t("预印本"), small, FontWeight::NORMAL) } else { 0. };
                                let cut = p.tags.len() > 2 || wide(title, base, weight) > title_room + 0.5 || wide(&meta, small, FontWeight::NORMAL) + beside > title_room + 0.5;
                                let tip = cut.then(|| {
                                    let (title, meta, tags) = (title.clone(), meta.clone(), p.tags.clone());
                                    move |_: &mut Window, cx: &mut App| AnyView::from(cx.new(|_| PaperTip { title: title.clone(), meta: meta.clone(), tags: tags.clone() }))
                                });
                                cell.id(("title", row)).when_some(tip, |d, tip| d.tooltip(tip)).child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .min_w_0()
                                        .w_full()
                                        .child(div().truncate().font_weight(weight).text_color(if p.reading == "unread" { Theme::text() } else { Theme::text_secondary() }).child(title.clone()))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(6.))
                                                .text_size(text_px(12.))
                                                .line_height(text_px(18.))
                                                .text_color(Theme::text_tertiary())
                                                .child(div().min_w_0().truncate().child(if meta.is_empty() { t("作者、年份待补").to_string() } else { meta.clone() }))
                                                .when(preprint, |d| d.child(div().flex_none().text_color(theme::status_colors("preprint").0).child(t("预印本"))))
                                                .children(p.tags.iter().take(2).map(|t| div().flex_none().text_color(Theme::accent()).child(format!("#{t}")))),
                                        ),
                                )
                                .into_any_element()
                            }
                            ColumnKind::Journal => tipped(cell.child(if p.journal.is_empty() { muted("—") } else { div().truncate().text_color(Theme::text_secondary()).child(p.journal.clone()) }), (wide(&p.journal, base, FontWeight::NORMAL) > room + 0.5).then(|| p.journal.clone())),
                            ColumnKind::Impact | ColumnKind::Zone => {
                                let journal = wisp_core::journals::find(&p.journal);
                                let which = zone_sort(&self.settings);
                                let said = journal.and_then(|j| if matches!(c.kind, ColumnKind::Impact) { j.impact_text() } else { zone_text(j, which) });
                                // A journal a list warns against says so in the colour of a warning, and why under the pointer.
                                let warned = journal.filter(|j| !j.warning.is_empty());
                                let tip = journal.filter(|_| said.is_some()).map(|j| [j.name().to_string(), journal_line(j), t(&j.warning).to_string()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n"));
                                tipped(cell.child(match said {
                                    Some(said) => div().truncate().text_color(if warned.is_some() { Theme::warning() } else { Theme::text_secondary() }).child(said),
                                    None => muted("—"),
                                }), tip)
                            }
                            ColumnKind::Year => cell.child(match p.year {
                                Some(year) => div().text_color(Theme::text_secondary()).child(year.to_string()),
                                None => muted("—"),
                            })
                            .into_any_element(),
                            ColumnKind::Field(id) => {
                                let said = p.fields.get(id).map(|values| values.join("、"));
                                let tip = said.clone().filter(|said| wide(said, base, FontWeight::NORMAL) > room + 0.5);
                                tipped(cell.child(match said {
                                    Some(said) => div().truncate().text_color(Theme::text_secondary()).child(said),
                                    None => muted("—"),
                                }), tip)
                            }
                            ColumnKind::Fulltext => chip(cell, &p.fulltext, label(&FULLTEXTS, &p.fulltext)),
                            ColumnKind::Reading => chip(cell, &p.reading, label(&READINGS, &p.reading)),
                            ColumnKind::Screening => chip(cell, &p.screening, label(&SCREENINGS, &p.screening)),
                        }
                    })
                    .collect();
                Some(
                    div()
                        .id(("row", row))
                        .flex()
                        .items_center()
                        .h(px(row_height))
                        .w(px(width))
                        .overflow_hidden()
                        .px(px(8.))
                        .border_b_1()
                        .border_color(Theme::line())
                        .cursor_pointer()
                        .when(selected, |d| d.bg(Theme::accent_soft()))
                        .when(!selected && pointed, |d| d.bg(Theme::hover()))
                        .when(!selected && !pointed, |d| d.hover(|s| s.bg(Theme::hover())))
                        .on_mouse_down(MouseButton::Right, self.on_paper_context(p.id.clone(), cx))
                        .on_click(cx.listener({
                            let id = p.id.clone();
                            move |this, ev: &ClickEvent, window, cx| this.row_clicked(&id, ev, window, cx)
                        }))
                        .children(cells)
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// A paper's title as the lists show it: translated while translations are on show and this one is there.
    fn listed_title<'a>(&'a self, lib: &'a Library, p: &'a Paper) -> &'a String {
        if !lib.show_translation { return &p.title }
        match lib.translations.get(&wisp_core::store::translation_key(&p.title, wisp_core::ai::translation_prompt(&self.settings.translation_prompt))) {
            Some(Translated::Done(text)) => text,
            _ => &p.title,
        }
    }

    /// A click on a paper's row, in the table or in the narrow list beside a paper that is asked about.
    fn row_clicked(&mut self, id: &str, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        // With ⌘ / Ctrl or Shift held the click marks papers; alone it shows one.
        let held = ev.modifiers();
        if held.secondary() || held.shift { return self.mark(id, held.shift, window, cx) }
        if let Some(lib) = self.lib.as_mut() { (lib.anchor, lib.cursor) = (Some(id.to_string()), None) }
        // A double click opens the PDF straight away; beside a paper that is asked about, its PDF is on show already.
        if ev.click_count() >= 2 && !self.asking(window) && let Some(lib) = self.lib.as_mut() && lib.pdf_file(id).is_some() { lib.tab = Tab::Pdf }
        self.run(Cmd::Select(id.to_string()), window, cx);
        self.run(Cmd::SetTab(self.lib.as_ref().map_or(Tab::Info, |l| l.tab)), window, cx);
        // The keyboard stays with the list, for the arrow keys; a question about the paper keeps its box.
        if self.lib.as_ref().is_some_and(|l| l.tab != Tab::Ask) { window.focus(&self.focus) }
    }

    /// The papers on show as a narrow list, at the left of a paper that is asked about: the way
    /// back at the top where the sidebar's corner is, the search, then a title and a line for each.
    fn render_ask_list(&mut self, width: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let (shown, total) = (lib.shown.len(), filter::view_count(&lib.papers, View::All, 0));
        let filtered = lib.filters.active() > 0 || !lib.search.read(cx).text().trim().is_empty();
        lib.search.update(cx, |input, _| input.set_placeholder("搜索…"));
        let body = if shown == 0 {
            div().flex_1().px(px(8.)).pt(px(8.)).text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_placeholder()).child(t("没有匹配的文献。换个搜索词，或按 Esc 回到完整列表调整筛选。")).into_any_element()
        } else {
            div().relative().flex().flex_col().flex_1().min_h_0()
                .child(uniform_list("ask-papers", shown, cx.processor(move |this, range: Range<usize>, _, cx| this.render_ask_rows(range, cx))).track_scroll(lib.table_scroll.clone()).flex_1())
                .child(crate::scrollbar::scrollbar("ask-papers-bar", lib.table_scroll.clone()))
                .into_any_element()
        };
        // The edge between the list and the paper is drawn by the detail panel, which is where it is dragged.
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            .h_full()
            .px(px(10.))
            .pb(px(8.))
            .pt(px(Self::corner_top()))
            .gap(px(8.))
            .child(self.render_corner(width >= ui::traffic_lights_width() + f32::from(text_px(150.)), true, cx).when(ui::custom_titlebar(), |d| d.mb(px(4.))))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .h(text_px(32.))
                    .px(px(10.))
                    .rounded(px(8.))
                    .bg(Theme::bg_body())
                    .border_1()
                    .border_color(if lib.search.focus_handle(cx).is_focused(window) { Theme::accent() } else { Theme::line() })
                    .child(ui::icon("search", 14., Theme::text_tertiary()))
                    .child(lib.search.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .px(px(8.))
                    .child(ui::caption(if lib.filters.tags.is_empty() && lib.filters.not_tags.is_empty() { view_label(lib.filters.view).to_string() } else { format!("{} · {}", view_label(lib.filters.view), lib.filters.tags_said()) }).min_w_0().truncate())
                    .child(div().flex_none().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(if filtered || shown != total { tf!("{shown} / {total} 篇", "{shown} / {total} papers") } else { tf!("{total} 篇", "{total} papers") })),
            )
            .child(body)
            .child(div().flex_none().px(px(8.)).truncate().text_size(text_px(11.)).text_color(Theme::text_placeholder()).child(tf!("↑↓ 切换文献 · {} 回到完整列表", "↑↓ Switch paper · {} Back to full list", crate::shortcuts::pretty("escape"))))
            .into_any_element()
    }

    fn render_ask_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(lib) = &self.lib else { return vec![] };
        let row_height = self.row_height(true);
        let compact = self.settings.density == "compact";
        range
            .filter_map(|row| {
                let p = lib.papers.get(*lib.shown.get(row)?)?;
                let selected = lib.selected.as_deref() == Some(p.id.as_str()) || lib.marked.contains(&p.id);
                let pointed = self.menu.as_ref().is_some_and(|menu| menu.paper.as_deref() == Some(p.id.as_str()));
                let unread = p.reading == "unread";
                let mut meta = vec![];
                let authors = crate::i18n::short_authors(p);
                if !authors.is_empty() { meta.push(authors) }
                if let Some(year) = p.year { meta.push(year.to_string()) }
                if meta.is_empty() && !p.journal.is_empty() { meta.push(p.journal.clone()) }
                Some(
                    // A row of the list's own height, and inside it the rounded piece that is pointed at, as in the sidebar.
                    div()
                        .id(("ask-row", row))
                        .h(px(row_height))
                        .w_full()
                        .py(px(1.))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Right, self.on_paper_context(p.id.clone(), cx))
                        .on_click(cx.listener({
                            let id = p.id.clone();
                            move |this, ev: &ClickEvent, window, cx| this.row_clicked(&id, ev, window, cx)
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .justify_center()
                                .size_full()
                                .px(px(8.))
                                .rounded(px(6.))
                                .when(selected, |d| d.bg(Theme::accent_soft()))
                                .when(!selected && pointed, |d| d.bg(Theme::hover()))
                                .when(!selected && !pointed, |d| d.hover(|s| s.bg(Theme::hover())))
                                .child(div().w_full().text_size(text_px(13.)).line_height(text_px(19.)).map(|d| if compact { d.truncate() } else { d.line_clamp(2) }).font_weight(if unread || selected { FontWeight::MEDIUM } else { FontWeight::NORMAL }).text_color(if selected { Theme::accent() } else if unread { Theme::text() } else { Theme::text_secondary() }).child(self.listed_title(lib, p).clone()))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .text_size(text_px(12.))
                                        .line_height(text_px(18.))
                                        .text_color(Theme::text_tertiary())
                                        .child(div().min_w_0().truncate().child(if meta.is_empty() { t("作者、年份待补").to_string() } else { meta.join(" · ") }))
                                        .when(p.starred, |d| d.child(ui::icon("star", 11., Theme::star())))
                                        // Asked about without its PDF, the answer has only the record to go by.
                                        .when(lib.pdf_file(&p.id).is_none(), |d| d.child(div().flex_none().text_color(Theme::text_placeholder()).child(t("无 PDF")))),
                                ),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_trash(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let rows: Vec<AnyElement> = lib
            .trash
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let p = &entry.paper;
                let chosen = lib.trash_chosen.contains(&p.id);
                // A click chooses the paper, with ⌘ / Ctrl or on its box one more, with Shift a stretch; its buttons are for it alone.
                div()
                    .id(SharedString::from(format!("trashed-{}", p.id)))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(16.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(Theme::line())
                    .cursor_pointer()
                    .when(chosen, |d| d.bg(Theme::accent_soft()))
                    .when(!chosen, |d| d.hover(|s| s.bg(Theme::hover())))
                    .on_mouse_down(MouseButton::Right, self.on_trashed_context(p.id.clone(), cx))
                    .on_click(cx.listener({
                        let id = p.id.clone();
                        move |this, ev: &ClickEvent, window, cx| this.choose_trashed(&id, ev.modifiers().secondary(), ev.modifiers().shift, window, cx)
                    }))
                    .child(div().id(("trash-tick", i)).flex_none().child(crate::extract::tick_box(chosen, true)).on_click(cx.listener({
                        let id = p.id.clone();
                        move |this, ev: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            this.choose_trashed(&id, true, ev.modifiers().shift, window, cx)
                        }
                    })))
                    .child(div().flex().flex_col().flex_1().min_w_0().child(div().truncate().when(chosen, |d| d.text_color(Theme::accent())).child(p.title.clone())).child(div().truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(tf!("{} · 删除于 {}", "{} · Deleted {}", if p.authors.is_empty() { t("作者待补").to_string() } else { crate::i18n::short_authors(p) }, time::date_text(&entry.deleted_at)))))
                    .child(ui::secondary_button(("restore", i), t("恢复")).on_click(self.on_alone(Cmd::RestorePaper(p.id.clone()), cx)))
                    .child(ui::ghost_button(("delete", i), Some("bin"), Some(t("彻底删除").into())).text_color(Theme::danger()).on_click(self.on_alone(Cmd::AskDeletePaper(p.id.clone()), cx)))
                    .into_any_element()
            })
            .collect();
        let (chosen, total) = (lib.trash_chosen_ids(), lib.trash.len());
        let count = chosen.len();
        // With papers chosen, the head's two buttons are for them, where 清空回收站 and 全部恢复 stand otherwise.
        let (delete, restore) = if count == 0 {
            (ui::ghost_button("trash-empty", Some("bin"), Some(t("清空回收站").into())).on_click(self.on(Cmd::AskEmptyTrash, cx)), ui::secondary_button("trash-restore-all", t("全部恢复")).on_click(self.on(Cmd::RestoreAll, cx)))
        } else {
            (ui::ghost_button("trash-delete-chosen", Some("bin"), Some(tf!("彻底删除所选 {count} 篇", "Delete {count} selected").into())).on_click(self.on(Cmd::AskDeletePapers(chosen.clone()), cx)), ui::secondary_button("trash-restore-chosen", tf!("恢复所选 {count} 篇", "Restore {count} selected")).on_click(self.on(Cmd::RestorePapers(chosen), cx)))
        };
        let all = count == total;
        div()
            .flex()
            .flex_col()
            .size_full()
            // 全部恢复 stands over the rows' 恢复, 全选 beside it; 清空回收站, which cannot be undone, by the title, away from them.
            .child(ui::column_head().gap(px(8.)).child(div().flex_none().text_size(text_px(ui::HEAD_TITLE)).font_weight(FontWeight::SEMIBOLD).child(t("回收站"))).child(ui::hint("trash-hint", t("这里的文献保留着笔记、词条和 PDF，可以恢复。它们仍会阻止同一篇文献被重复导入。")))
                .when(!rows.is_empty(), |d| d.child(delete.text_color(Theme::danger())).child(div().flex_1()).child(ui::ghost_button("trash-choose-all", None, Some(if all { t("取消全选") } else { t("全选") }.into())).on_click(self.on(Cmd::ChooseAllTrash(!all), cx))).child(restore)))
            .child(if rows.is_empty() { ui::empty_state("bin", t("回收站是空的"), "").into_any_element() } else { div().id("trash").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().children(rows).into_any_element() })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's own `test` attribute.
    use super::{ASK_LIST_MAX, ASK_LIST_MIN, ASK_LIST_W, COLUMN_MAX, COLUMN_MIN, SPLIT_CHAT_MIN, SPLIT_PDF_MIN, ask_list_width, dragged, sort_in_zone, sortable_width, split_chat_reach, split_chat_width};
    use crate::app::ColumnDrag;
    use crate::detail::SPLIT_MIN;

    #[test]
    fn a_sorted_header_has_room_for_its_name_and_arrow() {
        // What the cell leaves the name once sorted: its width less the padding, the gap and the arrow.
        let room = |width: f32| width - 16. - 15.;
        // 影响因子 is four characters of 12px: the 76 it had left 45.
        assert!(room(76.) < 48.);
        assert!(room(sortable_width("影响因子", 76., 1.)) >= 48.);
        assert!(room(sortable_width("Impact factor", 76., 1.)) >= 13. * 6.5);
        // A short name keeps the column's own width; larger type asks for more.
        assert_eq!(sortable_width("分区", 76., 1.), 76.);
        assert_eq!(sortable_width("Zone", 76., 1.), 76.);
        assert!(sortable_width("影响因子", 76., 1.5) > sortable_width("影响因子", 76., 1.));
    }

    #[test]
    fn a_sort_by_a_partition_is_by_the_one_the_column_shows() {
        let mut settings = wisp_core::settings::Settings::default();
        // 中科院 is the column's partition when none is chosen.
        assert_eq!(sort_in_zone("jcr-asc", &settings), "cas-asc");
        assert_eq!(sort_in_zone("xr-desc", &settings), "cas-desc");
        settings.journal_zone = "jcr".into();
        assert_eq!(sort_in_zone("cas-asc", &settings), "jcr-asc");
        assert_eq!(sort_in_zone("jcr-desc", &settings), "jcr-desc");
        // Any other sort is left as it is, the empty one (latest added) too.
        for sort in ["", "newest", "impact-desc", "year-asc", "title-desc"] { assert_eq!(sort_in_zone(sort, &settings), sort) }
    }

    #[test]
    fn list_beside_a_paper_asked_about() {
        // No wish: the width it comes with, where the window has room for it.
        assert_eq!(ask_list_width(0., 1360.), ASK_LIST_W);
        // The conversation and the PDF keep their least: the list gives way, down to none.
        assert_eq!(ask_list_width(0., SPLIT_MIN + 240.), 240.);
        assert_eq!(ask_list_width(0., SPLIT_MIN + ASK_LIST_MIN), ASK_LIST_MIN);
        assert_eq!(ask_list_width(0., SPLIT_MIN + ASK_LIST_MIN - 1.), 0.);
        // A wish is kept within the list's own bounds.
        assert_eq!((ask_list_width(340., 1360.), ask_list_width(40., 1360.), ask_list_width(900., 1600.)), (340., ASK_LIST_MIN, ASK_LIST_MAX));
    }

    #[test]
    fn conversation_beside_pdf() {
        // No wish: a good share of the panel, within its own bounds.
        assert_eq!((split_chat_width(0., 760.), split_chat_width(0., 1000.), split_chat_width(0., 1600.)), (360., 450., 520.));
        // A wish is kept where the panel has room, and the PDF keeps its least.
        assert_eq!(split_chat_width(600., 1600.), 600.);
        assert_eq!(split_chat_width(900., 1000.), 1000. - SPLIT_PDF_MIN);
        assert_eq!(split_chat_width(40., 1000.), SPLIT_CHAT_MIN);
        // The narrowest panel that shows both gives each its least.
        assert!(split_chat_width(2000., crate::detail::SPLIT_MIN) >= SPLIT_CHAT_MIN);
    }

    #[test]
    fn edge_between_two_columns() {
        let drag = |left: Option<f32>, right: f32, spare: f32| ColumnDrag { x: 500., left: left.map(|w| ("journal".to_string(), w)), right: ("year".to_string(), right), spare };
        // Beside the title, the column takes what the edge is moved by, as far as the title can spare.
        assert_eq!(dragged(&drag(None, 150., 200.), 440.), (None, 210.));
        assert_eq!(dragged(&drag(None, 150., 200.), 100.), (None, 350.));
        // To the right it narrows, down to the least a column has.
        assert_eq!(dragged(&drag(None, 150., 200.), 900.), (None, COLUMN_MIN));
        // Between two columns one gains what the other gives; neither goes under the least or over the most.
        assert_eq!(dragged(&drag(Some(150.), 60., 0.), 510.), (Some(160.), 50.));
        assert_eq!(dragged(&drag(Some(150.), 60., 0.), 900.), (Some(150. + 60. - COLUMN_MIN), COLUMN_MIN));
        assert_eq!(dragged(&drag(Some(150.), 60., 0.), 0.), (Some(COLUMN_MIN), 60. + 150. - COLUMN_MIN));
        assert_eq!(dragged(&drag(Some(150.), COLUMN_MAX - 20., 0.), 0.), (Some(130.), COLUMN_MAX));
    }

    #[test]
    fn edge_between_swapped_panes() {
        // A panel from 280 to 1360: the edge at 680 gives the conversation 400 on the left, or 680 on the right.
        assert_eq!(split_chat_reach(680., 280., 1360., false), 400.);
        assert_eq!(split_chat_reach(680., 280., 1360., true), 680.);
        // Dragged towards the conversation's own side, it narrows to its least in either order; the PDF keeps its least.
        assert_eq!(split_chat_width(split_chat_reach(300., 280., 1360., false), 1080.), SPLIT_CHAT_MIN);
        assert_eq!(split_chat_width(split_chat_reach(1340., 280., 1360., true), 1080.), SPLIT_CHAT_MIN);
        assert_eq!(split_chat_width(split_chat_reach(1340., 280., 1360., false), 1080.), 1080. - SPLIT_PDF_MIN);
        assert_eq!(split_chat_width(split_chat_reach(300., 280., 1360., true), 1080.), 1080. - SPLIT_PDF_MIN);
    }

    #[test]
    fn the_filters_of_numbers_say_what_they_keep() {
        use super::{impact_label, range_value, year_label, zone_label};
        use crate::app::FilterKey;
        // Unset, a chip says its name alone.
        assert_eq!((year_label(""), impact_label(""), zone_label("")), (String::new(), String::new(), String::new()));
        assert_eq!([year_label("2024"), year_label("last:3"), year_label("2020-2026"), year_label("2020-"), year_label("-2010")], ["2024", "近 3 年", "2020–2026", "2020 年起", "2010 年及以前"]);
        assert_eq!([impact_label("5-"), impact_label("-3"), impact_label("2.5-10")], ["≥ 5", "≤ 3", "2.5–10"]);
        assert_eq!([zone_label("cas:1"), zone_label("cas:2"), zone_label("xr:3"), zone_label("jcr:1"), zone_label("jcr:2")], ["中科院 1区", "中科院 1–2区", "新锐 1–3区", "JCR Q1", "JCR Q1–Q2"]);
        // The two boxes of 自定义区间…: either end alone, the ends the wrong way round, one year twice, nothing at all.
        let year = |low, high| range_value(&FilterKey::Year, low, high);
        assert_eq!([year("2000", "2026"), year("2020", ""), year("", "2010"), year("2026", "2000"), year("", "")], [Ok("2000-2026".to_string()), Ok("2020-".into()), Ok("-2010".into()), Ok("2000-2026".into()), Ok(String::new())]);
        assert_eq!(year_label(&year("2024", "2024").unwrap()), "2024");
        assert!(year("20", "").is_err() && year("2020", "soon").is_err() && year("-5", "").is_err());
        let impact = |low, high| range_value(&FilterKey::Impact, low, high);
        assert_eq!([impact("5", ""), impact("2.5", "10"), impact("10", "5"), impact("", "3.0")], [Ok("5-".to_string()), Ok("2.5-10".into()), Ok("5-10".into()), Ok("-3".into())]);
        assert!(impact("high", "").is_err() && impact("-1", "").is_err() && impact("inf", "").is_err());
        // What the boxes give, the filter reads.
        assert_eq!(wisp_core::filter::impact_span(&impact("2.5", "10").unwrap()), Some((Some(2.5), Some(10.))));
    }
}
