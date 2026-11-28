//! 术语表: how the library's terms are translated, kept with the library (a library is one research topic, so its
//! manuscripts and its reading share one table). A row pairs a Chinese term with its counterpart in one other
//! language (`manuscript::LANGUAGES`, English unless the person chose another) and is used both ways: a Chinese
//! text put into English and an English text put into Chinese. Between two languages neither of which is Chinese,
//! the rows of the two that name the same Chinese term are a pair through it (`pairs`).
//!
//! A request carries only the terms that occur in the text sent (`occurring`, `instructions`): a paragraph
//! without any is asked as it was before there was a table, so what was kept for it under its text and its
//! instructions (`store::translation_key`) still matches, and a term changed asks again only for what holds it.
//! What came back is looked over here too, with no model (`check`): a term of the table the translation does not
//! use, a number that is not in it.
//!
//! Rows are written only on the person's action: what a model proposes (`propose_prompt`, `parse_proposals`) and
//! what a one-off requirement names (`named_term`) are offered for the person to take.

use crate::store::Store;
use crate::structure::is_cjk;
use crate::time::now_iso;
use anyhow::{Result, bail};
use rusqlite::params;

/// The table, made with the library's own. An older build leaves it alone.
pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS glossary_terms (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- The Chinese term.
    zh TEXT NOT NULL,
    -- Its counterpart, in `language`.
    other TEXT NOT NULL,
    -- A code of `manuscript::LANGUAGES` other than `zh`.
    language TEXT NOT NULL DEFAULT 'en',
    created_at TEXT NOT NULL,
    -- One counterpart a language: a term translated two ways is no rule.
    UNIQUE (zh, language)
);
"#;

/// The language a row's other side is in while the person chose none.
pub const DEFAULT_LANGUAGE: &str = "en";

/// The most terms a request carries: past that a table is a page of its own, and a paragraph holds far fewer.
const MOST_SENT: usize = 40;

/// A row of the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    pub id: i64,
    pub zh: String,
    pub other: String,
    /// The other side's language, a code of `manuscript::LANGUAGES`.
    pub language: String,
}

impl Term {
    /// The side of the row in `language`, if it has one.
    fn side(&self, language: &str) -> Option<&str> {
        if language == "zh" { Some(&self.zh) } else if self.language == language { Some(&self.other) } else { None }
    }
}

impl Store {
    /// Every row, the newest first: one just added is at the top, where it was typed.
    pub fn glossary(&self) -> Result<Vec<Term>> {
        let mut stmt = self.db.prepare("SELECT id, zh, other, language FROM glossary_terms ORDER BY id DESC")?;
        let rows = stmt.query_map([], |r| Ok(Term { id: r.get(0)?, zh: r.get(1)?, other: r.get(2)?, language: r.get(3)? }))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Add a row, or give the Chinese term a new counterpart in that language where it had one: the person's latest word stands.
    pub fn add_term(&mut self, zh: &str, other: &str, language: &str) -> Result<i64> {
        let (zh, other, language) = (zh.trim(), other.trim(), language.trim());
        if zh.is_empty() || other.is_empty() { bail!("术语的两边都要填。") }
        if language == "zh" || !crate::manuscript::LANGUAGES.iter().any(|(code, _)| *code == language) { bail!("术语表不认识这种语言：{language}") }
        self.db.execute("INSERT INTO glossary_terms (zh, other, language, created_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (zh, language) DO UPDATE SET other = excluded.other", params![zh, other, language, now_iso()])?;
        Ok(self.db.query_row("SELECT id FROM glossary_terms WHERE zh = ?1 AND language = ?2", params![zh, language], |r| r.get(0))?)
    }

    /// Change both sides of a row.
    pub fn set_term(&mut self, id: i64, zh: &str, other: &str) -> Result<()> {
        let (zh, other) = (zh.trim(), other.trim());
        if zh.is_empty() || other.is_empty() { bail!("术语的两边都要填。") }
        let language: String = self.db.query_row("SELECT language FROM glossary_terms WHERE id = ?1", [id], |r| r.get(0))?;
        let taken: Option<i64> = self.db.query_row("SELECT id FROM glossary_terms WHERE zh = ?1 AND language = ?2 AND id != ?3", params![zh, language, id], |r| r.get(0)).ok();
        if taken.is_some() { bail!("术语表里已经有「{zh}」了。") }
        self.db.execute("UPDATE glossary_terms SET zh = ?2, other = ?3 WHERE id = ?1", params![id, zh, other])?;
        Ok(())
    }

    pub fn remove_term(&mut self, id: i64) -> Result<()> {
        self.db.execute("DELETE FROM glossary_terms WHERE id = ?1", [id])?;
        Ok(())
    }
}

/// The terms for a text in `from` put into `to` (codes of `manuscript::LANGUAGES`), each as the text has it and as
/// the result is to have it. In one language (a text reworded) each term is to stay as it is.
pub fn pairs(terms: &[Term], from: &str, to: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for term in terms {
        let pair = if from == to {
            term.side(from).map(|word| (word.to_string(), word.to_string()))
        } else if from == "zh" || to == "zh" {
            term.side(from).zip(term.side(to)).map(|(a, b)| (a.to_string(), b.to_string()))
        } else {
            // Through the Chinese term both rows name.
            term.side(from).and_then(|a| terms.iter().find(|other| other.zh == term.zh && other.language == to).map(|b| (a.to_string(), b.other.clone())))
        };
        if let Some(pair) = pair.filter(|(a, b)| !a.trim().is_empty() && !b.trim().is_empty()) && !out.iter().any(|(a, _)| a == &pair.0) { out.push(pair) }
    }
    out
}

/// A letter of a script that writes spaces between its words, or a digit: a term of such a script is a word of its own.
fn wordish(c: char) -> bool {
    c.is_alphanumeric() && !is_cjk(c)
}

/// The endings of a word of a science's English that change in its plural (`apex` → `apices`, `nucleus` → `nuclei`,
/// `stoma` → `stomata`), the singular's first: a term is found in its plural too, and the plain `s` and `es` are
/// allowed after any word.
const PLURALS: [(&str, &str); 12] = [("ex", "ices"), ("ix", "ices"), ("us", "i"), ("um", "a"), ("on", "a"), ("is", "es"), ("ma", "mata"), ("a", "ae"), ("y", "ies"), ("fe", "ves"), ("f", "ves"), ("x", "ces")];

/// A term as written and in the plurals its last word may take.
fn forms(term: &str) -> Vec<String> {
    let mut forms = vec![term.to_string()];
    let last = term.rsplit(' ').next().unwrap_or(term);
    if last.chars().count() > 3 && last.chars().all(|c| c.is_ascii_alphabetic()) {
        for (singular, plural) in PLURALS {
            if let Some(stem) = term.strip_suffix(singular) { forms.push(format!("{stem}{plural}")) }
        }
    }
    forms
}

/// Whether `term` occurs in `text`. A term in a script that writes spaces stands as a word, a plural allowed after
/// it (`s`, `es`, or one of `PLURALS`), and is found in any case, unless it has a capital after its first letter
/// (`SHR`, `mRNA`): a gene's name in capitals is not its mutant's in small letters. A Chinese term is found
/// wherever it is written.
pub fn occurs(text: &str, term: &str) -> bool {
    let term = term.trim();
    if term.is_empty() { return false }
    let exact = term.chars().skip(1).any(char::is_uppercase);
    let (hay, term) = if exact { (text.to_string(), term.to_string()) } else { (text.to_lowercase(), term.to_lowercase()) };
    let free = |rest: &str| !rest.chars().next().is_some_and(wordish);
    forms(&term).iter().any(|needle| {
        let (first, last) = (needle.chars().next(), needle.chars().next_back());
        let mut from = 0;
        while let Some(at) = hay[from..].find(needle.as_str()) {
            let (start, end) = (from + at, from + at + needle.len());
            let before = !first.is_some_and(wordish) || !hay[..start].chars().next_back().is_some_and(wordish);
            let rest = &hay[end..];
            let after = !last.is_some_and(wordish) || free(rest) || ["es", "s"].iter().any(|plural| rest.strip_prefix(plural).is_some_and(free));
            if before && after { return true }
            from = start + hay[start..].chars().next().map_or(1, char::len_utf8);
        }
        false
    })
}

/// Those of `pairs` whose first side occurs in `text`, the longest first, at most `MOST_SENT`.
pub fn occurring(text: &str, pairs: &[(String, String)]) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = pairs.iter().filter(|(term, _)| occurs(text, term)).cloned().collect();
    found.sort_by(|a, b| b.0.chars().count().cmp(&a.0.chars().count()).then_with(|| a.0.cmp(&b.0)));
    found.truncate(MOST_SENT);
    found
}

/// The lines that tell a model the terms: a table to translate by, or, in one language, the words to keep.
pub fn terms_said(terms: &[(String, String)]) -> String {
    if terms.is_empty() { return String::new() }
    if terms.iter().all(|(a, b)| a == b) {
        let listed: Vec<String> = terms.iter().map(|(a, _)| format!("- {a}")).collect();
        return format!("下面这些术语照原样写，不换成别的说法：\n{}", listed.join("\n"));
    }
    let listed: Vec<String> = terms.iter().map(|(a, b)| format!("- {a} → {b}")).collect();
    format!("术语按下表译（原文 → 译文），每一处都照这样译：\n{}", listed.join("\n"))
}

/// `base` with the terms that occur in what is sent and the manuscript's standing requirement after it. With
/// neither, `base` as it is: the instructions, and so what is kept under them, are those of before the table.
pub fn instructions(base: &str, terms: &[(String, String)], requirement: &str) -> String {
    let requirement = requirement.trim();
    if terms.is_empty() && requirement.is_empty() { return base.to_string() }
    let mut out = base.trim_end().to_string();
    if !terms.is_empty() {
        out.push_str("\n\n");
        out.push_str(&terms_said(terms));
    }
    if !requirement.is_empty() {
        out.push_str("\n\n这份稿件的要求：");
        out.push_str(requirement);
    }
    out
}

/// What a translation lacks that its source has, found with no model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lack {
    /// The source has `source`, a term of the table, and the translation not `wanted`, its counterpart.
    Term { source: String, wanted: String },
    /// A number of the source, as the source writes it, that the translation does not have.
    Number(String),
}

/// A text with its citations (`[@Key]`, the placeholders `[@1]`) taken out: their numbers are not the text's.
fn without_citations(text: &str) -> String {
    let (mut out, mut rest) = (String::new(), text);
    while let Some(at) = rest.find("[@") {
        out.push_str(&rest[..at]);
        match rest[at..].find(']') {
            Some(end) => {
                out.push(' ');
                rest = &rest[at + end + 1..];
            }
            None => {
                rest = &rest[at..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The numbers of a text as it writes them (`3.5`, `1,000`, `12`), each with the value it is read as
/// (`3.5`, `1000`, `12`) and whether it stands by a sign that makes even a small one worth keeping (`3%`).
/// A number glued to letters (`CO2`, `AtSHR1`, `10mM`) is part of a name or a unit, not one of these.
fn numbers(text: &str, decimal_comma: bool) -> Vec<(String, String, bool)> {
    let text: String = text.chars().map(|c| if ('０'..='９').contains(&c) { char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c) } else if c == '．' { '.' } else { c }).collect();
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![];
    let mut at = 0;
    while at < chars.len() {
        if !chars[at].is_ascii_digit() {
            at += 1;
            continue;
        }
        let start = at;
        while at < chars.len() && (chars[at].is_ascii_digit() || (matches!(chars[at], '.' | ',') && chars.get(at + 1).is_some_and(char::is_ascii_digit) && at > start)) { at += 1 }
        let glued = (start > 0 && chars[start - 1].is_alphabetic() && !is_cjk(chars[start - 1])) || chars.get(at).is_some_and(|c| c.is_alphabetic() && !is_cjk(*c));
        if glued { continue }
        let written: String = chars[start..at].iter().collect();
        // `1,000` groups thousands; `3,5` is a decimal comma in a language that writes one, else two numbers.
        let grouped = written.contains(',') && written.split(',').skip(1).all(|group| group.len() == 3 && !group.contains('.'));
        let values: Vec<String> = if grouped {
            vec![written.replace(',', "")]
        } else if written.contains(',') && decimal_comma {
            vec![written.replace(',', ".")]
        } else {
            written.split(',').map(str::to_string).collect()
        };
        let signed = chars[at..].iter().find(|c| !c.is_whitespace()).is_some_and(|c| matches!(c, '%' | '‰' | '℃' | '°' | '％'));
        for value in values {
            let value = value.trim_end_matches('.').to_string();
            if !value.is_empty() { out.push((written.clone(), value, signed)) }
        }
    }
    out
}

/// Whether a number is worth looking for in a translation: one with a fraction, one past ten, or one by a sign.
/// A small whole number is often written out (`three replicates`, `两次`) and would only be a false alarm.
fn worth_checking(value: &str, signed: bool) -> bool {
    signed || value.contains('.') || value.parse::<u64>().map_or(true, |n| n > 10)
}

/// The terms of `terms` (pairs as `pairs` gives them) that `source` holds and `translation` does not hold the
/// counterpart of, each counterpart once. None for a translation not yet made.
fn unused<'a>(source: &str, translation: &str, terms: &'a [(String, String)]) -> Vec<&'a (String, String)> {
    let mut out: Vec<&(String, String)> = vec![];
    if translation.trim().is_empty() { return out }
    for pair in terms {
        if occurs(source, &pair.0) && !occurs(translation, &pair.1) && !out.iter().any(|(_, wanted)| wanted == &pair.1) { out.push(pair) }
    }
    out
}

/// Whether `translation` leaves out the counterpart of a term of `terms` that `source` holds: a translation made
/// before the term was in the table, or before its counterpart was changed, or by a model that did not keep to it.
pub fn lacks_term(source: &str, translation: &str, terms: &[(String, String)]) -> bool {
    !unused(source, translation, terms).is_empty()
}

/// What `translation` lacks of `source`: each term of `terms` (pairs as `pairs` gives them) the source holds and the
/// translation does not hold the counterpart of, then each number of the source it does not have.
pub fn check(source: &str, translation: &str, terms: &[(String, String)]) -> Vec<Lack> {
    if translation.trim().is_empty() { return vec![] }
    let mut lacks: Vec<Lack> = unused(source, translation, terms).into_iter().map(|(term, wanted)| Lack::Term { source: term.clone(), wanted: wanted.clone() }).collect();
    let (source, translation) = (without_citations(source), without_citations(translation));
    let had: Vec<String> = numbers(&translation, true).into_iter().chain(numbers(&translation, false)).map(|(_, value, _)| value).collect();
    for (written, value, signed) in numbers(&source, false) {
        if !worth_checking(&value, signed) || had.contains(&value) { continue }
        if !lacks.contains(&Lack::Number(written.clone())) { lacks.push(Lack::Number(written)) }
    }
    lacks
}

/// The words that say a term is to be translated another way, the longest first so that `翻译成` is not read as `译成`.
const NAMING: [&str; 13] = ["统一翻译成", "统一译作", "统一译为", "翻译成", "翻译为", "翻译作", "译作", "译为", "译成", "翻成", "=>", "->", "→"];

/// A term a requirement typed for one request names (`niche 译作 微环境`, `把根毛译成 root hair`,
/// `translate niche as 微环境`), as a row of the table: its Chinese side and the other. `None` when no clause of
/// the requirement names one, or neither side or both are Chinese.
pub fn named_term(requirement: &str) -> Option<(String, String)> {
    let clean = |side: &str| side.trim().trim_matches(|c: char| matches!(c, '「' | '」' | '“' | '”' | '"' | '\'' | '‘' | '’' | '`' | '《' | '》') || c.is_whitespace()).to_string();
    for clause in requirement.split(['；', ';', '，', '。', '\n', '、']) {
        let clause = clause.trim();
        let lower = clause.to_lowercase();
        let (a, b) = if let Some(rest) = lower.strip_prefix("translate ").or_else(|| lower.strip_prefix("render ")) && let Some(at) = rest.find(" as ") {
            let skip = clause.len() - rest.len();
            (clause[skip..skip + at].to_string(), clause[skip + at + 4..].to_string())
        } else {
            let Some((at, word)) = NAMING.iter().filter_map(|word| clause.find(word).map(|at| (at, *word))).min_by_key(|(at, word)| (*at, std::cmp::Reverse(word.len()))) else { continue };
            let before = clause[..at].trim();
            let before = ["请把", "请将", "统一把", "统一将", "把", "将"].iter().find_map(|lead| before.strip_prefix(lead)).unwrap_or(before);
            (before.to_string(), clause[at + word.len()..].to_string())
        };
        let (a, b) = (clean(&a), clean(&b));
        if a.is_empty() || b.is_empty() || a == b || a.chars().count() > 60 || b.chars().count() > 60 { continue }
        let chinese = |side: &str| side.chars().any(is_cjk);
        match (chinese(&a), chinese(&b)) {
            (true, false) => return Some((a, b)),
            (false, true) => return Some((b, a)),
            _ => continue,
        }
    }
    None
}

/// The most of a manuscript a request for terms reads, in bytes: the start of a long one has most of its names.
pub const PROPOSE_LIMIT: usize = 30_000;

/// What a model is asked for when it proposes terms: the manuscript (`text`, written in `written`, a code) and the
/// language of the table's other side; the terms the table has already are left out of the answer. The
/// instructions and the message.
pub fn propose_prompt(text: &str, written: &str, other: &str, known: &[String]) -> (String, String) {
    fn name(code: &str) -> &str {
        if code == "zh" { "中文" } else { crate::model::label(&crate::manuscript::LANGUAGES, code) }
    }
    let other = name(other);
    let system = format!(
        "你是学术翻译的术语助手。从用户给出的论文稿件里找出需要统一译法的专有名词和这个领域的术语：基因和蛋白、物种、组织和结构、方法和仪器、软件、数据库、机构、专门的概念。\n\
         每个术语给出它的中文和{other}，各一行，格式是：中文 | {other}\n\
         稿件里写的是哪一边就照稿件原样写那一边，另一边给出这个领域的论文通用的译法。基因名、物种学名保留原样。\n\
         只列这一篇里真正出现的，最多 40 个；常用词不要列。只输出这些行，不加编号、标题和解释。"
    );
    let mut cut = text.trim();
    if cut.len() > PROPOSE_LIMIT { cut = &cut[..cut.floor_char_boundary(PROPOSE_LIMIT)] }
    let mut message = format!("稿件（{}）：\n\n{cut}", name(written));
    if !known.is_empty() { message.push_str(&format!("\n\n这些已经在术语表里，不要再列：{}", known.join("、"))) }
    (system, message)
}

/// The terms a model proposed, each as a row would hold it (Chinese, other): one a line, the two sides parted by `|`,
/// an arrow, a tab or a colon, whatever list or table marks the model put around them. A line whose sides are both
/// Chinese or neither is no term, and a table's head is none.
pub fn parse_proposals(reply: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    let strip = |side: &str| side.trim().trim_matches(|c: char| matches!(c, '*' | '`' | '"' | '“' | '”' | '「' | '」' | '|') || c.is_whitespace()).trim_end_matches(['；', ';', '，', ',', '。']).trim().to_string();
    for line in reply.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("```") || line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '=')) { continue }
        // A bullet or a number before the line.
        let line = line.trim_start_matches(['-', '*', '•', '·']).trim_start();
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        let line = if digits > 0 && line[digits..].starts_with(['.', ')', '、', '．']) { line[digits + 1..].trim_start() } else { line };
        let line = line.trim_matches('|').trim();
        let Some((a, b)) = [" | ", "|", "=>", "→", "->", "\t", "：", ": ", " — ", " – ", " - "].iter().find_map(|mark| line.split_once(mark)) else { continue };
        // A table may have more columns: the first two are the term.
        let b = b.split('|').next().unwrap_or(b);
        let (a, b) = (strip(a), strip(b));
        if a.is_empty() || b.is_empty() { continue }
        let chinese = |side: &str| side.chars().any(is_cjk);
        let (zh, other) = match (chinese(&a), chinese(&b)) {
            (true, false) => (a, b),
            (false, true) => (b, a),
            _ => continue,
        };
        let head = ["中文", "中文术语", "术语", "原文", "中文名", "中文名称"].contains(&zh.as_str()) || ["english", "translation", "term", "英文", "译名"].contains(&other.to_lowercase().as_str());
        if head || out.iter().any(|(known, _)| *known == zh) { continue }
        out.push((zh, other));
    }
    out
}

/// What a text is written in, a code of `manuscript::LANGUAGES`: Chinese or Japanese by its letters, else the
/// manuscript's language (`written`) where that writes in Latin letters, else English.
pub fn text_language(text: &str, written: &str) -> &'static str {
    let kana = text.chars().filter(|c| matches!(c, '\u{3040}'..='\u{30ff}')).count();
    let cjk = text.chars().filter(|c| is_cjk(*c)).count();
    let latin = text.chars().filter(|c| c.is_alphabetic() && !is_cjk(*c)).count();
    if cjk > 0 && cjk * 3 >= latin {
        return if kana * 5 >= cjk && kana > 0 { "ja" } else { "zh" };
    }
    crate::manuscript::LANGUAGES.iter().map(|(code, _)| *code).find(|code| *code == written && !matches!(*code, "zh" | "ja")).unwrap_or("en")
}

/// Where a text in `source` is translated to unless the person says otherwise: a text in the manuscript's language
/// (`written`) to the other of Chinese and English (English for any but English), any other to the manuscript's.
pub fn default_target(source: &str, written: &str) -> &'static str {
    let code = |wanted: &str| crate::manuscript::LANGUAGES.iter().map(|(code, _)| *code).find(|code| *code == wanted);
    if source != written && let Some(written) = code(written) { return written }
    if source == "en" { "zh" } else { "en" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(id: i64, zh: &str, other: &str, language: &str) -> Term {
        Term { id, zh: zh.into(), other: other.into(), language: language.into() }
    }

    #[test]
    fn rows_are_kept_one_counterpart_a_language() {
        let mut store = Store::in_memory(std::path::Path::new("/tmp")).unwrap();
        let niche = store.add_term(" 微环境 ", "niche", "en").unwrap();
        store.add_term("根毛", "root hair", "en").unwrap();
        store.add_term("微环境", "Nische", "de").unwrap();
        // The same Chinese term again in the same language: the latest word stands, in the same row.
        assert_eq!(store.add_term("微环境", "microenvironment", "en").unwrap(), niche);
        let rows = store.glossary().unwrap();
        assert_eq!(rows.iter().map(|t| (t.zh.as_str(), t.other.as_str(), t.language.as_str())).collect::<Vec<_>>(), vec![("微环境", "Nische", "de"), ("根毛", "root hair", "en"), ("微环境", "microenvironment", "en")]);
        assert!(store.add_term("", "x", "en").is_err() && store.add_term("x", "y", "zh").is_err() && store.add_term("x", "y", "xx").is_err());
        let hair = rows[1].id;
        assert!(store.set_term(hair, "微环境", "x").is_err());
        store.set_term(hair, "根毛", "root hairs").unwrap();
        store.remove_term(niche).unwrap();
        assert_eq!(store.glossary().unwrap().iter().map(|t| t.other.as_str()).collect::<Vec<_>>(), vec!["Nische", "root hairs"]);
    }

    #[test]
    fn a_row_is_used_both_ways_and_through_chinese() {
        let terms = vec![term(1, "微环境", "niche", "en"), term(2, "根毛", "root hair", "en"), term(3, "微环境", "Nische", "de"), term(4, "根冠", "Wurzelhaube", "de")];
        assert_eq!(pairs(&terms, "zh", "en"), vec![("微环境".into(), "niche".into()), ("根毛".into(), "root hair".into())]);
        assert_eq!(pairs(&terms, "en", "zh"), vec![("niche".into(), "微环境".into()), ("root hair".into(), "根毛".into())]);
        assert_eq!(pairs(&terms, "en", "de"), vec![("niche".to_string(), "Nische".to_string())]);
        assert_eq!(pairs(&terms, "de", "en"), vec![("Nische".to_string(), "niche".to_string())]);
        assert_eq!(pairs(&terms, "en", "en"), vec![("niche".to_string(), "niche".to_string()), ("root hair".to_string(), "root hair".to_string())]);
        assert_eq!(pairs(&terms, "zh", "zh").len(), 3);
        assert!(pairs(&terms, "fr", "zh").is_empty());
    }

    #[test]
    fn terms_are_found_as_words() {
        assert!(occurs("根尖分生组织里的干细胞", "分生组织"));
        assert!(occurs("The stem-cell Niche of the root", "niche"));
        assert!(occurs("two niches", "niche") && occurs("root hairs grow", "root hair"));
        assert!(!occurs("nichesque", "niche") && !occurs("subniche", "niche") && !occurs("root hairy", "root hair"));
        // The plurals of a science's English: as a model writes them.
        assert!(occurs("root apices were examined", "root apex") && occurs("in the nuclei", "nucleus") && occurs("open stomata", "stoma"));
        assert!(occurs("fallen leaves", "leaf") && occurs("two hyphae", "hypha") && occurs("cell bodies", "cell body") && occurs("their analyses", "analysis"));
        assert!(!occurs("the apicesque", "root apex") && !occurs("apices", "apex x"));
        // A name with capitals after its first letter is the name in that case only.
        assert!(occurs("SHR moves", "SHR") && !occurs("the shr mutant", "SHR"));
        assert!(occurs("mRNA levels", "mRNA") && !occurs("MRNA", "mRNA"));
        assert!(!occurs("anything", " ") && !occurs("", "x"));
        let pairs = vec![("分生组织".to_string(), "meristem".to_string()), ("根尖分生组织".to_string(), "root apical meristem".to_string()), ("根毛".to_string(), "root hair".to_string())];
        assert_eq!(occurring("根尖分生组织的细胞", &pairs), vec![pairs[1].clone(), pairs[0].clone()]);
        assert!(occurring("叶片", &pairs).is_empty());
    }

    #[test]
    fn instructions_change_only_where_a_term_or_a_requirement_is() {
        let base = "你是翻译助手。只输出译文。";
        assert_eq!(instructions(base, &[], "  "), base);
        let terms = vec![("微环境".to_string(), "niche".to_string())];
        assert_eq!(instructions(base, &terms, ""), "你是翻译助手。只输出译文。\n\n术语按下表译（原文 → 译文），每一处都照这样译：\n- 微环境 → niche");
        assert_eq!(instructions(base, &[], "美式拼写"), "你是翻译助手。只输出译文。\n\n这份稿件的要求：美式拼写");
        let kept = vec![("niche".to_string(), "niche".to_string())];
        assert!(instructions(base, &kept, "").ends_with("下面这些术语照原样写，不换成别的说法：\n- niche"));
    }

    #[test]
    fn a_translation_is_looked_over_with_no_model() {
        let terms = vec![("微环境".to_string(), "niche".to_string()), ("根毛".to_string(), "root hair".to_string())];
        let source = "干细胞微环境在 3.5 天内形成，约 1,200 个细胞（p < 0.05），重复三次，2 个品系，5% 蔗糖 [@Zhang2024]。";
        let good = "The stem-cell niche forms within 3.5 days, about 1200 cells (p < 0.05), three replicates, two lines, 5% sucrose [@1].";
        assert_eq!(check(source, good, &terms), vec![]);
        let bad = "The stem-cell microenvironment forms within a few days, about 1,200 cells (p < 0.5), with sucrose.";
        assert_eq!(check(source, bad, &terms), vec![Lack::Term { source: "微环境".into(), wanted: "niche".into() }, Lack::Number("3.5".into()), Lack::Number("0.05".into()), Lack::Number("5".into())]);
        // A decimal comma, a year, names with digits in them, citations.
        assert_eq!(check("增长 2.5 倍（2024 年）", "Anstieg um das 2,5-Fache (2024)", &[]), vec![]);
        assert_eq!(check("CO2 和 AtSHR1 [@Li2019 p.12]", "CO₂ and AtSHR1", &[]), vec![]);
        assert_eq!(check("第 12 页", "", &[]), vec![]);
        assert_eq!(check("１２ 株", "12 plants", &[]), vec![]);
    }

    #[test]
    fn a_translation_may_leave_out_a_term() {
        let terms = pairs(&[Term { id: 1, zh: "底层基因".into(), other: "foundational gene".into(), language: "en".into() }], "zh", "en");
        let source = "我们整理出一份底层基因列表。";
        assert!(lacks_term(source, "We compiled an underlying gene list.", &terms));
        // Its plural and a capital at the head of a sentence are the term.
        assert!(!lacks_term(source, "We compiled a list of foundational genes.", &terms));
        assert!(!lacks_term(source, "Foundational gene lists were compiled.", &terms));
        // A source without the term, a translation not yet made, no table.
        assert!(!lacks_term("其余的基因不稳定。", "The other genes are unstable.", &terms));
        assert!(!lacks_term(source, "", &terms));
        assert!(!lacks_term(source, "We compiled an underlying gene list.", &[]));
    }

    #[test]
    fn a_requirement_may_name_a_term() {
        assert_eq!(named_term("niche 译作 微环境"), Some(("微环境".into(), "niche".into())));
        assert_eq!(named_term("用被动语态；把根毛译成「root hair」"), Some(("根毛".into(), "root hair".into())));
        assert_eq!(named_term("Translate niche as 微环境"), Some(("微环境".into(), "niche".into())));
        assert_eq!(named_term("微环境 → niche"), Some(("微环境".into(), "niche".into())));
        assert_eq!(named_term("根毛统一翻译成 root hairs。"), Some(("根毛".into(), "root hairs".into())));
        assert_eq!(named_term("用被动语态"), None);
        assert_eq!(named_term("niche → Nische"), None);
        assert_eq!(named_term("译成英文"), None);
    }

    #[test]
    fn proposals_are_read_however_they_are_listed() {
        let reply = "以下是术语：\n| 中文 | English |\n|---|---|\n| 拟南芥 | *Arabidopsis thaliana* |\n| **根毛** | root hair | 结构 |\n1. 微环境 → niche\n- 共聚焦显微镜: confocal microscope\nroot apical meristem | 根尖分生组织\n拟南芥 | Arabidopsis\n根 | 根\nsucrose | sucrose\n```\n";
        assert_eq!(
            parse_proposals(reply),
            vec![("拟南芥".into(), "Arabidopsis thaliana".into()), ("根毛".into(), "root hair".into()), ("微环境".into(), "niche".into()), ("共聚焦显微镜".into(), "confocal microscope".into()), ("根尖分生组织".into(), "root apical meristem".into())]
        );
        let (system, message) = propose_prompt("根毛在紧实土壤中变短。", "zh", "en", &["微环境".into()]);
        assert!(system.contains("中文 | 英文") && message.contains("根毛在紧实土壤中变短") && message.ends_with("不要再列：微环境"));
    }

    #[test]
    fn a_text_goes_to_the_sensible_language() {
        assert_eq!(text_language("根毛在紧实土壤中变短（SHR 突变体）。", "zh"), "zh");
        assert_eq!(text_language("Root hairs are shorter in compacted soil.", "zh"), "en");
        assert_eq!(text_language("Die Wurzelhaare sind kürzer.", "de"), "de");
        assert_eq!(text_language("根毛は短くなる。", "en"), "ja");
        assert_eq!(default_target("zh", "zh"), "en");
        assert_eq!(default_target("en", "en"), "zh");
        assert_eq!(default_target("zh", "en"), "en");
        assert_eq!(default_target("en", "zh"), "zh");
        assert_eq!(default_target("de", "de"), "en");
        assert_eq!(default_target("en", "de"), "de");
    }
}
