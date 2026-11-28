//! 写作 from the home page: the manuscripts of every library, and those in none, in one place; the pen beside the
//! home page's star opens it. A manuscript need not wait for a library: 新稿件 here begins one that is in none
//! (未绑定, a conversation of the home page marked `writing`: `libraries::create_writing`), written in its own
//! window with the assistant beside it, which looks things up as the writing goes. What it gathers becomes a
//! library afterwards (建成文献库), or the manuscript is bound to one there already (绑定到文献库,
//! `Store::take_all`): the manuscript, its conversations and the papers kept with it go into that library, and it
//! is written on in that library's 写作, where every paper of the library can be cited. Nothing is brought to this
//! page: a click on a row goes to where the manuscript is kept, and the way back from there is to this page.

use crate::app::{Cmd, Modal, Page, Workspace};
use crate::i18n::t;
use crate::input::{InputEvent, TextInput};
use crate::theme::{self, Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::ops::Range;
use std::rc::Rc;
use wisp_core::libraries::{self, LibraryInfo};
use wisp_core::store::Store;
use wisp_core::time;
use wisp_core::written::{self, Held};

pub use wisp_core::written::Place;

/// How wide the list down the left is: 收藏's, the tutorials' and the settings'.
const SIDEBAR_W: f32 = 256.;
/// The height of a manuscript's row.
const ROW_H: f32 = 56.;
/// The widest the name of a row's library is written.
const OWNER_W: f32 = 200.;

/// What the page can be told.
#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// The pen on the home page.
    Open,
    /// ← 返回, or Esc.
    Close,
    /// All the manuscripts, those in no library, or one library's.
    Show(Place),
    /// A row: to the library or the conversation that holds it, at this manuscript.
    Go(String, String),
    /// 新稿件: in the library looked at alone, else in none.
    New,
    /// 绑定到文献库, of the manuscripts in no library that this conversation holds: the dialog that picks the library.
    AskBind(String),
    /// A library picked in that dialog.
    ChooseBind(String),
    /// The dialog's 绑定: what the conversation holds goes into the library.
    Bind(String, String),
}

/// The page as the window holds it while it is open, and under whatever was gone to from it.
pub struct Written {
    /// What each library and each conversation holds, the libraries in the shelf's order first, and every manuscript
    /// across them, the latest written first.
    held: Vec<Held>,
    order: Vec<(usize, usize)>,
    /// Those that could not be read, by name.
    failed: Vec<String>,
    /// Read at least once: before that the page says nothing of what there is.
    loaded: bool,
    /// Counts the readings: one that comes back after a later one began is dropped.
    run: u64,
    place: Place,
    search: Entity<TextInput>,
    _typed: Subscription,
    scroll: UniformListScrollHandle,
    side: ScrollHandle,
}

impl Workspace {
    pub(crate) fn written_do(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            Do::Open => {
                self.bell.open = false;
                // One page over the home page at a time.
                if self.collected.is_some() { self.run(Cmd::Collected(crate::collected::Do::Close), window, cx) }
                if self.written.is_none() {
                    let search = cx.new(|cx| TextInput::new("搜索题目和正文…", cx));
                    let typed = cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
                        if *event != InputEvent::Changed { return }
                        // Another set of rows: from its top.
                        if let Some(view) = this.written.as_mut() { view.scroll = UniformListScrollHandle::new() }
                        cx.notify();
                    });
                    self.written = Some(Written { held: vec![], order: vec![], failed: vec![], loaded: false, run: 0, place: Place::All, search, _typed: typed, scroll: UniformListScrollHandle::new(), side: ScrollHandle::new() });
                }
                self.written_read(cx);
                window.focus(&self.focus);
            }
            Do::Close => {
                self.written = None;
                window.focus(&self.focus);
            }
            Do::Show(place) => {
                let Some(view) = self.written.as_mut() else { return };
                if view.place != place { (view.place, view.scroll) = (place, UniformListScrollHandle::new()) }
            }
            Do::Go(holder, manuscript) => self.written_go(holder, manuscript, window, cx),
            Do::New => {
                let library = self.written.as_ref().and_then(|view| match &view.place {
                    Place::Library(id) => Some(id.clone()),
                    _ => None,
                });
                match library {
                    Some(id) => {
                        self.run(Cmd::OpenLibrary(id.clone()), window, cx);
                        if self.lib.as_ref().is_none_or(|lib| lib.info.id != id) { return }
                        self.run(Cmd::SetPage(Page::Write), window, cx);
                        // The empty manuscript 写作 opens a library with is the new one; beside any other, one is made.
                        let blank = self.lib.as_ref().is_some_and(|lib| lib.store.manuscript_blank(&lib.writing.manuscript).unwrap_or(false));
                        if !blank { self.run(Cmd::NewManuscript, window, cx) }
                    }
                    None => self.write_unbound(String::new(), window, cx),
                }
            }
            Do::AskBind(holder) => {
                if libraries::shelf(&self.libraries, &self.settings.library_order).is_empty() { return self.fail(t("还没有文献库可以绑定。先把它建成文献库，或者新建一个。"), cx) }
                self.modal_error.clear();
                self.modal = Some(Modal::Bind { from: holder, to: None });
            }
            Do::ChooseBind(id) => {
                if let Some(Modal::Bind { to, .. }) = self.modal.as_mut() { *to = Some(id) }
                self.modal_error.clear();
            }
            Do::Bind(from, to) => self.bind(from, to, window, cx),
        }
        cx.notify();
    }

    /// A manuscript in no library, begun now: its conversation is made, and its window opens on the manuscript, the
    /// title first. `asked` is what the home page's box held: it waits in the conversation's box, not sent.
    pub(crate) fn write_unbound(&mut self, asked: String, window: &mut Window, cx: &mut Context<Self>) {
        let language = if !self.settings.write_language.is_empty() {
            self.settings.default_write_language()
        } else if crate::i18n::is_english() {
            "en"
        } else {
            "zh"
        };
        let (info, manuscript) = match libraries::create_writing(&self.root, t(wisp_core::manuscript::UNTITLED), language) {
            Ok(made) => made,
            Err(err) => return self.fail(format!("{err:#}"), cx),
        };
        self.libraries = libraries::list(&self.root);
        self.home.listing = false;
        self.open_library(&info.id, window, cx);
        let Some(lib) = self.lib.as_mut().filter(|lib| lib.info.id == info.id) else { return };
        if !asked.trim().is_empty() { lib.library_chat.input.update(cx, |input, cx| input.set_text(asked.trim(), cx)) }
        self.run(Cmd::WriteManuscript(manuscript), window, cx);
        if let Some(lib) = self.lib.as_ref() { lib.writing.title.read(cx).focus(window) }
        cx.notify();
    }

    /// To a manuscript, where it is kept: a library's 写作, or the window of the conversation that holds it.
    fn written_go(&mut self, holder: String, manuscript: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(info) = self.libraries.iter().find(|info| info.id == holder).cloned() else { return };
        self.home.listing = false;
        self.run(Cmd::OpenLibrary(holder.clone()), window, cx);
        // Not opened (its folder is gone, its database is a newer build's): said by the opening.
        if self.lib.as_ref().is_none_or(|lib| lib.info.id != holder) { return }
        if !info.draft { self.run(Cmd::SetPage(Page::Write), window, cx) }
        if self.lib.as_ref().is_some_and(|lib| lib.writing.manuscripts.iter().any(|m| m.id == manuscript)) { self.run(Cmd::WriteManuscript(manuscript), window, cx) }
        if let Some(lib) = self.lib.as_ref() { lib.writing.editor.read(cx).focus(window) }
    }

    /// A manuscript begun on the home page is called what its manuscript is: the title typed is the name its
    /// conversation is listed under, until it has one of its own (建成文献库 names the library).
    pub(crate) fn writing_named(&mut self, title: &str, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut().filter(|lib| lib.info.draft && lib.info.writing) else { return };
        let name: String = title.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(60).collect();
        let name = if name.is_empty() { t(wisp_core::manuscript::UNTITLED).to_string() } else { name };
        if lib.info.name == name { return }
        lib.info.name = name.clone();
        if libraries::save(&lib.info).is_err() { return }
        if let Some(known) = self.libraries.iter_mut().find(|info| info.id == lib.info.id) { known.name = name.clone() }
        // The window is in use while its field says what was typed: named once that is done.
        let window = self.main_window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, _| window.set_window_title(&format!("{name} — Wisp Study")));
        });
    }

    /// A manuscript begun on the home page and left with nothing in it (no title, no word, nothing asked, no
    /// paper) is not kept: its folder goes the way a removed conversation's goes. Called when its window is left.
    pub(crate) fn retire_blank_writing(&mut self) -> Option<LibraryInfo> {
        let lib = self.lib.as_ref().filter(|lib| lib.info.draft && lib.info.writing && !lib.at_work())?;
        let unwritten = lib.writing.manuscripts.iter().all(|m| m.title.trim().is_empty() && m.sections.iter().all(|s| s.body.trim().is_empty()));
        let unsaid = lib.library_chat.messages.is_empty() && lib.store.list_all_conversations().is_ok_and(|kept| kept.is_empty()) && lib.writing.manuscripts.iter().all(|m| lib.store.list_conversations(&m.id).is_ok_and(|kept| kept.is_empty()));
        let bare = lib.papers.is_empty() && lib.writing.manuscripts.iter().all(|m| lib.store.comments(&m.id).is_ok_and(|kept| kept.is_empty()));
        (unwritten && unsaid && bare).then(|| lib.info.clone())
    }

    /// The libraries a conversation of the home page can be bound to: those on the shelf, in its order.
    fn bind_targets(&self) -> Vec<LibraryInfo> {
        libraries::shelf(&self.libraries, &self.settings.library_order).into_iter().cloned().collect()
    }

    /// 绑定到文献库: everything the conversation `from` holds goes into the library `to` (`Store::take_all`), its
    /// folder goes to `removed/` as a removed conversation's does, and the window opens that library's 写作 on the
    /// manuscript it brought.
    fn bind(&mut self, from: String, to: String, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(source), Some(target)) = (self.libraries.iter().find(|info| info.id == from && info.draft).cloned(), self.libraries.iter().find(|info| info.id == to && !info.draft).cloned()) else { return };
        // One that is answering goes on where it is: what it writes next would be written into a folder put away.
        let on_show = self.lib.as_ref().is_some_and(|lib| lib.info.id == from);
        if self.parked.iter().any(|lib| lib.info.id == from) || (on_show && self.lib.as_ref().is_some_and(|lib| lib.at_work())) { return self.modal_error = t("它还在回答，结束后再绑定。").into() }
        if on_show { self.write_snapshot("leave") }
        let taken = match self.lib.as_mut().filter(|lib| lib.info.id == from) {
            Some(lib) => Store::open(&target.dir).and_then(|mut store| store.take_all(&mut lib.store)),
            None => Store::open(&source.dir).and_then(|mut theirs| Store::open(&target.dir)?.take_all(&mut theirs)),
        };
        let taken = match taken {
            Ok(taken) => taken,
            Err(err) => return self.modal_error = format!("{err:#}"),
        };
        self.modal = None;
        if on_show { self.close_library(cx) }
        if let Err(err) = libraries::remove(&self.root, &source) { self.fail(format!("{err:#}"), cx) }
        self.libraries = libraries::list(&self.root);
        self.home.listing = false;
        self.open_library(&to, window, cx);
        if self.lib.as_ref().is_none_or(|lib| lib.info.id != to) { return self.written_read(cx) }
        if let Some(manuscript) = taken.manuscripts.first() {
            self.run(Cmd::SetPage(Page::Write), window, cx);
            self.run(Cmd::WriteManuscript(manuscript.clone()), window, cx);
        }
        let mut said = tf!("已绑定到「{}」。", "Bound to “{}”.", target.name);
        if taken.papers > 0 {
            said += &if taken.joined > 0 { tn!("带来 {n} 篇文献，其中 {} 篇这里已有，并到了原来那条上。", " {n} came along; {} of them were here already and were joined.", taken.papers, "paper", taken.joined) } else { tn!("带来 {n} 篇文献。", " {n} came along.", taken.papers, "paper") };
        }
        if !taken.rekeyed.is_empty() {
            let keys: Vec<String> = taken.rekeyed.iter().map(|(before, now)| format!("@{before} → @{now}")).collect();
            said += &tf!("这里别的文献已经用了同样的引用键，稿件里改写为：{}。", " Another paper here already had the same citation key; the manuscript now writes: {}.", keys.join(if crate::i18n::is_english() { ", " } else { "、" }));
        }
        if !taken.fields_left.is_empty() { said += &tf!("这里没有词条「{}」，它的内容没有带过来。", " This library has no field “{}”; its values were not brought.", taken.fields_left.join("」「")) }
        self.say(said, cx);
    }

    pub(crate) fn render_bind_dialog(&self, cx: &mut Context<Self>) -> Option<Div> {
        let Some(Modal::Bind { from, to }) = &self.modal else { return None };
        let source = self.libraries.iter().find(|info| info.id == *from)?;
        let rows = self.bind_targets().into_iter().enumerate().map(|(i, info)| {
            let chosen = to.as_deref() == Some(info.id.as_str());
            let accent = theme::accent_of(&info.color);
            div()
                .id(("bind-to", i))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(chosen, |d| d.bg(accent.soft()))
                .when(!chosen, |d| d.hover(|s| s.bg(Theme::hover())))
                .child(div().flex_none().size(px(10.)).rounded_full().bg(accent.strong()))
                .child(div().flex_1().min_w_0().font_weight(if chosen { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }).child(ui::one_line(&info.name)))
                .child(div().flex_none().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(tn!("{n} 篇", "{n}", info.paper_count, "paper")))
                .on_click(self.on(Cmd::Written(Do::ChooseBind(info.id.clone())), cx))
        });
        let note = if source.paper_count > 0 {
            tn!("稿件、对话和这里收进的 {n} 篇文献都放进选中的文献库，库里已有的不会重复。之后在那个文献库的「写作」里接着写，库里的文献都可以引用。绑定后不能解除。", "The manuscript, the conversations and the {n} kept here go into the library you pick; one it already holds is not doubled. You write on in that library’s Writing, where all its papers can be cited. Binding cannot be undone.", source.paper_count, "paper")
        } else {
            t("稿件和对话都放进选中的文献库。之后在那个文献库的「写作」里接着写，库里的文献都可以引用。绑定后不能解除。").to_string()
        };
        Some(
            ui::dialog(460.)
                .child(ui::dialog_title(t("绑定到文献库")))
                .child(
                    div().id("modal-body").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().gap(px(10.)).px(px(20.)).py(px(8.))
                        .child(div().font_weight(FontWeight::MEDIUM).line_height(text_px(21.)).child(ui::one_line(&source.name)))
                        .child(div().text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_tertiary()).child(note))
                        .child(div().flex().flex_col().gap(px(2.)).children(rows))
                        .child(ui::error_line(&self.modal_error)),
                )
                .child(
                    div().flex().flex_none().justify_end().gap(px(8.)).px(px(20.)).py(px(14.))
                        .child(ui::secondary_button("modal-cancel", t("取消")).on_click(self.on(Cmd::CloseModal, cx)))
                        .child(ui::primary_button("modal-ok", t("绑定").to_string()).when(to.is_none(), |d| d.opacity(0.4)).when(to.is_some(), |d| d.on_click(self.on(Cmd::SubmitModal, cx)))),
                ),
        )
    }

    /// Read what the libraries and the conversations hold, off the main thread: when the page opens, and when what
    /// was gone to from it is left. What was read before stays on show until the new reading is in.
    pub(crate) fn written_read(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.written.as_mut() else { return };
        view.run += 1;
        let run = view.run;
        let holders: Vec<LibraryInfo> = libraries::shelf(&self.libraries, &self.settings.library_order).into_iter().chain(self.libraries.iter().filter(|info| info.draft)).cloned().collect();
        let work = cx.background_executor().spawn(async move {
            let (mut held, mut failed) = (vec![], vec![]);
            for info in &holders {
                match written::read(info) {
                    Ok(one) => held.push(one),
                    Err(_) => failed.push(info.name.clone()),
                }
            }
            let order = written::merged(&held);
            (held, order, failed)
        });
        cx.spawn(async move |this, cx| {
            let (held, order, failed) = work.await;
            this.update(cx, |this, cx| {
                let Some(view) = this.written.as_mut().filter(|view| view.run == run) else { return };
                (view.held, view.order, view.failed, view.loaded) = (held, order, failed, true);
                // The place looked at alone holds nothing any more: all of them.
                if view.place != Place::All && written::count(&view.held, &view.place) == 0 { view.place = Place::All }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// What the debug step `written` stands for (`dev.rs`): the page opened or closed, a place shown, the n-th row
    /// on show gone to, 新稿件, the dialog that binds the n-th row's conversation, the n-th library picked in it.
    #[cfg(debug_assertions)]
    pub(crate) fn written_step(&self, arg: &str, cx: &App) -> Option<Cmd> {
        let (what, n) = arg.split_once(':').map_or((arg, None), |(what, n)| (what, n.parse::<usize>().ok()));
        let view = self.written.as_ref();
        let row = |n: Option<usize>| {
            let view = view?;
            let words = wisp_core::figure::search_words(view.search.read(cx).text());
            let &(of, at) = written::shown(&view.held, &view.order, &view.place, &words).get(n.unwrap_or(0))?;
            let one = view.held.get(of)?;
            Some((one.library.clone(), one.pieces.get(at)?.id.clone(), one.bound))
        };
        Some(Cmd::Written(match what {
            "" => Do::Open,
            "close" => Do::Close,
            "new" => Do::New,
            "all" => Do::Show(Place::All),
            "unbound" => Do::Show(Place::Unbound),
            "library" => Do::Show(Place::Library(view?.held.iter().filter(|one| one.bound && !one.pieces.is_empty()).nth(n.unwrap_or(0))?.library.clone())),
            "go" => row(n).map(|(holder, manuscript, _)| Do::Go(holder, manuscript))?,
            "bind" => match row(n) {
                Some((holder, _, false)) => Do::AskBind(holder),
                // No page open: the conversation on show.
                _ => Do::AskBind(self.lib.as_ref().filter(|lib| lib.info.draft)?.info.id.clone()),
            },
            "to" => Do::ChooseBind(self.bind_targets().get(n.unwrap_or(0))?.id.clone()),
            _ => return None,
        }))
    }

    /// Esc on the page, one layer at a time (design.md rule 53): the search field gives the keyboard back, then
    /// what is typed in it goes, then the page.
    pub(crate) fn written_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.written.as_mut() else { return };
        if view.search.focus_handle(cx).is_focused(window) {
            window.focus(&self.focus);
        } else if !view.search.read(cx).text().is_empty() {
            view.search.update(cx, |input, cx| input.set_text("", cx));
            view.scroll = UniformListScrollHandle::new();
        } else {
            self.run(Cmd::Written(Do::Close), window, cx);
        }
        cx.notify();
    }

    /// The search key on the page: its field.
    pub(crate) fn written_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.written.as_ref() { view.search.read(cx).focus(window) }
        cx.notify();
    }

    pub(crate) fn render_written(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.written.as_ref() else { return div().into_any_element() };
        let width = (f32::from(window.viewport_size().width) - SIDEBAR_W - 1.).max(240.);
        let query = view.search.read(cx).text().to_string();
        let words = wisp_core::figure::search_words(&query);
        let shown = Rc::new(written::shown(&view.held, &view.order, &view.place, &words));
        let (total, count) = (written::count(&view.held, &view.place), shown.len());
        // Where a row's manuscript is kept is written only while several holders are listed together.
        let across = view.place == Place::All;
        let sidebar = self.written_sidebar(view, cx);
        let focused = view.search.focus_handle(cx).is_focused(window);
        let search = div().flex().flex_1().items_center().gap(px(8.)).h(text_px(34.)).px(px(10.)).rounded(px(8.)).bg(Theme::bg_sunken()).border_1().border_color(if focused { Theme::accent() } else { Theme::bg_sunken() }).child(ui::icon("search", 15., Theme::text_tertiary())).child(view.search.clone());
        // 新稿件 says where the new one will be: in the library looked at alone, else in none.
        let into = match &view.place {
            Place::Library(id) => view.held.iter().find(|one| one.library == *id).map(|one| one.name.clone()),
            _ => None,
        };
        let new_tip = match &into {
            Some(name) => tf!("在「{}」里新建一篇稿件", "A new manuscript in “{}”", name),
            None => t("新建一篇稿件：不用先有文献库，之后可以绑定").to_string(),
        };
        let new = ui::primary_button("written-new", t("新稿件")).h(text_px(34.)).tooltip(ui::tip(new_tip)).on_click(self.on(Cmd::Written(Do::New), cx));

        let body = if !view.loaded {
            div().flex_1().into_any_element()
        } else if count == 0 && query.trim().is_empty() {
            ui::empty_state("pen-line", t("还没有稿件"), t("点「新稿件」直接开始写，不用先有文献库：边写边让助手查文献，之后可以把它绑定到已有的文献库，或者建成一个新的。")).into_any_element()
        } else if count == 0 {
            ui::empty_state("search", t("没有匹配的稿件"), if across { t("换一个搜索词试试。") } else { t("换一个搜索词，或者在左边选「全部稿件」。") }).into_any_element()
        } else {
            let shown = shown.clone();
            uniform_list("written-rows", count, cx.processor(move |this, range: Range<usize>, window, cx| range.filter_map(|at| shown.get(at).copied()).map(|(of, at)| this.written_row(of, at, across, width, window, cx)).collect::<Vec<_>>())).track_scroll(view.scroll.clone()).flex_1().into_any_element()
        };

        let unbound = shown.iter().filter(|&&(of, _)| view.held.get(of).is_some_and(|one| !one.bound)).count();
        let status = if !view.loaded || total == 0 {
            String::new()
        } else if count != total {
            tf!("显示 {count} / {total}", "Showing {count} / {total}")
        } else if across && unbound > 0 {
            tf!("共 {total} 篇，{unbound} 篇未绑定文献库", "{total} manuscripts, {unbound} in no library")
        } else {
            tf!("共 {total} 篇", "{total} manuscripts")
        };
        let failed = (!view.failed.is_empty()).then(|| tf!("没有读出来的文献库：{}", "Libraries that could not be read: {}", view.failed.join(if crate::i18n::is_english() { ", " } else { "、" })));
        let foot = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(text_px(30.))
            .px(px(16.))
            .border_t_1()
            .border_color(Theme::line())
            .text_size(text_px(12.))
            .text_color(Theme::text_tertiary())
            .gap(px(12.))
            .child(div().flex_none().child(status))
            .child(match failed {
                Some(failed) => div().min_w_0().truncate().text_color(Theme::danger()).child(failed),
                None => div().min_w_0().truncate().child(t("点一行接着写 · 未绑定的稿件可以随时绑定到文献库")),
            });
        let content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .bg(Theme::bg_body())
            .border_l_1()
            .border_color(Theme::line())
            .child(ui::title_strip())
            .child(ui::column_head().gap(px(8.)).child(search).child(new))
            .child(div().flex().flex_col().flex_1().min_h_0().child(body))
            .child(foot);
        div().flex().size_full().min_h_0().overflow_hidden().child(sidebar).child(content).into_any_element()
    }

    /// The list down the left: back, all the manuscripts, those in no library, then each library that holds one.
    fn written_sidebar(&self, view: &Written, cx: &mut Context<Self>) -> AnyElement {
        let row = |id: ElementId, label: String, count: usize, place: Place, this: &Self, cx: &mut Context<Self>| {
            let active = view.place == place;
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .h(text_px(30.))
                .px(px(8.))
                .rounded(px(7.))
                .cursor_pointer()
                .when(active, |d| d.bg(Theme::pressed()).font_weight(FontWeight::MEDIUM))
                .when(!active, |d| d.text_color(Theme::text_secondary()).hover(|s| s.bg(Theme::hover())))
                .on_click(this.on(Cmd::Written(Do::Show(place)), cx))
                .child(div().flex_1().min_w_0().truncate().child(label))
                // A number says there is something: none is written for none.
                .when(count > 0, |d| d.child(div().flex_none().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(count.to_string())))
                .into_any_element()
        };
        let mut rows = vec![row("written-all".into(), t("全部稿件").to_string(), written::count(&view.held, &Place::All), Place::All, self, cx)];
        let unbound = written::count(&view.held, &Place::Unbound);
        if unbound > 0 { rows.push(row("written-unbound".into(), t("未绑定").to_string(), unbound, Place::Unbound, self, cx)) }
        for one in view.held.iter().filter(|one| one.bound && !one.pieces.is_empty()) {
            rows.push(row(ElementId::Name(format!("written-of-{}", one.library).into()), one.name.clone(), one.pieces.len(), Place::Library(one.library.clone()), self, cx));
        }
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_W))
            .h_full()
            .min_h_0()
            .pt(px(Self::corner_top()))
            // The way back stands where the settings' and the tutorials' does: in the window's top-left corner.
            .child(
                div().px(px(10.)).child(
                    ui::title_strip()
                        .when(!ui::custom_titlebar(), |d| d.h(text_px(28.)))
                        .pl(px(ui::traffic_lights_width()))
                        .child(ui::ghost_button("written-back", Some("arrow-left"), Some(t("返回").into())).occlude().on_click(self.on(Cmd::Written(Do::Close), cx))),
                ),
            )
            .child(div().flex_none().px(px(18.)).pt(px(10.)).pb(px(8.)).font_weight(FontWeight::SEMIBOLD).text_color(Theme::text_secondary()).child(t("写作")))
            .child(div().id("written-holders").flex().flex_col().flex_1().min_h_0().px(px(10.)).pb(px(16.)).gap(px(1.)).overflow_y_scroll().track_scroll(&view.side).children(rows))
            .into_any_element()
    }

    /// A manuscript: its title over its first words, how long it is, where it is kept and when it was last written.
    /// One in no library has the way to bind it at the end of its row.
    fn written_row(&mut self, of: usize, at: usize, across: bool, width: f32, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some((one, piece)) = self.written.as_ref().and_then(|view| view.held.get(of)).and_then(|one| one.pieces.get(at).map(|piece| (one, piece))) else { return div().into_any_element() };
        let title = if piece.title.is_empty() { t(wisp_core::manuscript::UNTITLED).to_string() } else { ui::one_line(&piece.title) };
        let owner = if !one.bound { Some(t("未绑定").to_string()) } else if across { Some(one.name.clone()) } else { None };
        let length = (piece.length > 0).then(|| crate::write::count_label(piece.length, piece.letters));
        // Cut short, the title is there whole on hover.
        let owner_w = owner.as_ref().map_or(0., |name| ui::text_width(name, text_px(12.), window).min(OWNER_W) + 12.);
        let cut = ui::text_width(&title, text_px(13.), window) > width - 32. - 26. - 10. - owner_w - 84. - 12. - 80.;
        let bind = self.on(Cmd::Written(Do::AskBind(one.library.clone())), cx);
        div()
            .id(SharedString::from(format!("written-{}-{}", one.library, piece.id)))
            .flex()
            .items_center()
            .gap(px(12.))
            .w_full()
            .h(text_px(ROW_H))
            .px(px(16.))
            .border_b_1()
            .border_color(Theme::line())
            .cursor_pointer()
            .hover(|s| s.bg(Theme::hover()))
            .on_click(self.on(Cmd::Written(Do::Go(one.library.clone(), piece.id.clone())), cx))
            .child(div().flex().flex_none().items_center().justify_center().size(px(26.)).child(ui::icon("pen-line", 15., Theme::text_tertiary())))
            .child(
                div()
                    .id(SharedString::from(format!("written-title-{}-{}", one.library, piece.id)))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .when(cut, |d| d.tooltip(ui::tip(title.clone())))
                    .child(div().truncate().font_weight(FontWeight::MEDIUM).when(piece.title.is_empty(), |d| d.text_color(Theme::text_secondary())).child(title.clone()))
                    .child(div().truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(if piece.opening.is_empty() { t("还没有写").to_string() } else { piece.opening.clone() })),
            )
            .children(length.map(|length| div().flex_none().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(length)))
            .when_some(owner, |d, owner| d.child(div().flex_none().max_w(px(OWNER_W)).truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(owner)))
            .child(div().flex_none().w(px(84.)).text_right().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(time::date_text(&piece.updated_at)))
            // The place of the button is every row's, so the dates stand in one column.
            .child(div().flex().flex_none().items_center().justify_center().size(px(26.)).when(!one.bound, |d| {
                d.child(ui::ghost_button(SharedString::from(format!("written-bind-{}-{}", one.library, piece.id)), Some("link-2"), None).h(text_px(26.)).min_w(px(26.)).tooltip(ui::tip(t("绑定到文献库"))).on_click(move |ev, window, cx| {
                    cx.stop_propagation();
                    bind(ev, window, cx);
                }))
            }))
            .into_any_element()
    }
}
