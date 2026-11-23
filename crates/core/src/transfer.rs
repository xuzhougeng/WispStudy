//! 移到文献库: a paper taken from one library (or a conversation, a draft) into another, with what is its own
//! there: its files (copied, each once by its bytes), their text, parts and the place they were read to, the
//! highlights, boxes, notes and pieces marked on them, its conversations and the figures cut from it. Its
//! custom fields go to the fields of the same name and kind; the rest are named in the answer. Its screening
//! decision does not go: it was made against the other library's goal, and the paper waits here.
//! The library it came from keeps it in its trash, so nothing is lost by a move.
//! A paper this library already holds (the same DOI, PMID or title, or one of the same files) is not added
//! twice: what the moved one brings is joined to it, blanks filled, tags and notes added.

use crate::model::*;
use crate::store::{Store, copy_hashed, existing_paper, write_paper};
use anyhow::{Result, anyhow, bail};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::HashMap;

/// What came of a move.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Taken {
    /// The paper here: the moved one, or the one it was joined to.
    pub paper: String,
    /// It was joined to a paper this library already held.
    pub joined: bool,
    /// Files copied here (a file the paper here already had is not counted).
    pub files: usize,
    /// Highlights, boxes, notes and marked pieces brought along.
    pub marks: usize,
    /// Custom fields with values that this library has no field of the same name and kind for.
    pub fields_left: Vec<String>,
}

/// The rows of `table` whose `column` is `key`: the names of the columns and each row's values.
fn rows(db: &Connection, table: &str, column: &str, key: &str) -> Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut statement = db.prepare(&format!("SELECT * FROM {table} WHERE {column} = ?1"))?;
    let names: Vec<String> = statement.column_names().into_iter().map(String::from).collect();
    let count = names.len();
    let rows = statement.query_map([key], |r| (0..count).map(|i| r.get::<_, Value>(i)).collect::<rusqlite::Result<Vec<_>>>())?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((names, rows))
}

/// The value of a column of a row read by `rows`, as text.
fn text(names: &[String], row: &[Value], column: &str) -> String {
    match names.iter().position(|name| name == column).map(|at| &row[at]) {
        Some(Value::Text(value)) => value.clone(),
        Some(Value::Integer(value)) => value.to_string(),
        _ => String::new(),
    }
}

/// Write rows into `table`, each changed first by `change` (a column's name, its value); a row whose key is
/// already there stays as it is. How many were written.
fn put(tx: &Transaction, table: &str, names: &[String], rows: Vec<Vec<Value>>, change: impl Fn(&str, &mut Value)) -> Result<usize> {
    let marks: Vec<String> = (1..=names.len()).map(|i| format!("?{i}")).collect();
    let sql = format!("INSERT OR IGNORE INTO {table}({}) VALUES ({})", names.join(", "), marks.join(", "));
    let mut written = 0;
    for mut row in rows {
        for (name, value) in names.iter().zip(row.iter_mut()) { change(name, value) }
        written += tx.execute(&sql, rusqlite::params_from_iter(row))?;
    }
    Ok(written)
}

/// The rows of each table a file's id keys, from one library to another under the file's new id.
fn put_by_file(from: &Connection, tx: &Transaction, old: &str, new: &str) -> Result<()> {
    for table in ["fulltext_text", "structures", "sentences", "file_origins", "reading_places"] {
        let (names, found) = rows(from, table, "resource_id", old)?;
        put(tx, table, &names, found, |name, value| if name == "resource_id" { *value = Value::Text(new.into()) })?;
    }
    Ok(())
}

impl Store {
    /// Take the paper `id` of `from` into this library (see the module). `from` keeps it in its trash.
    pub fn take_paper(&mut self, from: &mut Store, id: &str) -> Result<Taken> {
        if from.root() == self.root() { bail!("这篇文献已经在这个文献库里。") }
        let paper = from.get_paper(id)?;
        let (file_names, files) = rows(&from.db, "resources", "paper_id", id)?;
        // The paper here: the same record, or one holding one of the same files.
        let mut held = existing_paper(&self.db, &paper)?;
        for file in &files {
            if held.is_some() { break }
            let sha = text(&file_names, file, "sha256");
            if sha.is_empty() { continue }
            held = self.db.query_row("SELECT paper_id FROM resources WHERE sha256 = ?1 ORDER BY added_at, id LIMIT 1", [sha], |r| r.get(0)).optional()?;
        }
        if let Some(held) = &held && self.in_trash(held)? { bail!("「{}」在这个文献库的回收站里：先把它恢复，再移过来。", paper.title) }
        let mut target = match &held {
            Some(held) => {
                let here = self.get_paper(held)?;
                let tags = clean_strings(here.tags.iter().chain(&paper.tags));
                let notes = match (here.notes.trim(), paper.notes.trim()) {
                    (_, "") => here.notes.clone(),
                    ("", theirs) => theirs.to_string(),
                    (ours, theirs) if ours.contains(theirs) => here.notes.clone(),
                    (ours, theirs) => format!("{ours}\n\n{theirs}"),
                };
                let rank = |reading: &str| READINGS.iter().position(|(key, _)| *key == reading).unwrap_or(0);
                let reading = if rank(&paper.reading) > rank(&here.reading) { paper.reading.clone() } else { here.reading.clone() };
                let starred = here.starred || paper.starred;
                let mut joined = crate::lookup::merge_lookup(here, paper.clone());
                (joined.tags, joined.notes, joined.reading, joined.starred) = (tags, notes, reading, starred);
                joined
            }
            None => {
                let mut moved = paper.clone();
                (moved.screening, moved.screening_reason, moved.reviewed_at) = ("pending".into(), String::new(), String::new());
                moved.fields.clear();
                moved.updated_at = crate::time::now_iso();
                if self.db.query_row("SELECT 1 FROM papers WHERE id = ?1", [&moved.id], |_| Ok(())).optional()?.is_some() { moved.id = new_id("P") }
                moved
            }
        };
        target.normalize();
        // The files are copied before anything is written: one that cannot be leaves the library as it was
        // (a copy already made stays in papers/, named by no record).
        let mut file_ids: HashMap<String, String> = HashMap::new();
        let mut copied: Vec<(String, Vec<Value>)> = vec![];
        for file in files {
            let (old, sha) = (text(&file_names, &file, "id"), text(&file_names, &file, "sha256"));
            let same: Option<String> = if sha.is_empty() { None } else { self.db.query_row("SELECT id FROM resources WHERE paper_id = ?1 AND sha256 = ?2", params![target.id, sha], |r| r.get(0)).optional()? };
            if let Some(same) = same {
                file_ids.insert(old, same);
                continue;
            }
            let new = new_id("R");
            let format = text(&file_names, &file, "format");
            let relative = if format.is_empty() { format!("papers/{new}") } else { format!("papers/{new}.{format}") };
            let source = from.root().join(text(&file_names, &file, "local_path").replace('/', std::path::MAIN_SEPARATOR_STR));
            std::fs::create_dir_all(self.root().join("papers"))?;
            copy_hashed(&source, &self.root().join(&relative)).map_err(|err| anyhow!("无法复制文件「{}」：{err}", text(&file_names, &file, "title")))?;
            let mut row = file;
            for (name, value) in file_names.iter().zip(row.iter_mut()) {
                match name.as_str() {
                    "id" => *value = Value::Text(new.clone()),
                    "paper_id" => *value = Value::Text(target.id.clone()),
                    "local_path" => *value = Value::Text(relative.clone()),
                    _ => {}
                }
            }
            file_ids.insert(old.clone(), new);
            copied.push((old, row));
        }
        // The pictures of the figures cut from it, copied the same way; one whose picture is gone stays behind.
        let (figure_names, figures) = rows(&from.db, "figures", "paper_id", id)?;
        let mut pictures: Vec<Vec<Value>> = vec![];
        for figure in figures {
            let path = text(&figure_names, &figure, "local_path");
            let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let relative = format!("figures/{}-{name}", new_id("G"));
            std::fs::create_dir_all(self.root().join("figures"))?;
            if std::fs::copy(from.root().join(path.replace('/', std::path::MAIN_SEPARATOR_STR)), self.root().join(&relative)).is_err() { continue }
            let mut row = figure;
            for (column, value) in figure_names.iter().zip(row.iter_mut()) {
                if column == "local_path" { *value = Value::Text(relative.clone()) }
            }
            pictures.push(row);
        }
        // Custom fields by name and kind: the ids are each library's own.
        let (field_names, field_rows) = rows(&from.db, "paper_fields", "paper_id", id)?;
        let theirs: HashMap<String, (String, String)> = from.list_fields()?.into_iter().map(|f| (f.id, (f.name, f.kind.as_str().to_string()))).collect();
        let ours: HashMap<(String, String), String> = self.list_fields()?.into_iter().map(|f| ((f.name, f.kind.as_str().to_string()), f.id)).collect();
        let mut fields_left = vec![];
        let mut kept_fields = vec![];
        for row in field_rows {
            let Some(field) = theirs.get(&text(&field_names, &row, "field_id")) else { continue };
            match ours.get(field) {
                Some(here) => kept_fields.push((here.clone(), row)),
                None => fields_left.push(field.0.clone()),
            }
        }
        let paper_id = target.id.clone();
        let to_paper = |name: &str, value: &mut Value| {
            if name == "paper_id" { *value = Value::Text(paper_id.clone()) }
            if name == "resource_id" && let Value::Text(old) = value && let Some(new) = file_ids.get(old.as_str()) { *value = Value::Text(new.clone()) }
        };
        let tx = self.db.transaction()?;
        write_paper(&tx, &target)?;
        let mut taken = Taken { paper: target.id.clone(), joined: held.is_some(), files: copied.len(), fields_left, ..Default::default() };
        for (old, row) in copied {
            put(&tx, "resources", &file_names, vec![row], |_, _| {})?;
            put_by_file(&from.db, &tx, &old, &file_ids[&old])?;
        }
        for (field, row) in kept_fields {
            put(&tx, "paper_fields", &field_names, vec![row], |name, value| match name {
                "paper_id" => *value = Value::Text(target.id.clone()),
                "field_id" => *value = Value::Text(field.clone()),
                _ => {}
            })?;
        }
        for table in ["pdf_annotations", "pdf_shapes", "pdf_cuts"] {
            let (names, found) = rows(&from.db, table, "paper_id", id)?;
            taken.marks += put(&tx, table, &names, found, to_paper)?;
        }
        put(&tx, "figures", &figure_names, pictures, to_paper)?;
        let (names, conversations) = rows(&from.db, "conversations", "paper_id", id)?;
        let conversation_ids: Vec<String> = conversations.iter().map(|row| text(&names, row, "id")).collect();
        put(&tx, "conversations", &names, conversations, to_paper)?;
        for conversation in conversation_ids {
            let (names, images) = rows(&from.db, "conversation_images", "conversation_id", &conversation)?;
            put(&tx, "conversation_images", &names, images, |_, _| {})?;
        }
        tx.commit()?;
        from.trash_paper(id)?;
        Ok(taken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wisp-test-{name}-{}", new_id("T")));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A library with one paper holding one file, its text, a highlight, a box, a place read to, a conversation and two fields.
    fn source(dir: &Path) -> (Store, String) {
        let mut store = Store::open(&dir.join("from")).unwrap();
        let mut field = FieldDef { name: "方法".into(), kind: FieldKind::Text, ..Default::default() };
        store.save_field(&mut field).unwrap();
        let mut other = FieldDef { name: "物种".into(), kind: FieldKind::Text, ..Default::default() };
        store.save_field(&mut other).unwrap();
        let mut paper = store.add_paper(Paper { title: "Root hairs and ethylene".into(), doi: "10.1234/root".into(), tags: vec!["根毛".into()], notes: "读过第二节".into(), reading: "read".into(), screening: "included".into(), ..Default::default() }).unwrap();
        paper.fields.insert(field.id.clone(), vec!["单细胞测序".into()]);
        paper.fields.insert(other.id.clone(), vec!["拟南芥".into()]);
        store.save_paper(&mut paper).unwrap();
        let pdf = dir.join("Smith 2020.pdf");
        std::fs::write(&pdf, b"%PDF-1.4 root hairs").unwrap();
        let file = store.import_file(&paper.id, &pdf, "fulltext").unwrap();
        store.save_fulltext_pages(&file.id, &Ok(vec!["Root hairs grow.".into()])).unwrap();
        store.save_reading_places(&[(file.id.clone(), (3, 120.))]).unwrap();
        store.db.execute("INSERT INTO pdf_annotations(id, paper_id, resource_id, page, kind, quote, boxes_json, created_at, updated_at) VALUES ('A-1', ?1, ?2, 1, 'highlight', 'Root hairs grow', '[]', 'now', 'now')", params![paper.id, file.id]).unwrap();
        store.db.execute("INSERT INTO pdf_shapes(id, paper_id, resource_id, page, kind, rect_json, created_at, updated_at) VALUES ('S-1', ?1, ?2, 1, 'box', '[0,0,1,1]', 'now', 'now')", params![paper.id, file.id]).unwrap();
        store.db.execute("INSERT INTO conversations(id, paper_id, title, questions, created_at, updated_at) VALUES ('C-1', ?1, '方法是什么', 1, 'now', 'now')", [&paper.id]).unwrap();
        (store, paper.id)
    }

    #[test]
    fn a_paper_moves_with_what_is_its_own() {
        let dir = temp_dir("take");
        let (mut from, id) = source(&dir);
        let mut to = Store::open(&dir.join("to")).unwrap();
        let mut field = FieldDef { name: "方法".into(), kind: FieldKind::Text, ..Default::default() };
        to.save_field(&mut field).unwrap();
        let taken = to.take_paper(&mut from, &id).unwrap();
        assert!(!taken.joined);
        assert_eq!((taken.files, taken.marks, taken.fields_left.clone()), (1, 2, vec!["物种".to_string()]));
        let paper = to.get_paper(&taken.paper).unwrap();
        // The decision was the other library's; the rest is the paper's own.
        assert_eq!((paper.screening.as_str(), paper.reading.as_str(), paper.notes.as_str(), paper.doi.as_str()), ("pending", "read", "读过第二节", "10.1234/root"));
        assert_eq!(paper.fields.get(&field.id), Some(&vec!["单细胞测序".to_string()]));
        let files = to.paper_files(&taken.paper).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].title, "Smith 2020.pdf");
        assert_eq!(std::fs::read(to.root().join(&files[0].local_path)).unwrap(), b"%PDF-1.4 root hairs");
        assert_eq!(to.reading_places().unwrap().get(&files[0].id), Some(&(3, 120.)));
        let on_file: i64 = to.db.query_row("SELECT count(*) FROM pdf_annotations WHERE resource_id = ?1 AND paper_id = ?2", params![files[0].id, taken.paper], |r| r.get(0)).unwrap();
        assert_eq!(on_file, 1);
        let pages: String = to.db.query_row("SELECT pages FROM fulltext_text WHERE resource_id = ?1", [&files[0].id], |r| r.get(0)).unwrap();
        assert!(pages.contains("Root hairs grow."));
        assert_eq!(to.list_conversations(&taken.paper).unwrap().len(), 1);
        // The other library keeps it, in its trash.
        assert!(from.in_trash(&id).unwrap());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_paper_held_already_is_joined_not_doubled() {
        let dir = temp_dir("join");
        let (mut from, id) = source(&dir);
        let mut to = Store::open(&dir.join("to")).unwrap();
        let here = to.add_paper(Paper { title: "Root hairs and ethylene".into(), doi: "10.1234/root".into(), journal: "Plant Cell".into(), tags: vec!["乙烯".into()], ..Default::default() }).unwrap();
        let taken = to.take_paper(&mut from, &id).unwrap();
        assert!(taken.joined);
        assert_eq!(taken.paper, here.id);
        assert_eq!(to.list_papers().unwrap().len(), 1);
        let paper = to.get_paper(&here.id).unwrap();
        assert_eq!((paper.journal.as_str(), paper.tags.clone(), paper.notes.as_str(), paper.reading.as_str()), ("Plant Cell", vec!["乙烯".to_string(), "根毛".to_string()], "读过第二节", "read"));
        assert_eq!(to.paper_files(&here.id).unwrap().len(), 1);
        // Moved back and forth: the same file is not attached twice, nor its marks.
        from.restore_paper(&id).unwrap();
        let again = to.take_paper(&mut from, &id).unwrap();
        assert_eq!((again.files, again.marks), (0, 0));
        assert_eq!(to.paper_files(&here.id).unwrap().len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_paper_in_the_trash_here_is_not_taken() {
        let dir = temp_dir("trashed");
        let (mut from, id) = source(&dir);
        let mut to = Store::open(&dir.join("to")).unwrap();
        let here = to.add_paper(Paper { title: "Root hairs and ethylene".into(), doi: "10.1234/root".into(), ..Default::default() }).unwrap();
        to.trash_paper(&here.id).unwrap();
        assert!(to.take_paper(&mut from, &id).unwrap_err().to_string().contains("回收站"));
        assert!(!from.in_trash(&id).unwrap());
        std::fs::remove_dir_all(dir).ok();
    }
}
