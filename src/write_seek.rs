//! 写作's search: the shortcut `search` (⌘F / Ctrl+F) on the writing page
//! opens a bar over the manuscript that looks for words in every section of
//! it, instead of in the papers. What is found in the section on show is lit
//! as it is typed, and the first place after the caret selected; Enter and
//! the arrows go from one place to the next, into another section when that
//! is where the next one is. Esc puts the bar away with the caret on the
//! place gone to.
//!
//! A page read through (预览, 译文对照, 译稿) stays as it is while it is
//! searched: the places are lit where the page draws them, side by side or
//! one under the other as the person has it, and going to one scrolls the
//! page there instead of opening the section to write. Over a section, a
//! place in another moves the page to that section, as a pill does. On 译稿's
//! page the version is looked in too, each block after the words it
//! translates; read alone, it is all that is looked in.

use crate::app::{Cmd, Workspace};
use crate::i18n::t;
use crate::input::{InputEvent, TextInput};
use crate::markdown::Place;
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use wisp_core::bilingual;
use wisp_core::follow::{self, Part};
use wisp_core::manuscript;
use wisp_core::preview::Reading;

/// Frames to wait for the editor to draw a section gone to before giving up on bringing the place into view.
const REVEAL_FRAMES: u8 = 8;

/// A place the words stand: in a section's text (`unit` None: its bytes there), or in the translation of a unit
/// of the version on show (翻译: its bytes in the translation as written, citations as the manuscript has them).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hit {
    pub(crate) section: String,
    pub(crate) unit: Option<usize>,
    pub(crate) range: Range<usize>,
}

/// What holds places on a page read through: a stretch of a section's text drawn (its bytes), or a unit of the version.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Holder {
    Words(String, Range<usize>),
    Unit(usize),
}

pub struct Seek {
    pub open: bool,
    pub input: Entity<TextInput>,
    /// The place gone to last.
    at: Option<Hit>,
    /// A byte of the section on show to bring into view once the editor has drawn it, and the frames left to wait.
    reveal: Option<(usize, u8)>,
    /// The byte is a heading's: its line goes to the top of the view, as a table of contents takes one there.
    top: bool,
    /// On a page read through: where each holder of places was last drawn, its top and bottom in the page. Emptied
    /// before the page is drawn, filled as it is.
    held: Rc<RefCell<Vec<(Holder, f32, f32)>>>,
    /// Where the place gone to was last drawn there, its top in the page: a stretch of words says it to the line.
    found: Rc<Cell<Option<f32>>>,
    /// The place gone to there waits to be brought into view: the frames left to wait for it to be drawn.
    read_reveal: Option<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// The next place (true) or the one before.
    Step(bool),
    Close,
}

impl Seek {
    /// A byte of the section on show to bring into view once the editor has drawn it (a place gone to from elsewhere: 预览).
    pub(crate) fn reveal_at(&mut self, at: usize) {
        (self.reveal, self.top) = (Some((at, REVEAL_FRAMES)), false);
    }

    /// A heading's line to the top of the view once the editor has drawn it (one gone to from the outline).
    pub(crate) fn reveal_heading(&mut self, at: usize) {
        (self.reveal, self.top) = (Some((at, REVEAL_FRAMES)), true);
    }

    pub fn new(subscriptions: &mut Vec<Subscription>, cx: &mut Context<Workspace>) -> Seek {
        let input = cx.new(|cx| TextInput::new("在稿件里查找", cx));
        // Esc goes on to the workspace, which puts the bar away (`Workspace::dismiss`).
        subscriptions.push(cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Submit => this.seek_step(true, cx),
            InputEvent::Changed => this.seek_typed(cx),
            _ => {}
        }));
        Seek { open: false, input, at: None, reveal: None, top: false, held: Rc::default(), found: Rc::default(), read_reveal: None }
    }
}

impl Workspace {
    pub(crate) fn run_seek(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            Do::Step(forward) => {
                self.seek_step(forward, cx);
                if let Some(lib) = self.lib.as_ref() { lib.writing.seek.input.read(cx).focus(window) }
            }
            Do::Close => self.seek_close(window, cx),
        }
    }

    /// The bar over the manuscript, with the words selected: what is selected in the manuscript (or on the page read
    /// through), if a short piece of one line, else what was looked for last.
    pub(crate) fn seek_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let read = self.read_written().and_then(|(section, range)| self.lib.as_ref()?.writing.current()?.section(&section)?.body.get(range).map(str::to_string));
        let Some(lib) = self.lib.as_mut() else { return };
        let writing = &mut lib.writing;
        let picked = if writing.reading { read.unwrap_or_default() } else { writing.editor.read(cx).selected().to_string() };
        writing.seek.open = true;
        let input = writing.seek.input.clone();
        input.update(cx, |input, cx| {
            if !picked.trim().is_empty() && !picked.contains('\n') && picked.chars().count() <= 80 { input.set_text(&picked, cx) }
            let end = input.text().len();
            input.select_range(0..end, cx);
        });
        input.read(cx).focus(window);
        self.seek_typed(cx);
    }

    fn seek_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let seek = &mut lib.writing.seek;
        if !seek.open { return }
        (seek.open, seek.at, seek.read_reveal) = (false, None, None);
        // Back to writing, where the place gone to is selected.
        if !lib.writing.reading { lib.writing.editor.read(cx).focus(window) }
        cx.notify();
    }

    /// Every place the words stand in the manuscript on show, section by section in its order: the section being
    /// written as the editor has it; on a page read through, as the page draws it, and on 译稿's the version's too,
    /// each after the words it translates.
    fn seek_hits(&self, cx: &App) -> Vec<Hit> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        let writing = &lib.writing;
        let Some(manuscript) = writing.current() else { return vec![] };
        let query = writing.seek.input.read(cx).text();
        if query.trim().is_empty() { return vec![] }
        let current = writing.editor.read(cx).text();
        let version = self.translate_shown().and_then(|key| writing.translate.kept.get(&key));
        let alone = version.is_some() && writing.translate.alone;
        manuscript
            .sections
            .iter()
            .flat_map(|section| {
                let text = if section.id == writing.section_id && !writing.reading { current } else { section.body.as_str() };
                // Beside the version a place is ordered by the block it is in, the manuscript's before the version's.
                let starts: Vec<usize> = if version.is_some() { bilingual::blocks(text).iter().map(|block| block.range.start).collect() } else { vec![] };
                let mut found: Vec<((usize, u8, usize), Hit)> = vec![];
                if !alone {
                    for range in manuscript::occurrences(text, query) {
                        let block = starts.iter().rev().find(|start| **start <= range.start).copied().unwrap_or(0);
                        found.push(((block, 0, range.start), Hit { section: section.id.clone(), unit: None, range }));
                    }
                }
                for (u, unit) in version.iter().flat_map(|version| version.units.iter().enumerate()).filter(|(_, unit)| unit.section == section.id && unit.part == Part::Text) {
                    let Some(kept) = version.and_then(|version| version.kept_at(u)).filter(|kept| !kept.text.trim().is_empty()) else { continue };
                    let block = starts.get(unit.block).copied().unwrap_or(usize::MAX);
                    for range in manuscript::occurrences(&follow::as_written(&kept.text, &unit.cites), query) {
                        found.push(((block, 1, range.start), Hit { section: section.id.clone(), unit: Some(u), range }));
                    }
                }
                found.sort_by_key(|(key, _)| *key);
                found.into_iter().map(|(_, hit)| hit).collect::<Vec<_>>()
            })
            .collect()
    }

    /// Whether a place is on show: in the section being written, or in what the page read through holds.
    fn seek_shown(&self, hit: &Hit) -> bool {
        let Some(writing) = self.lib.as_ref().map(|lib| &lib.writing) else { return false };
        if writing.reading { writing.facing_holds(&hit.section) } else { hit.unit.is_none() && hit.section == writing.section_id }
    }

    /// Where what holds a place was last drawn on the page read through: its top and bottom in the page.
    fn seek_held(&self, hit: &Hit) -> Option<(f32, f32)> {
        let held = self.lib.as_ref()?.writing.seek.held.borrow();
        held.iter()
            .find(|(holder, ..)| match (holder, hit.unit) {
                (Holder::Unit(u), Some(unit)) => *u == unit,
                (Holder::Words(section, within), None) => *section == hit.section && within.start <= hit.range.start && hit.range.start < within.end,
                _ => false,
            })
            .map(|(_, top, bottom)| (*top, *bottom))
    }

    /// Of `hits`, the first at or after the top of the page read through: drawn from there on, or in a section after
    /// what is on show.
    fn seek_from_view(&self, hits: &[Hit]) -> Option<usize> {
        let lib = self.lib.as_ref()?;
        let seen = -f32::from(lib.citing.read_scroll.offset().y);
        let order = |section: &str| lib.writing.current().and_then(|m| m.sections.iter().position(|s| s.id == section)).unwrap_or(0);
        let last = lib.writing.facing_sections().end.saturating_sub(1);
        hits.iter().position(|hit| match self.seek_held(hit) {
            Some((_, bottom)) => bottom > seen + 1.,
            None => order(&hit.section) > last,
        })
    }

    /// The words changed: the first place from the selection on in the section on show is selected, without leaving
    /// the section; on a page read through, the first on show from the top of the view.
    fn seek_typed(&mut self, cx: &mut Context<Self>) {
        let hits = self.seek_hits(cx);
        let Some(lib) = self.lib.as_ref() else { return };
        let here = if lib.writing.reading {
            self.seek_from_view(&hits).and_then(|n| hits.get(n)).filter(|hit| self.seek_shown(hit) && self.seek_held(hit).is_some()).cloned()
        } else {
            let from = lib.writing.editor.read(cx).selection().start;
            hits.into_iter().find(|hit| self.seek_shown(hit) && hit.range.start >= from)
        };
        let Some(lib) = self.lib.as_mut() else { return };
        lib.writing.seek.at = None;
        match here {
            Some(hit) => self.seek_go(hit, cx),
            None => cx.notify(),
        }
    }

    /// Go to the next place found, or the one before, from the place gone to last or else from the caret (on a page
    /// read through, from the top of the view); past the end, around to the beginning.
    fn seek_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let hits = self.seek_hits(cx);
        let Some(lib) = self.lib.as_ref() else { return };
        let writing = &lib.writing;
        if hits.is_empty() { return }
        let order = |section: &str| writing.current().and_then(|m| m.sections.iter().position(|s| s.id == section)).unwrap_or(0);
        let wanted = match writing.seek.at.as_ref().and_then(|at| hits.iter().position(|hit| hit == at)) {
            Some(n) if forward => (n + 1) % hits.len(),
            Some(n) => (n + hits.len() - 1) % hits.len(),
            None => {
                let after = if writing.reading {
                    self.seek_from_view(&hits).unwrap_or(hits.len())
                } else {
                    // The first place at or after the caret, in the manuscript's order.
                    let caret = (order(&writing.section_id), writing.editor.read(cx).selection().start);
                    hits.iter().position(|hit| (order(&hit.section), hit.range.start) >= caret).unwrap_or(hits.len())
                };
                if forward { after % hits.len() } else { (after + hits.len() - 1) % hits.len() }
            }
        };
        self.seek_go(hits[wanted].clone(), cx);
    }

    /// Go to a place: on a page read through, the page stays as it is and scrolls there (over a section, one in
    /// another moves the page to that section); else the section it is in is shown to write and the place selected.
    fn seek_go(&mut self, hit: Hit, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        if lib.writing.reading {
            if !lib.writing.facing_holds(&hit.section) {
                // As a pill moves the page over a section (`pick_section`): what stands beside the words read goes first.
                self.read_letgo(cx);
                let Some(lib) = self.lib.as_mut() else { return };
                lib.writing.translate.editing = None;
                lib.citing.read_scroll.set_offset(point(px(0.), px(0.)));
                let manuscript = lib.writing.manuscript.clone();
                self.write_show(manuscript, hit.section.clone(), cx);
            }
            let Some(lib) = self.lib.as_mut() else { return };
            let writing = &mut lib.writing;
            // The paragraph the page was opened at no longer holds the view.
            writing.facing_reveal = None;
            writing.seek.found.set(None);
            writing.seek.read_reveal = Some(REVEAL_FRAMES);
            writing.seek.at = Some(hit);
            return cx.notify();
        }
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.writing.section_id != hit.section {
            let manuscript = lib.writing.manuscript.clone();
            self.write_show(manuscript, hit.section.clone(), cx);
        }
        let Some(lib) = self.lib.as_mut() else { return };
        let writing = &mut lib.writing;
        writing.editor.update(cx, |input, cx| input.select_range(hit.range.clone(), cx));
        writing.seek.reveal_at(hit.range.start);
        writing.seek.at = Some(hit);
        cx.notify();
    }

    /// Before the manuscript is drawn: the place gone to into view, once the editor (or the page read through) has
    /// drawn the text it is in. On a page read through, what holds places is then recorded anew as the page is drawn.
    pub(crate) fn seek_reveal(&mut self, window: &mut Window, cx: &App) {
        if self.lib.as_ref().is_some_and(|lib| lib.writing.reading) {
            self.seek_read_reveal(window);
            if let Some(lib) = self.lib.as_ref() { lib.writing.seek.held.borrow_mut().clear() }
            return;
        }
        let Some(lib) = self.lib.as_mut() else { return };
        let writing = &mut lib.writing;
        let Some((at, frames)) = writing.seek.reveal else { return };
        let Some(top) = writing.editor.read(cx).top_of(at) else {
            writing.seek.reveal = frames.checked_sub(1).map(|left| (at, left));
            return window.request_animation_frame();
        };
        writing.seek.reveal = None;
        // A heading at the top, under the sheet's margin.
        if std::mem::take(&mut writing.seek.top) { return writing.editor_scroll.set_offset(point(px(0.), -(top - px(8.)).max(px(0.)))) }
        let (seen, height) = (-writing.editor_scroll.offset().y, writing.editor_scroll.bounds().size.height);
        // A few lines above it stay in view, so it is read in its place.
        if top < seen + px(20.) || top + px(60.) > seen + height { writing.editor_scroll.set_offset(point(px(0.), -(top - px(90.)).max(px(0.)))) }
    }

    /// The place gone to on a page read through into view, once it is drawn: its line where a stretch of words says
    /// it, else the top of the unit of the version or the picture it is in.
    fn seek_read_reveal(&mut self, window: &mut Window) {
        let Some(lib) = self.lib.as_ref() else { return };
        let seek = &lib.writing.seek;
        let Some(frames) = seek.read_reveal else { return };
        let Some(hit) = seek.at.clone() else { return self.lib.as_mut().unwrap().writing.seek.read_reveal = None };
        let top = seek.found.get().filter(|_| hit.unit.is_none()).or_else(|| self.seek_held(&hit).map(|(top, _)| top));
        let lib = self.lib.as_mut().unwrap();
        let Some(top) = top else {
            lib.writing.seek.read_reveal = frames.checked_sub(1);
            return window.request_animation_frame();
        };
        lib.writing.seek.read_reveal = None;
        let scroll = &lib.citing.read_scroll;
        let (seen, height) = (-f32::from(scroll.offset().y), f32::from(scroll.bounds().size.height));
        if top < seen + 20. || top + 60. > seen + height { scroll.set_offset(point(px(0.), px(-(top - 90.).max(0.)))) }
    }

    /// The places found in the section on show, to be lit in the editor: the one gone to stronger.
    pub(crate) fn seek_marks(&self, text: &str, cx: &App) -> Vec<(Range<usize>, Hsla)> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        let seek = &lib.writing.seek;
        if !seek.open || lib.writing.reading { return vec![] }
        let current = seek.at.as_ref().filter(|hit| hit.unit.is_none() && hit.section == lib.writing.section_id).map(|hit| hit.range.clone());
        manuscript::occurrences(text, seek.input.read(cx).text()).into_iter().map(|range| { let tint = if Some(&range) == current.as_ref() { Theme::found_current() } else { Theme::found() }; (range, tint) }).collect()
    }

    /// What is looked for while the bar is open, else nothing.
    pub(crate) fn seek_query(&self, cx: &App) -> String {
        self.lib.as_ref().map(|lib| &lib.writing.seek).filter(|seek| seek.open).map(|seek| seek.input.read(cx).text().to_string()).unwrap_or_default()
    }

    /// The places of `query` in a stretch drawn on a page read through, `within` of the text `written` (a section's,
    /// or a unit's translation as written, `unit`), drawn as `shown` with `labels`: where each is drawn and its tint,
    /// and where the one gone to is, if it is here.
    pub(crate) fn seek_read_marks(&self, query: &str, section: &str, unit: Option<usize>, written: &str, labels: &[(Range<usize>, String)], within: Range<usize>, shown: &str) -> (Vec<(Place, Place, Hsla)>, Option<(Place, Place)>) {
        let Some(seek) = self.lib.as_ref().map(|lib| &lib.writing.seek).filter(|seek| seek.open) else { return (vec![], None) };
        let ranges: Vec<Range<usize>> = manuscript::occurrences(written, query).into_iter().filter(|range| within.start <= range.start && range.start < within.end).collect();
        if ranges.is_empty() { return (vec![], None) }
        let current = seek.at.as_ref().filter(|hit| hit.unit == unit && hit.section == section).map(|hit| hit.range.clone());
        let reading = Reading::new(written, labels, within, shown);
        let mut gone = None;
        let marks = ranges
            .into_iter()
            .filter_map(|range| {
                let (from, to) = reading.drawn(range.clone())?;
                let here = current.as_ref() == Some(&range);
                if here { gone = Some((from, to)) }
                Some((from, to, if here { Theme::found_current() } else { Theme::found() }))
            })
            .collect();
        (marks, gone)
    }

    /// What tells the search where the place gone to was drawn on a page read through (`Selecting::drawn` of the
    /// stretch it is in): its top in the page.
    pub(crate) fn seek_found_drawn(&self) -> Rc<dyn Fn(crate::markdown::Drawn, &mut Window, &mut App)> {
        let Some(lib) = self.lib.as_ref() else { return Rc::new(|_, _, _| {}) };
        let (found, scroll) = (lib.writing.seek.found.clone(), lib.citing.read_scroll.clone());
        Rc::new(move |drawn, _, _| {
            let (view, offset) = (scroll.bounds(), scroll.offset());
            found.set(Some(f32::from(drawn.start.y - view.origin.y - offset.y)));
        })
    }

    /// What records where something that holds places on a page read through is drawn, laid over it.
    pub(crate) fn seek_holder(&self, holder: Holder) -> Canvas<()> {
        let (held, scroll) = self.lib.as_ref().map(|lib| (lib.writing.seek.held.clone(), lib.citing.read_scroll.clone())).unzip();
        canvas(|_, _, _| (), move |bounds, _, _, _| {
            let (Some(held), Some(scroll)) = (held, scroll) else { return };
            let (view, offset) = (scroll.bounds(), scroll.offset());
            let top = f32::from(bounds.origin.y - view.origin.y - offset.y);
            held.borrow_mut().push((holder, top, top + f32::from(bounds.size.height)));
        })
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// Something on a page read through that holds places without drawing them as words (a picture over its
    /// caption), with what records where it is drawn.
    pub(crate) fn seek_hold(&self, holder: Holder, element: AnyElement) -> AnyElement {
        div().relative().child(element).child(self.seek_holder(holder)).into_any_element()
    }

    /// The `wseek:<words>` debug step (`dev.rs`): the bar open with the words typed.
    #[cfg(debug_assertions)]
    pub(crate) fn dev_seek(&mut self, words: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.seek_open(window, cx);
        let Some(input) = self.lib.as_ref().map(|lib| lib.writing.seek.input.clone()) else { return };
        input.update(cx, |input, cx| input.set_text(words, cx));
        self.seek_typed(cx);
    }

    /// The bar over the manuscript while it is open: the words, how many places there are and which is gone to, the arrows, and ×.
    pub(crate) fn render_seek(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let lib = self.lib.as_ref()?;
        let seek = &lib.writing.seek;
        if !seek.open { return None }
        // On 译稿's page the version is looked in as well, and the field says so.
        let both = self.translate_shown().is_some();
        seek.input.update(cx, |input, _| input.set_placeholder(if both { "在主稿和译稿里查找" } else { "在稿件里查找" }));
        let typed = !seek.input.read(cx).text().trim().is_empty();
        let hits = self.seek_hits(cx);
        let gone = seek.at.as_ref().and_then(|at| hits.iter().position(|hit| hit == at));
        let here = hits.iter().filter(|hit| self.seek_shown(hit)).count();
        let (count, color) = match (typed, hits.len(), gone) {
            (false, ..) => (String::new(), Theme::text_tertiary()),
            (true, 0, _) => (t("没有找到").to_string(), Theme::danger()),
            (true, n, Some(at)) => (format!("{}/{n}", at + 1), Theme::text_secondary()),
            (true, n, None) if here == 0 => (tf!("{n} 处，在其他节", "{n}, in other sections"), Theme::text_secondary()),
            (true, n, None) => (tf!("{n} 处", "{n} found"), Theme::text_secondary()),
        };
        let able = !hits.is_empty();
        let arrow = |id: &'static str, icon: &str, tip: &'static str, forward: bool, cx: &mut Context<Self>| ui::icon_action(id, icon, Theme::text_secondary(), t(tip)).when(!able, |d| d.opacity(0.4).cursor_default()).when(able, |d| d.on_click(self.on(Cmd::Seek(Do::Step(forward)), cx)));
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .mx(px(16.))
                .mt(px(10.))
                .child(div().flex_1().min_w_0().child(ui::field(&seek.input, window, cx).py(px(4.))))
                .child(div().flex_none().min_w(px(36.)).px(px(4.)).text_size(text_px(12.)).text_color(color).child(count))
                .child(arrow("seek-previous", "chevron-up", "上一处", false, cx))
                .child(arrow("seek-next", "chevron-down", "下一处（Enter）", true, cx))
                .child(ui::icon_action("seek-close", "x", Theme::text_tertiary(), t("关闭（Esc）")).on_click(self.on(Cmd::Seek(Do::Close), cx))),
        )
    }
}
