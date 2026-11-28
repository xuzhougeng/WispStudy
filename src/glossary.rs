//! 术语 in 写作: the page of the panel beside the manuscript (`Side::Glossary`) where the manuscript's standing
//! requirement and the library's 术语表 are seen and changed. The requirement is the manuscript's (American
//! spelling, a journal's habits), the table the library's: every manuscript of the library and its translations
//! use it (`wisp_core::glossary`). A row pairs a Chinese term with its counterpart in one other language, the one
//! the page shows (英文 unless another is chosen in its head).
//!
//! Rows are written only by what the person does here: adding one, changing one (a click on it), taking the
//! model's proposals (the sparkle: the manuscript's proper nouns with a suggested translation each, ticked, edited
//! and taken by the person), or 记入术语表 beside a one-off requirement that names a term (write_assist.rs).

use crate::app::{Cmd, Library, MenuItem, Workspace};
use crate::i18n::t;
use crate::input::{InputEvent, TextInput};
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use wisp_core::glossary::{self, Term};
use wisp_core::manuscript::LANGUAGES;
use wisp_core::settings::Task;

/// What can be done on the page.
#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// Add the row typed into the fields at the top.
    Add,
    /// Change a row, or a proposal before it is taken: its two sides go into fields in its place.
    Edit(Target),
    /// Put the fields of the row being changed back into it; Esc leaves it as it was.
    Save,
    Cancel,
    Remove(i64),
    /// The language the other side of the rows shown is in, a code of `manuscript::LANGUAGES`.
    Language(String),
    /// Ask the model for the manuscript's terms; `Stop` drops what is asked.
    Propose,
    Stop,
    Tick(usize),
    TickAll(bool),
    /// Add the ticked proposals; `Dismiss` lets go of them all.
    Take,
    Dismiss,
}

/// What a field under change belongs to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Term(i64),
    Proposal(usize),
}

/// A term the model proposed: the person ticks, edits and takes it.
pub struct Proposal {
    pub zh: String,
    pub other: String,
    pub ticked: bool,
}

/// The model's proposals: asked for, or what came.
pub struct Proposing {
    pub busy: bool,
    pub error: String,
    pub rows: Vec<Proposal>,
    pub model: String,
    /// The language their other side is in.
    pub language: String,
}

pub struct Glossary {
    /// Every row, the newest first: read with the library (`Library::reload`) and after every change here.
    pub terms: Vec<Term>,
    /// The manuscript's standing requirement as it is typed, and the manuscript it is of.
    pub requirement: Entity<TextInput>,
    pub(crate) of: String,
    add_zh: Entity<TextInput>,
    add_other: Entity<TextInput>,
    edit_zh: Entity<TextInput>,
    edit_other: Entity<TextInput>,
    pub(crate) editing: Option<Target>,
    /// The language the page shows the rows' other side in; empty: the one the manuscript makes sensible.
    pub(crate) language: String,
    pub(crate) proposing: Option<Proposing>,
    /// Counts the requests for proposals: an answer to one dropped is not shown.
    generation: u64,
    scroll: ScrollHandle,
}

impl Glossary {
    pub fn new(subscriptions: &mut Vec<Subscription>, window: &mut Window, cx: &mut Context<Workspace>) -> Glossary {
        let requirement = cx.new(|cx| TextInput::new("例如：美式拼写；第一人称用 we；数字和单位按 Nature 的习惯", cx).multiline(2));
        let add_zh = cx.new(|cx| TextInput::new("中文", cx));
        let add_other = cx.new(|cx| TextInput::new("译名", cx));
        let edit_zh = cx.new(|cx| TextInput::new("中文", cx));
        let edit_other = cx.new(|cx| TextInput::new("译名", cx));
        subscriptions.push(cx.subscribe(&requirement, |this, input, event: &InputEvent, cx| if *event == InputEvent::Changed { this.requirement_typed(input.read(cx).text().to_string(), cx) }));
        // Enter in the first field goes on to the second while that is empty; in either, with both filled, it adds the row.
        for (input, first) in [(&add_zh, true), (&add_other, false)] {
            subscriptions.push(cx.subscribe_in(input, window, move |this, _, event: &InputEvent, window, cx| {
                if *event != InputEvent::Submit { return }
                let Some(lib) = this.lib.as_ref() else { return };
                let other = lib.glossary.add_other.clone();
                if first && other.read(cx).text().trim().is_empty() { return other.read(cx).focus(window) }
                this.run_glossary(Do::Add, window, cx);
            }));
        }
        for input in [&edit_zh, &edit_other] {
            subscriptions.push(cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Submit => this.run_glossary(Do::Save, window, cx),
                InputEvent::Cancel => this.run_glossary(Do::Cancel, window, cx),
                _ => {}
            }));
        }
        Glossary { terms: vec![], requirement, of: String::new(), add_zh, add_other, edit_zh, edit_other, editing: None, language: String::new(), proposing: None, generation: 0, scroll: ScrollHandle::new() }
    }
}

impl Library {
    /// The language the glossary's rows have their other side in on the page: the one chosen there, else the
    /// manuscript's when it is not Chinese, else English.
    pub(crate) fn glossary_language(&self) -> &'static str {
        let known = |code: &str| LANGUAGES.iter().map(|(known, _)| *known).find(|known| *known == code && *known != "zh");
        known(&self.glossary.language).or_else(|| known(self.writing.language())).unwrap_or(glossary::DEFAULT_LANGUAGE)
    }

    /// The manuscript's standing requirement, as kept.
    pub(crate) fn requirement(&self) -> String {
        self.writing.current().map(|manuscript| manuscript.requirement.trim().to_string()).unwrap_or_default()
    }
}

impl Workspace {
    /// The requirement's field shows the manuscript on show: called before 写作 is drawn and when another is shown.
    pub(crate) fn glossary_attend(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.glossary.of == lib.writing.manuscript { return }
        let kept = lib.writing.current().map(|manuscript| manuscript.requirement.clone()).unwrap_or_default();
        lib.glossary.of = lib.writing.manuscript.clone();
        lib.glossary.requirement.update(cx, |input, cx| input.set_text(&kept, cx));
    }

    /// The requirement typed: kept with its manuscript as it is typed, as the title is.
    pub(crate) fn requirement_typed(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        // Set by `glossary_attend` for another manuscript: nothing was typed.
        if lib.glossary.of != lib.writing.manuscript || lib.writing.current().is_some_and(|manuscript| manuscript.requirement == text) { return }
        let id = lib.writing.manuscript.clone();
        if let Err(err) = lib.store.set_manuscript_requirement(&id, &text) { return self.fail(tf!("稿件的要求没有保存：{err:#}", "The manuscript's requirement was not saved: {err:#}"), cx) }
        if let Some(manuscript) = lib.writing.manuscripts.iter_mut().find(|manuscript| manuscript.id == id) { manuscript.requirement = text }
        cx.notify();
    }

    /// Write the glossary and read it again; a failure is said.
    fn glossary_write(&mut self, cx: &mut Context<Self>, write: impl FnOnce(&mut wisp_core::store::Store) -> anyhow::Result<()>) -> bool {
        let Some(lib) = self.lib.as_mut() else { return false };
        let result = write(&mut lib.store).and_then(|_| lib.store.glossary());
        cx.notify();
        match result {
            Ok(terms) => {
                lib.glossary.terms = terms;
                true
            }
            Err(err) => {
                self.fail(format!("{err:#}"), cx);
                false
            }
        }
    }

    /// Put a term into the glossary, as the person asked beside a one-off requirement (write_assist.rs).
    pub(crate) fn glossary_record(&mut self, zh: String, other: String, language: String, cx: &mut Context<Self>) {
        if self.glossary_write(cx, |store| store.add_term(&zh, &other, &language).map(|_| ())) { self.say(tf!("已记入术语表：{zh} · {other}", "Added to the glossary: {zh} · {other}"), cx) }
    }

    pub(crate) fn run_glossary(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let language = lib.glossary_language().to_string();
        match what {
            Do::Add => {
                let (zh, other) = (lib.glossary.add_zh.read(cx).text().trim().to_string(), lib.glossary.add_other.read(cx).text().trim().to_string());
                if zh.is_empty() || other.is_empty() { return }
                // The Chinese term may be typed in either field.
                let (zh, other) = if !zh.chars().any(wisp_core::structure::is_cjk) && other.chars().any(wisp_core::structure::is_cjk) { (other, zh) } else { (zh, other) };
                let (first, second) = (lib.glossary.add_zh.clone(), lib.glossary.add_other.clone());
                if self.glossary_write(cx, |store| store.add_term(&zh, &other, &language).map(|_| ())) {
                    for input in [&first, &second] { input.update(cx, |input, cx| input.set_text("", cx)) }
                    first.read(cx).focus(window);
                }
            }
            Do::Edit(target) => {
                let (zh, other) = match target {
                    Target::Term(id) => match lib.glossary.terms.iter().find(|term| term.id == id) {
                        Some(term) => (term.zh.clone(), term.other.clone()),
                        None => return,
                    },
                    Target::Proposal(n) => match lib.glossary.proposing.as_ref().and_then(|proposing| proposing.rows.get(n)) {
                        Some(row) => (row.zh.clone(), row.other.clone()),
                        None => return,
                    },
                };
                let glossary = &mut lib.glossary;
                glossary.editing = Some(target);
                glossary.edit_zh.update(cx, |input, cx| input.set_text(&zh, cx));
                glossary.edit_other.update(cx, |input, cx| input.set_text(&other, cx));
                glossary.edit_zh.read(cx).focus(window);
            }
            Do::Save => {
                let Some(target) = lib.glossary.editing else { return };
                let (zh, other) = (lib.glossary.edit_zh.read(cx).text().trim().to_string(), lib.glossary.edit_other.read(cx).text().trim().to_string());
                match target {
                    Target::Term(id) => {
                        if !self.glossary_write(cx, |store| store.set_term(id, &zh, &other)) { return }
                    }
                    Target::Proposal(n) => {
                        if let Some(row) = lib.glossary.proposing.as_mut().and_then(|proposing| proposing.rows.get_mut(n)) && !zh.is_empty() && !other.is_empty() {
                            (row.zh, row.other, row.ticked) = (zh, other, true);
                        }
                    }
                }
                if let Some(lib) = self.lib.as_mut() { lib.glossary.editing = None }
            }
            Do::Cancel => lib.glossary.editing = None,
            Do::Remove(id) => {
                if lib.glossary.editing == Some(Target::Term(id)) { lib.glossary.editing = None }
                self.glossary_write(cx, |store| store.remove_term(id));
            }
            Do::Language(code) => {
                lib.glossary.language = code;
                lib.glossary.editing = None;
            }
            Do::Propose => self.glossary_propose(cx),
            Do::Stop | Do::Dismiss => {
                lib.glossary.generation += 1;
                lib.glossary.proposing = None;
                if matches!(lib.glossary.editing, Some(Target::Proposal(_))) { lib.glossary.editing = None }
            }
            Do::Tick(n) => {
                if let Some(row) = lib.glossary.proposing.as_mut().and_then(|proposing| proposing.rows.get_mut(n)) { row.ticked = !row.ticked }
            }
            Do::TickAll(on) => {
                if let Some(proposing) = lib.glossary.proposing.as_mut() { proposing.rows.iter_mut().for_each(|row| row.ticked = on) }
            }
            Do::Take => {
                let Some(proposing) = lib.glossary.proposing.as_ref().filter(|proposing| !proposing.busy) else { return };
                let taken: Vec<(String, String)> = proposing.rows.iter().filter(|row| row.ticked).map(|row| (row.zh.clone(), row.other.clone())).collect();
                let language = proposing.language.clone();
                if taken.is_empty() { return }
                let count = taken.len();
                if self.glossary_write(cx, |store| taken.iter().try_for_each(|(zh, other)| store.add_term(zh, other, &language).map(|_| ()))) {
                    if let Some(lib) = self.lib.as_mut() {
                        lib.glossary.proposing = None;
                        lib.glossary.editing = None;
                    }
                    self.say(tn!("已加入 {n} 条术语", "{n} terms added", count, "term"), cx);
                }
            }
        }
        cx.notify();
    }

    /// Ask the model for the manuscript's proper nouns and a translation of each: a list for the person, written nowhere.
    fn glossary_propose(&mut self, cx: &mut Context<Self>) {
        let profile = self.settings.profile_for_or_default(Task::Chat);
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.glossary.proposing.as_ref().is_some_and(|proposing| proposing.busy) { return }
        let language = lib.glossary_language().to_string();
        let model = crate::write_assist::model_name(&profile);
        let Some(manuscript) = lib.writing.current() else { return };
        // The section on show as it is typed; the others as kept.
        let typed = lib.writing.editor.read(cx).text().to_string();
        let text: Vec<String> = manuscript.sections.iter().map(|section| format!("{}\n{}", section.name, if section.id == lib.writing.section_id { typed.as_str() } else { section.body.as_str() })).collect();
        let text = format!("{}\n\n{}", manuscript.title, text.join("\n\n"));
        let known: Vec<String> = lib.glossary.terms.iter().filter(|term| term.language == language).map(|term| term.zh.clone()).collect();
        let (system, message) = glossary::propose_prompt(&text, manuscript.language.as_str(), &language, &known);
        lib.glossary.generation += 1;
        let generation = lib.glossary.generation;
        let mut proposing = Proposing { busy: true, error: String::new(), rows: vec![], model, language };
        let request = match profile.missing() {
            Some(missing) => Err(missing.to_string()),
            None if text.trim().len() < 8 => Err(t("稿件里还没有可读的文字。").to_string()),
            None => wisp_core::ai::plain_request(&profile, &system, &message),
        };
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                (proposing.busy, proposing.error) = (false, error);
                lib.glossary.proposing = Some(proposing);
                return cx.notify();
            }
        };
        lib.glossary.proposing = Some(proposing);
        let (library, protocol) = (lib.info.id.clone(), profile.protocol);
        let work = cx.background_executor().spawn(async move { wisp_net::complete(request, protocol) });
        cx.spawn(async move |this, cx| {
            let reply = work.await;
            this.update(cx, |this, cx| {
                let Some(lib) = this.lib.as_mut().filter(|lib| lib.info.id == library && lib.glossary.generation == generation) else { return };
                let Some(proposing) = lib.glossary.proposing.as_mut() else { return };
                proposing.busy = false;
                match reply {
                    Ok(reply) => {
                        // What the table has already in that language is not proposed again.
                        let known: Vec<&str> = lib.glossary.terms.iter().filter(|term| term.language == proposing.language).map(|term| term.zh.as_str()).collect();
                        proposing.rows = glossary::parse_proposals(&reply).into_iter().filter(|(zh, _)| !known.contains(&zh.as_str())).map(|(zh, other)| Proposal { zh, other, ticked: true }).collect();
                        if proposing.rows.is_empty() { proposing.error = t("模型没有找到需要统一译法的新术语。").into() }
                    }
                    Err(error) => proposing.error = error,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The languages the rows' other side may be in.
    pub(crate) fn glossary_language_menu(&self) -> Vec<MenuItem> {
        let on = self.lib.as_ref().map_or(glossary::DEFAULT_LANGUAGE, |lib| lib.glossary_language());
        LANGUAGES.iter().filter(|(code, _)| *code != "zh").map(|(code, name)| MenuItem { label: format!("{} ({code})", t(name)).into(), checked: *code == on, note: None, cmd: Cmd::Glossary(Do::Language(code.to_string())) }).collect()
    }

    /// The page: the manuscript's requirement, then the library's glossary in the language chosen.
    pub(crate) fn render_glossary(&self, width: f32, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let lib = self.lib.as_ref().unwrap();
        let glossary = &lib.glossary;
        let language = lib.glossary_language();
        // Widths in pixels: what wraps under a share is measured at another width than it is drawn.
        let inner = width - 28.;
        let half = ((inner - 34.) / 2.).floor();
        let heading = |text: &str| div().flex_none().text_size(text_px(12.)).font_weight(FontWeight::MEDIUM).text_color(Theme::text()).child(text.to_string());
        let quiet = |text: String| div().text_size(text_px(11.)).line_height(text_px(16.)).text_color(Theme::text_tertiary()).child(text);
        let configured = self.settings.profile_for_or_default(Task::Chat).missing().is_none();
        let busy = glossary.proposing.as_ref().is_some_and(|proposing| proposing.busy);

        let requirement = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .w(px(inner))
            .child(heading(t("这份稿件的要求")))
            .child(ui::field(&glossary.requirement, window, cx))
            .child(quiet(t("翻译、换个说法、润色和仿写这份稿件时都照这个来；术语的译法写在下面的术语表里。").into()));

        let shown: Vec<&Term> = glossary.terms.iter().filter(|term| term.language == language).collect();
        let language_button = ui::ghost_button("glossary-language", None, None).flex_none().h(text_px(22.)).px(px(6.)).gap(px(4.)).text_size(text_px(12.)).child(tf!("中文 ↔ {}", "Chinese ↔ {}", t(wisp_core::model::label(&LANGUAGES, language)))).child(ui::icon("chevron-down", 11., Theme::text_tertiary())).on_click(self.on_menu(crate::menus::MenuKind::GlossaryLanguage, cx));
        // A field a model can fill is offered by a bare sparkle; what it does is said on hover.
        let propose = ui::icon_action("glossary-propose", "sparkle", if configured && !busy { Theme::accent() } else { Theme::text_tertiary() }, if configured { t("从这份稿件里找出专有名词和术语，各给一个建议译名；由你勾选、修改后再加入") } else { t("配置模型后，可以从稿件里找出专有名词并建议译名") });
        let propose = if configured && !busy { propose.on_click(self.on(Cmd::Glossary(Do::Propose), cx)) } else { propose.opacity(0.6) };
        let head = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .w(px(inner))
            .child(heading(t("术语表")))
            .child(div().id("glossary-count").flex_1().min_w_0().truncate().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(tf!("{} 条 · 本文献库共用", "{} · shared by the library", shown.len())).tooltip(ui::tip(t("这个文献库的所有稿件都用这张表：翻译时只带上这段文字里出现的术语，对照里没照表译的会标出来。"))))
            .child(language_button)
            .child(propose);
        let field_box = |input: &Entity<TextInput>| div().flex_none().w(px(half)).child(ui::field(input, window, cx));
        let add = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .w(px(inner))
            .child(field_box(&glossary.add_zh))
            .child(field_box(&glossary.add_other))
            .child(ui::icon_action("glossary-add", "plus", Theme::accent(), t("加入术语表（回车）")).on_click(self.on(Cmd::Glossary(Do::Add), cx)));

        let proposals = glossary.proposing.as_ref().map(|proposing| self.render_proposals(proposing, inner, half, window, cx));
        let rows: Vec<AnyElement> = shown
            .iter()
            .map(|term| match glossary.editing {
                Some(Target::Term(id)) if id == term.id => self.render_term_fields(inner, half, window, cx),
                _ => div()
                    .id(("glossary-term", term.id as u64))
                    .group("glossary-row")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .w(px(inner))
                    .min_h(text_px(28.))
                    .px(px(4.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(Theme::hover()))
                    .on_click(self.on(Cmd::Glossary(Do::Edit(Target::Term(term.id))), cx))
                    .child(div().flex_none().w(px(half - 4.)).text_size(text_px(13.)).line_height(text_px(19.)).text_color(Theme::text()).child(term.zh.clone()))
                    .child(div().flex_none().w(px(half - 4.)).text_size(text_px(13.)).line_height(text_px(19.)).text_color(Theme::text_secondary()).child(term.other.clone()))
                    .child(div().flex_none().invisible().group_hover("glossary-row", |s| s.visible()).child(ui::icon_action(("glossary-remove", term.id as u64), "x", Theme::text_tertiary(), t("从术语表里去掉")).on_click(self.on(Cmd::Glossary(Do::Remove(term.id)), cx))))
                    .into_any_element(),
            })
            .collect();
        let empty = (rows.is_empty() && glossary.proposing.is_none()).then(|| quiet(t("还没有术语。在上面填一对中文和译名，或点星形让模型从稿件里找出专有名词。").into()).w(px(inner)));

        div()
            .id("glossary-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&glossary.scroll)
            .child(div().flex().flex_col().flex_none().gap(px(18.)).px(px(14.)).pt(px(12.)).pb(px(20.)).child(requirement).child(div().flex().flex_col().gap(px(8.)).child(head).child(add).children(proposals).child(div().flex().flex_col().gap(px(1.)).children(rows)).children(empty)))
    }

    /// The two fields of the row being changed, in its place.
    fn render_term_fields(&self, inner: f32, half: f32, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let glossary = &self.lib.as_ref().unwrap().glossary;
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .w(px(inner))
            .child(div().flex_none().w(px(half)).child(ui::field(&glossary.edit_zh, window, cx)))
            .child(div().flex_none().w(px(half)).child(ui::field(&glossary.edit_other, window, cx)))
            .child(ui::icon_action("glossary-save", "check", Theme::accent(), t("保存（回车）；Esc 不改")).on_click(self.on(Cmd::Glossary(Do::Save), cx)))
            .into_any_element()
    }

    /// What the model proposed: each with a box to tick, a click to change it, and what to do with them.
    fn render_proposals(&self, proposing: &Proposing, inner: f32, half: f32, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let editing = self.lib.as_ref().unwrap().glossary.editing;
        let ticked = proposing.rows.iter().filter(|row| row.ticked).count();
        let head = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(ui::icon("sparkle", 12., Theme::accent()))
            .child(div().flex_none().text_size(text_px(12.)).font_weight(FontWeight::MEDIUM).child(t("建议的术语")))
            .when(proposing.busy, |d| d.child(ui::spinner("glossary-proposing", 11., Theme::accent())))
            .child(div().flex_1().min_w_0().truncate().text_right().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(proposing.model.clone()))
            .child(if proposing.busy { ui::ghost_button("glossary-stop", None, Some(t("停止").into())).h(text_px(22.)).px(px(6.)).text_size(text_px(12.)).on_click(self.on(Cmd::Glossary(Do::Stop), cx)) } else { ui::ghost_button("glossary-dismiss", Some("x"), None).size(px(22.)).min_w(px(22.)).on_click(self.on(Cmd::Glossary(Do::Dismiss), cx)) });
        let rows: Vec<AnyElement> = proposing
            .rows
            .iter()
            .enumerate()
            .map(|(n, row)| {
                if editing == Some(Target::Proposal(n)) { return self.render_term_fields(inner - 20., half - 10., window, cx) }
                div()
                    .id(("glossary-proposal", n))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_h(text_px(28.))
                    .px(px(4.))
                    .rounded(px(6.))
                    .hover(|s| s.bg(Theme::hover()))
                    .child(div().id(("glossary-tick", n)).cursor_pointer().on_click(self.on(Cmd::Glossary(Do::Tick(n)), cx)).child(crate::extract::tick_box(row.ticked, true)))
                    .child(
                        div()
                            .id(("glossary-proposal-words", n))
                            .flex()
                            .flex_none()
                            .gap(px(6.))
                            .cursor_pointer()
                            .on_click(self.on(Cmd::Glossary(Do::Edit(Target::Proposal(n))), cx))
                            .tooltip(ui::tip(t("点一下修改")))
                            .child(div().flex_none().w(px(half - 16.)).text_size(text_px(13.)).line_height(text_px(19.)).text_color(Theme::text()).child(row.zh.clone()))
                            .child(div().flex_none().w(px(half - 16.)).text_size(text_px(13.)).line_height(text_px(19.)).text_color(Theme::text_secondary()).child(row.other.clone())),
                    )
                    .into_any_element()
            })
            .collect();
        let all = ticked == proposing.rows.len();
        let actions = (!proposing.busy && !proposing.rows.is_empty()).then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .pt(px(4.))
                .child(ui::primary_button("glossary-take", tf!("加入所选（{ticked}）", "Add selected ({ticked})")).h(text_px(26.)).px(px(10.)).text_size(text_px(12.)).when(ticked == 0, |d| d.opacity(0.45)).when(ticked > 0, |d| d.on_click(self.on(Cmd::Glossary(Do::Take), cx))))
                .child(ui::ghost_button("glossary-tick-all", None, Some(if all { t("全不选") } else { t("全选") }.into())).h(text_px(24.)).px(px(7.)).text_size(text_px(12.)).on_click(self.on(Cmd::Glossary(Do::TickAll(!all)), cx)))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .w(px(inner))
            .p(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_color(Theme::line())
            .bg(Theme::bg_body())
            .child(head)
            .when(proposing.busy, |d| d.child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(t("正在读稿件，找需要统一译法的术语…"))))
            .when(!proposing.error.is_empty(), |d| d.child(ui::error_line(&proposing.error)))
            .when(!proposing.rows.is_empty() && !proposing.busy, |d| d.child(div().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(t("勾选要加入的，点一行可以改；加入前什么都不会写进术语表。"))))
            .children(rows)
            .children(actions)
            .into_any_element()
    }
}

#[cfg(debug_assertions)]
impl Workspace {
    /// The `glossary` debug step (dev.rs).
    pub(crate) fn glossary_dev(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut parts = arg.split(';');
        let what = parts.next().unwrap_or("");
        let mut next = || parts.next().unwrap_or("").to_string();
        let nth = |this: &Self, n: &str| this.lib.as_ref().and_then(|lib| { let language = lib.glossary_language(); lib.glossary.terms.iter().filter(|term| term.language == language).nth(n.parse().unwrap_or(0)).map(|term| term.id) });
        let what = match what {
            "" => return self.run(Cmd::WriteSide(crate::app::Side::Glossary), window, cx),
            "add" => {
                let (zh, other) = (next(), next());
                let Some(lib) = self.lib.as_ref() else { return };
                let (first, second) = (lib.glossary.add_zh.clone(), lib.glossary.add_other.clone());
                first.update(cx, |input, cx| input.set_text(&zh, cx));
                second.update(cx, |input, cx| input.set_text(&other, cx));
                Do::Add
            }
            "edit" => match nth(self, &next()) { Some(id) => Do::Edit(Target::Term(id)), None => return },
            "editp" => Do::Edit(Target::Proposal(next().parse().unwrap_or(0))),
            // `glossary:set;<中文>;<译名>` types into the fields of the row being changed.
            "set" => {
                let (zh, other) = (next(), next());
                let Some(lib) = self.lib.as_ref() else { return };
                let (first, second) = (lib.glossary.edit_zh.clone(), lib.glossary.edit_other.clone());
                first.update(cx, |input, cx| input.set_text(&zh, cx));
                second.update(cx, |input, cx| input.set_text(&other, cx));
                return;
            }
            "save" => Do::Save,
            "remove" => match nth(self, &next()) { Some(id) => Do::Remove(id), None => return },
            "propose" => Do::Propose,
            "tick" => Do::Tick(next().parse().unwrap_or(0)),
            "take" => Do::Take,
            "dismiss" => Do::Dismiss,
            "lang" => Do::Language(next()),
            _ => return,
        };
        self.run_glossary(what, window, cx);
    }
}
