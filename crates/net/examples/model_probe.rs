//! How do different models use the tools that write a manuscript? Each model is asked the same things of the
//! same manuscript the way the window's 助手写 asks them: the request is `ai::chat_request` with the library's
//! tool table and the note of the writing conversation, the model's calls run on a library of its own in a
//! temporary folder (`compose::call_with`, as the window runs them, without the network), and it goes on until
//! it stops calling tools. What is judged is what became of the manuscript, not which tool or argument the model
//! chose: so the same cases compare models with one another, and a change to a tool or to the note with what was
//! there before (run the example of both builds).
//!
//!     cargo run -p wisp_net --example model_probe -- --base http://127.0.0.1:3425/v1 --key … --models a,b,c
//!         [--protocol openai|anthropic|responses] [--runs 3] [--cases sentence,long] [--steps 8] [--effort low]
//!         [--parallel 4] [--proxy direct] [--out calls.jsonl]
//!
//! Without `--key` the key is the environment's (`WISP_PROBE_KEY`, else what the protocol's profile reads).
//! Nothing of a real library is read or sent: the manuscript and its four papers are made up here. The table
//! counts, of each model's runs of a case: those that did what was asked, those that lost words nobody asked it
//! to touch (the fault this was written for: a model wrote a section's whole text where a change was meant), those
//! in which every writing call was refused, and those that wrote nothing. Words lost on the way count too (中途丢过):
//! a model cut a section of 8,000 characters to a paragraph and then wrote all of it again, which ends right and is
//! not what anyone wants to watch. `--out` keeps every writing call as the model sent it, one JSON object a line.

use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;
use wisp_core::ai::{self, ChatEvent, Profile, Protocol, Scope};
use wisp_core::compose::{self, Asked};
use wisp_core::libraries::{self, LibraryInfo};
use wisp_core::model::Paper;
use wisp_core::store::Store;
use wisp_core::tools;

const FIRST: &str = "低氧诱导因子（HIF）是细胞感知氧浓度变化的核心转录因子 [@Semenza2012]。HIF-1α 在常氧下被降解，在低氧下稳定并进入细胞核，与 HIF-1β 形成异二聚体后结合低氧反应元件，启动下游基因的转录 [@Kaelin2008]。";
const MIDDLE: &str = "在肿瘤里，HIF 通路的激活很常见。很多实体瘤的内部氧供不足，这会让 HIF-1α 积累起来，积累起来的 HIF-1α 又会去推动糖酵解相关基因和血管生成相关基因的表达，这样肿瘤细胞就能在缺氧的环境里活下来并且继续生长，另外也有研究发现 HIF-2α 在肾透明细胞癌里起的作用比 HIF-1α 更大 [@Keith2012]。";
const LAST: &str = "针对这一通路的药物已经进入临床。HIF-2α 抑制剂 belzutifan 在 VHL 病相关肾癌中显示出持久的疗效 [@Jonasch2021]，而 HIF 脯氨酰羟化酶抑制剂则被用来治疗肾性贫血。";
const INTRODUCTION: &str = "氧是多细胞生物能量代谢的基础，细胞需要随时感知氧的多少并作出调整。上世纪九十年代以来，对低氧诱导因子的研究逐步说明了这一感知是怎样实现的 [@Semenza2012]。";

/// What a case asks for, as it shows in the manuscript afterwards.
#[derive(Clone, Copy)]
enum Wants {
    /// A change in one place: this paragraph of the section reads otherwise, every other one as it did.
    Change(usize),
    /// The section that was empty has words, and nothing else changed.
    Fill,
    /// The section is another text altogether.
    Rewrite,
}

struct Case {
    name: &'static str,
    says: &'static str,
    /// How many times the middle paragraph stands in the section, each about another tumour: 1 is a section of some
    /// 390 characters, 46 one of some 8,000, where writing the whole text again is what a model would rather not do.
    middles: usize,
    ask: &'static str,
    wants: Wants,
}

const CASES: [Case; 5] = [
    Case { name: "sentence", says: "改一句", middles: 1, ask: "第 3 节里「HIF-1α 在常氧下被降解」说得太粗了，改成说明是经 VHL 介导的泛素化、被蛋白酶体降解。只改这一处，别的一个字都不要动。", wants: Wants::Change(0) },
    Case { name: "paragraph", says: "重写一段", middles: 1, ask: "第 3 节第二段太啰嗦，重写得紧凑一些，意思和引用不变。", wants: Wants::Change(1) },
    Case { name: "long", says: "长节里改一句", middles: 46, ask: "第 3 节里「HIF-1α 在常氧下被降解」说得太粗了，改成说明是经 VHL 介导的泛素化、被蛋白酶体降解。只改这一处，别的一个字都不要动。", wants: Wants::Change(0) },
    Case { name: "empty", says: "写空的一节", middles: 1, ask: "把第 4 节「展望」写出来，两三句话就行，不用查文献。", wants: Wants::Fill },
    Case { name: "rewrite", says: "整节重写", middles: 1, ask: "第 3 节整节重写，压缩到 150 字以内，引用保留。我确认要整节重写，直接动笔。", wants: Wants::Rewrite },
];

/// The paragraphs of the section a case works on.
fn paragraphs(case: &Case) -> Vec<String> {
    let middles = (1..=case.middles).map(|n| if case.middles == 1 { MIDDLE.to_string() } else { MIDDLE.replace("在肿瘤里", &format!("在第 {n} 类肿瘤里")) });
    std::iter::once(FIRST.to_string()).chain(middles).chain(std::iter::once(LAST.to_string())).collect()
}

/// The paragraphs of the case's section that no longer stand in `body` as they did, the one meant to change apart.
/// None where the whole section was to be written anew.
fn gone(case: &Case, body: &str) -> usize {
    let meant = match case.wants {
        Wants::Change(meant) => Some(meant),
        Wants::Fill => None,
        Wants::Rewrite => return 0,
    };
    paragraphs(case).iter().enumerate().filter(|(at, paragraph)| Some(*at) != meant && !body.contains(paragraph.as_str())).count()
}

/// How a run ended, by what the manuscript holds afterwards.
#[derive(Clone, Debug, PartialEq)]
enum Outcome {
    /// What was asked was done and nothing else changed.
    Right,
    /// So many paragraphs nobody asked it to touch are gone or read otherwise.
    Lost(usize),
    /// It called a tool that writes, and every such call was refused: the manuscript is as it was.
    Refused,
    /// It wrote nothing: answered in words, or went on reading until the steps ran out.
    Nothing,
    /// It wrote, but not what was asked (another section, the paragraph meant left as it was).
    Other(String),
    /// The request failed.
    Failed(String),
}

/// A call of a tool that writes the manuscript, as the model sent it and as it ended.
struct Wrote {
    name: String,
    sent: Value,
    /// What the tool handed back, or the sentence it refused with.
    ended: Result<Value, String>,
    /// The window would have asked the person first: a section about to lose most of its words (`compose::Replacing::drops`).
    asks: bool,
    /// After it, paragraphs nobody asked it to touch were gone from the section.
    cut: usize,
}

struct Run {
    model: String,
    case: &'static str,
    outcome: Outcome,
    wrote: Vec<Wrote>,
    steps: usize,
    seconds: f32,
    /// Tokens in and out over the steps, where the service says.
    used: (usize, usize),
}

struct Options {
    profile: Profile,
    models: Vec<String>,
    cases: Vec<&'static Case>,
    runs: usize,
    steps: usize,
    parallel: usize,
    out: Option<PathBuf>,
}

fn options() -> Result<Options, String> {
    let words: Vec<String> = std::env::args().skip(1).collect();
    let mut given: BTreeMap<String, String> = BTreeMap::new();
    let mut at = 0;
    while at < words.len() {
        let Some(name) = words[at].strip_prefix("--") else { return Err(format!("unexpected word: {}", words[at])) };
        let value = words.get(at + 1).ok_or(format!("--{name} needs a value"))?;
        given.insert(name.to_string(), value.clone());
        at += 2;
    }
    let take = |name: &str| given.get(name).map(|value| value.trim().to_string()).filter(|value| !value.is_empty());
    let number = |name: &str, otherwise: usize| take(name).map_or(Ok(otherwise), |value| value.parse::<usize>().map_err(|_| format!("--{name} is a number: {value}")));
    let protocol = match take("protocol").as_deref() {
        None | Some("openai") => Protocol::OpenAI,
        Some("anthropic") => Protocol::Anthropic,
        Some("responses") => Protocol::Responses,
        Some(other) => return Err(format!("--protocol is openai, anthropic or responses: {other}")),
    };
    let base_url = take("base").ok_or("--base is the service's address, e.g. http://127.0.0.1:3425/v1")?;
    let models: Vec<String> = take("models").ok_or("--models is a list of model names, with commas between")?.split(',').map(|model| model.trim().to_string()).filter(|model| !model.is_empty()).collect();
    let cases: Vec<&'static Case> = match take("cases") {
        None => CASES.iter().collect(),
        Some(names) => names.split(',').map(|name| CASES.iter().find(|case| case.name == name.trim()).ok_or(format!("no case {name}; there are: {}", CASES.iter().map(|case| case.name).collect::<Vec<_>>().join(", ")))).collect::<Result<_, _>>()?,
    };
    // A service on this machine is not reached through the proxy the environment names for the others.
    let local = ["127.0.0.1", "localhost", "[::1]"].iter().any(|host| base_url.contains(&format!("//{host}")));
    let profile = Profile { protocol, base_url, api_key: take("key").or_else(|| std::env::var("WISP_PROBE_KEY").ok()).unwrap_or_default(), effort: take("effort").unwrap_or_default(), proxy: take("proxy").unwrap_or_else(|| if local { "direct".into() } else { String::new() }), ..Default::default() };
    Ok(Options { profile, models, cases, runs: number("runs", 3)?, steps: number("steps", 8)?, parallel: number("parallel", 4)?.max(1), out: take("out").map(PathBuf::from) })
}

/// An error of the engine as a sentence, its causes with it.
fn said(error: impl std::fmt::Display) -> String {
    format!("{error:#}")
}

/// A library with the four papers the manuscript cites and the manuscript: an empty abstract, an introduction,
/// the section the cases work on, and an empty last section.
fn seed(root: &Path, case: &Case) -> Result<(LibraryInfo, Store, String), String> {
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
    let sections = [("摘要", String::new()), ("引言", INTRODUCTION.to_string()), ("HIF 通路的调控与靶向", paragraphs(case).join("\n\n")), ("展望", String::new())].map(|(name, body)| (name.to_string(), body));
    let made = store.create_manuscript_from_sections("HIF 通路与肿瘤", "zh", &sections, &[]).map_err(said)?;
    Ok((info, store, made.id))
}

/// One model asked one case once: the conversation of 助手写 about the section on show, until the model stops calling tools.
fn run(options: &Options, model: &str, case: &'static Case, root: &Path) -> Run {
    let started = Instant::now();
    let mut run = Run { model: model.to_string(), case: case.name, outcome: Outcome::Nothing, wrote: vec![], steps: 0, seconds: 0., used: (0, 0) };
    let ended = talk(options, model, case, root, &mut run);
    run.outcome = match ended {
        Ok(outcome) => outcome,
        Err(error) => Outcome::Failed(error),
    };
    run.seconds = started.elapsed().as_secs_f32();
    std::fs::remove_dir_all(root).ok();
    run
}

fn talk(options: &Options, model: &str, case: &'static Case, root: &Path, run: &mut Run) -> Result<Outcome, String> {
    let (info, mut store, manuscript_id) = seed(root, case)?;
    let before = store.manuscript(&manuscript_id).map_err(said)?.ok_or("the manuscript was not made")?;
    let papers = store.list_papers().map_err(said)?;
    let profile = Profile { model: model.to_string(), ..options.profile.clone() };
    let protocol = profile.protocol;
    // What the window hands the conversation that writes: the whole library, and with every question where the manuscript stands.
    let mut ctx = ai::build_context(&info.name, &info.topic, &papers.iter().collect::<Vec<_>>(), &store.list_fields().map_err(said)?, &BTreeMap::new(), &[], &Scope::Library, case.ask, &[]);
    ctx.add_sources(&papers);
    let note = compose::note(&Asked { manuscript: &before, showing: &before.sections[2].id, selection: "" });
    let mut wire = vec![ai::user_message_with(protocol, &format!("{}\n\n{note}", case.ask), &[])];
    let catalog = tools::library_catalog();
    let (offline, nothing) = (|_: &str| -> Result<String, String> { Err("这次探测不联网。".into()) }, |_: &str| -> Result<Vec<u8>, String> { Err("这次探测不联网。".into()) });
    while run.steps < options.steps {
        run.steps += 1;
        let (events, heard) = async_channel::unbounded();
        wisp_net::stream_chat(ai::chat_request(&profile, &ctx, &wire, &catalog)?, protocol, events);
        let mut turn = None;
        while let Ok(event) = heard.try_recv() {
            match event {
                ChatEvent::Done(done) => turn = Some(done),
                ChatEvent::Used(used) => run.used = (run.used.0 + used.input, run.used.1 + used.output),
                ChatEvent::Error(error) => return Err(error),
                ChatEvent::Text(_) | ChatEvent::Retrying(..) => {}
            }
        }
        let turn = turn.ok_or("the stream ended without a turn")?;
        wire.extend(turn.wire());
        if turn.calls.is_empty() { break }
        if !turn.runnable() { return Err(format!("the answer was cut off: {}", turn.stop)) }
        let mut results = vec![];
        for call in turn.calls {
            let ended = call.args.clone().and_then(|mut args| {
                // As the window does: a paper may be named by its label, and a call that names no manuscript is about the one on show.
                ai::resolve_labels(&mut args, &ctx.sources);
                let composes = tools::MANUSCRIPT.contains(&call.name.as_str()) || ["read_manuscript", "comment_manuscript"].contains(&call.name.as_str());
                if composes && args.is_object() && args["manuscript"].as_str().is_none_or(|named| named.trim().is_empty()) { args["manuscript"] = json!(manuscript_id) }
                let asks = call.name == "write_section" && compose::replacing(&store, &args, &ctx.sources).is_some_and(|replacing| replacing.drops);
                let ended = compose::call_with(&mut store, &info, &call.name, &args, &ctx.sources, &offline, &nothing, tools::Reach::default());
                if tools::MANUSCRIPT.contains(&call.name.as_str()) {
                    // What the section holds right after the call: what the person would be looking at.
                    let cut = store.manuscript(&manuscript_id).ok().flatten().and_then(|now| now.sections.get(2).map(|section| gone(case, &section.body))).unwrap_or(0);
                    run.wrote.push(Wrote { name: call.name.clone(), sent: args, ended: ended.clone(), asks, cut });
                }
                ended
            });
            if let (Err(error), true, Err(_)) = (&ended, tools::MANUSCRIPT.contains(&call.name.as_str()), &call.args) { run.wrote.push(Wrote { name: call.name.clone(), sent: Value::Null, ended: Err(error.clone()), asks: false, cut: 0 }) }
            results.push(match ended {
                Ok(value) => (call.id, tools::output_text(&value), false),
                Err(error) => (call.id, error, true),
            });
        }
        wire.extend(ai::tool_results(protocol, &results));
    }
    let after = store.manuscript(&manuscript_id).map_err(said)?.ok_or("the manuscript is gone")?;
    Ok(judge(case, &before, &after, &run.wrote))
}

/// What became of the manuscript, against what the case asked for.
fn judge(case: &Case, before: &wisp_core::manuscript::Manuscript, after: &wisp_core::manuscript::Manuscript, wrote: &[Wrote]) -> Outcome {
    let body = |manuscript: &wisp_core::manuscript::Manuscript, at: usize| manuscript.sections.get(at).map_or(String::new(), |section| section.body.trim().to_string());
    let untouched = if wrote.is_empty() { Outcome::Nothing } else if wrote.iter().all(|call| call.ended.is_err()) { Outcome::Refused } else { Outcome::Other("它调用了写稿件的工具，稿件却和原来一样".into()) };
    let held = paragraphs(case);
    let section = body(after, 2);
    let lost = gone(case, &section);
    // The outline as it was, and the other sections with it: a case is about one section.
    if after.sections.len() != before.sections.len() { return Outcome::Other("大纲变了".into()) }
    let elsewhere = |but: &[usize]| (0..before.sections.len()).filter(|at| !but.contains(at)).any(|at| body(before, at) != body(after, at));
    match case.wants {
        Wants::Change(meant) => {
            if lost > 0 { return Outcome::Lost(lost) }
            if elsewhere(&[2]) { return Outcome::Other("别的节也被改了".into()) }
            if section == body(before, 2) { return untouched }
            if section.contains(held[meant].as_str()) { return Outcome::Other("要改的那一段没有变".into()) }
            Outcome::Right
        }
        Wants::Fill => {
            if lost > 0 { return Outcome::Lost(lost) }
            if elsewhere(&[3]) { return Outcome::Other("别的节也被改了".into()) }
            if body(after, 3).is_empty() { untouched } else { Outcome::Right }
        }
        Wants::Rewrite => {
            if elsewhere(&[2]) { return Outcome::Other("别的节也被改了".into()) }
            if section == body(before, 2) { return untouched }
            if section.is_empty() { return Outcome::Lost(held.len()) }
            Outcome::Right
        }
    }
}

fn main() {
    let options = match options() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}\n\nmodel_probe --base <address> --models <a,b> [--key <key>] [--protocol openai|anthropic|responses] [--runs 3] [--cases {}] [--steps 8] [--effort <level>] [--parallel 4] [--proxy <address>|direct] [--out <file.jsonl>]", CASES.iter().map(|case| case.name).collect::<Vec<_>>().join(","));
            std::process::exit(2);
        }
    };
    let folder = std::env::temp_dir().join(format!("wisp-model-probe-{}", std::process::id()));
    let mut jobs = VecDeque::new();
    for model in &options.models {
        for case in &options.cases {
            for _ in 0..options.runs { jobs.push_back((model.clone(), *case, folder.join(jobs.len().to_string()))) }
        }
    }
    let total = jobs.len();
    eprintln!("{} 个模型 × {} 个用例 × {} 次 = {total} 次提问，同时 {} 个", options.models.len(), options.cases.len(), options.runs, options.parallel);
    let (jobs, done) = (Mutex::new(jobs), Mutex::new(Vec::<Run>::new()));
    std::thread::scope(|scope| {
        for _ in 0..options.parallel.min(total) {
            scope.spawn(|| {
                // The lock is let go of before a job runs: the guard ends with the closure that takes the job.
                while let Some((model, case, root)) = jobs.lock().ok().and_then(|mut jobs| jobs.pop_front()) {
                    let run = run(&options, &model, case, &root);
                    eprintln!("  {:<28} {:<10} {}", run.model, run.case, line(&run));
                    if let Ok(mut done) = done.lock() { done.push(run) }
                }
            });
        }
    });
    std::fs::remove_dir_all(&folder).ok();
    let done = done.into_inner().unwrap_or_default();
    if let Some(path) = &options.out {
        let lines: Vec<String> = done.iter().flat_map(|run| run.wrote.iter().map(move |call| json!({"model": run.model, "case": run.case, "outcome": line(run), "tool": call.name, "sent": call.sent, "result": call.ended.clone().unwrap_or_else(|error| json!({"refused": error})), "windowAsks": call.asks, "paragraphsGone": call.cut}).to_string())).collect();
        match std::fs::write(path, lines.join("\n") + "\n") {
            Ok(()) => eprintln!("写稿件的调用记在 {}（{} 条）", path.display(), lines.len()),
            Err(error) => eprintln!("没能写出 {}：{error}", path.display()),
        }
    }
    print!("{}", table(&options, &done));
}

/// One run in a few words.
fn line(run: &Run) -> String {
    let outcome = match &run.outcome {
        Outcome::Right => "做对".to_string(),
        Outcome::Lost(paragraphs) => format!("丢了原文（{paragraphs} 段）"),
        Outcome::Refused => format!("写稿件的调用都被拒绝：{}", run.wrote.last().and_then(|call| call.ended.clone().err()).unwrap_or_default()),
        Outcome::Nothing => "没有动笔".to_string(),
        Outcome::Other(why) => format!("其他：{why}"),
        Outcome::Failed(error) => format!("请求失败：{error}"),
    };
    // Each writing call: ✗ refused, ? the window would ask first, ! paragraphs nobody asked it to touch were gone after it.
    let tools: Vec<String> = run.wrote.iter().map(|call| format!("{}{}{}", call.name, if call.ended.is_err() { "✗" } else if call.asks { "?" } else { "" }, if call.cut > 0 { "!" } else { "" })).collect();
    format!("{outcome} · {} 步 · {:.0} 秒{}", run.steps, run.seconds, if tools.is_empty() { String::new() } else { format!(" · {}", tools.join(" ")) })
}

/// Every model's runs of every case, counted: a table in Markdown.
fn table(options: &Options, done: &[Run]) -> String {
    let mut out = String::from("\n| 模型 | 用例 | 做对 | 丢了原文 | 中途丢过 | 被拒绝 | 没动笔 | 其他 | 请求失败 | 被拒的调用 | 窗口会先问 | 平均步数 | 平均秒数 | 平均 token（入 / 出） |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for model in &options.models {
        for case in &options.cases {
            let runs: Vec<&Run> = done.iter().filter(|run| run.model == *model && run.case == case.name).collect();
            if runs.is_empty() { continue }
            let count = |is: fn(&Outcome) -> bool| runs.iter().filter(|run| is(&run.outcome)).count();
            let finished: Vec<&&Run> = runs.iter().filter(|run| !matches!(run.outcome, Outcome::Failed(_))).collect();
            let mean = |of: &dyn Fn(&Run) -> f32| if finished.is_empty() { 0. } else { finished.iter().map(|run| of(run)).sum::<f32>() / finished.len() as f32 };
            out.push_str(&format!(
                "| {model} | {} | {}/{} | {} | {} | {} | {} | {} | {} | {} | {} | {:.1} | {:.0} | {:.0} / {:.0} |\n",
                case.says,
                count(|outcome| *outcome == Outcome::Right),
                runs.len(),
                count(|outcome| matches!(outcome, Outcome::Lost(_))),
                // Runs that ended otherwise but had paragraphs gone after one of their calls.
                runs.iter().filter(|run| !matches!(run.outcome, Outcome::Lost(_)) && run.wrote.iter().any(|call| call.cut > 0)).count(),
                count(|outcome| *outcome == Outcome::Refused),
                count(|outcome| *outcome == Outcome::Nothing),
                count(|outcome| matches!(outcome, Outcome::Other(_))),
                count(|outcome| matches!(outcome, Outcome::Failed(_))),
                runs.iter().map(|run| run.wrote.iter().filter(|call| call.ended.is_err()).count()).sum::<usize>(),
                runs.iter().map(|run| run.wrote.iter().filter(|call| call.asks).count()).sum::<usize>(),
                mean(&|run| run.steps as f32),
                mean(&|run| run.seconds),
                mean(&|run| run.used.0 as f32),
                mean(&|run| run.used.1 as f32),
            ));
        }
    }
    out
}
