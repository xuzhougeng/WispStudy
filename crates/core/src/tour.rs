//! 新手引导: the steps a first window walks a person through, which of them is
//! on show, and where its card stands. The first three are done by doing them
//! (a library, papers in it, one of them opened), so the step on show is read
//! off the window: it is always the earliest one whose ground is not there
//! yet, and never points at something that is not on screen. The last three
//! only show a place, and go on at 下一步. Plain arithmetic in logical pixels;
//! the window draws whatever these give (`tour.rs` of the app).

use crate::settings::Settings;

/// One step of the tour, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Make a library, or open one.
    Library,
    /// Put papers into it.
    Papers,
    /// Open one of them.
    Open,
    /// The paper's three tabs: read it, ask about it.
    Read,
    /// The library's assistant.
    Ask,
    /// A review's path down the sidebar: 检索与导入, 筛选, 提取, 写作.
    Path,
}

impl Step {
    pub const ALL: [Step; 6] = [Step::Library, Step::Papers, Step::Open, Step::Read, Step::Ask, Step::Path];

    /// Whether its card stands under what it points at even where there is room beside it (`place`).
    pub fn card_under(self) -> bool {
        self == Step::Library
    }

    /// Its place among the steps, from 1: what the card says before 「/ 6」, and what the settings keep.
    pub fn number(self) -> usize {
        Step::ALL.iter().position(|step| *step == self).unwrap_or(0) + 1
    }

    /// The step of a number kept; one out of range is the nearest end.
    pub fn of(number: usize) -> Step {
        Step::ALL[number.clamp(1, Step::ALL.len()) - 1]
    }

    /// Whether doing what it says is what moves on from it: its card has no 下一步.
    pub fn done_by_doing(self) -> bool {
        matches!(self, Step::Library | Step::Papers | Step::Open)
    }
}

/// What of the window the steps go by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// A library is open (a conversation not yet made a library is none).
    pub library: bool,
    /// It holds a paper.
    pub papers: bool,
    /// One of them is opened beside the list.
    pub selected: bool,
}

/// A first start begins the tour: nothing on the shelf yet, or a tour begun earlier and neither finished nor closed.
/// Once it ended, either way, it comes back only when asked for (设置 › 关于).
pub fn due(settings: &Settings, libraries: usize) -> bool {
    !settings.tour_done && (settings.tour_step > 0 || libraries == 0)
}

/// How far the tour has got, given how far it had (`Settings::tour_step`) and what the window shows now: what the
/// person has done moves it on by itself, as far as the first step that only shows a place.
pub fn reached(reached: usize, seen: Seen) -> usize {
    let mut at = reached.clamp(1, Step::ALL.len());
    if at == 1 && seen.library { at = 2 }
    if at == 2 && seen.library && seen.papers { at = 3 }
    if at == 3 && seen.library && seen.papers && seen.selected { at = 4 }
    at
}

/// The step to show: the one reached, or an earlier one whose ground has gone since (the library was left,
/// the paper closed). The assistant and the sidebar's path need only a library.
pub fn shown(reached: usize, seen: Seen) -> Step {
    let at = Step::of(reached);
    if !seen.library { return Step::Library }
    if matches!(at, Step::Ask | Step::Path) { return at }
    if !seen.papers { return Step::of(at.number().min(Step::Papers.number())) }
    if !seen.selected { return Step::of(at.number().min(Step::Open.number())) }
    at
}

/// The tutorials of the project page, in the interface's language: where the last card sends whoever wants more.
pub fn tutorials_url(english: bool) -> &'static str {
    if english { "https://xuzhougeng.github.io/WispStudy/en/tutorials/" } else { "https://xuzhougeng.github.io/WispStudy/tutorials/" }
}

/// A place and size in the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// How far the card stands from what it points at, and from the window's edges.
pub const GAP: f32 = 14.;
pub const MARGIN: f32 = 12.;
/// How far from the window's corner the card waits when what it points at is not on screen.
pub const CORNER: f32 = 24.;

/// Where the card waits when its step's place is not on screen: the window's bottom left corner.
pub fn docked(card: (f32, f32), window: (f32, f32)) -> (f32, f32) {
    (CORNER.min((window.0 - card.0).max(0.)), (window.1 - CORNER - card.1).max(0.))
}

/// Where the card of a step stands: beside what it points at, never over it. To its right and level with its top
/// where there is room (a row of the sidebar), else under it, else to its left, else above. `under` asks for under it
/// first: on the home page the place to the right of a library is the next library. Under or above something much
/// wider than itself (a row of the table) it keeps to that thing's right end, where a row holds less. With no room
/// on any side it waits in the corner.
pub fn place(target: Rect, card: (f32, f32), window: (f32, f32), under: bool) -> (f32, f32) {
    let (width, height) = card;
    let fits_x = |x: f32| x >= MARGIN && x + width <= window.0 - MARGIN;
    let fits_y = |y: f32| y >= MARGIN && y + height <= window.1 - MARGIN;
    let within_x = |x: f32| x.min(window.0 - MARGIN - width).max(MARGIN);
    let within_y = |y: f32| y.min(window.1 - MARGIN - height).max(MARGIN);
    let along = if target.width > width * 2. { target.x + target.width - width } else { target.x };
    let (right, below) = (target.x + target.width + GAP, target.y + target.height + GAP);
    if under && fits_y(below) { return (within_x(along), below) }
    if fits_x(right) { return (right, within_y(target.y)) }
    if fits_y(below) { return (within_x(along), below) }
    let left = target.x - GAP - width;
    if fits_x(left) { return (left, within_y(target.y)) }
    let above = target.y - GAP - height;
    if fits_y(above) { return (within_x(along), above) }
    docked(card, window)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTHING: Seen = Seen { library: false, papers: false, selected: false };
    const EMPTY: Seen = Seen { library: true, papers: false, selected: false };
    const FILLED: Seen = Seen { library: true, papers: true, selected: false };
    const READING: Seen = Seen { library: true, papers: true, selected: true };

    #[test]
    fn a_first_start_begins_it_and_an_answer_ends_it() {
        let mut settings = Settings::default();
        assert!(due(&settings, 0));
        // Someone with libraries who never saw it is not walked through what they use every day.
        assert!(!due(&settings, 3));
        // Begun and not finished, it is taken up again at the next start, libraries or not.
        settings.tour_step = 2;
        assert!(due(&settings, 1));
        settings.tour_done = true;
        assert!(!due(&settings, 1));
        assert!(!due(&settings, 0));
    }

    #[test]
    fn doing_a_step_moves_on_from_it() {
        assert_eq!(reached(0, NOTHING), 1);
        assert_eq!(reached(1, NOTHING), 1);
        assert_eq!(reached(1, EMPTY), 2);
        assert_eq!(reached(2, EMPTY), 2);
        assert_eq!(reached(2, FILLED), 3);
        assert_eq!(reached(3, READING), 4);
        // Begun in a library with a paper open: what is already done is not asked for again.
        assert_eq!(reached(1, READING), 4);
        // The steps that show a place wait for 下一步, whatever the window shows.
        assert_eq!(reached(4, READING), 4);
        assert_eq!(reached(5, NOTHING), 5);
        assert_eq!(reached(9, READING), 6);
    }

    #[test]
    fn the_step_on_show_is_the_earliest_one_not_done() {
        assert_eq!(shown(1, NOTHING), Step::Library);
        assert_eq!(shown(2, EMPTY), Step::Papers);
        assert_eq!(shown(3, FILLED), Step::Open);
        assert_eq!(shown(4, READING), Step::Read);
        // The paper was closed before 下一步: back to opening one. The library was left: back to opening it.
        assert_eq!(shown(4, FILLED), Step::Open);
        assert_eq!(shown(4, NOTHING), Step::Library);
        assert_eq!(shown(6, NOTHING), Step::Library);
        // The sidebar's steps need no paper, open or at all.
        assert_eq!(shown(5, EMPTY), Step::Ask);
        assert_eq!(shown(6, FILLED), Step::Path);
        // A step is never shown ahead of the one reached.
        assert_eq!(shown(2, READING), Step::Papers);
    }

    #[test]
    fn the_steps_count_from_one() {
        assert_eq!(Step::Library.number(), 1);
        assert_eq!(Step::Path.number(), Step::ALL.len());
        for step in Step::ALL { assert_eq!(Step::of(step.number()), step) }
        assert_eq!(Step::of(0), Step::Library);
        assert_eq!(Step::of(40), Step::Path);
        assert!(Step::Open.done_by_doing() && !Step::Read.done_by_doing());
    }

    #[test]
    fn the_card_stands_beside_what_it_points_at() {
        let (card, window) = ((280., 160.), (1360., 860.));
        let place = |target: Rect, card: (f32, f32), window: (f32, f32)| super::place(target, card, window, false);
        let over = |at: (f32, f32), target: Rect| at.0 < target.x + target.width && at.0 + card.0 > target.x && at.1 < target.y + target.height && at.1 + card.1 > target.y;
        // A row of the sidebar: to its right, level with it.
        let row = Rect { x: 10., y: 300., width: 200., height: 30. };
        assert_eq!(place(row, card, window), (224., 300.));
        // One near the window's bottom: still to its right, moved up to stay in the window.
        let low = Rect { x: 10., y: 800., width: 200., height: 30. };
        assert_eq!(place(low, card, window), (224., 688.));
        // A row of the table, as wide as the page: under it, at its right end.
        let wide = Rect { x: 220., y: 120., width: 1140., height: 44. };
        assert_eq!(place(wide, card, window), (1068., 178.));
        // The tabs of the panel at the window's right edge: under them, from their left.
        let tabs = Rect { x: 928., y: 96., width: 150., height: 36. };
        assert_eq!(place(tabs, card, window), (928., 146.));
        // Something that leaves room only on its left, then only above.
        let tall = Rect { x: 1000., y: 0., width: 360., height: 860. };
        assert_eq!(place(tall, card, window), (706., 12.));
        let floor = Rect { x: 0., y: 400., width: 1360., height: 460. };
        assert_eq!(place(floor, card, window), (1068., 226.));
        for target in [row, low, wide, tabs, tall, floor] { assert!(!over(place(target, card, window), target), "{target:?}") }
        // A library's card on the home page: under it, the next card left in sight; with no room under it, beside it after all.
        let library = Rect { x: 76., y: 144., width: 296., height: 176. };
        assert_eq!(super::place(library, card, window, true), (76., 334.));
        assert_eq!(super::place(Rect { y: 600., ..library }, card, window, true), (386., 600.));
        assert!(Step::Library.card_under() && !Step::Ask.card_under());
        // Nothing but the thing itself in the window: the corner.
        let all = Rect { x: 0., y: 0., width: 1360., height: 860. };
        assert_eq!(place(all, card, window), docked(card, window));
        assert_eq!(docked(card, window), (24., 676.));
    }
}
