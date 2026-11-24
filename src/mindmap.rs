//! A paper's mind map (思维导图) as a column of the reader: the tree of
//! `wisp_core::mindmap` drawn with its root at the left, made by hand (the
//! keyboard on the node chosen, a passage sent from the pages) or drafted by
//! a model and then the reader's to change. The view holds the map and what
//! is chosen in it; whoever made it keeps the map (`MapEvent::Changed`),
//! shows a node's passage in the paper (`Goto`) and asks the model (`Draft`).

use crate::i18n::t;
use crate::input::{InputEvent, TextInput};
use crate::shortcuts::{Dismiss, MoveDown, MoveLeft, MoveRight, MoveUp, OpenSelected, TickSuggestion, TrashSelected};
use crate::theme::{text_px, Theme};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use wisp_core::mindmap::{self, Anchor, Layout, MindMap, Node, Step};

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
    scroll: ScrollHandle,
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
        MindMapView { map, chosen, editing: None, input, focus: cx.focus_handle(), scroll: ScrollHandle::new(), reveal: false, refocus: false, drafting: false, failed: None, asking: false, undone: vec![], redone: vec![], blur: None }
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
    /// `redo`, `up` / `down` / `in` / `out` to choose another, and `goto` for the passage of the one chosen.
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

    /// How large a node is drawn: its words wrapped at `NODE_WIDE`, with room for its page after them.
    fn size(node: &Node, depth: usize, window: &Window) -> (f32, f32) {
        let size = text_px(if depth == 0 { 14. } else { 13. });
        let line = f32::from(text_px(if depth == 0 { 21. } else { 19. }));
        let text: SharedString = if node.text.is_empty() { " ".into() } else { node.text.replace('\n', " ").into() };
        let mut font = window.text_style().font();
        if depth == 0 { font.weight = FontWeight::SEMIBOLD }
        let run = TextRun { len: text.len(), font, color: Theme::text(), background_color: None, underline: None, strikethrough: None };
        let (mut wide, mut lines) = (0f32, 0usize);
        if let Ok(shaped) = window.text_system().shape_text(text, size, &[run], Some(px(NODE_WIDE)), None) {
            for one in &shaped {
                lines += one.wrap_boundaries().len() + 1;
                wide = wide.max(if one.wrap_boundaries().is_empty() { f32::from(one.width()) } else { NODE_WIDE });
            }
        }
        let chip = if node.anchor.is_some() { PAGE_CHIP } else { 0. };
        // The node's border is drawn inside its width: the words keep the width they were measured at.
        ((wide.ceil() + 1.).max(24.) + 2. * NODE_PAD.0 + 2. + chip, lines.max(1) as f32 * line + 2. * NODE_PAD.1 + 2.)
    }

    /// The lines from each node to its children, drawn under the nodes.
    fn links(drawn: &Layout) -> impl IntoElement {
        let lines: Vec<(Point<Pixels>, Point<Pixels>)> = drawn.nodes.iter().filter_map(|node| {
            let parent = &drawn.nodes[node.parent?];
            Some((point(px(parent.x + parent.w), px(parent.y + parent.h / 2.)), point(px(node.x), px(node.y + node.h / 2.))))
        }).collect();
        let color = Theme::line_strong();
        canvas(|_, _, _| {}, move |bounds, _, window, _| {
            for (from, to) in &lines {
                let (from, to) = (bounds.origin + *from, bounds.origin + *to);
                let middle = (from.x + to.x) / 2.;
                let mut path = PathBuilder::stroke(px(1.25));
                path.move_to(from);
                path.cubic_bezier_to(to, point(middle, from.y), point(middle, to.y));
                if let Ok(path) = path.build() { window.paint_path(path, color) }
            }
        }).absolute().size_full()
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
        let drawn = mindmap::layout(&self.map, &|node, depth| Self::size(node, depth, window), GAP);
        // The node chosen comes into view: after a key, a new node, a passage sent from the pages.
        if std::mem::take(&mut self.reveal) && let Some(node) = self.chosen.and_then(|id| drawn.nodes.iter().find(|node| node.id == id)) {
            let view = self.scroll.bounds().size;
            let (mut at, room) = (self.scroll.offset(), 16.);
            let (left, top) = (EDGE + node.x, HEAD + node.y);
            if view.width > px(0.) {
                if px(left - room) + at.x < px(0.) { at.x = -px(left - room) } else if px(left + node.w + room) + at.x > view.width { at.x = view.width - px(left + node.w + room) }
                if px(top - room) + at.y < px(0.) { at.y = -px(top - room) } else if px(top + node.h + room) + at.y > view.height { at.y = view.height - px(top + node.h + room) }
                self.scroll.set_offset(point(at.x.min(px(0.)), at.y.min(px(0.))));
            }
        }
        let nodes: Vec<AnyElement> = drawn.nodes.iter().map(|placed| {
            let Some(node) = self.map.node(placed.id) else { return div().into_any_element() };
            let (id, root, chosen) = (placed.id, placed.depth == 0, self.chosen == Some(placed.id));
            let editing = self.editing == Some(id);
            let ground = if root { Theme::accent() } else if placed.depth == 1 { Theme::accent_soft() } else { Theme::bg_body() };
            let ink = if root { Theme::on_accent() } else { Theme::text() };
            let page = node.anchor.clone().map(|anchor| {
                let tip = if anchor.quote.is_empty() { t("在文献里看这一页").to_string() } else { format!("“{}”", anchor.quote) };
                div().id(("mind-map-page", id as usize)).flex().flex_none().items_center().justify_center().w(px(PAGE_CHIP - 6.)).ml(px(6.)).h(text_px(16.)).rounded(px(4.)).text_size(text_px(10.)).cursor_pointer()
                    .text_color(if root { Theme::on_accent() } else { Theme::accent() }).bg(if root { Theme::on_accent().opacity(0.18) } else { Theme::bg_body() }).hover(|s| s.bg(Theme::hover()))
                    .tooltip(ui::tip(tip))
                    .child(format!("p.{}", anchor.page))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose(id, window, cx);
                        cx.emit(MapEvent::Goto(anchor.clone()));
                    }))
            });
            let words = if editing {
                // A pixel width: a field under ancestors without one is measured at its own default.
                div().w(px((placed.w - 2. * NODE_PAD.0).max(140.))).child(self.input.clone()).into_any_element()
            } else {
                div().flex_1().min_w_0().child(if node.text.is_empty() { SharedString::from(" ") } else { SharedString::from(node.text.clone()) }).into_any_element()
            };
            // Folded, a node says how many it holds; unfolded, the dot at its right folds them away.
            let fold = (placed.children > 0).then(|| {
                div().id(("mind-map-fold", id as usize)).absolute().left(px(placed.x + placed.w + 3.)).top(px(placed.y + placed.h / 2. - 8.)).flex().items_center().justify_center().h(px(16.)).min_w(px(16.)).px(px(if placed.folded { 4. } else { 0. })).rounded_full()
                    .bg(Theme::bg_body()).border_1().border_color(Theme::line_strong()).text_size(text_px(10.)).text_color(Theme::text_tertiary()).cursor_pointer().hover(|s| s.bg(Theme::hover()))
                    .when(placed.folded, |d| d.child(format!("{}", placed.children)))
                    .when(!placed.folded, |d| d.child(ui::icon("minus", 9., Theme::text_tertiary())))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| { this.change(cx, |this| this.map.fold(id).then_some(id)); }))
            });
            let card = div()
                .id(("mind-map-node", id as usize))
                .absolute()
                .left(px(placed.x))
                .top(px(placed.y))
                .w(px(if editing { placed.w.max(140. + 2. * NODE_PAD.0) } else { placed.w }))
                .min_h(px(placed.h))
                .flex()
                .items_center()
                .px(px(NODE_PAD.0))
                .py(px(NODE_PAD.1))
                .rounded(px(if root { 9. } else { 7. }))
                .bg(ground)
                .border_1()
                .border_color(if chosen { Theme::accent() } else if placed.depth >= 2 { Theme::line() } else { ground })
                .when(chosen, |d| d.shadow(vec![BoxShadow { color: Theme::accent().opacity(0.35), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(1.5) }]))
                .text_size(text_px(if root { 14. } else { 13. }))
                .line_height(text_px(if root { 21. } else { 19. }))
                .when(root, |d| d.font_weight(FontWeight::SEMIBOLD))
                .text_color(ink)
                .cursor_pointer()
                .child(words)
                .children(page.filter(|_| !editing))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 { this.edit(id, window, cx) } else { this.choose(id, window, cx) }
                }));
            div().children(fold.filter(|_| !editing)).child(card).into_any_element()
        }).collect();
        // With nothing but its root, the map says how it is made.
        let hint = (self.map.count() <= 1 && !self.drafting).then(|| {
            let root = &drawn.nodes[0];
            div().absolute().left(px(root.x)).top(px(root.y + root.h + 14.)).w(px(300.)).flex().flex_col().gap(px(3.)).text_size(text_px(12.)).line_height(text_px(19.)).text_color(Theme::text_tertiary())
                .child(t("Tab 加一个分支，Enter 加同级，空格改字，Delete 删除。"))
                .child(t("在 PDF 里选一段，点「加到导图」，它就挂在选中的节点下面，点页码回到原文。"))
                .child(t("也可以点上面的 ✦ 让模型先起草，再自己改。"))
        });
        let (wide, tall) = (drawn.size.0 + 2. * EDGE + 40., drawn.size.1 + HEAD + EDGE + if hint.is_some() { 110. } else { 0. });
        let sheet = div().relative().flex_none().w(px(wide.max(360.))).h(px(tall))
            .child(div().absolute().left(px(EDGE)).top(px(HEAD)).w(px(drawn.size.0 + 40.)).h(px(drawn.size.1)).child(Self::links(&drawn)).children(nodes).children(hint));
        div()
            .id("mind-map")
            .key_context("MindMap")
            .track_focus(&self.focus)
            .size_full()
            .relative()
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
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| if std::mem::take(&mut this.asking) { cx.notify() } else { cx.propagate() }))
            .on_key_down(cx.listener(Self::key))
            // A press on the ground of the map takes the keyboard for it, and is not a press on the pages.
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                if this.editing.is_some() { this.finish(true, cx) }
                window.focus(&this.focus);
            }))
            .child(div().id("mind-map-scroll").size_full().overflow_scroll().track_scroll(&self.scroll).child(sheet))
            .child(self.tools(cx))
    }
}
