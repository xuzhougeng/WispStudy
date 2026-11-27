//! The popup menus: what each holds (`Workspace::open_menu`, from a `MenuKind`) and where it stands by what
//! opened it (`menu_place`). A row of a menu is a `Cmd` like any other click; `home.rs` draws the menu.

use crate::app::{Cmd, ExportKind, FilterKey, Talk, Workspace};
use crate::listening::{podcast_files, podcast_list};
use crate::viewers::MAX_PINS;
use crate::i18n::t;
use crate::pdf::{Split, Zoom, ZoomTo};
use gpui::*;
use wisp_core::ai;
use wisp_core::filter::{self, Filters};
use wisp_core::model::*;
use wisp_core::settings::Task;
use wisp_core::{compact, time};

#[derive(Clone, Debug, PartialEq)]
pub enum MenuKind {
    Add,
    Filter(FilterKey),
    Sort,
    /// The columns of the paper table: which are on show (a right click on their names).
    Columns,
    Export,
    Reading(String),
    Screening(String),
    LibraryCard(String),
    /// A conversation of the home page's 最近对话: rename it, make it a library, remove it.
    Draft(String),
    /// The sections of the manuscript an answer can be added to: the text, or `None` for what is selected in the library's conversation.
    InsertInto(Option<String>),
    /// Which model a task uses (设置 › AI).
    TaskModel(Task),
    /// 提取: the model to read what the last run left with.
    ExtractRetry,
    /// The services a provider can be added from (设置 › AI › 模型).
    AddProvider,
    /// How hard the n-th model of the provider on show thinks.
    ModelEffort(usize),
    /// Default thinking effort for chat conversations (设置 › AI › 任务 › 助手).
    DefaultChatEffort,
    /// Per-conversation thinking effort for a chat box.
    ChatEffort(crate::app::Place),
    /// What kind of model the candidate in `new_model` is to be added as (设置 › AI › 模型).
    NewModelKind,
    /// How large the PDF's pages are drawn.
    PdfZoom,
    /// Whether the PDF is shown twice, side by side or one over the other.
    PdfSplit,
    /// The tools the PDF's bar had no room for on its one line (`Workspace::more_items`).
    PdfMore,
    /// The papers kept beside that the window has no room for (`+N` on the last pane's bar): one chosen is shown there.
    Panes,
    /// 版面: how the papers on show are read (#194).
    Layout,
    /// Which of the paper's PDFs is read: its main text or one of its attachments.
    PdfFiles,
    /// 写作: the library's manuscripts, one section's own, export, and the language.
    Manuscripts,
    Section(String),
    ManuscriptExport,
    WriteLanguage,
    /// 译文对照: the language the manuscript is read beside.
    WriteFacingLanguage,
    /// 写作: which papers the candidates come from.
    WriteNarrow,
    /// The menus a right click opens (`src/context.rs`): on a paper of a list, a paper in the trash, a file
    /// of the selected paper, a tag of the sidebar, a past conversation (of the paper or the library), a
    /// text that can be selected (the whole of it), an answer (of which conversation, and its text).
    Paper(String),
    /// The ⋯ of the 信息 tab's row of actions: the paper's filing and removing, as its right-click menu names them.
    PaperMore(String),
    /// Several papers marked in the list.
    Papers(Vec<String>),
    Trashed(String),
    /// Several papers chosen in the trash (`Library::trash_chosen`).
    TrashedMany(Vec<String>),
    File(String),
    Tag(String),
    Conversation(Talk, String),
    Text(String),
    Answer(Talk, String),
    /// A figure of the figure library.
    Figure(String),
    /// A method of 方法库 or an introduction of 导言库.
    Excerpt(String),
    /// The podcast kept for a paper (its id): save its sound or its script elsewhere.
    PodcastFiles(String),
    /// 播客's playlist: every podcast kept in the library, the latest first.
    Podcasts,
    /// The speeds 朗读 and 播客 play at (`playback::SPEEDS`).
    PlaySpeed,
    /// The player's ⋯ for the podcast of a paper (its id): make it anew, save its sound or its script.
    PodcastMore(String),
}

pub struct MenuItem {
    pub label: SharedString,
    pub checked: bool,
    pub note: Option<SharedString>,
    pub cmd: Cmd,
}

pub struct Menu {
    /// Where the pointer was: a right-click's menu opens there.
    pub at: Point<Pixels>,
    /// The button or row that opened it, when one did: the menu stands by it and leaves it in sight (`menu_place`).
    pub from: Option<(Bounds<Pixels>, MenuSide)>,
    pub items: Vec<MenuItem>,
    /// The items a line is drawn above.
    pub breaks: Vec<usize>,
    /// The item the arrow keys are on; Enter takes it.
    pub active: Option<usize>,
    /// The paper the menu is about, marked in its list while the menu is open.
    pub paper: Option<String>,
    /// A filter's choices: the card of all the filters stays open under it (`render_papers`).
    pub filter: bool,
}

/// Where a menu stands by what opened it: under a button, or to the right of a row of a column of rows (the sidebar),
/// where a menu underneath would cover the rows that follow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuSide {
    Below,
    Beside,
}

/// Where a menu of `height` opened from `from` stands in a window of `window` height: the corner of the menu
/// that is fixed, and where. Under its opener, left edges in line; over it when there is no room underneath and
/// there is above; beside a row, tops in line. Never over the opener: a second click on it is how the menu is
/// put away, and it says what the menu belongs to.
pub fn menu_place(from: Bounds<Pixels>, side: MenuSide, height: Pixels, window: Pixels) -> (Corner, Point<Pixels>) {
    let (gap, margin) = (px(4.), px(8.));
    match side {
        MenuSide::Beside => (Corner::TopLeft, point(from.right() + gap, from.top())),
        MenuSide::Below if from.bottom() + gap + height > window - margin && from.top() - gap - height >= margin => (Corner::BottomLeft, point(from.left(), from.top() - gap)),
        MenuSide::Below => (Corner::TopLeft, point(from.left(), from.bottom() + gap)),
    }
}

impl Workspace {
    /// 分屏's choices: the viewer whole or in two halves, and what the other half shows.
    fn split_items(&self, items: &mut Vec<MenuItem>, breaks: &mut Vec<usize>, cx: &App) {
        let Some(lib) = &self.lib else { return };
        let Some(pdf) = &lib.pdf else { return };
        let view = pdf.read(cx);
        let other = lib.beside.as_ref().map(|beside| beside.view.read(cx).resource_id().to_string());
        let current = lib.beside.as_ref().map_or(view.split(), |beside| beside.split);
        // With 译文 on, the choice is where the translation stands: beside the pages, under them, or on them.
        if lib.facing_shown() {
            let in_place = self.settings.facing_in_place;
            for (label, split) in [("译文在右边：左右对照", Split::Side), ("译文在下面：上下对照", Split::Stacked)] {
                // Three columns have no way to stand one over the other: under the pages would take the third column
                // away unasked, so it is not offered until that column is put away.
                if split == Split::Stacked && current == Split::Triple { continue }
                // With the third column the translation is at the right as well: chosen again, it changes nothing.
                let here = current == split || (split == Split::Side && current == Split::Triple);
                items.push(MenuItem { label: t(label).into(), checked: !in_place && here, note: None, cmd: Cmd::PdfSplit(if here && !in_place { current } else { split }) });
            }
            items.push(MenuItem { label: t("译文替换原文：原地替换").into(), checked: in_place, note: None, cmd: Cmd::FacingPlace(true) });
            // A column more, scrolled on its own: a figure or the methods kept in sight beside the pages and their
            // translation. Papers kept beside have the width, each with its translation.
            if lib.pins.is_empty() {
                let third = !in_place && current == Split::Triple;
                breaks.push(items.len());
                items.push(MenuItem { label: t("再开一栏原文：单独滚动").into(), checked: third, note: None, cmd: Cmd::PdfSplit(if third { Split::Side } else { Split::Triple }) });
                // Or the page after the one read, which goes along with it: with the third column taken it is that column's label that says so.
                if !in_place && (view.next_shown() || !view.full()) { items.push(MenuItem { label: t("再开一栏下一页：跟着原文").into(), checked: view.next_shown(), note: None, cmd: Cmd::PdfNext }) }
            }
            return;
        }
        for (label, split) in [("不分屏", Split::Off), ("左右分屏", Split::Side), ("上下分屏", Split::Stacked)] {
            items.push(MenuItem { label: t(label).into(), checked: current == split, note: None, cmd: Cmd::PdfSplit(split) });
        }
        // Two pages across are a column more that shows the next page and goes along with the pages (design rule 35):
        // not a fourth way to split, so it stands apart from them and is ticked beside whichever is in force.
        if other.is_none() {
            breaks.push(items.len());
            items.push(MenuItem { label: t("两页并排：旁边一栏显示下一页").into(), checked: view.next_shown(), note: None, cmd: Cmd::PdfNext });
            breaks.push(items.len());
        }
        // 固定 or 自由: the same as the button on the bar beside 分屏, kept from paper to paper.
        let free = self.settings.pdf_split_free;
        items.push(MenuItem { label: t("自由调整：拖动分隔线，每一半各自缩放").into(), checked: free, note: None, cmd: Cmd::PdfSplitFree(!free) });
        // What the other half shows: the same file, or another of the paper's (a supplementary figure beside the text).
        // The translation has the other half while it is on.
        let choices = lib.selected.as_deref().map(|paper| lib.pdf_choices(paper)).unwrap_or_default();
        if choices.len() > 1 && !lib.facing_shown() {
            breaks.push(items.len());
            let shown = other.unwrap_or_else(|| if current == Split::Off { String::new() } else { view.resource_id().to_string() });
            for (id, label) in choices {
                items.push(MenuItem { label: tf!("另一半：{label}", "Other half: {label}").into(), checked: id == shown, note: None, cmd: Cmd::PdfBeside(id) });
            }
        }
    }

    /// What the PDF's bar had no room for (`Library::bar_level`, a level of `detail::BAR_NO_TOOLS` …), under its ⋯:
    /// the same things by the same names, the ones in use ticked.
    fn more_items(&self, items: &mut Vec<MenuItem>, breaks: &mut Vec<usize>, cx: &App) {
        let Some(lib) = &self.lib else { return };
        let Some(pdf) = &lib.pdf else { return };
        let Some(paper) = lib.selected.clone() else { return };
        let level = lib.bar_level;
        let view = pdf.read(cx);
        let plain = |label: &str, cmd: Cmd| MenuItem { label: label.to_string().into(), checked: false, note: None, cmd };
        let ticked = |label: &str, checked: bool, cmd: Cmd| MenuItem { label: label.to_string().into(), checked, note: None, cmd };
        let part = |items: &Vec<MenuItem>, breaks: &mut Vec<usize>| if !items.is_empty() && breaks.last() != Some(&items.len()) { breaks.push(items.len()) };
        if level >= crate::detail::BAR_NO_KEEP {
            if lib.pins.len() < MAX_PINS && !lib.pins.iter().any(|pin| pin.paper == paper) { items.push(ticked(t("留在旁边"), lib.keep, Cmd::KeepPane)) }
            if let Some((label, cmd)) = self.facing_next() { items.push(plain(&label, cmd)) }
        }
        if level >= crate::detail::BAR_NO_ZOOM {
            part(items, breaks);
            items.push(plain(t("放大"), Cmd::PdfZoom(ZoomTo::In)));
            items.push(plain(t("缩小"), Cmd::PdfZoom(ZoomTo::Out)));
            items.push(ticked(t("适应整页"), view.zoom() == Zoom::FitPage, Cmd::PdfZoom(ZoomTo::Set(Zoom::FitPage))));
        }
        if level >= crate::detail::BAR_NO_TOOLS {
            // Floating, the tools are in their column at the page's edge already.
            if !self.settings.pdf_tools_floating {
                part(items, breaks);
                self.split_items(items, breaks, cx);
                part(items, breaks);
                items.push(ticked(t("截图"), view.cutting(), Cmd::PdfCut));
                items.push(ticked(t("方框"), view.tool() == crate::pdf::Tool::Box, Cmd::PdfTool(crate::pdf::Tool::Box)));
                items.push(ticked(t("便签"), view.tool() == crate::pdf::Tool::Note, Cmd::PdfTool(crate::pdf::Tool::Note)));
                items.push(ticked(t("文本框"), view.tool() == crate::pdf::Tool::Text, Cmd::PdfTool(crate::pdf::Tool::Text)));
                part(items, breaks);
                let playing = lib.playing_here(view.resource_id());
                let paused = lib.aloud_shown.as_ref().is_some_and(|status| status.paused);
                let label = |kind, idle, pause, resume| if playing != Some(kind) { idle } else if paused { resume } else { pause };
                items.push(plain(label(crate::read_aloud::Kind::Aloud, t("朗读"), t("暂停朗读"), t("继续朗读")), Cmd::ReadAloud));
                items.push(plain(label(crate::read_aloud::Kind::Podcast, t("播客"), t("暂停播客"), t("继续播客")), Cmd::Podcast));
                let kept = podcast_files(lib, &paper);
                if kept.0.is_some() || playing == Some(crate::read_aloud::Kind::Podcast) { items.push(plain(t("重新生成播客…"), Cmd::AskNewPodcast)) }
                if kept.0.is_some() { items.push(plain(t("保存播客音频（WAV）…"), Cmd::SavePodcast(paper.clone(), true))) }
                if kept.1.is_some() { items.push(plain(t("保存播客稿子（Markdown）…"), Cmd::SavePodcast(paper.clone(), false))) }
            }
            part(items, breaks);
            items.push(plain(t("在新窗口中阅读"), Cmd::PaperWindow(paper.clone())));
            if !self.settings.pdf_tools_floating { items.push(plain(t("用系统应用打开"), Cmd::OpenFile(view.resource_id().to_string()))) }
            if view.has_exported_marks() { items.push(plain(t("导出带标注的 PDF…"), Cmd::ExportMarkedPdf(view.resource_id().to_string()))) }
            items.push(ticked(t("工具悬浮在页面右侧"), self.settings.pdf_tools_floating, Cmd::FloatPdfTools(!self.settings.pdf_tools_floating)));
        }
    }

    /// A click on a button opens its menu under it, not under the pointer: the button stays in sight.
    pub fn on_menu(&self, kind: MenuKind, cx: &mut Context<Self>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        self.on_menu_at(kind, MenuSide::Below, cx)
    }

    /// The same for a row among rows (the sidebar's): its menu opens to its right, over the page, and the rows under it stay in sight too.
    pub fn on_menu_beside(&self, kind: MenuKind, cx: &mut Context<Self>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        self.on_menu_at(kind, MenuSide::Beside, cx)
    }

    fn on_menu_at(&self, kind: MenuKind, side: MenuSide, cx: &mut Context<Self>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, ev: &ClickEvent, _, cx| {
            this.open_menu(kind.clone(), ev.position(), cx);
            if let Some(menu) = this.menu.as_mut() { menu.from = Some((ev.bounds(), side)) }
        })
    }

    /// The rows of a menu of models: each provider's models that `fits`, in its group; one without a key says so beside it.
    fn model_items(&self, fits: impl Fn(&wisp_core::provider::Model) -> bool, chosen: &str, cmd: impl Fn(String) -> Cmd, items: &mut Vec<MenuItem>, breaks: &mut Vec<usize>) {
        let fits = |m: &wisp_core::provider::Model| !m.model.trim().is_empty() && fits(m);
        let mut providers = self.settings.providers.iter().filter(|p| p.models.iter().any(fits)).peekable();
        while let Some(provider) = providers.next() {
            let note: SharedString = if provider.key().is_empty() { tf!("{} · 缺 API Key", "{} · no API key", provider.title()).into() } else { provider.title().into() };
            for model in provider.models.iter().filter(|m| fits(m)) {
                // A model that judges is no model like the others in the list: it says so.
                let note = if model.judges() { tf!("{note} · 判断模型", "{note} · judging model").into() } else { note.clone() };
                items.push(MenuItem { label: model.model.trim().to_string().into(), checked: chosen == model.id, note: Some(note), cmd: cmd(model.id.clone()) });
            }
            if providers.peek().is_some() { breaks.push(items.len()) }
        }
    }

    pub(crate) fn open_menu(&mut self, kind: MenuKind, at: Point<Pixels>, cx: &mut Context<Self>) {
        let item = |label: &str, cmd: Cmd| MenuItem { label: label.to_string().into(), checked: false, note: None, cmd };
        let mut items = vec![];
        let mut breaks = vec![];
        let paper = match &kind {
            MenuKind::Paper(id) => Some(id.clone()),
            _ => None,
        };
        let filter = matches!(kind, MenuKind::Filter(_));
        match kind {
            MenuKind::Paper(_) | MenuKind::PaperMore(_) | MenuKind::Papers(_) | MenuKind::Trashed(_) | MenuKind::TrashedMany(_) | MenuKind::File(_) | MenuKind::Tag(_) | MenuKind::Conversation(..) | MenuKind::Text(_) | MenuKind::Answer(..) => (items, breaks) = self.context_menu(&kind),
            MenuKind::Figure(id) => (items, breaks) = self.figure_menu(&id),
            MenuKind::Excerpt(id) => (items, breaks) = self.excerpt_menu(&id),
            MenuKind::Add => {
                items.push(item(t("通过 DOI / PMID / 网址添加…"), Cmd::AddById));
                items.push(item(t("导入 PDF 文件…"), Cmd::ImportPdfs));
                items.push(item(t("导入文件夹…"), Cmd::ImportFolder));
                items.push(item(t("导入 BibTeX / RIS…"), Cmd::ImportReferences));
                items.push(item(t("从 Zotero 导入…"), Cmd::ImportZotero));
                items.push(item(t("手动录入…"), Cmd::AddManual));
            }
            MenuKind::Manuscripts | MenuKind::Section(_) | MenuKind::ManuscriptExport | MenuKind::WriteLanguage => (items, breaks) = self.manuscript_menu(&kind, cx),
            MenuKind::WriteFacingLanguage => items = self.facing_language_menu(),
            MenuKind::WriteNarrow => items = self.narrowing_menu(),
            MenuKind::Export => {
                items.push(item("BibTeX（.bib）", Cmd::Export(ExportKind::Bibtex)));
                items.push(item(t("表格（.csv）"), Cmd::Export(ExportKind::Csv)));
                if let Some(id) = self.lib.as_ref().map(|l| l.info.id.clone()) {
                    items.push(item(t("整个文献库，含 PDF（.wispstudy）…"), Cmd::ExportLibrary(id.clone(), true)));
                    items.push(item(t("整个文献库，不含 PDF（.wispstudy）…"), Cmd::ExportLibrary(id, false)));
                }
            }
            MenuKind::Draft(id) => {
                items.push(item(t("重命名…"), Cmd::Draft(crate::draft::Do::Rename(id.clone()))));
                items.push(item(t("建成文献库…"), Cmd::Draft(crate::draft::Do::Promote(id.clone()))));
                if self.libraries.iter().any(|info| !info.draft) { items.push(item(t("绑定到文献库…"), Cmd::Written(crate::written::Do::AskBind(id.clone())))) }
                breaks.push(items.len());
                items.push(item(t("删除…"), Cmd::AskRemoveLibrary(id)));
            }
            MenuKind::LibraryCard(id) => {
                items.push(item(t("打开"), Cmd::OpenLibrary(id.clone())));
                items.push(item(t("编辑名称与主题…"), Cmd::EditLibrary(id.clone())));
                items.push(item(t("导出文献库，含 PDF…"), Cmd::ExportLibrary(id.clone(), true)));
                items.push(item(t("导出文献库，不含 PDF…"), Cmd::ExportLibrary(id.clone(), false)));
                items.push(item(t("移除文献库…"), Cmd::AskRemoveLibrary(id)));
            }
            MenuKind::PdfZoom => {
                let Some(current) = self.lib.as_ref().and_then(|lib| lib.pdf.as_ref()).map(|pdf| pdf.read(cx).zoom()) else { return };
                let fits = [(t("适应宽度"), Zoom::FitWidth), (t("适应整页"), Zoom::FitPage)].map(|(label, zoom)| (label.to_string(), zoom));
                let sizes = [50., 75., 100., 125., 150., 200., 300., 400.].map(|percent| (if percent == 100. { t("100%（实际大小）").to_string() } else { format!("{percent}%") }, Zoom::Percent(percent)));
                let key = crate::shortcuts::key_of("zoom-fit", &self.settings.shortcuts);
                for (label, zoom) in fits.into_iter().chain(sizes) {
                    let note = (zoom == Zoom::FitWidth && !key.is_empty()).then(|| crate::shortcuts::pretty(&key).into());
                    items.push(MenuItem { label: label.into(), checked: current == zoom, note, cmd: Cmd::PdfZoom(ZoomTo::Set(zoom)) });
                }
            }
            MenuKind::Layout => {
                let Some(lib) = &self.lib else { return };
                let (current, papers) = (lib.layout_mode(), 1 + lib.pins.len());
                for mode in wisp_core::panes::Mode::ALL {
                    // Its units as the layout is named, with one paper on show (精读 is 3·3·2); beside others each paper has its own share and the layout no such name.
                    let note = wisp_core::panes::grid(&wisp_core::panes::columns(mode, papers)).map(SharedString::from);
                    items.push(MenuItem { label: t(mode.label()).into(), checked: current == Some(mode), note, cmd: Cmd::Layout(mode) });
                }
            }
            MenuKind::PdfSplit => self.split_items(&mut items, &mut breaks, cx),
            MenuKind::PdfMore => self.more_items(&mut items, &mut breaks, cx),
            MenuKind::Panes => {
                let Some(lib) = &self.lib else { return };
                for paper in lib.panes_behind().into_iter().filter_map(|id| lib.paper(&id)) {
                    let title = if paper.title.trim().is_empty() { t("未命名文献").to_string() } else { crate::ui::one_line(paper.title.trim()) };
                    let title: String = if title.chars().count() > 56 { format!("{}…", title.chars().take(54).collect::<String>().trim_end()) } else { title };
                    items.push(MenuItem { label: title.into(), checked: false, note: None, cmd: Cmd::WorkInPane(paper.id.clone()) });
                }
            }
            MenuKind::PodcastFiles(paper) => {
                let Some(lib) = &self.lib else { return };
                let kept = podcast_files(lib, &paper);
                if kept.0.is_some() { items.push(MenuItem { label: t("保存播客音频（WAV）…").into(), checked: false, note: None, cmd: Cmd::SavePodcast(paper.clone(), true) }) }
                if kept.1.is_some() { items.push(MenuItem { label: t("保存播客稿子（Markdown）…").into(), checked: false, note: None, cmd: Cmd::SavePodcast(paper.clone(), false) }) }
            }
            MenuKind::PlaySpeed => {
                let speed = wisp_core::playback::speed(self.settings.play_speed);
                for (index, each) in wisp_core::playback::SPEEDS.into_iter().enumerate() {
                    items.push(MenuItem { label: wisp_core::playback::speed_label(each).into(), checked: each == speed, note: None, cmd: Cmd::PlaySpeed(index) });
                }
            }
            MenuKind::PodcastMore(paper) => {
                let Some(lib) = &self.lib else { return };
                items.push(item(t("重新生成播客…"), Cmd::AskNewPodcast));
                let kept = podcast_files(lib, &paper);
                if kept.0.is_some() || kept.1.is_some() { breaks.push(items.len()) }
                if kept.0.is_some() { items.push(item(t("保存播客音频（WAV）…"), Cmd::SavePodcast(paper.clone(), true))) }
                if kept.1.is_some() { items.push(item(t("保存播客稿子（Markdown）…"), Cmd::SavePodcast(paper.clone(), false))) }
            }
            MenuKind::Podcasts => {
                let Some(lib) = &self.lib else { return };
                let playing = lib.aloud.as_ref().and_then(|r| r.sound.clone());
                for (paper, sound, _) in podcast_list(lib) {
                    // How long it plays: 16-bit samples at the service's rate, after the WAV header.
                    let seconds = sound.size_bytes.saturating_sub(44) / 2 / wisp_core::speech::SAMPLE_RATE as u64;
                    let title = if paper.title.trim().is_empty() { t("未命名文献").to_string() } else { crate::ui::one_line(paper.title.trim()) };
                    let title: String = if title.chars().count() > 48 { format!("{}…", title.chars().take(46).collect::<String>().trim_end()) } else { title };
                    items.push(MenuItem { label: title.into(), checked: playing.as_deref() == Some(sound.id.as_str()), note: Some(format!("{}:{:02}", seconds / 60, seconds % 60).into()), cmd: Cmd::PlayPodcast(sound.id.clone()) });
                }
            }
            MenuKind::PdfFiles => {
                // The paper's main text and each PDF attachment, the one read ticked.
                let Some(lib) = &self.lib else { return };
                let Some(paper) = lib.selected.clone() else { return };
                let shown = lib.pdf_file(&paper).map(|f| f.id.clone());
                // The file in the other half is said so: chosen, it is the half worked in.
                let other = lib.beside.as_ref().map(|beside| beside.view.read(cx).resource_id().to_string());
                for (id, label) in lib.pdf_choices(&paper) {
                    let note = (other.as_ref() == Some(&id)).then(|| t("另一半").into());
                    items.push(MenuItem { label: label.into(), checked: shown.as_ref() == Some(&id), note, cmd: Cmd::ReadFile(id) });
                }
            }
            MenuKind::Columns => {
                let Some(lib) = &self.lib else { return };
                for (key, name) in crate::library::COLUMNS {
                    items.push(MenuItem { label: t(name).into(), checked: !self.settings.columns_hidden.iter().any(|k| k == key), note: None, cmd: Cmd::ToggleColumn(key.into()) });
                }
                for field in &lib.fields {
                    items.push(MenuItem { label: field.name.clone().into(), checked: field.in_table, note: None, cmd: Cmd::ToggleFieldInTable(field.id.clone()) });
                }
                if !self.settings.column_widths.is_empty() {
                    breaks.push(items.len());
                    items.push(item(t("恢复默认列宽"), Cmd::ResetColumnWidths));
                }
            }
            MenuKind::Sort => {
                let Some(lib) = &self.lib else { return };
                let current = if lib.filters.sort.is_empty() { "newest" } else { lib.filters.sort.as_str() };
                for (value, label) in [("newest", t("最近添加")), ("year-desc", t("发表年份 ↓")), ("year-asc", t("发表年份 ↑")), ("title-asc", t("题目 A→Z")), ("title-desc", t("题目 Z→A")), ("journal-asc", t("期刊 A→Z")), ("authors-asc", t("作者 A→Z"))] {
                    items.push(MenuItem { label: label.into(), checked: current == value, note: None, cmd: Cmd::SetSort(value.into()) });
                }
                let zone = format!("{}-asc", crate::library::zone_sort(&self.settings));
                items.push(MenuItem { label: t("影响因子 ↓").into(), checked: current == "impact-desc", note: None, cmd: Cmd::SetSort("impact-desc".into()) });
                items.push(MenuItem { label: t("分区 1→4").into(), checked: current == zone, note: None, cmd: Cmd::SetSort(zone) });
            }
            MenuKind::Reading(id) => {
                let Some(current) = self.lib.as_ref().and_then(|l| l.paper(&id)).map(|p| p.reading.clone()) else { return };
                for (value, label) in READINGS {
                    items.push(MenuItem { label: t(label).into(), checked: current == value, note: None, cmd: Cmd::SetReading(id.clone(), value.into()) });
                }
            }
            MenuKind::InsertInto(text) => {
                let Some(lib) = self.lib.as_ref() else { return };
                let Some(text) = text.or_else(|| lib.library_chat.picked_text()) else { return };
                let Some(manuscript) = lib.writing.current().or(lib.writing.manuscripts.first()) else { return };
                for section in &manuscript.sections {
                    let length = section.body.chars().count();
                    items.push(MenuItem { label: section.name.clone().into(), checked: false, note: Some(if length == 0 { t("空").into() } else { tf!("{length} 字", "{length} chars").into() }), cmd: Cmd::InsertInto(section.id.clone(), text.clone()) });
                }
            }
            MenuKind::AddProvider => {
                for preset in &wisp_core::provider::PRESETS {
                    let note = self.settings.providers.iter().any(|p| p.preset == preset.key).then(|| SharedString::from(t("已添加")));
                    items.push(MenuItem { label: preset.name.into(), checked: false, note, cmd: Cmd::AddProvider(preset.key.into()) });
                }
                breaks.push(items.len());
                items.push(MenuItem { label: t("其他 OpenAI 兼容服务").into(), checked: false, note: None, cmd: Cmd::AddProvider(String::new()) });
            }
            MenuKind::ModelEffort(index) => {
                let Some(view) = self.settings_view.as_ref() else { return };
                let Some(model) = self.settings.providers.get(view.provider()).and_then(|p| p.models.get(index)) else { return };
                if model.judges() {
                    // A model that judges has nothing to set: it is asked one way and does not think aloud.
                } else if model.draws() {
                    // A model that draws does not think: what is chosen is how it is asked for a picture.
                    let guessed = wisp_core::imagegen::guess(&self.settings.providers[view.provider()].base_url, &model.model);
                    for (key, name) in wisp_core::imagegen::INTERFACES {
                        let note = key.is_empty().then(|| SharedString::from(t(guessed.name())));
                        items.push(MenuItem { label: t(name).into(), checked: model.interface == key, note, cmd: Cmd::SetModelInterface(index, key.to_string()) });
                    }
                } else {
                    for (key, name) in ai::EFFORTS {
                        items.push(MenuItem { label: t(name).into(), checked: model.effort == key, note: None, cmd: Cmd::SetEffort(index, key.to_string()) });
                    }
                    // How much it takes in: a conversation is compacted before it outgrows this.
                    breaks.push(items.len());
                    let guessed = compact::window(&self.settings.providers[view.provider()].profile(&wisp_core::provider::Model { context: 0, ..model.clone() }));
                    items.push(MenuItem { label: t("上下文 · 自动").into(), checked: model.context == 0, note: Some(crate::settings::context_label(guessed).into()), cmd: Cmd::SetModelContext(index, 0) });
                    for tokens in wisp_core::provider::CONTEXTS {
                        items.push(MenuItem { label: tf!("上下文 · {}", "Context · {}", crate::settings::context_label(tokens)).into(), checked: model.context == tokens, note: None, cmd: Cmd::SetModelContext(index, tokens) });
                    }
                }
                if !items.is_empty() { breaks.push(items.len()) }
                // What kind of model it is: the name tells when it is added, and here it can be said otherwise.
                for (kind, name) in [("", t("对话模型")), (wisp_core::imagegen::KIND, t("生图模型")), (wisp_core::judge::KIND, t("判断模型"))] {
                    items.push(MenuItem { label: name.into(), checked: model.kind == kind, note: None, cmd: Cmd::SetModelKind(index, kind.to_string()) });
                }
            }
            MenuKind::DefaultChatEffort => {
                let model_effort = self.settings.profile_for_or_default(Task::Chat).effort;
                for (key, name) in ai::EFFORTS {
                    let note = (key.is_empty() && !model_effort.is_empty()).then(|| tf!("随模型：{}", "Model: {}", t(ai::effort_label(&model_effort))).into());
                    items.push(MenuItem { label: t(name).into(), checked: self.settings.chat_effort == key, note, cmd: Cmd::SetDefaultChatEffort(key.to_string()) });
                }
            }
            MenuKind::ChatEffort(place) => {
                let overridden = self.chat_effort_override(place).is_some();
                let default = self.settings.default_chat_effort();
                let current = self.chat_profile(place).effort;
                for (key, name) in ai::EFFORTS {
                    items.push(MenuItem { label: t(name).into(), checked: current == key, note: (overridden && key == default).then(|| t("默认设置").into()), cmd: Cmd::SetChatEffort(place, key.to_string()) });
                }
            }
            MenuKind::NewModelKind => {
                let Some(view) = self.settings_view.as_ref() else { return };
                let chosen = view.new_model_kind();
                let guessed = crate::settings::model_kind_label(wisp_core::provider::guessed_kind(&view.new_model_text(cx)));
                items.push(MenuItem { label: t("自动").into(), checked: chosen.is_none(), note: Some(guessed.into()), cmd: Cmd::SetNewModelKind(None) });
                breaks.push(items.len());
                for (kind, name) in [("", t("对话模型")), (wisp_core::imagegen::KIND, t("生图模型")), (wisp_core::judge::KIND, t("判断模型"))] {
                    items.push(MenuItem { label: name.into(), checked: chosen == Some(kind), note: None, cmd: Cmd::SetNewModelKind(Some(kind.to_string())) });
                }
            }
            MenuKind::TaskModel(task) => {
                let bound = self.settings.tasks.get(task).to_string();
                // Questions always have a model: the first one until another is chosen.
                let chosen = if task == Task::Chat || task.draws() { self.settings.profile_for(task).map(|p| p.id).unwrap_or_default() } else { bound };
                if task != Task::Chat && !task.draws() {
                    items.push(MenuItem { label: t("与助手相同").into(), checked: chosen.is_empty(), note: None, cmd: Cmd::SetTaskModel(task, String::new()) });
                    breaks.push(items.len());
                }
                self.model_items(|m| task.takes(m), &chosen, |id| Cmd::SetTaskModel(task, id), &mut items, &mut breaks);
            }
            MenuKind::ExtractRetry => {
                let Some(lib) = self.lib.as_ref() else { return };
                // 提取 writes values out: a model that only judges cannot.
                self.model_items(|m| m.speaks(), &self.extract_profile(&lib.extract.with).id, |id| Cmd::Extract(crate::extract::Do::Retry(id)), &mut items, &mut breaks);
            }
            MenuKind::Screening(id) => {
                let Some(current) = self.lib.as_ref().and_then(|l| l.paper(&id)).map(|p| p.screening.clone()) else { return };
                let hints = [("pending", t("还没有判断")), ("included", t("符合主题，重点阅读")), ("excluded", t("可能有用，保留备查")), ("uncertain", t("暂时判断不了")), ("irrelevant", t("与主题无关，默认隐藏"))];
                for (value, label) in SCREENINGS {
                    let hint = hints.iter().find(|(v, _)| *v == value).map(|(_, h)| SharedString::from(*h));
                    items.push(MenuItem { label: t(label).into(), checked: current == value, note: hint, cmd: Cmd::SetScreening(id.clone(), value.into()) });
                }
            }
            MenuKind::Filter(key) => {
                let Some(lib) = &self.lib else { return };
                let f = &lib.filters;
                let fixed = |table: &[(&str, &str)]| table.iter().map(|(v, l)| (v.to_string(), t(l).to_string(), None)).collect::<Vec<_>>();
                let counted = |list: Vec<(String, usize)>| list.into_iter().take(40).map(|(v, n)| (v.clone(), v, Some(n))).collect::<Vec<_>>();
                // Several tags can be chosen and some excluded: each chosen one is ticked, an excluded one says so.
                if key == FilterKey::Tag {
                    items.push(MenuItem { label: t("全部").into(), checked: f.tags.is_empty() && f.not_tags.is_empty(), note: None, cmd: Cmd::SetFilter(key.clone(), String::new()) });
                    for (tag, n) in filter::tag_counts(&lib.papers).into_iter().take(40) {
                        let note = if f.excluded(&tag) { t("已排除").to_string() } else { n.to_string() };
                        items.push(MenuItem { label: tag.clone().into(), checked: f.chosen(&tag), note: Some(note.into()), cmd: Cmd::SetFilter(key.clone(), tag) });
                    }
                } else {
                    // A span of numbers: the usual ones by name with how many papers each keeps, then one typed.
                    let now = time::now_millis();
                    let kept = |set: &dyn Fn(&mut Filters)| Some(filter::count_with(&lib.papers, set, now));
                    let (current, choices): (String, Vec<(String, String, Option<usize>)>) = match &key {
                        FilterKey::Year => (f.year.clone(), crate::library::YEARS_BACK.iter().map(|n| format!("last:{n}")).map(|value| (value.clone(), crate::library::year_label(&value), kept(&|f| f.year = value.clone()))).collect()),
                        FilterKey::Impact => (f.impact.clone(), crate::library::IMPACTS.iter().map(|least| filter::span_value(Some(least), None)).map(|value| (value.clone(), crate::library::impact_label(&value), kept(&|f| f.impact = value.clone()))).collect()),
                        FilterKey::Zone => {
                            let which = crate::library::zone_sort(&self.settings);
                            (f.zone.clone(), (1..=3).map(|last| filter::zone_value(which, last)).map(|value| (value.clone(), crate::library::zone_label(&value), kept(&|f| f.zone = value.clone()))).collect())
                        }
                        FilterKey::Source => (f.source.clone(), filter::source_counts(&lib.papers).into_iter().take(40).map(|(v, n)| (v.clone(), crate::i18n::phrase(&v), Some(n))).collect()),
                        FilterKey::Reading => (f.reading.clone(), fixed(&READINGS)),
                        FilterKey::Screening => (f.screening.clone(), fixed(&SCREENINGS)),
                        FilterKey::Fulltext => (f.fulltext.clone(), fixed(&FULLTEXTS)),
                        FilterKey::Preprint => (f.preprint.clone(), fixed(&[("yes", t("只看预印本")), ("no", t("不含预印本"))])),
                        FilterKey::Field(id) => (f.fields.get(id).cloned().unwrap_or_default(), counted(filter::field_counts(&lib.papers, id))),
                        FilterKey::Tag => unreachable!("the tags are listed above"),
                    };
                    items.push(MenuItem { label: t("全部").into(), checked: current.is_empty(), note: None, cmd: Cmd::SetFilter(key.clone(), String::new()) });
                    let mut named = current.is_empty();
                    for (value, label, count) in choices {
                        named |= current == value;
                        items.push(MenuItem { label: label.into(), checked: current == value, note: count.map(|n| n.to_string().into()), cmd: Cmd::SetFilter(key.clone(), value) });
                    }
                    match &key {
                        FilterKey::Year => {
                            // The years one by one come last; a span typed is ticked on its own row, which says what it is.
                            let years = counted(filter::years(&lib.papers));
                            let typed = !named && !years.iter().any(|(value, ..)| *value == current);
                            items.push(MenuItem { label: t("自定义区间…").into(), checked: typed, note: typed.then(|| crate::library::year_label(&current).into()), cmd: Cmd::FilterRange(key.clone()) });
                            breaks.push(items.len());
                            for (value, label, count) in years {
                                items.push(MenuItem { label: label.into(), checked: current == value, note: count.map(|n| n.to_string().into()), cmd: Cmd::SetFilter(key.clone(), value) });
                            }
                        }
                        FilterKey::Impact => items.push(MenuItem { label: t("自定义范围…").into(), checked: !named, note: (!named).then(|| crate::library::impact_label(&current).into()), cmd: Cmd::FilterRange(key.clone()) }),
                        FilterKey::Zone => {
                            // Which partition the zones are of is the 分区 column's (设置 › 外观): the same choice, here where it is needed.
                            breaks.push(items.len());
                            let which = crate::library::zone_sort(&self.settings);
                            for (value, label) in wisp_core::journals::ZONES {
                                items.push(MenuItem { label: tf!("分区表：{}", "Partition: {}", t(label)).into(), checked: value == which, note: None, cmd: Cmd::SetJournalZone(if value == "cas" { String::new() } else { value.to_string() }) });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if items.is_empty() { return }
        self.menu = Some(Menu { at, from: None, items, breaks, active: None, paper, filter });
        cx.notify();
    }
}

#[cfg(test)]
mod menu_tests {
    // Not `super::*`: that would bring in gpui's own `test` attribute.
    use super::{MenuSide, menu_place};
    use gpui::{Bounds, Corner, point, px, size};

    #[test]
    fn a_menu_stands_by_what_opened_it_and_never_over_it() {
        let window = px(860.);
        // Under a button, left edges in line, a little apart.
        let button = Bounds::new(point(px(100.), px(40.)), size(px(80.), px(30.)));
        assert_eq!(menu_place(button, MenuSide::Below, px(200.), window), (Corner::TopLeft, point(px(100.), px(74.))));
        // At the window's foot there is no room underneath: over it, the menu's foot a little above the button.
        let low = Bounds::new(point(px(100.), px(800.)), size(px(80.), px(30.)));
        assert_eq!(menu_place(low, MenuSide::Below, px(200.), window), (Corner::BottomLeft, point(px(100.), px(796.))));
        // A menu that fits just under it stays under it.
        let mid = Bounds::new(point(px(100.), px(600.)), size(px(80.), px(30.)));
        assert_eq!(menu_place(mid, MenuSide::Below, px(218.), window).0, Corner::TopLeft);
        assert_eq!(menu_place(mid, MenuSide::Below, px(219.), window).0, Corner::BottomLeft);
        // No room either way: under it, where the window's margin moves it up as it must.
        assert_eq!(menu_place(mid, MenuSide::Below, px(700.), window).0, Corner::TopLeft);
        // A row of the sidebar: to its right, tops in line, the rows under it left alone.
        let row = Bounds::new(point(px(10.), px(290.)), size(px(204.), px(30.)));
        assert_eq!(menu_place(row, MenuSide::Beside, px(200.), window), (Corner::TopLeft, point(px(218.), px(290.))));
    }
}
