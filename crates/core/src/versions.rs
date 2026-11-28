//! Several versions of the words in one answer: 换个说法 (the same language) and 译成… (another). One request asks
//! for three clearly different versions, each under a heading that says how it differs (`KINDS`), and `parse`
//! reads them back from whatever shape a model gives the headings; an answer without them is one version.

/// How a version differs from the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Close to the original: its structure and words.
    Close,
    /// As the field's papers say it.
    Idiomatic,
    /// As short as it can be with nothing lost.
    Concise,
    /// An answer that named no kind.
    Other,
}

/// The kinds asked for, in order: the heading the model writes (a key of the interface's labels too), and what it
/// is asked to make of a translation and of a rewording.
pub const KINDS: [(Kind, &str, &str, &str); 3] = [
    (Kind::Close, "贴近原文", "紧跟原文的结构和用词，意思一点不差。", "只换个别词语和句子的衔接，结构基本不动。"),
    (Kind::Idiomatic, "学术惯用", "用这个领域的学术论文惯用的说法和句式，读起来像母语作者写的。", "用这个领域的学术论文惯用的说法和句式重写。"),
    (Kind::Concise, "更简洁", "不丢任何信息，尽量简短。", "去掉冗余，尽量简短，不丢信息。"),
];

/// Other names a model gives the headings, in small letters: it may answer them in English.
const ALSO: [(Kind, &[&str]); 3] = [
    (Kind::Close, &["close to the original", "close", "faithful", "literal", "贴近原意", "忠实原文", "直译"]),
    (Kind::Idiomatic, &["idiomatic", "academic", "academic idiom", "conventional", "学术习惯", "地道"]),
    (Kind::Concise, &["concise", "more concise", "shorter", "简洁", "精简"]),
];

impl Kind {
    /// Its heading, a key of the interface's labels; empty for `Other`.
    pub fn label(self) -> &'static str {
        KINDS.iter().find(|(kind, ..)| *kind == self).map_or("", |(_, label, ..)| label)
    }
}

/// One version of the words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub kind: Kind,
    pub text: String,
}

/// What one request for versions carries besides the text: the paragraph it stands in (for sense, not to be
/// changed), the terms of the glossary that occur in it (`glossary::occurring`), the manuscript's standing
/// requirement, and what the person asked for this once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ask {
    /// The language the text is in, as a sentence names it (中文, 英文): what a translation is from.
    pub source: String,
    pub context: String,
    pub terms: Vec<(String, String)>,
    pub requirement: String,
    pub once: String,
    /// One version, not three: what each model of a comparison is asked (`compare`), the same words for each.
    pub single: bool,
}

/// The most of each paragraph around a text a request carries for its sense, in bytes.
const CONTEXT_LIMIT: usize = 1500;

/// What a request carries for the sense of `range` of `text` (a section): the paragraph it is in when it is a part
/// of one, else the paragraphs before and after it. Empty when there is none.
pub fn context(text: &str, range: std::ops::Range<usize>) -> String {
    let cut = |piece: &str| {
        let piece = piece.trim();
        if piece.len() <= CONTEXT_LIMIT { piece.to_string() } else { format!("{}…", &piece[..piece.floor_char_boundary(CONTEXT_LIMIT)]) }
    };
    let (start, end) = (text.floor_char_boundary(range.start.min(text.len())), text.floor_char_boundary(range.end.min(text.len())));
    let line = |at: usize| (text[..at].rfind('\n').map_or(0, |i| i + 1), text[at..].find('\n').map_or(text.len(), |i| at + i));
    let (from, to) = (line(start).0, line(end).1);
    let picked = text[start..end].trim();
    if !text[start..end].contains('\n') && text[from..to].trim() != picked { return cut(&text[from..to]) }
    let before = text[..from].lines().rev().find(|line| !line.trim().is_empty()).map(cut);
    let after = text[to..].lines().find(|line| !line.trim().is_empty()).map(cut);
    before.into_iter().chain(after).collect::<Vec<_>>().join("\n\n")
}

/// The instructions of a request for one version (each model of a comparison): only the words.
pub fn single_system() -> String {
    "你是学术写作助手，帮用户处理他正在写的论文稿件里的文字。\n\
     只输出处理后的文字本身：不加解释、标题、引号或 Markdown，不给几个备选。\n\
     文字里已有的引用（如 [@Smith2020]、(Smith et al., 2020)、[3]）保持原样，不删除、不改写；不要编造文献或数据。"
        .to_string()
}

/// The instructions of a request for versions: how the answer is laid out.
pub fn system() -> String {
    let headings: Vec<String> = KINDS.iter().map(|(_, label, ..)| format!("【{label}】\n……")).collect();
    format!(
        "你是学术写作助手，帮用户处理他正在写的论文稿件里的文字。\n\
         按用户要的格式输出：每个版本前一行是用【】括起来的标题，标题下面只写这个版本的文字，不加解释、引号或 Markdown。格式如下：\n{}\n\
         文字里已有的引用（如 [@Smith2020]、(Smith et al., 2020)、[3]）保持原样，不删除、不改写；不要编造文献或数据。",
        headings.join("\n")
    )
}

/// The message of a request: what is asked (`task`, a sentence), how each version differs (`translating` picks the
/// kinds' descriptions), then the terms, the requirements, the context and the text.
fn message(task: &str, translating: bool, text: &str, ask: &Ask) -> String {
    let kinds: Vec<String> = KINDS.iter().map(|(_, label, translated, reworded)| format!("【{label}】{}", if translating { translated } else { reworded })).collect();
    let mut out = if ask.single { task.to_string() } else { format!("{task}给出三个版本，每个都是完整的一份，彼此要有明显的差别：\n{}", kinds.join("\n")) };
    let terms = crate::glossary::terms_said(&ask.terms);
    if !terms.is_empty() { out.push_str(&format!("\n\n{terms}")) }
    if !ask.requirement.trim().is_empty() { out.push_str(&format!("\n\n这份稿件的要求：{}", ask.requirement.trim())) }
    if !ask.once.trim().is_empty() { out.push_str(&format!("\n\n这一次的要求{}：{}", if ask.single { "" } else { "（三个版本都照这个来）" }, ask.once.trim())) }
    if !ask.context.trim().is_empty() && ask.context.trim() != text.trim() { out.push_str(&format!("\n\n上下文（只用来理解意思，不要处理它）：\n{}", ask.context.trim())) }
    out.push_str(&format!("\n\n要处理的文字：\n{}", text.trim()));
    out
}

/// The message asking for three translations of `text` from `source` into `target` (as a sentence names them: 中文, 英文, 德文).
pub fn translate_message(source: &str, target: &str, text: &str, ask: &Ask) -> String {
    message(&format!("把下面的{source}文字译成{target}的学术论文文字：意思准确，不逐字直译；基因名、物种学名、数字和单位保持原样。"), true, text, ask)
}

/// The message asking for three other wordings of `text`, in its own language (`language`, as a sentence names it).
pub fn reword_message(language: &str, text: &str, ask: &Ask) -> String {
    message(&format!("换个说法写下面这段{language}文字：意思不变，不增删论断，仍用{language}。"), false, text, ask)
}

/// The kind a line names when it is a heading, and what follows the heading on that line.
fn heading(line: &str) -> Option<(Kind, &str)> {
    // Marks a model puts around a heading: `### `, `**`, a number, a bullet, brackets.
    let bare = line.trim().trim_start_matches(['#', '>', '-', '*', '•', ' ']).trim_start();
    let digits = bare.chars().take_while(char::is_ascii_digit).count();
    let bare = if digits > 0 && bare[digits..].starts_with(['.', ')', '、', '．', ' ']) { bare[digits + 1..].trim_start() } else { bare };
    let bare = bare.trim_start_matches(['*', '【', '[', '〔', '(', '（', '「']).trim_start();
    let lower = bare.to_lowercase();
    let names = KINDS.iter().map(|(kind, label, ..)| (*kind, *label)).chain(ALSO.iter().flat_map(|(kind, names)| names.iter().map(move |name| (*kind, *name))));
    // The longest name that leads the line: `学术惯用` before nothing shorter, `more concise` before `concise`.
    let (kind, name) = names.filter(|(_, name)| lower.starts_with(name)).max_by_key(|(_, name)| name.len())?;
    let rest = bare.get(name.len()..)?;
    // A heading ends at its closing marks; a word going on (`Closely`, `贴近原文的说法`) is no heading.
    let closing = |c: char| matches!(c, '*' | '】' | ']' | '〕' | ')' | '）' | '」' | '：' | ':' | ' ' | '-' | '—' | '.' | '#');
    if rest.chars().next().is_some_and(|c| !closing(c)) { return None }
    let rest = rest.trim_start_matches(closing).trim_start_matches(['】', ']', '）', ')', '：', ':', '—', '-', ' ', '*']).trim();
    // `Close (literal): …` and `贴近原文（直译）`: what is in brackets after the name is part of the heading.
    let rest = if rest.starts_with(['(', '（']) { rest.find([')', '）']).map_or(rest, |at| rest[at..].trim_start_matches([')', '）', '：', ':', ' ', '*'])) } else { rest };
    Some((kind, rest.trim()))
}

/// A version's text without what a model wraps around it: quotation marks, a code fence, a quote's `>`.
fn cleaned(lines: &[&str]) -> String {
    let lines: Vec<&str> = lines.iter().map(|line| line.trim_end()).filter(|line| !line.trim_start().starts_with("```")).map(|line| line.strip_prefix("> ").unwrap_or(line)).collect();
    let text = lines.join("\n");
    let text = text.trim();
    let pairs = [('"', '"'), ('“', '”'), ('「', '」')];
    let text = pairs.iter().find_map(|(open, close)| text.strip_prefix(*open).and_then(|rest| rest.strip_suffix(*close)).filter(|inner| !inner.contains(*open) && !inner.contains(*close))).unwrap_or(text);
    text.trim().to_string()
}

/// The versions of an answer, in the order written; an answer that names none is one version of kind `Other`.
/// Read while it streams too: the last version may be cut short.
pub fn parse(reply: &str) -> Vec<Version> {
    let mut versions: Vec<(Kind, Vec<&str>)> = vec![];
    let mut before: Vec<&str> = vec![];
    for line in reply.lines() {
        match heading(line) {
            Some((kind, rest)) => versions.push((kind, if rest.is_empty() { vec![] } else { vec![rest] })),
            None => match versions.last_mut() {
                Some((_, lines)) => lines.push(line),
                None => before.push(line),
            },
        }
    }
    if versions.is_empty() {
        let text = cleaned(&before);
        return if text.is_empty() { vec![] } else { vec![Version { kind: Kind::Other, text }] };
    }
    let mut out: Vec<Version> = vec![];
    for (kind, lines) in versions {
        let text = cleaned(&lines);
        if text.is_empty() { continue }
        // A kind named twice (a model's note after its versions, headed again) keeps the first.
        if out.iter().any(|version| version.kind == kind) { continue }
        out.push(Version { kind, text });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_versions_however_their_headings_are_written() {
        let plain = "【贴近原文】\nRoot hairs become shorter in compacted soil.\n\n【学术惯用】\nCompacted soil shortens root hairs.\n【更简洁】\nRoot hairs shorten in compacted soil.";
        let marked = "### 1. **贴近原文**\n\nRoot hairs become shorter in compacted soil.\n\n### 2. **学术惯用**\n\n\"Compacted soil shortens root hairs.\"\n\n### 3. **更简洁**\n\nRoot hairs shorten in compacted soil.\n";
        let inline = "贴近原文：Root hairs become shorter in compacted soil.\n学术惯用: Compacted soil shortens root hairs.\n**更简洁**：Root hairs shorten in compacted soil.";
        let english = "**Close to the original**\nRoot hairs become shorter in compacted soil.\n\n**Idiomatic (academic)**: Compacted soil shortens root hairs.\n\n- Concise —\n> Root hairs shorten in compacted soil.";
        let wanted = vec![
            Version { kind: Kind::Close, text: "Root hairs become shorter in compacted soil.".into() },
            Version { kind: Kind::Idiomatic, text: "Compacted soil shortens root hairs.".into() },
            Version { kind: Kind::Concise, text: "Root hairs shorten in compacted soil.".into() },
        ];
        for reply in [plain, marked, inline, english] { assert_eq!(parse(reply), wanted, "{reply}") }
    }

    #[test]
    fn fewer_versions_or_none_named() {
        // Two came back: those two.
        let two = parse("【贴近原文】\n根毛在紧实土壤中变短。\n【更简洁】\n");
        assert_eq!(two, vec![Version { kind: Kind::Close, text: "根毛在紧实土壤中变短。".into() }]);
        // No headings: the whole answer is one version.
        assert_eq!(parse("“根毛在紧实土壤中变短。”\n"), vec![Version { kind: Kind::Other, text: "根毛在紧实土壤中变短。".into() }]);
        assert!(parse("  \n").is_empty());
        // A word that begins like a heading is text; what came before the first heading is not a version.
        assert_eq!(parse("好的，以下是三个版本：\n【贴近原文】\nClosely related roots grow.")[0].text, "Closely related roots grow.");
        assert_eq!(parse("Closely related roots grow.")[0].kind, Kind::Other);
        // Still streaming: the last one is what has come.
        assert_eq!(parse("【贴近原文】\nA.\n【学术惯用】\nCompacted so").last().unwrap().text, "Compacted so");
        // A kind named again keeps the first.
        assert_eq!(parse("【贴近原文】\nA.\n【贴近原文】\nB.").len(), 1);
    }

    #[test]
    fn a_request_carries_what_it_is_to_follow() {
        let ask = Ask { source: "中文".into(), context: "根毛在紧实土壤中变短。这说明微环境很重要。".into(), terms: vec![("微环境".into(), "niche".into())], requirement: "American spelling".into(), once: "用被动语态".into(), single: false };
        let message = translate_message("中文", "英文", "这说明微环境很重要。", &ask);
        for part in ["译成英文", "【贴近原文】紧跟", "【更简洁】", "- 微环境 → niche", "这份稿件的要求：American spelling", "这一次的要求（三个版本都照这个来）：用被动语态", "上下文（只用来理解意思，不要处理它）：\n根毛在紧实土壤中变短。", "要处理的文字：\n这说明微环境很重要。"] {
            assert!(message.contains(part), "{part}\n{message}");
        }
        let bare = reword_message("英文", "Roots grow.", &Ask { context: "Roots grow.".into(), ..Default::default() });
        assert!(bare.contains("仍用英文") && bare.contains("【贴近原文】只换") && !bare.contains("上下文") && !bare.contains("要求"));
        assert!(system().contains("【学术惯用】\n……"));
        // A model of a comparison is asked for one version, with all the rest the same.
        let single = Ask { single: true, ..ask.clone() };
        let one = translate_message("中文", "英文", "这说明微环境很重要。", &single);
        assert!(!one.contains("三个版本") && !one.contains("【") && one.contains("- 微环境 → niche") && one.contains("用被动语态") && one.ends_with("这说明微环境很重要。"));
        assert!(single_system().contains("不给几个备选"));
        // The paragraph around a sentence; around a whole paragraph, the ones beside it.
        let text = "第一段。\n\n根毛变短。这说明微环境很重要。\n\n第三段。";
        let at = text.find("这说明").unwrap();
        assert_eq!(context(text, at..text.find("第三段").unwrap() - 2), "根毛变短。这说明微环境很重要。");
        let whole = text.find("根毛").unwrap()..text.find("第三段").unwrap() - 2;
        assert_eq!(context(text, whole), "第一段。\n\n第三段。");
        assert_eq!(context("只有一段。", 0..15), "");
    }
}
