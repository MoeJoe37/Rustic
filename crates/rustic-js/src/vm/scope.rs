//! Scope analysis before code generation (JS-04).
//!
//! The resolver walks a function body once, records every scope with its
//! declarations, and finds which bindings a nested function (or a `with`
//! body) can see. Those *captured* bindings live in heap environments so
//! closures can share them; every other binding lives in a register of
//! its frame, which costs no allocation and no lookup. Scopes are keyed by
//! the address of the AST node that introduces them, so code generation
//! finds them without re-deriving the walk order.

use std::any::TypeId;
use std::collections::HashMap;

use crate::ast::{
    Argument, ArrayElement, ArrowBody, ArrowFunction, Class, ClassElement, Expression,
    ExpressionKind, ForHead, ForInit, Function, MemberProperty, MethodKind, ObjectProperty,
    Pattern, PatternKind, PropertyKey, Statement, StatementKind, VariableKind,
};

/// A scope in a [`ScopeTree`].
pub(crate) type ScopeId = usize;

/// How a binding was declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingKind {
    /// `var`, or a function declaration at function top level.
    Var,
    /// A parameter.
    Param,
    /// `let`, or a block-level function declaration.
    Let,
    /// `const`.
    Const,
    /// A catch parameter.
    Catch,
    /// The implicit `arguments` object.
    Arguments,
    /// A named function expression's own name (immutable).
    SelfName,
    /// `this` of a derived class constructor: uninitialized until
    /// `super()` returns, and shared with the arrow functions inside.
    This,
    /// `new.target` of a function whose arrow functions read it.
    NewTarget,
    /// A private field's name (`#x`): the binding holds its private name.
    PrivateField,
    /// A private method: the binding holds the function.
    PrivateMethod,
    /// A private accessor: the binding holds the getter, and the binding
    /// [`private_setter_name`] names holds the setter.
    PrivateAccessor,
}

/// [`Binding::flags`] of a private element: it is static.
pub(crate) const PRIVATE_STATIC: u8 = 1;
/// A private accessor has a getter.
pub(crate) const PRIVATE_GETTER: u8 = 2;
/// A private accessor has a setter.
pub(crate) const PRIVATE_SETTER: u8 = 4;

/// The class-scope binding of the private element `#name`.
pub(crate) fn private_name(name: &str) -> String {
    format!("#{name}")
}

/// The class-scope binding of the setter of the private accessor
/// `#name`.
pub(crate) fn private_setter_name(name: &str) -> String {
    format!("#{name}=")
}

/// The binding of a derived constructor's `this`. Reserved words cannot
/// name script bindings, so these names never collide with one.
pub(crate) const THIS: &str = "this";
/// The binding of `new.target` for arrow functions.
pub(crate) const NEW_TARGET: &str = "new.target";
/// A class scope's binding of the class constructor itself, from which
/// `super` finds the parent class and the home objects.
pub(crate) const CLASS: &str = "super";
/// A class scope's binding of the function that initializes the instance
/// fields; `used` only when the class has any. `%` cannot appear in a
/// script identifier or a private name.
pub(crate) const FIELD_INIT: &str = "%init";
/// A class scope's binding of the private name every instance gets when
/// the class has private methods or accessors: their brand.
pub(crate) const BRAND: &str = "%brand";

/// The class-scope binding that keeps the computed key of the instance
/// field at `index` in the class body from definition to construction.
pub(crate) fn field_key_name(index: usize) -> String {
    format!("%key{index}")
}

impl BindingKind {
    /// Whether reads need a temporal-dead-zone check.
    pub(crate) fn has_tdz(self) -> bool {
        matches!(
            self,
            BindingKind::Let | BindingKind::Const | BindingKind::This
        )
    }
}

/// One binding of a scope.
#[derive(Clone, Debug)]
pub(crate) struct Binding {
    pub name: Box<str>,
    pub kind: BindingKind,
    /// Seen from a nested function or `with` body: lives in the scope's
    /// environment, not in a register.
    pub captured: bool,
    /// Referenced at all (decides whether `arguments` is materialized).
    pub used: bool,
    /// `PRIVATE_*` bits of a private element.
    pub flags: u8,
}

/// What introduced a scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    /// The top level of a script: its declarations are globals, found by
    /// name.
    Script,
    /// A function's parameters and top-level declarations.
    Function,
    /// A block, loop head, switch or catch clause.
    Block,
    /// A `with` body: names inside resolve dynamically.
    With,
}

/// A scope and its bindings.
#[derive(Clone, Debug)]
pub(crate) struct Scope {
    pub kind: ScopeKind,
    pub parent: Option<ScopeId>,
    /// The function scope (or script scope) this scope belongs to.
    pub function: ScopeId,
    pub bindings: Vec<Binding>,
    /// Strict code.
    pub strict: bool,
    /// An arrow function's scope: `this` and `new.target` come from
    /// outside.
    pub arrow: bool,
}

impl Scope {
    /// Whether entering this scope creates a heap environment.
    pub(crate) fn has_env(&self) -> bool {
        match self.kind {
            ScopeKind::Script => false,
            ScopeKind::With => true,
            ScopeKind::Function | ScopeKind::Block => self.bindings.iter().any(|b| b.captured),
        }
    }

    pub(crate) fn find(&self, name: &str) -> Option<usize> {
        self.bindings.iter().position(|b| &*b.name == name)
    }
}

/// Every scope of one script.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScopeTree {
    pub scopes: Vec<Scope>,
    /// AST node (address and type) → the scope it introduces.
    by_node: HashMap<NodeKey, ScopeId>,
}

/// An AST node's identity: its address, its type and which of its scopes
/// is meant. The type is part of the key because a struct and its first
/// field can share an address; the role tells apart the scopes one node
/// introduces (a `try` statement's block and its `finally`).
type NodeKey = (usize, TypeId, u8);

/// The scope a node introduces by itself.
pub(crate) const ROLE_SELF: u8 = 0;
/// A `try` statement's protected block, or a catch clause's body.
pub(crate) const ROLE_BODY: u8 = 1;
/// A `try` statement's `finally` block.
pub(crate) const ROLE_FINALLY: u8 = 2;
/// A class's instance field initializers, which run as one function.
pub(crate) const ROLE_FIELDS: u8 = 3;

fn node_key<T: 'static>(node: &T, role: u8) -> NodeKey {
    (std::ptr::from_ref(node) as usize, TypeId::of::<T>(), role)
}

impl ScopeTree {
    /// The scope `node` introduced, if any.
    pub(crate) fn scope_of<T: 'static>(&self, node: &T) -> Option<ScopeId> {
        self.scope_of_role(node, ROLE_SELF)
    }

    /// One of the scopes `node` introduced.
    pub(crate) fn scope_of_role<T: 'static>(&self, node: &T, role: u8) -> Option<ScopeId> {
        self.by_node.get(&node_key(node, role)).copied()
    }

    /// Analyze a script.
    pub(crate) fn analyze_script(body: &[Statement], strict: bool) -> ScopeTree {
        let mut r = Resolver {
            tree: ScopeTree::default(),
            stack: Vec::new(),
            derived_constructor: false,
        };
        let script = r.open(ScopeKind::Script, None, strict);
        // Top-level declarations are globals, found by name; only block
        // scopes inside get bindings.
        r.statements(body);
        r.stack.pop();
        debug_assert_eq!(script, 0);
        r.tree
    }

    /// Resolve `name` from `scope`: the scope holding it and the binding
    /// index, or `None` for a global or dynamic name. `crosses_with`
    /// reports a `with` scope on the way, which forces a dynamic lookup.
    pub(crate) fn resolve(&self, scope: ScopeId, name: &str) -> Resolution {
        let mut current = Some(scope);
        while let Some(id) = current {
            let s = &self.scopes[id];
            if s.kind == ScopeKind::With {
                return Resolution::Dynamic;
            }
            if let Some(index) = s.find(name) {
                return Resolution::Binding { scope: id, index };
            }
            current = s.parent;
        }
        Resolution::Dynamic
    }

    /// Resolve one of the bindings the compiler makes itself ([`THIS`],
    /// [`NEW_TARGET`], [`CLASS`]). They are lexical, so a `with` cannot
    /// hide them; `this` and `new.target` stop at the nearest function
    /// that is not an arrow function.
    pub(crate) fn resolve_special(&self, scope: ScopeId, name: &str) -> Option<(ScopeId, usize)> {
        let mut current = Some(scope);
        while let Some(id) = current {
            let s = &self.scopes[id];
            if let Some(index) = s.find(name) {
                return Some((id, index));
            }
            if s.kind == ScopeKind::Function && !s.arrow && matches!(name, THIS | NEW_TARGET) {
                return None;
            }
            current = s.parent;
        }
        None
    }
}

/// Where a name lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// A binding of a known scope.
    Binding { scope: ScopeId, index: usize },
    /// A global, or a name under a `with`: looked up by name at run time.
    Dynamic,
}

struct Resolver {
    tree: ScopeTree,
    stack: Vec<ScopeId>,
    /// The next function body is a derived class constructor.
    derived_constructor: bool,
}

impl Resolver {
    fn current(&self) -> ScopeId {
        *self.stack.last().expect("inside a scope")
    }

    fn open(&mut self, kind: ScopeKind, key: Option<NodeKey>, strict: bool) -> ScopeId {
        let parent = self.stack.last().copied();
        let id = self.tree.scopes.len();
        let function = match kind {
            ScopeKind::Script | ScopeKind::Function => id,
            _ => self.tree.scopes[parent.expect("blocks have a parent")].function,
        };
        let strict = strict || parent.is_some_and(|p| self.tree.scopes[p].strict);
        self.tree.scopes.push(Scope {
            kind,
            parent,
            function,
            bindings: Vec::new(),
            strict,
            arrow: false,
        });
        if let Some(key) = key {
            self.tree.by_node.insert(key, id);
        }
        self.stack.push(id);
        id
    }

    fn declare(&mut self, scope: ScopeId, name: &str, kind: BindingKind) {
        let s = &mut self.tree.scopes[scope];
        if s.kind == ScopeKind::Script {
            return;
        }
        match s.find(name) {
            // A var or function redeclaring a parameter or var shares it.
            Some(_) => {}
            None => s.bindings.push(Binding {
                name: name.into(),
                kind,
                captured: false,
                used: false,
                flags: 0,
            }),
        }
    }

    /// Note a reference to `name` from the current scope, marking the
    /// binding captured when the reference crosses a function boundary.
    fn reference(&mut self, name: &str) {
        let from = self.current();
        let from_function = self.tree.scopes[from].function;
        let mut current = Some(from);
        let mut through_with = false;
        while let Some(id) = current {
            let s = &self.tree.scopes[id];
            if s.kind == ScopeKind::With {
                through_with = true;
            }
            if let Some(index) = s.find(name) {
                let crosses = s.function != from_function || through_with;
                let binding = &mut self.tree.scopes[id].bindings[index];
                binding.used = true;
                binding.captured |= crosses;
                return;
            }
            current = s.parent;
        }
    }

    /// Note a use of a compiler-made binding (see
    /// [`ScopeTree::resolve_special`]). `new.target` is declared on demand
    /// in the nearest non-arrow function when an arrow function reads it.
    fn reference_special(&mut self, name: &str) {
        let from = self.current();
        let from_function = self.tree.scopes[from].function;
        let mut current = Some(from);
        while let Some(id) = current {
            let s = &self.tree.scopes[id];
            if let Some(index) = s.find(name) {
                let crosses = s.function != from_function;
                let binding = &mut self.tree.scopes[id].bindings[index];
                binding.used = true;
                binding.captured |= crosses;
                return;
            }
            if s.kind == ScopeKind::Function && !s.arrow && matches!(name, THIS | NEW_TARGET) {
                if name == NEW_TARGET && id != from_function {
                    self.tree.scopes[id].bindings.push(Binding {
                        name: name.into(),
                        kind: BindingKind::NewTarget,
                        captured: true,
                        used: true,
                        flags: 0,
                    });
                }
                return;
            }
            current = s.parent;
        }
    }

    /// Declare the private names of `c` in its scope. Their bindings are
    /// read from the class's methods and initializers, so they live in
    /// the scope's environment. Returns whether instances get a brand.
    fn declare_private_names(&mut self, scope: ScopeId, c: &Class) -> bool {
        let mut brand = false;
        for element in &c.body {
            let (key, kind, mut flags) = match element {
                ClassElement::Method {
                    key,
                    kind,
                    is_static,
                    ..
                } => {
                    let (binding, bit) = match kind {
                        MethodKind::Get => (BindingKind::PrivateAccessor, PRIVATE_GETTER),
                        MethodKind::Set => (BindingKind::PrivateAccessor, PRIVATE_SETTER),
                        _ => (BindingKind::PrivateMethod, 0),
                    };
                    (key, binding, bit | (u8::from(*is_static) * PRIVATE_STATIC))
                }
                ClassElement::Field { key, is_static, .. } => (
                    key,
                    BindingKind::PrivateField,
                    u8::from(*is_static) * PRIVATE_STATIC,
                ),
                ClassElement::StaticBlock(..) => continue,
            };
            let PropertyKey::Private(id) = key else {
                continue;
            };
            brand |= kind != BindingKind::PrivateField && flags & PRIVATE_STATIC == 0;
            let name = private_name(&id.name);
            let bindings = &mut self.tree.scopes[scope].bindings;
            match bindings.iter_mut().find(|b| *b.name == *name) {
                // The other half of an accessor pair.
                Some(existing) => {
                    existing.flags |= flags;
                    flags = existing.flags;
                }
                None => bindings.push(Binding {
                    name: name.into(),
                    kind,
                    captured: true,
                    used: true,
                    flags,
                }),
            }
            if flags & PRIVATE_SETTER != 0 {
                let setter = private_setter_name(&id.name);
                if self.tree.scopes[scope].find(&setter).is_none() {
                    self.tree.scopes[scope].bindings.push(Binding {
                        name: setter.into(),
                        kind: BindingKind::Const,
                        captured: true,
                        used: true,
                        flags: 0,
                    });
                }
            }
        }
        if brand {
            self.tree.scopes[scope].bindings.push(Binding {
                name: BRAND.into(),
                kind: BindingKind::Const,
                captured: true,
                used: true,
                flags: 0,
            });
        }
        // Static private methods are checked against the class itself.
        let class_index = self.tree.scopes[scope]
            .find(CLASS)
            .expect("declared by the caller");
        let has_private = self.tree.scopes[scope]
            .bindings
            .iter()
            .any(|b| b.name.starts_with('#'));
        if has_private {
            let binding = &mut self.tree.scopes[scope].bindings[class_index];
            binding.captured = true;
            binding.used = true;
        }
        brand
    }

    /// A class: a scope for its own name and for the binding `super`
    /// resolves through, then the heritage, keys and member functions.
    fn class(&mut self, c: &Class) {
        let scope = self.open(ScopeKind::Block, Some(node_key(c, ROLE_SELF)), true);
        self.declare(scope, CLASS, BindingKind::Const);
        if let Some(id) = &c.id {
            self.declare(scope, &id.name, BindingKind::Const);
        }
        if let Some(heritage) = &c.super_class {
            self.expression(heritage);
        }
        self.declare(scope, FIELD_INIT, BindingKind::Const);
        let mut has_constructor = false;
        let mut has_instance_fields = self.declare_private_names(scope, c);
        for (index, element) in c.body.iter().enumerate() {
            match element {
                ClassElement::Method {
                    key,
                    function,
                    kind,
                    ..
                } => {
                    self.property_key(key);
                    if *kind == MethodKind::Constructor {
                        has_constructor = true;
                        self.derived_constructor = c.super_class.is_some();
                    }
                    self.function(function, false);
                }
                ClassElement::Field {
                    key,
                    value,
                    is_static,
                    ..
                } => {
                    self.property_key(key);
                    if *is_static {
                        // Each static initializer runs as its own function
                        // with the class as `this`.
                        if let Some(value) = value {
                            self.open(
                                ScopeKind::Function,
                                Some(node_key(element, ROLE_SELF)),
                                true,
                            );
                            self.expression(value);
                            self.stack.pop();
                        }
                    } else {
                        has_instance_fields = true;
                        if matches!(key, PropertyKey::Computed(_)) {
                            // Evaluated once, with the class; used at
                            // every construction.
                            self.tree.scopes[scope].bindings.push(Binding {
                                name: field_key_name(index).into(),
                                kind: BindingKind::Const,
                                captured: true,
                                used: true,
                                flags: 0,
                            });
                        }
                    }
                }
                ClassElement::StaticBlock(body, _) => {
                    self.function_body(element, &[], None, body, false, true, None);
                }
            }
        }
        if has_instance_fields {
            // The instance initializers run as one function per
            // construction, reached through the class scope.
            let index = self.tree.scopes[scope]
                .find(FIELD_INIT)
                .expect("declared above");
            let binding = &mut self.tree.scopes[scope].bindings[index];
            binding.used = true;
            binding.captured = true;
            self.open(ScopeKind::Function, Some(node_key(c, ROLE_FIELDS)), true);
            for element in &c.body {
                if let ClassElement::Field {
                    value: Some(value),
                    is_static: false,
                    ..
                } = element
                {
                    self.expression(value);
                }
            }
            self.stack.pop();
        }
        if c.super_class.is_some() && !has_constructor {
            // The default constructor reads the class to find its parent.
            let index = self.tree.scopes[scope].find(CLASS).expect("declared above");
            let binding = &mut self.tree.scopes[scope].bindings[index];
            binding.used = true;
            binding.captured = true;
        }
        self.stack.pop();
    }

    /// Capture every binding visible from the current scope (a `with`
    /// body can name any of them dynamically).
    fn capture_all_visible(&mut self) {
        let mut current = Some(self.current());
        while let Some(id) = current {
            for b in &mut self.tree.scopes[id].bindings {
                b.captured = true;
                b.used = true;
            }
            current = self.tree.scopes[id].parent;
        }
    }

    // ---- declarations ------------------------------------------------

    /// Hoist `var` names (and, at function top level, function
    /// declarations) of `body` into `scope`, without entering nested
    /// functions.
    fn hoist_vars(&mut self, scope: ScopeId, body: &[Statement], top: bool) {
        for s in body {
            self.hoist_vars_in(scope, s, top);
        }
    }

    fn hoist_vars_in(&mut self, scope: ScopeId, s: &Statement, top: bool) {
        match &s.kind {
            StatementKind::Variable(d) if d.kind == VariableKind::Var => {
                for decl in &d.declarations {
                    self.pattern_names(&decl.target, &mut |r, n| {
                        r.declare(scope, n, BindingKind::Var)
                    });
                }
            }
            StatementKind::Function(f) if top => {
                if let Some(id) = &f.id {
                    self.declare(scope, &id.name, BindingKind::Var);
                }
            }
            StatementKind::Block(b) => self.hoist_vars(scope, b, false),
            StatementKind::If {
                consequent,
                alternate,
                ..
            } => {
                self.hoist_vars_in(scope, consequent, false);
                if let Some(a) = alternate {
                    self.hoist_vars_in(scope, a, false);
                }
            }
            StatementKind::For { init, body, .. } => {
                if let Some(ForInit::Variable(d)) = init {
                    if d.kind == VariableKind::Var {
                        for decl in &d.declarations {
                            self.pattern_names(&decl.target, &mut |r, n| {
                                r.declare(scope, n, BindingKind::Var);
                            });
                        }
                    }
                }
                self.hoist_vars_in(scope, body, false);
            }
            StatementKind::ForIn { left, body, .. } | StatementKind::ForOf { left, body, .. } => {
                if let ForHead::Variable(d) = left {
                    if d.kind == VariableKind::Var {
                        for decl in &d.declarations {
                            self.pattern_names(&decl.target, &mut |r, n| {
                                r.declare(scope, n, BindingKind::Var);
                            });
                        }
                    }
                }
                self.hoist_vars_in(scope, body, false);
            }
            StatementKind::While { body, .. }
            | StatementKind::DoWhile { body, .. }
            | StatementKind::Labeled { body, .. }
            | StatementKind::With { body, .. } => self.hoist_vars_in(scope, body, false),
            StatementKind::Switch { cases, .. } => {
                for c in cases {
                    self.hoist_vars(scope, &c.body, false);
                }
            }
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => {
                self.hoist_vars(scope, block, false);
                if let Some(h) = handler {
                    self.hoist_vars(scope, &h.body, false);
                }
                if let Some(f) = finalizer {
                    self.hoist_vars(scope, f, false);
                }
            }
            _ => {}
        }
    }

    /// Declare the lexical names (`let`, `const`, block functions) of a
    /// statement list directly in `scope`.
    fn declare_lexical(&mut self, scope: ScopeId, body: &[Statement], function_top: bool) {
        for s in body {
            match &s.kind {
                StatementKind::Variable(d) if d.kind != VariableKind::Var => {
                    let kind = if d.kind == VariableKind::Const {
                        BindingKind::Const
                    } else {
                        BindingKind::Let
                    };
                    for decl in &d.declarations {
                        self.pattern_names(&decl.target, &mut |r, n| r.declare(scope, n, kind));
                    }
                }
                StatementKind::Function(f) if !function_top => {
                    if let Some(id) = &f.id {
                        self.declare(scope, &id.name, BindingKind::Let);
                    }
                }
                StatementKind::Class(c) => {
                    if let Some(id) = &c.id {
                        self.declare(scope, &id.name, BindingKind::Let);
                    }
                }
                _ => {}
            }
        }
    }

    fn pattern_names(&mut self, p: &Pattern, f: &mut dyn FnMut(&mut Self, &str)) {
        match &p.kind {
            PatternKind::Identifier(id) => f(self, &id.name),
            PatternKind::Member(_) => {}
            PatternKind::Object { properties, rest } => {
                for prop in properties {
                    self.pattern_names(&prop.value, f);
                }
                if let Some(r) = rest {
                    self.pattern_names(r, f);
                }
            }
            PatternKind::Array { elements, rest } => {
                for e in elements.iter().flatten() {
                    self.pattern_names(e, f);
                }
                if let Some(r) = rest {
                    self.pattern_names(r, f);
                }
            }
            PatternKind::Assignment { target, .. } => self.pattern_names(target, f),
        }
    }

    // ---- walking -----------------------------------------------------

    fn statements(&mut self, body: &[Statement]) {
        for s in body {
            self.statement(s);
        }
    }

    fn block<T: 'static>(&mut self, node: &T, role: u8, body: &[Statement]) {
        let scope = self.open(ScopeKind::Block, Some(node_key(node, role)), false);
        self.declare_lexical(scope, body, false);
        self.statements(body);
        self.stack.pop();
    }

    fn statement(&mut self, s: &Statement) {
        match &s.kind {
            StatementKind::Expression { expression, .. } => self.expression(expression),
            StatementKind::Block(body) => self.block(s, ROLE_SELF, body),
            StatementKind::Empty | StatementKind::Debugger => {}
            StatementKind::Variable(d) => {
                for decl in &d.declarations {
                    self.pattern(&decl.target);
                    if let Some(init) = &decl.init {
                        self.expression(init);
                    }
                }
            }
            StatementKind::Function(f) => self.function(f, false),
            StatementKind::Class(c) => self.class(c),
            StatementKind::If {
                test,
                consequent,
                alternate,
            } => {
                self.expression(test);
                self.statement(consequent);
                if let Some(a) = alternate {
                    self.statement(a);
                }
            }
            StatementKind::For {
                init,
                test,
                update,
                body,
            } => {
                let scope = self.open(ScopeKind::Block, Some(node_key(s, ROLE_SELF)), false);
                if let Some(ForInit::Variable(d)) = init {
                    if d.kind != VariableKind::Var {
                        let kind = if d.kind == VariableKind::Const {
                            BindingKind::Const
                        } else {
                            BindingKind::Let
                        };
                        for decl in &d.declarations {
                            self.pattern_names(&decl.target, &mut |r, n| {
                                r.declare(scope, n, kind);
                            });
                        }
                    }
                }
                match init {
                    Some(ForInit::Variable(d)) => {
                        for decl in &d.declarations {
                            self.pattern(&decl.target);
                            if let Some(e) = &decl.init {
                                self.expression(e);
                            }
                        }
                    }
                    Some(ForInit::Expression(e)) => self.expression(e),
                    None => {}
                }
                if let Some(t) = test {
                    self.expression(t);
                }
                if let Some(u) = update {
                    self.expression(u);
                }
                self.statement(body);
                self.stack.pop();
            }
            StatementKind::ForIn { left, right, body }
            | StatementKind::ForOf {
                left, right, body, ..
            } => {
                self.expression(right);
                let scope = self.open(ScopeKind::Block, Some(node_key(s, ROLE_SELF)), false);
                match left {
                    ForHead::Variable(d) => {
                        if d.kind != VariableKind::Var {
                            let kind = if d.kind == VariableKind::Const {
                                BindingKind::Const
                            } else {
                                BindingKind::Let
                            };
                            for decl in &d.declarations {
                                self.pattern_names(&decl.target, &mut |r, n| {
                                    r.declare(scope, n, kind);
                                });
                            }
                        }
                        for decl in &d.declarations {
                            self.pattern(&decl.target);
                        }
                    }
                    ForHead::Pattern(p) => self.pattern(p),
                }
                self.statement(body);
                self.stack.pop();
            }
            StatementKind::While { test, body } | StatementKind::DoWhile { body, test } => {
                self.expression(test);
                self.statement(body);
            }
            StatementKind::Continue(_) | StatementKind::Break(_) => {}
            StatementKind::Return(e) => {
                if let Some(e) = e {
                    self.expression(e);
                }
            }
            StatementKind::With { object, body } => {
                self.expression(object);
                self.capture_all_visible();
                self.open(ScopeKind::With, Some(node_key(s, ROLE_SELF)), false);
                self.statement(body);
                self.stack.pop();
            }
            StatementKind::Switch {
                discriminant,
                cases,
            } => {
                self.expression(discriminant);
                let scope = self.open(ScopeKind::Block, Some(node_key(s, ROLE_SELF)), false);
                for c in cases {
                    self.declare_lexical(scope, &c.body, false);
                }
                for c in cases {
                    if let Some(t) = &c.test {
                        self.expression(t);
                    }
                    self.statements(&c.body);
                }
                self.stack.pop();
            }
            StatementKind::Labeled { body, .. } => self.statement(body),
            StatementKind::Throw(e) => self.expression(e),
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => {
                self.block(s, ROLE_BODY, block);
                if let Some(h) = handler {
                    let scope = self.open(ScopeKind::Block, Some(node_key(h, ROLE_SELF)), false);
                    if let Some(p) = &h.param {
                        self.pattern_names(p, &mut |r, n| r.declare(scope, n, BindingKind::Catch));
                        self.pattern(p);
                    }
                    self.block(h, ROLE_BODY, &h.body);
                    self.stack.pop();
                }
                if let Some(f) = finalizer {
                    self.block(s, ROLE_FINALLY, f);
                }
            }
            StatementKind::Import(_) | StatementKind::Export(_) => {}
        }
    }

    fn pattern(&mut self, p: &Pattern) {
        match &p.kind {
            PatternKind::Identifier(id) => self.reference(&id.name),
            PatternKind::Member(e) => self.expression(e),
            PatternKind::Object { properties, rest } => {
                for prop in properties {
                    if let PropertyKey::Computed(e) = &prop.key {
                        self.expression(e);
                    }
                    self.pattern(&prop.value);
                }
                if let Some(r) = rest {
                    self.pattern(r);
                }
            }
            PatternKind::Array { elements, rest } => {
                for e in elements.iter().flatten() {
                    self.pattern(e);
                }
                if let Some(r) = rest {
                    self.pattern(r);
                }
            }
            PatternKind::Assignment { target, default } => {
                self.pattern(target);
                self.expression(default);
            }
        }
    }

    #[allow(clippy::too_many_arguments, reason = "the parts of one function node")]
    fn function_body(
        &mut self,
        node: &(impl Sized + 'static),
        params: &[Pattern],
        rest: Option<&Pattern>,
        body: &[Statement],
        arrow: bool,
        strict: bool,
        self_name: Option<&str>,
    ) {
        let scope = self.open(ScopeKind::Function, Some(node_key(node, ROLE_SELF)), strict);
        self.tree.scopes[scope].arrow = arrow;
        if std::mem::take(&mut self.derived_constructor) {
            self.declare(scope, THIS, BindingKind::This);
        }
        if let Some(name) = self_name {
            self.declare(scope, name, BindingKind::SelfName);
        }
        for p in params.iter().chain(rest) {
            self.pattern_names(p, &mut |r, n| {
                // A parameter shadows the function's own name.
                if let Some(i) = r.tree.scopes[scope].find(n) {
                    if r.tree.scopes[scope].bindings[i].kind == BindingKind::SelfName {
                        r.tree.scopes[scope].bindings[i].kind = BindingKind::Param;
                        return;
                    }
                }
                r.declare(scope, n, BindingKind::Param);
            });
        }
        if !arrow && self.tree.scopes[scope].find("arguments").is_none() {
            self.declare(scope, "arguments", BindingKind::Arguments);
        }
        self.hoist_vars(scope, body, true);
        // A var or function named like the function itself replaces it.
        if let Some(name) = self_name {
            if let Some(i) = self.tree.scopes[scope].find(name) {
                if self.tree.scopes[scope].bindings[i].kind == BindingKind::SelfName
                    && self.declares_var(body, name)
                {
                    self.tree.scopes[scope].bindings[i].kind = BindingKind::Var;
                }
            }
        }
        self.declare_lexical(scope, body, true);
        for p in params.iter().chain(rest) {
            self.pattern(p);
        }
        self.statements(body);
        self.stack.pop();
    }

    fn declares_var(&mut self, body: &[Statement], name: &str) -> bool {
        let probe = self.tree.scopes.len();
        // Hoist into a scratch scope and look.
        self.tree.scopes.push(Scope {
            kind: ScopeKind::Function,
            parent: None,
            function: probe,
            bindings: Vec::new(),
            strict: false,
            arrow: false,
        });
        self.hoist_vars(probe, body, true);
        let found = self.tree.scopes[probe].find(name).is_some();
        self.tree.scopes.pop();
        found
    }

    fn function(&mut self, f: &Function, expression: bool) {
        let self_name = if expression {
            f.id.as_ref().map(|i| &*i.name)
        } else {
            None
        };
        self.function_body(
            f,
            &f.params,
            f.rest.as_ref(),
            &f.body,
            false,
            f.strict,
            self_name,
        );
    }

    fn arrow(&mut self, f: &ArrowFunction) {
        match &f.body {
            ArrowBody::Block(body) => {
                self.function_body(f, &f.params, f.rest.as_ref(), body, true, f.strict, None);
            }
            ArrowBody::Expression(e) => {
                let scope = self.open(ScopeKind::Function, Some(node_key(f, ROLE_SELF)), f.strict);
                self.tree.scopes[scope].arrow = true;
                for p in f.params.iter().chain(f.rest.as_ref()) {
                    self.pattern_names(p, &mut |r, n| r.declare(scope, n, BindingKind::Param));
                }
                for p in f.params.iter().chain(f.rest.as_ref()) {
                    self.pattern(p);
                }
                self.expression(e);
                self.stack.pop();
            }
        }
    }

    fn property_key(&mut self, key: &PropertyKey) {
        if let PropertyKey::Computed(e) = key {
            self.expression(e);
        }
    }

    fn expression(&mut self, e: &Expression) {
        // Left-nested chains can be very deep: walk their spine with an
        // explicit stack.
        let mut pending: Vec<&Expression> = vec![e];
        while let Some(e) = pending.pop() {
            match &e.kind {
                ExpressionKind::Identifier(id) => self.reference(&id.name),
                ExpressionKind::This => self.reference_special(THIS),
                ExpressionKind::NewTarget => self.reference_special(NEW_TARGET),
                ExpressionKind::Null
                | ExpressionKind::Boolean(_)
                | ExpressionKind::Number(_)
                | ExpressionKind::BigInt(..)
                | ExpressionKind::String(_)
                | ExpressionKind::RegExp { .. }
                | ExpressionKind::ImportMeta => {}
                ExpressionKind::Template(t) => pending.extend(t.expressions.iter()),
                ExpressionKind::TaggedTemplate { tag, template } => {
                    pending.push(tag);
                    pending.extend(template.expressions.iter());
                }
                ExpressionKind::Array(elements) => {
                    for el in elements.iter().flatten() {
                        match el {
                            ArrayElement::Expression(x) | ArrayElement::Spread(x) => {
                                pending.push(x);
                            }
                        }
                    }
                }
                ExpressionKind::Object(props) => {
                    for p in props {
                        match p {
                            ObjectProperty::Property { key, value, .. } => {
                                self.property_key(key);
                                pending.push(value);
                            }
                            ObjectProperty::Method { key, function, .. } => {
                                self.property_key(key);
                                self.function(function, false);
                            }
                            ObjectProperty::Spread(x) => pending.push(x),
                        }
                    }
                }
                ExpressionKind::Function(f) => self.function(f, true),
                ExpressionKind::Arrow(f) => self.arrow(f),
                ExpressionKind::Class(c) => self.class(c),
                ExpressionKind::Unary { argument, .. } | ExpressionKind::Await(argument) => {
                    pending.push(argument);
                }
                ExpressionKind::Update { target, .. } => self.pattern(target),
                ExpressionKind::Binary { left, right, .. }
                | ExpressionKind::Logical { left, right, .. } => {
                    pending.push(right);
                    pending.push(left);
                }
                ExpressionKind::PrivateIn { right, .. } => pending.push(right),
                ExpressionKind::Conditional {
                    test,
                    consequent,
                    alternate,
                } => {
                    pending.push(alternate);
                    pending.push(consequent);
                    pending.push(test);
                }
                ExpressionKind::Assignment { target, value, .. } => {
                    self.pattern(target);
                    pending.push(value);
                }
                ExpressionKind::Sequence(items) => pending.extend(items.iter()),
                ExpressionKind::Member {
                    object, property, ..
                } => {
                    if let MemberProperty::Computed(k) = property {
                        pending.push(k);
                    }
                    pending.push(object);
                }
                ExpressionKind::SuperMember(property) => {
                    self.reference_special(CLASS);
                    self.reference_special(THIS);
                    if let MemberProperty::Computed(k) = property {
                        pending.push(k);
                    }
                }
                ExpressionKind::Call {
                    callee, arguments, ..
                }
                | ExpressionKind::New { callee, arguments } => {
                    for a in arguments {
                        match a {
                            Argument::Expression(x) | Argument::Spread(x) => pending.push(x),
                        }
                    }
                    pending.push(callee);
                }
                ExpressionKind::SuperCall(arguments) => {
                    self.reference_special(CLASS);
                    self.reference_special(THIS);
                    self.reference_special(NEW_TARGET);
                    for a in arguments {
                        match a {
                            Argument::Expression(x) | Argument::Spread(x) => pending.push(x),
                        }
                    }
                }
                ExpressionKind::OptionalChain(x) | ExpressionKind::Parenthesized(x) => {
                    pending.push(x);
                }
                ExpressionKind::ImportCall { source, options } => {
                    pending.push(source);
                    if let Some(o) = options {
                        pending.push(o);
                    }
                }
                ExpressionKind::Yield { argument, .. } => {
                    if let Some(a) = argument {
                        pending.push(a);
                    }
                }
            }
        }
    }
}
