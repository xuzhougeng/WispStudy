//! 筛选记录: what the 筛选 page wrote, kept so that a screening can be taken
//! back later, not only while the page is open. A write is one confirmation
//! of the suggestions (`confirm`), or the decisions of one round of 逐篇
//! (`round`) or of one visit to the queue (`queue`); each of its lines is a
//! paper as it was before a decision and as the decision left it. The
//! decisions and their lines are written in one transaction
//! (`Store::record_screenings`), so the record never says what did not
//! happen. Taking a write back gives each paper what it was before the write,
//! but only where the paper is still as the write left it: a decision made
//! since, on the page or anywhere else, stays (`Store::undo_screening_write`).

use crate::model::{SCREENINGS, one_of};
use crate::screening::{Before, tally};
use crate::store::Store;
use crate::time::now_iso;
use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

/// The tables, made with the library's own (`store::SCHEMA`). An older build
/// leaves them alone; what it decides meanwhile is a decision made since, which
/// taking a write back leaves as it is.
pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS screening_writes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL CHECK (kind IN ('confirm', 'round', 'queue')),
    made_at TEXT NOT NULL,
    -- The library's screening goal when it was written.
    criteria TEXT NOT NULL DEFAULT '',
    -- When it was taken back; empty while it stands.
    undone_at TEXT NOT NULL DEFAULT ''
);

-- A paper of a write, in the order decided (a paper decided twice in a round
-- has two lines): what it was before the decision, and what it became.
CREATE TABLE IF NOT EXISTS screening_write_papers (
    write_id INTEGER NOT NULL REFERENCES screening_writes(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    paper_id TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    was_status TEXT NOT NULL,
    was_reason TEXT NOT NULL DEFAULT '',
    was_reviewed_at TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    reviewed_at TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (write_id, seq)
);
"#;

/// The kinds of write, with the name of the face of 筛选 each is made on.
pub const KINDS: [(&str, &str); 3] = [("confirm", "确认建议"), ("round", "逐篇"), ("queue", "批量")];

/// One decision: the paper as it was, and what it becomes.
#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    pub before: Before,
    pub status: String,
    pub reason: String,
    /// When it was decided; empty for a paper that waits again.
    pub reviewed_at: String,
}

impl Decision {
    pub fn paper_id(&self) -> &str {
        &self.before.paper_id
    }
}

/// A write as 筛选记录 lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScreeningWrite {
    pub id: i64,
    pub kind: String,
    pub made_at: String,
    pub criteria: String,
    /// When it was taken back; empty while it stands.
    pub undone_at: String,
    /// Its papers, each once, by what it made of them last, in the order of `SCREENINGS`.
    pub counts: [usize; 5],
    /// How many of them are still as it left them: those taking it back would change.
    pub standing: usize,
}

impl ScreeningWrite {
    pub fn papers(&self) -> usize {
        self.counts.iter().sum()
    }

    pub fn undone(&self) -> bool {
        !self.undone_at.is_empty()
    }
}

/// Give a paper its screening: the status, the reason kept with it and when it was decided.
pub(crate) fn set_screening(db: &Connection, id: &str, status: &str, reason: &str, reviewed_at: &str) -> Result<()> {
    if !one_of(&SCREENINGS, status) { bail!("不支持的收录状态：{status}") }
    let changed = db.execute("UPDATE papers SET screening_status = ?2, screening_reason = ?3, reviewed_at = ?4, updated_at = ?5 WHERE id = ?1", params![id, status, reason.trim(), reviewed_at, now_iso()])?;
    if changed == 0 { bail!("没有找到这篇文献：{id}") }
    Ok(())
}

/// Give the paper of a decision what it was before, if it is still as the decision left it. Whether it went back.
fn restore(db: &Connection, before: &Before, decision: &Decision) -> Result<bool> {
    let changed = db.execute(
        "UPDATE papers SET screening_status = ?2, screening_reason = ?3, reviewed_at = ?4, updated_at = ?5 WHERE id = ?1 AND screening_status = ?6 AND screening_reason = ?7 AND reviewed_at = ?8",
        params![before.paper_id, before.screening, before.reason.trim(), before.reviewed_at, now_iso(), decision.status, decision.reason.trim(), decision.reviewed_at],
    )?;
    Ok(changed > 0)
}

/// A line of a write as a decision.
fn line(r: &rusqlite::Row) -> rusqlite::Result<Decision> {
    Ok(Decision { before: Before { paper_id: r.get(0)?, screening: r.get(1)?, reason: r.get(2)?, reviewed_at: r.get(3)? }, status: r.get(4)?, reason: r.get(5)?, reviewed_at: r.get(6)? })
}

const LINE: &str = "paper_id, was_status, was_reason, was_reviewed_at, status, reason, reviewed_at";

impl Store {
    /// Write decisions of 筛选 and keep them in its record, in one
    /// transaction: all of them or none. They go onto `open` while it is a
    /// write that stands, else into a new write of `kind` under the goal
    /// `criteria`; the write they went into is returned.
    pub fn record_screenings(&mut self, open: Option<i64>, kind: &str, criteria: &str, decisions: &[Decision]) -> Result<i64> {
        if !KINDS.iter().any(|(key, _)| *key == kind) { bail!("不支持的筛选记录：{kind}") }
        let tx = self.db.transaction()?;
        let standing = match open {
            Some(id) => tx.query_row("SELECT id FROM screening_writes WHERE id = ?1 AND undone_at = ''", [id], |r| r.get::<_, i64>(0)).optional()?,
            None => None,
        };
        let id = match standing {
            Some(id) => id,
            None => {
                tx.execute("INSERT INTO screening_writes(kind, made_at, criteria) VALUES (?1, ?2, ?3)", params![kind, now_iso(), criteria.trim()])?;
                tx.last_insert_rowid()
            }
        };
        let mut seq: i64 = tx.query_row("SELECT coalesce(max(seq), 0) FROM screening_write_papers WHERE write_id = ?1", [id], |r| r.get(0))?;
        for d in decisions {
            set_screening(&tx, d.paper_id(), &d.status, &d.reason, &d.reviewed_at)?;
            seq += 1;
            let b = &d.before;
            tx.execute(
                &format!("INSERT INTO screening_write_papers(write_id, seq, {LINE}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"),
                params![id, seq, b.paper_id, b.screening, b.reason.trim(), b.reviewed_at, d.status, d.reason.trim(), d.reviewed_at],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// 筛选记录, the latest first. A write with no paper left (each decision
    /// taken back, or its papers deleted) is not listed.
    pub fn screening_writes(&self) -> Result<Vec<ScreeningWrite>> {
        let mut stmt = self.db.prepare("SELECT id, kind, made_at, criteria, undone_at FROM screening_writes ORDER BY id DESC")?;
        let mut writes = stmt.query_map([], |r| Ok(ScreeningWrite { id: r.get(0)?, kind: r.get(1)?, made_at: r.get(2)?, criteria: r.get(3)?, undone_at: r.get(4)?, ..Default::default() }))?.collect::<rusqlite::Result<Vec<_>>>()?;
        // The last line of each paper in each write, and whether the paper is still as that line left it.
        let mut last: HashMap<(i64, String), (String, bool)> = HashMap::new();
        let mut stmt = self.db.prepare("SELECT l.write_id, l.paper_id, l.status, p.screening_status = l.status AND p.screening_reason = l.reason AND p.reviewed_at = l.reviewed_at FROM screening_write_papers l JOIN papers p ON p.id = l.paper_id ORDER BY l.write_id, l.seq")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, bool>(3)?)))? {
            let (write, paper, status, still) = row?;
            last.insert((write, paper), (status, still));
        }
        let mut by_write: HashMap<i64, Vec<(String, bool)>> = HashMap::new();
        for ((write, _), line) in last { by_write.entry(write).or_default().push(line) }
        for write in &mut writes {
            let lines = by_write.remove(&write.id).unwrap_or_default();
            write.counts = tally(lines.iter().map(|(status, _)| status.as_str()));
            write.standing = lines.iter().filter(|(_, still)| *still).count();
        }
        writes.retain(|write| write.papers() > 0);
        Ok(writes)
    }

    /// Whether 筛选记录 holds anything: whether the page has a record to offer.
    pub fn has_screening_record(&self) -> Result<bool> {
        Ok(self.db.query_row("SELECT EXISTS(SELECT 1 FROM screening_write_papers)", [], |r| r.get(0))?)
    }

    /// Take a write back, in one transaction: each of its papers still as
    /// the write left it gets what it was before the write; one decided
    /// otherwise since keeps that decision. The write is marked as taken back.
    /// How many papers went back, and how many stayed as they are.
    pub fn undo_screening_write(&mut self, id: i64) -> Result<(usize, usize)> {
        let tx = self.db.transaction()?;
        match tx.query_row("SELECT undone_at FROM screening_writes WHERE id = ?1", [id], |r| r.get::<_, String>(0)).optional()? {
            None => bail!("筛选记录里没有这一次。"),
            Some(at) if !at.is_empty() => bail!("这一次已经撤销过了。"),
            Some(_) => {}
        }
        // Each paper once: what it was before its first decision here, and what its last one made it.
        let mut papers: Vec<(Before, Decision)> = vec![];
        {
            let mut stmt = tx.prepare(&format!("SELECT {LINE} FROM screening_write_papers WHERE write_id = ?1 ORDER BY seq"))?;
            for decision in stmt.query_map([id], line)? {
                let decision = decision?;
                match papers.iter_mut().find(|(before, _)| before.paper_id == decision.before.paper_id) {
                    Some((_, last)) => *last = decision,
                    None => papers.push((decision.before.clone(), decision)),
                }
            }
        }
        let mut back = 0;
        for (before, last) in &papers {
            if restore(&tx, before, last)? { back += 1 }
        }
        tx.execute("UPDATE screening_writes SET undone_at = ?2 WHERE id = ?1", params![id, now_iso()])?;
        tx.commit()?;
        Ok((back, papers.len() - back))
    }

    /// The last decision of a write taken back: its line leaves the record,
    /// and its paper gets what it was before, if it is still as the decision
    /// left it. The decision, and whether the paper went back; none when the
    /// write has no decision left or was taken back as a whole.
    pub fn take_back_screening(&mut self, id: i64) -> Result<Option<(Decision, bool)>> {
        let tx = self.db.transaction()?;
        let found = tx.query_row(
            &format!("SELECT seq, {LINE} FROM screening_write_papers WHERE write_id = ?1 AND write_id IN (SELECT id FROM screening_writes WHERE undone_at = '') ORDER BY seq DESC LIMIT 1"),
            [id],
            |r| Ok((r.get::<_, i64>(0)?, Decision { before: Before { paper_id: r.get(1)?, screening: r.get(2)?, reason: r.get(3)?, reviewed_at: r.get(4)? }, status: r.get(5)?, reason: r.get(6)?, reviewed_at: r.get(7)? })),
        ).optional()?;
        let Some((seq, decision)) = found else { return Ok(None) };
        let back = restore(&tx, &decision.before, &decision)?;
        tx.execute("DELETE FROM screening_write_papers WHERE write_id = ?1 AND seq = ?2", params![id, seq])?;
        tx.commit()?;
        Ok(Some((decision, back)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Paper;

    fn store() -> Store {
        let dir = std::env::temp_dir().join(format!("wisp-test-record-{}", crate::model::new_id("T")));
        std::fs::create_dir_all(&dir).unwrap();
        Store::in_memory(&dir).unwrap()
    }

    fn add(store: &mut Store, title: &str) -> Paper {
        store.add_paper(Paper { title: title.into(), ..Default::default() }).unwrap()
    }

    fn decide(store: &Store, id: &str, status: &str, at: &str) -> Decision {
        Decision { before: Before::of(&store.get_paper(id).unwrap()), status: status.into(), reason: format!(" because {status} "), reviewed_at: at.into() }
    }

    fn status(store: &Store, id: &str) -> String {
        store.get_paper(id).unwrap().screening
    }

    #[test]
    fn decisions_are_written_with_their_record() {
        let mut store = store();
        let (a, b) = (add(&mut store, "First"), add(&mut store, "Second"));
        assert!(!store.has_screening_record().unwrap() && store.screening_writes().unwrap().is_empty());
        let lines = [decide(&store, &a.id, "included", "2026-11-12T00:00:00.000Z"), decide(&store, &b.id, "irrelevant", "2026-11-12T00:00:00.000Z")];
        let id = store.record_screenings(None, "confirm", " roots ", &lines).unwrap();
        let first = store.get_paper(&a.id).unwrap();
        assert_eq!((first.screening.as_str(), first.screening_reason.as_str(), first.reviewed_at.as_str(), status(&store, &b.id).as_str()), ("included", "because included", "2026-11-12T00:00:00.000Z", "irrelevant"));
        let writes = store.screening_writes().unwrap();
        assert!(store.has_screening_record().unwrap());
        assert_eq!(writes.len(), 1);
        assert_eq!((writes[0].id, writes[0].kind.as_str(), writes[0].criteria.as_str(), writes[0].counts, writes[0].standing, writes[0].undone()), (id, "confirm", "roots", [0, 1, 0, 0, 1], 2, false));
        // One line that cannot be written leaves the others unwritten, and the record without them.
        assert!(store.record_screenings(None, "queue", "", &[decide(&store, &a.id, "pending", ""), Decision { before: Before { paper_id: "nobody".into(), screening: "pending".into(), reason: String::new(), reviewed_at: String::new() }, ..decide(&store, &b.id, "included", "") }]).is_err());
        assert!(store.record_screenings(None, "queue", "", &[decide(&store, &a.id, "pending", ""), decide(&store, &b.id, "maybe", "")]).is_err());
        assert!(store.record_screenings(None, "elsewhere", "", &[decide(&store, &a.id, "pending", "")]).is_err());
        assert_eq!((status(&store, &a.id).as_str(), store.screening_writes().unwrap().len()), ("included", 1));
    }

    #[test]
    fn a_write_taken_back_leaves_what_was_decided_since() {
        let mut store = store();
        let (a, b, c) = (add(&mut store, "One"), add(&mut store, "Two"), add(&mut store, "Three"));
        let lines = [decide(&store, &a.id, "included", "t1"), decide(&store, &b.id, "included", "t1"), decide(&store, &c.id, "irrelevant", "t1")];
        let confirmed = store.record_screenings(None, "confirm", "", &lines).unwrap();
        // Later, one of them is decided otherwise in the queue.
        let queue = store.record_screenings(None, "queue", "", &[decide(&store, &b.id, "uncertain", "t2")]).unwrap();
        let writes = store.screening_writes().unwrap();
        assert_eq!(writes.iter().map(|w| (w.id, w.papers(), w.standing)).collect::<Vec<_>>(), [(queue, 1, 1), (confirmed, 3, 2)]);
        assert_eq!(store.undo_screening_write(confirmed).unwrap(), (2, 1));
        assert_eq!((status(&store, &a.id), status(&store, &b.id), status(&store, &c.id)), ("pending".to_string(), "uncertain".to_string(), "pending".to_string()));
        assert!(store.get_paper(&a.id).unwrap().reviewed_at.is_empty());
        // Once only; the write stays listed, as taken back.
        assert!(store.undo_screening_write(confirmed).is_err() && store.undo_screening_write(999).is_err());
        let writes = store.screening_writes().unwrap();
        assert!(writes[1].undone() && writes[1].standing == 0 && !writes[0].undone());
        // Taking the later one back gives the paper what the confirmation made it.
        assert_eq!(store.undo_screening_write(queue).unwrap(), (1, 0));
        assert_eq!(status(&store, &b.id), "included");
    }

    #[test]
    fn a_round_goes_into_one_write_and_its_decisions_come_back_one_by_one() {
        let mut store = store();
        let (a, b) = (add(&mut store, "One"), add(&mut store, "Two"));
        let round = store.record_screenings(None, "round", "", &[decide(&store, &a.id, "included", "t1")]).unwrap();
        assert_eq!(store.record_screenings(Some(round), "round", "", &[decide(&store, &b.id, "excluded", "t2")]).unwrap(), round);
        // The same paper decided again in the round: a line more, still one paper.
        assert_eq!(store.record_screenings(Some(round), "round", "", &[decide(&store, &a.id, "irrelevant", "t3")]).unwrap(), round);
        let writes = store.screening_writes().unwrap();
        assert_eq!((writes.len(), writes[0].counts, writes[0].standing), (1, [0, 0, 1, 0, 1], 2));
        // The last decision first, the paper back to what it was before it.
        let (decision, back) = store.take_back_screening(round).unwrap().unwrap();
        assert!(back && decision.paper_id() == a.id && decision.status == "irrelevant");
        assert_eq!(status(&store, &a.id), "included");
        // A paper decided otherwise since keeps that decision; its line still goes.
        store.record_screenings(None, "queue", "", &[decide(&store, &b.id, "uncertain", "t4")]).unwrap();
        let (decision, back) = store.take_back_screening(round).unwrap().unwrap();
        assert!(!back && decision.paper_id() == b.id && status(&store, &b.id) == "uncertain");
        // The whole round taken back: the first paper waits again, as before the round.
        assert_eq!(store.undo_screening_write(round).unwrap(), (1, 0));
        assert_eq!(status(&store, &a.id), "pending");
        assert!(store.take_back_screening(round).unwrap().is_none());
        // Onto a write taken back, decisions begin a new one.
        let again = store.record_screenings(Some(round), "round", "", &[decide(&store, &a.id, "included", "t5")]).unwrap();
        assert_ne!(again, round);
        // A write emptied one decision at a time is no longer listed.
        store.take_back_screening(again).unwrap();
        assert!(store.screening_writes().unwrap().iter().all(|w| w.id != again));
    }

    #[test]
    fn a_deleted_paper_leaves_the_record() {
        let mut store = store();
        let (a, b) = (add(&mut store, "One"), add(&mut store, "Two"));
        store.record_screenings(None, "confirm", "", &[decide(&store, &a.id, "irrelevant", "t1")]).unwrap();
        let kept = store.record_screenings(None, "confirm", "", &[decide(&store, &b.id, "irrelevant", "t1")]).unwrap();
        store.trash_paper(&a.id).unwrap();
        store.empty_trash().unwrap();
        assert_eq!(store.screening_writes().unwrap().iter().map(|w| w.id).collect::<Vec<_>>(), [kept]);
    }
}
