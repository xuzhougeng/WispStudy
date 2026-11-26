//! 设置: a view of its own, with a sidebar of pages (通用, 外观, 快捷键, AI, 写作 (`cite_styles.rs`),
//! 自定义 (`custom.rs`), 网络, 关于) under 返回 at the top left, where the library has its way back. AI has three tabs: 模型 (the providers,
//! a list at the left and the one picked at the right), 任务 (which model
//! does what, with its instructions, and the services that are no model: 网页搜索, 文字识别, 朗读) and 命令 (the person's own slash commands).
//! 网络 is the way out and who is asking: the proxy and what the literature databases are told. The library that was open stays open
//! underneath. Every change is saved at once; there is no 保存 button.
//!
//! The layout follows quiet-md's settings (MIT): grouped rows in rounded
//! cards, the title on the left and the control on the right.

use crate::i18n::t;
use crate::app::{Cmd, MenuKind, Modal, Workspace, default_root};
use crate::input::{InputEvent, TextInput};
use crate::shortcuts::{self, GROUPS, SHORTCUTS};
use crate::theme::{self, Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use wisp_core::ai::{self, Protocol};
use wisp_core::provider::{self, Provider};
use wisp_core::libraries;
use wisp_core::ocr;
use wisp_core::settings::{Language, Spacing, Task, TextWidth};
use wisp_core::storage;
use wisp_core::update::{self, Release};
use wisp_core::websearch;

const SIDEBAR_W: f32 = 224.;
const COLUMN_W: f32 = 640.;
/// The AI page is wider: 模型 sets its list beside the provider's fields.
const AI_COLUMN_W: f32 = 880.;
/// The list of providers on 模型.
const PROVIDER_LIST_W: f32 = 232.;
/// A model's name on 模型.
const MODEL_W: f32 = 240.;
const INPUT_W: f32 = 320.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsPage {
    #[default]
    General,
    Appearance,
    Shortcuts,
    Ai,
    Writing,
    Customize,
    Network,
    Storage,
    About,
}

impl SettingsPage {
    pub(crate) const ALL: [SettingsPage; 9] = [SettingsPage::General, SettingsPage::Appearance, SettingsPage::Shortcuts, SettingsPage::Ai, SettingsPage::Writing, SettingsPage::Customize, SettingsPage::Network, SettingsPage::Storage, SettingsPage::About];

    pub(crate) fn title(self) -> &'static str {
        match self {
            SettingsPage::General => t("通用"),
            SettingsPage::Appearance => t("外观"),
            SettingsPage::Shortcuts => t("快捷键"),
            SettingsPage::Ai => "AI",
            SettingsPage::Writing => t("写作"),
            SettingsPage::Customize => t("自定义"),
            SettingsPage::Network => t("网络"),
            SettingsPage::Storage => t("存储空间"),
            SettingsPage::About => t("关于"),
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            SettingsPage::General => "sliders",
            SettingsPage::Appearance => "palette",
            SettingsPage::Shortcuts => "keyboard",
            SettingsPage::Ai => "sparkle",
            SettingsPage::Writing => "pen-line",
            SettingsPage::Customize => "plug",
            SettingsPage::Network => "globe",
            SettingsPage::Storage => "hard-drive",
            SettingsPage::About => "info",
        }
    }
}

/// The tabs of 设置 › AI.
/// Which of the two interface fonts a picker on 外观 sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontSlot {
    /// Latin letters, digits and symbols: the font text is set in.
    Latin,
    /// Chinese characters: the fallback for what the Latin font cannot draw.
    Cjk,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AiTab {
    #[default]
    Models,
    Tasks,
    Commands,
}

/// The open settings view.
pub struct SettingsView {
    pub page: SettingsPage,
    pub ai_tab: AiTab,
    pub(crate) custom_tab: crate::custom::Tab,
    scroll: ScrollHandle,
    /// The provider 模型 shows at the right, as an index into `Settings::providers`.
    provider: usize,
    protocol: Protocol,
    fields: ProviderFields,
    /// The proxy for everything.
    proxy: Entity<TextInput>,
    /// The address the literature databases may write to, and the key of the person's NCBI account (文献数据库).
    contact_email: Entity<TextInput>,
    ncbi_key: Entity<TextInput>,
    /// What 测试 found of the way out, for the proxy as it now is, and how many times it was asked or the
    /// proxy changed: an answer that comes late is of another way out.
    reach: Option<Reach>,
    reach_asked: usize,
    /// How many times the proxy, the email or the NCBI key was typed into: what they hold is put to use once
    /// the typing has paused (`settle`).
    typed: usize,
    /// The key of each web search engine, in the order of `websearch::ENGINES`.
    search_keys: Vec<Entity<TextInput>>,
    /// The key of each engine of 文字识别 that has one of its own (`ocr_engines_with_keys`), in the order of `ocr::ENGINES`.
    ocr_keys: Vec<Entity<TextInput>>,
    /// DeepSeek's key is the model service's (模型): a field of its own is shown, for 网页搜索 and for 文字识别,
    /// only where an earlier build kept one, so that it can be seen and taken away. Decided on opening, so
    /// that a field being emptied does not leave under the caret.
    deepseek_fields: [bool; 2],
    /// Whose name the comments and tracked changes of a Word export carry.
    pub(crate) author: Entity<TextInput>,
    /// 写作's list of citation styles (cite_styles.rs).
    pub(crate) styles: crate::cite_styles::StyleBrowser,
    translation_prompt: Entity<TextInput>,
    font_search: Entity<TextInput>,
    font_names: Vec<String>,
    /// The font whose picker is open under its row.
    font_picker: Option<FontSlot>,
    font_scroll: ScrollHandle,
    /// The command of the person's own whose fields 命令 shows, as an index into `Settings::commands`.
    command: Option<(usize, CommandFields)>,
    /// The connector whose fields 自定义 shows, as an index into `Settings::connectors`.
    pub(crate) connector: Option<(usize, crate::custom::Fields)>,
    /// The skill whose page 自定义 shows.
    pub(crate) skill: Option<crate::custom::SkillView>,
    /// The shortcut waiting for its new key. While it waits, every key
    /// binding is cleared, so the keys pressed reach `record_key`.
    recording: Option<String>,
    /// A shortcut and why the key pressed for it was refused.
    shortcut_error: Option<(String, String)>,
    /// What the account of the provider on show holds, by the provider's id: asked of a service that says (DeepSeek's `/user/balance`).
    balance: Option<(String, Held)>,
    /// What per-model tests on 模型 asked and what came back, and how many tests were begun: an answer of a test
    /// replaced for the same model is not wanted.
    trials: Vec<Trial>,
    trial_runs: usize,
    /// What 存储空间 last measured, and of which data folder.
    storage: Option<(PathBuf, storage::Space)>,
    /// 存储空间 is measuring the data folder.
    measuring: bool,
    /// Generation counter for 存储空间 measurements so a newer run supersedes an older one.
    storage_run: usize,
    /// The library whose parts 存储空间 shows, by id (or `removed:<folder>` for a removed library).
    storage_open: Option<String>,
    /// The folder inside `removed/` currently being permanently deleted.
    purging: Option<String>,
    _subscriptions: Vec<Subscription>,
}

/// Where a person makes the key of their NCBI account.
const NCBI_KEY_PAGE: &str = "https://account.ncbi.nlm.nih.gov/settings/";

/// Where 网络's 测试 goes: a database every search asks, which answers anyone.
const REACH_URL: &str = "https://api.crossref.org/works?rows=0";
const REACH_HOST: &str = "api.crossref.org";

/// What 网络's 测试 found of the way out.
enum Reach {
    Asking,
    /// An answer came, after this long.
    Reached(Duration),
    Failed(String),
}

/// What testing one model on 设置 › AI › 模型 asked, and what came back. `target` is `provider::Model::id` for an
/// added model, or empty for the candidate typed in `ProviderFields::new_model`. It stands only while the provider,
/// the global proxy and the model itself (name, thinking and kind) are as they were when asked.
struct Trial {
    run: usize,
    target: String,
    /// The provider when asked, and the proxy for everything then.
    asked: Provider,
    proxy: String,
    model: provider::Model,
    tried: Tried,
}

/// What came of asking one model.
enum Tried {
    Asking,
    /// It answered, after this long, and said this (cut to a line).
    Answered(Duration, String),
    Failed(String),
}

impl Trial {
    /// Whether it is of `provider` as it is now, through the same proxy.
    fn stands(&self, provider: &Provider, proxy: &str) -> bool {
        let asked = &self.asked;
        (asked.id.as_str(), asked.protocol, asked.base_url.trim(), asked.api_key.trim(), asked.proxy.trim(), self.proxy.as_str()) == (provider.id.as_str(), provider.protocol, provider.base_url.trim(), provider.api_key.trim(), provider.proxy.trim(), proxy)
    }

    /// Whether it is of `model` at `target` under `provider` and `proxy` as they all are now.
    fn matches(&self, provider: &Provider, proxy: &str, target: &str, model: &provider::Model) -> bool {
        self.target == target && !model.draws() && self.stands(provider, proxy) && (self.model.model.trim(), &self.model.effort, &self.model.kind) == (model.model.trim(), &model.effort, &model.kind)
    }
}

/// What a `provider::Model::kind` is called in the interface.
pub(crate) fn model_kind_label(kind: &str) -> &'static str {
    if kind == wisp_core::imagegen::KIND { t("生图模型") } else if kind == wisp_core::judge::KIND { t("判断模型") } else { t("对话模型") }
}

/// How much of why a model could not be asked is said in a row: a service may answer a wrong name with every name it
/// knows. More of it is under the pointer on the model's mark.
const BRIEF: usize = 160;

/// Why a model could not be asked, in the interface's language, as a sentence: in English the service's hint
/// (`ai::error_message`) comes first and would begin in lower case.
fn failure(err: &str) -> String {
    let said = crate::i18n::reasons(err);
    let mut chars = said.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// What a model's connection test shows beside its name, and why it failed (for the line under its row).
fn trial_mark(id: SharedString, tried: &Tried) -> (AnyElement, Option<String>) {
    let (icon, text, color, tip, reason) = match tried {
        Tried::Asking => return (div().flex().flex_none().items_center().child(ui::spinner(id, 13., Theme::text_tertiary())).into_any_element(), None),
        Tried::Answered(took, said) => ("check", tf!("{:.1} 秒", "{:.1} s", took.as_secs_f32()), Theme::success(), if said.is_empty() { t("模型有回应，没有说话。").to_string() } else { tf!("模型回复：{said}", "The model said: {said}") }, None),
        Tried::Failed(err) => ("x", t("失败").to_string(), Theme::danger(), wisp_core::mention::cut(&failure(err), 600), Some(wisp_core::mention::cut(&failure(err), BRIEF))),
    };
    let mark = div().id(id).flex().flex_none().items_center().gap(px(3.)).text_size(text_px(12.)).text_color(color).child(ui::icon(icon, 13., color)).child(text).tooltip(ui::tip(tip)).into_any_element();
    (mark, reason)
}

/// What is known of an account's balance.
enum Held {
    Asking,
    Known(wisp_core::usage::Balance),
    /// Why the service did not say.
    Unknown(String),
}

impl SettingsView {
    pub(crate) fn reset_scroll(&mut self) {
        self.scroll = ScrollHandle::new();
    }

    /// The provider on show on 模型.
    pub(crate) fn provider(&self) -> usize {
        self.provider
    }

    pub(crate) fn new_model_kind(&self) -> Option<&str> {
        self.fields.new_model_kind.as_deref()
    }

    pub(crate) fn new_model_text(&self, cx: &App) -> String {
        self.fields.new_model.read(cx).text().trim().to_string()
    }

    fn trial_of(&self, provider: &Provider, proxy: &str, target: &str, model: &provider::Model) -> Option<&Tried> {
        self.trials.iter().find(|trial| trial.matches(provider, proxy, target, model)).map(|trial| &trial.tried)
    }
}

/// The text fields of one provider and of the candidate model to add. Added models keep their names as stored and are
/// not editable; fields are made anew for the provider on show, so a change always belongs to that one.
struct ProviderFields {
    name: Entity<TextInput>,
    base_url: Entity<TextInput>,
    api_key: Entity<TextInput>,
    /// The service's own proxy.
    proxy: Entity<TextInput>,
    /// The name of the model to test and add.
    new_model: Entity<TextInput>,
    /// Explicit kind chosen for `new_model`; `None` guesses from its name (`provider::guessed_kind`).
    new_model_kind: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ProviderFields {
    fn new(provider: &Provider, cx: &mut Context<Workspace>) -> ProviderFields {
        let mut input = |placeholder: SharedString, text: &str| cx.new(|cx| TextInput::new(placeholder, cx).with_text(text));
        let preset = provider.preset();
        let name = input(preset.map_or_else(|| t("服务名称").into(), |p| p.name.into()), &provider.name);
        let base_url = input("https://…/v1".into(), &provider.base_url);
        let api_key = input(tf!("留空则读取环境变量 {}", "Leave blank to read {}", provider.key_env()).into(), &provider.api_key);
        let proxy = input(t("留空则使用全局代理").into(), &provider.proxy);
        let example = preset.and_then(|p| p.example()).unwrap_or("gpt-6.2-sol");
        let new_model = input(tf!("添加模型，例如 {example}", "Add a model, e.g. {example}").into(), "");
        api_key.update(cx, |input, _| input.masked = true);
        let mut subscriptions: Vec<Subscription> = [&name, &base_url, &api_key, &proxy]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                    if *event == InputEvent::Changed { this.save_provider(cx) }
                })
            })
            .collect();
        subscriptions.push(cx.subscribe(&new_model, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Submit => this.submit_new_model(cx),
            // The button beside it updates as the name is typed.
            InputEvent::Changed => cx.notify(),
            _ => {}
        }));
        ProviderFields { name, base_url, api_key, proxy, new_model, new_model_kind: None, _subscriptions: subscriptions }
    }

    /// The candidate model as it would be tested or added now.
    fn candidate_model(&self, cx: &App) -> provider::Model {
        let name = self.new_model.read(cx).text().trim().to_string();
        let kind = self.new_model_kind.clone().unwrap_or_else(|| provider::guessed_kind(&name).to_string());
        provider::Model { model: name, kind, ..Default::default() }
    }
}

/// The text fields of one command of the person's own, made anew for the one on show.
struct CommandFields {
    name: Entity<TextInput>,
    about: Entity<TextInput>,
    template: Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
}

impl CommandFields {
    fn new(command: &ai::Command, cx: &mut Context<Workspace>) -> CommandFields {
        let name = cx.new(|cx| TextInput::new("style-check", cx).with_text(&command.name));
        let about = cx.new(|cx| TextInput::new("显示在命令列表里的一句说明", cx).with_text(&command.about));
        let template = cx.new(|cx| TextInput::new("要发给模型的话，例如：指出下面这段文字里空泛、重复的表达，逐条说明理由并给出改法，保留数值、术语和引用：{}", cx).multiline(6).with_text(&command.template));
        let subscriptions = [&name, &about, &template]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                    if *event == InputEvent::Changed { this.save_command(cx) }
                })
            })
            .collect();
        CommandFields { name, about, template, _subscriptions: subscriptions }
    }

    /// The name as it would be kept: without the slash, in lower case.
    fn typed_name(&self, cx: &App) -> String {
        self.name.read(cx).text().trim().trim_start_matches('/').to_lowercase()
    }
}

/// What is wrong with the name typed for the `index`-th command, if anything.
fn name_problem(name: &str, index: usize, commands: &[ai::Command]) -> Option<String> {
    ai::command_name_problem(name, commands.iter().enumerate().filter(|(i, _)| *i != index).map(|(_, command)| command.name.as_str()))
}

/// Where the check for a newer release stands.
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateState {
    Idle,
    Checking,
    Current,
    /// A newer release, and what it would replace (`updater::target`): `None` when this app cannot replace itself where it is.
    Available(Release, Option<PathBuf>),
    Downloading(Release),
    /// Fetched, checked and unpacked beside the app: the release, the new app, what it replaces.
    Ready(Release, PathBuf, PathBuf),
    Failed(String),
}

// ---- controls

/// Rows in one rounded card split by hairlines, under an optional caption.
pub(crate) fn group(title: Option<&'static str>, rows: Vec<AnyElement>) -> Div {
    let count = rows.len();
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(px(8.))
        .children(title.map(|t| div().px(px(4.)).font_weight(FontWeight::SEMIBOLD).text_color(Theme::text_secondary()).child(t)))
        .child(div().flex().flex_col().w_full().rounded(px(12.)).border_1().border_color(Theme::line()).overflow_hidden().children(rows.into_iter().enumerate().map(move |(i, row)| div().flex().flex_col().w_full().when(i + 1 < count, |d| d.border_b_1().border_color(Theme::line())).child(row))))
}

/// One setting: its name and an optional explanation on the left, the control on the right.
pub(crate) fn row(title: impl Into<SharedString>, detail: Option<SharedString>, control: impl IntoElement) -> Div {
    row_said(title, detail.map(|detail| (detail, Theme::text_tertiary())), control)
}

/// A row whose line under the name says how things stand, in the colour of that: what is wrong in
/// `Theme::danger()`, so that it is not read past as one more explanation.
pub(crate) fn row_said(title: impl Into<SharedString>, detail: Option<(SharedString, Hsla)>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap(px(24.))
        .w_full()
        .min_h(px(48.))
        .px(px(16.))
        .py(px(10.))
        .child(div().flex().flex_col().flex_1().min_w(px(120.)).gap(px(2.)).child(div().child(title.into())).children(detail.map(|(said, color)| div().text_size(text_px(12.)).line_height(text_px(17.)).text_color(color).child(said))))
        .child(div().flex().flex_none().max_w_full().items_center().gap(px(8.)).child(control))
}

/// A text field in a row, `width` wide; one whose text cannot be used wears the colour of that.
fn field_of(input: &Entity<TextInput>, width: f32, wrong: bool, window: &Window, cx: &App) -> Div {
    div().flex_none().w(px(width)).child(ui::field(input, window, cx).when(wrong, |d| d.border_color(Theme::danger())))
}

/// The small print under a group.
pub(crate) fn note(text: &'static str) -> Div {
    div().px(px(4.)).text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_tertiary()).child(text)
}

pub(crate) fn switch(id: impl Into<ElementId>, on: bool) -> Stateful<Div> {
    div().id(id).flex().flex_none().items_center().w(px(34.)).h(px(20.)).p(px(2.)).rounded_full().cursor_pointer().when(on, |d| d.bg(Theme::accent()).justify_end()).when(!on, |d| d.bg(Theme::groove()).justify_start()).child(div().size(px(16.)).rounded_full().bg(gpui::white()).shadow(theme::shadow_card()))
}

/// A choice among a few: a groove with the chosen one raised.
/// The engines of 文字识别 that are asked with a key of their own.
fn ocr_engines_with_keys() -> impl Iterator<Item = &'static ocr::Engine> {
    ocr::ENGINES.iter().filter(|engine| !engine.key_env.is_empty())
}

pub(crate) fn segmented(id: &'static str, choices: Vec<(&'static str, Cmd)>, chosen: usize, this: &Workspace, cx: &mut Context<Workspace>) -> Div {
    div().flex().flex_none().flex_wrap().max_w_full().gap(px(2.)).min_h(px(28.)).p(px(2.)).rounded(px(9.)).bg(Theme::groove()).children(choices.into_iter().enumerate().map(|(i, (label, cmd))| {
        div()
            .id((id, i))
            .flex()
            .items_center()
            .justify_center()
            .h(text_px(24.))
            .px(px(12.))
            .rounded(px(7.))
            .cursor_pointer()
            .when(i == chosen, |d| d.bg(Theme::segment()).shadow(theme::shadow_card()))
            .when(i != chosen, |d| d.text_color(Theme::text_secondary()).hover(|s| s.text_color(Theme::text())))
            .on_click(this.on(cmd, cx))
            .child(label)
    }))
}

pub(crate) fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    ui::secondary_button(id, label).h(text_px(28.))
}

/// A text button in the accent (恢复默认 …).
/// A model's window as it is named: 128K, 1M.
pub(crate) fn context_label(tokens: usize) -> String {
    if tokens >= 1_000_000 { format!("{}M", tokens / 1_000_000) } else { format!("{}K", tokens / 1_000) }
}

fn link_button(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
    div().id(id).flex().flex_none().items_center().h(text_px(26.)).px(px(8.)).rounded(px(7.)).cursor_pointer().text_size(text_px(12.)).text_color(Theme::accent()).hover(|s| s.bg(Theme::hover())).child(label)
}

/// A shortcut's key; `recording` shows it waiting for the new one.
fn keycap(id: impl Into<ElementId>, key: &str, recording: bool) -> Stateful<Div> {
    let text = if recording { t("按下新的快捷键…").to_string() } else if key.is_empty() { t("未设置").to_string() } else { shortcuts::pretty(key) };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .min_w(px(148.))
        .h(text_px(28.))
        .px(px(12.))
        .rounded(px(7.))
        .border_1()
        .cursor_pointer()
        .text_size(text_px(12.))
        .when(recording, |d| d.border_color(Theme::accent()).text_color(Theme::accent()))
        .when(!recording, |d| d.border_color(gpui::transparent_black()).bg(Theme::hover()).hover(|s| s.bg(Theme::pressed())).text_color(if key.is_empty() { Theme::text_tertiary() } else { Theme::text() }))
        .child(text)
}

/// The app icon resampled once to 176px, its size at 88pt on a 2x screen:
/// GPUI samples images bilinearly without mipmaps, so shrinking the large
/// file on the GPU leaves jagged edges.
fn about_icon() -> ImageSource {
    const PATH: &str = "branding/app-icon.png";
    static ICON: LazyLock<Option<Arc<RenderImage>>> = LazyLock::new(|| {
        let bytes = crate::assets::Assets.load(PATH).ok()??;
        let mut small = image::imageops::resize(&image::load_from_memory(&bytes).ok()?.into_rgba8(), 176, 176, image::imageops::FilterType::Lanczos3);
        // GPUI wants BGRA.
        for pixel in small.pixels_mut() {
            pixel.0.swap(0, 2);
        }
        Some(Arc::new(RenderImage::new(vec![image::Frame::new(small)])))
    });
    match &*ICON {
        Some(icon) => icon.clone().into(),
        None => PATH.into(),
    }
}

impl Workspace {
    // ---- opening and closing

    pub fn open_settings(&mut self, page: SettingsPage, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = None;
        self.modal_subscriptions.clear();
        // What was put into the skills' folder meanwhile is read on opening the page.
        if page == SettingsPage::Customize { self.load_skills() }
        match self.settings_view.as_mut() {
            Some(view) => {
                if view.page != page { view.scroll = ScrollHandle::new() }
                view.page = page;
                view.shortcut_error = None;
            }
            None => {
                // 模型 opens on the provider of the model questions go to.
                let chat = self.settings.profile_for(Task::Chat).map(|p| p.id).unwrap_or_default();
                let index = self.settings.providers.iter().position(|p| p.models.iter().any(|m| m.id == chat)).unwrap_or(0);
                let shown = self.settings.providers.get(index).cloned().unwrap_or_default();
                let fields = ProviderFields::new(&shown, cx);
                let proxy = cx.new(|cx| TextInput::new("http://127.0.0.1:7890", cx).with_text(&self.settings.proxy));
                let contact_email = cx.new(|cx| TextInput::new("name@university.edu", cx).with_text(&self.settings.contact_email));
                let ncbi_key = cx.new(|cx| TextInput::new(t("可以不填"), cx).with_text(&self.settings.ncbi_api_key));
                ncbi_key.update(cx, |input, _| input.masked = true);
                let author = cx.new(|cx| TextInput::new(wisp_core::settings::Settings::default().author_name(), cx).with_text(&self.settings.author));
                let translation_prompt = cx.new(|cx| TextInput::new("留空则使用默认要求：译成简体中文，保留术语、基因名、物种学名和数字原样，只输出译文。", cx).multiline(3).with_text(&self.settings.translation_prompt));
                let font_search = cx.new(|cx| TextInput::new(t("搜索已安装的字体"), cx));
                let font_names = cx.text_system().all_font_names().into_iter().filter(|name| !name.starts_with('.')).collect();
                let search_keys: Vec<Entity<TextInput>> = websearch::ENGINES
                    .iter()
                    .map(|engine| {
                        let stored = self.settings.web_search.keys.get(engine.key).cloned().unwrap_or_default();
                        let input = cx.new(|cx| TextInput::new("API Key", cx).with_text(&stored));
                        input.update(cx, |input, _| input.masked = true);
                        input
                    })
                    .collect();
                let ocr_keys: Vec<Entity<TextInput>> = ocr_engines_with_keys()
                    .map(|engine| {
                        let stored = self.settings.ocr.keys.get(engine.key).cloned().unwrap_or_default();
                        let input = cx.new(|cx| TextInput::new("API Key", cx).with_text(&stored));
                        input.update(cx, |input, _| input.masked = true);
                        input
                    })
                    .collect();
                let mut subscriptions = vec![
                    cx.subscribe_in(&font_search, window, |this, input, event: &InputEvent, window, cx| {
                        if *event == InputEvent::Changed {
                            if let Some(view) = this.settings_view.as_mut() { view.font_scroll = ScrollHandle::new() }
                            cx.notify();
                        } else if *event == InputEvent::Submit {
                            let query = input.read(cx).text().trim().to_lowercase();
                            let found = this.settings_view.as_ref().and_then(|view| view.font_names.iter().find(|name| name.to_lowercase().contains(&query))).cloned();
                            let slot = this.settings_view.as_ref().and_then(|view| view.font_picker);
                            if !query.is_empty() && let Some(family) = found && let Some(slot) = slot { this.set_font_family(slot, family, window, cx) }
                        }
                    }),
                    cx.subscribe(&proxy, |this, _, event: &InputEvent, cx| {
                        if *event == InputEvent::Changed { this.settle(cx) }
                    }),
                    cx.subscribe(&contact_email, |this, _, event: &InputEvent, cx| {
                        if *event == InputEvent::Changed { this.settle(cx) }
                    }),
                    cx.subscribe(&ncbi_key, |this, _, event: &InputEvent, cx| {
                        if *event == InputEvent::Changed { this.settle(cx) }
                    }),
                    cx.subscribe(&author, |this, input, event: &InputEvent, cx| {
                        if *event == InputEvent::Changed {
                            this.settings.author = input.read(cx).text().trim().to_string();
                            this.save_settings(cx);
                        }
                    }),
                    cx.subscribe(&translation_prompt, |this, input, event: &InputEvent, cx| {
                        if *event == InputEvent::Changed {
                            this.settings.translation_prompt = input.read(cx).text().trim().to_string();
                            this.save_settings(cx);
                            cx.notify();
                        }
                    }),
                ];
                for (engine, input) in websearch::ENGINES.iter().zip(&search_keys) {
                    subscriptions.push(cx.subscribe(input, |this, input, event: &InputEvent, cx| {
                        if *event != InputEvent::Changed { return }
                        let key = input.read(cx).text().trim().to_string();
                        let keys = &mut this.settings.web_search.keys;
                        if key.is_empty() { keys.remove(engine.key); } else { keys.insert(engine.key.to_string(), key); }
                        // The engine picked has lost its key: the choice goes back to whichever has one.
                        if this.settings.web_search.engine.trim() == engine.key && websearch::key_for(&this.settings, engine).0.is_empty() { this.settings.web_search.engine = websearch::AUTO.into() }
                        this.save_settings(cx);
                        cx.notify();
                    }));
                }
                for (engine, input) in ocr_engines_with_keys().zip(&ocr_keys) {
                    subscriptions.push(cx.subscribe(input, |this, input, event: &InputEvent, cx| {
                        if *event != InputEvent::Changed { return }
                        let key = input.read(cx).text().trim().to_string();
                        let keys = &mut this.settings.ocr.keys;
                        if key.is_empty() { keys.remove(engine.key); } else { keys.insert(engine.key.to_string(), key); }
                        if this.settings.ocr.engine.trim() == engine.key && ocr::ready(&this.settings, engine.key).is_err() { this.settings.ocr.engine = ocr::AUTO.into() }
                        this.save_settings(cx);
                        cx.notify();
                    }));
                }
                let (styles, heard) = self.style_browser_new(window, cx);
                subscriptions.push(heard);
                let kept = |keys: &std::collections::BTreeMap<String, String>| keys.get("deepseek").is_some_and(|key| !key.trim().is_empty());
                let deepseek_fields = [kept(&self.settings.web_search.keys), kept(&self.settings.ocr.keys)];
                self.settings_view = Some(SettingsView { page, ai_tab: AiTab::default(), custom_tab: crate::custom::Tab::default(), scroll: ScrollHandle::new(), provider: index, protocol: shown.protocol, fields, proxy, contact_email, ncbi_key, reach: None, reach_asked: 0, typed: 0, search_keys, ocr_keys, deepseek_fields, author, styles, translation_prompt, font_search, font_names, font_picker: None, font_scroll: ScrollHandle::new(), command: None, connector: None, skill: None, recording: None, shortcut_error: None, balance: None, trials: vec![], trial_runs: 0, storage: None, measuring: false, storage_run: 0, storage_open: None, purging: None, _subscriptions: subscriptions });
                self.read_balance_shown(cx);
            }
        }
        // 存储空间 is measured anew each time it is opened: files come and go while it is closed.
        if page == SettingsPage::Storage { self.measure_storage(cx) }
        window.focus(&self.focus);
        cx.notify();
    }

    /// Back to where 设置 was opened from.
    pub fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_recording(cx);
        // What was typed a moment ago and has not settled yet is kept before its fields go.
        self.save_proxy(cx);
        self.save_contact(cx);
        self.settings_view = None;
        window.focus(&self.focus);
        cx.notify();
    }

    // ---- 存储空间

    /// Measure the data folder on a background thread; what was measured before stays on show meanwhile.
    pub fn measure_storage(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_mut() else { return };
        view.storage_run += 1;
        let run = view.storage_run;
        view.measuring = true;
        let root = self.root.clone();
        let measured = cx.background_executor().spawn({
            let root = root.clone();
            async move { storage::measure(&root) }
        });
        cx.spawn(async move |this, cx| {
            let space = measured.await;
            this.update(cx, |this, cx| {
                let current = this.root.clone();
                let Some(view) = this.settings_view.as_mut() else { return };
                if view.storage_run != run { return }
                view.measuring = false;
                // The data folder was changed meanwhile: this measure is of another folder.
                if current == root { view.storage = Some((root, space)) }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Show or fold the parts of one library on 存储空间.
    pub fn toggle_storage_library(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_mut() else { return };
        view.storage_open = if view.storage_open.as_deref() == Some(id.as_str()) { None } else { Some(id) };
        cx.notify();
    }

    /// Put a removed library from `removed/` back onto the home page (or 最近对话 for a draft).
    pub fn restore_removed_library(&mut self, folder: String, cx: &mut Context<Self>) {
        if self.settings_view.as_ref().is_some_and(|view| view.purging.as_deref() == Some(folder.as_str())) { return }
        match libraries::restore(&self.root, &folder) {
            Ok(info) => {
                self.libraries = libraries::list(&self.root);
                if let Some(view) = self.settings_view.as_mut() {
                    let key = format!("removed:{folder}");
                    if view.storage_open.as_deref() == Some(key.as_str()) { view.storage_open = None }
                }
                self.measure_storage(cx);
                if info.draft {
                    self.say(tf!("已恢复对话「{}」，它回到了「最近对话」。", "Restored conversation “{}”; it is back in Recent conversations.", info.name), cx);
                } else {
                    self.say(tf!("已恢复文献库「{}」，它回到了首页。", "Restored library “{}”; it is back on the home page.", info.name), cx);
                }
            }
            Err(err) => self.fail(format!("{err:#}"), cx),
        }
    }

    /// Ask before permanently deleting a removed library's folder from disk.
    pub fn ask_purge_removed_library(&mut self, folder: String, cx: &mut Context<Self>) {
        let Some((name, draft, paper_count, bytes)) = self.settings_view.as_ref()
            .filter(|view| view.purging.is_none())
            .and_then(|view| view.storage.as_ref())
            .filter(|(root, _)| *root == self.root)
            .and_then(|(_, space)| space.removed_libraries.iter().find(|lib| lib.folder == folder))
            .map(|lib| (lib.name.clone(), lib.draft, lib.paper_count, storage::human(lib.total()))) else { return };
        self.modal_error.clear();
        let papers = tn!("{n} 篇文献", "{n}", paper_count, "paper");
        let (title, message) = if draft {
            let message = if paper_count > 0 {
                tf!("「{}」（{}，占 {}）的文件夹连同对话、文献和稿件会从磁盘上完全删除，无法恢复。", "The folder of “{}” ({}, {}) — with its conversation, papers and manuscripts — will be deleted from disk completely and cannot be recovered.", name, papers, bytes)
            } else {
                tf!("「{}」（占 {}）的文件夹连同对话和稿件会从磁盘上完全删除，无法恢复。", "The folder of “{}” ({}) — with its conversation and manuscripts — will be deleted from disk completely and cannot be recovered.", name, bytes)
            };
            (tf!("彻底删除对话「{}」？", "Delete conversation “{}” permanently?", name), message)
        } else {
            (
                tf!("彻底删除文献库「{}」？", "Delete library “{}” permanently?", name),
                tf!("「{}」（{}，占 {}）的文件夹连同数据库、PDF、笔记、图片库和稿件会从磁盘上完全删除，无法恢复。", "The folder of “{}” ({}, {}) — with its database, PDFs, notes, figures and manuscripts — will be deleted from disk completely and cannot be recovered.", name, papers, bytes),
            )
        };
        self.modal = Some(Modal::Confirm { title, message, action: t("彻底删除").into(), cmd: Cmd::PurgeRemovedLibrary(folder) });
        cx.notify();
    }

    /// Permanently delete a removed library's folder on a background thread.
    pub fn purge_removed_library(&mut self, folder: String, cx: &mut Context<Self>) {
        self.modal = None;
        if self.settings_view.as_ref().is_some_and(|view| view.purging.is_some()) { return }
        if let Some(view) = self.settings_view.as_mut() { view.purging = Some(folder.clone()) }
        cx.notify();
        let root = self.root.clone();
        let deleted = cx.background_executor().spawn({
            let root = root.clone();
            let folder = folder.clone();
            async move { libraries::purge_removed(&root, &folder) }
        });
        cx.spawn(async move |this, cx| {
            let deleted = deleted.await;
            this.update(cx, |this, cx| {
                if let Some(view) = this.settings_view.as_mut() {
                    if view.purging.as_deref() == Some(folder.as_str()) { view.purging = None }
                    let key = format!("removed:{folder}");
                    if view.storage_open.as_deref() == Some(key.as_str()) { view.storage_open = None }
                }
                match deleted {
                    Ok(info) => {
                        if info.draft {
                            this.say(tf!("已彻底删除对话「{}」。", "Deleted conversation “{}” permanently.", info.name), cx);
                        } else {
                            this.say(tf!("已彻底删除文献库「{}」。", "Deleted library “{}” permanently.", info.name), cx);
                        }
                        if this.root == root { this.measure_storage(cx) }
                    }
                    Err(err) => this.fail(format!("{err:#}"), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- 通用

    pub fn pick_data_dir(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some(t("选择").into()), extensions: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await && let Some(dir) = paths.into_iter().next() {
                this.update(cx, |this, cx| this.set_data_dir(Some(dir), cx)).ok();
            }
        })
        .detach();
    }

    /// Keep the libraries in `dir`; `None` goes back to the default folder.
    /// Nothing is moved: the home page shows what the new folder holds.
    pub fn set_data_dir(&mut self, dir: Option<PathBuf>, cx: &mut Context<Self>) {
        let default = default_root();
        let root = dir.unwrap_or_else(|| default.clone());
        if root == self.root { return }
        if let Err(err) = std::fs::create_dir_all(&root) {
            return self.fail(tf!("无法使用这个目录：{err}", "Cannot use this folder: {err}"), cx);
        }
        // The default folder is stored as "unset", so it follows the user's Documents folder.
        self.settings.data_dir = if root == default { String::new() } else { root.to_string_lossy().to_string() };
        self.root = root;
        // The open library belongs to the old folder, and those answering off screen.
        self.lib = None;
        self.parked.clear();
        self.bell.forget(None);
        theme::set_accent("blue");
        self.libraries = libraries::list(&self.root);
        self.save_settings(cx);
        self.say(tf!("数据目录已改为 {}", "Data folder changed to {}", self.root.display()), cx);
    }

    // ---- 更新

    /// Start the automatic check: shortly after launch, then twice a day, while 自动检查更新 is on.
    pub fn watch_updates(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(10)).await;
            // By now the app an update replaced has quit, and what it left can go.
            cx.background_executor().spawn(async { crate::updater::tidy() }).await;
            loop {
                let alive = this.update(cx, |this, cx| {
                    if this.settings.auto_update { this.check_update(false, cx) }
                });
                if alive.is_err() { break }
                cx.background_executor().timer(Duration::from_secs(12 * 3600)).await;
            }
        })
        .detach();
    }

    /// Ask which release is the newest. `manual` (检查更新) also reports a
    /// failure; the automatic check stays silent except for one toast per
    /// new version, or the question once that version is downloaded.
    pub fn check_update(&mut self, manual: bool, cx: &mut Context<Self>) {
        if matches!(self.update, UpdateState::Checking | UpdateState::Downloading(_)) { return }
        if manual {
            self.update = UpdateState::Checking;
            cx.notify();
        }
        // WISP_STUDY_UPDATE_URL points the check elsewhere (testing).
        let url = wisp_core::settings::env("UPDATE_URL").filter(|u| !u.trim().is_empty()).unwrap_or_else(update::latest_url);
        cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async move { wisp_net::latest_release(&url) }).await;
            this.update(cx, |this, cx| {
                match found {
                    // The version already downloaded waits for its answer.
                    Ok(Some(release)) if matches!(&this.update, UpdateState::Ready(ready, ..) if ready.version == release.version) => {}
                    Ok(Some(release)) if update::newer(&release.version, env!("CARGO_PKG_VERSION")) => {
                        let target = crate::updater::target().filter(|_| crate::updater::asset(&release.version).is_some());
                        if this.settings.auto_download && let Some(target) = target {
                            return this.download_update(release, target, manual, cx);
                        }
                        if !manual && this.update_told.as_ref() != Some(&release.version) {
                            this.say(tf!("Wisp Study {} 已发布，可在「设置 › 关于」查看。", "Wisp Study {} is available. See Settings › About.", release.version), cx);
                        }
                        this.update_told = Some(release.version.clone());
                        this.update = UpdateState::Available(release, target);
                    }
                    Ok(_) => this.update = UpdateState::Current,
                    Err(err) if manual => this.update = UpdateState::Failed(err),
                    // A quiet retry at the next check; keep whatever was shown.
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fetch the release's package in the background, check it and unpack it beside the app
    /// (`updater::stage`); then ask whether to restart into it. Nothing of the running app changes before that answer.
    pub fn download_update(&mut self, release: Release, target: PathBuf, manual: bool, cx: &mut Context<Self>) {
        let Some(name) = crate::updater::asset(&release.version) else { return };
        // WISP_STUDY_UPDATE_FILES: the folder the package and its SHA256SUMS come from, elsewhere (testing).
        let base = wisp_core::settings::env("UPDATE_FILES").filter(|u| !u.trim().is_empty()).unwrap_or_else(|| update::files_url(&release));
        self.update = UpdateState::Downloading(release.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let place = target.clone();
            let staged = cx.background_executor().spawn(async move { wisp_net::download_release(&base, &name).and_then(|package| crate::updater::stage(&package, &place)) }).await;
            this.update(cx, |this, cx| {
                match staged {
                    Ok(staged) => {
                        this.update = UpdateState::Ready(release, staged, target);
                        this.offer_update(cx);
                    }
                    // The row offers the download again; the automatic one is tried at the next check.
                    Err(err) => {
                        if manual { this.fail(tf!("新版本没有下载好：{err}", "The new version was not downloaded: {err}"), cx) }
                        this.update = UpdateState::Available(release, Some(target));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The downloaded version is ready: ask once whether to restart into it. A dialog already open is not pushed aside.
    fn offer_update(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Ready(release, ..) = &self.update else { return };
        let title = tf!("Wisp Study {} 已下载好", "Wisp Study {} is ready", release.version);
        if self.modal.is_some() { return self.say(tf!("{title}，可在「设置 › 关于」重启并更新。", "{title}. Restart to update in Settings › About."), cx) }
        let back = if self.settings.restore_session { t("重新打开后回到现在的界面。") } else { "" };
        self.modal_error.clear();
        self.modal = Some(Modal::Confirm { title, message: tf!("现在重启就换成新版本，文献库和设置不受影响；正在进行的对话、筛选或导入会中断。{back}也可以先取消，稍后在「设置 › 关于」重启并更新。", "Restart now to switch to the new version. Libraries and settings are kept; a conversation, screening or import under way is interrupted. {back}Or cancel and restart to update later in Settings › About."), action: t("重启并更新").into(), cmd: Cmd::InstallUpdate });
    }

    /// 重启并更新: the new app takes this one's place and is started as this one quits. Where the
    /// window is has been kept all along (`keep_session`), so the new one opens there.
    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Ready(release, staged, target) = self.update.clone() else { return };
        self.modal = None;
        self.write_snapshot("leave");
        match crate::updater::install(&staged, &target) {
            Ok(()) => cx.quit(),
            Err(err) => {
                self.update = UpdateState::Available(release, Some(target));
                self.fail(tf!("新版本没有装上：{err}", "The new version was not installed: {err}"), cx);
            }
        }
    }

    /// Where the check stands and the one next step.
    fn update_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = tf!("当前版本 {}", "Current version {}", env!("CARGO_PKG_VERSION"));
        let check = |this: &Self, cx: &mut Context<Self>| Some(button("update-check", t("检查更新")).on_click(this.on(Cmd::CheckUpdate, cx)));
        let (title, detail, control): (String, Option<String>, Option<Stateful<Div>>) = match &self.update {
            UpdateState::Idle => (t("软件更新").into(), Some(current), check(self, cx)),
            UpdateState::Checking => (t("正在检查更新…").into(), Some(current), None),
            UpdateState::Current => (t("已是最新版本").into(), Some(current), check(self, cx)),
            UpdateState::Available(release, None) => (tf!("新版本 {} 可用", "Version {} available", release.version), Some(tf!("{current}。在发布页下载新版本后覆盖安装，文献库和设置不受影响。", "{current}. Download and install the new version from the releases page. Libraries and settings are kept.")), Some(button("update-open", t("前往下载")).on_click(self.on(Cmd::OpenUrl(release.url.clone()), cx)))),
            UpdateState::Available(release, Some(_)) => (tf!("新版本 {} 可用", "Version {} available", release.version), Some(tf!("{current}。下载好后重启即可更新，文献库和设置不受影响。", "{current}. Once downloaded, a restart updates the app. Libraries and settings are kept.")), Some(button("update-download", t("下载更新")).on_click(self.on(Cmd::DownloadUpdate, cx)))),
            UpdateState::Downloading(release) => (tf!("正在下载新版本 {}…", "Downloading version {}…", release.version), Some(current), None),
            UpdateState::Ready(release, ..) => (tf!("新版本 {} 已下载好", "Version {} is ready", release.version), Some(tf!("{current}。重启后换成新版本，文献库和设置不受影响。", "{current}. A restart switches to the new version. Libraries and settings are kept.")), Some(button("update-install", t("重启并更新")).on_click(self.on(Cmd::InstallUpdate, cx)))),
            UpdateState::Failed(err) => (t("检查更新失败").into(), Some(err.clone()), check(self, cx)),
        };
        row(title, detail.map(Into::into), div().children(control)).into_any_element()
    }

    // ---- 外观

    pub fn apply_typography(&self, window: &mut Window, cx: &App) {
        let names = cx.text_system().all_font_names();
        // A font that is not installed (any more) falls back to the platform's choice.
        let installed = |family: &String| if family.is_empty() || names.contains(family) { family.clone() } else { String::new() };
        theme::set_typography(&installed(&self.settings.font_family), &installed(&self.settings.cjk_font_family), self.settings.font_size);
        theme::set_reading(self.settings.reading_appearance);
        window.refresh();
    }

    pub fn set_font_family(&mut self, slot: FontSlot, family: String, window: &mut Window, cx: &mut Context<Self>) {
        match slot {
            FontSlot::Latin => self.settings.font_family = family,
            FontSlot::Cjk => self.settings.cjk_font_family = family,
        }
        if let Some(view) = self.settings_view.as_mut() { view.font_picker = None }
        self.save_settings(cx);
        self.apply_typography(window, cx);
        window.focus(&self.focus);
    }

    /// Open the picker of `slot` (closing the other one), or close it when it is the one open.
    pub(crate) fn toggle_font_picker(&mut self, slot: FontSlot, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_mut() else { return };
        view.font_picker = if view.font_picker == Some(slot) { None } else { Some(slot) };
        if view.font_picker.is_some() {
            view.font_scroll = ScrollHandle::new();
            view.font_search.update(cx, |input, cx| input.set_text("", cx));
            view.font_search.read(cx).focus_handle(cx).focus(window);
        } else {
            window.focus(&self.focus);
        }
        cx.notify();
    }

    #[cfg(debug_assertions)]
    pub(crate) fn search_settings_fonts(&mut self, query: &str, cx: &mut Context<Self>) {
        if let Some(view) = &self.settings_view { view.font_search.update(cx, |input, cx| input.set_text(query, cx)) }
    }

    /// Esc in 设置: one level up on the page on show, if it is a level down (design.md rule 54): an opened skill or
    /// MCP back to its list, a command's fields or a library's parts folded up again. False on a page's top level.
    pub fn settings_up(&mut self, cx: &mut Context<Self>) -> bool {
        use crate::custom::Tab;
        let Some(view) = self.settings_view.as_mut() else { return false };
        let up = match view.page {
            SettingsPage::Customize if view.custom_tab == Tab::Skills => view.skill.take().is_some(),
            SettingsPage::Customize => view.connector.take().is_some(),
            SettingsPage::Ai if view.ai_tab == AiTab::Commands => view.command.take().is_some(),
            SettingsPage::Storage => view.storage_open.take().is_some(),
            _ => false,
        };
        if up {
            view.reset_scroll();
            cx.notify();
        }
        up
    }

    pub fn dismiss_font_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(view) = self.settings_view.as_mut().filter(|view| view.font_picker.is_some() && view.page == SettingsPage::Appearance) else { return false };
        view.font_picker = None;
        window.focus(&self.focus);
        cx.notify();
        true
    }

    /// Whether `family` has a glyph of its own for Chinese, so that a Chinese fallback is never reached.
    /// A character the font lacks is shaped in another font, which shows as another font id on the run.
    fn draws_chinese(family: &str, window: &Window) -> bool {
        let font = gpui::font(family.to_string());
        let system = window.text_system();
        let own = system.resolve_font(&font);
        let run = TextRun { len: "文".len(), font, color: Theme::text(), background_color: None, underline: None, strikethrough: None };
        let line = system.shape_line("文".into(), px(16.), &[run], None);
        line.runs.iter().all(|run| run.font_id == own) && !line.runs.is_empty()
    }

    /// Use the light or the dark colours: the chosen one, else the system's.
    pub fn apply_appearance(&mut self, window: &mut Window) {
        let dark = match self.settings.appearance.as_str() {
            "light" => false,
            "dark" => true,
            _ => matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark),
        };
        theme::set_dark(dark);
        theme::set_high_contrast(self.settings.high_contrast);
        theme::set_work_animation(self.settings.work_animation);
        window.refresh();
    }

    // ---- 快捷键

    pub fn start_recording(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_mut() else { return };
        view.recording = Some(id);
        view.shortcut_error = None;
        // The keys go to the workspace, and none of them runs anything meanwhile.
        window.focus(&self.focus);
        cx.clear_key_bindings();
        cx.notify();
    }

    /// Stop waiting for a key and put the key bindings back.
    pub fn stop_recording(&mut self, cx: &mut Context<Self>) {
        if self.settings_view.as_mut().is_some_and(|view| view.recording.take().is_some()) {
            shortcuts::rebuild(&self.settings.shortcuts, cx);
            cx.notify();
        }
    }

    /// A key pressed while a shortcut waits for its new key.
    pub fn record_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.settings_view.as_ref().and_then(|view| view.recording.clone()) else { return };
        cx.stop_propagation();
        let stroke = &event.keystroke;
        if event.is_held || matches!(stroke.key.as_str(), "" | "shift" | "alt" | "ctrl" | "control" | "cmd" | "win" | "super" | "platform" | "fn" | "function") { return }
        if stroke.key == "escape" && !stroke.modifiers.modified() { return self.stop_recording(cx) }
        let key = stroke.unparse();
        match shortcuts::refusal(&id, &key, &self.settings.shortcuts) {
            Some(why) => {
                if let Some(view) = self.settings_view.as_mut() { view.shortcut_error = Some((id, why)) }
            }
            None => self.set_shortcut(&id, Some(key), cx),
        }
        cx.notify();
    }

    /// Give the shortcut a key (an empty one removes it); `None` restores its default.
    pub fn set_shortcut(&mut self, id: &str, key: Option<String>, cx: &mut Context<Self>) {
        match key {
            Some(key) if !shortcuts::is_default(id, &key) => {
                self.settings.shortcuts.insert(id.to_string(), key);
            }
            _ => {
                self.settings.shortcuts.remove(id);
            }
        }
        // A restored default may now collide with a key given to another shortcut meanwhile.
        let key = shortcuts::key_of(id, &self.settings.shortcuts);
        let clash = if key.is_empty() { None } else { shortcuts::refusal(id, &key, &self.settings.shortcuts) };
        if let Some(view) = self.settings_view.as_mut() {
            view.recording = None;
            view.shortcut_error = clash.map(|why| (id.to_string(), why));
        }
        self.save_settings(cx);
        shortcuts::rebuild(&self.settings.shortcuts, cx);
        cx.notify();
    }

    pub fn reset_shortcuts(&mut self, cx: &mut Context<Self>) {
        self.settings.shortcuts.clear();
        if let Some(view) = self.settings_view.as_mut() {
            view.recording = None;
            view.shortcut_error = None;
        }
        self.save_settings(cx);
        shortcuts::rebuild(&self.settings.shortcuts, cx);
        cx.notify();
    }

    // ---- AI

    /// 自定义 uses the same sub-panel navigation as AI; switching also refreshes skills.
    pub fn open_custom_tab(&mut self, tab: crate::custom::Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(SettingsPage::Customize, window, cx);
        let Some(view) = self.settings_view.as_mut() else { return };
        if view.custom_tab != tab { view.scroll = ScrollHandle::new() }
        view.custom_tab = tab;
        cx.notify();
    }

    /// 设置 › AI on one of its tabs, opening 设置 if it is not open.
    pub fn open_ai_tab(&mut self, tab: AiTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(SettingsPage::Ai, window, cx);
        let Some(view) = self.settings_view.as_mut() else { return };
        if view.ai_tab != tab { view.scroll = ScrollHandle::new() }
        view.ai_tab = tab;
        cx.notify();
    }

    pub fn pick_protocol(&mut self, protocol: Protocol, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_mut() else { return };
        if view.protocol == protocol { return }
        let same_service = view.protocol != Protocol::Anthropic && protocol != Protocol::Anthropic;
        view.protocol = protocol;
        // OpenCode Go speaks all three, each at its own address: go there, models and key untouched.
        if provider::is_opencode_go(view.fields.base_url.read(cx).text()) {
            view.fields.base_url.update(cx, |input, cx| input.set_text(provider::opencode_go_base(protocol), cx));
            return self.save_provider(cx);
        }
        // Chat completions and Responses are two interfaces of one service: its address and models stay.
        if same_service { return self.save_provider(cx) }
        // The other protocol's address would only fail: offer this one's default.
        let url = match protocol {
            Protocol::Anthropic => ai::ANTHROPIC_BASE,
            Protocol::OpenAI | Protocol::Responses => "https://api.openai.com/v1",
        };
        view.fields.base_url.update(cx, |input, cx| input.set_text(url, cx));
        self.save_provider(cx);
    }

    /// How hard the n-th model of the provider on show thinks.
    pub fn set_effort(&mut self, index: usize, effort: String, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_ref() else { return };
        let Some(model) = self.settings.providers.get_mut(view.provider).and_then(|p| p.models.get_mut(index)) else { return };
        if model.effort == effort { return }
        model.effort = effort;
        self.save_settings(cx);
        cx.notify();
    }

    /// How many tokens the n-th model of the provider on show takes in: when a conversation with it is compacted.
    pub fn set_model_context(&mut self, index: usize, tokens: usize, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_ref() else { return };
        let Some(model) = self.settings.providers.get_mut(view.provider).and_then(|p| p.models.get_mut(index)) else { return };
        if model.context == tokens { return }
        model.context = tokens;
        self.save_settings(cx);
        cx.notify();
    }

    /// Show a provider's fields at the right of 模型.
    pub fn select_provider(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.settings.providers.len() { return }
        if let Some(view) = self.settings_view.as_mut() { view.ai_tab = AiTab::Models }
        self.show_provider(index, cx);
    }

    fn show_provider(&mut self, index: usize, cx: &mut Context<Self>) {
        let shown = self.settings.providers.get(index).cloned().unwrap_or_default();
        let fields = ProviderFields::new(&shown, cx);
        if let Some(view) = self.settings_view.as_mut() { (view.provider, view.protocol, view.fields) = (index, shown.protocol, fields) }
        self.read_balance_shown(cx);
        cx.notify();
    }

    /// Ask what the account of the provider on show holds, where the service has an address for that and a key is set
    /// (DeepSeek): on showing the provider, and when its balance is clicked. Another provider shows none.
    pub fn read_balance_shown(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_mut() else { return };
        view.balance = None;
        let Some(provider) = self.settings.providers.get(view.provider) else { return };
        let profile = provider.profile(&provider::Model::default());
        if !wisp_core::usage::is_deepseek(&profile) { return }
        let Ok(request) = wisp_core::usage::balance_request(&profile) else { return };
        let id = provider.id.clone();
        view.balance = Some((id.clone(), Held::Asking));
        let reply = cx.background_executor().spawn(async move { wisp_net::balance(&request).and_then(|body| wisp_core::usage::read_balance(&body)) });
        cx.spawn(async move |this, cx| {
            let reply = reply.await;
            this.update(cx, |this, cx| {
                // Another provider is on show by now, or the settings are closed: the answer is not wanted.
                let Some((_, held)) = this.settings_view.as_mut().and_then(|view| view.balance.as_mut()).filter(|(shown, _)| *shown == id) else { return };
                *held = match reply {
                    Ok(balance) => Held::Known(balance),
                    Err(err) => Held::Unknown(err),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// One more provider, from a preset (`""` is a service set up by hand), and its fields on show.
    pub fn add_provider(&mut self, preset: &str, cx: &mut Context<Self>) {
        let provider = provider::preset(preset).map_or_else(|| Provider { name: tf!("服务 {}", "Provider {}", self.settings.providers.len() + 1), base_url: "https://".into(), ..Default::default() }, Provider::from_preset);
        let index = self.settings.add_provider(provider);
        self.save_settings(cx);
        self.show_provider(index, cx);
    }

    pub fn remove_provider(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.settings.providers.len() { return }
        self.settings.remove_provider(index);
        self.save_settings(cx);
        let shown = self.settings_view.as_ref().map_or(0, |view| view.provider);
        self.show_provider(if shown > index { shown - 1 } else { shown.min(self.settings.providers.len().saturating_sub(1)) }, cx);
    }

    /// Enter in `new_model`: add the candidate once it draws or has passed its test, otherwise test it first.
    pub fn submit_new_model(&mut self, cx: &mut Context<Self>) {
        let proxy = self.settings.global_proxy();
        let Some(view) = self.settings_view.as_ref() else { return };
        let Some(provider) = self.settings.providers.get(view.provider) else { return };
        let candidate = view.fields.candidate_model(cx);
        if candidate.model.is_empty() { return }
        if candidate.draws() || matches!(view.trial_of(provider, &proxy, "", &candidate), Some(Tried::Answered(..))) {
            self.add_model(cx);
        } else {
            self.test_new_model(cx);
        }
    }

    pub fn set_new_model_kind(&mut self, kind: Option<String>, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_mut() else { return };
        view.fields.new_model_kind = kind;
        cx.notify();
    }

    #[cfg(debug_assertions)]
    pub(crate) fn set_new_model_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(view) = &self.settings_view { view.fields.new_model.update(cx, |input, cx| input.set_text(text, cx)) }
    }

    /// Add what the field under the models holds as one more model of the provider on show. A model that does not
    /// draw must have passed its connection test under the current provider settings first.
    pub fn add_model(&mut self, cx: &mut Context<Self>) {
        let proxy = self.settings.global_proxy();
        let Some(view) = self.settings_view.as_ref() else { return };
        let (index, candidate, explicit_kind) = (view.provider, view.fields.candidate_model(cx), view.fields.new_model_kind.clone());
        if candidate.model.is_empty() { return }
        let Some(provider) = self.settings.providers.get(index) else { return };
        // The same model twice would only be two rows to tell apart.
        if provider.models.iter().any(|m| m.model == candidate.model) {
            return self.fail(tf!("{} 已经在这个服务的模型里了。", "{} is already one of this provider's models.", candidate.model), cx);
        }
        if !candidate.draws() && !matches!(view.trial_of(provider, &proxy, "", &candidate), Some(Tried::Answered(..))) { return }
        let provider_snapshot = provider.clone();
        let Some(id) = self.settings.add_model_with_kind(index, &candidate.model, explicit_kind.as_deref()) else { return };
        self.save_settings(cx);
        let Some(view) = self.settings_view.as_mut() else { return };
        // Carry the passed test result onto the newly added model row so its check mark stays in sight.
        if let Some(trial) = view.trials.iter_mut().find(|trial| trial.matches(&provider_snapshot, &proxy, "", &candidate)) {
            trial.target = id;
        }
        view.fields.new_model.update(cx, |input, cx| input.set_text("", cx));
        view.fields.new_model_kind = None;
        cx.notify();
    }

    pub fn remove_model(&mut self, model: usize, cx: &mut Context<Self>) {
        let Some(index) = self.settings_view.as_ref().map(|view| view.provider) else { return };
        let removed_id = self.settings.providers.get(index).and_then(|p| p.models.get(model)).map(|m| m.id.clone());
        self.settings.remove_model(index, model);
        self.save_settings(cx);
        if let Some(view) = self.settings_view.as_mut() && let Some(id) = removed_id {
            view.trials.retain(|trial| trial.target != id);
        }
        cx.notify();
    }

    pub fn reset_translation_prompt(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = &self.settings_view { view.translation_prompt.update(cx, |input, cx| input.set_text("", cx)) }
        self.settings.translation_prompt.clear();
        self.save_settings(cx);
        cx.notify();
    }

    // ---- 命令

    /// One more command, under a name to be changed, and its fields on show.
    pub fn add_command(&mut self, cx: &mut Context<Self>) {
        let index = self.settings.add_command();
        self.save_settings(cx);
        self.select_command(index, cx);
    }

    /// Show a command's fields under its row; a click on the one already open folds it up again.
    pub fn select_command(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(command) = self.settings.commands.get(index) else { return };
        if let Some(view) = self.settings_view.as_mut() { view.ai_tab = AiTab::Commands }
        let open = self.settings_view.as_ref().and_then(|view| view.command.as_ref()).map(|(open, _)| *open);
        let fields = (open != Some(index)).then(|| (index, CommandFields::new(command, cx)));
        if let Some(view) = self.settings_view.as_mut() { view.command = fields }
        cx.notify();
    }

    pub fn remove_command(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.settings.commands.len() { return }
        self.settings.commands.remove(index);
        if let Some(view) = self.settings_view.as_mut() { view.command = None }
        self.save_settings(cx);
        cx.notify();
    }

    /// Keep what 命令 shows as the command on show. A name that cannot be
    /// used is said beside the field, and the last good one stays.
    fn save_command(&mut self, cx: &mut Context<Self>) {
        let Some((index, fields)) = self.settings_view.as_ref().and_then(|view| view.command.as_ref()) else { return };
        let Some(mut command) = self.settings.commands.get(*index).cloned() else { return };
        let name = fields.typed_name(cx);
        if name_problem(&name, *index, &self.settings.commands).is_none() { command.name = name }
        command.about = fields.about.read(cx).text().trim().to_string();
        command.template = fields.template.read(cx).text().trim().to_string();
        cx.notify();
        if self.settings.commands[*index] == command { return }
        let index = *index;
        self.settings.commands[index] = command;
        self.save_settings(cx);
    }

    /// The proxy, the email and the NCBI key are put to use once the typing has paused, not at every letter:
    /// an address on its way to another passes through ones that could be addresses too (`http://1`), and one
    /// of those kept would be "the last good one" a text that cannot be used falls back to.
    fn settle(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_mut() else { return };
        view.typed += 1;
        let typed = view.typed;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(600)).await;
            this.update(cx, |this, cx| {
                if this.settings_view.as_ref().is_none_or(|view| view.typed != typed) { return }
                this.save_proxy(cx);
                this.save_contact(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Keep the global proxy as typed, and put it to use once it is an
    /// address that can work; until then the last good one stays in force, and the page says so. An address
    /// written without its scheme is kept as the HTTP proxy it is read as.
    fn save_proxy(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_mut() else { return };
        let typed = view.proxy.read(cx).text().trim().to_string();
        cx.notify();
        if wisp_core::settings::proxy_problem(&typed, false).is_some() { return }
        let address = wisp_core::settings::proxy_address(&typed);
        if self.settings.proxy == address { return }
        (view.reach, view.reach_asked) = (None, view.reach_asked + 1);
        self.settings.proxy = address;
        // An address taken away takes its switch with it: the next one typed is in use.
        if self.settings.proxy.is_empty() { self.settings.proxy_off = false }
        wisp_net::set_proxy(&self.settings.global_proxy());
        self.save_settings(cx);
    }

    /// 网络's switch: the address stays, and requests go as if none were set, or take it again.
    pub fn toggle_proxy(&mut self, cx: &mut Context<Self>) {
        self.settings.proxy_off = !self.settings.proxy_off;
        if let Some(view) = self.settings_view.as_mut() { (view.reach, view.reach_asked) = (None, view.reach_asked + 1) }
        wisp_net::set_proxy(&self.settings.global_proxy());
        self.save_settings(cx);
        cx.notify();
    }

    /// 网络's 测试: one request the way requests now go, and whether an answer came.
    pub fn test_reach(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_mut() else { return };
        if matches!(view.reach, Some(Reach::Asking)) { return }
        (view.reach, view.reach_asked) = (Some(Reach::Asking), view.reach_asked + 1);
        let asked = view.reach_asked;
        cx.notify();
        let reply = cx.background_executor().spawn(async move { wisp_net::try_reach(REACH_URL) });
        cx.spawn(async move |this, cx| {
            let reply = reply.await;
            this.update(cx, |this, cx| {
                // The proxy was changed meanwhile, or the settings are closed: the answer is of another way out.
                let Some(view) = this.settings_view.as_mut().filter(|view| view.reach_asked == asked) else { return };
                view.reach = Some(match reply {
                    Ok(took) => Reach::Reached(took),
                    Err(err) => Reach::Failed(err),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Ask the `index`-th model of the provider on show one short question (`wisp_net::test_model`), as its tasks
    /// would ask it (its thinking too), and put the answer beside it. A model that draws is not asked.
    pub fn test_model(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_ref() else { return };
        let Some(model) = self.settings.providers.get(view.provider).and_then(|p| p.models.get(index)).cloned() else { return };
        self.run_model_trial(model.id.clone(), model, cx);
    }

    /// Ask the candidate model typed in `new_model` one short question before adding it.
    pub fn test_new_model(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.settings_view.as_ref() else { return };
        let candidate = view.fields.candidate_model(cx);
        if candidate.model.is_empty() || candidate.draws() { return }
        if self.settings.providers.get(view.provider).is_some_and(|p| p.models.iter().any(|m| m.model == candidate.model)) {
            return self.fail(tf!("{} 已经在这个服务的模型里了。", "{} is already one of this provider's models.", candidate.model), cx);
        }
        self.run_model_trial(String::new(), candidate, cx);
    }

    fn run_model_trial(&mut self, target: String, model: provider::Model, cx: &mut Context<Self>) {
        if model.draws() || model.model.trim().is_empty() { return }
        let proxy = self.settings.global_proxy();
        let Some(view) = self.settings_view.as_mut() else { return };
        let Some(provider) = self.settings.providers.get(view.provider).cloned() else { return };
        if matches!(view.trial_of(&provider, &proxy, &target, &model), Some(Tried::Asking)) { return }
        view.trial_runs += 1;
        let run = view.trial_runs;
        view.trials.retain(|trial| !(trial.asked.id == provider.id && trial.target == target));
        view.trials.push(Trial { run, target, asked: provider.clone(), proxy, model: model.clone(), tried: Tried::Asking });
        cx.notify();
        let profile = provider.profile(&model);
        let reply = cx.background_executor().spawn(async move { wisp_net::test_model(&profile) });
        cx.spawn(async move |this, cx| {
            let reply = reply.await;
            this.update(cx, |this, cx| {
                // Another test for this model was begun since, or the settings are closed.
                let Some(trial) = this.settings_view.as_mut().and_then(|view| view.trials.iter_mut().find(|trial| trial.run == run)) else { return };
                trial.tried = match reply {
                    Ok((said, took)) => Tried::Answered(took, said),
                    Err(err) => Tried::Failed(err),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Keep who the literature databases are told is asking, and tell them so from the next request on. An
    /// address still being typed is not kept: the last one that could be an address stays until this one can.
    fn save_contact(&mut self, cx: &mut Context<Self>) {
        let Some(view) = &self.settings_view else { return };
        let (email, key) = (view.contact_email.read(cx).text().trim().to_string(), view.ncbi_key.read(cx).text().trim().to_string());
        cx.notify();
        let email = if wisp_core::settings::email_problem(&email).is_none() { email } else { self.settings.contact_email.clone() };
        if (&email, &key) == (&self.settings.contact_email, &self.settings.ncbi_api_key) { return }
        (self.settings.contact_email, self.settings.ncbi_api_key) = (email, key);
        wisp_net::set_contact(self.settings.contact());
        self.save_settings(cx);
    }

    /// Keep what 模型 shows as the provider on show. Its models keep their
    /// names, ids and thinking settings.
    fn save_provider(&mut self, cx: &mut Context<Self>) {
        let Some(view) = &self.settings_view else { return };
        let text = |input: &Entity<TextInput>| input.read(cx).text().trim().to_string();
        let (fields, index) = (&view.fields, view.provider);
        let Some(saved) = self.settings.providers.get(index) else { return };
        let mut provider = saved.clone();
        (provider.name, provider.protocol, provider.base_url, provider.api_key, provider.proxy) = (text(&fields.name), view.protocol, text(&fields.base_url), text(&fields.api_key), text(&fields.proxy));
        cx.notify();
        if *saved == provider { return }
        self.settings.providers[index] = provider;
        self.save_settings(cx);
    }

    // ---- drawing

    pub fn render_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // 写作 shows a style on works of its own: read once, before the page is drawn.
        if self.settings_view.as_ref().is_some_and(|view| view.page == SettingsPage::Writing) { self.tend_style_browser() }
        let Some(view) = &self.settings_view else { return div().into_any_element() };
        let page = view.page;
        let nav = |id: &'static str, icon: &'static str, label: &'static str, active: bool, cmd: Cmd, this: &Self, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(9.))
                .h(text_px(32.))
                .px(px(8.))
                .rounded(px(7.))
                .cursor_pointer()
                .when(active, |d| d.bg(Theme::pressed()).font_weight(FontWeight::MEDIUM))
                .when(!active, |d| d.text_color(Theme::text_secondary()).hover(|s| s.bg(Theme::hover())))
                .on_click(this.on(cmd, cx))
                .child(ui::icon(icon, 16., if active { Theme::text() } else { Theme::text_secondary() }))
                .child(label)
        };
        let pages: Vec<AnyElement> = SettingsPage::ALL.into_iter().map(|p| nav(p.icon(), p.icon(), p.title(), p == page, Cmd::OpenSettings(p), self, cx).into_any_element()).collect();
        let sidebar = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_W))
            .h_full()
            .px(px(10.))
            .pb(px(10.))
            .pt(px(Self::corner_top()))
            .gap(px(2.))
            // The way back stands where the library's does: in the window's top-left corner, beside the traffic lights.
            .child(
                ui::title_strip()
                    .when(!ui::custom_titlebar(), |d| d.h(text_px(28.)))
                    .pl(px(ui::traffic_lights_width()))
                    .child(ui::ghost_button("settings-back", Some("arrow-left"), Some(t("返回").into())).occlude().on_click(self.on(Cmd::CloseSettings, cx))),
            )
            .child(div().px(px(8.)).pt(px(10.)).pb(px(8.)).font_weight(FontWeight::SEMIBOLD).text_color(Theme::text_secondary()).child(t("设置")))
            .children(pages);

        let body = match page {
            SettingsPage::General => self.general_page(cx),
            SettingsPage::Appearance => self.appearance_page(window, cx),
            SettingsPage::Shortcuts => self.shortcuts_page(cx),
            SettingsPage::Ai => self.ai_page(window, cx),
            SettingsPage::Writing => self.writing_page(window, cx),
            SettingsPage::Customize => self.customize_page(window, cx),
            SettingsPage::Network => self.network_page(window, cx),
            SettingsPage::Storage => self.storage_page(cx),
            SettingsPage::About => self.about_page(cx),
        };
        // The page's column in pixels, not as a share under a `max_w`: Taffy measures wrapped text at the
        // frame's full width but draws it at the narrower column, so the page came out taller than measured
        // and its last rows could not be scrolled to.
        let column = self.column_width(window).max(0.);
        let changed = page == SettingsPage::Shortcuts && !self.settings.shortcuts.is_empty();
        let heading = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .child(div().flex_1().min_w_0().px(px(4.)).text_size(text_px(22.)).line_height(text_px(28.)).font_weight(FontWeight::SEMIBOLD).child(page.title()))
            .when(changed, |d| d.child(link_button("shortcuts-reset", t("全部恢复默认")).on_click(self.on(Cmd::ResetShortcuts, cx))));
        let content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .bg(Theme::bg_body())
            .border_l_1()
            .border_color(Theme::line())
            .child(ui::title_strip())
            .child(
                div()
                    .id("settings-content")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&view.scroll)
                    // The scroll handle measures its direct child. Keep that child at its
                    // natural height: shrinking it hides the overflow of long tool lists.
                    // Every page stands in one frame, as wide as the widest page and centred; a narrower page
                    // keeps to its left, so the left edge does not move from page to page.
                    .child(div().flex().flex_col().flex_none().items_center().w_full().px(px(32.)).pt(px(12.)).pb(px(64.)).child(div().flex().flex_col().flex_none().w_full().max_w(px(AI_COLUMN_W)).child(div().flex().flex_col().flex_none().w(px(column)).gap(px(24.)).child(heading).child(body)))),
            );
        div().flex().size_full().min_h_0().overflow_hidden().child(sidebar).child(content).into_any_element()
    }

    /// The width of the page's column in pixels. A wrapping text field needs
    /// to be told its width, since every width above it is a share of its parent's.
    pub(crate) fn column_width(&self, window: &Window) -> f32 {
        let page = self.settings_view.as_ref().map(|view| view.page);
        (f32::from(window.viewport_size().width) - SIDEBAR_W - 65.).min(if matches!(page, Some(SettingsPage::Ai | SettingsPage::Writing | SettingsPage::Customize)) { AI_COLUMN_W } else { COLUMN_W })
    }

    fn general_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let custom = !self.settings.data_dir.trim().is_empty();
        let reveal = if cfg!(target_os = "macos") { t("在访达中显示") } else if cfg!(target_os = "windows") { t("在资源管理器中显示") } else { t("在文件管理器中显示") };
        let folder = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .when(custom, |d| d.child(link_button("data-reset", t("恢复默认")).on_click(self.on(Cmd::ResetDataDir, cx))))
            .child(button("data-reveal", reveal).on_click(self.on(Cmd::RevealDataDir, cx)))
            .child(button("data-pick", t("更改…")).on_click(self.on(Cmd::PickDataDir, cx)));
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(24.))
            .child(group(None, vec![row(t("界面语言"), Some(t("立即生效，自动保存。").into()), segmented("language", vec![("中文", Cmd::SetLanguage(Language::Zh)), ("English", Cmd::SetLanguage(Language::En))], usize::from(self.settings.language == Language::En), self, cx)).into_any_element()]))
            .child(
                group(Some(t("数据")), vec![row(t("数据目录"), Some(self.root.display().to_string().into()), folder).into_any_element()])
                    .child(note(t("所有文献库都在这个文件夹下，每个文献库是其中的一个子文件夹。改到别的目录后，首页显示那个目录里的文献库；原目录里的文献库不会被移动。"))),
            )
            .child(group(Some(t("启动")), vec![row(t("回到上次的界面"), Some(t("打开应用时回到上次离开的文献库、页面和文献；关闭后总是从首页开始。").into()), switch("restore-session", self.settings.restore_session).on_click(self.on(Cmd::ToggleRestoreSession, cx))).into_any_element()]))
            .when(crate::background::SUPPORTED, |d| {
                let detail = if cfg!(target_os = "macos") { t("关闭窗口后应用留在程序坞中，检索同步、筛选和导入照常进行；点程序坞图标回到窗口，⌘Q 退出。") } else { t("关闭窗口后应用留在通知区域，检索同步、筛选和导入照常进行；点托盘图标回到窗口，在它的菜单里退出。") };
                d.child(group(Some(t("后台")), vec![row(t("关闭窗口后在后台运行"), Some(detail.into()), switch("keep-running", self.settings.keep_running).on_click(self.on(Cmd::ToggleKeepRunning, cx))).into_any_element()]))
            })
            .child(group(
                Some(t("更新")),
                [
                    Some(row(t("自动检查更新"), Some(t("只读取一份版本信息，不发送任何数据；新版本由你决定何时安装。").into()), switch("auto-update", self.settings.auto_update).on_click(self.on(Cmd::ToggleAutoUpdate, cx))).into_any_element()),
                    // Only where the app can replace itself (`updater`); elsewhere a release is announced and no more.
                    crate::updater::SUPPORTED.then(|| row(t("自动下载更新"), Some(t("发现新版本后在后台下载并核对校验值，下载好了再问你是否重启更新；关闭后只提示有新版本。").into()), switch("auto-download", self.settings.auto_download).on_click(self.on(Cmd::ToggleAutoDownload, cx))).into_any_element()),
                    Some(self.update_row(cx)),
                ]
                .into_iter()
                .flatten()
                .collect(),
            ))
            .into_any_element()
    }

    fn appearance_page(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let mode = match self.settings.appearance.as_str() {
            "light" => 1,
            "dark" => 2,
            _ => 0,
        };
        let density_index = match self.settings.density.as_str() { "compact" => 0, "comfortable" => 2, _ => 1 };
        let set = |value: &str| Cmd::SetAppearance(value.to_string());
        let density = |value: &str| Cmd::SetDensity(value.to_string());
        let zone_index = match self.settings.journal_zone.as_str() { "xr" => 1, "jcr" => 2, _ => 0 };
        let zone = |value: &str| Cmd::SetJournalZone(value.to_string());
        let appearance = group(
            None,
            vec![
                row(crate::i18n::tc("appearance.theme", "主题"), Some(t("跟随系统时，随系统的浅色 / 深色模式切换。").into()), segmented("appearance", vec![(t("跟随系统"), set("")), (t("浅色"), set("light")), (t("深色"), set("dark"))], mode, self, cx)).into_any_element(),
                row(t("文献列表"), Some(t("紧凑显示更多文献，舒适增加行间留白。").into()), segmented("density", vec![(t("紧凑"), density("compact")), (t("标准"), density("")), (t("舒适"), density("comfortable"))], density_index, self, cx)).into_any_element(),
                row(t("期刊分区"), Some(tf!("文献列表的「分区」列显示哪一种分区。数据随应用附带：{}。", "Which partition the list's Zone column shows. The data comes with the app: {}.", t(wisp_core::journals::EDITIONS)).into()), segmented("journal-zone", vec![(t("中科院"), zone("")), (t("新锐"), zone("xr")), (t("JCR"), zone("jcr"))], zone_index, self, cx)).into_any_element(),
                row(t("增强对比度"), Some(t("加深辅助文字和分隔线，在浅色与深色主题下都生效。").into()), switch("high-contrast", self.settings.high_contrast).on_click(self.on(Cmd::ToggleHighContrast, cx))).into_any_element(),
                row(t("工作动画"), Some(t("等待助手阅读、检索或生成时，显示像素研究员动画。").into()), switch("work-animation", self.settings.work_animation).on_click(self.on(Cmd::ToggleWorkAnimation, cx))).into_any_element(),
            ],
        )
        .child(note(t("标识色属于文献库：在文献库的「编辑名称与主题」里选择，打开文献库后界面随之变色。")));
        let Some(view) = &self.settings_view else { return appearance.into_any_element() };
        let size = self.settings.font_size;
        let latin = &self.settings.font_family;
        let cjk = &self.settings.cjk_font_family;
        let missing = |family: &String| !family.is_empty() && !view.font_names.contains(family);
        // A Latin font with Chinese glyphs of its own draws Chinese itself: the fallback is never asked.
        let shadowed = !cjk.is_empty() && !latin.is_empty() && !missing(latin) && Self::draws_chinese(latin, window);
        let latin_note = t(if missing(latin) { "此字体未安装，当前使用系统默认字体。" } else { "用于英文字母、数字和符号。" }).to_string();
        let cjk_note = if missing(cjk) { t("此字体未安装，当前使用系统默认中文字体。").to_string() }
            else if shadowed { tf!("「{}」自带中文字形，汉字仍由它显示。要换中文字体，先把西文字体换成不含中文的字体。", "“{}” has Chinese glyphs of its own and still draws Chinese. To use a Chinese font here, choose a Western font without Chinese.", latin) }
            else { t("用于汉字和中文标点，西文字体里没有的字都由它显示。").to_string() };
        let step = |id, label, delta, enabled: bool, cx: &mut Context<Self>| button(id, label).min_w(px(32.))
            .when(!enabled, |d| d.opacity(0.35).cursor_default())
            .when(enabled, |d| d.on_click(self.on(Cmd::SetFontSize(size + delta), cx)));
        let sizes = div().flex().items_center().gap(px(8.))
            .child(step("font-smaller", "−", -1., size > wisp_core::settings::MIN_FONT_SIZE, cx))
            .child(div().min_w(px(52.)).text_center().child(format!("{size} px")))
            .child(step("font-larger", "+", 1., size < wisp_core::settings::MAX_FONT_SIZE, cx));
        let mut rows = Vec::new();
        for (slot, family, title, note_text) in [(FontSlot::Latin, latin, t("西文字体"), latin_note), (FontSlot::Cjk, cjk, t("中文字体"), cjk_note)] {
            let open = view.font_picker == Some(slot);
            let label = if family.is_empty() { t("系统默认").to_string() } else { family.clone() };
            // The picker is set in the system's font whatever is chosen: a font that draws letters as
            // pictures (Webdings) or has none would otherwise hide its own name and what is typed to find another.
            let choose = button(if slot == FontSlot::Latin { "font-family" } else { "font-family-cjk" }, label).font_family(theme::UI_FONT).max_w(px(280.)).overflow_hidden()
                .child(ui::icon(if open { "chevron-up" } else { "chevron-down" }, 14., Theme::text_secondary()))
                .on_click(cx.listener(move |this, _, window, cx| this.toggle_font_picker(slot, window, cx)));
            rows.push(row(title, Some(note_text.into()), choose).into_any_element());
            if !open { continue }
            let query = view.font_search.read(cx).text().trim().to_lowercase();
            let fonts: Vec<_> = view.font_names.iter().filter(|name| name.to_lowercase().contains(&query)).collect();
            let empty = fonts.is_empty();
            let height = text_px(30. * fonts.len().clamp(1, 6) as f32).min(px(200.));
            // What each font looks like for the text it is picked for.
            let sample = if slot == FontSlot::Latin { "Aa Bb 123" } else { "文献阅读" };
            let choices = fonts.into_iter().enumerate().map(|(i, name)| {
                // The name is always readable; what the font looks like is the sample beside it.
                ui::menu_row(("font-choice", i), name.clone(), name == family, None)
                    .child(div().flex_none().max_w(px(160.)).overflow_hidden().whitespace_nowrap().font_family(name.clone()).text_color(Theme::text_secondary()).child(sample))
                    .on_click(self.on(Cmd::SetFontFamily(slot, name.clone()), cx)).into_any_element()
            }).collect::<Vec<_>>();
            rows.push(div().flex().flex_col().gap(px(8.)).px(px(16.)).py(px(12.)).font_family(theme::UI_FONT)
                .child(ui::field(&view.font_search, window, cx))
                .child(ui::menu_row("font-default", t("系统默认"), family.is_empty(), None).on_click(self.on(Cmd::SetFontFamily(slot, String::new()), cx)))
                .child(div().id("font-choices").h(height).overflow_y_scroll().track_scroll(&view.font_scroll).children(choices)
                    .when(empty, |d| d.child(note(t("没有匹配的字体")))))
                .into_any_element());
        }
        rows.push(row(t("界面字号"), Some(t("11–20 px，标题与正文按比例调整。修改后自动保存。").into()), sizes).into_any_element());
        rows.push(div().flex().flex_col().gap(px(8.)).p(px(16.)).bg(Theme::bg_sunken())
            .child(div().text_size(text_px(16.)).font_weight(FontWeight::SEMIBOLD).child(t("字体预览")))
            .child(div().child("文献阅读与写作 · Reading & Writing"))
            .child(div().text_size(text_px(12.)).text_color(Theme::text_secondary()).child("Aa Bb Cc · 0123456789 · α β γ"))
            .into_any_element());
        div().flex().flex_col().gap(px(24.)).child(appearance)
            .child(group(Some(t("字体与字号")), rows))
            .child(div().flex().flex_wrap().items_center().justify_between().gap(px(12.))
                .child(note(t("仅调整应用内显示，PDF 页面和导出文档的排版保持原样。")).flex_1().min_w(px(200.)).max_w_full())
                .child(link_button("font-reset", t("恢复默认字体与字号")).on_click(self.on(Cmd::ResetTypography, cx))))
            .child(self.reading_appearance_page(cx))
            .child(group(None, vec![row(t("恢复全部外观设置"), Some(t("恢复主题、字体、对比度、列表密度和阅读排版。").into()), button("appearance-reset", t("恢复默认")).on_click(self.on(Cmd::ResetAppearance, cx))).into_any_element()]))
            .into_any_element()
    }

    fn reading_appearance_page(&self, cx: &mut Context<Self>) -> Div {
        let reading = self.settings.reading_appearance;
        let line_index = Spacing::ALL.iter().position(|value| *value == reading.line_spacing).unwrap_or(1);
        let paragraph_index = Spacing::ALL.iter().position(|value| *value == reading.paragraph_spacing).unwrap_or(1);
        let width_index = TextWidth::ALL.iter().position(|value| *value == reading.width).unwrap_or(1);
        let lines = segmented("reading-lines", Spacing::ALL.into_iter().map(|value| (t(value.label()), Cmd::SetLineSpacing(value))).collect(), line_index, self, cx);
        let paragraphs = segmented("reading-paragraphs", Spacing::ALL.into_iter().map(|value| (t(value.label()), Cmd::SetParagraphSpacing(value))).collect(), paragraph_index, self, cx);
        let widths = segmented("reading-width", TextWidth::ALL.into_iter().map(|value| (t(value.label()), Cmd::SetTextWidth(value))).collect(), width_index, self, cx);
        let preview = div().flex().flex_col().gap(px(10.)).p(px(16.)).bg(Theme::bg_sunken())
            .child(div().font_weight(FontWeight::SEMIBOLD).child(t("阅读排版预览")))
            .child(div().w_full().max_w(theme::manuscript_width()).child(crate::markdown::render("reading-preview", t("合适的行距让长段落更容易阅读。摘要、助手回答与稿件正文会随行距设置一起调整，方便连续阅读和对照资料。\n\n段落间距帮助区分不同的观点。这些设置只改变应用内的显示，导出内容仍保留原有排版。"), None, None)));
        div().flex().flex_col().gap(px(12.))
            .child(group(Some(t("阅读与写作")), vec![
                row(t("正文行距"), Some(t("调整摘要、助手回答与稿件正文的行距。").into()), lines).into_any_element(),
                row(t("段落间距"), Some(t("调整助手回答与稿件正文的段落间隔。").into()), paragraphs).into_any_element(),
                row(t("正文宽度"), Some(t("稿件正文一行最宽多少；助手的回答从这个宽度起，窗口宽时随之加宽，最多一倍半。「铺满」随窗口变宽；空间不足时自动适应窗口。").into()), widths).into_any_element(),
                preview.into_any_element(),
            ]))
            .child(div().flex().justify_end().child(link_button("reading-reset", t("恢复默认阅读排版")).on_click(self.on(Cmd::ResetReadingAppearance, cx))))
    }

    fn shortcuts_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = &self.settings_view else { return div().into_any_element() };
        let custom = &self.settings.shortcuts;
        let mut groups: Vec<Vec<AnyElement>> = GROUPS.iter().map(|_| vec![]).collect();
        for shortcut in &SHORTCUTS {
            let id = shortcut.id;
            let key = shortcuts::key_of(id, custom);
            let recording = view.recording.as_deref() == Some(id);
            let error = view.shortcut_error.as_ref().filter(|(e, _)| e == id).map(|(_, why)| why.clone());
            let line = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .w_full()
                .min_h(px(28.))
                .child(div().flex_1().min_w_0().child(t(shortcut.label)))
                .when(custom.contains_key(id) && !recording, |d| d.child(link_button(SharedString::from(format!("reset-{id}")), t("恢复默认")).on_click(self.on(Cmd::ResetShortcut(id.into()), cx))))
                .when(!key.is_empty() && !recording, |d| d.child(link_button(SharedString::from(format!("clear-{id}")), t("移除")).invisible().group_hover("shortcut-row", |s| s.visible()).on_click(self.on(Cmd::ClearShortcut(id.into()), cx))))
                .child(keycap(SharedString::from(format!("key-{id}")), &key, recording).on_click(self.on(Cmd::RecordShortcut(id.into()), cx)));
            groups[shortcut.group].push(
                div()
                    .group("shortcut-row")
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .w_full()
                    .px(px(16.))
                    .py(px(8.))
                    .child(line)
                    .when(recording, |d| d.child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(t("按下新的组合键即可保存 · Esc 取消"))))
                    .children(error.map(|why| div().text_size(text_px(12.)).line_height(text_px(17.)).text_color(Theme::danger()).child(why)))
                    .into_any_element(),
            );
        }
        // Esc is not for sale: every dialog and menu relies on it.
        groups[0].push(div().flex().items_center().w_full().px(px(16.)).py(px(8.)).min_h(px(44.)).child(div().flex_1().min_w_0().child(t("关闭对话框和菜单，返回上一层"))).child(div().min_w(px(148.)).text_size(text_px(12.)).text_color(Theme::text_tertiary()).flex().justify_center().child("Esc")).into_any_element());
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(24.))
            .children(GROUPS.iter().zip(groups).map(|(title, rows)| group(Some(t(title)), rows)))
            .child(note(t("点一个快捷键，再按下新的组合键。文本框里的编辑按键（复制、粘贴、全选等）是固定的。")))
            // The keys of 筛选 are bare letters, and fixed: shown so that they can be found.
            .child(group(Some(t("筛选")), shortcuts::SCREEN_KEYS.iter().map(|(key, what)| div().flex().items_center().w_full().px(px(16.)).py(px(8.)).min_h(px(44.)).child(div().flex_1().min_w_0().child(t(what))).child(div().min_w(px(148.)).text_size(text_px(12.)).text_color(Theme::text_tertiary()).flex().justify_center().child(*key)).into_any_element()).collect()))
            .child(note(t("筛选页里的按键是固定的：在批量的队列里对点开的那一篇、在逐篇里对当前这一篇、在确认建议里对光标所在的一行生效。正在输入文字时它们不起作用。")))
            .into_any_element()
    }

    fn ai_page(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = &self.settings_view else { return div().into_any_element() };
        let tabs = [(AiTab::Models, crate::i18n::tc("ai.models", "模型")), (AiTab::Tasks, t("任务")), (AiTab::Commands, t("命令"))];
        let bar = div().flex().items_center().gap(px(4.)).children(tabs.into_iter().map(|(tab, label)| {
            let active = view.ai_tab == tab;
            div()
                .id(("ai-tab", tab as usize))
                .flex()
                .items_center()
                .h(text_px(32.))
                .px(px(14.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(active, |d| d.bg(Theme::pressed()).font_weight(FontWeight::MEDIUM))
                .when(!active, |d| d.text_color(Theme::text_secondary()).hover(|s| s.bg(Theme::hover()).text_color(Theme::text())))
                .on_click(self.on(Cmd::AiTab(tab), cx))
                .child(label)
        }));
        let body = match view.ai_tab {
            AiTab::Models => self.models_tab(window, cx),
            AiTab::Tasks => self.tasks_tab(window, cx),
            AiTab::Commands => self.commands_tab(window, cx),
        };
        div().flex().flex_col().w_full().gap(px(20.)).child(bar).child(body).into_any_element()
    }

    /// 模型: the providers in a list at the left, the one picked at the right
    /// with its address, key and models.
    fn models_tab(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = &self.settings_view else { return div().into_any_element() };
        let settings = &self.settings;
        let providers = &settings.providers;
        let shown = view.provider.min(providers.len().saturating_sub(1));
        let warn = theme::status_colors("uncertain");
        // Which model each task ends up with, to say beside the providers and models what they do.
        let used: Vec<(Task, String)> = Task::ALL.into_iter().filter_map(|task| settings.profile_for(task).map(|p| (task, p.id))).collect();

        let rows: Vec<AnyElement> = providers
            .iter()
            .enumerate()
            .map(|(i, provider)| {
                let active = i == shown;
                let missing = provider.key().is_empty();
                let busy = used.iter().any(|(_, id)| provider.models.iter().any(|m| m.id == *id));
                let n = provider.models.len();
                let about: SharedString = if missing { t("缺 API Key").into() } else if n > 0 && provider.models.iter().all(|m| m.draws()) { tf!("{n} 个生图模型", "{n} image models").into() } else { tf!("{} 个模型", "{} models", n).into() };
                div()
                    .id(("provider", i))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .w_full()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(9.))
                    .cursor_pointer()
                    .when(active, |d| d.bg(Theme::pressed()))
                    .when(!active, |d| d.hover(|s| s.bg(Theme::hover())))
                    .on_click(self.on(Cmd::SelectProvider(i), cx))
                    .child(provider_tile(&provider.preset, 32.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(1.))
                            .child(div().truncate().font_weight(FontWeight::MEDIUM).child(provider.title()))
                            .child(div().truncate().text_size(text_px(12.)).text_color(if missing { warn.0 } else { Theme::text_tertiary() }).child(about)),
                    )
                    // In use: some task's model is one of its models.
                    .when(busy, |d| d.child(div().flex_none().size(px(7.)).rounded_full().bg(Theme::accent())))
                    .into_any_element()
            })
            .collect();
        let list = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(PROVIDER_LIST_W))
            .p(px(6.))
            .gap(px(2.))
            .border_r_1()
            .border_color(Theme::line())
            .bg(Theme::bg_sunken())
            .children(rows)
            .child(
                div()
                    .id("provider-add")
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .w_full()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(9.))
                    .cursor_pointer()
                    .text_color(Theme::text_secondary())
                    .hover(|s| s.bg(Theme::hover()).text_color(Theme::text()))
                    .on_click(self.on_menu(MenuKind::AddProvider, cx))
                    .child(div().flex().flex_none().items_center().justify_center().size(px(32.)).rounded(px(9.)).border_1().border_dashed().border_color(Theme::line_strong()).child(ui::icon("plus", 15., Theme::text_secondary())))
                    .child(t("添加服务")),
            );

        let detail = match providers.get(shown) {
            Some(provider) => self.provider_detail(provider, &used, window, cx),
            None => div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .p(px(32.))
                .child(ui::icon("sparkle", 22., Theme::text_tertiary()))
                .child(div().font_weight(FontWeight::MEDIUM).child(t("还没有模型服务")))
                .child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(t("在左边添加一个服务，填上它的 API Key。")))
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(8.))
            .child(div().flex().w_full().min_h(px(520.)).rounded(px(12.)).border_1().border_color(Theme::line()).overflow_hidden().child(list).child(detail))
            .child(note(t("一个服务可以提供多个模型，各个任务用哪一个在「任务」里选。修改会自动保存在本机的 settings.json 里。")))
            .into_any_element()
    }

    /// The right of 模型: one provider's settings and its models.
    fn provider_detail(&self, provider: &Provider, used: &[(Task, String)], window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = &self.settings_view else { return div().into_any_element() };
        let fields = &view.fields;
        let protocol_name = |protocol: Protocol| match protocol {
            Protocol::Anthropic => "Claude API",
            Protocol::OpenAI => t("OpenAI 兼容"),
            Protocol::Responses => "OpenAI Responses",
        };
        let (privacy, thinking) = match view.protocol {
            Protocol::Anthropic => (t("用到这个服务的模型时，问题和相关文献的记录、笔记、全文页面（筛选时是待判断的文献，翻译时是题目和摘要）会发送给它。API Key 以明文保存在本机的 settings.json 里。"), t("思考：「默认」不发送思考参数，由模型自己决定；其余各档按 effort 发送。「关闭」让模型不思考，适合翻译这样的任务；Claude Opus 5.5 和 Fable 不能关闭思考，对它们改用最低一档。")),
            Protocol::Responses => (t("OpenAI 的 Responses 接口（/responses），地址写到 /v1。对话不会保存在服务端：每次请求带上完整的对话，模型的推理内容以加密形式随之传回。用到这个服务的模型时，问题和相关文献的内容会发送给它。API Key 以明文保存在本机的 settings.json 里。"), t("思考：「默认」不发送推理参数。各档按 reasoning.effort 发送，「极高」「最高」按「高」，「关闭」是 none。模型不认这个参数时服务会报错，改回「默认」即可。")),
            Protocol::OpenAI => (t("适用于任何兼容 chat/completions 的服务，地址写到 /v1。用到这个服务的模型时，问题和相关文献的内容会发送给它。API Key 以明文保存在本机的 settings.json 里。"), t("思考：「默认」不发送思考参数。各档按 reasoning_effort 发送，「极高」「最高」按「高」。「关闭」因服务而异：OpenAI 是 reasoning_effort: none，百炼和硅基流动是 enable_thinking: false，DeepSeek、火山方舟、智谱、Kimi 等是 thinking: disabled。模型不认这个参数时服务会报错，改回「默认」即可。")),
        };
        let input_w = px(INPUT_W - 40.);
        let field = |input: &Entity<TextInput>| div().w(input_w).child(ui::field(input, window, cx));
        let proxy_problem = wisp_core::settings::proxy_problem(fields.proxy.read(cx).text(), true).map(|problem| crate::i18n::phrase(&problem));
        let proxy_wrong = proxy_problem.is_some();
        let proxy_detail = proxy_problem.unwrap_or_else(|| t("留空则使用「网络」页的全局代理；填 direct 表示不走代理。").to_string());
        let key_detail = if !provider.api_key.trim().is_empty() { t("已保存在本机。").to_string() } else if provider.key().is_empty() { tf!("还没有填写，环境变量 {} 也是空的。", "Not set, and {} is empty too.", provider.key_env()) } else { tf!("正在使用环境变量 {}。", "Using environment variable {}.", provider.key_env()) };
        let (name, base_url, api_key, proxy) = (field(&fields.name), field(&fields.base_url), field(&fields.api_key), field_of(&fields.proxy, INPUT_W - 40., proxy_wrong, window, cx));
        let global_proxy = self.settings.global_proxy();
        let settings_rows = [
            row(t("名称"), None, name),
            row(t("接口"), None, segmented("protocol", vec![("Claude API", Cmd::PickProtocol(Protocol::Anthropic)), (t("OpenAI 兼容"), Cmd::PickProtocol(Protocol::OpenAI)), ("Responses", Cmd::PickProtocol(Protocol::Responses))], match view.protocol { Protocol::Anthropic => 0, Protocol::OpenAI => 1, Protocol::Responses => 2 }, self, cx)),
            row(t("服务地址"), None, base_url),
            row("API Key", Some(key_detail.into()), api_key),
            row_said(t("代理"), Some((proxy_detail.into(), if proxy_wrong { Theme::danger() } else { Theme::text_tertiary() })), proxy),
        ];
        let console = provider.preset().map(|p| p.console);
        let header = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(16.))
            .pt(px(16.))
            .pb(px(12.))
            .child(provider_tile(&provider.preset, 40.))
            .child(div().flex().flex_col().flex_1().min_w_0().gap(px(1.)).child(div().truncate().text_size(text_px(16.)).line_height(text_px(22.)).font_weight(FontWeight::SEMIBOLD).child(provider.title())).child(div().truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(format!("{} · {}", protocol_name(view.protocol), provider.base_url.trim()))))
            // What the account holds, where the service says: a click asks again.
            .children(view.balance.as_ref().filter(|(id, _)| *id == provider.id).map(|(_, held)| {
                let (text, color, tip) = match held {
                    Held::Asking => (t("余额 …").to_string(), Theme::text_tertiary(), t("正在向 DeepSeek 查询余额").to_string()),
                    Held::Known(balance) if balance.available => (tf!("余额 {}", "Balance {}", balance.text()), Theme::text_secondary(), t("账户余额，来自 DeepSeek 的余额接口。点击刷新").to_string()),
                    Held::Known(balance) => (tf!("余额 {}（不足）", "Balance {} (insufficient)", balance.text()), Theme::danger(), t("余额不足以调用 API，需要充值。点击刷新").to_string()),
                    Held::Unknown(err) => (t("余额未取得").to_string(), Theme::text_tertiary(), tf!("{err} 点击重试", "{err} Click to retry")),
                };
                div().id("provider-balance").flex_none().px(px(8.)).h(text_px(24.)).flex().items_center().rounded(px(6.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).text_size(text_px(12.)).text_color(color).child(text).tooltip(ui::tip(tip)).on_click(self.on(Cmd::ReadBalance, cx))
            }))
            .children(console.map(|url| link_button("provider-console", t("获取 API Key")).on_click(self.on(Cmd::OpenUrl(url.to_string()), cx))));

        // The models, each with its locked name, its own test button, its thinking and the tasks it does.
        let models: Vec<AnyElement> = provider
            .models
            .iter()
            .enumerate()
            .map(|(i, model)| {
                let effort = ai::EFFORTS.iter().find(|(key, _)| *key == model.effort).map_or(t("默认"), |(_, name)| t(name));
                // A model that draws has no thinking to set: how it is asked for a picture stands in its place.
                let (draws, judges) = (model.draws(), model.judges());
                let asked = wisp_core::imagegen::Interface::from_key(&model.interface).map_or(t("自动"), |interface| t(interface.name()));
                let serves: Vec<&str> = used.iter().filter(|(_, id)| *id == model.id).map(|(task, _)| t(task.label())).collect();
                let tested = view.trial_of(provider, &global_proxy, &model.id, model);
                let asking = matches!(tested, Some(Tried::Asking));
                let (mark, reason) = match tested {
                    Some(tried) => {
                        let (mark, reason) = trial_mark(SharedString::from(format!("model-tested-{}", model.id)), tried);
                        (Some(mark), reason)
                    }
                    None => (None, None),
                };
                let name_cut = ui::text_width(&model.model, text_px(13.), window) > MODEL_W - 22.;
                let name_box = div()
                    .id(("model-name", i))
                    .flex()
                    .flex_none()
                    .items_center()
                    .w(px(MODEL_W))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(Theme::line())
                    .bg(Theme::bg_sunken())
                    .text_size(text_px(13.))
                    .line_height(text_px(20.))
                    .text_color(Theme::text())
                    .child(div().min_w_0().truncate().child(model.model.clone()))
                    .when(name_cut, |d| d.tooltip(ui::tip(model.model.clone())));
                let line = div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .w_full()
                    .px(px(16.))
                    .py(px(6.))
                    .child(name_box)
                    .children(mark)
                    .when(draws, |d| d.child(div().id(("model-draws", i)).flex().flex_none().items_center().gap(px(4.)).h(text_px(22.)).px(px(7.)).rounded(px(6.)).bg(Theme::accent_soft()).text_size(text_px(11.)).text_color(Theme::accent()).child(ui::icon("image", 12., Theme::accent())).child(t("生图模型")).tooltip(|_, cx| cx.new(|_| crate::write_assist::Tip(t("这个模型按描述画图，不回答问题。").to_string())).into())))
                    .when(judges, |d| d.child(div().id(("model-judges", i)).flex().flex_none().items_center().gap(px(4.)).h(text_px(22.)).px(px(7.)).rounded(px(6.)).bg(Theme::accent_soft()).text_size(text_px(11.)).text_color(Theme::accent()).child(ui::icon("list-todo", 12., Theme::accent())).child(t("判断模型")).tooltip(|_, cx| cx.new(|_| crate::write_assist::Tip(t("这个模型只在给定的选项里作判断并给出概率，不写文字。").to_string())).into())))
                    .child(div().flex_1().min_w_0().truncate().text_size(text_px(12.)).text_color(Theme::accent()).child(if draws && !serves.is_empty() { t("在用").to_string() } else { serves.join(" · ") }))
                    .child(
                        div()
                            .id(("model-effort", i))
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(4.))
                            .h(text_px(28.))
                            .px(px(10.))
                            .rounded(px(7.))
                            .cursor_pointer()
                            .text_size(text_px(12.))
                            .text_color(Theme::text_secondary())
                            .bg(Theme::hover())
                            .hover(|s| s.bg(Theme::pressed()))
                            .on_click(self.on_menu(MenuKind::ModelEffort(i), cx))
                            .child(if judges { t("类型").to_string() } else if draws { tf!("接口 · {asked}", "Interface · {asked}") } else if model.context > 0 { tf!("思考 · {effort} · {}", "Thinking · {effort} · {}", context_label(model.context)) } else { tf!("思考 · {effort}", "Thinking · {effort}") })
                            .child(ui::icon("chevron-down", 12., Theme::text_tertiary())),
                    )
                    .when(!draws, |d| {
                        let can_test = !asking && !model.model.trim().is_empty();
                        d.child(button(("model-test", i), t("测试")).when(!can_test, |d| d.opacity(0.5).cursor_default()).when(can_test, |d| d.on_click(self.on(Cmd::TestModel(i), cx))))
                    })
                    .child(ui::ghost_button(("model-remove", i), Some("x"), None).tooltip(|_, cx| cx.new(|_| crate::write_assist::Tip(t("删除这个模型").to_string())).into()).on_click(self.on(Cmd::RemoveModel(i), cx)));
                match reason {
                    Some(reason) => div().flex().flex_col().w_full().child(line).child(div().px(px(16.)).pb(px(6.)).text_size(text_px(12.)).line_height(text_px(17.)).text_color(Theme::danger()).child(reason)).into_any_element(),
                    None => line.into_any_element(),
                }
            })
            .collect();
        let candidate = fields.candidate_model(cx);
        let typed = !candidate.model.is_empty();
        let new_draws = candidate.draws();
        let new_tested = typed.then(|| view.trial_of(provider, &global_proxy, "", &candidate)).flatten();
        let new_asking = matches!(new_tested, Some(Tried::Asking));
        let new_passed = matches!(new_tested, Some(Tried::Answered(..)));
        let (new_mark, new_reason) = match new_tested {
            Some(tried) => {
                let (mark, reason) = trial_mark("model-tested-new".into(), tried);
                (Some(mark), reason)
            }
            None => (None, None),
        };
        let action = if new_draws || new_passed {
            ui::primary_button("model-add", t("添加")).h(text_px(28.)).px(px(12.)).when(!typed, |d| d.opacity(0.5).cursor_default()).when(typed, |d| d.on_click(self.on(Cmd::AddModel, cx)))
        } else {
            let can_test = typed && !new_asking;
            button("model-test-new", t("测试")).when(!can_test, |d| d.opacity(0.5).cursor_default()).when(can_test, |d| d.on_click(self.on(Cmd::TestNewModel, cx)))
        };
        let add_line = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .w_full()
            .px(px(16.))
            .py(px(6.))
            .child(div().flex_none().w(px(MODEL_W)).child(ui::field(&fields.new_model, window, cx)))
            .children(new_mark)
            .child(div().flex_1())
            .child(
                div()
                    .id("model-new-kind")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.))
                    .h(text_px(28.))
                    .px(px(10.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_size(text_px(12.))
                    .text_color(Theme::text_secondary())
                    .bg(Theme::hover())
                    .hover(|s| s.bg(Theme::pressed()))
                    .on_click(self.on_menu(MenuKind::NewModelKind, cx))
                    .child(model_kind_label(&candidate.kind))
                    .child(ui::icon("chevron-down", 12., Theme::text_tertiary())),
            )
            .child(action);
        let add = match new_reason {
            Some(reason) => div().flex().flex_col().w_full().child(add_line).child(div().px(px(16.)).pb(px(6.)).text_size(text_px(12.)).line_height(text_px(17.)).text_color(Theme::danger()).child(reason)).into_any_element(),
            None => add_line.into_any_element(),
        };
        let caption = |text: &'static str| div().px(px(16.)).pt(px(18.)).pb(px(6.)).text_size(text_px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(Theme::text_secondary()).child(text);
        let small = |text: &'static str| div().text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_tertiary()).child(text);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(header)
            .children(settings_rows.into_iter().map(|row| row.border_t_1().border_color(Theme::line())))
            .child(div().border_t_1().border_color(Theme::line()).child(caption(t("模型"))))
            .when(provider.models.is_empty(), |d| d.child(div().px(px(16.)).pb(px(4.)).child(small(t("还没有模型：在下面写上模型名称，先测试连接，通过后添加。")))))
            .children(models)
            .child(add)
            // A service whose models only draw shows nothing under 任务's other menus: say why where a chat model is added.
            .when(!provider.models.iter().any(|m| m.speaks()) && provider.models.iter().any(|m| m.draws()), |d| {
                let names: Vec<&str> = provider.models.iter().filter(|m| m.draws()).map(|m| m.model.trim()).collect();
                let text = tf!("这个服务还没有对话模型：{} 只画图，用于生图任务。在上面添加一个对话模型，「任务」里才选得到它。", "This service has no chat model yet: {} only draws pictures, for image generation. Add a chat model above to choose it under Tasks.", names.join(if crate::i18n::is_english() { ", " } else { "、" }));
                d.child(div().px(px(16.)).pt(px(2.)).pb(px(4.)).text_size(text_px(12.)).line_height(text_px(18.)).text_color(theme::status_colors("uncertain").0).child(text))
            })
            .child(div().flex().flex_col().gap(px(6.)).px(px(16.)).pt(px(14.)).pb(px(16.)).when(provider.models.iter().any(|m| m.speaks()) || provider.models.is_empty(), |d| d.child(small(thinking))).when(provider.models.iter().any(|m| m.judges()), |d| d.child(small(t("判断：带「判断」标记的是 System One 这一类模型（如 TypeSafe 的 Jev）：给它材料和事先定好的选项，它在选项里作出判断，并给每个选项一个概率，不写文字，所以又快又便宜。在「任务」里可以把「批量筛选」交给它：每篇文献得到一个结论和把握，把握不足的留给你判断，没有理由和原文摘录。它不能回答问题，别的任务用不了它；这个服务的「接口」一项对它不起作用。")))).when(provider.models.iter().any(|m| m.draws()), |d| d.child(small(t("生图：带「生图」标记的模型按描述画图，在图片库里用，名字里看得出是生图模型的会自动标上，也可以在右边的菜单里改。「接口」是向服务要图的方式：「自动」时 Gemini 官方地址用它的原生接口，别处的 Gemini 模型（中转服务）走对话接口，其余走 OpenAI 的 Images 接口（/images/generations，火山方舟的 Seedream 也是它）。描述会发送给这个服务。")))).child(small(privacy)))
            .child(div().flex_1())
            .child(row(t("删除这个服务"), Some(t("连同它的模型一起删除；用到它们的任务改用默认的模型。").into()), button("provider-remove", t("删除…")).text_color(Theme::danger()).on_click(self.on(Cmd::AskRemoveProvider(view.provider.min(self.settings.providers.len().saturating_sub(1))), cx))).border_t_1().border_color(Theme::line()))
            .into_any_element()
    }

    /// 任务: which model each task uses, and how it is asked.
    fn tasks_tab(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = &self.settings_view else { return div().into_any_element() };
        let settings = &self.settings;
        let any = settings.providers.iter().any(|p| p.models.iter().any(|m| !m.model.trim().is_empty()));
        let warn = theme::status_colors("uncertain");
        let pick = |n: usize, task: Task, this: &Self, cx: &mut Context<Self>| {
            // 生图 is done by a model that draws, or by none: it borrows no other task's model.
            let any = if task.draws() { !settings.image_profiles().is_empty() } else { any };
            let Some(used) = (if task.draws() { settings.profile_for(task) } else { Some(settings.profile_for_or_default(task)) }) else {
                return div().id(("task-model", n)).flex().flex_none().items_center().gap(px(8.)).child(div().text_size(text_px(12.)).text_color(warn.0).child(t("还没有生图模型"))).child(button(("task-model-add", n), t("去添加")).on_click(this.on(Cmd::AiTab(AiTab::Models), cx)));
            };
            let follows = task != Task::Chat && !task.draws() && settings.tasks.get(task).is_empty();
            let shell = div()
                .id(("task-model", n))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .h(text_px(30.))
                .max_w(px(360.))
                .px(px(10.))
                .rounded(px(8.))
                .when(follows, |d| d.child(div().flex_none().text_color(Theme::text_tertiary()).child(t("同助手"))))
                .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(used.title()))
                // What still keeps the model from answering is said where it is chosen.
                .child(match used.missing() {
                    Some(what) => div().min_w_0().truncate().text_size(text_px(12.)).text_color(warn.0).child(if used.provider.is_empty() { t(what).to_string() } else { format!("{} · {}", used.provider, t(what)) }),
                    None => div().min_w_0().truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(used.provider.clone()),
                });
            if any { shell.bg(Theme::hover()).hover(|s| s.bg(Theme::pressed())).cursor_pointer().child(ui::icon("chevron-down", 12., Theme::text_secondary())).on_click(this.on_menu(MenuKind::TaskModel(task), cx)) } else { shell.text_color(Theme::text_secondary()) }
        };
        // A task: its mark, its name and what uses it, and the model it is done with.
        let head = |n: usize, task: Task, icon: &'static str, about: &'static str, this: &Self, cx: &mut Context<Self>| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(14.))
                .w_full()
                .px(px(16.))
                .py(px(14.))
                .child(div().flex().flex_none().items_center().justify_center().size(px(34.)).rounded(px(9.)).bg(Theme::accent_soft()).child(ui::icon(icon, 17., Theme::accent())))
                .child(div().flex().flex_col().flex_1().min_w(px(200.)).gap(px(2.)).child(div().font_weight(FontWeight::SEMIBOLD).child(t(task.label()))).child(div().text_size(text_px(12.)).line_height(text_px(17.)).text_color(Theme::text_tertiary()).child(t(about))))
                .child(pick(n, task, this, cx))
                .into_any_element()
        };
        const WORKERS: [usize; 6] = [1, 2, 3, 4, 6, 8];
        let workers: Vec<(&'static str, Cmd)> = ["1", "2", "3", "4", "6", "8"].into_iter().zip(WORKERS).map(|(name, count)| (name, Cmd::SetScreenWorkers(count))).collect();
        let voices: Vec<(&'static str, Cmd)> = wisp_core::speech::VOICES.iter().map(|(key, label)| (t(label), Cmd::SetSpeechVoice(key.to_string()))).collect();
        let translators: Vec<(&'static str, Cmd)> = ["1", "2", "3", "4", "6", "8"].into_iter().zip(WORKERS).map(|(name, count)| (name, Cmd::SetTranslateWorkers(count))).collect();
        let sub = |row: Div| row.pl(px(64.)).border_t_1().border_color(Theme::line()).into_any_element();
        // In pixels, less the card's border and padding: the instructions wrap.
        let wide = self.column_width(window) - 34.;
        let prompt = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(16.))
            .py(px(12.))
            .border_t_1()
            .border_color(Theme::line())
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(16.))
                    .w(px(wide))
                    .child(div().flex().flex_col().flex_1().gap(px(2.)).child(t("提示词")).child(div().text_size(text_px(12.)).line_height(text_px(17.)).text_color(Theme::text_tertiary()).child(t("留空则用默认的要求。可以指定别的目标语言、术语的译法或输出的格式；改了以后，同一段文字按新的要求重新翻译。"))))
                    .when(!settings.translation_prompt.is_empty(), |d| d.child(link_button("translation-reset", t("恢复默认")).on_click(self.on(Cmd::ResetTranslationPrompt, cx)))),
            )
            .child(div().w(px(wide)).child(ui::field(&view.translation_prompt, window, cx)))
            .into_any_element();
        // Each card is given its width in pixels: under widths that are shares, a
        // column guesses the translation card's height at another width and
        // leaves the difference under it.
        let column = self.column_width(window);
        let card = |rows: Vec<AnyElement>| group(None, rows).w(px(column));
        div()
            .flex()
            .flex_col()
            .w(px(column))
            .gap(px(16.))
            .child(card(vec![head(0, Task::Chat, "sparkle", "助手和单篇文献的对话。向导拟检索式、图片库里看图写图注和标签、写作页的改写、润色、配引用、找引用的出处和让助手改稿也用它；看图需要能读图片的模型。", self, cx)]))
            .child(card(vec![head(1, Task::Screen, "list-todo", "逐篇阅读文献：筛选页的批量建议、复核和单篇再问，以及提取页按词条填写。文献多的时候，可以给它一个更快、更便宜的模型；也可以交给 Jev 这样的判断模型，它读摘要，只给结论和把握，不写理由，把握不足的留给你。选了判断模型时，提取仍用助手的模型。", self, cx), sub(row(t("同时阅读的篇数"), Some(t("批量筛选时同时发出的请求数。服务限速时调小。").into()), segmented("workers", workers, WORKERS.iter().position(|count| *count == settings.screen_workers).unwrap_or(usize::MAX), self, cx)))]))
            .child(card(vec![head(2, Task::Translate, "scan-text", "翻译题目、摘要、PDF 里选中的文字和对照译文的全文；写作页把你写的话变成候选句的检索词也用它。翻译用不着思考：可以给它一个把「思考」设为关闭的小模型。", self, cx), sub(row(t("同时翻译的篇数"), Some(t("批量翻译标题、摘要，或对照翻译全文的段落时同时发出的请求数。服务限速时调小。").into()), segmented("translate-workers", translators, WORKERS.iter().position(|count| *count == settings.translate_workers).unwrap_or(usize::MAX), self, cx))), prompt]))
            .child(card(vec![head(3, Task::Structure, "file-text", "写作页的准备工作：把全文按导言、方法、结果、讨论分开，以及读出扫描件页面上的文字（需要能看图的模型）。", self, cx)]))
            .child(card(vec![head(4, Task::Image, "image", "按一段描述画图：图片库里的「生成图片」。只能用带「生图」标记的模型，比如 OpenAI 的 gpt-image、Gemini 的图像模型、火山方舟的 Seedream；在「模型」里给一个服务添加这样的模型。", self, cx)]))
            // What follows is no model of these: the engines of 网页搜索 and 文字识别, and DeepSeek's speech service.
            .children(self.service_cards(view, column, window, cx))
            .child(card(vec![row(t("朗读的声音"), Some(match self.settings.speech_provider() { Some(p) if !p.key().is_empty() => tf!("「朗读」（PDF 工具栏和选中的文字）用这个声音读；「播客」不用它，两位主持人一女一男，各有自己的声音。都用 DeepSeek 的语音服务，Key 取自「{}」；按字数计费。", "Read aloud (PDF toolbar and selected text) uses this voice. Podcasts don't: their two hosts, a woman and a man, keep their own voices. Both use DeepSeek's speech service with the key of “{}”; billed by characters.", p.title()), _ => t("「朗读」（PDF 工具栏和选中的文字）用这个声音读；「播客」不用它，两位主持人一女一男，各有自己的声音。都要用 DeepSeek 的语音服务：先在「模型」里添加 DeepSeek 并填写 API Key。").to_string() }.into()), segmented("speech-voice", voices, wisp_core::speech::VOICES.iter().position(|(key, _)| *key == wisp_core::speech::voice(&settings.speech_voice)).unwrap_or(0), self, cx)).into_any_element()]))
            .child(note(t("没有单独指定模型的任务，使用助手的模型；助手没有指定时，用第一个填好了 API Key 的模型。生图除外：它只用生图模型。译过的文字保存在文献库里，不会再次请求。")))
            .into_any_element()
    }

    fn commands_tab(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = &self.settings_view else { return div().into_any_element() };
        let commands = &self.settings.commands;
        // In pixels, as for the translation instructions on 任务: the template wraps.
        let wide = self.column_width(window) - 34.;
        let mut open = view.command.as_ref().map(|(index, fields)| {
            let problem = name_problem(&fields.typed_name(cx), *index, commands);
            let wrong = problem.is_some();
            let field = |input: &Entity<TextInput>| div().w(px(INPUT_W)).child(ui::field(input, window, cx));
            let name_detail = problem.map_or_else(|| tf!("在提问框里输入 /{} 使用。", "Type /{} in the chat input to use it.", commands[*index].name), |why| tf!("{why}现在仍是 /{}。", "{why}The command is still /{}.", commands[*index].name));
            let name = div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(24.))
                .w_full()
                .min_h(px(48.))
                .px(px(16.))
                .py(px(10.))
                .child(div().flex().flex_col().flex_1().min_w_0().gap(px(2.)).child(t("名称")).child(div().text_size(text_px(12.)).line_height(text_px(17.)).text_color(if wrong { Theme::danger() } else { Theme::text_tertiary() }).child(name_detail)))
                .child(field(&fields.name));
            let template = div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(16.))
                .py(px(12.))
                .child(div().flex().flex_col().gap(px(2.)).w(px(wide)).child(t("模板")).child(div().text_size(text_px(12.)).line_height(text_px(17.)).text_color(Theme::text_tertiary()).child(t("发给模型的就是这段话。写上 {} 的地方换成命令后面的内容，这时命令后面必须写内容；不写 {} 的模板可以单独使用，后面写了内容就接在末尾。"))))
                .child(div().w(px(wide)).child(ui::field(&fields.template, window, cx)));
            let about = row(t("说明"), None, field(&fields.about));
            let remove = row(t("删除这个命令"), Some(t("用过它的对话保留着当时发出的内容，不受影响。").into()), button("command-remove", t("删除…")).text_color(Theme::danger()).on_click(self.on(Cmd::AskRemoveCommand(*index), cx)));
            (*index, div().flex().flex_col().w_full().bg(Theme::bg_sunken()).children([name, about, template, remove].into_iter().map(|row| row.border_t_1().border_color(Theme::line()))))
        });
        let mut own: Vec<AnyElement> = commands
            .iter()
            .enumerate()
            .map(|(i, command)| {
                let shown = open.as_ref().is_some_and(|(index, _)| *index == i);
                let about = if command.template.trim().is_empty() { t("还没有写模板").to_string() } else if command.about.is_empty() { command.template.split_whitespace().collect::<Vec<_>>().join(" ") } else { command.about.clone() };
                let head = div()
                    .id(("command", i))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .w_full()
                    .min_h(px(48.))
                    .px(px(16.))
                    .py(px(9.))
                    .cursor_pointer()
                    .hover(|s| s.bg(Theme::hover()))
                    .on_click(self.on(Cmd::SelectCommand(i), cx))
                    .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(Theme::accent()).child(format!("/{}", command.name)))
                    .child(div().flex_1().min_w_0().truncate().text_color(Theme::text_tertiary()).child(about))
                    .child(ui::icon(if shown { "chevron-up" } else { "chevron-down" }, 14., Theme::text_tertiary()));
                div().flex().flex_col().w_full().child(head).children(open.take_if(|_| shown).map(|(_, fields)| fields)).into_any_element()
            })
            .collect();
        own.push(div().flex().items_center().w_full().px(px(16.)).py(px(10.)).child(button("command-add", t("添加命令")).on_click(self.on(Cmd::AddCommand, cx))).into_any_element());
        let builtin = ai::SLASH.iter().map(|command| row(format!("/{}", command.name), Some(t(command.about).into()), div().max_w(px(280.)).truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(t(command.hint))).into_any_element()).collect();
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(24.))
            .child(group(Some(t("我的命令")), own).child(note(t("把常用的提问存成一条命令：在提问框里输入 / 就能选到它，选中后可以接着写内容，按 Enter 才发送。命令只是一段提示：它不运行脚本，也不会让助手多出工具或绕过确认；材料范围、模型和引文核对都和平常提问一样。修改会自动保存在本机的 settings.json 里，保存时不请求模型。"))))
            .child(group(Some(t("内置命令")), builtin).child(note(t("内置命令不能修改，也不能被同名的命令替换。"))))
            .into_any_element()
    }

    fn network_page(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = &self.settings_view else { return div().into_any_element() };
        // The page is given its width in pixels: under widths that are shares its height is guessed at another
        // width, short of what its wrapped notes take, and the last group could not be scrolled to.
        div()
            .flex()
            .flex_col()
            .w(px(self.column_width(window)))
            .gap(px(24.))
            .child(self.proxy_group(view, window, cx))
            .child(self.databases_group(view, window, cx))
            // Said once for the page, not under every group.
            .child(note(t("这一页填的内容以明文保存在这台电脑的 settings.json 里，命令行 wisp 用的是同一份。")))
            .into_any_element()
    }

    /// 代理: the address, with what requests take right now said under its name, and a way to try it. What is
    /// said is what is in force (`Settings::global_proxy`), not what the field holds: an address that cannot
    /// be used leaves the last good one in use, and the line says that one.
    fn proxy_group(&self, view: &SettingsView, window: &Window, cx: &mut Context<Self>) -> Div {
        let typed = view.proxy.read(cx).text().trim().to_string();
        let problem = wisp_core::settings::proxy_problem(&typed, false).map(|problem| crate::i18n::phrase(&problem));
        let in_force = self.settings.global_proxy();
        let environment = wisp_core::settings::env_proxy();
        let said: (String, Hsla) = match (&problem, in_force.is_empty(), self.settings.proxy_off && !self.settings.proxy.is_empty()) {
            (Some(problem), false, _) => (tf!("{problem}仍在使用 {in_force}。", "{problem} Still using {in_force}."), Theme::danger()),
            (Some(problem), true, _) => (problem.clone(), Theme::danger()),
            (None, false, _) => (tf!("正在使用 {in_force}。", "Using {in_force}."), Theme::success()),
            (None, true, off) => (
                match (off, environment) {
                    (true, Some(name)) => tf!("已停用，正在使用环境变量 {name}。", "Off: using the environment variable {name}."),
                    (true, None) => t("已停用，直接连接。").to_string(),
                    (false, Some(name)) => tf!("没有填写，正在使用环境变量 {name}。", "Not set: using the environment variable {name}."),
                    (false, None) => t("没有填写，直接连接。").to_string(),
                },
                Theme::text_tertiary(),
            ),
        };
        // The switch is of an address that is kept: with none there is nothing to switch.
        let address = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(field_of(&view.proxy, if self.settings.proxy.is_empty() { INPUT_W } else { INPUT_W - 44. }, problem.is_some(), window, cx))
            .when(!self.settings.proxy.is_empty(), |d| d.child(switch("proxy-on", !self.settings.proxy_off).on_click(self.on(Cmd::ToggleProxy, cx))));
        let asking = matches!(view.reach, Some(Reach::Asking));
        let found: (String, Hsla) = match &view.reach {
            None => (tf!("按现在的设置访问一次 {REACH_HOST}，看能不能连上。", "Asks {REACH_HOST} once the way requests now go, to see whether it answers."), Theme::text_tertiary()),
            Some(Reach::Asking) => (tf!("正在连接 {REACH_HOST}…", "Connecting to {REACH_HOST}…"), Theme::text_tertiary()),
            Some(Reach::Reached(took)) => (tf!("连上了 {REACH_HOST}，用时 {:.1} 秒。", "Reached {REACH_HOST} in {:.1} s.", took.as_secs_f32()), Theme::success()),
            // Through a proxy, the reason is most often the proxy's: it is named.
            Some(Reach::Failed(err)) if !in_force.is_empty() => (tf!("经 {in_force} 没有连上 {REACH_HOST}。{}", "Could not reach {REACH_HOST} through {in_force}. {}", crate::i18n::reasons(err)), Theme::danger()),
            Some(Reach::Failed(err)) => (tf!("没有连上 {REACH_HOST}。{}", "Could not reach {REACH_HOST}. {}", crate::i18n::reasons(err)), Theme::danger()),
        };
        let test = button("proxy-test", t("测试")).when(asking, |d| d.opacity(0.5).cursor_default()).when(!asking, |d| d.on_click(self.on(Cmd::TestReach, cx)));
        group(Some(t("代理")), vec![row_said(t("代理地址"), Some((said.0.into(), said.1)), address).into_any_element(), row_said(t("连接测试"), Some((found.0.into(), found.1)), test).into_any_element()])
            .child(note(t("文献检索、检查更新和模型请求都经过它。模型服务可以在「AI › 模型」里另设代理。")))
    }

    /// 文献数据库: who PubMed, Crossref and Europe PMC are told is asking. Nothing here is needed to search;
    /// each field says what it buys.
    fn databases_group(&self, view: &SettingsView, window: &Window, cx: &mut Context<Self>) -> Div {
        let problem = wisp_core::settings::email_problem(view.contact_email.read(cx).text()).map(|problem| crate::i18n::phrase(&problem));
        // The address the databases are told now: the last one that could be an address.
        let contact = self.settings.contact();
        let email: (String, Hsla) = match &problem {
            Some(problem) if !contact.email.is_empty() => (tf!("{problem}仍在使用 {}。", "{problem} Still using {}.", contact.email), Theme::danger()),
            Some(problem) => (problem.clone(), Theme::danger()),
            None => (t("留了邮箱，Crossref 和 PubMed 放得更宽。").to_string(), Theme::text_tertiary()),
        };
        let from_environment = self.settings.ncbi_api_key.trim().is_empty() && !contact.ncbi_key.is_empty();
        let key = if from_environment { tf!("正在使用环境变量 {}。", "Using environment variable {}.", "NCBI_API_KEY") } else { t("PubMed 每秒可请求 10 次，而不是 3 次。").to_string() };
        let key_field = div().flex().items_center().gap(px(4.)).w(px(INPUT_W)).child(div().flex_1().min_w_0().child(ui::field(&view.ncbi_key, window, cx))).child(link_button("ncbi-key-get", t("获取 API Key")).on_click(self.on(Cmd::OpenUrl(NCBI_KEY_PAGE.into()), cx)));
        let rows = vec![
            row_said(t("联系邮箱"), Some((email.0.into(), email.1)), field_of(&view.contact_email, INPUT_W, problem.is_some(), window, cx)).into_any_element(),
            row(tf!("NCBI API Key", "NCBI API key"), Some(key.into()), key_field).into_any_element(),
        ];
        group(Some(t("文献数据库")), rows).child(note(t("两项都可以不填，不填也能检索。邮箱只发给 PubMed、Crossref 和 Europe PMC，不用注册。")))
    }

    /// 网页搜索 and 文字识别 on 任务: services that are no model of 模型, each a card as a task has one, with
    /// its engines under it. An engine is a row: its name, whether it is the one asked, and its key or where
    /// its key comes from. There is no list to pick from beside the rows: the rows are the list.
    fn service_cards(&self, view: &SettingsView, column: f32, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let settings = &self.settings;
        let warn = theme::status_colors("uncertain");
        let small = |text: SharedString| div().flex_none().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(text);
        // Whether an engine is the one asked, could be, or was picked and cannot be.
        let mark = |n: usize, key: &'static str, lacks: &'static str, in_use: Option<&'static str>, picked: &str, usable: bool, pick: Cmd, this: &Self, cx: &mut Context<Self>| -> Option<AnyElement> {
            if in_use == Some(key) { return Some(ui::chip(t("使用中"), Theme::accent(), Theme::accent_soft()).into_any_element()) }
            if picked == key { return Some(ui::chip(t(lacks), warn.0, warn.1).into_any_element()) }
            usable.then(|| link_button(("engine-pick", n), t("改用它")).on_click(this.on(pick, cx)).into_any_element())
        };
        // DeepSeek is asked with the key of its model service: said, with the way there while there is none.
        let borrowed = |n: usize, has_key: bool, this: &Self, cx: &mut Context<Self>| -> AnyElement {
            match settings.speech_provider().filter(|provider| !provider.key().is_empty()) {
                Some(provider) => small(tf!("用「模型」里 {} 的 Key", "Uses the key of {} under Models", provider.title()).into()).into_any_element(),
                None if has_key => small(tf!("正在使用环境变量 {}", "Using environment variable {}", "DEEPSEEK_API_KEY").into()).into_any_element(),
                None => div().flex().flex_none().items_center().gap(px(8.)).child(small(t("用「模型」里 DeepSeek 服务的 Key").into())).child(button(("engine-add", n), if settings.speech_provider().is_some() { t("去填写") } else { t("去添加") }).on_click(this.on(Cmd::AiTab(AiTab::Models), cx))).into_any_element(),
            }
        };
        let own = |n: usize, input: &Entity<TextInput>, site: &'static str, this: &Self, cx: &mut Context<Self>| -> AnyElement {
            div().flex().flex_none().items_center().gap(px(4.)).child(field_of(input, INPUT_W - 60., false, window, cx)).child(link_button(("engine-key-get", n), t("获取 API Key")).on_click(this.on(Cmd::OpenUrl(site.to_string()), cx))).into_any_element()
        };
        let engine = |name: SharedString, about: Option<SharedString>, mark: Option<AnyElement>, key: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .w_full()
                .min_h(px(48.))
                .pl(px(64.))
                .pr(px(16.))
                .py(px(8.))
                .child(div().flex().flex_col().flex_1().min_w_0().gap(px(2.)).child(div().flex().items_center().gap(px(8.)).min_h(text_px(22.)).child(div().flex_none().child(name)).children(mark)).children(about.map(|about| div().text_size(text_px(12.)).line_height(text_px(17.)).text_color(Theme::text_tertiary()).child(about))))
                .child(key)
                .into_any_element()
        };
        let head = |icon: &'static str, title: &'static str, about: &'static str, hint: Option<&'static str>, id: &'static str, on: bool, toggle: Cmd, this: &Self, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .w_full()
                .px(px(16.))
                .py(px(14.))
                .child(div().flex().flex_none().items_center().justify_center().size(px(34.)).rounded(px(9.)).bg(Theme::accent_soft()).child(ui::icon(icon, 17., Theme::accent())))
                .child(div().flex().flex_col().flex_1().min_w_0().gap(px(2.)).child(div().font_weight(FontWeight::SEMIBOLD).child(t(title))).child(div().text_size(text_px(12.)).line_height(text_px(17.)).text_color(Theme::text_tertiary()).child(t(about))))
                .children(hint.filter(|_| on).map(|hint| small(t(hint).into())))
                .child(switch(id, on).on_click(this.on(toggle, cx)))
                .into_any_element()
        };
        let card = |rows: Vec<AnyElement>| group(None, rows).w(px(column)).into_any_element();

        // 网页搜索.
        let picked = settings.web_search.engine.trim();
        let on = picked != websearch::OFF;
        let in_use = websearch::ready(settings, "").ok().map(|ready| ready.engine.key);
        let mut search = vec![head("search", "网页搜索", "助手查文献库以外的信息时用它：软件文档、指南、新闻。搜索词发给所选的服务，按它的价格计费。", (in_use.is_none() && picked.is_empty()).then_some("填上一个引擎的 Key 就能用"), "web-search-on", on, Cmd::SetWebSearchEngine(if on { websearch::OFF } else { websearch::AUTO }.into()), self, cx)];
        if on {
            for (n, (searcher, input)) in websearch::ENGINES.iter().zip(&view.search_keys).enumerate() {
                let has_key = !websearch::key_for(settings, searcher).0.is_empty();
                let stored = settings.web_search.keys.get(searcher.key).is_some_and(|key| !key.trim().is_empty());
                let shared = searcher.key == "deepseek";
                let field = !shared || view.deepseek_fields[0];
                let about = if shared && field { Some(t("这里的 Key 只用于网页搜索；清空后用「模型」里 DeepSeek 服务的 Key。").into()) } else if !shared && has_key && !stored { Some(tf!("正在使用环境变量 {}。", "Using environment variable {}.", searcher.key_env).into()) } else { None };
                let key = if field { own(n, input, searcher.site, self, cx) } else { borrowed(n, has_key, self, cx) };
                search.push(engine(searcher.name.into(), about, mark(n, searcher.key, "缺少 API Key", in_use, picked, has_key, Cmd::SetWebSearchEngine(searcher.key.into()), self, cx), key));
            }
        }

        // 文字识别.
        let picked = settings.ocr.engine.trim();
        let on = picked != ocr::OFF;
        let in_use = ocr::ready(settings, "").ok().map(|ready| ready.engine.key);
        let mut reading = vec![head("scan", "文字识别", "读出扫描件这类没有文字的 PDF 页面：文献详情「文件」里的「识别页面文字」。", (in_use.is_none() && picked.is_empty()).then_some("配好下面任一种就能用"), "ocr-on", on, Cmd::SetOcrEngine(if on { ocr::OFF } else { ocr::AUTO }.into()), self, cx)];
        if on {
            let mut keys = view.ocr_keys.iter();
            for (n, reader) in ocr::ENGINES.iter().enumerate() {
                let n = n + websearch::ENGINES.len();
                let usable = ocr::ready(settings, reader.key).is_ok();
                let (about, key, lacks) = if reader.key == ocr::MODEL {
                    // The model of 整理全文, whose card above says what it still lacks.
                    let model = settings.profile_for(Task::Structure).filter(|profile| profile.missing().is_none());
                    let key = small(model.map_or_else(|| t("「整理全文」的模型还不能用").into(), |profile| profile.title().into())).into_any_element();
                    (t("上面「整理全文」的模型把页面当图片一页一页地读，要用能看图的模型。"), key, "缺少模型")
                } else {
                    let input = keys.next();
                    let key = match input.filter(|_| view.deepseek_fields[1]) {
                        Some(input) => own(n, input, reader.site, self, cx),
                        None => borrowed(n, usable, self, cx),
                    };
                    (t("整个文件上传给 DeepSeek，按页计费；每个文件最多 1000 页、100 MB。"), key, "缺少 API Key")
                };
                reading.push(engine(t(reader.name).into(), Some(about.into()), mark(n, reader.key, lacks, in_use, picked, usable, Cmd::SetOcrEngine(reader.key.into()), self, cx), key));
            }
        }
        vec![card(search), card(reading)]
    }

    fn storage_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = &self.settings_view else { return div().into_any_element() };
        let reveal_label = if cfg!(target_os = "macos") { t("在访达中显示") } else if cfg!(target_os = "windows") { t("在资源管理器中显示") } else { t("在文件管理器中显示") };
        let refresh = button("storage-refresh", t("重新计算"))
            .when(view.measuring, |d| d.opacity(0.5).cursor_default())
            .when(!view.measuring, |d| d.on_click(self.on(Cmd::MeasureStorage, cx)));
        let Some((_, space)) = view.storage.as_ref().filter(|(root, _)| *root == self.root) else {
            let waiting = div().flex().items_center().gap(px(8.)).text_color(Theme::text_secondary()).child(ui::spinner("storage-spinner", 14., Theme::text_secondary())).child(t("正在计算…"));
            return group(None, vec![row(t("数据目录"), Some(self.root.display().to_string().into()), waiting).into_any_element()]).into_any_element();
        };
        let total = space.total();
        let share = |bytes: u64| if total == 0 { 0. } else { bytes as f32 / total as f32 };
        let size = |bytes: u64| div().flex_none().min_w(px(72.)).text_right().text_color(Theme::text_secondary()).child(storage::human(bytes));
        let (shelved, drafts): (Vec<&storage::LibrarySpace>, Vec<&storage::LibrarySpace>) = space.libraries.iter().partition(|lib| !lib.draft);

        // The whole folder as one bar: each library in its colour, the largest first, what is not a library in grey at the end.
        let mut segments: Vec<(f32, Hsla)> = shelved.iter().map(|lib| (share(lib.total()), theme::accent_of(&lib.color).strong())).collect();
        let rest = drafts.iter().map(|lib| lib.total()).sum::<u64>() + space.removed + space.other;
        segments.push((share(rest), Theme::line_strong()));
        let bar = div().flex().w_full().h(px(8.)).rounded_full().overflow_hidden().bg(Theme::groove())
            .children(segments.into_iter().filter(|(part, _)| *part > 0.).map(|(part, color)| div().h_full().flex_none().w(relative(part)).bg(color)));
        let (count, papers_in_all) = (shelved.len(), shelved.iter().map(|lib| lib.paper_count).sum::<usize>());
        let counted = if crate::i18n::is_english() { format!("{count} {}, {} in all", if count == 1 { "library" } else { "libraries" }, crate::i18n::count(papers_in_all, "paper")) } else { format!("{count} 个文献库，共 {papers_in_all} 篇文献") };
        let summary = div().flex().flex_col().w_full().gap(px(12.)).px(px(16.)).py(px(14.))
            .child(div().flex().items_center().justify_between().gap(px(16.))
                .child(div().flex().flex_col().gap(px(2.)).min_w_0()
                    .child(div().text_size(text_px(22.)).line_height(text_px(28.)).font_weight(FontWeight::SEMIBOLD).child(storage::human(total)))
                    .child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(counted)))
                .child(div().flex().flex_none().items_center().gap(px(8.))
                    .when(view.measuring, |d| d.child(ui::spinner("storage-spinner", 14., Theme::text_secondary())))
                    .child(refresh)
                    .child(button("storage-reveal-root", reveal_label).on_click(self.on(Cmd::RevealDataDir, cx)))))
            .child(bar);

        // One library: its colour, name and paper count, its size and share; pressed, its parts under it.
        let open = view.storage_open.clone();
        let library_row = |key: String, color: Hsla, name: String, detail: String, parts: [u64; 6], dir: Option<PathBuf>, this: &Self, cx: &mut Context<Self>| -> AnyElement {
            let bytes: u64 = parts.iter().sum();
            let expanded = open.as_deref() == Some(key.as_str());
            let head = div()
                .id(SharedString::from(format!("storage-{key}")))
                .flex()
                .items_center()
                .gap(px(12.))
                .w_full()
                .min_h(px(48.))
                .px(px(16.))
                .py(px(8.))
                .cursor_pointer()
                .hover(|s| s.bg(Theme::hover()))
                .on_click(this.on(Cmd::StorageLibrary(key.clone()), cx))
                .child(ui::icon(if expanded { "chevron-down" } else { "chevron-right" }, 14., Theme::text_tertiary()))
                .child(div().flex_none().size(px(10.)).rounded_full().bg(color))
                .child(div().flex().flex_col().flex_1().min_w_0().gap(px(2.))
                    .child(div().truncate().child(ui::one_line(&name)))
                    .child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(detail)))
                .child(div().flex_none().w(px(96.)).h(px(4.)).rounded_full().bg(Theme::groove()).overflow_hidden().child(div().h_full().w(relative(share(bytes))).bg(color)))
                .child(div().flex_none().w(px(40.)).text_right().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(format!("{:.0}%", share(bytes) * 100.)))
                .child(size(bytes));
            let body = expanded.then(|| {
                let lines = storage::Part::ALL.iter().zip(parts).filter(|(_, bytes)| *bytes > 0).map(|(part, bytes)| {
                    div().flex().items_center().gap(px(12.)).w_full()
                        .child(div().flex_1().min_w_0().text_color(Theme::text_secondary()).child(t(part.label())))
                        .child(div().flex_none().w(px(96.)).h(px(4.)).rounded_full().bg(Theme::groove()).overflow_hidden().child(div().h_full().w(relative(if bytes == 0 { 0. } else { parts_share(bytes, &parts) })).bg(color.opacity(0.6))))
                        .child(div().flex_none().w(px(40.)))
                        .child(size(bytes))
                });
                div().flex().flex_col().w_full().gap(px(8.)).pl(px(64.)).pr(px(16.)).pb(px(12.))
                    .children(lines)
                    .children(dir.map(|dir| div().flex().pt(px(4.)).child(button(SharedString::from(format!("storage-reveal-{key}")), reveal_label).on_click(this.on(Cmd::RevealDir(dir), cx)))))
            });
            div().flex().flex_col().w_full().child(head).children(body).into_any_element()
        };
        let papers = |count: usize| tn!("{n} 篇文献", "{n}", count, "paper");
        let mut rows: Vec<AnyElement> = shelved.iter().map(|lib| library_row(lib.id.clone(), theme::accent_of(&lib.color).strong(), lib.name.clone(), papers(lib.paper_count), lib.parts, Some(lib.dir.clone()), self, cx)).collect();
        if !drafts.is_empty() {
            let mut parts = [0u64; 6];
            for lib in &drafts { for (sum, bytes) in parts.iter_mut().zip(lib.parts) { *sum += bytes } }
            rows.push(library_row("drafts".into(), Theme::text_tertiary(), t("最近对话").to_string(), tn!("{n} 个首页对话，还没有存为文献库", "{n} from the home page, not made libraries yet", drafts.len(), "conversation"), parts, None, self, cx));
        }
        let removed_rows: Vec<AnyElement> = space.removed_libraries.iter().map(|lib| {
            let key = format!("removed:{}", lib.folder);
            let bytes = lib.total();
            let expanded = open.as_deref() == Some(key.as_str());
            let busy = view.purging.as_deref() == Some(lib.folder.as_str());
            let color = if lib.draft { Theme::text_tertiary() } else { theme::accent_of(&lib.color).strong().opacity(0.6) };
            let mut bits: Vec<String> = Vec::new();
            if lib.draft { bits.push(t("首页对话").to_string()) }
            if !lib.draft || lib.paper_count > 0 { bits.push(papers(lib.paper_count)) }
            if let Some(ms) = lib.removed_at {
                let date = wisp_core::time::millis_date(ms);
                if !date.is_empty() { bits.push(tf!("移除于 {date}", "Removed {date}")) }
            }
            let detail = bits.join(" · ");
            let actions = if busy {
                div().flex().flex_none().items_center().gap(px(6.)).text_size(text_px(12.)).text_color(Theme::text_tertiary())
                    .child(ui::spinner(SharedString::from(format!("storage-purging-{}", lib.folder)), 13., Theme::text_tertiary()))
                    .child(t("正在删除…"))
                    .into_any_element()
            } else {
                div().flex().flex_none().items_center().gap(px(6.))
                    .child(button(SharedString::from(format!("storage-restore-{}", lib.folder)), t("恢复")).on_click(self.on_alone(Cmd::RestoreRemovedLibrary(lib.folder.clone()), cx)))
                    .child(ui::ghost_button(SharedString::from(format!("storage-purge-{}", lib.folder)), Some("bin"), Some(t("彻底删除").into())).text_color(Theme::danger()).on_click(self.on_alone(Cmd::AskPurgeRemovedLibrary(lib.folder.clone()), cx)))
                    .into_any_element()
            };
            let head = div()
                .id(SharedString::from(format!("storage-{key}")))
                .flex()
                .items_center()
                .gap(px(12.))
                .w_full()
                .min_h(px(48.))
                .px(px(16.))
                .py(px(8.))
                .cursor_pointer()
                .hover(|s| s.bg(Theme::hover()))
                .on_click(self.on(Cmd::StorageLibrary(key.clone()), cx))
                .child(ui::icon(if expanded { "chevron-down" } else { "chevron-right" }, 14., Theme::text_tertiary()))
                .child(div().flex_none().size(px(10.)).rounded_full().bg(color))
                .child(div().flex().flex_col().flex_1().min_w_0().gap(px(2.))
                    .child(div().truncate().child(ui::one_line(&lib.name)))
                    .child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).truncate().child(detail)))
                .child(size(bytes))
                .child(actions);
            let body = expanded.then(|| {
                let lines = storage::Part::ALL.iter().zip(lib.parts).filter(|(_, bytes)| *bytes > 0).map(|(part, bytes)| {
                    div().flex().items_center().gap(px(12.)).w_full()
                        .child(div().flex_1().min_w_0().text_color(Theme::text_secondary()).child(t(part.label())))
                        .child(div().flex_none().w(px(96.)).h(px(4.)).rounded_full().bg(Theme::groove()).overflow_hidden().child(div().h_full().w(relative(if bytes == 0 { 0. } else { parts_share(bytes, &lib.parts) })).bg(color)))
                        .child(div().flex_none().w(px(40.)))
                        .child(size(bytes))
                });
                div().flex().flex_col().w_full().gap(px(8.)).pl(px(64.)).pr(px(16.)).pb(px(12.))
                    .children(lines)
                    .child(div().flex().pt(px(4.)).child(button(SharedString::from(format!("storage-reveal-{key}")), reveal_label).on_click(self.on(Cmd::RevealDir(lib.dir.clone()), cx))))
            });
            div().flex().flex_col().w_full().child(head).children(body).into_any_element()
        }).collect();
        let removed_other = space.removed_other();
        let mut aside: Vec<AnyElement> = Vec::new();
        if removed_other > 0 {
            let label = if space.removed_libraries.is_empty() { t("removed 文件夹") } else { t("removed 文件夹中的其他内容") };
            aside.push(row(label, Some(t("removed 文件夹里不属于任何文献库的文件。").into()), div().flex().items_center().gap(px(12.)).child(size(removed_other)).child(button("storage-reveal-removed", reveal_label).on_click(self.on(Cmd::RevealDir(self.root.join("removed")), cx)))).into_any_element());
        }
        if space.other > 0 {
            aside.push(row(t("其他"), Some(t("数据目录里不属于任何文献库的文件，例如引文样式。").into()), size(space.other)).into_any_element());
        }
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(24.))
            .child(group(None, vec![summary.into_any_element()]))
            .when(!rows.is_empty(), |d| d.child(group(Some(crate::i18n::tc("storage.libraries", "文献库")), rows)))
            .when(!removed_rows.is_empty(), |d| d.child(group(Some(t("已移除的文献库")), removed_rows)))
            .when(!aside.is_empty(), |d| d.child(group(Some(t("文献库之外")), aside)))
            .child(note(t("点开一个文献库，看文献文件、数据库、图片库和稿件各占多少；回收站里的文献仍算在文献文件里。已移除的文献库可以在这里恢复或彻底删除；移除的附件仍留在各自文献库的 removed 文件夹里，只有清空回收站或彻底删除文献库会从磁盘删掉文件。")))
            .into_any_element()
    }

    fn about_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let open = |id: &'static str, label: &'static str, url: String, this: &Self, cx: &mut Context<Self>| button(id, label).on_click(this.on(Cmd::OpenUrl(url), cx));
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(24.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(8.))
                    .child(img(about_icon()).size(px(88.)))
                    .child(div().pt(px(4.)).text_size(text_px(20.)).line_height(text_px(26.)).font_weight(FontWeight::SEMIBOLD).child("Wisp Study"))
                    .child(div().text_color(Theme::text_secondary()).child(tf!("版本 {}", "Version {}", env!("CARGO_PKG_VERSION"))))
                    .child(div().text_color(Theme::text_tertiary()).child(t("本地优先的文献库：收集文献、整理、预览 PDF、对文献库提问。"))),
            )
            .child(group(None, vec![self.update_row(cx)]))
            .child(group(
                None,
                vec![
                    // Begun by itself only at a first start; from here whenever it is wanted again (tour.rs).
                    row(t("新手引导"), Some(t("一步一步指给你看：建文献库、放进文献、阅读和提问。").into()), button("about-tour", t("开始")).on_click(self.on(Cmd::Tour(crate::tour::Do::Begin), cx))).into_any_element(),
                    row(t("项目主页"), Some(format!("github.com/xuzhougeng/WispStudy · {}", t("用得顺手的话，欢迎在 GitHub 上点个 star")).into()), open("about-project", t("打开"), update::PROJECT_URL.to_string(), self, cx)).into_any_element(),
                    row(t("开源许可"), Some(t("Wisp Study 以 MIT 协议发布；随附的第三方组件见 THIRD_PARTY.md。").into()), open("about-license", t("查看"), format!("{}/blob/main/THIRD_PARTY.md", update::PROJECT_URL), self, cx)).into_any_element(),
                ],
            ))
            .into_any_element()
    }
}

/// One part's share of its library, for the bar beside it.
fn parts_share(bytes: u64, parts: &[u64; 6]) -> f32 {
    let total: u64 = parts.iter().sum();
    if total == 0 { 0. } else { bytes as f32 / total as f32 }
}

/// A provider's mark: white on a tile of the service's colour.
fn provider_tile(preset: &str, size: f32) -> Div {
    let (top, bottom) = theme::provider_tile(preset);
    let mark = if provider::preset(preset).is_some() { format!("provider-{preset}") } else { "provider-custom".to_string() };
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(size * 0.28))
        .bg(linear_gradient(160., linear_color_stop(top, 0.), linear_color_stop(bottom, 1.)))
        .shadow(theme::shadow_card())
        .child(ui::icon(&mark, size * 0.56, gpui::white()))
}

#[cfg(test)]
mod settings_trial_tests {
    // Not `super::*`: that would bring in gpui's `test` attribute.
    use super::{Trial, Tried};
    use std::time::Duration;
    use wisp_core::ai::Protocol;
    use wisp_core::provider::{Model, Provider};

    fn sample_provider() -> Provider {
        Provider { id: "p1".into(), name: "DeepSeek".into(), preset: "deepseek".into(), protocol: Protocol::OpenAI, base_url: "https://api.deepseek.com".into(), api_key: "sk-1".into(), proxy: String::new(), models: vec![Model { id: "m1".into(), model: "deepseek-flash".into(), ..Default::default() }, Model { id: "m2".into(), model: "deepseek-v4-pro".into(), ..Default::default() }] }
    }

    #[test]
    fn per_model_trials_stand_independently_and_invalidate_on_change() {
        let provider = sample_provider();
        let m1 = provider.models[0].clone();
        let m2 = provider.models[1].clone();
        let candidate = Model { model: "deepseek-reasoner".into(), ..Default::default() };
        let mut trials = [
            Trial { run: 1, target: "m1".into(), asked: provider.clone(), proxy: String::new(), model: m1.clone(), tried: Tried::Answered(Duration::from_millis(400), "ready".into()) },
            Trial { run: 2, target: String::new(), asked: provider.clone(), proxy: String::new(), model: candidate.clone(), tried: Tried::Answered(Duration::from_millis(600), "ready".into()) },
        ];
        // m1 and the candidate both stand, while m2 has not been tested yet.
        assert!(trials[0].matches(&provider, "", "m1", &m1));
        assert!(!trials[0].matches(&provider, "", "m2", &m2));
        assert!(trials[1].matches(&provider, "", "", &candidate));
        // Editing the candidate's name or kind invalidates its passed test, reverting it to needing a test.
        assert!(!trials[1].matches(&provider, "", "", &Model { model: "deepseek-reasoner-v2".into(), ..candidate.clone() }));
        assert!(!trials[1].matches(&provider, "", "", &Model { kind: wisp_core::judge::KIND.into(), ..candidate.clone() }));
        assert!(!trials[1].matches(&provider, "", "", &Model { kind: wisp_core::imagegen::KIND.into(), ..candidate.clone() }));
        // Renaming the service's display label keeps the test valid; changing key, URL, protocol or proxy invalidates it.
        assert!(trials[0].matches(&Provider { name: "My DeepSeek".into(), ..provider.clone() }, "", "m1", &m1));
        assert!(!trials[0].matches(&Provider { api_key: "sk-2".into(), ..provider.clone() }, "", "m1", &m1));
        assert!(!trials[0].matches(&Provider { base_url: "https://other.example/v1".into(), ..provider.clone() }, "", "m1", &m1));
        assert!(!trials[0].matches(&Provider { protocol: Protocol::Responses, ..provider.clone() }, "", "m1", &m1));
        assert!(!trials[0].matches(&provider, "http://127.0.0.1:7890", "m1", &m1));
        // Changing m1's thinking effort invalidates m1's trial, leaving the candidate's untouched.
        assert!(!trials[0].matches(&provider, "", "m1", &Model { effort: "low".into(), ..m1 }));
        assert!(trials[1].matches(&provider, "", "", &candidate));
        // Once the candidate is added as m3, promoting its target carries the passed test onto m3.
        trials[1].target = "m3".into();
        let added = Model { id: "m3".into(), ..candidate };
        assert!(trials[1].matches(&provider, "", "m3", &added));
        assert!(!trials[1].matches(&provider, "", "", &added));
    }
}
