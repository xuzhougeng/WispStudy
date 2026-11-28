//! 翻译 in 写作: one manuscript in several languages, the one it is written in the primary (主稿) and each
//! other a version that follows it (`wisp_core::follow`). It grows out of 译文对照 (`write_facing.rs`): the
//! same languages icon, the same two halves, the primary at the left as the preview draws it. While the
//! language on the right is one the manuscript keeps a version in, the right half is that version, kept and
//! editable block by block (a click on a block edits it in place); in any other language it is 译文对照's
//! look, read only, whose language menu keeps it as a version («保留成译稿»).
//!
//! Following: before each frame the version on show is found against the primary as it now stands
//! (`follow::align`, `settle`) and what changed is saved. A block a model made follows by itself: it is asked
//! for again (`Run`, a run of `works` that goes on when the library is left, one block at a time through
//! `translate_text`, so it is cached as every translation is). A block the person changed keeps their words
//! and says 「原文已改」 with the words that differ, until they choose 重译, 只带改动 (a comparison to take or
//! leave) or 已核对. Nothing in a version ever flows back into the primary.

use crate::app::{Cmd, MenuItem, MenuKind, Workspace};
use crate::i18n::{label, t};
use crate::input::{InputEvent, TextInput};
use crate::manuscripts::READ_SIZE;
use crate::theme::{self, Theme, text_px};
use crate::translate::{Translated, TranslationRun};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use wisp_core::ai;
use wisp_core::bilingual::{self, Kind};
use wisp_core::follow::{self, Kept, Part, State, Unit};
use wisp_core::manuscript::{self, LANGUAGES, Manuscript};
use wisp_core::settings::Task;
use wisp_core::store::translation_key;
use wisp_core::structure::SECTIONS;
use wisp_core::glossary;
use wisp_core::versions;
use wisp_core::translation::{Batch, Job};

/// The page's measures, as 译文对照 has them: the room between the halves, the narrowest half, the page's sides.
const GUTTER: f32 = 48.;
const HALF_MIN: f32 = 300.;
const PAD: f32 = 28.;

#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// Work on this language: the version kept in it, or a new one made and its first pass begun.
    Language(String),
    /// Edit in place the unit at this place among the primary's units (`follow::units`).
    Edit(usize),
    /// Stop editing.
    Done,
    /// 重译: what the person wrote set aside, the unit translated anew.
    Redo(usize),
    /// 只带改动: ask for the translation with only the change of the source carried over.
    Carry(usize),
    TakeCarry(usize),
    LeaveCarry(usize),
    /// 已核对: the translation fits the source as it now stands.
    Checked(usize),
    /// Scroll to the next block with something to do.
    Next,
    /// Ask again for what failed, and go on after a stop.
    Retry,
    Stop,
    /// 设为主稿.
    Primary,
    /// The language no longer followed; what it kept stays for when it is added again.
    Remove,
    /// The version read alone, without the primary beside it.
    Alone(bool),
    /// A translation set aside, to the clipboard.
    CopyAside(i64),
    /// The version on show written out, in its language.
    Export(crate::manuscripts::ManuscriptExport),
    /// The unit edited asked of the models compared (设置's 对照的模型), each giving its version; with fewer than
    /// two chosen, the translation model's three kinds.
    Compare(usize),
    /// One of those versions taken into the unit.
    CompareTake(usize),
    CompareClose,
}

/// A version as the window holds it: what it keeps, found against the primary's units.
pub struct Version {
    pub kept: Vec<Kept>,
    pub units: Vec<Unit>,
    pub pairs: Vec<Option<usize>>,
    /// What the units were when it was last settled.
    signature: u64,
}

impl Version {
    fn kept_at(&self, u: usize) -> Option<&Kept> {
        self.pairs.get(u).copied().flatten().map(|k| &self.kept[k])
    }

    fn state(&self, u: usize) -> State {
        follow::state(self.kept_at(u))
    }

    /// How many units are 未译 and 原文已改.
    fn left(&self) -> (usize, usize) {
        let states: Vec<State> = (0..self.units.len()).map(|u| self.state(u)).collect();
        (states.iter().filter(|s| **s == State::Untranslated).count(), states.iter().filter(|s| **s == State::Changed).count())
    }

    fn aside(&self) -> Vec<&Kept> {
        self.kept.iter().filter(|k| !k.gone.is_empty()).collect()
    }
}

/// The unit being edited in place.
#[derive(Clone, Debug, PartialEq)]
pub struct Editing {
    pub manuscript: String,
    pub language: String,
    pub unit: usize,
    /// What the unit said when the editing began: a click meant for it, not for what stands there since.
    pub sent: String,
}

/// 只带改动 for one kept translation.
#[derive(Clone, Debug)]
pub enum Carry {
    Busy,
    Ready(String),
    Failed(String),
}

/// The versions of one unit from several models (or of one model's three kinds), while it is edited.
pub struct Compared {
    pub unit: usize,
    pub sent: String,
    /// A version a model (`versions::Ask::single`); else the three kinds of one model's answer.
    pub single: bool,
    pub comparison: crate::compare::Comparison,
}

/// A run of translations for one version: its first pass, and what follows by itself after.
pub struct Run {
    pub manuscript: String,
    pub language: String,
    /// Each job's instructions, by its key: a block's carry the glossary's terms it holds.
    pub prompts: HashMap<String, String>,
    pub run: TranslationRun,
}

pub struct Translating {
    /// The languages each manuscript keeps a version in, besides its own (`Store::version_languages`).
    pub versions: HashMap<String, Vec<String>>,
    /// What those versions keep, as read and settled: by manuscript and language.
    pub kept: HashMap<(String, String), Version>,
    pub editing: Option<Editing>,
    pub editor: Entity<TextInput>,
    /// 只带改动, by the id of the kept translation.
    pub carry: HashMap<i64, Carry>,
    pub run: Option<Run>,
    /// The person stopped the run: what follows by itself waits for 继续.
    pub paused: bool,
    /// The version read alone.
    pub alone: bool,
    /// Where each block with something to do was drawn, in the scrolled page: by unit.
    pub tops: Rc<RefCell<BTreeMap<usize, f32>>>,
    /// Several versions of the unit edited, to take one.
    pub compare: Option<Compared>,
    /// Counts the comparisons asked for: an answer to an earlier one is dropped.
    generation: u64,
    loaded: bool,
}

impl Translating {
    pub fn new(subscriptions: &mut Vec<Subscription>, cx: &mut Context<Workspace>) -> Translating {
        let editor = cx.new(|cx| TextInput::new("写下这一段的译文…", cx).multiline(1).markdown());
        subscriptions.push(cx.subscribe(&editor, |this, input, event: &InputEvent, cx| match event {
            InputEvent::Changed => this.translate_typed(input.read(cx).text().to_string(), cx),
            InputEvent::Cancel => this.translate_letgo(cx),
            _ => {}
        }));
        Translating { versions: HashMap::new(), kept: HashMap::new(), editing: None, editor, carry: HashMap::new(), run: None, paused: false, alone: false, tops: Rc::default(), compare: None, generation: 0, loaded: false }
    }

    /// The run as `works` counts it.
    pub(crate) fn run_batch(&self) -> Option<&Batch> {
        self.run.as_ref().map(|run| &run.run.batch)
    }
}

/// A number standing for a text, for element ids made of what a row shows.
fn hashed(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// What the units of a manuscript are, as one number: a version is settled again when it changes.
fn signature(units: &[Unit]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for unit in units { (&unit.section, unit.part.key(), unit.block, &unit.sent, &unit.cites).hash(&mut hasher) }
    hasher.finish()
}

/// A block's Markdown drawn as the preview draws it.
fn page(id: String, text: &str) -> AnyElement {
    crate::markdown::render_page(&id, &manuscript::section_markdown(text, 1), READ_SIZE).into_any_element()
}

/// A text with its citation placeholders as the person reads them while deciding: `[1]`.
fn plainly(text: &str) -> String {
    let mut out = text.to_string();
    for n in follow::placeholders(text) { out = out.replacen(&format!("[@{n}]"), &format!("[{n}]"), 1) }
    out
}

impl Workspace {
    // ---- what is on show

    /// The version the right half shows: the manuscript on show and the language of 译文对照, while the
    /// manuscript keeps a version in it.
    pub(crate) fn translate_shown(&self) -> Option<(String, String)> {
        if !self.write_facing_shown() { return None }
        let lib = self.lib.as_ref()?;
        let manuscript = lib.writing.current()?;
        let language = self.write_facing_language();
        lib.writing.translate.versions.get(&manuscript.id).filter(|list| list.iter().any(|code| code == language)).map(|_| (manuscript.id.clone(), language.to_string()))
    }

    /// The version on show as a manuscript in its language, for 复制全文 and the exports; none while the primary is what is read.
    pub(crate) fn translated_manuscript(&self) -> Option<Manuscript> {
        let (id, language) = self.translate_shown()?;
        let lib = self.lib.as_ref()?;
        let version = lib.writing.translate.kept.get(&(id.clone(), language.clone()))?;
        let manuscript = lib.writing.manuscripts.iter().find(|m| m.id == id)?;
        let mut out = follow::rendered(manuscript, &language, &version.kept, &version.units, &version.pairs);
        // Another id than the primary's: what the store keeps of the primary (its comments, the Word file it came from) is not the version's.
        out.id = format!("{id}@{language}");
        Some(out)
    }

    /// The glossary's pairs from a manuscript's language into `code`, and the manuscript's standing requirement:
    /// what a block's instructions are made with.
    pub(crate) fn translate_rules(&self, id: &str, code: &str) -> (Vec<(String, String)>, String) {
        let Some(lib) = self.lib.as_ref() else { return (vec![], String::new()) };
        let Some(manuscript) = lib.writing.manuscripts.iter().find(|m| m.id == id) else { return (vec![], String::new()) };
        (glossary::pairs(&lib.glossary.terms, &manuscript.language, code), manuscript.requirement.trim().to_string())
    }

    /// The instructions a block is translated with: 译文对照's (`bilingual::prompt_for`), the glossary's terms the
    /// block holds and the manuscript's requirement after them, so a block 译文对照 has read is kept under the same key.
    fn translate_prompt(code: &str, sent: &str, rules: &(Vec<(String, String)>, String)) -> String {
        bilingual::prompt_for(code, sent.trim(), &rules.0, &rules.1)
    }

    /// The key a block's translation is kept under in the library's translations.
    fn translate_key(code: &str, sent: &str, rules: &(Vec<(String, String)>, String)) -> String {
        translation_key(sent.trim(), ai::translation_prompt(&Self::translate_prompt(code, sent, rules)))
    }

    /// The versions read from the library once it is open, and again after a change to them.
    fn translate_versions(&mut self) {
        let Some(lib) = self.lib.as_mut() else { return };
        let translate = &mut lib.writing.translate;
        translate.versions = lib.store.version_languages().unwrap_or_default();
        translate.loaded = true;
    }

    /// The version of a manuscript in a language as it now stands: read from the library the first time, and
    /// found against the primary again whenever its units changed, what changed saved. A unit set aside is said.
    fn translate_settled(&mut self, id: &str, language: &str, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(manuscript) = lib.writing.manuscripts.iter().find(|m| m.id == id) else { return };
        let units = follow::units(manuscript);
        let sign = signature(&units);
        let key = (id.to_string(), language.to_string());
        if lib.writing.translate.kept.get(&key).is_some_and(|version| version.signature == sign) { return }
        let kept = match lib.writing.translate.kept.remove(&key) {
            Some(version) => version.kept,
            None => match lib.store.version_blocks(id, language) {
                Ok(kept) => kept,
                Err(err) => return self.fail(tf!("译稿读不了：{err:#}", "Cannot read the translation: {err:#}"), cx),
            },
        };
        let pairs = follow::align(&kept, &units);
        let mut settled = follow::settle(kept, &units, &pairs, &wisp_core::time::now_iso());
        let saved = if settled.changed { lib.store.save_version(id, language, &mut settled.kept, &settled.dropped) } else { Ok(()) };
        lib.writing.translate.kept.insert(key, Version { kept: settled.kept, units, pairs: settled.pairs, signature: sign });
        if let Err(err) = saved { return self.fail(tf!("译稿没有保存：{err:#}", "Translation not saved: {err:#}"), cx) }
        if settled.set_aside > 0 { self.say(tn!("原文删去或合并了段落：{n} 段改过的译文已搁置，在 ⋯ › 搁置的译文 里", "Paragraphs were removed or merged in the primary: {n} edited translations set aside, under ⋯ › Set aside", settled.set_aside, "translation"), cx) }
    }

    /// Before the page is drawn: the version on show settled, and what follows by itself asked for.
    pub(crate) fn translate_tend(&mut self, cx: &mut Context<Self>) {
        if !self.lib.as_ref().is_some_and(|lib| lib.writing.translate.loaded) { self.translate_versions() }
        let Some((id, language)) = self.translate_shown() else { return };
        self.translate_settled(&id, &language, cx);
        // An edit begun on a unit that has since gone, or another manuscript's, is put away.
        if let Some(lib) = self.lib.as_mut() {
            let translate = &mut lib.writing.translate;
            let stale = translate.editing.as_ref().is_some_and(|editing| (editing.manuscript != id || editing.language != language) || translate.kept.get(&(id.clone(), language.clone())).and_then(|v| v.units.get(editing.unit)).is_none_or(|unit| unit.sent != editing.sent));
            if stale { translate.editing = None }
        }
        self.translate_follow(&id, &language, cx);
    }

    /// The units of a version a model translates by itself (`follow::wanted`), asked for unless a run for it is
    /// going, the person stopped it, or they failed and wait for 重试.
    fn translate_follow(&mut self, id: &str, language: &str, cx: &mut Context<Self>) {
        let rules = self.translate_rules(id, language);
        let Some(lib) = self.lib.as_ref() else { return };
        let translate = &lib.writing.translate;
        if translate.paused || translate.run.as_ref().is_some_and(|run| run.run.batch.running()) { return }
        let Some(version) = translate.kept.get(&(id.to_string(), language.to_string())) else { return };
        let failed = |sent: &str| matches!(lib.translations.get(&Self::translate_key(language, sent, &rules)), Some(Translated::Failed(_)));
        let texts: Vec<String> = follow::wanted(&version.kept, &version.pairs).into_iter().map(|u| version.units[u].sent.clone()).filter(|sent| !failed(sent)).collect();
        if texts.is_empty() { return }
        self.translate_start(id, language, texts, cx);
    }

    /// A run over these texts of a version, as many in flight as 设置 › AI › 任务 says.
    fn translate_start(&mut self, id: &str, language: &str, texts: Vec<String>, cx: &mut Context<Self>) {
        let profile = self.settings.profile_for_or_default(Task::Translate);
        let rules = self.translate_rules(id, language);
        let workers = self.settings.translate_workers.max(1);
        let Some(lib) = self.lib.as_mut() else { return };
        // Each block with its own instructions (the terms it holds): a job a text, those said twice one job.
        let mut batch = Batch::texts(std::iter::empty::<&str>(), "");
        let mut prompts: HashMap<String, String> = HashMap::new();
        for text in texts.iter().map(|text| text.trim()).filter(|text| !text.is_empty()) {
            let prompt = Self::translate_prompt(language, text, &rules);
            let key = translation_key(text, ai::translation_prompt(&prompt));
            if prompts.insert(key.clone(), prompt).is_some() { continue }
            batch.total += 1;
            batch.jobs.push_back(Job { key, text: text.to_string(), papers: 1 });
        }
        let prompt = bilingual::prompt(language);
        lib.writing.translate.run = Some(Run { manuscript: id.to_string(), language: language.to_string(), prompts, run: TranslationRun { batch, model: profile.labelled(), profile, prompt, workers } });
        self.translate_follow_next(cx);
    }

    /// Keep the run's requests in flight, and write what came back into its version. Called when it starts and
    /// whenever a translation comes back (`translate_text`), in the library on show or one left (`in_library`).
    pub(crate) fn translate_follow_next(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(run) = lib.writing.translate.run.as_mut() else { return };
        let settled: Vec<(String, String, Result<String, String>)> = run.run.batch.working.iter().filter_map(|job| match lib.translations.get(&job.key) {
            Some(Translated::Busy) => None,
            Some(Translated::Done(text)) => Some((job.key.clone(), job.text.clone(), Ok(text.clone()))),
            Some(Translated::Failed(error)) => Some((job.key.clone(), job.text.clone(), Err(error.clone()))),
            None => Some((job.key.clone(), job.text.clone(), Err(t("没有取得译文，请重试。").into()))),
        }).collect();
        let (id, language) = (run.manuscript.clone(), run.language.clone());
        for (key, text, result) in settled {
            if let Some(run) = self.lib.as_mut().and_then(|lib| lib.writing.translate.run.as_mut()) { run.run.batch.finish(&key, result.clone().map(|_| ()), false) }
            if let Ok(translated) = result { self.translate_put(&id, &language, &text, &translated, cx) }
        }
        loop {
            let Some(run) = self.lib.as_mut().and_then(|lib| lib.writing.translate.run.as_mut()) else { return };
            if run.run.batch.working.len() >= run.run.workers { break }
            let Some(job) = run.run.batch.take() else { break };
            let (profile, prompt) = (run.run.profile.clone(), run.prompts.get(&job.key).cloned().unwrap_or_else(|| run.run.prompt.clone()));
            self.translate_text(&job.text, &profile, &prompt, true, false, cx);
            let Some(lib) = self.lib.as_mut() else { return };
            let done = match lib.translations.get(&job.key) {
                Some(Translated::Busy) => continue,
                Some(Translated::Done(text)) => Ok(text.clone()),
                Some(Translated::Failed(error)) => Err(error.clone()),
                None => Err(t("没有取得译文，请重试。").to_string()),
            };
            if let Some(run) = lib.writing.translate.run.as_mut() { run.run.batch.finish(&job.key, done.clone().map(|_| ()), true) }
            if let Ok(translated) = done { self.translate_put(&id, &language, &job.text, &translated, cx) }
        }
        cx.notify();
    }

    /// A model's translation of `sent` written into every unit of the version that says it and follows by itself:
    /// one with nothing kept, or one the model made whose source has changed. What the person changed is left alone.
    fn translate_put(&mut self, id: &str, language: &str, sent: &str, translated: &str, cx: &mut Context<Self>) {
        self.translate_settled(id, language, cx);
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(version) = lib.writing.translate.kept.get_mut(&(id.to_string(), language.to_string())) else { return };
        let mut failed = None;
        for u in 0..version.units.len() {
            if version.units[u].sent.trim() != sent.trim() { continue }
            let made = follow::made(&version.units[u], translated);
            let one = match version.pairs[u] {
                Some(k) if version.kept[k].edited || version.kept[k].source == version.kept[k].anchor => continue,
                Some(k) => {
                    let old = &mut version.kept[k];
                    (old.text, old.source, old.cites) = (made.text, made.source, made.cites);
                    old
                }
                None => {
                    version.kept.push(Kept { position: u, ..made });
                    version.pairs[u] = Some(version.kept.len() - 1);
                    version.kept.last_mut().unwrap()
                }
            };
            if let Err(err) = lib.store.save_version_block(id, language, one) { failed = Some(err) }
        }
        if let Some(err) = failed { self.fail(tf!("译文没有保存：{err:#}", "Translation not saved: {err:#}"), cx) }
    }

    /// The unit `at` of the version on show, while it still says what it said when the click was drawn.
    fn translate_unit(&self, at: usize) -> Option<(String, String, Unit, Option<Kept>)> {
        let (id, language) = self.translate_shown()?;
        let version = self.lib.as_ref()?.writing.translate.kept.get(&(id.clone(), language.clone()))?;
        let unit = version.units.get(at)?.clone();
        Some((id, language, unit, version.kept_at(at).cloned()))
    }

    /// A kept translation changed by the window: written into the version on show and saved.
    fn translate_keep(&mut self, at: usize, change: impl FnOnce(&Unit, &mut Kept), cx: &mut Context<Self>) {
        let Some((id, language, unit, kept)) = self.translate_unit(at) else { return };
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(version) = lib.writing.translate.kept.get_mut(&(id.clone(), language.clone())) else { return };
        let mut one = kept.unwrap_or_else(|| Kept { position: at, edited: true, ..follow::made(&unit, "") });
        change(&unit, &mut one);
        let saved = lib.store.save_version_block(&id, &language, &mut one);
        match version.pairs[at] {
            Some(k) => version.kept[k] = one,
            None => {
                version.kept.push(one);
                version.pairs[at] = Some(version.kept.len() - 1);
            }
        }
        if let Err(err) = saved { self.fail(tf!("译文没有保存：{err:#}", "Translation not saved: {err:#}"), cx) }
        cx.notify();
    }

    /// Typing in a block: what is written is kept as it is typed, the person's from now on.
    pub(crate) fn translate_typed(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(editing) = self.lib.as_ref().and_then(|lib| lib.writing.translate.editing.clone()) else { return };
        let Some((_, _, unit, kept)) = self.translate_unit(editing.unit).filter(|(id, language, unit, _)| *id == editing.manuscript && *language == editing.language && unit.sent == editing.sent) else { return };
        let written = follow::placeheld(&text, &unit.cites);
        if kept.as_ref().is_some_and(|kept| kept.text == written) { return }
        self.translate_keep(editing.unit, |_, one| {
            one.text = written;
            one.edited = true;
        }, cx);
    }

    /// Editing put away: Esc, a click elsewhere on the page.
    pub(crate) fn translate_letgo(&mut self, cx: &mut Context<Self>) {
        if let Some(lib) = self.lib.as_mut() && lib.writing.translate.editing.take().is_some() {
            lib.writing.translate.compare = None;
            cx.notify()
        }
    }

    /// When 译文对照 is turned on for a manuscript that keeps versions, the language beside it is one of them.
    pub(crate) fn translate_entered(&mut self, cx: &mut Context<Self>) {
        if !self.lib.as_ref().is_some_and(|lib| lib.writing.translate.loaded) { self.translate_versions() }
        let Some(lib) = self.lib.as_ref() else { return };
        let Some(list) = lib.writing.current().and_then(|m| lib.writing.translate.versions.get(&m.id)).filter(|list| !list.is_empty()) else { return };
        let on = self.write_facing_language();
        if list.iter().any(|code| code == on) { return }
        self.settings.write_facing_language = list[0].clone();
        self.save_settings(cx);
    }

    pub(crate) fn run_translate(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            Do::Language(code) => self.translate_language(code, cx),
            Do::Edit(at) => {
                let Some((id, language, unit, kept)) = self.translate_unit(at) else { return };
                let Some(lib) = self.lib.as_mut() else { return };
                let translate = &mut lib.writing.translate;
                translate.editing = Some(Editing { manuscript: id, language, unit: at, sent: unit.sent.clone() });
                let shown = kept.map(|kept| follow::as_written(&kept.text, &unit.cites)).unwrap_or_default();
                let editor = translate.editor.clone();
                editor.update(cx, |input, cx| {
                    input.set_text(&shown, cx);
                    input.select_range(shown.len()..shown.len(), cx);
                });
                editor.read(cx).focus(window);
            }
            Do::Done => self.translate_letgo(cx),
            Do::Redo(at) => self.translate_redo(at, cx),
            Do::Carry(at) => self.translate_carry(at, cx),
            Do::TakeCarry(at) => {
                let Some((_, _, _, Some(kept))) = self.translate_unit(at) else { return };
                let Some(Carry::Ready(text)) = self.lib.as_mut().and_then(|lib| lib.writing.translate.carry.remove(&kept.id)) else { return };
                self.translate_keep(at, |unit, one| {
                    one.text = text.trim().to_string();
                    one.source = unit.sent.clone();
                }, cx);
            }
            Do::LeaveCarry(at) => {
                let Some((_, _, _, Some(kept))) = self.translate_unit(at) else { return };
                if let Some(lib) = self.lib.as_mut() { lib.writing.translate.carry.remove(&kept.id); }
            }
            Do::Checked(at) => self.translate_keep(at, |unit, one| one.source = unit.sent.clone(), cx),
            Do::Next => self.translate_next(),
            Do::Retry => {
                let Some((id, language)) = self.translate_shown() else { return };
                let rules = self.translate_rules(&id, &language);
                let Some(lib) = self.lib.as_mut() else { return };
                lib.writing.translate.paused = false;
                if let Some(version) = lib.writing.translate.kept.get(&(id, language.clone())) {
                    let keys: Vec<String> = version.units.iter().map(|unit| Self::translate_key(&language, &unit.sent, &rules)).collect();
                    lib.translations.retain(|key, state| !(matches!(state, Translated::Failed(_)) && keys.contains(key)));
                }
            }
            Do::Stop => {
                if let Some(lib) = self.lib.as_mut() {
                    lib.writing.translate.paused = true;
                    if let Some(run) = lib.writing.translate.run.as_mut() { run.run.batch.stop() }
                }
            }
            Do::Primary => self.translate_primary(cx),
            Do::Remove => {
                let Some((id, language)) = self.translate_shown() else { return };
                let Some(lib) = self.lib.as_mut() else { return };
                if let Err(err) = lib.store.remove_version(&id, &language) { return self.fail(format!("{err:#}"), cx) }
                lib.writing.translate.kept.remove(&(id, language.clone()));
                lib.writing.translate.editing = None;
                self.translate_versions();
                let name = label(&LANGUAGES, &language).to_string();
                self.say(tf!("不再跟随{name}译稿；它保留的译文还在，再选「保留成译稿」就回来", "The {name} version is no longer kept in step; what it kept is still there for when you keep it again"), cx);
            }
            Do::Alone(on) => if let Some(lib) = self.lib.as_mut() { lib.writing.translate.alone = on },
            Do::Compare(at) => self.translate_compare(at, cx),
            Do::CompareTake(n) => self.translate_compare_take(n, cx),
            Do::CompareClose => if let Some(lib) = self.lib.as_mut() { lib.writing.translate.compare = None },
            Do::Export(kind) => if let Some(manuscript) = self.translated_manuscript() { self.export_this(manuscript, kind, cx) },
            Do::CopyAside(kept) => {
                let Some((id, language)) = self.translate_shown() else { return };
                let text = self.lib.as_ref().and_then(|lib| lib.writing.translate.kept.get(&(id, language))).and_then(|v| v.kept.iter().find(|k| k.id == kept)).map(|k| plainly(&k.text));
                if let Some(text) = text {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    self.say(t("已复制这段搁置的译文。"), cx);
                }
            }
        }
        cx.notify();
    }

    /// The language to work on: its version, or one made now and its first pass begun. The primary's own is refused.
    fn translate_language(&mut self, code: String, cx: &mut Context<Self>) {
        if !self.lib.as_ref().is_some_and(|lib| lib.writing.translate.loaded) { self.translate_versions() }
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(id) = lib.writing.current().map(|m| m.id.clone()) else { return };
        let had = lib.writing.translate.versions.get(&id).is_some_and(|list| list.contains(&code));
        if !had {
            if let Err(err) = lib.store.add_version(&id, &code) { return self.fail(format!("{err:#}"), cx) }
            lib.writing.translate.paused = false;
            self.translate_versions();
        }
        if self.settings.write_facing_language != code {
            self.settings.write_facing_language = code.clone();
            self.save_settings(cx);
        }
        if let Some(lib) = self.lib.as_mut() {
            lib.writing.facing = true;
            lib.writing.reading = true;
            lib.writing.translate.editing = None;
        }
        if had { return }
        // The first pass: every unit, those 译文对照 already has read from the library's translations at once.
        self.translate_settled(&id, &code, cx);
        self.translate_follow(&id, &code, cx);
        let name = label(&LANGUAGES, &code).to_string();
        self.say(tf!("{name}译稿已建好：正在逐段翻译，离开文献库也会继续", "The {name} version is made: it is being translated paragraph by paragraph, and goes on if you leave the library"), cx);
    }

    /// 重译: what the person wrote is set aside, kept, and the unit is the model's again, translated from the source as it stands.
    fn translate_redo(&mut self, at: usize, cx: &mut Context<Self>) {
        let Some((id, language, unit, Some(kept))) = self.translate_unit(at) else { return };
        let Some(lib) = self.lib.as_mut() else { return };
        if kept.edited {
            let mut aside = Kept { id: 0, gone: wisp_core::time::now_iso(), ..kept.clone() };
            if let Err(err) = lib.store.save_version_block(&id, &language, &mut aside) { return self.fail(format!("{err:#}"), cx) }
            if let Some(version) = lib.writing.translate.kept.get_mut(&(id.clone(), language.clone())) { version.kept.push(aside) }
        }
        lib.writing.translate.carry.remove(&kept.id);
        // The model's from now on, and not from this source: it follows by itself.
        self.translate_keep(at, |_, one| {
            one.edited = false;
            one.source.clear();
        }, cx);
        if let Some(lib) = self.lib.as_mut() { lib.writing.translate.paused = false }
        let busy = self.lib.as_ref().and_then(|lib| lib.writing.translate.run.as_ref()).is_some_and(|run| run.run.batch.running());
        if !busy { self.translate_start(&id, &language, vec![unit.sent], cx) }
    }

    /// 只带改动: the model is given the source then and now and the person's translation, and gives that
    /// translation with the change carried over, for the person to take or leave.
    fn translate_carry(&mut self, at: usize, cx: &mut Context<Self>) {
        let Some((id, language, unit, Some(kept))) = self.translate_unit(at) else { return };
        let profile = self.settings.profile_for_or_default(Task::Translate);
        // The glossary's terms the new source holds and the manuscript's requirement, as in every request of the version.
        let (pairs, requirement) = self.translate_rules(&id, &language);
        let system = glossary::instructions(&follow::carry_prompt(&language), &glossary::occurring(&unit.sent, &pairs), &requirement);
        let request = ai::plain_request(&profile, &system, &follow::carry_question(&kept.source, &unit.sent, &kept.text));
        let Some(lib) = self.lib.as_mut() else { return };
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                lib.writing.translate.carry.insert(kept.id, Carry::Failed(error));
                return cx.notify();
            }
        };
        lib.writing.translate.carry.insert(kept.id, Carry::Busy);
        let (library, protocol, which) = (lib.info.id.clone(), profile.protocol, kept.id);
        let work = cx.background_executor().spawn(async move { wisp_net::complete(request, protocol) });
        cx.spawn(async move |this, cx| {
            let answer = work.await;
            this.update(cx, |this, cx| this.in_library(&library, cx, |this, cx| {
                let Some(lib) = this.lib.as_mut() else { return };
                // Taken back or settled meanwhile.
                if !matches!(lib.writing.translate.carry.get(&which), Some(Carry::Busy)) { return }
                let said = match answer {
                    Ok(text) if !text.trim().is_empty() => Carry::Ready(text.trim().trim_matches('"').trim().to_string()),
                    Ok(_) => Carry::Failed(t("模型没有给出译文。").into()),
                    Err(error) => Carry::Failed(error),
                };
                lib.writing.translate.carry.insert(which, said);
                cx.notify();
            })).ok();
        }).detach();
        cx.notify();
    }

    /// The unit asked of the models compared, the same request for each, so the versions differ only by the model:
    /// the block's words with its citations as written, the paragraphs around it for sense, the glossary's terms it
    /// holds and the manuscript's requirement. With fewer than two models chosen, one request for three kinds.
    fn translate_compare(&mut self, at: usize, cx: &mut Context<Self>) {
        let Some((id, language, unit, _)) = self.translate_unit(at) else { return };
        let Some(lib) = self.lib.as_ref() else { return };
        let Some(manuscript) = lib.writing.manuscripts.iter().find(|m| m.id == id) else { return };
        let context = manuscript.sections.iter().find(|s| s.id == unit.section).filter(|_| unit.part == Part::Text).and_then(|s| bilingual::blocks(&s.body).get(unit.block).map(|block| versions::context(&s.body, block.range.clone()))).unwrap_or_default();
        let primary = manuscript.language.clone();
        let chosen = self.settings.compare_profiles();
        let (single, profiles) = if chosen.len() >= 2 { (true, chosen) } else { (false, vec![self.settings.profile_for_or_default(Task::Translate)]) };
        let (pairs, requirement) = self.translate_rules(&id, &language);
        let ask = versions::Ask { source: ai::language_name(&primary), context, terms: glossary::occurring(&unit.sent, &pairs), requirement, once: String::new(), single };
        let (system, message) = match ai::assist_prompt(&ai::Assist::Translate, &ai::language_name(&language), &follow::as_written(&unit.sent, &unit.cites), &[], &[], &ask) {
            Ok(asked) => asked,
            Err(error) => return self.fail(error, cx),
        };
        let requests = profiles.iter().map(|profile| (ai::plain_request(profile, &system, &message), profile.protocol)).collect();
        let Some(lib) = self.lib.as_mut() else { return };
        let translate = &mut lib.writing.translate;
        translate.generation += 1;
        let generation = translate.generation;
        translate.compare = Some(Compared { unit: at, sent: unit.sent.clone(), single, comparison: crate::compare::Comparison::new(&profiles, generation) });
        let library = lib.info.id.clone();
        self.compare_ask(library, generation, requests, |this| this.lib.as_mut()?.writing.translate.compare.as_mut().map(|compared| &mut compared.comparison), |_| {}, cx);
    }

    /// The versions a comparison has so far: what each says it is (a model, a kind), its words, and whether it is still coming.
    fn translate_compared(compared: &Compared) -> Vec<(String, String, bool, String)> {
        let rows = &compared.comparison.rows;
        if compared.single {
            return rows.iter().enumerate().map(|(n, row)| (row.model.clone(), compared.comparison.version(n).unwrap_or_else(|| row.result.trim().to_string()), row.busy, row.error.clone())).collect();
        }
        let Some(row) = rows.first() else { return vec![] };
        let parsed = versions::parse(&row.result);
        if parsed.is_empty() { return vec![(row.model.clone(), String::new(), row.busy, row.error.clone())] }
        parsed.into_iter().map(|version| (t(version.kind.label()).to_string(), version.text, row.busy, row.error.clone())).collect()
    }

    /// A version taken: the unit's words now, the person's choice (a model does not translate over it).
    fn translate_compare_take(&mut self, n: usize, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(compared) = lib.writing.translate.compare.take() else { return };
        let Some((_, text, _, _)) = Self::translate_compared(&compared).into_iter().nth(n).filter(|(_, text, busy, _)| !*busy && !text.trim().is_empty()) else {
            lib.writing.translate.compare = Some(compared);
            return;
        };
        let at = compared.unit;
        let Some((_, _, unit, _)) = self.translate_unit(at).filter(|(_, _, unit, _)| unit.sent == compared.sent) else { return };
        let written = follow::placeheld(&text, &unit.cites);
        self.translate_keep(at, |unit, one| {
            one.text = written;
            one.source = unit.sent.clone();
            one.edited = true;
        }, cx);
        // The field shows what was taken, still being edited.
        if let Some(lib) = self.lib.as_ref() && lib.writing.translate.editing.as_ref().is_some_and(|editing| editing.unit == at) {
            let editor = lib.writing.translate.editor.clone();
            editor.update(cx, |input, cx| input.set_text(text.trim(), cx));
        }
    }

    /// 设为主稿: the version's words become the manuscript's, the primary's a version of them. Refused while a
    /// unit is 未译 or 原文已改, or lost a citation, and said how many.
    fn translate_primary(&mut self, cx: &mut Context<Self>) {
        let Some((id, language)) = self.translate_shown() else { return };
        self.translate_settled(&id, &language, cx);
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(manuscript) = lib.writing.manuscripts.iter().find(|m| m.id == id) else { return };
        let Some(version) = lib.writing.translate.kept.get(&(id.clone(), language.clone())) else { return };
        let old = manuscript.language.clone();
        let mut swap = match follow::swap(manuscript, &version.kept, &version.units, &version.pairs) {
            Ok(swap) => swap,
            Err(unready) => {
                let mut said = vec![];
                if unready.untranslated > 0 { said.push(tf!("{} 段未译", "{} untranslated", unready.untranslated)) }
                if unready.changed > 0 { said.push(tf!("{} 段原文已改", "{} with a changed source", unready.changed)) }
                if unready.cites > 0 { said.push(tf!("{} 段译文的引用对不上", "{} whose citations do not match", unready.cites)) }
                return self.fail(tf!("还不能设为主稿：{}。先把它们处理完", "Not yet: {}. Settle them first", said.join(t("，"))), cx);
            }
        };
        if let Err(err) = lib.store.swap_primary(&id, &language, &mut swap) { return self.fail(tf!("没有设为主稿：{err:#}", "Not made the primary: {err:#}"), cx) }
        let translate = &mut lib.writing.translate;
        translate.kept.retain(|(m, _), _| *m != id);
        (translate.editing, translate.run) = (None, None);
        translate.carry.clear();
        self.translate_versions();
        // The old primary is the version beside it now: the pairing is the same, its direction turned round.
        self.settings.write_facing_language = old.clone();
        self.save_settings(cx);
        self.write_reload(None, cx);
        let (new, old) = (label(&LANGUAGES, &language).to_string(), label(&LANGUAGES, &old).to_string());
        self.say(tf!("{new}稿已是主稿，{old}稿跟随它；每一节改前的文字留在历史版本里", "The {new} text is now the primary and the {old} one follows it; each section's earlier text is in its history"), cx);
    }

    /// Scroll to the next block with something to do after what is in view, from the top again past the last.
    fn translate_next(&mut self) {
        let Some(lib) = self.lib.as_ref() else { return };
        let tops = lib.writing.translate.tops.borrow().clone();
        let scroll = &lib.citing.read_scroll;
        let seen = -f32::from(scroll.offset().y);
        let Some(top) = tops.values().copied().filter(|top| *top > seen + 90.).fold(None, |a: Option<f32>, b| Some(a.map_or(b, |a| a.min(b)))).or_else(|| tops.values().copied().reduce(f32::min)) else { return };
        scroll.set_offset(point(px(0.), -px((top - 80.).max(0.))));
    }

    // ---- menus

    /// 译文对照's languages, each marked when the manuscript keeps a version in it (保留成译稿 is a button in its head).
    pub(crate) fn translate_facing_menu(&self) -> (Vec<MenuItem>, Vec<usize>) {
        let mut items = self.facing_language_menu();
        let versions = self.lib.as_ref().and_then(|lib| lib.writing.current().and_then(|m| lib.writing.translate.versions.get(&m.id))).cloned().unwrap_or_default();
        for item in &mut items {
            if let Cmd::WriteFacingLanguage(code) = &item.cmd && versions.contains(code) { item.note = Some(t("译稿").into()) }
        }
        (items, vec![])
    }

    /// The versions of the manuscript to work on, and the languages to make one in.
    pub(crate) fn translate_language_menu(&self) -> (Vec<MenuItem>, Vec<usize>) {
        let Some(lib) = self.lib.as_ref() else { return (vec![], vec![]) };
        let Some(manuscript) = lib.writing.current() else { return (vec![], vec![]) };
        let versions = lib.writing.translate.versions.get(&manuscript.id).cloned().unwrap_or_default();
        let on = self.write_facing_language();
        let mut items: Vec<MenuItem> = versions.iter().map(|code| MenuItem { label: format!("{} ({code})", label(&LANGUAGES, code)).into(), checked: code == on, note: Some(t("译稿").into()), cmd: Cmd::Translate(Do::Language(code.clone())) }).collect();
        let breaks = vec![items.len()];
        for (code, name) in LANGUAGES.iter().filter(|(code, _)| *code != manuscript.language && !versions.iter().any(|v| v == code)) {
            items.push(MenuItem { label: tf!("新建{}译稿", "New {} version", t(name)).into(), checked: false, note: None, cmd: Cmd::Translate(Do::Language(code.to_string())) });
        }
        (items, breaks)
    }

    /// In 导出, while a version is read beside the manuscript: the same ways out for the version, in its language.
    pub(crate) fn translate_export_items(&self) -> Vec<MenuItem> {
        use crate::manuscripts::ManuscriptExport;
        let Some((_, language)) = self.translate_shown() else { return vec![] };
        let name = label(&LANGUAGES, &language);
        [("Markdown（.md）", ManuscriptExport::Markdown), ("Word（.docx）", ManuscriptExport::Docx), (t("复制全文"), ManuscriptExport::Copy)]
            .into_iter()
            .map(|(what, kind)| MenuItem { label: tf!("{name}译稿：{what}", "{name} version: {what}").into(), checked: false, note: None, cmd: Cmd::Translate(Do::Export(kind)) })
            .collect()
    }

    /// ⋯ in the head of the version: read it alone, make it the primary, what was set aside, stop following it.
    pub(crate) fn translate_more_menu(&self) -> (Vec<MenuItem>, Vec<usize>) {
        let Some((id, language)) = self.translate_shown() else { return (vec![], vec![]) };
        let Some(lib) = self.lib.as_ref() else { return (vec![], vec![]) };
        let translate = &lib.writing.translate;
        let item = |label: String, cmd: Cmd| MenuItem { label: label.into(), checked: false, note: None, cmd };
        let mut items = vec![MenuItem { label: t("只看译稿").into(), checked: translate.alone, note: None, cmd: Cmd::Translate(Do::Alone(!translate.alone)) }];
        let mut breaks = vec![items.len()];
        items.push(item(t("设为主稿").to_string(), Cmd::Translate(Do::Primary)));
        if let Some(version) = translate.kept.get(&(id, language.clone())) {
            let aside = version.aside();
            if !aside.is_empty() {
                breaks.push(items.len());
                for kept in aside {
                    let text = ui::one_line(&plainly(&kept.text));
                    let short: String = text.chars().take(28).collect();
                    let label = if short.len() < text.len() { format!("{short}…") } else { short };
                    items.push(MenuItem { label: tf!("复制搁置的译文：{label}", "Copy set-aside translation: {label}").into(), checked: false, note: None, cmd: Cmd::Translate(Do::CopyAside(kept.id)) });
                }
            }
        }
        breaks.push(items.len());
        items.push(item(tf!("不再跟随{}译稿", "Stop keeping the {} version", label(&LANGUAGES, &language)), Cmd::Translate(Do::Remove)));
        (items, breaks)
    }

    // ---- the page

    /// What the version on show has to say in the line under the page.
    pub(crate) fn translate_status(&self) -> Option<String> {
        let (id, language) = self.translate_shown()?;
        let version = self.lib.as_ref()?.writing.translate.kept.get(&(id, language.clone()))?;
        let name = label(&LANGUAGES, &language);
        let (untranslated, changed) = version.left();
        Some(if untranslated + changed == 0 { tf!("{name}译稿跟着主稿 · 点一段译文直接改，改过的不会被重译 · 点一节的名称回去写", "The {name} version follows the primary · Click a paragraph's translation to edit it; what you changed is never translated over · Click a section name to write it") } else { tf!("{name}译稿跟着主稿 · 点一段译文直接改 · 标出的段落要你看一下", "The {name} version follows the primary · Click a paragraph's translation to edit it · The marked paragraphs want a look") })
    }

    /// The manuscript at the left as the preview draws it, the version at the right, block beside block at the
    /// same height, each block edited in place; under it in a column too narrow for two halves. `width` is the room.
    pub(crate) fn render_translating(&self, width: f32, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let Some((id, code)) = self.translate_shown() else { return div().into_any_element() };
        let Some(manuscript) = lib.writing.current() else { return div().into_any_element() };
        let translate = &lib.writing.translate;
        let Some(version) = translate.kept.get(&(id.clone(), code.clone())) else { return div().into_any_element() };
        let rules = self.translate_rules(&id, &code);
        let alone = translate.alone;
        let (measure, room) = (f32::from(theme::manuscript_width()), (width - 2. * PAD).max(1.));
        let both = room.min(2. * measure + GUTTER);
        let side = !alone && both >= 2. * HALF_MIN + GUTTER;
        let half = if side { (both - GUTTER) / 2. } else { room.min(measure) };
        let whole = if side { both } else { half };
        let editing = translate.editing.as_ref().filter(|editing| editing.manuscript == id && editing.language == code).map(|editing| editing.unit);
        translate.tops.borrow_mut().clear();
        let at: HashMap<(&str, Part, usize), usize> = version.units.iter().enumerate().map(|(u, unit)| ((unit.section.as_str(), unit.part, unit.block), u)).collect();
        let busy = |sent: &str| lib.translations.get(&Self::translate_key(&code, sent, &rules)).cloned();
        let named = |size: f32, line: f32, text: String| div().text_size(text_px(size)).line_height(text_px(line)).font_weight(FontWeight::SEMIBOLD).child(text);

        // A block of the primary and what stands beside it. The counterpart of the block edited has a line at its side.
        let pair = |left: Option<AnyElement>, right: AnyElement, foot: bool, marked: bool| -> AnyElement {
            let left = left.map(|left| div().relative().child(left).when(marked, |d| d.child(div().absolute().top_0().bottom_0().left(px(-12.)).w(px(2.)).rounded(px(1.)).bg(Theme::accent()))));
            if alone { return div().flex_none().w(px(half)).child(right).into_any_element() }
            if side {
                div().flex().flex_none().gap(px(GUTTER)).when(foot, |d| d.items_end()).when(!foot, |d| d.items_start()).child(div().flex_none().w(px(half)).children(left)).child(div().flex_none().w(px(half)).child(right)).into_any_element()
            } else {
                div().flex().flex_col().flex_none().gap(px(6.)).w(px(half)).children(left).child(div().text_color(Theme::text_secondary()).child(right)).into_any_element()
            }
        };

        // What stands at the right for a unit: the field while it is edited, else its translation drawn (a click
        // edits it), and only where the person has something to do, a mark.
        let right = |u: usize, size: Option<(f32, f32)>, drawn: &dyn Fn(&str) -> AnyElement, cx: &mut Context<Self>| -> AnyElement {
            let unit = &version.units[u];
            let kept = version.kept_at(u);
            let state = follow::state(kept);
            let key = format!("wt-{}-{}", u, hashed(&unit.sent));
            if editing == Some(u) {
                let field = div().relative().w(px(half)).ml(px(-9.)).px(px(8.)).py(px(2.)).rounded(px(6.)).border_1().border_color(Theme::accent().opacity(0.55)).bg(Theme::bg_body()).text_size(text_px(size.map_or(READ_SIZE, |s| s.0))).line_height(size.map_or(theme::body_line(26.), |s| text_px(s.1))).child(translate.editor.clone())
                    // A field a model can fill: a bare sparkle at its side asks the models compared for their versions.
                    .child(div().absolute().top(px(4.)).right(px(-26.)).child(ui::icon_action(SharedString::from(format!("{key}-compare")), "sparkle", Theme::text_tertiary(), t("让对照的几个模型各译一版，挑一版用（设置 › AI 里选对照的模型；只有一个时，给三种译法）")).on_click(self.on(Cmd::Translate(Do::Compare(u)), cx))));
                let compared = translate.compare.as_ref().filter(|compared| compared.unit == u && compared.sent == unit.sent).map(|compared| self.render_translate_compared(compared, &key, half, cx));
                return div().flex().flex_col().gap(px(6.)).child(field).children(compared).into_any_element();
            }
            let wants = matches!(state, State::Untranslated) || (state == State::Changed && kept.is_some_and(|k| k.edited)) || kept.is_some_and(|k| !follow::cites_kept(&unit.sent, &k.text));
            let shown: Option<AnyElement> = kept.filter(|k| !k.text.trim().is_empty()).map(|k| drawn(&k.text));
            let asked = busy(&unit.sent);
            let spinner = |id: String| div().flex().items_center().h(theme::body_line(24.)).child(ui::spinner(SharedString::from(id), 13., Theme::text_tertiary())).into_any_element();
            let mut column = div().flex().flex_col().gap(px(6.));
            let body = div().id(SharedString::from(key.clone())).cursor_text().rounded(px(4.)).hover(|s| s.bg(Theme::hover())).on_click(self.on(Cmd::Translate(Do::Edit(u)), cx));
            column = match (shown, state, &asked) {
                // Being asked for, first or again: what there is, faded, and a spinner.
                (shown, State::Untranslated | State::Changed, Some(Translated::Busy)) if !kept.is_some_and(|k| k.edited) => column.children(shown.map(|shown| div().opacity(0.5).child(shown).into_any_element())).child(spinner(format!("{key}-busy"))),
                (None, State::Untranslated, Some(Translated::Failed(error))) => column.child(div().flex().child(div().id(SharedString::from(format!("{key}-failed"))).text_size(text_px(12.)).line_height(theme::body_line(24.)).text_color(Theme::danger()).tooltip(ui::tip(ui::one_line(error))).child(t("没有译成")))),
                (None, _, _) => column.child(body.child(div().text_size(text_px(13.)).line_height(theme::body_line(24.)).text_color(Theme::text_tertiary()).child(t("未译 · 点这里自己写")))),
                (Some(shown), _, _) => {
                    // Looked over with no model, as 译文对照 does: a term of the glossary not used, a number lost, is a mark at its right.
                    let lacks = kept.map(|k| glossary::check(&unit.sent, &k.text, &glossary::occurring(&unit.sent, &rules.0))).unwrap_or_default();
                    let mark = (!lacks.is_empty()).then(|| div().id(SharedString::from(format!("{key}-lacks"))).absolute().top(px(5.)).right(px(-20.)).child(ui::icon("triangle-alert", 12., Theme::warning())).tooltip(ui::tip(crate::write_assist::lacks_said(&lacks))));
                    column.child(body.relative().child(shown).children(mark))
                }
            };
            // The citations of the source missing from the translation, or others in it.
            if let Some(k) = kept.filter(|k| !k.text.trim().is_empty() && !follow::cites_kept(&unit.sent, &k.text)) {
                let said = tf!("译文的引用和原文对不上：原文 {} 处，译文 {} 处。点译文改一下", "The translation's citations do not match the source's: {} in the source, {} here. Click to edit", follow::placeholders(&unit.sent).len(), follow::placeholders(&k.text).len());
                column = column.child(div().flex().child(div().id(SharedString::from(format!("{key}-cites"))).text_size(text_px(11.)).text_color(Theme::danger()).tooltip(ui::tip(said)).child(t("引用对不上"))));
            }
            // 原文已改 under the person's words: what changed in the source, and the three ways on.
            if let Some(k) = kept.filter(|k| state == State::Changed && k.edited) {
                column = column.child(self.render_translate_changed(u, k, unit, &key, cx));
            }
            if wants {
                let (tops, scroll) = (translate.tops.clone(), lib.citing.read_scroll.clone());
                column = column.relative().child(canvas(|_, _, _| (), move |bounds, _, _, _| {
                    let (view, offset) = (scroll.bounds(), scroll.offset());
                    tops.borrow_mut().insert(u, f32::from(bounds.origin.y - view.origin.y - offset.y));
                }).absolute().top_0().left_0().size_full());
            }
            column.into_any_element()
        };

        let title = at.get(&("", Part::Title, 0)).map(|&u| {
            let drawn = |text: &str| named(24., 32., ui::one_line(&plainly(text))).into_any_element();
            pair(Some(named(24., 32., crate::i18n::manuscript_title(manuscript).to_string()).into_any_element()), right(u, Some((24., 32.)), &drawn, cx), false, editing == Some(u))
        });
        let mut sections: Vec<AnyElement> = vec![];
        for (n, section) in manuscript.sections.iter().enumerate() {
            let labels = lib.citing.read.whole.get(n).map(Vec::as_slice).unwrap_or(&[]);
            let read = |within: std::ops::Range<usize>| wisp_core::cite::read_as(&section.body, labels, within);
            let heading = div()
                .id(("write-translate-heading", n))
                .flex()
                .items_baseline()
                .gap(px(8.))
                .cursor_pointer()
                .on_click(self.on(Cmd::WriteSection(section.id.clone()), cx))
                .child(named(20., 28., section.name.clone()).text_color(Theme::text()).hover(|s| s.text_color(Theme::accent())))
                .when(section.kind != "other", |d| d.child(div().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(label(&SECTIONS, &section.kind).to_string())));
            let mut rows = vec![];
            match at.get(&(section.id.as_str(), Part::Name, 0)) {
                Some(&u) => {
                    let drawn = |text: &str| named(20., 28., ui::one_line(&plainly(text))).into_any_element();
                    rows.push(pair(Some(heading.into_any_element()), right(u, Some((20., 28.)), &drawn, cx), false, editing == Some(u)));
                }
                None => rows.push(pair(Some(heading.into_any_element()), div().into_any_element(), false, false)),
            }
            let blocks = bilingual::blocks(&section.body);
            if blocks.is_empty() { rows.push(pair(Some(div().text_size(text_px(13.)).text_color(Theme::text_tertiary()).child(t("这一节还没有写。")).into_any_element()), div().into_any_element(), false, false)) }
            let pictures = manuscript::picture_lines(&section.body);
            for (b, block) in blocks.iter().enumerate() {
                let id = format!("write-translate-{n}-{b}");
                let unit = at.get(&(section.id.as_str(), Part::Text, b)).copied();
                let restored = |text: &str| bilingual::restored(text, &section.body, block, labels);
                match block.kind {
                    Kind::Picture => {
                        let Some(picture) = pictures.iter().find(|picture| picture.line == block.range) else { continue };
                        let caption = section.body[picture.line.clone()].find(picture.caption.as_str()).filter(|_| !picture.caption.is_empty()).map(|start| read(picture.line.start + start..picture.line.start + start + picture.caption.len()));
                        let shown = manuscript::PictureLine { caption: caption.unwrap_or_else(|| picture.caption.clone()), ..picture.clone() };
                        let drawn = |text: &str| div().w_full().text_size(text_px(13.)).text_color(Theme::text_secondary()).child(crate::markdown::render(&format!("{id}-t"), &restored(text), None, None)).into_any_element();
                        let beside = match unit { Some(u) => right(u, Some((13., 20.)), &drawn, cx), None => div().into_any_element() };
                        rows.push(pair(Some(self.render_manuscript_picture(format!("{id}-p"), &shown, half, window)), beside, true, unit.is_some() && editing == unit));
                    }
                    Kind::Kept => {
                        // Code and rules read the same in any language: beside themselves, and not said twice in one column.
                        let same = page(format!("{id}-t"), &section.body[block.range.clone()]);
                        let original = self.render_read_piece(section, n, b, block.range.clone(), &manuscript::section_markdown(&read(block.range.clone()), 1), true, window, cx);
                        if alone { rows.push(pair(None, same, false, false)) } else if side { rows.push(pair(Some(original), same, false, false)) } else { rows.push(pair(Some(original), div().into_any_element(), false, false)) }
                    }
                    Kind::Words => {
                        let drawn = |text: &str| page(format!("{id}-t"), &restored(text));
                        let beside = match unit { Some(u) => right(u, None, &drawn, cx), None => div().into_any_element() };
                        let original = (!alone).then(|| self.render_read_piece(section, n, b, block.range.clone(), &manuscript::section_markdown(&read(block.range.clone()), 1), true, window, cx));
                        rows.push(pair(original, beside, false, unit.is_some() && editing == unit));
                    }
                }
            }
            sections.push(div().flex().flex_col().flex_none().gap(theme::paragraph_gap(10.)).text_size(text_px(READ_SIZE)).children(rows).into_any_element());
        }
        let references = if alone { None } else { self.render_references_read(cx).map(|list| pair(Some(list), div().into_any_element(), false, false)) };

        let divider = || div().absolute().top_0().bottom_0().left(px(half + GUTTER / 2.)).w(px(1.)).bg(Theme::line());
        let small = |text: &str| div().flex_none().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(text.to_string());
        let head = self.render_translate_head(version, &code, half, cx);
        let primary_name = label(&LANGUAGES, &manuscript.language).to_string();
        let head = div().flex().flex_none().justify_center().px(px(PAD)).child(
            div().relative().flex().flex_none().items_center().gap(px(GUTTER)).w(px(whole)).h(text_px(30.))
                .when(side, |d| d.child(div().flex().flex_none().items_center().gap(px(6.)).w(px(half)).child(small(t("主稿"))).child(small(&primary_name))).child(divider()))
                .child(head),
        );
        // A press on the page outside the field puts the editing away; one in the field is the field's.
        let sheet = div()
            .id("write-translate-view")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&lib.citing.read_scroll)
            .border_b_1()
            .border_color(Theme::line())
            .child(div().flex().justify_center().px(px(PAD)).pb(px(24.)).child(div().relative().flex().flex_col().flex_none().gap(px(30.)).w(px(whole)).pt(px(12.)).text_color(Theme::text()).when(side, |d| d.child(divider())).children(title).children(sections).children(references)));
        div().flex().flex_col().flex_1().min_h_0().child(head).child(sheet).into_any_element()
    }

    /// The versions asked of several models for the unit edited, each with 采用; × puts them away.
    fn render_translate_compared(&self, compared: &Compared, key: &str, half: f32, cx: &mut Context<Self>) -> AnyElement {
        let rows = Self::translate_compared(compared);
        let mut card = div().flex().flex_col().gap(px(8.)).w(px(half)).ml(px(-9.)).px(px(10.)).py(px(8.)).rounded(px(6.)).border_1().border_color(Theme::line()).bg(Theme::bg_sunken());
        card = card.child(div().flex().items_center().child(div().flex_1().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(t(if compared.single { "几个模型各译一版" } else { "三种译法" }))).child(ui::icon_action(SharedString::from(format!("{key}-compare-close")), "x", Theme::text_tertiary(), t("收起")).on_click(self.on(Cmd::Translate(Do::CompareClose), cx))));
        for (n, (said, text, busy, error)) in rows.into_iter().enumerate() {
            let id = compared.comparison.rows.get(if compared.single { n } else { 0 }).map_or(String::new(), |row| row.profile.clone());
            let head = div().flex().items_center().gap(px(6.)).child(div().flex_1().min_w_0().truncate().text_size(text_px(11.)).text_color(Theme::text_secondary()).child(said))
                .when(busy, |d| d.child(ui::spinner(SharedString::from(format!("{key}-compare-{n}-{id}")), 11., Theme::text_tertiary())))
                .when(!busy && !text.trim().is_empty(), |d| d.child(ui::ghost_button(SharedString::from(format!("{key}-take-{n}-{id}")), None, Some(t("采用").into())).h(text_px(22.)).px(px(6.)).text_size(text_px(12.)).on_click(self.on(Cmd::Translate(Do::CompareTake(n)), cx))));
            let words = if !error.is_empty() && text.trim().is_empty() { div().text_size(text_px(12.)).text_color(Theme::danger()).child(ui::one_line(&error)) } else { div().text_size(text_px(13.)).line_height(text_px(20.)).text_color(Theme::text()).child(text) };
            card = card.child(div().flex().flex_col().gap(px(2.)).child(head).child(words));
        }
        card.into_any_element()
    }

    /// 原文已改 under a translation the person made their own: the words of the source that changed, and 重译,
    /// 只带改动 and 已核对; while 只带改动 is asked, a spinner; once it came, the comparison to take or leave.
    fn render_translate_changed(&self, u: usize, kept: &Kept, unit: &Unit, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let small = |id: String, name: &'static str, what: Do, cx: &mut Context<Self>| ui::ghost_button(SharedString::from(id), None, Some(t(name).into())).h(text_px(22.)).px(px(6.)).text_size(text_px(12.)).on_click(self.on(Cmd::Translate(what), cx));
        let note = |text: &'static str| div().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(t(text));
        let (before, after) = (plainly(&kept.source), plainly(&unit.sent));
        let changed: AnyElement = match ai::word_diff(&before, &after) {
            Some((_, now)) => div().text_size(text_px(13.)).line_height(text_px(20.)).text_color(Theme::text_secondary()).child(crate::write_assist::marked(&now)).into_any_element(),
            None => div().text_size(text_px(13.)).line_height(text_px(20.)).text_color(Theme::text_secondary()).child(after.clone()).into_any_element(),
        };
        let card = div().flex().flex_col().gap(px(6.)).px(px(10.)).py(px(8.)).rounded(px(6.)).bg(Theme::note()).border_l_2().border_color(Theme::note_line());
        let card = match lib.writing.translate.carry.get(&kept.id) {
            Some(Carry::Ready(proposed)) => {
                let (now, then) = (plainly(&kept.text), plainly(proposed));
                // The same words back: nothing of the change was missing from the translation.
                let said = if now.trim() == then.trim() { "只带改动：译文不用改，已经合乎改后的原文" } else { "只带改动：只改了下面标出的地方" };
                let compared: AnyElement = match ai::word_diff(&now, &then) {
                    Some((old, new)) => div().flex().flex_col().gap(px(4.))
                        .child(div().text_size(text_px(13.)).line_height(text_px(20.)).text_color(Theme::text_secondary()).child(crate::write_assist::marked(&old)))
                        .child(div().text_size(text_px(13.)).line_height(text_px(20.)).text_color(Theme::text()).child(crate::write_assist::marked(&new)))
                        .into_any_element(),
                    None => div().text_size(text_px(13.)).line_height(text_px(20.)).text_color(Theme::text()).child(then).into_any_element(),
                };
                card.child(note(said)).child(compared).child(div().flex().gap(px(4.)).child(small(format!("{key}-take"), "采用", Do::TakeCarry(u), cx)).child(small(format!("{key}-leave"), "不用", Do::LeaveCarry(u), cx)))
            }
            Some(Carry::Busy) => card.child(note("原文改了：")).child(changed).child(div().flex().items_center().gap(px(6.)).child(ui::spinner(SharedString::from(format!("{key}-carry")), 12., Theme::text_tertiary())).child(note("正在只带改动…"))),
            failed => {
                let error = if let Some(Carry::Failed(error)) = failed { Some(error.clone()) } else { None };
                card.child(note("原文改了："))
                    .child(changed)
                    .children(error.map(|error| div().text_size(text_px(11.)).text_color(Theme::danger()).child(tf!("只带改动没有成：{}", "Carrying the change failed: {}", ui::one_line(&error)))))
                    .child(div().flex().gap(px(4.))
                        .child(small(format!("{key}-redo"), "重译", Do::Redo(u), cx).tooltip(ui::tip(t("按改后的原文重新翻译；你改的译文搁置起来，不会丢"))))
                        .child(small(format!("{key}-carryit"), "只带改动", Do::Carry(u), cx).tooltip(ui::tip(t("让模型只把原文的这处改动带进你的译文，其余不动；先给你对照，再由你决定"))))
                        .child(small(format!("{key}-checked"), "已核对", Do::Checked(u), cx).tooltip(ui::tip(t("译文已经合乎改后的原文：去掉这个标记")))))
            }
        };
        card.into_any_element()
    }

    /// What the version's half says of itself, in its top left corner: what it is, in which language (a menu),
    /// and only when there is any: how many blocks are left and the way to the next, how far a run has come, what failed.
    fn render_translate_head(&self, version: &Version, code: &str, half: f32, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let translate = &lib.writing.translate;
        let small = |text: String| div().flex_none().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(text);
        let language = ui::ghost_button("write-translate-language", None, None).flex_none().h(text_px(22.)).px(px(6.)).gap(px(4.)).text_size(text_px(12.)).child(label(&LANGUAGES, code).to_string()).child(ui::icon("chevron-down", 11., Theme::text_tertiary())).on_click(self.on_menu(MenuKind::TranslateLanguage, cx));
        let (untranslated, changed) = version.left();
        let run = translate.run.as_ref().filter(|run| run.manuscript == lib.writing.manuscript && run.language == code && run.run.batch.running());
        let mut status = div().flex().flex_1().min_w_0().items_center().justify_end().gap(px(6.));
        if let Some(run) = run {
            let batch = &run.run.batch;
            status = status.child(ui::spinner("write-translate-busy", 12., Theme::text_tertiary())).child(small(tf!("正在翻译 {} / {}", "Translating {} / {}", batch.done(), batch.total))).child(ui::icon_action("write-translate-stop", "square", Theme::text_tertiary(), t("停下：已经译好的留着，其余的等你继续")).on_click(self.on(Cmd::Translate(Do::Stop), cx)));
        } else {
            let mut said = vec![];
            if untranslated > 0 { said.push(tf!("未译 {untranslated}", "{untranslated} untranslated")) }
            if changed > 0 { said.push(tf!("原文已改 {changed}", "{changed} source changed")) }
            if !said.is_empty() {
                status = status.child(div().flex_none().text_size(text_px(11.)).text_color(Theme::text_secondary()).child(said.join(" · ")));
                status = status.child(ui::icon_action("write-translate-next", "chevron-down", Theme::text_secondary(), t("下一处要看的段落")).on_click(self.on(Cmd::Translate(Do::Next), cx)));
                // A stop, or what failed: asked again only by the person.
                if translate.paused || untranslated > 0 { status = status.child(ui::ghost_button("write-translate-retry", None, Some(t(if translate.paused { "继续" } else { "重试" }).into())).flex_none().h(text_px(22.)).px(px(6.)).text_size(text_px(12.)).on_click(self.on(Cmd::Translate(Do::Retry), cx))) }
            }
        }
        let more = ui::icon_action("write-translate-more", "ellipsis", Theme::text_tertiary(), t("只看译稿、设为主稿、搁置的译文")).on_click(self.on_menu(MenuKind::TranslateMore, cx));
        div().flex().flex_none().items_center().gap(px(4.)).w(px(half)).child(small(t("译稿").to_string())).child(language).child(status).child(more).into_any_element()
    }

    /// The `wtranslate:` debug steps (`dev.rs`).
    #[cfg(debug_assertions)]
    pub(crate) fn dev_translate(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (step, rest) = arg.split_once(';').unwrap_or((arg, ""));
        let at = || rest.parse::<usize>().unwrap_or(0);
        let what = match step {
            "edit" => Do::Edit(at()),
            "redo" => Do::Redo(at()),
            "carry" => Do::Carry(at()),
            "take" => Do::TakeCarry(at()),
            "leave" => Do::LeaveCarry(at()),
            "check" => Do::Checked(at()),
            "compare" => Do::Compare(at()),
            "pick" => Do::CompareTake(at()),
            "done" => Do::Done,
            "next" => Do::Next,
            "retry" => Do::Retry,
            "stop" => Do::Stop,
            "primary" => Do::Primary,
            "remove" => Do::Remove,
            "alone" => Do::Alone(true),
            "together" => Do::Alone(false),
            "type" => {
                let text = rest.replace('¶', "\n");
                let Some(editor) = self.lib.as_ref().map(|lib| lib.writing.translate.editor.clone()) else { return };
                editor.update(cx, |input, cx| input.set_text(&text, cx));
                return self.translate_typed(text, cx);
            }
            "body" => {
                let (n, text) = rest.split_once(';').unwrap_or((rest, ""));
                let text = text.replace('¶', "\n\n");
                let Some(lib) = self.lib.as_mut() else { return };
                let Some(section) = lib.writing.current().and_then(|m| m.sections.get(n.parse::<usize>().unwrap_or(0))).map(|s| s.id.clone()) else { return };
                if let Err(err) = lib.store.save_section_body(&section, &text) { return self.fail(format!("{err:#}"), cx) }
                let current = lib.writing.section_id == section;
                if let Some(kept) = lib.writing.manuscripts.iter_mut().flat_map(|m| m.sections.iter_mut()).find(|s| s.id == section) { kept.body = text.clone() }
                if current { lib.writing.editor.update(cx, |input, cx| input.set_text(&text, cx)) }
                return cx.notify();
            }
            "dump" => {
                let Some((id, language)) = self.translate_shown() else { return };
                let Some(version) = self.lib.as_ref().and_then(|lib| lib.writing.translate.kept.get(&(id, language))) else { return };
                let units: Vec<serde_json::Value> = version.units.iter().enumerate().map(|(u, unit)| {
                    let kept = version.kept_at(u);
                    serde_json::json!({ "at": u, "part": unit.part.key(), "state": version.state(u).label(), "sent": unit.sent, "source": kept.map(|k| k.source.clone()), "text": kept.map(|k| k.text.clone()), "edited": kept.is_some_and(|k| k.edited) })
                }).collect();
                let aside: Vec<&str> = version.aside().into_iter().map(|k| k.text.as_str()).collect();
                let carry: Vec<String> = self.lib.as_ref().map(|lib| lib.writing.translate.carry.iter().map(|(id, c)| format!("{id}: {c:?}")).collect()).unwrap_or_default();
                let _ = std::fs::write(rest, serde_json::to_string_pretty(&serde_json::json!({ "units": units, "aside": aside, "carry": carry })).unwrap_or_default());
                return;
            }
            code => Do::Language(code.to_string()),
        };
        self.run(Cmd::Translate(what), window, cx);
    }
}
