//! 网页搜索: the web searched through a service that answers in JSON, for
//! what the literature databases do not hold (a software's documentation, a
//! guideline, an institution's page, news). Every engine is one entry of
//! `ENGINES` and an arm in `request` and `read`; nothing else knows which
//! engines there are. Nothing here sends a request: `wisp_net::web_search`
//! does, and the tool `web_search` is handed it (`tools::Reach`).

use crate::ai::HttpRequest;
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A service that searches the web.
#[derive(Debug, PartialEq)]
pub struct Engine {
    /// What settings and the tool's `engine` call it.
    pub key: &'static str,
    pub name: &'static str,
    /// The environment variable its key is read from when none is stored.
    pub key_env: &'static str,
    /// The most results one request returns.
    pub most: usize,
    /// The longest query it takes, in characters; 0 is no limit of its own.
    pub longest: usize,
    /// Where a key is had.
    pub site: &'static str,
}

/// The engines, in the order 自动 tries them. A new engine is a row here and an arm in `request` and `read`.
pub const ENGINES: [Engine; 3] = [
    // https://api-docs.deepseek.com/zh-cn/guides/search/: the key of DeepSeek's chat API.
    Engine { key: "deepseek", name: "DeepSeek", key_env: "DEEPSEEK_API_KEY", most: 50, longest: 128, site: "https://platform.deepseek.com/api_keys" },
    // https://github.com/jina-ai/reader (s.jina.ai): a key is required.
    Engine { key: "jina", name: "Jina", key_env: "JINA_API_KEY", most: 20, longest: 0, site: "https://jina.ai/api-dashboard" },
    // https://you.com/docs/api-reference/search/v1-search: 100 results without passages, 50 with them.
    Engine { key: "you", name: "You.com", key_env: "YDC_API_KEY", most: 50, longest: 0, site: "https://you.com/platform/api-keys" },
];

/// `WebSearchSettings::engine` for the first engine that has a key.
pub const AUTO: &str = "";
/// `WebSearchSettings::engine` for no web search at all: the assistant is not offered the tool.
pub const OFF: &str = "off";
/// How many results a search returns when the caller does not say.
pub const DEFAULT_COUNT: usize = 8;
/// The most of a page's text one result carries; the rest is cut and says so.
pub const CONTENT_CHARS: usize = 6000;
/// The most of a summary made of an engine's passages (You.com's `highlights`).
const SUMMARY_CHARS: usize = 1200;

/// 设置 › 网络 › 网页搜索.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WebSearchSettings {
    /// An `Engine::key`, `AUTO` or `OFF`.
    pub engine: String,
    /// The key stored for an engine, by `Engine::key`. DeepSeek's may be left out: the DeepSeek model service's is used.
    pub keys: BTreeMap<String, String>,
}

pub fn engine(key: &str) -> Option<&'static Engine> {
    ENGINES.iter().find(|engine| engine.key == key.trim().to_ascii_lowercase())
}

/// An engine with the key it is asked with and the proxy it is reached through.
#[derive(Clone, Debug, PartialEq)]
pub struct Ready {
    pub engine: &'static Engine,
    pub key: String,
    /// The proxy of the model service whose key it borrows (`Provider::proxy`); empty is the global one.
    pub proxy: String,
}

/// The key an engine would be asked with now, and the proxy that goes with it; an empty key when it has none.
/// Stored for web search first; for DeepSeek then the DeepSeek model service's (stored or `DEEPSEEK_API_KEY`); then the engine's variable.
pub fn key_for(settings: &Settings, engine: &'static Engine) -> (String, String) {
    key_from(settings, engine, &|name| std::env::var(name).ok())
}

fn key_from(settings: &Settings, engine: &'static Engine, env: &dyn Fn(&str) -> Option<String>) -> (String, String) {
    let stored = settings.web_search.keys.get(engine.key).map(|key| key.trim()).unwrap_or("");
    if !stored.is_empty() { return (stored.to_string(), String::new()) }
    if engine.key == "deepseek" && let Some(provider) = settings.providers.iter().find(|p| p.preset == "deepseek" || p.base_url.contains("api.deepseek.com")) {
        let key = provider.key();
        if !key.is_empty() { return (key, provider.proxy.clone()) }
    }
    (env(engine.key_env).map(|key| key.trim().to_string()).unwrap_or_default(), String::new())
}

/// The engine a search goes to: `asked` (an `Engine::key`) when given, else the one chosen in settings, else
/// the first with a key. The error is a sentence for the person or the model, saying where a key goes.
pub fn ready(settings: &Settings, asked: &str) -> Result<Ready, String> {
    ready_from(settings, asked, &|name| std::env::var(name).ok())
}

fn ready_from(settings: &Settings, asked: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Ready, String> {
    let chosen = settings.web_search.engine.trim();
    if asked.trim().is_empty() && chosen == OFF { return Err("网页搜索已在「设置 › 网络」关闭。".into()) }
    let wanted = if asked.trim().is_empty() { chosen } else { asked.trim() };
    if !wanted.is_empty() {
        let Some(engine) = engine(wanted) else { return Err(format!("没有这个网页搜索引擎：{wanted}。可用的有 {}。", names())) };
        let (key, proxy) = key_from(settings, engine, env);
        if key.is_empty() { return Err(format!("{} 网页搜索需要 API Key：在「设置 › 网络 › 网页搜索」填写，或设置环境变量 {}（在 {} 申请）。", engine.name, engine.key_env, engine.site)) }
        return Ok(Ready { engine, key, proxy });
    }
    ENGINES.iter().find_map(|engine| {
        let (key, proxy) = key_from(settings, engine, env);
        (!key.is_empty()).then_some(Ready { engine, key, proxy })
    })
    .ok_or_else(|| format!("网页搜索需要 {} 之一的 API Key：在「设置 › 网络 › 网页搜索」填写；DeepSeek 也可以用「设置 › AI」里 DeepSeek 服务的 Key。", names()))
}

fn names() -> String {
    ENGINES.iter().map(|engine| engine.key).collect::<Vec<_>>().join("、")
}

/// One search as asked.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub query: String,
    /// How many results, 1 to the engine's `most`.
    pub count: usize,
    /// Whether each result brings the page's text, not only a summary.
    pub content: bool,
}

/// One result of a search.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub title: String,
    pub url: String,
    /// What the engine says of the page with the query in mind.
    pub summary: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub site: String,
    /// When the page was published, as the engine gives it; empty when it does not know.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub date: String,
    /// The page's text, at most `CONTENT_CHARS`; empty unless asked for.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,
}

/// What a search found, and through which engine.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub engine: &'static Engine,
    pub hits: Vec<Hit>,
}

/// The front end's way of searching: the engine asked for (empty: as settings say) and the query.
pub type Search<'a> = &'a dyn Fn(&str, &Query) -> Result<Found, String>;

/// The request for one search. Jina's is a GET: its `body` is null.
pub fn request(ready: &Ready, query: &Query) -> Result<HttpRequest, String> {
    let words = query.query.trim();
    if words.is_empty() { return Err("搜索词不能为空。".into()) }
    let engine = ready.engine;
    if engine.longest > 0 && words.chars().count() > engine.longest { return Err(format!("{} 的搜索词最多 {} 个字符，这次有 {} 个：请缩短。", engine.name, engine.longest, words.chars().count())) }
    let count = query.count.clamp(1, engine.most);
    let auth = ("Authorization".to_string(), format!("Bearer {}", ready.key));
    let proxy = ready.proxy.clone();
    Ok(match engine.key {
        "deepseek" => HttpRequest { url: "https://api.deepseek.com/v1/tools/search".into(), headers: vec![auth, ("Content-Type".into(), "application/json".into())], body: json!({"query": words, "count": count, "need_content": query.content}), proxy },
        "jina" => {
            let mut headers = vec![auth, ("Accept".into(), "application/json".into())];
            // Without it every result's page is fetched and read, which takes longer and costs more.
            if !query.content { headers.push(("X-Respond-With".into(), "no-content".into())) }
            HttpRequest { url: format!("https://s.jina.ai/?q={}&num={count}", crate::lookup::encode(words)), headers, body: Value::Null, proxy }
        }
        "you" => {
            // Without the page's text, the passages that speak to the query (free) make the summary; with it, the page as Markdown.
            let extraction = if query.content { json!({"extraction_mode": "full_page"}) } else { json!({"extraction_mode": "highlights", "highlights": {"total_max_tokens": 1024}}) };
            HttpRequest { url: "https://ydc-index.io/v1/search".into(), headers: vec![("X-API-Key".into(), ready.key.clone()), ("Content-Type".into(), "application/json".into())], body: json!({"query": words, "count": count, "extraction": extraction}), proxy }
        }
        other => return Err(format!("没有这个网页搜索引擎：{other}。")),
    })
}

/// The results in an engine's answer, or what went wrong in a sentence: `status` is the HTTP status.
pub fn read(engine: &Engine, status: u16, body: &str) -> Result<Vec<Hit>, String> {
    let value: Option<Value> = serde_json::from_str(body).ok();
    if !(200..300).contains(&status) {
        let said = value.as_ref().and_then(|v| v["error"]["message"].as_str().or(v["readableMessage"].as_str()).or(v["message"].as_str()).or(v["detail"].as_str()).or(v["error"].as_str())).map(str::trim).filter(|s| !s.is_empty()).map(String::from).unwrap_or_else(|| body.trim().chars().take(300).collect());
        let hint = match status {
            401 => "API Key 无效",
            402 => "账户余额不足",
            403 => "请求被拒绝",
            408 | 504 => "搜索超时，请稍后再试",
            429 => "请求过于频繁或额度不足，请稍后再试",
            500..=599 => "搜索服务暂时不可用，请稍后再试",
            _ => "搜索请求有误",
        };
        let said = if said.is_empty() { String::new() } else { format!("：{said}") };
        return Err(format!("{} 网页搜索：{hint}（HTTP {status}）{said}", engine.name));
    }
    let Some(value) = value else { return Err(format!("{} 网页搜索返回的不是 JSON。", engine.name)) };
    let text = |item: &Value, key: &str| item[key].as_str().unwrap_or("").trim().to_string();
    let items = match engine.key {
        "deepseek" => &value["results"],
        "jina" => &value["data"],
        "you" => &value["results"]["web"],
        _ => &Value::Null,
    };
    // An engine that found nothing may say so with an empty list or none at all.
    let Some(items) = items.as_array() else { return if items.is_null() { Ok(vec![]) } else { Err(format!("{} 网页搜索的返回内容无法识别。", engine.name)) } };
    Ok(items
        .iter()
        .map(|item| {
            let summary = match engine.key {
                // `summary` is the longer one, written for a model to read.
                "deepseek" => Some(text(item, "summary")).filter(|s| !s.is_empty()).unwrap_or_else(|| text(item, "snippet")),
                // The passages are about the query; `description` is the page's own, often empty.
                "you" => Some(passages(&item["contents"]["highlights"])).filter(|s| !s.is_empty()).or_else(|| Some(text(item, "description")).filter(|s| !s.is_empty())).unwrap_or_else(|| passages(&item["snippets"])),
                _ => text(item, "description"),
            };
            let site = text(item, if engine.key == "deepseek" { "site_name" } else { "siteName" });
            let date = ["date", "publishedTime", "page_age"].iter().map(|key| text(item, key)).find(|d| !d.is_empty()).unwrap_or_default();
            let content = if engine.key == "you" { item["contents"]["markdown"].as_str().unwrap_or("").trim().to_string() } else { text(item, "content") };
            Hit { title: text(item, "title"), url: text(item, "url"), summary, site, date, content: clipped(&content) }
        })
        .filter(|hit| !hit.url.is_empty())
        .collect())
}

/// A list of passages as one summary, at most `SUMMARY_CHARS`.
fn passages(list: &Value) -> String {
    let joined = list.as_array().map(|items| items.iter().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" … ")).unwrap_or_default();
    match joined.char_indices().nth(SUMMARY_CHARS) {
        Some((at, _)) => format!("{}…", &joined[..at]),
        None => joined,
    }
}

/// A page's text cut to `CONTENT_CHARS`, saying so where it was cut.
fn clipped(text: &str) -> String {
    match text.char_indices().nth(CONTENT_CHARS) {
        Some((at, _)) => format!("{}…（正文过长，只保留了前 {CONTENT_CHARS} 个字符）", &text[..at]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;

    fn settings_with(engine: &str, keys: &[(&str, &str)]) -> Settings {
        let mut settings = Settings::default();
        settings.providers.clear();
        settings.web_search = WebSearchSettings { engine: engine.into(), keys: keys.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() };
        settings
    }

    /// `ready` with the environment given, not the process's.
    fn ready(settings: &Settings, asked: &str, env: &[(&str, &str)]) -> Result<Ready, String> {
        let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        ready_from(settings, asked, &|name| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
    }

    #[test]
    fn the_engine_comes_from_the_call_the_settings_or_the_first_with_a_key() {
        // A stored key is found; 自动 takes the first engine that has one, in the order of `ENGINES`.
        let jina = settings_with(AUTO, &[("jina", " jk ")]);
        let found = ready(&jina, "", &[]).unwrap();
        assert_eq!((found.engine.key, found.key.as_str()), ("jina", "jk"));
        assert_eq!(ready(&jina, "", &[("DEEPSEEK_API_KEY", "dk")]).unwrap().engine.key, "deepseek");
        // Asked by name, whatever settings chose; one without a key says where a key goes; an unknown name says which there are.
        assert!(ready(&jina, "DeepSeek", &[]).unwrap_err().contains("DEEPSEEK_API_KEY"));
        assert_eq!(ready(&jina, "deepseek", &[("DEEPSEEK_API_KEY", "dk")]).unwrap().key, "dk");
        assert!(ready(&jina, "google", &[]).unwrap_err().contains("deepseek、jina、you"));
        assert_eq!(ready(&settings_with(AUTO, &[]), "", &[("YDC_API_KEY", "yk")]).unwrap().engine.key, "you");
        assert!(ready(&settings_with(AUTO, &[]), "", &[]).unwrap_err().contains("设置 › 网络"));
        // The engine chosen in settings, even with another's key at hand.
        assert!(ready(&settings_with("deepseek", &[("jina", "jk")]), "", &[]).is_err());
        assert_eq!(ready(&settings_with("jina", &[]), "", &[("JINA_API_KEY", "env")]).unwrap().key, "env");
        // Switched off: the tool says where it is switched on, unless an engine is named.
        let off = settings_with(OFF, &[("jina", "jk")]);
        assert!(ready(&off, "", &[]).unwrap_err().contains("关闭"));
        assert_eq!(ready(&off, "jina", &[]).unwrap().engine.key, "jina");
    }

    #[test]
    fn deepseek_borrows_the_model_services_key_and_proxy() {
        let mut settings = settings_with("deepseek", &[]);
        settings.providers = vec![Provider { preset: "deepseek".into(), base_url: "https://api.deepseek.com".into(), api_key: "sk-ds".into(), proxy: "direct".into(), ..Default::default() }];
        let found = ready(&settings, "", &[]).unwrap();
        assert_eq!((found.key.as_str(), found.proxy.as_str()), ("sk-ds", "direct"));
        // One stored for web search comes first, through the global proxy.
        settings.web_search.keys.insert("deepseek".into(), "sk-search".into());
        let found = ready(&settings, "", &[]).unwrap();
        assert_eq!((found.key.as_str(), found.proxy.as_str()), ("sk-search", ""));
    }

    #[test]
    fn requests_as_each_engine_wants_them() {
        let query = Query { query: " CRISPR 脱靶 site:nature.com ".into(), count: 80, content: false };
        let deepseek = request(&Ready { engine: engine("deepseek").unwrap(), key: "k".into(), proxy: String::new() }, &query).unwrap();
        assert_eq!(deepseek.url, "https://api.deepseek.com/v1/tools/search");
        assert_eq!(deepseek.body, json!({"query": "CRISPR 脱靶 site:nature.com", "count": 50, "need_content": false}));
        assert!(deepseek.headers.contains(&("Authorization".into(), "Bearer k".into())));
        let jina = request(&Ready { engine: engine("jina").unwrap(), key: "k".into(), proxy: "direct".into() }, &query).unwrap();
        assert_eq!(jina.url, "https://s.jina.ai/?q=CRISPR%20%E8%84%B1%E9%9D%B6%20site%3Anature.com&num=20");
        assert!(jina.body.is_null() && jina.proxy == "direct");
        assert!(jina.headers.contains(&("X-Respond-With".into(), "no-content".into())));
        let with_text = request(&Ready { engine: engine("jina").unwrap(), key: "k".into(), proxy: String::new() }, &Query { content: true, ..query }).unwrap();
        assert!(!with_text.headers.iter().any(|(name, _)| name == "X-Respond-With"));
        // DeepSeek's limit on a query is said before anything is sent.
        let long = Query { query: "词".repeat(129), count: 5, content: false };
        assert!(request(&Ready { engine: engine("deepseek").unwrap(), key: "k".into(), proxy: String::new() }, &long).unwrap_err().contains("128"));
        assert!(request(&Ready { engine: engine("jina").unwrap(), key: "k".into(), proxy: String::new() }, &long).is_ok());
        // You.com: the key in its own header; passages without the page's text, the page as Markdown with it.
        let you = Ready { engine: engine("you").unwrap(), key: "yk".into(), proxy: String::new() };
        let plain = request(&you, &Query { content: false, ..long.clone() }).unwrap();
        assert_eq!(plain.url, "https://ydc-index.io/v1/search");
        assert!(plain.headers.contains(&("X-API-Key".into(), "yk".into())) && !plain.headers.iter().any(|(name, _)| name == "Authorization"));
        assert_eq!(plain.body["extraction"]["extraction_mode"], "highlights");
        let full = request(&you, &Query { query: "q".into(), count: 99, content: true }).unwrap();
        assert_eq!(full.body, json!({"query": "q", "count": 50, "extraction": {"extraction_mode": "full_page"}}));
    }

    #[test]
    fn answers_read_into_one_shape() {
        let deepseek = engine("deepseek").unwrap();
        let body = r#"{"results": [{"rerank_score": 2.4, "title": "DeepSeek V4", "url": "https://example.com/v4", "snippet": "short", "summary": "long summary", "content": "", "site_name": "Example News", "site_icon": null, "date": "2026-06-01T00:00:00+00:00", "authority": 3},
            {"title": "No summary", "url": "https://example.com/2", "snippet": "only the snippet", "summary": "", "content": "", "site_name": null, "date": null}]}"#;
        let hits = read(deepseek, 200, body).unwrap();
        assert_eq!(hits[0], Hit { title: "DeepSeek V4".into(), url: "https://example.com/v4".into(), summary: "long summary".into(), site: "Example News".into(), date: "2026-06-01T00:00:00+00:00".into(), content: String::new() });
        assert_eq!((hits[1].summary.as_str(), hits[1].site.as_str()), ("only the snippet", ""));
        assert_eq!(read(deepseek, 200, r#"{"results": []}"#).unwrap(), vec![]);
        let jina = engine("jina").unwrap();
        let body = format!(r#"{{"code": 200, "status": 20000, "data": [{{"title": "Reader", "url": "https://jina.ai/reader", "description": "LLM-friendly input", "content": "{}", "date": "2024-05-01", "usage": {{"tokens": 9}}}}]}}"#, "x".repeat(CONTENT_CHARS + 10));
        let hits = read(jina, 200, &body).unwrap();
        assert_eq!((hits[0].summary.as_str(), hits[0].date.as_str()), ("LLM-friendly input", "2024-05-01"));
        assert!(hits[0].content.starts_with(&"x".repeat(CONTENT_CHARS)) && hits[0].content.ends_with("个字符）"));
        // Errors in each engine's words, with what the status means.
        let refused = read(deepseek, 402, r#"{"error": {"message": "Insufficient Balance", "type": "invalid_request_error", "param": null, "code": "invalid_request_error"}}"#).unwrap_err();
        assert_eq!(refused, "DeepSeek 网页搜索：账户余额不足（HTTP 402）：Insufficient Balance");
        let refused = read(jina, 401, r#"{"data": null, "code": 401, "name": "AuthenticationRequiredError", "status": 40103, "message": "Authentication is required", "readableMessage": "AuthenticationRequiredError: Authentication is required"}"#).unwrap_err();
        assert!(refused.starts_with("Jina 网页搜索：API Key 无效（HTTP 401）：AuthenticationRequiredError"));
        assert!(read(jina, 200, "<html>").unwrap_err().contains("不是 JSON"));
        let you = engine("you").unwrap();
        let body = r###"{"results": {"web": [{"url": "https://arxiv.org/html/2602.15217v1", "title": "Do we have a quantum computer?", "description": "", "page_age": "2026-02-16T21:26:51", "contents": {"highlights": ["# Abstract\nWhile experts agree", "## I Introduction\nThe NISQ era"]}, "favicon_url": "https://you.com/favicon?domain=arxiv.org"},
            {"url": "https://example.com/full", "title": "Full page", "description": "The page's own words", "contents": {"markdown": "# Full\ntext"}},
            {"url": "https://example.com/old", "title": "Snippets only", "snippets": ["one", "two"]}],
            "knowledge": [{"type": "answer", "title": "Overview", "attribution": []}]}, "metadata": {"query": "q"}}"###;
        let hits = read(you, 200, body).unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!((hits[0].summary.as_str(), hits[0].date.as_str()), ("# Abstract\nWhile experts agree … ## I Introduction\nThe NISQ era", "2026-02-16T21:26:51"));
        assert_eq!((hits[1].summary.as_str(), hits[1].content.as_str()), ("The page's own words", "# Full\ntext"));
        assert_eq!(hits[2].summary, "one … two");
        assert_eq!(read(you, 200, r#"{"results": {}}"#).unwrap(), vec![]);
        assert_eq!(read(you, 401, r#"{"detail": "Invalid API key"}"#).unwrap_err(), "You.com 网页搜索：API Key 无效（HTTP 401）：Invalid API key");
        assert!(read(you, 422, r#"{"error": "highlights supports count up to 50, got 80"}"#).unwrap_err().ends_with("got 80"));
    }
}
