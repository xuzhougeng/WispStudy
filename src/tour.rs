//! 新手引导: a first window is walked through the app one step at a time. A
//! small card stands beside the thing to press next, a ring round that
//! thing; the page under them is neither dimmed nor closed off, because the
//! person does each step for real, in their own library. Which step is on
//! show and where the card stands is `wisp_core::tour`; here are the places
//! the steps point at, the card, and what begins and ends it.

use crate::app::{Cmd, Workspace};
use crate::i18n::t;
use crate::settings::SettingsPage;
use crate::theme::{self, Theme, text_px};
use crate::ui;
use gpui::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wisp_core::settings::Task;
use wisp_core::tour::{self, Seen, Step};

/// How wide the card is, before the text's scale.
const CARD: f32 = 292.;
/// How far the ring stands off what it surrounds, and how round its corners are.
const RING: f32 = 4.;
const RADIUS: f32 = 9.;

/// What the tour can be told.
#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// From its first step, or as far as the window already is: 设置 › 关于, the palette.
    Begin,
    /// 下一步 on the card of a step that only shows a place; on the last one, 完成.
    Next,
    /// The card's ×: over, and not begun again by itself.
    End,
}

/// The tour as the window holds it. How far it got is in the settings (`tour_step`, `tour_done`).
#[derive(Default)]
pub struct Tour {
    /// On show in this window: begun by a first start (`tour::due`) or asked for.
    pub on: bool,
    /// Where the things a step may point at were drawn in this frame, by name (`Workspace::tour_spot`).
    spots: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
}

/// What a step points at: the first of these groups that is on screen. The names of a group are one place
/// (the paper's three tabs, the four rows of a review's path).
fn places(step: Step, shelf: bool) -> &'static [&'static [&'static str]] {
    match step {
        Step::Library if shelf => &[&["home-library"], &["library-new"]],
        Step::Library => &[&["library-new"]],
        Step::Papers => &[&["guide-choice"], &["add-paper"]],
        Step::Open => &[&["paper-row"], &["view-all"]],
        Step::Read => &[&["tab-info", "tab-pdf", "tab-ask"]],
        Step::Ask => &[&["page-ask"]],
        Step::Path => &[&["page-sources", "page-screen", "page-extract", "page-write"]],
    }
}

/// What the card of a step says: its heading and the sentence under it. Each stands without the ring too, for
/// when its place is not on screen and the card waits in the corner.
fn words(step: Step, shelf: bool) -> (&'static str, &'static str) {
    match step {
        Step::Library if shelf => (t("打开一个文献库"), t("在首页点开一个文献库，或者新建一个：引导在文献库里接着走。")),
        Step::Library => (t("先建一个文献库"), t("一个研究主题一个文献库。在首页点「新建文献库」，起个名字就行，主题可以以后再写。")),
        Step::Papers => (t("往里放几篇文献"), t("选一种方式：手头有 PDF，直接拖进窗口；有 DOI 或 PMID，点「添加文献」粘贴；也可以从 Zotero 导入，或者让助手帮你检索。")),
        Step::Open => (t("点开一篇"), t("在文献列表里点一行，它的信息、PDF 和笔记在右边打开。")),
        Step::Read => (t("读它，问它"), t("「信息」是摘要、标签和笔记；「PDF」里读原文、划重点、看译文；「提问」就这一篇问助手，回答能点回原文。")),
        Step::Ask => (t("向整个文献库提问"), t("侧栏的「助手」读得到库里所有文献：找文献、做比较、起草综述，回答都带出处。")),
        Step::Path => (t("写综述时，顺着往下走"), t("侧栏这四项按做事的先后排：检索与导入、筛选、提取、写作。用到哪一步，再点开哪一步。")),
    }
}

impl Workspace {
    /// Laid over something a step may point at, it says where that thing was drawn. Nothing while no tour is on.
    /// Several things under one name are one place; what is scrolled out of sight is not pointed at.
    pub(crate) fn tour_spot(&self, name: &'static str) -> Option<Canvas<()>> {
        if !self.tour.on { return None }
        let spots = self.tour.spots.clone();
        let spot = canvas(
            move |bounds, window, _| {
                let seen = bounds.intersect(&window.content_mask().bounds);
                if seen.size.width > px(0.) && seen.size.height > px(0.) { spots.borrow_mut().entry(name).and_modify(|all| *all = all.union(&seen)).or_insert(seen); }
            },
            |_, _, _, _| {},
        );
        Some(spot.absolute().top_0().left_0().size_full())
    }

    /// What of the window the steps go by.
    fn tour_seen(&self) -> Seen {
        match self.lib.as_ref().filter(|lib| !lib.info.draft) {
            Some(lib) => Seen { library: true, papers: !lib.papers.is_empty(), selected: lib.selected.is_some() },
            None => Seen::default(),
        }
    }

    /// A first start begins the tour, where nothing is on the shelf yet or one begun was left unfinished. A scripted
    /// debug window begins it only by its own step (`tour`), and a settings file that could not be read begins none.
    pub(crate) fn tour_start(&mut self) {
        let libraries = self.libraries.iter().filter(|info| !info.draft).count();
        self.tour.on = wisp_core::settings::env("DEV").is_none() && self.toast.is_none() && tour::due(&self.settings, libraries);
    }

    /// After anything changed: what the person has done since moves the tour on, and how far it got is kept, so a
    /// window closed halfway takes it up again.
    pub(crate) fn tour_follow(&mut self, cx: &mut Context<Self>) {
        if !self.tour.on || self.away { return }
        let at = tour::reached(self.settings.tour_step, self.tour_seen());
        if at != self.settings.tour_step {
            self.settings.tour_step = at;
            self.save_settings(cx);
        }
    }

    pub(crate) fn tour_do(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            Do::Begin => {
                // Asked for in 设置: the steps are on the pages behind it.
                if self.settings_view.is_some() { self.close_settings(window, cx) }
                self.tour.on = true;
                self.settings.tour_done = false;
                self.settings.tour_step = tour::reached(1, self.tour_seen());
                self.save_settings(cx);
            }
            Do::Next if self.tour.on => {
                let at = tour::reached(self.settings.tour_step, self.tour_seen());
                if at >= Step::ALL.len() { return self.tour_end(t("引导走完了。想再看一遍，在「设置 › 关于」里。"), cx) }
                self.settings.tour_step = at + 1;
                self.save_settings(cx);
            }
            Do::End if self.tour.on => self.tour_end(t("引导已结束。想再看一遍，在「设置 › 关于」里。"), cx),
            Do::Next | Do::End => {}
        }
    }

    /// Over, finished or closed: it is not begun again by itself, and the last word says where it is found.
    fn tour_end(&mut self, said: &str, cx: &mut Context<Self>) {
        self.tour.on = false;
        self.settings.tour_done = true;
        self.settings.tour_step = 0;
        self.save_settings(cx);
        self.say(said.to_string(), cx);
    }

    /// The step on show: a ring round what it points at and its card beside that, over the page and under any
    /// dialog. Nothing while a dialog, a menu, the palette or 设置 has the window: the steps are of the pages.
    pub(crate) fn render_tour(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.tour.on { return None }
        // The places are said anew with every frame, as the page is drawn (before this layer, which comes after it).
        self.tour.spots.borrow_mut().clear();
        if self.settings_view.is_some() || self.modal.is_some() || self.menu.is_some() || self.palette.is_some() { return None }
        let seen = self.tour_seen();
        let step = tour::shown(tour::reached(self.settings.tour_step, seen), seen);
        let shelf = self.libraries.iter().any(|info| !info.draft);
        let (title, body) = words(step, shelf);
        let last = step.number() == Step::ALL.len();
        // The assistant, translation and screening all need a model: said where the assistant is shown, with the way there.
        let unconfigured = step == Step::Ask && self.settings.profile_for_or_default(Task::Chat).key().is_empty();
        let small = |button: Stateful<Div>| button.h(text_px(28.)).px(px(12.));
        let mut actions: Vec<AnyElement> = vec![];
        if unconfigured { actions.push(small(ui::secondary_button("tour-model", t("配置模型"))).on_click(self.on(Cmd::OpenSettings(SettingsPage::Ai), cx)).into_any_element()) }
        if last { actions.push(small(ui::secondary_button("tour-tutorials", t("图文教程"))).on_click(self.on(Cmd::Tutorial(crate::tutorial::Do::Open), cx)).into_any_element()) }
        if !step.done_by_doing() { actions.push(small(ui::primary_button("tour-next", if last { t("完成") } else { t("下一步") })).on_click(self.on(Cmd::Tour(Do::Next), cx)).into_any_element()) }
        let mut card = div()
            .occlude()
            .flex()
            .flex_col()
            .gap(px(6.))
            .w(text_px(CARD))
            .pl(px(16.))
            .pr(px(10.))
            .pt(px(10.))
            .pb(px(14.))
            .rounded(px(12.))
            .bg(Theme::bg_body())
            .border_1()
            .border_color(Theme::line())
            .shadow(theme::shadow_panel())
            // A press on the card is the card's: it does not end a selection or reach the page under it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(ui::caption(tf!("新手引导 · {} / {}", "Getting started · {} of {}", step.number(), Step::ALL.len())))
                    .child(ui::icon_action("tour-end", "x", Theme::text_tertiary(), t("结束引导")).on_click(self.on(Cmd::Tour(Do::End), cx))),
            )
            .child(div().pr(px(6.)).text_size(text_px(15.)).line_height(text_px(22.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().pr(px(6.)).text_color(Theme::text_secondary()).child(body));
        if unconfigured { card = card.child(div().pr(px(6.)).text_color(Theme::text_secondary()).child(t("助手、翻译和筛选都要先配置一个模型。"))) }
        // A step done by doing it has no button: the card says so, or the person looks for one.
        if step.done_by_doing() { card = card.child(div().pr(px(6.)).pt(px(2.)).text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(t("做完这一步，引导自己接着走。"))) }
        if !actions.is_empty() { card = card.child(div().flex().items_center().justify_end().gap(px(8.)).pr(px(6.)).pt(px(6.)).children(actions)) }
        let card = ui::fade_in(("tour-card", step.number()), card).into_any_element();
        let (spots, wanted) = (self.tour.spots.clone(), places(step, shelf));
        let (line, glow) = (Theme::accent(), Theme::accent().opacity(0.2));
        let layer = canvas(
            move |bounds, window, cx| {
                let mut card = card;
                let target = {
                    let spots = spots.borrow();
                    wanted.iter().find_map(|names| names.iter().filter_map(|name| spots.get(name).copied()).reduce(|all, one| all.union(&one)))
                };
                let ring = target.map(|target| target.dilate(px(RING)));
                let size = card.layout_as_root(AvailableSpace::min_size(), window, cx);
                let (card_size, room) = ((f32::from(size.width), f32::from(size.height)), (f32::from(bounds.size.width), f32::from(bounds.size.height)));
                let at = match ring {
                    Some(ring) => tour::place(tour::Rect { x: f32::from(ring.origin.x - bounds.origin.x), y: f32::from(ring.origin.y - bounds.origin.y), width: f32::from(ring.size.width), height: f32::from(ring.size.height) }, card_size, room, step.card_under()),
                    None => tour::docked(card_size, room),
                };
                card.prepaint_at(bounds.origin + point(px(at.0), px(at.1)), window, cx);
                (ring, card)
            },
            move |_, (ring, mut card), window, cx| {
                if let Some(ring) = ring {
                    window.paint_quad(quad(ring.dilate(px(3.)), px(RADIUS + 3.), transparent_black(), px(3.), glow, BorderStyle::Solid));
                    window.paint_quad(quad(ring, px(RADIUS), transparent_black(), px(2.), line, BorderStyle::Solid));
                }
                card.paint(window, cx);
            },
        );
        Some(layer.absolute().top_0().left_0().size_full().into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::{places, words};
    use crate::i18n::translate;
    use wisp_core::settings::Language;
    use wisp_core::tour::Step;

    #[test]
    fn every_card_reads_in_english() {
        for step in Step::ALL {
            for shelf in [false, true] {
                let (title, body) = words(step, shelf);
                assert_ne!(translate(Language::En, title), title, "{step:?}");
                assert_ne!(translate(Language::En, body), body, "{step:?}");
            }
        }
    }

    /// A name no element says would leave its step's card in the corner without a word of complaint.
    #[test]
    fn every_place_a_step_points_at_is_marked_in_the_window() {
        let drawn = [include_str!("home.rs"), include_str!("guide.rs"), include_str!("library.rs"), include_str!("detail.rs")].concat();
        for step in Step::ALL {
            for shelf in [false, true] {
                for name in places(step, shelf).iter().flat_map(|names| names.iter()) { assert!(drawn.contains(&format!("\"{name}\"")), "{name}") }
            }
        }
    }
}
