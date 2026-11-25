//! 铃铛: the bell on the home page. What a library left at work (`Workspace::parked`, `works.rs`) finished while
//! nobody was looking is said once in a toast and kept here, one row each, until the window closes: a run
//! through its papers that ended (筛选, 提取, 查找全文, a batch of translations, a paper's whole translation) or a
//! conversation that answered. `Workspace::in_library` notes them as it hands back the library it ran a call in.
//!
//! The bell is drawn once there is a row (`render_bell`), with a dot while some row is new. Opening the list makes
//! every row seen; the rows that were new stand out while it is open. Opening a library by any way makes its rows
//! seen too: the person is where they lead. A click on a row opens that library where the result is
//! (`bell_go`): the page of the run, the conversation that answered.

use crate::app::{Cmd, Page, Tab, Talk, Workspace};
use crate::i18n::t;
use crate::menus::{MenuSide, menu_place};
use crate::theme::{Theme, text_px};
use crate::ui;
use crate::works::Kind;
use gpui::{prelude::FluentBuilder, *};
use wisp_core::time;

/// The rows kept: older ones go.
const KEEP: usize = 30;
/// The list's width, and how tall a row of it is (two lines) before the text's scale.
const LIST_WIDTH: f32 = 340.;
const ROW_HEIGHT: f32 = 50.;

/// What a click on the bell and its rows does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Do {
    Open,
    Close,
    /// The row with this id: to its library, where the result is.
    Go(u64),
}

/// Where a row leads in its library.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Spot {
    /// A run's page; for a paper's whole translation, the paper (`paper`, empty for the others).
    Run { kind: Kind, paper: String },
    /// A conversation that answered: in which place, its id, and for one with a paper, the paper.
    Answer { talk: Talk, chat: String, paper: String },
}

/// One thing that ended off screen.
#[derive(Clone, Debug)]
pub(crate) struct Done {
    pub id: u64,
    pub library: String,
    pub name: String,
    pub spot: Spot,
    /// What came of it, without the library's name: 「筛选做完了：80 篇」.
    pub said: String,
    /// It stopped on an error: said in the danger colour.
    pub error: bool,
    /// It went through to its end (a run done to its total; any answer).
    pub whole: bool,
    pub at: String,
    pub seen: bool,
}

#[derive(Default)]
pub(crate) struct Bell {
    /// Newest first.
    pub done: Vec<Done>,
    pub open: bool,
    /// The rows that were new when the list was opened: they stand out while it is.
    fresh: Vec<u64>,
    /// Where the bell was last drawn: the list hangs from it.
    at: Option<Bounds<Pixels>>,
    next: u64,
}

impl Bell {
    pub(crate) fn unseen(&self) -> usize {
        self.done.iter().filter(|done| !done.seen).count()
    }

    fn push(&mut self, mut done: Done) {
        self.next += 1;
        done.id = self.next;
        self.done.insert(0, done);
        self.done.truncate(KEEP);
    }

    /// The library was opened: what its rows lead to is in front of the person.
    pub(crate) fn seen_library(&mut self, id: &str) {
        for done in self.done.iter_mut().filter(|done| done.library == id) { done.seen = true }
    }

    /// The library was removed, or the data folder changed (`None`): its rows lead nowhere.
    pub(crate) fn forget(&mut self, id: Option<&str>) {
        self.done.retain(|done| id.is_some_and(|id| done.library != id));
        if self.done.is_empty() { self.open = false }
    }
}

/// A conversation answering before a call of `in_library`: found again after it by `key`.
pub(crate) struct Asking {
    talk: Talk,
    key: u64,
    id: String,
    paper: String,
    title: String,
}

impl Workspace {
    /// The conversations of the open library answering now, in their places and set aside.
    pub(crate) fn answers_under_way(&self) -> Vec<Asking> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        let placed = [Talk::Library, Talk::Paper, Talk::Write].into_iter().map(|talk| (talk, lib.chat(talk)));
        let aside = lib.aside.iter().map(|chat| (Talk::Library, chat));
        placed
            .chain(aside)
            .filter(|(_, chat)| chat.busy)
            .map(|(talk, chat)| Asking {
                talk,
                key: chat.key,
                id: chat.id.clone(),
                paper: if talk == Talk::Paper { lib.talk_about(talk).unwrap_or_default() } else { String::new() },
                title: ui::one_line(&chat.transcript().title_by(|text| self.mention_plain(crate::mention::Place::Chat(talk), text))),
            })
            .collect()
    }

    /// After a call of `in_library`, with the library still in `lib`'s place: what of `asked` and `running` has ended.
    pub(crate) fn note_ended(&mut self, asked: Vec<Asking>, running: &[Kind]) {
        let Some(lib) = self.lib.as_ref() else { return };
        let (library, name, at) = (lib.info.id.clone(), lib.info.name.clone(), time::now_iso());
        let mut ended = vec![];
        for asking in asked {
            let now = [Talk::Library, Talk::Paper, Talk::Write].into_iter().map(|talk| lib.chat(talk)).chain(lib.aside.iter()).find(|chat| chat.key == asking.key);
            if now.is_some_and(|chat| chat.busy) { continue }
            // Its id once it was kept; one let go from where it was set aside keeps the one it had.
            let chat = now.map(|chat| chat.id.clone()).filter(|id| !id.is_empty()).unwrap_or(asking.id);
            let title = asking.title;
            let said = if title.is_empty() { t("助手答完了").to_string() } else { tf!("助手答完了：「{title}」", "The assistant answered: “{title}”") };
            ended.push((Spot::Answer { talk: asking.talk, chat, paper: asking.paper }, said, false, true));
        }
        for work in lib.works().into_iter().filter(|work| running.contains(&work.kind) && !work.running) {
            // A paper's whole translation leads to that paper: the one on show, or one kept beside it.
            let paper = if work.kind == Kind::Facing {
                lib.pins.iter().find(|pin| pin.facing.as_ref().is_some_and(|facing| facing.file == lib.facing_for)).map(|pin| pin.paper.clone()).or_else(|| lib.selected.clone()).unwrap_or_default()
            } else {
                String::new()
            };
            ended.push((Spot::Run { kind: work.kind, paper }, work.outcome(), work.done < work.total && !work.error.is_empty(), work.done >= work.total));
        }
        for (spot, said, error, whole) in ended {
            self.bell.push(Done { id: 0, library: library.clone(), name: name.clone(), spot, said, error, whole, at: at.clone(), seen: false });
        }
    }

    pub(crate) fn bell_do(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            Do::Open => {
                let bell = &mut self.bell;
                bell.fresh = bell.done.iter().filter(|done| !done.seen).map(|done| done.id).collect();
                for done in &mut bell.done { done.seen = true }
                bell.open = !bell.done.is_empty();
            }
            Do::Close => self.bell.open = false,
            Do::Go(id) => self.bell_go(id, window, cx),
        }
        cx.notify();
    }

    /// To the library of the row `id`, where what ended there is: as a click on its card, then the run's page or the
    /// conversation, the way the person would go there themselves.
    fn bell_go(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.bell.open = false;
        let Some(done) = self.bell.done.iter().find(|done| done.id == id).cloned() else { return };
        if !self.libraries.iter().any(|info| info.id == done.library) {
            self.bell.forget(Some(&done.library));
            return self.say(tf!("「{}」已经不在首页上了", "“{}” is no longer on the home page", done.name), cx);
        }
        self.run(Cmd::OpenLibrary(done.library.clone()), window, cx);
        let Some(lib) = self.lib.as_ref().filter(|lib| lib.info.id == done.library) else { return };
        let known = |paper: &str| !paper.is_empty() && lib.papers.iter().any(|p| p.id == paper);
        match done.spot {
            Spot::Run { kind: Kind::Screen, .. } => self.run(Cmd::SetPage(Page::Screen), window, cx),
            Spot::Run { kind: Kind::Extract, .. } => self.run(Cmd::SetPage(Page::Extract), window, cx),
            // What each paper came to, while the library still holds the run (taken up as it stood); else the papers, with the files found.
            Spot::Run { kind: Kind::Fetch, .. } => {
                let report = lib.fetching.is_some();
                self.run(Cmd::SetPage(Page::Papers), window, cx);
                if report { self.run(Cmd::ShowFetch, window, cx) }
            }
            Spot::Run { kind: Kind::Translate, .. } => self.run(Cmd::SetPage(Page::Papers), window, cx),
            Spot::Run { kind: Kind::Facing, paper } => {
                let shown = lib.facing;
                if !known(&paper) { return }
                if lib.page != Page::Papers { self.run(Cmd::SetPage(Page::Papers), window, cx) }
                self.run(Cmd::Select(paper), window, cx);
                self.run(Cmd::SetTab(Tab::Pdf), window, cx);
                // The translation beside the pages once it is whole: turned on for one cut short, it would go on translating.
                if !shown && done.whole { self.toggle_facing(cx) }
            }
            Spot::Answer { talk: Talk::Paper, chat, paper } => {
                if !known(&paper) { return }
                if lib.page == Page::Ask { self.run(Cmd::SetPage(Page::Papers), window, cx) }
                self.run(Cmd::Select(paper), window, cx);
                self.run(Cmd::SetTab(Tab::Ask), window, cx);
                if self.lib.as_ref().is_some_and(|lib| lib.paper_chat.id != chat) && !chat.is_empty() { self.run(Cmd::OpenChat(Talk::Paper, chat), window, cx) }
            }
            Spot::Answer { talk: Talk::Write, .. } => self.run(Cmd::SetPage(Page::Write), window, cx),
            Spot::Answer { talk: Talk::Library, chat, .. } => {
                if self.asking(window) { self.run(Cmd::LeaveAsk, window, cx) }
                self.run(Cmd::SetPage(Page::Ask), window, cx);
                if self.lib.as_ref().is_some_and(|lib| lib.library_chat.id != chat) && !chat.is_empty() { self.run(Cmd::OpenChat(Talk::Library, chat), window, cx) }
            }
        }
    }

    fn bell_measured(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.bell.at == Some(bounds) { return }
        self.bell.at = Some(bounds);
        if self.bell.open { cx.notify() }
    }

    /// The bell in the home page's head, once something ended off screen: a dot on it while a row is new.
    pub(crate) fn render_bell(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.bell.done.is_empty() { return None }
        let unseen = self.bell.unseen();
        let entity = cx.entity();
        let measure = canvas(|_, _, _| (), move |bounds, _, window, cx| {
            window.defer(cx, move |_, cx| entity.update(cx, |this, cx| this.bell_measured(bounds, cx)));
        })
        .absolute()
        .size_full();
        let tip = if unseen > 0 { tf!("后台做完的事：{unseen} 件新的", "Done in the background: {unseen} new") } else { t("后台做完的事").to_string() };
        Some(
            // Apart from the two ways of laying out the libraries beside it, which are one control.
            ui::ghost_button("home-bell", Some("bell"), None)
                .relative()
                .mr(px(8.))
                .when(self.bell.open, |d| d.bg(Theme::pressed()))
                .tooltip(ui::tip(tip))
                .on_click(self.on(Cmd::Bell(Do::Open), cx))
                .child(measure)
                .when(unseen > 0, |d| d.child(div().absolute().top(px(4.)).right(px(4.)).size(px(8.)).rounded_full().border_1().border_color(Theme::bg_window()).bg(Theme::accent())))
                .into_any_element(),
        )
    }

    /// The list under the bell: a row for each thing that ended, its library first, then when; what came of it under.
    pub(crate) fn render_bell_list(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bell = &self.bell;
        if !bell.open || self.lib.is_some() || self.settings_view.is_some() { return None }
        let room = LIST_WIDTH - 8. - 16.;
        let rows: Vec<AnyElement> = bell.done.iter().enumerate().map(|(n, done)| {
            let fresh = bell.fresh.contains(&done.id);
            let when = if matches!(time::ago_today(&done.at), time::Ago::Today) { time::clock_time(&done.at) } else { time::short_time(&done.at) };
            let said = ui::one_line(&done.said);
            // Cut short, it is there whole on hover.
            let cut = ui::text_width(&said, text_px(12.), window) > room || ui::text_width(&done.name, text_px(13.), window) > room - 48.;
            div()
                .id(("bell-row", n))
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(px(8.))
                .py(px(7.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(Theme::hover()))
                .when(cut, |d| d.tooltip(ui::tip(format!("{}\n{}", done.name, done.said))))
                .on_click(self.on(Cmd::Bell(Do::Go(done.id)), cx))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_1().min_w_0().truncate().when(fresh, |d| d.font_weight(FontWeight::SEMIBOLD)).text_color(if fresh { Theme::text() } else { Theme::text_secondary() }).child(ui::one_line(&done.name)))
                        .child(div().flex_none().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(when)),
                )
                .child(div().truncate().text_size(text_px(12.)).line_height(text_px(17.)).text_color(if done.error { Theme::danger() } else { Theme::text_secondary() }).child(said))
                .into_any_element()
        }).collect();
        let height = (text_px(ROW_HEIGHT) * rows.len() as f32 + text_px(26.) + px(10.)).min(px(520.));
        let viewport = window.viewport_size();
        let from = bell.at.unwrap_or(Bounds { origin: point(viewport.width - px(LIST_WIDTH + 48.), px(40.)), size: size(px(28.), px(28.)) });
        let (corner, at) = menu_place(from, MenuSide::Below, height, viewport.height);
        let close = |this: &mut Self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>| this.bell_do(Do::Close, window, cx);
        Some(
            div()
                .id("bell-layer")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(close))
                .on_mouse_down(MouseButton::Right, cx.listener(close))
                .child(
                    anchored().anchor(corner).position(at).snap_to_window_with_margin(px(8.)).child(
                        ui::popover()
                            .id("bell-list")
                            .w(px(LIST_WIDTH))
                            .max_w(px(LIST_WIDTH))
                            .max_h(px(520.))
                            .overflow_y_scroll()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                            .child(div().px(px(8.)).pt(px(4.)).pb(px(2.)).text_size(text_px(11.)).line_height(text_px(16.)).text_color(Theme::text_tertiary()).child(t("后台做完的事")))
                            .children(rows),
                    ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's own `test` attribute.
    use super::{Bell, Done, Spot};
    use crate::works::Kind;

    fn done(library: &str) -> Done {
        Done { id: 0, library: library.into(), name: library.into(), spot: Spot::Run { kind: Kind::Screen, paper: String::new() }, said: String::new(), error: false, whole: true, at: String::new(), seen: false }
    }

    #[test]
    fn rows_are_new_until_seen_and_go_with_their_library() {
        let mut bell = Bell::default();
        for _ in 0..super::KEEP { bell.push(done("a")) }
        bell.push(done("b"));
        // Newest first, the oldest let go, every id its own.
        assert_eq!(bell.done.len(), super::KEEP);
        assert_eq!(bell.done[0].library, "b");
        assert_eq!(bell.done[0].id, super::KEEP as u64 + 1);
        assert_eq!(bell.unseen(), super::KEEP);
        bell.seen_library("b");
        assert_eq!(bell.unseen(), super::KEEP - 1);
        bell.forget(Some("a"));
        assert_eq!(bell.done.len(), 1);
        bell.forget(None);
        assert!(bell.done.is_empty());
    }
}
