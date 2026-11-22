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

/// Where the prompt's buttons are, in CSS pixels from the strip's
/// top-left corner (the drawing and the hit test share these).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Buttons {
    /// Block: left, top, width, height.
    pub block: (f64, f64, f64, f64),
    /// Allow: left, top, width, height.
    pub allow: (f64, f64, f64, f64),
}

/// The buttons of a prompt row in a strip `width` CSS pixels wide.
pub fn buttons(width: f64) -> Buttons {
    let (button, gap, height) = (64.0, 8.0, 20.0);
    let top = f64::from(ROW) + 4.0;
    let allow_left = (width - button - gap).max(0.0);
    let block_left = (allow_left - button - gap).max(0.0);
    Buttons {
        block: (block_left, top, button, height),
        allow: (allow_left, top, button, height),
    }
}

fn inside((left, top, width, height): (f64, f64, f64, f64), x: f64, y: f64) -> bool {
    x >= left && x < left + width && y >= top && y < top + height
}

/// The strip's height in CSS pixels: one row, two with a prompt.
pub fn height(permissions: &Permissions) -> u32 {
    if permissions.pending().is_some() {
        ROW * 2
    } else {
        ROW
    }
}

/// The answer a click at (`x`, `y`) CSS pixels gives, if it hits a button.
pub fn hit(permissions: &Permissions, width: f64, x: f64, y: f64) -> Option<Answer> {
    permissions.pending()?;
    let buttons = buttons(width);
    if inside(buttons.allow, x, y) {
        Some(Answer::Allow)
    } else if inside(buttons.block, x, y) {
        Some(Answer::Block)
    } else {
        None
    }
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

/// The strip as an HTML document `width` CSS pixels wide.
pub fn document(address: &Address, permissions: &Permissions, width: u32) -> String {
    let width_f = f64::from(width);
    let (badge, badge_color) = match address.security {
        Security::Secure => ("Secure", "#1a7f37"),
        Security::NotSecure => ("Not secure", "#b42318"),
        Security::File => ("File", "#444"),
        Security::Opaque => ("", "#444"),
    };
    let badge_chars = badge.len() + 2;
    let room = ((width_f - 16.0) / CHARACTER_WIDTH).floor().max(0.0) as usize;
    let (host, detail) = elide(address, room.saturating_sub(badge_chars));
    let mut html = format!(
        "<!doctype html><html><head><meta charset=utf-8><style>\
         html,body{{margin:0;background:#e8e8e8;}}\
         div{{position:absolute;left:0;width:{width}px;height:{ROW}px;\
         font:13px/{ROW}px monospace;white-space:pre;overflow:hidden;}}\
         b{{font-weight:normal;color:#111;}} i{{font-style:normal;color:#666;}}\
         span{{position:absolute;height:20px;font:13px/20px monospace;text-align:center;\
         border:1px solid #888;background:#fff;color:#111;}}\
         </style></head><body>\
         <div style=\"top:0;border-bottom:1px solid #aaa;\">&#32;<i style=\"color:{badge_color}\">{badge}</i>{sep}<b>{host}</b><i>{detail}</i></div>",
        sep = if badge.is_empty() { "" } else { "  " },
        host = escape(&host),
        detail = escape(&detail),
    );
    if let Some(pending) = permissions.pending() {
        let buttons = buttons(width_f);
        let room = ((buttons.block.0 - 16.0) / CHARACTER_WIDTH)
            .floor()
            .max(0.0) as usize;
        let ask = format!(
            " {} wants to {}",
            pending.origin(),
            pending.permission().label()
        );
        let ask: String = ask.chars().take(room).collect();
        let _ = write!(
            html,
            "<div style=\"top:{ROW}px;background:#fff8d6;border-bottom:1px solid #aaa;\">{}</div>",
            escape(&ask)
        );
        for ((left, top, w, h), label) in [(buttons.block, "Block"), (buttons.allow, "Allow")] {
            let _ = write!(
                html,
                "<span style=\"left:{left}px;top:{top}px;width:{}px;height:{}px;\">{label}</span>",
                w - 2.0,
                h - 2.0
            );
        }
    }
    html.push_str("</body></html>");
    html
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

/// Render the strip at `width` device pixels and `scale` device pixels per
/// CSS pixel, in this (trusted) process, scripts off.
pub fn paint(
    address: &Address,
    permissions: &Permissions,
    width: u32,
    scale: f64,
) -> Result<rustic_raster::Surface, String> {
    let css_width = ((f64::from(width) / scale).floor() as u32).max(1);
    let css_height = height(permissions);
    let html = document(address, permissions, css_width);
    let url = Url::parse("about:blank", None).map_err(|_| "no strip URL".to_owned())?;
    let document = SessionDocument {
        session: Rc::new(StripSession {
            url,
            csp: CspList::default(),
        }),
        bytes: html.into_bytes(),
        transport_encoding: None,
    };
    let limits = LoaderLimits::default();
    let mut engine = Engine::new(1);
    let parsed = loader::load_session_document(&mut engine, &document, limits)
        .map_err(|error| format!("cannot draw the address bar: {error}"))?;
    let view = rustic_layout::View::new(f64::from(css_width), f64::from(css_height), scale, 1.0)
        .map_err(|_| "cannot size the address bar".to_owned())?;
    loader::render_list(&parsed, std::path::Path::new("about:blank"), limits, view)
        .map(|(_, surface)| surface)
        .map_err(|error| format!("cannot draw the address bar: {error}"))
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

    #[test]
    fn buttons_hit_only_while_a_prompt_is_shown() {
        let now = Instant::now();
        let mut permissions = Permissions::default();
        assert_eq!(hit(&permissions, 400.0, 360.0, 40.0), None);
        assert_eq!(height(&permissions), ROW);
        permissions.request(
            &url("https://a.test/").origin(),
            Permission::Camera,
            true,
            now,
        );
        assert_eq!(height(&permissions), ROW * 2);
        let buttons = buttons(400.0);
        let (x, y) = (buttons.allow.0 + 1.0, buttons.allow.1 + 1.0);
        assert_eq!(hit(&permissions, 400.0, x, y), Some(Answer::Allow));
        let (x, y) = (buttons.block.0 + 1.0, buttons.block.1 + 1.0);
        assert_eq!(hit(&permissions, 400.0, x, y), Some(Answer::Block));
        assert_eq!(hit(&permissions, 400.0, 5.0, 5.0), None);
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
        let html = document(&shown, &permissions, 1000);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.is_ascii());
        let surface = paint(&shown, &permissions, 400, 2.0).unwrap();
        assert_eq!(surface.width(), 400);
        assert_eq!(surface.height(), ROW * 2 * 2);
    }
}
