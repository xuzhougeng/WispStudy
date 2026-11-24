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

/// The hue, saturation and lightness of a colour of `COLORS` (anything else as the first): what the window
/// draws a mark in (`theme.rs`) and what a PDF exported with its marks gives it (`annotpdf`).
pub fn hue(key: &str) -> (f32, f32, f32) {
    match key {
        "green" => (135., 0.55, 0.5),
        "blue" => (205., 0.85, 0.55),
        "pink" => (335., 0.85, 0.62),
        "purple" => (270., 0.6, 0.62),
        "red" => (2., 0.8, 0.55),
        "orange" => (26., 0.95, 0.55),
        _ => (48., 1., 0.5),
    }
}

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

    /// Put a mark back as `mark` has it: the same id, made and last changed at the same moments, on the same page
    /// and place, with the same words, note and colours. A mark removed comes back as it was, one changed since
    /// goes back to this. What undo and redo write (`step_pdf_marks`).
    pub fn restore_pdf_annotation(&mut self, mark: &Annotation) -> Result<()> {
        if mark.page == 0 || mark.page > i32::MAX as usize { bail!("PDF 标注需要有效的页码。") }
        self.check_pdf(&mark.paper_id, &mark.resource_id)?;
        let (id, paper, resource, page, note, made, changed) = (&mark.id, &mark.paper_id, &mark.resource_id, mark.page as i64, &mark.note, &mark.created_at, &mark.updated_at);
        if !mark.kind.shape() {
            if mark.boxes.is_empty() || mark.quote.trim().is_empty() { bail!("PDF 标注的位置无效。") }
            self.db.execute(
                "INSERT OR REPLACE INTO pdf_annotations (id, paper_id, resource_id, page, kind, quote, boxes_json, note, color, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![id, paper, resource, page, mark.kind.key(), mark.quote, serde_json::to_string(&mark.boxes)?, note, mark.color, made, changed],
            )?;
            return Ok(());
        }
        let Some(rect) = mark.boxes.first() else { bail!("PDF 标注的位置无效。") };
        let rect = serde_json::to_string(rect)?;
        match mark.kind {
            Kind::Cut => self.db.execute(
                "INSERT OR REPLACE INTO pdf_cuts (id, paper_id, resource_id, page, rect_json, note, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![id, paper, resource, page, rect, note, made, changed],
            )?,
            Kind::Text => {
                let style = mark.style.clone().unwrap_or_default();
                self.db.execute(
                    "INSERT OR REPLACE INTO pdf_texts (id, paper_id, resource_id, page, rect_json, note, fill, border, ink, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![id, paper, resource, page, rect, note, style.fill, style.border, style.ink, made, changed],
                )?
            }
            _ => self.db.execute(
                "INSERT OR REPLACE INTO pdf_shapes (id, paper_id, resource_id, page, kind, rect_json, note, color, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![id, paper, resource, page, mark.kind.key(), rect, note, mark.color, made, changed],
            )?,
        };
        Ok(())
    }

    /// Take back the last change of a file's marks kept in `history` (`back`), or do again the last one taken back.
    /// All of a change or none of it is written. `Ok(None)`: there is nothing to take back, or to do again.
    pub fn step_pdf_marks(&mut self, resource: &str, history: &mut History, back: bool) -> Result<Option<Change>> {
        let Some(change) = (if back { history.undo.pop() } else { history.redo.pop() }) else { return Ok(None) };
        self.db.execute_batch("SAVEPOINT mark_step")?;
        let written = change.0.iter().try_for_each(|(before, after)| {
            let (to, from) = if back { (before, after) } else { (after, before) };
            match (to, from) {
                (Some(mark), _) => self.restore_pdf_annotation(mark),
                // Made by the change: it goes again (already gone is as good).
                (None, Some(mark)) => { self.remove_pdf_annotation(resource, &mark.id).ok(); Ok(()) }
                (None, None) => Ok(()),
            }
        });
        if let Err(error) = written {
            self.db.execute_batch("ROLLBACK TO mark_step; RELEASE mark_step")?;
            return Err(error);
        }
        self.db.execute_batch("RELEASE mark_step")?;
        if back { history.redo.push(change.clone()) } else { history.undo.push(change.clone()) }
        Ok(Some(change))
    }
}

/// One change of a file's marks, as undo takes it back: each mark it touched as it was before and after (`None`:
/// not there). A save of a text box's words and edges together is one change.
#[derive(Clone, Debug, PartialEq)]
pub struct Change(pub Vec<(Option<Annotation>, Option<Annotation>)>);

impl Change {
    /// What a write did, from the file's marks read before and after it. A mark whose only difference is
    /// `updated_at` (a note saved as it was) did not change.
    pub fn between(before: &[Annotation], after: &[Annotation]) -> Change {
        let same = |a: &Annotation, b: &Annotation| Annotation { updated_at: String::new(), ..a.clone() } == Annotation { updated_at: String::new(), ..b.clone() };
        let mut pairs: Vec<(Option<Annotation>, Option<Annotation>)> = before.iter().filter_map(|old| match after.iter().find(|new| new.id == old.id) {
            Some(new) if same(old, new) => None,
            new => Some((Some(old.clone()), new.cloned())),
        }).collect();
        pairs.extend(after.iter().filter(|new| !before.iter().any(|old| old.id == new.id)).map(|new| (None, Some(new.clone()))));
        Change(pairs)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Where to look once the change is taken back (`back`) or done again: the 1-based page and the edges of the
    /// first mark it touched, where it now stands, or where it stood if it is gone.
    pub fn place(&self, back: bool) -> Option<(usize, [f32; 4])> {
        let (before, after) = self.0.first()?;
        let (now, was) = if back { (before, after) } else { (after, before) };
        let mark = now.as_ref().or(was.as_ref())?;
        let first = mark.boxes.first()?;
        let rect = mark.boxes.iter().fold(*first, |r, b| [r[0].min(b[0]), r[1].min(b[1]), r[2].max(b[2]), r[3].max(b[3])]);
        Some((mark.page, rect))
    }
}

/// The changes of one file's marks that can be taken back, and those taken back that can be done again, the
/// newest last. Kept in memory only, by the window, while its library is open.
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Change>,
    redo: Vec<Change>,
}

/// How many changes of a file can be taken back.
const HISTORY: usize = 100;

impl History {
    /// A change just written: the next undo takes it back, and what was taken back before can no longer be done again.
    pub fn record(&mut self, change: Change) {
        if change.is_empty() { return }
        self.redo.clear();
        self.undo.push(change);
        if self.undo.len() > HISTORY { self.undo.remove(0); }
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
    fn every_change_of_every_kind_of_mark_is_taken_back_and_done_again_as_it_was() {
        let root = std::env::temp_dir().join(new_id("wisp-annotation-undo"));
        let source = root.join("source.pdf");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&source, b"%PDF-1.4 test source").unwrap();
        let mut store = Store::in_memory(&root).unwrap();
        let paper = store.add_paper(Paper { title: "Roots".into(), ..Default::default() }).unwrap();
        let file = store.import_file(&paper.id, &source, "fulltext").unwrap();
        let mut history = History::default();
        // Each write as the window makes it: the file's marks read before and after, the difference recorded.
        let write = |store: &mut Store, history: &mut History, change: &dyn Fn(&mut Store)| {
            let before = store.pdf_annotations("", &file.id).unwrap();
            change(store);
            history.record(Change::between(&before, &store.pdf_annotations("", &file.id).unwrap()));
        };
        let id = |store: &Store, kind: Kind| store.pdf_annotations("", &file.id).unwrap().into_iter().find(|a| a.kind == kind).unwrap().id;
        write(&mut store, &mut history, &|s| { s.add_pdf_annotation(&paper.id, &file.id, 1, Kind::Highlight, "Root growth", &[[10., 20., 80., 30.], [10., 35., 50., 45.]]).unwrap(); });
        write(&mut store, &mut history, &|s| { s.add_pdf_annotation(&paper.id, &file.id, 1, Kind::Underline, "growth", &[[40., 20., 80., 30.]]).unwrap(); });
        write(&mut store, &mut history, &|s| { s.add_pdf_shape(&paper.id, &file.id, 2, Kind::Box, [50., 100., 200., 300.], "").unwrap(); });
        write(&mut store, &mut history, &|s| { s.add_pdf_shape(&paper.id, &file.id, 2, Kind::Note, [40., 60., 0., 0.], "Compare").unwrap(); });
        write(&mut store, &mut history, &|s| { s.add_pdf_shape(&paper.id, &file.id, 3, Kind::Cut, [20., 20., 120., 90.], "").unwrap(); });
        write(&mut store, &mut history, &|s| { s.add_pdf_text(&paper.id, &file.id, 3, [100., 120., 300., 200.], "auxin", &TextStyle::default()).unwrap(); });
        let (highlight, underline, frame, note, cut, text) = (id(&store, Kind::Highlight), id(&store, Kind::Underline), id(&store, Kind::Box), id(&store, Kind::Note), id(&store, Kind::Cut), id(&store, Kind::Text));
        // A note saved as it was, the same piece marked again: nothing to take back.
        let undoable = history.undo.len();
        write(&mut store, &mut history, &|s| s.update_pdf_annotation_note(&file.id, &note, "Compare").unwrap());
        write(&mut store, &mut history, &|s| { s.add_pdf_shape(&paper.id, &file.id, 3, Kind::Cut, [20., 20., 120., 90.], "").unwrap(); });
        assert_eq!(history.undo.len(), undoable);
        // Not recorded, the note keeps the moment it was saved at: the marks as made are read after it.
        let made = store.pdf_annotations("", &file.id).unwrap();
        // Every kind of edit, then everything removed.
        write(&mut store, &mut history, &|s| s.update_pdf_annotation_note(&file.id, &highlight, "Why roots").unwrap());
        write(&mut store, &mut history, &|s| s.recolor_pdf_annotation(&file.id, &highlight, "green").unwrap());
        write(&mut store, &mut history, &|s| s.recolor_pdf_annotation(&file.id, &frame, "blue").unwrap());
        write(&mut store, &mut history, &|s| s.move_pdf_shape(&file.id, &frame, [60., 110., 210., 320.]).unwrap());
        write(&mut store, &mut history, &|s| s.move_pdf_shape(&file.id, &note, [70., 80., 0., 0.]).unwrap());
        write(&mut store, &mut history, &|s| s.move_pdf_shape(&file.id, &cut, [30., 30., 130., 100.]).unwrap());
        write(&mut store, &mut history, &|s| s.restyle_pdf_text(&file.id, &text, &TextStyle::default().with(TextPart::Ink, "blue")).unwrap());
        // A text box's words and edges written together are one change.
        write(&mut store, &mut history, &|s| { s.update_pdf_annotation_note(&file.id, &text, "PIN proteins").unwrap(); s.move_pdf_shape(&file.id, &text, [100., 120., 300., 260.]).unwrap() });
        let edited = store.pdf_annotations("", &file.id).unwrap();
        for mark in [&highlight, &underline, &frame, &note, &cut] { write(&mut store, &mut history, &|s| s.remove_pdf_annotation(&file.id, mark).unwrap()) }
        // Written empty, the text box goes.
        write(&mut store, &mut history, &|s| s.update_pdf_annotation_note(&file.id, &text, " ").unwrap());
        assert!(store.pdf_annotations("", &file.id).unwrap().is_empty());
        // Back through the removals: each mark comes back as it was, the same id, times, place, words and colours.
        for _ in 0..6 { assert!(store.step_pdf_marks(&file.id, &mut history, true).unwrap().is_some()) }
        assert_eq!(store.pdf_annotations("", &file.id).unwrap(), edited);
        // Back through the edits to the marks as made, and further to none.
        for _ in 0..8 { store.step_pdf_marks(&file.id, &mut history, true).unwrap(); }
        assert_eq!(store.pdf_annotations("", &file.id).unwrap(), made);
        let back = store.step_pdf_marks(&file.id, &mut history, true).unwrap().unwrap();
        assert_eq!(back.place(true), Some((3, [100., 120., 300., 200.])));
        for _ in 0..5 { store.step_pdf_marks(&file.id, &mut history, true).unwrap(); }
        assert!(store.pdf_annotations("", &file.id).unwrap().is_empty());
        assert_eq!(store.step_pdf_marks(&file.id, &mut history, true).unwrap(), None);
        // Done again, all of it: as it was left.
        for _ in 0..20 { store.step_pdf_marks(&file.id, &mut history, false).unwrap(); }
        assert!(store.pdf_annotations("", &file.id).unwrap().is_empty());
        for _ in 0..6 { store.step_pdf_marks(&file.id, &mut history, true).unwrap(); }
        assert_eq!(store.pdf_annotations("", &file.id).unwrap(), edited);
        // The first one done again is where it stands now, the highlight's lines as one place.
        let again = store.step_pdf_marks(&file.id, &mut history, false).unwrap().unwrap();
        assert_eq!(again.place(false), Some((1, [10., 20., 80., 45.])));
        store.step_pdf_marks(&file.id, &mut history, true).unwrap();
        // A new change: what was taken back cannot be done again.
        write(&mut store, &mut history, &|s| s.recolor_pdf_annotation(&file.id, &highlight, "pink").unwrap());
        assert_eq!(store.step_pdf_marks(&file.id, &mut history, false).unwrap(), None);
        // A mark whose file has gone cannot come back; the change is not half written.
        store.step_pdf_marks(&file.id, &mut history, true).unwrap();
        write(&mut store, &mut history, &|s| s.remove_pdf_annotation(&file.id, &frame).unwrap());
        store.remove_file(&file.id).unwrap();
        assert!(store.step_pdf_marks(&file.id, &mut history, true).is_err());
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
