//! 文字识别 (OCR): the text of a file whose pages give none (a scan, a
//! font without an encoding), read off the pages by a service. Every engine
//! is one entry of `ENGINES`; one that takes the file itself also has an arm
//! in `request` and `read`, and nothing else knows which engines there are.
//! Nothing here sends a request: `wisp_net::read_file` does, and the tool
//! `ocr_pdf` is handed it (`tools::Reach`).
//!
//! What an engine answered is kept as it came (`Page::raw`, the table
//! `ocr_pages`): recognition costs, and the places it gives (`blocks`) are
//! for a text layer over the page that is still to be drawn.

use crate::settings::{Settings, Task};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A way of reading the pages of a file.
#[derive(Debug, PartialEq)]
pub struct Engine {
    /// What settings and the tool's `engine` call it.
    pub key: &'static str,
    pub name: &'static str,
    /// The environment variable its key is read from when none is stored; empty for an engine without a key of its own.
    pub key_env: &'static str,
    /// Where a key is had.
    pub site: &'static str,
    /// The most pages of one file it takes; 0 is no limit of its own.
    pub pages: usize,
    /// The largest file it takes, in bytes; 0 is no limit of its own.
    pub bytes: usize,
}

/// The engine that is a model of 设置 › AI looking at pictures of the pages, one page a request.
pub const MODEL: &str = "model";

/// The engines, in the order 自动 tries them. A new one that takes the file is a row here and an arm in `request` and `read`.
pub const ENGINES: [Engine; 2] = [
    // https://api-docs.deepseek.com/zh-cn/guides/ocr/: the key of DeepSeek's chat API, the whole file in one request.
    Engine { key: "deepseek", name: "DeepSeek OCR", key_env: "DEEPSEEK_API_KEY", site: "https://platform.deepseek.com/api_keys", pages: 1000, bytes: 100 << 20 },
    // The model of 写作's groundwork (`Task::Structure`), which has to be one that sees pictures.
    Engine { key: MODEL, name: "看图模型", key_env: "", site: "", pages: 0, bytes: 0 },
];

/// `OcrSettings::engine` for the first engine that can be asked.
pub const AUTO: &str = "";
/// `OcrSettings::engine` for none at all.
pub const OFF: &str = "off";
/// What DeepSeek is asked for (`purpose`): a PDF is read page by page either way, and this is the one meant for text.
const PURPOSE: &str = "text_recognition";

/// 设置 › 网络 › 文字识别.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OcrSettings {
    /// An `Engine::key`, `AUTO` or `OFF`.
    pub engine: String,
    /// The key stored for an engine, by `Engine::key`. DeepSeek's may be left out: the DeepSeek model service's is used.
    pub keys: BTreeMap<String, String>,
}

pub fn engine(key: &str) -> Option<&'static Engine> {
    ENGINES.iter().find(|engine| engine.key == key.trim().to_ascii_lowercase())
}

/// An engine with what it is asked with.
#[derive(Clone, Debug, PartialEq)]
pub struct Ready {
    pub engine: &'static Engine,
    pub key: String,
    /// The proxy of the model service whose key it borrows (`Provider::proxy`); empty is the global one.
    pub proxy: String,
    /// The address of the model service whose key it borrows; empty is the engine's own.
    pub base: String,
}

impl Ready {
    /// Who read the text, as kept with it (`structures.text_by`) and shown: the engine, or the model that looked.
    pub fn author(&self, settings: &Settings) -> String {
        if self.engine.key == MODEL { settings.profile_for_or_default(Task::Structure).labelled() } else { self.engine.name.to_string() }
    }
}

/// The key an engine would be asked with now, the proxy and the address that go with it; an empty key when it has none.
/// Stored for 文字识别 first; for DeepSeek then the DeepSeek model service's (stored or `DEEPSEEK_API_KEY`); then the engine's variable.
pub fn key_for(settings: &Settings, engine: &'static Engine) -> (String, String, String) {
    key_from(settings, engine, &|name| std::env::var(name).ok())
}

fn key_from(settings: &Settings, engine: &'static Engine, env: &dyn Fn(&str) -> Option<String>) -> (String, String, String) {
    let stored = settings.ocr.keys.get(engine.key).map(|key| key.trim()).unwrap_or("");
    if !stored.is_empty() { return (stored.to_string(), String::new(), String::new()) }
    if engine.key == "deepseek" && let Some(provider) = settings.providers.iter().find(|p| (p.preset == "deepseek" || crate::speech::is_service(&p.base_url)) && !p.key().is_empty()) {
        return (provider.key(), provider.proxy.clone(), provider.base_url.clone());
    }
    (env(engine.key_env).map(|key| key.trim().to_string()).unwrap_or_default(), String::new(), String::new())
}

/// The engine a file is read with: `asked` (an `Engine::key`) when given, else the one chosen in settings, else
/// the first that can be asked. The error is a sentence for the person or the model, saying what is missing.
pub fn ready(settings: &Settings, asked: &str) -> Result<Ready, String> {
    ready_from(settings, asked, &|name| std::env::var(name).ok())
}

fn ready_from(settings: &Settings, asked: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Ready, String> {
    let chosen = settings.ocr.engine.trim();
    if asked.trim().is_empty() && chosen == OFF { return Err("文字识别已在「设置 › 网络」关闭。".into()) }
    let wanted = if asked.trim().is_empty() { chosen } else { asked.trim() };
    let one = |engine: &'static Engine| -> Result<Ready, String> {
        if engine.key == MODEL {
            // A model that cannot be asked yet says what it lacks; whether it sees pictures only a request tells.
            return match settings.profile_for(Task::Structure).filter(|profile| profile.missing().is_none()) {
                Some(_) => Ok(Ready { engine, key: String::new(), proxy: String::new(), base: String::new() }),
                None => Err("看图模型读页面需要一个能看图的模型：在「设置 › AI」配置，并指给「整理全文」这项任务。".into()),
            };
        }
        let (key, proxy, base) = key_from(settings, engine, env);
        if key.is_empty() { return Err(format!("{} 需要 API Key：在「设置 › 网络 › 文字识别」填写，或设置环境变量 {}（在 {} 申请）。", engine.name, engine.key_env, engine.site)) }
        Ok(Ready { engine, key, proxy, base })
    };
    if !wanted.is_empty() {
        let Some(engine) = engine(wanted) else { return Err(format!("没有这个文字识别引擎：{wanted}。可用的有 {}。", names())) };
        return one(engine);
    }
    ENGINES.iter().find_map(|engine| one(engine).ok()).ok_or_else(|| "文字识别需要 DeepSeek 的 API Key（在「设置 › 网络 › 文字识别」填写，或用「设置 › AI」里 DeepSeek 服务的 Key），或一个能看图的模型（「设置 › AI」）。".to_string())
}

fn names() -> String {
    ENGINES.iter().map(|engine| engine.key).collect::<Vec<_>>().join("、")
}

/// A file on its way to an engine: a POST whose body is a form with the file in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Upload {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub proxy: String,
}

/// Where DeepSeek's OCR is asked: at the root of the host its key is for, whatever path chat has there.
fn deepseek_endpoint(base: &str) -> String {
    if base.trim().is_empty() { return "https://api.deepseek.com/v1/tools/ocr".into() }
    crate::speech::endpoint(base).replace("/v1/tools/tts", "/v1/tools/ocr")
}

/// The request that has an engine read the file `name` holds. What the engine would refuse is said before anything is sent.
pub fn request(ready: &Ready, name: &str, file: &[u8]) -> Result<Upload, String> {
    let engine = ready.engine;
    if engine.bytes > 0 && file.len() > engine.bytes {
        return Err(format!("{} 最多接受 {} MB 的文件，这个文件有 {} MB：先用「压缩 PDF」把它变小。", engine.name, engine.bytes >> 20, file.len().div_ceil(1 << 20)));
    }
    match engine.key {
        "deepseek" => {
            let (kind, body) = form(&[("purpose", PURPOSE)], name, file);
            Ok(Upload { url: deepseek_endpoint(&ready.base), headers: vec![("Authorization".into(), format!("Bearer {}", ready.key)), ("Content-Type".into(), kind)], body, proxy: ready.proxy.clone() })
        }
        MODEL => Err("看图模型按页面的图片读，不接受整个文件。".into()),
        other => Err(format!("没有这个文字识别引擎：{other}。")),
    }
}

/// A `multipart/form-data` body of text fields and one file, and the content type that names its boundary.
fn form(fields: &[(&str, &str)], name: &str, file: &[u8]) -> (String, Vec<u8>) {
    // No file holds a line made of its own length and of when it was sent.
    let boundary = format!("wisp-study-{:x}-{:x}", crate::time::now_millis(), file.len());
    let mut body = Vec::with_capacity(file.len() + 512);
    for (field, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    // A name the header can carry as it is: the service reads the kind of file off its bytes.
    let plain: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
    let plain = if plain.trim_matches('_').is_empty() || !plain.contains('.') { "file.pdf".to_string() } else { plain };
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{plain}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// One page as an engine read it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    /// The text of the page, as it is kept and asked about.
    pub text: String,
    /// What the engine answered for the page, its marks of place included.
    pub raw: String,
    /// The engine stopped before the page's end: its text is not all there.
    pub cut: bool,
}

/// A file as an engine read it.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub engine: &'static Engine,
    pub pages: Vec<Page>,
}

impl Reading {
    /// The pages whose text stops short of their end, 1-based.
    pub fn cut(&self) -> Vec<usize> {
        self.pages.iter().enumerate().filter(|(_, page)| page.cut).map(|(index, _)| index + 1).collect()
    }
}

/// The pages (from 0) of a file whose text was read off its pages that came back without any: those a second
/// reading sends again. None of a file that gave its own text (a page of it without words is a figure, not a page
/// left unread) or that was read only in part (the pages not read yet come first).
pub fn blank_pages(info: Option<&crate::store::StructureInfo>, pages: &[String]) -> Vec<usize> {
    if info.is_none_or(|info| info.text_by.is_empty() || info.read_in_part()) { return vec![] }
    pages.iter().enumerate().filter(|(_, page)| page.trim().is_empty()).map(|(index, _)| index).collect()
}

/// A PDF of only these pages of `pdf` (from 0, in order): what is sent to read a few pages again, so the pages
/// that have their text are not read, nor billed, twice.
pub fn only_pages(pdf: &[u8], keep: &[usize]) -> Result<Vec<u8>, String> {
    let mut doc = lopdf::Document::load_mem(pdf).map_err(|err| format!("无法解析这个 PDF 的结构（{err}）。"))?;
    let count = doc.get_pages().len() as u32;
    let gone: Vec<u32> = (1..=count).filter(|number| !keep.contains(&(*number as usize - 1))).collect();
    if gone.len() as u32 == count { return Err("要读的页面不在这个 PDF 里。".into()) }
    doc.delete_pages(&gone);
    doc.prune_objects();
    let mut bytes = vec![];
    doc.save_to(&mut bytes).map_err(|err| format!("无法写出要读的页面（{err}）。"))?;
    Ok(bytes)
}

/// The whole file again with the pages read a second time in their places: `pages` and `raw` are what was kept
/// (`raw` may be shorter: a file no engine read before), `at` the pages sent, `read` what came back for them, or for
/// the whole file when it was sent whole. A page that came back without text keeps what it had.
pub fn refill(pages: &[String], raw: &[String], at: &[usize], read: Vec<Page>) -> Result<Vec<Page>, String> {
    let mut whole: Vec<Page> = pages.iter().enumerate().map(|(index, text)| Page { text: text.clone(), raw: raw.get(index).cloned().unwrap_or_default(), cut: false }).collect();
    let read: Vec<(usize, Page)> = match read.len() {
        n if n == at.len() => at.iter().copied().zip(read).collect(),
        n if n == pages.len() => read.into_iter().enumerate().filter(|(index, _)| at.contains(index)).collect(),
        n => return Err(format!("送去 {} 页，读回 {n} 页，对不上哪一页是哪一页。", at.len())),
    };
    for (index, page) in read {
        if !page.text.trim().is_empty() && let Some(slot) = whole.get_mut(index) { *slot = page }
    }
    Ok(whole)
}

/// The front end's way of reading a file: the engine asked for (empty: as settings say), the file's name and its bytes.
pub type Read<'a> = &'a dyn Fn(&str, &str, &[u8]) -> Result<Reading, String>;

/// The pages in an engine's answer, or what went wrong in a sentence: `status` is the HTTP status.
/// DeepSeek answers 200 once it has begun and may still fail: then the body says so.
pub fn read(engine: &Engine, status: u16, body: &str) -> Result<Vec<Page>, String> {
    let value: Option<Value> = serde_json::from_str(body).ok();
    let failure = value.as_ref().map(|v| &v["error"]).filter(|error| !error.is_null());
    if !(200..300).contains(&status) || failure.is_some() {
        let error = failure.unwrap_or(&Value::Null);
        let said = error["message"].as_str().or(error.as_str()).or(value.as_ref().and_then(|v| v["message"].as_str())).map(str::trim).filter(|s| !s.is_empty()).map(String::from).unwrap_or_else(|| if value.is_some() { String::new() } else { body.trim().chars().take(300).collect() });
        let late = error["code"].as_str().or(error["type"].as_str()).is_some_and(|code| code.contains("timeout"));
        let hint = match status {
            _ if late => "没有在限定的时间里读完，页数多的文件可以稍后再试",
            200..300 => "没有读成",
            400 => "文件无法处理（设了密码、已损坏或不是支持的格式）",
            401 => "API Key 无效",
            402 => "账户余额不足",
            408 => "文件上传得太慢",
            413 => "文件太大",
            429 => "同时在读的文件太多，请稍后再试",
            504 => "没有在限定的时间里读完，页数多的文件可以稍后再试",
            500..=599 => "服务暂时不可用，请稍后再试",
            _ => "请求有误",
        };
        let said = if said.is_empty() { String::new() } else { format!("：{said}") };
        let code = if (200..300).contains(&status) { String::new() } else { format!("（HTTP {status}）") };
        return Err(format!("{}：{hint}{code}{said}", engine.name));
    }
    let Some(value) = value else { return Err(format!("{} 返回的不是 JSON。", engine.name)) };
    let Some(items) = value["pages"].as_array() else { return Err(format!("{} 的返回内容无法识别。", engine.name)) };
    let mut items: Vec<&Value> = items.iter().collect();
    items.sort_by_key(|item| item["index"].as_u64().unwrap_or(u64::MAX));
    Ok(items
        .iter()
        .map(|item| {
            let raw = item["text"].as_str().unwrap_or("").to_string();
            Page { text: plain(&raw), cut: item["finish_reason"] == "length", raw }
        })
        .collect())
}

/// A piece of a page an engine says the place of.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub text: String,
    /// Left, top, right, bottom, as fractions of the page's width and height from its top left.
    pub rect: [f32; 4],
}

/// One stretch of an engine's answer: text it says the place of, or text as it stands.
enum Piece<'a> {
    Placed(&'a str, Option<[f32; 4]>),
    Loose(&'a str),
}

/// An answer cut at its marks of place: `<｜ref｜>text<｜/ref｜><｜box｜>[[x1, y1, x2, y2]]<｜/box｜>`, the corners in
/// thousandths of the page. The bars are read wide or plain, and `det` as `box`: the model behind the service
/// has written both.
fn pieces(raw: &str) -> Vec<Piece<'_>> {
    // The next of `names` as an opening or closing mark at or after `from`: where it starts and where it ends.
    let mark = |from: usize, closing: bool, names: &[&str]| -> Option<(usize, usize)> {
        let mut at = from;
        while let Some(found) = raw[at..].find('<') {
            let start = at + found;
            let rest = &raw[start + 1..];
            let Some(bar) = ['｜', '|'].into_iter().find(|bar| rest.starts_with(*bar)) else {
                at = start + 1;
                continue;
            };
            let inner = &rest[bar.len_utf8()..];
            let inner = if closing { inner.strip_prefix('/') } else { Some(inner) };
            if let Some(end) = inner.and_then(|inner| names.iter().find_map(|name| inner.strip_prefix(name)?.strip_prefix(bar)?.strip_prefix('>'))) {
                return Some((start, raw.len() - end.len()));
            }
            at = start + 1;
        }
        None
    };
    let mut out = vec![];
    let mut at = 0;
    while let Some((start, text_from)) = mark(at, false, &["ref"]) {
        let Some((text_to, after)) = mark(text_from, true, &["ref"]) else { break };
        if start > at { out.push(Piece::Loose(&raw[at..start])) }
        // The place follows its text at once, or the text has none.
        let (rect, next) = match mark(after, false, &["box", "det"]).filter(|(open, _)| raw[after..*open].trim().is_empty()) {
            Some((_, from)) => match mark(from, true, &["box", "det"]) {
                Some((to, end)) => (rect(&raw[from..to]), end),
                None => (None, after),
            },
            None => (None, after),
        };
        out.push(Piece::Placed(&raw[text_from..text_to], rect));
        at = next;
    }
    if at < raw.len() { out.push(Piece::Loose(&raw[at..])) }
    out
}

/// The rectangle around the boxes a mark names (`[[x1, y1, x2, y2]]`, or several), as fractions of the page.
fn rect(boxes: &str) -> Option<[f32; 4]> {
    let numbers: Vec<f32> = boxes.split(|c: char| !(c.is_ascii_digit() || c == '.')).filter(|n| !n.is_empty()).filter_map(|n| n.parse().ok()).collect();
    if numbers.len() < 4 || !numbers.len().is_multiple_of(4) { return None }
    let mut around = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for corners in numbers.chunks(4) {
        around = [around[0].min(corners[0]), around[1].min(corners[1]), around[2].max(corners[2]), around[3].max(corners[3])];
    }
    let around = around.map(|n| (n / 1000.).clamp(0., 1.));
    (around[2] > around[0] && around[3] > around[1]).then_some(around)
}

/// The pieces of a page an engine gave the place of, in the order it read them.
pub fn blocks(raw: &str) -> Vec<Block> {
    pieces(raw).into_iter().filter_map(|piece| match piece {
        Piece::Placed(text, Some(rect)) if !text.trim().is_empty() => Some(Block { text: text.trim().to_string(), rect }),
        _ => None,
    })
    .collect()
}

/// The text of a page as it is kept and asked about, made of what an engine wrote. DeepSeek writes Markdown: a
/// heading opens with `#`, a table is HTML, and every number with a unit is a formula (`$4\ ^\circ\text{C}$`).
/// Left like that, no heading names a part of the paper and no sentence is found by its words; so the marks go,
/// a table becomes its rows, and a formula that says no more than figures, units and plain signs becomes those
/// (`untex`). A formula that is one stays as written. Marks of place go too: a piece with a place is a block of
/// the page (a heading, a paragraph), so a blank line stands between two of them.
pub fn plain(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for piece in pieces(raw) {
        match piece {
            Piece::Placed(text, _) => {
                let kept = out.trim_end().len();
                out.truncate(kept);
                if !out.is_empty() { out.push_str("\n\n") }
                out.push_str(text.trim());
                out.push_str("\n\n");
            }
            Piece::Loose(text) => {
                // After a block, what follows starts a paragraph of its own.
                let text = if out.ends_with("\n\n") { text.trim_start() } else { text };
                out.push_str(text);
            }
        }
    }
    // Tables and formulas are set aside while the Markdown is read: an underscore or a star of theirs is no emphasis.
    let mut aside: Vec<String> = vec![];
    let mut hold = |text: String| {
        aside.push(text);
        format!("{HELD}{}{HELD_END}", aside.len() - 1)
    };
    let mut lifted = String::with_capacity(out.len());
    let mut at = 0;
    for (range, rows) in tables(&out) {
        lifted.push_str(&out[at..range.start]);
        lifted.push_str("\n\n");
        lifted.push_str(&hold(rows));
        lifted.push_str("\n\n");
        at = range.end;
    }
    lifted.push_str(&out[at..]);
    let mut worded = String::with_capacity(lifted.len());
    let mut at = 0;
    for (whole, inner) in formulas(&lifted) {
        worded.push_str(&lifted[at..whole.start]);
        worded.push_str(&hold(untex(&lifted[inner]).unwrap_or_else(|| lifted[whole.clone()].to_string())));
        at = whole.end;
    }
    worded.push_str(&lifted[at..]);
    let mut text = crate::markdown::plain(&mended(&worded));
    for (index, held) in aside.iter().enumerate().rev() {
        text = text.replace(&format!("{HELD}{index}{HELD_END}"), held);
    }
    // Lines without spaces at their ends, and no more than one blank line between two.
    let mut tidy = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines().map(str::trim_end) {
        if line.trim().is_empty() {
            blank += 1;
            continue;
        }
        if !tidy.is_empty() { tidy.push_str(if blank > 0 { "\n\n" } else { "\n" }) }
        blank = 0;
        tidy.push_str(line);
    }
    tidy
}

/// A text of Markdown with the paragraphs put together again that the end of a column cut in two: the engine
/// reads the columns in their order and starts a paragraph anew where one ends. Only what plainly goes on is
/// joined: a long paragraph that stops at a small letter or a comma, before one that opens with a small letter
/// or a figure (or, in Chinese, Japanese and Korean, a paragraph that stops at a letter before one that opens
/// with one); neither is a heading, an item of a list or a table, which the Markdown still says here.
fn mended(text: &str) -> String {
    let mut paragraphs: Vec<String> = vec![];
    let mut open = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            open = false;
        } else if open && let Some(last) = paragraphs.last_mut() {
            last.push('\n');
            last.push_str(line);
        } else {
            paragraphs.push(line.to_string());
            open = true;
        }
    }
    // What stands by itself whatever comes before it: a heading, an item, a quotation, a picture, a table or a formula set aside.
    let own = |paragraph: &str| {
        let start = paragraph.trim_start();
        let numbered = start.split_once(['.', ')']).is_some_and(|(number, rest)| !number.is_empty() && number.len() <= 3 && number.chars().all(|c| c.is_ascii_digit()) && rest.starts_with(' '));
        numbered || start.starts_with(['#', '<', '|', '>', '!', HELD]) || start.starts_with("- ") || start.starts_with("* ") || start.starts_with("+ ") || start.starts_with("$$")
    };
    let mut out: Vec<String> = vec![];
    for paragraph in paragraphs {
        let glue = out.last().filter(|before| !own(before) && !own(&paragraph)).and_then(|before| {
            let (last, first) = (before.trim_end().chars().next_back()?, paragraph.trim_start().chars().next()?);
            let length = before.chars().count();
            if crate::structure::is_cjk(last) && crate::structure::is_cjk(first) && length >= 30 {
                Some("")
            } else if (last.is_ascii_lowercase() || last == ',') && (first.is_ascii_lowercase() || first.is_ascii_digit()) && length >= 60 {
                Some(" ")
            } else {
                None
            }
        });
        match (glue, out.last_mut()) {
            (Some(glue), Some(before)) => {
                let kept = before.trim_end().len();
                before.truncate(kept);
                before.push_str(glue);
                before.push_str(paragraph.trim_start());
            }
            _ => out.push(paragraph),
        }
    }
    out.join("\n\n")
}

/// What stands for a table or a formula set aside in `plain`, around its number: characters no page holds.
const HELD: char = '\u{E000}';
const HELD_END: char = '\u{E001}';

/// The HTML tables of a text: where each stands, and its rows as lines, the cells of a row apart by ` | `.
fn tables(text: &str) -> Vec<(std::ops::Range<usize>, String)> {
    // ASCII letters only change, so every byte keeps its place.
    let lower = text.to_ascii_lowercase();
    let mut out = vec![];
    let mut at = 0;
    while let Some(start) = lower[at..].find("<table").map(|found| at + found) {
        let Some(end) = lower[start..].find("</table>").map(|found| start + found + "</table>".len()) else { break };
        let (mut rows, mut row, mut cell): (Vec<String>, Vec<String>, String) = (vec![], vec![], String::new());
        let mut rest = &text[start..end];
        while let Some(open) = rest.find('<') {
            cell.push_str(&rest[..open]);
            let Some(close) = rest[open..].find('>').map(|found| open + found) else { break };
            let tag = rest[open + 1..close].trim().to_ascii_lowercase();
            let name = tag.trim_start_matches('/').split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap_or("");
            match (tag.starts_with('/'), name) {
                (true, "td" | "th") => row.push(entities(&std::mem::take(&mut cell)).split_whitespace().collect::<Vec<_>>().join(" ")),
                (true, "tr") => {
                    if row.iter().any(|cell| !cell.is_empty()) { rows.push(row.join(" | ")) }
                    row.clear();
                }
                (false, "td" | "th") => cell.clear(),
                (false, "br") => cell.push(' '),
                _ => {}
            }
            rest = &rest[close + 1..];
        }
        out.push((start..end, rows.join("\n")));
        at = end;
    }
    out
}

/// A text with the few entities a table's cell is written with read.
fn entities(text: &str) -> String {
    text.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

/// The formulas of a text, as Markdown marks them: each whole, and what is between its marks. `$…$` on one
/// line (opening before a character that is no blank, closing after one and not before a digit, so that two
/// prices are no formula), `$$…$$`, `\(…\)` and `\[…\]`.
fn formulas(text: &str) -> Vec<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let bytes = text.as_bytes();
    let mut out = vec![];
    let mut at = 0;
    while at < bytes.len() {
        let rest = &text[at..];
        let escaped = at > 0 && bytes[at - 1] == b'\\';
        let found = if rest.starts_with("$$") && !escaped {
            rest[2..].find("$$").map(|end| (2, end + 2, 2))
        } else if rest.starts_with('$') && !escaped {
            let opens = rest[1..].chars().next().is_some_and(|c| !c.is_whitespace() && c != '$');
            let line = rest[1..].find('\n').map_or(rest.len(), |end| end + 1);
            let closes = |end: &usize| !rest[1..*end].ends_with(char::is_whitespace) && !rest[1..*end].ends_with('\\') && !rest[end + 1..].starts_with(|c: char| c.is_ascii_digit());
            if opens { rest[1..line].match_indices('$').map(|(end, _)| end + 1).find(closes).map(|end| (1, end, 1)) } else { None }
        } else if rest.starts_with("\\(") && !escaped {
            rest.find("\\)").map(|end| (2, end, 2))
        } else if rest.starts_with("\\[") && !escaped {
            rest.find("\\]").map(|end| (2, end, 2))
        } else {
            None
        };
        match found {
            Some((open, end, close)) => {
                out.push((at..at + end + close, at + open..at + end));
                at += end + close;
            }
            None => at += rest.chars().next().map_or(1, char::len_utf8),
        }
    }
    out
}

/// The signs a formula names that a text simply writes.
const SIGNS: [(&str, &str); 66] = [
    ("alpha", "α"), ("beta", "β"), ("gamma", "γ"), ("delta", "δ"), ("epsilon", "ε"), ("varepsilon", "ε"), ("zeta", "ζ"), ("eta", "η"), ("theta", "θ"), ("iota", "ι"), ("kappa", "κ"),
    ("lambda", "λ"), ("mu", "μ"), ("nu", "ν"), ("xi", "ξ"), ("pi", "π"), ("rho", "ρ"), ("sigma", "σ"), ("tau", "τ"), ("upsilon", "υ"), ("phi", "φ"), ("varphi", "φ"), ("chi", "χ"),
    ("psi", "ψ"), ("omega", "ω"), ("Gamma", "Γ"), ("Delta", "Δ"), ("Theta", "Θ"), ("Lambda", "Λ"), ("Xi", "Ξ"), ("Pi", "Π"), ("Sigma", "Σ"), ("Phi", "Φ"), ("Psi", "Ψ"), ("Omega", "Ω"),
    ("pm", "±"), ("mp", "∓"), ("times", "×"), ("cdot", "·"), ("div", "÷"), ("leq", "≤"), ("le", "≤"), ("geq", "≥"), ("ge", "≥"), ("neq", "≠"), ("ne", "≠"), ("approx", "≈"), ("sim", "~"),
    ("equiv", "≡"), ("propto", "∝"), ("infty", "∞"), ("rightarrow", "→"), ("to", "→"), ("leftarrow", "←"), ("leftrightarrow", "↔"), ("degree", "°"), ("prime", "′"), ("ldots", "…"),
    ("dots", "…"), ("cdots", "…"), ("ll", "≪"), ("gg", "≫"), ("ell", "ℓ"), ("partial", "∂"), ("AA", "Å"), ("angstrom", "Å"),
];
/// The commands that set what is in their braces in another face: the words are the same.
const FACES: [&str; 10] = ["text", "textrm", "textit", "textbf", "mathrm", "mathit", "mathbf", "mathsf", "operatorname", "mbox"];

/// A formula as the text it stands for, when it says no more than figures, units and plain signs: `4\ ^\circ\text{C}`
/// is `4 °C`, `10^{-3}` is `10⁻³`, `\text{CO}_2` is `CO₂`. `None` for one that is built (a fraction, a sum, a
/// letter raised or lowered, braces of its own): that stays as the engine wrote it.
fn untex(tex: &str) -> Option<String> {
    let chars: Vec<char> = tex.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    // The command at `at` (after its backslash): its name and where it ends. A name is letters, or one other character.
    let command = |at: usize| -> Option<(String, usize)> {
        let first = *chars.get(at)?;
        if !first.is_ascii_alphabetic() { return Some((first.to_string(), at + 1)) }
        let end = (at..chars.len()).find(|i| !chars[*i].is_ascii_alphabetic()).unwrap_or(chars.len());
        Some((chars[at..end].iter().collect(), end))
    };
    // What stands in the braces opening at `at`, when it is plain: no command and no braces within.
    let braced = |at: usize| -> Option<(String, usize)> {
        if chars.get(at) != Some(&'{') { return None }
        let end = (at + 1..chars.len()).find(|i| chars[*i] == '}')?;
        let inner: String = chars[at + 1..end].iter().collect();
        (!inner.contains(['{', '\\'])).then_some((inner, end + 1))
    };
    while at < chars.len() {
        match chars[at] {
            '\\' => {
                let (name, end) = command(at + 1)?;
                at = end;
                match name.as_str() {
                    face if FACES.contains(&face) => {
                        let (words, end) = braced(at)?;
                        out.push_str(&words);
                        at = end;
                    }
                    " " | "," | ";" | ":" | "quad" | "qquad" => out.push(' '),
                    "!" | "left" | "right" => {}
                    "%" | "&" | "#" | "_" | "$" | "{" | "}" => out.push_str(&name),
                    sign => {
                        out.push_str(SIGNS.iter().find(|(known, _)| *known == sign)?.1);
                    }
                }
            }
            mark @ ('^' | '_') => {
                // What is raised or lowered: braces, a command, or one character.
                let (raised, end) = match chars.get(at + 1)? {
                    '{' => {
                        let end = (at + 2..chars.len()).find(|i| chars[*i] == '}')?;
                        (chars[at + 2..end].iter().collect::<String>(), end + 1)
                    }
                    '\\' => {
                        let (name, end) = command(at + 2)?;
                        (format!("\\{name}"), end)
                    }
                    one => (one.to_string(), at + 2),
                };
                at = end;
                match (mark, raised.trim()) {
                    ('^', "\\circ") => out.push('°'),
                    ('^', "\\prime") => out.push('′'),
                    (_, figures) if !figures.is_empty() => {
                        for figure in figures.chars() {
                            let table = if mark == '^' { "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻" } else { "₀₁₂₃₄₅₆₇₈₉₊₋" };
                            let index = "0123456789+-".find(if figure == '−' { '-' } else { figure })?;
                            out.push(table.chars().nth(index)?);
                        }
                    }
                    _ => return None,
                }
            }
            '{' | '}' | '&' => return None,
            '~' => {
                out.push(' ');
                at += 1;
            }
            plain => {
                out.push(plain);
                at += 1;
            }
        }
    }
    let said = out.split_whitespace().collect::<Vec<_>>().join(" ");
    (!said.is_empty()).then_some(said)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;

    fn settings_with(engine: &str, keys: &[(&str, &str)]) -> Settings {
        let mut settings = Settings::default();
        settings.providers.clear();
        settings.ocr = OcrSettings { engine: engine.into(), keys: keys.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() };
        settings
    }

    /// `ready` with the environment given, not the process's.
    fn ready(settings: &Settings, asked: &str, env: &[(&str, &str)]) -> Result<Ready, String> {
        let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        ready_from(settings, asked, &|name| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
    }

    #[test]
    fn the_engine_comes_from_the_call_the_settings_or_the_first_that_can_be_asked() {
        // A stored key, the variable, and nothing at all.
        let stored = settings_with(AUTO, &[("deepseek", " dk ")]);
        let found = ready(&stored, "", &[]).unwrap();
        assert_eq!((found.engine.key, found.key.as_str(), found.base.as_str()), ("deepseek", "dk", ""));
        assert_eq!(ready(&settings_with(AUTO, &[]), "", &[("DEEPSEEK_API_KEY", "env")]).unwrap().key, "env");
        let none = ready(&settings_with(AUTO, &[]), "", &[]).unwrap_err();
        assert!(none.contains("设置 › 网络 › 文字识别") && none.contains("能看图的模型"), "{none}");
        // Asked by name, whatever settings chose; an unknown name says which there are.
        assert!(ready(&settings_with(MODEL, &[]), "DeepSeek", &[]).unwrap_err().contains("DEEPSEEK_API_KEY"));
        assert!(ready(&stored, "tesseract", &[]).unwrap_err().contains("deepseek、model"));
        // Switched off: said so, unless an engine is named.
        let off = settings_with(OFF, &[("deepseek", "dk")]);
        assert!(ready(&off, "", &[]).unwrap_err().contains("关闭"));
        assert_eq!(ready(&off, "deepseek", &[]).unwrap().engine.key, "deepseek");
        // A model that can be asked is the second engine, and the one taken when chosen.
        let mut seeing = settings_with(AUTO, &[]);
        seeing.add_provider(Provider { name: "视觉".into(), base_url: "https://example.com/v1".into(), api_key: "k".into(), models: vec![crate::provider::Model { model: "v".into(), ..Default::default() }], ..Default::default() });
        let found = ready(&seeing, "", &[]).unwrap();
        assert_eq!((found.engine.key, found.author(&seeing).as_str()), (MODEL, "v · 视觉"));
        assert_eq!(ready(&seeing, "", &[("DEEPSEEK_API_KEY", "env")]).unwrap().engine.key, "deepseek");
        seeing.ocr.engine = MODEL.into();
        assert_eq!(ready(&seeing, "", &[("DEEPSEEK_API_KEY", "env")]).unwrap().engine.key, MODEL);
        assert!(ready(&settings_with(MODEL, &[("deepseek", "dk")]), "", &[]).unwrap_err().contains("设置 › AI"));
    }

    #[test]
    fn deepseek_borrows_the_model_services_key_proxy_and_host() {
        let mut settings = settings_with("deepseek", &[]);
        settings.providers = vec![Provider { preset: "deepseek".into(), base_url: "http://127.0.0.1:8080/anthropic".into(), api_key: "sk-ds".into(), proxy: "direct".into(), ..Default::default() }];
        let found = ready(&settings, "", &[]).unwrap();
        assert_eq!((found.key.as_str(), found.proxy.as_str(), found.author(&settings).as_str()), ("sk-ds", "direct", "DeepSeek OCR"));
        assert_eq!(request(&found, "a.pdf", b"%PDF").unwrap().url, "http://127.0.0.1:8080/v1/tools/ocr");
        // One stored for 文字识别 comes first, at DeepSeek's own address through the global proxy.
        settings.ocr.keys.insert("deepseek".into(), "sk-ocr".into());
        let found = ready(&settings, "", &[]).unwrap();
        assert_eq!((found.key.as_str(), found.proxy.as_str()), ("sk-ocr", ""));
        assert_eq!(request(&found, "a.pdf", b"%PDF").unwrap().url, "https://api.deepseek.com/v1/tools/ocr");
    }

    #[test]
    fn a_file_goes_up_as_a_form() {
        let deepseek = Ready { engine: engine("deepseek").unwrap(), key: "k".into(), proxy: "direct".into(), base: String::new() };
        let upload = request(&deepseek, "扫描 的\"论文\".pdf", b"%PDF-1.7\r\nbytes").unwrap();
        assert!(upload.headers.contains(&("Authorization".into(), "Bearer k".into())) && upload.proxy == "direct");
        let kind = &upload.headers.iter().find(|(name, _)| name == "Content-Type").unwrap().1;
        let boundary = kind.strip_prefix("multipart/form-data; boundary=").unwrap();
        let body = String::from_utf8(upload.body).unwrap();
        // The two fields the service takes, and no other; a name the header can carry.
        assert_eq!(body, format!("--{boundary}\r\nContent-Disposition: form-data; name=\"purpose\"\r\n\r\ntext_recognition\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"________.pdf\"\r\nContent-Type: application/octet-stream\r\n\r\n%PDF-1.7\r\nbytes\r\n--{boundary}--\r\n"));
        assert!(String::from_utf8(request(&deepseek, "论文", b"x").unwrap().body).unwrap().contains("filename=\"file.pdf\""));
        // What the service would refuse is said before the file is sent.
        let large = vec![0u8; (100 << 20) + 1];
        let refused = request(&deepseek, "big.pdf", &large).unwrap_err();
        assert!(refused.contains("100 MB") && refused.contains("101 MB") && refused.contains("压缩 PDF"), "{refused}");
        let model = Ready { engine: engine(MODEL).unwrap(), key: String::new(), proxy: String::new(), base: String::new() };
        assert!(request(&model, "a.pdf", b"x").unwrap_err().contains("图片"));
    }

    #[test]
    fn an_answer_read_page_by_page() {
        let deepseek = engine("deepseek").unwrap();
        // The service keeps the connection with line breaks before the answer; pages come in their order whatever the list's.
        let body = "\n\n\n{\"id\": \"ocr-1\", \"object\": \"ocr.result\", \"created\": 1, \"purpose\": \"text_recognition\", \"page_count\": 2, \"pages\": [\
            {\"index\": 1, \"text\": \"second page\", \"finish_reason\": \"length\"},\
            {\"index\": 0, \"text\": \"<｜ref｜>标题<｜/ref｜><｜box｜>[[52, 40, 948, 96]]<｜/box｜>\\n<｜ref｜>第一段。<｜/ref｜><｜box｜>[[52, 120, 948, 300]]<｜/box｜>\", \"finish_reason\": \"stop\"}], \"usage\": {\"input_tokens\": 1, \"output_tokens\": 2, \"total_tokens\": 3}}";
        let pages = read(deepseek, 200, body).unwrap();
        assert_eq!((pages[0].text.as_str(), pages[0].cut), ("标题\n\n第一段。", false));
        assert!(pages[0].raw.starts_with("<｜ref｜>标题"));
        assert_eq!((pages[1].text.as_str(), pages[1].cut), ("second page", true));
        assert_eq!(Reading { engine: deepseek, pages }.cut(), vec![2]);
        assert_eq!(read(deepseek, 200, r#"{"pages": []}"#).unwrap(), vec![]);
        // A failure after it began comes with 200; one before, with its status. Both in the service's words.
        let late = read(deepseek, 200, "\n\n{\"error\": {\"message\": \"OCR did not finish in time\", \"type\": \"gateway_timeout_error\", \"param\": null, \"code\": \"gateway_timeout_error\"}}").unwrap_err();
        assert_eq!(late, "DeepSeek OCR：没有在限定的时间里读完，页数多的文件可以稍后再试：OCR did not finish in time");
        assert_eq!(read(deepseek, 402, r#"{"error": {"message": "Insufficient Balance"}}"#).unwrap_err(), "DeepSeek OCR：账户余额不足（HTTP 402）：Insufficient Balance");
        assert_eq!(read(deepseek, 429, "").unwrap_err(), "DeepSeek OCR：同时在读的文件太多，请稍后再试（HTTP 429）");
        assert!(read(deepseek, 400, "<html>bad</html>").unwrap_err().ends_with("：<html>bad</html>"));
        assert!(read(deepseek, 200, "<html>").unwrap_err().contains("不是 JSON"));
        assert!(read(deepseek, 200, r#"{"id": "ocr-1"}"#).unwrap_err().contains("无法识别"));
    }

    /// What DeepSeek answered for a scan of two pages (`purpose: text_recognition`, November 2026), as it came.
    const ANSWERED: [&str; 2] = [
        "# Root hair elongation under low phosphate in *Arabidopsis*\n\nZhang Wei, Li Na and Wang Fang\n\nInstitute of Plant Sciences\n\n## Abstract\n\nPhosphate is often the nutrient that limits plant growth. We measured root hair length in seedlings grown on media with $1\\text{ mM}$ or $10\\ \\mu\\text{M}$ phosphate and found that hairs were $2.4$ times longer under low phosphate ($\\text{p} < 0.001$).\n\n### 1. Introduction\n\nRoot hairs extend the absorbing surface of the root. Earlier work showed that their length responds to the supply of phosphate, iron and nitrate, yet the timing of that response has rarely been followed in single seedlings. Here we image the same roots every hour for seven days.\n\nThe transcription factor RSL4 sets the final length of a hair cell, and its abundance rises when phosphate is scarce [1, 2].\n\n### 2. Methods\n\nSeeds of Col-0 were sterilised, kept at $4\\ ^\\circ\\text{C}$ for two days and sown on half-strength MS plates. Plates stood upright at $22\\ ^\\circ\\text{C}$ under $16\\text{ h}$ of light. Length was measured in Fiji from the base of each hair to its tip; the growth rate is $\\text{v} = \\Delta\\text{L} / \\Delta\\text{t}$.\n\n中文摘要：低磷条件下，拟南芥根毛明显伸长，平均长度为对照的 $2.4$ 倍。",
        "### 3. Results\n\nHairs on low phosphate grew for longer, not faster: the rate over the first six hours was the same in both media, and growth stopped after 9 h on high phosphate and after\n\n21 h on low phosphate.\n\nTable 1 gives the mean length on day seven for three replicates.\n\n<table border=1><tr><td>Medium</td><td>Replicates</td><td>Length (μm)</td></tr><tr><td>1 mM phosphate</td><td>3</td><td>312 ± 28</td></tr><tr><td>10 μM phosphate</td><td>3</td><td>751 ± 64</td></tr></table>\n\n### 4. Discussion\n\nThe longer period of growth agrees with a model in which RSL4 is degraded more slowly when phosphate is scarce. Whether the same holds for iron remains to be tested.\n\n[1] Yi K. et al. (2010) A basic helix-loop-helix transcription factor controls cell growth and size in root hairs. Nat. Genet. 42, 264–267.\n\n[2] Datta S. et al. (2015) Intensity of a pulse of RSL4 transcription factor synthesis determines Arabidopsis root hair cell size. Nat. Plants 1, 15138.\n\n## References",
    ];

    #[test]
    fn what_the_service_writes_becomes_the_text_of_a_page() {
        let pages: Vec<String> = ANSWERED.iter().map(|raw| plain(raw)).collect();
        // Headings without their marks, each alone; figures and units as a text writes them.
        assert!(pages[0].starts_with("Root hair elongation under low phosphate in Arabidopsis\n\nZhang Wei, Li Na and Wang Fang\n\nInstitute of Plant Sciences\n\nAbstract\n\nPhosphate is often"), "{}", pages[0]);
        assert!(pages[0].contains("grown on media with 1 mM or 10 μM phosphate and found that hairs were 2.4 times longer under low phosphate (p < 0.001).\n\n1. Introduction\n\nRoot hairs extend"), "{}", pages[0]);
        assert!(pages[0].contains("kept at 4 °C for two days") && pages[0].contains("upright at 22 °C under 16 h of light") && pages[0].contains("the growth rate is v = ΔL / Δt."), "{}", pages[0]);
        assert!(pages[0].ends_with("平均长度为对照的 2.4 倍。") && !pages[0].contains('$') && !pages[0].contains('#') && !pages[0].contains('*'), "{}", pages[0]);
        // A table as its rows.
        assert!(pages[1].contains("for three replicates.\n\nMedium | Replicates | Length (μm)\n1 mM phosphate | 3 | 312 ± 28\n10 μM phosphate | 3 | 751 ± 64\n\n4. Discussion\n\nThe longer period"), "{}", pages[1]);
        assert!(pages[1].starts_with("3. Results\n\nHairs on low phosphate") && pages[1].ends_with("15138.\n\nReferences") && !pages[1].contains('<'), "{}", pages[1]);
        // With the marks gone the headings name the parts of the paper, which is what the text is sorted by.
        let made = crate::structure::by_rule(&pages, "");
        let part = |words: &str| made.sentences.iter().find(|sentence| sentence.text.contains(words)).map(|sentence| sentence.section.as_str());
        assert_eq!((part("Root hairs extend"), part("Seeds of Col-0"), part("grew for longer"), part("agrees with a model")), (Some("introduction"), Some("methods"), Some("results"), Some("discussion")));
        assert!(made.sentences.iter().any(|sentence| sentence.text == "Seeds of Col-0 were sterilised, kept at 4 °C for two days and sown on half-strength MS plates."));
    }

    #[test]
    fn a_formula_that_is_only_figures_and_units_is_text() {
        for (tex, said) in [("2.4", "2.4"), ("1\\text{ mM}", "1 mM"), ("10\\ \\mu\\text{M}", "10 μM"), ("4\\ ^\\circ\\text{C}", "4 °C"), ("37^{\\circ}\\mathrm{C}", "37°C"), ("\\text{p} < 0.001", "p < 0.001"), ("10^{-3}", "10⁻³"), ("\\text{CO}_2", "CO₂"), ("\\text{Ca}^{2+}", "Ca²⁺"), ("5\\%", "5%"), ("312 \\pm 28", "312 ± 28"), ("\\sim 3\\,\\text{kb}", "~ 3 kb"), ("n = 3", "n = 3"), ("\\alpha\\text{-helix}", "α-helix"), ("\\leq 0.05", "≤ 0.05"), ("3' \\rightarrow 5'", "3' → 5'")] {
            assert_eq!(untex(tex).as_deref(), Some(said), "{tex}");
        }
        // One that is built stays as it was written.
        for tex in ["\\frac{a}{b}", "x_i", "e^{x}", "\\sum_{i=1}^{n} x_i", "\\sqrt{2}", "{a}", "a & b", "\\text{a{b}}", "\\unknown", ""] {
            assert_eq!(untex(tex), None, "{tex}");
        }
        assert_eq!(plain("The rate is $\\frac{\\Delta L}{\\Delta t}$ at $25\\ ^\\circ\\text{C}$, with $x_i * y_i$ and $a_1 * b_1$."), "The rate is $\\frac{\\Delta L}{\\Delta t}$ at 25 °C, with $x_i * y_i$ and a₁ * b₁.");
        assert_eq!(plain("$$E = mc^2$$\n\n\\(5\\,\\text{mM}\\) and \\[\\int_0^1 f\\]"), "E = mc²\n\n5 mM and \\[\\int_0^1 f\\]");
        // Money is no formula; nor is a mark that never closes.
        assert_eq!(plain("It cost $5 and then $10 more; from $5–$10. An open $x and a \\$ sign."), "It cost $5 and then $10 more; from $5–$10. An open $x and a $ sign.");
    }

    #[test]
    fn a_paragraph_cut_by_the_end_of_a_column_is_one_again() {
        let long = "Hairs on low phosphate grew for longer, not faster, and growth stopped after 9 h on high phosphate and after";
        assert_eq!(plain(&format!("### 3. Results\n\n{long}\n\n21 h on low phosphate.\n\nTable 1 gives the mean.")), format!("3. Results\n\n{long} 21 h on low phosphate.\n\nTable 1 gives the mean."));
        assert_eq!(plain(&format!("{long},\n\nwhich was expected.")), format!("{long}, which was expected."));
        // Not across a heading, an item or a table, not after a sentence's end, and not for a short line such as a title's.
        for apart in [format!("{long}\n\n## methods\n\nseeds were sown."), format!("{long}\n\n1. first item"), format!("{long}\n\n- an item"), format!("{long}.\n\nand so on."), format!("{long}\n\nA new sentence."), "Root hair growth\n\nand phosphate".to_string(), format!("{long}\n\n<table><tr><td>a</td></tr></table>")] {
            assert_eq!(plain(&apart).matches("\n\n").count(), apart.matches("\n\n").count(), "{apart}");
        }
        // Chinese has no small letters: a paragraph that stops in the middle of a sentence goes on.
        let zh = "低磷条件下，拟南芥根毛明显伸长，平均长度为对照的两倍以上，这一结果在三次独立重复中";
        assert_eq!(plain(&format!("{zh}\n\n都得到了验证。\n\n## 讨论\n\n根毛")), format!("{zh}都得到了验证。\n\n讨论\n\n根毛"));
        assert_eq!(plain("摘要\n\n低磷条件下根毛伸长。"), "摘要\n\n低磷条件下根毛伸长。");
    }

    #[test]
    fn a_table_becomes_its_rows() {
        let html = "Before.\n<TABLE class=\"x\">\n<tr><th>Gene</th><th colspan=\"2\">Fold&nbsp;change</th></tr>\n<tr><td>RSL4</td><td>2.4<br>(n = 3)</td><td>p &lt; 0.05 &amp; up</td></tr>\n<tr><td></td><td> </td></tr>\n</TABLE>\nAfter *that*.";
        assert_eq!(plain(html), "Before.\n\nGene | Fold change\nRSL4 | 2.4 (n = 3) | p < 0.05 & up\n\nAfter that.");
        // One that never ends is left as it stands.
        assert_eq!(tables("<table><tr><td>a").len(), 0);
    }

    #[test]
    fn marks_of_place_are_read_and_taken_out() {
        let raw = "<｜ref｜>Root growth<｜/ref｜><｜box｜>[[52, 40, 948, 96]]<｜/box｜><｜ref｜>Roots grow\ndownwards.<｜/ref｜><｜box｜>[[52, 120, 500, 300], [520, 120, 948, 200]]<｜/box｜>\nA line without a place.  \n\n\n\nAnother.";
        assert_eq!(plain(raw), "Root growth\n\nRoots grow\ndownwards.\n\nA line without a place.\n\nAnother.");
        let found = blocks(raw);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].text.as_str(), found[0].rect), ("Root growth", [0.052, 0.04, 0.948, 0.096]));
        // Several boxes of one text: the rectangle around them.
        assert_eq!(found[1].rect, [0.052, 0.12, 0.948, 0.3]);
        // Plain bars and `det`, as the model behind the service also writes them; a text without a place stays text.
        let other = "<|ref|>Title<|/ref|><|det|>[[0, 0, 500, 100]]<|/det|>\nbody <|ref|>loose<|/ref|> tail";
        assert_eq!(plain(other), "Title\n\nbody\n\nloose\n\ntail");
        assert_eq!(blocks(other), vec![Block { text: "Title".into(), rect: [0., 0., 0.5, 0.1] }]);
        // Nothing marked: the text as it is, tidied; an angle bracket of the text's own is no mark.
        assert_eq!(plain("a < b\n\n\nc <｜ref｜>unclosed"), "a < b\n\nc <｜ref｜>unclosed");
        assert!(blocks("plain text").is_empty() && plain("").is_empty());
        // A box that is no box gives the text no place.
        assert!(blocks("<｜ref｜>x<｜/ref｜><｜box｜>[[1, 2, 3]]<｜/box｜>").is_empty());
        assert_eq!(plain("<｜ref｜>x<｜/ref｜><｜box｜>[[1, 2, 3]]<｜/box｜>"), "x");
    }

    /// A PDF of one line of text a page.
    fn pages_saying(lines: &[&str]) -> Vec<u8> {
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font = doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica"});
        let kids: Vec<Object> = lines.iter().map(|line| {
            let content = doc.add_object(Stream::new(dictionary! {}, format!("BT /F1 12 Tf 72 720 Td ({line}) Tj ET").into_bytes()));
            doc.add_object(dictionary! {"Type" => "Page", "Parent" => pages_id, "Contents" => content}).into()
        }).collect();
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => lines.len() as i64, "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}}));
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
        doc.trailer.set("Root", catalog);
        let mut bytes = vec![];
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn pages_left_empty_are_read_again_and_put_back_in_their_places() {
        use crate::store::StructureInfo;
        let pages: Vec<String> = ["Roots", "", "  ", "Shoots"].map(String::from).to_vec();
        // Only of a file whose pages were read off them, and read to the end.
        let read_by = |text_by: &str, pages_read: usize| StructureInfo { text_by: text_by.into(), pages_read, page_count: 4, ..Default::default() };
        assert_eq!(blank_pages(Some(&read_by("DeepSeek OCR", 4)), &pages), vec![1, 2]);
        assert!(blank_pages(Some(&read_by("", 0)), &pages).is_empty());
        assert!(blank_pages(Some(&read_by("DeepSeek OCR", 2)), &pages).is_empty());
        assert!(blank_pages(None, &pages).is_empty());
        // The pages sent are those pages alone, in their order.
        let pdf = pages_saying(&["Roots", "Petioles", "Stipules", "Shoots"]);
        let sent = only_pages(&pdf, &[1, 2]).unwrap();
        let read = crate::fulltext::pdf_pages(&sent).unwrap();
        assert_eq!(read.len(), 2);
        assert!(read[0].contains("Petioles") && read[1].contains("Stipules"), "{read:?}");
        assert!(only_pages(&pdf, &[9]).is_err() && only_pages(b"not a pdf", &[0]).is_err());
        // What comes back takes their places; a page still empty keeps what it had, and so does its raw answer.
        let page = |text: &str| Page { text: text.into(), raw: format!("<{text}>"), cut: false };
        let raw: Vec<String> = ["<Roots>", "", "", "<Shoots>"].map(String::from).to_vec();
        let whole = refill(&pages, &raw, &[1, 2], vec![page("Petioles"), page("")]).unwrap();
        assert_eq!(whole.iter().map(|page| page.text.as_str()).collect::<Vec<_>>(), ["Roots", "Petioles", "  ", "Shoots"]);
        assert_eq!((whole[0].raw.as_str(), whole[1].raw.as_str(), whole[3].raw.as_str()), ("<Roots>", "<Petioles>", "<Shoots>"));
        // The file sent whole: the pages asked for are taken from it, the others stay. A file no engine read has no raw answers.
        let whole = refill(&pages, &[], &[2], vec![page("x"), page("y"), page("Stipules"), page("z")]).unwrap();
        assert_eq!(whole.iter().map(|page| page.text.as_str()).collect::<Vec<_>>(), ["Roots", "", "Stipules", "Shoots"]);
        assert_eq!(whole[0].raw, "");
        assert!(refill(&pages, &raw, &[1, 2], vec![page("a"), page("b"), page("c")]).is_err());
    }
}
