//! Text of the window that can be selected with the pointer and copied: a
//! paper's title, authors and abstract, a question asked. (An answer has its
//! own selection over its Markdown, `markdown.rs`; a text field its own.)
//! One text at a time: drag for a stretch, click twice for a word, three
//! times for the whole, ⌘C / Ctrl+C or a right click to copy.

use crate::app::{Cmd, MenuKind, Workspace};
use crate::theme::Theme;
use gpui::{prelude::FluentBuilder, *};
use std::ops::Range;

/// What is selected: in which text (a key and the text itself, so that
/// another paper's abstract under the same key is not marked), from where
/// the button went down to where the pointer is.
pub struct Picked {
    pub key: SharedString,
    pub text: SharedString,
    anchor: usize,
    head: usize,
    /// The button is still down.
    pub held: bool,
    /// The text as it was laid out when the button went down: where the pointer is in it.
    layout: TextLayout,
    /// A link waits for release; moving the pointer to select text cancels it.
    link: Option<(Follow, Point<Pixels>)>,
}

/// Where a click on a text leads: an address, or a command of the window (a mention in a question, `mention.rs`).
#[derive(Clone)]
enum Follow {
    Url(String),
    Run(Cmd),
}

impl Picked {
    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub fn chosen(&self) -> &str {
        self.text.get(self.range()).unwrap_or("")
    }
}

/// The byte of `text` under a point of the window; past an end, that end.
fn index_at(layout: &TextLayout, text: &str, position: Point<Pixels>) -> usize {
    // A text not laid out in this frame has no place to be found in.
    let index = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| layout.index_for_position(position).unwrap_or_else(|index| index))).unwrap_or(0);
    floor(text, index)
}

/// What a selectable text draws over its words: the stretches marked in the accent, the selection's ground over all.
/// Cut at every edge of both, since the runs a text is drawn in may not overlap.
fn highlights(selected: Option<Range<usize>>, marks: impl Iterator<Item = Range<usize>>) -> Vec<(Range<usize>, HighlightStyle)> {
    let marks: Vec<Range<usize>> = marks.filter(|range| !range.is_empty()).collect();
    let mut edges: Vec<usize> = marks.iter().flat_map(|range| [range.start, range.end]).chain(selected.iter().flat_map(|range| [range.start, range.end])).collect();
    edges.sort();
    edges.dedup();
    edges
        .windows(2)
        .filter_map(|pair| {
            let piece = pair[0]..pair[1];
            let marked = marks.iter().any(|range| range.start <= piece.start && piece.end <= range.end);
            let picked = selected.as_ref().is_some_and(|range| range.start <= piece.start && piece.end <= range.end);
            (marked || picked).then(|| (piece, HighlightStyle { color: marked.then(Theme::accent), font_weight: marked.then_some(FontWeight::MEDIUM), background_color: picked.then(Theme::selection), ..Default::default() }))
        })
        .collect()
}

fn floor(text: &str, index: usize) -> usize {
    (0..=index.min(text.len())).rev().find(|at| text.is_char_boundary(*at)).unwrap_or(0)
}

/// The word a double click picks: the same in every text read, and on a PDF page (`pdftext::word_marks`).
pub use wisp_core::selection::word_at;

impl Workspace {
    /// `text` drawn so that it can be selected. `key` tells this text from the others on show.
    pub fn selectable(&self, key: impl Into<SharedString>, text: impl Into<SharedString>, cx: &mut Context<Self>) -> Div {
        self.selectable_text(key.into(), text.into(), None, vec![], cx)
    }

    /// `selectable` with stretches drawn in the accent (the things a question names with @), each leading where its
    /// command goes when it is clicked, not dragged.
    pub fn selectable_marked(&self, key: impl Into<SharedString>, text: impl Into<SharedString>, marks: Vec<(Range<usize>, Option<Cmd>)>, cx: &mut Context<Self>) -> Div {
        self.selectable_text(key.into(), text.into(), None, marks, cx)
    }

    /// A single click opens the link; dragging and right clicking still select and copy.
    pub fn selectable_link(&self, key: impl Into<SharedString>, text: impl Into<SharedString>, url: String, cx: &mut Context<Self>) -> Div {
        self.selectable_text(key.into(), text.into(), Some(url), vec![], cx).cursor_pointer().text_color(Theme::accent()).hover(|s| s.underline())
    }

    fn selectable_text(&self, key: SharedString, text: SharedString, url: Option<String>, marks: Vec<(Range<usize>, Option<Cmd>)>, cx: &mut Context<Self>) -> Div {
        let selected = self.picked.as_ref().filter(|picked| picked.key == key && picked.text == text).map(Picked::range).filter(|range| !range.is_empty());
        let styled = StyledText::new(text.clone()).with_highlights(highlights(selected, marks.iter().map(|(range, _)| range.clone())));
        let layout = styled.layout().clone();
        // Over a mention that leads somewhere the pointer is a hand, as over a link.
        let clickable: Vec<Range<usize>> = marks.iter().filter(|(_, go)| go.is_some()).map(|(range, _)| range.clone()).collect();
        let over = self.mark_over.as_ref() == Some(&key);
        div()
            .when(over, |d| d.cursor_pointer())
            .when(!over, |d| d.cursor_text())
            .when(!clickable.is_empty(), |d| {
                let (key, layout) = (key.clone(), layout.clone());
                d.on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                    if event.pressed_button.is_some() { return }
                    let at = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| layout.index_for_position(event.position).ok())).ok().flatten();
                    let now = at.filter(|at| clickable.iter().any(|range| range.start <= *at && *at < range.end)).map(|_| key.clone());
                    if this.mark_over != now {
                        this.mark_over = now;
                        cx.notify();
                    }
                }))
            })
            .on_mouse_down(MouseButton::Left, cx.listener({
                let (key, text, layout) = (key.clone(), text.clone(), layout.clone());
                move |this, event: &MouseDownEvent, window, cx| {
                    let at = index_at(&layout, &text, event.position);
                    let (range, held) = match event.click_count {
                        0 | 1 => (at..at, true),
                        2 => (word_at(&text, at), false),
                        _ => (0..text.len(), false),
                    };
                    // One selection in the window: an answer's goes. The keyboard goes to the window, so that copying finds this one and not a text box.
                    if let Some(lib) = this.lib.as_mut() { lib.unpick(); }
                    window.focus(&this.focus);
                    // A click on a mention goes where it leads; a drag from it selects, as from a link.
                    let run = marks.iter().find(|(range, go)| go.is_some() && range.start <= at && at <= range.end).and_then(|(_, go)| go.clone()).map(Follow::Run);
                    let link = run.or(url.clone().map(Follow::Url)).filter(|_| held).map(|follow| (follow, event.position));
                    this.picked = Some(Picked { key: key.clone(), text: text.clone(), anchor: range.start, head: range.end, held, layout: layout.clone(), link });
                    cx.notify();
                }
            }))
            // A click is over before the layer that follows the pointer is drawn (home.rs): the text hears the end of it itself.
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event, window, cx| this.pick_release(event, window, cx)))
            .on_mouse_down(MouseButton::Right, self.on_context(MenuKind::Text(text.to_string()), cx))
            .child(styled)
    }

    /// The pointer moved with the button held, anywhere in the window: the selection reaches to it.
    pub fn pick_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) { return self.pick_end(cx) }
        let Some(picked) = self.picked.as_mut().filter(|picked| picked.held) else { return };
        if picked.link.as_ref().is_some_and(|(_, start)| (event.position - *start).magnitude() > 4.) { picked.link = None }
        let head = index_at(&picked.layout, &picked.text, event.position);
        if head != picked.head {
            picked.head = head;
            cx.notify();
        }
    }

    /// Both the text and the selection overlay finish a click here.
    pub fn pick_release(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let follow = self.picked.as_ref().filter(|picked| picked.held && picked.range().is_empty() && event.click_count == 1)
            .and_then(|picked| picked.link.as_ref()).filter(|(_, start)| (event.position - *start).magnitude() <= 4.).map(|(follow, _)| follow.clone());
        self.pick_end(cx);
        match follow {
            Some(Follow::Url(url)) => cx.open_url(&url),
            Some(Follow::Run(cmd)) => self.run(cmd, window, cx),
            None => {}
        }
    }

    /// Finish or cancel selection without following a link.
    pub fn pick_end(&mut self, cx: &mut Context<Self>) {
        let Some(picked) = self.picked.as_mut().filter(|picked| picked.held) else { return };
        picked.held = false;
        if picked.range().is_empty() { self.picked = None }
        cx.notify();
    }
}
