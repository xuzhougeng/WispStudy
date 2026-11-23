//! PDF reading marks kept in SQLite, separate from the source file. Geometry
//! uses page points from the top left, just like `pdftext::Mark`, so zoom and
//! raster cache eviction do not change an annotation's anchor.

use crate::model::new_id;
use crate::store::Store;
use crate::time::now_iso;
use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS pdf_annotations (
    id TEXT PRIMARY KEY NOT NULL,
    paper_id TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    resource_id TEXT NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    page INTEGER NOT NULL CHECK (page > 0),
    kind TEXT NOT NULL CHECK (kind IN ('underline', 'highlight')),
    quote TEXT NOT NULL CHECK (length(trim(quote)) > 0),
    boxes_json TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pdf_annotations_source ON pdf_annotations(resource_id, page);
CREATE INDEX IF NOT EXISTS idx_pdf_annotations_paper ON pdf_annotations(paper_id);
-- Marks placed on a page rather than on its text: a box drawn round a figure or a passage,
-- a sticky note anywhere. Beside the first table, whose kinds are fixed by its CHECK.
-- A box's `rect` is its four edges; a note's is the spot its icon stands on (left = right, top = bottom).
CREATE TABLE IF NOT EXISTS pdf_shapes (
    id TEXT PRIMARY KEY NOT NULL,
    paper_id TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    resource_id TEXT NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    page INTEGER NOT NULL CHECK (page > 0),
    kind TEXT NOT NULL CHECK (kind IN ('box', 'note')),
    rect_json TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    color TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pdf_shapes_source ON pdf_shapes(resource_id, page);
-- Pieces of a page marked to be cut out later (a figure for the slides, a table to ask about): only the
-- place is kept, the picture is taken from the page when the mark is used. A table of their own, since
-- the CHECK of `pdf_shapes` in libraries already made knows two kinds only.
CREATE TABLE IF NOT EXISTS pdf_cuts (
    id TEXT PRIMARY KEY NOT NULL,
    paper_id TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    resource_id TEXT NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    page INTEGER NOT NULL CHECK (page > 0),
    rect_json TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pdf_cuts_source ON pdf_cuts(resource_id, page);
-- Text boxes written on a page (`note` is what they say), each with the colours of its fill, its border and its
-- words (keys of `TEXT_FILLS`, `TEXT_BORDERS`, `TEXT_INKS`). A table of their own for the same reason as
-- `pdf_cuts`; a build from before them does not know it and leaves it be.
CREATE TABLE IF NOT EXISTS pdf_texts (
    id TEXT PRIMARY KEY NOT NULL,
    paper_id TEXT NOT NULL REFERENCES papers(id) ON DELETE CASCADE,
    resource_id TEXT NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    page INTEGER NOT NULL CHECK (page > 0),
    rect_json TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    fill TEXT NOT NULL DEFAULT '',
    border TEXT NOT NULL DEFAULT '',
    ink TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pdf_texts_source ON pdf_texts(resource_id, page);
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Underline,
    Highlight,
    /// A hollow box drawn round part of a page.
    Box,
    /// A sticky note at a spot of a page; its text is `note`.
    Note,
    /// A piece of a page marked to be cut out later (`pdf_cuts`): drawn as a dashed frame.
    Cut,
    /// A text box written on the page (`pdf_texts`), as on a slide: its text is `note`, its colours `style`.
    Text,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self { Self::Underline => "underline", Self::Highlight => "highlight", Self::Box => "box", Self::Note => "note", Self::Cut => "cut", Self::Text => "text" }
    }

    /// Placed on the page (`pdf_shapes`, `pdf_cuts`, `pdf_texts`), not on its text.
    pub fn shape(self) -> bool {
        matches!(self, Self::Box | Self::Note | Self::Cut | Self::Text)
    }
}

/// The colours a highlight or a box can have, stored by these keys; the first is what a new highlight gets.
/// A highlight of a build from before colours reads as the first.
pub const COLORS: &[&str] = &["yellow", "green", "blue", "pink", "purple", "red", "orange"];
/// What a new box is drawn in: it stands out from the highlights round it.
pub const BOX_COLOR: &str = "red";

/// The colours a text box's fill, border and words can have, by key; `none` leaves the fill or the border out.
/// The page is white under them whatever the window's theme, so a light fill and dark words read on it.
pub const TEXT_FILLS: &[&str] = &["none", "white", "yellow", "green", "blue", "pink", "purple", "orange"];
pub const TEXT_BORDERS: &[&str] = &["none", "black", "gray", "yellow", "green", "blue", "pink", "purple", "red", "orange"];
pub const TEXT_INKS: &[&str] = &["black", "gray", "red", "orange", "green", "blue", "purple"];

/// One of the three colours of a text box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextPart {
    Fill,
    Border,
    Ink,
}

impl TextPart {
    pub const ALL: [TextPart; 3] = [TextPart::Fill, TextPart::Border, TextPart::Ink];

    /// The keys this part can take.
    pub fn keys(self) -> &'static [&'static str] {
        match self { Self::Fill => TEXT_FILLS, Self::Border => TEXT_BORDERS, Self::Ink => TEXT_INKS }
    }
}

/// The colours of a text box: what it is filled with, its border and its words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextStyle {
    pub fill: String,
    pub border: String,
    pub ink: String,
}

impl Default for TextStyle {
    /// What the first text box of a file is drawn in: a pale yellow card with a yellow edge and black words,
    /// told apart from the page's own text at a glance.
    fn default() -> Self {
        TextStyle { fill: "yellow".into(), border: "yellow".into(), ink: "black".into() }
    }
}

impl TextStyle {
    pub fn get(&self, part: TextPart) -> &str {
        match part { TextPart::Fill => &self.fill, TextPart::Border => &self.border, TextPart::Ink => &self.ink }
    }

    /// The same with one part in another colour.
    pub fn with(&self, part: TextPart, key: &str) -> TextStyle {
        let mut style = self.clone();
        *match part { TextPart::Fill => &mut style.fill, TextPart::Border => &mut style.border, TextPart::Ink => &mut style.ink } = key.into();
        style
    }

    /// As kept: a key this build does not know (one of a later build, a blank) reads as the default's.
    fn read(fill: String, border: String, ink: String) -> TextStyle {
        let base = TextStyle::default();
        let known = |part: TextPart, key: String| if part.keys().contains(&key.as_str()) { key } else { base.get(part).to_string() };
        TextStyle { fill: known(TextPart::Fill, fill), border: known(TextPart::Border, border), ink: known(TextPart::Ink, ink) }
    }

    fn check(&self) -> Result<()> {
        for part in TextPart::ALL {
            if !part.keys().contains(&self.get(part)) { bail!("不支持的文本框颜色：{}", self.get(part)) }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotation {
    pub id: String,
    pub paper_id: String,
    pub resource_id: String,
    /// 1-based page in this attachment, not the printed page number.
    pub page: usize,
    pub kind: Kind,
    /// Original selected text, empty for a box or a note. The user's interpretation lives in `note`.
    pub quote: String,
    /// One rectangle per selected line: [left, top, right, bottom] in points. A box has its own edges
    /// as the one rectangle, a note the spot it stands on (left = right, top = bottom).
    pub boxes: Vec<[f32; 4]>,
    /// The person's note on the passage, or a sticky note's text.
    pub note: String,
    /// A key of `COLORS` for a highlight or a box; empty for an underline, a note, a marked piece and a text box.
    pub color: String,
    /// A text box's colours; `None` for every other mark.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<TextStyle>,
    pub created_at: String,
    pub updated_at: String,
}

impl Store {
    /// Visible annotations in reading order. Empty filters mean the whole
    /// library; trashed papers retain their marks but are not exposed here.
    pub fn pdf_annotations(&self, paper: &str, resource: &str) -> Result<Vec<Annotation>> {
        let mut stmt = self.db.prepare(
            "SELECT id, paper_id, resource_id, page, kind, quote, boxes_json, note, created_at, updated_at, color, fill, border, ink FROM (
               SELECT id, paper_id, resource_id, page, kind, quote, boxes_json, note, created_at, updated_at, color, '' AS fill, '' AS border, '' AS ink FROM pdf_annotations
               UNION ALL
               SELECT id, paper_id, resource_id, page, kind, '', '[' || rect_json || ']', note, created_at, updated_at, color, '', '', '' FROM pdf_shapes
               UNION ALL
               SELECT id, paper_id, resource_id, page, 'cut', '', '[' || rect_json || ']', note, created_at, updated_at, '', '', '', '' FROM pdf_cuts
               UNION ALL
               SELECT id, paper_id, resource_id, page, 'text', '', '[' || rect_json || ']', note, created_at, updated_at, '', fill, border, ink FROM pdf_texts)
             WHERE (?1 = '' OR paper_id = ?1) AND (?2 = '' OR resource_id = ?2)
             AND paper_id NOT IN (SELECT paper_id FROM trashed_papers)
             ORDER BY paper_id, resource_id, page, created_at, id",
        )?;
        let rows = stmt.query_map(params![paper, resource], |r| {
            let kind: String = r.get(4)?;
            let boxes: String = r.get(6)?;
            let color: String = r.get(10)?;
            let invalid = |i, e| rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e));
            let kind: Kind = serde_json::from_value(serde_json::Value::String(kind)).map_err(|e| invalid(4, e))?;
            let color = match kind {
                Kind::Underline | Kind::Note | Kind::Cut | Kind::Text => String::new(),
                Kind::Highlight | Kind::Box if COLORS.contains(&color.as_str()) => color,
                Kind::Highlight => COLORS[0].into(),
                Kind::Box => BOX_COLOR.into(),
            };
            let style = (kind == Kind::Text).then(|| Ok::<_, rusqlite::Error>(TextStyle::read(r.get(11)?, r.get(12)?, r.get(13)?))).transpose()?;
            Ok(Annotation {
                id: r.get(0)?, paper_id: r.get(1)?, resource_id: r.get(2)?, page: r.get::<_, i64>(3)? as usize, kind,
                quote: r.get(5)?, boxes: serde_json::from_str(&boxes).map_err(|e| invalid(6, e))?,
                note: r.get(7)?, color, style, created_at: r.get(8)?, updated_at: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn add_pdf_annotation(&mut self, paper: &str, resource: &str, page: usize, kind: Kind, quote: &str, boxes: &[[f32; 4]]) -> Result<Annotation> {
        let quote = quote.trim();
        if kind.shape() { bail!("方框和便签用 add_pdf_shape 保存。") }
        if page == 0 || page > i32::MAX as usize || quote.is_empty() || quote.len() > 100_000 { bail!("PDF 标注需要有效的页码和选中文字。") }
        if boxes.is_empty() || boxes.len() > 10_000 || boxes.iter().any(|b| b.iter().any(|n| !n.is_finite()) || b[2] <= b[0] || b[3] <= b[1]) { bail!("PDF 标注的位置无效。") }
        self.check_pdf(paper, resource)?;
        // A repeated click on the same action must not create duplicate marks.
        if let Some(existing) = self.pdf_annotations(paper, resource)?.into_iter().find(|a| a.page == page && a.kind == kind && a.quote == quote && a.boxes == boxes) { return Ok(existing) }
        let now = now_iso();
        let color = if kind == Kind::Highlight { COLORS[0] } else { "" };
        let annotation = Annotation { id: new_id("A"), paper_id: paper.into(), resource_id: resource.into(), page, kind, quote: quote.into(), boxes: boxes.to_vec(), note: String::new(), color: color.into(), style: None, created_at: now.clone(), updated_at: now };
        self.db.execute(
            "INSERT INTO pdf_annotations (id, paper_id, resource_id, page, kind, quote, boxes_json, note, color, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '', ?8, ?9, ?9)",
            params![annotation.id, paper, resource, page as i64, kind.key(), quote, serde_json::to_string(boxes)?, color, annotation.created_at],
        )?;
        Ok(annotation)
    }

    fn check_pdf(&self, paper: &str, resource: &str) -> Result<()> {
        let valid: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM resources WHERE id = ?1 AND paper_id = ?2 AND lower(format) = 'pdf'
             AND paper_id NOT IN (SELECT paper_id FROM trashed_papers))", params![resource, paper], |r| r.get(0),
        )?;
        if !valid { bail!("PDF 附件不存在或不属于这篇文献。") }
        Ok(())
    }

    /// A box drawn round part of a page (`rect` its edges), or a sticky note at a spot of it (`rect` that spot twice,
    /// `note` its text). A box is drawn in `BOX_COLOR` until another is chosen. A piece marked to be cut out
    /// (`Kind::Cut`) is kept once: the same place marked again is the mark there.
    pub fn add_pdf_shape(&mut self, paper: &str, resource: &str, page: usize, kind: Kind, rect: [f32; 4], note: &str) -> Result<Annotation> {
        if !kind.shape() { bail!("下划线和高亮用 add_pdf_annotation 保存。") }
        if kind == Kind::Text { bail!("文本框用 add_pdf_text 保存。") }
        if page == 0 || page > i32::MAX as usize { bail!("PDF 标注需要有效的页码。") }
        let rect = valid_rect(kind, rect)?;
        if note.len() > 100_000 { bail!("便签过长。") }
        self.check_pdf(paper, resource)?;
        let now = now_iso();
        let color = if kind == Kind::Box { BOX_COLOR } else { "" };
        if kind == Kind::Cut && let Some(existing) = self.pdf_annotations(paper, resource)?.into_iter().find(|a| a.kind == kind && a.page == page && a.boxes == [rect]) { return Ok(existing) }
        let shape = Annotation { id: new_id("A"), paper_id: paper.into(), resource_id: resource.into(), page, kind, quote: String::new(), boxes: vec![rect], note: note.trim().into(), color: color.into(), style: None, created_at: now.clone(), updated_at: now };
        if kind == Kind::Cut {
            self.db.execute(
                "INSERT INTO pdf_cuts (id, paper_id, resource_id, page, rect_json, note, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![shape.id, paper, resource, page as i64, serde_json::to_string(&rect)?, shape.note, shape.created_at],
            )?;
            return Ok(shape);
        }
        self.db.execute(
            "INSERT INTO pdf_shapes (id, paper_id, resource_id, page, kind, rect_json, note, color, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![shape.id, paper, resource, page as i64, kind.key(), serde_json::to_string(&rect)?, shape.note, color, shape.created_at],
        )?;
        Ok(shape)
    }

    /// A text box written on a page: `rect` its edges, `text` what it says (kept as written, but for blanks at
    /// either end), `style` its colours. One with nothing written is not kept.
    pub fn add_pdf_text(&mut self, paper: &str, resource: &str, page: usize, rect: [f32; 4], text: &str, style: &TextStyle) -> Result<Annotation> {
        if page == 0 || page > i32::MAX as usize { bail!("PDF 标注需要有效的页码。") }
        let rect = valid_rect(Kind::Text, rect)?;
        let text = text.trim();
        if text.is_empty() { bail!("文本框里还没有文字。") }
        if text.len() > 100_000 { bail!("文本框的文字过长。") }
        style.check()?;
        self.check_pdf(paper, resource)?;
        let now = now_iso();
        let shape = Annotation { id: new_id("A"), paper_id: paper.into(), resource_id: resource.into(), page, kind: Kind::Text, quote: String::new(), boxes: vec![rect], note: text.into(), color: String::new(), style: Some(style.clone()), created_at: now.clone(), updated_at: now };
        self.db.execute(
            "INSERT INTO pdf_texts (id, paper_id, resource_id, page, rect_json, note, fill, border, ink, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            params![shape.id, paper, resource, page as i64, serde_json::to_string(&rect)?, text, style.fill, style.border, style.ink, shape.created_at],
        )?;
        Ok(shape)
    }

    /// A text box takes other colours; its text and place stay as they were.
    pub fn restyle_pdf_text(&mut self, resource: &str, id: &str, style: &TextStyle) -> Result<()> {
        style.check()?;
        if self.db.execute("UPDATE pdf_texts SET fill = ?1, border = ?2, ink = ?3, updated_at = ?4 WHERE id = ?5 AND resource_id = ?6", params![style.fill, style.border, style.ink, now_iso(), id, resource])? == 0 { bail!("这个文本框不存在。") }
        Ok(())
    }

    /// A box, a piece marked or a text box moved or sized, a note moved: on the same page, to `rect`.
    pub fn move_pdf_shape(&mut self, resource: &str, id: &str, rect: [f32; 4]) -> Result<()> {
        let kind: Option<String> = self.db.query_row("SELECT kind FROM pdf_shapes WHERE id = ?1 AND resource_id = ?2", params![id, resource], |r| r.get(0)).optional()?;
        let Some(kind) = kind else {
            for (table, kind) in [("pdf_texts", Kind::Text), ("pdf_cuts", Kind::Cut)] {
                let found: bool = self.db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = ?1 AND resource_id = ?2)"), params![id, resource], |r| r.get(0))?;
                if !found { continue }
                let rect = serde_json::to_string(&valid_rect(kind, rect)?)?;
                self.db.execute(&format!("UPDATE {table} SET rect_json = ?1, updated_at = ?2 WHERE id = ?3"), params![rect, now_iso(), id])?;
                return Ok(());
            }
            bail!("PDF 标注不存在。")
        };
        let rect = valid_rect(if kind == "box" { Kind::Box } else { Kind::Note }, rect)?;
        self.db.execute("UPDATE pdf_shapes SET rect_json = ?1, updated_at = ?2 WHERE id = ?3", params![serde_json::to_string(&rect)?, now_iso(), id])?;
        Ok(())
    }

    /// Notes can change; the selected original text and its anchor cannot. A sticky note's text is its note, and
    /// a text box's what it says: a text box written empty goes, since one with nothing written is not kept.
    pub fn update_pdf_annotation_note(&mut self, resource: &str, id: &str, note: &str) -> Result<()> {
        if note.len() > 100_000 { bail!("标注笔记过长。") }
        if note.trim().is_empty() && self.db.execute("DELETE FROM pdf_texts WHERE id = ?1 AND resource_id = ?2", params![id, resource])? > 0 { return Ok(()) }
        let params = params![note.trim(), now_iso(), id, resource];
        if self.db.execute("UPDATE pdf_annotations SET note = ?1, updated_at = ?2 WHERE id = ?3 AND resource_id = ?4", params)? == 0
            && self.db.execute("UPDATE pdf_shapes SET note = ?1, updated_at = ?2 WHERE id = ?3 AND resource_id = ?4", params)? == 0
            && self.db.execute("UPDATE pdf_cuts SET note = ?1, updated_at = ?2 WHERE id = ?3 AND resource_id = ?4", params)? == 0
            && self.db.execute("UPDATE pdf_texts SET note = ?1, updated_at = ?2 WHERE id = ?3 AND resource_id = ?4", params)? == 0 { bail!("PDF 标注不存在。") }
        Ok(())
    }

    /// A highlight or a box takes another colour; its text and place stay as they were.
    pub fn recolor_pdf_annotation(&mut self, resource: &str, id: &str, color: &str) -> Result<()> {
        if !COLORS.contains(&color) { bail!("不支持的颜色：{color}") }
        let params = params![color, now_iso(), id, resource];
        if self.db.execute("UPDATE pdf_annotations SET color = ?1, updated_at = ?2 WHERE id = ?3 AND resource_id = ?4 AND kind = 'highlight'", params)? == 0
            && self.db.execute("UPDATE pdf_shapes SET color = ?1, updated_at = ?2 WHERE id = ?3 AND resource_id = ?4 AND kind = 'box'", params)? == 0 { bail!("这条高亮不存在。") }
        Ok(())
    }

    pub fn remove_pdf_annotation(&mut self, resource: &str, id: &str) -> Result<()> {
        if self.db.execute("DELETE FROM pdf_annotations WHERE id = ?1 AND resource_id = ?2", params![id, resource])? == 0
            && self.db.execute("DELETE FROM pdf_shapes WHERE id = ?1 AND resource_id = ?2", params![id, resource])? == 0
            && self.db.execute("DELETE FROM pdf_cuts WHERE id = ?1 AND resource_id = ?2", params![id, resource])? == 0
            && self.db.execute("DELETE FROM pdf_texts WHERE id = ?1 AND resource_id = ?2", params![id, resource])? == 0 { bail!("PDF 标注不存在。") }
        Ok(())
    }
}

/// A shape's place, its corners put in order: a box needs some size, a note is a spot.
fn valid_rect(kind: Kind, [x0, y0, x1, y1]: [f32; 4]) -> Result<[f32; 4]> {
    if [x0, y0, x1, y1].iter().any(|n| !n.is_finite() || n.abs() > 100_000.) { bail!("PDF 标注的位置无效。") }
    match kind {
        Kind::Note => Ok([x0, y0, x0, y0]),
        _ => {
            let rect = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
            if rect[2] - rect[0] < 1. || rect[3] - rect[1] < 1. { bail!(match kind { Kind::Cut => "截图标记太小。", Kind::Text => "文本框太小。", _ => "方框太小。" }) }
            Ok(rect)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Paper;

    #[test]
    fn marks_survive_reopening_without_changing_pdf_and_follow_their_attachment() {
        let root = std::env::temp_dir().join(new_id("wisp-annotations"));
        let source = root.join("source.pdf");
        let mut store = Store::open(&root).unwrap();
        std::fs::write(&source, b"%PDF-1.4 test source").unwrap();
        let paper = store.add_paper(Paper { title: "Roots".into(), ..Default::default() }).unwrap();
        let other = store.add_paper(Paper { title: "Leaves".into(), ..Default::default() }).unwrap();
        let file = store.import_file(&paper.id, &source, "fulltext").unwrap();
        let other_file = store.import_file(&other.id, &source, "fulltext").unwrap();
        let boxes = vec![[10., 20., 80., 30.], [10., 35., 50., 45.]];
        let mark = store.add_pdf_annotation(&paper.id, &file.id, 2, Kind::Underline, "Root growth", &boxes).unwrap();
        assert_eq!(store.add_pdf_annotation(&paper.id, &file.id, 2, Kind::Underline, "Root growth", &boxes).unwrap().id, mark.id);
        assert!(store.add_pdf_annotation(&paper.id, &other_file.id, 1, Kind::Highlight, "Wrong paper", &boxes).is_err());
        assert!(store.update_pdf_annotation_note(&other_file.id, &mark.id, "wrong").is_err());
        assert!(store.remove_pdf_annotation(&other_file.id, &mark.id).is_err());
        store.update_pdf_annotation_note(&file.id, &mark.id, " Compare with leaf growth. ").unwrap();
        let path = store.file_path(&file);
        drop(store);
        let mut store = Store::open(&root).unwrap();
        let saved = store.pdf_annotations(&paper.id, &file.id).unwrap().remove(0);
        assert_eq!((&saved.quote, &saved.boxes, saved.page, saved.kind), (&mark.quote, &boxes, 2, Kind::Underline));
        assert_eq!(saved.note, "Compare with leaf growth.");
        assert_eq!(std::fs::read(path).unwrap(), std::fs::read(&source).unwrap());
        assert!(store.pdf_annotations("", &other_file.id).unwrap().is_empty());
        store.trash_paper(&paper.id).unwrap();
        assert!(store.pdf_annotations("", "").unwrap().is_empty());
        store.restore_paper(&paper.id).unwrap();
        assert_eq!(store.pdf_annotations("", "").unwrap().len(), 1);
        store.remove_pdf_annotation(&file.id, &mark.id).unwrap();
        assert!(store.pdf_annotations("", "").unwrap().is_empty());
        store.add_pdf_annotation(&paper.id, &file.id, 1, Kind::Highlight, "Root", &boxes).unwrap();
        store.remove_file(&file.id).unwrap();
        assert!(store.pdf_annotations("", "").unwrap().is_empty());
        store.add_pdf_annotation(&other.id, &other_file.id, 1, Kind::Underline, "Leaf", &boxes).unwrap();
        store.delete_paper(&other.id).unwrap();
        assert!(store.pdf_annotations("", "").unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_highlight_changes_colour_in_place_and_only_the_one_removed_goes() {
        let root = std::env::temp_dir().join(new_id("wisp-annotation-colour"));
        let source = root.join("source.pdf");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&source, b"%PDF-1.4 test source").unwrap();
        let mut store = Store::in_memory(&root).unwrap();
        let paper = store.add_paper(Paper { title: "Roots".into(), ..Default::default() }).unwrap();
        let file = store.import_file(&paper.id, &source, "fulltext").unwrap();
        let boxes = vec![[10., 20., 80., 30.]];
        let first = store.add_pdf_annotation(&paper.id, &file.id, 1, Kind::Highlight, "Root growth", &boxes).unwrap();
        let second = store.add_pdf_annotation(&paper.id, &file.id, 1, Kind::Highlight, "growth", &[[40., 20., 80., 30.]]).unwrap();
        let line = store.add_pdf_annotation(&paper.id, &file.id, 1, Kind::Underline, "Root", &[[10., 20., 30., 30.]]).unwrap();
        assert_eq!((first.color.as_str(), line.color.as_str()), (COLORS[0], ""));
        store.update_pdf_annotation_note(&file.id, &first.id, "Compare").unwrap();
        store.recolor_pdf_annotation(&file.id, &first.id, "green").unwrap();
        let saved = store.pdf_annotations("", &file.id).unwrap();
        let recoloured = saved.iter().find(|a| a.id == first.id).unwrap();
        assert_eq!((recoloured.color.as_str(), &recoloured.boxes, recoloured.quote.as_str(), recoloured.note.as_str()), ("green", &boxes, "Root growth", "Compare"));
        assert_eq!(saved.iter().find(|a| a.id == second.id).unwrap().color, COLORS[0]);
        assert!(store.recolor_pdf_annotation(&file.id, &first.id, "black").is_err());
        assert!(store.recolor_pdf_annotation(&file.id, &line.id, "green").is_err());
        assert!(store.recolor_pdf_annotation("other", &first.id, "blue").is_err());
        // A highlight of a build from before colours reads as the first colour.
        store.db.execute("UPDATE pdf_annotations SET color = '' WHERE id = ?1", [&second.id]).unwrap();
        assert_eq!(store.pdf_annotations("", &file.id).unwrap().iter().find(|a| a.id == second.id).unwrap().color, COLORS[0]);
        store.remove_pdf_annotation(&file.id, &second.id).unwrap();
        let mut left: Vec<_> = store.pdf_annotations("", &file.id).unwrap().into_iter().map(|a| (a.id, a.color)).collect();
        left.sort();
        let mut kept = vec![(first.id, "green".to_string()), (line.id, String::new())];
        kept.sort();
        assert_eq!(left, kept);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_piece_marked_to_be_cut_out_is_kept_once_moved_noted_and_removed() {
        let root = std::env::temp_dir().join(new_id("wisp-annotation-cuts"));
        let source = root.join("source.pdf");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&source, b"%PDF-1.4 test source").unwrap();
        let mut store = Store::in_memory(&root).unwrap();
        let paper = store.add_paper(Paper { title: "Roots".into(), ..Default::default() }).unwrap();
        let file = store.import_file(&paper.id, &source, "fulltext").unwrap();
        let frame = store.add_pdf_shape(&paper.id, &file.id, 3, Kind::Box, [50., 100., 200., 300.], "").unwrap();
        // Dragged from the bottom right up; the same place marked again is the same mark.
        let cut = store.add_pdf_shape(&paper.id, &file.id, 3, Kind::Cut, [200., 300., 50., 100.], "").unwrap();
        assert_eq!((cut.kind, cut.boxes[0], cut.color.as_str()), (Kind::Cut, [50., 100., 200., 300.], ""));
        assert_eq!(store.add_pdf_shape(&paper.id, &file.id, 3, Kind::Cut, [50., 100., 200., 300.], "").unwrap().id, cut.id);
        assert!(store.add_pdf_shape(&paper.id, &file.id, 3, Kind::Cut, [1., 1., 1.5, 40.], "").is_err());
        store.move_pdf_shape(&file.id, &cut.id, [60., 110., 210., 320.]).unwrap();
        store.update_pdf_annotation_note(&file.id, &cut.id, " Slide 3 ").unwrap();
        assert!(store.recolor_pdf_annotation(&file.id, &cut.id, "green").is_err());
        assert!(store.move_pdf_shape("other", &cut.id, [1., 1., 40., 40.]).is_err());
        let all = store.pdf_annotations(&paper.id, &file.id).unwrap();
        let kept = all.iter().find(|a| a.id == cut.id).unwrap();
        assert_eq!((all.len(), kept.kind, kept.boxes[0], kept.note.as_str()), (2, Kind::Cut, [60., 110., 210., 320.], "Slide 3"));
        // The box beside it stays where it was.
        assert_eq!(all.iter().find(|a| a.id == frame.id).unwrap().boxes[0], [50., 100., 200., 300.]);
        store.remove_pdf_annotation(&file.id, &cut.id).unwrap();
        assert!(store.remove_pdf_annotation(&file.id, &cut.id).is_err());
        assert_eq!(store.pdf_annotations(&paper.id, &file.id).unwrap().len(), 1);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn boxes_and_notes_are_kept_moved_recoloured_and_removed_beside_the_marks_on_text() {
        let root = std::env::temp_dir().join(new_id("wisp-annotation-shapes"));
        let source = root.join("source.pdf");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&source, b"%PDF-1.4 test source").unwrap();
        let mut store = Store::in_memory(&root).unwrap();
        let paper = store.add_paper(Paper { title: "Roots".into(), ..Default::default() }).unwrap();
        let file = store.import_file(&paper.id, &source, "fulltext").unwrap();
        let mark = store.add_pdf_annotation(&paper.id, &file.id, 1, Kind::Highlight, "Root", &[[10., 20., 30., 30.]]).unwrap();
        // Drawn from the bottom right up: the corners are put in order.
        let frame = store.add_pdf_shape(&paper.id, &file.id, 2, Kind::Box, [200., 300., 50., 100.], "").unwrap();
        let note = store.add_pdf_shape(&paper.id, &file.id, 1, Kind::Note, [40., 60., 90., 99.], " Compare with Fig. 2 ").unwrap();
        assert_eq!((frame.boxes[0], frame.color.as_str()), ([50., 100., 200., 300.], BOX_COLOR));
        assert_eq!((note.boxes[0], note.note.as_str(), note.color.as_str()), ([40., 60., 40., 60.], "Compare with Fig. 2", ""));
        assert!(store.add_pdf_shape(&paper.id, &file.id, 1, Kind::Box, [1., 1., 1.5, 40.], "").is_err());
        assert!(store.add_pdf_shape(&paper.id, &file.id, 1, Kind::Highlight, [1., 1., 40., 40.], "").is_err());
        assert!(store.add_pdf_annotation(&paper.id, &file.id, 1, Kind::Box, "x", &[[1., 1., 40., 40.]]).is_err());
        assert!(store.add_pdf_shape(&paper.id, "missing", 1, Kind::Note, [1., 1., 1., 1.], "").is_err());
        let all = store.pdf_annotations(&paper.id, &file.id).unwrap();
        // By page; the highlight and the note of page 1 are made in the same instant, so their order is the ids' and not fixed.
        let mut first: Vec<String> = all[..2].iter().map(|a| a.kind.key().to_string()).collect();
        first.sort();
        assert_eq!((all.iter().map(|a| a.page).collect::<Vec<_>>(), first, all[2].kind), (vec![1, 1, 2], vec![Kind::Highlight.key().to_string(), Kind::Note.key().to_string()], Kind::Box));
        store.move_pdf_shape(&file.id, &frame.id, [60., 110., 210., 320.]).unwrap();
        store.move_pdf_shape(&file.id, &note.id, [70., 80., 0., 0.]).unwrap();
        store.recolor_pdf_annotation(&file.id, &frame.id, "orange").unwrap();
        store.update_pdf_annotation_note(&file.id, &note.id, "Rewritten").unwrap();
        assert!(store.recolor_pdf_annotation(&file.id, &note.id, "green").is_err());
        assert!(store.move_pdf_shape(&file.id, &mark.id, [1., 1., 40., 40.]).is_err());
        let all = store.pdf_annotations("", &file.id).unwrap();
        let find = |id: &str| all.iter().find(|a| a.id == id).unwrap().clone();
        assert_eq!((find(&frame.id).boxes[0], find(&frame.id).color), ([60., 110., 210., 320.], "orange".to_string()));
        assert_eq!((find(&note.id).boxes[0], find(&note.id).note), ([70., 80., 70., 80.], "Rewritten".to_string()));
        store.remove_pdf_annotation(&file.id, &note.id).unwrap();
        assert!(store.remove_pdf_annotation(&file.id, &note.id).is_err());
        assert_eq!(store.pdf_annotations("", "").unwrap().len(), 2);
        store.trash_paper(&paper.id).unwrap();
        assert!(store.pdf_annotations("", "").unwrap().is_empty());
        store.restore_paper(&paper.id).unwrap();
        store.remove_file(&file.id).unwrap();
        assert!(store.pdf_annotations("", "").unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_text_box_keeps_its_words_colours_and_place_and_goes_when_written_empty() {
        let root = std::env::temp_dir().join(new_id("wisp-annotation-texts"));
        let source = root.join("source.pdf");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&source, b"%PDF-1.4 test source").unwrap();
        let mut store = Store::open(&root).unwrap();
        let paper = store.add_paper(Paper { title: "Roots".into(), ..Default::default() }).unwrap();
        let file = store.import_file(&paper.id, &source, "fulltext").unwrap();
        let frame = store.add_pdf_shape(&paper.id, &file.id, 1, Kind::Box, [10., 10., 50., 50.], "").unwrap();
        // Drawn from the bottom right up; the words are kept as written, lines and all, but for blanks at either end.
        let style = TextStyle::default().with(TextPart::Fill, "none").with(TextPart::Ink, "blue");
        let text = store.add_pdf_text(&paper.id, &file.id, 2, [300., 200., 100., 120.], "  生长素 auxin\nflows down  ", &style).unwrap();
        assert_eq!((text.kind, text.boxes[0], text.note.as_str(), text.color.as_str()), (Kind::Text, [100., 120., 300., 200.], "生长素 auxin\nflows down", ""));
        assert_eq!(text.style.as_ref().map(|s| (s.fill.as_str(), s.border.as_str(), s.ink.as_str())), Some(("none", "yellow", "blue")));
        // Nothing written, too small, an unknown colour, the wrong way in, a file of another paper: none is kept.
        assert!(store.add_pdf_text(&paper.id, &file.id, 2, [10., 10., 90., 90.], " \n ", &style).is_err());
        assert!(store.add_pdf_text(&paper.id, &file.id, 2, [10., 10., 10.5, 90.], "x", &style).is_err());
        assert!(store.add_pdf_text(&paper.id, &file.id, 2, [10., 10., 90., 90.], "x", &style.with(TextPart::Ink, "none")).is_err());
        assert!(store.add_pdf_shape(&paper.id, &file.id, 2, Kind::Text, [10., 10., 90., 90.], "x").is_err());
        assert!(store.add_pdf_text(&paper.id, "missing", 2, [10., 10., 90., 90.], "x", &style).is_err());
        // Moved and sized, written again, coloured anew: each keeps the rest as it was.
        store.move_pdf_shape(&file.id, &text.id, [110., 130., 330., 260.]).unwrap();
        store.update_pdf_annotation_note(&file.id, &text.id, "PIN proteins").unwrap();
        store.restyle_pdf_text(&file.id, &text.id, &style.with(TextPart::Border, "none").with(TextPart::Fill, "white")).unwrap();
        assert!(store.restyle_pdf_text(&file.id, &text.id, &style.with(TextPart::Fill, "teal")).is_err());
        assert!(store.restyle_pdf_text(&file.id, &frame.id, &style).is_err());
        assert!(store.recolor_pdf_annotation(&file.id, &text.id, "green").is_err());
        assert!(store.move_pdf_shape(&file.id, &text.id, [1., 1., 1.2, 40.]).is_err());
        assert!(store.move_pdf_shape("other", &text.id, [1., 1., 40., 40.]).is_err());
        drop(store);
        // Reopened, it is as it was left, and the box beside it too.
        let mut store = Store::open(&root).unwrap();
        let all = store.pdf_annotations(&paper.id, &file.id).unwrap();
        let kept = all.iter().find(|a| a.id == text.id).unwrap();
        assert_eq!((kept.page, kept.boxes[0], kept.note.as_str()), (2, [110., 130., 330., 260.], "PIN proteins"));
        assert_eq!(kept.style, Some(TextStyle { fill: "white".into(), border: "none".into(), ink: "blue".into() }));
        assert_eq!((all.iter().find(|a| a.id == frame.id).unwrap().style.clone(), all.len()), (None, 2));
        // Read as an agent reads it: the kind and the colours are said, and only a text box has `style`.
        let json = serde_json::to_value(kept).unwrap();
        assert_eq!((json["kind"].as_str(), json["style"]["ink"].as_str()), (Some("text"), Some("blue")));
        assert!(serde_json::to_value(all.iter().find(|a| a.id == frame.id).unwrap()).unwrap().get("style").is_none());
        // A colour of a later build reads as the first text box's.
        store.db.execute("UPDATE pdf_texts SET fill = 'teal' WHERE id = ?1", [&text.id]).unwrap();
        assert_eq!(store.pdf_annotations("", &file.id).unwrap().iter().find(|a| a.id == text.id).unwrap().style.as_ref().unwrap().fill, TextStyle::default().fill);
        // Written empty, it goes; nothing else does.
        store.update_pdf_annotation_note(&file.id, &text.id, "  \n ").unwrap();
        assert_eq!(store.pdf_annotations("", &file.id).unwrap().iter().map(|a| a.id.clone()).collect::<Vec<_>>(), vec![frame.id.clone()]);
        let again = store.add_pdf_text(&paper.id, &file.id, 1, [10., 10., 90., 90.], "Again", &TextStyle::default()).unwrap();
        store.remove_pdf_annotation(&file.id, &again.id).unwrap();
        assert!(store.remove_pdf_annotation(&file.id, &again.id).is_err());
        // In the trash with its paper, gone with its file.
        store.add_pdf_text(&paper.id, &file.id, 1, [10., 10., 90., 90.], "Kept", &TextStyle::default()).unwrap();
        store.trash_paper(&paper.id).unwrap();
        assert!(store.pdf_annotations("", "").unwrap().is_empty());
        store.restore_paper(&paper.id).unwrap();
        assert_eq!(store.pdf_annotations("", "").unwrap().len(), 2);
        store.remove_file(&file.id).unwrap();
        assert!(store.pdf_annotations("", "").unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_library_from_before_text_boxes_gains_their_table() {
        let root = std::env::temp_dir().join(new_id("wisp-annotation-texts-old"));
        let mut store = Store::open(&root).unwrap();
        store.db.execute_batch("DROP TABLE pdf_texts").unwrap();
        let paper = store.add_paper(Paper { title: "Existing paper".into(), ..Default::default() }).unwrap();
        drop(store);
        let mut store = Store::open(&root).unwrap();
        assert!(store.pdf_annotations("", "").unwrap().is_empty());
        assert!(store.add_pdf_text(&paper.id, "missing", 1, [1., 1., 40., 40.], "x", &TextStyle::default()).is_err());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_anchors_are_rejected_and_old_libraries_gain_the_table() {
        let root = std::env::temp_dir().join(new_id("wisp-annotation-migration"));
        let mut store = Store::open(&root).unwrap();
        store.db.execute_batch("DROP TABLE pdf_annotations").unwrap();
        let paper = store.add_paper(Paper { title: "Existing paper".into(), ..Default::default() }).unwrap();
        drop(store);
        let mut store = Store::open(&root).unwrap();
        assert_eq!(store.get_paper(&paper.id).unwrap().title, "Existing paper");
        assert!(store.pdf_annotations("", "").unwrap().is_empty());
        for (page, quote, boxes) in [(0, "word", vec![[1., 2., 3., 4.]]), (1, " ", vec![[1., 2., 3., 4.]]), (1, "word", vec![]), (1, "word", vec![[3., 2., 1., 4.]]), (1, "word", vec![[1., 2., f32::NAN, 4.]])] {
            assert!(store.add_pdf_annotation(&paper.id, "missing", page, Kind::Underline, quote, &boxes).is_err());
        }
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
