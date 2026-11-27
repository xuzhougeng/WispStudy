//! 移到文献库 and 复制到文献库: a paper taken from one library (or a conversation, a draft) into another, with what is
//! its own there: its files (copied, each once by its bytes), their text, parts and the place they were read to, the
//! highlights, boxes, notes and pieces marked on them, its conversations, the figures cut from it and the
//! methods and introductions kept from it, with their links to those figures. Its custom fields go to the fields of the same name
//! and kind; the rest are named in the answer. Its screening decision does not go: it was made against the
//! other library's goal, and the paper waits here. Moved, the library it came from keeps it in its trash, so nothing
//! is lost by a move; copied, that library keeps it as it was.
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
    for table in ["fulltext_text", "structures", "sentences", "file_origins", "reading_places", "ocr_pages"] {
        let (names, found) = rows(from, table, "resource_id", old)?;
        put(tx, table, &names, found, |name, value| if name == "resource_id" { *value = Value::Text(new.into()) })?;
    }
    Ok(())
}

impl Store {
    /// The paper here that is `paper` (by DOI, PMID or title, or holding one of `files` by bytes); errs when it is in
    /// the trash, saying it is to be restored before the paper is moved here, or copied (`keep`).
    fn held_here(&self, paper: &Paper, file_names: &[String], files: &[Vec<Value>], keep: bool) -> Result<Option<String>> {
        let mut held = existing_paper(&self.db, paper)?;
        for file in files {
            if held.is_some() { break }
            let sha = text(file_names, file, "sha256");
            if sha.is_empty() { continue }
            held = self.db.query_row("SELECT paper_id FROM resources WHERE sha256 = ?1 ORDER BY added_at, id LIMIT 1", [sha], |r| r.get(0)).optional()?;
        }
        if let Some(held) = &held && self.in_trash(held)? {
            if keep { bail!("「{}」在这个文献库的回收站里：先把它恢复，再复制过来。", paper.title) }
            bail!("「{}」在这个文献库的回收站里：先把它恢复，再移过来。", paper.title)
        }
        Ok(held)
    }

    /// Take the papers `ids` of `from` into this library, in order (移到文献库). Checked beforehand so a paper in this
    /// library's trash or a missing file stops the whole lot before any paper moves.
    pub fn take_papers(&mut self, from: &mut Store, ids: &[String]) -> Result<Vec<Taken>> {
        let ordered = self.ready_to_take(from, ids, false)?;
        ordered.iter().map(|id| self.take_paper(from, id)).collect()
    }

    /// Copy the papers `ids` of `from` into this library, in order (复制到文献库): what `take_papers` brings, while
    /// `from` keeps them as they were.
    pub fn copy_papers(&mut self, from: &Store, ids: &[String]) -> Result<Vec<Taken>> {
        let ordered = self.ready_to_take(from, ids, true)?;
        ordered.iter().map(|id| self.bring_paper(from, id, true)).collect()
    }

    /// `ids` once each, in order, after checking that none is in this library's trash and that every file to copy is
    /// there: one that is not stops the whole lot before any paper is taken.
    fn ready_to_take(&self, from: &Store, ids: &[String], keep: bool) -> Result<Vec<String>> {
        if from.root() == self.root() { bail!("这篇文献已经在这个文献库里。") }
        let mut ordered = vec![];
        for id in ids {
            if !ordered.contains(id) { ordered.push(id.clone()) }
        }
        for id in &ordered {
            let paper = from.get_paper(id)?;
            let (file_names, files) = rows(&from.db, "resources", "paper_id", id)?;
            let held = self.held_here(&paper, &file_names, &files, keep)?;
            for file in &files {
                let sha = text(&file_names, file, "sha256");
                if let (Some(held), false) = (&held, sha.is_empty()) && self.db.query_row("SELECT 1 FROM resources WHERE paper_id = ?1 AND sha256 = ?2", params![held, sha], |_| Ok(())).optional()?.is_some() { continue }
                let source = from.root().join(text(&file_names, file, "local_path").replace('/', std::path::MAIN_SEPARATOR_STR));
                std::fs::metadata(&source).map_err(|err| anyhow!("无法复制文件「{}」：{err}", text(&file_names, file, "title")))?;
            }
        }
        Ok(ordered)
    }

    /// Take the paper `id` of `from` into this library (see the module). `from` keeps it in its trash.
    pub fn take_paper(&mut self, from: &mut Store, id: &str) -> Result<Taken> {
        let taken = self.bring_paper(from, id, false)?;
        from.trash_paper(id)?;
        Ok(taken)
    }

    /// Write the paper `id` of `from` into this library with what is its own there (see the module), `from` left as
    /// it is. `keep` (a copy) only changes what is said when the paper is in this library's trash.
    fn bring_paper(&mut self, from: &Store, id: &str, keep: bool) -> Result<Taken> {
        if from.root() == self.root() { bail!("这篇文献已经在这个文献库里。") }
        let paper = from.get_paper(id)?;
        let (file_names, files) = rows(&from.db, "resources", "paper_id", id)?;
        // The paper here: the same record, or one holding one of the same files.
        let held = self.held_here(&paper, &file_names, &files, keep)?;
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
        // The pictures of the figures cut from it, copied the same way; one whose picture is gone stays behind, and
        // one this library has already (the paper copied here before) is not copied again.
        let (figure_names, figures) = rows(&from.db, "figures", "paper_id", id)?;
        let mut pictures: Vec<Vec<Value>> = vec![];
        for figure in figures {
            if self.db.query_row("SELECT 1 FROM figures WHERE id = ?1", [text(&figure_names, &figure, "id")], |_| Ok(())).optional()?.is_some() { continue }
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
        for table in ["pdf_annotations", "pdf_shapes", "pdf_cuts", "pdf_texts"] {
            let (names, found) = rows(&from.db, table, "paper_id", id)?;
            taken.marks += put(&tx, table, &names, found, to_paper)?;
        }
        put(&tx, "figures", &figure_names, pictures, to_paper)?;
        // Its methods and introductions, and their links to the figures this library holds now; a link to one it does
        // not stays behind.
        let (excerpt_names, excerpts) = rows(&from.db, "excerpts", "paper_id", id)?;
        let excerpt_ids: Vec<String> = excerpts.iter().map(|row| text(&excerpt_names, row, "id")).collect();
        put(&tx, "excerpts", &excerpt_names, excerpts, to_paper)?;
        for excerpt in excerpt_ids {
            let (names, links) = rows(&from.db, "excerpt_figures", "excerpt_id", &excerpt)?;
            let held = |row: &Vec<Value>| tx.query_row("SELECT 1 FROM figures WHERE id = ?1", [text(&names, row, "figure_id")], |_| Ok(())).optional().ok().flatten().is_some();
            let links: Vec<Vec<Value>> = links.into_iter().filter(held).collect();
            put(&tx, "excerpt_figures", &names, links, |_, _| {})?;
        }
        let (names, conversations) = rows(&from.db, "conversations", "paper_id", id)?;
        let conversation_ids: Vec<String> = conversations.iter().map(|row| text(&names, row, "id")).collect();
        put(&tx, "conversations", &names, conversations, to_paper)?;
        for conversation in conversation_ids {
            let (names, images) = rows(&from.db, "conversation_images", "conversation_id", &conversation)?;
            put(&tx, "conversation_images", &names, images, |_, _| {})?;
        }
        tx.commit()?;
        Ok(taken)
    }

    /// 绑定到文献库: take everything a conversation or a manuscript begun on the home page holds (`from`, a library
    /// not on the shelf) into this library. Its papers come as `take_paper` brings them, one it already holds
    /// joined, and a paper in its trash that a manuscript cites comes too, since the reference list names it. Its
    /// manuscripts come with their sections and earlier versions, the file each began from, their comments and
    /// the passages their citations rest on; its conversations, the manuscripts' among them. A citation keeps the
    /// key it was written with where that key is free here or is the same paper's; where another paper has it,
    /// the citation is written with the key the paper has here (`TakenAll::rekeyed`). A picture a manuscript
    /// shows is kept in this library's figures, once by its bytes, and the line that shows it points there.
    /// `from` keeps what it had, out of sight: the papers in its trash, the manuscripts put away. Nothing is
    /// taken while one of the papers lies in this library's trash, or one of the manuscripts is here already.
    pub fn take_all(&mut self, from: &mut Store) -> Result<TakenAll> {
        if from.root() == self.root() { bail!("它已经在这个文献库里。") }
        let manuscripts = from.manuscripts()?;
        for manuscript in &manuscripts {
            if self.db.query_row("SELECT 1 FROM manuscripts WHERE id = ?1", [&manuscript.id], |_| Ok(())).optional()?.is_some() { bail!("稿件「{}」已经在这个文献库里。", if manuscript.title.is_empty() { crate::manuscript::UNTITLED } else { &manuscript.title }) }
        }
        // What the manuscripts cite there, read while the papers are where the keys were given.
        let (open, trash, kept, figures) = (from.list_papers()?, from.list_trash()?, from.cite_keys()?, from.list_figures()?);
        let index = crate::cite::Index::new(&open, trash.iter().map(|entry| &entry.paper), &kept);
        let mut cited: Vec<(String, String)> = vec![];
        for body in manuscripts.iter().flat_map(|manuscript| &manuscript.sections).map(|section| section.body.as_str()) {
            for one in crate::cite::find(body, &index).into_iter().filter(|one| !one.old) {
                let (key, Some(paper)) = (one.written.trim_start_matches('@'), one.found.paper()) else { continue };
                if !cited.iter().any(|(known, _)| known.eq_ignore_ascii_case(key)) { cited.push((key.to_string(), paper.to_string())) }
            }
        }
        let wanted: Vec<&Paper> = open.iter().chain(trash.iter().map(|entry| &entry.paper).filter(|paper| cited.iter().any(|(_, id)| *id == paper.id))).collect();
        for paper in &wanted {
            if let Some(held) = existing_paper(&self.db, paper)? && self.in_trash(&held)? { bail!("「{}」在这个文献库的回收站里：先把它恢复，再绑定。", paper.title) }
        }
        let mut taken = TakenAll::default();
        let mut ids: HashMap<String, String> = HashMap::new();
        let brought = (|| -> Result<()> {
            for paper in &wanted {
                // One cited from the trash is a paper again while it is taken.
                if from.in_trash(&paper.id)? { from.restore_paper(&paper.id)? }
                let one = self.take_paper(from, &paper.id)?;
                taken.papers += 1;
                taken.joined += usize::from(one.joined);
                for field in one.fields_left {
                    if !taken.fields_left.contains(&field) { taken.fields_left.push(field) }
                }
                ids.insert(paper.id.clone(), one.paper);
            }
            // The keys: each citation's paper is the same paper here, by the key it was written with or the one it has here.
            let mut renamed: HashMap<String, String> = HashMap::new();
            for (key, paper) in &cited {
                let Some(here) = ids.get(paper) else { continue };
                let (ours, our_trash, our_kept) = (self.list_papers()?, self.list_trash()?, self.cite_keys()?);
                let our_index = crate::cite::Index::new(&ours, our_trash.iter().map(|entry| &entry.paper), &our_kept);
                let now = match our_index.holder(key) {
                    Some(holder) if holder != here => our_index.key(here).unwrap_or(key).to_string(),
                    _ => key.clone(),
                };
                self.keep_cite_key(here, &now)?;
                if now != *key {
                    renamed.insert(key.to_lowercase(), now.clone());
                    taken.rekeyed.push((key.clone(), now));
                }
            }
            // The pictures the manuscripts show: this library's own from here on.
            let mut addresses: HashMap<String, String> = HashMap::new();
            for address in manuscripts.iter().flat_map(crate::manuscript::pictures_used) {
                if addresses.contains_key(&address) { continue }
                let Ok(bytes) = std::fs::read(from.root().join(address.replace('/', std::path::MAIN_SEPARATOR_STR))) else { continue };
                let held = match self.figure_holding(&bytes)? {
                    Some(held) => held.local_path,
                    None => {
                        let mut figure = match figures.iter().find(|figure| figure.local_path == address) {
                            Some(theirs) => crate::figure::Figure { id: String::new(), paper_id: ids.get(&theirs.paper_id).cloned().unwrap_or_default(), resource_id: String::new(), ..theirs.clone() },
                            None => crate::figure::Figure { origin: crate::figure::UPLOADED.into(), ..Default::default() },
                        };
                        self.add_figure(&mut figure, &bytes)?;
                        figure.local_path
                    }
                };
                addresses.insert(address, held);
            }
            // A text as it reads here: its keys and the addresses of its pictures.
            let rewrite = |written: &str| -> String {
                let mut edits: Vec<(std::ops::Range<usize>, &str)> = vec![];
                for one in crate::cite::find(written, &index).into_iter().filter(|one| !one.old) {
                    if let Some(now) = renamed.get(&one.written.trim_start_matches('@').to_lowercase()) { edits.push((one.range.start + 1..one.range.end, now.as_str())) }
                }
                for picture in crate::manuscript::picture_lines(written) {
                    if let Some(now) = addresses.get(&picture.address).filter(|now| **now != picture.address) { edits.push((picture.place, now.as_str())) }
                }
                edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
                let mut out = written.to_string();
                for (range, now) in edits { out.replace_range(range, now) }
                out
            };
            let rewritten = |names: &[&str]| {
                let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
                move |name: &str, value: &mut Value| {
                    if names.iter().any(|known| known == name) && let Value::Text(written) = value { *written = rewrite(written) }
                }
            };
            // A paper joined to one held here has that one's id: what a conversation says of it follows.
            let joined: Vec<(&String, &String)> = ids.iter().filter(|(old, new)| old != new).collect();
            let said = |name: &str, value: &mut Value| {
                if name == "body" && let Value::Text(body) = value {
                    for (old, new) in &joined { *body = body.replace(old.as_str(), new) }
                }
            };
            // The Word files the manuscripts began from, copied before anything is written.
            let mut sources = vec![];
            for manuscript in &manuscripts {
                let (names, found) = rows(&from.db, "manuscript_sources", "manuscript_id", &manuscript.id)?;
                for row in &found {
                    let path = text(&names, row, "path");
                    if path.is_empty() { continue }
                    let (source, target) = (from.root().join(path.replace('/', std::path::MAIN_SEPARATOR_STR)), self.root().join(path.replace('/', std::path::MAIN_SEPARATOR_STR)));
                    if let Some(folder) = target.parent() { std::fs::create_dir_all(folder)? }
                    // A file that is gone leaves the manuscript one made here: its words are in the sections.
                    if source.is_file() { std::fs::copy(&source, &target).map_err(|err| anyhow!("无法复制稿件的 Word 文件：{err}"))?; }
                }
                sources.push((names, found));
            }
            let tx = self.db.transaction()?;
            let mut conversations: Vec<String> = vec![];
            let mut talks = |tx: &Transaction, about: &str| -> Result<()> {
                let (names, found) = rows(&from.db, "conversations", "paper_id", about)?;
                conversations.extend(found.iter().map(|row| text(&names, row, "id")));
                put(tx, "conversations", &names, found, said)?;
                Ok(())
            };
            talks(&tx, "")?;
            for (manuscript, (source_names, source_rows)) in manuscripts.iter().zip(sources) {
                let (names, found) = rows(&from.db, "manuscripts", "id", &manuscript.id)?;
                put(&tx, "manuscripts", &names, found, |_, _| {})?;
                let (names, found) = rows(&from.db, "manuscript_sections", "manuscript_id", &manuscript.id)?;
                put(&tx, "manuscript_sections", &names, found, rewritten(&["body"]))?;
                for section in &manuscript.sections {
                    // An earlier version's number is each library's own.
                    let (mut names, mut found) = rows(&from.db, "section_snapshots", "section_id", &section.id)?;
                    if let Some(at) = names.iter().position(|name| name == "id") {
                        names.remove(at);
                        for row in &mut found { row.remove(at); }
                    }
                    put(&tx, "section_snapshots", &names, found, rewritten(&["body"]))?;
                }
                put(&tx, "manuscript_sources", &source_names, source_rows, |_, _| {})?;
                let (names, found) = rows(&from.db, "manuscript_comments", "manuscript_id", &manuscript.id)?;
                put(&tx, "manuscript_comments", &names, found, rewritten(&["quote", "suggestion", "replaced"]))?;
                // A passage of a paper that did not come has nothing to stand on here.
                let (names, found) = rows(&from.db, "cite_passages", "manuscript_id", &manuscript.id)?;
                let found: Vec<Vec<Value>> = found.into_iter().filter(|row| ids.contains_key(&text(&names, row, "paper_id"))).collect();
                put(&tx, "cite_passages", &names, found, |name, value| {
                    if name == "paper_id" && let Value::Text(old) = value && let Some(new) = ids.get(old.as_str()) { *value = Value::Text(new.clone()) }
                })?;
                talks(&tx, &manuscript.id)?;
            }
            for conversation in &conversations {
                let (names, images) = rows(&from.db, "conversation_images", "conversation_id", conversation)?;
                put(&tx, "conversation_images", &names, images, |_, _| {})?;
                // A review lives with its conversation.
                let (names, reviews) = rows(&from.db, "review_conversations", "conversation_id", conversation)?;
                for review in reviews.iter().map(|row| text(&names, row, "review_id")) {
                    let (names, found) = rows(&from.db, "review_projects", "id", &review)?;
                    put(&tx, "review_projects", &names, found, said)?;
                }
                put(&tx, "review_conversations", &names, reviews, |_, _| {})?;
            }
            tx.commit()?;
            taken.conversations = conversations.len();
            Ok(())
        })();
        if let Err(err) = brought {
            // What was not brought whole stays where it was: the papers taken so far are there again, and here too.
            for old in ids.keys() { let _ = from.restore_paper(old); }
            return Err(err);
        }
        for manuscript in &manuscripts { from.remove_manuscript(&manuscript.id)? }
        taken.manuscripts = manuscripts.into_iter().map(|manuscript| manuscript.id).collect();
        Ok(taken)
    }
}

/// What came of taking in everything a conversation or a manuscript of the home page held (`Store::take_all`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TakenAll {
    /// The papers brought, and how many of them were joined to papers this library already held.
    pub papers: usize,
    pub joined: usize,
    /// The manuscripts brought, by their ids, which they keep.
    pub manuscripts: Vec<String>,
    /// The conversations brought: its own, and those about its manuscripts.
    pub conversations: usize,
    /// The citation keys another paper has here, each with the key its citations are written with now.
    pub rekeyed: Vec<(String, String)>,
    /// Custom fields with values that this library has no field of the same name and kind for.
    pub fields_left: Vec<String>,
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
        store.db.execute("INSERT INTO pdf_texts(id, paper_id, resource_id, page, rect_json, note, fill, border, ink, created_at, updated_at) VALUES ('T-1', ?1, ?2, 1, '[0,0,40,20]', '根毛', 'yellow', 'none', 'black', 'now', 'now')", params![paper.id, file.id]).unwrap();
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
        assert_eq!((taken.files, taken.marks, taken.fields_left.clone()), (1, 3, vec!["物种".to_string()]));
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
        // A text box goes with its file, its words and colours as they were.
        let written: (String, String) = to.db.query_row("SELECT note, fill FROM pdf_texts WHERE resource_id = ?1 AND paper_id = ?2", params![files[0].id, taken.paper], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(written, ("根毛".to_string(), "yellow".to_string()));
        let pages: String = to.db.query_row("SELECT pages FROM fulltext_text WHERE resource_id = ?1", [&files[0].id], |r| r.get(0)).unwrap();
        assert!(pages.contains("Root hairs grow."));
        assert_eq!(to.list_conversations(&taken.paper).unwrap().len(), 1);
        // The other library keeps it, in its trash.
        assert!(from.in_trash(&id).unwrap());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_paper_moves_with_its_excerpts_and_their_figures() {
        let dir = temp_dir("methods");
        let (mut from, id) = source(&dir);
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x10\x00\x00\x00\x10\x08\x02\x00\x00\x00";
        let mut blot = crate::figure::Figure { paper_id: id.clone(), label: "Fig. 2".into(), ..Default::default() };
        from.add_figure(&mut blot, png).unwrap();
        let mut drawn = crate::figure::Figure::default();
        from.add_figure(&mut drawn, png).unwrap();
        let mut method = crate::excerpt::Excerpt { paper_id: id.clone(), name: "Western blot".into(), text: "Proteins were separated.".into(), ..Default::default() };
        from.add_excerpt(&mut method).unwrap();
        from.set_excerpt_figures(&method.id, &[blot.id.clone(), drawn.id.clone()]).unwrap();
        let mut intro = crate::excerpt::Excerpt { kind: crate::excerpt::INTRO.into(), paper_id: id.clone(), name: "从根毛切入".into(), ..Default::default() };
        from.add_excerpt(&mut intro).unwrap();
        let mut to = Store::open(&dir.join("to")).unwrap();
        let taken = to.take_paper(&mut from, &id).unwrap();
        // The method is the paper's here; its link to the paper's figure came along, the one to a picture of the other library did not.
        let moved = to.get_excerpt(&method.id).unwrap();
        assert_eq!((moved.paper_id.as_str(), moved.text.as_str()), (taken.paper.as_str(), "Proteins were separated."));
        assert_eq!(to.excerpt_figures(&method.id).unwrap(), vec![blot.id.clone()]);
        assert_eq!(to.get_excerpt(&intro.id).unwrap().kind, crate::excerpt::INTRO);
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

    /// A conversation of the home page that holds: a paper with a file, cited; one this library has too, cited; one in
    /// its trash, cited; one that is not cited; a manuscript citing the three with a picture, an earlier version, a
    /// comment and a passage; and two conversations, its own and the manuscript's.
    fn writing(dir: &Path) -> (Store, crate::manuscript::Manuscript, [String; 4]) {
        let mut store = Store::open(&dir.join("from")).unwrap();
        let zhang = store.add_paper(Paper { title: "Root hairs under salt".into(), authors: "Zhang, Wei; Li, Na".into(), year: Some(2021), doi: "10.1234/salt".into(), ..Default::default() }).unwrap();
        let pdf = dir.join("Zhang 2021.pdf");
        std::fs::write(&pdf, b"%PDF-1.4 salt").unwrap();
        store.import_file(&zhang.id, &pdf, "fulltext").unwrap();
        let li = store.add_paper(Paper { title: "Ethylene and the root".into(), authors: "Li, Na".into(), year: Some(2019), doi: "10.1234/ethylene".into(), ..Default::default() }).unwrap();
        let wang = store.add_paper(Paper { title: "Auxin transport".into(), authors: "Wang, Fang".into(), year: Some(2020), ..Default::default() }).unwrap();
        let other = store.add_paper(Paper { title: "Never cited".into(), authors: "Okada, K".into(), year: Some(2018), ..Default::default() }).unwrap();
        for (paper, key) in [(&zhang, "Zhang2021"), (&li, "Li2019"), (&wang, "Wang2020")] { store.keep_cite_key(&paper.id, key).unwrap() }
        store.trash_paper(&wang.id).unwrap();
        let mut drawn = crate::figure::Figure { legend: "A root tip".into(), ..Default::default() };
        store.add_figure(&mut drawn, b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x10\x00\x00\x00\x10\x08\x02\x00\x00\x00").unwrap();
        let made = store.create_piece("盐与根毛", "zh").unwrap();
        let section = made.sections[0].id.clone();
        store.save_section_body(&section, "早先的一版，引了 [@Zhang2021]。这一版写得足够长，值得留下来作为一个版本。").unwrap();
        assert!(store.take_snapshot(&section, "pause").unwrap());
        store.save_section_body(&section, &format!("根毛在盐胁迫下变短 [@Zhang2021; @li2019]，生长素也参与 [@Wang2020]，还有没收进来的 [@Nobody1999]。\n\n![根尖]({})", drawn.local_path)).unwrap();
        store.add_comment(&made.id, &section, "变短 [@Zhang2021; @li2019]", "这里要不要再引一篇？", "我", crate::comment::PERSON).unwrap();
        store.db.execute("INSERT INTO cite_passages(id, manuscript_id, section_id, paper_id, claim, passage, origin, created_at) VALUES ('Q-1', ?1, ?2, ?3, '根毛在盐胁迫下变短', 'Root hairs were shorter.', 'cited', 'now')", params![made.id, section, zhang.id]).unwrap();
        store.db.execute("INSERT INTO conversations(id, paper_id, title, questions, body, created_at, updated_at) VALUES ('C-own', '', '找几篇文献', 1, ?1, 'now', 'now')", [format!("{{\"sources\":[\"{}\",\"{}\"]}}", zhang.id, li.id)]).unwrap();
        store.db.execute("INSERT INTO conversations(id, paper_id, title, questions, created_at, updated_at) VALUES ('C-write', ?1, '写第一段', 1, 'now', 'now')", [&made.id]).unwrap();
        store.db.execute("INSERT INTO conversation_images(conversation_id, id, media_type, data) VALUES ('C-write', 'I-1', 'image/png', x'00')", []).unwrap();
        let made = store.manuscript(&made.id).unwrap().unwrap();
        (store, made, [zhang.id, li.id, wang.id, other.id])
    }

    #[test]
    fn everything_a_writing_holds_is_taken_into_a_library() {
        let dir = temp_dir("all");
        let (mut from, made, [zhang, li, wang, other]) = writing(&dir);
        let mut to = Store::open(&dir.join("to")).unwrap();
        // Here already: another Zhang of 2021, cited here by the key, and the same paper of Li's.
        let earlier = to.add_paper(Paper { title: "Salt stress in rice".into(), authors: "Zhang, Min".into(), year: Some(2021), ..Default::default() }).unwrap();
        to.keep_cite_key(&earlier.id, "Zhang2021").unwrap();
        let held = to.add_paper(Paper { title: "Ethylene and the root".into(), authors: "Li, Na".into(), year: Some(2019), doi: "10.1234/ethylene".into(), ..Default::default() }).unwrap();
        let taken = to.take_all(&mut from).unwrap();
        assert_eq!((taken.papers, taken.joined, taken.manuscripts.clone(), taken.conversations), (4, 1, vec![made.id.clone()], 2));
        assert_eq!(taken.rekeyed, [("Zhang2021".to_string(), "Zhang2021b".to_string())]);
        // The papers: the cited one from the trash too, a paper again; Li's joined to the one held.
        let papers = to.list_papers().unwrap();
        assert_eq!(papers.len(), 5);
        let id_of = |title: &str| papers.iter().find(|paper| paper.title == title).map(|paper| paper.id.clone()).unwrap();
        let (zhang_here, wang_here) = (id_of("Root hairs under salt"), id_of("Auxin transport"));
        assert_eq!((id_of("Ethylene and the root"), id_of("Never cited")), (held.id.clone(), other.clone()));
        assert_eq!(to.paper_files(&zhang_here).unwrap().len(), 1);
        // The manuscript, with its id: the key another paper has here is written anew, the others stay, and so does one no paper has.
        let here = to.manuscript(&made.id).unwrap().unwrap();
        assert_eq!((here.title.as_str(), here.language.as_str(), here.sections.len(), here.sections[0].bare, here.sections[0].id == made.sections[0].id), ("盐与根毛", "zh", 1, true, true));
        let body = &here.sections[0].body;
        assert!(body.starts_with("根毛在盐胁迫下变短 [@Zhang2021b; @li2019]，生长素也参与 [@Wang2020]，还有没收进来的 [@Nobody1999]。"), "{body}");
        let kept = to.cite_keys().unwrap();
        let index = crate::cite::Index::new(&papers, [], &kept);
        let names: Vec<Option<String>> = crate::cite::find(body, &index).iter().map(|cited| cited.found.paper().map(String::from)).collect();
        assert_eq!(names, [Some(zhang_here.clone()), Some(held.id.clone()), Some(wang_here.clone()), None]);
        assert_eq!(index.key(&earlier.id), Some("Zhang2021"));
        // Its picture is this library's, and the line shows it from here.
        let pictures = crate::manuscript::pictures_used(&here);
        let figures = to.list_figures().unwrap();
        assert_eq!((pictures.len(), figures.len(), figures[0].legend.as_str()), (1, 1, "A root tip"));
        assert_eq!(pictures[0], figures[0].local_path);
        assert!(to.root().join(&pictures[0]).is_file());
        // The earlier version, the comment and the passage read as the text does now.
        let versions = to.snapshots(&here.sections[0].id).unwrap();
        assert_eq!(versions.len(), 1);
        assert!(versions[0].body.contains("[@Zhang2021b]"));
        let comments = to.comments(&made.id).unwrap();
        assert_eq!((comments.len(), comments[0].quote.as_str()), (1, "变短 [@Zhang2021b; @li2019]"));
        let passages = to.passages(&made.id).unwrap();
        assert_eq!((passages.len(), passages[0].paper_id.as_str(), passages[0].passage.as_str()), (1, zhang_here.as_str(), "Root hairs were shorter."));
        // The conversations: its own is the library's, the manuscript's stays the manuscript's; a paper joined is named by the id it has here.
        assert_eq!((to.list_conversations("").unwrap().len(), to.list_conversations(&made.id).unwrap().len()), (1, 1));
        let said: String = to.db.query_row("SELECT body FROM conversations WHERE id = 'C-own'", [], |r| r.get(0)).unwrap();
        assert!(said.contains(&held.id) && !said.contains(&li) && said.contains(&zhang), "{said}");
        let images: i64 = to.db.query_row("SELECT count(*) FROM conversation_images WHERE conversation_id = 'C-write'", [], |r| r.get(0)).unwrap();
        assert_eq!(images, 1);
        // Where it came from keeps it all, out of sight.
        assert!(from.manuscripts().unwrap().is_empty() && from.list_papers().unwrap().is_empty());
        assert_eq!(from.list_trash().unwrap().len(), 4);
        assert!(from.in_trash(&wang).unwrap());
        // Nothing is taken twice.
        assert!(to.take_all(&mut from).unwrap().manuscripts.is_empty());
        assert_eq!(to.manuscripts().unwrap().len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn nothing_is_taken_past_a_paper_in_the_trash_here() {
        let dir = temp_dir("all-trashed");
        let (mut from, made, [zhang, ..]) = writing(&dir);
        let mut to = Store::open(&dir.join("to")).unwrap();
        let here = to.add_paper(Paper { title: "Root hairs under salt".into(), doi: "10.1234/salt".into(), ..Default::default() }).unwrap();
        to.trash_paper(&here.id).unwrap();
        assert!(to.take_all(&mut from).unwrap_err().to_string().contains("回收站"));
        // As it was, on both sides.
        assert_eq!((from.manuscripts().unwrap().len(), from.list_papers().unwrap().len(), from.in_trash(&zhang).unwrap()), (1, 3, false));
        assert!(to.manuscript(&made.id).unwrap().is_none() && to.list_papers().unwrap().is_empty() && to.list_conversations("").unwrap().is_empty());
        // A manuscript with nothing but words: an article bound to a library it cites nothing of.
        let mut plain = Store::open(&dir.join("plain")).unwrap();
        let piece = plain.create_piece("", "zh").unwrap();
        plain.save_section_body(&piece.sections[0].id, "写给公众号的一篇。").unwrap();
        to.restore_paper(&here.id).unwrap();
        let taken = to.take_all(&mut plain).unwrap();
        assert_eq!((taken.papers, taken.manuscripts.len(), taken.conversations, taken.rekeyed.len()), (0, 1, 0, 0));
        assert_eq!(to.manuscript(&piece.id).unwrap().unwrap().sections[0].body, "写给公众号的一篇。");
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

    #[test]
    fn several_papers_move_together_or_none_when_one_is_trashed_here() {
        let dir = temp_dir("batch");
        let (mut from, first) = source(&dir);
        let second = from.add_paper(Paper { title: "Auxin transport in roots".into(), doi: "10.1234/auxin".into(), tags: vec!["生长素".into()], ..Default::default() }).unwrap();
        let mut to = Store::open(&dir.join("to")).unwrap();
        let trashed = to.add_paper(Paper { title: "Auxin transport in roots".into(), doi: "10.1234/auxin".into(), ..Default::default() }).unwrap();
        to.trash_paper(&trashed.id).unwrap();
        // The second is in the target's trash: the first does not move either.
        assert!(to.take_papers(&mut from, &[first.clone(), second.id.clone()]).unwrap_err().to_string().contains("回收站"));
        assert!(!from.in_trash(&first).unwrap() && !from.in_trash(&second.id).unwrap());
        assert!(to.list_papers().unwrap().is_empty());

        to.restore_paper(&trashed.id).unwrap();
        let taken = to.take_papers(&mut from, &[first.clone(), second.id.clone(), first.clone()]).unwrap();
        assert_eq!(taken.len(), 2);
        assert_eq!((taken[0].joined, taken[1].joined), (false, true));
        assert_eq!(to.list_papers().unwrap().len(), 2);
        assert!(from.in_trash(&first).unwrap() && from.in_trash(&second.id).unwrap());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_paper_copied_stays_where_it_was() {
        let dir = temp_dir("copy");
        let (mut from, id) = source(&dir);
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x10\x00\x00\x00\x10\x08\x02\x00\x00\x00";
        let mut blot = crate::figure::Figure { paper_id: id.clone(), label: "Fig. 2".into(), ..Default::default() };
        from.add_figure(&mut blot, png).unwrap();
        let mut to = Store::open(&dir.join("to")).unwrap();
        let taken = to.copy_papers(&from, std::slice::from_ref(&id)).unwrap();
        assert_eq!(taken.len(), 1);
        assert_eq!((taken[0].joined, taken[0].files, taken[0].marks), (false, 1, 3));
        // Here as a move brings it: its file, its conversation, its figure, and its own decision still to make.
        let here = to.get_paper(&taken[0].paper).unwrap();
        assert_eq!((here.screening.as_str(), here.notes.as_str()), ("pending", "读过第二节"));
        let files = to.paper_files(&here.id).unwrap();
        assert_eq!(std::fs::read(to.root().join(&files[0].local_path)).unwrap(), b"%PDF-1.4 root hairs");
        assert_eq!((to.list_conversations(&here.id).unwrap().len(), to.list_figures().unwrap().len()), (1, 1));
        // There, nothing changed: not in the trash, the decision, the file, the conversation as they were.
        assert!(!from.in_trash(&id).unwrap());
        let there = from.get_paper(&id).unwrap();
        assert_eq!(there.screening, "included");
        let theirs = from.paper_files(&id).unwrap();
        assert!(from.root().join(&theirs[0].local_path).is_file());
        assert_eq!((from.list_conversations(&id).unwrap().len(), from.list_figures().unwrap().len()), (1, 1));
        // Copied a second time: the paper here takes nothing twice, not even the figure's picture.
        let again = to.copy_papers(&from, std::slice::from_ref(&id)).unwrap();
        assert_eq!((again[0].joined, again[0].files, again[0].marks), (true, 0, 0));
        assert_eq!((to.list_papers().unwrap().len(), to.paper_files(&here.id).unwrap().len(), to.list_figures().unwrap().len()), (1, 1, 1));
        assert_eq!(std::fs::read_dir(to.root().join("figures")).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_paper_in_the_trash_here_is_not_copied() {
        let dir = temp_dir("copy-trashed");
        let (from, id) = source(&dir);
        let mut to = Store::open(&dir.join("to")).unwrap();
        let here = to.add_paper(Paper { title: "Root hairs and ethylene".into(), doi: "10.1234/root".into(), ..Default::default() }).unwrap();
        to.trash_paper(&here.id).unwrap();
        let refused = to.copy_papers(&from, std::slice::from_ref(&id)).unwrap_err().to_string();
        assert!(refused.contains("再复制过来"), "{refused}");
        assert_eq!(to.paper_files(&here.id).unwrap().len(), 0);
        std::fs::remove_dir_all(dir).ok();
    }
}
