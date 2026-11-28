//! 检索与导入: the saved searches of a library, a trial run of one of them,
//! and the record of what the runs brought in.

use crate::i18n::{label, t};
use crate::app::{Cmd, Workspace};
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use wisp_core::model::{SCREENINGS};
use wisp_core::sources;
use wisp_core::store::{ImportBatch, Store};
use wisp_core::{libraries, time, tools};

/// How far one run of a saved search is: written by the thread that runs it (`sources::reporting`, after each
/// request), read by its card while it is drawn. Nothing is asked of the database for it.
#[derive(Default)]
pub struct SearchProgress {
    /// The records it has, those the library held included.
    pub done: AtomicUsize,
    /// How many it is to bring; 0 until the database has said its count.
    pub of: AtomicUsize,
    /// The run is over; the window stops drawing the card again for it.
    pub over: AtomicBool,
}

/// The saved searches of a library being run, and what the runs since the last time none was running came to:
/// `works.rs` lists them as one run (运行检索 3200 / 12519), so leaving the library does not stop them.
#[derive(Default)]
pub struct SearchRuns {
    pub going: HashMap<String, Arc<SearchProgress>>,
    /// Of the runs over: the records they brought, how many they were to, and why one did not finish.
    pub done: usize,
    pub total: usize,
    pub error: String,
}

impl SearchRuns {
    /// As one run: all records had and to have, the runs over and those going.
    pub fn counts(&self) -> (usize, usize) {
        self.going.values().fold((self.done, self.total), |(done, total), run| (done + run.done.load(Ordering::Relaxed), total + run.of.load(Ordering::Relaxed)))
    }
}

/// 「命中 90 篇，取回 5 篇，新增 5 篇，跳过 0 篇」, or why the run failed.
fn run_summary(batch: &ImportBatch) -> (String, bool) {
    if !batch.error.is_empty() && batch.fetched == 0 { return (tf!("失败：{}", "Failed: {}", crate::i18n::reasons(&batch.error)), true) }
    let total = batch.remote_total.map(|n| tf!("命中 {n} 篇，", "{n} results, ")).unwrap_or_default();
    // Some pages came, then one did not: what came is in the library.
    if !batch.error.is_empty() { return (tf!("{total}取回 {} 篇后中断，新增 {} 篇，跳过已有的 {} 篇。{}", "{total}stopped after retrieving {}, added {}, skipped {} existing. {}", batch.fetched, batch.added, batch.skipped, crate::i18n::reasons(&batch.error)), true) }
    (tf!("{total}取回 {} 篇，新增 {} 篇，跳过已有的 {} 篇", "{total}retrieved {}, added {}, skipped {} existing", batch.fetched, batch.added, batch.skipped), false)
}

impl Workspace {
    /// One line where a search starts, for someone who has given the databases no address: what giving one
    /// buys, the way to the field, and a way to never be told again. Gone once an address is set.
    pub fn contact_hint(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.settings.contact_hint_dismissed || !self.settings.contact_email.trim().is_empty() { return None }
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .child(ui::icon("info", 13., Theme::text_tertiary()))
                .child(div().min_w_0().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(t("填上联系邮箱，PubMed 和 Crossref 允许更大量的访问。")))
                .child(ui::ghost_button("contact-hint-go", None, Some(t("去填写").into())).on_click(self.on(Cmd::OpenSettings(crate::settings::SettingsPage::Network), cx)))
                .child(div().flex_1())
                .child(ui::icon_action("contact-hint-close", "x", Theme::text_tertiary(), t("不再提示")).on_click(self.on(Cmd::DismissContactHint, cx))),
        )
    }

    pub fn render_searches(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let mut cards: Vec<AnyElement> = vec![];
        for (i, search) in lib.searches.iter().enumerate() {
            let source = label(&sources::KINDS, &search.kind).to_string();
            let progress = lib.searching.going.get(&search.id);
            let running = progress.is_some();
            let last = match (progress, &search.last_run) {
                // How far the run is, in place of how the last one went.
                (Some(progress), _) => {
                    let (done, of) = (progress.done.load(Ordering::Relaxed), progress.of.load(Ordering::Relaxed));
                    let said = if of == 0 { t("正在检索…").to_string() } else if done >= of { tf!("取回了 {done} 篇，正在导入…", "Retrieved {done}, importing…") } else { tf!("正在取回：{done} / {of} 篇", "Retrieving: {done} / {of}") };
                    div().flex().items_center().gap(px(6.)).text_size(text_px(12.)).text_color(Theme::text_secondary()).child(ui::spinner(SharedString::from(format!("search-run-{}", search.id)), 11., Theme::accent())).child(said)
                }
                (None, Some(batch)) => {
                    let (summary, failed) = run_summary(batch);
                    div().text_size(text_px(12.)).text_color(if failed { Theme::danger() } else { Theme::text_tertiary() }).child(tf!("上次运行 {}：{summary}", "Last run {}: {summary}", time::short_time(&batch.started_at)))
                }
                (None, None) => div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(t("还没有运行过")),
            };
            let all = sources::takes_all(&search.kind, search.max_results);
            let most = sources::ceiling(&search.kind);
            // More hits than the search takes, as its last run counted them.
            let hits = search.last_run.as_ref().and_then(|batch| batch.remote_total).map(|n| n as usize).filter(|&n| n > search.max_results && search.kind != "crossref");
            // A search set to take all whose hits are more than one search can bring takes the latest of them.
            let takes = if all && hits.is_none() { t("每次取回全部命中的记录").to_string() } else { tf!("每次最多取回最新的 {} 篇", "Retrieve up to the latest {} papers each run", search.max_results) };
            // A way to take them all. Past what one search can bring there is none: the way round it is said to
            // whoever asked for all, not under every search that watches the newest of a broad query.
            let more = hits.filter(|_| !running).and_then(|n| match (all, n > most) {
                (true, _) => Some(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(tf!("命中多于一条检索式能取回的 {most} 篇：缩小检索式，或按年份拆成几条", "More hits than one search can retrieve ({most}): narrow the query, or split it by year")).into_any_element()),
                (false, false) => Some(ui::ghost_button(("take-all", i), None, Some(tf!("取回全部 {n} 篇", "Retrieve all {n}").into())).on_click(self.on(Cmd::TakeAllHits(search.id.clone()), cx)).into_any_element()),
                (false, true) => None,
            });
            let trial = lib.trial.as_ref().filter(|(id, _)| *id == search.id).map(|(_, found)| self.render_trial(found.as_ref(), cx));
            cards.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .p(px(14.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(Theme::line())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_wrap()
                            .gap(px(8.))
                            .child(div().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(search.name.clone()))
                            .when(search.name != source, |d| d.child(ui::chip(source.clone(), Theme::text_secondary(), Theme::bg_sunken())))
                            .when(!search.enabled, |d| d.child(ui::chip(t("已停用"), Theme::text_tertiary(), Theme::bg_sunken())))
                            .child(div().flex_1())
                            .child(ui::secondary_button(("try-search", i), t("试检索")).on_click(self.on(Cmd::TrySearch(search.id.clone()), cx)))
                            .child(ui::primary_button(("run-search", i), if running { t("运行中…") } else { t("运行并导入") }).when(running, |d| d.opacity(0.5)).on_click(self.on(Cmd::RunSearch(search.id.clone()), cx)))
                            .child(ui::ghost_button(("toggle-search", i), None, Some(if search.enabled { t("停用").into() } else { t("启用").into() })).on_click(self.on(Cmd::ToggleSearch(search.id.clone()), cx)))
                            .child(ui::ghost_button(("edit-search", i), Some("pen-line"), None).on_click(self.on(Cmd::EditSearch(search.id.clone()), cx)))
                            .child(ui::ghost_button(("remove-search", i), Some("bin"), None).on_click(self.on(Cmd::AskRemoveSearch(search.id.clone()), cx))),
                    )
                    .child(div().px(px(10.)).py(px(8.)).rounded(px(6.)).bg(Theme::bg_sunken()).text_size(text_px(13.)).line_height(text_px(20.)).child(search.query.clone()))
                    .child(div().flex().items_center().flex_wrap().gap(px(12.)).child(last).child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(takes)).children(more))
                    .children(trial)
                    .into_any_element(),
            );
        }

        let batches: Vec<AnyElement> = lib
            .batches
            .iter()
            .enumerate()
            .map(|(i, batch)| {
                let (summary, failed) = run_summary(batch);
                let undone = !batch.undone_at.is_empty();
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(Theme::line())
                    .child(div().flex_none().w(px(84.)).text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(time::short_time(&batch.started_at)))
                    .child(div().flex().flex_col().flex_1().min_w_0().child(div().truncate().text_size(text_px(13.)).child(crate::i18n::import_label(&batch.label))).child(div().truncate().text_size(text_px(12.)).text_color(if failed { Theme::danger() } else { Theme::text_tertiary() }).child(summary)))
                    .when(undone, |d| d.child(ui::chip(t("已撤销"), Theme::text_tertiary(), Theme::bg_sunken())))
                    .when(!undone && batch.added > 0, |d| d.child(ui::ghost_button(("undo-import", i), None, Some(t("撤销").into())).on_click(self.on(Cmd::AskUndoImport(batch.id.clone()), cx))))
                    .into_any_element()
            })
            .collect();

        let empty = cards.is_empty();
        // How often the enabled searches run on their own.
        let hours = lib.info.auto_sync_hours;
        let auto = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .px(px(16.))
            .py(px(12.))
            .child(div().pr(px(4.)).text_size(text_px(13.)).text_color(Theme::text_secondary()).child(t("自动更新")))
            .children([(0, t("关闭")), (6, t("每 6 小时")), (24, t("每天")), (168, t("每周"))].into_iter().map(|(value, name)| {
                let on = hours == value;
                div()
                    .id(("auto-sync", value as usize))
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(text_px(26.))
                    .px(px(10.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if on { Theme::accent() } else { Theme::line() })
                    .when(on, |d| d.bg(Theme::accent_soft()).text_color(Theme::accent()))
                    .text_size(text_px(12.))
                    .cursor_pointer()
                    .child(name)
                    .on_click(self.on(Cmd::SetAutoSync(value), cx))
            }))
            .child(div().pl(px(6.)).min_w_0().truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(if hours == 0 { t("只在点「运行并导入」时访问数据库。") } else { t("应用开着时按这个间隔运行已启用的检索式；关着时错过的，下次打开时补上。") }));
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                ui::column_head()
                    .gap(px(8.))
                    .child(div().min_w_0().truncate().text_size(text_px(ui::HEAD_TITLE)).font_weight(FontWeight::SEMIBOLD).child(t("检索与导入")))
                    .child(ui::hint("searches-hint", t("为文献数据库保存检索式：先试检索，满意了再运行导入。导入的文献都是「待判断」，已有的会跳过，每次导入可以整批撤销。")))
                    .child(div().flex_1())
                    .when(!empty, |d| d.child(ui::secondary_button("run-all-searches", t("全部运行")).on_click(self.on(Cmd::RunAllSearches, cx))))
                    .child(ui::secondary_button("open-guide", t("让助手拟检索式")).on_click(self.on(Cmd::OpenGuide(false), cx)))
                    .child(ui::primary_button("new-search", t("新建检索式")).on_click(self.on(Cmd::NewSearch, cx))),
            )
            .children(self.contact_hint(cx).map(|hint| hint.px(px(16.)).pt(px(8.))))
            .when(!empty, |d| d.child(auto))
            .child(if empty {
                ui::empty_state("search", t("还没有检索式"), t("检索式是写给 PubMed、Europe PMC、Crossref 或 arXiv 的查询。保存后可以反复运行，把新发表的文献带进文献库。")).into_any_element()
            } else {
                div()
                    .id("searches")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(16.))
                    .pb(px(16.))
                    .gap(px(12.))
                    .children(cards)
                    .when(!batches.is_empty(), |d| d.child(div().flex().flex_col().pt(px(8.)).child(ui::caption(t("导入记录")).pb(px(4.))).children(batches)))
                    .into_any_element()
            })
            .into_any_element()
    }

    /// What a trial run found: the count in the database, and the newest
    /// records with what the library already knows about each.
    fn render_trial(&self, found: Option<&Result<Value, String>>, cx: &mut Context<Self>) -> AnyElement {
        let close = ui::ghost_button("close-trial", Some("x"), None).on_click(self.on(Cmd::CloseTrial, cx));
        let panel = div().flex().flex_col().gap(px(6.)).p(px(10.)).rounded(px(8.)).border_1().border_color(Theme::line()).bg(Theme::bg_body());
        let found = match found {
            None => return panel.child(div().flex().items_center().child(div().flex_1().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(t("正在检索…"))).child(close)).into_any_element(),
            Some(Err(error)) => return panel.child(div().flex().items_center().child(div().flex_1().child(ui::error_line(&crate::i18n::reasons(error)))).child(close)).into_any_element(),
            Some(Ok(found)) => found,
        };
        let total = found["total"].as_u64().map(|n| tf!("数据库中共命中 {n} 篇。", "{n} results in the database. ")).unwrap_or_default();
        let head = tf!("{total}取回最新的 {} 篇：{} 篇是新的，{} 篇已在文献库中。", "{total}Retrieved the latest {} papers: {} new, {} already in this library.", found["fetched"], found["new"], found["inLibrary"]);
        let rows = found["results"].as_array().into_iter().flatten().map(|record| {
            let text = |key: &str| record[key].as_str().unwrap_or("").to_string();
            let mut about: Vec<String> = vec![crate::i18n::short_names(&text("authors")), record["year"].as_i64().map(|y| y.to_string()).unwrap_or_default(), text("journal")];
            about.retain(|part| !part.is_empty());
            // Already in the library: say so, and what was decided about it.
            let known = record["inLibrary"].as_object().map(|held| if held["inTrash"] == true { t("在回收站").to_string() } else { tf!("已在库中 · {}", "In library · {}", label(&SCREENINGS, held["screening"].as_str().unwrap_or(""))) });
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .py(px(5.))
                .border_t_1()
                .border_color(Theme::line())
                .child(div().flex().flex_col().flex_1().min_w_0().child(div().truncate().text_size(text_px(13.)).text_color(if known.is_some() { Theme::text_tertiary() } else { Theme::text() }).child(text("title"))).child(div().truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(about.join(" · "))))
                .children(known.map(|known| ui::chip(known, Theme::text_secondary(), Theme::bg_sunken())))
        });
        panel.child(div().flex().items_center().child(div().flex_1().text_size(text_px(12.)).text_color(Theme::text_secondary()).child(head)).child(close)).children(rows).into_any_element()
    }
}

impl Workspace {
    /// 运行并导入: run a saved search on a connection of its own, its card saying how far it is (`SearchProgress`).
    /// It goes on when the library is left (`works.rs`): what it ends is handled through `in_library`, and said by
    /// it when nobody is looking.
    pub(crate) fn run_saved_search(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.searching.going.contains_key(&id) { return }
        // A first run after none was going starts the count again.
        if lib.searching.going.is_empty() { (lib.searching.done, lib.searching.total, lib.searching.error) = (0, 0, String::new()) }
        let progress = Arc::new(SearchProgress::default());
        lib.searching.going.insert(id.clone(), progress.clone());
        let (info, args) = (lib.info.clone(), serde_json::json!({"id": id}));
        let library = info.id.clone();
        let heard = progress.clone();
        let work = cx.background_executor().spawn(async move {
            let report = move |done: usize, of: usize| {
                heard.done.store(done, Ordering::Relaxed);
                heard.of.store(of, Ordering::Relaxed);
            };
            sources::reporting(report, || {
                let mut store = Store::open(&info.dir).map_err(|e| format!("{e:#}"))?;
                tools::call_in_with(&mut store, &info, "run_search", &args, &wisp_net::get_text, &wisp_net::download, wisp_net::reach())
            })
        });
        // The card, and the run's entry elsewhere, drawn again while it goes on.
        let ticking = progress.clone();
        cx.spawn(async move |this, cx| {
            while !ticking.over.load(Ordering::Relaxed) {
                cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() { break }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = work.await;
            progress.over.store(true, Ordering::Relaxed);
            this.update(cx, |this, cx| {
                this.in_library(&library, cx, |this, cx| this.search_ran(&id, result, cx));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A run of a saved search is over: the library read again (it wrote through its own connection, a failed run
    /// too), and what it brought said, unless the library is off screen, where `in_library` says how it ended.
    fn search_ran(&mut self, id: &str, result: Result<Value, String>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        if let Some(run) = lib.searching.going.remove(id) {
            let (done, of) = (run.done.load(Ordering::Relaxed), run.of.load(Ordering::Relaxed));
            let stopped = match &result {
                Ok(run) => run["stopped"].as_str().map(str::to_string),
                Err(err) => Some(err.clone()),
            };
            // One that failed before it had anything counts what it was to bring as not brought.
            lib.searching.done += done;
            lib.searching.total += if stopped.is_some() { of.max(done + 1) } else { of.max(done) };
            if let Some(why) = stopped { lib.searching.error = why }
        }
        self.change(cx, |_| Ok(()));
        if self.away { return }
        match result {
            Ok(run) if run["stopped"].is_string() => self.fail(tf!("{}：新增 {} 篇。{}", "{}: {} added. {}", run["search"].as_str().unwrap_or(t("检索")), run["added"], run["stopped"].as_str().unwrap_or_default()), cx),
            Ok(run) => self.say(tf!("{}：新增 {} 篇，跳过已有的 {} 篇", "{}: {} added, {} existing skipped", run["search"].as_str().unwrap_or(t("检索")), run["added"], run["skipped"]), cx),
            Err(err) => self.fail(err, cx),
        }
    }

    /// 取回全部 on a card: the search set to take every hit its database gives, and run.
    pub(crate) fn take_all_hits(&mut self, id: String, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut search) = self.lib.as_ref().and_then(|lib| lib.searches.iter().find(|s| s.id == id).cloned()) else { return };
        search.max_results = sources::ceiling(&search.kind);
        if !self.change(cx, |lib| lib.store.save_search(&mut search)) { return }
        self.run_saved_search(id, cx);
    }

    /// While the app runs, look once a minute for saved searches whose turn
    /// has come, in every library that updates on its own, and run them.
    /// What was missed while the app was closed is due at the first look.
    pub(crate) fn watch_searches(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(20)).await;
            loop {
                let Ok(root) = this.update(cx, |this, _| this.root.clone()) else { break };
                // One look at a time: the next waits for this one's runs to end.
                let outcome = cx.background_executor().spawn(async move { tools::call(&root, "sync_searches", &serde_json::json!({}), &wisp_net::get_text, &wisp_net::download) }).await;
                let alive = this.update(cx, |this, cx| {
                    let Ok(outcome) = outcome else { return };
                    let runs = outcome["runs"].as_array().cloned().unwrap_or_default();
                    if runs.is_empty() { return }
                    this.libraries = libraries::list(&this.root);
                    // The open library shows what came in; the others show it when opened.
                    let Some(open) = this.lib.as_ref().map(|l| l.info.id.clone()) else { return cx.notify() };
                    let mine: Vec<&Value> = runs.iter().filter(|run| run["libraryId"] == open.as_str()).collect();
                    if mine.is_empty() { return }
                    this.change(cx, |_| Ok(()));
                    let added: u64 = mine.iter().filter_map(|run| run["added"].as_u64()).sum();
                    if added > 0 { this.say(tf!("自动更新：新增 {added} 篇待判断的文献", "Automatic update: {added} new papers awaiting screening"), cx) }
                });
                if alive.is_err() { break }
                cx.background_executor().timer(std::time::Duration::from_secs(60)).await;
            }
        })
        .detach();
    }
}
