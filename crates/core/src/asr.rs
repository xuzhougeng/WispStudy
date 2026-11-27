//! 语音输入 (ASR): speech-to-text through a service that answers in JSON,
//! abstracted like `websearch` and `ocr`. Every engine is one entry of
//! `ENGINES` and an arm in `request` and `read`; nothing else knows which
//! engines there are. Nothing here sends a request: `wisp_net::transcribe`
//! / `wisp_net::send_audio` does.

use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A service that turns speech into text.
#[derive(Debug, PartialEq)]
pub struct Engine {
    /// What settings and callers call it.
    pub key: &'static str,
    pub name: &'static str,
    /// The environment variable its key is read from when none is stored.
    pub key_env: &'static str,
    /// Where a key is had.
    pub site: &'static str,
    /// The largest audio file it takes, in bytes; 0 is no limit of its own.
    pub bytes: usize,
    /// The longest clip it takes, in seconds; 0 is no limit of its own.
    pub seconds: usize,
}

/// The engines, in the order 自动 tries them. A new engine is a row here and an arm in `request` and `read`.
pub const ENGINES: [Engine; 1] = [
    // https://api-docs.deepseek.com/zh-cn/guides/asr/: the key of DeepSeek's chat API, up to 120 s / 8 MiB.
    Engine { key: "deepseek", name: "DeepSeek", key_env: "DEEPSEEK_API_KEY", site: "https://platform.deepseek.com/api_keys", bytes: 8 << 20, seconds: 120 },
];

/// `AsrSettings::engine` for the first engine that has a key.
pub const AUTO: &str = "";
/// `AsrSettings::engine` for no voice input at all.
pub const OFF: &str = "off";
/// Sample rate of the 16-bit mono PCM WAV sent to ASR engines.
pub const SAMPLE_RATE: u32 = 16_000;

/// 设置 › AI › 任务 › 语音输入.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AsrSettings {
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

/// The key an engine would be asked with now, the proxy and the address that go with it; an empty key when it has none.
/// Stored for 语音输入 first; for DeepSeek then the DeepSeek model service's (stored or `DEEPSEEK_API_KEY`); then the engine's variable.
pub fn key_for(settings: &Settings, engine: &'static Engine) -> (String, String, String) {
    key_from(settings, engine, &|name| std::env::var(name).ok())
}

fn key_from(settings: &Settings, engine: &'static Engine, env: &dyn Fn(&str) -> Option<String>) -> (String, String, String) {
    let stored = settings.asr.keys.get(engine.key).map(|key| key.trim()).unwrap_or("");
    if !stored.is_empty() { return (stored.to_string(), String::new(), String::new()) }
    if engine.key == "deepseek" && let Some(provider) = settings.providers.iter().find(|p| (p.preset == "deepseek" || crate::speech::is_service(&p.base_url)) && !p.key().is_empty()) {
        return (provider.key(), provider.proxy.clone(), provider.base_url.clone());
    }
    (env(engine.key_env).map(|key| key.trim().to_string()).unwrap_or_default(), String::new(), String::new())
}

/// The engine speech goes to: `asked` (an `Engine::key`) when given, else the one chosen in settings, else
/// the first with a key. The error is a sentence saying where a key goes.
pub fn ready(settings: &Settings, asked: &str) -> Result<Ready, String> {
    ready_from(settings, asked, &|name| std::env::var(name).ok())
}

fn ready_from(settings: &Settings, asked: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Ready, String> {
    let chosen = settings.asr.engine.trim();
    if asked.trim().is_empty() && chosen == OFF { return Err("语音输入已在「设置 › AI › 任务」关闭。".into()) }
    let wanted = if asked.trim().is_empty() { chosen } else { asked.trim() };
    if !wanted.is_empty() {
        let Some(engine) = engine(wanted) else { return Err(format!("没有这个语音识别引擎：{wanted}。可用的有 {}。", names())) };
        let (key, proxy, base) = key_from(settings, engine, env);
        if key.is_empty() { return Err(format!("{} 语音输入需要 API Key：在{}填写，或设置环境变量 {}（在 {} 申请）。", engine.name, if engine.key == "deepseek" { "「设置 › AI › 模型」的 DeepSeek 服务里" } else { "「设置 › AI › 任务 › 语音输入」" }, engine.key_env, engine.site)) }
        return Ok(Ready { engine, key, proxy, base });
    }
    ENGINES.iter().find_map(|engine| {
        let (key, proxy, base) = key_from(settings, engine, env);
        (!key.is_empty()).then_some(Ready { engine, key, proxy, base })
    })
    .ok_or_else(|| format!("语音输入需要 {} 之一的 API Key：在「设置 › AI › 任务 › 语音输入」填写；DeepSeek 用的是「设置 › AI › 模型」里 DeepSeek 服务的 Key。", names()))
}

fn names() -> String {
    ENGINES.iter().map(|engine| engine.key).collect::<Vec<_>>().join("、")
}

/// Mix interleaved `channels` to mono, linearly resample from `from_rate` to `to_rate`, and quantize to 16-bit PCM.
pub fn resample_mono_i16(samples: &[f32], channels: usize, from_rate: u32, to_rate: u32) -> Vec<i16> {
    if samples.is_empty() || channels == 0 || from_rate == 0 || to_rate == 0 { return vec![] }
    let frames = samples.len() / channels;
    if frames == 0 { return vec![] }
    let mono = |frame: usize| -> f32 {
        let base = frame * channels;
        samples[base..base + channels].iter().copied().sum::<f32>() / channels as f32
    };
    let quantize = |sample: f32| -> i16 {
        let clamped = sample.clamp(-1., 1.);
        (clamped * i16::MAX as f32).round() as i16
    };
    if from_rate == to_rate {
        return (0..frames).map(|frame| quantize(mono(frame))).collect();
    }
    let out_frames = ((frames as u64 * to_rate as u64) / from_rate as u64).max(1) as usize;
    let ratio = from_rate as f64 / to_rate as f64;
    (0..out_frames)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = (pos as usize).min(frames - 1);
            let frac = (pos - idx as f64) as f32;
            let a = mono(idx);
            let b = mono((idx + 1).min(frames - 1));
            quantize(a + (b - a) * frac)
        })
        .collect()
}

/// A 44-byte RIFF/WAVE 16-bit mono PCM file around `pcm`.
pub fn wav(pcm: &[i16], rate: u32) -> Vec<u8> {
    let data = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for sample in pcm {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// Duration in milliseconds of `sample_count` mono samples at `rate`.
pub fn duration_ms(sample_count: usize, rate: u32) -> u64 {
    if rate == 0 { 0 } else { (sample_count as u64 * 1000) / rate as u64 }
}

/// An audio clip on its way to an engine: a POST whose body is a form with the file in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Upload {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub proxy: String,
}

/// Where DeepSeek's ASR is asked: at the root of the host its key is for, whatever path chat has there.
fn deepseek_endpoint(base: &str) -> String {
    if base.trim().is_empty() { return "https://api.deepseek.com/v1/tools/asr".into() }
    crate::speech::endpoint(base).replace("/v1/tools/tts", "/v1/tools/asr")
}

/// The request that sends `audio` (named `name`) to an engine. What the engine would refuse is said before anything is sent.
pub fn request(ready: &Ready, name: &str, audio: &[u8]) -> Result<Upload, String> {
    if audio.is_empty() { return Err("没有录到声音。".into()) }
    let engine = ready.engine;
    if engine.bytes > 0 && audio.len() > engine.bytes {
        return Err(format!("{} 语音输入最多接受 {} MB 的音频，这段有 {} MB：请缩短说话时间。", engine.name, engine.bytes >> 20, audio.len().div_ceil(1 << 20)));
    }
    match engine.key {
        "deepseek" => {
            let (kind, body) = form(name, audio);
            Ok(Upload { url: deepseek_endpoint(&ready.base), headers: vec![("Authorization".into(), format!("Bearer {}", ready.key)), ("Content-Type".into(), kind)], body, proxy: ready.proxy.clone() })
        }
        other => Err(format!("没有这个语音识别引擎：{other}。")),
    }
}

/// A `multipart/form-data` body carrying one audio `file` field, and the content type that names its boundary.
fn form(name: &str, audio: &[u8]) -> (String, Vec<u8>) {
    let boundary = format!("wisp-study-{:x}-{:x}", crate::time::now_millis(), audio.len());
    let mut body = Vec::with_capacity(audio.len() + 256);
    let plain: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
    let plain = if plain.trim_matches('_').is_empty() || !plain.contains('.') { "speech.wav".to_string() } else { plain };
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{plain}\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes());
    body.extend_from_slice(audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// What came back from transcribing an audio clip.
#[derive(Clone, Debug, PartialEq)]
pub struct Transcript {
    pub engine: &'static Engine,
    pub text: String,
    pub duration_ms: Option<u64>,
}

/// The transcript in an engine's answer, or what went wrong in a sentence: `status` is the HTTP status.
pub fn read(engine: &'static Engine, status: u16, body: &str) -> Result<Transcript, String> {
    let value: Option<Value> = serde_json::from_str(body).ok();
    let failure = value.as_ref().map(|v| &v["error"]).filter(|error| !error.is_null());
    if !(200..300).contains(&status) || failure.is_some() {
        let error = failure.unwrap_or(&Value::Null);
        let said = error["message"].as_str().or(error.as_str()).or(value.as_ref().and_then(|v| v["message"].as_str())).map(str::trim).filter(|s| !s.is_empty()).map(String::from).unwrap_or_else(|| if value.is_some() { String::new() } else { body.trim().chars().take(300).collect() });
        let hint = match status {
            200..300 => "没有识别成",
            400 => "音频无法识别（格式或采样率不受支持）",
            401 => "API Key 无效",
            402 => "账户余额不足",
            408 | 504 => "语音识别超时，请稍后再试",
            413 => "音频文件太大",
            429 => "请求过于频繁或额度不足，请稍后再试",
            500..=599 => "语音识别服务暂时不可用，请稍后再试",
            _ => "语音识别请求有误",
        };
        let said = if said.is_empty() { String::new() } else { format!("：{said}") };
        let code = if (200..300).contains(&status) { String::new() } else { format!("（HTTP {status}）") };
        return Err(format!("{} 语音输入：{hint}{code}{said}", engine.name));
    }
    let Some(value) = value else { return Err(format!("{} 语音输入返回的不是 JSON。", engine.name)) };
    let Some(text) = value["text"].as_str() else { return Err(format!("{} 语音输入的返回内容无法识别。", engine.name)) };
    Ok(Transcript { engine, text: text.trim().to_string(), duration_ms: value["duration_ms"].as_u64() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;

    fn settings_with(engine: &str, keys: &[(&str, &str)]) -> Settings {
        let mut settings = Settings::default();
        settings.providers.clear();
        settings.asr = AsrSettings { engine: engine.into(), keys: keys.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() };
        settings
    }

    fn ready(settings: &Settings, asked: &str, env: &[(&str, &str)]) -> Result<Ready, String> {
        let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        ready_from(settings, asked, &|name| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
    }

    #[test]
    fn the_engine_comes_from_the_call_the_settings_or_the_deepseek_key() {
        let empty = settings_with(AUTO, &[]);
        assert!(ready(&empty, "", &[]).unwrap_err().contains("设置 › AI › 任务 › 语音输入"));
        assert_eq!(ready(&empty, "", &[("DEEPSEEK_API_KEY", "dk")]).unwrap().engine.key, "deepseek");
        assert!(ready(&empty, "google", &[]).unwrap_err().contains("deepseek"));
        assert!(ready(&empty, "deepseek", &[]).unwrap_err().contains("DEEPSEEK_API_KEY"));

        // Switched off: refuses unless named explicitly.
        let off = settings_with(OFF, &[("deepseek", "dk")]);
        assert!(ready(&off, "", &[]).unwrap_err().contains("关闭"));
        assert_eq!(ready(&off, "deepseek", &[]).unwrap().key, "dk");

        // Borrows DeepSeek model provider's key, proxy, and base URL.
        let mut borrowed = settings_with(AUTO, &[]);
        borrowed.providers = vec![Provider { preset: "deepseek".into(), base_url: "http://127.0.0.1:8787/v1".into(), api_key: "sk-ds".into(), proxy: "direct".into(), ..Default::default() }];
        let found = ready(&borrowed, "", &[]).unwrap();
        assert_eq!((found.key.as_str(), found.proxy.as_str(), found.base.as_str()), ("sk-ds", "direct", "http://127.0.0.1:8787/v1"));

        // Explicit key overrides borrowed provider key.
        borrowed.asr.keys.insert("deepseek".into(), "sk-asr".into());
        let found = ready(&borrowed, "", &[]).unwrap();
        assert_eq!((found.key.as_str(), found.proxy.as_str(), found.base.as_str()), ("sk-asr", "", ""));
    }

    #[test]
    fn audio_is_mixed_resampled_and_wrapped_as_wav() {
        // Stereo 32 kHz -> mono 16 kHz halves the frame count and averages L+R.
        let stereo = vec![0.5_f32, -0.5, 1.0, 1.0, 0.0, 0.0, -1.0, -1.0];
        let pcm = resample_mono_i16(&stereo, 2, 32_000, 16_000);
        assert_eq!(pcm, vec![0, 0]);
        assert_eq!(resample_mono_i16(&[], 1, 16_000, 16_000), Vec::<i16>::new());
        assert_eq!(resample_mono_i16(&[0.5], 0, 16_000, 16_000), Vec::<i16>::new());

        let bytes = wav(&[0, 16384, -16384], 16_000);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes(bytes[34..36].try_into().unwrap()), 16);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 6);
        assert_eq!(bytes.len(), 50);
        assert_eq!(duration_ms(16_000, 16_000), 1000);
        assert_eq!(duration_ms(16_000, 0), 0);
    }

    #[test]
    fn requests_and_responses_are_built_and_parsed() {
        let ds = Ready { engine: engine("deepseek").unwrap(), key: "sk-1".into(), proxy: "direct".into(), base: "https://api.deepseek.com/v1".into() };
        let upload = request(&ds, "录音.wav", b"RIFFtest").unwrap();
        assert_eq!(upload.url, "https://api.deepseek.com/v1/tools/asr");
        assert_eq!(upload.proxy, "direct");
        assert!(upload.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer sk-1"));
        let body = String::from_utf8_lossy(&upload.body);
        assert!(body.contains("name=\"file\"; filename=\"__.wav\"") && body.contains("RIFFtest"));
        assert!(request(&ds, "a.wav", &[]).unwrap_err().contains("没有录到声音"));
        let too_big = vec![0u8; (8 << 20) + 1];
        assert!(request(&ds, "a.wav", &too_big).unwrap_err().contains("8 MB"));

        let ok = read(ds.engine, 200, r#"{"request_id": "r1", "text": " 帮我找文献 ", "duration_ms": 2350}"#).unwrap();
        assert_eq!((ok.text.as_str(), ok.duration_ms), ("帮我找文献", Some(2350)));
        assert_eq!(read(ds.engine, 401, r#"{"error": {"message": "Unauthorized"}}"#).unwrap_err(), "DeepSeek 语音输入：API Key 无效（HTTP 401）：Unauthorized");
        assert_eq!(read(ds.engine, 402, r#"{"error": {"message": "Insufficient Balance"}}"#).unwrap_err(), "DeepSeek 语音输入：账户余额不足（HTTP 402）：Insufficient Balance");
        assert!(read(ds.engine, 413, "{}").unwrap_err().contains("太大"));
        assert!(read(ds.engine, 200, "<html>").unwrap_err().contains("不是 JSON"));
    }
}
