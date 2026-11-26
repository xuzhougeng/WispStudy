//! 全库查找 (#273): the words in the list's search box looked for through the whole library, exactly, in
//! every full text and attachment, in the reading marks (the text marked and what was written on them), in
//! the notes, titles and abstracts (`wisp_core::find`). Enter in the search box, the offer at its end or the
//! palette shows the places where the list is, by paper, each with its page and its sentence; a click opens
//! the PDF there with the sentence or the mark lit, or the notes. Esc, or 返回列表, is the list again.
//! The search runs on the background executor, a stale one stops, and nothing is written.

use crate::app::{Cmd, Page, Tab, Workspace};
use crate::context::Nav;
use crate::i18n::t;
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use wisp_core::annotation::Kind;
use wisp_core::find::{self, Found, Hit, Place};
use wisp_core::store::Store;

/// The places of one paper listed at most; the paper's row says how many it has.
const PER_PAPER: usize = 30;
/// While the places are on show the search follows what is typed, after this pause.
const PAUSE: Duration = Duration::from_millis(350);
/// Characters of a passage shown ahead of the words, so they stay in view on one line.
const AHEAD: usize = 36;

#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// Look for what the search box holds (Enter in it, the offer at its end).
    Open,
    /// Look for these words: the palette's row. They are put in the search box first.
    Query(String),
    /// Back to the list.
    Close,
    /// The n-th row: a paper is shown, a place gone to.
    Go(usize),
}

enum Row {
    /// A paper: its id, how many places it has, how many are listed.
    Paper(String, usize, usize),
    /// A place: its index in `Found::hits`.
    Hit(usize),
}

#[derive(Default)]
pub struct Finding {
    /// The places are on show in the list's place.
    pub open: bool,
    /// The words asked for last, and whether that search is still going.
    asked: String,
    running: bool,
    /// The answer on show, and the words it is for.
    found: Option<Found>,
    shown: String,
    rows: Vec<Row>,
    /// The row the arrow keys are on.
    cursor: Option<usize>,
    /// Counts the searches asked: one whose number is no longer this stops, and its answer is dropped.
    asks: Arc<AtomicU64>,
    scroll: UniformListScrollHandle,
}

impl Finding {
    fn rows_of(found: &Found) -> Vec<Row> {
        let mut rows = vec![];
        for (n, hit) in found.hits.iter().enumerate() {
            if n == 0 || found.hits[n - 1].paper_id != hit.paper_id {
                let listed = found.hits[n..].iter().take_while(|h| h.paper_id == hit.paper_id).count();
                rows.push(Row::Paper(hit.paper_id.clone(), found.counts.get(&hit.paper_id).copied().unwrap_or(listed), listed));
            }
            rows.push(Row::Hit(n));
        }
        rows
    }
}

impl Workspace {
    pub(crate) fn run_find(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        match what {
            Do::Open => {
                let query = lib.search.read(cx).text().trim().to_string();
                if query.is_empty() { return lib.search.read(cx).focus(window) }
                self.turn_page(Page::Papers, None, window, cx);
                let Some(lib) = self.lib.as_mut() else { return };
                // Enter on a row the arrows chose goes there; else the words are looked for (again).
                if lib.finding.open && lib.finding.asked == query && !lib.finding.running && let Some(at) = lib.finding.cursor { return self.run_find(Do::Go(at), window, cx) }
                self.find_start(query, false, cx);
            }
            Do::Query(words) => {
                // The list under the places is narrowed by the same words, as if they were typed.
                lib.search.update(cx, |input, cx| input.set_text(&words, cx));
                lib.search.read(cx).focus(window);
                lib.filters.search = words.clone();
                lib.refilter();
                self.turn_page(Page::Papers, None, window, cx);
                self.find_start(words.trim().to_string(), false, cx);
            }
            Do::Close => {
                lib.finding.asks.fetch_add(1, Ordering::Relaxed);
                lib.finding = Finding { asks: lib.finding.asks.clone(), ..Finding::default() };
            }
            Do::Go(at) => {
                lib.finding.cursor = Some(at);
                match lib.finding.rows.get(at) {
                    Some(Row::Paper(id, ..)) => {
                        let id = id.clone();
                        self.run(Cmd::Select(id), window, cx);
                    }
                    Some(Row::Hit(n)) => {
                        let Some(hit) = lib.finding.found.as_ref().and_then(|found| found.hits.get(*n)).cloned() else { return };
                        self.go_to_hit(hit, window, cx);
                    }
                    None => {}
                }
            }
        }
        cx.notify();
    }

    /// The search box changed while the places are on show: they follow it after a pause; emptied, the list is back.
    pub(crate) fn find_typed(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut().filter(|lib| lib.finding.open) else { return };
        if text.trim().is_empty() {
            lib.finding.asks.fetch_add(1, Ordering::Relaxed);
            lib.finding = Finding { asks: lib.finding.asks.clone(), ..Finding::default() };
            return cx.notify();
        }
        self.find_start(text.trim().to_string(), true, cx);
    }

    /// Look for `query` on the background executor, after `PAUSE` when `typed`; a newer search stops it.
    fn find_start(&mut self, query: String, typed: bool, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let ask = lib.finding.asks.fetch_add(1, Ordering::Relaxed) + 1;
        if !lib.finding.open { lib.finding.scroll = UniformListScrollHandle::new() }
        (lib.finding.open, lib.finding.asked, lib.finding.running) = (true, query.clone(), true);
        let (library, dir, asks) = (lib.info.id.clone(), lib.info.dir.clone(), lib.finding.asks.clone());
        let executor = cx.background_executor().clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            if typed {
                executor.timer(PAUSE).await;
                if asks.load(Ordering::Relaxed) != ask { return }
            }
            let current = asks.clone();
            let found = executor
                .spawn(async move {
                    let store = Store::open(&dir).map_err(|e| format!("{e:#}"))?;
                    find::in_library(&store, &query, None, find::Texts::All, PER_PAPER, &|| current.load(Ordering::Relaxed) != ask).map_err(|e| format!("{e:#}"))
                })
                .await;
            this.update(cx, |this, cx| {
                // The library closed meanwhile, or a newer search asked: this answer is not wanted.
                let Some(lib) = this.lib.as_mut().filter(|lib| lib.info.id == library) else { return };
                if asks.load(Ordering::Relaxed) != ask { return }
                match found {
                    Ok(found) if !found.stopped => {
                        let finding = &mut lib.finding;
                        let moved = finding.shown != finding.asked;
                        (finding.rows, finding.shown, finding.running) = (Finding::rows_of(&found), finding.asked.clone(), false);
                        finding.found = Some(found);
                        if moved {
                            finding.cursor = None;
                            finding.scroll.scroll_to_item(0, ScrollStrategy::Top);
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        lib.finding.running = false;
                        this.fail(error, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open the place a hit names: the PDF at its page with the sentence or the mark lit, or the record and its notes.
    fn go_to_hit(&mut self, hit: Hit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let id = hit.paper_id.clone();
        // The file is a PDF of the paper's; the main text is named by none, as a citation names it.
        let pdf = lib.paper_files(&id).into_iter().any(|f| f.id == hit.file && f.format == "pdf");
        let file = (lib.main_pdf(&id).map(|f| f.id.as_str()) != Some(hit.file.as_str())).then(|| hit.file.clone());
        match hit.place {
            Place::Text if pdf => self.run(Cmd::GotoFile(id, file, Some(hit.page), hit.passage.clone()), window, cx),
            Place::Mark if pdf => {
                self.show_file(&id, file, cx);
                self.select(Some(id), window, cx);
                if let Some(lib) = self.lib.as_mut() { lib.tab = Tab::Pdf }
                self.ensure_pdf(cx);
                let mark = hit.mark.clone();
                // A highlight or an underline is lit line by line; a mark placed on the page is gone to as it is drawn.
                let lines = mark.as_ref().filter(|m| matches!(m.kind, Kind::Highlight | Kind::Underline)).map(|m| m.boxes.clone()).unwrap_or_default();
                let top = mark.as_ref().and_then(|m| m.boxes.first()).map_or(0., |b| b[1]);
                if let Some(pdf) = self.lib.as_ref().and_then(|l| l.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.show_mark(hit.page, top, lines, cx)) }
            }
            Place::Notes => {
                self.run(Cmd::OpenPaper(id, Tab::Info), window, cx);
                // The words in the notes selected, with the keyboard there.
                if let Some(lib) = self.lib.as_ref() {
                    let notes = lib.notes.clone();
                    notes.update(cx, |input, cx| {
                        let words = hit.at.clone();
                        if input.text().get(words.clone()).is_some() { input.select_range(words, cx) }
                    });
                    notes.read(cx).focus(window);
                }
            }
            _ => self.run(Cmd::OpenPaper(id, Tab::Info), window, cx),
        }
    }

    /// The arrow keys and Enter while the places are on show.
    pub(crate) fn find_nav(&mut self, nav: Nav, window: &mut Window, cx: &mut Context<Self>) {
        let Some(finding) = self.lib.as_mut().map(|lib| &mut lib.finding) else { return };
        let count = finding.rows.len();
        if count == 0 { return }
        let at = match (nav, finding.cursor) {
            (Nav::Down, None) | (Nav::First, _) => 0,
            (Nav::Down, Some(at)) => (at + 1).min(count - 1),
            (Nav::Up, at) => at.unwrap_or(1).saturating_sub(1),
            (Nav::Last, _) => count - 1,
            (Nav::Open, Some(at)) => return self.run_find(Do::Go(at), window, cx),
            _ => return,
        };
        finding.cursor = Some(at);
        finding.scroll.scroll_to_item(at, ScrollStrategy::Center);
        cx.notify();
    }

    /// The offer at the end of the search box: look for its words in the full texts and the marks too.
    pub(crate) fn find_offer(&self, width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let lib = self.lib.as_ref()?;
        if lib.finding.open || lib.search.read(cx).text().trim().is_empty() { return None }
        let words = width >= f32::from(text_px(520.));
        let tip = t("在全文、附件、标注和笔记里逐字查找这个词（Enter）");
        Some(
            div()
                .id("find-everywhere")
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .h(text_px(24.))
                .px(px(7.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(text_px(12.))
                .text_color(Theme::text_tertiary())
                .hover(|s| s.bg(Theme::hover()).text_color(Theme::text_secondary()))
                .child(ui::icon("file-text", 13., Theme::text_tertiary()))
                .when(words, |d| d.child(t("查全文与标注")).child(div().text_color(Theme::text_placeholder()).child("↵")))
                .tooltip(ui::tip(tip))
                .on_click(self.on(Cmd::LibraryFind(Do::Open), cx))
                .into_any_element(),
        )
    }

    /// What the foot of the list says under the places: how many full texts were looked through.
    pub(crate) fn found_foot(&self) -> Option<String> {
        let found = self.lib.as_ref().filter(|lib| lib.finding.open)?.finding.found.as_ref()?;
        Some(tn!("查了 {n} 篇的全文", "Searched the full text of {n}", found.searched, "paper"))
    }

    /// The line over the places, where the column names are over the list: what was looked for and how much came of it.
    pub(crate) fn render_found_head(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(lib) = self.lib.as_ref() else { return div().into_any_element() };
        let finding = &lib.finding;
        let said = match finding.found.as_ref().filter(|_| !finding.shown.is_empty()) {
            Some(found) if found.total > 0 => {
                let papers = found.counts.len();
                let (places, papers) = (tn!("{n} 处", "{n}", found.total, "place"), tn!("{n} 篇", "{n}", papers, "paper"));
                tf!("「{}」：{}，{}", "“{}”: {} in {}", finding.shown, places, papers)
            }
            Some(_) => tf!("「{}」", "“{}”", finding.shown),
            None => tf!("正在查找「{}」…", "Looking for “{}”…", finding.asked),
        };
        // Papers with a PDF whose text was not read: what they say was not looked through.
        let unseen = finding.found.as_ref().map_or(0, |found| found.textless.len() + found.unread.len());
        let unseen_tip = finding.found.as_ref().map(|found| tf!("{} 篇的 PDF 没有读出文字（扫描件、加密或无文字层），{} 篇还没有读取全文：它们的正文这次没有查到，没找到不说明里面没有。可以在文献详情里「识别页面文字」，或按「全文」筛选出「PDF 无文字」。", "{} papers have a PDF that gave no text (scanned, locked or without a text layer) and {} have not been read yet: their text was not searched, so “not found” does not mean they lack the words. Use “Read page text” in a paper's details, or filter Full text by “PDF without text”.", found.textless.len(), found.unread.len()));
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .h(text_px(32.))
            .px(px(16.))
            .border_t_1()
            .border_b_1()
            .border_color(Theme::line())
            .bg(Theme::bg_sunken())
            .text_size(text_px(12.))
            .text_color(Theme::text_secondary())
            .when(finding.running, |d| d.child(ui::spinner("find-running", 13., Theme::text_tertiary())))
            .child(div().flex_1().min_w_0().truncate().child(ui::one_line(&said)))
            .when(unseen > 0, |d| {
                d.child(div().id("find-unseen").flex_none().text_color(Theme::text_tertiary()).child(tn!("{n} 篇读不出正文", "{n} unread", unseen, "PDF")).when_some(unseen_tip, |d, tip| d.tooltip(ui::tip(tip))))
            })
            .child(ui::ghost_button("find-close", Some("x"), Some(t("返回列表").into())).h(text_px(24.)).text_size(text_px(12.)).tooltip(ui::tip(t("回到文献列表（Esc）"))).on_click(self.on(Cmd::LibraryFind(Do::Close), cx)))
            .into_any_element()
    }

    /// The places, by paper, in the list's place.
    pub(crate) fn render_found(&self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let Some(lib) = self.lib.as_ref() else { return div().into_any_element() };
        let finding = &lib.finding;
        match finding.found.as_ref() {
            None => return div().flex().flex_1().into_any_element(),
            Some(found) if found.hits.is_empty() => {
                let hint = if found.textless.len() + found.unread.len() > 0 { tn!("题目、摘要、笔记、标注和已读出文字的全文里都没有这几个字。另有 {n} 篇 PDF 没有读出文字，没有查到。", "Not in any title, abstract, note, mark or full text that was read; the text of {n} could not be searched.", found.textless.len() + found.unread.len(), "PDF") } else { t("题目、摘要、笔记、标注和全文里都没有这几个字。").to_string() };
                return ui::empty_state("search", &tf!("没有找到「{}」", "“{}” was not found", finding.shown), &hint).child(ui::secondary_button("find-back", t("返回列表")).on_click(self.on(Cmd::LibraryFind(Do::Close), cx))).into_any_element();
            }
            Some(_) => {}
        }
        let count = finding.rows.len();
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(uniform_list("found", count, cx.processor(move |this, range: Range<usize>, _window, cx| this.render_found_rows(range, width, cx))).track_scroll(finding.scroll.clone()).flex_1())
            .child(crate::scrollbar::scrollbar("found-bar", finding.scroll.clone()))
            .into_any_element()
    }

    fn render_found_rows(&mut self, range: Range<usize>, width: f32, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        let finding = &lib.finding;
        let Some(found) = finding.found.as_ref() else { return vec![] };
        let label_w = if crate::i18n::is_english() { 124. } else { 108. };
        range
            .filter_map(|i| {
                let row = div().id(("found-row", i)).w_full().h(text_px(30.)).flex().items_center().gap(px(10.)).px(px(16.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).when(finding.cursor == Some(i), |d| d.bg(Theme::accent_soft())).on_click(self.on(Cmd::LibraryFind(Do::Go(i)), cx));
                Some(match finding.rows.get(i)? {
                    Row::Paper(id, count, listed) => {
                        let title = lib.papers.iter().find(|p| &p.id == id).map_or(String::new(), |p| p.title.clone());
                        let title = if title.is_empty() { t("（无题目）").to_string() } else { ui::one_line(&title) };
                        let said = if count > listed { tf!("{} 处，列出 {}", "{} places, {} listed", count, listed) } else { tn!("{n} 处", "{n}", *count, "place") };
                        row.when(i > 0, |d| d.border_t_1().border_color(Theme::line()))
                            .child(div().flex_1().min_w_0().truncate().text_size(text_px(13.)).font_weight(FontWeight::MEDIUM).text_color(Theme::text()).child(title))
                            .child(div().flex_none().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(said))
                            .into_any_element()
                    }
                    Row::Hit(n) => {
                        let hit = found.hits.get(*n)?;
                        let (text, words) = hit.shown(if width < 560. { AHEAD / 2 } else { AHEAD });
                        let lit = HighlightStyle { background_color: Some(Theme::found()), color: Some(Theme::text()), ..Default::default() };
                        row.pl(px(28.))
                            .child(div().flex_none().w(text_px(label_w)).truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(self.place_label(hit)))
                            .child(div().flex_1().min_w_0().truncate().text_size(text_px(13.)).text_color(Theme::text_secondary()).child(StyledText::new(text).with_highlights([(words, lit)])))
                            .into_any_element()
                    }
                })
            })
            .collect()
    }

    /// Where in the paper a place is, in a few words: 第 5 页, S1 第 2 页, 高亮 · 第 3 页, 笔记.
    fn place_label(&self, hit: &Hit) -> String {
        let attachment = self.lib.as_ref().and_then(|lib| lib.attachments(&hit.paper_id).iter().position(|f| f.id == hit.file)).map(|n| n + 1);
        let page = match attachment {
            Some(n) => tf!("S{} 第 {} 页", "S{} p. {}", n, hit.page),
            None => tf!("第 {} 页", "p. {}", hit.page),
        };
        match hit.place {
            Place::Title => t("题目").to_string(),
            Place::Abstract => t("摘要").to_string(),
            Place::Notes => t("笔记").to_string(),
            Place::Text => page,
            Place::Mark => {
                let Some(mark) = hit.mark.as_ref() else { return page };
                let kind = match mark.kind {
                    Kind::Highlight => t("高亮"),
                    Kind::Underline => t("下划线"),
                    Kind::Box => t("方框"),
                    Kind::Note => t("便签"),
                    Kind::Text => t("文本框"),
                    Kind::Cut => t("标记"),
                };
                // What was written on a highlight or an underline, apart from the text it marks.
                if mark.written && matches!(mark.kind, Kind::Highlight | Kind::Underline) { tf!("{}的批注 · {}", "{} note · {}", kind, page) } else { tf!("{} · {}", "{} · {}", kind, page) }
            }
        }
    }
}
