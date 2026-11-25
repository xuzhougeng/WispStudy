//! 教程: the tutorials of the project page, read in the window. The book beside the home page's bell opens
//! them: the shelf and every tutorial down the left, the one chosen in a column on the right. The words are
//! built in (`wisp_core::tutorial`, from the site's own HTML); the screenshots are fetched from the project
//! page the first time a tutorial is shown and kept in the cache folder, so a window with no network still
//! reads the words, each picture standing as the box it would fill, with what it shows.

use crate::app::{Cmd, Workspace};
use crate::i18n::{is_english, t};
use crate::theme::{self, Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use wisp_core::markdown::Rich;
use wisp_core::tutorial::{self, Block, Link, Shelf, Tutorial};

/// How wide the list of tutorials is.
const SIDEBAR_W: f32 = 256.;
/// The widest the reading column gets: a screenshot at this width still shows its words.
const COLUMN_W: f32 = 760.;

/// What the tutorials can be told.
#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// The book on the home page, or the last card of 新手引导: the shelf.
    Open,
    /// ← 返回, or Esc.
    Close,
    /// A page by name (`index` is the shelf), at a section of it or its top.
    Show(String, Option<String>),
    /// A link in a tutorial, as written on the page.
    Follow(String),
    /// The page on show, on the project page in the browser.
    Browser,
}

/// A screenshot of the project page, as far as it got.
enum Shot {
    Fetching,
    Ready(PathBuf),
    /// Not fetched (no network, or the site no longer has it): its box says what it shows.
    Missing,
}

/// The tutorials as the window holds them while they are open.
pub struct Tutorials {
    /// The language they were read in: the interface's, when they were opened.
    english: bool,
    shelf: Shelf,
    /// The tutorial on show, by name, and what it was read into; none is the shelf.
    open: Option<(String, Tutorial)>,
    scroll: ScrollHandle,
    side: ScrollHandle,
    shots: HashMap<String, Shot>,
}

impl Tutorials {
    fn new() -> Self {
        let english = is_english();
        Tutorials { english, shelf: tutorial::shelf(english), open: None, scroll: ScrollHandle::new(), side: ScrollHandle::new(), shots: HashMap::new() }
    }

    fn slug(&self) -> &str {
        self.open.as_ref().map_or("index", |(slug, _)| slug.as_str())
    }
}

/// A screenshot, fetched and kept where `tutorial::cached` says.
fn fetch(url: &str) -> Result<PathBuf, String> {
    let path = tutorial::cached(url).ok_or_else(|| format!("{url} 不在项目主页上。"))?;
    let bytes = wisp_net::download(url)?;
    // A page that says "not found" comes back as text, not as a picture.
    if image::guess_format(&bytes).is_err() { return Err(format!("{url} 不是图片。")) }
    if let Some(folder) = path.parent() { std::fs::create_dir_all(folder).map_err(|err| err.to_string())? }
    // Written whole under another name first, so a picture cut short is never taken for one kept.
    let part = path.with_extension("part");
    std::fs::write(&part, &bytes).and_then(|()| std::fs::rename(&part, &path)).map_err(|err| err.to_string())?;
    Ok(path)
}

impl Workspace {
    pub(crate) fn tutorial_do(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            Do::Open => {
                self.bell.open = false;
                if self.tutorials.is_none() { self.tutorials = Some(Tutorials::new()) }
                window.focus(&self.focus);
            }
            Do::Close => {
                self.tutorials = None;
                window.focus(&self.focus);
            }
            Do::Show(slug, anchor) => self.tutorial_show(&slug, anchor.as_deref(), cx),
            Do::Follow(href) => {
                let Some(view) = &self.tutorials else { return };
                match tutorial::link(&href, view.slug(), view.english) {
                    Link::Page(slug, anchor) => self.tutorial_show(&slug, anchor.as_deref(), cx),
                    Link::Web(url) => cx.open_url(&url),
                }
            }
            Do::Browser => {
                let Some(view) = &self.tutorials else { return };
                cx.open_url(&tutorial::page_url(Some(view.slug()), view.english));
            }
        }
        cx.notify();
    }

    /// A page of the tutorials on show, at the top or at a section; the pictures it has not yet are fetched.
    fn tutorial_show(&mut self, slug: &str, anchor: Option<&str>, cx: &mut Context<Self>) {
        let Some(view) = self.tutorials.as_mut() else { return };
        if view.slug() != slug {
            view.open = if slug == "index" { None } else { tutorial::tutorial(slug, view.english).map(|page| (slug.to_string(), page)) };
            view.scroll.set_offset(point(px(0.), px(0.)));
        }
        // The column's first child is the page's head; a section is the one after it.
        match anchor.and_then(|anchor| view.open.as_ref()?.1.sections.iter().position(|section| section.id == anchor)) {
            Some(at) => view.scroll.scroll_to_top_of_item(at + 1),
            None => view.scroll.set_offset(point(px(0.), px(0.))),
        }
        self.tutorial_fetch(cx);
    }

    /// Fetch, one after the other, the screenshots of the tutorial on show that are neither kept nor on their way.
    fn tutorial_fetch(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.tutorials.as_mut() else { return };
        let Some((_, page)) = &view.open else { return };
        let base = tutorial::base(view.english);
        let mut wanted = vec![];
        for block in page.sections.iter().flat_map(|section| &section.blocks) {
            let Block::Figure { src, .. } = block else { continue };
            let url = tutorial::resolve(&base, src);
            if view.shots.contains_key(&url) { continue }
            match tutorial::cached(&url).filter(|path| path.is_file()) {
                Some(path) => { view.shots.insert(url, Shot::Ready(path)); }
                None => {
                    view.shots.insert(url.clone(), Shot::Fetching);
                    wanted.push(url);
                }
            }
        }
        if wanted.is_empty() { return }
        cx.spawn(async move |this, cx| {
            for url in wanted {
                let fetched = cx.background_executor().spawn({
                    let url = url.clone();
                    async move { fetch(&url) }
                });
                let fetched = fetched.await;
                let open = this.update(cx, |this, cx| {
                    // Closed meanwhile: what was fetched is kept for next time, the rest is left.
                    let Some(view) = this.tutorials.as_mut() else { return false };
                    view.shots.insert(url, match fetched {
                        Ok(path) => Shot::Ready(path),
                        Err(_) => Shot::Missing,
                    });
                    cx.notify();
                    true
                });
                if !matches!(open, Ok(true)) { break }
            }
        })
        .detach();
    }

    pub(crate) fn render_tutorials(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // The interface's language changed while they were open: read again in the new one.
        if self.tutorials.as_ref().is_some_and(|view| view.english != is_english()) {
            let slug = self.tutorials.as_ref().map(|view| view.slug().to_string()).unwrap_or_default();
            self.tutorials = Some(Tutorials::new());
            self.tutorial_show(&slug, None, cx);
        }
        let Some(view) = &self.tutorials else { return div().into_any_element() };
        let column = (f32::from(window.viewport_size().width) - SIDEBAR_W - 65.).clamp(240., COLUMN_W);
        let sidebar = self.tutorial_sidebar(view, window, cx);
        let pieces = match &view.open {
            Some((slug, page)) => self.tutorial_page(view, slug, page, column, cx),
            None => self.tutorial_shelf(view, cx),
        };
        let browser = ui::ghost_button("tutorial-browser", Some("globe"), None).occlude().tooltip(ui::tip(t("在浏览器中打开"))).on_click(self.on(Cmd::Tutorial(Do::Browser), cx));
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
            .child(ui::title_strip().justify_end().pr(px(if cfg!(target_os = "windows") { 150. } else { 12. })).child(browser))
            .child(
                div()
                    .id("tutorial-content")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .items_center()
                    .px(px(32.))
                    .pb(px(64.))
                    .gap(px(28.))
                    .overflow_y_scroll()
                    .track_scroll(&view.scroll)
                    // Each piece is a child of the scroll itself, at its own height, so a section can be scrolled to.
                    .children(pieces.into_iter().map(|piece| div().flex().flex_col().flex_none().w(px(column)).child(piece))),
            );
        div().flex().size_full().min_h_0().overflow_hidden().child(sidebar).child(content).into_any_element()
    }

    /// The list down the left: back, the shelf, and each tutorial under its part; the one on show with its sections.
    fn tutorial_sidebar(&self, view: &Tutorials, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let slug = view.slug();
        let row = |id: ElementId, active: bool| {
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
        };
        let room = SIDEBAR_W - 20. - 16. - 30.;
        let mut rows: Vec<AnyElement> = vec![
            row("tutorial-shelf".into(), view.open.is_none())
                .on_click(self.on(Cmd::Tutorial(Do::Show("index".into(), None)), cx))
                .child(ui::icon("book-open", 15., if view.open.is_none() { Theme::text() } else { Theme::text_secondary() }))
                .child(t("全部教程"))
                .into_any_element(),
        ];
        for part in &view.shelf.parts {
            rows.push(div().flex_none().px(px(8.)).pt(px(14.)).pb(px(4.)).text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(part.name.clone()).into_any_element());
            for entry in &part.entries {
                let active = entry.slug == slug;
                let title = entry.short_title().to_string();
                // Cut short, it is there whole on hover.
                let cut = ui::text_width(&title, text_px(13.), window) > room;
                rows.push(
                    row(ElementId::Name(format!("tutorial-{}", entry.slug).into()), active)
                        .on_click(self.on(Cmd::Tutorial(Do::Show(entry.slug.clone(), None)), cx))
                        .when(cut, |d| d.tooltip(ui::tip(entry.title.clone())))
                        .child(div().flex_none().w(px(22.)).text_size(text_px(12.)).text_color(if active { Theme::accent() } else { Theme::text_tertiary() }).child(entry.number.clone()))
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .into_any_element(),
                );
                // The sections of the one on show, under it: a click goes to one.
                if let Some((_, page)) = view.open.as_ref().filter(|_| active) {
                    for section in &page.sections {
                        let cmd = Cmd::Tutorial(Do::Show(entry.slug.clone(), Some(section.id.clone())));
                        rows.push(
                            div()
                                .id(ElementId::Name(format!("tutorial-{}-{}", entry.slug, section.id).into()))
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(px(6.))
                                .h(text_px(26.))
                                .pl(px(38.))
                                .pr(px(8.))
                                .rounded(px(6.))
                                .text_size(text_px(12.5))
                                .text_color(Theme::text_secondary())
                                .cursor_pointer()
                                .hover(|s| s.bg(Theme::hover()))
                                .when(ui::text_width(&section.title, text_px(12.5), window) > room - 24., |d| d.tooltip(ui::tip(section.title.clone())))
                                .on_click(self.on(cmd, cx))
                                .child(div().flex_1().min_w_0().truncate().child(section.title.clone()))
                                .into_any_element(),
                        );
                    }
                }
            }
        }
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_W))
            .h_full()
            .min_h_0()
            .pt(px(Self::corner_top()))
            // The way back stands where the settings' does: in the window's top-left corner, beside the traffic lights.
            .child(
                div().px(px(10.)).child(
                    ui::title_strip()
                        .when(!ui::custom_titlebar(), |d| d.h(text_px(28.)))
                        .pl(px(ui::traffic_lights_width()))
                        .child(ui::ghost_button("tutorial-back", Some("arrow-left"), Some(t("返回").into())).occlude().on_click(self.on(Cmd::Tutorial(Do::Close), cx))),
                ),
            )
            .child(div().flex_none().px(px(18.)).pt(px(10.)).pb(px(8.)).font_weight(FontWeight::SEMIBOLD).text_color(Theme::text_secondary()).child(t("教程")))
            .child(div().id("tutorial-list").flex().flex_col().flex_1().min_h_0().px(px(10.)).pb(px(16.)).gap(px(1.)).overflow_y_scroll().track_scroll(&view.side).children(rows))
            .into_any_element()
    }

    /// The shelf: what the tutorials are, then each part and its tutorials, a click away.
    fn tutorial_shelf(&self, view: &Tutorials, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let shelf = &view.shelf;
        let mut pieces = vec![self.tutorial_head(&shelf.kicker, &shelf.title, &shelf.lead, &shelf.facts, "index", cx)];
        for (n, part) in shelf.parts.iter().enumerate() {
            let entries: Vec<AnyElement> = part
                .entries
                .iter()
                .map(|entry| {
                    let needs = entry.needs.iter().map(|(term, value)| format!("{term}　{value}")).collect::<Vec<_>>().join("　·　");
                    div()
                        .id(ElementId::Name(format!("shelf-{}", entry.slug).into()))
                        .flex()
                        .gap(px(14.))
                        .px(px(14.))
                        .py(px(12.))
                        .rounded(px(10.))
                        .cursor_pointer()
                        .hover(|s| s.bg(Theme::hover()))
                        .on_click(self.on(Cmd::Tutorial(Do::Show(entry.slug.clone(), None)), cx))
                        .child(div().flex_none().w(px(24.)).pt(px(1.)).text_size(text_px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(Theme::accent()).child(entry.number.clone()))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap(px(4.))
                                .child(div().text_size(text_px(15.)).line_height(text_px(22.)).font_weight(FontWeight::MEDIUM).child(entry.title.clone()))
                                .child(div().text_size(text_px(13.)).line_height(theme::body_line(20.)).text_color(Theme::text_secondary()).child(entry.summary.text.clone()))
                                .child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(needs)),
                        )
                        .into_any_element()
                })
                .collect();
            pieces.push(
                div()
                    .id(("shelf-part", n))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().pb(px(6.)).mx(px(14.)).mb(px(4.)).border_b_1().border_color(Theme::line()).text_size(text_px(17.)).line_height(text_px(26.)).font_weight(FontWeight::SEMIBOLD).child(part.name.clone()))
                    .children(entries)
                    .into_any_element(),
            );
        }
        pieces
    }

    /// The head of a page: the line over the title, the title, the lead and the facts under it.
    fn tutorial_head(&self, kicker: &str, title: &str, lead: &Rich, facts: &[(String, Rich)], key: &str, cx: &mut Context<Self>) -> AnyElement {
        let facts: Vec<AnyElement> = facts
            .iter()
            .enumerate()
            .map(|(n, (term, value))| {
                div()
                    .flex()
                    .gap(px(16.))
                    .py(px(8.))
                    .when(n > 0, |d| d.border_t_1().border_color(Theme::line()))
                    .child(div().flex_none().w(px(84.)).text_color(Theme::text_tertiary()).child(term.clone()))
                    .child(div().flex_1().min_w_0().child(self.tutorial_text(&format!("{key}-fact-{n}"), value, cx)))
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .pt(px(8.))
            .child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(kicker.to_string()))
            .child(div().text_size(text_px(26.)).line_height(text_px(34.)).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
            .child(div().text_size(text_px(15.)).line_height(theme::body_line(25.)).text_color(Theme::text_secondary()).child(self.tutorial_text(&format!("{key}-lead"), lead, cx)))
            .when(!facts.is_empty(), |d| d.child(div().flex().flex_col().mt(px(6.)).py(px(2.)).border_t_1().border_b_1().border_color(Theme::line()).text_size(text_px(13.)).line_height(theme::body_line(20.)).children(facts)))
            .into_any_element()
    }

    /// A tutorial: its head, a piece for each section, and its foot with the ones before and after it.
    fn tutorial_page(&self, view: &Tutorials, slug: &str, page: &Tutorial, column: f32, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut pieces = vec![self.tutorial_head(&page.kicker, &page.title, &page.lead, &page.facts, slug, cx)];
        let base = tutorial::base(view.english);
        for section in &page.sections {
            let key = format!("{slug}-{}", section.id);
            let blocks: Vec<AnyElement> = section.blocks.iter().enumerate().map(|(n, block)| self.tutorial_block(view, &base, &format!("{key}-{n}"), block, column, cx)).collect();
            pieces.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::paragraph_gap(14.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(10.))
                            .pt(px(8.))
                            .text_size(text_px(19.))
                            .line_height(text_px(28.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .when(!section.number.is_empty(), |d| d.child(div().flex_none().text_color(Theme::accent()).child(section.number.clone())))
                            .child(section.title.clone()),
                    )
                    .children(blocks)
                    .into_any_element(),
            );
        }
        let (before, after) = view.shelf.neighbours(slug);
        let neighbour = |entry: Option<&tutorial::Entry>, next: bool, cx: &mut Context<Self>| {
            let Some(entry) = entry else { return div().flex_1().into_any_element() };
            let label = if next { t("下一篇") } else { t("上一篇") };
            div()
                .id(if next { "tutorial-next" } else { "tutorial-prev" })
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .px(px(14.))
                .py(px(10.))
                .rounded(px(10.))
                .border_1()
                .border_color(Theme::line())
                .when(next, |d| d.items_end())
                .cursor_pointer()
                .hover(|s| s.bg(Theme::hover()))
                .on_click(self.on(Cmd::Tutorial(Do::Show(entry.slug.clone(), None)), cx))
                .child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(format!("{label} · {}", entry.number)))
                .child(div().max_w_full().truncate().font_weight(FontWeight::MEDIUM).child(entry.short_title().to_string()))
                .into_any_element()
        };
        let colophon: Vec<AnyElement> = page.colophon.iter().enumerate().map(|(n, text)| self.tutorial_text(&format!("{slug}-colophon-{n}"), text, cx).into_any_element()).collect();
        let (before, after) = (neighbour(before, false, cx), neighbour(after, true, cx));
        pieces.push(
            div()
                .flex()
                .flex_col()
                .gap(px(20.))
                .pt(px(8.))
                .child(div().flex().flex_col().gap(px(8.)).pt(px(16.)).border_t_1().border_color(Theme::line()).text_size(text_px(12.5)).line_height(theme::body_line(20.)).text_color(Theme::text_tertiary()).children(colophon))
                .child(div().flex().gap(px(12.)).text_size(text_px(14.)).child(before).child(after))
                .into_any_element(),
        );
        pieces
    }

    fn tutorial_block(&self, view: &Tutorials, base: &str, key: &str, block: &Block, column: f32, cx: &mut Context<Self>) -> AnyElement {
        let body = |d: Div| d.text_size(text_px(14.5)).line_height(theme::body_line(24.));
        match block {
            Block::Paragraph(text) => body(div()).child(self.tutorial_text(key, text, cx)).into_any_element(),
            Block::Heading(text) => div().pt(px(6.)).text_size(text_px(15.5)).line_height(text_px(24.)).font_weight(FontWeight::SEMIBOLD).child(self.tutorial_text(key, text, cx)).into_any_element(),
            Block::List { ordered, items } => body(div().flex().flex_col().gap(px(4.)))
                .children(items.iter().enumerate().map(|(n, item)| {
                    div()
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .child(div().flex_none().min_w(px(16.)).text_right().text_color(Theme::text_tertiary()).child(if *ordered { format!("{}.", n + 1) } else { "•".into() }))
                        .child(div().flex_1().min_w_0().child(self.tutorial_text(&format!("{key}-{n}"), item, cx)))
                }))
                .into_any_element(),
            Block::Quote { cite, paragraphs } => body(div().flex().flex_col().gap(px(6.)).pl(px(14.)).border_l_2().border_color(Theme::line_strong()))
                .when(!cite.is_empty(), |d| d.child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(cite.clone())))
                .children(paragraphs.iter().enumerate().map(|(n, text)| div().text_color(Theme::text_secondary()).child(self.tutorial_text(&format!("{key}-{n}"), text, cx))))
                .into_any_element(),
            Block::Figure { src, alt, width, height, caption } => {
                let url = tutorial::resolve(base, src);
                let h = if *width > 0. { column * height / width } else { column * 0.63 };
                let missing = {
                    let alt = alt.clone();
                    move || div().size_full().flex().flex_col().items_center().justify_center().gap(px(6.)).p(px(24.)).text_center().text_size(text_px(12.5)).line_height(text_px(19.)).text_color(Theme::text_tertiary()).child(ui::icon("image", 20., Theme::text_tertiary())).child(alt.clone()).child(div().text_size(text_px(11.5)).child(t("截图在项目主页上，联网后可以看到"))).into_any_element()
                };
                let picture = match view.shots.get(&url) {
                    Some(Shot::Ready(path)) => img(path.clone()).w(px(column)).h(px(h)).with_fallback(missing).into_any_element(),
                    Some(Shot::Fetching) => div().size_full().flex().items_center().justify_center().child(ui::spinner(SharedString::from(format!("{key}-spin")), 18., Theme::text_tertiary())).into_any_element(),
                    _ => missing(),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .py(px(4.))
                    .child(div().w(px(column)).h(px(h)).rounded(px(8.)).border_1().border_color(Theme::line()).bg(Theme::bg_sunken()).overflow_hidden().child(picture))
                    .child(div().text_size(text_px(12.5)).line_height(theme::body_line(20.)).text_color(Theme::text_secondary()).child(self.tutorial_text(key, caption, cx)))
                    .into_any_element()
            }
            Block::Code { caption, text } => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .when(!caption.is_empty(), |d| d.child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(caption.clone())))
                .child(div().px(px(14.)).py(px(12.)).rounded(px(8.)).border_1().border_color(Theme::line()).bg(Theme::bg_sunken()).font_family(theme::MONO_FONT).text_size(text_px(12.)).line_height(text_px(19.)).child(self.selectable(SharedString::from(key.to_string()), text.clone(), cx)))
                .into_any_element(),
            Block::Note { warn, label, text } => body(div().flex().gap(px(10.)).px(px(14.)).py(px(10.)).rounded(px(8.)).bg(if *warn { Theme::danger_soft() } else { Theme::accent_soft() }))
                .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(if *warn { Theme::danger() } else { Theme::accent() }).child(label.clone()))
                .child(div().flex_1().min_w_0().child(self.tutorial_text(key, text, cx)))
                .into_any_element(),
            Block::Table { caption, head, rows } => {
                let widths: Vec<Vec<usize>> = rows.iter().map(|row| row.iter().map(|cell| wisp_core::markdown::width(&cell.text)).collect()).collect();
                let shares = wisp_core::markdown::column_shares(&widths);
                let lines: Vec<AnyElement> = rows
                    .iter()
                    .enumerate()
                    .map(|(r, row)| {
                        div()
                            .flex()
                            .w_full()
                            .border_color(Theme::line())
                            .when(r > 0, |d| d.border_t_1())
                            .when(r < *head, |d| d.bg(Theme::bg_sunken()).font_weight(FontWeight::MEDIUM))
                            .children(row.iter().enumerate().map(|(c, cell)| div().w(relative(shares.get(c).copied().unwrap_or(0.))).min_w_0().px(px(10.)).py(px(6.)).border_color(Theme::line()).when(c > 0, |d| d.border_l_1()).child(self.tutorial_text(&format!("{key}-{r}-{c}"), cell, cx))))
                            .into_any_element()
                    })
                    .collect();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().text_size(text_px(12.5)).text_color(Theme::text_secondary()).child(self.tutorial_text(&format!("{key}-caption"), caption, cx)))
                    .child(div().flex().flex_col().rounded(px(8.)).border_1().border_color(Theme::line()).overflow_hidden().text_size(text_px(13.)).line_height(theme::body_line(20.)).children(lines))
                    .into_any_element()
            }
        }
    }

    /// Text of a tutorial with its marks, selectable; a link goes to another tutorial or section here, anywhere else in the browser.
    fn tutorial_text(&self, key: &str, text: &Rich, cx: &mut Context<Self>) -> Div {
        let mut styles: Vec<(Range<usize>, HighlightStyle)> = vec![];
        let mut links = vec![];
        for (range, marks) in text.spans() {
            if let Some(href) = &marks.link {
                links.push((range.clone(), Some(Cmd::Tutorial(Do::Follow(href.clone())))));
            }
            let mut style = HighlightStyle::default();
            if marks.bold { style.font_weight = Some(FontWeight::SEMIBOLD) }
            if marks.italic { style.font_style = Some(FontStyle::Italic) }
            if marks.code { style.background_color = Some(Theme::groove()) }
            if style != HighlightStyle::default() { styles.push((range, style)) }
        }
        self.selectable_styled(SharedString::from(format!("tutorial-{key}")), text.text.clone(), &styles, links, cx)
    }
}
