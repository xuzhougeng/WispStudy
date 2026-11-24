//! The PDF views of a library and what they tell the window: the paper on show, the papers kept beside it
//! (`Pinned`), a file in the other half (`Beside`), a paper's own window (`PaperWindow`), 版面, and the mind
//! map as a column. `pdf_event` is where a viewer's events (a selection, a mark, a figure number followed)
//! become the window's. The viewer itself is `pdf.rs`.

use crate::app::{Cmd, Library, Page, Tab, Talk, Workspace};
use crate::chat_state::{ChatQuote, MAX_QUOTES, Picture};
use crate::listening::pages_of;
use crate::i18n::t;
use crate::mindmap::{MapEvent, MindMapView};
use crate::pdf::{PdfEvent, PdfView, Shows, Split, Zoom};
use gpui::*;
use wisp_core::model::*;
use wisp_core::store::Store;

/// A paper kept in a pane of its own beside the paper on show (#194): choosing another paper leaves it
/// where it is, for reading two side by side. Laid out with the paper on show by `wisp_core::panes`.
pub struct Pinned {
    pub paper: String,
    pub view: Entity<PdfView>,
    _events: Subscription,
    /// Its translation's paragraphs while 译文 is on (`Workspace::load_pinned_facing`), as `Library::facing_text` is the paper on show's.
    pub facing: Option<crate::facing::Facing>,
}

/// A paper read in a window of its own (#194, `reader.rs`): the viewer is made and kept here, so what it does
/// goes where the PDF in the panel's goes.
pub struct PaperWindow {
    pub paper: String,
    pub view: Entity<PdfView>,
    pub(crate) _events: Subscription,
    pub window: WindowHandle<crate::reader::Reader>,
}

/// The most papers pinned beside the one on show: a layout has at most `MAX_COLUMNS` panes.
pub const MAX_PINS: usize = wisp_core::panes::MAX_COLUMNS - 1;

/// The other half of a viewer split between two files of a paper (#185): the main text in one half, a
/// supplement in the other, each a `PdfView` of its own with its own place, marks and selection.
pub struct Beside {
    /// The viewer not worked in now; `Library::pdf` is the one that is. The two change roles when the pointer goes to work in this one.
    pub view: Entity<PdfView>,
    _events: Subscription,
    /// Side by side or one over the other; never `Off`.
    pub split: Split,
    /// `view` is drawn in the first half, at the left or the top.
    pub first: bool,
}

impl Library {
    /// The stored PDF to preview and read: the one chosen (`read_file`), else the first full-text PDF, else any PDF.
    pub fn pdf_file(&self, id: &str) -> Option<&PaperFile> {
        let files = self.paper_files(id);
        let chosen = self.read_file.get(id).and_then(|chosen| files.iter().find(|f| &f.id == chosen && f.format == "pdf"));
        chosen.or(files.iter().find(|f| f.kind == "fulltext" && f.format == "pdf")).or(files.iter().find(|f| f.format == "pdf")).copied()
    }

    /// The paper's main-text PDF, whatever is read now.
    pub fn main_pdf(&self, id: &str) -> Option<&PaperFile> {
        let files = self.paper_files(id);
        files.iter().find(|f| f.kind == "fulltext" && f.format == "pdf").or(files.iter().find(|f| f.format == "pdf")).copied()
    }

    /// The paper's PDF attachments, in the order `S1`, `S2`, … count them (`Store::attachment_pages`).
    pub fn attachments(&self, id: &str) -> Vec<&PaperFile> {
        self.paper_files(id).into_iter().filter(|f| f.kind != "fulltext" && f.format == "pdf").collect()
    }

    /// The papers on show in the order of their panes: those kept beside and, among them, the paper on show.
    pub fn pane_order(&self) -> Vec<String> {
        let mut order: Vec<String> = self.pins.iter().map(|pin| pin.paper.clone()).collect();
        if let Some(id) = &self.selected { order.insert(self.main_at.min(order.len()), id.clone()) }
        order
    }

    /// The papers on show that the window has no room for, in the order of their panes (`panes_from`, `panes_shown`).
    pub fn panes_behind(&self) -> Vec<String> {
        let drawn = self.panes_from..self.panes_from + self.panes_shown.max(1);
        self.pane_order().into_iter().enumerate().filter(|(at, _)| !drawn.contains(at)).map(|(_, id)| id).collect()
    }

    /// Put the panes in `order`: the papers kept beside, and the paper on show where it stands among them.
    /// A paper the order does not name comes last.
    pub fn order_panes(&mut self, order: &[String]) {
        let at = |id: &str| order.iter().position(|named| named == id).unwrap_or(usize::MAX);
        self.pins.sort_by_key(|pin| at(&pin.paper));
        let main = self.selected.as_deref().map_or(usize::MAX, at);
        self.main_at = self.pins.iter().filter(|pin| at(&pin.paper) < main).count();
    }

    /// Put the viewer away, both its halves while it shows two files.
    pub fn close_pdf(&mut self) {
        (self.pdf, self.pdf_events, self.beside) = (None, None, None);
        // The next paper opens with even halves.
        self.split_share = 0.5;
    }

    /// The other half shows the same file as the half worked in: 分屏 自由 on one file, two viewers of it.
    pub fn beside_itself(&self, cx: &App) -> bool {
        let (Some(pdf), Some(beside)) = (&self.pdf, &self.beside) else { return false };
        pdf.read(cx).resource_id() == beside.view.read(cx).resource_id()
    }

    /// The paper of a viewer that is not the panel's: one kept beside, or read in a window of its own.
    pub fn apart(&self, view: &Entity<PdfView>) -> Option<String> {
        self.pins.iter().find(|pin| &pin.view == view).map(|pin| pin.paper.clone()).or_else(|| self.windows.iter().find(|w| &w.view == view).map(|w| w.paper.clone()))
    }

    /// Note where the files on show are read at, here and in the library's database, before they are put
    /// away: another paper opened, the library closed or the app quit (#194).
    pub fn keep_places(&mut self, cx: &App) {
        // The same file in both halves (分屏 自由) is read at where the half worked in is.
        let beside = self.beside.as_ref().filter(|_| !self.beside_itself(cx)).map(|beside| &beside.view);
        let views = self.pdf.iter().chain(beside).chain(self.pins.iter().map(|pin| &pin.view)).chain(self.windows.iter().map(|w| &w.view));
        let mut places: Vec<(String, (usize, f32))> = views.map(|view| { let view = view.read(cx); (view.resource_id().to_string(), view.place()) }).collect();
        // A kept podcast is read at a sample of its sound.
        places.extend(self.podcast_place());
        for (id, place) in &places { self.file_places.insert(id.clone(), *place); }
        let _ = self.store.save_reading_places(&places);
    }

    /// How the papers on show are read now, when it is one of the layouts offered (版面): the tab, the PDF beside the
    /// conversation and 译文 say it.
    pub fn layout_mode(&self) -> Option<wisp_core::panes::Mode> {
        use wisp_core::panes::Mode;
        self.selected.as_ref()?;
        match (self.tab, self.split, self.facing) {
            (Tab::Pdf, _, false) => Some(Mode::Read),
            (Tab::Pdf, _, true) => Some(Mode::Translate),
            (Tab::Ask, true, false) => Some(Mode::Ask),
            (Tab::Ask, true, true) => Some(Mode::Study),
            _ => None,
        }
    }

    /// Whether the selected paper's PDF is on show: on its tab, or beside (or under) the paper's conversation.
    pub fn pdf_on_show(&self) -> bool {
        self.tab == Tab::Pdf || (self.tab == Tab::Ask && self.split)
    }

    /// Whether the translation is beside the pages now: the switch is on and the PDF is on show (#193).
    pub fn facing_shown(&self) -> bool {
        self.facing && self.pdf_on_show()
    }

    /// The viewer's size as it was laid out, both halves of it while it shows two files.
    pub fn pdf_area(&self, cx: &App) -> (f32, f32) {
        let Some(pdf) = &self.pdf else { return (0., 0.) };
        let (width, height) = { let view = pdf.read(cx); (view.width(), view.height()) };
        let Some(beside) = &self.beside else { return (width, height) };
        let other = beside.view.read(cx);
        match beside.split {
            Split::Stacked => (width, height + other.height() + crate::pdf::DIVIDER),
            _ => (width + other.width() + crate::pdf::DIVIDER, height),
        }
    }

    /// The paper's PDFs as the viewer offers them: the main text, then each attachment, labelled as the toolbar's title names them.
    pub fn pdf_choices(&self, id: &str) -> Vec<(String, String)> {
        let main = self.main_pdf(id).map(|f| (f.id.clone(), tf!("正文 · {}", "Main text · {}", f.title)));
        let attachments = self.attachments(id).into_iter().enumerate().map(|(i, f)| (f.id.clone(), tf!("附件 S{} · {}", "Attachment S{} · {}", i + 1, f.title)));
        main.into_iter().chain(attachments).collect()
    }

    /// The number an attachment is cited by (`S2` is 2); `None` for the main text.
    pub fn attachment_number(&self, paper: &str, file: &str) -> Option<usize> {
        self.attachments(paper).iter().position(|f| f.id == file).map(|i| i + 1)
    }
}

impl Workspace {
    /// Open the PDF viewer when its tab is showing and the paper has a PDF.
    pub(crate) fn ensure_pdf(&mut self, cx: &mut Context<Self>) {
        self.open_pdf(cx);
        // The translation beside the pages follows the file on show.
        self.load_facing(cx);
    }

    fn open_pdf(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        if !lib.pdf_on_show() || lib.pdf.is_some() { return }
        let Some(file) = lib.selected.as_deref().and_then(|id| lib.pdf_file(id)).cloned() else { return };
        let Some((pdf, events)) = self.pdf_view(&file, cx) else { return };
        let Some(lib) = self.lib.as_mut() else { return };
        (lib.pdf, lib.pdf_events) = (Some(pdf), Some(events));
    }

    /// A viewer of a file of the selected paper, with its own marks, at the place it was left, and what it says heard.
    pub(crate) fn pdf_view(&mut self, file: &PaperFile, cx: &mut Context<Self>) -> Option<(Entity<PdfView>, Subscription)> {
        self.pdf_view_at(file, None, None, cx)
    }

    /// A viewer of a file at a zoom and a place of its own (`None`: the fit papers open at, where the file was left).
    pub(crate) fn pdf_view_at(&mut self, file: &PaperFile, zoom: Option<Zoom>, place: Option<(usize, f32)>, cx: &mut Context<Self>) -> Option<(Entity<PdfView>, Subscription)> {
        let lib = self.lib.as_mut()?;
        let path = lib.store.file_path(file);
        let annotations = match lib.store.pdf_annotations(&file.paper_id, &file.id) {
            Ok(annotations) => annotations,
            Err(error) => { self.fail(format!("{error:#}"), cx); return None }
        };
        let zoom = zoom.unwrap_or(lib.zoom);
        let place = place.or_else(|| lib.file_places.get(&file.id).copied());
        let pdf = cx.new(|cx| {
            let mut pdf = PdfView::new(&path, zoom, file, annotations, cx);
            // A file read before, of this paper, opens where it was left.
            if let Some(place) = place { pdf.resume(place, cx) }
            pdf
        });
        let events = cx.subscribe(&pdf, |this, view, event: &PdfEvent, cx| this.pdf_event(view, event, cx));
        let (figures, collecting) = self.figure_frames(&file.id);
        pdf.update(cx, |pdf, cx| pdf.set_figures(figures, collecting, cx));
        // The paper's other PDFs: a figure's number in this file may name a figure in one of them (#229).
        if let Some(lib) = self.lib.as_ref() {
            let main = lib.main_pdf(&file.paper_id).map(|f| f.id.clone());
            let named = |f: &PaperFile, n: usize| if main.as_deref() == Some(f.id.as_str()) { t("正文").to_string() } else { tf!("附件 S{}", "Attachment S{}", n) };
            let all: Vec<(&PaperFile, String)> = lib.main_pdf(&file.paper_id).into_iter().map(|f| (f, named(f, 0))).chain(lib.attachments(&file.paper_id).into_iter().enumerate().map(|(n, f)| (f, named(f, n + 1)))).collect();
            let own = all.iter().find(|(f, _)| f.id == file.id).map(|(_, label)| label.clone());
            let siblings: Vec<crate::pdf::Sibling> = all.iter().filter(|(f, _)| f.id != file.id).map(|(f, label)| crate::pdf::Sibling { resource: f.id.clone(), label: label.clone(), path: lib.store.file_path(f), main: main.as_deref() == Some(f.id.as_str()) }).collect();
            if let (Some(label), false) = (own, siblings.is_empty()) {
                let is_main = main.as_deref() == Some(file.id.as_str());
                pdf.update(cx, |pdf, cx| pdf.set_siblings(label, is_main, siblings, cx));
            }
        }
        Some((pdf, events))
    }

    /// Show a file of the paper in the other half of the viewer (#185): a supplementary figure kept in view
    /// while the text is read. Each half is a viewer of its own file, with its own place and marks. The file
    /// worked in asked for is the viewer split on itself again, as 左右分屏 does.
    pub(crate) fn show_beside(&mut self, file: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(pdf) = lib.pdf.clone() else { return };
        let Some(paper) = lib.selected.clone() else { return };
        // The translation keeps the other half while it is on.
        if lib.facing_shown() { return }
        let (shown, inner, width) = { let view = pdf.read(cx); (view.resource_id().to_string(), view.split(), view.width()) };
        // As the halves stand now; split anew, side by side where there is room for two pages.
        let split = crate::pdf::beside_split(lib.beside.as_ref().map(|beside| beside.split), inner, width);
        if file == shown {
            // 自由: the file again in a viewer of its own beside it.
            if self.settings.pdf_split_free {
                self.split_itself(split, None, false, cx);
                return;
            }
            self.close_beside(cx);
            return pdf.update(cx, |pdf, cx| pdf.set_split(split, cx));
        }
        if lib.beside.as_ref().is_some_and(|beside| beside.view.read(cx).resource_id() == file) { return }
        let Some(paper_file) = lib.paper_files(&paper).into_iter().find(|f| f.id == file && f.format == "pdf").cloned() else { return };
        self.close_beside(cx);
        // Each half is one file now: the viewer worked in shows its pages once.
        pdf.update(cx, |pdf, cx| pdf.set_split(Split::Off, cx));
        let Some((view, events)) = self.pdf_view(&paper_file, cx) else { return };
        let Some(lib) = self.lib.as_mut() else { return };
        lib.beside = Some(Beside { view, _events: events, split, first: false });
        cx.notify();
    }

    /// The other half's file put away; it keeps its place for when it is shown again.
    /// Read the papers on show in a layout of 版面 (#194): the tab, the PDF beside the conversation and 译文 as the
    /// mode says, and the widths of the list and the conversation from its units in this window (they are dragged
    /// as before afterwards). 提问 has the list at the left and the conversation before the papers; 精读 has the
    /// window, the papers with their translations, and the conversation at the right.
    pub(crate) fn set_layout(&mut self, mode: wisp_core::panes::Mode, window: &mut Window, cx: &mut Context<Self>) {
        use wisp_core::panes::{self, Mode, Pane, Placement};
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.selected.is_none() { return }
        let mut columns = panes::columns(mode, 1 + lib.pins.len());
        // A translation that stands on the pages themselves takes no column.
        if self.settings.facing_in_place { columns.retain(|c| c.pane != Pane::Translation) }
        let placed = panes::arrange(&columns, f32::from(window.viewport_size().width), 1.);
        let width_of = |pane: Pane| columns.iter().zip(&placed).find(|(c, _)| c.pane == pane).and_then(|(_, p)| if let Placement::Shown { width, .. } = p { Some(*width) } else { None });
        let (chat, list) = (width_of(Pane::Chat), width_of(Pane::List));
        let asking = matches!(mode, Mode::Ask | Mode::Study);
        if asking {
            lib.split = true;
            lib.reading = mode == Mode::Study;
            self.settings.pdf_first = mode == Mode::Study;
            // With papers kept beside, the widths follow from the papers each time the window is drawn (`pane_frame`),
            // anew from here on; the reader's widths beside one paper stay what they are.
            if lib.pins.is_empty() {
                if let Some(chat) = chat { self.settings.chat_width = chat }
                if let Some(list) = list { self.settings.ask_list_width = list }
            } else {
                lib.pane_chat = 0.;
            }
            self.save_settings(cx);
        }
        self.run(Cmd::SetTab(if asking { Tab::Ask } else { Tab::Pdf }), window, cx);
        if self.lib.as_ref().is_some_and(|lib| lib.facing != mode.translates()) { self.toggle_facing(cx) }
        cx.notify();
    }

    /// 留在旁边 on the PDF's bar: the paper on show is marked to stay (`Library::keep`), and the next paper chosen
    /// opens beside it (`select`). Nothing is shown twice meanwhile. In reading mode there is no list to choose
    /// from: the library comes back around the paper, and the two have the window again once the other is chosen.
    pub(crate) fn keep_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(paper) = lib.selected.clone() else { return };
        if lib.keep {
            lib.keep = false;
            return cx.notify();
        }
        if lib.pins.len() >= MAX_PINS { return self.fail(tf!("最多在旁边留 {MAX_PINS} 篇。先关掉一栏。", "At most {MAX_PINS} papers can stay beside. Close a pane first."), cx) }
        if lib.pdf_file(&paper).is_none() { return self.fail(t("这篇文献还没有 PDF，没有可以放在旁边的页面。"), cx) }
        lib.keep = true;
        if lib.reading && lib.page == Page::Papers { lib.reading = false }
        self.ensure_pdf(cx);
        let listed = self.detail_width(window) < f32::from(window.viewport_size().width);
        self.say(if listed { t("已留住这篇。在列表里选另一篇，两篇并排显示。") } else { t("已留住这篇。再打开另一篇，两篇并排显示。") }, cx);
        cx.notify();
    }

    /// The papers on show are about to change (one kept beside, or let go): the conversation about them ends
    /// here, kept in the history under what it was about, and the next question starts one about the papers then on show.
    fn leave_paper_talk(&mut self, cx: &mut Context<Self>) {
        self.stop_chat(Talk::Paper, cx);
        if let Some(lib) = self.lib.as_mut() { lib.paper_chat.reset(cx) }
    }

    /// The viewer of the paper on show taken out as a pane of its own, with its translation's paragraphs: the paper
    /// goes on being read in it as it is (its place, its zoom, what is selected and searched for). None while no
    /// viewer of it is on show.
    pub(crate) fn main_as_pin(&mut self, cx: &mut Context<Self>) -> Option<Pinned> {
        self.close_beside(cx);
        let lib = self.lib.as_mut()?;
        let paper = lib.selected.clone()?;
        if lib.pdf.is_none() || lib.pdf_events.is_none() { return None }
        Some(Pinned { paper, view: lib.pdf.take()?, _events: lib.pdf_events.take()?, facing: lib.facing_text.take() })
    }

    /// Work in the pane of a paper kept beside (#194): it becomes the paper on show (the title over the panes,
    /// the tools on its bar, the keys, 信息 and the list's selection are its), and the paper that was takes its
    /// place among those kept beside. No pane moves and no viewer is made anew: the two change roles, as the two
    /// halves of a viewer do (`work_in`). One behind `+N` comes into view with it: the panes drawn are those
    /// around the paper on show. It is no step of 后退 / 前进: both papers were on show.
    pub(crate) fn work_in_pane(&mut self, paper: &str, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(at) = lib.pins.iter().position(|pin| pin.paper == paper) else { return };
        lib.keep = false;
        let order = lib.pane_order();
        let pin = lib.pins.remove(at);
        if let Some(left) = self.main_as_pin(cx) && let Some(lib) = self.lib.as_mut() { lib.pins.push(left) }
        self.picked = None;
        let Some(lib) = self.lib.as_mut() else { return };
        lib.marked.clear();
        lib.keep_places(cx);
        lib.selected = Some(pin.paper.clone());
        lib.close_pdf();
        // The file its pane shows is the one the paper is read in.
        let file = pin.view.read(cx).resource_id().to_string();
        if lib.main_pdf(&pin.paper).is_some_and(|main| main.id == file) { lib.read_file.remove(&pin.paper); } else { lib.read_file.insert(pin.paper.clone(), file); }
        (lib.pdf, lib.pdf_events, lib.facing_text) = (Some(pin.view), Some(pin._events), pin.facing);
        lib.order_panes(&order);
        lib.detail_scroll = ScrollHandle::new();
        if let Some(notes) = lib.selected_paper().map(|paper| paper.notes.clone()) { lib.notes.update(cx, |input, cx| input.set_text(&notes, cx)) }
        self.ensure_pdf(cx);
        self.translate_selected(false, cx);
        cx.notify();
    }

    /// Papers kept beside need the window: beside the table a panel has room for one. On the PDF tab, outside
    /// the layout for asking, the papers get the window as in reading mode (Esc brings the library back).
    pub(crate) fn room_for_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asking = self.asking(window);
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.pins.is_empty() || lib.selected.is_none() || lib.reading || asking || lib.tab != Tab::Pdf { return }
        lib.reading = true;
        self.ensure_pdf(cx);
        cx.notify();
    }

    /// Keep a paper in a pane of its own beside the one on show (#194). Of the paper on show, the file being read
    /// at the place it is read at; of another, the file it opens with, where it was left.
    pub(crate) fn pin(&mut self, paper: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.pins.iter().any(|pin| pin.paper == paper) { return self.say(t("这篇已经在旁边的一栏里了。"), cx) }
        if lib.pins.len() >= MAX_PINS { return self.fail(tf!("最多在旁边留 {MAX_PINS} 篇。先关掉一栏。", "At most {MAX_PINS} papers can stay beside. Close a pane first."), cx) }
        let shown = lib.pdf.as_ref().filter(|_| lib.selected.as_deref() == Some(paper.as_str())).map(|view| { let view = view.read(cx); (view.resource_id().to_string(), view.place()) });
        if let Some((file, place)) = &shown { lib.file_places.insert(file.clone(), *place); }
        let file = match &shown {
            Some((id, _)) => lib.files.iter().find(|f| &f.id == id).cloned(),
            None => lib.pdf_file(&paper).cloned(),
        };
        let Some(file) = file else { return self.fail(t("这篇文献还没有 PDF，没有可以放在旁边的页面。"), cx) };
        let Some((view, events)) = self.pdf_view(&file, cx) else { return };
        // One more paper on show: the conversation about them is another one.
        self.leave_paper_talk(cx);
        let Some(lib) = self.lib.as_mut() else { return };
        lib.pins.push(Pinned { paper, view, _events: events, facing: None });
        // Pinned from the record of the paper on show: its pane is beside the PDF.
        if lib.tab == Tab::Info && lib.selected.is_some() { lib.tab = Tab::Pdf }
        // With 译文 on, its translation is read as well.
        self.ensure_pdf(cx);
        cx.notify();
    }

    /// Let a pinned paper go: its pane closes, and its file keeps its place for when it is read again.
    pub(crate) fn unpin(&mut self, paper: &str, cx: &mut Context<Self>) {
        if self.lib.as_ref().is_none_or(|lib| !lib.pins.iter().any(|pin| pin.paper == paper)) { return }
        // One paper fewer on show: the conversation about them is another one.
        self.leave_paper_talk(cx);
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(at) = lib.pins.iter().position(|pin| pin.paper == paper) else { return };
        let order = lib.pane_order();
        let pin = lib.pins.remove(at);
        lib.order_panes(&order);
        // The width the conversation was dragged to went with the papers beside.
        if lib.pins.is_empty() { lib.pane_chat = 0. }
        let view = pin.view.read(cx);
        let place = (view.resource_id().to_string(), view.place());
        lib.file_places.insert(place.0.clone(), place.1);
        let _ = lib.store.save_reading_places(&[place]);
        cx.notify();
    }

    pub(crate) fn close_beside(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        // The same file in the other half: its place is the half worked in's, which stays.
        let itself = lib.beside_itself(cx);
        let Some(beside) = lib.beside.take() else { return };
        let view = beside.view.read(cx);
        if !itself { lib.file_places.insert(view.resource_id().to_string(), view.place()); }
        cx.notify();
    }

    /// 分屏 自由 on one file (#245): the file worked in is shown again in the other half, a viewer of its own at
    /// the same zoom, so each half is zoomed on its own and the band between them is dragged. The other half opens
    /// at `place` (where the half worked in is, without one) and is the first, at the left or the top, when `first`.
    /// Its viewer, or `None` when there is no file to show twice.
    pub(crate) fn split_itself(&mut self, split: Split, place: Option<(usize, f32)>, first: bool, cx: &mut Context<Self>) -> Option<Entity<PdfView>> {
        let lib = self.lib.as_ref()?;
        let pdf = lib.pdf.clone()?;
        let paper = lib.selected.clone()?;
        if split == Split::Off || lib.facing_shown() { return None }
        if lib.beside_itself(cx) { return lib.beside.as_ref().map(|beside| beside.view.clone()) }
        let (file, zoom, here) = { let view = pdf.read(cx); (view.resource_id().to_string(), view.zoom(), view.place()) };
        let paper_file = lib.paper_files(&paper).into_iter().find(|f| f.id == file && f.format == "pdf").cloned()?;
        self.close_beside(cx);
        pdf.update(cx, |pdf, cx| pdf.set_split(Split::Off, cx));
        let (view, events) = self.pdf_view_at(&paper_file, Some(zoom), Some(place.unwrap_or(here)), cx)?;
        let lib = self.lib.as_mut()?;
        lib.beside = Some(Beside { view: view.clone(), _events: events, split, first });
        cx.notify();
        Some(view)
    }

    /// 分屏 自由 or 固定 (#245). A file split on itself changes how it is drawn and stays where it is: 自由 is two
    /// viewers of it, each with its own zoom; 固定 is one viewer showing it twice, its halves even and zoomed together.
    pub(crate) fn set_split_free(&mut self, free: bool, cx: &mut Context<Self>) {
        self.settings.pdf_split_free = free;
        self.save_settings(cx);
        if free { self.part_itself(cx) } else { self.join_itself(cx) }
        cx.notify();
    }

    /// The map is to come as a column of the viewer worked in: under 分屏 自由 a file split on itself is two viewers,
    /// which have no third column, so they become one viewer showing the file twice first, as for 译文 (`toggle_facing`),
    /// and the map stands to the right of both halves. `PdfEvent::MapGone` parts them again.
    pub(crate) fn join_for_map(&mut self, view: &Entity<PdfView>, cx: &mut Context<Self>) {
        if view.read(cx).map_shown() || self.lib.as_ref().is_none_or(|lib| lib.pdf.as_ref() != Some(view) || !lib.beside_itself(cx)) { return }
        self.join_itself(cx);
    }

    /// One viewer showing its file twice, each half scrolled on its own, becomes two viewers of the file (分屏 自由):
    /// not while another viewer stands beside it, nor while 译文 is on show.
    pub(crate) fn part_itself(&mut self, cx: &mut Context<Self>) {
        let Some(pdf) = self.lib.as_ref().filter(|lib| lib.beside.is_none() && !lib.facing_shown()).and_then(|lib| lib.pdf.clone()) else { return };
        if let Some((split, place, first)) = pdf.update(cx, |pdf, cx| pdf.part(cx)) { self.split_itself(split, Some(place), first, cx); }
    }

    /// Two viewers of one file become one viewer showing it twice, its halves even: the other half stays where it
    /// is read, and the zoom is the one of the half worked in.
    pub(crate) fn join_itself(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(pdf) = lib.pdf.clone() else { return };
        lib.split_share = 0.5;
        if !lib.beside_itself(cx) { return }
        let Some(beside) = lib.beside.take() else { return };
        let place = beside.view.read(cx).place();
        pdf.update(cx, |pdf, cx| pdf.join(beside.split, place, beside.first, cx));
    }

    /// The pointer went to work in a viewer. Beside another file it becomes the one worked in (`Library::pdf`):
    /// the toolbar, the keys and a citation speak of it, and the paper is read in its file (`read_file`). The two
    /// change roles, not places.
    fn work_in(&mut self, view: &Entity<PdfView>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(beside) = lib.beside.as_mut().filter(|beside| &beside.view == view) else { return };
        let (Some(pdf), Some(events)) = (lib.pdf.as_mut(), lib.pdf_events.as_mut()) else { return };
        std::mem::swap(pdf, &mut beside.view);
        std::mem::swap(events, &mut beside._events);
        beside.first = !beside.first;
        let file = view.read(cx).resource_id().to_string();
        if let Some(paper) = lib.selected.clone() {
            if lib.main_pdf(&paper).is_some_and(|main| main.id == file) { lib.read_file.remove(&paper); } else { lib.read_file.insert(paper, file); }
        }
        cx.notify();
    }

    /// Show a file of a paper in the viewer: an attachment by its id, or (`None`) the main text. The file on
    /// show keeps its place for when it is come back to, and each file has its own marks.
    pub(crate) fn show_file(&mut self, paper: &str, file: Option<String>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        match file {
            Some(file) => lib.read_file.insert(paper.to_string(), file),
            None => lib.read_file.remove(paper),
        };
        let wanted = lib.pdf_file(paper).map(|f| f.id.clone());
        // A file in the other half is gone to there; the half worked in keeps its own.
        if lib.selected.as_deref() == Some(paper) && let Some(other) = lib.beside.as_ref().map(|beside| beside.view.clone()) && wanted.as_deref() == Some(other.read(cx).resource_id()) && !lib.beside_itself(cx) {
            self.work_in(&other, cx);
            return cx.notify();
        }
        if let Some(pdf) = lib.pdf.clone() {
            let (shown, place) = { let view = pdf.read(cx); (view.resource_id().to_string(), view.place()) };
            if Some(&shown) != wanted.as_ref() {
                lib.file_places.insert(shown, place);
                (lib.pdf, lib.pdf_events) = (None, None);
            }
        }
        self.ensure_pdf(cx);
        cx.notify();
    }

    /// Whether a viewer is the panel's, which can show another file of its paper; one in a window of its own
    /// or kept beside cannot, and says so.
    fn panel_viewer(&mut self, view: &Entity<PdfView>, cx: &mut Context<Self>) -> bool {
        let panel = self.lib.as_ref().is_some_and(|lib| lib.pdf.as_ref() == Some(view));
        if !panel { self.say(t("这张图在这篇文献的另一个文件里：在文献库窗口里读这篇，才能固定对照或前往。"), cx) }
        panel
    }

    /// Read another PDF of the selected paper in the half worked in (`show_file`), and hand back its viewer
    /// when it is the one on show now.
    fn read_in_panel(&mut self, file: &str, cx: &mut Context<Self>) -> Option<Entity<PdfView>> {
        let lib = self.lib.as_ref()?;
        let paper = lib.selected.clone()?;
        let main = lib.main_pdf(&paper).map(|f| f.id.clone());
        self.show_file(&paper, (main.as_deref() != Some(file)).then(|| file.to_string()), cx);
        self.lib.as_ref()?.pdf.clone().filter(|pdf| pdf.read(cx).resource_id() == file)
    }

    /// What the PDF viewer reports: its selection copied, or handed to a conversation.
    /// The mind map of a viewer's paper, made for the viewer the first time it is asked for: the one kept in
    /// the library, or a new one with the paper's title for its root (kept once something is made of it).
    pub(crate) fn ensure_map(&mut self, viewer: &Entity<PdfView>, cx: &mut Context<Self>) -> Option<Entity<MindMapView>> {
        if let Some(map) = viewer.read(cx).map() { return Some(map.clone()) }
        let paper = viewer.read(cx).paper_id().to_string();
        let lib = self.lib.as_ref()?;
        let title = lib.papers.iter().find(|p| p.id == paper).map(|p| p.title.clone()).unwrap_or_default();
        let kept = match lib.store.mind_map(&paper) {
            Ok(kept) => kept,
            Err(error) => {
                self.fail(tf!("无法读取思维导图：{error:#}", "Cannot read the mind map: {error:#}"), cx);
                return None;
            }
        };
        let map = cx.new(|cx| MindMapView::new(kept.unwrap_or_else(|| wisp_core::mindmap::MindMap::new(&title)), cx));
        // The viewer holds the map and this hearing of it: a strong hold on the viewer here would keep both for ever.
        let weak = viewer.downgrade();
        let heard = cx.subscribe(&map, move |this, map, event: &MapEvent, cx| if let Some(viewer) = weak.upgrade() { this.map_event(&viewer, &map, event, cx) });
        viewer.update(cx, |pdf, _| pdf.set_map(map.clone(), heard));
        Some(map)
    }

    /// What was done in a viewer's mind map: a change is kept and shown by every other viewer of the paper, a
    /// node's page is gone to in the viewer the map stands in, and a draft is asked of the model.
    fn map_event(&mut self, viewer: &Entity<PdfView>, map: &Entity<MindMapView>, event: &MapEvent, cx: &mut Context<Self>) {
        let (paper, shown) = { let pdf = viewer.read(cx); (pdf.paper_id().to_string(), pdf.resource_id().to_string()) };
        match event {
            MapEvent::Changed => {
                let kept = map.read(cx).map().clone();
                let Some(lib) = self.lib.as_mut() else { return };
                if let Err(error) = lib.store.save_mind_map(&paper, &kept) { return self.fail(tf!("无法保存思维导图：{error:#}", "Cannot save the mind map: {error:#}"), cx) }
                let viewers = lib.pdf.iter().chain(lib.beside.iter().map(|beside| &beside.view)).chain(lib.pins.iter().map(|pin| &pin.view)).chain(lib.windows.iter().map(|window| &window.view));
                let others: Vec<Entity<MindMapView>> = viewers.filter(|other| *other != viewer && other.read(cx).paper_id() == paper).filter_map(|other| other.read(cx).map().cloned()).collect();
                for other in others { other.update(cx, |other, cx| other.set_map(kept.clone(), cx)) }
            }
            MapEvent::Goto(anchor) => {
                if anchor.file.is_empty() || anchor.file == shown { return viewer.update(cx, |pdf, cx| pdf.find(anchor.page, &anchor.quote, cx)) }
                // Another file of the paper: the panel shows it; a pane or a window of its own reads its one file.
                if self.lib.as_ref().is_none_or(|lib| lib.pdf.as_ref() != Some(viewer)) { return self.say(t("这段出处在这篇文献的另一个文件里：在文献库窗口里读这篇，才能前往。"), cx) }
                self.show_file(&paper, Some(anchor.file.clone()), cx);
                if let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.find(anchor.page, &anchor.quote, cx)) }
            }
            MapEvent::Draft => self.draft_map(viewer, map, cx),
        }
    }

    /// A draft of a paper's mind map from the model (the assistant's): written from the text of the file the
    /// viewer shows, its passages looked up on the pages, and then the reader's to change. It takes the place
    /// of the map there, which is a step of Undo away.
    pub(crate) fn draft_map(&mut self, viewer: &Entity<PdfView>, map: &Entity<MindMapView>, cx: &mut Context<Self>) {
        if map.read(cx).drafting() { return }
        let profile = self.settings.profile_for_or_default(wisp_core::settings::Task::Chat);
        let language = match self.settings.language { wisp_core::settings::Language::Zh => "中文", wisp_core::settings::Language::En => "英文" };
        let Some(lib) = self.lib.as_ref() else { return };
        let (paper, file) = { let pdf = viewer.read(cx); (pdf.paper_id().to_string(), pdf.resource_id().to_string()) };
        let Some(paper) = lib.papers.iter().find(|p| p.id == paper).cloned() else { return };
        let kept = match lib.store.fulltext_pages(&file) { Ok(Some(Ok(pages))) => Some(pages), _ => None };
        let path = lib.files.iter().find(|f| f.id == file).map(|f| lib.store.file_path(f));
        let library = lib.info.id.clone();
        map.update(cx, |map, cx| map.set_drafting(true, cx));
        let protocol = profile.protocol;
        let work = cx.background_executor().spawn(async move {
            // A file that gives no text still has its abstract to draw from.
            let pages = pages_of(kept, path).unwrap_or_default();
            let request = wisp_core::mindmap::request(&profile, language, &paper, &pages)?;
            let reply = wisp_net::complete(request, protocol)?;
            wisp_core::mindmap::read_reply(&reply, &paper, &file, &pages)
        });
        let map = map.downgrade();
        cx.spawn(async move |this, cx| {
            let drafted: Result<wisp_core::mindmap::MindMap, String> = work.await;
            this.update(cx, |this, cx| {
                // The library was closed since, or the viewer with its map.
                if this.lib.as_ref().is_none_or(|lib| lib.info.id != library) { return }
                let Some(map) = map.upgrade() else { return };
                map.update(cx, |map, cx| match drafted {
                    Ok(drafted) => map.take_draft(drafted, cx),
                    Err(error) => map.draft_failed(error, cx),
                });
            }).ok();
        }).detach();
    }

    fn pdf_event(&mut self, view: Entity<PdfView>, event: &PdfEvent, cx: &mut Context<Self>) {
        match event {
            PdfEvent::Worked => self.work_in(&view, cx),
            // A view's label asked for a translation, or gave up the last one: what the 译文 switch does.
            PdfEvent::Facing(on) => if self.lib.as_ref().is_some_and(|lib| lib.facing != *on) { self.toggle_facing(cx) },
            // A press in the pane of a paper kept beside: that paper is the one worked in from here on.
            // The paper's mind map is asked for: by a view's label, or by 导图 for a viewer that had none yet.
            PdfEvent::Map(at) => {
                if self.ensure_map(&view, cx).is_none() { return }
                let shown = view.update(cx, |pdf, cx| match at {
                    Some(at) => { pdf.show_in_view(*at, Shows::Map, cx); true }
                    None => pdf.toggle_map(cx),
                });
                if !shown { self.say(t("已经有三栏了：先关掉一栏，或在某一栏的标签里选「显示导图」。"), cx) }
            }
            // Under 分屏 自由 the file shown twice is two viewers again once the map is gone (`part_itself` checks the rest).
            PdfEvent::MapGone => if self.settings.pdf_split_free && self.lib.as_ref().is_some_and(|lib| lib.pdf.as_ref() == Some(&view)) { self.part_itself(cx) },
            // 加到导图: the passage is a node under the one chosen, pointing back at its page; the map comes into view.
            PdfEvent::MapPassage(page, text) => {
                let Some(map) = self.ensure_map(&view, cx) else { return };
                let file = view.read(cx).resource_id().to_string();
                let quote = text.split_whitespace().collect::<Vec<_>>().join(" ");
                // A node is a few words: a long passage is cut for it, and kept whole as what it points at.
                let words = if quote.chars().count() > 60 { quote.chars().take(60).collect::<String>() + "…" } else { quote.clone() };
                map.update(cx, |map, cx| map.add_passage(&words, wisp_core::mindmap::Anchor { file, page: *page, quote }, cx));
                self.join_for_map(&view, cx);
                if !view.update(cx, |pdf, cx| pdf.map_shown() || pdf.toggle_map(cx)) { self.say(t("已加到导图。现在有三栏，关掉一栏就能看到它。"), cx) }
            }
            PdfEvent::Pressed => {
                if let Some(paper) = self.lib.as_ref().and_then(|lib| lib.pins.iter().find(|pin| pin.view == view).map(|pin| pin.paper.clone())) { self.work_in_pane(&paper, cx) }
            }
            PdfEvent::Copied => self.say(t("已复制"), cx),
            PdfEvent::StepMarks(back) => self.step_marks(&view, *back, cx),
            PdfEvent::Translate(text, profile) => self.translate_passage(text.clone(), profile.clone(), cx),
            PdfEvent::ReadAloud(..) | PdfEvent::Figure(_) if self.lib.as_ref().is_some_and(|lib| lib.apart(&view).is_some()) => self.say(t("朗读和收进图片库只对正在读的那篇：先在列表里选中它。"), cx),
            PdfEvent::ReadAloud(page, text) => self.read_aloud(Some((*page, text.clone())), cx),
            PdfEvent::Figure(piece) => self.collect_piece(piece, cx),
            PdfEvent::Annotate { paper, resource, page, kind, quote, boxes } => self.change_pdf_annotation(resource, |store| store.add_pdf_annotation(paper, resource, *page, *kind, quote, boxes).map(|_| ()), cx),
            PdfEvent::AnnotationNote { resource, id, note } => self.change_pdf_annotation(resource, |store| store.update_pdf_annotation_note(resource, id, note), cx),
            PdfEvent::RecolorAnnotation { resource, id, color } => self.change_pdf_annotation(resource, |store| store.recolor_pdf_annotation(resource, id, color), cx),
            PdfEvent::RemoveAnnotation { resource, id } => self.change_pdf_annotation(resource, |store| store.remove_pdf_annotation(resource, id), cx),
            PdfEvent::ExportMarked => { let id = view.read(cx).resource_id().to_string(); self.export_marked_pdf(id, None, cx) }
            PdfEvent::AddShape { paper, resource, page, kind, rect, note } => self.change_pdf_annotation(resource, |store| store.add_pdf_shape(paper, resource, *page, *kind, *rect, note).map(|_| ()), cx),
            PdfEvent::MoveShape { resource, id, rect } => self.change_pdf_annotation(resource, |store| store.move_pdf_shape(resource, id, *rect), cx),
            PdfEvent::AddText { paper, resource, page, rect, text, style } => self.change_pdf_annotation(resource, |store| store.add_pdf_text(paper, resource, *page, *rect, text, style).map(|_| ()), cx),
            // The words first: written empty, the box goes and there is nothing to size.
            PdfEvent::WriteText { resource, id, text, rect } => self.change_pdf_annotation(resource, |store| {
                store.update_pdf_annotation_note(resource, id, text)?;
                match rect { Some(rect) if !text.trim().is_empty() => store.move_pdf_shape(resource, id, *rect), _ => Ok(()) }
            }, cx),
            PdfEvent::RestyleText { resource, id, style } => self.change_pdf_annotation(resource, |store| store.restyle_pdf_text(resource, id, style), cx),
            PdfEvent::OpenFigure(id) => self.open_figure_on_page(id, cx),
            // 固定对照: the figure stays in the other half of this viewer. A file of the paper shown there gives it up.
            PdfEvent::FigureBeside { file: None, page, top, mark } => {
                let Some(lib) = self.lib.as_ref() else { return };
                let main = lib.pdf.as_ref() == Some(&view);
                let (inner, width) = { let pdf = view.read(cx); (pdf.split(), pdf.width()) };
                let split = crate::pdf::beside_split(lib.beside.as_ref().filter(|_| main).map(|beside| beside.split), inner, width);
                // 自由: the other half is a viewer of the same file, there at the figure.
                if main && self.settings.pdf_split_free && !lib.facing_shown() {
                    if let Some(other) = self.split_itself(split, None, false, cx) { other.update(cx, |pdf, cx| pdf.arrive(vec![], Some((*page, *top)), mark.clone(), cx)) }
                    return;
                }
                if main { self.close_beside(cx) }
                view.update(cx, |pdf, cx| pdf.keep_beside(split, *page, *top, mark.clone(), cx));
            }
            // A figure in another file of the paper: that file is shown in the other half, at the figure.
            PdfEvent::FigureBeside { file: Some(file), page, top, mark } => {
                if !self.panel_viewer(&view, cx) { return }
                // The translation keeps the other half while it is on, even written on the pages themselves.
                if self.lib.as_ref().is_some_and(|lib| lib.facing_shown()) { return self.say(t("开着「译文」时另一半不能换成别的文件：先关掉译文，或者用「前往原位置」。"), cx) }
                self.show_beside(file.clone(), cx);
                let Some(other) = self.lib.as_ref().and_then(|lib| lib.beside.as_ref()).map(|beside| beside.view.clone()).filter(|other| other.read(cx).resource_id() == file) else { return };
                other.update(cx, |pdf, cx| pdf.arrive(vec![], Some((*page, *top)), mark.clone(), cx));
            }
            // 前往原位置 of a figure in another file: the half worked in shows that file at the figure, and is
            // handed the places to come back to.
            PdfEvent::FigureGo { file, page, top, mark, trail } => {
                if !self.panel_viewer(&view, cx) {
                    // It stays where it is, with the places it had before this one.
                    let kept = trail[..trail.len().saturating_sub(1)].to_vec();
                    return view.update(cx, |pdf, cx| pdf.arrive(kept, None, vec![], cx));
                }
                if let Some(pdf) = self.read_in_panel(file, cx) { pdf.update(cx, |pdf, cx| pdf.arrive(trail.clone(), Some((*page, *top)), mark.clone(), cx)) }
            }
            // 返回刚才位置 to a place in another file: that file again, where it was left and as large.
            PdfEvent::FigureReturn { back, trail } => {
                let Some((file, _)) = back.from.clone() else { return };
                if !self.panel_viewer(&view, cx) { return }
                if let Some(pdf) = self.read_in_panel(&file, cx) { pdf.update(cx, |pdf, cx| pdf.restore(back.clone(), Some(trail.clone()), cx)) }
            }
            PdfEvent::Reframe { page, bbox, png } => {
                let resource = view.read(cx).resource_id().to_string();
                self.reframe_piece(&resource, *page, *bbox, png, cx);
            }
            PdfEvent::Picture(png, to_library) => {
                let talk = if *to_library { Talk::Library } else { Talk::Paper };
                self.attach_images(talk, vec![Picture::Bytes(png.clone())], cx);
                if let Some(lib) = self.lib.as_mut() { lib.focus_box = Some(talk) }
                if *to_library { self.say(t("截图已加到文献库对话的输入框上方。"), cx) } else { self.show_paper_chat(cx) }
            }
            // The conversation beside several papers is about all of them: a passage of any goes to it, under its own paper.
            PdfEvent::Quote(page, text, to_library) => {
                let Some(lib) = self.lib.as_mut() else { return };
                // A paper kept beside is one the conversation is about; one read in a window of its own is not, unless it is on
                // show here as well: what is quoted from it there goes to the library's conversation.
                let pinned = lib.apart(&view);
                let to_library = &(*to_library || pinned.as_ref().is_some_and(|paper| !lib.talk_papers().contains(paper)));
                let Some(paper_id) = pinned.or_else(|| lib.selected.clone()) else { return };
                let file_id = view.read(cx).resource_id().to_string();
                let quote = ChatQuote { paper_id, file_id, page: *page, text: text.clone() };
                let talk = if *to_library { Talk::Library } else { Talk::Paper };
                let chat = lib.chat_mut(talk);
                if chat.quotes.len() >= MAX_QUOTES { return self.fail(tf!("一个问题最多带 {MAX_QUOTES} 段原文。先发出去，或者去掉一段。", "A question can include at most {MAX_QUOTES} passages. Send it first or remove one."), cx) }
                if !chat.quotes.contains(&quote) { chat.quotes.push(quote) }
                chat.history = None;
                // The question is written next: the caret goes to the box, after the draft already there.
                lib.focus_box = Some(talk);
                if *to_library {
                    self.say(t("已加到文献库对话的输入框上方。"), cx);
                } else {
                    self.show_paper_chat(cx);
                }
                cx.notify();
            }
        }
    }

    fn change_pdf_annotation(&mut self, resource: &str, change: impl FnOnce(&mut Store) -> anyhow::Result<()>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        // An event queued by an old viewer cannot edit the newly selected PDF: the marks go to the viewers showing their
        // file, every one of them (a paper pinned beside may be the one on show as well).
        let views: Vec<Entity<PdfView>> = lib.pdf.iter().chain(lib.beside.as_ref().map(|beside| &beside.view)).chain(lib.pins.iter().map(|pin| &pin.view)).chain(lib.windows.iter().map(|w| &w.view)).filter(|view| view.read(cx).resource_id() == resource).cloned().collect();
        if views.is_empty() { return }
        // The file's marks before and after the write are what its undo takes back (`annotation::Change`).
        let before = lib.store.pdf_annotations("", resource).unwrap_or_default();
        match change(&mut lib.store).and_then(|()| lib.store.pdf_annotations("", resource)) {
            Ok(annotations) => {
                lib.mark_history.entry(resource.to_string()).or_default().record(wisp_core::annotation::Change::between(&before, &annotations));
                for pdf in views { pdf.update(cx, |pdf, cx| pdf.annotations_saved(annotations.clone(), cx)) }
                self.say(t("PDF 标注已保存"), cx);
            }
            Err(error) => self.fail(format!("{error:#}"), cx),
        }
    }

    /// ⌘Z (`back`) or ⇧⌘Z in a viewer: the last change of the marks of the file it shows is taken back, or done again.
    /// Each file has its own: the key never changes a file out of sight. Every viewer of the file shows the marks
    /// as they now are, and the one the key was pressed in brings the mark into view.
    fn step_marks(&mut self, view: &Entity<PdfView>, back: bool, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let resource = view.read(cx).resource_id().to_string();
        let mut history = lib.mark_history.remove(&resource).unwrap_or_default();
        let stepped = lib.store.step_pdf_marks(&resource, &mut history, back).and_then(|change| Ok((change, lib.store.pdf_annotations("", &resource)?)));
        lib.mark_history.insert(resource.clone(), history);
        match stepped {
            Ok((None, _)) => self.say(if back { t("没有可以撤销的标注改动") } else { t("没有可以重做的标注改动") }, cx),
            Ok((Some(change), annotations)) => {
                let views: Vec<Entity<PdfView>> = lib.pdf.iter().chain(lib.beside.as_ref().map(|beside| &beside.view)).chain(lib.pins.iter().map(|pin| &pin.view)).chain(lib.windows.iter().map(|w| &w.view)).filter(|other| other.read(cx).resource_id() == resource).cloned().collect();
                for pdf in views { pdf.update(cx, |pdf, cx| pdf.annotations_saved(annotations.clone(), cx)) }
                if let Some((page, rect)) = change.place(back) { view.update(cx, |pdf, cx| pdf.reveal(page, rect, cx)) }
                self.say(if back { t("已撤销标注改动") } else { t("已重做标注改动") }, cx);
            }
            Err(error) => self.fail(format!("{error:#}"), cx),
        }
    }

    /// Something from the PDF went to the paper's own conversation: the
    /// conversation comes up with the PDF to its right, the window laid
    /// out for the two.
    fn show_paper_chat(&mut self, cx: &mut Context<Self>) {
        self.unfold_chat(cx);
        let Some(lib) = self.lib.as_mut() else { return };
        if !(lib.split && lib.tab == Tab::Ask) {
            if lib.tab != Tab::Ask { lib.ask_from = lib.tab }
            (lib.split, lib.tab) = (true, Tab::Ask);
            self.say(t("已加到这篇文献的提问里。"), cx);
        }
        cx.notify();
    }
}
