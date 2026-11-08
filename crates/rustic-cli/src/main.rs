use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

use rustic_base::CancellationToken;
use rustic_engine::Viewport;

mod loader;

use loader::{LoadError, LoaderLimits};

const USAGE: &str = "Rustic browser engine development scaffold

Usage:
  rustic info
  rustic dump-dom [--max-input-bytes N] [--max-output-bytes N] [--chunk-bytes N] [--] <file>
  rustic --help | --version

dump-dom reads one local HTML file (UTF-8, scripting disabled, no network) and
prints a deterministic DOM tree dump. URLs are rejected.";

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
            "HTML: partial local document parsing via dump-dom; CSS, JavaScript, networking, and rendering: not implemented"
        );
        return ExitCode::SUCCESS;
    }
    if args[0] == "dump-dom" {
        return dump_dom(&args[1..]);
    }
    eprintln!(
        "Unsupported command. Rendering and navigation are not implemented; see docs/ROADMAP.md."
    );
    ExitCode::from(2)
}

fn dump_dom(args: &[OsString]) -> ExitCode {
    let (limits, path) = match parse_dump_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("rustic dump-dom: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let path = match loader::local_path(&path) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("rustic dump-dom: {error}");
            return ExitCode::from(2);
        }
    };
    let result = loader::load_document(&path, limits, CancellationToken::default())
        .and_then(|parsed| loader::dump_dom(&parsed, limits.max_dump_bytes));
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
            eprintln!("rustic dump-dom: {error}");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("rustic dump-dom: {}: {error}", path.display());
            ExitCode::from(1)
        }
    }
}

fn parse_dump_args(args: &[OsString]) -> Result<(LoaderLimits, OsString), String> {
    let mut limits = LoaderLimits::default();
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
                Some(option) if option.starts_with('-') && option != "-" => {
                    return Err(format!("unknown option {option}"));
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
    path.map(|path| (limits, path))
        .ok_or_else(|| "missing file path".into())
}
