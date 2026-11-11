//! Parse a file (or `-e SOURCE`) and print the S-expression dump or the
//! syntax error: `cargo run -p rustic-js --example parse -- [--module] FILE`.

use std::process::ExitCode;

use rustic_js::{ParseLimits, Source, ast, parse_module, parse_script};

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let module = args.first().is_some_and(|a| a == "--module");
    if module {
        args.remove(0);
    }
    let text = match args.as_slice() {
        [flag, source] if flag == "-e" => source.clone(),
        [path] => match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("{path}: {e}");
                return ExitCode::FAILURE;
            }
        },
        _ => {
            eprintln!("usage: parse [--module] (FILE | -e SOURCE)");
            return ExitCode::FAILURE;
        }
    };
    let source = match Source::from_str_limited(&text, 64 * 1024 * 1024) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let limits = ParseLimits::default();
    let parsed = if module {
        parse_module(&source, &limits)
    } else {
        parse_script(&source, &limits)
    };
    match parsed {
        Ok(program) => match ast::dump(&program) {
            Ok(text) => {
                println!("{text}");
                ExitCode::SUCCESS
            }
            Err(_) => {
                eprintln!("tree too deep to print");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            let at = match &e {
                rustic_js::ParseError::Syntax(s) => source.position(s.span.start),
                rustic_js::ParseError::TooDeep(span) => source.position(span.start),
                rustic_js::ParseError::Cancelled => source.position(0),
            };
            eprintln!("{}:{}: {e}", at.line, at.column);
            ExitCode::FAILURE
        }
    }
}
