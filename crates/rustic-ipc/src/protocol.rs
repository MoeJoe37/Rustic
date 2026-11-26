//! Protocol version 2 between the broker and a renderer (ADR 0056;
//! frames and surfaces, ADR 0062).
//!
//! The broker starts the renderer with one [`Start`]: the command to run
//! and the document bytes it is allowed to see. Everything else the
//! renderer needs it *asks for* with a numbered request; the broker checks
//! each request against the authority it granted that renderer and replies
//! once. The renderer ends with [`RendererMessage::Done`].

use crate::Message;
use crate::wire::{DecodeError, Reader, Writer};

/// Most header fields in one message.
pub const MAX_HEADERS: usize = 256;
/// Most argument or policy strings in one message.
pub const MAX_STRINGS: usize = 256;
/// Most Web Storage keys returned at once.
pub const MAX_STORAGE_KEYS: usize = 100_000;

/// The document a renderer may render, as granted by the broker.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocumentGrant {
    /// The document URL (its base URL and origin).
    pub url: String,
    /// The document bytes.
    pub bytes: Vec<u8>,
    /// The response `charset`, if any.
    pub transport_charset: Option<String>,
    /// A local file (UTF-8 fallback, file directory grant) rather than a
    /// network response.
    pub local: bool,
    /// `Content-Security-Policy` values.
    pub csp: Vec<String>,
    /// `Content-Security-Policy-Report-Only` values.
    pub csp_report_only: Vec<String>,
    /// The document's referrer policy token (may be empty).
    pub referrer_policy: String,
    /// Whether the origin has Web Storage (not opaque).
    pub storage: bool,
    /// `bytes` is only what has arrived so far: the renderer asks for the
    /// rest with [`RendererMessage::DocumentChunk`] (TD-165).
    pub streamed: bool,
}

/// The renderer's job.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Start {
    /// The command (`dump-dom`, `render`, …).
    pub command: String,
    /// Its non-authority arguments (limits, view, points).
    pub args: Vec<String>,
    /// The document.
    pub document: DocumentGrant,
    /// Font files the user granted (the renderer opens no files).
    pub fonts: Vec<Vec<u8>>,
    /// `generic=family` mappings for those fonts (system fonts, ADR 0074).
    pub families: Vec<String>,
}

/// Most granted font files in one [`Start`].
pub const MAX_FONTS: usize = 64;

/// Most frames one document may ask for, and most layers in a [`Surface`].
pub const MAX_FRAMES: usize = 16;

/// An `iframe` the renderer asks the broker to load and render in another
/// renderer (ADR 0062). The renderer never receives the frame's document
/// or pixels, only a handle to place them with.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrameRequest {
    /// The resolved `src` URL, `about:srcdoc` or `about:blank`.
    pub url: String,
    /// The `srcdoc` attribute.
    pub srcdoc: Option<String>,
    /// The `sandbox` attribute's tokens, when it has the attribute.
    pub sandbox: Option<Vec<String>>,
    /// The frame's viewport width in CSS pixels.
    pub width: u32,
    /// The frame's viewport height in CSS pixels.
    pub height: u32,
}

/// Where a frame's pixels go: within the `width × height` rectangle at
/// `x, y` of a [`Surface`], each output pixel gains `coverage / 255` of
/// the frame's pixel at the same offset from the rectangle's origin.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrameLayer {
    /// The handle the broker gave the frame.
    pub handle: u32,
    /// Left edge in surface pixels.
    pub x: u32,
    /// Top edge in surface pixels.
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// One weight per pixel, row by row.
    pub coverage: Vec<u8>,
}

/// A rendered page: premultiplied RGBA rows and the frames to composite
/// into it (`render` in a renderer sends this instead of a PNG).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Surface {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Premultiplied RGBA, four bytes per pixel, row by row.
    pub pixels: Vec<u8>,
    /// Frame layers, in paint order.
    pub layers: Vec<FrameLayer>,
}

/// Modifier bits in [`PageInput`]: Control, Alt, Shift, Meta.
pub const MODIFIER_CONTROL: u8 = 1;
/// Alt.
pub const MODIFIER_ALT: u8 = 2;
/// Shift.
pub const MODIFIER_SHIFT: u8 = 4;
/// Meta (Command, Windows key).
pub const MODIFIER_META: u8 = 8;

/// Longest `key` or `code` string in a [`PageInput::Key`], in bytes.
pub const MAX_KEY_BYTES: usize = 64;

/// Longest IME composition text or assistive-technology value in a
/// [`PageInput`], in bytes (ADR 0073).
pub const MAX_INPUT_TEXT_BYTES: usize = 64 * 1024;

/// Largest encoded accessibility tree a renderer may send (ADR 0073).
pub const MAX_ACCESSIBILITY_BYTES: usize = 32 * 1024 * 1024;

/// An IME composition step in a [`PageInput::Composition`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompositionPhase {
    /// Composition started.
    Start,
    /// The composition text changed.
    Update,
    /// The text was committed and composition ended.
    End,
}

/// What assistive technology asked a node to do ([`PageInput::Assistive`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssistiveRequest {
    /// Focus it.
    Focus,
    /// Its default action.
    Click,
    /// Replace a text field's value.
    SetValue(String),
    /// Select UTF-16 offsets in a text field.
    SetSelection {
        /// Where the selection starts.
        anchor: u32,
        /// Where it ends.
        focus: u32,
    },
}

/// A frame request for a long-lived page (TD-182, ADR 0069).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameAsk {
    /// The viewport width in CSS pixels (1 to 16384).
    pub width: u32,
    /// The viewport height in CSS pixels (1 to 16384).
    pub height: u32,
    /// Device pixels per CSS pixel (0.25 to 8).
    pub scale: f64,
    /// Page zoom (0.25 to 5); the layout viewport is the size over it.
    pub zoom: f64,
    /// Where the user scrolled the viewport since the last frame, in CSS
    /// pixels; `None` keeps the page's own position.
    pub scroll: Option<(f64, f64)>,
    /// Time since the previous frame request, in microseconds (the
    /// page's clock advances by it, at most one second per frame).
    pub elapsed_us: u64,
    /// Find in page: the query (at most [`MAX_FIND_BYTES`]) and the
    /// active match; matches are marked on the frame.
    pub find: Option<(String, u32)>,
}

/// Longest find-in-page query in a [`FrameAsk`], in bytes.
pub const MAX_FIND_BYTES: usize = 1024;

/// Input for a long-lived page (TD-182): sent by the broker, which got it
/// from the trusted window host; a renderer never synthesizes it.
#[derive(Clone, Debug, PartialEq)]
pub enum PageInput {
    /// Advance the page's clock, run what is due and paint a frame;
    /// answered by [`RendererMessage::Painted`].
    Frame(FrameAsk),
    /// A primary click in the window (answered by
    /// [`RendererMessage::Handled`]), in CSS pixels at 100% zoom from
    /// the page area's top-left corner (the renderer divides by the zoom).
    Click {
        /// From the left edge.
        x: f64,
        /// From the top edge.
        y: f64,
        /// `MODIFIER_*` bits.
        modifiers: u8,
    },
    /// A key press or release (DOM `key` and `code` values; answered by
    /// [`RendererMessage::Handled`]).
    Key {
        /// Pressed rather than released.
        down: bool,
        /// The `key` value, such as `a` or `Enter`.
        key: String,
        /// The `code` value, such as `KeyA` or `Enter`.
        code: String,
        /// An automatic repeat.
        repeat: bool,
        /// `MODIFIER_*` bits.
        modifiers: u8,
    },
    /// An IME composition step for the focused element (answered by
    /// [`RendererMessage::Handled`]; ADR 0073).
    Composition {
        /// Which step.
        phase: CompositionPhase,
        /// The composition or committed text (empty for a start).
        text: String,
    },
    /// An assistive technology's request for one accessibility node, by
    /// the id the page's tree gave it (answered by
    /// [`RendererMessage::Handled`]; ADR 0073).
    Assistive {
        /// The accessibility node id.
        node: u64,
        /// The request.
        request: AssistiveRequest,
    },
    /// Send the page's accessibility tree as it is now (answered by
    /// [`RendererMessage::Accessibility`]; ADR 0073).
    Accessibility,
}

/// A long-lived page's answer to [`PageInput::Frame`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Painted {
    /// The page at the asked size and scale.
    pub surface: Surface,
    /// The viewport position painted, in CSS pixels.
    pub scroll: (f64, f64),
    /// When the page wants its next frame (a timer or animation frame
    /// callback), in microseconds from now; `None` when nothing is due.
    pub next_frame_us: Option<u64>,
    /// Find in page: the match count, the active match and its top and
    /// bottom in CSS pixels of the document (when the frame asked).
    pub found: Option<(u32, u32, f64, f64)>,
    /// The focused editable text field's border box (left, top, width,
    /// height in CSS pixels of the viewport; all zero when it has no box),
    /// so the host enables its input method there (ADR 0074).
    pub editing: Option<(f64, f64, f64, f64)>,
}

/// What a resource request is for (decides the broker's policy checks).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceKind {
    /// A stylesheet or `@import`, for a document in quirks mode or not
    /// (a same-origin sheet there may have any MIME type).
    Stylesheet {
        /// The document is in quirks mode.
        quirks: bool,
    },
    /// An image.
    Image,
    /// A script, with the element's nonce and parser state.
    Script {
        /// The `nonce` attribute.
        nonce: Option<String>,
        /// Whether the parser inserted the element.
        parser_inserted: bool,
        /// A module script (CORS mode).
        module: bool,
    },
}

/// Why the broker did not supply a resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceFailure {
    /// Refused by the renderer's grant or the document's policy.
    Blocked,
    /// It does not exist.
    NotFound,
    /// It is larger than the requested budget.
    TooLarge,
    /// Anything else.
    Failed,
    /// The load was cancelled.
    Cancelled,
}

/// A subresource body and its `charset`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceBody {
    /// The bytes.
    pub bytes: Vec<u8>,
    /// The response `charset`, if any.
    pub charset: Option<String>,
}

/// A script `fetch()` as the renderer asks for it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FetchRequest {
    /// The method.
    pub method: String,
    /// The URL.
    pub url: String,
    /// Header fields.
    pub headers: Vec<(String, Vec<u8>)>,
    /// The body.
    pub body: Option<Vec<u8>>,
    /// The request mode token.
    pub mode: String,
    /// The credentials mode token.
    pub credentials: String,
    /// The redirect mode token.
    pub redirect: String,
    /// The referrer policy token.
    pub referrer_policy: String,
}

/// A filtered `fetch()` response.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FetchResponse {
    /// `basic`, `cors`, `opaque` or `opaqueredirect`.
    pub kind: String,
    /// The final URL, if visible.
    pub url: Option<String>,
    /// Whether redirects were followed.
    pub redirected: bool,
    /// The status.
    pub status: u16,
    /// The status message.
    pub status_text: Vec<u8>,
    /// Visible header fields.
    pub headers: Vec<(String, Vec<u8>)>,
    /// The visible body when buffered.
    pub body: Vec<u8>,
    /// Broker-local stream handle when the body is deferred.
    pub stream: Option<u64>,
}

/// Which Web Storage area.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageArea {
    /// `localStorage`.
    Local,
    /// `sessionStorage`.
    Session,
}

/// One Web Storage operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageOp {
    /// `length`.
    Length,
    /// `key(index)`.
    Key(u32),
    /// `getItem(key)`.
    Get(Vec<u16>),
    /// `setItem(key, value)`.
    Set(Vec<u16>, Vec<u16>),
    /// `removeItem(key)`.
    Remove(Vec<u16>),
    /// `clear()`.
    Clear,
    /// Every key.
    Keys,
}

/// A Web Storage reply.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageValue {
    /// A count.
    Count(u32),
    /// A key or value, if present.
    Units(Option<Vec<u16>>),
    /// Every key.
    Keys(Vec<Vec<u16>>),
    /// The change was made.
    Done,
    /// A quota refused the change.
    QuotaExceeded,
    /// The document has no storage.
    Refused,
}

/// Why a streamed document body stopped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DocumentFailure {
    /// It passed the document byte limit.
    TooLarge,
    /// The transfer failed, with the broker's description.
    Failed(String),
}

/// Broker to renderer.
#[derive(Clone, Debug, PartialEq)]
pub enum BrokerMessage {
    /// The job.
    Start(Start),
    /// The reply to [`RendererMessage::Prefetch`] number `id`: the
    /// fetches started (TD-165).
    Prefetched {
        /// The request number.
        id: u32,
    },
    /// The reply to [`RendererMessage::DocumentChunk`] number `id`: the
    /// next piece of the document body, `None` at its end.
    DocumentChunk {
        /// The request number.
        id: u32,
        /// The piece, or why the body stopped.
        result: Result<Option<Vec<u8>>, DocumentFailure>,
    },
    /// The reply to [`RendererMessage::Resource`] number `id`.
    Resource {
        /// The request number.
        id: u32,
        /// The body or why there is none.
        result: Result<ResourceBody, ResourceFailure>,
    },
    /// The reply to [`RendererMessage::Fetch`] number `id`; `None` is a
    /// network error.
    Fetch {
        /// The request number.
        id: u32,
        /// The response.
        result: Option<FetchResponse>,
    },
    /// A chunk, end of stream, or failure for a deferred Fetch body.
    FetchBody {
        /// The request number.
        id: u32,
        /// The chunk, clean EOF, or failure.
        result: Result<Option<Vec<u8>>, ResourceFailure>,
    },
    /// The reply to [`RendererMessage::Storage`] number `id`.
    Storage {
        /// The request number.
        id: u32,
        /// The result.
        value: StorageValue,
    },
    /// The reply to [`RendererMessage::Frame`] number `id`: a handle for
    /// the frame's layer, or why it is not shown.
    Frame {
        /// The request number.
        id: u32,
        /// The handle.
        result: Result<u32, ResourceFailure>,
    },
    /// The reply to [`RendererMessage::Popup`] number `id`: whether the
    /// popup opened.
    Popup {
        /// The request number.
        id: u32,
        /// Whether it opened.
        opened: bool,
    },
    /// Input or a frame request for a long-lived page (`render --live`).
    Page(PageInput),
    /// The reply to [`RendererMessage::AutomationRead`] number `id`: the
    /// next automation command the broker admitted (ADR 0072), or `None`
    /// when the client closed the session.
    AutomationCommand {
        /// The request number.
        id: u32,
        /// One protocol line, without its line break.
        line: Option<String>,
    },
}

/// Renderer to broker.
#[derive(Clone, Debug, PartialEq)]
pub enum RendererMessage {
    /// Start fetching subresources the renderer will ask for, side by
    /// side (TD-165); each still comes back through
    /// [`RendererMessage::Resource`].
    Prefetch {
        /// The request number.
        id: u32,
        /// URLs and what each is for.
        requests: Vec<(String, ResourceKind)>,
    },
    /// Ask for the next piece of a streamed document body (TD-165).
    DocumentChunk {
        /// The request number.
        id: u32,
    },
    /// Ask for a subresource.
    Resource {
        /// The request number.
        id: u32,
        /// The URL.
        url: String,
        /// What it is for.
        kind: ResourceKind,
        /// The renderer's byte budget for it.
        max_bytes: u64,
    },
    /// Run a script `fetch()`.
    Fetch {
        /// The request number.
        id: u32,
        /// The request.
        request: FetchRequest,
    },
    /// Pull the next chunk of a Fetch body or release its handle.
    FetchBody {
        /// The request number.
        id: u32,
        /// A stream opened by this renderer's broker.
        stream: u64,
        /// Release without reading a chunk.
        close: bool,
    },
    /// A Web Storage operation.
    Storage {
        /// The request number.
        id: u32,
        /// The area.
        area: StorageArea,
        /// The operation.
        op: StorageOp,
    },
    /// A line for the user's diagnostics (console, CSP reports).
    Diagnostic(String),
    /// Load and render an `iframe` in another renderer.
    Frame {
        /// The request number.
        id: u32,
        /// The frame.
        request: FrameRequest,
    },
    /// The rendered surface (`render` only), sent before [`Self::Done`].
    Surface(Surface),
    /// Open a popup the page asked for with transient activation; the
    /// broker decides its browsing context group (ADR 0057 §3).
    Popup {
        /// The request number.
        id: u32,
        /// The resolved URL.
        url: String,
        /// `noopener` (implied by `noreferrer`).
        noopener: bool,
        /// `noreferrer`.
        noreferrer: bool,
    },
    /// A long-lived page's frame, answering [`PageInput::Frame`].
    Painted(Painted),
    /// A long-lived page delivered a click or key (and ran what it
    /// started); the broker sends nothing else until this arrives.
    Handled,
    /// A long-lived page's encoded accessibility tree (`rustic_a11y::wire`),
    /// answering [`PageInput::Accessibility`]; the broker decodes it as
    /// hostile input.
    Accessibility(Vec<u8>),
    /// An automation protocol line for the client (ADR 0072); the broker
    /// forwards it only when it is one well-formed response or event.
    AutomationOutput(String),
    /// Ask for the next automation command.
    AutomationRead {
        /// The request number.
        id: u32,
    },
    /// The job finished: an exit status and the command's output.
    Done {
        /// 0 success, 1 load failure, 2 usage error.
        status: u8,
        /// Standard output bytes.
        output: Vec<u8>,
    },
}

pub(crate) fn put_headers(writer: &mut Writer, headers: &[(String, Vec<u8>)]) {
    writer.u32(u32::try_from(headers.len()).unwrap_or(u32::MAX));
    for (name, value) in headers {
        writer.str(name);
        writer.bytes(value);
    }
}

pub(crate) fn get_headers(reader: &mut Reader<'_>) -> Result<Vec<(String, Vec<u8>)>, DecodeError> {
    let count = reader.u32()? as usize;
    if count > MAX_HEADERS {
        return Err(DecodeError::OutOfRange);
    }
    (0..count)
        .map(|_| Ok((reader.string()?, reader.bytes()?)))
        .collect()
}

fn put_units(writer: &mut Writer, units: &[u16]) {
    let bytes: Vec<u8> = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
    writer.bytes(&bytes);
}

fn get_units(reader: &mut Reader<'_>) -> Result<Vec<u16>, DecodeError> {
    let bytes = reader.bytes()?;
    if bytes.len() % 2 != 0 {
        return Err(DecodeError::OutOfRange);
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect())
}

pub(crate) fn put_opt_bytes(writer: &mut Writer, value: Option<&[u8]>) {
    match value {
        Some(bytes) => {
            writer.u8(1);
            writer.bytes(bytes);
        }
        None => writer.u8(0),
    }
}

pub(crate) fn get_opt_bytes(reader: &mut Reader<'_>) -> Result<Option<Vec<u8>>, DecodeError> {
    match reader.u8()? {
        0 => Ok(None),
        1 => reader.bytes().map(Some),
        _ => Err(DecodeError::InvalidFlag),
    }
}

fn put_f64(writer: &mut Writer, value: f64) {
    writer.u64(value.to_bits());
}

/// A finite number (positions and scales are never NaN or infinite).
fn get_f64(reader: &mut Reader<'_>) -> Result<f64, DecodeError> {
    let value = f64::from_bits(reader.u64()?);
    if value.is_finite() {
        Ok(value)
    } else {
        Err(DecodeError::OutOfRange)
    }
}

fn put_surface(writer: &mut Writer, surface: &Surface) {
    writer.u32(surface.width);
    writer.u32(surface.height);
    writer.bytes(&surface.pixels);
    writer.u32(u32::try_from(surface.layers.len()).unwrap_or(u32::MAX));
    for layer in &surface.layers {
        writer.u32(layer.handle);
        writer.u32(layer.x);
        writer.u32(layer.y);
        writer.u32(layer.width);
        writer.u32(layer.height);
        writer.bytes(&layer.coverage);
    }
}

fn get_surface(reader: &mut Reader<'_>) -> Result<Surface, DecodeError> {
    let width = reader.u32()?;
    let height = reader.u32()?;
    let pixels = reader.bytes()?;
    let count = reader.u32()? as usize;
    if count > MAX_FRAMES {
        return Err(DecodeError::OutOfRange);
    }
    let layers = (0..count)
        .map(|_| {
            Ok(FrameLayer {
                handle: reader.u32()?,
                x: reader.u32()?,
                y: reader.u32()?,
                width: reader.u32()?,
                height: reader.u32()?,
                coverage: reader.bytes()?,
            })
        })
        .collect::<Result<_, DecodeError>>()?;
    Ok(Surface {
        width,
        height,
        pixels,
        layers,
    })
}

fn put_page_input(writer: &mut Writer, input: &PageInput) {
    match input {
        PageInput::Frame(ask) => {
            writer.u8(0);
            writer.u32(ask.width);
            writer.u32(ask.height);
            put_f64(writer, ask.scale);
            put_f64(writer, ask.zoom);
            match ask.scroll {
                Some((x, y)) => {
                    writer.u8(1);
                    put_f64(writer, x);
                    put_f64(writer, y);
                }
                None => writer.u8(0),
            }
            writer.u64(ask.elapsed_us);
            match &ask.find {
                Some((text, index)) => {
                    writer.u8(1);
                    writer.str(text);
                    writer.u32(*index);
                }
                None => writer.u8(0),
            }
        }
        PageInput::Click { x, y, modifiers } => {
            writer.u8(1);
            put_f64(writer, *x);
            put_f64(writer, *y);
            writer.u8(*modifiers);
        }
        PageInput::Key {
            down,
            key,
            code,
            repeat,
            modifiers,
        } => {
            writer.u8(2);
            writer.bool(*down);
            writer.str(key);
            writer.str(code);
            writer.bool(*repeat);
            writer.u8(*modifiers);
        }
        PageInput::Composition { phase, text } => {
            writer.u8(3);
            writer.u8(match phase {
                CompositionPhase::Start => 0,
                CompositionPhase::Update => 1,
                CompositionPhase::End => 2,
            });
            writer.str(text);
        }
        PageInput::Assistive { node, request } => {
            writer.u8(4);
            writer.u64(*node);
            match request {
                AssistiveRequest::Focus => writer.u8(0),
                AssistiveRequest::Click => writer.u8(1),
                AssistiveRequest::SetValue(text) => {
                    writer.u8(2);
                    writer.str(text);
                }
                AssistiveRequest::SetSelection { anchor, focus } => {
                    writer.u8(3);
                    writer.u32(*anchor);
                    writer.u32(*focus);
                }
            }
        }
        PageInput::Accessibility => writer.u8(5),
    }
}

fn get_page_input(reader: &mut Reader<'_>) -> Result<PageInput, DecodeError> {
    let short = |value: String| {
        if value.len() <= MAX_KEY_BYTES {
            Ok(value)
        } else {
            Err(DecodeError::OutOfRange)
        }
    };
    let text = |value: String| {
        if value.len() <= MAX_INPUT_TEXT_BYTES {
            Ok(value)
        } else {
            Err(DecodeError::OutOfRange)
        }
    };
    Ok(match reader.u8()? {
        0 => {
            let width = reader.u32()?;
            let height = reader.u32()?;
            let scale = get_f64(reader)?;
            let zoom = get_f64(reader)?;
            let scroll = match reader.u8()? {
                0 => None,
                1 => Some((get_f64(reader)?, get_f64(reader)?)),
                _ => return Err(DecodeError::InvalidFlag),
            };
            if !(1..=16_384).contains(&width)
                || !(1..=16_384).contains(&height)
                || !(0.25..=8.0).contains(&scale)
                || !(0.25..=5.0).contains(&zoom)
            {
                return Err(DecodeError::OutOfRange);
            }
            PageInput::Frame(FrameAsk {
                width,
                height,
                scale,
                zoom,
                scroll,
                elapsed_us: reader.u64()?,
                find: match reader.u8()? {
                    0 => None,
                    1 => {
                        let text = reader.string()?;
                        if text.is_empty() || text.len() > MAX_FIND_BYTES {
                            return Err(DecodeError::OutOfRange);
                        }
                        Some((text, reader.u32()?))
                    }
                    _ => return Err(DecodeError::InvalidFlag),
                },
            })
        }
        1 => PageInput::Click {
            x: get_f64(reader)?,
            y: get_f64(reader)?,
            modifiers: reader.u8()?,
        },
        2 => PageInput::Key {
            down: reader.bool()?,
            key: short(reader.string()?)?,
            code: short(reader.string()?)?,
            repeat: reader.bool()?,
            modifiers: reader.u8()?,
        },
        3 => {
            let phase = match reader.u8()? {
                0 => CompositionPhase::Start,
                1 => CompositionPhase::Update,
                2 => CompositionPhase::End,
                tag => return Err(DecodeError::UnknownTag(u16::from(tag))),
            };
            PageInput::Composition {
                phase,
                text: text(reader.string()?)?,
            }
        }
        4 => {
            let node = reader.u64()?;
            let request = match reader.u8()? {
                0 => AssistiveRequest::Focus,
                1 => AssistiveRequest::Click,
                2 => AssistiveRequest::SetValue(text(reader.string()?)?),
                3 => AssistiveRequest::SetSelection {
                    anchor: reader.u32()?,
                    focus: reader.u32()?,
                },
                tag => return Err(DecodeError::UnknownTag(u16::from(tag))),
            };
            PageInput::Assistive { node, request }
        }
        5 => PageInput::Accessibility,
        tag => return Err(DecodeError::UnknownTag(u16::from(tag))),
    })
}

impl Message for BrokerMessage {
    fn encode(&self, writer: &mut Writer) {
        match self {
            Self::Prefetched { id } => {
                writer.u16(8);
                writer.u32(*id);
            }
            Self::DocumentChunk { id, result } => {
                writer.u16(9);
                writer.u32(*id);
                match result {
                    Ok(Some(bytes)) => {
                        writer.u8(0);
                        writer.bytes(bytes);
                    }
                    Ok(None) => writer.u8(1),
                    Err(DocumentFailure::TooLarge) => writer.u8(2),
                    Err(DocumentFailure::Failed(why)) => {
                        writer.u8(3);
                        writer.str(why);
                    }
                }
            }
            Self::Start(start) => {
                writer.u16(1);
                writer.str(&start.command);
                writer.strs(&start.args);
                let document = &start.document;
                writer.str(&document.url);
                writer.bytes(&document.bytes);
                writer.opt_str(document.transport_charset.as_deref());
                writer.bool(document.local);
                writer.strs(&document.csp);
                writer.strs(&document.csp_report_only);
                writer.str(&document.referrer_policy);
                writer.bool(document.storage);
                writer.bool(document.streamed);
                writer.u32(u32::try_from(start.fonts.len()).unwrap_or(u32::MAX));
                for font in &start.fonts {
                    writer.bytes(font);
                }
                writer.strs(&start.families);
            }
            Self::Resource { id, result } => {
                writer.u16(2);
                writer.u32(*id);
                match result {
                    Ok(body) => {
                        writer.u8(0);
                        writer.bytes(&body.bytes);
                        writer.opt_str(body.charset.as_deref());
                    }
                    Err(failure) => writer.u8(failure_tag(*failure)),
                }
            }
            Self::Fetch { id, result } => {
                writer.u16(3);
                writer.u32(*id);
                match result {
                    None => writer.u8(0),
                    Some(response) => {
                        writer.u8(1);
                        writer.str(&response.kind);
                        writer.opt_str(response.url.as_deref());
                        writer.bool(response.redirected);
                        writer.u16(response.status);
                        writer.bytes(&response.status_text);
                        put_headers(writer, &response.headers);
                        writer.bytes(&response.body);
                        match response.stream {
                            Some(stream) => {
                                writer.u8(1);
                                writer.u64(stream);
                            }
                            None => writer.u8(0),
                        }
                    }
                }
            }
            Self::FetchBody { id, result } => {
                writer.u16(10);
                writer.u32(*id);
                match result {
                    Ok(None) => writer.u8(0),
                    Ok(Some(chunk)) => {
                        writer.u8(1);
                        writer.bytes(chunk);
                    }
                    Err(failure) => {
                        writer.u8(2);
                        writer.u8(failure_tag(*failure));
                    }
                }
            }
            Self::Storage { id, value } => {
                writer.u16(4);
                writer.u32(*id);
                match value {
                    StorageValue::Count(count) => {
                        writer.u8(0);
                        writer.u32(*count);
                    }
                    StorageValue::Units(units) => {
                        writer.u8(1);
                        match units {
                            Some(units) => {
                                writer.u8(1);
                                put_units(writer, units);
                            }
                            None => writer.u8(0),
                        }
                    }
                    StorageValue::Keys(keys) => {
                        writer.u8(2);
                        writer.u32(u32::try_from(keys.len()).unwrap_or(u32::MAX));
                        for key in keys {
                            put_units(writer, key);
                        }
                    }
                    StorageValue::Done => writer.u8(3),
                    StorageValue::QuotaExceeded => writer.u8(4),
                    StorageValue::Refused => writer.u8(5),
                }
            }
            Self::Frame { id, result } => {
                writer.u16(5);
                writer.u32(*id);
                match result {
                    Ok(handle) => {
                        writer.u8(0);
                        writer.u32(*handle);
                    }
                    Err(failure) => writer.u8(failure_tag(*failure)),
                }
            }
            Self::Popup { id, opened } => {
                writer.u16(6);
                writer.u32(*id);
                writer.bool(*opened);
            }
            Self::Page(input) => {
                writer.u16(7);
                put_page_input(writer, input);
            }
            Self::AutomationCommand { id, line } => {
                writer.u16(32);
                writer.u32(*id);
                writer.opt_str(line.as_deref());
            }
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match reader.u16()? {
            1 => {
                let command = reader.string()?;
                let args = reader.strings(MAX_STRINGS)?;
                let document = DocumentGrant {
                    url: reader.string()?,
                    bytes: reader.bytes()?,
                    transport_charset: reader.opt_string()?,
                    local: reader.bool()?,
                    csp: reader.strings(MAX_STRINGS)?,
                    csp_report_only: reader.strings(MAX_STRINGS)?,
                    referrer_policy: reader.string()?,
                    storage: reader.bool()?,
                    streamed: reader.bool()?,
                };
                let count = reader.u32()? as usize;
                if count > MAX_FONTS {
                    return Err(DecodeError::OutOfRange);
                }
                let fonts = (0..count)
                    .map(|_| reader.bytes())
                    .collect::<Result<_, _>>()?;
                let families = reader.strings(MAX_STRINGS)?;
                Ok(Self::Start(Start {
                    command,
                    args,
                    document,
                    fonts,
                    families,
                }))
            }
            2 => {
                let id = reader.u32()?;
                let result = match reader.u8()? {
                    0 => Ok(ResourceBody {
                        bytes: reader.bytes()?,
                        charset: reader.opt_string()?,
                    }),
                    tag => Err(failure_of(tag)?),
                };
                Ok(Self::Resource { id, result })
            }
            3 => {
                let id = reader.u32()?;
                let result = match reader.u8()? {
                    0 => None,
                    1 => Some(FetchResponse {
                        kind: reader.string()?,
                        url: reader.opt_string()?,
                        redirected: reader.bool()?,
                        status: reader.u16()?,
                        status_text: reader.bytes()?,
                        headers: get_headers(reader)?,
                        body: reader.bytes()?,
                        stream: match reader.u8()? {
                            0 => None,
                            1 => Some(reader.u64()?),
                            _ => return Err(DecodeError::InvalidFlag),
                        },
                    }),
                    _ => return Err(DecodeError::InvalidFlag),
                };
                Ok(Self::Fetch { id, result })
            }
            4 => {
                let id = reader.u32()?;
                let value = match reader.u8()? {
                    0 => StorageValue::Count(reader.u32()?),
                    1 => StorageValue::Units(match reader.u8()? {
                        0 => None,
                        1 => Some(get_units(reader)?),
                        _ => return Err(DecodeError::InvalidFlag),
                    }),
                    2 => {
                        let count = reader.u32()? as usize;
                        if count > MAX_STORAGE_KEYS {
                            return Err(DecodeError::OutOfRange);
                        }
                        StorageValue::Keys(
                            (0..count)
                                .map(|_| get_units(reader))
                                .collect::<Result<_, _>>()?,
                        )
                    }
                    3 => StorageValue::Done,
                    4 => StorageValue::QuotaExceeded,
                    5 => StorageValue::Refused,
                    tag => return Err(DecodeError::UnknownTag(u16::from(tag))),
                };
                Ok(Self::Storage { id, value })
            }
            5 => {
                let id = reader.u32()?;
                let result = match reader.u8()? {
                    0 => Ok(reader.u32()?),
                    tag => Err(failure_of(tag)?),
                };
                Ok(Self::Frame { id, result })
            }
            6 => Ok(Self::Popup {
                id: reader.u32()?,
                opened: reader.bool()?,
            }),
            7 => Ok(Self::Page(get_page_input(reader)?)),
            8 => Ok(Self::Prefetched { id: reader.u32()? }),
            9 => {
                let id = reader.u32()?;
                let result = match reader.u8()? {
                    0 => Ok(Some(reader.bytes()?)),
                    1 => Ok(None),
                    2 => Err(DocumentFailure::TooLarge),
                    3 => Err(DocumentFailure::Failed(reader.string()?)),
                    tag => return Err(DecodeError::UnknownTag(u16::from(tag))),
                };
                Ok(Self::DocumentChunk { id, result })
            }
            32 => Ok(Self::AutomationCommand {
                id: reader.u32()?,
                line: reader.opt_string()?,
            }),
            10 => {
                let id = reader.u32()?;
                let result = match reader.u8()? {
                    0 => Ok(None),
                    1 => Ok(Some(reader.bytes()?)),
                    2 => Err(failure_of(reader.u8()?)?),
                    _ => return Err(DecodeError::InvalidFlag),
                };
                Ok(Self::FetchBody { id, result })
            }
            tag => Err(DecodeError::UnknownTag(tag)),
        }
    }
}

fn failure_tag(failure: ResourceFailure) -> u8 {
    match failure {
        ResourceFailure::Blocked => 1,
        ResourceFailure::NotFound => 2,
        ResourceFailure::TooLarge => 3,
        ResourceFailure::Failed => 4,
        ResourceFailure::Cancelled => 5,
    }
}

fn failure_of(tag: u8) -> Result<ResourceFailure, DecodeError> {
    Ok(match tag {
        1 => ResourceFailure::Blocked,
        2 => ResourceFailure::NotFound,
        3 => ResourceFailure::TooLarge,
        4 => ResourceFailure::Failed,
        5 => ResourceFailure::Cancelled,
        tag => return Err(DecodeError::UnknownTag(u16::from(tag))),
    })
}

/// Most requests in one [`RendererMessage::Prefetch`].
pub const MAX_PREFETCH_REQUESTS: usize = 4096;

fn put_resource_kind(writer: &mut Writer, kind: &ResourceKind) {
    match kind {
        ResourceKind::Stylesheet { quirks } => {
            writer.u8(0);
            writer.bool(*quirks);
        }
        ResourceKind::Image => writer.u8(1),
        ResourceKind::Script {
            nonce,
            parser_inserted,
            module,
        } => {
            writer.u8(2);
            writer.opt_str(nonce.as_deref());
            writer.bool(*parser_inserted);
            writer.bool(*module);
        }
    }
}

fn get_resource_kind(reader: &mut Reader<'_>) -> Result<ResourceKind, DecodeError> {
    Ok(match reader.u8()? {
        0 => ResourceKind::Stylesheet {
            quirks: reader.bool()?,
        },
        1 => ResourceKind::Image,
        2 => ResourceKind::Script {
            nonce: reader.opt_string()?,
            parser_inserted: reader.bool()?,
            module: reader.bool()?,
        },
        tag => return Err(DecodeError::UnknownTag(u16::from(tag))),
    })
}

impl Message for RendererMessage {
    fn encode(&self, writer: &mut Writer) {
        match self {
            Self::DocumentChunk { id } => {
                writer.u16(13);
                writer.u32(*id);
            }
            Self::Prefetch { id, requests } => {
                writer.u16(12);
                writer.u32(*id);
                writer.u32(u32::try_from(requests.len()).unwrap_or(u32::MAX));
                for (url, kind) in requests {
                    writer.str(url);
                    put_resource_kind(writer, kind);
                }
            }
            Self::Resource {
                id,
                url,
                kind,
                max_bytes,
            } => {
                writer.u16(1);
                writer.u32(*id);
                writer.str(url);
                put_resource_kind(writer, kind);
                writer.u64(*max_bytes);
            }
            Self::Fetch { id, request } => {
                writer.u16(2);
                writer.u32(*id);
                writer.str(&request.method);
                writer.str(&request.url);
                put_headers(writer, &request.headers);
                put_opt_bytes(writer, request.body.as_deref());
                writer.str(&request.mode);
                writer.str(&request.credentials);
                writer.str(&request.redirect);
                writer.str(&request.referrer_policy);
            }
            Self::FetchBody { id, stream, close } => {
                writer.u16(14);
                writer.u32(*id);
                writer.u64(*stream);
                writer.bool(*close);
            }
            Self::Storage { id, area, op } => {
                writer.u16(3);
                writer.u32(*id);
                writer.u8(match area {
                    StorageArea::Local => 0,
                    StorageArea::Session => 1,
                });
                match op {
                    StorageOp::Length => writer.u8(0),
                    StorageOp::Key(index) => {
                        writer.u8(1);
                        writer.u32(*index);
                    }
                    StorageOp::Get(key) => {
                        writer.u8(2);
                        put_units(writer, key);
                    }
                    StorageOp::Set(key, value) => {
                        writer.u8(3);
                        put_units(writer, key);
                        put_units(writer, value);
                    }
                    StorageOp::Remove(key) => {
                        writer.u8(4);
                        put_units(writer, key);
                    }
                    StorageOp::Clear => writer.u8(5),
                    StorageOp::Keys => writer.u8(6),
                }
            }
            Self::Diagnostic(text) => {
                writer.u16(4);
                writer.str(text);
            }
            Self::Done { status, output } => {
                writer.u16(5);
                writer.u8(*status);
                writer.bytes(output);
            }
            Self::Frame { id, request } => {
                writer.u16(6);
                writer.u32(*id);
                writer.str(&request.url);
                writer.opt_str(request.srcdoc.as_deref());
                match &request.sandbox {
                    Some(tokens) => {
                        writer.u8(1);
                        writer.strs(tokens);
                    }
                    None => writer.u8(0),
                }
                writer.u32(request.width);
                writer.u32(request.height);
            }
            Self::Surface(surface) => {
                writer.u16(7);
                put_surface(writer, surface);
            }
            Self::Handled => writer.u16(10),
            Self::Accessibility(tree) => {
                writer.u16(11);
                writer.bytes(tree);
            }
            Self::Painted(painted) => {
                writer.u16(9);
                put_surface(writer, &painted.surface);
                put_f64(writer, painted.scroll.0);
                put_f64(writer, painted.scroll.1);
                match painted.next_frame_us {
                    Some(us) => {
                        writer.u8(1);
                        writer.u64(us);
                    }
                    None => writer.u8(0),
                }
                match painted.found {
                    Some((count, active, top, bottom)) => {
                        writer.u8(1);
                        writer.u32(count);
                        writer.u32(active);
                        put_f64(writer, top);
                        put_f64(writer, bottom);
                    }
                    None => writer.u8(0),
                }
                match painted.editing {
                    Some((x, y, width, height)) => {
                        writer.u8(1);
                        for value in [x, y, width, height] {
                            put_f64(writer, value);
                        }
                    }
                    None => writer.u8(0),
                }
            }
            Self::Popup {
                id,
                url,
                noopener,
                noreferrer,
            } => {
                writer.u16(8);
                writer.u32(*id);
                writer.str(url);
                writer.bool(*noopener);
                writer.bool(*noreferrer);
            }
            Self::AutomationOutput(line) => {
                writer.u16(32);
                writer.str(line);
            }
            Self::AutomationRead { id } => {
                writer.u16(33);
                writer.u32(*id);
            }
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match reader.u16()? {
            1 => {
                let id = reader.u32()?;
                let url = reader.string()?;
                let kind = get_resource_kind(reader)?;
                let max_bytes = reader.u64()?;
                Ok(Self::Resource {
                    id,
                    url,
                    kind,
                    max_bytes,
                })
            }
            2 => {
                let id = reader.u32()?;
                let request = FetchRequest {
                    method: reader.string()?,
                    url: reader.string()?,
                    headers: get_headers(reader)?,
                    body: get_opt_bytes(reader)?,
                    mode: reader.string()?,
                    credentials: reader.string()?,
                    redirect: reader.string()?,
                    referrer_policy: reader.string()?,
                };
                Ok(Self::Fetch { id, request })
            }
            3 => {
                let id = reader.u32()?;
                let area = match reader.u8()? {
                    0 => StorageArea::Local,
                    1 => StorageArea::Session,
                    tag => return Err(DecodeError::UnknownTag(u16::from(tag))),
                };
                let op = match reader.u8()? {
                    0 => StorageOp::Length,
                    1 => StorageOp::Key(reader.u32()?),
                    2 => StorageOp::Get(get_units(reader)?),
                    3 => StorageOp::Set(get_units(reader)?, get_units(reader)?),
                    4 => StorageOp::Remove(get_units(reader)?),
                    5 => StorageOp::Clear,
                    6 => StorageOp::Keys,
                    tag => return Err(DecodeError::UnknownTag(u16::from(tag))),
                };
                Ok(Self::Storage { id, area, op })
            }
            4 => Ok(Self::Diagnostic(reader.string()?)),
            5 => Ok(Self::Done {
                status: reader.u8()?,
                output: reader.bytes()?,
            }),
            6 => {
                let id = reader.u32()?;
                let request = FrameRequest {
                    url: reader.string()?,
                    srcdoc: reader.opt_string()?,
                    sandbox: match reader.u8()? {
                        0 => None,
                        1 => Some(reader.strings(MAX_STRINGS)?),
                        _ => return Err(DecodeError::InvalidFlag),
                    },
                    width: reader.u32()?,
                    height: reader.u32()?,
                };
                Ok(Self::Frame { id, request })
            }
            7 => Ok(Self::Surface(get_surface(reader)?)),
            10 => Ok(Self::Handled),
            11 => {
                let tree = reader.bytes()?;
                if tree.len() > MAX_ACCESSIBILITY_BYTES {
                    return Err(DecodeError::OutOfRange);
                }
                Ok(Self::Accessibility(tree))
            }
            9 => Ok(Self::Painted(Painted {
                surface: get_surface(reader)?,
                scroll: (get_f64(reader)?, get_f64(reader)?),
                next_frame_us: match reader.u8()? {
                    0 => None,
                    1 => Some(reader.u64()?),
                    _ => return Err(DecodeError::InvalidFlag),
                },
                found: match reader.u8()? {
                    0 => None,
                    1 => Some((
                        reader.u32()?,
                        reader.u32()?,
                        get_f64(reader)?,
                        get_f64(reader)?,
                    )),
                    _ => return Err(DecodeError::InvalidFlag),
                },
                editing: match reader.u8()? {
                    0 => None,
                    1 => Some((
                        get_f64(reader)?,
                        get_f64(reader)?,
                        get_f64(reader)?,
                        get_f64(reader)?,
                    )),
                    _ => return Err(DecodeError::InvalidFlag),
                },
            })),
            8 => Ok(Self::Popup {
                id: reader.u32()?,
                url: reader.string()?,
                noopener: reader.bool()?,
                noreferrer: reader.bool()?,
            }),
            12 => {
                let id = reader.u32()?;
                let count = reader.u32()? as usize;
                if count > MAX_PREFETCH_REQUESTS {
                    return Err(DecodeError::OutOfRange);
                }
                let mut requests = Vec::with_capacity(count);
                for _ in 0..count {
                    let url = reader.string()?;
                    requests.push((url, get_resource_kind(reader)?));
                }
                Ok(Self::Prefetch { id, requests })
            }
            13 => Ok(Self::DocumentChunk { id: reader.u32()? }),
            32 => Ok(Self::AutomationOutput(reader.string()?)),
            33 => Ok(Self::AutomationRead { id: reader.u32()? }),
            14 => Ok(Self::FetchBody {
                id: reader.u32()?,
                stream: reader.u64()?,
                close: reader.bool()?,
            }),
            tag => Err(DecodeError::UnknownTag(tag)),
        }
    }
}
