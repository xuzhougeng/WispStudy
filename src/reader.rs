//! A paper read in a window of its own (#194): the PDF apart from the library's window, which stays free for
//! the list, another paper's abstract or the conversation. The window holds the viewer and a strip over it;
//! everything the viewer does (marks, quotes, translations of a selection) goes to the library's window, which
//! made the viewer and keeps it (`Library::windows`), as it does for the PDF in its own panel.

use crate::app::{Cmd, Workspace};
use crate::i18n::t;
use crate::pdf::{PdfView, Zoom, ZoomTo};
use crate::shortcuts::{ZoomFit, ZoomIn, ZoomOut};
use crate::theme::{self, Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};

pub struct Reader {
    workspace: WeakEntity<Workspace>,
    /// The paper read, its id.
    paper: String,
    title: SharedString,
    view: Entity<PdfView>,
    focus: FocusHandle,
}

impl Reader {
    pub fn new(workspace: WeakEntity<Workspace>, paper: String, title: SharedString, view: Entity<PdfView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Closed by the person: the library's window lets the viewer go and keeps where it was read.
        let (closing, which) = (workspace.clone(), paper.clone());
        window.on_window_should_close(cx, move |_, cx| {
            closing.update(cx, |workspace, cx| workspace.paper_window_closed(&which, cx)).ok();
            true
        });
        let focus = cx.focus_handle();
        window.focus(&focus);
        Reader { workspace, paper, title, view, focus }
    }

    fn zoom(&mut self, to: ZoomTo, cx: &mut Context<Self>) {
        self.view.update(cx, |pdf, cx| pdf.zoom_to(to, cx));
        cx.notify();
    }
}

impl Render for Reader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = f32::from(window.viewport_size().width);
        self.view.update(cx, |pdf, _| pdf.set_width(width));
        let (page, count, percent, zoom) = { let view = self.view.read(cx); (view.current_page(), view.page_count(), view.percent(), view.zoom()) };
        let tool = |id: &'static str, icon: &'static str| ui::ghost_button(id, Some(icon), None).h(text_px(24.)).min_w(px(24.)).occlude();
        let on = |to: ZoomTo| cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| this.zoom(to, cx));
        let paper = self.paper.clone();
        let workspace = self.workspace.clone();
        // Over the pages, a strip that is the window's title bar: the paper, where it is read, its size, and the way back to the library.
        let strip = ui::title_strip()
            .gap(px(8.))
            .pl(px(12. + ui::traffic_lights_width()))
            .pr(px(if cfg!(target_os = "windows") { 3. * ui::CONTROL_W + 8. } else { 12. }))
            .border_b_1()
            .border_color(Theme::line())
            .text_size(text_px(12.))
            .text_color(Theme::text_tertiary())
            .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::MEDIUM).text_color(Theme::text()).child(ui::one_line(&self.title)))
            .when(count > 0, |d| {
                d.child(div().flex_none().child(tf!("{page} / {count} 页", "{page} / {count} pages")))
                    .child(tool("reader-zoom-out", "minus").on_click(on(ZoomTo::Out)))
                    .child(ui::ghost_button("reader-zoom", None, Some(format!("{}%", percent.round()).into())).h(text_px(24.)).px(px(6.)).text_size(text_px(12.)).occlude().tooltip(ui::tip(t("适应宽度"))).on_click(on(ZoomTo::Set(Zoom::FitWidth))))
                    .child(tool("reader-zoom-in", "plus").on_click(on(ZoomTo::In)))
                    .child(ui::ghost_button("reader-zoom-page", None, None).h(text_px(24.)).min_w(px(24.)).occlude().child(ui::icon("scan", 16., if zoom == Zoom::FitPage { Theme::accent() } else { Theme::text_secondary() })).tooltip(ui::tip(t("适应整页"))).on_click(on(ZoomTo::Set(Zoom::FitPage))))
            })
            // Back to the library's window, with this paper open there.
            .child(ui::ghost_button("reader-library", Some("library"), Some(t("文献库").into())).h(text_px(24.)).px(px(6.)).text_size(text_px(12.)).occlude().tooltip(ui::tip(t("在文献库的窗口里打开这篇"))).on_click(move |_, _, cx| {
                workspace.update(cx, |workspace, cx| workspace.show_in_library(&paper, cx)).ok();
            }));
        div()
            .id("reader")
            .key_context("Reader")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(ZoomTo::In, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(ZoomTo::Out, cx)))
            .on_action(cx.listener(|this, _: &ZoomFit, _, cx| this.zoom(ZoomTo::Set(Zoom::FitWidth), cx)))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(Theme::bg_body())
            .font(theme::ui_font())
            .text_size(text_px(13.))
            .line_height(text_px(20.))
            .text_color(Theme::text())
            .child(strip)
            .child(div().flex_1().min_h_0().child(self.view.clone()))
            .children(ui::window_controls(window))
    }
}

impl Workspace {
    /// Read a paper in a window of its own (#194). Of the paper on show, the file being read at the place it is read
    /// at, and its panel closes: the library's window is free again. A paper already in a window brings that window up.
    pub(crate) fn open_paper_window(&mut self, paper: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        if let Some(open) = lib.windows.iter().find(|w| w.paper == paper) {
            open.window.update(cx, |_, window, _| window.activate_window()).ok();
            return;
        }
        let on_show = lib.selected.as_deref() == Some(paper.as_str());
        let shown = lib.pdf.as_ref().filter(|_| on_show).map(|view| { let view = view.read(cx); (view.resource_id().to_string(), view.place()) });
        if let Some((file, place)) = &shown { lib.file_places.insert(file.clone(), *place); }
        let file = match &shown {
            Some((id, _)) => lib.files.iter().find(|f| &f.id == id).cloned(),
            None => lib.pdf_file(&paper).cloned(),
        };
        let Some(file) = file else { return self.fail(t("这篇文献还没有 PDF，没有可以在窗口里读的页面。"), cx) };
        let title: SharedString = lib.paper(&paper).map_or(String::new(), |p| p.title.clone()).into();
        let Some((view, events)) = self.pdf_view(&file, cx) else { return };
        let workspace = cx.entity().downgrade();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(900.), px(1000.)), cx))),
            app_id: Some(crate::APP_ID.into()),
            titlebar: Some(TitlebarOptions { title: Some(title.clone()), appears_transparent: ui::custom_titlebar(), traffic_light_position: Some(point(px(14.), px(11.))) }),
            window_min_size: Some(size(px(480.), px(360.))),
            // As the library's window: a scripted debug start takes no keyboard.
            focus: wisp_core::settings::env("DEV").is_none(),
            ..Default::default()
        };
        let (shown_view, which) = (view.clone(), paper.clone());
        let opened = cx.open_window(options, move |window, cx| cx.new(|cx| Reader::new(workspace, which, title, shown_view, window, cx)));
        let handle = match opened {
            Ok(handle) => handle,
            Err(error) => return self.fail(tf!("无法打开新窗口：{error:#}", "Cannot open a new window: {error:#}"), cx),
        };
        let Some(lib) = self.lib.as_mut() else { return };
        lib.windows.push(crate::app::PaperWindow { paper: paper.clone(), view, _events: events, window: handle });
        // The paper is read there now: the library's window goes back to its list.
        if on_show {
            lib.reading = false;
            self.select(None, window, cx);
        }
        cx.notify();
    }

    /// The person closed a paper's window: the viewer goes, and where it was read is kept.
    pub(crate) fn paper_window_closed(&mut self, paper: &str, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(at) = lib.windows.iter().position(|w| w.paper == paper) else { return };
        let closed = lib.windows.remove(at);
        let view = closed.view.read(cx);
        let place = (view.resource_id().to_string(), view.place());
        lib.file_places.insert(place.0.clone(), place.1);
        let _ = lib.store.save_reading_places(&[place]);
        cx.notify();
    }

    /// The paper of a window shown in the library's window: selected, its PDF, the window brought to the front.
    fn show_in_library(&mut self, paper: &str, cx: &mut Context<Self>) {
        let (paper, main, workspace) = (paper.to_string(), self.main_window, cx.entity());
        cx.defer(move |cx| {
            main.update(cx, |_, window, cx| {
                workspace.update(cx, |this, cx| this.run(Cmd::OpenPaper(paper, crate::app::Tab::Pdf), window, cx));
                window.activate_window();
            })
            .ok();
        });
    }
}
