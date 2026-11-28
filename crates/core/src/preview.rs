//! 预览 draws a section of a manuscript as its reader reads it: the Markdown drawn, each citation as its label
//! (`cite::read_as`). What is selected there, or tinted for a comment, is a stretch of that drawing; what a comment
//! keeps, and where the editor goes, are bytes of the section's text. `Reading` ties the two together.
//!
//! Both are read as characters without white space: the drawing from the pieces it is drawn in (`markdown::texts`),
//! the text line by line as the editor reads it (`markdown::lines`), its syntax left out and each citation as the
//! label it is drawn as. They say the same characters but where a reader of Markdown draws something of its own (a
//! list's number, a table's bars), so they are laid side by side character by character, and where they part they
//! are met again a few characters on.

use crate::markdown::{self, Part, Place};
use std::ops::Range;

/// How far past a place where the drawing and the text part the two are looked along for where they agree again.
const AHEAD: usize = 48;
/// How many characters in a row must agree for the two to be in step again.
const AGREE: usize = 3;

/// A stretch of a section as the preview draws it, and the section's text it was drawn from.
#[derive(Debug)]
pub struct Reading {
    /// The characters of the text as they are drawn, each with the bytes of the text it stands for: a word's
    /// character its own bytes, each character of a citation's label the whole citation.
    written: Vec<(char, Range<usize>)>,
    /// The characters of the drawing, each with where it is drawn.
    drawn: Vec<(char, Place)>,
    /// For each of `drawn`, the one of `written` it is.
    pairs: Vec<Option<usize>>,
}

impl Reading {
    /// `shown` is the Markdown the preview draws for `within` of the section's text `body`, made with `labels`
    /// (`Formatter::labels` of that text).
    pub fn new(body: &str, labels: &[(Range<usize>, String)], within: Range<usize>, shown: &str) -> Reading {
        let written = written_chars(body, labels, within);
        let drawn: Vec<(char, Place)> = markdown::texts(shown).iter().enumerate().flat_map(|(piece, text)| text.char_indices().filter(|(_, c)| !c.is_whitespace()).map(move |(byte, c)| (c, (piece, byte))).collect::<Vec<_>>()).collect();
        let pairs = pair(&written.iter().map(|(c, _)| *c).collect::<Vec<_>>(), &drawn.iter().map(|(c, _)| *c).collect::<Vec<_>>());
        Reading { written, drawn, pairs }
    }

    /// The bytes of the text that what is drawn from `from` to `to` (places of the drawing, in order) stands for:
    /// from the first character's to the last one's. None when nothing drawn there is of the text.
    pub fn written(&self, from: Place, to: Place) -> Option<Range<usize>> {
        let ranges: Vec<&Range<usize>> = self.drawn.iter().zip(&self.pairs).filter(|((_, at), _)| from <= *at && *at < to).filter_map(|(_, paired)| paired.map(|w| &self.written[w].1)).collect();
        Some(ranges.iter().map(|range| range.start).min()?..ranges.iter().map(|range| range.end).max()?)
    }

    /// Where the bytes `range` of the text are drawn: from the place of the first character drawn of them to the
    /// end of the last. None when none of them is drawn.
    pub fn drawn(&self, range: Range<usize>) -> Option<(Place, Place)> {
        let mut inside = self.drawn.iter().zip(&self.pairs).filter(|(_, paired)| paired.is_some_and(|w| { let bytes = &self.written[w].1; bytes.start < range.end && range.start < bytes.end }));
        let first = inside.next()?;
        let &(c, (piece, byte)) = inside.next_back().unwrap_or(first).0;
        Some((first.0.1, (piece, byte + c.len_utf8())))
    }
}

/// The characters of `within` of `body` as they are drawn, each with its bytes: what `markdown::lines` reads as words
/// (no syntax, no list bullet, no picture, no rule), a citation as its label, a table without its bars and the line of
/// dashes under its head.
fn written_chars(body: &str, labels: &[(Range<usize>, String)], within: Range<usize>) -> Vec<(char, Range<usize>)> {
    // The labels `read_as` puts in: those inside the stretch, over a citation in brackets, in order without overlap.
    let mut kept: Vec<&(Range<usize>, String)> = vec![];
    for label in labels {
        let (range, _) = label;
        let fits = within.start <= range.start && range.end <= within.end && body.get(range.clone()).is_some_and(|written| written.starts_with('[') && written.ends_with(']'));
        if fits && kept.last().is_none_or(|(last, _)| last.end <= range.start) { kept.push(label) }
    }
    let lines = markdown::lines(body);
    let rows: Vec<&str> = lines.iter().map(|line| &body[line.range.clone()]).collect();
    let mut out = vec![];
    // The lines of the table being read, and the label last said: a citation in two pieces is said once.
    let (mut table, mut said): (Range<usize>, Option<usize>) = (0..0, None);
    for (at, line) in lines.iter().enumerate() {
        if line.range.end < within.start || line.range.start >= within.end { continue }
        if !table.contains(&at) {
            let rows = crate::manuscript::table_rows(&lines, &rows, at);
            if rows > 0 { table = at..at + rows }
        }
        let in_table = table.contains(&at);
        if (in_table && at == table.start + 1) || line.rule || line.picture.is_some() { continue }
        for piece in line.pieces.iter().filter(|piece| piece.part == Part::Text) {
            for (offset, c) in body[piece.range.clone()].char_indices() {
                let byte = piece.range.start + offset;
                if !within.contains(&byte) || c.is_whitespace() || (in_table && c == '|') { continue }
                if let Some(n) = kept.iter().position(|(range, _)| range.contains(&byte)) {
                    if said != Some(n) {
                        said = Some(n);
                        let (range, label) = kept[n];
                        out.extend(label.chars().filter(|c| !c.is_whitespace()).map(|c| (c, range.clone())));
                    }
                    continue;
                }
                out.push((c, byte..byte + c.len_utf8()));
            }
        }
    }
    out
}

/// For each of `drawn`, the one of `written` it is: the two in step where they say the same; where they part, met
/// again at the nearest place where the next few characters agree, the fewest passed over on either side.
fn pair(written: &[char], drawn: &[char]) -> Vec<Option<usize>> {
    let agree = |w: usize, d: usize| {
        let n = AGREE.min(written.len() - w).min(drawn.len() - d);
        n > 0 && written[w..w + n] == drawn[d..d + n] && (n == AGREE || w + n == written.len() || d + n == drawn.len())
    };
    let mut out = vec![None; drawn.len()];
    let (mut w, mut d) = (0, 0);
    while w < written.len() && d < drawn.len() {
        if written[w] == drawn[d] {
            out[d] = Some(w);
            (w, d) = (w + 1, d + 1);
            continue;
        }
        let again = (1..=2 * AHEAD).find_map(|skip| (skip.saturating_sub(AHEAD)..=skip.min(AHEAD)).map(|a| (w + a, d + skip - a)).find(|&(w, d)| w < written.len() && d < drawn.len() && agree(w, d)));
        // Nowhere near: the two are taken to say different things here, one character each.
        (w, d) = again.unwrap_or((w + 1, d + 1));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cite::read_as;
    use crate::manuscript::{picture_lines, section_markdown};

    /// What the preview draws of `within` of `body`, and the reading of it.
    fn read(body: &str, labels: &[(Range<usize>, String)], within: Range<usize>) -> (String, Reading) {
        let shown = section_markdown(&read_as(body, labels, within.clone()), 1);
        let reading = Reading::new(body, labels, within, &shown);
        (shown, reading)
    }

    /// The place of the `n`-th `word` in what is drawn, and the place after it.
    fn place_of(shown: &str, word: &str) -> (Place, Place) {
        for (piece, text) in markdown::texts(shown).iter().enumerate() {
            if let Some(byte) = text.find(word) { return ((piece, byte), (piece, byte + word.len())) }
        }
        panic!("{word:?} is not drawn in {shown:?}")
    }

    #[test]
    fn words_selected_in_the_preview_are_found_where_they_are_written() {
        let body = "# Intro\nRoots grow **towards** water.\nShoots do not.";
        let (shown, reading) = read(body, &[], 0..body.len());
        let (from, to) = place_of(&shown, "towards water");
        let range = reading.written(from, to).unwrap();
        // From the first word to the last, the bold's stars inside: what the editor would select.
        assert_eq!(&body[range.clone()], "towards** water");
        assert_eq!(reading.drawn(range), Some((from, to)));
        // Across two paragraphs, and a heading.
        let (from, _) = place_of(&shown, "Intro");
        let (_, to) = place_of(&shown, "do not");
        assert_eq!(&body[reading.written(from, to).unwrap()], "Intro\nRoots grow **towards** water.\nShoots do not");
        // Nothing drawn: nothing written.
        assert_eq!(reading.written(from, from), None);
    }

    #[test]
    fn a_citation_is_drawn_as_its_label_and_stands_for_all_of_it() {
        let body = "Roots grow [@Zhang2024 p.2 \"towards water\"] fast.";
        let cited = body.find('[').unwrap()..body.find(']').unwrap() + 1;
        let labels = vec![(cited.clone(), "(Zhang et al., 2024)".to_string())];
        let (shown, reading) = read(body, &labels, 0..body.len());
        assert_eq!(shown, "Roots grow (Zhang et al., 2024) fast.");
        let (from, _) = place_of(&shown, "grow");
        let (_, to) = place_of(&shown, "fast");
        assert_eq!(&body[reading.written(from, to).unwrap()], "grow [@Zhang2024 p.2 \"towards water\"] fast");
        // A piece of the label is the whole citation; the quotation written in it is not words of the page.
        let (from, to) = place_of(&shown, "et al");
        assert_eq!(reading.written(from, to), Some(cited.clone()));
        assert_eq!(reading.drawn(cited), Some(place_of(&shown, "(Zhang et al., 2024)")));
        // A key nothing has stays as written, and is drawn so.
        let (shown, reading) = read("See [@Nobody2020] here.", &[], 0..23);
        let (from, to) = place_of(&shown, "[@Nobody2020] here");
        assert_eq!(reading.written(from, to), Some(4..22));
    }

    #[test]
    fn what_markdown_draws_of_its_own_is_passed_over() {
        let body = "1. first item\n2. second item\n\n| Gene | Role |\n|---|---|\n| *PIN1* | auxin |\n\n- [ ] check the roots";
        let (shown, reading) = read(body, &[], 0..body.len());
        for word in ["second item", "Role", "PIN1", "auxin", "check the roots"] {
            let (from, to) = place_of(&shown, word);
            let range = reading.written(from, to).unwrap();
            assert_eq!(&body[range.clone()], word, "{word}");
            assert_eq!(reading.drawn(range), Some((from, to)), "{word}");
        }
    }

    #[test]
    fn a_stretch_between_pictures_is_read_by_itself() {
        let body = "Before the figure.\n![Roots in **soil**](figures/G1.png)\nAfter the figure, roots.";
        let picture = picture_lines(body).remove(0);
        let after = picture.line.end..body.len();
        let (shown, reading) = read(body, &[], after);
        assert_eq!(shown, "After the figure, roots.");
        let (from, to) = place_of(&shown, "roots");
        // The second "roots" of the text: the caption over it is the picture's, not this stretch's.
        assert_eq!(reading.written(from, to), Some(body.len() - 6..body.len() - 1));
        let (shown, reading) = read(body, &[], 0..picture.line.start);
        let (from, to) = place_of(&shown, "figure");
        assert_eq!(reading.written(from, to), Some(11..17));
    }

    #[test]
    fn the_two_meet_again_after_they_part() {
        // Drawn with a character the text lacks, and the text with some the drawing lacks: the rest still pairs.
        let written: Vec<char> = "abc12defghij".chars().collect();
        let drawn: Vec<char> = "abcdefXghij".chars().collect();
        let pairs = pair(&written, &drawn);
        let paired: String = pairs.iter().map(|w| w.map_or('_', |w| written[w])).collect();
        assert_eq!(paired, "abcdef_ghij");
    }
}
