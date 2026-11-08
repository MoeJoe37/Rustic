// Compatibility identifiers from WHATWG HTML, last updated 2026-11-06.
// https://html.spec.whatwg.org/multipage/parsing.html#the-initial-insertion-mode
// Standards data notice: ../../data/WHATWG-LICENSE.md. Algorithm is original Rust.
use crate::Doctype;

/// HTML compatibility mode selected at the start of document construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentMode {
    /// Standards mode.
    NoQuirks,
    /// Legacy table-cell sizing compatibility; layout integration is pending.
    LimitedQuirks,
    /// Legacy full quirks compatibility; layout integration is pending.
    Quirks,
}

const QUIRKS_PREFIXES: &[&str] = &[
    "+//Silmaril//dtd html Pro v0r11 19970101//",
    "-//AS//DTD HTML 3.0 asWedit + extensions//",
    "-//AdvaSoft Ltd//DTD HTML 3.0 asWedit + extensions//",
    "-//IETF//DTD HTML 2.0 Level 1//",
    "-//IETF//DTD HTML 2.0 Level 2//",
    "-//IETF//DTD HTML 2.0 Strict Level 1//",
    "-//IETF//DTD HTML 2.0 Strict Level 2//",
    "-//IETF//DTD HTML 2.0 Strict//",
    "-//IETF//DTD HTML 2.0//",
    "-//IETF//DTD HTML 2.1E//",
    "-//IETF//DTD HTML 3.0//",
    "-//IETF//DTD HTML 3.2 Final//",
    "-//IETF//DTD HTML 3.2//",
    "-//IETF//DTD HTML 3//",
    "-//IETF//DTD HTML Level 0//",
    "-//IETF//DTD HTML Level 1//",
    "-//IETF//DTD HTML Level 2//",
    "-//IETF//DTD HTML Level 3//",
    "-//IETF//DTD HTML Strict Level 0//",
    "-//IETF//DTD HTML Strict Level 1//",
    "-//IETF//DTD HTML Strict Level 2//",
    "-//IETF//DTD HTML Strict Level 3//",
    "-//IETF//DTD HTML Strict//",
    "-//IETF//DTD HTML//",
    "-//Metrius//DTD Metrius Presentational//",
    "-//Microsoft//DTD Internet Explorer 2.0 HTML Strict//",
    "-//Microsoft//DTD Internet Explorer 2.0 HTML//",
    "-//Microsoft//DTD Internet Explorer 2.0 Tables//",
    "-//Microsoft//DTD Internet Explorer 3.0 HTML Strict//",
    "-//Microsoft//DTD Internet Explorer 3.0 HTML//",
    "-//Microsoft//DTD Internet Explorer 3.0 Tables//",
    "-//Netscape Comm. Corp.//DTD HTML//",
    "-//Netscape Comm. Corp.//DTD Strict HTML//",
    "-//O'Reilly and Associates//DTD HTML 2.0//",
    "-//O'Reilly and Associates//DTD HTML Extended 1.0//",
    "-//O'Reilly and Associates//DTD HTML Extended Relaxed 1.0//",
    "-//SQ//DTD HTML 2.0 HoTMetaL + extensions//",
    "-//SoftQuad Software//DTD HoTMetaL PRO 6.0::19990601::extensions to HTML 4.0//",
    "-//SoftQuad//DTD HoTMetaL PRO 4.0::19971010::extensions to HTML 4.0//",
    "-//Spyglass//DTD HTML 2.0 Extended//",
    "-//Sun Microsystems Corp.//DTD HotJava HTML//",
    "-//Sun Microsystems Corp.//DTD HotJava Strict HTML//",
    "-//W3C//DTD HTML 3 1995-03-24//",
    "-//W3C//DTD HTML 3.2 Draft//",
    "-//W3C//DTD HTML 3.2 Final//",
    "-//W3C//DTD HTML 3.2//",
    "-//W3C//DTD HTML 3.2S Draft//",
    "-//W3C//DTD HTML 4.0 Frameset//",
    "-//W3C//DTD HTML 4.0 Transitional//",
    "-//W3C//DTD HTML Experimental 19960712//",
    "-//W3C//DTD HTML Experimental 970421//",
    "-//W3C//DTD W3 HTML//",
    "-//W3O//DTD W3 HTML 3.0//",
    "-//WebTechs//DTD Mozilla HTML 2.0//",
    "-//WebTechs//DTD Mozilla HTML//",
];

const HTML4_PREFIXES: &[&str] = &[
    "-//W3C//DTD HTML 4.01 Frameset//",
    "-//W3C//DTD HTML 4.01 Transitional//",
];

fn starts_with_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .as_bytes()
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix.as_bytes()))
}

pub(super) fn classify(doctype: &Doctype) -> DocumentMode {
    let public = doctype.public_id().unwrap_or("");
    let system = doctype.system_id().unwrap_or("");
    let html4 = HTML4_PREFIXES
        .iter()
        .any(|prefix| starts_with_ascii_case(public, prefix));
    if doctype.force_quirks()
        || doctype.name() != Some("html")
        || [
            "-//W3O//DTD W3 HTML Strict 3.0//EN//",
            "-/W3C/DTD HTML 4.0 Transitional/EN",
            "HTML",
        ]
        .iter()
        .any(|value| public.eq_ignore_ascii_case(value))
        || system.eq_ignore_ascii_case("http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd")
        || QUIRKS_PREFIXES
            .iter()
            .any(|prefix| starts_with_ascii_case(public, prefix))
        || (html4 && system.is_empty())
    {
        DocumentMode::Quirks
    } else if html4
        || [
            "-//W3C//DTD XHTML 1.0 Frameset//",
            "-//W3C//DTD XHTML 1.0 Transitional//",
        ]
        .iter()
        .any(|prefix| starts_with_ascii_case(public, prefix))
    {
        DocumentMode::LimitedQuirks
    } else {
        DocumentMode::NoQuirks
    }
}
