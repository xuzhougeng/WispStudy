//! A vertical scrollbar beside a `uniform_list`: it shows how far down the list is and how much of it is
//! in view, and its thumb is dragged (or the track pressed) to go there. It reads the list's scroll handle
//! while it is drawn, after the list, so it stands where the list is this frame. Placed as the last child
//! of a `relative()` parent that holds the list, it covers the list's right edge; the wheel over it still
//! scrolls the list.

use crate::theme::Theme;
use gpui::*;
use std::cell::RefCell;
use std::rc::Rc;

/// How wide the strip that hears the pointer is, how far the track stands in from its ends, how wide
/// the thumb is at rest and under the pointer, and the shortest it gets.
const STRIP: f32 = 12.;
const INSET: f32 = 3.;
const THUMB_W: f32 = 6.;
const THUMB_W_OVER: f32 = 8.;
const THUMB_MIN: f32 = 28.;

/// The thumb on a track `track` long, of a list `view` tall that scrolls `max` down and is `offset` down:
/// its length, and how far down the track it stands.
fn thumb(track: f32, view: f32, max: f32, offset: f32) -> (f32, f32) {
    let len = (track * view / (view + max).max(1.)).max(THUMB_MIN).min(track);
    (len, (track - len) * (offset / max.max(1.)).clamp(0., 1.))
}

/// How far down the list is with the thumb's top `top` down the track: the inverse of `thumb`.
fn offset_at(track: f32, len: f32, max: f32, top: f32) -> f32 {
    (top / (track - len).max(1.) * max).clamp(0., max)
}

pub struct Scrollbar {
    id: ElementId,
    handle: UniformListScrollHandle,
}

pub fn scrollbar(id: impl Into<ElementId>, handle: UniformListScrollHandle) -> Scrollbar {
    Scrollbar { id: id.into(), handle }
}

/// Kept between frames: how far below the thumb's top it is held while dragged, and whether it was
/// drawn as under the pointer, to know when that changes.
#[derive(Default)]
struct Held {
    grab: Option<f32>,
    over: bool,
}

pub struct Bar {
    hitbox: Hitbox,
    track: f32,
    len: f32,
    top: f32,
    max: f32,
}

impl IntoElement for Scrollbar {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for Scrollbar {
    type RequestLayoutState = ();
    type PrepaintState = Option<Bar>;

    fn id(&self) -> Option<ElementId> { Some(self.id.clone()) }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> { None }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style { position: Position::Absolute, ..Default::default() };
        style.inset = Edges { top: px(0.).into(), right: px(0.).into(), bottom: px(0.).into(), left: Length::Auto };
        style.size.width = px(STRIP).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, _: &mut App) -> Option<Bar> {
        let base = self.handle.0.borrow().base_handle.clone();
        let (view, max, offset) = (f32::from(base.bounds().size.height), f32::from(base.max_offset().height), -f32::from(base.offset().y));
        let track = f32::from(bounds.size.height) - 2. * INSET;
        // A list that all fits has no bar, and nothing over its edge.
        if max < 1. || track < THUMB_MIN { return None }
        let (len, top) = thumb(track, view, max, offset);
        Some(Bar { hitbox: window.insert_hitbox(bounds, HitboxBehavior::BlockMouseExceptScroll), track, len, top, max })
    }

    fn paint(&mut self, id: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), bar: &mut Option<Bar>, window: &mut Window, _: &mut App) {
        let held = window.with_element_state(id.unwrap(), |state: Option<Rc<RefCell<Held>>>, _| {
            let state = state.unwrap_or_default();
            (state.clone(), state)
        });
        let Some(Bar { hitbox, track, len, top, max }) = bar.take() else {
            *held.borrow_mut() = Held::default();
            return;
        };
        let over = hitbox.is_hovered(window) || held.borrow().grab.is_some();
        held.borrow_mut().over = over;
        let width = if over { THUMB_W_OVER } else { THUMB_W };
        let thumb_bounds = Bounds::new(point(bounds.right() - px(INSET + width - 1.), bounds.top() + px(INSET + top)), size(px(width), px(len)));
        window.paint_quad(fill(thumb_bounds, Theme::text_tertiary().opacity(if over { 0.85 } else { 0.4 })).corner_radii(px(width / 2.)));
        window.set_cursor_style(CursorStyle::Arrow, &hitbox);

        let view = window.current_view();
        let handle = self.handle.clone();
        let origin = bounds.top() + px(INSET);
        // The list goes to where the thumb's top is, with the thumb held `grab` below it at `y`.
        let go_down = Rc::new(move |y: Pixels, grab: f32| {
            let base = handle.0.borrow().base_handle.clone();
            let x = base.offset().x;
            base.set_offset(point(x, px(-offset_at(track, len, max, f32::from(y - origin) - grab))));
        });
        let go_moved = go_down.clone();
        let (down, moved, up) = (held.clone(), held.clone(), held);
        let pressed = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if !phase.bubble() || event.button != MouseButton::Left || !pressed.is_hovered(window) { return }
            // Pressed on the thumb, it is held where it was taken; beside it, its middle comes under the pointer.
            let at = f32::from(event.position.y - origin);
            let grab = if at >= top && at <= top + len { at - top } else { len / 2. };
            down.borrow_mut().grab = Some(grab);
            go_down(event.position.y, grab);
            cx.stop_propagation();
            cx.notify(view);
        });
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if !phase.bubble() { return }
            let grab = moved.borrow().grab;
            match grab {
                Some(grab) if event.pressed_button == Some(MouseButton::Left) => {
                    go_moved(event.position.y, grab);
                    cx.notify(view);
                }
                // Let go somewhere the release was not heard.
                Some(_) => {
                    moved.borrow_mut().grab = None;
                    cx.notify(view);
                }
                None => if hitbox.is_hovered(window) != moved.borrow().over { cx.notify(view) },
            }
        });
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if !phase.bubble() || event.button != MouseButton::Left { return }
            if up.borrow_mut().grab.take().is_some() { cx.notify(view) }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{THUMB_MIN, offset_at, thumb};

    #[test]
    fn the_thumb_stands_for_the_part_in_view() {
        // A list twice as tall as its view: the thumb is half the track, and runs its whole length.
        assert_eq!(thumb(400., 400., 400., 0.), (200., 0.));
        assert_eq!(thumb(400., 400., 400., 200.), (200., 100.));
        assert_eq!(thumb(400., 400., 400., 400.), (200., 200.));
        // Thousands of rows: the thumb stays long enough to take hold of, inside its track.
        assert_eq!(thumb(400., 400., 400_000., 400_000.), (THUMB_MIN, 400. - THUMB_MIN));
        // Dragging the thumb back to where it stands gives the same offset.
        let (len, top) = thumb(400., 400., 1200., 300.);
        assert!((offset_at(400., len, 1200., top) - 300.).abs() < 0.01);
        assert_eq!(offset_at(400., len, 1200., -50.), 0.);
        assert_eq!(offset_at(400., len, 1200., 1000.), 1200.);
    }
}
