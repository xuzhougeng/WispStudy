//! 助手写: the writing page's other mode, where the assistant leads. The
//! person talks with it in the middle of the window and it writes the
//! manuscript at the right. It looks through the library with the tools
//! every conversation has and changes the manuscript with three of its own:
//! `edit_section` (`edit`) changes one section where its words stand,
//! `write_section` (`write`) writes the whole of one, `set_outline`
//! (`outline`) says which sections the manuscript has. What a section held
//! is kept as a snapshot before the assistant's first change to it, so a
//! version can always be gone back to. `note` is what each question of
//! such a conversation tells the model about the manuscript, `label_citations`
//! turns the citations of a conversation (`[P3 p.5 "…"]`) into the
//! manuscript's before a tool runs and `keep_quoted` keeps the sentences
//! they quoted as the passages those citations rest on, `copied` finds the
//! sentences written that repeat the paper they cite, and `hunks` is what
//! changed between two texts, for the window to show.

use crate::ai::Source;
use crate::cite::{self, Found, Index, Quoted};
use crate::manuscript::{self, Manuscript, Section};
use crate::passage;
use crate::revise::{self, Edit, Placed};
use crate::store::Store;
use crate::structure::{self, SECTIONS};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::ops::Range;

/// The two tools that write words into a section: what they write cites as the conversation does (`call_with`).
pub const WORDS: [&str; 2] = ["edit_section", "write_section"];

/// The most sections `set_outline` takes at once: a list longer than this is a mistake, not an outline.
const MOST_SECTIONS: usize = 60;

fn word<'a>(args: &'a Value, key: &str) -> &'a str {
    args[key].as_str().unwrap_or("").trim()
}

fn names(sections: &[Section]) -> String {
    sections.iter().map(|s| format!("「{}」", s.name)).collect::<Vec<_>>().join("、")
}

/// The manuscript `wanted` names by id or title; the only one when nothing is said.
pub fn find_manuscript(store: &Store, wanted: &str) -> Result<Manuscript> {
    let manuscripts = store.manuscripts()?;
    let listed = || manuscripts.iter().map(|m| format!("「{}」", m.shown_title())).collect::<Vec<_>>().join("、");
    match wanted {
        "" if manuscripts.len() == 1 => Ok(manuscripts[0].clone()),
        "" if manuscripts.is_empty() => bail!("写作页还没有稿件。"),
        "" => bail!("有 {} 份稿件，用 manuscript 指定一份：{}", manuscripts.len(), listed()),
        _ => manuscripts.iter().find(|m| m.id == wanted || m.shown_title() == wanted).cloned().ok_or_else(|| anyhow!("没有这份稿件：{wanted}。现有：{}", listed())),
    }
}

/// `[@<paper id>]` written as `[@Key]`: a caller that knows a paper only by its id cites it so.
pub fn keyed(text: &str, index: &Index) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        out.push_str(&rest[..at + 1]);
        rest = &rest[at + 1..];
        let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_')).unwrap_or(rest.len());
        match index.key(&rest[..end]) {
            Some(key) => out.push_str(key),
            None => out.push_str(&rest[..end]),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// The section the two tools that write one name: `manuscript` by id or title, `section` by id or name.
fn section_named(store: &Store, args: &Value) -> Result<(Manuscript, Section)> {
    let manuscript = find_manuscript(store, word(args, "manuscript"))?;
    let wanted = word(args, "section");
    if wanted.is_empty() { bail!("用 section 指定要写的一节（节的 ID 或名称）。现有：{}", names(&manuscript.sections)) }
    let Some(section) = manuscript.sections.iter().find(|s| s.id == wanted).or_else(|| manuscript.sections.iter().find(|s| s.name == wanted)).cloned() else {
        bail!("这份稿件没有「{wanted}」这一节。现有：{}。要新加一节，用 set_outline。", names(&manuscript.sections))
    };
    Ok((manuscript, section))
}

/// A section's text after a tool wrote it: when it `changed`, what the section held is kept as a snapshot
/// first, and a key written into a manuscript stays with its paper from then on. What the tool hands back.
fn keep(store: &mut Store, manuscript: &Manuscript, section: &Section, body: &str, changed: usize, index: &Index) -> Result<Value> {
    let cited = cite::find(body, index);
    if changed > 0 {
        store.take_snapshot(&section.id, "assistant")?;
        store.save_section_body(&section.id, body)?;
        for one in &cited {
            if let (Found::Paper(paper), false) = (&one.found, one.old) && let Some(key) = index.key(paper) { store.keep_cite_key(paper, key)? }
        }
    }
    let unknown: Vec<&str> = cited.iter().filter(|one| !one.old && one.found == Found::Unknown).map(|one| one.written.as_str()).collect();
    let mut out = json!({
        "manuscript": manuscript.id,
        "section": {"id": section.id, "name": section.name},
        "edited": changed,
        "characters": body.chars().count(),
    });
    if !unknown.is_empty() { out["unknownCitations"] = json!(unknown) }
    Ok(out)
}

/// The tool `edit_section`: changes to one section of a manuscript, each made where its words stand
/// (`revise::place`; no words to find is the end of the section). What no change names stays as it is.
/// The citations of the new words name papers by key or by id; the keys are kept with their papers.
/// The whole of a section is written by `write`, a tool of its own: while this one took either, a model
/// that meant a change began the whole text instead and put a few words where a section had been.
pub fn edit(store: &mut Store, args: &Value) -> Result<Value> {
    let (manuscript, section) = section_named(store, args)?;
    let Some(edits) = args["edits"].as_array().filter(|edits| !edits.is_empty()) else { bail!("用 edits 给出要改的地方，每项是 old 和 new。要写出一节的全文，用 write_section。") };
    let (papers, trash, kept) = (store.list_papers()?, store.list_trash()?, store.cite_keys()?);
    let index = Index::new(&papers, trash.iter().map(|entry| &entry.paper), &kept);
    let mut body = section.body.clone();
    let (mut changed, mut missed, mut near) = (0, vec![], None);
    for one in edits {
        let (old, new) = (one["old"].as_str().unwrap_or(""), keyed(one["new"].as_str().unwrap_or(""), &index));
        match revise::place(&body, &Edit { old: old.to_string(), new }, near) {
            Placed::At(range, new) => {
                near = Some(range.start + new.len());
                body.replace_range(range, &new);
                changed += 1;
            }
            Placed::Same => {}
            Placed::Missing => missed.push(old.to_string()),
        }
    }
    let mut out = keep(store, &manuscript, &section, &body, changed, &index)?;
    if !missed.is_empty() {
        out["missed"] = json!(missed);
        out["hint"] = json!("missed 里的原文没有在这一节里找到，这几处没有改。原文要从这一节现在的文字里逐字摘出：先用 read_manuscript 读这一节，再改。");
    }
    Ok(out)
}

/// The tool `write_section`: the whole text of one section, in place of what it holds: for a section with
/// nothing in it, or one the person wants written anew. A section that has words is never emptied this way,
/// and what a replacement did to its length is handed back (`replaced`), with a word to the model when
/// most of the section went (`Replacing::drops`).
pub fn write(store: &mut Store, args: &Value) -> Result<Value> {
    let (manuscript, section) = section_named(store, args)?;
    let Some(text) = args["text"].as_str() else { bail!("用 text 给出这一节完整的新正文。只改其中几处，用 edit_section。") };
    let (papers, trash, kept) = (store.list_papers()?, store.list_trash()?, store.cite_keys()?);
    let index = Index::new(&papers, trash.iter().map(|entry| &entry.paper), &kept);
    let text = keyed(text.trim(), &index);
    let held = section.body.trim();
    if text.is_empty() && !held.is_empty() { bail!("text 是空的：不会把写了内容的一节清空。要删去一段，用 edit_section 并把 new 留空。") }
    if text == held { return keep(store, &manuscript, &section, &section.body, 0, &index) }
    let mut out = keep(store, &manuscript, &section, &text, 1, &index)?;
    if let Some(replacing) = Replacing::of(held, &text) {
        let unit = replacing.unit();
        out["replaced"] = json!({"before": replacing.before, "after": replacing.after, "unit": unit});
        if replacing.drops {
            out["replacedHint"] = json!(format!("这一节原有 {} {unit}，现在整节是你给的 {} {unit}：原来的文字大半不在了。如果本意只是改其中几处，这是用错了工具：告诉用户原来的文字留作了历史版本，可以在写作页稿件上方的「版本」里恢复，恢复后用 edit_section 逐处修改。", replacing.before, replacing.after));
        }
    }
    Ok(out)
}

/// The share of a section's words a text written in its place has to have, for the section not to have lost most of them.
const STAYS: f32 = 0.5;

/// What writing a section anew does to one that has words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Replacing {
    /// How long it is and would be, as the outline counts it (`manuscript::text_length`).
    pub before: usize,
    pub after: usize,
    /// What the two count: letters (字) when either text has Chinese in it, else words (词).
    pub letters: bool,
    /// Most of what the section says would go: less than `STAYS` of its words are left. Counted as `words` counts
    /// them, a word of English as two characters of Chinese, so a section put into another language is not one cut short.
    pub drops: bool,
}

impl Replacing {
    /// `None` for a section with nothing in it: there a text is written, nothing is replaced.
    pub fn of(before: &str, after: &str) -> Option<Replacing> {
        if before.trim().is_empty() { return None }
        let amount = |text: &str| words(text).iter().map(|(_, counts)| counts).sum::<f32>();
        let ((was, letters), (is, more)) = (manuscript::text_length(before), manuscript::text_length(after));
        Some(Replacing { before: was, after: is, letters: letters || more, drops: amount(after) < amount(before) * STAYS })
    }

    /// What its numbers are called.
    pub fn unit(&self) -> &'static str {
        if self.letters { "字" } else { "词" }
    }
}

/// A section a call of `write_section` would write anew, when it has words: the section (its id), what it holds
/// and what it would hold, for the window to show the two in the section's place before the person says yes.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposed {
    pub section: String,
    pub before: String,
    pub after: String,
    pub replacing: Replacing,
}

/// What a call of `write_section` would do to the section it names, when that section has words. The
/// citations of the text are as the manuscript will have them (`label_citations`), not as the conversation
/// wrote them: a label carries the sentence it quotes, which the manuscript does not keep. None for a call
/// that gives no words: that one is refused by itself.
pub fn proposed(store: &Store, args: &Value, sources: &[Source]) -> Option<Proposed> {
    let (_, section) = section_named(store, args).ok()?;
    let (papers, trash, kept) = (store.list_papers().ok()?, store.list_trash().ok()?, store.cite_keys().ok()?);
    let index = Index::new(&papers, trash.iter().map(|entry| &entry.paper), &kept);
    let mut args = args.clone();
    label_citations(&mut args, sources, &index);
    let after = Some(keyed(args["text"].as_str()?.trim(), &index)).filter(|text| !text.is_empty())?;
    let replacing = Replacing::of(&section.body, &after)?;
    Some(Proposed { section: section.id, before: section.body.trim().to_string(), after, replacing })
}

/// `proposed`, for what it does to the section's length alone.
pub fn replacing(store: &Store, args: &Value, sources: &[Source]) -> Option<Replacing> {
    proposed(store, args, sources).map(|proposed| proposed.replacing)
}

/// One line of the outline `set_outline` asks for.
struct Line {
    /// The section it keeps: its id or its present name; empty for a new one.
    of: String,
    name: String,
    kind: String,
}

/// The tool `set_outline`: the sections a manuscript is to have, in order,
/// and its title and language when given. A section named by id or by the
/// name it has stays what it is, under the new name, kind and place; the
/// others are new. A section left out goes only when nothing was ever
/// written in it: one with text stays, after those asked for, and is said.
/// Without `sections` the outline stays as it is.
pub fn outline(store: &mut Store, args: &Value) -> Result<Value> {
    let manuscript = find_manuscript(store, word(args, "manuscript"))?;
    let title = args["title"].as_str().map(str::trim).filter(|title| !title.is_empty());
    let language = Some(word(args, "language")).filter(|code| !code.is_empty());
    if let Some(code) = language && !manuscript::is_language(code) { bail!("不认识这种语言：{code}。可以是：{}", manuscript::LANGUAGES.iter().map(|(code, _)| *code).collect::<Vec<_>>().join("、")) }
    let asked: &[Value] = match &args["sections"] {
        Value::Array(list) if !list.is_empty() => list,
        Value::Null if title.is_some() || language.is_some() => &[],
        _ => bail!("用 sections 给出稿件的各节，按先后排列；每项有 name，保留现有的一节时带上它的 id。只改题目或语言时可以不给 sections。"),
    };
    if asked.len() > MOST_SECTIONS { bail!("一份稿件最多 {MOST_SECTIONS} 节。") }
    let mut lines = vec![];
    for one in asked {
        let line = Line { of: word(one, "id").to_string(), name: word(one, "name").to_string(), kind: word(one, "kind").to_string() };
        if !line.kind.is_empty() && !manuscript::is_kind(&line.kind) { bail!("没有这种类型：{}。可以是：{}", line.kind, SECTIONS.iter().map(|(key, _)| *key).collect::<Vec<_>>().join("、")) }
        lines.push(line);
    }
    // Which section each line keeps: the one its id names, else the one that has its name; each once.
    let mut taken: Vec<&str> = vec![];
    let mut wanted = vec![];
    for line in &lines {
        let free = |s: &&Section| !taken.contains(&s.id.as_str());
        let found = if line.of.is_empty() { manuscript.sections.iter().filter(free).find(|s| s.name == line.name) } else { manuscript.sections.iter().filter(free).find(|s| s.id == line.of).or_else(|| manuscript.sections.iter().filter(free).find(|s| s.name == line.of)) };
        if found.is_none() && !line.of.is_empty() { bail!("这份稿件没有「{}」这一节。现有：{}。新加的一节不要带 id。", line.of, names(&manuscript.sections)) }
        let name = if line.name.is_empty() { found.map(|s| s.name.clone()).unwrap_or_default() } else { line.name.clone() };
        if name.is_empty() { bail!("每一节都需要 name。") }
        // A new section's kind is what its name says, when it says: that is where its candidate sentences come from.
        let kind = match (line.kind.as_str(), found) {
            ("", Some(section)) => section.kind.clone(),
            ("", None) => structure::canonical(&name).filter(|kind| manuscript::is_kind(kind)).unwrap_or("other").to_string(),
            (kind, _) => kind.to_string(),
        };
        if let Some(section) = found { taken.push(&section.id) }
        wanted.push(manuscript::Arrange { id: found.map(|s| s.id.clone()).unwrap_or_default(), name, kind });
    }
    let arranged = if wanted.is_empty() { manuscript::Arranged::default() } else { store.arrange_sections(&manuscript.id, &wanted)? };
    if let Some(title) = title { store.rename_manuscript(&manuscript.id, title)? }
    if let Some(code) = language { store.set_manuscript_language(&manuscript.id, code)? }
    let now = store.manuscript(&manuscript.id)?.unwrap_or_default();
    let mut out = json!({
        "manuscript": now.id,
        "title": now.shown_title(),
        "language": now.language,
        "sections": now.sections.iter().map(|s| json!({"id": s.id, "name": s.name, "kind": s.kind, "characters": s.body.chars().count()})).collect::<Vec<_>>(),
        "added": arranged.added,
        "removed": arranged.removed,
    });
    if !arranged.kept.is_empty() {
        out["kept"] = json!(arranged.kept);
        out["hint"] = json!("kept 里的节没有列在大纲里，但写过内容，所以没有删除，排在了最后。要去掉它们，请用户在大纲里自己删除。");
    }
    Ok(out)
}

/// What a call of one of the three tools would do, in a line or two: for the card that asks the person first,
/// where the conversation is not the manuscript's own, or a section of it would lose most of its words.
pub fn preview(store: &Store, name: &str, args: &Value) -> Result<Vec<String>> {
    preview_for(store, name, args, &[])
}

/// `preview` for a conversation, whose citations are labels (`sources`): a text is counted as the manuscript will have it.
pub fn preview_for(store: &Store, name: &str, args: &Value, sources: &[Source]) -> Result<Vec<String>> {
    let manuscript = find_manuscript(store, word(args, "manuscript"))?;
    let title = manuscript.shown_title();
    let section = word(args, "section");
    let section = manuscript.sections.iter().find(|s| s.id == section || s.name == section).map_or(section, |s| s.name.as_str());
    Ok(match name {
        "edit_section" => args["edits"].as_array().map(|edits| format!("《{title}》的「{section}」：改 {} 处", edits.len())).into_iter().collect(),
        "write_section" => match (replacing(store, args, sources), args["text"].as_str()) {
            (Some(replacing), _) => vec![format!("《{title}》的「{section}」：整节 {} {unit}换成新写的 {} {unit}", replacing.before, replacing.after, unit = replacing.unit())],
            (None, Some(text)) => vec![format!("《{title}》的「{section}」：整节写成新的 {}", manuscript::length_said(text))],
            _ => vec![],
        },
        _ => {
            let sections: Vec<&str> = args["sections"].as_array().into_iter().flatten().map(|one| word(one, "name")).filter(|name| !name.is_empty()).collect();
            let mut lines = vec![];
            if !sections.is_empty() { lines.push(format!("《{title}》的大纲改为：{}", sections.join("、"))) }
            if !word(args, "title").is_empty() { lines.push(format!("《{title}》的题目改为：{}", word(args, "title"))) }
            if !word(args, "language").is_empty() { lines.push(format!("《{title}》的正文语言改为：{}", crate::ai::language_name(word(args, "language")))) }
            lines
        }
    })
}

/// What the model is told about the manuscript with every question of the
/// conversation that writes it: the record, not what was said earlier, is
/// where the manuscript stands.
pub struct Asked<'a> {
    pub manuscript: &'a Manuscript,
    /// The section on show beside the conversation: its id.
    pub showing: &'a str,
    /// What is selected in it.
    pub selection: &'a str,
}

pub fn note(asked: &Asked) -> String {
    let manuscript = asked.manuscript;
    let language = crate::ai::language_name(&manuscript.language);
    let mut out = format!(
        "【写作模式】你在主导这份稿件的写作：用户在窗口右边看着稿件，在这里和你对话，由你动笔。\n稿件：《{}》（ID {}），正文用{language}写。\n大纲（节 ID · 名称 · 类型 · 现有字数）：\n",
        manuscript.shown_title(),
        manuscript.id
    );
    for (n, section) in manuscript.sections.iter().enumerate() {
        let kind = SECTIONS.iter().find(|(key, _)| *key == section.kind).map_or(section.kind.as_str(), |(_, name)| name);
        let length = section.body.chars().filter(|c| !c.is_whitespace()).count();
        out.push_str(&format!("{}. {} · {} · {kind} · {}{}\n", n + 1, section.id, section.name, if length == 0 { "未写".to_string() } else { format!("{length} 字") }, if section.id == asked.showing { " ← 右边正显示这一节" } else { "" }));
    }
    let selection = asked.selection.trim();
    if !selection.is_empty() {
        let name = manuscript.section(asked.showing).map_or("", |s| s.name.as_str());
        out.push_str(&format!("用户在「{name}」里选中了下面这段文字，这次的要求多半是针对它的：\n{selection}\n"));
    }
    // The sentences to write after are asked for in the manuscript's language, where `find_sentences` knows it.
    let written_in = if structure::LANGUAGES.iter().any(|(code, _)| *code == manuscript.language) { format!("language 用 \"{}\"（这种语言一句也没有时去掉 language 再看：别的语言的句子只用来看一段怎么组织，措辞不能照搬）；", manuscript.language) } else { String::new() };
    out.push_str(&format!(
        "做法：\n\
- 稿件只通过工具改动：edit_section 逐处修改一节，write_section 写出一节的全文（只用于空的一节，或用户明确要整节重写的一节），set_outline 调整各节、题目和正文语言（用户要的语言和上面写的不一样时，先用它改过来）。节里已有文字时用 edit_section：用户写的句子，要求之外的一字不动。要写进稿件的正文不要贴在对话里；对话里用几句话说明做了什么、依据哪些文献、参照了哪几篇的写法、还缺什么，让用户去右边看稿。\n\
- 动笔前先读：要改的一节用 read_manuscript 读它现在的文字，用户可能刚亲手改过，以稿件现在的文字为准；edit_section 的 old 要从中逐字摘出。\n\
- 论断要有出处：先用 find_sentences、find_citations、search_fulltext 等在文献库里找到文献和原句，再写。写进稿件的引用和回答里一样，尽量带上它所依据的原句：[P3 p.5 \"逐字摘出的原句\"]，出自摘要的原句不写页码：[P3 \"逐字摘出的原句\"]，实在没有可摘的原句才写 [P3]。写入时会换成稿件的引用格式，引的那句原文会记为这处引用的出处，用户点一下就能核对。文献库里找不到依据的论断不要配引用，更不要编造文献；在对话里指出来。\n\
- {}\n\
- 正文用{language}写，是 Markdown；节的名称不要作为标题写进正文。\n\
- 只做用户要求的范围。删去或合并写了内容的节、大段删改之前，先在对话里说明并征得同意；整节重写会让一节少掉一大半文字时，窗口还会再问用户一次。",
        by_example(&written_in)
    ));
    out
}

/// How the assistant is told to write: after the sentences the library's
/// papers have in the same part, as 仿写 does for a paragraph, and in its
/// own words, which `copied` checks of every sentence that cites a paper.
/// `written_in` is the clause about the language to ask `find_sentences`
/// for, when there is one.
fn by_example(written_in: &str) -> String {
    format!(
        "写法要有参照：写一节或成段的新文字之前，先用 find_sentences 看文献库里的论文在同一部分是怎么写的。section 用这一节的类型（大纲里写着；类型是「其他」的一节，按它实际的内容选最接近的部分）；{written_in}先不带 query 看各篇怎么开头，再用 queries 给出要写的内容的英文关键词找相近的句子，context 设为 true 可以看到前后句。照这些句子的写法来写：一段里先说什么后说什么、句子之间怎么衔接、这个领域惯用的说法、句子的长短和详略。一段围绕一个判断，把回答同一问题的几篇文献放在一起说，不要一篇一句地罗列；换一个题目也成立的句子不写。这一部分在文献库里没有句子可参照时（sentencesInSection 为 0）照常写，并在对话里说一声。\n\
- 用自己的话写：候选句和引用标签里的原句是参照和出处，不是正文。写进稿件的每句话都要自己写，和任何一句原文连续相同的词不超过五六个；候选句里的论断和数据，只有引用它出自的文献时才用。确实要用原话时，加引号写成直接引语。写稿件的结果里有 copied 时，那几句和所引文献的原文大段相同：用 edit_section 改成自己的话；用户要的就是原话时，问他是加引号还是改写。"
    )
}

/// A piece of text the assistant is about to write into a manuscript: the
/// text as the manuscript takes it, and the sentences its citations quoted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Piece {
    pub text: String,
    pub quoted: Vec<Quoted>,
}

/// Before `edit_section` or `write_section` runs for a conversation: the citations of the
/// words to be written (`text`, and every `new` of `edits`) are turned from
/// the conversation's (`[P3 p.5 "…"]`) into the manuscript's (`[@Key, p. 5]`)
/// in `args`. The pieces written, each with what it quoted, for `keep_quoted`.
pub fn label_citations(args: &mut Value, sources: &[Source], index: &Index) -> Vec<Piece> {
    let mut pieces = vec![];
    let mut convert = |value: &mut Value| {
        let Some(text) = value.as_str() else { return };
        let converted = cite::convert_ai_citations(text, sources, index);
        pieces.push(Piece { text: converted.text.clone(), quoted: converted.quoted });
        *value = json!(converted.text);
    };
    // Only what the call has: asking a JSON object for a key it lacks would put the key there.
    if let Some(text) = args.get_mut("text") { convert(text) }
    for one in args.get_mut("edits").and_then(Value::as_array_mut).into_iter().flatten() {
        if let Some(new) = one.get_mut("new") { convert(new) }
    }
    pieces
}

/// A tool called for a conversation: as `tools::call_in`, and for
/// `edit_section` and `write_section` the citations of the words to be written are the
/// conversation's (`label_citations`), and the sentences they quote are
/// kept as the passages those citations rest on (`keep_quoted`). The
/// result says how many were kept and which quotations their papers do not hold.
pub fn call(store: &mut Store, info: &crate::libraries::LibraryInfo, name: &str, args: &Value, sources: &[Source], fetch: crate::sources::Fetch, download: crate::sources::Download) -> Result<Value, String> {
    call_with(store, info, name, args, sources, fetch, download, crate::tools::Reach::default())
}

pub fn call_with(store: &mut Store, info: &crate::libraries::LibraryInfo, name: &str, args: &Value, sources: &[Source], fetch: crate::sources::Fetch, download: crate::sources::Download, reach: crate::tools::Reach) -> Result<Value, String> {
    if !WORDS.contains(&name) { return crate::tools::call_in_with(store, info, name, args, fetch, download, reach) }
    // As the schema reads them first: a list a model wrote as a string is a list before its citations are looked at.
    let mut args = crate::tools::fitted(name, args)?;
    let pieces = {
        let read = |store: &Store| -> Result<_> { Ok((store.list_papers()?, store.list_trash()?, store.cite_keys()?)) };
        let (papers, trash, kept) = read(store).map_err(|e| format!("{e:#}"))?;
        label_citations(&mut args, sources, &Index::new(&papers, trash.iter().map(|entry| &entry.paper), &kept))
    };
    let mut value = crate::tools::call_in(store, info, name, &args, fetch, download)?;
    // A label that names no paper of this conversation went into the manuscript as it was written: the model is told, to put it right.
    let mut stray: Vec<String> = pieces.iter().flat_map(|piece| crate::ai::citation_spans(&piece.text)).flat_map(|(_, cited)| cited.into_iter().map(|one| one.label)).collect();
    stray.dedup();
    if !stray.is_empty() {
        value["unknownLabels"] = json!(stray);
        value["labelsHint"] = json!("unknownLabels 里的标签在这次对话里不对应任何文献，已原样留在稿件里。用 edit_section 把它们改成正确的文献标签，或去掉这几处引用。");
    }
    let (Some(manuscript), Some(section)) = (value["manuscript"].as_str().map(str::to_string), value["section"]["id"].as_str().map(str::to_string)) else { return Ok(value) };
    let body = store.manuscript(&manuscript).ok().flatten().and_then(|m| m.section(&section).map(|s| s.body.clone())).unwrap_or_default();
    // The section is written by now: a passage that could not be kept takes nothing back.
    // A sentence that repeats the paper it cites is the paper's, not the manuscript's: the model is told, to say it in its own words.
    if let Ok(copied) = copied(store, &body, &pieces) && !copied.is_empty() {
        value["copied"] = json!(copied.iter().map(|(sentence, shared)| json!({"sentence": sentence, "shared": shared})).collect::<Vec<_>>());
        value["copiedHint"] = json!("copied 里的句子（sentence）有一长串词（shared）和它所引文献的原文一字不差，这是照抄。用 edit_section 把这几句改成自己的话：意思和引用不变，说法换掉；确实要用原话时，加引号写成直接引语。相同的只是一串专有名词（物种、方法的名称）时不用改。");
    }
    if let Ok((kept, unfound)) = keep_quoted(store, &manuscript, &section, &body, &pieces) {
        if kept > 0 { value["passagesKept"] = json!(kept) }
        if !unfound.is_empty() {
            value["quotesNotFound"] = json!(unfound);
            value["quotesHint"] = json!("quotesNotFound 里的原句没有在所引文献的摘要或全文里找到，没有记为出处。核对这几处引用：换成文献里确有的句子，或去掉引用，并在对话里告诉用户。");
        }
    }
    Ok(value)
}

/// After `edit_section` wrote `pieces` into a section that now reads
/// `body`: the sentences they quoted for their citations are kept as what
/// those citations rest on, where the paper really says them, on the page
/// it does. How many were kept, and the quotations their papers do not hold.
pub fn keep_quoted(store: &mut Store, manuscript_id: &str, section_id: &str, body: &str, pieces: &[Piece]) -> Result<(usize, Vec<String>)> {
    // Where each piece stands in the section, as it was written or without the space around it.
    let mut quoted: Vec<Quoted> = vec![];
    for piece in pieces.iter().filter(|piece| !piece.quoted.is_empty()) {
        let lead = piece.text.len() - piece.text.trim_start().len();
        let Some(at) = body.find(piece.text.trim()) else { continue };
        quoted.extend(piece.quoted.iter().filter(|one| one.at >= lead).map(|one| Quoted { at: at + one.at - lead, ..one.clone() }));
    }
    if quoted.is_empty() { return Ok((0, vec![])) }
    let (papers, trash, kept) = (store.list_papers()?, store.list_trash()?, store.cite_keys()?);
    let index = Index::new(&papers, trash.iter().map(|entry| &entry.paper), &kept);
    let (mut new, mut unfound) = (vec![], vec![]);
    for (paper_id, claim, mut candidate) in passage::quoted(body, &cite::find(body, &index), &quoted) {
        let Some(paper) = papers.iter().find(|paper| paper.id == paper_id) else { continue };
        let pages = store.paper_pages(&paper_id)?;
        match crate::ai::locate_quote(&candidate.text, paper, pages.as_deref()) {
            Some(Some(page)) => candidate.page = page,
            Some(None) => (candidate.part, candidate.page) = ("abstract".into(), 0),
            // The paper does not say it: nothing to rest on.
            None => {
                unfound.push(candidate.text);
                continue;
            }
        }
        new.push((section_id.to_string(), paper_id, claim, candidate, String::new(), passage::CITED.to_string()));
    }
    if !new.is_empty() { store.keep_passages(manuscript_id, &new)?; }
    Ok((new.len(), unfound))
}

/// After `edit_section` wrote `pieces` into a section that now reads
/// `body`: the sentences among them that repeat a paper they cite, each
/// with the words it shares with it (`shared`). A sentence is compared with
/// all the library holds of the paper, its abstract and the pages of its
/// full text, whether or not it quoted a sentence for the citation.
pub fn copied(store: &Store, body: &str, pieces: &[Piece]) -> Result<Vec<(String, String)>> {
    // Where each piece stands in the section, without the space around it.
    let written: Vec<Range<usize>> = pieces.iter().map(|piece| piece.text.trim()).filter(|text| !text.is_empty()).filter_map(|text| body.find(text).map(|at| at..at + text.len())).collect();
    if written.is_empty() { return Ok(vec![]) }
    let (papers, trash, kept) = (store.list_papers()?, store.list_trash()?, store.cite_keys()?);
    let index = Index::new(&papers, trash.iter().map(|entry| &entry.paper), &kept);
    // What each paper cited says, read once.
    let mut said: std::collections::BTreeMap<String, Vec<(String, f32)>> = Default::default();
    let mut out: Vec<(String, String)> = vec![];
    for citation in passage::citations(body, &cite::find(body, &index)) {
        if !written.iter().any(|range| range.contains(&citation.at.start)) || out.iter().any(|(sentence, _)| *sentence == citation.claim) { continue }
        if !said.contains_key(&citation.paper) {
            let mut text = papers.iter().find(|paper| paper.id == citation.paper).map(|paper| paper.abstract_text.clone()).unwrap_or_default();
            for page in store.paper_pages(&citation.paper)?.unwrap_or_default() {
                text.push('\n');
                text.push_str(&page);
            }
            said.insert(citation.paper.clone(), words(&text));
        }
        if let Some(shared) = shared(&body[citation.sentence.clone()], &said[&citation.paper]) { out.push((citation.claim, shared)) }
    }
    Ok(out)
}

/// The longest run of words a sentence may share with a paper it cites before it is the paper's sentence
/// written again: a term with the words around it runs to five or six.
const REPEATED: f32 = 8.;

/// Whether a sentence of the manuscript repeats `source`, a sentence or a whole paper: a run of `REPEATED`
/// words stands in both, one after another. What the sentence puts between quotation marks is said to be
/// another's, and is not counted.
pub fn repeats(sentence: &str, source: &str) -> bool {
    shared(sentence, &words(source)).is_some()
}

/// The words a sentence shares with a text of `theirs` in one run, when the run is long enough to be copied.
fn shared(sentence: &str, theirs: &[(String, f32)]) -> Option<String> {
    own_stretches(sentence).iter().find_map(|stretch| {
        let ours = words(stretch);
        let (counts, end) = shared_run(&ours, theirs);
        if counts < REPEATED { return None }
        // Back from where the run ends in ours, for as long as it counts.
        let (mut start, mut left) = (end, counts);
        while left > 0. && start > 0 {
            start -= 1;
            left -= ours[start].1;
        }
        Some(ours[start..end].iter().map(|(word, _)| word.as_str()).collect::<Vec<_>>().join(" "))
    })
}

/// The stretches of a sentence that are in its own voice: without its citations and what it quotes.
fn own_stretches(sentence: &str) -> Vec<String> {
    let (mut out, mut now, mut closing) = (vec![], String::new(), None);
    for c in sentence.chars() {
        match (closing, c) {
            (Some(end), c) if c == end => closing = None,
            (Some(_), _) => {}
            (None, '“' | '"' | '「' | '『' | '«' | '[') => {
                closing = Some(match c { '“' => '”', '「' => '」', '『' => '』', '«' => '»', '[' => ']', _ => '"' });
                out.push(std::mem::take(&mut now));
            }
            (None, c) => now.push(c),
        }
    }
    out.push(now);
    out
}

/// The words of a text to compare by, each with what it counts: runs of letters and digits in lowercase, a
/// hyphen or an apostrophe inside one kept (`single-nucleus` is one word); a character of Chinese, Japanese
/// or Korean is a word of its own and counts half, two of them saying about what one word does.
fn words(text: &str) -> Vec<(String, f32)> {
    let wide = |c: char| matches!(c, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{ac00}'..='\u{d7af}');
    let (mut out, mut now) = (vec![], String::new());
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().copied().enumerate() {
        let joins = matches!(c, '-' | '\'' | '’') && !now.is_empty() && chars.get(i + 1).is_some_and(|next| next.is_alphanumeric() && !wide(*next));
        if c.is_alphanumeric() && !wide(c) || joins {
            now.extend(c.to_lowercase());
            continue;
        }
        if !now.is_empty() { out.push((std::mem::take(&mut now), 1.)) }
        if wide(c) { out.push((c.to_string(), 0.5)) }
    }
    if !now.is_empty() { out.push((now, 1.)) }
    out
}

/// The longest run of words two texts share one after another: what its words count, and where it ends in `ours`.
fn shared_run(ours: &[(String, f32)], theirs: &[(String, f32)]) -> (f32, usize) {
    let (mut best, mut before, mut row) = ((0f32, 0), vec![0f32; theirs.len() + 1], vec![0f32; theirs.len() + 1]);
    for (n, (word, counts)) in ours.iter().enumerate() {
        for (at, (other, _)) in theirs.iter().enumerate() {
            row[at + 1] = if word == other { before[at] + counts } else { 0. };
            if row[at + 1] > best.0 { best = (row[at + 1], n + 1) }
        }
        std::mem::swap(&mut before, &mut row);
    }
    best
}

/// A stretch that differs between two texts: these bytes of the one before, those of the one after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub before: Range<usize>,
    pub after: Range<usize>,
}

/// What changed from `before` to `after`, line by line: each run of lines
/// that differ is one hunk, in the order of the text, and two runs with
/// nothing but empty lines between them are one. A line here is a
/// paragraph of the manuscript or the empty line between two. Putting each
/// hunk's words of `after` where its bytes of `before` are, from the last
/// to the first, makes `after` of `before`.
pub fn hunks(before: &str, after: &str) -> Vec<Hunk> {
    if before == after { return vec![] }
    // Every line with its line break, the last one given one: lines are alike only when all of them is.
    let (old, new) = (format!("{before}\n"), format!("{after}\n"));
    let lines = |text: &str| -> Vec<Range<usize>> {
        let mut at = 0;
        text.split_inclusive('\n').map(|line| { at += line.len(); at - line.len()..at }).collect()
    };
    let (a, b) = (lines(&old), lines(&new));
    let same = |i: usize, j: usize| old[a[i].clone()] == new[b[j].clone()];
    // How many lines the two have in common from each pair of places on, in order.
    let mut table = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i][j] = if same(i, j) { table[i + 1][j + 1] + 1 } else { table[i + 1][j].max(table[i][j + 1]) };
        }
    }
    let start = |ranges: &[Range<usize>], at: usize, text: &str| ranges.get(at).map_or(text.len(), |range| range.start);
    let (mut i, mut j) = (0, 0);
    let mut out: Vec<Hunk> = vec![];
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && same(i, j) {
            (i, j) = (i + 1, j + 1);
            continue;
        }
        let (from_a, from_b) = (start(&a, i, &old), start(&b, j, &new));
        while (i < a.len() || j < b.len()) && !(i < a.len() && j < b.len() && same(i, j)) {
            if j >= b.len() || (i < a.len() && table[i + 1][j] > table[i][j + 1]) { i += 1 } else { j += 1 }
        }
        let hunk = Hunk { before: from_a..start(&a, i, &old), after: from_b..start(&b, j, &new) };
        match out.last_mut() {
            Some(last) if old[last.before.end..hunk.before.start].trim().is_empty() => (last.before.end, last.after.end) = (hunk.before.end, hunk.after.end),
            _ => out.push(hunk),
        }
    }
    // The line break given to the last line is not the text's: a hunk that reaches it takes the one before its lines instead.
    for hunk in &mut out {
        if hunk.before.end <= before.len() && hunk.after.end <= after.len() { continue }
        if hunk.before.start > 0 && hunk.after.start > 0 { (hunk.before.start, hunk.after.start) = (hunk.before.start - 1, hunk.after.start - 1) }
        (hunk.before.end, hunk.after.end) = (hunk.before.end.min(before.len()), hunk.after.end.min(after.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Paper;
    use std::path::Path;

    fn store() -> Store {
        Store::in_memory(Path::new("/tmp/wisp-compose-test")).unwrap()
    }

    fn library() -> (Store, Manuscript, Paper) {
        let mut store = store();
        let paper = store.add_paper(Paper { title: "Root stem cell niche".into(), authors: "Zhang, Wei; Li, Na".into(), year: Some(2021), abstract_text: "WOX5 maintains the quiescent centre of the root.".into(), ..Default::default() }).unwrap();
        let made = store.create_manuscript("根的综述", "zh").unwrap();
        (store, made, paper)
    }

    #[test]
    fn a_section_is_written_changed_and_its_versions_kept() {
        let (mut store, made, paper) = library();
        let section = made.sections[1].clone();
        // The whole text of an empty section, citing a paper by its id.
        let done = write(&mut store, &json!({"section": section.name, "text": format!("根尖有干细胞微环境 [@{}]。\n\n第二段。", paper.id)})).unwrap();
        assert_eq!((done["edited"].as_u64(), done["section"]["id"].as_str(), done["replaced"].is_null()), (Some(1), Some(section.id.as_str()), true));
        let body = store.manuscript(&made.id).unwrap().unwrap().section(&section.id).unwrap().body.clone();
        assert_eq!(body, "根尖有干细胞微环境 [@Zhang2021]。\n\n第二段。");
        // The key stays with its paper; nothing was there to keep as a version.
        assert_eq!(store.cite_keys().unwrap().get("Zhang2021"), Some(&paper.id));
        assert!(store.snapshots(&section.id).unwrap().is_empty());
        // Changes where the words stand; one whose words are not there is said, the others are made.
        let done = edit(&mut store, &json!({"manuscript": made.id, "section": section.id, "edits": [{"old": "第二段。", "new": "第二段写长了。"}, {"old": "没有这句", "new": "x"}, {"old": "", "new": "结尾一段 [@Nobody1999]。"}]})).unwrap();
        assert_eq!((done["edited"].as_u64(), done["missed"][0].as_str(), done["unknownCitations"][0].as_str()), (Some(2), Some("没有这句"), Some("@Nobody1999")));
        let now = store.manuscript(&made.id).unwrap().unwrap().section(&section.id).unwrap().body.clone();
        assert_eq!(now, "根尖有干细胞微环境 [@Zhang2021]。\n\n第二段写长了。\n\n结尾一段 [@Nobody1999]。");
        // What the section said before is a version to go back to.
        let versions = store.snapshots(&section.id).unwrap();
        assert_eq!((versions.len(), versions[0].body.as_str(), versions[0].why.as_str()), (1, body.as_str(), "assistant"));
        // Nothing to do changes nothing and keeps no version.
        let done = edit(&mut store, &json!({"section": section.id, "edits": [{"old": "第二段写长了。", "new": "第二段写长了。"}]})).unwrap();
        assert_eq!((done["edited"].as_u64(), store.snapshots(&section.id).unwrap().len()), (Some(0), 1));
        // What is refused, and why.
        assert!(write(&mut store, &json!({"section": "没有", "text": "x"})).unwrap_err().to_string().contains("没有「没有」这一节"));
        assert!(edit(&mut store, &json!({"section": section.id})).unwrap_err().to_string().contains("write_section"));
        assert!(write(&mut store, &json!({"section": section.id})).unwrap_err().to_string().contains("edit_section"));
        assert!(write(&mut store, &json!({"section": section.id, "text": "  "})).unwrap_err().to_string().contains("清空"));
        assert!(write(&mut store, &json!({"text": "x"})).unwrap_err().to_string().contains("section"));
    }

    #[test]
    fn a_section_written_anew_says_what_became_of_it() {
        let (mut store, made, paper) = library();
        let (section, empty) = (made.sections[1].clone(), made.sections[0].clone());
        let held = "根尖的静止中心由 WOX5 维持 [@Zhang2021]。\n\n干细胞微环境的信号来自静止中心，这些信号让周围的干细胞保持不分化的状态。\n\n静止中心受损以后，邻近的干细胞会分裂来补上它。";
        let length = |text: &str| manuscript::text_length(text).0 as u64;
        store.save_section_body(&section.id, held).unwrap();
        let body = |store: &Store| store.manuscript(&made.id).unwrap().unwrap().section(&section.id).unwrap().body.clone();
        // A change is no place for a section's whole text: a few words meant for a change leave the section as it is.
        assert!(edit(&mut store, &json!({"section": section.id, "text": "错误：应使用 edits 参数"})).is_err());
        assert_eq!(body(&store), held);
        // What a call would do, for the card that asks: a section with nothing in it is written, one with words is replaced.
        assert_eq!(preview(&store, "write_section", &json!({"section": empty.id, "text": "一二三"})).unwrap(), vec![format!("《根的综述》的「{}」：整节写成新的 3 字", empty.name)]);
        assert_eq!(preview(&store, "write_section", &json!({"section": section.id, "text": "一二三"})).unwrap(), vec![format!("《根的综述》的「{}」：整节 {} 字换成新写的 3 字", section.name, length(held))]);
        // A conversation's label carries the sentence it quotes; the manuscript keeps the citation alone, and that is what is counted.
        let sources = vec![Source { label: "P1".into(), paper_id: paper.id.clone(), title: paper.title.clone(), ..Default::default() }];
        let quoting = json!({"section": section.id, "text": "静止中心 [P1 \"WOX5 maintains the quiescent centre of the root.\"]。"});
        assert_eq!(replacing(&store, &quoting, &sources), Some(Replacing { before: length(held) as usize, after: 5, letters: true, drops: true }));
        assert_eq!(replacing(&store, &json!({"section": empty.id, "text": "一二三"}), &sources), None);
        // The two texts, for the window to show in the section's place: what it holds, and what it would as the manuscript will have it.
        let shown = proposed(&store, &quoting, &sources).unwrap();
        assert_eq!((shown.section.as_str(), shown.before.as_str(), shown.after.as_str()), (section.id.as_str(), held, "静止中心 [@Zhang2021]。"));
        // A call that gives no words is refused by itself: there is nothing to show or ask.
        assert_eq!(proposed(&store, &json!({"section": section.id, "text": " "}), &sources), None);
        // Written anew at about its length: what became of it is said, and there is nothing to put right.
        let anew = "WOX5 维持着根尖的静止中心 [@Zhang2021]。\n\n静止中心发出的信号构成了干细胞微环境，周围的干细胞因此不分化。\n\n它受损时，邻近的干细胞分裂并取代它。";
        let done = write(&mut store, &json!({"section": section.id, "text": anew})).unwrap();
        assert_eq!((done["replaced"]["before"].as_u64(), done["replaced"]["after"].as_u64(), done["replacedHint"].is_null()), (Some(length(held)), Some(length(anew)), true));
        // The same text again is no change, and no version.
        let done = write(&mut store, &json!({"section": section.id, "text": format!("  {anew}\n")})).unwrap();
        assert_eq!((done["edited"].as_u64(), done["replaced"].is_null(), store.snapshots(&section.id).unwrap().len()), (Some(0), true, 1));
        // A few words where a section stood are written as asked, the model is told what that did, and the section is a version to go back to.
        let done = write(&mut store, &json!({"section": section.id, "text": "错误：应使用 edits 参数"})).unwrap();
        assert!(done["replacedHint"].as_str().unwrap().contains("历史版本") && done["replaced"] == json!({"before": length(anew), "after": 8, "unit": "字"}));
        assert!(store.snapshots(&section.id).unwrap().iter().any(|version| version.body == anew));
        // Most of a section going is fewer than half of its words left; a section put into another language has lost none.
        assert_eq!(Replacing::of(" \n", "x"), None);
        assert!(Replacing::of(held, "根尖的静止中心由 WOX5 维持 [@Zhang2021]。").unwrap().drops);
        assert!(!Replacing::of(held, "根尖的静止中心由 WOX5 维持 [@Zhang2021]。\n\n干细胞微环境的信号来自静止中心，这些信号让周围的干细胞保持不分化的状态。").unwrap().drops);
        let (chinese, english) = ("根尖的静止中心由 WOX5 维持，它发出的信号让周围的干细胞不分化。", "WOX5 maintains the quiescent centre of the root tip, and its signals keep the surrounding stem cells undifferentiated.");
        assert!(!Replacing::of(chinese, english).unwrap().drops && !Replacing::of(english, chinese).unwrap().drops);
        // Words are what an English text is counted in, and letters as soon as one of the two has Chinese.
        assert_eq!((Replacing::of(english, "Too short.").map(|r| (r.before, r.after, r.unit(), r.drops)), Replacing::of(english, chinese).map(|r| r.unit())), (Some((18, 2, "词", true)), Some("字")));
    }

    #[test]
    fn an_outline_keeps_what_was_written() {
        let (mut store, made, _) = library();
        let (abstract_id, intro) = (made.sections[0].id.clone(), made.sections[1].clone());
        store.save_section_body(&intro.id, "已经写了的导言。").unwrap();
        // The introduction under a new name, the abstract by its name, two new sections; the three unwritten ones are left out.
        let done = outline(&mut store, &json!({"title": "根尖干细胞", "sections": [{"name": made.sections[0].name}, {"id": intro.id, "name": "研究背景"}, {"name": "干细胞微环境"}, {"name": "Discussion"}]})).unwrap();
        let now = store.manuscript(&made.id).unwrap().unwrap();
        assert_eq!(now.title, "根尖干细胞");
        assert_eq!(now.sections.iter().map(|s| (s.name.as_str(), s.kind.as_str())).collect::<Vec<_>>(), vec![(made.sections[0].name.as_str(), "abstract"), ("研究背景", "introduction"), ("干细胞微环境", "other"), ("Discussion", "discussion")]);
        assert_eq!((now.sections[0].id.as_str(), now.sections[1].id.as_str(), now.sections[1].body.as_str()), (abstract_id.as_str(), intro.id.as_str(), "已经写了的导言。"));
        assert_eq!((done["added"].as_array().map(Vec::len), done["removed"].as_array().map(Vec::len), done["kept"].is_null()), (Some(2), Some(3), true));
        // A section with text that the outline leaves out stays, at the end, and is said.
        let done = outline(&mut store, &json!({"sections": [{"name": "干细胞微环境"}]})).unwrap();
        let now = store.manuscript(&made.id).unwrap().unwrap();
        assert_eq!(now.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["干细胞微环境", "研究背景"]);
        assert_eq!(done["kept"][0], "研究背景");
        assert!(outline(&mut store, &json!({"sections": []})).is_err() && outline(&mut store, &json!({})).is_err());
        // The language and the title alone: the outline stays as it is.
        let done = outline(&mut store, &json!({"language": "en", "title": "Root stem cells"})).unwrap();
        assert_eq!((done["language"].as_str(), done["title"].as_str(), done["sections"].as_array().map(Vec::len)), (Some("en"), Some("Root stem cells"), Some(2)));
        assert!(outline(&mut store, &json!({"language": "tlh"})).unwrap_err().to_string().contains("不认识这种语言"));
        outline(&mut store, &json!({"title": "根尖干细胞", "language": "zh"})).unwrap();
        assert!(outline(&mut store, &json!({"sections": [{"id": "S-nowhere", "name": "x"}]})).unwrap_err().to_string().contains("没有「S-nowhere」"));
        assert!(outline(&mut store, &json!({"sections": [{"name": "x", "kind": "poem"}]})).unwrap_err().to_string().contains("没有这种类型"));
        assert_eq!(preview(&store, "set_outline", &json!({"sections": [{"name": "甲"}, {"name": "乙"}]})).unwrap(), vec!["《根尖干细胞》的大纲改为：甲、乙"]);
        assert_eq!(preview(&store, "edit_section", &json!({"section": "研究背景", "edits": [{}, {}]})).unwrap(), vec!["《根尖干细胞》的「研究背景」：改 2 处"]);
    }

    #[test]
    fn a_conversations_citations_become_the_manuscripts_with_their_passages() {
        let (mut store, made, paper) = library();
        let section = made.sections[1].id.clone();
        let sources = vec![Source { label: "P1".into(), paper_id: paper.id.clone(), title: paper.title.clone(), ..Default::default() }];
        let mut args = json!({"section": section, "edits": [{"old": "", "new": "WOX5 维持静止中心 [P1 \"WOX5 maintains the quiescent centre of the root.\"]。另一句没有依据 [P1 \"The paper never says this sentence at all.\"]。"}]});
        let pieces = {
            let (papers, kept) = (store.list_papers().unwrap(), store.cite_keys().unwrap());
            label_citations(&mut args, &sources, &Index::new(&papers, std::iter::empty(), &kept))
        };
        assert_eq!(args["edits"][0]["new"], "WOX5 维持静止中心 [@Zhang2021]。另一句没有依据 [@Zhang2021]。");
        assert!(args.get("text").is_none() && pieces[0].quoted.len() == 2);
        edit(&mut store, &args).unwrap();
        let body = store.manuscript(&made.id).unwrap().unwrap().section(&section).unwrap().body.clone();
        // Only the sentence the paper says is kept as what its citation rests on.
        assert_eq!(keep_quoted(&mut store, &made.id, &section, &body, &pieces).unwrap(), (1, vec!["The paper never says this sentence at all.".to_string()]));
        let kept = store.passages(&made.id).unwrap();
        assert_eq!((kept.len(), kept[0].paper_id.as_str(), kept[0].passage.as_str(), kept[0].claim.as_str()), (1, paper.id.as_str(), "WOX5 maintains the quiescent centre of the root.", "WOX5 维持静止中心。"));
        // The same through the call a conversation makes: what was kept and what was not is said, and a label of no paper too.
        let info = crate::libraries::LibraryInfo::default();
        let nothing = |_: &str| -> Result<String, String> { Err("no network".into()) };
        let none = |_: &str| -> Result<Vec<u8>, String> { Err("no network".into()) };
        let done = call(&mut store, &info, "edit_section", &json!({"section": section, "edits": [{"old": "", "new": "再写一句 [P1 \"WOX5 maintains the quiescent centre of the root.\"]，又一句 [P9]。"}]}), &sources, &nothing, &none).unwrap();
        assert_eq!((done["passagesKept"].as_u64(), done["unknownLabels"][0].as_str(), done["quotesNotFound"].is_null()), (Some(1), Some("P9"), true));
        assert!(store.manuscript(&made.id).unwrap().unwrap().section(&section).unwrap().body.ends_with("再写一句 [@Zhang2021]，又一句 [P9]。"));
        // A sentence that repeats the paper it cites is handed back to be said in the manuscript's own words, with or without a
        // sentence quoted for the citation; what stands between quotation marks is not, nor a sentence written before this call.
        let done = call(&mut store, &info, "edit_section", &json!({"section": section, "edits": [{"old": "", "new": "It is known that WOX5 maintains the quiescent centre of the root [P1]. As they put it, “WOX5 maintains the quiescent centre of the root” [P1 \"WOX5 maintains the quiescent centre of the root.\"]."}]}), &sources, &nothing, &none).unwrap();
        assert_eq!(done["copied"], json!([{"sentence": "It is known that WOX5 maintains the quiescent centre of the root.", "shared": "wox5 maintains the quiescent centre of the root"}]));
        assert!(done["copiedHint"].as_str().unwrap().contains("改成自己的话") && done["passagesKept"] == 1);
        assert!(crate::tools::describe_result(&Ok(done)).ends_with("」，1 句和所引文献的原文大段相同"));
        let done = call(&mut store, &info, "edit_section", &json!({"section": section, "edits": [{"old": "", "new": "Its loss lets the centre divide [P1]."}]}), &sources, &nothing, &none).unwrap();
        assert!(done["copied"].is_null() && done["edited"] == 1);
        // Any other tool goes as the table has it.
        assert!(call(&mut store, &info, "list_manuscripts", &json!({}), &sources, &nothing, &none).unwrap()["manuscripts"].is_array());
    }

    #[test]
    fn a_sentence_that_repeats_its_source() {
        let source = "Protoplast isolation is often limited by stress-induced artifacts, low cell recovery, and reduced applicability to lignified tissues.";
        // Eight words one after another are the paper's sentence again, whatever is put around them; case and punctuation do not hide it.
        assert!(repeats("In plants, protoplast isolation is often limited by stress-induced artifacts and low cell recovery [@Wang2026, p. 2].", source));
        // The same said otherwise shares its terms and no more.
        assert!(!repeats("Stress-induced artifacts and low cell recovery limit protoplast isolation, above all in lignified tissues [@Wang2026, p. 2].", source));
        // A hyphenated term is one word: a name alone is never a run.
        assert_eq!(words("Single-nucleus RNA sequencing (snRNA-seq) can’t").iter().map(|(word, _)| word.as_str()).collect::<Vec<_>>(), ["single-nucleus", "rna", "sequencing", "snrna-seq", "can’t"]);
        // What is quoted as another's words is not the manuscript's own, and a citation between two stretches parts them.
        assert!(!repeats("They note that “protoplast isolation is often limited by stress-induced artifacts, low cell recovery” in such tissues.", source));
        assert!(!repeats("Protoplast isolation is often limited [@A] by stress-induced artifacts, low cell recovery.", source));
        // Chinese counts by the character, two to a word: fifteen in a row are not yet a run, sixteen are.
        let theirs = "原生质体制备会引入胁迫响应并使部分细胞类型的回收率偏低。";
        assert!(!repeats("我们发现原生质体制备会引入胁迫响应并使。", theirs) && repeats("我们发现原生质体制备会引入胁迫响应并使部。", theirs));
        assert!(!repeats("", source) && !repeats("Roots grow.", ""));
    }

    #[test]
    fn the_note_says_where_the_manuscript_stands() {
        let (mut store, made, _) = library();
        store.save_section_body(&made.sections[1].id, "写了 六个字。").unwrap();
        let now = store.manuscript(&made.id).unwrap().unwrap();
        let said = note(&Asked { manuscript: &now, showing: &now.sections[1].id, selection: " 六个字 " });
        assert!(said.contains(&format!("（ID {}）", now.id)) && said.contains("正文用中文写"));
        assert!(said.contains(&format!("2. {} · {} · 导言 · 6 字 ← 右边正显示这一节", now.sections[1].id, now.sections[1].name)));
        assert!(said.contains(&format!("1. {} · {} · 摘要 · 未写\n", now.sections[0].id, now.sections[0].name)));
        assert!(said.contains(&format!("用户在「{}」里选中了下面这段文字", now.sections[1].name)) && said.contains("\n六个字\n"));
        assert!(!note(&Asked { manuscript: &now, showing: "", selection: "" }).contains("选中了"));
        // How to write: after the library's sentences of the same part, asked for in the manuscript's language when the tool knows it.
        assert!(said.contains("先用 find_sentences 看文献库里的论文在同一部分是怎么写的") && said.contains("language 用 \"zh\"") && said.contains("用户写的句子，要求之外的一字不动") && said.contains("\n- 用自己的话写：") && said.contains("结果里有 copied 时"));
        let odd = Manuscript { language: "la".into(), ..now.clone() };
        let said = note(&Asked { manuscript: &odd, showing: "", selection: "" });
        assert!(said.contains("写法要有参照") && !said.contains("language 用"));
    }

    #[test]
    fn what_changed_between_two_texts() {
        assert_eq!(hunks("a\n\nb\n\nc", "a\n\nb\n\nc"), vec![]);
        // A paragraph changed in place.
        assert_eq!(hunks("a\n\nb\n\nc", "a\n\nB!\n\nc"), vec![Hunk { before: 3..5, after: 3..6 }]);
        // One put in, one taken out, each a hunk of its own.
        let (before, after) = ("one\n\ntwo\n\nthree\n\nfour", "one\n\nnew\n\ntwo\n\nthree");
        let found = hunks(before, after);
        assert_eq!(found.len(), 2);
        assert_eq!((&before[found[0].before.clone()], &after[found[0].after.clone()]), ("", "new\n\n"));
        assert_eq!((&before[found[1].before.clone()], &after[found[1].after.clone()]), ("\n\nfour", ""));
        // Words added at the end of an empty text, and everything taken away.
        assert_eq!(hunks("", "x\n\ny"), vec![Hunk { before: 0..0, after: 0..4 }]);
        assert_eq!(hunks("x", ""), vec![Hunk { before: 0..1, after: 0..0 }]);
        // Applying the hunks from the last to the first makes the one text of the other.
        let mut made = before.to_string();
        for hunk in found.iter().rev() { made.replace_range(hunk.before.clone(), &after[hunk.after.clone()]) }
        assert_eq!(made, after);
        assert_eq!(hunks("a\n", "a"), vec![Hunk { before: 1..2, after: 1..1 }]);
    }
}
