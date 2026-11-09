//! Local `<img src>` loading for layout and paint.
//!
//! Each HTML `img` element that generated a box and has a non-empty `src`
//! is resolved against the document base URL (the first `<base href>`, else
//! the document's `file:` URL) and read only if it names a file in the
//! document's directory or below it, under the same policy as linked
//! stylesheets. Files are read in bounded chunks and decoded as PNG within
//! the remaining budgets. An image that is blocked, missing, too large or
//! not a valid PNG is broken: it keeps a zero natural size and paints no
//! content, and the load continues. One image pixel is one CSS pixel.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;

use rustic_base::CancellationToken;
use rustic_dom::{Document, NodeId};
use rustic_layout::{BoxTree, CssPx, NaturalSize};
use rustic_paint::Image;
use rustic_style::DomElement;
use rustic_url::Url;

use crate::reftest::{attribute, is_html, resolve};
use crate::stylesheets::file_url;

/// Budgets for the images of one document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageLimits {
    /// Images read; later ones are broken.
    pub max_images: usize,
    /// Total encoded bytes read across all images.
    pub max_file_bytes: usize,
    /// Total decoded RGBA bytes across all images.
    pub max_decoded_bytes: usize,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            max_images: 256,
            max_file_bytes: 32 * 1024 * 1024,
            max_decoded_bytes: 256 * 1024 * 1024,
        }
    }
}

/// Decoded images and the natural sizes layout needs, by element.
#[derive(Debug, Default)]
pub struct LoadedImages {
    /// Decoded content for paint.
    pub images: HashMap<NodeId, Arc<Image>>,
    /// Natural sizes to apply with [`BoxTree::set_natural_sizes`].
    pub sizes: Vec<(NodeId, NaturalSize)>,
}

/// Read at most `max` bytes of `path` in `chunk`-sized reads; `None` if the
/// file is unreadable, not a regular file or longer than `max`.
fn read_bounded(path: &Path, max: usize, chunk: usize) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > max as u64 {
        return None;
    }
    let mut buffer = vec![0; chunk.clamp(1, max.max(1))];
    let mut bytes = Vec::new();
    loop {
        match file.read(&mut buffer) {
            Ok(0) => return Some(bytes),
            Ok(n) if bytes.len() + n <= max => bytes.extend_from_slice(&buffer[..n]),
            Ok(_) => return None,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
}

/// Load the images of `document`, loaded from the canonical file `path`,
/// for the replaced boxes in `tree`.
pub fn load(
    document: &Document,
    tree: &BoxTree,
    path: &Path,
    limits: ImageLimits,
    chunk_bytes: usize,
    cancellation: &CancellationToken,
) -> LoadedImages {
    let mut loaded = LoadedImages::default();
    let replaced: HashMap<NodeId, ()> = tree
        .boxes()
        .iter()
        .filter(|b| b.natural_size().is_some())
        .map(|b| (b.node(), ()))
        .collect();
    let document_url = file_url(path);
    let root = path.parent();
    let Ok(walk) = document.walk(document.root()) else {
        return loaded;
    };
    let mut base: Option<Option<Url>> = None;
    let mut sources = Vec::new();
    for (id, _) in walk {
        let Some(element) = DomElement::new(document, id) else {
            continue;
        };
        if is_html(element, "base") && base.is_none() {
            if let Some(href) = attribute(element, "href") {
                base = Some(Url::parse(&href, document_url.as_ref()).ok());
            }
        } else if is_html(element, "img") && replaced.contains_key(&id) {
            if let Some(src) = attribute(element, "src").filter(|s| !s.is_empty()) {
                sources.push((id, src));
            }
        }
    }
    let base = base.flatten().or(document_url);
    let (mut files, mut file_bytes, mut decoded_bytes) = (0usize, 0usize, 0usize);
    for (node, src) in sources {
        if cancellation.check().is_err() || files >= limits.max_images {
            break;
        }
        let Ok(file) = resolve(&src, base.as_ref(), root) else {
            continue;
        };
        files += 1;
        let Some(bytes) = read_bounded(&file, limits.max_file_bytes - file_bytes, chunk_bytes)
        else {
            continue;
        };
        file_bytes += bytes.len();
        let png_limits = rustic_codecs::PngLimits {
            max_bytes: limits.max_decoded_bytes - decoded_bytes,
            ..rustic_codecs::PngLimits::default()
        };
        let Ok(decoded) = rustic_codecs::decode_png(&bytes, png_limits, cancellation) else {
            continue;
        };
        let size = (
            CssPx::new(f64::from(decoded.width)),
            CssPx::new(f64::from(decoded.height)),
        );
        let (width, height) = (decoded.width, decoded.height);
        let Some(image) = Image::new(width, height, decoded.pixels) else {
            continue;
        };
        let (Ok(w), Ok(h)) = size else {
            continue;
        };
        decoded_bytes += image.pixels().len();
        loaded
            .sizes
            .push((node, NaturalSize::from_dimensions(w, h)));
        loaded.images.insert(node, Arc::new(image));
    }
    loaded
}
