//! A question and its answer: what the box takes (`chat_input`, pictures, files), `send_question`, the steps
//! of one answer (`chat_step`: a request, its stream, the tool calls it ends in, `run_tools`), stopping,
//! compacting a conversation that has outgrown the model's window, and keeping it (`save_chat`). A
//! conversation set aside while it answers goes on through `in_chat`.

use crate::app::{ScopeDo, Talk, Workspace};
use crate::chat_state::{Chat, ChatImage, ChatItem, MAX_ATTACHMENTS, PendingTools, Picture, PreparedCall, Retracted, Spent, exchange, said, wire_images};
use crate::i18n::t;
use crate::input::InputEvent;
use gpui::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use serde_json::Value;
use wisp_core::ai::{self, ChatEvent, Scope, ToolCall};
use wisp_core::libraries::LibraryInfo;
use wisp_core::model::*;
use wisp_core::settings::Task;
use wisp_core::store::{Conversation, Store};
use wisp_core::manuscript::Manuscript;
use wisp_core::{compact, time, tools, usage};

/// A question as the model is sent it: each token of `prompt` that names something the place knows written as the
/// model reads it, and the note of what they are after it (`wisp_core::mention`): a paper of `papers` (by `keys`), a
/// library of `shelf`, a tag of `tags` (when `offers.tags`) with its papers, a section of `manuscripts`. A paper not yet
/// among `sources` (a paper named, the first papers of a tag named) is added under the next label, as the tools' finds
/// are. `prompt` as it is when nothing is named.
/// A figure of `figures` (by its token's value, `Workspace::question_figures`) is named by its paper's label and page.
/// A method or an introduction of `kept` (when `offers.excerpts`) by its passage, notes and source, its paper under a
/// label as a paper named is.
#[allow(clippy::too_many_arguments)]
fn mention_note(prompt: &str, keys: &wisp_core::mention::Keys, papers: &[Paper], tags: &[(String, usize)], manuscripts: &[Manuscript], shelf: &[LibraryInfo], offers: crate::mention::Offers, figures: &[(String, wisp_core::mention::FigureSeen)], kept: &Kept, sources: Option<&mut Vec<ai::Source>>) -> String {
    use wisp_core::mention::{self, Named, Thing};
    let Some(sources) = sources else { return prompt.to_string() };
    let tokens = mention::tokens(prompt);
    if tokens.is_empty() { return prompt.to_string() }
    // The papers of a tag, as the list counts them: those judged irrelevant apart.
    let tagged = |name: &str| -> Vec<&Paper> {
        if !offers.tags || !tags.iter().any(|(known, _)| known == name) { return vec![] }
        papers.iter().filter(|paper| paper.screening != "irrelevant" && paper.tags.iter().any(|tag| tag == name)).collect()
    };
    let own = |sources: &[ai::Source], id: &str| sources.iter().any(|source| source.paper_id == id && source.library.is_empty());
    // The paper of a figure named is the conversation's: a source already, unless it was taken up from 历史.
    let figure_paper = |value: &str| wisp_core::mention::figure_named(value).map(|named| named.paper);
    let figured: Vec<String> = figures.iter().filter_map(|(value, _)| figure_paper(value)).collect();
    let mut named_ids: Vec<&str> = tokens.iter().filter_map(|token| keys.paper(&token.named)).chain(figured.iter().map(String::as_str)).collect();
    let excerpt = |value: &str| kept.excerpts.iter().find(|excerpt| offers.excerpts && excerpt.id == value);
    for token in &tokens {
        if let Named::Other { kind: mention::TAG, value } = token.named { named_ids.extend(tagged(&mention::decode(value)).into_iter().take(10).map(|paper| paper.id.as_str())) }
        // The paper a method or an introduction is from is cited by its label.
        if let Named::Other { kind: mention::EXCERPT, value } = token.named && let Some(excerpt) = excerpt(value) && !excerpt.paper_id.is_empty() { named_ids.push(excerpt.paper_id.as_str()) }
    }
    let mut missing: Vec<Paper> = vec![];
    for id in named_ids {
        if own(sources, id) || missing.iter().any(|paper| paper.id == id) { continue }
        if let Some(paper) = papers.iter().find(|paper| paper.id == id) { missing.push(paper.clone()) }
    }
    if !missing.is_empty() {
        let mut ctx = ai::Context { sources: std::mem::take(sources), ..Default::default() };
        ctx.add_sources(&missing);
        *sources = ctx.sources;
    }
    let sources: &[ai::Source] = sources;
    let label = |id: &str| sources.iter().find(|source| source.paper_id == id && source.library.is_empty()).map(|source| source.label.clone());
    let find = |named: &Named| -> Option<Thing> {
        match named {
            Named::Other { kind: mention::LIBRARY, value } => shelf.iter().find(|info| info.id == *value).map(Thing::Library),
            Named::Other { kind: mention::TAG, value } => {
                let name = mention::decode(value);
                let papers = tagged(&name);
                if papers.is_empty() { return None }
                Some(Thing::Tag { papers: papers.into_iter().map(|paper| (label(&paper.id).unwrap_or_default(), paper)).collect(), name })
            }
            Named::Other { kind: mention::SECTION, value } => manuscripts.iter().find_map(|manuscript| manuscript.section(value).map(|section| Thing::Section { manuscript, section })),
            Named::Other { kind: mention::FIGURE, value } => {
                let value = mention::decode(value);
                let (_, seen) = figures.iter().find(|(known, _)| *known == value)?;
                Some(Thing::Figure(mention::FigureSeen { label: label(&figure_paper(&value)?)?, ..seen.clone() }))
            }
            Named::Other { kind: mention::EXCERPT, value } => {
                let excerpt = excerpt(value)?;
                let paper = papers.iter().find(|paper| !excerpt.paper_id.is_empty() && paper.id == excerpt.paper_id).and_then(|paper| Some((paper, label(&paper.id)?)));
                let attachment = kept.files.iter().find(|(id, _)| *id == excerpt.id).and_then(|(_, n)| *n);
                // A method's figures as the person linked them; an introduction has none.
                let figures: Vec<&wisp_core::figure::Figure> = if excerpt.kind == wisp_core::excerpt::METHOD { kept.links.iter().filter(|(of, _)| *of == excerpt.id).filter_map(|(_, figure)| kept.figures.iter().find(|known| known.id == *figure)).collect() } else { vec![] };
                Some(Thing::Excerpt { excerpt, paper, attachment, figures })
            }
            named => {
                let id = keys.paper(named)?;
                let paper = papers.iter().find(|paper| paper.id == id)?;
                Some(Thing::Paper { paper, label: label(id)? })
            }
        }
    };
    let (text, named) = mention::resolve(prompt, find, crate::i18n::is_english());
    if named.is_empty() { prompt.to_string() } else { format!("{text}\n\n{}", mention::note(&named)) }
}

/// The methods and introductions a question may name, as `mention_note` reads them: the library's, the links to
/// figures, the figures of 图片库, and for each named one selected in an attachment that attachment's number.
struct Kept<'a> {
    excerpts: &'a [wisp_core::excerpt::Excerpt],
    links: &'a [(String, String)],
    figures: &'a [wisp_core::figure::Figure],
    files: Vec<(String, Option<usize>)>,
}

/// The steps one question may take. Each is a request to the model; all but
/// the last end in tool calls. With the results before the last one the
/// model is told to answer from what it has (`ai::LAST_STEP_NOTE`).
const MAX_CHAT_STEPS: usize = 24;

/// The tools that write a new file outside the library, a Word file cited anew and slides: where the file goes is asked in a save dialog (`place_outputs`).
const PLACED: [&str; 2] = ["restyle_docx", SLIDES];
const SLIDES: &str = "make_slides";
/// What a call hears of a tool the conversation was given that is no longer there to call (`ai::Table`).
const GONE: &str = "这个工具现在用不了：它所在的连接器已经关闭或断开，或者技能都已关闭。请照实告诉用户，这次对话里不要再调用它。";
/// The tool that leaves the assistant's remarks on a manuscript as comments: they are read again after it ran (`comments_arrived`).
const COMMENT: &str = "comment_manuscript";
/// The tool that puts a card of buttons under the answer: the window keeps its arguments (`Chat::offer`).
const OFFER: &str = "offer_action";

/// The tool that takes a file in as a manuscript: the writing page shows it once it ran (`manuscript_arrived`).
const TAKE_IN: &str = "import_manuscript";

impl Workspace {
    /// Run `f` with the conversation `key` in `talk`'s place of the open library: the one there, or one set aside while it
    /// was answering (`Library::aside`), exchanged with it for the call and back after. None when it is neither: it was
    /// stopped, removed or let go.
    pub(crate) fn in_chat<R>(&mut self, talk: Talk, key: u64, cx: &mut Context<Self>, f: impl FnOnce(&mut Self, &mut Context<Self>) -> R) -> Option<R> {
        let lib = self.lib.as_mut()?;
        if lib.chat(talk).key == key { return Some(f(self, cx)) }
        let at = lib.aside.iter().position(|chat| chat.key == key).filter(|_| talk == Talk::Library)?;
        let mut aside = lib.aside.remove(at);
        exchange(&mut lib.library_chat, &mut aside);
        let result = f(self, cx);
        let lib = self.lib.as_mut()?;
        exchange(&mut lib.library_chat, &mut aside);
        if aside.busy {
            lib.aside.insert(at.min(lib.aside.len()), aside);
        } else {
            // Kept as it went (`save_chat`): 历史 has it, and lists it anew if it is open.
            if lib.library_chat.history.is_some() { lib.library_chat.history = lib.list_history(Talk::Library).ok() }
            self.say(t("放到后台的那段对话答完了，在「历史」里可以找到"), cx);
        }
        cx.notify();
        Some(result)
    }

    /// The library's conversation, answering, makes room for another: it goes on off screen (`Library::aside`) and its
    /// place gets an empty one, with the same box.
    pub(crate) fn chat_aside(&mut self, cx: &mut Context<Self>) {
        let mut fresh = Chat::new("", cx);
        let Some(lib) = self.lib.as_mut() else { return };
        exchange(&mut lib.library_chat, &mut fresh);
        fresh.history = None;
        lib.aside.push(fresh);
    }

    /// Take up from 历史 a conversation set aside while it answers, as it stands. True when `id` was one.
    pub(crate) fn take_aside(&mut self, id: &str) -> bool {
        let Some(lib) = self.lib.as_mut() else { return false };
        let Some(at) = lib.aside.iter().position(|chat| !id.is_empty() && chat.id == id) else { return false };
        let mut taken = lib.aside.remove(at);
        exchange(&mut lib.library_chat, &mut taken);
        lib.library_chat.history = None;
        lib.library_chat.scroll.scroll_to_bottom();
        true
    }

    /// A key or a paste in a conversation's box.
    pub(crate) fn chat_input(&mut self, talk: Talk, event: InputEvent, cx: &mut Context<Self>) {
        // While what the `@` being typed can name is on offer, the keys are that list's.
        if self.mention_key(crate::mention::Place::Chat(talk), event, cx) { return }
        let skills = self.skills_on();
        let Some(lib) = self.lib.as_mut() else { return };
        let chat = lib.chat_mut(talk);
        let (offered, picked) = (chat.slash_list(&self.settings.commands, &skills, cx).len(), chat.slash);
        match event {
            // Enter on a command in the list takes it; otherwise the message goes.
            InputEvent::Submit if offered > 0 => return self.pick_slash(talk, picked, cx),
            InputEvent::Submit => return self.ask(talk, cx),
            InputEvent::Tab if offered > 0 => return self.pick_slash(talk, picked, cx),
            InputEvent::Changed => {
                chat.slash_closed = false;
                chat.slash = 0;
            }
            InputEvent::Up if offered > 0 => chat.slash = (chat.slash + offered - 1) % offered,
            InputEvent::Down if offered > 0 => chat.slash = (chat.slash + 1) % offered,
            InputEvent::Cancel => chat.slash_closed = true,
            InputEvent::Images => {
                let pictures = chat.input.update(cx, |input, _| input.take_images()).into_iter().map(|image| Picture::Bytes(image.bytes)).collect();
                return self.attach_images(talk, pictures, cx);
            }
            _ => {}
        }
        // While the list is on show the arrow keys and Tab are its keys.
        let choosing = !chat.slash_list(&self.settings.commands, &skills, cx).is_empty() || chat.mentions.open(chat.input.read(cx));
        chat.input.update(cx, |input, _| input.choosing = choosing);
        cx.notify();
    }

    /// Take one of the slash commands on offer: one the window carries out
    /// runs, the others are written out for their subject to follow, or
    /// to be sent as they are.
    pub(crate) fn pick_slash(&mut self, talk: Talk, at: usize, cx: &mut Context<Self>) {
        let skills = self.skills_on();
        let Some(lib) = self.lib.as_mut() else { return };
        let chat = lib.chat_mut(talk);
        let Some(command) = chat.slash_list(&self.settings.commands, &skills, cx).into_iter().nth(at) else { return };
        let alone = command.runs;
        chat.slash = 0;
        chat.input.update(cx, |input, cx| {
            input.choosing = false;
            input.set_text(&if alone { format!("/{}", command.name) } else { format!("/{} ", command.name) }, cx);
        });
        if alone { self.ask(talk, cx) }
        cx.notify();
    }

    /// Word and PowerPoint files to go with the next question of a conversation: kept by their paths, read only when the assistant's tools are.
    pub(crate) fn attach_documents(&mut self, talk: Talk, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let chat = lib.chat_mut(talk);
        for path in paths {
            if !chat.documents.contains(&path) { chat.documents.push(path) }
        }
        cx.notify();
    }

    /// Read pictures in and put them with the next question. Each is made
    /// one a model takes (`picture::prepare`) off the main thread.
    pub(crate) fn attach_images(&mut self, talk: Talk, mut pictures: Vec<Picture>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let (library, selected) = (lib.info.id.clone(), lib.selected.clone());
        let chat = lib.chat_mut(talk);
        let room = MAX_ATTACHMENTS.saturating_sub(chat.attachments.len() + chat.attaching);
        let too_many = pictures.len() > room;
        pictures.truncate(room);
        chat.attaching += pictures.len();
        if too_many { self.fail(tf!("一次提问最多带 {MAX_ATTACHMENTS} 张图片。", "A question can include at most {MAX_ATTACHMENTS} images."), cx) }
        if pictures.is_empty() { return }
        let work = cx.background_executor().spawn(async move {
            let read = |picture: Picture| match picture {
                Picture::Bytes(bytes) => Ok(bytes),
                Picture::File(path) => std::fs::read(&path).map_err(|err| tf!("无法读取 {}：{err}", "Cannot read {}: {err}", path.display())),
            };
            pictures.into_iter().map(|picture| read(picture).and_then(crate::picture::prepare)).collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let prepared = work.await;
            this.update(cx, |this, cx| {
                // Another library, or another paper's conversation by now: the pictures were not meant for it.
                let Some(lib) = this.lib.as_mut().filter(|l| l.info.id == library && (talk != Talk::Paper || l.selected == selected)) else { return };
                let chat = lib.chat_mut(talk);
                chat.attaching = chat.attaching.saturating_sub(prepared.len());
                let mut failed = None;
                for picture in prepared {
                    match picture {
                        Ok(image) => chat.attachments.push(ChatImage { id: new_id("I"), image: Arc::new(image) }),
                        Err(err) => failed = Some(err),
                    }
                }
                if let Some(err) = failed { this.fail(err, cx) }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn ask(&mut self, talk: Talk, cx: &mut Context<Self>) {
        self.send_question(talk, None, cx);
    }

    /// Ask what the library's box holds: the first question of a conversation started on the home page.
    pub(crate) fn ask_library(&mut self, cx: &mut Context<Self>) {
        self.ask(Talk::Library, cx);
    }

    /// Ask what is in the box; or, with `again`, a question asked before
    /// (its words, pictures and files), leaving the box alone. True when it was sent.
    fn send_question(&mut self, talk: Talk, again: Option<(String, Vec<ChatImage>, Vec<String>)>, cx: &mut Context<Self>) -> bool {
        let profile = self.settings.profile_for_or_default(Task::Chat);
        // The conversation that writes says of its manuscript what it is and where it stands.
        let composing = if talk == Talk::Write {
            let Some(note) = self.agent_note(cx) else {
                self.fail(t("先在写作里打开一份稿件。"), cx);
                return false;
            };
            Some(note)
        } else {
            None
        };
        // A conversation not yet made a library: what it was handed to review, said with every question, as the
        // manuscript is to the conversation that writes it (the manuscript can come after the first question).
        let draft = talk == Talk::Library && self.in_draft();
        let shelf: Vec<LibraryInfo> = self.libraries.iter().filter(|info| draft && !info.draft).cloned().collect();
        let skills = self.skills_on();
        // What `@` names here besides papers: the same as the box offered.
        let offers = self.offers(crate::mention::Place::Chat(talk));
        let reviewing = self.lib.as_ref().filter(|_| draft).and_then(|lib| {
            let handed: Vec<(&Manuscript, Option<String>)> = lib.writing.manuscripts.iter().filter(|m| m.sections.iter().any(|s| !s.body.trim().is_empty())).map(|m| (m, lib.store.manuscript_source(&m.id).ok().flatten().filter(|source| source.kind == "docx").map(|source| lib.store.source_file(&source).display().to_string()))).collect();
            ai::manuscript_review_note(&handed)
        });
        // The figures named in a paper's conversation: what the model is told of each, and their pictures. One still
        // read or cut out holds the question back until it is (`mention::figures_ready` sends it then).
        if self.lib.as_ref().is_none_or(|lib| lib.chat(talk).busy) { return false }
        let figures = {
            let Some(lib) = self.lib.as_ref() else { return false };
            let chat = lib.chat(talk);
            let (text, have) = match &again {
                Some((text, images, _)) => (text.clone(), images.clone()),
                None => (chat.input.read(cx).text().to_string(), chat.attachments.clone()),
            };
            match self.question_figures(talk, &text, &have, again.is_some(), cx) {
                Some(figures) => figures,
                None => {
                    if let Some(lib) = self.lib.as_mut() {
                        let chat = lib.chat_mut(talk);
                        chat.mentions.waiting = true;
                        chat.note = t("正在截取问题里点到的图，截好就发送…").into();
                    }
                    cx.notify();
                    return false;
                }
            }
        };
        let Some(lib) = self.lib.as_mut() else { return false };
        lib.chat_mut(talk).mentions.waiting = false;
        // The papers the paper's conversation is about: the one on show and those kept beside it.
        let about = lib.talk_papers();
        // The libraries' names in a question say nothing of the language it is asked in.
        let names: Vec<String> = std::iter::once(lib.info.name.clone()).chain(shelf.iter().map(|info| info.name.clone())).collect();
        let chat = chat_of!(lib, talk);
        let (question, again, files) = match again {
            Some((text, images, files)) => (text, Some(images), files),
            None => (chat.input.read(cx).text().trim().to_string(), None, chat.documents.iter().map(|path| path.display().to_string()).collect::<Vec<_>>()),
        };
        // A Word file sent without a word is to have its citations looked at; a PowerPoint file, to be read as the reference for slides.
        let (decks, words): (Vec<String>, Vec<String>) = files.iter().cloned().partition(|file| crate::comments::is_deck(std::path::Path::new(file)));
        let question = match (question.is_empty(), words.is_empty(), decks.is_empty()) {
            (true, false, _) => t("看看这个 Word 文件的引用，我想重排它的引用格式。").to_string(),
            (true, true, false) => t("看看这份幻灯片：之后做幻灯片时以它为参考。").to_string(),
            _ => question,
        };
        let asked_again = again.is_some();
        // A review is begun once, by the question that first asked for it.
        let begin_review = !asked_again && talk == Talk::Library && question.strip_prefix("/review").is_some_and(|rest| rest.starts_with(char::is_whitespace));
        let nothing = match &again { Some(images) => question.is_empty() && images.is_empty(), None => question.is_empty() && chat.attachments.is_empty() && chat.quotes.is_empty() };
        if nothing || chat.busy { return false }
        chat.error.clear();
        chat.note.clear();
        if !asked_again && chat.attaching > 0 {
            chat.error = t("图片还在读取，稍等一下再发送。").into();
            cx.notify();
            return false;
        }
        // `/search …` and the like stand for a longer request, a skill's name for its instructions; the bubble shows what was typed.
        let asked = if question.is_empty() && !asked_again && !chat.quotes.is_empty() { t("这段原文说了什么？在这篇文献里起什么作用？") } else { question.as_str() };
        // The language the answer is to be in, by the person's words: everything else the model reads is in Chinese.
        let reply = ai::reply_language(asked, &names.iter().map(String::as_str).collect::<Vec<_>>(), crate::i18n::is_english());
        let prompt = match ai::read_input(asked, &self.settings.commands, &skills) {
            Ok(ai::Input::Ask(prompt)) => prompt,
            Ok(ai::Input::NewChat) => {
                chat.reset(cx);
                if talk == Talk::Library { lib.scope_reset(cx) }
                cx.notify();
                return false;
            }
            Ok(ai::Input::Compact) => {
                chat.input.update(cx, |input, cx| input.set_text("", cx));
                self.compact_chat(talk, true, cx);
                return false;
            }
            Err(err) => {
                chat.error = err;
                cx.notify();
                return false;
            }
        };
        chat.history = None;
        // The panel of papers floats over the place the answer comes to.
        if talk != Talk::Paper { chat.materials_open = false }

        // A changed context or protocol starts the wire again from the text of what was said:
        // the model's own turns are only valid in the conversation that produced them.
        let replay = chat.ctx.is_none() || chat.protocol != Some(profile.protocol);
        if chat.ctx.is_none() {
            let scope = match (talk, about.as_slice()) {
                (Talk::Paper, []) => return false,
                (Talk::Paper, [one]) => Scope::Paper(one.clone()),
                // Several papers side by side are read together (#194).
                (Talk::Paper, several) => Scope::Papers(several.to_vec()),
                _ => Scope::Library,
            };
            let fulltext = match lib.store.all_fulltext() {
                Ok(fulltext) => fulltext,
                Err(err) => {
                    chat.error = format!("{err:#}");
                    cx.notify();
                    return false;
                }
            };
            // A question about one paper reads that paper; one about the library reads the current list.
            let papers: Vec<&Paper> = match &scope {
                Scope::Paper(_) | Scope::Papers(_) => lib.papers.iter().collect(),
                // The assistant that writes draws on the whole library, whatever the list shows; so does a conversation not yet made a library, which has no list.
                Scope::Library if talk == Talk::Write || draft => lib.papers.iter().collect(),
                // As `Library::scope_papers`, field by field: the conversation is held meanwhile.
                Scope::Library if !lib.asked.is_empty() => lib.papers.iter().filter(|p| lib.asked.contains(&p.id)).collect(),
                Scope::Library => lib.shown.iter().map(|&i| &lib.papers[i]).filter(|p| lib.scope_hits.as_ref().is_none_or(|hits| hits.contains(&p.id))).collect(),
            };
            // A Word file to restyle needs no paper of the library.
            // Nor does a conversation not yet made a library: it starts from an idea, and looks in the literature databases.
            // Nor a library with no paper yet, for the same reason (#192): only a list the filters emptied is a mistake to point out.
            if papers.is_empty() && !lib.papers.is_empty() && talk != Talk::Write && files.is_empty() && !draft {
                chat.error = t("当前列表里没有文献。清空筛选条件后再问。").into();
                cx.notify();
                return false;
            }
            // A conversation taken up from the history keeps the labels its answers already use.
            // A paper is read with its attachments: supplementary figures, tables and methods are part of it.
            let attachments = match &scope {
                Scope::Paper(id) => lib.store.attachment_pages(id).unwrap_or_default(),
                Scope::Papers(_) | Scope::Library => vec![],
            };
            // The papers named with @ are among those given their whole record when room is short: their titles count as the question's words.
            let named: Vec<&str> = wisp_core::mention::tokens(&prompt).iter().filter_map(|token| lib.keys.paper(&token.named)).filter_map(|id| lib.papers.iter().find(|paper| paper.id == id)).map(|paper| paper.title.as_str()).collect();
            let ranked = if named.is_empty() { prompt.clone() } else { format!("{prompt}\n{}", named.join("\n")) };
            let mut ctx = ai::build_context(&lib.info.name, &lib.info.topic, &papers, &lib.fields, &fulltext, &attachments, &scope, &ranked, &chat.known);
            // Its tables kept as files (副表) are named, not handed over: `read_table` reads what a question needs of them.
            if let Scope::Paper(id) = &scope { ctx.add_tables(id, &lib.files) }
            // The tools reach every paper of the library, so every paper can be cited.
            ctx.add_sources(&lib.papers);
            // What an earlier part of the conversation read in other libraries is among them: `build_context` keeps every known source.
            // Without a library the model is told so, and where papers are found then.
            // And which libraries are on the shelf, which it can read and not change (`tools::ACROSS`).
            if draft { ctx.instructions = format!("{}{}", ai::draft_instructions(&ctx), ai::shelf_note(&shelf)) }
            chat.ctx = Some(ctx);
            // What the model is given is new from its first word: the tools are those on offer now, and none of before.
            chat.table.clear();
            // The panel over the conversation lists what was handed over from here on: all of it, not what its search had narrowed the list to.
            if talk == Talk::Library && lib.scope_hits.take().is_some() {
                lib.scope_words.clear();
                lib.scope_find.update(cx, |input, cx| input.set_text("", cx));
            }
        }
        if replay {
            // Questions with their pictures, as they were sent (what a slash command stood for then); the pieces of one answer as one turn.
            let mut wire: Vec<Value> = vec![];
            // A compacted conversation begins at its summary: what the summary stands for is not sent again.
            let from = match &chat.compacted {
                Some(compacted) => {
                    if !compacted.summary.is_empty() { wire = compact::summary_messages(&compacted.summary) }
                    compacted.at.min(chat.messages.len())
                }
                None => 0,
            };
            let mut answer = String::new();
            for item in &chat.messages[from..] {
                match item {
                    ChatItem::User { text, images, prompt, .. } => {
                        if !answer.is_empty() { wire.push(ai::assistant_message(&std::mem::take(&mut answer))) }
                        // A question from before the request was kept is read again as it would be now.
                        let sent = if prompt.is_empty() { ai::expand_slash(text) } else { prompt.clone() };
                        wire.push(ai::user_message_with(profile.protocol, &sent, &wire_images(images)));
                    }
                    ChatItem::Answer(text) if !text.is_empty() => {
                        if !answer.is_empty() { answer.push_str("\n\n") }
                        answer.push_str(text);
                    }
                    _ => {}
                }
            }
            if !answer.is_empty() { wire.push(ai::assistant_message(&answer)) }
            chat.wire = wire;
            chat.protocol = Some(profile.protocol);
        }
        // The things named with @: each token written as the model reads it (its label and short name), and a note of them
        // after the question. A paper not yet a source of the conversation gets the next label; the context itself is not touched.
        // The attachment each method or introduction named was selected in, as the answers number them.
        let named: Vec<String> = wisp_core::mention::tokens(&prompt).iter().filter_map(|token| match token.named { wisp_core::mention::Named::Other { kind: wisp_core::mention::EXCERPT, value } => Some(value.to_string()), _ => None }).collect();
        let excerpt_files: Vec<(String, Option<usize>)> = lib.excerpts.items.iter().filter(|excerpt| named.contains(&excerpt.id) && !excerpt.resource_id.is_empty()).map(|excerpt| (excerpt.id.clone(), lib.attachment_number(&excerpt.paper_id, &excerpt.resource_id))).collect();
        let kept = Kept { excerpts: &lib.excerpts.items, links: &lib.excerpts.links, figures: &lib.gallery.items, files: excerpt_files };
        let chat = chat_of!(lib, talk);
        let prompt = mention_note(&prompt, &lib.keys, &lib.papers, &lib.tags, &lib.writing.manuscripts, &shelf, offers, &figures.seen, &kept, chat.ctx.as_mut().map(|ctx| &mut ctx.sources));
        // The passages selected in a PDF go first, written as the answers cite: the labels are known by now.
        // A question asked again has them in its words already; the box's stay for the box.
        let quotes = if asked_again { vec![] } else { std::mem::take(&mut chat.quotes) };
        // A passage of an attachment is written with its number, as the answers cite it.
        let numbers: Vec<Option<usize>> = quotes.iter().map(|quote| lib.attachment_number(&quote.paper_id, &quote.file_id)).collect();
        let chat = chat_of!(lib, talk);
        let quoted: Vec<String> = quotes.iter().zip(numbers).filter_map(|(quote, file)| Some(ai::quoted_passage(&chat.sources().iter().find(|source| source.paper_id == quote.paper_id)?.label, file, quote.page, &quote.text))).collect();
        let (prompt, question) = if quoted.is_empty() {
            (prompt, question)
        } else {
            (format!("我在文献的 PDF 里选中了下面的原文，问题是就它提的；引用它时沿用这里的出处写法。\n{}\n\n{prompt}", quoted.join("\n")), [quoted.join("\n"), question].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n"))
        };
        // The record, not what the model says of itself, is where the review stands: each question of a review's conversation says so.
        // Only what steers the writing goes along (the sections by name: sending their text with every question would send it many times over),
        // except with the question that asks for the saved sections to be checked, which is about that text.
        let prompt = match chat.review.as_ref().filter(|_| talk == Talk::Library) {
            Some(review) if crate::chat::verifies_sections(&question) && review.sections.iter().any(|section| !section.body.trim().is_empty()) => format!("{prompt}\n\n{}\n\n{}", crate::chat::review_note(review), crate::chat::review_sections(review)),
            Some(review) => format!("{prompt}\n\n{}", crate::chat::review_note(review)),
            None => prompt,
        };
        // The same for the conversation that writes a manuscript: where the manuscript stands, what is on show and selected in it.
        let prompt = match composing.or(reviewing) {
            Some(note) => format!("{prompt}\n\n{note}"),
            None => prompt,
        };
        // The files that came with the question: the assistant is told where they are and what it can do with them.
        let prompt = if words.is_empty() { prompt } else { format!("{prompt}\n\n{}", wisp_core::restyle::note(&words)) };
        let prompt = if decks.is_empty() { prompt } else { format!("{prompt}\n\n{}", wisp_core::slideref::note(&decks)) };
        let prompt = match reply { Some(reply) => format!("{prompt}\n\n{reply}"), None => prompt };
        // The pictures of the figures it names go after those it has; a question asked again has them already.
        let images = match again {
            Some(images) => images,
            None => std::mem::take(&mut chat.attachments).into_iter().chain(figures.pictures).collect(),
        };
        let documents = if asked_again { vec![] } else { std::mem::take(&mut chat.documents) };
        chat.wire.push(ai::user_message_with(profile.protocol, &prompt, &wire_images(&images)));
        // Kept with the question, so the conversation taken up later asks what was asked now, whatever the command says by then.
        let sent = if prompt == question { String::new() } else { prompt };
        chat.messages.push(ChatItem::User { text: question, images, prompt: sent, files });
        chat.steps = 0;
        (chat.no_summary, chat.squeezed, chat.overflow, chat.spent) = (false, false, None, None);
        chat.started = Some(Instant::now());
        // What the last question changed in the manuscript has been seen: the next changes are this one's.
        if talk == Talk::Write {
            let asked = match chat.messages.last() { Some(ChatItem::User { text, .. }) => text.clone(), _ => String::new() };
            self.agent_asked(&asked);
        }
        let sent = self.chat_step(talk, cx);
        if let Some(lib) = self.lib.as_mut() {
            let chat = chat_of!(lib, talk);
            if sent {
                // What 撤回 took away is not put back after a question that followed it.
                chat.retracted = None;
                if !asked_again { chat.input.update(cx, |input, cx| input.set_text("", cx)) }
            } else {
                // Nothing was sent: the question stays in the box, its pictures beside it, to be asked again (one asked again goes back to its place, `regenerate`).
                chat.wire.pop();
                let popped = chat.messages.pop();
                if !asked_again {
                    if let Some(ChatItem::User { images, text, .. }) = popped { chat.attachments = crate::mention::box_pictures(&mut lib.figure_cuts, &text, images) }
                    chat.quotes = quotes;
                    chat.documents = documents;
                }
                if chat.messages.is_empty() { chat.ctx = None }
            }
        }
        // Kept from the question on, so that closing the window mid-answer does not lose it.
        if sent {
            self.save_chat(talk, cx);
            // The transcript is saved first so the review and its conversation
            // can be associated atomically, including across application restarts.
            if begin_review { self.start_review(cx) }
        }
        cx.notify();
        sent
    }

    /// 撤回: the question at `at` and all that followed are taken out of the
    /// conversation, and its words and pictures come back to the box, to be
    /// changed and sent again. Until the next question it can be put back
    /// (`unretract`). What a tool already did to the library stays done.
    pub(crate) fn retract(&mut self, talk: Talk, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let chat = chat_of!(lib, talk);
        if chat.busy || !matches!(chat.messages.get(at), Some(ChatItem::User { .. })) { return }
        let mut tail = chat.cut(at);
        // Nothing left: the material is chosen again with the next first question.
        if chat.messages.is_empty() { chat.forget_context() }
        let Some(ChatItem::User { text, images, files, .. }) = tail.first() else { return };
        let (text, images) = (text.clone(), images.clone());
        for file in files.iter().map(PathBuf::from) {
            if !chat.documents.contains(&file) { chat.documents.push(file) }
        }
        // A second 撤回 further up takes the first's along: both are put back together, with the box as it was before the first.
        let (draft, attachments) = match chat.retracted.take() {
            Some(earlier) => {
                tail.extend(earlier.tail);
                (earlier.draft, earlier.attachments)
            }
            None => (chat.input.read(cx).text().to_string(), chat.attachments.clone()),
        };
        // What was being typed meanwhile is not lost: it follows the words taken back.
        let shown = if draft.trim().is_empty() { text } else { format!("{text}\n\n{}", draft.trim()) };
        chat.input.update(cx, |input, cx| input.set_text(&shown, cx));
        chat.retracted = Some(Retracted { tail, draft, attachments: attachments.clone() });
        // A figure's picture stays with the figure's token: put with the question again when it is sent.
        let images = crate::mention::box_pictures(&mut lib.figure_cuts, &shown, images);
        chat.attachments = images.into_iter().chain(attachments).collect();
        chat.input.read(cx).focus(window);
        self.save_chat(talk, cx);
        cx.notify();
    }

    /// Put back what the last 撤回 took away, and the box as it was before.
    pub(crate) fn unretract(&mut self, talk: Talk, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let chat = chat_of!(lib, talk);
        if chat.busy { return }
        let Some(retracted) = chat.retracted.take() else { return };
        chat.messages.extend(retracted.tail);
        // Written again from the text with the next question, as after the cut.
        chat.wire.clear();
        chat.protocol = None;
        chat.error.clear();
        chat.note.clear();
        chat.input.update(cx, |input, cx| input.set_text(&retracted.draft, cx));
        chat.attachments = retracted.attachments;
        chat.scroll.scroll_to_bottom();
        self.save_chat(talk, cx);
        cx.notify();
    }

    /// 重新回答: the answer that ends at `at` (the conversation's last) is
    /// asked for again with the question before it, under the material
    /// the conversation already has.
    pub(crate) fn regenerate(&mut self, talk: Talk, at: usize, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let chat = chat_of!(lib, talk);
        if chat.busy || at + 1 != chat.messages.len() { return }
        let Some(asked) = chat.messages[..at].iter().rposition(|item| matches!(item, ChatItem::User { .. })) else { return };
        let mut tail = chat.cut(asked);
        let question = tail.remove(0);
        let ChatItem::User { text, images, files, .. } = &question else { return };
        let again = (text.clone(), images.clone(), files.clone());
        chat.retracted = None;
        if !self.send_question(talk, Some(again), cx) && let Some(lib) = self.lib.as_mut() {
            // Not sent (no model, no key): the conversation is as it was, and says why.
            let chat = chat_of!(lib, talk);
            chat.messages.push(question);
            chat.messages.extend(tail);
            cx.notify();
        }
    }

    /// 从这里分支: what was said up to the end of the answer at `at`
    /// becomes a conversation of its own, the one on show from now on; the
    /// whole of the one it came from stays in the history.
    pub(crate) fn fork_chat(&mut self, talk: Talk, at: usize, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let chat = lib.chat(talk);
        if chat.busy || chat.review.is_some() || at >= chat.messages.len() { return }
        self.save_chat(talk, cx);
        let Some(lib) = self.lib.as_mut() else { return };
        let chat = chat_of!(lib, talk);
        chat.cut(at + 1);
        chat.retracted = None;
        // Kept anew under an id of its own, its pictures with it; a yes to every change was said to the other conversation.
        chat.id.clear();
        chat.created_at.clear();
        chat.kept = false;
        chat.stored_images.clear();
        chat.allow_changes = false;
        self.save_chat(talk, cx);
        self.say(t("已分支为新对话，原对话仍在「历史」里"), cx);
        cx.notify();
    }

    /// Send the conversation to the model and follow its turn: the text as
    /// it arrives, then either the end of the answer or the tools it asked
    /// for. False when the request could not be made.
    fn chat_step(&mut self, talk: Talk, cx: &mut Context<Self>) -> bool {
        let profile = self.settings.profile_for_or_default(Task::Chat);
        // Beside the library's tools: the connectors' and the one that reads a skill.
        let outside: Vec<_> = self.outside_tools().into_iter().map(|(tool, _)| tool).collect();
        // Web search is offered only while an engine can be asked: switched off or without a key, the model would only hear why not.
        let searches = wisp_core::websearch::ready(&self.settings, "").is_ok();
        // Pages are read for the assistant by an engine that takes the file; a model's look at pictures of them is the window's own run.
        let reads = wisp_core::ocr::ready(&self.settings, "").is_ok_and(|ready| ready.engine.key != wisp_core::ocr::MODEL);
        let Some(lib) = self.lib.as_mut() else { return false };
        // A conversation of the home page also reads the libraries on the shelf (`tools::ACROSS`); one inside a library keeps to it.
        let draft = lib.info.draft && talk == Talk::Library;
        let chat = chat_of!(lib, talk);
        if chat.ctx.is_none() { return false }
        // What is on offer is looked at when a question is asked, not at every step of its answer: a connector that comes up or
        // drops out in the middle of one would have the service read the whole request anew (`ai::Table`).
        if chat.steps == 0 || chat.table.is_empty() {
            let mut catalog = if draft { tools::draft_catalog() } else { tools::library_catalog() };
            catalog.retain(|tool| (searches || tool.name != "web_search") && (reads || tool.name != "ocr_pdf"));
            chat.table.offer(&catalog, &outside);
        }
        let build = |chat: &Chat| ai::chat_request_with(&profile, chat.ctx.as_ref()?, &chat.wire, &[], chat.table.tools()).ok();
        let mut request = match chat.ctx.as_ref().map(|ctx| ai::chat_request_with(&profile, ctx, &chat.wire, &[], chat.table.tools())) {
            Some(Ok(request)) => request,
            Some(Err(err)) => {
                chat.error = err;
                chat.end_run();
                return false;
            }
            None => return false,
        };
        // A conversation that has outgrown the model's window is compacted before it is sent (`wisp_core::compact`): first by rule, the
        // tool results of earlier turns cut to their head and tail; then by a summary of what was said before this question, which the
        // model writes (`compact_chat`, which takes this step once it has it); the oldest turns are left out only when no summary came.
        let estimate = compact::tokens(&request.body);
        let overflow = chat.overflow.take();
        if let Some(protocol) = chat.protocol.filter(|_| overflow.is_some() || (estimate as f32 * chat.calibration) as usize > compact::limit(&profile)) {
            // What the request holds besides the conversation: the instructions, the documents, the tools.
            let fixed = estimate.saturating_sub(compact::wire_tokens(&chat.wire));
            let (limit, scale, forced) = (compact::limit(&profile), chat.calibration, overflow.is_some());
            // The service said too long where the estimate says it fits: the estimate is no guide, every layer is tried.
            let over = |wire: &[Value]| forced || ((fixed + compact::wire_tokens(wire)) as f32 * scale) as usize > limit;
            let before = compact::wire_tokens(&chat.wire);
            match compact::make_room(protocol, &mut chat.wire, (limit as f32 / scale) as usize / 10, over) {
                compact::Room::Summarise if !chat.no_summary => {
                    chat.overflow = overflow;
                    self.compact_chat(talk, false, cx);
                    return true;
                }
                compact::Room::Summarise => {
                    let dropped = compact::drop_turns(&mut chat.wire, over);
                    if dropped > 0 {
                        let (from, summary, earlier) = chat.compacted.take().map_or((0, String::new(), 0), |compacted| (compacted.at, compacted.summary, compacted.dropped));
                        chat.compacted = Some(compact::Compaction { at: chat.question_at(from, dropped), summary, before, after: compact::wire_tokens(&chat.wire), dropped: earlier + dropped });
                    }
                    compact::tighten(protocol, &mut chat.wire, over);
                }
                compact::Room::Done => {}
            }
            if compact::wire_tokens(&chat.wire) != before {
                let Some(shorter) = build(chat) else { return false };
                request = shorter;
            } else if let Some(error) = overflow {
                // Nothing could be taken out: the service's refusal stands.
                chat.error = error;
                chat.end_run();
                return false;
            }
        }
        chat.sent_estimate = compact::tokens(&request.body);
        chat.messages.push(ChatItem::Answer(String::new()));
        chat.busy = true;
        chat.steps += 1;
        chat.scroll.scroll_to_bottom();
        let (generation, key) = (chat.generation, chat.key);
        let library = lib.info.id.clone();

        let (sender, events) = async_channel::unbounded();
        let protocol = profile.protocol;
        // DeepSeek says what it read from its cache, and its prices are listed: the question's use is shown under the answer.
        let deepseek = usage::is_deepseek(&profile).then(|| profile.model.clone());
        cx.background_executor().spawn(async move { wisp_net::stream_chat(request, protocol, sender) }).detach();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                // The conversation's library, on show or answering off screen (`in_library`), and the conversation, in its place or set aside (`in_chat`).
                let alive = this.update(cx, |this, cx| this.in_library(&library, cx, |this, cx| this.in_chat(talk, key, cx, |this, cx| {
                    let Some(lib) = this.lib.as_mut() else { return false };
                    let chat = chat_of!(lib, talk);
                    if chat.generation != generation { return false }
                    // Keep following the answer while the reader is at the bottom.
                    let at_bottom = chat.scroll.offset().y <= -chat.scroll.max_offset().height + px(40.);
                    let mut calls = None;
                    let mut again = false;
                    let more = match event {
                        ChatEvent::Text(text) => {
                            chat.note.clear();
                            if let Some(ChatItem::Answer(answer)) = chat.messages.last_mut() { answer.push_str(&text) }
                            true
                        }
                        ChatEvent::Retrying(attempt, why) => {
                            chat.note = tf!("模型服务暂时没有回应（{why}），正在重试，第 {attempt} 次…", "The model service has not responded ({why}). Retrying, attempt {attempt}…");
                            true
                        }
                        ChatEvent::Used(used) => {
                            // The service's own count corrects the estimate the next step is measured with.
                            if chat.sent_estimate > 0 && used.input > 0 { chat.calibration = (used.input as f32 / chat.sent_estimate as f32).clamp(0.5, 3.) }
                            if let Some(model) = &deepseek {
                                let spent = chat.spent.get_or_insert_with(Spent::default);
                                spent.usage.add(&used);
                                spent.requests += 1;
                                // Priced request by request: the hour it was made in decides the rate.
                                if let Some(cost) = usage::deepseek_cost(model, &used, time::now_millis() / 1000) { spent.cost = Some(spent.cost.unwrap_or(0.) + cost) }
                            }
                            true
                        }
                        // Too long for the model after all (its window is smaller than assumed): the conversation is compacted and the step taken again, once for a question.
                        ChatEvent::Error(error) if compact::context_exceeded(&error) && !chat.squeezed => {
                            chat.note.clear();
                            chat.squeezed = true;
                            chat.overflow = Some(error);
                            chat.drop_empty_answer();
                            chat.steps = chat.steps.saturating_sub(1);
                            again = true;
                            false
                        }
                        ChatEvent::Error(error) => {
                            chat.note.clear();
                            chat.error = error;
                            chat.end_run();
                            false
                        }
                        ChatEvent::Done(turn) => {
                            chat.note.clear();
                            chat.wire.extend(turn.wire());
                            if turn.calls.is_empty() {
                                chat.note = ai::stop_note(&turn.stop).unwrap_or_default().to_string();
                                chat.end_run();
                            } else if !turn.runnable() || chat.steps >= MAX_CHAT_STEPS {
                                // The calls still get an answer: a conversation that ends on an unanswered call cannot continue.
                                let (why, note) = if turn.runnable() {
                                    (t("已达到单次提问的步数上限，工具没有执行。"), t("已达到单次提问的步数上限。可以继续提问，让它接着做。"))
                                } else {
                                    (t("回答被截断，工具没有执行。"), ai::stop_note(&turn.stop).unwrap_or(t("回答没有正常结束。")))
                                };
                                let results: Vec<(String, String, bool)> = turn.calls.iter().map(|call| (call.id.clone(), why.to_string(), true)).collect();
                                chat.wire.extend(ai::tool_results(protocol, &results));
                                chat.note = note.to_string();
                                chat.end_run();
                            } else {
                                chat.drop_empty_answer();
                                calls = Some(turn.calls);
                            }
                            false
                        }
                    };
                    if at_bottom { chat.scroll.scroll_to_bottom() }
                    let ended = !chat.busy;
                    if let Some(calls) = calls { this.run_tools(talk, calls, cx) }
                    if ended || (again && !this.chat_step(talk, cx)) { this.save_chat(talk, cx) }
                    if ended { this.read_balance(talk, cx) }
                    cx.notify();
                    more
                }).unwrap_or(false)).unwrap_or(false));
                if !matches!(alive, Ok(true)) { break }
            }
        })
        .detach();
        true
    }

    /// Make the tools a turn asked for ready, show them, and run them:
    /// at once, or after the person's consent when one of them would bring
    /// papers in, take them out, or drop a saved search.
    fn run_tools(&mut self, talk: Talk, calls: Vec<ToolCall>, cx: &mut Context<Self>) {
        // A connector's tool or a skill: looked up before the library is borrowed.
        let mut outside: Vec<_> = calls.iter().map(|call| self.outside_call(&call.name, &call.args)).collect();
        outside.reverse();
        // The libraries on the shelf by id and name, and this one's own: a call that reads another of them says which.
        let shelf: Vec<(String, String)> = self.libraries.iter().filter(|info| !info.draft).map(|info| (info.id.clone(), info.name.clone())).collect();
        let Some(lib) = self.lib.as_mut() else { return };
        let own = (lib.info.id.clone(), lib.info.name.clone());
        let store = &lib.store;
        // The manuscript the conversation that writes is about.
        let about = Some(lib.writing.agent.about.clone()).filter(|id| talk == Talk::Write && !id.is_empty());
        let chat = chat_of!(lib, talk);
        let first = chat.messages.len();
        let catalog = tools::catalog();
        let mut prepared = vec![];
        // A section the conversation that writes wants to write anew and that would lose most of its words: shown in its place while the yes is waited for.
        let mut proposal = None;
        for call in calls {
            if let Some((outside, confirm, line, details)) = outside.pop().flatten() {
                chat.messages.push(ChatItem::Tool { call: line.clone(), result: None });
                prepared.push(PreparedCall { id: call.id, name: call.name, args: call.args, writes: false, confirm, line, details, outside: Some(outside) });
                continue;
            }
            let tool = catalog.iter().find(|t| t.name == call.name);
            // The model may name a paper by its label; one read in another library brings that library along.
            let composes = tools::MANUSCRIPT.contains(&call.name.as_str()) || ["read_manuscript", COMMENT].contains(&call.name.as_str());
            // The person reads a section by its name, not by the id the model calls it by.
            let named = call.args.as_ref().ok().filter(|_| composes).and_then(|args| {
                let manuscript = wisp_core::compose::find_manuscript(store, args["manuscript"].as_str().map(str::trim).filter(|named| !named.is_empty()).or(about.as_deref()).unwrap_or("")).ok()?;
                let name = manuscript.section(args["section"].as_str()?.trim())?.name.clone();
                let mut args = args.clone();
                args["section"] = Value::String(name);
                Some(args)
            });
            let mut line = tools::describe_call(&call.name, named.as_ref().or(call.args.as_ref().ok()).unwrap_or(&Value::Null));
            let args = call.args.map(|mut args| {
                ai::resolve_labels(&mut args, chat.sources());
                args
            });
            // A tool the conversation was given and that is no longer there to call (`ai::Table` keeps it as it was sent): a connector
            // switched off or not connected, the last skill switched off. The call fails with why, not with there being no such tool.
            let args = if tool.is_none() && chat.table.has(&call.name) { Err(GONE.to_string()) } else { args };
            // A call that reads another library says which, by its name: the line is what the person sees of it.
            if let Some(named) = args.as_ref().ok().and_then(|args| args["library"].as_str()).map(str::trim).filter(|named| !named.is_empty() && *named != own.0 && *named != own.1) {
                let name = shelf.iter().find(|(id, name)| id == named || name.eq_ignore_ascii_case(named)).map_or(named, |(_, name)| name.as_str());
                line = tf!("{line} · 文献库「{name}」", "{line} · in “{name}”");
            }
            chat.messages.push(ChatItem::Tool { call: line.clone(), result: None });
            let args = args.map(|mut args| {
                // Said of no manuscript, a call of that conversation is about its own.
                if composes && let Some(about) = &about && args["manuscript"].as_str().is_none_or(|named| named.trim().is_empty()) && args.is_object() { args["manuscript"] = Value::String(about.clone()) }
                args
            });
            // In the conversation that writes a manuscript the pen was handed over: writing that manuscript is not asked about again.
            let handed = about.as_ref().is_some_and(|about| tools::MANUSCRIPT.contains(&call.name.as_str()) && args.as_ref().is_ok_and(|args| args["manuscript"].as_str() == Some(about)));
            // But for a section written anew that would lose most of its words (`compose::Replacing::drops`): the pen is for writing, and a
            // section is not cut to a few words on a model's word alone (one model put a note to itself where 7,400 characters stood).
            let offered = args.as_ref().ok().filter(|_| handed && call.name == "write_section").and_then(|args| wisp_core::compose::proposed(store, args, chat.sources())).filter(|proposed| proposed.replacing.drops);
            let handed = handed && offered.is_none();
            if proposal.is_none() { proposal = offered }
            // A call that cannot run as written has nothing to ask about: it fails by itself. Nor has one that writes a
            // new file (a Word file cited anew, slides): the person says where it goes in the save dialog, which is their yes (`place_outputs`).
            let asks = args.is_ok() && tool.is_some_and(|t| t.confirm) && !handed && !PLACED.contains(&call.name.as_str());
            // What it would change, for the card that asks; one that would be refused anyway is not asked about.
            let details = match &args {
                // A manuscript's lines count the text as the manuscript will have it, its citations without the sentences their labels quote.
                Ok(args) if asks && tools::MANUSCRIPT.contains(&call.name.as_str()) => wisp_core::compose::preview_for(store, &call.name, args, chat.sources()).ok(),
                Ok(args) if asks => tools::preview_call(store, &call.name, args).ok(),
                _ => Some(vec![]),
            };
            let (confirm, details) = (asks && details.is_some(), details.unwrap_or_default());
            prepared.push(PreparedCall { id: call.id, name: call.name, args, writes: tool.is_some_and(|t| t.writes), confirm, line, details, outside: None });
        }
        chat.scroll.scroll_to_bottom();
        if prepared.iter().any(|call| call.confirm) && !chat.allow_changes {
            chat.pending = Some(PendingTools { calls: prepared, first });
            // The question is put where the text is: the section comes up, and in its place stand what it would hold and what it holds.
            let elsewhere = proposal.as_ref().zip(about).filter(|(proposal, _)| proposal.section != lib.writing.section_id).map(|(proposal, about)| (about, proposal.section.clone()));
            if talk == Talk::Write { lib.writing.agent.proposal = proposal }
            if let Some((manuscript, section)) = elsewhere { self.write_show(manuscript, section, cx) }
            return cx.notify();
        }
        if talk == Talk::Write { lib.writing.agent.proposal = None }
        self.place_outputs(talk, prepared, first, 0, cx);
    }

    /// Before a call that writes a new file runs (`PLACED`: a Word file cited anew, slides), the person says where the
    /// file goes: the save dialog 导出 uses, opened beside the file given, or in the person's documents for slides, which
    /// are named by their title. Closing it is a no to that call. The file given is never the one written, and a file
    /// already there is not written over: the new one gets a free name beside it. Then the calls run (`execute_tools`).
    pub(crate) fn place_outputs(&mut self, talk: Talk, mut calls: Vec<PreparedCall>, first: usize, from: usize, cx: &mut Context<Self>) {
        let next = calls.iter().enumerate().skip(from).find(|(_, call)| PLACED.contains(&call.name.as_str()) && call.outside.is_none() && call.args.is_ok()).map(|(n, _)| n);
        let (Some(at), Some(lib)) = (next, self.lib.as_mut()) else { return self.execute_tools(talk, calls, first, true, cx) };
        let (library, generation, key) = (lib.info.id.clone(), lib.chat(talk).generation, lib.chat(talk).key);
        let args = calls[at].args.clone().unwrap_or_default();
        let scripted = wisp_core::settings::env("DEV").is_some();
        let (name, directory, ending) = if calls[at].name == SLIDES {
            // The deck's title, or that of the paper it is about.
            let title = args["title"].as_str().map(str::trim).filter(|title| !title.is_empty()).or_else(|| args["paperId"].as_str().and_then(|id| lib.papers.iter().find(|paper| paper.id == id)).map(|paper| paper.title.as_str())).unwrap_or("");
            let stem: String = wisp_core::archive::file_stem(title, "slides").chars().take(80).collect();
            // A scripted run writes nothing into the person's documents.
            let directory = if scripted { self.root.clone() } else { dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(|| self.root.clone()) };
            (format!("{stem}.pptx"), directory, "pptx")
        } else {
            let source = PathBuf::from(args["path"].as_str().unwrap_or(""));
            // `draft-nature.docx`: the file's name and the style's.
            let style = args["style"].as_str().and_then(|key| key.split(':').nth(1)).map(|name| name.trim_end_matches(".csl").to_string()).filter(|name| !name.is_empty()).unwrap_or_else(|| "restyled".into());
            let name = format!("{}-{style}.docx", source.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_else(|| "document".into()));
            (name, source.parent().filter(|folder| folder.is_dir()).map(std::path::Path::to_path_buf).or_else(dirs::document_dir).or_else(dirs::home_dir).unwrap_or_else(|| self.root.clone()), "docx")
        };
        // A scripted run has nobody at a dialog: the file goes beside the one given.
        let picked = (!scripted).then(|| cx.prompt_for_new_path(&directory, Some(&name)));
        cx.spawn(async move |this, cx| {
            let chosen = match picked {
                Some(picked) => picked.await.ok().and_then(Result::ok).flatten(),
                None => Some(directory.join(&name)),
            };
            this.update(cx, |this, cx| this.in_library(&library, cx, |this, cx| this.in_chat(talk, key, cx, |this, cx| {
                // Another conversation by now: its calls are not run for this one.
                if !this.lib.as_mut().is_some_and(|lib| lib.chat_mut(talk).generation == generation) { return }
                match chosen {
                    Some(mut path) => {
                        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ending)) { path.set_extension(ending); }
                        let (folder, stem) = (path.parent().map(std::path::Path::to_path_buf).unwrap_or_default(), path.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_default());
                        let mut n = 2;
                        while path.exists() {
                            path = folder.join(format!("{stem} ({n}).{ending}"));
                            n += 1;
                        }
                        if let Ok(args) = calls[at].args.as_mut() { args["output"] = Value::String(path.display().to_string()) }
                    }
                    None => calls[at].args = Err("用户关掉了保存对话框，没有选保存位置，新文件没有写。不要重试；问用户是否还要输出。".into()),
                }
                this.place_outputs(talk, calls, first, at + 1, cx);
            })))
            .ok();
        })
        .detach();
    }

    /// Run a turn's tools off the main thread, then hand the results to
    /// the model with the next step. Calls that only read run side by
    /// side; a turn that changes the library runs in the order it was
    /// given. Without the person's consent (`allowed`) the calls that asked
    /// for it are answered with a refusal and the others still run.
    pub(crate) fn execute_tools(&mut self, talk: Talk, calls: Vec<PreparedCall>, first: usize, allowed: bool, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let info = lib.info.clone();
        let chat = lib.chat_mut(talk);
        let Some(protocol) = chat.protocol else { return };
        // What the conversation can cite. A call that reads another library adds the papers it found there, so the calls
        // of a turn share the list: two of them running side by side never give one label to two papers.
        let sources: Arc<std::sync::Mutex<Vec<ai::Source>>> = Arc::new(std::sync::Mutex::new(chat.sources().to_vec()));
        let (known, read) = (sources.lock().map_or(0, |sources| sources.len()), sources.clone());
        let root = self.root.clone();
        let (generation, key) = (chat.generation, chat.key);
        chat.running = calls.iter().map(|call| call.id.clone()).collect();
        let wrote = calls.iter().any(|call| call.writes && (allowed || !call.confirm));
        let composed = calls.iter().any(|call| tools::MANUSCRIPT.contains(&call.name.as_str()) && (allowed || !call.confirm));
        let commented = calls.iter().any(|call| call.name == COMMENT);
        let taken_in = calls.iter().any(|call| call.name == TAKE_IN && call.outside.is_none() && (allowed || !call.confirm));
        // The last card offered in this step; arguments that are no card are the tool's to refuse.
        let offer = calls.iter().filter(|call| call.name == OFFER).filter_map(|call| call.args.as_ref().ok().and_then(|args| wisp_core::offer::parse(args).ok())).next_back();
        let library = info.id.clone();
        let executor = cx.background_executor().clone();
        let tasks: Vec<gpui::Task<Vec<(String, String, bool, String)>>> = if wrote || calls.len() == 1 {
            vec![executor.spawn(async move { calls.into_iter().map(|call| run_tool(&root, &info, &sources, call, allowed)).collect() })]
        } else {
            calls
                .into_iter()
                .map(|call| {
                    let (root, info, sources) = (root.clone(), info.clone(), sources.clone());
                    executor.spawn(async move { vec![run_tool(&root, &info, &sources, call, allowed)] })
                })
                .collect()
        };
        cx.spawn(async move |this, cx| {
            let mut done = vec![];
            for task in tasks {
                done.extend(task.await);
            }
            this.update(cx, |this, cx| this.in_library(&library, cx, |this, cx| {
                let Some(lib) = this.lib.as_mut() else { return };
                // A change was made whether or not its answer is still wanted: the window reads the library again.
                let reloaded = if wrote { lib.reload() } else { Ok(()) };
                // And the manuscripts, when one was written: the editor follows, and what changed is shown.
                if composed { this.manuscripts_synced(cx) }
                // Comments left on a manuscript: they are shown on it, the section on show first.
                // A file taken in as a manuscript: it is listed in 写作 and shown there.
                if taken_in { this.manuscript_arrived(cx) }
                if commented { this.comments_arrived(cx) }
                // The rest is the conversation's, in its place or set aside (`in_chat`).
                this.in_chat(talk, key, cx, |this, cx| {
                    let Some(lib) = this.lib.as_mut() else { return };
                    // Papers read in another library are citable from here on, under the labels their results carried.
                    if let (Some(ctx), Ok(read)) = (chat_of!(lib, talk).ctx.as_mut(), read.lock()) && ctx.sources.len() == known { ctx.sources.extend(read.iter().skip(known).cloned()) }
                    // Papers brought in by a tool are citable from the next step on, under labels that follow the ones in use.
                    if wrote && let Some(ctx) = chat_of!(lib, talk).ctx.as_mut() { ctx.add_sources(&lib.papers) }
                    let chat = lib.chat_mut(talk);
                    if chat.generation == generation {
                        for (n, (_, _, failed, shown)) in done.iter().enumerate() {
                            if let Some(ChatItem::Tool { result, .. }) = chat.messages.get_mut(first + n) { *result = Some((shown.clone(), *failed)) }
                        }
                        if let Some(offer) = offer { chat.offer = Some((chat.messages.len(), offer)) }
                        let results: Vec<(String, String, bool)> = done.into_iter().map(|(id, output, failed, _)| (id, output, failed)).collect();
                        let mut results = ai::tool_results(protocol, &results);
                        // One step is left: the model is told to answer with what it has.
                        if chat.steps + 1 >= MAX_CHAT_STEPS { results = ai::with_note(protocol, results, ai::LAST_STEP_NOTE) }
                        chat.wire.extend(results);
                        chat.running.clear();
                        if !this.chat_step(talk, cx) { this.save_chat(talk, cx) }
                    }
                });
                if let Err(err) = reloaded { this.fail(format!("{err:#}"), cx) }
                cx.notify();
            }))
            .ok();
        })
        .detach();
    }

    /// Stop the answer being given. What still arrives for it is dropped;
    /// the conversation stays one that can go on: the calls in flight are
    /// answered as stopped, and the text so far stays as the model's turn.
    pub(crate) fn stop_chat(&mut self, talk: Talk, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let chat = lib.chat_mut(talk);
        if !chat.busy { return }
        chat.generation += 1;
        // A summary asked for by the person, with no answer awaited: the conversation is as it was.
        let idle = chat.compacting && chat.started.is_none();
        let unanswered: Vec<String> = match chat.pending.take() {
            Some(pending) => pending.calls.into_iter().map(|call| call.id).collect(),
            None => std::mem::take(&mut chat.running),
        };
        if let Some(protocol) = chat.protocol {
            if !unanswered.is_empty() {
                let results: Vec<(String, String, bool)> = unanswered.into_iter().map(|id| (id, t("用户中止了这次提问，这个工具的结果没有交回。").to_string(), true)).collect();
                chat.wire.extend(ai::tool_results(protocol, &results));
            } else if !idle && let Some(ChatItem::Answer(text)) = chat.messages.last() && !text.is_empty() {
                chat.wire.push(ai::assistant_message(text));
            }
        }
        for item in chat.messages.iter_mut() {
            if let ChatItem::Tool { result, .. } = item && result.is_none() { *result = Some((t("已中止").into(), true)) }
        }
        chat.note = t("已停止。").into();
        chat.end_run();
        self.save_chat(talk, cx);
        cx.notify();
    }

    /// Have the model write a summary of the conversation, and give it that in place of what was said from then on
    /// (`wisp_core::compact`). While a question is being answered (`chat_step` found the conversation too long) the summary
    /// stands for everything before that question, and the step is taken once it is in; asked for by the person (`/compact`)
    /// it stands for everything said. The summary is made from what the conversation shows (questions, answers, the lines of
    /// the tool calls), not from the wire: that is what a conversation taken up again is sent, and it needs no tools or
    /// thinking blocks to be valid. The conversation on show keeps every message; a line marks where the summary stands.
    fn compact_chat(&mut self, talk: Talk, manual: bool, cx: &mut Context<Self>) {
        let profile = self.settings.profile_for_or_default(Task::Chat);
        let Some(lib) = self.lib.as_mut() else { return };
        let library = lib.info.id.clone();
        let chat = chat_of!(lib, talk);
        if manual && chat.busy {
            chat.error = t("请先等回答结束，再压缩对话。").into();
            return cx.notify();
        }
        let at = if manual { chat.messages.len() } else { chat.messages.iter().rposition(|item| matches!(item, ChatItem::User { .. })).unwrap_or(0) };
        // An earlier summary goes into the new one with what was said since.
        let (from, earlier) = match &chat.compacted {
            Some(compacted) if compacted.at <= at => (compacted.at, compacted.summary.clone()),
            _ => (0, String::new()),
        };
        let items: Vec<ai::Said> = chat.messages[from..at].iter().filter_map(said).collect();
        (chat.error, chat.note) = (String::new(), String::new());
        if manual && !items.iter().any(|item| matches!(item, ai::Said::Answer { .. })) {
            chat.note = if from > 0 { t("上次压缩之后还没有新的回答，不需要再压缩。") } else { t("还没有可以压缩的对话。") }.into();
            return cx.notify();
        }
        // What the summary takes the place of, as the model would be sent it.
        let prompt = compact::summary_prompt(&items, &earlier, compact::window(&profile) / 2);
        let before = if chat.wire.is_empty() { compact::text_tokens(&prompt) } else if manual { compact::wire_tokens(&chat.wire) } else { compact::wire_tokens(&chat.wire[..compact::turn_starts(&chat.wire).last().copied().unwrap_or(0)]) };
        let request = ai::plain_request(&profile, compact::SUMMARY_SYSTEM, &prompt);
        chat.busy = true;
        chat.compacting = true;
        chat.history = None;
        chat.scroll.scroll_to_bottom();
        let (generation, key, protocol) = (chat.generation, chat.key, profile.protocol);
        let reply = cx.background_executor().spawn(async move { wisp_net::complete(request?, protocol) });
        cx.spawn(async move |this, cx| {
            let reply = reply.await;
            this.update(cx, |this, cx| this.in_library(&library, cx, |this, cx| this.in_chat(talk, key, cx, |this, cx| this.summary_arrived(talk, generation, at, before, manual, reply, cx)))).ok();
        })
        .detach();
        cx.notify();
    }

    /// An answer from DeepSeek is in: ask what the account still holds, for the line under the answer (`Chat::spent`).
    /// Asked once per question, and not at all for another service; an answer that does not come leaves the line without it.
    fn read_balance(&mut self, talk: Talk, cx: &mut Context<Self>) {
        let profile = self.settings.profile_for_or_default(Task::Chat);
        let Some(lib) = self.lib.as_ref() else { return };
        let chat = lib.chat(talk);
        if chat.spent.is_none() || !usage::is_deepseek(&profile) { return }
        let Ok(request) = usage::balance_request(&profile) else { return };
        let (library, generation, key) = (lib.info.id.clone(), chat.generation, chat.key);
        let reply = cx.background_executor().spawn(async move { wisp_net::balance(&request).and_then(|body| usage::read_balance(&body)) });
        cx.spawn(async move |this, cx| {
            let Ok(balance) = reply.await else { return };
            this.update(cx, |this, cx| {
                this.in_library(&library, cx, |this, cx| {
                    let Some(lib) = this.lib.as_mut() else { return };
                    let chat = chat_of!(lib, talk);
                    if chat.key == key && chat.generation == generation && let Some(spent) = chat.spent.as_mut() { spent.balance = Some(balance) }
                    cx.notify();
                })
            })
            .ok();
        })
        .detach();
    }

    /// The summary `compact_chat` asked for is in, or could not be had.
    fn summary_arrived(&mut self, talk: Talk, generation: u64, at: usize, before: usize, manual: bool, reply: Result<String, String>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let chat = chat_of!(lib, talk);
        if chat.generation != generation || !chat.compacting { return }
        chat.compacting = false;
        let summary = reply.and_then(|reply| compact::read_summary(&reply).ok_or_else(|| t("模型没有写出摘要。").to_string()));
        if manual { chat.busy = false }
        match summary {
            Ok(summary) => {
                let pair = compact::summary_messages(&summary);
                let after = compact::wire_tokens(&pair);
                if manual {
                    // Written again from the summary with the next question, as after a cut.
                    chat.wire.clear();
                    chat.protocol = None;
                } else {
                    let current = compact::turn_starts(&chat.wire).last().copied().unwrap_or(0);
                    chat.wire.splice(..current, pair);
                }
                chat.compacted = Some(compact::Compaction { at, summary, before, after, dropped: 0 });
                chat.compacted_open = false;
                // The line that says so stands under all that was said: it is brought into view.
                if manual { chat.scroll.scroll_to_bottom() }
                // A refusal as too long is answered by this summary: the step after it goes by the estimate again.
                chat.overflow = None;
                if manual { self.say(tf!("已压缩：约 {before} → {after} token", "Compacted: about {before} → {after} tokens"), cx) }
            }
            Err(err) if manual => chat.error = tf!("对话没有压缩成：{err}", "The conversation was not compacted: {err}"),
            Err(err) => {
                // The question is still answered: `chat_step` leaves the oldest turns out instead.
                chat.no_summary = true;
                self.say(tf!("摘要没有写成（{err}），较早的对话这次不再发给模型", "No summary could be written ({err}); the earlier conversation is left out this time"), cx);
            }
        }
        if manual || !self.chat_step(talk, cx) { self.save_chat(talk, cx) }
        cx.notify();
    }

    /// Keep the conversation in the library's history, as it stands.
    pub(crate) fn save_chat(&mut self, talk: Talk, cx: &mut Context<Self>) {
        // Listed with what its first question names with @ by their short names, as its bubble shows them.
        let title = self.lib.as_ref().map(|lib| lib.chat(talk).transcript().title_by(|text| self.mention_plain(crate::mention::Place::Chat(talk), text))).unwrap_or_default();
        let Some(lib) = self.lib.as_mut() else { return };
        // What the conversation is about is what it is kept under: nothing for the library's, the paper, the manuscript.
        let Some(paper_id) = lib.talk_about(talk) else { return };
        let chat = chat_of!(lib, talk);
        if !chat.messages.iter().any(|item| matches!(item, ChatItem::User { .. })) { return }
        let mut conversation = Conversation { id: chat.id.clone(), paper_id, transcript: chat.transcript(), created_at: chat.created_at.clone(), title, ..Default::default() };
        let mut kept = vec![];
        let result = lib.store.save_conversation(&mut conversation).and_then(|()| {
            for item in &chat.messages {
                let ChatItem::User { images, .. } = item else { continue };
                for picture in images.iter().filter(|picture| !chat.stored_images.contains(&picture.id)) {
                    lib.store.save_conversation_image(&conversation.id, &picture.id, picture.image.format.mime_type(), &picture.image.bytes)?;
                    kept.push(picture.id.clone());
                }
            }
            Ok(())
        });
        chat.stored_images.extend(kept);
        (chat.id, chat.created_at) = (conversation.id, conversation.created_at);
        chat.kept = result.is_ok();
        if let Err(err) = result { self.fail(tf!("这次对话没能保存：{err:#}", "Conversation not saved: {err:#}"), cx) }
    }

    /// Take up a conversation from the history: what was said is shown
    /// again, and the next question continues it. The model is given the
    /// library as it is now, under the labels the old answers use.
    pub(crate) fn open_chat(&mut self, talk: Talk, id: &str, cx: &mut Context<Self>) {
        self.open_chat_said(talk, id, cx);
        // The figures its questions name are drawn by their numbers and lead to them once the paper's figures are read.
        if talk == Talk::Paper { self.want_figures(cx) }
        if talk == Talk::Library && let Some(Err(err)) = self.lib.as_mut().map(|lib| lib.load_review()) { self.fail(tf!("无法读取综述记录：{err:#}", "Cannot read review record: {err:#}"), cx) }
    }

    /// The search box of the panel over the library's conversation was typed into.
    pub fn scope_typed(&mut self, words: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        lib.scope_words = words;
        lib.scope_refilter();
        cx.notify();
    }

    /// The search over 历史 was typed into: the conversations holding every word are looked up once, here.
    pub fn history_typed(&mut self, talk: Talk, words: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let hits = if words.trim().is_empty() { Ok(None) } else { lib.store.conversations_saying(&words).map(Some) };
        match hits {
            Ok(hits) => lib.chat_mut(talk).history_hits = hits,
            Err(err) => self.fail(format!("{err:#}"), cx),
        }
        cx.notify();
    }

    /// The panel over the library's conversation changes what the first question is put to.
    pub(crate) fn scope(&mut self, what: ScopeDo, cx: &mut Context<Self>) {
        self.pinning = false;
        let Some(lib) = self.lib.as_mut() else { return };
        // The material is handed over with the first question; after that the scope is the conversation's.
        let open = lib.library_chat.ctx.is_none() && lib.library_chat.messages.is_empty();
        match what {
            ScopeDo::ClearFind => {
                lib.scope_find.update(cx, |input, cx| input.set_text("", cx));
                return self.scope_typed(String::new(), cx);
            }
            _ if !open => return self.say(t("这次对话的材料已经交给模型。在「新对话」里可以重新限定文献。"), cx),
            ScopeDo::Pin(id) => {
                if lib.paper(&id).is_some() && !lib.asked.contains(&id) { lib.asked.push(id) }
            }
            ScopeDo::Unpin(id) => lib.asked.retain(|asked| *asked != id),
            ScopeDo::UnpinAll => lib.asked.clear(),
        }
        cx.notify();
    }

    fn open_chat_said(&mut self, talk: Talk, id: &str, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let conversation = match lib.store.conversation(id) {
            Ok(Some(conversation)) => conversation,
            Ok(None) => return self.fail(t("这次对话已经不在了。"), cx),
            Err(err) => return self.fail(format!("{err:#}"), cx),
        };
        if talk == Talk::Library { lib.scope_reset(cx) }
        let chat = chat_of!(lib, talk);
        chat.reset(cx);
        for item in conversation.transcript.items {
            chat.messages.push(match item {
                ai::Said::User { text, images, prompt, files } => {
                    let mut pictures = vec![];
                    for image_id in images {
                        // A picture that cannot be read back is left out; the question stays.
                        let Ok(Some((media_type, data))) = lib.store.conversation_image(&conversation.id, &image_id) else { continue };
                        let Some(format) = crate::picture::format_of(&media_type) else { continue };
                        chat.stored_images.insert(image_id.clone());
                        pictures.push(ChatImage { id: image_id, image: Arc::new(Image::from_bytes(format, data)) });
                    }
                    ChatItem::User { text, images: pictures, prompt, files }
                }
                ai::Said::Answer { text } => ChatItem::Answer(text),
                ai::Said::Tool { call, result, failed } => ChatItem::Tool { call, result: Some((result, failed)) },
            });
        }
        chat.known = conversation.transcript.sources;
        chat.compacted = conversation.transcript.compacted.filter(|compacted| compacted.at <= chat.messages.len());
        (chat.id, chat.created_at) = (conversation.id, conversation.created_at);
        chat.kept = true;
        chat.scroll = ScrollHandle::new();
        chat.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Run a library tool off the main thread, on a connection of its own
    /// and with the network, and hand the result back if the same library
    /// is still open.
    pub fn library_tool(&mut self, name: &'static str, args: Value, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, Result<Value, String>, &mut Context<Self>) + 'static) {
        self.library_tool_on(name, args, false, cx, done)
    }

    /// The same, for a run that goes on when its library is left (`works.rs`): with `off_screen`, the result is also
    /// handed back to a library kept at work, in `lib`'s place for the call (`in_library`).
    pub fn library_tool_on(&mut self, name: &'static str, args: Value, off_screen: bool, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, Result<Value, String>, &mut Context<Self>) + 'static) {
        let Some(lib) = self.lib.as_ref() else { return };
        let info = lib.info.clone();
        let library = info.id.clone();
        cx.notify();
        let work = cx.background_executor().spawn(async move {
            let mut store = Store::open(&info.dir).map_err(|e| format!("{e:#}"))?;
            tools::call_in_with(&mut store, &info, name, &args, &wisp_net::get_text, &wisp_net::download, wisp_net::reach())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                if off_screen {
                    this.in_library(&library, cx, |this, cx| done(this, result, cx));
                } else {
                    if this.lib.as_ref().is_none_or(|l| l.info.id != library) { return }
                    done(this, result, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Run one tool call for the assistant on a connection of its own (the
/// window's stays free while a PDF's text is extracted): the call's id,
/// what the model reads, whether it failed, and the line for the window.
fn run_tool(root: &std::path::Path, info: &LibraryInfo, sources: &std::sync::Mutex<Vec<ai::Source>>, call: PreparedCall, allowed: bool) -> (String, String, bool, String) {
    if call.confirm && !allowed {
        return (call.id, "用户没有同意这次操作，它没有执行。不要重试；需要时向用户说明它会做什么，由用户决定。".into(), true, t("没有执行：未获同意").into());
    }
    if let Some(outside) = call.outside {
        let result = match outside {
            crate::custom::Outside::Said(said) => said,
            crate::custom::Outside::Connector(connector, tool) => call.args.and_then(|args| wisp_net::mcp::call(&connector, &tool, &args)),
        };
        return match result {
            Ok(text) => (call.id, text, false, t("完成").into()),
            Err(err) => (call.id, err.clone(), true, err),
        };
    }
    let result = call.args.and_then(|args| {
        // A conversation of the home page lists the shelf, and reads a library on it (`tools::ACROSS`): nothing there is changed.
        if info.draft && call.name == "list_libraries" { return tools::call(root, &call.name, &args, &wisp_net::get_text, &wisp_net::download) }
        if info.draft && let Some(other) = tools::across(root, info, &call.name, &args)? {
            let mut store = Store::open(&other.dir).map_err(|e| format!("{e:#}"))?;
            let mut value = tools::call_in_with(&mut store, &other, &call.name, &args, &wisp_net::get_text, &wisp_net::download, wisp_net::reach())?;
            // The papers it names can be cited: each gets a label of this conversation, which says the library with it.
            let found: Vec<(String, String)> = ai::ids_in(&value).into_iter().filter_map(|id| store.get_paper(&id).ok().map(|paper| (paper.id, paper.title))).collect();
            let mut sources = sources.lock().map_err(|_| t("这次对话的文献列表读不出来。").to_string())?;
            ai::add_sources_of(&mut sources, &other.id, &other.name, &found);
            ai::add_labels_of(&mut value, &sources, &other.id);
            if value.is_object() { value["library"] = serde_json::json!(other.name) }
            return Ok(value);
        }
        let sources = sources.lock().map(|sources| sources.clone()).unwrap_or_default();
        let mut store = Store::open(&info.dir).map_err(|e| format!("{e:#}"))?;
        // As the tool table has it; what is written into a manuscript cites as the conversation does (`compose::call`).
        let mut value = wisp_core::compose::call_with(&mut store, info, &call.name, &args, &sources, &wisp_net::get_text, &wisp_net::download, wisp_net::reach())?;
        ai::add_labels(&mut value, &sources);
        Ok(value)
    });
    let shown = tools::describe_result(&result);
    match result {
        Ok(value) => (call.id, tools::output_text(&value), false, shown),
        Err(err) => (call.id, err, true, shown),
    }
}
