//! The actions on the selected text of the manuscript, or the paragraph the
//! caret is in. They are of two kinds, and the bar and the card keep them
//! apart, since what comes back is not the same thing:
//!
//! - looking in the library, which changes nothing: 找句子 (the candidates
//!   beside the manuscript, write_find.rs) and 配引用 (the papers each
//!   sentence can cite, write_cite.rs);
//! - a new version of the words: 润色, 换个说法, 译成…, 仿写 (rewritten after
//!   the library's sentences closest to it) and the person's own commands.
//!   The answer streams into a card over the candidates, beside the original
//!   with the words that changed marked; only the person puts it into the
//!   draft, and Undo takes that back. 换个说法 and 译成… give three versions
//!   in one answer (`wisp_core::versions`), each said how it differs; a click
//!   on one replaces the text, and a line under them asks again with a
//!   requirement for this once.
//! - 译成… is told apart by where its versions go, since the primary holds one
//!   language (`Destination`): into the manuscript's own language a click
//!   replaces the text; into a language the manuscript keeps a version (译稿)
//!   in, it goes into that version's sentence that pairs with the text
//!   (write_translate.rs, `Placing`) and the primary is not touched; into any
//!   other a click copies it, and a line under the card keeps a version in
//!   that language. 替换原文 stays a version's corner icon in the last two.
//!
//! Every new version of the words carries the glossary's terms the text holds
//! and the manuscript's standing requirement (glossary.rs), and each version
//! is looked over with no model (`glossary::check`): a term not used as the
//! table says, a number lost, is a quiet mark with the reason on hover.

use crate::i18n::t;
use crate::app::{Cmd, MenuItem, Workspace, Writing};
use crate::input::{InputEvent, TextInput};
use crate::settings::SettingsPage;
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::ops::Range;
use wisp_core::ai::{self, Assist, Change, ChatEvent, Profile};
use wisp_core::glossary::{self, Lack};
use wisp_core::manuscript::LANGUAGES;
use wisp_core::settings::Task;
use wisp_core::versions::{self, Kind, Version};

/// The writing page's request to the model, if one was made.
pub struct Assisting {
    pub card: Option<Card>,
    /// Counts the requests: an answer to an earlier one, stopped or dropped, is not shown.
    pub(crate) generation: u64,
    /// 仿写 is waiting for the sentences closest to its paragraph: the request that asked for them.
    pub(crate) awaiting: Option<u64>,
    /// The line under a card of versions: a requirement for this once; Enter asks again with it.
    pub(crate) once: Entity<TextInput>,
}

/// One action's result, with what it was asked about.
pub struct Card {
    pub assist: Assist,
    pub language: String,
    /// The part of the manuscript the paragraph is in: the card is for that part only.
    pub section: String,
    /// The text sent, and where in the draft it stood when it was sent.
    pub original: String,
    pub range: Range<usize>,
    pub result: String,
    pub busy: bool,
    pub error: String,
    /// Why the answer ended early, or that it is being asked again.
    pub note: String,
    pub model: String,
    /// 仿写: the papers its candidates are from and their keys (paper, key), kept with their papers once a result that cites them is used.
    pub keys: Vec<(String, String)>,
    /// 配引用: what was found in the library, in place of a result to compare (write_cite.rs).
    pub cite: Option<crate::write_cite::Found>,
    /// What the text is in, and for 译成… what it is put into: codes of `manuscript::LANGUAGES`.
    pub source: String,
    pub target: String,
    /// The paragraph around the text, sent for its sense (`versions::context`).
    pub context: String,
    /// The glossary's terms the text holds, as the request had them: what each version is looked over by.
    pub terms: Vec<(String, String)>,
    /// The requirement for this once the answer was asked with.
    pub once: String,
    /// 译成… and 换个说法 asked of several models: their versions, one a model, in place of `result`.
    pub compare: Option<crate::compare::Comparison>,
}

impl Card {
    pub fn new(assist: Assist, language: String, section: String, original: String, range: Range<usize>, model: String) -> Card {
        Card { assist, language, section, original, range, result: String::new(), busy: true, error: String::new(), note: String::new(), model, keys: vec![], cite: None, source: String::new(), target: String::new(), context: String::new(), terms: vec![], once: String::new(), compare: None }
    }

    /// The versions of the answer as far as it has come; one for an action that gives one.
    pub fn versions(&self) -> Vec<Version> {
        // A comparison has a version a model, in the models' order: one that gave none is empty there.
        if let Some(compare) = &self.compare { return (0..compare.rows.len()).map(|n| Version { kind: Kind::Other, text: compare.version(n).unwrap_or_default() }).collect() }
        if self.assist.versions() { return versions::parse(&self.result) }
        let text = self.result.trim();
        if text.is_empty() { vec![] } else { vec![Version { kind: Kind::Other, text: text.to_string() }] }
    }
}

/// What a request asked again keeps of the card before it: the text, where it stood, and what it was put into.
struct Again {
    original: String,
    range: Range<usize>,
    context: String,
    target: String,
    once: String,
}

/// One of the manuscript's tools: a button beside what is selected.
struct Tool {
    id: ElementId,
    icon: Option<&'static str>,
    name: String,
    tip: String,
    able: bool,
    cmd: Cmd,
    /// Which group of the row it is in: 1 looking in the library, 2 a new version of the words, 3 notes.
    group: u8,
}

/// What an action works on: the selection, or the paragraph the caret is in; where it stands and what it says.
pub(crate) fn target(writing: &Writing, cx: &App) -> (Range<usize>, String) {
    let editor = writing.editor.read(cx);
    let (text, selection) = (editor.text(), editor.selection());
    let range = if selection.is_empty() { ai::paragraph_at(text, editor.caret()) } else { trimmed(text, selection) };
    (range.clone(), text[range].to_string())
}

/// A model as a card names it: its name, and what the service calls it when that is something else.
pub(crate) fn model_name(profile: &Profile) -> String {
    let (title, model) = (profile.title(), profile.model.trim());
    if model.is_empty() || title == model { title } else { format!("{title} · {model}") }
}

/// What the person does with a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    Replace,
    Below,
    Copy,
    Drop,
    /// One of several versions, by its place: put where it goes (`Destination`), or copied.
    Take(usize),
    CopyOne(usize),
    /// A version put in the text's place wherever versions of its language go: an English abstract in a Chinese manuscript.
    ReplaceOne(usize),
    /// 保留成…译稿: a version kept in the card's language, which its versions go into from then on.
    Keep,
}

/// Where a version of 译成… goes, by the language it is in: the primary holds one language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    /// The manuscript's own language, and every action but 译成…: in the text's place.
    Replace,
    /// A language the manuscript keeps a version in: into that version's sentence that pairs with the text.
    Version,
    /// Any other: for reading and copying.
    Copy,
}

impl Assisting {
    pub fn new(subscriptions: &mut Vec<Subscription>, window: &mut Window, cx: &mut Context<Workspace>) -> Assisting {
        let once = cx.new(|cx| TextInput::new("再提个要求，回车重写：例如 用被动语态；niche 译作 微环境", cx));
        subscriptions.push(cx.subscribe_in(&once, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::Submit => this.assist_again(cx),
            // What it says may name a term: 记入术语表 is offered while it does.
            InputEvent::Changed => cx.notify(),
            _ => {}
        }));
        Assisting { card: None, generation: 0, awaiting: None, once }
    }

    /// The person went on to another part of the manuscript: the card was about this one.
    pub fn leave(&mut self) {
        self.generation += 1;
        self.awaiting = None;
        self.card = None;
    }
}

impl Workspace {
    /// The language the manuscript is written in, as the prompts name it.
    /// The manuscript's own (`Writing::language`); English while no library is open.
    pub fn manuscript_language(&self) -> String {
        match self.lib.as_ref().map(|lib| lib.writing.language()) {
            None | Some("en") => "English".into(),
            Some(code) => wisp_core::model::label(&wisp_core::manuscript::LANGUAGES, code).to_string(),
        }
    }

    /// Do `assist` on the selection, or on the paragraph the caret is in.
    pub fn assist_start(&mut self, assist: Assist, cx: &mut Context<Self>) {
        // A new request has no requirement for this once yet.
        if let Some(lib) = self.lib.as_ref() { lib.writing.assist.once.update(cx, |input, cx| input.set_text("", cx)) }
        self.assist_send(assist, false, None, cx);
    }

    /// `searched`: the sentences 仿写 asked for have come, and it goes on with them.
    pub(crate) fn assist_begin(&mut self, assist: Assist, searched: bool, cx: &mut Context<Self>) {
        self.assist_send(assist, searched, None, cx);
    }

    /// Ask the card's request again: with the requirement typed under it (Enter there), or into another language.
    pub(crate) fn assist_again(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let once = lib.writing.assist.once.read(cx).text().trim().to_string();
        let Some(card) = lib.writing.assist.card.as_ref().filter(|card| !card.busy && card.assist.versions()) else { return };
        let again = Again { original: card.original.clone(), range: card.range.clone(), context: card.context.clone(), target: card.target.clone(), once };
        self.assist_send(card.assist.clone(), false, Some(again), cx);
    }

    /// 译成… into the language chosen over the card.
    pub(crate) fn assist_retarget(&mut self, code: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let Some(card) = lib.writing.assist.card.as_ref().filter(|card| card.assist == Assist::Translate) else { return };
        let again = Again { original: card.original.clone(), range: card.range.clone(), context: card.context.clone(), target: code, once: card.once.clone() };
        self.assist_send(Assist::Translate, false, Some(again), cx);
    }

    /// The language a text goes into by 译成… unless the person chooses another (`glossary::default_target`).
    pub(crate) fn translate_target(&self, text: &str) -> &'static str {
        let written = self.lib.as_ref().map_or(wisp_core::manuscript::DEFAULT_LANGUAGE, |lib| lib.writing.language());
        glossary::default_target(glossary::text_language(text, written), written)
    }

    /// Where a version of `assist` in `target` goes (`Destination`).
    pub(crate) fn assist_destination(&self, assist: &Assist, target: &str) -> Destination {
        let Some(lib) = self.lib.as_ref() else { return Destination::Replace };
        if *assist != Assist::Translate || target.is_empty() || target == lib.writing.language() { return Destination::Replace }
        let kept = lib.writing.current().and_then(|m| lib.writing.translate.versions.get(&m.id)).is_some_and(|list| list.iter().any(|code| code == target));
        if kept { Destination::Version } else { Destination::Copy }
    }

    /// The models that can be compared, each ticked while chosen; one that cannot be asked says why.
    pub(crate) fn compare_menu(&self) -> Vec<MenuItem> {
        let chosen = &self.settings.compare_models;
        self.settings.profiles().iter().map(|profile| MenuItem { label: model_name(profile).into(), checked: chosen.contains(&profile.id), note: profile.missing().map(|missing| t(missing).into()), cmd: Cmd::CompareModel(profile.id.clone()) }).collect()
    }

    /// A model chosen for comparisons, or no longer: at most `settings::COMPARE_MOST`.
    pub(crate) fn compare_toggle(&mut self, id: String, cx: &mut Context<Self>) {
        let chosen = &mut self.settings.compare_models;
        if let Some(at) = chosen.iter().position(|known| *known == id) {
            chosen.remove(at);
        } else if chosen.len() >= wisp_core::settings::COMPARE_MOST {
            return self.say(tf!("最多同时对照 {} 个模型", "At most {} models are compared at once", wisp_core::settings::COMPARE_MOST), cx);
        } else {
            chosen.push(id);
        }
        self.save_settings(cx);
        cx.notify();
    }

    /// The languages the card's text can be translated into: every one but its own.
    pub(crate) fn assist_target_menu(&self) -> Vec<MenuItem> {
        let Some(card) = self.lib.as_ref().and_then(|lib| lib.writing.assist.card.as_ref()) else { return vec![] };
        LANGUAGES.iter().filter(|(code, _)| *code != card.source).map(|(code, name)| MenuItem { label: format!("{} ({code})", t(name)).into(), checked: *code == card.target, note: None, cmd: Cmd::WriteAssistTarget(code.to_string()) }).collect()
    }

    fn assist_send(&mut self, assist: Assist, searched: bool, again: Option<Again>, cx: &mut Context<Self>) {
        if self.lib.as_ref().is_none_or(|lib| !searched && lib.writing.assist.card.as_ref().is_some_and(|card| card.busy)) { return }
        // Its answer is a list of papers, not a version of the paragraph.
        if assist == Assist::Cite { return self.cite_start(cx) }
        let profile = self.settings.profile_for_or_default(Task::Chat);
        let own = self.settings.commands.clone();
        let compared = self.settings.compare_profiles();
        let (written, target_default) = {
            let lib = self.lib.as_ref().unwrap();
            let (_, original) = target(&lib.writing, cx);
            (lib.writing.language().to_string(), self.translate_target(again.as_ref().map_or(original.as_str(), |again| again.original.as_str())))
        };
        let Some(lib) = self.lib.as_mut() else { return };
        let requirement = lib.requirement();
        let terms_kept = lib.glossary.terms.clone();
        let writing = &mut lib.writing;
        let chosen = again.as_ref().map(|again| again.target.clone()).filter(|target| !target.is_empty());
        let (range, original, context, once) = match again {
            Some(Again { original, range, context, once, .. }) => (range, original, context, once),
            None => {
                let (range, original) = target(writing, cx);
                let context = versions::context(writing.editor.read(cx).text(), range.clone());
                (range, original, context, String::new())
            }
        };
        let source = glossary::text_language(&original, &written).to_string();
        // What the result is in: for 译成… the language chosen over the card, else the one sensible for the text;
        // 换个说法 stays in the text's own; the others write the manuscript's.
        let target = match &assist {
            Assist::Translate => chosen.unwrap_or_else(|| target_default.to_string()),
            Assist::Reword => source.clone(),
            _ => written.clone(),
        };
        let language = ai::language_name(&target);
        writing.assist.generation += 1;
        let generation = writing.assist.generation;
        let mut card = Card::new(assist.clone(), language.clone(), writing.section_id.clone(), original, range, model_name(&profile));
        card.terms = glossary::occurring(&card.original, &glossary::pairs(&terms_kept, &source, &target));
        (card.source, card.target, card.context, card.once) = (source.clone(), target, context, once);
        // 仿写 goes by the library's sentences closest to this paragraph. Those beside the manuscript may be how the papers open the part, or what was found for another paragraph: then they are looked for first.
        if assist == Assist::Rewrite && !searched && !card.original.trim().is_empty() && profile.missing().is_none() {
            let search = &writing.search;
            let for_this = !search.searching && search.asked.as_ref().is_some_and(|asked| !asked.text.is_empty() && (asked.literal || card.original.trim().ends_with(asked.text.as_str())));
            if !for_this {
                card.note = t("正在文献库里找与这段相近的句子…").into();
                writing.assist.card = Some(card);
                writing.side_open = true;
                writing.assist.awaiting = Some(generation);
                return self.write_find(false, cx);
            }
        }
        let sentences = writing.search.candidates.as_ref().and_then(|found| found.as_ref().ok()).and_then(|found| found["sentences"].as_array()).cloned().unwrap_or_default();
        // A paper the rewrite draws on is cited by its key, as everywhere in the manuscript.
        let index = wisp_core::cite::Index::new(&lib.papers, lib.trash.iter().map(|entry| &entry.paper), &lib.citing.kept);
        let examples: Vec<ai::Example> = sentences
            .iter()
            .map(|sentence| {
                let field = |key: &str| sentence[key].as_str().unwrap_or("").to_string();
                ai::Example { text: field("text"), cite: field("cite"), key: index.key(&field("paperId")).unwrap_or("").to_string(), page: sentence["page"].as_u64().map(|page| page as usize), heading: field("heading") }
            })
            .collect();
        if assist == Assist::Rewrite { card.keys = sentences.iter().filter_map(|sentence| sentence["paperId"].as_str()).filter_map(|paper| Some((paper.to_string(), index.key(paper)?.to_string()))).collect() }
        // What the request carries besides the text: the terms it holds, the manuscript's requirement, and for versions the paragraph around it and the requirement for this once.
        let mut ask = versions::Ask { source: source.clone(), context: card.context.clone(), terms: card.terms.clone(), requirement, once: card.once.clone(), single: false };
        // Two or more models chosen for a comparison: each is asked for one version, in the same words (compare.rs).
        if assist.versions() && compared.len() >= 2 {
            ask.single = true;
            let prompt = ai::assist_prompt(&assist, &language, &card.original, &examples, &own, &ask);
            let (system, message) = match prompt {
                Ok(prompt) => prompt,
                Err(error) => {
                    (card.busy, card.error) = (false, error);
                    writing.assist.card = Some(card);
                    writing.side_open = true;
                    return cx.notify();
                }
            };
            let requests: Vec<_> = compared.iter().map(|profile| (ai::plain_request(profile, &system, &message), profile.protocol)).collect();
            card.model = tf!("{} 个模型对照", "{} models compared", compared.len());
            card.compare = Some(crate::compare::Comparison::new(&compared, generation));
            let library = lib.info.id.clone();
            writing.assist.card = Some(card);
            writing.side_open = true;
            let locate: crate::compare::Locate = |this| this.lib.as_mut()?.writing.assist.card.as_mut()?.compare.as_mut();
            // The card is busy while any of its models is.
            let settled = |this: &mut Workspace| {
                if let Some(card) = this.lib.as_mut().and_then(|lib| lib.writing.assist.card.as_mut()) && let Some(compare) = card.compare.as_ref() { card.busy = compare.busy() }
            };
            return self.compare_ask(library, generation, requests, locate, settled, cx);
        }
        let request = match ai::assist_request(&profile, &assist, &language, &card.original, &examples, &own, &ask) {
            Ok(request) => request,
            Err(error) => {
                (card.busy, card.error) = (false, error);
                writing.assist.card = Some(card);
                writing.side_open = true;
                return cx.notify();
            }
        };
        let (section, library, protocol) = (card.section.clone(), lib.info.id.clone(), profile.protocol);
        writing.assist.card = Some(card);
        writing.side_open = true;
        cx.notify();

        let (sender, events) = async_channel::unbounded();
        cx.background_executor()
            .spawn(async move {
                wisp_net::stream_chat(request, protocol, sender);
            })
            .detach();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let alive = this.update(cx, |this, cx| {
                    let Some(lib) = this.lib.as_mut().filter(|l| l.info.id == library) else { return false };
                    let writing = &mut lib.writing;
                    if writing.assist.generation != generation { return false }
                    // The person went on to another part: an answer about this one is not wanted there.
                    if writing.section_id != section {
                        writing.assist.card = None;
                        return false;
                    }
                    let Some(card) = writing.assist.card.as_mut() else { return false };
                    let more = match event {
                        ChatEvent::Text(text) => {
                            card.note.clear();
                            card.result.push_str(&text);
                            true
                        }
                        ChatEvent::Retrying(attempt, why) => {
                            card.note = tf!("模型服务暂时没有回应（{why}），正在重试，第 {attempt} 次…", "The model service has not responded ({why}). Retrying, attempt {attempt}…");
                            true
                        }
                        ChatEvent::Used(_) => true,
                        ChatEvent::Error(error) => {
                            (card.busy, card.error) = (false, error);
                            card.note.clear();
                            false
                        }
                        ChatEvent::Done(turn) => {
                            card.busy = false;
                            card.note = ai::stop_note(&turn.stop).unwrap_or_default().to_string();
                            if card.result.trim().is_empty() && card.error.is_empty() { card.error = t("模型没有返回内容。").into() }
                            false
                        }
                    };
                    cx.notify();
                    more
                });
                if !matches!(alive, Ok(true)) { break }
            }
        })
        .detach();
    }

    /// Stop the request in flight, keeping what arrived.
    pub fn assist_stop(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let assist = &mut lib.writing.assist;
        assist.generation += 1;
        assist.awaiting = None;
        if let Some(card) = assist.card.as_mut().filter(|card| card.busy) {
            card.busy = false;
            if let Some(compare) = card.compare.as_mut() { compare.stop() }
            // What 配引用 had found by then stays, for the person to tick.
            let partial = card.cite.as_ref().map_or(!card.result.trim().is_empty() || card.compare.as_ref().is_some_and(|compare| compare.rows.iter().any(|row| !row.result.trim().is_empty())), |found| !found.rows.is_empty());
            if let Some(found) = card.cite.as_mut() { found.stage = crate::write_cite::Stage::Done }
            card.note = if partial { t("已停止，下面是停止前收到的部分。").into() } else { t("已停止。").into() };
        }
        cx.notify();
    }

    /// Do what the person chose with the result.
    pub fn assist_use(&mut self, how: Use, window: &mut Window, cx: &mut Context<Self>) {
        if how == Use::Keep { return self.assist_keep(cx) }
        let destination = self.lib.as_ref().and_then(|lib| lib.writing.assist.card.as_ref()).map_or(Destination::Replace, |card| self.assist_destination(&card.assist, &card.target));
        let Some(lib) = self.lib.as_mut() else { return };
        let writing = &mut lib.writing;
        if how == Use::Drop {
            writing.assist.generation += 1;
            writing.assist.awaiting = None;
            writing.assist.card = None;
            return cx.notify();
        }
        // A model of a comparison that has answered can be taken while the others still write.
        let ready = |card: &Card| match (&card.compare, how) {
            (Some(compare), Use::Take(n) | Use::CopyOne(n)) => compare.rows.get(n).is_some_and(|row| !row.busy),
            _ => !card.busy,
        };
        let Some(card) = writing.assist.card.as_ref().filter(|card| ready(card) && card.cite.is_none()) else { return };
        // A card of versions has its first taken by 替换原文 and 复制; a version chosen is that one.
        let pick = match how { Use::Take(n) | Use::CopyOne(n) | Use::ReplaceOne(n) => n, _ => 0 };
        let result = card.versions().get(pick).map(|version| version.text.clone()).unwrap_or_default();
        if result.is_empty() { return }
        // A version taken goes where versions of its language go.
        let how = match (how, destination) {
            (Use::Take(_), Destination::Version) => return self.assist_into_version(result, cx),
            (Use::Take(_), Destination::Copy) | (Use::CopyOne(_), _) => Use::Copy,
            (Use::Take(_) | Use::ReplaceOne(_), _) => Use::Replace,
            (other, _) => other,
        };
        if how == Use::Copy {
            cx.write_to_clipboard(ClipboardItem::new_string(result));
            return self.say(t("已复制"), cx);
        }
        // The keys the result cites stay with their papers from now on, as when a paper is cited by hand.
        let pins: Vec<(String, String)> = card.keys.iter().filter(|(_, key)| wisp_core::citefind::cites(&result, key)).cloned().collect();
        // The draft may have been edited meanwhile: the paragraph is found again by what it says.
        let text = writing.editor.read(cx).text().to_string();
        let found = if text.get(card.range.clone()) == Some(card.original.as_str()) { Some(card.range.clone()) } else { text.find(&card.original).map(|at| at..at + card.original.len()) };
        let (changed, why) = (how == Use::Replace && found.is_some(), (card.original.clone(), crate::i18n::assist_title(&card.assist, &card.language)));
        match (how, found) {
            (Use::Replace, Some(range)) => {
                // A translation into a language that spaces its words, put between words of it: the spaces it needs there.
                let spacing = card.assist == Assist::Translate && !matches!(card.target.as_str(), "zh" | "ja");
                let put = if spacing { spaced(&text, range.clone(), &result) } else { result.clone() };
                writing.editor.update(cx, |input, cx| input.replace_range(range, &put, cx))
            }
            (Use::Replace, None) => return self.fail(t("原来那段已经改动，找不到了：可以复制结果，自己放进去。"), cx),
            (_, found) => {
                // Below the paragraph's line; without it, at the end of the draft.
                let end = found.map_or(text.len(), |range| text[range.end..].find('\n').map_or(text.len(), |at| range.end + at));
                let before = if text[..end].trim().is_empty() { "" } else { "\n\n" };
                writing.editor.update(cx, |input, cx| input.replace_range(end..end, &format!("{before}{result}"), cx));
            }
        }
        writing.assist.card = None;
        writing.editor.read(cx).focus(window);
        for (paper, key) in pins {
            if lib.citing.kept.contains_key(&key) { continue }
            if lib.store.keep_cite_key(&paper, &key).is_ok() { lib.citing.kept.insert(key, paper); }
        }
        // In a manuscript that came from a Word file, what the model changed is said on the spot, as a comment.
        if changed { self.comment_on_change(&why.0, &result, &why.1) }
        cx.notify();
    }

    /// 译成… into a language the manuscript keeps a version in: the version taken goes into that version's sentence
    /// that pairs with the text (`Workspace::translate_place`), never into the primary.
    fn assist_into_version(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let writing = &mut lib.writing;
        let Some(card) = writing.assist.card.as_ref().filter(|card| card.section == writing.section_id) else { return };
        let Some(manuscript) = writing.current().map(|m| m.id.clone()) else { return };
        // The draft may have been edited meanwhile: the text is found again by what it says.
        let body = writing.editor.read(cx).text().to_string();
        let found = if body.get(card.range.clone()) == Some(card.original.as_str()) { Some(card.range.clone()) } else { body.find(&card.original).map(|at| at..at + card.original.len()) };
        let blocks = wisp_core::bilingual::blocks(&body);
        let Some((b, block)) = found.as_ref().and_then(|range| blocks.iter().enumerate().find(|(_, block)| block.range.start <= range.start && range.start < block.range.end)) else { return self.fail(t("原来那段已经改动，找不到了：可以复制结果，自己放进去。"), cx) };
        let range = found.unwrap();
        // A selection that runs on past its paragraph has no one place: the paragraph it begins in is opened, for the person to place it.
        let loose = range.end > block.range.end;
        let sel = range.start - block.range.start..range.end.min(block.range.end) - block.range.start;
        let choices: Vec<(String, String)> = card.versions().iter().enumerate().filter(|(_, version)| !version.text.trim().is_empty()).map(|(n, version)| (version_name(card, n, version).to_string(), version.text.clone())).collect();
        let placing = crate::write_translate::Placing { manuscript, language: card.target.clone(), section: card.section.clone(), block: b, sent: block.sent.clone(), source: body[block.range.clone()].to_string(), sel, text, choices, loose };
        writing.assist.generation += 1;
        writing.assist.card = None;
        self.translate_place(placing, false, cx);
        cx.notify();
    }

    /// 保留成…译稿 from a card of 译成… into a language the manuscript keeps no version in: the version made and its
    /// first pass begun without leaving the editor; the card stays, and its versions go into that version from now on.
    fn assist_keep(&mut self, cx: &mut Context<Self>) {
        let Some(code) = self.lib.as_ref().and_then(|lib| lib.writing.assist.card.as_ref()).filter(|card| card.assist == Assist::Translate && !card.busy).map(|card| card.target.clone()) else { return };
        if self.translate_add_version(&code, cx) {
            let name = crate::i18n::label(&LANGUAGES, &code).to_string();
            self.say_with(tf!("{name}译稿已开启，正在逐段翻译：点一个版本放进这一句", "The {name} version is kept and being translated: click a version to put it into this sentence"), t("查看"), Cmd::Translate(crate::write_translate::Do::Language(code)), cx);
        }
        cx.notify();
    }

    /// The tools for what is selected in the manuscript, in groups: what looks in the library and changes nothing,
    /// what proposes a new version of the words, and the person's own notes.
    fn assist_tools(&self, cx: &App) -> Vec<Tool> {
        let lib = self.lib.as_ref().unwrap();
        let language = self.manuscript_language();
        // 译成… names the language the selected text would go into: the other of 中文 and 英文, or the manuscript's
        // language; its tooltip says where the versions go, by that language.
        let into = self.translate_target(&target(&lib.writing, cx).1);
        let version_name = crate::i18n::label(&LANGUAGES, into).to_string();
        let translate_tip = match self.assist_destination(&Assist::Translate, into) {
            Destination::Replace => t("给出三种译法：贴近原文、学术惯用、更简洁，按术语表译术语；点一种替换选中的文字。目标语言可以在结果上方换。").to_string(),
            Destination::Version => tf!("给出三种译法：贴近原文、学术惯用、更简洁，按术语表译术语；点一种放进{version_name}译稿里对应的句子，主稿不动。目标语言可以在结果上方换。", "Three translations: close to the original, academic idiom, more concise, with terms as the glossary has them; click one to put it into the matching sentence of the {version_name} version, the primary unchanged. The target language can be changed over the result."),
            Destination::Copy => tf!("给出三种译法：贴近原文、学术惯用、更简洁，按术语表译术语，供阅读和复制，主稿不动；也可以保留成{version_name}译稿再放进去。目标语言可以在结果上方换。", "Three translations: close to the original, academic idiom, more concise, with terms as the glossary has them, to read and copy, the primary unchanged; or keep a {version_name} version and put one into it. The target language can be changed over the result."),
        };
        let busy = lib.writing.assist.card.as_ref().is_some_and(|card| card.busy);
        let configured = self.settings.profile_for_or_default(Task::Chat).missing().is_none();
        let tool = |id: ElementId, icon: Option<&'static str>, name: String, tip: String, able: bool, cmd: Cmd, group: u8| Tool { id, icon, name, tip, able, cmd, group };
        let key = self.find_key();
        let mut tools = vec![];
        // Looking in the library: nothing of the text changes.
        tools.push(tool("write-bar-find".into(), Some("scan-text"), t("找句子").into(), tf!("在文献库里找与这段相近的句子，列在右边供参考和引用（{key}）。不改你的文字。", "Find sentences in the library similar to this text, listed at the right for reference and citing ({key}). Your text is not changed."), true, Cmd::WriteFind, 1));
        tools.push(tool("write-bar-cite".into(), Some("library"), crate::i18n::assist_title(&Assist::Cite, &language), t("在文献库里逐句找可以引用的文献，列出每篇和它自己的原句。勾选后只把引用加到句末，不改你的文字。").into(), !busy, Cmd::WriteAssist(Assist::Cite), 1));
        // A new version of the words, for the person to take or leave.
        if configured {
            let mut actions = vec![Assist::Polish, Assist::Reword, Assist::Translate, Assist::Rewrite];
            actions.extend(self.settings.commands.iter().filter(|command| !command.template.trim().is_empty()).map(|command| Assist::Own(command.name.clone())));
            for (n, assist) in actions.into_iter().enumerate() {
                let tip = match &assist {
                    Assist::Polish => t("只改语法、用词和不通顺的地方。给出改后的文字并标出改了哪里，由你决定是否替换。").to_string(),
                    Assist::Translate => translate_tip.clone(),
                    Assist::Reword => t("同一种语言换个说法，给出三个版本：贴近原文、学术惯用、更简洁，标出和原文不同的词；点一个替换原文。").to_string(),
                    Assist::Rewrite => t("先在文献库里找与这段相近的句子，再参照它们的结构和措辞改写；意思以你写的为准。由你决定是否替换。").to_string(),
                    Assist::Own(name) => self.settings.commands.iter().find(|command| command.name == *name).map(|command| command.about.clone()).unwrap_or_default(),
                    Assist::Cite => String::new(),
                };
                // The sparkle leads the group.
                let name = if assist == Assist::Translate { crate::i18n::assist_title(&assist, into) } else { crate::i18n::assist_title(&assist, &language) };
                tools.push(tool(("write-assist", n).into(), (n == 0).then_some("sparkle"), name, tip, !busy, Cmd::WriteAssist(assist), 2));
            }
        } else {
            tools.push(tool("write-assist-settings".into(), Some("settings-gear"), t("配置模型").into(), t("配置模型后可以润色、翻译、仿写这一段").into(), true, Cmd::OpenSettings(SettingsPage::Ai), 2));
        }
        tools.push(tool("write-comment".into(), Some("message-square"), t("批注").into(), t("给这段文字写一条批注").into(), true, Cmd::Comment(crate::comments::Do::Write), 3));
        tools
    }

    /// A row of those tools by their names, a line between its groups.
    fn tool_buttons(&self, tools: Vec<Tool>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut items = vec![];
        let mut group = tools.first().map_or(0, |tool| tool.group);
        for tool in tools {
            if tool.group != group { items.push(div().flex_none().w(px(1.)).h(px(14.)).mx(px(4.)).bg(Theme::line()).into_any_element()) }
            group = tool.group;
            let Tool { id, icon, name, tip, able, cmd, .. } = tool;
            items.push(
                ui::ghost_button(id, icon, Some(name.into()))
                    .h(px(30.))
                    .px(px(7.))
                    .text_size(text_px(12.))
                    .when(!able, |d| d.opacity(0.45).cursor_default())
                    .when(able, |d| d.on_click(self.on(cmd, cx)))
                    .when(!tip.is_empty(), |d| d.tooltip(move |_, cx| cx.new(|_| Tip(tip.clone())).into()))
                    .into_any_element(),
            );
        }
        items
    }

    /// The actions for what is selected in the manuscript, in a popover beside it: the only place they are.
    pub fn render_assist_bar(&self, cx: &mut Context<Self>) -> Div {
        let items = self.tool_buttons(self.assist_tools(cx), cx);
        div().flex().flex_none().items_center().flex_wrap().gap(px(2.)).p(px(4.)).text_color(Theme::text()).children(items)
    }

    /// The line the manuscript stands on: what there is to say of the text, and nothing to press. Where the
    /// person writes (`writing`) it is the section's length and that it is saved, at the right end; beside the
    /// conversation that writes, or reading, it is a sentence from the text's left edge.
    pub(crate) fn render_write_foot(&self, writing: bool, status: String) -> Div {
        let row = div().flex().flex_none().items_center().h(text_px(28.)).pb(px(4.)).overflow_hidden();
        let said = div().min_w_0().truncate().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(status);
        if writing { row.px(px(16.)).justify_end().child(said) } else { row.px(px(18.)).child(said.flex_1()) }
    }

    /// What the last action came to, over the candidates; `None` without one for this part.
    /// `width` is the panel's: wrapped text is measured by it, not by what a column would give it.
    pub fn render_assist_card(&self, width: f32, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let lib = self.lib.as_ref().unwrap();
        let card = lib.writing.assist.card.as_ref().filter(|card| card.section == lib.writing.section_id)?;
        let small = |id: &'static str, name: &'static str| ui::ghost_button(id, None, Some(name.into())).h(text_px(24.)).px(px(7.)).text_size(text_px(12.));
        // 配引用 lists papers; every other action shows a version of the words beside the original.
        let (body, actions, working) = match &card.cite {
            Some(found) => ((card.busy || !found.rows.is_empty()).then(|| self.render_cite_body(card, found, cx)), (!card.busy && found.offering()).then(|| self.render_cite_actions(found, cx)), false),
            // Several versions: each one a click to take; under them the line that asks again.
            None if card.assist.versions() => {
                let compared = card.compare.is_some();
                let done = !card.busy && (!card.result.trim().is_empty() || card.versions().iter().any(|version| !version.text.trim().is_empty()));
                ((compared || card.busy || !card.result.is_empty()).then(|| self.render_versions(card, done, cx)), (!card.busy).then(|| self.render_versions_actions(done, width - 44., window, cx)), card.busy && !compared && !card.result.is_empty())
            }
            None => {
                let done = !card.busy && !card.result.trim().is_empty();
                ((card.busy || !card.result.is_empty()).then(|| self.render_assist_compared(card, done)), done.then(|| self.render_assist_actions(cx)), card.busy && !card.result.is_empty())
            }
        };
        // 译成… says where to, and is the menu of where else it can go.
        let title: AnyElement = if card.assist == Assist::Translate && !card.busy {
            ui::ghost_button("write-assist-target", None, None).flex_none().h(text_px(22.)).px(px(4.)).gap(px(4.)).text_size(text_px(12.)).font_weight(FontWeight::MEDIUM).child(crate::i18n::assist_title(&card.assist, &card.language)).child(ui::icon("chevron-down", 11., Theme::text_tertiary())).tooltip(ui::tip(t("换一种目标语言再译"))).on_click(self.on_menu(crate::menus::MenuKind::WriteAssistTarget, cx)).into_any_element()
        } else {
            div().flex_none().text_size(text_px(12.)).font_weight(FontWeight::MEDIUM).child(crate::i18n::assist_title(&card.assist, &card.language)).into_any_element()
        };
        // 译成… says where its versions go, by their language: the primary holds one.
        let destination = self.assist_destination(&card.assist, &card.target);
        let name = crate::i18n::label(&LANGUAGES, &card.target).to_string();
        let whither = (card.assist == Assist::Translate).then(|| {
            let (said, tip) = match destination {
                Destination::Replace => (t("替换选中的文字").to_string(), t("译成稿件本身的语言：点一个版本替换选中的文字").to_string()),
                Destination::Version => (tf!("放进{name}译稿", "Into the {name} version"), tf!("点一个版本，放进{name}译稿里与选中文字对应的句子；主稿不动", "Click a version to put it into the sentence of the {name} version that matches the selection; the primary is not changed")),
                Destination::Copy => (t("只供复制").to_string(), tf!("稿件没有{name}译稿：点一个版本复制，主稿不动", "The manuscript keeps no {name} version: click a version to copy it; the primary is not changed")),
            };
            div().id("write-assist-where").flex_none().min_w_0().truncate().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(said).tooltip(ui::tip(tip))
        });
        let card_el = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width - 20.))
            .gap(px(8.))
            .mx(px(10.))
            .mb(px(10.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_color(Theme::line())
            .bg(Theme::bg_body())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(ui::icon(if card.cite.is_some() { "library" } else { "sparkle" }, 13., Theme::accent()))
                    .child(title)
                    .children(whither)
                    .when(working, |d| d.child(ui::spinner("write-assist-busy", 11., Theme::accent())))
                    .child(div().flex_1())
                    // Who answers versions is chosen where they are read: one model, or several compared (compare.rs).
                    .child(if card.assist.versions() {
                        ui::ghost_button("write-assist-models", None, None).flex_none().min_w_0().max_w(px(width * 0.45)).h(text_px(22.)).px(px(4.)).gap(px(3.)).text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(div().min_w_0().truncate().child(card.model.clone())).child(ui::icon("chevron-down", 10., Theme::text_tertiary())).tooltip(ui::tip(t("选两个以上的模型，就同时请它们各写一版来对照；只选一个时，它一次给出三种写法"))).on_click(self.on_menu(crate::menus::MenuKind::CompareModels, cx)).into_any_element()
                    } else {
                        div().flex_none().min_w_0().max_w(px(width * 0.45)).truncate().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(card.model.clone()).into_any_element()
                    })
                    .when(card.busy, |d| d.child(small("write-assist-stop", t("停止")).on_click(self.on(Cmd::WriteAssistStop, cx))))
                    .when(!card.busy, |d| d.child(ui::ghost_button("write-assist-close", Some("x"), None).size(px(22.)).min_w(px(22.)).on_click(self.on(Cmd::WriteAssistUse(Use::Drop), cx)))),
            )
            // A request refused before it was sent has nothing to show.
            .children(body)
            .when(!card.note.is_empty(), |d| d.child(div().text_size(text_px(11.)).line_height(text_px(16.)).text_color(Theme::text_tertiary()).child(card.note.clone())))
            .when(!card.error.is_empty(), |d| d.child(ui::error_line(t(&card.error))))
            .children(actions);
        Some(card_el.into_any_element())
    }

    /// The original and the result of an action that gives a new version of the words.
    fn render_assist_compared(&self, card: &Card, done: bool) -> Stateful<Div> {
        // Words marked once the answer is in: while it streams the comparison would flicker.
        // A command of the person's own may answer with comments, which are not a version of the paragraph.
        let marks = if done && !matches!(card.assist, Assist::Own(_)) { ai::word_diff(&card.original, card.result.trim()) } else { None };
        let (before, after) = match &marks {
            Some((before, after)) => (marked(before), marked(after)),
            None => (plain(&card.original), plain(card.result.trim())),
        };
        let label = |text: &'static str| div().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(text);
        div()
            .id("write-assist-body")
            .flex()
            .flex_col()
            .gap(px(8.))
            .max_h(px(380.))
            .overflow_y_scroll()
            .child(label(t("原文")))
            .child(div().px(px(10.)).py(px(8.)).rounded(px(8.)).bg(Theme::bg_sunken()).text_size(text_px(12.)).line_height(text_px(19.)).text_color(Theme::text_secondary()).child(before))
            .child(div().flex().items_center().gap(px(6.)).child(label(match card.assist { Assist::Translate => t("译文"), Assist::Own(_) => t("结果"), _ => t("改后") })).children((done && !matches!(card.assist, Assist::Own(_))).then(|| lack_mark("write-assist-lack".into(), card, card.result.trim())).flatten()))
            .child(if card.result.trim().is_empty() && card.busy {
                div().flex().items_center().gap(px(6.)).text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(crate::mascot::inline("write-assist-wait", crate::mascot::Activity::Writing)).child(if card.note.is_empty() { t("正在写…") } else { "" })
            } else {
                div().text_size(text_px(13.)).line_height(text_px(21.)).text_color(Theme::text()).child(after)
            })
    }

    /// The versions of an answer, each with how it differs, what it lacks (a mark), a copy button, and a click that
    /// puts it in the text's place. A rewording has the words that differ from the original marked.
    fn render_versions(&self, card: &Card, done: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let label = |text: &str| div().flex_none().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(text.to_string());
        let shown = card.versions();
        // Where a click puts a version, and what its corner keeps of the rest: 复制 where a click does not copy, 替换原文 where it does not replace.
        let destination = self.assist_destination(&card.assist, &card.target);
        let rows: Vec<AnyElement> = shown
            .iter()
            .enumerate()
            .map(|(n, version)| {
                // A model of a comparison is its own: its name heads its version, and it is ready, waiting or failed by itself.
                let row = card.compare.as_ref().and_then(|compare| compare.rows.get(n));
                let done = row.map_or(done, |row| !row.busy && !version.text.trim().is_empty());
                let marks = if done && card.assist == Assist::Reword { ai::word_diff(&card.original, &version.text) } else { None };
                let words = match &marks {
                    Some((_, after)) => marked(after),
                    None => plain(&version.text),
                };
                let name = version_name(card, n, version);
                let waiting = row.filter(|row| row.busy).map(|_| ui::spinner(SharedString::from(format!("write-version-busy-{n}")), 10., Theme::text_tertiary()));
                let said = row.map(|row| (row.error.clone(), row.note.clone()));
                let group: SharedString = format!("write-version-{n}").into();
                let copy = self.on(Cmd::WriteAssistUse(Use::CopyOne(n)), cx);
                let replace = self.on(Cmd::WriteAssistUse(Use::ReplaceOne(n)), cx);
                div()
                    .id(("write-version", n))
                    .group(group.clone())
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(4.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(Theme::line())
                    .bg(Theme::bg_body())
                    .when(done, |d| d.cursor_pointer().hover(|s| s.border_color(Theme::accent()).bg(Theme::hover())).on_click(self.on(Cmd::WriteAssistUse(Use::Take(n)), cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(text_px(18.))
                            .child(label(name))
                            .children(waiting)
                            .children(done.then(|| lack_mark(SharedString::from(format!("write-version-lack-{n}")), card, &version.text)).flatten())
                            .child(div().flex_1())
                            .when(done, |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .gap(px(2.))
                                        .invisible()
                                        .group_hover(group.clone(), |s| s.visible())
                                        .when(destination != Destination::Copy, |d| {
                                            d.child(ui::icon_action(("write-version-copy", n), "copy", Theme::text_tertiary(), t("复制这一版")).on_click(move |event, window, cx| {
                                                // The version under it is not taken too.
                                                cx.stop_propagation();
                                                copy(event, window, cx)
                                            }))
                                        })
                                        .when(destination != Destination::Replace, |d| {
                                            d.child(ui::icon_action(("write-version-replace", n), "pen-line", Theme::text_tertiary(), t("仍然替换主稿里选中的文字")).on_click(move |event, window, cx| {
                                                cx.stop_propagation();
                                                replace(event, window, cx)
                                            }))
                                        }),
                                )
                            }),
                    )
                    .when(!version.text.is_empty(), |d| d.child(div().text_size(text_px(13.)).line_height(text_px(21.)).text_color(Theme::text()).child(words)))
                    .when_some(said, |d, (error, note)| d.when(!error.is_empty(), |d| d.child(ui::error_line(&error))).when(!note.is_empty(), |d| d.child(div().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(note))))
                    .into_any_element()
            })
            .collect();
        let waiting = (card.busy && shown.is_empty() && card.compare.is_none()).then(|| div().flex().items_center().gap(px(6.)).text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(crate::mascot::inline("write-assist-wait", crate::mascot::Activity::Writing)).child(if card.note.is_empty() { t("正在写…") } else { "" }));
        div()
            .id("write-assist-body")
            .flex()
            .flex_col()
            .gap(px(8.))
            .max_h(px(460.))
            .overflow_y_scroll()
            // What scrolls is one column that keeps its height: the versions are not squeezed into the room.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(8.))
                    .child(label(t("原文")))
                    .child(div().flex_none().px(px(10.)).py(px(8.)).rounded(px(8.)).bg(Theme::bg_sunken()).text_size(text_px(12.)).line_height(text_px(19.)).text_color(Theme::text_secondary()).child(card.original.clone()))
                    .children(waiting)
                    .children(rows),
            )
    }

    /// Under the versions: the line for a requirement this once (Enter asks again), 记入术语表 while it names a
    /// term, and 放弃. `inner` is the card's width inside its padding.
    fn render_versions_actions(&self, done: bool, inner: f32, window: &Window, cx: &mut Context<Self>) -> Div {
        let once = &self.lib.as_ref().unwrap().writing.assist.once;
        let named = glossary::named_term(once.read(cx).text());
        let record = named.map(|(zh, other)| ui::secondary_button("write-assist-record", t("记入术语表")).flex_none().h(text_px(28.)).px(px(8.)).text_size(text_px(12.)).tooltip(ui::tip(tf!("把「{zh}」↔ {other} 加进这个文献库的术语表，以后翻译都照它", "Add “{zh}” ↔ {other} to the library's glossary, for every later translation"))).on_click(self.on(Cmd::WriteAssistRecord, cx)));
        let field_width = if record.is_some() { inner - 96. } else { inner };
        let small = |id: &'static str, name: &'static str| ui::ghost_button(id, None, Some(name.into())).h(text_px(24.)).px(px(7.)).text_size(text_px(12.));
        // What a click on a version does, said where it is done: the destination of its language.
        let card = self.lib.as_ref().unwrap().writing.assist.card.as_ref();
        let (destination, name) = card.map_or((Destination::Replace, String::new()), |card| (self.assist_destination(&card.assist, &card.target), crate::i18n::label(&LANGUAGES, &card.target).to_string()));
        let undo = if cfg!(target_os = "macos") { "⌘Z" } else { "Ctrl+Z" };
        let hint = match destination {
            _ if !done => String::new(),
            Destination::Replace => if cfg!(target_os = "macos") { t("点一个版本替换原文 · ⌘Z 可撤销").to_string() } else { t("点一个版本替换原文 · Ctrl+Z 可撤销").to_string() },
            Destination::Version => tf!("点一个版本放进{name}译稿的这一句 · 主稿不动，在译稿里 {undo} 可撤销", "Click a version to put it into this sentence of the {name} version · the primary is not changed; {undo} in the version undoes it"),
            Destination::Copy => t("点一个版本复制 · 主稿不动").to_string(),
        };
        // No version in the language yet: the quiet way to keep one, so the versions have somewhere to go.
        let keep = (done && destination == Destination::Copy).then(|| ui::ghost_button("write-assist-keep", Some("languages"), Some(tf!("保留成{name}译稿，放进这一句", "Keep a {name} version and put it into this sentence").into())).flex_none().h(text_px(24.)).px(px(6.)).text_size(text_px(12.)).text_color(Theme::text_secondary()).tooltip(ui::tip(tf!("按{name}逐段翻译整篇稿件，跟着主稿走；之后点一个版本，放进它对应的句子", "Translate the whole manuscript into {name}, paragraph by paragraph, following the primary; then click a version to put it into its sentence"))).on_click(self.on(Cmd::WriteAssistUse(Use::Keep), cx)));
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(div().flex().items_center().gap(px(6.)).child(div().flex_none().w(px(field_width)).child(ui::field(once, window, cx))).children(record))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(div().flex_1().min_w_0().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(hint))
                    .child(small("write-assist-drop", t("放弃")).on_click(self.on(Cmd::WriteAssistUse(Use::Drop), cx))),
            )
            .children(keep.map(|keep| div().flex().ml(px(-6.)).child(keep)))
    }

    /// 记入术语表: the term the requirement typed under the card names, in the language of the translation's other side.
    pub(crate) fn assist_record(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let Some((zh, other)) = glossary::named_term(lib.writing.assist.once.read(cx).text()) else { return };
        let card = lib.writing.assist.card.as_ref();
        let language = card.and_then(|card| [card.source.as_str(), card.target.as_str()].into_iter().find(|code| !code.is_empty() && *code != "zh").map(str::to_string)).unwrap_or_else(|| lib.glossary_language().to_string());
        self.glossary_record(zh, other, language, cx);
    }

    /// What can be done with a new version of the words.
    fn render_assist_actions(&self, cx: &mut Context<Self>) -> Div {
        let small = |id: &'static str, name: &'static str| ui::ghost_button(id, None, Some(name.into())).h(text_px(24.)).px(px(7.)).text_size(text_px(12.));
        div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(4.))
            .child(ui::primary_button("write-assist-replace", t("替换原文")).h(text_px(26.)).px(px(10.)).text_size(text_px(12.)).on_click(self.on(Cmd::WriteAssistUse(Use::Replace), cx)))
            .child(ui::secondary_button("write-assist-below", t("插在下面")).h(text_px(26.)).px(px(10.)).text_size(text_px(12.)).on_click(self.on(Cmd::WriteAssistUse(Use::Below), cx)))
            .child(small("write-assist-copy", t("复制")).on_click(self.on(Cmd::WriteAssistUse(Use::Copy), cx)))
            .child(small("write-assist-drop", t("放弃")).on_click(self.on(Cmd::WriteAssistUse(Use::Drop), cx)))
            .child(div().flex_1())
            .child(div().flex_none().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(if cfg!(target_os = "macos") { t("⌘Z 可撤销") } else { t("Ctrl+Z 可撤销") }))
    }
}

/// What a version is called over its words: its model in a comparison, else its kind.
fn version_name<'a>(card: &'a Card, n: usize, version: &Version) -> &'a str {
    match (card.compare.as_ref().and_then(|compare| compare.rows.get(n)), version.kind) {
        (Some(row), _) => row.model.as_str(),
        (None, Kind::Other) => if card.assist == Assist::Translate { t("译文") } else { t("改后") },
        (None, kind) => t(kind.label()),
    }
}

/// A quiet mark where a version lacks what the source has (`glossary::check`), the reasons in its tooltip; none when it lacks nothing.
fn lack_mark(id: SharedString, card: &Card, text: &str) -> Option<AnyElement> {
    let lacks = glossary::check(&card.original, text, &card.terms);
    if lacks.is_empty() { return None }
    Some(div().id(id).flex().flex_none().child(ui::icon("triangle-alert", 12., Theme::warning())).tooltip(ui::tip(lacks_said(&lacks))).into_any_element())
}

/// What a version lacks, said in a sentence.
pub(crate) fn lacks_said(lacks: &[Lack]) -> String {
    let said: Vec<String> = lacks
        .iter()
        .map(|lack| match lack {
            Lack::Term { source, wanted } if source == wanted => tf!("术语「{source}」没有照原样保留", "The term “{source}” was not kept as written"),
            Lack::Term { source, wanted } => tf!("术语表里「{source}」译作 {wanted}，这里没有用", "The glossary translates “{source}” as {wanted}; it is not used here"),
            Lack::Number(number) => tf!("原文里的 {number} 这里没有", "{number} from the original is missing here"),
        })
        .collect();
    said.join(if crate::i18n::is_english() { "; " } else { "；" })
}

/// `put` as it stands in `range` of `text` in a language that spaces its words: a space before it after a word or a
/// full stop, and after it before a word (a Chinese sentence between English ones is written without them).
fn spaced(text: &str, range: Range<usize>, put: &str) -> String {
    let put = put.trim();
    let before = text[..range.start].chars().next_back().is_some_and(|c| !c.is_whitespace() && !"([{“‘\"'（【「".contains(c));
    let after = text[range.end..].chars().next().is_some_and(|c| c.is_alphanumeric() || "([“\"".contains(c));
    format!("{}{put}{}", if before { " " } else { "" }, if after { " " } else { "" })
}

/// A selection without the spaces and line breaks around it.
fn trimmed(text: &str, range: Range<usize>) -> Range<usize> {
    let picked = &text[range.clone()];
    let start = range.start + (picked.len() - picked.trim_start().len());
    start..start + picked.trim().len()
}

fn plain(text: &str) -> StyledText {
    StyledText::new(text.to_string())
}

/// A version of the paragraph with what the other lacks marked: struck out in the original, shaded in the result.
pub(crate) fn marked(pieces: &[(Change, String)]) -> StyledText {
    let (mut text, mut highlights) = (String::new(), vec![]);
    for (n, (change, piece)) in pieces.iter().enumerate() {
        let range = text.len()..text.len() + piece.len();
        text.push_str(piece);
        // A space between two changed words is part of one change, not a gap in it.
        let between = piece.trim().is_empty() && n > 0 && pieces.get(n + 1).is_some_and(|(next, _)| *next != Change::Same && *next == pieces[n - 1].0);
        let change = if between { pieces[n - 1].0 } else { *change };
        let style = match change {
            Change::Same => continue,
            Change::Removed => HighlightStyle { color: Some(Theme::danger()), strikethrough: Some(StrikethroughStyle { thickness: px(1.), color: None }), ..Default::default() },
            Change::Added => HighlightStyle { background_color: Some(Theme::accent_soft()), ..Default::default() },
        };
        // Spaces alone are not worth marking.
        if between || !piece.trim().is_empty() { highlights.push((range, style)) }
    }
    StyledText::new(text).with_highlights(highlights)
}

/// The description of one of the person's commands, over its button.
pub(crate) struct Tip(pub(crate) String);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().max_w(px(280.)).px(px(8.)).py(px(5.)).rounded(px(6.)).border_1().border_color(Theme::line()).bg(Theme::bg_body()).text_size(text_px(12.)).text_color(Theme::text_secondary()).child(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::spaced;

    #[test]
    fn a_translation_between_words_gets_the_spaces_it_needs() {
        let text = "Plants were grown in three soils. 我们每天测量主根的长度。Root hairs were counted.";
        let at = text.find("我们").unwrap();
        let range = at..at + "我们每天测量主根的长度。".len();
        assert_eq!(spaced(text, range, "We measured it daily."), "We measured it daily. ");
        assert_eq!(spaced("a.我们。b", 2..11, " X. "), " X. ");
        assert_eq!(spaced("（我们）", 3..9, "we"), "we");
    }
}
