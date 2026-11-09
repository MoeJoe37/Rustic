use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

use rustic_base::{CancellationToken, EscapedText};
use rustic_engine::Viewport;
use rustic_html::ParsedDocument;
use std::path::Path;

mod loader;
mod reftest;
mod stylesheets;

use loader::{LoadError, LoaderLimits};

const USAGE: &str = "Rustic browser engine development scaffold

Usage:
  rustic info
  rustic dump-dom [OPTIONS] [--] <file>
  rustic dump-style [OPTIONS] [--] <file>
  rustic dump-boxes [OPTIONS] [--] <file>
  rustic dump-formatting [OPTIONS] [--] <file>
  rustic dump-layout [OPTIONS] [--] <file>
  rustic dump-paint [OPTIONS] [--] <file>
  rustic render --output <png> [--scale N] [OPTIONS] [--] <file>
  rustic reftest [--scale N] [OPTIONS] [--] <test>
  rustic hit-test --at X,Y [--at X,Y ...] [OPTIONS] [--] <file>
  rustic --help | --version

dump-dom reads one local HTML file (UTF-8, scripting disabled, no network) and
prints a deterministic DOM tree dump. dump-style loads the file the same way,
applies the HTML user-agent stylesheet, the document's <style> elements and
its <link rel=stylesheet> and @import sheets, and prints each element's
computed style for a 1280x720 viewport. Linked and imported sheets load only
from files in the document's directory or below it; network URLs and paths
outside it are blocked. URLs are rejected as the file argument.

dump-boxes loads the same styles and prints a principal-box snapshot (no
anonymous/line boxes, layout positions or painting yet).
dump-formatting prints normalized static flow boxes with anonymous blocks and
split inline continuations, preserving unshaped text. Other formatting types,
positioning/floats and special/replaced elements fail explicitly.
dump-layout prints used content/padding/border rectangles for static non-replaced
blocks with collapsed vertical margins and visible overflow. Inline/text layout and
other unsupported formatting fail explicitly.
dump-paint prints the display list for that layout: the canvas color (with
root/body background propagation), block background colors and solid borders
in stacking-context paint order, with overflow clips and opacity groups.
render rasterizes the same display list on the CPU and writes an RGBA PNG to
--output (overwriting it). --scale sets device pixels per CSS pixel (default 1,
at most 64). Text, images and non-solid borders fail explicitly; the
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

Options for all dump commands (positive integers; exceeding any limit except
--max-diagnostics fails the load):
  --max-input-bytes N    file and tokenizer input bytes (default 8388608)
  --max-output-bytes N   dump output bytes (default 67108864)
  --chunk-bytes N        bytes read per step (default 65536)
  --max-nodes N          document nodes, including the root (default 100000)
  --max-depth N          element nesting depth (default 256)
  --max-diagnostics N    retained tokenizer and tree recoveries, each; later
                         recoveries are counted, not fatal (default 1024)

Normalization options for dump-formatting (exceeding one fails):
  --max-format-boxes N      allocated boxes, including temporary (default 100000)
  --max-format-depth N      formatting tree edges from a root (default 512)
  --max-format-steps N      visits, sequence entries and text units (default 16000000)

Stylesheet options for styled dump commands (exceeding one skips the sheet, never fatal):
  --max-stylesheets N       linked and imported sheets fetched (default 64)
  --max-stylesheet-bytes N  total bytes of those sheets (default 8388608)
  --max-import-depth N      @import nesting (default 16)";

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
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
        println!("Rustic {} — foundation stage", env!("CARGO_PKG_VERSION"));
        println!(
            "Default viewport: {}x{}",
            viewport.width(),
            viewport.height()
        );
        println!(
            "HTML: local document parsing via dump-dom; CSS: selectors, cascade and computed style via dump-style; M3: principal boxes via dump-boxes, normalized flow boxes via dump-formatting, block reference geometry via dump-layout and block background/border painting via dump-paint and render, reftests via reftest, hit testing via hit-test; text layout, JavaScript and networking: not implemented"
        );
        return ExitCode::SUCCESS;
    }
    if args[0] == "dump-dom" {
        return dump_command("dump-dom", &args[1..], loader::dump_dom);
    }
    if args[0] == "dump-style" {
        return dump_command("dump-style", &args[1..], loader::dump_style);
    }
    if args[0] == "dump-boxes" {
        return dump_command("dump-boxes", &args[1..], loader::dump_boxes);
    }
    if args[0] == "dump-layout" {
        return dump_command("dump-layout", &args[1..], loader::dump_layout);
    }
    if args[0] == "dump-paint" {
        return dump_command("dump-paint", &args[1..], loader::dump_paint);
    }
    if args[0] == "render" {
        return render_command(&args[1..]);
    }
    if args[0] == "hit-test" {
        return hit_test_command(&args[1..]);
    }
    if args[0] == "reftest" {
        return reftest_command(&args[1..]);
    }
    if args[0] == "dump-formatting" {
        return dump_command("dump-formatting", &args[1..], loader::dump_formatting);
    }
    eprintln!(
        "Unsupported command. Rendering and navigation are not implemented; see docs/ROADMAP.md."
    );
    ExitCode::from(2)
}

fn dump_command(
    command: &str,
    args: &[OsString],
    dump: fn(&ParsedDocument, &Path, LoaderLimits) -> Result<String, LoadError>,
) -> ExitCode {
    let (limits, path) = match parse_dump_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("rustic {command}: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    dump_command_with(command, limits, &path, |parsed, path| {
        dump(parsed, path, limits)
    })
}

/// Load a document, run `dump` and write its output to stdout.
fn dump_command_with(
    command: &str,
    limits: LoaderLimits,
    path: &OsString,
    dump: impl FnOnce(&ParsedDocument, &Path) -> Result<String, LoadError>,
) -> ExitCode {
    match load(command, limits, path, dump) {
        Ok(dump) => {
            let mut stdout = io::stdout().lock();
            if stdout
                .write_all(dump.as_bytes())
                .and_then(|()| stdout.flush())
                .is_err()
            {
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

/// Load a local document and run `then`, reporting errors on stderr.
fn load<T>(
    command: &str,
    limits: LoaderLimits,
    path: &std::ffi::OsStr,
    then: impl FnOnce(&ParsedDocument, &Path) -> Result<T, LoadError>,
) -> Result<T, ExitCode> {
    let path = match loader::local_path(path) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("rustic {command}: {error}");
            return Err(ExitCode::from(2));
        }
    };
    let result = loader::load_document(&path, limits, CancellationToken::default())
        .and_then(|parsed| then(&parsed, &path));
    match result {
        Ok(value) => Ok(value),
        Err(error @ LoadError::InvalidLimits) => {
            eprintln!("rustic {command}: {error}");
            Err(ExitCode::from(2))
        }
        Err(error) => {
            // The path is user input: escape it so control characters in a file
            // name cannot forge terminal output or extra log lines.
            eprintln!(
                "rustic {command}: {}: {error}",
                EscapedText(&path.to_string_lossy())
            );
            Err(ExitCode::from(1))
        }
    }
}

/// Arguments of the rendering commands: `--output`, `--scale` and the dump
/// options and file path.
struct RenderArgs {
    output: Option<OsString>,
    scale: rustic_layout::DeviceScale,
    limits: LoaderLimits,
    path: OsString,
}

fn parse_render_args(args: &[OsString]) -> Result<RenderArgs, String> {
    let mut output = None;
    let mut scale = 1.0;
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
            _ => rest.push(arg.clone()),
        }
    }
    let scale = rustic_layout::DeviceScale::new(scale)
        .map_err(|_| "--scale must be greater than 0 and at most 64")?;
    let (limits, path) = parse_dump_args(&rest)?;
    Ok(RenderArgs {
        output,
        scale,
        limits,
        path,
    })
}

fn render_command(args: &[OsString]) -> ExitCode {
    let usage = |message: String| {
        eprintln!("rustic render: {message}\n\n{USAGE}");
        ExitCode::from(2)
    };
    let args = match parse_render_args(args) {
        Ok(args) => args,
        Err(message) => return usage(message),
    };
    let Some(output) = args.output else {
        return usage("missing --output".into());
    };
    let (limits, scale) = (args.limits, args.scale);
    let png = match load("render", limits, &args.path, |parsed, path| {
        loader::render_png(parsed, path, limits, scale)
    }) {
        Ok(png) => png,
        Err(code) => return code,
    };
    if let Err(error) = std::fs::write(&output, png) {
        eprintln!(
            "rustic render: {}: {error}",
            EscapedText(&Path::new(&output).to_string_lossy())
        );
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn hit_test_command(args: &[OsString]) -> ExitCode {
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
            return ExitCode::from(2);
        }
    };
    dump_command_with("hit-test", limits, &path, |parsed, path| {
        loader::hit_test(parsed, path, limits, &points)
    })
}

fn reftest_command(args: &[OsString]) -> ExitCode {
    let args = match parse_render_args(args) {
        Ok(args) if args.output.is_some() => Err("--output is not accepted".into()),
        other => other,
    };
    let args = match args {
        Ok(args) => args,
        Err(message) => {
            eprintln!("rustic reftest: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let (limits, scale) = (args.limits, args.scale);
    let loaded = load("reftest", limits, &args.path, |parsed, path| {
        let test = std::fs::canonicalize(path).map_err(loader::LoadError::Io)?;
        let surface = loader::render_surface(parsed, path, limits, scale)?;
        Ok((surface, reftest::manifest(parsed.document(), &test)))
    });
    let (surface, manifest) = match loaded {
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
    let mut report = String::new();
    let mut all = manifest.invalid_fuzzy.is_empty() && !manifest.references.is_empty();
    for reference in &manifest.references {
        let detail = match &reference.path {
            Err(reason) => Err((*reason).to_owned()),
            Ok(path) => load("reftest", limits, path.as_os_str(), |parsed, path| {
                loader::render_surface(parsed, path, limits, scale)
            })
            .map_err(|_| "failed to render".to_owned()),
        };
        let (pass, detail) = match detail {
            Err(reason) => (false, reason),
            Ok(other) => {
                let difference = rustic_raster::compare(&surface, &other);
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
        return ExitCode::from(1);
    }
    if all {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn parse_dump_args(args: &[OsString]) -> Result<(LoaderLimits, OsString), String> {
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
