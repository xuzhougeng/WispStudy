//! Model services as the settings keep them: a provider (one address, one
//! key, one proxy) offering any number of models. A task is bound to one of
//! those models; what a request is made with is the flat `ai::Profile` a
//! provider and one of its models make together (`Provider::profile`).

use crate::ai::{ANTHROPIC_BASE, DEFAULT_MODEL, Profile, Protocol};
use serde::{Deserialize, Serialize};

/// A service a provider can start from: its address, its interface, the
/// environment variable its key may be in and the models it is set up with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preset {
    pub key: &'static str,
    pub name: &'static str,
    pub protocol: Protocol,
    pub base_url: &'static str,
    /// Read when no key is stored. Each service has its own, so a key for one is never sent to another.
    pub key_env: &'static str,
    pub models: &'static [&'static str],
    /// The models it is set up with that draw (`imagegen`).
    pub images: &'static [&'static str],
    /// The models it is set up with that judge (`judge`).
    pub judges: &'static [&'static str],
    /// Where an API key is had.
    pub console: &'static str,
}

pub const PRESETS: [Preset; 9] = [
    Preset { key: "deepseek", name: "DeepSeek", protocol: Protocol::OpenAI, base_url: "https://api.deepseek.com", key_env: "DEEPSEEK_API_KEY", models: &["deepseek-flash", "deepseek-v4-pro"], images: &[], judges: &[], console: "https://platform.deepseek.com/api_keys" },
    Preset { key: "kimi", name: "Kimi", protocol: Protocol::OpenAI, base_url: "https://api.moonshot.cn/v1", key_env: "MOONSHOT_API_KEY", models: &["kimi-k2.6"], images: &[], judges: &[], console: "https://platform.moonshot.cn/console/api-keys" },
    Preset { key: "glm", name: "GLM", protocol: Protocol::OpenAI, base_url: "https://open.bigmodel.cn/api/paas/v4", key_env: "ZHIPUAI_API_KEY", models: &["glm-5.3", "glm-5.1"], images: &[], judges: &[], console: "https://open.bigmodel.cn/usercenter/apikeys" },
    Preset { key: "claude", name: "Claude", protocol: Protocol::Anthropic, base_url: ANTHROPIC_BASE, key_env: "ANTHROPIC_API_KEY", models: &[DEFAULT_MODEL, "claude-sonnet-5-5", "claude-haiku-5-5"], images: &[], judges: &[], console: "https://console.anthropic.com/settings/keys" },
    Preset { key: "openai", name: "OpenAI", protocol: Protocol::Responses, base_url: "https://api.openai.com/v1", key_env: "OPENAI_API_KEY", models: &["gpt-6.2-sol"], images: &["gpt-image-2"], judges: &[], console: "https://platform.openai.com/api-keys" },
    // Chat goes to Google's OpenAI-compatible address; pictures to Gemini's own interface beside it (`imagegen::guess`).
    Preset { key: "gemini", name: "Gemini", protocol: Protocol::OpenAI, base_url: "https://generativelanguage.googleapis.com/v1beta/openai", key_env: "GEMINI_API_KEY", models: &["gemini-4-hydrogen"], images: &["gemini-3.1-flash-image-preview", "gemini-3-pro-image-preview"], judges: &[], console: "https://aistudio.google.com/apikey" },
    // 火山方舟: 豆包 for chat, Seedream (the model behind 即梦) for pictures.
    Preset { key: "ark", name: "火山方舟", protocol: Protocol::OpenAI, base_url: "https://ark.cn-beijing.volces.com/api/v3", key_env: "ARK_API_KEY", models: &[], images: &["doubao-seedream-4-0-250828"], judges: &[], console: "https://console.volcengine.com/ark/region:ark+cn-beijing/apiKey" },
    // A System One service: its models judge and write nothing (`judge`). The interface chosen for chat is not used.
    Preset { key: "typesafe", name: "TypeSafe", protocol: Protocol::OpenAI, base_url: "https://api.typesafe.ai/v1", key_env: "TYPESAFE_API_KEY", models: &[], images: &[], judges: &["jev-latest"], console: "https://console.typesafe.ai/" },
    // OpenCode's subscription: one key for many vendors' models. Each model has a native interface (Kimi and GLM:
    // chat completions; MiniMax, Qwen: Claude's under `…/go/anthropic`; Grok, GPT: Responses) and the service
    // translates the others, so one address serves them all. Every request names its session (`ai::opencode_session`).
    Preset { key: "opencode-go", name: "OpenCode Go", protocol: Protocol::OpenAI, base_url: "https://opencode.ai/inference/go/openai/v1", key_env: "OPENCODE_API_KEY", models: &["kimi-k3", "glm-5.3"], images: &[], judges: &[], console: "https://opencode.ai/console" },
];

/// The providers a new settings file starts with.
pub const DEFAULT_PRESETS: [&str; 3] = ["deepseek", "kimi", "glm"];

impl Preset {
    /// The model name a box for adding one suggests: one that answers in words, a judge only where the
    /// service has nothing else by nature. 火山方舟 names its chat models per account: its documentation's example.
    pub fn example(&self) -> Option<&'static str> {
        if let Some(model) = self.models.first() { return Some(model) }
        if self.images.is_empty() && !self.judges.is_empty() { return Some(self.judges[0]) }
        (self.key == "ark").then_some("doubao-seed-1-6-251015")
    }
}

pub fn preset(key: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.key == key)
}

/// The preset an address belongs to, for settings written before presets existed.
pub fn preset_of_address(base_url: &str) -> Option<&'static Preset> {
    let host = base_url.split("://").nth(1).unwrap_or(base_url).split(['/', ':']).next().unwrap_or("").to_lowercase();
    let key = if host.ends_with("deepseek.com") { "deepseek" } else if host.contains("moonshot") { "kimi" } else if host.ends_with("bigmodel.cn") { "glm" } else if host.ends_with("anthropic.com") { "claude" } else if host == "api.openai.com" { "openai" } else if host == "generativelanguage.googleapis.com" { "gemini" } else if host.starts_with("ark.") && host.ends_with("volces.com") { "ark" } else if host == "api.typesafe.ai" { "typesafe" } else if is_opencode_go(base_url) { "opencode-go" } else { return None };
    preset(key)
}

/// Whether an address is OpenCode Go's: `opencode.ai/inference/go/…`, or `opencode.ai/zen/go/…` as it was first given.
/// OpenCode's pay-as-you-go service (`…/zen/v1`) is another, with its own billing.
pub fn is_opencode_go(base_url: &str) -> bool {
    let rest = base_url.split("://").nth(1).unwrap_or(base_url).to_lowercase();
    let (host, path) = rest.split_once('/').unwrap_or((&rest, ""));
    host.split(':').next() == Some("opencode.ai") && (path.starts_with("inference/go") || path.starts_with("zen/go"))
}

/// OpenCode Go's address for an interface: Claude's has its own, chat completions and Responses share one.
pub fn opencode_go_base(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Anthropic => "https://opencode.ai/inference/go/anthropic",
        Protocol::OpenAI | Protocol::Responses => "https://opencode.ai/inference/go/openai/v1",
    }
}

/// One model a provider offers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Model {
    /// What the task bindings point at; unique among all providers' models.
    pub id: String,
    /// The model's name as the service knows it (`deepseek-v4-pro`).
    pub model: String,
    /// How hard it thinks, a key of `ai::EFFORTS`.
    pub effort: String,
    /// Empty for a model that answers in words; `imagegen::KIND` for one that draws, `judge::KIND` for one that judges.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// For a model that draws: how it is asked, a key of `imagegen::INTERFACES`; empty leaves it to `imagegen::guess`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub interface: String,
    /// How many tokens it takes in, for knowing when a conversation has to be compacted; 0 leaves it to `compact::window`.
    #[serde(skip_serializing_if = "crate::ai::is_zero")]
    pub context: usize,
}

/// The windows a model can be said to have in the settings, in tokens.
pub const CONTEXTS: [usize; 7] = [16_000, 32_000, 64_000, 128_000, 200_000, 400_000, 1_000_000];

impl Model {
    /// Whether it draws pictures, not answers in words.
    pub fn draws(&self) -> bool {
        self.kind == crate::imagegen::KIND
    }

    /// Whether it judges: it decides among given answers and writes nothing.
    pub fn judges(&self) -> bool {
        self.kind == crate::judge::KIND
    }

    /// Whether it answers in words, as every task but drawing can use.
    pub fn speaks(&self) -> bool {
        !self.draws() && !self.judges()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    /// What the person calls it; empty is the preset's name, else the address's host.
    pub name: String,
    /// The `Preset::key` it started from; empty for a service set up by hand.
    pub preset: String,
    pub protocol: Protocol,
    pub base_url: String,
    /// Empty means the preset's environment variable, else ANTHROPIC_API_KEY / OPENAI_API_KEY.
    pub api_key: String,
    /// A proxy for this service alone: empty uses the global one, `direct` none at all.
    pub proxy: String,
    pub models: Vec<Model>,
}

impl Default for Provider {
    /// A service set up by hand: anything that speaks chat completions.
    fn default() -> Self {
        Provider { id: String::new(), name: String::new(), preset: String::new(), protocol: Protocol::OpenAI, base_url: String::new(), api_key: String::new(), proxy: String::new(), models: vec![] }
    }
}

impl Provider {
    pub fn from_preset(preset: &Preset) -> Provider {
        Provider { preset: preset.key.into(), protocol: preset.protocol, base_url: preset.base_url.into(), models: preset.models.iter().map(|model| Model { model: model.to_string(), ..Default::default() }).chain(preset.images.iter().map(|model| Model { model: model.to_string(), kind: crate::imagegen::KIND.into(), ..Default::default() })).chain(preset.judges.iter().map(|model| Model { model: model.to_string(), kind: crate::judge::KIND.into(), ..Default::default() })).collect(), ..Default::default() }
    }

    pub fn preset(&self) -> Option<&'static Preset> {
        preset(&self.preset)
    }

    pub fn title(&self) -> String {
        if !self.name.trim().is_empty() { return self.name.trim().to_string() }
        if let Some(preset) = self.preset() { return preset.name.into() }
        let host = self.base_url.split("://").nth(1).unwrap_or(&self.base_url).split(['/', ':']).next().unwrap_or("").trim();
        if host.is_empty() { "未命名服务".into() } else { host.to_string() }
    }

    /// The environment variable a key is read from when none is stored.
    pub fn key_env(&self) -> &'static str {
        match self.preset() {
            // A preset moved to another interface of the same service keeps its variable, and OpenCode Go's
            // key is the same at all three of its addresses,
            Some(preset) if preset.protocol == self.protocol || (preset.protocol != Protocol::Anthropic && self.protocol != Protocol::Anthropic) || preset.key == "opencode-go" => preset.key_env,
            _ => match self.protocol {
                Protocol::Anthropic => "ANTHROPIC_API_KEY",
                Protocol::OpenAI | Protocol::Responses => "OPENAI_API_KEY",
            },
        }
    }

    /// What a request with one of its models is made with.
    pub fn profile(&self, model: &Model) -> Profile {
        Profile { id: model.id.clone(), name: String::new(), provider: self.title(), protocol: self.protocol, base_url: self.base_url.clone(), api_key: self.api_key.clone(), key_env: self.key_env().into(), model: model.model.clone(), effort: model.effort.clone(), proxy: self.proxy.clone(), interface: model.interface.clone(), kind: model.kind.clone(), context: model.context }
    }

    /// The key it would send now, stored or from the environment.
    pub fn key(&self) -> String {
        self.profile(&Model::default()).key()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_keys() {
        let deepseek = Provider::from_preset(preset("deepseek").unwrap());
        assert_eq!((deepseek.title().as_str(), deepseek.models.len(), deepseek.key_env()), ("DeepSeek", 2, "DEEPSEEK_API_KEY"));
        // A key of the OpenAI kind is never what DeepSeek is asked with.
        let profile = deepseek.profile(&deepseek.models[0]);
        assert_eq!((profile.key_env.as_str(), profile.model.as_str(), profile.provider.as_str(), profile.title().as_str()), ("DEEPSEEK_API_KEY", "deepseek-flash", "DeepSeek", "deepseek-flash"));
        let custom = Provider { base_url: "http://localhost:11434/v1".into(), ..Default::default() };
        assert_eq!((custom.title().as_str(), custom.key_env()), ("localhost", "OPENAI_API_KEY"));
        assert_eq!(preset_of_address("https://api.moonshot.cn/v1").map(|p| p.key), Some("kimi"));
        assert_eq!(preset_of_address("https://api.anthropic.com").map(|p| p.key), Some("claude"));
        assert_eq!(preset_of_address("http://127.0.0.1:8787").map(|p| p.key), None);
        assert!(DEFAULT_PRESETS.iter().all(|key| preset(key).is_some()));
        // The services that draw come with their drawing models, marked as such, and one that answers in words.
        let openai = Provider::from_preset(preset("openai").unwrap());
        assert_eq!(openai.models.iter().map(|m| (m.model.as_str(), m.draws())).collect::<Vec<_>>(), [("gpt-6.2-sol", false), ("gpt-image-2", true)]);
        let gemini = Provider::from_preset(preset("gemini").unwrap());
        assert_eq!((gemini.models.iter().filter(|m| m.speaks()).count(), gemini.models.iter().filter(|m| m.draws()).count(), gemini.key_env()), (1, 2, "GEMINI_API_KEY"));
        assert_eq!(crate::imagegen::interface(&gemini.profile(&gemini.models[1])), crate::imagegen::Interface::Gemini);
        // The add box suggests a model that answers in words, a judge only where the service has nothing else.
        assert_eq!(PRESETS.map(|p| p.example()), [Some("deepseek-flash"), Some("kimi-k2.6"), Some("glm-5.3"), Some(DEFAULT_MODEL), Some("gpt-6.2-sol"), Some("gemini-4-hydrogen"), Some("doubao-seed-1-6-251015"), Some("jev-latest"), Some("kimi-k3")]);
        assert_eq!(preset_of_address("https://ark.cn-beijing.volces.com/api/v3").map(|p| p.key), Some("ark"));
        assert_eq!(preset_of_address("https://generativelanguage.googleapis.com/v1beta/openai").map(|p| p.key), Some("gemini"));
        // A service whose models judge: its own key, its own address, and no other kind of model.
        let typesafe = Provider::from_preset(preset("typesafe").unwrap());
        assert_eq!(typesafe.models.iter().map(|m| (m.model.as_str(), m.judges(), m.speaks())).collect::<Vec<_>>(), [("jev-latest", true, false)]);
        assert!(typesafe.profile(&typesafe.models[0]).judges() && typesafe.key_env() == "TYPESAFE_API_KEY");
        assert_eq!(preset_of_address("https://api.typesafe.ai/v1").map(|p| p.key), Some("typesafe"));
        // OpenCode Go, at either of its addresses; its pay-as-you-go sibling is not it.
        let go = Provider::from_preset(preset("opencode-go").unwrap());
        assert_eq!((go.title().as_str(), go.key_env(), go.models.iter().filter(|m| m.speaks()).count()), ("OpenCode Go", "OPENCODE_API_KEY", 2));
        let go_claude = Provider { protocol: Protocol::Anthropic, base_url: "https://opencode.ai/inference/go/anthropic".into(), ..go.clone() };
        assert_eq!(go_claude.key_env(), "OPENCODE_API_KEY");
        assert_eq!((opencode_go_base(Protocol::OpenAI), go.base_url.as_str()), (preset("opencode-go").unwrap().base_url, "https://opencode.ai/inference/go/openai/v1"));
        assert_eq!(preset_of_address("https://opencode.ai/inference/go/anthropic").map(|p| p.key), Some("opencode-go"));
        assert_eq!(preset_of_address("https://opencode.ai/zen/go/v1").map(|p| p.key), Some("opencode-go"));
        assert_eq!(preset_of_address("https://opencode.ai/zen/v1").map(|p| p.key), None);
        assert_eq!(preset_of_address("https://notopencode.ai/inference/go").map(|p| p.key), None);
        // A model that answers in words is written as before: no kind, no interface.
        assert_eq!(serde_json::to_string(&Model { id: "m1".into(), model: "x".into(), ..Default::default() }).unwrap(), r#"{"id":"m1","model":"x","effort":""}"#);
    }
}
