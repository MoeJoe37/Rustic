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

/// What a resource request is for (decides the broker's policy checks).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceKind {
    /// A stylesheet or `@import`.
    Stylesheet,
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
    /// The visible body.
    pub body: Vec<u8>,
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

/// Broker to renderer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BrokerMessage {
    /// The job.
    Start(Start),
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
}

/// Renderer to broker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RendererMessage {
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

impl Message for BrokerMessage {
    fn encode(&self, writer: &mut Writer) {
        match self {
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
                writer.u32(u32::try_from(start.fonts.len()).unwrap_or(u32::MAX));
                for font in &start.fonts {
                    writer.bytes(font);
                }
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
                };
                let count = reader.u32()? as usize;
                if count > MAX_FONTS {
                    return Err(DecodeError::OutOfRange);
                }
                let fonts = (0..count)
                    .map(|_| reader.bytes())
                    .collect::<Result<_, _>>()?;
                Ok(Self::Start(Start {
                    command,
                    args,
                    document,
                    fonts,
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

impl Message for RendererMessage {
    fn encode(&self, writer: &mut Writer) {
        match self {
            Self::Resource {
                id,
                url,
                kind,
                max_bytes,
            } => {
                writer.u16(1);
                writer.u32(*id);
                writer.str(url);
                match kind {
                    ResourceKind::Stylesheet => writer.u8(0),
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
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        match reader.u16()? {
            1 => {
                let id = reader.u32()?;
                let url = reader.string()?;
                let kind = match reader.u8()? {
                    0 => ResourceKind::Stylesheet,
                    1 => ResourceKind::Image,
                    2 => ResourceKind::Script {
                        nonce: reader.opt_string()?,
                        parser_inserted: reader.bool()?,
                        module: reader.bool()?,
                    },
                    tag => return Err(DecodeError::UnknownTag(u16::from(tag))),
                };
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
            7 => {
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
                Ok(Self::Surface(Surface {
                    width,
                    height,
                    pixels,
                    layers,
                }))
            }
            tag => Err(DecodeError::UnknownTag(tag)),
        }
    }
}
