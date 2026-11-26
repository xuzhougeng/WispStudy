//! Pixel-art researcher state machine and miniature progress-desk diorama.
//!
//! Driven by real task state (`Activity`, `done / total`) and rendered directly
//! via GPUI `canvas` + `paint_quad` using `Theme::*` semantic tokens so light
//! and dark themes, high contrast, and all seven library accents match.
//!
//! Two drawings, by how much room there is: the desk (`desk`, `desk_large`), a scene with the researcher at work
//! and the run's progress as two piles of paper, and the face (`inline`, `inline_large`), which is what a line of
//! text has room for. Either is drawn in cells of whole pixels of the screen (`whole_cell`).

use crate::theme::{self, Theme};
use crate::ui;
use gpui::*;
use std::time::Duration;

/// How wide a frame of the desk's researcher is; its frames are checked against it.
#[cfg(test)]
const SPRITE_W: usize = 16;
const DESK_SPRITE_H: usize = 15;
const SCENE_W: f32 = 38.;
const SCENE_H: f32 = 26.;
const MAX_STACK_SHEETS: usize = 6;

/// What the pixel researcher is doing right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Activity {
    Reading,
    Searching,
    Thinking,
    Writing,
    Drawing,
    Compacting,
    Importing,
    #[allow(dead_code)]
    Listening,
    WaitingUser,
    /// Nothing to do: the face on the box of a conversation between two questions.
    Idle,
    Done,
    Paused,
}

impl Activity {
    /// Map a formatted tool call line (e.g. `"检索全文：query"`) to an activity.
    pub fn from_tool_call(call: &str) -> Self {
        let title = call.split('：').next().unwrap_or(call).trim();
        if ["搜", "检", "查", "找", "期刊"].iter().any(|k| title.contains(k)) {
            Self::Searching
        } else if ["导入", "导出", "保存笔记", "新建文献库", "整理"].iter().any(|k| title.contains(k)) {
            Self::Importing
        } else if ["写", "改", "批注", "大纲", "幻灯片", "引用", "翻译", "预览笔记"].iter().any(|k| title.contains(k)) {
            Self::Writing
        } else if title.contains('画') && !title.contains("导图") {
            Self::Drawing
        } else if ["读", "看", "附表", "导图", "筛选", "词条", "文献库"].iter().any(|k| title.contains(k)) {
            Self::Reading
        } else {
            Self::Thinking
        }
    }

    /// Map a background batch run's state to an activity.
    pub fn from_run(running: bool, done: usize, total: usize, active: Self) -> Self {
        if running {
            active
        } else if total > 0 && done >= total {
            Self::Done
        } else {
            Self::Paused
        }
    }

    fn is_looping(self) -> bool {
        !matches!(self, Self::Done | Self::Paused)
    }
}

/// Compute `(left_todo_sheets, right_done_sheets)` bound to `done / total`.
pub fn paper_stacks(done: usize, total: usize, max_sheets: usize) -> (usize, usize) {
    if total == 0 || max_sheets == 0 {
        return (0, 0);
    }
    let clamped = done.min(total);
    let left = if clamped >= total {
        0
    } else {
        (((total - clamped) as f32 / total as f32) * max_sheets as f32).ceil().clamp(1.0, max_sheets as f32) as usize
    };
    let right = if clamped == 0 {
        0
    } else {
        ((clamped as f32 / total as f32) * max_sheets as f32).ceil().clamp(1.0, max_sheets as f32) as usize
    };
    (left, right)
}

/// Miniature researcher-at-desk diorama (`76 × 52` logical px).
pub fn desk(id: impl std::fmt::Display, activity: Activity, progress: Option<(usize, usize)>) -> AnyElement {
    desk_sized(id, activity, progress, 2.0)
}

/// Larger researcher-at-desk diorama (`114 × 78` logical px), e.g. for figure drawing.
pub fn desk_large(id: impl std::fmt::Display, activity: Activity, progress: Option<(usize, usize)>) -> AnyElement {
    desk_sized(id, activity, progress, 3.0)
}

/// The researcher's face beside a line of text (`24 × 18` logical px, cells of two): a card's waiting line, a heading.
pub fn inline(id: impl std::fmt::Display, activity: Activity) -> AnyElement {
    inline_sized(id, activity, 2.)
}

/// The face on a line it has to itself (`36 × 27` logical px, cells of three): the one line of a conversation that
/// says the assistant is at work.
pub fn inline_large(id: impl std::fmt::Display, activity: Activity) -> AnyElement {
    inline_sized(id, activity, 3.)
}

/// The face in cells of `cell` logical pixels. There is no size in between: a cell is a whole number of pixels,
/// or its edges are soft and the face cannot be made out (what sixteen cells in eighteen pixels looked like).
fn inline_sized(id: impl std::fmt::Display, activity: Activity, cell: f32) -> AnyElement {
    if !theme::work_animation() {
        return quiet_inline(id, activity, if cell > 2. { 19. } else { 18. });
    }
    let face = move |beat: usize| canvas(|_, _, _| (), move |bounds, _, window, _| paint_inline(bounds, cell, activity, beat, window)).size_full();
    let frame = div().flex_none().w(px(INLINE_W as f32 * cell)).h(px(INLINE_H as f32 * cell));
    if !activity.is_looping() {
        return frame.child(face(0)).into_any_element();
    }
    // Keyed by what it does as well: another activity begins its loop anew.
    let beats = inline_beats(activity);
    frame
        .with_animation(SharedString::from(format!("{id}-{activity:?}")), Animation::new(Duration::from_millis(beats as u64 * INLINE_BEAT_MS)).repeat(), move |frame, t| {
            frame.child(face(((t * beats as f32) as usize).min(beats - 1)))
        })
        .into_any_element()
}

fn quiet_inline(id: impl std::fmt::Display, activity: Activity, size_px: f32) -> AnyElement {
    let icon_size = (size_px - 5.).max(11.);
    match activity {
        Activity::WaitingUser => ui::icon("info", icon_size.max(13.), Theme::accent()).into_any_element(),
        Activity::Idle => div().into_any_element(),
        Activity::Done => ui::icon("check", icon_size.max(13.), Theme::success()).into_any_element(),
        Activity::Paused => ui::icon("pause", icon_size.max(12.), Theme::text_tertiary()).into_any_element(),
        _ => ui::spinner(SharedString::from(format!("{id}-quiet")), icon_size, Theme::accent()).into_any_element(),
    }
}

fn desk_sized(id: impl std::fmt::Display, activity: Activity, progress: Option<(usize, usize)>, scale: f32) -> AnyElement {
    if !theme::work_animation() {
        return quiet_desk(id, activity, scale);
    }
    let w = SCENE_W * scale;
    let h = SCENE_H * scale;
    let card = div().relative().flex_none().w(px(w)).h(px(h)).rounded(px(8.)).border_1().border_color(Theme::line()).bg(Theme::bg_body()).overflow_hidden();
    let anim_id = SharedString::from(format!("{id}-{activity:?}"));
    match activity {
        Activity::Paused => card
            .child(canvas(|_, _, _| (), move |bounds, _, window, _| paint_desk_scene(bounds, scale, activity, progress, 0, window)).size_full())
            .into_any_element(),
        Activity::Done => card
            .with_animation(anim_id, Animation::new(Duration::from_millis(2400)), move |shell, t| {
                let beat = ((t * 12.0).floor() as usize).min(11);
                shell.child(canvas(|_, _, _| (), move |bounds, _, window, _| paint_desk_scene(bounds, scale, Activity::Done, progress, beat, window)).size_full())
            })
            .into_any_element(),
        _ => card
            .with_animation(anim_id, Animation::new(Duration::from_millis(6400)).repeat(), move |shell, t| {
                let beat = ((t * 32.0).floor() as usize).min(31);
                shell.child(canvas(|_, _, _| (), move |bounds, _, window, _| paint_desk_scene(bounds, scale, activity, progress, beat, window)).size_full())
            })
            .into_any_element(),
    }
}

fn quiet_desk(id: impl std::fmt::Display, activity: Activity, scale: f32) -> AnyElement {
    let w = (21. * scale).round();
    let h = (27. * scale).round();
    let lines = [1., 1., 1., 0.6].map(|width| div().h(px(2.)).w(relative(width)).rounded_full().bg(Theme::line_strong()));
    let sheet = div()
        .relative()
        .flex()
        .flex_col()
        .flex_none()
        .justify_center()
        .gap(px(6.))
        .w(px(w))
        .h(px(h))
        .px(px(8.))
        .rounded(px(4.))
        .border_1()
        .border_color(Theme::line_strong())
        .bg(Theme::bg_body())
        .children(lines);
    if !activity.is_looping() {
        return sheet.into_any_element();
    }
    let scan = div().absolute().left(px(-4.)).right(px(-4.)).h(px(2.)).rounded_full().bg(Theme::accent());
    sheet
        .child(scan.with_animation(SharedString::from(format!("{id}-quiet-scan")), Animation::new(Duration::from_millis(2400)).repeat().with_easing(pulsating_between(0., 1.)), move |line, at| {
            line.top(px(5. + (h - 12.) * at))
        }))
        .into_any_element()
}

// ---- semantic colour palette & crisp pixel painting

fn palette_color(glyph: u8) -> Option<Hsla> {
    match glyph {
        b'.' | b' ' => None,
        b'#' => Some(Theme::text()),
        b'@' => Some(Theme::text_secondary()),
        b'+' => Some(Theme::text_tertiary()),
        b'-' => Some(Theme::line_strong()),
        b'w' => Some(Theme::bg_body()),
        b'p' => Some(Theme::bg_sunken()),
        b'*' => Some(Theme::accent()),
        b's' => Some(Theme::accent_soft()),
        b'g' => Some(Theme::success()),
        b'y' => Some(Theme::warning()),
        _ => None,
    }
}

/// A cell of `nominal` logical pixels as this screen can draw it: a whole number of the screen's own pixels. A cell
/// that ends between two of them is drawn with soft edges, and pixel art of soft edges is a blur: at 125 % two
/// logical pixels are two and a half of the screen's. `grow` takes the nearer whole number, for a sprite that may
/// reach a little past its box; without it the cell only gets smaller, for one that has to stay inside.
fn whole_cell(nominal: f32, scale_factor: f32, grow: bool) -> f32 {
    let device = nominal * scale_factor;
    (if grow { device.round() } else { device.floor() }).max(1.) / scale_factor
}

/// A coordinate moved to the nearest pixel of the screen.
fn on_pixel(at: f32, scale_factor: f32) -> f32 {
    (at * scale_factor).round() / scale_factor
}

/// A rectangle of the grid, each edge on a pixel of the screen: neighbours share their edges, so no line shows between them.
fn paint_rect(origin: Point<Pixels>, scale: f32, gx: f32, gy: f32, gw: f32, gh: f32, color: Hsla, window: &mut Window) {
    let scale_factor = window.scale_factor();
    let edge = |from: Pixels, cells: f32| px(on_pixel(f32::from(from) + cells * scale, scale_factor));
    let rect = Bounds::from_corners(point(edge(origin.x, gx), edge(origin.y, gy)), point(edge(origin.x, gx + gw), edge(origin.y, gy + gh)));
    window.paint_quad(fill(rect, color));
}

/// Rows of glyphs, a rectangle for each run of one colour.
fn paint_grid<Row: AsRef<[u8]>>(origin: Point<Pixels>, scale: f32, rows: &[Row], color_of: fn(u8) -> Option<Hsla>, window: &mut Window) {
    for (y, row) in rows.iter().enumerate() {
        let bytes = row.as_ref();
        let mut x = 0;
        while x < bytes.len() {
            let ch = bytes[x];
            let start = x;
            while x < bytes.len() && bytes[x] == ch {
                x += 1;
            }
            if let Some(color) = color_of(ch) {
                paint_rect(origin, scale, start as f32, y as f32, (x - start) as f32, 1., color, window);
            }
        }
    }
}

// ---- desk diorama scene (38 × 26 grid)

fn researcher_holds_mug(activity: Activity, beat: usize) -> bool {
    match activity {
        Activity::Reading | Activity::Writing | Activity::Thinking => (24..=29).contains(&beat),
        Activity::Searching => (27..=29).contains(&beat),
        Activity::Drawing => (26..=29).contains(&beat),
        Activity::Done => beat >= 4,
        _ => false,
    }
}

fn paint_desk_scene(bounds: Bounds<Pixels>, scale: f32, activity: Activity, progress: Option<(usize, usize)>, beat: usize, window: &mut Window) {
    // Cells of whole pixels that fit the card, and the scene in the middle of what a smaller cell leaves free.
    let scale = whole_cell(scale, window.scale_factor(), false);
    let origin = point(bounds.origin.x + (bounds.size.width - px(SCENE_W * scale)) / 2., bounds.origin.y + (bounds.size.height - px(SCENE_H * scale)) / 2.);
    let sprite_origin = point(px(f32::from(origin.x) + 10. * scale), px(f32::from(origin.y) + 4. * scale));

    // 1. Desk surface, underside shadow, and legs.
    paint_rect(origin, scale, 2., 19., 34., 1., Theme::line_strong(), window);
    paint_rect(origin, scale, 3., 20., 32., 1., Theme::bg_sunken(), window);
    paint_rect(origin, scale, 4., 21., 2., 4., Theme::line_strong(), window);
    paint_rect(origin, scale, 32., 21., 2., 4., Theme::line_strong(), window);

    // 2. Props & real-progress paper stacks on the desk.
    let holding_mug = researcher_holds_mug(activity, beat);
    let mut mug_x = Some(25.);
    match progress {
        Some((done, total)) => {
            let (left, right) = paper_stacks(done, total, MAX_STACK_SHEETS);
            paint_paper_stack(origin, scale, 2., 18., left, false, false, window);
            paint_paper_stack(origin, scale, 29., 18., right, true, activity == Activity::Done, window);
            // Flying paper arc from left stack to right stack on page-flip beats.
            if activity == Activity::Reading && left > 0 && beat < 24 && beat % 8 == 3 {
                paint_rect(origin, scale, 18., 8., 3., 2., Theme::accent(), window);
                paint_rect(origin, scale, 19., 9., 1., 1., Theme::bg_body(), window);
            }
            if activity == Activity::Reading && left > 0 && beat < 24 && beat % 8 == 4 {
                paint_rect(origin, scale, 24., 10., 3., 2., Theme::accent(), window);
                paint_rect(origin, scale, 25., 11., 1., 1., Theme::bg_body(), window);
            }
        }
        None => match activity {
            Activity::Drawing => {
                mug_x = Some(4.);
                paint_easel(origin, scale, beat, window);
            }
            Activity::Importing => {
                let carried = (beat / 4) % 4 + 1;
                paint_paper_stack(origin, scale, 2., 18., 4 - (carried % 3), false, false, window);
                paint_bookshelf(origin, scale, carried, window);
                mug_x = Some(24.);
            }
            Activity::Compacting => {
                paint_paper_stack(origin, scale, 2., 18., 5 - (beat % 4), false, false, window);
                paint_scroll_bundle(origin, scale, beat, window);
            }
            _ => {
                paint_paper_stack(origin, scale, 2., 18., 3, false, false, window);
                paint_pen_cup(origin, scale, window);
            }
        },
    }

    // Always keep the researcher's steaming coffee mug on the desk unless they are holding it!
    if !holding_mug && let Some(mx) = mug_x {
        paint_desk_mug(origin, scale, mx, beat, window);
    }

    // 3. Pixel researcher sprite (16 × 15 at gx=10, gy=4).
    paint_grid(sprite_origin, scale, desk_sprite(activity, beat), palette_color, window);

    // 4. Activity status badge on paused/done states.
    if activity == Activity::Paused {
        paint_grid(point(px(f32::from(origin.x) + 27. * scale), px(f32::from(origin.y) + 4. * scale)), scale, PAUSE_MARK, palette_color, window);
    }
}

fn paint_paper_stack(origin: Point<Pixels>, scale: f32, x: f32, base_y: f32, sheets: usize, done_side: bool, finished: bool, window: &mut Window) {
    for idx in 0..sheets {
        let y = base_y - (idx as f32 * 2.);
        let jitter = if idx % 2 == 1 { 0.5 } else { 0. };
        paint_rect(origin, scale, x + jitter, y - 1., 6., 1., Theme::bg_body(), window);
        paint_rect(origin, scale, x + jitter, y, 6., 1., Theme::line_strong(), window);
        if done_side && idx + 1 == sheets {
            let mark = if finished { Theme::success() } else { Theme::accent() };
            paint_rect(origin, scale, x + jitter + 4., y - 1., 2., 1., mark, window);
        }
    }
}

fn paint_desk_mug(origin: Point<Pixels>, scale: f32, x: f32, beat: usize, window: &mut Window) {
    // Mug body + rim + handle
    paint_rect(origin, scale, x, 16., 3., 1., Theme::bg_body(), window);
    paint_rect(origin, scale, x, 17., 3., 2., Theme::text_secondary(), window);
    paint_rect(origin, scale, x + 3., 17., 1., 2., Theme::text_tertiary(), window);
    // Gentle curling steam
    let phase = beat % 4;
    if phase == 0 || phase == 1 {
        paint_rect(origin, scale, x + 1., 14., 1., 1., Theme::text_tertiary(), window);
        paint_rect(origin, scale, x + 2., 13., 1., 1., Theme::line_strong(), window);
    } else {
        paint_rect(origin, scale, x + 2., 14., 1., 1., Theme::text_tertiary(), window);
        paint_rect(origin, scale, x + 1., 13., 1., 1., Theme::line_strong(), window);
    }
}

fn paint_pen_cup(origin: Point<Pixels>, scale: f32, window: &mut Window) {
    paint_rect(origin, scale, 31., 16., 3., 3., Theme::line_strong(), window);
    paint_rect(origin, scale, 32., 14., 1., 2., Theme::accent(), window);
    paint_rect(origin, scale, 33., 15., 1., 1., Theme::text_tertiary(), window);
}

fn paint_easel(origin: Point<Pixels>, scale: f32, beat: usize, window: &mut Window) {
    // Easel legs + shelf
    paint_rect(origin, scale, 30., 6., 1., 1., Theme::text_tertiary(), window);
    paint_rect(origin, scale, 28., 15., 1., 4., Theme::text_tertiary(), window);
    paint_rect(origin, scale, 33., 15., 1., 4., Theme::text_tertiary(), window);
    paint_rect(origin, scale, 27., 15., 8., 1., Theme::line_strong(), window);
    // Canvas frame + white surface
    paint_rect(origin, scale, 27., 7., 8., 8., Theme::line_strong(), window);
    paint_rect(origin, scale, 28., 8., 6., 6., Theme::bg_body(), window);
    // Progressive pixel strokes on canvas
    let stage = (beat / 4) % 6;
    paint_rect(origin, scale, 29., 11., 4., 2., Theme::accent_soft(), window);
    if stage >= 1 {
        paint_rect(origin, scale, 29., 9., 2., 2., Theme::accent(), window);
    }
    if stage >= 2 {
        paint_rect(origin, scale, 31., 10., 2., 1., Theme::text_secondary(), window);
    }
    if stage >= 3 {
        paint_rect(origin, scale, 30., 12., 3., 1., Theme::accent(), window);
    }
    if stage >= 4 {
        paint_rect(origin, scale, 32., 9., 1., 1., Theme::warning(), window);
    }
}

fn paint_bookshelf(origin: Point<Pixels>, scale: f32, count: usize, window: &mut Window) {
    paint_rect(origin, scale, 29., 10., 7., 9., Theme::line_strong(), window);
    paint_rect(origin, scale, 30., 11., 5., 7., Theme::bg_sunken(), window);
    let colors = [Theme::accent(), Theme::text_secondary(), Theme::success(), Theme::text_tertiary()];
    for (i, &color) in colors.iter().take(count).enumerate() {
        paint_rect(origin, scale, 30. + i as f32 * 1.2, 13., 1., 5., color, window);
    }
}

fn paint_scroll_bundle(origin: Point<Pixels>, scale: f32, beat: usize, window: &mut Window) {
    let squish = beat % 4;
    let h = (5 - squish).max(2) as f32;
    paint_rect(origin, scale, 29., 19. - h, 6., h, Theme::bg_body(), window);
    paint_rect(origin, scale, 29., 19. - h, 6., 1., Theme::line_strong(), window);
    paint_rect(origin, scale, 31., 19. - h, 2., h, Theme::accent(), window);
}

const PAUSE_MARK: &[&str] = &[
    ".yyy.",
    "y...y",
    "...y.",
    "..y..",
    ".....",
    "..y..",
];

// ---- desk researcher 16 × 15 frames

fn desk_sprite(activity: Activity, beat: usize) -> &'static [&'static str; DESK_SPRITE_H] {
    match activity {
        Activity::Reading => match beat {
            24 | 25 => DESK_COFFEE_LIFT,
            26 => DESK_COFFEE_SIP_A,
            27 => DESK_COFFEE_SIP_B,
            28 | 29 => DESK_COFFEE_SIP_A,
            30 | 31 => DESK_GLASSES_GLINT,
            b => match b % 4 {
                0 => DESK_READ_LEFT,
                1 => DESK_READ_MID,
                2 => DESK_READ_RIGHT,
                _ => DESK_READ_FLIP,
            },
        },
        Activity::Searching => match beat {
            24 | 25 => DESK_GLASSES_GLINT,
            26 => DESK_COFFEE_LIFT,
            27 => DESK_COFFEE_SIP_A,
            28 => DESK_COFFEE_SIP_B,
            29 => DESK_COFFEE_LIFT,
            30 | 31 => DESK_SEARCH_MID,
            b => match b % 4 {
                0 => DESK_SEARCH_LEFT,
                1 => DESK_SEARCH_MID,
                2 => DESK_SEARCH_RIGHT,
                _ => DESK_SEARCH_MID,
            },
        },
        Activity::Writing => match beat {
            24 | 25 => DESK_COFFEE_LIFT,
            26 => DESK_COFFEE_SIP_A,
            27 => DESK_COFFEE_SIP_B,
            28 | 29 => DESK_COFFEE_SIP_A,
            30 | 31 => DESK_GLASSES_GLINT,
            b => match b % 4 {
                0 => DESK_WRITE_A,
                1 => DESK_WRITE_B,
                2 => DESK_WRITE_C,
                _ => DESK_WRITE_B,
            },
        },
        Activity::Drawing => match beat {
            24 | 25 => DESK_DRAW_ADMIRE,
            26 => DESK_COFFEE_LIFT,
            27 => DESK_COFFEE_SIP_A,
            28 => DESK_COFFEE_SIP_B,
            29 => DESK_COFFEE_LIFT,
            30 | 31 => DESK_DRAW_ADMIRE,
            b => match b % 4 {
                0 => DESK_DRAW_A,
                1 => DESK_DRAW_B,
                2 => DESK_DRAW_C,
                _ => DESK_DRAW_B,
            },
        },
        Activity::Thinking => match beat {
            24 | 25 => DESK_COFFEE_LIFT,
            26 => DESK_COFFEE_SIP_A,
            27 => DESK_COFFEE_SIP_B,
            28 | 29 => DESK_COFFEE_SIP_A,
            30 | 31 => DESK_GLASSES_GLINT,
            b => match b % 4 {
                0 => DESK_THINK_A,
                1 => DESK_THINK_B,
                2 => DESK_THINK_C,
                _ => DESK_THINK_B,
            },
        },
        Activity::Importing => match beat {
            24 | 25 => DESK_WIPE_BROW,
            26..=29 => DESK_STRETCH,
            30 | 31 => DESK_GLASSES_GLINT,
            b => match b % 4 {
                0 => DESK_IMPORT_A,
                1 => DESK_IMPORT_B,
                2 => DESK_IMPORT_C,
                _ => DESK_IMPORT_B,
            },
        },
        Activity::Compacting => match beat {
            24 | 25 => DESK_WIPE_BROW,
            26..=29 => DESK_STRETCH,
            30 | 31 => DESK_GLASSES_GLINT,
            b => match b % 4 {
                0 => DESK_COMPACT_A,
                1 => DESK_COMPACT_B,
                2 => DESK_COMPACT_C,
                _ => DESK_COMPACT_B,
            },
        },
        Activity::Listening => match beat % 4 {
            0 => DESK_LISTEN_A,
            1 => DESK_LISTEN_B,
            2 => DESK_LISTEN_C,
            _ => DESK_LISTEN_B,
        },
        Activity::WaitingUser | Activity::Idle => match beat % 4 {
            0 | 2 => DESK_WAIT_A,
            _ => DESK_WAIT_B,
        },
        Activity::Done => match beat {
            0..=3 => DESK_DONE_CELEBRATE,
            4 | 5 => DESK_COFFEE_SIP_A,
            6..=8 => DESK_COFFEE_SIP_B,
            _ => DESK_DONE_REST,
        },
        Activity::Paused => DESK_PAUSED,
    }
}

const DESK_READ_LEFT: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...##ww###ww#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "...+wwww*wwww+..",
    "..+wwwwww*wwww+.",
    "..------------..",
];

const DESK_READ_MID: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#w#w##w#w#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "...+wwww*wwww+..",
    "..+wwwwww*wwww+.",
    "..------------..",
];

const DESK_READ_RIGHT: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#ww###ww##...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "...+wwww*wwww+..",
    "..+wwwwww*wwww+.",
    "..------------..",
];

const DESK_READ_FLIP: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#w#w##w#w#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@+*+..",
    "....@@@@@@@*w*..",
    "...+wwww*www*+..",
    "..+wwwwww*wwww+.",
    "..------------..",
];

const DESK_SEARCH_LEFT: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...##ww###ww#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "..***@@@@@@@....",
    ".*ss*+@@@@@@+...",
    "..***@@@@@@@+...",
    "...+wwwwwwww+...",
    "..+wwwwwwwwww+..",
    "..------------..",
];

const DESK_SEARCH_MID: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#w#w##w#w#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    ".....***@@@@....",
    "...+*ss*@@@@+...",
    "...++***@@@@+...",
    "...+wwwwwwww+...",
    "..+wwwwwwwwww+..",
    "..------------..",
];

const DESK_SEARCH_RIGHT: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#ww###ww##...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    ".......@@@@***..",
    "...+@@@@@@+*ss*.",
    "...+@@@@@@@***..",
    "...+wwwwwwww+...",
    "..+wwwwwwwwww+..",
    "..------------..",
];

const DESK_WRITE_A: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...##ww###ww#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@*@@....",
    "...+wwww*wwww+..",
    "..+www**wwwww+..",
    "..------------..",
];

const DESK_WRITE_B: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#w#w##w#w#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@*@@...",
    "...+wwwwww*ww+..",
    "..+www***wwww+..",
    "..------------..",
];

const DESK_WRITE_C: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#ww###ww##...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@@*@...",
    "...+wwwwwww*w+..",
    "..+www****www+..",
    "..------------..",
];

const DESK_DRAW_A: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#ww###ww##...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@+**..",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_DRAW_B: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#ww###ww##...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@.+..",
    "...@@@@@@@@+*...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_DRAW_C: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#ww###ww##...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@++...",
    "...++@@@@@@+**..",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_DRAW_ADMIRE: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#ww###ww##...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "...@@@@@@@@.....",
    "..@@@@@@@@@.....",
    "...++@@@@@@++...",
    "...+@++++@@+*...",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_THINK_A: &[&str; DESK_SPRITE_H] = &[
    ".........*......",
    ".....######.....",
    "....########....",
    "....#w#ww#w#....",
    "...#www##www#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######+....",
    "....@@@@@@+@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_THINK_B: &[&str; DESK_SPRITE_H] = &[
    ".......*...*....",
    ".....######.....",
    "....########....",
    "....#w#ww#w#....",
    "...#www##www#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######+....",
    "....@@@@@@+@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_THINK_C: &[&str; DESK_SPRITE_H] = &[
    "......*..*..*...",
    ".....######.....",
    "....########....",
    "....#w#ww#w#....",
    "...#www##www#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######+....",
    "....@@@@@@+@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_IMPORT_A: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#w#w##w#w#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@wwwwww@@...",
    "...+@------@+...",
    "...+@wwwwww@+...",
    "....@------@....",
    "....@@@@@@@@....",
    "...----------...",
];

const DESK_IMPORT_B: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#w#w##w#w#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...+@wwwwww@+...",
    "...+@------@+...",
    "....@wwwwww@....",
    "....@------@....",
    "...----------...",
];

const DESK_IMPORT_C: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#ww###ww##...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@+*w*.",
    "....@@@@@@@@*--.",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_COMPACT_A: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#w#w##w#w#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...+@@@@@@@@+...",
    "....+wwwwww+....",
    "....+------+....",
    "....+wwwwww+....",
    "....+------+....",
    "...----------...",
];

const DESK_COMPACT_B: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#w#w##w#w#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...+@@@@@@@@+...",
    "....+ww**ww+....",
    "....+--**--+....",
    "....+ww**ww+....",
    "...----------...",
];

const DESK_COMPACT_C: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#w#w##w#w#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "....+ww**ww+....",
    "....+--**--+....",
    "...----------...",
];

const DESK_LISTEN_A: &[&str; DESK_SPRITE_H] = &[
    ".........*......",
    ".....######.....",
    "....*######*....",
    "...*#wwwwww#*...",
    "...*#w#ww#w#*...",
    "...*###ww###*...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_LISTEN_B: &[&str; DESK_SPRITE_H] = &[
    "........*..**...",
    ".....######.....",
    "....*######*....",
    "...*#wwwwww#*...",
    "...*#w#ww#w#*...",
    "...*###ww###*...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_LISTEN_C: &[&str; DESK_SPRITE_H] = &[
    ".......*...*....",
    "....*######*....",
    "...*########*...",
    "...*#wwwwww#*...",
    "...*#w#ww#w#*...",
    "....###ww###....",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_WAIT_A: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########..*.",
    "....#wwwwww#.*w*",
    "...#w#w##w#w#***",
    "...####ww####.+.",
    "....#wwwwww#..+.",
    ".....######..++.",
    "....@@@@@@@@@...",
    "...@@@@@@@@@@...",
    "...++@@@@@@.....",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_WAIT_B: &[&str; DESK_SPRITE_H] = &[
    ".............*..",
    ".....######.*w*.",
    "....########***.",
    "....#wwwwww#.+..",
    "...#w#w##w#w#+..",
    "...####ww####+..",
    "....#wwwwww#.+..",
    ".....######.++..",
    "....@@@@@@@@@...",
    "...@@@@@@@@@@...",
    "...++@@@@@@.....",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

// ---- expressive coffee-sipping, glasses-glint, wipe-brow & done frames (16 × 15)

const DESK_COFFEE_LIFT: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#ww###ww##...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@.+..",
    "...@@@@@@@@@@+..",
    "...++@@@@@@www..",
    "....@@@@@@@+@@+.",
    "....@@@@@@@+@@..",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_COFFEE_SIP_A: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#w##ww##w#...",
    "...####ww####...",
    "....#wwwwww#.+..",
    ".....##www##+...",
    "....@@@+@@+@@...",
    "...@@@@@@@+@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_COFFEE_SIP_B: &[&str; DESK_SPRITE_H] = &[
    "........+.......",
    ".....######.....",
    "....########....",
    "....#wwwwww#+...",
    "...#w##ww##w#...",
    "...####ww####...",
    "....#wwwwww#+...",
    ".....##www##....",
    "....@@@+@@+@@...",
    "...@@@@@@@+@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_GLASSES_GLINT: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "..+#*w*##*w*#...",
    "..+####ww####...",
    "...+#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_WIPE_BROW: &[&str; DESK_SPRITE_H] = &[
    "............y...",
    ".....######.y...",
    "....########....",
    "..++#wwwwww#....",
    "..+####ww####...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "....@@@@@@@++...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_STRETCH: &[&str; DESK_SPRITE_H] = &[
    "................",
    "..+..######..+..",
    "..+.########.+..",
    "..+.#wwwwww#.+..",
    "...#w##ww##w#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_DONE_CELEBRATE: &[&str; DESK_SPRITE_H] = &[
    ".............g..",
    "..+..######.g...",
    "..+.########g...",
    "..+.#wwwwww#....",
    "...#w##ww##w#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_DONE_REST: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#w##ww##w#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.+...",
    "....@@@@@@@@+...",
    "...@@@@@@@@www..",
    "...++@@@@@@+@@+.",
    "....@@@@@@@+@@..",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

const DESK_PAUSED: &[&str; DESK_SPRITE_H] = &[
    "................",
    ".....######.....",
    "....########....",
    "....#wwwwww#....",
    "...#w#w##w#w#...",
    "...####ww####...",
    "....#wwwwww#....",
    ".....######.....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...++@@@@@@++...",
    "....@@@@@@@@....",
    "....@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
];

// ---- the researcher's face for a line of text (12 × 9 cells)
//
// A line of text is some twenty pixels tall, which is nine cells of two. The desk's researcher (hair, a face,
// glasses, a body, a desk, in six tones) does not fit into that: sixteen cells in eighteen pixels were a grey
// smudge whose frames differed by two pixels. So a line gets the face alone: one fill with the two lenses cut out
// of it, and what it is doing is said by what moves as a whole (where the eyes look, a blink, a squat, a tapping
// foot, a line growing under it). The frames are made from a pose, not drawn one by one.

const INLINE_W: usize = 12;
const INLINE_H: usize = 9;
/// Beats in one loop of the face at work.
const INLINE_BEATS: usize = 12;
/// Beats in one loop of the face with nothing to do: eight seconds, most of them still.
const IDLE_BEATS: usize = 40;

fn inline_beats(activity: Activity) -> usize {
    if activity == Activity::Idle { IDLE_BEATS } else { INLINE_BEATS }
}
/// How long a beat lasts.
const INLINE_BEAT_MS: u64 = 200;

const FACE: [&[u8; INLINE_W]; INLINE_H] = [
    b"..********..",
    b".**********.",
    b"************",
    b"**www**www**",
    b"**www**www**",
    b"**www**www**",
    b"************",
    b".**********.",
    b"..**....**..",
];
/// A line being written, on the row the feet stand on.
const WRITTEN: &[u8; INLINE_W] = b".##.###.###.";
/// Where each lens begins; a lens is three cells each way.
const LENSES: [usize; 2] = [2, 7];
const LENS_TOP: usize = 3;
/// The two cells of each foot.
const FEET: [[usize; 2]; 2] = [[2, 3], [8, 9]];

/// The eyes of one beat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Eyes {
    /// Looking at a column of the lens (0 to 2), up or down: a pupil two cells tall.
    At(usize, bool),
    /// Looking out of the screen, at the reader.
    Out,
    Shut,
    /// Shut and pleased: `^ ^`.
    Glad,
}

/// What the face does in one beat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pose {
    eyes: Eyes,
    /// A cell lower, on its feet.
    squat: bool,
    /// Which feet are on the ground.
    feet: [bool; 2],
    /// How far the line under it is written; `None` where it stands and writes nothing.
    written: Option<usize>,
}

/// Four things a face of this size can be seen doing, its idling, and two it rests in. The desk has room to tell searching
/// from reading and drawing from writing; here they share a pose.
fn pose(activity: Activity, beat: usize) -> Pose {
    use Eyes::{At, Glad, Out, Shut};
    let beat = beat % inline_beats(activity);
    let standing = Pose { eyes: Out, squat: false, feet: [true, true], written: None };
    match activity {
        // Along a line, a rest at its end, back to the start of the next; a blink after the third.
        Activity::Reading | Activity::Importing | Activity::Listening => Pose { eyes: if beat == 11 { Shut } else { At([0, 1, 2, 2][beat % 4], false) }, ..standing },
        // Around: down to the left, up, across, down to the right, back.
        Activity::Searching => Pose { eyes: [At(0, false), At(0, true), At(1, true), At(2, true), At(2, false), At(1, false)][beat / 2], ..standing },
        // Up to one side, then to the other, with a dip between.
        Activity::Thinking => Pose { eyes: At([2, 2, 2, 2, 2, 1, 0, 0, 0, 0, 0, 1][beat], true), squat: beat % 6 == 5, ..standing },
        // The eyes follow the line as it grows; it is read over, and a blink clears it.
        Activity::Writing | Activity::Drawing | Activity::Compacting => {
            let written = if beat == 11 { 0 } else { (beat + 1).min(INLINE_W - 2) };
            Pose { eyes: if beat == 11 { Shut } else { At(written * 3 / (INLINE_W - 1), false) }, written: Some(written), ..standing }
        }
        // Looking at the reader, a foot tapping twice.
        Activity::WaitingUser => Pose { eyes: if beat == 9 { Shut } else { Out }, feet: [true, !matches!(beat, 2 | 4)], ..standing },
        // Between two questions it stands and looks out: a blink, and once a loop a look to either side. Little enough
        // not to draw the eye from what is being read.
        Activity::Idle => Pose { eyes: match beat { 12 => Shut, 26..=28 => At(0, false), 30..=32 => At(2, false), _ => Out }, ..standing },
        Activity::Done => Pose { eyes: Glad, ..standing },
        Activity::Paused => Pose { eyes: Shut, ..standing },
    }
}

/// The cells of the face in one beat: `*` the body (`+` where it rests), `w` a lens, `o` a pupil, `#` what it wrote.
fn inline_frame(activity: Activity, beat: usize) -> [[u8; INLINE_W]; INLINE_H] {
    let pose = pose(activity, beat);
    let body = if activity == Activity::Paused { b'+' } else { b'*' };
    let mut rows = FACE.map(|row| row.map(|cell| if cell == b'*' { body } else { cell }));
    for left in LENSES {
        let mut lens = |x: usize, y: usize, glyph: u8| rows[LENS_TOP + y][left + x] = glyph;
        match pose.eyes {
            Eyes::At(x, up) => {
                let top = if up { 0 } else { 1 };
                lens(x, top, b'o');
                lens(x, top + 1, b'o');
            }
            Eyes::Out => lens(1, 1, b'o'),
            Eyes::Shut => (0..3).for_each(|x| {
                lens(x, 0, body);
                lens(x, 2, body);
            }),
            Eyes::Glad => (0..3).for_each(|x| (0..3).for_each(|y| lens(x, y, if (x, y) == (1, 0) || (x != 1 && y == 1) { b'w' } else { body }))),
        }
    }
    let ground = INLINE_H - 1;
    match pose.written {
        Some(written) => rows[ground] = std::array::from_fn(|x| if WRITTEN[x] == b'#' && x <= written { b'#' } else { b'.' }),
        None => FEET.iter().zip(pose.feet).filter(|(_, down)| !down).for_each(|(foot, _)| foot.iter().for_each(|&x| rows[ground][x] = b'.')),
    }
    if pose.squat {
        rows.copy_within(0..ground, 1);
        rows[0] = [b'.'; INLINE_W];
    }
    rows
}

fn face_color(glyph: u8) -> Option<Hsla> {
    // The white of an eye is the lighter of paper and ink in either set of colours, the pupil the darker.
    let (white, pupil) = if theme::dark() { (Theme::text(), Theme::bg_body()) } else { (Theme::bg_body(), Theme::text()) };
    match glyph {
        b'*' => Some(Theme::accent()),
        b'+' => Some(Theme::text_tertiary()),
        b'w' => Some(white),
        b'o' => Some(pupil),
        b'#' => Some(Theme::text()),
        _ => None,
    }
}

fn paint_inline(bounds: Bounds<Pixels>, cell: f32, activity: Activity, beat: usize, window: &mut Window) {
    // The nearer whole number of the screen's pixels: at 125 % the face is a little past its box, not smaller than meant.
    let cell = whole_cell(cell, window.scale_factor(), true);
    let origin = point(bounds.origin.x + (bounds.size.width - px(INLINE_W as f32 * cell)) / 2., bounds.origin.y + (bounds.size.height - px(INLINE_H as f32 * cell)) / 2.);
    paint_grid(origin, cell, &inline_frame(activity, beat), face_color, window);
}

#[cfg(test)]
mod tests {
    // Do not `use super::*;` here: `gpui::*` in the parent module brings in gpui's own `#[test]` attribute.
    use super::{
        Activity, DESK_COFFEE_LIFT, DESK_COFFEE_SIP_A, DESK_COFFEE_SIP_B, DESK_COMPACT_A, DESK_COMPACT_B, DESK_COMPACT_C,
        DESK_DONE_CELEBRATE, DESK_DONE_REST, DESK_DRAW_A, DESK_DRAW_ADMIRE, DESK_DRAW_B, DESK_DRAW_C, DESK_GLASSES_GLINT,
        DESK_IMPORT_A, DESK_IMPORT_B, DESK_IMPORT_C, DESK_LISTEN_A, DESK_LISTEN_B, DESK_LISTEN_C, DESK_PAUSED, DESK_READ_FLIP,
        DESK_READ_LEFT, DESK_READ_MID, DESK_READ_RIGHT, DESK_SEARCH_LEFT, DESK_SEARCH_MID, DESK_SEARCH_RIGHT, DESK_SPRITE_H,
        DESK_STRETCH, DESK_THINK_A, DESK_THINK_B, DESK_THINK_C, DESK_WAIT_A, DESK_WAIT_B, DESK_WIPE_BROW, DESK_WRITE_A,
        DESK_WRITE_B, DESK_WRITE_C, Eyes, FACE, IDLE_BEATS, INLINE_BEATS, inline_beats, inline_frame, INLINE_H, INLINE_W, LENS_TOP, LENSES, on_pixel,
        paper_stacks, pose, SPRITE_W, whole_cell,
    };

    const VALID_GLYPHS: &[u8] = b". #@+-wp*sgy";

    #[test]
    fn desk_sprites_have_exact_dimensions_and_valid_glyphs() {
        let frames: &[&[&str; DESK_SPRITE_H]] = &[
            DESK_READ_LEFT, DESK_READ_MID, DESK_READ_RIGHT, DESK_READ_FLIP,
            DESK_SEARCH_LEFT, DESK_SEARCH_MID, DESK_SEARCH_RIGHT,
            DESK_WRITE_A, DESK_WRITE_B, DESK_WRITE_C,
            DESK_DRAW_A, DESK_DRAW_B, DESK_DRAW_C, DESK_DRAW_ADMIRE,
            DESK_THINK_A, DESK_THINK_B, DESK_THINK_C,
            DESK_IMPORT_A, DESK_IMPORT_B, DESK_IMPORT_C,
            DESK_COMPACT_A, DESK_COMPACT_B, DESK_COMPACT_C,
            DESK_LISTEN_A, DESK_LISTEN_B, DESK_LISTEN_C,
            DESK_WAIT_A, DESK_WAIT_B,
            DESK_COFFEE_LIFT, DESK_COFFEE_SIP_A, DESK_COFFEE_SIP_B,
            DESK_GLASSES_GLINT, DESK_WIPE_BROW, DESK_STRETCH,
            DESK_DONE_CELEBRATE, DESK_DONE_REST, DESK_PAUSED,
        ];
        for (idx, frame) in frames.iter().enumerate() {
            assert_eq!(frame.len(), DESK_SPRITE_H, "desk frame {idx} height");
            for (y, row) in frame.iter().enumerate() {
                assert!(row.is_ascii(), "desk frame {idx} row {y} must be ASCII");
                assert_eq!(row.len(), SPRITE_W, "desk frame {idx} row {y} width: {row}");
                for byte in row.as_bytes() {
                    assert!(VALID_GLYPHS.contains(byte), "desk frame {idx} row {y} invalid glyph {:?}", *byte as char);
                }
            }
        }
    }

    const ALL: [Activity; 12] = [
        Activity::Idle,
        Activity::Reading, Activity::Searching, Activity::Thinking, Activity::Writing, Activity::Drawing, Activity::Compacting,
        Activity::Importing, Activity::Listening, Activity::WaitingUser, Activity::Done, Activity::Paused,
    ];

    #[test]
    fn the_face_keeps_its_shape_in_every_beat() {
        for activity in ALL {
            for beat in 0..inline_beats(activity) {
                let frame = inline_frame(activity, beat);
                let body = if activity == Activity::Paused { b'+' } else { b'*' };
                for (y, row) in frame.iter().enumerate() {
                    for (x, &cell) in row.iter().enumerate() {
                        assert!(b".*+wo#".contains(&cell), "{activity:?} beat {beat}: glyph {:?} at {x};{y}", cell as char);
                        // Only one fill: a face at rest is all `+`, one at work all `*`.
                        assert!(cell != if body == b'*' { b'+' } else { b'*' }, "{activity:?} beat {beat}: two fills");
                    }
                }
                // A squat moves the head a row down and nothing else; the pupils stay inside the lenses either way.
                let down = usize::from(pose(activity, beat).squat);
                for (y, row) in frame.iter().enumerate() {
                    for (x, &cell) in row.iter().enumerate() {
                        let in_lens = (LENS_TOP + down..LENS_TOP + down + 3).contains(&y) && LENSES.iter().any(|&left| (left..left + 3).contains(&x));
                        if matches!(cell, b'o' | b'w') { assert!(in_lens, "{activity:?} beat {beat}: an eye outside its lens at {x};{y}") }
                        if cell == b'#' { assert_eq!(y, INLINE_H - 1, "{activity:?} beat {beat}: writing off the ground") }
                    }
                }
                // The outline is the one drawn, wherever the eyes look: what moves is inside it or under it.
                for y in 0..INLINE_H - 1 - down {
                    for x in 0..INLINE_W {
                        assert_eq!(frame[y + down][x] == b'.', FACE[y][x] == b'.', "{activity:?} beat {beat}: outline at {x};{y}");
                    }
                }
            }
        }
    }

    #[test]
    fn what_the_face_does_can_be_told_apart() {
        let frames = |activity| (0..INLINE_BEATS).map(|beat| inline_frame(activity, beat)).collect::<Vec<_>>();
        // The four it can be seen doing differ from one another, and each of them moves.
        let shown = [Activity::Reading, Activity::Thinking, Activity::Writing, Activity::WaitingUser];
        for (n, a) in shown.iter().enumerate() {
            let of_a = frames(*a);
            assert!(of_a.iter().any(|frame| *frame != of_a[0]), "{a:?} stands still");
            for b in &shown[n + 1..] { assert_ne!(of_a, frames(*b), "{a:?} and {b:?} look the same") }
        }
        // Reading goes along a line and blinks at the end of the third; thinking looks up and dips twice.
        assert_eq!((0..4).map(|beat| pose(Activity::Reading, beat).eyes).collect::<Vec<_>>(), [Eyes::At(0, false), Eyes::At(1, false), Eyes::At(2, false), Eyes::At(2, false)]);
        assert_eq!(pose(Activity::Reading, 11).eyes, Eyes::Shut);
        assert!((0..INLINE_BEATS).all(|beat| matches!(pose(Activity::Thinking, beat).eyes, Eyes::At(_, true))));
        assert_eq!((0..INLINE_BEATS).filter(|&beat| pose(Activity::Thinking, beat).squat).count(), 2);
        // A line is written from its first word to its last, the eyes going with it, and cleared.
        let written: Vec<usize> = (0..INLINE_BEATS).map(|beat| pose(Activity::Writing, beat).written.unwrap()).collect();
        assert_eq!(written, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 10, 0]);
        assert_eq!([0, 5, 9].map(|beat| pose(Activity::Writing, beat).eyes), [Eyes::At(0, false), Eyes::At(1, false), Eyes::At(2, false)]);
        assert_eq!(inline_frame(Activity::Writing, 9)[INLINE_H - 1], *b".##.###.###.");
        assert_eq!(inline_frame(Activity::Writing, 11)[INLINE_H - 1], *b"............");
        // Waiting looks at the reader and taps one foot; the two at rest do not move.
        assert_eq!(inline_frame(Activity::WaitingUser, 0)[INLINE_H - 1], *b"..**....**..");
        assert_eq!(inline_frame(Activity::WaitingUser, 2)[INLINE_H - 1], *b"..**........");
        // With nothing to do it is still but for a blink and two looks aside, in a loop of eight seconds.
        let idle: Vec<_> = (0..IDLE_BEATS).map(|beat| inline_frame(Activity::Idle, beat)).collect();
        assert_eq!(idle.iter().filter(|frame| **frame == idle[0]).count(), IDLE_BEATS - 7);
        assert_eq!(idle[0], inline_frame(Activity::WaitingUser, 0));
        for still in [Activity::Done, Activity::Paused] { assert!(frames(still).iter().all(|frame| *frame == inline_frame(still, 0))) }
    }

    #[test]
    fn cells_are_whole_pixels_of_the_screen() {
        for scale_factor in [1., 1.25, 1.5, 1.75, 2., 2.5, 3.] {
            for nominal in [2., 3.] {
                for grow in [false, true] {
                    let cell = whole_cell(nominal, scale_factor, grow);
                    let device = cell * scale_factor;
                    assert!((device - device.round()).abs() < 1e-4 && device >= 1., "{nominal} px at {scale_factor}: {device} device px");
                    if !grow { assert!(cell <= nominal + 1e-4, "{nominal} px at {scale_factor} grew to {cell}") }
                    assert!((cell - nominal).abs() <= 0.5 / scale_factor + 1e-4 || !grow, "{nominal} px at {scale_factor}: {cell}");
                    // Every edge of a row of cells is on a pixel, and the cells are all as wide, wherever the row begins.
                    let edges: Vec<f32> = (0..=INLINE_W).map(|n| on_pixel(13.37 + n as f32 * cell, scale_factor) * scale_factor).collect();
                    for pair in edges.windows(2) {
                        assert!((pair[0] - pair[0].round()).abs() < 1e-3, "an edge between two pixels at {scale_factor}");
                        assert!((pair[1] - pair[0] - device).abs() < 1e-3, "cells of two widths at {scale_factor}");
                    }
                }
            }
        }
        // On the screens most have, a cell is what was asked for.
        assert_eq!([1., 2.].map(|scale_factor| whole_cell(2., scale_factor, true)), [2., 2.]);
        assert_eq!(whole_cell(2., 1.5, false), 2.);
        // At 125 % the face takes three of the screen's pixels a cell and the desk, which must fit its card, two.
        assert_eq!((whole_cell(2., 1.25, true) * 1.25, whole_cell(2., 1.25, false) * 1.25), (3., 2.));
    }

    #[test]
    fn tool_calls_map_to_expected_activities() {
        assert_eq!(Activity::from_tool_call("检索全文：root hair"), Activity::Searching);
        assert_eq!(Activity::from_tool_call("搜索网页：CRISPR"), Activity::Searching);
        assert_eq!(Activity::from_tool_call("查期刊分区与影响因子：Nature"), Activity::Searching);
        assert_eq!(Activity::from_tool_call("读取文献：P1"), Activity::Reading);
        assert_eq!(Activity::from_tool_call("读取思维导图：P2"), Activity::Reading);
        assert_eq!(Activity::from_tool_call("改写段落：Methods"), Activity::Writing);
        assert_eq!(Activity::from_tool_call("留批注：Intro"), Activity::Writing);
        assert_eq!(Activity::from_tool_call("导入本地 PDF：a.pdf"), Activity::Importing);
        assert_eq!(Activity::from_tool_call("生图画机制图：diagram"), Activity::Drawing);
        assert_eq!(Activity::from_tool_call("其他未命名工具"), Activity::Thinking);
    }

    #[test]
    fn run_state_and_paper_stacks_handle_boundaries() {
        assert_eq!(Activity::from_run(true, 3, 10, Activity::Reading), Activity::Reading);
        assert_eq!(Activity::from_run(false, 10, 10, Activity::Reading), Activity::Done);
        assert_eq!(Activity::from_run(false, 12, 10, Activity::Reading), Activity::Done);
        assert_eq!(Activity::from_run(false, 4, 10, Activity::Reading), Activity::Paused);
        assert_eq!(Activity::from_run(false, 0, 0, Activity::Reading), Activity::Paused);

        assert_eq!(paper_stacks(0, 0, 6), (0, 0));
        assert_eq!(paper_stacks(0, 10, 6), (6, 0));
        assert_eq!(paper_stacks(5, 10, 6), (3, 3));
        assert_eq!(paper_stacks(10, 10, 6), (0, 6));
        assert_eq!(paper_stacks(12, 10, 6), (0, 6));
    }
}
