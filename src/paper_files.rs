//! What is done to a paper's files: an attachment picked, one taken off or put in another's place, its note,
//! a PDF made smaller, saved with its marks written in, read again, and the full texts that gave no text read
//! anew (`extract_missing`).

use crate::app::Workspace;
use crate::i18n::t;
use gpui::*;
use std::path::PathBuf;
use wisp_core::model::*;
use wisp_core::fulltext;

/// Full texts read again at the same time (`extract_missing`), at most one per core.
const READ_WORKERS: usize = 4;

impl Workspace {
    /// Files picked for a paper: as `kind` (`fulltext` or `supplement`), or, without one, the first PDF its main text
    /// when it has none yet and the rest attachments.
    pub(crate) fn pick_attachment(&mut self, paper_id: String, kind: Option<&'static str>, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some(t("添加").into()), extensions: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                this.update(cx, |this, cx| {
                    let mut added = 0;
                    this.change(cx, |lib| {
                        let mut has_fulltext = lib.files.iter().any(|f| f.paper_id == paper_id && f.kind == "fulltext");
                        for path in &paths {
                            let is_pdf = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
                            // The first PDF of a paper is its full text; the rest are supplements.
                            let kind = kind.unwrap_or(if is_pdf && !has_fulltext { "fulltext" } else { "supplement" });
                            has_fulltext |= is_pdf;
                            lib.store.import_file(&paper_id, path, kind)?;
                            added += 1;
                        }
                        Ok(())
                    });
                    if added > 0 {
                        this.reopen_viewers(cx);
                        this.extract_missing(cx);
                        this.say(tf!("已添加 {added} 个附件。", "Added {added} attachments."), cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// 移除附件: the file leaves its paper, to `removed/`; another PDF of the paper, if it has one, takes the viewer.
    pub(crate) fn remove_file(&mut self, id: String, cx: &mut Context<Self>) {
        if self.change(cx, |lib| lib.store.remove_file(&id)) {
            self.reopen_viewers(cx);
            self.say(t("附件已移除（文件移到了 removed 文件夹）。"), cx);
        }
    }

    /// 换成另一个文件… (#359): a file picked to take this one's place (`swap_file`).
    pub(crate) fn pick_swap(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(library) = self.lib.as_ref().map(|lib| lib.info.id.clone()) else { return };
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some(t("替换").into()), extensions: None });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            this.update(cx, |this, cx| if this.lib.as_ref().is_some_and(|lib| lib.info.id == library) { this.swap_file(id, &path, cx) }).ok();
        })
        .detach();
    }

    /// The file at `path` takes the place of the file `id` (`Store::swap_file`), as the main text or as the same
    /// attachment number, and is read anew; the one it replaces goes to `removed/`.
    pub(crate) fn swap_file(&mut self, id: String, path: &std::path::Path, cx: &mut Context<Self>) {
        let mut swapped = None;
        if !self.change(cx, |lib| lib.store.swap_file(&id, path).map(|file| swapped = Some(file))) { return }
        let Some(file) = swapped else { return };
        // The file chosen to read in its paper is the one in its place now.
        if let Some(lib) = self.lib.as_mut() && lib.read_file.get(&file.paper_id) == Some(&id) { lib.read_file.insert(file.paper_id.clone(), file.id.clone()); }
        self.reopen_viewers(cx);
        self.extract_missing(cx);
        if file.id == id {
            self.say(tf!("「{}」已放回原处。", "“{}” is back in place.", file.title), cx);
        } else {
            self.say(tf!("已换成「{}」。原来的文件移到了 removed 文件夹。", "Replaced with “{}”. The earlier file moved to the removed folder.", file.title), cx);
        }
    }

    /// 添加备注… / 编辑备注… (#359): what the person notes about a file, shown under its name in the detail panel.
    pub(crate) fn file_note_dialog(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = self.lib.as_ref().and_then(|lib| lib.files.iter().find(|f| f.id == id)).map(|f| f.note.clone()) else { return };
        self.modal_subscriptions.clear();
        self.modal_error.clear();
        let text = self.modal_input("例如：作者寄来的修订版；补充表 S3 是引物序列", &note, true, window, cx);
        text.read(cx).focus(window);
        self.modal = Some(crate::app::Modal::FileNote { file: id, text });
        cx.notify();
    }

    /// Make a stored PDF smaller (`shrink`), out of the window's way, and keep the result when it is worth it.
    pub(crate) fn compress_file(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(file) = lib.files.iter().find(|f| f.id == id) else { return };
        if file.format != "pdf" || !lib.shrinking.insert(id.clone()) { return }
        let (library, path) = (lib.info.id.clone(), lib.store.file_path(file));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let shrunk = cx.background_executor().spawn(async move { std::fs::read(&path).map_err(|e| anyhow::anyhow!(tf!("无法读取文件：{e}", "Cannot read file: {e}"))).and_then(|data| wisp_core::shrink::shrink(&data)) }).await;
            this.update(cx, |this, cx| {
                let Some(lib) = this.lib.as_mut().filter(|l| l.info.id == library) else { return };
                lib.shrinking.remove(&id);
                let (before, after) = match &shrunk { Ok(s) => (file_size(s.before as u64), file_size(s.bytes.len() as u64)), Err(_) => Default::default() };
                match shrunk {
                    Ok(shrunk) if shrunk.worth() => {
                        let kept = lib.store.replace_file(&id, &shrunk.bytes).and_then(|_| lib.reload());
                        match kept {
                            Ok(()) => {
                                // The viewer reads the file anew, at the place it was left.
                                let showing = lib.pdf.as_ref().is_some_and(|view| view.read(cx).resource_id() == id);
                                if showing {
                                    lib.keep_places(cx);
                                    lib.close_pdf();
                                    this.ensure_pdf(cx);
                                }
                                this.say(tf!("已压缩：{before} → {after}。原文件移到了 removed 文件夹。", "Compressed: {before} → {after}. The original moved to the removed folder."), cx);
                            }
                            Err(err) => this.fail(format!("{err:#}"), cx),
                        }
                    }
                    Ok(_) => {
                        lib.no_smaller.insert(id);
                        this.say(tf!("压缩后是 {after}（原来 {before}），小不到一成，文件保持原样。", "Compressed it would be {after} (now {before}): less than a tenth smaller, so the file stays as it is."), cx);
                    }
                    Err(err) => {
                        lib.no_smaller.insert(id);
                        this.fail(tf!("没有压缩：{err:#}", "Not compressed: {err:#}"), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A copy of a stored PDF with all its marks written in as PDF annotations, which other readers show (#273):
    /// saved where the person picks, or at `to` (a scripted run). The file in the library is not touched.
    /// A file without marks to write is said so, and nothing is saved: a plain copy is what 在文件夹中显示 gives.
    pub(crate) fn export_marked_pdf(&mut self, id: String, to: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let Some(file) = lib.files.iter().find(|f| f.id == id && f.format == "pdf") else { return };
        let marks = match lib.store.pdf_annotations(&file.paper_id, &file.id) {
            Ok(marks) => marks.into_iter().filter(|mark| wisp_core::annotpdf::exported(mark.kind)).collect::<Vec<_>>(),
            Err(err) => return self.fail(format!("{err:#}"), cx),
        };
        if marks.is_empty() { return self.say(t("这个文件还没有标注（截图标记不算），没有可写进 PDF 的。"), cx) }
        let source = lib.store.file_path(file);
        let title = std::path::Path::new(&file.title).file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_default();
        let stem = wisp_core::archive::file_stem(&title, "paper");
        let name = tf!("{stem}（带标注）.pdf", "{stem} (annotated).pdf");
        let directory = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(|| self.root.clone());
        let picked = to.is_none().then(|| cx.prompt_for_new_path(&directory, Some(&name)));
        cx.spawn(async move |this, cx| {
            let mut path = match (picked, to) {
                (Some(picked), _) => match picked.await { Ok(Ok(Some(path))) => path, _ => return },
                (None, Some(to)) => to,
                (None, None) => return,
            };
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
                let mut named = path.into_os_string();
                named.push(".pdf");
                path = PathBuf::from(named);
            }
            let target = path.clone();
            let result = cx.background_executor().spawn(async move { wisp_core::annotpdf::write(&source, &marks, &target) }).await;
            this.update(cx, |this, cx| match result {
                Ok(marked) if marked.left > 0 => this.say(tf!("已导出带 {} 处标注的 PDF 到 {}（{} 处所在的页这个文件没有，没有写进去）", "Exported the PDF with {} marks to {} ({} on pages the file lacks were left out)", marked.written, path.display(), marked.left), cx),
                Ok(marked) => this.say(tf!("已导出带 {} 处标注的 PDF 到 {}", "Exported the PDF with {} marks to {}", marked.written, path.display()), cx),
                Err(err) => this.fail(tf!("没有导出：{err:#}", "Not exported: {err:#}"), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Try once more to get the text of a stored file: after the reader has
    /// improved, or the file was put right outside the app.
    pub(crate) fn reread_file(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(file) = lib.files.iter().find(|f| f.id == id) else { return };
        if !lib.extracting.insert(id.clone()) { return }
        let (library, path) = (lib.info.id.clone(), lib.store.file_path(file));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let pages = cx.background_executor().spawn(async move { fulltext::file_pages(&path) }).await;
            this.update(cx, |this, cx| {
                let Some(lib) = this.lib.as_mut().filter(|l| l.info.id == library) else { return };
                lib.extracting.remove(&id);
                let said = match &pages {
                    Ok(pages) => Ok(tf!("读出了 {} 页文字，现在可以用于提问。", "Read {} pages of text, now available for chat.", pages.len())),
                    Err(reason) => Err(tf!("还是没有读出文字：{}", "Still no readable text: {}", crate::i18n::text_failure(reason))),
                };
                let kept = lib.store.save_fulltext_pages(&id, &pages).and_then(|()| lib.reload());
                lib.texts_changed();
                match (kept, said) {
                    (Err(err), _) => this.fail(format!("{err:#}"), cx),
                    (Ok(()), Ok(said)) => this.say(said, cx),
                    (Ok(()), Err(said)) => this.fail(said, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Read the text of the stored full texts that have not been read and
    /// sorted into parts yet, a few side by side: new files, and those an
    /// earlier build read without telling headings from paragraphs.
    pub fn extract_missing(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let _ = lib.store.restructure_outdated();
        let mut todo = vec![];
        for file in &lib.files {
            if lib.extracting.contains(&file.id) { continue }
            // A PDF attachment is read once, for the paper's conversation; a main text also whenever it is to be sorted again.
            let unread = file.kind != "fulltext" && file.format == "pdf" && matches!(lib.store.fulltext_pages(&file.id), Ok(None));
            if (file.kind == "fulltext" && matches!(lib.store.needs_structure(&file.id), Ok(true))) || unread {
                todo.push((file.id.clone(), lib.store.file_path(file), lib.store.fulltext_pages(&file.id).ok().flatten()));
            }
        }
        if todo.is_empty() { return }
        lib.extracting.extend(todo.iter().map(|(id, ..)| id.clone()));
        let library = lib.info.id.clone();
        todo.reverse();
        let queue = std::rc::Rc::new(std::cell::RefCell::new(todo));
        // Reading is the processor's work: one file per core, up to a few.
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, READ_WORKERS).min(queue.borrow().len());
        for _ in 0..workers {
            let (queue, library) = (queue.clone(), library.clone());
            cx.spawn(async move |this, cx| {
                loop {
                    let Some((id, path, known)) = queue.borrow_mut().pop() else { break };
                    let pages = cx.background_executor().spawn(async move { fulltext::reread(&path, known) }).await;
                    let same = this.update(cx, |this, cx| {
                        let Some(lib) = this.lib.as_mut().filter(|l| l.info.id == library) else { return false };
                        lib.extracting.remove(&id);
                        let _ = lib.store.save_fulltext_pages(&id, &pages);
                        // Whether a text came of it shows in the list.
                        let _ = lib.reload();
                        // The candidates on show were found without this paper.
                        if lib.extracting.is_empty() { this.write_texts_changed(cx) }
                        cx.notify();
                        true
                    });
                    if !matches!(same, Ok(true)) { break }
                }
            })
            .detach();
        }
    }
}
