//! @ in the question boxes (`wisp_core::mention`): `/` says what to do, `@` which thing is meant. Typing `@` in a
//! conversation's box, or in the home page's, offers the things it can name over the box, as the slash commands are
//! offered, in one flat list: papers in every conversation; the library's tags in its conversation (助手); the
//! sections of a manuscript in the conversation that writes it (助手写, its own manuscript) and in 助手 (the one
//! 写作 has on show); the libraries on the shelf in the home page's assistant (its box and a conversation not yet made
//! a library). Taking a row writes the thing's token into the text (`[@Zhang2024]`, `[@tag:<name>]`,
//! `[@section:<id>]`, `[@library:<id>]`), which the box draws folded as a short name, the sent question as the same
//! name in the accent, a click on it going to the thing.
//!
//! In a paper's conversation `@` also names a figure or table of the paper being read (`[@figure:<paper>/<file>/<number>]`),
//! the figures first: drawn as the paper writes its number (`Fig. 3`), sent with its caption and page and its picture
//! cut out of the page (`question_figures`), a click on it going to the figure in the PDF. The figures are read off
//! the main thread when they are first wanted (`want_figures`), or taken from the viewer once it read them.
//!
//! What a box is offered is worked out when its text changes (`mention_typed`, against what is in memory: the papers,
//! `Library::tags`, `Writing::manuscripts`) and kept in its `Mentions`; drawing only reads it. Which places offer what
//! is `Workspace::offers`. A paper can also be dragged into the library's conversation once its first question was
//! asked (`mention_paper`): before that a drag keeps the first question to the papers dragged (`ScopeDo::Pin`).

use crate::app::{ChatImage, Cmd, Page, Tab, Talk, Workspace};
use crate::i18n::t;
use crate::input::{InputEvent, TextInput};
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use wisp_core::figref;
use wisp_core::manuscript::{Manuscript, Section};
use wisp_core::mention::{self, Candidate, FIGURE, FigureCandidate, LIBRARY, Named, OFFERED, SECTION, TAG};
use wisp_core::model::Paper;

/// The box a mention is typed in: a conversation's, or the home page's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Chat(Talk),
    Home,
}

/// What a row of the popup names. In the order the kinds are listed where several are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Library,
    Figure,
    Paper,
    Tag,
    Section,
}

impl Kind {
    /// The quiet caption over its rows when rows of another kind are on show too.
    fn caption(self) -> &'static str {
        match self {
            Kind::Library => "文献库",
            Kind::Figure => "图表",
            Kind::Paper => "文献",
            Kind::Tag => "标签",
            Kind::Section => "稿件",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Kind::Library => "library",
            Kind::Figure => "image",
            Kind::Paper => "file-text",
            Kind::Tag => "tag",
            Kind::Section => "heading",
        }
    }
}

/// What a place offers besides papers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Offers {
    pub libraries: bool,
    pub tags: bool,
    pub sections: bool,
    /// The figures and tables of the papers a paper's conversation is about.
    pub figures: bool,
}

/// A row of the popup: what it names, the token taking it writes, its title and what stands at its right.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub kind: Kind,
    pub token: String,
    pub title: String,
    pub side: String,
}

/// A box's mentions: what the `@` being typed is offered, and the folds last given to the box.
#[derive(Default)]
pub struct Mentions {
    /// The rows on offer, empty when there are none (the popup closes by itself).
    pub hits: Vec<Hit>,
    /// Where the `@` and what follows it to the caret stand: what taking a row replaces.
    pub at: Range<usize>,
    /// The row Enter or Tab takes.
    pub picked: usize,
    /// The `@` (its byte) whose list was closed with Escape: it is not offered again until another `@` is typed.
    pub closed: Option<usize>,
    /// The box's `TextInput::revision` and the folds given for it: given again when either changed (a paper renamed,
    /// a tag or a section gone, the language switched while a token sits in the box).
    pub folded: Option<(u64, Vec<(Range<usize>, String)>)>,
    /// The question was asked while a figure it names was still being read or cut out: it is sent once that is done
    /// (`figures_ready`).
    pub waiting: bool,
}

/// A paper's figures and tables as `@` offers them: those of each of its PDFs, the main text's first, each file's in
/// the order they stand in it; `None` while they are read. `files` are the PDFs they were read from: a paper given
/// another since is read again.
pub struct PaperFigures {
    pub files: Vec<String>,
    pub figures: Option<Vec<FigureOffer>>,
}

/// A figure or table of a paper where a file of it shows it.
#[derive(Clone, Debug)]
pub struct FigureOffer {
    /// The file's id, where it is, and its number among the paper's attachments (`S1`); `None` the main text.
    pub file: String,
    pub path: PathBuf,
    pub attachment: Option<usize>,
    pub place: figref::Place,
    /// What is cut out of the pages for its picture, in reading order (`figref::Place::pieces`).
    pub pieces: Vec<(usize, [f32; 4])>,
}

impl FigureOffer {
    /// Its number as the paper writes it: `Fig. 3`, `Table 2`, `图 3`.
    pub fn label(&self) -> String {
        let written = self.place.shown.label.trim().trim_end_matches(['.', ':', '|', '：', '．']).trim();
        if written.is_empty() { mention::key_written(self.place.shown.key) } else { written.to_string() }
    }

    /// The 1-based page it is cited by: its caption's.
    pub fn page(&self) -> usize {
        self.place.page + 1
    }
}

/// A figure's picture for a question, by its token's value: being cut out, cut, or why it could not be.
#[derive(Clone)]
pub enum Cut {
    Cutting,
    Ready(ChatImage),
    Failed(String),
}

/// What figures a question names come to: for each, by its token's value, what the model is told of it
/// (`mention::FigureSeen`, its picture's place among the question's pictures given), and the pictures to put with
/// the question after those it has.
pub struct QuestionFigures {
    pub seen: Vec<(String, mention::FigureSeen)>,
    pub pictures: Vec<ChatImage>,
}

impl Mentions {
    /// The popup is on show: rows on offer for the `@` the caret is still after.
    pub fn open(&self, input: &TextInput) -> bool {
        !self.hits.is_empty() && mention::typed(input.text(), input.caret()) == Some(self.at.clone())
    }
}

/// Where a mention leads when it is clicked in a sent question.
#[derive(Clone, Debug, PartialEq)]
enum Target {
    Paper(String),
    Tag(String),
    Section(String, String),
    Library(String),
    /// A figure, by its token's value.
    Figure(String),
}

impl Workspace {
    /// The box of a place, while that place exists.
    fn mention_box(&self, place: Place) -> Option<Entity<TextInput>> {
        match place {
            Place::Chat(talk) => self.lib.as_ref().map(|lib| lib.chat(talk).input.clone()),
            Place::Home => Some(self.home.input.clone()),
        }
    }

    fn mentions_mut(&mut self, place: Place) -> Option<&mut Mentions> {
        match place {
            Place::Chat(talk) => self.lib.as_mut().map(|lib| &mut lib.chat_mut(talk).mentions),
            Place::Home => Some(&mut self.home.mentions),
        }
    }

    pub(crate) fn mentions(&self, place: Place) -> Option<&Mentions> {
        match place {
            Place::Chat(talk) => self.lib.as_ref().map(|lib| &lib.chat(talk).mentions),
            Place::Home => Some(&self.home.mentions),
        }
    }

    /// What a place can name besides papers: the libraries on the shelf in the home page's assistant (its box and a
    /// conversation not yet made a library); the library's tags in its conversation; the sections of the manuscript
    /// a conversation is about (`mention_manuscript`).
    pub(crate) fn offers(&self, place: Place) -> Offers {
        let draft = self.lib.as_ref().is_some_and(|lib| lib.info.draft);
        match place {
            Place::Home => Offers { libraries: true, ..Default::default() },
            Place::Chat(talk) => Offers { libraries: talk == Talk::Library && draft, tags: talk == Talk::Library, sections: self.mention_manuscript(place).is_some(), figures: talk == Talk::Paper },
        }
    }

    /// The manuscript whose sections a place offers: the one the conversation that writes is about, the one 写作 has on
    /// show for the library's conversation (in a conversation not yet made a library, the one beside it, once it has
    /// text). None without a section that has a name.
    fn mention_manuscript(&self, place: Place) -> Option<&Manuscript> {
        let (Place::Chat(talk), Some(lib)) = (place, self.lib.as_ref()) else { return None };
        let writing = &lib.writing;
        let manuscript = match talk {
            Talk::Write => {
                let about = if writing.agent.about.is_empty() { &writing.manuscript } else { &writing.agent.about };
                writing.manuscripts.iter().find(|m| m.id == *about)
            }
            Talk::Library if lib.info.draft && !self.draft_manuscript() => None,
            Talk::Library => writing.current(),
            Talk::Paper => None,
        };
        manuscript.filter(|m| m.sections.iter().any(|s| !s.name.trim().is_empty()))
    }

    /// The libraries on the shelf, in its order.
    fn shelf(&self) -> impl Iterator<Item = &wisp_core::libraries::LibraryInfo> {
        self.libraries.iter().filter(|info| !info.draft)
    }

    /// The papers a place offers, in the order `@` alone lists them: the paper(s) asked about or selected in the list
    /// first, then those on show in their order, then the rest of the library. None in the home page's box.
    fn candidates(&self, place: Place) -> Vec<&Paper> {
        let (Place::Chat(talk), Some(lib)) = (place, self.lib.as_ref()) else { return vec![] };
        let mut ids: Vec<&str> = vec![];
        if !lib.info.draft {
            if talk == Talk::Paper { ids.extend(lib.talk_papers().iter().filter_map(|id| lib.paper(id)).map(|paper| paper.id.as_str())) }
            if lib.marked.is_empty() { ids.extend(lib.selected.as_deref().and_then(|id| lib.paper(id)).map(|paper| paper.id.as_str())) } else { ids.extend(lib.marked.iter().filter_map(|id| lib.paper(id)).map(|paper| paper.id.as_str())) }
            ids.extend(lib.shown.iter().map(|&i| lib.papers[i].id.as_str()));
        }
        ids.extend(lib.papers.iter().map(|paper| paper.id.as_str()));
        let mut seen = std::collections::HashSet::new();
        ids.into_iter().filter(|id| seen.insert(*id)).filter_map(|id| lib.paper(id)).collect()
    }

    /// The text or the caret of a box changed: what its `@` is offered now, if one is being typed.
    pub(crate) fn mention_typed(&mut self, place: Place, cx: &mut Context<Self>) {
        let Some(input) = self.mention_box(place) else { return };
        let (text, caret) = { let input = input.read(cx); (input.text().to_string(), input.caret()) };
        let typed = mention::typed(&text, caret);
        // The `@` closed with Escape stays closed; another one, or none, lets it go.
        let closed = self.mentions(place).and_then(|m| m.closed).filter(|closed| typed.as_ref().is_some_and(|at| at.start == *closed));
        let typed = typed.filter(|_| closed.is_none());
        // The figures of the papers a paper's conversation is about are read the first time an `@` is typed there.
        if typed.is_some() && self.offers(place).figures { self.want_figures(cx) }
        let hits = match &typed {
            Some(at) => self.mention_hits(place, &text[at.start + 1..at.end]),
            None => vec![],
        };
        let Some(mentions) = self.mentions_mut(place) else { return };
        mentions.closed = closed;
        if hits != mentions.hits { mentions.picked = 0 }
        mentions.hits = hits;
        mentions.at = typed.unwrap_or(0..0);
        let choosing = !mentions.hits.is_empty();
        if choosing { input.update(cx, |input, _| input.choosing = true) }
    }

    /// The rows for a query typed after `@`, one flat list of every kind the place offers. `@` alone: the first rows
    /// of each kind in its order (sections before papers where a manuscript is written, papers, the most used tags and
    /// sections in the library's conversation), each kind with rows getting at least two (`mention::share`). A query:
    /// every kind matched, names that begin with it before the rest and, of the same rank, papers, tags, then
    /// sections; the rows of a kind kept together, the kind with the best match first.
    fn mention_hits(&self, place: Place, query: &str) -> Vec<Hit> {
        let offers = self.offers(place);
        let alone = query.trim().is_empty();
        let mut lists: Vec<(Kind, Vec<(u8, Hit)>)> = vec![];
        if offers.libraries {
            let found: Vec<(u8, &wisp_core::libraries::LibraryInfo)> = if alone { self.shelf().take(OFFERED).map(|info| (0, info)).collect() } else { mention::matching_libraries_ranked(query, self.shelf(), OFFERED) };
            lists.push((Kind::Library, found.into_iter().map(|(rank, info)| (rank, Hit { kind: Kind::Library, token: mention::token(LIBRARY, &info.id), title: info.name.clone(), side: tf!("{} 篇", "{} papers", info.paper_count) })).collect()));
        }
        if let (Place::Chat(talk), Some(lib)) = (place, self.lib.as_ref()) {
            let candidates = self.candidates(place);
            let papers: Vec<(u8, &Paper)> = if alone {
                candidates.into_iter().take(OFFERED).map(|paper| (0, paper)).collect()
            } else {
                let sources = lib.chat(talk).sources();
                let label = |id: &str| sources.iter().find(|source| source.paper_id == id && source.library.is_empty()).map_or("", |source| source.label.as_str());
                mention::matching_ranked(query, candidates.into_iter().map(|paper| Candidate { paper, key: lib.keys.key(&paper.id).unwrap_or(""), label: label(&paper.id) }), OFFERED)
            };
            let papers: Vec<(u8, Hit)> = papers.into_iter().filter_map(|(rank, paper)| Some((rank, Hit { kind: Kind::Paper, token: mention::paper_token(lib.keys.key(&paper.id)?), title: ui::one_line(&paper.title), side: mention::byline(paper) }))).collect();
            let tags: Vec<(u8, Hit)> = if !offers.tags {
                vec![]
            } else {
                let found: Vec<(u8, &(String, usize))> = if alone { lib.tags.iter().take(OFFERED).map(|tag| (0, tag)).collect() } else { mention::matching_tags(query, &lib.tags, OFFERED) };
                found.into_iter().map(|(rank, (name, count))| (rank, Hit { kind: Kind::Tag, token: mention::token(TAG, name), title: name.clone(), side: tf!("{} 篇", "{} papers", count) })).collect()
            };
            let sections: Vec<(u8, Hit)> = match self.mention_manuscript(place) {
                Some(manuscript) => {
                    let named = manuscript.sections.iter().filter(|section| !section.name.trim().is_empty()).map(|section| (manuscript, section));
                    let found: Vec<(u8, (&Manuscript, &Section))> = if alone { named.take(OFFERED).map(|pair| (0, pair)).collect() } else { mention::matching_sections(query, named, OFFERED) };
                    found.into_iter().map(|(rank, (manuscript, section))| (rank, Hit { kind: Kind::Section, token: mention::token(SECTION, &section.id), title: ui::one_line(section.name.trim()), side: mention::cut(crate::i18n::manuscript_title(manuscript), 16) })).collect()
                }
                None => vec![],
            };
            let figures: Vec<(u8, Hit)> = if offers.figures { self.figure_hits(alone, query) } else { vec![] };
            // Where a manuscript is written its sections come first; where a paper is read, its figures.
            match talk {
                Talk::Write => lists.extend([(Kind::Section, sections), (Kind::Paper, papers)]),
                Talk::Paper => lists.extend([(Kind::Figure, figures), (Kind::Paper, papers)]),
                Talk::Library => lists.extend([(Kind::Paper, papers), (Kind::Tag, tags), (Kind::Section, sections)]),
            }
        }
        lists.retain(|(_, rows)| !rows.is_empty());
        if alone {
            let given = mention::share(&lists.iter().map(|(_, rows)| rows.len()).collect::<Vec<_>>(), OFFERED);
            return lists.into_iter().zip(given).flat_map(|((_, rows), n)| rows.into_iter().take(n).map(|(_, hit)| hit)).collect();
        }
        // The best eight of all, by rank and then figures, papers, tags, sections; shown kind by kind.
        let mut all: Vec<(u8, Kind, usize, Hit)> = lists.into_iter().flat_map(|(kind, rows)| rows.into_iter().enumerate().map(move |(n, (rank, hit))| (rank, kind, n, hit))).collect();
        all.sort_by_key(|(rank, kind, n, _)| (*rank, *kind, *n));
        all.truncate(OFFERED);
        let mut kinds: Vec<(u8, Kind)> = vec![];
        for (rank, kind, _, _) in &all {
            if !kinds.iter().any(|(_, known)| known == kind) { kinds.push((*rank, *kind)) }
        }
        let place_of = |kind: Kind| kinds.iter().position(|(_, known)| *known == kind).unwrap_or(0);
        all.sort_by_key(|(rank, kind, n, _)| (place_of(*kind), *rank, *n));
        all.into_iter().map(|(_, _, _, hit)| hit).collect()
    }

    /// A key heard by a box while its popup may be on show. True when the popup took it.
    pub(crate) fn mention_key(&mut self, place: Place, event: InputEvent, cx: &mut Context<Self>) -> bool {
        if event == InputEvent::Changed { self.mention_typed(place, cx) }
        let Some(input) = self.mention_box(place) else { return false };
        let open = self.mentions(place).is_some_and(|m| m.open(input.read(cx)));
        if !open { return false }
        let Some(mentions) = self.mentions_mut(place) else { return false };
        let rows = mentions.hits.len();
        match event {
            InputEvent::Submit | InputEvent::Tab => {
                let at = mentions.picked;
                self.pick_mention(place, at, cx);
            }
            InputEvent::Up => mentions.picked = (mentions.picked + rows - 1) % rows,
            InputEvent::Down => mentions.picked = (mentions.picked + 1) % rows,
            // Closed until another `@` is typed.
            InputEvent::Cancel => {
                mentions.closed = Some(mentions.at.start);
                mentions.hits.clear();
                input.update(cx, |input, _| input.choosing = false);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Take the n-th row on offer: the `@` and its query give way to the thing's token and a space, one step of Undo.
    pub(crate) fn pick_mention(&mut self, place: Place, n: usize, cx: &mut Context<Self>) {
        let Some(input) = self.mention_box(place) else { return };
        let Some(mentions) = self.mentions_mut(place) else { return };
        let Some(hit) = mentions.hits.get(n).cloned() else { return };
        let at = mentions.at.clone();
        mentions.hits.clear();
        mentions.picked = 0;
        input.update(cx, |input, cx| {
            if input.text().get(at.clone()).is_none_or(|typed| !typed.starts_with('@')) { return }
            input.choosing = false;
            let written = format!("{} ", hit.token);
            input.replace_range(at.clone(), &written, cx);
            let end = at.start + written.len();
            input.select_range(end..end, cx);
        });
        cx.notify();
    }

    /// The rows of the figures and tables of the papers a paper's conversation is about, those read so far: each paper's
    /// in the order of its panes, each file's in the order they stand in it. `@` alone: in that order; a query: by its
    /// number typed loosely or the words of its caption (`mention::matching_figures`). With several papers side by
    /// side, which one's at the right of the row, before the page.
    fn figure_hits(&self, alone: bool, query: &str) -> Vec<(u8, Hit)> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        let papers = lib.talk_papers();
        let several = papers.len() > 1;
        let english = crate::i18n::is_english();
        let all: Vec<(&Paper, &FigureOffer)> = papers.iter().filter_map(|id| Some((lib.paper(id)?, lib.figure_lists.get(id)?.figures.as_ref()?))).flat_map(|(paper, figures)| figures.iter().map(move |figure| (paper, figure))).collect();
        // `@` alone beside several papers: the first of each paper's, shared as the kinds share the rows (`mention::share`)
        // of what the papers leave.
        let found: Vec<(u8, usize)> = if alone {
            let counts: Vec<usize> = papers.iter().map(|id| all.iter().filter(|(paper, _)| paper.id == *id).count()).collect();
            // The papers themselves have their two rows after the figures.
            let given = mention::share(&counts, OFFERED - mention::FLOOR);
            let mut at = 0;
            let mut rows = vec![];
            for (count, n) in counts.into_iter().zip(given) {
                rows.extend((at..at + n).map(|row| (0, row)));
                at += count;
            }
            rows
        } else {
            let labels: Vec<String> = all.iter().map(|(_, figure)| figure.label()).collect();
            mention::matching_figures(query, all.iter().zip(&labels).map(|((_, figure), label)| FigureCandidate { key: figure.place.shown.key, label, legend: &figure.place.shown.legend }), OFFERED)
        };
        found
            .into_iter()
            .map(|(rank, n)| {
                let (paper, figure) = all[n];
                let page = match figure.attachment {
                    Some(at) => format!("S{at} p.{}", figure.page()),
                    None => format!("p.{}", figure.page()),
                };
                let side = if several { format!("{} · {page}", mention::short_name(paper, english)) } else { page };
                let title = ui::one_line(&format!("{} {}", figure.label(), figure.place.shown.legend.trim()));
                (rank, Hit { kind: Kind::Figure, token: mention::token(FIGURE, &mention::figure_value(&paper.id, &figure.file, figure.place.shown.key)), title, side })
            })
            .collect()
    }

    /// The PDFs of a paper whose figures `@` offers: the main text, then the attachments, each with its number among
    /// them (`S1`, as `Store::attachment_pages` and the answers count them).
    fn figure_files(&self, paper: &str) -> Vec<(String, PathBuf, Option<usize>)> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        let main = lib.main_pdf(paper);
        let mut files: Vec<(String, PathBuf, Option<usize>)> = main.iter().map(|file| (file.id.clone(), lib.store.file_path(file), None)).collect();
        for (n, file) in lib.attachments(paper).into_iter().enumerate() {
            if main.is_some_and(|main| main.id == file.id) { continue }
            files.push((file.id.clone(), lib.store.file_path(file), Some(n + 1)));
        }
        files
    }

    /// Read, off the main thread, the figures of the papers a paper's conversation is about that are not known yet
    /// (or whose PDFs changed since): what `@` offers there. A file a viewer already read is not read again
    /// (`pdf::file_figures`). What comes back is kept only for the library it was read for.
    pub(crate) fn want_figures(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let library = lib.info.id.clone();
        let wanted: Vec<(String, Vec<(String, PathBuf, Option<usize>)>)> = lib.talk_papers().into_iter().map(|paper| { let files = self.figure_files(&paper); (paper, files) }).collect();
        for (paper, files) in wanted {
            let ids: Vec<String> = files.iter().map(|(id, ..)| id.clone()).collect();
            let Some(lib) = self.lib.as_mut() else { return };
            if lib.figure_lists.get(&paper).is_some_and(|known| known.files == ids) { continue }
            lib.figure_lists.insert(paper.clone(), PaperFigures { files: ids.clone(), figures: None });
            let work = cx.background_executor().spawn(async move { read_figures(files) });
            let library = library.clone();
            cx.spawn(async move |this, cx| {
                let figures = work.await;
                this.update(cx, |this, cx| {
                    let Some(lib) = this.lib.as_mut().filter(|lib| lib.info.id == library) else { return };
                    let Some(known) = lib.figure_lists.get_mut(&paper).filter(|known| known.files == ids) else { return };
                    known.figures = Some(figures);
                    this.figures_ready(cx);
                })
                .ok();
            })
            .detach();
        }
    }

    /// A paper's figures were read, or a figure's picture cut out: the paper's box is offered them if its `@` is being
    /// typed, and a question waiting for them is sent.
    fn figures_ready(&mut self, cx: &mut Context<Self>) {
        let place = Place::Chat(Talk::Paper);
        let Some(lib) = self.lib.as_ref() else { return };
        let typing = { let input = lib.paper_chat.input.read(cx); mention::typed(input.text(), input.caret()).is_some() };
        if typing { self.mention_typed(place, cx) }
        if self.mentions(place).is_some_and(|m| m.waiting) && !self.figures_pending(Talk::Paper, cx) {
            if let Some(m) = self.mentions_mut(place) { m.waiting = false }
            self.ask(Talk::Paper, cx);
        }
        cx.notify();
    }

    /// The figures named in a conversation's box whose papers' figures are still read or whose pictures are still cut.
    fn figures_pending(&self, talk: Talk, cx: &App) -> bool {
        let Some(lib) = self.lib.as_ref() else { return false };
        let text = lib.chat(talk).input.read(cx).text().to_string();
        mention::tokens(&text).iter().any(|token| {
            let Named::Other { kind: FIGURE, value } = token.named else { return false };
            let value = mention::decode(value);
            let Some(named) = mention::figure_named(&value) else { return false };
            lib.figure_lists.get(&named.paper).is_some_and(|known| known.figures.is_none()) || matches!(lib.figure_cuts.get(&value), Some(Cut::Cutting))
        })
    }

    /// Cut out the picture of a figure named in a question, off the main thread, as a picture pasted is made one a
    /// model takes (`picture::prepare`). Kept for the library it was cut for, by the token's value.
    fn cut_figure(&mut self, value: &str, figure: &FigureOffer, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.figure_cuts.contains_key(value) { return }
        lib.figure_cuts.insert(value.to_string(), Cut::Cutting);
        let (library, value, path, pieces) = (lib.info.id.clone(), value.to_string(), figure.path.clone(), figure.pieces.clone());
        let id = mention::figure_picture_id(&value);
        let work = cx.background_executor().spawn(async move {
            // Why there is none is told to the model, in the words it reads everything else in.
            if !path.exists() { return Err("PDF 文件已经不在了".to_string()) }
            let png = crate::pdf::figure_picture(&path, &pieces).ok_or_else(|| "这一页画不出来".to_string())?;
            crate::picture::prepare(png).map_err(|_| "截出的图片读不了".to_string())
        });
        cx.spawn(async move |this, cx| {
            let cut = work.await;
            this.update(cx, |this, cx| {
                let Some(lib) = this.lib.as_mut().filter(|lib| lib.info.id == library) else { return };
                lib.figure_cuts.insert(value, match cut {
                    Ok(image) => Cut::Ready(ChatImage { id, image: Arc::new(image) }),
                    Err(why) => Cut::Failed(why),
                });
                this.figures_ready(cx);
            })
            .ok();
        })
        .detach();
    }

    /// The figures a question about a paper names, before it is sent: what the model is told of each and the pictures
    /// that go with it, after the pictures it has (`have`; a question asked again has its figures' pictures among them
    /// already, and none is put with it twice). `None` while one of them is still read or cut out: those still
    /// wanted are begun, and the question waits for them.
    pub(crate) fn question_figures(&mut self, talk: Talk, question: &str, have: &[ChatImage], again: bool, cx: &mut Context<Self>) -> Option<QuestionFigures> {
        let mut out = QuestionFigures { seen: vec![], pictures: vec![] };
        if !self.offers(Place::Chat(talk)).figures { return Some(out) }
        let values: Vec<String> = mention::tokens(question).iter().filter_map(|token| match token.named {
            Named::Other { kind: FIGURE, value } => Some(mention::decode(value)),
            _ => None,
        }).collect();
        if values.is_empty() { return Some(out) }
        let mut wait = false;
        let mut taken: Vec<String> = vec![];
        for value in values {
            if taken.contains(&value) { continue }
            taken.push(value.clone());
            let Some(named) = mention::figure_named(&value) else { continue };
            let lib = self.lib.as_ref()?;
            if lib.paper(&named.paper).is_none() { continue }
            let known = lib.figure_lists.get(&named.paper).filter(|known| known.files == self.figure_files(&named.paper).into_iter().map(|(id, ..)| id).collect::<Vec<_>>());
            let Some(figures) = known.map(|known| known.figures.as_ref()) else {
                // Not read yet (a conversation taken up from 历史, a token typed or pasted): read now, and wait.
                if !again { self.want_figures(cx); wait = true }
                continue;
            };
            let Some(figures) = figures else { wait |= !again; continue };
            // A figure no longer found stays the words of its token.
            let Some(figure) = figures.iter().find(|figure| figure.file == named.file && figure.place.shown.key == named.key).cloned() else { continue };
            let id = mention::figure_picture_id(&value);
            let had = have.iter().chain(&out.pictures).position(|picture| picture.id == id);
            let picture = match (had, lib.figure_cuts.get(&value).cloned()) {
                (Some(at), _) => Ok(at + 1),
                (None, _) if again => Err("这个问题当时没有带上它的图片".to_string()),
                (None, Some(Cut::Ready(image))) if have.len() + out.pictures.len() < crate::chat_state::MAX_ATTACHMENTS => {
                    out.pictures.push(image);
                    Ok(have.len() + out.pictures.len())
                }
                (None, Some(Cut::Ready(_))) => Err(format!("一个问题最多带 {} 张图片", crate::chat_state::MAX_ATTACHMENTS)),
                (None, Some(Cut::Failed(why))) => Err(why),
                (None, Some(Cut::Cutting)) => { wait = true; continue }
                (None, None) => {
                    self.cut_figure(&value, &figure, cx);
                    wait = true;
                    continue;
                }
            };
            out.seen.push((value, mention::FigureSeen { label: String::new(), attachment: figure.attachment, page: figure.page(), key: figure.label(), legend: figure.place.shown.legend.trim().to_string(), picture }));
        }
        (!wait).then_some(out)
    }

    /// A figure's mention clicked in a sent question: the paper's PDF (the file the figure is in) at the figure, its
    /// caption marked, as a citation's page is gone to: beside the conversation where there is room.
    pub(crate) fn mention_figure(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(named) = mention::figure_named(&value) else { return };
        let Some(lib) = self.lib.as_ref() else { return };
        if lib.paper(&named.paper).is_none() { return self.say(t("这篇文献已经不在文献库里了。"), cx) }
        let figure = match lib.figure_lists.get(&named.paper).map(|known| known.figures.as_ref()) {
            Some(Some(figures)) => figures.iter().find(|figure| figure.file == named.file && figure.place.shown.key == named.key).cloned(),
            Some(None) => return self.say(t("还在读这篇文献的图表，稍等再点一次。"), cx),
            None => {
                self.want_figures(cx);
                return self.say(t("还在读这篇文献的图表，稍等再点一次。"), cx);
            }
        };
        let Some(figure) = figure else { return self.say(t("这篇文献里已经找不到这张图了。"), cx) };
        let paper = named.paper;
        let file = (lib.main_pdf(&paper).map(|f| f.id.as_str()) != Some(figure.file.as_str())).then(|| figure.file.clone());
        let mut beside = lib.tab == Tab::Ask && lib.talk_papers().contains(&paper);
        self.show_file(&paper, file, cx);
        self.select(Some(paper), window, cx);
        if beside {
            self.run(Cmd::BesidePdf(true), window, cx);
            // A window too small for the two side by side shows the PDF in the conversation's place.
            beside = self.detail_width(window) >= crate::detail::SPLIT_MIN;
        }
        if !beside && let Some(lib) = self.lib.as_mut() { lib.tab = Tab::Pdf }
        self.ensure_pdf(cx);
        let Some((page, rect)) = figure.pieces.first().copied() else { return };
        // The caption is marked where it stands by the figure; a figure apart from its caption is gone to as it is.
        let mark = if figure.place.apart.is_none() { vec![figure.place.shown.caption] } else { vec![] };
        if let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.show_figure(page, rect[1], mark, cx)) }
        cx.notify();
    }

    /// A paper dropped on the library's conversation after its first question, or taken with its plus in the panel of
    /// papers: its token and a space at the box's caret (in place of what is selected there), one step of Undo, and
    /// the box has the keyboard. Before the first question the same drop keeps that question to the paper instead.
    pub(crate) fn mention_paper(&mut self, talk: Talk, paper_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.pinning = false;
        let Some(lib) = self.lib.as_ref() else { return };
        let Some(key) = lib.keys.key(paper_id) else { return };
        let token = mention::paper_token(key);
        let input = lib.chat(talk).input.clone();
        input.update(cx, |input, cx| {
            let at = input.selection();
            // Apart from the word before it, so the token is not read as part of it.
            let before = input.text()[..at.start].chars().next_back();
            let written = if before.is_some_and(|c| !c.is_whitespace()) { format!(" {token} ") } else { format!("{token} ") };
            input.replace_range(at.clone(), &written, cx);
            let end = at.start + written.len();
            input.select_range(end..end, cx);
        });
        input.read(cx).focus(window);
        cx.notify();
    }

    /// A tag's mention clicked: the papers' list kept to that tag, as a click on the tag in the sidebar away from the list does.
    pub(crate) fn mention_tag(&mut self, tag: String) {
        let Some(lib) = self.lib.as_mut() else { return };
        lib.filters.not_tags.retain(|known| *known != tag);
        lib.filters.tags = vec![tag];
        lib.page = Page::Papers;
        lib.refilter();
    }

    /// A section's mention clicked: 写作 at that section (in a conversation not yet made a library, the manuscript
    /// beside it). One removed since is said so.
    pub(crate) fn mention_section(&mut self, manuscript: String, section: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        if !lib.writing.manuscripts.iter().any(|m| m.id == manuscript && m.section(&section).is_some()) { return self.say(t("这一节已经不在稿件里了。"), cx) }
        if !lib.info.draft && lib.page != Page::Write { self.run(Cmd::SetPage(Page::Write), window, cx) }
        if let Some(lib) = self.lib.as_mut() { lib.writing.reading = false }
        self.write_show(manuscript, section, cx);
    }

    /// What each token of a box's text names that this place knows, how it is drawn and where a click on it leads: a
    /// paper's short name, `#tag`, `§section`, a library's name. A token nothing has is left out: it is drawn as written.
    fn mention_labels(&self, place: Place, text: &str) -> Vec<(Range<usize>, String, Option<Target>)> {
        if !text.contains("[@") { return vec![] }
        let english = crate::i18n::is_english();
        let offers = self.offers(place);
        let lib = self.lib.as_ref().filter(|_| matches!(place, Place::Chat(_)));
        mention::tokens(text)
            .into_iter()
            .filter_map(|token| match token.named {
                Named::Other { kind: LIBRARY, value } if offers.libraries => self.shelf().find(|info| info.id == value).map(|info| (token.range.clone(), info.name.clone(), Some(Target::Library(info.id.clone())))),
                Named::Other { kind: TAG, value } if offers.tags => {
                    let name = mention::decode(value);
                    let lib = lib?;
                    lib.tags.iter().any(|(known, _)| *known == name).then(|| (token.range.clone(), format!("#{name}"), (!lib.info.draft).then(|| Target::Tag(name.clone()))))
                }
                // A figure is drawn as the paper writes its number; while the paper's figures are still read, as its number reads.
                Named::Other { kind: FIGURE, value } if offers.figures => {
                    let lib = lib?;
                    let value = mention::decode(value);
                    let named = mention::figure_named(&value)?;
                    lib.paper(&named.paper)?;
                    let label = match lib.figure_lists.get(&named.paper).and_then(|known| known.figures.as_ref()) {
                        Some(figures) => figures.iter().find(|figure| figure.file == named.file && figure.place.shown.key == named.key)?.label(),
                        None => lib.files.iter().any(|file| file.id == named.file).then(|| mention::key_written(named.key))?,
                    };
                    Some((token.range.clone(), label, Some(Target::Figure(value))))
                }
                // A section of any of the library's manuscripts: the one on show may have changed since it was named.
                Named::Other { kind: SECTION, value } => {
                    let lib = lib?;
                    lib.writing.manuscripts.iter().find_map(|m| m.section(value).map(|section| (m, section))).map(|(m, section)| (token.range.clone(), format!("§{}", ui::one_line(section.name.trim())), Some(Target::Section(m.id.clone(), section.id.clone()))))
                }
                named => {
                    let lib = lib?;
                    let paper = lib.paper(lib.keys.paper(&named)?)?;
                    Some((token.range, mention::short_name(paper, english), Some(Target::Paper(paper.id.clone()))))
                }
            })
            .collect()
    }

    /// Give a box the folds of its mentions, when its text changed since they were last given or what they name
    /// reads otherwise now. Called before the box is drawn; reads only what is in memory.
    pub(crate) fn fold_mentions(&mut self, place: Place, cx: &mut Context<Self>) {
        let Some(input) = self.mention_box(place) else { return };
        let revision = input.read(cx).revision();
        let folds: Vec<(Range<usize>, String)> = self.mention_labels(place, input.read(cx).text()).into_iter().map(|(range, label, _)| (range, label)).collect();
        if self.mentions(place).is_some_and(|m| m.folded.as_ref().is_some_and(|(at, given)| *at == revision && *given == folds)) { return }
        input.update(cx, |input, _| input.set_folds(folds.clone()));
        if let Some(mentions) = self.mentions_mut(place) { mentions.folded = Some((revision, folds)) }
    }

    /// A question as its bubble shows it: each mention as its short name, and what a click on it does (a paper's goes
    /// to that paper, as its label in an answer does; a tag's to the list of its papers; a section's to 写作 there; a
    /// library's opens it). What 复制 copies.
    pub(crate) fn mention_shown(&self, talk: Talk, text: &str) -> (String, Vec<(Range<usize>, Option<Cmd>)>) {
        self.shown_in(Place::Chat(talk), text)
    }

    /// The words of a box with each mention as its short name: what a conversation started with them is named, and
    /// the title in 历史 of one saved before titles were.
    pub(crate) fn mention_plain(&self, place: Place, text: &str) -> String {
        self.shown_in(place, text).0
    }

    fn shown_in(&self, place: Place, text: &str) -> (String, Vec<(Range<usize>, Option<Cmd>)>) {
        let labels = self.mention_labels(place, text);
        if labels.is_empty() { return (text.to_string(), vec![]) }
        let sources = match (place, self.lib.as_ref()) {
            (Place::Chat(talk), Some(lib)) => lib.chat(talk).sources(),
            _ => &[],
        };
        let mut shown = String::with_capacity(text.len());
        let mut marks = vec![];
        let mut at = 0;
        for (range, label, target) in labels {
            shown.push_str(&text[at..range.start]);
            let from = shown.len();
            shown.push_str(&label);
            let go = target.map(|target| match target {
                Target::Paper(id) => match (place, sources.iter().find(|source| source.paper_id == id && source.library.is_empty())) {
                    (Place::Chat(talk), Some(source)) => Cmd::GotoCited(talk, source.label.clone(), None, None, String::new()),
                    _ => Cmd::GotoFile(id, None, None, String::new()),
                },
                Target::Tag(name) => Cmd::MentionTag(name),
                Target::Section(manuscript, section) => Cmd::MentionSection(manuscript, section),
                Target::Library(id) => Cmd::OpenLibrary(id),
                Target::Figure(value) => Cmd::MentionFigure(value),
            });
            marks.push((from..shown.len(), go));
            at = range.end;
        }
        shown.push_str(&text[at..]);
        (shown, marks)
    }

    /// The popup over a box while its `@` is offered something: the slash commands' list, its rows by kind, with a quiet
    /// caption over each kind when there are several. `left` is where the box's text begins, `room` how wide the list may be.
    pub(crate) fn render_mentions(&self, place: Place, left: f32, room: f32, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let input = self.mention_box(place)?;
        if !input.focus_handle(cx).is_focused(window) { return None }
        let mentions = self.mentions(place)?;
        if !mentions.open(input.read(cx)) { return None }
        let several = mentions.hits.iter().any(|hit| hit.kind != mentions.hits[0].kind);
        let side = match place {
            Place::Chat(talk) => talk.side(),
            Place::Home => "home",
        };
        let mut rows: Vec<AnyElement> = vec![];
        for (n, hit) in mentions.hits.iter().enumerate() {
            if several && (n == 0 || mentions.hits[n - 1].kind != hit.kind) {
                rows.push(ui::caption(t(hit.kind.caption())).px(px(8.)).pt(px(if n == 0 { 2. } else { 6. })).pb(px(2.)).into_any_element());
            }
            rows.push(
                crate::chat::choice_row(SharedString::from(format!("mention-{side}-{n}")), n == mentions.picked)
                    .on_click(self.on(Cmd::PickMention(place, n), cx))
                    .child(ui::icon(hit.kind.icon(), 14., Theme::text_tertiary()))
                    .child(div().flex_1().min_w_0().truncate().text_color(Theme::text()).child(hit.title.clone()))
                    .when(!hit.side.is_empty(), |d| d.child(div().flex_none().max_w(px(180.)).truncate().text_size(text_px(12.)).text_color(Theme::text_placeholder()).child(hit.side.clone())))
                    .into_any_element(),
            );
        }
        Some(crate::chat::choices(left, room, rows))
    }

    /// A key or a change in the home page's box.
    pub(crate) fn home_input(&mut self, event: InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.mention_key(Place::Home, event, cx) { return }
        if event == InputEvent::Submit { return self.draft_send(window, cx) }
        let open = self.home.mentions.open(self.home.input.read(cx));
        self.home.input.update(cx, |input, _| input.choosing = open);
        cx.notify();
    }
}

/// The figures and tables of a paper's PDFs (`figure_files`), as `@` offers them: of each number the place its file
/// shows it surest (`figref::Index::find`), in the order they stand in the file. Off the main thread.
fn read_figures(files: Vec<(String, PathBuf, Option<usize>)>) -> Vec<FigureOffer> {
    let mut out = vec![];
    for (file, path, attachment) in files {
        let Some(read) = crate::pdf::file_figures(&path) else { continue };
        let mut keys: Vec<figref::Key> = vec![];
        let mut places: Vec<&figref::Place> = vec![];
        for place in read.index.places() {
            if keys.contains(&place.shown.key) { continue }
            keys.push(place.shown.key);
            if let Some(best) = read.index.find(place.shown.key) { places.push(best) }
        }
        // A figure whose caption goes on from another page ("See legend on previous page") is told by the longest of its captions.
        let legend = |key: figref::Key| read.index.places().iter().filter(|place| place.shown.key == key).map(|place| place.shown.legend.as_str()).max_by_key(|legend| legend.len()).unwrap_or_default().to_string();
        places.sort_by(|a, b| (a.page, a.shown.caption[1]).partial_cmp(&(b.page, b.shown.caption[1])).unwrap_or(std::cmp::Ordering::Equal));
        let size = |page: usize| read.sizes.get(page).copied().unwrap_or((595., 842.));
        out.extend(places.into_iter().map(|place| {
            let mut kept = place.clone();
            kept.shown.legend = legend(place.shown.key);
            FigureOffer { file: file.clone(), path: path.clone(), attachment, pieces: place.pieces(size), place: kept }
        }));
    }
    out
}

/// A question's pictures as they go back to its box (撤回, or a question that could not be sent): without those of
/// the figures its text names, which the tokens stand for in the box and which go with it again when it is sent
/// (kept in `Library::figure_cuts`).
pub(crate) fn box_pictures(cuts: &mut std::collections::HashMap<String, Cut>, text: &str, images: Vec<ChatImage>) -> Vec<ChatImage> {
    let named: Vec<String> = mention::tokens(text).iter().filter_map(|token| match token.named {
        Named::Other { kind: FIGURE, value } => Some(mention::decode(value)),
        _ => None,
    }).collect();
    if named.is_empty() { return images }
    images.into_iter().filter(|picture| {
        let Some(value) = named.iter().find(|value| mention::figure_picture_id(value) == picture.id) else { return true };
        cuts.insert(value.clone(), Cut::Ready(picture.clone()));
        false
    }).collect()
}
