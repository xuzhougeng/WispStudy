//! The network: metadata lookups, literature searches, the streamed chat
//! request and the update check. Shared by the app and `wisp`. Everything
//! here blocks, so the app runs it on the background executor. Beside the
//! network, what else a tool is lent that core cannot do by itself: a PDF's
//! pages as pictures (`pages`) and the system's own reading of them (`sight`).

pub mod mcp;
pub mod notes;
pub mod oauth;
pub mod pages;
pub mod sight;

use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::time::Duration;
use wisp_core::ai::{self, ChatEvent, HttpRequest, Protocol, TurnReader};
use wisp_core::asr;
use wisp_core::imagegen;
use wisp_core::lookup::{self, Identifier};
use wisp_core::model::Paper;
use wisp_core::ocr;
use wisp_core::sources::{self, Contact};
use wisp_core::update::{self, Release};
use wisp_core::websearch;

const USER_AGENT: &str = concat!("WispStudy/", env!("CARGO_PKG_VERSION"));

/// The proxy every request of this process goes through, from the settings.
static PROXY: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());

/// Set the global proxy (`http://…`, `socks5://…`); empty leaves the choice
/// to the environment (`ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY`).
pub fn set_proxy(address: &str) {
    *PROXY.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = address.trim().to_string();
}

/// Who the literature databases are told is asking, from the settings (`Settings::contact`).
static CONTACT: std::sync::RwLock<Contact> = std::sync::RwLock::new(Contact { email: String::new(), ncbi_key: String::new() });

/// Set who the literature databases are told is asking: from the next request on, `get_text` adds the
/// address and the NCBI key to what goes to them (`sources::identified`), and PubMed is asked at the pace
/// a key allows.
pub fn set_contact(contact: Contact) {
    *CONTACT.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = contact;
}

fn contact() -> Contact {
    CONTACT.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

/// Which proxy a request takes. `own` is the setting of whoever asks (a
/// model profile): an address of its own, `direct` for none at all, or empty
/// for the global one, which when empty too is the environment's.
fn proxy_for(own: &str) -> Result<Option<ureq::Proxy>, String> {
    let own = own.trim();
    if own.eq_ignore_ascii_case("direct") { return Ok(None) }
    let global = PROXY.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    let address = if own.is_empty() { global.as_str() } else { own };
    if address.is_empty() { return Ok(ureq::Proxy::try_from_env()) }
    ureq::Proxy::new(address).map(Some).map_err(|err| format!("代理地址 {address} 无法使用：{err}"))
}

fn agent(overall: Option<Duration>, proxy: &str) -> Result<ureq::Agent, String> {
    patient_agent(overall, proxy, Duration::from_secs(120))
}

/// An agent that waits `answer` for a response to begin.
fn patient_agent(overall: Option<Duration>, proxy: &str, answer: Duration) -> Result<ureq::Agent, String> {
    Ok(ureq::Agent::config_builder()
        // Error responses carry the reason in their body; read it rather than a bare status.
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(answer))
        .timeout_global(overall)
        .user_agent(USER_AGENT)
        .proxy(proxy_for(proxy)?)
        .build()
        .into())
}

fn network_error(err: ureq::Error) -> String {
    format!("网络请求失败：{err}")
}

/// How long a host is waited for to begin answering by a caller that has a time to keep.
const HURRIED_ANSWER: Duration = Duration::from_secs(30);

/// The agent for a request that may take `own` at the most. A caller with a time to keep (`sources::within`: a
/// pasted address) gets no more than what is left of it, and does not wait two minutes for a host that holds
/// the connection without a word; once the time is up nothing is asked.
fn timed_agent(own: Duration) -> Result<ureq::Agent, String> {
    match wisp_core::sources::time_left() {
        None => agent(Some(own), ""),
        Some(left) if left.is_zero() => Err(format!("{}。", wisp_core::sources::TIME_UP)),
        Some(left) => patient_agent(Some(own.min(left)), "", HURRIED_ANSWER.min(left)),
    }
}

/// Why a request to `host` failed, as `said` puts it; one that ended with its caller's time says that instead
/// of the timeout's own words.
fn cut_short(host: &str, said: String) -> String {
    if wisp_core::sources::time_up() { format!("{host} 迟迟没有答完，没有再等。") } else { said }
}

/// GET a document as text: what `wisp_core::sources` asks the literature
/// databases for (JSON from most, XML from PubMed and arXiv). A database that
/// wants to know who is asking is told (`set_contact`); the address `url` is
/// what errors name, never the one sent.
pub fn get_text(url: &str) -> Result<String, String> {
    let host = url.split('/').nth(2).unwrap_or(url);
    let contact = contact();
    let asked = sources::identified(url, &contact);
    // A service that says "too fast" is asked again, a little later each time.
    for wait in [Some(1500), Some(4000), None] {
        take_turn(host, !contact.ncbi_key.trim().is_empty());
        let mut response = timed_agent(Duration::from_secs(60))?.get(&asked).call().map_err(|err| cut_short(host, network_error(err)))?;
        let status = response.status().as_u16();
        match (status, wait) {
            (200..300, _) => return response.body_mut().with_config().limit(64 << 20).read_to_string().map_err(|e| cut_short(host, format!("无法读取 {host} 的返回内容：{e}"))),
            (429, Some(millis)) => std::thread::sleep(Duration::from_millis(millis)),
            (429, None) => break,
            // NCBI answers a key it does not know with 400, whatever was asked.
            (400, _) if sources::keyed(url, &contact) && response.body_mut().with_config().limit(1 << 16).read_to_string().is_ok_and(|body| body.to_lowercase().contains("api key") || body.contains("api-key")) => return Err(BAD_NCBI_KEY.into()),
            _ => return Err(format!("{host} 返回 HTTP {status}。")),
        }
    }
    Err(held_back(host, sources::more_allowed(url, &contact)))
}

/// How long an audio transcription upload may take.
const ASR_WAIT: Duration = Duration::from_secs(60);

/// Transcribe `audio` (named `name`) with the engine `asked` (empty: as settings say).
pub fn transcribe(asked: &str, name: &str, audio: &[u8]) -> Result<asr::Transcript, String> {
    let settings = wisp_core::settings::Settings::load(&wisp_core::settings::settings_file()).map_err(|e| format!("{e:#}"))?;
    let ready = asr::ready(&settings, asked)?;
    send_audio(&asr::request(&ready, name, audio)?, ready.engine)
}

/// Send an audio upload and wait for its transcript, retrying when the service is too busy to start.
pub fn send_audio(upload: &asr::Upload, engine: &'static asr::Engine) -> Result<asr::Transcript, String> {
    let agent = patient_agent(Some(ASR_WAIT), &upload.proxy, ASR_WAIT)?;
    let mut tries = 0;
    loop {
        let mut call = agent.post(&upload.url);
        for (name, value) in &upload.headers { call = call.header(name, value) }
        let last = tries >= RETRY_WAITS.len();
        match call.send(upload.body.as_slice()) {
            Ok(mut response) if last || !matches!(response.status().as_u16(), 429 | 500 | 502 | 503) => {
                let status = response.status().as_u16();
                let text = response.body_mut().with_config().limit(4 << 20).read_to_string().map_err(|e| format!("{} 语音输入的返回内容没有读完：{e}", engine.name))?;
                return asr::read(engine, status, &text);
            }
            Ok(_) => {}
            Err(err) if last || matches!(err, ureq::Error::Timeout(_)) => return Err(format!("{} 语音输入：{}", engine.name, network_error(err))),
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(RETRY_WAITS[tries]));
        tries += 1;
    }
}

/// Said when NCBI turns down the key of the settings.
const BAD_NCBI_KEY: &str = "NCBI 没有接受这个 API Key：请在「设置 › 网络 › 文献数据库」核对，或者清空它。";

/// What is said of a host that kept answering "too fast", with what would have it take more when there is such a thing.
fn held_back(host: &str, more: Option<&str>) -> String {
    sources::reasons(&[format!("{host} 限制了请求频率，请稍后再试"), more.unwrap_or_default().to_string()])
}

/// The most a fetched file may be: a paper with its figures is a few megabytes, a book a few dozen.
const MAX_DOWNLOAD: u64 = 120 << 20;

/// GET a file as bytes: what `find_pdf` asks the open-access addresses
/// for. Whether it is the PDF hoped for is the caller's to check; a page
/// that asks to log in comes back with 200 as well.
pub fn download(url: &str) -> Result<Vec<u8>, String> {
    let host = url.split('/').nth(2).unwrap_or(url);
    let mut response = timed_agent(Duration::from_secs(180))?.get(url).header("Accept", "application/pdf,*/*;q=0.8").call().map_err(|err| cut_short(host, network_error(err)))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) { return Err(format!("{host} 返回 HTTP {status}。")) }
    response.body_mut().with_config().limit(MAX_DOWNLOAD).read_to_vec().map_err(|e| cut_short(host, format!("无法读取 {host} 的返回内容：{e}")))
}

/// NCBI allows three requests a second without a key and ten with one,
/// counted across everything this process sends: searches running side by
/// side take turns, one every 400 ms, or every 110 ms when `keyed`. arXiv
/// asks for three seconds between two requests, which a search of several
/// pages has to keep. Other hosts are not held back.
fn take_turn(host: &str, keyed: bool) {
    static NEXT: std::sync::Mutex<[Option<std::time::Instant>; 2]> = std::sync::Mutex::new([None; 2]);
    let Some((slot, gap)) = [("ncbi.nlm.nih.gov", if keyed { 110 } else { 400 }), ("arxiv.org", 3000)].iter().enumerate().find_map(|(slot, (name, gap))| host.ends_with(name).then_some((slot, *gap))) else { return };
    let now = std::time::Instant::now();
    let at = {
        let mut next = NEXT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let at = next[slot].filter(|at| *at > now).unwrap_or(now);
        next[slot] = Some(at + Duration::from_millis(gap));
        at
    };
    std::thread::sleep(at.saturating_duration_since(now));
}

/// The paper behind a DOI or PMID (`lookup::find`): Crossref first for a
/// DOI, completed by Europe PMC; Europe PMC alone for a PMID.
pub fn lookup_paper(id: &Identifier) -> Result<Paper, String> {
    lookup::find(id, &get_text)
}

/// The newest published release, read off the redirect `url` answers with
/// (`update::latest_url()`); `None` when nothing has been published. Only
/// the redirect is read: no page or package is fetched.
pub fn latest_release(url: &str) -> Result<Option<Release>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).max_redirects(0).timeout_global(Some(Duration::from_secs(20))).user_agent(USER_AGENT).proxy(proxy_for("")?).build().into();
    let response = agent.get(url).call().map_err(network_error)?;
    let status = response.status().as_u16();
    match status {
        300..400 => Ok(response.headers().get("location").and_then(|v| v.to_str().ok()).and_then(update::release_from_location)),
        200..300 | 404 => Ok(None),
        _ => Err(format!("更新服务器返回 HTTP {status}")),
    }
}

/// Whether `url` answers the way requests now go: through the global proxy (`set_proxy`), else the
/// environment's, else directly. How long the answer took to begin. Any answer counts, whatever its status:
/// what is asked is whether there is a way there, which is what 设置 › 网络's 测试 shows.
pub fn try_reach(url: &str) -> Result<Duration, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).max_redirects(0).timeout_global(Some(Duration::from_secs(15))).user_agent(USER_AGENT).proxy(proxy_for("")?).build().into();
    let started = std::time::Instant::now();
    agent.get(url).call().map_err(network_error)?;
    Ok(started.elapsed())
}

/// The most a release's package may be; they are some tens of megabytes.
const MAX_PACKAGE: u64 = 512 << 20;

/// A release's package, checked: the file `name` of the folder `base`
/// (`update::files_url`), held against its line in that folder's
/// `SHA256SUMS`. A package that does not match is not handed on.
pub fn download_release(base: &str, name: &str) -> Result<Vec<u8>, String> {
    let base = base.trim_end_matches('/');
    let sums = get_text(&format!("{base}/SHA256SUMS"))?;
    let expected = update::checksum(&sums, name).ok_or_else(|| format!("发布的校验文件里没有 {name}。"))?;
    // GitHub can be slow to reach: the package gets half an hour.
    let mut response = agent(Some(Duration::from_secs(1800)), "")?.get(format!("{base}/{name}")).call().map_err(network_error)?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) { return Err(format!("更新服务器返回 HTTP {status}")) }
    let bytes = response.body_mut().with_config().limit(MAX_PACKAGE).read_to_vec().map_err(|e| format!("更新包没有下载完：{e}"))?;
    if update::sha256(&bytes) != expected { return Err("下载的更新包与发布的校验值不一致，已丢弃。".into()) }
    Ok(bytes)
}

/// How long to wait before each further try of a request the service could
/// not take: two more tries, as the model SDKs make.
const RETRY_WAITS: [u64; 2] = if cfg!(test) { [10, 10] } else { [1000, 3000] };

/// The longest a service's own `retry-after` is waited for, in milliseconds.
const RETRY_AFTER_CAP: u64 = 20_000;

/// Statuses that say "not now", not "not this": too many requests, a
/// service that is overloaded or failing, a request that timed out there.
fn retryable(status: u16) -> bool {
    matches!(status, 408 | 409 | 429 | 500..=599)
}

/// Send a chat request and forward the model's turn: its text as it
/// arrives, then the whole turn with any tool calls. A request the service
/// is too busy for, or that never reached it, is sent again a little later,
/// up to twice; nothing of an answer has arrived by then, so nothing is
/// repeated. Returns when the stream ends or the receiver is gone (the
/// conversation was reset or stopped).
pub fn stream_chat(request: HttpRequest, protocol: Protocol, events: async_channel::Sender<ChatEvent>) {
    let send = |event| events.send_blocking(event).is_ok();
    // The model service may have a proxy of its own, or be told to use none.
    let agent = match agent(None, &request.proxy) {
        Ok(agent) => agent,
        Err(err) => {
            send(ChatEvent::Error(err));
            return;
        }
    };
    // Compact JSON: the documents can be hundreds of kilobytes, and indentation only adds to them.
    let body = request.body.to_string();
    let mut tries = 0;
    let mut response = loop {
        let mut call = agent.post(&request.url);
        for (name, value) in &request.headers {
            call = call.header(name, value);
        }
        let last = tries >= RETRY_WAITS.len();
        let (why, wait) = match call.send(body.as_str()) {
            Ok(response) => {
                let status = response.status().as_u16();
                if last || !retryable(status) { break response }
                // The service may say how long it wants to be left alone.
                let asked = response.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<f64>().ok()).map(|seconds| ((seconds.max(0.) * 1000.) as u64).min(RETRY_AFTER_CAP));
                (format!("HTTP {status}"), asked)
            }
            // A request that waited two minutes for nothing is not worth two more waits.
            Err(err) if last || matches!(err, ureq::Error::Timeout(_)) => {
                send(ChatEvent::Error(network_error(err)));
                return;
            }
            Err(err) => (network_error(err), None),
        };
        let wait = wait.unwrap_or(RETRY_WAITS[tries]);
        tries += 1;
        if !send(ChatEvent::Retrying(tries, why)) { return }
        std::thread::sleep(Duration::from_millis(wait));
    };
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        send(ChatEvent::Error(ai::error_message(status, &body)));
        return;
    }
    let mut reader = TurnReader::new(protocol);
    for line in BufReader::new(response.body_mut().as_reader()).lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                send(ChatEvent::Error(format!("连接中断：{err}")));
                return;
            }
        };
        match reader.line(&line) {
            Ok(Some(text)) => {
                if !send(ChatEvent::Text(text)) { return }
            }
            Ok(None) => {}
            Err(error) => {
                send(ChatEvent::Error(error));
                return;
            }
        }
    }
    if let Some(used) = reader.used() && !send(ChatEvent::Used(used)) { return }
    send(ChatEvent::Done(reader.finish()));
}

/// GET what an account holds at a model service (`wisp_core::usage::balance_request`): the answer as text, for
/// `usage::read_balance`. Asked once, after an answer; a service that does not answer in twenty seconds is not waited for.
pub fn balance(request: &HttpRequest) -> Result<String, String> {
    let mut call = agent(Some(Duration::from_secs(20)), &request.proxy)?.get(&request.url);
    for (name, value) in &request.headers {
        call = call.header(name, value);
    }
    let mut response = call.call().map_err(network_error)?;
    let status = response.status().as_u16();
    let body = response.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| format!("无法读取余额接口的返回内容：{e}"))?;
    if !(200..300).contains(&status) { return Err(ai::error_message(status, &body)) }
    Ok(body)
}

/// What `complete` says of an answer without a word in it. Where nothing
/// is itself an answer (no block of a file is body text), the caller tells
/// it apart from a failure by this.
pub const NO_CONTENT: &str = "模型没有返回内容。";

/// Send a request and wait for the whole answer as text: for a one-off
/// question whose reply is read by code, not shown as it arrives.
pub fn complete(request: HttpRequest, protocol: Protocol) -> Result<String, String> {
    let (sender, events) = async_channel::unbounded();
    stream_chat(request, protocol, sender);
    let mut text = String::new();
    while let Ok(event) = events.try_recv() {
        match event {
            ChatEvent::Text(more) => text.push_str(&more),
            ChatEvent::Retrying(..) | ChatEvent::Used(_) => {}
            ChatEvent::Error(error) => return Err(error),
            // A reply that was cut off or declined is not one to parse.
            ChatEvent::Done(turn) => {
                if let Some(note) = ai::stop_note(&turn.stop) { return Err(note.to_string()) }
            }
        }
    }
    if text.trim().is_empty() { return Err(NO_CONTENT.into()) }
    Ok(text)
}

/// How long 连接测试 waits for a model to answer its one question.
const TEST_WAIT: Duration = Duration::from_secs(60);

/// What 连接测试 says of a model that did not answer within `TEST_WAIT`.
pub const TEST_TIMED_OUT: &str = "一分钟内没有答完，没有再等。";

/// 设置 › AI › 模型's 连接测试: ask a model its one question (`ai::test_request`, or `judge::test_request` for a
/// model that judges) the way its requests go, through its service's proxy, and wait for the whole answer. What it
/// said, cut to a line, and how long that took. A service too busy to answer is not asked again, and none is waited
/// for past a minute: what is asked is how things stand now.
pub fn test_model(profile: &ai::Profile) -> Result<(String, Duration), String> {
    let request = if profile.judges() { wisp_core::judge::test_request(profile)? } else { ai::test_request(profile)? };
    let started = std::time::Instant::now();
    let timed_out = |err: String| if started.elapsed() >= TEST_WAIT { TEST_TIMED_OUT.to_string() } else { err };
    let mut call = agent(Some(TEST_WAIT), &request.proxy)?.post(&request.url);
    for (name, value) in &request.headers {
        call = call.header(name, value);
    }
    let mut response = call.send(request.body.to_string().as_str()).map_err(|err| if matches!(err, ureq::Error::Timeout(_)) { TEST_TIMED_OUT.to_string() } else { network_error(err) })?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        return Err(ai::error_message(status, &body));
    }
    if profile.judges() {
        let reply = response.body_mut().with_config().limit(1 << 20).read_json::<Value>().map_err(|e| timed_out(format!("无法解析模型服务的返回内容：{e}")))?;
        return Ok((wisp_core::judge::read_test(&reply)?, started.elapsed()));
    }
    let mut reader = TurnReader::new(profile.protocol);
    let (mut text, mut stray) = (String::new(), String::new());
    for line in BufReader::new(response.body_mut().as_reader()).lines() {
        let line = line.map_err(|err| timed_out(format!("连接中断：{err}")))?;
        // What is no line of a stream is kept, to show what came instead of an answer.
        if !line.trim().is_empty() && !line.starts_with("data:") && !line.starts_with("event:") && !line.starts_with(':') && stray.len() < 300 { stray.push_str(line.trim()) }
        if let Some(more) = reader.line(&line)? { text.push_str(&more) }
    }
    // A model that thought and said nothing still answered, saying why it stopped; one that refused did not.
    let turn = reader.finish();
    if text.trim().is_empty() {
        if let Some(note) = ai::stop_note(&turn.stop) { return Err(note.to_string()) }
        // Not a word and no end of an answer: something else answered at that address.
        if turn.stop == "end" { return Err(if stray.is_empty() { NOT_A_MODEL.to_string() } else { format!("{NOT_A_MODEL}收到的是：{}", stray.chars().take(200).collect::<String>()) }) }
    }
    Ok((ai::test_reply(&text), started.elapsed()))
}

/// What 连接测试 says when the service answered with no stream of a model's answer: a wrong address or interface.
pub const NOT_A_MODEL: &str = "服务有回应，但不是模型的回答，请检查服务地址和接口。";

/// How long a service may take over one picture: the large ones take minutes.
const PICTURE_WAIT: Duration = Duration::from_secs(300);

/// The most an answer with a picture in it, or the picture itself, may be.
const MAX_PICTURE: u64 = 96 << 20;

/// Ask a model service for a picture (`imagegen::request`) and wait for
/// it: the bytes of a PNG, JPEG or WebP. A service too busy to take the
/// request is asked again like a chat request; one that answers with an
/// address has the picture fetched from there, through the same proxy.
pub fn generate_image(request: HttpRequest, interface: imagegen::Interface) -> Result<Vec<u8>, String> {
    let reply = post_json(&request, PICTURE_WAIT)?;
    let picture = match imagegen::read(interface, &reply)? {
        imagegen::Made::Picture(bytes) => bytes,
        imagegen::Made::Address(url) => {
            let mut response = patient_agent(Some(Duration::from_secs(180)), &request.proxy, Duration::from_secs(120))?.get(&url).call().map_err(network_error)?;
            let status = response.status().as_u16();
            if !(200..300).contains(&status) { return Err(format!("图片画好了，但取不回来（HTTP {status}）。")) }
            response.body_mut().with_config().limit(MAX_PICTURE).read_to_vec().map_err(|e| format!("图片画好了，但取不回来：{e}"))?
        }
    };
    if wisp_core::figure::picture_kind(&picture).is_none() { return Err("模型服务返回的不是 PNG、JPEG 或 WebP 图片。".into()) }
    Ok(picture)
}

/// Ask a model that judges (`judge::request`) and wait for its answer, a
/// JSON document read by `judge::read_screening`. These models answer in a
/// fraction of a second; a busy service is asked again like any other.
pub fn judge(request: HttpRequest) -> Result<Value, String> {
    post_json(&request, Duration::from_secs(60))
}

/// POST a request whose answer is one JSON document, waiting `answer` for it to begin (`post_retrying`).
fn post_json(request: &HttpRequest, answer: Duration) -> Result<Value, String> {
    let mut response = post_retrying(request, answer)?;
    response.body_mut().with_config().limit(MAX_PICTURE).read_json::<Value>().map_err(|e| format!("无法解析模型服务的返回内容：{e}"))
}

/// Ask the speech service for one piece of text read aloud (`speech::request`) and hand the sound to
/// `take` as it arrives, in reads of a few kilobytes; `take` answers false to stop. A service too
/// busy to begin is asked again; once sound has come, a cut connection is an error, not a retry.
pub fn speak(request: &HttpRequest, mut take: impl FnMut(&[u8]) -> bool) -> Result<(), String> {
    use std::io::Read;
    let mut response = post_retrying(request, Duration::from_secs(60))?;
    let mut reader = response.body_mut().as_reader();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let n = reader.read(&mut buffer).map_err(|err| format!("朗读的声音没有传完：{err}"))?;
        if n == 0 || !take(&buffer[..n]) { return Ok(()) }
    }
}

/// POST a request, asking again (up to twice) while the service is too busy or was not reached, and
/// return a response that succeeded; an error response is said in the service's words.
fn post_retrying(request: &HttpRequest, answer: Duration) -> Result<ureq::http::Response<ureq::Body>, String> {
    let agent = patient_agent(None, &request.proxy, answer)?;
    let body = request.body.to_string();
    let mut tries = 0;
    let mut response = loop {
        let mut call = agent.post(&request.url);
        for (name, value) in &request.headers {
            call = call.header(name, value);
        }
        let last = tries >= RETRY_WAITS.len();
        match call.send(body.as_str()) {
            Ok(response) if last || !retryable(response.status().as_u16()) => break response,
            Ok(_) => {}
            // Minutes were waited already: asking again would only double them.
            Err(err) if last || matches!(err, ureq::Error::Timeout(_)) => return Err(network_error(err)),
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(RETRY_WAITS[tries]));
        tries += 1;
    };
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        return Err(ai::error_message(status, &body));
    }
    Ok(response)
}

/// What the tools are lent here beyond plain requests: Notion through its connector, web search, and the reading of pages that give no text.
pub fn reach() -> wisp_core::tools::Reach<'static> {
    wisp_core::tools::Reach { publish: Some(&notes::publish), search: Some(&web_search), read: Some(&read_file) }
}

/// Have the engine `asked` (empty: as settings say) read the pages of the file `name` holds. Settings are read anew
/// each time, as for a web search. A model that looks at pictures of the pages is not asked from here: it reads
/// a file in parts, one request a page, which is the window's run (`sorting.rs`).
pub fn read_file(asked: &str, name: &str, file: &[u8]) -> Result<ocr::Reading, String> {
    let settings = wisp_core::settings::Settings::load(&wisp_core::settings::settings_file()).map_err(|e| format!("{e:#}"))?;
    let ready = ocr::ready(&settings, asked)?;
    if ready.engine.key == ocr::MODEL { return Err("看图模型一页一页地读，只在桌面应用里用：这里请用系统文字识别（engine 为 system），或在「设置 › AI › 模型」的 DeepSeek 服务里填写 Key。".into()) }
    Ok(ocr::Reading { engine: ready.engine, pages: read_with(&ready, name, file)? })
}

/// The pages of a file as an engine that takes the whole file reads them: the system's own on this machine, or a
/// service the file is sent to.
pub fn read_with(ready: &ocr::Ready, name: &str, file: &[u8]) -> Result<Vec<ocr::Page>, String> {
    if ready.engine.key == ocr::SYSTEM { return sight::read(file) }
    send_file(&ocr::request(ready, name, file)?, ready.engine)
}

/// How long an engine may take over one file, the upload included: DeepSeek gives up after three minutes itself.
const READING_WAIT: Duration = Duration::from_secs(240);

/// Send a file to be read and wait for its pages. A service too busy to begin (429, 500, 502, 503) is asked again;
/// one that began and ran out of time is not, since the file would only go up and run out again.
pub fn send_file(upload: &ocr::Upload, engine: &'static ocr::Engine) -> Result<Vec<ocr::Page>, String> {
    let agent = patient_agent(Some(READING_WAIT), &upload.proxy, READING_WAIT)?;
    let mut tries = 0;
    loop {
        let mut call = agent.post(&upload.url);
        for (name, value) in &upload.headers { call = call.header(name, value) }
        let last = tries >= RETRY_WAITS.len();
        match call.send(upload.body.as_slice()) {
            Ok(mut response) if last || !matches!(response.status().as_u16(), 429 | 500 | 502 | 503) => {
                let status = response.status().as_u16();
                let text = response.body_mut().with_config().limit(256 << 20).read_to_string().map_err(|e| format!("{} 的返回内容没有读完：{e}", engine.name))?;
                return ocr::read(engine, status, &text);
            }
            Ok(_) => {}
            Err(err) if last || matches!(err, ureq::Error::Timeout(_)) => return Err(format!("{}：{}", engine.name, network_error(err))),
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(RETRY_WAITS[tries]));
        tries += 1;
    }
}

/// Search the web with the engine `asked` (empty: as settings say). Settings are read anew each time, so a
/// key typed into the window holds for the next search, the command line's included.
pub fn web_search(asked: &str, query: &websearch::Query) -> Result<websearch::Found, String> {
    let settings = wisp_core::settings::Settings::load(&wisp_core::settings::settings_file()).map_err(|e| format!("{e:#}"))?;
    let ready = websearch::ready(&settings, asked)?;
    let hits = send_search(&websearch::request(&ready, query)?, ready.engine, query.content)?;
    Ok(websearch::Found { engine: ready.engine, hits })
}

/// Send one search (a GET when the request has no body) and read its answer. A search only reads, so one
/// the service is too busy for is asked again, as DeepSeek's documentation says to (408, 429, 5xx).
fn send_search(request: &HttpRequest, engine: &'static websearch::Engine, content: bool) -> Result<Vec<websearch::Hit>, String> {
    // Pages read for their text take a while longer to come.
    let answer = Duration::from_secs(if content { 90 } else { 30 });
    let agent = patient_agent(Some(answer + Duration::from_secs(30)), &request.proxy, answer)?;
    let body = request.body.to_string();
    let mut tries = 0;
    loop {
        let sent = if request.body.is_null() {
            let mut call = agent.get(&request.url);
            for (name, value) in &request.headers { call = call.header(name, value) }
            call.call()
        } else {
            let mut call = agent.post(&request.url);
            for (name, value) in &request.headers { call = call.header(name, value) }
            call.send(body.as_str())
        };
        let last = tries >= RETRY_WAITS.len();
        match sent {
            Ok(mut response) if last || !retryable(response.status().as_u16()) => {
                let status = response.status().as_u16();
                let text = response.body_mut().with_config().limit(16 << 20).read_to_string().map_err(|e| format!("{} 网页搜索的返回内容没有读完：{e}", engine.name))?;
                return websearch::read(engine, status, &text);
            }
            Ok(_) => {}
            Err(err) if last || matches!(err, ureq::Error::Timeout(_)) => return Err(format!("{} 网页搜索：{}", engine.name, network_error(err))),
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(RETRY_WAITS[tries]));
        tries += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use wisp_core::ai::{Context, Profile, chat_request, user_message};

    #[test]
    fn the_balance_of_an_account() {
        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"is_available\": true, \"balance_infos\": [{\"currency\": \"CNY\", \"total_balance\": \"110.00\"}]}");
        let profile = Profile { protocol: Protocol::OpenAI, base_url: format!("{base}/v1"), api_key: "k".into(), proxy: "direct".into(), ..Default::default() };
        let body = balance(&wisp_core::usage::balance_request(&profile).unwrap()).unwrap();
        assert_eq!(wisp_core::usage::read_balance(&body).unwrap().text(), "¥110.00");
        let asked = server.join().unwrap().to_lowercase();
        assert!(asked.starts_with("get /user/balance ") && asked.contains("authorization: bearer k"), "{asked}");
        // A refusal is said in the service's words.
        let (base, server) = serve("HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"error\": {\"message\": \"Authentication Fails\"}}");
        let refused = balance(&wisp_core::usage::balance_request(&Profile { base_url: base, ..profile }).unwrap()).unwrap_err();
        server.join().unwrap();
        assert!(refused.contains("401") && refused.contains("Authentication Fails"), "{refused}");
    }

    #[test]
    fn a_model_tested() {
        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"One word.\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"rea\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"dy\"},\"finish_reason\":\"stop\"}]}\n\n\
data: [DONE]\n\n");
        let profile = Profile { protocol: Protocol::OpenAI, base_url: format!("{base}/v1"), api_key: "k".into(), model: "m".into(), effort: "high".into(), proxy: "direct".into(), ..Default::default() };
        let (reply, _) = test_model(&profile).unwrap();
        assert_eq!(reply, "ready");
        let asked = server.join().unwrap();
        assert!(asked.starts_with("POST /v1/chat/completions ") && asked.contains(ai::TEST_QUESTION) && asked.contains("\"reasoning_effort\":\"high\""), "{asked}");
        // A refusal in the service's words, with what it most often means.
        let (base, server) = serve("HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"error\": {\"message\": \"Model Not Exist\"}}");
        let refused = test_model(&Profile { base_url: base.clone(), ..profile.clone() }).unwrap_err();
        server.join().unwrap();
        assert_eq!(refused, "地址或模型名称不存在（HTTP 404）：Model Not Exist");
        // Something that is no model answering at the address.
        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\nconnection: close\r\n\r\n<html>Welcome</html>");
        let stranger = test_model(&Profile { base_url: base, ..profile.clone() }).unwrap_err();
        server.join().unwrap();
        assert_eq!(stranger, format!("{NOT_A_MODEL}收到的是：<html>Welcome</html>"));
        // A model that judges is asked a question with two answers.
        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"answers\": {\"test\": {\"type\": \"choice\", \"choice\": \"true\", \"confidence\": 0.98}}}");
        let (choice, _) = test_model(&Profile { base_url: format!("{base}/v1"), kind: wisp_core::judge::KIND.into(), ..profile }).unwrap();
        let asked = server.join().unwrap();
        assert!(asked.starts_with("POST /v1/systemone ") && asked.contains("\"questions\""), "{asked}");
        assert_eq!(choice, "true");
    }

    #[test]
    fn a_web_search_sent_and_read() {
        let deepseek = websearch::engine("deepseek").unwrap();
        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"results\": [{\"title\": \"T\", \"url\": \"https://example.com\", \"snippet\": \"s\", \"summary\": \"long\", \"content\": \"\", \"site_name\": null, \"date\": null}]}");
        let ready = websearch::Ready { engine: deepseek, key: "k".into(), proxy: "direct".into() };
        let mut request = websearch::request(&ready, &websearch::Query { query: "q".into(), count: 3, content: false }).unwrap();
        request.url = format!("{base}/v1/tools/search");
        let hits = send_search(&request, deepseek, false).unwrap();
        assert_eq!((hits[0].title.as_str(), hits[0].summary.as_str()), ("T", "long"));
        let asked = server.join().unwrap().to_lowercase();
        assert!(asked.starts_with("post /v1/tools/search ") && asked.contains("authorization: bearer k") && asked.contains("\"count\":3"), "{asked}");
        // Jina's is a GET; its refusal is said in its words.
        let jina = websearch::engine("jina").unwrap();
        let (base, server) = serve("HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"data\": null, \"code\": 401, \"message\": \"Authentication is required\"}");
        let mut request = websearch::request(&websearch::Ready { engine: jina, ..ready }, &websearch::Query { query: "q w".into(), count: 3, content: false }).unwrap();
        request.url = request.url.replace("https://s.jina.ai", &base);
        let refused = send_search(&request, jina, false).unwrap_err();
        let asked = server.join().unwrap().to_lowercase();
        assert!(asked.starts_with("get /?q=q%20w&num=3 ") && asked.contains("x-respond-with: no-content"), "{asked}");
        assert!(refused.contains("Jina") && refused.contains("401") && refused.contains("Authentication is required"), "{refused}");
    }

    #[test]
    fn a_file_sent_to_be_read() {
        let deepseek = ocr::engine("deepseek").unwrap();
        // Line breaks keep the connection while the pages are read; the answer follows them.
        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n\n\n{\"id\": \"ocr-1\", \"object\": \"ocr.result\", \"page_count\": 1, \"pages\": [{\"index\": 0, \"text\": \"<｜ref｜>Scanned<｜/ref｜><｜box｜>[[10, 10, 900, 60]]<｜/box｜>\", \"finish_reason\": \"stop\"}]}");
        let ready = ocr::Ready { engine: deepseek, key: "k".into(), proxy: "direct".into(), base: format!("{base}/v1") };
        let upload = ocr::request(&ready, "scan.pdf", b"%PDF-1.4 pages").unwrap();
        let pages = send_file(&upload, deepseek).unwrap();
        assert_eq!((pages.len(), pages[0].text.as_str()), (1, "Scanned"));
        let asked = server.join().unwrap();
        assert!(asked.starts_with("POST /v1/tools/ocr ") && asked.to_lowercase().contains("authorization: bearer k") && asked.to_lowercase().contains("content-type: multipart/form-data; boundary=wisp-study-"), "{asked}");
        assert!(asked.contains("name=\"purpose\"\r\n\r\ntext_recognition") && asked.contains("filename=\"scan.pdf\"") && asked.contains("%PDF-1.4 pages"), "{asked}");
        // A refusal before it began, and a failure after, both in the service's words.
        let (base, server) = serve("HTTP/1.1 402 Payment Required\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"error\": {\"message\": \"Insufficient Balance\"}}");
        let refused = send_file(&ocr::request(&ocr::Ready { base, ..ready.clone() }, "scan.pdf", b"%PDF").unwrap(), deepseek).unwrap_err();
        server.join().unwrap();
        assert_eq!(refused, "DeepSeek OCR：账户余额不足（HTTP 402）：Insufficient Balance");
        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n\n{\"error\": {\"message\": \"OCR did not finish in time\", \"type\": \"gateway_timeout_error\", \"param\": null, \"code\": \"gateway_timeout_error\"}}");
        let late = send_file(&ocr::request(&ocr::Ready { base, ..ready }, "scan.pdf", b"%PDF").unwrap(), deepseek).unwrap_err();
        server.join().unwrap();
        assert!(late.contains("限定的时间") && late.contains("OCR did not finish in time"), "{late}");
    }

    #[test]
    fn an_audio_clip_sent_to_be_transcribed() {
        let deepseek = asr::engine("deepseek").unwrap();
        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"request_id\": \"asr-1\", \"text\": \"帮我找几篇文献\", \"duration_ms\": 1800}");
        let ready = asr::Ready { engine: deepseek, key: "k".into(), proxy: "direct".into(), base: format!("{base}/v1") };
        let upload = asr::request(&ready, "speech.wav", b"RIFFtestwav").unwrap();
        let transcript = send_audio(&upload, deepseek).unwrap();
        assert_eq!((transcript.text.as_str(), transcript.duration_ms), ("帮我找几篇文献", Some(1800)));
        let asked = server.join().unwrap();
        assert!(asked.starts_with("POST /v1/tools/asr ") && asked.to_lowercase().contains("authorization: bearer k") && asked.to_lowercase().contains("content-type: multipart/form-data; boundary=wisp-study-"), "{asked}");
        assert!(asked.contains("name=\"file\"; filename=\"speech.wav\"") && asked.contains("RIFFtestwav"), "{asked}");
        let (base, server) = serve("HTTP/1.1 402 Payment Required\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"error\": {\"message\": \"Insufficient Balance\"}}");
        let refused = send_audio(&asr::request(&asr::Ready { base, ..ready }, "speech.wav", b"RIFF").unwrap(), deepseek).unwrap_err();
        server.join().unwrap();
        assert_eq!(refused, "DeepSeek 语音输入：账户余额不足（HTTP 402）：Insufficient Balance");
    }

    /// Serve one canned HTTP response on a local port; returns the base URL and the request it received.
    pub(crate) fn serve(response: &'static str) -> (String, std::thread::JoinHandle<String>) {
        serve_in_turn(vec![response])
    }

    /// Serve one canned response to each connection in turn; returns the base URL and the last request received.
    pub(crate) fn serve_in_turn(responses: Vec<&'static str>) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut last = String::new();
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut received = vec![];
                let mut buffer = [0u8; 4096];
                // Read the headers and the body they announce.
                loop {
                    let n = stream.read(&mut buffer).unwrap();
                    received.extend_from_slice(&buffer[..n]);
                    let text = String::from_utf8_lossy(&received).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length: usize = text.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                        // ureq sends a JSON body chunked: it ends with a zero-length chunk.
                        let chunked = text.to_lowercase().contains("transfer-encoding: chunked");
                        if (chunked && text.ends_with("0\r\n\r\n")) || (!chunked && received.len() >= end + 4 + length) { break }
                    }
                    if n == 0 { break }
                }
                stream.write_all(response.as_bytes()).unwrap();
                last = String::from_utf8_lossy(&received).to_string();
            }
            last
        });
        (base, handle)
    }

    /// A caller with a time to keep is not held by a host that takes the connection and says nothing, and asks
    /// nothing more once the time is up.
    #[test]
    fn a_silent_host_is_left_when_the_time_is_up() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/paper.pdf", listener.local_addr().unwrap());
        // Holds the connection longer than the test waits; not joined, it ends with the process.
        std::thread::spawn(move || {
            let held = listener.accept();
            std::thread::sleep(Duration::from_secs(5));
            drop(held);
        });
        let began = std::time::Instant::now();
        let (cut, refused) = wisp_core::sources::within(Duration::from_millis(400), || (download(&url), get_text(&url)));
        assert!(began.elapsed() < Duration::from_secs(3), "{:?}", began.elapsed());
        assert!(cut.as_ref().is_err_and(|err| err.contains("迟迟没有答完")), "{cut:?}");
        assert_eq!(refused, Err(format!("{}。", wisp_core::sources::TIME_UP)));
    }

    /// A database that kept saying "too fast" is named, with what would have it take more from this caller.
    #[test]
    fn a_host_holding_back_says_what_would_help() {
        let nobody = Contact::default();
        assert_eq!(held_back("api.crossref.org", sources::more_allowed("https://api.crossref.org/works?rows=5", &nobody)), format!("api.crossref.org 限制了请求频率，请稍后再试；{}。", sources::GIVE_EMAIL));
        assert_eq!(held_back("export.arxiv.org", sources::more_allowed("https://export.arxiv.org/api/query", &nobody)), "export.arxiv.org 限制了请求频率，请稍后再试。");
    }

    /// Send the conversation `wire` to the server at `base` and collect what comes back.
    fn step(base: String, protocol: Protocol, wire: &[Value]) -> (Vec<ChatEvent>, HttpRequest) {
        let profile = Profile { protocol, base_url: base, api_key: "test-key".into(), model: "m".into(), ..Default::default() };
        let ctx = Context { instructions: "I".into(), documents: "D".into(), ..Default::default() };
        let request = chat_request(&profile, &ctx, wire, &wisp_core::tools::library_catalog()).unwrap();
        let (sender, events) = async_channel::unbounded();
        stream_chat(request.clone(), protocol, sender);
        let mut seen = vec![];
        while let Ok(event) = events.try_recv() {
            seen.push(event);
        }
        (seen, request)
    }

    fn ask(base: String, protocol: Protocol) -> (Vec<ChatEvent>, HttpRequest) {
        step(base, protocol, &[user_message("你好")])
    }

    /// One question answered in two steps: the model calls a tool, gets the
    /// result, and answers. What matters is what the second request carries.
    #[test]
    fn tool_round_trip() {
        let (base, server) = serve(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"c2ln\"}}\n\n\
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"search_fulltext\",\"input\":{}}}\n\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\": \\\"chromium\\\"}\"}}\n\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
        );
        let mut wire = vec![user_message("哪篇文献提到 Chromium？")];
        let (events, _) = step(base, Protocol::Anthropic, &wire);
        server.join().unwrap();
        let [ChatEvent::Done(turn)] = &events[..] else { panic!("{events:?}") };
        assert_eq!((turn.calls[0].name.as_str(), turn.calls[0].args.clone()), ("search_fulltext", Ok(serde_json::json!({"query": "chromium"}))));
        assert!(turn.runnable());

        wire.push(turn.message.clone());
        wire.extend(ai::tool_results(Protocol::Anthropic, &[(turn.calls[0].id.clone(), "{\"total\":1}".into(), false)]));
        let (base, server) = serve(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"[P1 p.2] 提到了。\"}}\n\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
        );
        let (events, request) = step(base, Protocol::Anthropic, &wire);
        server.join().unwrap();
        assert_eq!(events[0], ChatEvent::Text("[P1 p.2] 提到了。".into()));
        assert!(matches!(&events[1], ChatEvent::Done(turn) if turn.calls.is_empty() && turn.stop == "end_turn"));
        // The model's turn went back whole, followed by the result of its call.
        let sent = &request.body["messages"];
        assert_eq!(sent[1]["content"][0], serde_json::json!({"type": "thinking", "thinking": "", "signature": "c2ln"}));
        assert_eq!(sent[1]["content"][1]["input"]["query"], "chromium");
        assert_eq!(sent[2]["content"][0], serde_json::json!({"type": "tool_result", "tool_use_id": "toolu_1", "content": "{\"total\":1}", "is_error": false}));
    }

    #[test]
    fn anthropic_stream() {
        let (base, server) = serve(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"c2ln\"}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"根尖 \"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"[P1]\"}}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        let (events, _) = ask(base, Protocol::Anthropic);
        assert_eq!(events[..2], [ChatEvent::Text("根尖 ".into()), ChatEvent::Text("[P1]".into())]);
        let ChatEvent::Done(turn) = &events[2] else { panic!("{events:?}") };
        // The turn goes back to the model as it came, the signed thinking block included.
        assert_eq!((turn.stop.as_str(), turn.calls.len(), turn.message["content"][0]["signature"].as_str(), turn.message["content"][1]["text"].as_str()), ("end_turn", 0, Some("c2ln"), Some("根尖 [P1]")));
        let received = server.join().unwrap();
        assert!(received.starts_with("POST /v1/messages "), "{received}");
        assert!(received.to_lowercase().contains("x-api-key: test-key"));
        assert!(received.contains("\"stream\":true") && received.contains("你好") && received.contains("\"search_fulltext\""), "{received}");
        // A local address is not Anthropic's API: no beta parameters.
        assert!(!received.contains("fallbacks"));
    }

    #[test]
    fn responses_stream() {
        let (base, server) = serve(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"根尖 [P1]\"}\n\n\
event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"根尖 [P1]\"}]}}\n\n\
event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
        );
        let (events, request) = ask(format!("{base}/v1"), Protocol::Responses);
        assert_eq!(events[0], ChatEvent::Text("根尖 [P1]".into()));
        assert!(matches!(&events[1], ChatEvent::Done(turn) if turn.stop == "stop" && turn.wire()[0]["id"] == "msg_1"));
        let received = server.join().unwrap();
        assert!(received.starts_with("POST /v1/responses "), "{received}");
        assert!(received.to_lowercase().contains("authorization: bearer test-key"));
        assert_eq!((request.body["store"].as_bool(), request.body["input"][0]["content"].as_str()), (Some(false), Some("你好")));
    }

    /// A profile's own proxy takes its request; `direct` and a bad address are what they say.
    #[test]
    fn profile_proxy() {
        let reply = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}]}\n\n";
        let ask_through = |base: &str, proxy: &str| {
            let profile = Profile { protocol: Protocol::OpenAI, base_url: base.into(), api_key: "k".into(), model: "m".into(), proxy: proxy.into(), ..Default::default() };
            let request = chat_request(&profile, &Context { instructions: "I".into(), documents: "D".into(), ..Default::default() }, &[user_message("你好")], &[]).unwrap();
            let (sender, events) = async_channel::unbounded();
            stream_chat(request, Protocol::OpenAI, sender);
            std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>()
        };
        // A proxy that grants the tunnel it is asked for and then answers inside it itself. The
        // service's name resolves nowhere: only the proxy can have answered.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut read_until = |end: &str| {
                let mut received = vec![];
                let mut buffer = [0u8; 4096];
                while !String::from_utf8_lossy(&received).ends_with(end) {
                    let n = stream.read(&mut buffer).unwrap();
                    if n == 0 { break }
                    received.extend_from_slice(&buffer[..n]);
                }
                String::from_utf8_lossy(&received).to_string()
            };
            let asked = read_until("\r\n\r\n");
            let mut stream = stream;
            stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
            let mut received = vec![];
            let mut buffer = [0u8; 4096];
            // The whole request: its body is either counted or chunked, ending with a zero-length chunk.
            loop {
                let n = stream.read(&mut buffer).unwrap();
                received.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&received).to_lowercase();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length: usize = text.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                    if (text.contains("transfer-encoding: chunked") && text.ends_with("0\r\n\r\n")) || (!text.contains("transfer-encoding: chunked") && received.len() >= end + 4 + length) { break }
                }
                if n == 0 { break }
            }
            stream.write_all(reply.as_bytes()).unwrap();
            (asked, String::from_utf8_lossy(&received).to_string())
        });
        let events = ask_through("http://model.invalid/v1", &proxy);
        assert_eq!(events[0], ChatEvent::Text("Hi".into()), "{events:?}");
        let (asked, inner) = server.join().unwrap();
        assert!(asked.starts_with("CONNECT model.invalid:80 "), "{asked}");
        assert!(inner.starts_with("POST /v1/chat/completions "), "{inner}");

        let (base, server) = serve(reply);
        assert_eq!(ask_through(&format!("{base}/v1"), "direct")[0], ChatEvent::Text("Hi".into()));
        assert!(server.join().unwrap().starts_with("POST /v1/chat/completions "));

        let events = ask_through("http://model.invalid/v1", "not a proxy");
        assert!(matches!(&events[..], [ChatEvent::Error(e)] if e.contains("代理地址")), "{events:?}");
    }

    #[test]
    fn openai_stream_and_errors() {
        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n");
        let (events, _) = ask(format!("{base}/v1"), Protocol::OpenAI);
        assert_eq!(events[0], ChatEvent::Text("Hi".into()));
        assert!(matches!(&events[1], ChatEvent::Done(turn) if turn.stop == "stop" && turn.message["content"] == "Hi"));
        assert!(server.join().unwrap().starts_with("POST /v1/chat/completions "));

        let (base, server) = serve("HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: 41\r\nconnection: close\r\n\r\n{\"error\":{\"message\":\"invalid x-api-key\"}}");
        let (events, _) = ask(base, Protocol::Anthropic);
        assert_eq!(events, [ChatEvent::Error("API Key 无效或没有权限（HTTP 401）：invalid x-api-key".into())]);
        server.join().unwrap();

        // Nothing listening: tried three times, then a network error, not a hang.
        let (events, _) = ask("http://127.0.0.1:9".into(), Protocol::OpenAI);
        assert!(matches!(&events[..], [ChatEvent::Retrying(1, _), ChatEvent::Retrying(2, _), ChatEvent::Error(e)] if e.starts_with("网络请求失败")), "{events:?}");
    }

    #[test]
    fn a_busy_service_is_asked_again() {
        const BUSY: &str = "HTTP/1.1 529 Overloaded\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}";
        const LIMITED: &str = "HTTP/1.1 429 Too Many Requests\r\nretry-after: 0.01\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        const ANSWER: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}]}\n\n";
        // Overloaded, then rate-limited, then answered: the answer arrives once, after two notices.
        let (base, server) = serve_in_turn(vec![BUSY, LIMITED, ANSWER]);
        let (events, _) = ask(format!("{base}/v1"), Protocol::OpenAI);
        assert_eq!(events[..3], [ChatEvent::Retrying(1, "HTTP 529".into()), ChatEvent::Retrying(2, "HTTP 429".into()), ChatEvent::Text("Hi".into())], "{events:?}");
        assert!(matches!(&events[3], ChatEvent::Done(turn) if turn.stop == "stop"));
        server.join().unwrap();
        // Still busy on the third try: the service's own words, and no fourth request.
        let (base, server) = serve_in_turn(vec![BUSY, BUSY, BUSY]);
        let (events, _) = ask(base, Protocol::Anthropic);
        assert!(matches!(&events[..], [ChatEvent::Retrying(1, _), ChatEvent::Retrying(2, _), ChatEvent::Error(e)] if e.contains("Overloaded")), "{events:?}");
        server.join().unwrap();
    }

    /// A picture asked for through each interface: what is sent where, and what comes back.
    #[test]
    fn pictures() {
        // "\x89PNG\r\n\x1a\n" in base64.
        const PNG: &str = "iVBORw0KGgo=";
        let profile = |base: String, model: &str, interface: &str| Profile { protocol: Protocol::OpenAI, base_url: base, api_key: "test-key".into(), model: model.into(), interface: interface.into(), ..Default::default() };
        let draw = |profile: &Profile| generate_image(imagegen::request(profile, "a root tip", imagegen::Shape::Wide).unwrap(), imagegen::interface(profile));

        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"created\":1,\"data\":[{\"b64_json\":\"iVBORw0KGgo=\"}]}");
        assert_eq!(draw(&profile(format!("{base}/v1"), "gpt-image-2", "")).as_deref(), Ok(&b"\x89PNG\r\n\x1a\n"[..]));
        let received = server.join().unwrap();
        assert!(received.starts_with("POST /v1/images/generations "), "{received}");
        assert!(received.to_lowercase().contains("authorization: bearer test-key") && received.contains("\"size\":\"1536x1024\"") && received.contains("a root tip"), "{received}");

        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Here.\"},{\"inlineData\":{\"mimeType\":\"image/png\",\"data\":\"iVBORw0KGgo=\"}}]}}]}");
        assert_eq!(draw(&profile(format!("{base}/v1beta/openai"), "gemini-3-pro-image-preview", "gemini")).map(|bytes| bytes.len()), Ok(8));
        let received = server.join().unwrap();
        assert!(received.starts_with("POST /v1beta/models/gemini-3-pro-image-preview:generateContent "), "{received}");
        assert!(received.to_lowercase().contains("x-goog-api-key: test-key") && received.contains("\"aspectRatio\":\"3:2\""), "{received}");

        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"\",\"images\":[{\"type\":\"image_url\",\"image_url\":{\"url\":\"data:image/png;base64,iVBORw0KGgo=\"}}]}}]}");
        assert_eq!(draw(&profile(format!("{base}/v1"), "gemini-3.1-flash-image-preview", "")).map(|bytes| bytes.len()), Ok(8));
        assert!(server.join().unwrap().starts_with("POST /v1/chat/completions "));
        assert_eq!(PNG, ai::base64(b"\x89PNG\r\n\x1a\n"));

        // Words in place of a picture, bytes that are no picture, and a refusal are each said as they are.
        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"choices\":[{\"message\":{\"content\":\"I can only describe it.\"}}]}");
        assert_eq!(draw(&profile(format!("{base}/v1"), "some-model", "chat")), Err("模型没有返回图片，它说：I can only describe it.".into()));
        server.join().unwrap();
        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"data\":[{\"b64_json\":\"JVBERi0xLjc=\"}]}");
        assert!(draw(&profile(format!("{base}/v1"), "gpt-image-2", "")).unwrap_err().contains("不是 PNG"));
        server.join().unwrap();
        let (base, server) = serve("HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"error\":{\"message\":\"Your request was rejected by the safety system.\"}}");
        assert_eq!(draw(&profile(format!("{base}/v1"), "gpt-image-2", "")), Err("请求被拒绝（HTTP 400）：Your request was rejected by the safety system.".into()));
        server.join().unwrap();
    }

    /// A paper judged by a model that judges: what is sent where, what is made of the answer, and how its errors read.
    #[test]
    fn sound_comes_as_it_is_sent() {
        let ask = |base: String| wisp_core::speech::request(&base, "test-key", "", "tide", "Hello.").unwrap();
        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\nconnection: close\r\n\r\n\x00\x40\x00\x20\x01");
        let mut pcm = wisp_core::speech::Pcm::default();
        let mut samples = vec![];
        speak(&ask(base), |bytes| { samples.extend(pcm.samples(bytes)); true }).unwrap();
        assert_eq!(samples, [0.5, 0.25]);
        let received = server.join().unwrap();
        assert!(received.starts_with("POST /v1/tools/tts "), "{received}");
        assert!(received.contains("\"voice_id\":\"tide\"") && received.contains("\"format\":\"pcm\""), "{received}");

        // A refusal is said in the service's words; a busy service is asked again.
        let (base, _server) = serve("HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"error\":{\"message\":\"Content Exists Risk\"}}");
        assert_eq!(speak(&ask(base), |_| true), Err("请求被拒绝（HTTP 400）：Content Exists Risk".into()));
        let (base, _server) = serve_in_turn(vec!["HTTP/1.1 503 Service Unavailable\r\nconnection: close\r\ncontent-length: 0\r\n\r\n", "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 2\r\n\r\n\x00\x00"]);
        let mut got = 0;
        speak(&ask(base), |bytes| { got += bytes.len(); true }).unwrap();
        assert_eq!(got, 2);
    }

    #[test]
    fn judgements() {
        let paper = Paper { title: "A single-cell atlas of the root".into(), abstract_text: "We profile 100,000 cells.".into(), ..Default::default() };
        let jev = |base: String| Profile { protocol: Protocol::OpenAI, base_url: format!("{base}/v1"), api_key: "test-key".into(), model: "jev-latest".into(), kind: wisp_core::judge::KIND.into(), ..Default::default() };
        let ask = |base: String| judge(wisp_core::judge::screen_request(&jev(base), "root development", "", &paper, None).unwrap().0);

        let (base, server) = serve("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"model\":\"jev-1.13.0\",\"answers\":{\"verdict\":{\"type\":\"choice\",\"choice\":\"include\",\"confidence\":0.88,\"probabilities\":{\"include\":0.93,\"related\":0.05,\"irrelevant\":0.02}}},\"usage\":{\"input_tokens\":212,\"output_tokens\":1}}");
        let judged = wisp_core::judge::read_screening(&ask(base).unwrap()).unwrap();
        assert_eq!((judged.verdict.as_str(), judged.confidence), ("included", Some(0.88)));
        let received = server.join().unwrap();
        assert!(received.starts_with("POST /v1/systemone "), "{received}");
        assert!(received.to_lowercase().contains("authorization: bearer test-key") && received.contains("\"type\":\"choice\"") && received.contains("A single-cell atlas of the root"), "{received}");

        // What is wrong with a request comes as `detail`: a sentence, or a list naming the field.
        let (base, server) = serve("HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"detail\":\"Invalid API key\"}");
        assert_eq!(ask(base), Err("API Key 无效或没有权限（HTTP 401）：Invalid API key".into()));
        server.join().unwrap();
        let (base, server) = serve("HTTP/1.1 422 Unprocessable Entity\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"detail\":[{\"loc\":[\"body\",\"model\"],\"msg\":\"Unknown model\",\"type\":\"value_error\"}]}");
        assert_eq!(ask(base), Err("请求被拒绝（HTTP 422）：Unknown model".into()));
        server.join().unwrap();
        // Overloaded twice, then answered.
        const BUSY: &str = "HTTP/1.1 529 Overloaded\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        let (base, server) = serve_in_turn(vec![BUSY, BUSY, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"answers\":{\"verdict\":{\"choice\":\"irrelevant\",\"confidence\":0.95}}}"]);
        assert_eq!(ask(base).map(|reply| wisp_core::judge::read_screening(&reply).map(|j| j.verdict)), Ok(Ok("irrelevant".to_string())));
        server.join().unwrap();
    }

    /// A service that answers with an address: the picture is fetched from there.
    #[test]
    fn a_picture_at_an_address() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let address = format!("{base}/out/a.png");
        let server = std::thread::spawn(move || {
            let mut asked = vec![];
            for n in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut received = vec![];
                let mut buffer = [0u8; 4096];
                loop {
                    let read = stream.read(&mut buffer).unwrap();
                    received.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&received).to_lowercase();
                    // The whole request: its body is either counted or chunked, ending with a zero-length chunk.
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length: usize = text.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                        let chunked = text.contains("transfer-encoding: chunked");
                        if (chunked && text.ends_with("0\r\n\r\n")) || (!chunked && received.len() >= end + 4 + length) { break }
                    }
                    if read == 0 { break }
                }
                asked.push(String::from_utf8_lossy(&received).to_string());
                if n == 0 {
                    let body = format!("{{\"data\":[{{\"url\":\"{address}\"}}]}}");
                    stream.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
                } else {
                    stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: image/jpeg\r\ncontent-length: 4\r\nconnection: close\r\n\r\n\xff\xd8\xff\xe0").unwrap();
                }
            }
            asked
        });
        let profile = Profile { protocol: Protocol::OpenAI, base_url: format!("{base}/api/v3"), api_key: "k".into(), model: "doubao-seedream-4-0-250828".into(), ..Default::default() };
        let picture = generate_image(imagegen::request(&profile, "根尖", imagegen::Shape::Square).unwrap(), imagegen::interface(&profile));
        assert_eq!(picture.as_deref(), Ok(&b"\xff\xd8\xff\xe0"[..]));
        let asked = server.join().unwrap();
        assert!(asked[0].starts_with("POST /api/v3/images/generations ") && asked[0].contains("\"watermark\":false"), "{}", asked[0]);
        assert!(asked[1].starts_with("GET /out/a.png "), "{}", asked[1]);
    }

    #[test]
    fn a_host_that_answers_is_reached_whatever_it_says() {
        let (base, server) = serve("HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        assert!(try_reach(&format!("{base}/works?rows=0")).is_ok());
        assert!(server.join().unwrap().starts_with("GET /works?rows=0 "));
        // Nothing listens there: the reason is said.
        let gone = { let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); format!("http://{}", listener.local_addr().unwrap()) };
        assert!(try_reach(&gone).unwrap_err().starts_with("网络请求失败："));
    }

    #[test]
    fn latest_release_reads_the_redirect() {
        let (base, server) = serve("HTTP/1.1 302 Found\r\nlocation: https://github.com/xuzhougeng/WispStudy/releases/tag/v9.8.7\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        let release = latest_release(&format!("{base}/releases/latest")).unwrap().unwrap();
        assert_eq!((release.version.as_str(), release.url.as_str()), ("9.8.7", "https://github.com/xuzhougeng/WispStudy/releases/tag/v9.8.7"));
        assert!(server.join().unwrap().starts_with("GET /releases/latest "));

        // No release published: GitHub redirects to the list. Elsewhere: never offered.
        for response in ["HTTP/1.1 302 Found\r\nlocation: https://github.com/xuzhougeng/WispStudy/releases\r\ncontent-length: 0\r\nconnection: close\r\n\r\n", "HTTP/1.1 302 Found\r\nlocation: https://example.com/releases/tag/v9.9.9\r\ncontent-length: 0\r\nconnection: close\r\n\r\n", "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"] {
            let (base, server) = serve(response);
            assert_eq!(latest_release(&base), Ok(None));
            server.join().unwrap();
        }
        let (base, server) = serve("HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        assert_eq!(latest_release(&base), Err("更新服务器返回 HTTP 503".into()));
        server.join().unwrap();
        assert!(latest_release("http://127.0.0.1:9").unwrap_err().starts_with("网络请求失败"));
    }

    #[test]
    fn a_release_package_is_held_against_its_checksum() {
        let ok = |body: &str| -> &'static str { format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).leak() };
        let sums = ok(&format!("{}  other.zip\n{}  a.dmg\n", "0".repeat(64), update::sha256(b"package")));
        let (base, server) = serve_in_turn(vec![sums, ok("package")]);
        assert_eq!(download_release(&format!("{base}/download/v9/"), "a.dmg").as_deref(), Ok(&b"package"[..]));
        assert!(server.join().unwrap().starts_with("GET /download/v9/a.dmg "));

        // Another content than the one published, a file the release does not list, a release without the file.
        let (base, server) = serve_in_turn(vec![sums, ok("tampered")]);
        assert_eq!(download_release(&base, "a.dmg"), Err("下载的更新包与发布的校验值不一致，已丢弃。".into()));
        server.join().unwrap();
        let (base, server) = serve(sums);
        assert_eq!(download_release(&base, "b.dmg"), Err("发布的校验文件里没有 b.dmg。".into()));
        server.join().unwrap();
        let (base, server) = serve_in_turn(vec![sums, "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"]);
        assert_eq!(download_release(&base, "a.dmg"), Err("更新服务器返回 HTTP 404".into()));
        server.join().unwrap();
    }

    /// Talks to Crossref and Europe PMC: `cargo test -- --ignored lookup`.
    #[test]
    #[ignore]
    fn lookup_real_identifiers() {
        let paper = lookup_paper(&Identifier::Doi("10.1038/s41586-020-2649-2".into())).unwrap();
        assert_eq!(paper.title, "Array programming with NumPy");
        assert_eq!((paper.year, paper.journal.as_str(), paper.pmid.as_str()), (Some(2020), "Nature", "32939066"));
        assert!(paper.authors.starts_with("Harris, Charles R.") && !paper.abstract_text.is_empty());
        let by_pmid = lookup_paper(&Identifier::Pmid("32939066".into())).unwrap();
        assert_eq!(by_pmid.doi, "10.1038/s41586-020-2649-2");
        assert!(lookup_paper(&Identifier::Doi("10.9999/definitely-not-a-real-doi-000".into())).is_err());
    }
}
