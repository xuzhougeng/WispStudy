//! The dialogs: each is a `Modal` variant, built here with its text fields and submitted here
//! (`Workspace::submit_modal`). Drawing them is `dialogs.rs`.

use crate::app::{Cmd, FilterKey, ReviewText, Talk, Workspace};
use crate::importing::IMPORT_WORKERS;
use crate::i18n::t;
use crate::input::{InputEvent, TextInput};
use crate::theme;
use gpui::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use wisp_core::libraries::{self, LibraryInfo};
use wisp_core::lookup::{self, Identifier};
use wisp_core::model::*;
use wisp_core::store::SavedSearch;
use wisp_core::manuscript::Snapshot;
use wisp_core::{complete, pdftext, zotero};

/// 按题目补全: what is asked, the records found, and the one being
/// looked at with the fields to take from it. Nothing is written before
/// the person has picked a record and said which fields. 按 DOI / PMID
/// 补全 (`by_id`) is the same with the one record its number names.
pub struct CompleteForm {
    pub paper_id: String,
    /// Looked up by the paper's DOI or PMID: no title to type, one record, no list to go back to.
    pub by_id: bool,
    /// When the paper was last changed as the dialog opened: a change made meanwhile stops the writing.
    seen: String,
    pub title: Entity<TextInput>,
    pub author: Entity<TextInput>,
    pub year: Entity<TextInput>,
    /// The paper's title is the name of its file: not something to look for.
    pub placeholder: bool,
    /// The title in the box was read off the first page of the PDF.
    pub guessed: bool,
    pub busy: bool,
    pub found: Option<complete::Found>,
    /// For each record found, the other paper of the library that it is, if any: (id, title).
    pub held: Vec<Option<(String, String)>>,
    pub chosen: Option<usize>,
    pub fields: Vec<&'static str>,
}

pub struct PaperForm {
    /// The paper being edited; `None` adds a new one.
    pub id: Option<String>,
    pub title: Entity<TextInput>,
    pub authors: Entity<TextInput>,
    pub year: Entity<TextInput>,
    pub journal: Entity<TextInput>,
    pub volume: Entity<TextInput>,
    pub issue: Entity<TextInput>,
    pub pages: Entity<TextInput>,
    pub doi: Entity<TextInput>,
    pub pmid: Entity<TextInput>,
    pub url: Entity<TextInput>,
    pub tags: Entity<TextInput>,
    pub abstract_text: Entity<TextInput>,
    /// Text and Tags fields, by field id.
    pub fields: Vec<(String, Entity<TextInput>)>,
    /// The chosen option of each Select field.
    pub selects: BTreeMap<String, String>,
}

/// 通过 DOI / PMID 添加 at work on what was typed: how far it is, and what came of each identifier.
#[derive(Default)]
pub struct IdRun {
    pub total: usize,
    pub done: usize,
    /// Ids of the papers added.
    added: Vec<String>,
    /// A sentence for each paper the library (or its trash) already held.
    skipped: Vec<String>,
    /// The identifiers nothing came of, each with the reason.
    failed: Vec<(String, String)>,
}

pub enum Modal {
    ExportNote(Box<crate::notes::Form>),
    /// A library new or edited; one that is a conversation not yet made a library is made one (建成文献库).
    /// `proposing` while the model proposes its name and topic (the sparkle).
    Library { editing: Option<String>, name: Entity<TextInput>, topic: Entity<TextInput>, color: String, proposing: bool },
    /// Another name for a conversation of the home page.
    Rename { id: String, name: Entity<TextInput> },
    /// Another name for a conversation of a library's 历史, in that place's list.
    RenameChat { talk: Talk, id: String, name: Entity<TextInput> },
    /// `run` while the identifiers are looked up.
    AddById { input: Entity<TextInput>, run: Option<IdRun> },
    Paper(Box<PaperForm>),
    Complete(Box<CompleteForm>),
    Fields { editing: Option<String>, name: Entity<TextInput>, options: Entity<TextInput>, hint: Entity<TextInput>, kind: FieldKind, in_table: bool },
    Search { editing: Option<String>, kind: String, name: Entity<TextInput>, query: Entity<TextInput>, max: Entity<TextInput> },
    Confirm { title: String, message: String, action: String, cmd: Cmd },
    /// Another name for a tag, on every paper that has it.
    Tag { tag: String, name: Entity<TextInput> },
    /// The two ends of a filter by year or by impact factor (`FilterKey::Year`, `FilterKey::Impact`); either may be left empty.
    Range { key: FilterKey, low: Entity<TextInput>, high: Entity<TextInput> },
    /// 跳到第几页: the page of the PDF on show to go to, of `count`.
    PdfPage { page: Entity<TextInput>, count: usize },
    /// Tags for several papers at once.
    TagPapers { ids: Vec<String>, name: Entity<TextInput> },
    /// 清空回收站: the papers in the trash deleted from disk, once the phrase is typed.
    EmptyTrash { phrase: Entity<TextInput> },
    /// What came of each PDF of the last import (`Library::import`).
    ImportReport,
    /// `open`: Zotero holds its database right now, as far as the usual place of it tells.
    Zotero { open: bool },
    /// The database picked holds more than one library with items, or collections: which library to import (`None`: all),
    /// and which of its collections (`picked`, `zotero::Collection::id`; none: the whole library).
    ZoteroLibraries { path: PathBuf, libraries: Vec<zotero::Library>, chosen: Option<i64>, collections: Vec<zotero::Collection>, picked: Vec<i64> },
    ZoteroReport,
    /// What came of looking for each paper's open full text (`Library::fetching`).
    FetchReport,
    /// An answer about a paper, to be looked over before it joins that paper's notes.
    Note { paper_id: String, text: Entity<TextInput> },
    /// 写作: a section's name and kind (`editing` its id, `None` for a new one).
    Section { editing: Option<String>, name: Entity<TextInput>, kind: String },
    /// 写作: a section's earlier versions, newest first.
    History { section: String, snapshots: Vec<Snapshot>, chosen: usize },
    /// A claim of a review and the source it rests on.
    ReviewEvidence(Box<crate::chat::EvidenceForm>),
    /// A text of a review's record, to change by hand.
    ReviewText { what: ReviewText, input: Entity<TextInput> },
    /// A figure of the figure library, or a piece of a PDF page about to become one.
    Figure(Box<crate::figures::FigureForm>),
    /// A method of 方法库 or an introduction of 导言库, or a passage of a PDF about to become one (`excerpts.rs`).
    Excerpt(Box<crate::excerpts::ExcerptForm>),
    /// 提取: which fields to ask the assistant about, for which papers.
    Extract(Box<crate::extract::Form>),
    /// 写作: which paragraphs of a Word file without heading styles start a section.
    Headings(Box<crate::comments::HeadingsForm>),
    /// 写作's 资料库: the figures to bring in and to put into the manuscript. Its state is `Library::write_figures`.
    WriteFigures,
    /// 移到文献库: the paper, and the library chosen for it (`transfer.rs`).
    MoveTo { paper: String, to: Option<String> },
    /// 归入已有文献: the papers whose files go under another, what narrows the papers offered, and the one chosen (`filing.rs`).
    FileInto { papers: Vec<String>, query: Entity<TextInput>, to: Option<String> },
}

impl Workspace {
    pub fn modal_input(&mut self, placeholder: &'static str, text: &str, multiline: bool, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        let input = cx.new(|cx| {
            let input = TextInput::new(placeholder, cx);
            if multiline { input.multiline(3).with_text(text) } else { input.with_text(text) }
        });
        if !multiline {
            let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if *event == InputEvent::Submit { this.run(Cmd::SubmitModal, window, cx) }
            });
            self.modal_subscriptions.push(subscription);
        }
        input
    }

    /// Open 按题目补全 for a paper. Its title goes into the box unless it is
    /// only the name of its file; then the first page of its PDF is read
    /// for one, which the person looks over before anything is asked.
    pub(crate) fn complete_dialog(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.modal_subscriptions.clear();
        self.modal_error.clear();
        let Some(lib) = &self.lib else { return };
        let Some(paper) = lib.paper(&id).cloned() else { return };
        let files: Vec<PaperFile> = lib.paper_files(&id).into_iter().cloned().collect();
        let placeholder = complete::placeholder_title(&paper, &files);
        let pdfs: Vec<PathBuf> = files.iter().filter(|file| file.format == "pdf").map(|file| lib.store.file_path(file)).collect();
        let library = lib.info.id.clone();
        let title = self.modal_input("文献的题目", if placeholder { "" } else { &paper.title }, true, window, cx);
        title.update(cx, |input, _| input.min_lines = 2);
        let first_author = paper.authors.split(';').next().unwrap_or("").trim().to_string();
        let author = self.modal_input("一位作者的姓名，可不填", &first_author, false, window, cx);
        let year = self.modal_input("例如 2019，可不填", &paper.year.map(|y| y.to_string()).unwrap_or_default(), false, window, cx);
        title.read(cx).focus(window);
        self.modal = Some(Modal::Complete(Box::new(CompleteForm { paper_id: id.clone(), seen: paper.updated_at.clone(), title, author, year, placeholder, by_id: false, guessed: false, busy: false, found: None, held: vec![], chosen: None, fields: vec![] })));
        cx.notify();
        if !placeholder || pdfs.is_empty() { return }
        cx.spawn(async move |this, cx| {
            let shown = cx.background_executor().spawn(async move { pdfs.into_iter().find_map(|path| pdftext::title_guess(std::fs::read(path).ok()?)) }).await;
            this.update(cx, |this, cx| {
                if this.lib.as_ref().is_none_or(|l| l.info.id != library) { return }
                let (Some(Modal::Complete(form)), Some(shown)) = (this.modal.as_mut(), shown) else { return };
                // Only into a box the person has not written in meanwhile.
                if form.paper_id != id || !form.title.read(cx).text().trim().is_empty() { return }
                form.title.update(cx, |input, cx| input.set_text(&shown, cx));
                form.guessed = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open 按 DOI / PMID 补全 for a paper and ask at once: there is nothing to type.
    pub(crate) fn complete_by_id_dialog(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.modal_subscriptions.clear();
        self.modal_error.clear();
        let Some(lib) = &self.lib else { return };
        let Some(paper) = lib.paper(&id).cloned() else { return };
        if complete::identifier(&paper).is_none() { return }
        let files: Vec<PaperFile> = lib.paper_files(&id).into_iter().cloned().collect();
        let placeholder = complete::placeholder_title(&paper, &files);
        // The title search's boxes stay unused; the form is shared with it.
        let (title, author, year) = (self.modal_input("", "", false, window, cx), self.modal_input("", "", false, window, cx), self.modal_input("", "", false, window, cx));
        self.modal = Some(Modal::Complete(Box::new(CompleteForm { paper_id: id, by_id: true, seen: paper.updated_at.clone(), title, author, year, placeholder, guessed: false, busy: false, found: None, held: vec![], chosen: None, fields: vec![] })));
        window.focus(&self.focus);
        self.complete_lookup(cx);
    }

    /// 按 DOI / PMID 补全: ask for the record the paper's number names, and open it field by field.
    fn complete_lookup(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::Complete(form)) = self.modal.as_mut() else { return };
        if form.busy { return }
        let Some(paper) = self.lib.as_ref().and_then(|lib| lib.paper(&form.paper_id)).cloned() else { return };
        self.modal_error.clear();
        form.busy = true;
        let (paper_id, placeholder) = (form.paper_id.clone(), form.placeholder);
        let library = self.lib.as_ref().map(|l| l.info.id.clone()).unwrap_or_default();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async move { complete::by_identifier(&paper, placeholder, &wisp_net::get_text) }).await;
            this.update(cx, |this, cx| {
                let Some(lib) = this.lib.as_ref().filter(|l| l.info.id == library) else { return };
                let Some(Modal::Complete(form)) = this.modal.as_mut().filter(|modal| matches!(modal, Modal::Complete(form) if form.paper_id == paper_id && form.by_id)) else { return };
                form.busy = false;
                match found {
                    Ok(candidate) => {
                        let held = lib.store.existing_paper(&candidate.paper).ok().flatten().filter(|id| *id != paper_id).map(|id| (id.clone(), lib.paper(&id).or_else(|| lib.trash.iter().map(|entry| &entry.paper).find(|paper| paper.id == id)).map(|paper| paper.title.clone()).unwrap_or_default()));
                        form.fields = match lib.paper(&paper_id) {
                            Some(paper) if candidate.similarity >= complete::ALIKE_ENOUGH => complete::suggested(paper, &candidate.paper, placeholder),
                            _ => vec![],
                        };
                        form.held = vec![held];
                        form.found = Some(complete::Found { candidates: vec![candidate], asked: vec![] });
                        form.chosen = Some(0);
                    }
                    Err(error) => this.modal_error = error,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 按题目补全, the first step: ask the sources about the title in the box.
    fn complete_find(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::Complete(form)) = self.modal.as_mut() else { return };
        if form.busy { return }
        let text = |input: &Entity<TextInput>| input.read(cx).text().trim().to_string();
        let title = text(&form.title);
        if title.is_empty() {
            self.modal_error = t("写上题目再查找。").into();
            return;
        }
        let year = match text(&form.year) {
            year if year.is_empty() => None,
            year => match year.parse::<i32>().ok().filter(|y| (1800..=2200).contains(y)) {
                Some(year) => Some(year),
                None => {
                    self.modal_error = t("年份应为数字，例如 2019；不确定可以留空。").into();
                    return;
                }
            },
        };
        let query = complete::Query { title, author: text(&form.author), year };
        form.busy = true;
        let paper_id = form.paper_id.clone();
        let library = self.lib.as_ref().map(|l| l.info.id.clone()).unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async move { complete::find(&query, &wisp_net::get_text) }).await;
            this.update(cx, |this, cx| {
                let Some(lib) = this.lib.as_ref().filter(|l| l.info.id == library) else { return };
                let Some(Modal::Complete(form)) = this.modal.as_mut().filter(|modal| matches!(modal, Modal::Complete(form) if form.paper_id == paper_id)) else { return };
                form.busy = false;
                // A record the library holds as another paper cannot be written into this one.
                form.held = found.candidates.iter().map(|candidate| lib.store.existing_paper(&candidate.paper).ok().flatten().filter(|id| *id != paper_id).map(|id| (id.clone(), lib.paper(&id).or_else(|| lib.trash.iter().map(|entry| &entry.paper).find(|paper| paper.id == id)).map(|paper| paper.title.clone()).unwrap_or_default()))).collect();
                if found.asked.iter().all(|asked| !asked.error.is_empty()) { this.modal_error = t("几个来源都没有查成，见下面的原因。检查网络或「设置 › 网络」里的代理后再试。").into() }
                form.chosen = None;
                form.found = Some(found);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 按题目补全, the last step: write the fields taken from the record
    /// looked at into the paper.
    fn complete_apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Modal::Complete(form)) = self.modal.as_ref() else { return };
        let (Some(at), Some(found)) = (form.chosen, form.found.as_ref()) else { return };
        let (id, candidate, fields) = (form.paper_id.clone(), found.candidates[at].clone(), form.fields.clone());
        let problem = if form.held.get(at).is_some_and(Option::is_some) {
            Some(t("文献库里已经有这条记录了，不能再写进这一篇。"))
        } else if fields.is_empty() {
            Some(t("没有勾选要写入的字段。"))
        } else if self.lib.as_ref().and_then(|lib| lib.paper(&id)).is_none_or(|paper| paper.updated_at != form.seen) {
            Some(t("这篇文献在查找期间被改过。关掉这个窗口重新打开，按它现在的内容再核对。"))
        } else {
            None
        };
        if let Some(problem) = problem {
            self.modal_error = problem.into();
            return;
        }
        if !self.edit_paper(&id, cx, |paper| complete::apply(paper, &candidate, &fields)) { return }
        self.run(Cmd::CloseModal, window, cx);
        self.say(tf!("已补全：{}", "Completed: {}", self.lib.as_ref().and_then(|lib| lib.paper(&id)).map(|paper| paper.title.clone()).unwrap_or_default()), cx);
    }

    pub(crate) fn library_dialog(&mut self, editing: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.modal_subscriptions.clear();
        self.modal_error.clear();
        let current = editing.as_ref().and_then(|id| self.libraries.iter().find(|l| &l.id == id).or(self.lib.as_ref().map(|l| &l.info)).cloned());
        // A new library, or a conversation about to be made one, takes the next colour of the shelf.
        let next = libraries::COLORS[self.libraries.iter().filter(|l| !l.draft).count() % libraries::COLORS.len()].to_string();
        let (name, topic, color) = match current {
            Some(l) if l.draft => (l.name, l.topic, next),
            Some(l) => (l.name, l.topic, l.color),
            None => (String::new(), String::new(), next),
        };
        let name = self.modal_input("例如：植物根尖单细胞", &name, false, window, cx);
        let topic = self.modal_input("这个文献库收集什么？关注哪些问题？（会作为 AI 提问的背景）", &topic, true, window, cx);
        name.read(cx).focus(window);
        self.modal = Some(Modal::Library { editing, name, topic, color, proposing: false });
    }

    pub(crate) fn paper_dialog(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.modal_subscriptions.clear();
        self.modal_error.clear();
        let Some(lib) = &self.lib else { return };
        let paper = id.as_ref().and_then(|id| lib.paper(id)).cloned().unwrap_or_default();
        let fields = lib.fields.clone();
        let title = self.modal_input("文献题目", &paper.title, true, window, cx);
        title.update(cx, |input, _| input.min_lines = 2);
        let abstract_text = self.modal_input("摘要", &paper.abstract_text, true, window, cx);
        abstract_text.update(cx, |input, _| input.min_lines = 4);
        let mut form = PaperForm {
            id,
            title,
            authors: self.modal_input("作者，用分号分隔：Zhang, Wei; Li, Na", &paper.authors, false, window, cx),
            year: self.modal_input("例如 2024", &paper.year.map(|y| y.to_string()).unwrap_or_default(), false, window, cx),
            journal: self.modal_input("期刊或会议", &paper.journal, false, window, cx),
            volume: self.modal_input("", &paper.volume, false, window, cx),
            issue: self.modal_input("", &paper.issue, false, window, cx),
            pages: self.modal_input("例如 357-362", &paper.pages, false, window, cx),
            doi: self.modal_input("10.xxxx/…", &paper.doi, false, window, cx),
            pmid: self.modal_input("PubMed 编号", &paper.pmid, false, window, cx),
            url: self.modal_input("https://…", &paper.url, false, window, cx),
            tags: self.modal_input("标签，用逗号分隔", &paper.tags.join(", "), false, window, cx),
            abstract_text,
            fields: vec![],
            selects: BTreeMap::new(),
        };
        for field in fields {
            let values = paper.fields.get(&field.id).cloned().unwrap_or_default();
            match field.kind {
                FieldKind::Select => {
                    if let Some(value) = values.first() { form.selects.insert(field.id.clone(), value.clone()); }
                }
                FieldKind::Text => form.fields.push((field.id.clone(), self.modal_input("", &values.join(" "), false, window, cx))),
                FieldKind::Tags => form.fields.push((field.id.clone(), self.modal_input("多个值用逗号分隔", &values.join(", "), false, window, cx))),
            }
        }
        form.title.read(cx).focus(window);
        self.modal = Some(Modal::Paper(Box::new(form)));
    }

    pub(crate) fn fields_dialog(&mut self, editing: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.modal_subscriptions.clear();
        self.modal_error.clear();
        let field = editing.as_ref().and_then(|id| self.lib.as_ref()?.fields.iter().find(|f| &f.id == id).cloned()).unwrap_or(FieldDef { in_table: true, kind: FieldKind::Tags, ..Default::default() });
        let name = self.modal_input("词条名称，例如：物种、研究方法、所属课题", &field.name, false, window, cx);
        let options = self.modal_input("可选值，用逗号分隔（单选必填）", &field.options.join(", "), false, window, cx);
        let hint = self.modal_input("说明（可留空）：让助手提取时找什么，例如“研究用的物种，写学名”", &field.hint, false, window, cx);
        self.modal = Some(Modal::Fields { editing, name, options, hint, kind: field.kind, in_table: field.in_table });
    }

    pub(crate) fn search_dialog(&mut self, editing: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.modal_subscriptions.clear();
        self.modal_error.clear();
        let search = editing.as_ref().and_then(|id| self.lib.as_ref()?.searches.iter().find(|s| &s.id == id).cloned()).unwrap_or(SavedSearch { kind: "pubmed".into(), max_results: 50, enabled: true, ..Default::default() });
        let name = self.modal_input("名称，例如：根尖单细胞（可以留空）", &search.name, false, window, cx);
        let query = self.modal_input("检索式", &search.query, true, window, cx);
        let max = self.modal_input("50", &search.max_results.to_string(), false, window, cx);
        self.modal = Some(Modal::Search { editing, kind: search.kind, name, query, max });
    }

    /// One identifier of 通过 DOI / PMID 添加 looked up: its paper joins the library unless it is held already. With
    /// the last one the dialog closes and says what came of them; those nothing came of stay in it with the reasons,
    /// to be mended and tried again. `false` when the dialog that asked is no longer open on this library.
    fn added_by_id(&mut self, asked: EntityId, library: &str, id: Identifier, found: Result<Paper, String>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !matches!(&self.modal, Some(Modal::AddById { input, run: Some(_) }) if input.entity_id() == asked) { return false }
        let Some(lib) = self.lib.as_mut().filter(|l| l.info.id == library) else { return false };
        let stored = found.and_then(|paper| lib.store.import_papers(vec![paper]).and_then(|result| lib.reload().map(|()| result)).map_err(|e| format!("{e:#}")));
        let Some(Modal::AddById { input, run }) = self.modal.as_mut() else { return false };
        let Some(going) = run.as_mut() else { return false };
        let (Identifier::Doi(name) | Identifier::Pmid(name)) = id;
        going.done += 1;
        match stored.map(|result| (result.added.into_iter().next(), result.skipped.into_iter().next())) {
            Ok((Some(paper), _)) => going.added.push(paper.id),
            Ok((None, Some(held))) => going.skipped.push(if held.in_trash { tf!("「{}」在回收站中。", "“{}” is in the trash.", held.title) } else { tf!("「{}」已在文献库中。", "“{}” is already in this library.", held.title) }),
            Ok((None, None)) => {}
            Err(err) => going.failed.push((name, err)),
        }
        cx.notify();
        if going.done < going.total { return true }
        let (input, run) = (input.clone(), run.take().unwrap_or_default());
        let single = run.total == 1;
        // One paper says its title, a list how many: 「导入 5 篇，跳过 2 篇已有的。」
        let summary = match (run.added.as_slice(), run.skipped.as_slice()) {
            ([], []) => String::new(),
            ([only], []) if single => tf!("已添加：{}", "Added: {}", self.lib.as_ref().and_then(|l| l.paper(only)).map(|p| p.title.clone()).unwrap_or_default()),
            ([], [only]) => only.clone(),
            (added, skipped) => tf!("导入 {} 篇{}。", "Imported {} papers{}.", added.len(), if skipped.is_empty() { String::new() } else { tf!("，跳过 {} 篇已有的", ", skipped {} existing", skipped.len()) }),
        };
        // The one paper asked for and already held is said in the dialog, where the identifier still is.
        if run.failed.is_empty() && !(single && run.added.is_empty()) {
            self.run(Cmd::CloseModal, window, cx);
            if let [only] = run.added.as_slice() { self.select(Some(only.clone()), window, cx) }
            self.say(summary, cx);
            return false;
        }
        let mut lines = vec![];
        if !single {
            input.update(cx, |input, cx| input.set_text(&run.failed.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join("\n"), cx));
            if !summary.is_empty() { lines.push(summary.clone()) }
            lines.push(tf!("{} 个没有添加，留在了上面，可以改好再试：", "{} not added. They are left above to fix and try again:", run.failed.len()));
        } else if run.failed.is_empty() {
            lines.push(summary);
        }
        lines.extend(run.failed.iter().map(|(name, err)| if single { err.clone() } else { format!("{name}：{err}") }));
        self.modal_error = lines.join("\n");
        false
    }

    pub(crate) fn submit_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = |input: &Entity<TextInput>, cx: &App| input.read(cx).text().trim().to_string();
        self.modal_error.clear();
        let Some(modal) = self.modal.as_mut() else { return };
        match modal {
            Modal::ExportNote(_) => {}
            Modal::Confirm { cmd, .. } => {
                let cmd = cmd.clone();
                self.run(cmd, window, cx);
            }
            Modal::ImportReport => self.run(Cmd::CloseModal, window, cx),
            Modal::MoveTo { paper, to } => match to.clone() {
                Some(to) => {
                    let paper = paper.clone();
                    self.run(Cmd::MovePaper(paper, to), window, cx)
                }
                None => self.modal_error = t("选一个文献库。").into(),
            },
            Modal::FileInto { papers, to, .. } => match to.clone() {
                Some(to) => {
                    let papers = papers.clone();
                    self.run(Cmd::FileInto(papers, to), window, cx)
                }
                None => self.choose_only_file_target(cx),
            },
            Modal::Section { .. } | Modal::History { .. } => self.submit_manuscript_modal(window, cx),
            Modal::Headings(_) => {
                self.headings_done(true, cx);
                if self.modal.is_none() { window.focus(&self.focus) }
            }
            Modal::ReviewEvidence(_) => self.submit_review_evidence(window, cx),
            Modal::ReviewText { .. } => self.submit_review_text(window, cx),
            Modal::Figure(_) => self.submit_figure(window, cx),
            Modal::Excerpt(_) => self.submit_excerpt(window, cx),
            Modal::WriteFigures => {}
            Modal::Complete(form) => {
                if form.chosen.is_some() { self.complete_apply(window, cx) } else if form.by_id { self.complete_lookup(cx) } else { self.complete_find(cx) }
            }
            Modal::Zotero { .. } => self.run(Cmd::PickZotero, window, cx),
            Modal::ZoteroLibraries { path, chosen, picked, .. } => {
                let (path, chosen, picked) = (path.clone(), *chosen, picked.clone());
                self.import_zotero(path, chosen, picked, cx);
            }
            Modal::ZoteroReport => self.run(Cmd::CloseModal, window, cx),
            Modal::FetchReport => self.run(Cmd::CloseModal, window, cx),
            Modal::Library { editing, name, topic, color, .. } => {
                let (name, topic, color) = (text(name, cx), text(topic, cx), color.clone());
                let mut created = None;
                let result = match editing.clone() {
                    None => libraries::create(&self.root, &name, &topic, &color).map(|info| created = Some(info.id)),
                    Some(id) => {
                        // The open library's own record is the newer one: its screening goal is typed on 筛选 and kept there, not in the home page's list.
                        let Some(mut info) = self.lib.as_ref().filter(|l| l.info.id == id).map(|l| l.info.clone()).or_else(|| self.libraries.iter().find(|l| l.id == id).cloned()) else { return };
                        // 建成文献库: the conversation is the library from now on, in place (`draft.rs`).
                        let made = if info.draft { libraries::promote(&mut info, &name, &topic, &color) } else {
                            (info.name, info.topic, info.color) = (name, topic, color);
                            libraries::save(&info)
                        };
                        made.map(|_| {
                            if let Some(lib) = self.lib.as_mut().filter(|l| l.info.id == info.id) {
                                theme::set_accent(&info.color);
                                window.set_window_title(&format!("{} — Wisp Study", info.name));
                                // A changed topic changes what the assistant is told.
                                lib.library_chat.forget_context();
                                lib.paper_chat.forget_context();
                                lib.info = LibraryInfo { paper_count: lib.info.paper_count, opened_at: lib.info.opened_at.clone(), ..info };
                            }
                        })
                    }
                };
                match result {
                    Ok(()) => {
                        self.libraries = libraries::list(&self.root);
                        self.run(Cmd::CloseModal, window, cx);
                        // A new library opens on its guide: where to start filling it.
                        if let Some(id) = created {
                            self.run(Cmd::OpenLibrary(id), window, cx);
                            self.run(Cmd::OpenGuide(true), window, cx);
                        }
                    }
                    Err(err) => self.modal_error = format!("{err:#}"),
                }
            }
            Modal::Rename { id, name } => {
                let (id, name) = (id.clone(), text(name, cx));
                let Some(mut info) = self.lib.as_ref().filter(|l| l.info.id == id).map(|l| l.info.clone()).or_else(|| self.libraries.iter().find(|l| l.id == id).cloned()) else { return };
                info.name = name;
                match libraries::save(&info) {
                    Ok(()) => {
                        if let Some(lib) = self.lib.as_mut().filter(|l| l.info.id == id) {
                            lib.info.name = info.name.clone();
                            window.set_window_title(&format!("{} — Wisp Study", info.name));
                        }
                        self.libraries = libraries::list(&self.root);
                        self.run(Cmd::CloseModal, window, cx);
                    }
                    Err(err) => self.modal_error = format!("{err:#}"),
                }
            }
            Modal::RenameChat { talk, id, name } => {
                let (talk, id, name) = (*talk, id.clone(), text(name, cx));
                let Some(lib) = self.lib.as_mut() else { return };
                match lib.store.rename_conversation(&id, &name).and_then(|()| lib.list_history(talk)) {
                    Ok(list) => {
                        if lib.chat(talk).history.is_some() { lib.chat_mut(talk).history = Some(list) }
                        self.run(Cmd::CloseModal, window, cx);
                    }
                    Err(err) => self.modal_error = format!("{err:#}"),
                }
            }
            Modal::Tag { tag, name } => {
                let (tag, name) = (tag.clone(), text(name, cx));
                if name.is_empty() {
                    self.modal_error = t("标签不能没有名字。要去掉它，在标签上点右键选「去掉这个标签」。").into();
                    return;
                }
                if name == tag || self.retag(&tag, Some(&name), cx) { self.run(Cmd::CloseModal, window, cx) }
            }
            Modal::PdfPage { page, count } => {
                let (typed, count) = (text(page, cx), *count);
                match crate::library::page_typed(&typed, count) {
                    Ok(page) => {
                        self.run(Cmd::CloseModal, window, cx);
                        if let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.go_to(page, cx)) }
                    }
                    Err(why) => self.modal_error = why,
                }
            }
            Modal::Range { key, low, high } => {
                let (key, low, high) = (key.clone(), text(low, cx), text(high, cx));
                match crate::library::range_value(&key, &low, &high) {
                    Ok(value) => {
                        self.run(Cmd::CloseModal, window, cx);
                        self.run(Cmd::SetFilter(key, value), window, cx);
                    }
                    Err(why) => self.modal_error = why,
                }
            }
            Modal::EmptyTrash { phrase } => {
                if crate::dialogs::phrase_typed(&text(phrase, cx)) { self.run(Cmd::EmptyTrash, window, cx) }
            }
            Modal::TagPapers { ids, name } => {
                let (ids, tags) = (ids.clone(), wisp_core::model::split_list(&text(name, cx)));
                if tags.is_empty() {
                    self.modal_error = t("写上要添加的标签，多个用逗号或分号隔开。").into();
                    return;
                }
                let done = self.change(cx, |lib| {
                    for id in &ids {
                        let mut paper = lib.store.get_paper(id)?;
                        for tag in &tags { if !paper.tags.contains(tag) { paper.tags.push(tag.clone()) } }
                        lib.store.save_paper(&mut paper)?;
                    }
                    Ok(())
                });
                if done {
                    self.run(Cmd::CloseModal, window, cx);
                    self.say(tf!("已为 {} 篇文献添加标签。", "Added tags to {} papers.", ids.len()), cx);
                }
            }
            Modal::Note { paper_id, text: input } => {
                let (paper_id, note) = (paper_id.clone(), text(input, cx));
                if note.is_empty() {
                    self.modal_error = t("没有内容可以记下。").into();
                    return;
                }
                // Added after what is there: the reader's own notes are never written over.
                let mut joined = String::new();
                let saved = self.edit_paper(&paper_id, cx, |paper| {
                    let before = paper.notes.trim_end();
                    paper.notes = if before.is_empty() { note.clone() } else { format!("{before}\n\n{note}") };
                    joined = paper.notes.clone();
                });
                if !saved { return }
                if let Some(lib) = self.lib.as_ref().filter(|lib| lib.selected.as_deref() == Some(paper_id.as_str())) { lib.notes.update(cx, |input, cx| input.set_text(&joined, cx)) }
                self.run(Cmd::CloseModal, window, cx);
                self.say(t("已记到这篇文献的「我的笔记」里。"), cx);
            }
            Modal::AddById { input, run } => {
                if run.is_some() { return }
                let (ids, words) = lookup::parse_identifiers(&text(input, cx));
                // The web addresses among the other words: an article's page at its publisher, or its PDF.
                let (urls, unknown): (Vec<String>, Vec<String>) = words.into_iter().partition(|word| wisp_core::webpage::is_address(word));
                if ids.is_empty() && urls.is_empty() {
                    self.modal_error = t("没有认出 DOI、PMID 或网址。DOI 形如 10.1038/s41586-020-2649-2，PMID 是一串数字，网址以 https:// 开头。").into();
                    return;
                }
                // Several, or any address, are an import of their own, in the background; what is neither is left out.
                if ids.len() > 1 || !unknown.is_empty() || !urls.is_empty() {
                    self.run(Cmd::CloseModal, window, cx);
                    // The addresses first: the identifiers then join their run, and the words left out are said in it.
                    self.import_urls(urls, cx);
                    self.import_ids(ids, unknown, cx);
                    return;
                }
                *run = Some(IdRun { total: ids.len(), ..Default::default() });
                // The dialog that asked, told by its field: closed meanwhile, its answers are dropped, and it is not
                // taken for a dialog opened after it (which it would close, adding papers nobody is waiting for).
                let asked = input.entity_id();
                let library = self.lib.as_ref().map(|l| l.info.id.clone()).unwrap_or_default();
                // The wait is the two services asked about each identifier: a few are asked side by side.
                let queue = std::rc::Rc::new(std::cell::RefCell::new(ids.into_iter().rev().collect::<Vec<_>>()));
                for _ in 0..IMPORT_WORKERS.min(queue.borrow().len()) {
                    let (queue, library) = (queue.clone(), library.clone());
                    cx.spawn_in(window, async move |this, cx| {
                        loop {
                            let Some(id) = queue.borrow_mut().pop() else { break };
                            let looked = id.clone();
                            let found = cx.background_executor().spawn(async move { wisp_net::lookup_paper(&looked) }).await;
                            let open = this.update_in(cx, |this, window, cx| this.added_by_id(asked, &library, id, found, window, cx));
                            if !matches!(open, Ok(true)) { break }
                        }
                    })
                    .detach();
                }
            }
            Modal::Paper(form) => {
                let Some(lib) = self.lib.as_mut() else { return };
                let mut paper = form.id.as_ref().and_then(|id| lib.paper(id)).cloned().unwrap_or_default();
                let year = text(&form.year, cx);
                paper.year = match year.parse::<i32>() {
                    Ok(year) => Some(year),
                    Err(_) if year.is_empty() => None,
                    Err(_) => {
                        self.modal_error = t("年份应为数字，例如 2024；不确定可以留空。").into();
                        return;
                    }
                };
                paper.title = text(&form.title, cx);
                paper.authors = text(&form.authors, cx);
                paper.journal = text(&form.journal, cx);
                paper.volume = text(&form.volume, cx);
                paper.issue = text(&form.issue, cx);
                paper.pages = text(&form.pages, cx);
                paper.doi = text(&form.doi, cx);
                paper.pmid = text(&form.pmid, cx);
                paper.url = text(&form.url, cx);
                paper.tags = split_list(&text(&form.tags, cx));
                paper.abstract_text = text(&form.abstract_text, cx);
                paper.fields.clear();
                for (id, input) in &form.fields {
                    let value = text(input, cx);
                    let kind = lib.fields.iter().find(|f| &f.id == id).map(|f| f.kind);
                    let values = if kind == Some(FieldKind::Tags) { split_list(&value) } else { clean_strings([value]) };
                    if !values.is_empty() { paper.fields.insert(id.clone(), values); }
                }
                for (id, value) in &form.selects {
                    paper.fields.insert(id.clone(), vec![value.clone()]);
                }
                let editing = form.id.is_some();
                let result = if editing { lib.store.save_paper(&mut paper).map(|_| paper.id.clone()) } else { lib.store.add_paper(paper).map(|p| p.id) };
                match result {
                    Ok(id) => {
                        self.change(cx, |_| Ok(()));
                        self.run(Cmd::CloseModal, window, cx);
                        if !editing { self.select(Some(id), window, cx) }
                        // An edited title or abstract is a new text to translate.
                        self.translate_selected(false, cx);
                        self.say(if editing { t("修改已保存。") } else { t("已添加。") }, cx);
                    }
                    Err(err) => self.modal_error = format!("{err:#}"),
                }
            }
            Modal::Extract(_) => self.submit_extract(window, cx),
            Modal::Fields { editing, name, options, hint, kind, in_table } => {
                let mut field = FieldDef { id: editing.clone().unwrap_or_default(), name: text(name, cx), kind: *kind, options: split_list(&text(options, cx)), in_table: *in_table, hint: text(hint, cx) };
                let Some(lib) = self.lib.as_mut() else { return };
                match lib.store.save_field(&mut field) {
                    Ok(()) => {
                        self.change(cx, |_| Ok(()));
                        self.fields_dialog(None, window, cx);
                    }
                    Err(err) => self.modal_error = format!("{err:#}"),
                }
            }
            Modal::Search { editing, kind, name, query, max } => {
                let Ok(max_results) = text(max, cx).parse::<usize>() else {
                    self.modal_error = t("每次取回的篇数应为数字，例如 50。").into();
                    return;
                };
                let Some(lib) = self.lib.as_mut() else { return };
                // An edited search keeps whether it is enabled.
                let enabled = editing.as_ref().and_then(|id| lib.searches.iter().find(|s| &s.id == id)).is_none_or(|s| s.enabled);
                let mut search = SavedSearch { id: editing.clone().unwrap_or_default(), kind: kind.clone(), name: text(name, cx), query: text(query, cx), max_results, enabled, ..Default::default() };
                match lib.store.save_search(&mut search) {
                    Ok(()) => {
                        // What the old query found says nothing about the new one.
                        lib.trial = None;
                        self.change(cx, |_| Ok(()));
                        self.run(Cmd::CloseModal, window, cx);
                    }
                    Err(err) => self.modal_error = format!("{err:#}"),
                }
            }
        }
        cx.notify();
    }
}
