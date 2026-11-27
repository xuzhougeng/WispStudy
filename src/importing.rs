//! Taking papers in: PDFs and folders picked or dropped (`import_paths`, a few read at a time), a Zotero
//! database, identifiers (DOI / PMID) and addresses. Each run keeps what came of every file (`ImportRun`), which
//! the report shows.

use crate::app::{Page, Talk, Workspace};
use crate::modal::Modal;
use crate::i18n::t;
use gpui::*;
use std::path::PathBuf;
use serde_json::Value;
use wisp_core::lookup::{self, Identifier};
use wisp_core::model::*;
use wisp_core::store::Store;
use wisp_core::{archive, fulltext, import, zotero};

/// The PDFs of an import read at the same time.
pub(crate) const IMPORT_WORKERS: usize = 3;

/// What became of one PDF of an import.
#[derive(Clone, Debug, PartialEq)]
pub enum Imported {
    Added,
    /// The library already held its paper: the file was attached to that.
    Attached,
    /// Its paper is in the trash: nothing was imported.
    InTrash,
    /// An identifier of a paper the library already holds: nothing to do.
    Held,
    Failed(String),
}

pub struct ImportRow {
    /// The file's name, or the identifier looked up.
    pub name: String,
    pub outcome: Imported,
    pub paper_id: Option<String>,
    /// No DOI was found in the file: its title and authors are still to be filled in.
    pub unrecognised: bool,
    /// Why no text came of the file; empty when it did.
    pub textless: String,
    /// An address pasted (`import_url`) whose paper came without its PDF: why, in a sentence.
    pub no_pdf: String,
}

impl ImportRow {
    /// Whether the person still has something to do about this file.
    pub fn open(&self) -> bool {
        self.unrecognised || !self.textless.is_empty() || !self.no_pdf.is_empty() || matches!(self.outcome, Imported::Failed(_) | Imported::InTrash)
    }
}

/// The PDFs dropped or picked in one go, or the identifiers pasted into 通过 DOI / PMID 添加 (more of either
/// meanwhile join it): how far the reading is, then what came of each, until it is put away.
#[derive(Default)]
pub struct ImportRun {
    pub total: usize,
    /// How many of `total` are identifiers looked up rather than files read.
    pub ids: usize,
    /// Names of the files currently being read; duplicate names are separate jobs.
    pub working: Vec<String>,
    pub rows: Vec<ImportRow>,
    /// Words pasted with the identifiers that are neither a DOI nor a PMID: left out, said once in the report.
    pub unknown: Vec<String>,
    /// How many of `ids` are web addresses (`import_url`) rather than identifiers.
    pub urls: usize,
}

#[derive(Default)]
pub struct ZoteroRun {
    pub id: String,
    pub running: bool,
    pub total: usize,
    pub current: String,
    pub rows: Vec<zotero::Row>,
    pub other_attachments: usize,
    pub error: String,
}

impl ZoteroRun {
    pub fn summary(&self) -> String {
        if self.running { return tf!("正在导入 Zotero · {}/{} · {}", "Importing Zotero · {}/{} · {}", self.rows.len(), self.total, self.current) }
        let pdfs = tn!("新增 PDF {n} 个", "{n} added", self.rows.iter().map(|r| r.files_added).sum::<usize>(), "PDF");
        tf!("新增 {} 篇 · 已有 {} 篇 · 回收站 {} 篇 · 失败 {} 篇 · {pdfs} · 待处理 {} 项", "{} added · {} existing · {} in trash · {} failed · {pdfs} · {} pending",
            self.rows.iter().filter(|r| r.added).count(),
            self.rows.iter().filter(|r| !r.added && r.paper_id.is_some()).count(),
            self.rows.iter().filter(|r| r.in_trash).count(),
            self.rows.iter().filter(|r| r.paper_id.is_none() && !r.in_trash).count(),
            self.rows.iter().map(|r| r.problems.len()).sum::<usize>())
    }
}

impl ImportRun {
    pub fn running(&self) -> bool {
        self.rows.len() < self.total
    }

    /// The job's name, as what it imports: PDFs, identifiers, or both.
    pub fn title(&self) -> &'static str {
        match (self.ids, self.running()) {
            (0, true) => t("导入 PDF"),
            (0, false) => t("PDF 导入完成"),
            (ids, true) if ids == self.total && self.urls == 0 => t("按 DOI / PMID 导入"),
            (ids, false) if ids == self.total && self.urls == 0 => t("DOI / PMID 导入完成"),
            (_, true) if self.urls == self.total => t("按网址导入"),
            (_, false) if self.urls == self.total => t("网址导入完成"),
            (_, true) => t("正在导入"),
            (_, false) => t("导入完成"),
        }
    }

    /// How many files came to this.
    pub fn count(&self, of: impl Fn(&ImportRow) -> bool) -> usize {
        self.rows.iter().filter(|row| of(row)).count()
    }

    /// What came of the files, in a line: 「新增 15 · 附到已有文献 3」.
    pub fn summary(&self) -> String {
        let mut facts = vec![];
        for (n, text) in [
            (self.count(|row| row.outcome == Imported::Added), t("新增")),
            (self.count(|row| row.outcome == Imported::Attached), t("附到已有文献")),
            (self.count(|row| row.outcome == Imported::InTrash), t("在回收站里、未导入")),
            (self.count(|row| row.outcome == Imported::Held), t("已有、跳过")),
            (self.count(|row| matches!(row.outcome, Imported::Failed(_))), t("没有成功")),
            (self.unknown.len(), t("没认出、跳过")),
        ] {
            if n > 0 { facts.push(format!("{text} {n}")) }
        }
        facts.join(" · ")
    }
}

/// What `import_url` said of one address, as a row of the import's report.
fn url_row(url: &str, result: Result<Value, String>) -> ImportRow {
    let mut row = ImportRow { name: url.to_string(), outcome: Imported::Added, paper_id: None, unrecognised: false, textless: String::new(), no_pdf: String::new() };
    let found = match result {
        Ok(found) => found,
        Err(err) => {
            row.outcome = Imported::Failed(err);
            return row;
        }
    };
    let said = |key: &str| found[key].as_str().unwrap_or("").to_string();
    let pdf = said("pdf");
    row.outcome = match said("outcome").as_str() {
        "added" => Imported::Added,
        "attached" => Imported::Attached,
        // The library held the paper and the address brought its PDF.
        "held" if pdf == "saved" => Imported::Attached,
        "held" => Imported::Held,
        "in_trash" => Imported::InTrash,
        _ => Imported::Failed(said("reason")),
    };
    row.paper_id = found["paperId"].as_str().map(str::to_string);
    row.unrecognised = found["unrecognised"].as_bool().unwrap_or(false);
    row.textless = said("textReason");
    if pdf == "none" { row.no_pdf = said("pdfReason") }
    row
}

impl Workspace {
    pub(crate) fn pick_zotero(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        if lib.zotero.as_ref().is_some_and(|run| run.running) { return }
        let library = lib.info.id.clone();
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some(t("选择 zotero.sqlite").into()), extensions: Some(vec!["sqlite".into()]) });
        cx.spawn(async move |this, cx| {
            let result = picked.await;
            this.update(cx, |this, cx| {
                if this.lib.as_ref().is_none_or(|l| l.info.id != library) { return }
                match result {
                    Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                        Some(path) => this.zotero_picked(path, cx),
                        None => this.modal_error = t("没有得到所选文件的路径，请重新选择 zotero.sqlite。").into(),
                    },
                    Ok(Ok(None)) => {}
                    Ok(Err(err)) => { this.modal_error = tf!("无法打开文件选择器：{err:#}", "Cannot open file picker: {err:#}"); }
                    Err(err) => { this.modal_error = tf!("文件选择器未能返回结果：{err}", "File picker returned no result: {err}"); }
                }
                cx.notify();
            }).ok();
        }).detach();
    }

    /// A database picked: when more than one of its libraries holds items, the person chooses which to import;
    /// otherwise the import starts. A database that cannot be listed goes on to the import, whose report says why.
    pub(crate) fn zotero_picked(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let library = lib.info.id.clone();
        let listing = path.clone();
        cx.spawn(async move |this, cx| {
            // Collections that cannot be listed leave the choice of a library, as before there were any.
            let (listed, collections) = cx.background_executor().spawn(async move { (zotero::libraries(&listing), zotero::collections(&listing).unwrap_or_default()) }).await;
            this.update(cx, |this, cx| {
                // Closed meanwhile, or another library opened: nothing goes on.
                if this.lib.as_ref().is_none_or(|l| l.info.id != library) || !matches!(this.modal, Some(Modal::Zotero { .. })) { return }
                let offered: Vec<zotero::Library> = listed.map(|all| all.into_iter().filter(|l| l.items > 0).collect()).unwrap_or_default();
                // An empty collection imports nothing, and neither does what is under it.
                let collections: Vec<zotero::Collection> = collections.into_iter().filter(|c| c.items > 0 && offered.iter().any(|l| l.id == c.library)).collect();
                if offered.is_empty() || (offered.len() < 2 && collections.is_empty()) { return this.import_zotero(path, None, vec![], cx) }
                this.modal_error.clear();
                this.modal = Some(Modal::ZoteroLibraries { path, chosen: Some(offered[0].id), libraries: offered, collections, picked: vec![] });
                cx.notify();
            }).ok();
        }).detach();
    }

    /// `only`: one library of the database (`zotero::Library::id`); `None` imports all of them.
    /// `collections`: only the items in these (`zotero::Collection::id`) and what is under them; none, the whole library.
    pub(crate) fn import_zotero(&mut self, path: PathBuf, only: Option<i64>, collections: Vec<i64>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.zotero.as_ref().is_some_and(|run| run.running) { return }
        let library = lib.info.id.clone();
        let root = lib.store.root().to_path_buf();
        let run_id = new_id("Z");
        lib.zotero = Some(ZoteroRun { id: run_id.clone(), running: true, current: t("正在读取数据库…").into(), ..Default::default() });
        lib.page = Page::Papers;
        self.modal = Some(Modal::ZoteroReport);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let parsed = cx.background_executor().spawn(async move { zotero::read(&path, only, &collections) }).await;
            let entries = this.update(cx, |this, cx| {
                let run = this.lib.as_mut().filter(|l| l.info.id == library)?.zotero.as_mut().filter(|r| r.id == run_id)?;
                let entries = match parsed {
                    Ok(imported) => {
                        run.total = imported.entries.len();
                        run.other_attachments = imported.other_attachments;
                        Some(imported.entries)
                    }
                    Err(err) => { run.error = format!("{err:#}"); run.running = false; None }
                };
                cx.notify();
                entries
            });
            let Ok(Some(entries)) = entries else { return };
            for entry in entries {
                let active = this.update(cx, |this, cx| {
                    let Some(run) = this.lib.as_mut().filter(|l| l.info.id == library).and_then(|l| l.zotero.as_mut()).filter(|r| r.id == run_id) else { return false };
                    run.current = entry.paper.title.clone();
                    cx.notify();
                    true
                });
                if !matches!(active, Ok(true)) { return }
                let folder = root.clone();
                let row = cx.background_executor().spawn(async move {
                    match Store::open(&folder) {
                        Ok(mut store) => zotero::import_entry(&mut store, entry),
                        Err(err) => zotero::Row { title: entry.paper.title, problems: vec![format!("{err:#}")], ..Default::default() },
                    }
                }).await;
                let active = this.update(cx, |this, cx| {
                    let Some(run) = this.lib.as_mut().filter(|l| l.info.id == library).and_then(|l| l.zotero.as_mut()).filter(|r| r.id == run_id) else { return false };
                    run.rows.push(row);
                    cx.notify();
                    true
                });
                if !matches!(active, Ok(true)) { return }
            }
            this.update(cx, |this, cx| {
                let Some(lib) = this.lib.as_mut().filter(|l| l.info.id == library) else { return };
                let Some(run) = lib.zotero.as_mut().filter(|r| r.id == run_id) else { return };
                run.running = false;
                if let Err(err) = lib.reload() { this.fail(format!("{err:#}"), cx) }
                this.extract_missing(cx);
                cx.notify();
            }).ok();
        }).detach();
    }

    pub(crate) fn pick_files(&mut self, prompt: &'static str, pdfs: bool, cx: &mut Context<Self>) {
        let extensions: Vec<SharedString> = if pdfs { vec!["pdf".into()] } else { vec!["bib".into(), "ris".into(), "txt".into(), "enw".into()] };
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some(prompt.into()), extensions: Some(extensions) });
        self.import_picked(picked, cx);
    }

    /// What a file picker gave goes to the import; a picker that failed says so instead of doing nothing.
    fn import_picked<E: std::fmt::Display + 'static>(&mut self, picked: impl Future<Output = Result<anyhow::Result<Option<Vec<PathBuf>>>, E>> + 'static, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = picked.await;
            this.update(cx, |this, cx| match result {
                Ok(Ok(Some(paths))) => this.import_paths(paths, cx),
                Ok(Ok(None)) => {}
                Ok(Err(err)) => this.fail(tf!("无法打开文件选择器：{err:#}", "Cannot open file picker: {err:#}"), cx),
                Err(err) => this.fail(tf!("文件选择器未能返回结果：{err}", "File picker returned no result: {err}"), cx),
            }).ok();
        })
        .detach();
    }

    /// Folders picked: every PDF under them, tagged with the folders it lies in.
    pub(crate) fn pick_folders(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: true, prompt: Some(t("导入").into()), extensions: None });
        self.import_picked(picked, cx);
    }

    /// Files picked or dropped: PDFs become papers, reference files are
    /// imported, and a folder gives every PDF under it, tagged with the
    /// folders between it and the file (`import::folder_pdfs`).
    pub fn import_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        // A library from another Wisp Study becomes a library of its own, wherever it is dropped.
        let (archives, paths): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|path| path.is_file() && archive::is_archive_name(path));
        for path in archives { self.import_library(path, cx) }
        if paths.is_empty() { return }
        // On the home page a Word file or a PDF goes with the first question to its assistant (`draft.rs`).
        if self.lib.is_none() { return self.home_attach(paths, cx) }
        // On the assistant's page a Word file goes with the next question: it was written elsewhere and stays so, its citations are what is asked about.
        // In a conversation not yet made a library it is the manuscript to review, as below. A PowerPoint file goes with the next
        // question in both: the reference slides are made after, read where it is.
        let (asking, draft) = self.lib.as_ref().map_or((false, false), |lib| (lib.page == Page::Ask, lib.info.draft));
        let (documents, paths): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|path| asking && path.is_file() && ((!draft && crate::comments::is_word(path)) || crate::comments::is_deck(path)));
        if !documents.is_empty() { self.attach_documents(Talk::Library, documents, cx) }
        if paths.is_empty() { return }
        // Elsewhere a Word file is a manuscript to go on with; Markdown and plain text are, on the writing page.
        let writing = self.lib.as_ref().is_some_and(|lib| lib.page == Page::Write);
        let (drafts, paths): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|path| path.is_file() && (path.extension().is_some_and(|e| e.eq_ignore_ascii_case("docx")) || (writing && (crate::comments::is_manuscript_file(path) || path.extension().is_some_and(|e| e.eq_ignore_ascii_case("txt"))))));
        if !drafts.is_empty() { self.import_manuscript_files(drafts, cx) }
        if paths.is_empty() { return }
        let mut pdfs: Vec<(PathBuf, Vec<String>)> = vec![];
        let mut others = vec![];
        let mut empty_folders = vec![];
        for path in paths {
            if path.is_dir() {
                let found = import::folder_pdfs(&path);
                if found.is_empty() { empty_folders.push(path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()) }
                pdfs.extend(found);
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
                pdfs.push((path, vec![]));
            } else {
                others.push(path);
            }
        }
        if !empty_folders.is_empty() && pdfs.is_empty() && others.is_empty() {
            return self.fail(tf!("文件夹「{}」里没有 PDF。", "No PDFs in folder “{}”.", empty_folders.join("」「")), cx);
        }
        let mut added = vec![];
        let mut skipped = 0;
        let mut errors = vec![];
        // Records a file holds that could not be kept as they are: the rest of the file comes in all the same.
        let mut left_out = vec![];
        let several = others.len() > 1;
        for path in others {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let parsed = std::fs::read(&path).map_err(|e| tf!("无法读取 {name}：{e}", "Cannot read {name}: {e}")).and_then(|bytes| import::parse_references(&String::from_utf8_lossy(&bytes), &tf!("导入 · {name}", "Import · {name}")).map_err(|e| format!("{name}：{e}")));
            match parsed {
                Ok(papers) => {
                    let mut result = None;
                    self.change(cx, |lib| {
                        result = Some(lib.store.import_records(papers)?);
                        Ok(())
                    });
                    if let Some(result) = result {
                        skipped += result.skipped.len();
                        added.extend(result.added.into_iter().map(|p| p.id));
                        let file = if several { format!("{name} ") } else { String::new() };
                        left_out.extend(result.invalid.into_iter().map(|record| {
                            let title = if record.title.chars().count() > 40 { format!("{}…", record.title.chars().take(40).collect::<String>()) } else { record.title };
                            tf!("{file}第 {} 条「{title}」：{}", "{file}record {} “{title}”: {}", record.n, record.why)
                        }));
                    }
                }
                Err(err) => errors.push(err),
            }
        }
        let done = tf!("导入 {} 篇{}", "Imported {} papers{}", added.len(), if skipped > 0 { tf!("，跳过 {skipped} 篇已有的", ", skipped {skipped} existing") } else { String::new() });
        if !errors.is_empty() {
            self.fail(errors.join("；"), cx);
        } else if !left_out.is_empty() {
            let more = if left_out.len() > 2 { tf!("等 {} 条", " and {} more", left_out.len() - 2) } else { String::new() };
            self.fail(tf!("{done}；{} 条没有收进：{}{more}。", "{done}; {} not taken in: {}{more}.", left_out.len(), left_out.iter().take(2).cloned().collect::<Vec<_>>().join("；")), cx);
        } else if !added.is_empty() || skipped > 0 {
            self.say(tf!("{done}。", "{done}."), cx);
        }
        if pdfs.is_empty() { return }
        let Some(lib) = self.lib.as_mut() else { return };
        // More files while some are still being read join them; a report already read gives way.
        match lib.import.as_mut().filter(|run| run.running()) {
            Some(run) => run.total += pdfs.len(),
            None => lib.import = Some(ImportRun { total: pdfs.len(), ..Default::default() }),
        }
        let library = lib.info.id.clone();
        cx.notify();
        // Most of the wait is the two services asked about each DOI: a few files are read side by side.
        let queue = std::rc::Rc::new(std::cell::RefCell::new(pdfs.into_iter().rev().collect::<Vec<_>>()));
        for _ in 0..IMPORT_WORKERS.min(queue.borrow().len()) {
            let (queue, library) = (queue.clone(), library.clone());
            cx.spawn(async move |this, cx| {
                loop {
                    let Some((path, tags)) = queue.borrow_mut().pop() else { break };
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let known = this.update(cx, |this, cx| {
                        let Some(run) = this.lib.as_mut().filter(|l| l.info.id == library).and_then(|l| l.import.as_mut()) else { return false };
                        run.working.push(name);
                        cx.notify();
                        true
                    });
                    if !matches!(known, Ok(true)) { break }
                    // Read the text, look for a DOI on the first pages, and ask Crossref about it.
                    let file = path.clone();
                    let (pages, found) = cx
                        .background_executor()
                        .spawn(async move {
                            let pages = fulltext::file_pages(&file);
                            let doi = pages.as_ref().ok().and_then(|pages| lookup::find_doi(&pages.iter().take(2).cloned().collect::<Vec<_>>().join("\n")));
                            let found = doi.and_then(|doi| wisp_net::lookup_paper(&Identifier::Doi(doi)).ok());
                            (pages, found)
                        })
                        .await;
                    let done = this.update(cx, |this, cx| {
                        if this.lib.as_ref().is_none_or(|l| l.info.id != library) { return false }
                        this.add_pdf(&path, &tags, pages, found, cx);
                        true
                    });
                    if !matches!(done, Ok(true)) { break }
                }
            })
            .detach();
        }
    }

    /// One read PDF: a new paper with the file attached, or the file attached
    /// to the paper the library already holds for its DOI. What came of it
    /// joins the import's report; a single file says so at once instead.
    /// `tags` (the folders the file lay in) go to the paper, a paper already
    /// held included.
    pub(crate) fn add_pdf(&mut self, path: &std::path::Path, tags: &[String], pages: Result<Vec<String>, String>, found: Option<Paper>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let stem = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| t("未命名文献").into());
        let recognised = found.is_some();
        let mut paper = found.unwrap_or_else(|| Paper { title: stem.clone(), ..Default::default() });
        paper.source = t("PDF 导入").into();
        paper.tags = clean_strings(paper.tags.iter().chain(tags));
        let mut row = ImportRow { name: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| stem.clone()), outcome: Imported::Added, paper_id: None, unrecognised: false, textless: String::new(), no_pdf: String::new() };
        let stored = (|| -> anyhow::Result<()> {
            let known = lib.store.existing_paper(&{
                let mut probe = paper.clone();
                probe.normalize();
                probe
            })?;
            // The same file under another name is the paper that holds it: without a DOI nothing else says so.
            let known = match known {
                Some(known) => Some(known),
                None => lib.store.paper_holding(path)?,
            };
            let target = match known {
                Some(existing) => {
                    if lib.trash.iter().any(|t| t.paper.id == existing) {
                        row.outcome = Imported::InTrash;
                        return Ok(());
                    }
                    row.outcome = Imported::Attached;
                    let mut held = lib.store.get_paper(&existing)?;
                    if tags.iter().any(|tag| !held.tags.contains(tag)) {
                        held.tags = clean_strings(held.tags.iter().chain(tags));
                        lib.store.save_paper(&mut held)?;
                    }
                    existing
                }
                None => {
                    row.unrecognised = !recognised;
                    lib.store.add_paper(paper)?.id
                }
            };
            let kind = if lib.store.paper_files(&target)?.iter().any(|f| f.kind == "fulltext") { "supplement" } else { "fulltext" };
            let file = lib.store.import_file(&target, path, kind)?;
            if lib.store.needs_structure(&file.id)? { lib.store.save_fulltext_pages(&file.id, &pages)? }
            // A supplement without text takes nothing from the questions: the paper's own text is there.
            if kind == "fulltext" && let Err(reason) = &pages { row.textless = reason.clone() }
            row.paper_id = Some(target);
            Ok(())
        })()
        .and_then(|()| lib.reload());
        if let Err(err) = stored { row.outcome = Imported::Failed(format!("{err:#}")) }
        let title = row.paper_id.as_deref().and_then(|id| lib.paper(id)).map(|p| p.title.clone()).unwrap_or_default();
        let said = match &row.outcome {
            Imported::Added if row.unrecognised => tf!("已添加「{stem}」。没有识别出 DOI：可以在文献详情里「按题目补全」。", "Added “{stem}”. DOI not detected; use “Look up by title” in paper details."),
            Imported::Added => tf!("已添加：{title}", "Added: {title}"),
            Imported::Attached => tf!("「{stem}」已在文献库中，PDF 已附到原条目。", "“{stem}” is already in this library. PDF attached to existing record."),
            Imported::InTrash => tf!("「{stem}」对应的文献在回收站中，未导入。", "“{stem}” is in trash and was not imported."),
            Imported::Held => tf!("「{stem}」已在文献库中。", "“{stem}” is already in this library."),
            Imported::Failed(err) => tf!("「{stem}」没有导入：{}", "“{stem}” not imported: {}", crate::i18n::reasons(err)),
        };
        let said = if row.textless.is_empty() { said } else { tf!("{said} 但没有读出文字（{}），全文暂时不能用于提问。", "{said} But no text was extracted ({}); full text is unavailable for chat.", t(fulltext::failure_brief(&row.textless))) };
        let failed = matches!(row.outcome, Imported::Failed(_));
        let Some(run) = lib.import.as_mut() else { return };
        if let Some(at) = run.working.iter().position(|name| name == &row.name) { run.working.remove(at); }
        run.rows.push(row);
        let (single, finished) = (run.total == 1, !run.running());
        if finished {
            run.working.clear();
            // The paper the last file became is opened when nothing else is.
            if let Some(id) = run.rows.iter().rev().find_map(|row| row.paper_id.clone()) && lib.selected.is_none() {
                lib.selected = Some(id);
                let notes = lib.selected_paper().map(|p| p.notes.clone()).unwrap_or_default();
                lib.notes.update(cx, |input, cx| input.set_text(&notes, cx));
            }
        }
        // One file is one sentence; several are a report that stays until it is read.
        if single {
            lib.import = None;
            if failed { self.fail(said, cx) } else { self.say(said, cx) }
        }
        cx.notify();
    }

    /// The identifiers pasted into 通过 DOI / PMID 添加, more than one, looked up in the background as an import of
    /// their own (a PDF import going on takes them in): the dialog closes at once, the progress and the report are
    /// the import's. Words that are no identifier are left out and said in the report.
    pub(crate) fn import_ids(&mut self, ids: Vec<Identifier>, unknown: Vec<String>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let run = match lib.import.as_mut().filter(|run| run.running()) {
            Some(run) => run,
            None => lib.import.insert(ImportRun::default()),
        };
        run.total += ids.len();
        run.ids += ids.len();
        run.unknown.extend(unknown);
        let library = lib.info.id.clone();
        cx.notify();
        let queue = std::rc::Rc::new(std::cell::RefCell::new(ids.into_iter().rev().collect::<Vec<_>>()));
        for _ in 0..IMPORT_WORKERS.min(queue.borrow().len()) {
            let (queue, library) = (queue.clone(), library.clone());
            cx.spawn(async move |this, cx| {
                loop {
                    let Some(id) = queue.borrow_mut().pop() else { break };
                    let (Identifier::Doi(name) | Identifier::Pmid(name)) = id.clone();
                    let known = this.update(cx, |this, cx| {
                        let Some(run) = this.lib.as_mut().filter(|l| l.info.id == library).and_then(|l| l.import.as_mut()) else { return false };
                        run.working.push(name.clone());
                        cx.notify();
                        true
                    });
                    if !matches!(known, Ok(true)) { break }
                    let found = cx.background_executor().spawn(async move { wisp_net::lookup_paper(&id) }).await;
                    let done = this.update(cx, |this, cx| {
                        let Some(lib) = this.lib.as_mut().filter(|l| l.info.id == library) else { return false };
                        let stored = found.and_then(|paper| lib.store.import_papers(vec![paper]).and_then(|result| lib.reload().map(|()| result)).map_err(|e| format!("{e:#}")));
                        let mut row = ImportRow { name: name.clone(), outcome: Imported::Added, paper_id: None, unrecognised: false, textless: String::new(), no_pdf: String::new() };
                        match stored.map(|result| (result.added.into_iter().next(), result.skipped.into_iter().next())) {
                            Ok((Some(paper), _)) => row.paper_id = Some(paper.id),
                            Ok((None, Some(held))) => {
                                row.outcome = if held.in_trash { Imported::InTrash } else { Imported::Held };
                                row.paper_id = Some(held.existing_id);
                            }
                            Ok((None, None)) => row.outcome = Imported::Failed(t("没有查到这篇文献。").into()),
                            Err(err) => row.outcome = Imported::Failed(err),
                        }
                        let Some(run) = lib.import.as_mut() else { return false };
                        if let Some(at) = run.working.iter().position(|working| working == &name) { run.working.remove(at); }
                        run.rows.push(row);
                        if !run.running() { run.working.clear() }
                        cx.notify();
                        true
                    });
                    if !matches!(done, Ok(true)) { break }
                }
            })
            .detach();
        }
    }

    /// Web addresses pasted into 通过 DOI / PMID / 网址添加, taken in one after another in the background as part
    /// of the import's run (`import_url`, on a connection of its own): each page fetched once, its paper added
    /// and its PDF kept when it can be had. A row for each joins the import's report.
    pub(crate) fn import_urls(&mut self, urls: Vec<String>, cx: &mut Context<Self>) {
        if urls.is_empty() { return }
        let Some(lib) = self.lib.as_mut() else { return };
        let run = match lib.import.as_mut().filter(|run| run.running()) {
            Some(run) => run,
            None => lib.import.insert(ImportRun::default()),
        };
        run.total += urls.len();
        run.ids += urls.len();
        run.urls += urls.len();
        cx.notify();
        self.import_next_url(std::rc::Rc::new(std::cell::RefCell::new(urls.into_iter().rev().collect())), cx);
    }

    /// The next address of `import_urls`; the run stops when the library is closed or its report put away.
    fn import_next_url(&mut self, queue: std::rc::Rc<std::cell::RefCell<Vec<String>>>, cx: &mut Context<Self>) {
        let Some(run) = self.lib.as_mut().and_then(|lib| lib.import.as_mut()) else { return };
        let Some(url) = queue.borrow_mut().pop() else { return };
        run.working.push(url.clone());
        self.library_tool("import_url", serde_json::json!({"url": url}), cx, move |this, result, cx| {
            let Some(lib) = this.lib.as_mut() else { return };
            let row = url_row(&url, result);
            if row.paper_id.is_some() {
                // A paper or a PDF came in: the list, the detail panel and the next question see it.
                let _ = lib.reload();
                lib.texts_changed();
                if row.paper_id == lib.selected { lib.close_pdf() }
            }
            let Some(run) = lib.import.as_mut() else { return };
            if let Some(at) = run.working.iter().position(|working| working == &url) { run.working.remove(at); }
            run.rows.push(row);
            if !run.running() { run.working.clear() }
            this.ensure_pdf(cx);
            this.import_next_url(queue, cx);
        });
    }
}
