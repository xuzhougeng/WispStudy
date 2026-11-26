//! 稿件 across the shelf: the manuscripts of every library, and those begun on the home page that are in no
//! library yet (未绑定: `LibraryInfo::draft`), read side by side for the pen on the home page. Nothing is moved
//! or copied by the reading: a manuscript is written where it is kept, in its library's 写作 or in the window
//! of the conversation that holds it. The empty manuscript 写作 opens a library with, which nothing was done
//! with (`Store::manuscript_blank`), is not one of them.

use crate::libraries::LibraryInfo;
use crate::manuscript::{Manuscript, text_length};
use crate::store::Store;
use anyhow::Result;
use std::path::PathBuf;

/// One manuscript, as a row says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Piece {
    pub id: String,
    /// Empty until the person gives it one.
    pub title: String,
    pub updated_at: String,
    /// How much is written (`manuscript::text_length`), and whether letters were counted (字, else 词).
    pub length: usize,
    pub letters: bool,
    /// The first words written, on one line: what tells two manuscripts without a title apart.
    pub opening: String,
    /// Its title, the names of its sections and its text in lower case: what a search looks through.
    hay: String,
}

/// How many characters of a manuscript's first words a row keeps.
const OPENING_CHARS: usize = 120;

impl Piece {
    pub fn of(manuscript: &Manuscript) -> Piece {
        let text: String = manuscript.sections.iter().map(|section| section.body.as_str()).collect::<Vec<_>>().join("\n");
        let (length, letters) = text_length(&text);
        // A picture's line is no words; nor is what marks a heading or a list.
        let opening = text.lines().map(|line| line.trim().trim_start_matches(['#', '>', '-', '*', ' ']).trim()).find(|line| !line.is_empty() && !line.starts_with("![")).unwrap_or("");
        let opening: String = opening.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(OPENING_CHARS).collect();
        let names: Vec<&str> = manuscript.sections.iter().filter(|section| !section.bare).map(|section| section.name.as_str()).collect();
        let hay = format!("{}\n{}\n{text}", manuscript.title, names.join("\n")).to_lowercase();
        Piece { id: manuscript.id.clone(), title: manuscript.title.trim().to_string(), updated_at: manuscript.updated_at.clone(), length, letters, opening, hay }
    }

    /// Whether it says every word (lower case already), in its title, a section's name or its text.
    pub fn says(&self, words: &[String]) -> bool {
        words.iter().all(|word| self.hay.contains(word.as_str()))
    }
}

/// The manuscripts one library holds, or one conversation of the home page.
#[derive(Clone, Debug, Default)]
pub struct Held {
    /// The library's id, its name and its folder.
    pub library: String,
    pub name: String,
    pub dir: PathBuf,
    /// It is a library on the shelf: its manuscripts are bound to it. Not so for a conversation of the home page.
    pub bound: bool,
    /// The latest written first.
    pub pieces: Vec<Piece>,
}

impl Held {
    /// What `store` holds, the library (or conversation) `info`.
    pub fn of(info: &LibraryInfo, store: &Store) -> Result<Held> {
        let mut pieces = vec![];
        for manuscript in store.manuscripts()? {
            if !store.manuscript_blank(&manuscript.id)? { pieces.push(Piece::of(&manuscript)) }
        }
        pieces.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.id.cmp(&b.id)));
        Ok(Held { library: info.id.clone(), name: info.name.clone(), dir: info.dir.clone(), bound: !info.draft, pieces })
    }
}

/// What the library or conversation `info` holds, read from its folder.
pub fn read(info: &LibraryInfo) -> Result<Held> {
    Held::of(info, &Store::open(&info.dir)?)
}

/// Which manuscripts are looked at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Place {
    /// All of them.
    #[default]
    All,
    /// Those in no library.
    Unbound,
    /// One library's, by its id.
    Library(String),
}

impl Place {
    fn holds(&self, held: &Held) -> bool {
        match self {
            Place::All => true,
            Place::Unbound => !held.bound,
            Place::Library(id) => held.bound && held.library == *id,
        }
    }
}

/// How many manuscripts a place holds.
pub fn count(held: &[Held], place: &Place) -> usize {
    held.iter().filter(|one| place.holds(one)).map(|one| one.pieces.len()).sum()
}

/// Every manuscript, as (holder, place in its list), the latest written first whoever holds it; of two written
/// in the same moment, the one whose holder comes first.
pub fn merged(held: &[Held]) -> Vec<(usize, usize)> {
    let mut all: Vec<(usize, usize, &str)> = held.iter().enumerate().flat_map(|(of, one)| one.pieces.iter().enumerate().map(move |(at, piece)| (of, at, piece.updated_at.as_str()))).collect();
    all.sort_by(|a, b| b.2.cmp(a.2));
    all.into_iter().map(|(of, at, _)| (of, at)).collect()
}

/// Those of `order` on show: of the place looked at, and saying every word searched for (`figure::search_words`).
pub fn shown(held: &[Held], order: &[(usize, usize)], place: &Place, words: &[String]) -> Vec<(usize, usize)> {
    order.iter().copied().filter(|&(of, at)| held.get(of).is_some_and(|one| place.holds(one) && one.pieces.get(at).is_some_and(|piece| piece.says(words)))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::figure::search_words;
    use crate::libraries;
    use crate::model::new_id;

    #[test]
    fn manuscripts_are_listed_across_libraries_and_outside_them() {
        let root = std::env::temp_dir().join(format!("wisp-written-{}", new_id("T")));
        std::fs::create_dir_all(&root).unwrap();
        // A library with a manuscript that has words, and the empty one 写作 opened it with.
        let roots = libraries::create(&root, "根毛", "", "green").unwrap();
        let mut store = Store::open(&roots.dir).unwrap();
        store.create_manuscript("", "en").unwrap();
        let review = store.create_manuscript("Root hairs under salt", "en").unwrap();
        store.save_section_body(&review.sections[1].id, "## Background\n\n![A root](figures/a.png)\n\nRoot hairs are **short** under salt.").unwrap();
        // One begun on the home page, in no library: a piece of text under no heading, kept though nothing is written.
        let (unbound, piece) = libraries::create_writing(&root, crate::manuscript::UNTITLED, "zh").unwrap();
        assert!(unbound.draft && unbound.writing);
        let mut own = Store::open(&unbound.dir).unwrap();
        let made = own.manuscript(&piece).unwrap().unwrap();
        assert_eq!((made.sections.len(), made.sections[0].bare, made.sections[0].name.as_str(), made.language.as_str()), (1, true, "正文", "zh"));
        assert!(!own.manuscript_blank(&piece).unwrap());
        // A conversation of the home page that holds no manuscript is read, and holds none.
        let talk = libraries::create_draft(&root, "一个想法").unwrap();

        let all = libraries::list(&root);
        let find = |id: &str| all.iter().find(|info| info.id == id).unwrap();
        let held = [read(find(&roots.id)).unwrap(), read(find(&unbound.id)).unwrap(), read(find(&talk.id)).unwrap()];
        assert_eq!(held.iter().map(|one| (one.bound, one.pieces.len())).collect::<Vec<_>>(), [(true, 1), (false, 1), (false, 0)]);
        let first = &held[0].pieces[0];
        assert_eq!((first.title.as_str(), first.length, first.letters, first.opening.as_str()), ("Root hairs under salt", 9, false, "Background"));
        assert_eq!((held[1].pieces[0].title.as_str(), held[1].pieces[0].length, held[1].pieces[0].opening.as_str()), ("", 0, ""));

        own.save_section_body(&made.sections[0].id, "盐胁迫下，根毛为什么变短？").unwrap();
        let held = [held[0].clone(), read(find(&unbound.id)).unwrap(), held[2].clone()];
        assert_eq!((held[1].pieces[0].length, held[1].pieces[0].letters, held[1].pieces[0].opening.as_str()), (11, true, "盐胁迫下，根毛为什么变短？"));
        // The one written last is first, whoever holds it.
        let order = merged(&held);
        assert_eq!(order, [(1, 0), (0, 0)]);
        assert_eq!((count(&held, &Place::All), count(&held, &Place::Unbound), count(&held, &Place::Library(roots.id.clone())), count(&held, &Place::Library(unbound.id.clone()))), (2, 1, 1, 0));
        assert_eq!(shown(&held, &order, &Place::Unbound, &[]), [(1, 0)]);
        assert_eq!(shown(&held, &order, &Place::Library(roots.id.clone()), &[]), [(0, 0)]);
        // A word of the title, of a heading, of the text; all of them must be said.
        assert_eq!(shown(&held, &order, &Place::All, &search_words("SALT background")), [(0, 0)]);
        assert_eq!(shown(&held, &order, &Place::All, &search_words("根毛")), [(1, 0)]);
        assert!(shown(&held, &order, &Place::Unbound, &search_words("salt")).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
