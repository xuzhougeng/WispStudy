//! Pixel-art researcher state machine and miniature progress-desk diorama.
//!
//! Driven by real task state (`Activity`, `done / total`) and rendered directly
//! via GPUI `canvas` + `paint_quad` using `Theme::*` semantic tokens so light
//! and dark themes, high contrast, and all seven library accents match.

use crate::theme::{self, Theme};
use crate::ui;
use gpui::*;
use std::time::Duration;

const SPRITE_W: usize = 16;
const DESK_SPRITE_H: usize = 15;
const INLINE_H: usize = 16;
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

/// Compact inline researcher icon (`size_px × size_px`) for chat rows and sidebar progress.
pub fn inline(id: impl std::fmt::Display, activity: Activity, size_px: f32) -> AnyElement {
    if !theme::work_animation() {
        return quiet_inline(id, activity, size_px);
    }
    let anim_id = SharedString::from(format!("{id}-{activity:?}"));
    match activity {
        Activity::Paused => canvas(|_, _, _| (), move |bounds, _, window, _| {
            paint_inline_frame(bounds, inline_paused_frame(), window);
        })
        .size(px(size_px))
        .flex_none()
        .into_any_element(),
        Activity::Done => canvas(|_, _, _| (), move |bounds, _, window, _| {
            paint_inline_frame(bounds, inline_done_frame(), window);
        })
        .size(px(size_px))
        .flex_none()
        .into_any_element(),
        _ => div()
            .flex_none()
            .size(px(size_px))
            .with_animation(anim_id, Animation::new(Duration::from_millis(4000)).repeat(), move |box_el, t| {
                let beat = ((t * 20.0).floor() as usize).min(19);
                box_el.child(
                    canvas(|_, _, _| (), move |bounds, _, window, _| {
                        paint_inline_frame(bounds, inline_frame(activity, beat), window);
                    })
                    .size_full(),
                )
            })
            .into_any_element(),
    }
}

fn quiet_inline(id: impl std::fmt::Display, activity: Activity, size_px: f32) -> AnyElement {
    let icon_size = (size_px - 5.).max(11.);
    match activity {
        Activity::WaitingUser => ui::icon("info", icon_size.max(13.), Theme::accent()).into_any_element(),
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

fn paint_rect(origin: Point<Pixels>, scale: f32, gx: f32, gy: f32, gw: f32, gh: f32, color: Hsla, window: &mut Window) {
    let ox = f32::from(origin.x).floor();
    let oy = f32::from(origin.y).floor();
    let rect = Bounds::new(point(px(ox + gx * scale), px(oy + gy * scale)), size(px(gw * scale), px(gh * scale)));
    window.paint_quad(fill(rect, color));
}

fn paint_grid(origin: Point<Pixels>, scale: f32, rows: &[&str], window: &mut Window) {
    let ox = f32::from(origin.x).floor();
    let oy = f32::from(origin.y).floor();
    for (y, row) in rows.iter().enumerate() {
        let bytes = row.as_bytes();
        let mut x = 0;
        while x < bytes.len() {
            let ch = bytes[x];
            let Some(color) = palette_color(ch) else {
                x += 1;
                continue;
            };
            let start = x;
            while x < bytes.len() && bytes[x] == ch {
                x += 1;
            }
            let rect = Bounds::new(
                point(px(ox + start as f32 * scale), px(oy + y as f32 * scale)),
                size(px((x - start) as f32 * scale), px(scale)),
            );
            window.paint_quad(fill(rect, color));
        }
    }
}

fn paint_inline_frame(bounds: Bounds<Pixels>, rows: &[&str; INLINE_H], window: &mut Window) {
    let scale = (f32::from(bounds.size.width).min(f32::from(bounds.size.height)) / SPRITE_W as f32).max(1.);
    let ox = f32::from(bounds.origin.x) + ((f32::from(bounds.size.width) - SPRITE_W as f32 * scale) * 0.5).floor();
    let oy = f32::from(bounds.origin.y) + ((f32::from(bounds.size.height) - INLINE_H as f32 * scale) * 0.5).floor();
    paint_grid(point(px(ox), px(oy)), scale, rows, window);
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
    let origin = bounds.origin;
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
    paint_grid(sprite_origin, scale, desk_sprite(activity, beat), window);

    // 4. Activity status badge on paused/done states.
    if activity == Activity::Paused {
        paint_grid(point(px(f32::from(origin.x) + 27. * scale), px(f32::from(origin.y) + 4. * scale)), scale, PAUSE_MARK, window);
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
        Activity::WaitingUser => match beat % 4 {
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

// ---- compact inline researcher 16 × 16 frames

fn inline_frame(activity: Activity, beat: usize) -> &'static [&'static str; INLINE_H] {
    match activity {
        Activity::Reading => match beat {
            16 | 17 => INLINE_COFFEE_A,
            18 | 19 => INLINE_COFFEE_B,
            b => match b % 4 {
                0 => INLINE_READ_A,
                1 => INLINE_READ_B,
                2 => INLINE_READ_C,
                _ => INLINE_READ_B,
            },
        },
        Activity::Searching => match beat {
            16..=19 => INLINE_GLINT,
            b => match b % 4 {
                0 => INLINE_SEARCH_A,
                1 => INLINE_SEARCH_B,
                2 => INLINE_SEARCH_C,
                _ => INLINE_SEARCH_B,
            },
        },
        Activity::Writing => match beat {
            16 | 17 => INLINE_COFFEE_A,
            18 | 19 => INLINE_COFFEE_B,
            b => match b % 4 {
                0 => INLINE_WRITE_A,
                1 => INLINE_WRITE_B,
                2 => INLINE_WRITE_C,
                _ => INLINE_WRITE_B,
            },
        },
        Activity::Drawing => match beat {
            16..=19 => INLINE_COFFEE_A,
            b => match b % 4 {
                0 => INLINE_DRAW_A,
                1 => INLINE_DRAW_B,
                2 => INLINE_DRAW_C,
                _ => INLINE_DRAW_B,
            },
        },
        Activity::Thinking => match beat {
            16 | 17 => INLINE_COFFEE_A,
            18 | 19 => INLINE_COFFEE_B,
            b => match b % 4 {
                0 => INLINE_THINK_A,
                1 => INLINE_THINK_B,
                2 => INLINE_THINK_C,
                _ => INLINE_THINK_B,
            },
        },
        Activity::Compacting | Activity::Importing => match beat {
            16..=19 => INLINE_WIPE,
            b => match b % 4 {
                0 => INLINE_WORK_A,
                1 => INLINE_WORK_B,
                2 => INLINE_WORK_C,
                _ => INLINE_WORK_B,
            },
        },
        Activity::Listening => match beat % 2 {
            0 => INLINE_LISTEN_A,
            _ => INLINE_LISTEN_B,
        },
        Activity::WaitingUser => match beat % 2 {
            0 => INLINE_WAIT_A,
            _ => INLINE_WAIT_B,
        },
        Activity::Done => inline_done_frame(),
        Activity::Paused => inline_paused_frame(),
    }
}

fn inline_done_frame() -> &'static [&'static str; INLINE_H] {
    INLINE_DONE
}

fn inline_paused_frame() -> &'static [&'static str; INLINE_H] {
    INLINE_PAUSED
}

const INLINE_READ_A: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..##ww####ww##..",
    "..#w#w#ww#w#w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@@++..",
    "..+wwww**wwww+..",
    ".+wwwwwwwwwwww+.",
    ".--------------.",
    "................",
];

const INLINE_READ_B: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..###w####w###..",
    "..#w#w#ww#w#w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@@++..",
    "..+wwww**wwww+..",
    ".+wwwwwwwwwwww+.",
    ".--------------.",
    "................",
];

const INLINE_READ_C: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..##ww####ww##..",
    "..#ww#w##w#ww#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@+*+..",
    "..+wwww**ww*w*+.",
    ".+wwwwwwwwwwww+.",
    ".--------------.",
    "................",
];

const INLINE_SEARCH_A: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..###ww##ww###..",
    "..##ww####ww##..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "..***@@@@@@@@...",
    ".*ss*+@@@@@@@@..",
    ".*ss*+@@@@@@@@..",
    "..***@@@@@@@@...",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
];

const INLINE_SEARCH_B: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..###w####w###..",
    "..#w#w#ww#w#w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "....***@@@@@@...",
    "...*ss*+@@@@@@..",
    "...*ss*+@@@@@@..",
    "....***@@@@@@...",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
];

const INLINE_SEARCH_C: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..##ww####ww##..",
    "..#ww##ww##ww#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@***..",
    "..@@@@@@@@+*ss*.",
    "..@@@@@@@@+*ss*.",
    "...@@@@@@@@***..",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
];

const INLINE_WRITE_A: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..###w####w###..",
    "..#w#w#ww#w#w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@*@@++..",
    "..+wwwww*wwww+..",
    "..+ww**wwwwww+..",
    "..------------..",
    "................",
];

const INLINE_WRITE_B: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..###w####w###..",
    "..#w#w#ww#w#w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@*@++..",
    "..+wwwwww*www+..",
    "..+ww***wwwww+..",
    "..------------..",
    "................",
];

const INLINE_WRITE_C: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..###w####w###..",
    "..#w#w#ww#w#w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@*++..",
    "..+wwwwwww*ww+..",
    "..+ww****wwww+..",
    "..------------..",
    "................",
];

const INLINE_DRAW_A: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..###w####w###..",
    "..#w#w#ww#w#w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@*+...",
    "..@@@@@@@@*++...",
    "..++@@@@@@+.....",
    "..+wwww*wwww+...",
    "..+www***www+...",
    "..------------..",
    "................",
];

const INLINE_DRAW_B: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..###w####w###..",
    "..#w#w#ww#w#w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########.+..",
    "...@@@@@@@@+*...",
    "..@@@@@@@@++....",
    "..++@@@@@@+.....",
    "..+www**wwww+...",
    "..+www***www+...",
    "..------------..",
    "................",
];

const INLINE_DRAW_C: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..###w####w###..",
    "..#w#w#ww#w#w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@++...",
    "..@@@@@@@@+**...",
    "..++@@@@@@+.....",
    "..+www***www+...",
    "..+www*ww*ww+...",
    "..------------..",
    "................",
];

const INLINE_THINK_A: &[&str; INLINE_H] = &[
    "..........*.....",
    "....########....",
    "...##########...",
    "...#w#wwww#w#...",
    "..#www####www#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########+...",
    "...@@@@@@@@+@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@@++..",
    "...@@@@@@@@@@...",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
    "................",
];

const INLINE_THINK_B: &[&str; INLINE_H] = &[
    "........*...*...",
    "....########....",
    "...##########...",
    "...#w#wwww#w#...",
    "..#www####www#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########+...",
    "...@@@@@@@@+@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@@++..",
    "...@@@@@@@@@@...",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
    "................",
];

const INLINE_THINK_C: &[&str; INLINE_H] = &[
    "......*..*..*...",
    "....########....",
    "...##########...",
    "...#w#wwww#w#...",
    "..#www####www#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########+...",
    "...@@@@@@@@+@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@@++..",
    "...@@@@@@@@@@...",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
    "................",
];

const INLINE_WORK_A: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..###w####w###..",
    "..#w#w#ww#w#w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@@@...",
    "..++@@@@@@@@++..",
    "...+wwwwwwww+...",
    "...+--****--+...",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
];

const INLINE_WORK_B: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..###w####w###..",
    "..#w#w#ww#w#w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@@++..",
    "...+ww****ww+...",
    "...+--****--+...",
    "...----------...",
    "................",
];

const INLINE_WORK_C: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..###w####w###..",
    "..#w#w#ww#w#w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@@++..",
    "...@@@@@@@@@@...",
    "...+ww****ww+...",
    "...----------...",
    "................",
];

const INLINE_LISTEN_A: &[&str; INLINE_H] = &[
    "..........*.....",
    "....########....",
    "...*########*...",
    "..*#wwwwwwww#*..",
    "..*###wwww###*..",
    "..*#w#w##w#w#*..",
    "...####ww####...",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@@++..",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
    "................",
];

const INLINE_LISTEN_B: &[&str; INLINE_H] = &[
    "........*..**...",
    "...*########*...",
    "..*##########*..",
    "..*#wwwwwwww#*..",
    "..*###wwww###*..",
    "...#w#w##w#w#...",
    "...####ww####...",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@@++..",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
    "................",
];

const INLINE_WAIT_A: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########.*.",
    "...#wwwwwwww#*w*",
    "..###w####w##***",
    "..#w#w#ww#w#w#+.",
    "..#####ww#####+.",
    "...#wwwwwwww#.+.",
    "....########.++.",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
    "................",
];

const INLINE_WAIT_B: &[&str; INLINE_H] = &[
    ".............*..",
    "....########*w*.",
    "...##########***",
    "...#wwwwwwww#.+.",
    "..###w####w##.+.",
    "..#w#w#ww#w#w#+.",
    "..#####ww#####+.",
    "...#wwwwwwww#.+.",
    "....########.++.",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@@....",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
    "................",
];

const INLINE_COFFEE_A: &[&str; INLINE_H] = &[
    ".............+..",
    "....########....",
    "...##########+..",
    "...#wwwwwwww#...",
    "..#w##wwww##w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....##wwww##.+..",
    "...@@@@@@@@www..",
    "..@@@@@@@@@+@@+.",
    "..++@@@@@@@+@@..",
    "...@@@@@@@@@@...",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
    "................",
];

const INLINE_COFFEE_B: &[&str; INLINE_H] = &[
    "........+.......",
    "....########....",
    "...##########...",
    "...#wwwwwwww#+..",
    "..#w##wwww##w#..",
    "..#####ww#####..",
    "...#wwwwwwww#+..",
    "....##www###....",
    "...@@@@+@@+@@...",
    "..@@@@@@@@+@@...",
    "..++@@@@@@@@++..",
    "...@@@@@@@@@@...",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
    "................",
];

const INLINE_GLINT: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    ".+##*w*##*w*#...",
    ".+#####ww#####..",
    "..+#wwwwwwww#...",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@@++..",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
    "................",
];

const INLINE_WIPE: &[&str; INLINE_H] = &[
    ".............y..",
    "....########.y..",
    "...##########...",
    ".++#wwwwwwww#...",
    ".+#####ww#####..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "....@@@@@@@@....",
    "...@@@@@@@@@@...",
    "...@@@@@@@@@++..",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
    "................",
    "................",
];

const INLINE_DONE: &[&str; INLINE_H] = &[
    "..............g.",
    "....########.g..",
    "...##########g..",
    "...#wwwwwwww#...",
    "..#w##wwww##w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@www..",
    "..@@@@@@@@@+@@+.",
    "..++@@@@@@@+@@..",
    "...@@@@@@@@@@...",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
    "................",
];

const INLINE_PAUSED: &[&str; INLINE_H] = &[
    "................",
    "....########....",
    "...##########...",
    "...#wwwwwwww#...",
    "..###w####w###..",
    "..#w#w#ww#w#w#..",
    "..#####ww#####..",
    "...#wwwwwwww#...",
    "....########....",
    "...@@@@@@@@@@...",
    "..@@@@@@@@@@@@..",
    "..++@@@@@@@@++..",
    "...+wwwwwwww+...",
    "...----------...",
    "................",
    "................",
];

#[cfg(test)]
mod tests {
    // Do not `use super::*;` here: `gpui::*` in the parent module brings in gpui's own `#[test]` attribute.
    use super::{
        Activity, DESK_COMPACT_A, DESK_COMPACT_B, DESK_COMPACT_C, DESK_COFFEE_LIFT, DESK_COFFEE_SIP_A, DESK_COFFEE_SIP_B,
        DESK_DONE_CELEBRATE, DESK_DONE_REST, DESK_DRAW_A, DESK_DRAW_ADMIRE, DESK_DRAW_B, DESK_DRAW_C, DESK_GLASSES_GLINT,
        DESK_IMPORT_A, DESK_IMPORT_B, DESK_IMPORT_C, DESK_LISTEN_A, DESK_LISTEN_B, DESK_LISTEN_C, DESK_PAUSED, DESK_READ_FLIP,
        DESK_READ_LEFT, DESK_READ_MID, DESK_READ_RIGHT, DESK_SEARCH_LEFT, DESK_SEARCH_MID, DESK_SEARCH_RIGHT, DESK_SPRITE_H,
        DESK_STRETCH, DESK_THINK_A, DESK_THINK_B, DESK_THINK_C, DESK_WAIT_A, DESK_WAIT_B, DESK_WIPE_BROW, DESK_WRITE_A,
        DESK_WRITE_B, DESK_WRITE_C, INLINE_COFFEE_A, INLINE_COFFEE_B, INLINE_DONE, INLINE_DRAW_A, INLINE_DRAW_B, INLINE_DRAW_C,
        INLINE_GLINT, INLINE_H, INLINE_LISTEN_A, INLINE_LISTEN_B, INLINE_PAUSED, INLINE_READ_A, INLINE_READ_B, INLINE_READ_C,
        INLINE_SEARCH_A, INLINE_SEARCH_B, INLINE_SEARCH_C, INLINE_THINK_A, INLINE_THINK_B, INLINE_THINK_C, INLINE_WAIT_A,
        INLINE_WAIT_B, INLINE_WIPE, INLINE_WORK_A, INLINE_WORK_B, INLINE_WORK_C, INLINE_WRITE_A, INLINE_WRITE_B, INLINE_WRITE_C,
        SPRITE_W, paper_stacks,
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

    #[test]
    fn inline_sprites_have_exact_dimensions_and_valid_glyphs() {
        let frames: &[&[&str; INLINE_H]] = &[
            INLINE_READ_A, INLINE_READ_B, INLINE_READ_C,
            INLINE_SEARCH_A, INLINE_SEARCH_B, INLINE_SEARCH_C,
            INLINE_WRITE_A, INLINE_WRITE_B, INLINE_WRITE_C,
            INLINE_DRAW_A, INLINE_DRAW_B, INLINE_DRAW_C,
            INLINE_THINK_A, INLINE_THINK_B, INLINE_THINK_C,
            INLINE_WORK_A, INLINE_WORK_B, INLINE_WORK_C,
            INLINE_LISTEN_A, INLINE_LISTEN_B,
            INLINE_WAIT_A, INLINE_WAIT_B,
            INLINE_COFFEE_A, INLINE_COFFEE_B, INLINE_GLINT, INLINE_WIPE,
            INLINE_DONE, INLINE_PAUSED,
        ];
        for (idx, frame) in frames.iter().enumerate() {
            assert_eq!(frame.len(), INLINE_H, "inline frame {idx} height");
            for (y, row) in frame.iter().enumerate() {
                assert!(row.is_ascii(), "inline frame {idx} row {y} must be ASCII");
                assert_eq!(row.len(), SPRITE_W, "inline frame {idx} row {y} width: {row}");
                for byte in row.as_bytes() {
                    assert!(VALID_GLYPHS.contains(byte), "inline frame {idx} row {y} invalid glyph {:?}", *byte as char);
                }
            }
        }
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
