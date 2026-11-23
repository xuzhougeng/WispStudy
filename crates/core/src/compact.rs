//! Compacting a conversation that has outgrown the model's context window.
//!
//! The layers follow mangopi-cli's context manager, cheapest first, each
//! tried only while the request is still too long:
//!
//! 1. the tool results of earlier turns are cut to their head and tail
//!    (`trim_results`): a page of full text read three questions ago can be
//!    read again;
//! 2. what was said before the question being answered is replaced by a
//!    summary the model writes (`summary_prompt`, `summary_messages`);
//! 3. when no summary could be had, the oldest turns are left out
//!    (`drop_turns`);
//! 4. the results of the earlier steps of the question being answered are
//!    cut as well.
//!
//! Only what the model is sent changes: the conversation the person sees
//! keeps every word, with a line where the summary stands for what is above
//! it (`Compaction`). The model's own turns are never edited (their thinking
//! is signed); they are kept whole or left out whole.

use crate::ai::{self, LAST_STEP_NOTE, Profile, Protocol, Said};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The window assumed for a model nothing is known of.
pub const DEFAULT_WINDOW: usize = 128_000;
/// What a picture counts for, whatever its size.
const IMAGE_TOKENS: usize = 1_500;
/// The room Claude is asked to keep for its answer (`max_tokens` of `ai::chat_request`).
const ANSWER_ROOM: usize = 64_000;
/// The turns before the one being answered whose tool results are left whole by the first layer.
pub const RECENT_TURNS: usize = 3;
/// A tool result above this many tokens is cut, to this many characters of its head and its tail.
const RESULT_MAX: usize = 800;
const RESULT_HEAD: usize = 400;
const RESULT_TAIL: usize = 200;
/// What a cut result ends with: it is not cut twice, and the model knows what to do about it.
pub const TRIMMED: &str = "（这条较早的工具结果已压缩；需要其中的内容时请重新调用工具）";
/// What is said before the question being answered is worth a summary from this many tokens on, whatever the window.
const WORTH_SUMMARY: usize = 2_000;

/// How many tokens a model takes in: what the settings say of it, else a
/// guess by its name that errs on the small side.
pub fn window(profile: &Profile) -> usize {
    if profile.context > 0 { return profile.context }
    let model = profile.model.to_lowercase();
    if model.contains("gemini") {
        1_000_000
    } else if model.contains("claude") {
        200_000
    } else {
        DEFAULT_WINDOW
    }
}

/// The tokens a request may hold before the conversation is compacted:
/// four fifths of the window, and for Claude no more than leaves the room
/// its answer is promised.
pub fn limit(profile: &Profile) -> usize {
    let window = window(profile);
    let limit = window * 4 / 5;
    match profile.protocol {
        Protocol::Anthropic => limit.min(window.saturating_sub(ANSWER_ROOM)).max(window / 2),
        _ => limit,
    }
}

/// About how many tokens a text is: three letters of Latin text to one
/// (prose is nearer four, JSON nearer two and a half), a Chinese character
/// one. Measured on Claude and GPT models; where a service counts a request
/// itself, the app corrects this by what it says (`ChatEvent::Used`).
pub fn text_tokens(text: &str) -> usize {
    let ascii = text.bytes().filter(u8::is_ascii).count();
    let wide = text.chars().count() - ascii;
    ascii.div_ceil(3) + wide
}

/// About how many tokens a part of a request is: a message, the whole
/// wire, or the request's body. A picture counts the same whatever its
/// bytes; a signature is not read by the model.
pub fn tokens(value: &Value) -> usize {
    match value {
        Value::String(text) => text_tokens(text),
        Value::Array(items) => items.iter().map(tokens).sum(),
        Value::Object(map) => {
            if matches!(map.get("type").and_then(Value::as_str), Some("image" | "image_url" | "input_image")) { return IMAGE_TOKENS }
            3 + map
                .iter()
                .map(|(key, value)| match key.as_str() {
                    "signature" => 0,
                    "encrypted_content" => value.as_str().map_or(0, |text| text.len() / 8),
                    _ => 1 + tokens(value),
                })
                .sum::<usize>()
        }
        Value::Null => 0,
        _ => 1,
    }
}

/// The same for a run of messages.
pub fn wire_tokens(wire: &[Value]) -> usize {
    wire.iter().map(tokens).sum()
}

/// Whether a message of the wire is something the person asked: where a
/// turn begins. Tool results travel as the user's too (Claude), and so does
/// the word before the last step; neither begins a turn.
fn asks(message: &Value) -> bool {
    if message.get("role").and_then(Value::as_str) != Some("user") { return false }
    match message.get("content") {
        Some(Value::String(text)) => text != LAST_STEP_NOTE,
        Some(Value::Array(blocks)) => !blocks.iter().any(|block| block["type"] == "tool_result"),
        _ => false,
    }
}

/// Whether a message is the summary an earlier compaction left at the head of the wire.
fn is_summary(message: &Value) -> bool {
    message.get("content").and_then(Value::as_str).is_some_and(|text| text.starts_with(SUMMARY_HEAD))
}

/// Where each turn begins in the wire: a question and all that answered it.
pub fn turn_starts(wire: &[Value]) -> Vec<usize> {
    wire.iter().enumerate().filter(|(_, message)| asks(message)).map(|(i, _)| i).collect()
}

/// A long tool result as its head and its tail; `None` for one short enough or already cut.
fn shorten(text: &str) -> Option<String> {
    if text.ends_with(TRIMMED) || text_tokens(text) <= RESULT_MAX { return None }
    let count = text.chars().count();
    if count <= RESULT_HEAD + RESULT_TAIL { return None }
    let head: String = text.chars().take(RESULT_HEAD).collect();
    let tail: String = text.chars().skip(count - RESULT_TAIL).collect();
    Some(format!("{}\n……（省略 {} 字）……\n{}\n{TRIMMED}", head.trim_end(), count - RESULT_HEAD - RESULT_TAIL, tail.trim_start()))
}

/// Cut the long tool results among these messages to their head and tail.
/// The calls and the model's turns stay as they are, so the conversation is
/// as valid as before. Returns how many results were cut.
pub fn trim_results(protocol: Protocol, wire: &mut [Value]) -> usize {
    fn cut(slot: Option<&mut Value>) -> usize {
        let Some(slot) = slot else { return 0 };
        let Some(short) = slot.as_str().and_then(shorten) else { return 0 };
        *slot = Value::String(short);
        1
    }
    let mut done = 0;
    for message in wire {
        // `get_mut`, never an index: indexing would give a message a field it does not have.
        match protocol {
            Protocol::Anthropic => {
                for block in message.get_mut("content").and_then(Value::as_array_mut).into_iter().flatten() {
                    if block["type"] == "tool_result" { done += cut(block.get_mut("content")) }
                }
            }
            Protocol::OpenAI => {
                if message["role"] == "tool" { done += cut(message.get_mut("content")) }
            }
            Protocol::Responses => {
                if message["type"] == "function_call_output" { done += cut(message.get_mut("output")) }
            }
        }
    }
    done
}

/// What the rules alone made of a conversation that was too long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Room {
    /// It fits now, or nothing more can be done about it.
    Done,
    /// It is still too long, and what was said before the question being
    /// answered is worth a summary: the caller asks the model for one, puts
    /// it in (`summary_messages`) and calls `tighten`.
    Summarise,
}

/// The first layer: cut the tool results of the turns before the last few,
/// and say whether a summary is the next thing to try. `over` says whether
/// a wire is still too long for the model; `worth` is how many tokens what
/// was said before must hold for a summary of it to be asked for (a tenth
/// of what the model takes: less buys too little room for a request).
pub fn make_room(protocol: Protocol, wire: &mut [Value], worth: usize, over: impl Fn(&[Value]) -> bool) -> Room {
    let starts = turn_starts(wire);
    if starts.len() > RECENT_TURNS {
        trim_results(protocol, &mut wire[..starts[starts.len() - RECENT_TURNS]]);
        if !over(wire) { return Room::Done }
    }
    let current = starts.last().copied().unwrap_or(0);
    let earlier = &wire[..current];
    // A summary and nothing since is not summarised again.
    let fresh = earlier.len() > 2 || earlier.first().is_some_and(|first| !is_summary(first));
    if fresh && wire_tokens(earlier) >= worth.max(WORTH_SUMMARY) { return Room::Summarise }
    tighten(protocol, wire, over);
    Room::Done
}

/// The last layer: cut the results of every step but the latest, the
/// earlier steps of the question being answered among them. What the model
/// is about to read stays whole.
pub fn tighten(protocol: Protocol, wire: &mut [Value], over: impl Fn(&[Value]) -> bool) {
    if !over(wire) { return }
    // The latest results are the run of messages at the end that the model did not write.
    let written = |message: &Value| message["role"] == "assistant" || matches!(message["type"].as_str(), Some("function_call" | "reasoning" | "message"));
    let latest = wire.iter().rposition(written).map_or(0, |i| i + 1);
    trim_results(protocol, &mut wire[..latest]);
}

/// The fallback when no summary could be had: leave out the oldest turns,
/// whole, until the wire fits or only the turn being answered is left. A
/// summary at the head stays. Returns how many turns went.
pub fn drop_turns(wire: &mut Vec<Value>, over: impl Fn(&[Value]) -> bool) -> usize {
    let head = if wire.first().is_some_and(is_summary) { turn_starts(wire).get(1).copied().unwrap_or(wire.len()) } else { 0 };
    let mut dropped = 0;
    while over(wire) {
        let starts: Vec<usize> = turn_starts(wire).into_iter().filter(|&start| start >= head).collect();
        if starts.len() < 2 { break }
        wire.drain(starts[0]..starts[1]);
        dropped += 1;
    }
    dropped
}

/// A compaction as a conversation keeps it: the model is given the summary
/// in place of the first `at` things said, which the person still sees.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Compaction {
    /// How many of the conversation's items the summary stands for.
    pub at: usize,
    /// Empty when no summary could be had and the items were only left out.
    pub summary: String,
    /// About how many tokens that part of the conversation was, and what stands for it now.
    pub before: usize,
    pub after: usize,
    /// How many turns among them were left out without a summary of them.
    pub dropped: usize,
}

/// What the model that writes the summary is told it is doing.
pub const SUMMARY_SYSTEM: &str = "你负责把用户与文献助手的一段对话压缩成摘要。这份摘要会代替原来的对话交给助手，让它不必重读原对话就能接着工作。只输出摘要本身，不调用工具。";

/// What a summary on the wire begins with.
pub const SUMMARY_HEAD: &str = "（系统提示）这次对话较早的部分已经压缩，下面是它的摘要。当时读过的全文页面和工具结果不在这里，需要时请重新调用工具查阅。请在这份摘要的基础上继续。\n\n";
const SUMMARY_ACK: &str = "好的，我会在这份摘要的基础上继续。";

const SUMMARY_ASK: &str = "把上面的对话记录压缩成一份摘要，按这个结构写：\n\n\
1. 用户的目的和要求：用户想做成什么；对范围、语言、格式、篇幅等提过的要求和偏好逐条保留，不要概括掉。\n\
2. 用户说过的话：按顺序列出用户的每一次提问或指示，各用一句话。\n\
3. 进行中的流程：用户用命令启动了分步骤的工作（如综述、核对）时，写明有哪几步、每一步的要求、已经做到哪一步、哪些内容已经得到用户确认。没有就写「无」。\n\
4. 涉及的文献：出现过的文献标签（如 P3）和题目，每篇读到了什么；标明读的是全文、摘要还是只有题录。\n\
5. 已有的结论和内容：助手给出的要点、数字、比较，和已经写成的文字（大纲、段落）；后面还会用到的成文内容尽量原样保留。引用保持原来的写法，如 [P3 p.5]，不改动标签和页码。\n\
6. 做过的操作：检索、导入、整理、写入稿件、生成文件等改动了文献库或磁盘的操作及其结果（数量、文件路径）。\n\
7. 未完成的事和下一步：还没答完的问题、答应了还没做的事、用户最近在问什么。\n\n\
要求：只依据对话记录，不补充记录里没有的内容，不确定的写「不确定」。用对话所用的语言写。不要寒暄，直接从「1.」开始。";

/// A long text as its beginning and its end.
fn clip(text: &str, cap: usize) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= cap { return text.to_string() }
    let (head, tail) = (cap * 2 / 3, cap / 3);
    format!("{}\n……（中间省略 {} 字）……\n{}", text.chars().take(head).collect::<String>().trim_end(), count - head - tail, text.chars().skip(count - tail).collect::<String>().trim_start())
}

/// The conversation as a record to summarise, no part of it longer than `cap` characters.
fn record(items: &[Said], cap: usize) -> String {
    let mut out = String::new();
    for item in items {
        match item {
            Said::User { text, images, files, prompt } => {
                // A command is what it stood for when it was sent: its steps are what the model was asked to follow.
                let asked = if text.trim_start().starts_with('/') && !prompt.is_empty() { prompt } else { text };
                out.push_str(&format!("【用户】{}", clip(asked, cap)));
                if !images.is_empty() { out.push_str(&format!("\n（附 {} 张图片）", images.len())) }
                if !files.is_empty() { out.push_str(&format!("\n（附文件：{}）", files.join("、"))) }
            }
            Said::Answer { text } => out.push_str(&format!("【助手】{}", clip(text, cap))),
            Said::Tool { call, result, failed } => out.push_str(&format!("【工具】{call} → {}{}", if *failed { "失败：" } else { "" }, clip(result, cap.min(400)))),
        }
        out.push_str("\n\n");
    }
    out
}

/// The request for a summary of what was said: the record, an earlier
/// summary before it when the conversation was compacted before, and what
/// the summary is to hold. `room` is how many characters the record may
/// take: long answers are clipped first, then the oldest of it goes.
pub fn summary_prompt(items: &[Said], earlier: &str, room: usize) -> String {
    let mut cap = 12_000;
    let mut text = record(items, cap);
    while text.chars().count() > room && cap > 300 {
        cap /= 2;
        text = record(items, cap);
    }
    let count = text.chars().count();
    if count > room { text = format!("（更早的记录过长，已省略）\n\n{}", text.chars().skip(count - room).collect::<String>()) }
    let before = if earlier.trim().is_empty() { String::new() } else { format!("【此前的摘要】（对话更早的部分此前压缩过；把它和后面的对话合并成一份新的摘要，其中仍然有用的内容不要丢）\n{}\n\n", earlier.trim()) };
    format!("对话记录：\n\n{before}{}{SUMMARY_ASK}", text)
}

/// The summary out of the model's reply: what is between `<summary>` tags
/// when it wrote them, without an `<analysis>` it thought aloud in. `None`
/// for a reply that holds nothing to go on with.
pub fn read_summary(reply: &str) -> Option<String> {
    let mut text = reply.trim();
    if let Some((_, rest)) = text.split_once("</analysis>") { text = rest.trim() }
    if let Some((_, rest)) = text.split_once("<summary>") { text = rest.split("</summary>").next().unwrap_or(rest).trim() }
    (text.chars().count() >= 20).then(|| text.to_string())
}

/// A summary as the wire carries it, in place of what it stands for: said
/// to the model as the person's, and acknowledged, so the turns after it
/// still alternate.
pub fn summary_messages(summary: &str) -> Vec<Value> {
    vec![ai::user_message(&format!("{SUMMARY_HEAD}{}", summary.trim())), ai::assistant_message(SUMMARY_ACK)]
}

/// Whether a service's error says the request was too long for the model:
/// each has its own words for it.
pub fn context_exceeded(error: &str) -> bool {
    let error = error.to_lowercase();
    ["context length", "context window", "context_length", "context limit", "maximum context", "prompt is too long", "input is too long", "too many tokens", "too many input tokens", "exceeds the maximum", "reduce the length", "上下文长度", "上下文窗口", "超出了模型", "输入过长"].iter().any(|words| error.contains(words))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn long(word: &str) -> String {
        word.repeat(4000)
    }

    /// A turn with one tool call on each protocol: the question, the model's call, the result, the answer.
    fn turn(protocol: Protocol, n: usize, result: &str) -> Vec<Value> {
        let id = format!("call_{n}");
        let mut wire = vec![ai::user_message(&format!("问题 {n}"))];
        match protocol {
            Protocol::Anthropic => {
                wire.push(json!({"role": "assistant", "content": [{"type": "thinking", "thinking": "想一想", "signature": "sig"}, {"type": "tool_use", "id": id, "name": "read_fulltext", "input": {}}]}));
                wire.extend(ai::tool_results(protocol, &[(id, result.to_string(), false)]));
                wire.push(json!({"role": "assistant", "content": [{"type": "text", "text": format!("回答 {n}")}]}));
            }
            Protocol::OpenAI => {
                wire.push(json!({"role": "assistant", "content": null, "tool_calls": [{"id": id, "type": "function", "function": {"name": "read_fulltext", "arguments": "{}"}}]}));
                wire.extend(ai::tool_results(protocol, &[(id, result.to_string(), false)]));
                wire.push(ai::assistant_message(&format!("回答 {n}")));
            }
            Protocol::Responses => {
                wire.push(json!({"type": "reasoning", "id": format!("rs_{n}"), "summary": [], "encrypted_content": "gAAA"}));
                wire.push(json!({"type": "function_call", "call_id": id, "name": "read_fulltext", "arguments": "{}"}));
                wire.extend(ai::tool_results(protocol, &[(id, result.to_string(), false)]));
                wire.push(json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": format!("回答 {n}")}]}));
            }
        }
        wire
    }

    fn result_of(protocol: Protocol, wire: &[Value], turn: usize) -> String {
        let id = format!("call_{turn}");
        wire.iter()
            .find_map(|message| match protocol {
                Protocol::Anthropic => message["content"].as_array()?.iter().find(|block| block["tool_use_id"] == id.as_str())?["content"].as_str(),
                Protocol::OpenAI => message["content"].as_str().filter(|_| message["tool_call_id"] == id.as_str()),
                Protocol::Responses => message["output"].as_str().filter(|_| message["call_id"] == id.as_str()),
            })
            .unwrap()
            .to_string()
    }

    #[test]
    fn tokens_are_estimated() {
        assert_eq!(text_tokens("abcdefgh"), 3);
        assert_eq!(text_tokens("根系感知土壤"), 6);
        // A picture counts the same whatever its bytes, a signature nothing.
        let picture = json!({"role": "user", "content": [{"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "A".repeat(400_000)}}]});
        assert!(tokens(&picture) < 2 * IMAGE_TOKENS);
        assert_eq!(tokens(&json!({"type": "thinking", "thinking": "abcd", "signature": "x".repeat(900)})), tokens(&json!({"type": "thinking", "thinking": "abcd"})));
    }

    #[test]
    fn window_and_limit() {
        let profile = |protocol, model: &str, context| Profile { protocol, model: model.into(), context, ..Default::default() };
        assert_eq!(limit(&profile(Protocol::OpenAI, "deepseek-v4-pro", 0)), 102_400);
        // Claude keeps the room its answer is promised.
        assert_eq!(limit(&profile(Protocol::Anthropic, "claude-opus-5-5", 0)), 136_000);
        assert_eq!(limit(&profile(Protocol::Anthropic, "claude-opus-5-5", 1_000_000)), 800_000);
        assert_eq!(limit(&profile(Protocol::OpenAI, "local", 8_000)), 6_400);
        assert_eq!(window(&profile(Protocol::OpenAI, "gemini-3-pro", 0)), 1_000_000);
    }

    #[test]
    fn turns_begin_at_questions() {
        for protocol in [Protocol::Anthropic, Protocol::OpenAI, Protocol::Responses] {
            let mut wire = turn(protocol, 1, "页面");
            let second = wire.len();
            wire.extend(turn(protocol, 2, "页面"));
            // The word before the last step is not a question, wherever the protocol puts it.
            let noted = ai::with_note(protocol, ai::tool_results(protocol, &[("call_9".into(), "结果".into(), false)]), LAST_STEP_NOTE);
            wire.extend(noted);
            assert_eq!(turn_starts(&wire), vec![0, second], "{protocol:?}");
        }
    }

    #[test]
    fn old_results_are_cut_and_the_rest_stays() {
        for protocol in [Protocol::Anthropic, Protocol::OpenAI, Protocol::Responses] {
            let page = format!("开头{}结尾", long("页"));
            let mut wire: Vec<Value> = (1..=5).flat_map(|n| turn(protocol, n, &page)).collect();
            let before = wire.clone();
            let limit = wire_tokens(&wire) - 1;
            assert_eq!(make_room(protocol, &mut wire, 0, |wire| wire_tokens(wire) > limit), Room::Done, "{protocol:?}");
            // The two oldest turns' results are cut; the last three are whole.
            for n in 1..=2 {
                let cut = result_of(protocol, &wire, n);
                assert!(cut.starts_with("开头") && cut.ends_with(TRIMMED) && cut.contains("结尾") && cut.chars().count() < 800, "{protocol:?} {cut}");
            }
            for n in 3..=5 {
                assert_eq!(result_of(protocol, &wire, n), page);
            }
            // Nothing else changed: the same messages, the model's turns as it sent them.
            assert_eq!(wire.len(), before.len());
            for (now, was) in wire.iter().zip(&before) {
                if was["role"] == "assistant" || was["type"] == "reasoning" || was["type"] == "function_call" { assert_eq!(now, was) }
            }
            // Cut once: a second pass over the same turns finds nothing to do.
            let old = turn_starts(&wire)[2];
            assert_eq!(trim_results(protocol, &mut wire[..old]), 0);
        }
    }

    #[test]
    fn a_summary_is_asked_for_when_cutting_is_not_enough() {
        let protocol = Protocol::OpenAI;
        let mut wire: Vec<Value> = (1..=5).flat_map(|n| turn(protocol, n, &long("页"))).collect();
        // Too little said for the room a summary would make: the results are cut instead, and the turns stay.
        let mut small = wire.clone();
        assert_eq!(make_room(protocol, &mut small, 1_000_000, |_| true), Room::Done);
        assert_eq!((small.len(), result_of(protocol, &small, 4).ends_with(TRIMMED)), (wire.len(), true));
        assert_eq!(make_room(protocol, &mut wire, 0, |_| true), Room::Summarise);
        // The summary in, what was before the question being answered is gone and its turn is whole.
        let current = *turn_starts(&wire).last().unwrap();
        let mut compacted = summary_messages("1. 用户想了解根系。");
        compacted.extend(wire.drain(current..));
        assert_eq!(compacted.len(), 2 + 4);
        assert_eq!(result_of(protocol, &compacted, 5), long("页"));
        // A summary and nothing since is not summarised again; the turn's own earlier results go next.
        let id = "call_6".to_string();
        compacted.push(json!({"role": "assistant", "content": null, "tool_calls": [{"id": id, "type": "function", "function": {"name": "read_fulltext", "arguments": "{}"}}]}));
        compacted.extend(ai::tool_results(protocol, &[(id, long("新"), false)]));
        assert_eq!(make_room(protocol, &mut compacted, 0, |_| true), Room::Done);
        assert!(result_of(protocol, &compacted, 5).ends_with(TRIMMED));
        assert_eq!(result_of(protocol, &compacted, 6), long("新"));
        assert!(compacted[0]["content"].as_str().unwrap().starts_with(SUMMARY_HEAD));
    }

    #[test]
    fn oldest_turns_are_dropped_whole() {
        for protocol in [Protocol::Anthropic, Protocol::OpenAI, Protocol::Responses] {
            let mut wire: Vec<Value> = (1..=4).flat_map(|n| turn(protocol, n, "页面")).collect();
            let two = wire_tokens(&wire) / 2 + 2;
            assert_eq!(drop_turns(&mut wire, |wire| wire_tokens(wire) > two), 2, "{protocol:?}");
            assert_eq!(wire, (3..=4).flat_map(|n| turn(protocol, n, "页面")).collect::<Vec<_>>());
            // The turn being answered is never dropped.
            assert_eq!(drop_turns(&mut wire, |_| true), 1);
            assert_eq!(wire, turn(protocol, 4, "页面"));
            // A summary at the head stays where it is.
            let mut wire: Vec<Value> = summary_messages("摘要").into_iter().chain((1..=3).flat_map(|n| turn(protocol, n, "页面"))).collect();
            assert_eq!(drop_turns(&mut wire, |_| true), 2);
            assert_eq!(wire, summary_messages("摘要").into_iter().chain(turn(protocol, 3, "页面")).collect::<Vec<_>>());
        }
    }

    #[test]
    fn the_record_to_summarise() {
        let items = vec![
            Said::User { text: "/review 根系".into(), images: vec![], files: vec![], prompt: "围绕下面的主题撰写综述：根系\n\n1. 确认写作要求".into() },
            Said::Tool { call: "检索全文：auxin".into(), result: "3 篇命中".into(), failed: false },
            Said::Answer { text: "要求如下 [P3 p.5]。".into() },
            Said::User { text: "这张图说明什么？".into(), images: vec!["I1".into()], files: vec!["/tmp/a.docx".into()], prompt: "这张图说明什么？\n\n[当前综述记录]".into() },
            Said::Answer { text: long("长") },
        ];
        let prompt = summary_prompt(&items, "", 100_000);
        // A command goes as what it stood for; a typed question as typed.
        assert!(prompt.contains("【用户】围绕下面的主题撰写综述：根系") && prompt.contains("【用户】这张图说明什么？\n（附 1 张图片）\n（附文件：/tmp/a.docx）") && !prompt.contains("[当前综述记录]"));
        assert!(prompt.contains("【工具】检索全文：auxin → 3 篇命中") && prompt.contains("【助手】要求如下 [P3 p.5]。") && prompt.ends_with(SUMMARY_ASK) && !prompt.contains("此前的摘要"));
        // Little room: the long answer is clipped, the short parts stay.
        let small = summary_prompt(&items, "更早谈过茎。", 1_500);
        assert!(small.contains("中间省略") && small.contains("【助手】要求如下 [P3 p.5]。") && small.contains("【此前的摘要】") && small.contains("更早谈过茎。"));
        assert!(small.chars().count() < 1_500 + SUMMARY_ASK.chars().count() + 200);
    }

    #[test]
    fn the_summary_is_read_from_the_reply() {
        assert_eq!(read_summary("<analysis>先想想</analysis>\n<summary>\n1. 用户的目的和要求：了解根系如何感知土壤紧实。\n</summary>").unwrap(), "1. 用户的目的和要求：了解根系如何感知土壤紧实。");
        assert_eq!(read_summary("  1. 用户的目的和要求：了解根系如何感知土壤紧实。 ").unwrap(), "1. 用户的目的和要求：了解根系如何感知土壤紧实。");
        assert_eq!(read_summary("好的。"), None);
        let wire = summary_messages(" 摘要正文 ");
        assert_eq!((wire[0]["role"].as_str(), wire[1]["role"].as_str()), (Some("user"), Some("assistant")));
        assert_eq!(wire[0]["content"], json!(format!("{SUMMARY_HEAD}摘要正文")));
        assert!(is_summary(&wire[0]) && asks(&wire[0]));
    }

    #[test]
    fn services_say_too_long_in_their_own_words() {
        for error in [
            "请求被拒绝（HTTP 400）：This model's maximum context length is 131072 tokens. However, you requested 140000 tokens",
            "请求被拒绝（HTTP 400）：prompt is too long: 213042 tokens > 200000 maximum",
            "请求被拒绝（HTTP 400）：Your input exceeds the context window of this model.",
            "请求被拒绝（HTTP 400）：Input is too long for requested model.",
        ] {
            assert!(context_exceeded(error), "{error}");
        }
        assert!(!context_exceeded("API Key 无效或没有权限（HTTP 401）：invalid x-api-key"));
    }
}
