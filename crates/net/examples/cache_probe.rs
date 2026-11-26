//! How much of a conversation's request does a service read from its cache? A model is asked three things of a
//! made-up library the way the window's assistant asks them: the request is `ai::chat_request_with` with a tool
//! table, the model's calls run on a library in a temporary folder (`tools::call_in_with`, without the network),
//! and every request's tokens are printed as the service counted them (`usage::Usage`): what it read, how much
//! of that from its cache, what it had to read anew.
//!
//!     cargo run -p wisp_net --example cache_probe -- --base http://127.0.0.1:3425/v1 --model deepseek/deepseek-flash
//!         [--protocol openai|anthropic|responses] [--key …] [--tools full|reading|none|shift|drop] [--pause 0]
//!         [--drop web_search] [--effort off] [--steps 4] [--warm]
//!
//! `--tools`: `full` is the table of a conversation inside a library, `reading` only the tools that read papers
//! (`tools::ACROSS`), `none` no tool at all (what the rest of the request weighs), `shift` the full table
//! with one more tool after it from the fourth request on, as when a connector comes up in the middle of a
//! conversation, and `drop` the full table without one tool (`--drop`, `web_search` unless told) from the fourth request on, as
//! when its engine is switched off in the middle of one. Every run starts with words of its own at the head of the instructions and of the first tool,
//! so nothing an earlier run left in the service's cache is counted; `--warm` leaves them out. `--pause` waits
//! that many seconds between requests: a service may need a moment before what it just read can be read from
//! its cache. Nothing of a real library is read or sent.
//!
//! The columns: `input`, `cached`, `missed` and `output` are the service's counts; `est.all` is this project's
//! own estimate of the whole request (`compact::tokens`) and `est.tools` of its `tools`.

use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use wisp_core::ai::{self, ChatEvent, Profile, Protocol, Scope};
use wisp_core::compact;
use wisp_core::connector;
use wisp_core::libraries::{self, LibraryInfo};
use wisp_core::model::Paper;
use wisp_core::store::Store;
use wisp_core::tools;
use wisp_core::usage::Usage;

/// What is asked, one question after the other in one conversation. Each names the tool to use, so that the
/// requests of two runs are as alike as a model allows.
const ASKS: [&str; 3] = [
    "这个文献库里有几篇文献？用 list_papers 查一下，只回答篇数。",
    "用 get_paper 读讲 belzutifan 的那一篇，用一句话说它的结论。",
    "用 search_fulltext 检索 hypoxia，用一句话告诉我有没有命中。",
];

/// The request from which `shift` has one more tool and `drop` one less.
const SHIFT_AT: usize = 4;

#[derive(Clone, Copy, PartialEq)]
enum Table {
    Full,
    Reading,
    None,
    Shift,
    Drop,
}

struct Options {
    profile: Profile,
    table: Table,
    pause: u64,
    /// The tool `drop` takes out.
    dropped: String,
    steps: usize,
    warm: bool,
}

fn options() -> Result<Options, String> {
    let words: Vec<String> = std::env::args().skip(1).collect();
    let take = |name: &str| words.iter().position(|word| word == &format!("--{name}")).and_then(|at| words.get(at + 1)).cloned();
    let number = |name: &str, default: u64| take(name).map_or(Ok(default), |value| value.parse::<u64>().map_err(|_| format!("--{name} is a number: {value}")));
    let protocol = match take("protocol").as_deref() {
        None | Some("openai") => Protocol::OpenAI,
        Some("anthropic") => Protocol::Anthropic,
        Some("responses") => Protocol::Responses,
        Some(other) => return Err(format!("--protocol is openai, anthropic or responses: {other}")),
    };
    let table = match take("tools").as_deref() {
        None | Some("full") => Table::Full,
        Some("reading") => Table::Reading,
        Some("none") => Table::None,
        Some("shift") => Table::Shift,
        Some("drop") => Table::Drop,
        Some(other) => return Err(format!("--tools is full, reading, none, shift or drop: {other}")),
    };
    let base_url = take("base").ok_or("--base is the service's address, e.g. http://127.0.0.1:3425/v1")?;
    let model = take("model").ok_or("--model is the model's name")?;
    // A service on this machine is not reached through the proxy the environment names for the others, and asks for no key.
    let local = ["127.0.0.1", "localhost", "[::1]"].iter().any(|host| base_url.contains(&format!("//{host}")));
    let api_key = take("key").or_else(|| std::env::var("WISP_PROBE_KEY").ok()).unwrap_or_else(|| if local { "local".into() } else { String::new() });
    let profile = Profile { protocol, base_url, model, api_key, effort: take("effort").unwrap_or_default(), proxy: take("proxy").unwrap_or_else(|| if local { "direct".into() } else { String::new() }), ..Default::default() };
    Ok(Options { profile, table, dropped: take("drop").unwrap_or_else(|| "web_search".into()), pause: number("pause", 0)?, steps: number("steps", 4)? as usize, warm: words.iter().any(|word| word == "--warm") })
}

/// An error of the engine as a sentence, its causes with it.
fn said(error: impl std::fmt::Display) -> String {
    format!("{error:#}")
}

/// A library of four papers, records and abstracts only.
fn seed(root: &Path) -> Result<(LibraryInfo, Store), String> {
    let info = libraries::create(root, "HIF 通路", "低氧诱导因子与肿瘤", "blue").map_err(said)?;
    let mut store = Store::open(&info.dir).map_err(said)?;
    for (title, authors, year, journal, about) in [
        ("Hypoxia-inducible factors in physiology and medicine", "Semenza, Gregg L.", 2012, "Cell", "Hypoxia-inducible factors direct the transcriptional response of cells to reduced oxygen availability."),
        ("Oxygen sensing by metazoans: the central role of the HIF hydroxylase pathway", "Kaelin, William G.; Ratcliffe, Peter J.", 2008, "Molecular Cell", "Prolyl hydroxylation marks HIF-alpha for recognition by VHL, ubiquitination and proteasomal degradation when oxygen is available."),
        ("HIF1α and HIF2α: sibling rivalry in hypoxic tumour growth and progression", "Keith, Brian; Johnson, Randall S.; Simon, M. Celeste", 2012, "Nature Reviews Cancer", "HIF-1α and HIF-2α have distinct and sometimes opposing roles in tumour growth; HIF-2α drives clear cell renal carcinoma."),
        ("Belzutifan for renal cell carcinoma in von Hippel-Lindau disease", "Jonasch, Eric; Donskov, Frede", 2021, "New England Journal of Medicine", "The HIF-2α inhibitor belzutifan produced durable responses in renal cell carcinoma associated with VHL disease."),
    ] {
        store.add_paper(Paper { title: title.into(), authors: authors.into(), year: Some(year), journal: journal.into(), abstract_text: about.into(), ..Default::default() }).map_err(said)?;
    }
    Ok((info, store))
}

fn probe(options: &Options, root: &Path) -> Result<(), String> {
    let (info, mut store) = seed(root)?;
    let papers = store.list_papers().map_err(said)?;
    let (profile, protocol) = (&options.profile, options.profile.protocol);
    // Built once, with the first question, as the window does: the follow-ups send the same instructions and documents.
    let mut ctx = ai::build_context(&info.name, &info.topic, &papers.iter().collect::<Vec<_>>(), &store.list_fields().map_err(said)?, &BTreeMap::new(), &[], &Scope::Library, ASKS[0], &[]);
    ctx.add_sources(&papers);
    let mut catalog = match options.table {
        Table::Full | Table::Shift | Table::Drop => tools::library_catalog(),
        Table::Reading => tools::library_catalog().into_iter().filter(|tool| tools::ACROSS.contains(&tool.name)).collect(),
        Table::None => vec![],
    };
    if !options.warm {
        let mark = format!("（探测 {}-{}）", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos()));
        ctx.instructions = format!("{mark}\n{}", ctx.instructions);
        if let Some(first) = catalog.first_mut() { first.description = Box::leak(format!("{mark}{}", first.description).into_boxed_str()) }
    }
    let late = [connector::Tool { name: "notes__search".into(), tool: "search".into(), description: "在用户的笔记应用里按关键词查找笔记，返回标题和摘要。".into(), input_schema: json!({"type": "object", "properties": {"query": {"type": "string", "description": "关键词"}}, "required": ["query"]}), read_only: true }];
    let (offline, nothing) = (|_: &str| -> Result<String, String> { Err("这次探测不联网。".into()) }, |_: &str| -> Result<Vec<u8>, String> { Err("这次探测不联网。".into()) });
    println!("{:>3} {:>3} {:>5} {:>8} {:>8} {:>8} {:>6} {:>7}  {:>9} {:>9}  calls", "#", "ask", "tools", "input", "cached", "missed", "hit", "output", "est.all", "est.tools");
    let (mut wire, mut sent, mut all, mut later) = (vec![], 0, Usage::default(), Usage::default());
    for (at, ask) in ASKS.iter().enumerate() {
        wire.push(ai::user_message_with(protocol, ask, &[]));
        for _ in 0..options.steps {
            if sent > 0 && options.pause > 0 { std::thread::sleep(std::time::Duration::from_secs(options.pause)) }
            sent += 1;
            if options.table == Table::Drop && sent == SHIFT_AT { catalog.retain(|tool| tool.name != options.dropped) }
            let more: &[connector::Tool] = if options.table == Table::Shift && sent >= SHIFT_AT { &late } else { &[] };
            let request = ai::chat_request_with(profile, &ctx, &wire, &catalog, more)?;
            let (whole, table) = (compact::tokens(&request.body), request.body.get("tools").map_or(0, compact::tokens));
            let (events, heard) = async_channel::unbounded();
            wisp_net::stream_chat(request, protocol, events);
            let (mut turn, mut used) = (None, None);
            while let Ok(event) = heard.try_recv() {
                match event {
                    ChatEvent::Done(done) => turn = Some(done),
                    ChatEvent::Used(counted) => used = Some(counted),
                    ChatEvent::Error(error) => return Err(error),
                    ChatEvent::Text(_) | ChatEvent::Retrying(..) => {}
                }
            }
            let turn = turn.ok_or("the stream ended without a turn")?;
            let used = used.ok_or("the service did not say what the request used")?;
            let names: Vec<&str> = turn.calls.iter().map(|call| call.name.as_str()).collect();
            let hit = match used.cached { Some(cached) if used.input > 0 => format!("{:.0}%", cached as f64 * 100. / used.input as f64), _ => "?".into() };
            println!("{sent:>3} {:>3} {:>5} {:>8} {:>8} {:>8} {hit:>6} {:>7}  {whole:>9} {table:>9}  {}", at + 1, catalog.len() + more.len(), used.input, used.cached.map_or("?".into(), |cached| cached.to_string()), used.missed(), used.output, names.join(","));
            all.add(&used);
            if sent > 1 { later.add(&used) }
            wire.extend(turn.wire());
            if turn.calls.is_empty() { break }
            if !turn.runnable() { return Err(format!("the answer was cut off: {}", turn.stop)) }
            let mut results = vec![];
            for call in turn.calls {
                let ended = call.args.clone().and_then(|mut args| {
                    // As the window does: a paper may be named by its label.
                    ai::resolve_labels(&mut args, &ctx.sources);
                    if call.name.contains("__") { return Err("这个连接器在探测里不可用。".into()) }
                    tools::call_in_with(&mut store, &info, &call.name, &args, &offline, &nothing, tools::Reach::default())
                });
                results.push(match ended {
                    Ok(value) => (call.id, tools::output_text(&value), false),
                    Err(error) => (call.id, error, true),
                });
            }
            wire.extend(ai::tool_results(protocol, &results));
        }
    }
    let share = |used: &Usage| if used.input == 0 { 0. } else { used.cached.unwrap_or(0) as f64 * 100. / used.input as f64 };
    println!("\nall {sent} requests: read {}, from the cache {} ({:.0}%), anew {}", all.input, all.cached.unwrap_or(0), share(&all), all.missed());
    println!("after the first:  read {}, from the cache {} ({:.0}%), anew {}", later.input, later.cached.unwrap_or(0), share(&later), later.missed());
    Ok(())
}

fn main() {
    let options = match options() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}\n\ncache_probe --base <address> --model <name> [--key <key>] [--protocol openai|anthropic|responses] [--tools full|reading|none|shift|drop] [--pause <seconds>] [--drop <tool>] [--effort <level>] [--steps 4] [--warm]");
            std::process::exit(2);
        }
    };
    let root = std::env::temp_dir().join(format!("wisp-cache-probe-{}", std::process::id()));
    let ended = probe(&options, &root);
    std::fs::remove_dir_all(&root).ok();
    if let Err(error) = ended {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
