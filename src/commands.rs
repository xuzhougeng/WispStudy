//! Every click is a `Cmd`: an element sends one (`Workspace::on`), a key ends in one (`shortcut`), and
//! `Workspace::run` is the one place that says what each does. A new feature usually means a variant here,
//! its arm in `run`, and the element that emits it.

use crate::app::{Library, Page, Side, Tab, Talk, Workspace};
use crate::chat_state::{ChatItem, Consent, Picture};
use crate::listening::podcast_files;
use crate::manuscripts::ManuscriptExport;
use crate::modal::Modal;
use crate::screen_run::ScreenMode;
use crate::i18n::t;
use crate::input::{InputEvent, TextInput};
use crate::pdf::{PdfView, Split, Zoom, ZoomTo};
use crate::settings::{AiTab, SettingsPage, UpdateState};
use crate::shortcuts::Dismiss;
use crate::theme;
use gpui::*;
use std::path::PathBuf;
use wisp_core::ai::{self, Protocol};
use wisp_core::filter::{self, Filters, View};
use wisp_core::libraries;
use wisp_core::model::*;
use wisp_core::settings::{Language, Session, Task, session_file};
use wisp_core::store::SavedSearch;
use wisp_core::{complete, time, zotero};

#[derive(Clone, Debug, PartialEq)]
pub enum FilterKey {
    Tag,
    Year,
    /// The journal's impact factor, a span (`Filters::impact`).
    Impact,
    /// The journal's zone in the partition the 分区 column shows (`Filters::zone`).
    Zone,
    Reading,
    Screening,
    Fulltext,
    Source,
    Preprint,
    Field(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExportKind {
    Bibtex,
    Csv,
}

/// What a person does to a review's record from the conversation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReviewDo {
    Requirements,
    ConfirmRequirements,
    /// The framework drawn from the reviews already published, which the outline is built against.
    Framework,
    ConfirmFramework,
    /// On to the outline without one.
    SkipFramework,
    Outline,
    ConfirmOutline,
    /// Keep a text as a section; its citations become rows of evidence.
    Section,
    Checks,
    Complete,
}

/// A text of a review's record that can be changed by hand.
#[derive(Clone, Debug, PartialEq)]
pub enum ReviewText {
    Requirements,
    Framework,
    Outline,
    /// The free notes on the evidence.
    Notes,
    Checks,
    /// A section, by its name.
    Section(String),
}

/// The scope of the library's next conversation, as the panel over it changes it: a paper kept to
/// (dragged into the conversation, or its plus clicked), let go again, all let go, the panel's search emptied.
#[derive(Clone, Debug, PartialEq)]
pub enum ScopeDo {
    Pin(String),
    Unpin(String),
    UnpinAll,
    ClearFind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    // Home
    OpenLibrary(String),
    /// A library's card was dragged onto another's (moved, onto): it takes that place on the home page.
    MoveLibrary(String, String),
    /// The home page's libraries as rows (true) or as cards: the switch over them, kept in the settings.
    HomeList(bool),
    NewLibrary,
    EditLibrary(String),
    AskRemoveLibrary(String),
    RemoveLibrary(String),
    /// The whole library as a `.wispstudy` file, with its PDFs and other attachments or without.
    ExportLibrary(String, bool),
    /// A `.wispstudy` file from another Wisp Study, taken in as a new library.
    ImportLibrary,
    GoHome,
    /// The home page's assistant and the conversations not yet made libraries (`draft.rs`).
    Draft(crate::draft::Do),
    RevealFolder,
    OpenUrl(String),
    /// The home page's invitation to star the project, answered: true opens the project's page first; either way it is not made again.
    AnswerStar(bool),
    /// 新手引导: begun, moved on, or closed (`tour.rs`).
    Tour(crate::tour::Do),
    /// 铃铛 on the home page: its list, and a row of it (`bell.rs`).
    Bell(crate::bell::Do),
    /// 教程: the book beside the bell, and the tutorials it opens (`tutorial.rs`).
    Tutorial(crate::tutorial::Do),
    /// The line that offers a contact email where a search starts (`Workspace::contact_hint`), closed for good.
    DismissContactHint,
    NoteOpen(Option<Talk>, bool),
    NotePick(usize),
    NoteDestination(bool),
    NotePreview,
    NoteSave(bool),
    NoteResolve(bool),
    // Settings
    OpenSettings(SettingsPage),
    CloseSettings,
    PickDataDir,
    ResetDataDir,
    RevealDataDir,
    /// Open a folder in the system's file manager (存储空间).
    RevealDir(PathBuf),
    /// Measure the data folder again on 设置 › 存储空间.
    MeasureStorage,
    /// Show or fold one library's parts on 存储空间, by id (`drafts` for the conversations not made libraries).
    StorageLibrary(String),
    ToggleAutoUpdate,
    ToggleAutoDownload,
    ToggleRestoreSession,
    ToggleKeepRunning,
    CheckUpdate,
    DownloadUpdate,
    InstallUpdate,
    SetAppearance(String),
    SetLanguage(Language),
    SetDensity(String),
    /// Which partition the 分区 column shows (`journals::ZONES`).
    SetJournalZone(String),
    /// 设置 › 网络's switch: the proxy kept is used, or left aside.
    ToggleProxy,
    /// 设置 › 网络's 测试: one request the way requests now go.
    TestReach,
    /// 设置 › AI › 任务 › 网页搜索: an engine's key, `websearch::AUTO` or `websearch::OFF`.
    SetWebSearchEngine(String),
    /// 设置 › AI › 任务 › 文字识别: an engine's key, `ocr::AUTO` or `ocr::OFF`.
    SetOcrEngine(String),
    /// The voice 朗读 reads in, a key of `speech::VOICES`.
    SetSpeechVoice(String),
    /// The Latin or the Chinese interface font; empty is the platform's own.
    SetFontFamily(crate::settings::FontSlot, String),
    SetFontSize(f32),
    ResetTypography,
    SetLineSpacing(wisp_core::settings::Spacing),
    SetParagraphSpacing(wisp_core::settings::Spacing),
    SetTextWidth(wisp_core::settings::TextWidth),
    ResetReadingAppearance,
    ToggleHighContrast,
    ResetAppearance,
    RecordShortcut(String),
    ClearShortcut(String),
    ResetShortcut(String),
    ResetShortcuts,
    /// 设置 › AI on one of its tabs.
    AiTab(AiTab),
    PickProtocol(Protocol),
    /// Show another provider's settings on 设置 › AI › 模型.
    SelectProvider(usize),
    /// Ask again what the account of the provider on show holds.
    ReadBalance,
    /// 设置 › AI › 模型: ask the n-th model of the provider on show one short question (`wisp_net::test_model`).
    TestModel(usize),
    /// 设置 › AI › 模型: ask the candidate model typed in `new_model` one short question before adding it.
    TestNewModel,
    /// One more provider, from a `provider::Preset` key; empty is a service set up by hand.
    AddProvider(String),
    AskRemoveProvider(usize),
    RemoveProvider(usize),
    /// One more model of the provider on show, from what its field holds (once tested, or directly when it draws).
    AddModel,
    /// Choose the kind of the candidate model to add (`None` guesses from its name).
    SetNewModelKind(Option<String>),
    RemoveModel(usize),
    /// How hard the n-th model of the provider on show thinks, a key of `ai::EFFORTS`.
    SetEffort(usize, String),
    /// How many tokens the n-th model of the provider on show takes in; 0 leaves it to its name.
    SetModelContext(usize, usize),
    /// What kind the n-th model of the provider on show is: one that answers in words (empty), draws, or judges.
    SetModelKind(usize, String),
    /// How the n-th model of the provider on show is asked for a picture, a key of `imagegen::INTERFACES`.
    SetModelInterface(usize, String),
    /// Bind a task to a profile id; an empty id follows the default.
    SetTaskModel(Task, String),
    SetScreenWorkers(usize),
    SetTranslateWorkers(usize),
    ResetTranslationPrompt,
    /// One more slash command of the person's own, on 设置 › 命令.
    AddCommand,
    /// Show or fold up the fields of the n-th.
    SelectCommand(usize),
    AskRemoveCommand(usize),
    /// 设置 › 自定义: the connectors and the skills (`custom.rs`).
    Custom(crate::custom::Do),
    RemoveCommand(usize),
    // Collect
    AddById,
    AddManual,
    ImportReferences,
    ImportZotero,
    PickZotero,
    /// One library of the Zotero database chosen to import (`zotero::Library::id`), or `None`: all of them.
    ChooseZoteroLibrary(Option<i64>),
    /// A collection of the Zotero database ticked or unticked (`zotero::Collection::id`).
    ChooseZoteroCollection(i64),
    ShowZotero,
    ImportPdfs,
    ImportFolder,
    /// Show what came of each file of the last import.
    ShowImport,
    /// Put the report of the last import away.
    CloseImport,
    /// List the papers whose PDF gave no text.
    ShowTextless,
    /// Open a paper named in the import's report.
    OpenImported(String),
    /// Look for the open full text of one paper, or of every paper of the current list that has no PDF.
    FindPdfs(Option<String>),
    StopFetch,
    ShowFetch,
    CloseFetch,
    // The list
    SetView(View),
    SetPage(Page),
    /// `FilterKey::Tag` with a tag is a plain click on it (`Filters::click_tag`); empty lets every tag choice go.
    SetFilter(FilterKey, String),
    /// 自定义区间…: the two ends of a filter by year or by impact factor, typed (`Modal::Range`).
    FilterRange(FilterKey),
    /// A click on a tag in the sidebar; true with ⌘ / Ctrl held, to add it to the chosen tags or take it out.
    ClickTag(String, bool),
    /// Exclude a tag, or let an excluded one go (⌥ / Alt-click, the tag's menu).
    ExcludeTag(String),
    /// The keyboard to the sidebar's tag search; true empties it first (its ×).
    FindTag(bool),
    ClearFilters,
    SetSort(String),
    Select(String),
    /// What is done with several papers marked in the list (`src/context.rs`): a new conversation about
    /// them alone, starred or not, a reading status, tags added (the dialog), their open full texts
    /// looked for, moved to the trash (after asking).
    AskPapers(Vec<String>),
    StarPapers(Vec<String>, bool),
    ReadPapers(Vec<String>, String),
    TagPapers(Vec<String>),
    FindPdfsOf(Vec<String>),
    AskTrashPapers(Vec<String>),
    /// 移到文献库: the dialog for a paper, a library chosen in it, and the move (`transfer.rs`).
    AskMovePaper(String),
    ChooseMoveTarget(String),
    MovePaper(String, String),
    /// 归入已有文献: the dialog for the papers whose files are to go under another, the paper chosen in it, the
    /// filing, and the way back while its toast is up (`filing.rs`).
    AskFileInto(Vec<String>),
    ChooseFileTarget(String),
    FileInto(Vec<String>, String),
    UndoFiling(Vec<String>),
    /// The files of a paper shown under its row of the list, or put away again.
    UnfoldFiles(String),
    TrashPapers(Vec<String>),
    /// Open a paper on one page of the detail panel.
    OpenPaper(String, Tab),
    CloseDetail,
    ToggleStar(String),
    Export(ExportKind),
    // One paper
    SetReading(String, String),
    SetScreening(String, String),
    EditPaper(String),
    TrashPaper(String),
    RestorePaper(String),
    AskDeletePaper(String),
    DeletePaper(String),
    /// Every paper in the trash back in the library.
    RestoreAll,
    /// 回收站多选: every paper of the trash chosen or none (全选 / 取消全选); the chosen ones restored,
    /// or deleted for good after asking.
    ChooseAllTrash(bool),
    RestorePapers(Vec<String>),
    AskDeletePapers(Vec<String>),
    DeletePapers(Vec<String>),
    /// 清空回收站: the dialog that asks for the phrase, and the deletion it leads to.
    AskEmptyTrash,
    EmptyTrash,
    OpenLink(String),
    AttachPdf(String),
    OpenFile(String),
    /// Show a stored file in the file manager.
    RevealFile(String),
    RemoveFile(String),
    /// Try once more to get the text of a stored file.
    RereadFile(String),
    /// Make a stored PDF smaller: its pictures written anew (#242).
    CompressFile(String),
    /// Save a copy of a stored PDF with its reading marks written in as PDF annotations (#273), where the person picks.
    ExportMarkedPdf(String),
    /// Have a model that sees pictures read the pages of a file that gave no text.
    ReadPages(String),
    SetTab(Tab),
    ToggleWide,
    /// Reading mode: the selected paper has the whole window, or the library is back around it.
    ToggleReading,
    /// Put the library's sidebar away, or bring it back.
    ToggleSidebar,
    /// Draw the pages of the PDF on show larger, smaller, or at a given size.
    PdfZoom(ZoomTo),
    /// The page count on the PDF's bar: a page typed is gone to (`Modal::PdfPage`).
    AskPdfPage,
    /// 页面总览 of the PDF on show, on or off: all its pages small, to go to one.
    PdfOverview,
    /// 下一页 beside the pages of the PDF on show, on or off: a column more, a page on (两页并排).
    PdfNext,
    /// Show the PDF on show twice, each half scrolled on its own, or once again.
    PdfSplit(Split),
    /// Show a file of the paper (its id) in the other half of the viewer; the file worked in is the viewer split on itself.
    PdfBeside(String),
    /// Keep this paper in a pane of its own beside the paper on show (#194), / let it go again.
    PanePaper(String),
    ClosePane(String),
    /// 留在旁边: the paper on show stays in a pane beside when another is chosen, / it does not after all.
    KeepPane,
    /// A paper kept beside becomes the paper on show, in the pane where it is (`Workspace::work_in_pane`).
    WorkInPane(String),
    /// Read this paper in a window of its own (#194).
    PaperWindow(String),
    /// Read the papers on show in this way (版面).
    Layout(wisp_core::panes::Mode),
    /// Cut a piece out of a page as a picture for a question, or stop doing so.
    PdfCut,
    /// Read the file on show aloud from the page in view; while it is read, pause or go on.
    ReadAloud,
    /// 播客: the podcast of the paper on show. While it plays, pause or go on; else the one kept (from
    /// where it was left), and only when there is none two hosts talk the paper over anew.
    Podcast,
    /// Make the podcast of the paper on show (or of the one heard) anew, after `AskNewPodcast` asked.
    NewPodcast,
    AskNewPodcast,
    /// Pause what is read aloud or talked over, or go on.
    PauseReading,
    /// Play 朗读 and 播客 at the speed of `playback::SPEEDS` at this index, from now on.
    PlaySpeed(usize),
    /// Stop what is read aloud or talked over, wherever it is of.
    StopReading,
    /// Save a copy of the podcast kept for a paper (its id): its sound (true) or its script.
    SavePodcast(String, bool),
    /// Play a kept podcast again, by its sound's attachment, from where it was left (while it plays, pause or go on).
    PlayPodcast(String),
    /// 朗读 and 播客 have no voice yet: 设置 › AI on the DeepSeek service, added when there is none, for its key.
    SetUpSpeech,
    /// Draw boxes or place sticky notes on the pages, or go back to selecting text.
    PdfTool(crate::pdf::Tool),
    /// Show this file of the selected paper in the viewer (its main text or an attachment).
    ReadFile(String),
    /// Add files to a paper as its main text (`fulltext`) or as attachments (`supplement`).
    AttachAs(String, &'static str),
    /// Show the paper's PDF to the right of its conversation, or put it away.
    BesidePdf(bool),
    /// The paper's PDF and its conversation change places (`Settings::pdf_first`).
    SwapPanes,
    /// The PDF's tools float at the page's right edge, or stand in its toolbar (`Settings::pdf_tools_floating`).
    FloatPdfTools(bool),
    /// 分屏 自由 or 固定 (`Settings::pdf_split_free`, #245): halves of any width, each zoomed on its own, or of one size zoomed together.
    PdfSplitFree(bool),
    /// 导图 on the PDF's bar: the paper's mind map as a column of the reader, or that column put away.
    PdfMap,
    /// Out of the layout for asking about a paper: the tab it was come to from, with the library around it.
    LeaveAsk,
    /// The paper's conversation floats over its PDF, or stands beside it (`Settings::floating`).
    FloatChat(bool),
    /// Fold the floating conversation into its button, or open it again.
    FoldChat(bool),
    /// Show the selected paper's title and abstract translated, or stop showing it.
    ToggleTranslation,
    RetryTranslation,
    TranslatePapers(Vec<String>, wisp_core::translation::Field),
    StopBatchTranslation,
    DismissBatchTranslation,
    /// 译文 on the pages themselves (原地替换), or on a second copy of them beside or under the pages (对照): `Settings::facing_in_place`.
    FacingPlace(bool),
    /// Show the pages again beside themselves with their paragraphs translated in place, or put that away (`src/facing.rs`).
    ToggleFacing,
    /// Translate the paragraphs not translated yet; this is also how a run stopped halfway goes on.
    FacingRun,
    /// Translate the paper kept beside (its id), as 翻译全文 does the paper on show.
    FacingRunPane(String),
    FacingStop,
    // Custom fields
    ManageFields,
    EditField(String),
    DeleteField(String),
    MoveField(String, i32),
    ToggleFieldInTable(String),
    /// Show or put away a column of the paper table that is not a custom field's.
    ToggleColumn(String),
    /// The columns of the paper table get back the widths they come with.
    ResetColumnWidths,
    AddPreset,
    // Questions
    Ask(Talk),
    /// Ask one of the suggested questions.
    Suggest(Talk, String),
    /// The assistant's card under its answer (`Chat::offer`): press its n-th button, or put it away (`None`).
    Offer(Talk, Option<usize>),
    /// Leave a slash command in the assistant's box, for its words to follow.
    StartCommand(&'static str),
    NewChat(Talk),
    /// Stop the answer being given.
    StopChat(Talk),
    /// Show the past conversations, or go back to the one in hand.
    ToggleHistory(Talk),
    /// Take up a past conversation again.
    OpenChat(Talk, String),
    AskRemoveChat(Talk, String),
    RemoveChat(Talk, String),
    /// 历史 lists every conversation of the library (true) or only those of this place: the library's, the paper's.
    HistoryAll(Talk, bool),
    /// Empty the search over 历史.
    ClearHistoryFind(Talk),
    /// The dialog that gives a conversation of 历史 a name of the person's.
    AskRenameChat(Talk, String),
    /// Take the n-th of the slash commands on offer.
    PickSlash(Talk, usize),
    /// Take the n-th of what the `@` being typed in a box is offered (`mention.rs`).
    PickMention(crate::mention::Place, usize),
    /// A paper of the panel over the library's conversation dropped on it, or taken with its plus, after the first
    /// question: its mention at the box's caret (`Workspace::mention_paper`).
    MentionPaper(Talk, String),
    /// A tag's mention clicked in a sent question: the list of its papers.
    MentionTag(String),
    /// A section's mention clicked in a sent question: 写作 there (manuscript id, section id).
    MentionSection(String, String),
    /// A figure's mention clicked in a sent question (its token's value): the figure in the paper's PDF.
    MentionFigure(String),
    /// Choose pictures to go with the next question.
    AttachImages(Talk),
    RemoveAttachment(Talk, usize),
    /// Let go of a Word file that was to go with the next question.
    RemoveDocument(Talk, usize),
    /// Take a passage quoted for the next question away again.
    RemoveQuote(Talk, usize),
    /// The person's answer to the assistant wanting to change the library.
    AnswerTools(Talk, Consent),
    /// Unfold or fold the tool calls that start at this place in the conversation.
    ToggleTools(Talk, usize),
    /// Show or put away the summary a compacted conversation goes on from.
    ToggleCompacted(Talk),
    CopyText(String),
    /// The diagram of this Mermaid source, to the clipboard as a picture on white.
    CopyDiagram(String),
    /// 撤回: the question at this place in the conversation and all after it are taken back; its words and pictures return to the box.
    Retract(Talk, usize),
    /// 重新回答: the answer that ends at this place is asked for again, from the question before it.
    Regenerate(Talk, usize),
    /// 从这里分支: a new conversation of what was said up to the end of the answer at this place; the one it came from stays in the history.
    Fork(Talk, usize),
    /// Put back what the last 撤回 took away.
    Unretract(Talk),
    /// Save an answer and its source labels to a new manuscript, at the person's request.
    AnswerToManuscript(String, String),
    /// Add this text (an answer, or what is selected in one) at the end of a section of the manuscript.
    InsertInto(String, String),
    /// Start the review record of the conversation with the library (`src/chat.rs`; `/review` does it by itself).
    StartReview,
    ReviewToManuscript,
    /// The review's record in place of the conversation, or the conversation again.
    ToggleReview,
    /// A step of the review with the text it takes (an answer, or what is selected in one; empty for a confirmation).
    Review(ReviewDo, String),
    /// The dialog that changes a text of the review's record.
    EditReviewText(ReviewText),
    AskRemoveReviewSection(String),
    RemoveReviewSection(String),
    /// The dialog for a claim and its source: a row of the record (its id), or a new one for this claim.
    EditReviewEvidence(Option<String>, String),
    PickEvidencePaper(String),
    PickEvidenceSource(String),
    PickEvidenceSection(String),
    /// The person's judgment of a row of evidence; the one it has already takes it back.
    JudgeReviewEvidence(String, wisp_core::review::EvidenceJudgment),
    AskRemoveReviewEvidence(String),
    RemoveReviewEvidence(String),
    /// Copy what is selected in a text of the window that is not an answer (`src/select.rs`).
    CopySelected,
    /// A tag of the library under another name (the dialog), or taken off every paper (after asking).
    RenameTag(String),
    AskRemoveTag(String),
    RemoveTag(String),
    /// The pointer over the text of an answer (by its place in the conversation): selecting it.
    PickText(Talk, usize, crate::markdown::Pointer, crate::markdown::Place),
    /// Copy what is selected in an answer; or put it into the question box as a quotation.
    CopyPicked(Talk),
    QuotePicked(Talk),
    /// Make a note of the selected paper out of an answer about it: the question, and the answer.
    AnswerToNote(String, String),
    /// Show or put away the panel of the papers the library's questions are put to.
    ToggleMaterials,
    /// What that panel does to the scope of the next conversation (`ScopeDo`).
    Scope(ScopeDo),
    /// Show every source of an answer (by its place in the conversation), or the first few again.
    ToggleSources(Talk, usize),
    /// Open a paper; with a page, its PDF there; with a quoted sentence too, that sentence highlighted.
    GotoSource(String, Option<usize>, String),
    /// The same in one file of the paper: an attachment by its id, or (`None`) the main text.
    GotoFile(String, Option<String>, Option<usize>, String),
    /// The same for a citation as an answer writes it: the label of a conversation, an attachment's number, a page, a quotation.
    GotoCited(Talk, String, Option<usize>, Option<usize>, String),
    // Searches and imports
    NewSearch,
    EditSearch(String),
    AskRemoveSearch(String),
    RemoveSearch(String),
    /// Run a saved search without importing, to see what it finds.
    TrySearch(String),
    CloseTrial,
    RunSearch(String),
    RunAllSearches,
    AskUndoImport(String),
    UndoImport(String),
    PickSource(String),
    // The guide
    /// Show the guide; true starts at the choice a new library gets.
    OpenGuide(bool),
    GuideSearch,
    GuideToggleSource(String),
    GuideGenerate,
    GuideToggleIdea(usize),
    GuideImport,
    // Screening
    /// Ask the assistant about the pending papers it has not judged yet.
    ScreenStart,
    /// The assistant asked again about the papers already decided, against the goal as it is now;
    /// with `true`, only about the included ones (继续筛: the next, tighter round of the funnel).
    ScreenRecheck(bool),
    /// Move the decided papers that are not included to the trash, after asking.
    AskTrashUnincluded,
    ScreenStop,
    /// The person's decision on a paper, made on the screening page.
    ScreenDecide(String, String),
    /// Show only the queue's papers with this suggestion (a key of `SCREENINGS`), those without one (`none`), or all (empty).
    ScreenFilter(String),
    /// How the queue is worked through: with the assistant's run over it (批量), one paper at a time (逐篇), or its suggestions confirmed together (确认).
    ScreenMode(ScreenMode),
    /// 逐篇: show the paper at this place of the round; one past the last is the round's summary.
    ScreenGo(usize),
    /// 逐篇: take the round's last decision back.
    ScreenUndo,
    /// Ask the assistant about this one paper.
    ScreenAskOne(String),
    /// The paper being screened beside the page: its PDF when it has one, else its record.
    ScreenBeside,
    /// 确认: count a paper in or leave it out; give it another status; the suggested verdict on show; all of
    /// those on show in or out; the least sure first; the paper the keys act on.
    ConfirmTick(String),
    ConfirmChoose(String, String),
    ConfirmPart(String),
    ConfirmTickAll(bool),
    ConfirmSort,
    /// A click on a row: the keys act on it, and it is shown in full under the list again.
    ConfirmCursor(String),
    /// The × of the row shown in full under the list: it is put away until a row is clicked. Esc here leaves for the queue, as the page's foot says.
    ConfirmClose,
    /// Write what is ticked; take that back.
    ConfirmApply,
    ConfirmUndo,
    /// How often the open library runs its searches on its own, in hours; 0 turns it off.
    SetAutoSync(u32),
    ToggleSearch(String),
    // Writing
    /// Write another section of the manuscript: its id, or a key of `structure::SECTIONS` for the first section of that kind.
    WriteSection(String),
    /// Write another of the library's manuscripts.
    WriteManuscript(String),
    NewManuscript,
    AskRemoveManuscript(String),
    RemoveManuscript(String),
    /// What the manuscript is written in: a code of `manuscript::LANGUAGES`.
    WriteLanguage(String),
    /// Add a section (`None`) or change one's name and kind: the dialog.
    EditSection(Option<String>),
    PickSectionKind(String),
    MoveSection(String, i64),
    AskRemoveSection(String),
    RemoveSection(String),
    /// Cut the section being written at the caret's paragraph: that paragraph its heading (true), or the first of a new section.
    SplitSection(bool),
    /// Put a section back into the one before it.
    MergeSection(String),
    /// The whole manuscript to read (true), or one section to write.
    WriteReading(bool),
    /// The top-left arrow of a manuscript: out of 写作, back to the page it was opened from.
    LeaveWrite,
    ExportManuscript(ManuscriptExport),
    /// 设置 › 写作: the style manuscripts cite in that chose none of their own, by its key (`csl::style`).
    DefaultCiteStyle(String),
    /// 「导出」 › 引用样式: the dialog of the manuscript's own style; a click in it looks at a style.
    OpenCiteStyles,
    LookCiteStyle(String),
    /// 添加 .csl 文件, in either place the styles are listed.
    AddCiteStyle,
    /// A style the person added: shown where its file is, taken out of the list, and put back by the toast's 撤销.
    RevealCiteStyle(String),
    RemoveCiteStyle(String),
    RestoreCiteStyle { file: String, xml: String, default: bool },
    /// 调整 on the list of styles: the sliders by a style's name show what of it can be changed here, or put it
    /// away; a detail is set (none: 还原, the style's own again); what is set is kept as a style of the person's
    /// own; and the sparkle takes the style to the assistant.
    AdjustCiteStyles,
    TuneCiteStyle(Option<crate::cite_styles::Knob>),
    KeepCiteStyle,
    AskCiteStyle(String),
    /// The section's earlier versions.
    WriteHistory,
    HistoryPick(usize),
    /// Cite a paper (by id) at the caret: its key, `[@Zhang2024]` (`src/cite.rs`).
    WriteCite(String),
    /// 引用 on a sentence of a paper: cite the paper and keep the sentence as what the citation rests on.
    WriteCiteFrom(String, wisp_core::passage::Candidate),
    /// The passages citations rest on (write_source.rs).
    Source(crate::write_source::Do),
    /// The reference list of the whole manuscript (true) or of the part being written.
    WriteCiteScope(bool),
    /// Put citations in the part being written right: (start, end, what is there, what replaces it), last first.
    WriteCiteFix(Vec<(usize, usize, String, String)>),
    WriteCopyReferences,
    /// The page of the panel beside the manuscript: the candidates, the comments, or the reference list.
    WriteSide(Side),
    /// The reference list of the whole manuscript: beside it, or, while it is read through, at its end.
    WriteReferences,
    /// The cited papers as BibTeX.
    WriteExportCited,
    /// Have the model sort the full texts into parts (false), or read the pages of the files that gave no text (true).
    WriteRun(bool),
    WriteStop,
    /// Look for candidates: the selected text, or the paragraph at the caret.
    WriteFind,
    /// 后退 (false) / 前进 (true) through the papers read (#194).
    StepHistory(bool),
    /// Show (true) or hide the card under the citation the pointer is on (`Settings::citation_cards_hidden`).
    CitationCards(bool),
    /// Only the papers with this tag (empty: any), and only the starred ones.
    WriteOnly(String, bool),
    /// Show or hide the sentences around a candidate, by its card's id.
    WriteUnfold(String),
    /// 批注, and manuscripts that start from a file (`comments.rs`).
    Comment(crate::comments::Do),
    /// Have the model work on the selected text or the caret's paragraph; stop it; use what it gave.
    WriteAssist(ai::Assist),
    WriteAssistStop,
    WriteAssistUse(crate::write_assist::Use),
    /// 配引用's card: tick a paper, show more, write the ticked citations (write_cite.rs).
    WriteCited(crate::write_cite::Do),
    /// The list of what the assistant changed in the section on show (`revise.rs`).
    Revise(crate::revise::Do),
    /// 写作's other mode, where the assistant writes (`write_agent.rs`).
    Agent(crate::write_agent::Do),
    /// 写作's 资料库: the pictures a manuscript can show, brought in and put into it (`write_figures.rs`).
    WriteFigures(crate::write_figures::Do),
    /// Looking for words in the manuscript (`write_seek.rs`).
    Seek(crate::write_seek::Do),
    /// Looking for words through the whole library: full texts, marks, notes (`library_find.rs`).
    LibraryFind(crate::library_find::Do),
    // The figure library (`src/figures.rs`)
    /// Only the figures with this tag; the tag chosen already, or none, shows all of them.
    FigureTag(String),
    /// Look at a figure and change what is written about it: the dialog.
    OpenFigure(String),
    /// Open the PDF a figure was cut from, at its place.
    FigureSource(String),
    /// Ask about a figure's paper with the figure as a picture.
    AskFigure(String),
    CopyFigure(String),
    /// Save a figure's picture somewhere else.
    SaveFigure(String),
    AskRemoveFigure(String),
    RemoveFigure(String),
    /// Add one of the tags on offer to the dialog's tags.
    FigureAddTag(String),
    /// Have a model that sees pictures write a field of the dialog: the legend, tags, or a reading for the notes.
    FigureInfer(wisp_core::figure::Ask),
    /// 生图: the dialog a picture is drawn in from a description, to keep as a figure.
    DrawFigure,
    /// Have the model that draws make the dialog's picture from its description; while it draws, stop it.
    FigureDraw,
    /// The shape of the picture the dialog asks for.
    FigureShape(wisp_core::imagegen::Shape),
    /// 提取: everything the page, its dialog and the sparkles by a paper's fields do (`src/extract.rs`).
    Extract(crate::extract::Do),
    /// 方法库 and 导言库: everything their pages and a passage's dialog do (`src/excerpts.rs`).
    Excerpt(crate::excerpts::Do),
    // Dialogs
    CloseModal,
    SubmitModal,
    /// 按题目补全: look for the records a paper without its metadata may be.
    CompletePaper(String),
    /// 按 DOI / PMID 补全: the record the paper's own number names, to take its missing fields from.
    CompleteById(String),
    /// Look at one of the records found, or (`None`) go back to all of them.
    CompleteChoose(Option<usize>),
    /// Take or leave one field of the record looked at.
    CompleteField(&'static str),
    PickColor(String),
    PickKind(FieldKind),
    PickOption(String, String),
    ToggleInTable,
}

impl Workspace {
    pub fn on(&self, cmd: Cmd, cx: &mut Context<Self>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, _, window, cx| this.run(cmd.clone(), window, cx))
    }

    pub fn run(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        // Anything else than a key ends the wait for a shortcut's new key.
        if !matches!(cmd, Cmd::RecordShortcut(_)) { self.stop_recording(cx) }
        cx.notify();
        match cmd {
            Cmd::OpenLibrary(id) => self.open_library(&id, window, cx),
            Cmd::MoveLibrary(moved, onto) => {
                if moved == onto { return }
                self.settings.library_order = libraries::reorder(&self.libraries, &self.settings.library_order, &moved, &onto);
                self.save_settings(cx);
                cx.notify();
            }
            Cmd::GoHome => {
                self.write_snapshot("leave");
                self.close_library(cx);
                // One level up: from a conversation, or the assistant's page, to the home page.
                (self.home.open, self.home.listing) = (false, false);
                self.settings_view = None;
                self.modal = None;
                theme::set_accent("blue");
                self.libraries = libraries::list(&self.root);
                window.set_window_title("Wisp Study");
                window.focus(&self.focus);
            }
            Cmd::Draft(what) => self.run_draft(what, window, cx),
            Cmd::AskMovePaper(paper) => self.ask_move_paper(paper, cx),
            Cmd::ChooseMoveTarget(id) => {
                if let Some(Modal::MoveTo { to, .. }) = self.modal.as_mut() { *to = Some(id) }
                self.modal_error.clear();
            }
            Cmd::MovePaper(paper, to) => self.move_paper(paper, to, window, cx),
            Cmd::AskFileInto(papers) => self.ask_file_into(papers, window, cx),
            Cmd::ChooseFileTarget(id) => {
                if let Some(Modal::FileInto { to, .. }) = self.modal.as_mut() { *to = Some(id) }
                self.modal_error.clear();
            }
            Cmd::FileInto(papers, to) => self.file_into(papers, to, window, cx),
            Cmd::UndoFiling(papers) => self.undo_filing(papers, window, cx),
            Cmd::UnfoldFiles(id) => {
                if let Some(lib) = self.lib.as_mut() {
                    if !lib.unfolded.remove(&id) { lib.unfolded.insert(id); }
                    lib.lay_rows();
                }
            }
            Cmd::NewLibrary => self.library_dialog(None, window, cx),
            Cmd::EditLibrary(id) => self.library_dialog(Some(id), window, cx),
            Cmd::AskRemoveLibrary(id) => {
                let Some(info) = self.libraries.iter().find(|l| l.id == id).or(self.lib.as_ref().map(|l| &l.info)) else { return };
                self.modal_error.clear();
                // A conversation goes the way a library goes: its folder to removed/, nothing deleted.
                if info.draft {
                    self.modal = Some(Modal::Confirm {
                        title: tf!("删除对话「{}」？", "Delete conversation “{}”?", info.name),
                        message: tf!("对话会从「最近对话」里消失。它的文件夹（对话、{} 篇收进的文献和交来的稿件）会移到数据目录下的 removed 文件夹，不会被删除，需要时可以手动移回。", "The conversation will disappear from Recent conversations. Its folder (the conversation, {} papers kept and any manuscript) moves to removed in the data directory. You can move it back manually.", info.paper_count),
                        action: t("删除").into(),
                        cmd: Cmd::RemoveLibrary(id),
                    });
                    return;
                }
                self.modal = Some(Modal::Confirm {
                    title: tf!("移除文献库「{}」？", "Remove library “{}”?", info.name),
                    message: tf!("文献库会从首页消失。它的文件夹（{} 篇文献、PDF 和笔记）会移到数据目录下的 removed 文件夹，不会被删除，需要时可以手动移回。", "The library will disappear from home. Its folder ({} papers, PDFs and notes) moves to removed in the data directory. You can move it back manually.", info.paper_count),
                    action: t("移除").into(),
                    cmd: Cmd::RemoveLibrary(id),
                });
            }
            Cmd::RemoveLibrary(id) => {
                self.modal = None;
                // The database must be closed before its folder can move; one answering off screen goes with it.
                if self.lib.as_ref().is_some_and(|l| l.info.id == id) { self.lib = None }
                self.parked.retain(|lib| lib.info.id != id);
                self.bell.forget(Some(&id));
                let Some(info) = self.libraries.iter().find(|l| l.id == id).cloned() else { return };
                match libraries::remove(&self.root, &info) {
                    Ok(target) => self.say(tf!("已移到 {}", "Moved to {}", target.display()), cx),
                    Err(err) => self.fail(format!("{err:#}"), cx),
                }
                self.libraries = libraries::list(&self.root);
            }
            Cmd::ExportLibrary(id, attachments) => self.export_library(&id, attachments, cx),
            Cmd::ImportLibrary => self.pick_library_file(cx),
            Cmd::RevealFolder => {
                let path = self.lib.as_ref().map_or(self.root.clone(), |l| l.info.dir.clone());
                cx.open_with_system(&path);
            }
            Cmd::OpenUrl(url) => cx.open_url(&url),
            Cmd::AnswerStar(star) => {
                if star { cx.open_url(wisp_core::update::PROJECT_URL) }
                self.star_ask = false;
                self.settings.star_answered = true;
                self.save_settings(cx);
            }
            Cmd::Tour(what) => self.tour_do(what, window, cx),
            Cmd::Bell(what) => self.bell_do(what, window, cx),
            Cmd::Tutorial(what) => self.tutorial_do(what, window, cx),
            Cmd::DismissContactHint => {
                self.settings.contact_hint_dismissed = true;
                self.save_settings(cx);
            }
            Cmd::NoteOpen(talk, notion) => self.open_note(talk, notion, window, cx),
            Cmd::NotePick(at) => self.pick_note(at, cx),
            Cmd::NoteDestination(notion) => self.note_destination(notion, cx),
            Cmd::NotePreview => self.preview_note(cx),
            Cmd::NoteSave(notion) => self.save_note(notion, cx),
            Cmd::NoteResolve(saved) => self.resolve_note(saved, cx),

            Cmd::OpenSettings(page) => self.open_settings(page, window, cx),
            Cmd::CloseSettings => self.close_settings(window, cx),
            Cmd::PickDataDir => self.pick_data_dir(cx),
            Cmd::ResetDataDir => self.set_data_dir(None, cx),
            Cmd::RevealDataDir => cx.open_with_system(&self.root),
            Cmd::RevealDir(dir) => cx.open_with_system(&dir),
            Cmd::MeasureStorage => self.measure_storage(cx),
            Cmd::StorageLibrary(id) => self.toggle_storage_library(id, cx),
            Cmd::ToggleAutoUpdate => {
                self.settings.auto_update = !self.settings.auto_update;
                self.save_settings(cx);
                if self.settings.auto_update { self.check_update(false, cx) }
            }
            Cmd::ToggleAutoDownload => {
                self.settings.auto_download = !self.settings.auto_download;
                self.save_settings(cx);
                // A release already found is fetched now rather than at the next check.
                if self.settings.auto_download && let UpdateState::Available(release, Some(target)) = self.update.clone() { self.download_update(release, target, false, cx) }
            }
            Cmd::ToggleRestoreSession => {
                self.settings.restore_session = !self.settings.restore_session;
                self.save_settings(cx);
                // Off: nothing is kept. On: `keep_session` writes where the window is now.
                let _ = std::fs::remove_file(session_file());
                self.session = Session::default();
            }
            Cmd::ToggleKeepRunning => {
                self.settings.keep_running = !self.settings.keep_running;
                self.save_settings(cx);
            }
            Cmd::CheckUpdate => self.check_update(true, cx),
            Cmd::DownloadUpdate => {
                if let UpdateState::Available(release, Some(target)) = self.update.clone() { self.download_update(release, target, true, cx) }
            }
            Cmd::InstallUpdate => self.install_update(cx),
            Cmd::SetAppearance(value) => {
                self.settings.appearance = value;
                self.save_settings(cx);
                self.apply_appearance(window);
            }
            Cmd::SetFontFamily(slot, value) => self.set_font_family(slot, value, window, cx),
            Cmd::SetFontSize(value) => {
                self.settings.font_size = wisp_core::settings::font_size(value);
                self.save_settings(cx);
                self.apply_typography(window, cx);
            }
            Cmd::ResetTypography => {
                self.settings.font_family.clear();
                self.settings.cjk_font_family.clear();
                self.settings.font_size = wisp_core::settings::DEFAULT_FONT_SIZE;
                self.save_settings(cx);
                self.apply_typography(window, cx);
            }
            Cmd::SetLineSpacing(value) => {
                self.settings.reading_appearance.line_spacing = value;
                self.save_settings(cx);
                self.apply_typography(window, cx);
            }
            Cmd::SetParagraphSpacing(value) => {
                self.settings.reading_appearance.paragraph_spacing = value;
                self.save_settings(cx);
                self.apply_typography(window, cx);
            }
            Cmd::SetTextWidth(value) => {
                self.settings.reading_appearance.width = value;
                self.save_settings(cx);
                self.apply_typography(window, cx);
            }
            Cmd::ResetReadingAppearance => {
                self.settings.reading_appearance = Default::default();
                self.save_settings(cx);
                self.apply_typography(window, cx);
            }
            Cmd::ToggleHighContrast => {
                self.settings.high_contrast = !self.settings.high_contrast;
                self.save_settings(cx);
                self.apply_appearance(window);
            }
            Cmd::ResetAppearance => {
                self.settings.reset_appearance();
                self.dismiss_font_picker(window, cx);
                self.save_settings(cx);
                self.apply_typography(window, cx);
                self.apply_appearance(window);
            }
            Cmd::SetDensity(value) => {
                self.settings.density = value;
                self.save_settings(cx);
                window.refresh();
            }
            Cmd::ToggleProxy => self.toggle_proxy(cx),
            Cmd::TestReach => self.test_reach(cx),
            Cmd::SetWebSearchEngine(engine) => {
                self.settings.web_search.engine = engine;
                self.save_settings(cx);
                cx.notify();
            }
            Cmd::SetOcrEngine(engine) => {
                self.settings.ocr.engine = engine;
                self.save_settings(cx);
                cx.notify();
            }
            Cmd::SetJournalZone(value) => {
                self.settings.journal_zone = value;
                self.save_settings(cx);
                // A list sorted by the other partition is sorted by this one, and the library keeps that sort: the
                // 分区 header has the arrow whenever the list is in its order.
                let mut failed = None;
                if let Some(lib) = self.lib.as_mut() {
                    // So is a list filtered by zone: the same zones, of the partition now on show.
                    let (sort, zone) = (crate::library::sort_in_zone(&lib.filters.sort, &self.settings), filter::zone_in(&lib.filters.zone, crate::library::zone_sort(&self.settings)));
                    if sort != lib.filters.sort || zone != lib.filters.zone {
                        (lib.filters.sort, lib.filters.zone) = (sort, zone);
                        lib.refilter();
                    }
                    let kept = crate::library::sort_in_zone(&lib.info.sort, &self.settings);
                    if kept != lib.info.sort {
                        lib.info.sort = kept;
                        failed = libraries::save(&lib.info).err();
                    }
                }
                if let Some(err) = failed { self.fail(format!("{err:#}"), cx) }
                window.refresh();
            }
            Cmd::SetLanguage(language) => {
                self.settings.language = language;
                crate::i18n::set_language(language);
                crate::background::set_menus(cx);
                self.save_settings(cx);
                cx.refresh_windows();
            }
            Cmd::RecordShortcut(id) => self.start_recording(id, window, cx),
            Cmd::ClearShortcut(id) => self.set_shortcut(&id, Some(String::new()), cx),
            Cmd::ResetShortcut(id) => self.set_shortcut(&id, None, cx),
            Cmd::ResetShortcuts => self.reset_shortcuts(cx),
            Cmd::PickProtocol(value) => self.pick_protocol(value, cx),
            Cmd::AiTab(tab) => self.open_ai_tab(tab, window, cx),
            Cmd::SelectProvider(index) => self.select_provider(index, cx),
            Cmd::ReadBalance => self.read_balance_shown(cx),
            Cmd::TestModel(index) => self.test_model(index, cx),
            Cmd::TestNewModel => self.test_new_model(cx),
            Cmd::AddProvider(preset) => self.add_provider(&preset, cx),
            Cmd::AskRemoveProvider(index) => {
                let Some(provider) = self.settings.providers.get(index) else { return };
                let serves: Vec<&str> = Task::ALL.into_iter().filter(|task| provider.models.iter().any(|m| m.id == self.settings.tasks.get(*task))).map(|task| t(task.label())).collect();
                let after = if serves.is_empty() { String::new() } else { tf!("它的模型现在用于{}，删除后这些任务改用默认的模型。", "Its models currently serve {}. These tasks will use the default model after removal.", serves.join("、")) };
                self.modal_error.clear();
                self.modal = Some(Modal::Confirm { title: tf!("删除服务「{}」？", "Delete provider “{}”?", provider.title()), message: tf!("它的地址、API Key 和 {} 个模型会从 settings.json 里删除。{after}", "Its URL, API key and {} models will be removed from settings.json. {after}", provider.models.len()), action: t("删除").into(), cmd: Cmd::RemoveProvider(index) });
            }
            Cmd::RemoveProvider(index) => {
                self.run(Cmd::CloseModal, window, cx);
                self.remove_provider(index, cx);
            }
            Cmd::AddModel => self.add_model(cx),
            Cmd::SetNewModelKind(kind) => self.set_new_model_kind(kind, cx),
            Cmd::RemoveModel(index) => self.remove_model(index, cx),
            Cmd::SetEffort(index, value) => self.set_effort(index, value, cx),
            Cmd::SetModelContext(index, tokens) => self.set_model_context(index, tokens, cx),
            Cmd::SetModelKind(index, kind) => {
                let Some(provider) = self.settings_view.as_ref().map(|view| view.provider()) else { return };
                self.settings.set_model_kind(provider, index, &kind);
                self.save_settings(cx);
            }
            Cmd::SetModelInterface(index, value) => {
                let Some(provider) = self.settings_view.as_ref().map(|view| view.provider()) else { return };
                let Some(model) = self.settings.providers.get_mut(provider).and_then(|p| p.models.get_mut(index)) else { return };
                model.interface = value;
                self.save_settings(cx);
            }
            Cmd::SetTaskModel(task, id) => {
                self.settings.tasks.set(task, &id);
                self.save_settings(cx);
            }
            Cmd::SetScreenWorkers(count) => {
                self.settings.screen_workers = count.clamp(1, wisp_core::settings::MAX_SCREEN_WORKERS);
                self.save_settings(cx);
            }
            Cmd::SetTranslateWorkers(count) => {
                self.settings.translate_workers = count.clamp(1, wisp_core::settings::MAX_SCREEN_WORKERS);
                self.save_settings(cx);
            }
            Cmd::ResetTranslationPrompt => self.reset_translation_prompt(cx),
            Cmd::SetSpeechVoice(voice) => {
                self.settings.speech_voice = voice;
                self.save_settings(cx);
            }
            Cmd::AddCommand => self.add_command(cx),
            Cmd::Custom(what) => self.run_custom(what, window, cx),
            Cmd::SelectCommand(index) => self.select_command(index, cx),
            Cmd::AskRemoveCommand(index) => {
                let Some(command) = self.settings.commands.get(index) else { return };
                self.modal_error.clear();
                self.modal = Some(Modal::Confirm { title: tf!("删除命令 /{}？", "Delete command /{}?", command.name), message: t("它的模板会从 settings.json 里删除。用过它的对话不受影响：对话里保存着当时发出的内容。").into(), action: t("删除").into(), cmd: Cmd::RemoveCommand(index) });
            }
            Cmd::RemoveCommand(index) => {
                self.run(Cmd::CloseModal, window, cx);
                self.remove_command(index, cx);
            }

            Cmd::AddById => {
                let input = self.modal_input("DOI、PMID 或它们的链接；多个时一行一个，或用空格分开", "", true, window, cx);
                // Enter looks them up, as when the field held one line; a pasted list brings its own line breaks.
                input.update(cx, |input, _| input.submit_on_enter = true);
                let submit = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| if *event == InputEvent::Submit { this.run(Cmd::SubmitModal, window, cx) });
                self.modal_subscriptions.push(submit);
                input.read(cx).focus(window);
                self.modal_error.clear();
                self.modal = Some(Modal::AddById { input, run: None });
            }
            Cmd::AddManual => self.paper_dialog(None, window, cx),
            Cmd::EditPaper(id) => self.paper_dialog(Some(id), window, cx),
            Cmd::ImportReferences => self.pick_files(t("导入"), false, cx),
            Cmd::ImportZotero => {
                if self.lib.is_none() { return self.fail(t("请先打开一个文献库。"), cx) }
                self.modal_error.clear();
                self.modal = Some(if self.lib.as_ref().and_then(|l| l.zotero.as_ref()).is_some_and(|r| r.running) { Modal::ZoteroReport } else { Modal::Zotero { open: zotero::default_database().is_some_and(|path| zotero::in_use(&path)) } });
            }
            Cmd::PickZotero => self.pick_zotero(cx),
            Cmd::ChooseZoteroLibrary(id) => {
                if let Some(Modal::ZoteroLibraries { chosen, collections, picked, .. }) = self.modal.as_mut() {
                    *chosen = id;
                    // Ticks in another library than the one now chosen would import nothing of it.
                    picked.retain(|p| id.is_none_or(|l| collections.iter().any(|c| c.id == *p && c.library == l)));
                }
            }
            Cmd::ChooseZoteroCollection(id) => {
                if let Some(Modal::ZoteroLibraries { collections, picked, .. }) = self.modal.as_mut() {
                    if let Some(at) = picked.iter().position(|p| *p == id) { picked.remove(at); }
                    else if !zotero::under_picked(collections, picked, id) {
                        // A collection takes in what is under it: ticks there are now part of this one.
                        picked.retain(|p| !zotero::under_picked(collections, &[id], *p));
                        picked.push(id);
                    }
                }
            }
            Cmd::ShowZotero => {
                self.modal_error.clear();
                self.modal = Some(Modal::ZoteroReport);
            }
            Cmd::ImportPdfs => self.pick_files(t("导入"), true, cx),
            Cmd::ImportFolder => self.pick_folders(cx),
            Cmd::ShowImport => {
                self.modal_subscriptions.clear();
                self.modal_error.clear();
                self.modal = Some(Modal::ImportReport);
            }
            Cmd::CloseImport => {
                if let Some(lib) = self.lib.as_mut() && lib.import.as_ref().is_some_and(|run| !run.running()) { lib.import = None }
                if matches!(self.modal, Some(Modal::ImportReport)) { self.run(Cmd::CloseModal, window, cx) }
            }
            Cmd::OpenImported(id) => {
                self.run(Cmd::CloseModal, window, cx);
                if let Some(lib) = self.lib.as_mut() && matches!(lib.page, Page::Trash | Page::Sources | Page::Guide) { lib.page = Page::Papers }
                self.select(Some(id), window, cx);
            }
            Cmd::FindPdfs(only) => self.fetch_start(only, cx),
            Cmd::StopFetch => {
                if let Some(run) = self.lib.as_mut().and_then(|lib| lib.fetching.as_mut()) {
                    run.stop();
                }
            }
            Cmd::ShowFetch => {
                self.modal_subscriptions.clear();
                self.modal_error.clear();
                self.modal = Some(Modal::FetchReport);
            }
            Cmd::CloseFetch => {
                if let Some(lib) = self.lib.as_mut() && lib.fetching.as_ref().is_some_and(|run| !run.running()) { lib.fetching = None }
                if matches!(self.modal, Some(Modal::FetchReport)) { self.run(Cmd::CloseModal, window, cx) }
            }
            Cmd::ShowTextless => {
                if matches!(self.modal, Some(Modal::ImportReport)) { self.run(Cmd::CloseModal, window, cx) }
                if let Some(lib) = self.lib.as_mut() {
                    lib.filters = Filters { fulltext: "textless".into(), sort: crate::library::sort_in_zone(&lib.info.sort, &self.settings), ..Default::default() };
                    lib.search.update(cx, |input, cx| input.set_text("", cx));
                    lib.page = Page::Papers;
                    lib.refilter();
                }
            }

            Cmd::SetView(view) => {
                if self.lib.as_ref().is_some_and(|lib| lib.finding.open) { self.run_find(crate::library_find::Do::Close, window, cx) }
                if let Some(lib) = self.lib.as_mut() {
                    lib.filters.view = view;
                    lib.page = Page::Papers;
                    lib.refilter();
                }
            }
            Cmd::SetPage(page) => {
                if self.lib.as_ref().is_some_and(|l| l.page == Page::Write && page != Page::Write) { self.write_snapshot("leave") }
                // What 提取 last wrote is on show, with the way back, until the page is left.
                if let Some(lib) = self.lib.as_mut().filter(|l| l.page == Page::Extract && page != Page::Extract) { lib.extract.last = None }
                // The same for what 确认 last wrote: taken back later, it would undo decisions made since.
                if let Some(lib) = self.lib.as_mut().filter(|l| l.page == Page::Screen && page != Page::Screen) { lib.screen.confirmed = None }
                if page == Page::Write {
                    self.write_ensure(cx);
                    self.agent_attend(cx);
                    self.agent_versions(cx);
                }
                if let Some(lib) = self.lib.as_mut() {
                    // What was chosen in the trash is for that visit: none chosen on coming back, nor after leaving.
                    if lib.page == Page::Trash || page == Page::Trash { (lib.trash_chosen, lib.trash_anchor) = (vec![], None) }
                    if page == Page::Write { lib.enter_write() } else { lib.page = page }
                    lib.reading = false;
                    if page == Page::Ask { lib.library_chat.input.read(cx).focus(window) }
                    if page == Page::Write {
                        lib.writing.editor.read(cx).focus(window);
                        lib.writing.structures = lib.store.structures().unwrap_or_default();
                        self.write_texts_changed(cx);
                    }
                }
                if page == Page::Screen { self.round_arrived(window, cx) }
            }
            Cmd::WriteSection(_) | Cmd::WriteManuscript(_) | Cmd::NewManuscript | Cmd::AskRemoveManuscript(_) | Cmd::RemoveManuscript(_) | Cmd::WriteLanguage(_) | Cmd::EditSection(_) | Cmd::PickSectionKind(_) | Cmd::MoveSection(..) | Cmd::AskRemoveSection(_) | Cmd::RemoveSection(_) | Cmd::SplitSection(_) | Cmd::MergeSection(_) | Cmd::WriteReading(_) | Cmd::LeaveWrite | Cmd::ExportManuscript(_) | Cmd::WriteHistory | Cmd::HistoryPick(_) => self.run_manuscript(cmd, window, cx),
            Cmd::FigureTag(_) | Cmd::OpenFigure(_) | Cmd::FigureSource(_) | Cmd::AskFigure(_) | Cmd::CopyFigure(_) | Cmd::SaveFigure(_) | Cmd::AskRemoveFigure(_) | Cmd::RemoveFigure(_) | Cmd::FigureAddTag(_) | Cmd::FigureInfer(_) | Cmd::DrawFigure | Cmd::FigureDraw | Cmd::FigureShape(_) => self.run_figures(cmd, window, cx),
            Cmd::WriteCiteFrom(id, from) => {
                self.cite_paper(&id, Some(from), cx);
                if let Some(lib) = self.lib.as_ref() { lib.writing.editor.read(cx).focus(window) }
            }
            Cmd::Source(what) => self.run_source(what, cx),
            Cmd::WriteCite(id) => {
                self.cite_paper(&id, None, cx);
                if let Some(lib) = self.lib.as_ref() { lib.writing.editor.read(cx).focus(window) }
            }
            Cmd::WriteCiteScope(whole) => {
                if let Some(lib) = self.lib.as_mut() { lib.citing.whole = whole }
            }
            Cmd::WriteCiteFix(pieces) => self.cite_fix(pieces, cx),
            Cmd::DefaultCiteStyle(style) => self.set_default_cite_style(style, cx),
            Cmd::OpenCiteStyles => self.open_cite_styles(window, cx),
            Cmd::LookCiteStyle(style) => {
                if let Some(Modal::CiteStyle { chosen, .. }) = self.modal.as_mut() { *chosen = style }
            }
            Cmd::AddCiteStyle => self.add_cite_style(cx),
            Cmd::RevealCiteStyle(style) => self.reveal_cite_style(&style, cx),
            Cmd::RemoveCiteStyle(style) => self.remove_cite_style(style, cx),
            Cmd::RestoreCiteStyle { file, xml, default } => self.restore_cite_style(&file, &xml, default, cx),
            Cmd::AdjustCiteStyles => self.adjust_cite_styles(cx),
            Cmd::TuneCiteStyle(knob) => self.tune_cite_style(knob, cx),
            Cmd::KeepCiteStyle => {
                self.keep_cite_style(cx);
            }
            Cmd::AskCiteStyle(style) => self.ask_cite_style(&style, window, cx),
            Cmd::WriteCopyReferences => self.copy_references(cx),
            Cmd::WriteSide(side) => {
                // The page on show asked for again folds the panel under a narrow manuscript, or opens it; another page opens it.
                if let Some(lib) = self.lib.as_mut() {
                    lib.writing.side_open = lib.writing.side != side || !lib.writing.side_open;
                    lib.writing.side = side;
                }
            }
            Cmd::WriteReferences => {
                // Beside the conversation the list is a column of its own: the row opens and closes it.
                let agent = self.settings.agent_writing;
                if let Some(lib) = self.lib.as_mut() && agent && !lib.writing.reading && lib.writing.side == Side::References && lib.citing.whole {
                    lib.writing.side = Side::Candidates;
                } else if let Some(lib) = self.lib.as_mut() {
                    (lib.writing.side, lib.writing.side_open, lib.citing.whole) = (Side::References, true, true);
                    // Read through, the manuscript ends in its references: there is no panel beside it.
                    if lib.writing.reading { lib.citing.read_scroll.scroll_to_bottom() }
                }
            }
            Cmd::WriteExportCited => self.export_cited(cx),
            Cmd::WriteRun(reading) => self.write_run_start(reading, None, cx),
            Cmd::WriteFind => self.write_find(false, cx),
            Cmd::StepHistory(forward) => self.step_history(forward, window, cx),
            Cmd::CitationCards(on) => {
                self.settings.citation_cards_hidden = !on;
                self.save_settings(cx);
            }
            Cmd::WriteOnly(tag, starred) => self.write_narrow(tag, starred, cx),
            Cmd::WriteUnfold(card) => {
                if let Some(unfolded) = self.lib.as_mut().map(|lib| &mut lib.writing.search.unfolded) && !unfolded.remove(&card) { unfolded.insert(card); }
            }
            Cmd::Comment(what) => self.run_comment(what, window, cx),
            Cmd::WriteAssist(assist) => self.assist_start(assist, cx),
            Cmd::WriteAssistStop => self.assist_stop(cx),
            Cmd::WriteAssistUse(how) => self.assist_use(how, window, cx),
            Cmd::WriteCited(what) => self.cite_do(what, window, cx),
            Cmd::Revise(what) => self.run_revise(what, cx),
            Cmd::Seek(what) => self.run_seek(what, window, cx),
            Cmd::LibraryFind(what) => self.run_find(what, window, cx),
            Cmd::Agent(what) => self.run_agent(what, window, cx),
            Cmd::WriteFigures(what) => self.run_write_figures(what, window, cx),
            Cmd::WriteStop => {
                if let Some(lib) = self.lib.as_mut() {
                    let sorting = &mut lib.writing.sorting;
                    sorting.run += 1;
                    sorting.todo.clear();
                    sorting.working.clear();
                }
            }
            Cmd::SetFilter(key, value) => {
                if let Some(lib) = self.lib.as_mut() {
                    let f = &mut lib.filters;
                    match key {
                        FilterKey::Tag if value.is_empty() => (f.tags, f.not_tags) = (vec![], vec![]),
                        FilterKey::Tag => f.click_tag(&value, false),
                        FilterKey::Year => f.year = value,
                        FilterKey::Impact => f.impact = value,
                        FilterKey::Zone => f.zone = value,
                        FilterKey::Reading => f.reading = value,
                        FilterKey::Screening => f.screening = value,
                        FilterKey::Fulltext => f.fulltext = value,
                        FilterKey::Source => f.source = value,
                        FilterKey::Preprint => f.preprint = value,
                        FilterKey::Field(id) => {
                            if value.is_empty() { f.fields.remove(&id); } else { f.fields.insert(id, value); }
                        }
                    }
                    lib.page = Page::Papers;
                    lib.refilter();
                }
            }
            Cmd::FilterRange(key) => {
                let Some(current) = self.lib.as_ref().map(|lib| if key == FilterKey::Impact { lib.filters.impact.clone() } else { lib.filters.year.clone() }) else { return };
                // A span set comes back into the two boxes; a year alone and 近 N 年 start them empty.
                let (from, to) = current.split_once('-').filter(|_| !current.starts_with("last:")).unwrap_or_default();
                self.modal_subscriptions.clear();
                self.modal_error.clear();
                let (low, high) = if key == FilterKey::Impact { (self.modal_input("最低，例如 5", from, false, window, cx), self.modal_input("最高，可留空", to, false, window, cx)) } else { (self.modal_input("最早，例如 2000", from, false, window, cx), self.modal_input("最晚，可留空", to, false, window, cx)) };
                low.read(cx).focus(window);
                self.modal = Some(Modal::Range { key, low, high });
            }
            Cmd::ClickTag(tag, add) => {
                if let Some(lib) = self.lib.as_mut() {
                    // Away from the list the tags are not lit, so a plain click chooses just this one.
                    if lib.page == Page::Papers || add {
                        lib.filters.click_tag(&tag, add);
                    } else {
                        lib.filters.not_tags.retain(|t| *t != tag);
                        lib.filters.tags = vec![tag];
                    }
                    lib.page = Page::Papers;
                    lib.refilter();
                }
            }
            Cmd::ExcludeTag(tag) => {
                if let Some(lib) = self.lib.as_mut() {
                    if lib.page == Page::Papers || !lib.filters.excluded(&tag) { lib.filters.exclude_tag(&tag) }
                    lib.page = Page::Papers;
                    lib.refilter();
                }
            }
            Cmd::FindTag(empty) => {
                if let Some(lib) = self.lib.as_ref() {
                    if empty { lib.tag_find.update(cx, |input, cx| input.set_text("", cx)) }
                    lib.tag_find.read(cx).focus(window);
                }
            }
            Cmd::ClearFilters => {
                if let Some(lib) = self.lib.as_mut() {
                    lib.filters.clear();
                    lib.search.update(cx, |input, cx| input.set_text("", cx));
                    lib.refilter();
                }
            }
            Cmd::SetSort(sort) => {
                let Some(lib) = self.lib.as_mut() else { return };
                lib.filters.sort = sort.clone();
                lib.refilter();
                lib.info.sort = sort;
                if let Err(err) = libraries::save(&lib.info) { self.fail(format!("{err:#}"), cx) }
            }
            Cmd::Select(id) => self.select(Some(id), window, cx),
            Cmd::AskPapers(_) | Cmd::StarPapers(..) | Cmd::ReadPapers(..) | Cmd::TagPapers(_) | Cmd::FindPdfsOf(_) | Cmd::AskTrashPapers(_) | Cmd::TrashPapers(_) => self.run_batch(cmd, window, cx),
            Cmd::OpenPaper(id, tab) => {
                self.select(Some(id), window, cx);
                self.run(Cmd::SetTab(tab), window, cx);
            }
            Cmd::CloseDetail => self.select(None, window, cx),
            Cmd::ToggleStar(id) => {
                self.edit_paper(&id, cx, |p| p.starred = !p.starred);
            }
            Cmd::Export(kind) => self.export(kind, cx),

            Cmd::SetReading(id, value) => {
                self.edit_paper(&id, cx, |p| p.reading = value);
            }
            Cmd::SetScreening(id, value) => {
                self.edit_paper(&id, cx, |p| p.decide(&value, ""));
            }
            Cmd::TrashPaper(id) => {
                // The paper on show goes: the one that takes its row is shown next, so a list can be gone through.
                let row = self.lib.as_ref().filter(|lib| lib.selected.as_deref() == Some(id.as_str())).and_then(|lib| lib.shown.iter().position(|&i| lib.papers[i].id == id));
                if self.change(cx, |lib| lib.store.trash_paper(&id)) {
                    let next = row.and_then(|row| self.lib.as_ref().and_then(|lib| lib.shown.get(row).or(lib.shown.last()).map(|&i| lib.papers[i].id.clone())));
                    if next.is_some() { self.select(next, window, cx) }
                    self.say(t("已移到回收站。"), cx)
                }
            }
            Cmd::RestorePaper(id) => {
                if self.change(cx, |lib| lib.store.restore_paper(&id)) { self.say(t("已恢复。"), cx) }
            }
            Cmd::AskDeletePaper(id) => {
                let title = self.lib.as_ref().and_then(|l| l.trash.iter().find(|t| t.paper.id == id)).map(|t| t.paper.title.clone()).unwrap_or_default();
                self.modal_error.clear();
                self.modal = Some(Modal::Confirm { title: t("彻底删除这篇文献？").into(), message: tf!("「{title}」连同它的笔记、词条、标注、PDF 和图片，会从磁盘上完全删除，无法恢复。删除之后它不再阻止重复导入。", "“{title}”, with its notes, fields, annotations, PDFs and pictures, will be deleted from disk completely and cannot be recovered. It will no longer block re-import."), action: t("彻底删除").into(), cmd: Cmd::DeletePaper(id) });
            }
            Cmd::DeletePaper(id) => {
                self.modal = None;
                self.change(cx, |lib| lib.store.purge_trashed(&id));
            }
            Cmd::RestoreAll => {
                let ids: Vec<String> = self.lib.as_ref().map(|lib| lib.trash.iter().map(|entry| entry.paper.id.clone()).collect()).unwrap_or_default();
                self.run(Cmd::RestorePapers(ids), window, cx);
            }
            Cmd::ChooseAllTrash(all) => {
                if let Some(lib) = self.lib.as_mut() {
                    lib.trash_chosen = if all { lib.trash.iter().map(|entry| entry.paper.id.clone()).collect() } else { vec![] };
                    lib.trash_anchor = None;
                }
                window.focus(&self.focus);
            }
            Cmd::RestorePapers(ids) => {
                let done = self.change(cx, |lib| {
                    for id in &ids { lib.store.restore_paper(id)? }
                    Ok(())
                });
                if done { self.say(if ids.len() == 1 { t("已恢复。").to_string() } else { tf!("已恢复 {} 篇。", "Restored {} papers.", ids.len()) }, cx) }
            }
            Cmd::AskDeletePapers(ids) => {
                let total = self.lib.as_ref().map_or(0, |lib| lib.trash.len());
                // One is that paper's own question; all of them is emptying the trash, which asks for the phrase.
                if let [id] = ids.as_slice() { return self.run(Cmd::AskDeletePaper(id.clone()), window, cx) }
                if ids.is_empty() { return }
                if ids.len() == total { return self.run(Cmd::AskEmptyTrash, window, cx) }
                self.modal_error.clear();
                let count = ids.len();
                self.modal = Some(Modal::Confirm { title: tf!("彻底删除这 {count} 篇文献？", "Delete these {count} papers permanently?"), message: t("它们连同各自的笔记、词条、标注、PDF 和图片，会从磁盘上完全删除，无法恢复。删除之后它们不再阻止重复导入。").into(), action: t("彻底删除").into(), cmd: Cmd::DeletePapers(ids) });
            }
            Cmd::DeletePapers(ids) => {
                self.modal = None;
                let mut count = 0;
                if self.change(cx, |lib| lib.store.purge_trashed_papers(&ids).map(|n| count = n)) { self.say(tf!("已彻底删除 {count} 篇。", "Deleted {count} papers permanently."), cx) }
                window.focus(&self.focus);
            }
            Cmd::AskEmptyTrash => {
                if self.lib.as_ref().is_none_or(|lib| lib.trash.is_empty()) { return }
                self.modal_subscriptions.clear();
                self.modal_error.clear();
                let phrase = self.modal_input("", "", false, window, cx);
                // The confirm button lights up as the phrase is typed.
                let typed = cx.subscribe_in(&phrase, window, |_, _, event: &InputEvent, _, cx| if *event == InputEvent::Changed { cx.notify() });
                self.modal_subscriptions.push(typed);
                phrase.read(cx).focus(window);
                self.modal = Some(Modal::EmptyTrash { phrase });
            }
            Cmd::EmptyTrash => {
                self.modal = None;
                self.modal_subscriptions.clear();
                let mut count = 0;
                if self.change(cx, |lib| lib.store.empty_trash().map(|n| count = n)) { self.say(tf!("已彻底删除 {count} 篇。", "Deleted {count} papers permanently."), cx) }
                window.focus(&self.focus);
            }
            Cmd::OpenLink(id) => {
                if let Some(link) = self.lib.as_ref().and_then(|l| l.paper(&id)).map(|p| p.link()).filter(|l| !l.is_empty()) {
                    cx.open_url(&link);
                }
            }
            Cmd::AttachPdf(id) => self.pick_attachment(id, None, cx),
            Cmd::OpenFile(id) => {
                if let Some(lib) = &self.lib && let Some(file) = lib.files.iter().find(|f| f.id == id) {
                    cx.open_with_system(&lib.store.file_path(file));
                }
            }
            Cmd::RevealFile(id) => {
                if let Some(lib) = &self.lib && let Some(file) = lib.files.iter().find(|f| f.id == id) {
                    cx.reveal_path(&lib.store.file_path(file));
                }
            }
            Cmd::RemoveFile(id) => {
                if self.change(cx, |lib| lib.store.remove_file(&id)) {
                    if let Some(lib) = self.lib.as_mut() { lib.close_pdf() }
                    // Another PDF of the paper, if it has one, takes the viewer.
                    self.ensure_pdf(cx);
                    self.say(t("附件已移除（文件移到了 removed 文件夹）。"), cx);
                }
            }
            Cmd::RereadFile(id) => self.reread_file(id, cx),
            Cmd::CompressFile(id) => self.compress_file(id, cx),
            Cmd::ExportMarkedPdf(id) => self.export_marked_pdf(id, None, cx),
            Cmd::ReadPages(id) => self.write_run_start(true, Some(id), cx),
            Cmd::SetTab(tab) => {
                // Without a model the question box is not drawn: the keyboard stays with the window, so Esc is still heard.
                let asking = !self.settings.profile_for_or_default(Task::Chat).key().is_empty();
                if let Some(lib) = self.lib.as_mut() {
                    if tab == Tab::Ask && lib.tab != Tab::Ask { lib.ask_from = lib.tab }
                    lib.tab = tab;
                    if tab == Tab::Ask && asking { lib.paper_chat.input.read(cx).focus(window) } else if tab == Tab::Ask { window.focus(&self.focus) }
                }
                self.ensure_pdf(cx);
            }
            Cmd::ToggleWide => {
                if let Some(lib) = self.lib.as_mut() { lib.wide = !lib.wide }
            }
            Cmd::ToggleReading => {
                if let Some(lib) = self.lib.as_mut().filter(|lib| lib.selected.is_some()) {
                    lib.reading = !lib.reading;
                    // Reading starts at the paper itself: the PDF, when there is one and the record was on show.
                    if lib.reading && lib.tab == Tab::Info && lib.selected.clone().is_some_and(|id| lib.pdf_file(&id).is_some()) { lib.tab = Tab::Pdf }
                }
                self.ensure_pdf(cx);
            }
            Cmd::HomeList(rows) => {
                if self.settings.home_list != rows {
                    self.settings.home_list = rows;
                    self.save_settings(cx);
                }
            }
            Cmd::ToggleSidebar => {
                // Asking about a paper put the sidebar away for the while: wanting it back is leaving that.
                if self.asking(window) {
                    self.run(Cmd::LeaveAsk, window, cx);
                    if !self.settings.sidebar_hidden { return }
                }
                self.settings.sidebar_hidden = !self.settings.sidebar_hidden;
                self.save_settings(cx);
            }
            Cmd::LeaveAsk => {
                if let Some(lib) = self.lib.as_mut().filter(|lib| lib.tab == Tab::Ask) { lib.tab = lib.ask_from }
                self.ensure_pdf(cx);
                window.focus(&self.focus);
            }
            Cmd::PdfCut => {
                if let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.toggle_cutting(cx)) }
            }
            Cmd::StopReading => {
                if let Some(lib) = self.lib.as_mut() { lib.stop_reading(cx) }
                cx.notify();
            }
            Cmd::SavePodcast(paper, audio) => self.save_podcast(&paper, audio, cx),
            Cmd::SetUpSpeech => {
                self.modal = None;
                let known = self.settings.providers.iter().position(|p| p.preset == "deepseek" || wisp_core::speech::is_service(&p.base_url));
                self.open_ai_tab(crate::settings::AiTab::Models, window, cx);
                match known {
                    Some(index) => self.select_provider(index, cx),
                    None => self.add_provider("deepseek", cx),
                }
            }
            Cmd::PlayPodcast(sound) => {
                let again = self.lib.as_ref().and_then(|lib| lib.aloud.as_ref()).is_some_and(|r| r.sound.as_deref() == Some(sound.as_str()));
                if again { return self.run(Cmd::PauseReading, window, cx) }
                self.play_podcast(&sound, cx);
            }
            Cmd::PauseReading => {
                let Some(lib) = self.lib.as_mut() else { return };
                if let Some(reading) = lib.aloud.as_ref() {
                    let paused = reading.status().paused;
                    reading.pause(!paused);
                    lib.aloud_shown = Some(reading.status());
                }
                cx.notify();
            }
            Cmd::PlaySpeed(index) => {
                let speed = wisp_core::playback::SPEEDS.get(index).copied().unwrap_or(1.);
                self.settings.play_speed = speed;
                self.save_settings(cx);
                if let Some(reading) = self.lib.as_ref().and_then(|lib| lib.aloud.as_ref()) { reading.set_speed(speed) }
                cx.notify();
            }
            Cmd::AskNewPodcast => {
                self.modal = Some(Modal::Confirm { title: t("重新生成播客？").into(), message: t("会请模型重写对话稿，再用 DeepSeek 的语音服务读出来，按字数计费。已存的那一版仍留在附件里。").into(), action: t("重新生成").into(), cmd: Cmd::NewPodcast });
                cx.notify();
            }
            Cmd::NewPodcast => {
                self.modal = None;
                // Of the paper on show; from the bar at the window's foot, of the one heard.
                let Some(lib) = self.lib.as_ref() else { return };
                let on_show = lib.pdf.as_ref().map(|pdf| pdf.read(cx).resource_id().to_string());
                let heard = lib.aloud.as_ref().filter(|r| r.kind == crate::read_aloud::Kind::Podcast).map(|r| r.resource.clone());
                let away = heard.is_some() && lib.podcast_away(cx).is_some();
                let Some(resource) = (if away { heard } else { on_show.or(heard) }) else { return };
                self.podcast(&resource, cx);
            }
            Cmd::ReadAloud | Cmd::Podcast => {
                let kind = if matches!(cmd, Cmd::Podcast) { crate::read_aloud::Kind::Podcast } else { crate::read_aloud::Kind::Aloud };
                // While its button's reading is here, the button pauses it and goes on: the place is kept.
                // The other one, or 播客 of another paper, stops it and starts its own.
                let Some(lib) = self.lib.as_ref() else { return };
                let Some(on_show) = lib.pdf.as_ref().map(|pdf| pdf.read(cx).resource_id().to_string()) else { return };
                if lib.playing_here(&on_show) == Some(kind) { return self.run(Cmd::PauseReading, window, cx) }
                if kind == crate::read_aloud::Kind::Aloud { return self.read_aloud(None, cx) }
                // A podcast of the paper was kept: it plays from where it was left. Making one anew is asked for apart (`AskNewPodcast`).
                let paper = lib.files.iter().find(|f| f.id == on_show).map(|f| f.paper_id.clone()).unwrap_or_default();
                match podcast_files(lib, &paper).0.map(|f| f.id.clone()) {
                    Some(sound) => self.play_podcast(&sound, cx),
                    None => self.podcast(&on_show, cx),
                }
            }
            Cmd::PdfTool(tool) => {
                if let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.set_tool(tool, cx)) }
            }
            Cmd::ReadFile(file) => {
                let Some(lib) = self.lib.as_mut() else { return };
                let Some(paper) = lib.files.iter().find(|f| f.id == file).map(|f| f.paper_id.clone()) else { return };
                if lib.selected.as_deref() != Some(paper.as_str()) { self.select(Some(paper.clone()), window, cx) }
                let Some(lib) = self.lib.as_mut() else { return };
                // Read where there is room: beside the paper's conversation when it is open, else on the PDF tab.
                if !(lib.tab == Tab::Ask && lib.split) { lib.tab = Tab::Pdf }
                let main = lib.main_pdf(&paper).map(|f| f.id.clone());
                self.show_file(&paper, (main.as_deref() != Some(file.as_str())).then_some(file), cx);
                cx.notify();
            }
            Cmd::AttachAs(paper, kind) => self.pick_attachment(paper, Some(kind), cx),
            // With the translation in the second half, one page again is putting the translation away.
            Cmd::PdfSplit(Split::Off) if self.lib.as_ref().is_some_and(|lib| lib.facing_shown()) => self.toggle_facing(cx),
            Cmd::FacingPlace(in_place) => {
                // A habit: it stays from paper to paper, and every paper on show follows it.
                self.settings.facing_in_place = in_place;
                self.save_settings(cx);
                self.sync_facing(cx);
                if let Some(lib) = self.lib.as_ref() {
                    let views: Vec<Entity<PdfView>> = lib.pdf.iter().chain(lib.pins.iter().map(|pin| &pin.view)).cloned().collect();
                    for view in views { view.update(cx, |pdf, cx| pdf.set_in_place(in_place, cx)) }
                }
            }
            Cmd::PdfSplit(split) => {
                // Beside or under the pages is where a translation stands that is not on them.
                if split != Split::Off && self.settings.facing_in_place && self.lib.as_ref().is_some_and(|lib| lib.facing_shown()) { self.run(Cmd::FacingPlace(false), window, cx) }
                // Two files: one again is the one worked in, else the two halves change how they stand.
                if self.lib.as_ref().is_some_and(|lib| lib.beside.is_some()) {
                    if split == Split::Off { self.close_beside(cx) } else if let Some(beside) = self.lib.as_mut().and_then(|lib| lib.beside.as_mut()) { beside.split = split }
                    return cx.notify();
                }
                // 自由: the file again in a viewer of its own beside it (with 译文 on, the split says where the translation stands).
                // (A viewer that shows the map beside its pages stays one viewer: its second half comes inside it.)
                if split != Split::Off && self.settings.pdf_split_free && self.lib.as_ref().is_some_and(|lib| !lib.facing_shown() && !lib.pdf.as_ref().is_some_and(|pdf| pdf.read(cx).map_shown())) {
                    self.split_itself(split, None, false, cx);
                    return;
                }
                // It is for this paper: the next one opens whole.
                if let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.choose_split(split, cx)) }
            }
            Cmd::PdfBeside(file) => self.show_beside(file, cx),
            Cmd::PanePaper(paper) => {
                self.pin(paper, cx);
                self.room_for_panes(window, cx);
            }
            Cmd::KeepPane => self.keep_pane(window, cx),
            Cmd::PaperWindow(paper) => self.open_paper_window(paper, window, cx),
            Cmd::ClosePane(paper) => {
                // The pane of the paper on show: the one after it is worked in from here on (the one before, when it is the last).
                if self.lib.as_ref().is_some_and(|lib| lib.selected.as_deref() == Some(paper.as_str())) {
                    let Some(next) = self.lib.as_ref().and_then(|lib| lib.pins.get(lib.main_at).or(lib.pins.last()).map(|pin| pin.paper.clone())) else { return };
                    self.work_in_pane(&next, cx);
                }
                self.unpin(&paper, cx);
            }
            Cmd::WorkInPane(paper) => self.work_in_pane(&paper, cx),
            Cmd::Layout(mode) => self.set_layout(mode, window, cx),
            Cmd::BesidePdf(show) => {
                // Beside the conversation the window is laid out for the two (`asking`); put away, the library is back around it.
                if let Some(lib) = self.lib.as_mut() { lib.split = show }
                self.ensure_pdf(cx);
            }
            Cmd::SwapPanes => {
                // Only the order is drawn anew: the PDF, the conversation and the question being typed stay as they are.
                self.settings.pdf_first = !self.settings.pdf_first;
                self.save_settings(cx);
            }
            Cmd::PdfSplitFree(free) => self.set_split_free(free, cx),
            Cmd::FloatPdfTools(on) => {
                // Only where the tools are drawn changes; a tool in use stays in use.
                self.settings.pdf_tools_floating = on;
                self.save_settings(cx);
            }
            Cmd::FloatChat(on) => self.float_chat(on, window, cx),
            Cmd::FoldChat(fold) => self.fold_chat(fold, window, cx),
            Cmd::PdfNext => {
                let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()) else { return };
                // Three columns already, none of them the next page: nothing can come, and the way that is left is said.
                let nowhere = { let view = pdf.read(cx); view.full() && !view.next_shown() };
                if nowhere { return self.say(t("已经有三栏了。点一栏左上角的标签，可以让它改为显示下一页。"), cx) }
                pdf.update(cx, |pdf, cx| pdf.toggle_next(cx));
            }
            Cmd::PdfOverview => {
                if let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.toggle_overview(cx)) }
            }
            Cmd::AskPdfPage => {
                let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()) else { return };
                let (page, count) = { let view = pdf.read(cx); (view.current_page(), view.page_count()) };
                if count == 0 { return }
                self.modal_subscriptions.clear();
                self.modal_error.clear();
                // The page read at, selected: a number typed takes its place.
                let at = page.to_string();
                let page = self.modal_input("页码", &at, false, window, cx);
                page.update(cx, |input, cx| input.select_range(0..at.len(), cx));
                page.read(cx).focus(window);
                self.modal = Some(Modal::PdfPage { page, count });
            }
            Cmd::PdfZoom(to) => {
                if let Some(lib) = self.lib.as_mut().filter(|lib| lib.tab == Tab::Pdf || lib.split) && let Some(pdf) = lib.pdf.clone() {
                    // A fit chosen is the one the next paper opens at; a percentage stays with this paper.
                    lib.zoom = pdf.update(cx, |pdf, cx| {
                        pdf.zoom_to(to, cx);
                        pdf.zoom()
                    })
                    .carried(lib.zoom);
                }
            }
            Cmd::ToggleTranslation => {
                if let Some(lib) = self.lib.as_mut() { lib.show_translation = !lib.show_translation }
                self.load_title_translations();
                self.translate_selected(true, cx);
            }
            Cmd::RetryTranslation => self.translate_selected(true, cx),
            Cmd::TranslatePapers(ids, field) => self.translate_batch(ids, field, cx),
            Cmd::StopBatchTranslation => {
                if let Some(run) = self.lib.as_mut().and_then(|lib| lib.translation_run.as_mut()) { run.batch.stop() }
            }
            Cmd::ToggleFacing => {
                // A translation beside the pages is a column more: with three there, say so instead of turning 译文 on to no effect.
                let full = !self.settings.facing_in_place && self.lib.as_ref().is_some_and(|lib| !lib.facing && lib.pdf.as_ref().is_some_and(|pdf| pdf.read(cx).full()));
                if full { return self.say(t("已经有三栏了：先关掉一栏，或在某一栏的标签里选「显示译文」。"), cx) }
                self.toggle_facing(cx)
            }
            Cmd::PdfMap => {
                let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()) else { return };
                if self.ensure_map(&pdf, cx).is_none() { return }
                self.join_for_map(&pdf, cx);
                if !pdf.update(cx, |pdf, cx| pdf.toggle_map(cx)) { self.say(t("已经有三栏了：先关掉一栏，或在某一栏的标签里选「显示导图」。"), cx) }
            }
            Cmd::FacingRun => self.run_facing(cx),
            Cmd::FacingRunPane(paper) => self.run_pinned_facing(&paper, cx),
            Cmd::FacingStop => self.stop_facing(cx),
            Cmd::DismissBatchTranslation => {
                if let Some(lib) = self.lib.as_mut().filter(|lib| lib.translation_run.as_ref().is_some_and(|run| !run.batch.running())) { lib.translation_run = None }
            }

            Cmd::Extract(what) => self.run_extract(what, window, cx),
            Cmd::Excerpt(what) => self.run_excerpts(what, window, cx),
            Cmd::ManageFields => self.fields_dialog(None, window, cx),
            Cmd::EditField(id) => self.fields_dialog(Some(id), window, cx),
            Cmd::DeleteField(id) => {
                self.change(cx, |lib| lib.store.delete_field(&id));
                self.fields_dialog(None, window, cx);
            }
            Cmd::MoveField(id, step) => {
                self.change(cx, |lib| lib.store.move_field(&id, step));
            }
            Cmd::ToggleFieldInTable(id) => {
                self.change(cx, |lib| {
                    let Some(mut field) = lib.fields.iter().find(|f| f.id == id).cloned() else { return Ok(()) };
                    field.in_table = !field.in_table;
                    lib.store.save_field(&mut field)
                });
            }
            Cmd::ToggleColumn(key) => {
                let hidden = &mut self.settings.columns_hidden;
                match hidden.iter().position(|k| *k == key) {
                    Some(i) => drop(hidden.remove(i)),
                    None => hidden.push(key),
                }
                self.save_settings(cx);
            }
            Cmd::ResetColumnWidths => {
                self.settings.column_widths.clear();
                self.save_settings(cx);
            }
            Cmd::AddPreset => {
                self.change(cx, |lib| {
                    // BiblioCell's single-cell fields, as a starting point. A field's name and options are the person's from here on: they are made in the interface's language.
                    for (name, kind, options) in [("物种", FieldKind::Tags, vec![]), ("组织", FieldKind::Tags, vec![]), ("测序类型", FieldKind::Tags, vec!["scRNA-seq", "snRNA-seq", "scATAC-seq", "Multiome", "空间转录组"]), ("测序平台", FieldKind::Tags, vec!["10x Genomics Chromium", "BD Rhapsody", "Smart-seq2"])] {
                        if lib.fields.iter().any(|f| f.name == name || f.name == t(name)) { continue }
                        lib.store.save_field(&mut FieldDef { name: t(name).into(), kind, options: options.into_iter().map(|option| t(option).to_string()).collect(), in_table: name == "物种", ..Default::default() })?;
                    }
                    Ok(())
                });
            }

            Cmd::Ask(paper) => self.ask(paper, cx),
            Cmd::Offer(talk, pressed) => {
                let Some((_, offer)) = self.lib.as_mut().and_then(|lib| lib.chat_mut(talk).offer.take()) else { return };
                // Put away in a conversation not yet a library, it does not give way to the window's own card of 建成文献库: that is 稍后 as well.
                if pressed.is_none() && self.in_draft() { return self.run(Cmd::Draft(crate::draft::Do::Later), window, cx) }
                let Some(action) = pressed.and_then(|n| offer.actions.get(n)).filter(|action| self.offer_fits(action)) else { return cx.notify() };
                let draft = self.lib.as_ref().map(|lib| lib.info.id.clone()).filter(|_| self.in_draft());
                let cmd = match action.kind.as_str() {
                    "reply" => Cmd::Suggest(talk, action.arg.clone()),
                    "promote_library" => match draft { Some(id) => Cmd::Draft(crate::draft::Do::Promote(id)), None => return },
                    "find_pdfs" => Cmd::FindPdfs(None),
                    _ => Cmd::SetPage(Page::from_key(&action.arg)),
                };
                self.run(cmd, window, cx);
            }
            Cmd::Suggest(paper, question) => {
                if let Some(lib) = self.lib.as_ref() {
                    lib.chat(paper).input.update(cx, |input, cx| input.set_text(&question, cx));
                }
                self.ask(paper, cx);
            }
            Cmd::StartCommand(name) => {
                if let Some(lib) = self.lib.as_ref() {
                    lib.library_chat.input.update(cx, |input, cx| input.set_text(&format!("/{name} "), cx));
                    lib.library_chat.input.read(cx).focus(window);
                }
                self.chat_input(Talk::Library, InputEvent::Changed, cx);
            }
            Cmd::NewChat(paper) => {
                // The library's conversation, answering, goes on off screen (历史 takes it up again); another's answer
                // still arriving is stopped first, so that what there is of it is kept.
                let aside = paper == Talk::Library && self.lib.as_ref().is_some_and(|lib| lib.library_chat.busy);
                if aside { self.chat_aside(cx) } else { self.stop_chat(paper, cx) }
                // A conversation whose last save failed is tried once more; if that fails too, it says so itself.
                if self.lib.as_ref().is_some_and(|lib| !lib.chat(paper).kept) { self.save_chat(paper, cx) }
                let mut kept = false;
                if let Some(lib) = self.lib.as_mut() {
                    let chat = chat_of!(lib, paper);
                    kept = chat.kept;
                    chat.reset(cx);
                    chat.input.read(cx).focus(window);
                    // A new conversation with the library is about the list on show again.
                    if paper == Talk::Library { lib.scope_reset(cx) }
                }
                if aside { self.say(t("上一段对话还在后台回答，在「历史」里可以找到。"), cx) } else if kept { self.say(t("上一段对话已保存，在「历史」里可以找到。"), cx) }
            }
            Cmd::StopChat(paper) => self.stop_chat(paper, cx),
            Cmd::ToggleHistory(paper) => {
                let Some(lib) = self.lib.as_mut() else { return };
                let listed = if lib.chat(paper).history.is_none() { lib.list_history(paper).map(Some) } else { Ok(None) };
                let list = match listed {
                    Ok(list) => list,
                    Err(err) => return self.fail(format!("{err:#}"), cx),
                };
                // The list opens unsearched, with the keyboard in its search; closed, the keyboard is the box's again.
                let chat = lib.chat_mut(paper);
                chat.history = list;
                chat.history_find.update(cx, |input, cx| input.set_text("", cx));
                chat.history_hits = None;
                if chat.history.is_some() { chat.history_find.read(cx).focus(window) } else { chat.input.read(cx).focus(window) }
            }
            Cmd::HistoryAll(paper, all) => {
                let Some(lib) = self.lib.as_mut().filter(|lib| lib.history_spans(paper) && lib.chat(paper).history.is_some()) else { return };
                lib.history_all = all;
                match lib.list_history(paper) {
                    Ok(list) => lib.chat_mut(paper).history = Some(list),
                    Err(err) => return self.fail(format!("{err:#}"), cx),
                }
                lib.chat(paper).history_find.read(cx).focus(window);
            }
            Cmd::ClearHistoryFind(paper) => {
                if let Some(lib) = self.lib.as_mut() {
                    let chat = lib.chat_mut(paper);
                    chat.history_find.update(cx, |input, cx| input.set_text("", cx));
                    chat.history_hits = None;
                    chat.history_find.read(cx).focus(window);
                }
            }
            Cmd::AskRenameChat(paper, id) => {
                let Some(title) = self.lib.as_ref().and_then(|l| l.chat(paper).history.as_ref()?.iter().find(|c| c.id == id).map(|c| c.title.clone())) else { return };
                self.modal_subscriptions.clear();
                self.modal_error.clear();
                let name = self.modal_input("对话的名称", &title, false, window, cx);
                name.read(cx).focus(window);
                self.modal = Some(Modal::RenameChat { talk: paper, id, name });
            }
            // A row of 全部对话 that is another's is taken up in its own place: the paper selected with its conversation, or the library's page.
            Cmd::OpenChat(paper, id) if self.lib.as_ref().is_some_and(|lib| lib.chat(paper).history.as_ref().and_then(|list| list.iter().find(|c| c.id == id)).is_some_and(|c| lib.talk_about(paper).is_none_or(|about| about != c.paper_id))) => {
                let Some(lib) = self.lib.as_mut() else { return };
                let owner = lib.chat(paper).history.as_ref().and_then(|list| list.iter().find(|c| c.id == id)).map(|c| c.paper_id.clone()).unwrap_or_default();
                lib.chat_mut(paper).history = None;
                if owner.is_empty() {
                    // Asked about, the paper has the window: out of that first, so the library's page is on show.
                    if self.asking(window) { self.run(Cmd::LeaveAsk, window, cx) }
                    self.run(Cmd::SetPage(Page::Ask), window, cx);
                    self.run(Cmd::OpenChat(Talk::Library, id), window, cx);
                } else {
                    // As a click in the list: the paper selected, its PDF or record following; then its conversation, beside the list.
                    if self.lib.as_ref().is_some_and(|lib| lib.page == Page::Ask) { self.run(Cmd::SetPage(Page::Papers), window, cx) }
                    // It was a conversation with that paper alone, and papers side by side have one of their own (#194): those kept
                    // beside are let go, the paper itself staying where it is read when it is one of them.
                    self.work_in_pane(&owner, cx);
                    let beside: Vec<String> = self.lib.as_ref().map(|lib| lib.pins.iter().map(|pin| pin.paper.clone()).collect()).unwrap_or_default();
                    for paper in beside { self.unpin(&paper, cx) }
                    if let Some(lib) = self.lib.as_mut() { lib.keep = false }
                    self.select(Some(owner), window, cx);
                    self.run(Cmd::SetTab(Tab::Ask), window, cx);
                    self.run(Cmd::OpenChat(Talk::Paper, id), window, cx);
                }
            }
            Cmd::OpenChat(paper, id) => {
                // The conversation on show, answering: 历史 only closes over it.
                let here = self.lib.as_ref().is_some_and(|lib| lib.chat(paper).busy && lib.chat(paper).id == id);
                if here {
                    if let Some(lib) = self.lib.as_mut() { lib.chat_mut(paper).history = None }
                } else {
                    // The library's conversation, answering, goes on off screen as another is taken up; one off screen is taken up as it stands.
                    if paper == Talk::Library && self.lib.as_ref().is_some_and(|lib| lib.library_chat.busy) { self.chat_aside(cx) } else { self.stop_chat(paper, cx) }
                    if paper != Talk::Library || !self.take_aside(&id) { self.open_chat(paper, &id, cx) }
                }
                if let Some(lib) = self.lib.as_ref() { lib.chat(paper).input.read(cx).focus(window) }
            }
            Cmd::AskRemoveChat(paper, id) => {
                let Some(title) = self.lib.as_ref().and_then(|l| l.chat(paper).history.as_ref()?.iter().find(|c| c.id == id).map(|c| c.title.clone())) else { return };
                // A review is reached through its conversation alone, so it goes with it: say so first.
                let reviewed = self.lib.as_ref().is_some_and(|l| l.store.review_for_conversation(&id).is_ok_and(|review| review.is_some()));
                self.modal_error.clear();
                let warning = if reviewed { t("\n\n这次对话的综述记录（写作要求、大纲、正文和证据）会一起删除；已经存为稿件的内容不受影响。") } else { "" };
                self.modal = Some(Modal::Confirm { title: t("删除这次对话？").into(), message: tf!("「{title}」连同其中的图片会从这个文献库的历史对话里删除，不能恢复。文献库里的文献不受影响。{warning}", "“{title}” and its images will be permanently removed from this library's chat history. Papers are kept.{warning}"), action: t("删除").into(), cmd: Cmd::RemoveChat(paper, id) });
            }
            Cmd::RemoveChat(paper, id) => {
                if let Some(lib) = self.lib.as_mut() {
                    let result = lib.store.remove_conversation(&id).and_then(|()| lib.list_history(paper));
                    // The conversation in hand, once removed, would only be saved again with the next answer;
                    // the papers it was kept to go with it, as for any new conversation.
                    // Removed from 全部对话, it may be in the other place's hands.
                    for place in [Talk::Library, Talk::Paper, Talk::Write] {
                        if lib.chat(place).id != id { continue }
                        if place == Talk::Library { lib.scope_reset(cx) }
                        let chat = lib.chat_mut(place);
                        let listed = chat.history.take();
                        chat.reset(cx);
                        chat.history = listed;
                    }
                    // One answering off screen goes with it: what still arrives for it finds nothing.
                    lib.aside.retain(|chat| chat.id != id);
                    let chat = lib.chat_mut(paper);
                    match result {
                        Ok(list) => chat.history = Some(list),
                        Err(err) => self.fail(format!("{err:#}"), cx),
                    }
                }
                self.run(Cmd::CloseModal, window, cx);
            }
            Cmd::PickMention(place, at) => {
                self.pick_mention(place, at, cx);
                match place {
                    crate::mention::Place::Chat(talk) => if let Some(lib) = self.lib.as_ref() { lib.chat(talk).input.read(cx).focus(window) },
                    crate::mention::Place::Home => self.home.input.read(cx).focus(window),
                }
            }
            Cmd::MentionPaper(talk, id) => self.mention_paper(talk, &id, window, cx),
            Cmd::MentionTag(tag) => self.mention_tag(tag),
            Cmd::MentionSection(manuscript, section) => self.mention_section(manuscript, section, window, cx),
            Cmd::MentionFigure(value) => self.mention_figure(value, window, cx),
            Cmd::PickSlash(paper, at) => {
                self.pick_slash(paper, at, cx);
                if let Some(lib) = self.lib.as_ref() { lib.chat(paper).input.read(cx).focus(window) }
            }
            Cmd::AttachImages(paper) => {
                // The assistant of the library also takes a Word file, to restyle its citations, and a PowerPoint file, as the
                // reference slides are made after; in a conversation not yet made a library a Word file is a manuscript to
                // review and a PDF a paper, taken in as a drop takes them.
                let documents = paper == Talk::Library;
                let draft = documents && self.in_draft();
                let extensions: Vec<SharedString> = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"].into_iter().chain(if documents { ["docx", "pptx", "potx"].as_slice() } else { [].as_slice() }.iter().copied()).chain(draft.then_some("pdf")).map(SharedString::from).collect();
                let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some(if draft { t("添加图片、Word、PowerPoint 文件或 PDF") } else if documents { t("添加图片、Word 或 PowerPoint 文件") } else { t("添加图片") }.into()), extensions: Some(extensions) });
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = picked.await {
                        this.update(cx, |this, cx| {
                            let (files, pictures): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|path| documents && (crate::comments::is_word(path) || crate::comments::is_deck(path) || (draft && crate::draft::starts_one(path))));
                            if draft && !files.is_empty() { this.import_paths(files, cx) } else if !files.is_empty() { this.attach_documents(paper, files, cx) }
                            if !pictures.is_empty() { this.attach_images(paper, pictures.into_iter().map(Picture::File).collect(), cx) }
                        })
                        .ok();
                    }
                })
                .detach();
            }
            Cmd::RemoveDocument(paper, at) => {
                if let Some(lib) = self.lib.as_mut() {
                    let chat = lib.chat_mut(paper);
                    if at < chat.documents.len() { chat.documents.remove(at); }
                }
            }
            Cmd::RemoveAttachment(paper, at) => {
                if let Some(lib) = self.lib.as_mut() {
                    let chat = lib.chat_mut(paper);
                    if at < chat.attachments.len() { chat.attachments.remove(at); }
                }
            }
            Cmd::RemoveQuote(paper, at) => {
                if let Some(lib) = self.lib.as_mut() {
                    let chat = lib.chat_mut(paper);
                    if at < chat.quotes.len() { chat.quotes.remove(at); }
                }
            }
            Cmd::AnswerTools(paper, consent) => {
                let Some(pending) = self.lib.as_mut().and_then(|lib| {
                    // Answered: what the conversation that writes had put in a section's place for the answer goes.
                    if paper == Talk::Write { lib.writing.agent.proposal = None }
                    let chat = lib.chat_mut(paper);
                    if consent == Consent::Always { chat.allow_changes = true }
                    chat.pending.take()
                }) else {
                    return;
                };
                if consent == Consent::Deny { self.execute_tools(paper, pending.calls, pending.first, false, cx) } else { self.place_outputs(paper, pending.calls, pending.first, 0, cx) }
            }
            Cmd::ToggleTools(paper, at) => {
                if let Some(lib) = self.lib.as_mut() {
                    let chat = lib.chat_mut(paper);
                    if !chat.tools_open.remove(&at) { chat.tools_open.insert(at); }
                }
            }
            Cmd::ToggleCompacted(talk) => {
                if let Some(lib) = self.lib.as_mut() {
                    let chat = lib.chat_mut(talk);
                    chat.compacted_open = !chat.compacted_open;
                    // A summary under all that was said unfolds below the fold.
                    if chat.compacted_open && chat.compacted.as_ref().is_some_and(|compacted| compacted.at >= chat.messages.len()) { chat.scroll.scroll_to_bottom() }
                }
            }
            Cmd::AnswerToManuscript(question, answer) => self.answer_to_manuscript(question, answer, window, cx),
            Cmd::InsertInto(section, text) => self.insert_into_section(section, text, cx),
            Cmd::StartReview => self.start_review(cx),
            Cmd::ReviewToManuscript => self.review_to_manuscript(window, cx),
            Cmd::ToggleReview => {
                if let Some(lib) = self.lib.as_mut() { lib.library_chat.review_open = !lib.library_chat.review_open && lib.library_chat.review.is_some() }
            }
            Cmd::Review(what, text) => self.review_do(what, text, cx),
            Cmd::EditReviewText(what) => self.edit_review_text(what, window, cx),
            Cmd::AskRemoveReviewSection(name) => {
                self.modal_error.clear();
                self.modal = Some(Modal::Confirm { title: tf!("删除综述记录中的「{name}」？", "Delete “{name}” from the review record?"), message: t("这一节会从综述记录中移除，它的引用带来的、你还没有判断过的证据也一起移除。已创建的稿件不受影响。").into(), action: t("删除").into(), cmd: Cmd::RemoveReviewSection(name) });
            }
            Cmd::RemoveReviewSection(name) => {
                self.review_apply(wisp_core::review::ReviewAction::RemoveSection { name }, t("这一节已从综述记录中移除"), cx);
                self.run(Cmd::CloseModal, window, cx);
            }
            Cmd::EditReviewEvidence(id, claim) => self.edit_review_evidence(id, claim, window, cx),
            Cmd::PickEvidencePaper(id) => {
                if let Some(Modal::ReviewEvidence(form)) = self.modal.as_mut() { form.paper_id = id }
            }
            Cmd::PickEvidenceSource(source) => {
                if let Some(Modal::ReviewEvidence(form)) = self.modal.as_mut() { form.source = source }
            }
            Cmd::PickEvidenceSection(section) => {
                if let Some(Modal::ReviewEvidence(form)) = self.modal.as_mut() { form.section = section }
            }
            Cmd::JudgeReviewEvidence(id, judgment) => {
                use wisp_core::review::{EvidenceJudgment, ReviewAction};
                // The judgment a row already has, clicked again, is taken back.
                let has = self.lib.as_ref().and_then(|lib| lib.library_chat.review.as_ref()?.evidence_rows.iter().find(|row| row.id == id).map(|row| row.judgment == judgment && row.source_current));
                let judgment = if has == Some(true) { EvidenceJudgment::Unreviewed } else { judgment };
                self.review_apply(ReviewAction::JudgeEvidence { id, judgment }, "", cx);
            }
            Cmd::AskRemoveReviewEvidence(id) => {
                self.modal_error.clear();
                self.modal = Some(Modal::Confirm { title: t("删除这条证据记录？").into(), message: t("这条论断与来源的关联会从综述记录中移除，文献和正文不受影响。").into(), action: t("删除").into(), cmd: Cmd::RemoveReviewEvidence(id) });
            }
            Cmd::RemoveReviewEvidence(id) => {
                self.review_apply(wisp_core::review::ReviewAction::RemoveEvidence { id }, t("证据记录已删除"), cx);
                self.run(Cmd::CloseModal, window, cx);
            }
            Cmd::CopyText(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.say(t("已复制"), cx);
            }
            Cmd::CopyDiagram(source) => {
                // Drawn anew for a white page and three times as fine: not on the thread that draws the window.
                cx.spawn(async move |this, cx| {
                    let png = cx.background_executor().spawn(async move { crate::diagram::png(&source) }).await;
                    this.update(cx, |this, cx| match png {
                        Ok(bytes) => {
                            cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, bytes)));
                            this.say(t("图片已复制"), cx);
                        }
                        Err(err) => this.fail(tf!("这张图画不出来：{err}", "This diagram cannot be drawn: {err}"), cx),
                    })
                    .ok();
                })
                .detach();
            }
            Cmd::Retract(talk, at) => self.retract(talk, at, window, cx),
            Cmd::Regenerate(talk, at) => self.regenerate(talk, at, cx),
            Cmd::Fork(talk, at) => self.fork_chat(talk, at, cx),
            Cmd::Unretract(talk) => self.unretract(talk, cx),
            Cmd::CopySelected => {
                if let Some(text) = self.picked.as_ref().map(|picked| picked.chosen().to_string()).filter(|text| !text.is_empty()) {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    self.say(t("已复制"), cx);
                }
            }
            Cmd::RenameTag(tag) => {
                self.modal_subscriptions.clear();
                self.modal_error.clear();
                let name = self.modal_input("标签的新名字", &tag, false, window, cx);
                name.read(cx).focus(window);
                self.modal = Some(Modal::Tag { tag, name });
            }
            Cmd::AskRemoveTag(tag) => {
                let count = self.lib.as_ref().map_or(0, |lib| lib.papers.iter().filter(|p| p.tags.contains(&tag)).count());
                self.modal_error.clear();
                self.modal = Some(Modal::Confirm { title: tf!("去掉标签「{tag}」？", "Remove tag “{tag}”?"), message: tf!("它会从 {count} 篇文献上去掉。文献本身不动。", "It will be removed from {count} papers. The papers are kept."), action: t("去掉标签").into(), cmd: Cmd::RemoveTag(tag) });
            }
            Cmd::RemoveTag(tag) => {
                self.modal = None;
                if self.retag(&tag, None, cx) { self.say(t("标签已去掉。"), cx) }
            }
            Cmd::AnswerToNote(question, answer) => {
                let Some(lib) = self.lib.as_ref() else { return };
                let Some(paper_id) = lib.selected.clone() else { return };
                let note = ai::note_from_answer(&question, &answer, lib.paper_chat.sources(), &paper_id, &time::date_text(&time::now_iso()), crate::i18n::is_english());
                self.modal_subscriptions.clear();
                self.modal_error.clear();
                let text = cx.new(|cx| TextInput::new("要记下的内容", cx).multiline(8).with_text(&note));
                text.read(cx).focus(window);
                self.modal = Some(Modal::Note { paper_id, text });
            }
            Cmd::PickText(paper, at, pointer, place) => {
                use crate::markdown::Pointer;
                let Some(lib) = self.lib.as_mut() else { return };
                // The keyboard goes to the window, so that copying finds the selection and not a text box.
                if matches!(pointer, Pointer::Down | Pointer::Word) {
                    window.focus(&self.focus);
                    self.picked = None;
                }
                // One selection at a time: starting one in a conversation ends the other's.
                if matches!(pointer, Pointer::Down | Pointer::Word) {
                    for other in [Talk::Library, Talk::Paper, Talk::Write].into_iter().filter(|other| *other != paper) { lib.chat_mut(other).picked = None }
                }
                let chat = lib.chat_mut(paper);
                match (pointer, chat.picked.as_mut()) {
                    (Pointer::Down, _) => (chat.picked, chat.spot) = (Some((at, place, place, true)), None),
                    // By the rule of every text read (`select::word_at`). Done at once and not held: the hand's slip
                    // between the press and the release would else be a drag that cuts the word short.
                    (Pointer::Word, _) => {
                        let Some(ChatItem::Answer(answer)) = chat.messages.get(at) else { return };
                        let word = crate::markdown::texts(answer).get(place.0).map(|piece| crate::select::word_at(piece, place.1)).filter(|word| !word.is_empty() && crate::markdown::text_between(answer, (place.0, word.start), (place.0, word.end)).trim() != "");
                        (chat.picked, chat.spot) = (word.map(|word| (at, (place.0, word.start), (place.0, word.end), false)), None);
                    }
                    (Pointer::Drag, Some((answer, _, to, true))) if *answer == at => *to = place,
                    (Pointer::Up, Some((_, from, to, held))) if *held => {
                        *held = false;
                        // A click selects nothing.
                        if from == to { chat.picked = None }
                    }
                    _ => {}
                }
            }
            Cmd::CopyPicked(paper) => {
                if let Some(text) = self.lib.as_ref().and_then(|lib| lib.chat(paper).picked_text()) {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    self.say(t("已复制"), cx);
                }
            }
            Cmd::QuotePicked(paper) => {
                let Some(lib) = self.lib.as_mut() else { return };
                let chat = lib.chat_mut(paper);
                let Some(text) = chat.picked_text() else { return };
                chat.picked = None;
                // What the assistant said, as a quotation in the box: its words, marked as such, before the reader's own.
                let quoted: String = text.lines().map(|line| format!("> {line}\n")).collect();
                // Put after the draft as one step of its Undo, the caret at the end for the question.
                chat.input.update(cx, |input, cx| {
                    let (kept, end) = (input.text().trim_end().len(), input.text().len());
                    input.replace_range(kept..end, &if kept == 0 { format!("{quoted}\n") } else { format!("\n\n{quoted}\n") }, cx);
                    let end = input.text().len();
                    input.select_range(end..end, cx);
                });
                chat.input.read(cx).focus(window);
            }
            Cmd::ToggleMaterials => {
                if let Some(lib) = self.lib.as_mut() {
                    lib.library_chat.materials_open = !lib.library_chat.materials_open;
                    // The panel is a place to search: its box has the keyboard while it is open, the question's after.
                    if lib.library_chat.materials_open { lib.scope_find.read(cx).focus(window) } else { lib.library_chat.input.read(cx).focus(window) }
                }
            }
            Cmd::Scope(what) => self.scope(what, cx),
            Cmd::ToggleSources(paper, at) => {
                if let Some(lib) = self.lib.as_mut() {
                    let chat = lib.chat_mut(paper);
                    if !chat.sources_open.remove(&at) { chat.sources_open.insert(at); }
                }
            }
            Cmd::GotoCited(paper, label, file, page, quote) => {
                let Some(lib) = self.lib.as_ref() else { return };
                let chat = lib.chat(paper);
                let Some(source) = chat.sources().iter().find(|source| source.label == label).cloned() else { return self.fail(tf!("这次对话里没有 {label} 这篇文献。", "Paper {label} is not in this conversation."), cx) };
                // Read in another library: that library is opened at the paper and the page cited. The conversation is kept
                // as it is and found again from the home page (最近对话); one still answering goes on off screen.
                if !source.library.is_empty() {
                    if !self.libraries.iter().any(|info| info.id == source.library && !info.draft) { return self.fail(tf!("文献库「{}」已经不在了。", "The library “{}” is no longer there.", source.library_name), cx) }
                    self.open_library(&source.library, window, cx);
                    if self.lib.as_ref().is_none_or(|lib| lib.info.id != source.library) { return }
                    if self.lib.as_ref().is_some_and(|lib| lib.paper(&source.paper_id).is_none()) { return self.fail(tf!("这篇文献已经不在文献库「{}」里了。", "This paper is no longer in the library “{}”.", source.library_name), cx) }
                    return self.run(Cmd::GotoFile(source.paper_id, None, page, quote), window, cx);
                }
                let id = source.paper_id;
                // Where the sentence was found, when it is not where the answer says.
                let page = match chat.checks.get(&(label, file, page, quote.clone())) {
                    Some(ai::Quoted::Elsewhere(at) | ai::Quoted::Begins(at, _)) => Some(*at),
                    _ => page,
                };
                let file = file.and_then(|n| lib.attachments(&id).get(n - 1).map(|f| f.id.clone()));
                self.run(Cmd::GotoFile(id, file, page, quote), window, cx);
            }
            Cmd::GotoSource(id, page, quote) => self.run(Cmd::GotoFile(id, None, page, quote), window, cx),
            Cmd::GotoFile(id, file, page, quote) => {
                // The file cited is the one shown: the main text, unless an attachment is named.
                if page.is_some() { self.show_file(&id, file, cx) }
                // Cited from the paper's own conversation: the PDF opens to its right, where the window has room.
                // (Among several papers side by side, the one cited is worked in where it is.)
                let mut beside = self.lib.as_ref().is_some_and(|lib| lib.tab == Tab::Ask && lib.talk_papers().contains(&id) && lib.pdf_file(&id).is_some()) && page.is_some();
                self.select(Some(id), window, cx);
                if beside {
                    self.run(Cmd::BesidePdf(true), window, cx);
                    // A window too small for the two side by side shows the PDF in the conversation's place.
                    beside = self.detail_width(window) >= crate::detail::SPLIT_MIN;
                }
                if let Some(page) = page && let Some(lib) = self.lib.as_mut() {
                    if !beside { lib.tab = Tab::Pdf }
                    self.ensure_pdf(cx);
                    if let Some(pdf) = self.lib.as_ref().and_then(|l| l.pdf.clone()) {
                        pdf.update(cx, |pdf, cx| pdf.find(page, &quote, cx));
                    }
                }
            }

            Cmd::NewSearch => self.search_dialog(None, window, cx),
            Cmd::EditSearch(id) => self.search_dialog(Some(id), window, cx),
            Cmd::AskRemoveSearch(id) => {
                let Some(search) = self.lib.as_ref().and_then(|l| l.searches.iter().find(|s| s.id == id)) else { return };
                self.modal_error.clear();
                self.modal = Some(Modal::Confirm { title: tf!("删除检索式「{}」？", "Delete search “{}”?", search.name), message: t("它导入过的文献和运行记录都会保留，只是不再运行这个检索式。").into(), action: t("删除").into(), cmd: Cmd::RemoveSearch(id) });
            }
            Cmd::RemoveSearch(id) => {
                self.change(cx, |lib| lib.store.remove_search(&id));
                self.run(Cmd::CloseModal, window, cx);
            }
            Cmd::TrySearch(id) => {
                let Some(lib) = self.lib.as_mut() else { return };
                let Some(search) = lib.searches.iter().find(|s| s.id == id) else { return };
                let args = serde_json::json!({"source": search.kind, "query": search.query, "limit": 20});
                lib.trial = Some((id.clone(), None));
                self.library_tool("search_online", args, cx, move |this, result, _| {
                    // Only when this trial is still the one on show.
                    if let Some(lib) = this.lib.as_mut() && let Some((shown, found)) = lib.trial.as_mut() && *shown == id { *found = Some(result) }
                });
            }
            Cmd::CloseTrial => {
                if let Some(lib) = self.lib.as_mut() { lib.trial = None }
            }
            Cmd::RunSearch(id) => {
                let Some(lib) = self.lib.as_mut() else { return };
                if !lib.running.insert(id.clone()) { return }
                self.library_tool("run_search", serde_json::json!({"id": id}), cx, move |this, result, cx| {
                    if let Some(lib) = this.lib.as_mut() { lib.running.remove(&id); }
                    // The run wrote through its own connection, failed runs included.
                    this.change(cx, |_| Ok(()));
                    match result {
                        Ok(run) if run["stopped"].is_string() => this.fail(tf!("{}：新增 {} 篇。{}", "{}: {} added. {}", run["search"].as_str().unwrap_or(t("检索")), run["added"], run["stopped"].as_str().unwrap_or_default()), cx),
                        Ok(run) => this.say(tf!("{}：新增 {} 篇，跳过已有的 {} 篇", "{}: {} added, {} existing skipped", run["search"].as_str().unwrap_or(t("检索")), run["added"], run["skipped"]), cx),
                        Err(err) => this.fail(err, cx),
                    }
                });
            }
            Cmd::RunAllSearches => {
                let ids: Vec<String> = self.lib.as_ref().map(|l| l.searches.iter().filter(|s| s.enabled).map(|s| s.id.clone()).collect()).unwrap_or_default();
                for id in ids {
                    self.run(Cmd::RunSearch(id), window, cx);
                }
            }
            Cmd::AskUndoImport(id) => {
                let Some(batch) = self.lib.as_ref().and_then(|l| l.batches.iter().find(|b| b.id == id)) else { return };
                self.modal_error.clear();
                self.modal = Some(Modal::Confirm {
                    title: t("撤销这次导入？").into(),
                    message: tf!("{}\n\n这次导入新增的 {} 篇文献里，还没有处理过的会移出文献库，记录另存在文献库文件夹的 removed 里，可以再导入；已经做过收录判断、读过、收藏、写过笔记、加过标签或附件、提过问、被稿件引用的会保留。", "{}\n\nOf {} papers added by this import, untouched papers leave the library; their records are set aside in the library folder under removed and can be imported again. Papers already screened, read, starred, annotated, tagged, given attachments, asked about or cited in a manuscript are kept.", crate::i18n::import_label(&batch.label), batch.added),
                    action: t("撤销导入").into(),
                    cmd: Cmd::UndoImport(id),
                });
            }
            Cmd::UndoImport(id) => {
                let mut undone = None;
                if self.change(cx, |lib| lib.store.undo_batch(&id).map(|done| undone = Some(done)))
                    && let Some(done) = undone
                {
                    let aside = if done.set_aside.is_empty() { String::new() } else { tf!("，记录另存在 {}", "; records set aside in {}", done.set_aside) };
                    self.say(if done.kept.is_empty() { tf!("已撤销：移出了 {} 篇{aside}", "Undone: {} papers removed{aside}", done.removed) } else { tf!("已撤销：移出了 {} 篇{aside}；保留了处理过的 {} 篇", "Undone: {} removed{aside}; {} edited papers kept", done.removed, done.kept.len()) }, cx);
                }
                self.run(Cmd::CloseModal, window, cx);
            }
            Cmd::PickSource(value) => {
                if let Some(Modal::Search { kind, .. }) = self.modal.as_mut() { *kind = value }
            }

            Cmd::SetAutoSync(hours) => {
                let Some(lib) = self.lib.as_mut() else { return };
                lib.info.auto_sync_hours = hours;
                if let Err(err) = libraries::save(&lib.info) { self.fail(format!("{err:#}"), cx) }
                self.libraries = libraries::list(&self.root);
            }
            Cmd::ToggleSearch(id) => {
                self.change(cx, |lib| {
                    let Some(mut search) = lib.searches.iter().find(|s| s.id == id).cloned() else { return Ok(()) };
                    search.enabled = !search.enabled;
                    lib.store.save_search(&mut search)
                });
            }
            Cmd::ScreenStart => self.screen_start("pending", None, cx),
            Cmd::ScreenRecheck(included) => self.screen_start(if included { "included" } else { "decided" }, None, cx),
            Cmd::AskTrashUnincluded => {
                let Some(lib) = self.lib.as_ref() else { return };
                let ids = lib.unincluded();
                if ids.is_empty() { return }
                let counts = wisp_core::screening::tally(ids.iter().filter_map(|id| lib.paper(id)).map(|p| p.screening.as_str()));
                let each: Vec<String> = SCREENINGS.iter().zip(counts).filter(|(_, n)| *n > 0).map(|((key, _), n)| format!("{} {n}", crate::i18n::label(&SCREENINGS, key))).collect();
                self.modal_error.clear();
                self.modal = Some(Modal::Confirm { title: tf!("把未纳入的 {} 篇移到回收站？", "Move {} papers not included to trash?", ids.len()), message: tf!("其中{}。它们的笔记、词条和 PDF 都留着，可以在回收站里恢复。", "Of them: {}. Their notes, fields and PDFs are kept, and they can be restored from the trash.", each.join(" · ")), action: t("移到回收站").into(), cmd: Cmd::TrashPapers(ids) });
            }
            Cmd::ScreenStop => {
                if let Some(lib) = self.lib.as_mut() {
                    // Answers still on their way belong to a run that is over.
                    lib.screen.run += 1;
                    lib.screen.todo.clear();
                    lib.screen.reading.clear();
                }
            }
            Cmd::ScreenFilter(filter) => {
                if let Some(lib) = self.lib.as_mut() { lib.screen.filter = filter }
            }
            Cmd::ScreenDecide(..) | Cmd::ScreenMode(_) | Cmd::ScreenGo(_) | Cmd::ScreenUndo | Cmd::ScreenAskOne(_) | Cmd::ScreenBeside => self.run_screen(cmd, window, cx),
            Cmd::ConfirmTick(_) | Cmd::ConfirmChoose(..) | Cmd::ConfirmPart(_) | Cmd::ConfirmTickAll(_) | Cmd::ConfirmSort | Cmd::ConfirmCursor(_) | Cmd::ConfirmClose | Cmd::ConfirmApply | Cmd::ConfirmUndo => self.run_confirm(cmd, window, cx),

            Cmd::OpenGuide(choosing) => {
                if let Some(lib) = self.lib.as_mut() {
                    lib.page = Page::Guide;
                    lib.guide.choosing = choosing;
                    if !choosing { lib.guide.keywords.read(cx).focus(window) }
                }
            }
            Cmd::GuideSearch => {
                if let Some(lib) = self.lib.as_mut() {
                    lib.guide.choosing = false;
                    lib.guide.keywords.read(cx).focus(window);
                }
            }
            Cmd::GuideToggleSource(kind) => {
                if let Some(lib) = self.lib.as_mut() {
                    let kinds = &mut lib.guide.kinds;
                    // At least one database stays chosen.
                    if let Some(at) = kinds.iter().position(|k| *k == kind) { if kinds.len() > 1 { kinds.remove(at); } } else { kinds.push(kind) }
                }
            }
            Cmd::GuideGenerate => self.guide_generate(cx),
            Cmd::GuideToggleIdea(index) => {
                if let Some(idea) = self.lib.as_mut().and_then(|l| l.guide.ideas.get_mut(index)) { idea.chosen = !idea.chosen }
            }
            Cmd::GuideImport => {
                let Some(lib) = self.lib.as_mut() else { return };
                let mut saved = vec![];
                let mut failure = None;
                for entry in lib.guide.ideas.iter().filter(|entry| entry.chosen) {
                    // Named after what the search aims at, as far as a name may go.
                    let mut search = SavedSearch { kind: entry.idea.source.clone(), name: entry.idea.why.chars().take(40).collect(), query: entry.idea.query.clone(), max_results: wisp_core::sources::broad_limit(&entry.idea.source), enabled: true, ..Default::default() };
                    match lib.store.save_search(&mut search) {
                        Ok(()) => saved.push(search.id),
                        Err(err) => failure = Some(format!("{err:#}")),
                    }
                }
                if saved.is_empty() && failure.is_none() { return }
                // What was saved is on the searches page from here on.
                lib.guide.ideas.retain(|entry| !entry.chosen);
                lib.page = Page::Sources;
                self.change(cx, |_| Ok(()));
                if let Some(err) = failure { self.fail(err, cx) }
                for id in saved {
                    self.run(Cmd::RunSearch(id), window, cx);
                }
            }

            Cmd::CloseModal => {
                // A file not imported: the next one waiting is asked about.
                if matches!(self.modal, Some(Modal::Headings(_))) { self.headings_done(false, cx) }
                if matches!(self.modal, Some(Modal::Headings(_))) { return cx.notify() }
                // A figure looked at from 资料库 gives the place back to it.
                let back = matches!(&self.modal, Some(Modal::Figure(form)) if form.back) && self.lib.as_ref().is_some_and(|lib| lib.page == Page::Write);
                self.modal = back.then_some(Modal::WriteFigures);
                self.modal_subscriptions.clear();
                self.figure_drag = None;
                // The piece a figure dialog was for is no longer shown on its page.
                self.sync_figure_frames(cx);
                window.focus(&self.focus);
            }
            Cmd::SubmitModal => self.submit_modal(window, cx),
            Cmd::CompletePaper(id) => self.complete_dialog(id, window, cx),
            Cmd::CompleteById(id) => self.complete_by_id_dialog(id, window, cx),
            Cmd::CompleteChoose(at) => {
                self.modal_error.clear();
                let Some(Modal::Complete(form)) = self.modal.as_mut().filter(|modal| !matches!(modal, Modal::Complete(form) if form.by_id && at.is_none())) else { return };
                form.chosen = at.filter(|at| form.found.as_ref().is_some_and(|found| *at < found.candidates.len()));
                form.fields = match (form.chosen, self.lib.as_ref().and_then(|lib| lib.paper(&form.paper_id))) {
                    (Some(at), Some(paper)) => match &form.found.as_ref().unwrap().candidates[at] {
                        candidate if candidate.similarity >= complete::ALIKE_ENOUGH => complete::suggested(paper, &candidate.paper, form.placeholder),
                        _ => vec![],
                    },
                    _ => vec![],
                };
            }
            Cmd::CompleteField(key) => {
                let Some(Modal::Complete(form)) = self.modal.as_mut() else { return };
                match form.fields.iter().position(|taken| *taken == key) {
                    Some(at) => { form.fields.remove(at); }
                    None => form.fields.push(key),
                }
            }
            Cmd::PickColor(value) => {
                if let Some(Modal::Library { color, .. }) = self.modal.as_mut() { *color = value }
            }
            Cmd::PickKind(value) => {
                if let Some(Modal::Fields { kind, .. }) = self.modal.as_mut() { *kind = value }
            }
            Cmd::ToggleInTable => {
                if let Some(Modal::Fields { in_table, .. }) = self.modal.as_mut() { *in_table = !*in_table }
            }
            Cmd::PickOption(field, option) => {
                if let Some(Modal::Paper(form)) = self.modal.as_mut() {
                    if form.selects.get(&field) == Some(&option) { form.selects.remove(&field); } else { form.selects.insert(field, option); }
                }
            }
        }
        // A conversation not yet made a library has one page, its conversation: whatever asked for another (a view,
        // a filter, a search, the import of a file) leaves it where it is (`draft.rs`).
        if let Some(lib) = self.lib.as_mut().filter(|lib| lib.info.draft && lib.page != Page::Ask) { lib.page = Page::Ask }
    }

    pub fn dismiss(&mut self, action: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
        } else if self.bell.open {
            self.bell.open = false;
            cx.notify();
        } else if self.palette.is_some() {
            self.close_palette(window, cx);
        } else if self.modal.as_ref().is_some_and(|modal| matches!(modal, Modal::Figure(form) if form.beside_page())) && !self.figure_dialog_focused(window, cx) {
            // The figure dialog standing beside the page: Esc on the page (a selection copied from) does not put away what was written.
        } else if let Some(Modal::Excerpt(form)) = self.modal.as_mut().filter(|modal| matches!(modal, Modal::Excerpt(form) if form.picking)) {
            // The list of figures to link, open inside the dialog: folded up before the dialog goes.
            form.picking = false;
            window.focus(&self.focus);
        } else if self.modal.is_some() {
            self.run(Cmd::CloseModal, window, cx);
        } else if self.dismiss_font_picker(window, cx) || self.settings_up(cx) {
        } else if self.settings_view.is_some() {
            self.close_settings(window, cx);
        } else if self.tutorials.is_some() && self.lib.is_none() {
            self.run(Cmd::Tutorial(crate::tutorial::Do::Close), window, cx);
        } else if let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()).filter(|pdf| pdf.read(cx).overview_open()) {
            // 页面总览 was opened from the bar, so the viewer need not have the keyboard: it is put away before the paper is.
            pdf.update(cx, |pdf, cx| pdf.toggle_overview(cx));
        } else if self.lib.is_none() && self.home.open {
            // The assistant's page goes back to the home page; what is in its box stays.
            self.run(Cmd::GoHome, window, cx);
        } else if self.lib.as_ref().is_some_and(|l| l.page == Page::Write && l.writing.seek.open) {
            // The search over the manuscript, from its own field or the manuscript: before a paper beside it is closed.
            self.run(Cmd::Seek(crate::write_seek::Do::Close), window, cx);
        } else if let Some(talk) = self.lib.as_ref().and_then(|lib| [Talk::Library, Talk::Paper, Talk::Write].into_iter().find(|&talk| lib.chat(talk).history.is_some() && lib.chat(talk).history_find.focus_handle(cx).is_focused(window))) {
            // 历史's search: what is typed goes first, then the list.
            let typed = self.lib.as_ref().is_some_and(|lib| !lib.chat(talk).history_find.read(cx).text().is_empty());
            self.run(if typed { Cmd::ClearHistoryFind(talk) } else { Cmd::ToggleHistory(talk) }, window, cx);
        } else if self.picked.take().is_some() || self.lib.as_mut().is_some_and(Library::unpick) {
        } else if self.lib.as_ref().is_some_and(|l| l.write_figures_shown()) {
            // The panel floating over the conversation is put away first; what it searched for stays.
            if let Some(lib) = self.lib.as_mut() {
                lib.library_chat.materials_open = false;
                lib.library_chat.input.read(cx).focus(window);
            }
        } else if self.lib.as_ref().is_some_and(|l| l.page == Page::Extract && l.selected.is_none() && l.extract_detail_shown()) {
            // 提取's row in full under the list, once no paper stands beside it: the list gets its room back.
            self.run(Cmd::Extract(crate::extract::Do::Close), window, cx);
        } else if self.lib.as_ref().is_some_and(|l| l.page == Page::Trash && !l.trash_chosen.is_empty()) {
            if let Some(lib) = self.lib.as_mut() { (lib.trash_chosen, lib.trash_anchor) = (vec![], None) }
        } else if self.lib.as_ref().is_some_and(|l| !l.marked.is_empty()) {
            if let Some(lib) = self.lib.as_mut() { lib.marked.clear() }
        } else if self.lib.as_ref().is_some_and(|l| l.reading && l.selected.is_some()) {
            // Out of reading mode first: the paper stays open where it was read.
            if let Some(lib) = self.lib.as_mut() { lib.reading = false }
        } else if self.asking(window) {
            // Out of asking first, back to the library with the paper still open.
            self.run(Cmd::LeaveAsk, window, cx);
        } else if self.lib.as_ref().is_some_and(|l| l.selected.is_some()) {
            self.select(None, window, cx);
        } else if self.lib.as_ref().is_some_and(|l| l.page == Page::Papers && l.finding.open) {
            // The places found in the whole library give the list back.
            self.run(Cmd::LibraryFind(crate::library_find::Do::Close), window, cx);
        } else if self.lib.as_ref().is_some_and(|l| l.page == Page::Screen && l.screen.mode != ScreenMode::Batch) {
            // Out of a round or the confirmation, back to the queue.
            self.run(Cmd::ScreenMode(ScreenMode::Batch), window, cx);
        } else if action.leave && self.lib.is_some() {
            self.leave_by_key(window, cx);
        }
        cx.notify();
    }

    /// Esc with nothing left to put away in the library: where the corner's ← goes (design.md rule 53), out of a
    /// manuscript to the library, else back to 所有文献库.
    /// Not in one press from where words are typed: a field with the keyboard gives it back to the page first (what is
    /// in it stays), words in the search box go next (the list in full), and a conversation's box holding words not
    /// yet sent keeps the library open, since leaving would lose them.
    fn leave_by_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.context_stack().iter().any(|context| context.contains("TextInput")) { return window.focus(&self.focus) }
        // The corner's arrow is the way out from here: out of a manuscript to the page 写作 was opened from.
        if self.outline_shown() && !self.in_draft() { return self.run(Cmd::LeaveWrite, window, cx) }
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.page == Page::Papers && !lib.search.read(cx).text().is_empty() {
            // `set_text` says nothing to the box's subscription: the list is given back here.
            lib.search.update(cx, |input, cx| input.set_text("", cx));
            lib.filters.search.clear();
            return lib.refilter();
        }
        if [Talk::Library, Talk::Paper, Talk::Write].into_iter().any(|talk| !lib.chat(talk).input.read(cx).text().trim().is_empty()) {
            return self.say(t("助手的输入框里还有没发出的字：发出或删掉之后，Esc 才回到所有文献库"), cx);
        }
        self.run(Cmd::GoHome, window, cx);
    }

    /// A keyboard shortcut (shortcuts.rs) was pressed.
    pub fn shortcut(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        // A dialog keeps the keyboard to itself.
        if self.modal.is_some() { return }
        if id == "palette" { return self.toggle_palette(window, cx) }
        // So does the palette: its own key puts it away, the others wait.
        if self.palette.is_some() { return }
        let in_settings = self.settings_view.is_some();
        // ⌘+ / ⌘- / ⌘0 are the PDF's size while one is on show. Anywhere else, the home page and the settings
        // included, they are the size of the interface's text (设置 › 外观's 字号), as in a browser.
        let pdf_shown = !in_settings && self.lib.as_ref().is_some_and(|lib| lib.selected.is_some() && lib.pdf.is_some() && (lib.tab == Tab::Pdf || lib.split));
        let cmd = match id {
            "zoom-in" | "zoom-out" | "zoom-fit" if !pdf_shown => return self.step_text_size(match id { "zoom-in" => Some(1.), "zoom-out" => Some(-1.), _ => None }, window, cx),
            "settings" if !in_settings => Cmd::OpenSettings(SettingsPage::General),
            "home" => Cmd::GoHome,
            "new-library" if !in_settings && self.lib.is_none() => Cmd::NewLibrary,
            // On the home side, "ask the assistant" is its page.
            "ask" if !in_settings && self.lib.is_none() => Cmd::Draft(crate::draft::Do::Begin),
            // The rest act on the open library, when it is the one in view.
            _ if in_settings || self.lib.is_none() => return,
            // A conversation not yet made a library has no list to search or add to, no sidebar and no manuscript to find for.
            "search" | "add" | "import-pdf" | "ask" | "sidebar" | "write-find" if self.in_draft() => return,
            // On the writing page the words are looked for in the manuscript, not among the papers (a PDF with the keyboard looks in itself, pdf.rs).
            "search" if self.lib.as_ref().is_some_and(|l| l.page == Page::Write && !l.reading) => return self.seek_open(window, cx),
            "search" => {
                if let Some(lib) = self.lib.as_mut() {
                    lib.page = Page::Papers;
                    lib.reading = false;
                }
                // A window too narrow for the list beside a paper asked about: the search box is with the full list.
                if self.asking(window) && self.ask_list_width(window) <= 0. { self.run(Cmd::LeaveAsk, window, cx) }
                if let Some(lib) = self.lib.as_ref() {
                    lib.search.read(cx).focus(window);
                    cx.notify();
                }
                return;
            }
            "add" => Cmd::AddById,
            "import-pdf" => Cmd::ImportPdfs,
            "ask" => Cmd::SetPage(Page::Ask),
            "detail" if self.lib.as_ref().is_some_and(|l| l.selected.is_some()) => Cmd::ToggleWide,
            "sidebar" => Cmd::ToggleSidebar,
            // On the writing page with no paper beside it, the manuscript's own reading view: 预览.
            "reading" if self.lib.as_ref().is_some_and(|l| l.page == Page::Write && !l.reading) && self.detail_width(window) <= 0. => Cmd::WriteReading(!self.lib.as_ref().is_some_and(|l| l.writing.reading)),
            "reading" if self.lib.as_ref().is_some_and(|l| l.selected.is_some()) => Cmd::ToggleReading,
            // The PDF's size, while a PDF is on show.
            "zoom-in" => Cmd::PdfZoom(ZoomTo::In),
            "zoom-out" => Cmd::PdfZoom(ZoomTo::Out),
            "zoom-fit" => Cmd::PdfZoom(ZoomTo::Set(Zoom::FitWidth)),
            // The candidates stand beside the manuscript the person writes; beside the conversation that writes there are none to show.
            "back" if self.lib.is_some() => Cmd::StepHistory(false),
            "forward" if self.lib.is_some() => Cmd::StepHistory(true),
            "write-find" if !self.settings.agent_writing && self.lib.as_ref().is_some_and(|l| l.page == Page::Write) => Cmd::WriteFind,
            // What is selected in an answer: the paper's conversation first, when both have something.
            "copy-picked" if self.picked.is_some() => Cmd::CopySelected,
            "copy-picked" => match self.lib.as_ref().and_then(|lib| [Talk::Paper, Talk::Library, Talk::Write].into_iter().find(|talk| lib.chat(*talk).picked.is_some())) {
                Some(talk) => Cmd::CopyPicked(talk),
                None => return,
            },
            _ => return,
        };
        self.run(cmd, window, cx);
    }

    /// One step of the interface's text size from the keyboard, or back to the default (`None`). The toast says
    /// the size it came to, and at either end that it goes no further.
    fn step_text_size(&mut self, step: Option<f32>, window: &mut Window, cx: &mut Context<Self>) {
        use wisp_core::settings::{DEFAULT_FONT_SIZE, font_size};
        let (was, size) = (self.settings.font_size, step.map_or(DEFAULT_FONT_SIZE, |step| font_size(self.settings.font_size + step)));
        if size != was { self.run(Cmd::SetFontSize(size), window, cx) }
        self.say(if size == was && step.is_some() { tf!("文字大小 {size} px，已到头", "Text size {size} px, as far as it goes") } else { tf!("文字大小 {size} px", "Text size {size} px") }, cx);
    }
}
