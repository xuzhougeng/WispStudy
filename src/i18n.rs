//! Desktop UI translations. Chinese source text is the lookup key; unknown
//! keys fall back unchanged. Paper content and model prompts are not looked up.

use std::collections::HashMap;
use std::sync::{LazyLock, atomic::{AtomicBool, Ordering}};
use wisp_core::settings::Language;

static ENGLISH: AtomicBool = AtomicBool::new(false);
static EN: LazyLock<HashMap<String, String>> = LazyLock::new(|| serde_json::from_str(include_str!("../locales/en.json")).expect("valid English UI catalog"));

pub fn set_language(language: Language) {
    ENGLISH.store(language == Language::En, Ordering::Relaxed);
}

pub fn is_english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

pub fn translate(language: Language, text: &str) -> &str {
    if language == Language::En { EN.get(text).map_or(text, String::as_str) } else { text }
}

pub fn t(text: &str) -> &str {
    translate(if is_english() { Language::En } else { Language::Zh }, text)
}

/// A context key for source words with different meanings, such as Off / Close.
pub fn tc<'a>(key: &str, text: &'a str) -> &'a str {
    if is_english() { EN.get(key).map_or_else(|| t(text), String::as_str) } else { text }
}

/// Stored keys are language independent; translate only a known label.
pub fn label<'a>(table: &'a [(&'a str, &'a str)], value: &'a str) -> &'a str {
    table.iter().find(|(key, _)| *key == value).map_or(value, |(_, text)| t(text))
}

/// `Paper::short_authors` in the interface's language: core says "Zhang 等" for three authors or more.
pub fn short_authors(paper: &wisp_core::model::Paper) -> String {
    let short = paper.short_authors();
    match short.strip_suffix(" 等") {
        Some(first) if is_english() && paper.authors.split(';').filter(|name| !name.trim().is_empty()).count() > 2 => format!("{first} et al."),
        _ => short,
    }
}

/// Sentences core makes around a value and keeps or returns in Chinese (what a tool was called with and what came
/// back, as a conversation keeps them; what a call would change; why a file was not fetched or gave no text), with
/// their English: they are recognized where they are drawn. In the Chinese `{}` stands for any words and `{n}` for a
/// number. The English names the values by their place: `{0}` as it is, `{0:t}` looked up in turn, `{0|page|pages}`
/// the word for one or for several. Of two patterns one text fits, the first is taken: the more exact goes first.
const PHRASES: &[(&str, &str)] = &[
    // What a tool was called with (`tools::describe_call`).
    ("第 {n} 页起", "from page {0}"),
    ("第 {n} 行起", "from row {0}"),
    ("最多 {n} 行", "{0} {0|row|rows} at most"),
    ("全文 {n} 字", "{0} {0|character|characters} in all"),
    ("全文 {n} 词", "{0} {0|word|words} in all"),
    ("{n} 篇", "{0} {0|paper|papers}"),
    ("{n} 处", "{0} {0|edit|edits}"),
    ("{n} 条", "{0} {0|comment|comments}"),
    ("{n} 节", "{0} {0|section|sections}"),
    // What came back (`tools::describe_result`).
    ("{n} 条结果", "{0} {0|result|results}"),
    ("{}：{n} 个网页", "{0}: {1} {1|web page|web pages}"),
    ("改了 {n} 篇，{n} 篇不用改", "Changed {0}, {1} already as asked"),
    ("改了 {n} 篇", "Changed {0} {0|paper|papers}"),
    ("在「{}」留了 {n} 条批注，{n} 条的原文没有找到", "Left {1} {1|comment|comments} in “{0}”; the words of {2} were not found"),
    ("在「{}」留了 {n} 条批注", "Left {1} {1|comment|comments} in “{0}”"),
    ("「{}」的 {n} 处没有对上原文，没有改", "{1} {1|edit|edits} to “{0}” did not match its text; nothing changed"),
    ("已写入「{}」，另有 {n} 处没有对上原文", "Wrote “{0}”; {1} more {1|edit|edits} did not match its text"),
    ("已写入「{}」，{n} 句和所引文献的原文大段相同", "Wrote “{0}”; {1} {1|sentence repeats|sentences repeat} the paper cited"),
    ("已把「{}」整节换成新写的：{n} 字 → {n} 字", "Replaced the whole of “{0}”: {1} → {2} {2|character|characters}"),
    ("已把「{}」整节换成新写的：{n} 词 → {n} 词", "Replaced the whole of “{0}”: {1} → {2} {2|word|words}"),
    ("已写入「{}」", "Wrote “{0}”"),
    ("「{}」不用改", "“{0}” needs no change"),
    ("「{}」已经是稿件《{}》", "“{0}” is already the manuscript “{1}”"),
    ("已导入「{}」：{n} 节，{n} 张图", "Imported “{0}”: {1} {1|section|sections}, {2} {2|picture|pictures}"),
    ("已导入「{}」：{n} 节", "Imported “{0}”: {1} {1|section|sections}"),
    ("大纲现有 {n} 节", "The outline now has {0} {0|section|sections}"),
    ("{n} 份稿件", "{0} {0|manuscript|manuscripts}"),
    ("读了 {n} 节", "Read {0} {0|section|sections}"),
    ("读出了 {n} 个文件的文字，{n} 个没有读成", "Read the text of {0} {0|file|files}; {1} could not be read"),
    ("读出了 {n} 个文件的文字", "Read the text of {0} {0|file|files}"),
    ("读了 {n} 页", "Read {0} {0|page|pages}"),
    ("读了 {n} 行，共 {n} 行", "Read {0} of {1} {1|row|rows}"),
    ("找到 {n} 行，读了 {n} 行", "Found {0} {0|row|rows}, read {1}"),
    ("找到 {n} 行", "Found {0} {0|row|rows}"),
    ("{n} 句", "{0} {0|sentence|sentences}"),
    // What a call would change, for the card that asks (`tools::preview_call`).
    ("{}（最多取最新的 {n} 条）", "{0} (the latest {1} at most)"),
    ("{}（取回全部命中的记录）", "{0} (every one of its hits)"),
    ("《{}》的「{}」：整节 {n} 字换成新写的 {n} 字", "“{1}” of “{0}”: the whole section, {2} {2|character|characters}, replaced by {3} newly written"),
    ("《{}》的「{}」：整节 {n} 词换成新写的 {n} 词", "“{1}” of “{0}”: the whole section, {2} {2|word|words}, replaced by {3} newly written"),
    ("《{}》的「{}」：整节写成新的 {n} 字", "“{1}” of “{0}”: the whole section written anew, {2} {2|character|characters}"),
    ("《{}》的「{}」：整节写成新的 {n} 词", "“{1}” of “{0}”: the whole section written anew, {2} {2|word|words}"),
    ("《{}》的「{}」：改 {n} 处", "“{1}” of “{0}”: {2} {2|edit|edits}"),
    ("《{}》的大纲改为：{}", "The outline of “{0}” becomes: {1}"),
    ("《{}》的题目改为：{}", "The title of “{0}” becomes: {1}"),
    ("《{}》的正文语言改为：{}", "The language of “{0}” becomes: {1:t}"),
    ("「{}」已经是写作页的稿件《{}》，不会再导入一份。", "“{0}” is already the manuscript “{1}” on the Writing page; it will not be imported again."),
    ("把「{}」导入写作页，成为一份新稿件；原文件不会改动。", "Import “{0}” to the Writing page as a new manuscript; the file itself is not changed."),
    ("{n} 节：{}", "{0} {0|section|sections}: {1}"),
    ("文件里的 {n} 条批注一起带进来。", "The file’s {0} {0|comment|comments} come with it."),
    ("{}留在 Word 文件里，导出时还在原处。", "{0:t} stay in the Word file and are in place again on export."),
    ("文件旁边的 {n} 张图一起收进图片库，稿件里还在原处。", "The {0} {0|picture|pictures} beside the file {0|comes|come} into Figures and {0|stays|stay} in place in the manuscript."),
    ("{n} 张图没有带进来（{}）：{}", "{0} {0|picture|pictures} not brought in ({1:t}): {2}"),
    ("{n} 个表格、{n} 张图", "{0} {0|table|tables} and {1} {1|picture|pictures}"),
    ("{n} 个表格", "{0} {0|table|tables}"),
    ("{n} 张图", "{0} {0|picture|pictures}"),
    ("Notion 父页面：{}", "Notion parent page: {0}"),
    ("本地文件：{}", "Local file: {0}"),
    ("标题：{}", "Title: {0}"),
    ("正文 {n} 字，{n} 张图片；原始笔记不变。", "{0} {0|character|characters} of text, {1} {1|picture|pictures}; the note itself is not changed."),
    ("仅在已手动核对 Notion 后确认：{}。", "Confirm only after checking Notion yourself: {0:t}."),
    ("记录：{}；页面：{}", "Record: {0}; page: {1}"),
    ("记录：{}；页面：", "Record: {0}; page: none"),
    ("《{}》", "“{0}”"),
    // Why a request failed (`wisp_net`) and why a full text was not fetched (`tools::find_pdf`, `openaccess`).
    ("网络请求失败：{}", "The request failed: {0}"),
    ("连接中断：{}", "The connection broke: {0}"),
    ("无法解析模型服务的返回内容：{}", "What the model service returned could not be read: {0}"),
    // What 设置 › AI › 模型's 连接测试 says of an address where no model answers (`wisp_net::test_model`).
    ("服务有回应，但不是模型的回答，请检查服务地址和接口。收到的是：{}", "The service answered, but not as a model does: check the address and the interface. What came back: {0}"),
    ("{} 返回 HTTP {n}", "{0} returned HTTP {1}"),
    ("{} 限制了请求频率，请稍后再试", "{0} is limiting requests; try again later"),
    ("{} 迟迟没有答完，没有再等", "{0} took too long to answer; gave up waiting"),
    ("无法读取 {} 的返回内容：{}", "Could not read what {0} returned: {1}"),
    ("代理地址 {} 无法使用：{}", "The proxy address {0} cannot be used: {1}"),
    // What is wrong with a proxy address as typed (`settings::proxy_problem`).
    ("不支持 {}:// 代理。可用：{}。", "{0}:// proxies are not supported. Use one of: {1}."),
    ("没有找到这篇文献：{}", "Paper not found: {0}"),
    // Why a file gave no text (`fulltext`), and what the tools say of a paper without one.
    ("无法读取文件：{}", "The file could not be read: {0}"),
    // Pages drawn as pictures and read by the system (`wisp_net::pages`, `wisp_net::sight`).
    ("无法解析这个 PDF（{}）。", "Cannot parse this PDF ({0})."),
    ("页面图像写不出来：{}", "Cannot write page image: {0}"),
    ("系统文字识别没有读成：{}", "The system's text recognition could not read it: {0}"),
    ("不支持提取 .{} 文件的文字。", "Extracting text from .{0} files is not supported."),
    ("「{}」没有保存全文，只有书目信息和摘要。", "“{0}” has no full text stored, only its record and abstract."),
    ("这篇文献的全文只有 {n} 页。", "This paper’s full text has only {0} {0|page|pages}."),
    // Why an engine did not read a file's pages (`ocr`).
    ("{}（HTTP {n}）：{}", "{0:t} (HTTP {1}): {2}"),
    ("{}（HTTP {n}）", "{0:t} (HTTP {1})"),
    ("{} 需要 API Key：在「设置 › AI › 模型」的 DeepSeek 服务里填写，或设置环境变量 {}（在 {} 申请）。", "{0:t} needs an API key: enter it for the DeepSeek service under Settings › AI › Models, or set the environment variable {1} (get one at {2})."),
    ("{} 最多接受 {n} MB 的文件，这个文件有 {n} MB：先用「压缩 PDF」把它变小。", "{0:t} takes files of up to {1} MB and this one has {2} MB: make it smaller with “Compress PDF” first."),
    ("{} 返回的不是 JSON。", "What {0:t} returned is not JSON."),
    ("{} 的返回内容无法识别。", "What {0:t} returned could not be made sense of."),
    // What a record found by title or number has in common with the paper (`complete`).
    ("{} 一致", "Same {0}"),
    ("题目有 {n}% 的词相同", "{0}% of the title’s words match"),
    ("年份 {n}", "Year {0}"),
    ("年份是 {n}，不是 {n}", "Year is {0}, not {1}"),
    ("作者里有 {}", "Authors include {0}"),
    ("作者里没有 {}", "Authors do not include {0}"),
    // Saving a search (`Store::save_search`, `sources`).
    ("不支持的来源：{}", "Unsupported source: {0}"),
    ("{} 每次取回的篇数应在 1 到 {n} 之间。", "{0}: the number of papers per run must be between 1 and {1}."),
    // Why a search brought fewer than it had hits (`sources::pubmed_ids`, the arXiv page past its last).
    ("PubMed 命中 {n} 篇，只取得其中 {n} 篇的编号", "PubMed has {0} results; ids came for only {1} of them"),
    ("{} 这一天就有 {n} 篇，PubMed 只给出其中 {n} 篇", "{0} alone has {1} records, of which PubMed names only {2}"),
    ("arXiv 没有给出后面的记录：{}", "arXiv gave no further records: {0}"),
    ("检索式不存在：{}", "No such saved search: {0}"),
    // Why a Zotero attachment was not taken in (`zotero::attachment_path`).
    ("找不到 PDF，请先在 Zotero 中下载或修复路径：{}", "PDF not found; download it in Zotero or fix its path first: {0}"),
    ("附件没有可用的本地路径，请先在 Zotero 中下载：{}", "The attachment has no local file; download it in Zotero first: {0}"),
    ("相对链接附件需要 Zotero 的链接附件基础目录，请在 Zotero 中改用绝对路径或在 Wisp Study 中手动添加：{}", "A relative linked attachment needs Zotero’s base directory for linked files; use an absolute path in Zotero, or add the file in Wisp Study: {0}"),
    ("附件路径超出 Zotero storage 目录：{}", "The attachment path leads outside Zotero’s storage folder: {0}"),
    ("附件路径无效：{}", "Invalid attachment path: {0}"),
    // Where a paper came from (`Paper::source`), as an import of a file names it (`tools::import_references`).
    ("导入 · {}", "Import · {0}"),
    // Why a skill was not brought in, shown, taken out or put back (`skill`).
    ("{} 里没有 SKILL.md。", "{0} holds no SKILL.md."),
    ("已经有名为 {} 的技能文件夹。", "There is already a skill folder named {0}."),
    ("无法保存技能：{}", "The skill could not be saved: {0}"),
    ("无法读取 {}：{}", "{0} could not be read: {1}"),
    ("{} 不在这项技能的文件夹里。", "{0} is not in this skill’s folder."),
    ("无法移除技能：{}", "The skill could not be removed: {0}"),
    ("无法恢复技能：{}", "The skill could not be put back: {0}"),
    // Why a paper could not be moved or copied into another library (`transfer`).
    ("「{}」在这个文献库的回收站里：先把它恢复，再移过来。", "“{0}” is in this library’s trash: restore it first, then move it."),
    ("「{}」在这个文献库的回收站里：先把它恢复，再复制过来。", "“{0}” is in this library’s trash: restore it first, then copy it."),
    ("无法复制文件「{}」：{}", "Could not copy file “{0}”: {1}"),
    // A file put in another's place, and its note (`Store::swap_file`, `Store::set_file_note`).
    ("这篇文献已经有这个文件：{}。", "This paper already has this file: {0}."),
    ("备注不能超过 {} 个字。", "A note can be at most {0} characters."),
];

/// The values `text` gives the holes of `pattern`, when it is that pattern filled in.
fn holes<'a>(mut pattern: &str, mut text: &'a str) -> Option<Vec<&'a str>> {
    let mut found = vec![];
    loop {
        let (at, number) = match (pattern.find("{}"), pattern.find("{n}")) {
            (Some(any), Some(number)) => if number < any { (number, true) } else { (any, false) },
            (Some(any), None) => (any, false),
            (None, Some(number)) => (number, true),
            (None, None) => return (text == pattern).then_some(found),
        };
        text = text.strip_prefix(&pattern[..at])?;
        pattern = &pattern[at + if number { 3 } else { 2 }..];
        // A value ends where the words after its hole begin; after the last hole, where they end the text.
        let next = pattern.split('{').next().unwrap_or("");
        let end = if next.len() == pattern.len() { text.strip_suffix(next)?.len() } else { text.find(next)? };
        let value = &text[..end];
        if value.is_empty() || (number && !value.bytes().all(|b| b.is_ascii_digit())) { return None }
        found.push(value);
        text = &text[end..];
    }
}

/// An English pattern with its values put in.
fn filled(english: &str, values: &[&str]) -> String {
    let mut out = String::new();
    let mut rest = english;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').map_or(rest.len(), |n| open + n);
        let mut words = rest[open + 1..close].split('|');
        let (place, looked_up) = words.next().map_or(("", false), |place| place.strip_suffix(":t").map_or((place, false), |place| (place, true)));
        let value = place.parse::<usize>().ok().and_then(|n| values.get(n)).copied().unwrap_or("");
        match (words.next(), words.next()) {
            (Some(one), Some(several)) => out.push_str(if value == "1" { one } else { several }),
            _ if looked_up => out.push_str(&english_of(value).unwrap_or_else(|| value.to_string())),
            _ => out.push_str(value),
        }
        rest = rest.get(close + 1..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// A sentence of core in English: one of the catalog, or one of `PHRASES` filled in. None for any other text.
fn english_of(text: &str) -> Option<String> {
    if let Some(known) = EN.get(text) { return Some(known.clone()) }
    PHRASES.iter().find_map(|(pattern, english)| holes(pattern, text).map(|values| filled(english, &values)))
}

/// Several reasons core gave as one sentence (`sources::reasons`: 「Springer：link.springer.com 返回 HTTP 403；…。」),
/// each in English: who says it stays, and a reason not known stays as it is.
fn english_reasons(text: &str) -> String {
    if let Some(whole) = EN.get(text) { return whole.clone() }
    let one = |part: &str| -> Option<String> {
        // Quoted among others, a sentence has lost its full stop.
        let stopped = format!("{part}。");
        english_of(part).or_else(|| english_of(&stopped).map(|said| said.trim_end_matches('.').to_string()))
    };
    let (body, stop) = text.strip_suffix('。').map_or((text, ""), |body| (body, "."));
    let mut known = false;
    let parts: Vec<String> = body.split('；').map(|part| {
        // Who says it comes first: 「Springer：link.springer.com 返回 HTTP 403」 is Springer's, not a host of that name.
        let said = part.split_once('：').and_then(|(who, what)| one(what).map(|what| format!("{}: {what}", translate(Language::En, who)))).or_else(|| one(part));
        known |= said.is_some();
        said.unwrap_or_else(|| part.to_string())
    }).collect();
    if known { format!("{}{stop}", parts.join("; ")) } else { text.to_string() }
}

/// A tool's call as a conversation keeps it (`tools::describe_call`: 「阅读全文：P-1 · 第 3 页起」) in English.
fn english_call(line: &str) -> String {
    match line.split_once('：') {
        Some((title, detail)) => format!("{}: {}", translate(Language::En, title), detail.split(" · ").map(|part| english_of(part).unwrap_or_else(|| part.to_string())).collect::<Vec<_>>().join(" · ")),
        None => translate(Language::En, line).to_string(),
    }
}

/// A sentence core made, in the interface's language: one of the catalog or of `PHRASES`; any other stays as it is.
pub fn phrase(text: &str) -> String {
    if is_english() { english_of(text).unwrap_or_else(|| text.to_string()) } else { text.to_string() }
}

/// Why something was not done, as core says it, several reasons in one sentence: each in the interface's language.
pub fn reasons(text: &str) -> String {
    if is_english() { english_reasons(text) } else { text.to_string() }
}

/// What a tool was called with, as a conversation keeps it, in the interface's language.
pub fn tool_call(line: &str) -> String {
    if is_english() { english_call(line) } else { line.to_string() }
}

/// The tool a kept call names: what stands before the colon of 「检索全文：chromium」.
pub fn tool_title(line: &str) -> &str {
    t(line.split('：').next().unwrap_or(line))
}

/// Why a file gave no text, in the interface's language. The sentence kept with the file is Chinese, and that of an
/// earlier build is worded otherwise: its kind (`fulltext::failure_kind`) says which of today's sentences it is.
pub fn text_failure(message: &str) -> String {
    use wisp_core::fulltext;
    if !is_english() { return message.to_string() }
    english_of(message).unwrap_or_else(|| match fulltext::failure_kind(message) {
        "locked" => t(fulltext::LOCKED).to_string(),
        "encoding" => t(fulltext::NO_ENCODING).to_string(),
        "no_text" => t(fulltext::NO_TEXT_LAYER).to_string(),
        // Not a sentence about the file: what an engine or a service said of reading it.
        _ => english_reasons(message),
    })
}

/// The name an import is kept under (`tools::run_search`: 「PubMed：root[Title]」), its colon in the interface's language.
pub fn import_label(label: &str) -> String {
    match label.split_once('：') {
        Some((source, query)) if is_english() && wisp_core::sources::KINDS.iter().any(|(_, name)| *name == source) => format!("{source}: {query}"),
        _ => label.to_string(),
    }
}

/// A count with its English word: `1 paper`, `3 papers`.
pub fn count(n: impl ToString, one: &str) -> String {
    let n = n.to_string();
    if n == "1" { format!("1 {one}") } else { format!("{n} {one}s") }
}

/// Authors as a tool's answer names them (`Paper::short_authors`, 「Zhang 等」), in the interface's language.
pub fn short_names(authors: &str) -> String {
    match authors.strip_suffix(" 等") {
        Some(first) if is_english() && !first.contains(';') => format!("{first} et al."),
        _ => authors.to_string(),
    }
}

pub fn manuscript_title(manuscript: &wisp_core::manuscript::Manuscript) -> &str {
    if manuscript.title.trim().is_empty() { t(wisp_core::manuscript::UNTITLED) } else { manuscript.shown_title() }
}

pub fn assist_title(assist: &wisp_core::ai::Assist, language: &str) -> String {
    use wisp_core::ai::Assist;
    match assist {
        Assist::Rewrite => t("仿写").into(),
        Assist::Polish => t("润色").into(),
        Assist::Cite => t("配引用").into(),
        Assist::Translate if is_english() => format!("Translate to {}", t(&wisp_core::ai::language_name(language))),
        _ => assist.title(language),
    }
}

/// Both format literals are checked by Rust, including named captures and
/// precision specifiers. Values are formatted only by the selected branch.
macro_rules! tf {
    ($zh:literal, $en:literal $(, $args:expr)* $(,)?) => {
        if crate::i18n::is_english() { format!($en $(, $args)*) } else { format!($zh $(, $args)*) }
    };
}

/// `tf!` for a message that counts things. `{n}` is the number in the Chinese; in the English it is the number with
/// the word given after it, which agrees with it: `1 change`, `2 changes`.
macro_rules! tn {
    ($zh:literal, $en:literal, $n:expr, $one:literal $(, $args:expr)* $(,)?) => {
        if crate::i18n::is_english() { format!($en $(, $args)*, n = crate::i18n::count($n, $one)) } else { format!($zh $(, $args)*, n = $n) }
    };
}

#[cfg(test)]
mod tests {
    use super::{english_call, english_of, english_reasons, translate, Language};
    use serde_json::json;
    use wisp_core::tools::{describe_call, describe_result};

    fn chinese(text: &str) -> bool {
        text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c) || "：；，。「」《》（）".contains(c))
    }

    #[test]
    fn language_lookup_preserves_unknown_and_user_content() {
        assert_eq!(translate(Language::En, "设置"), "Settings");
        assert_eq!(translate(Language::Zh, "设置"), "设置");
        let title = "我的研究：A study of roots";
        assert_eq!(translate(Language::En, title), title);
        assert_eq!(translate(Language::Zh, "Unknown label"), "Unknown label");
        assert_eq!(translate(Language::En, ""), "");
    }

    #[test]
    fn shared_labels_have_english_translations() {
        use wisp_core::{ai, model, settings::Task, structure};
        for table in [&model::SCREENINGS[..], &model::READINGS, &model::FULLTEXTS, &model::PUBLICATION_STATUSES, &structure::SECTIONS, &structure::LANGUAGES, &ai::EFFORTS, &wisp_core::excerpt::KINDS, &wisp_core::screening_record::KINDS] {
            for (_, text) in table { assert_ne!(translate(Language::En, text), *text, "missing translation: {text}") }
        }
        for task in Task::ALL { assert_ne!(translate(Language::En, task.label()), task.label()) }
        for shortcut in &crate::shortcuts::SHORTCUTS { assert_ne!(translate(Language::En, shortcut.label), shortcut.label) }
    }

    #[test]
    fn every_tool_is_named_in_english() {
        let missing: Vec<&str> = wisp_core::tools::catalog().iter().map(|tool| tool.title).filter(|title| translate(Language::En, title) == *title).collect();
        assert!(missing.is_empty(), "tools without an English name in locales/en.json: {missing:?}");
    }

    #[test]
    fn a_tools_call_and_what_came_back_are_read_in_english() {
        assert_eq!(english_call("阅读全文：P-1 · 第 3 页起"), "Read full text: P-1 · from page 3");
        assert_eq!(english_call("读取表格：FLC · R-1 · 第 51 行起 · 最多 20 行"), "Read table: FLC · R-1 · from row 51 · 20 rows at most");
        assert_eq!((english_of("读了 50 行，共 302 行").as_deref(), english_of("找到 12 行，读了 5 行").as_deref(), english_of("找到 1 行").as_deref(), english_of("没有找到这样的行").as_deref()), (Some("Read 50 of 302 rows"), Some("Found 12 rows, read 5"), Some("Found 1 row"), Some("No such rows")));
        assert_eq!(english_call(&describe_call("search_fulltext", &json!({"query": "10x  Genomics", "limit": 5}))), "Search full text: 10x Genomics · limit 5");
        assert_eq!(english_call(&describe_call("library_status", &json!({}))), "Library overview");
        assert_eq!(english_call(&describe_call("write_section", &json!({"section": "Methods", "text": "Roots adapt to soil."}))), "Write manuscript: Methods · 4 words in all");
        assert_eq!(english_call(&describe_call("write_section", &json!({"section": "Methods", "text": "根尖"}))), "Write manuscript: Methods · 2 characters in all");
        assert_eq!(english_call(&describe_call("edit_section", &json!({"section": "Methods", "edits": [{"old": "a", "new": "b"}]}))), "Edit manuscript: Methods · 1 edit");
        assert_eq!(english_call(&describe_call("organise_papers", &json!({"changes": [{"id": "a"}, {"id": "b"}]}))), "Organise papers: 2 papers");
        // A connector's tool and a skill are named as they are.
        assert_eq!((english_call("Notion · search：roots"), english_call("使用技能：review")), ("Notion · search: roots".to_string(), "Using skill: review".to_string()));
        // Every shape of `describe_result`, with names that are not Chinese: no Chinese is left.
        let section = json!({"name": "Methods"});
        let results = [
            json!({"total": 3}), json!({"total": 1}), json!({"engine": "DeepSeek", "results": [1]}), json!({"changed": 2, "unchanged": 1}), json!({"changed": 1, "unchanged": 0}),
            json!({"commented": 2, "missed": [], "section": section}), json!({"commented": 2, "missed": ["x"], "section": section}),
            json!({"edited": 0, "missed": [], "section": section}), json!({"edited": 1, "missed": [], "section": section}), json!({"edited": 0, "missed": ["x"], "section": section}), json!({"edited": 1, "missed": ["x"], "section": section}), json!({"edited": 1, "missed": [], "copied": [1, 2], "section": section}),
            json!({"file": "Roots.docx", "manuscript": {"title": "Roots", "sections": [1, 2]}}), json!({"file": "Roots.docx", "already": true, "manuscript": {"title": "Roots", "sections": [1]}}),
            json!({"sections": [1, 2, 3], "added": []}), json!({"manuscripts": [1]}), json!({"sections": [1]}), json!({"files": [{"outcome": "read"}, {"outcome": "failed"}], "remaining": 0}), json!({"files": [{"outcome": "read"}], "remaining": 0}),
            json!({"pages": [1, 2]}), json!({"sentences": [1]}), json!({}),
        ];
        for result in results {
            let said = describe_result(&Ok(result));
            let english = english_of(&said).unwrap_or_else(|| panic!("no English for {said}"));
            assert!(!chinese(&english), "{said} => {english}");
        }
        assert_eq!((english_of("3 条结果").as_deref(), english_of("1 条结果").as_deref(), english_of("读了 2 页").as_deref()), (Some("3 results"), Some("1 result"), Some("Read 2 pages")));
        assert_eq!(english_of("在「Methods」留了 2 条批注，1 条的原文没有找到").as_deref(), Some("Left 2 comments in “Methods”; the words of 1 were not found"));
        assert_eq!(english_of("已写入「Methods」，1 句和所引文献的原文大段相同").as_deref(), Some("Wrote “Methods”; 1 sentence repeats the paper cited"));
        // What a person wrote is no sentence of core's.
        assert_eq!((english_of("我的研究：A study of roots"), english_of("3 条意见"), english_of("")), (None, None, None));
    }

    #[test]
    fn what_a_call_would_change_is_read_in_english() {
        assert_eq!(english_of("root[Title]（最多取最新的 600 条）").as_deref(), Some("root[Title] (the latest 600 at most)"));
        assert_eq!(english_of("《Crossref》").as_deref(), Some("“Crossref”"));
        assert_eq!(english_of("《Roots》的「Methods」：改 2 处").as_deref(), Some("“Methods” of “Roots”: 2 edits"));
        assert_eq!(english_of("《Roots》的「Methods」：整节 7409 字换成新写的 15 字").as_deref(), Some("“Methods” of “Roots”: the whole section, 7409 characters, replaced by 15 newly written"));
        assert_eq!(english_of("已把「Methods」整节换成新写的：7409 字 → 15 字").as_deref(), Some("Replaced the whole of “Methods”: 7409 → 15 characters"));
        assert_eq!(english_of("《Roots》的「Methods」：整节 900 词换成新写的 1 词").as_deref(), Some("“Methods” of “Roots”: the whole section, 900 words, replaced by 1 newly written"));
        assert_eq!(english_of("《Roots》的大纲改为：A、B").as_deref(), Some("The outline of “Roots” becomes: A、B"));
        assert_eq!(english_of("《Roots》的正文语言改为：英文").as_deref(), Some("The language of “Roots” becomes: English"));
        assert_eq!(english_of("1 个表格、2 张图留在 Word 文件里，导出时还在原处。").as_deref(), Some("1 table and 2 pictures stay in the Word file and are in place again on export."));
        assert_eq!(english_of("文件旁边的 1 张图一起收进图片库，稿件里还在原处。").as_deref(), Some("The 1 picture beside the file comes into Figures and stays in place in the manuscript."));
        assert_eq!(english_of("2 张图没有带进来（不是 PNG、JPEG 或 WebP 图片）：figures/a.svg、b.pdf").as_deref(), Some("2 pictures not brought in (not a PNG, JPEG or WebP picture): figures/a.svg、b.pdf"));
        assert_eq!(english_of("已导入「draft.md」：3 节，2 张图").as_deref(), Some("Imported “draft.md”: 3 sections, 2 pictures"));
        assert_eq!(english_of("仅在已手动核对 Notion 后确认：笔记已成功写入。").as_deref(), Some("Confirm only after checking Notion yourself: the note was saved."));
    }

    #[test]
    fn reasons_are_read_in_english_one_by_one() {
        assert_eq!(english_reasons("Springer：link.springer.com 返回 HTTP 403；拿不到文件时，可以在浏览器里打开文献的链接下载，再把 PDF 拖进窗口。"), "Springer: link.springer.com returned HTTP 403; when the file cannot be fetched, open the paper’s link in a browser, download the PDF and drag it into the window.");
        assert_eq!(english_reasons("journal.example：返回的不是 PDF，多半是登录页或验证页；花的时间太长，没有再试下去。"), "journal.example: what came back is not a PDF, most likely a sign-in or verification page; it took too long; no further attempts were made.");
        assert_eq!(english_reasons("www.ebi.ac.uk 返回 HTTP 503。"), "www.ebi.ac.uk returned HTTP 503.");
        assert_eq!(english_reasons("请填写检索式。"), "Enter a query.");
        assert_eq!(english_reasons(&format!("api.crossref.org 限制了请求频率，请稍后再试；{}。", wisp_core::sources::GIVE_EMAIL)), "api.crossref.org is limiting requests; try again later; it allows more requests once a contact email is set under Settings › Network › Literature databases.");
        assert_eq!(english_reasons("看图模型：账户余额不足（HTTP 402）：Insufficient Balance"), "Vision model: insufficient balance (HTTP 402): Insufficient Balance");
        // What 连接测试 can say of a model (`wisp_net::test_model`).
        assert_eq!(english_reasons("地址或模型名称不存在（HTTP 404）：Model Not Exist"), "no such address or model name (HTTP 404): Model Not Exist");
        assert_eq!(english_reasons(&format!("{}收到的是：<html>Welcome</html>", wisp_net::NOT_A_MODEL)), "The service answered, but not as a model does: check the address and the interface. What came back: <html>Welcome</html>");
        assert_eq!(english_reasons(wisp_net::TEST_TIMED_OUT), "No answer within a minute; gave up waiting.");
        // A reason not known, and words that are no reason at all, stay as they are.
        assert_eq!((english_reasons("磁盘满了；www.ebi.ac.uk 返回 HTTP 503。"), english_reasons("我的研究：根；叶")), ("磁盘满了; www.ebi.ac.uk returned HTTP 503.".to_string(), "我的研究：根；叶".to_string()));
    }

    #[test]
    fn the_sentences_core_keeps_have_english() {
        use wisp_core::{fulltext, journals, podcast, structure};
        for sentence in [fulltext::LOCKED, fulltext::NO_ENCODING, fulltext::NO_TEXT_LAYER, structure::NO_BODY, journals::EDITIONS, wisp_core::sources::TIME_UP, wisp_core::sources::GIVE_EMAIL, wisp_core::sources::GIVE_NCBI_KEY, wisp_core::comment::ASSISTANT_NAME, podcast::NO_SCRIPT] { assert_ne!(translate(Language::En, sentence), sentence) }
        assert_eq!((english_of("无法读取文件：No such file").as_deref(), english_of("不支持提取 .docx 文件的文字。").as_deref()), (Some("The file could not be read: No such file"), Some("Extracting text from .docx files is not supported.")));
        // The journal table's own words: why a list warns against a journal, and the categories of the two partitions.
        let mut words: Vec<&str> = journals::table().journals.iter().flat_map(|journal| [journal.warning.as_str(), journal.cas_category.as_str(), journal.xr_category.as_str()]).filter(|word| !word.is_empty()).collect();
        words.sort();
        words.dedup();
        let missing: Vec<&str> = words.into_iter().filter(|word| chinese(translate(Language::En, word))).collect();
        assert!(missing.is_empty(), "words of journals.tsv without English in locales/en.json: {missing:?}");
        // The hosts are drawn by the names a script kept under the English interface gives them.
        for (host, (_, name, _)) in podcast::HOSTS.iter().enumerate() { assert_eq!(translate(Language::En, name), podcast::HOSTS_EN[host]) }
        // What a record found by its title has in common with the paper.
        // Why a Zotero attachment was not taken in, and where a paper came from.
        assert_eq!(english_of("找不到 PDF，请先在 Zotero 中下载或修复路径：/z/storage/AB12/a.pdf").as_deref(), Some("PDF not found; download it in Zotero or fix its path first: /z/storage/AB12/a.pdf"));
        // Why a skill was not brought in or shown, and why a paper could not move.
        assert_eq!((english_of("已经有名为 plain 的技能文件夹。").as_deref(), english_of("无法读取 references/a.md：denied").as_deref()), (Some("There is already a skill folder named plain."), Some("references/a.md could not be read: denied")));
        assert_eq!(english_of("「Root hairs」在这个文献库的回收站里：先把它恢复，再移过来。").as_deref(), Some("“Root hairs” is in this library’s trash: restore it first, then move it."));
        assert_eq!(english_of("「Root hairs」在这个文献库的回收站里：先把它恢复，再复制过来。").as_deref(), Some("“Root hairs” is in this library’s trash: restore it first, then copy it."));
        for source in ["Zotero 本地导入", "PDF 导入", "网址导入", "导入 · refs.bib"] { assert!(english_of(source).is_some_and(|english| !chinese(&english)), "{source}") }
        for note in ["题目一致", "DOI 一致", "题目有 87% 的词相同", "年份 2019", "年份是 2019，不是 2020", "作者里有 Wang", "作者里没有 Wang"] { assert!(english_of(note).is_some_and(|english| !chinese(&english)), "{note}") }
    }
}
