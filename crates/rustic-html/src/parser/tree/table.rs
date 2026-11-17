use rustic_dom::DomError;

use super::stack::Scope;
use super::{Chars, ErrorKind, InsertionMode, ParseError, Step, Tag, Tok, TreeBuilder};

const TABLE_CONTEXT: &[&str] = &["table", "template", "html"];
const BODY_CONTEXT: &[&str] = &["tbody", "tfoot", "thead", "template", "html"];
const ROW_CONTEXT: &[&str] = &["tr", "template", "html"];
const SECTIONS: &[&str] = &["tbody", "thead", "tfoot"];

fn implied(name: &str) -> Tag {
    Tag {
        name: name.into(),
        attributes: Vec::new(),
        self_closing: false,
    }
}

impl TreeBuilder {
    pub(super) fn in_table(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Text(..)
                if self.current_is_any(&["table", "tbody", "template", "tfoot", "thead", "tr"]) =>
            {
                self.pending_table_text.clear();
                self.pending_table_text_other = false;
                self.original_mode = self.insertion_mode;
                self.insertion_mode = InsertionMode::InTableText;
                return Ok(Step::Reprocess);
            }
            Tok::Annotation(annotation) => self.insert_annotation(None, annotation)?,
            Tok::Doctype(_) => return self.ignore(),
            Tok::Start(tag) => match tag.name() {
                "caption" => {
                    self.clear_to_context(TABLE_CONTEXT);
                    self.push_marker()?;
                    self.insert_html(tag)?;
                    self.insertion_mode = InsertionMode::InCaption;
                }
                "colgroup" => {
                    self.clear_to_context(TABLE_CONTEXT);
                    self.insert_html(tag)?;
                    self.insertion_mode = InsertionMode::InColumnGroup;
                }
                "col" => {
                    self.clear_to_context(TABLE_CONTEXT);
                    self.insert_html(&implied("colgroup"))?;
                    self.insertion_mode = InsertionMode::InColumnGroup;
                    return Ok(Step::Reprocess);
                }
                "tbody" | "tfoot" | "thead" => {
                    self.clear_to_context(TABLE_CONTEXT);
                    self.insert_html(tag)?;
                    self.insertion_mode = InsertionMode::InTableBody;
                }
                "td" | "th" | "tr" => {
                    self.clear_to_context(TABLE_CONTEXT);
                    self.insert_html(&implied("tbody"))?;
                    self.insertion_mode = InsertionMode::InTableBody;
                    return Ok(Step::Reprocess);
                }
                "table" => {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    if !self.in_scope("table", Scope::Table) {
                        return Ok(Step::Consumed);
                    }
                    self.pop_until("table");
                    self.reset_insertion_mode();
                    return Ok(Step::Reprocess);
                }
                "style" | "script" | "template" => return self.in_head(token),
                "input"
                    if tag.attributes().iter().any(|attribute| {
                        attribute.name() == "type"
                            && attribute.value().eq_ignore_ascii_case("hidden")
                    }) =>
                {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    self.insert_void(tag)?;
                }
                "form" => {
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    if self.has_open("template") || self.form.is_some() {
                        return Ok(Step::Consumed);
                    }
                    self.form = Some(self.insert_html(tag)?);
                    self.open.pop();
                }
                _ => return self.foster(token),
            },
            Tok::End("table") => {
                if !self.in_scope("table", Scope::Table) {
                    return self.ignore();
                }
                self.pop_until("table");
                self.reset_insertion_mode();
            }
            Tok::End(
                "body" | "caption" | "col" | "colgroup" | "html" | "tbody" | "td" | "tfoot" | "th"
                | "thead" | "tr",
            ) => return self.ignore(),
            Tok::End("template") => return self.in_head(token),
            Tok::Eof => return self.in_body(token),
            _ => return self.foster(token),
        }
        Ok(Step::Consumed)
    }

    /// Process a misplaced table token with in-body rules and foster parenting.
    fn foster(&mut self, token: Tok) -> Result<Step, ParseError> {
        self.diagnostic(ErrorKind::UnexpectedToken)?;
        self.foster_parenting = true;
        let result = self.in_body(token);
        self.foster_parenting = false;
        result
    }

    pub(super) fn in_table_text(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Text(Chars::Null, run) => self.ignore_nulls(run),
            Tok::Text(class, text) => {
                self.pending_table_text
                    .try_reserve(text.len())
                    .map_err(|_| DomError::AllocationFailed)?;
                self.pending_table_text.push_str(text);
                if class == Chars::Other {
                    self.pending_table_text_other = true;
                }
                Ok(Step::Consumed)
            }
            _ => {
                let text = std::mem::take(&mut self.pending_table_text);
                if self.pending_table_text_other {
                    // Text with non-whitespace is foster-parented as a whole,
                    // with the in-body character rules for each run.
                    self.diagnostic(ErrorKind::UnexpectedToken)?;
                    self.foster_parenting = true;
                    let result = (|| {
                        for (class, run) in super::runs(&text) {
                            self.in_body(Tok::Text(class, run))?;
                        }
                        Ok::<(), ParseError>(())
                    })();
                    self.foster_parenting = false;
                    result?;
                } else {
                    self.insert_text(&text)?;
                }
                self.insertion_mode = self.original_mode;
                Ok(Step::Reprocess)
            }
        }
    }

    pub(super) fn in_caption(&mut self, token: Tok) -> Result<Step, ParseError> {
        let reprocess = match token {
            Tok::End("caption") => false,
            Tok::Start(tag)
                if matches!(
                    tag.name(),
                    "caption"
                        | "col"
                        | "colgroup"
                        | "tbody"
                        | "td"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "tr"
                ) =>
            {
                true
            }
            Tok::End("table") => true,
            Tok::End(
                "body" | "col" | "colgroup" | "html" | "tbody" | "td" | "tfoot" | "th" | "thead"
                | "tr",
            ) => return self.ignore(),
            _ => return self.in_body(token),
        };
        if !self.in_scope("caption", Scope::Table) {
            return self.ignore();
        }
        self.close_element("caption")?;
        self.clear_formatting_to_marker();
        self.insertion_mode = InsertionMode::InTable;
        Ok(if reprocess {
            Step::Reprocess
        } else {
            Step::Consumed
        })
    }

    pub(super) fn in_column_group(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Text(Chars::Space, text) => self.insert_text(text)?,
            Tok::Annotation(annotation) => self.insert_annotation(None, annotation)?,
            Tok::Doctype(_) => return self.ignore(),
            Tok::Start(tag) if tag.name() == "html" => return self.in_body(token),
            Tok::Start(tag) if tag.name() == "col" => self.insert_void(tag)?,
            Tok::End("colgroup") => {
                if !self.current_is("colgroup") {
                    return self.ignore();
                }
                self.open.pop();
                self.insertion_mode = InsertionMode::InTable;
            }
            Tok::End("col") => return self.ignore(),
            Tok::Start(tag) if tag.name() == "template" => return self.in_head(token),
            Tok::End("template") => return self.in_head(token),
            Tok::Eof => return self.in_body(token),
            _ => {
                if !self.current_is("colgroup") {
                    return self.ignore();
                }
                self.open.pop();
                self.insertion_mode = InsertionMode::InTable;
                return Ok(Step::Reprocess);
            }
        }
        Ok(Step::Consumed)
    }

    pub(super) fn in_table_body(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Start(tag) if tag.name() == "tr" => {
                self.clear_to_context(BODY_CONTEXT);
                self.insert_html(tag)?;
                self.insertion_mode = InsertionMode::InRow;
            }
            Tok::Start(tag) if matches!(tag.name(), "th" | "td") => {
                self.diagnostic(ErrorKind::UnexpectedToken)?;
                self.clear_to_context(BODY_CONTEXT);
                self.insert_html(&implied("tr"))?;
                self.insertion_mode = InsertionMode::InRow;
                return Ok(Step::Reprocess);
            }
            Tok::End(name @ ("tbody" | "tfoot" | "thead")) => {
                if !self.in_scope(name, Scope::Table) {
                    return self.ignore();
                }
                self.clear_to_context(BODY_CONTEXT);
                self.open.pop();
                self.insertion_mode = InsertionMode::InTable;
            }
            Tok::Start(tag)
                if matches!(
                    tag.name(),
                    "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead"
                ) =>
            {
                return self.leave_table_body();
            }
            Tok::End("table") => return self.leave_table_body(),
            Tok::End("body" | "caption" | "col" | "colgroup" | "html" | "td" | "th" | "tr") => {
                return self.ignore();
            }
            _ => return self.in_table(token),
        }
        Ok(Step::Consumed)
    }

    fn leave_table_body(&mut self) -> Result<Step, ParseError> {
        if !self.in_scope_any(SECTIONS, Scope::Table) {
            return self.ignore();
        }
        self.clear_to_context(BODY_CONTEXT);
        self.open.pop();
        self.insertion_mode = InsertionMode::InTable;
        Ok(Step::Reprocess)
    }

    pub(super) fn in_row(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::Start(tag) if matches!(tag.name(), "th" | "td") => {
                self.clear_to_context(ROW_CONTEXT);
                self.insert_html(tag)?;
                self.insertion_mode = InsertionMode::InCell;
                self.push_marker()?;
            }
            Tok::End("tr") => {
                if !self.in_scope("tr", Scope::Table) {
                    return self.ignore();
                }
                self.clear_to_context(ROW_CONTEXT);
                self.open.pop();
                self.insertion_mode = InsertionMode::InTableBody;
            }
            Tok::Start(tag)
                if matches!(
                    tag.name(),
                    "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead" | "tr"
                ) =>
            {
                return self.leave_row();
            }
            Tok::End("table") => return self.leave_row(),
            Tok::End(name @ ("tbody" | "tfoot" | "thead")) => {
                if !self.in_scope(name, Scope::Table) {
                    return self.ignore();
                }
                if !self.in_scope("tr", Scope::Table) {
                    return Ok(Step::Consumed);
                }
                self.clear_to_context(ROW_CONTEXT);
                self.open.pop();
                self.insertion_mode = InsertionMode::InTableBody;
                return Ok(Step::Reprocess);
            }
            Tok::End("body" | "caption" | "col" | "colgroup" | "html" | "td" | "th") => {
                return self.ignore();
            }
            _ => return self.in_table(token),
        }
        Ok(Step::Consumed)
    }

    fn leave_row(&mut self) -> Result<Step, ParseError> {
        if !self.in_scope("tr", Scope::Table) {
            return self.ignore();
        }
        self.clear_to_context(ROW_CONTEXT);
        self.open.pop();
        self.insertion_mode = InsertionMode::InTableBody;
        Ok(Step::Reprocess)
    }

    pub(super) fn in_cell(&mut self, token: Tok) -> Result<Step, ParseError> {
        match token {
            Tok::End(name @ ("td" | "th")) => {
                if !self.in_scope(name, Scope::Table) {
                    return self.ignore();
                }
                self.close_element(name)?;
                self.clear_formatting_to_marker();
                self.insertion_mode = InsertionMode::InRow;
                Ok(Step::Consumed)
            }
            Tok::Start(tag)
                if matches!(
                    tag.name(),
                    "caption"
                        | "col"
                        | "colgroup"
                        | "tbody"
                        | "td"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "tr"
                ) =>
            {
                if !self.in_scope_any(&["td", "th"], Scope::Table) {
                    return self.ignore();
                }
                self.close_cell()?;
                Ok(Step::Reprocess)
            }
            Tok::End("body" | "caption" | "col" | "colgroup" | "html") => self.ignore(),
            Tok::End(name @ ("table" | "tbody" | "tfoot" | "thead" | "tr")) => {
                if !self.in_scope(name, Scope::Table) {
                    return self.ignore();
                }
                self.close_cell()?;
                Ok(Step::Reprocess)
            }
            _ => self.in_body(token),
        }
    }

    fn close_cell(&mut self) -> Result<(), ParseError> {
        self.generate_implied_end_tags(None);
        if !self.current_is_any(&["td", "th"]) {
            self.diagnostic(ErrorKind::UnexpectedToken)?;
        }
        self.pop_until_any(&["td", "th"]);
        self.clear_formatting_to_marker();
        self.insertion_mode = InsertionMode::InRow;
        Ok(())
    }

    pub(super) fn in_template(&mut self, token: Tok) -> Result<Step, ParseError> {
        let mode = match token {
            Tok::Text(..) | Tok::Annotation(_) | Tok::Doctype(_) => return self.in_body(token),
            Tok::Start(tag) => match tag.name() {
                "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script"
                | "style" | "template" | "title" => return self.in_head(token),
                "caption" | "colgroup" | "tbody" | "tfoot" | "thead" => InsertionMode::InTable,
                "col" => InsertionMode::InColumnGroup,
                "tr" => InsertionMode::InTableBody,
                "td" | "th" => InsertionMode::InRow,
                _ => InsertionMode::InBody,
            },
            Tok::End("template") => return self.in_head(token),
            Tok::End(_) => return self.ignore(),
            Tok::Eof => {
                if !self.has_open("template") {
                    self.done = true;
                    return Ok(Step::Consumed);
                }
                self.diagnostic(ErrorKind::UnclosedElements)?;
                self.pop_until("template");
                self.clear_formatting_to_marker();
                self.template_modes.pop();
                self.reset_insertion_mode();
                return Ok(Step::Reprocess);
            }
        };
        self.template_modes.pop();
        self.template_modes.push(mode);
        self.insertion_mode = mode;
        Ok(Step::Reprocess)
    }
}
