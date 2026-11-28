//! Wisp Study: a local-first literature library. Collect papers, organise
//! them with tags and custom fields, preview PDFs and ask questions over a
//! library. The engine is `wisp_core` (crates/core, no GPUI); this crate is
//! the GPUI application.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

#[macro_use]
mod i18n;
#[macro_use]
mod app;
mod assets;
mod chat_run;
mod chat_state;
mod commands;
mod fetch;
mod importing;
mod listening;
mod menus;
mod modal;
mod paper_files;
mod screen_run;
mod sorting;
mod viewers;
#[cfg(debug_assertions)]
mod dev;
mod background;
mod bell;
mod chat;
mod cite;
mod cite_styles;
mod comments;
mod compare;
mod context;
mod custom;
mod detail;
mod diagram;
mod extract;
mod facing;
mod figures;
mod filing;
mod floating;
mod glossary;
mod dialogs;
mod draft;
mod excerpts;
mod guide;
mod home;
mod input;
mod library;
mod library_find;
mod manuscripts;
mod write_figures;
mod markdown;
mod mascot;
mod mention;
mod mindmap;
mod notes;
mod open_file;
mod palette;
mod pdf;
mod picture;
mod player;
mod read_aloud;
mod reader;
mod revise;
mod screen;
mod screen_confirm;
mod screen_record;
mod screen_single;
mod searches;
mod scrollbar;
mod select;
mod settings;
mod shortcuts;
mod theme;
mod tour;
mod tutorial;
mod collected;
mod written;
mod tagging;
mod transfer;
mod translate;
mod ui;
mod updater;
mod voice;
mod works;
mod write;
mod write_find;
mod write_seek;
mod write_read;
mod write_agent;
mod write_assist;
mod write_cite;
mod write_facing;
mod write_translate;
mod write_source;

use gpui::*;

pub(crate) const APP_ID: &str = "com.wisp.study";

fn main() {
    #[cfg(target_os = "windows")]
    if background::hand_over() { return }
    let app = Application::new().with_assets(assets::Assets);
    background::on_reopen(&app);
    // macOS hands a file over (打开方式 › Wisp Study, a PDF dropped on the Dock icon) as an address, maybe
    // before the window is there: it waits in the channel until the workspace listens (`open_file.rs`).
    let (handed, inbox) = async_channel::unbounded::<Vec<std::path::PathBuf>>();
    app.on_open_urls(move |urls| {
        let paths: Vec<_> = urls.iter().filter_map(|url| open_file::file_url_path(url)).collect();
        if !paths.is_empty() { handed.try_send(paths).ok(); }
    });
    app.run(|cx: &mut App| {
        background::install(cx);
        let bounds = Bounds::centered(None, size(px(1360.), px(860.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some(APP_ID.into()),
                titlebar: Some(TitlebarOptions { title: Some("Wisp Study".into()), appears_transparent: ui::custom_titlebar(), traffic_light_position: Some(point(px(14.), px(11.))) }),
                window_min_size: Some(size(px(900.), px(560.))),
                focus: wisp_core::settings::env("DEV").is_none(),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| app::Workspace::new(window, cx)),
        )
        .expect("open window")
        .update(cx, |workspace, window, cx| {
            background::watch(cx.entity(), window, cx);
            workspace.start_session(window, cx);
            // After the session is back: a file handed over at the start is what is read.
            workspace.listen_for_files(inbox, window, cx);
            #[cfg(debug_assertions)]
            workspace.dev_start(window, cx);
        })
        .ok();
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() { cx.quit() }
        })
        .detach();
        // A scripted debug start must not take the keyboard from whatever the user is doing.
        if wisp_core::settings::env("DEV").is_none() { cx.activate(true) }
    });
}
