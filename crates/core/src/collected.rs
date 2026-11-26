//! 收藏 across the shelf: what every library holds under 收藏 (its starred papers, its figures, the passages of
//! 方法库 and 导言库), read side by side for the star on the home page. Nothing is moved or copied: each library
//! keeps its own, and inside a library 收藏 shows that library's alone. What is read here is a list to look through
//! and to go from, each thing back to the library it is in; the one thing written from it is a paper's star.

use crate::excerpt::{Excerpt, INTRO, METHOD};
use crate::figure::Figure;
use crate::libraries::LibraryInfo;
use crate::model::Paper;
use crate::store::Store;
use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The four things 收藏 holds, in the order its switch names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Papers,
    Figures,
    Methods,
    Intros,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Papers, Kind::Figures, Kind::Methods, Kind::Intros];

    pub fn key(self) -> &'static str {
        match self {
            Kind::Papers => "papers",
            Kind::Figures => "figures",
            Kind::Methods => "methods",
            Kind::Intros => "intros",
        }
    }

    pub fn from_key(key: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.key() == key)
    }

    /// The kind of excerpt a page of passages lists.
    fn excerpt(self) -> Option<&'static str> {
        match self {
            Kind::Methods => Some(METHOD),
            Kind::Intros => Some(INTRO),
            _ => None,
        }
    }
}

/// What one library holds under 收藏.
#[derive(Clone, Debug, Default)]
pub struct Held {
    /// The library's id, its name and its folder.
    pub library: String,
    pub name: String,
    pub dir: PathBuf,
    /// Its starred papers, the newest first.
    pub papers: Vec<Paper>,
    pub figures: Vec<Figure>,
    /// The passages of both kinds.
    pub excerpts: Vec<Excerpt>,
    /// Which figures a method is linked to, as (passage, figure), in the person's order.
    pub links: Vec<(String, String)>,
    /// The papers its figures and passages are from that are not starred themselves: where each is from is said by them.
    pub sources: Vec<Paper>,
}

impl Held {
    /// What `store`, the library `library` called `name`, holds.
    pub fn of(library: &str, name: &str, store: &Store) -> Result<Held> {
        let figures = store.list_figures()?;
        let excerpts = store.list_excerpts(None)?;
        let from: HashSet<&str> = figures.iter().map(|figure| figure.paper_id.as_str()).chain(excerpts.iter().map(|excerpt| excerpt.paper_id.as_str())).filter(|id| !id.is_empty()).collect();
        let (mut papers, mut sources) = (vec![], vec![]);
        for paper in store.list_papers()? {
            if paper.starred { papers.push(paper) } else if from.contains(paper.id.as_str()) { sources.push(paper) }
        }
        let links = store.excerpt_links()?;
        Ok(Held { library: library.into(), name: name.into(), dir: store.root().to_path_buf(), papers, figures, excerpts, links, sources })
    }

    /// How many of a kind it holds. A paper whose star was taken off here since it was read is not counted.
    pub fn count(&self, kind: Kind) -> usize {
        match kind {
            Kind::Papers => self.papers.iter().filter(|paper| paper.starred).count(),
            Kind::Figures => self.figures.len(),
            Kind::Methods | Kind::Intros => self.excerpts.iter().filter(|excerpt| Some(excerpt.kind.as_str()) == kind.excerpt()).count(),
        }
    }

    pub fn total(&self) -> usize {
        Kind::ALL.into_iter().map(|kind| self.count(kind)).sum()
    }

    /// A paper of the library that something here is, or is from.
    pub fn paper(&self, id: &str) -> Option<&Paper> {
        if id.is_empty() { return None }
        self.papers.iter().chain(&self.sources).find(|paper| paper.id == id)
    }

    /// The figures a method is linked to that the library still has, in the person's order.
    pub fn figures_of(&self, excerpt: &str) -> Vec<&Figure> {
        self.links.iter().filter(|(of, _)| of == excerpt).filter_map(|(_, id)| self.figures.iter().find(|figure| figure.id == *id)).collect()
    }

    /// When the thing at `at` of a kind was added, and whether it is of that kind at all.
    fn added(&self, kind: Kind, at: usize) -> Option<&str> {
        match kind {
            Kind::Papers => self.papers.get(at).map(|paper| paper.added_at.as_str()),
            Kind::Figures => self.figures.get(at).map(|figure| figure.added_at.as_str()),
            Kind::Methods | Kind::Intros => self.excerpts.get(at).filter(|excerpt| Some(excerpt.kind.as_str()) == kind.excerpt()).map(|excerpt| excerpt.added_at.as_str()),
        }
    }

    fn len(&self, kind: Kind) -> usize {
        match kind {
            Kind::Papers => self.papers.len(),
            Kind::Figures => self.figures.len(),
            Kind::Methods | Kind::Intros => self.excerpts.len(),
        }
    }

    /// Whether the thing at `at` of a kind says every word, in itself or in the record of its paper.
    fn says(&self, kind: Kind, at: usize, words: &[String]) -> bool {
        match kind {
            Kind::Papers => self.papers.get(at).is_some_and(|paper| paper_matches(paper, words)),
            Kind::Figures => self.figures.get(at).is_some_and(|figure| figure.matches(self.paper(&figure.paper_id), words)),
            Kind::Methods | Kind::Intros => self.excerpts.get(at).is_some_and(|excerpt| excerpt.matches(self.paper(&excerpt.paper_id), words)),
        }
    }
}

/// What the library `info` holds, read from its folder.
pub fn read(info: &LibraryInfo) -> Result<Held> {
    Held::of(&info.id, &info.name, &Store::open(&info.dir)?)
}

/// Everything of a kind in the libraries, as (library, place in its list of that kind), the newest first whichever
/// library it is in; of two added in the same moment, the one of the library earlier on the shelf goes first.
/// For a kind of passage the place is in `Held::excerpts`, which holds both kinds.
pub fn merged(held: &[Held], kind: Kind) -> Vec<(usize, usize)> {
    let mut all: Vec<(usize, usize, &str)> = vec![];
    for (library, one) in held.iter().enumerate() {
        all.extend((0..one.len(kind)).filter_map(|at| one.added(kind, at).map(|added| (library, at, added))));
    }
    all.sort_by(|a, b| b.2.cmp(a.2));
    all.into_iter().map(|(library, at, _)| (library, at)).collect()
}

/// Those of `order` on show: of one library when one is chosen, and saying every word searched for
/// (`figure::search_words`).
pub fn shown(held: &[Held], order: &[(usize, usize)], kind: Kind, library: Option<&str>, words: &[String]) -> Vec<(usize, usize)> {
    order.iter().copied().filter(|&(of, at)| held.get(of).is_some_and(|one| library.is_none_or(|id| one.library == id) && one.says(kind, at, words))).collect()
}

/// Whether every word is found in a paper's record: its title, authors, journal, year, identifiers, tags and notes.
/// The words are lowercase already.
pub fn paper_matches(paper: &Paper, words: &[String]) -> bool {
    if words.is_empty() { return true }
    let mut hay = [paper.title.as_str(), paper.authors.as_str(), paper.journal.as_str(), paper.doi.as_str(), paper.pmid.as_str(), paper.notes.as_str()].join("\n");
    if let Some(year) = paper.year { hay.push_str(&format!("\n{year}")) }
    for tag in &paper.tags { hay.push('\n'); hay.push_str(tag) }
    let hay = hay.to_lowercase();
    words.iter().all(|word| hay.contains(word.as_str()))
}

/// Star a paper of the library in `dir`, or take its star off: the one change made from the list across libraries.
pub fn star(dir: &Path, paper: &str, starred: bool) -> Result<()> {
    let mut store = Store::open(dir)?;
    let mut paper = store.get_paper(paper)?;
    paper.starred = starred;
    store.save_paper(&mut paper)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::figure::search_words;
    use crate::model::new_id;

    fn png() -> Vec<u8> {
        b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x10\x00\x00\x00\x10\x08\x02\x00\x00\x00".to_vec()
    }

    fn folder(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("wisp-collected-{name}-{}", new_id("T")));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// A library with a starred paper, one that is not, a figure and a method of the unstarred one linked together,
    /// and an introduction the person wrote down.
    fn library(name: &str, title: &str) -> (PathBuf, Held) {
        let root = folder(name);
        let mut store = Store::open(&root).unwrap();
        let kept = store.add_paper(Paper { title: title.into(), authors: "Zhang, Wei; Li, Na".into(), year: Some(2021), starred: true, tags: vec!["根".into()], ..Default::default() }).unwrap();
        let other = store.add_paper(Paper { title: format!("{title}, the methods"), authors: "Okada, K".into(), year: Some(2019), ..Default::default() }).unwrap();
        store.add_paper(Paper { title: "Never looked at again".into(), ..Default::default() }).unwrap();
        let mut figure = Figure { paper_id: other.id.clone(), page: 3, label: "Fig. 2".into(), legend: "Confocal sections of the root tip.".into(), ..Default::default() };
        store.add_figure(&mut figure, &png()).unwrap();
        let mut method = Excerpt { paper_id: other.id.clone(), page: 7, name: "Confocal imaging".into(), text: "Roots were stained with propidium iodide.".into(), ..Default::default() };
        store.add_excerpt(&mut method).unwrap();
        store.set_excerpt_figures(&method.id, std::slice::from_ref(&figure.id)).unwrap();
        let mut intro = Excerpt { kind: INTRO.into(), name: "从临床问题切入".into(), text: "Drought limits yield.".into(), ..Default::default() };
        store.add_excerpt(&mut intro).unwrap();
        let held = Held::of(name, name, &store).unwrap();
        assert_eq!((kept.starred, held.dir.as_path()), (true, root.as_path()));
        (root, held)
    }

    #[test]
    fn a_library_is_read_for_what_it_holds() {
        let (root, held) = library("roots", "Auxin and the root stem cell niche");
        assert_eq!(Kind::ALL.map(|kind| held.count(kind)), [1, 1, 1, 1]);
        assert_eq!(held.total(), 4);
        // The starred paper alone is listed; the paper the figure and the method are from is kept to say where they are from, the third is not read.
        assert_eq!((held.papers.len(), held.sources.len()), (1, 1));
        let figure = &held.figures[0];
        assert_eq!(held.paper(&figure.paper_id).map(|paper| paper.authors.as_str()), Some("Okada, K"));
        assert!(held.paper("").is_none());
        let method = held.excerpts.iter().find(|excerpt| excerpt.kind == METHOD).unwrap();
        assert_eq!(held.figures_of(&method.id).iter().map(|figure| figure.label.as_str()).collect::<Vec<_>>(), ["Fig. 2"]);
        // A star taken off from the list is written to the library, and no longer counted.
        let id = held.papers[0].id.clone();
        star(&root, &id, false).unwrap();
        assert!(!Store::open(&root).unwrap().get_paper(&id).unwrap().starred);
        assert_eq!(read(&LibraryInfo { id: "roots".into(), name: "Roots".into(), dir: root.clone(), ..Default::default() }).unwrap().count(Kind::Papers), 0);
        let mut held = held;
        held.papers[0].starred = false;
        assert_eq!((held.count(Kind::Papers), held.total()), (0, 3));
        assert!(star(&root, "P-none", true).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn libraries_are_listed_together_and_narrowed() {
        let (first, mut roots) = library("roots", "Auxin and the root stem cell niche");
        let (second, mut leaves) = library("leaves", "Stomata under drought");
        // Whatever the clock gave them: the leaf library's are the newer, but for its paper.
        for (held, day) in [(&mut roots, "2026-01-02"), (&mut leaves, "2026-03-04")] {
            held.figures[0].added_at = day.into();
            for excerpt in &mut held.excerpts { excerpt.added_at = day.into() }
        }
        roots.papers[0].added_at = "2026-05-06".into();
        leaves.papers[0].added_at = "2026-01-01".into();
        let held = [roots, leaves];
        assert_eq!(merged(&held, Kind::Papers), [(0, 0), (1, 0)]);
        assert_eq!(merged(&held, Kind::Figures), [(1, 0), (0, 0)]);
        // A passage is placed in the list of both kinds; each kind lists its own.
        let (methods, intros) = (merged(&held, Kind::Methods), merged(&held, Kind::Intros));
        assert_eq!((methods.len(), intros.len()), (2, 2));
        assert!(methods.iter().all(|&(library, at)| held[library].excerpts[at].kind == METHOD) && intros.iter().all(|&(library, at)| held[library].excerpts[at].kind == INTRO));
        assert_eq!((methods[0].0, intros[0].0), (1, 1));
        // Added in the same moment: the library earlier on the shelf first.
        let mut same = held.clone();
        same[1].figures[0].added_at = "2026-01-02".into();
        assert_eq!(merged(&same, Kind::Figures), [(0, 0), (1, 0)]);

        // One library's alone; a word of the thing itself, or of the paper it is from.
        let order = merged(&held, Kind::Figures);
        assert_eq!(shown(&held, &order, Kind::Figures, None, &[]), order);
        assert_eq!(shown(&held, &order, Kind::Figures, Some("roots"), &[]), [(0, 0)]);
        assert_eq!(shown(&held, &order, Kind::Figures, None, &search_words("confocal okada")), order);
        assert_eq!(shown(&held, &order, Kind::Figures, None, &search_words("Stomata")), [(1, 0)]);
        assert!(shown(&held, &order, Kind::Figures, Some("roots"), &search_words("stomata")).is_empty());
        let papers = merged(&held, Kind::Papers);
        assert_eq!(shown(&held, &papers, Kind::Papers, None, &search_words("zhang 2021 根")), papers);
        assert_eq!(shown(&held, &papers, Kind::Papers, None, &search_words("auxin")), [(0, 0)]);
        assert_eq!(shown(&held, &merged(&held, Kind::Intros), Kind::Intros, None, &search_words("临床")).len(), 2);
        assert_eq!((Kind::from_key("methods"), Kind::from_key("notes")), (Some(Kind::Methods), None));
        for root in [first, second] { let _ = std::fs::remove_dir_all(root); }
    }
}
