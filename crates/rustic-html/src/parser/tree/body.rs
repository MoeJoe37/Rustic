use super::stack::Scope;
use super::{
    Chars, DocumentMode, ErrorKind, InsertionMode, Ns, ParseError, Step, Tag, Tok, TokenizerMode,
    TreeBuilder,
};

const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "center",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "header",
    "hgroup",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "search",
    "section",
    "summary",
    "ul",
];

const BLOCK_ENDS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "button",
    "center",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "header",
    "hgroup",
    "listing",
    "main",
    "menu",
    "nav",
    "ol",
    "pre",
    "search",
    "section",
    "summary",
    "ul",
];

const HEADINGS: &[&str] = &["h1", "h2", "h3", "h4", "h5", "h6"];

const FORMATTING: &[&str] = &[
    "b", "big", "code", "em", "font", "i", "s", "small", "strike", "strong", "tt", "u",
];

fn tag(name: &str) -> Tag {
    Tag {
        name: name.into(),
        attributes: Vec::new(),
        self_closing: false,
    }
}

impl TreeBuilder {
    pub(super) fn in_body(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Text(Chars::Null, run) => return self.ignore_nulls(run),
            Tok::Text(class, text) => {
                self.reconstruct_formatting()?;
                self.insert_text(text)?;
                if class == Chars::Other {
                    self.frameset_ok = false;
                }
            }
            Tok::Comment(text) => self.comment(None, text)?,
            Tok::Doctype(_) => return self.ignore(),
            Tok::Start(tag) => return self.body_start(tag),
            Tok::End(name) => return self.body_end(name),
            Tok::Eof => {
                if !self.template_modes.is_empty() {
                    return self.in_template(token);
                }
                self.check_unclosed()?;
                self.done = true;
            }
        }
        Ok(Step::Consumed)
    }

    fn body_start(&mut self, tag: &Tag) -> Result<Step, ParseError> {
        let name = tag.name();
        match name {
            "html" => {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                if !self.has_open("template") {
                    self.add_missing_attributes(self.open[0], tag)?;
                }
            }
            "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script" | "style"
            | "template" | "title" => return self.in_head(Tok::Start(tag)),
            "body" => {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                if self.open.len() > 1
                    && self.is_html(self.open[1], "body")
                    && !self.has_open("template")
                {
                    self.frameset_ok = false;
                    self.add_missing_attributes(self.open[1], tag)?;
                }
            }
            "frameset" => {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                if self.open.len() > 1 && self.is_html(self.open[1], "body") && self.frameset_ok {
                    let body = self.open[1];
                    self.document.detach(body)?;
                    self.open.truncate(1);
                    self.insert_html(tag)?;
                    self.insertion_mode = InsertionMode::InFrameset;
                }
            }
            _ if BLOCKS.contains(&name) => {
                self.close_p_in_button_scope()?;
                self.insert_html(tag)?;
            }
            _ if HEADINGS.contains(&name) => {
                self.close_p_in_button_scope()?;
                if self.current_is_any(HEADINGS) {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    self.open.pop();
                }
                self.insert_html(tag)?;
            }
            "pre" | "listing" => {
                self.close_p_in_button_scope()?;
                self.insert_html(tag)?;
                self.skip_newline = true;
                self.frameset_ok = false;
            }
            "form" => {
                let template = self.has_open("template");
                if self.form.is_some() && !template {
                    return self.ignore();
                }
                self.close_p_in_button_scope()?;
                let id = self.insert_html(tag)?;
                if !template {
                    self.form = Some(id);
                }
            }
            "li" | "dd" | "dt" => {
                self.frameset_ok = false;
                let matches: &[&str] = if name == "li" { &["li"] } else { &["dd", "dt"] };
                for index in (0..self.open.len()).rev() {
                    let id = self.open[index];
                    if let Some(found) = self.html_name(id).filter(|n| matches.contains(n)) {
                        let found = found.to_owned();
                        self.generate_implied_end_tags(Some(&found));
                        if !self.current_is(&found) {
                            self.diagnostic(ErrorKind::UnexpectedToken)?;
                        }
                        self.pop_until(&found);
                        break;
                    }
                    if self.is_special(id)
                        && !matches!(self.html_name(id), Some("address" | "div" | "p"))
                    {
                        break;
                    }
                }
                self.close_p_in_button_scope()?;
                self.insert_html(tag)?;
            }
            "plaintext" => {
                self.close_p_in_button_scope()?;
                self.insert_html(tag)?;
                self.text_mode = Some(TokenizerMode::Plaintext);
            }
            "button" => {
                if self.in_scope("button", Scope::Default) {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    self.generate_implied_end_tags(None);
                    self.pop_until("button");
                }
                self.reconstruct_formatting()?;
                self.insert_html(tag)?;
                self.frameset_ok = false;
            }
            "a" => {
                if let Some(index) = self.formatting_after_marker("a") {
                    let super::Entry::Element(element, _) = self.formatting[index] else {
                        unreachable!("formatting lookup returns elements");
                    };
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    if !self.adoption_agency("a")? {
                        self.any_other_end_tag("a")?;
                    }
                    if let Some(index) = self.formatting_position(element) {
                        self.formatting.remove(index);
                    }
                    if let Some(index) = self.open.iter().position(|id| *id == element) {
                        self.open.remove(index);
                    }
                }
                self.reconstruct_formatting()?;
                let id = self.insert_html(tag)?;
                self.push_formatting(id, tag)?;
            }
            _ if FORMATTING.contains(&name) => {
                self.reconstruct_formatting()?;
                let id = self.insert_html(tag)?;
                self.push_formatting(id, tag)?;
            }
            "nobr" => {
                self.reconstruct_formatting()?;
                if self.in_scope("nobr", Scope::Default) {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    if !self.adoption_agency("nobr")? {
                        self.any_other_end_tag("nobr")?;
                    }
                    self.reconstruct_formatting()?;
                }
                let id = self.insert_html(tag)?;
                self.push_formatting(id, tag)?;
            }
            "applet" | "marquee" | "object" => {
                self.reconstruct_formatting()?;
                self.insert_html(tag)?;
                self.push_marker()?;
                self.frameset_ok = false;
            }
            "table" => {
                if self.document_mode != DocumentMode::Quirks {
                    self.close_p_in_button_scope()?;
                }
                self.insert_html(tag)?;
                self.frameset_ok = false;
                self.insertion_mode = InsertionMode::InTable;
            }
            // Fragment case: inside a select context these are ignored.
            "input" | "select" if self.context_is("select") => return self.ignore(),
            "area" | "br" | "embed" | "img" | "keygen" | "wbr" | "input" => {
                if name == "input" && self.in_scope("select", Scope::Default) {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    self.pop_until("select");
                }
                self.reconstruct_formatting()?;
                self.insert_void(tag)?;
                let hidden = name == "input"
                    && tag.attributes().iter().any(|attribute| {
                        attribute.name() == "type"
                            && attribute.value().eq_ignore_ascii_case("hidden")
                    });
                if !hidden {
                    self.frameset_ok = false;
                }
            }
            "param" | "source" | "track" => self.insert_void(tag)?,
            "hr" => {
                self.close_p_in_button_scope()?;
                if self.in_scope("select", Scope::Default) {
                    self.generate_implied_end_tags(None);
                    if self.in_scope_any(&["option", "optgroup"], Scope::Default) {
                        self.diagnostic(ErrorKind::UnexpectedToken)?;
                    }
                }
                self.insert_void(tag)?;
                self.frameset_ok = false;
            }
            "image" => {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                let renamed = Tag {
                    name: "img".into(),
                    attributes: tag.attributes.clone(),
                    self_closing: tag.self_closing,
                };
                self.dispatch(Tok::Start(&renamed))?;
                // The renamed token carries the acknowledgement for this one.
                return Ok(Step::Consumed);
            }
            "textarea" => {
                if self.in_scope("select", Scope::Default) {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    self.pop_until("select");
                }
                self.insert_text_element(tag, TokenizerMode::Rcdata)?;
                self.skip_newline = true;
                self.frameset_ok = false;
            }
            "xmp" => {
                self.close_p_in_button_scope()?;
                self.reconstruct_formatting()?;
                self.frameset_ok = false;
                self.insert_text_element(tag, TokenizerMode::Rawtext)?;
            }
            "iframe" => {
                self.frameset_ok = false;
                self.insert_text_element(tag, TokenizerMode::Rawtext)?;
            }
            "noembed" => self.insert_text_element(tag, TokenizerMode::Rawtext)?,
            "select" => {
                if self.in_scope("select", Scope::Default) {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    self.pop_until("select");
                } else {
                    self.reconstruct_formatting()?;
                    self.insert_html(tag)?;
                    self.frameset_ok = false;
                }
            }
            "option" | "optgroup" => {
                if self.in_scope("select", Scope::Default) {
                    self.generate_implied_end_tags((name == "option").then_some("optgroup"));
                    let nested: &[&str] = if name == "option" {
                        &["option"]
                    } else {
                        &["option", "optgroup"]
                    };
                    if self.in_scope_any(nested, Scope::Default) {
                        self.diagnostic(ErrorKind::UnexpectedToken)?;
                    }
                } else if self.current_is("option") {
                    self.open.pop();
                }
                self.reconstruct_formatting()?;
                self.insert_html(tag)?;
            }
            "rb" | "rtc" | "rp" | "rt" => {
                if self.in_scope("ruby", Scope::Default) {
                    if matches!(name, "rb" | "rtc") {
                        self.generate_implied_end_tags(None);
                        if !self.current_is("ruby") {
                            self.diagnostic(ErrorKind::UnexpectedToken)?;
                        }
                    } else {
                        self.generate_implied_end_tags(Some("rtc"));
                        if !self.current_is_any(&["rtc", "ruby"]) {
                            self.diagnostic(ErrorKind::UnexpectedToken)?;
                        }
                    }
                }
                self.insert_html(tag)?;
            }
            "math" | "svg" => {
                self.reconstruct_formatting()?;
                let ns = if name == "math" { Ns::MathMl } else { Ns::Svg };
                self.insert_element(tag, ns)?;
                if tag.self_closing() {
                    self.open.pop();
                    self.self_closing_acknowledged = true;
                }
            }
            "caption" | "col" | "colgroup" | "frame" | "head" | "tbody" | "td" | "tfoot" | "th"
            | "thead" | "tr" => return self.ignore(),
            _ => {
                self.reconstruct_formatting()?;
                self.insert_html(tag)?;
            }
        }
        Ok(Step::Consumed)
    }

    fn body_end(&mut self, name: &str) -> Result<Step, ParseError> {
        match name {
            "template" => return self.in_head(Tok::End(name)),
            "body" | "html" => {
                if !self.in_scope("body", Scope::Default) {
                    return self.ignore();
                }
                self.check_unclosed()?;
                self.insertion_mode = InsertionMode::AfterBody;
                if name == "html" {
                    return Ok(Step::Reprocess);
                }
            }
            _ if BLOCK_ENDS.contains(&name) => {
                if !self.in_scope(name, Scope::Default) {
                    return self.ignore();
                }
                self.close_element(name)?;
            }
            "form" => {
                if self.has_open("template") {
                    if !self.in_scope("form", Scope::Default) {
                        return self.ignore();
                    }
                    self.close_element("form")?;
                } else {
                    let node = self.form.take();
                    let Some(node) = node
                        .filter(|node| self.in_scope_where(Scope::Default, |_, id| id == *node))
                    else {
                        return self.ignore();
                    };
                    self.generate_implied_end_tags(None);
                    if self.current() != node {
                        self.diagnostic(ErrorKind::UnexpectedToken)?;
                    }
                    let index = self
                        .open
                        .iter()
                        .position(|id| *id == node)
                        .expect("open form");
                    self.open.remove(index);
                }
            }
            "p" => {
                if !self.in_scope("p", Scope::Button) {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    self.insert_implied("p")?;
                }
                self.close_p()?;
            }
            "li" | "dd" | "dt" => {
                let scope = if name == "li" {
                    Scope::ListItem
                } else {
                    Scope::Default
                };
                if !self.in_scope(name, scope) {
                    return self.ignore();
                }
                self.generate_implied_end_tags(Some(name));
                if !self.current_is(name) {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                }
                self.pop_until(name);
            }
            _ if HEADINGS.contains(&name) => {
                if !self.in_scope_any(HEADINGS, Scope::Default) {
                    return self.ignore();
                }
                self.generate_implied_end_tags(None);
                if !self.current_is(name) {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                }
                self.pop_until_any(HEADINGS);
            }
            "a" | "nobr" => {
                if !self.adoption_agency(name)? {
                    self.any_other_end_tag(name)?;
                }
            }
            _ if FORMATTING.contains(&name) => {
                if !self.adoption_agency(name)? {
                    self.any_other_end_tag(name)?;
                }
            }
            "applet" | "marquee" | "object" => {
                if !self.in_scope(name, Scope::Default) {
                    return self.ignore();
                }
                self.close_element(name)?;
                self.clear_formatting_to_marker();
            }
            "br" => {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                return self.body_start(&tag("br"));
            }
            _ => self.any_other_end_tag(name)?,
        }
        Ok(Step::Consumed)
    }

    pub(super) fn any_other_end_tag(&mut self, name: &str) -> Result<(), ParseError> {
        for index in (0..self.open.len()).rev() {
            let id = self.open[index];
            if self.is_html(id, name) {
                self.generate_implied_end_tags(Some(name));
                if self.current() != id {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                }
                self.open.truncate(index);
                return Ok(());
            }
            if self.is_special(id) {
                return self.diagnostic(ErrorKind::UnexpectedToken);
            }
        }
        Ok(())
    }
}
