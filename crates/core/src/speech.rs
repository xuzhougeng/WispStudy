//! 朗读: a paper's text read aloud by DeepSeek's speech service
//! (`POST {host}/v1/tools/tts`). What is decided here: which text is read
//! (a PDF page's lines joined again, citation numbers and addresses left
//! out, the reference list not read), how it is cut into requests, the
//! request itself, and the sound that comes back (16-bit mono PCM at 24 kHz)
//! as samples. Sending is `wisp_net::speak`, playing the window's.

use crate::ai::HttpRequest;
use crate::fulltext::bare;
use crate::model::Paper;
use serde_json::json;

/// What the service sends with `format: pcm`: 16-bit little-endian mono at this rate.
pub const SAMPLE_RATE: u32 = 24_000;

/// The voices the service has, by `voice_id`, with what the settings call them. The first is its default.
pub const VOICES: [(&str, &str); 2] = [("stella", "女声"), ("tide", "男声")];

/// What one request carries at most, in the service's count (`billed`). The service takes 10,000;
/// a smaller piece brings the first sound sooner and wastes little when the reader stops.
pub const PIECE: usize = 1_500;

/// The voice to ask with: the one chosen when the service has it, else its default.
pub fn voice(chosen: &str) -> &'static str {
    VOICES.iter().find(|(key, _)| *key == chosen.trim()).map_or(VOICES[0].0, |(key, _)| key)
}

/// Whether a model service's address is DeepSeek's, the one that speaks.
pub fn is_service(base_url: &str) -> bool {
    host(base_url).ends_with("deepseek.com")
}

fn host(base_url: &str) -> String {
    base_url.split("://").nth(1).unwrap_or(base_url).split(['/', ':', '?']).next().unwrap_or("").trim().to_lowercase()
}

/// Where the sound is asked for: the speech service sits at the host's root, whatever path chat has (`/v1`, `/anthropic`).
pub fn endpoint(base_url: &str) -> String {
    let base = base_url.trim();
    let scheme = if base.starts_with("http://") { "http" } else { "https" };
    let rest = base.split("://").nth(1).unwrap_or(base);
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    format!("{scheme}://{authority}/v1/tools/tts")
}

/// The request for one piece of text, in `voice` (a key of `VOICES`).
pub fn request(base_url: &str, key: &str, proxy: &str, voice_id: &str, text: &str) -> Result<HttpRequest, String> {
    if key.trim().is_empty() { return Err("朗读要用 DeepSeek 的 API Key：请在「设置 › AI」里填写。".into()) }
    if text.trim().is_empty() { return Err("没有可朗读的文字。".into()) }
    Ok(HttpRequest {
        url: endpoint(base_url),
        headers: vec![("authorization".into(), format!("Bearer {}", key.trim())), ("content-type".into(), "application/json".into())],
        // The text is already plain: Markdown filtering would only take its asterisks and brackets.
        body: json!({"text": text, "voice_id": voice(voice_id), "format": "pcm", "filter_markdown": false}),
        proxy: proxy.trim().to_string(),
    })
}

/// A character the service counts twice: Chinese, Japanese and Korean script and full-width forms.
fn wide(c: char) -> bool {
    matches!(c, '\u{2E80}'..='\u{9FFF}' | '\u{AC00}'..='\u{D7AF}' | '\u{F900}'..='\u{FAFF}' | '\u{FF00}'..='\u{FFEF}' | '\u{20000}'..='\u{2FA1F}')
}

/// What a text counts for in a request: a Chinese character two, anything else one.
pub fn billed(text: &str) -> usize {
    text.chars().map(|c| if wide(c) { 2 } else { 1 }).sum()
}

/// The heading a reference list begins under, on a line of its own.
fn references_heading(line: &str) -> bool {
    let line = line.trim().trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c.is_whitespace()).trim_end_matches([':', '：']).trim();
    ["references", "bibliography", "literature cited", "references and notes", "参考文献", "引用文献"].iter().any(|h| line.eq_ignore_ascii_case(h))
}

/// A page's text as it is read: its lines joined again (a word hyphenated at a line's end made whole,
/// Chinese joined without a space), lines that are only a page number left out, and the bracketed
/// citation numbers (`[12]`, `[3–5, 8]`) and web addresses that only interrupt the listener taken out.
pub fn readable(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.chars().all(|c| c.is_ascii_digit())) {
        let last = out.chars().last();
        let first = line.chars().next();
        match (last, first) {
            (None, _) => {}
            // `exam-` + `ple`: one word again; a hyphen before a capital or a digit stays (`Wnt-`, `COVID-19`).
            (Some('-'), Some(next)) if next.is_lowercase() && out.chars().rev().nth(1).is_some_and(char::is_alphabetic) => { out.pop(); }
            (Some('-'), _) if out.chars().rev().nth(1).is_some_and(char::is_alphanumeric) => {}
            (Some(before), Some(next)) if wide(before) || wide(next) => {}
            _ => out.push(' '),
        }
        out.push_str(line);
    }
    let out = without_citations(&out);
    let out: Vec<&str> = out.split_whitespace().filter(|word| !(word.starts_with("http://") || word.starts_with("https://") || word.starts_with("www.") || word.to_ascii_lowercase().starts_with("doi:"))).collect();
    // A citation taken from before a full stop leaves a space there: `cells [3].` → `cells .`
    out.join(" ").replace(" .", ".").replace(" ,", ",").replace(" ;", ";")
}

/// The text without bracketed citation numbers: `[12]`, `[3, 5]`, `[3–5]`, `［1］`.
fn without_citations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(['[', '［']) {
        let open_len = rest[open..].chars().next().map_or(1, char::len_utf8);
        let inside = &rest[open + open_len..];
        let close = inside.find([']', '］']);
        let cited = close.is_some_and(|close| {
            let inner = &inside[..close];
            !inner.trim().is_empty() && inner.chars().any(|c| c.is_ascii_digit()) && inner.chars().all(|c| c.is_ascii_digit() || matches!(c, ',' | '，' | '-' | '–' | '—' | ' '))
        });
        out.push_str(&rest[..open]);
        match close {
            Some(close) if cited => {
                let close_len = inside[close..].chars().next().map_or(1, char::len_utf8);
                rest = &inside[close + close_len..];
            }
            _ => {
                out.push_str(&rest[open..open + open_len]);
                rest = inside;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Botanical authors' names a species' name ends with (`Ipomoea batatas (L.) Lam.`), and the words of
/// a name's rank (`subsp.`, `cv.`): a full stop after them ends no sentence. Beside the abbreviations
/// the sentences of the full text are cut by (`structure::ABBREVIATIONS`).
const NAME_ABBREVIATIONS: &[&str] = &[
    "lam", "thunb", "mill", "willd", "benth", "hook", "hook.f", "dc", "juss", "lindl", "pers", "sm", "schltr", "spreng", "steud", "engl", "desf", "gaertn", "roxb", "sw", "poir", "mart", "planch", "burm", "merr", "retz", "michx", "raf", "torr", "sieb", "zucc", "maxim",
    "franch", "hemsl", "rchb", "griseb", "wall", "ait", "salisb", "medik", "cav", "jacq", "nutt", "kunth", "hort", "subsp", "ssp", "cv", "nov", "syn", "auct", "emend", "viz", "resp", "approx", "dept", "univ", "jan", "feb", "mar", "apr", "jun", "jul", "aug", "sep",
    "sept", "oct", "dec", "mr", "mrs", "ms", "chr", "sect", "sec", "suppl", "chap",
];

/// Where a sentence may end: after these, and for the Latin ones only before a space. A full stop
/// before a word all in small letters ends none (`Lam. or sweetpotato`), nor one after an abbreviation
/// (`et al.`, `Fig.`, `e.g.`, `vs.`, `Lam.`) or an initial (`J.`, `O. R.`).
fn sentence_end(text: &str, at: usize, c: char) -> bool {
    match c {
        '。' | '！' | '？' | '；' => true,
        // Not a semicolon between the citations in brackets: `(Denyer et al., 2019; Ryu et al., 2019)`.
        ';' if text[..at].rfind('(').is_some_and(|open| text[open..at].find(')').is_none() && at - open < 400) => false,
        '!' | '?' | ';' => text[at + c.len_utf8()..].chars().next().is_none_or(char::is_whitespace),
        '.' => {
            let after = &text[at + 1..];
            if !after.chars().next().is_none_or(char::is_whitespace) { return false }
            // A word all in small letters goes on the sentence; one with a capital in it may open one (`scRNA-seq`, `mRNA`).
            let next = after.split_whitespace().next().unwrap_or("");
            if next.chars().next().is_some_and(char::is_lowercase) && !next.chars().any(|c| c.is_uppercase() || c.is_ascii_digit()) { return false }
            let word = text[..at].rsplit(|c: char| c.is_whitespace() || c == '(' || c == '[').next().unwrap_or("").trim_start_matches(['“', '"', '‘', '\'']).to_lowercase();
            let initial = !word.is_empty() && word.split('.').all(|part| part.chars().count() == 1 && part.chars().all(char::is_alphabetic));
            !(initial || crate::structure::ABBREVIATIONS.contains(&word.as_str()) || NAME_ABBREVIATIONS.contains(&word.as_str()))
        }
        _ => false,
    }
}

/// The text cut after each sentence's end, nothing of it left out: a piece read aloud is followed on
/// the page a sentence at a time.
pub fn sentences(text: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    for (at, c) in text.char_indices() {
        if sentence_end(text, at, c) {
            let end = at + c.len_utf8();
            out.push(&text[start..end]);
            start = end;
        }
    }
    if start < text.len() { out.push(&text[start..]) }
    out
}

/// Which sentence of `text` is heard when `heard` (0 to 1) of its sound has played: the sentences take
/// the time their length (`billed`) gives them. The sentence, trimmed; `None` when the text has none.
pub fn sentence_at(text: &str, heard: f32) -> Option<&str> {
    let all: Vec<&str> = sentences(text).into_iter().filter(|s| !s.trim().is_empty()).collect();
    let total: usize = all.iter().map(|s| billed(s)).sum();
    let mut passed = 0;
    for sentence in &all {
        passed += billed(sentence);
        if passed as f32 >= heard.clamp(0., 1.) * total as f32 { return Some(sentence.trim()) }
    }
    all.last().map(|s| s.trim())
}

/// The text cut into pieces of at most `budget` (`billed`), at the ends of sentences where it can be,
/// else at a comma or a space, else anywhere.
pub fn pieces(text: &str, budget: usize) -> Vec<String> {
    let budget = budget.max(16);
    let mut out: Vec<String> = vec![];
    let mut current = String::new();
    for sentence in sentences(text).into_iter().flat_map(|s| cut(s, budget)) {
        if !current.is_empty() && billed(&current) + billed(&sentence) > budget {
            out.push(current.trim().to_string());
            current.clear();
        }
        current.push_str(&sentence);
    }
    if !current.trim().is_empty() { out.push(current.trim().to_string()) }
    out.retain(|piece| !piece.is_empty());
    out
}

/// A sentence too long for one piece, cut at commas or spaces, else anywhere.
fn cut(sentence: &str, budget: usize) -> Vec<String> {
    if billed(sentence) <= budget { return vec![sentence.to_string()] }
    let mut out = vec![];
    let mut current = String::new();
    for c in sentence.chars() {
        current.push(c);
        let soft = matches!(c, ',' | '，' | '、' | ':' | '：') || c.is_whitespace();
        if billed(&current) >= budget || (soft && billed(&current) >= budget * 3 / 4) {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() { out.push(current) }
    out
}

/// What is read from page `first` (1-based) on: each page's readable text (`page_texts`) in pieces,
/// with its page. Reading stops at the reference list.
pub fn from_pages(pages: &[String], first: usize, paper: &Paper) -> Vec<(usize, String)> {
    page_texts(pages, paper).into_iter().enumerate().skip(first.max(1) - 1).flat_map(|(index, text)| pieces(&text, PIECE).into_iter().map(move |piece| (index + 1, piece))).collect()
}

/// The whole readable text of the pages, up to the reference list: what a podcast is written from.
pub fn body(pages: &[String], paper: &Paper) -> String {
    page_texts(pages, paper).into_iter().filter(|text| !text.is_empty()).collect::<Vec<_>>().join(" ")
}

/// What each page gives to be read, up to the reference list (the pages after it give nothing and are
/// left out). Of every page: the lines that come back on page after page (running heads and feet,
/// `structure::furniture`) and its number or count (`Page 2 of 17`) are left out, a word set letter by
/// letter (`R E S E A R C H`) is no word, and a heading standing alone ends as a sentence does. Of the
/// first page only what the paper says: its title and authors (found by the record's title) first,
/// then the abstract and the body, without the small print of licence, copyright, correspondence,
/// affiliations, dates, editors, keywords and the journal's name (`front_matter`, the record's journal).
pub fn page_texts(pages: &[String], paper: &Paper) -> Vec<String> {
    let furniture = crate::structure::furniture(pages);
    let mut out = vec![];
    for (index, page) in pages.iter().enumerate() {
        let lines: Vec<&str> = page.lines().collect();
        let end = lines.iter().position(|line| references_heading(line));
        let mut blocks = page_blocks(&lines[..end.unwrap_or(lines.len())], &furniture);
        // The first page's sidebar of labelled fields may run on to the next (PLOS's `Competing interests: …`).
        blocks.retain(|block| { let lower = block[0].to_lowercase(); !FIELDS.iter().any(|field| lower.starts_with(field)) });
        if index == 0 { blocks = first_page(blocks, paper, &furniture) }
        out.push(readable(&ended_headings(blocks).concat().join("\n")));
        if end.is_some() { break }
    }
    out
}

/// A page's lines in blocks (the lines between two blank ones), without its furniture and numbers,
/// letter-spaced words and the bullets before items. A line before an item that has no end of its
/// own gets one: `Highlights` / `• This is…`.
fn page_blocks(lines: &[&str], furniture: &std::collections::HashSet<String>) -> Vec<Vec<String>> {
    let mut blocks: Vec<Vec<String>> = vec![vec![]];
    for line in lines.iter().map(|line| line.trim()) {
        if line.is_empty() {
            if blocks.last().is_some_and(|block| !block.is_empty()) { blocks.push(vec![]) }
            continue;
        }
        if page_number(line) || furniture.contains(&crate::structure::shape(line)) { continue }
        let item = line.strip_prefix(['•', '▪', '■', '◆', '●', '◦', '‣']).map(str::trim_start);
        let line = without_spaced_letters(item.unwrap_or(line));
        if line.is_empty() { continue }
        let block = blocks.last_mut().expect("a block");
        if item.is_some() && let Some(before) = block.last_mut() && !before.ends_with(['.', '!', '?', ':', ';', '。', '！', '？', '：', '；']) { before.push('.') }
        block.push(line);
    }
    // A block without a word of two letters is no text to read: a logo's glyphs (`a1111111111`), a lone mark, a table's numbers.
    blocks.retain(|block| block.iter().any(|line| line.split(|c: char| !c.is_alphabetic()).any(|word| word.chars().count() >= 2 || word.chars().any(crate::structure::is_cjk))));
    blocks
}

/// A line that only numbers its page: `7`, `Page 2 of 17`, `2 of 17`, `2 / 22`, `第 3 页`.
fn page_number(line: &str) -> bool {
    let number = |word: &str| !word.is_empty() && word.chars().all(|c| c.is_ascii_digit());
    let page = |word: &str| word.eq_ignore_ascii_case("page") || word.eq_ignore_ascii_case("p.");
    let chinese = line.strip_prefix('第').and_then(|rest| rest.strip_suffix('页')).is_some_and(|n| number(n.trim()));
    chinese || match line.split_whitespace().collect::<Vec<_>>().as_slice() {
        [n] => number(n),
        [p, n] => page(p) && number(n),
        [n, of, m] => number(n) && (of.eq_ignore_ascii_case("of") || *of == "/") && number(m),
        [p, n, of, m] => page(p) && number(n) && of.eq_ignore_ascii_case("of") && number(m),
        _ => false,
    }
}

/// A line without the words set letter by letter (`R E S E A R C H Open Access` → `Open Access`):
/// a masthead's, not text to read. Four capitals or more apart make one.
fn without_spaced_letters(line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    let single = |word: &str| word.chars().count() == 1 && word.chars().all(char::is_uppercase);
    let mut kept: Vec<&str> = vec![];
    let mut at = 0;
    while at < words.len() {
        let run = words[at..].iter().take_while(|word| single(word)).count();
        if run < 4 { kept.extend(&words[at..at + run.max(1)]) }
        at += run.max(1);
    }
    if kept.len() == words.len() { line.to_string() } else { kept.join(" ") }
}

/// The blocks with a full stop after each heading that has none (`Results`, `Sweetpotato root protoplast
/// isolation`), so it is heard and marked as a sentence of its own and does not run into the paragraph
/// it heads. Only a heading followed on its page by what starts a sentence: a few words standing alone
/// at a column's end may be a sentence cut by the column.
fn ended_headings(mut blocks: Vec<Vec<String>>) -> Vec<Vec<String>> {
    for at in 0..blocks.len().saturating_sub(1) {
        let lines: Vec<&str> = blocks[at].iter().map(String::as_str).collect();
        let text = readable(&lines.join("\n"));
        let next = blocks[at + 1][0].chars().next();
        if crate::structure::looks_like_heading(&lines, &text) && !text.contains(". ") && next.is_some_and(|c| c.is_ascii_digit() || (c.is_alphabetic() && !c.is_lowercase())) {
            blocks[at].last_mut().expect("a line").push('.');
        }
    }
    blocks
}

/// The first page in the order it is read: the title and its authors first, then what follows the
/// abstract's heading, without the front matter. In some layouts (BMC's of 2024) the title comes last
/// in the page's text, under the abstract; what stood before the title and before an `Abstract`
/// heading is a masthead's. Without a record's title found on the page, only the front matter is left out.
fn first_page(blocks: Vec<Vec<String>>, paper: &Paper, furniture: &std::collections::HashSet<String>) -> Vec<Vec<String>> {
    let text = |block: &Vec<String>| readable(&block.join("\n"));
    let texts: Vec<String> = blocks.iter().map(text).collect();
    let mut drop = vec![false; blocks.len()];
    let journal = bare(&paper.journal);
    for at in 0..blocks.len() {
        // The journal's name standing alone (`BMC Plant Biology`, `PLOS ONE`), also as the record shortens it.
        let named = bare(&texts[at]);
        if journal.chars().count() >= 6 && (named == journal || (named.starts_with(&journal) && named.len() <= journal.len() + 8)) { drop[at] = true }
        // A line of a few words found in the running head of the other pages: the journal's name as the masthead has it.
        let line = crate::structure::shape(&texts[at]);
        if blocks[at].len() == 1 && (6..=60).contains(&line.chars().count()) && crate::structure::canonical(&texts[at]).is_none() && furniture.iter().any(|head| head.contains(&line) && head.len() > line.len()) { drop[at] = true }
        match front_matter(&texts[at]) {
            Front::Not => {}
            Front::Alone => drop[at] = true,
            Front::WithNext => {
                drop[at] = true;
                // The value of a label set apart from it (`EDITED BY` / `George V. Popescu, …`), unless the paper goes on.
                if texts.get(at + 1).is_some_and(|next| crate::structure::canonical(next).is_none() && crate::structure::width(next) <= 400) { drop[at + 1] = true }
            }
        }
    }
    let title = bare(&paper.title);
    let found = (title.chars().count() >= 12).then(|| (0..blocks.len()).find(|at| !drop[*at] && same_title(&bare(&texts[*at]), &title))).flatten();
    let mut out: Vec<Vec<String>> = vec![];
    let mut skip = drop.clone();
    if let Some(at) = found {
        out.push(vec![ended(&texts[at])]);
        skip[at] = true;
        // The authors: the blocks after the title that are lists of names, their marks of affiliation left out.
        let mut names = vec![];
        for next in at + 1..blocks.len() {
            if drop[next] { continue }
            let plain = unmarked(&texts[next]);
            if !(plain.is_empty() || authors(&plain)) { break }
            if !plain.is_empty() { names.push(plain) }
            skip[next] = true;
        }
        if !names.is_empty() { out.push(vec![ended(&names.join(", "))]) }
        // Before the title only what comes from the abstract's heading on.
        let from = (0..at).find(|before| !drop[*before] && crate::structure::canonical(&texts[*before]) == Some("abstract")).unwrap_or(at);
        skip[..from].iter_mut().for_each(|s| *s = true);
    }
    // A list of names elsewhere (no title found) is read without its marks too.
    let names = |block: Vec<String>| {
        let text = readable(&block.join("\n"));
        let plain = unmarked(&text);
        if plain != text && authors(&plain) { vec![ended(&plain)] } else { block }
    };
    out.extend(blocks.into_iter().zip(skip).filter(|(_, skip)| !skip).map(|(block, _)| names(keywords_left_out(block))).filter(|block| !block.is_empty()));
    out
}

/// A block without the keywords that end it (`Keywords Ipomoea batatas (L.) Lam., scRNA-seq, …`).
fn keywords_left_out(mut block: Vec<String>) -> Vec<String> {
    if let Some(from) = block.iter().position(|line| { let lower = line.to_lowercase(); lower.starts_with("keywords") || lower.starts_with("key words") }) { block.truncate(from) }
    block
}

/// A text with a sentence's end: a title or a list of names has none of its own.
fn ended(text: &str) -> String {
    let text = text.trim();
    if text.ends_with(['.', '!', '?', '。', '！', '？']) { text.to_string() } else { format!("{text}.") }
}

/// Whether a block of the first page is the record's title, compared bare: as it is, or cut a little
/// differently (a subtitle, a word the record spells otherwise).
fn same_title(block: &str, title: &str) -> bool {
    let (b, t) = (block.chars().count(), title.chars().count());
    block == title || (block.contains(title) && b * 10 <= t * 13) || (title.contains(block) && b * 10 >= t * 8)
}

/// The labelled fields of a first page's sidebar, which no paragraph of a paper begins with.
const FIELDS: [&str; 11] = ["competing interests:", "funding:", "data availability statement:", "citation:", "editor:", "academic editor:", "copyright:", "received:", "accepted:", "published:", "*correspondence:"];

/// What a first page's block is when it is front matter: `Alone`, or a label whose value is the next block.
#[derive(PartialEq)]
enum Front { Not, Alone, WithNext }

/// The labels of a first page's small print whose value may stand in a block of its own after them.
const LABELS: [&str; 10] = ["edited by", "reviewed by", "correspondence", "*correspondence", "citation", "copyright", "specialty section", "keywords", "key words", "abbreviations"];

/// What a first page's small print begins with, on the same line as its value.
const FRONT_STARTS: [&str; 30] = [
    "type ", "received", "accepted", "published", "revised", "citation:", "editor:", "academic editor:", "handling editor", "data availability", "funding:", "competing interests:", "keywords", "key words", "doi:", "doi ", "cite this", "copyright", "article history", "edited by",
    "reviewed by", "open access", "*correspondence", "correspondence", "specialty section", "†", "‡", "§", "☯", "*",
];

/// A first page's masthead, standing alone: the kind of article and the like.
const MASTHEAD: [&str; 18] = ["open access", "research", "research open access", "research article", "article", "articles", "original research", "original article", "review", "review article", "brief report", "methodology", "methodology article", "software", "database", "short communication", "letter", "research paper"];

/// Words of a line of affiliations.
const INSTITUTIONS: [&str; 13] = ["university", "institute", "college", "department", "laboratory", "academy", "hospital", "school of", "centre", "center for", "center of", "faculty of", "key lab"];

/// Whether a block of the first page is front matter (licence, copyright, correspondence, dates, editors,
/// affiliations, notes on the authors, the kind of article): not the paper's own words. A journal's
/// name standing alone is the furniture of its other pages or the masthead's.
fn front_matter(text: &str) -> Front {
    let lower = text.to_lowercase();
    let label = lower.trim_end_matches([':', '：']).trim();
    if LABELS.contains(&label) { return Front::WithNext }
    if MASTHEAD.contains(&label) || crate::structure::is_notice(text) || crate::structure::is_button(text) || text.contains('@') || FRONT_STARTS.iter().any(|start| lower.starts_with(start)) { return Front::Alone }
    if ["contributed equally", "co-first author", "equal contribution", "unedited version of this manuscript", "this article was submitted to"].iter().any(|mark| lower.contains(mark)) { return Front::Alone }
    // Affiliations: institutions and places, no sentence of the paper's (they have no verbs).
    let spaced = format!(" {lower} ");
    let says = [" we ", " is ", " are ", " was ", " were ", " has ", " have ", " this study ", " that ", " which "].iter().any(|word| spaced.contains(word));
    if INSTITUTIONS.iter().any(|word| lower.contains(word)) && !says { return Front::Alone }
    Front::Not
}

/// A list of names: parts apart by commas, `and` or `&`, each a few words that start with capitals
/// (or a particle, `van`, `de`), with initials (`Bastiaan O. R. Bargmann`) but no sentence.
fn authors(text: &str) -> bool {
    if crate::structure::width(text) > 700 || text.contains(['@', ':']) || crate::structure::canonical(text).is_some() { return false }
    let parts: Vec<&str> = text.split([',', ';', '&']).flat_map(|part| part.split(" and ")).map(str::trim).filter(|part| !part.is_empty()).collect();
    let particle = |word: &str| ["van", "von", "de", "der", "den", "da", "di", "du", "del", "dos", "le", "la", "y", "bin", "al"].contains(&word);
    let name = |part: &&str| {
        let words: Vec<&str> = part.split_whitespace().collect();
        let capital = |word: &&str| word.chars().next().is_some_and(|c| c.is_alphabetic() && !c.is_lowercase());
        // The first and the last word a capital (`Xue lian Liang`, `Ludwig van Beethoven`).
        (1..=5).contains(&words.len()) && words.first().is_some_and(capital) && words.last().is_some_and(|word| capital(word) || particle(word))
    };
    !parts.is_empty() && parts.iter().all(name) && (parts.len() >= 2 || parts[0].split_whitespace().count() >= 2)
}

/// Names without their marks of affiliation and notes: `Nan Zhao1,3†, Xiawei Ding1,2†` → `Nan Zhao,
/// Xiawei Ding`; also the `ID` of an ORCID mark set after a name (`Guojun GuoID`).
fn unmarked(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mark = |c: char| c.is_ascii_digit() || "†‡*∗§¶☯#".contains(c);
    let mut out = String::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        let after_name = out.chars().last().is_none_or(|last| last.is_alphabetic() || last == '.' || last.is_whitespace() || last == ',');
        if mark(c) && (after_name || !c.is_ascii_digit()) {
            // The commas between the numbers go with them: `Zhao1,3†,` → `Zhao,`.
            while at < chars.len() && (mark(chars[at]) || (chars[at] == ',' && chars.get(at + 1).is_some_and(|n| mark(*n)))) { at += 1 }
            continue;
        }
        out.push(c);
        at += 1;
    }
    let words: Vec<String> = out.split_whitespace().map(|word| {
        let (stem, comma) = word.strip_suffix(',').map_or((word, ""), |stem| (stem, ","));
        let stem = stem.strip_suffix("ID").filter(|rest| rest.chars().last().is_some_and(char::is_lowercase)).unwrap_or(stem);
        format!("{stem}{comma}")
    }).collect();
    words.join(" ").replace(" ,", ",").trim_matches([',', ' ']).to_string()
}

/// The sound as it arrives, bytes to samples: a sample whose two bytes come in two reads is kept whole.
#[derive(Default)]
pub struct Pcm {
    odd: Option<u8>,
}

impl Pcm {
    /// The samples in `bytes` (and the byte left over from the last time), in -1 to 1.
    pub fn samples(&mut self, bytes: &[u8]) -> Vec<f32> {
        let mut all = Vec::with_capacity(bytes.len() + 1);
        all.extend(self.odd.take());
        all.extend_from_slice(bytes);
        if all.len() % 2 == 1 { self.odd = all.pop() }
        all.as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair) as f32 / 32768.).collect()
    }
}

/// How long `samples` sound.
pub fn seconds(samples: usize) -> f32 {
    samples as f32 / SAMPLE_RATE as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_endpoint_is_at_the_hosts_root() {
        assert_eq!(endpoint("https://api.deepseek.com"), "https://api.deepseek.com/v1/tools/tts");
        assert_eq!(endpoint("https://api.deepseek.com/v1/"), "https://api.deepseek.com/v1/tools/tts");
        assert_eq!(endpoint("https://api.deepseek.com/anthropic"), "https://api.deepseek.com/v1/tools/tts");
        assert_eq!(endpoint("http://127.0.0.1:8080/v1"), "http://127.0.0.1:8080/v1/tools/tts");
        assert!(is_service("https://api.deepseek.com/v1") && !is_service("https://api.moonshot.cn/v1"));
    }

    #[test]
    fn a_request_asks_for_pcm_in_a_known_voice() {
        let asked = request("https://api.deepseek.com", " sk-1 ", "", "tide", "Hello.").unwrap();
        assert_eq!(asked.body, json!({"text": "Hello.", "voice_id": "tide", "format": "pcm", "filter_markdown": false}));
        assert!(asked.headers.contains(&("authorization".into(), "Bearer sk-1".into())));
        assert_eq!(request("https://api.deepseek.com", "k", "", "nobody", "x").unwrap().body["voice_id"], "stella");
        assert!(request("https://api.deepseek.com", "", "", "", "x").unwrap_err().contains("API Key"));
        assert!(request("https://api.deepseek.com", "k", "", "", "  ").is_err());
    }

    #[test]
    fn chinese_counts_twice() {
        assert_eq!(billed("ab 1"), 4);
        assert_eq!(billed("细胞分裂"), 8);
        assert_eq!(billed("细胞，cell"), 10);
    }

    #[test]
    fn lines_are_joined_as_they_are_read() {
        let page = "The exam-\nple shows that cells [3]\ndivide [12, 14–16].\n7\nSee https://x.org/a for COVID-\n19 data.\n细胞\n分裂［1］很快。";
        assert_eq!(readable(page), "The example shows that cells divide. See for COVID-19 data.细胞分裂很快。");
        // Brackets with words in them are part of the text.
        assert_eq!(readable("the [Ca2+] level and [see above]"), "the [Ca2+] level and [see above]");
        assert_eq!(readable("an unclosed [3 bracket"), "an unclosed [3 bracket");
    }

    #[test]
    fn pieces_end_with_sentences_and_keep_to_the_budget() {
        let text = "One two three. Four five six. Seven eight nine. e.g. ten";
        assert_eq!(pieces(text, 30), ["One two three. Four five six.", "Seven eight nine. e.g. ten"]);
        let chinese = "细胞分裂。".repeat(10);
        let cut = pieces(&chinese, 20);
        assert!(cut.iter().all(|piece| billed(piece) <= 20 && piece.ends_with('。')), "{cut:?}");
        assert_eq!(cut.concat(), chinese);
        // A sentence longer than a piece is cut too, and nothing of it is lost.
        let long = "word ".repeat(100);
        let cut = pieces(&long, 64);
        assert!(cut.iter().all(|piece| billed(piece) <= 64));
        assert_eq!(cut.join(" ").split_whitespace().count(), 100);
        assert!(pieces("  ", 64).is_empty());
    }

    #[test]
    fn the_sentence_heard_follows_the_time_played() {
        let text = "One two. Three four five six seven eight. 九十。";
        assert_eq!(sentences(text), ["One two.", " Three four five six seven eight.", " 九十。"]);
        assert_eq!(sentence_at(text, 0.), Some("One two."));
        assert_eq!(sentence_at(text, 0.5), Some("Three four five six seven eight."));
        assert_eq!(sentence_at(text, 1.), Some("九十。"));
        assert_eq!(sentence_at(text, 7.), Some("九十。"));
        assert_eq!(sentence_at("  ", 0.5), None);
    }

    #[test]
    fn reading_starts_at_a_page_and_stops_at_the_references() {
        let pages = vec!["Abstract.\nFirst page.".to_string(), "Second page.".to_string(), "Last words.\n\nReferences\n1. Smith J. A paper.".to_string(), "More references.".to_string()];
        let none = Paper::default();
        assert_eq!(from_pages(&pages, 2, &none), [(2, "Second page.".to_string()), (3, "Last words.".to_string())]);
        assert_eq!(from_pages(&pages, 0, &none)[0], (1, "Abstract. First page.".to_string()));
        assert!(references_heading("  5. References: ") && references_heading("参考文献") && !references_heading("References are given below."));
        assert!(from_pages(&pages, 9, &none).is_empty());
    }

    /// Pages after the first as the reader gives them: the running head and foot round some lines of text.
    fn later_pages(head: &dyn Fn(usize) -> String, bodies: &[&str]) -> Vec<String> {
        bodies.iter().enumerate().map(|(index, body)| head(index + 2).replace("{body}", body)).collect()
    }

    /// BMC Plant Biology, 2024 (Zhao et al., "Transcriptional landscape of sweetpotato root tip
    /// development at the single-cell level"): pages 1 to 4 as `fulltext::file_pages` reads them, cut short.
    fn bmc() -> (Vec<String>, Paper) {
        let first = "R E S E A R C H Open Access\n\n© The Author(s) 2024. Open Access This article is licensed under a Creative Commons Attribution-NonCommercial-NoDerivatives 4.0\nInternational License, which permits any non-commercial use, sharing, distribution and reproduction in any medium or format, as long as you\ncreativecommons.org/licenses/by-nc-nd/4.0/.\n\nZhao et al. BMC Plant Biology (2024) 24:952\nhttps://doi.org/10.1186/s12870-024-05574-8\n\nBMC Plant Biology\n\n†Nan Zhao and Xiawei Ding co-first author.\n\n*Correspondence:\nXue lian Liang\nliangxuelian2005@sina.com\nCrop Research Institute, Guangdong Academy of Agricultural\nSciences, Guangdong Provincial Key Laboratory of Crops Genetics and\nImprovement, Guangzhou 510000, Guangdong, China\n\nAbstract\n\nSingle-cell transcriptome sequencing (scRNA-seq) is a powerful tool for describing the transcriptome dynamics\nof plant development but has not yet been utilized to analyze the tissue ontology of sweetpotato. This study\nmapped the single-cell transcriptome of sweetpotato root tips.\nHighlights\n• This is the first single-cell transcriptional atlas of sweetpotato root apex tissue.\n• GATA4 may be involved in regulating sweetpotato root tip development.\nKeywords Ipomoea batatas (L.) Lam., scRNA-seq, Root, Transitional cells, Developmental trajectory\n\nTranscriptional landscape of sweetpotato root\ntip development at the single-cell level\n\nNan Zhao1,3†, Xiawei Ding1,2†, CaiHuan Tian, Shixin Wang, Shuyan Xie, Hongda Zou, Hao Liu, Jingyi Chen,\nXue lian Liang3* and Lifei Huang1*";
        let mut pages = vec![first.to_string()];
        pages.extend(later_pages(&|n| format!("Page {n} of 17\n\nZhao et al. BMC Plant Biology (2024) 24:952\n\n{{body}}"), &[
            "Introduction\n\nIpomoea batatas (L.) Lam. or sweetpotato, belonging\nto the order Ipomoea of the family Convolvulaceae [1,\n2], exhibits homologous hexaploidy (2n = 6x = 90) with\na large genome. Sweetpotato tubers are starch rich.",
            "Fig. 1 Cellular heterogeneity cluster analysis of the sweetpotato root single-cell transcriptome.\n\nA total of 8,545 significantly up-regulated genes were identified.",
            "Since increased cell division rate increases the probability of spontaneous mutations, the cells divide less.",
        ]));
        (pages, Paper { title: "Transcriptional landscape of sweetpotato root tip development at the single-cell level".into(), journal: "BMC Plant Biol".into(), ..Default::default() })
    }

    /// Frontiers in Plant Science, 2024 (Bi et al., "Dynamics of alternative polyadenylation in single
    /// root cells of Arabidopsis thaliana"): the title first, the sidebar of front matter after the abstract.
    fn frontiers() -> (Vec<String>, Paper) {
        let first = "Dynamics of alternative\npolyadenylation in single root\ncells of Arabidopsis thaliana\n\nXingyu Bi1†, Sheng Zhu2†, Fei Liu and Xiaohui Wu*\n\nCancer Institute, Suzhou Medical College, Soochow University, Suzhou, China,Operational\nTechnology Research and Evaluation Center, China Nuclear Power Operation Technology\nCorporation, Ltd, Wuhan, China\n\nIntroduction: Single-cell RNA-seq (scRNA-seq) technologies have been widely\nused to reveal the diversity and complexity of cells.\n\nDiscussion: This study reveals that APA provides an additional layer of\ninformation for determining cell identity.\n\nKEYWORDS\n\nalternative polyadenylation, single-cell RNA-seq, root development, RNA processing,\nbioinformatics and computational biology\n\nFrontiers in Plant Science frontiersin.org\n\n01\n\nOPEN ACCESS\n\nEDITED BY\n\nGeorge V. Popescu,\nMississippi State University, United States\n\nREVIEWED BY\n\nXiujun Zhang,\nChinese Academy of Sciences (CAS), China\n\n*CORRESPONDENCE\n\nXiaohui Wu\nxhwu@suda.edu.cn\n\n†These authors have contributed equally to\nthis work\n\nRECEIVED 23 May 2024\nACCEPTED 02 September 2024\nPUBLISHED 20 September 2024\n\nCITATION\n\nBi X, Zhu S, Liu F and Wu X (2024) Dynamics\nof alternative polyadenylation in single root\ncells of Arabidopsis thaliana.\nFront. Plant Sci. 15:1437118.\ndoi: 10.3389/fpls.2024.1437118\n\nCOPYRIGHT\n\n© 2024 Bi, Zhu, Liu and Wu. This is an open-\naccess article distributed under the terms of\nthe Creative Commons Attribution License\n(CC BY).\n\nTYPE Original Research\nPUBLISHED 20 September 2024\nDOI 10.3389/fpls.2024.1437118";
        let mut pages = vec![first.to_string()];
        pages.extend(later_pages(&|n| format!("{{body}}\n\nBi et al. 10.3389/fpls.2024.1437118\n\nFrontiers in Plant Science frontiersin.org\n\n0{n}"), &[
            "1 Introduction\n\nRecent advances in single-cell RNA-seq (scRNA-seq) have\nopened an unprecedented opportunity (Saliba et al., 2014; Kolodziejczyk et al., 2015; Wang and\nNavin, 2015). Generally, these studies define cell types in\nArabidopsis root (e.g., epidermis, endodermis, cortex, stele, and\nroot cap).",
            "2 Materials and methods\n\n2.1 Datasets\n\nThe data of five studies were taken.",
            "3 Results\n\nThe sites were found.",
        ]));
        (pages, Paper { title: "Dynamics of alternative polyadenylation in single root cells of Arabidopsis thaliana".into(), journal: "Frontiers in Plant Science".into(), ..Default::default() })
    }

    /// PLOS ONE, 2024 (Guo et al., "Single-cell transcriptome analysis reveals distinct cell populations
    /// in dorsal root ganglia…"): the authors broken by ORCID marks, the sidebar and Crossmark's glyphs
    /// set in the middle of the introduction's sentence, a field of it on page 2.
    fn plos() -> (Vec<String>, Paper) {
        let first = "RESEARCH ARTICLE\n\nSingle-cell transcriptome analysis reveals\ndistinct cell populations in dorsal root ganglia\nand their potential roles in diabetic peripheral\nneuropathy\n\nGuojun GuoID\n\n1☯, Jing Chen2☯, Qixiao Shen*, Zhenbing ChenID\n\n*\n\n1 Department of Hand Surgery, Union Hospital, Tongji Medical College, Huazhong University of Science and\nTechnology, Wuhan, Hubei, China\n\n☯ These authors contributed equally to this work.\n* zbchen@hust.edu.cn (ZC); 13872097111@163.com (QS)\n\nAbstract\n\nDiabetic peripheral neuropathy (DPN) is a common complication associated with diabetes,\nand can affect quality of life considerably.\n\nIntroduction\n\nDiabetic peripheral neuropathy (DPN) is the most common form of neuropathy [1]. With the\nprogress of the disease, the individuals may suffer from foot ulceration, neuropathic pain or\n\nPLOS ONE\n\nPLOS ONE | https://doi.org/10.1371/journal.pone.0306424 July 31, 2024 1 / 22\n\na1111111111\na1111111111\n\nOPEN ACCESS\n\nCitation: Guo G, Chen J, Shen Q, Chen Z (2024)\nSingle-cell transcriptome analysis reveals distinct\ncell populations in dorsal root ganglia.\n\nEditor: Xianmin Zhu, ShanghaiTech University,\nCHINA\n\nReceived: January 22, 2024\n\nCopyright: © 2024 Guo et al. This is an open access\narticle distributed under the terms of the Creative\nCommons Attribution License.\n\nFunding: This work was supported by the National\nNatural Science Foundation of China (82001339).";
        let mut pages = vec![first.to_string()];
        pages.extend(later_pages(&|n| format!("PLOS ONE scRNA-seq and diabetic peripheral neuropathy\n\n{{body}}\n\nPLOS ONE | https://doi.org/10.1371/journal.pone.0306424 July 31, 2024 {n} / 22"), &[
            "even lower-limb amputation, and the quality of life is impaired significantly [2]. The pathogen-\nesis of DPN is complex.\n\nCompeting interests: The authors have declared\nthat no competing interests exist.",
            "Neurons were grouped (Fig. 1A). The SGCs were many.",
            "The rats were kept.",
        ]));
        (pages, Paper { title: "Single-cell transcriptome analysis reveals distinct cell populations in dorsal root ganglia and their potential roles in diabetic peripheral neuropathy".into(), journal: "PLoS ONE".into(), ..Default::default() })
    }

    #[test]
    fn running_heads_feet_and_page_numbers_are_not_read() {
        for (pages, paper) in [bmc(), frontiers(), plos()] {
            let read: Vec<(usize, String)> = from_pages(&pages, 2, &paper);
            let all = read.iter().map(|(_, piece)| piece.as_str()).collect::<Vec<_>>().join(" ");
            for furniture in ["Page 2 of 17", "of 17", "Zhao et al.", "BMC Plant Biology", "Bi et al.", "frontiersin.org", "Frontiers in Plant Science", "PLOS ONE", "/ 22", "July 31", "Competing interests"] {
                assert!(!all.contains(furniture), "{furniture:?} read in {all:?}");
            }
            assert!(read.iter().all(|(page, _)| *page >= 2));
        }
        let (pages, paper) = bmc();
        assert!(from_pages(&pages, 2, &paper)[0].1.starts_with("Introduction. Ipomoea batatas (L.) Lam. or sweetpotato, belonging to the order Ipomoea"));
        let (pages, paper) = plos();
        assert_eq!(page_texts(&pages, &paper)[1], "even lower-limb amputation, and the quality of life is impaired significantly. The pathogenesis of DPN is complex.");
        assert!(page_number("Page 2 of 17") && page_number("2 / 22") && page_number("12") && page_number("第 3 页") && !page_number("2 of the cells"));
    }

    #[test]
    fn the_first_page_reads_title_authors_abstract_then_body() {
        let (pages, paper) = bmc();
        let first = &from_pages(&pages, 1, &paper)[0];
        assert_eq!(first.0, 1);
        assert_eq!(first.1, "Transcriptional landscape of sweetpotato root tip development at the single-cell level. Nan Zhao, Xiawei Ding, CaiHuan Tian, Shixin Wang, Shuyan Xie, Hongda Zou, Hao Liu, Jingyi Chen, Xue lian Liang and Lifei Huang. Abstract. Single-cell transcriptome sequencing (scRNA-seq) is a powerful tool for describing the transcriptome dynamics of plant development but has not yet been utilized to analyze the tissue ontology of sweetpotato. This study mapped the single-cell transcriptome of sweetpotato root tips. Highlights. This is the first single-cell transcriptional atlas of sweetpotato root apex tissue. GATA4 may be involved in regulating sweetpotato root tip development.");
        let (pages, paper) = frontiers();
        assert_eq!(page_texts(&pages, &paper)[0], "Dynamics of alternative polyadenylation in single root cells of Arabidopsis thaliana. Xingyu Bi, Sheng Zhu, Fei Liu and Xiaohui Wu. Introduction: Single-cell RNA-seq (scRNA-seq) technologies have been widely used to reveal the diversity and complexity of cells. Discussion: This study reveals that APA provides an additional layer of information for determining cell identity.");
        let (pages, paper) = plos();
        assert_eq!(page_texts(&pages, &paper)[0], "Single-cell transcriptome analysis reveals distinct cell populations in dorsal root ganglia and their potential roles in diabetic peripheral neuropathy. Guojun Guo, Jing Chen, Qixiao Shen, Zhenbing Chen. Abstract. Diabetic peripheral neuropathy (DPN) is a common complication associated with diabetes, and can affect quality of life considerably. Introduction. Diabetic peripheral neuropathy (DPN) is the most common form of neuropathy. With the progress of the disease, the individuals may suffer from foot ulceration, neuropathic pain or");
        // Without the record the front matter is left out all the same, the journal's name too (it is in the running head), and the names lose their marks.
        let (pages, _) = bmc();
        let text = &page_texts(&pages, &Paper::default())[0];
        assert!(text.starts_with("Abstract. Single-cell") && text.ends_with("Transcriptional landscape of sweetpotato root tip development at the single-cell level. Nan Zhao, Xiawei Ding, CaiHuan Tian, Shixin Wang, Shuyan Xie, Hongda Zou, Hao Liu, Jingyi Chen, Xue lian Liang and Lifei Huang."), "{text}");
        // The podcast is written from the same text.
        let (pages, paper) = bmc();
        assert!(body(&pages, &paper).starts_with("Transcriptional landscape") && !body(&pages, &paper).contains("Page 2"));
    }

    #[test]
    fn letter_spaced_words_and_marks_of_names() {
        assert_eq!(without_spaced_letters("R E S E A R C H Open Access"), "Open Access");
        assert_eq!(without_spaced_letters("genes A B C were"), "genes A B C were");
        assert_eq!(unmarked("Nan Zhao1,3†, Xiawei Ding1,2†, Xue lian Liang3* and Lifei Huang1*"), "Nan Zhao, Xiawei Ding, Xue lian Liang and Lifei Huang");
        assert_eq!(unmarked("1☯, Jing Chen2☯, Qixiao Shen*, Zhenbing ChenID"), "Jing Chen, Qixiao Shen, Zhenbing Chen");
        assert!(authors("Tran N. Chau, Prakash Raj Timilsena, Bastiaan O. R. Bargmann & Song Li") && !authors("Single-cell transcriptome sequencing is a powerful tool") && !authors("Abstract"));
    }

    #[test]
    fn sentences_do_not_end_at_abbreviations() {
        let cut = |text: &'static str| sentences(text).into_iter().map(str::trim).collect::<Vec<_>>();
        // BMC Plant Biology: a species' author, a figure, an initial of a genus.
        assert_eq!(cut("Ipomoea batatas (L.) Lam. or sweetpotato, belonging to the order Ipomoea. Total RNA was used (Fig. 1a). After removing cells, the data between sweetpotato, A. thaliana, and maize were compared."), [
            "Ipomoea batatas (L.) Lam. or sweetpotato, belonging to the order Ipomoea.", "Total RNA was used (Fig. 1a).", "After removing cells, the data between sweetpotato, A. thaliana, and maize were compared.",
        ]);
        // Frontiers: author–year citations, `e.g.`; a sentence that opens with a name in mixed case.
        assert_eq!(cut("They opened an opportunity (Saliba et al., 2014; Kolodziejczyk et al., 2015; Wang and Navin, 2015). Cell types in root (e.g., epidermis) are known at the transcriptome level. scRNA-seq is a technology."), [
            "They opened an opportunity (Saliba et al., 2014; Kolodziejczyk et al., 2015; Wang and Navin, 2015).", "Cell types in root (e.g., epidermis) are known at the transcriptome level.", "scRNA-seq is a technology.",
        ]);
        // Nature Communications: initials of names; `et al.`, `vs.`, `i.e.` before capitals.
        assert_eq!(cut("By Bastiaan O. R. Bargmann & Song Li. As Denyer et al. Showed, the cells (i.e. Root hairs) of WT vs. Mutant differ. Done; next."), ["By Bastiaan O. R. Bargmann & Song Li.", "As Denyer et al. Showed, the cells (i.e. Root hairs) of WT vs. Mutant differ.", "Done;", "next."]);
    }

    #[test]
    fn samples_split_across_reads_stay_whole() {
        let mut pcm = Pcm::default();
        let bytes: Vec<u8> = [0i16, 16384, -32768].iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut got = pcm.samples(&bytes[..3]);
        got.extend(pcm.samples(&bytes[3..]));
        assert_eq!(got, [0., 0.5, -1.]);
        assert_eq!(seconds(48_000), 2.);
    }
}
