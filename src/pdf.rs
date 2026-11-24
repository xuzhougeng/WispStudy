//! PDF preview. A worker thread owns the parsed file and rasterises pages
//! with hayro (pure Rust, no system PDF library); the view is a virtualised
//! list that asks for the pages on screen and keeps the latest few. The same
//! thread finds a quoted sentence among the glyphs of a page, so a citation
//! can be shown where it stands, and the words typed into the find bar
//! (the shortcut `search` while the viewer has the keyboard, or 自己搜索
//! when a citation's sentence was not found there).

use crate::i18n::t;
use crate::app::Translated;
use crate::theme::{Theme, text_px};
use crate::ui;
use crate::input::{InputEvent, TextInput};
use gpui::{prelude::FluentBuilder, *};
use wisp_core::annotation::{Annotation, COLORS, Kind, TextPart, TextStyle};
use wisp_core::figref;
use wisp_core::handles::{self, Grab};
use wisp_core::pdftext::{self, Mark};

actions!(pdf, [CopySelection, FindInPdf, RemoveChosen, UndoMark, RedoMark]);

/// What the viewer tells whoever shows it.
pub enum PdfEvent {
    /// A view's label asks for 译文 (the first view to show a translation) or gives it up (the last one):
    /// the owner turns it on or off, as its switch does.
    Facing(bool),
    /// The paper's mind map is asked of the owner, who has it: for a view whose label said 显示导图, or
    /// (`None`) for 导图 on the bar. The owner gives it (`PdfView::set_map`) and has the viewer show it.
    Map(Option<usize>),
    /// No view shows the map any more (closed, or told to show something else): under 分屏 自由 the owner parts
    /// a file split on itself into two viewers again, as it does when 译文 is put away.
    MapGone,
    /// A passage selected on a 1-based page is to be a node of the paper's mind map.
    MapPassage(usize, String),
    /// The pointer went down or turned the wheel in the viewer: beside a viewer of another file of the paper, this is the one worked in from now on.
    Worked,
    /// The pointer went down in the viewer: among the panes of several papers, this paper is the one worked in from now on.
    /// (Turning the wheel in a pane to look something up does not make it so.)
    Pressed,
    /// The selected text went to the clipboard.
    Copied,
    /// ⌘Z (`true`) or ⇧⌘Z in the viewer: the last change of this file's marks is taken back, or done again.
    StepMarks(bool),
    /// Add what is selected to a conversation: the 1-based page, the text, and whether to the library's (else the paper's own).
    Quote(usize, String, bool),
    /// A piece of a page as a picture (PNG) for the next question, and whether for the library's conversation.
    Picture(Vec<u8>, bool),
    /// A piece of a page to keep in the figure library. The viewer goes back to selecting text, so the page's
    /// words can be copied into the dialog; the owner shows the piece meanwhile (`PdfView::set_figures`).
    Figure(Piece),
    /// Translate the selected text: the text, and the model service (its id) asked for this once, else the one translation is bound to.
    /// The answer comes back through `PdfView::show_translation`.
    Translate(String, Option<String>),
    /// Read the selected text aloud (朗读): its 1-based page and the text.
    ReadAloud(usize, String),
    /// Reading metadata, saved by the owner before the viewer redraws it.
    Annotate { paper: String, resource: String, page: usize, kind: Kind, quote: String, boxes: Vec<[f32; 4]> },
    AnnotationNote { resource: String, id: String, note: String },
    /// A highlight or a box takes another colour, a key of `annotation::COLORS`; its range stays.
    RecolorAnnotation { resource: String, id: String, color: &'static str },
    RemoveAnnotation { resource: String, id: String },
    /// Save a copy of the file with its marks written in, for other readers (标注's 导出带标注的 PDF…).
    ExportMarked,
    /// A box drawn on a page, or a sticky note placed there (`rect` its spot twice) with its text.
    AddShape { paper: String, resource: String, page: usize, kind: Kind, rect: [f32; 4], note: String },
    /// A text box written on a page: its edges (grown to what it says), its words and colours.
    AddText { paper: String, resource: String, page: usize, rect: [f32; 4], text: String, style: TextStyle },
    /// A text box written again: its words (none, and it goes), and its edges when they grew with them.
    WriteText { resource: String, id: String, text: String, rect: Option<[f32; 4]> },
    /// A text box takes other colours.
    RestyleText { resource: String, id: String, style: TextStyle },
    /// A box moved or sized, a note moved, on its page.
    MoveShape { resource: String, id: String, rect: [f32; 4] },
    /// The number on the corner of a figure kept was clicked: its dialog, to change it or take it out.
    OpenFigure(String),
    /// The frame of the figure whose dialog is open was moved or sized: the piece it frames now, as a picture,
    /// with its 1-based page and box. It is the dialog's until the dialog is saved.
    Reframe { page: usize, bbox: [f32; 4], png: Vec<u8> },
    /// 固定对照 of a figure's preview: keep what stands `top` points down a 0-based page in the viewer's other
    /// half. The owner knows what that half holds now (another file of the paper) and answers with `keep_beside`.
    /// `file` is another PDF of the paper (its id) when the figure is in that one: it is shown in the other half.
    /// `mark` are the lines to mark there (a paragraph's words that name a figure), none for a figure.
    FigureBeside { file: Option<String>, page: usize, top: f32, mark: Vec<[f32; 4]> },
    /// 前往原位置 of a figure in another PDF of the paper: show that file in the half worked in, at this place
    /// of a 0-based page. `trail` are the places to come back to, the one just left last, each naming its file;
    /// the viewer that arrives is handed them (`PdfView::arrive`).
    FigureGo { file: String, page: usize, top: f32, mark: Vec<[f32; 4]>, trail: Vec<Return> },
    /// 返回刚才位置 to a place in another PDF of the paper: show that file again and `PdfView::restore` it
    /// there, with the places left before it.
    FigureReturn { back: Return, trail: Vec<Return> },
}

/// The names of the colours, in the order of `annotation::COLORS`.
const COLOR_NAMES: [&str; 7] = ["黄色", "绿色", "蓝色", "粉色", "紫色", "红色", "橙色"];

/// What the pointer does on a page besides selecting text and cutting pieces out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    /// Drag out a hollow box round a figure or a passage.
    Box,
    /// Click to put a sticky note there.
    Note,
    /// Drag out a text box (or click for one of `TEXT_NEW` size) and write in it on the page.
    Text,
}

/// The side of a sticky note's icon on screen, whatever the zoom.
const NOTE_ICON: f32 = 22.;
/// How near the pointer must be to a box's edge to take hold of it, in screen pixels whatever the zoom.
const EDGE_REACH: f32 = 5.;
/// A chosen rectangle's handles: the side of the square drawn, and how near the pointer must be to one to
/// take it, in screen pixels whatever the zoom, so they stay easy to hit on a page zoomed out.
const HANDLE: f32 = 9.;
const GRIP: f32 = 7.;
/// The note card's width.
const NOTE_W: f32 = 280.;
/// A text box's words, in the page's points so they grow and shrink with it: their size, the room between
/// lines (times the size), the room kept inside the box's edge; and a box made with a click instead of a drag.
const TEXT_PT: f32 = 11.;
const TEXT_LEADING: f32 = 1.4;
const TEXT_PAD: f32 = 4.;
const TEXT_NEW: (f32, f32) = (180., 40.);
/// The names of the text box's colours, by key.
fn text_color_name(key: &str) -> &'static str {
    match key {
        "none" => "无", "white" => "白色", "black" => "黑色", "gray" => "灰色", "yellow" => "黄色", "green" => "绿色",
        "blue" => "蓝色", "pink" => "粉色", "purple" => "紫色", "red" => "红色", _ => "橙色",
    }
}
fn text_part_name(part: TextPart) -> &'static str {
    match part { TextPart::Fill => "填充", TextPart::Border => "边框", TextPart::Ink => "文字" }
}

/// The text box being written: one already kept (`id`), or a new one not kept until something is written in it,
/// with its 0-based page and edges.
#[derive(Clone, Debug, PartialEq)]
struct TextEdit {
    id: Option<String>,
    page: usize,
    rect: [f32; 4],
}

/// A mark placed on a page (a box, a note, a piece marked to be cut out) or the figure dialog's frame, held
/// by the pointer: its id, 0-based page and place when it was taken, where it is held (`Grab::Body` moves the
/// whole of it, a handle sizes it), where on the page it was taken, and whether it has moved yet.
struct ShapeDrag {
    id: String,
    page: usize,
    rect: [f32; 4],
    grab: Grab,
    from: (f32, f32),
    moved: bool,
    /// It was chosen before this press: a text box clicked again is written in.
    again: bool,
}

impl ShapeDrag {
    /// The pointer held it to a point of its page, of `size` points: where it stands now; `None` before the
    /// pointer went far enough to be a drag. Sized, it keeps `handles::MIN_SIDE`; either way it stays on the page.
    fn follow(&mut self, x: f32, y: f32, scale: f32, size: (f32, f32)) -> Option<[f32; 4]> {
        let (dx, dy) = (x - self.from.0, y - self.from.1);
        if !self.moved && dx.abs().max(dy.abs()) * scale < 3. { return None }
        self.moved = true;
        Some(handles::drag(self.rect, self.grab, self.from, (x, y), size, handles::MIN_SIDE))
    }
}

/// A mark that is one rectangle on its page, so it can be chosen with a click and sized by its handles: a
/// box, a piece marked to be cut out, a text box. A new kind of the same sort is added here, and `shape_at`,
/// `rect_grips`, the menu and the drag layer serve it as they are.
fn sizable(kind: Kind) -> bool {
    matches!(kind, Kind::Box | Kind::Cut | Kind::Text)
}

/// The pointer's look over a part of a rectangle: a closed hand moving it, the arrows of the way a handle sizes it.
fn grab_cursor(grab: Grab) -> CursorStyle {
    match grab {
        Grab::Body => CursorStyle::ClosedHand,
        Grab::Handle(0, _) => CursorStyle::ResizeUpDown,
        Grab::Handle(_, 0) => CursorStyle::ResizeLeftRight,
        Grab::Handle(x, y) if x == y => CursorStyle::ResizeUpLeftDownRight,
        Grab::Handle(..) => CursorStyle::ResizeUpRightDownLeft,
    }
}

/// What a rectangle placed on a page shows the pointer, laid over it on a page drawn at `scale`: its edge
/// says it can be taken (a pointing hand, an open hand once chosen) and, chosen, its handles show
/// (`handles::handles`), each with the arrows of the way it sizes. It draws no outline of its own and takes
/// no press: the viewer's press finds what is under the pointer (`shape_at`, `frame_at`), with the same reaches.
/// A text box's inside is its own (`body`): a pointing hand says a click chooses it, and chosen, the text
/// cursor that a click there writes in it.
fn rect_grips(rect: [f32; 4], scale: f32, chosen: bool, body: bool) -> Div {
    let [x0, y0, x1, y1] = handles::normal(rect);
    let (w, h) = ((x1 - x0) * scale, (y1 - y0) * scale);
    let edge = if chosen { CursorStyle::OpenHand } else { CursorStyle::PointingHand };
    let inside = body.then(|| div().absolute().inset_0().cursor(if chosen { CursorStyle::IBeam } else { CursorStyle::PointingHand }));
    let strip = |left: f32, top: f32, width: f32, height: f32| div().absolute().left(px(left)).top(px(top)).w(px(width)).h(px(height)).cursor(edge);
    let r = EDGE_REACH;
    let strips = [strip(-r, -r, w + 2. * r, 2. * r), strip(-r, h - r, w + 2. * r, 2. * r), strip(-r, r, 2. * r, (h - 2. * r).max(0.)), strip(w - r, r, 2. * r, (h - 2. * r).max(0.))];
    let grips = chosen.then(|| handles::handles(rect, GRIP / scale).map(|handle| {
        let (hx, hy) = handles::handle_point(rect, handle);
        let (left, top) = ((hx - x0) * scale - HANDLE / 2., (hy - y0) * scale - HANDLE / 2.);
        div().absolute().left(px(left)).top(px(top)).size(px(HANDLE)).rounded(px(2.)).bg(Theme::handle_fill()).border_1().border_color(Theme::handle_ring()).shadow_sm().cursor(grab_cursor(Grab::Handle(handle.0, handle.1)))
    }).collect::<Vec<_>>());
    div().absolute().left(px(x0 * scale)).top(px(y0 * scale)).w(px(w)).h(px(h)).children(inside).children(strips).children(grips.into_iter().flatten())
}

/// A highlight that was clicked, and the menu beside it: the highlight's id, its 0-based page, and in that page's points
/// where the click was across and the top and bottom of the line clicked. A box chosen has the same menu (`PdfView::chosen`).
#[derive(Clone)]
struct MarkMenu {
    id: String,
    page: usize,
    x: f32,
    line: (f32, f32),
}

/// A piece cut out of a page, for the figure library.
pub struct Piece {
    pub png: Vec<u8>,
    /// The 1-based page it stands on, and its box there in the page's points.
    pub page: usize,
    pub bbox: [f32; 4],
    /// What the page says about it, when a caption stands by it.
    pub caption: Option<wisp_core::figure::Caption>,
}

/// A paragraph of a page with its translation, set in its place on the facing copy of the page.
pub struct FacingPiece {
    /// Where the paragraph stands: `[left, top, right, bottom]` in the page's points.
    pub rect: [f32; 4],
    /// The size the paragraph is set in.
    pub size: f32,
    /// The translation, once there is one; until then the page shows the paragraph itself.
    pub text: Option<SharedString>,
    /// Being asked for.
    pub busy: bool,
}

/// The colour a paragraph stands on, read off the page's picture just outside its box (left, right and
/// above; what two of the three agree on, else the first), and black or white to write on it.
fn ground(image: &RenderImage, page: (f32, f32), rect: [f32; 4]) -> Option<(Hsla, Hsla)> {
    let (size, bytes) = (image.size(0), image.as_bytes(0)?);
    let (width, height) = (i32::from(size.width) as f32, i32::from(size.height) as f32);
    let at = |x: f32, y: f32| {
        let (column, row) = ((x / page.0 * width).clamp(0., width - 1.) as usize, (y / page.1 * height).clamp(0., height - 1.) as usize);
        // BGRA, as the picture is handed to the window.
        bytes.get((row * width as usize + column) * 4..).and_then(|pixel| Some([*pixel.get(2)?, *pixel.get(1)?, *pixel.first()?]))
    };
    let [x0, y0, x1, y1] = rect;
    let seen = [at(x0 - 3., (y0 + y1) / 2.)?, at(x1 + 3., (y0 + y1) / 2.)?, at((x0 + x1) / 2., y0 - 3.)?];
    let near = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 6);
    let [r, g, b] = if near(seen[1], seen[2]) && !near(seen[0], seen[1]) { seen[1] } else { seen[0] };
    let light = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32 > 140.;
    Some((gpui::rgb((r as u32) << 16 | (g as u32) << 8 | b as u32).into(), if light { gpui::black() } else { gpui::white() }))
}

/// The translation of the selected text, on a card beside it. It lasts as long as that selection.
struct Translation {
    selection: Selection,
    text: String,
    /// The model service asked for this card alone, by id.
    profile: Option<String>,
    state: Translated,
    /// The model the text is translated with: its id (empty for the default one) and its name.
    model: (String, String),
    /// The list of models to translate it with instead is open.
    menu: bool,
}

/// The translation card's width and the height it may grow to, in logical pixels.
const CARD_W: f32 = 360.;
const CARD_TALL: f32 = 280.;
/// The room kept between the card and the viewer's edges, and the gap between it and the selection.
const CARD_EDGE: f32 = 8.;
const CARD_GAP: f32 = 6.;

/// Where the translation card stands in the viewer.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CardPlace {
    left: f32,
    width: f32,
    /// Its top edge, when it hangs under the selection; else its bottom edge, from the viewer's bottom, when it stands over it.
    top: Option<f32>,
    bottom: Option<f32>,
    /// How tall it may be there.
    max_height: f32,
}

/// The card for a selection whose lines cover `anchor` (left, top, right,
/// bottom in the viewer's pixels): under it when the room there takes a
/// card `tall` high or is the larger, else over it; its height held to the
/// room it has, and nothing of it past the viewer's edges, even for a
/// selection scrolled partly out of view.
fn card_place(anchor: [f32; 4], width: f32, tall: f32, (view_w, view_h): (f32, f32)) -> CardPlace {
    let width = width.min(view_w - 2. * CARD_EDGE).max(0.);
    let left = anchor[0].clamp(CARD_EDGE, (view_w - width - CARD_EDGE).max(CARD_EDGE));
    let (low, high) = (CARD_EDGE, (view_h - CARD_EDGE).max(CARD_EDGE));
    let under = (anchor[3] + CARD_GAP).clamp(low, high);
    let over = (anchor[1] - CARD_GAP).clamp(low, high);
    let (room_under, room_over) = (high - under, over - low);
    // A sliver of room is no place for it: past that it may cover the selection.
    let least = 80f32.min(high - low);
    if room_under >= tall || room_under >= room_over {
        let top = under.min(high - least);
        CardPlace { left, width, top: Some(top), bottom: None, max_height: (high - top).min(tall) }
    } else {
        let edge = over.max(low + least);
        CardPlace { left, width, top: None, bottom: Some(view_h - edge), max_height: (edge - low).min(tall) }
    }
}

/// Another PDF of the paper a viewer's file belongs to (#229, step 2): a number in the file on show may name
/// a figure in it, as the main text's "Supplementary Fig. S2" names one of the supplement.
#[derive(Clone, Debug, PartialEq)]
pub struct Sibling {
    /// The file's id.
    pub resource: String,
    /// As the reader knows it: 正文, 附件 S1.
    pub label: String,
    pub path: PathBuf,
    /// The paper's main text, else an attachment.
    pub main: bool,
}

/// A file read for its figures: where it shows them, where its text speaks of them, its pages' sizes.
type FileFigures = figref::Read;

/// The files read for their figures lately, by path: a paper's supplement is read once, not each time one
/// of its files is opened. A library's files do not change under their names.
static FILE_FIGURES: std::sync::Mutex<Vec<(PathBuf, Arc<FileFigures>)>> = std::sync::Mutex::new(Vec::new());
const KEEP_FILE_FIGURES: usize = 12;

/// Where a PDF shows its figures (`figref::file_index`), from what was read before or from the file.
fn file_figures(path: &Path) -> Option<Arc<FileFigures>> {
    if let Some(known) = FILE_FIGURES.lock().ok().and_then(|kept| kept.iter().find(|(of, _)| of == path).map(|(_, figures)| figures.clone())) { return Some(known) }
    let figures = Arc::new(figref::file_index(std::fs::read(path).ok()?, FIGURE_PAGES)?);
    if let Ok(mut kept) = FILE_FIGURES.lock() {
        kept.retain(|(of, _)| of != path);
        kept.push((path.to_path_buf(), figures.clone()));
        if kept.len() > KEEP_FILE_FIGURES { kept.remove(0); }
    }
    Some(figures)
}

/// A figure a number in the text may mean: the file it is in (`None` the one on show, else its place among
/// the viewer's `siblings`) and where that file shows it.
#[derive(Clone)]
struct Target {
    file: Option<usize>,
    place: figref::Place,
}

/// A paragraph that speaks of a figure, as the card of 相关文字 lists it: its file (`None` the one on show,
/// else its place among the viewer's `siblings`) and what `figref::speaking` found there.
#[derive(Clone)]
struct Said {
    file: Option<usize>,
    spoken: figref::Spoken,
}

/// The card of a figure's number clicked in the text (#229): the figures it may mean, the one on show
/// (`None` while they are listed to choose from), and the mention it stands by.
#[derive(Clone)]
struct FigureCard {
    targets: Vec<Target>,
    at: Option<usize>,
    /// 相关文字: in place of the figure on show, the paragraphs of the paper's files that speak of it, as
    /// they were found when this was asked for; and whether it was asked from the figure's preview (which
    /// is gone back to) or at the figure itself, by its caption's number.
    said: Option<Vec<Said>>,
    from_preview: bool,
    /// The 0-based page of the mention clicked, and the box of its words in that page's points.
    page: usize,
    anchor: [f32; 4],
    scroll: ScrollHandle,
}

/// The widest a figure's preview is, the tallest, and the largest it draws a page at, in pixels per point:
/// a figure of one column is not blown up to the card's full width.
const FIGURE_W: f32 = 620.;
const FIGURE_TALL: f32 = 640.;
const FIGURE_SCALE: f32 = 1.6;
/// The narrowest it is: its header and buttons need this much.
const FIGURE_W_MIN: f32 = 300.;
/// How wide the list of the figures a number may mean is, and the list of the paragraphs that speak of one.
const CHOICES_W: f32 = 420.;
const SAID_W: f32 = 460.;
/// How far under the top of the view a paragraph gone to stands, in points of its page.
const SAID_LEAD: f32 = 48.;
/// Pages read at a time for where the file shows its figures, and the pages of a long file that are read.
const FIGURE_CHUNK: usize = 4;
const FIGURE_PAGES: usize = 400;
/// Places 前往原位置 keeps to come back to.
const RETURNS: usize = 8;
/// Pages of the paper's other files kept as pictures for a figure's card.
const KEEP_FOREIGN: usize = 6;

/// A place left for a figure by 前往原位置: the half, the page at its top and how far down it in points
/// (`PdfView::place`), how far across in points, and the zoom. `from` is the file it is in when that is not
/// the one on show: its id, and what the reader calls it.
#[derive(Clone, Debug, PartialEq)]
pub struct Return {
    pane: usize,
    place: (usize, f32),
    across: f32,
    zoom: Zoom,
    pub from: Option<(String, String)>,
}

/// A page of a PDF as a picture, `scale` pixels to the point: its size in pixels and its bytes as GPUI wants
/// them (BGRA). For a page of a file no viewer has open.
fn page_picture(path: &Path, index: usize, scale: f32) -> Option<(u32, u32, Vec<u8>)> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
    let pdf = Pdf::new(std::fs::read(path).ok()?).ok()?;
    let pages = pdf.pages();
    let page = pages.get(index)?;
    let (cache, interpreter, settings) = (RenderCache::new(), InterpreterSettings::default(), RenderSettings::default());
    let pixmap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(page, &cache, &interpreter, &settings, &PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE }))).ok()?;
    let mut bytes = pixmap.data_as_u8_slice().to_vec();
    for pixel in bytes.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some((pixmap.width() as u32, pixmap.height() as u32, bytes))
}

/// A stretch of one page's glyphs, from where the pointer went down to where it is.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Selection {
    page: usize,
    anchor: usize,
    focus: usize,
}
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

/// Rendered pages kept in memory at most, and the bytes they may take together:
/// a page at reading width is about 18 MB, one enlarged four times close to 100.
const KEEP_PAGES: usize = 10;
const KEEP_BYTES: usize = 240 << 20;
/// The pixels of one rendered page at most. Past this an enlarged page is
/// drawn softer rather than larger.
const MAX_PAGE_PIXELS: f32 = 24_000_000.;
/// Device pixels per point of the first, quick picture of a page: a quarter of the work of one for a dense display.
const PREVIEW_SCALE: f32 = 1.5;
const PAGE_GAP: f32 = 12.;
/// The room left at either side of a page.
const MARGIN: f32 = 16.;
// The bar down the pages stands in the margin beside a page fitted to the width, and covers none of it.
const _: () = assert!(crate::scrollbar::STRIP <= MARGIN);
/// Logical pixels a PDF point takes at 100 %: a point is 1/72 inch, a pixel 1/96.
const POINT_PX: f32 = 96. / 72.;
const ZOOM_MIN: f32 = 25.;
const ZOOM_MAX: f32 = 500.;
/// The sizes the zoom buttons and keys step through, in percent.
const ZOOM_STEPS: [f32; 16] = [25., 33., 50., 67., 75., 90., 100., 110., 125., 150., 175., 200., 250., 300., 400., 500.];

/// How large the pages are drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    /// Each page as wide as the viewer.
    FitWidth,
    /// Each page whole in the viewer.
    FitPage,
    /// A fixed size, in percent of the page's own.
    Percent(f32),
}

impl Zoom {
    /// Whether the pages follow the viewer's size.
    pub fn fits(self) -> bool {
        !matches!(self, Zoom::Percent(_))
    }

    /// What the next paper opens at, this one being left at `self` and
    /// `kept` the fit before: a fit is how the reader reads and stays; a
    /// percentage was for a figure of this paper, and the fit comes back.
    pub fn carried(self, kept: Zoom) -> Zoom {
        if self.fits() { self } else { kept }
    }
}

/// How the views of the file stand: once, twice (each half scrolled on its own: a figure in one, the page
/// that speaks of it in the other) or in three columns. What each view shows is its own (`PdfView::shows`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Split {
    #[default]
    Off,
    /// Two halves side by side.
    Side,
    /// One half over the other.
    Stacked,
    /// Three columns: the pages, their translation, and the pages again.
    Triple,
}

impl Split {
    /// How many views of the pages it shows.
    fn views(self) -> usize {
        match self {
            Split::Off => 1,
            Split::Side | Split::Stacked => 2,
            Split::Triple => 3,
        }
    }
}

/// What a view of the reader shows: the pages, the pages with their paragraphs translated, or the paper's mind
/// map (`crate::mindmap`), which is no view of the pages and stands among them as a column of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Shows {
    #[default]
    Pages,
    Theirs,
    Map,
}

/// A view of the reader: what it shows, and whom it scrolls with. Views of one `group` are read as one, the
/// pages and their translation: the one last scrolled leads the others. A view of another group is scrolled
/// on its own: a figure, a method or the references kept in sight while the text is read on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct View {
    shows: Shows,
    group: u8,
}

/// The views on show, at most three, in the order they stand in.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shown {
    views: [View; 3],
    len: usize,
}

impl std::ops::Deref for Shown {
    type Target = [View];
    fn deref(&self) -> &[View] {
        &self.views[..self.len]
    }
}

/// The views of a viewer from what each shows, in the order they stand in. This is the one rule of what
/// scrolls together, and nobody sets it: a translation is read as one with the pages next to it (those
/// before it, else those after it) and scrolls in their group; pages scroll on their own, and so do a
/// translation with no pages beside it and the map. A group is named by the place of its pages.
fn views(shows: &[Shows]) -> Shown {
    let len = shows.len().min(3);
    let mut views = [View { shows: Shows::Pages, group: 0 }; 3];
    for at in 0..len {
        let pages = |n: usize| n < len && shows[n] == Shows::Pages;
        let group = if shows[at] != Shows::Theirs { at } else if at > 0 && pages(at - 1) { at - 1 } else if pages(at + 1) { at + 1 } else { at };
        views[at] = View { shows: shows[at], group: group as u8 };
    }
    Shown { views, len }
}

/// The view that goes when `shown` views are one too many: pages scrolled on their own, then a translation
/// scrolled on its own (of two alike, the one not worked in), then the map, before the pages and their
/// translation are parted. The paper itself stays: the last view that is not the map never goes for it.
fn spare(shown: &[View], active: usize) -> usize {
    let alone = |at: usize| shown.iter().filter(|view| view.group == shown[at].group).count() == 1;
    let last = |shows: Shows, but: Option<usize>| (0..shown.len()).rev().find(|&at| Some(at) != but && shown[at].shows == shows && alone(at));
    let paper = shown.iter().filter(|view| view.shows != Shows::Map).count();
    let of_paper = || last(Shows::Pages, Some(active)).or_else(|| last(Shows::Pages, None)).or_else(|| last(Shows::Theirs, Some(active))).or_else(|| last(Shows::Theirs, None));
    (if paper > 1 { of_paper() } else { None }).or_else(|| shown.iter().rposition(|view| view.shows == Shows::Map)).unwrap_or(shown.len().saturating_sub(1))
}

/// The band between the two halves, in logical pixels: wide enough to tell a page that both halves show from one page.
/// Two viewers of two files of a paper side by side have the same band between them.
pub const DIVIDER: f32 = 4.;

/// The size of a half of a viewer of this size; all of it while it is not split.
fn pane_size(split: Split, view: (f32, f32)) -> (f32, f32) {
    match split {
        Split::Off => view,
        Split::Side => (((view.0 - DIVIDER) / 2.).max(0.), view.1),
        Split::Stacked => (view.0, ((view.1 - DIVIDER) / 2.).max(0.)),
        Split::Triple => (((view.0 - 2. * DIVIDER) / 3.).max(0.), view.1),
    }
}

/// Where a view of a split viewer starts, from the viewer's top left corner.
fn pane_origin(split: Split, view: (f32, f32), pane: usize) -> (f32, f32) {
    let half = pane_size(split, view);
    match split {
        Split::Off => (0., 0.),
        Split::Side | Split::Triple => (pane as f32 * (half.0 + DIVIDER), 0.),
        Split::Stacked => (0., pane as f32 * (half.1 + DIVIDER)),
    }
}

/// How the viewer stands once another file of the paper is put in its other half: as its halves stand
/// now (two files, or one file twice), else side by side where a viewer `width` wide has room for two pages.
pub fn beside_split(two_files: Option<Split>, one_file: Split, width: f32) -> Split {
    match (two_files, one_file) {
        (Some(split), _) if split != Split::Off => split,
        (_, Split::Off) => if width >= crate::facing::FACING_MIN { Split::Side } else { Split::Stacked },
        (_, Split::Triple) => Split::Side,
        (_, split) => split,
    }
}

/// The least a half is given of the viewer while the band between two halves is dragged (分屏 自由), in logical pixels.
pub const HALF_MIN: f32 = 160.;

/// The first half's share of the room two halves have (the viewer less `DIVIDER`) once the band between them,
/// taken at `share`, has moved `moved` pixels: each half keeps `HALF_MIN`, or half the room when there is less.
pub fn dragged_share(share: f32, moved: f32, room: f32) -> f32 {
    if room <= 0. { return 0.5 }
    let least = (HALF_MIN / room).min(0.5);
    (share + moved / room).clamp(least, 1. - least)
}

/// The half of a split viewer a point of it is in, the point given from the viewer's top left corner.
fn pane_at(split: Split, view: (f32, f32), at: (f32, f32)) -> usize {
    let half = pane_size(split, view);
    match split {
        Split::Off => 0,
        Split::Side => (at.0 >= half.0 + DIVIDER) as usize,
        Split::Stacked => (at.1 >= half.1 + DIVIDER) as usize,
        Split::Triple => (at.0 >= half.0 + DIVIDER) as usize + (at.0 >= 2. * (half.0 + DIVIDER)) as usize,
    }
}

/// One view of the pages: where it is scrolled to.
struct Pane {
    list: ListState,
    /// How far the pages are moved to the left, when they are wider than the view.
    scroll_x: f32,
}

impl Pane {
    fn new() -> Self {
        Pane { list: ListState::new(0, ListAlignment::Top, px(600.)), scroll_x: 0. }
    }
}

/// A change of zoom asked for from the toolbar, the menu or the keyboard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZoomTo {
    In,
    Out,
    Set(Zoom),
}

/// Logical pixels per point for a page of this size (in points) in a viewer of this size.
fn page_scale(zoom: Zoom, page: (f32, f32), view: (f32, f32)) -> f32 {
    let fit_width = (view.0 - 2. * MARGIN).max(120.) / page.0.max(1.);
    match zoom {
        Zoom::FitWidth => fit_width,
        // Until the viewer has been laid out its height is not known: the width decides.
        Zoom::FitPage if view.1 > 0. => fit_width.min((view.1 - 2. * PAGE_GAP).max(120.) / page.1.max(1.)),
        Zoom::FitPage => fit_width,
        Zoom::Percent(percent) => percent.clamp(ZOOM_MIN, ZOOM_MAX) / 100. * POINT_PX,
    }
}

/// Device pixels per point a page is rendered at to be shown at `wanted`:
/// the next of a few steps, so that a small change of zoom keeps the
/// picture, within what one picture may take.
fn render_scale(page: (f32, f32), wanted: f32) -> f32 {
    let step = [2., 3., 4., 6., 8., 12., 16.].into_iter().find(|step| *step >= wanted - 0.01).unwrap_or(16.);
    // Pixmaps are at most 65535 px a side; huge posters render smaller.
    step.min((MAX_PAGE_PIXELS / (page.0 * page.1).max(1.)).sqrt()).min(8000. / page.0.max(page.1).max(1.))
}

/// Whether a picture rendered at `at` device pixels per point still serves
/// a page that wants `wanted` (its `render_scale`) and is shown at `shown`:
/// never softer, and up to half again sharper, so that a viewer made a
/// little narrower (a panel dragged, the window resized) does not render
/// its pages again, while one zoomed far out lets the large pictures go.
fn picture_serves(at: f32, wanted: f32, shown: f32) -> bool {
    at >= wanted - 0.01 && at <= wanted.max(shown * 1.5) + 0.01
}

/// The next size up or down from this one, in percent.
/// The bar that moves pages wider than their view sideways: how far its
/// track stands in from the view's edges, how tall the strip that hears the
/// pointer is, and the shortest its thumb gets.
const BAR_INSET: f32 = 8.;
const BAR_HEIGHT: f32 = 14.;
const BAR_THUMB_MIN: f32 = 36.;

/// The thumb of the sideways bar in a view `width` wide whose pages can go
/// `max_x` to the left: its length, and how far along the track it stands.
fn bar_thumb(width: f32, max_x: f32, scroll_x: f32) -> (f32, f32) {
    let track = (width - 2. * BAR_INSET).max(1.);
    let thumb = (track * width / (width + max_x)).max(BAR_THUMB_MIN).min(track);
    (thumb, (track - thumb) * (scroll_x / max_x.max(1.)).clamp(0., 1.))
}

fn zoom_step(percent: f32, up: bool) -> f32 {
    if up { ZOOM_STEPS.into_iter().find(|step| *step > percent + 0.5).unwrap_or(ZOOM_MAX) } else { ZOOM_STEPS.into_iter().rev().find(|step| *step < percent - 0.5).unwrap_or(ZOOM_MIN) }
}

/// The pages a quotation is looked for on before giving up.
const FIND_PAGES: usize = 300;
/// Pages whose glyphs the worker keeps once read, for the next search.
const KEEP_GLYPHS: usize = 120;
/// Room the find bar and the citation's notice leave at the right of the viewer, for 标注's button.
const FIND_CLEAR: f32 = 120.;
/// Words of a sentence not found tried at most, one after another, until one is on some page.
const SEARCH_TRIES: usize = 6;
/// Places the find bar shows at most.
const SEARCH_HITS: usize = 999;
/// Page renders kept from a burst of requests; older ones are handed back.
const KEEP_REQUESTS: usize = 3;

#[derive(Default, Debug, PartialEq)]
enum CitationStatus {
    #[default]
    None,
    PageOnly,
    Searching,
    Found,
    /// Only the beginning of the quotation is on the page.
    Partial,
    Missing,
}

/// The latest citation click, independent of when its PDF or search finishes.
#[derive(Default)]
struct CitationFocus {
    request: u64,
    page: Option<usize>,
    boxes: Vec<[f32; 4]>,
    status: CitationStatus,
    /// The sentence looked for: what 自己搜索 starts from when it was not found.
    quote: String,
}

impl CitationFocus {
    fn clear(&mut self) {
        *self = Self { request: self.request.wrapping_add(1), ..Self::default() };
    }

    fn begin(&mut self, page: usize, quote: &str) -> u64 {
        self.clear();
        self.page = Some(page.saturating_sub(1));
        self.quote = quote.to_string();
        self.status = if quote.trim().is_empty() { CitationStatus::PageOnly } else { CitationStatus::Searching };
        self.request
    }

    fn index(&self, count: usize) -> Option<usize> {
        Some(self.page?.min(count.checked_sub(1)?))
    }

    /// A late search must never replace a newer citation or move back to it.
    fn finish(&mut self, request: u64, found: Option<(usize, Vec<[f32; 4]>, bool)>) -> bool {
        if request != self.request || self.status != CitationStatus::Searching { return false }
        match found.filter(|(_, boxes, _)| !boxes.is_empty()) {
            Some((page, boxes, whole)) => {
                self.page = Some(page);
                self.boxes = boxes;
                self.status = if whole { CitationStatus::Found } else { CitationStatus::Partial };
            }
            None => self.status = CitationStatus::Missing,
        }
        true
    }
}

/// The find bar: words typed to look for in the whole file, the places found, and the one gone to.
struct Finder {
    input: Entity<TextInput>,
    _heard: Subscription,
    request: u64,
    hits: Vec<(usize, Vec<[f32; 4]>)>,
    at: Option<usize>,
    searching: bool,
    /// Opened because a citation's sentence was not found in the file: said under the bar.
    missed: bool,
    /// More words of that sentence to try while the one in the bar is on no page.
    tries: Vec<String>,
}

/// Words of a sentence the PDF did not have word for word, to look for instead, the likeliest first: its
/// longer words of letters (terms, names) by length, then pieces of four of its Chinese characters.
pub(crate) fn search_words(quote: &str) -> Vec<String> {
    let is_cjk = wisp_core::structure::is_cjk;
    let mut words: Vec<String> = vec![];
    for word in quote.split(|c: char| !c.is_alphanumeric() && c != '-').map(|w| w.trim_matches('-')).filter(|w| w.chars().count() >= 4 && !w.chars().any(is_cjk)) {
        if !words.iter().any(|known| known.eq_ignore_ascii_case(word)) { words.push(word.to_string()) }
    }
    // Longest first; of the same length, in the sentence's order.
    words.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    let letters: Vec<char> = quote.chars().collect();
    for run in letters.split(|c| !is_cjk(*c)).filter(|run| run.len() >= 2) {
        words.extend(run.chunks(4).filter(|piece| piece.len() >= 2).map(|piece| piece.iter().collect::<String>()));
    }
    words.truncate(SEARCH_TRIES);
    words
}

enum Request {
    /// Render a page at so many device pixels per point.
    Page(usize, f32),
    /// Look for a quoted sentence, starting at this 0-based page.
    Find(u64, Option<usize>, String),
    /// Look for words typed into the find bar, on every page.
    Search(u64, String),
    /// Look for the sentence being read aloud on its 0-based page (or the next one).
    Spoken(u64, usize, String),
    /// The glyphs of a page, for selecting text on it.
    Marks(usize),
    /// Where the file shows its figures: read the next `FIGURE_CHUNK` pages from this 0-based one.
    Figures(usize),
}

enum Reply {
    /// Page sizes in points, once, after the file parsed.
    Opened(Vec<(f32, f32)>),
    Failed(String),
    /// A rendered page: its index, the scale it was rendered at, its size in pixels and the pixels.
    Page(usize, f32, u32, u32, Vec<u8>),
    /// A page whose render was dropped for newer ones: ask again when it is in view.
    Skipped(usize),
    /// Where the quotation is: the page, one box per line in page points, and whether that is all of it.
    Found(u64, usize, Vec<[f32; 4]>, bool),
    NotFound(u64),
    /// Where the words of the find bar stand: each place's page and one box per line, in reading order.
    Hits(u64, Vec<(usize, Vec<[f32; 4]>)>),
    /// Where the sentence read aloud is: its page and one box per line, none when it was not found.
    Spoken(u64, usize, Vec<[f32; 4]>),
    Marks(usize, Vec<Mark>),
    /// What some pages show of figures (`figref::shown`, and `figref::unclaimed`), each with its 0-based page;
    /// and the page to read on from, none when the file is read through.
    /// With them, where those pages' text speaks of figures, each with its paragraph (`figref::spoken`).
    Figures(Vec<(usize, Vec<figref::Shown>, Option<[f32; 4]>)>, Vec<figref::Spoken>, Option<usize>),
}

/// Collects the glyphs of a page as it is interpreted: what each one says
/// and where it stands, in page points with the origin at the top left.
#[derive(Default)]
struct Glyphs {
    texts: Vec<String>,
    boxes: Vec<[f32; 4]>,
}

mod device {
    use super::Glyphs;
    use hayro::hayro_interpret::font::GlyphRun;
    use hayro::hayro_interpret::hayro_cmap::BfString;
    use hayro::hayro_interpret::{BlendMode, ClipPath, Device, DrawMode, DrawProps, Image, ImageDrawProps, SoftMask};
    use hayro::kurbo::{BezPath, Point};

    impl<'a> Device<'a> for Glyphs {
        fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
            for glyph in run.glyphs() {
                let text = match glyph.as_unicode() {
                    Some(BfString::Char(c)) => c.to_string(),
                    Some(BfString::String(text)) => text,
                    None => continue,
                };
                // Glyph space is a thousandth of the font size. The advance is not known
                // here: half an em from the origin, from below the baseline to the cap height.
                let place = props.transform * glyph.transform();
                let corners = [(0., -220.), (0., 780.), (500., -220.), (500., 780.)].map(|(x, y)| place * Point::new(x, y));
                let (xs, ys) = (corners.map(|p| p.x as f32), corners.map(|p| p.y as f32));
                self.texts.push(text);
                self.boxes.push([xs.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max), ys.iter().copied().fold(f32::MIN, f32::max)]);
            }
        }
        fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
        fn push_clip_path(&mut self, _: &ClipPath) {}
        fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
        fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
        fn pop_clip(&mut self) {}
        fn pop_transparency_group(&mut self) {}
    }
}

/// The pages of a PDF from `skip` on, at most `limit` of them, as PNG
/// pictures for a model to read a file that gives no text of its own, and
/// how many pages the file has. One and a half pixels per point: enough
/// to read print by, and small enough to send.
pub fn page_pictures(path: &Path, skip: usize, limit: usize) -> Result<(Vec<Vec<u8>>, usize), String> {
    pictures_of(path, |index| index >= skip, limit)
}

/// Pictures of these pages (from 0) of a PDF, in order: pages read again.
pub fn page_pictures_at(path: &Path, at: &[usize]) -> Result<Vec<Vec<u8>>, String> {
    let (pictures, _) = pictures_of(path, |index| at.contains(&index), at.len())?;
    if pictures.len() != at.len() { return Err(t("要读的页面不在这个 PDF 里。").into()) }
    Ok(pictures)
}

fn pictures_of(path: &Path, wanted: impl Fn(usize) -> bool, limit: usize) -> Result<(Vec<Vec<u8>>, usize), String> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
    let data = std::fs::read(path).map_err(|err| tf!("无法读取文件：{err}", "Cannot read file: {err}"))?;
    let pdf = Pdf::new(data).map_err(|err| tf!("无法解析这个 PDF（{err:?}）。", "Cannot parse this PDF ({err:?})."))?;
    let (cache, interpreter, settings) = (RenderCache::new(), InterpreterSettings::default(), RenderSettings::default());
    let mut out = vec![];
    let count = pdf.pages().len();
    for page in pdf.pages().iter().enumerate().filter(|(index, _)| wanted(*index)).map(|(_, page)| page).take(limit) {
        let (w, h) = page.render_dimensions();
        let scale = 1.5_f32.min(2000. / w.max(h).max(1.));
        let pixmap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(page, &cache, &interpreter, &settings, &PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE }))).map_err(|_| t("这个 PDF 的页面画不出来。").to_string())?;
        // On an opaque white background premultiplied colour is the colour itself.
        let picture = image::RgbaImage::from_raw(pixmap.width() as u32, pixmap.height() as u32, pixmap.data_as_u8_slice().to_vec()).ok_or(t("这个 PDF 的页面画不出来。"))?;
        let mut png = vec![];
        image::DynamicImage::ImageRgba8(picture).to_rgb8().write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|err| tf!("页面图像写不出来：{err}", "Cannot write page image: {err}"))?;
        out.push(png);
    }
    if out.is_empty() { return Err(t("这个 PDF 没有页面。").into()) }
    Ok((out, count))
}

fn worker(path: PathBuf, requests: mpsc::Receiver<Request>, replies: async_channel::Sender<Reply>) {
    use hayro::hayro_interpret::{Context, InterpreterCache, InterpreterSettings, TransformExt, interpret_page};
    use hayro::hayro_syntax::Pdf;
    use hayro::kurbo::Rect;
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(err) => {
            let _ = replies.send_blocking(Reply::Failed(tf!("无法读取文件：{err}", "Cannot read file: {err}")));
            return;
        }
    };
    let pdf = match Pdf::new(data) {
        Ok(pdf) => pdf,
        Err(err) => {
            let _ = replies.send_blocking(Reply::Failed(tf!("无法解析这个 PDF（{err:?}）。可以改用系统应用打开。", "Cannot parse this PDF ({err:?}). Try opening it with a system app.")));
            return;
        }
    };
    let pages = pdf.pages();
    if replies.send_blocking(Reply::Opened(pages.iter().map(|p| p.render_dimensions()).collect())).is_err() { return }
    let cache = RenderCache::new();
    let text_cache = InterpreterCache::new();
    let (interpreter, settings) = (InterpreterSettings::default(), RenderSettings::default());
    // The glyphs of pages read for a quotation or a search, kept for the next one (of a long file, the first pages read).
    let mut read: HashMap<usize, Arc<Glyphs>> = HashMap::new();
    // The part of the paper (a key of `structure::SECTIONS`) the pages read for their figures have come to.
    let mut section = String::new();
    let glyphs_of = |index: usize, read: &mut HashMap<usize, Arc<Glyphs>>| -> Option<Arc<Glyphs>> {
        if let Some(known) = read.get(&index) { return Some(known.clone()) }
        let page = pages.get(index)?;
        let (w, h) = page.render_dimensions();
        let glyphs = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut glyphs = Glyphs::default();
            let mut context = Context::new(page.initial_transform(true).to_kurbo(), Rect::new(0., 0., w as f64, h as f64), &text_cache, page.xref(), interpreter.clone());
            interpret_page(page, &mut context, &mut glyphs);
            glyphs
        }))
        .ok()
        .map(Arc::new)?;
        if read.len() < KEEP_GLYPHS { read.insert(index, glyphs.clone()); }
        Some(glyphs)
    };
    while let Ok(first) = requests.recv() {
        let mut waiting = vec![first];
        waiting.extend(requests.try_iter());
        // Of several quotations asked for in a row, only the last still matters.
        let find = waiting.iter().rev().find_map(|request| match request {
            Request::Find(request, page, quote) => Some((*request, *page, quote.clone())),
            Request::Page(..) | Request::Marks(_) | Request::Search(..) | Request::Spoken(..) | Request::Figures(_) => None,
        });
        // Reading aloud moves on a sentence at a time: only the latest is looked for.
        let spoken = waiting.iter().rev().find_map(|request| match request {
            Request::Spoken(request, page, sentence) => Some((*request, *page, sentence.clone())),
            _ => None,
        });
        let search = waiting.iter().rev().find_map(|request| match request {
            Request::Search(request, words) => Some((*request, words.clone())),
            _ => None,
        });
        // Scrolling queues pages faster than they render: the latest few are drawn,
        // newest first, and the view is told about the rest so it can ask again.
        let mut wanted: Vec<(usize, f32)> = vec![];
        for request in waiting.iter().rev() {
            if let Request::Page(index, scale) = request && !wanted.iter().any(|(known, _)| known == index) { wanted.push((*index, *scale)) }
        }
        for (skipped, _) in wanted.split_off(wanted.len().min(KEEP_REQUESTS)) {
            if replies.send_blocking(Reply::Skipped(skipped)).is_err() { return }
        }

        if let Some((request, start, quote)) = find {
            // The page named first, then its neighbours, then the rest in order.
            let mut order: Vec<usize> = start.into_iter().flat_map(|page| [Some(page), page.checked_sub(1), Some(page + 1)]).flatten().filter(|page| *page < pages.len()).collect();
            let rest: Vec<usize> = (0..pages.len().min(FIND_PAGES)).filter(|page| !order.contains(page)).collect();
            order.extend(rest);
            let mut reply = Reply::NotFound(request);
            for index in order {
                let Some(glyphs) = glyphs_of(index, &mut read) else { continue };
                // The whole quotation on a later page is better than its beginning on this one.
                if let Some((range, whole)) = wisp_core::fulltext::find_quote(&glyphs.texts, &glyphs.boxes, &quote) {
                    if whole || matches!(reply, Reply::NotFound(_)) { reply = Reply::Found(request, index, wisp_core::fulltext::line_boxes(&glyphs.boxes[range]), whole) }
                    if whole { break }
                }
            }
            if replies.send_blocking(reply).is_err() { return }
        }

        if let Some((request, page, sentence)) = spoken {
            // A sentence may run on to the next page; it is looked for there too.
            let found = [page, page + 1].into_iter().filter(|index| *index < pages.len()).find_map(|index| {
                let glyphs = glyphs_of(index, &mut read)?;
                wisp_core::fulltext::find_spoken(&glyphs.texts, &glyphs.boxes, &sentence).map(|range| (index, wisp_core::fulltext::line_boxes(&glyphs.boxes[range])))
            });
            let (index, boxes) = found.unwrap_or((page, vec![]));
            if replies.send_blocking(Reply::Spoken(request, index, boxes)).is_err() { return }
        }

        if let Some((request, words)) = search {
            let mut hits = vec![];
            for index in 0..pages.len().min(FIND_PAGES) {
                if hits.len() >= SEARCH_HITS { break }
                let Some(glyphs) = glyphs_of(index, &mut read) else { continue };
                hits.extend(wisp_core::fulltext::find_all(&glyphs.texts, &glyphs.boxes, &words).into_iter().map(|range| (index, wisp_core::fulltext::line_boxes(&glyphs.boxes[range]))));
            }
            hits.truncate(SEARCH_HITS);
            if replies.send_blocking(Reply::Hits(request, hits)).is_err() { return }
        }

        for (index, scale) in wanted {
            let Some(page) = pages.get(index) else { continue };
            let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(page, &cache, &interpreter, &settings, &PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE })));
            let reply = match rendered {
                Ok(pixmap) => {
                    let (width, height) = (pixmap.width() as u32, pixmap.height() as u32);
                    // Opaque white background, so premultiplied equals straight; GPUI wants BGRA.
                    let mut bytes = pixmap.data_as_u8_slice().to_vec();
                    for pixel in bytes.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                    Reply::Page(index, scale, width, height, bytes)
                }
                Err(_) => Reply::Page(index, scale, 0, 0, vec![]),
            };
            if replies.send_blocking(reply).is_err() { return }
        }

        // The glyphs of the pages asked for, once the pages themselves are drawn: a page is looked at before it is selected in.
        let mut marked: Vec<usize> = vec![];
        for request in &waiting {
            if let Request::Marks(index) = request && !marked.contains(index) { marked.push(*index) }
        }
        for index in marked {
            let Some(page) = pages.get(index) else { continue };
            if replies.send_blocking(Reply::Marks(index, pdftext::page_marks(page, &text_cache))).is_err() { return }
        }

        // Last, where the file shows its figures, a few pages at a time: pages asked for meanwhile are drawn in between.
        if let Some(from) = waiting.iter().filter_map(|request| if let Request::Figures(from) = request { Some(*from) } else { None }).min() {
            let last = pages.len().min(FIGURE_PAGES);
            let end = (from + FIGURE_CHUNK).min(last);
            // The part of the paper the text is in runs on from page to page: the pages are read in order.
            if from == 0 { section.clear() }
            let mut said = vec![];
            let read = (from..end).filter_map(|index| {
                let page = pages.get(index)?;
                let size = page.render_dimensions();
                let (marks, shapes) = pdftext::page_drawn(page, &text_cache);
                let shown = figref::shown(&marks, &shapes, size);
                let loose = figref::unclaimed(&shapes, &shown, size);
                said.extend(figref::spoken(index, &marks, &mut section));
                Some((index, shown, loose))
            }).collect();
            if replies.send_blocking(Reply::Figures(read, said, (end < last).then_some(end))).is_err() { return }
        }
    }
}

pub struct PdfView {
    sizes: Vec<(f32, f32)>,
    /// The rendered pages: the picture, the scale it was rendered at, the bytes it takes.
    images: HashMap<usize, (Arc<RenderImage>, f32, usize)>,
    /// Pages that failed to render.
    broken: HashSet<usize>,
    /// Most recently shown last.
    recent: VecDeque<usize>,
    pending: HashSet<usize>,
    /// The views of the pages: as many as `views` says are on show, the others kept for when they are.
    panes: [Pane; 3],
    split: Split,
    /// 译文: the second half shows the pages with their paragraphs translated in place, and the two halves
    /// scroll as one. By page (0-based), what stands in place of what (`Workspace::sync_facing`).
    facing: Option<HashMap<usize, Vec<FacingPiece>>>,
    /// The translation stands on the pages themselves, each paragraph in its own place (原地替换), instead of on
    /// a second copy of them in the other half: the viewer is not split for it.
    in_place: bool,
    /// What each view shows (`views` makes the views on show of the first of these, as many as the split has).
    shows: [Shows; 3],
    /// The paper's mind map, once its owner made one for this viewer (`set_map`): what a view that shows the
    /// map draws. With the owner's hearing of it, which goes when the viewer does.
    map: Option<(Entity<crate::mindmap::MindMapView>, Subscription)>,
    /// Whether the owner can give this viewer a map (the library's window).
    pub maps: bool,
    /// How two views stand once a third is put away: as they stood before it came.
    stood: Split,
    /// The view asked to show the translation while there was none (显示译文 of its label): it shows it when
    /// the translation comes, instead of a view more.
    wish: Option<usize>,
    /// The view whose label's menu is open.
    view_menu: Option<usize>,
    /// The view whose label is held, and whether it has changed places since: dragged over the view beside
    /// it, the two change places (`carry`).
    carrying: Option<(usize, bool)>,
    /// The label held was carried to another place and let go: the click that may follow is not one on it.
    carried: bool,
    /// Whether the owner can turn 译文 on for this viewer (the library's window; not a paper's own window).
    pub translates: bool,
    /// The translation's half was placed by the viewer's width, not chosen in 分屏: it goes beside the pages or
    /// under them as the width changes (`fit_facing`).
    facing_auto: bool,
    /// The half the pointer last worked in: the one the page number, the keys and a citation speak of.
    active: usize,
    /// By group of views (`View::group`), the view of it last worked in: the one the others of its group follow.
    led: [usize; 3],
    /// The half the selection, the piece cut out or the highlight clicked was made in: what is offered for it stands there.
    held: usize,
    requests: mpsc::Sender<Request>,
    citation: CitationFocus,
    error: Option<String>,
    /// The viewer's size in logical pixels; the height is known once it has been laid out.
    width: f32,
    height: f32,
    zoom: Zoom,
    /// How far the pages can be moved to the left, when they are wider than the view.
    max_x: f32,
    /// The sideways bar held by the pointer: its view, and how far from the thumb's left end it is held.
    sliding: Option<(usize, f32)>,
    /// Device pixels per logical pixel of the window the viewer is in.
    device_scale: f32,
    /// A page to jump to once the file has opened.
    goto: Option<usize>,
    /// A reading place to come back to once the file has opened (`place`).
    resume: Option<(usize, f32)>,
    /// The glyphs of the pages seen lately, and the pages they were asked for.
    marks: HashMap<usize, Arc<Vec<Mark>>>,
    marks_pending: HashSet<usize>,
    selection: Option<Selection>,
    /// The pointer is held down and drawing the selection out.
    selecting: bool,
    /// Whether the library's conversation is on show beside the paper's, to offer both for a quotation.
    pub library_chat: bool,
    /// The pointer cuts a piece out of a page instead of selecting its text.
    cutting: bool,
    /// The piece being cut or cut out: its page, and two corners in the page's points.
    cut: Option<(usize, [f32; 4])>,
    /// The translation asked for the selection.
    translation: Option<Translation>,
    /// The model services the card offers, by id and name.
    models: Vec<(String, String)>,
    paper_id: String,
    resource_id: String,
    annotations: Vec<Annotation>,
    annotations_open: bool,
    annotation_edit: Option<String>,
    annotation_note: Entity<TextInput>,
    /// The highlight the pointer went down on, and where in the window: a release close by is a click on it.
    pressed_mark: Option<(MarkMenu, Point<Pixels>)>,
    /// The menu of a highlight clicked: its colours and 删除.
    mark_menu: Option<MarkMenu>,
    /// The box or marked piece chosen with a click (`sizable`): its handles show and size it, its edge moves
    /// it, its menu stands under it, and Delete removes it. A press anywhere else in the pages lets it go, as Esc does.
    chosen: Option<String>,
    /// What the pointer does on a page; cutting is apart from it (`cutting`).
    tool: Tool,
    /// The box being drawn: its page, and two corners in the page's points.
    drawing: Option<(usize, [f32; 4])>,
    /// A box or note held by the pointer, and where it is shown meanwhile (until the owner saves it).
    shape_drag: Option<ShapeDrag>,
    shape_shown: Option<(String, [f32; 4])>,
    /// The sticky note whose card is open, and the one being written before it is kept: its page and spot.
    note_open: Option<String>,
    note_draft: Option<(usize, f32, f32)>,
    /// The open card's text is being written.
    note_editing: bool,
    note_input: Entity<TextInput>,
    /// The text box being written in, on the page itself (`text_input`); what a new one is drawn in (the
    /// colours last chosen for one); how tall each one's words stand, in points, by id ("" the new one), as
    /// last drawn: a box is never shorter than its words; the colours of the chosen one being picked.
    text_edit: Option<TextEdit>,
    text_input: Entity<TextInput>,
    text_style: TextStyle,
    text_tall: HashMap<String, f32>,
    text_palette: Option<(String, TextPart)>,
    /// The text box written in a moment ago and put away with Esc: chosen once it is kept and has an id.
    text_choose: Option<(usize, [f32; 4])>,
    /// Hears the text box's field lose the keyboard: what was written is kept then.
    text_blur: Option<Subscription>,
    focus: FocusHandle,
    finder: Option<Finder>,
    /// Counts the searches asked for, so a late answer to an older one is dropped.
    searches: u64,
    /// 朗读: the sentence being heard, marked on its page.
    spoken: Spoken,
    /// The figures of the figure library cut from this file, and the piece whose dialog is open (`set_figures`).
    figures: Vec<KeptFigure>,
    collecting: Option<Collecting>,
    /// The frame of `collecting` held by an edge (moved) or a corner (sized); its `id` is empty.
    frame_drag: Option<ShapeDrag>,
    /// Where the file shows its figures (#229), read in the background once its first page is drawn.
    figure_places: figref::Index,
    figures_asked: bool,
    /// Where the file's text speaks of figures, each mention with its paragraph: read with `figure_places`.
    figure_spoken: Vec<figref::Spoken>,
    /// Where each page's text speaks of a figure, with one box per line of the words: kept as `marks` is.
    mentions: HashMap<usize, Arc<Vec<(figref::Mention, Vec<[f32; 4]>)>>>,
    /// The mention the pointer went down on, and where in the window: a release close by is a click on it.
    pressed_figure: Option<(FigureCard, Point<Pixels>)>,
    /// The figure whose number was clicked, shown by the number.
    figure_card: Option<FigureCard>,
    /// The places left by 前往原位置, the latest last.
    returns: Vec<Return>,
    /// What the reader calls this file among the paper's PDFs (正文, 附件 S1), and whether it is the main
    /// text; the label is empty when the paper has no other PDF, and nothing need be said.
    label: String,
    main: bool,
    /// The paper's other PDFs, and where each shows its figures once it is read (`set_siblings`).
    siblings: Vec<(Sibling, Option<Arc<FileFigures>>)>,
    /// Pages of those files as pictures, for a figure's card: by sibling and 0-based page, the latest last.
    foreign: Vec<((usize, usize), Arc<RenderImage>)>,
    foreign_pending: HashSet<(usize, usize)>,
}

/// The piece of a page in the figure dialog on show: one just cut out (`figure` is `None`) or a figure kept,
/// opened again. Its frame has grips, and moved or sized it frames another piece (`PdfEvent::Reframe`).
#[derive(Clone, Debug, PartialEq)]
pub struct Collecting {
    pub figure: Option<String>,
    /// 1-based.
    pub page: usize,
    pub rect: [f32; 4],
}

/// A figure of the figure library as its page shows it: a dashed frame in `Theme::success`, so a piece kept is
/// told from one only marked (`Kind::Cut`, the accent).
#[derive(Clone, Debug, PartialEq)]
pub struct KeptFigure {
    pub id: String,
    /// 1-based, as the figure has it.
    pub page: usize,
    pub rect: [f32; 4],
    /// The number the paper gives it, on the frame's corner; empty when there is none.
    pub label: String,
}

/// The sentence read aloud as the page shows it: the one asked for (1-based page and sentence), and where
/// it was found (0-based page and one box per line).
#[derive(Default)]
struct Spoken {
    request: u64,
    asked: Option<(usize, String)>,
    page: usize,
    boxes: Vec<[f32; 4]>,
}

impl EventEmitter<PdfEvent> for PdfView {}

impl PdfView {
    pub fn new(path: &Path, zoom: Zoom, file: &wisp_core::model::PaperFile, annotations: Vec<Annotation>, cx: &mut Context<Self>) -> Self {
        // A new text box takes the colours of the one last changed in this file.
        let text_style = annotations.iter().filter(|a| a.kind == Kind::Text).max_by(|a, b| a.updated_at.cmp(&b.updated_at)).and_then(|a| a.style.clone()).unwrap_or_default();
        let text_input = cx.new(|cx| TextInput::new("输入文字", cx).multiline(1));
        cx.subscribe(&text_input, |this: &mut Self, _, event: &InputEvent, cx| if matches!(event, InputEvent::Cancel) { this.finish_text(true, cx) }).detach();
        let (requests, request_rx) = mpsc::channel();
        let (reply_tx, replies) = async_channel::unbounded();
        let worker_file = path.to_path_buf();
        std::thread::Builder::new().name("pdf".into()).spawn(move || worker(worker_file, request_rx, reply_tx)).ok();
        cx.spawn(async move |this, cx| {
            while let Ok(reply) = replies.recv().await {
                if this.update(cx, |this, cx| this.receive(reply, cx)).is_err() { break }
            }
        })
        .detach();
        // A page's picture stays in the window's atlas until it is let go of there: dropping the view alone would leave every page it drew.
        cx.on_release(|this: &mut Self, cx| {
            for (_, (image, ..)) in this.images.drain() { cx.drop_image(image, None) }
            for (_, image) in this.foreign.drain(..) { cx.drop_image(image, None) }
        })
        .detach();
        PdfView {
            sizes: vec![],
            images: HashMap::new(),
            broken: HashSet::new(),
            recent: VecDeque::new(),
            pending: HashSet::new(),
            panes: [Pane::new(), Pane::new(), Pane::new()],
            split: Split::Off,
            facing: None,
            in_place: false,
            shows: [Shows::Pages; 3],
            map: None,
            maps: false,
            stood: Split::Side,
            wish: None,
            view_menu: None,
            carrying: None,
            carried: false,
            translates: false,
            facing_auto: false,
            active: 0,
            led: [0, 1, 2],
            held: 0,
            requests,
            error: None,
            width: 0.,
            height: 0.,
            zoom,
            max_x: 0.,
            sliding: None,
            device_scale: 2.,
            goto: None,
            resume: None,
            citation: CitationFocus::default(),
            marks: HashMap::new(),
            marks_pending: HashSet::new(),
            selection: None,
            selecting: false,
            library_chat: false,
            cutting: false,
            cut: None,
            translation: None,
            models: vec![],
            paper_id: file.paper_id.clone(),
            resource_id: file.id.clone(),
            annotations,
            annotations_open: false,
            annotation_edit: None,
            annotation_note: cx.new(|cx| TextInput::new("写下这段原文的笔记…", cx).multiline(3)),
            pressed_mark: None,
            mark_menu: None,
            chosen: None,
            tool: Tool::Select,
            drawing: None,
            shape_drag: None,
            shape_shown: None,
            note_open: None,
            note_draft: None,
            note_editing: false,
            note_input: cx.new(|cx| TextInput::new("写下想法…", cx).multiline(4)),
            text_edit: None,
            text_input,
            text_style,
            text_tall: HashMap::new(),
            text_palette: None,
            text_choose: None,
            text_blur: None,
            focus: cx.focus_handle(),
            finder: None,
            searches: 0,
            spoken: Spoken::default(),
            figures: Vec::new(),
            collecting: None,
            frame_drag: None,
            figure_places: figref::Index::default(),
            figures_asked: false,
            figure_spoken: vec![],
            mentions: HashMap::new(),
            pressed_figure: None,
            figure_card: None,
            returns: vec![],
            label: String::new(),
            main: true,
            siblings: vec![],
            foreign: vec![],
            foreign_pending: HashSet::new(),
        }
    }

    /// The paper's other PDFs, with what this one is called among them: a figure's number here may name a
    /// figure there. Each is read for its figures in the background, once (`file_figures`).
    pub fn set_siblings(&mut self, label: String, main: bool, siblings: Vec<Sibling>, cx: &mut Context<Self>) {
        (self.label, self.main) = (label, main);
        for (_, image) in self.foreign.drain(..) { cx.drop_image(image, None) }
        self.foreign_pending.clear();
        self.siblings = siblings.into_iter().map(|sibling| (sibling, None)).collect();
        for (sibling, _) in &self.siblings {
            let (path, resource) = (sibling.path.clone(), sibling.resource.clone());
            cx.spawn(async move |this, cx| {
                let figures = cx.background_executor().spawn(async move { file_figures(&path) }).await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(slot) = this.siblings.iter_mut().find(|(sibling, _)| sibling.resource == resource) { slot.1 = figures }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// Draw boxes, place notes, write text boxes, or go back to selecting text; a second click on the same tool goes back too.
    pub fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.finish_text(false, cx);
        self.tool = if self.tool == tool { Tool::Select } else { tool };
        (self.cutting, self.cut, self.drawing, self.selection, self.selecting, self.translation, self.mark_menu, self.chosen) = (false, None, None, None, false, None, None, None);
        self.close_note();
        cx.notify();
    }

    /// Where a box or note stands now: moved by the pointer, or as kept; a text box as tall as its words at least.
    fn rect_of(&self, a: &Annotation) -> [f32; 4] {
        let rect = match &self.shape_shown {
            Some((id, rect)) if *id == a.id => *rect,
            _ => a.boxes.first().copied().unwrap_or_default(),
        };
        if a.kind == Kind::Text { self.grown(&a.id, rect) } else { rect }
    }

    /// A text box's edges with its bottom as low as its words reach, as they were last drawn (`text_tall`, by
    /// its id, "" for one not kept yet): a box is never shorter than what it says, as a slide's text box grows
    /// with its text. Sized narrower, its words wrap onto more lines and it grows taller.
    fn grown(&self, key: &str, rect: [f32; 4]) -> [f32; 4] {
        let mut rect = handles::normal(rect);
        if let Some(tall) = self.text_tall.get(key) { rect[3] = rect[3].max(rect[1] + tall) }
        rect
    }

    /// How tall a text box's words stood when drawn, in points.
    fn measured_text(&mut self, key: String, tall: f32, cx: &mut Context<Self>) {
        if self.text_tall.get(&key).is_none_or(|was| (was - tall).abs() > 0.25) {
            self.text_tall.insert(key, tall);
            cx.notify();
        }
    }

    /// Write in a text box on its page: a new one (`id` none, kept once something is written), or one there.
    fn start_text(&mut self, edit: TextEdit, window: &mut Window, cx: &mut Context<Self>) {
        let kept = edit.id.as_ref().and_then(|id| self.annotations.iter().find(|a| &a.id == id));
        let text = kept.map(|a| a.note.clone()).unwrap_or_default();
        let ink = kept.and_then(|a| a.style.as_ref()).unwrap_or(&self.text_style).ink.clone();
        self.text_input.update(cx, |input, cx| {
            input.set_paper_ink(Theme::text_ink(&ink));
            input.set_text(&text, cx);
        });
        (self.chosen, self.mark_menu, self.text_palette, self.selection, self.selecting) = (None, None, None, None, false);
        self.close_note();
        self.text_edit = Some(edit);
        // Whatever takes the keyboard from the field (a press elsewhere, a button of the bar) ends the writing, and what was written is kept.
        if self.text_blur.is_none() {
            let handle = self.text_input.focus_handle(cx);
            self.text_blur = Some(cx.on_blur(&handle, window, |this, window, cx| if !this.text_input.focus_handle(cx).is_focused(window) { this.finish_text(false, cx) }));
        }
        self.text_input.read(cx).focus(window);
        cx.notify();
    }

    /// The text box being written in is done with: a new one is kept if something was written, one there keeps
    /// its words anew (written empty, it goes), each as tall as its words. `choose`, it stays chosen (Esc), as a
    /// slide's text box does after it is written in.
    fn finish_text(&mut self, choose: bool, cx: &mut Context<Self>) {
        let Some(edit) = self.text_edit.take() else { return };
        let text = self.text_input.read(cx).text().trim().to_string();
        match edit.id {
            None => {
                let rect = self.grown("", edit.rect);
                self.text_tall.remove("");
                if !text.is_empty() {
                    if choose { self.text_choose = Some((edit.page, rect)) }
                    cx.emit(PdfEvent::AddText { paper: self.paper_id.clone(), resource: self.resource_id.clone(), page: edit.page + 1, rect, text, style: self.text_style.clone() });
                }
            }
            Some(id) => {
                let Some(a) = self.annotations.iter().find(|a| a.id == id) else { return cx.notify() };
                let (kept, rect) = (a.boxes.first().copied().unwrap_or_default(), self.rect_of(a));
                let grew = (!same_place(kept, rect)).then_some(rect);
                if text != a.note || grew.is_some() { cx.emit(PdfEvent::WriteText { resource: self.resource_id.clone(), id: id.clone(), text: text.clone(), rect: grew }) }
                self.chosen = (choose && !text.is_empty()).then_some(id);
            }
        }
        cx.notify();
    }

    /// The chosen text box takes another colour for one of its parts; a new one is drawn in these colours from now on.
    fn restyle_text(&mut self, part: TextPart, key: &str, cx: &mut Context<Self>) {
        let Some(a) = self.chosen_mark().filter(|a| a.kind == Kind::Text) else { return };
        let style = a.style.clone().unwrap_or_default().with(part, key);
        let (id, changed) = (a.id.clone(), a.style.as_ref() != Some(&style));
        self.text_style = style.clone();
        if changed { cx.emit(PdfEvent::RestyleText { resource: self.resource_id.clone(), id, style }) }
        cx.notify();
    }

    /// Write in the text box being written as if typed (a debug step): `\n` breaks the line.
    pub fn dev_write(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.text_edit.is_none() { return }
        let text = text.replace("\\n", "\n");
        self.text_input.update(cx, |input, cx| {
            let text = format!("{}{text}", input.text());
            input.set_text(&text, cx);
        });
    }

    /// A text box as its page draws it at `scale`: filled, edged and written in its colours, its words wrapped
    /// to its width (the field they are being written in, `writing`). What they take is measured as drawn
    /// (`measured_text`), so the box stands as tall as they are.
    fn text_box(&self, key: String, rect: [f32; 4], style: &TextStyle, text: Option<SharedString>, scale: f32, cx: &mut Context<Self>) -> Div {
        let [x0, y0, x1, y1] = handles::normal(rect);
        let border = (style.border != "none").then(|| (0.9 * scale).clamp(1., 3.));
        let edge = border.unwrap_or(0.);
        let (pad, width) = (TEXT_PAD * scale, ((x1 - x0) * scale - 2. * edge).max(2.));
        let view = cx.entity().downgrade();
        let measure = canvas(move |bounds, window, cx| {
            let tall = (f32::from(bounds.size.height) + 2. * edge) / scale;
            window.defer(cx, move |_, cx| { view.update(cx, |this, cx| this.measured_text(key, tall, cx)).ok(); });
        }, |_, _, _, _| {}).absolute().top_0().left_0().size_full();
        // The words' own width in pixels: a wrapping field under widths that are all percentages would be measured at 240.
        let inner = div().w(px((width - 2. * pad).max(1.)));
        let words = match text { Some(text) => inner.child(text), None => inner.child(self.text_input.clone()) };
        div().absolute().left(px(x0 * scale)).top(px(y0 * scale)).w(px((x1 - x0) * scale)).h(px((y1 - y0) * scale)).overflow_hidden().rounded(px(2.)).bg(Theme::text_fill(&style.fill))
            .when_some(border, |d, edge| d.border(px(edge)).border_color(Theme::text_border(&style.border)))
            .child(div().relative().w(px(width)).p(px(pad)).text_size(px(TEXT_PT * scale)).line_height(px(TEXT_PT * TEXT_LEADING * scale)).text_color(Theme::text_ink(&style.ink)).child(words).child(measure))
    }

    /// The mark placed on the page under a point of a 0-based page, and where it is held: the chosen one by a
    /// handle or its edge (`handles::grab_at`), before any other since its handles stand out of it; a note by
    /// its icon (`NOTE_ICON` screen pixels whatever the zoom); a box or a marked piece by its edge. Inside a box
    /// is the page's, to select its text. The last made is on top.
    fn shape_at(&self, page: usize, x: f32, y: f32) -> Option<(String, Grab)> {
        let scale = self.scale_of(page);
        let (reach, grip) = (EDGE_REACH / scale, GRIP / scale);
        if let Some(a) = self.chosen_mark().filter(|a| a.page == page + 1) && let Some(grab) = handles::grab_at(self.rect_of(a), (x, y), grip, reach) { return Some((a.id.clone(), grab)) }
        self.annotations.iter().rev().filter(|a| a.page == page + 1 && a.kind.shape()).find_map(|a| {
            let rect = self.rect_of(a);
            if a.kind == Kind::Note {
                let side = NOTE_ICON / scale;
                return (x >= rect[0] && x <= rect[0] + side && y >= rect[1] && y <= rect[1] + side).then(|| (a.id.clone(), Grab::Body));
            }
            // A text box is not hollow: what it holds is its own, so a press anywhere on it takes it.
            let within = a.kind == Kind::Text && x >= rect[0] && x <= rect[2] && y >= rect[1] && y <= rect[3];
            (within || handles::on_edge(rect, (x, y), reach)).then(|| (a.id.clone(), Grab::Body))
        })
    }

    /// The mark chosen, while it is there.
    fn chosen_mark(&self) -> Option<&Annotation> {
        let id = self.chosen.as_deref()?;
        self.annotations.iter().find(|a| a.id == id)
    }

    /// Put the note card away, and a note not yet kept with it.
    fn close_note(&mut self) {
        (self.note_open, self.note_draft, self.note_editing) = (None, None, false);
    }

    /// Open a note's card, to read or (`edit`) to write.
    fn open_note(&mut self, id: String, edit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.annotations.iter().find(|a| a.id == id).map(|a| a.note.clone()).unwrap_or_default();
        (self.note_open, self.note_draft, self.note_editing, self.mark_menu, self.chosen) = (Some(id), None, edit, None, None);
        self.note_input.update(cx, |input, cx| input.set_text(&text, cx));
        if edit { self.note_input.read(cx).focus(window) }
        cx.notify();
    }

    /// Keep what the open card says: a new note, or the text of one there.
    fn save_note(&mut self, cx: &mut Context<Self>) {
        let note = self.note_input.read(cx).text().trim().to_string();
        match (self.note_draft, self.note_open.clone()) {
            (Some((page, x, y)), _) => {
                // A note with nothing written is not kept.
                if note.is_empty() { return self.close_note() }
                cx.emit(PdfEvent::AddShape { paper: self.paper_id.clone(), resource: self.resource_id.clone(), page: page + 1, kind: Kind::Note, rect: [x, y, x, y], note });
            }
            (None, Some(id)) => cx.emit(PdfEvent::AnnotationNote { resource: self.resource_id.clone(), id, note }),
            (None, None) => {}
        }
        cx.notify();
    }

    /// The sticky note's card: beside the page where there is room, so it covers no text, else beside its icon.
    fn note_card(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (page, spot, text) = match (self.note_draft, &self.note_open) {
            (Some((page, x, y)), _) => (page, (x, y), String::new()),
            (None, Some(id)) => {
                let a = self.annotations.iter().find(|a| &a.id == id)?;
                let [x, y, ..] = self.rect_of(a);
                (a.page - 1, (x, y), a.note.clone())
            }
            (None, None) => return None,
        };
        let scale = self.scale_of(page);
        let (sheet_x, sheet_y) = self.sheet_place(page)?;
        let sheet_w = self.sizes.get(page)?.0 * scale;
        let (icon_x, icon_y) = (sheet_x + spot.0 * scale, sheet_y + spot.1 * scale);
        let room_right = self.width - (sheet_x + sheet_w);
        let left = if room_right >= NOTE_W + 16. {
            sheet_x + sheet_w + 8.
        } else if sheet_x >= NOTE_W + 16. {
            sheet_x - NOTE_W - 8.
        } else if icon_x + NOTE_ICON + 6. + NOTE_W <= self.width - 8. {
            icon_x + NOTE_ICON + 6.
        } else {
            icon_x - NOTE_W - 6.
        };
        let left = left.clamp(8., (self.width - NOTE_W - 8.).max(8.));
        let top = icon_y.clamp(8., (self.height - 200.).max(8.));
        let button = |id: &'static str, label: &'static str| div().id(id).flex().flex_none().items_center().h(text_px(24.)).px(px(8.)).rounded(px(6.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).child(label);
        let existing = self.note_open.clone().filter(|_| self.note_draft.is_none());
        let body = if self.note_editing {
            div().flex().flex_col().gap(px(8.))
                .child(ui::field(&self.note_input, window, cx))
                .child(div().flex().items_center().gap(px(4.))
                    .child(button("pdf-note-save", t("保存")).text_color(Theme::accent()).on_click(cx.listener(|this, _, _, cx| this.save_note(cx))))
                    .child(button("pdf-note-cancel", t("取消")).on_click(cx.listener(|this, _, _, cx| {
                        if this.note_draft.is_some() { this.close_note() } else { this.note_editing = false }
                        cx.notify();
                    }))))
        } else {
            let edit = existing.clone();
            div().flex().flex_col().gap(px(8.))
                .child(div().line_height(text_px(20.)).text_color(if text.is_empty() { Theme::text_placeholder() } else { Theme::text() }).child(if text.is_empty() { t("（空白便签）").to_string() } else { text }))
                .child(div().flex().items_center().gap(px(4.))
                    .child(button("pdf-note-edit", t("编辑")).text_color(Theme::accent()).on_click(cx.listener(move |this, _, window, cx| if let Some(id) = edit.clone() { this.open_note(id, true, window, cx) })))
                    .child(div().flex_1())
                    .when_some(existing.clone(), |d, id| d.child(button("pdf-note-remove", t("删除")).text_color(Theme::danger()).on_click(cx.listener(move |this, _, _, cx| {
                        this.close_note();
                        cx.emit(PdfEvent::RemoveAnnotation { resource: this.resource_id.clone(), id: id.clone() });
                        cx.notify();
                    })))))
        };
        Some(
            div()
                .id("pdf-note-card")
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(NOTE_W))
                .max_h(px((self.height - 16.).max(0.)))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(12.))
                .rounded(px(10.))
                .bg(Theme::bg_body())
                .border_1()
                .border_color(Theme::line_strong())
                .shadow(crate::theme::shadow_card())
                .text_size(text_px(13.))
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().flex().items_center().gap(px(6.)).text_size(text_px(12.)).text_color(Theme::text_tertiary())
                    .child(ui::icon("sticky-note", 14., Theme::warning()))
                    .child(div().flex_1().child(tf!("第 {} 页 · 便签", "Page {} · Note", page + 1)))
                    .child(ui::ghost_button("pdf-note-close", Some("x"), None).h(text_px(20.)).on_click(cx.listener(|this, _, _, cx| { this.close_note(); cx.notify(); }))))
                .child(body)
                .into_any_element(),
        )
    }

    pub fn page_count(&self) -> usize {
        self.sizes.len()
    }

    /// Called only after a successful database write. Failed writes leave the
    /// selection or draft intact, so the user can retry without losing text.
    pub fn annotations_saved(&mut self, annotations: Vec<Annotation>, cx: &mut Context<Self>) {
        self.annotations = annotations;
        self.selection = None;
        self.annotation_edit = None;
        self.mark_menu = None;
        (self.shape_drag, self.shape_shown, self.drawing) = (None, None, None);
        // A box sized or recoloured stays chosen, to go on with it; one removed goes. A text box just written and put away with Esc is chosen.
        if let Some((page, rect)) = self.text_choose.take() && let Some(a) = self.annotations.iter().rev().find(|a| a.kind == Kind::Text && a.page == page + 1 && a.boxes.first().is_some_and(|b| same_place(*b, rect))) { self.chosen = Some(a.id.clone()) }
        if self.chosen_mark().is_none() { self.chosen = None }
        if self.text_palette.as_ref().is_some_and(|(id, _)| self.chosen.as_ref() != Some(id)) { self.text_palette = None }
        // A note just kept, or one whose text was saved, stays open to read; one gone goes with its card.
        if self.note_open.as_ref().is_some_and(|id| !self.annotations.iter().any(|a| &a.id == id)) { self.note_open = None }
        (self.note_draft, self.note_editing) = (None, false);
        cx.notify();
    }

    pub fn annotate(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let Some((page, quote)) = self.selected() else { return };
        let Some(selection) = self.selection else { return };
        let Some(marks) = self.marks.get(&selection.page).and_then(|marks| marks.get(selection.anchor.min(selection.focus)..=selection.anchor.max(selection.focus))) else { return };
        let boxes = pdftext::mark_boxes(marks);
        if boxes.is_empty() { return }
        cx.emit(PdfEvent::Annotate { paper: self.paper_id.clone(), resource: self.resource_id.clone(), page, kind, quote, boxes });
    }

    /// The page at the top of the view, 1-based: of a split viewer, the half last worked in.
    pub fn current_page(&self) -> usize {
        self.list().logical_scroll_top().item_ix + 1
    }

    /// The pages as the half last worked in shows them.
    /// The find bar, with its words selected: those given, else what is selected on a page, else what was looked for last.
    /// For a citation not found, `words` is the first of `search_words` and the rest are tried after it when it finds nothing.
    pub fn open_finder(&mut self, words: Option<String>, missed: bool, window: &mut Window, cx: &mut Context<Self>) {
        let words = words.or_else(|| self.selected().map(|(_, text)| text).filter(|text| text.chars().count() <= 80 && !text.contains('\n')));
        if self.finder.is_none() {
            let input = cx.new(|cx| TextInput::new("在 PDF 里查找", cx));
            let heard = cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
                // The person's own words: no more of the sentence's are tried.
                InputEvent::Changed => {
                    if let Some(finder) = this.finder.as_mut() { finder.tries.clear() }
                    this.finder_search(cx)
                }
                InputEvent::Submit => this.finder_step(true, cx),
                _ => {}
            });
            self.finder = Some(Finder { input, _heard: heard, request: 0, hits: vec![], at: None, searching: false, missed, tries: vec![] });
        }
        let Some(finder) = self.finder.as_mut() else { return };
        finder.missed = missed;
        finder.tries = if missed { search_words(&self.citation.quote).into_iter().skip(1).collect() } else { vec![] };
        let input = finder.input.clone();
        let given = words.is_some();
        input.update(cx, |input, cx| {
            if let Some(words) = &words { input.set_text(words, cx) }
            let end = input.text().len();
            input.select_range(0..end, cx);
        });
        input.read(cx).focus(window);
        if given || finder.hits.is_empty() { self.finder_search(cx) }
        cx.notify();
    }

    fn close_finder(&mut self, window: &mut Window) {
        self.finder = None;
        window.focus(&self.focus);
    }

    /// Ask the worker for the words now in the bar.
    fn finder_search(&mut self, cx: &mut Context<Self>) {
        self.searches += 1;
        let request = self.searches;
        let Some(finder) = self.finder.as_mut() else { return };
        let words = finder.input.read(cx).text().to_string();
        (finder.request, finder.hits, finder.at) = (request, vec![], None);
        finder.searching = !wisp_core::fulltext::bare(&words).is_empty();
        if finder.searching { let _ = self.requests.send(Request::Search(request, words)); }
        cx.notify();
    }

    /// The next place found (true) or the one before; past the end, around to the beginning.
    fn finder_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(finder) = self.finder.as_ref() else { return };
        let count = finder.hits.len();
        if count == 0 { return }
        let next = match finder.at {
            Some(n) if forward => (n + 1) % count,
            Some(n) => (n + count - 1) % count,
            None => 0,
        };
        self.finder_go(next, cx);
    }

    /// Bring a place found below the find bar, and into the view sideways.
    fn finder_go(&mut self, n: usize, cx: &mut Context<Self>) {
        let Some(finder) = self.finder.as_mut() else { return };
        let Some((index, boxes)) = finder.hits.get(n).cloned() else { return };
        finder.at = Some(n);
        let below = if finder.missed { 160. } else { 110. };
        if let Some(first) = boxes.first() {
            let scale = self.scale_of(index);
            // Near the top of its page the place is brought down by showing the foot of the page before.
            let down = first[1] * scale - below;
            let top = match index.checked_sub(1).filter(|_| down < 0.) {
                Some(before) => ListOffset { item_ix: before, offset_in_item: px((self.sizes.get(before).map_or(842., |size| size.1) * self.scale_of(before) + PAGE_GAP + down).max(0.)) },
                None => ListOffset { item_ix: index, offset_in_item: px(down.max(0.)) },
            };
            self.list().scroll_to(top);
            if self.max_x > 0. { self.panes[self.active].scroll_x = (first[0] * scale - 40.).clamp(0., self.max_x) }
        }
        cx.notify();
    }

    /// The find bar at the top of the viewer: the words, how many places and which is gone to, the arrows and ×;
    /// opened for a citation not found, what to do about it.
    fn finder_bar(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let finder = self.finder.as_ref()?;
        let typed = !wisp_core::fulltext::bare(finder.input.read(cx).text()).is_empty();
        let (count, color) = match (typed, finder.searching, finder.hits.len(), finder.at) {
            (false, ..) => (String::new(), Theme::text_tertiary()),
            (true, true, ..) => (t("查找中…").to_string(), Theme::text_tertiary()),
            (true, false, 0, _) => (t("没有找到").to_string(), Theme::danger()),
            (true, false, n, Some(at)) => (format!("{}/{n}", at + 1), Theme::text_secondary()),
            (true, false, n, None) => (tf!("{n} 处", "{n} found"), Theme::text_secondary()),
        };
        let able = !finder.hits.is_empty();
        let arrow = |id: &'static str, icon: &str, tip: &'static str, forward: bool, cx: &mut Context<Self>| ui::icon_action(id, icon, Theme::text_secondary(), t(tip)).when(!able, |d| d.opacity(0.4).cursor_default()).when(able, |d| d.on_click(cx.listener(move |this, _, window, cx| { this.finder_step(forward, cx); if let Some(finder) = this.finder.as_ref() { finder.input.read(cx).focus(window) } })));
        // At the top left: the right corner has 标注's button and its panel.
        Some(
            div()
                .absolute()
                .top(px(8.))
                .left(px(12.))
                .w(px(340f32.min(self.width - FIND_CLEAR).max(200.)))
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(6.))
                .rounded(px(8.))
                .bg(Theme::bg_body())
                .border_1()
                .border_color(Theme::line())
                .shadow(crate::theme::shadow_card())
                .occlude()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(div().flex_1().min_w_0().child(ui::field(&finder.input, window, cx).py(px(4.))))
                        .child(div().flex_none().min_w(px(32.)).px(px(2.)).text_size(text_px(12.)).text_color(color).child(count))
                        .child(arrow("pdf-find-previous", "chevron-up", "上一处", false, cx))
                        .child(arrow("pdf-find-next", "chevron-down", "下一处（Enter）", true, cx))
                        .child(ui::icon_action("pdf-find-close", "x", Theme::text_tertiary(), t("关闭（Esc）")).on_click(cx.listener(|this, _, window, cx| { this.close_finder(window); cx.notify() }))),
                )
                .when(finder.missed, |d| d.child(div().px(px(4.)).text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_secondary()).child(t("引用的这句话没能在 PDF 里逐字对上：可能是改写、翻译，或排版把句子拆开了。已用句中的词查找；也可以换成别的词、数字或术语。")))),
        )
    }

    fn list(&self) -> &ListState {
        &self.panes[self.active].list
    }

    pub fn split(&self) -> Split {
        self.split
    }

    /// The views on show, in their order: what each shows and whom it scrolls with.
    fn shown(&self) -> Shown {
        views(&self.shows[..self.split.views()])
    }

    /// The view scrolled apart from the one worked in, where there is one: where something is kept in sight
    /// (固定对照) while the reading goes on. The map is no place for a page.
    fn apart(&self) -> Option<usize> {
        let shown = self.shown();
        (0..shown.len()).find(|pane| shown[*pane].shows != Shows::Map && shown[*pane].group != shown[self.active.min(shown.len() - 1)].group)
    }

    /// Whether 固定对照 has a view to keep something in: one scrolled on its own that is there, or one that can
    /// still come (not beside a translation, which has that place, nor as a fourth view). A figure of another
    /// file goes to a second viewer, which has the reader to itself.
    fn can_keep(&self, other_file: bool) -> bool {
        if other_file { return !self.facing_beside() && !self.map_shown() }
        self.apart().is_some() || (!self.facing_beside() && self.split.views() < 3)
    }

    /// The size of one view of the pages: the viewer's, or a half of it.
    fn pane_size(&self) -> (f32, f32) {
        pane_size(self.split, (self.width, self.height))
    }

    /// Show the file once, twice (side by side or one over the other) or in three columns. A view that comes
    /// starts where the first is; of those that go, `spare` says which.
    pub fn set_split(&mut self, split: Split, cx: &mut Context<Self>) {
        // The pages and their translation stay together: side by side or one over the other, as chosen.
        if self.facing_beside() && split == Split::Off { return }
        // A third view of the pages stands beside a translation only.
        if split == Split::Triple && !self.facing_beside() { return }
        self.stand(split, cx);
    }

    /// `set_split` without asking what is there: the views that go are taken out, the size of a view changes,
    /// and those that come start where the first is, showing the pages (the translation where every view of
    /// the paper shows it: 原地替换).
    fn stand(&mut self, split: Split, cx: &mut Context<Self>) {
        if split == self.split { return }
        let (was, now) = (self.split.views(), split.views());
        let of_paper = || self.shows[..was].iter().filter(|shows| **shows != Shows::Map);
        let theirs = of_paper().count() > 0 && of_paper().all(|shows| *shows == Shows::Theirs);
        for n in (now..was).rev() {
            let view = spare(&views(&self.shows[..n + 1]), self.active);
            self.take_out(view, n + 1);
        }
        self.resize((0., 0.), |this| this.split = split);
        for pane in was..now {
            let top = self.panes[0].list.logical_scroll_top();
            self.panes[pane].list.reset(self.sizes.len());
            self.panes[pane].list.scroll_to(top);
            self.panes[pane].scroll_x = self.panes[0].scroll_x;
            self.shows[pane] = if theirs { Shows::Theirs } else { Shows::Pages };
        }
        self.settle();
        cx.notify();
    }

    /// Of `count` views, one leaves the row: those after it close up, each with its place and what it shows.
    /// The size of a view is the caller's to change (`resize`).
    fn take_out(&mut self, view: usize, count: usize) {
        self.panes[view..count].rotate_left(1);
        self.shows[view..count].rotate_left(1);
        self.shows[count - 1] = Shows::Pages;
        let at = |pane: usize| if pane > view { pane - 1 } else if pane == view { view.min(count.saturating_sub(2)) } else { pane };
        (self.active, self.held) = (at(self.active), at(self.held));
    }

    /// A view comes into the row at `at`, the one made last (`stand` put it at the end): those from there on
    /// move one place on.
    fn bring_to(&mut self, at: usize) {
        let last = self.split.views() - 1;
        if at >= last { return }
        self.panes[at..=last].rotate_right(1);
        self.shows[at..=last].rotate_right(1);
        let moved = |pane: usize| if pane >= at && pane < last { pane + 1 } else { pane };
        (self.active, self.held) = (moved(self.active), moved(self.held));
    }

    /// After the views changed: what is said of a view speaks of one of the paper that is on show (the page
    /// number, the keys and a selection are never the map's), and each group is led by its pages.
    fn settle(&mut self) {
        let shown = self.shown();
        let paper = (0..shown.len()).find(|at| shown[*at].shows != Shows::Map).unwrap_or(0);
        let of_paper = |pane: usize| if shown.get(pane).is_some_and(|view| view.shows != Shows::Map) { pane } else { paper };
        (self.active, self.held) = (of_paper(self.active), of_paper(self.held));
        (self.led, self.view_menu) = ([0, 1, 2], None);
        if self.carrying.is_some_and(|(view, _)| view >= shown.len()) { self.carrying = None }
    }

    /// A view's label: its view is closed (关闭这一栏), as its words say; a translation closed is 译文 put
    /// away, when it was the only one.
    pub fn close_view(&mut self, view: usize, cx: &mut Context<Self>) {
        let (count, shown) = (self.split.views(), self.shown());
        if count < 2 || view >= count { return }
        // The paper itself stays: its last view is not closed for the map.
        if shown[view].shows != Shows::Map && shown.iter().filter(|other| other.shows != Shows::Map).count() < 2 { return }
        let had_map = self.map_shown();
        self.take_out(view, count);
        let split = if count == 3 { self.stood } else { Split::Off };
        self.resize((0., 0.), |this| this.split = split);
        self.settle();
        self.say_facing(cx);
        self.say_map(had_map, cx);
        cx.notify();
    }

    /// A view's label: the view changes places with the one beside it (移到左边 / 移到右边). A translation
    /// goes with the pages it then stands next to.
    pub fn move_view(&mut self, view: usize, to: usize, cx: &mut Context<Self>) {
        let count = self.split.views();
        if view >= count || to >= count || view == to { return }
        self.panes.swap(view, to);
        self.shows.swap(view, to);
        let at = |pane: usize| if pane == view { to } else if pane == to { view } else { pane };
        (self.active, self.held) = (at(self.active), at(self.held));
        self.settle();
        cx.notify();
    }

    /// A view's label held and the pointer moved: over another view, the view carried takes a place toward
    /// it, one at a time, so the row under the pointer is always what letting go leaves.
    fn carry(&mut self, position: Point<Pixels>, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let Some((view, _)) = self.carrying else { return };
        let at = (f32::from(position.x - bounds.origin.x), f32::from(position.y - bounds.origin.y));
        let over = pane_at(self.split, (f32::from(bounds.size.width), f32::from(bounds.size.height)), at).min(self.split.views() - 1);
        if over == view || view >= self.split.views() { return }
        let to = if over > view { view + 1 } else { view - 1 };
        self.move_view(view, to, cx);
        self.carrying = Some((to, true));
    }

    /// A view's label: the view shows the pages, their translation or the map (显示原文 / 显示译文 / 显示导图),
    /// as its words say. The first view asked for a translation turns 译文 on (the owner hears
    /// `PdfEvent::Facing` and starts it), and the last one to give it up turns it off; a map the viewer has
    /// not been given yet is asked of the owner (`PdfEvent::Map`), who calls this again with it.
    pub fn show_in_view(&mut self, view: usize, shows: Shows, cx: &mut Context<Self>) {
        let shown = self.shown();
        if view >= shown.len() { return }
        self.view_menu = None;
        match shows {
            Shows::Theirs if self.facing.is_none() => {
                self.wish = Some(view);
                cx.emit(PdfEvent::Facing(true));
                return cx.notify();
            }
            // One map, and never in place of the last view of the paper.
            Shows::Map if shown[view].shows != Shows::Map => {
                if shown.iter().any(|other| other.shows == Shows::Map) || shown.iter().filter(|other| other.shows != Shows::Map).count() < 2 { return cx.notify() }
                if self.map.is_none() {
                    cx.emit(PdfEvent::Map(Some(view)));
                    return cx.notify();
                }
            }
            _ => {}
        }
        let had_map = self.map_shown();
        self.shows[view] = shows;
        self.facing_auto = false;
        self.settle();
        self.say_facing(cx);
        self.say_map(had_map, cx);
        cx.notify();
    }

    /// The menu of a view's label, as a click on the label opens it.
    pub fn open_view_menu(&mut self, view: usize, cx: &mut Context<Self>) {
        self.view_menu = (view < self.split.views()).then_some(view);
        cx.notify();
    }

    /// No view shows the translation any more: 译文 is put away.
    fn say_facing(&mut self, cx: &mut Context<Self>) {
        if self.facing.is_some() && !self.shown().iter().any(|view| view.shows == Shows::Theirs) { cx.emit(PdfEvent::Facing(false)) }
    }

    /// The map was shown and no view shows it now: the owner hears it (`PdfEvent::MapGone`).
    fn say_map(&mut self, had: bool, cx: &mut Context<Self>) {
        if had && !self.map_shown() { cx.emit(PdfEvent::MapGone) }
    }

    /// The paper's mind map for this viewer, from its owner, who hears of what is done in it.
    pub fn set_map(&mut self, map: Entity<crate::mindmap::MindMapView>, heard: Subscription) {
        self.map = Some((map, heard));
    }

    pub fn map(&self) -> Option<&Entity<crate::mindmap::MindMapView>> {
        self.map.as_ref().map(|(map, _)| map)
    }

    /// Whether a view shows the map.
    pub fn map_shown(&self) -> bool {
        self.shown().iter().any(|view| view.shows == Shows::Map)
    }

    /// Whether the viewer has as many views as it can show: one more has nowhere to come.
    pub fn full(&self) -> bool {
        self.split.views() >= 3
    }

    /// 导图 on the reader's bar. What it gives depends on what is there, each as a person expects it:
    /// - A view shows the map: that view is closed.
    /// - One or two views: the map is a column more, at the right, and the views there stay as they are.
    /// - Three views: there is no room for a fourth. Nothing changes and `false` says so, for the owner to
    ///   tell the person (a view's own label can still be told to show the map).
    pub fn toggle_map(&mut self, cx: &mut Context<Self>) -> bool {
        let shown = self.shown();
        if let Some(view) = shown.iter().position(|view| view.shows == Shows::Map) {
            self.close_view(view, cx);
            return true;
        }
        if shown.len() >= 3 { return false }
        if self.map.is_none() {
            cx.emit(PdfEvent::Map(None));
            return true;
        }
        if shown.len() == 2 { self.stood = self.split }
        self.stand(if shown.len() == 2 { Split::Triple } else if self.width >= crate::facing::FACING_MIN { Split::Side } else { Split::Stacked }, cx);
        self.shows[shown.len()] = Shows::Map;
        // The map was put there by hand: the translation is no longer moved under the pages by the width.
        self.facing_auto = false;
        self.settle();
        cx.notify();
        true
    }

    pub fn zoom(&self) -> Zoom {
        self.zoom
    }

    /// The file split on itself put back together for two viewers of it (分屏 自由, #245): how the halves stood, where the
    /// half not worked in is read at (as `place` gives it) and whether it is the first, at the left or the top. The half
    /// worked in stays here, whole. `None` unless the viewer shows two views of the paper each scrolled on its own: not
    /// while it is whole, nor while the pages stand with their translation or with the map.
    pub fn part(&mut self, cx: &mut Context<Self>) -> Option<(Split, (usize, f32), bool)> {
        if self.split.views() != 2 || self.facing_beside() || self.map_shown() || self.sizes.is_empty() { return None }
        let (split, other) = (self.split, 1 - self.active.min(1));
        let place = self.pane_place(other);
        self.set_split(Split::Off, cx);
        Some((split, place, other == 0))
    }

    /// The file split on itself again from two viewers of it (分屏 固定): the other half at `place`, first (at the
    /// left or the top) when `first` says so; the half here is the one worked in.
    pub fn join(&mut self, split: Split, place: (usize, f32), first: bool, cx: &mut Context<Self>) {
        if split.views() != 2 || self.split != Split::Off || self.facing_beside() || self.sizes.is_empty() { return }
        self.set_split(split, cx);
        let index = place.0.min(self.sizes.len() - 1);
        self.panes[1].list.scroll_to(ListOffset { item_ix: index, offset_in_item: px(place.1 * self.scale_of(index)) });
        if first {
            self.panes.swap(0, 1);
            self.shows.swap(0, 1);
            (self.active, self.held) = (1, 1);
        }
        cx.notify();
    }

    /// Where a half is read at, as `place` gives it.
    fn pane_place(&self, pane: usize) -> (usize, f32) {
        let top = self.panes[pane].list.logical_scroll_top();
        (top.item_ix, f32::from(top.offset_in_item) / self.scale_of(top.item_ix).max(0.01))
    }

    /// Show the pages' translation (`Some`: the paragraphs of each page, 0-based), or put it away:
    /// `add_translation`, `drop_translation`.
    pub fn set_facing(&mut self, pieces: Option<HashMap<usize, Vec<FacingPiece>>>, cx: &mut Context<Self>) {
        match (self.facing.is_some(), pieces.is_some()) {
            (false, true) => {
                self.facing = pieces;
                self.add_translation(cx);
            }
            (true, false) => {
                self.facing = None;
                self.drop_translation(cx);
            }
            _ => self.facing = pieces,
        }
        cx.notify();
    }

    pub fn facing(&self) -> bool {
        self.facing.is_some()
    }

    /// Whether the translation is shown as a second copy of the pages, next to them: a view of it in the
    /// group of its pages.
    pub fn facing_beside(&self) -> bool {
        self.facing.is_some() && self.shown().iter().enumerate().any(|(at, view)| view.shows == Shows::Theirs && view.group as usize != at)
    }

    /// The translation on the pages themselves (原地替换), or on a second copy of them (对照): the reader's
    /// habit, which says how a translation comes. One on show changes over to it.
    pub fn set_in_place(&mut self, in_place: bool, cx: &mut Context<Self>) {
        if in_place == self.in_place { return }
        self.in_place = in_place;
        if self.facing.is_none() { return }
        self.drop_translation(cx);
        self.add_translation(cx);
        cx.notify();
    }

    /// The translation comes: what that gives depends on the views it meets, and each row is what a person
    /// expects there before clicking (docs/dev/design.md, 操作直觉 35), not one rule applied to all of them.
    /// - A view asked for it (显示译文 of its label): that view shows it. The person said which.
    /// - 原地替换 is the reader's habit: every view of the paper shows it, and none is added.
    /// - The pages once: the pages and their translation, side by side where there is room for two pages.
    /// - Two views already: three columns, the translation next to the first view of the pages. The other
    ///   (a second half kept on a figure, the map) was put where it is on purpose, so it stays as it is.
    /// - Three views already: there is nowhere for it to come, and the owner does not ask for it then
    ///   (`full`). Asked all the same, 译文 is put away again.
    fn add_translation(&mut self, cx: &mut Context<Self>) {
        let count = self.split.views();
        if let Some(view) = self.wish.take().filter(|view| *view < count) {
            self.shows[view] = Shows::Theirs;
            return self.settle();
        }
        if self.in_place {
            for shows in &mut self.shows[..count] { if *shows == Shows::Pages { *shows = Shows::Theirs } }
            return;
        }
        let Some(pages) = self.shows[..count].iter().position(|shows| *shows == Shows::Pages) else { return self.say_facing(cx) };
        if count >= 3 { return self.say_facing(cx) }
        self.facing_auto = count == 1;
        if count == 2 { self.stood = self.split }
        self.stand(if count == 2 { Split::Triple } else if self.width >= crate::facing::FACING_MIN { Split::Side } else { Split::Stacked }, cx);
        // The view that came stands right after its pages, where they are.
        self.bring_to(pages + 1);
        self.shows[pages + 1] = Shows::Theirs;
        self.settle();
    }

    /// The translation goes: again by what is there, each as a person expects it (`add_translation`).
    /// - A translation next to its pages: its view is taken out. Of three views the two others are left,
    ///   standing as two did before.
    /// - A translation standing alone (原地替换, or its pages were closed): the view shows the pages again.
    fn drop_translation(&mut self, cx: &mut Context<Self>) {
        self.wish = None;
        loop {
            let shown = self.shown();
            let Some(view) = (0..shown.len()).rev().find(|&at| shown[at].shows == Shows::Theirs && shown[at].group as usize != at) else { break };
            self.take_out(view, shown.len());
            let split = if shown.len() == 3 { self.stood } else { Split::Off };
            self.resize((0., 0.), |this| this.split = split);
        }
        for shows in &mut self.shows { if *shows == Shows::Theirs { *shows = Shows::Pages } }
        self.settle();
        cx.notify();
    }

    /// How large the page in view is drawn, in percent of its own size.
    pub fn percent(&self) -> f32 {
        self.scale_of(self.list().logical_scroll_top().item_ix) / POINT_PX * 100.
    }

    /// Go to a 1-based page and look for a sentence quoted from it; when it
    /// is found, there or on another page, it is highlighted and scrolled
    /// into view. A citation without a quotation marks the page itself.
    pub fn find(&mut self, page: usize, quote: &str, cx: &mut Context<Self>) {
        self.go_to(page, cx);
        let request = self.citation.begin(page, quote);
        if self.citation.status == CitationStatus::Searching {
            let _ = self.requests.send(Request::Find(request, page.checked_sub(1), quote.to_string()));
        }
    }

    /// 朗读: mark the sentence heard (a 1-based page and the sentence), or none. The view follows it while
    /// the sentence before was in view; a reader who scrolled away to look at something is left there.
    pub fn follow_spoken(&mut self, heard: Option<(usize, String)>, cx: &mut Context<Self>) {
        if self.spoken.asked == heard { return }
        self.spoken.request = self.spoken.request.wrapping_add(1);
        match heard {
            Some((page, sentence)) if page > 0 && !sentence.trim().is_empty() => {
                let _ = self.requests.send(Request::Spoken(self.spoken.request, page - 1, sentence.clone()));
                self.spoken.asked = Some((page, sentence));
            }
            _ => {
                self.spoken = Spoken { request: self.spoken.request, ..Spoken::default() };
                cx.notify();
            }
        }
    }

    /// Whether lines of a 0-based page (in its points) stand wholly in the view of the half worked in.
    fn in_view(&self, index: usize, lines: &[[f32; 4]]) -> bool {
        let (Some(first), Some(last)) = (lines.first(), lines.last()) else { return false };
        let top = self.list().logical_scroll_top();
        if index < top.item_ix || self.sizes.is_empty() { return false }
        let height = self.pane_size().1;
        let mut y = -f32::from(top.offset_in_item);
        for page in top.item_ix..index {
            y += self.sizes.get(page).map_or(842., |size| size.1) * self.scale_of(page) + PAGE_GAP;
            if y > height { return false }
        }
        let scale = self.scale_of(index);
        y + first[1] * scale >= 0. && y + last[3] * scale <= height
    }

    /// How many logical pixels a point of this page takes.
    fn scale_of(&self, index: usize) -> f32 {
        page_scale(self.zoom, self.sizes.get(index).copied().unwrap_or((595., 842.)), self.pane_size())
    }

    /// Where a page's left edge stands in its view: in the middle while
    /// the page fits, else moved by that view's sideways scroll.
    fn sheet_left(&self, pane: usize, sheet_width: f32) -> f32 {
        let width = self.pane_size().0;
        if sheet_width + 2. * MARGIN <= width { (width - sheet_width) / 2. } else { MARGIN - self.panes[pane].scroll_x }
    }

    /// Change what decides the size of the pages (the zoom, the viewer's
    /// size, the split), and keep in view what was: the point `anchor`
    /// pixels from the top left corner of the half last worked in stays
    /// under it, and the other half keeps its top.
    fn resize(&mut self, anchor: (f32, f32), change: impl FnOnce(&mut Self)) {
        let before = [0, 1, 2].map(|pane| {
            let top = self.panes[pane].list.logical_scroll_top();
            let scale = self.scale_of(top.item_ix);
            (top, scale, self.sheet_left(pane, self.sizes.get(top.item_ix).map_or(595., |size| size.0) * scale))
        });
        change(self);
        let widest = self.sizes.iter().enumerate().map(|(index, (width, _))| width * self.scale_of(index)).fold(0., f32::max);
        // A page fitted to the width overflows it by a rounding at most: less than half a pixel is none, and draws no sideways bar.
        let excess = widest + 2. * MARGIN - self.pane_size().0;
        self.max_x = if excess > 0.5 { excess } else { 0. };
        for (pane, (top, before, left)) in before.into_iter().enumerate() {
            let anchor = if pane == self.active { anchor } else { (anchor.0, 0.) };
            let page_height = self.sizes.get(top.item_ix).map_or(842., |size| size.1);
            let after = self.scale_of(top.item_ix);
            let ratio = after / before.max(0.001);
            // Sideways: the point of the page under the anchor comes back under it.
            self.panes[pane].scroll_x = (MARGIN - (anchor.0 - (anchor.0 - left) * ratio)).clamp(0., self.max_x);
            if (ratio - 1.).abs() < 0.0005 { continue }
            // Page heights follow the scale: measure the list again.
            let offset = ((f32::from(top.offset_in_item) + anchor.1) * ratio - anchor.1).clamp(0., (page_height * after - 1.).max(0.));
            self.panes[pane].list.reset(self.sizes.len());
            self.panes[pane].list.scroll_to(ListOffset { item_ix: top.item_ix, offset_in_item: px(offset) });
        }
    }

    /// Draw the pages at another size. `anchor` is the point of the half
    /// last worked in that stays where it is (the pointer's place); without one, the top.
    pub fn set_zoom(&mut self, zoom: Zoom, anchor: Option<(f32, f32)>, cx: &mut Context<Self>) {
        let zoom = match zoom {
            Zoom::Percent(percent) => Zoom::Percent(percent.clamp(ZOOM_MIN, ZOOM_MAX)),
            fit => fit,
        };
        if zoom == self.zoom && zoom != Zoom::FitPage { return }
        if zoom != self.zoom { self.resize(anchor.unwrap_or((self.pane_size().0 / 2., 0.)), |this| this.zoom = zoom) }
        // The whole page: the one each half is on comes to its top, so all four edges are in view; asked again, it comes back there.
        if zoom == Zoom::FitPage {
            for pane in &mut self.panes {
                let page = pane.list.logical_scroll_top().item_ix;
                pane.list.scroll_to(ListOffset { item_ix: page, offset_in_item: px(0.) });
            }
        }
        cx.notify();
    }

    pub fn zoom_to(&mut self, to: ZoomTo, cx: &mut Context<Self>) {
        let zoom = match to {
            ZoomTo::In => Zoom::Percent(zoom_step(self.percent(), true)),
            ZoomTo::Out => Zoom::Percent(zoom_step(self.percent(), false)),
            ZoomTo::Set(zoom) => zoom,
        };
        self.set_zoom(zoom, None, cx);
    }

    /// The wheel turned over the viewer, before the list of pages hears of
    /// it. With ⌘ or Ctrl it changes the zoom about the pointer; sideways
    /// (or with Shift) it moves pages wider than the viewer. Anything else
    /// is left to the list, which scrolls down the pages.
    fn wheel(&mut self, event: &ScrollWheelEvent, bounds: Bounds<Pixels>, cx: &mut Context<Self>) -> bool {
        let delta = event.delta.pixel_delta(px(20.));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        cx.emit(PdfEvent::Worked);
        // The half under the pointer is the one worked in from here on.
        let at = (f32::from(event.position.x - bounds.origin.x), f32::from(event.position.y - bounds.origin.y));
        let view = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let pane = pane_at(self.split, view, at);
        // Over the map, the wheel is the map's.
        if self.shown().get(pane).is_none_or(|view| view.shows == Shows::Map) { return false }
        if pane != self.active {
            self.active = pane;
            cx.notify();
        }
        if event.modifiers.secondary() || event.modifiers.control {
            if dy != 0. {
                let origin = pane_origin(self.split, view, pane);
                let anchor = (at.0 - origin.0, at.1 - origin.1);
                self.set_zoom(Zoom::Percent(self.percent() * (dy / 240.).exp()), Some(anchor), cx);
            }
            return true;
        }
        // A wheel that only turns one way moves sideways with Shift held.
        let sideways = if event.modifiers.shift && dx == 0. { dy } else { dx };
        if self.max_x <= 0. || sideways == 0. || (sideways.abs() <= dy.abs() && !event.modifiers.shift) { return false }
        self.panes[pane].scroll_x = (self.panes[pane].scroll_x - sideways).clamp(0., self.max_x);
        cx.notify();
        true
    }

    /// Scroll to a 1-based page.
    /// The pointer went down on a view's sideways bar: the thumb is held where it was pressed,
    /// and a press beside it brings its middle under the pointer first.
    fn bar_press(&mut self, pane: usize, x: Pixels, cx: &mut Context<Self>) {
        let (thumb, left) = bar_thumb(self.pane_size().0, self.max_x, self.panes[pane].scroll_x);
        let at = f32::from(x - self.panes[pane].list.viewport_bounds().left()) - BAR_INSET;
        self.sliding = Some((pane, if at >= left && at <= left + thumb { at - left } else { thumb / 2. }));
        cx.emit(PdfEvent::Worked);
        cx.emit(PdfEvent::Pressed);
        self.slide(x, cx);
    }

    /// The thumb of the bar held follows the pointer, at `x` in the window, and the pages go the other way.
    fn slide(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some((pane, grab)) = self.sliding else { return };
        let width = self.pane_size().0;
        let room = (width - 2. * BAR_INSET - bar_thumb(width, self.max_x, 0.).0).max(1.);
        let left = f32::from(x - self.panes[pane].list.viewport_bounds().left()) - BAR_INSET - grab;
        self.panes[pane].scroll_x = (left / room * self.max_x).clamp(0., self.max_x);
        cx.notify();
    }

    /// The bar at the foot of a view whose pages are wider than it: it shows that there is more
    /// to either side and how much, and its thumb is dragged to get there.
    fn sideways_bar(&self, pane: usize, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if self.max_x <= 0. { return None }
        let (thumb, left) = bar_thumb(self.pane_size().0, self.max_x, self.panes[pane].scroll_x);
        let held = self.sliding.is_some_and(|(held, _)| held == pane);
        Some(
            div().id(("pdf-sideways", pane)).group("pdf-sideways").absolute().left(px(BAR_INSET)).right(px(BAR_INSET)).bottom(px(2.)).h(px(BAR_HEIGHT)).cursor_default()
                // The pages under the bar hear nothing of the press: no selection starts there.
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.bar_press(pane, event.position.x, cx);
                }))
                .child(div().absolute().left(px(left)).top(px(3.)).w(px(thumb)).h(px(BAR_HEIGHT - 6.)).rounded(px((BAR_HEIGHT - 6.) / 2.)).bg(Theme::text_tertiary().opacity(if held { 1. } else { 0.55 })).group_hover("pdf-sideways", |s| s.bg(Theme::text_tertiary()))),
        )
    }

    pub fn go_to(&mut self, page: usize, cx: &mut Context<Self>) {
        self.citation.clear();
        self.scroll_to_page(page, cx);
    }

    fn scroll_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
        if self.sizes.is_empty() {
            self.goto = Some(page);
        } else {
            self.list().scroll_to(ListOffset { item_ix: page.clamp(1, self.sizes.len()) - 1, offset_in_item: px(0.) });
        }
        cx.notify();
    }

    /// Where the view is read at: the 0-based page at its top, and how far down that page in its points.
    /// Kept by the owner when another file of the paper is shown, and handed back by `resume`.
    pub fn place(&self) -> (usize, f32) {
        // Not open yet: where it is to open, not the top it shows meanwhile.
        if self.sizes.is_empty() && let Some(place) = self.resume { return place }
        let top = self.list().logical_scroll_top();
        (top.item_ix, f32::from(top.offset_in_item) / self.scale_of(top.item_ix).max(0.01))
    }

    /// Come back to a place `place` gave, now or once the file has opened.
    pub fn resume(&mut self, (index, down): (usize, f32), cx: &mut Context<Self>) {
        if self.sizes.is_empty() { return self.resume = Some((index, down)) }
        let index = index.min(self.sizes.len() - 1);
        self.list().scroll_to(ListOffset { item_ix: index, offset_in_item: px(down * self.scale_of(index)) });
        cx.notify();
    }

    /// The file on show.
    pub fn paper_id(&self) -> &str {
        &self.paper_id
    }

    pub fn resource_id(&self) -> &str {
        &self.resource_id
    }

    /// Bring a mark taken back or done again into the view (`rect` on a 1-based page, in its points), unless it is there already.
    pub fn reveal(&mut self, page: usize, rect: [f32; 4], cx: &mut Context<Self>) {
        if page == 0 || self.in_view(page - 1, &[rect]) { return }
        self.go_to_place(page, rect[1], cx);
    }

    /// Whether the file has marks a copy of it would carry (导出带标注的 PDF…).
    pub fn has_exported_marks(&self) -> bool {
        self.annotations.iter().any(|a| wisp_core::annotpdf::exported(a.kind))
    }

    /// Go to a place on a 1-based page: what stands `top` points below the page's top comes a little under the top of the view.
    pub fn go_to_place(&mut self, page: usize, top: f32, cx: &mut Context<Self>) {
        self.citation.clear();
        if self.sizes.is_empty() { return self.go_to(page, cx) }
        let index = page.clamp(1, self.sizes.len()) - 1;
        self.list().scroll_to(ListOffset { item_ix: index, offset_in_item: px((top * self.scale_of(index) - 48.).max(0.)) });
        cx.notify();
    }

    fn receive(&mut self, reply: Reply, cx: &mut Context<Self>) {
        match reply {
            Reply::Opened(sizes) => {
                self.sizes = sizes;
                for pane in &self.panes { pane.list.reset(self.sizes.len()) }
                self.resize((0., 0.), |_| {});
                if let Some(page) = self.goto.take() { self.scroll_to_page(page, cx) }
                if let Some(place) = self.resume.take() { self.resume(place, cx) }
            }
            Reply::Failed(error) => self.error = Some(error),
            Reply::Skipped(index) => {
                self.pending.remove(&index);
            }
            Reply::Found(request, index, boxes, whole) => {
                if !self.citation.finish(request, Some((index, boxes, whole))) { return }
                // Bring the first line of the quotation a little below the top of the view, and into it sideways.
                if let Some(first) = self.citation.boxes.first() {
                    let scale = self.scale_of(index);
                    self.list().scroll_to(ListOffset { item_ix: index, offset_in_item: px((first[1] * scale - 80.).max(0.)) });
                    if self.max_x > 0. { self.panes[self.active].scroll_x = (first[0] * scale - 40.).clamp(0., self.max_x) }
                }
            }
            Reply::NotFound(request) => { self.citation.finish(request, None); }
            Reply::Spoken(request, index, boxes) => {
                if request != self.spoken.request { return }
                // Followed while what was heard before was in view (or nothing was): the sentence comes to a third of the way down.
                let follow = self.spoken.boxes.is_empty() || self.in_view(self.spoken.page, &self.spoken.boxes);
                if follow && !boxes.is_empty() && !self.in_view(index, &boxes) {
                    let scale = self.scale_of(index);
                    let down = (boxes[0][1] * scale - self.pane_size().1 / 3.).max(0.);
                    self.list().scroll_to(ListOffset { item_ix: index, offset_in_item: px(down) });
                }
                // Not found on the page: the last sentence found stays marked until the next one is.
                if !boxes.is_empty() { (self.spoken.page, self.spoken.boxes) = (index, boxes) }
            }
            Reply::Hits(request, hits) => {
                let Some(finder) = self.finder.as_mut().filter(|finder| finder.request == request) else { return };
                // A word of a sentence not found that is on no page either: the next one.
                if hits.is_empty() && !finder.tries.is_empty() {
                    let next = finder.tries.remove(0);
                    let input = finder.input.clone();
                    input.update(cx, |input, cx| {
                        input.set_text(&next, cx);
                        input.select_range(0..next.len(), cx);
                    });
                    return self.finder_search(cx);
                }
                (finder.hits, finder.searching, finder.at) = (hits, false, None);
                // The first place from the page in view on.
                let top = self.list().logical_scroll_top().item_ix;
                let first = self.finder.as_ref().and_then(|finder| finder.hits.iter().position(|(page, _)| *page >= top).or((!finder.hits.is_empty()).then_some(0)));
                if let Some(n) = first { self.finder_go(n, cx) }
            }
            Reply::Marks(index, marks) => {
                self.marks_pending.remove(&index);
                // Of a long paper, the glyphs of the pages read last are enough.
                if self.marks.len() > 40 {
                    let keep = self.selection.map(|selection| selection.page);
                    self.marks.retain(|page, _| Some(*page) == keep || page.abs_diff(index) <= 4);
                    self.mentions.retain(|page, _| self.marks.contains_key(page));
                }
                let mentions = figref::mentions(&marks).into_iter().filter_map(|mention| Some((mention, pdftext::mark_boxes(marks.get(mention.from..=mention.to)?)))).collect();
                self.mentions.insert(index, Arc::new(mentions));
                self.marks.insert(index, Arc::new(marks));
            }
            Reply::Figures(read, said, next) => {
                self.figure_spoken.extend(said);
                for (index, shown, loose) in read {
                    let height = self.sizes.get(index).map_or(842., |size| size.1);
                    self.figure_places.add(index, shown, loose, height);
                }
                match next {
                    Some(next) => { let _ = self.requests.send(Request::Figures(next)); }
                    // Read through: the legends that stand apart from their figures find them.
                    None => self.figure_places.settle(),
                }
            }
            Reply::Page(index, scale, width, height, bytes) => {
                self.pending.remove(&index);
                // The first page is drawn: now the file is read for its figures, behind what is looked at.
                if !std::mem::replace(&mut self.figures_asked, true) { let _ = self.requests.send(Request::Figures(0)); }
                match image::RgbaImage::from_raw(width, height, bytes).filter(|_| width > 0) {
                    Some(buffer) => {
                        let size = width as usize * height as usize * 4;
                        // A page rendered again at another size takes the place of the one it had.
                        if let Some((old, ..)) = self.images.insert(index, (Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])), scale, size)) { cx.drop_image(old, None) }
                        self.touch(index);
                        // Views scrolled apart show other pages: each group of them more to keep.
                        let shown = self.shown();
                        let keep = KEEP_PAGES + 6 * (0..shown.len()).filter(|&at| at > 0 && shown[at].group as usize == at).count();
                        while self.recent.len() > keep || (self.recent.len() > 2 && self.images.values().map(|(.., size)| size).sum::<usize>() > KEEP_BYTES) {
                            if let Some(old) = self.recent.pop_front() && let Some((image, ..)) = self.images.remove(&old) {
                                cx.drop_image(image, None);
                            }
                        }
                    }
                    None => {
                        self.broken.insert(index);
                    }
                }
            }
        }
        cx.notify();
    }

    fn touch(&mut self, index: usize) {
        self.recent.retain(|i| *i != index);
        self.recent.push_back(index);
    }

    /// A page as one of the views draws it. The two share its picture and what is marked on it.
    fn page(&mut self, pane: usize, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let (w, h) = self.sizes.get(index).copied().unwrap_or((595., 842.));
        let scale = self.scale_of(index);
        let (width, height) = (w * scale, h * scale);
        // The picture this size wants. One made for another size is shown, stretched, until its own arrives.
        let wanted = render_scale((w, h), scale * self.device_scale);
        let image = self.images.get(&index).map(|(image, at, _)| (image.clone(), *at));
        let stale = image.as_ref().is_none_or(|(_, at)| !picture_serves(*at, wanted, scale * self.device_scale));
        if image.is_some() { self.touch(index) }
        if stale && !self.broken.contains(&index) && self.pending.insert(index) {
            // A page not drawn yet comes quickly and coarse first, then at the size it is shown at.
            let _ = self.requests.send(Request::Page(index, if image.is_none() { wanted.min(PREVIEW_SCALE) } else { wanted }));
        }
        if !self.marks.contains_key(&index) && self.marks_pending.insert(index) {
            let _ = self.requests.send(Request::Marks(index));
        }
        // What is selected on this page, under the quotation's marker.
        let selected: Vec<Div> = self
            .selection
            .filter(|selection| selection.page == index)
            .and_then(|selection| Some(pdftext::mark_boxes(self.marks.get(&index)?.get(selection.anchor.min(selection.focus)..=selection.anchor.max(selection.focus))?)))
            .unwrap_or_default()
            .into_iter()
            .map(|line| div().absolute().left(px(line[0] * scale)).top(px(line[1] * scale)).w(px((line[2] - line[0]) * scale)).h(px((line[3] - line[1]) * scale)).bg(Theme::selection()))
            .collect();
        // The piece being cut out of this page.
        let cut = self.cut.filter(|(page, _)| *page == index).map(|(_, [x0, y0, x1, y1])| div().absolute().left(px(x0.min(x1) * scale)).top(px(y0.min(y1) * scale)).w(px((x1 - x0).abs() * scale)).h(px((y1 - y0).abs() * scale)).border_2().border_color(Theme::accent()).bg(Theme::selection()));
        let cutting = self.cutting;
        // Metadata is painted over the current raster at the same scale as
        // the glyphs, independently of both selection and citation marks.
        // A highlight can be clicked for its menu, so the pointer says so over it.
        let annotations: Vec<Div> = self.annotations.iter().filter(|a| a.page == index + 1 && !a.kind.shape()).flat_map(|a| a.boxes.iter().map(move |line| {
            let (top, height, color) = match a.kind {
                Kind::Underline => (line[3] * scale - (1.3 * scale).max(1.), (1.3 * scale).max(1.), Theme::underline_mark()),
                _ => (line[1] * scale, (line[3] - line[1]) * scale, Theme::highlight(&a.color)),
            };
            div().absolute().left(px(line[0] * scale)).top(px(top)).w(px((line[2] - line[0]) * scale)).h(px(height)).bg(color).when(a.kind == Kind::Highlight && !cutting, |d| d.cursor_pointer())
        })).collect();
        // Boxes, hollow so what they frame stays clear. Over each, while the pointer selects, what its edge and
        // (the one chosen) its eight handles do (`rect_grips`), drawn after every mark so a handle is on top.
        let chosen = self.chosen.as_deref();
        let free = self.tool == Tool::Select && !cutting;
        // A piece marked to be cut out later is a dashed frame in the accent, and says it can be clicked; one kept
        // in the figure library since is in the colour of a figure kept.
        // The text box being written in is drawn apart, with its field, and has no handles meanwhile.
        let writing = self.text_edit.clone().filter(|edit| edit.page == index);
        let written = writing.as_ref().and_then(|edit| edit.id.clone());
        let rects: Vec<&Annotation> = self.annotations.iter().filter(|a| a.page == index + 1 && sizable(a.kind) && written.as_ref() != Some(&a.id)).collect();
        let shapes: Vec<Div> = rects.iter().filter(|a| a.kind != Kind::Text).map(|a| {
            let [x0, y0, x1, y1] = self.rect_of(a);
            let marked = a.kind == Kind::Cut;
            let color = if !marked { Theme::highlight_swatch(&a.color) } else if self.kept_at(index, [x0, y0, x1, y1]) { Theme::success() } else { Theme::accent() };
            let picked = chosen == Some(a.id.as_str());
            div().absolute().left(px(x0 * scale - 1.)).top(px(y0 * scale - 1.)).w(px((x1 - x0) * scale + 2.)).h(px((y1 - y0) * scale + 2.)).border_2().border_color(color).rounded(px(2.))
                .when(marked, |d| d.border_dashed().when(self.tool == Tool::Select, |d| d.cursor_pointer()).when(picked, |d| d.bg(Theme::selection())))
        }).collect();
        // The chosen one's last, so its handles stand over a box that overlaps it.
        let mut grips: Vec<Div> = if free { rects.iter().filter(|a| chosen != Some(a.id.as_str())).map(|a| rect_grips(self.rect_of(a), scale, false, a.kind == Kind::Text)).collect() } else { vec![] };
        if free && let Some(a) = rects.iter().find(|a| chosen == Some(a.id.as_str())) { grips.push(rect_grips(self.rect_of(a), scale, true, a.kind == Kind::Text)) }
        // Text boxes, their words on the page in their own colours, the last made on top; the one being written
        // holds its field, ringed in the accent, and a press on it is the field's (to place the caret, select words).
        let texts: Vec<Div> = rects.iter().filter(|a| a.kind == Kind::Text).map(|a| self.text_box(a.id.clone(), self.rect_of(a), &a.style.clone().unwrap_or_default(), Some(a.note.clone().into()), scale, cx)).collect();
        let writing = writing.and_then(|edit| {
            let (key, rect, style) = match &edit.id {
                Some(id) => {
                    let a = self.annotations.iter().find(|a| &a.id == id)?;
                    (id.clone(), self.rect_of(a), a.style.clone().unwrap_or_default())
                }
                None => (String::new(), self.grown("", edit.rect), self.text_style.clone()),
            };
            let [x0, y0, x1, y1] = rect;
            let ring = div().absolute().left(px(x0 * scale - 3.)).top(px(y0 * scale - 3.)).w(px((x1 - x0) * scale + 6.)).h(px((y1 - y0) * scale + 6.)).border_1().border_dashed().border_color(Theme::accent()).rounded(px(3.));
            Some(div().absolute().inset_0().child(ring).child(self.text_box(key, rect, &style, None, scale, cx).cursor_text().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())))
        });
        // The figures kept from this page: a dashed frame in the colour of a figure kept, its number on the corner.
        // It takes no clicks: what it frames is still the page's, to select and copy. A mark at the same place draws it.
        // Its number is what is clicked to open the figure again, to change it, frame it anew or take it out.
        let editing = self.collecting.as_ref().and_then(|c| c.figure.clone());
        let kept: Vec<Div> = self.figures.iter().filter(|f| f.page == index + 1 && editing.as_ref() != Some(&f.id) && !self.annotations.iter().any(|a| a.kind == Kind::Cut && a.page == f.page && same_place(self.rect_of(a), f.rect))).map(|f| {
            let [x0, y0, x1, y1] = f.rect;
            let id = f.id.clone();
            let chip = div().id(SharedString::from(format!("kept-{}", f.id))).absolute().left(px(-2.)).top(px(-2.)).flex().cursor_pointer()
                .tooltip(ui::tip(t("已收藏到图片库：点开可以修改、重新框选或移出")))
                // A press on the number is not a press on the page.
                .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, _, cx| { cx.stop_propagation(); cx.emit(PdfEvent::OpenFigure(id.clone())) })).items_center().gap(px(3.)).max_w(px(((x1 - x0) * scale).max(24.))).px(px(4.)).py(px(1.)).rounded_br(px(4.)).bg(Theme::success()).text_color(gpui::white()).text_size(px(10.)).line_height(px(14.))
                .child(ui::icon("check", 10., gpui::white()))
                .when(!f.label.is_empty(), |d| d.child(div().min_w_0().truncate().child(f.label.clone())));
            div().absolute().left(px(x0 * scale - 1.)).top(px(y0 * scale - 1.)).w(px((x1 - x0) * scale + 2.)).h(px((y1 - y0) * scale + 2.)).border_2().border_dashed().border_color(Theme::success()).rounded(px(2.)).child(chip)
        }).collect();
        // The piece in the figure dialog, as it was cut, until the dialog is put away; it is chosen the whole time:
        // its handles size it and its edge moves it while text is selected (`frame_at`).
        let collecting = self.collecting.as_ref().filter(|c| c.page == index + 1).map(|c| {
            let [x0, y0, x1, y1] = c.rect;
            div().absolute().left(px(x0 * scale)).top(px(y0 * scale)).w(px((x1 - x0) * scale)).h(px((y1 - y0) * scale)).border_2().border_color(Theme::accent()).bg(Theme::selection())
        });
        if free && let Some(c) = self.collecting.as_ref().filter(|c| c.page == index + 1) { grips.push(rect_grips(c.rect, scale, true, false)) }
        // A box being drawn; a text box being drawn in the colours it will have.
        let drawing = self.drawing.filter(|(page, _)| *page == index).map(|(_, [x0, y0, x1, y1])| {
            let frame = div().absolute().left(px(x0.min(x1) * scale)).top(px(y0.min(y1) * scale)).w(px((x1 - x0).abs() * scale)).h(px((y1 - y0).abs() * scale)).rounded(px(2.));
            if self.tool == Tool::Text {
                frame.bg(Theme::text_fill(&self.text_style.fill)).border_1().border_dashed().border_color(if self.text_style.border == "none" { Theme::accent() } else { Theme::text_border(&self.text_style.border) })
            } else {
                frame.border_2().border_color(Theme::highlight_swatch(wisp_core::annotation::BOX_COLOR))
            }
        });
        // Sticky notes, folded to an icon at their spot; the open one ringed. A note being written shows where it will go.
        let open = self.note_open.clone();
        let mut notes: Vec<Div> = self.annotations.iter().filter(|a| a.page == index + 1 && a.kind == Kind::Note).map(|a| {
            let [x, y, ..] = self.rect_of(a);
            let lit = open.as_deref() == Some(a.id.as_str());
            note_icon(x * scale, y * scale, lit).when(free, |d| d.cursor_pointer())
        }).collect();
        if let Some((page, x, y)) = self.note_draft && page == index { notes.push(note_icon(x * scale, y * scale, true)) }
        // The quotation on this page: a translucent marker over each of its lines.
        let cited = self.citation.index(self.sizes.len()) == Some(index);
        let marks: Vec<Div> = self
            .citation.boxes
            .iter()
            .filter(|_| cited)
            .map(|line| div().absolute().left(px(line[0] * scale - 1.)).top(px(line[1] * scale)).w(px((line[2] - line[0]) * scale + 2.)).h(px((line[3] - line[1]) * scale)).rounded(px(2.)).bg(hsla(50. / 360., 1., 0.5, 0.35)))
            .collect();
        // The sentence being read aloud: a rule of ink under each of its lines over a faint wash, unlike a selection's fill (`Theme::spoken`).
        let spoken: Vec<Div> = self.spoken.boxes.iter().filter(|_| self.spoken.page == index).map(|line| div().absolute().left(px(line[0] * scale - 1.)).top(px(line[1] * scale - 1.)).w(px((line[2] - line[0]) * scale + 2.)).h(px((line[3] - line[1]) * scale + 3.)).bg(Theme::spoken()).border_b_2().border_color(Theme::spoken_line())).collect();
        // The numbers of figures in the text that lead to a figure of this file: the pointer says they can be
        // clicked, and a line shows under the one it is over. Nothing is drawn on them otherwise.
        let refs: Vec<Div> = if free { self.figure_mentions(index).into_iter().flat_map(|(_, boxes)| boxes.iter().copied()).map(|line| div().absolute().left(px(line[0] * scale)).top(px(line[1] * scale)).w(px((line[2] - line[0]) * scale)).h(px((line[3] - line[1]) * scale + 1.)).cursor_pointer().border_b_1().border_color(gpui::transparent_black()).hover(|s| s.border_color(Theme::accent()))).collect() } else { vec![] };
        // What the find bar found on this page: the place gone to stronger.
        let found: Vec<Div> = self
            .finder
            .iter()
            .flat_map(|finder| finder.hits.iter().enumerate().filter(|(_, (page, _))| *page == index).flat_map(move |(n, (_, lines))| lines.iter().map(move |line| (finder.at == Some(n), *line))))
            .map(|(current, line)| div().absolute().left(px(line[0] * scale - 1.)).top(px(line[1] * scale)).w(px((line[2] - line[0]) * scale + 2.)).h(px((line[3] - line[1]) * scale)).rounded(px(2.)).bg(if current { Theme::found_current() } else { Theme::found() }))
            .collect();
        let sheet = div().relative().flex_none().ml(px(self.sheet_left(pane, width))).w(px(width)).h(px(height)).bg(gpui::white()).border_1().border_color(Theme::line()).overflow_hidden();
        // The facing copy of the page: each paragraph translated stands in the paragraph's own box, on the
        // page's white, set no larger than the paragraph and as small as it takes to stand in it
        // (`translation::fit`). One being asked for is tinted; the rest of the page is as it was.
        if self.shown().get(pane).is_some_and(|view| view.shows == Shows::Theirs) && let Some(facing) = &self.facing {
            let picture = image.as_ref().map(|(image, _)| image.clone());
            let pieces = facing.get(&index).map(Vec::as_slice).unwrap_or_default().iter().filter_map(|piece| {
                let [x0, y0, x1, y1] = piece.rect;
                // The colour the paragraph stands on (a tinted box, a table's band), read off the page beside it.
                let (paper, ink) = picture.as_ref().and_then(|image| ground(image, (w, h), piece.rect)).unwrap_or((gpui::white(), gpui::black()));
                let place = div().absolute().left(px((x0 - 1.) * scale)).top(px((y0 - 0.5) * scale)).w(px((x1 - x0 + 2.) * scale));
                match &piece.text {
                    Some(text) => {
                        let size = wisp_core::translation::fit(text, x1 - x0, y1 - y0, piece.size);
                        Some(place.min_h(px((y1 - y0 + 1.) * scale)).max_h(px((y1 - y0).max(size * wisp_core::translation::LEADING) * scale + size * scale)).overflow_hidden().bg(paper).text_color(ink).text_size(px(size * scale)).line_height(px(size * wisp_core::translation::LEADING * scale)).child(text.clone()))
                    }
                    None if piece.busy => Some(place.h(px((y1 - y0 + 1.) * scale)).rounded(px(2.)).bg(Theme::accent().opacity(0.10))),
                    None => None,
                }
            });
            let sheet = match image {
                Some((image, _)) => sheet.child(img(image).w(px(width)).h(px(height))).children(pieces),
                None => sheet.flex().items_center().justify_center().text_size(text_px(12.)).text_color(Theme::text_placeholder()).child(tf!("第 {} 页", "Page {}", index + 1)),
            };
            return div().w_full().flex().pb(px(PAGE_GAP)).child(sheet).into_any_element();
        }
        let sheet = match image {
            Some((image, _)) => sheet.when(cutting || !free, |d| d.cursor_crosshair()).when(free, |d| d.cursor_text()).child(img(image).w(px(width)).h(px(height))).children(refs).children(annotations).children(spoken).children(selected).children(marks).children(found).children(kept).children(collecting).children(cut).children(shapes).children(texts).children(drawing).children(notes).children(grips).children(writing),
            None => sheet.flex().items_center().justify_center().text_size(text_px(12.)).text_color(Theme::text_placeholder()).child(if self.broken.contains(&index) { tf!("第 {} 页无法渲染", "Cannot render page {}", index + 1) } else { tf!("第 {} 页", "Page {}", index + 1) }),
        };
        // Draw over the image without changing its size or covering the text.
        // This also makes page-only citations visible while a page is loading.
        let sheet = sheet.when(cited, |d| d.child(div().absolute().inset_0().border_2().border_color(Theme::accent())));
        div().w_full().flex().pb(px(PAGE_GAP)).child(sheet).into_any_element()
    }

    /// Where a page stands in the window, in one of the views. The list leaves out its own top padding from what it says, though it draws every item below it.
    fn item_bounds(&self, pane: usize, index: usize) -> Option<Bounds<Pixels>> {
        self.panes[pane].list.bounds_for_item(index).map(|bounds| Bounds { origin: point(bounds.origin.x, bounds.origin.y + px(PAGE_GAP)), size: bounds.size })
    }

    /// The view a point of the window is in.
    fn pane_under(&self, position: Point<Pixels>) -> Option<usize> {
        let shown = self.shown();
        (0..shown.len()).find(|pane| shown[*pane].shows != Shows::Map && self.panes[*pane].list.viewport_bounds().contains(&position))
    }

    /// Where a page's sheet stands in the viewer, in the half its selection, cut or highlight was made in: its top left corner.
    fn sheet_place(&self, page: usize) -> Option<(f32, f32)> {
        let (item, view) = (self.item_bounds(self.held, page)?, self.panes[0].list.viewport_bounds());
        let left = self.sheet_left(self.held, self.sizes.get(page)?.0 * self.scale_of(page));
        Some((f32::from(item.left() - view.left()) + left, f32::from(item.top() - view.top())))
    }

    /// A point of the window in a page's own points, wherever the pointer is: off the page too, while the page
    /// is laid out in that view.
    fn point_on(&self, pane: usize, page: usize, position: Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = self.item_bounds(pane, page)?;
        let scale = self.scale_of(page);
        let left = f32::from(bounds.left()) + self.sheet_left(pane, self.sizes.get(page)?.0 * scale);
        Some(((f32::from(position.x) - left) / scale, f32::from(position.y - bounds.top()) / scale))
    }

    /// The page under a point of the window in one of the views, and the point in that page's own points.
    fn locate(&self, pane: usize, position: Point<Pixels>) -> Option<(usize, f32, f32)> {
        let top = self.panes[pane].list.logical_scroll_top().item_ix;
        (top..self.sizes.len().min(top + 8)).find_map(|index| {
            let bounds = self.item_bounds(pane, index)?;
            if position.y < bounds.top() || position.y >= bounds.bottom() { return None }
            let scale = self.scale_of(index);
            let left = f32::from(bounds.left()) + self.sheet_left(pane, self.sizes[index].0 * scale);
            Some((index, (f32::from(position.x) - left) / scale, f32::from(position.y - bounds.top()) / scale))
        })
    }

    /// The pointer went down on a page: a selection starts at the glyph under it; with a tool, a box or a note
    /// starts there; on a box or a note, that is taken hold of.
    fn press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        // A press beside the text box being written in ends the writing (one on it is the field's own).
        self.finish_text(false, cx);
        cx.emit(PdfEvent::Worked);
        cx.emit(PdfEvent::Pressed);
        (self.selection, self.selecting, self.translation, self.pressed_mark, self.shape_drag, self.frame_drag, self.mark_menu) = (None, false, None, None, None, None, None);
        (self.figure_card, self.pressed_figure) = (None, None);
        // The half pressed in is the one worked in; a drag stays in it. The line between the two is neither.
        let Some(pane) = self.pane_under(event.position) else {
            if self.cutting { self.cut = None }
            return cx.notify();
        };
        (self.active, self.held) = (pane, pane);
        if self.cutting {
            self.chosen = None;
            // A piece of the page is cut out from here.
            self.cut = self.locate(pane, event.position).map(|(page, x, y)| (page, [x, y, x, y]));
            self.selecting = self.cut.is_some();
            // A click on a piece marked before is for that mark, not a new piece.
            self.pressed_mark = self.cut.and_then(|(page, [x, y, ..])| Some((self.cut_mark_at(page, x, y)?, event.position)));
            return cx.notify();
        }
        let Some((page, x, y)) = self.locate(pane, event.position) else {
            self.close_note();
            self.chosen = None;
            return cx.notify();
        };
        match self.tool {
            Tool::Box | Tool::Text => {
                self.drawing = Some((page, [x, y, x, y]));
                self.selecting = true;
                return cx.notify();
            }
            Tool::Note => {
                // The icon's top left at the pointer, the card open to write in. One note a click of the tool:
                // a click on the page afterwards is for reading again, not for another note.
                (self.note_open, self.note_draft, self.note_editing, self.tool) = (None, Some((page, x, y)), true, Tool::Select);
                self.note_input.update(cx, |input, cx| input.set_text("", cx));
                self.note_input.read(cx).focus(window);
                return cx.notify();
            }
            Tool::Select => {}
        }
        // The frame of the figure in the dialog: its handles size it, its edge moves it.
        if let Some(grab) = self.frame_at(page, x, y) && let Some(c) = self.collecting.as_ref() {
            self.frame_drag = Some(ShapeDrag { id: String::new(), page, rect: c.rect, grab, from: (x, y), moved: false, again: false });
            self.selecting = true;
            return cx.notify();
        }
        // A mark placed on the page is taken hold of. A box or a marked piece is chosen at once, so its handles
        // show while it is dragged: by a handle it is sized, by its edge moved. A note dragged moves, clicked opens.
        if let Some((id, grab)) = self.shape_at(page, x, y) && let Some(a) = self.annotations.iter().find(|a| a.id == id) {
            let rect = self.rect_of(a);
            // A text box clicked when chosen, or double-clicked, is written in once the button comes up (`release`): a
            // field given the keyboard on the press loses it again before the press is over.
            let again = self.chosen.as_deref() == Some(id.as_str()) || (a.kind == Kind::Text && event.click_count >= 2);
            if !again { self.text_palette = None }
            self.chosen = sizable(a.kind).then(|| id.clone());
            self.shape_drag = Some(ShapeDrag { id, page, rect, grab, from: (x, y), moved: false, again });
            self.selecting = true;
            return cx.notify();
        }
        // A press anywhere else on the page lets the chosen mark go.
        self.chosen = None;
        if self.note_draft.is_none() && !self.note_editing { self.note_open = None }
        self.pressed_mark = self.highlight_at(page, x, y).map(|(id, line)| MarkMenu { id, page, x, line }).or_else(|| self.cut_mark_at(page, x, y)).map(|menu| (menu, event.position));
        // A figure's number in the text: a click on it shows the figure. A highlight over it has the click.
        if self.pressed_mark.is_none() && event.click_count == 1 { self.pressed_figure = self.figure_at(page, x, y).map(|card| (card, event.position)) }
        if let Some((marks, at)) = self.marks.get(&page).and_then(|marks| Some((marks, pdftext::mark_at(marks, x, y)?))) {
            self.selection = Some(Selection { page, anchor: at, focus: at });
            self.selecting = true;
            // A double click takes the word, done at once: its bar shows, and no highlight's menu opens.
            if event.click_count == 2 {
                let (anchor, focus) = pdftext::word_marks(marks, at);
                // A space alone is nothing to select.
                let word = (!pdftext::marks_text(&marks[anchor..=focus]).is_empty()).then_some(Selection { page, anchor, focus });
                (self.selection, self.selecting, self.pressed_mark) = (word, false, None);
            }
        }
        cx.notify();
    }

    /// The highlight under a point of a 0-based page, and the top and bottom of its line there. Where
    /// highlights overlap it is the one drawn on top, the one made last.
    fn highlight_at(&self, page: usize, x: f32, y: f32) -> Option<(String, (f32, f32))> {
        self.annotations.iter().rev().filter(|a| a.kind == Kind::Highlight && a.page == page + 1).find_map(|a| {
            let line = a.boxes.iter().find(|b| x >= b[0] - 1. && x <= b[2] + 1. && y >= b[1] - 1. && y <= b[3] + 1.)?;
            Some((a.id.clone(), (line[1], line[3])))
        })
    }

    /// The piece marked to be cut out under a point of a 0-based page, as the menu a click on it opens: the
    /// one made last where they overlap. Its edge is taken hold of by `shape_at`; this is the whole of it.
    fn cut_mark_at(&self, page: usize, x: f32, y: f32) -> Option<MarkMenu> {
        self.annotations.iter().rev().filter(|a| a.kind == Kind::Cut && a.page == page + 1).find_map(|a| {
            let [x0, y0, x1, y1] = self.rect_of(a);
            (x >= x0 && x <= x1 && y >= y0 && y <= y1).then(|| MarkMenu { id: a.id.clone(), page, x: x0, line: (y0, y1) })
        })
    }

    /// The pointer moved with the button held: the selection reaches to the glyph under it, on the page it began on.
    fn drag(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting { return }
        if event.pressed_button != Some(MouseButton::Left) { return self.release(None, window, cx) }
        // A mark or the dialog's frame held follows the pointer on its own page, wherever the pointer goes
        // (off the page, onto the next, out of the viewer): it stays on its page, at the page's edge.
        if let Some(held_page) = self.shape_drag.as_ref().or(self.frame_drag.as_ref()).map(|held| held.page) {
            let Some((x, y)) = self.point_on(self.held, held_page, event.position) else { return };
            let (scale, size) = (self.scale_of(held_page), self.sizes.get(held_page).copied().unwrap_or((595., 842.)));
            if let Some(held) = self.shape_drag.as_mut() {
                let Some(rect) = held.follow(x, y, scale, size) else { return };
                self.shape_shown = Some((held.id.clone(), rect));
            } else if let Some(held) = self.frame_drag.as_mut() {
                let Some(rect) = held.follow(x, y, scale, size) else { return };
                if let Some(c) = self.collecting.as_mut() { c.rect = rect }
            }
            return cx.notify();
        }
        let Some((page, x, y)) = self.locate(self.held, event.position) else { return };
        if let Some((draw_page, corners)) = self.drawing.as_mut() {
            if *draw_page == page {
                (corners[2], corners[3]) = (x, y);
                cx.notify();
            }
            return;
        }
        if let Some((cut_page, corners)) = self.cut.as_mut() {
            if *cut_page == page {
                (corners[2], corners[3]) = (x, y);
                cx.notify();
            }
            return;
        }
        let Some(at) = self.marks.get(&page).and_then(|marks| pdftext::mark_at(marks, x, y)) else { return };
        if let Some(selection) = self.selection.as_mut().filter(|selection| selection.page == page && selection.focus != at) {
            selection.focus = at;
            cx.notify();
        }
    }

    /// The button came up, at `at` in the window when it is known: a pointer that never left its glyph
    /// selected nothing, and one that went down on a highlight and came up close by clicked it.
    fn release(&mut self, at: Option<Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        let (pressed, figure) = (self.pressed_mark.take(), self.pressed_figure.take());
        if !std::mem::take(&mut self.selecting) && pressed.is_none() && figure.is_none() { return }
        // The dialog's frame let go of: the piece it frames now goes to the dialog.
        if let Some(held) = self.frame_drag.take() {
            // Sized down to nothing, it was a slip of the hand: the frame is as it was.
            if let Some(c) = self.collecting.as_mut() && (c.rect[2] - c.rect[0] < 8. || c.rect[3] - c.rect[1] < 8.) { c.rect = held.rect }
            if held.moved && let Some(c) = self.collecting.as_ref() && c.rect != held.rect && let Some(png) = self.picture_of(held.page, c.rect) {
                cx.emit(PdfEvent::Reframe { page: held.page + 1, bbox: c.rect, png });
            }
            return cx.notify();
        }
        // A mark let go of: moved or sized, it is kept where it is now in the page's points (shown there until
        // the owner has saved it); clicked, a note opens, and a box stays chosen (it was on the press).
        if let Some(held) = self.shape_drag.take() {
            match (held.moved, self.shape_shown.clone()) {
                (true, Some((id, rect))) if id == held.id && rect != held.rect => cx.emit(PdfEvent::MoveShape { resource: self.resource_id.clone(), id, rect }),
                (true, _) => self.shape_shown = None,
                (false, _) => {
                    let kind = self.annotations.iter().find(|a| a.id == held.id).map(|a| a.kind);
                    if kind == Some(Kind::Note) {
                        if self.note_open.as_deref() == Some(held.id.as_str()) { self.close_note() } else { (self.note_open, self.note_draft, self.note_editing) = (Some(held.id.clone()), None, false) }
                    }
                    // A text box clicked once is chosen; clicked again, it is written in.
                    if kind == Some(Kind::Text) && held.again { return self.start_text(TextEdit { id: Some(held.id), page: held.page, rect: held.rect }, window, cx) }
                }
            }
            return cx.notify();
        }
        // A text box drawn is written in at once, one to a click of the tool, as a sticky note; a click makes one of
        // `TEXT_NEW` size there, on the page.
        if self.tool == Tool::Text && let Some((page, rect)) = self.drawing.take() {
            let [x0, y0, x1, y1] = handles::normal(rect);
            let rect = if x1 - x0 >= 8. && y1 - y0 >= 8. { [x0, y0, x1, y1] } else {
                let (w, h) = self.sizes.get(page).copied().unwrap_or((595., 842.));
                let (left, top) = (x0.min(w - TEXT_NEW.0).max(0.), y0.min(h - TEXT_NEW.1).max(0.));
                [left, top, (left + TEXT_NEW.0).min(w), (top + TEXT_NEW.1).min(h)]
            };
            self.tool = Tool::Select;
            return self.start_text(TextEdit { id: None, page, rect }, window, cx);
        }
        // A box drawn: one too small to frame anything was a slip of the hand. The tool stays on for the next one.
        if let Some((page, [x0, y0, x1, y1])) = self.drawing.take() && (x1 - x0).abs() >= 8. && (y1 - y0).abs() >= 8. {
            cx.emit(PdfEvent::AddShape { paper: self.paper_id.clone(), resource: self.resource_id.clone(), page: page + 1, kind: Kind::Box, rect: [x0, y0, x1, y1], note: String::new() });
            return cx.notify();
        }
        if self.selection.is_some_and(|selection| selection.anchor == selection.focus) { self.selection = None }
        // A piece too small to show anything was a slip of the hand.
        if self.cut.is_some_and(|(_, [x0, y0, x1, y1])| (x1 - x0).abs() < 8. || (y1 - y0).abs() < 8.) { self.cut = None }
        if let (Some((menu, from)), Some(at)) = (pressed, at) && self.selection.is_none() && self.cut.is_none() && f32::from((at.x - from.x).abs()).max(f32::from((at.y - from.y).abs())) <= 4. {
            // A marked piece clicked within is chosen, as by its edge; a highlight has its menu.
            if self.annotations.iter().any(|a| a.id == menu.id && sizable(a.kind)) { self.chosen = Some(menu.id) } else { self.mark_menu = Some(menu) }
        }
        // A figure's number clicked, not dragged across: its figure is shown by it.
        if let (Some((card, from)), Some(at)) = (figure, at) && self.selection.is_none() && self.cut.is_none() && f32::from((at.x - from.x).abs()).max(f32::from((at.y - from.y).abs())) <= 4. {
            self.figure_card = Some(card);
        }
        cx.notify();
    }

    /// Where a mention on a 0-based page may lead, among the paper's files: the file (`None` this one, else
    /// its place among `siblings`) and the place in it. Not a caption's own number, nor a mention within the
    /// figure it names. More than one is a choice: the same number in two files, or the figures a run names.
    fn figure_leads(&self, marks: &[Mark], mention: &figref::Mention, page: usize) -> Vec<(Option<usize>, &figref::Place)> {
        // This file first, then the others that have been read, each remembering its place among them.
        let read: Vec<(usize, &FileFigures, bool)> = self.siblings.iter().enumerate().filter_map(|(n, (sibling, figures))| Some((n, &**figures.as_ref()?, sibling.main))).collect();
        let mut files = vec![figref::File { index: &self.figure_places, main: self.main }];
        files.extend(read.iter().map(|(_, figures, main)| figref::File { index: &figures.index, main: *main }));
        mention.keys().flat_map(|key| figref::places(key, &files, 0)).filter(|(at, place)| *at != 0 || !figref::own(marks, mention, page, place)).map(|(at, place)| (at.checked_sub(1).map(|n| read[n].0), place)).collect()
    }

    /// The mentions of figures on a 0-based page that lead somewhere, each with the boxes of its words.
    fn figure_mentions(&self, page: usize) -> Vec<(&figref::Mention, &Vec<[f32; 4]>)> {
        let (Some(marks), Some(mentions)) = (self.marks.get(&page), self.mentions.get(&page)) else { return vec![] };
        mentions.iter().filter(|(mention, _)| self.figure_opened(marks, mention, page).is_some() || !self.figure_leads(marks, mention, page).is_empty()).map(|(mention, boxes)| (mention, boxes)).collect()
    }

    /// The figure of this file whose caption a mention opens ("Fig. 3 | …"): at the figure, its number
    /// leads the other way, to the paragraphs that speak of it.
    fn figure_opened(&self, marks: &[Mark], mention: &figref::Mention, page: usize) -> Option<&figref::Place> {
        self.figure_places.find(mention.key).filter(|place| figref::opens(marks, mention, page, place))
    }

    /// The card a click on a mention opens: the one figure it means, or the list of those it may mean; for
    /// the number a caption opens with, the paragraphs that speak of its figure (相关文字).
    fn figure_card_of(&self, page: usize, mention: &figref::Mention, boxes: &[[f32; 4]]) -> Option<FigureCard> {
        let marks = self.marks.get(&page)?;
        let anchor = boxes.iter().copied().reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])?;
        if let Some(place) = self.figure_opened(marks, mention, page) {
            let target = Target { file: None, place: place.clone() };
            let said = self.figure_said(&target);
            return Some(FigureCard { targets: vec![target], at: Some(0), said: Some(said), from_preview: false, page, anchor, scroll: ScrollHandle::new() });
        }
        let targets: Vec<Target> = self.figure_leads(marks, mention, page).into_iter().map(|(file, place)| Target { file, place: place.clone() }).collect();
        Some(FigureCard { at: (targets.len() == 1).then_some(0), targets, said: None, from_preview: false, page, anchor, scroll: ScrollHandle::new() })
    }

    /// The paragraphs of the paper's files that speak of a figure (`figref::speaking`): the main text's
    /// first. Of the other files, those read so far.
    fn figure_said(&self, target: &Target) -> Vec<Said> {
        let read: Vec<(usize, &FileFigures, bool)> = self.siblings.iter().enumerate().filter_map(|(n, (sibling, figures))| Some((n, &**figures.as_ref()?, sibling.main))).collect();
        let mut files = vec![figref::File { index: &self.figure_places, main: self.main }];
        files.extend(read.iter().map(|(_, figures, main)| figref::File { index: &figures.index, main: *main }));
        let mut spoken: Vec<&[figref::Spoken]> = vec![&self.figure_spoken];
        spoken.extend(read.iter().map(|(_, figures, _)| figures.spoken.as_slice()));
        // Where the figure's file stands among them; one not read (any more) has nothing said of it.
        let there = match target.file {
            None => 0,
            Some(n) => match read.iter().position(|(at, ..)| *at == n) {
                Some(at) => at + 1,
                None => return vec![],
            },
        };
        figref::speaking(&target.place, there, &files, &spoken).into_iter().map(|(at, said)| Said { file: at.checked_sub(1).map(|n| read[n].0), spoken: said.clone() }).collect()
    }

    /// 相关文字 of the figure on show in the card: the paragraphs that speak of it, in its place.
    pub fn show_said(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.figure_card.as_ref().and_then(|card| card.targets.get(card.at?)).cloned() else { return };
        let said = self.figure_said(&target);
        if let Some(card) = self.figure_card.as_mut() {
            (card.said, card.from_preview) = (Some(said), true);
            card.scroll = ScrollHandle::new();
        }
        cx.notify();
    }

    /// The n-th paragraph the card lists, taken out of it with the card put away.
    fn take_said(&mut self, n: usize) -> Option<Said> {
        let said = self.figure_card.as_ref()?.said.as_ref()?.get(n)?.clone();
        self.figure_card = None;
        Some(said)
    }

    /// Mark lines of a 0-based page as a citation followed is marked: the words gone to.
    fn mark_lines(&mut self, page: usize, lines: Vec<[f32; 4]>) {
        self.citation.clear();
        if lines.is_empty() { return }
        (self.citation.page, self.citation.boxes, self.citation.status) = (Some(page), lines, CitationStatus::Found);
    }

    /// Go to a reading mark the library's search found (`library_find.rs`): its page, `top` points down it, with the
    /// lines of the text it marks lit as a citation's sentence is; a mark placed on the page is given no lines.
    pub fn show_mark(&mut self, page: usize, top: f32, lines: Vec<[f32; 4]>, cx: &mut Context<Self>) {
        self.go_to_place(page, top, cx);
        self.mark_lines(page.saturating_sub(1), lines);
        cx.notify();
    }

    /// Go to the n-th paragraph the card lists: the half worked in shows it with the figure's number
    /// marked, and the place left is kept to come back to. One in another file is the owner's to show.
    pub fn go_to_said(&mut self, n: usize, cx: &mut Context<Self>) {
        let Some(said) = self.take_said(n) else { return };
        if self.sizes.is_empty() { return }
        let (page, lines) = (said.spoken.page, said.spoken.boxes.clone());
        let top = (lines.first().map_or(said.spoken.rect[1], |line| line[1]) - SAID_LEAD).max(0.);
        let here = self.here();
        self.returns.push(here);
        if let Some((sibling, _)) = said.file.and_then(|n| self.siblings.get(n)) {
            let file = sibling.resource.clone();
            let trail = self.trail();
            cx.emit(PdfEvent::FigureGo { file, page, top, mark: lines, trail });
            return cx.notify();
        }
        if self.returns.len() > RETURNS { self.returns.remove(0); }
        let index = page.min(self.sizes.len() - 1);
        let scale = self.scale_of(index);
        self.mark_lines(index, lines);
        self.list().scroll_to(ListOffset { item_ix: index, offset_in_item: px(top * scale) });
        if self.max_x > 0. { self.panes[self.active].scroll_x = (said.spoken.rect[0] * scale - 40.).clamp(0., self.max_x) }
        cx.notify();
    }

    /// 固定对照 of the n-th paragraph the card lists: it stays in the other half, marked.
    pub fn pin_said(&mut self, n: usize, cx: &mut Context<Self>) {
        let Some(said) = self.take_said(n) else { return };
        let lines = said.spoken.boxes.clone();
        let top = (lines.first().map_or(said.spoken.rect[1], |line| line[1]) - SAID_LEAD).max(0.);
        let file = said.file.and_then(|n| self.siblings.get(n)).map(|(sibling, _)| sibling.resource.clone());
        cx.emit(PdfEvent::FigureBeside { file, page: said.spoken.page, top, mark: lines });
        cx.notify();
    }

    /// The figure a point of a 0-based page speaks of, as the card a click there opens.
    fn figure_at(&self, page: usize, x: f32, y: f32) -> Option<FigureCard> {
        let (mention, boxes) = self.figure_mentions(page).into_iter().find(|(_, boxes)| boxes.iter().any(|b| x >= b[0] - 1. && x <= b[2] + 1. && y >= b[1] - 1. && y <= b[3] + 1.))?;
        self.figure_card_of(page, mention, boxes)
    }

    /// Show the figure the n-th mention of a 0-based page leads to, as a click on it does (a debug step);
    /// false when the page's glyphs or the file's figures are not read yet, or it has no such mention.
    pub fn open_figure_mention(&mut self, page: usize, n: usize, cx: &mut Context<Self>) -> bool {
        let Some(card) = self.figure_mentions(page).into_iter().nth(n).and_then(|(mention, boxes)| self.figure_card_of(page, mention, boxes)) else { return false };
        self.figure_card = Some(card);
        self.held = self.active;
        cx.notify();
        true
    }

    /// Choose the n-th of the figures the card lists (a debug step; a click on its row does the same).
    pub fn pick_figure(&mut self, n: usize, cx: &mut Context<Self>) {
        if let Some(card) = self.figure_card.as_mut().filter(|card| n < card.targets.len()) { (card.at, card.said) = (Some(n), None) }
        cx.notify();
    }

    /// The sizes of the pages of the file a figure is in.
    fn sizes_of(&self, file: Option<usize>) -> &[(f32, f32)] {
        match file {
            None => &self.sizes,
            Some(n) => self.siblings.get(n).and_then(|(_, figures)| figures.as_ref()).map_or(&[], |figures| &figures.sizes),
        }
    }

    /// What the reader calls the file a figure is in; nothing for a paper of one PDF.
    fn file_label(&self, file: Option<usize>) -> Option<String> {
        match file {
            None => (!self.label.is_empty()).then(|| self.label.clone()),
            Some(n) => self.siblings.get(n).map(|(sibling, _)| sibling.label.clone()),
        }
    }

    /// What to look at of a figure, in reading order (`figref::Place::pieces`), by the sizes of its file's pages.
    fn pieces_of(&self, target: &Target) -> Vec<(usize, [f32; 4])> {
        let sizes = self.sizes_of(target.file);
        target.place.pieces(|page| sizes.get(page).copied().unwrap_or((595., 842.)))
    }

    /// Where a figure begins in its file: the 0-based page, and how far down and across it in points.
    fn figure_start(&self, target: &Target) -> (usize, f32, f32) {
        let (page, rect) = self.pieces_of(target).into_iter().next().unwrap_or((target.place.page, target.place.shown.caption));
        (page, rect[1], rect[0])
    }

    /// The figure on show in the card, taken out of it: what its buttons act on.
    fn take_figure(&mut self) -> Option<Target> {
        let card = self.figure_card.as_ref()?;
        let target = card.targets.get(card.at?)?.clone();
        self.figure_card = None;
        Some(target)
    }

    /// 固定对照: the figure of the card stays in the viewer's other half, and the text is read on in this one.
    /// One in another file of the paper has that file shown there.
    pub fn pin_figure(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.take_figure() else { return };
        let (page, top, _) = self.figure_start(&target);
        let file = target.file.and_then(|n| self.siblings.get(n)).map(|(sibling, _)| sibling.resource.clone());
        cx.emit(PdfEvent::FigureBeside { file, page, top, mark: vec![] });
        cx.notify();
    }

    /// Show what stands `top` points down a 0-based page in the view scrolled apart from the one worked in
    /// (`apart`), splitting the viewer for it (as `split` says) when it shows its pages once. The view worked
    /// in stays where it is.
    pub fn keep_beside(&mut self, split: Split, page: usize, top: f32, mark: Vec<[f32; 4]>, cx: &mut Context<Self>) {
        // Beside a translation, the third column is where it is kept; without that column the translation has the other half.
        if !self.can_keep(false) || self.sizes.is_empty() { return }
        // No view scrolled on its own yet: one comes, beside the pages or as the third column beside the pages and the map.
        if self.apart().is_none() {
            let two = self.split.views() == 2;
            if two { self.stood = self.split }
            self.stand(if two { Split::Triple } else if split == Split::Off { Split::Side } else { split }, cx);
        }
        let Some(other) = self.apart() else { return };
        // It is the one its group follows now: the pages and their translation go there together.
        self.led[self.shown()[other].group as usize] = other;
        let index = page.min(self.sizes.len() - 1);
        if !mark.is_empty() { self.mark_lines(index, mark) }
        self.panes[other].list.scroll_to(ListOffset { item_ix: index, offset_in_item: px((top * self.scale_of(index) - PAGE_GAP).max(0.)) });
        cx.notify();
    }

    /// Where the half worked in is read at now, as a place to come back to.
    fn here(&self) -> Return {
        let top = self.list().logical_scroll_top();
        let scale = self.scale_of(top.item_ix).max(0.01);
        Return { pane: self.active, place: (top.item_ix, f32::from(top.offset_in_item) / scale), across: self.panes[self.active].scroll_x / scale, zoom: self.zoom, from: None }
    }

    /// The places to come back to, each naming its file, to be handed to the viewer of another file.
    fn trail(&mut self) -> Vec<Return> {
        let mut trail = std::mem::take(&mut self.returns);
        for back in trail.iter_mut().filter(|back| back.from.is_none()) { back.from = Some((self.resource_id.clone(), self.label.clone())) }
        trail
    }

    /// Take the places to come back to as another viewer handed them: those in this file are its own again.
    fn take_trail(&mut self, mut trail: Vec<Return>) {
        for back in &mut trail {
            if back.from.as_ref().is_some_and(|(file, _)| *file == self.resource_id) { back.from = None }
        }
        if trail.len() > RETURNS { trail.drain(..trail.len() - RETURNS); }
        self.returns = trail;
    }

    /// 前往原位置: the half worked in goes to the card's figure, and the place it leaves is kept to come back
    /// to. A figure in another file of the paper is the owner's to show (`PdfEvent::FigureGo`).
    pub fn go_to_figure(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.take_figure() else { return };
        if self.sizes.is_empty() { return }
        let (page, down, across) = self.figure_start(&target);
        let here = self.here();
        if let Some((sibling, _)) = target.file.and_then(|n| self.siblings.get(n)) {
            let file = sibling.resource.clone();
            self.returns.push(here);
            let trail = self.trail();
            cx.emit(PdfEvent::FigureGo { file, page, top: down, mark: vec![], trail });
            return cx.notify();
        }
        self.returns.push(here);
        if self.returns.len() > RETURNS { self.returns.remove(0); }
        let index = page.min(self.sizes.len() - 1);
        let scale = self.scale_of(index);
        self.citation.clear();
        self.list().scroll_to(ListOffset { item_ix: index, offset_in_item: px((down * scale - PAGE_GAP).max(0.)) });
        if self.max_x > 0. { self.panes[self.active].scroll_x = (across * scale - 40.).clamp(0., self.max_x) }
        cx.notify();
    }

    /// This viewer was shown for a figure another file's text named, or for a paragraph that speaks of
    /// another file's figure: it has the places to come back to, and goes to what stands `top` points down
    /// a 0-based page (now, or once the file has opened), with `mark` marked there. With no place, it only
    /// takes the trail back: the file asked for could not be shown.
    pub fn arrive(&mut self, trail: Vec<Return>, place: Option<(usize, f32)>, mark: Vec<[f32; 4]>, cx: &mut Context<Self>) {
        self.take_trail(trail);
        if let Some((page, top)) = place {
            self.mark_lines(page, mark);
            let gap = PAGE_GAP / self.scale_of(page).max(0.01);
            self.resume((page, (top - gap).max(0.)), cx);
        }
        cx.notify();
    }

    /// 返回刚才位置: back to the place the last 前往原位置 left, in its half, at the zoom it was read at. A
    /// place in another file of the paper is the owner's to show (`PdfEvent::FigureReturn`).
    pub fn return_back(&mut self, cx: &mut Context<Self>) {
        let Some(back) = self.returns.pop() else { return };
        if back.from.is_some() {
            let trail = self.trail();
            cx.emit(PdfEvent::FigureReturn { back, trail });
            return cx.notify();
        }
        self.restore(back, None, cx);
    }

    /// Read on at a place left before: its zoom, its half, its page and how far down and across. With a
    /// trail, this viewer was shown for it and takes the places left before that one.
    pub fn restore(&mut self, back: Return, trail: Option<Vec<Return>>, cx: &mut Context<Self>) {
        if let Some(trail) = trail { self.take_trail(trail) }
        // Not open yet: it opens there.
        if self.sizes.is_empty() {
            (self.zoom, self.resume) = (back.zoom, Some(back.place));
            return cx.notify();
        }
        if back.zoom != self.zoom { self.set_zoom(back.zoom, None, cx) }
        let pane = back.pane.min(self.shown().len() - 1);
        self.active = pane;
        let index = back.place.0.min(self.sizes.len() - 1);
        let scale = self.scale_of(index);
        self.panes[pane].list.scroll_to(ListOffset { item_ix: index, offset_in_item: px(back.place.1 * scale) });
        self.panes[pane].scroll_x = (back.across * scale).clamp(0., self.max_x);
        cx.notify();
    }

    /// A page of another file of the paper as a picture for the card, asked for once: it is drawn in the
    /// background from the file itself, since no viewer of it need be open.
    fn want_foreign(&mut self, file: usize, page: usize, scale: f32, cx: &mut Context<Self>) {
        if !self.foreign_pending.insert((file, page)) { return }
        let Some((sibling, figures)) = self.siblings.get(file) else { return };
        let size = figures.as_ref().and_then(|figures| figures.sizes.get(page).copied()).unwrap_or((595., 842.));
        let (path, resource, scale) = (sibling.path.clone(), sibling.resource.clone(), render_scale(size, scale * self.device_scale));
        cx.spawn(async move |this, cx| {
            let picture = cx.background_executor().spawn(async move { page_picture(&path, page, scale) }).await;
            let _ = this.update(cx, |this, cx| {
                // The paper's files may have changed meanwhile: the picture is of the file it was asked of. One
                // that could not be drawn stays asked for, so it is not tried again and again.
                let Some(file) = this.siblings.iter().position(|(sibling, _)| sibling.resource == resource) else { return };
                let Some(buffer) = picture.and_then(|(width, height, bytes)| image::RgbaImage::from_raw(width, height, bytes)) else { return };
                this.foreign_pending.remove(&(file, page));
                this.foreign.push(((file, page), Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))));
                while this.foreign.len() > KEEP_FOREIGN { cx.drop_image(this.foreign.remove(0).1, None) }
                cx.notify();
            });
        })
        .detach();
    }

    /// A piece of a page drawn `scale` pixels to the point: the page's picture behind a window of the
    /// piece's size. A page with no picture yet is asked for, and its place kept meanwhile.
    fn figure_piece(&mut self, file: Option<usize>, page: usize, [x0, y0, x1, y1]: [f32; 4], scale: f32, cx: &mut Context<Self>) -> Div {
        let (w, h) = self.sizes_of(file).get(page).copied().unwrap_or((595., 842.));
        let frame = div().relative().flex_none().w(px((x1 - x0) * scale)).h(px((y1 - y0) * scale)).overflow_hidden().bg(gpui::white());
        let picture = match file {
            None => self.images.get(&page).map(|(image, ..)| image.clone()),
            Some(n) => self.foreign.iter().find(|(of, _)| *of == (n, page)).map(|(_, image)| image.clone()),
        };
        if let Some(image) = picture {
            if file.is_none() { self.touch(page) }
            return frame.child(img(image).absolute().left(px(-x0 * scale)).top(px(-y0 * scale)).w(px(w * scale)).h(px(h * scale)));
        }
        match file {
            None => if !self.broken.contains(&page) && self.pending.insert(page) { let _ = self.requests.send(Request::Page(page, render_scale((w, h), scale * self.device_scale))); },
            Some(n) => self.want_foreign(n, page, scale, cx),
        }
        // Said at its top: a tall piece's middle may be out of the card's sight.
        frame.flex().justify_center().pt(px(28.)).text_size(text_px(12.)).text_color(Theme::text_placeholder()).child(if file.is_none() && self.broken.contains(&page) { tf!("第 {} 页无法渲染", "Cannot render page {}", page + 1) } else { tf!("第 {} 页", "Page {}", page + 1) })
    }

    /// Where a figure is, in a line: its file when the paper has several, the figure's page, and the
    /// caption's when that is another.
    fn figure_whereabouts(&self, target: &Target) -> String {
        let place = &target.place;
        let pages = match (place.apart, place.pictured()) {
            (Some((figure, _)), _) => tf!("图在第 {} 页，图注在第 {} 页", "Figure on page {}, caption on page {}", figure + 1, place.page + 1),
            (None, true) => tf!("第 {} 页", "Page {}", place.page + 1),
            (None, false) => tf!("第 {} 页 · 只找到图注", "Page {} · caption only", place.page + 1),
        };
        match self.file_label(target.file) {
            Some(file) => format!("{file} · {pages}"),
            None => pages,
        }
    }

    /// The figure whose number was clicked, by the number: the piece of the page it takes with its caption
    /// (the figure and then the caption, when they are on two pages), and under it what else can be done:
    /// keep it in the other half, or go there. A number that may mean several figures (the same one in two
    /// of the paper's files, those a run names) lists them first, each with its file and page.
    fn figure_card(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.selecting { return None }
        let card = self.figure_card.clone()?;
        let (x, y) = self.sheet_place(card.page)?;
        let at = self.scale_of(card.page);
        let anchor = [x + card.anchor[0] * at, y + card.anchor[1] * at, x + card.anchor[2] * at, y + card.anchor[3] * at];
        let room = FIGURE_W.min(self.width - 2. * CARD_EDGE).max(120.);
        let close = div().id("pdf-figure-close").flex().flex_none().items_center().justify_center().size(px(24.)).rounded(px(6.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).child(ui::icon("x", 13., Theme::text_tertiary())).on_click(cx.listener(|this, _, _, cx| {
            this.figure_card = None;
            cx.notify();
        }));
        let header = || div().flex().flex_none().items_center().gap(px(8.)).h(text_px(34.)).pl(px(12.)).pr(px(4.)).border_b_1().border_color(Theme::line());
        let frame = |place: CardPlace| {
            div()
                .id("pdf-figure")
                .absolute()
                .left(px(place.left))
                .when_some(place.top, |d, top| d.top(px(top)))
                .when_some(place.bottom, |d, bottom| d.bottom(px(bottom)))
                .w(px(place.width))
                .max_h(px(place.max_height))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(10.))
                .bg(Theme::bg_body())
                .border_1()
                .border_color(Theme::line_strong())
                .shadow(crate::theme::shadow_card())
                .font(crate::theme::ui_font())
                .text_size(text_px(13.))
                .line_height(text_px(20.))
                .occlude()
                // A press on the card is not a press on the page.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        let Some(target) = card.at.and_then(|at| card.targets.get(at)) else {
            // Which of them: a row each, the number and where it is, and how its legend begins.
            let rows: Vec<Stateful<Div>> = card.targets.iter().enumerate().map(|(n, target)| {
                let legend = ui::one_line(&target.place.shown.legend);
                div().id(("pdf-figure-choice", n)).flex().flex_col().flex_none().px(px(12.)).py(px(7.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).when(n > 0, |d| d.border_t_1().border_color(Theme::line()))
                    .child(div().flex().items_center().gap(px(8.))
                        .child(div().flex_none().text_color(Theme::text()).font_weight(FontWeight::MEDIUM).child(ui::one_line(&target.place.shown.label)))
                        .child(div().flex_1().min_w_0().truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(self.figure_whereabouts(target))))
                    .when(!legend.is_empty(), |d| d.child(div().truncate().text_size(text_px(12.)).text_color(Theme::text_secondary()).child(legend)))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_figure(n, cx)))
            }).collect();
            let width = CHOICES_W.min(room);
            let place = card_place(anchor, width, (34. + rows.len() as f32 * 52.).min(FIGURE_TALL), (self.width, self.height));
            let title = tf!("这里可能指 {} 张图", "{} figures it may mean", card.targets.len());
            return Some(
                frame(place)
                    .child(header().child(div().flex_1().min_w_0().truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(title)).child(close))
                    .child(div().id("pdf-figure-choices").flex().flex_col().flex_shrink().min_h_0().overflow_y_scroll().track_scroll(&card.scroll).children(rows))
                    .into_any_element(),
            );
        };
        if let Some(said) = &card.said {
            // The caption by itself, then where the text speaks of the figure: the part of the paper, the file
            // and page, and the paragraph from the sentence that names it. A row goes there; its pin keeps
            // it in the other half.
            let shown = &target.place.shown;
            let caption = (!shown.legend.is_empty()).then(|| div().flex().flex_col().flex_none().gap(px(2.)).px(px(12.)).py(px(8.)).border_b_1().border_color(Theme::line())
                .child(div().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(t("图注")))
                .child(div().line_clamp(4).text_ellipsis().text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_secondary()).child(shown.legend.clone())));
            let count = if said.is_empty() { t("没有在文中找到提到这张图的段落。").to_string() } else { tf!("文中提到它的 {} 处", "{} places in the text speak of it", said.len()) };
            let rows: Vec<Stateful<Div>> = said.iter().enumerate().map(|(n, said)| {
                let part = wisp_core::structure::SECTIONS.iter().find(|(key, _)| *key == said.spoken.section).map(|(_, label)| t(label).to_string());
                let whereabouts = part.into_iter().chain(self.file_label(said.file)).chain([tf!("第 {} 页", "Page {}", said.spoken.page + 1)]).collect::<Vec<_>>().join(" · ");
                let pin = self.can_keep(said.file.is_some()).then(|| div().id(("pdf-said-pin", n)).flex().flex_none().items_center().justify_center().size(px(22.)).rounded(px(5.)).cursor_pointer().hover(|s| s.bg(Theme::pressed())).tooltip(ui::tip(t("固定对照"))).child(ui::icon("columns-2", 13., Theme::text_tertiary()))
                    // A press on the pin is not a press on its row.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.pin_said(n, cx) })));
                div().id(("pdf-said", n)).flex().flex_col().flex_none().gap(px(1.)).pl(px(12.)).pr(px(6.)).py(px(7.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).border_t_1().border_color(Theme::line())
                    .child(div().flex().items_center().gap(px(6.)).h(px(22.)).child(div().flex_1().min_w_0().truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(whereabouts)).children(pin))
                    .child(div().pr(px(6.)).line_clamp(3).text_ellipsis().text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text()).child(said.spoken.text.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| this.go_to_said(n, cx)))
            }).collect();
            let tall = 34. + if caption.is_some() { 104. } else { 0. } + 30. + rows.len() as f32 * 92.;
            let place = card_place(anchor, SAID_W.min(room), tall.min(FIGURE_TALL), (self.width, self.height));
            // Back to the figure's preview, when it was asked from there.
            let back = card.from_preview.then(|| div().id("pdf-figure-back").flex().flex_none().items_center().justify_center().size(px(24.)).ml(px(-6.)).rounded(px(6.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).tooltip(ui::tip(t("回到这张图"))).child(ui::icon("chevron-left", 14., Theme::text_tertiary())).on_click(cx.listener(|this, _, _, cx| {
                if let Some(card) = this.figure_card.as_mut() { (card.said, card.scroll) = (None, ScrollHandle::new()) }
                cx.notify();
            })));
            let header = header()
                .children(back)
                .child(div().flex_none().text_color(Theme::text()).font_weight(FontWeight::MEDIUM).child(ui::one_line(&shown.label)))
                .child(div().flex_1().min_w_0().truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(t("相关文字")))
                .child(close);
            return Some(
                frame(place)
                    .child(header)
                    .child(div().id("pdf-figure-said").flex().flex_col().flex_shrink().min_h_0().overflow_y_scroll().track_scroll(&card.scroll)
                        .children(caption)
                        .child(div().flex_none().px(px(12.)).py(px(7.)).text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(count))
                        .children(rows))
                    .into_any_element(),
            );
        }
        let pieces = self.pieces_of(target);
        // One scale for all of it: the widest piece fills the card, and a narrow one is not blown up.
        let widest = pieces.iter().map(|(_, rect)| rect[2] - rect[0]).fold(1., f32::max);
        let scale = (room / widest).min(FIGURE_SCALE);
        let width = (widest * scale).max(FIGURE_W_MIN.min(room));
        let tall: f32 = pieces.iter().map(|(_, rect)| (rect[3] - rect[1]) * scale + 1.).sum::<f32>() + 34. + 38.;
        let place = card_place(anchor, width + 2., tall.min(FIGURE_TALL), (self.width, self.height));
        // Back to the list it was chosen from, where there was one.
        let back = (card.targets.len() > 1).then(|| div().id("pdf-figure-back").flex().flex_none().items_center().justify_center().size(px(24.)).ml(px(-6.)).rounded(px(6.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).tooltip(ui::tip(t("回到候选"))).child(ui::icon("chevron-left", 14., Theme::text_tertiary())).on_click(cx.listener(|this, _, _, cx| {
            if let Some(card) = this.figure_card.as_mut() { card.at = None }
            cx.notify();
        })));
        let header = header()
            .children(back)
            .child(div().flex_none().text_color(Theme::text()).font_weight(FontWeight::MEDIUM).child(ui::one_line(&target.place.shown.label)))
            .child(div().flex_1().min_w_0().truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(self.figure_whereabouts(target)))
            .child(close);
        let (file, last) = (target.file, pieces.len().saturating_sub(1));
        let pictures: Vec<Div> = pieces.iter().enumerate().map(|(n, (page, rect))| {
            let piece = self.figure_piece(file, *page, *rect, scale, cx);
            div().w_full().flex().justify_center().bg(gpui::white()).when(n < last, |d| d.border_b_1().border_color(Theme::line())).child(piece)
        }).collect();
        let button = |id: &'static str, label: &'static str| div().id(id).flex().flex_none().items_center().h(text_px(26.)).px(px(9.)).rounded(px(6.)).cursor_pointer().text_color(Theme::accent()).hover(|s| s.bg(Theme::hover())).child(label);
        // The translation has the other half while it stands beside the pages: the third column is then where a figure is kept.
        let pin = self.can_keep(file.is_some()).then(|| button("pdf-figure-pin", t("固定对照")).on_click(cx.listener(|this, _, _, cx| this.pin_figure(cx))));
        let go = button("pdf-figure-go", t("前往原位置")).on_click(cx.listener(|this, _, _, cx| this.go_to_figure(cx)));
        // The other way: from the figure to the text that speaks of it. Apart from the two that move the pages.
        let said = button("pdf-figure-said-open", t("相关文字")).text_color(Theme::text_secondary()).on_click(cx.listener(|this, _, _, cx| this.show_said(cx)));
        Some(
            frame(place)
                .child(header)
                .child(div().id("pdf-figure-pieces").flex().flex_col().flex_shrink().min_h_0().overflow_y_scroll().track_scroll(&card.scroll).children(pictures))
                .child(div().flex().flex_none().items_center().gap(px(2.)).h(text_px(38.)).px(px(6.)).border_t_1().border_color(Theme::line()).child(said).child(div().flex_1()).children(pin).child(go))
                .into_any_element(),
        )
    }

    /// 返回刚才位置, at the foot of the half that was left: where the last 前往原位置 came from, with its
    /// file when that is another of the paper's. It stays until it is used or put away.
    fn return_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let back = self.returns.last()?;
        // Not laid out yet (a viewer just made for another file): its foot is not known.
        if self.height <= 0. { return None }
        let pane = if back.from.is_some() { self.active } else { back.pane.min(self.shown().len() - 1) };
        let half_h = self.pane_size().1;
        let (left, top) = pane_origin(self.split, (self.width, self.height), pane);
        let page = tf!("第 {} 页", "Page {}", back.place.0 + 1);
        let whence = match &back.from {
            Some((_, file)) if !file.is_empty() => format!("{file} · {page}"),
            _ => page,
        };
        Some(
            div()
                .absolute()
                .left(px(left + 12.))
                .top(px(top + half_h - 28. - 30.))
                .flex()
                .items_center()
                .h(px(30.))
                .rounded(px(8.))
                .bg(Theme::bg_body())
                .border_1()
                .border_color(Theme::line_strong())
                .shadow(crate::theme::shadow_card())
                .font(crate::theme::ui_font())
                .text_size(text_px(12.))
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div().id("pdf-return").flex().items_center().gap(px(6.)).h_full().pl(px(9.)).pr(px(6.)).rounded_l(px(8.)).cursor_pointer().hover(|s| s.bg(Theme::hover()))
                        .child(ui::icon("undo-2", 13., Theme::accent()))
                        .child(div().text_color(Theme::accent()).child(t("返回刚才位置")))
                        .child(div().text_color(Theme::text_tertiary()).child(whence))
                        .on_click(cx.listener(|this, _, _, cx| this.return_back(cx))),
                )
                .child(
                    div().id("pdf-return-drop").flex().items_center().justify_center().w(px(26.)).h_full().rounded_r(px(8.)).cursor_pointer().hover(|s| s.bg(Theme::hover()))
                        .tooltip(ui::tip(t("不回去了")))
                        .child(ui::icon("x", 12., Theme::text_tertiary()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.returns.pop();
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    /// The mark the menu is for: the highlight clicked, else the mark chosen.
    fn menu_id(&self) -> Option<String> {
        self.mark_menu.as_ref().map(|menu| menu.id.clone()).or_else(|| self.chosen.clone())
    }

    /// The highlight of the menu, or the box chosen, takes another colour. A box stays chosen.
    fn recolor(&mut self, color: &'static str, cx: &mut Context<Self>) {
        let Some(id) = self.menu_id() else { return };
        self.mark_menu = None;
        if self.annotations.iter().any(|a| a.id == id && a.color != color) {
            cx.emit(PdfEvent::RecolorAnnotation { resource: self.resource_id.clone(), id, color });
        }
        cx.notify();
    }

    /// The highlight of the menu, or the mark chosen, goes, and only that one.
    fn remove_mark(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.menu_id() else { return };
        (self.mark_menu, self.chosen) = (None, None);
        cx.emit(PdfEvent::RemoveAnnotation { resource: self.resource_id.clone(), id });
        cx.notify();
    }

    /// Cut pieces out of pages with the pointer, or go back to selecting text.
    pub fn toggle_cutting(&mut self, cx: &mut Context<Self>) {
        self.finish_text(false, cx);
        self.cutting = !self.cutting;
        (self.tool, self.drawing, self.mark_menu, self.chosen) = (Tool::Select, None, None, None);
        self.close_note();
        (self.cut, self.selection, self.selecting, self.translation) = (None, None, false, None);
        cx.notify();
    }

    /// The list of marks is open under its button at the top right, where the floating tools would stand.
    pub fn annotations_open(&self) -> bool {
        self.annotations_open
    }

    /// The viewer's width, as `set_width` was last told.
    pub fn width(&self) -> f32 {
        self.width
    }

    /// The viewer's height, as it was last laid out.
    pub fn height(&self) -> f32 {
        self.height
    }

    pub fn cutting(&self) -> bool {
        self.cutting
    }

    /// The piece cut out, as a PNG.
    fn cut_picture(&self) -> Option<Vec<u8>> {
        let (page, rect) = self.cut?;
        self.picture_of(page, rect)
    }

    /// A piece of a 0-based page as a PNG: taken from the picture the page is drawn with.
    fn picture_of(&self, page: usize, [x0, y0, x1, y1]: [f32; 4]) -> Option<Vec<u8>> {
        let (image, scale, _) = self.images.get(&page)?;
        let (width, height) = (image.size(0).width.0 as u32, image.size(0).height.0 as u32);
        let edge = |at: f32, most: u32| ((at * scale).round().max(0.) as u32).min(most);
        let (left, right, top, bottom) = (edge(x0.min(x1), width), edge(x0.max(x1), width), edge(y0.min(y1), height), edge(y0.max(y1), height));
        if right <= left || bottom <= top { return None }
        let pixels = image.as_bytes(0)?;
        let mut piece = image::RgbaImage::new(right - left, bottom - top);
        for (x, y, pixel) in piece.enumerate_pixels_mut() {
            let at = (((top + y) * width + left + x) * 4) as usize;
            // The page is kept as BGRA.
            *pixel = image::Rgba([pixels[at + 2], pixels[at + 1], pixels[at], 255]);
        }
        let mut png = vec![];
        image::DynamicImage::ImageRgba8(piece).to_rgb8().write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
        Some(png)
    }

    /// Hand the piece cut out to a conversation as a picture, and go back to selecting text.
    fn send_cut(&mut self, to_library: bool, cx: &mut Context<Self>) {
        let Some(png) = self.cut_picture() else { return };
        (self.cut, self.cutting) = (None, false);
        cx.emit(PdfEvent::Picture(png, to_library));
        cx.notify();
    }

    /// Hand the piece cut out to the figure library, with the caption the page gives it. The viewer goes back
    /// to selecting text, so what the page says of the figure can be copied into the dialog standing beside it;
    /// the piece is shown where it was until the dialog is put away (`set_figures`).
    pub fn collect_cut(&mut self, cx: &mut Context<Self>) {
        let (Some(png), Some((page, [x0, y0, x1, y1]))) = (self.cut_picture(), self.cut) else { return };
        let bbox = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
        let caption = self.marks.get(&page).and_then(|marks| wisp_core::figure::caption(marks, bbox));
        (self.cut, self.cutting, self.selecting) = (None, false, false);
        cx.emit(PdfEvent::Figure(Piece { png, page: page + 1, bbox, caption }));
        cx.notify();
    }

    /// The figures of the library cut from this file, and the piece whose dialog is open: drawn on their pages.
    pub fn set_figures(&mut self, figures: Vec<KeptFigure>, collecting: Option<Collecting>, cx: &mut Context<Self>) {
        // The frame being held is the pointer's until it is let go.
        if self.frame_drag.is_some() && self.collecting.as_ref().map(|c| &c.figure) == collecting.as_ref().map(|c| &c.figure) { return self.figures = figures }
        if self.figures == figures && self.collecting == collecting { return }
        (self.figures, self.collecting) = (figures, collecting);
        cx.notify();
    }

    /// The frame of the dialog's piece under a point of a 0-based page: a handle (to size it) or its edge (to
    /// move it). The frame is always chosen while the dialog is open.
    fn frame_at(&self, page: usize, x: f32, y: f32) -> Option<Grab> {
        let c = self.collecting.as_ref().filter(|c| c.page == page + 1)?;
        let scale = self.scale_of(page);
        handles::grab_at(c.rect, (x, y), GRIP / scale, EDGE_REACH / scale)
    }

    /// A figure of the library stands at this place of a 0-based page.
    fn kept_at(&self, page: usize, rect: [f32; 4]) -> bool {
        self.figures.iter().any(|f| f.page == page + 1 && same_place(f.rect, rect))
    }

    /// Leave the piece cut out marked on its page, a dashed frame to click later: for the figure library or
    /// a question then. Cutting stays on for the next one.
    fn mark_cut(&mut self, cx: &mut Context<Self>) {
        let Some((page, rect)) = self.cut.take() else { return };
        cx.emit(PdfEvent::AddShape { paper: self.paper_id.clone(), resource: self.resource_id.clone(), page: page + 1, kind: Kind::Cut, rect, note: String::new() });
        cx.notify();
    }

    /// The piece of the menu's mark, with its 0-based page and its place there.
    fn marked_piece(&self) -> Option<(Vec<u8>, usize, [f32; 4])> {
        let id = self.menu_id()?;
        let mark = self.annotations.iter().find(|a| a.id == id && a.kind == Kind::Cut)?;
        let (page, rect) = (mark.page.checked_sub(1)?, self.rect_of(mark));
        Some((self.picture_of(page, rect)?, page, rect))
    }

    /// Hand the piece of the menu's mark to the figure library, as `collect_cut` does a piece just cut. The mark stays.
    fn collect_mark(&mut self, cx: &mut Context<Self>) {
        let Some((png, page, bbox)) = self.marked_piece() else { return };
        (self.mark_menu, self.chosen) = (None, None);
        let caption = self.marks.get(&page).and_then(|marks| wisp_core::figure::caption(marks, bbox));
        cx.emit(PdfEvent::Figure(Piece { png, page: page + 1, bbox, caption }));
        cx.notify();
    }

    /// Hand the piece of the menu's mark to a conversation as a picture, and go back to selecting text. The mark stays.
    fn send_mark(&mut self, to_library: bool, cx: &mut Context<Self>) {
        let Some((png, ..)) = self.marked_piece() else { return };
        (self.mark_menu, self.chosen, self.cut, self.cutting) = (None, None, None, false);
        cx.emit(PdfEvent::Picture(png, to_library));
        cx.notify();
    }

    /// Mark a piece of a 0-based page as cut out, as a drag would.
    pub fn set_cut(&mut self, page: usize, bbox: [f32; 4], cx: &mut Context<Self>) {
        (self.cutting, self.cut, self.selection, self.selecting, self.translation) = (true, Some((page, bbox)), None, false, None);
        self.held = self.active;
        cx.notify();
    }

    /// The selected text as it reads, and the 1-based page it is on.
    pub fn selected(&self) -> Option<(usize, String)> {
        let selection = self.selection?;
        let marks = self.marks.get(&selection.page)?;
        let text = pdftext::marks_text(marks.get(selection.anchor.min(selection.focus)..=selection.anchor.max(selection.focus))?);
        (!text.is_empty()).then_some((selection.page + 1, text))
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let Some((_, text)) = self.selected() else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.emit(PdfEvent::Copied);
    }

    /// Hand what is selected to a conversation, and let go of it.
    pub fn quote(&mut self, to_library: bool, cx: &mut Context<Self>) {
        let Some((page, text)) = self.selected() else { return };
        self.selection = None;
        cx.emit(PdfEvent::Quote(page, text, to_library));
        cx.notify();
    }

    /// 加到导图: what is selected becomes a node of the paper's mind map, pointing back here.
    pub fn map_selection(&mut self, cx: &mut Context<Self>) {
        let Some((page, text)) = self.selected() else { return };
        self.selection = None;
        cx.emit(PdfEvent::MapPassage(page, text));
        cx.notify();
    }

    /// Select glyphs `from` to `to` of a 0-based page, as a drag would; nothing when that page's glyphs are not at hand.
    pub fn select_marks(&mut self, page: usize, from: usize, to: usize, cx: &mut Context<Self>) -> bool {
        let Some(marks) = self.marks.get(&page) else { return false };
        if marks.is_empty() { return false }
        let last = marks.len() - 1;
        (self.selection, self.selecting, self.translation, self.cut) = (Some(Selection { page, anchor: from.min(last), focus: to.min(last) }), false, None, None);
        self.held = self.active;
        cx.notify();
        true
    }

    /// Hand the selected text to the window to be read aloud, and let go of it: the sentence heard is marked instead.
    pub fn read_selection_aloud(&mut self, cx: &mut Context<Self>) {
        let Some((page, text)) = self.selected() else { return };
        self.selection = None;
        cx.emit(PdfEvent::ReadAloud(page, text));
        cx.notify();
    }

    /// Translate the selected text, with the model service `profile` for this once or the usual one.
    pub fn translate(&mut self, profile: Option<String>, cx: &mut Context<Self>) {
        let (Some(selection), Some((_, text))) = (self.selection, self.selected()) else { return };
        // The model on show stays until the window says which one was asked.
        let model = self.translation.take().map(|t| t.model).unwrap_or_default();
        self.translation = Some(Translation { selection, text: text.clone(), profile: profile.clone(), state: Translated::Busy, model, menu: false });
        cx.emit(PdfEvent::Translate(text, profile));
        cx.notify();
    }

    /// The text of the translation card on show, and the model asked for it alone, for the window to say how it stands.
    pub fn translation_asked(&self) -> Option<(String, Option<String>)> {
        self.translation.as_ref().map(|t| (t.text.clone(), t.profile.clone()))
    }

    /// How the card's translation stands, the model it is made with (id and name), and the models it can be asked of instead.
    pub fn show_translation(&mut self, state: Translated, model: (String, String), models: Vec<(String, String)>, cx: &mut Context<Self>) {
        let Some(card) = self.translation.as_mut() else { return };
        (card.state, card.model) = (state, model);
        self.models = models;
        cx.notify();
    }

    fn copy_translation(&mut self, cx: &mut Context<Self>) {
        let Some(Translated::Done(text)) = self.translation.as_ref().map(|t| &t.state) else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        cx.emit(PdfEvent::Copied);
    }

    /// The translation of the selection, on a card under it (over it when the room is there), kept inside the viewer.
    fn translation_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.selecting { return None }
        let card = self.translation.as_ref().filter(|t| Some(t.selection) == self.selection)?;
        let selection = card.selection;
        let boxes = pdftext::mark_boxes(self.marks.get(&selection.page)?.get(selection.anchor.min(selection.focus)..=selection.anchor.max(selection.focus))?);
        let (first, last) = (boxes.first()?, boxes.last()?);
        let scale = self.scale_of(selection.page);
        let (x, y) = self.sheet_place(selection.page)?;
        let (left, right) = boxes.iter().fold((f32::MAX, f32::MIN), |(l, r), line| (l.min(line[0]), r.max(line[2])));
        let place = card_place([x + left * scale, y + first[1] * scale, x + right * scale, y + last[3] * scale], CARD_W, CARD_TALL, (self.width, self.height));
        let button = |id: &'static str, label: &'static str| div().id(id).flex().flex_none().items_center().h(text_px(24.)).px(px(8.)).rounded(px(6.)).cursor_pointer().text_color(Theme::accent()).hover(|s| s.bg(Theme::hover())).child(label);
        let body = match &card.state {
            Translated::Busy => div().flex().items_center().gap(px(8.)).text_color(Theme::text_secondary()).child(ui::spinner("pdf-translation-busy", 14., Theme::text_tertiary())).child(t("正在翻译…")),
            Translated::Done(text) => div().text_color(Theme::text()).child(text.clone()),
            Translated::Failed(error) => div().text_color(Theme::danger()).child(error.clone()),
        };
        let chosen = &card.model.0;
        let menu = card.menu.then(|| {
            let rows: Vec<AnyElement> = self
                .models
                .iter()
                .enumerate()
                .map(|(n, (id, name))| {
                    let id = id.clone();
                    ui::menu_row(("pdf-translation-with", n), name.clone(), id == *chosen, None).on_click(cx.listener(move |this, _, _, cx| this.translate(Some(id.clone()), cx))).into_any_element()
                })
                .collect();
            div()
                .flex()
                .flex_col()
                .flex_none()
                .p(px(4.))
                .border_b_1()
                .border_color(Theme::line())
                .text_size(text_px(13.))
                .child(div().px(px(8.)).py(px(4.)).text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(t("只这一次换用，不改设置")))
                .when(rows.is_empty(), |d| d.child(div().px(px(8.)).py(px(6.)).text_color(Theme::text_secondary()).child(t("还没有配置模型：在 设置 › AI 里添加。"))))
                .children(rows)
        });
        let footer = match &card.state {
            Translated::Done(_) => Some(button("pdf-translation-copy", t("复制译文")).on_click(cx.listener(|this, _, _, cx| this.copy_translation(cx)))),
            Translated::Failed(_) => Some(button("pdf-translation-retry", t("重试")).on_click(cx.listener(|this, _, _, cx| {
                let profile = this.translation.as_ref().and_then(|t| t.profile.clone());
                this.translate(profile, cx);
            }))),
            Translated::Busy => None,
        };
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .h(text_px(34.))
            .pl(px(12.))
            .pr(px(4.))
            .border_b_1()
            .border_color(Theme::line())
            .child(div().flex_1().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(t("译文")))
            .child(
                div()
                    .id("pdf-translation-model")
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(4.))
                    .h(text_px(24.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(text_px(12.))
                    .text_color(Theme::text_secondary())
                    .hover(|s| s.bg(Theme::hover()))
                    .when(card.menu, |d| d.bg(Theme::pressed()))
                    .child(div().max_w(px(180.)).truncate().child(card.model.1.clone()))
                    .child(ui::icon("chevron-down", 12., Theme::text_tertiary()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(card) = this.translation.as_mut() { card.menu = !card.menu }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("pdf-translation-close")
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(Theme::hover()))
                    .child(ui::icon("x", 13., Theme::text_tertiary()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.translation = None;
                        cx.notify();
                    })),
            );
        Some(
            div()
                .id("pdf-translation")
                .absolute()
                .left(px(place.left))
                .when_some(place.top, |d, top| d.top(px(top)))
                .when_some(place.bottom, |d, bottom| d.bottom(px(bottom)))
                .w(px(place.width))
                .max_h(px(place.max_height))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(10.))
                .bg(Theme::bg_body())
                .border_1()
                .border_color(Theme::line_strong())
                .shadow(crate::theme::shadow_card())
                .font(crate::theme::ui_font())
                .text_size(text_px(13.))
                .line_height(text_px(20.))
                .occlude()
                // A press on the card is not a press on the page.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(header)
                .children(menu)
                .child(div().id("pdf-translation-text").flex_shrink().min_h_0().overflow_y_scroll().px(px(12.)).py(px(10.)).child(body))
                .children(footer.map(|button| div().flex().flex_none().justify_end().px(px(4.)).pb(px(4.)).child(button)))
                .into_any_element(),
        )
    }

    /// What can be done with the selection, beside where it ends.
    fn selection_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.selecting { return None }
        // The selection's translation takes the bar's place while it is on show.
        if self.cut.is_none() && self.translation.as_ref().is_some_and(|t| Some(t.selection) == self.selection) { return None }
        // What the bar is about: the piece cut out of a page, or the text selected; and the box it stands under.
        let (page, last, piece) = match (self.cut, self.selection) {
            (Some((page, [x0, y0, x1, y1])), _) => (page, [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)], true),
            (None, Some(selection)) => {
                let marks = self.marks.get(&selection.page)?;
                (selection.page, pdftext::mark_boxes(marks.get(selection.anchor.min(selection.focus)..=selection.anchor.max(selection.focus))?).pop()?, false)
            }
            (None, None) => return None,
        };
        let scale = self.scale_of(page);
        let (x, y) = self.sheet_place(page)?;
        // Under the last line of the selection, kept inside the viewer.
        let (left, top) = (x + last[0] * scale, y + last[3] * scale + 6.);
        let button = |id: &'static str, label: &'static str| div().id(id).flex().flex_none().items_center().h(text_px(26.)).px(px(9.)).rounded(px(6.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).child(label);
        Some(
            div()
                .absolute()
                .left(px(left.clamp(8., (self.width - 520.).max(8.))))
                .top(px(top.clamp(8., (self.height - 150.).max(8.))))
                .max_w(px((self.width - 16.).max(0.)))
                .max_h(px((self.height - 16.).clamp(0., 140.)))
                .id("pdf-selection-bar")
                .overflow_y_scroll()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(2.))
                .p(px(3.))
                .rounded(px(8.))
                .bg(Theme::bg_body())
                .border_1()
                .border_color(Theme::line_strong())
                .shadow(crate::theme::shadow_card())
                .text_size(text_px(12.))
                .occlude()
                // A press on the bar is not a press on the page.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .when(!piece, |d| d.child(button("pdf-copy", t("复制")).on_click(cx.listener(|this, _, _, cx| this.copy(cx)))))
                .when(!piece, |d| d.child(button("pdf-underline", t("下划线")).on_click(cx.listener(|this, _, _, cx| this.annotate(Kind::Underline, cx)))).child(button("pdf-highlight", t("高亮")).on_click(cx.listener(|this, _, _, cx| this.annotate(Kind::Highlight, cx)))))
                .when(!piece && self.library_chat, |d| d.child(button("pdf-quote-library", t("加到文献库对话")).text_color(Theme::accent()).on_click(cx.listener(|this, _, _, cx| this.quote(true, cx)))).child(button("pdf-quote-paper", t("加到这篇的对话")).text_color(Theme::accent()).on_click(cx.listener(|this, _, _, cx| this.quote(false, cx)))))
                .when(!piece && !self.library_chat, |d| d.child(button("pdf-quote-paper", t("添加到对话")).text_color(Theme::accent()).on_click(cx.listener(|this, _, _, cx| this.quote(false, cx)))))
                .when(!piece, |d| d.child(button("pdf-translate", t("翻译")).on_click(cx.listener(|this, _, _, cx| this.translate(None, cx)))))
                .when(!piece && self.maps, |d| d.child(button("pdf-to-map", t("加到导图")).on_click(cx.listener(|this, _, _, cx| this.map_selection(cx)))))
                .when(!piece, |d| d.child(button("pdf-read-aloud", t("朗读")).on_click(cx.listener(|this, _, _, cx| this.read_selection_aloud(cx)))))
                // A piece of the page: a figure to keep, or a picture for the next question.
                .when(piece, |d| d.child(button("pdf-cut-keep", t("收藏到图片库")).text_color(Theme::accent()).on_click(cx.listener(|this, _, _, cx| this.collect_cut(cx)))))
                .when(piece && self.library_chat, |d| d.child(button("pdf-cut-library", t("截图加到文献库对话")).on_click(cx.listener(|this, _, _, cx| this.send_cut(true, cx)))).child(button("pdf-cut-paper", t("截图加到这篇的对话")).on_click(cx.listener(|this, _, _, cx| this.send_cut(false, cx)))))
                .when(piece && !self.library_chat, |d| d.child(button("pdf-cut-paper", t("用这张截图提问")).on_click(cx.listener(|this, _, _, cx| this.send_cut(false, cx)))))
                // Or left on the page as a mark, to come back to.
                .when(piece, |d| d.child(button("pdf-cut-mark", t("标记")).tooltip(ui::tip(t("在页面上留一圈虚线，之后点它再收藏或提问"))).on_click(cx.listener(|this, _, _, cx| this.mark_cut(cx)))))
                .when(piece, |d| d.child(button("pdf-cut-cancel", t("取消")).on_click(cx.listener(|this, _, _, cx| this.toggle_cutting(cx)))))
                .into_any_element(),
        )
    }

    /// While a mark or the figure's frame is held, a layer over the whole window hears the pointer: an element
    /// hears only the moves made over itself, and a handle is often dragged past the viewer's edge. It keeps the
    /// pointer's look of what is done (a closed hand moving, the arrows of the way it is sized) wherever it goes.
    fn drag_layer(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let grab = self.shape_drag.as_ref().or(self.frame_drag.as_ref())?.grab;
        let size = window.viewport_size();
        let layer = div()
            .id("pdf-drag-layer")
            .w(size.width)
            .h(size.height)
            .occlude()
            .cursor(grab_cursor(grab))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| this.drag(event, window, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, window, cx| this.release(Some(event.position), window, cx)));
        Some(deferred(anchored().position(point(px(0.), px(0.))).child(layer)).with_priority(3).into_any_element())
    }

    /// What can be done with a highlight clicked, under the line clicked (over it at the viewer's foot):
    /// its colours, the one it has ringed, and 删除高亮. The same small bar as the selection's. A piece marked
    /// to be cut out has what a piece just cut has instead of colours: the figure library, a question.
    /// What a view shows, at its top left corner while there is more than one view: 原文 or 译文. It is also
    /// where the view is changed: its menu (`view_menu`).
    fn view_label(&self, view: usize, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let shown = self.shown();
        if shown.len() < 2 { return None }
        let shows = shown.get(view)?.shows;
        let open = self.view_menu == Some(view);
        Some(
            div()
                .id(("pdf-view-label", view))
                .absolute()
                .top(px(6.))
                .left(px(6.))
                .flex()
                .items_center()
                .gap(px(2.))
                .h(text_px(20.))
                .pl(px(7.))
                .pr(px(4.))
                .rounded(px(5.))
                .bg(if open { Theme::pressed() } else { Theme::bg_body() })
                .border_1()
                .border_color(if self.carrying.is_some_and(|(held, moved)| held == view && moved) { Theme::accent() } else { Theme::line() })
                .text_size(text_px(11.))
                .text_color(if shows == Shows::Pages { Theme::text_secondary() } else { Theme::accent() })
                .cursor_pointer()
                .hover(|s| s.bg(Theme::hover()))
                .occlude()
                .child(t(match shows { Shows::Pages => "原文", Shows::Theirs => "译文", Shows::Map => "导图" }))
                .child(ui::icon("chevron-down", 10., Theme::text_tertiary()))
                .tooltip(ui::tip(t("点击更改，拖动换位")))
                // A press on the label is not a press on the page under it: it takes hold of the view.
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    (this.carrying, this.carried) = (Some((view, false)), false);
                }))
                // Let go where it was pressed, it opens its menu; carried to another place, it has done its work.
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.carrying = None;
                    if std::mem::take(&mut this.carried) { return cx.notify() }
                    this.view_menu = if this.view_menu == Some(view) { None } else { Some(view) };
                    cx.notify();
                })),
        )
    }

    /// The menu of a view's label, under it: what the view shows, its place among the views, and its close.
    fn view_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = self.view_menu?;
        let shown = self.shown();
        let here = *shown.get(view)?;
        let (left, top) = pane_origin(self.split, (self.width, self.height), view);
        let stacked = self.split == Split::Stacked;
        let line = || div().flex_none().h(px(1.)).mx(px(6.)).my(px(4.)).bg(Theme::line());
        let pages = ui::menu_row("pdf-view-pages", t("显示原文"), here.shows == Shows::Pages, None).on_click(cx.listener(move |this, _, _, cx| this.show_in_view(view, Shows::Pages, cx)));
        // The map where the owner can give one: one of them, and not in place of the last view of the paper.
        let room = here.shows == Shows::Map || (!self.map_shown() && shown.iter().filter(|other| other.shows != Shows::Map).count() > 1);
        let map = (self.maps && room).then(|| ui::menu_row("pdf-view-map", t("显示导图"), here.shows == Shows::Map, None).on_click(cx.listener(move |this, _, _, cx| this.show_in_view(view, Shows::Map, cx))));
        // A translation is offered where the owner can start one, or one is on show already.
        let theirs = (self.translates || self.facing.is_some()).then(|| ui::menu_row("pdf-view-theirs", t("显示译文"), here.shows == Shows::Theirs, None).on_click(cx.listener(move |this, _, _, cx| this.show_in_view(view, Shows::Theirs, cx))));
        let before = (view > 0).then(|| ui::menu_row("pdf-view-before", t(if stacked { "移到上面" } else { "移到左边" }), false, None).on_click(cx.listener(move |this, _, _, cx| this.move_view(view, view - 1, cx))));
        let after = (view + 1 < shown.len()).then(|| ui::menu_row("pdf-view-after", t(if stacked { "移到下面" } else { "移到右边" }), false, None).on_click(cx.listener(move |this, _, _, cx| this.move_view(view, view + 1, cx))));
        let closes = here.shows == Shows::Map || shown.iter().filter(|other| other.shows != Shows::Map).count() > 1;
        let close = ui::menu_row("pdf-view-close", t(if stacked { "关闭这一半" } else { "关闭这一栏" }), false, None).on_click(cx.listener(move |this, _, _, cx| this.close_view(view, cx)));
        const WIDTH: f32 = 168.;
        Some(
            ui::popover()
                .id("pdf-view-menu")
                .absolute()
                .left(px((left + 6.).min((self.width - WIDTH - 8.).max(8.))))
                .top(px(top + 6. + 20. + 6.))
                .w(px(WIDTH))
                .p(px(5.))
                // A press on the menu is not a press on the page; one anywhere else puts it away.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| if this.view_menu.take().is_some() { cx.notify() }))
                .child(pages)
                .children(theirs)
                .children(map)
                .child(line())
                .children(before)
                .children(after)
                .when(closes, |d| d.child(line()).child(close))
                .into_any_element(),
        )
    }

    fn mark_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // A chosen mark's menu stands under it where it is now, clear of its handles, and steps aside while it is dragged.
        let (menu, gap) = match (&self.mark_menu, self.chosen_mark()) {
            (Some(menu), _) => (menu.clone(), 6.),
            (None, Some(a)) if self.shape_drag.as_ref().is_none_or(|held| !held.moved) => {
                let [x0, y0, _, y1] = self.rect_of(a);
                (MarkMenu { id: a.id.clone(), page: a.page.checked_sub(1)?, x: x0, line: (y0, y1) }, HANDLE + 4.)
            }
            _ => return None,
        };
        let mark = self.annotations.iter().find(|a| a.id == menu.id)?;
        let scale = self.scale_of(menu.page);
        let (x, y) = self.sheet_place(menu.page)?;
        const HEIGHT: f32 = 34.;
        if mark.kind == Kind::Text {
            let open = self.text_palette.as_ref().filter(|(id, _)| *id == mark.id).map(|(_, part)| *part);
            let tall = if open.is_some() { HEIGHT * 2. } else { HEIGHT };
            let below = y + menu.line.1 * scale + gap;
            let top = if below + tall + 8. <= self.height { below } else { y + menu.line.0 * scale - gap - tall };
            let left = (x + menu.x * scale).clamp(8., (self.width - 400.).max(8.));
            return Some(self.text_menu(mark, open, left, top.clamp(8., (self.height - tall - 8.).max(8.)), cx));
        }
        let marked = mark.kind == Kind::Cut;
        let button = |id: &'static str, label: &'static str| div().id(id).flex().flex_none().items_center().h(text_px(26.)).px(px(9.)).rounded(px(6.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).child(label);
        let below = y + menu.line.1 * scale + gap;
        let top = if below + HEIGHT + 8. <= self.height { below } else { y + menu.line.0 * scale - gap - HEIGHT };
        let swatches = COLORS.iter().zip(COLOR_NAMES).map(|(&key, name)| {
            div()
                .id(SharedString::from(format!("pdf-mark-{key}")))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(26.))
                .rounded_full()
                .cursor_pointer()
                .hover(|s| s.bg(Theme::hover()))
                .when(mark.color == key, |d| d.border_2().border_color(Theme::text_secondary()))
                .child(div().size(px(14.)).rounded_full().bg(Theme::highlight_swatch(key)))
                .tooltip(ui::tip(t(name)))
                .on_click(cx.listener(move |this, _, _, cx| this.recolor(key, cx)))
        });
        Some(
            div()
                .id("pdf-mark-menu")
                .absolute()
                .left(px((x + menu.x * scale - if marked { 0. } else { 16. }).clamp(8., (self.width - if marked { 420. } else { 240. }).max(8.))))
                .top(px(top.clamp(8., (self.height - HEIGHT - 8.).max(8.))))
                .flex()
                .items_center()
                .gap(px(1.))
                .p(px(3.))
                .rounded(px(8.))
                .bg(Theme::bg_body())
                .border_1()
                .border_color(Theme::line_strong())
                .shadow(crate::theme::shadow_card())
                .text_size(text_px(12.))
                .occlude()
                // A press on the menu is not a press on the page; one anywhere else puts it away.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| if this.mark_menu.take().is_some() { cx.notify() }))
                .when(!marked, |d| d.children(swatches))
                .when(marked, |d| d.child(button("pdf-mark-keep", t("收藏到图片库")).text_color(Theme::accent()).on_click(cx.listener(|this, _, _, cx| this.collect_mark(cx)))))
                .when(marked && self.library_chat, |d| d.child(button("pdf-mark-library", t("截图加到文献库对话")).on_click(cx.listener(|this, _, _, cx| this.send_mark(true, cx)))).child(button("pdf-mark-paper", t("截图加到这篇的对话")).on_click(cx.listener(|this, _, _, cx| this.send_mark(false, cx)))))
                .when(marked && !self.library_chat, |d| d.child(button("pdf-mark-paper", t("用这张截图提问")).on_click(cx.listener(|this, _, _, cx| this.send_mark(false, cx)))))
                .child(div().flex_none().w(px(1.)).h(px(16.)).mx(px(4.)).bg(Theme::line()))
                .child(div().id("pdf-mark-remove").flex().flex_none().items_center().h(text_px(26.)).px(px(9.)).rounded(px(6.)).cursor_pointer().text_color(Theme::danger()).hover(|s| s.bg(Theme::hover())).child(match mark.kind { Kind::Box => t("删除方框"), Kind::Cut => t("删除标记"), _ => t("删除高亮") }).on_click(cx.listener(|this, _, _, cx| this.remove_mark(cx))))
                .into_any_element(),
        )
    }

    /// The menu of a text box chosen: its three colours, each a chip that opens its swatches in a row under it,
    /// writing in it again, and 删除文本框.
    fn text_menu(&self, mark: &Annotation, open: Option<TextPart>, left: f32, top: f32, cx: &mut Context<Self>) -> AnyElement {
        let style = mark.style.clone().unwrap_or_default();
        let button = |id: &'static str, label: &'static str| div().id(id).flex().flex_none().items_center().h(text_px(26.)).px(px(9.)).rounded(px(6.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).child(label);
        let rule = || div().flex_none().w(px(1.)).h(px(16.)).mx(px(4.)).bg(Theme::line());
        let parts = TextPart::ALL.into_iter().map(|part| {
            let key = style.get(part).to_string();
            div().id(SharedString::from(format!("pdf-text-{}", text_part_name(part))))
                .flex().flex_none().items_center().gap(px(5.)).h(text_px(26.)).px(px(7.)).rounded(px(6.)).cursor_pointer().hover(|s| s.bg(Theme::hover()))
                .when(open == Some(part), |d| d.bg(Theme::hover()))
                .child(text_swatch(part, &key, 14.)).child(t(text_part_name(part))).child(ui::icon("chevron-down", 12., Theme::text_tertiary()))
                .tooltip(ui::tip(tf!("{}颜色：{}", "{} colour: {}", t(text_part_name(part)), t(text_color_name(&key)))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let Some(id) = this.chosen.clone() else { return };
                    this.text_palette = if this.text_palette.as_ref().is_some_and(|(_, was)| *was == part) { None } else { Some((id, part)) };
                    cx.notify();
                }))
        });
        let palette = open.map(|part| {
            let now = style.get(part).to_string();
            div().flex().items_center().gap(px(1.)).pt(px(3.)).mt(px(2.)).border_t_1().border_color(Theme::line()).children(part.keys().iter().map(|&key| {
                div().id(SharedString::from(format!("pdf-text-{key}"))).flex().flex_none().items_center().justify_center().size(px(26.)).rounded_full().cursor_pointer().hover(|s| s.bg(Theme::hover()))
                    .when(now == key, |d| d.border_2().border_color(Theme::text_secondary()))
                    .child(text_swatch(part, key, 16.)).tooltip(ui::tip(t(text_color_name(key))))
                    .on_click(cx.listener(move |this, _, _, cx| this.restyle_text(part, key, cx)))
            }))
        });
        let (id, page, rect) = (mark.id.clone(), mark.page.saturating_sub(1), self.rect_of(mark));
        div()
            .id("pdf-mark-menu")
            .absolute()
            .left(px(left))
            .top(px(top))
            .flex()
            .flex_col()
            .p(px(3.))
            .rounded(px(8.))
            .bg(Theme::bg_body())
            .border_1()
            .border_color(Theme::line_strong())
            .shadow(crate::theme::shadow_card())
            .text_size(text_px(12.))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().flex().items_center().gap(px(1.))
                .children(parts)
                .child(rule())
                .child(button("pdf-text-write", t("编辑文字")).on_click(cx.listener(move |this, _, window, cx| this.start_text(TextEdit { id: Some(id.clone()), page, rect }, window, cx))))
                .child(rule())
                .child(button("pdf-mark-remove", t("删除文本框")).text_color(Theme::danger()).on_click(cx.listener(|this, _, _, cx| this.remove_mark(cx)))))
            .children(palette)
            .into_any_element()
    }

    /// A bounded, scrollable list also lets a mark be found after reopening
    /// the file, without depending on a hit target over the PDF text.
    fn annotation_panel(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let button = |id, label: &'static str| div().id(id).px(px(8.)).py(px(4.)).rounded(px(5.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).child(label);
        let width = (self.width - 24.).clamp(0., 360.);
        let list = self.annotations_open.then(|| {
            let rows = self.annotations.iter().map(|annotation| {
                let (id, edit_id, remove_id) = (annotation.id.clone(), annotation.id.clone(), annotation.id.clone());
                let (page, top) = (annotation.page, annotation.boxes.first().map(|b| b[1]).unwrap_or(0.));
                let note = annotation.note.clone();
                let editing = self.annotation_edit.as_deref() == Some(id.as_str());
                let kind = match annotation.kind { Kind::Underline => t("下划线"), Kind::Highlight => t("高亮"), Kind::Box => t("方框"), Kind::Note => t("便签"), Kind::Cut => t("截图标记"), Kind::Text => t("文本框") };
                div().id(SharedString::from(id)).p(px(10.)).border_b_1().border_color(Theme::line()).flex().flex_col().gap(px(7.))
                    .child(div().flex().flex_wrap().items_center().gap(px(4.))
                        .children(matches!(annotation.kind, Kind::Highlight | Kind::Box).then(|| div().flex_none().size(px(8.)).mr(px(2.)).rounded_full().bg(Theme::highlight_swatch(&annotation.color))))
                        .child(div().flex_1().text_color(Theme::text_secondary()).child(tf!("第 {page} 页 · {kind}", "Page {page} · {kind}")))
                        .child(button("annotation-locate", t("定位")).on_click(cx.listener(move |this, _, _, cx| this.go_to_place(page, top, cx))))
                        .child(button("annotation-edit", if annotation.kind == Kind::Text { t("文字") } else { t("笔记") }).on_click(cx.listener(move |this, _, _, cx| {
                            this.annotation_edit = Some(edit_id.clone());
                            this.annotation_note.update(cx, |input, cx| input.set_text(&note, cx));
                            cx.notify();
                        })))
                        .child(button("annotation-delete", t("删除")).on_click(cx.listener(move |this, _, _, cx| cx.emit(PdfEvent::RemoveAnnotation { resource: this.resource_id.clone(), id: remove_id.clone() })))))
                    .when(!annotation.quote.is_empty(), |d| d.child(div().max_h(px(100.)).overflow_hidden().child(annotation.quote.clone())))
                    .when(!editing && !annotation.note.is_empty(), |d| d.child(div().text_color(Theme::text_secondary()).child(annotation.note.clone())))
                    .when(editing, |d| d.child(ui::field(&self.annotation_note, window, cx)).child(div().flex().gap(px(5.))
                        .child(button("annotation-save", t("保存笔记")).text_color(Theme::accent()).on_click(cx.listener(|this, _, _, cx| {
                            if let Some(id) = this.annotation_edit.clone() { cx.emit(PdfEvent::AnnotationNote { resource: this.resource_id.clone(), id, note: this.annotation_note.read(cx).text().to_string() }) }
                        })))
                        .child(button("annotation-cancel", t("取消")).on_click(cx.listener(|this, _, _, cx| { this.annotation_edit = None; cx.notify(); })))))
            });
            div().id("pdf-annotations-list").w_full().max_h(px((self.height - 70.).max(0.))).overflow_y_scroll().bg(Theme::bg_body()).border_1().border_color(Theme::line_strong()).rounded(px(8.))
                .when(self.annotations.is_empty(), |d| d.child(div().p(px(12.)).text_color(Theme::text_secondary()).child(t("选中 PDF 文字后，可添加下划线或高亮；「方框」「便签」「文本框」三个工具可以框出区域、在任意位置记笔记，或直接把字写在页面上。"))))
                .children(rows)
                // The marks go with a copy of the file, for other readers and other people; pieces marked to be cut out stay here.
                .when(self.has_exported_marks(), |d| d.child(div().p(px(6.)).flex().child(button("annotation-export", t("导出带标注的 PDF…")).text_color(Theme::accent()).on_click(cx.listener(|_, _, _, cx| cx.emit(PdfEvent::ExportMarked))))))
        });
        // In from the right edge by the width of the bar down the pages, which stands beside it.
        div().absolute().top(px(8.)).right(px(crate::scrollbar::STRIP + 4.)).max_w(px(width)).flex().flex_col().items_end().gap(px(6.)).text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text()).occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().id("pdf-annotations-toggle").px(px(10.)).py(px(5.)).rounded(px(6.)).bg(Theme::bg_body()).border_1().border_color(Theme::line_strong()).cursor_pointer()
                .child(tf!("标注 ({})", "Annotations ({})", self.annotations.len()))
                .on_click(cx.listener(|this, _, _, cx| { this.annotations_open = !this.annotations_open; cx.notify(); })))
            .children(list.map(|list| div().w(px(width)).child(list)))
            .into_any_element()
    }

    /// The viewer, `width` logical pixels wide.
    pub fn set_width(&mut self, width: f32) {
        if (self.width - width).abs() > 0.5 { self.resize((0., 0.), |this| this.width = width) }
    }

    /// The translation the viewer placed itself stands beside the pages where its width has room for both, else
    /// under them: the layout changed (版面, a pane beside) since it was placed (#194).
    pub fn fit_facing(&mut self, cx: &mut Context<Self>) {
        if !self.facing_beside() || !self.facing_auto { return }
        let split = if self.width >= crate::facing::FACING_MIN { Split::Side } else { Split::Stacked };
        if split != self.split { self.set_split(split, cx) }
    }

    /// How the person split the viewer in 分屏: theirs from now on, the translation's half too.
    pub fn choose_split(&mut self, split: Split, cx: &mut Context<Self>) {
        self.facing_auto = false;
        // A translation left alone by its label (its pages closed) gets its pages back to stand beside.
        if split != Split::Off && self.facing.is_some() && !self.in_place && !self.facing_beside() && !self.full() {
            self.drop_translation(cx);
            self.add_translation(cx);
            self.facing_auto = false;
        }
        self.set_split(split, cx);
    }

    /// The viewer's height, as it was laid out.
    fn set_height(&mut self, height: f32, cx: &mut Context<Self>) {
        if (self.height - height).abs() <= 0.5 { return }
        self.resize((0., 0.), |this| this.height = height);
        cx.notify();
    }
}

/// A colour of a text box to choose it by, `size` across: the fill as a disc, the border as a ring, the words as
/// a letter; `none` an empty ring with a cross. Ringed in the menu's line, so white shows on a light menu and
/// black on a dark one.
fn text_swatch(part: TextPart, key: &str, size: f32) -> Div {
    let chip = div().flex().flex_none().items_center().justify_center().size(px(size)).rounded_full();
    if key == "none" { return chip.border_1().border_color(Theme::line_strong()).child(ui::icon("x", size * 0.7, Theme::text_tertiary())) }
    match part {
        TextPart::Fill => chip.bg(Theme::text_fill(key)).border_1().border_color(Theme::line_strong()),
        TextPart::Border => chip.border(px((size / 5.).round())).border_color(Theme::text_border(key)),
        TextPart::Ink => chip.bg(Theme::text_fill("white")).border_1().border_color(Theme::line_strong()).text_size(px(size * 0.72)).font_weight(FontWeight::BOLD).text_color(Theme::text_ink(key)).child("A"),
    }
}

/// A sticky note folded to its icon, at a spot of a page in screen pixels; `lit` while its card is open.
fn note_icon(left: f32, top: f32, lit: bool) -> Div {
    div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .size(px(NOTE_ICON))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .bg(Theme::note_paper())
        .border_1()
        .border_color(if lit { Theme::accent() } else { Theme::note_edge() })
        .when(lit, |d| d.border_2())
        .shadow(crate::theme::shadow_card())
        .child(ui::icon("sticky-note", 13., Theme::note_ink()))
}

impl Render for PdfView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(error) = &self.error {
            return ui::empty_state("image-broken", t("无法预览"), error).size_full().into_any_element();
        }
        if self.sizes.is_empty() {
            return ui::empty_state("file-text", t("正在打开 PDF…"), "").size_full().into_any_element();
        }
        self.device_scale = window.scale_factor();
        // A field put away (a text box left with Esc, a note saved with its button) gives the keyboard back to the
        // viewer: a focus left on a field no longer drawn would keep ⌘Z and Delete from the viewer.
        let put_away = |input: &Entity<TextInput>, drawn: bool| !drawn && input.focus_handle(cx).is_focused(window);
        if put_away(&self.text_input, self.text_edit.is_some()) || put_away(&self.note_input, self.note_editing) || put_away(&self.annotation_note, self.annotation_edit.is_some()) { window.focus(&self.focus) }
        // Views of one group are read as one (the page and its translation): the one last scrolled leads the
        // others. While a view of another group is worked in, they stay as they were left.
        let shown = self.shown();
        if self.active < shown.len() {
            self.led[shown[self.active].group as usize] = self.active;
        }
        for (pane, view) in shown.iter().enumerate() {
            // A lead that is no longer of this group (the views changed): its pages lead.
            let lead = Some(self.led[view.group as usize]).filter(|lead| shown.get(*lead).is_some_and(|lead| lead.group == view.group)).unwrap_or(view.group as usize);
            if lead == pane { continue }
            let (top, theirs) = (self.panes[lead].list.logical_scroll_top(), self.panes[pane].list.logical_scroll_top());
            if top.item_ix != theirs.item_ix || top.offset_in_item != theirs.offset_in_item { self.panes[pane].list.scroll_to(top) }
            self.panes[pane].scroll_x = self.panes[lead].scroll_x;
        }
        let (measured, wheel, slid, let_go) = (cx.entity(), cx.entity(), cx.entity(), cx.entity());
        let bars = [0, 1, 2].map(|pane| self.sideways_bar(pane, cx));
        let mut bars = bars.into_iter();
        let labels = [0, 1, 2].map(|pane| self.view_label(pane, cx));
        let mut labels = labels.into_iter();
        let view_menu = self.view_menu(cx);
        // A view's pages, with its sideways bar over their foot.
        let map = self.map().cloned();
        let mut pane = |pane: usize| {
            let entity = cx.entity();
            // The map is no list of pages: its own view, under the same label.
            if shown.get(pane).is_some_and(|view| view.shows == Shows::Map) {
                bars.next();
                return div().size_full().relative().children(map.clone()).children(labels.next().flatten());
            }
            let pages = list(self.panes[pane].list.clone(), move |index, _, cx| entity.update(cx, |this, cx| this.page(pane, index, cx))).size_full().pt(px(PAGE_GAP));
            // Up the right edge, the bar that goes down the pages; it stops above the sideways bar. Pressed, its view is the one worked in, and leads its group.
            let pressed = cx.entity();
            // Held, it says the page it has come to: the pages pass too fast to be read off themselves.
            let (said, count) = (self.panes[pane].list.clone(), self.sizes.len());
            let down = crate::scrollbar::scrollbar(("pdf-down", pane), self.panes[pane].list.clone()).saying(move || { let page = said.logical_scroll_top().item_ix + 1; tf!("{page} / {count} 页", "{page} / {count} pages").into() }).on_press(move |_, cx| pressed.update(cx, |this: &mut Self, cx| {
                this.active = pane;
                cx.emit(PdfEvent::Worked);
                cx.emit(PdfEvent::Pressed);
            }));
            let down = div().absolute().top_0().right_0().bottom(px(if self.max_x > 0. { BAR_HEIGHT + 2. } else { 0. })).w(px(crate::scrollbar::STRIP)).child(down);
            div().size_full().relative().child(pages).child(down).children(bars.next().flatten()).children(labels.next().flatten())
        };
        // The pages once, or twice with a line between: each half scrolls on its own.
        let pages = match self.split {
            Split::Off => pane(0).into_any_element(),
            split => {
                let half = |pane: Div| div().flex_1().min_w_0().min_h_0().overflow_hidden().child(pane);
                let line = || {
                    let line = div().flex_none().bg(Theme::line_strong());
                    if split == Split::Stacked { line.h(px(DIVIDER)).w_full() } else { line.w(px(DIVIDER)).h_full() }
                };
                let mut row = div().size_full().flex().when(split == Split::Stacked, |d| d.flex_col()).child(half(pane(0)));
                for next in 1..shown.len() { row = row.child(line()).child(half(pane(next))) }
                row.into_any_element()
            }
        };
        // A card for a selection that has since gone goes with it.
        if self.translation.as_ref().is_some_and(|t| Some(t.selection) != self.selection) { self.translation = None }
        let bar = self.selection_bar(cx);
        let card = self.translation_card(cx);
        let figure_card = self.figure_card(cx);
        let return_chip = self.return_chip(cx);
        let mark_menu = self.mark_menu(cx);
        let note_card = self.note_card(window, cx);
        let notice = match self.citation.status {
            CitationStatus::PageOnly => Some(t("此引用仅提供页码，已标出引用页。")),
            CitationStatus::Searching => Some(t("正在定位引用原文…")),
            CitationStatus::Missing => Some(t("已跳到引用的页，但没有在 PDF 里找到这句原文。")),
            CitationStatus::Partial => Some(t("只在 PDF 里找到这句引文的开头，已标出；其余部分与原文对不上，请自行核对。")),
            CitationStatus::None | CitationStatus::Found => None,
        };
        // Where the sentence was not found word for word, the person looks for it: 自己搜索 opens the find bar with a word of it.
        let missed = matches!(self.citation.status, CitationStatus::Missing | CitationStatus::Partial);
        // The find bar, once open, is what is said about it.
        let notice = notice.filter(|_| self.finder.is_none());
        let finder = self.finder_bar(window, cx);
        div()
            .id("pdf-viewer")
            .key_context("PdfView")
            .track_focus(&self.focus)
            .size_full()
            .bg(Theme::bg_sunken())
            .relative()
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &FindInPdf, window, cx| this.open_finder(None, false, window, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::press))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| this.drag(event, window, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, window, cx| this.release(Some(event.position), window, cx)))
            // Delete (or Backspace) removes the mark chosen, or the highlight whose menu is open; with neither, the key
            // goes on to the window (the paper selected to the trash). A text field of the viewer hears its own first.
            .on_action(cx.listener(|this, _: &RemoveChosen, _, cx| if this.menu_id().is_some() { this.remove_mark(cx) } else { cx.propagate() }))
            // The marks' undo; a text field of the viewer (a note, a text box being written) hears its own first.
            .on_action(cx.listener(|_, _: &UndoMark, _, cx| cx.emit(PdfEvent::StepMarks(true))))
            .on_action(cx.listener(|_, _: &RedoMark, _, cx| cx.emit(PdfEvent::StepMarks(false))))
            // Esc puts a highlight's menu away before it does anything else.
            // Then lets a chosen mark go, then a note's card, then a tool in use.
            .on_action(cx.listener(|this, _: &crate::shortcuts::Dismiss, window, cx| {
                if this.text_edit.is_some() {
                    this.finish_text(true, cx);
                } else if this.figure_card.take().is_some() || this.view_menu.take().is_some() || this.mark_menu.take().is_some() || this.text_palette.take().is_some() || this.chosen.take().is_some() {
                } else if this.note_open.is_some() || this.note_draft.is_some() {
                    this.close_note();
                } else if this.finder.is_some() {
                    this.close_finder(window);
                } else if this.tool != Tool::Select {
                    (this.tool, this.drawing) = (Tool::Select, None);
                } else {
                    return cx.propagate();
                }
                cx.notify();
            }))
            // Under the pages: it learns the viewer's height, and hears the wheel before the list does.
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |bounds, hitbox, window, cx| {
                        let height = f32::from(bounds.size.height);
                        window.defer(cx, move |_, cx| measured.update(cx, |this, cx| this.set_height(height, cx)));
                        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture && hitbox.is_hovered(window) && wheel.update(cx, |this, cx| this.wheel(event, bounds, cx)) { cx.stop_propagation() }
                        });
                        // A label carried to another place is let go wherever the button comes up; one let go where it
                        // was pressed is still held when its click is heard, and opens its menu.
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase != DispatchPhase::Capture { return }
                            let_go.update(cx, |this, cx| if this.carrying.is_some_and(|(_, moved)| moved) {
                                (this.carrying, this.carried) = (None, true);
                                cx.notify();
                            });
                        });
                        // A sideways bar held is heard wherever the pointer goes, until the button comes up.
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase != DispatchPhase::Capture { return }
                            slid.update(cx, |this, cx| {
                                // A view's label held is carried the same way.
                                if this.carrying.is_some() {
                                    if event.pressed_button == Some(MouseButton::Left) { this.carry(event.position, bounds, cx) } else { this.carrying = None; cx.notify() }
                                }
                                if this.sliding.is_none() { return }
                                if event.pressed_button == Some(MouseButton::Left) { this.slide(event.position.x, cx) } else { this.sliding = None; cx.notify() }
                            });
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(pages)
            // Explain whether the citation names a page or a passage within it.
            .children(bar)
            .children(card)
            .children(return_chip)
            .children(mark_menu)
            .children(view_menu)
            .children(note_card)
            .children(notice.map(|notice| {
                let search = ui::ghost_button("pdf-cite-search", Some("search"), Some(t("自己搜索").into())).h(text_px(22.)).px(px(6.)).text_size(text_px(12.)).text_color(Theme::accent()).on_click(cx.listener(|this, _, window, cx| {
                    let word = search_words(&this.citation.quote).into_iter().next().unwrap_or_default();
                    this.open_finder(Some(word), true, window, cx);
                }));
                div().absolute().top(px(8.)).left(px(8.)).right(px(FIND_CLEAR - 12.)).flex().justify_center().child(div().flex().items_center().gap(px(6.)).max_w(px((self.width - FIND_CLEAR - 4.).max(160.))).pl(px(10.)).pr(px(if missed { 4. } else { 10. })).py(px(if missed { 2. } else { 5. })).rounded(px(6.)).bg(Theme::bg_body()).border_1().border_color(Theme::line()).text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_secondary()).child(div().flex_shrink().min_w_0().child(notice)).when(missed, |d| d.child(search)))
            }))
            .children(finder)
            .child(self.annotation_panel(window, cx))
            // Over the notice and 标注's button: it is looked at and put away again.
            .children(figure_card)
            .children(self.drag_layer(window, cx))
            .into_any_element()
    }
}

/// Two boxes on a page are the same place: a mark and the figure kept from it, as rounded when they were saved.
fn same_place(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1.5)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's own `test` attribute.
    use super::{BAR_INSET, BAR_THUMB_MIN, CARD_EDGE, CardPlace, CitationFocus, CitationStatus, DIVIDER, MARGIN, PAGE_GAP, POINT_PX, Shows, Split, Zoom, bar_thumb, beside_split, card_place, dragged_share, HALF_MIN, page_scale, pane_at, pane_origin, pane_size, spare, views, picture_serves, render_scale, search_words, zoom_step};

    #[test]
    fn a_sentence_not_found_is_looked_for_by_its_words() {
        // The longer words first, each once, short ones left out.
        assert_eq!(search_words("Libraries were prepared with single-cell Chromium kits; libraries again."), ["single-cell", "Libraries", "prepared", "Chromium", "again", "were"]);
        assert_eq!(search_words("根系发育受到生长素的调控。"), ["根系发育", "受到生长", "素的调控"]);
        // A term in a Chinese sentence before the characters around it.
        assert_eq!(search_words("我们用 Arabidopsis 做了实验"), ["Arabidopsis", "我们用", "做了实验"]);
        assert!(search_words("").is_empty());
    }

    #[test]
    fn another_file_beside_keeps_how_the_halves_stand() {
        let (wide, narrow) = (crate::facing::FACING_MIN, crate::facing::FACING_MIN - 1.);
        // Two files already: the other half changes its file, not its place.
        assert_eq!(beside_split(Some(Split::Stacked), Split::Off, wide), Split::Stacked);
        // One file split on itself: its second half takes the other file, as it stood.
        assert_eq!((beside_split(None, Split::Side, narrow), beside_split(None, Split::Stacked, wide)), (Split::Side, Split::Stacked));
        // Whole: side by side where two pages have room, else one over the other.
        assert_eq!((beside_split(None, Split::Off, wide), beside_split(None, Split::Off, narrow)), (Split::Side, Split::Stacked));
    }

    #[test]
    fn the_band_between_free_halves_follows_the_pointer_and_leaves_each_some_room() {
        let room = 1000.;
        assert_eq!(dragged_share(0.5, 100., room), 0.6);
        assert_eq!(dragged_share(0.5, -250., room), 0.25);
        // Never narrower than `HALF_MIN`, either way.
        assert_eq!((dragged_share(0.5, -900., room), dragged_share(0.5, 900., room)), (HALF_MIN / room, 1. - HALF_MIN / room));
        // A viewer too small for two such halves splits even; one not laid out yet too.
        assert_eq!((dragged_share(0.5, 80., 2. * HALF_MIN - 1.), dragged_share(0.3, 10., 0.)), (0.5, 0.5));
    }

    #[test]
    fn a_split_viewer_has_two_equal_halves() {
        let view = (800. + DIVIDER, 600. + DIVIDER);
        assert_eq!(pane_size(Split::Off, view), view);
        assert_eq!((pane_size(Split::Side, view), pane_size(Split::Stacked, view)), ((400., view.1), (view.0, 300.)));
        // Before the viewer is laid out its height is not known: a half is not less than nothing.
        assert_eq!(pane_size(Split::Stacked, (800., 0.)), (800., 0.));
        // A point is in the first half up to the line between them, in the second past it.
        assert_eq!((pane_at(Split::Off, view, (700., 500.)), pane_at(Split::Side, view, (399., 500.)), pane_at(Split::Side, view, (400. + DIVIDER, 10.))), (0, 0, 1));
        assert_eq!((pane_at(Split::Stacked, view, (700., 299.)), pane_at(Split::Stacked, view, (10., 300. + DIVIDER))), (0, 1));
        // At a fit the pages of a half are as wide as the half, side by side; a percentage stays.
        let a4 = (595., 842.);
        assert_eq!(page_scale(Zoom::FitWidth, a4, pane_size(Split::Side, view)), (400. - 2. * MARGIN) / 595.);
        assert_eq!(page_scale(Zoom::Percent(150.), a4, pane_size(Split::Side, view)), page_scale(Zoom::Percent(150.), a4, view));
        // The whole page in each half: with the gap above and below it, a page is no taller and no wider than its half.
        for split in [Split::Off, Split::Side, Split::Stacked] {
            let half = pane_size(split, view);
            let scale = page_scale(Zoom::FitPage, a4, half);
            assert!(a4.1 * scale + 2. * PAGE_GAP <= half.1 + 0.01 && a4.0 * scale + 2. * MARGIN <= half.0 + 0.01, "{split:?}");
        }
        // One over the other, the height decides, and the page fills it.
        assert!((page_scale(Zoom::FitPage, a4, pane_size(Split::Stacked, view)) * 842. + 2. * PAGE_GAP - 300.).abs() < 0.01);
    }

    #[test]
    fn three_columns_are_equal_and_a_point_is_in_one() {
        let view = (900. + 2. * DIVIDER, 600.);
        assert_eq!(pane_size(Split::Triple, view), (300., 600.));
        assert_eq!([0, 1, 2].map(|pane| pane_origin(Split::Triple, view, pane)), [(0., 0.), (300. + DIVIDER, 0.), (600. + 2. * DIVIDER, 0.)]);
        assert_eq!([299., 300. + DIVIDER, 599. + DIVIDER, 600. + 2. * DIVIDER, 899.].map(|x| pane_at(Split::Triple, view, (x, 10.))), [0, 1, 1, 2, 2]);
        // The second view of the other ways starts past the line between the two.
        let two = (800. + DIVIDER, 600. + DIVIDER);
        assert_eq!((pane_origin(Split::Off, two, 0), pane_origin(Split::Side, two, 1), pane_origin(Split::Stacked, two, 1)), ((0., 0.), (400. + DIVIDER, 0.), (0., 300. + DIVIDER)));
        // Too narrow for anything: not less than nothing.
        assert_eq!(pane_size(Split::Triple, (4., 600.)).0, 0.);
    }

    #[test]
    fn views_say_what_is_shown_and_what_scrolls_together() {
        let groups = |shows: &[Shows]| views(shows).iter().map(|view| view.group).collect::<Vec<_>>();
        let (p, t, m) = (Shows::Pages, Shows::Theirs, Shows::Map);
        // Once, and split on itself: each half at its own place.
        assert_eq!((groups(&[p]), groups(&[p, p])), (vec![0], vec![0, 1]));
        // 对照: a translation scrolls with the pages next to it, whichever side they stand on.
        assert_eq!((groups(&[p, t]), groups(&[t, p])), (vec![0, 0], vec![1, 1]));
        // Three columns: the pair, and the pages again scrolled on their own, before or after the pair.
        assert_eq!((groups(&[p, t, p]), groups(&[p, p, t]), groups(&[t, p, p])), (vec![0, 0, 2], vec![0, 1, 1], vec![1, 1, 2]));
        // Between two, a translation goes with the pages before it.
        assert_eq!(groups(&[p, t, t]), vec![0, 0, 2]);
        // 原地替换: every view reads the translation, each at its own place.
        assert_eq!((groups(&[t]), groups(&[t, t])), (vec![0], vec![0, 1]));
        // The map scrolls on its own wherever it stands, and a translation does not take it for its pages.
        assert_eq!((groups(&[p, m]), groups(&[p, t, m]), groups(&[m, t, p]), groups(&[t, m])), (vec![0, 1], vec![0, 0, 2], vec![0, 2, 2], vec![0, 1]));
        // Never more views than a viewer has.
        assert_eq!(views(&[p, t, p, t]).len(), 3);
        assert_eq!((Split::Off.views(), Split::Side.views(), Split::Stacked.views(), Split::Triple.views()), (1, 2, 2, 3));
    }

    #[test]
    fn the_view_that_goes_is_the_one_scrolled_on_its_own() {
        let (p, t, m) = (Shows::Pages, Shows::Theirs, Shows::Map);
        // Beside a pair, the pages scrolled apart go, wherever they stand, and though they were worked in last.
        assert_eq!((spare(&views(&[p, t, p]), 0), spare(&views(&[p, p, t]), 2), spare(&views(&[t, p, p]), 0)), (2, 0, 2));
        assert_eq!((spare(&views(&[p, t, p]), 2), spare(&views(&[p, p, t]), 0)), (2, 0));
        // Two halves alike: the one not worked in.
        assert_eq!((spare(&views(&[p, p]), 0), spare(&views(&[p, p]), 1), spare(&views(&[t, t]), 1)), (1, 0, 0));
        // A translation with no pages of its own goes before a pair is parted; a pair alone loses its translation.
        assert_eq!((spare(&views(&[p, t, t]), 0), spare(&views(&[p, t]), 1)), (2, 1));
        // The map goes before a pair is parted, after a second copy of the pages, and never leaves the reader without the paper.
        assert_eq!((spare(&views(&[p, t, m]), 0), spare(&views(&[p, p, m]), 0), spare(&views(&[p, m, p]), 2)), (2, 1, 0));
        assert_eq!((spare(&views(&[p, m]), 0), spare(&views(&[m, p]), 1), spare(&views(&[m, t]), 1)), (1, 0, 0));
    }

    #[test]
    fn page_only_citation_marks_its_page_before_or_after_pdf_opens() {
        let citation = wisp_core::ai::citations("结果见 [P429 p.8]。").remove(0);
        let mut focus = CitationFocus::default();
        focus.begin(citation.page.unwrap(), &citation.quote);
        assert_eq!(focus.status, CitationStatus::PageOnly);
        assert_eq!(focus.index(0), None);
        assert_eq!(focus.index(12), Some(7));
        assert!(focus.boxes.is_empty()); // A page reference must not invent a passage.
        focus.begin(4, " \n ");
        assert_eq!((&focus.status, focus.index(12)), (&CitationStatus::PageOnly, Some(3)));
        focus.begin(99, "");
        assert_eq!(focus.index(12), Some(11)); // Match navigation's page clamping.
        focus.begin(0, "");
        assert_eq!(focus.index(12), Some(0));
    }

    #[test]
    fn newer_citation_or_navigation_ignores_old_search_results() {
        let mut focus = CitationFocus::default();
        let old = focus.begin(4, "A quoted sentence.");
        let boxes = vec![[10., 20., 90., 30.]];
        focus.begin(8, "");
        assert!(!focus.finish(old, Some((3, boxes.clone(), true))));
        assert!(!focus.finish(old, None));
        assert_eq!((&focus.status, focus.index(12)), (&CitationStatus::PageOnly, Some(7)));

        let old = focus.begin(4, "An earlier quotation.");
        let latest = focus.begin(6, "Another quotation.");
        assert!(!focus.finish(old, None));
        assert!(focus.finish(latest, Some((6, boxes.clone(), true))));
        assert_eq!((&focus.status, focus.index(12), &focus.boxes), (&CitationStatus::Found, Some(6), &boxes));
        assert!(!focus.finish(old, Some((3, boxes, true))));

        let pending = focus.begin(4, "Still searching.");
        focus.clear();
        assert!(!focus.finish(pending, None));
        assert_eq!((&focus.status, focus.index(12)), (&CitationStatus::None, None));
    }

    #[test]
    fn missing_quote_keeps_page_marker_and_next_click_clears_notice() {
        let mut focus = CitationFocus::default();
        let request = focus.begin(4, "Not in the PDF.");
        assert!(focus.finish(request, None));
        assert_eq!((&focus.status, focus.index(12)), (&CitationStatus::Missing, Some(3)));
        focus.begin(8, "");
        assert_eq!((&focus.status, focus.index(12)), (&CitationStatus::PageOnly, Some(7)));
        let request = focus.begin(4, "Text without drawable boxes.");
        assert!(focus.finish(request, Some((3, vec![], true))));
        assert_eq!(focus.status, CitationStatus::Missing);
        // Only the beginning found: marked, and said to be a part.
        let request = focus.begin(4, "A quotation that drifts from the page.");
        assert!(focus.finish(request, Some((3, vec![[10., 20., 90., 30.]], false))));
        assert_eq!((&focus.status, focus.boxes.len()), (&CitationStatus::Partial, 1));
    }

    #[test]
    fn translation_card_stays_in_view() {
        let view = (800., 600.);
        // Under the selection, where it starts, as tall as it may be.
        let under = card_place([100., 100., 400., 140.], 360., 280., view);
        assert_eq!(under, CardPlace { left: 100., width: 360., top: Some(146.), bottom: None, max_height: 280. });
        // Near the right edge it moves in; near the bottom it stands over the selection, its bottom over the first line.
        let over = card_place([700., 500., 780., 540.], 360., 280., view);
        assert_eq!((over.left, over.top, over.bottom, over.max_height), (800. - 360. - CARD_EDGE, None, Some(600. - 494.), 280.));
        // With little room either way it takes the side with more, held to that room.
        let short = card_place([10., 150., 300., 200.], 360., 280., (800., 300.));
        assert_eq!((short.top, short.max_height), (None, 144. - CARD_EDGE));
        // A viewer narrower than the card: the card is narrowed to it.
        let narrow = card_place([0., 10., 50., 20.], 360., 280., (300., 600.));
        assert_eq!((narrow.left, narrow.width), (CARD_EDGE, 300. - 2. * CARD_EDGE));
        // A selection scrolled out of view above or below: the card stays inside, at the edge nearest it.
        let gone_up = card_place([100., -400., 300., -300.], 360., 280., view);
        assert_eq!((gone_up.top, gone_up.max_height), (Some(CARD_EDGE), 280.));
        let gone_down = card_place([100., 900., 300., 950.], 360., 280., view);
        assert_eq!((gone_down.bottom, gone_down.max_height), (Some(CARD_EDGE), 280.));
        // A selection at the very bottom of a short viewer still leaves the card some room, over the text.
        let low = card_place([100., 0., 300., 290.], 360., 280., (800., 300.));
        let bottom = low.bottom.map(|b| 300. - b).or(low.top).unwrap();
        assert!(low.max_height >= 80. && bottom - CARD_EDGE >= 0. && bottom <= 300. - CARD_EDGE);
    }

    #[test]
    fn the_sideways_bar_shows_how_much_is_out_of_view() {
        // A view 800 wide over pages twice as wide: the thumb is half the track, and runs its whole length.
        let track = 800. - 2. * BAR_INSET;
        assert_eq!(bar_thumb(800., 800., 0.), (track / 2., 0.));
        assert_eq!(bar_thumb(800., 800., 400.), (track / 2., track / 4.));
        assert_eq!(bar_thumb(800., 800., 800.), (track / 2., track / 2.));
        // Zoomed far in, the thumb stays long enough to take hold of, and inside its track.
        let (thumb, left) = bar_thumb(300., 30000., 30000.);
        assert_eq!((thumb, left), (BAR_THUMB_MIN, 300. - 2. * BAR_INSET - BAR_THUMB_MIN));
    }

    #[test]
    fn sizes_of_a_page() {
        let (a4, view) = ((595., 842.), (800., 600.));
        // As wide as the viewer less its margins; whole in it; its own size.
        assert_eq!(page_scale(Zoom::FitWidth, a4, view), (800. - 2. * MARGIN) / 595.);
        assert!((page_scale(Zoom::FitPage, a4, view) * 842. - (600. - 24.)).abs() < 0.01);
        assert_eq!(page_scale(Zoom::FitPage, a4, (800., 0.)), page_scale(Zoom::FitWidth, a4, view));
        assert_eq!((page_scale(Zoom::Percent(100.), a4, view), page_scale(Zoom::Percent(9000.), a4, view)), (POINT_PX, 5. * POINT_PX));
        // The buttons step through round sizes, from wherever the wheel left the zoom.
        assert_eq!((zoom_step(100., true), zoom_step(100., false), zoom_step(137.2, true), zoom_step(137.2, false)), (110., 90., 150., 125.));
        assert_eq!((zoom_step(500., true), zoom_step(25., false)), (500., 25.));
    }

    #[test]
    fn pictures_within_bounds() {
        let a4 = (595., 842.);
        // Reading width on a dense display, and a little more of the same: one picture serves both.
        assert_eq!((render_scale(a4, 2.6), render_scale(a4, 2.9), render_scale(a4, 1.3)), (3., 3., 2.));
        // Enlarged five times on a dense display a page would be 400 MB: it is held to what a picture may take.
        let capped = render_scale(a4, 13.3);
        assert!(capped < 7. && capped * capped * 595. * 842. <= 24_000_001.);
        // A poster stays under the longest side a pixmap can have.
        assert!(render_scale((10_000., 4_000.), 2.) * 10_000. <= 8000.5);
    }

    #[test]
    fn a_fit_follows_the_viewer_and_the_reader() {
        let a4 = (595., 842.);
        // At a fit the page grows and shrinks with the reading area; a percentage does not.
        assert!(page_scale(Zoom::FitWidth, a4, (1400., 900.)) > page_scale(Zoom::FitWidth, a4, (700., 900.)));
        assert_eq!(page_scale(Zoom::Percent(150.), a4, (1400., 900.)), page_scale(Zoom::Percent(150.), a4, (700., 900.)));
        // The next paper opens at the fit the reader chose; a percentage was for one figure.
        assert_eq!(Zoom::FitWidth.carried(Zoom::FitWidth), Zoom::FitWidth);
        assert_eq!(Zoom::FitPage.carried(Zoom::FitWidth), Zoom::FitPage);
        assert_eq!(Zoom::Percent(150.).carried(Zoom::FitWidth), Zoom::FitWidth);
        assert_eq!(Zoom::Percent(150.).carried(Zoom::FitPage), Zoom::FitPage);
        assert!(Zoom::FitPage.fits() && !Zoom::Percent(100.).fits());
    }

    #[test]
    fn a_narrower_viewer_keeps_its_pictures() {
        let a4 = (595., 842.);
        // A page shown at 3.2 device pixels a point wants a picture at 4; at 2.9 it would want 3, and the 4 serves.
        let (wide, narrow) = (render_scale(a4, 3.2), render_scale(a4, 2.9));
        assert_eq!((wide, narrow), (4., 3.));
        assert!(picture_serves(wide, narrow, 2.9));
        // Wider again, a softer picture does not.
        assert!(!picture_serves(narrow, wide, 3.2));
        // Zoomed far out, a large picture is let go of.
        assert!(!picture_serves(12., render_scale(a4, 2.), 2.));
        assert!(picture_serves(3., 3., 2.6));
    }
}
