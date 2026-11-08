use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

use rustic_base::{CancellationToken, EscapedText};
use rustic_engine::Viewport;
use rustic_html::ParsedDocument;
use std::path::Path;

mod loader;
mod stylesheets;

use loader::{LoadError, LoaderLimits};

const USAGE: &str = "Rustic browser engine development scaffold

Usage:
  rustic info
  rustic dump-dom [OPTIONS] [--] <file>
  rustic dump-style [OPTIONS] [--] <file>
  rustic dump-boxes [OPTIONS] [--] <file>
  rustic dump-layout [OPTIONS] [--] <file>
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
dump-layout prints used content/padding/border rectangles for static non-replaced
blocks with collapsed vertical margins and visible overflow. Inline/text layout and
other unsupported formatting fail explicitly. No command paints pixels yet.

Options for all dump commands (positive integers; exceeding any limit except
--max-diagnostics fails the load):
  --max-input-bytes N    file and tokenizer input bytes (default 8388608)
  --max-output-bytes N   dump output bytes (default 67108864)
  --chunk-bytes N        bytes read per step (default 65536)
  --max-nodes N          document nodes, including the root (default 100000)
  --max-depth N          element nesting depth (default 256)
  --max-diagnostics N    retained tokenizer and tree recoveries, each; later
                         recoveries are counted, not fatal (default 1024)

Stylesheet options for dump-style, dump-boxes and dump-layout (exceeding one skips the sheet, never fatal):
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
            "HTML: local document parsing via dump-dom; CSS: selectors, cascade and computed style via dump-style; M3: principal boxes via dump-boxes and block reference geometry via dump-layout; full layout, JavaScript, networking, and rendering: not implemented"
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
    let path = match loader::local_path(&path) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("rustic {command}: {error}");
            return ExitCode::from(2);
        }
    };
    let result = loader::load_document(&path, limits, CancellationToken::default())
        .and_then(|parsed| dump(&parsed, &path, limits));
    match result {
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
        Err(error @ LoadError::InvalidLimits) => {
            eprintln!("rustic {command}: {error}");
            ExitCode::from(2)
        }
        Err(error) => {
            // The path is user input: escape it so control characters in a file
            // name cannot forge terminal output or extra log lines.
            eprintln!(
                "rustic {command}: {}: {error}",
                EscapedText(&path.to_string_lossy())
            );
            ExitCode::from(1)
        }
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
