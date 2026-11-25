//! 引用样式: the styles a manuscript can cite in, as a list with the one looked at shown beside it on works of
//! its own (`cite::sample`: a sentence as a reader reads it, and the entries of its list). The same list stands
//! in two places. On 设置 › 写作 a click chooses the style of every manuscript that chose none (the settings
//! have no 保存: a change is kept at once). In the dialog of a manuscript (「导出」 › 引用样式) a click looks
//! and 用这个样式 chooses, for that manuscript alone; its first row is to follow the settings' again.
//!
//! The list is the common built-in styles in two groups, by how they cite (`csl::NUMBERED`, `csl::BY_AUTHOR`),
//! and the ones the person added or Zotero has; what is typed into the field over it is looked for among all
//! the built-in ones. A built-in style is listed by a short name where it has one (`csl::short_name`).

use crate::app::{Cmd, Modal, Workspace};
use crate::context::Nav;
use crate::i18n::t;
use crate::input::{InputEvent, TextInput};
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::path::Path;
use wisp_core::cite;
use wisp_core::csl::{self, Listed};

/// The list, beside the style on show.
const LIST_W: f32 = 264.;
/// The dialog of a manuscript's style, where the window leaves it the room.
const DIALOG_W: f32 = 820.;

/// The list of styles and the one on show beside it, in one of its two places.
pub(crate) struct StyleBrowser {
    /// What is looked for among every style.
    search: Entity<TextInput>,
    scroll: ScrollHandle,
    /// Every built-in style by name, and the ones the person added and Zotero's: read once, and again when one is added or removed.
    bundled: Vec<Listed>,
    added: Vec<Listed>,
    /// The style on show, read and tried once: not in every frame.
    shown: Option<Shown>,
    /// How many of the next frames scroll the list to the style on show: on opening, and after a file was added (its
    /// row is at the list's end). Two, since a list drawn for the first time does not know yet that it scrolls.
    reveal: std::cell::Cell<u8>,
}

/// A style as the right side shows it.
struct Shown {
    key: String,
    /// Its title as its file gives it; empty for the two plain forms, which have no file.
    title: String,
    /// Whether it numbers the works; none for a style that could not be read.
    numbered: Option<bool>,
    sample: Result<cite::Sample, String>,
}

impl StyleBrowser {
    fn read(&mut self, root: &Path) {
        self.added = csl::added(root);
        self.shown = None;
        self.reveal.set(2);
    }

    /// Have the style of this key on show; one that is on show already is not read again.
    fn show(&mut self, key: &str, root: &Path) {
        if self.shown.as_ref().is_some_and(|shown| shown.key == key) { return }
        let style = csl::style(key, root);
        let (title, numbered) = match &style {
            Ok(cite::Style::Csl(loaded)) => (loaded.name.clone(), Some(loaded.numbered())),
            Ok(cite::Style::Numeric) => (String::new(), Some(true)),
            Ok(cite::Style::AuthorYear) => (String::new(), Some(false)),
            Err(_) => (String::new(), None),
        };
        self.shown = Some(Shown { key: key.to_string(), title, numbered, sample: style.map(|style| cite::sample(&style)).map_err(|err| format!("{err:#}")) });
    }
}

/// A line of the list: what a group is, or a style.
enum Line {
    Caption(&'static str),
    Style { key: String, label: String, note: Option<&'static str> },
}

/// The name a style is listed by: the two plain forms as what they write, a built-in style by its short name
/// where it has one, any other by its own title.
pub(crate) fn style_label(key: &str, title: &str) -> String {
    match key {
        "" | "author-year" => t("作者-年份 (Zhang et al., 2024)").to_string(),
        "numeric" => t("编号 [1]").to_string(),
        _ => csl::short_name(key).map(|short| t(short).to_string()).unwrap_or_else(|| if title.is_empty() { key.to_string() } else { title.to_string() }),
    }
}

/// A text with what its style sets apart, for a place that draws nothing raised (`Rich::raised_as_characters`).
fn styled(rich: &cite::Rich) -> StyledText {
    let rich = rich.raised_as_characters();
    let looks: Vec<_> = rich.looks.iter().filter(|(range, look)| rich.text.get(range.clone()).is_some() && (look.italic || look.bold)).map(|(range, look)| (range.clone(), HighlightStyle { font_style: look.italic.then_some(FontStyle::Italic), font_weight: look.bold.then_some(FontWeight::SEMIBOLD), ..Default::default() })).collect();
    StyledText::new(rich.text).with_highlights(looks)
}

impl Workspace {
    /// The style the manuscript being written cites in: its own, else the one the settings keep.
    pub(crate) fn cite_style_key(&self) -> String {
        let own = self.lib.as_ref().and_then(|lib| lib.writing.current()).map(|manuscript| manuscript.cite_style.clone()).unwrap_or_default();
        if own.is_empty() { self.settings.cite_style.clone() } else { own }
    }

    /// The name a style is listed by, read from its file where it is one the person added.
    pub(crate) fn cite_style_name(&self, key: &str) -> String {
        match key {
            // Where a style is only named (the export menu, the row that follows the default), the plain forms go without their example.
            "" | "author-year" => t("作者-年份").to_string(),
            "numeric" => t("编号").to_string(),
            _ if csl::short_name(key).is_none() => csl::name_of(key, &self.data_root()).unwrap_or_else(|| key.to_string()),
            _ => style_label(key, ""),
        }
    }

    /// A list of styles for one of its two places, with what is typed into its field heard: Enter there is
    /// the dialog's 用这个样式.
    pub(crate) fn style_browser_new(&mut self, window: &mut Window, cx: &mut Context<Self>) -> (StyleBrowser, Subscription) {
        // Those that cite in footnotes cannot be used here, so they are not offered.
        let bundled: Vec<Listed> = csl::bundled().into_iter().filter(|style| !csl::in_notes(&style.key)).collect();
        let search = cx.new(|cx| TextInput::new(tf!("搜索全部 {} 个内置样式", "Search all {} built-in styles", bundled.len()), cx));
        let heard = cx.subscribe_in(&search, window, |this, _, event: &InputEvent, window, cx| {
            if *event == InputEvent::Submit && matches!(this.modal, Some(Modal::CiteStyle { .. })) { return this.run(Cmd::SubmitModal, window, cx) }
            if *event != InputEvent::Changed { return }
            if let Some(browser) = this.style_browser_mut() { browser.scroll = ScrollHandle::new() }
            cx.notify();
        });
        (StyleBrowser { search, scroll: ScrollHandle::new(), bundled, added: csl::added(&self.data_root()), shown: None, reveal: std::cell::Cell::new(2) }, heard)
    }

    /// The list on show: the dialog's while it is open, else the settings'.
    fn style_browser_mut(&mut self) -> Option<&mut StyleBrowser> {
        if let Some(Modal::CiteStyle { browser, .. }) = self.modal.as_mut() { return Some(browser) }
        self.settings_view.as_mut().map(|view| &mut view.styles)
    }

    /// Before either place is drawn: the style its right side shows is read, once.
    pub(crate) fn tend_style_browser(&mut self) {
        let (root, default) = (self.data_root(), self.settings.cite_style.clone());
        let chosen = match &self.modal {
            Some(Modal::CiteStyle { chosen, .. }) => if chosen.is_empty() { default } else { chosen.clone() },
            _ => default,
        };
        if let Some(browser) = self.style_browser_mut() { browser.show(&chosen, &root) }
    }

    /// The keys over the list on show (design rule 55), from the window or from its search field, which passes
    /// the arrows on: ↑↓ go along the styles across the groups and stop at the ends, Home/End and Page Up/Down
    /// further; a move is what a click on the row is (in the dialog it looks, in the settings it chooses), and
    /// the list scrolls to it. Enter in the dialog is 用这个样式. False when no list of styles has the keys.
    pub(crate) fn style_nav(&mut self, nav: Nav, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (browser, at, follow, pick): (&StyleBrowser, String, bool, fn(String) -> Cmd) = match &self.modal {
            Some(Modal::CiteStyle { browser, chosen }) => (browser, chosen.clone(), true, Cmd::LookCiteStyle),
            None => match &self.settings_view {
                Some(view) if view.page == crate::settings::SettingsPage::Writing => (&view.styles, self.settings.cite_style.clone(), false, Cmd::DefaultCiteStyle),
                _ => return false,
            },
            _ => return false,
        };
        if nav == Nav::Open {
            if follow { self.run(Cmd::SubmitModal, window, cx) }
            return true;
        }
        let keys: Vec<String> = self.style_lines(browser, &at, follow, cx).into_iter().filter_map(|line| match line { Line::Style { key, .. } => Some(key), Line::Caption(_) => None }).collect();
        if keys.is_empty() { return true }
        let here = keys.iter().position(|key| *key == at || (!follow && at.is_empty() && key == "author-year"));
        let last = keys.len() - 1;
        // A screenful: as many rows as the list shows.
        let page = ((f32::from(browser.scroll.bounds().size.height) / f32::from(text_px(30.))).floor() as usize).saturating_sub(1).max(1);
        let to = match (nav, here) {
            (Nav::First, _) => 0,
            (Nav::Last, _) => last,
            // None on show (it is not among what the field found): the first or the last of what is.
            (Nav::Down | Nav::PageDown, None) => 0,
            (Nav::Up | Nav::PageUp, None) => last,
            (Nav::Down, Some(at)) => (at + 1).min(last),
            (Nav::Up, Some(at)) => at.saturating_sub(1),
            (Nav::PageDown, Some(at)) => (at + page).min(last),
            (Nav::PageUp, Some(at)) => at.saturating_sub(page),
            _ => return true,
        };
        if here == Some(to) { return true }
        // Over two frames, as on opening: the caption over a group's first style comes into view with it.
        browser.reveal.set(2);
        self.run(pick(keys[to].clone()), window, cx);
        cx.notify();
        true
    }

    // ---- what is done with a style

    /// 设置 › 写作: the style of every manuscript that chose none of its own.
    pub(crate) fn set_default_cite_style(&mut self, key: String, cx: &mut Context<Self>) {
        self.settings.cite_style = key;
        self.save_settings(cx);
        cx.notify();
    }

    /// 「导出」 › 引用样式: the dialog, on the style the manuscript now has.
    pub(crate) fn open_cite_styles(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chosen) = self.lib.as_ref().and_then(|lib| lib.writing.current()).map(|manuscript| manuscript.cite_style.clone()) else { return };
        let (browser, heard) = self.style_browser_new(window, cx);
        self.modal_error.clear();
        self.modal_subscriptions.clear();
        self.modal_subscriptions.push(heard);
        // The keyboard is in the search field: what is typed looks for a style, the arrows move in the list (`style_nav`).
        browser.search.read(cx).focus(window);
        self.modal = Some(Modal::CiteStyle { browser, chosen });
        cx.notify();
    }

    /// 用这个样式: the manuscript cites in the style the dialog is on; none, it follows the settings' again.
    pub(crate) fn submit_cite_style(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::CiteStyle { chosen, .. }) = &self.modal else { return };
        let chosen = chosen.clone();
        let Some(lib) = self.lib.as_mut() else { return };
        let id = lib.writing.manuscript.clone();
        if let Err(err) = lib.store.set_manuscript_cite_style(&id, &chosen) {
            self.modal_error = format!("{err:#}");
            return cx.notify();
        }
        if let Some(manuscript) = lib.writing.manuscripts.iter_mut().find(|manuscript| manuscript.id == id) { manuscript.cite_style = chosen }
        self.modal = None;
        self.modal_subscriptions.clear();
        cx.notify();
    }

    /// 添加 .csl 文件: a style as Zotero's style repository has them, taken into the data folder and put where
    /// it was asked for: the dialog goes to it, the settings choose it.
    pub(crate) fn add_cite_style(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some(t("添加").into()), extensions: Some(vec!["csl".into()]) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            this.update(cx, |this, cx| this.take_cite_style(&path, cx)).ok();
        })
        .detach();
    }

    pub(crate) fn take_cite_style(&mut self, path: &Path, cx: &mut Context<Self>) {
        let root = self.data_root();
        let style = match csl::add(&root, path) {
            Ok(style) => style,
            Err(err) => return self.fail(tf!("没有添加：{err:#}", "Not added: {err:#}"), cx),
        };
        if let Some(browser) = self.style_browser_mut() { browser.read(&root) }
        match self.modal.as_mut() {
            Some(Modal::CiteStyle { chosen, .. }) => {
                *chosen = style.key;
                self.say(tf!("已添加「{}」", "Added “{}”", style.name), cx);
            }
            _ => {
                self.settings.cite_style = style.key;
                self.save_settings(cx);
                self.say(tf!("已添加「{}」，并设为默认", "Added “{}” and made it the default", style.name), cx);
            }
        }
        cx.notify();
    }

    /// A style the person added leaves the list; its file is kept in `styles/removed/`, and the toast puts it back.
    pub(crate) fn remove_cite_style(&mut self, key: String, cx: &mut Context<Self>) {
        let root = self.data_root();
        let name = self.cite_style_name(&key);
        let (file, xml) = match csl::remove(&root, &key) {
            Ok(removed) => removed,
            Err(err) => return self.fail(tf!("没有移除：{err:#}", "Not removed: {err:#}"), cx),
        };
        // Manuscripts that followed it are back on author and year; one that chose it for itself says so when it is exported.
        let default = self.settings.cite_style == key;
        if default {
            self.settings.cite_style.clear();
            self.save_settings(cx);
        }
        if let Some(Modal::CiteStyle { chosen, .. }) = self.modal.as_mut() && *chosen == key { chosen.clear() }
        if let Some(browser) = self.style_browser_mut() { browser.read(&root) }
        self.say_undoable(tf!("已移除「{name}」，文件留在 styles/removed 里", "Removed “{name}”; its file is kept in styles/removed"), Cmd::RestoreCiteStyle { file, xml, default }, cx);
        cx.notify();
    }

    /// The toast's 撤销: the style is back under its name, and the default again if it was.
    pub(crate) fn restore_cite_style(&mut self, file: &str, xml: &str, default: bool, cx: &mut Context<Self>) {
        let root = self.data_root();
        let style = match csl::save(&root, file, xml) {
            Ok(style) => style,
            Err(err) => return self.fail(tf!("没有放回：{err:#}", "Not put back: {err:#}"), cx),
        };
        if default {
            self.settings.cite_style = style.key;
            self.save_settings(cx);
        }
        if let Some(browser) = self.style_browser_mut() { browser.read(&root) }
        self.say(tf!("已放回「{}」", "“{}” is back", style.name), cx);
        cx.notify();
    }

    pub(crate) fn reveal_cite_style(&self, key: &str, cx: &mut Context<Self>) {
        if let Some(file) = csl::file(&self.data_root(), key) { cx.reveal_path(&file) }
    }

    // ---- drawing

    /// The lines of the list: its groups, or what the field's words find among every style.
    fn style_lines(&self, browser: &StyleBrowser, at: &str, follow: bool, cx: &App) -> Vec<Line> {
        let default = self.settings.cite_style.as_str();
        let said_default = |key: &str| (key == default || (default.is_empty() && key == "author-year")).then_some("默认");
        let line = |style: &Listed| Line::Style { label: style_label(&style.key, &style.name), note: said_default(&style.key).or(style.key.starts_with("zotero:").then_some("Zotero")), key: style.key.clone() };
        let plain: Vec<Listed> = ["author-year", "numeric"].into_iter().map(|key| Listed { key: key.to_string(), name: String::new() }).collect();
        let query = browser.search.read(cx).text().trim().to_lowercase();
        if !query.is_empty() {
            // Among every style, by the name it is listed by, its own title and its key.
            let found = |style: &&Listed| [style_label(&style.key, &style.name), style.name.clone(), style.key.clone()].iter().any(|said| said.to_lowercase().contains(&query));
            return plain.iter().chain(&browser.added).chain(&browser.bundled).filter(found).map(line).collect();
        }
        let mut lines = vec![];
        // A manuscript may follow the settings': said with what that is now.
        if follow { lines.push(Line::Style { key: String::new(), label: tf!("跟随默认（{}）", "Follow the default ({})", self.cite_style_name(default)), note: None }) }
        let groups = [("通用", plain), ("顺序编码", csl::common_numbered()), ("著者-出版年", csl::common_by_author()), ("自己添加的", browser.added.clone())];
        // The style on show, or the default, that is in none of the groups (found by searching, or a file since gone) is still listed.
        let listed = |key: &str| key.is_empty() || groups.iter().any(|(_, styles)| styles.iter().any(|style| style.key == key));
        let mut others: Vec<Listed> = vec![];
        for key in [at, default] {
            if listed(key) || others.iter().any(|style| style.key == key) { continue }
            others.push(browser.bundled.iter().find(|style| style.key == key).cloned().unwrap_or_else(|| Listed { key: key.to_string(), name: String::new() }));
        }
        for (caption, styles) in groups.into_iter().chain([("其他", others)]) {
            if styles.is_empty() { continue }
            lines.push(Line::Caption(caption));
            lines.extend(styles.iter().map(line));
        }
        lines
    }

    /// The list at the left and the style on show at the right, `width` by `height`. `at` is the key of the
    /// style on show (none: the manuscript follows the settings', offered as the first row when `follow`);
    /// `pick` is what a click on a row does.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn style_browser(&self, browser: &StyleBrowser, at: &str, follow: bool, pick: fn(String) -> Cmd, width: f32, height: f32, window: &Window, cx: &mut Context<Self>) -> Div {
        let lines = self.style_lines(browser, at, follow, cx);
        let empty = lines.is_empty();
        // With no words in the field, the two plain forms answer to the keys of an older build too.
        let on = |key: &str| key == at || (!follow && at.is_empty() && key == "author-year");
        let reveal = browser.reveal.get();
        if reveal > 0 {
            browser.reveal.set(reveal - 1);
            // First to the caption over a group's first style, then to the style: the group's name stays in view above it.
            if let Some(row) = lines.iter().position(|line| matches!(line, Line::Style { key, .. } if on(key))) { browser.scroll.scroll_to_item(if reveal > 1 && row > 0 && matches!(lines[row - 1], Line::Caption(_)) { row - 1 } else { row }) }
            if reveal > 1 { cx.spawn(async move |this, cx| { this.update(cx, |_, cx| cx.notify()).ok(); }).detach() }
        }
        let rows: Vec<AnyElement> = lines
            .into_iter()
            .enumerate()
            .map(|(i, line)| match line {
                Line::Caption(caption) => div().flex_none().px(px(8.)).pt(px(if i == 0 { 2. } else { 12. })).pb(px(4.)).child(ui::caption(t(caption))).into_any_element(),
                Line::Style { key, label, note } => {
                    let shown = on(&key);
                    div()
                        .id(("cite-style", i))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(8.))
                        .h(text_px(30.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(shown, |d| d.bg(Theme::pressed()).font_weight(FontWeight::MEDIUM))
                        .when(!shown, |d| d.hover(|s| s.bg(Theme::hover())))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .children(note.map(|note| div().flex_none().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(t(note))))
                        .on_click(self.on(pick(key), cx))
                        .into_any_element()
                }
            })
            .collect();
        let list = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(LIST_W))
            .h_full()
            .border_r_1()
            .border_color(Theme::line())
            .child(div().flex_none().p(px(8.)).child(ui::field(&browser.search, window, cx)))
            .child(div().id("cite-style-list").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().track_scroll(&browser.scroll).px(px(8.)).pb(px(8.)).children(rows).when(empty, |d| d.child(div().px(px(8.)).py(px(6.)).text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_tertiary()).child(t("没有匹配的样式。期刊自己的样式可以从 Zotero 样式库下载 .csl 文件后添加。")))));
        div().flex().flex_none().w(px(width)).h(px(height)).rounded(px(12.)).border_1().border_color(Theme::line()).overflow_hidden().child(list).child(self.style_shown(browser, at, follow, (width - LIST_W - 2.).max(0.), cx))
    }

    /// The right side: the style's name, what it is, and how it reads on works of its own.
    fn style_shown(&self, browser: &StyleBrowser, at: &str, follow: bool, width: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let pane = div().id("cite-style-shown").flex().flex_col().flex_none().w(px(width)).h_full().overflow_y_scroll().px(px(20.)).py(px(16.)).gap(px(16.));
        let Some(shown) = &browser.shown else { return pane };
        let following = follow && at.is_empty();
        let name = if following { tf!("跟随默认（{}）", "Follow the default ({})", self.cite_style_name(&shown.key)) } else { style_label(&shown.key, &shown.title) };
        // What it is, in a line: its own title where it is listed by a shorter name, how it cites, where it is from.
        let mut about: Vec<String> = vec![];
        if !shown.title.is_empty() && shown.title != name { about.push(shown.title.clone()) }
        if let Some(numbered) = shown.numbered { about.push(t(if numbered { "顺序编码" } else { "著者-出版年" }).to_string()) }
        about.push(match shown.key.split_once(':') {
            Some(("csl", _)) => t("内置").to_string(),
            Some(("file", file)) => tf!("自己添加的 {file}", "Added by you, {file}"),
            Some(("zotero", file)) => tf!("Zotero 里的 {file}", "From Zotero, {file}"),
            _ => t("应用自带的简单写法").to_string(),
        });
        if following { about.push(t("在 设置 › 写作 里换了默认，这份稿件跟着换").to_string()) }
        let own = shown.key.starts_with("file:");
        let reveal = if cfg!(target_os = "macos") { t("在访达中显示") } else if cfg!(target_os = "windows") { t("在资源管理器中显示") } else { t("在文件管理器中显示") };
        let head = div()
            .flex()
            .items_start()
            .gap(px(8.))
            .child(div().flex().flex_col().flex_1().min_w_0().gap(px(3.)).child(div().text_size(text_px(15.)).line_height(text_px(21.)).font_weight(FontWeight::SEMIBOLD).child(name)).child(div().text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_tertiary()).child(about.join(" · "))))
            .when(own, |d| {
                d.child(ui::icon_action("cite-style-reveal", "folder-open", Theme::text_secondary(), reveal).size(px(28.)).on_click(self.on(Cmd::RevealCiteStyle(shown.key.clone()), cx)))
                    .child(ui::icon_action("cite-style-remove", "bin", Theme::text_secondary(), t("移除")).size(px(28.)).on_click(self.on(Cmd::RemoveCiteStyle(shown.key.clone()), cx)))
            });
        let part = |caption: &'static str, body: Div| div().flex().flex_col().gap(px(6.)).child(ui::caption(t(caption))).child(body);
        let text = || div().text_size(text_px(13.)).line_height(text_px(21.));
        match &shown.sample {
            Ok(sample) => pane.child(head).child(part("正文里", text().child(styled(&sample.sentence)))).child(part("参考文献", div().flex().flex_col().gap(px(8.)).children(sample.list.iter().map(|entry| text().child(styled(entry)))))),
            // A file that cannot be cited with says why, in the colour of what is wrong.
            Err(why) => pane.child(head).child(div().text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::danger()).child(tf!("这个样式读不了：{}", "This style cannot be read: {}", crate::i18n::phrase(why)))),
        }
    }

    /// The dialog of a manuscript's style.
    pub(crate) fn render_cite_style_dialog(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let Some(Modal::CiteStyle { browser, chosen }) = &self.modal else { return None };
        let view = window.viewport_size();
        let width = (f32::from(view.width) * 0.92).min(DIALOG_W);
        let height = (f32::from(view.height) * 0.9 - 150.).clamp(220., 500.);
        Some(
            ui::dialog(width)
                .child(ui::dialog_title(t("这份稿件的引用样式")))
                .child(div().flex().flex_col().flex_none().gap(px(8.)).px(px(20.)).py(px(8.)).child(self.style_browser(browser, chosen, true, Cmd::LookCiteStyle, width - 40., height, window, cx)).child(ui::error_line(&self.modal_error)))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(8.))
                        .px(px(20.))
                        .py(px(14.))
                        .child(ui::secondary_button("cite-style-add", t("添加 .csl 文件…")).on_click(self.on(Cmd::AddCiteStyle, cx)))
                        .child(div().flex_1())
                        .child(ui::secondary_button("modal-cancel", t("取消")).on_click(self.on(Cmd::CloseModal, cx)))
                        .child(ui::primary_button("modal-ok", t("用这个样式")).on_click(self.on(Cmd::SubmitModal, cx))),
                ),
        )
    }

    /// 设置 › 写作: whose name a Word file's comments carry, and the style manuscripts cite in.
    pub(crate) fn writing_page(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        use crate::settings::{button, group, note, row};
        let Some(view) = &self.settings_view else { return div().into_any_element() };
        let column = self.column_width(window);
        // The list scrolls in its card, which is as tall as the window leaves it: the page itself stays put.
        let height = (f32::from(window.viewport_size().height) - 300.).clamp(300., 720.);
        div()
            .flex()
            .flex_col()
            .w(px(column))
            .gap(px(24.))
            .child(group(None, vec![row(t("署名"), Some(t("导出到 Word 的批注和修订用这个名字；留空用这台电脑的用户名。").into()), div().w(px(320.)).child(ui::field(&view.author, window, cx))).into_any_element()]))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(px(8.))
                    .child(div().flex().items_center().justify_between().gap(px(12.)).px(px(4.)).child(div().font_weight(FontWeight::SEMIBOLD).text_color(Theme::text_secondary()).child(t("引用样式"))).child(button("cite-style-add", t("添加 .csl 文件…")).on_click(self.on(Cmd::AddCiteStyle, cx))))
                    .child(self.style_browser(&view.styles, &self.settings.cite_style, false, Cmd::DefaultCiteStyle, column, height, window, cx))
                    .child(note(t("点一个样式就是默认：没有单独选过的稿件都用它。某一份稿件要用别的，在写作页「导出」里的「引用样式」选。"))),
            )
            .into_any_element()
    }
}
