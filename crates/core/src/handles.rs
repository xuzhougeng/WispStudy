//! A rectangle placed on a PDF page once it is chosen (a box, a piece marked to be cut out, the frame of a
//! figure being kept, a text box): eight handles size it, its edge moves it, as a shape chosen in a slide
//! editor. Everything is in the page's points from the top left, as the marks are kept, so the zoom does not
//! change it; reaches are handed in as points (the window's pixels over the page's scale).

/// Where a rectangle is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grab {
    /// By its edge: the whole of it moves.
    Body,
    /// By one of its eight handles: the edge it moves across (-1 the left, 1 the right, 0 neither) and down
    /// (-1 the top, 1 the bottom, 0 neither). A corner moves two edges, a side's middle one.
    Handle(i8, i8),
}

/// The eight handles, clockwise from the top left corner.
pub const HANDLES: [(i8, i8); 8] = [(-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)];

/// The smallest side a rectangle is sized down to, in points: a handle dragged past the edge across stops
/// this far from it, so a box never turns inside out or shrinks to nothing.
pub const MIN_SIDE: f32 = 8.;

/// `[left, top, right, bottom]`, whichever corners it was given.
pub fn normal([x0, y0, x1, y1]: [f32; 4]) -> [f32; 4] {
    [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
}

/// Where a handle stands on a rectangle.
pub fn handle_point(rect: [f32; 4], (hx, hy): (i8, i8)) -> (f32, f32) {
    let [x0, y0, x1, y1] = normal(rect);
    let at = |side: i8, low: f32, high: f32| match side { -1 => low, 1 => high, _ => (low + high) / 2. };
    (at(hx, x0, x1), at(hy, y0, y1))
}

/// The handles a rectangle shows: its corners always, the middle of a side only where the side is long
/// enough for that handle to stand apart from the corners (`grip` is a handle's reach), so a small box
/// keeps handles that can be told apart.
pub fn handles(rect: [f32; 4], grip: f32) -> impl Iterator<Item = (i8, i8)> {
    let [x0, y0, x1, y1] = normal(rect);
    let (wide, tall) = (x1 - x0 > grip * 4., y1 - y0 > grip * 4.);
    HANDLES.into_iter().filter(move |&(hx, hy)| (hx != 0 || wide) && (hy != 0 || tall))
}

/// The pointer is on a rectangle's edge: within `reach` of its outline, inside or out. What it frames is
/// not the rectangle's: a press there is for the page's text.
pub fn on_edge(rect: [f32; 4], (x, y): (f32, f32), reach: f32) -> bool {
    let [x0, y0, x1, y1] = normal(rect);
    let within = |grow: f32| x >= x0 - grow && x <= x1 + grow && y >= y0 - grow && y <= y1 + grow;
    within(reach) && !within(-reach)
}

/// What of a chosen rectangle is under a point: a handle within `grip` of it (the nearest, where two are
/// near), else its edge within `reach`; nothing inside it, beyond the edge, so its text can still be selected.
pub fn grab_at(rect: [f32; 4], (x, y): (f32, f32), grip: f32, reach: f32) -> Option<Grab> {
    let near = handles(rect, grip)
        .map(|handle| {
            let (hx, hy) = handle_point(rect, handle);
            (handle, (x - hx).abs().max((y - hy).abs()))
        })
        .filter(|(_, distance)| *distance <= grip)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some(((hx, hy), _)) = near { return Some(Grab::Handle(hx, hy)) }
    on_edge(rect, (x, y), reach).then_some(Grab::Body)
}

/// A rectangle held by `grab` at `from` and the pointer now at `to`, on a page of `size` points. Moved, it
/// keeps its size and stays on the page. Sized, the edges the handle names follow the pointer and the others
/// stay where they are; an edge stops at the page's border and `min` short of the edge across from it.
pub fn drag(rect: [f32; 4], grab: Grab, from: (f32, f32), to: (f32, f32), (w, h): (f32, f32), min: f32) -> [f32; 4] {
    let [x0, y0, x1, y1] = normal(rect);
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    match grab {
        Grab::Body => {
            // Not past the page's border, whichever side the rectangle comes to; one larger than the page stays at its left and top.
            let dx = dx.min(w - x1).max(-x0);
            let dy = dy.min(h - y1).max(-y0);
            [x0 + dx, y0 + dy, x1 + dx, y1 + dy]
        }
        Grab::Handle(hx, hy) => {
            let (mut r, step) = ([x0, y0, x1, y1], [dx, dy]);
            for (axis, side, limit) in [(0, hx, w), (1, hy, h)] {
                let (low, high) = (r[axis], r[axis + 2]);
                match side {
                    -1 => r[axis] = (low + step[axis]).min(high - min).max(0_f32.min(high - min)),
                    1 => r[axis + 2] = (high + step[axis]).max(low + min).min(limit.max(low + min)),
                    _ => {}
                }
            }
            r
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: (f32, f32) = (600., 800.);
    const BOX: [f32; 4] = [100., 200., 300., 320.];

    #[test]
    fn a_corner_sizes_both_ways_and_a_side_one() {
        // The bottom right corner, out and down: wider and taller, the top left stays.
        assert_eq!(drag(BOX, Grab::Handle(1, 1), (300., 320.), (350., 400.), PAGE, MIN_SIDE), [100., 200., 350., 400.]);
        // In and up: smaller.
        assert_eq!(drag(BOX, Grab::Handle(1, 1), (300., 320.), (250., 300.), PAGE, MIN_SIDE), [100., 200., 250., 300.]);
        // The top left corner moves the left and the top.
        assert_eq!(drag(BOX, Grab::Handle(-1, -1), (100., 200.), (80., 150.), PAGE, MIN_SIDE), [80., 150., 300., 320.]);
        // The right side's middle: only wider, however the pointer goes up or down.
        assert_eq!(drag(BOX, Grab::Handle(1, 0), (300., 260.), (360., 500.), PAGE, MIN_SIDE), [100., 200., 360., 320.]);
        // The bottom's middle: only taller.
        assert_eq!(drag(BOX, Grab::Handle(0, 1), (200., 320.), (10., 380.), PAGE, MIN_SIDE), [100., 200., 300., 380.]);
        // The left side's middle, to the right: narrower from the left.
        assert_eq!(drag(BOX, Grab::Handle(-1, 0), (100., 260.), (150., 260.), PAGE, MIN_SIDE), [150., 200., 300., 320.]);
    }

    #[test]
    fn a_handle_dragged_past_the_edge_across_stops_short_of_it() {
        // The right edge dragged far to the left of the left one: the box keeps the least width, not a negative one.
        assert_eq!(drag(BOX, Grab::Handle(1, 0), (300., 260.), (20., 260.), PAGE, MIN_SIDE), [100., 200., 108., 320.]);
        // The top dragged below the bottom.
        assert_eq!(drag(BOX, Grab::Handle(0, -1), (200., 200.), (200., 700.), PAGE, MIN_SIDE), [100., 312., 300., 320.]);
        // A corner past both.
        let r = drag(BOX, Grab::Handle(-1, -1), (100., 200.), (500., 700.), PAGE, MIN_SIDE);
        assert_eq!(r, [292., 312., 300., 320.]);
        assert!(r[2] - r[0] >= MIN_SIDE && r[3] - r[1] >= MIN_SIDE);
    }

    #[test]
    fn nothing_leaves_the_page() {
        // Sized past the page's border: the edge stops at it.
        assert_eq!(drag(BOX, Grab::Handle(1, 1), (300., 320.), (900., 1200.), PAGE, MIN_SIDE), [100., 200., 600., 800.]);
        assert_eq!(drag(BOX, Grab::Handle(-1, -1), (100., 200.), (-50., -50.), PAGE, MIN_SIDE), [0., 0., 300., 320.]);
        // Moved past it: the whole box stops at it, the same size.
        assert_eq!(drag(BOX, Grab::Body, (100., 200.), (900., -400.), PAGE, MIN_SIDE), [400., 0., 600., 120.]);
        assert_eq!(drag(BOX, Grab::Body, (100., 200.), (130., 210.), PAGE, MIN_SIDE), [130., 210., 330., 330.]);
        // A sticky note is its spot: moved, it stays on the page.
        assert_eq!(drag([590., 10., 590., 10.], Grab::Body, (590., 10.), (700., 5.), PAGE, MIN_SIDE), [600., 5., 600., 5.]);
    }

    #[test]
    fn a_box_already_smaller_than_the_least_is_not_thrown_about() {
        // Kept by an older build at 4 points wide: its other handles still work, and dragging out makes it larger.
        let thin = [100., 100., 104., 200.];
        assert_eq!(drag(thin, Grab::Handle(0, 1), (102., 200.), (102., 250.), PAGE, MIN_SIDE), [100., 100., 104., 250.]);
        assert_eq!(drag(thin, Grab::Handle(1, 0), (104., 150.), (130., 150.), PAGE, MIN_SIDE), [100., 100., 130., 200.]);
        // Drawn back the wrong way round, it is read the right way round.
        assert_eq!(normal([300., 320., 100., 200.]), BOX);
    }

    #[test]
    fn a_chosen_box_is_held_by_its_handles_and_its_edge_only() {
        let (grip, reach) = (4., 3.);
        assert_eq!(grab_at(BOX, (101., 199.), grip, reach), Some(Grab::Handle(-1, -1)));
        assert_eq!(grab_at(BOX, (300., 320.), grip, reach), Some(Grab::Handle(1, 1)));
        assert_eq!(grab_at(BOX, (200., 322.), grip, reach), Some(Grab::Handle(0, 1)));
        assert_eq!(grab_at(BOX, (298., 260.), grip, reach), Some(Grab::Handle(1, 0)));
        // The edge between handles moves it.
        assert_eq!(grab_at(BOX, (150., 201.), grip, reach), Some(Grab::Body));
        assert_eq!(grab_at(BOX, (99., 290.), grip, reach), Some(Grab::Body));
        // Inside, away from the edge, the page's text is still for selecting; outside is nothing either.
        assert_eq!(grab_at(BOX, (200., 260.), grip, reach), None);
        assert_eq!(grab_at(BOX, (90., 260.), grip, reach), None);
        // A small box has only its corners, the nearest one taken.
        let small = [100., 100., 110., 110.];
        assert_eq!(handles(small, grip).count(), 4);
        assert_eq!(handles(BOX, grip).count(), 8);
        assert_eq!(grab_at(small, (108., 109.), grip, reach), Some(Grab::Handle(1, 1)));
        assert_eq!(grab_at(small, (102., 101.), grip, reach), Some(Grab::Handle(-1, -1)));
    }

    #[test]
    fn an_edge_is_near_the_outline_on_either_side() {
        assert!(on_edge(BOX, (100., 260.), 3.));
        assert!(on_edge(BOX, (97.5, 260.), 3.));
        assert!(on_edge(BOX, (102.5, 260.), 3.));
        assert!(!on_edge(BOX, (110., 260.), 3.));
        assert!(!on_edge(BOX, (90., 260.), 3.));
        assert_eq!(handle_point(BOX, (0, 1)), (200., 320.));
        assert_eq!(handle_point(BOX, (-1, 0)), (100., 260.));
    }
}
