use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

use rustic_base::EscapedText;
use rustic_engine::{Engine, Viewport};
use std::path::Path;

mod broker;
mod fonts;
mod images;
mod loader;
mod network;
mod reftest;
mod renderer;
mod storage;
mod stylesheets;

use loader::{LoadError, LoadedDocument, LoaderLimits};

const USAGE: &str = "Rustic browser engine development tools

Usage:
  rustic info
  rustic dump-dom [OPTIONS] [--] <file>
  rustic dump-style [OPTIONS] [--] <file>
  rustic dump-boxes [OPTIONS] [--] <file>
  rustic dump-formatting [OPTIONS] [--] <file>
  rustic dump-layout [OPTIONS] [--] <file>
  rustic dump-paint [OPTIONS] [--] <file>
  rustic render --output <png> [VIEW] [OPTIONS] [--] <file>
  rustic reftest [VIEW] [OPTIONS] [--] <test>
  rustic hit-test --at X,Y [--at X,Y ...] [OPTIONS] [--] <file>
  rustic --help | --version

dump-dom reads one local HTML file (UTF-8, scripting disabled by default) and
prints a deterministic DOM tree dump. dump-style loads the file the same way,
applies the HTML user-agent stylesheet, the document's <style> elements and
its <link rel=stylesheet> and @import sheets, and prints each element's
computed style for a 1280x720 viewport. Linked and imported sheets load only
from files in the document's directory or below it; network URLs and paths
outside it are blocked. URLs are rejected as the file argument unless network
origins are granted (below).

dump-boxes loads the same styles and prints a principal-box snapshot (no
anonymous/line boxes, layout positions or painting yet).
dump-formatting prints normalized static flow boxes with anonymous blocks and
split inline continuations, preserving unshaped text. Other formatting types,
positioning/floats and special/replaced elements fail explicitly.
dump-layout prints used content/padding/border rectangles for blocks with
collapsed vertical margins, plus line boxes, text fragments and inline boxes
laid out with the same fonts as dump-paint. Unsupported formatting fails explicitly.
dump-paint prints the display list for that layout: the canvas color (with
root/body background propagation), block and inline background colors, solid
borders and text glyph paths in stacking-context paint order, with overflow
clips and opacity groups, and <img> content. Layout and paint use explicit
--font files when supplied, otherwise the bundled test fonts (which map only a
few characters); <img src> loads local PNG, JPEG, GIF and WebP files from the
document's directory or below it, like stylesheets.
render rasterizes the same display list on the CPU and writes an RGBA PNG to
--output (overwriting it). VIEW options set the window: --viewport WxH is its
size in pixels at 100% zoom (default 1280x720), --scale the device pixel ratio
(default 1) and --zoom the page zoom (0.25 to 5, default 1). Zoom lays the page
out in a viewport of the window over the zoom and paints it at scale times
zoom (at most 64), so the PNG is the window size times the scale. The
PNG counts against --max-output-bytes.
reftest renders a test page and every <link rel=match|mismatch href> reference
the same way and compares the pixels, allowing the differences that
<meta name=fuzzy content=\"maxDifference=A;totalPixels=B\"> states (each a
number N meaning 0-N, or a range N-M). References load only from the test's
directory or below it. It prints one line per reference and a final PASS or
FAIL line, and exits 0 when every reference holds and 1 otherwise.
hit-test lays out the file like dump-paint and prints, for each --at point in
CSS pixels, the topmost box there in paint order (honoring overflow clips and
visibility; the root element when no box is hit), or none outside the
1280x720 viewport.

Use --run-scripts with any document command to run inline classics, local
external classics and static JavaScript modules before producing output.
The snapshot uses a manual clock for at most 256 timer turns and offers
one animation frame.

Process isolation: document commands (all but reftest) run their page in a
renderer child process that asks this process for every file, network and
storage access. --single-process runs everything in one process.
--sandbox (Linux, needs bubblewrap; or RUSTIC_SANDBOX=1) confines the
renderer: no files, no network, no other processes; it fails rather than
run unconfined. --renderer-timeout SECONDS stops a renderer that runs
longer (default 120).

Network (development prototype, loopback only): any document command accepts
an http:// or https:// URL in place of <file> when each origin it may reach is
granted with --allow-origin ORIGIN=ADDRESS (for example
--allow-origin http://a.test:8080=127.0.0.1:8080; ADDRESS must be loopback,
there is no DNS; repeat up to 64). Redirects, cookies, CORS, CSP, mixed
content and nosniff apply to the document's stylesheets, scripts and images;
other origins are blocked. --ca-file PEM trusts only the given roots instead
of the platform store; certificate errors are never bypassed. A response
without a BOM, charset or <meta> declaration decodes with the --locale TAG
fallback encoding (HTML's table; default en, windows-1252); a declaration
found after the prescanned prefix reloads the document once in it.

Limits for all dump commands (positive integers; exceeding any limit except
--max-diagnostics fails the load):
  --font FILE            explicit TTF/OTF/WOFF/WOFF2 (repeat up to 64; bounded local files)
  --max-input-bytes N    file and tokenizer input bytes (default 8388608)
  --max-output-bytes N   dump output bytes (default 67108864)
  --chunk-bytes N        bytes read per step (default 65536)
  --max-nodes N          document nodes, including the root (default 100000);
                         a larger page is truncated, with a warning
  --max-depth N          document tree depth (default 256); deeper elements
                         become siblings, as in other browsers
  --max-diagnostics N    retained tokenizer and tree recoveries, each; later
                         recoveries are counted, not fatal (default 1024)

Normalization options for dump-formatting (exceeding one fails):
  --max-format-boxes N      allocated boxes, including temporary (default 100000)
  --max-format-depth N      formatting tree edges from a root (default 512)
  --max-format-steps N      visits, sequence entries and text units (default 16000000)

Stylesheet options for styled dump commands (exceeding one skips the sheet, never fatal):
  --max-stylesheets N       linked and imported sheets fetched (default 64)
  --max-stylesheet-bytes N  total bytes of those sheets (default 8388608)
  --max-import-depth N      @import nesting (default 16)

Image options for layout and paint commands (exceeding one breaks the image, never fatal):
  --max-images N                 <img> files read (default 256)
  --max-image-bytes N            total bytes of those files (default 33554432)
  --max-image-decoded-bytes N    total decoded RGBA bytes (default 268435456)";

fn main() -> ExitCode {
    if env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == renderer::RENDERER_COMMAND)
    {
        return ExitCode::from(renderer::run());
    }

    let args = match fonts::configure(env::args_os().skip(1).collect()) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("rustic: {e}");
            return ExitCode::FAILURE;
        }
    };
    let args = match network::configure(args)
        .and_then(storage::configure)
        .and_then(broker::configure)
    {
        Ok(args) => args,
        Err(e) => {
            eprintln!("rustic: {e}");
            return ExitCode::from(2);
        }
    };
    if args.is_empty() || (args.len() == 1 && args[0] == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.len() == 1 && args[0] == "--version" {
        println!("rustic {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if args.len() == 1 && args[0] == "info" {
        let viewport = Viewport::new(1280, 720).expect("built-in viewport is within limits");
        println!("Rustic {} — static renderer", env!("CARGO_PKG_VERSION"));
        println!(
            "Default viewport: {}x{}",
            viewport.width(),
            viewport.height()
        );
        println!(
            "HTML: local document parsing via dump-dom; CSS: selectors, cascade and computed style via dump-style; M3: principal boxes via dump-boxes, normalized flow boxes via dump-formatting, block reference geometry via dump-layout and block, inline and text painting via dump-paint and render, reftests via reftest, hit testing via hit-test; OpenType text shaping and CSS wrapping available; JavaScript via --run-scripts; HTTP(S) to loopback origins granted with --allow-origin; general remote browsing: not implemented"
        );
        return ExitCode::SUCCESS;
    }
    if broker::splits(&args[0]) && !broker::single_process() {
        return ExitCode::from(broker::run(&args[0].to_string_lossy(), &args[1..]));
    }
    let mut engine = Engine::new(1);
    if let Some(status) = run_command(&mut engine, &args) {
        return ExitCode::from(status);
    }
    eprintln!(
        "Unsupported command. Rendering and navigation are not implemented; see docs/ROADMAP.md."
    );
    ExitCode::from(2)
}

/// Run a document command in this process; `None` for an unknown command.
/// A renderer runs the same function on its granted document.
pub(crate) fn run_command(engine: &mut Engine, args: &[OsString]) -> Option<u8> {
    if args[0] == "dump-dom" {
        return Some(dump_command(
            engine,
            "dump-dom",
            &args[1..],
            loader::dump_dom,
        ));
    }
    if args[0] == "dump-style" {
        return Some(dump_command(
            engine,
            "dump-style",
            &args[1..],
            loader::dump_style,
        ));
    }
    if args[0] == "dump-boxes" {
        return Some(dump_command(
            engine,
            "dump-boxes",
            &args[1..],
            loader::dump_boxes,
        ));
    }
    if args[0] == "dump-layout" {
        return Some(dump_command(
            engine,
            "dump-layout",
            &args[1..],
            loader::dump_layout,
        ));
    }
    if args[0] == "dump-paint" {
        return Some(dump_command(
            engine,
            "dump-paint",
            &args[1..],
            loader::dump_paint,
        ));
    }
    if args[0] == "render" {
        return Some(render_command(engine, &args[1..]));
    }
    if args[0] == "hit-test" {
        return Some(hit_test_command(engine, &args[1..]));
    }
    if args[0] == "reftest" {
        return Some(reftest_command(engine, &args[1..]));
    }
    if args[0] == "dump-formatting" {
        return Some(dump_command(
            engine,
            "dump-formatting",
            &args[1..],
            loader::dump_formatting,
        ));
    }
    None
}

fn dump_command(
    engine: &mut Engine,
    command: &str,
    args: &[OsString],
    dump: fn(&LoadedDocument, &Path, LoaderLimits) -> Result<String, LoadError>,
) -> u8 {
    let (limits, path) = match parse_dump_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("rustic {command}: {message}\n\n{USAGE}");
            return 2;
        }
    };
    dump_command_with(engine, command, limits, &path, |parsed, path| {
        dump(parsed, path, limits)
    })
}

/// Load a document, run `dump` and write its output to stdout.
fn dump_command_with(
    engine: &mut Engine,
    command: &str,
    limits: LoaderLimits,
    path: &OsString,
    dump: impl FnOnce(&LoadedDocument, &Path) -> Result<String, LoadError>,
) -> u8 {
    match load(engine, command, limits, path, dump) {
        Ok(dump) => {
            if renderer::emit_stdout(dump.as_bytes()).is_err() {
                return 1;
            }
            0
        }
        Err(code) => code,
    }
}

/// Load a local document and run `then`, reporting errors on stderr.
fn load<T>(
    engine: &mut Engine,
    command: &str,
    limits: LoaderLimits,
    path: &std::ffi::OsStr,
    then: impl FnOnce(&LoadedDocument, &Path) -> Result<T, LoadError>,
) -> Result<T, u8> {
    // A renderer loads the document its broker granted, never the path.
    let granted = renderer::take_granted();
    let network = granted.is_none() && network::is_network_argument(path);
    if network && !network::has_grants() {
        eprintln!(
            "rustic {command}: network URLs need --allow-origin ORIGIN=ADDRESS for each origin; \
             nothing was sent"
        );
        return Err(2);
    }
    let path = if network || granted.is_some() {
        std::path::PathBuf::from(path)
    } else {
        match loader::local_path(path) {
            Ok(path) => path,
            Err(error) => {
                eprintln!("rustic {command}: {error}");
                return Err(2);
            }
        }
    };
    let loaded = if let Some(granted) = &granted {
        loader::load_session_document(engine, granted, limits)
    } else if network {
        let url = path.to_str().unwrap_or_default();
        loader::load_network_document(engine, url, limits)
    } else {
        loader::load_document(engine, &path, limits)
    };
    let result = loaded.and_then(|parsed| {
        if parsed.parsed().truncated() {
            // A page over its node or payload budget degrades visibly
            // (ADR 0046): the output covers the content before that point.
            eprintln!(
                "rustic {command}: {}: warning: document truncated at its node or size limit \
                 (--max-nodes); the rest of the page was ignored",
                EscapedText(&path.to_string_lossy())
            );
        }
        let value = then(&parsed, &path)?;
        loader::check_cancelled(parsed.cancellation())?;
        Ok(value)
    });
    match result {
        Ok(value) => Ok(value),
        Err(error @ LoadError::InvalidLimits) => {
            eprintln!("rustic {command}: {error}");
            Err(2)
        }
        Err(error) => {
            // The path is user input: escape it so control characters in a file
            // name cannot forge terminal output or extra log lines.
            eprintln!(
                "rustic {command}: {}: {error}",
                EscapedText(&path.to_string_lossy())
            );
            Err(1)
        }
    }
}

/// Arguments of the rendering commands: `--output`, the view options
/// (`--viewport`, `--scale`, `--zoom`) and the dump options and file path.
pub(crate) struct RenderArgs {
    pub(crate) output: Option<OsString>,
    view: rustic_layout::View,
    pub(crate) limits: LoaderLimits,
    pub(crate) path: OsString,
}

pub(crate) fn parse_render_args(args: &[OsString]) -> Result<RenderArgs, String> {
    let mut output = None;
    let (mut scale, mut zoom, mut window) = (1.0, 1.0, (1280.0, 720.0));
    let mut rest = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--") => {
                rest.push(arg.clone());
                rest.extend(iter.by_ref().cloned());
            }
            Some("--output") => match iter.next() {
                Some(value) => output = Some(value.clone()),
                None => return Err("--output requires a value".into()),
            },
            Some("--scale") => {
                scale = iter
                    .next()
                    .and_then(|v| v.to_str())
                    .and_then(|v| v.parse::<f64>().ok())
                    .ok_or("--scale requires a number")?;
            }
            Some("--zoom") => {
                zoom = iter
                    .next()
                    .and_then(|v| v.to_str())
                    .and_then(|v| v.parse::<f64>().ok())
                    .ok_or("--zoom requires a number")?;
            }
            Some("--viewport") => {
                window = iter
                    .next()
                    .and_then(|v| v.to_str())
                    .and_then(|v| v.split_once('x'))
                    .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
                    .filter(|&(w, h)| (1..=16_384).contains(&w) && (1..=16_384).contains(&h))
                    .map(|(w, h)| (f64::from(w), f64::from(h)))
                    .ok_or("--viewport requires WxH with each side from 1 to 16384")?;
            }
            _ => rest.push(arg.clone()),
        }
    }
    rustic_layout::DeviceScale::new(scale)
        .map_err(|_| "--scale must be greater than 0 and at most 64")?;
    if !(rustic_layout::View::MIN_ZOOM..=rustic_layout::View::MAX_ZOOM).contains(&zoom) {
        return Err("--zoom must be from 0.25 to 5".into());
    }
    let view = rustic_layout::View::new(window.0, window.1, scale, zoom)
        .map_err(|_| "--scale times --zoom must be at most 64")?;
    let (mut limits, path) = parse_dump_args(&rest)?;
    limits.script_view = view;
    Ok(RenderArgs {
        output,
        view,
        limits,
        path,
    })
}

fn render_command(engine: &mut Engine, args: &[OsString]) -> u8 {
    let usage = |message: String| {
        eprintln!("rustic render: {message}\n\n{USAGE}");
        2
    };
    let args = match parse_render_args(args) {
        Ok(args) => args,
        Err(message) => return usage(message),
    };
    let Some(output) = args.output else {
        return usage("missing --output".into());
    };
    let (limits, view) = (args.limits, args.view);
    let png = match load(engine, "render", limits, &args.path, |parsed, path| {
        loader::render_png(parsed, path, limits, view)
    }) {
        Ok(png) => png,
        Err(code) => return code,
    };
    if let Err(error) = renderer::emit_file(&output, png) {
        eprintln!(
            "rustic render: {}: {error}",
            EscapedText(&Path::new(&output).to_string_lossy())
        );
        return 1;
    }
    0
}

fn hit_test_command(engine: &mut Engine, args: &[OsString]) -> u8 {
    let mut points = Vec::new();
    let mut rest = Vec::new();
    let mut iter = args.iter();
    let parsed: Result<(), String> = (|| {
        while let Some(arg) = iter.next() {
            match arg.to_str() {
                Some("--") => {
                    rest.push(arg.clone());
                    rest.extend(iter.by_ref().cloned());
                }
                Some("--at") => {
                    let point = iter
                        .next()
                        .and_then(|v| v.to_str())
                        .and_then(|v| v.split_once(','))
                        .and_then(|(x, y)| Some((x.parse::<f64>().ok()?, y.parse::<f64>().ok()?)))
                        .filter(|(x, y)| x.is_finite() && y.is_finite())
                        .ok_or("--at requires a point X,Y of finite numbers")?;
                    points.push(point);
                }
                _ => rest.push(arg.clone()),
            }
        }
        if points.is_empty() {
            return Err("missing --at".into());
        }
        Ok(())
    })();
    let parsed = parsed.and_then(|()| parse_dump_args(&rest));
    let (limits, path) = match parsed {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("rustic hit-test: {message}\n\n{USAGE}");
            return 2;
        }
    };
    dump_command_with(engine, "hit-test", limits, &path, |parsed, path| {
        loader::hit_test(parsed, path, limits, &points)
    })
}

fn reftest_command(engine: &mut Engine, args: &[OsString]) -> u8 {
    let args = match parse_render_args(args) {
        Ok(args) if args.output.is_some() => Err("--output is not accepted".into()),
        other => other,
    };
    let args = match args {
        Ok(args) => args,
        Err(message) => {
            eprintln!("rustic reftest: {message}\n\n{USAGE}");
            return 2;
        }
    };
    let (limits, view) = (args.limits, args.view);
    let loaded = load(engine, "reftest", limits, &args.path, |parsed, path| {
        let test = std::fs::canonicalize(path).map_err(loader::LoadError::Io)?;
        let rendered = loader::render_list(parsed, path, limits, view)?;
        Ok((
            rendered,
            reftest::manifest(parsed.parsed().document(), &test),
        ))
    });
    let (rendered, manifest) = match loaded {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let shown = EscapedText(&Path::new(&args.path).to_string_lossy()).to_string();
    for content in &manifest.invalid_fuzzy {
        eprintln!(
            "rustic reftest: {shown}: invalid fuzzy value {:?}",
            EscapedText(content).to_string()
        );
    }
    if manifest.references.is_empty() {
        eprintln!("rustic reftest: {shown}: no rel=match or rel=mismatch reference");
    }
    let surface = &rendered.1;
    let mut report = String::new();
    let mut all = manifest.invalid_fuzzy.is_empty() && !manifest.references.is_empty();
    for reference in &manifest.references {
        let detail = match &reference.path {
            Err(reason) => Err((*reason).to_owned()),
            Ok(path) => load(
                engine,
                "reftest",
                limits,
                path.as_os_str(),
                |parsed, path| {
                    let other = loader::render_list(parsed, path, limits, view)?;
                    // Paint conformance for damage: repainting what changed
                    // from the reference to the test gives the test exactly.
                    let exact = loader::damage_repaints_exactly(
                        &other,
                        &rendered,
                        view.scale(),
                        parsed.cancellation(),
                    )?;
                    Ok((other.1, exact))
                },
            )
            .map_err(|_| "failed to render".to_owned()),
        };
        let (pass, detail) = match detail {
            Err(reason) => (false, reason),
            Ok((_, false)) => (
                false,
                "repainting the damaged tiles does not give the test's pixels".to_owned(),
            ),
            Ok((other, true)) => {
                let difference = rustic_raster::compare(surface, &other);
                let detail = match difference {
                    None => "sizes differ".to_owned(),
                    Some(d) => format!(
                        "{} pixels differ, max channel difference {}, allowed maxDifference={} totalPixels={}",
                        d.pixels,
                        d.max_channel,
                        reference.fuzzy.max_difference,
                        reference.fuzzy.total_pixels
                    ),
                };
                (reftest::passes(reference, difference), detail)
            }
        };
        all &= pass;
        report.push_str(&format!(
            "{} {} {}: {detail}\n",
            if pass { "PASS" } else { "FAIL" },
            reference.relation,
            EscapedText(&reference.href)
        ));
    }
    report.push_str(&format!("{} {shown}\n", if all { "PASS" } else { "FAIL" }));
    let mut stdout = io::stdout().lock();
    if stdout
        .write_all(report.as_bytes())
        .and_then(|()| stdout.flush())
        .is_err()
    {
        return 1;
    }
    if all { 0 } else { 1 }
}

pub(crate) fn parse_dump_args(args: &[OsString]) -> Result<(LoaderLimits, OsString), String> {
    let mut limits = LoaderLimits::default();
    let mut diagnostics = None;
    let mut path = None;
    let mut options_done = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if !options_done {
            let target = match arg.to_str() {
                Some("--") => {
                    options_done = true;
                    continue;
                }
                Some("--run-scripts") => {
                    limits.run_scripts = true;
                    continue;
                }
                Some("--max-input-bytes") => Some(&mut limits.max_file_bytes),
                Some("--max-output-bytes") => Some(&mut limits.max_dump_bytes),
                Some("--chunk-bytes") => Some(&mut limits.read_chunk_bytes),
                Some("--max-nodes") => Some(&mut limits.parser.document.max_nodes),
                Some("--max-depth") => Some(&mut limits.parser.document.max_depth),
                Some("--max-format-boxes") => Some(&mut limits.formatting.max_boxes),
                Some("--max-format-depth") => Some(&mut limits.formatting.max_depth),
                Some("--max-format-steps") => Some(&mut limits.formatting.max_steps),
                Some("--max-stylesheets") => Some(&mut limits.stylesheets.max_sheets),
                Some("--max-stylesheet-bytes") => Some(&mut limits.stylesheets.max_total_bytes),
                Some("--max-import-depth") => Some(&mut limits.stylesheets.max_import_depth),
                Some("--max-images") => Some(&mut limits.images.max_images),
                Some("--max-image-bytes") => Some(&mut limits.images.max_file_bytes),
                Some("--max-image-decoded-bytes") => Some(&mut limits.images.max_decoded_bytes),
                Some("--max-diagnostics") => {
                    Some(diagnostics.insert(limits.parser.max_tree_diagnostics))
                }
                Some(option) if option.starts_with('-') && option != "-" => {
                    return Err(format!("unknown option {}", EscapedText(option)));
                }
                _ => None,
            };
            if let Some(target) = target {
                let name = arg.to_string_lossy();
                let value = iter
                    .next()
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| format!("{name} requires a value"))?;
                *target = value
                    .parse::<usize>()
                    .ok()
                    .filter(|&value| value > 0)
                    .ok_or_else(|| format!("{name} must be a positive integer"))?;
                continue;
            }
        }
        if path.replace(arg.clone()).is_some() {
            return Err("expected exactly one file path".into());
        }
    }
    if let Some(diagnostics) = diagnostics {
        limits.parser.tokenizer.max_diagnostics = diagnostics;
        limits.parser.max_tree_diagnostics = diagnostics;
    }
    path.map(|path| (limits, path))
        .ok_or_else(|| "missing file path".into())
}
