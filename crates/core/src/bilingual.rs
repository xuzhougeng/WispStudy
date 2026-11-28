//! 译文对照: a manuscript read through with each paragraph's translation beside it.
//!
//! A section's text is cut into the blocks its preview draws (those of `manuscript::section_markdown`: a line
//! is a paragraph, the items of a list, the rows of a table and the lines of fenced code stay together), and
//! each block that says something is translated by itself, so a paragraph changed is the only one asked for
//! again. What is sent has a placeholder (`[@1]`) in each citation's place: a key and the sentence quoted in a
//! citation are not for a model to rewrite, and the request stays the same when the citations are numbered
//! anew. `restored` puts back into the answer what the preview reads there.

use crate::{manuscript, markdown, model};
use std::ops::Range;

/// What a block of a section is to its translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Words to translate: a paragraph, a heading, a list, a table.
    Words,
    /// A picture on a line of its own: its caption is what is translated.
    Picture,
    /// Fenced code and rules: the same in any language.
    Kept,
}

/// A block of a section's text, as 译文对照 sets it beside its translation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// In bytes of the section's text.
    pub range: Range<usize>,
    pub kind: Kind,
    /// What is asked to be translated, each citation a placeholder; empty when nothing is.
    pub sent: String,
    /// Where the citations behind the placeholders are written, in bytes of the section's text: the first is `[@1]`.
    pub cites: Vec<Range<usize>>,
}

/// The citations written in `text`, which begins at byte `base` of its section.
fn cites_in(text: &str, base: usize) -> Vec<Range<usize>> {
    markdown::lines(text).iter().flat_map(|line| line.pieces.iter().filter(|piece| piece.cite).map(|piece| base + piece.range.start..base + piece.range.end)).collect()
}

/// The stretch `range` of `body` with a placeholder in each citation's place, and where those citations are written.
fn with_placeholders(body: &str, range: Range<usize>) -> (String, Vec<Range<usize>>) {
    let cites = cites_in(&body[range.clone()], range.start);
    let (mut sent, mut at) = (String::new(), range.start);
    for (n, cite) in cites.iter().enumerate() {
        sent.push_str(&body[at..cite.start]);
        sent.push_str(&format!("[@{}]", n + 1));
        at = cite.end;
    }
    sent.push_str(&body[at..range.end]);
    (sent, cites)
}

/// The blocks of a section's text, in order: what its preview draws one under the other. Blank lines are none.
pub fn blocks(body: &str) -> Vec<Block> {
    let lines = markdown::lines(body);
    let written: Vec<&str> = lines.iter().map(|line| &body[line.range.clone()]).collect();
    // Each block's range and kind, and whether it is a list: the words are read once the ranges are whole.
    let mut cut: Vec<(Range<usize>, Kind, bool)> = vec![];
    // What the block being made is: a list, code, or neither.
    let (mut list, mut code) = (false, false);
    let mut at = 0;
    while at < lines.len() {
        let line = &lines[at];
        let rows = manuscript::table_rows(&lines, &written, at);
        if rows > 0 {
            cut.push((line.range.start..lines[at + rows - 1].range.end, Kind::Words, false));
            (list, code) = (false, false);
            at += rows;
            continue;
        }
        at += 1;
        if line.code {
            match cut.last_mut().filter(|_| code) {
                Some(block) => block.0.end = line.range.end,
                None => cut.push((line.range.clone(), Kind::Kept, false)),
            }
            (list, code) = (false, true);
            continue;
        }
        code = false;
        if body[line.range.clone()].trim().is_empty() {
            list = false;
            continue;
        }
        if line.picture.is_some() || line.rule {
            cut.push((line.range.clone(), if line.rule { Kind::Kept } else { Kind::Picture }, false));
            list = false;
            continue;
        }
        if line.item.is_some() {
            match cut.last_mut().filter(|_| list) {
                Some(block) => block.0.end = line.range.end,
                None => cut.push((line.range.clone(), Kind::Words, true)),
            }
            list = true;
            continue;
        }
        list = false;
        cut.push((line.range.clone(), Kind::Words, false));
    }
    cut.into_iter()
        .map(|(range, kind, list)| {
            let (sent, cites) = match kind {
                Kind::Kept => (String::new(), vec![]),
                Kind::Words => with_placeholders(body, range.clone()),
                // A picture says its caption; one without a caption says nothing.
                Kind::Picture => markdown::picture_line(&body[range.clone()]).map_or((String::new(), vec![]), |(caption, _)| with_placeholders(body, range.start + caption.start..range.start + caption.end)),
            };
            // A list keeps how far its first item is set in: the items under it are read by that.
            let sent = if list { sent.trim_end() } else { sent.trim() };
            Block { range, kind, sent: sent.to_string(), cites }
        })
        .collect()
}

/// A block's translation as the preview draws it: in each placeholder's place how that citation reads
/// (`labels`: `cite::Formatter::labels` of the section's text), or the citation as it is written when it has no
/// label. A placeholder the answer left out is a citation the translation does not show; one it made up stays.
pub fn restored(translated: &str, body: &str, block: &Block, labels: &[(Range<usize>, String)]) -> String {
    let (mut out, mut rest) = (String::new(), translated);
    while let Some(at) = rest.find("[@") {
        let after = &rest[at + 2..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let cite = after[..digits].parse::<usize>().ok().filter(|n| *n > 0 && after[digits..].starts_with(']')).and_then(|n| block.cites.get(n - 1));
        match cite {
            Some(cite) => {
                out.push_str(&rest[..at]);
                out.push_str(labels.iter().find(|(range, _)| range == cite).map_or(&body[cite.clone()], |(_, label)| label.as_str()));
                rest = &after[digits + 1..];
            }
            None => {
                out.push_str(&rest[..at + 2]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The instructions a block is translated with, into the language of `code` (`manuscript::LANGUAGES`).
pub fn prompt(code: &str) -> String {
    let language = if code == "zh" { "简体中文" } else { model::label(&manuscript::LANGUAGES, code) };
    format!(
        "你是翻译助手。把用户给出的一段论文稿件译成{language}：意思准确，用这个领域的学术论文惯用的说法，不逐字直译。\n\
         保留 Markdown 记号（标题开头的 #、**加粗**、列表的记号、表格的 | 和它的每一行）、公式、数字、基因名和物种学名原样。形如 [@1] 的记号是引用的占位，原样留在译文里对应的位置。\n\
         只输出译文，不加解释、不加引号。"
    )
}

/// The instructions a block (`sent`) is translated with: `prompt`, and after it the glossary's terms that occur in
/// the block (`terms`: `glossary::pairs` from the manuscript's language into `code`) and the manuscript's standing
/// requirement. A block with no term, in a manuscript with no requirement, has the instructions of before the
/// glossary, so its kept translation still matches; a term changed asks again only for the blocks that hold it.
pub fn prompt_for(code: &str, sent: &str, terms: &[(String, String)], requirement: &str) -> String {
    crate::glossary::instructions(&prompt(code), &crate::glossary::occurring(sent, terms), requirement)
}

/// The language a manuscript written in `written` is set beside, a code of `manuscript::LANGUAGES`: the one
/// the person chose (`chosen`), else the reader's own (`reader`: the interface's); when that is the language it
/// is written in, English for a Chinese manuscript and Chinese for any other.
pub fn language(chosen: &str, written: &str, reader: &str) -> &'static str {
    let other = |wanted: &str| manuscript::LANGUAGES.iter().map(|(code, _)| *code).find(|code| *code == wanted && *code != written);
    other(chosen).or_else(|| other(reader)).unwrap_or(if written == "zh" { "en" } else { "zh" })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "Roots grow slowly in compacted soil [@Zhang2024 p.3 \"slow growth\"], as others found [@Li2019; @Wang2020].\n\n## Root hairs\n\n- first item [@Li2019]\n- second item\n\n| Line | Length |\n| --- | --- |\n| WT | 12 |\n\n```\nlet x = 1;\n\nlet y = 2;\n```\n\n![Root hairs of the mutant [@Zhang2024]](figures/G1.png)\n\n![](figures/G2.png)\n\n---\nLast words.";

    #[test]
    fn a_section_is_cut_as_its_preview_draws_it() {
        let blocks = blocks(BODY);
        let said: Vec<(Kind, &str)> = blocks.iter().map(|block| (block.kind, block.sent.as_str())).collect();
        assert_eq!(
            said,
            vec![
                (Kind::Words, "Roots grow slowly in compacted soil [@1], as others found [@2]."),
                (Kind::Words, "## Root hairs"),
                (Kind::Words, "- first item [@1]\n- second item"),
                (Kind::Words, "| Line | Length |\n| --- | --- |\n| WT | 12 |"),
                (Kind::Kept, ""),
                (Kind::Picture, "Root hairs of the mutant [@1]"),
                (Kind::Picture, ""),
                (Kind::Kept, ""),
                (Kind::Words, "Last words."),
            ]
        );
        // The blocks are where they stand in the text, the code with its fences and the blank line in it.
        assert_eq!(&BODY[blocks[4].range.clone()], "```\nlet x = 1;\n\nlet y = 2;\n```");
        assert_eq!(&BODY[blocks[0].cites[0].clone()], "[@Zhang2024 p.3 \"slow growth\"]");
        assert_eq!(&BODY[blocks[0].cites[1].clone()], "[@Li2019; @Wang2020]");
        assert_eq!(&BODY[blocks[5].cites[0].clone()], "[@Zhang2024]");
        assert!(super::blocks("").is_empty() && super::blocks("\n \n").is_empty());
    }

    #[test]
    fn a_translation_reads_its_citations_as_the_preview_does() {
        let blocks = blocks(BODY);
        let first = &blocks[0];
        // The second citation has a label, the first names a paper nothing has: it stays as written.
        let labels = vec![(first.cites[1].clone(), "(Li, 2019; Wang, 2020)".to_string())];
        let translated = "前人发现 [@2]，根在紧实土壤中生长缓慢 [@1]。";
        assert_eq!(restored(translated, BODY, first, &labels), "前人发现 (Li, 2019; Wang, 2020)，根在紧实土壤中生长缓慢 [@Zhang2024 p.3 \"slow growth\"]。");
        // A placeholder the model made up, and brackets that are none, stay.
        assert_eq!(restored("甲 [@3] 乙 [@x] 丙 [@", BODY, first, &labels), "甲 [@3] 乙 [@x] 丙 [@");
        assert_eq!(restored("没有引用。", BODY, first, &labels), "没有引用。");
    }

    #[test]
    fn the_language_beside_is_not_the_one_written_in() {
        // The reader's own, unless the manuscript is in it.
        assert_eq!(language("", "en", "zh"), "zh");
        assert_eq!(language("", "zh", "zh"), "en");
        assert_eq!(language("", "en", "en"), "zh");
        assert_eq!(language("", "de", "en"), "en");
        // The one chosen, while it is another than the manuscript's and one there is.
        assert_eq!(language("ja", "en", "zh"), "ja");
        assert_eq!(language("en", "en", "zh"), "zh");
        assert_eq!(language("xx", "en", "zh"), "zh");
        assert!(prompt("zh").contains("译成简体中文") && prompt("de").contains("译成德文") && prompt("en").starts_with("你是翻译助手"));
        // Only the terms a block holds are in its instructions; with none and no requirement, they are the plain ones.
        let terms = vec![("微环境".to_string(), "niche".to_string()), ("根毛".to_string(), "root hair".to_string())];
        assert_eq!(prompt_for("en", "叶片变黄。", &terms, ""), prompt("en"));
        let held = prompt_for("en", "干细胞微环境 [@1]。", &terms, "");
        assert!(held.starts_with(&prompt("en")) && held.ends_with("- 微环境 → niche") && !held.contains("根毛"));
        assert!(prompt_for("en", "叶片变黄。", &terms, "American spelling").ends_with("这份稿件的要求：American spelling"));
    }
}
