//! `Workspace`, the window's root view: the home page of libraries, or one
//! open library (`Library`). All state lives in these two; every click becomes a
//! `Cmd` handled by `Workspace::run` (`commands.rs`), and the other modules are
//! `impl Workspace` blocks, each for one part of the window: what it runs or
//! what it draws. Here are the state itself, opening and leaving a library,
//! selecting a paper, and the one way a change is written (`Workspace::change`).

use crate::i18n::t;
use crate::input::{InputEvent, TextInput};
use crate::pdf::{PdfView, Zoom};
use crate::settings::{SettingsView, UpdateState};
use crate::shortcuts;
use crate::theme;
use gpui::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use serde_json::Value;
use wisp_core::ai;
use wisp_core::filter::{self, Filters};
use wisp_core::libraries::{self, LibraryInfo};
use wisp_core::model::*;
pub(crate) use wisp_core::settings::default_root;
use wisp_core::settings::{Session, Settings, session_file, settings_file};
use wisp_core::store::{ConversationInfo, ImportBatch, SavedSearch, Store, StructureInfo, Suggestion, TrashEntry};
use wisp_core::manuscript::Manuscript;
use wisp_core::{archive, export, time};

// What the window holds is defined with the code that runs it; its names are passed on here, so the rest of
// the crate takes them from `crate::app` wherever they are written.
pub use crate::chat_state::{Chat, ChatImage, ChatItem, Consent, Picture, Spent};
pub use crate::commands::{Cmd, ExportKind, FilterKey, ReviewDo, ReviewText, ScopeDo};
pub use crate::fetch::{FetchRow, FetchRun};
pub use crate::guide::Guide;
pub use crate::importing::{ImportRow, ImportRun, Imported, ZoteroRun};
pub(crate) use crate::listening::{podcast_files, podcast_list};
pub use crate::menus::{Menu, MenuItem, MenuKind, menu_place};
pub use crate::modal::Modal;
pub use crate::screen_run::{Confirmed, Screen, ScreenMode};
pub use crate::sorting::Sorting;
pub use crate::translate::Translated;
pub use crate::viewers::{Beside, MAX_PINS, PaperWindow, Pinned};

/// What the middle of an open library shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Papers,
    /// 助手: the conversation with the library, over the current list.
    Ask,
    Trash,
    /// 筛选: the papers waiting for a decision, with what the assistant suggests for each.
    Screen,
    /// The guide of a new library: where to start, and searches composed with the assistant.
    Guide,
    /// 检索与导入: saved searches for the literature databases and what they brought in.
    Sources,
    /// 写作: the manuscript, and beside it how the library's papers put the same part.
    Write,
    /// 图片库: the figures cut out of the library's PDFs, the other half of 收藏 (`src/figures.rs`).
    Figures,
    /// 提取: what the assistant proposes for the custom fields, looked over and written together (`src/extract.rs`).
    Extract,
}

impl Page {
    /// The word a page goes by where it is written down: `session.json`, a `page:` step of a scripted start.
    pub fn key(self) -> &'static str {
        match self {
            Page::Papers => "papers",
            Page::Ask => "ask",
            Page::Trash => "trash",
            Page::Screen => "screen",
            Page::Guide => "guide",
            Page::Sources => "sources",
            Page::Write => "write",
            Page::Figures => "figures",
            Page::Extract => "extract",
        }
    }

    /// The page of that word; the papers for any other.
    pub fn from_key(key: &str) -> Page {
        [Page::Ask, Page::Trash, Page::Screen, Page::Guide, Page::Sources, Page::Write, Page::Figures, Page::Extract].into_iter().find(|page| page.key() == key).unwrap_or(Page::Papers)
    }
}

/// An edge between two columns of a library that the pointer can hold and move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Between the sidebar and the list.
    Sidebar,
    /// Between the list and the detail panel.
    Detail,
    /// Between a paper's conversation and its PDF.
    Split,
    /// Between the conversation that writes and its manuscript (`write_agent.rs`).
    Manuscript,
    /// Between two columns of the paper table (`Workspace::column_drag` says which).
    Column,
    /// Between the two halves of a split PDF, while they are 自由 (`Library::halves_drag`).
    Halves,
}

/// The edge between two columns of the paper table, as it was when the pointer took it.
pub struct ColumnDrag {
    /// Where the pointer was.
    pub x: f32,
    /// The column at the edge's left with its width, unless that is the title, which has what the others leave.
    pub left: Option<(String, f32)>,
    /// The column at its right.
    pub right: (String, f32),
    /// How much the title can still give up.
    pub spare: f32,
}

/// Which of a library's conversations: the one with the library (助手), the one about the selected paper,
/// or the one that writes the manuscript on the writing page (助手写, `write_agent.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Talk {
    Library,
    Paper,
    Write,
}

impl Talk {
    /// A word for the ids of a conversation's elements: more than one conversation can be on screen.
    pub fn side(self) -> &'static str {
        match self {
            Talk::Library => "library",
            Talk::Paper => "paper",
            Talk::Write => "write",
        }
    }
}

/// The conversation `$talk` names, borrowed alone: the rest of the library can be read meanwhile.
macro_rules! chat_of {
    ($lib:expr, $talk:expr) => {
        match $talk {
            $crate::app::Talk::Library => &mut $lib.library_chat,
            $crate::app::Talk::Paper => &mut $lib.paper_chat,
            $crate::app::Talk::Write => &mut $lib.write_chat,
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Info,
    Pdf,
    Ask,
}

impl Tab {
    /// As `Page::key`.
    pub fn key(self) -> &'static str {
        match self {
            Tab::Info => "info",
            Tab::Pdf => "pdf",
            Tab::Ask => "ask",
        }
    }

    pub fn from_key(key: &str) -> Tab {
        [Tab::Pdf, Tab::Ask].into_iter().find(|tab| tab.key() == key).unwrap_or(Tab::Info)
    }
}

pub struct Toast {
    pub message: SharedString,
    pub error: bool,
    pub at: Instant,
    /// What was just done can be taken back: the word on the button and what it runs.
    pub action: Option<(SharedString, Cmd)>,
}

/// A row of the list of papers: a paper on show (its place in `Library::papers`) or, under a paper unfolded, one
/// of its files (its place in `Library::files`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Row {
    Paper(usize),
    File(usize),
}
/// How many papers 后退 remembers.
const HISTORY: usize = 50;

/// The writing page: the manuscript and section being written, and the
/// sentences the library's papers have in the same part. `Writing::kind`
/// is that part, `Writing::language` what the manuscript is written in
/// (both in `src/manuscripts.rs`).
pub struct Writing {
    /// The library's manuscripts, oldest first; a section's text is kept here as it is typed, as in the store.
    pub manuscripts: Vec<Manuscript>,
    /// The manuscript being written: its id.
    pub manuscript: String,
    /// The section being written: its id.
    pub section_id: String,
    /// The whole manuscript is shown to be read, instead of one section to write.
    pub reading: bool,
    /// The manuscript's title, edited in place.
    pub title: Entity<TextInput>,
    /// Counts the changes typed, so a pause is the timer of the last one still being the last.
    pub(crate) typed: u64,
    pub editor: Entity<TextInput>,
    /// Words to look for, instead of what is being written.
    pub find: Entity<TextInput>,
    /// The candidates: what was asked for, what was found (write_find.rs).
    pub search: crate::write_find::CandidateSearch,
    /// How each full text was sorted into parts, by file id.
    pub structures: BTreeMap<String, StructureInfo>,
    pub sorting: Sorting,
    pub scroll: ScrollHandle,
    /// The model's work on a paragraph (`write_assist.rs`).
    pub assist: crate::write_assist::Assisting,
    /// What the assistant that writes changed in the section on show, lit and listed (`revise.rs`).
    pub revise: crate::revise::Revising,
    /// The comments on the manuscript, and the assistant's review (`comments.rs`).
    pub comments: crate::comments::Commenting,
    /// Where the manuscript's editor is scrolled to.
    pub editor_scroll: ScrollHandle,
    /// Which page the panel beside the manuscript shows.
    pub side: Side,
    /// Whether that panel is open where it stands under the manuscript, in a page too narrow for both side by side: folded to its tabs until asked for.
    pub side_open: bool,
    /// The other mode, where the assistant writes: which manuscript its conversation is about, what it changed, the versions to step through (`write_agent.rs`).
    pub agent: crate::write_agent::Agent,
    /// Looking for words in the manuscript (`write_seek.rs`).
    pub seek: crate::write_seek::Seek,
    /// Beside a draft's conversation the manuscript was closed: the conversation has the page until one is shown again (`write_show`).
    pub away: bool,
}

/// The pages of the panel beside the manuscript: what there is to draw on, what was said about the text, what it cites.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    #[default]
    Candidates,
    Comments,
    References,
}

pub struct Library {
    pub info: LibraryInfo,
    pub store: Store,
    pub papers: Vec<Paper>,
    pub fields: Vec<FieldDef>,
    pub files: Vec<PaperFile>,
    pub trash: Vec<TrashEntry>,
    /// 筛选's rounds, oldest first (`Store::begin_round`).
    pub rounds: Vec<wisp_core::store::ScreeningRound>,
    pub filters: Filters,
    /// Indices into `papers` of the rows shown, in order.
    pub shown: Vec<usize>,
    /// The rows of the list as it is drawn: the papers of `shown`, each unfolded one followed by its files (`lay_rows`).
    pub rows: Vec<Row>,
    /// The papers whose files are shown under their rows.
    pub unfolded: HashSet<String>,
    /// The papers that have something to unfold: an attachment, or more files than one.
    pub folding: HashSet<String>,
    /// What the sidebar counts beside its views and tags, under the search and filters set (`filter::counts`).
    pub counts: filter::Counts,
    pub search: Entity<TextInput>,
    /// What the search box holds looked for through the whole library, on show in the list's place (`library_find.rs`).
    pub finding: crate::library_find::Finding,
    pub page: Page,
    pub selected: Option<String>,
    /// The papers marked in the list with Shift or ⌘ / Ctrl, for doing something with all of them.
    pub marked: Vec<String>,
    /// Where a stretch marked with Shift starts, and where it reaches to.
    pub anchor: Option<String>,
    pub cursor: Option<String>,
    /// The papers chosen in the trash, kept apart from `marked`; emptied when the trash is opened or
    /// left. `trash_anchor` is where a stretch chosen with Shift starts.
    pub trash_chosen: Vec<String>,
    pub trash_anchor: Option<String>,
    /// The papers the library's next conversation is kept to: picked in the list, or dragged into the
    /// conversation from the panel over it. None means the list on show, narrowed by that panel's search.
    pub asked: Vec<String>,
    /// The search box of the panel over the library's conversation, and what it holds.
    pub scope_find: Entity<TextInput>,
    pub scope_words: String,
    /// The papers of the library that hold those words; `None` while nothing is typed. Kept, since a
    /// frame cannot search every abstract: `scope_refilter` makes it anew when the words or the papers change.
    pub scope_hits: Option<HashSet<String>>,
    /// 历史 of the library's conversation and of the paper's lists every conversation of the library (全部对话), as last chosen while it is open.
    pub history_all: bool,
    pub tab: Tab,
    /// The detail panel takes most of the window (reading a PDF).
    pub wide: bool,
    /// Reading mode: the sidebar and the list are put away and the paper has the whole window.
    pub reading: bool,
    /// The paper's PDF is wanted to the right of its conversation. While the paper is asked about, the
    /// window is laid out for the two (`Workspace::asking`).
    pub split: bool,
    /// The tab the asking was come to from, which leaving it goes back to.
    pub ask_from: Tab,
    /// The page 写作 was opened from, which leaving a manuscript goes back to (`Library::enter_write`).
    pub write_from: Page,
    /// Whether the window was last drawn laid out for asking: the list finds its paper again when that changes.
    pub ask_drawn: bool,
    /// The floating conversation's window or button while the pointer holds it (floating.rs).
    pub float_drag: Option<crate::floating::Drag>,
    /// The size of the area the floating conversation keeps within, as last drawn.
    pub float_area: (f32, f32),
    pub notes: Entity<TextInput>,
    pub note_links: BTreeMap<String, String>,
    /// The viewer of the file worked in: the toolbar, the keys and a citation followed speak of it.
    pub pdf: Option<Entity<PdfView>>,
    /// 朗读: the file on show read aloud, and how it stood when last drawn. It belongs to the viewer:
    /// another file on show, or the library closed, stops it (dropping it stops the sound).
    pub aloud: Option<crate::read_aloud::Reading>,
    pub aloud_shown: Option<crate::read_aloud::Status>,
    /// The player's bar as it was last drawn (its left edge and width in the window), and where the pointer
    /// holds it while it is dragged: the place is sought on release.
    pub seek_bar: (f32, f32),
    pub seek_held: Option<f32>,
    pub(crate) pdf_events: Option<Subscription>,
    /// Another file of the paper in the other half of the viewer (#185), while there is one; 分屏 自由, the same file too.
    pub beside: Option<Beside>,
    /// 分屏 自由: the first half's share of the room the two halves have (0.5 is even); 固定 is always even.
    pub split_share: f32,
    /// The band between the halves as the pointer took it: where along the split it was, the share then, the room the halves have.
    pub halves_drag: Option<(f32, f32, f32)>,
    /// Papers pinned beside the one on show, in the order of their panes.
    pub pins: Vec<Pinned>,
    /// Papers read in windows of their own.
    pub windows: Vec<PaperWindow>,
    /// Where the paper on show stands among them: its pane is before that of `pins[main_at]`, after all of them
    /// when there is none. The paper worked in changes (`Workspace::work_in_pane`); the panes stay where they are.
    pub main_at: usize,
    /// The panes the window had room for when it was last drawn (`render_paper_area`): `panes_shown` of them from
    /// the `panes_from`-th on, the paper on show among them; the rest are behind `+N` on the last of them.
    pub panes_from: usize,
    pub panes_shown: usize,
    /// The paper on show is to stay in a pane beside when another is chosen (留在旁边, `Cmd::KeepPane`): `select` pins it then.
    pub keep: bool,
    /// The conversation's width dragged while papers are kept beside (`Workspace::pane_frame`); 0 is its share.
    /// It goes with this arrangement: the width beside one paper is the reader's habit (`Settings::chat_width`).
    pub pane_chat: f32,
    /// How far the PDF's bar gave way to stay one line (the levels `detail::BAR_ICONS` … `BAR_NO_KEEP`): what its ⋯ menu holds.
    pub bar_level: usize,
    /// The conversation whose question box takes the keyboard when it is next drawn: something from a PDF went to it
    /// (#205). Handed over where the box is drawn (`render_chat`), so a box not on screen is never given it; what is not
    /// taken in that frame is let go.
    pub focus_box: Option<Talk>,
    /// The fit PDF pages are drawn at (`Zoom::carried`): it stays from paper to paper, a percentage does not.
    pub zoom: Zoom,
    pub library_chat: Chat,
    pub paper_chat: Chat,
    /// The conversation that writes the manuscript on the writing page (`write_agent.rs`).
    pub write_chat: Chat,
    pub table_scroll: UniformListScrollHandle,
    pub detail_scroll: ScrollHandle,
    /// Files whose text is being extracted.
    pub extracting: HashSet<String>,
    /// PDFs being made smaller (`Cmd::CompressFile`).
    pub shrinking: HashSet<String>,
    /// PDFs made smaller before (`Store::shrunk_files`), and those found in this window to get no smaller:
    /// neither is offered the button again.
    pub shrunk: HashSet<String>,
    pub no_smaller: HashSet<String>,
    /// The file of each paper chosen to read, when it is not the main text (`Cmd::ReadFile`).
    pub read_file: HashMap<String, String>,
    /// Where each file was read at when another took its place or its paper was left (`PdfView::place`):
    /// read from the library's database when it opens, written there by `keep_places` (#194).
    pub file_places: HashMap<String, (usize, f32)>,
    /// What can be undone and redone of each file's marks, by the file's id (`Workspace::step_marks`): in memory, gone with the library.
    pub mark_history: HashMap<String, wisp_core::annotation::History>,
    /// The papers read before the one on show, the last one last, and those gone back from (`Workspace::step_history`).
    pub back: Vec<String>,
    pub forward: Vec<String>,
    pub guide: Guide,
    pub screen: Screen,
    /// The assistant's suggestion for each paper that has one.
    pub suggestions: HashMap<String, Suggestion>,
    /// Translations asked for in this window, by `store::translation_key`.
    pub translations: HashMap<String, Translated>,
    /// How many requests went out for each of them: the answer to an earlier one, back after a later, is dropped.
    pub translation_asks: HashMap<String, u64>,
    pub translation_run: Option<crate::translate::TranslationRun>,
    pub translation_session: Arc<()>,
    /// Whether the detail panel shows the selected paper translated. It
    /// stays on from paper to paper, so each one opened is translated.
    pub show_translation: bool,
    /// Whether the PDF tab shows the full text's translation beside the pages. Like `show_translation`
    /// it stays on from paper to paper; nothing is asked of a model until the person says so.
    pub facing: bool,
    /// The paragraphs of the file read, while `facing` is on (`Workspace::load_facing`).
    pub facing_text: Option<crate::facing::Facing>,
    /// The switch was just turned on: translate as soon as the paragraphs are read.
    pub facing_start: bool,
    /// The run translating them.
    pub facing_run: Option<crate::translate::TranslationRun>,
    /// The file `facing_run` translates: the paper on show's, or one kept beside it.
    pub facing_for: String,
    pub searches: Vec<SavedSearch>,
    /// The latest import batches, newest first.
    pub batches: Vec<ImportBatch>,
    /// The trial run on show: which search, and what it found once it is back.
    pub trial: Option<(String, Option<Result<Value, String>>)>,
    /// Searches being run right now.
    pub running: HashSet<String>,
    /// The PDFs being read in, or what came of the last lot until it is put away.
    pub import: Option<ImportRun>,
    pub zotero: Option<ZoteroRun>,
    /// The round of looking for open full texts, likewise.
    pub fetching: Option<FetchRun>,
    /// Where the fetched files came from, by file: (address, who listed it, version).
    pub origins: BTreeMap<String, (String, String, String)>,
    pub writing: Writing,
    pub citing: crate::cite::Citing,
    /// 图片库: the figures cut out of the library's PDFs.
    pub gallery: crate::figures::Gallery,
    /// 写作's 资料库, and the pictures the manuscript on show draws (`write_figures.rs`).
    pub write_figures: crate::write_figures::Shelf,
    /// 提取: the assistant's proposals for the custom fields, its run over the papers, and their confirmation.
    pub extract: crate::extract::Extract,
    /// Conversations with the library set aside by 新对话 (or another taken up from 历史) while they were answering: they go
    /// on off screen and are let go when the answer is in; 历史 takes one up again (`Workspace::chat_aside`, `in_chat`).
    pub aside: Vec<Chat>,
    _subscriptions: Vec<Subscription>,
}

pub struct Workspace {
    pub focus: FocusHandle,
    pub settings: Settings,
    settings_path: PathBuf,
    pub root: PathBuf,
    pub libraries: Vec<LibraryInfo>,
    pub lib: Option<Library>,
    /// Libraries left while one of their conversations was still answering: kept off screen until the answer is in
    /// (`close_library`, `in_library`), and taken up again as they are when opened.
    pub parked: Vec<Library>,
    /// While `in_library` has a library left at work in `lib`'s place for a call: what the call does is not on show.
    pub away: bool,
    /// 设置, shown instead of the home page or the library while it is open.
    pub settings_view: Option<SettingsView>,
    /// 连接器: where the connection of each one tried stands (`custom.rs`).
    pub connections: Vec<(wisp_core::connector::Connector, crate::custom::Connection)>,
    /// 技能: what the skills' folder held when it was last read.
    pub skills: Vec<wisp_core::skill::Skill>,
    pub update: UpdateState,
    /// The version the automatic check last announced, so it says it once.
    pub update_told: Option<String>,
    /// Where the window is, as last written to `session.json` (`keep_session`).
    pub(crate) session: Session,
    pub modal: Option<Modal>,
    pub modal_error: String,
    pub modal_subscriptions: Vec<Subscription>,
    /// The figure dialog of a piece cut out stands over the PDF without hiding it (figures.rs): moved by its
    /// title this far from the window's centre, kept for the next one; and the press holding it meanwhile.
    pub figure_at: (f32, f32),
    pub figure_drag: Option<(Point<Pixels>, (f32, f32))>,
    pub menu: Option<Menu>,
    /// The command palette, while it is open (`palette.rs`).
    pub palette: Option<crate::palette::Palette>,
    pub toast: Option<Toast>,
    pub home_scroll: ScrollHandle,
    /// The home page's assistant: the box of a first question and its files (`draft.rs`).
    pub home: crate::draft::Home,
    /// The edge being dragged: the sidebar's, the detail panel's, or the one between a paper's conversation and its PDF.
    pub resizing: Option<Edge>,
    pub column_drag: Option<ColumnDrag>,
    /// What is selected in a text of the window that is not an answer or a text field.
    pub picked: Option<crate::select::Picked>,
    /// The next `select` is a step through the papers read (`step_history`), not a paper chosen.
    stepping: bool,
    /// The library's window: a paper's own window shows its paper there (`show_in_library`).
    pub main_window: AnyWindowHandle,
    /// A paper of the scope panel is held and on its way into the conversation (`chat::PaperDrag`).
    pub pinning: bool,
    /// The pointer is over the filters of a list too narrow for them, or over the card that shows them all.
    pub filters_over: [bool; 2],
    /// Those filters were drawn in this frame; a frame without them forgets the pointer.
    pub filters_drawn: bool,
    /// The home page invites the person, once, to star the project (`settings::star_due` at start, or the debug step `starask`).
    pub star_ask: bool,
    /// 新手引导, while it walks a first window through the app (`tour.rs`).
    pub tour: crate::tour::Tour,
}

impl Library {
    /// 写作 becomes the page, and the one it was opened from is kept for the way back (`Cmd::LeaveWrite`).
    pub(crate) fn enter_write(&mut self) {
        if self.page != Page::Write { self.write_from = self.page }
        self.page = Page::Write;
    }

    /// Whether one of its conversations is answering, or waits for the person's yes to a tool.
    pub(crate) fn answering(&self) -> bool {
        !self.aside.is_empty() || [Talk::Library, Talk::Paper, Talk::Write].into_iter().any(|talk| self.chat(talk).busy)
    }

    /// Whether one of its conversations, in its place or set aside, waits for the person's yes to a tool.
    pub(crate) fn waits_for_yes(&self) -> bool {
        self.aside.iter().any(|chat| chat.pending.is_some()) || [Talk::Library, Talk::Paper, Talk::Write].into_iter().any(|talk| self.chat(talk).pending.is_some())
    }

    pub fn chat(&self, talk: Talk) -> &Chat {
        match talk {
            Talk::Library => &self.library_chat,
            Talk::Paper => &self.paper_chat,
            Talk::Write => &self.write_chat,
        }
    }

    pub(crate) fn chat_mut(&mut self, talk: Talk) -> &mut Chat {
        chat_of!(self, talk)
    }

    /// Ends the selection made in an answer, in whichever conversation it is; says whether there was one.
    /// One the pointer still holds is being made, and stays.
    pub(crate) fn unpick(&mut self) -> bool {
        let mut ended = false;
        for talk in [Talk::Library, Talk::Paper, Talk::Write] {
            let chat = self.chat_mut(talk);
            if chat.picked.is_some_and(|(.., held)| !held) {
                chat.picked = None;
                ended = true;
            }
        }
        ended
    }

    /// What a conversation of this kind is about, which is what it is kept under in the library's history
    /// (`conversations.paper_id`): nothing for the library's, the selected paper, the manuscript being written.
    /// None while there is no such paper or manuscript.
    pub fn talk_about(&self, talk: Talk) -> Option<String> {
        match talk {
            Talk::Library => Some(String::new()),
            // Several papers side by side have a conversation of their own, whichever of them is worked in.
            Talk::Paper => match self.talk_papers().as_mut_slice() {
                [] => None,
                [one] => Some(one.clone()),
                several => {
                    several.sort();
                    Some(several.join("+"))
                }
            },
            Talk::Write => Some(self.writing.agent.about.clone()).filter(|id| !id.is_empty()),
        }
    }

    /// The papers the paper's conversation is about (#194): the one on show, or, with others kept beside it, all
    /// of them in the order of their panes. They are read together and the questions are mostly how they compare.
    pub fn talk_papers(&self) -> Vec<String> {
        if self.pins.is_empty() { self.selected.iter().cloned().collect() } else { self.pane_order() }
    }

    /// Whether 历史 in this place can list every conversation of the library: the library's and a paper's can, a
    /// manuscript's keeps to its own, and a conversation not yet made a library has its own list (`render_draft_history`).
    pub fn history_spans(&self, talk: Talk) -> bool {
        talk != Talk::Write && !self.info.draft
    }

    /// What 历史 in this place lists: the conversations kept under what it is about, or every one of the library (`history_all`).
    pub fn list_history(&self, talk: Talk) -> anyhow::Result<Vec<ConversationInfo>> {
        if self.history_all && self.history_spans(talk) { return self.store.list_all_conversations() }
        self.store.list_conversations(&self.talk_about(talk).unwrap_or_default())
    }

    pub fn paper(&self, id: &str) -> Option<&Paper> {
        self.papers.iter().find(|p| p.id == id)
    }

    /// The papers a question to the library is put to: those it is kept to, else what the panel over the conversation lists.
    pub fn scope_papers(&self) -> Vec<&Paper> {
        if self.asked.is_empty() { return self.scope_listed() }
        let asked: HashSet<&str> = self.asked.iter().map(String::as_str).collect();
        self.papers.iter().filter(|p| asked.contains(p.id.as_str())).collect()
    }

    /// The list on show, narrowed by the search of the panel over the library's conversation.
    pub fn scope_listed(&self) -> Vec<&Paper> {
        self.shown.iter().map(|&i| &self.papers[i]).filter(|p| self.scope_has(&p.id)).collect()
    }

    /// Whether a paper holds the words of that panel's search.
    pub fn scope_has(&self, id: &str) -> bool {
        self.scope_hits.as_ref().is_none_or(|hits| hits.contains(id))
    }

    /// Look the panel's words up in the papers again.
    pub fn scope_refilter(&mut self) {
        let words = self.scope_words.trim();
        self.scope_hits = (!words.is_empty()).then(|| self.papers.iter().filter(|p| filter::says(p, words)).map(|p| p.id.clone()).collect());
    }

    /// A new conversation with the library is about the list on show again: nothing kept to, nothing searched for.
    pub fn scope_reset(&mut self, cx: &mut App) {
        self.asked.clear();
        self.scope_words.clear();
        self.scope_hits = None;
        self.scope_find.update(cx, |input, cx| input.set_text("", cx));
    }

    /// The panel of the papers in scope is on show over the library's conversation.
    pub fn write_figures_shown(&self) -> bool {
        let chat = &self.library_chat;
        self.page == Page::Ask && chat.materials_open && chat.history.is_none() && (chat.ctx.is_some() || chat.messages.is_empty() || self.info.draft)
    }

    pub fn selected_paper(&self) -> Option<&Paper> {
        self.selected.as_deref().and_then(|id| self.paper(id))
    }

    pub fn paper_files(&self, id: &str) -> Vec<&PaperFile> {
        self.files.iter().filter(|f| f.paper_id == id).collect()
    }

    /// A full text was read that could not be before: the next question
    /// is given the library as it is now. An answer on its way keeps what it has.
    pub(crate) fn texts_changed(&mut self) {
        for chat in [&mut self.paper_chat, &mut self.library_chat, &mut self.write_chat] {
            if !chat.busy { chat.forget_context() }
        }
    }

    /// Look up, in the papers they name, the quotations of the answers
    /// that have not been looked up yet. An answer still being written is
    /// left until it is whole.
    pub fn check_quotes(&mut self, talk: Talk) {
        let chat = self.chat(talk);
        let finished = if chat.busy { chat.messages.len().saturating_sub(1) } else { chat.messages.len() };
        let mut wanted: Vec<(ai::Citation, String)> = vec![];
        for item in &chat.messages[..finished] {
            let ChatItem::Answer(text) = item else { continue };
            for citation in ai::citations(text) {
                if citation.quote.is_empty() || chat.checks.contains_key(&(citation.label.clone(), citation.file, citation.page, citation.quote.clone())) { continue }
                // A paper of another library is not read here: its sentence is looked for when the citation is followed there.
                let Some(source) = chat.sources().iter().find(|source| source.label == citation.label && source.library.is_empty()) else { continue };
                wanted.push((citation, source.paper_id.clone()));
            }
        }
        if wanted.is_empty() { return }
        // Each paper's pages are read once, however often it is quoted.
        let mut texts: HashMap<String, Option<Vec<String>>> = HashMap::new();
        let mut attachment_texts: HashMap<(String, usize), Option<Vec<String>>> = HashMap::new();
        let mut checks = vec![];
        for (citation, paper_id) in wanted {
            let Some(paper) = self.papers.iter().find(|paper| paper.id == paper_id) else { continue };
            // A quotation from an attachment is looked up in that attachment's pages.
            let pages = match citation.file {
                Some(n) => attachment_texts.entry((paper_id.clone(), n)).or_insert_with(|| self.store.attachment_pages(&paper_id).ok().and_then(|files| files.into_iter().nth(n - 1)).map(|(_, pages)| pages).filter(|pages| !pages.is_empty())),
                None => texts.entry(paper_id.clone()).or_insert_with(|| self.store.paper_pages(&paper_id).ok().flatten()),
            };
            let found = ai::check_quote(&citation.quote, citation.page, paper, pages.as_deref());
            checks.push(((citation.label, citation.file, citation.page, citation.quote), found));
        }
        self.chat_mut(talk).checks.extend(checks);
    }

    /// Read the review of the conversation with the library again, and look its quotations up in the papers.
    /// Called when the conversation, the record or the papers change, never while drawing.
    pub fn load_review(&mut self) -> anyhow::Result<()> {
        let review = if self.library_chat.id.is_empty() { None } else { self.store.review_for_conversation(&self.library_chat.id)? };
        let mut texts: HashMap<String, Option<Vec<String>>> = HashMap::new();
        let mut checks = HashMap::new();
        for row in review.iter().flat_map(|review| &review.evidence_rows).filter(|row| !row.quote.is_empty()) {
            let Some(paper) = self.papers.iter().find(|paper| paper.id == row.paper_id) else { continue };
            let pages = texts.entry(row.paper_id.clone()).or_insert_with(|| self.store.paper_pages(&row.paper_id).ok().flatten());
            checks.insert(row.id.clone(), ai::check_quote(&row.quote, row.page, paper, pages.as_deref()));
        }
        if review.is_none() { self.library_chat.review_open = false }
        (self.library_chat.review, self.library_chat.review_checks) = (review, checks);
        Ok(())
    }

    pub fn refilter(&mut self) {
        let now = time::now_millis();
        self.shown = filter::filter_papers(&self.papers, &self.filters, now);
        self.counts = filter::counts(&self.papers, &self.filters, now);
        self.lay_rows();
        self.scope_refilter();
    }

    /// The rows of the list again, after the papers on show, their files or what is unfolded changed. A paper's
    /// files stand under it in the order of `files`: the main text, then the others as the detail panel and
    /// `S1`, `S2`, … count them. A paper with its main text and nothing else has nothing to unfold: its row is the file.
    pub fn lay_rows(&mut self) {
        let mut held: HashMap<&str, (usize, bool)> = HashMap::new();
        for file in &self.files {
            let of = held.entry(file.paper_id.as_str()).or_default();
            *of = (of.0 + 1, of.1 || file.kind != "fulltext");
        }
        self.folding = held.into_iter().filter(|(_, (count, attached))| *count > 1 || *attached).map(|(id, _)| id.to_string()).collect();
        let mut rows = Vec::with_capacity(self.shown.len());
        for &at in &self.shown {
            rows.push(Row::Paper(at));
            let id = &self.papers[at].id;
            if self.unfolded.contains(id) && self.folding.contains(id) { rows.extend((0..self.files.len()).filter(|&file| &self.files[file].paper_id == id).map(Row::File)) }
        }
        self.rows = rows;
    }

    /// The row of the list that the `at`-th paper on show stands in.
    pub fn row_of(&self, at: usize) -> usize {
        if self.rows.len() == self.shown.len() { return at }
        self.shown.get(at).and_then(|paper| self.rows.iter().position(|row| *row == Row::Paper(*paper))).unwrap_or(at)
    }

    /// Read everything again after a change.
    pub fn reload(&mut self) -> anyhow::Result<()> {
        self.note_links = self.store.note_links()?;
        self.papers = self.store.list_papers()?;
        self.fields = self.store.list_fields()?;
        self.files = self.store.list_files()?;
        self.trash = self.store.list_trash()?;
        // A pinned paper taken away, or its file, goes from its pane.
        let (papers, files) = (&self.papers, &self.files);
        let order = self.pane_order();
        self.pins.retain(|pin| papers.iter().any(|p| p.id == pin.paper) && files.iter().any(|f| f.paper_id == pin.paper && f.format == "pdf"));
        self.order_panes(&order);
        self.rounds = self.store.screening_rounds()?;
        self.searches = self.store.list_searches()?;
        self.suggestions = self.store.list_suggestions()?.into_iter().map(|s| (s.paper_id.clone(), s)).collect();
        self.batches = self.store.list_batches(30)?;
        self.writing.structures = self.store.structures()?;
        // A paper's record may have changed: how its citations read is worked out again.
        self.citing.read.made_from = 0;
        self.origins = self.store.file_origins()?;
        self.shrunk = self.store.shrunk_files()?;
        self.gallery.items = self.store.list_figures()?;
        self.extract.proposals = wisp_core::extract::Proposals::new(self.store.list_proposals()?);
        self.extract_settle();
        // Filters on a field that no longer exists would hide everything.
        let known: HashSet<&str> = self.fields.iter().map(|f| f.id.as_str()).collect();
        self.filters.fields.retain(|id, _| known.contains(id.as_str()));
        if self.selected.as_ref().is_some_and(|id| self.paper(id).is_none()) {
            self.selected = None;
            self.close_pdf();
        }
        let held: HashSet<&str> = self.papers.iter().map(|p| p.id.as_str()).collect();
        self.screen.round.keep(|id| held.contains(id));
        let gone = |ids: &Vec<String>| ids.iter().any(|id| !held.contains(id.as_str()));
        if gone(&self.marked) || gone(&self.asked) {
            let held: HashSet<String> = held.into_iter().map(str::to_string).collect();
            self.marked.retain(|id| held.contains(id));
            self.asked.retain(|id| held.contains(id));
        }
        // Chosen papers restored or deleted meanwhile are no longer chosen.
        let trashed: HashSet<&str> = self.trash.iter().map(|entry| entry.paper.id.as_str()).collect();
        self.trash_chosen.retain(|id| trashed.contains(id.as_str()));
        if self.trash_anchor.as_deref().is_some_and(|id| !trashed.contains(id)) { self.trash_anchor = None }
        self.refilter();
        // A paper changed or gone is another source for the review's evidence.
        self.load_review()?;
        Ok(())
    }
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings_path = settings_file();
        let mut toast = None;
        let settings = Settings::load(&settings_path).unwrap_or_else(|err| {
            toast = Some(Toast { message: tf!("{err:#}；本次使用默认设置。", "{err:#}; using default settings for this session.").into(), error: true, at: Instant::now(), action: None });
            Settings::default()
        });
        crate::i18n::set_language(settings.language);
        wisp_net::set_proxy(&settings.global_proxy());
        wisp_net::set_contact(settings.contact());
        let root = settings.data_root(&default_root());
        let _ = std::fs::create_dir_all(&root);
        let focus = cx.focus_handle();
        window.focus(&focus);
        theme::set_accent("blue");
        // A diagram in an answer is drawn by a function that knows no workspace: what its buttons ask comes here.
        let workspace = cx.weak_entity();
        crate::diagram::on_ask(move |ask, window, cx| {
            let crate::diagram::Ask::CopyPicture(source) = ask;
            workspace.update(cx, |this, cx| this.run(Cmd::CopyDiagram(source), window, cx)).ok();
        });
        shortcuts::rebuild(&settings.shortcuts, cx);
        let libraries = libraries::list(&root);
        let home = crate::draft::Home::new(window, cx);
        let mut this = Workspace { focus, settings, settings_path, root, libraries, lib: None, parked: vec![], away: false, stepping: false, main_window: window.window_handle(), settings_view: None, connections: vec![], skills: vec![], update: UpdateState::Idle, update_told: None, session: Session::default(), modal: None, modal_error: String::new(), modal_subscriptions: vec![], figure_at: (0., 0.), figure_drag: None, menu: None, palette: None, toast, home_scroll: ScrollHandle::new(), home, resizing: None, column_drag: None, picked: None, pinning: false, filters_over: [false; 2], filters_drawn: false, star_ask: false, tour: Default::default() };
        // A new day of use is counted once at start; on the fifth the home page asks for a star, until answered.
        // A settings file that could not be read is not written over with defaults for it (the toast says so).
        if this.toast.is_none() && wisp_core::settings::note_use(&mut this.settings, &wisp_core::time::today()) { this.save_settings(cx) }
        this.star_ask = wisp_core::settings::star_due(&this.settings);
        // A first start is walked through the app; what the person does from then on moves the steps (`tour.rs`).
        this.tour_start();
        cx.observe_self(|this, cx| this.tour_follow(cx)).detach();
        // The library's window closed: the papers' own windows go with it (their viewers are this one's).
        cx.on_release(|this: &mut Workspace, cx| {
            if let Some(lib) = this.lib.as_mut() { for open in lib.windows.drain(..) { open.window.update(cx, |_, window, _| window.remove_window()).ok(); } }
        })
        .detach();
        this.apply_appearance(window);
        this.apply_typography(window, cx);
        cx.observe_window_appearance(window, |this, window, _| this.apply_appearance(window)).detach();
        this.watch_updates(cx);
        this.watch_searches(cx);
        this.load_skills();
        this.connect_all(cx);
        this
    }

    /// Once the window is there: go back to where it was last (回到上次的界面), and from now on keep where it is.
    pub fn start_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A scripted debug start says itself where it begins (its `restore` step is this).
        if self.settings.restore_session && wisp_core::settings::env("DEV").is_none() { self.restore_session(window, cx) }
        cx.observe_self(|this, _| this.keep_session()).detach();
        // Quitting with a paper open: its file opens where it was left next time too (#194).
        cx.on_app_quit(|this, cx| {
            if let Some(lib) = this.lib.as_mut() { lib.keep_places(cx) }
            async {}
        })
        .detach();
    }

    /// Write down where the window is whenever that changed, so the next start (after quitting,
    /// an update or a crash) can be the same place: the library, its page, the paper on show.
    // ponytail: the window's size, the scroll positions and the PDF's page are not kept; fields of `Session` when they are missed.
    fn keep_session(&mut self) {
        if !self.settings.restore_session { return }
        let now = self.lib.as_ref().map(|lib| Session { library: lib.info.id.clone(), page: lib.page.key().into(), paper: lib.selected.clone().unwrap_or_default(), tab: lib.tab.key().into(), wide: lib.wide, reading: lib.reading }).unwrap_or_default();
        if now != self.session {
            let _ = now.save(&session_file());
            self.session = now;
        }
    }

    /// Go back to where `session.json` says the window was. A library or a paper that is gone since is simply not opened.
    pub(crate) fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let session = Session::load(&session_file());
        if session.library.is_empty() { return }
        self.open_library(&session.library, window, cx);
        if self.lib.as_ref().is_none_or(|lib| lib.info.id != session.library) { return }
        self.run(Cmd::SetPage(Page::from_key(&session.page)), window, cx);
        if self.lib.as_ref().is_some_and(|lib| lib.paper(&session.paper).is_some()) {
            self.select(Some(session.paper.clone()), window, cx);
            self.run(Cmd::SetTab(Tab::from_key(&session.tab)), window, cx);
            if let Some(lib) = self.lib.as_mut() { (lib.wide, lib.reading) = (session.wide, session.reading) }
        }
    }

    pub fn say(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.show_toast(message.into(), false, None, cx);
    }

    pub fn fail(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.show_toast(message.into(), true, None, cx);
    }

    /// What was done, with a button that takes it back: it stays up long enough to be read and reached.
    pub fn say_undoable(&mut self, message: impl Into<SharedString>, undo: Cmd, cx: &mut Context<Self>) {
        self.show_toast(message.into(), false, Some((t("撤销").into(), undo)), cx);
    }

    fn show_toast(&mut self, message: SharedString, error: bool, action: Option<(SharedString, Cmd)>, cx: &mut Context<Self>) {
        // What failed is often said by core, in Chinese: a sentence the catalog knows is shown in the interface's language.
        let message = if crate::i18n::is_english() { crate::i18n::phrase(&message).into() } else { message };
        let at = Instant::now();
        // Errors stay longer: they usually need reading twice. One with a way back stays until it can be reached.
        let seconds = if action.is_some() { 10 } else if error { 7 } else { 3 };
        self.toast = Some(Toast { message, error, at, action });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(seconds)).await;
            this.update(cx, |this, cx| {
                if this.toast.as_ref().is_some_and(|t| t.at == at) {
                    this.toast = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Run a store operation on the open library, reload, and report a failure.
    pub(crate) fn change(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Library) -> anyhow::Result<()>) -> bool {
        let Some(lib) = self.lib.as_mut() else { return false };
        let result = f(lib).and_then(|_| lib.reload());
        cx.notify();
        match result {
            Ok(()) => {
                // A figure kept or taken away is shown so on its page.
                self.sync_figure_frames(cx);
                true
            }
            Err(err) => {
                self.fail(format!("{err:#}"), cx);
                false
            }
        }
    }

    pub fn save_settings(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = self.settings.save(&self.settings_path) {
            self.fail(format!("{err:#}"), cx);
        }
    }

    // ---- libraries

    /// Leave the open library. One at work (`Library::at_work`: a conversation answering, a run going through its
    /// papers) is kept, off screen, until that is over: leaving a library does not stop what it is doing.
    pub(crate) fn close_library(&mut self, cx: &mut App) {
        let Some(mut lib) = self.lib.take() else { return };
        lib.keep_places(cx);
        // A paper's own window belongs to its library.
        for open in lib.windows.drain(..) { open.window.update(cx, |_, window, _| window.remove_window()).ok(); }
        if lib.at_work() { self.parked.push(lib) }
    }

    /// Run `f` with the library `id` as the open one: the open library itself, or one answering off screen (`parked`),
    /// put in its place meanwhile and back after. What a conversation's answer brings is so handled the same way
    /// whether or not the person is looking at it. None when neither is that library.
    pub(crate) fn in_library<R>(&mut self, id: &str, cx: &mut Context<Self>, f: impl FnOnce(&mut Self, &mut Context<Self>) -> R) -> Option<R> {
        if self.lib.as_ref().is_some_and(|lib| lib.info.id == id) { return Some(f(self, cx)) }
        let at = self.parked.iter().position(|lib| lib.info.id == id)?;
        let parked = self.parked.remove(at);
        // What it was at before the call: what the call ends is said, since nobody was looking.
        let (answering, running): (bool, Vec<crate::works::Kind>) = (parked.answering(), parked.running().into_iter().map(|work| work.kind).collect());
        let open = self.lib.replace(parked);
        let away = std::mem::replace(&mut self.away, true);
        let result = f(self, cx);
        self.away = away;
        let back = std::mem::replace(&mut self.lib, open);
        if let Some(back) = back.filter(|lib| lib.info.id == id) {
            let name = back.info.name.clone();
            if answering && !back.answering() { self.say(tf!("「{name}」里的助手答完了，打开它就能看到", "The assistant has finished in “{name}”; open it to see the answer"), cx) }
            for work in back.works() {
                if running.contains(&work.kind) && !work.running { self.say(work.ended(&name), cx) }
            }
            if back.at_work() { self.parked.insert(at.min(self.parked.len()), back) } else { self.parked_done(back, cx) }
            // The marks on the home page and the entries in the sidebar follow what came back.
            cx.notify();
        }
        Some(result)
    }

    /// A library left at work has nothing more going on: what it did was kept with it as it went (the conversation,
    /// the suggestions, the translations), and it is let go. `in_library` has said what ended.
    fn parked_done(&mut self, lib: Library, cx: &mut Context<Self>) {
        drop(lib);
        self.libraries = libraries::list(&self.root);
        cx.notify();
    }

    pub(crate) fn open_library(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut info) = self.libraries.iter().find(|l| l.id == id).cloned() else { return };
        // The library on show, at work: it stays as it is, not opened a second time.
        if self.lib.as_ref().is_some_and(|lib| lib.info.id == id && lib.at_work()) { return }
        // One answering off screen is taken up as it stands: the answer goes on arriving where it is read.
        if let Some(at) = self.parked.iter().position(|lib| lib.info.id == id) {
            self.write_snapshot("leave");
            let mut lib = self.parked.remove(at);
            info.opened_at = time::now_iso();
            let _ = libraries::save(&info);
            theme::set_accent(&info.color);
            window.set_window_title(&format!("{} — Wisp Study", info.name));
            // What the home page knows of it is newer (a name changed meanwhile).
            lib.info = info;
            if lib.info.draft { lib.page = Page::Ask }
            if let Err(err) = lib.reload() { self.fail(format!("{err:#}"), cx) }
            self.close_library(cx);
            self.lib = Some(lib);
            window.focus(&self.focus);
            self.extract_missing(cx);
            return;
        }
        let store = match Store::open(&info.dir) {
            Ok(store) => store,
            Err(err) => return self.fail(format!("{err:#}"), cx),
        };
        let file_places = store.reading_places().unwrap_or_default();
        self.write_snapshot("leave");
        info.opened_at = time::now_iso();
        let _ = libraries::save(&info);
        theme::set_accent(&info.color);
        window.set_window_title(&format!("{} — Wisp Study", info.name));
        let search = cx.new(|cx| TextInput::new("搜索题目、作者、DOI、标签、词条、笔记…", cx));
        let notes = cx.new(|cx| TextInput::new("读这篇文献时想到的…", cx).multiline(4));
        let scope_find = cx.new(|cx| TextInput::new("在这些文献里找：题目、作者、标签、摘要…", cx));
        let subscriptions = vec![
            // The panel over the conversation searches the papers in scope as it is typed into.
            cx.subscribe(&scope_find, |this, input, event: &InputEvent, cx| {
                if *event == InputEvent::Changed { this.scope_typed(input.read(cx).text().to_string(), cx) }
            }),
            cx.subscribe_in(&search, window, |this, input, event: &InputEvent, window, cx| {
                if *event == InputEvent::Changed && let Some(lib) = this.lib.as_mut() {
                    lib.filters.search = input.read(cx).text().to_string();
                    lib.page = Page::Papers;
                    lib.refilter();
                    this.find_typed(input.read(cx).text().to_string(), cx);
                    cx.notify();
                }
                // Enter: the words looked for through the whole library.
                if *event == InputEvent::Submit { this.run(Cmd::LibraryFind(crate::library_find::Do::Open), window, cx) }
            }),
            cx.subscribe(&notes, |this, input, event: &InputEvent, cx| {
                if *event == InputEvent::Changed {
                    let text = input.read(cx).text().to_string();
                    if let Some(id) = this.lib.as_ref().and_then(|l| l.selected.clone()) {
                        this.edit_paper(&id, cx, |p| p.notes = text);
                    }
                }
            }),
        ];
        let library_chat = Chat::new("交代一件事或提一个问题，输入 / 选择命令，可以粘贴图片", cx);
        let paper_chat = Chat::new(crate::chat::PAPER_PLACEHOLDER, cx);
        let write_chat = Chat::new("告诉助手写什么、改什么", cx);
        let mut subscriptions = subscriptions;
        for (chat, talk) in [(&library_chat, Talk::Library), (&paper_chat, Talk::Paper), (&write_chat, Talk::Write)] {
            subscriptions.push(cx.subscribe(&chat.input, move |this, _, event: &InputEvent, cx| this.chat_input(talk, *event, cx)));
            // 历史 is narrowed as its search is typed into: the library is asked here, never while drawing.
            subscriptions.push(cx.subscribe(&chat.history_find, move |this, input, event: &InputEvent, cx| if *event == InputEvent::Changed { this.history_typed(talk, input.read(cx).text().to_string(), cx) }));
        }
        let guide = Guide::new(&info.topic, cx);
        let screen = Screen::new(&info.criteria, cx);
        // The goal is the library's, kept with it as it is typed; suggestions made against another one stop counting.
        subscriptions.push(cx.subscribe(&screen.criteria, |this, input, event: &InputEvent, cx| {
            if *event != InputEvent::Changed { return }
            let goal = input.read(cx).text().trim().to_string();
            let Some(lib) = this.lib.as_mut().filter(|l| l.info.criteria != goal) else { return };
            lib.info.criteria = goal;
            if let Err(err) = libraries::save(&lib.info) { this.fail(tf!("筛选目标没有保存：{err:#}", "Screening criteria not saved: {err:#}"), cx) }
        }));
        let manuscripts = store.manuscripts().unwrap_or_default();
        let (manuscript, section_id) = Writing::latest(&manuscripts);
        let opened = manuscripts.iter().find(|m| m.id == manuscript);
        let draft = opened.and_then(|m| m.section(&section_id)).map(|s| s.body.clone()).unwrap_or_default();
        // The empty field invites the title: 未命名稿件 is the outline's name for the manuscript, not what is typed here.
        let title = cx.new(|cx| TextInput::new("输入稿件题目…", cx).with_text(&opened.map(|m| m.title.clone()).unwrap_or_default()));
        subscriptions.push(cx.subscribe(&title, |this, input, event: &InputEvent, cx| {
            if *event == InputEvent::Changed { this.write_title_typed(input.read(cx).text().to_string(), cx) }
        }));
        let editor = cx.new(|cx| {
            let mut input = TextInput::new(crate::write::PLACEHOLDER, cx).multiline(14).markdown().with_text(&draft);
            // A picture pasted into the manuscript is kept in 资料库 and put in at the caret (write_figures.rs).
            input.accepts_images = true;
            input
        });
        let find = cx.new(|cx| TextInput::new("找句子：输入词或短语，例如 remains unclear", cx));
        subscriptions.push(cx.subscribe(&editor, |this, input, event: &InputEvent, cx| {
            if let InputEvent::MarkClicked(n) = *event { return this.comment_mark_clicked(n, cx) }
            if *event == InputEvent::Images {
                let pasted = input.update(cx, |input, _| input.take_images());
                return this.manuscript_pasted(pasted, cx);
            }
            if *event != InputEvent::Changed { return }
            let text = input.read(cx).text().to_string();
            this.write_typed(text, cx);
        }));
        // A citation pointed at says what it rests on; ⌘ or Ctrl with a click opens that sentence in its paper (write_source.rs).
        subscriptions.push(cx.subscribe_in(&editor, window, |this, _, event: &InputEvent, window, cx| match *event {
            InputEvent::LinkHovered => cx.notify(),
            InputEvent::LinkClicked(n, at) => this.citation_follow(n, at, window, cx),
            _ => {}
        }));
        // Candidates are asked for: the find box on Enter, the manuscript by the shortcut.
        subscriptions.push(cx.subscribe(&find, |this, _, event: &InputEvent, cx| if *event == InputEvent::Submit { this.write_find(true, cx) }));
        let (comments, heard) = crate::comments::Commenting::new(store.comments(&manuscript).unwrap_or_default(), store.manuscript_source(&manuscript).ok().flatten(), cx);
        subscriptions.extend(heard);
        let seek = crate::write_seek::Seek::new(&mut subscriptions, cx);
        let writing = Writing { manuscripts, manuscript, section_id, reading: false, title, typed: 0, editor, find, search: Default::default(), structures: BTreeMap::new(), sorting: Sorting::default(), scroll: ScrollHandle::new(), assist: Default::default(), revise: Default::default(), comments, editor_scroll: ScrollHandle::new(), side: Side::default(), side_open: false, agent: Default::default(), seek, away: false };
        let citing = crate::cite::Citing::new(&store, &writing.manuscript, &mut subscriptions, cx);
        let gallery = crate::figures::Gallery::new(&mut subscriptions, cx);
        let write_figures = crate::write_figures::Shelf::new(&mut subscriptions, cx);
        // The sort kept with the library may name a partition the 分区 column no longer shows (the setting was changed while
        // another library, or none, was open): the list is sorted by the column's, so its header says so.
        let filters = Filters { sort: crate::library::sort_in_zone(&info.sort, &self.settings), ..Default::default() };
        let mut lib = Library {
            info,
            store,
            papers: vec![],
            fields: vec![],
            files: vec![],
            trash: vec![],
            rounds: vec![],
            filters,
            shown: vec![],
            rows: vec![],
            unfolded: HashSet::new(),
            folding: HashSet::new(),
            counts: filter::Counts::default(),
            search,
            finding: Default::default(),
            page: Page::Papers,
            selected: None,
            tab: Tab::Info,
            wide: false,
            reading: false,
            split: true,
            ask_from: Tab::Info,
            write_from: Page::Papers,
            ask_drawn: false,
            float_drag: None,
            float_area: (0., 0.),
            notes,
            pdf: None,
            aloud: None,
            aloud_shown: None,
            seek_bar: (0., 0.),
            seek_held: None,
            pdf_events: None,
            beside: None,
            split_share: 0.5,
            halves_drag: None,
            pins: vec![],
            windows: vec![],
            main_at: 0,
            panes_from: 0,
            panes_shown: 1,
            keep: false,
            pane_chat: 0.,
            bar_level: 0,
            focus_box: None,
            zoom: Zoom::FitWidth,
            library_chat,
            paper_chat,
            write_chat,
            marked: vec![],
            anchor: None,
            trash_chosen: vec![],
            trash_anchor: None,
            cursor: None,
            asked: vec![],
            scope_find,
            scope_words: String::new(),
            scope_hits: None,
            history_all: false,
            table_scroll: UniformListScrollHandle::new(),
            detail_scroll: ScrollHandle::new(),
            extracting: HashSet::new(),
            shrinking: HashSet::new(),
            shrunk: HashSet::new(),
            no_smaller: HashSet::new(),
            read_file: HashMap::new(),
            file_places,
            mark_history: HashMap::new(),
            back: vec![],
            forward: vec![],
            guide,
            screen,
            suggestions: HashMap::new(),
            translations: HashMap::new(),
            translation_asks: HashMap::new(),
            translation_run: None,
            translation_session: Arc::new(()),
            show_translation: false,
            facing: false,
            facing_text: None,
            facing_start: false,
            facing_run: None,
            facing_for: String::new(),
            searches: vec![],
            batches: vec![],
            trial: None,
            running: HashSet::new(),
            import: None,
            zotero: None,
            fetching: None,
            origins: BTreeMap::new(),
            note_links: BTreeMap::new(),
            writing,
            citing,
            gallery,
            write_figures,
            extract: crate::extract::Extract::new(),
            aside: vec![],
            _subscriptions: subscriptions,
        };
        if let Err(err) = lib.reload() {
            return self.fail(format!("{err:#}"), cx);
        }
        // A conversation not yet made a library is its conversation: the page 助手, its last conversation taken up again.
        let draft = lib.info.draft;
        if draft { lib.page = Page::Ask }
        let last = if draft { lib.store.list_conversations("").ok().and_then(|list| list.into_iter().next()) } else { None };
        self.close_library(cx);
        self.lib = Some(lib);
        window.focus(&self.focus);
        if let Some(last) = last { self.open_chat(Talk::Library, &last.id, cx) }
        self.extract_missing(cx);
    }

    // ---- one paper

    /// 后退 / 前进 (#194): the paper read before the one on show, or the one gone back from; one taken away
    /// since is passed over. Its file opens where it was left (`Library::keep_places`).
    pub(crate) fn step_history(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let to = loop {
            let Some(id) = (if forward { lib.forward.pop() } else { lib.back.pop() }) else { return };
            if lib.paper(&id).is_some() && lib.selected.as_ref() != Some(&id) { break id }
        };
        if let Some(on_show) = lib.selected.clone() { if forward { lib.back.push(on_show) } else { lib.forward.push(on_show) } }
        self.stepping = true;
        self.select(Some(to), window, cx);
        self.stepping = false;
    }

    pub(crate) fn select(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        if lib.selected == id { return }
        // A paper is on show once: one kept beside that is chosen is worked in where it is.
        if let Some(id) = id.as_deref().filter(|id| lib.pins.iter().any(|pin| pin.paper == *id)) { return self.work_in_pane(id, cx) }
        // The conversation with the papers being left is kept as far as it got, under what it was about until now.
        self.stop_chat(Talk::Paper, cx);
        // 留在旁边: the paper being left stays in its pane as it is read there, and the one chosen opens in a pane after it.
        let kept = self.lib.as_mut().and_then(|lib| if std::mem::take(&mut lib.keep) && id.is_some() { lib.selected.clone() } else { None });
        let mut order = vec![];
        if let (Some(kept), Some(chosen)) = (&kept, &id) {
            order = self.lib.as_ref().map(|lib| lib.pane_order()).unwrap_or_default();
            let after = order.iter().position(|paper| paper == kept).map_or(order.len(), |at| at + 1);
            order.insert(after, chosen.clone());
            match self.main_as_pin(cx) {
                Some(pin) => if let Some(lib) = self.lib.as_mut() { lib.pins.push(pin) },
                // Its record was on show: a viewer is made for its pane.
                None => self.pin(kept.clone(), cx),
            }
        }
        self.picked = None;
        // One paper on show ends a marking of several.
        if id.is_some() && let Some(lib) = self.lib.as_mut() { lib.marked.clear() }
        let Some(lib) = self.lib.as_mut() else { return };
        // The paper left is where 后退 goes (a step through them is not one more of them), and its file opens where it was left.
        if !std::mem::take(&mut self.stepping) && let Some(left) = lib.selected.clone() {
            if lib.back.last() != Some(&left) { lib.back.push(left) }
            if lib.back.len() > HISTORY { lib.back.remove(0); }
            lib.forward.clear();
        }
        lib.keep_places(cx);
        lib.selected = id;
        lib.close_pdf();
        lib.paper_chat.reset(cx);
        lib.detail_scroll = ScrollHandle::new();
        match lib.selected_paper() {
            Some(paper) => {
                let notes = paper.notes.clone();
                lib.notes.update(cx, |input, cx| input.set_text(&notes, cx));
            }
            None => {
                lib.wide = false;
                lib.reading = false;
                (lib.tab, lib.ask_from) = (Tab::Info, Tab::Info);
                window.focus(&self.focus);
            }
        }
        if kept.is_some() && let Some(lib) = self.lib.as_mut() { lib.order_panes(&order) }
        self.ensure_pdf(cx);
        self.translate_selected(false, cx);
        if kept.is_some() { self.room_for_panes(window, cx) }
        cx.notify();
    }

    /// A tag under another name on every paper that has it (one that has both keeps one), or taken off
    /// them. The list stays on the same papers when it is narrowed to that tag.
    pub(crate) fn retag(&mut self, tag: &str, name: Option<&str>, cx: &mut Context<Self>) -> bool {
        self.change(cx, |lib| {
            let ids: Vec<String> = lib.papers.iter().filter(|p| p.tags.iter().any(|t| t == tag)).map(|p| p.id.clone()).collect();
            for id in ids {
                let mut paper = lib.store.get_paper(&id)?;
                let mut tags: Vec<String> = vec![];
                for old in &paper.tags {
                    let kept = if old == tag { name.map(str::to_string) } else { Some(old.clone()) };
                    if let Some(kept) = kept && !tags.contains(&kept) { tags.push(kept) }
                }
                paper.tags = tags;
                lib.store.save_paper(&mut paper)?;
            }
            lib.filters.retag(tag, name);
            Ok(())
        })
    }

    /// Change one paper and save it.
    pub(crate) fn edit_paper(&mut self, id: &str, cx: &mut Context<Self>, edit: impl FnOnce(&mut Paper)) -> bool {
        self.change(cx, |lib| {
            let mut paper = lib.store.get_paper(id)?;
            edit(&mut paper);
            lib.store.save_paper(&mut paper)
        })
    }

    /// The whole library in one `.wispstudy` file, for another Wisp Study.
    pub(crate) fn export_library(&mut self, id: &str, attachments: bool, cx: &mut Context<Self>) {
        // The open library's own description is the newest.
        let Some(info) = self.lib.as_ref().map(|l| &l.info).filter(|i| i.id == id).or_else(|| self.libraries.iter().find(|l| l.id == id)).cloned() else { return };
        let directory = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(|| self.root.clone());
        let picked = cx.prompt_for_new_path(&directory, Some(&archive::file_name(&info.name)));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut path))) = picked.await else { return };
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case(archive::EXTENSION)) {
                let mut named = path.into_os_string();
                named.push(format!(".{}", archive::EXTENSION));
                path = PathBuf::from(named);
            }
            this.update(cx, |this, cx| this.say(tf!("正在导出「{}」…", "Exporting “{}”…", info.name), cx)).ok();
            let target = path.clone();
            let result = cx.background_executor().spawn(async move { archive::export(&info, &target, attachments) }).await;
            this.update(cx, |this, cx| match result {
                Ok(manifest) if attachments => this.say(tf!("已导出 {} 篇文献和 {} 个文件到 {}", "Exported {} papers and {} files to {}", manifest.papers, manifest.files, path.display()), cx),
                Ok(manifest) => this.say(tf!("已导出 {} 篇文献（不含 PDF）到 {}", "Exported {} papers (without PDFs) to {}", manifest.papers, path.display()), cx),
                Err(err) => this.fail(format!("{err:#}"), cx),
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn pick_library_file(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some(t("导入").into()), extensions: Some(vec![archive::EXTENSION.into(), archive::FORMER_EXTENSION.into()]) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            this.update(cx, |this, cx| for path in paths { this.import_library(path, cx) }).ok();
        })
        .detach();
    }

    /// A `.wispstudy` file taken in as a new library; the home page shows it once it is whole.
    pub fn import_library(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.say(tf!("正在导入 {file}…", "Importing {file}…"), cx);
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { archive::import(&root, &path) }).await;
            this.update(cx, |this, cx| {
                this.libraries = libraries::list(&this.root);
                match result {
                    Ok(info) => this.say(tf!("已导入文献库「{}」，{} 篇文献", "Imported library “{}” with {} papers", info.name, info.paper_count), cx),
                    Err(err) => this.fail(tf!("无法导入 {file}：{}", "Cannot import {file}: {}", format!("{err:#}")), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn export(&mut self, kind: ExportKind, cx: &mut Context<Self>) {
        let Some(lib) = &self.lib else { return };
        let papers: Vec<&Paper> = lib.shown.iter().map(|&i| &lib.papers[i]).collect();
        if papers.is_empty() { return self.fail(t("当前列表是空的，没有可导出的文献。"), cx) }
        let (text, extension) = match kind {
            ExportKind::Bibtex => (export::bibtex(&papers), "bib"),
            ExportKind::Csv => (export::papers_csv(&papers, &lib.fields), "csv"),
        };
        let count = papers.len();
        let name = format!("{}.{extension}", lib.info.name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "-"));
        let directory = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(|| self.root.clone());
        let picked = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut path))) = picked.await {
                if path.extension().is_none() { path.set_extension(extension); }
                let result = std::fs::write(&path, text);
                this.update(cx, |this, cx| match result {
                    Ok(()) => this.say(tf!("已导出 {count} 篇到 {}", "Exported {count} papers to {}", path.display()), cx),
                    Err(err) => this.fail(tf!("无法写入 {}：{err}", "Cannot write {}: {err}", path.display()), cx),
                })
                .ok();
            }
        })
        .detach();
    }
}
