//! 翻译 in 写作: one manuscript in several languages, the one it is written in the primary (主稿) and each
//! other a version that follows it (`wisp_core::follow`). It grows out of 译文对照 (`write_facing.rs`): the
//! same languages icon, the same two halves, the primary at the left as the preview draws it. While the
//! language on the right is one the manuscript keeps a version in, the right half is that version, kept and
//! editable block by block and sentence by sentence; in any other language it is 译文对照's look, read only,
//! whose language menu keeps it as a version («保留成译稿»).
//!
//! Following: before each frame the version on show is found against the primary as it now stands
//! (`follow::align`, `settle`) and what changed is saved. A block a model made follows by itself: it is asked
//! for again (`Run`, a run of `works` that goes on when the library is left, one block at a time through
//! `translate_text`, so it is cached as every translation is). A block the person changed keeps their words
//! and says 「原文已改」 with the words that differ, until they choose 重译, 只带改动 (a comparison to take or
//! leave) or 已核对. Nothing in a version ever flows back into the primary.

use crate::app::{Cmd, MenuItem, MenuKind, Modal, Workspace};
use crate::i18n::{label, t};
use crate::input::{InputEvent, TextInput};
use crate::manuscripts::{ManuscriptExport, READ_SIZE};
use crate::markdown::Place;
use crate::theme::{self, Theme, text_px};
use crate::translate::{Translated, TranslationRun};
use crate::ui;
use crate::write_facing::{GUTTER, PAD};
use gpui::{prelude::FluentBuilder, *};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use wisp_core::ai;
use wisp_core::bilingual::{self, Kind};
use wisp_core::cite;
use wisp_core::follow::{self, Kept, Part, SentenceAction, State, Unit, Unready};
use wisp_core::glossary;
use wisp_core::manuscript::{self, LANGUAGES, Manuscript};
use wisp_core::preview::Reading;
use wisp_core::settings::Task;
use wisp_core::store::translation_key;
use wisp_core::structure::SECTIONS;
use wisp_core::translation::{Batch, Job};
use wisp_core::versions;

/// Formats a translated version can be exported in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportOut {
    Markdown,
    Docx,
    Copy,
    BilingualMarkdown,
    BilingualDocx,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// One-click entry from the manuscript bar: opens a kept version if one exists, else creates and starts
    /// translating into `write_facing_language()`.
    Open,
    /// Work on this language: the version kept in it, or a new one made and its first pass begun.
    Language(String),
    /// Edit in place the unit at this place among the primary's units (`follow::units`).
    Edit(usize),
    /// Edit in place the unit clicked at a specific rendered `Place` (placing the caret in the sentence clicked).
    EditAt(usize, Option<Place>),
    /// Hover over (or leave) a rendered translated block, highlighting the sentence and its source counterpart.
    Hover(Option<(usize, Place)>),
    /// Stop editing.
    Done,
    /// Confirm the current block as reviewed and jump to edit the next editable block (`⌘Enter`).
    EditNext,
    /// Refine a single sentence (or selected span when `true`) inside unit `usize`.
    Sentence(usize, SentenceAction, bool),
    /// One-click repair of missing or extra citation placeholders (`[@1]`) in unit `usize`.
    FixCites(usize),
    /// 重译: what the person wrote set aside, the unit translated anew.
    Redo(usize),
    /// 只带改动: ask for the translation with only the change of the source carried over.
    Carry(usize),
    TakeCarry(usize),
    LeaveCarry(usize),
    /// 已核对 / 标为已审校: the translation fits the source as it now stands, locked against automatic overwrite.
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
    /// Export the version in `language` (empty = currently shown), checking readiness first for pure exports.
    Export(String, ExportOut),
    /// Export confirmed after the readiness prompt.
    ExportNow(String, ExportOut),
    /// The unit edited asked of the models compared (设置's 对照的模型), each giving its version; with fewer than
    /// two chosen, the translation model's three kinds.
    Compare(usize),
    /// One of those versions taken into the unit (or into the active sentence span).
    CompareTake(usize),
    CompareClose,
    /// The version in this language on show, at the paragraph a version of 译成… was just put into, open for
    /// editing with what was put selected, so Undo there takes it back.
    Look(String),
    /// A version of 译成… whose paragraph has since been translated: put into it now (`Translating::placing`).
    Place,
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

    /// How many units of the sections on show (`within`: a unit's section, "" for the title) are 未译 and 原文已改.
    fn left(&self, within: impl Fn(&str) -> bool) -> (usize, usize) {
        let states: Vec<State> = (0..self.units.len()).filter(|u| within(&self.units[*u].section)).map(|u| self.state(u)).collect();
        (states.iter().filter(|s| **s == State::Untranslated).count(), states.iter().filter(|s| **s == State::Changed).count())
    }

    /// How many units in view are human-reviewed (`State::Edited`), and total units in view.
    fn reviewed(&self, within: impl Fn(&str) -> bool) -> (usize, usize) {
        let indices: Vec<usize> = (0..self.units.len()).filter(|u| within(&self.units[*u].section)).collect();
        let edited = indices.iter().filter(|&&u| self.state(u) == State::Edited).count();
        (edited, indices.len())
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

/// The versions of one unit (or one sentence span within it) from several models or one model's three kinds.
pub struct Compared {
    pub unit: usize,
    pub sent: String,
    /// When `Some((span, title))`, replaces only `span` inside `translate.editor` (single-sentence refinement).
    pub span: Option<(Range<usize>, &'static str)>,
    /// A version a model (`versions::Ask::single`); else the three kinds of one model's answer.
    pub single: bool,
    /// The versions of 译成… brought from the primary, for the person to place: 采用 puts one where the field's
    /// selection is (`Placing`, when where it goes was not sure).
    pub place: bool,
    pub comparison: crate::compare::Comparison,
}

/// A version of 译成… taken for a stretch of the primary, put into the version in its language (write_assist.rs):
/// the block it is in, where in it, and what the card had.
#[derive(Clone, Debug)]
pub struct Placing {
    pub manuscript: String,
    pub language: String,
    pub section: String,
    pub block: usize,
    /// What the block's unit says (`Unit::sent`): a placing for words that have since changed is not made.
    pub sent: String,
    /// The block's words as written, and the stretch of them selected.
    pub source: String,
    pub sel: Range<usize>,
    /// The version taken, and every version the card had (what it is, its words), listed if it is placed by hand.
    pub text: String,
    pub choices: Vec<(String, String)>,
    /// The selection ran on past its paragraph: it has no one place, and the paragraph is opened for the person to place it.
    pub loose: bool,
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
    /// Sentence hovered in a non-edited translated block: `(unit, target_sentence_range_in_written)`.
    pub hovered: Option<(usize, Range<usize>)>,
    /// Requests focus for `editor` on the next frame (after `⌘Enter` advances to the next block).
    pub focus_editor: bool,
    /// 只带改动, by the id of the kept translation.
    pub carry: HashMap<i64, Carry>,
    pub run: Option<Run>,
    /// The person stopped the run: what follows by itself waits for 继续.
    pub paused: bool,
    /// The version read alone.
    pub alone: bool,
    /// Where each block with something to do was drawn, in the scrolled page: by unit.
    pub tops: Rc<RefCell<BTreeMap<usize, f32>>>,
    /// Where every unit in view was drawn, for stepping with `⌘Enter`.
    pub all_tops: Rc<RefCell<BTreeMap<usize, f32>>>,
    /// Several versions of the unit (or sentence) edited, to take one.
    pub compare: Option<Compared>,
    /// A version of 译成… waiting for its paragraph to be translated before it is put into it.
    pub placing: Option<Placing>,
    /// Counts the comparisons asked for: an answer to an earlier one is dropped.
    generation: u64,
    pub(crate) loaded: bool,
}

impl Translating {
    pub fn new(subscriptions: &mut Vec<Subscription>, cx: &mut Context<Workspace>) -> Translating {
        let editor = cx.new(|cx| TextInput::new("写下这一段的译文…", cx).multiline(1).markdown());
        // Observe caret & selection changes so the paired source sentence highlight updates live.
        subscriptions.push(cx.observe(&editor, |_, _, cx| cx.notify()));
        subscriptions.push(cx.subscribe(&editor, |this, input, event: &InputEvent, cx| match event {
            InputEvent::Changed => this.translate_typed(input.read(cx).text().to_string(), cx),
            InputEvent::Submit => this.translate_advance(cx),
            InputEvent::Cancel => this.translate_letgo(cx),
            _ => {}
        }));
        Translating {
            versions: HashMap::new(),
            kept: HashMap::new(),
            editing: None,
            editor,
            hovered: None,
            focus_editor: false,
            carry: HashMap::new(),
            run: None,
            paused: false,
            alone: false,
            tops: Rc::default(),
            all_tops: Rc::default(),
            compare: None,
            placing: None,
            generation: 0,
            loaded: false,
        }
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
    for unit in units {
        (&unit.section, unit.part.key(), unit.block, &unit.sent, &unit.cites).hash(&mut hasher)
    }
    hasher.finish()
}

/// A block's Markdown drawn as the preview draws it.
fn page(id: String, text: &str) -> AnyElement {
    crate::markdown::render_page(&id, &manuscript::section_markdown(text, 1), READ_SIZE).into_any_element()
}

/// A text with its citation placeholders as the person reads them while deciding: `[1]`.
fn plainly(text: &str) -> String {
    let mut out = text.to_string();
    for n in follow::placeholders(text) {
        out = out.replacen(&format!("[@{n}]"), &format!("[{n}]"), 1)
    }
    out
}

/// Expand `text` (`[@1]` placeholders) into `as_written` form while recording where each formatted citation label stands,
/// so `wisp_core::preview::Reading` maps accurately between `written` bytes and `shown` Markdown `Place`s.
fn written_with_labels(text: &str, cites: &[String], cite_labels: &[Option<String>]) -> (String, Vec<(Range<usize>, String)>) {
    let mut out = String::new();
    let mut labels = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("[@") {
        let after = &rest[at + 2..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        match after[..digits].parse::<usize>().ok().filter(|n| *n > 0 && after[digits..].starts_with(']')).and_then(|n| cites.get(n - 1).map(|c| (n - 1, c))) {
            Some((idx, cite)) => {
                out.push_str(&rest[..at]);
                let start = out.len();
                out.push_str(cite);
                let end = out.len();
                if let Some(Some(label)) = cite_labels.get(idx) {
                    labels.push((start..end, label.clone()));
                }
                rest = &after[digits + 1..];
            }
            None => {
                out.push_str(&rest[..at + 2]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    (out, labels)
}

/// Convert a single `Place` `(piece, byte)` in `shown` Markdown into a 1-character `(from, to)` span for `Reading::written`.
fn char_span_at(shown: &str, place: Place) -> Option<(Place, Place)> {
    let pieces = crate::markdown::texts(shown);
    let piece_text = pieces.get(place.0)?;
    if piece_text.is_empty() { return None }
    let clamped = place.1.min(piece_text.len().saturating_sub(1));
    let start = (0..=clamped)
        .rev()
        .find(|&b| piece_text.is_char_boundary(b) && !piece_text[b..].chars().next().is_some_and(char::is_whitespace))
        .or_else(|| (clamped..piece_text.len()).find(|&b| piece_text.is_char_boundary(b) && !piece_text[b..].chars().next().is_some_and(char::is_whitespace)))?;
    let ch = piece_text[start..].chars().next()?;
    Some(((place.0, start), (place.0, start + ch.len_utf8())))
}

/// Format a manuscript's Markdown with CSL citations resolved and a formatted reference list appended when available.
fn formatted_markdown(manuscript: &Manuscript, index: &cite::Index<'_>, style: &cite::Style) -> String {
    let bodies: Vec<&str> = manuscript.sections.iter().map(|s| s.body.as_str()).collect();
    let format = cite::Formatter::new(&bodies, index, style, &manuscript.language);
    let mut formatted = manuscript.clone();
    for section in &mut formatted.sections {
        section.body = format.apply(&section.body, index);
    }
    let mut out = manuscript::markdown(&formatted);
    if !format.references().is_empty() {
        out.push_str(&format!("\n## {}\n\n", format.heading()));
        let entries: Vec<String> = format.references().iter().map(|(_, entry)| entry.text.clone()).collect();
        out.push_str(&entries.join("\n\n"));
        out.push('\n');
    }
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

    /// The byte range in `section.body` (within `within` of `piece`) of the source sentence paired with the
    /// currently edited or hovered sentence on the translation side.
    pub(crate) fn translate_linked_source(&self, section_id: &str, piece: usize, within: Range<usize>, cx: &App) -> Option<Range<usize>> {
        let (id, language) = self.translate_shown()?;
        let lib = self.lib.as_ref()?;
        let translate = &lib.writing.translate;
        let version = translate.kept.get(&(id.clone(), language.clone()))?;
        let manuscript = lib.writing.manuscripts.iter().find(|m| m.id == id)?;
        let section = manuscript.sections.iter().find(|s| s.id == section_id)?;
        let source_block = section.body.get(within.clone())?;
        // 1. Active inline editor in this block: highlight the source sentence corresponding to the caret / selection.
        if let Some(editing) = translate.editing.as_ref().filter(|e| e.manuscript == id && e.language == language) {
            let unit = version.units.get(editing.unit)?;
            if unit.part == Part::Text && unit.section == section_id && unit.block == piece {
                let input = translate.editor.read(cx);
                let link = follow::sentence_pair(source_block, input.text(), input.selection())?;
                return Some(within.start + link.source.start..within.start + link.source.end);
            }
        }
        // 2. Hovered sentence in a rendered translated block (only highlighted when there are multiple sentences, or on hover).
        let (hovered_unit, target_range) = translate.hovered.as_ref()?;
        let unit = version.units.get(*hovered_unit)?;
        if unit.part != Part::Text || unit.section != section_id || unit.block != piece { return None }
        let kept = version.kept_at(*hovered_unit)?;
        let written = follow::as_written(&kept.text, &unit.cites);
        let link = follow::sentence_pair(source_block, &written, target_range.clone())?;
        Some(within.start + link.source.start..within.start + link.source.end)
    }

    fn build_translated_manuscript(&self, id: &str, language: &str) -> Option<Manuscript> {
        let lib = self.lib.as_ref()?;
        let version = lib.writing.translate.kept.get(&(id.to_string(), language.to_string()))?;
        let manuscript = lib.writing.manuscripts.iter().find(|m| m.id == id)?;
        let mut out = follow::rendered(manuscript, language, &version.kept, &version.units, &version.pairs);
        // Another id than the primary's: what the store keeps of the primary (its comments, the Word file it came from) is not the version's.
        out.id = format!("{id}@{language}");
        Some(out)
    }

    fn build_bilingual_manuscript(&self, id: &str, language: &str) -> Option<Manuscript> {
        let lib = self.lib.as_ref()?;
        let version = lib.writing.translate.kept.get(&(id.to_string(), language.to_string()))?;
        let manuscript = lib.writing.manuscripts.iter().find(|m| m.id == id)?;
        let mut out = follow::bilingual_manuscript(manuscript, language, &version.kept, &version.units, &version.pairs);
        out.id = format!("{id}@bilingual-{language}");
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
    pub(crate) fn translate_versions(&mut self) {
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
        if settled.set_aside > 0 {
            self.say(tn!("原文删去或合并了段落：{n} 段改过的译文已搁置，在 ⋯ › 搁置的译文 里", "Paragraphs were removed or merged in the primary: {n} edited translations set aside, under ⋯ › Set aside", settled.set_aside, "translation"), cx)
        }
    }

    /// Before the page is drawn: the version on show settled, focus restored if stepped, and what follows by itself asked for.
    pub(crate) fn translate_tend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.lib.as_ref().is_some_and(|lib| lib.writing.translate.loaded) { self.translate_versions() }
        let Some((id, language)) = self.translate_shown() else { return };
        self.translate_settled(&id, &language, cx);
        // An edit begun on a unit that has since gone, or another manuscript's, is put away.
        if let Some(lib) = self.lib.as_mut() {
            let translate = &mut lib.writing.translate;
            let stale = translate.editing.as_ref().is_some_and(|editing| {
                (editing.manuscript != id || editing.language != language)
                    || translate.kept.get(&(id.clone(), language.clone())).and_then(|v| v.units.get(editing.unit)).is_none_or(|unit| unit.sent != editing.sent)
            });
            if stale {
                translate.editing = None;
                translate.compare = None;
            }
            if std::mem::take(&mut translate.focus_editor) {
                let editor = translate.editor.clone();
                editor.read(cx).focus(window);
            }
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

    /// Keep the run's requests in flight, and write what came back into its version.
    pub(crate) fn translate_follow_next(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(run) = lib.writing.translate.run.as_mut() else { return };
        let settled: Vec<(String, String, Result<String, String>)> = run
            .run
            .batch
            .working
            .iter()
            .filter_map(|job| match lib.translations.get(&job.key) {
                Some(Translated::Busy) => None,
                Some(Translated::Done(text)) => Some((job.key.clone(), job.text.clone(), Ok(text.clone()))),
                Some(Translated::Failed(error)) => Some((job.key.clone(), job.text.clone(), Err(error.clone()))),
                None => Some((job.key.clone(), job.text.clone(), Err(t("没有取得译文，请重试。").into()))),
            })
            .collect();
        let (id, language) = (run.manuscript.clone(), run.language.clone());
        for (key, text, result) in settled {
            if let Some(run) = self.lib.as_mut().and_then(|lib| lib.writing.translate.run.as_mut()) {
                run.run.batch.finish(&key, result.clone().map(|_| ()), false)
            }
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
            if let Some(run) = lib.writing.translate.run.as_mut() {
                run.run.batch.finish(&job.key, done.clone().map(|_| ()), true)
            }
            if let Ok(translated) = done { self.translate_put(&id, &language, &job.text, &translated, cx) }
        }
        cx.notify();
    }

    /// A model's translation of `sent` written into every unit of the version that says it and follows by itself.
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
        // A version of 译成… was waiting for this paragraph.
        let waiting = self.lib.as_ref().and_then(|lib| lib.writing.translate.placing.as_ref()).is_some_and(|p| p.manuscript == id && p.language == language && p.sent.trim() == sent.trim());
        if waiting && let Some(placing) = self.lib.as_mut().and_then(|lib| lib.writing.translate.placing.take()) { self.translate_place(placing, true, cx) }
    }

    /// The unit `at` of the version on show, while it still says what it said when the click was drawn.
    fn translate_unit(&self, at: usize) -> Option<(String, String, Unit, Option<Kept>)> {
        let (id, language) = self.translate_shown()?;
        self.translate_unit_in(&id, &language, at)
    }

    /// The unit `at` of the version of manuscript `id` in `language`, whether or not it is on show.
    fn translate_unit_in(&self, id: &str, language: &str, at: usize) -> Option<(String, String, Unit, Option<Kept>)> {
        let version = self.lib.as_ref()?.writing.translate.kept.get(&(id.to_string(), language.to_string()))?;
        let unit = version.units.get(at)?.clone();
        Some((id.to_string(), language.to_string(), unit, version.kept_at(at).cloned()))
    }

    /// A kept translation changed by the window: written into the version on show and saved.
    fn translate_keep(&mut self, at: usize, change: impl FnOnce(&Unit, &mut Kept), cx: &mut Context<Self>) {
        let Some((id, language)) = self.translate_shown() else { return };
        self.translate_keep_in(&id, &language, at, change, cx)
    }

    /// The same for the version of manuscript `id` in `language`, on show or not (a version of 译成… put into it).
    fn translate_keep_in(&mut self, id: &str, language: &str, at: usize, change: impl FnOnce(&Unit, &mut Kept), cx: &mut Context<Self>) {
        let Some((id, language, unit, kept)) = self.translate_unit_in(id, language, at) else { return };
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

    /// Typing in a block: what is written is kept as it is typed, the person's from now on. The block is the
    /// editing's own, on show or not: a version of 译成… is put into it through the field while the plain editor is on show.
    pub(crate) fn translate_typed(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(editing) = self.lib.as_ref().and_then(|lib| lib.writing.translate.editing.clone()) else { return };
        let Some((_, _, unit, kept)) = self.translate_unit_in(&editing.manuscript, &editing.language, editing.unit).filter(|(_, _, unit, _)| unit.sent == editing.sent) else { return };
        let written = follow::placeheld(&text, &unit.cites);
        if kept.as_ref().is_some_and(|kept| kept.text == written) { return }
        self.translate_keep_in(&editing.manuscript, &editing.language, editing.unit, |unit, one| {
            one.text = written;
            one.source = unit.sent.clone();
            one.edited = true;
        }, cx);
    }

    /// Editing put away: Esc, or a click elsewhere on the page.
    pub(crate) fn translate_letgo(&mut self, cx: &mut Context<Self>) {
        if let Some(lib) = self.lib.as_mut() && lib.writing.translate.editing.take().is_some() {
            lib.writing.translate.compare = None;
            cx.notify()
        }
    }

    /// `⌘Enter` or `下一段`: confirm the current block as reviewed and step into the next editable block in view.
    pub(crate) fn translate_advance(&mut self, cx: &mut Context<Self>) {
        let Some(editing) = self.lib.as_ref().and_then(|lib| lib.writing.translate.editing.clone()) else { return };
        self.translate_keep(editing.unit, |unit, one| {
            one.source = unit.sent.clone();
            one.edited = true;
        }, cx);
        let next_unit = self.lib.as_ref().and_then(|lib| {
            let version = lib.writing.translate.kept.get(&(editing.manuscript.clone(), editing.language.clone()))?;
            ((editing.unit + 1)..version.units.len()).find(|&u| lib.writing.facing_holds(&version.units[u].section))
        });
        let Some(next) = next_unit else {
            return self.translate_letgo(cx);
        };
        let Some((id, language, unit, kept)) = self.translate_unit(next) else { return };
        let Some(lib) = self.lib.as_mut() else { return };
        let top = lib.writing.translate.all_tops.borrow().get(&next).copied();
        if let Some(top) = top {
            lib.citing.read_scroll.set_offset(point(px(0.), -px((top - 96.).max(0.))));
        }
        let translate = &mut lib.writing.translate;
        translate.compare = None;
        translate.hovered = None;
        translate.editing = Some(Editing { manuscript: id, language, unit: next, sent: unit.sent.clone() });
        translate.focus_editor = true;
        let shown = kept.map(|kept| follow::as_written(&kept.text, &unit.cites)).unwrap_or_default();
        let editor = translate.editor.clone();
        editor.update(cx, |input, cx| {
            input.set_text(&shown, cx);
            input.select_range(shown.len()..shown.len(), cx);
        });
        cx.notify();
    }

    /// Open inline editing on unit `at`, placing the caret at the clicked sentence when `place` is given.
    fn translate_edit_at(&mut self, at: usize, place: Option<Place>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, language, unit, kept)) = self.translate_unit(at) else { return };
        let Some(lib) = self.lib.as_ref() else { return };
        let mut caret_pos: Option<usize> = None;
        if let (Some(place), Some(k)) = (place, kept.as_ref())
            && unit.part == Part::Text
            && let Some(manuscript) = lib.writing.current()
            && let Some((n, section)) = manuscript.sections.iter().enumerate().find(|(_, s)| s.id == unit.section)
        {
            let blocks = bilingual::blocks(&section.body);
            if let Some(block) = blocks.get(unit.block) {
                let sec_labels = lib.citing.read.whole.get(n).map(Vec::as_slice).unwrap_or(&[]);
                let cite_labels: Vec<Option<String>> = block.cites.iter().map(|r| sec_labels.iter().find(|(lr, _)| lr == r).map(|(_, l)| l.clone())).collect();
                let (written, tr_labels) = written_with_labels(&k.text, &unit.cites, &cite_labels);
                let shown_md = manuscript::section_markdown(&bilingual::restored(&k.text, &section.body, block, sec_labels), 1);
                let reading = Reading::new(&written, &tr_labels, 0..written.len(), &shown_md);
                if let Some((from, to)) = char_span_at(&shown_md, place) && let Some(range) = reading.written(from, to) {
                    let source_text = &section.body[block.range.clone()];
                    caret_pos = follow::sentence_pair(source_text, &written, range.clone()).map(|link| link.target.end).or(Some(range.end));
                }
            }
        }
        let Some(lib) = self.lib.as_mut() else { return };
        let translate = &mut lib.writing.translate;
        translate.compare = None;
        translate.hovered = None;
        translate.editing = Some(Editing { manuscript: id, language, unit: at, sent: unit.sent.clone() });
        let shown = kept.map(|kept| follow::as_written(&kept.text, &unit.cites)).unwrap_or_default();
        let pos = caret_pos.unwrap_or(shown.len()).min(shown.len());
        let pos = (0..=pos).rev().find(|&b| shown.is_char_boundary(b)).unwrap_or(0);
        let editor = translate.editor.clone();
        editor.update(cx, |input, cx| {
            input.set_text(&shown, cx);
            input.select_range(pos..pos, cx);
        });
        editor.read(cx).focus(window);
    }

    /// Update hovered sentence state when moving over a rendered translated block.
    fn translate_hover(&mut self, hit: Option<(usize, Place)>, cx: &mut Context<Self>) {
        let computed = hit.and_then(|(at, place)| {
            let (_, _, unit, Some(kept)) = self.translate_unit(at)? else { return None };
            if unit.part != Part::Text || kept.text.trim().is_empty() { return None }
            let lib = self.lib.as_ref()?;
            let manuscript = lib.writing.current()?;
            let (n, section) = manuscript.sections.iter().enumerate().find(|(_, s)| s.id == unit.section)?;
            let blocks = bilingual::blocks(&section.body);
            let block = blocks.get(unit.block)?;
            let sec_labels = lib.citing.read.whole.get(n).map(Vec::as_slice).unwrap_or(&[]);
            let cite_labels: Vec<Option<String>> = block.cites.iter().map(|r| sec_labels.iter().find(|(lr, _)| lr == r).map(|(_, l)| l.clone())).collect();
            let (written, tr_labels) = written_with_labels(&kept.text, &unit.cites, &cite_labels);
            let shown_md = manuscript::section_markdown(&bilingual::restored(&kept.text, &section.body, block, sec_labels), 1);
            let reading = Reading::new(&written, &tr_labels, 0..written.len(), &shown_md);
            let (from, to) = char_span_at(&shown_md, place)?;
            let range = reading.written(from, to)?;
            let link = follow::sentence_pair(&section.body[block.range.clone()], &written, range)?;
            Some((at, link.target))
        });
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.writing.translate.hovered != computed {
            lib.writing.translate.hovered = computed;
            cx.notify();
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
        if let Do::Hover(hit) = what {
            return self.translate_hover(hit, cx);
        }
        match what {
            Do::Hover(_) => unreachable!(),
            Do::Open => {
                if !self.lib.as_ref().is_some_and(|lib| lib.writing.translate.loaded) { self.translate_versions() }
                let target = self
                    .lib
                    .as_ref()
                    .and_then(|lib| {
                        let m = lib.writing.current()?;
                        let list = lib.writing.translate.versions.get(&m.id)?;
                        let wanted = self.write_facing_language();
                        if list.iter().any(|c| c == wanted) {
                            Some(wanted.to_string())
                        } else {
                            list.first().filter(|c| *c != &m.language).cloned()
                        }
                    })
                    .unwrap_or_else(|| self.write_facing_language().to_string());
                self.translate_language(target, cx);
            }
            Do::Language(code) => self.translate_language(code, cx),
            Do::Edit(at) => self.translate_edit_at(at, None, window, cx),
            Do::EditAt(at, place) => self.translate_edit_at(at, place, window, cx),
            Do::Done => self.translate_letgo(cx),
            Do::EditNext => {
                self.translate_advance(cx);
                if let Some(lib) = self.lib.as_mut() && std::mem::take(&mut lib.writing.translate.focus_editor) {
                    let editor = lib.writing.translate.editor.clone();
                    editor.read(cx).focus(window);
                }
            }
            Do::Sentence(at, action, from_sel) => self.translate_sentence(at, action, from_sel, cx),
            Do::FixCites(at) => {
                let Some((_, _, unit, Some(kept))) = self.translate_unit(at) else { return };
                let fixed = follow::fix_cites(&unit.sent, &kept.text);
                self.translate_keep(at, |unit, one| {
                    one.text = fixed.clone();
                    one.source = unit.sent.clone();
                    one.edited = true;
                }, cx);
                if let Some(lib) = self.lib.as_ref() && lib.writing.translate.editing.as_ref().is_some_and(|e| e.unit == at) {
                    let shown = follow::as_written(&fixed, &unit.cites);
                    let editor = lib.writing.translate.editor.clone();
                    editor.update(cx, |input, cx| input.set_text(&shown, cx));
                }
                self.say(t("已补齐这一段的引用标记。"), cx);
            }
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
            Do::Checked(at) => self.translate_keep(at, |unit, one| {
                one.source = unit.sent.clone();
                one.edited = true;
            }, cx),
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
                lib.writing.translate.hovered = None;
                self.translate_versions();
                let name = label(&LANGUAGES, &language).to_string();
                self.say(tf!("不再跟随{name}译稿；它保留的译文还在，再打开就回来", "The {name} version is no longer kept in step; what it kept is still there for when you open it again"), cx);
            }
            Do::Alone(on) => if let Some(lib) = self.lib.as_mut() { lib.writing.translate.alone = on },
            Do::Compare(at) => self.translate_compare(at, cx),
            Do::CompareTake(n) => self.translate_compare_take(n, window, cx),
            Do::CompareClose => if let Some(lib) = self.lib.as_mut() { lib.writing.translate.compare = None },
            Do::Look(language) => self.translate_look(language, cx),
            Do::Place => {
                if let Some(placing) = self.lib.as_mut().and_then(|lib| lib.writing.translate.placing.take()) { self.translate_place(placing, false, cx) }
            }
            Do::Export(lang, out) => self.translate_ask_export(lang, out, cx),
            Do::ExportNow(lang, out) => {
                self.modal = None;
                self.translate_export_now(lang, out, cx);
            }
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

    /// A version kept in `code` for the manuscript being written and its first pass begun, without putting it on show
    /// (保留成…译稿 from the card of 译成…). `false` when it could not be made.
    pub(crate) fn translate_add_version(&mut self, code: &str, cx: &mut Context<Self>) -> bool {
        if !self.lib.as_ref().is_some_and(|lib| lib.writing.translate.loaded) { self.translate_versions() }
        let Some(lib) = self.lib.as_mut() else { return false };
        let Some(id) = lib.writing.current().filter(|m| m.language != code).map(|m| m.id.clone()) else { return false };
        if !lib.writing.translate.versions.get(&id).is_some_and(|list| list.iter().any(|known| known == code)) {
            if let Err(err) = lib.store.add_version(&id, code) {
                self.fail(format!("{err:#}"), cx);
                return false;
            }
            lib.writing.translate.paused = false;
            self.translate_versions();
        }
        self.translate_settled(&id, code, cx);
        self.translate_follow(&id, code, cx);
        true
    }

    /// The language to work on: its version, or one made now and its first pass begun.
    pub(crate) fn translate_language(&mut self, code: String, cx: &mut Context<Self>) {
        if !self.lib.as_ref().is_some_and(|lib| lib.writing.translate.loaded) { self.translate_versions() }
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(manuscript) = lib.writing.current() else { return };
        let code = if code == manuscript.language {
            bilingual::language("", &manuscript.language, if crate::i18n::is_english() { "en" } else { "zh" }).to_string()
        } else {
            code
        };
        let id = manuscript.id.clone();
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
            let entering = !lib.writing.reading;
            lib.writing.face();
            lib.writing.translate.editing = None;
            lib.writing.translate.hovered = None;
            if entering { self.write_snapshot("leave") }
        }
        self.translate_settled(&id, &code, cx);
        self.translate_follow(&id, &code, cx);
        if !had {
            let name = label(&LANGUAGES, &code).to_string();
            self.say(tf!("{name}译稿已开启：正在逐段翻译，点任意段落或句子可直接修改，改完即可导出", "The {name} version is ready: translating paragraph by paragraph; click any paragraph or sentence to edit, then export"), cx);
        }
    }

    /// 重译: what the person wrote is set aside, kept, and the unit is the model's again, translated from the source as it stands.
    fn translate_redo(&mut self, at: usize, cx: &mut Context<Self>) {
        let Some((id, language, unit, Some(kept))) = self.translate_unit(at) else { return };
        let Some(lib) = self.lib.as_mut() else { return };
        if kept.edited && !kept.text.trim().is_empty() {
            let mut aside = Kept { id: 0, gone: wisp_core::time::now_iso(), ..kept.clone() };
            if let Err(err) = lib.store.save_version_block(&id, &language, &mut aside) { return self.fail(format!("{err:#}"), cx) }
            if let Some(version) = lib.writing.translate.kept.get_mut(&(id.clone(), language.clone())) { version.kept.push(aside) }
        }
        lib.writing.translate.carry.remove(&kept.id);
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

    /// Single-sentence (or selected span) refinement inside the active inline editor.
    fn translate_sentence(&mut self, at: usize, action: SentenceAction, from_sel: bool, cx: &mut Context<Self>) {
        let Some((id, language, unit, _)) = self.translate_unit(at) else { return };
        let Some(lib) = self.lib.as_ref() else { return };
        let Some(editing) = lib.writing.translate.editing.as_ref().filter(|e| e.unit == at && e.manuscript == id && e.language == language) else { return };
        let _ = editing;
        let input = lib.writing.translate.editor.read(cx);
        let target_block = input.text().to_string();
        let sel = input.selection();
        let source_block = follow::as_written(&unit.sent, &unit.cites);
        let link = follow::sentence_pair(&source_block, &target_block, sel.clone());
        let (target_span, source_sent) = if from_sel && !sel.is_empty() && !target_block[sel.clone()].trim().is_empty() {
            let src = link.as_ref().map(|l| source_block[l.source.clone()].to_string()).unwrap_or_else(|| source_block.clone());
            (sel, src)
        } else if let Some(link) = link {
            (link.target.clone(), source_block[link.source].to_string())
        } else if !target_block.trim().is_empty() {
            (0..target_block.len(), source_block.clone())
        } else {
            // Untranslated block: fall back to whole-block comparison.
            return self.translate_compare(at, cx);
        };
        let target_sent = target_block[target_span.clone()].to_string();
        let chosen = self.settings.compare_profiles();
        let (single, profiles) = if chosen.len() >= 2 { (true, chosen) } else { (false, vec![self.settings.profile_for_or_default(Task::Translate)]) };
        let (pairs, requirement) = self.translate_rules(&id, &language);
        let terms = glossary::occurring(&source_sent, &pairs);
        let (system, message) = follow::sentence_prompt(action, &language, &source_sent, &target_sent, &source_block, &target_block, &terms, &requirement, single);
        let requests = profiles.iter().map(|profile| (ai::plain_request(profile, &system, &message), profile.protocol)).collect();
        let title = match action {
            SentenceAction::Retranslate => "本句重译",
            SentenceAction::Reword => "本句换个说法",
            SentenceAction::Polish => "本句润色精简",
        };
        let Some(lib) = self.lib.as_mut() else { return };
        let translate = &mut lib.writing.translate;
        translate.generation += 1;
        let generation = translate.generation;
        translate.compare = Some(Compared { unit: at, sent: unit.sent.clone(), span: Some((target_span, title)), single, place: false, comparison: crate::compare::Comparison::new(&profiles, generation) });
        let library = lib.info.id.clone();
        self.compare_ask(library, generation, requests, |this| this.lib.as_mut()?.writing.translate.compare.as_mut().map(|compared| &mut compared.comparison), |_| {}, cx);
    }

    /// Whole-unit comparison across models (or three kinds from one model).
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
        translate.compare = Some(Compared { unit: at, sent: unit.sent.clone(), span: None, single, place: false, comparison: crate::compare::Comparison::new(&profiles, generation) });
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
        if parsed.is_empty() {
            let fallback = if !row.busy && !row.result.trim().is_empty() { row.result.trim().to_string() } else { String::new() };
            return vec![(row.model.clone(), fallback, row.busy, row.error.clone())];
        }
        parsed.into_iter().map(|version| (t(version.kind.label()).to_string(), version.text, row.busy, row.error.clone())).collect()
    }

    /// A version taken: replaces either the active sentence span (keeping Undo in `editor`) or the whole unit.
    fn translate_compare_take(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(compared) = lib.writing.translate.compare.take() else { return };
        let Some((_, text, _, _)) = Self::translate_compared(&compared).into_iter().nth(n).filter(|(_, text, busy, _)| !*busy && !text.trim().is_empty()) else {
            lib.writing.translate.compare = Some(compared);
            return;
        };
        let chosen = text.trim().trim_matches('"').trim().to_string();
        let at = compared.unit;
        // Brought from the primary to be placed (`Placing`): it goes where the person put the field's selection, the caret's place when nothing is selected.
        if compared.place {
            let Some(lib) = self.lib.as_ref() else { return };
            let Some(editing) = lib.writing.translate.editing.as_ref().filter(|e| e.unit == at && e.sent == compared.sent) else { return };
            let spaced = !matches!(editing.language.as_str(), "zh" | "ja");
            let editor = lib.writing.translate.editor.clone();
            let (sel, before) = { let input = editor.read(cx); (input.selection(), input.text()[..input.selection().start].chars().last()) };
            let put = if sel.is_empty() && spaced && before.is_some_and(|c| !c.is_whitespace()) { format!(" {chosen}") } else { chosen };
            editor.update(cx, |input, cx| input.replace_range(sel, &put, cx));
            editor.read(cx).focus(window);
            return;
        }
        if let Some((span, _)) = compared.span
            && let Some(lib) = self.lib.as_ref()
            && lib.writing.translate.editing.as_ref().is_some_and(|e| e.unit == at && e.sent == compared.sent)
        {
            let editor = lib.writing.translate.editor.clone();
            let current = editor.read(cx).text().to_string();
            if current.get(span.clone()).is_some() {
                editor.update(cx, |input, cx| input.replace_range(span, &chosen, cx));
                editor.read(cx).focus(window);
                return;
            }
        }
        let Some((_, _, unit, _)) = self.translate_unit(at).filter(|(_, _, unit, _)| unit.sent == compared.sent) else { return };
        let written = follow::placeheld(&chosen, &unit.cites);
        self.translate_keep(at, |unit, one| {
            one.text = written;
            one.source = unit.sent.clone();
            one.edited = true;
        }, cx);
        if let Some(lib) = self.lib.as_ref() && lib.writing.translate.editing.as_ref().is_some_and(|editing| editing.unit == at) {
            let editor = lib.writing.translate.editor.clone();
            editor.update(cx, |input, cx| input.set_text(&chosen, cx));
            editor.read(cx).focus(window);
        }
    }

    // ---- a version of 译成… put into a version (write_assist.rs)

    /// Put a version of 译成… into the version in its language, where the stretch of the primary it is for pairs
    /// with whole sentences there (`follow::sentences_in`). It goes through the field of that paragraph, so it is the
    /// person's change, saved as typing is, and Undo in that field (查看) takes it back; the primary is not touched.
    /// Where the place is not sure, the paragraph is opened for editing with the versions under it, for the person to
    /// place; a paragraph not translated yet is translated first, and the version put in once it is. `later`: that
    /// translation has just come, and nothing is opened by itself.
    pub(crate) fn translate_place(&mut self, placing: Placing, later: bool, cx: &mut Context<Self>) {
        let (id, language) = (placing.manuscript.clone(), placing.language.clone());
        let name = label(&LANGUAGES, &language).to_string();
        self.translate_settled(&id, &language, cx);
        let found = self.lib.as_ref().and_then(|lib| {
            let version = lib.writing.translate.kept.get(&(id.clone(), language.clone()))?;
            let u = version.units.iter().position(|unit| unit.part == Part::Text && unit.section == placing.section && unit.block == placing.block && unit.sent == placing.sent)?;
            let unit = version.units[u].clone();
            let written = version.kept_at(u).map(|kept| follow::as_written(&kept.text, &unit.cites)).unwrap_or_default();
            Some((u, unit, written))
        });
        let Some((u, unit, written)) = found else { return self.fail(t("原来那段已经改动，找不到了：可以复制结果，自己放进去。"), cx) };
        // The version on show with another paragraph being edited: what came later is not put into it under the person's hands.
        let shown = self.translate_shown().is_some_and(|(on, code)| on == id && code == language);
        let busy = shown && self.lib.as_ref().and_then(|lib| lib.writing.translate.editing.as_ref()).is_some_and(|e| e.unit != u || e.manuscript != id || e.language != language);
        let sure = if placing.loose { None } else { follow::sentences_in(&placing.source, &written, placing.sel.clone()) };
        match sure {
            Some(range) if !(later && busy) => {
                let Some(lib) = self.lib.as_mut() else { return };
                let translate = &mut lib.writing.translate;
                (translate.compare, translate.hovered) = (None, None);
                translate.editing = Some(Editing { manuscript: id, language: language.clone(), unit: u, sent: unit.sent.clone() });
                translate.focus_editor = shown;
                let editor = translate.editor.clone();
                let text = placing.text.trim().to_string();
                editor.update(cx, |input, cx| {
                    input.set_text(&written, cx);
                    input.replace_range(range, &text, cx);
                });
                if shown { self.say(tf!("已放进{name}译稿", "Put into the {name} version"), cx) } else { self.say_with(tf!("已放进{name}译稿", "Put into the {name} version"), t("查看"), Cmd::Translate(Do::Look(language)), cx) }
            }
            None if !later && !placing.loose && written.trim().is_empty() => {
                // Not translated yet: it is first, and the version is put in when it comes (`translate_put`).
                if let Some(lib) = self.lib.as_mut() { lib.writing.translate.placing = Some(placing.clone()) }
                if !self.translate_soon(&id, &language, &unit.sent, cx) {
                    if let Some(lib) = self.lib.as_mut() { lib.writing.translate.placing = None }
                    return self.translate_open_placing(placing, u, &unit, &written, cx);
                }
                // A translation kept already has come at once and been put in.
                if self.lib.as_ref().is_some_and(|lib| lib.writing.translate.placing.is_some()) {
                    self.say(tf!("{name}译稿还没译到这一段：先译它，译好就把这一句放进去", "The {name} version has not reached this paragraph: it is translated first, and this sentence put in once it is"), cx)
                }
            }
            _ if later => {
                if let Some(lib) = self.lib.as_mut() { lib.writing.translate.placing = Some(placing) }
                self.say_with(tf!("{name}译稿译好了这一段，这一句放在哪要你来定", "The {name} version has this paragraph now; where this sentence goes is yours to choose"), t("去放"), Cmd::Translate(Do::Place), cx)
            }
            _ => self.translate_open_placing(placing, u, &unit, &written, cx),
        }
    }

    /// Where a version of 译成… goes is not sure: the version in its language on show, the paragraph open for editing
    /// with the versions under it, its likeliest place selected, so 采用 puts one there or where the person selects.
    fn translate_open_placing(&mut self, placing: Placing, u: usize, unit: &Unit, written: &str, cx: &mut Context<Self>) {
        self.translate_language(placing.language.clone(), cx);
        let Some(lib) = self.lib.as_mut() else { return };
        let translate = &mut lib.writing.translate;
        translate.generation += 1;
        let rows = placing.choices.iter().map(|(said, text)| crate::compare::Row { model: said.clone(), result: text.clone(), busy: false, error: String::new(), note: String::new() }).collect();
        translate.compare = Some(Compared { unit: u, sent: unit.sent.clone(), span: None, single: true, place: true, comparison: crate::compare::Comparison { rows, generation: translate.generation } });
        translate.editing = Some(Editing { manuscript: placing.manuscript.clone(), language: placing.language.clone(), unit: u, sent: unit.sent.clone() });
        translate.focus_editor = true;
        let guess = (!written.trim().is_empty()).then(|| follow::sentence_pair(written, &placing.source, placing.sel.clone()).map(|link| link.source)).flatten().unwrap_or(written.len()..written.len());
        let editor = translate.editor.clone();
        editor.update(cx, |input, cx| {
            input.set_text(written, cx);
            input.select_range(guess, cx);
        });
        self.write_facing_keep(&unit.section, unit.block);
        cx.notify();
    }

    /// The paragraph saying `sent` of a version translated before what else waits: put first in the run going, or
    /// asked for alone. `false` when another version's run is going.
    fn translate_soon(&mut self, id: &str, language: &str, sent: &str, cx: &mut Context<Self>) -> bool {
        let rules = self.translate_rules(id, language);
        let key = Self::translate_key(language, sent, &rules);
        let Some(lib) = self.lib.as_mut() else { return false };
        // A translation that failed is asked for again, as 重试 does.
        if matches!(lib.translations.get(&key), Some(Translated::Failed(_))) { lib.translations.remove(&key); }
        lib.writing.translate.paused = false;
        match lib.writing.translate.run.as_mut().filter(|run| run.run.batch.running()) {
            Some(run) if run.manuscript == id && run.language == language => {
                if run.run.batch.working.iter().any(|job| job.key == key) { return true }
                let job = match run.run.batch.jobs.iter().position(|job| job.key == key) {
                    Some(at) => run.run.batch.jobs.remove(at).unwrap(),
                    None => {
                        run.prompts.insert(key.clone(), Self::translate_prompt(language, sent, &rules));
                        run.run.batch.total += 1;
                        Job { key, text: sent.trim().to_string(), papers: 1 }
                    }
                };
                run.run.batch.jobs.push_front(job);
                true
            }
            Some(_) => false,
            None => {
                self.translate_start(id, language, vec![sent.to_string()], cx);
                true
            }
        }
    }

    /// 查看 after a version of 译成… was put into the version in `language`: that version on show, the paragraph in
    /// view and open for editing with what was put selected, Undo there taking it back.
    fn translate_look(&mut self, language: String, cx: &mut Context<Self>) {
        let current = self.lib.as_ref().and_then(|lib| lib.writing.current()).map(|m| m.id.clone());
        let editing = self.lib.as_ref().and_then(|lib| lib.writing.translate.editing.clone()).filter(|e| e.language == language && Some(&e.manuscript) == current.as_ref());
        self.translate_language(language, cx);
        let Some(editing) = editing else { return };
        let Some(unit) = self.translate_unit_in(&editing.manuscript, &editing.language, editing.unit).map(|(_, _, unit, _)| unit).filter(|unit| unit.sent == editing.sent) else { return };
        // A paragraph of a section that is not on the page is not opened.
        if !self.lib.as_ref().is_some_and(|lib| lib.writing.facing_holds(&unit.section)) { return }
        self.write_facing_keep(&unit.section, unit.block);
        if let Some(lib) = self.lib.as_mut() {
            lib.writing.translate.editing = Some(editing);
            lib.writing.translate.focus_editor = true;
        }
    }

    /// Check readiness before exporting a pure translated version; prompt with counts if anything is unfinished.
    fn translate_ask_export(&mut self, lang: String, out: ExportOut, cx: &mut Context<Self>) {
        let Some((id, code)) = self.resolve_export_target(&lang) else { return };
        self.translate_settled(&id, &code, cx);
        let bilingual = matches!(out, ExportOut::BilingualMarkdown | ExportOut::BilingualDocx);
        if !bilingual && let Some(version) = self.lib.as_ref().and_then(|lib| lib.writing.translate.kept.get(&(id.clone(), code.clone()))) {
            let unready = follow::export_unready(&version.kept, &version.units, &version.pairs);
            if unready != Unready::default() {
                let name = label(&LANGUAGES, &code);
                let mut said = vec![];
                if unready.untranslated > 0 { said.push(tf!("{} 段未译（导出会保留原语言文字）", "{} untranslated (will keep primary text)", unready.untranslated)) }
                if unready.changed > 0 { said.push(tf!("{} 段原文改动尚未核对", "{} with unreviewed source changes", unready.changed)) }
                if unready.cites > 0 { said.push(tf!("{} 段引用标记对不上", "{} with mismatched citations", unready.cites)) }
                self.modal_error.clear();
                self.modal = Some(Modal::Confirm {
                    title: tf!("{name}译稿还有待处理的段落，仍要导出？", "Export {name} version with pending paragraphs?"),
                    message: tf!("当前译稿中还有：{}。你可以取消后点译稿栏头的「⌄」逐处核对，也可以直接导出当前状态。", "Still pending: {}. Cancel to review them via the header's next button, or export as is.", said.join(t("；"))),
                    action: t("仍要导出").into(),
                    cmd: Cmd::Translate(Do::ExportNow(code, out)),
                });
                return;
            }
        }
        self.translate_export_now(code, out, cx);
    }

    fn resolve_export_target(&self, lang: &str) -> Option<(String, String)> {
        if !lang.is_empty() {
            let id = self.lib.as_ref()?.writing.current()?.id.clone();
            return Some((id, lang.to_string()));
        }
        if let Some(shown) = self.translate_shown() { return Some(shown) }
        let lib = self.lib.as_ref()?;
        let m = lib.writing.current()?;
        let first = lib.writing.translate.versions.get(&m.id)?.first()?.clone();
        Some((m.id.clone(), first))
    }

    /// Execute export of the translated version (pure or bilingual, Word / Markdown / Clipboard).
    fn translate_export_now(&mut self, lang: String, out: ExportOut, cx: &mut Context<Self>) {
        let Some((id, code)) = self.resolve_export_target(&lang) else { return };
        self.translate_settled(&id, &code, cx);
        let name_lang = label(&LANGUAGES, &code).to_string();
        match out {
            ExportOut::Docx => {
                let Some(mut manuscript) = self.build_translated_manuscript(&id, &code) else { return };
                if manuscript.sections.iter().all(|s| s.body.trim().is_empty()) { return self.fail(t("这份稿件还没有写什么。"), cx) }
                // Disambiguate filename when title wasn't translated or matches primary.
                let primary_title = self.lib.as_ref().and_then(|l| l.writing.manuscripts.iter().find(|m| m.id == id)).map(|m| m.title.trim().to_string()).unwrap_or_default();
                if manuscript.title.trim().is_empty() || manuscript.title.trim() == primary_title {
                    let base = if primary_title.is_empty() { t(manuscript::UNTITLED).to_string() } else { primary_title };
                    manuscript.title = format!("{base}-{code}");
                }
                self.export_this(manuscript, ManuscriptExport::Docx, cx);
            }
            ExportOut::Copy => {
                let Some(manuscript) = self.build_translated_manuscript(&id, &code) else { return };
                if manuscript.sections.iter().all(|s| s.body.trim().is_empty()) { return self.fail(t("这份稿件还没有写什么。"), cx) }
                let text = self.format_plain_with_cites(&manuscript);
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.say(tf!("已复制{name_lang}译稿全文。", "Copied full {name_lang} version."), cx);
            }
            ExportOut::Markdown => {
                let Some(manuscript) = self.build_translated_manuscript(&id, &code) else { return };
                if manuscript.sections.iter().all(|s| s.body.trim().is_empty()) { return self.fail(t("这份稿件还没有写什么。"), cx) }
                let md = self.format_markdown_with_cites(&manuscript);
                let primary_title = self.lib.as_ref().and_then(|l| l.writing.manuscripts.iter().find(|m| m.id == id)).map(|m| crate::i18n::manuscript_title(m).to_string()).unwrap_or_default();
                let stem = if manuscript.title.trim().is_empty() || manuscript.title.trim() == primary_title {
                    format!("{}-{code}", crate::manuscripts::file_name(&primary_title))
                } else {
                    crate::manuscripts::file_name(&manuscript.title)
                };
                self.save_exported_bytes(md.into_bytes(), "md", &stem, self.markdown_pictures(&manuscript), String::new(), cx);
            }
            ExportOut::BilingualMarkdown => {
                let Some(manuscript) = self.build_bilingual_manuscript(&id, &code) else { return };
                if manuscript.sections.iter().all(|s| s.body.trim().is_empty()) { return self.fail(t("这份稿件还没有写什么。"), cx) }
                let md = self.format_markdown_with_cites(&manuscript);
                let base = self.lib.as_ref().and_then(|l| l.writing.manuscripts.iter().find(|m| m.id == id)).map(|m| crate::manuscripts::file_name(crate::i18n::manuscript_title(m))).unwrap_or_else(|| manuscript::UNTITLED.to_string());
                let suffix = if crate::i18n::is_english() { format!("bilingual-{code}") } else { format!("双语对照-{code}") };
                self.save_exported_bytes(md.into_bytes(), "md", &format!("{base}-{suffix}"), self.markdown_pictures(&manuscript), String::new(), cx);
            }
            ExportOut::BilingualDocx => {
                let Some(manuscript) = self.build_bilingual_manuscript(&id, &code) else { return };
                if manuscript.sections.iter().all(|s| s.body.trim().is_empty()) { return self.fail(t("这份稿件还没有写什么。"), cx) }
                let (bytes, note) = match self.manuscript_word(&manuscript, false) {
                    Ok(done) => done,
                    Err(err) => return self.fail(err, cx),
                };
                let base = self.lib.as_ref().and_then(|l| l.writing.manuscripts.iter().find(|m| m.id == id)).map(|m| crate::manuscripts::file_name(crate::i18n::manuscript_title(m))).unwrap_or_else(|| manuscript::UNTITLED.to_string());
                let suffix = if crate::i18n::is_english() { format!("bilingual-{code}") } else { format!("双语对照-{code}") };
                self.save_exported_bytes(bytes, "docx", &format!("{base}-{suffix}"), vec![], note, cx);
            }
        }
    }

    fn format_markdown_with_cites(&self, manuscript: &Manuscript) -> String {
        let Some(lib) = self.lib.as_ref() else { return manuscript::markdown(manuscript) };
        let style_key = if manuscript.cite_style.is_empty() { &self.settings.cite_style } else { &manuscript.cite_style };
        let Ok(style) = wisp_core::csl::style(style_key, &self.data_root()) else { return manuscript::markdown(manuscript) };
        let index = crate::cite::index(lib);
        formatted_markdown(manuscript, &index, &style)
    }

    fn format_plain_with_cites(&self, manuscript: &Manuscript) -> String {
        let Some(lib) = self.lib.as_ref() else { return manuscript::plain_text(manuscript) };
        let style_key = if manuscript.cite_style.is_empty() { &self.settings.cite_style } else { &manuscript.cite_style };
        let Ok(style) = wisp_core::csl::style(style_key, &self.data_root()) else { return manuscript::plain_text(manuscript) };
        let index = crate::cite::index(lib);
        let bodies: Vec<&str> = manuscript.sections.iter().map(|s| s.body.as_str()).collect();
        let format = cite::Formatter::new(&bodies, &index, &style, &manuscript.language);
        let mut formatted = manuscript.clone();
        for section in &mut formatted.sections { section.body = format.apply(&section.body, &index) }
        let mut out = manuscript::plain_text(&formatted);
        if !format.references().is_empty() {
            out.push_str(&format!("\n{}\n\n", format.heading()));
            out.push_str(&format.references().iter().map(|(_, e)| e.text.clone()).collect::<Vec<_>>().join("\n\n"));
            out.push('\n');
        }
        out
    }

    fn save_exported_bytes(&self, bytes: Vec<u8>, extension: &'static str, stem: &str, pictures: Vec<(String, std::path::PathBuf)>, note: String, cx: &mut Context<Self>) {
        let name = format!("{stem}.{extension}");
        let directory = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(|| self.root.clone());
        let picked = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut path))) = picked.await {
                if path.extension().is_none() { path.set_extension(extension); }
                let result = std::fs::write(&path, bytes);
                let note = if result.is_ok() && !pictures.is_empty() { crate::manuscripts::pictures_beside(&path, &pictures) } else { note };
                this.update(cx, |this, cx| match result {
                    Ok(()) => this.say(tf!("已导出到 {}{note}", "Exported to {}{note}", path.display()), cx),
                    Err(err) => this.fail(tf!("无法写入 {}：{err}", "Cannot write {}: {err}", path.display()), cx),
                }).ok();
            }
        }).detach();
    }

    /// 设为主稿: the version's words become the manuscript's, the primary's a version of them.
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
        (translate.editing, translate.hovered, translate.run) = (None, None, None);
        translate.carry.clear();
        self.translate_versions();
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

    /// 译文对照's languages, each marked when the manuscript keeps a version in it, then where the translation stands.
    pub(crate) fn translate_facing_menu(&self) -> (Vec<MenuItem>, Vec<usize>) {
        let mut items = self.facing_language_menu();
        let versions = self.lib.as_ref().and_then(|lib| lib.writing.current().and_then(|m| lib.writing.translate.versions.get(&m.id))).cloned().unwrap_or_default();
        for item in &mut items {
            if let Cmd::WriteFacingLanguage(code) = &item.cmd && versions.contains(code) { item.note = Some(t("译稿").into()) }
        }
        let breaks = vec![items.len()];
        items.extend(self.facing_arrangement(false));
        (items, breaks)
    }

    /// The versions of the manuscript to work on, the languages to make one in, and where the version stands.
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
        let mut breaks = breaks;
        breaks.push(items.len());
        items.extend(self.facing_arrangement(lib.writing.translate.alone));
        (items, breaks)
    }

    /// Dedicated export menu on the version's header (`导出译稿 ⌄`): pure version exports + bilingual review exports.
    pub(crate) fn translate_export_menu(&self) -> (Vec<MenuItem>, Vec<usize>) {
        let Some((_, code)) = self.translate_shown() else { return (vec![], vec![]) };
        let name = label(&LANGUAGES, &code);
        let item = |label: String, out: ExportOut| MenuItem { label: label.into(), checked: false, note: None, cmd: Cmd::Translate(Do::Export(code.clone(), out)) };
        let items = vec![
            item(tf!("Word（.docx）：{name}译稿", "Word (.docx): {name} version"), ExportOut::Docx),
            item(tf!("Markdown（.md）：{name}译稿", "Markdown (.md): {name} version"), ExportOut::Markdown),
            item(tf!("复制{name}译稿全文", "Copy full {name} version"), ExportOut::Copy),
            item(t("双语对照 Word（.docx）：原文 + 译稿").into(), ExportOut::BilingualDocx),
            item(t("双语对照 Markdown（.md）：原文 + 译稿").into(), ExportOut::BilingualMarkdown),
        ];
        (items, vec![3])
    }

    /// In the main top-bar 导出 menu: available whenever the current manuscript keeps ANY version (even while back in
    /// normal primary-language editing mode!).
    pub(crate) fn translate_export_items(&self) -> Vec<MenuItem> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        let Some(manuscript) = lib.writing.current() else { return vec![] };
        let codes: Vec<String> = if let Some((_, shown)) = self.translate_shown() {
            vec![shown]
        } else {
            lib.writing.translate.versions.get(&manuscript.id).cloned().unwrap_or_default()
        };
        let mut out = Vec::new();
        for code in codes {
            if code == manuscript.language { continue }
            let name = label(&LANGUAGES, &code);
            for (what, kind) in [
                ("Word（.docx）", ExportOut::Docx),
                ("Markdown（.md）", ExportOut::Markdown),
                (t("复制全文"), ExportOut::Copy),
                (t("双语对照 Word（.docx）"), ExportOut::BilingualDocx),
            ] {
                out.push(MenuItem { label: tf!("{name}译稿：{what}", "{name} version: {what}").into(), checked: false, note: None, cmd: Cmd::Translate(Do::Export(code.clone(), kind)) });
            }
        }
        out
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
        let writing = &self.lib.as_ref()?.writing;
        let version = writing.translate.kept.get(&(id, language.clone()))?;
        let name = label(&LANGUAGES, &language);
        let (untranslated, changed) = version.left(|section| writing.facing_holds(section));
        Some(if untranslated + changed == 0 {
            tf!("{name}译稿实时跟随主稿 · 悬停对照原句，点击任意句子直接修改或单句重译 · 改完点右上角「导出译稿」", "The {name} version follows the primary · Hover to align sentences, click any sentence to edit or retranslate · Click Export over it when ready")
        } else {
            tf!("{name}译稿实时跟随主稿 · 点击任意句子直接修改 · 标出的段落待你核对", "The {name} version follows the primary · Click any sentence to edit · Marked paragraphs need your review")
        })
    }

    /// Floating selection toolbar shown when text is selected inside `translate.editor`.
    fn render_translate_selection_bar(&self, u: usize, cx: &mut Context<Self>) -> Div {
        let mut btn = |id: &'static str, icon: Option<&'static str>, text: &'static str, action: SentenceAction| {
            ui::ghost_button(id, icon, Some(t(text).into()))
                .h(px(28.))
                .px(px(7.))
                .text_size(text_px(12.))
                .on_click(self.on(Cmd::Translate(Do::Sentence(u, action, true)), cx))
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .p(px(3.))
            .text_color(Theme::text())
            .child(btn("wt-sel-retranslate", Some("sparkle"), "重译选中", SentenceAction::Retranslate))
            .child(btn("wt-sel-reword", None, "换个说法", SentenceAction::Reword))
            .child(btn("wt-sel-polish", None, "润色精简", SentenceAction::Polish))
    }

    /// Compact action strip under the active inline editor: current sentence index, 1-click single-sentence AI tools,
    /// whole-paragraph comparison, and `⌘Enter` / `完成`.
    fn render_translate_edit_bar(&self, u: usize, unit: &Unit, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let input = lib.writing.translate.editor.read(cx);
        let source_written = follow::as_written(&unit.sent, &unit.cites);
        let link = (unit.part == Part::Text).then(|| follow::sentence_pair(&source_written, input.text(), input.selection())).flatten();
        let has_sel = !input.selected().trim().is_empty();
        let sent_badge = match (&link, has_sel) {
            (_, true) => t("已选中片段").to_string(),
            (Some(l), false) if l.total > 1 => tf!("第 {} / {} 句", "Sentence {} / {}", l.index + 1, l.total),
            _ => t("当前段").to_string(),
        };
        let chip = |sub: &str, icon: Option<&'static str>, label_str: &'static str, tip: String, cmd: Cmd, cx: &mut Context<Self>| {
            ui::ghost_button(SharedString::from(format!("{key}-{sub}")), icon, Some(t(label_str).into()))
                .h(text_px(22.))
                .px(px(6.))
                .text_size(text_px(11.5))
                .tooltip(ui::tip(tip))
                .on_click(self.on(cmd, cx))
        };
        let next_shortcut = if cfg!(target_os = "macos") { "⌘↵" } else { "Ctrl+↵" };
        let sentence_tools = (unit.part == Part::Text).then(|| {
            vec![
                chip("sent-retranslate", Some("sparkle"), if has_sel { "重译选中" } else { "重译本句" }, t("结合整段语境与对应原句，给出三种译法替换本句").into(), Cmd::Translate(Do::Sentence(u, SentenceAction::Retranslate, has_sel)), cx).into_any_element(),
                chip("sent-reword", None, "换个说法", t("为当前这句提供三种不同风格的学术表达").into(), Cmd::Translate(Do::Sentence(u, SentenceAction::Reword, has_sel)), cx).into_any_element(),
                chip("sent-polish", None, "润色精简", t("修正语法并精简当前这句的学术表达").into(), Cmd::Translate(Do::Sentence(u, SentenceAction::Polish, has_sel)), cx).into_any_element(),
            ]
        });
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(3.))
            .ml(px(-4.))
            .pt(px(2.))
            .child(div().px(px(5.)).py(px(1.)).rounded(px(4.)).bg(Theme::accent_soft()).text_size(text_px(11.)).text_color(Theme::accent()).child(sent_badge))
            .children(sentence_tools.into_iter().flatten())
            .child(chip("para-compare", None, "整段多版", t("让对照模型（或单模型三种译法）重译整段").into(), Cmd::Translate(Do::Compare(u)), cx))
            .child(div().flex_1())
            .child(chip("edit-next", None, "下一段", tf!("确认本段并跳到下一段继续审改（{next_shortcut}）", "Confirm this block and move to the next ({next_shortcut})"), Cmd::Translate(Do::EditNext), cx))
            .child(
                ui::secondary_button(SharedString::from(format!("{key}-done")), t("完成"))
                    .h(text_px(22.))
                    .px(px(8.))
                    .text_size(text_px(11.5))
                    .on_click(self.on(Cmd::Translate(Do::Done), cx)),
            )
            .into_any_element()
    }

    /// The manuscript at the left as the preview draws it, the version at the right, block beside block at the
    /// same height, each block and sentence editable in place.
    pub(crate) fn render_translating(&self, width: f32, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let Some((id, code)) = self.translate_shown() else { return div().into_any_element() };
        let Some(manuscript) = lib.writing.current() else { return div().into_any_element() };
        let (within, whole_shown) = (lib.writing.facing_sections(), !lib.writing.facing_section());
        let translate = &lib.writing.translate;
        let Some(version) = translate.kept.get(&(id.clone(), code.clone())) else { return div().into_any_element() };
        let rules = self.translate_rules(&id, &code);
        let alone = translate.alone;
        let (side, half, whole) = crate::write_facing::measures(width, !alone && !self.settings.write_facing_interleaved);
        let editing = translate.editing.as_ref().filter(|editing| editing.manuscript == id && editing.language == code).map(|editing| editing.unit);
        translate.tops.borrow_mut().clear();
        translate.all_tops.borrow_mut().clear();
        let at: HashMap<(&str, Part, usize), usize> = version.units.iter().enumerate().map(|(u, unit)| ((unit.section.as_str(), unit.part, unit.block), u)).collect();
        let busy = |sent: &str| lib.translations.get(&Self::translate_key(&code, sent, &rules)).cloned();
        let named = |size: f32, line: f32, text: String| div().text_size(text_px(size)).line_height(text_px(line)).font_weight(FontWeight::SEMIBOLD).child(text);

        // A block of the primary and what stands beside it. The counterpart of the block edited has an accent line at its side.
        let pair = |left: Option<AnyElement>, right: AnyElement, foot: bool, marked: bool| -> AnyElement {
            let left = left.map(|left| div().relative().child(left).when(marked, |d| d.child(div().absolute().top_0().bottom_0().left(px(-12.)).w(px(2.)).rounded(px(1.)).bg(Theme::accent()))));
            if alone { return div().flex_none().w(px(half)).child(right).into_any_element() }
            if side {
                div().flex().flex_none().gap(px(GUTTER)).when(foot, |d| d.items_end()).when(!foot, |d| d.items_start()).child(div().flex_none().w(px(half)).children(left)).child(div().flex_none().w(px(half)).child(right)).into_any_element()
            } else {
                div().flex().flex_col().flex_none().gap(px(6.)).w(px(half)).children(left).child(div().text_color(Theme::text_secondary()).child(right)).into_any_element()
            }
        };

        let me = cx.entity().downgrade();

        // What stands at the right for a unit: the field + sentence tools while edited, else its interactive translation.
        let right = |u: usize, size: Option<(f32, f32)>, drawn: &dyn Fn(&str) -> AnyElement, cx: &mut Context<Self>| -> AnyElement {
            let unit = &version.units[u];
            let kept = version.kept_at(u);
            let state = follow::state(kept);
            let key = format!("wt-{}-{}", u, hashed(&unit.sent));
            let track_all = {
                let (all_tops, scroll) = (translate.all_tops.clone(), lib.citing.read_scroll.clone());
                canvas(|_, _, _| (), move |bounds, _, _, _| {
                    let (view, offset) = (scroll.bounds(), scroll.offset());
                    all_tops.borrow_mut().insert(u, f32::from(bounds.origin.y - view.origin.y - offset.y));
                })
                .absolute()
                .top_0()
                .left_0()
                .size_full()
            };
            if editing == Some(u) {
                let editor_el = if unit.part == Part::Text {
                    crate::input::with_selection_toolbar(&translate.editor, self.render_translate_selection_bar(u, cx)).into_any_element()
                } else {
                    translate.editor.clone().into_any_element()
                };
                let field = div()
                    .relative()
                    .w(px(half))
                    .ml(px(-9.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(Theme::accent().opacity(0.65))
                    .bg(Theme::bg_body())
                    .text_color(Theme::text())
                    .text_size(text_px(size.map_or(READ_SIZE, |s| s.0)))
                    .line_height(size.map_or(theme::body_line(26.), |s| text_px(s.1)))
                    .child(editor_el);
                let bar = self.render_translate_edit_bar(u, unit, &key, cx);
                let compared = translate.compare.as_ref().filter(|compared| compared.unit == u && compared.sent == unit.sent).map(|compared| self.render_translate_compared(compared, &key, half, cx));
                return div()
                    .relative()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(field)
                    .child(bar)
                    .children(compared)
                    .child(track_all)
                    .into_any_element();
            }
            let wants = matches!(state, State::Untranslated) || (state == State::Changed && kept.is_some_and(|k| k.edited)) || kept.is_some_and(|k| !follow::cites_kept(&unit.sent, &k.text));
            let shown: Option<AnyElement> = kept.filter(|k| !k.text.trim().is_empty()).map(|k| drawn(&k.text));
            let asked = busy(&unit.sent);
            let spinner = |id: String| div().flex().items_center().h(theme::body_line(24.)).child(ui::spinner(SharedString::from(id), 13., Theme::text_tertiary())).into_any_element();
            let group_id = SharedString::from(format!("{key}-grp"));
            let mut column = div().relative().group(group_id.clone()).flex().flex_col().gap(px(6.)).child(track_all);
            let body = div()
                .id(SharedString::from(key.clone()))
                .cursor_text()
                .rounded(px(4.))
                .hover(|s| s.bg(Theme::hover()))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(self.on(Cmd::Translate(Do::Edit(u)), cx));
            column = match (shown, state, &asked) {
                (shown, State::Untranslated | State::Changed, Some(Translated::Busy)) if !kept.is_some_and(|k| k.edited) => {
                    column.children(shown.map(|shown| div().opacity(0.5).child(shown).into_any_element())).child(spinner(format!("{key}-busy")))
                }
                (None, State::Untranslated, Some(Translated::Failed(error))) => {
                    column.child(div().flex().child(div().id(SharedString::from(format!("{key}-failed"))).text_size(text_px(12.)).line_height(theme::body_line(24.)).text_color(Theme::danger()).tooltip(ui::tip(ui::one_line(error))).child(t("没有译成"))))
                }
                (None, _, _) => column.child(body.child(div().text_size(text_px(13.)).line_height(theme::body_line(24.)).text_color(Theme::text_tertiary()).child(t("未译 · 点这里自己写")))),
                (Some(shown), _, _) => {
                    let lacks = kept.map(|k| glossary::check(&unit.sent, &k.text, &glossary::occurring(&unit.sent, &rules.0))).unwrap_or_default();
                    let mark = (!lacks.is_empty()).then(|| div().id(SharedString::from(format!("{key}-lacks"))).absolute().top(px(5.)).right(px(-20.)).child(ui::icon("triangle-alert", 12., Theme::warning())).tooltip(ui::tip(crate::write_assist::lacks_said(&lacks))));
                    let reviewed = state == State::Edited;
                    let check_click = self.on(Cmd::Translate(Do::Checked(u)), cx);
                    let review_action = (state == State::Translated).then(|| {
                        div()
                            .invisible()
                            .group_hover(group_id.clone(), |s| s.visible())
                            .absolute()
                            .top(px(-2.))
                            .right(px(2.))
                            .rounded(px(5.))
                            .bg(Theme::bg_body())
                            .border_1()
                            .border_color(Theme::line())
                            .shadow(theme::shadow_card())
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                ui::ghost_button(SharedString::from(format!("{key}-mark-ok")), Some("check"), Some(t("标为已审校").into()))
                                    .h(text_px(20.))
                                    .px(px(6.))
                                    .text_size(text_px(11.))
                                    .tooltip(ui::tip(t("标记这段译文已核对无误：锁定措辞，主稿未改时不会被模型覆盖")))
                                    .on_click(move |ev, window, cx| {
                                        cx.stop_propagation();
                                        check_click(ev, window, cx);
                                    }),
                            )
                    });
                    // Subtle green bar on the left margin of human-edited / reviewed blocks so progress is clear at a glance.
                    let reviewed_bar = reviewed.then(|| {
                        div()
                            .id(SharedString::from(format!("{key}-reviewed")))
                            .absolute()
                            .top(px(4.))
                            .bottom(px(4.))
                            .left(px(-8.))
                            .w(px(2.))
                            .rounded(px(1.))
                            .bg(Theme::success().opacity(0.55))
                            .tooltip(ui::tip(t("已人工修改或审校确认")))
                    });
                    column.child(body.relative().child(shown).children(reviewed_bar).children(mark).children(review_action))
                }
            };
            // Citation placeholder mismatch + 1-click restore button.
            if let Some(k) = kept.filter(|k| !k.text.trim().is_empty() && !follow::cites_kept(&unit.sent, &k.text)) {
                let said = tf!("译文的引用和原文对不上：原文 {} 处，译文 {} 处", "The translation's citations do not match the source's: {} in the source, {} here", follow::placeholders(&unit.sent).len(), follow::placeholders(&k.text).len());
                column = column.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().id(SharedString::from(format!("{key}-cites"))).text_size(text_px(11.)).text_color(Theme::danger()).tooltip(ui::tip(said)).child(t("引用对不上")))
                        .child(
                            ui::ghost_button(SharedString::from(format!("{key}-fix-cites")), None, Some(t("补回引用").into()))
                                .h(text_px(20.))
                                .px(px(6.))
                                .text_size(text_px(11.))
                                .text_color(Theme::accent())
                                .tooltip(ui::tip(t("自动按原文补齐缺失的引用标记，并去掉多余编号")))
                                .on_click(self.on(Cmd::Translate(Do::FixCites(u)), cx)),
                        ),
                );
            }
            if let Some(k) = kept.filter(|k| state == State::Changed && k.edited) {
                column = column.child(div().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(self.render_translate_changed(u, k, unit, &key, cx)));
            }
            if wants {
                let (tops, scroll) = (translate.tops.clone(), lib.citing.read_scroll.clone());
                column = column.child(
                    canvas(|_, _, _| (), move |bounds, _, _, _| {
                        let (view, offset) = (scroll.bounds(), scroll.offset());
                        tops.borrow_mut().insert(u, f32::from(bounds.origin.y - view.origin.y - offset.y));
                    })
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                );
            }
            column.into_any_element()
        };

        let title = at.get(&("", Part::Title, 0)).filter(|_| whole_shown).map(|&u| {
            let drawn = |text: &str| named(24., 32., ui::one_line(&plainly(text))).into_any_element();
            pair(Some(named(24., 32., crate::i18n::manuscript_title(manuscript).to_string()).into_any_element()), right(u, Some((24., 32.)), &drawn, cx), false, editing == Some(u))
        });
        let mut sections: Vec<AnyElement> = vec![];
        for (n, section) in manuscript.sections.iter().enumerate().filter(|(n, _)| within.contains(n)) {
            let labels = lib.citing.read.whole.get(n).map(Vec::as_slice).unwrap_or(&[]);
            let read = |within: std::ops::Range<usize>| wisp_core::cite::read_as(&section.body, labels, within);
            let heading = div()
                .id(("write-translate-heading", n))
                .flex()
                .items_baseline()
                .gap(px(8.))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
            if blocks.is_empty() {
                rows.push(pair(Some(div().text_size(text_px(13.)).text_color(Theme::text_tertiary()).child(t("这一节还没有写。")).into_any_element()), div().into_any_element(), false, false))
            }
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
                        rows.push(self.facing_revealed(&section.id, b, pair(Some(self.render_manuscript_picture(format!("{id}-p"), &shown, half, window)), beside, true, unit.is_some() && editing == unit)));
                    }
                    Kind::Kept => {
                        let same = page(format!("{id}-t"), &section.body[block.range.clone()]);
                        let original = self.render_read_piece(section, n, b, block.range.clone(), &manuscript::section_markdown(&read(block.range.clone()), 1), true, window, cx);
                        let row = if alone { pair(None, same, false, false) } else if side { pair(Some(original), same, false, false) } else { pair(Some(original), div().into_any_element(), false, false) };
                        rows.push(self.facing_revealed(&section.id, b, row));
                    }
                    Kind::Words => {
                        let drawn = |text: &str| -> AnyElement {
                            let Some(u) = unit else { return page(format!("{id}-t"), &restored(text)) };
                            let u_ref = &version.units[u];
                            let shown_md = manuscript::section_markdown(&restored(text), 1);
                            let mut marks = Vec::new();
                            if let Some((hu, tr_range)) = translate.hovered.as_ref().filter(|(hu, _)| *hu == u) {
                                let _ = hu;
                                let cite_labels: Vec<Option<String>> = block.cites.iter().map(|r| labels.iter().find(|(lr, _)| lr == r).map(|(_, l)| l.clone())).collect();
                                let (written, tr_labels) = written_with_labels(text, &u_ref.cites, &cite_labels);
                                let reading = Reading::new(&written, &tr_labels, 0..written.len(), &shown_md);
                                if let Some((from, to)) = reading.drawn(tr_range.clone()) {
                                    marks.push((from, to, Theme::accent().opacity(0.11)));
                                }
                            }
                            let (hover_me, click_me) = (me.clone(), me.clone());
                            crate::markdown::render_page_interactive(
                                &format!("{id}-t"),
                                &shown_md,
                                READ_SIZE,
                                marks,
                                Rc::new(move |place, window, cx| {
                                    hover_me.update(cx, |this, cx| this.run(Cmd::Translate(Do::Hover(place.map(|p| (u, p)))), window, cx)).ok();
                                }),
                                Rc::new(move |place, window, cx| {
                                    click_me.update(cx, |this, cx| this.run(Cmd::Translate(Do::EditAt(u, place)), window, cx)).ok();
                                }),
                            )
                        };
                        let beside = match unit { Some(u) => right(u, None, &drawn, cx), None => div().into_any_element() };
                        let original = (!alone).then(|| self.render_read_piece(section, n, b, block.range.clone(), &manuscript::section_markdown(&read(block.range.clone()), 1), true, window, cx));
                        rows.push(self.facing_revealed(&section.id, b, pair(original, beside, false, unit.is_some() && editing == unit)));
                    }
                }
            }
            sections.push(div().flex().flex_col().flex_none().gap(theme::paragraph_gap(10.)).text_size(text_px(READ_SIZE)).children(rows).into_any_element());
        }
        let references = if !whole_shown {
            None
        } else if alone {
            self.render_references_for(Some(&code), cx).map(|list| pair(None, list, false, false))
        } else {
            let left_refs = self.render_references_read(cx);
            let right_refs = side.then(|| self.render_references_for(Some(&code), cx)).flatten();
            if left_refs.is_some() || right_refs.is_some() {
                Some(pair(left_refs, right_refs.unwrap_or_else(|| div().into_any_element()), false, false))
            } else {
                None
            }
        };

        let divider = || div().absolute().top_0().bottom_0().left(px(half + GUTTER / 2.)).w(px(1.)).bg(Theme::line());
        let small = |text: &str| div().flex_none().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(text.to_string());
        let head = self.render_translate_head(version, &code, half, cx);
        let primary_name = label(&LANGUAGES, &manuscript.language).to_string();
        let head = div().flex().flex_none().justify_center().px(px(PAD)).child(
            div()
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(GUTTER))
                .w(px(whole))
                .h(text_px(32.))
                .when(side, |d| d.child(div().flex().flex_none().items_center().gap(px(6.)).w(px(half)).child(small(t("主稿"))).child(small(&primary_name))).child(divider()))
                .child(head),
        );
        // A press on the page outside the field puts the editing away; one inside the editing block stops propagation.
        let sheet = div()
            .id("write-translate-view")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&lib.citing.read_scroll)
            .border_b_1()
            .border_color(Theme::line())
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.translate_letgo(cx)))
            .child(div().flex().justify_center().px(px(PAD)).pb(px(24.)).child(div().relative().flex().flex_col().flex_none().gap(px(30.)).w(px(whole)).pt(px(12.)).text_color(Theme::text()).when(side, |d| d.child(divider())).children(title).children(sections).children(references)));
        div().flex().flex_col().flex_1().min_h_0().child(head).child(sheet).into_any_element()
    }

    /// The versions asked of several models (or three kinds from one model) for the unit or sentence edited.
    fn render_translate_compared(&self, compared: &Compared, key: &str, half: f32, cx: &mut Context<Self>) -> AnyElement {
        let rows = Self::translate_compared(compared);
        let heading = if compared.place { t("从主稿译来 · 在上面选中它要换掉的地方，再点采用").to_string() } else { match compared.span {
            Some((_, label)) => format!("{} · {}", t(label), t(if compared.single { "多模型对照" } else { "三种改法" })),
            None => t(if compared.single { "几个模型各译一版" } else { "三种译法" }).to_string(),
        } };
        let mut card = div().flex().flex_col().gap(px(8.)).w(px(half)).ml(px(-9.)).px(px(10.)).py(px(8.)).rounded(px(6.)).border_1().border_color(Theme::line()).bg(Theme::bg_sunken());
        card = card.child(div().flex().items_center().child(div().flex_1().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(heading)).child(ui::icon_action(SharedString::from(format!("{key}-compare-close")), "x", Theme::text_tertiary(), t("收起")).on_click(self.on(Cmd::Translate(Do::CompareClose), cx))));
        for (n, (said, text, busy, error)) in rows.into_iter().enumerate() {
            let id = compared.comparison.rows.get(if compared.single { n } else { 0 }).map_or(0, |row| hashed(&row.model));
            let head = div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().flex_1().min_w_0().truncate().text_size(text_px(11.)).text_color(Theme::text_secondary()).child(said))
                .when(busy, |d| d.child(ui::spinner(SharedString::from(format!("{key}-compare-{n}-{id}")), 11., Theme::text_tertiary())))
                .when(!busy && !text.trim().is_empty(), |d| d.child(ui::ghost_button(SharedString::from(format!("{key}-take-{n}-{id}")), None, Some(t("采用").into())).h(text_px(22.)).px(px(6.)).text_size(text_px(12.)).text_color(Theme::accent()).on_click(self.on(Cmd::Translate(Do::CompareTake(n)), cx))));
            let words = if !error.is_empty() && text.trim().is_empty() {
                div().text_size(text_px(12.)).text_color(Theme::danger()).child(ui::one_line(&error))
            } else {
                div().text_size(text_px(13.)).line_height(text_px(20.)).text_color(Theme::text()).child(text)
            };
            card = card.child(div().flex().flex_col().gap(px(2.)).child(head).child(words));
        }
        card.into_any_element()
    }

    /// 原文已改 under a translation the person made their own: the words of the source that changed, and 重译,
    /// 只带改动 and 已核对.
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
                let said = if now.trim() == then.trim() { "只带改动：译文不用改，已经合乎改后的原文" } else { "只带改动：只改了下面标出的地方" };
                let compared: AnyElement = match ai::word_diff(&now, &then) {
                    Some((old, new)) => div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
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
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(small(format!("{key}-redo"), "重译", Do::Redo(u), cx).tooltip(ui::tip(t("按改后的原文重新翻译；你改的译文搁置起来，不会丢"))))
                            .child(small(format!("{key}-carryit"), "只带改动", Do::Carry(u), cx).tooltip(ui::tip(t("让模型只把原文的这处改动带进你的译文，其余不动；先给你对照，再由你决定"))))
                            .child(small(format!("{key}-checked"), "已核对", Do::Checked(u), cx).tooltip(ui::tip(t("译文已经合乎改后的原文：去掉这个标记")))),
                    )
            }
        };
        card.into_any_element()
    }

    /// Header of the translation half: target language selector, live translation/review progress, prominent `导出译稿` button, and `⋯`.
    fn render_translate_head(&self, version: &Version, code: &str, half: f32, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let translate = &lib.writing.translate;
        let small = |text: String| div().flex_none().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(text);
        let language = ui::ghost_button("write-translate-language", None, None)
            .flex_none()
            .h(text_px(24.))
            .px(px(6.))
            .gap(px(4.))
            .text_size(text_px(12.))
            .font_weight(FontWeight::MEDIUM)
            .child(label(&LANGUAGES, code).to_string())
            .child(ui::icon("chevron-down", 11., Theme::text_tertiary()))
            .on_click(self.on_menu(MenuKind::TranslateLanguage, cx));
        let (untranslated, changed) = version.left(|section| lib.writing.facing_holds(section));
        let (reviewed, total_units) = version.reviewed(|section| lib.writing.facing_holds(section));
        let run = translate.run.as_ref().filter(|run| run.manuscript == lib.writing.manuscript && run.language == code && run.run.batch.running());
        let mut status = div().flex().flex_1().min_w_0().items_center().justify_end().gap(px(5.));
        if let Some(run) = run {
            let batch = &run.run.batch;
            status = status
                .child(ui::spinner("write-translate-busy", 12., Theme::text_tertiary()))
                .child(small(tf!("正在翻译 {} / {}", "Translating {} / {}", batch.done(), batch.total)))
                .child(ui::icon_action("write-translate-stop", "square", Theme::text_tertiary(), t("停下：已经译好的留着，其余的等你继续")).on_click(self.on(Cmd::Translate(Do::Stop), cx)));
        } else {
            let mut said = vec![];
            if untranslated > 0 { said.push(tf!("未译 {untranslated}", "{untranslated} untranslated")) }
            if changed > 0 { said.push(tf!("原文已改 {changed}", "{changed} source changed")) }
            if !said.is_empty() {
                status = status.child(div().flex_none().text_size(text_px(11.)).text_color(Theme::text_secondary()).child(said.join(" · ")));
                status = status.child(ui::icon_action("write-translate-next", "chevron-down", Theme::text_secondary(), t("下一处要看的段落")).on_click(self.on(Cmd::Translate(Do::Next), cx)));
                if translate.paused || untranslated > 0 {
                    status = status.child(ui::ghost_button("write-translate-retry", None, Some(t(if translate.paused { "继续" } else { "重试" }).into())).flex_none().h(text_px(22.)).px(px(6.)).text_size(text_px(12.)).on_click(self.on(Cmd::Translate(Do::Retry), cx)))
                }
            } else if total_units > 0 {
                status = status.child(
                    div()
                        .id("write-translate-reviewed-count")
                        .flex_none()
                        .text_size(text_px(11.))
                        .text_color(if reviewed == total_units { Theme::success() } else { Theme::text_tertiary() })
                        .tooltip(ui::tip(t("你手动修改过或点过「标为已审校」的段落数")))
                        .child(tf!("已审校 {reviewed} / {total_units}", "Reviewed {reviewed} / {total_units}")),
                );
            }
        }
        let export = ui::ghost_button("write-translate-export", Some("export"), Some(t("导出译稿").into()))
            .flex_none()
            .h(text_px(24.))
            .px(px(7.))
            .gap(px(4.))
            .text_size(text_px(12.))
            .text_color(Theme::accent())
            .tooltip(ui::tip(t("导出当前译稿（Word / Markdown / 复制全文）或中英双语对照审阅稿")))
            .child(ui::icon("chevron-down", 10., Theme::accent()))
            .on_click(self.on_menu(MenuKind::TranslateExport, cx));
        let more = ui::icon_action("write-translate-more", "ellipsis", Theme::text_tertiary(), t("只看译稿、设为主稿、搁置的译文")).on_click(self.on_menu(MenuKind::TranslateMore, cx));
        div().flex().flex_none().items_center().gap(px(4.)).w(px(half)).child(small(t("译稿").to_string())).child(language).child(status).child(export).child(more).into_any_element()
    }

    /// The `wtranslate:` debug steps (`dev.rs`).
    #[cfg(debug_assertions)]
    pub(crate) fn dev_translate(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (step, rest) = arg.split_once(';').unwrap_or((arg, ""));
        let at = || rest.parse::<usize>().unwrap_or(0);
        let what = match step {
            "open" => Do::Open,
            "edit" => Do::Edit(at()),
            "nextedit" => Do::EditNext,
            "fixcites" => Do::FixCites(at()),
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
            "look" => Do::Look(rest.to_string()),
            "place" => Do::Place,
            // Undo in the field of the block edited, as ⌘Z there does.
            "undo" => {
                if let Some(lib) = self.lib.as_ref() { lib.writing.translate.editor.update(cx, |input, cx| input.undo_last(cx)) }
                return;
            }
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
