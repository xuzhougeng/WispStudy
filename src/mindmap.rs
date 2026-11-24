//! A paper's mind map (思维导图) as a column of the reader: the tree of
//! `wisp_core::mindmap` drawn with its root at the left, made by hand (the
//! keyboard on the node chosen, a passage sent from the pages) or drafted by
//! a model and then the reader's to change. The view holds the map and what
//! is chosen in it; whoever made it keeps the map (`MapEvent::Changed`),
//! shows a node's passage in the paper (`Goto`) and asks the model (`Draft`).
//!
//! The map is a sheet the reader moves about and draws larger or smaller: the wheel moves it (with ⌘ / Ctrl,
//! or a pinch, it zooms about the pointer), its ground is pulled with the pointer, and a node held and moved is
//! dropped onto another, under it or, at its top or bottom edge, before or after it.

use crate::i18n::t;
use crate::input::{InputEvent, TextInput};
use crate::shortcuts::{Dismiss, MoveDown, MoveLeft, MoveRight, MoveUp, OpenSelected, TickSuggestion, TrashSelected, ZoomFit, ZoomIn, ZoomOut};
use crate::theme::{text_px, Theme};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::{cell::Cell, collections::HashSet, rc::Rc};
use wisp_core::mindmap::{self, Anchor, Layout, MindMap, Node, Place, Step};

/// The widest a node's words run before they wrap, and the room around them in a node.
const NODE_WIDE: f32 = 220.;
const NODE_PAD: (f32, f32) = (10., 5.);
/// Between a node and its children, and between two nodes one under the other.
const GAP: (f32, f32) = (36., 10.);
/// Around the whole map, and above it for the column's label and the map's own tools.
const EDGE: f32 = 24.;
const HEAD: f32 = 34.;
/// A node's page, after its words.
const PAGE_CHIP: f32 = 34.;
/// How many earlier states of the map can be gone back to.
const UNDO: usize = 50;
/// How much smaller and larger the map is drawn, and the step of a click or a key.
const ZOOM: (f32, f32) = (0.3, 2.5);
const ZOOM_STEP: f32 = 1.2;
/// How far the pointer goes with something held before it is a drag and not a click.
const DRAG: f32 = 4.;
/// How much of the map stays in view however far it is pulled away.
const KEEP: f32 = 56.;

/// What the pointer holds.
#[derive(Clone, Copy)]
enum Grab {
    /// The map's ground (or its root), taken where it was pressed: the map follows the pointer.
    Ground { from: Point<Pixels>, pan: (f32, f32), moving: bool },
    /// A node; once the pointer has gone `DRAG` it is carried, and `drop` says where it would go if let go now.
    Node { id: u64, from: Point<Pixels>, at: Point<Pixels>, moving: bool, drop: Option<(u64, Place)> },
}

impl Grab {
    fn moving(&self) -> bool {
        match self { Grab::Ground { moving, .. } | Grab::Node { moving, .. } => *moving }
    }
}

pub enum MapEvent {
    /// The map was changed by the reader: it is to be kept.
    Changed,
    /// A node's passage is to be shown in the paper.
    Goto(Anchor),
    /// A draft of the map is asked of the model.
    Draft,
}

pub struct MindMapView {
    map: MindMap,
    chosen: Option<u64>,
    /// The node being written in.
    editing: Option<u64>,
    input: Entity<TextInput>,
    focus: FocusHandle,
    /// How large the map is drawn, 1 being its own size.
    zoom: f32,
    /// Where the sheet's top left stands in the view: the map is moved by moving it.
    pan: (f32, f32),
    /// The view's bounds in the window, as last drawn.
    view: Rc<Cell<Bounds<Pixels>>>,
    /// The map as last drawn (at `zoom`), for what a drag is over.
    drawn: Layout,
    grab: Option<Grab>,
    /// The whole map is to be fitted into the view when it is next drawn.
    fit: bool,
    /// The node chosen is to be brought into view when the map is next drawn.
    reveal: bool,
    /// The map is to take the keyboard back when it is next drawn: the field that had it is gone.
    refocus: bool,
    /// A model is drafting the map.
    drafting: bool,
    /// Why the last draft did not come.
    failed: Option<String>,
    /// The map has nodes of the reader's: a draft would take their place, so it is asked first.
    asking: bool,
    undone: Vec<MindMap>,
    redone: Vec<MindMap>,
    blur: Option<Subscription>,
}

impl EventEmitter<MapEvent> for MindMapView {}

impl Focusable for MindMapView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl MindMapView {
    pub fn new(map: MindMap, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("写下要点", cx));
        cx.subscribe(&input, |this: &mut Self, _, event: &InputEvent, cx| match event {
            // Done by the keyboard, the keyboard stays with the map: the next key is for the node written.
            InputEvent::Submit => { this.refocus = true; this.finish(true, cx) }
            InputEvent::Cancel => { this.refocus = true; this.finish(false, cx) }
            _ => {}
        }).detach();
        let chosen = Some(map.root.id);
        MindMapView { map, chosen, editing: None, input, focus: cx.focus_handle(), zoom: 1., pan: (0., 0.), view: Rc::default(), drawn: Layout::default(), grab: None, fit: false, reveal: false, refocus: false, drafting: false, failed: None, asking: false, undone: vec![], redone: vec![], blur: None }
    }

    pub fn map(&self) -> &MindMap {
        &self.map
    }

    /// The map as another viewer of the paper left it: what is chosen here stays chosen while it is still there.
    pub fn set_map(&mut self, map: MindMap, cx: &mut Context<Self>) {
        if map == self.map { return }
        self.map = map;
        self.editing = None;
        if self.chosen.is_some_and(|id| self.map.node(id).is_none()) { self.chosen = Some(self.map.root.id) }
        cx.notify();
    }

    pub fn drafting(&self) -> bool {
        self.drafting
    }

    /// A model is at work on a draft, or has left off.
    pub fn set_drafting(&mut self, drafting: bool, cx: &mut Context<Self>) {
        (self.drafting, self.asking) = (drafting, false);
        if drafting { self.failed = None }
        cx.notify();
    }

    /// The draft came: it is the map now, and the one before it is a step of Undo away.
    pub fn take_draft(&mut self, map: MindMap, cx: &mut Context<Self>) {
        self.drafting = false;
        self.change(cx, |this| {
            this.map = map;
            Some(this.map.root.id)
        });
    }

    /// The draft did not come, and why.
    pub fn draft_failed(&mut self, error: String, cx: &mut Context<Self>) {
        (self.drafting, self.failed) = (false, Some(error));
        cx.notify();
    }

    /// A passage of the paper becomes a node under the one chosen (the root without one), pointing back at it.
    pub fn add_passage(&mut self, text: &str, anchor: Anchor, cx: &mut Context<Self>) {
        let words: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if words.is_empty() { return }
        let under = self.chosen.filter(|id| self.map.node(*id).is_some()).unwrap_or(self.map.root.id);
        self.change(cx, |this| this.map.add_child(under, &words, Some(anchor)));
    }

    /// A step of the keyboard without the keyboard, for a scripted window (`pdfmap:` of `WISP_STUDY_DEV`):
    /// `child:<words>` / `sibling:<words>` of the node chosen, `remove`, `fold`, `indent`, `outdent`, `undo`,
    /// `redo`, `up` / `down` / `in` / `out` to choose another, `goto` for the passage of the one chosen,
    /// `zoom:in` / `zoom:out` / `zoom:fit` / `zoom:<percent>`, and `move:<id>;before|under|after` (the node chosen
    /// dropped on another, as a drag does).
    pub fn script(&mut self, what: &str, cx: &mut Context<Self>) {
        let chosen = self.chosen.unwrap_or(self.map.root.id);
        let (what, words) = what.split_once(':').unwrap_or((what, ""));
        match what {
            "child" => { self.change(cx, |this| this.map.add_child(chosen, words, None)); }
            "sibling" => { self.change(cx, |this| this.map.add_after(chosen, words)); }
            "remove" => self.remove(cx),
            "fold" => { self.change(cx, |this| this.map.fold(chosen).then_some(chosen)); }
            "indent" => { self.change(cx, |this| this.map.indent(chosen).then_some(chosen)); }
            "outdent" => { self.change(cx, |this| this.map.outdent(chosen).then_some(chosen)); }
            "undo" => self.undo(true, cx),
            "redo" => self.undo(false, cx),
            "up" => self.step(Step::Up, cx),
            "down" => self.step(Step::Down, cx),
            "in" => self.step(Step::In, cx),
            "out" => self.step(Step::Out, cx),
            "zoom" => match words {
                "in" => self.zoom_to(self.zoom * ZOOM_STEP, None, cx),
                "out" => self.zoom_to(self.zoom / ZOOM_STEP, None, cx),
                "fit" => { self.fit = true; cx.notify() }
                percent => if let Ok(percent) = percent.parse::<f32>() { self.zoom_to(percent / 100., None, cx) },
            },
            "move" => {
                let (target, place) = words.split_once(';').unwrap_or((words, "under"));
                let place = match place { "before" => Place::Before, "after" => Place::After, _ => Place::Under };
                if let Ok(target) = target.parse::<u64>() { self.change(cx, |this| this.map.move_node(chosen, target, place).then_some(chosen)); }
            }
            "goto" => if let Some(anchor) = self.map.node(chosen).and_then(|node| node.anchor.clone()) { cx.emit(MapEvent::Goto(anchor)) },
            _ => {}
        }
    }

    /// Change the map: what it was is kept for Undo, `change` says which node is chosen after it (`None` when
    /// nothing came of it), and whoever keeps the map hears of it.
    fn change(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Self) -> Option<u64>) -> bool {
        let before = self.map.clone();
        let Some(chosen) = change(self) else { return false };
        (self.chosen, self.reveal) = (Some(chosen), true);
        if self.map != before {
            self.undone.push(before);
            if self.undone.len() > UNDO { self.undone.remove(0); }
            self.redone.clear();
            cx.emit(MapEvent::Changed);
        }
        cx.notify();
        true
    }

    fn undo(&mut self, back: bool, cx: &mut Context<Self>) {
        let (from, to) = if back { (&mut self.undone, &mut self.redone) } else { (&mut self.redone, &mut self.undone) };
        let Some(map) = from.pop() else { return };
        to.push(std::mem::replace(&mut self.map, map));
        self.editing = None;
        if self.chosen.is_some_and(|id| self.map.node(id).is_none()) { self.chosen = Some(self.map.root.id) }
        cx.emit(MapEvent::Changed);
        cx.notify();
    }

    fn choose(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some_and(|editing| editing != id) { self.finish(true, cx) }
        (self.chosen, self.reveal, self.asking) = (Some(id), true, false);
        if self.editing.is_none() { window.focus(&self.focus) }
        cx.notify();
    }

    /// Write in a node: its words in a field where it stands. Enter keeps them, Esc leaves the node as it was,
    /// and whatever takes the keyboard away keeps them too.
    fn edit(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.map.node(id).map(|node| node.text.clone()) else { return };
        (self.chosen, self.editing, self.reveal) = (Some(id), Some(id), true);
        self.input.update(cx, |input, cx| {
            input.set_text(&text, cx);
            input.select_range(0..text.len(), cx);
        });
        if self.blur.is_none() {
            let handle = self.input.focus_handle(cx);
            self.blur = Some(cx.on_blur(&handle, window, |this, _, cx| this.finish(true, cx)));
        }
        self.input.read(cx).focus(window);
        cx.notify();
    }

    /// The node written in is done with. Kept, its words are the field's; a node left without words is one
    /// that was made and not wanted, and goes.
    fn finish(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(id) = self.editing.take() else { return };
        let text = self.input.read(cx).text().trim().to_string();
        let empty = self.map.node(id).is_some_and(|node| node.text.is_empty());
        if (keep && text.is_empty() || !keep && empty) && id != self.map.root.id {
            // A node just made and left empty was never part of the map: taking it out is not a step of its own.
            let chosen = self.map.remove(id);
            if empty { self.undone.pop(); }
            self.chosen = chosen;
            cx.emit(MapEvent::Changed);
        } else if keep && !text.is_empty() {
            let before = self.map.clone();
            if self.map.set_text(id, &text) {
                // The words of a node just made belong to the step that made it.
                if !empty { self.undone.push(before) }
                self.redone.clear();
                cx.emit(MapEvent::Changed);
            }
        }
        cx.notify();
    }

    /// A new node beside the one chosen (`child`: under it), written in at once.
    fn add(&mut self, child: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chosen) = self.chosen else { return };
        if self.editing.is_some() { self.finish(true, cx) }
        if self.change(cx, |this| if child { this.map.add_child(chosen, "", None) } else { this.map.add_after(chosen, "") }) && let Some(id) = self.chosen {
            self.edit(id, window, cx);
        }
    }

    fn step(&mut self, step: Step, cx: &mut Context<Self>) {
        let from = self.chosen.unwrap_or(self.map.root.id);
        // Into a folded node is to unfold it: its children are what the key asks for.
        if step == Step::In && self.map.node(from).is_some_and(|node| node.folded) {
            self.change(cx, |this| this.map.fold(from).then_some(from));
            return;
        }
        if let Some(to) = self.map.step(from, step) {
            (self.chosen, self.reveal) = (Some(to), true);
            cx.notify();
        }
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        let Some(chosen) = self.chosen else { return };
        self.change(cx, |this| this.map.remove(chosen));
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() { return }
        let Some(chosen) = self.chosen else { return };
        let keys = &event.keystroke;
        let handled = match keys.key.as_str() {
            "tab" if keys.modifiers.shift => self.change(cx, |this| this.map.outdent(chosen).then_some(chosen)) || true,
            "tab" => { self.add(true, window, cx); true }
            "space" | "f2" => { self.edit(chosen, window, cx); true }
            "up" | "down" if keys.modifiers.alt => self.change(cx, |this| this.map.shift(chosen, keys.key == "down").then_some(chosen)) || true,
            "right" if keys.modifiers.alt => self.change(cx, |this| this.map.indent(chosen).then_some(chosen)) || true,
            "left" if keys.modifiers.alt => self.change(cx, |this| this.map.outdent(chosen).then_some(chosen)) || true,
            "z" if keys.modifiers.secondary() => { self.undo(!keys.modifiers.shift, cx); true }
            _ => false,
        };
        if handled { cx.stop_propagation() }
    }

    /// How large a node is drawn at `zoom`: its words wrapped at `NODE_WIDE`, with room for its page after them.
    /// Measured at the size it is drawn, so the words wrap where they are drawn.
    fn size(node: &Node, depth: usize, zoom: f32, window: &Window) -> (f32, f32) {
        let size = text_px(if depth == 0 { 14. } else { 13. }) * zoom;
        let line = f32::from(text_px(if depth == 0 { 21. } else { 19. })) * zoom;
        let wide_most = NODE_WIDE * zoom;
        let text: SharedString = if node.text.is_empty() { " ".into() } else { node.text.replace('\n', " ").into() };
        let mut font = window.text_style().font();
        if depth == 0 { font.weight = FontWeight::SEMIBOLD }
        let run = TextRun { len: text.len(), font, color: Theme::text(), background_color: None, underline: None, strikethrough: None };
        let (mut wide, mut lines) = (0f32, 0usize);
        if let Ok(shaped) = window.text_system().shape_text(text, size, &[run], Some(px(wide_most)), None) {
            for one in &shaped {
                lines += one.wrap_boundaries().len() + 1;
                wide = wide.max(if one.wrap_boundaries().is_empty() { f32::from(one.width()) } else { wide_most });
            }
        }
        let chip = if node.anchor.is_some() { PAGE_CHIP * zoom } else { 0. };
        // The node's border is drawn inside its width: the words keep the width they were measured at.
        ((wide.ceil() + 1.).max(24. * zoom) + 2. * NODE_PAD.0 * zoom + 2. + chip, lines.max(1) as f32 * line + 2. * NODE_PAD.1 * zoom + 2.)
    }

    /// The lines from each node to its children, drawn under the nodes.
    fn links(drawn: &Layout, zoom: f32) -> impl IntoElement {
        let lines: Vec<(Point<Pixels>, Point<Pixels>)> = drawn.nodes.iter().filter_map(|node| {
            let parent = &drawn.nodes[node.parent?];
            Some((point(px(parent.x + parent.w), px(parent.y + parent.h / 2.)), point(px(node.x), px(node.y + node.h / 2.))))
        }).collect();
        let color = Theme::line_strong();
        canvas(|_, _, _| {}, move |bounds, _, window, _| {
            for (from, to) in &lines {
                let (from, to) = (bounds.origin + *from, bounds.origin + *to);
                let middle = (from.x + to.x) / 2.;
                let mut path = PathBuilder::stroke(px(1.25 * zoom.max(0.6)));
                path.move_to(from);
                path.cubic_bezier_to(to, point(middle, from.y), point(middle, to.y));
                if let Ok(path) = path.build() { window.paint_path(path, color) }
            }
        }).absolute().size_full()
    }

    /// Where the map's own top left stands in the view.
    fn origin(&self) -> (f32, f32) {
        (self.pan.0 + EDGE, self.pan.1 + HEAD)
    }

    /// A point of the window as a point of the view.
    fn in_view(&self, at: Point<Pixels>) -> (f32, f32) {
        let view = self.view.get();
        (f32::from(at.x - view.origin.x), f32::from(at.y - view.origin.y))
    }

    /// Draw the map `to` times its own size, the point `about` of the view (its middle without one) staying put.
    fn zoom_to(&mut self, to: f32, about: Option<(f32, f32)>, cx: &mut Context<Self>) {
        let to = to.clamp(ZOOM.0, ZOOM.1);
        if (to - self.zoom).abs() < 0.001 { return }
        let size = self.view.get().size;
        let about = about.unwrap_or((f32::from(size.width) / 2., f32::from(size.height) / 2.));
        let ((x, y), k) = (self.origin(), to / self.zoom);
        self.pan = (about.0 - (about.0 - x) * k - EDGE, about.1 - (about.1 - y) * k - HEAD);
        self.zoom = to;
        cx.notify();
    }

    /// The wheel moves the map; with ⌘ or Ctrl (a pinch comes as the wheel with Ctrl) it zooms about the pointer.
    fn wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let delta = event.delta.pixel_delta(px(20.));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if event.modifiers.secondary() || event.modifiers.control {
            if dy != 0. { self.zoom_to(self.zoom * (dy / 240.).exp(), Some(self.in_view(event.position)), cx) }
            return;
        }
        // A wheel that only turns one way moves sideways with Shift held.
        let (dx, dy) = if event.modifiers.shift && dx == 0. { (dy, 0.) } else { (dx, dy) };
        self.pan = (self.pan.0 + dx, self.pan.1 + dy);
        cx.notify();
    }

    /// A node and all that hangs on it: what a drag of it carries, and where it cannot be dropped.
    fn carried(&self, id: u64) -> HashSet<u64> {
        fn walk(node: &Node, out: &mut HashSet<u64>) {
            out.insert(node.id);
            for child in &node.children { walk(child, out) }
        }
        let mut out = HashSet::new();
        if let Some(node) = self.map.node(id) { walk(node, &mut out) }
        out
    }

    /// Where node `id` would go let go at `at`, a point of the view: onto the middle of a node, under it; at its
    /// top or bottom edge, or the gap there, before or after it; to its right, under it. Never into what it
    /// carries, and nowhere it would not move.
    fn drop_at(&self, id: u64, at: (f32, f32)) -> Option<(u64, Place)> {
        let carried = self.carried(id);
        let (x, y) = self.origin();
        let (at, gap) = ((at.0 - x, at.1 - y), (GAP.0 * self.zoom, GAP.1 * self.zoom));
        let over = self.drawn.nodes.iter().find(|node| !carried.contains(&node.id) && at.0 >= node.x && at.0 <= node.x + node.w + gap.0 && at.1 >= node.y - gap.1 / 2. && at.1 <= node.y + node.h + gap.1 / 2.)?;
        let share = (at.1 - over.y) / over.h;
        let place = if over.depth == 0 || at.0 > over.x + over.w { Place::Under } else if share < 0.25 { Place::Before } else if share > 0.75 { Place::After } else { Place::Under };
        let mut map = self.map.clone();
        map.move_node(id, over.id, place).then_some((over.id, place))
    }

    /// A press on the map's ground, or on its root: the map is taken to be moved.
    fn press_ground(&mut self, at: Point<Pixels>) {
        self.grab = Some(Grab::Ground { from: at, pan: self.pan, moving: false });
    }

    /// The pointer moved with something held. A release that was not heard ends it.
    fn drag(&mut self, at: Point<Pixels>, held: bool, cx: &mut Context<Self>) {
        let Some(grab) = self.grab else { return };
        if !held { return self.release(cx) }
        let far = |from: Point<Pixels>| f32::from((at.x - from.x).abs()).max(f32::from((at.y - from.y).abs())) > DRAG;
        match grab {
            Grab::Ground { from, pan, moving } => {
                self.grab = Some(Grab::Ground { from, pan, moving: moving || far(from) });
                self.pan = (pan.0 + f32::from(at.x - from.x), pan.1 + f32::from(at.y - from.y));
            }
            Grab::Node { id, from, moving, .. } => {
                let moving = moving || far(from);
                let drop = if moving { self.drop_at(id, self.in_view(at)) } else { None };
                self.grab = Some(Grab::Node { id, from, at, moving, drop });
            }
        }
        cx.notify();
    }

    /// Let go: a node carried over a place is moved there, a step of Undo.
    fn release(&mut self, cx: &mut Context<Self>) {
        let Some(grab) = self.grab.take() else { return };
        if let Grab::Node { id, moving: true, drop: Some((target, place)), .. } = grab {
            self.change(cx, |this| this.map.move_node(id, target, place).then_some(id));
        }
        cx.notify();
    }

    /// The map's size: smaller, its own size, larger, and the whole of it in the view. At the view's foot, out of
    /// the way of the nodes, which start at the top left.
    fn zoom_bar(&self, cx: &mut Context<Self>) -> Div {
        let button = |id: &'static str, icon: &'static str, tip: &'static str| {
            div().id(id).flex().flex_none().items_center().justify_center().size(text_px(22.)).rounded(px(5.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).tooltip(ui::tip(t(tip))).child(ui::icon(icon, 13., Theme::text_secondary()))
        };
        let percent = div().id("mind-map-zoom-reset").flex().flex_none().items_center().justify_center().w(text_px(42.)).h(text_px(22.)).rounded(px(5.)).text_size(text_px(11.)).text_color(Theme::text_secondary()).cursor_pointer().hover(|s| s.bg(Theme::hover()))
            .tooltip(ui::tip(t("原始大小")))
            .child(format!("{:.0}%", self.zoom * 100.))
            .on_click(cx.listener(|this, _, _, cx| this.zoom_to(1., None, cx)));
        div().absolute().right(px(10.)).bottom(px(10.)).flex().items_center().gap(px(1.)).p(px(2.)).rounded(px(7.)).bg(Theme::bg_body()).border_1().border_color(Theme::line())
            // A press on the bar is not a press on the map's ground.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(button("mind-map-zoom-out", "minus", "缩小").on_click(cx.listener(|this, _, _, cx| this.zoom_to(this.zoom / ZOOM_STEP, None, cx))))
            .child(percent)
            .child(button("mind-map-zoom-in", "plus", "放大").on_click(cx.listener(|this, _, _, cx| this.zoom_to(this.zoom * ZOOM_STEP, None, cx))))
            .child(button("mind-map-fit", "scan", "整张导图放进这一栏").on_click(cx.listener(|this, _, _, cx| { this.fit = true; cx.notify() })))
    }

    /// The map's own tools, beside the column's label: a draft from the model, and the outline to the clipboard.
    fn tools(&self, cx: &mut Context<Self>) -> Div {
        let button = |id: &'static str, icon: &'static str, tip: &'static str| {
            div().id(id).flex().flex_none().items_center().justify_center().size(text_px(20.)).rounded(px(5.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).tooltip(ui::tip(t(tip))).child(ui::icon(icon, 13., Theme::text_tertiary()))
        };
        let own = self.map.count() > 1;
        let draft = if self.drafting {
            div().flex().flex_none().items_center().gap(px(5.)).px(px(4.)).text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(ui::spinner("mind-map-drafting", 12., Theme::accent())).child(t("正在起草…")).into_any_element()
        } else {
            button("mind-map-draft", "sparkle", if own { "让模型重新起草这张导图" } else { "让模型起草这张导图" }).on_click(cx.listener(move |this, _, _, cx| {
                // A draft takes the place of what is there: with nodes of the reader's, it is asked first.
                if own && !this.asking { this.asking = true } else { cx.emit(MapEvent::Draft) }
                cx.notify();
            })).into_any_element()
        };
        let copy = button("mind-map-copy", "copy", "复制大纲").on_click(cx.listener(|this, _, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(this.map.outline()))));
        let small = |id: &'static str, label: &'static str| div().id(id).flex().flex_none().items_center().h(text_px(20.)).px(px(6.)).rounded(px(5.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).child(t(label));
        let asking = self.asking.then(|| {
            div().flex().flex_none().items_center().gap(px(2.)).pl(px(6.)).text_size(text_px(11.)).text_color(Theme::text_secondary())
                .child(t("用模型的草稿换掉现在的导图？"))
                .child(small("mind-map-draft-yes", "换掉").text_color(Theme::accent()).on_click(cx.listener(|_, _, _, cx| cx.emit(MapEvent::Draft))))
                .child(small("mind-map-draft-no", "不换").on_click(cx.listener(|this, _, _, cx| { this.asking = false; cx.notify() })))
        });
        let failed = self.failed.clone().map(|error| div().flex_1().min_w_0().truncate().pl(px(6.)).text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(error));
        div().absolute().top(px(6.)).left(px(58.)).right(px(8.)).flex().items_center().gap(px(2.)).h(text_px(20.)).child(draft).when(own && !self.drafting, |d| d.child(copy)).children(asking).children(failed)
    }
}

impl Render for MindMapView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.refocus) && self.editing.is_none() { window.focus(&self.focus) }
        let view = self.view.get().size;
        let (wide, high) = (f32::from(view.width), f32::from(view.height));
        // The whole map in the view, in its middle: measured at its own size, then drawn at the size that fits.
        if wide > 0. && std::mem::take(&mut self.fit) {
            let whole = mindmap::layout(&self.map, &|node, depth| Self::size(node, depth, 1., window), GAP);
            let room = (wide - 2. * EDGE, high - HEAD - EDGE - 30.);
            if whole.size.0 > 0. && whole.size.1 > 0. {
                self.zoom = (room.0 / whole.size.0).min(room.1 / whole.size.1).clamp(ZOOM.0, 1.25);
                self.pan = ((wide - whole.size.0 * self.zoom) / 2. - EDGE, ((room.1 - whole.size.1 * self.zoom) / 2.).max(0.));
            }
        }
        let zoom = self.zoom;
        let gap = (GAP.0 * zoom, GAP.1 * zoom);
        let drawn = mindmap::layout(&self.map, &|node, depth| Self::size(node, depth, zoom, window), gap);
        if wide > 0. {
            // The node chosen comes into view: after a key, a new node, a passage sent from the pages.
            if std::mem::take(&mut self.reveal) && let Some(node) = self.chosen.and_then(|id| drawn.nodes.iter().find(|node| node.id == id)) {
                let ((x, y), room) = (self.origin(), 16.);
                let (left, top) = (x + node.x, y + node.y);
                if left - room < 0. || node.w + 2. * room > wide { self.pan.0 += room - left } else if left + node.w + room > wide { self.pan.0 -= left + node.w + room - wide }
                if top < HEAD { self.pan.1 += HEAD - top } else if top + node.h + room > high { self.pan.1 -= (top + node.h + room - high).min(top - HEAD) }
            }
            // However far it is pulled, a piece of the map stays in view.
            let keep = |pan: f32, edge: f32, size: f32, view: f32| {
                let (least, most) = (KEEP.min(size) - edge - size, view - KEEP - edge);
                if least <= most { pan.clamp(least, most) } else { pan }
            };
            self.pan = (keep(self.pan.0, EDGE, drawn.size.0, wide), keep(self.pan.1, HEAD, drawn.size.1, high));
        }
        let (x, y) = self.origin();
        // What a drag carries is drawn faint where it was, and where it would go is marked.
        let (carried, drop) = match self.grab {
            Some(Grab::Node { id, moving: true, drop, .. }) => (self.carried(id), drop),
            _ => (HashSet::new(), None),
        };
        let text_size = |root: bool| text_px(if root { 14. } else { 13. }) * zoom;
        let line_height = |root: bool| text_px(if root { 21. } else { 19. }) * zoom;
        let nodes: Vec<AnyElement> = drawn.nodes.iter().map(|placed| {
            let Some(node) = self.map.node(placed.id) else { return div().into_any_element() };
            let (id, root, chosen) = (placed.id, placed.depth == 0, self.chosen == Some(placed.id));
            let editing = self.editing == Some(id);
            let target = drop.is_some_and(|(target, place)| target == id && place == Place::Under);
            let ground = if root { Theme::accent() } else if placed.depth == 1 { Theme::accent_soft() } else { Theme::bg_body() };
            let ink = if root { Theme::on_accent() } else { Theme::text() };
            let page = node.anchor.clone().map(|anchor| {
                let tip = if anchor.quote.is_empty() { t("在文献里看这一页").to_string() } else { format!("“{}”", anchor.quote) };
                div().id(("mind-map-page", id as usize)).flex().flex_none().items_center().justify_center().w(px((PAGE_CHIP - 6.) * zoom)).ml(px(6. * zoom)).h(text_px(16.) * zoom).rounded(px(4. * zoom)).text_size(text_px(10.) * zoom).cursor_pointer()
                    .text_color(if root { Theme::on_accent() } else { Theme::accent() }).bg(if root { Theme::on_accent().opacity(0.18) } else { Theme::bg_body() }).hover(|s| s.bg(Theme::hover()))
                    .tooltip(ui::tip(tip))
                    .child(format!("p.{}", anchor.page))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose(id, window, cx);
                        cx.emit(MapEvent::Goto(anchor.clone()));
                    }))
            });
            let least = (140. + 2. * NODE_PAD.0) * zoom;
            let words = if editing {
                // A pixel width: a field under ancestors without one is measured at its own default.
                div().w(px((placed.w - 2. * NODE_PAD.0 * zoom).max(140. * zoom))).child(self.input.clone()).into_any_element()
            } else {
                div().flex_1().min_w_0().child(if node.text.is_empty() { SharedString::from(" ") } else { SharedString::from(node.text.clone()) }).into_any_element()
            };
            // Folded, a node says how many it holds; unfolded, the dot at its right folds them away.
            let fold = (placed.children > 0).then(|| {
                div().id(("mind-map-fold", id as usize)).absolute().left(px(placed.x + placed.w + 3. * zoom)).top(px(placed.y + placed.h / 2. - 8. * zoom)).flex().items_center().justify_center().h(px(16. * zoom)).min_w(px(16. * zoom)).px(px(if placed.folded { 4. * zoom } else { 0. })).rounded_full()
                    .bg(Theme::bg_body()).border_1().border_color(Theme::line_strong()).text_size(text_px(10.) * zoom).text_color(Theme::text_tertiary()).cursor_pointer().hover(|s| s.bg(Theme::hover()))
                    .when(placed.folded, |d| d.child(format!("{}", placed.children)))
                    .when(!placed.folded, |d| d.child(ui::icon("minus", 9. * zoom, Theme::text_tertiary())))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| { this.change(cx, |this| this.map.fold(id).then_some(id)); }))
            });
            let card = div()
                .id(("mind-map-node", id as usize))
                .absolute()
                .left(px(placed.x))
                .top(px(placed.y))
                .w(px(if editing { placed.w.max(least) } else { placed.w }))
                .min_h(px(placed.h))
                .flex()
                .items_center()
                .px(px(NODE_PAD.0 * zoom))
                .py(px(NODE_PAD.1 * zoom))
                .rounded(px(if root { 9. } else { 7. } * zoom))
                .bg(ground)
                .border_1()
                .border_color(if chosen || target { Theme::accent() } else if placed.depth >= 2 { Theme::line() } else { ground })
                .when(chosen && !target, |d| d.shadow(vec![BoxShadow { color: Theme::accent().opacity(0.35), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(1.5) }]))
                .when(target, |d| d.shadow(vec![BoxShadow { color: Theme::accent().opacity(0.5), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(3.) }]))
                .when(carried.contains(&id), |d| d.opacity(0.35))
                .text_size(text_size(root))
                .line_height(line_height(root))
                .when(root, |d| d.font_weight(FontWeight::SEMIBOLD))
                .text_color(ink)
                .cursor_pointer()
                .child(words)
                .children(page.filter(|_| !editing))
                // Held, the root moves the map and any other node is taken to be carried; a click is still a click.
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if this.editing == Some(id) { return }
                    if root { return this.press_ground(event.position) }
                    this.grab = Some(Grab::Node { id, from: event.position, at: event.position, moving: false, drop: None });
                }))
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 { this.edit(id, window, cx) } else { this.choose(id, window, cx) }
                }));
            div().children(fold.filter(|_| !editing && !carried.contains(&id))).child(card).into_any_element()
        }).collect();
        // Before or after a node: a line in the gap on that side of it.
        let line = drop.and_then(|(target, place)| {
            let node = drawn.nodes.iter().find(|node| node.id == target).filter(|_| place != Place::Under)?;
            let at = if place == Place::Before { node.y - gap.1 / 2. } else { node.y + node.h + gap.1 / 2. };
            Some(div().absolute().left(px(node.x)).top(px(at - 1.5)).w(px(node.w)).h(px(3.)).rounded(px(1.5)).bg(Theme::accent()))
        });
        // With nothing but its root, the map says how it is made.
        let hint = (self.map.count() <= 1 && !self.drafting).then(|| {
            let root = &drawn.nodes[0];
            div().absolute().left(px(root.x)).top(px(root.y + root.h + 14.)).w(px(300.)).flex().flex_col().gap(px(3.)).text_size(text_px(12.)).line_height(text_px(19.)).text_color(Theme::text_tertiary())
                .child(t("Tab 加一个分支，Enter 加同级，空格改字，Delete 删除。"))
                .child(t("在 PDF 里选一段，点「加到导图」，它就挂在选中的节点下面，点页码回到原文。"))
                .child(t("也可以点上面的 ✦ 让模型先起草，再自己改。"))
                .child(t("拖动空白处移动导图，⌘ 或 Ctrl 加滚轮缩放；把节点拖到另一个节点上，它就挂到那里。"))
        });
        let sheet = div().absolute().left(px(x)).top(px(y)).w(px(drawn.size.0 + 40. * zoom)).h(px(drawn.size.1)).child(Self::links(&drawn, zoom)).children(nodes).children(line).children(hint);
        // The node carried, beside the pointer: the pointer itself says where it goes, and the mark there stays in sight.
        let ghost = match self.grab {
            Some(Grab::Node { id, at, moving: true, .. }) => drawn.nodes.iter().find(|node| node.id == id).zip(self.map.node(id)).map(|(placed, node)| {
                let at = self.in_view(at);
                div().absolute().left(px(at.0 + 14.)).top(px(at.1 + 10.)).w(px(placed.w)).min_h(px(placed.h)).flex().items_center().px(px(NODE_PAD.0 * zoom)).py(px(NODE_PAD.1 * zoom)).rounded(px(7. * zoom))
                    .bg(Theme::bg_body()).border_1().border_color(Theme::accent()).opacity(0.8)
                    .shadow(vec![BoxShadow { color: Theme::text().opacity(0.16), offset: point(px(0.), px(3.)), blur_radius: px(10.), spread_radius: px(0.) }])
                    .text_size(text_size(false)).line_height(line_height(false)).text_color(Theme::text())
                    .child(div().flex_1().min_w_0().child(SharedString::from(node.text.clone())))
            }),
            _ => None,
        };
        let moving = self.grab.is_some_and(|grab| grab.moving());
        self.drawn = drawn;
        let weak = cx.entity().downgrade();
        let bounds = self.view.clone();
        let held = self.grab.is_some();
        div()
            .id("mind-map")
            .key_context("MindMap")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(Theme::bg_sunken())
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.step(Step::Up, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.step(Step::Down, cx)))
            .on_action(cx.listener(|this, _: &MoveLeft, _, cx| this.step(Step::Out, cx)))
            .on_action(cx.listener(|this, _: &MoveRight, _, cx| this.step(Step::In, cx)))
            .on_action(cx.listener(|this, _: &OpenSelected, window, cx| this.add(false, window, cx)))
            // Space is bound for 筛选 (an action, heard before any key): here it writes in the node chosen, as F2 does.
            .on_action(cx.listener(|this, _: &TickSuggestion, window, cx| if let Some(chosen) = this.chosen.filter(|_| this.editing.is_none()) { this.edit(chosen, window, cx) }))
            .on_action(cx.listener(|this, _: &TrashSelected, _, cx| this.remove(cx)))
            .on_action(cx.listener(|this, _: &crate::pdf::RemoveChosen, _, cx| this.remove(cx)))
            // The keys that size a PDF size the map while it has the keyboard; the fit is the whole map.
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom_to(this.zoom * ZOOM_STEP, None, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom_to(this.zoom / ZOOM_STEP, None, cx)))
            .on_action(cx.listener(|this, _: &ZoomFit, _, cx| { this.fit = true; cx.notify() }))
            // Esc puts down a node being carried, then the question about a draft. In a node's field it ends the
            // edit and nothing more: the field hands Esc on for a dialog around it to close, and that is the map.
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| {
                if this.editing.is_some() { return }
                if this.grab.take().is_some() || std::mem::take(&mut this.asking) { cx.notify() } else { cx.propagate() }
            }))
            .on_key_down(cx.listener(Self::key))
            .on_scroll_wheel(cx.listener(Self::wheel))
            // A press on the ground of the map takes the keyboard for it and the map to be moved, and is not a press on the pages.
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                if this.editing.is_some() { this.finish(true, cx) }
                window.focus(&this.focus);
                this.press_ground(event.position);
            }))
            // Where the view stands in the window, for the pointer; and while something is held, the pointer anywhere.
            .child(canvas(move |bounds_now, _, _| bounds.set(bounds_now), move |_, _, window, _| {
                if !held { return }
                let up = weak.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase.bubble() { let _ = weak.update(cx, |this, cx| this.drag(event.position, event.pressed_button == Some(MouseButton::Left), cx)); }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase.bubble() && event.button == MouseButton::Left { let _ = up.update(cx, |this, cx| this.release(cx)); }
                });
            }).absolute().size_full())
            .child(sheet)
            .children(ghost)
            // While the map or a node is moved, the pointer is a closed hand over everything, and nothing under it lights up.
            .when(moving, |d| d.child(div().id("mind-map-held").absolute().size_full().occlude().cursor_grabbing()))
            .child(self.tools(cx))
            .child(self.zoom_bar(cx))
    }
}
