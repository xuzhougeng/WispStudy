//! 写作's manuscripts in the window: which one is written and which of its
//! sections, making and removing them, the sections' names, kinds and
//! order, the whole manuscript to read, export, and each section's history.
//! The rules (what a new manuscript holds, when a snapshot is worth keeping,
//! what an export looks like) are `wisp_core::manuscript`'s.

use crate::i18n::{label, t};
use crate::app::{Cmd, MenuItem, MenuKind, Modal, Page, Workspace, Writing};
use crate::input::TextInput;
use crate::theme::{self, Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use wisp_core::manuscript::{self, LANGUAGES, Manuscript, Section};
use wisp_core::structure::SECTIONS;

/// What a manuscript is exported as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManuscriptExport {
    Markdown,
    Docx,
    /// A Word file with what changed since the file the manuscript came from written as revisions.
    DocxTracked,
    /// The whole text to the clipboard.
    Copy,
}

impl Writing {
    /// The manuscript being written.
    pub fn current(&self) -> Option<&Manuscript> {
        self.manuscripts.iter().find(|m| m.id == self.manuscript)
    }

    /// The section being written.
    pub fn section(&self) -> Option<&Section> {
        self.current().and_then(|m| m.section(&self.section_id))
    }

    /// 译文对照 over the section on show: turned on while the section was written, it is that section beside its
    /// translation, and a pill or a row of the outline moves it to another section with the translation still on.
    pub fn facing_section(&self) -> bool {
        self.reading && self.facing && !self.facing_whole
    }

    /// The whole manuscript is read: 预览, or it beside its translation. What the eye says.
    pub fn reading_whole(&self) -> bool {
        self.reading && !self.facing_section()
    }

    /// 译文对照 turned on over what is on show: the section being written, or the whole manuscript read through. The
    /// panel beside it stays as it was (`facing_panel`).
    pub fn face(&mut self) {
        if !(self.reading && self.facing) { (self.facing_whole, self.facing_panel) = (self.reading, !self.reading) }
        (self.reading, self.facing) = (true, true);
    }

    /// The panel beside the manuscript is on show: while a section is written, and under 译稿 when it was on show as
    /// 译稿 was turned on. 预览 has none.
    pub fn panel_shown(&self) -> bool {
        !self.reading || (self.facing && self.facing_panel)
    }

    /// The sections read beside their translation, by their places: every one, or over a section the one on show
    /// and its subsections (what is under it is part of it, as the outline counts it).
    pub fn facing_sections(&self) -> std::ops::Range<usize> {
        let Some(m) = self.current() else { return 0..0 };
        let whole = 0..m.sections.len();
        if !self.facing_section() { return whole }
        m.sections.iter().position(|s| s.id == self.section_id).map_or(whole, |at| manuscript::under(&m.sections, at))
    }

    /// Whether what is read beside its translation holds this section; "" is the title, which only the whole
    /// manuscript has.
    pub fn facing_holds(&self, section: &str) -> bool {
        if !self.facing_section() { return true }
        self.current().is_some_and(|m| m.sections[self.facing_sections()].iter().any(|s| s.id == section))
    }

    /// Which part of the library's papers the candidates come from: the
    /// section's kind, a key of `structure::SECTIONS`.
    pub fn kind(&self) -> &str {
        self.section().map_or("introduction", |section| section.kind.as_str())
    }

    /// What the manuscript is written in: a code of `manuscript::LANGUAGES`.
    pub fn language(&self) -> &str {
        self.current().map_or(manuscript::DEFAULT_LANGUAGE, |m| m.language.as_str())
    }

    pub(crate) fn section_mut(&mut self) -> Option<&mut Section> {
        let (manuscript, section) = (self.manuscript.clone(), self.section_id.clone());
        self.manuscripts.iter_mut().find(|m| m.id == manuscript)?.sections.iter_mut().find(|s| s.id == section)
    }

    /// The manuscript and section to open with: the one last written in.
    pub(crate) fn latest(manuscripts: &[Manuscript]) -> (String, String) {
        let Some(manuscript) = manuscripts.iter().max_by(|a, b| a.updated_at.cmp(&b.updated_at).then_with(|| latest_section(a).cmp(latest_section(b)))) else { return Default::default() };
        // The section last written in; of those written at the same time (a migration), the first with text.
        let key = |s: &Section| (!s.body.trim().is_empty(), s.updated_at.clone());
        let section = manuscript.sections.iter().enumerate().max_by(|(n, a), (m, b)| key(a).cmp(&key(b)).then(m.cmp(n))).map(|(_, s)| s.id.clone()).unwrap_or_default();
        (manuscript.id.clone(), section)
    }
}

/// When a version of a section was kept, in words.
pub(crate) fn snapshot_why(why: &str) -> &'static str {
    match why {
        "pause" => t("停笔时"),
        "leave" => t("离开时"),
        "restore" => t("恢复前"),
        "split" => t("拆分前"),
        "merge" => t("并入前"),
        "assistant" => t("助手改动前"),
        "insert" => t("插入前"),
        _ => "",
    }
}

fn latest_section(manuscript: &Manuscript) -> &str {
    manuscript.sections.iter().map(|s| s.updated_at.as_str()).max().unwrap_or("")
}

pub(crate) fn file_name(title: &str) -> String {
    let name: String = title.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "-").trim().chars().take(80).collect();
    if name.is_empty() { manuscript::UNTITLED.to_string() } else { name }
}

/// Preserve the conversation-local labels as an explicit lookup beside the
/// imported answer. These are not verified citations or a formatted bibliography.
fn answer_source_labels(answer: &str, sources: &[wisp_core::ai::Source], papers: &[wisp_core::model::Paper]) -> String {
    let mut labels: Vec<String> = Vec::new();
    for citation in wisp_core::ai::citations(answer) {
        if !labels.contains(&citation.label) { labels.push(citation.label) }
    }
    labels.into_iter().map(|label| {
        let matching: Vec<_> = sources.iter().filter(|source| source.label == label).collect();
        let [source] = matching.as_slice() else {
            return if matching.is_empty() { tf!("[{label}] 无法解析：对话中没有该标签，请核对。", "[{label}] cannot be resolved: this label is absent from the conversation. Please check it.") }
                else { tf!("[{label}] 标签对应多个来源，请核对。", "[{label}] matches multiple sources. Please check it.") }
        };
        let mut line = tf!("[{label}] {} (文献 ID: {})", "[{label}] {} (paper ID: {})", source.title, source.paper_id);
        if let Some(paper) = papers.iter().find(|paper| paper.id == source.paper_id) {
            if !paper.authors.trim().is_empty() { line.push_str(&format!("; {}", paper.authors.trim())) }
            if let Some(year) = paper.year { line.push_str(&format!("; {year}")) }
            if !paper.doi.trim().is_empty() { line.push_str(&format!("; DOI: {}", paper.doi.trim())) }
        }
        line
    }).collect::<Vec<_>>().join("\n\n")
}

/// Copy the pictures a Markdown file written at `path` names to the same places beside it, and say so in words.
pub(crate) fn pictures_beside(path: &std::path::Path, pictures: &[(String, std::path::PathBuf)]) -> String {
    let beside = path.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    let copied = pictures
        .iter()
        .filter(|(address, from)| {
            let to = beside.join(address.replace('/', std::path::MAIN_SEPARATOR_STR));
            to.parent().is_some_and(|folder| std::fs::create_dir_all(folder).is_ok()) && std::fs::copy(from, &to).is_ok()
        })
        .count();
    if copied == pictures.len() {
        tf!("（{copied} 张图片在它旁边的 figures 文件夹里）", " ({copied} pictures in the figures folder beside it)")
    } else {
        tf!("（{copied} 张图片在它旁边的 figures 文件夹里，{} 张没有复制过去）", " ({copied} pictures in the figures folder beside it, {} could not be copied)", pictures.len() - copied)
    }
}

/// The size the words of a manuscript read through are set in.
pub(crate) const READ_SIZE: f32 = 15.;

impl Workspace {
    /// Save a finished chat answer in its own manuscript, never replacing an
    /// existing draft. Conversation-local citation labels travel with it.
    pub(crate) fn answer_to_manuscript(&mut self, question: String, answer: String, window: &mut Window, cx: &mut Context<Self>) {
        let title: String = question.trim().strip_prefix("/review").unwrap_or(question.trim()).trim().chars().take(80).collect();
        let sources = self.lib.as_ref().map(|lib| lib.library_chat.sources().to_vec()).unwrap_or_default();
        self.draft_to_manuscript(title, vec![(t("正文").into(), answer)], sources, window, cx);
    }

    /// Add an answer, or part of one, at the end of a section of a
    /// manuscript. Its citations become the manuscript's (`[P3 p.5 "…"]`
    /// to `[@Key, p. 5]`); what the section held is kept as a snapshot. The
    /// person stays in the conversation: more may follow.
    pub(crate) fn insert_into_section(&mut self, section_id: String, text: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let known = match lib.store.cite_keys() { Ok(keys) => keys, Err(err) => return self.fail(tf!("无法读取引用记录：{err:#}", "Cannot read citation records: {err:#}"), cx) };
        let part = {
            let index = wisp_core::cite::Index::new(&lib.papers, lib.trash.iter().map(|entry| &entry.paper), &known);
            wisp_core::cite::convert_ai_citations(&text, lib.library_chat.sources(), &index)
        };
        let body = match lib.store.append_to_section(&section_id, &part.text, &part.keys) {
            Ok(body) => body,
            Err(err) => return self.fail(tf!("没有加进稿件：{err:#}", "Not added to the manuscript: {err:#}"), cx),
        };
        if let Ok(kept) = lib.store.cite_keys() { lib.citing.kept = kept }
        // The sentences the answer quoted are what its citations rest on in the manuscript.
        let manuscript = lib.writing.manuscripts.iter().find(|m| m.section(&section_id).is_some()).map(|m| m.id.clone()).unwrap_or_default();
        self.sources_quoted(&manuscript, &section_id, &part.text, &part.quoted, cx);
        let Some(lib) = self.lib.as_mut() else { return };
        let mut name = String::new();
        for section in lib.writing.manuscripts.iter_mut().flat_map(|m| m.sections.iter_mut()).filter(|s| s.id == section_id) {
            section.body = body.clone();
            name = section.name.clone();
        }
        // The section on show has the text in its editor as well.
        if lib.writing.section_id == section_id { lib.writing.editor.update(cx, |input, cx| input.set_text(&body, cx)) }
        self.say(tf!("已加到「{name}」的末尾；请核对引用与原文。", "Added at the end of “{name}”; check the citations against the sources."), cx);
    }

    /// Assemble every recorded section in its saved order into a new manuscript.
    pub(crate) fn review_to_manuscript(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(review) = self.lib.as_ref().and_then(|lib| lib.library_chat.review.clone()).filter(|review| review.outline_confirmed && !review.sections.is_empty()) else { return self.fail(t("请先确认大纲并保存正文。"), cx) };
        let sources = self.lib.as_ref().map(|lib| lib.library_chat.sources().to_vec()).unwrap_or_default();
        self.draft_to_manuscript(review.title, review.sections.into_iter().map(|s| (s.name, s.body)).collect(), sources, window, cx);
    }

    /// A new manuscript of text the assistant wrote. `sources` are the papers its labels (`P3`) name: those of the conversation it was written in.
    pub(crate) fn draft_to_manuscript(&mut self, title: String, sections: Vec<(String, String)>, sources: Vec<wisp_core::ai::Source>, window: &mut Window, cx: &mut Context<Self>) {
        if sections.is_empty() || sections.iter().all(|(_, body)| body.trim().is_empty()) { return }
        let Some(lib) = self.lib.as_mut() else { return };
        // Resolve only unambiguous labels using the same citation keys as the
        // writing page; a quoted sentence is left out, its page kept. Labels
        // that do not resolve remain literal and get a lookup below.
        // Conversion does not verify a claim or page.
        let known = match lib.store.cite_keys() { Ok(keys) => keys, Err(err) => return self.fail(tf!("无法读取引用记录：{err:#}", "Cannot read citation records: {err:#}"), cx) };
        let converted = {
            let index = wisp_core::cite::Index::new(&lib.papers, lib.trash.iter().map(|entry| &entry.paper), &known);
            sections.iter().map(|(name, body)| (name.clone(), wisp_core::cite::convert_ai_citations(body, &sources, &index))).collect::<Vec<_>>()
        };
        let keys: Vec<(String, String)> = converted.iter().flat_map(|(_, part)| part.keys.clone()).collect();
        let quoted: Vec<Vec<wisp_core::cite::Quoted>> = converted.iter().map(|(_, part)| part.quoted.clone()).collect();
        let mut sections: Vec<(String, String)> = converted.into_iter().map(|(name, part)| (name, part.text)).collect();
        let answer = sections.iter().map(|(_, body)| body.as_str()).collect::<Vec<_>>().join("\n\n");
        let labels = answer_source_labels(&answer, &sources, &lib.papers);
        if !labels.is_empty() { sections.push((t("来源标签（请核对）").into(), labels)) }
        let language = lib.writing.language().to_string();
        // Creating all sections and pinning keys are a single database transaction.
        let made = match lib.store.create_manuscript_from_sections(&title, &language, &sections, &keys) {
            Ok(made) => made,
            Err(err) => return self.fail(tf!("稿件没有保存：{err:#}", "Manuscript not saved: {err:#}"), cx),
        };
        let id = made.id.clone();
        let section = made.sections.first().map(|section| section.id.clone()).unwrap_or_default();
        lib.citing.kept = match lib.store.cite_keys() {
            Ok(kept) => kept,
            Err(err) => return self.fail(tf!("新稿件 {id} 已保存，但无法重新读取引用键：{err:#}", "New manuscript {id} saved, but citation keys could not be reloaded: {err:#}"), cx),
        };
        let brought: Vec<(String, String, Vec<wisp_core::cite::Quoted>)> = made.sections.iter().zip(quoted).map(|(section, quoted)| (section.id.clone(), section.body.clone(), quoted)).collect();
        lib.writing.manuscripts.push(made);
        lib.writing.reading = false;
        lib.writing.structures = lib.store.structures().unwrap_or_default();
        lib.enter_write();
        self.write_retire_blank(&id);
        // The sentences the assistant quoted are what the citations rest on in the new manuscript.
        for (section, body, quoted) in &brought { self.sources_quoted(&id, section, body, quoted, cx) }
        self.write_show(id, section, cx);
        self.write_texts_changed(cx);
        if let Some(lib) = self.lib.as_ref() { lib.writing.editor.read(cx).focus(window) }
        self.say(t("已保存为新稿件；请核对引用与原文。"), cx);
    }

    // ---- which manuscript and section

    /// Make sure the library has a manuscript to write in.
    pub(crate) fn write_ensure(&mut self, cx: &mut Context<Self>) {
        let default_lang = self.settings.default_write_language();
        let Some(lib) = self.lib.as_mut().filter(|l| l.writing.manuscripts.is_empty()) else { return };
        match lib.store.create_manuscript("", default_lang) {
            Ok(made) => {
                let (id, section) = (made.id.clone(), made.sections.get(1).map(|s| s.id.clone()).unwrap_or_default());
                lib.writing.manuscripts.push(made);
                self.write_show(id, section, cx);
            }
            Err(err) => self.fail(tf!("稿件没有建好：{err:#}", "Cannot create manuscript: {err:#}"), cx),
        }
    }

    /// After a manuscript was made or taken in: the empty one 写作 opened the library with gives way to it.
    pub(crate) fn write_retire_blank(&mut self, keep: &str) {
        let Some(lib) = self.lib.as_mut() else { return };
        let retired = lib.store.retire_blank_manuscripts(keep).unwrap_or_default();
        lib.writing.manuscripts.retain(|manuscript| !retired.contains(&manuscript.id));
    }

    /// Keep the section being written as a snapshot when it changed: on leaving it, or after a pause.
    pub(crate) fn write_snapshot(&mut self, why: &str) {
        let Some(lib) = self.lib.as_mut() else { return };
        let section = lib.writing.section_id.clone();
        if !section.is_empty() { let _ = lib.store.take_snapshot(&section, why); }
    }

    /// Write this section of this manuscript (the first one when it is not
    /// there), after keeping the one left.
    pub(crate) fn write_show(&mut self, manuscript_id: String, section_id: String, cx: &mut Context<Self>) {
        let leaving = self.lib.as_ref().is_some_and(|l| l.writing.manuscript != manuscript_id || l.writing.section_id != section_id);
        if leaving { self.write_snapshot("leave") }
        let Some(lib) = self.lib.as_mut() else { return };
        let writing = &mut lib.writing;
        let Some(manuscript) = writing.manuscripts.iter().find(|m| m.id == manuscript_id).or(writing.manuscripts.first()) else { return };
        let section = manuscript.section(&section_id).or(manuscript.sections.first());
        let (body, title) = (section.map(|s| s.body.clone()).unwrap_or_default(), manuscript.title.clone());
        writing.section_id = section.map(|s| s.id.clone()).unwrap_or_default();
        writing.manuscript = manuscript.id.clone();
        // A manuscript shown is on show: one closed beside a draft's conversation comes back (a menu, a new one, comments arriving).
        writing.away = false;
        writing.assist.leave();
        writing.revise.leave();
        writing.editor.update(cx, |input, cx| input.set_text(&body, cx));
        writing.title.update(cx, |input, cx| input.set_text(&title, cx));
        // A comment being written was about the section left.
        writing.comments.drafting = None;
        writing.comments.focused = None;
        writing.editor_scroll.set_offset(point(px(0.), px(0.)));
        self.comments_load();
        self.sources_load();
        self.write_openers(cx);
        // The conversation that writes is this manuscript's; what the assistant changed in this section is lit, and its versions are those to step through.
        self.agent_attend(cx);
        self.agent_marks(cx);
        self.agent_versions(cx);
        cx.notify();
    }

    /// Read the manuscripts again after a change to their shape, staying where the person was.
    pub(crate) fn write_reload(&mut self, section_id: Option<String>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        match lib.store.manuscripts() {
            Ok(list) => lib.writing.manuscripts = list,
            Err(err) => return self.fail(format!("{err:#}"), cx),
        }
        let (manuscript, section) = (lib.writing.manuscript.clone(), section_id.unwrap_or_else(|| lib.writing.section_id.clone()));
        // Already left: the snapshot of the section left was taken by the caller.
        lib.writing.section_id = section.clone();
        self.write_show(manuscript, section, cx);
    }

    /// The section a `write:` step or `Cmd::WriteSection` names: its id, or the first of a kind.
    pub(crate) fn write_section(&mut self, wanted: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let Some(manuscript) = lib.writing.current() else { return };
        let Some(section) = manuscript.section(&wanted).or_else(|| manuscript.sections.iter().find(|s| s.kind == wanted)).map(|s| s.id.clone()) else { return };
        let id = manuscript.id.clone();
        lib.writing.editor.read(cx).focus(window);
        if let Some(lib) = self.lib.as_mut() { lib.writing.reading = false }
        self.write_show(id, section, cx);
    }

    /// A section chosen in the pills or the outline: written, or while 译文对照 is on, read beside its translation
    /// in its place, from its top (the whole manuscript beside its translation narrows to it).
    fn pick_section(&mut self, wanted: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref().filter(|lib| lib.writing.reading && lib.writing.facing) else { return self.write_section(wanted, window, cx) };
        let Some(manuscript) = lib.writing.current() else { return };
        let Some(section) = manuscript.section(&wanted).or_else(|| manuscript.sections.iter().find(|s| s.kind == wanted)).map(|s| s.id.clone()) else { return };
        let id = manuscript.id.clone();
        self.read_letgo(cx);
        let Some(lib) = self.lib.as_mut() else { return };
        lib.writing.facing_whole = false;
        lib.writing.facing_reveal = None;
        lib.writing.translate.editing = None;
        lib.citing.read_scroll.set_offset(point(px(0.), px(0.)));
        self.write_show(id, section, cx);
    }

    /// A heading of a section's text chosen in the outline or the palette (its line starts at byte `at`): the section
    /// written, the caret at the end of the heading's line and the line at the top of the view. The section already
    /// being written is not shown anew, which would take its Undo. While 译文对照 is on, the section is read beside
    /// its translation, from its top.
    fn pick_heading(&mut self, section: String, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let here = self.lib.as_ref().is_some_and(|lib| lib.writing.section_id == section && !lib.writing.reading);
        if !here { self.pick_section(section.clone(), window, cx) }
        let Some(lib) = self.lib.as_mut().filter(|lib| lib.writing.section_id == section && !lib.writing.reading) else { return };
        let writing = &mut lib.writing;
        let Some(head) = manuscript::heads(writing.editor.read(cx).text()).into_iter().find(|head| head.line.start == at) else { return };
        writing.editor.update(cx, |input, cx| input.select_range(head.line.end..head.line.end, cx));
        writing.editor.read(cx).focus(window);
        writing.seek.reveal_heading(head.line.start);
        cx.notify();
    }

    /// A section merged into the one before it (`Store::merge_section`), that one now shown: where what went in
    /// begins is in view with the caret at it, and the toast says what it became, with 撤销 (design rule 40).
    fn write_merged(&mut self, merged: manuscript::Merged, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let was = merged.back.into.body.trim();
        let at = if was.is_empty() { 0 } else { was.len() + 2 }.min(merged.section.body.len());
        let writing = &mut lib.writing;
        writing.editor.update(cx, |input, cx| input.select_range(at..at, cx));
        writing.editor.read(cx).focus(window);
        if merged.headings { writing.seek.reveal_heading(at) } else { writing.seek.reveal_at(at) }
        let (into, name) = (merged.section.name.clone(), merged.back.gone.first().map(|(section, _)| section.name.clone()).unwrap_or_default());
        let more = merged.back.gone.len().saturating_sub(1);
        let said = match (merged.headings, more) {
            (true, 0) => tf!("「{name}」成了「{into}」正文里的标题", "“{name}” is a heading in the text of “{into}” now"),
            (true, _) => tf!("「{name}」和它下面的 {more} 个小节成了「{into}」正文里的标题", "“{name}” and the {more} subsections under it are headings in the text of “{into}” now"),
            (false, _) => tf!("「{name}」并入了「{into}」", "“{name}” is part of “{into}” now"),
        };
        self.say_undoable(said, Cmd::Unmerge(merged.back), cx);
    }

    /// Typing in a section: keep the text now, and a snapshot once typing pauses.
    pub(crate) fn write_typed(&mut self, text: String, cx: &mut Context<Self>) {
        // Writing on, the person keeps the merge: 撤销 would put back the text before what is typed now.
        if matches!(self.toast.as_ref().and_then(|toast| toast.action.as_ref()), Some((_, Cmd::Unmerge(_)))) { self.toast = None }
        let Some(lib) = self.lib.as_mut() else { return };
        let section = lib.writing.section_id.clone();
        if section.is_empty() { return }
        if let Err(err) = lib.store.save_section_body(&section, &text) { return self.fail(tf!("草稿没有保存：{err:#}", "Draft not saved: {err:#}"), cx) }
        if let Some(kept) = lib.writing.section_mut() { kept.body = text }
        lib.writing.typed += 1;
        let (typed, library) = (lib.writing.typed, lib.info.id.clone());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(manuscript::PAUSE_SECS)).await;
            this.update(cx, |this, _| {
                let Some(lib) = this.lib.as_mut().filter(|l| l.info.id == library && l.writing.typed == typed) else { return };
                let _ = lib.store.take_snapshot(&section, "pause");
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn write_title_typed(&mut self, title: String, cx: &mut Context<Self>) {
        // One begun on the home page is listed under its manuscript's title.
        self.writing_named(&title, cx);
        let Some(lib) = self.lib.as_mut() else { return };
        let id = lib.writing.manuscript.clone();
        if let Err(err) = lib.store.rename_manuscript(&id, &title) { return self.fail(tf!("题目没有保存：{err:#}", "Title not saved: {err:#}"), cx) }
        if let Some(m) = lib.writing.manuscripts.iter_mut().find(|m| m.id == id) { m.title = title.trim().to_string() }
        cx.notify();
    }

    // ---- commands

    pub(crate) fn run_manuscript(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            Cmd::WriteSection(section) => self.write_section(section, window, cx),
            Cmd::PickSection(section) => self.pick_section(section, window, cx),
            Cmd::PickHeading(section, at) => self.pick_heading(section, at, window, cx),
            Cmd::WriteManuscript(id) => {
                let section = self.lib.as_ref().and_then(|l| l.writing.manuscripts.iter().find(|m| m.id == id)).map(|m| Writing::latest(std::slice::from_ref(m)).1).unwrap_or_default();
                if let Some(lib) = self.lib.as_mut() { lib.writing.reading = false }
                self.write_show(id, section, cx);
            }
            Cmd::NewManuscript => {
                let configured = (!self.settings.write_language.is_empty()).then(|| self.settings.default_write_language().to_string());
                let Some(lib) = self.lib.as_mut() else { return };
                let language = configured.unwrap_or_else(|| lib.writing.language().to_string());
                match lib.store.create_manuscript("", &language) {
                    Ok(made) => {
                        let (id, section) = (made.id.clone(), made.sections.get(1).map(|s| s.id.clone()).unwrap_or_default());
                        lib.writing.manuscripts.push(made);
                        lib.writing.reading = false;
                        // A new one beside an empty one nothing was done with: the empty one goes.
                        self.write_retire_blank(&id);
                        self.write_show(id, section, cx);
                        if let Some(lib) = self.lib.as_ref() { lib.writing.title.read(cx).focus(window) }
                    }
                    Err(err) => self.fail(format!("{err:#}"), cx),
                }
            }
            Cmd::AskRemoveManuscript(id) => {
                let Some(m) = self.lib.as_ref().and_then(|l| l.writing.manuscripts.iter().find(|m| m.id == id)) else { return };
                let title = crate::i18n::manuscript_title(m).to_string();
                self.modal = Some(Modal::Confirm { title: tf!("移除稿件「{title}」？", "Remove manuscript “{title}”?"), message: t("它不再列在写作里。文字仍留在这个文献库的数据库中，不会被删除。").into(), action: t("移除").into(), cmd: Cmd::RemoveManuscript(id) });
            }
            Cmd::RemoveManuscript(id) => {
                self.modal = None;
                self.write_snapshot("leave");
                let Some(lib) = self.lib.as_mut() else { return };
                if let Err(err) = lib.store.remove_manuscript(&id) { return self.fail(format!("{err:#}"), cx) }
                lib.writing.manuscripts.retain(|m| m.id != id);
                let (manuscript, section) = Writing::latest(&lib.writing.manuscripts);
                lib.writing.manuscript = manuscript.clone();
                lib.writing.section_id = section.clone();
                if manuscript.is_empty() { self.write_ensure(cx) } else { self.write_show(manuscript, section, cx) }
            }
            Cmd::WriteLanguage(code) => {
                let Some(lib) = self.lib.as_mut() else { return };
                let id = lib.writing.manuscript.clone();
                if let Err(err) = lib.store.set_manuscript_language(&id, &code) { return self.fail(format!("{err:#}"), cx) }
                self.write_reload(None, cx);
                if self.write_facing_shown() { self.translate_entered(cx) }
            }
            Cmd::EditSection(editing) => {
                let Some(lib) = self.lib.as_ref() else { return };
                let shown = editing.as_ref().and_then(|id| lib.writing.current()?.section(id));
                let (name, kind) = shown.map_or((String::new(), "other".to_string()), |s| (s.name.clone(), s.kind.clone()));
                let input = cx.new(|cx| TextInput::new("例如 Root hairs、研究现状", cx).with_text(&name));
                input.read(cx).focus(window);
                self.modal_error.clear();
                self.modal = Some(Modal::Section { editing, under: None, name: input, kind });
            }
            Cmd::AddSubsection(parent) => {
                // A subsection is of the part its section is: a method's candidates come from the papers' methods.
                let Some(kind) = self.lib.as_ref().and_then(|lib| lib.writing.current()?.section(&parent)).map(|s| s.kind.clone()) else { return };
                let input = cx.new(|cx| TextInput::new("例如 Cell culture、根长的变化", cx));
                input.read(cx).focus(window);
                self.modal_error.clear();
                self.modal = Some(Modal::Section { editing: None, under: Some(parent), name: input, kind });
            }
            Cmd::PickSectionKind(key) => {
                if let Some(Modal::Section { kind, name, editing, under: None }) = self.modal.as_mut() {
                    // A new section's name follows its kind until the person types one.
                    let language = self.lib.as_ref().map_or("en", |l| l.writing.language()).to_string();
                    let typed = name.read(cx).text().trim().to_string();
                    if editing.is_none() && (typed.is_empty() || typed == manuscript::default_name(kind, &language)) && key != "other" {
                        let named = manuscript::default_name(&key, &language);
                        name.update(cx, |input, cx| input.set_text(&named, cx));
                    }
                    *kind = key;
                } else if let Some(Modal::Section { kind, .. }) = self.modal.as_mut() {
                    *kind = key;
                }
            }
            Cmd::MoveSection(id, by) => {
                let Some(lib) = self.lib.as_mut() else { return };
                if let Err(err) = lib.store.move_section(&id, by) { return self.fail(format!("{err:#}"), cx) }
                self.write_reload(None, cx);
            }
            Cmd::IndentSection(id, deeper) => {
                let Some(lib) = self.lib.as_mut() else { return };
                if let Err(err) = lib.store.indent_section(&id, deeper) { return self.fail(format!("{err:#}"), cx) }
                self.write_reload(None, cx);
            }
            Cmd::AskRemoveSection(id) => {
                let Some(section) = self.lib.as_ref().and_then(|l| l.writing.current()?.section(&id)).cloned() else { return };
                if self.lib.as_ref().and_then(|l| l.writing.current()).is_some_and(|m| m.sections.len() <= 1) { return self.fail(t("稿件至少要留一节。"), cx) }
                if section.body.trim().is_empty() { return self.run_manuscript(Cmd::RemoveSection(id), window, cx) }
                let words = section.body.split_whitespace().count();
                // Its subsections are not removed with it: they stay where they are, a level up.
                let subsections = self.lib.as_ref().and_then(|l| l.writing.current()).and_then(|m| m.sections.iter().position(|s| s.id == id).map(|at| manuscript::under(&m.sections, at).len() - 1)).unwrap_or(0);
                let mut message = tf!("这一节写了 {words} 词，连同它的历史版本会一起删除，不能恢复。要留着文字，先用「导出」或「复制全文」。", "This section contains {words} words. Its text and history will be deleted permanently. Export or copy the manuscript first to keep them.");
                if subsections > 0 { message.push_str(&tf!("它下面的 {subsections} 个小节不删，升一级留在原处。", " Its {subsections} subsections stay where they are, one level up.")) }
                self.modal = Some(Modal::Confirm { title: tf!("删除「{}」这一节？", "Delete section “{}”?", section.name), message, action: t("删除").into(), cmd: Cmd::RemoveSection(id) });
            }
            Cmd::RemoveSection(id) => {
                self.modal = None;
                let Some(lib) = self.lib.as_mut() else { return };
                let Some(manuscript) = lib.writing.current() else { return };
                let at = manuscript.sections.iter().position(|s| s.id == id).unwrap_or(0);
                let next = manuscript.sections.iter().filter(|s| s.id != id).nth(at.saturating_sub(1)).map(|s| s.id.clone());
                let staying = if lib.writing.section_id == id { next } else { None };
                if let Err(err) = lib.store.remove_section(&id) { return self.fail(format!("{err:#}"), cx) }
                self.write_reload(staying, cx);
            }
            Cmd::SplitSection(cut) => {
                let Some(lib) = self.lib.as_mut() else { return };
                let writing = &lib.writing;
                let (text, caret) = {
                    let editor = writing.editor.read(cx);
                    (editor.text().to_string(), editor.selection().start)
                };
                let at = wisp_core::ai::paragraph_at(&text, caret);
                let section = writing.section_id.clone();
                // What is typed is saved as it is typed; this is the text the store cuts.
                if let Err(err) = lib.store.save_section_body(&section, &text) { return self.fail(format!("{err:#}"), cx) }
                match lib.store.split_section(&section, at, cut) {
                    Ok(made) => {
                        lib.writing.search.asked = None;
                        self.write_reload(Some(made.id.clone()), cx);
                        // A section cut off without a heading is given its name by the person.
                        if cut != manuscript::Cut::Paragraph { if let Some(lib) = self.lib.as_ref() { lib.writing.editor.read(cx).focus(window) } } else { return self.run_manuscript(Cmd::EditSection(Some(made.id)), window, cx) }
                    }
                    Err(err) => self.fail(format!("{err:#}"), cx),
                }
            }
            Cmd::MergeSection(id) => {
                self.write_snapshot("leave");
                let Some(lib) = self.lib.as_mut() else { return };
                match lib.store.merge_section(&id) {
                    Ok(merged) => {
                        lib.writing.search.asked = None;
                        self.write_reload(Some(merged.section.id.clone()), cx);
                        self.write_merged(merged, window, cx);
                    }
                    Err(err) => self.fail(format!("{err:#}"), cx),
                }
            }
            Cmd::Unmerge(back) => {
                let Some(lib) = self.lib.as_mut() else { return };
                match lib.store.unmerge(&back) {
                    Ok(_) => {
                        lib.writing.search.asked = None;
                        // Back in the section that was merged, as before.
                        self.write_reload(back.gone.first().map(|(section, _)| section.id.clone()), cx);
                    }
                    Err(err) => self.fail(format!("{err:#}"), cx),
                }
            }
            Cmd::WriteReading(reading) => {
                // What stands beside the words read goes with the preview; a comment written there is kept first.
                self.read_letgo(cx);
                if let Some(lib) = self.lib.as_mut() {
                    lib.writing.reading = reading;
                    // The eye is the manuscript alone: beside its translation is the button next to it.
                    lib.writing.facing = false;
                    if !reading { lib.writing.editor.read(cx).focus(window) }
                }
                if reading { self.write_snapshot("leave") }
            }
            Cmd::LeaveWrite => {
                // Back is out of the manuscript altogether: the page changes with the sidebar, to the one 写作 was opened from.
                let from = self.lib.as_ref().map_or(Page::Papers, |lib| lib.write_from);
                window.focus(&self.focus);
                self.run(Cmd::SetPage(from), window, cx);
            }
            Cmd::ExportManuscript(kind) => self.export_manuscript(kind, cx),
            Cmd::WriteHistory => {
                self.write_snapshot("leave");
                let Some(lib) = self.lib.as_ref() else { return };
                let section = lib.writing.section_id.clone();
                match lib.store.snapshots(&section) {
                    Ok(snapshots) => {
                        self.modal_error.clear();
                        self.modal = Some(Modal::History { section, snapshots, chosen: 0 });
                    }
                    Err(err) => self.fail(format!("{err:#}"), cx),
                }
            }
            Cmd::HistoryPick(n) => {
                if let Some(Modal::History { chosen, .. }) = self.modal.as_mut() { *chosen = n }
            }
            _ => {}
        }
        cx.notify();
    }

    /// The section dialog's or the history's button.
    pub(crate) fn submit_manuscript_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.modal.as_ref() {
            Some(Modal::Section { editing, under, name, kind }) => {
                let (editing, under, name, kind) = (editing.clone(), under.clone(), name.read(cx).text().trim().to_string(), kind.clone());
                let Some(lib) = self.lib.as_mut() else { return };
                let manuscript = lib.writing.manuscript.clone();
                let result = match (&editing, &under) {
                    (Some(id), _) => lib.store.edit_section(id, &name, &kind).map(|_| id.clone()),
                    (None, Some(parent)) => lib.store.add_subsection(parent, &name, &kind).map(|s| s.id),
                    (None, None) => lib.store.add_section(&manuscript, &name, &kind).map(|s| s.id),
                };
                match result {
                    Ok(id) => {
                        self.modal = None;
                        if editing.is_none() { self.write_snapshot("leave") }
                        // A section's kind decides its candidates: they are looked for again.
                        if let Some(lib) = self.lib.as_mut() { lib.writing.search.asked = None }
                        self.write_reload(Some(id), cx);
                        if let Some(lib) = self.lib.as_ref() { lib.writing.editor.read(cx).focus(window) }
                    }
                    Err(err) => self.modal_error = format!("{err:#}"),
                }
            }
            Some(Modal::History { snapshots, chosen, .. }) => {
                let Some(snapshot) = snapshots.get(*chosen).cloned() else { self.modal = None; return cx.notify() };
                let Some(lib) = self.lib.as_mut() else { return };
                match lib.store.restore_snapshot(snapshot.id) {
                    Ok(_) => {
                        self.modal = None;
                        self.write_reload(Some(snapshot.section_id), cx);
                        self.say(t("已恢复这一版。恢复前的文字也留了一个版本，可以再换回去。"), cx);
                    }
                    Err(err) => self.modal_error = format!("{err:#}"),
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn export_manuscript(&mut self, kind: ManuscriptExport, cx: &mut Context<Self>) {
        let Some(manuscript) = self.lib.as_ref().and_then(|l| l.writing.current()).cloned() else { return };
        self.export_this(manuscript, kind, cx)
    }

    /// A manuscript written out: the one being written, or a version of it in another language (`write_translate.rs`).
    pub(crate) fn export_this(&mut self, manuscript: Manuscript, kind: ManuscriptExport, cx: &mut Context<Self>) {
        if manuscript.sections.iter().all(|s| s.body.trim().is_empty()) { return self.fail(t("这份稿件还没有写什么。"), cx) }
        // What a Word file was made of: said once it is written.
        let mut note = String::new();
        let (bytes, extension) = match kind {
            ManuscriptExport::Copy => {
                cx.write_to_clipboard(ClipboardItem::new_string(manuscript::plain_text(&manuscript)));
                return self.say(t("已复制整份稿件。"), cx);
            }
            ManuscriptExport::Markdown => (manuscript::markdown(&manuscript).into_bytes(), "md"),
            ManuscriptExport::Docx | ManuscriptExport::DocxTracked => match self.manuscript_word(&manuscript, kind == ManuscriptExport::DocxTracked) {
                Ok((bytes, said)) => {
                    note = said;
                    (bytes, "docx")
                }
                Err(error) => return self.fail(error, cx),
            },
        };
        let name = format!("{}.{extension}", file_name(crate::i18n::manuscript_title(&manuscript)));
        let directory = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(|| self.root.clone());
        let pictures = if kind == ManuscriptExport::Markdown { self.markdown_pictures(&manuscript) } else { vec![] };
        let picked = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut path))) = picked.await {
                if path.extension().is_none() { path.set_extension(extension); }
                let result = std::fs::write(&path, bytes);
                let note = if result.is_ok() && !pictures.is_empty() { pictures_beside(&path, &pictures) } else { note };
                this.update(cx, |this, cx| match result {
                    Ok(()) => this.say(tf!("已导出到 {}{note}", "Exported to {}{note}", path.display()), cx),
                    Err(err) => this.fail(tf!("无法写入 {}：{err}", "Cannot write {}: {err}", path.display()), cx),
                })
                .ok();
            }
        })
        .detach();
    }

    /// The manuscript as a Word file with its comments, and what was done in words: one that came from a Word file is that file written again.
    pub(crate) fn manuscript_word(&self, manuscript: &Manuscript, track: bool) -> Result<(Vec<u8>, String), String> {
        let Some(lib) = self.lib.as_ref() else { return Err(String::new()) };
        let mut comments = lib.store.comments(&manuscript.id).map_err(|err| format!("{err:#}"))?;
        // The assistant's comments are known by where they are from, not by its name: the file names it in the interface's language.
        for comment in comments.iter_mut().filter(|comment| comment.origin == wisp_core::comment::ASSISTANT && comment.author == wisp_core::comment::ASSISTANT_NAME) { comment.author = t(wisp_core::comment::ASSISTANT_NAME).to_string() }
        let style = wisp_core::csl::style(if manuscript.cite_style.is_empty() { &self.settings.cite_style } else { &manuscript.cite_style }, &self.data_root()).map_err(|err| tf!("没有导出，引用样式读不了：{err:#}", "Not exported, the citation style cannot be read: {err:#}"))?;
        let (done, from_file) = lib.store.manuscript_word(manuscript, &comments, track, &self.settings.author_name(), style).map_err(|err| tf!("没有导出：{err:#}", "Not exported: {err:#}"))?;
        let mut said = vec![];
        if from_file {
            if done.changed + done.added + done.removed == 0 { said.push(t("文字与原文件相同").to_string()) }
            if done.changed > 0 { said.push(tf!("改动 {} 段", "{} paragraphs changed", done.changed)) }
            if done.added > 0 { said.push(tf!("新增 {} 段", "{} added", done.added)) }
            if done.removed > 0 { said.push(tf!("删去 {} 段", "{} removed", done.removed)) }
            // A table written in the manuscript is put into the file: its text is not among the paragraphs counted.
            if done.tables > 0 { said.push(tn!("新增表格 {n} 张", "{n} added", done.tables, "table")) }
            if track && done.changed + done.added + done.removed + done.tables > 0 { said.push(t("都记为修订").to_string()) }
        } else if lib.writing.comments.source.as_ref().is_some_and(|source| source.kind == "docx") && lib.writing.current().is_some_and(|m| m.id == manuscript.id) {
            said.push(t("原来的 Word 文件不在了，按新文档导出").to_string());
        }
        if done.comments > 0 { said.push(tf!("批注 {} 条", "{} comments", done.comments)) }
        if done.pictures > 0 { said.push(tf!("图片 {} 张", "{} pictures", done.pictures)) }
        // A picture whose file is gone, or a WebP, which Word does not read: its caption stands alone.
        if !done.left_out.is_empty() { said.push(tf!("{} 张图片没有放进去（文件不在了，或是 Word 读不了的 WebP）", "{} pictures left out (file missing, or a WebP, which Word cannot read)", done.left_out.len())) }
        Ok((done.bytes, if said.is_empty() { String::new() } else { tf!("（{}）", " ({})", said.join(if crate::i18n::is_english() { ", " } else { "，" })) }))
    }

    /// The pictures a manuscript's Markdown names, with where each is kept. The file names them by their place in
    /// the library (`figures/…`): copied to the same place beside it (`pictures_beside`), it reads the same there
    /// and Pandoc finds them.
    pub(crate) fn markdown_pictures(&self, manuscript: &Manuscript) -> Vec<(String, std::path::PathBuf)> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        manuscript::pictures_used(manuscript).into_iter().map(|address| (address.clone(), lib.store.root().join(address.replace('/', std::path::MAIN_SEPARATOR_STR)))).collect()
    }

    /// Write the manuscript on show as Markdown at `path`, its pictures beside it, without asking where: a scripted run's export.
    pub(crate) fn write_markdown_to(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let Some(manuscript) = self.lib.as_ref().and_then(|l| l.writing.current()).cloned() else { return };
        let pictures = self.markdown_pictures(&manuscript);
        match std::fs::write(&path, manuscript::markdown(&manuscript)) {
            Ok(()) => self.say(tf!("已导出到 {}{}", "Exported to {}{}", path.display(), if pictures.is_empty() { String::new() } else { pictures_beside(&path, &pictures) }), cx),
            Err(err) => self.fail(tf!("无法写入 {}：{err}", "Cannot write {}: {err}", path.display()), cx),
        }
    }

    /// Write the manuscript on show as a Word file at `path`, without asking where: a scripted run's export.
    pub(crate) fn write_word_to(&mut self, path: std::path::PathBuf, track: bool, cx: &mut Context<Self>) {
        let Some(manuscript) = self.lib.as_ref().and_then(|l| l.writing.current()).cloned() else { return };
        match self.manuscript_word(&manuscript, track).and_then(|(bytes, said)| std::fs::write(&path, bytes).map(|_| said).map_err(|err| err.to_string())) {
            Ok(said) => self.say(tf!("已导出到 {}{said}", "Exported to {}{said}", path.display()), cx),
            Err(error) => self.fail(error, cx),
        }
    }

    /// The folder that holds the libraries, and the citation styles the person added (`csl::FOLDER`).
    pub(crate) fn data_root(&self) -> std::path::PathBuf {
        self.settings.data_root(&wisp_core::settings::default_root())
    }

    /// The items of the writing page's menus.
    pub(crate) fn manuscript_menu(&self, kind: &MenuKind, cx: &App) -> (Vec<MenuItem>, Vec<usize>) {
        let item = |label: String, checked: bool, cmd: Cmd| MenuItem { label: label.into(), checked, note: None, cmd };
        let Some(writing) = self.lib.as_ref().map(|l| &l.writing) else { return (vec![], vec![]) };
        let (mut items, mut breaks) = (vec![], vec![]);
        match kind {
            MenuKind::Manuscripts => {
                for m in &writing.manuscripts {
                    let words: usize = m.sections.iter().map(|s| s.body.split_whitespace().count()).sum();
                    items.push(MenuItem { label: crate::i18n::manuscript_title(m).to_string().into(), checked: m.id == writing.manuscript, note: Some(tf!("{words} 词", "{words} words").into()), cmd: Cmd::WriteManuscript(m.id.clone()) });
                }
                // Beside a draft's conversation an empty manuscript would not be shown: there a manuscript is handed in or saved from an answer.
                if !self.in_draft() { items.push(item(t("新建稿件").into(), false, Cmd::NewManuscript)) }
                items.push(item(t("导入 Word / Markdown 文件…").into(), false, Cmd::Comment(crate::comments::Do::Import)));
                // What the assistant wrote as a review can be gone on with here.
                for (conversation, title, sections) in self.reviews_written().into_iter().take(5) {
                    let short: String = title.chars().take(24).collect();
                    items.push(MenuItem { label: tf!("从助手的综述新建：{short}", "From the assistant's review: {short}").into(), checked: false, note: Some(tf!("{sections} 节", "{sections} sections").into()), cmd: Cmd::Comment(crate::comments::Do::FromReview(conversation)) });
                }
                if writing.manuscripts.len() > 1 || writing.current().is_some_and(|m| m.sections.iter().any(|s| !s.body.trim().is_empty())) {
                    items.push(item(t("移除这份稿件…").into(), false, Cmd::AskRemoveManuscript(writing.manuscript.clone())));
                }
            }
            MenuKind::Section(id) => {
                let Some(m) = writing.current() else { return (items, breaks) };
                let at = m.sections.iter().position(|s| s.id == *id).unwrap_or(0);
                if at >= m.sections.len() { return (items, breaks) }
                let level = m.sections[at].level;
                let deepest = m.sections[manuscript::under(&m.sections, at)].iter().map(|s| s.level).max().unwrap_or(level);
                items.push(item(t("名称与类型…").into(), false, Cmd::EditSection(Some(id.clone()))));
                if level < manuscript::MOST_LEVELS { items.push(item(t("添加小节…").into(), false, Cmd::AddSubsection(id.clone()))) }
                // A section moves among those beside it, with its subsections; up and down a level, the order stays.
                let (before, after) = (manuscript::sibling(&m.sections, at, false), manuscript::sibling(&m.sections, at, true));
                if before.is_some() || after.is_some() || level > 1 { breaks.push(items.len()) }
                if before.is_some() { items.push(item(t("前移").into(), false, Cmd::MoveSection(id.clone(), -1))) }
                if after.is_some() { items.push(item(t("后移").into(), false, Cmd::MoveSection(id.clone(), 1))) }
                if before.is_some() && deepest < manuscript::MOST_LEVELS { items.push(item(t("降为上一节的小节").into(), false, Cmd::IndentSection(id.clone(), true))) }
                if level > 1 { items.push(item(t("升一级").into(), false, Cmd::IndentSection(id.clone(), false))) }
                breaks.push(items.len());
                items.push(item(t("历史版本…").into(), false, Cmd::WriteHistory));
                // The cuts of a section are made where the caret is: in the section being written, while it is the text on show.
                let cuts = *id == writing.section_id && !writing.reading && !writing.editor.read(cx).text().trim().is_empty();
                if cuts || at > 0 { breaks.push(items.len()) }
                if cuts {
                    items.push(item(t("把光标所在的一段设为标题").into(), false, Cmd::SplitSection(manuscript::Cut::Heading)));
                    if level < manuscript::MOST_LEVELS { items.push(item(t("把光标所在的一段设为小节标题").into(), false, Cmd::SplitSection(manuscript::Cut::Subheading))) }
                    items.push(item(t("从光标所在的一段拆成新节").into(), false, Cmd::SplitSection(manuscript::Cut::Paragraph)));
                }
                if at > 0 { items.push(item(t("并入上一节").into(), false, Cmd::MergeSection(id.clone()))) }
                if m.sections.len() > 1 { breaks.push(items.len()) }
                if m.sections.len() > 1 { items.push(item(t("删除这一节…").into(), false, Cmd::AskRemoveSection(id.clone()))) }
            }
            MenuKind::ManuscriptExport => {
                items.push(item("Markdown（.md）".into(), false, Cmd::ExportManuscript(ManuscriptExport::Markdown)));
                // A manuscript that came from a Word file goes back into that file.
                if writing.comments.source.as_ref().is_some_and(|source| source.kind == "docx") {
                    items.push(item(t("Word（.docx）：原文件的版式，带批注").into(), false, Cmd::ExportManuscript(ManuscriptExport::Docx)));
                    items.push(item(t("Word（.docx）：改动记为修订，带批注").into(), false, Cmd::ExportManuscript(ManuscriptExport::DocxTracked)));
                } else {
                    items.push(item(if writing.comments.list.is_empty() { "Word（.docx）".into() } else { t("Word（.docx），带批注").into() }, false, Cmd::ExportManuscript(ManuscriptExport::Docx)));
                }
                items.push(item(t("复制全文").into(), false, Cmd::ExportManuscript(ManuscriptExport::Copy)));
                // The version read beside it, in its language.
                let version = self.translate_export_items();
                if !version.is_empty() {
                    breaks.push(items.len());
                    items.extend(version);
                }
                // How it cites is the manuscript's own, chosen in a dialog of its own: the menu says which style it is.
                breaks.push(items.len());
                items.push(MenuItem { label: t("引用样式…").into(), checked: false, note: Some(self.cite_style_name(&self.cite_style_key()).into()), cmd: Cmd::OpenCiteStyles });
            }
            MenuKind::WriteLanguage => {
                for (code, name) in LANGUAGES { items.push(item(format!("{} ({code})", t(name)), writing.language() == code, Cmd::WriteLanguage(code.to_string()))) }
            }
            _ => {}
        }
        (items, breaks)
    }

    // ---- render

    /// The sidebar, when it is there, is the manuscript's outline: 写作 is the page and there is a manuscript.
    /// The library's navigation is one level up, behind the corner's arrow (`Cmd::LeaveWrite`).
    pub(crate) fn outline_shown(&self) -> bool {
        self.lib.as_ref().is_some_and(|lib| lib.page == Page::Write && lib.writing.current().is_some())
    }

    /// The sidebar while writing: the corner, whose arrow leaves 写作 for the page of the
    /// library it was opened from, the manuscript, and its sections with how long each is and how many
    /// comments wait in it. A section's row opens it; the one written in has its menu.
    pub(crate) fn render_outline(&self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let writing = &lib.writing;
        let Some(manuscript) = writing.current() else { return div().into_any_element() };
        let whole: String = manuscript.sections.iter().map(|s| s.body.as_str()).collect::<Vec<_>>().join("\n");
        let waiting: usize = manuscript.sections.iter().map(|s| writing.comments.open_in(&s.id)).sum();
        let mut about = vec![crate::write::length_label(&whole)];
        if waiting > 0 { about.push(tf!("{waiting} 条批注待处理", "{waiting} open comments")) }
        let row = |id: ElementId, on: bool| div().id(id).flex().items_center().gap(px(8.)).px(px(8.)).rounded(px(6.)).cursor_pointer().when(on, |d| d.bg(Theme::accent_soft())).when(!on, |d| d.hover(|s| s.bg(Theme::hover())));
        let sections = &manuscript.sections;
        // The outline as it is read: the sections, and the headings written in their text (`##` is 3.1 in 方法).
        let entries = manuscript::contents(sections);
        // The headings of the section being written are listed; another section's fold into how many it has, so the
        // outline is as long as the manuscript has sections, however many headings it has.
        let shows = |at: usize| sections[at].id == writing.section_id;
        // The heading the caret is under is lit: where the person writes.
        let caret = writing.editor.read(cx).selection().start;
        let lit = entries.iter().rposition(|entry| shows(entry.at) && entry.head.as_ref().is_some_and(|head| head.line.start <= caret)).filter(|_| !writing.reading_whole());
        let rows: Vec<AnyElement> = entries
            .iter()
            .enumerate()
            .filter_map(|(e, entry)| {
                let (n, section) = (entry.at, &sections[entry.at]);
                if let Some(head) = entry.head.as_ref() {
                    if !shows(n) { return None }
                    // A heading's part runs to the next heading as deep or less deep, or the end of the section's text.
                    let end = entries[e + 1..].iter().take_while(|next| next.at == n).filter_map(|next| next.head.as_ref()).find(|next| next.down <= head.down).map_or(section.body.len(), |next| next.line.start);
                    let part = section.body.get(head.line.end..end).unwrap_or("");
                    let on = lit == Some(e);
                    return Some(
                        row(("outline-head", e).into(), false)
                            .py(px(4.))
                            .pl(px(8. + 14. * f32::from(entry.level.saturating_sub(1))))
                            .on_click(self.on(Cmd::PickHeading(section.id.clone(), head.line.start), cx))
                            .child(div().flex_none().min_w(px(16.)).text_size(text_px(11.)).text_color(if on { Theme::accent() } else { Theme::text_tertiary() }).child(entry.number.clone()))
                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .items_baseline()
                                    .gap(px(6.))
                                    .child(div().flex_1().min_w_0().truncate().text_size(text_px(13.)).when(on, |d| d.text_color(Theme::accent()).font_weight(FontWeight::MEDIUM)).when(!on, |d| d.text_color(Theme::text_secondary())).child(head.name.clone()))
                                    .when(!part.trim().is_empty(), |d| d.child(div().flex_none().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(crate::write::length_label(part)))),
                            )
                            .into_any_element(),
                    );
                }
                let on = section.id == writing.section_id && !writing.reading_whole();
                // A section with subsections is all that is under it: 结果 is its results, as long as they are together.
                let block = &sections[manuscript::under(sections, n)];
                let children = block.iter().filter(|s| s.level == section.level + 1).count();
                let whole = block.iter().map(|s| s.body.as_str()).collect::<Vec<_>>().join("\n");
                let empty = whole.trim().is_empty();
                let noted = writing.comments.open_in(&section.id);
                let mut said = vec![];
                // A subsection says its part only where it is not that of the section it is under.
                let over = manuscript::parent(sections, n).map(|p| sections[p].kind.as_str());
                if section.kind != "other" && label(&SECTIONS, &section.kind) != section.name && over != Some(section.kind.as_str()) { said.push(label(&SECTIONS, &section.kind).to_string()) }
                if children > 0 { said.push(tf!("{children} 个小节", "{children} subsections")) }
                // Its headings folded: how many there are, opened with the section.
                let folded = if shows(n) { 0 } else { entries[e + 1..].iter().take_while(|next| next.at == n).count() };
                if folded > 0 { said.push(tf!("{folded} 个标题", "{folded} headings")) }
                said.push(if empty { t("未写").to_string() } else { crate::write::length_label(&whole) });
                let sub = section.level > 1;
                Some(
                    row(("outline-section", n).into(), on)
                        .py(px(if sub { 5. } else { 6. }))
                        // Each level a step in: the numbers stand under the name of the section they are of.
                        .pl(px(8. + 14. * f32::from(section.level.saturating_sub(1))))
                        .on_click(self.on(Cmd::PickSection(section.id.clone()), cx))
                        .when(on, |d| d.on_mouse_down(MouseButton::Right, self.on_context(MenuKind::Section(section.id.clone()), cx)))
                        .child(div().flex_none().min_w(px(16.)).text_size(text_px(11.)).text_color(if on { Theme::accent() } else { Theme::text_tertiary() }).child(entry.number.clone()))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap(px(1.))
                                // A subsection is one line, its length at the end: the outline of a paper with many methods stays short.
                                .when(sub, |d| d.flex_row().items_baseline().gap(px(6.)))
                                .child(div().truncate().when(sub, |d| d.flex_1().min_w_0()).text_size(text_px(13.)).when(on, |d| d.text_color(Theme::accent()).font_weight(FontWeight::MEDIUM)).when(!on, |d| d.text_color(if empty { Theme::text_tertiary() } else { Theme::text() })).child(section.name.clone()))
                                .child(div().truncate().when(sub, |d| d.flex_none()).text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(said.join(" · "))),
                        )
                        // The assistant changed it in answer to the last question.
                        .when(writing.agent.changed.get(&section.id).is_some_and(|before| *before != section.body), |d| d.child(div().id(("outline-changed", n)).flex_none().size(px(6.)).rounded(px(3.)).bg(Theme::accent()).tooltip(|_, cx| cx.new(|_| crate::write_assist::Tip(t("助手刚改过这一节").to_string())).into())))
                        // How many comments wait in the section.
                        .when(noted > 0, |d| d.child(div().flex().flex_none().items_center().justify_center().min_w(px(18.)).h(text_px(18.)).px(px(5.)).rounded(px(9.)).bg(Theme::note_strong()).text_size(text_px(10.)).text_color(Theme::text()).child(noted.to_string())))
                        .when(on, |d| d.child(div().id("outline-section-menu").flex().flex_none().items_center().justify_center().size(px(20.)).rounded(px(10.)).hover(|s| s.bg(Theme::accent().opacity(0.12))).on_click(self.on_menu(MenuKind::Section(section.id.clone()), cx)).child(ui::icon("chevron-down", 12., Theme::accent()))))
                        .into_any_element(),
                )
            })
            .collect();
        let (cited, unresolved, references_on) = self.references_row();
        let small_row = |id: &'static str, icon: &'static str, name: String, on: bool| {
            row(id.into(), on).h(text_px(30.)).text_size(text_px(13.)).text_color(if on { Theme::accent() } else { Theme::text_secondary() }).child(div().flex().flex_none().w(px(16.)).justify_center().child(ui::icon(icon, 14., if on { Theme::accent() } else { Theme::text_tertiary() }))).child(div().flex_1().min_w_0().truncate().child(name))
        };
        div()
            .id("write-outline")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            .h_full()
            .px(px(10.))
            .pb(px(10.))
            .pt(px(Self::corner_top()))
            .gap(px(10.))
            .child(self.render_corner(width >= ui::traffic_lights_width() + f32::from(text_px(150.)), false, cx).when(ui::custom_titlebar(), |d| d.mb(px(4.))))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .pl(px(8.))
                    .pr(px(2.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(3.))
                            .child(div().text_size(text_px(15.)).font_weight(FontWeight::SEMIBOLD).line_height(text_px(21.)).text_color(Theme::text()).child(crate::i18n::manuscript_title(manuscript).to_string()))
                            .child(div().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(about.join(" · "))),
                    )
                    // Who writes: the person, or the assistant in a conversation beside the manuscript.
                    .child(self.render_mode_switch(cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(px(2.))
                    .child(ui::caption(t("大纲")).px(px(8.)).py(px(4.)))
                    .child(div().id("outline-sections").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().gap(px(1.)).children(rows).child(small_row("outline-add", "plus", t("添加一节").into(), false).on_click(self.on(Cmd::EditSection(None), cx)))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .pt(px(6.))
                    .border_t_1()
                    .border_color(Theme::line())
                    // What the manuscript draws on besides papers: pictures to bring in and to put into it (write_figures.rs).
                    .child({
                        let pictures = manuscript::pictures_used(manuscript).len();
                        small_row("outline-materials", "image", t("资料库").into(), matches!(self.modal, Some(Modal::WriteFigures)))
                            .on_click(self.on(Cmd::WriteFigures(crate::write_figures::Do::Open), cx))
                            .when(pictures > 0, |d| d.child(div().flex_none().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(pictures.to_string())))
                    })
                    // The manuscript's reference list, where a paper has it: after its sections.
                    .child(
                        small_row("outline-references", "library", t("参考文献").into(), references_on)
                            .on_click(self.on(Cmd::WriteReferences, cx))
                            .when(unresolved, |d| d.child(div().flex_none().size(px(6.)).rounded(px(3.)).bg(Theme::danger())))
                            .when(cited > 0, |d| d.child(div().flex_none().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(cited.to_string()))),
                    )
                    .child(small_row("outline-manuscripts", "list", tf!("稿件 {}", "Manuscripts {}", writing.manuscripts.len()), false).on_click(self.on_menu(MenuKind::Manuscripts, cx))),
            )
            .into_any_element()
    }


    /// Above the editor: the title, the manuscript's own buttons, and its sections.
    pub(crate) fn render_manuscript_bar(&self, width: f32, window: &Window, cx: &mut Context<Self>) -> Div {
        let lib = self.lib.as_ref().unwrap();
        let writing = &lib.writing;
        let Some(manuscript) = writing.current() else { return div() };
        let language = LANGUAGES.iter().find(|(code, _)| *code == manuscript.language).map_or(manuscript.language.as_str(), |(_, name)| t(name));
        // The bar is one row: in a narrow column the tools keep their icons and say their names in a tooltip.
        let roomy = width >= f32::from(text_px(560.));
        let tool = |id: &'static str, icon: &'static str, text: Option<SharedString>| {
            let tip = text.clone();
            ui::ghost_button(id, Some(icon), text.filter(|_| roomy)).h(text_px(26.)).px(px(7.)).text_size(text_px(12.))
                .when_some(tip.filter(|_| !roomy), |d, tip| d.tooltip(move |_, cx| cx.new(|_| crate::write_assist::Tip(tip.to_string())).into()))
        };
        let numbers = manuscript::numbers(&manuscript.sections);
        let pills: Vec<AnyElement> = manuscript
            .sections
            .iter()
            .enumerate()
            .map(|(n, section)| {
                let on = section.id == writing.section_id && !writing.reading_whole();
                let empty = !manuscript::written_under(&manuscript.sections, n);
                // A subsection is a smaller pill that says its number: 3.1 is the first under the third.
                let sub = section.level > 1;
                let noted = writing.comments.open_in(&section.id);
                div()
                    .id(("write-part", n))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(5.))
                    .h(text_px(28.))
                    .px(px(12.))
                    .rounded(px(14.))
                    .text_size(text_px(13.))
                    .cursor_pointer()
                    .border_1()
                    .when(on, |d| d.bg(Theme::accent_soft()).text_color(Theme::accent()).border_color(Theme::accent().opacity(0.35)).font_weight(FontWeight::MEDIUM))
                    .when(!on, |d| d.border_color(Theme::line()).text_color(if empty { Theme::text_tertiary() } else { Theme::text_secondary() }).hover(|s| s.bg(Theme::hover())))
                    .when(sub, |d| d.h(text_px(24.)).px(px(9.)).text_size(text_px(12.)))
                    .on_click(self.on(Cmd::PickSection(section.id.clone()), cx))
                    .when(sub, |d| d.child(div().text_color(Theme::text_tertiary()).child(numbers[n].clone())))
                    .child(section.name.clone())
                    // How many comments wait in the section.
                    .when(noted > 0, |d| d.child(div().flex().flex_none().items_center().justify_center().min_w(px(16.)).h(px(16.)).px(px(4.)).rounded(px(8.)).bg(Theme::note_strong()).text_size(text_px(10.)).text_color(Theme::text()).child(noted.to_string())))
                    .when(on, |d| d.child(div().id("write-part-menu").flex().items_center().justify_center().size(px(18.)).rounded(px(9.)).hover(|s| s.bg(Theme::accent().opacity(0.12))).on_click(self.on_menu(MenuKind::Section(section.id.clone()), cx)).child(ui::icon("chevron-down", 12., Theme::accent()))))
                    .into_any_element()
            })
            .collect();
        let add = div().id("write-part-add").flex().flex_none().items_center().justify_center().size(px(28.)).rounded(px(14.)).cursor_pointer().border_1().border_color(Theme::line()).hover(|s| s.bg(Theme::hover())).on_click(self.on(Cmd::EditSection(None), cx)).child(ui::icon("plus", 14., Theme::text_secondary()));
        let title_focused = writing.title.focus_handle(cx).is_focused(window);
        // Over a section beside its translation the eye is off: it is the whole manuscript, and alone.
        let reading = writing.reading_whole();
        let preview_tip = if reading { t("预览中：再点一下回到编辑") } else { t("预览：看排好版的全文，Markdown 照它写的样子画出来") };
        let preview = div()
            .id("write-preview")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(text_px(26.))
            .rounded(px(6.))
            .cursor_pointer()
            .when(reading, |d| d.bg(Theme::accent_soft()))
            .when(!reading, |d| d.hover(|s| s.bg(Theme::hover())))
            .on_click(self.on(Cmd::WriteReading(!reading), cx))
            .tooltip(move |_, cx| cx.new(|_| crate::write_assist::Tip(preview_tip.to_string())).into())
            .child(ui::icon("eye", 14., if reading { Theme::accent() } else { Theme::text_secondary() }));
        // 语言配置（主稿写作语言 → 目标译文语言）+ 一键译稿开关: paired side-by-side so the writer sees what language
        // they are drafting in, what language it translates into, and can configure both in one popover.
        let facing = writing.reading && writing.facing;
        let target_code = self.write_facing_language();
        let target_name = label(&LANGUAGES, target_code);
        let has_version = writing.translate.versions.get(&manuscript.id).is_some_and(|list| !list.is_empty());
        let lang_open = self.menu.as_ref().is_some_and(|m| m.kind == MenuKind::WriteLanguage);
        let lang_tip = tf!(
            "写作语言（主稿）：{language} → 目标译文：{target_name}\n点击同时设置主稿写作语言与目标译文语言",
            "Writing language (primary): {language} → Target translation: {target_name}\nClick to configure primary and translation languages"
        );
        let language_button = div()
            .id("write-language")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .h(text_px(26.))
            .px(px(8.))
            .rounded(px(6.))
            .cursor_pointer()
            .text_size(text_px(12.))
            .when(lang_open, |d| d.bg(Theme::accent_soft()).text_color(Theme::accent()))
            .when(!lang_open, |d| d.text_color(Theme::text()).hover(|s| s.bg(Theme::hover())))
            .on_click(self.on_menu(MenuKind::WriteLanguage, cx))
            .tooltip(move |_, cx| cx.new(|_| crate::write_assist::Tip(lang_tip.clone())).into())
            .when(roomy, |d| d.child(div().text_color(if lang_open { Theme::accent() } else { Theme::text_tertiary() }).child(t("写作："))))
            .child(format!("{language} → {target_name}"))
            .child(ui::icon("chevron-down", 11., if lang_open { Theme::accent() } else { Theme::text_tertiary() }));
        let facing_tip = match (facing, writing.facing_section() || !writing.reading) {
            (true, true) => t("译稿模式：开。再点一下收起译稿，回到这一节继续写").to_string(),
            (true, false) => t("译稿模式：开。再点一下收起译稿，回到原文预览").to_string(),
            (false, true) => tf!("打开{target_name}译稿：自动逐段翻译，支持逐段或逐句修改并导出投稿；原文改动自动同步", "Open {target_name} version: translate automatically, edit by paragraph or sentence, and export"),
            (false, false) => tf!("打开全文{target_name}译稿：自动逐段翻译，支持逐段或逐句修改并导出投稿", "Open full {target_name} version: translate automatically, edit by paragraph or sentence, and export"),
        };
        let facing_cmd = if facing { Cmd::WriteFacing(false) } else { Cmd::Translate(crate::write_translate::Do::Open) };
        let facing_button = div()
            .id("write-facing")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(5.))
            .h(text_px(26.))
            .px(px(if roomy { 8. } else { 6. }))
            .rounded(px(6.))
            .cursor_pointer()
            .text_size(text_px(12.))
            .border_1()
            .when(facing, |d| d.bg(Theme::accent_soft()).text_color(Theme::accent()).border_color(Theme::accent().opacity(0.35)).font_weight(FontWeight::MEDIUM))
            .when(!facing, |d| d.border_color(if has_version { Theme::accent().opacity(0.28) } else { Theme::line() }).text_color(if has_version { Theme::accent() } else { Theme::text_secondary() }).hover(|s| s.bg(Theme::hover())))
            .on_click(self.on(facing_cmd, cx))
            .tooltip(move |_, cx| cx.new(|_| crate::write_assist::Tip(facing_tip.clone())).into())
            .child(ui::icon("languages", 14., if facing || has_version { Theme::accent() } else { Theme::text_secondary() }))
            .when(roomy, |d| d.child(t("译稿")));
        // 引用卡片: whether a citation the pointer is on shows its card. A way of looking at the manuscript, so it stands here, in either
        // mode of 写作, and not among the tools for a paragraph. Lit while on; following a citation with a click works either way.
        let cards = !self.settings.citation_cards_hidden;
        let cards_tip = if cards { tf!("引用卡片：开。指针停在引用上时显示文献和它依据的原句；点击关掉。{}+点击引用仍会打开文献。", "Citation cards: on. Pointing at a citation shows the paper and the sentence it rests on; click to turn off. {}+click on a citation still opens the paper.", crate::write_source::follow_key()) } else { tf!("引用卡片：关。点击后指针停在引用上时显示文献和它依据的原句。{}+点击引用仍会打开文献。", "Citation cards: off. Click to show the paper and the sentence it rests on when pointing at a citation. {}+click on a citation still opens the paper.", crate::write_source::follow_key()) };
        let cards_button = div()
            .id("write-citation-cards")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(text_px(26.))
            .rounded(px(6.))
            .cursor_pointer()
            .when(cards, |d| d.bg(Theme::accent_soft()))
            .when(!cards, |d| d.hover(|s| s.bg(Theme::hover())))
            .on_click(self.on(Cmd::CitationCards(!cards), cx))
            .tooltip(move |_, cx| cx.new(|_| crate::write_assist::Tip(cards_tip.clone())).into())
            .child(ui::icon("quote", 14., if cards { Theme::accent() } else { Theme::text_secondary() }));
        // 铺满: the text as wide as the column, or back to the standard measure; the same setting as 设置 › 外观's 正文宽度. Lit while on.
        let fill = self.settings.reading_appearance.width == wisp_core::settings::TextWidth::Fill;
        let fill_tip = if fill { t("铺满：开。正文随窗口宽度铺满；点击回到标准宽度。设置 › 外观 里也可以选正文宽度。") } else { t("铺满：关。点击让正文随窗口宽度铺满，窗口多宽，一行就多长。") };
        let fill_button = div()
            .id("write-fill")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(text_px(26.))
            .rounded(px(6.))
            .cursor_pointer()
            .when(fill, |d| d.bg(Theme::accent_soft()))
            .when(!fill, |d| d.hover(|s| s.bg(Theme::hover())))
            .on_click(self.on(Cmd::SetTextWidth(if fill { wisp_core::settings::TextWidth::Standard } else { wisp_core::settings::TextWidth::Fill }), cx))
            .tooltip(move |_, cx| cx.new(|_| crate::write_assist::Tip(fill_tip.to_string())).into())
            .child(ui::icon("expand", 14., if fill { Theme::accent() } else { Theme::text_secondary() }));
        // The outline in the sidebar has the title, who writes, the sections and the manuscripts: here they are only while it is put away.
        let outlined = self.outline_shown() && !self.settings.sidebar_hidden;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(
                ui::column_head()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .px(px(6.))
                            .py(px(2.))
                            .ml(px(-6.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(if title_focused { if outlined { Theme::line() } else { Theme::accent().opacity(0.55) } } else { transparent_black() })
                            .hover(|s| s.bg(Theme::hover()))
                            // Beside the outline it is only where the title is changed: the outline shows it.
                            .when(outlined, |d| d.text_size(text_px(13.)).text_color(Theme::text_secondary()))
                            .when(!outlined, |d| d.text_size(text_px(ui::HEAD_TITLE)).font_weight(FontWeight::SEMIBOLD))
                            .child(writing.title.clone()),
                    )
                    // Beside the conversation of a draft the manuscript is one to review: there is no other mode to switch to.
                    .when(!outlined && !self.in_draft(), |d| d.child(self.render_mode_switch(cx)))
                    // Beside a draft's conversation it says its name even in a narrow column: it is where the manuscripts are switched and removed.
                    .when(!outlined && !self.in_draft(), |d| d.child(tool("write-manuscripts", "list", Some(tf!("稿件 {}", "Manuscript {}", writing.manuscripts.len()).into())).on_click(self.on_menu(MenuKind::Manuscripts, cx))))
                    .when(!outlined && self.in_draft(), |d| d.child(ui::ghost_button("write-manuscripts", Some("list"), Some(tf!("稿件 {}", "Manuscript {}", writing.manuscripts.len()).into())).h(text_px(26.)).px(px(7.)).text_size(text_px(12.)).on_click(self.on_menu(MenuKind::Manuscripts, cx))))
                    // 资料库 is a row of the outline: here while that is put away.
                    .when(!outlined, |d| d.child(tool("write-materials", "image", Some(t("资料库").into())).on_click(self.on(Cmd::WriteFigures(crate::write_figures::Do::Open), cx))))
                    .child(cards_button)
                    // 预览 shows the whole manuscript as it reads, Markdown drawn; lit while it does, a click goes back to writing.
                    .child(preview)
                    .child(fill_button)
                    // Primary + target language configuration popover, paired with the 1-click 译稿 toggle.
                    .child(language_button)
                    .child(facing_button)
                    // Where the translation stands, beside the switch that shows it, while it is on (design rule 21).
                    .when(facing, |d| d.child(self.render_facing_arrangement(cx)))
                    .children(if self.settings.agent_writing && !self.in_draft() { self.render_version_stepper(cx) } else { None })
                    .child(tool("write-history", "history", None).on_click(self.on(Cmd::WriteHistory, cx)))
                    .child(tool("write-export", "export", Some(t("导出").into())).on_click(self.on_menu(MenuKind::ManuscriptExport, cx)))
                    // Beside a draft's conversation the manuscript can be closed: the conversation has the page, 稿件 in its header brings it back.
                    .when(self.in_draft(), |d| d.child(ui::ghost_button("draft-manuscript-close", Some("x"), None).h(text_px(26.)).px(px(7.)).tooltip(ui::tip(t("关闭稿件：对话占满页面，稿件留在对话里，点对话上方的「稿件」再打开"))).on_click(self.on(Cmd::Draft(crate::draft::Do::Manuscript(false)), cx)))),
            )
            .when(!outlined, |d| d.child(div().flex().items_center().flex_wrap().gap(px(6.)).px(px(16.)).pt(px(12.)).children(pills).child(add)))
    }

    /// Unified popover opened from `write-language`: explains and configures BOTH the primary writing language
    /// and the target translation language in one place, with an option to save as default and open the version.
    pub(crate) fn render_write_language_layer(
        &self,
        from: Option<(Bounds<Pixels>, crate::menus::MenuSide)>,
        at: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let lib = self.lib.as_ref()?;
        let writing = &lib.writing;
        let manuscript = writing.current()?;
        let primary = manuscript.language.clone();
        let target = self.write_facing_language().to_string();
        let target_name = label(&LANGUAGES, &target).to_string();
        let facing = writing.reading && writing.facing;
        let versions = writing.translate.versions.get(&manuscript.id).cloned().unwrap_or_default();

        let chip = |id: ElementId, text: String, chosen: bool| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .h(text_px(26.))
                .px(px(10.))
                .rounded(px(6.))
                .border_1()
                .cursor_pointer()
                .text_size(text_px(12.))
                .when(chosen, |d| d.bg(Theme::accent_soft()).border_color(Theme::accent().opacity(0.45)).text_color(Theme::accent()).font_weight(FontWeight::MEDIUM))
                .when(!chosen, |d| d.border_color(Theme::line()).text_color(Theme::text_secondary()).hover(|s| s.bg(Theme::hover())))
                .child(text)
        };

        let primary_chips: Vec<AnyElement> = LANGUAGES
            .iter()
            .map(|&(code, name)| {
                let chosen = primary == code;
                let code_owned = code.to_string();
                chip(SharedString::from(format!("wlang-primary-{code}")).into(), t(name).to_string(), chosen)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(lib) = this.lib.as_mut() else { return };
                        let id = lib.writing.manuscript.clone();
                        if let Err(err) = lib.store.set_manuscript_language(&id, &code_owned) {
                            return this.fail(format!("{err:#}"), cx);
                        }
                        this.write_reload(None, cx);
                        if this.settings.write_facing_language == code_owned {
                            let fallback = wisp_core::bilingual::language("", &code_owned, if crate::i18n::is_english() { "en" } else { "zh" });
                            this.settings.write_facing_language = fallback.to_string();
                            this.save_settings(cx);
                        }
                        if this.write_facing_shown() {
                            let next_target = this.write_facing_language().to_string();
                            this.translate_language(next_target, cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();

        let target_chips: Vec<AnyElement> = LANGUAGES
            .iter()
            .filter(|&&(code, _)| code != primary)
            .map(|&(code, name)| {
                let chosen = target == code;
                let kept = versions.iter().any(|v| v == code);
                let text = if kept { format!("{} · {}", t(name), t("译稿")) } else { t(name).to_string() };
                let code_owned = code.to_string();
                chip(SharedString::from(format!("wlang-target-{code}")).into(), text, chosen)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.write_facing_language = code_owned.clone();
                        this.save_settings(cx);
                        if this.write_facing_shown() {
                            this.translate_language(code_owned.clone(), cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();

        let is_default = self.settings.default_write_language() == primary && self.write_facing_language() == target && !self.settings.write_language.is_empty() && !self.settings.write_facing_language.is_empty();
        let (save_primary, save_target) = (primary.clone(), target.clone());
        let default_control = if is_default {
            div().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(t("✓ 已是新建稿件默认")).into_any_element()
        } else {
            div()
                .id("wlang-save-default")
                .px(px(6.))
                .py(px(3.))
                .mx(px(-6.))
                .rounded(px(5.))
                .cursor_pointer()
                .text_size(text_px(11.5))
                .text_color(Theme::accent())
                .hover(|s| s.bg(Theme::hover()))
                .tooltip(ui::tip(t("以后新建的稿件也默认使用这套写作语言与译文语言")))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings.write_language = save_primary.clone();
                    this.settings.write_facing_language = save_target.clone();
                    this.save_settings(cx);
                    cx.notify();
                }))
                .child(t("设为新建默认"))
                .into_any_element()
        };

        let open_target = target.clone();
        let right_actions = if facing {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(ui::secondary_button("wlang-fold", t("收起译稿")).h(text_px(26.)).px(px(10.)).text_size(text_px(12.)).on_click(self.on(Cmd::WriteFacing(false), cx)))
                .child(
                    ui::primary_button("wlang-close", t("完成"))
                        .h(text_px(26.))
                        .px(px(10.))
                        .text_size(text_px(12.))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.notify();
                        })),
                )
        } else {
            div().flex().items_center().child(
                ui::primary_button("wlang-open-translate", tf!("开启{target_name}译稿", "Open {target_name} version"))
                    .h(text_px(26.))
                    .px(px(11.))
                    .text_size(text_px(12.))
                    .on_click(self.on(Cmd::Translate(crate::write_translate::Do::Language(open_target)), cx)),
            )
        };

        let section_head = |title: &'static str, desc: &'static str| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(text_px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(Theme::text()).child(t(title)))
                .child(div().text_size(text_px(11.)).line_height(text_px(16.)).text_color(Theme::text_tertiary()).child(t(desc)))
        };

        let card = ui::popover()
            .id("write-language-popover")
            .w(px(356.))
            .max_w(px(356.))
            .p(px(14.))
            .gap(px(12.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(section_head("写作语言（主稿）", "你动笔起草所用的语言；文献候选句检索、润色与改写均以此为准。"))
                    .child(div().flex().flex_wrap().gap(px(6.)).children(primary_chips)),
            )
            .child(div().h(px(1.)).bg(Theme::line()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(section_head("译文语言（目标语）", "主稿自动翻译成的目标语言；支持逐段或逐句修改，改完即可导出投稿。"))
                    .child(div().flex().flex_wrap().gap(px(6.)).children(target_chips)),
            )
            .child(div().h(px(1.)).bg(Theme::line()))
            .child(div().flex().items_center().justify_between().gap(px(8.)).child(default_control).child(right_actions));

        let (corner, pos) = from.map_or((Corner::TopLeft, at), |(bounds, side)| crate::app::menu_place(bounds, side, px(280.), window.viewport_size().height));
        let close = |this: &mut Self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>| {
            this.menu = None;
            cx.notify();
        };
        Some(
            div()
                .id("menu-layer")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(close))
                .on_mouse_down(MouseButton::Right, cx.listener(close))
                .child(anchored().anchor(corner).position(pos).snap_to_window_with_margin(px(8.)).child(card))
                .into_any_element(),
        )
    }

    /// The whole manuscript, to be read through; a heading opens its section. Its words can be selected, copied and
    /// commented on, and a comment's words are tinted (write_read.rs).
    pub(crate) fn render_manuscript_reading(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let Some(manuscript) = lib.writing.current() else { return div().into_any_element() };
        let sections: Vec<AnyElement> = (0..manuscript.sections.len()).map(|n| self.render_section_read(n, 1, false, window, cx)).collect();
        div()
            .id("write-reading-view")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&lib.citing.read_scroll)
            // No frame, like the section being written: one line under it, where the text scrolls away.
            .border_b_1()
            .border_color(Theme::line())
            .child(div().flex().justify_center().px(px(28.)).py(px(24.)).child(div().flex().flex_col().gap(px(22.)).w_full().max_w(theme::manuscript_width()).child(div().text_size(text_px(24.)).font_weight(FontWeight::SEMIBOLD).line_height(text_px(32.)).child(crate::i18n::manuscript_title(manuscript).to_string())).children(sections).children(self.render_references_read(cx))))
            .into_any_element()
    }

    /// The section at `n` as it reads, as the exported file has it: its name over its Markdown drawn as it says, a heading in it
    /// a level under its name, a picture's line the picture over its caption (write_figures.rs), its citations as
    /// the exported file has them (design rule 32), a caption's too. A click on its name opens it to write, and
    /// with `open` a click anywhere on it: where it stands under the section being written. `top` is the level
    /// drawn as the page's first: the whole manuscript's sections start at 1, what is under a section at one
    /// below it, and a heading is smaller for each level under that.
    pub(crate) fn render_section_read(&self, n: usize, top: u8, open: bool, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let Some(manuscript) = lib.writing.current() else { return div().into_any_element() };
        let Some(section) = manuscript.sections.get(n) else { return div().into_any_element() };
        let depth = section.level.saturating_sub(top) + 1;
        let written = manuscript::section_markdown(&section.body, depth);
        let labels = lib.citing.read.whole.get(n).map(Vec::as_slice).unwrap_or(&[]);
        // The text between two pictures is a piece; read through, its words can be selected, copied and commented on
        // (write_read.rs). Under the section being written a click anywhere opens it, so there it is only drawn.
        let mut pieces: Vec<AnyElement> = vec![];
        let mut words = 0;
        for (at, part) in crate::write_read::parts(&section.body, labels, depth).into_iter().enumerate() {
            match part {
                crate::write_read::Part::Words(_, shown) if open => pieces.push(crate::markdown::render_page(&format!("write-read-{n}-{at}"), &shown, READ_SIZE).into_any_element()),
                crate::write_read::Part::Words(within, shown) => {
                    pieces.push(self.render_read_piece(section, n, words, within, &shown, false, window, cx));
                    words += 1;
                }
                crate::write_read::Part::Picture(picture) => {
                    let drawn = self.render_manuscript_picture(format!("write-read-picture-{n}-{at}"), &picture, f32::from(theme::manuscript_width()), window);
                    pieces.push(if open { drawn } else { self.seek_hold(crate::write_seek::Holder::Words(section.id.clone(), picture.line.clone()), drawn) });
                }
            }
        }
        // A section of its subsections alone says nothing of itself: they are what it holds.
        let alone = manuscript::under(&manuscript.sections, n).len() == 1;
        let size = match depth {
            1 => 20.,
            2 => 17.,
            _ => 15.5,
        };
        let over = manuscript::parent(&manuscript.sections, n).map(|p| manuscript.sections[p].kind.as_str());
        div()
            .id(("write-read-section", n))
            .flex()
            .flex_col()
            .gap(px(10.))
            // A new section stands apart from the one before more than a subsection from its section.
            .when(section.level == 1 && n > 0, |d| d.mt(px(10.)))
            .when(open, |d| d.cursor_pointer().rounded(px(6.)).px(px(10.)).py(px(6.)).mx(px(-10.)).hover(|s| s.bg(Theme::hover())).on_click(self.on(Cmd::WriteSection(section.id.clone()), cx)))
            .child(
                div()
                    .id(("write-read-heading", n))
                    .flex()
                    .items_baseline()
                    .gap(px(8.))
                    .cursor_pointer()
                    .on_click(self.on(Cmd::WriteSection(section.id.clone()), cx))
                    .child(div().text_size(text_px(size)).font_weight(FontWeight::SEMIBOLD).text_color(Theme::text()).hover(|s| s.text_color(Theme::accent())).child(section.name.clone()))
                    .when(section.kind != "other" && over != Some(section.kind.as_str()), |d| d.child(div().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(label(&SECTIONS, &section.kind).to_string()))),
            )
            .when(written.is_empty() && alone, |d| d.child(div().text_size(text_px(13.)).text_color(Theme::text_tertiary()).child(t("这一节还没有写。"))))
            .when(!written.is_empty(), |d| d.child(div().flex().flex_col().gap(theme::paragraph_gap(10.)).text_size(text_px(READ_SIZE)).text_color(Theme::text()).children(pieces)))
            .into_any_element()
    }

    /// Under the section being written, what is under it, as it reads: 结果 is all its results, each opened with a
    /// click to be written alone. None for a section without subsections.
    pub(crate) fn render_under_section(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let writing = &self.lib.as_ref()?.writing;
        let manuscript = writing.current()?;
        let at = manuscript.sections.iter().position(|s| s.id == writing.section_id)?;
        let block = manuscript::under(&manuscript.sections, at);
        if block.len() < 2 { return None }
        // The open section is the page's first level: its subsections are drawn a level under it.
        let top = manuscript.sections[at].level;
        // Centred by a row that the editor's column stretches, as 预览 is. A width in percent does not reach this
        // far down the column: with `w_full().mx_auto()` the block was as wide as its longest line and stood in the
        // middle, so under 铺满 the subsections began right of the section's own text and had a short rule.
        Some(
            div()
                .flex()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .w_full()
                        .max_w(theme::manuscript_width())
                        .px(px(18.))
                        .pt(px(14.))
                        .pb(px(24.))
                        .border_t_1()
                        .border_color(Theme::line())
                        .children((block.start + 1..block.end).map(|n| self.render_section_read(n, top, true, window, cx))),
                )
                .into_any_element(),
        )
    }

    /// The dialog for a section's name and kind.
    pub(crate) fn render_section_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        let Some(Modal::Section { editing, under, name, kind }) = self.modal.as_ref() else { return None };
        // A subsection is said to be one, and whose.
        let parent = under.as_ref().and_then(|id| self.lib.as_ref()?.writing.current()?.section(id)).map(|s| s.name.clone());
        let kinds: Vec<AnyElement> = SECTIONS
            .iter()
            .enumerate()
            .map(|(n, (key, shown))| {
                let on = kind == key;
                div().id(("section-kind", n)).flex().flex_none().items_center().h(text_px(28.)).px(px(12.)).rounded(px(14.)).text_size(text_px(13.)).cursor_pointer().border_1().when(on, |d| d.bg(Theme::accent_soft()).text_color(Theme::accent()).border_color(Theme::accent().opacity(0.35))).when(!on, |d| d.border_color(Theme::line()).text_color(Theme::text_secondary()).hover(|s| s.bg(Theme::hover()))).on_click(self.on(Cmd::PickSectionKind(key.to_string()), cx)).child(t(shown).to_string()).into_any_element()
            })
            .collect();
        let hint = if kind == "other" { t("候选句来自文献里不属于导言、方法、结果、讨论的段落。综述的各节、致谢、展望都可以选它。") } else { t("候选句来自文献库里的文献在这一部分的句子。") };
        Some(
            ui::dialog(480.)
                .child(ui::dialog_title(match (editing.is_some(), &parent) {
                    (true, _) => t("这一节的名称与类型").to_string(),
                    (false, Some(parent)) => tf!("在「{parent}」下添加小节", "Add a subsection under “{parent}”"),
                    (false, None) => t("添加一节").to_string(),
                }))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .px(px(20.))
                        .py(px(8.))
                        .child(ui::form_row(t("名称"), ui::field(name, window, cx)))
                        .child(ui::form_row(t("类型"), div().flex().flex_wrap().gap(px(6.)).children(kinds)))
                        .child(div().text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_tertiary()).child(hint))
                        .child(ui::error_line(&self.modal_error)),
                )
                .child(self.manuscript_footer(if editing.is_some() { t("保存") } else { t("添加") }, true, cx)),
        )
    }

    fn manuscript_footer(&self, confirm: &'static str, enabled: bool, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_none()
            .justify_end()
            .gap(px(8.))
            .px(px(20.))
            .py(px(14.))
            .child(ui::secondary_button("modal-cancel", t("取消")).on_click(self.on(Cmd::CloseModal, cx)))
            .when(enabled, |d| d.child(ui::primary_button("modal-ok", confirm).on_click(self.on(Cmd::SubmitModal, cx))))
    }

    /// A section's earlier versions: one to look at, and to restore.
    pub(crate) fn render_history_dialog(&self, cx: &mut Context<Self>) -> Option<Div> {
        let Some(Modal::History { section, snapshots, chosen }) = self.modal.as_ref() else { return None };
        let name = self.lib.as_ref().and_then(|l| l.writing.current()?.section(section)).map(|s| s.name.clone()).unwrap_or_default();
        let why = snapshot_why;
        let rows: Vec<AnyElement> = snapshots
            .iter()
            .enumerate()
            .map(|(n, snapshot)| {
                let on = n == *chosen;
                let words = snapshot.body.split_whitespace().count();
                div()
                    .id(("history-row", n))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(Theme::accent_soft()))
                    .when(!on, |d| d.hover(|s| s.bg(Theme::hover())))
                    .on_click(self.on(Cmd::HistoryPick(n), cx))
                    .child(div().text_size(text_px(13.)).text_color(if on { Theme::accent() } else { Theme::text() }).child(wisp_core::time::short_time(&snapshot.taken_at)))
                    .child(div().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(tf!("{} · {words} 词", "{} · {words} words", why(&snapshot.why))))
                    .into_any_element()
            })
            .collect();
        let shown = snapshots.get(*chosen);
        let body = match shown {
            None => div().flex().flex_col().px(px(20.)).py(px(12.)).child(ui::empty_state("history", t("还没有历史版本"), &tf!("停笔 {} 秒，或换到别的一节时，这一节的文字有了变化就会留一个版本，最多 {} 个。", "When you pause for {} seconds or switch sections, changed text is saved as a version, up to {} versions.", manuscript::PAUSE_SECS, manuscript::KEEP_SNAPSHOTS))),
            Some(snapshot) => div()
                .flex()
                .h(px(380.))
                .gap(px(12.))
                .px(px(20.))
                .py(px(8.))
                .child(div().id("history-list").flex().flex_col().flex_none().w(px(170.)).h_full().overflow_y_scroll().gap(px(2.)).children(rows))
                .child(
                    div()
                        .id("history-text")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .gap(px(10.))
                        .p(px(14.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(Theme::line())
                        .bg(Theme::bg_body())
                        .when(snapshot.body.trim().is_empty(), |d| d.child(div().text_size(text_px(13.)).text_color(Theme::text_tertiary()).child(t("（这一版是空的）"))))
                        .when(!snapshot.body.trim().is_empty(), |d| d.child(div().text_size(text_px(14.)).text_color(Theme::text()).child(crate::markdown::render("write-version", &manuscript::section_markdown(&snapshot.body, 2), None, None)))),
                ),
        };
        Some(
            ui::dialog(680.)
                .child(ui::dialog_title(tf!("「{name}」的历史版本", "Version history of “{name}”")))
                .child(body)
                .child(div().px(px(20.)).child(ui::error_line(&self.modal_error)))
                .child(div().px(px(20.)).text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(t("恢复时，现在的文字会先留一个版本，所以恢复也可以撤回。")))
                .child(self.manuscript_footer(t("恢复这一版"), shown.is_some(), cx)),
        )
    }
}


#[cfg(test)]
mod answer_import_tests {
    use super::answer_source_labels;
    use wisp_core::{ai::Source, model::Paper};

    #[test]
    fn carries_known_and_unresolved_conversation_labels_into_new_draft() {
        let sources = vec![Source { label: "P1".into(), paper_id: "paper-id".into(), title: "A study".into(), ..Default::default() }];
        let papers = vec![Paper { id: "paper-id".into(), authors: "A Author".into(), year: Some(2020), doi: "10.1/example".into(), ..Default::default() }];
        let lookup = answer_source_labels("Result [P1 p.2], again [P1 p.3], unknown [P9].", &sources, &papers);
        assert_eq!(lookup.matches("[P1]").count(), 1);
        assert!(lookup.contains("A study") && lookup.contains("paper-id") && lookup.contains("10.1/example"));
        assert!(lookup.contains("[P9] 无法解析"));
        assert!(answer_source_labels("No citations", &sources, &papers).is_empty());
        let ambiguous = vec![sources[0].clone(), Source { label: "P1".into(), paper_id: "other".into(), title: "Other".into(), ..Default::default() }];
        assert!(answer_source_labels("[P1]", &ambiguous, &papers).contains("多个来源"));
    }
}
