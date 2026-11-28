//! HTML interfaces used by the HTML element constructor algorithm.

use rustic_js::runtime::{Agent, JsResult, NativeFn, Value};

use super::super::install_interface;

// Keep wrapper selection, constructor identity and definition validation together.
macro_rules! interfaces {
    ($($index:literal => $interface:literal [$($tag:literal)|*] $(, $parent:literal)?;)*) => {
        fn interface(local: &str) -> Option<&'static str> {
            match local {
                $($($tag => Some($interface),)*)*
                _ => None,
            }
        }

        pub(in crate::page) fn constructor(name: &str) -> NativeFn {
            match name {
                $($interface => construct::<$index>,)*
                _ => construct::<0>,
            }
        }

        fn name(index: u8) -> &'static str {
            match index {
                $($index => $interface,)*
                _ => "HTMLElement",
            }
        }

        pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
            $($(install_interface(
                agent, $interface, Some($parent), construct::<$index>, 0, &[], |_, _| Ok(()),
            )?;)?)*
            Ok(())
        }
    }
}

interfaces! {
    1 => "HTMLAnchorElement" ["a"];
    2 => "HTMLAreaElement" ["area"];
    3 => "HTMLButtonElement" ["button"];
    4 => "HTMLDetailsElement" ["details"];
    5 => "HTMLDialogElement" ["dialog"];
    6 => "HTMLFormElement" ["form"];
    7 => "HTMLIFrameElement" ["iframe"];
    8 => "HTMLImageElement" ["img"];
    9 => "HTMLInputElement" ["input"];
    10 => "HTMLLinkElement" ["link"];
    11 => "HTMLSlotElement" ["slot"];
    12 => "HTMLTextAreaElement" ["textarea"];
    13 => "HTMLBRElement" ["br"], "HTMLElement";
    14 => "HTMLBaseElement" ["base"], "HTMLElement";
    15 => "HTMLBodyElement" ["body"], "HTMLElement";
    16 => "HTMLCanvasElement" ["canvas"], "HTMLElement";
    17 => "HTMLDataElement" ["data"], "HTMLElement";
    18 => "HTMLDataListElement" ["datalist"], "HTMLElement";
    19 => "HTMLDivElement" ["div"], "HTMLElement";
    20 => "HTMLDListElement" ["dl"], "HTMLElement";
    21 => "HTMLEmbedElement" ["embed"], "HTMLElement";
    22 => "HTMLFieldSetElement" ["fieldset"], "HTMLElement";
    23 => "HTMLHeadingElement" ["h1" | "h2" | "h3" | "h4" | "h5" | "h6"], "HTMLElement";
    24 => "HTMLHeadElement" ["head"], "HTMLElement";
    25 => "HTMLHRElement" ["hr"], "HTMLElement";
    26 => "HTMLHtmlElement" ["html"], "HTMLElement";
    27 => "HTMLLabelElement" ["label"], "HTMLElement";
    28 => "HTMLLegendElement" ["legend"], "HTMLElement";
    29 => "HTMLLIElement" ["li"], "HTMLElement";
    30 => "HTMLMapElement" ["map"], "HTMLElement";
    31 => "HTMLMenuElement" ["menu"], "HTMLElement";
    32 => "HTMLMetaElement" ["meta"], "HTMLElement";
    33 => "HTMLMeterElement" ["meter"], "HTMLElement";
    34 => "HTMLModElement" ["del" | "ins"], "HTMLElement";
    35 => "HTMLObjectElement" ["object"], "HTMLElement";
    36 => "HTMLOListElement" ["ol"], "HTMLElement";
    37 => "HTMLOptGroupElement" ["optgroup"], "HTMLElement";
    38 => "HTMLOptionElement" ["option"], "HTMLElement";
    39 => "HTMLOutputElement" ["output"], "HTMLElement";
    40 => "HTMLParagraphElement" ["p"], "HTMLElement";
    41 => "HTMLPictureElement" ["picture"], "HTMLElement";
    42 => "HTMLPreElement" ["pre" | "listing" | "xmp"], "HTMLElement";
    43 => "HTMLProgressElement" ["progress"], "HTMLElement";
    44 => "HTMLQuoteElement" ["blockquote" | "q"], "HTMLElement";
    45 => "HTMLScriptElement" ["script"], "HTMLElement";
    46 => "HTMLSelectElement" ["select"], "HTMLElement";
    47 => "HTMLSourceElement" ["source"], "HTMLElement";
    48 => "HTMLSpanElement" ["span"], "HTMLElement";
    49 => "HTMLStyleElement" ["style"], "HTMLElement";
    50 => "HTMLTableCaptionElement" ["caption"], "HTMLElement";
    51 => "HTMLTableCellElement" ["td" | "th"], "HTMLElement";
    52 => "HTMLTableColElement" ["col" | "colgroup"], "HTMLElement";
    53 => "HTMLTableElement" ["table"], "HTMLElement";
    54 => "HTMLTableRowElement" ["tr"], "HTMLElement";
    55 => "HTMLTableSectionElement" ["tbody" | "thead" | "tfoot"], "HTMLElement";
    56 => "HTMLTemplateElement" ["template"], "HTMLElement";
    57 => "HTMLTimeElement" ["time"], "HTMLElement";
    58 => "HTMLTitleElement" ["title"], "HTMLElement";
    59 => "HTMLTrackElement" ["track"], "HTMLElement";
    60 => "HTMLUListElement" ["ul"], "HTMLElement";
    61 => "HTMLMediaElement" [], "HTMLElement";
    62 => "HTMLAudioElement" ["audio"], "HTMLMediaElement";
    63 => "HTMLVideoElement" ["video"], "HTMLMediaElement";
    64 => "HTMLMarqueeElement" ["marquee"], "HTMLElement";
    65 => "HTMLFontElement" ["font"], "HTMLElement";
    66 => "HTMLFrameElement" ["frame"], "HTMLElement";
    67 => "HTMLFrameSetElement" ["frameset"], "HTMLElement";
    68 => "HTMLParamElement" ["param"], "HTMLElement";
    69 => "HTMLDirectoryElement" ["dir"], "HTMLElement";
}

pub(in crate::page) fn wrapper(local: &str) -> &'static str {
    interface(local).unwrap_or("HTMLElement")
}

pub(super) fn extends(local: &str) -> Option<&'static str> {
    interface(local).or_else(|| {
        matches!(
            local,
            "abbr"
                | "acronym"
                | "address"
                | "article"
                | "aside"
                | "b"
                | "bdi"
                | "bdo"
                | "big"
                | "center"
                | "cite"
                | "code"
                | "dd"
                | "dfn"
                | "dt"
                | "em"
                | "figcaption"
                | "figure"
                | "footer"
                | "header"
                | "hgroup"
                | "i"
                | "kbd"
                | "main"
                | "mark"
                | "nav"
                | "nobr"
                | "noembed"
                | "noframes"
                | "noscript"
                | "plaintext"
                | "rb"
                | "rp"
                | "rt"
                | "rtc"
                | "ruby"
                | "s"
                | "samp"
                | "search"
                | "section"
                | "small"
                | "strike"
                | "strong"
                | "sub"
                | "summary"
                | "sup"
                | "tt"
                | "u"
                | "var"
                | "wbr"
        )
        .then_some("HTMLElement")
    })
}

pub(in crate::page) fn construct<const INDEX: u8>(
    agent: &mut Agent,
    _: Value,
    _: &[Value],
) -> JsResult<Value> {
    super::construct_element(agent, name(INDEX))
}
