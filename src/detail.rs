//! The detail panel of the selected paper: 信息 (record, statuses, custom
//! fields, notes, files), PDF (preview) and 提问 (questions about this paper).

use crate::i18n::{label, t};
use crate::app::{Cmd, Edge, MenuKind, Tab, Talk, Translated, Workspace};
use crate::read_aloud::Kind;
use crate::pdf::{Split, Tool, Zoom, ZoomTo};
use crate::theme::{self, Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use wisp_core::model::*;
use wisp_core::store::translation_key;
use wisp_core::{ai, time};
use wisp_core::floating as geo;
use wisp_core::panes::{self, Column, Pane, Placement};

/// The narrowest detail panel that shows a PDF and the paper's conversation side by side.
pub const SPLIT_MIN: f32 = 760.;

/// A pane's bar over its pages: one line of this height whatever the pane's width, as tall as the header of the
/// paper's conversation beside it, so that the lines under them run on as one (docs/dev/design.md: the frame
/// stays and the content gives way).
pub const BAR_H: f32 = 40.;
/// The levels the bar of the paper on show gives way by to stay one line, each with all before it: the tools by
/// their icons (the names in their tooltips); the zoom as its percentage alone; the tools under ⋯
/// (`Workspace::more_items`); the translation's count in a tooltip; the zoom's steps under ⋯ too and 译文 as its
/// icon; 留在旁边 and the translation's next step under ⋯.
pub const BAR_ICONS: usize = 1;
pub const BAR_SHORT_ZOOM: usize = 2;
pub const BAR_NO_TOOLS: usize = 3;
pub const BAR_BRIEF_FACING: usize = 4;
pub const BAR_NO_ZOOM: usize = 5;
pub const BAR_NO_KEEP: usize = 6;
const BAR_LEVELS: usize = 7;
/// What a bar keeps for its title at the least: the rest of a long title is cut with an ellipsis.
const BAR_TITLE_MIN: f32 = 72.;

/// What the bar of the paper on show draws (`Workspace::pdf_bar`).
struct PdfBar {
    width: f32,
    file_id: String,
    paper_id: String,
    file_title: String,
    /// The paper has other PDFs: the title is a menu of them.
    others: bool,
    page: usize,
    count: usize,
    percent: f32,
    zoom: Zoom,
    /// The fit the reader reads at, which a zoom by hand goes back to.
    fit: Zoom,
    cutting: bool,
    split: Split,
    pen: Tool,
    facing: bool,
    /// The paper's mind map stands in a column of the viewer.
    map: bool,
    /// The tools float at the page's edge (`Settings::pdf_tools_floating`).
    floating: bool,
    /// 分屏 自由 (`Settings::pdf_split_free`): the halves are dragged wider or narrower, each zoomed on its own.
    free: bool,
    can_pin: bool,
    keep: bool,
    /// Beside the conversation: the bar ends in the way to put the PDF away.
    beside: bool,
    aloud: Option<(Kind, crate::read_aloud::Status)>,
    /// One of several panes: the bar names its paper, as theirs do, and ends in the way to close this pane.
    paned: bool,
    /// The papers the window has no room for, when this is the last pane drawn (`Workspace::panes_behind`).
    hidden: Vec<String>,
}

/// The width a row of a bar takes: its parts', the gaps between them and the bar's own padding.
fn bar_need(parts: &[(AnyElement, f32)]) -> f32 {
    parts.iter().map(|part| part.1).sum::<f32>() + ui::BAR_GAP * parts.len().saturating_sub(1) as f32 + 24.
}

impl Workspace {
    pub fn render_detail(&mut self, width: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let Some(p) = lib.selected_paper() else { return div().into_any_element() };
        let tab = lib.tab;
        let reading = lib.reading;
        // Asked about with the PDF beside: the panel is what the narrow list leaves of the window (`Workspace::asking`).
        let asking = self.asking(window);
        // The edge at the panel's left is there to drag while something stands at its left.
        let edged = !reading && (!asking || self.ask_list_width(window) > 0.);
        // Beside the narrow list the one-line header is the title strip itself, level with the list's way back.
        let in_strip = asking && edged && ui::custom_titlebar();
        let has_pdf = lib.pdf_file(&p.id).is_some();
        let title = if lib.show_translation && let Some(Translated::Done(text)) = lib.translations.get(&translation_key(&p.title, ai::translation_prompt(&self.settings.translation_prompt))) { text } else { &p.title };
        // Asking about a paper: the conversation, and the paper's PDF to its right where the panel has room for both
        // (to its left once the reader swapped them, `Settings::pdf_first`).
        // A paper without one keeps the layout it is asked about in, with the ways to get its PDF in the PDF's place.
        let split = lib.split && tab == Tab::Ask && (has_pdf || asking) && width >= SPLIT_MIN;
        let pdf_first = self.settings.pdf_first;
        // Two panes side by side can change places; a conversation floating over the PDF has no side.
        // The button stays where it is whichever side each pane is on: a second click undoes the first. An id of
        // each order: the tooltip under the pointer is made anew after the click, saying what the next click does.
        let swap = (split && !(has_pdf && self.settings.floating.on)).then(|| {
            ui::ghost_button(if pdf_first { "split-swap-back" } else { "split-swap" }, Some("arrow-left-right"), None).h(text_px(26.)).min_w(text_px(26.)).occlude()
                .tooltip(ui::tip(if pdf_first { t("把 PDF 移回右边") } else { t("把 PDF 移到左边") }))
                .on_click(self.on(Cmd::SwapPanes, cx))
        });
        // 版面: how the papers on show are read, named by the layout in force. In the header row the panes share,
        // like the swap, so it stays where it is whatever it changes.
        let layout = {
            let mode = lib.layout_mode();
            ui::ghost_button("layout", None, None).h(text_px(26.)).px(px(6.)).text_size(text_px(12.)).occlude()
                .child(div().flex().items_center().gap(px(4.)).child(mode.map_or(t("版面"), |mode| t(mode.label()))).child(ui::icon("chevron-down", 11., Theme::text_tertiary())))
                .tooltip(ui::tip(t("版面：阅读、对照译文、提问、精读")))
                .on_click(self.on_menu(MenuKind::Layout, cx))
        };
        let tab_button = |id: &'static str, label: &'static str, value: Tab, this: &Self, cx: &mut Context<Self>| {
            let active = tab == value;
            div()
                .id(id)
                .flex()
                .items_center()
                .h_full()
                .px(px(4.))
                .border_b_2()
                .cursor_pointer()
                .occlude()
                .border_color(if active { Theme::accent() } else { gpui::transparent_black() })
                .text_color(if active { Theme::text() } else { Theme::text_tertiary() })
                .when(active, |d| d.font_weight(FontWeight::MEDIUM))
                .hover(|s| s.text_color(Theme::text()))
                .on_click(this.on(Cmd::SetTab(value), cx))
                .child(label)
        };
        // The paper's own two buttons, the last of its header in the panel and in reading mode alike, as a window's
        // are: the paper takes the window or gives it back, then the paper is closed. The way out of reading mode is
        // where the way in was, so the pointer need not move. An id of each: the tooltip is made anew after the click.
        let corner = {
            let key = crate::shortcuts::key_of("reading", &self.settings.shortcuts);
            let (id, icon, tip) = if reading {
                ("reading-leave", "shrink", tf!("退出阅读模式 {}", "Leave reading mode {}", crate::shortcuts::pretty("escape")))
            } else {
                ("reading-enter", "expand", tf!("阅读模式：整个窗口留给这篇文献 {}", "Reading mode: the whole window for this paper {}", crate::shortcuts::pretty(&key)).trim_end().to_string())
            };
            div().flex().flex_none().items_center().gap(px(6.))
                .child(ui::ghost_button(id, Some(icon), None).occlude().tooltip(ui::tip(tip)).on_click(self.on(Cmd::ToggleReading, cx)))
                .child(ui::ghost_button("detail-close", Some("x"), None).occlude().on_click(self.on(Cmd::CloseDetail, cx)))
        };
        // Reading mode, and a paper that is asked about, keep one line over the paper: what is read, the three tabs, and the way back.
        let header = if reading || asking {
            (if in_strip { ui::title_strip() } else { div().flex().flex_none().items_center().h(text_px(36.)) })
                .gap(px(16.))
                .px(px(16.))
                .when(in_strip && cfg!(target_os = "windows"), |d| d.pr(px(3. * ui::CONTROL_W + 8.)))
                .border_b_1()
                .border_color(Theme::line())
                .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(title.clone()))
                .child(tab_button("tab-info", t("信息"), Tab::Info, self, cx))
                .child(tab_button("tab-pdf", if has_pdf { "PDF" } else { t("PDF（无）") }, Tab::Pdf, self, cx))
                .child(tab_button("tab-ask", t("提问"), Tab::Ask, self, cx))
                .when(tab == Tab::Ask && has_pdf && !(lib.split && width >= SPLIT_MIN), |d| d.child(ui::ghost_button("beside-pdf", Some("file-text"), Some(t("对照 PDF").into())).h(text_px(26.)).text_size(text_px(12.)).occlude().on_click(self.on(Cmd::BesidePdf(true), cx))))
                .child(layout)
                .when_some(swap, |d, swap| d.child(swap))
                .child(if reading {
                    corner
                } else {
                    div().flex_none().child(ui::ghost_button("ask-leave", Some("x"), Some(tf!("退出提问 {}", "Leave chat {}", crate::shortcuts::pretty("escape")).into())).h(text_px(26.)).text_size(text_px(12.)).occlude().on_click(self.on(Cmd::LeaveAsk, cx)))
                })
        } else {
            // The title fills the band the column beside it uses for its header, and the tabs the band of the
            // table's column names under that: the lines under both run on across the window.
            let beside_table = lib.page == crate::app::Page::Papers && !lib.wide;
            // The table's column names have a line over them as well as under: beside them the title's line is that
            // upper one and the tabs, one pixel shorter, end on the lower one.
            let (band, tabs) = if beside_table { (Self::papers_head() + px(1.), text_px(32.) - px(1.)) } else { (ui::head_h(), text_px(32.)) };
            div()
            .flex()
            .flex_col()
            .flex_none()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(band)
                    .px(px(16.))
                    .border_b_1()
                    .border_color(Theme::line())
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_start()
                            .gap(px(6.))
                            .child(self.selectable("detail-title", title.clone(), cx).flex_1().min_w_0().line_clamp(if beside_table { 3 } else { 2 }).text_size(text_px(ui::HEAD_TITLE)).line_height(text_px(22.)).font_weight(FontWeight::SEMIBOLD))
                            .child(corner),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(16.))
                    .h(tabs)
                    .px(px(16.))
                    .border_b_1()
                    .border_color(Theme::line())
                    .bg(Theme::bg_sunken())
                    .child(tab_button("tab-info", t("信息"), Tab::Info, self, cx))
                    .child(tab_button("tab-pdf", if has_pdf { "PDF" } else { t("PDF（无）") }, Tab::Pdf, self, cx))
                    .child(tab_button("tab-ask", t("提问"), Tab::Ask, self, cx))
                    .child(div().flex_1())
                    // Asking with the PDF put away, or in a panel too narrow for both: one click brings it beside the conversation.
                    .when(tab == Tab::Ask && has_pdf && !(lib.split && width >= SPLIT_MIN), |d| d.child(ui::ghost_button("beside-pdf", Some("file-text"), Some(t("对照 PDF").into())).h(text_px(26.)).text_size(text_px(12.)).occlude().on_click(self.on(Cmd::BesidePdf(true), cx))))
                    .child(layout)
                    .when_some(swap, |d, swap| d.child(swap)),
            ) };
        let body = match tab {
            Tab::Ask if split && has_pdf && self.settings.floating.on => self.render_floating(width, window, cx),
            Tab::Ask if split => {
                // The conversation keeps its width on either side; the PDF has the rest.
                let chat_width = self.chat_width(width, window);
                let chat = self.render_chat(Talk::Paper, window, cx);
                let pdf = self.render_paper_area(width - chat_width, window, cx);
                // The edge between the two: drag it to give either more room, click it twice for the widths they came with.
                let held = self.resizing == Some(Edge::Split);
                let between = div()
                    .id("split-edge")
                    .group("split-edge")
                    .absolute()
                    .top_0()
                    .left(px(if pdf_first { width - chat_width } else { chat_width } - 5.))
                    .w(px(9.))
                    .h_full()
                    .flex()
                    .justify_center()
                    .cursor_col_resize()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        if event.click_count >= 2 {
                            this.resizing = None;
                            // With papers kept beside, the width dragged is this arrangement's (`Library::pane_chat`).
                            match this.lib.as_mut().filter(|lib| !lib.pins.is_empty()) {
                                Some(lib) => lib.pane_chat = 0.,
                                None => this.settings.chat_width = 0.,
                            }
                            this.save_settings(cx);
                        } else {
                            this.resizing = Some(Edge::Split);
                        }
                        cx.notify();
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
                    .child(div().w(px(2.)).h_full().when(held, |d| d.bg(Theme::accent())).when(!held, |d| d.group_hover("split-edge", |s| s.bg(Theme::accent().opacity(0.5)))));
                // Swapped, only the order changes: the PDF and the conversation are the same entities, drawn at the other side.
                let chat = div().flex_none().w(px(chat_width)).h_full().border_color(Theme::line()).map(|d| if pdf_first { d.border_l_1() } else { d.border_r_1() }).child(chat);
                let pdf = div().flex_1().min_w_0().h_full().child(pdf);
                div().relative().flex().size_full().map(|d| if pdf_first { d.child(pdf).child(chat) } else { d.child(chat).child(pdf) }).child(between).into_any_element()
            }
            Tab::Info => self.render_info(window, cx),
            Tab::Pdf => self.render_paper_area(width, window, cx),
            Tab::Ask => self.render_chat(Talk::Paper, window, cx),
        };
        // The panel's left edge: drag it to give the list or the paper more room, click it twice for the width it came with.
        let resizing = self.resizing == Some(Edge::Detail);
        let edge = div()
            .id("detail-edge")
            .group("detail-edge")
            .absolute()
            .top_0()
            .left(px(-4.))
            .w(px(9.))
            .h_full()
            .flex()
            .justify_center()
            .cursor_col_resize()
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                if event.click_count >= 2 {
                    this.resizing = None;
                    // Beside a paper that is asked about, the edge is the narrow list's.
                    if asking { this.settings.ask_list_width = 0. } else { this.settings.detail_width = 0. }
                    if let Some(lib) = this.lib.as_mut() { lib.wide = false }
                    this.save_settings(cx);
                } else {
                    this.resizing = Some(Edge::Detail);
                }
                cx.notify();
            }))
            // A tap is over before the layer that follows the pointer is drawn: the edge hears the end of it itself.
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
            .child(div().w(px(2.)).h_full().when(resizing, |d| d.bg(Theme::accent())).when(!resizing, |d| d.group_hover("detail-edge", |s| s.bg(Theme::accent().opacity(0.5)))));
        // Where the panel is the window (reading mode, or asked about without the list): no edge to drag, no line at its left.
        div().relative().flex().flex_col().flex_none().w(px(width)).h_full().bg(Theme::bg_body()).when(edged, |d| d.border_l_1().border_color(Theme::line())).when(!in_strip, |d| d.child(ui::title_strip())).child(header).child(div().flex().flex_col().flex_1().min_h_0().child(body)).when(edged, |d| d.child(edge)).into_any_element()
    }

    /// The translated abstract replaces its source in place. Until it is
    /// ready, keep the source readable alongside progress or a retry.
    fn render_translation(&self, p: &Paper, cx: &mut Context<Self>) -> Option<AnyElement> {
        let lib = self.lib.as_ref().filter(|l| l.show_translation)?;
        let prompt = ai::translation_prompt(&self.settings.translation_prompt);
        let texts: Vec<(String, &Translated)> = [&p.title, &p.abstract_text].into_iter().filter(|text| !text.trim().is_empty()).filter_map(|text| {
            let key = translation_key(text, prompt);
            lib.translations.get(&key).map(|state| (key, state))
        }).collect();
        if texts.is_empty() { return None }
        let abstract_key = translation_key(&p.abstract_text, prompt);
        let abstract_text = match lib.translations.get(&abstract_key) {
            Some(Translated::Done(text)) => text,
            _ => &p.abstract_text,
        };
        let mut card = div().flex().flex_col().gap(px(6.)).child(if abstract_text.is_empty() {
            div().text_color(Theme::text_placeholder()).child(t("没有摘要"))
        } else {
            self.selectable("detail-abstract", abstract_text.clone(), cx).text_color(Theme::text_secondary()).line_height(theme::body_line(21.))
        });
        if texts.iter().any(|(_, state)| matches!(state, Translated::Busy)) {
            card = card.child(div().flex().items_center().gap(px(6.)).text_size(text_px(12.)).text_color(Theme::accent()).child(ui::spinner("translating", 13., Theme::accent())).child(t("正在翻译…")));
        }
        if let Some(error) = texts.iter().find_map(|(_, state)| match state {
            Translated::Failed(error) => Some(error.clone()),
            _ => None,
        }) {
            card = card.child(div().flex().items_center().gap(px(8.)).child(div().flex_1().min_w_0().child(ui::error_line(&error))).child(ui::ghost_button("translate-retry", None, Some(t("重试").into())).on_click(self.on(Cmd::RetryTranslation, cx))));
        }
        Some(card.into_any_element())
    }

    fn render_info(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let Some(p) = lib.selected_paper() else { return div().into_any_element() };
        let id = p.id.clone();
        let section = |title: &'static str| div().flex().flex_col().gap(px(6.)).child(ui::caption(title));
        // A status the reader changes with one click.
        let pick = |element_id: &'static str, value: &str, text: &str, kind: MenuKind, this: &Self, cx: &mut Context<Self>| {
            let (color, fill) = theme::status_colors(value);
            div()
                .id(element_id)
                .flex()
                .flex_none()
                .items_center()
                .gap(px(3.))
                .h(text_px(24.))
                .pl(px(9.))
                .pr(px(6.))
                .rounded(px(12.))
                .bg(fill)
                .text_color(color)
                .text_size(text_px(12.))
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .on_click(this.on_menu(kind, cx))
                .child(text.to_string())
                .child(ui::icon("chevron-down", 11., color))
        };
        let link = p.link();
        let mut source_line = vec![];
        // `Nature 585(7825), 357–362`: where in the journal, when the record says.
        if !p.journal.is_empty() { source_line.push(format!("{} {}", p.journal, p.placed()).trim().to_string()) }
        if let Some(year) = p.year { source_line.push(year.to_string()) }
        if p.publication_status != "unknown" { source_line.push(label(&PUBLICATION_STATUSES, &p.publication_status).to_string()) }
        // The journal's impact factor and zones, when the table the app carries knows it; its categories, editions and the name it was found under on hover.
        let journal = wisp_core::journals::find(&p.journal).filter(|j| !crate::library::journal_line(j).is_empty() || !j.warning.is_empty());
        let metrics = journal.map(|j| {
            let mut about = vec![];
            if wisp_core::journals::key(j.name()) != wisp_core::journals::key(&p.journal) { about.push(tf!("按「{}」查到", "Matched as \"{}\"", j.name())) }
            if !j.quartile_category.is_empty() { about.push(tf!("JCR {}：{}", "JCR {}: {}", j.quartile, j.quartile_category)) }
            // The categories and the lists' names are the table's own words: a label table, read in the interface's language.
            if !j.cas_category.is_empty() { about.push(tf!("{}：{}", "{}: {}", t("中科院大类"), t(&j.cas_category))) }
            if !j.xr_category.is_empty() { about.push(tf!("{}：{}", "{}: {}", t("新锐大类"), t(&j.xr_category))) }
            about.push(t(wisp_core::journals::EDITIONS).to_string());
            div()
                .id("detail-journal")
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .text_size(text_px(12.))
                .text_color(Theme::text_tertiary())
                .child(crate::library::journal_line(j))
                .when(!j.warning.is_empty(), |d| d.child(div().text_color(Theme::warning()).child(t(&j.warning).to_string())))
                .tooltip(ui::tip(about.join("\n")))
        });
        let identifiers: Vec<AnyElement> = [("DOI", &p.doi, format!("https://doi.org/{}", p.doi)), ("PMID", &p.pmid, format!("https://pubmed.ncbi.nlm.nih.gov/{}/", p.pmid))]
            .into_iter()
            .filter(|(_, value, _)| !value.is_empty())
            .map(|(name, value, url)| div().flex().gap(px(6.)).text_size(text_px(12.)).child(div().flex_none().w(px(40.)).text_color(Theme::text_tertiary()).child(name)).child(self.selectable_link(name, value.clone(), url, cx).min_w_0()).into_any_element())
            .collect();
        let field_rows = self.render_field_rows(p, cx);
        let files = lib.paper_files(&id);
        // The file read in the viewer, and the numbers the attachments are cited by.
        let (shown, attachments) = (lib.pdf_file(&id).map(|f| f.id.clone()), lib.attachments(&id).iter().map(|f| f.id.clone()).collect::<Vec<_>>());
        let (main_rows, attachment_rows): (Vec<(bool, AnyElement)>, Vec<(bool, AnyElement)>) = files
            .iter()
            .enumerate()
            .map(|(i, file)| {
                let reading = lib.extracting.contains(&file.id);
                let shrinking = lib.shrinking.contains(&file.id);
                // A heavy PDF is offered to be made smaller, until it has been or turned out not to get smaller.
                let heavy = file.format == "pdf" && file.size_bytes >= wisp_core::shrink::HEAVY && !lib.shrunk.contains(&file.id) && !lib.no_smaller.contains(&file.id);
                // By a model's eyes, when the file gave no text of its own.
                let by_eye = lib.writing.sorting.reading && lib.writing.sorting.working.contains(&file.id);
                let text = lib.store.fulltext_pages(&file.id).ok().flatten();
                let failure = match &text {
                    Some(Err(error)) if !reading && !by_eye => Some(error.clone()),
                    _ => None,
                };
                let note = match &text {
                    _ if shrinking => t("正在压缩…").to_string(),
                    _ if reading => t("正在提取文字…").to_string(),
                    _ if by_eye => t("正在识别页面文字…").to_string(),
                    Some(Ok(pages)) => {
                        // How much of the file gave text: a page without any is a figure, or a page that could not be read.
                        let with_text = pages.iter().filter(|page| !page.trim().is_empty()).count();
                        let count = if with_text == pages.len() { tf!("{} 页文字可用于提问", "{} pages available for chat", pages.len()) } else { tf!("{} 页中 {with_text} 页有文字，可用于提问", "{with_text} of {} pages contain text for chat", pages.len()) };
                        match lib.writing.structures.get(&file.id).filter(|info| !info.text_by.is_empty()) {
                            Some(info) if info.read_in_part() => tf!("{count}（由 {} 读出，全文 {} 页，还有 {} 页没读）", "{count} (read by {}; {} pages total, {} unread)", info.text_by, info.page_count, info.page_count - info.pages_read),
                            Some(info) => tf!("{count}（由 {} 读出）", "{count} (read by {})", info.text_by),
                            None => count,
                        }
                    }
                    Some(Err(error)) => tf!("没读出文字：{}", "No text extracted: {}", t(wisp_core::fulltext::failure_brief(error))),
                    None => String::new(),
                };
                // A fetched file says where it is from.
                let note = match lib.origins.get(&file.id) {
                    Some((_, source, version)) => tf!("{note} · 来自 {}", "{note} · From {}", [source.as_str(), t(wisp_core::openaccess::version_label(version))].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ")),
                    None => note,
                };
                let small = |id: (&'static str, usize), text: &'static str| ui::secondary_button(id, text).h(text_px(24.)).px(px(8.)).text_size(text_px(12.));
                let row = div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(Theme::line())
                    .when(shown.as_deref() == Some(file.id.as_str()) && files.iter().filter(|f| f.format == "pdf").count() > 1, |d| d.border_color(Theme::accent()))
                    .on_mouse_down(MouseButton::Right, self.on_context(MenuKind::File(file.id.clone()), cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(if reading || by_eye || shrinking { ui::spinner(SharedString::from(format!("file-busy-{}", file.id)), 16., Theme::accent()).into_any_element() } else { ui::icon("file-text", 18., Theme::accent()).into_any_element() })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().truncate().child(file.title.clone()))
                                    .child(div().truncate().text_size(text_px(12.)).text_color(if failure.is_some() { Theme::warning() } else { Theme::text_tertiary() }).child(format!("{} · {}{}", match attachments.iter().position(|a| a == &file.id) { Some(n) => format!("S{}", n + 1), None if file.kind == "fulltext" => t("正文").to_string(), None => t("附件").to_string() }, file_size(file.size_bytes), if note.is_empty() { String::new() } else { format!(" · {note}") }))),
                            )
                            // A PDF is read in the viewer here, an attachment as well as the main text.
                            .when(heavy && !shrinking, |d| d.child(small(("file-compress", i), t("压缩")).tooltip(ui::tip(t("把过大的图片缩小，文字、页面和标注不变。原文件移到 removed 文件夹。"))).on_click(self.on(Cmd::CompressFile(file.id.clone()), cx))))
                            .when(file.format == "pdf", |d| d.child(small(("file-read", i), crate::i18n::tc("阅读·打开文件", "阅读")).on_click(self.on(Cmd::ReadFile(file.id.clone()), cx))))
                            .child(ui::ghost_button(("file-open", i), Some("link"), None).tooltip(ui::tip(t("用系统应用打开"))).on_click(self.on(Cmd::OpenFile(file.id.clone()), cx)))
                            .child(ui::ghost_button(("file-remove", i), Some("x"), None).on_click(self.on(Cmd::RemoveFile(file.id.clone()), cx))),
                    )
                    // A file without text: why, what it means for questions, and the two things to try.
                    .when_some(failure, |d, error| {
                        d.child(div().text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_secondary()).child(tf!("{}在读出文字之前，提问只能用到这篇文献的记录和摘要。", "{} Until text is extracted, questions use only this paper's record and abstract.", crate::i18n::text_failure(&error))))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(6.))
                                    .child(small(("file-reread", i), t("重新读取")).on_click(self.on(Cmd::RereadFile(file.id.clone()), cx)))
                                    .when(file.format == "pdf" && wisp_core::fulltext::readable_by_eye(&error), |d| d.child(small(("file-by-eye", i), t("识别页面文字")).on_click(self.on(Cmd::ReadPages(file.id.clone()), cx)))),
                            )
                            .when(lib.writing.sorting.reading && !lib.writing.sorting.error.is_empty(), |d| d.child(ui::error_line(&lib.writing.sorting.error)))
                    })
                    // Pages whose text was read off them and came back empty: those pages again, while any is.
                    .when_some(match &text { Some(Ok(pages)) if !by_eye && !reading && file.format == "pdf" => Some(wisp_core::ocr::blank_pages(lib.writing.structures.get(&file.id), pages).len()).filter(|n| *n > 0), _ => None }, |d, blank| {
                        d.child(div().flex().child(ui::secondary_button(("file-read-blank", i), tn!("识别没有文字的 {n} 页", "Recognise the {n} without text", blank, "page")).h(text_px(24.)).px(px(8.)).text_size(text_px(12.)).tooltip(ui::tip(t("只把这些页再送去识别，读出的文字放回原处；只有图的页面多半还是读不出文字。"))).on_click(self.on(Cmd::ReadPages(file.id.clone()), cx))))
                    })
                    // Pages a model read in part: the rest are read from where it stopped.
                    .when(!by_eye && file.format == "pdf" && lib.writing.structures.get(&file.id).is_some_and(|info| info.read_in_part()), |d| d.child(div().flex().child(small(("file-read-rest", i), t("读完剩下的页面")).on_click(self.on(Cmd::ReadPages(file.id.clone()), cx)))))
                    .into_any_element();
                (file.kind == "fulltext", row)
            })
            .partition(|(main, _)| *main);
        let (main_rows, attachment_rows): (Vec<AnyElement>, Vec<AnyElement>) = (main_rows.into_iter().map(|(_, row)| row).collect(), attachment_rows.into_iter().map(|(_, row)| row).collect());
        let has_fields = !lib.fields.is_empty();
        let translating = lib.show_translation;
        let abstract_body = self.render_translation(p, cx).unwrap_or_else(|| {
            if p.abstract_text.is_empty() { div().text_color(Theme::text_placeholder()).child(t("没有摘要")) } else { self.selectable("detail-abstract", p.abstract_text.clone(), cx).text_color(Theme::text_secondary()).line_height(theme::body_line(21.)) }.into_any_element()
        });
        let abstract_head = div()
            .flex()
            .items_center()
            .justify_between()
            .child(ui::caption(t("摘要")))
            .child(ui::ghost_button("translate", None, Some(if translating { t("显示原文").into() } else { t("显示译文").into() })).h(text_px(22.)).text_size(text_px(12.)).when(translating, |d| d.text_color(Theme::accent())).on_click(self.on(Cmd::ToggleTranslation, cx)));
        let notes_head = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(div().flex_1().min_w_0().child(ui::caption(t("我的笔记"))))
            .when_some(lib.note_links.get(&id).cloned(), |d, url| d.child(
                ui::ghost_button("note-link", Some("link"), Some("Notion".into()))
                    .h(text_px(22.)).px(px(6.)).text_size(text_px(12.))
                    .tooltip(|_, cx| cx.new(|_| crate::write_assist::Tip(t("打开 Notion").into())).into())
                    .on_click(self.on(Cmd::OpenUrl(url), cx)),
            ))
            .child(ui::ghost_button("note-export", None, Some(t("保存 / 导出").into()))
                .h(text_px(22.)).text_size(text_px(12.))
                .on_click(self.on(Cmd::NoteOpen(None, true), cx)));
        div()
            .id("detail-info")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&lib.detail_scroll)
            .p(px(16.))
            .gap(px(18.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(if p.authors.is_empty() { div().text_color(Theme::text_secondary()).child(t("作者待补")) } else { self.selectable("detail-authors", p.authors.clone(), cx).text_color(Theme::text_secondary()) })
                    .when(!source_line.is_empty(), |d| d.child(self.selectable("detail-source", source_line.join(" · "), cx).text_size(text_px(12.)).text_color(Theme::text_tertiary())))
                    .children(metrics)
                    .children(identifiers),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(pick("pick-reading", &p.reading, label(&READINGS, &p.reading), MenuKind::Reading(id.clone()), self, cx))
                    .child(pick("pick-screening", &p.screening, label(&SCREENINGS, &p.screening), MenuKind::Screening(id.clone()), self, cx))
                    .child(ui::status_chip(&p.fulltext, label(&FULLTEXTS, &p.fulltext)))
                    .child(ui::ghost_button("detail-star", Some("star"), Some(if p.starred { t("已收藏").into() } else { t("收藏").into() })).when(p.starred, |d| d.text_color(Theme::star())).on_click(self.on(Cmd::ToggleStar(id.clone()), cx))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(ui::secondary_button("detail-edit", t("编辑信息与词条")).on_click(self.on(Cmd::EditPaper(id.clone()), cx)))
                    // A record without a number of its own can be looked for by its title.
                    .when(p.doi.is_empty() && p.pmid.is_empty(), |d| d.child(ui::secondary_button("detail-complete", t("按题目补全")).on_click(self.on(Cmd::CompletePaper(id.clone()), cx))))
                    // One with its number and a gap in what it says can be completed from the record the number names.
                    .when((!p.doi.is_empty() || !p.pmid.is_empty()) && (p.journal.is_empty() || p.authors.is_empty() || p.year.is_none()), |d| d.child(ui::secondary_button("detail-complete-id", if p.doi.is_empty() { t("按 PMID 补全") } else { t("按 DOI 补全") }).on_click(self.on(Cmd::CompleteById(id.clone()), cx))))
                    .when(!link.is_empty(), |d| d.child(ui::secondary_button("detail-link", t("打开网页")).on_click(self.on(Cmd::OpenLink(id.clone()), cx))))
                    // Into another library, with its files and marks (transfer.rs); a conversation's paper has no other way there.
                    .when(!self.move_targets().is_empty(), |d| d.child(ui::secondary_button("detail-move", t("移到文献库…")).on_click(self.on(Cmd::AskMovePaper(id.clone()), cx))))
                    .child(ui::ghost_button("detail-trash", Some("bin"), Some(t("移到回收站").into())).h(text_px(32.)).on_click(self.on(Cmd::TrashPaper(id.clone()), cx))),
            )
            .child(section(t("标签")).child(if p.tags.is_empty() {
                div().text_color(Theme::text_placeholder()).child(t("没有标签"))
            } else {
                div().flex().flex_wrap().gap(px(4.)).children(p.tags.iter().map(|t| ui::chip(format!("#{t}"), Theme::accent(), Theme::accent_soft())))
            }))
            .child(section(t("自定义词条")).child(if has_fields {
                div().flex().flex_col().gap(px(8.)).children(field_rows)
            } else {
                div().text_color(Theme::text_placeholder()).child(t("这个文献库还没有自定义词条。在左下角「自定义词条」里添加，例如物种、研究方法、所属课题。"))
            }))
            .child(div().flex().flex_col().gap(px(6.)).child(abstract_head).child(abstract_body))
            .child(div().flex().flex_col().gap(px(6.)).child(notes_head).child(ui::field(&lib.notes, window, cx)))
            // The paper itself and what comes with it, each with its own way to add one.
            .child(section(t("正文 PDF")).child(div().flex().flex_col().gap(px(6.)).children(main_rows).child(div().flex().child(ui::secondary_button("attach-main", t("添加正文 PDF")).on_click(self.on(Cmd::AttachAs(id.clone(), "fulltext"), cx))))))
            .child(section(t("附件／补充材料")).child(div().flex().flex_col().gap(px(6.)).children(attachment_rows).when(files.iter().all(|f| f.kind == "fulltext"), |d| d.child(div().text_size(text_px(12.)).text_color(Theme::text_placeholder()).child(t("补图、补表、补充方法等文件放在这里；PDF 附件可以在阅读器里打开，提问时会和正文一起读。")))).child(div().flex().child(ui::secondary_button("attach-more", t("添加附件")).on_click(self.on(Cmd::AttachAs(id.clone(), "supplement"), cx))))))
            .child(div().text_size(text_px(12.)).text_color(Theme::text_placeholder()).child(tf!("来源：{} · 添加于 {}", "Source: {} · Added {}", crate::i18n::phrase(&p.source), time::date_text(&p.added_at))))
            .into_any_element()
    }

    /// The paper on show and those kept beside it (#194), each in a pane of its own laid out by
    /// `wisp_core::panes` in the order they stand in (`Library::pane_order`): side by side while the width has
    /// room for them. Where it has room for fewer, as many as fit are drawn, those next to each other that
    /// include the paper on show (`Library::panes_from`, moved no further than that takes), and the others are
    /// behind `+N` on the bar of the last pane drawn (a menu of them: one chosen is worked in, and so drawn).
    /// No pane changes its place for that. The pane of the paper on show is marked by a line over it, and a
    /// press in another pane makes that the paper on show where it is. With 译文 on, each paper has its translation as a
    /// column of the same width next to it where there is room (the viewer's two halves side by side: one
    /// scale, one scroll), else under it in its own pane.
    pub fn render_paper_area(&mut self, width: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(lib) = self.lib.as_mut() else { return div().into_any_element() };
        if lib.pins.is_empty() {
            (lib.main_at, lib.panes_from, lib.panes_shown) = (0, 0, 1);
            return self.render_pdf_tab(width, &[], window, cx);
        }
        lib.main_at = lib.main_at.min(lib.pins.len());
        // How many papers the width has room for side by side, and which: those from `panes_from` on, moved only as far as the paper on show needs.
        let all = lib.pins.len() + 1;
        let shown = panes::arrange(&vec![Column { pane: Pane::Paper, units: 1 }; all], width, 1.).iter().filter(|placed| placed.shown()).count().max(1);
        let from = lib.panes_from.min(all - shown).min(lib.main_at).max((lib.main_at + 1).saturating_sub(shown));
        (lib.panes_from, lib.panes_shown) = (from, shown);
        // Those panes in their order (None is the paper on show, else the n-th kept beside), each paper's column, its
        // translation's next to it while its viewer shows one (`owner` is the pane of each column), and where each is drawn.
        let laid_out = |lib: &crate::app::Library, cx: &App| {
            let mut panes: Vec<Option<usize>> = (0..lib.pins.len()).map(Some).collect();
            panes.insert(lib.main_at, None);
            let panes: Vec<Option<usize>> = panes.into_iter().skip(from).take(shown).collect();
            let (mut columns, mut owner) = (vec![], vec![]);
            for (k, pane) in panes.iter().enumerate() {
                columns.push(Column { pane: Pane::Paper, units: 1 });
                owner.push(k);
                let view = match pane { None => lib.pdf.as_ref(), Some(n) => Some(&lib.pins[*n].view) };
                if view.is_some_and(|view| view.read(cx).facing_beside()) {
                    columns.push(Column { pane: Pane::Translation, units: 1 });
                    owner.push(k);
                }
            }
            let placed = panes::arrange(&columns, width, 1.);
            (panes, columns, owner, placed)
        };
        let (panes, columns, owner, placed) = laid_out(lib, cx);
        // The papers with no room, in their order; the last pane drawn says how many and leads to them.
        let hidden = lib.panes_behind();
        let last = (0..columns.len()).rev().find(|&i| columns[i].pane == Pane::Paper && placed[i].shown());
        let views: Vec<Option<Entity<crate::pdf::PdfView>>> = panes.iter().map(|pane| match pane { None => lib.pdf.clone(), Some(n) => Some(lib.pins[*n].view.clone()) }).collect();
        let mut row = div().flex().size_full().overflow_hidden();
        let mut first = true;
        for (i, placement) in placed.iter().enumerate() {
            let Placement::Shown { width: own, .. } = *placement else { continue };
            if columns[i].pane != Pane::Paper { continue }
            // A translation of its own column widens the pane over both; else it stands under the pages.
            let translation = (i + 1 < columns.len() && columns[i + 1].pane == Pane::Translation && owner[i + 1] == owner[i]).then_some(i + 1);
            let wide = translation.and_then(|t| match placed[t] { Placement::Shown { width, .. } => Some(width + 1.), _ => None });
            let own = own + wide.unwrap_or(0.);
            let k = owner[i];
            if let Some(view) = &views[k] && view.read(cx).facing_beside() {
                view.update(cx, |pdf, cx| pdf.set_split(if wide.is_some() { Split::Side } else { Split::Stacked }, cx));
            }
            let behind: &[String] = if Some(i) == last { &hidden } else { &[] };
            let pane = match panes[k] {
                None => self.render_pdf_tab(own, behind, window, cx),
                Some(n) => self.render_pinned(n, own, behind, window, cx),
            };
            // The pane worked in: a line of the accent over it, where a tab has its line under it.
            let worked_in = panes[k].is_none();
            if !first { row = row.child(div().flex_none().w(px(1.)).h_full().bg(Theme::line())) }
            first = false;
            row = row.child(div().relative().flex_none().w(px(own)).h_full().overflow_hidden().child(pane).when(worked_in, |d| d.child(div().absolute().top_0().left_0().w_full().h(px(2.)).bg(Theme::accent()))));
        }
        row.into_any_element()
    }

    /// `+N` on the bar of the last pane drawn: the papers the window has no room for, as a menu to show one of
    /// them in that pane. With the width it takes.
    fn panes_behind(&self, hidden: &[String], window: &Window, cx: &mut Context<Self>) -> (AnyElement, f32) {
        let label = format!("+{}", hidden.len());
        let width = ui::text_width(&label, text_px(12.), window) + 27.;
        (div().id("panes-behind").flex().flex_none().items_center().gap(px(2.)).h(text_px(22.)).px(px(6.)).rounded(px(5.)).bg(Theme::bg_sunken()).cursor_pointer().text_color(Theme::text_secondary()).hover(|s| s.text_color(Theme::text())).child(label).child(ui::icon("chevron-down", 11., Theme::text_tertiary())).on_click(self.on_menu(MenuKind::Panes, cx)).into_any_element(), width)
    }

    /// The pane of a paper kept beside: a bar with its title, its page, its translation and the way to close it,
    /// over its PDF. A press in it, or on its title, makes it the paper on show where it is. One line as tall as
    /// every pane's (`BAR_H`): in a narrow pane the title is cut, then the translation's count goes into a tooltip.
    /// `hidden` are the papers with no room, which the last pane's bar leads to.
    fn render_pinned(&mut self, n: usize, width: f32, hidden: &[String], window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let Some(pin) = lib.pins.get(n) else { return div().into_any_element() };
        let (paper, view) = (pin.paper.clone(), pin.view.clone());
        let title = lib.paper(&paper).map_or(String::new(), |p| ui::one_line(&p.title));
        let library_chat = lib.page == crate::app::Page::Ask;
        view.update(cx, |pdf, _| {
            pdf.set_width(width);
            pdf.library_chat = library_chat;
            pdf.translates = true;
            pdf.maps = true;
        });
        let (page, count) = { let view = view.read(cx); (view.current_page(), view.page_count()) };
        let window: &Window = window;
        let wide = |text: &str| ui::text_width(text, text_px(12.), window);
        let pages = tf!("{page} / {count} 页", "{page} / {count} pages");
        // What the bar holds beside the translation: the title, the page, the papers with no room, the close.
        let fixed = |facing: Vec<(AnyElement, f32)>, cx: &mut Context<Self>| -> Vec<(AnyElement, f32)> {
            let mut parts = vec![(div().id(SharedString::from(format!("pin-title-{paper}"))).flex_shrink().min_w_0().truncate().cursor_pointer().text_color(Theme::text_secondary()).hover(|s| s.text_color(Theme::text())).child(title.clone()).tooltip(ui::tip(title.clone())).on_click(self.on(Cmd::WorkInPane(paper.clone()), cx)).into_any_element(), BAR_TITLE_MIN)];
            parts.push((div().flex_1().into_any_element(), 0.));
            if count > 0 { parts.push((div().flex_none().child(pages.clone()).into_any_element(), wide(&pages))) }
            parts.extend(facing);
            if !hidden.is_empty() { parts.push(self.panes_behind(hidden, window, cx)) }
            parts.push((ui::ghost_button(SharedString::from(format!("pin-close-{paper}")), Some("x"), None).h(text_px(24.)).min_w(px(24.)).tooltip(ui::tip(t("关掉这一栏"))).on_click(self.on(Cmd::ClosePane(paper.clone()), cx)).into_any_element(), 24.));
            parts
        };
        // The translation's count and next step while they have room, else the step alone, else neither.
        let facing = self.pinned_facing_controls(n, false, window, cx);
        let mut parts = fixed(facing, cx);
        if bar_need(&parts) > width {
            let facing = self.pinned_facing_controls(n, true, window, cx);
            parts = fixed(facing, cx);
        }
        if bar_need(&parts) > width { parts = fixed(vec![], cx) }
        let bar = div().flex().flex_none().items_center().gap(px(ui::BAR_GAP)).h(text_px(BAR_H)).px(px(12.)).overflow_hidden().border_b_1().border_color(Theme::line()).text_size(text_px(12.)).text_color(Theme::text_tertiary()).children(parts.into_iter().map(|part| part.0));
        div().flex().flex_col().size_full().child(bar).child(div().flex_1().min_h_0().child(view)).into_any_element()
    }

    pub fn render_pdf_tab(&mut self, width: f32, hidden: &[String], window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let Some(p) = lib.selected_paper() else { return div().into_any_element() };
        let Some(file) = lib.pdf_file(&p.id) else {
            let looking = lib.fetching.as_ref().is_some_and(|run| run.running() && (run.working.contains(&p.id)));
            let last = lib.fetching.as_ref().and_then(|run| run.rows.iter().rev().find(|row| row.paper_id == p.id)).map(|row| row.detail.clone()).filter(|detail| !detail.is_empty());
            return ui::empty_state("file-text", t("这篇文献还没有 PDF"), t("添加后可以在这里预览，全文也会用于提问。也可以替你找开放获取的版本：只取能免费合法获取的，查找时会把 DOI 或 PMID 发给 OpenAlex、Europe PMC 和出版商。"))
                .child(div().flex().flex_wrap().gap(px(8.)).child(ui::primary_button("pdf-attach", t("添加 PDF")).on_click(self.on(Cmd::AttachPdf(p.id.clone()), cx))).child(if looking { ui::secondary_button("pdf-find", t("正在查找…")) } else { ui::secondary_button("pdf-find", t("查找开放全文")).on_click(self.on(Cmd::FindPdfs(Some(p.id.clone())), cx)) }))
                .when_some(last, |d, detail| d.child(div().max_w(px(420.)).text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_tertiary()).child(detail)))
                .size_full()
                .into_any_element();
        };
        let Some(pdf) = lib.pdf.clone() else { return div().into_any_element() };
        let (file_id, file_title, paper_id) = (file.id.clone(), file.title.clone(), p.id.clone());
        // The paper's PDFs beyond the one on show: the title becomes a menu of them, saying which is read.
        let attachments = lib.attachments(&p.id);
        let number = attachments.iter().position(|f| f.id == file_id).map(|i| i + 1);
        let others = lib.paper_files(&p.id).iter().filter(|f| f.format == "pdf").count() > 1;
        // Among the panes of several papers each bar names its paper, whichever is worked in: the title over them names one only.
        let paned = !lib.pins.is_empty();
        let paper_title = ui::one_line(&p.title);
        let file_title = match number {
            Some(n) if paned => tf!("附件 S{n} · {paper_title}", "Attachment S{n} · {paper_title}"),
            None if paned => paper_title,
            Some(n) => tf!("附件 S{n} · {file_title}", "Attachment S{n} · {file_title}"),
            None if others => tf!("正文 · {file_title}", "Main text · {file_title}"),
            None => file_title,
        };
        let beside = lib.split && lib.tab == Tab::Ask;
        let can_pin = lib.pins.len() < crate::app::MAX_PINS && !lib.pins.iter().any(|pin| pin.paper == p.id);
        let facing = lib.facing;
        // 朗读 or 播客: how the reading of this file stands, while it is read.
        let aloud = lib.playing_here(&file_id).and_then(|kind| Some((kind, lib.aloud_shown.clone()?)));
        // With the library's conversation on show as well, a selection can go to either.
        let library_chat = lib.page == crate::app::Page::Ask;
        // Another file of the paper in the other half: each half is a viewer of its own, drawn in its place
        // whichever is worked in.
        let pair = lib.beside.as_ref().map(|beside| (beside.view.clone(), beside.split, beside.first));
        let pdf_height = lib.pdf_area(cx).1;
        // 自由, the halves share the room as the band between them was dragged; 固定, evenly.
        let free = self.settings.pdf_split_free;
        let share = if free { lib.split_share } else { 0.5 };
        let room = (width - crate::pdf::DIVIDER).max(0.);
        let first_width = (room * share).round();
        let widths = match &pair {
            // The viewer worked in, the other one.
            Some((_, Split::Side, first)) => if *first { (room - first_width, first_width) } else { (first_width, room - first_width) },
            _ => (width, width),
        };
        for (view, half_width) in std::iter::once((&pdf, widths.0)).chain(pair.as_ref().map(|(view, ..)| (view, widths.1))) {
            view.update(cx, |pdf, cx| {
                pdf.set_width(half_width);
                pdf.library_chat = library_chat;
            pdf.translates = true;
            pdf.maps = true;
                // Alone in the area, the translation goes beside or under the pages by the width; beside other panes the layout places it (`render_paper_area`).
                if !paned { pdf.fit_facing(cx) }
            });
        }
        let (page, count, percent, zoom, cutting, split, pen, marks_open) = {
            let view = pdf.read(cx);
            (view.current_page(), view.page_count(), view.percent(), view.zoom(), view.cutting(), pair.as_ref().map_or(view.split(), |(_, split, _)| *split), view.tool(), view.annotations_open())
        };
        let viewer = match pair {
            None => pdf.into_any_element(),
            Some((other, split, first)) => {
                let (one, two) = if first { (other, pdf) } else { (pdf, other) };
                // The half worked in (the toolbar, the keys and the wheel's zoom speak of it) has a line of the accent along its top.
                let worked = if first { 1 } else { 0 };
                let half = |view: Entity<crate::pdf::PdfView>, grow: f32, at: usize| {
                    let mut half = div().relative().flex_basis(px(0.)).min_w_0().min_h_0().overflow_hidden().child(view);
                    half.style().flex_grow = Some(grow);
                    half.when(at == worked, |d| d.child(div().absolute().top_0().left_0().w_full().h(px(2.)).bg(Theme::accent())))
                };
                // The same band as between the halves of one file split.
                let stacked = split == Split::Stacked;
                let line = div().flex_none().bg(Theme::line_strong());
                let line = if stacked { line.h(px(crate::pdf::DIVIDER)).w_full() } else { line.w(px(crate::pdf::DIVIDER)).h_full() };
                let halves = div().size_full().flex().when(stacked, |d| d.flex_col()).child(half(one, share, 0)).child(line).child(half(two, 1. - share, 1));
                // 自由: the band is taken and dragged; twice, the halves are even again. Over both halves, so neither takes the press.
                let grip = free.then(|| {
                    let held = self.resizing == Some(Edge::Halves);
                    let length = if stacked { (pdf_height - crate::pdf::DIVIDER).max(0.) } else { room };
                    let at = (length * share).round() + crate::pdf::DIVIDER / 2.;
                    let grip = div().id("pdf-halves-edge").group("pdf-halves-edge").absolute().flex().justify_center().items_center().occlude();
                    let grip = if stacked { grip.left_0().w_full().top(px(at - 4.5)).h(px(9.)).cursor_row_resize() } else { grip.top_0().h_full().left(px(at - 4.5)).w(px(9.)).cursor_col_resize() };
                    let mark = div().when(held, |d| d.bg(Theme::accent())).when(!held, |d| d.group_hover("pdf-halves-edge", |s| s.bg(Theme::accent().opacity(0.5))));
                    let mark = if stacked { mark.h(px(2.)).w_full() } else { mark.w(px(2.)).h_full() };
                    grip.on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        let Some(lib) = this.lib.as_mut() else { return };
                        if event.click_count >= 2 {
                            this.resizing = None;
                            lib.split_share = 0.5;
                        } else {
                            let along = if stacked { f32::from(event.position.y) } else { f32::from(event.position.x) };
                            lib.halves_drag = Some((along, lib.split_share, length));
                            this.resizing = Some(Edge::Halves);
                        }
                        cx.notify();
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
                    .child(mark)
                });
                div().size_full().relative().child(halves).children(grip).into_any_element()
            }
        };
        // The tools stand in the toolbar, or float at the page's right edge: a habit, kept for every paper.
        let floating = self.settings.pdf_tools_floating;
        let pdf_map = self.lib.as_ref().and_then(|lib| lib.pdf.as_ref()).is_some_and(|pdf| pdf.read(cx).map_shown());
        let bar = PdfBar { width, file_id: file_id.clone(), paper_id, file_title, others, page, count, percent, zoom, fit: lib.zoom, cutting, split, pen, facing, map: pdf_map, floating, free, can_pin, keep: lib.keep, beside, aloud, paned, hidden: hidden.to_vec() };
        // One line whatever the width: what it has no room for gives way level by level, the last of them under ⋯.
        let mut level = 0;
        let parts = loop {
            let parts = self.pdf_bar(&bar, level, window, cx);
            if bar_need(&parts) <= width || level + 1 >= BAR_LEVELS { break parts }
            level += 1;
        };
        if let Some(lib) = self.lib.as_mut() { lib.bar_level = level }
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(div().flex().flex_none().items_center().gap(px(ui::BAR_GAP)).h(text_px(BAR_H)).px(px(12.)).overflow_hidden().border_b_1().border_color(Theme::line()).text_size(text_px(12.)).text_color(Theme::text_tertiary()).children(parts.into_iter().map(|part| part.0)))
            .child(div().flex_1().min_h_0().relative().child(viewer).children(self.player_on_page(&file_id, width, cx)).when(floating && count > 0, |d| d.child(self.pdf_tool_column(file_id, width, pdf_height, cutting, split, pen, marks_open, cx))))
            .into_any_element()
    }

    /// The bar of the paper on show at a `level` of giving way (`BAR_ICONS` …), each part with the width it
    /// takes: which file is read, the page, the zoom, 译文, the tools, 留在旁边, the ways out. The title takes
    /// what the others leave and is cut where that is less than its words.
    fn pdf_bar(&self, bar: &PdfBar, level: usize, window: &Window, cx: &mut Context<Self>) -> Vec<(AnyElement, f32)> {
        let wide = |text: &str| ui::text_width(text, text_px(12.), window);
        let (named, tools, zooms, keeps) = (level < BAR_ICONS, level < BAR_NO_TOOLS, level < BAR_NO_ZOOM, level < BAR_NO_KEEP);
        let has = bar.count > 0;
        let docked = has && !bar.floating && tools;
        // A thing to click by its name (`icon` before it), or, once names have no room, by `mark` alone with the name in its tooltip.
        let button = |id: &'static str, icon: Option<&'static str>, mark: &'static str, label: &str, lit: bool, tip: Option<String>, named: bool| -> (Stateful<Div>, f32) {
            if named {
                let button = ui::ghost_button(id, icon, Some(label.to_string().into())).h(text_px(24.)).px(px(6.)).text_size(text_px(12.)).when(lit, |d| d.text_color(Theme::accent())).when_some(tip, |d, tip| d.tooltip(ui::tip(tip)));
                (button, wide(label) + 12. + if icon.is_some() { 21. } else { 0. })
            } else {
                (ui::ghost_button(id, None, None).h(text_px(24.)).min_w(px(24.)).child(ui::icon(mark, 16., if lit { Theme::accent() } else { Theme::text_secondary() })).tooltip(ui::tip(tip.unwrap_or_else(|| label.to_string()))), 24.)
            }
        };
        let mut parts: Vec<(AnyElement, f32)> = vec![];
        let mut put = |part: (Stateful<Div>, f32)| parts.push((part.0.into_any_element(), part.1));
        // Which file is read, and the way to the others: 正文 and each attachment.
        put((if bar.others {
            div().id("pdf-files").flex().flex_shrink().min_w_0().max_w(px(320.)).items_center().gap(px(4.)).cursor_pointer().rounded(px(5.)).hover(|s| s.text_color(Theme::text_secondary())).child(div().min_w_0().truncate().child(bar.file_title.clone())).child(ui::icon("chevron-down", 11., Theme::text_tertiary())).on_click(self.on_menu(MenuKind::PdfFiles, cx))
        } else {
            div().id("pdf-files").flex_shrink().min_w_0().truncate().child(bar.file_title.clone()).tooltip(ui::tip(bar.file_title.clone()))
        // Among several panes, the one worked in has its title in the colour of what is read.
        }.when(bar.paned, |d| d.text_color(Theme::text()).font_weight(FontWeight::MEDIUM)), BAR_TITLE_MIN));
        // The title is only as wide as it reads; the space after it is no button.
        put((div().id("pdf-bar-space").flex_1(), 0.));
        if has {
            let pages = tf!("{} / {} 页", "{} / {} pages", bar.page, bar.count);
            let width = wide(&pages);
            put((div().id("pdf-pages").flex_none().child(pages), width));
            // Smaller, the size in force (a menu of sizes), larger; the whole page in one click; zoomed by hand, the
            // way back to the fit the reader reads at. In a narrow bar the size alone: its menu and ⋯ have the rest.
            let fit_label = |zoom: Zoom| if zoom == Zoom::FitPage { t("适应整页") } else { t("适应宽度") };
            let size = if bar.zoom.fits() && level < BAR_SHORT_ZOOM { format!("{} · {}%", fit_label(bar.zoom), bar.percent.round()) } else { format!("{}%", bar.percent.round()) };
            let step = |id: &'static str, icon: &'static str| ui::ghost_button(id, Some(icon), None).h(text_px(24.)).min_w(px(24.));
            let back = (zooms && !bar.zoom.fits() && bar.fit != Zoom::FitPage).then(|| fit_label(bar.fit));
            let width = wide(&size) + 12. + if zooms { 72. + back.map_or(0., |back| wide(back) + 12.) } else { 0. };
            put((div()
                .id("pdf-zoom-group")
                .flex()
                .flex_none()
                .items_center()
                .when(zooms, |d| d.child(step("pdf-zoom-out", "minus").on_click(self.on(Cmd::PdfZoom(ZoomTo::Out), cx))))
                .child(ui::ghost_button("pdf-zoom", None, Some(size.into())).h(text_px(24.)).px(px(6.)).text_size(text_px(12.)).on_click(self.on_menu(MenuKind::PdfZoom, cx)))
                .when(zooms, |d| {
                    d.child(step("pdf-zoom-in", "plus").on_click(self.on(Cmd::PdfZoom(ZoomTo::In), cx)))
                        // The whole page in one click from any size; in force it is marked, and a click brings the page's top back.
                        .child(ui::ghost_button("pdf-zoom-page", None, None).h(text_px(24.)).min_w(px(24.)).child(ui::icon("scan", 16., if bar.zoom == Zoom::FitPage { Theme::accent() } else { Theme::text_secondary() })).tooltip(ui::tip(t("适应整页"))).on_click(self.on(Cmd::PdfZoom(ZoomTo::Set(Zoom::FitPage)), cx)))
                })
                .when_some(back, |d, back| d.child(ui::ghost_button("pdf-zoom-fit", None, Some(back.into())).h(text_px(24.)).px(px(6.)).text_size(text_px(12.)).on_click(self.on(Cmd::PdfZoom(ZoomTo::Set(bar.fit)), cx)))), width));
            // How the paper is read, a switch on what it changes: the pages again beside themselves, their
            // paragraphs translated in place. Lit while on, with how far the translation is and the one thing to do next.
            let (switch, width) = button("pdf-facing", Some("languages"), "languages", t("译文"), bar.facing, Some(if bar.facing { t("收起译文") } else { t("对照译文：左边原文，右边把每段换成译文的页面") }.to_string()), zooms);
            put((switch.on_click(self.on(Cmd::ToggleFacing, cx)), width));
            // The paper's mind map, a column more beside the pages: made by hand, or drafted by the model and then changed.
            let (map, width) = button("pdf-map", Some("git-branch"), "git-branch", t("导图"), bar.map, Some(if bar.map { t("收起思维导图") } else { t("思维导图：在旁边一栏整理这篇文献的要点，节点可以连回原文") }.to_string()), zooms);
            put((map.on_click(self.on(Cmd::PdfMap, cx)), width));
        }
        drop(put);
        if has && keeps { parts.extend(self.facing_controls(level >= BAR_BRIEF_FACING, window, cx)) }
        let mut put = |part: (Stateful<Div>, f32)| parts.push((part.0.into_any_element(), part.1));
        if docked {
            // The same file twice, side by side or one over the other: a figure in one half, the page that speaks of it in the other.
            // With 译文 on it says where the translation stands, on the pages themselves among the places.
            let replaced = bar.facing && self.settings.facing_in_place;
            let split_label = match bar.split { _ if replaced => t("原地替换"), Split::Off => t("分屏"), Split::Side => t("左右分屏"), Split::Stacked => t("上下分屏"), Split::Triple => t("三栏对照") };
            let (split, width) = button("pdf-split", None, "columns-2", split_label, replaced || bar.split != Split::Off, None, named);
            put((split.on_click(self.on_menu(MenuKind::PdfSplit, cx)), width));
            // Split, how the halves are held: 固定, even and zoomed together; 自由, the band dragged and each half zoomed on its own.
            if bar.split != Split::Off && !bar.facing && !bar.map {
                let (label, icon, tip) = if bar.free { (t("自由"), "lock-open", t("自由：拖动中间的分隔线调整两边宽度；点一下某一边，缩放只改这一边。点此改回固定")) } else { (t("固定"), "lock", t("固定：两边一样宽，缩放一起变。点此改为自由：分隔线可以拖动，两边各自缩放")) };
                let (hold, width) = button("pdf-split-free", Some(icon), icon, label, bar.free, Some(tip.to_string()), named);
                put((hold.on_click(self.on(Cmd::PdfSplitFree(!bar.free), cx)), width));
            }
            // Cut a piece out of a page (a figure, a table): to keep in the figure library, or as a picture for the next question.
            let (cut, width) = button("pdf-cut", None, "scissors", if bar.cutting { t("取消截图") } else { t("截图") }, bar.cutting, None, named);
            put((cut.on_click(self.on(Cmd::PdfCut, cx)), width));
            // Marks placed on the page rather than its text: a box round a figure or a passage, a sticky note anywhere.
            let (boxed, width) = button("pdf-box", Some("square"), "square", t("方框"), bar.pen == Tool::Box, Some(t("在页面上拖出方框，框住图表或文字").to_string()), named);
            put((boxed.on_click(self.on(Cmd::PdfTool(Tool::Box), cx)), width));
            let (note, width) = button("pdf-note", Some("sticky-note"), "sticky-note", t("便签"), bar.pen == Tool::Note, Some(t("点击页面任意位置添加便签").to_string()), named);
            put((note.on_click(self.on(Cmd::PdfTool(Tool::Note), cx)), width));
            let (text, width) = button("pdf-text", Some("type"), "type", t("文本框"), bar.pen == Tool::Text, Some(t("在页面上拖出文本框，直接在框里写字").to_string()), named);
            put((text.on_click(self.on(Cmd::PdfTool(Tool::Text), cx)), width));
        }
        drop(put);
        // 朗读 from the page in view, 播客 of the whole paper; while one plays (the player at the pages' foot says where),
        // its button pauses it and goes on, and stays on the bar when the other tools have gone under ⋯.
        if has && !bar.floating {
            let kept = self.lib.as_ref().is_some_and(|lib| crate::app::podcast_files(lib, &bar.paper_id).0.is_some());
            for kind in [Kind::Aloud, Kind::Podcast] {
                let playing = bar.aloud.as_ref().is_some_and(|(playing, _)| *playing == kind);
                if !tools && !playing { continue }
                parts.push(self.read_aloud_button(kind, bar.aloud.as_ref(), kept, if playing { !zooms } else { !named }, window, cx));
            }
            if tools && let Some(save) = self.podcast_save_button("pdf-podcast-save", &bar.paper_id, 24., cx) { parts.push((save, 24.)) }
        }
        let mut put = |part: (Stateful<Div>, f32)| parts.push((part.0.into_any_element(), part.1));
        // 留在旁边: this paper stays in a pane of its own when the next one is chosen (#194). Lit while it waits for that one.
        if has && bar.can_pin && keeps {
            let tip = if bar.keep { t("已留住：再选一篇文献，这篇留在旁边的一栏里。再点一下取消") } else { t("留住这篇：再选别的文献时，它留在旁边的一栏里，两篇并排对照") };
            let (keep, width) = button("pdf-pin", Some("pin"), "pin", t("留在旁边"), bar.keep, Some(tip.to_string()), named);
            put((keep.on_click(self.on(Cmd::KeepPane, cx)), width));
        }
        // This paper in a window of its own: the library's window is free for the list (#194).
        if has && tools {
            let (apart, width) = button("pdf-window", None, "app-window", t("新窗口"), false, Some(t("在新窗口中阅读：文献库的窗口留给列表和别的文献").to_string()), named);
            put((apart.on_click(self.on(Cmd::PaperWindow(bar.paper_id.clone()), cx)), width));
        }
        // Floating, the column has it; a file that could not be read keeps it here, the way to read it elsewhere.
        if (!bar.floating && tools) || !has {
            let (open, width) = button("pdf-open", Some("link"), "link", t("用系统应用打开"), false, None, named && bar.width >= 700.);
            put((open.on_click(self.on(Cmd::OpenFile(bar.file_id.clone()), cx)), width));
        }
        // Where the tools stand; the same click in the same place puts them back.
        if has && tools { put((ui::ghost_button("pdf-tools-float", None, None).h(text_px(24.)).min_w(px(24.)).child(ui::icon("panel-right", 16., if bar.floating { Theme::accent() } else { Theme::text_secondary() })).tooltip(ui::tip(if bar.floating { t("把工具放回工具栏") } else { t("工具悬浮在页面右侧") })).on_click(self.on(Cmd::FloatPdfTools(!bar.floating), cx)), 24.)) }
        // What the bar has no room for.
        if has && !tools { put((ui::ghost_button("pdf-more", Some("ellipsis"), None).h(text_px(24.)).min_w(px(24.)).tooltip(ui::tip(t("更多工具"))).on_click(self.on_menu(MenuKind::PdfMore, cx)), 24.)) }
        drop(put);
        // The papers the window has no room for, on the last pane's bar.
        if !bar.hidden.is_empty() { parts.push(self.panes_behind(&bar.hidden, window, cx)) }
        let mut put = |part: (Stateful<Div>, f32)| parts.push((part.0.into_any_element(), part.1));
        // One of several panes: close this one, the next is worked in. Else, beside the conversation: put the PDF away again.
        if bar.paned {
            put((ui::ghost_button("pdf-pane-close", Some("x"), None).h(text_px(24.)).min_w(px(24.)).tooltip(ui::tip(t("关掉这一栏"))).on_click(self.on(Cmd::ClosePane(bar.paper_id.clone()), cx)), 24.));
        } else if bar.beside {
            put((ui::ghost_button("pdf-beside-close", Some("x"), None).h(text_px(24.)).min_w(px(24.)).on_click(self.on(Cmd::BesidePdf(false), cx)), 24.));
        }
        drop(put);
        parts
    }

    /// 朗读 or 播客 in the PDF's toolbar: from the page in view (朗读) or the whole paper talked over by
    /// two hosts (播客; the one `kept`, when there is one). While it plays, the same button, lit and with the
    /// same words, pauses it and goes on (a spinner while the next sound, or a podcast's script, is on its
    /// way); the player at the pages' foot has the rest. `brief` is its mark alone, what it says in its
    /// tooltip. With the width it takes on the bar.
    fn read_aloud_button(&self, kind: Kind, aloud: Option<&(Kind, crate::read_aloud::Status)>, kept: bool, brief: bool, window: &Window, cx: &mut Context<Self>) -> (AnyElement, f32) {
        let podcast = kind == Kind::Podcast;
        let (id, icon, cmd) = if podcast { ("pdf-podcast", "headphones", Cmd::Podcast) } else { ("pdf-read-aloud", "volume-2", Cmd::ReadAloud) };
        let button = ui::ghost_button(id, None, None).h(text_px(24.)).gap(px(4.)).text_size(text_px(12.)).on_click(self.on(cmd, cx));
        let button = if brief { button.min_w(px(24.)) } else { button.px(px(6.)) };
        let label = if podcast { t("播客") } else { t("朗读") };
        let tip = match aloud.filter(|(playing, _)| *playing == kind) {
            None if podcast && kept => t("播放这篇存下的播客，从上次停下的地方接着放；要重新生成，在播放器的 ⋯ 里"),
            None if podcast => t("一女一男两位主持人聊这篇论文：模型先写对话稿，再用 DeepSeek 的两种声音读出来。离开这篇也接着播，窗口底部有回来的入口；全部生成后，声音和稿子存为这篇的附件，旁边的按钮可以下载"),
            None => t("从这一页起朗读，读到参考文献为止；读到哪一句，页面上就标出哪一句"),
            Some((_, status)) if status.paused => t("继续"),
            Some(_) => t("暂停"),
        };
        let width = if brief { 24. } else { ui::text_width(label, text_px(12.), window) + 30. };
        let button = match aloud.filter(|(playing, _)| *playing == kind) {
            None => button.child(ui::icon(icon, 14., Theme::text_secondary())).when(!brief, |d| d.child(label)),
            Some((_, status)) => button
                .text_color(Theme::accent())
                .child(if status.waiting && !status.paused { ui::spinner(if podcast { "pdf-podcast-wait" } else { "pdf-read-aloud-wait" }, 14., Theme::accent()).into_any_element() } else { ui::icon(icon, 14., Theme::accent()).into_any_element() })
                .when(!brief, |d| d.child(label)),
        };
        (button.tooltip(ui::tip(tip)).into_any_element(), width)
    }

    /// Beside 播客, once a podcast of the paper is kept: a menu to save its sound or its script elsewhere.
    pub(crate) fn podcast_save_button(&self, id: &'static str, paper: &str, size: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let lib = self.lib.as_ref()?;
        let (sound, script) = crate::app::podcast_files(lib, paper);
        if sound.is_none() && script.is_none() { return None }
        Some(ui::ghost_button(id, None, None).size(px(size)).child(ui::icon("export", if size >= 28. { 16. } else { 14. }, Theme::text_secondary())).tooltip(ui::tip(t("下载播客的音频或稿子"))).on_click(self.on_menu(MenuKind::PodcastFiles(paper.to_string()), cx)).into_any_element())
    }

    /// The PDF's tools as a column of icons at the page's right edge, under 标注 (or beside its list
    /// while that is open): at hand wherever the page is read, the names in the tooltips.
    #[allow(clippy::too_many_arguments)]
    fn pdf_tool_column(&self, file_id: String, width: f32, pdf_height: f32, cutting: bool, split: Split, pen: Tool, marks_open: bool, cx: &mut Context<Self>) -> AnyElement {
        let playing = self.lib.as_ref().and_then(|lib| lib.playing_here(&file_id));
        let (aloud, podcast) = (playing == Some(Kind::Aloud), playing == Some(Kind::Podcast));
        // While one plays its button pauses it and goes on.
        let paused = self.lib.as_ref().and_then(|lib| lib.aloud_shown.as_ref()).is_some_and(|status| status.paused);
        let pause_tip = if paused { "继续" } else { "暂停" };
        let tool = |id: &'static str, icon: &'static str, lit: bool, tip: &'static str| ui::ghost_button(id, None, None).size(px(28.)).child(ui::icon(icon, 16., if lit { Theme::accent() } else { Theme::text_secondary() })).tooltip(ui::tip(t(tip)));
        let line = || div().flex_none().h(px(1.)).mx(px(4.)).my(px(2.)).bg(Theme::line());
        // Where the reader dragged it, or under 标注; there, it stands to the left of the list of marks
        // while that is open and leaves room (as wide as `PdfView::annotation_panel` makes it).
        let area = (width, pdf_height);
        let (mut right, top) = geo::tools_at(self.settings.pdf_tools_at, area);
        let list = (width - 16.).clamp(0., 360.);
        if self.settings.pdf_tools_at.is_none() && marks_open && width - list - 16. >= geo::TOOLS.0 + 8. { right = list + 16. }
        let held = self.lib.as_ref().and_then(|lib| lib.float_drag).is_some_and(|drag| drag.held_tools());
        let split_tip = match split { Split::Off => "分屏", Split::Side => "左右分屏", Split::Stacked => "上下分屏", Split::Triple => "三栏对照" };
        let free = self.settings.pdf_split_free;
        let facing = self.lib.as_ref().is_some_and(|lib| lib.facing_shown());
        div()
            .absolute()
            .top(px(top))
            .right(px(right))
            .w(px(geo::TOOLS.0))
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(3.))
            .rounded(px(8.))
            .bg(Theme::bg_body())
            .border_1()
            .border_color(Theme::line_strong())
            // A press on the column is not a press on the page: no selection, cut or mark starts under it.
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // The grip: drag the column anywhere over the page; twice, it goes home under 标注.
            .child(
                div()
                    .id("pdf-float-grip")
                    .flex()
                    .flex_none()
                    .justify_center()
                    .items_center()
                    .h(px(10.))
                    .rounded(px(4.))
                    .cursor(if held { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
                    .hover(|s| s.bg(Theme::hover()))
                    .child(div().w(px(14.)).h(px(2.)).rounded(px(1.)).bg(Theme::line_strong()))
                    .tooltip(ui::tip(t("拖动以移动，双击放回原处")))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.tools_press(event, cx);
                    })),
            )
            .child(tool("pdf-float-split", "columns-2", split != Split::Off, split_tip).on_click(self.on_menu(MenuKind::PdfSplit, cx)))
            .when(split != Split::Off && !facing, |d| d.child(if free {
                tool("pdf-float-split-free", "lock-open", true, "自由：拖动中间的分隔线调整两边宽度；点一下某一边，缩放只改这一边。点此改回固定").on_click(self.on(Cmd::PdfSplitFree(false), cx))
            } else {
                tool("pdf-float-split-free", "lock", false, "固定：两边一样宽，缩放一起变。点此改为自由：分隔线可以拖动，两边各自缩放").on_click(self.on(Cmd::PdfSplitFree(true), cx))
            }))
            .child(tool("pdf-float-cut", "scissors", cutting, if cutting { "取消截图" } else { "截图" }).on_click(self.on(Cmd::PdfCut, cx)))
            .child(tool("pdf-float-box", "square", pen == Tool::Box, "在页面上拖出方框，框住图表或文字").on_click(self.on(Cmd::PdfTool(Tool::Box), cx)))
            .child(tool("pdf-float-note", "sticky-note", pen == Tool::Note, "点击页面任意位置添加便签").on_click(self.on(Cmd::PdfTool(Tool::Note), cx)))
            .child(tool("pdf-float-text", "type", pen == Tool::Text, "在页面上拖出文本框，直接在框里写字").on_click(self.on(Cmd::PdfTool(Tool::Text), cx)))
            .child(tool("pdf-float-read", "volume-2", aloud, if aloud { pause_tip } else { "从这一页起朗读" }).on_click(self.on(Cmd::ReadAloud, cx)))
            .child(tool("pdf-float-podcast", "headphones", podcast, if podcast { pause_tip } else { "播客：两位主持人聊这篇论文" }).on_click(self.on(Cmd::Podcast, cx)))
            .children(self.lib.as_ref().and_then(|lib| lib.files.iter().find(|f| f.id == file_id)).map(|f| f.paper_id.clone()).and_then(|paper| self.podcast_save_button("pdf-float-podcast-save", &paper, 28., cx)))
            .child(line())
            .child(tool("pdf-float-open", "link", false, "用系统应用打开").on_click(self.on(Cmd::OpenFile(file_id), cx)))
            .into_any_element()
    }
}
