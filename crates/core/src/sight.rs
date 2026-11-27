//! A page as the system's own text recognition saw it (macOS Vision,
//! Windows.Media.Ocr): words with the boxes they stand in, and no more. What
//! a paper's text needs beyond that is made here: the lines put in the order
//! they are read (columns, and what spans them), and the words as the marks
//! `pdftext` makes a page's text of, so that a page read off a picture is cut
//! into paragraphs and headings by the same rules as one that gave its own
//! text. Nothing here asks the system: `wisp_net::sight` does.

use crate::pdftext::Mark;
use serde::{Deserialize, Serialize};

/// A word as it was recognised: its text and the box it stands in (left, top, right, bottom, in points from
/// the page's top left).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Word {
    #[serde(rename = "t")]
    pub text: String,
    #[serde(rename = "b")]
    pub rect: [f32; 4],
    /// Written right after the word before it, no space between: a letter of Chinese or Japanese after
    /// another, a mark after a word.
    #[serde(rename = "j", default, skip_serializing_if = "std::ops::Not::not")]
    pub joined: bool,
}

/// A page as it was recognised: its size in points and its lines, each the words of one line in their order.
/// Kept as it came (`ocr_pages`, as JSON): the text can be made of it again, and a text layer laid over the page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    pub size: [f32; 2],
    pub lines: Vec<Vec<Word>>,
}

/// A word of a recognised line of text: where it starts in the line and how long it is, both in UTF-16 units
/// (what the systems count a string in), and whether it is written right after the one before it.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece<'a> {
    pub text: &'a str,
    pub start: usize,
    pub len: usize,
    pub joined: bool,
}

/// A line of text an engine gives whole, cut into the pieces it is asked the places of: a word where spaces
/// set words apart, and every letter of Chinese, Japanese or Korean by itself, since a line of those has no
/// spaces and a selection goes by the letter.
pub fn pieces(line: &str) -> Vec<Piece<'_>> {
    // (bytes from, bytes to, UTF-16 start, UTF-16 length, a space before it)
    let mut cut: Vec<(usize, usize, usize, usize, bool)> = vec![];
    // Where the word being read starts, in bytes and in UTF-16 units.
    let mut open: Option<(usize, usize)> = None;
    let (mut units, mut spaced) = (0, true);
    for (at, c) in line.char_indices() {
        let alone = crate::structure::is_cjk(c);
        if (c.is_whitespace() || alone) && let Some((from, start)) = open.take() {
            cut.push((from, at, start, units - start, spaced));
            spaced = false;
        }
        if c.is_whitespace() {
            spaced = true;
        } else if alone {
            cut.push((at, at + c.len_utf8(), units, c.len_utf16(), spaced));
            spaced = false;
        } else if open.is_none() {
            open = Some((at, units));
        }
        units += c.len_utf16();
    }
    if let Some((from, start)) = open { cut.push((from, line.len(), start, units - start, spaced)) }
    cut.into_iter().enumerate().map(|(n, (from, to, start, len, spaced))| Piece { text: &line[from..to], start, len, joined: n > 0 && !spaced }).collect()
}

/// Whether two words an engine gives apart are written together: the end of one and the start of the next
/// are both letters or marks of Chinese, Japanese or Korean. For an engine that gives words and no line.
pub fn written_together(before: &str, after: &str) -> bool {
    let unspaced = |c: char| crate::structure::is_cjk(c) || crate::structure::cjk_mark(c);
    before.chars().next_back().is_some_and(unspaced) && after.chars().next().is_some_and(unspaced)
}

/// Whether a page read as one that may hold Chinese holds next to none: under a tenth of its letters. Such a
/// page is read again as Latin, which an engine that was told to expect Chinese reads less well.
pub fn latin(lines: &[Vec<Word>]) -> bool {
    let letters = || lines.iter().flatten().flat_map(|word| word.text.chars());
    let han = letters().filter(|c| crate::structure::is_cjk(*c)).count();
    let plain = letters().filter(char::is_ascii_alphabetic).count();
    plain > 0 && han * 10 < han + plain
}

/// The lines of a page that was read in bands, one above the other and each a little into the next: `read`
/// is every line a band gave, with how far it stands inside its band. Where two bands read the same line (one
/// of them cut by the band's end, or both whole and a hair apart), the reading from deeper inside its band
/// is the one kept.
pub fn without_repeats(mut read: Vec<(Vec<Word>, f32)>) -> Vec<Vec<Word>> {
    read.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut kept: Vec<(Vec<Word>, [f32; 4])> = vec![];
    for (line, _) in read {
        let rect = around(&line);
        let area = |r: &[f32; 4]| (r[2] - r[0]).max(0.) * (r[3] - r[1]).max(0.);
        let same = |other: &[f32; 4]| {
            let shared = [rect[0].max(other[0]), rect[1].max(other[1]), rect[2].min(other[2]), rect[3].min(other[3])];
            area(&shared) > area(&rect).min(area(other)) * 0.5
        };
        if !kept.iter().any(|(_, other)| same(other)) { kept.push((line, rect)) }
    }
    // As a page is read: from the top, and from the left on a row.
    kept.sort_by(|a, b| middle(&a.1).total_cmp(&middle(&b.1)).then(a.1[0].total_cmp(&b.1[0])));
    kept.into_iter().map(|(line, _)| line).collect()
}

/// The page an engine's answer is kept as: its text, and what was seen as JSON.
pub fn page(seen: &Seen) -> crate::ocr::Page {
    crate::ocr::Page { text: text(seen), raw: serde_json::to_string(seen).unwrap_or_default(), cut: false }
}

/// What was kept of a page (`Page::raw`) as it was seen; none when it is another engine's answer.
pub fn kept(raw: &str) -> Option<Seen> {
    serde_json::from_str(raw).ok()
}

/// The text of a page as it is kept and asked about: one line of text per line of the page, a blank line
/// wherever the page shows a break (`pdftext::page_text`).
pub fn text(seen: &Seen) -> String {
    crate::pdftext::page_text(&marks(seen))
}

/// A stretch of a recognised line that is read as one: the line, or a part of one that an engine drew across
/// the gap between two columns.
struct Stretch<'a> {
    words: &'a [Word],
    rect: [f32; 4],
}

/// Words of a line this far apart, in line heights, stand in two columns: a space between words is a quarter
/// of that, and the loosest justified line stays under it.
const COLUMN_GAP: f32 = 1.0;

fn around(words: &[Word]) -> [f32; 4] {
    words.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |r, word| [r[0].min(word.rect[0]), r[1].min(word.rect[1]), r[2].max(word.rect[2]), r[3].max(word.rect[3])])
}

fn stretches(seen: &Seen) -> Vec<Stretch<'_>> {
    let mut out = vec![];
    for line in &seen.lines {
        let line: &[Word] = line;
        if line.iter().all(|word| word.text.trim().is_empty()) { continue }
        let whole = around(line);
        let apart = (whole[3] - whole[1]).max(1.) * COLUMN_GAP;
        let mut start = 0;
        for at in 1..=line.len() {
            if at == line.len() || line[at].rect[0] - line[at - 1].rect[2] > apart {
                out.push(Stretch { words: &line[start..at], rect: around(&line[start..at]) });
                start = at;
            }
        }
    }
    out
}

fn middle(rect: &[f32; 4]) -> f32 {
    (rect[1] + rect[3]) / 2.
}

/// A line this wide, as a share of the page, is a line of text: a cell of a table or a label in a figure is
/// narrower, and two rows of those side by side are no columns.
const TEXT_WIDE: f32 = 0.18;
/// Lines of text side by side, in all, before a gap between them is taken for the gap between two columns.
const SIDE_BY_SIDE: usize = 3;

/// How many lines of text stand side by side across `x`: between two lines that cross it, the fewer of those
/// to its left and to its right, counted only where each side has two at least. `sorted` is top to bottom.
fn side_by_side(rects: &[[f32; 4]], sorted: &[usize], x: f32, wide: f32) -> usize {
    let (mut score, mut left, mut right) = (0, 0, 0);
    let mut close = |left: &mut usize, right: &mut usize| {
        if *left >= 2 && *right >= 2 { score += (*left).min(*right) }
        (*left, *right) = (0, 0);
    };
    for &item in sorted {
        let rect = &rects[item];
        if crosses(rect, x) {
            close(&mut left, &mut right);
        } else if rect[2] - rect[0] >= wide * TEXT_WIDE {
            if (rect[0] + rect[2]) / 2. < x { left += 1 } else { right += 1 }
        }
    }
    close(&mut left, &mut right);
    score
}

fn crosses(rect: &[f32; 4], x: f32) -> bool {
    rect[0] < x - 1. && rect[2] > x + 1.
}

/// Where the gap between two columns runs among these lines, when they stand in columns: the place most
/// lines of text stand side by side across, in the middle between those to its left and to its right.
fn gutter(rects: &[[f32; 4]], sorted: &[usize], wide: f32) -> Option<f32> {
    // (lines side by side, the width of the gap, its middle)
    let mut best: Option<(usize, f32, f32)> = None;
    let mut tried: Vec<i32> = vec![];
    for &item in sorted {
        // Just past the end of a line: of the lines of a left column, the longest end in the gap.
        let x = rects[item][2] + 1.5;
        let key = (x / 2.).round() as i32;
        if tried.contains(&key) { continue }
        tried.push(key);
        let score = side_by_side(rects, sorted, x, wide);
        if score < SIDE_BY_SIDE { continue }
        // The gap itself: from the longest line at its left to the first at its right.
        let apart = sorted.iter().map(|item| &rects[*item]).filter(|rect| !crosses(rect, x));
        let left = apart.clone().filter(|rect| (rect[0] + rect[2]) / 2. < x).map(|rect| rect[2]).fold(f32::MIN, f32::max);
        let right = apart.filter(|rect| (rect[0] + rect[2]) / 2. >= x).map(|rect| rect[0]).fold(f32::MAX, f32::min);
        let clear = right - left;
        // A line that happens to end where the next column begins leaves no gap; of two places as many lines
        // stand across, the wider is the one between the columns.
        if clear >= GUTTER && best.is_none_or(|(most, widest, _)| score > most || (score == most && clear > widest)) { best = Some((score, clear, (left + right) / 2.)) }
    }
    best.map(|(.., x)| x)
}

/// The narrowest gap between two columns, in points.
const GUTTER: f32 = 4.;

/// The order the lines of a page are read in, from the boxes they stand in: a page set in columns is read
/// one column after the other, and what spans the columns (a title, an abstract, a legend) where it stands,
/// after the columns above it and before those below. `wide` is the page's width.
pub(crate) fn reading_order(rects: &[[f32; 4]], wide: f32) -> Vec<usize> {
    let mut out = Vec::with_capacity(rects.len());
    let mut all: Vec<usize> = (0..rects.len()).collect();
    all.sort_by(|a, b| middle(&rects[*a]).total_cmp(&middle(&rects[*b])));
    read(rects, all, wide, 0, &mut out);
    out
}

/// `sorted` (top to bottom) in the order it is read, onto `out`.
fn read(rects: &[[f32; 4]], sorted: Vec<usize>, wide: f32, depth: usize, out: &mut Vec<usize>) {
    // Too few lines to stand in columns, or columns within columns beyond what a page has: read as they stand.
    if sorted.len() <= SIDE_BY_SIDE || depth >= 8 { return rows(rects, sorted, out) }
    let Some(x) = gutter(rects, &sorted, wide) else { return rows(rects, sorted, out) };
    // What stands between two lines that cross the gap: its left side, then its right, when both have lines.
    let band = |band: &mut Vec<usize>, out: &mut Vec<usize>| {
        let (left, right): (Vec<usize>, Vec<usize>) = std::mem::take(band).into_iter().partition(|item| (rects[*item][0] + rects[*item][2]) / 2. < x);
        if left.is_empty() || right.is_empty() {
            // One side only: nothing to tell apart at this gap; a gap of its own is looked for.
            let one = if left.is_empty() { right } else { left };
            if one.len() < sorted.len() { read(rects, one, wide, depth + 1, out) } else { rows(rects, one, out) }
        } else {
            read(rects, left, wide, depth + 1, out);
            read(rects, right, wide, depth + 1, out);
        }
    };
    let mut held = vec![];
    for &item in &sorted {
        if crosses(&rects[item], x) {
            band(&mut held, out);
            out.push(item);
        } else {
            held.push(item);
        }
    }
    band(&mut held, out);
}

/// Lines that stand in one column, top to bottom; those on one row left to right.
fn rows(rects: &[[f32; 4]], sorted: Vec<usize>, out: &mut Vec<usize>) {
    let mut row: Vec<usize> = vec![];
    for item in sorted {
        if let Some(first) = row.first().map(|first| &rects[*first]) {
            let rect = &rects[item];
            // On the row while its middle is within the row's first line.
            if (middle(rect) - middle(first)).abs() > (first[3] - first[1]).min(rect[3] - rect[1]) * 0.5 {
                row.sort_by(|a, b| rects[*a][0].total_cmp(&rects[*b][0]));
                out.append(&mut row);
            }
        }
        row.push(item);
    }
    row.sort_by(|a, b| rects[*a][0].total_cmp(&rects[*b][0]));
    out.append(&mut row);
}

/// How much taller a line's box is than those of the page's text before it counts as set in a larger size,
/// and how much lower before it counts as set in a smaller: an engine's boxes are no measure of the letters.
/// The lines of one paragraph come a third apart, and a short line without a tall letter (`measurement.`) in
/// a box two thirds as high as its neighbours'.
const LARGER: f32 = 1.4;
const SMALLER: f32 = 0.5;
/// From one size to the next.
const SIZE_STEP: f32 = 1.5;
/// The box an engine draws around a line of text, in sizes of its letters.
const BOX: f32 = 1.2;

/// The height most of the page's text stands in, by letters and not by lines: the text of a page outweighs
/// its few titles and its many labels.
fn usual_height(stretches: &[Stretch]) -> f32 {
    let mut weighed: Vec<(f32, usize)> = stretches.iter().map(|stretch| ((stretch.rect[3] - stretch.rect[1]).max(1.), stretch.words.iter().map(|word| word.text.chars().count()).sum())).collect();
    weighed.sort_by(|a, b| a.0.total_cmp(&b.0));
    let half = weighed.iter().map(|(_, letters)| letters).sum::<usize>() / 2;
    let mut seen = 0;
    weighed.iter().find(|(_, letters)| { seen += letters; seen > half }).map_or(10., |(height, _)| *height)
}

/// The size a line is set in, from the height of its box: that of the page's text unless it is plainly
/// larger (a title) or smaller, and then a step from it. The lines of a paragraph come out alike.
fn size(stretch: &Stretch, usual: f32) -> f32 {
    let taller = (stretch.rect[3] - stretch.rect[1]).max(1.) / usual;
    let steps = (taller.ln() / SIZE_STEP.ln()).round();
    let steps = if taller >= LARGER { steps.max(1.) } else if taller <= SMALLER { steps.min(-1.) } else { 0. };
    usual / BOX * SIZE_STEP.powf(steps)
}

/// Lines one under the other, as an engine places them, put where a page sets them: an engine's boxes stand
/// a point higher or lower and further left or right from line to line, which a page's lines do not, and
/// `pdftext` tells a paragraph's end by just such differences. `lines` is (left edge, middle, right edge,
/// size) in the order the lines are read. Lines a like distance apart are spread evenly between the first
/// and the last of them, and those that start within a letter's width of most start where most do.
fn straightened(lines: &mut [(f32, f32, f32, f32)]) {
    let mut start = 0;
    for at in 1..=lines.len() {
        // A run of lines of one column: each under the one before it, by a line's distance or two, set alike.
        let follows = at < lines.len() && {
            let (last, line) = (lines[at - 1], lines[at]);
            let gap = line.1 - last.1;
            line.3 == last.3 && gap > last.3 * 0.8 && gap < last.3 * 3.2 && line.0 < last.2 && last.0 < line.2
        };
        if follows { continue }
        let run = &mut lines[start..at];
        start = at;
        if run.len() < 3 { continue }
        let size = run[0].3;
        let mut gaps: Vec<f32> = run.windows(2).map(|pair| pair[1].1 - pair[0].1).collect();
        gaps.sort_by(f32::total_cmp);
        let usual = gaps[gaps.len() / 2];
        // Within the run, each stretch of lines the usual distance apart.
        let mut from = 0;
        for to in 1..=run.len() {
            if to < run.len() && ((run[to].1 - run[to - 1].1) - usual).abs() <= usual * 0.2 { continue }
            if to - from >= 3 {
                let (first, step) = (run[from].1, (run[to - 1].1 - run[from].1) / (to - from - 1) as f32);
                for (n, line) in run[from..to].iter_mut().enumerate() { line.1 = first + step * n as f32 }
            }
            from = to;
        }
        let mut lefts: Vec<f32> = run.iter().map(|line| line.0).collect();
        lefts.sort_by(f32::total_cmp);
        let left = lefts[lefts.len() / 2];
        for line in run.iter_mut() {
            if (line.0 - left).abs() <= size * 0.35 { line.0 = left }
        }
    }
}

/// The words of a page as the marks `pdftext` reads a page from, in the order they are read: a mark a
/// letter, the letters of a word spread evenly over its box, and a space where the engine read one. What a
/// selection, a quotation looked up and a figure's number go by on a page that gave its own text.
pub fn marks(seen: &Seen) -> Vec<Mark> {
    let stretches = stretches(seen);
    let rects: Vec<[f32; 4]> = stretches.iter().map(|stretch| stretch.rect).collect();
    let usual = usual_height(&stretches);
    let order = reading_order(&rects, seen.size[0]);
    let mut lines: Vec<(f32, f32, f32, f32)> = order.iter().map(|index| (rects[*index][0], middle(&rects[*index]), rects[*index][2], size(&stretches[*index], usual))).collect();
    straightened(&mut lines);
    let mut out: Vec<Mark> = vec![];
    for (index, (left, middle, _, size)) in order.into_iter().zip(lines) {
        let stretch = &stretches[index];
        // The middle of a line's box is a little above the middle of its letters' height: the baseline under it.
        let y = middle + size * 0.3;
        // No engine says which lines are bold. A line that names a part of the paper (`3. Results`, `1 引言`) is a
        // heading whatever it looks like, and is marked the way a page marks one, so that it stands by itself.
        let said = stretch.words.iter().fold(String::new(), |said, word| if said.is_empty() || word.joined { said + &word.text } else { said + " " + &word.text });
        let bold = said.chars().count() <= 60 && crate::structure::canonical(&said).is_some();
        let mut last: Option<f32> = None;
        for word in stretch.words {
            let letters: Vec<char> = word.text.chars().filter(|c| !c.is_whitespace()).collect();
            if letters.is_empty() { continue }
            let (x0, x1) = (word.rect[0], word.rect[2].max(word.rect[0] + 0.1));
            // The words of a line follow one another: one the engine places before the end of the last starts there.
            let x0 = match last {
                Some(last) => x0.max(last).min(x1 - 0.1),
                None => left.min(x1 - 0.1),
            };
            if let Some(last) = last {
                if word.joined {
                    // No gap for `pdftext` to read a space into.
                    if let Some(before) = out.last_mut() { before.x1 = x0 }
                } else {
                    out.push(Mark { text: " ".into(), x0: last, x1: x0, y, size, bold });
                }
            }
            let step = (x1 - x0) / letters.len() as f32;
            for (n, letter) in letters.iter().enumerate() {
                out.push(Mark { text: letter.to_string(), x0: x0 + step * n as f32, x1: x0 + step * (n + 1) as f32, y, size, bold });
            }
            last = Some(x1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line of words set at `x`, its foot at `y`, in letters `size` points tall and half as wide.
    fn line(text: &str, x: f32, y: f32, size: f32) -> Vec<Word> {
        let mut at = x;
        text.split(' ').map(|word| {
            let wide = word.chars().count() as f32 * size * 0.5;
            let made = Word { text: word.into(), rect: [at, y - size, at + wide, y], joined: false };
            at += wide + size * 0.25;
            made
        }).collect()
    }

    fn words(lines: &[Vec<Word>]) -> Vec<String> {
        let seen = Seen { size: [600., 800.], lines: lines.to_vec() };
        let stretches = stretches(&seen);
        let rects: Vec<[f32; 4]> = stretches.iter().map(|stretch| stretch.rect).collect();
        reading_order(&rects, 600.).into_iter().map(|index| stretches[index].words.iter().map(|word| word.text.as_str()).collect::<Vec<_>>().join(" ")).collect()
    }

    /// Two columns of `rows` lines from `top` down, as an engine gives them: row by row, left then right.
    fn two_columns(rows: usize, top: f32) -> Vec<Vec<Word>> {
        (0..rows).flat_map(|row| {
            let y = top + row as f32 * 12.;
            [line(&format!("left line number {row} of the column"), 50., y, 10.), line(&format!("right line number {row} of the column"), 320., y, 10.)]
        }).collect()
    }

    #[test]
    fn columns_are_read_one_after_the_other() {
        let read = words(&two_columns(4, 200.));
        assert_eq!(read.iter().map(|line| &line[..line.find(" of").unwrap()]).collect::<Vec<_>>(), ["left line number 0", "left line number 1", "left line number 2", "left line number 3", "right line number 0", "right line number 1", "right line number 2", "right line number 3"]);
        // A short last line of the left column, and one of the right that starts indented, change nothing.
        let mut ragged = two_columns(4, 200.);
        ragged[6] = line("left end.", 50., 236., 10.);
        ragged[1] = line("right indented line of the column", 335., 200., 10.);
        let read = words(&ragged);
        assert_eq!((read[3].as_str(), read[4].as_str()), ("left end.", "right indented line of the column"));
    }

    #[test]
    fn what_spans_the_columns_is_read_where_it_stands() {
        // A title and an abstract over two columns, a legend across them halfway down, a page number under them.
        let mut page = vec![line("A title that runs over both of the columns of the page", 50., 60., 16.), line("An abstract set across the whole width of the page under it", 50., 90., 10.)];
        page.extend(two_columns(3, 150.));
        page.push(line("Figure 1. A legend that is as wide as the two columns together.", 50., 260., 10.));
        page.extend(two_columns(3, 300.));
        page.push(line("12", 290., 760., 10.));
        let read = words(&page);
        let starts: Vec<&str> = read.iter().map(|line| line.split(' ').next().unwrap()).collect();
        assert_eq!(starts, ["A", "An", "left", "left", "left", "right", "right", "right", "Figure", "left", "left", "left", "right", "right", "right", "12"]);
    }

    #[test]
    fn three_columns_and_a_line_drawn_across_two() {
        let page: Vec<Vec<Word>> = (0..4).flat_map(|row| {
            let y = 100. + row as f32 * 12.;
            [line(&format!("first column line {row} goes here"), 40., y, 10.), line(&format!("second column line {row} goes on"), 230., y, 10.), line(&format!("third column line {row} ends here"), 420., y, 10.)]
        }).collect();
        let read = words(&page);
        assert_eq!(read.iter().map(|line| line.split(' ').next().unwrap()).collect::<Vec<_>>(), ["first", "first", "first", "first", "second", "second", "second", "second", "third", "third", "third", "third"]);
        // An engine that takes the two lines of a row for one: cut at the gap, and read as two.
        let drawn_across: Vec<Vec<Word>> = (0..4).map(|row| {
            let y = 200. + row as f32 * 12.;
            let mut both = line(&format!("left line number {row} of the column"), 50., y, 10.);
            both.extend(line(&format!("right line number {row} of the column"), 320., y, 10.));
            both
        }).collect();
        let read = words(&drawn_across);
        assert_eq!(read.len(), 8);
        assert!(read[..4].iter().all(|line| line.starts_with("left")) && read[4..].iter().all(|line| line.starts_with("right")), "{read:?}");
    }

    #[test]
    fn a_table_and_a_running_head_are_read_by_rows() {
        // Cells side by side under a paragraph: each row left to right, not one column of cells after the other.
        let mut page = vec![line("A paragraph of the text that fills the width of the page from side to side", 50., 100., 10.)];
        for (row, (gene, change)) in [("RSL4", "2.4"), ("RHD6", "1.1"), ("LRL3", "0.7"), ("GL2", "0.2")].into_iter().enumerate() {
            let y = 140. + row as f32 * 12.;
            page.push(line(gene, 60., y, 10.));
            page.push(line(change, 300., y, 10.));
        }
        assert_eq!(words(&page)[1..], ["RSL4", "2.4", "RHD6", "1.1", "LRL3", "0.7", "GL2", "0.2"]);
        // A journal's name at the left and a page number at the right of one row.
        let head = [line("Plant Physiology", 50., 40., 9.), line("1042", 500., 40., 9.), line("The text of the page begins here and goes on for a line", 50., 80., 10.)];
        assert_eq!(words(&head), ["Plant Physiology", "1042", "The text of the page begins here and goes on for a line"]);
        assert!(words(&[]).is_empty());
    }

    #[test]
    fn a_page_seen_becomes_the_text_of_a_page() {
        // Boxes as an engine draws them: a point higher or lower from line to line, one line in a box a
        // fifth taller than the next, a short line without a tall letter in a low one.
        let mut lines = vec![line("Root hair growth", 50., 80., 16.)];
        lines.extend([line("Root hairs grow longer when phosphate", 50., 120., 10.), line("is scarce and stop soon in rich media", 50., 132.8, 11.5), line("measurement.", 50.8, 143.6, 7.)]);
        // A heading that names a part, set like the text and no further from it than a line is from the next.
        lines.push(line("2. Methods", 49.4, 156., 10.));
        lines.extend([line("Seeds were sown on plates and kept", 50., 168., 10.), line("in the dark for two days before use.", 51., 180.5, 10.)]);
        lines.extend((0..3).map(|row| line(&format!("right line number {row} of the column"), 320., 120. + row as f32 * 12., 10.)));
        let seen = Seen { size: [600., 800.], lines };
        let made = text(&seen);
        // The title alone, since it is set larger; the lines of a paragraph together whatever their boxes; the
        // heading by itself; and the columns one after the other.
        assert_eq!(made, "Root hair growth\n\nRoot hairs grow longer when phosphate\nis scarce and stop soon in rich media\nmeasurement.\n\n2. Methods\n\nSeeds were sown on plates and kept\nin the dark for two days before use.\n\nright line number 0 of the column\nright line number 1 of the column\nright line number 2 of the column");
        // The lines of a column stand evenly under one another and start at one edge, as a page sets them.
        let marks = marks(&seen);
        let starts: Vec<&Mark> = marks.iter().enumerate().filter(|(at, mark)| *at == 0 || crate::pdftext::starts_line(&marks[at - 1], mark)).map(|(_, mark)| mark).collect();
        let steps: Vec<f32> = starts[1..7].windows(2).map(|pair| pair[1].y - pair[0].y).collect();
        assert!(steps.iter().all(|step| (step - steps[0]).abs() < 0.01 && (11.5..12.6).contains(step)), "{steps:?}");
        assert!(starts[1..7].iter().all(|mark| mark.x0 == 50. && mark.size == starts[1].size) && starts[0].size > starts[1].size * 1.4, "{starts:?}");
        assert!(starts[4].bold && !starts[3].bold && !starts[5].bold);
        // Kept as it was seen, and read back.
        let kept_as = page(&seen);
        assert_eq!((kept_as.text.as_str(), kept_as.cut), (made.as_str(), false));
        assert!(kept_as.raw.starts_with(r#"{"size":[600.0,800.0],"lines":[[{"t":"Root","b":[50.0,64.0,82.0,80.0]},{"t":"hair","b":["#), "{}", kept_as.raw);
        assert_eq!(kept(&kept_as.raw), Some(seen));
        assert_eq!(kept("# A heading DeepSeek wrote"), None);
    }

    #[test]
    fn a_heading_in_chinese_stands_by_itself() {
        // A letter a word, as an engine gives a line of Chinese.
        let zh = |text: &str, x: f32, foot: f32| -> Vec<Word> {
            text.chars().filter(|c| !c.is_whitespace()).enumerate().map(|(n, c)| Word { text: c.to_string(), rect: [x + n as f32 * 10., foot - 12., x + n as f32 * 10. + 9.5, foot], joined: n > 0 }).collect()
        };
        let lines = vec![zh("根毛最终的长度可以通过两种方式改变细胞可以", 50., 100.), zh("长得更快也可以长得更久。", 50., 115.), zh("2材料与方法", 50., 130.), zh("野生型种子经表面消毒后在低温黑暗中放置两天", 50., 145.), zh("播种在二分之一浓度培养基的平板上。", 50., 160.)];
        assert_eq!(text(&Seen { size: [600., 800.], lines }), "根毛最终的长度可以通过两种方式改变细胞可以\n长得更快也可以长得更久。\n\n2材料与方法\n\n野生型种子经表面消毒后在低温黑暗中放置两天\n播种在二分之一浓度培养基的平板上。");
    }

    #[test]
    fn a_line_two_bands_read_is_kept_once() {
        let at = |text: &str, x: f32, y: f32, tall: f32| vec![Word { text: text.into(), rect: [x, y, x + 0.4, y + tall], joined: false }];
        // The line at the end of a band: cut short by the upper band, whole in the lower; and one both read whole, a hair apart.
        let read = vec![
            (at("above", 0.1, 0.300, 0.012), 0.07),
            (at("cut", 0.1, 0.330, 0.006), 0.0),
            (at("both", 0.1, 0.345, 0.012), 0.015),
            (at("both.", 0.1, 0.3445, 0.012), 0.025),
            (at("whole", 0.1, 0.329, 0.012), 0.04),
            (at("beside", 0.55, 0.329, 0.012), 0.04),
            (at("below", 0.1, 0.360, 0.012), 0.06),
        ];
        let kept: Vec<String> = without_repeats(read).into_iter().map(|line| line[0].text.clone()).collect();
        assert_eq!(kept, ["above", "whole", "beside", "both.", "below"]);
        assert!(without_repeats(vec![]).is_empty());
        // A page of English with a Chinese name on it is Latin; an abstract in Chinese with its gene names is not.
        let said = |text: &str| vec![vec![Word { text: text.into(), ..Default::default() }]];
        assert!(latin(&said("Root hairs grow longer when phosphate is scarce (张伟, 2021).")));
        assert!(!latin(&said("低磷条件下 RSL4 的表达升高，根毛伸长。")) && !latin(&said("12 345")) && !latin(&[]));
    }

    #[test]
    fn a_line_is_cut_into_the_pieces_it_is_asked_the_places_of() {
        let cut = |line: &str| pieces(line).into_iter().map(|piece| (piece.text.to_string(), piece.start, piece.len, piece.joined)).collect::<Vec<_>>();
        assert_eq!(cut("Root hairs  grow."), [("Root".to_string(), 0, 4, false), ("hairs".to_string(), 5, 5, false), ("grow.".to_string(), 12, 5, false)]);
        // A letter of Chinese alone, written after the one before; a Latin word among them as one; a mark after its letter.
        assert_eq!(cut("低磷下RSL4 的表达。"), [("低".to_string(), 0, 1, false), ("磷".to_string(), 1, 1, true), ("下".to_string(), 2, 1, true), ("RSL4".to_string(), 3, 4, true), ("的".to_string(), 8, 1, false), ("表".to_string(), 9, 1, true), ("达".to_string(), 10, 1, true), ("。".to_string(), 11, 1, true)]);
        // Counted as the systems count: a letter outside the first plane is two units.
        assert_eq!(cut("𝛼 helix"), [("𝛼".to_string(), 0, 2, false), ("helix".to_string(), 3, 5, false)]);
        assert!(cut("  \t ").is_empty() && cut("").is_empty());
        assert!(written_together("磷", "条") && written_together("达", "。") && written_together("。", "根"));
        assert!(!written_together("下", "RSL4") && !written_together("Root", "hairs") && !written_together("", "根"));
    }

    #[test]
    fn letters_written_together_stay_together() {
        // Chinese, a letter a word as an engine gives it, with a Latin word and a figure among them.
        let letter = |text: &str, x: f32, joined: bool| Word { text: text.into(), rect: [x, 100., x + if text.chars().count() > 1 { 22. } else { 10. }, 112.], joined };
        let lines = vec![vec![letter("低", 50., false), letter("磷", 62.5, true), letter("条", 75., true), letter("件", 87.5, true), letter("下", 100., true), letter("RSL4", 114., false), letter("的", 140., false), letter("表", 152.5, true), letter("达", 165., true), letter("。", 176., true)]];
        let seen = Seen { size: [600., 800.], lines };
        assert_eq!(text(&seen), "低磷条件下 RSL4 的表达。");
        // One mark a letter, a space between words, and a line's marks on one baseline.
        let made = marks(&seen);
        assert_eq!(made.iter().map(|mark| mark.text.as_str()).collect::<String>(), "低磷条件下 RSL4 的表达。");
        assert!(made.iter().all(|mark| mark.y == made[0].y && mark.size == made[0].size && mark.x1 >= mark.x0));
        assert_eq!(crate::pdftext::marks_text(&made[..5]), "低磷条件下");
        // An engine's `joined` is kept short: left out where it is not set.
        assert_eq!(serde_json::to_string(&letter("磷", 62.5, true)).unwrap(), r#"{"t":"磷","b":[62.5,100.0,72.5,112.0],"j":true}"#);
    }
}
