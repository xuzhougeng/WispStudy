//! The player of 朗读 and 播客: one card for what is heard, at the foot of its PDF while that is on show,
//! and at the window's foot while a podcast plays on elsewhere (`Workspace::podcast_bar`). It pauses and
//! goes on, plays at another speed and stops; a podcast's card has a bar to play it from anywhere made so
//! far, and its ⋯ makes it anew or saves it. The positions and the speeds are `wisp_core::playback`.

use crate::app::{Cmd, MenuKind, Workspace};
use crate::i18n::t;
use crate::read_aloud::{Kind, Status};
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use wisp_core::playback;

/// The card's width at most; narrower in a narrower area.
const PLAYER_W: f32 = 460.;

impl Workspace {
    /// The player on the pages of the file `file` (`PaperFile::id`) in an area `width` wide, while what is
    /// heard is of it (`Library::playing_here`).
    pub(crate) fn player_on_page(&self, file: &str, width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let lib = self.lib.as_ref()?;
        let kind = lib.playing_here(file)?;
        let status = lib.aloud_shown.clone()?;
        let paper = lib.files.iter().find(|f| f.id == file).map(|f| f.paper_id.clone()).unwrap_or_default();
        let card = self.player_card(kind, &paper, None, status, (width - 32.).clamp(0., PLAYER_W), cx);
        Some(div().absolute().bottom(px(16.)).left_0().w_full().flex().justify_center().child(card).into_any_element())
    }

    /// The podcast heard on after its paper was left: the same card at the window's foot, with the paper's
    /// title, which goes back to its PDF.
    pub(crate) fn podcast_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let lib = self.lib.as_ref()?;
        let (paper, resource, status) = lib.podcast_away(cx)?;
        let title = ui::one_line(if paper.title.trim().is_empty() { t("未命名文献") } else { paper.title.trim() });
        let card = self.player_card(Kind::Podcast, &paper.id, Some((title, resource)), status, PLAYER_W, cx);
        Some(div().absolute().bottom(px(44.)).left_0().w_full().flex().justify_center().child(card).into_any_element())
    }

    /// The card: pause or go on, what is heard, the speed, ⋯ and the playlist for a podcast, stop; under
    /// them a podcast's bar. `away` is the title of the paper heard and its file, when its PDF is not on show.
    fn player_card(&self, kind: Kind, paper: &str, away: Option<(String, String)>, status: Status, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let Some(lib) = self.lib.as_ref() else { return div().into_any_element() };
        let podcast = kind == Kind::Podcast;
        let where_ = match (podcast, status.mark) {
            (true, None) => t("正在写播客稿…").to_string(),
            (true, Some(turn)) => tf!("第 {} 轮", "Turn {}", turn),
            (false, Some(page)) => tf!("第 {} 页", "Page {}", page),
            (false, None) => t("选中的文字").to_string(),
        };
        // Who says what is heard now; 朗读 marks its sentence on the page instead.
        let said = status.heard.as_ref().filter(|_| podcast).map(|heard| {
            let host = wisp_core::podcast::HOSTS.iter().find(|(_, _, voice)| *voice == heard.voice).map_or("", |(_, name, _)| t(name));
            ui::one_line(&tf!("{host}：{}", "{host}: {}", heard.sentence))
        });
        let small = |id: &'static str, icon: &'static str, tip: String| ui::ghost_button(id, None, None).size(px(28.)).child(ui::icon(icon, 15., Theme::text_secondary())).tooltip(ui::tip(tip));
        // The same place pauses and goes on; a spinner there while the next sound is on its way.
        let pause: AnyElement = if status.waiting && !status.paused { ui::spinner("player-wait", 16., Theme::accent()).into_any_element() } else { ui::icon(if status.paused { "play" } else { "pause" }, 16., Theme::accent()).into_any_element() };
        let pause = ui::ghost_button("player-pause", None, None).size(px(32.)).rounded(px(16.)).child(pause).tooltip(ui::tip(if status.paused { t("继续") } else { t("暂停") })).on_click(self.on(Cmd::PauseReading, cx));
        let what = if podcast { t("播客") } else { t("朗读") };
        let heading = match &away {
            Some((title, _)) => div().flex().items_center().gap(px(6.)).text_size(text_px(13.)).text_color(Theme::text()).child(div().flex_1().min_w_0().truncate().child(title.clone())).child(div().flex_none().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(where_)),
            None => div().flex().items_center().gap(px(6.)).text_size(text_px(13.)).text_color(Theme::text()).child(div().flex_none().child(what)).child(div().flex_none().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(where_)),
        };
        let info = div().id("player-info").flex().flex_col().flex_1().min_w_0().px(px(4.)).py(px(2.)).rounded(px(6.)).child(heading).when_some(said, |d, said| d.child(div().w_full().truncate().text_size(text_px(12.)).text_color(Theme::text_secondary()).child(said)));
        // Away from its PDF the title is the way back to it.
        let info = match &away {
            Some((_, resource)) => info.cursor_pointer().hover(|s| s.bg(Theme::hover())).tooltip(ui::tip(t("回到这篇文献的 PDF"))).on_click(self.on(Cmd::ReadFile(resource.clone()), cx)),
            None => info,
        };
        let speed = playback::speed_label(playback::speed(self.settings.play_speed));
        let speed = ui::ghost_button("player-speed", None, Some(speed.into())).h(px(28.)).min_w(px(40.)).px(px(6.)).text_size(text_px(12.)).text_color(Theme::text_secondary()).tooltip(ui::tip(t("播放速度"))).on_click(self.on_menu(MenuKind::PlaySpeed, cx));
        let listed = podcast && !crate::app::podcast_list(lib).is_empty();
        let stop_tip = if podcast && !status.made { t("停止：还没生成完的播客不会存下") } else if podcast { t("停止：下次点「播客」从这里接着放") } else { t("停止朗读") };
        let controls = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(pause)
            .child(info)
            .child(speed)
            .when(podcast, |d| d.child(small("player-more", "ellipsis", t("更多").to_string()).on_click(self.on_menu(MenuKind::PodcastMore(paper.to_string()), cx))))
            .when(listed, |d| d.child(small("player-list", "list", t("播放列表").to_string()).on_click(self.on_menu(MenuKind::Podcasts, cx))))
            .child(ui::ghost_button("player-stop", None, None).size(px(28.)).child(ui::icon("square", 12., Theme::text_secondary())).tooltip(ui::tip(stop_tip)).on_click(self.on(Cmd::StopReading, cx)));
        let seekable = podcast && lib.aloud.as_ref().is_some_and(|r| r.seekable()) && status.length > 0;
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .w(px(width))
            .p(px(4.))
            .rounded(px(12.))
            .bg(Theme::bg_body())
            .border_1()
            .border_color(Theme::line())
            .shadow(crate::theme::shadow_panel())
            .occlude()
            // A press on the card is not a press on the page under it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(controls)
            .when(seekable, |d| d.child(self.seek_row(&status, cx)))
            .into_any_element()
    }

    /// A podcast's bar: where it is, what was made so far, and the length. While it is still being made the
    /// bar spans what was made, which is all one can go to, and the length says it is still growing.
    fn seek_row(&self, status: &Status, cx: &mut Context<Self>) -> AnyElement {
        let Some(lib) = self.lib.as_ref() else { return div().into_any_element() };
        // Held, the bar follows the pointer and the place is sought on release.
        let at = match lib.seek_held {
            Some(x) => playback::seek_to(x, lib.seek_bar.0, lib.seek_bar.1, status.length),
            None => status.played,
        };
        let share = playback::fraction(at, status.length);
        let entity = cx.entity();
        let measure = canvas(|_, _, _| (), move |bounds, _, window, cx| {
            let bar = (f32::from(bounds.origin.x), f32::from(bounds.size.width));
            window.defer(cx, move |_, cx| entity.update(cx, |this, _| if let Some(lib) = this.lib.as_mut() { lib.seek_bar = bar }));
        })
        .absolute()
        .size_full();
        let clock = |samples: usize| div().flex_none().min_w(px(38.)).text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(playback::clock(samples));
        let knob = 10.;
        let track = div()
            .id("player-seek")
            .relative()
            .flex_1()
            .h(px(16.))
            .cursor_pointer()
            .child(measure)
            .child(div().absolute().left_0().right_0().top(px(6.)).h(px(4.)).rounded(px(2.)).bg(Theme::line_strong()))
            .child(div().absolute().left_0().top(px(6.)).h(px(4.)).w(relative(share)).rounded(px(2.)).bg(Theme::accent()))
            .child(div().absolute().top(px(3.)).left(relative(share)).ml(px(-knob / 2.)).size(px(knob)).rounded(px(knob / 2.)).bg(Theme::accent()))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.seek_press(f32::from(event.position.x), cx);
            }));
        let length = div().flex().flex_none().min_w(px(52.)).justify_end().items_center().gap(px(3.)).child(clock(status.length).min_w(px(0.))).when(!status.made, |d| d.child(ui::spinner("player-making", 10., Theme::text_tertiary())));
        let length = if status.made { length.into_any_element() } else { div().id("player-length").child(length).tooltip(ui::tip(t("还在生成：长度是已生成的部分，可以拖到其中任何地方"))).into_any_element() };
        div().flex().items_center().gap(px(8.)).px(px(8.)).pb(px(2.)).child(clock(at)).child(track).child(length).into_any_element()
    }

    /// The bar is pressed at `x`: it is held from here on.
    fn seek_press(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        lib.seek_held = Some(x);
        cx.notify();
    }

    /// While the bar is held, a layer over the whole window hears the pointer wherever it goes; on release the
    /// podcast plays on from there.
    pub(crate) fn render_seek_layer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.lib.as_ref()?.seek_held?;
        Some(
            div()
                .id("seek-layer")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .cursor_pointer()
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    let Some(lib) = this.lib.as_mut() else { return };
                    lib.seek_held = Some(f32::from(event.position.x));
                    cx.notify();
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.seek_release(cx)))
                .into_any_element(),
        )
    }

    fn seek_release(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(x) = lib.seek_held.take() else { return };
        if let Some(reading) = lib.aloud.as_ref() {
            reading.seek(playback::seek_to(x, lib.seek_bar.0, lib.seek_bar.1, reading.status().length));
            lib.aloud_shown = Some(reading.status());
        }
        cx.notify();
    }
}
