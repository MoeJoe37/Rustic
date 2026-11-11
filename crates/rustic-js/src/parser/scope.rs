//! Declaration scopes and the early errors they decide: duplicate lexical
//! declarations, lexical/var conflicts (ECMA-262 §14.2.1.1, §15.2.1.1,
//! §16.1.1), private name resolution (§15.7.1) and module exports
//! (§16.2.1.1).

use std::collections::{HashMap, HashSet};

use super::{PResult, Parser};
use crate::source::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScopeKind {
    /// A script's top level: function declarations are var-scoped.
    Script,
    /// A module's top level: function declarations are lexical.
    Module,
    /// A function body (parameters live in the same scope record here).
    Function,
    /// A block, `switch` case block, loop head or class static block body.
    Block,
    /// A `catch` parameter list; its body is a child block.
    Catch,
}

#[derive(Debug)]
pub(super) struct Scope {
    pub(super) kind: ScopeKind,
    /// Lexically declared names; `true` marks a sloppy-mode block function,
    /// which Annex B.3.2.4 lets another such function redeclare.
    lexical: HashMap<Box<str>, bool>,
    /// Var-declared names hoisted through or declared in this scope.
    vars: HashSet<Box<str>>,
    /// Parameter names (function scopes) or catch parameters.
    params: HashSet<Box<str>>,
    /// The catch parameter is a single identifier (Annex B.3.4 allows a
    /// `var` of the same name in the body).
    simple_catch: bool,
}

/// Private names of one class body.
#[derive(Debug, Default)]
pub(super) struct ClassScope {
    /// Declared names and how: 1 getter, 2 setter, 3 both, 4 other; plus
    /// whether the declaration is static.
    declared: HashMap<Box<str>, (u8, bool)>,
    /// References not yet resolved, with their spans.
    references: Vec<(Box<str>, Span)>,
}

/// How a private name is declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PrivateKind {
    Getter,
    Setter,
    Other,
}

impl Parser<'_> {
    pub(super) fn push_scope(&mut self, kind: ScopeKind) {
        self.scopes.push(Scope {
            kind,
            lexical: HashMap::new(),
            vars: HashSet::new(),
            params: HashSet::new(),
            simple_catch: false,
        });
    }

    pub(super) fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn current_scope(&mut self) -> &mut Scope {
        // The parser pushes the top-level scope before parsing anything.
        let last = self.scopes.len() - 1;
        &mut self.scopes[last]
    }

    /// Declare function parameters in the current (function) scope.
    /// Duplicates are checked by the caller, which knows whether they are
    /// allowed.
    pub(super) fn declare_params(&mut self, names: &[(Box<str>, Span)]) {
        let scope = self.current_scope();
        for (name, _) in names {
            scope.params.insert(name.clone());
        }
    }

    /// Declare catch parameters in the current (catch) scope; duplicates
    /// are errors.
    pub(super) fn declare_catch_params(
        &mut self,
        names: &[(Box<str>, Span)],
        simple: bool,
    ) -> PResult<()> {
        let mut seen = HashSet::new();
        for (name, span) in names {
            if !seen.insert(name.clone()) {
                return Err(self.error(*span, "duplicate catch parameter"));
            }
        }
        let scope = self.current_scope();
        scope.simple_catch = simple;
        scope.params.extend(seen);
        Ok(())
    }

    /// Declare a `let`, `const`, `class` or block-level function name.
    /// `block_function` marks a sloppy-mode plain function declaration in a
    /// block (Annex B allows such functions to share a name).
    pub(super) fn declare_lexical(
        &mut self,
        name: &str,
        span: Span,
        block_function: bool,
    ) -> PResult<()> {
        let index = self.scopes.len() - 1;
        let scope = &self.scopes[index];
        let duplicate = match scope.lexical.get(name) {
            Some(&previous) => !(previous && block_function),
            None => false,
        };
        let conflicts_with_param = scope.kind == ScopeKind::Function && scope.params.contains(name);
        // A catch body's lexical names may not shadow its parameters.
        let shadows_catch = index > 0
            && scope.kind == ScopeKind::Block
            && self.scopes[index - 1].kind == ScopeKind::Catch
            && self.scopes[index - 1].params.contains(name);
        if duplicate || scope.vars.contains(name) || conflicts_with_param || shadows_catch {
            return Err(self.error(span, format!("redeclaration of `{name}`")));
        }
        self.scopes[index]
            .lexical
            .insert(name.into(), block_function);
        Ok(())
    }

    /// Declare a `var` (or a var-scoped function) name: it is hoisted to
    /// the nearest function or top-level scope and conflicts with lexical
    /// names of every scope it passes through.
    pub(super) fn declare_var(&mut self, name: &str, span: Span) -> PResult<()> {
        for index in (0..self.scopes.len()).rev() {
            let scope = &self.scopes[index];
            let lexical = scope.lexical.contains_key(name);
            let catch_conflict = scope.kind == ScopeKind::Catch
                && scope.params.contains(name)
                && !scope.simple_catch;
            if lexical || catch_conflict {
                return Err(self.error(span, format!("redeclaration of `{name}`")));
            }
            let kind = scope.kind;
            self.scopes[index].vars.insert(name.into());
            if matches!(
                kind,
                ScopeKind::Function | ScopeKind::Script | ScopeKind::Module
            ) {
                break;
            }
        }
        Ok(())
    }

    /// Whether the innermost scope is a function or top-level scope, where
    /// function declarations follow var rules in scripts and functions.
    pub(super) fn at_var_scope_top(&self) -> bool {
        self.scopes
            .last()
            .is_some_and(|s| matches!(s.kind, ScopeKind::Function | ScopeKind::Script))
    }

    // ---- private names ---------------------------------------------------

    pub(super) fn push_class(&mut self) {
        self.classes.push(ClassScope::default());
    }

    /// Record a private name declaration; a getter and a setter with the
    /// same staticness may share a name, anything else is a duplicate.
    pub(super) fn declare_private(
        &mut self,
        name: &str,
        span: Span,
        kind: PrivateKind,
        is_static: bool,
    ) -> PResult<()> {
        if name == "constructor" {
            return Err(self.error(span, "`#constructor` is not a valid private name"));
        }
        let bit = match kind {
            PrivateKind::Getter => 1,
            PrivateKind::Setter => 2,
            PrivateKind::Other => 4,
        };
        let Some(class) = self.classes.last_mut() else {
            return Err(self.error(span, "private name outside a class"));
        };
        match class.declared.get_mut(name) {
            None => {
                class.declared.insert(name.into(), (bit, is_static));
                Ok(())
            }
            Some((existing, existing_static))
                if bit != 4
                    && *existing != 4
                    && *existing & bit == 0
                    && *existing_static == is_static =>
            {
                *existing |= bit;
                Ok(())
            }
            Some(_) => Err(self.error(span, format!("duplicate private name `#{name}`"))),
        }
    }

    /// Record a private name reference, resolved when its class ends.
    pub(super) fn reference_private(&mut self, name: &str, span: Span) -> PResult<()> {
        match self.classes.last_mut() {
            Some(class) => {
                class.references.push((name.into(), span));
                Ok(())
            }
            None => Err(self.error(span, format!("undeclared private name `#{name}`"))),
        }
    }

    /// End a class body: references it declares resolve; the rest move to
    /// the enclosing class, or are errors at the outermost class.
    pub(super) fn pop_class(&mut self) -> PResult<()> {
        let Some(class) = self.classes.pop() else {
            return Ok(());
        };
        for (name, span) in class.references {
            if class.declared.contains_key(&name) {
                continue;
            }
            match self.classes.last_mut() {
                Some(outer) => outer.references.push((name, span)),
                None => {
                    return Err(self.error(span, format!("undeclared private name `#{name}`")));
                }
            }
        }
        Ok(())
    }

    // ---- module exports ------------------------------------------------

    /// Record an exported name; each may be exported once.
    pub(super) fn export_name(&mut self, name: Vec<u16>, span: Span) -> PResult<()> {
        if self.exported.iter().any(|(n, _)| *n == name) {
            return Err(self.error(span, "duplicate export name"));
        }
        self.exported.push((name, span));
        Ok(())
    }

    /// Every `export { local }` must name a module-level declaration.
    pub(super) fn check_module_exports(&mut self) -> PResult<()> {
        let Some(top) = self.scopes.first() else {
            return Ok(());
        };
        for (name, span) in &self.export_refs {
            if !top.lexical.contains_key(name) && !top.vars.contains(name) {
                return Err(self.error(*span, format!("export of undeclared `{name}`")));
            }
        }
        Ok(())
    }
}
