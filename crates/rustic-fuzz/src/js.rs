//! ECMAScript parsing (`rustic-js`): crash-freedom, bounded spans and a
//! strict/sloppy differential.
//!
//! Invariants: script and module parses never panic or overflow the stack;
//! a syntax error points inside the source; a successful parse dumps
//! identically twice; and source that parses as strict script (with a
//! `"use strict";` prologue) also parses as sloppy script, since strict mode
//! only removes syntax.
use crate::Target;
use rustic_js::{ParseError, ParseLimits, Source, ast, parse_module, parse_script};

pub(crate) struct JsParseTarget;

/// One snippet per line of `corpus/js-parse/*.js`.
pub(crate) fn seeds(root: &std::path::Path) -> std::io::Result<Vec<Vec<u8>>> {
    let directory = root.join("crates/rustic-fuzz/corpus/js-parse");
    let mut seeds = Vec::new();
    for (_, bytes) in crate::read_dir_inputs(&directory)? {
        let source = String::from_utf8_lossy(&bytes);
        seeds.extend(source.lines().map(|line| line.as_bytes().to_vec()));
    }
    Ok(seeds)
}

/// Parse `text` and check the error span or the dump.
fn parse(text: &str, module: bool) -> Result<Result<(), ParseError>, String> {
    let source = Source::from_str_limited(text, usize::MAX).map_err(|e| e.to_string())?;
    let limits = ParseLimits::default();
    let result = if module {
        parse_module(&source, &limits)
    } else {
        parse_script(&source, &limits)
    };
    let len = source.units().len() as u32;
    match result {
        Ok(program) => {
            if program.span.end != len {
                return Err("program span does not cover the source".into());
            }
            let first = ast::dump(&program);
            if first != ast::dump(&program) {
                return Err("dump is not deterministic".into());
            }
            Ok(Ok(()))
        }
        Err(ParseError::Syntax(e)) if e.span.start > e.span.end || e.span.end > len => {
            Err(format!(
                "syntax error span {} outside the source ({len} units)",
                e.span
            ))
        }
        Err(e) => Ok(Err(e)),
    }
}

impl Target for JsParseTarget {
    fn name(&self) -> &'static str {
        "js-parse"
    }

    fn max_len(&self) -> usize {
        4096
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"function ",
            b"function* ",
            b"async ",
            b"await ",
            b"yield ",
            b"class ",
            b"extends ",
            b"static ",
            b"#x",
            b"super",
            b"new.target",
            b"import.meta",
            b"import ",
            b"export ",
            b"let ",
            b"const ",
            b"var ",
            b"for (",
            b" of ",
            b" in ",
            b"=>",
            b"...",
            b"?.",
            b"??",
            b"**",
            b"`${",
            b"}`",
            b"/re/g",
            b"\"use strict\";",
            b"0x",
            b"0b",
            b"1_0",
            b"10n",
            b"\\u{61}",
            b"\\u0061",
            b"<!--",
            b"\n-->",
            b"/*",
            b"*/",
            b"\n",
            b"\xe2\x80\xa8",
            b"((((((((",
            b"[[[[[[[[",
            b"{{{{{{{{",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let text = String::from_utf8_lossy(input);
        let sloppy = parse(&text, false)?;
        let _module = parse(&text, true)?;
        let strict = parse(&format!("\"use strict\";\n{text}"), false)?;
        if let (Ok(()), Err(ParseError::Syntax(e))) = (&strict, &sloppy) {
            return Err(format!(
                "strict script parsed but sloppy script failed: {e}"
            ));
        }
        Ok(())
    }
}
