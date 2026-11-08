// SVG/MathML name-adjustment tables from WHATWG HTML, last updated 2026-11-06.
// https://html.spec.whatwg.org/multipage/parsing.html#parsing-main-inforeign
// Standards data notice: ../../../data/WHATWG-LICENSE.md. Algorithm is original Rust.
use super::{Chars, ErrorKind, ParseError, Step, Tok, TreeBuilder};

const XLINK: &str = "http://www.w3.org/1999/xlink";
const XML: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS: &str = "http://www.w3.org/2000/xmlns/";

const SVG_TAGS: &[(&str, &str)] = &[
    ("altglyph", "altGlyph"),
    ("altglyphdef", "altGlyphDef"),
    ("altglyphitem", "altGlyphItem"),
    ("animatecolor", "animateColor"),
    ("animatemotion", "animateMotion"),
    ("animatetransform", "animateTransform"),
    ("clippath", "clipPath"),
    ("feblend", "feBlend"),
    ("fecolormatrix", "feColorMatrix"),
    ("fecomponenttransfer", "feComponentTransfer"),
    ("fecomposite", "feComposite"),
    ("feconvolvematrix", "feConvolveMatrix"),
    ("fediffuselighting", "feDiffuseLighting"),
    ("fedisplacementmap", "feDisplacementMap"),
    ("fedistantlight", "feDistantLight"),
    ("fedropshadow", "feDropShadow"),
    ("feflood", "feFlood"),
    ("fefunca", "feFuncA"),
    ("fefuncb", "feFuncB"),
    ("fefuncg", "feFuncG"),
    ("fefuncr", "feFuncR"),
    ("fegaussianblur", "feGaussianBlur"),
    ("feimage", "feImage"),
    ("femerge", "feMerge"),
    ("femergenode", "feMergeNode"),
    ("femorphology", "feMorphology"),
    ("feoffset", "feOffset"),
    ("fepointlight", "fePointLight"),
    ("fespecularlighting", "feSpecularLighting"),
    ("fespotlight", "feSpotLight"),
    ("fetile", "feTile"),
    ("feturbulence", "feTurbulence"),
    ("foreignobject", "foreignObject"),
    ("glyphref", "glyphRef"),
    ("lineargradient", "linearGradient"),
    ("radialgradient", "radialGradient"),
    ("textpath", "textPath"),
];

const SVG_ATTRIBUTES: &[(&str, &str)] = &[
    ("attributename", "attributeName"),
    ("attributetype", "attributeType"),
    ("basefrequency", "baseFrequency"),
    ("baseprofile", "baseProfile"),
    ("calcmode", "calcMode"),
    ("clippathunits", "clipPathUnits"),
    ("diffuseconstant", "diffuseConstant"),
    ("edgemode", "edgeMode"),
    ("filterunits", "filterUnits"),
    ("glyphref", "glyphRef"),
    ("gradienttransform", "gradientTransform"),
    ("gradientunits", "gradientUnits"),
    ("kernelmatrix", "kernelMatrix"),
    ("kernelunitlength", "kernelUnitLength"),
    ("keypoints", "keyPoints"),
    ("keysplines", "keySplines"),
    ("keytimes", "keyTimes"),
    ("lengthadjust", "lengthAdjust"),
    ("limitingconeangle", "limitingConeAngle"),
    ("markerheight", "markerHeight"),
    ("markerunits", "markerUnits"),
    ("markerwidth", "markerWidth"),
    ("maskcontentunits", "maskContentUnits"),
    ("maskunits", "maskUnits"),
    ("numoctaves", "numOctaves"),
    ("pathlength", "pathLength"),
    ("patterncontentunits", "patternContentUnits"),
    ("patterntransform", "patternTransform"),
    ("patternunits", "patternUnits"),
    ("pointsatx", "pointsAtX"),
    ("pointsaty", "pointsAtY"),
    ("pointsatz", "pointsAtZ"),
    ("preservealpha", "preserveAlpha"),
    ("preserveaspectratio", "preserveAspectRatio"),
    ("primitiveunits", "primitiveUnits"),
    ("refx", "refX"),
    ("refy", "refY"),
    ("repeatcount", "repeatCount"),
    ("repeatdur", "repeatDur"),
    ("requiredextensions", "requiredExtensions"),
    ("requiredfeatures", "requiredFeatures"),
    ("specularconstant", "specularConstant"),
    ("specularexponent", "specularExponent"),
    ("spreadmethod", "spreadMethod"),
    ("startoffset", "startOffset"),
    ("stddeviation", "stdDeviation"),
    ("stitchtiles", "stitchTiles"),
    ("surfacescale", "surfaceScale"),
    ("systemlanguage", "systemLanguage"),
    ("tablevalues", "tableValues"),
    ("targetx", "targetX"),
    ("targety", "targetY"),
    ("textlength", "textLength"),
    ("viewbox", "viewBox"),
    ("viewtarget", "viewTarget"),
    ("xchannelselector", "xChannelSelector"),
    ("ychannelselector", "yChannelSelector"),
    ("zoomandpan", "zoomAndPan"),
];

/// Start tags that break out of foreign content back to HTML rules.
const BREAKOUT: &[&str] = &[
    "b",
    "big",
    "blockquote",
    "body",
    "br",
    "center",
    "code",
    "dd",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "hr",
    "i",
    "img",
    "li",
    "listing",
    "menu",
    "meta",
    "nobr",
    "ol",
    "p",
    "pre",
    "ruby",
    "s",
    "small",
    "span",
    "strong",
    "strike",
    "sub",
    "sup",
    "table",
    "tt",
    "u",
    "ul",
    "var",
];

fn lookup<'a>(table: &[(&str, &'a str)], name: &'a str) -> &'a str {
    table
        .binary_search_by(|(key, _)| (*key).cmp(name))
        .map_or(name, |index| table[index].1)
}

pub(super) fn adjust_svg_tag(name: &str) -> &str {
    lookup(SVG_TAGS, name)
}

pub(super) fn adjust_svg_attribute(name: &str) -> &str {
    lookup(SVG_ATTRIBUTES, name)
}

pub(super) fn adjust_mathml_attribute(name: &str) -> &str {
    if name == "definitionurl" {
        "definitionURL"
    } else {
        name
    }
}

/// Namespace, prefix and local name for adjusted foreign attributes.
pub(super) fn foreign_attribute(name: &str) -> Option<(&'static str, Option<&str>, &str)> {
    match name {
        "xlink:actuate" | "xlink:arcrole" | "xlink:href" | "xlink:role" | "xlink:show"
        | "xlink:title" | "xlink:type" => Some((XLINK, Some("xlink"), &name[6..])),
        "xml:lang" | "xml:space" => Some((XML, Some("xml"), &name[4..])),
        "xmlns" => Some((XMLNS, None, name)),
        "xmlns:xlink" => Some((XMLNS, Some("xmlns"), "xlink")),
        _ => None,
    }
}

impl TreeBuilder {
    /// The rules for parsing tokens in foreign content.
    pub(super) fn foreign(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Text(Chars::Null, text) => {
                for _ in text.chars() {
                    self.diagnostic(ErrorKind::UnexpectedNull)?;
                    self.insert_text("\u{fffd}")?;
                }
            }
            Tok::Text(class, text) => {
                self.insert_text(text)?;
                if class == Chars::Other {
                    self.frameset_ok = false;
                }
            }
            Tok::Comment(text) => self.comment(None, text)?,
            Tok::Doctype(_) => return self.ignore(),
            Tok::Start(tag)
                if BREAKOUT.contains(&tag.name())
                    || (tag.name() == "font"
                        && tag
                            .attributes()
                            .iter()
                            .any(|a| matches!(a.name(), "color" | "face" | "size"))) =>
            {
                return self.break_out();
            }
            Tok::End("br" | "p") => return self.break_out(),
            Tok::Start(tag) => {
                let ns = self.ns(self.current());
                self.insert_element(tag, ns)?;
                if tag.self_closing() {
                    // Scripting is disabled: an SVG script is never processed.
                    self.open.pop();
                    self.self_closing_acknowledged = true;
                }
            }
            Tok::End(name) => {
                let mut index = self.open.len() - 1;
                if !self.local(self.open[index]).eq_ignore_ascii_case(name) {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                }
                while index > 0 {
                    let id = self.open[index];
                    if self.local(id).eq_ignore_ascii_case(name) {
                        self.open.truncate(index);
                        return Ok(Step::Consumed);
                    }
                    index -= 1;
                    if self.ns(self.open[index]) == super::Ns::Html {
                        return self.step(self.insertion_mode, token);
                    }
                }
            }
            Tok::Eof => unreachable!("EOF always uses insertion-mode rules"),
        }
        Ok(Step::Consumed)
    }

    fn break_out(&mut self) -> Result<Step, ParseError> {
        self.diagnostic(ErrorKind::UnexpectedToken)?;
        while let Some(&id) = self.open.last() {
            if self.ns(id) == super::Ns::Html
                || self.html_integration_point(id)
                || (self.ns(id) == super::Ns::MathMl
                    && matches!(self.local(id), "mi" | "mo" | "mn" | "ms" | "mtext"))
            {
                break;
            }
            self.open.pop();
        }
        Ok(Step::Reprocess)
    }
}
