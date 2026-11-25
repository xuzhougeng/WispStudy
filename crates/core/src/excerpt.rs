//! 方法库 and 导言库: passages kept from the papers of a library in the
//! papers' own words, of two kinds. A method (`METHOD`) is an experimental
//! method, kept to gather, compare and choose among methods and to change a
//! parameter with evidence behind the change; an introduction (`INTRO`) is
//! how a paper opens and leads into its question (the background it chooses,
//! how it states the gap and arrives at its aim), kept to look over when
//! writing one's own. Both keep the words as they were selected (`text`) and
//! the place they stand (paper, file, page), so they always lead back to
//! their source; the person gives each a name to find it by and put it
//! beside others of its kind, tags, and notes of their own. One the person
//! wrote down themselves has no paper: its `paper_id` is empty.
//!
//! An excerpt can be linked to figures of 图片库 (`excerpt_figures`): for a
//! method, what it produced, the blot a Western blot protocol gave. The
//! window offers the links for methods only; the store keeps them for any.
//! They follow an excerpt or a figure out of the library. An excerpt has no
//! file, so taking one out is a plain row deleted.

use crate::model::{Paper, clean_strings, new_id};
use crate::store::Store;
use crate::time::now_iso;
use anyhow::{Result, anyhow, bail};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

/// The kinds, as stored.
pub const METHOD: &str = "method";
pub const INTRO: &str = "intro";

/// The kinds with what they are called, the label translated where it is shown.
pub const KINDS: [(&str, &str); 2] = [(METHOD, "方法"), (INTRO, "导言")];

/// The tables, made with the library's own (`store::SCHEMA`) after the figures'. An older build does not know
/// them and reads the rest of the database as before.
pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS excerpts (
    id TEXT PRIMARY KEY NOT NULL,
    -- What it is: `method` or `intro`.
    kind TEXT NOT NULL DEFAULT 'method',
    -- The paper it was taken from; empty for one the person wrote down. Not a reference, so that it can be
    -- empty: the trigger below is what takes a paper's excerpts along with it.
    paper_id TEXT NOT NULL DEFAULT '',
    -- The file its words were selected in. Not a reference: the excerpt stays when that file is taken off its paper.
    resource_id TEXT NOT NULL DEFAULT '',
    -- The 1-based page its words stand on; 0 when unknown.
    page INTEGER NOT NULL DEFAULT 0,
    name TEXT NOT NULL DEFAULT '',
    -- The paper's words, as selected.
    text TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT '',
    tags_json TEXT NOT NULL DEFAULT '[]',
    added_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_excerpts_paper ON excerpts(paper_id);
-- The figures an excerpt is linked to, in the order the person put them.
CREATE TABLE IF NOT EXISTS excerpt_figures (
    excerpt_id TEXT NOT NULL,
    figure_id TEXT NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (excerpt_id, figure_id)
);
CREATE INDEX IF NOT EXISTS idx_excerpt_figures_figure ON excerpt_figures(figure_id);
CREATE TRIGGER IF NOT EXISTS excerpts_follow_paper AFTER DELETE ON papers BEGIN
    DELETE FROM excerpts WHERE paper_id = old.id;
END;
CREATE TRIGGER IF NOT EXISTS excerpt_figures_follow_excerpt AFTER DELETE ON excerpts BEGIN
    DELETE FROM excerpt_figures WHERE excerpt_id = old.id;
END;
CREATE TRIGGER IF NOT EXISTS excerpt_figures_follow_figure AFTER DELETE ON figures BEGIN
    DELETE FROM excerpt_figures WHERE figure_id = old.id;
END;
"#;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Excerpt {
    /// Empty until it is first kept.
    pub id: String,
    /// `METHOD` or `INTRO`.
    pub kind: String,
    /// The paper it was taken from; empty for one the person wrote down.
    pub paper_id: String,
    /// The file its words were selected in; empty when unknown.
    pub resource_id: String,
    /// The 1-based page its words stand on; 0 when unknown.
    pub page: usize,
    /// What the person calls it: "Western blot", "从临床问题切入".
    pub name: String,
    /// The paper's words, as selected.
    pub text: String,
    /// The person's own: why it suits, which parameters were changed, what to borrow.
    pub notes: String,
    pub tags: Vec<String>,
    pub added_at: String,
    pub updated_at: String,
}

impl Default for Excerpt {
    fn default() -> Self {
        Excerpt { id: String::new(), kind: METHOD.into(), paper_id: String::new(), resource_id: String::new(), page: 0, name: String::new(), text: String::new(), notes: String::new(), tags: vec![], added_at: String::new(), updated_at: String::new() }
    }
}

impl Excerpt {
    /// What stands for it in a line: its name, else the start of its words.
    pub fn headline(&self) -> &str {
        if !self.name.trim().is_empty() { return self.name.trim() }
        self.text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("")
    }

    /// Whether every word is found in what it says or in the record of its paper.
    /// The words are lowercase already (`figure::search_words`).
    pub fn matches(&self, paper: Option<&Paper>, words: &[String]) -> bool {
        if words.is_empty() { return true }
        let mut hay = [self.name.as_str(), self.text.as_str(), self.notes.as_str()].join("\n");
        for tag in &self.tags { hay.push('\n'); hay.push_str(tag) }
        if let Some(paper) = paper { hay.push_str(&format!("\n{}\n{}\n{}\n{}", paper.title, paper.authors, paper.journal, paper.year.map(|y| y.to_string()).unwrap_or_default())) }
        let hay = hay.to_lowercase();
        words.iter().all(|word| hay.contains(word.as_str()))
    }
}

/// The tags of the excerpts of a kind with how many carry each, the commonest first.
pub fn tag_counts(excerpts: &[Excerpt], kind: &str) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = vec![];
    for tag in excerpts.iter().filter(|excerpt| excerpt.kind == kind).flat_map(|excerpt| &excerpt.tags) {
        match counts.iter_mut().find(|(known, _)| known == tag) {
            Some((_, count)) => *count += 1,
            None => counts.push((tag.clone(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts
}

const COLUMNS: &str = "id, kind, paper_id, resource_id, page, name, text, notes, tags_json, added_at, updated_at";

fn row_to_excerpt(r: &rusqlite::Row) -> rusqlite::Result<Excerpt> {
    Ok(Excerpt {
        id: r.get(0)?,
        kind: r.get(1)?,
        paper_id: r.get(2)?,
        resource_id: r.get(3)?,
        page: r.get::<_, i64>(4)?.max(0) as usize,
        name: r.get(5)?,
        text: r.get(6)?,
        notes: r.get(7)?,
        tags: serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or_default(),
        added_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

/// What a person may write about an excerpt, tidied: one line for the name, no blank ends, no tag twice.
fn tidy(excerpt: &mut Excerpt) -> Result<()> {
    if !KINDS.iter().any(|(kind, _)| *kind == excerpt.kind) { bail!("不认识的摘录类型：{}", excerpt.kind) }
    excerpt.name = excerpt.name.split_whitespace().collect::<Vec<_>>().join(" ");
    excerpt.text = excerpt.text.trim().to_string();
    excerpt.notes = excerpt.notes.trim().to_string();
    excerpt.tags = clean_strings(excerpt.tags.iter());
    if excerpt.name.is_empty() && excerpt.text.is_empty() { bail!("还没有名称，也没有原文。") }
    Ok(())
}

impl Store {
    /// Keep an excerpt of a paper, or, with no paper named, one the person wrote down. It gets its id and its dates.
    pub fn add_excerpt(&mut self, excerpt: &mut Excerpt) -> Result<()> {
        tidy(excerpt)?;
        if !excerpt.paper_id.is_empty() {
            let held: Option<String> = self.db.query_row("SELECT id FROM papers WHERE id = ?1", [&excerpt.paper_id], |r| r.get(0)).optional()?;
            if held.is_none() { bail!("文献不存在：{}", excerpt.paper_id) }
        }
        excerpt.id = new_id("E");
        excerpt.added_at = now_iso();
        excerpt.updated_at = excerpt.added_at.clone();
        self.db.execute(
            &format!("INSERT INTO excerpts({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"),
            params![excerpt.id, excerpt.kind, excerpt.paper_id, excerpt.resource_id, excerpt.page as i64, excerpt.name, excerpt.text, excerpt.notes, serde_json::to_string(&excerpt.tags)?, excerpt.added_at, excerpt.updated_at],
        )?;
        Ok(())
    }

    /// Keep what a person changed about an excerpt: its name, words, notes and tags. Its kind and where it came from stay.
    pub fn save_excerpt(&mut self, excerpt: &mut Excerpt) -> Result<()> {
        tidy(excerpt)?;
        excerpt.updated_at = now_iso();
        let changed = self.db.execute(
            "UPDATE excerpts SET name = ?2, text = ?3, notes = ?4, tags_json = ?5, updated_at = ?6 WHERE id = ?1",
            params![excerpt.id, excerpt.name, excerpt.text, excerpt.notes, serde_json::to_string(&excerpt.tags)?, excerpt.updated_at],
        )?;
        if changed == 0 { bail!("摘录不存在：{}", excerpt.id) }
        Ok(())
    }

    /// Take an excerpt out of the library, and its links to figures with it. It has no file to set aside.
    pub fn remove_excerpt(&mut self, id: &str) -> Result<()> {
        self.db.execute("DELETE FROM excerpts WHERE id = ?1", [id])?;
        Ok(())
    }

    /// The excerpts of a kind (`None`: of every kind) whose paper is outside the trash, and those written down
    /// without one, the newest first.
    pub fn list_excerpts(&self, kind: Option<&str>) -> Result<Vec<Excerpt>> {
        let mut stmt = self.db.prepare(&format!("SELECT {COLUMNS} FROM excerpts WHERE paper_id NOT IN (SELECT paper_id FROM trashed_papers) AND (?1 IS NULL OR kind = ?1) ORDER BY added_at DESC, id DESC"))?;
        let excerpts = stmt.query_map([kind], row_to_excerpt)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(excerpts)
    }

    pub fn get_excerpt(&self, id: &str) -> Result<Excerpt> {
        self.db.query_row(&format!("SELECT {COLUMNS} FROM excerpts WHERE id = ?1"), [id], row_to_excerpt).optional()?.ok_or_else(|| anyhow!("摘录不存在：{id}"))
    }

    /// The figures an excerpt is linked to are these, in this order: those before that are not among them are
    /// let go, and an id that is no figure of the library is passed over.
    pub fn set_excerpt_figures(&mut self, excerpt_id: &str, figures: &[String]) -> Result<()> {
        let held: Option<String> = self.db.query_row("SELECT id FROM excerpts WHERE id = ?1", [excerpt_id], |r| r.get(0)).optional()?;
        if held.is_none() { bail!("摘录不存在：{excerpt_id}") }
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM excerpt_figures WHERE excerpt_id = ?1", [excerpt_id])?;
        for (position, figure) in figures.iter().enumerate() {
            tx.execute("INSERT OR IGNORE INTO excerpt_figures(excerpt_id, figure_id, position) SELECT ?1, id, ?3 FROM figures WHERE id = ?2", params![excerpt_id, figure, position as i64])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The figures an excerpt is linked to, in the person's order.
    pub fn excerpt_figures(&self, excerpt_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.db.prepare("SELECT figure_id FROM excerpt_figures WHERE excerpt_id = ?1 ORDER BY position, figure_id")?;
        let ids = stmt.query_map([excerpt_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(ids)
    }

    /// The excerpts a figure is linked to, the newest first.
    pub fn figure_excerpts(&self, figure_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.db.prepare("SELECT l.excerpt_id FROM excerpt_figures l JOIN excerpts e ON e.id = l.excerpt_id WHERE l.figure_id = ?1 ORDER BY e.added_at DESC, e.id DESC")?;
        let ids = stmt.query_map([figure_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(ids)
    }

    /// Every link of the library, as (excerpt, figure), each excerpt's in the person's order: what the window keeps.
    pub fn excerpt_links(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.db.prepare("SELECT excerpt_id, figure_id FROM excerpt_figures ORDER BY excerpt_id, position, figure_id")?;
        let links = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(links)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::figure::{Figure, search_words};

    fn png() -> Vec<u8> {
        b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x10\x00\x00\x00\x10\x08\x02\x00\x00\x00".to_vec()
    }

    fn library(name: &str) -> (std::path::PathBuf, Store, Paper) {
        let root = std::env::temp_dir().join(format!("wisp-excerpt-{name}-{}", new_id("T")));
        std::fs::create_dir_all(&root).unwrap();
        let mut store = Store::in_memory(&root).unwrap();
        let paper = store.add_paper(Paper { title: "Auxin and the root stem cell niche".into(), authors: "Zhang, Wei; Li, Na".into(), year: Some(2021), ..Default::default() }).unwrap();
        (root, store, paper)
    }

    #[test]
    fn excerpts_of_both_kinds_are_kept_changed_and_removed() {
        let (root, mut store, paper) = library("kept");
        let mut blot = Excerpt { paper_id: paper.id.clone(), resource_id: "R-1".into(), page: 7, name: " Western  blot ".into(), text: "Proteins were separated by 10% SDS-PAGE.\n".into(), tags: vec!["蛋白".into(), " 蛋白 ".into(), "western".into()], ..Default::default() };
        store.add_excerpt(&mut blot).unwrap();
        assert_eq!((blot.kind.as_str(), blot.name.as_str(), blot.text.as_str(), blot.tags.len(), blot.id.starts_with("E-")), (METHOD, "Western blot", "Proteins were separated by 10% SDS-PAGE.", 2, true));
        assert_eq!(store.get_excerpt(&blot.id).unwrap(), blot);
        let mut opening = Excerpt { kind: INTRO.into(), paper_id: paper.id.clone(), page: 1, name: "从临床问题切入".into(), text: "Root growth limits crop yield under drought.".into(), ..Default::default() };
        store.add_excerpt(&mut opening).unwrap();

        // Each kind is listed apart, or both together.
        assert_eq!(store.list_excerpts(Some(METHOD)).unwrap(), vec![blot.clone()]);
        assert_eq!(store.list_excerpts(Some(INTRO)).unwrap(), vec![opening.clone()]);
        assert_eq!(store.list_excerpts(None).unwrap().len(), 2);

        // Nothing to call it by, a kind not known, or a paper that is not there: refused.
        assert!(store.add_excerpt(&mut Excerpt::default()).unwrap_err().to_string().contains("没有名称"));
        assert!(store.add_excerpt(&mut Excerpt { kind: "result".into(), name: "x".into(), ..Default::default() }).unwrap_err().to_string().contains("不认识"));
        assert!(store.add_excerpt(&mut Excerpt { paper_id: "nobody".into(), name: "x".into(), ..Default::default() }).unwrap_err().to_string().contains("文献不存在"));
        // One the person wrote down has no paper.
        let mut own = Excerpt { name: "我们的原生质体分离".into(), notes: "酶解时间改为 1.5 h".into(), ..Default::default() };
        store.add_excerpt(&mut own).unwrap();

        // Its words can be changed; its kind and where it came from cannot.
        let mut changed = Excerpt { notes: "转膜改成 90 min".into(), page: 1, paper_id: String::new(), kind: INTRO.into(), ..blot.clone() };
        store.save_excerpt(&mut changed).unwrap();
        let read = store.get_excerpt(&blot.id).unwrap();
        assert_eq!((read.notes.as_str(), read.page, read.paper_id.as_str(), read.kind.as_str()), ("转膜改成 90 min", 7, paper.id.as_str(), METHOD));

        // A paper in the trash takes its excerpts out of the list; restored, they are back. One without a paper stays.
        store.trash_paper(&paper.id).unwrap();
        assert_eq!(store.list_excerpts(None).unwrap(), vec![own.clone()]);
        store.restore_paper(&paper.id).unwrap();
        assert_eq!(store.list_excerpts(None).unwrap().len(), 3);

        store.remove_excerpt(&blot.id).unwrap();
        assert!(store.get_excerpt(&blot.id).is_err());
        assert!(store.save_excerpt(&mut changed).unwrap_err().to_string().contains("摘录不存在"));
        // A deleted paper takes its excerpts along, of both kinds, and only those.
        store.delete_paper(&paper.id).unwrap();
        assert_eq!(store.list_excerpts(None).unwrap(), vec![own]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn links_follow_an_excerpt_a_figure_and_a_paper() {
        let (root, mut store, paper) = library("links");
        let other = store.add_paper(Paper { title: "Cytokinin in the shoot".into(), ..Default::default() }).unwrap();
        let mut gel = Figure { paper_id: paper.id.clone(), label: "Fig. 2".into(), ..Default::default() };
        store.add_figure(&mut gel, &png()).unwrap();
        let mut bars = Figure { paper_id: other.id.clone(), label: "Fig. 5".into(), ..Default::default() };
        store.add_figure(&mut bars, &png()).unwrap();
        let mut blot = Excerpt { paper_id: paper.id.clone(), name: "Western blot".into(), ..Default::default() };
        store.add_excerpt(&mut blot).unwrap();
        let mut qpcr = Excerpt { paper_id: other.id.clone(), name: "qPCR".into(), ..Default::default() };
        store.add_excerpt(&mut qpcr).unwrap();

        // In the order given; what is no figure is passed over, a figure twice is there once.
        store.set_excerpt_figures(&blot.id, &[bars.id.clone(), "G-none".into(), gel.id.clone(), bars.id.clone()]).unwrap();
        assert_eq!(store.excerpt_figures(&blot.id).unwrap(), vec![bars.id.clone(), gel.id.clone()]);
        store.set_excerpt_figures(&qpcr.id, std::slice::from_ref(&bars.id)).unwrap();
        // Both were kept in the same instant, so which comes first is their ids' say: compare them as a set.
        let mut of_bars = store.figure_excerpts(&bars.id).unwrap();
        of_bars.sort();
        let mut both = vec![qpcr.id.clone(), blot.id.clone()];
        both.sort();
        assert_eq!(of_bars, both);
        assert_eq!(store.excerpt_links().unwrap().len(), 3);
        assert!(store.set_excerpt_figures("E-none", &[]).unwrap_err().to_string().contains("摘录不存在"));
        // The set is replaced, not added to.
        store.set_excerpt_figures(&blot.id, std::slice::from_ref(&gel.id)).unwrap();
        assert_eq!(store.excerpt_figures(&blot.id).unwrap(), vec![gel.id.clone()]);

        // A figure taken out takes its links; so does an excerpt.
        store.remove_figure(&bars.id).unwrap();
        assert!(store.excerpt_figures(&qpcr.id).unwrap().is_empty());
        store.set_excerpt_figures(&qpcr.id, std::slice::from_ref(&gel.id)).unwrap();
        store.remove_excerpt(&qpcr.id).unwrap();
        assert_eq!(store.figure_excerpts(&gel.id).unwrap(), vec![blot.id.clone()]);
        // A deleted paper takes its excerpts, its figures, and every link of either.
        store.delete_paper(&paper.id).unwrap();
        assert!(store.excerpt_links().unwrap().is_empty() && store.list_excerpts(None).unwrap().is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn excerpts_are_found_by_their_words_and_their_paper() {
        let paper = Paper { title: "Auxin and the root stem cell niche".into(), authors: "Zhang, Wei".into(), year: Some(2021), journal: "Plant Cell".into(), ..Default::default() };
        let method = Excerpt { name: "Western blot".into(), text: "Proteins were transferred to PVDF membranes.".into(), notes: "转膜时间可以缩短".into(), tags: vec!["蛋白".into(), "western".into()], ..Default::default() };
        for (query, found) in [("", true), ("western pvdf", true), ("蛋白", true), ("转膜", true), ("zhang 2021", true), ("plant cell", true), ("elisa", false), ("western elisa", false)] {
            assert_eq!(method.matches(Some(&paper), &search_words(query)), found, "{query}");
        }
        assert!(!method.matches(None, &search_words("zhang")));
        let intro = Excerpt { kind: INTRO.into(), name: "从产量缺口切入".into(), text: "Drought costs a fifth of the yield.".into(), tags: vec!["western".into(), "缺口".into()], ..Default::default() };
        assert!(intro.matches(Some(&paper), &search_words("drought 缺口 niche")));
        let other = Excerpt { tags: vec!["western".into(), "qPCR".into()], ..Default::default() };
        // Tags are counted within a kind.
        let all = [method.clone(), other, intro.clone()];
        assert_eq!(tag_counts(&all, METHOD), vec![("western".to_string(), 2), ("qPCR".to_string(), 1), ("蛋白".to_string(), 1)]);
        assert_eq!(tag_counts(&all, INTRO), vec![("western".to_string(), 1), ("缺口".to_string(), 1)]);
        assert_eq!((method.headline(), Excerpt { text: "\n  Cells were lysed.\nMore.".into(), ..Default::default() }.headline()), ("Western blot", "Cells were lysed."));
    }
}
