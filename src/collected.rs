//! 收藏 of every library, in one place: the star beside the home page's book opens it. Inside a library 收藏
//! shows that library's own (its starred papers, 图片库, 方法库 and 导言库); here the same four are listed for
//! all the libraries on the shelf together (`wisp_core::collected`), the newest first, each row naming the
//! library it is in. The libraries are down the left, to look at one alone. Nothing is brought here: a click
//! on a row goes to its library, to that paper, figure or passage, where it is read and changed; the way back
//! from there is to this page. The one thing done in place is a paper's star.

use crate::app::{Cmd, Page, Workspace};
use crate::excerpts::{ROW_FIGURES, excerpt_row};
use crate::figures::{CARD_H, GAP, KEEP_THUMBS, PICTURE_H, THUMB_EDGE, THUMB_WORKERS, Thumb, collection_groove, collection_labels, collection_side, figure_card, grid, pixels};
use crate::i18n::t;
use crate::input::{InputEvent, TextInput};
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use wisp_core::collected::{self, Held};
use wisp_core::filter::View;
use wisp_core::libraries::{self, LibraryInfo};

pub use wisp_core::collected::Kind;

/// How wide the list of libraries is: the tutorials' and the settings' list.
const SIDEBAR_W: f32 = 256.;
/// The height of a paper's row.
const PAPER_H: f32 = 56.;
/// The widest the name of a row's library is written.
const OWNER_W: f32 = 200.;

/// What the page can be told.
#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// The star on the home page.
    Open,
    /// ← 返回, or Esc.
    Close,
    /// 文献, 图片, 方法 or 导言.
    Show(Kind),
    /// One library's alone, by its id; none is all of them.
    Library(Option<String>),
    /// A row: to its library, at the thing of this kind with this id.
    Go(String, Kind, String),
    /// The star of a paper of a library, put on or taken off.
    Star(String, String),
}

/// The page as the window holds it while it is open, and under a library gone to from it.
pub struct Collected {
    /// What each library holds, in the shelf's order, and everything of each kind across them, the newest first.
    held: Vec<Held>,
    order: [Vec<(usize, usize)>; 4],
    /// The libraries that could not be read, by name.
    failed: Vec<String>,
    /// Read at least once: before that the page says nothing of what there is.
    loaded: bool,
    /// Counts the readings: one that comes back after a later one began is dropped.
    run: u64,
    kind: Kind,
    library: Option<String>,
    search: Entity<TextInput>,
    _typed: Subscription,
    scroll: UniformListScrollHandle,
    side: ScrollHandle,
    /// The pictures read so far, by library and figure, the least lately shown first in `recent`.
    thumbs: HashMap<String, Arc<RenderImage>>,
    recent: VecDeque<String>,
    loading: HashSet<String>,
    broken: HashSet<String>,
}

impl Collected {
    /// How many of a kind there are, in the library looked at alone or in all of them.
    fn count(&self, kind: Kind) -> usize {
        self.held.iter().filter(|one| self.library.as_ref().is_none_or(|id| one.library == *id)).map(|one| one.count(kind)).sum()
    }

    fn touch(&mut self, key: &str) {
        if self.recent.back().is_some_and(|last| last == key) { return }
        self.recent.retain(|other| other != key);
        self.recent.push_back(key.to_string());
    }
}

/// What a paper's second line says: who, when, where.
fn paper_line(paper: &wisp_core::model::Paper) -> String {
    let mut parts = vec![];
    let who = crate::i18n::short_authors(paper);
    if !who.is_empty() { parts.push(who) }
    if let Some(year) = paper.year { parts.push(year.to_string()) }
    if !paper.journal.trim().is_empty() { parts.push(ui::one_line(&paper.journal)) }
    parts.join(" · ")
}

impl Workspace {
    pub(crate) fn collected_do(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            Do::Open => {
                self.bell.open = false;
                if self.collected.is_none() {
                    let search = cx.new(|cx| TextInput::new("搜索题目、作者、图注、原文、标签、备注…", cx));
                    let typed = cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
                        if *event != InputEvent::Changed { return }
                        // Another set of rows: from its top.
                        if let Some(view) = this.collected.as_mut() { view.scroll = UniformListScrollHandle::new() }
                        cx.notify();
                    });
                    self.collected = Some(Collected { held: vec![], order: Default::default(), failed: vec![], loaded: false, run: 0, kind: Kind::Papers, library: None, search, _typed: typed, scroll: UniformListScrollHandle::new(), side: ScrollHandle::new(), thumbs: HashMap::new(), recent: VecDeque::new(), loading: HashSet::new(), broken: HashSet::new() });
                }
                self.collected_read(cx);
                window.focus(&self.focus);
            }
            Do::Close => {
                if let Some(view) = self.collected.take() {
                    for (_, image) in view.thumbs { cx.drop_image(image, Some(window)) }
                }
                window.focus(&self.focus);
            }
            Do::Show(kind) => {
                let Some(view) = self.collected.as_mut() else { return };
                if view.kind != kind { (view.kind, view.scroll) = (kind, UniformListScrollHandle::new()) }
            }
            Do::Library(library) => {
                let Some(view) = self.collected.as_mut() else { return };
                if view.library != library { (view.library, view.scroll) = (library, UniformListScrollHandle::new()) }
            }
            Do::Go(library, kind, id) => {
                self.run(Cmd::OpenLibrary(library.clone()), window, cx);
                // Not opened (its folder is gone, its database is a newer build's): said by the opening.
                if self.lib.as_ref().is_none_or(|lib| lib.info.id != library) { return }
                match kind {
                    Kind::Papers => {
                        self.run(Cmd::SetView(View::Starred), window, cx);
                        self.run(Cmd::Select(id), window, cx);
                    }
                    Kind::Figures => {
                        self.run(Cmd::SetPage(Page::Figures), window, cx);
                        self.run(Cmd::OpenFigure(id), window, cx);
                    }
                    Kind::Methods | Kind::Intros => {
                        self.run(Cmd::SetPage(if kind == Kind::Methods { Page::Methods } else { Page::Intros }), window, cx);
                        self.run(Cmd::Excerpt(crate::excerpts::Do::Open(id)), window, cx);
                    }
                }
            }
            Do::Star(library, id) => {
                let Some((dir, paper)) = self.collected.as_mut().and_then(|view| view.held.iter_mut().find(|one| one.library == library)).and_then(|one| one.papers.iter_mut().find(|paper| paper.id == id).map(|paper| (one.dir.clone(), paper))) else { return };
                // The row stays where it is with its star hollow, to be put back with another click: it leaves the list when the page is next read.
                let starred = !paper.starred;
                match collected::star(&dir, &id, starred) {
                    Ok(()) => paper.starred = starred,
                    Err(err) => self.fail(format!("{err:#}"), cx),
                }
            }
        }
        cx.notify();
    }

    /// Read what the libraries on the shelf hold, off the main thread: when the page opens, and when a library gone
    /// to from it is left. What was read before stays on show until the new reading is in.
    pub(crate) fn collected_read(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.collected.as_mut() else { return };
        view.run += 1;
        let run = view.run;
        let shelf: Vec<LibraryInfo> = libraries::shelf(&self.libraries, &self.settings.library_order).into_iter().cloned().collect();
        let work = cx.background_executor().spawn(async move {
            let (mut held, mut failed) = (vec![], vec![]);
            for info in &shelf {
                match collected::read(info) {
                    Ok(one) => held.push(one),
                    Err(_) => failed.push(info.name.clone()),
                }
            }
            let order = Kind::ALL.map(|kind| collected::merged(&held, kind));
            (held, order, failed)
        });
        cx.spawn(async move |this, cx| {
            let (held, order, failed) = work.await;
            this.update(cx, |this, cx| {
                let Some(view) = this.collected.as_mut().filter(|view| view.run == run) else { return };
                (view.held, view.order, view.failed, view.loaded) = (held, order, failed, true);
                // The library looked at alone is gone, or holds nothing any more: all of them.
                if view.library.as_ref().is_some_and(|id| !view.held.iter().any(|one| one.library == *id && one.total() > 0)) { view.library = None }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// What the debug step `collected` stands for (`dev.rs`): the page opened, a kind or a library shown, the n-th
    /// row on show clicked, or its star.
    #[cfg(debug_assertions)]
    pub(crate) fn collected_step(&self, arg: &str, cx: &App) -> Option<Cmd> {
        let (what, n) = arg.split_once(':').map_or((arg, None), |(what, n)| (what, n.parse::<usize>().ok()));
        let view = self.collected.as_ref();
        Some(Cmd::Collected(match what {
            "" => Do::Open,
            "close" => Do::Close,
            "library" => Do::Library(n.and_then(|n| view?.held.iter().filter(|one| one.total() > 0).nth(n)).map(|one| one.library.clone())),
            "go" | "star" => {
                let view = view?;
                let words = wisp_core::figure::search_words(view.search.read(cx).text());
                let &(library, at) = collected::shown(&view.held, &view.order[view.kind as usize], view.kind, view.library.as_deref(), &words).get(n.unwrap_or(0))?;
                let one = view.held.get(library)?;
                match (what, view.kind) {
                    ("star", Kind::Papers) => Do::Star(one.library.clone(), one.papers.get(at)?.id.clone()),
                    ("star", _) => return None,
                    (_, Kind::Papers) => Do::Go(one.library.clone(), Kind::Papers, one.papers.get(at)?.id.clone()),
                    (_, Kind::Figures) => Do::Go(one.library.clone(), Kind::Figures, one.figures.get(at)?.id.clone()),
                    (_, kind) => Do::Go(one.library.clone(), kind, one.excerpts.get(at)?.id.clone()),
                }
            }
            kind => Do::Show(Kind::from_key(kind)?),
        }))
    }

    /// Esc on the page, one layer at a time (design.md rule 53): the search field gives the keyboard back, then
    /// what is typed in it goes, then the page.
    pub(crate) fn collected_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.collected.as_mut() else { return };
        if view.search.focus_handle(cx).is_focused(window) {
            window.focus(&self.focus);
        } else if !view.search.read(cx).text().is_empty() {
            view.search.update(cx, |input, cx| input.set_text("", cx));
            view.scroll = UniformListScrollHandle::new();
        } else {
            self.run(Cmd::Collected(Do::Close), window, cx);
        }
        cx.notify();
    }

    /// The search key on the page: its field.
    pub(crate) fn collected_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.collected.as_ref() { view.search.read(cx).focus(window) }
        cx.notify();
    }

    /// The picture of a figure of the library at `library`: the one read before, or its reading started (a few at a
    /// time; the rest when these are back), as a library's own cards have theirs (`figure_thumb`).
    fn collected_thumb(&mut self, library: usize, figure: &str, cx: &mut Context<Self>) -> Thumb {
        let Some(view) = self.collected.as_mut() else { return Thumb::Broken };
        let Some((key, path)) = view.held.get(library).and_then(|one| one.figures.iter().find(|held| held.id == figure).map(|held| (format!("{}/{figure}", one.library), one.dir.join(held.local_path.replace('/', std::path::MAIN_SEPARATOR_STR))))) else { return Thumb::Broken };
        if let Some(image) = view.thumbs.get(&key).cloned() {
            view.touch(&key);
            return Thumb::Ready(image);
        }
        if view.broken.contains(&key) { return Thumb::Broken }
        if view.loading.contains(&key) || view.loading.len() >= THUMB_WORKERS { return Thumb::Loading }
        view.loading.insert(key.clone());
        let work = cx.background_executor().spawn(async move { std::fs::read(&path).ok().and_then(|bytes| pixels(&bytes, THUMB_EDGE)) });
        cx.spawn(async move |this, cx| {
            let picture = work.await;
            this.update(cx, |this, cx| {
                // Closed meanwhile: nothing to keep it for.
                let Some(view) = this.collected.as_mut() else { return };
                view.loading.remove(&key);
                match picture {
                    Some(picture) => {
                        view.thumbs.insert(key.clone(), Arc::new(RenderImage::new(vec![image::Frame::new(picture)])));
                        view.touch(&key);
                        while view.recent.len() > KEEP_THUMBS {
                            if let Some(old) = view.recent.pop_front() && let Some(image) = view.thumbs.remove(&old) { cx.drop_image(image, None) }
                        }
                    }
                    None => {
                        view.broken.insert(key);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        Thumb::Loading
    }

    pub(crate) fn render_collected(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.collected.as_ref() else { return div().into_any_element() };
        let width = (f32::from(window.viewport_size().width) - SIDEBAR_W - 1.).max(240.);
        let kind = view.kind;
        let query = view.search.read(cx).text().to_string();
        let words = wisp_core::figure::search_words(&query);
        let shown = Rc::new(collected::shown(&view.held, &view.order[kind as usize], kind, view.library.as_deref(), &words));
        let (total, count) = (view.count(kind), shown.len());
        // Whose a row is, is written only while several libraries are listed together.
        let across = view.library.is_none();
        let sidebar = self.collected_sidebar(view, cx);

        let counts = Kind::ALL.map(|kind| view.count(kind));
        let switch = collection_groove().children(Kind::ALL.into_iter().zip(["collected-papers", "collected-figures", "collected-methods", "collected-intros"]).zip(collection_labels(counts)).map(|((of, id), label)| collection_side(id, label, of == kind).on_click(self.on(Cmd::Collected(Do::Show(of)), cx))));
        let focused = view.search.focus_handle(cx).is_focused(window);
        let search = div().flex().flex_1().items_center().gap(px(8.)).h(text_px(34.)).px(px(10.)).rounded(px(8.)).bg(Theme::bg_sunken()).border_1().border_color(if focused { Theme::accent() } else { Theme::bg_sunken() }).child(ui::icon("search", 15., Theme::text_tertiary())).child(view.search.clone());

        let body = if !view.loaded {
            div().flex_1().into_any_element()
        } else if count == 0 && query.trim().is_empty() {
            let (icon, title) = match kind {
                Kind::Papers => ("star", t("还没有收藏文献")),
                Kind::Figures => ("image", t("还没有收藏图片")),
                Kind::Methods => ("flask-conical", t("还没有收藏方法")),
                Kind::Intros => ("quote", t("还没有收藏导言")),
            };
            ui::empty_state(icon, title, if across { t("在任何一个文献库里收藏的文献、图片、方法和导言都列在这里，点一项回到它所在的文献库。") } else { t("这个文献库里还没有。在左边选「全部文献库」，看所有文献库的。") }).into_any_element()
        } else if count == 0 {
            let title = match kind {
                Kind::Papers => t("没有匹配的文献"),
                Kind::Figures => t("没有匹配的图片"),
                Kind::Methods => t("没有匹配的方法"),
                Kind::Intros => t("没有匹配的导言"),
            };
            ui::empty_state("search", title, if across { t("换一个搜索词试试。") } else { t("换一个搜索词，或者在左边选「全部文献库」。") }).into_any_element()
        } else if kind == Kind::Figures {
            let (columns, card) = grid(width);
            let shown = shown.clone();
            uniform_list("collected-figure-rows", count.div_ceil(columns), cx.processor(move |this, range: Range<usize>, _, cx| this.collected_figure_rows(range, &shown, columns, card, across, cx))).track_scroll(view.scroll.clone()).flex_1().into_any_element()
        } else {
            let shown = shown.clone();
            uniform_list(if kind == Kind::Papers { "collected-paper-rows" } else { "collected-excerpt-rows" }, count, cx.processor(move |this, range: Range<usize>, window, cx| range.filter_map(|at| shown.get(at).copied()).map(|(library, at)| if kind == Kind::Papers { this.collected_paper_row(library, at, across, width, window, cx) } else { this.collected_excerpt_row(library, at, across, cx) }).collect::<Vec<_>>())).track_scroll(view.scroll.clone()).flex_1().into_any_element()
        };

        // How many there are and in how many libraries; a library that could not be read is named, as a fault.
        // A paper whose star was just taken off stays as a row until the page is read again, and is not one of them.
        let kept: Vec<usize> = shown.iter().filter(|&&(library, at)| kind != Kind::Papers || view.held.get(library).and_then(|one| one.papers.get(at)).is_some_and(|paper| paper.starred)).map(|&(library, _)| library).collect();
        let (count, libraries) = (kept.len(), kept.iter().collect::<HashSet<_>>().len());
        let status = if !view.loaded || total == 0 {
            String::new()
        } else if count != total {
            tf!("显示 {count} / {total}", "Showing {count} / {total}")
        } else if across {
            match kind {
                Kind::Papers => tf!("共 {total} 篇，在 {libraries} 个文献库里", "{total} papers in {libraries} libraries"),
                Kind::Figures => tf!("共 {total} 张，在 {libraries} 个文献库里", "{total} figures in {libraries} libraries"),
                Kind::Methods => tf!("共 {total} 个，在 {libraries} 个文献库里", "{total} methods in {libraries} libraries"),
                Kind::Intros => tf!("共 {total} 段，在 {libraries} 个文献库里", "{total} introductions in {libraries} libraries"),
            }
        } else {
            match kind {
                Kind::Papers => tf!("共 {total} 篇", "{total} papers"),
                Kind::Figures => tf!("共 {total} 张", "{total} figures"),
                Kind::Methods => tf!("共 {total} 个", "{total} methods"),
                Kind::Intros => tf!("共 {total} 段", "{total} introductions"),
            }
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
                None => div().min_w_0().truncate().child(if kind == Kind::Papers { t("点一行回到它所在的文献库 · 点星标取消收藏") } else { t("点一项回到它所在的文献库，在那里查看和修改") }),
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
            .child(ui::column_head().gap(px(8.)).child(switch).child(search))
            .child(div().flex().flex_col().flex_1().min_h_0().when(kind == Kind::Figures, |d| d.pt(px(GAP))).child(body))
            .child(foot);
        div().flex().size_full().min_h_0().overflow_hidden().child(sidebar).child(content).into_any_element()
    }

    /// The list down the left: back, all the libraries together, then each library that holds something.
    fn collected_sidebar(&self, view: &Collected, cx: &mut Context<Self>) -> AnyElement {
        let row = |id: ElementId, label: String, count: usize, active: bool, cmd: Cmd, this: &Self, cx: &mut Context<Self>| {
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
                .on_click(this.on(cmd, cx))
                .child(div().flex_1().min_w_0().truncate().child(label))
                // A number says there is something: none is written for none.
                .when(count > 0, |d| d.child(div().flex_none().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(count.to_string())))
                .into_any_element()
        };
        let all: usize = view.held.iter().map(Held::total).sum();
        let mut rows = vec![row("collected-all".into(), t("全部文献库").to_string(), all, view.library.is_none(), Cmd::Collected(Do::Library(None)), self, cx)];
        for one in view.held.iter().filter(|one| one.total() > 0) {
            rows.push(row(ElementId::Name(format!("collected-of-{}", one.library).into()), one.name.clone(), one.total(), view.library.as_deref() == Some(one.library.as_str()), Cmd::Collected(Do::Library(Some(one.library.clone()))), self, cx));
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
                        .child(ui::ghost_button("collected-back", Some("arrow-left"), Some(t("返回").into())).occlude().on_click(self.on(Cmd::Collected(Do::Close), cx))),
                ),
            )
            .child(div().flex_none().px(px(18.)).pt(px(10.)).pb(px(8.)).font_weight(FontWeight::SEMIBOLD).text_color(Theme::text_secondary()).child(t("收藏")))
            .child(div().id("collected-libraries").flex().flex_col().flex_1().min_h_0().px(px(10.)).pb(px(16.)).gap(px(1.)).overflow_y_scroll().track_scroll(&view.side).children(rows))
            .into_any_element()
    }

    /// A starred paper: its star, its title over who, when and where, and the library it is in.
    fn collected_paper_row(&mut self, library: usize, at: usize, across: bool, width: f32, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some((one, paper)) = self.collected.as_ref().and_then(|view| view.held.get(library)).and_then(|one| one.papers.get(at).map(|paper| (one, paper))) else { return div().into_any_element() };
        let title = ui::one_line(&paper.title);
        let owner = across.then(|| one.name.clone());
        // Cut short, the title is there whole on hover.
        let owner_w = owner.as_ref().map_or(0., |name| ui::text_width(name, text_px(12.), window).min(OWNER_W) + 12.);
        let cut = ui::text_width(&title, text_px(13.), window) > width - 32. - 26. - 10. - owner_w;
        let star = self.on(Cmd::Collected(Do::Star(one.library.clone(), paper.id.clone())), cx);
        div()
            .id(SharedString::from(format!("collected-paper-{}-{}", one.library, paper.id)))
            .flex()
            .items_center()
            .gap(px(10.))
            .w_full()
            .h(text_px(PAPER_H))
            .px(px(16.))
            .border_b_1()
            .border_color(Theme::line())
            .cursor_pointer()
            .hover(|s| s.bg(Theme::hover()))
            .on_click(self.on(Cmd::Collected(Do::Go(one.library.clone(), Kind::Papers, paper.id.clone())), cx))
            .child(
                div()
                    .id(SharedString::from(format!("collected-star-{}-{}", one.library, paper.id)))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(26.))
                    .rounded(px(6.))
                    .hover(|s| s.bg(Theme::pressed()))
                    .tooltip(ui::tip(if paper.starred { t("取消收藏") } else { t("收藏") }))
                    .on_click(move |ev, window, cx| {
                        cx.stop_propagation();
                        star(ev, window, cx);
                    })
                    .child(ui::icon("star", 15., if paper.starred { Theme::star() } else { Theme::text_placeholder() })),
            )
            .child(
                div()
                    .id(SharedString::from(format!("collected-title-{}-{}", one.library, paper.id)))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .when(cut, |d| d.tooltip(ui::tip(title.clone())))
                    .child(div().truncate().font_weight(FontWeight::MEDIUM).when(!paper.starred, |d| d.text_color(Theme::text_secondary())).child(title.clone()))
                    .child(div().truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(paper_line(paper))),
            )
            .when_some(owner, |d, owner| d.child(div().flex_none().max_w(px(OWNER_W)).truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(owner)))
            .into_any_element()
    }

    fn collected_figure_rows(&mut self, range: Range<usize>, shown: &[(usize, usize)], columns: usize, card: f32, across: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        range
            .map(|row| {
                let cards: Vec<AnyElement> = shown.iter().skip(row * columns).take(columns).map(|&(library, at)| self.collected_figure_card(library, at, card, across, cx)).collect();
                div().flex().flex_none().gap(px(GAP)).w_full().h(px(PICTURE_H + GAP) + text_px(CARD_H - PICTURE_H)).px(px(16.)).pb(px(GAP)).children(cards).into_any_element()
            })
            .collect()
    }

    fn collected_figure_card(&mut self, library: usize, at: usize, width: f32, across: bool, cx: &mut Context<Self>) -> AnyElement {
        let Some(figure) = self.collected.as_ref().and_then(|view| view.held.get(library)).and_then(|one| one.figures.get(at)).cloned() else { return div().into_any_element() };
        let thumb = self.collected_thumb(library, &figure.id, cx);
        let Some(one) = self.collected.as_ref().and_then(|view| view.held.get(library)) else { return div().into_any_element() };
        figure_card(SharedString::from(format!("collected-figure-{}-{}", one.library, figure.id)), &figure, one.paper(&figure.paper_id), thumb, width, across.then(|| one.name.clone()))
            .on_click(self.on(Cmd::Collected(Do::Go(one.library.clone(), Kind::Figures, figure.id.clone())), cx))
            .into_any_element()
    }

    fn collected_excerpt_row(&mut self, library: usize, at: usize, across: bool, cx: &mut Context<Self>) -> AnyElement {
        let Some(excerpt) = self.collected.as_ref().and_then(|view| view.held.get(library)).and_then(|one| one.excerpts.get(at)).cloned() else { return div().into_any_element() };
        let method = excerpt.kind == wisp_core::excerpt::METHOD;
        let linked: Vec<String> = if method { self.collected.as_ref().and_then(|view| view.held.get(library)).map(|one| one.figures_of(&excerpt.id).into_iter().map(|figure| figure.id.clone()).collect()).unwrap_or_default() } else { vec![] };
        let thumbs: Vec<(String, Thumb)> = linked.iter().take(ROW_FIGURES).map(|id| (id.clone(), self.collected_thumb(library, id, cx))).collect();
        let Some(one) = self.collected.as_ref().and_then(|view| view.held.get(library)) else { return div().into_any_element() };
        excerpt_row(SharedString::from(format!("collected-excerpt-{}-{}", one.library, excerpt.id)), &excerpt, one.paper(&excerpt.paper_id), thumbs, linked.len(), across.then(|| one.name.clone()))
            .on_click(self.on(Cmd::Collected(Do::Go(one.library.clone(), if method { Kind::Methods } else { Kind::Intros }, excerpt.id.clone())), cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's own `test` attribute.
    use super::paper_line;
    use wisp_core::model::Paper;

    #[test]
    fn a_paper_says_who_when_and_where() {
        let paper = Paper { authors: "Zhang Wei; Li Na; Wang Fang".into(), year: Some(2021), journal: "Plant\n Cell".into(), ..Default::default() };
        assert_eq!(paper_line(&paper), "Zhang Wei 等 · 2021 · Plant Cell");
        assert_eq!(paper_line(&Paper { year: Some(2019), ..Default::default() }), "2019");
        assert_eq!(paper_line(&Paper::default()), "");
    }
}
