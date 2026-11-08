use super::{ErrorKind, InsertionMode, NodeId, Ns, ParseError, TreeBuilder};

/// Element-in-scope boundary sets from the standard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Scope {
    Default,
    ListItem,
    Button,
    Table,
}

const IMPLIED: &[&str] = &[
    "dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt", "rtc",
];

const THOROUGH: &[&str] = &[
    "caption", "colgroup", "dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt", "rtc",
    "tbody", "td", "tfoot", "th", "thead", "tr",
];

impl TreeBuilder {
    pub(super) fn is_special(&self, id: NodeId) -> bool {
        let local = self.local(id);
        match self.ns(id) {
            Ns::Html => matches!(
                local,
                "address"
                    | "applet"
                    | "area"
                    | "article"
                    | "aside"
                    | "base"
                    | "basefont"
                    | "bgsound"
                    | "blockquote"
                    | "body"
                    | "br"
                    | "button"
                    | "caption"
                    | "center"
                    | "col"
                    | "colgroup"
                    | "dd"
                    | "details"
                    | "dir"
                    | "div"
                    | "dl"
                    | "dt"
                    | "embed"
                    | "fieldset"
                    | "figcaption"
                    | "figure"
                    | "footer"
                    | "form"
                    | "frame"
                    | "frameset"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "head"
                    | "header"
                    | "hgroup"
                    | "hr"
                    | "html"
                    | "iframe"
                    | "img"
                    | "input"
                    | "keygen"
                    | "li"
                    | "link"
                    | "listing"
                    | "main"
                    | "marquee"
                    | "menu"
                    | "meta"
                    | "nav"
                    | "noembed"
                    | "noframes"
                    | "noscript"
                    | "object"
                    | "ol"
                    | "p"
                    | "param"
                    | "plaintext"
                    | "pre"
                    | "script"
                    | "search"
                    | "section"
                    | "source"
                    | "style"
                    | "summary"
                    | "table"
                    | "tbody"
                    | "td"
                    | "template"
                    | "textarea"
                    | "tfoot"
                    | "th"
                    | "thead"
                    | "title"
                    | "tr"
                    | "track"
                    | "ul"
                    | "wbr"
                    | "xmp"
            ),
            Ns::MathMl => matches!(
                local,
                "mi" | "mo" | "mn" | "ms" | "mtext" | "annotation-xml"
            ),
            Ns::Svg => matches!(local, "foreignObject" | "desc" | "title"),
        }
    }

    fn scope_boundary(&self, id: NodeId, scope: Scope) -> bool {
        let local = self.local(id);
        let ns = self.ns(id);
        if scope == Scope::Table {
            return ns == Ns::Html && matches!(local, "html" | "table" | "template");
        }
        let base = match ns {
            Ns::Html => matches!(
                local,
                "applet"
                    | "caption"
                    | "html"
                    | "table"
                    | "td"
                    | "th"
                    | "marquee"
                    | "object"
                    | "template"
            ),
            Ns::MathMl => matches!(
                local,
                "mi" | "mo" | "mn" | "ms" | "mtext" | "annotation-xml"
            ),
            Ns::Svg => matches!(local, "foreignObject" | "desc" | "title"),
        };
        base || (ns == Ns::Html
            && match scope {
                Scope::ListItem => matches!(local, "ol" | "ul"),
                Scope::Button => local == "button",
                _ => false,
            })
    }

    /// Whether an element matching `target` is in the given scope.
    pub(super) fn in_scope_where(
        &self,
        scope: Scope,
        target: impl Fn(&Self, NodeId) -> bool,
    ) -> bool {
        for &id in self.open.iter().rev() {
            if target(self, id) {
                return true;
            }
            if self.scope_boundary(id, scope) {
                return false;
            }
        }
        false
    }

    pub(super) fn in_scope(&self, name: &str, scope: Scope) -> bool {
        self.in_scope_where(scope, |tree, id| tree.is_html(id, name))
    }

    pub(super) fn in_scope_any(&self, names: &[&str], scope: Scope) -> bool {
        self.in_scope_where(scope, |tree, id| {
            tree.html_name(id).is_some_and(|name| names.contains(&name))
        })
    }

    pub(super) fn has_open(&self, name: &str) -> bool {
        self.open.iter().any(|id| self.is_html(*id, name))
    }

    pub(super) fn current_is(&self, name: &str) -> bool {
        self.open.last().is_some_and(|id| self.is_html(*id, name))
    }

    pub(super) fn current_is_any(&self, names: &[&str]) -> bool {
        self.open
            .last()
            .and_then(|id| self.html_name(*id))
            .is_some_and(|name| names.contains(&name))
    }

    /// Pop until an HTML element with one of the names has been popped.
    pub(super) fn pop_until_any(&mut self, names: &[&str]) {
        while let Some(id) = self.open.pop() {
            if self.html_name(id).is_some_and(|name| names.contains(&name)) {
                break;
            }
        }
    }

    pub(super) fn pop_until(&mut self, name: &str) {
        self.pop_until_any(&[name]);
    }

    pub(super) fn generate_implied_end_tags(&mut self, except: Option<&str>) {
        while self.current_is_any(IMPLIED) && !except.is_some_and(|name| self.current_is(name)) {
            self.open.pop();
        }
    }

    pub(super) fn generate_all_implied_end_tags(&mut self) {
        while self.current_is_any(THOROUGH) {
            self.open.pop();
        }
    }

    /// Generate implied end tags, diagnose a mismatched current node and pop
    /// through the named element.
    pub(super) fn close_element(&mut self, name: &str) -> Result<(), ParseError> {
        self.generate_implied_end_tags(None);
        if !self.current_is(name) {
            self.diagnostic(ErrorKind::UnexpectedToken)?;
        }
        self.pop_until(name);
        Ok(())
    }

    pub(super) fn close_p(&mut self) -> Result<(), ParseError> {
        self.generate_implied_end_tags(Some("p"));
        if !self.current_is("p") {
            self.diagnostic(ErrorKind::UnexpectedToken)?;
        }
        self.pop_until("p");
        Ok(())
    }

    pub(super) fn close_p_in_button_scope(&mut self) -> Result<(), ParseError> {
        if self.in_scope("p", Scope::Button) {
            self.close_p()?;
        }
        Ok(())
    }

    pub(super) fn clear_to_context(&mut self, names: &[&str]) {
        while !self.current_is_any(names) {
            self.open.pop();
        }
    }

    pub(super) fn reset_insertion_mode(&mut self) {
        for index in (0..self.open.len()).rev() {
            let last = index == 0;
            let id = match self.context {
                Some(context) if last => context,
                _ => self.open[index],
            };
            let mode = match self.html_name(id) {
                Some("td" | "th") if !last => InsertionMode::InCell,
                Some("tr") => InsertionMode::InRow,
                Some("tbody" | "thead" | "tfoot") => InsertionMode::InTableBody,
                Some("caption") => InsertionMode::InCaption,
                Some("colgroup") => InsertionMode::InColumnGroup,
                Some("table") => InsertionMode::InTable,
                Some("template") => *self
                    .template_modes
                    .last()
                    .expect("open template has a template insertion mode"),
                Some("head") if !last => InsertionMode::InHead,
                Some("body") => InsertionMode::InBody,
                Some("frameset") => InsertionMode::InFrameset,
                Some("html") if self.head.is_none() => InsertionMode::BeforeHead,
                Some("html") => InsertionMode::AfterHead,
                _ if last => InsertionMode::InBody,
                _ => continue,
            };
            self.insertion_mode = mode;
            return;
        }
        self.insertion_mode = InsertionMode::InBody;
    }

    /// Diagnose open elements that the standard does not allow at body end/EOF.
    pub(super) fn check_unclosed(&mut self) -> Result<(), ParseError> {
        if self.open.iter().any(|id| {
            !matches!(
                self.html_name(*id),
                Some(
                    "dd" | "dt"
                        | "li"
                        | "optgroup"
                        | "option"
                        | "p"
                        | "rb"
                        | "rp"
                        | "rt"
                        | "rtc"
                        | "tbody"
                        | "td"
                        | "tfoot"
                        | "th"
                        | "thead"
                        | "tr"
                        | "body"
                        | "html"
                )
            )
        }) {
            self.diagnostic(ErrorKind::UnclosedElements)?;
        }
        Ok(())
    }
}
