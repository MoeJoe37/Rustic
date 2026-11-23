//! The window's trusted UI (ISO-05, TD-173): the address display, the
//! permission prompt and the clipboard rules, drawn and decided by the
//! broker, outside anything a page paints.
//!
//! The shell shows this strip above the page and never lets page pixels
//! into it (`rustic_shell::present`). The strip is a document this module
//! writes itself, every value escaped and limited to printable ASCII (hosts
//! in their ASCII form, so look-alike Unicode never reaches the address),
//! rendered in this process without scripts. The address is the URL the
//! broker committed ([`crate::broker::committed_url`]); no renderer reports
//! it.
//!
//! Permissions follow the browser rules a page cannot bend: only secure
//! contexts with a non-opaque origin may ask, only with transient user
//! activation, one prompt at a time, decisions are kept per origin for the
//! session, three dismissals embargo the origin, a prompt dies with its
//! document, and its buttons ignore clicks for [`INPUT_PROTECTION`] after
//! it appears so a page cannot time a click onto it.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rustic_engine::Engine;
use rustic_net::csp::CspList;
use rustic_style::FetchError;
use rustic_url::{Origin, Url};

use crate::loader::{self, LoaderLimits};
use crate::network::{ResourceKind, Session, SessionDocument};

/// The address row's height in CSS pixels.
pub const ROW: u32 = 28;

/// How long a new prompt ignores clicks on its buttons.
pub const INPUT_PROTECTION: Duration = Duration::from_millis(500);

/// Dismissals after which an origin's requests are denied unprompted.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "page permission APIs reach this once renderers are long-lived (M8, TD-173)"
    )
)]
pub const EMBARGO_DISMISSALS: u8 = 3;

/// Characters a CSS pixel of width holds in the strip's 13px monospace
/// text (about 7.8px per character), rounded down.
const CHARACTER_WIDTH: f64 = 8.0;

/// What the address display says about the connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Security {
    /// A secure context: `https:`, `wss:` or a loopback `http:` host.
    Secure,
    /// Anything else on the network: shown as "Not secure".
    NotSecure,
    /// A local file.
    File,
    /// No host to show (`about:`, `data:`, ...).
    Opaque,
}

/// The address as the strip shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Address {
    /// The connection state.
    pub security: Security,
    /// The scheme and host (ASCII), the part a user must be able to trust.
    pub host: String,
    /// The rest (port, path, query), shown less prominently.
    pub detail: String,
}

/// Printable ASCII only: anything else becomes `?`.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '?'
            }
        })
        .collect()
}

/// The address display for `url`. Credentials and the fragment are never
/// shown; the host is in its ASCII (punycode) form.
pub fn address(url: &Url) -> Address {
    let scheme = url.scheme();
    let Some(host) = url.host().map(rustic_url::Host::serialize) else {
        let security = if scheme == "file" {
            Security::File
        } else {
            Security::Opaque
        };
        let shown = url.serialize_without_fragment();
        let (host, detail) = if scheme == "file" {
            ("file://".to_owned(), url.pathname())
        } else {
            (shown, String::new())
        };
        return Address {
            security,
            host: printable(&host),
            detail: printable(&detail),
        };
    };
    let security = if scheme == "file" {
        Security::File
    } else if rustic_net::mixed_content::is_secure_context(&url.origin(), None) {
        Security::Secure
    } else {
        Security::NotSecure
    };
    let mut detail = String::new();
    if let Some(port) = url.port() {
        let _ = write!(detail, ":{port}");
    }
    detail.push_str(&url.pathname());
    if let Some(query) = url.query() {
        let _ = write!(detail, "?{query}");
    }
    Address {
        security,
        host: printable(&format!("{scheme}://{host}")),
        detail: printable(&detail),
    }
}

/// Fit `address` into `characters`: the host keeps its end (the
/// registrable domain) and loses its start; the detail loses its end.
pub fn elide(address: &Address, characters: usize) -> (String, String) {
    let host_len = address.host.chars().count();
    if host_len > characters {
        let keep = characters.saturating_sub(3);
        let tail: String = address.host.chars().skip(host_len - keep).collect();
        return (format!("...{tail}"), String::new());
    }
    let room = characters - host_len;
    let detail_len = address.detail.chars().count();
    if detail_len <= room {
        return (address.host.clone(), address.detail.clone());
    }
    let keep = room.saturating_sub(3);
    let head: String = address.detail.chars().take(keep).collect();
    let detail = if room >= 3 {
        format!("{head}...")
    } else {
        String::new()
    };
    (address.host.clone(), detail)
}

/// A capability a page must ask the user for.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "page permission APIs reach this once renderers are long-lived (M8, TD-173)"
    )
)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Permission {
    /// Notifications.
    Notifications,
    /// Geolocation.
    Geolocation,
    /// The camera.
    Camera,
    /// The microphone.
    Microphone,
    /// Reading the clipboard.
    ClipboardRead,
}

impl Permission {
    /// The prompt's wording.
    pub fn label(self) -> &'static str {
        match self {
            Self::Notifications => "show notifications",
            Self::Geolocation => "know your location",
            Self::Camera => "use your camera",
            Self::Microphone => "use your microphone",
            Self::ClipboardRead => "see text and images copied to the clipboard",
        }
    }
}

/// The verdict for one request.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "page permission APIs reach this once renderers are long-lived (M8, TD-173)"
    )
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    /// Allowed.
    Granted,
    /// Refused (by the user or by policy).
    Denied,
    /// Waiting for the user's answer in the prompt.
    Prompt,
}

/// The user's answer to a prompt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Answer {
    /// The Allow button.
    Allow,
    /// The Block button.
    Block,
    /// Closed without choosing (Escape, or a navigation).
    Dismiss,
}

/// The prompt on screen.
#[derive(Clone, Debug)]
pub struct Pending {
    origin: Origin,
    permission: Permission,
    shown: Instant,
}

impl Pending {
    /// The asking origin, serialized (ASCII).
    pub fn origin(&self) -> String {
        printable(&self.origin.serialize())
    }

    /// What it asks for.
    pub fn permission(&self) -> Permission {
        self.permission
    }
}

/// The session's permission decisions (never stored on disk).
#[derive(Debug, Default)]
pub struct Permissions {
    decided: HashMap<(String, Permission), bool>,
    dismissed: HashMap<(String, Permission), u8>,
    pending: Option<Pending>,
}

impl Permissions {
    /// The prompt on screen, if any.
    pub fn pending(&self) -> Option<&Pending> {
        self.pending.as_ref()
    }

    /// A page of `origin` asks for `permission`; `activation` is whether
    /// the broker saw transient user activation for it.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "page permission APIs reach this once renderers are long-lived (M8, TD-173)"
        )
    )]
    pub fn request(
        &mut self,
        origin: &Origin,
        permission: Permission,
        activation: bool,
        now: Instant,
    ) -> Decision {
        if origin.is_opaque() || !rustic_net::mixed_content::is_secure_context(origin, None) {
            return Decision::Denied;
        }
        let key = (origin.serialize(), permission);
        if let Some(&granted) = self.decided.get(&key) {
            return if granted {
                Decision::Granted
            } else {
                Decision::Denied
            };
        }
        if self.dismissed.get(&key).copied().unwrap_or(0) >= EMBARGO_DISMISSALS
            || !activation
            || self.pending.is_some()
        {
            return Decision::Denied;
        }
        self.pending = Some(Pending {
            origin: origin.clone(),
            permission,
            shown: now,
        });
        Decision::Prompt
    }

    /// The user answered the prompt. A button press inside
    /// [`INPUT_PROTECTION`] is ignored (`None`, the prompt stays); a
    /// dismissal always closes it.
    pub fn answer(
        &mut self,
        answer: Answer,
        now: Instant,
    ) -> Option<(Origin, Permission, Decision)> {
        let pending = self.pending.as_ref()?;
        if answer != Answer::Dismiss
            && now.saturating_duration_since(pending.shown) < INPUT_PROTECTION
        {
            return None;
        }
        let pending = self.pending.take()?;
        let key = (pending.origin.serialize(), pending.permission);
        let decision = match answer {
            Answer::Allow => {
                self.decided.insert(key, true);
                Decision::Granted
            }
            Answer::Block => {
                self.decided.insert(key, false);
                Decision::Denied
            }
            Answer::Dismiss => {
                let count = self.dismissed.entry(key).or_insert(0);
                *count = count.saturating_add(1);
                Decision::Denied
            }
        };
        Some((pending.origin, pending.permission, decision))
    }

    /// The window committed a new document: its predecessor's prompt
    /// closes unanswered (counted as a dismissal), so no answer can land on
    /// another origin.
    pub fn navigated(&mut self, now: Instant) {
        if self.pending.is_some() {
            let _ = self.answer(Answer::Dismiss, now);
        }
    }

    /// Clipboard writes need transient activation and a focused window;
    /// they never prompt.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "page permission APIs reach this once renderers are long-lived (M8, TD-173)"
        )
    )]
    pub fn clipboard_write(origin: &Origin, activation: bool, focused: bool) -> bool {
        !origin.is_opaque()
            && rustic_net::mixed_content::is_secure_context(origin, None)
            && activation
            && focused
    }

    /// Clipboard reads also need a focused window, then the permission.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "page permission APIs reach this once renderers are long-lived (M8, TD-173)"
        )
    )]
    pub fn clipboard_read(
        &mut self,
        origin: &Origin,
        activation: bool,
        focused: bool,
        now: Instant,
    ) -> Decision {
        if !focused {
            return Decision::Denied;
        }
        self.request(origin, Permission::ClipboardRead, activation, now)
    }
}

/// A rectangle in CSS pixels from the strip's top-left corner: left, top,
/// width, height.
pub type Rect = (f64, f64, f64, f64);

/// What a click in the strip can hit. The drawing and the hit test share
/// [`layout`], so what is drawn is what is clicked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Region {
    /// A tab (switches to it).
    Tab(usize),
    /// A tab's close button.
    CloseTab(usize),
    /// The new-tab button.
    NewTab,
    /// Back.
    Back,
    /// Forward.
    Forward,
    /// Reload, or Stop while loading.
    Reload,
    /// The address field.
    Address,
    /// The zoom level (resets it).
    Zoom,
    /// The find field.
    FindField,
    /// Previous match.
    FindPrevious,
    /// Next match.
    FindNext,
    /// Close the find bar.
    FindClose,
    /// The prompt's Block button.
    Block,
    /// The prompt's Allow button.
    Allow,
    /// Close the download notice.
    DownloadClose,
}

/// One tab as the strip shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabView {
    /// Its title (the address, for now: pages cannot set the strip's
    /// text).
    pub title: String,
    /// Whether it is loading.
    pub loading: bool,
}

/// The address field as the strip shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AddressView {
    /// The committed address (not being edited).
    Committed(Address),
    /// The user's own text being edited, with the caret's character
    /// index and any IME composition at the caret.
    Editing {
        /// The text.
        text: String,
        /// The caret, in characters.
        caret: usize,
        /// Whether the whole text is selected (typing replaces it).
        selected: bool,
        /// The IME composition, shown at the caret.
        preedit: String,
    },
}

/// The find bar as the strip shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FindView {
    /// The query.
    pub text: String,
    /// The caret, in characters.
    pub caret: usize,
    /// The IME composition at the caret.
    pub preedit: String,
    /// Match count and the active match, once the page reported them.
    pub result: Option<(usize, usize)>,
}

/// Everything the strip shows.
#[derive(Clone, Debug)]
pub struct Strip<'a> {
    /// The tabs, left to right.
    pub tabs: &'a [TabView],
    /// The active tab.
    pub active: usize,
    /// Whether back and forward lead anywhere.
    pub back: bool,
    /// See `back`.
    pub forward: bool,
    /// Whether the active tab is loading (Reload becomes Stop).
    pub loading: bool,
    /// The address field.
    pub address: AddressView,
    /// The active tab's zoom factor.
    pub zoom: f64,
    /// The find bar, when open.
    pub find: Option<FindView>,
    /// The permission prompt on screen.
    pub prompt: Option<&'a Pending>,
    /// A download notice.
    pub download: Option<String>,
    /// The profile's label.
    pub profile: &'a str,
}

/// The strip's height in CSS pixels for these rows.
pub fn strip_height(strip: &Strip<'_>) -> u32 {
    let rows = 2
        + u32::from(strip.find.is_some())
        + u32::from(strip.prompt.is_some())
        + u32::from(strip.download.is_some());
    ROW * rows
}

/// Where everything clickable is, for a strip `width` CSS pixels wide.
pub fn layout(strip: &Strip<'_>, width: f64) -> Vec<(Region, Rect)> {
    let mut regions = Vec::new();
    let row = f64::from(ROW);
    let (button, gap, inner) = (22.0, 4.0, 20.0);
    // Row 1: tabs, the new-tab button, the profile label on the right.
    let profile = (strip.profile.chars().count().min(24) as f64 + 2.0) * CHARACTER_WIDTH;
    let room = (width - profile - button - 2.0 * gap).max(0.0);
    let count = strip.tabs.len().max(1) as f64;
    let tab = (room / count).clamp(24.0, 200.0);
    let mut x = gap;
    for index in 0..strip.tabs.len() {
        regions.push((Region::Tab(index), (x, 4.0, tab - 2.0, row - 4.0)));
        regions.push((Region::CloseTab(index), (x + tab - 20.0, 7.0, 16.0, 16.0)));
        x += tab;
    }
    regions.push((Region::NewTab, (x + 2.0, 4.0, button, inner)));
    // Row 2: back, forward, reload/stop, the address, the zoom level.
    let top = row + 4.0;
    regions.push((Region::Back, (gap, top, button, inner)));
    regions.push((Region::Forward, (gap * 2.0 + button, top, button, inner)));
    regions.push((Region::Reload, (gap * 3.0 + button * 2.0, top, 48.0, inner)));
    let zoom = 48.0;
    let address_left = gap * 4.0 + button * 2.0 + 48.0;
    let address_width = (width - address_left - zoom - gap * 2.0).max(0.0);
    regions.push((Region::Address, (address_left, top, address_width, inner)));
    regions.push((
        Region::Zoom,
        (address_left + address_width + gap, top, zoom, inner),
    ));
    let mut next = row * 2.0;
    if strip.find.is_some() {
        let top = next + 4.0;
        let field = (width * 0.4).clamp(80.0, 320.0);
        regions.push((Region::FindField, (gap, top, field, inner)));
        let after = gap * 2.0 + field + 96.0;
        regions.push((Region::FindPrevious, (after, top, button, inner)));
        regions.push((Region::FindNext, (after + button + gap, top, button, inner)));
        regions.push((
            Region::FindClose,
            ((width - button - gap).max(0.0), top, button, inner),
        ));
        next += row;
    }
    if strip.prompt.is_some() {
        let (wide, top) = (64.0, next + 4.0);
        let allow_left = (width - wide - 8.0).max(0.0);
        let block_left = (allow_left - wide - 8.0).max(0.0);
        regions.push((Region::Block, (block_left, top, wide, inner)));
        regions.push((Region::Allow, (allow_left, top, wide, inner)));
        next += row;
    }
    if strip.download.is_some() {
        regions.push((
            Region::DownloadClose,
            ((width - button - gap).max(0.0), next + 4.0, button, inner),
        ));
    }
    regions
}

fn inside((left, top, width, height): Rect, x: f64, y: f64) -> bool {
    x >= left && x < left + width && y >= top && y < top + height
}

/// What a click at (`x`, `y`) CSS pixels hits. Tab close buttons win over
/// their tab.
pub fn hit(strip: &Strip<'_>, width: f64, x: f64, y: f64) -> Option<Region> {
    let regions = layout(strip, width);
    regions
        .iter()
        .find(|(region, rect)| matches!(region, Region::CloseTab(_)) && inside(*rect, x, y))
        .or_else(|| regions.iter().find(|(_, rect)| inside(*rect, x, y)))
        .map(|&(region, _)| region)
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in printable(text).chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// At most `room` characters of `text`, with "..." when cut.
fn fit(text: &str, room: usize) -> String {
    let count = text.chars().count();
    if count <= room {
        return text.to_owned();
    }
    let keep = room.saturating_sub(3);
    let head: String = text.chars().take(keep).collect();
    if room >= 3 {
        format!("{head}...")
    } else {
        head
    }
}

/// An edited line with its caret: the visible window keeps the caret in
/// view; the caret is a left border on the text after it.
fn edited(text: &str, caret: usize, preedit: &str, room: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    let preedit: String = preedit.chars().collect();
    let room = room.max(4);
    let start = caret.saturating_sub(room.saturating_sub(4));
    let before: String = chars[start..caret].iter().collect();
    let after: String = chars[caret..].iter().take(room).collect();
    let composing = if preedit.is_empty() {
        String::new()
    } else {
        format!(
            "<u style=\"text-decoration:underline;color:#111\">{}</u>",
            escape(&preedit)
        )
    };
    format!(
        "<b>{}</b>{composing}<b style=\"border-left:1px solid #111\">{}&#8203;</b>",
        escape(&before),
        escape(&after)
    )
}

fn boxed(rect: Rect, label: &str, extra: &str) -> String {
    let (left, top, w, h) = rect;
    format!(
        "<span style=\"left:{left}px;top:{top}px;width:{}px;height:{}px;{extra}\">{}</span>",
        (w - 2.0).max(0.0),
        (h - 2.0).max(0.0),
        escape(label)
    )
}

/// The strip as an HTML document `width` CSS pixels wide.
pub fn document(strip: &Strip<'_>, width: u32) -> String {
    let width_f = f64::from(width);
    let regions = layout(strip, width_f);
    let at = |wanted: Region| {
        regions
            .iter()
            .find(|(region, _)| *region == wanted)
            .map(|&(_, rect)| rect)
    };
    let height = strip_height(strip);
    let mut html = format!(
        "<!doctype html><html><head><meta charset=utf-8><style>\
         html,body{{margin:0;background:#dcdcdc;}}\
         div{{position:absolute;left:0;width:{width}px;height:{ROW}px;\
         font:13px/{ROW}px monospace;white-space:pre;overflow:hidden;}}\
         b{{font-weight:normal;color:#111;}} i{{font-style:normal;color:#666;}}\
         span{{position:absolute;height:20px;font:13px/20px monospace;text-align:center;\
         border:1px solid #999;background:#f4f4f4;color:#111;overflow:hidden;white-space:pre;}}\
         </style></head><body style=\"height:{height}px\">\
         <div style=\"top:0;\"></div>"
    );
    for (index, tab) in strip.tabs.iter().enumerate() {
        let Some(rect) = at(Region::Tab(index)) else {
            continue;
        };
        let room = ((rect.2 - 28.0) / CHARACTER_WIDTH).floor().max(0.0) as usize;
        let title = if tab.loading {
            format!("* {}", tab.title)
        } else {
            tab.title.clone()
        };
        let style = if index == strip.active {
            "background:#fff;border-bottom:1px solid #fff;text-align:left;"
        } else {
            "background:#e8e8e8;text-align:left;"
        };
        html.push_str(&boxed(rect, &format!(" {}", fit(&title, room)), style));
        if let Some(close) = at(Region::CloseTab(index)) {
            html.push_str(&boxed(close, "x", "border:0;background:transparent;"));
        }
    }
    if let Some(rect) = at(Region::NewTab) {
        html.push_str(&boxed(rect, "+", ""));
    }
    let profile = fit(strip.profile, 24);
    let _ = write!(
        html,
        "<div style=\"top:0;left:auto;right:0;width:{}px;text-align:right;\"><i>{} </i></div>",
        (profile.chars().count() as f64 + 2.0) * CHARACTER_WIDTH,
        escape(&profile)
    );
    // Toolbar.
    let _ = write!(
        html,
        "<div style=\"top:{ROW}px;background:#fff;border-bottom:1px solid #aaa;\"></div>"
    );
    let dim = |enabled: bool| if enabled { "" } else { "color:#aaa;" };
    if let Some(rect) = at(Region::Back) {
        html.push_str(&boxed(rect, "<", dim(strip.back)));
    }
    if let Some(rect) = at(Region::Forward) {
        html.push_str(&boxed(rect, ">", dim(strip.forward)));
    }
    if let Some(rect) = at(Region::Reload) {
        html.push_str(&boxed(
            rect,
            if strip.loading { "Stop" } else { "Reload" },
            "",
        ));
    }
    if let Some(rect) = at(Region::Address) {
        let (left, top, w, h) = rect;
        let room = ((w - 12.0) / CHARACTER_WIDTH).floor().max(0.0) as usize;
        let inner = match &strip.address {
            AddressView::Committed(address) => {
                let (badge, badge_color) = match address.security {
                    Security::Secure => ("Secure", "#1a7f37"),
                    Security::NotSecure => ("Not secure", "#b42318"),
                    Security::File => ("File", "#444"),
                    Security::Opaque => ("", "#444"),
                };
                let badge_chars = if badge.is_empty() { 0 } else { badge.len() + 2 };
                let (host, detail) = elide(address, room.saturating_sub(badge_chars));
                format!(
                    "<i style=\"color:{badge_color}\">{badge}</i>{sep}<b>{}</b><i>{}</i>",
                    escape(&host),
                    escape(&detail),
                    sep = if badge.is_empty() { "" } else { "  " },
                )
            }
            AddressView::Editing {
                text,
                caret,
                selected,
                preedit,
            } => {
                if *selected && preedit.is_empty() {
                    format!(
                        "<b style=\"background:#b3d4fc\">{}</b>",
                        escape(&fit(text, room))
                    )
                } else {
                    edited(text, *caret, preedit, room)
                }
            }
        };
        let border = if matches!(strip.address, AddressView::Editing { .. }) {
            "border-color:#1f6feb;"
        } else {
            ""
        };
        let _ = write!(
            html,
            "<span style=\"left:{left}px;top:{top}px;width:{}px;height:{}px;text-align:left;\
             background:#fff;{border}\"> {inner}</span>",
            (w - 2.0).max(0.0),
            (h - 2.0).max(0.0),
        );
    }
    if let Some(rect) = at(Region::Zoom) {
        let percent = (strip.zoom * 100.0).round();
        html.push_str(&boxed(rect, &format!("{percent}%"), ""));
    }
    let mut next = ROW * 2;
    if let Some(find) = &strip.find {
        let _ = write!(
            html,
            "<div style=\"top:{next}px;background:#f2f2f2;border-bottom:1px solid #aaa;\"></div>"
        );
        if let Some(rect) = at(Region::FindField) {
            let (left, top, w, h) = rect;
            let room = ((w - 12.0) / CHARACTER_WIDTH).floor().max(0.0) as usize;
            let _ = write!(
                html,
                "<span style=\"left:{left}px;top:{top}px;width:{}px;height:{}px;text-align:left;\
                 background:#fff;border-color:#1f6feb;\"> {}</span>",
                (w - 2.0).max(0.0),
                (h - 2.0).max(0.0),
                edited(&find.text, find.caret, &find.preedit, room)
            );
            let result = match find.result {
                Some((0, _)) => "No matches".to_owned(),
                Some((count, active)) => format!("{} of {count}", active + 1),
                None => String::new(),
            };
            let _ = write!(
                html,
                "<div style=\"top:{next}px;left:{}px;width:92px;\"><i>{}</i></div>",
                left + w + 6.0,
                escape(&result)
            );
        }
        for (region, label) in [
            (Region::FindPrevious, "^"),
            (Region::FindNext, "v"),
            (Region::FindClose, "x"),
        ] {
            if let Some(rect) = at(region) {
                html.push_str(&boxed(rect, label, ""));
            }
        }
        next += ROW;
    }
    if let Some(pending) = strip.prompt {
        let block = at(Region::Block).map_or(0.0, |rect| rect.0);
        let room = ((block - 16.0) / CHARACTER_WIDTH).floor().max(0.0) as usize;
        let ask = format!(
            " {} wants to {}",
            pending.origin(),
            pending.permission().label()
        );
        let ask: String = ask.chars().take(room).collect();
        let _ = write!(
            html,
            "<div style=\"top:{next}px;background:#fff8d6;border-bottom:1px solid #aaa;\">{}</div>",
            escape(&ask)
        );
        for (region, label) in [(Region::Block, "Block"), (Region::Allow, "Allow")] {
            if let Some(rect) = at(region) {
                html.push_str(&boxed(rect, label, ""));
            }
        }
        next += ROW;
    }
    if let Some(download) = &strip.download {
        let room = ((width_f - 40.0) / CHARACTER_WIDTH).floor().max(0.0) as usize;
        let _ = write!(
            html,
            "<div style=\"top:{next}px;background:#e6f0ff;border-bottom:1px solid #aaa;\"> {}</div>",
            escape(&fit(download, room))
        );
        if let Some(rect) = at(Region::DownloadClose) {
            html.push_str(&boxed(rect, "x", ""));
        }
    }
    html.push_str("</body></html>");
    html
}

/// A trusted page the browser shows in place of a document (a load
/// error, a refused download, an empty tab): a heading and a detail line,
/// escaped printable ASCII.
pub fn message_document(heading: &str, detail: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=utf-8><style>\
         html,body{{margin:0;background:#fff;color:#222;}}\
         h1{{font:20px/28px sans-serif;margin:48px 32px 12px;}}\
         p{{font:14px/20px sans-serif;margin:0 32px;color:#555;white-space:pre-wrap;}}\
         </style></head><body><h1>{}</h1><p>{}</p></body></html>",
        escape(heading),
        escape(detail)
    )
}

/// The strip's document: no subresources, no scripts, no storage.
#[derive(Debug)]
struct StripSession {
    url: Url,
    csp: CspList,
}

impl Session for StripSession {
    fn document_url(&self) -> &Url {
        &self.url
    }
    fn csp(&self) -> &CspList {
        &self.csp
    }
    fn fetch(&self, _: &Url, _: &ResourceKind, _: usize) -> Result<Vec<u8>, FetchError> {
        Err(FetchError::Blocked)
    }
    fn charset_of(&self, _: &Url) -> Option<String> {
        None
    }
    fn script_fetch(
        &self,
        _: rustic_engine::ScriptRequest,
    ) -> Result<rustic_engine::ScriptResponse, rustic_engine::NetworkFailure> {
        Err(rustic_engine::NetworkFailure)
    }
    fn storage_host(&self) -> Result<Option<Rc<dyn rustic_engine::StorageHost>>, String> {
        Ok(None)
    }
    fn finish(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Render a trusted `html` document of `css_height` CSS pixels at `width`
/// device pixels and `scale` device pixels per CSS pixel, in this
/// (trusted) process, scripts off: the strip, or a message page.
pub fn paint_trusted(
    html: String,
    width: u32,
    css_height: u32,
    scale: f64,
) -> Result<rustic_raster::Surface, String> {
    let scale = rustic_shell::usable_scale(scale);
    let css_width = ((f64::from(width) / scale).floor() as u32).max(1);
    let url = Url::parse("about:blank", None).map_err(|_| "no strip URL".to_owned())?;
    let document = SessionDocument {
        session: Rc::new(StripSession {
            url,
            csp: CspList::default(),
        }),
        bytes: std::cell::RefCell::new(html.into_bytes()),
        transport_encoding: None,
    };
    let limits = LoaderLimits::default();
    let mut engine = Engine::new(1);
    let parsed = loader::load_session_document(&mut engine, &document, limits)
        .map_err(|error| format!("cannot draw the browser's own UI: {error}"))?;
    let view = rustic_layout::View::new(
        f64::from(css_width),
        f64::from(css_height.max(1)),
        scale,
        1.0,
    )
    .map_err(|_| "cannot size the browser's own UI".to_owned())?;
    loader::render_list(&parsed, std::path::Path::new("about:blank"), limits, view)
        .map(|(_, surface)| surface)
        .map_err(|error| format!("cannot draw the browser's own UI: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text, None).unwrap()
    }

    #[test]
    fn addresses_show_ascii_hosts_without_credentials_or_fragments() {
        let shown = address(&url("https://user:pw@bücher.example:8443/a?b=1#secret"));
        assert_eq!(shown.security, Security::Secure);
        assert_eq!(shown.host, "https://xn--bcher-kva.example");
        assert_eq!(shown.detail, ":8443/a?b=1");
        assert_eq!(
            address(&url("http://a.test/")).security,
            Security::NotSecure
        );
        assert_eq!(
            address(&url("http://127.0.0.1/")).security,
            Security::Secure
        );
        assert_eq!(address(&url("file:///tmp/a.html")).security, Security::File);
        let opaque = address(&url("data:text/html,<b>hi</b>"));
        assert_eq!(opaque.security, Security::Opaque);
        assert!(
            opaque
                .host
                .chars()
                .all(|c| c.is_ascii_graphic() || c == ' ')
        );
    }

    #[test]
    fn elision_keeps_the_end_of_the_host_and_the_start_of_the_path() {
        let shown = address(&url(
            "https://login.bank.example.attacker.test/very/long/path",
        ));
        let (host, detail) = elide(&shown, 30);
        assert_eq!(host, "....bank.example.attacker.test");
        assert_eq!(host.len(), 30);
        assert!(host.ends_with("attacker.test"));
        assert_eq!(detail, "");
        let (host, detail) = elide(&address(&url("https://a.test/abcdefghij")), 20);
        assert_eq!(host, "https://a.test");
        assert_eq!(detail, "/ab...");
        let (_, detail) = elide(&address(&url("https://a.test/x")), 40);
        assert_eq!(detail, "/x");
    }

    #[test]
    fn prompts_need_a_secure_origin_activation_and_no_other_prompt() {
        let now = Instant::now();
        let secure = url("https://a.test/").origin();
        let mut permissions = Permissions::default();
        assert_eq!(
            permissions.request(
                &url("http://a.test/").origin(),
                Permission::Camera,
                true,
                now
            ),
            Decision::Denied
        );
        assert_eq!(
            permissions.request(
                &url("data:text/html,x").origin(),
                Permission::Camera,
                true,
                now
            ),
            Decision::Denied
        );
        assert_eq!(
            permissions.request(&secure, Permission::Camera, false, now),
            Decision::Denied
        );
        assert_eq!(
            permissions.request(&secure, Permission::Camera, true, now),
            Decision::Prompt
        );
        let other = url("https://b.test/").origin();
        assert_eq!(
            permissions.request(&other, Permission::Geolocation, true, now),
            Decision::Denied
        );
        assert_eq!(permissions.pending().unwrap().origin(), "https://a.test");
    }

    #[test]
    fn early_clicks_are_ignored_and_answers_are_kept_per_origin() {
        let start = Instant::now();
        let origin = url("https://a.test/").origin();
        let mut permissions = Permissions::default();
        permissions.request(&origin, Permission::Notifications, true, start);
        let early = start + INPUT_PROTECTION / 2;
        assert_eq!(permissions.answer(Answer::Allow, early), None);
        assert!(permissions.pending().is_some());
        let later = start + INPUT_PROTECTION;
        let (_, _, decision) = permissions.answer(Answer::Allow, later).unwrap();
        assert_eq!(decision, Decision::Granted);
        // Remembered for the origin, without activation or a prompt.
        assert_eq!(
            permissions.request(&origin, Permission::Notifications, false, later),
            Decision::Granted
        );
        // Another origin and another permission still ask.
        assert_eq!(
            permissions.request(&origin, Permission::Camera, true, later),
            Decision::Prompt
        );
        permissions.answer(Answer::Block, later + INPUT_PROTECTION);
        assert_eq!(
            permissions.request(&origin, Permission::Camera, true, later),
            Decision::Denied
        );
    }

    #[test]
    fn dismissals_embargo_and_navigation_closes_the_prompt() {
        let now = Instant::now();
        let origin = url("https://a.test/").origin();
        let mut permissions = Permissions::default();
        for _ in 0..EMBARGO_DISMISSALS {
            assert_eq!(
                permissions.request(&origin, Permission::Geolocation, true, now),
                Decision::Prompt
            );
            // A dismissal is accepted at once.
            assert!(permissions.answer(Answer::Dismiss, now).is_some());
        }
        assert_eq!(
            permissions.request(&origin, Permission::Geolocation, true, now),
            Decision::Denied
        );
        permissions.request(&origin, Permission::Microphone, true, now);
        permissions.navigated(now);
        assert!(permissions.pending().is_none());
        assert_eq!(
            permissions.answer(Answer::Allow, now + INPUT_PROTECTION),
            None
        );
    }

    #[test]
    fn clipboard_access_needs_focus_and_activation() {
        let now = Instant::now();
        let origin = url("https://a.test/").origin();
        assert!(Permissions::clipboard_write(&origin, true, true));
        assert!(!Permissions::clipboard_write(&origin, false, true));
        assert!(!Permissions::clipboard_write(&origin, true, false));
        assert!(!Permissions::clipboard_write(
            &url("http://a.test/").origin(),
            true,
            true
        ));
        let mut permissions = Permissions::default();
        assert_eq!(
            permissions.clipboard_read(&origin, true, false, now),
            Decision::Denied
        );
        assert_eq!(
            permissions.clipboard_read(&origin, true, true, now),
            Decision::Prompt
        );
        assert_eq!(
            permissions.pending().unwrap().permission(),
            Permission::ClipboardRead
        );
    }

    fn strip<'a>(prompt: Option<&'a Pending>, tabs: &'a [TabView]) -> Strip<'a> {
        Strip {
            tabs,
            active: 0,
            back: false,
            forward: true,
            loading: false,
            address: AddressView::Committed(address(&url("https://a.test/x"))),
            zoom: 1.0,
            find: None,
            prompt,
            download: None,
            profile: "Default",
        }
    }

    #[test]
    fn prompt_buttons_hit_only_while_a_prompt_is_shown() {
        let now = Instant::now();
        let tabs = [TabView {
            title: "a.test".into(),
            loading: false,
        }];
        let mut permissions = Permissions::default();
        let plain = strip(None, &tabs);
        assert_eq!(strip_height(&plain), ROW * 2);
        assert!(
            !layout(&plain, 400.0)
                .iter()
                .any(|(r, _)| *r == Region::Allow)
        );
        permissions.request(
            &url("https://a.test/").origin(),
            Permission::Camera,
            true,
            now,
        );
        let prompted = strip(permissions.pending(), &tabs);
        assert_eq!(strip_height(&prompted), ROW * 3);
        let regions = layout(&prompted, 400.0);
        let at = |wanted: Region| {
            let (_, (x, y, _, _)) = *regions.iter().find(|(r, _)| *r == wanted).unwrap();
            (x + 1.0, y + 1.0)
        };
        let (x, y) = at(Region::Allow);
        assert_eq!(hit(&prompted, 400.0, x, y), Some(Region::Allow));
        let (x, y) = at(Region::Block);
        assert_eq!(hit(&prompted, 400.0, x, y), Some(Region::Block));
        // A tab's close button wins over the tab under it.
        let (x, y) = at(Region::CloseTab(0));
        assert_eq!(hit(&prompted, 400.0, x, y), Some(Region::CloseTab(0)));
        assert_eq!(hit(&prompted, 400.0, 399.0, 200.0), None);
    }

    #[test]
    fn the_strip_escapes_what_it_shows_and_renders() {
        let now = Instant::now();
        let shown = Address {
            security: Security::NotSecure,
            host: "http://a.test".into(),
            detail: "/<script>alert(1)</script>\u{202e}".into(),
        };
        let mut permissions = Permissions::default();
        permissions.request(
            &url("https://a.test/").origin(),
            Permission::Camera,
            true,
            now,
        );
        let tabs = [TabView {
            title: "<b>\u{202e}evil</b>".into(),
            loading: true,
        }];
        let mut view = strip(permissions.pending(), &tabs);
        view.address = AddressView::Committed(shown);
        view.download = Some("Downloaded <x>.exe".into());
        view.find = Some(FindView {
            text: "<i>".into(),
            caret: 1,
            preedit: "\u{65e5}".into(),
            result: Some((2, 0)),
        });
        let html = document(&view, 1000);
        assert!(!html.contains("<script>") && !html.contains("<x>") && !html.contains("<i></"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.is_ascii());
        let height = strip_height(&view);
        assert_eq!(height, ROW * 5);
        let surface = paint_trusted(document(&view, 200), 400, height, 2.0).unwrap();
        assert_eq!(surface.width(), 400);
        assert_eq!(surface.height(), height * 2);
        let page = paint_trusted(message_document("Oops", "<p>why"), 200, 100, 1.0).unwrap();
        assert_eq!((page.width(), page.height()), (200, 100));
    }
}
