//! Local `<img src>` loading for layout and paint.
//!
//! Each HTML `img` element that generated a box and has a non-empty `src`
//! is resolved against the document base URL (the first `<base href>`, else
//! the document's `file:` URL) and read only if it names a file in the
//! document's directory or below it, under the same policy as linked
//! stylesheets. Files are read in bounded chunks into a per-document
//! [`ImageStore`], which shares one entry between elements naming the same
//! file, reads natural sizes from headers and decodes PNG, JPEG, GIF and
//! WebP within the decoded-byte budget. Animated images show their first
//! frame (TD-82). An image that is blocked, missing, too large or not a
//! valid image is broken: it keeps a zero natural size and paints no
//! content, and the load continues. One image pixel is one CSS pixel.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;

use rustic_base::CancellationToken;
use rustic_codecs::{ImageError, ImageStore, StoreError, StoreLimits};
use rustic_dom::{Document, NodeId};
use rustic_layout::{BoxTree, CssPx, NaturalSize};
use rustic_paint::Image;
use rustic_style::{DomElement, FetchError};
use rustic_url::Url;

use crate::reftest::{attribute, is_html, resolve};
use crate::stylesheets::{AppliedLocalFiles, LocalStylesheetFetcher, StagedLocalFile, file_url};

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

/// How one document's images are read: the chunk size and the view that
/// selects among `srcset` candidates.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ImageRead<'a> {
    /// Bytes per file read.
    pub(crate) chunk_bytes: usize,
    /// The view source selection uses.
    pub(crate) view: crate::srcset::ViewInfo,
    /// Lazy images not loaded yet (HTML §2.5.7).
    pub(crate) deferred: &'a std::collections::HashSet<NodeId>,
}

/// Decoded images and the natural sizes layout needs, by element.
#[derive(Debug, Default)]
pub struct LoadedImages {
    /// Decoded content for paint.
    pub images: HashMap<NodeId, Arc<Image>>,
    /// Natural sizes to apply with [`BoxTree::set_natural_sizes`].
    pub sizes: Vec<(NodeId, NaturalSize)>,
    /// Each requested `img` element's selected absolute URL, decoded or
    /// not (`currentSrc`).
    pub sources: HashMap<NodeId, String>,
    /// Computed styles of SVG content for paint.
    pub svg_styles: HashMap<NodeId, Arc<rustic_style::ComputedStyle>>,
}

/// Read at most `max` bytes of `path` in `chunk`-sized reads; `None` if the
/// file is unreadable, not a regular file or longer than `max`.
pub(crate) fn read_bounded(path: &Path, max: usize, chunk: usize) -> Option<Vec<u8>> {
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

/// Read and validate an image completion *before* it is made visible to
/// layout or paint. A failed decode leaves both the active image and its
/// rendering-input revision untouched. The decoder uses the same budgets as
/// an ordinary image load, rather than trusting only headers or encoded size.
pub(crate) fn stage_image_completion(
    fetcher: &LocalStylesheetFetcher,
    applied: &AppliedLocalFiles,
    url: &Url,
    limits: ImageLimits,
    cancellation: &CancellationToken,
) -> Result<StagedLocalFile, FetchError> {
    if limits.max_images == 0 {
        return Err(FetchError::TooLarge);
    }
    let staged = fetcher.stage_completion_as(
        applied,
        url,
        limits.max_file_bytes,
        &crate::network::ResourceKind::Image,
    )?;
    let mut store = ImageStore::new(StoreLimits {
        max_entries: 1,
        max_encoded_bytes: limits.max_file_bytes,
        max_decoded_bytes: limits.max_decoded_bytes,
        ..StoreLimits::default()
    });
    let image = store
        .insert("staged-resource", staged.bytes().to_vec())
        .map_err(map_staged_image_error)?;
    let animation = store
        .decode(image, cancellation)
        .map_err(map_staged_image_error)?;
    let first = animation.frames.first().ok_or(FetchError::Failed)?;
    if first.image.pixels.len() > limits.max_decoded_bytes {
        return Err(FetchError::TooLarge);
    }
    Ok(staged)
}

fn map_staged_image_error(error: StoreError) -> FetchError {
    match error {
        StoreError::Full | StoreError::Broken(ImageError::Limit) => FetchError::TooLarge,
        StoreError::Broken(ImageError::Cancelled) => FetchError::Cancelled,
        StoreError::Unknown | StoreError::Broken(_) => FetchError::Failed,
    }
}

/// How far outside the viewport a `loading=lazy` image or iframe still
/// loads, in CSS pixels on every side (the HTML "lazy load root margin" is
/// UA-defined; this is the margin Chromium uses on fast connections).
pub(crate) const LAZY_LOAD_MARGIN: f64 = 1250.0;

/// Whether `element` is in the Lazy loading state (HTML §2.5.7): its
/// `loading` attribute is `lazy`, ASCII case-insensitively.
pub(crate) fn is_lazy(element: DomElement<'_>) -> bool {
    attribute(element, "loading").is_some_and(|value| value.eq_ignore_ascii_case("lazy"))
}

/// The `img` elements of `document` with a box in `tree` that load lazily.
pub(crate) fn lazy_images(
    document: &Document,
    tree: &BoxTree,
) -> std::collections::HashSet<NodeId> {
    tree.boxes()
        .iter()
        .filter(|b| b.natural_size().is_some())
        .map(|b| b.node())
        .filter(|&node| {
            DomElement::new(document, node)
                .is_some_and(|element| is_html(element, "img") && is_lazy(element))
        })
        .collect()
}

/// Load images against a scripted document's applied byte snapshot. Other
/// callers retain the reference command's direct local-file behavior.
pub(crate) fn load_applied(
    document: &Document,
    tree: &BoxTree,
    path: &Path,
    limits: ImageLimits,
    read: ImageRead<'_>,
    cancellation: &CancellationToken,
    applied: Option<(&AppliedLocalFiles, Option<&dyn crate::network::Session>)>,
) -> LoadedImages {
    let ImageRead {
        chunk_bytes,
        view,
        deferred,
    } = read;
    // A network document always reads through its applied snapshot.
    let network = applied.and_then(|(_, network)| network);
    let applied = applied.map(|(applied, _)| applied);
    let mut loaded = LoadedImages::default();
    let replaced: HashMap<NodeId, ()> = tree
        .boxes()
        .iter()
        .filter(|b| b.natural_size().is_some())
        .map(|b| (b.node(), ()))
        .collect();
    let document_url = match network {
        Some(session) => Some(session.document_url().clone()),
        None => file_url(path),
    };
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
        } else if is_html(element, "img") && replaced.contains_key(&id) && !deferred.contains(&id) {
            // `srcset`, `sizes` and `picture` sources choose the URL.
            if let Some(chosen) = crate::srcset::img_source(document, id, view) {
                sources.push((id, chosen.url, chosen.density));
            }
        }
    }
    let base = base.flatten().or(document_url);
    let mut store = ImageStore::new(StoreLimits {
        max_entries: limits.max_images,
        max_encoded_bytes: limits.max_file_bytes,
        max_decoded_bytes: limits.max_decoded_bytes,
        ..StoreLimits::default()
    });
    let mut decoded: HashMap<rustic_codecs::ImageId, Option<Arc<Image>>> = HashMap::new();
    let mut decoded_bytes = 0usize;
    let mut svg_images: HashMap<String, Option<Arc<Image>>> = HashMap::new();
    for (node, src, density) in sources {
        if cancellation.check().is_err() {
            break;
        }
        let Ok(url) = Url::parse(&src, base.as_ref()) else {
            continue;
        };
        loaded.sources.insert(node, url.serialize());
        // Snapshot keys use the resolved URL before filesystem inspection.
        // If a previously applied file is replaced or removed, that change
        // cannot leak into a later DOM-triggered layout recomputation.
        let file = if applied.is_none() {
            let Ok(path) = resolve(&src, base.as_ref(), root) else {
                continue;
            };
            Some(path)
        } else {
            None
        };
        let key = file.as_ref().map_or_else(
            || url.serialize(),
            |file| file.to_string_lossy().into_owned(),
        );
        if let Some(image) = svg_images.get(&key) {
            if let Some(image) = image.clone() {
                let density = if density > 0.0 { density } else { 1.0 };
                if let (Ok(w), Ok(h)) = (
                    CssPx::new(f64::from(image.width()) / density),
                    CssPx::new(f64::from(image.height()) / density),
                ) {
                    loaded
                        .sizes
                        .push((node, NaturalSize::from_dimensions(w, h)));
                    loaded.images.insert(node, image);
                }
            }
            continue;
        }
        let id = match store.lookup(&key) {
            Some(id) => id,
            None => {
                let stats = store.stats();
                if stats.entries >= limits.max_images {
                    break;
                }
                let remaining = limits.max_file_bytes - stats.encoded_bytes;
                let bytes = if let Some(applied) = applied {
                    applied
                        .read(&url, remaining, || {
                            if let Some(session) = network {
                                return session.fetch(
                                    &url,
                                    &crate::network::ResourceKind::Image,
                                    remaining,
                                );
                            }
                            let file = resolve(&src, base.as_ref(), root)
                                .map_err(|_| FetchError::Blocked)?;
                            read_bounded(&file, remaining, chunk_bytes).ok_or(FetchError::Failed)
                        })
                        .ok()
                        .map(|bytes| bytes.to_vec())
                } else {
                    file.as_deref()
                        .and_then(|path| read_bounded(path, remaining, chunk_bytes))
                };
                let Some(bytes) = bytes else {
                    continue;
                };
                if crate::svg_image::sniff(&bytes) {
                    // SVG images are rendered here, once per URL.
                    let image = svg_images.entry(key.clone()).or_insert_with(|| {
                        let image = crate::svg_image::render(
                            &bytes,
                            crate::fonts::database(),
                            cancellation,
                        )?;
                        let size = image.width() as usize * image.height() as usize * 4;
                        if size > limits.max_decoded_bytes - decoded_bytes {
                            return None;
                        }
                        decoded_bytes += size;
                        Some(Arc::new(image))
                    });
                    if let Some(image) = image.clone() {
                        let density = if density > 0.0 { density } else { 1.0 };
                        if let (Ok(w), Ok(h)) = (
                            CssPx::new(f64::from(image.width()) / density),
                            CssPx::new(f64::from(image.height()) / density),
                        ) {
                            loaded
                                .sizes
                                .push((node, NaturalSize::from_dimensions(w, h)));
                            loaded.images.insert(node, image);
                        }
                    }
                    continue;
                }
                let Ok(id) = store.insert(&key, bytes) else {
                    continue;
                };
                id
            }
        };
        let image = decoded.entry(id).or_insert_with(|| {
            let animation = store.decode(id, cancellation).ok()?;
            // Paint owns the pixels from here: drop the store's copy and
            // charge the document budget instead.
            store.evict(id);
            let animation = Arc::try_unwrap(animation).unwrap_or_else(|a| (*a).clone());
            let first = animation.frames.into_iter().next()?.image;
            let bytes = first.pixels.len();
            if bytes > limits.max_decoded_bytes - decoded_bytes {
                return None;
            }
            decoded_bytes += bytes;
            Image::new(first.width, first.height, first.pixels).map(Arc::new)
        });
        let Some(image) = image.clone() else {
            continue;
        };
        // The selected density scales the natural size.
        let density = if density > 0.0 { density } else { 1.0 };
        let size = (
            CssPx::new(f64::from(image.width()) / density),
            CssPx::new(f64::from(image.height()) / density),
        );
        let (Ok(w), Ok(h)) = size else {
            continue;
        };
        loaded
            .sizes
            .push((node, NaturalSize::from_dimensions(w, h)));
        loaded.images.insert(node, image);
    }
    loaded
}
