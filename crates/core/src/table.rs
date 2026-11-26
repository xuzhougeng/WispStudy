//! 副表: a table kept as a file, most often a paper's supplementary table. An Excel workbook (`.xlsx`, `.xlsm`)
//! is read with the zip and XML readers of `docx`; `.csv`, `.tsv` and a tab-separated `.txt` are read as text,
//! whatever encoding a spreadsheet program saved them in. A sheet is handed over row by row and never held
//! whole: a supplementary table of a hundred thousand genes is read to find three of them.
//!
//! A cell is the words a person would read in it: a shared string, a number as the spreadsheet shows it
//! (fifteen digits, so 0.30000000000000004 is 0.3), a date where the cell's format is one. A formula is its
//! last value, as the file keeps it.

use crate::docx::{Node, Zip, attr, node, unescape};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

/// The formats an attachment is known as a table by. A `.txt` is read too when it is asked for by name
/// (`open`), but an attachment that is one is text first.
pub const FORMATS: [&str; 5] = ["xlsx", "xlsm", "csv", "tsv", "tab"];

/// A file this large is not read: every call reads the file anew.
const MAX_BYTES: u64 = 256 << 20;
/// The widest sheet a spreadsheet holds; a cell that claims a column beyond it is not believed.
const MAX_COLUMNS: usize = 16_384;

pub fn is_table(format: &str) -> bool {
    FORMATS.contains(&format.to_ascii_lowercase().as_str())
}

/// A table file opened: its sheets by name (one, named after nothing, for a text file).
pub struct Book {
    pub sheets: Vec<String>,
    source: Source,
}

enum Source {
    Workbook(Workbook),
    Text { text: String, delimiter: char },
}

struct Workbook {
    zip: Zip,
    /// The part of each sheet, in the order of `Book::sheets`.
    parts: Vec<String>,
    shared: Vec<String>,
    /// For each cell style, whether numbers in it are dates.
    dates: Vec<bool>,
    /// Days counted from 1904, as old Mac workbooks do.
    from_1904: bool,
}

/// One sheet, loaded: its rows are read from here.
pub struct Sheet<'a> {
    book: &'a Book,
    xml: Cow<'a, str>,
}

pub fn open(path: &Path) -> Result<Book, String> {
    let format = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let size = std::fs::metadata(path).map_err(|e| format!("无法读取文件 {}：{e}", path.display()))?.len();
    if size > MAX_BYTES { return Err(format!("这个文件有 {}，太大，读不了。", crate::model::file_size(size))) }
    read(std::fs::read(path).map_err(|e| format!("无法读取文件 {}：{e}", path.display()))?, &format)
}

/// A table file's bytes, read as the format (a lowercase extension) says.
pub fn read(bytes: Vec<u8>, format: &str) -> Result<Book, String> {
    match format {
        "xlsx" | "xlsm" => workbook(&bytes),
        "csv" | "tsv" | "tab" | "txt" => {
            let text = decoded(&bytes);
            let delimiter = match format {
                "tsv" | "tab" => '\t',
                "csv" => delimiter_of(&text),
                // A text file is a table when tabs set its columns apart.
                _ if text.lines().take(20).any(|line| line.contains('\t')) => '\t',
                _ => return Err("这个 .txt 文件不是用制表符分列的表格。".into()),
            };
            Ok(Book { sheets: vec![String::new()], source: Source::Text { text, delimiter } })
        }
        "xls" | "xlsb" | "ods" | "numbers" => Err(format!("读不了 .{format} 文件：请在 Excel 或 WPS 里另存为 .xlsx 或 .csv 再读。")),
        other => Err(format!("read_table 读的是表格文件（.xlsx、.csv、.tsv），不是 .{other} 文件。")),
    }
}

impl Book {
    pub fn sheet(&self, at: usize) -> Result<Sheet<'_>, String> {
        let xml = match &self.source {
            Source::Text { .. } => Cow::Borrowed(""),
            Source::Workbook(book) => {
                let part = book.parts.get(at).ok_or("没有这个表。")?;
                Cow::Owned(book.zip.text(part).map_err(|_| NOT_WORKBOOK.to_string())?)
            }
        };
        Ok(Sheet { book: self, xml })
    }
}

impl Sheet<'_> {
    /// The sheet's rows from the top, each under the number a spreadsheet gives it (from 1) and without the
    /// empty cells at its end; a row with nothing in it is passed over. `visit` answers whether to go on.
    pub fn rows(&self, visit: &mut dyn FnMut(usize, Vec<String>) -> bool) {
        match &self.book.source {
            Source::Text { text, delimiter } => delimited(text, *delimiter, visit),
            Source::Workbook(book) => book.rows(&self.xml, visit),
        }
    }
}

// ---- Excel workbooks

const NOT_WORKBOOK: &str = "这不是一个读得了的 Excel 文件（.xlsx）。";

/// An element's name without its namespace prefix: some programs write `x:row` for `row`.
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// (id, type, target) of each relationship of a part.
fn relations(xml: &str) -> Vec<(String, String, String)> {
    let (mut out, mut at) = (vec![], 0);
    while let Some((piece, end)) = node(xml, at) {
        if let Node::Open(name, tag) | Node::Empty(name, tag) = piece
            && local(name) == "Relationship"
        {
            out.push((attr(tag, "Id").unwrap_or_default(), attr(tag, "Type").unwrap_or_default(), attr(tag, "Target").unwrap_or_default()));
        }
        at = end;
    }
    out
}

fn workbook(bytes: &[u8]) -> Result<Book, String> {
    // A workbook with a password, and one of the old format under a new name, are not zip archives.
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) { return Err("这个 Excel 文件加了密码，或者是改了扩展名的旧格式（.xls），读不了：请去掉密码或另存为 .xlsx 再读。".into()) }
    let zip = Zip::read(bytes).map_err(|_| NOT_WORKBOOK.to_string())?;
    let part = |name: &str| zip.text(name).map_err(|_| NOT_WORKBOOK.to_string());
    let main = relations(&part("_rels/.rels")?).into_iter().find(|(_, kind, _)| kind.ends_with("/officeDocument")).map_or("xl/workbook.xml".to_string(), |(_, _, target)| target.trim_start_matches('/').to_string());
    let book = part(&main)?;
    if book.is_empty() { return Err(NOT_WORKBOOK.into()) }
    let targets: HashMap<String, (String, String)> = relations(&part(&crate::docx::rels_of(&main))?).into_iter().map(|(id, kind, target)| (id, (kind, crate::docx::beside(&main, &target)))).collect();
    let (mut sheets, mut parts, mut from_1904, mut at) = (vec![], vec![], false, 0);
    while let Some((piece, end)) = node(&book, at) {
        if let Node::Open(name, tag) | Node::Empty(name, tag) = piece {
            match local(name) {
                "workbookPr" => from_1904 = attr(tag, "date1904").is_some_and(|value| value == "1" || value == "true"),
                "sheet" => {
                    // A sheet that holds a chart alone has no rows.
                    if let Some((kind, target)) = attr(tag, "r:id").and_then(|id| targets.get(&id))
                        && kind.ends_with("/worksheet")
                        && zip.has(target)
                    {
                        sheets.push(attr(tag, "name").unwrap_or_default());
                        parts.push(target.clone());
                    }
                }
                _ => {}
            }
        }
        at = end;
    }
    if sheets.is_empty() { return Err("这个 Excel 文件里没有表。".into()) }
    let strings = targets.values().find(|(kind, _)| kind.ends_with("/sharedStrings")).map_or("xl/sharedStrings.xml", |(_, target)| target.as_str());
    let styles = targets.values().find(|(kind, _)| kind.ends_with("/styles")).map_or("xl/styles.xml", |(_, target)| target.as_str());
    let (shared, dates) = (shared_strings(&part(strings)?), date_styles(&part(styles)?));
    Ok(Book { sheets, source: Source::Workbook(Workbook { zip, parts, shared, dates, from_1904 }) })
}

/// The strings the cells of a workbook share, in order. One written in several looks is its runs together; how
/// it is read aloud in Japanese (`rPh`) is no part of it.
fn shared_strings(xml: &str) -> Vec<String> {
    let (mut out, mut stack, mut one, mut at): (Vec<String>, Vec<&str>, String, usize) = (vec![], vec![], String::new(), 0);
    while let Some((piece, end)) = node(xml, at) {
        match piece {
            Node::Open(name, _) => stack.push(local(name)),
            Node::Empty(name, _) if local(name) == "si" => out.push(String::new()),
            Node::Close => {
                if stack.pop() == Some("si") { out.push(spelled(&std::mem::take(&mut one))) }
            }
            Node::Text(text) if stack.last() == Some(&"t") && !stack.contains(&"rPh") => one.push_str(&unescape(text)),
            _ => {}
        }
        at = end;
    }
    out
}

/// A string with the characters a workbook writes as `_x000D_` put back; a carriage return is left out.
fn spelled(text: &str) -> String {
    if !text.contains("_x") { return text.to_string() }
    let (mut out, mut rest) = (String::with_capacity(text.len()), text);
    while let Some(at) = rest.find("_x") {
        out.push_str(&rest[..at]);
        let code = rest.get(at + 2..at + 7).filter(|code| code.ends_with('_')).and_then(|code| u32::from_str_radix(&code[..4], 16).ok()).and_then(char::from_u32);
        match code {
            Some(c) => {
                if c != '\r' { out.push(c) }
                rest = &rest[at + 7..];
            }
            None => {
                out.push_str("_x");
                rest = &rest[at + 2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether numbers in a cell of each style are dates: by the style's number format, a built-in one or the
/// workbook's own.
fn date_styles(xml: &str) -> Vec<bool> {
    let (mut own, mut out, mut stack, mut at): (HashMap<usize, bool>, Vec<bool>, Vec<&str>, usize) = (HashMap::new(), vec![], vec![], 0);
    while let Some((piece, end)) = node(xml, at) {
        if let Node::Open(name, tag) | Node::Empty(name, tag) = piece {
            let id = || attr(tag, "numFmtId").and_then(|id| id.parse::<usize>().ok());
            match local(name) {
                "numFmt" => {
                    if let Some(id) = id() { own.insert(id, dated(&attr(tag, "formatCode").unwrap_or_default())); }
                }
                "xf" if stack.last() == Some(&"cellXfs") => {
                    let id = id().unwrap_or(0);
                    out.push(own.get(&id).copied().unwrap_or(matches!(id, 14..=22 | 27..=36 | 45..=47 | 50..=58)));
                }
                _ => {}
            }
        }
        match piece {
            Node::Open(name, _) => stack.push(local(name)),
            Node::Close => drop(stack.pop()),
            _ => {}
        }
        at = end;
    }
    out
}

/// Whether a number format writes a date or a time: it names a day, month, year, hour or second outside the
/// words it quotes and the conditions and colours it brackets.
fn dated(code: &str) -> bool {
    let (mut quoted, mut bracketed, mut skip) = (false, false, false);
    for c in code.chars() {
        if skip {
            skip = false;
        } else if quoted {
            quoted = c != '"';
        } else if bracketed {
            bracketed = c != ']';
        } else {
            match c {
                '"' => quoted = true,
                '[' => bracketed = true,
                '\\' | '_' | '*' => skip = true,
                'd' | 'D' | 'm' | 'M' | 'y' | 'Y' | 'h' | 'H' | 's' | 'S' => return true,
                _ => {}
            }
        }
    }
    false
}

/// A column's number (from 0) off a cell's name: `C` of `C12`.
fn column_of(cell: &str) -> Option<usize> {
    let letters = cell.bytes().take_while(u8::is_ascii_alphabetic).count();
    if letters == 0 || letters > 3 { return None }
    Some(cell.bytes().take(letters).fold(0usize, |n, b| n * 26 + (b.to_ascii_uppercase() - b'A') as usize + 1) - 1)
}

/// A column named by its letters alone (`C`, `aa`), as its number.
pub fn column_at(name: &str) -> Option<usize> {
    let name = name.trim();
    if !name.bytes().all(|b| b.is_ascii_alphabetic()) { return None }
    column_of(name).filter(|column| *column < MAX_COLUMNS)
}

/// A column's letters from its number (from 0): `A`, `Z`, `AA`.
pub fn column_name(mut at: usize) -> String {
    let mut name = vec![];
    loop {
        name.push(b'A' + (at % 26) as u8);
        if at < 26 { break }
        at = at / 26 - 1;
    }
    name.reverse();
    String::from_utf8(name).unwrap_or_default()
}

/// A number as a spreadsheet shows it: to fifteen digits, written out when it is of a size to read, with a
/// power of ten when it is not (`3.2e-120`). A whole number stays as it is written.
fn number(raw: &str) -> String {
    let raw = raw.trim();
    if raw.bytes().all(|b| b.is_ascii_digit() || b == b'-') { return raw.to_string() }
    let Some(value) = raw.parse::<f64>().ok().filter(|value| value.is_finite()) else { return raw.to_string() };
    if value == 0.0 { return "0".into() }
    let written = format!("{:.14e}", value.abs());
    let Some((mantissa, power)) = written.split_once('e').and_then(|(mantissa, power)| Some((mantissa, power.parse::<i32>().ok()?))) else { return raw.to_string() };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = match digits.trim_end_matches('0') {
        "" => "0",
        kept => kept,
    };
    let sign = if value < 0.0 { "-" } else { "" };
    if !(-5..15).contains(&power) {
        return if digits.len() == 1 { format!("{sign}{digits}e{power}") } else { format!("{sign}{}.{}e{power}", &digits[..1], &digits[1..]) };
    }
    if power < 0 { return format!("{sign}0.{}{digits}", "0".repeat((-power - 1) as usize)) }
    let whole = power as usize + 1;
    if digits.len() <= whole { format!("{sign}{digits}{}", "0".repeat(whole - digits.len())) } else { format!("{sign}{}.{}", &digits[..whole], &digits[whole..]) }
}

/// The day (and time of day) a spreadsheet counts as `serial`: days since the end of 1899, or since 1904.
fn date(serial: f64, from_1904: bool) -> Option<String> {
    if !(0.0..2_958_466.0).contains(&serial) { return None }
    let (mut days, mut seconds) = (serial.floor() as i64, ((serial - serial.floor()) * 86_400.0).round() as u32);
    if seconds >= 86_400 { (days, seconds) = (days + 1, 0) }
    let time = format!("{:02}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60);
    // A time of day alone: day 0 is no day.
    if days == 0 { return Some(time) }
    // The spreadsheet's 1900 has a 29 February it never had: days before it are counted one short.
    let start = if from_1904 { chrono::NaiveDate::from_ymd_opt(1904, 1, 1)? } else { chrono::NaiveDate::from_ymd_opt(1899, 12, if days < 60 { 31 } else { 30 })? };
    let day = start.checked_add_days(chrono::Days::new(days as u64))?.format("%Y-%m-%d").to_string();
    Some(if seconds == 0 { day } else { format!("{day} {time}") })
}

#[derive(Default)]
struct Cell {
    column: usize,
    kind: String,
    style: usize,
    value: String,
    inline: String,
}

impl Workbook {
    fn said(&self, cell: Cell) -> String {
        let value = cell.value.trim();
        match cell.kind.as_str() {
            "s" => value.parse::<usize>().ok().and_then(|at| self.shared.get(at)).cloned().unwrap_or_default(),
            "inlineStr" => spelled(&cell.inline),
            "str" => spelled(&cell.value),
            "b" => if value == "1" { "TRUE".into() } else { "FALSE".into() },
            "e" | "d" => value.to_string(),
            _ if value.is_empty() => String::new(),
            _ if self.dates.get(cell.style) == Some(&true) => value.parse::<f64>().ok().and_then(|serial| date(serial, self.from_1904)).unwrap_or_else(|| number(value)),
            _ => number(value),
        }
    }

    fn rows(&self, xml: &str, visit: &mut dyn FnMut(usize, Vec<String>) -> bool) {
        let (mut stack, mut at): (Vec<&str>, usize) = (vec![], 0);
        // A row or a cell that does not say where it stands follows the one before it.
        let (mut row, mut cells, mut cell): (usize, Vec<String>, Option<Cell>) = (0, vec![], None);
        let mut column = 0;
        while let Some((piece, end)) = node(xml, at) {
            at = end;
            match piece {
                Node::Open(name, tag) | Node::Empty(name, tag) => {
                    let name = local(name);
                    match name {
                        "row" => {
                            row = attr(tag, "r").and_then(|r| r.parse().ok()).unwrap_or(row + 1);
                            (cells, column) = (vec![], 0);
                        }
                        "c" => {
                            let here = attr(tag, "r").and_then(|r| column_of(&r)).unwrap_or(column);
                            column = here + 1;
                            if matches!(piece, Node::Open(..)) { cell = Some(Cell { column: here, kind: attr(tag, "t").unwrap_or_default(), style: attr(tag, "s").and_then(|s| s.parse().ok()).unwrap_or(0), ..Default::default() }) }
                        }
                        _ => {}
                    }
                    if matches!(piece, Node::Open(..)) { stack.push(name) }
                }
                Node::Text(text) => {
                    let Some(cell) = cell.as_mut() else { continue };
                    match stack.last() {
                        Some(&"v") => cell.value.push_str(&unescape(text)),
                        Some(&"t") if !stack.contains(&"rPh") => cell.inline.push_str(&unescape(text)),
                        _ => {}
                    }
                }
                Node::Close => match stack.pop() {
                    Some("c") => {
                        let Some(cell) = cell.take() else { continue };
                        let place = cell.column;
                        let said = self.said(cell);
                        if !said.trim().is_empty() && place < MAX_COLUMNS {
                            if cells.len() <= place { cells.resize(place + 1, String::new()) }
                            cells[place] = said;
                        }
                    }
                    Some("row") if !cells.is_empty() && !visit(row, std::mem::take(&mut cells)) => return,
                    _ => {}
                },
            }
        }
    }
}

// ---- text files

/// A text file's words, whatever it was saved as: UTF-8, the UTF-16 a spreadsheet calls "Unicode text", the
/// GB18030 of a Chinese Windows, or the Latin of a western one.
fn decoded(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) { return String::from_utf8_lossy(rest).into_owned() }
    for (mark, big) in [([0xFF, 0xFE], false), ([0xFE, 0xFF], true)] {
        if let Some(rest) = bytes.strip_prefix(&mark) {
            let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|pair| if big { u16::from_be_bytes(*pair) } else { u16::from_le_bytes(*pair) }).collect();
            return String::from_utf16_lossy(&units);
        }
    }
    if let Ok(text) = std::str::from_utf8(bytes) { return text.to_string() }
    // Chinese text is bytes above ASCII in pairs; a µ or a ± of a Latin file stands alone between ASCII letters,
    // and read as half a Chinese character it would take the letter after it along.
    let paired = bytes.split(|b| b.is_ascii()).all(|run| run.len() % 2 == 0);
    if paired {
        let (text, _, broken) = encoding_rs::GB18030.decode(bytes);
        if !broken { return text.into_owned() }
    }
    encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned()
}

/// What sets the columns of a `.csv` apart: a comma, or the semicolon or tab some programs write under that
/// name. The one the first lines hold most of, outside quotation marks.
fn delimiter_of(text: &str) -> char {
    let mut counts = [(',', 0usize), (';', 0), ('\t', 0)];
    let mut quoted = false;
    for c in text.lines().take(10).flat_map(|line| line.chars().chain(['\n'])) {
        if c == '"' { quoted = !quoted }
        if quoted { continue }
        if let Some(count) = counts.iter_mut().find(|(delimiter, _)| *delimiter == c) { count.1 += 1 }
    }
    // The comma when none is ahead: `max_by_key` keeps the last of equals.
    counts.iter().rev().max_by_key(|(_, count)| *count).map_or(',', |(delimiter, _)| *delimiter)
}

fn delimited(text: &str, delimiter: char, visit: &mut dyn FnMut(usize, Vec<String>) -> bool) {
    let mut pass = |number: usize, mut cells: Vec<String>| {
        while cells.last().is_some_and(|cell| cell.is_empty()) { cells.pop(); }
        cells.is_empty() || visit(number, cells)
    };
    // Tabs: a line is a row, and a quotation mark is one only around a whole cell. A gene's description may
    // open with a mark that nothing closes.
    if delimiter == '\t' {
        for (n, line) in text.lines().enumerate() {
            let cells = line.split('\t').map(|cell| {
                let cell = cell.trim();
                match cell.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) {
                    Some(inner) if cell.len() >= 2 => inner.replace("\"\"", "\""),
                    _ => cell.to_string(),
                }
            }).collect();
            if !pass(n + 1, cells) { return }
        }
        return;
    }
    // Commas: a cell in quotation marks may hold the delimiter and line breaks, and a mark written twice is one.
    let (mut cells, mut cell, mut quoted, mut closed, mut number) = (vec![], String::new(), false, false, 0usize);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    cell.push('"');
                    chars.next();
                }
                '"' => (quoted, closed) = (false, true),
                c => cell.push(c),
            }
        } else if c == '"' && !closed && cell.trim().is_empty() {
            cell.clear();
            quoted = true;
        } else if c == delimiter || c == '\n' || c == '\r' {
            // What a quoted cell says is kept as it stands; a bare one loses the spaces around it.
            cells.push(if closed { std::mem::take(&mut cell) } else { std::mem::take(&mut cell).trim().to_string() });
            closed = false;
            if c == delimiter { continue }
            if c == '\r' && chars.peek() == Some(&'\n') { chars.next(); }
            number += 1;
            if !pass(number, std::mem::take(&mut cells)) { return }
        } else {
            cell.push(c);
        }
    }
    if !cell.is_empty() || !cells.is_empty() {
        cells.push(if closed { cell } else { cell.trim().to_string() });
        pass(number + 1, cells);
    }
}

// ---- what a table says of itself

/// Whether a cell holds a number, as a table writes one: `12`, `-0.5`, `3.2e-10`, `1,204`, `45%`.
fn numeric(cell: &str) -> bool {
    let cell = cell.trim().trim_end_matches('%').replace(',', "");
    !cell.is_empty() && cell.parse::<f64>().is_ok()
}

/// Which of a sheet's first rows names its columns: the first that fills most of the table's width with words.
/// A table often opens with its title in one cell ("Table S3. Differentially expressed genes"), which is not it;
/// a sheet of numbers alone has none.
pub fn header(first: &[(usize, Vec<String>)]) -> Option<usize> {
    let filled = |cells: &[String]| cells.iter().filter(|cell| !cell.trim().is_empty()).count();
    let widest = first.iter().map(|(_, cells)| filled(cells)).max().filter(|widest| *widest >= 2)?;
    let at = first.iter().position(|(_, cells)| filled(cells) * 5 >= widest * 3)?;
    let cells = &first[at].1;
    (cells.iter().filter(|cell| numeric(cell)).count() * 2 <= filled(cells)).then_some(at)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A workbook as Excel writes one: each sheet's rows are cells of text (shared strings), numbers written
    /// bare (`=1.5`), dates (`@45000`) and nothing.
    pub(crate) fn workbook(sheets: &[(&str, Vec<Vec<&str>>)]) -> Vec<u8> {
        const HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
        let escape = |text: &str| text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let mut zip = Zip::default();
        let (mut shared, mut book, mut rels): (Vec<String>, String, String) = (vec![], String::new(), String::new());
        for (n, (name, rows)) in sheets.iter().enumerate() {
            let mut data = String::new();
            for (r, row) in rows.iter().enumerate() {
                if row.is_empty() { continue }
                data.push_str(&format!("<row r=\"{}\" spans=\"1:{}\">", r + 1, row.len()));
                for (c, cell) in row.iter().enumerate() {
                    let at = format!("{}{}", column_name(c), r + 1);
                    if let Some(value) = cell.strip_prefix('=') {
                        data.push_str(&format!("<c r=\"{at}\"><v>{value}</v></c>"));
                    } else if let Some(value) = cell.strip_prefix('@') {
                        data.push_str(&format!("<c r=\"{at}\" s=\"1\"><v>{value}</v></c>"));
                    } else if !cell.is_empty() {
                        let index = shared.iter().position(|known| known == cell).unwrap_or_else(|| { shared.push(cell.to_string()); shared.len() - 1 });
                        data.push_str(&format!("<c r=\"{at}\" t=\"s\"><v>{index}</v></c>"));
                    }
                }
                data.push_str("</row>");
            }
            zip.set(&format!("xl/worksheets/sheet{}.xml", n + 1), format!("{HEAD}<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><dimension ref=\"A1\"/><sheetData>{data}</sheetData></worksheet>").as_bytes());
            book.push_str(&format!("<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>", escape(name), n + 1, n + 1));
            rels.push_str(&format!("<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{}.xml\"/>", n + 1, n + 1));
        }
        let strings: String = shared.iter().map(|text| format!("<si><t xml:space=\"preserve\">{}</t></si>", escape(text))).collect();
        zip.set("[Content_Types].xml", format!("{HEAD}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>").as_bytes());
        zip.set("_rels/.rels", format!("{HEAD}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>").as_bytes());
        zip.set("xl/workbook.xml", format!("{HEAD}<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><workbookPr/><sheets>{book}</sheets></workbook>").as_bytes());
        zip.set("xl/_rels/workbook.xml.rels", format!("{HEAD}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{rels}<Relationship Id=\"rId90\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings\" Target=\"sharedStrings.xml\"/><Relationship Id=\"rId91\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>").as_bytes());
        zip.set("xl/sharedStrings.xml", format!("{HEAD}<sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">{strings}</sst>").as_bytes());
        zip.set("xl/styles.xml", format!("{HEAD}<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><cellStyleXfs count=\"1\"><xf numFmtId=\"14\"/></cellStyleXfs><cellXfs count=\"2\"><xf numFmtId=\"0\"/><xf numFmtId=\"14\" applyNumberFormat=\"1\"/></cellXfs></styleSheet>").as_bytes());
        zip.write()
    }

    fn rows_of(book: &Book, sheet: usize) -> Vec<(usize, Vec<String>)> {
        let mut out = vec![];
        book.sheet(sheet).unwrap().rows(&mut |n, cells| {
            out.push((n, cells));
            true
        });
        out
    }

    fn row(number: usize, cells: &[&str]) -> (usize, Vec<String>) {
        (number, cells.iter().map(|cell| cell.to_string()).collect())
    }

    #[test]
    fn a_workbook_is_read_sheet_by_sheet_as_a_person_reads_its_cells() {
        let bytes = workbook(&[
            ("DEGs", vec![vec!["Table S1. Genes & <more>"], vec![], vec!["Gene", "log2FC", "padj", "Sampled"], vec!["FLC", "=-2.5", "=3.2E-120", "@45000"], vec!["SOC1", "=0.30000000000000004", "", "@45000.5"]]),
            ("Primers", vec![vec!["Name", "Sequence"], vec!["FLC-F", "ATGGGA"]]),
        ]);
        let book = read(bytes, "xlsx").unwrap();
        assert_eq!(book.sheets, vec!["DEGs", "Primers"]);
        // An empty row is passed over and keeps its number; empty cells at a row's end are not there.
        assert_eq!(rows_of(&book, 0), vec![
            row(1, &["Table S1. Genes & <more>"]),
            row(3, &["Gene", "log2FC", "padj", "Sampled"]),
            row(4, &["FLC", "-2.5", "3.2e-120", "2023-03-15"]),
            row(5, &["SOC1", "0.3", "", "2023-03-15 12:00:00"]),
        ]);
        assert_eq!(rows_of(&book, 1), vec![row(1, &["Name", "Sequence"]), row(2, &["FLC-F", "ATGGGA"])]);
        // Reading stops when the visitor says so.
        let mut seen = 0;
        book.sheet(0).unwrap().rows(&mut |_, _| {
            seen += 1;
            false
        });
        assert_eq!(seen, 1);
    }

    #[test]
    fn cells_other_programs_write_are_read_too() {
        // Prefixed names, cells without a place, a string written in the cell, in several looks, a formula's
        // text, a yes, an error, a character written as a code, and Japanese reading marks.
        let sheet = "<x:worksheet xmlns:x=\"m\"><x:sheetData>\
            <x:row><x:c t=\"inlineStr\"><x:is><x:t>in &amp; line</x:t></x:is></x:c><x:c t=\"str\"><x:f>A1&amp;\"x\"</x:f><x:v>made</x:v></x:c><x:c t=\"b\"><x:v>1</x:v></x:c><x:c t=\"e\"><x:v>#N/A</x:v></x:c></x:row>\
            <x:row><x:c r=\"C2\" t=\"s\"><x:v>0</x:v></x:c><x:c t=\"s\"><x:v>1</x:v></x:c><x:c r=\"ZZZ2\"><x:v>9</x:v></x:c></x:row>\
            <x:row r=\"7\"><x:c r=\"A7\"/><x:c r=\"B7\" s=\"1\"><x:v>0.25</x:v></x:c><x:c r=\"C7\" s=\"2\"><x:v>1</x:v></x:c><x:c r=\"D7\"><x:v>1234567.125</x:v></x:c></x:row>\
            </x:sheetData></x:worksheet>";
        let mut zip = Zip::default();
        zip.set("xl/workbook.xml", b"<workbook><workbookPr date1904=\"1\"/><sheets><sheet name=\"S\" r:id=\"rId1\"/><sheet name=\"Chart\" r:id=\"rId2\"/></sheets></workbook>");
        zip.set("xl/_rels/workbook.xml.rels", b"<Relationships><Relationship Id=\"rId1\" Type=\"http://purl.oclc.org/ooxml/officeDocument/relationships/worksheet\" Target=\"/xl/worksheets/one.xml\"/><Relationship Id=\"rId2\" Type=\"x/chartsheet\" Target=\"chartsheets/sheet1.xml\"/></Relationships>");
        zip.set("xl/worksheets/one.xml", sheet.as_bytes());
        zip.set("xl/sharedStrings.xml", "<sst><si><r><t>bold</t></r><r><rPr/><t xml:space=\"preserve\"> plain</t></r></si><si><t>東京</t><rPh><t>トウキョウ</t></rPh><phoneticPr/></si></sst>".as_bytes());
        zip.set("xl/styles.xml", b"<styleSheet><numFmts><numFmt numFmtId=\"164\" formatCode=\"h:mm:ss\"/><numFmt numFmtId=\"165\" formatCode=\"[Red]0.00&quot;days&quot;\"/></numFmts><cellXfs><xf numFmtId=\"165\"/><xf numFmtId=\"164\"/><xf numFmtId=\"15\"/></cellXfs></styleSheet>");
        let book = read(zip.write(), "xlsx").unwrap();
        assert_eq!(book.sheets, vec!["S"]);
        assert_eq!(rows_of(&book, 0), vec![
            row(1, &["in & line", "made", "TRUE", "#N/A"]),
            row(2, &["", "", "bold plain", "東京"]),
            // A time of day; a day counted from 1904; a number that is one.
            row(7, &["", "06:00:00", "1904-01-02", "1234567.125"]),
        ]);
        assert_eq!(spelled("a_x000D__x000A_b_x41"), "a\nb_x41");
    }

    #[test]
    fn numbers_and_dates_are_written_as_a_spreadsheet_shows_them() {
        for (raw, shown) in [("42", "42"), ("-7", "-7"), ("0.1", "0.1"), ("0.30000000000000004", "0.3"), ("1E-3", "0.001"), ("2.5E-5", "0.000025"), ("3.2E-120", "3.2e-120"), ("-1E-6", "-1e-6"), ("1.5E+3", "1500"), ("123456789012345.6", "123456789012346"), ("1E+15", "1e15"), ("0.0", "0"), ("abc", "abc")] {
            assert_eq!(number(raw), shown, "{raw}");
        }
        assert_eq!(date(1.0, false).unwrap(), "1900-01-01");
        assert_eq!(date(59.0, false).unwrap(), "1900-02-28");
        assert_eq!(date(61.0, false).unwrap(), "1900-03-01");
        assert_eq!(date(45000.75, false).unwrap(), "2023-03-15 18:00:00");
        assert_eq!(date(0.5, false).unwrap(), "12:00:00");
        assert_eq!(date(-1.0, false), None);
        assert!(dated("yyyy-mm-dd") && dated("[$-409]d-mmm-yy;@") && dated("h:mm AM/PM"));
        assert!(!dated("General") && !dated("0.00E+00") && !dated("#,##0.00_);[Red](#,##0.00)") && !dated("0.0\"d\"") && !dated("0\\d"));
        assert_eq!((column_of("A1"), column_of("Z9"), column_of("AA10"), column_of("XFD1"), column_of("12")), (Some(0), Some(25), Some(26), Some(16_383), None));
        assert_eq!((column_name(0), column_name(25), column_name(26), column_name(701), column_name(702), column_name(16_383)), ("A".into(), "Z".into(), "AA".into(), "ZZ".into(), "AAA".into(), "XFD".into()));
    }

    #[test]
    fn text_tables_are_read_with_their_quotes_delimiters_and_encodings() {
        let read_text = |bytes: &[u8], format: &str| rows_of(&read(bytes.to_vec(), format).unwrap(), 0);
        // Quoted cells hold commas, line breaks and marks written twice; blank lines keep their numbers.
        assert_eq!(read_text(b"\xEF\xBB\xBFgene,note,n\r\nFLC,\"a, \"\"b\"\"\nc\", 3 \r\n\r\nSOC1,,\n,,\nlast", "csv"), vec![
            row(1, &["gene", "note", "n"]),
            row(2, &["FLC", "a, \"b\"\nc", "3"]),
            row(4, &["SOC1"]),
            row(6, &["last"]),
        ]);
        // A semicolon or a tab under the name of a comma.
        assert_eq!(read_text("a;b;c\n1,5;2;3\n".as_bytes(), "csv"), vec![row(1, &["a", "b", "c"]), row(2, &["1,5", "2", "3"])]);
        assert_eq!(read_text(b"a\tb\n1\t2\n", "csv"), vec![row(1, &["a", "b"]), row(2, &["1", "2"])]);
        // Tabs: a mark that nothing closes is part of the cell, and does not swallow the rest of the file.
        assert_eq!(read_text(b"id\tdescription\nAT1\t\"putative protein\nAT2\t\"quoted \"\"x\"\"\"\n", "tsv"), vec![row(1, &["id", "description"]), row(2, &["AT1", "\"putative protein"]), row(3, &["AT2", "quoted \"x\""])]);
        assert_eq!(read_text(b"a\tb\n1\t2\n", "txt"), vec![row(1, &["a", "b"]), row(2, &["1", "2"])]);
        assert!(read(b"just words\nand more".to_vec(), "txt").is_err_and(|e| e.contains("制表符")));
        // UTF-16 with its mark, GB18030, and a Latin file whose µ stands alone.
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("基因\tn\nFLC\t1\n".encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(read_text(&utf16, "tsv"), vec![row(1, &["基因", "n"]), row(2, &["FLC", "1"])]);
        assert_eq!(read_text(&encoding_rs::GB18030.encode("基因,表达量\n开花,高\n").0, "csv"), vec![row(1, &["基因", "表达量"]), row(2, &["开花", "高"])]);
        assert_eq!(read_text(b"drug,dose\nABA,50 \xB5M\n", "csv"), vec![row(1, &["drug", "dose"]), row(2, &["ABA", "50 µM"])]);
    }

    #[test]
    fn what_is_not_a_table_says_so() {
        assert!(read(b"\xD0\xCF\x11\xE0old".to_vec(), "xlsx").is_err_and(|e| e.contains("密码")));
        assert!(read(b"not a zip".to_vec(), "xlsx").is_err_and(|e| e.contains("Excel")));
        assert!(read(vec![], "xls").is_err_and(|e| e.contains("另存为")));
        assert!(read(vec![], "pdf").is_err_and(|e| e.contains(".pdf")));
        assert!(is_table("XLSX") && is_table("tsv") && !is_table("txt") && !is_table("pdf"));
    }

    #[test]
    fn the_header_is_the_first_row_of_words_across_the_table() {
        let rows = |rows: &[&[&str]]| rows.iter().enumerate().map(|(n, cells)| row(n + 1, cells)).collect::<Vec<_>>();
        assert_eq!(header(&rows(&[&["Gene", "log2FC", "padj"], &["FLC", "-2.5", "0.01"]])), Some(0));
        // A title in one cell, then the names.
        assert_eq!(header(&rows(&[&["Table S3. Differentially expressed genes"], &[], &["Gene", "log2FC", "padj", "2021"], &["FLC", "-2.5", "0.01", "5"]])), Some(2));
        // Numbers alone have no names; nor has one column.
        assert_eq!(header(&rows(&[&["1", "2", "3"], &["4", "5", "6"]])), None);
        assert_eq!(header(&rows(&[&["Gene"], &["FLC"]])), None);
        assert_eq!(header(&[]), None);
    }
}
