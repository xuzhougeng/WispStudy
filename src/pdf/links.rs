//! Links in a PDF (`wisp_core::pdflink`): a click never opens an address by itself. The pointer's hand and a
//! line under a link say it can be clicked, and hovering it says where it leads; a click shows a card by the
//! link with the site it truly leads to, the whole address, what about it may mislead and that it comes from
//! the file, and the browser is opened only from the card. A link to anything but a web page or a mail (a file
//! on this computer, a program, a script) is not opened from here at all: the card says so, and it can be copied.

use super::*;

/// The card of a link clicked, by the link: where it leads and what to know of it.
#[derive(Clone)]
pub(super) struct LinkCard {
    /// The 0-based page of the link, and the box of all its lines in that page's points.
    page: usize,
    anchor: [f32; 4],
    verdict: Verdict,
}

/// How wide the card is, and how tall it is at most.
const LINK_W: f32 = 400.;
const LINK_TALL: f32 = 320.;

/// About how many lines a text of the card's body takes in `room` pixels: a Chinese character is as wide as two
/// letters. The card's height is guessed from it, so it hangs under the link wherever it fits there.
fn lines_in(text: &str, room: f32) -> f32 {
    let wide: f32 = text.chars().map(|c| if wisp_core::structure::is_cjk(c) { 12. } else { 6.8 }).sum::<f32>() * f32::from(text_px(1.));
    (wide / room.max(1.)).ceil().max(1.)
}

/// What a reader is told of a warning, and whether it is grave: one that says the link may not lead where it
/// seems to (in the danger colour), else a note of how it goes there.
fn warning_text(warning: &Warning, site: &str) -> (String, bool) {
    match warning {
        Warning::Disguised(named) => (tf!("页面上写的是 {named}，链接实际去往 {site}。", "The page says {named}, but the link goes to {site}."), true),
        Warning::Credentials => (tf!("网址在 @ 前加了一段名字，真正去往的是 {site}。", "The address puts a name before an @; it really goes to {site}."), true),
        Warning::Lookalike(own) => (tf!("网站名里有非拉丁字母（读作 {own}），可能在冒充长得像的网站。", "The site's name has non-Latin letters (it reads {own}) and may imitate a site that looks alike."), true),
        Warning::Local => (t("这个链接指向这台电脑或局域网里的设备。").to_string(), true),
        Warning::Numeric => (t("网站用一串数字（IP 地址）代替名字。").to_string(), false),
        Warning::Plain => (t("网址以 http 开头，不加密，途中可能被看到或改动。").to_string(), false),
    }
}

impl PdfView {
    /// The links of a 0-based page, each with what is told of it; none before its glyphs are read.
    fn links_on(&self, page: usize) -> &[(Link, Verdict)] {
        self.links.get(&page).map(|links| links.as_slice()).unwrap_or_default()
    }

    /// Unseen boxes over the links of a page in a view: the pointer's hand and a line of the accent under the one
    /// hovered say it can be clicked, as over a figure's number; hovering says where it leads. Nothing is drawn
    /// on them otherwise: the page shows its links as it sets them.
    pub(super) fn link_boxes(&self, pane: usize, page: usize, scale: f32) -> Vec<Stateful<Div>> {
        // Not over the bar of a selection just made across a link, nor while it is made.
        let tips = self.selection.is_none() && !self.selecting;
        let mut boxes = vec![];
        for (n, (link, verdict)) in self.links_on(page).iter().enumerate() {
            for (line, b) in link.boxes.iter().enumerate() {
                // Unique in the view: the same page may be drawn in both halves.
                let id = ((pane * 10_000 + page) * 1_000 + n.min(999)) * 64 + line.min(63);
                boxes.push(
                    div()
                        .id(("pdf-link", id))
                        .absolute()
                        .left(px(b[0] * scale))
                        .top(px(b[1] * scale))
                        .w(px((b[2] - b[0]) * scale))
                        .h(px((b[3] - b[1]) * scale + 1.))
                        .cursor_pointer()
                        .border_b_1()
                        .border_color(gpui::transparent_black())
                        .hover(|s| s.border_color(Theme::accent()))
                        .when(tips, |d| d.tooltip(ui::tip(verdict.url.clone()))),
                );
            }
        }
        boxes
    }

    /// The link under a point of a 0-based page, as the card a click on it shows.
    pub(super) fn link_at(&self, page: usize, x: f32, y: f32) -> Option<LinkCard> {
        let (link, verdict) = self.links_on(page).iter().find(|(link, _)| link.boxes.iter().any(|b| x >= b[0] - 1. && x <= b[2] + 1. && y >= b[1] - 1. && y <= b[3] + 1.))?;
        let anchor = link.boxes.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]);
        Some(LinkCard { page, anchor, verdict: verdict.clone() })
    }

    /// The card of the n-th link (from 0) of a 0-based page, as a click on it shows (the debug step `pdflink`);
    /// false when the page has no such link, or its glyphs are not read yet.
    pub(crate) fn open_link_card(&mut self, page: usize, n: usize, cx: &mut Context<Self>) -> bool {
        let Some([x0, y0, ..]) = self.links_on(page).get(n).and_then(|(link, _)| link.boxes.first().copied()) else { return false };
        self.link_card = self.link_at(page, x0 + 0.5, y0 + 0.5);
        cx.notify();
        self.link_card.is_some()
    }

    /// The address of the card goes to the browser (or the mail program), and the card away. Never one that is
    /// not a web page or a mail.
    pub(crate) fn open_link(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self.link_card.take() else { return };
        if card.verdict.opens() { cx.open_url(&card.verdict.url) }
        cx.notify();
    }

    /// The address of the card copied, as it would be opened, and the card away.
    pub(crate) fn copy_link(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self.link_card.take() else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(card.verdict.url));
        cx.emit(PdfEvent::Copied);
        cx.notify();
    }

    /// The card of the link clicked: the site in its header, the whole address under it, what may mislead in
    /// it, that it comes from the file; at its foot 复制链接, and the way to open it apart at the other end.
    pub(super) fn link_card(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.selecting { return None }
        let card = self.link_card.clone()?;
        let (x, y) = self.sheet_place(card.page)?;
        let at = self.scale_of(card.page);
        let anchor = [x + card.anchor[0] * at, y + card.anchor[1] * at, x + card.anchor[2] * at, y + card.anchor[3] * at];
        let verdict = &card.verdict;
        let close = div().id("pdf-link-close").flex().flex_none().items_center().justify_center().size(px(24.)).rounded(px(6.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).child(ui::icon("x", 13., Theme::text_tertiary())).on_click(cx.listener(|this, _, _, cx| {
            this.link_card = None;
            cx.notify();
        }));
        let (icon, title) = match verdict.goes {
            Goes::Web => ("globe", verdict.site.clone()),
            Goes::Mail => ("link", verdict.site.clone()),
            Goes::Other if verdict.site.is_empty() => ("link", t("不是网址").to_string()),
            Goes::Other => ("link", verdict.site.clone()),
        };
        let header = div().flex().flex_none().items_center().gap(px(8.)).h(text_px(34.)).pl(px(12.)).pr(px(4.)).border_b_1().border_color(Theme::line())
            .child(ui::icon(icon, 14., Theme::text_tertiary()))
            .child(div().flex_1().min_w_0().truncate().text_color(Theme::text()).font_weight(FontWeight::MEDIUM).child(ui::one_line(&title)))
            .child(close);
        // What is to be known first: why it is not opened from here, else what may mislead, the gravest first.
        let mut said: Vec<(String, bool)> = vec![];
        if !verdict.opens() {
            said.push((if verdict.site.is_empty() { t("这个链接没有写明要去的网址，不从这里打开。").to_string() } else { tf!("这个链接要打开的不是网页（{}），为了安全不从这里打开；需要时可以复制下来。", "This link opens something other than a web page ({}); for safety it is not opened from here. You can copy it.", verdict.site) }, true));
        }
        said.extend(verdict.warnings.iter().map(|warning| warning_text(warning, &verdict.site)));
        let grave = said.iter().any(|(_, grave)| *grave);
        let note = t("链接来自这份 PDF，不一定可信。看清要去的网站再打开。");
        // How tall it is: the header and the foot, the address (five lines at most), what is said, the note.
        let width = LINK_W.min(self.width - 2. * CARD_EDGE).max(0.);
        let (room, line) = (width - 26., f32::from(text_px(18.)));
        let said_tall: f32 = said.iter().map(|(text, _)| lines_in(text, room - 19.) * line + 8.).sum();
        let tall = f32::from(text_px(34.)) + f32::from(text_px(38.)) + 22. + lines_in(&verdict.url, room).min(5.) * line + said_tall + 8. + lines_in(note, room) * line;
        let lines: Vec<Div> = said.into_iter().map(|(text, grave)| {
            let color = if grave { Theme::danger() } else { Theme::text_secondary() };
            div().flex().items_start().gap(px(6.)).child(div().flex_none().pt(px(2.)).child(ui::icon(if grave { "triangle-alert" } else { "info" }, 13., color))).child(div().flex_1().min_w_0().text_color(color).child(text))
        }).collect();
        let body = div().id("pdf-link-body").flex().flex_col().flex_shrink().min_h_0().overflow_y_scroll().gap(px(8.)).px(px(12.)).py(px(10.)).text_size(text_px(12.)).line_height(text_px(18.))
            // The whole address, as it would be opened: what a site's name in the header leaves out is read here.
            .child(div().text_color(Theme::text_secondary()).line_clamp(5).text_ellipsis().child(verdict.url.clone()))
            .children(lines)
            .child(div().text_color(Theme::text_tertiary()).child(note));
        let button = |id: &'static str, label: &str| div().id(id).flex().flex_none().items_center().h(text_px(26.)).px(px(9.)).rounded(px(6.)).cursor_pointer().hover(|s| s.bg(Theme::hover())).child(label.to_string());
        let copy = button("pdf-link-copy", t("复制链接")).text_color(Theme::text_secondary()).on_click(cx.listener(|this, _, _, cx| this.copy_link(cx)));
        // Opening is at the other end from copying, and says it goes ahead in spite of what was said.
        let open = verdict.opens().then(|| {
            let label = match (verdict.goes, grave) {
                (Goes::Mail, _) => t("写邮件"),
                (_, true) => t("仍要打开"),
                _ => t("在浏览器中打开"),
            };
            button("pdf-link-open", label).text_color(if grave { Theme::danger() } else { Theme::accent() }).on_click(cx.listener(|this, _, _, cx| this.open_link(cx)))
        });
        let footer = div().flex().flex_none().items_center().gap(px(2.)).h(text_px(38.)).px(px(6.)).border_t_1().border_color(Theme::line()).child(copy).child(div().flex_1()).children(open);
        let place = card_place(anchor, width, tall.min(LINK_TALL), (self.width, self.height));
        Some(
            div()
                .id("pdf-link-card")
                .absolute()
                .left(px(place.left))
                .when_some(place.top, |d, top| d.top(px(top)))
                .when_some(place.bottom, |d, bottom| d.bottom(px(bottom)))
                .w(px(place.width))
                .max_h(px(place.max_height))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(10.))
                .bg(Theme::bg_body())
                .border_1()
                .border_color(Theme::line_strong())
                .shadow(crate::theme::shadow_card())
                .font(crate::theme::ui_font())
                .text_size(text_px(13.))
                .line_height(text_px(20.))
                .occlude()
                // A press on the card is not a press on the page.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(header)
                .child(body)
                .child(footer)
                .into_any_element(),
        )
    }
}
