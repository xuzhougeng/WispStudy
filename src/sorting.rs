//! The model at work on the full texts of a library, one paper after another (`Sorting`): sorting a text
//! into its parts, or reading the pages of a file that gave no text.

use crate::app::Workspace;
use crate::screen_run::SCREEN_GIVE_UP;
use crate::i18n::t;
use gpui::*;
use wisp_core::ai;
use wisp_core::settings::Task;
use wisp_core::store::StructureInfo;
use wisp_core::structure;

/// 写作's groundwork under way: a model sorting full texts into the parts
/// of a paper, or reading the pages of files that gave no text.
#[derive(Default)]
pub struct Sorting {
    /// The run reads pages; otherwise it sorts parts.
    pub reading: bool,
    /// The files still to do, last first.
    pub(crate) todo: Vec<String>,
    /// The files being worked on.
    pub working: Vec<String>,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    streak: usize,
    pub model: String,
    pub error: String,
    /// The files whose pages were read only in part, longer than `READ_PAGES`: (title, pages read, pages in all).
    pub cut: Vec<(String, usize, usize)>,
    /// The files with pages an engine stopped in the middle of: (title, those pages, 1-based).
    pub short: Vec<(String, Vec<usize>)>,
    /// What reads the pages in a run that reads (`ocr`): chosen when the run starts, the same for all its files.
    engine: Option<wisp_core::ocr::Ready>,
    /// Bumped when a run starts or stops, so what still arrives for an earlier one is dropped.
    pub(crate) run: u64,
}

impl Sorting {
    pub fn running(&self) -> bool {
        !self.working.is_empty() || !self.todo.is_empty()
    }
}

/// The pages of one file a model is asked to read in one run: a paper is
/// rarely longer. What is left is read by the next run, from where this one stopped.
pub const READ_PAGES: usize = 40;

impl Workspace {
    /// Start the model on the full texts: sorting those the rules sorted
    /// into parts, or reading the pages of those that gave no text.
    pub(crate) fn write_run_start(&mut self, reading: bool, only: Option<String>, cx: &mut Context<Self>) {
        let profile = self.settings.profile_for_or_default(Task::Structure);
        let workers = self.settings.screen_workers;
        // Pages are read by the engine of 文字识别 (设置 › 网络), which may be the model that sorts.
        let engine = if reading { Some(wisp_core::ocr::ready(&self.settings, "")) } else { None };
        let by_model = engine.as_ref().is_none_or(|engine| engine.as_ref().is_ok_and(|ready| ready.engine.key == wisp_core::ocr::MODEL));
        let author = match &engine {
            Some(Ok(ready)) => ready.author(&self.settings),
            _ => profile.labelled(),
        };
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.writing.sorting.running() { return }
        lib.writing.sorting.error.clear();
        let unready = match &engine {
            Some(Err(err)) => Some(err.clone()),
            _ if by_model => ai::plain_request(&profile, "s", "p").err(),
            _ => None,
        };
        if let Some(err) = unready {
            // Said where the run was asked for: of a run that reads, under the file.
            let sorting = &mut lib.writing.sorting;
            (sorting.error, sorting.reading, sorting.total, sorting.done, sorting.failed) = (err, reading, 0, 0, 0);
            return cx.notify();
        }
        let engine = engine.and_then(Result::ok);
        // Pages to look at: only a PDF has them. Parts to sort: the texts the rules had to guess at.
        // A file asked for by name whose pages were read and some came back empty: those pages again (never the
        // whole library's, which would send the pages of figures again with every run).
        let blank = |id: &str, info: &StructureInfo| only.is_some() && !wisp_core::ocr::blank_pages(Some(info), &lib.store.fulltext_pages(id).ok().flatten().and_then(Result::ok).unwrap_or_default()).is_empty();
        let mut todo: Vec<String> = lib.writing.structures.iter().filter(|(id, _)| only.as_ref().is_none_or(|only| only == *id)).filter(|(id, info)| if reading { (info.method == "none" || info.read_in_part() || blank(id, info)) && lib.files.iter().any(|f| f.id == **id && f.format == "pdf") } else { info.worth_asking() }).map(|(id, _)| id.clone()).collect();
        if todo.is_empty() { return }
        todo.reverse();
        let sorting = &mut lib.writing.sorting;
        sorting.run += 1;
        (sorting.reading, sorting.total, sorting.done, sorting.failed, sorting.streak, sorting.todo) = (reading, todo.len(), 0, 0, 0, todo);
        sorting.cut.clear();
        sorting.short.clear();
        sorting.model = author;
        sorting.engine = engine;
        let run = sorting.run;
        // A model reads a file's pages one after another; two files at a time is as much as a service takes kindly.
        for _ in 0..if reading && by_model { workers.min(2) } else { workers } {
            self.write_run_next(run, cx);
        }
        cx.notify();
    }

    /// Work on the next file of the run, and on the one after it once this is done.
    fn write_run_next(&mut self, run: u64, cx: &mut Context<Self>) {
        enum Made {
            Sorted(Vec<structure::Sentence>),
            /// All the pages read so far, and how many the file has.
            Read(Vec<String>, usize),
            /// The whole file as an engine that takes files read it.
            Recognised(Vec<wisp_core::ocr::Page>),
        }
        let profile = self.settings.profile_for_or_default(Task::Structure);
        let Some(lib) = self.lib.as_mut().filter(|l| l.writing.sorting.run == run) else { return };
        let Some(file_id) = lib.writing.sorting.todo.pop() else { return };
        let Some(file) = lib.files.iter().find(|f| f.id == file_id).cloned() else { return self.write_run_next(run, cx) };
        let (title, summary) = lib.paper(&file.paper_id).map(|p| (p.title.clone(), p.abstract_text.clone())).unwrap_or_default();
        // An engine that takes the file itself; a model looks at pictures of the pages instead.
        let engine = lib.writing.sorting.engine.clone().filter(|ready| lib.writing.sorting.reading && ready.engine.key != wisp_core::ocr::MODEL);
        let author = engine.as_ref().map_or_else(|| profile.labelled(), |ready| ready.engine.name.to_string());
        let engine_key = engine.as_ref().map_or("", |ready| ready.engine.key);
        let (library, protocol, reading) = (lib.info.id.clone(), profile.protocol, lib.writing.sorting.reading);
        let path = lib.store.file_path(&file);
        let file_name = file.title.clone();
        let pages = lib.store.fulltext_pages(&file.id).ok().flatten().and_then(Result::ok).unwrap_or_default();
        // A file read in part goes on from the first page not read.
        let before = if reading && lib.writing.structures.get(&file.id).is_some_and(StructureInfo::read_in_part) { pages.clone() } else { vec![] };
        // A file read whole with pages that came back empty: only those are read again, and put back in their places.
        let blank = if reading { wisp_core::ocr::blank_pages(lib.writing.structures.get(&file.id), &pages) } else { vec![] };
        let raw = if blank.is_empty() { vec![] } else { lib.store.ocr_pages(&file.id).ok().flatten().map(|(_, raw)| raw).unwrap_or_default() };
        lib.writing.sorting.working.push(file_id.clone());
        let name = title.clone();
        let work = cx.background_executor().spawn(async move {
            // Pages read again that still came back empty: said, and nothing kept changes.
            let still_blank = |whole: &[wisp_core::ocr::Page], at: &[usize]| at.iter().all(|at| whole.get(*at).is_none_or(|page| page.text.trim().is_empty()));
            let blank_left = |n: usize| tf!("没有文字的 {n} 页又读了一次，仍然没有文字，多半是只有图的页面。", "The {n} pages without text were read again and still have none; they are most likely figures only.");
            if let Some(ready) = engine {
                let bytes = std::fs::read(&path).map_err(|err| tf!("无法读取文件：{err}", "Cannot read file: {err}"))?;
                if !blank.is_empty() {
                    // Only those pages go (a file the reader here cannot cut goes whole, and they are taken from it).
                    let sent = wisp_core::ocr::only_pages(&bytes, &blank).unwrap_or(bytes);
                    let read = wisp_net::send_file(&wisp_core::ocr::request(&ready, &file_name, &sent)?, ready.engine)?;
                    let whole = wisp_core::ocr::refill(&pages, &raw, &blank, read)?;
                    if still_blank(&whole, &blank) { return Err(blank_left(blank.len())) }
                    return Ok(Made::Recognised(whole));
                }
                let pages = wisp_net::send_file(&wisp_core::ocr::request(&ready, &file_name, &bytes)?, ready.engine)?;
                if pages.iter().all(|page| page.text.trim().is_empty()) { return Err(t("页面上没有识别出文字。").to_string()) }
                Ok(Made::Recognised(pages))
            } else if reading && !blank.is_empty() {
                // A model looks at pictures of those pages, as many as one run reads.
                let at: Vec<usize> = blank.iter().copied().take(READ_PAGES).collect();
                let mut read = vec![];
                for picture in crate::pdf::page_pictures_at(&path, &at)? {
                    let request = ai::read_page_request(&profile, ai::ImageData { media_type: "image/png".into(), data: ai::base64(&picture) })?;
                    read.push(wisp_core::ocr::Page { text: ai::page_reading(&wisp_net::complete(request, protocol)?), ..Default::default() });
                }
                let whole = wisp_core::ocr::refill(&pages, &[], &at, read)?;
                if still_blank(&whole, &at) { return Err(blank_left(at.len())) }
                let count = whole.len();
                Ok(Made::Read(whole.into_iter().map(|page| page.text).collect(), count))
            } else if reading {
                let mut pages = before;
                let (pictures, count) = crate::pdf::page_pictures(&path, pages.len(), READ_PAGES)?;
                for picture in pictures {
                    let request = ai::read_page_request(&profile, ai::ImageData { media_type: "image/png".into(), data: ai::base64(&picture) })?;
                    pages.push(ai::page_reading(&wisp_net::complete(request, protocol)?));
                }
                if pages.iter().all(|page| page.trim().is_empty()) { return Err(t("模型在页面上没有读到文字。").to_string()) }
                Ok(Made::Read(pages, count))
            } else {
                let blocks = structure::blocks(&pages);
                let request = ai::plain_request(&profile, structure::OUTLINE_SYSTEM, &structure::outline_prompt(&name, &blocks))?;
                // Told to list only body text and explain nothing, a model answers a file of legends (a supplement) with nothing: that is its answer, and `parse_outline` says what it means.
                let reply = match wisp_net::complete(request, protocol) {
                    Err(nothing) if nothing == wisp_net::NO_CONTENT => String::new(),
                    reply => reply?,
                };
                let read = structure::parse_outline(&reply, &blocks, &summary)?;
                // What stands under a heading that names its part stays there; the model decides the rest.
                let (labels, sure) = structure::assign_sure(&blocks, &summary);
                Ok(Made::Sorted(structure::sentences(&blocks, &structure::merge(&labels, &sure, &read))))
            }
        });
        cx.spawn(async move |this, cx| {
            let made = work.await;
            this.update(cx, |this, cx| {
                let Some(lib) = this.lib.as_mut().filter(|l| l.info.id == library && l.writing.sorting.run == run) else { return };
                lib.writing.sorting.working.retain(|working| *working != file_id);
                lib.writing.sorting.done += 1;
                let saved = made.and_then(|made| {
                    match made {
                        Made::Sorted(sentences) => lib.store.save_model_structure(&file_id, &author, &sentences),
                        Made::Read(pages, count) => {
                            // A file longer than one run reads says how much of it was read.
                            if pages.len() < count { lib.writing.sorting.cut.push((title.clone(), pages.len(), count)) }
                            lib.store.save_read_pages(&file_id, pages, &author, count)
                        }
                        Made::Recognised(pages) => {
                            let short: Vec<usize> = pages.iter().enumerate().filter(|(_, page)| page.cut).map(|(index, _)| index + 1).collect();
                            if !short.is_empty() { lib.writing.sorting.short.push((title.clone(), short)) }
                            let count = pages.len();
                            let (texts, raw): (Vec<String>, Vec<String>) = pages.into_iter().map(|page| (page.text, page.raw)).unzip();
                            lib.store.save_read_pages(&file_id, texts, &author, count).and_then(|()| lib.store.save_ocr_pages(&file_id, engine_key, &raw))
                        }
                    }
                    .map_err(|e| format!("{e:#}"))
                });
                let sorting = &mut lib.writing.sorting;
                match saved {
                    Ok(()) => {
                        sorting.streak = 0;
                        // A paper whose pages were read has a text now, and says so in the list.
                        let _ = lib.reload();
                        lib.texts_changed();
                        this.write_run_next(run, cx);
                        this.write_texts_changed(cx);
                    }
                    Err(err) => {
                        sorting.failed += 1;
                        // A file without body text is an answer about the file: the service is well, and the run goes on.
                        if err != structure::NO_BODY { sorting.streak += 1 }
                        // Why, as core says it: in the interface's language where the sentence is a known one.
                        let err = crate::i18n::text_failure(&err);
                        if sorting.streak >= SCREEN_GIVE_UP {
                            sorting.error = tf!("连续 {SCREEN_GIVE_UP} 篇没有成功，已停止。最后一篇「{title}」：{err}", "Stopped after {SCREEN_GIVE_UP} consecutive failures. Last paper “{title}”: {err}");
                            sorting.todo.clear();
                        } else {
                            sorting.error = if sorting.reading { tf!("「{title}」没有读成：{err}", "Could not read “{title}”: {err}") } else { tf!("「{title}」没有整理成：{err}", "Could not structure “{title}”: {err}") };
                            this.write_run_next(run, cx);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
