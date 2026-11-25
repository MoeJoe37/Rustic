//! Incremental restyling: which elements a DOM mutation can affect, and a
//! restyle that recomputes only those (CSS-06).
//!
//! [`StyledDocument`] keeps the styles of a full [`crate::style_tree`] walk. The
//! caller reports each mutation after making it ([`StyledDocument::note`]),
//! and [`StyledDocument::restyle`] then recomputes the elements the
//! mutations may have affected, reusing every other element's style. The
//! result always equals a fresh [`crate::style_tree`] walk; the tests and the
//! `css-invalidation` fuzz target check exactly that.
//!
//! What a mutation affects comes from an [`InvalidationMap`] built from the
//! cascade's selectors:
//!
//! * An attribute that no selector tests (and that no element state reads)
//!   affects nothing, except `style`, which affects its own element.
//! * An attribute tested only in subject compounds affects its element; one
//!   tested in an ancestor compound also affects the element's subtree, and
//!   with sibling combinators the following siblings' subtrees too.
//! * A change to an element's children affects the children themselves when
//!   any selector is structural (`:nth-*`, `:*-child`, `:*-of-type`,
//!   `:empty`, `+`, `~`) and the parent (for `:empty`); inserted or moved
//!   elements are always styled afresh with their subtrees.
//! * `:has()` makes a change restyle the possible anchors: the changed
//!   element's ancestors as far as the relative selectors reach, with their
//!   preceding siblings when relative selectors use `+` or `~` (and anchor
//!   subtrees when a `:has()` sits outside the subject compound).
//! * HTML element states restyle what their facts reach: the subtree under a
//!   changed `lang`, `dir`, `disabled` or `contenteditable`, the outermost
//!   `dir=auto` ancestor of changed text, and every `input`, `button` and
//!   `option` when radio groups, form owners, default buttons or selected
//!   options can change. A pragma language change restyles everything.
//!
//! A computed-style change on an element restyles its children (they
//! inherit), and a change on the root restyles everything (`rem`, `rlh`).

use std::collections::HashMap;

use rustic_css::{
    Combinator, ComplexSelector, CssParser, PseudoClass, SelectorList, SimpleSelector,
};
use rustic_dom::{Document, DomError, NodeId, NodeKind};

use crate::dom::{Dirty, Reuse, style_walk};
use crate::{
    Cascade, ComputeContext, ContainerSize, DomElement, ElementStyle, ElementView, MatchContext,
    style_tree_with_sizes,
};

/// Attributes that the HTML form-control states read (`:checked`,
/// `:default`, `:disabled`, `:read-write`, `:placeholder-shown`, ...).
const FORM_ATTRIBUTES: &[&str] = &[
    "checked",
    "contenteditable",
    "disabled",
    "form",
    "id",
    "multiple",
    "name",
    "placeholder",
    "readonly",
    "required",
    "selected",
    "size",
    "type",
    "value",
];

/// Of [`FORM_ATTRIBUTES`], those that can change the state of elements
/// elsewhere in the tree: radio group membership, form owners, default
/// buttons and the selected option of a `select`.
const FORM_GROUP_ATTRIBUTES: &[&str] = &["checked", "form", "id", "name", "selected", "type"];

/// Elements whose state a form-group change can flip.
const FORM_GROUP_ELEMENTS: &[&str] = &["button", "input", "option"];

/// Which element states the cascade's selectors use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct States {
    /// Form-control states.
    form: bool,
    /// `:lang()`.
    lang: bool,
    /// `:dir()`.
    dir: bool,
}

impl States {
    fn any(self) -> bool {
        self.form || self.lang || self.dir
    }
}

/// Where a simple selector sits.
#[derive(Clone, Copy, Debug)]
struct Position {
    /// In the subject compound of a top-level rule selector.
    subject: bool,
    /// Inside a `:has()` argument, with whether the test sits in a nested
    /// selector list's non-subject compound (`:has(:is(.p .q))`), where it
    /// can test elements above the anchor or before it among siblings.
    has: Option<bool>,
    /// Inside a nested selector list (`:is()`, `:not()`, `of S`, `&`).
    nested: bool,
}

/// What the cascade's selectors depend on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InvalidationMap {
    /// Attribute local names (ASCII lowercase) that selectors test outside
    /// `:has()` arguments, with whether any test sits outside a subject
    /// compound. `#id` counts as `id` and `.class` as `class`.
    attributes: HashMap<String, bool>,
    /// Any `:nth-*`, `:first-*`, `:last-*`, `:only-*` or `:empty`.
    structural: bool,
    /// Structural tests in ancestor compounds can change descendant subjects.
    structural_outside_subject: bool,
    /// Filtered sibling counting can read arbitrary selector dependencies.
    nth_filter: bool,
    /// Scoping boundaries can depend on selectors outside the subject subtree.
    scoped_rules: bool,
    /// Any `+` or `~` combinator outside `:has()` arguments.
    siblings: bool,
    /// The `:has()` dependencies.
    has: HasMap,
    /// Element states used outside `:has()` arguments.
    states: States,
    /// Whether an element state sits outside a subject compound.
    states_outside_subject: bool,
}

/// What `:has()` selectors depend on.
#[derive(Clone, Debug, Default, PartialEq)]
struct HasMap {
    /// Any `:has()`.
    any: bool,
    /// A `:has()` outside a subject compound (`.a:has(.b) .c`): an anchor
    /// whose result changes restyles its subtree, not only itself.
    outside_subject: bool,
    /// Attributes tested in `:has()` arguments, with whether a test sits in
    /// a nested list's non-subject compound.
    attributes: HashMap<String, bool>,
    /// How far above a changed element an anchor can be: the most child
    /// combinators in one relative selector, or `None` when one uses a
    /// descendant combinator.
    reach: Option<usize>,
    /// Any `+` or `~` in a relative selector, leading or inner.
    siblings: bool,
    /// Element states used in `:has()` arguments.
    states: States,
    /// An element state in a nested list's non-subject compound of a
    /// `:has()` argument. Such states are rare and restyle everything.
    deep_states: bool,
}

impl InvalidationMap {
    /// The dependencies of every selector in `cascade`.
    pub fn new(cascade: &Cascade) -> Self {
        let mut map = Self {
            has: HasMap {
                reach: Some(0),
                ..HasMap::default()
            },
            ..Self::default()
        };
        let top = Position {
            subject: true,
            has: None,
            nested: false,
        };
        for sheet in cascade.sheets() {
            for rule in &sheet.rules {
                map.list(&rule.selectors, top);
                map.scoped_rules |= rule.scope.is_some();
            }
        }
        map
    }

    fn list(&mut self, list: &SelectorList, at: Position) {
        for selector in list.selectors() {
            self.complex(selector, at);
        }
    }

    fn nested(&mut self, list: &SelectorList, at: Position) {
        self.list(list, Position { nested: true, ..at });
    }

    fn complex(&mut self, selector: &ComplexSelector, at: Position) {
        let siblings = selector
            .combinators()
            .iter()
            .any(|c| matches!(c, Combinator::NextSibling | Combinator::SubsequentSibling));
        match at.has {
            Some(_) => self.has.siblings |= siblings,
            None => self.siblings |= siblings,
        }
        let last = selector.compounds().len().saturating_sub(1);
        for (i, compound) in selector.compounds().iter().enumerate() {
            let here = Position {
                subject: at.subject && i == last,
                has: at.has.map(|deep| deep || (at.nested && i != last)),
                nested: at.nested,
            };
            for simple in compound.simple_selectors() {
                self.simple(simple, here);
            }
        }
    }

    fn attribute(&mut self, name: &str, at: Position) {
        let name = name.to_ascii_lowercase();
        match at.has {
            Some(deep) => *self.has.attributes.entry(name).or_insert(false) |= deep,
            None => *self.attributes.entry(name).or_insert(false) |= !at.subject,
        }
    }

    fn simple(&mut self, simple: &SimpleSelector, at: Position) {
        match simple {
            SimpleSelector::Id(_) => self.attribute("id", at),
            SimpleSelector::Class(_) => self.attribute("class", at),
            SimpleSelector::Attribute(a) => self.attribute(&a.local, at),
            SimpleSelector::PseudoClass(class) => self.pseudo_class(class, at),
            SimpleSelector::Nesting(Some(list)) => self.nested(list, at),
            SimpleSelector::Type { .. }
            | SimpleSelector::Universal { .. }
            | SimpleSelector::Nesting(None)
            | SimpleSelector::RelativeAnchor => {}
        }
    }

    fn state(&mut self, at: Position, set: fn(&mut States)) {
        match at.has {
            Some(deep) => {
                set(&mut self.has.states);
                self.has.deep_states |= deep;
            }
            None => {
                set(&mut self.states);
                self.states_outside_subject |= !at.subject;
            }
        }
    }

    fn pseudo_class(&mut self, class: &PseudoClass, at: Position) {
        match class {
            PseudoClass::Empty | PseudoClass::OnlyChild | PseudoClass::OnlyOfType => {
                self.structural = true;
                self.structural_outside_subject |= !at.subject;
            }
            PseudoClass::Nth { nth, .. } => {
                self.structural = true;
                self.structural_outside_subject |= !at.subject;
                if let Some(of) = &nth.of {
                    self.nth_filter = true;
                    self.nested(of, at);
                }
            }
            PseudoClass::Is(list) | PseudoClass::Where(list) | PseudoClass::Not(list) => {
                self.nested(list, at);
            }
            PseudoClass::Host(list) => {
                if let Some(list) = list {
                    // The host is outside the shadow tree whose elements
                    // the rule styles.
                    self.nested(
                        list,
                        Position {
                            subject: false,
                            ..at
                        },
                    );
                }
            }
            PseudoClass::Has(list) => {
                self.has.any = true;
                self.has.outside_subject |= !at.subject;
                for relative in list.selectors() {
                    let selector = relative.selector();
                    let mut children = 0usize;
                    let mut unbounded = false;
                    for combinator in selector.combinators() {
                        match combinator {
                            Combinator::Child => children += 1,
                            Combinator::Descendant => unbounded = true,
                            Combinator::NextSibling | Combinator::SubsequentSibling => {}
                        }
                    }
                    self.has.reach = match self.has.reach {
                        Some(reach) if !unbounded => Some(reach.max(children)),
                        _ => None,
                    };
                    self.complex(
                        selector,
                        Position {
                            subject: false,
                            has: Some(at.has.unwrap_or(false)),
                            nested: false,
                        },
                    );
                }
            }
            PseudoClass::AnyLink { .. } => self.attribute("href", at),
            PseudoClass::Form(_) => self.state(at, |s| s.form = true),
            PseudoClass::Lang(_) => self.state(at, |s| s.lang = true),
            PseudoClass::Dir(_) => self.state(at, |s| s.dir = true),
            PseudoClass::Root
            | PseudoClass::Scope
            | PseudoClass::Visited
            | PseudoClass::State(_) => {}
        }
    }
}

fn attribute_value<'a>(element: DomElement<'a>, local: &str) -> Option<&'a [u16]> {
    let document = element.document();
    match document.node(element.id()).ok()?.kind() {
        NodeKind::Element(e) => e
            .attributes()
            .iter()
            .find(|a| {
                a.name().namespace().is_none() && a.name().local().eq_ignore_ascii_case(local)
            })
            .map(|a| a.value().as_units()),
        _ => None,
    }
}

fn units_eq_ci(units: &[u16], keyword: &str) -> bool {
    units.len() == keyword.len()
        && units
            .iter()
            .zip(keyword.bytes())
            .all(|(&u, b)| u8::try_from(u).is_ok_and(|u| u.eq_ignore_ascii_case(&b)))
}

/// A DOM change, reported after it was made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Mutation {
    /// An attribute of `element` was set, changed or removed.
    Attribute {
        /// The element.
        element: NodeId,
        /// The attribute's local name (compared ASCII case-insensitively).
        local: String,
    },
    /// Nodes were inserted into, removed from or moved within `parent`
    /// (an element, shadow root, document or fragment).
    Children {
        /// The parent.
        parent: NodeId,
    },
    /// The data of a text node (or another character-data node) changed.
    /// The text of a `<style>` element changes a stylesheet as well: rebuild
    /// the cascade and report [`StyledDocument::cascade_changed`].
    Text {
        /// The node.
        node: NodeId,
    },
    /// Anything else, such as a changed stylesheet or a newly attached
    /// shadow root: restyle everything.
    Everything,
}

/// Restyle statistics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RestyleStats {
    /// Elements that went through the cascade again.
    pub recomputed: usize,
    /// Elements whose earlier style was reused.
    pub reused: usize,
}

/// A document's styles, kept up to date incrementally.
#[derive(Clone, Debug)]
pub struct StyledDocument {
    styles: Vec<ElementStyle>,
    parents: Vec<Option<NodeId>>,
    index: HashMap<NodeId, usize>,
    map: InvalidationMap,
    dirty: Dirty,
    sizes: HashMap<NodeId, ContainerSize>,
    /// The `<meta http-equiv=content-language>` values in tree order, kept
    /// when `:lang()` is used: inserting or removing one changes the
    /// default language of every element.
    pragmas: Vec<Vec<u16>>,
}

impl StyledDocument {
    /// Style the whole document (as [`crate::style_tree`] from its root).
    pub fn new<'a>(
        document: &'a Document,
        cascade: &Cascade,
        parser: &CssParser,
        cx: &MatchContext<DomElement<'a>>,
        compute: &ComputeContext,
    ) -> Result<Self, DomError> {
        let walked = style_walk(
            document,
            document.root(),
            cascade,
            parser,
            cx,
            (compute, &HashMap::new()),
            None,
            None,
        )?;
        let mut styled = Self {
            styles: Vec::new(),
            parents: Vec::new(),
            index: HashMap::new(),
            map: InvalidationMap::new(cascade),
            dirty: Dirty::default(),
            sizes: HashMap::new(),
            pragmas: Vec::new(),
        };
        styled.pragmas = styled.current_pragmas(document);
        styled.store(walked.styles, walked.parents);
        Ok(styled)
    }

    fn store(&mut self, styles: Vec<ElementStyle>, parents: Vec<Option<NodeId>>) {
        self.index = styles
            .iter()
            .enumerate()
            .map(|(i, s)| (s.node, i))
            .collect();
        self.styles = styles;
        self.parents = parents;
    }

    /// Every element's style, in the order [`crate::style_tree`] returns them.
    pub fn styles(&self) -> &[ElementStyle] {
        &self.styles
    }

    /// The style of `element`, if it was styled.
    pub fn style(&self, element: NodeId) -> Option<&ElementStyle> {
        self.index.get(&element).map(|&i| &self.styles[i])
    }

    /// Whether any mutation since the last restyle needs work.
    pub fn needs_restyle(&self) -> bool {
        !self.dirty.is_clean()
    }

    /// Record that the cascade changed (sheets added or removed, or a
    /// different media environment with [`Cascade::set_environment`]
    /// reporting a change): everything is restyled, and the selector
    /// dependencies are rebuilt from `cascade`.
    pub fn cascade_changed(&mut self, cascade: &Cascade) {
        self.map = InvalidationMap::new(cascade);
        self.dirty.all = true;
    }

    fn uses_lang(&self) -> bool {
        self.map.states.lang || self.map.has.states.lang
    }

    fn uses_dir(&self) -> bool {
        self.map.states.dir || self.map.has.states.dir
    }

    fn uses_form(&self) -> bool {
        self.map.states.form || self.map.has.states.form
    }

    /// The pragma-set language candidates (empty when `:lang()` is unused).
    fn current_pragmas(&self, document: &Document) -> Vec<Vec<u16>> {
        if !self.uses_lang() {
            return Vec::new();
        }
        let Ok(walk) = document.walk(document.root()) else {
            return Vec::new();
        };
        walk.filter_map(|(node, _)| {
            let element = DomElement::new(document, node)?;
            let is_pragma = element.local_name().eq_ignore_ascii_case("meta")
                && attribute_value(element, "http-equiv")
                    .is_some_and(|v| units_eq_ci(v, "content-language"));
            is_pragma.then(|| attribute_value(element, "content").unwrap_or(&[]).to_vec())
        })
        .collect()
    }

    /// Mark `element` as changed: itself, or with `outside` its subtree and
    /// (with sibling combinators) its following siblings' subtrees.
    fn changed(&mut self, document: &Document, element: NodeId, outside: bool, siblings: bool) {
        if outside {
            self.dirty.subtrees.insert(element);
            if siblings {
                self.following_siblings(document, element);
            }
        } else {
            self.dirty.elements.insert(element);
        }
    }

    /// The `:has()` anchors whose result a change at `element` can flip:
    /// its ancestors as far as a relative selector reaches, with their
    /// preceding siblings when relative selectors use sibling combinators
    /// (`element` itself is included, conservatively). With `deep`, the
    /// change can also flip what matches anywhere in `element`'s subtree
    /// and its following siblings' subtrees, which are marked as well.
    fn has_anchors(&mut self, document: &Document, element: NodeId, deep: bool) {
        if !self.map.has.any {
            return;
        }
        if deep {
            self.dirty.subtrees.insert(element);
            if self.map.has.siblings {
                self.following_siblings(document, element);
            }
        }
        let outside = self.map.has.outside_subject;
        let siblings = self.map.siblings;
        let mut node = DomElement::new(document, element);
        let mut level = 0usize;
        while let Some(current) = node {
            if self.map.has.reach.is_some_and(|reach| level > reach) {
                break;
            }
            self.changed(document, current.id(), outside, siblings);
            if self.map.has.siblings {
                let mut before = current.previous_sibling_element();
                while let Some(sibling) = before {
                    self.changed(document, sibling.id(), outside, siblings);
                    before = sibling.previous_sibling_element();
                }
            }
            node = current.parent_element();
            level += 1;
        }
    }

    /// Element states at `element` may have changed (with `subtree`, also
    /// anywhere below it).
    fn state_changed(&mut self, document: &Document, element: NodeId, subtree: bool) {
        if self.map.states.any() {
            let outside = self.map.states_outside_subject;
            self.changed(
                document,
                element,
                outside || subtree,
                outside && self.map.siblings,
            );
        }
        if self.map.has.states.any() {
            if self.map.has.deep_states {
                self.dirty.all = true;
            }
            self.has_anchors(document, element, subtree);
        }
    }

    /// The outermost `dir=auto` (or `bdi`) element at or above `node`:
    /// its directionality depends on the text and `dir` attributes below it.
    fn auto_direction_root(document: &Document, node: NodeId) -> Option<NodeId> {
        let mut current = DomElement::new(document, node);
        let mut outermost = None;
        while let Some(element) = current {
            let auto = element.local_name().eq_ignore_ascii_case("bdi")
                || attribute_value(element, "dir").is_some_and(|v| units_eq_ci(v, "auto"));
            if auto {
                outermost = Some(element.id());
            }
            current = element.parent_element();
        }
        outermost
    }

    /// Form controls whose state a group-level change can flip: radio
    /// groups, default buttons and selected options.
    fn form_group_changed(&mut self, document: &Document) {
        let Ok(walk) = document.walk(document.root()) else {
            self.dirty.all = true;
            return;
        };
        let controls: Vec<NodeId> = walk
            .filter_map(|(node, _)| {
                let element = DomElement::new(document, node)?;
                FORM_GROUP_ELEMENTS
                    .iter()
                    .any(|name| element.local_name().eq_ignore_ascii_case(name))
                    .then_some(node)
            })
            .collect();
        for control in controls {
            self.state_changed(document, control, false);
        }
    }

    /// Text or children changed under `parent`: the states that read
    /// descendants (`dir=auto`, a textarea's placeholder, a fieldset's
    /// legend, a select's options, radio groups and default buttons).
    fn content_changed(&mut self, document: &Document, parent: NodeId, children: bool) {
        if self.uses_dir() {
            if let Some(root) = Self::auto_direction_root(document, parent) {
                self.state_changed(document, root, true);
            }
        }
        if self.uses_form() {
            match DomElement::new(document, parent).map(|e| e.local_name().to_ascii_lowercase()) {
                // Its placeholder shows only while it has no text.
                Some(name) if name == "textarea" => self.state_changed(document, parent, false),
                // A fieldset's first legend escapes its `disabled`, and a
                // select's options decide which one is selected.
                Some(name) if matches!(name.as_str(), "fieldset" | "select" | "optgroup") => {
                    if let Ok(children) = document.children(parent) {
                        let children: Vec<NodeId> = children
                            .filter(|&c| DomElement::new(document, c).is_some())
                            .collect();
                        for child in children {
                            self.state_changed(document, child, true);
                        }
                    }
                }
                _ => {}
            }
            if children {
                self.form_group_changed(document);
            }
        }
        if children && self.uses_lang() {
            let pragmas = self.current_pragmas(document);
            if pragmas != self.pragmas {
                self.pragmas = pragmas;
                self.dirty.all = true;
            }
        }
    }

    fn state_attribute_changed(&mut self, document: &Document, element: NodeId, local: &str) {
        if self.uses_lang() {
            match local {
                "lang" => self.state_changed(document, element, true),
                "http-equiv" | "content" => {
                    let pragmas = self.current_pragmas(document);
                    if pragmas != self.pragmas {
                        self.pragmas = pragmas;
                        self.dirty.all = true;
                    }
                }
                _ => {}
            }
        }
        if self.uses_dir() && matches!(local, "dir" | "type" | "value") {
            self.state_changed(document, element, true);
            let parent = document.node(element).ok().and_then(|n| n.parent());
            if let Some(root) = parent.and_then(|p| Self::auto_direction_root(document, p)) {
                self.state_changed(document, root, true);
            }
        }
        if self.uses_form() && FORM_ATTRIBUTES.contains(&local) {
            // `disabled` on a fieldset, `contenteditable` and a select's
            // `multiple` or `size` reach descendants.
            self.state_changed(document, element, true);
            if FORM_GROUP_ATTRIBUTES.contains(&local) {
                self.form_group_changed(document);
            }
        }
    }

    /// Mark everything at and after `element` among its siblings, with
    /// subtrees.
    fn following_siblings(&mut self, document: &Document, element: NodeId) {
        let mut current = document.node(element).ok().and_then(|n| n.next_sibling());
        while let Some(id) = current {
            if matches!(
                document.node(id).map(|n| n.kind()),
                Ok(NodeKind::Element(_))
            ) {
                self.dirty.subtrees.insert(id);
            }
            current = document.node(id).ok().and_then(|n| n.next_sibling());
        }
    }

    fn attribute_changed(&mut self, document: &Document, element: NodeId, local: &str) {
        let local = local.to_ascii_lowercase();
        if local == "style" {
            self.dirty.elements.insert(element);
        }
        self.state_attribute_changed(document, element, &local);
        if let Some(&deep) = self.map.has.attributes.get(&local) {
            self.has_anchors(document, element, deep);
        }
        if let Some(&outside_subject) = self.map.attributes.get(&local) {
            let siblings = self.map.siblings;
            self.changed(document, element, outside_subject, siblings);
        }
    }

    /// Record a mutation already made to `document`.
    pub fn note(&mut self, document: &Document, mutation: Mutation) {
        // A filter may depend on ancestors, preceding siblings or :has(), so
        // its membership can change outside the mutation's sibling list.
        // Keep the reference fallback until those dependencies carry their
        // counting scope (TD-36).
        // Scope membership and proximity can change after any mutation
        // affecting a root or limit selector. Full restyle is the reference
        // path until those selectors have dedicated invalidation (TD-119).
        if self.map.nth_filter || self.map.scoped_rules {
            self.dirty.all = true;
        }
        match mutation {
            Mutation::Everything => self.dirty.all = true,
            Mutation::Attribute { element, local } => {
                self.attribute_changed(document, element, &local);
            }
            Mutation::Children { parent } => {
                self.dirty.children = true;
                // Inserted and moved elements are found by the walk, since
                // their style parent changed or they have no style yet.
                if self.map.structural || self.map.siblings {
                    if let Ok(children) = document.children(parent) {
                        for child in children {
                            if matches!(
                                document.node(child).map(|n| n.kind()),
                                Ok(NodeKind::Element(_))
                            ) {
                                if self.map.siblings || self.map.structural_outside_subject {
                                    self.dirty.subtrees.insert(child);
                                } else {
                                    self.dirty.elements.insert(child);
                                }
                            }
                        }
                    }
                    self.changed(
                        document,
                        parent,
                        self.map.structural_outside_subject,
                        self.map.siblings,
                    );
                }
                self.content_changed(document, parent, true);
                if self.map.has.any {
                    // The parent (`:empty`, `> x`) and its remaining
                    // children (`+ x`, `~ x`, `:nth-child()` in arguments)
                    // may be or lead to anchors.
                    self.has_anchors(document, parent, false);
                    if let Ok(children) = document.children(parent) {
                        let children: Vec<NodeId> = children.collect();
                        for child in children {
                            if DomElement::new(document, child).is_some() {
                                self.has_anchors(document, child, false);
                            }
                        }
                    }
                }
            }
            Mutation::Text { node } => {
                let parent = document.node(node).ok().and_then(|n| n.parent());
                if let Some(parent) = parent {
                    if self.map.structural_outside_subject {
                        self.changed(document, parent, true, self.map.siblings);
                    } else if self.map.structural {
                        self.dirty.elements.insert(parent);
                    }
                    self.content_changed(document, parent, false);
                    self.has_anchors(document, parent, false);
                }
            }
        }
    }

    /// Recompute what the noted mutations may have changed. `cascade` must
    /// be the cascade of the last [`Self::new`] or [`Self::cascade_changed`].
    pub fn restyle<'a>(
        &mut self,
        document: &'a Document,
        cascade: &Cascade,
        parser: &CssParser,
        cx: &MatchContext<DomElement<'a>>,
        compute: &ComputeContext,
    ) -> Result<RestyleStats, DomError> {
        if self.dirty.is_clean() {
            return Ok(RestyleStats {
                recomputed: 0,
                reused: self.styles.len(),
            });
        }
        let previous: HashMap<NodeId, (ElementStyle, Option<NodeId>)> =
            std::mem::take(&mut self.styles)
                .into_iter()
                .zip(std::mem::take(&mut self.parents))
                .map(|(style, parent)| (style.node, (style, parent)))
                .collect();
        let dirty = std::mem::take(&mut self.dirty);
        let reuse = Reuse {
            previous,
            dirty: &dirty,
        };
        let walked = style_walk(
            document,
            document.root(),
            cascade,
            parser,
            cx,
            (compute, &self.sizes),
            Some(reuse),
            None,
        )?;
        let stats = RestyleStats {
            recomputed: walked.recomputed,
            reused: walked.styles.len() - walked.recomputed,
        };
        self.store(walked.styles, walked.parents);
        self.pragmas = self.current_pragmas(document);
        Ok(stats)
    }

    /// Whether these styles equal a fresh [`crate::style_tree`] walk (for tests and
    /// fuzzing).
    pub fn matches_full_restyle<'a>(
        &self,
        document: &'a Document,
        cascade: &Cascade,
        parser: &CssParser,
        cx: &MatchContext<DomElement<'a>>,
        compute: &ComputeContext,
    ) -> Result<bool, DomError> {
        let full = style_tree_with_sizes(
            document,
            document.root(),
            cascade,
            parser,
            cx,
            compute,
            &self.sizes,
        )?;
        Ok(full == self.styles)
    }

    /// Record `element`'s content-box size from layout (or forget it with
    /// `None`) for `@container` size features. A change restyles the
    /// element's subtree on the next [`Self::restyle`].
    pub fn set_container_size(&mut self, element: NodeId, size: Option<ContainerSize>) {
        let old = match size {
            Some(size) => self.sizes.insert(element, size),
            None => self.sizes.remove(&element),
        };
        if old != size {
            self.dirty.subtrees.insert(element);
        }
    }

    /// The container sizes recorded with [`Self::set_container_size`].
    pub fn container_sizes(&self) -> &HashMap<NodeId, ContainerSize> {
        &self.sizes
    }
}
