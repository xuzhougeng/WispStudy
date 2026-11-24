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
    if numbers.len() < 4 || numbers.len() % 4 != 0 { return None }
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

/// The text of a page without its marks of place. A piece with a place is a block of the page (a heading, a
/// paragraph), so a blank line stands between two of them, as the pages of a PDF that gives its own text have it.
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
    // Lines without spaces at their ends, and no more than one blank line between two.
    let mut tidy = String::with_capacity(out.len());
    let mut blank = 0;
    for line in out.lines().map(str::trim_end) {
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
}
