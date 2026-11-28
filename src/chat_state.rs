//! A conversation as the window holds it: `Chat`, the items it shows (`ChatItem`), what a question carries
//! (pictures, passages of a PDF) and the tool calls waiting for the person's yes. A library has three
//! (`Talk`); sending a question and taking the answer is `chat_run.rs`, drawing is `chat.rs`.

use crate::i18n::t;
use crate::input::TextInput;
use gpui::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use serde_json::Value;
use wisp_core::ai::{self, Protocol};
use wisp_core::store::ConversationInfo;
use wisp_core::{compact, usage};

/// The pictures one question may carry.
pub const MAX_ATTACHMENTS: usize = 8;
/// Passages of a PDF a question may carry.
pub const MAX_QUOTES: usize = 6;

/// A picture in a conversation: as it is drawn and sent, and the id it is kept under.
#[derive(Clone)]
pub struct ChatImage {
    pub id: String,
    pub image: Arc<Image>,
}

/// The pictures as a request carries them.
pub(crate) fn wire_images(images: &[ChatImage]) -> Vec<ai::ImageData> {
    images.iter().map(|picture| ai::ImageData { media_type: picture.image.format.mime_type().to_string(), data: ai::base64(&picture.image.bytes) }).collect()
}

/// Where a picture to attach comes from: the clipboard, or a file still to be read.
pub enum Picture {
    Bytes(Vec<u8>),
    File(PathBuf),
}

/// What a conversation shows, in order.
pub enum ChatItem {
    /// A question, as typed, with the pictures that went with it, and
    /// what went to the model where that is not the text (a slash command
    /// written out, passages from a PDF put before it).
    User { text: String, images: Vec<ChatImage>, prompt: String, files: Vec<String> },
    /// Text of the model; one answer may come in several pieces with tool calls between them.
    Answer(String),
    /// A tool the model called, and what came back (summary, failed) once it has run.
    Tool { call: String, result: Option<(String, bool)> },
}

/// What the person says to the assistant wanting to change the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consent {
    Once,
    /// This and every later change in the same conversation.
    Always,
    Deny,
}

/// A tool call made ready to run.
pub struct PreparedCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) args: Result<Value, String>,
    pub(crate) writes: bool,
    /// The person is asked before it runs (`ToolDef::confirm`).
    pub confirm: bool,
    /// The call as the window words it.
    pub line: String,
    /// What it would change, a line per paper (`tools::preview_call`), for the card that asks.
    pub details: Vec<String>,
    /// Not a tool of the library: a connector's, or a skill being read.
    pub outside: Option<crate::custom::Outside>,
}

impl PreparedCall {
    /// Running it may change something: a tool that writes the library or a new file (`ToolDef::writes`), or a
    /// connector's that is not read-only, which is what its `confirm` says (`outside_call`; its `writes` is always false).
    pub(crate) fn changes(&self) -> bool {
        self.writes || (self.confirm && matches!(self.outside, Some(crate::custom::Outside::Connector(..))))
    }

    /// What stopping says to it once it was handed to run (`execute_tools`, with the person's yes or not: `allowed`).
    /// One that runs nothing, refused or not runnable as written, did not run; the others run on to their end.
    pub(crate) fn unanswered(&self, allowed: bool) -> Unanswered {
        if self.args.is_err() || (self.confirm && !allowed) { Unanswered::NotRun } else if self.changes() { Unanswered::Changing } else { Unanswered::Reading }
    }
}

/// What stopping a question says to a call of its turn that has no result yet (`stop_chat`): the model reads it as the
/// call's result. Every call gets one: a conversation that ends on an unanswered call cannot continue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unanswered {
    /// It never ran: it waited for the person's yes or for where its file goes, was refused, or could not run as written.
    NotRun,
    /// It runs on and only reads.
    Reading,
    /// It runs on and changes something (the library, a file, a connector's service): by now it may have.
    Changing,
}

impl Unanswered {
    pub(crate) fn result(self) -> &'static str {
        match self {
            Unanswered::NotRun => t("用户中止了这次提问，这个工具没有执行。"),
            Unanswered::Reading => t("用户中止了这次提问，这个工具的结果没有交回。"),
            Unanswered::Changing => t("用户中止了这次提问，这个工具的结果没有交回；它可能已经生效，重做之前请先核对。"),
        }
    }
}

/// The calls of the turn being answered that have no result yet, and what stopping says to each: those waiting for the
/// person's yes never ran, the others are as `Chat::running` holds them. Each call once, by its id.
pub(crate) fn unanswered(pending: Option<PendingTools>, running: Vec<(String, Unanswered)>) -> Vec<(String, Unanswered)> {
    let waiting = pending.into_iter().flat_map(|pending| pending.calls).map(|call| (call.id, Unanswered::NotRun));
    let mut owed: Vec<(String, Unanswered)> = vec![];
    for (id, said) in waiting.chain(running) {
        if !owed.iter().any(|(known, _)| *known == id) { owed.push((id, said)) }
    }
    owed
}

/// The calls of a turn that wait for the person's consent, and where their lines stand in the conversation.
pub struct PendingTools {
    pub calls: Vec<PreparedCall>,
    pub(crate) first: usize,
}

/// A passage selected in a paper's PDF, to go with the next question.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatQuote {
    pub paper_id: String,
    /// The file it was selected in: the paper's main text or an attachment.
    pub file_id: String,
    /// The 1-based page it stands on.
    pub page: usize,
    pub text: String,
}

/// One conversation: with the library (the current list) or with one paper.
pub struct Chat {
    pub input: Entity<TextInput>,
    pub messages: Vec<ChatItem>,
    /// The conversation as the model's protocol carries it: questions, the
    /// model's turns as it sent them, tool results. Only ever appended to.
    pub(crate) wire: Vec<Value>,
    /// The protocol `wire` is written in.
    pub(crate) protocol: Option<Protocol>,
    /// Requests made for the question being answered.
    pub steps: usize,
    /// What the model was given, built with the first question.
    pub ctx: Option<ai::Context>,
    /// The tools its requests carry, settled when a question is asked and the same for every step of its answer
    /// (`ai::Table`: it only grows, until the context is built anew).
    pub(crate) table: ai::Table,
    pub busy: bool,
    pub error: String,
    /// Why the last answer ended early, if it did; while one is awaited, what it is waiting for.
    pub note: String,
    pub scroll: ScrollHandle,
    /// The question box's own scroll area: a long question scrolls inside it, the caret kept in sight (`TextInput::follow`).
    pub input_scroll: ScrollHandle,
    /// The answers (by their place in `messages`) whose sources are all on show.
    pub sources_open: HashSet<usize>,
    /// Bumped by 新对话 and by stopping, so what still arrives for the old run is dropped.
    pub generation: u64,
    /// The conversation in the library's history this is; empty until a question was asked.
    pub id: String,
    /// The conversation as it stands is in the library's history.
    pub(crate) kept: bool,
    pub(crate) created_at: String,
    /// What the labels of a conversation taken up again name, until its context is built anew.
    pub(crate) known: Vec<ai::Source>,
    /// The pictures already kept with the conversation.
    pub(crate) stored_images: HashSet<String>,
    /// Passages selected in a PDF that go with the next question.
    pub quotes: Vec<ChatQuote>,
    /// Pictures that go with the next question, and how many more are still being read.
    pub attachments: Vec<ChatImage>,
    pub attaching: usize,
    /// Files that go with the next question, by their paths: Word files, for the assistant to restyle their citations
    /// (`wisp_core::restyle`), and PowerPoint files, the reference slides are made after (`wisp_core::slideref`).
    pub documents: Vec<PathBuf>,
    /// Which of the slash commands on offer is picked.
    pub slash: usize,
    /// The list of commands was closed with Escape; typing opens it again.
    pub(crate) slash_closed: bool,
    /// What the `@` being typed in the box is offered, and how its mentions are drawn (`mention.rs`): the box's, like `input`.
    pub mentions: crate::mention::Mentions,
    /// The past conversations, while their list is on show.
    pub history: Option<Vec<ConversationInfo>>,
    /// The search over that list, the place's like the box; and the conversations holding what is typed in it
    /// (`Store::conversations_saying`), None while it is empty.
    pub history_find: Entity<TextInput>,
    pub history_hits: Option<HashSet<String>>,
    /// The tool calls waiting for the person's consent.
    pub pending: Option<PendingTools>,
    /// The person allowed every change for the rest of this conversation.
    pub(crate) allow_changes: bool,
    /// The calls of the turn once they left `pending`, waiting for where a file goes (`place_outputs`) or running
    /// (`execute_tools`), with what each is told if the person stops (`unanswered`).
    pub(crate) running: Vec<(String, Unanswered)>,
    /// The groups of tool lines (by the place of their first) that are unfolded.
    pub tools_open: HashSet<usize>,
    /// The list of what the model was (or will be) given is unfolded.
    pub materials_open: bool,
    /// The review this conversation writes, as last read from the library (`Library::load_review`); only the conversation with the library has one.
    pub review: Option<wisp_core::review::Review>,
    /// The quotations of the review's evidence looked up in their papers, by the row's id.
    pub review_checks: HashMap<String, ai::Quoted>,
    /// The review's record is on show in place of the conversation.
    pub review_open: bool,
    /// How many messages there were when the review last took a step: an answer from before it is not offered to the next step.
    pub review_mark: usize,
    /// What is selected in an answer: the answer's place among the messages, the two ends in order of selecting, and whether the pointer is still held.
    pub picked: Option<(usize, crate::markdown::Place, crate::markdown::Place, bool)>,
    /// Where that selection was last drawn, and how big the bar of actions beside it came out (`chat::Spot`).
    pub spot: Option<crate::chat::Spot>,
    pub bar_size: Option<Size<Pixels>>,
    /// The quotations of the answers looked up in the papers they name, by label, page and quotation.
    pub checks: HashMap<(String, Option<usize>, Option<usize>, String), ai::Quoted>,
    /// When the question being answered was asked.
    pub started: Option<Instant>,
    /// What the last 撤回 took away, until the next question: it can be put back (`Cmd::Unretract`).
    pub retracted: Option<Retracted>,
    /// How wide the conversation was last drawn: its column of the reading width is centred in it (`chat::reading_pad`).
    pub drawn_width: f32,
    /// Which conversation this is while the window runs: what a step of its answer finds it by, in its place or set aside (`Workspace::in_chat`).
    pub key: u64,
    /// The card the assistant put under its answer (`offer_action`), with how many messages there were then: it is drawn while no question came
    /// after it, and goes once a button of it is pressed or it is put away (`Cmd::Offer`). Not kept with the conversation.
    pub offer: Option<(usize, wisp_core::offer::Offer)>,
    /// The summary the model is given in place of the first of `messages`, once the conversation outgrew its window or the person asked
    /// (`/compact`, `compact_chat`). The person still sees every message; a line marks where the summary stands.
    pub compacted: Option<compact::Compaction>,
    /// That summary is on show under its line.
    pub compacted_open: bool,
    /// A summary of the earlier conversation is being written.
    pub compacting: bool,
    /// No summary could be had for the question being answered: the oldest turns are left out instead.
    pub(crate) no_summary: bool,
    /// How the service's own count of a request compares with the estimate of it (`ChatEvent::Used`): the next estimate is scaled by it.
    /// It is the model's and the estimate's, not the conversation's: a new conversation keeps it.
    pub(crate) calibration: f32,
    /// The estimate of the request in flight.
    pub(crate) sent_estimate: usize,
    /// The service refused the last request as too long, in these words: the next step compacts whatever the estimate says.
    pub(crate) overflow: Option<String>,
    /// That was already tried for the question being answered.
    pub(crate) squeezed: bool,
    /// What the question last asked used, where the service says what it read from its cache (DeepSeek): the line under the answer.
    pub spent: Option<Spent>,
    /// Per-conversation thinking effort override (`None` follows the default in Settings › AI).
    pub effort: Option<String>,
}

/// What a question used, over all the requests it took: the service's own counts (`wisp_core::usage`), about what
/// that cost at its listed prices, and what the account holds, asked once the answer is in. Not kept with the conversation.
#[derive(Default)]
pub struct Spent {
    pub usage: usage::Usage,
    pub requests: usize,
    /// In US dollars; `None` for a model without a listed price.
    pub cost: Option<f64>,
    pub balance: Option<usage::Balance>,
}

/// What an item of a conversation is kept as; nothing for an answer piece without text.
pub(crate) fn said(item: &ChatItem) -> Option<ai::Said> {
    match item {
        ChatItem::User { text, images, prompt, files } => Some(ai::Said::User { text: text.clone(), images: images.iter().map(|picture| picture.id.clone()).collect(), files: files.clone(), prompt: prompt.clone() }),
        ChatItem::Answer(text) if !text.is_empty() => Some(ai::Said::Answer { text: text.clone() }),
        ChatItem::Answer(_) => None,
        ChatItem::Tool { call, result } => {
            let (result, failed) = result.clone().unwrap_or_default();
            Some(ai::Said::Tool { call: call.clone(), result, failed })
        }
    }
}

/// Where `Chat::key` comes from.
static CHAT_KEYS: AtomicU64 = AtomicU64::new(1);

/// Put `other` in a conversation's place and the one there in `other`. The box stays where it is: it is the place's, heard by the window.
pub(crate) fn exchange(place: &mut Chat, other: &mut Chat) {
    std::mem::swap(place, other);
    std::mem::swap(&mut place.input, &mut other.input);
    std::mem::swap(&mut place.mentions, &mut other.mentions);
    std::mem::swap(&mut place.history_find, &mut other.history_find);
}

/// What 撤回 took out of a conversation, and what the box held before.
pub struct Retracted {
    /// The questions and answers taken away, in order.
    pub(crate) tail: Vec<ChatItem>,
    /// The box's text and pictures before the question came back into it.
    pub(crate) draft: String,
    pub(crate) attachments: Vec<ChatImage>,
}

impl Retracted {
    pub fn questions(&self) -> usize {
        self.tail.iter().filter(|item| matches!(item, ChatItem::User { .. })).count()
    }
}

impl Chat {
    pub(crate) fn new(placeholder: &'static str, cx: &mut App) -> Chat {
        let input_scroll = ScrollHandle::new();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(placeholder, cx).multiline(1);
            input.submit_on_enter = true;
            input.accepts_images = true;
            input.follow = Some(input_scroll.clone());
            input
        });
        Chat {
            input,
            messages: vec![],
            wire: vec![],
            protocol: None,
            steps: 0,
            ctx: None,
            table: ai::Table::default(),
            busy: false,
            error: String::new(),
            note: String::new(),
            scroll: ScrollHandle::new(),
            input_scroll,
            sources_open: Default::default(),
            generation: 0,
            id: String::new(),
            kept: false,
            created_at: String::new(),
            known: vec![],
            stored_images: Default::default(),
            quotes: vec![],
            attachments: vec![],
            attaching: 0,
            documents: vec![],
            slash: 0,
            slash_closed: false,
            mentions: Default::default(),
            history: None,
            history_find: cx.new(|cx| TextInput::new("找对话：标题、文献、说过的话", cx)),
            history_hits: None,
            pending: None,
            allow_changes: false,
            running: vec![],
            tools_open: Default::default(),
            materials_open: false,
            review: None,
            review_checks: HashMap::new(),
            review_open: false,
            review_mark: 0,
            picked: None,
            spot: None,
            bar_size: None,
            checks: HashMap::new(),
            started: None,
            retracted: None,
            drawn_width: 0.,
            key: CHAT_KEYS.fetch_add(1, Ordering::Relaxed),
            offer: None,
            compacted: None,
            compacted_open: false,
            compacting: false,
            no_summary: false,
            calibration: 1.,
            sent_estimate: 0,
            overflow: None,
            squeezed: false,
            spent: None,
            effort: None,
        }
    }

    /// What the labels in the answers name.
    pub fn sources(&self) -> &[ai::Source] {
        self.ctx.as_ref().map_or(&self.known, |ctx| &ctx.sources)
    }

    /// The ends of what is selected in an answer, the earlier first.
    pub fn picked_range(&self) -> Option<(usize, crate::markdown::Place, crate::markdown::Place)> {
        let (at, from, to, _) = self.picked?;
        Some(if to < from { (at, to, from) } else { (at, from, to) })
    }

    /// The text selected in an answer.
    pub fn picked_text(&self) -> Option<String> {
        let (at, from, to) = self.picked_range()?;
        let ChatItem::Answer(answer) = self.messages.get(at)? else { return None };
        let text = crate::markdown::text_between(answer, from, to);
        (!text.trim().is_empty()).then_some(text)
    }

    /// The slash commands to offer for what is in the box right now.
    /// `own` are the person's commands from the settings.
    pub fn slash_list(&self, own: &[ai::Command], skills: &[wisp_core::skill::Skill], cx: &App) -> Vec<ai::Offer> {
        if self.slash_closed { vec![] } else { ai::slash_matches(self.input.read(cx).text(), own, skills) }
    }

    /// Have the context built again with the next question. The labels
    /// stay what they were: the answers already given use them.
    pub(crate) fn forget_context(&mut self) {
        if let Some(ctx) = self.ctx.take() { self.known = ctx.sources }
        // The texts may have changed: the quotations are looked up again.
        self.checks.clear();
    }

    /// An answer piece that never got any text leaves no empty bubble behind.
    pub(crate) fn drop_empty_answer(&mut self) {
        if matches!(self.messages.last(), Some(ChatItem::Answer(text)) if text.is_empty()) { self.messages.pop(); }
    }

    /// What was said from this place on is gone. The model's turns in
    /// `wire` belong to the conversation as it was: it is written again
    /// from what is left (as `ask` replays) with the next question.
    pub(crate) fn cut(&mut self, at: usize) -> Vec<ChatItem> {
        let tail = self.messages.split_off(at.min(self.messages.len()));
        self.sources_open.retain(|&i| i < at);
        self.tools_open.retain(|&i| i < at);
        self.review_mark = self.review_mark.min(at);
        self.picked = None;
        self.spot = None;
        self.error.clear();
        self.note.clear();
        self.history = None;
        self.wire.clear();
        self.protocol = None;
        // A summary that stood for more than is left stands for nothing: what is left is sent as it was said.
        if self.compacted.as_ref().is_some_and(|compacted| compacted.at > at) {
            self.compacted = None;
            self.compacted_open = false;
        }
        tail
    }

    /// The place among `messages` of the n-th question from `from` on; their end when there are fewer.
    pub(crate) fn question_at(&self, from: usize, n: usize) -> usize {
        self.messages.iter().enumerate().skip(from).filter(|(_, item)| matches!(item, ChatItem::User { .. })).nth(n).map_or(self.messages.len(), |(i, _)| i)
    }

    /// The question has been answered, or given up on.
    pub(crate) fn end_run(&mut self) {
        self.busy = false;
        self.compacting = false;
        self.overflow = None;
        self.started = None;
        self.pending = None;
        self.running.clear();
        self.drop_empty_answer();
        if self.note.is_empty() && self.error.is_empty() && !matches!(self.messages.last(), Some(ChatItem::Answer(_))) { self.note = t("模型没有返回内容。").into() }
    }

    /// The conversation as it is kept in the library: what was said, and
    /// the papers it names, so the labels still lead somewhere later (`Transcript::of`).
    pub(crate) fn transcript(&self) -> ai::Transcript {
        let items = self.messages.iter().filter_map(said).collect();
        // Where the summary stands is counted in what is kept: an answer piece without text is not.
        let compacted = self.compacted.clone().map(|mut compacted| {
            compacted.at = self.messages[..compacted.at.min(self.messages.len())].iter().filter_map(said).count();
            compacted
        });
        ai::Transcript::of(items, self.sources(), compacted)
    }

    pub(crate) fn reset(&mut self, cx: &mut App) {
        self.messages.clear();
        self.wire.clear();
        self.protocol = None;
        self.steps = 0;
        self.ctx = None;
        self.busy = false;
        self.error.clear();
        self.note.clear();
        self.sources_open.clear();
        self.generation += 1;
        self.id.clear();
        self.kept = false;
        self.created_at.clear();
        self.known.clear();
        self.stored_images.clear();
        self.quotes.clear();
        self.attachments.clear();
        self.attaching = 0;
        self.documents.clear();
        self.slash = 0;
        self.slash_closed = false;
        self.mentions = Default::default();
        self.history = None;
        self.pending = None;
        self.allow_changes = false;
        self.running.clear();
        self.tools_open.clear();
        self.review = None;
        self.review_checks.clear();
        self.review_open = false;
        self.review_mark = 0;
        self.picked = None;
        self.spot = None;
        self.checks.clear();
        self.started = None;
        self.retracted = None;
        self.compacted = None;
        self.compacted_open = false;
        self.compacting = false;
        self.no_summary = false;
        self.overflow = None;
        self.squeezed = false;
        self.spent = None;
        self.effort = None;
        self.input.update(cx, |input, cx| {
            input.choosing = false;
            input.set_text("", cx);
        });
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's own `test` attribute.
    use super::{PendingTools, PreparedCall, Unanswered, unanswered};
    use crate::custom::Outside;
    use serde_json::Value;
    use wisp_core::connector::Connector;

    fn call(id: &str, writes: bool, confirm: bool, outside: Option<Outside>) -> PreparedCall {
        PreparedCall { id: id.into(), name: String::new(), args: Ok(Value::Null), writes, confirm, line: String::new(), details: vec![], outside }
    }

    // As `outside_call` makes them: a connector's tool that is not read-only is asked about, a read-only one is not.
    fn connector(id: &str, read_only: bool) -> PreparedCall {
        call(id, false, !read_only, Some(Outside::Connector(Connector::default(), "pages.create".into())))
    }

    #[test]
    fn a_running_call_that_reads_is_not_said_to_have_changed_anything() {
        assert_eq!(call("a", false, false, None).unanswered(true), Unanswered::Reading);
        assert_eq!(connector("b", true).unanswered(true), Unanswered::Reading);
        // A skill being read.
        assert_eq!(call("c", false, false, Some(Outside::Said(Ok("说明".into())))).unanswered(true), Unanswered::Reading);
    }

    #[test]
    fn a_running_call_that_writes_may_have_taken_effect() {
        // A library tool that writes, asked about and allowed, or not asked about (the pen handed over, a file placed in its dialog).
        assert_eq!(call("a", true, true, None).unanswered(true), Unanswered::Changing);
        assert_eq!(call("b", true, false, None).unanswered(false), Unanswered::Changing);
        // A connector's that is not read-only: its `writes` is false, its `confirm` says it.
        assert_eq!(connector("c", false).unanswered(true), Unanswered::Changing);
    }

    #[test]
    fn a_call_that_runs_nothing_did_not_run() {
        // Refused: it is answered with the refusal and does nothing.
        assert_eq!(call("a", true, true, None).unanswered(false), Unanswered::NotRun);
        assert_eq!(connector("b", false).unanswered(false), Unanswered::NotRun);
        // Arguments that did not parse, or a save dialog closed (`place_outputs`): it fails by itself.
        let broken = PreparedCall { args: Err("用户关掉了保存对话框".into()), ..call("c", true, false, None) };
        assert_eq!(broken.unanswered(true), Unanswered::NotRun);
    }

    #[test]
    fn every_call_waiting_or_running_is_answered_once() {
        // Waiting for the person's yes: none of them ran, the one that only reads included.
        let pending = PendingTools { calls: vec![call("a", true, true, None), call("b", false, false, None)], first: 0 };
        assert_eq!(unanswered(Some(pending), vec![]), vec![("a".into(), Unanswered::NotRun), ("b".into(), Unanswered::NotRun)]);
        // Waiting for a save dialog (`place_outputs`), then running (`execute_tools`): as `running` holds them.
        assert_eq!(unanswered(None, vec![("a".into(), Unanswered::NotRun), ("b".into(), Unanswered::NotRun)]), vec![("a".into(), Unanswered::NotRun), ("b".into(), Unanswered::NotRun)]);
        assert_eq!(unanswered(None, vec![("a".into(), Unanswered::Changing), ("b".into(), Unanswered::Reading)]), vec![("a".into(), Unanswered::Changing), ("b".into(), Unanswered::Reading)]);
        // A call in both is answered once, as one that did not run: two results for one call are refused as surely as none.
        let pending = PendingTools { calls: vec![call("a", true, true, None)], first: 0 };
        assert_eq!(unanswered(Some(pending), vec![("a".into(), Unanswered::Changing), ("c".into(), Unanswered::Reading)]), vec![("a".into(), Unanswered::NotRun), ("c".into(), Unanswered::Reading)]);
        assert!(unanswered(None, vec![]).is_empty());
    }

    #[test]
    fn each_way_of_being_unanswered_says_something_else() {
        let said = [Unanswered::NotRun, Unanswered::Reading, Unanswered::Changing].map(Unanswered::result);
        assert!(said[0] != said[1] && said[1] != said[2] && said[0] != said[2]);
    }
}
