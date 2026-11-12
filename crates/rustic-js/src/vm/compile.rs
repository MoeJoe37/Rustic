//! Code generation: AST to register bytecode (JS-04).
//!
//! Each function compiles in one pass over its AST, guided by the
//! [`ScopeTree`]. Expressions are compiled into a destination register;
//! temporaries are allocated stack-wise above the function's local
//! registers. Left-nested chains (`a + b + c ...`, `a.b.c ...`) are walked
//! iteratively, and other nesting is bounded by a stack guard, so hostile
//! source cannot overflow the Rust stack. The AST is not kept: once a
//! script is compiled, only bytecode remains.

use std::collections::HashMap;
use std::rc::Rc;

use super::bytecode::{
    BinOp, CodeId, FunctionCode, FunctionKind, Handler, NamedSite, Op, Reg, UnOp, declare,
};
use super::scope::{
    BindingKind, CLASS, FIELD_INIT, NEW_TARGET, ROLE_BODY, ROLE_FIELDS, ROLE_FINALLY, Resolution,
    ScopeId, ScopeKind, ScopeTree, THIS, field_key_name,
};
use crate::ast::{
    Argument, ArrayElement, ArrowBody, ArrowFunction, AssignOp, BinaryOp, Class, ClassElement,
    Expression, ExpressionKind, ForHead, ForInit, Function, LogicalOp, MemberProperty, MethodKind,
    ObjectProperty, Pattern, PatternKind, Program, PropertyKey, Statement, StatementKind, UnaryOp,
    UpdateOp, VariableDeclaration, VariableKind,
};
use crate::runtime::{Agent, BINDING_MUTABLE, BINDING_STRICT, JsStr, OutOfMemory, StringId, Value};
use crate::source::Span;

/// Why a program could not be compiled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileError {
    /// Syntax the engine does not run yet, with where it is.
    Unsupported(&'static str, Span),
    /// Nesting too deep for the compiler's stack budget.
    TooDeep,
    /// More registers, constants or sites than the bytecode can address.
    TooLarge,
    /// The heap ran out while allocating constants.
    OutOfMemory,
}

impl From<OutOfMemory> for CompileError {
    fn from(_: OutOfMemory) -> Self {
        CompileError::OutOfMemory
    }
}

type CResult<T> = Result<T, CompileError>;

/// Stack the compiler may use beyond its entry frame.
const MAX_STACK_BYTES: usize = 1024 * 1024;

fn stack_address() -> usize {
    let marker = 0u8;
    std::ptr::from_ref(&marker) as usize
}

/// Compile a parsed script.
pub(crate) fn compile_script(agent: &mut Agent, program: &Program) -> CResult<CodeId> {
    let tree = ScopeTree::analyze_script(&program.body, program.strict);
    let name = agent.names.empty;
    let mut g = Gen::new(
        agent,
        &tree,
        0,
        FunctionKind::Script,
        program.strict,
        name,
        stack_address(),
    );
    g.script(&program.body)?;
    g.finish(0, 0)
}

/// Where a jump that leaves a control structure goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Exit {
    Break(usize),
    Continue(usize),
    Return,
}

#[derive(Debug)]
enum ControlKind {
    /// A loop: `continue` is allowed.
    Loop,
    /// A labelled statement or `switch`: only `break`.
    Breakable,
    /// A `try` with a `finally`: exits route through it.
    Finally {
        kind: Reg,
        value: Reg,
        entry_jumps: Vec<usize>,
        exits: Vec<Exit>,
    },
    /// A `for-of` loop body: exits close the iterator, except a
    /// `continue` of the loop itself (the control at `owner`).
    Iterator { iterator: Reg, owner: usize },
}

#[derive(Debug)]
struct Control {
    kind: ControlKind,
    labels: Vec<Box<str>>,
    /// Plain `break` (no label) targets this.
    unlabeled_break: bool,
    break_depth: u16,
    continue_depth: u16,
    breaks: Vec<usize>,
    continues: Vec<usize>,
    /// Where `continue` goes, once known.
    continue_target: Option<u32>,
}

struct Gen<'a, 't> {
    agent: &'a mut Agent,
    tree: &'t ScopeTree,
    ops: Vec<Op>,
    constants: Vec<Value>,
    numbers: HashMap<u64, u32>,
    strings: HashMap<Vec<u16>, u32>,
    atoms: HashMap<StringId, u32>,
    functions: Vec<CodeId>,
    scopes: Vec<Rc<Vec<(StringId, u8)>>>,
    scope_index: HashMap<ScopeId, u32>,
    handlers: Vec<Handler>,
    sites: Vec<NamedSite>,
    /// Registers of uncaptured bindings.
    locals: HashMap<(ScopeId, usize), Reg>,
    next_reg: u32,
    max_reg: u32,
    scope: ScopeId,
    function_scope: ScopeId,
    /// Environments pushed by this function so far (for exits and
    /// handlers).
    depth: u16,
    controls: Vec<Control>,
    kind: FunctionKind,
    strict: bool,
    name: StringId,
    /// Script completion value.
    completion: Option<Reg>,
    labels: Vec<Box<str>>,
    stack_base: usize,
    /// Set by a logical member assignment: the jump that skips its store.
    pending_skip: Option<usize>,
    /// Where `super.x` starts its lookup in this function: `Some(true)`
    /// in a static class method (the parent class), `Some(false)` in an
    /// instance method or constructor (the parent's prototype). Arrow
    /// functions inherit it.
    home_static: Option<bool>,
    /// The value of `home_static` for the next method compiled.
    next_home: Option<bool>,
}

impl<'a, 't> Gen<'a, 't> {
    fn new(
        agent: &'a mut Agent,
        tree: &'t ScopeTree,
        function_scope: ScopeId,
        kind: FunctionKind,
        strict: bool,
        name: StringId,
        stack_base: usize,
    ) -> Self {
        Self {
            agent,
            tree,
            ops: Vec::new(),
            constants: Vec::new(),
            numbers: HashMap::new(),
            strings: HashMap::new(),
            atoms: HashMap::new(),
            functions: Vec::new(),
            scopes: Vec::new(),
            scope_index: HashMap::new(),
            handlers: Vec::new(),
            sites: Vec::new(),
            locals: HashMap::new(),
            next_reg: 0,
            max_reg: 0,
            scope: function_scope,
            function_scope,
            depth: 0,
            controls: Vec::new(),
            kind,
            strict,
            name,
            completion: None,
            labels: Vec::new(),
            stack_base,
            pending_skip: None,
            home_static: None,
            next_home: None,
        }
    }

    fn guard(&self) -> CResult<()> {
        if stack_address().abs_diff(self.stack_base) > MAX_STACK_BYTES {
            return Err(CompileError::TooDeep);
        }
        Ok(())
    }

    fn finish(mut self, params: u16, length: u16) -> CResult<CodeId> {
        // Always, even after a final `return`: a jump over that statement
        // (`if (x) return`) lands here.
        let r = self.temp()?;
        if self.kind == FunctionKind::DerivedConstructor {
            // The result is `this`, which must be bound by now.
            self.load_this(r)?;
        } else {
            self.emit(Op::Undefined { dst: r });
        }
        self.emit(Op::Return { src: r });
        // Jump targets are `u32`. Nothing has run yet, so a target that
        // wrapped while compiling is rejected here before it can be used.
        u32::try_from(self.ops.len()).map_err(|_| CompileError::TooLarge)?;
        let registers = u16::try_from(self.max_reg).map_err(|_| CompileError::TooLarge)?;
        let code = FunctionCode {
            ops: self.ops,
            constants: self.constants,
            functions: self.functions,
            scopes: self.scopes,
            handlers: self.handlers,
            registers: registers.max(1),
            params,
            length,
            name: self.name,
            strict: self.strict,
            kind: self.kind,
            sites: self.sites.into_boxed_slice(),
        };
        Ok(self.agent.heap.alloc_code(code)?)
    }

    // ---- emission ----------------------------------------------------

    fn emit(&mut self, op: Op) -> usize {
        self.ops.push(op);
        self.ops.len() - 1
    }

    fn here(&self) -> u32 {
        self.ops.len() as u32
    }

    /// Point the jump at `at` to `target`.
    fn patch(&mut self, at: usize, target: u32) {
        debug_assert!(
            target as usize > at || matches!(self.ops[at], Op::Loop { .. }),
            "backward jump at {at} is not a safe point"
        );
        match &mut self.ops[at] {
            Op::Jump { target: t }
            | Op::JumpIfTrue { target: t, .. }
            | Op::JumpIfFalse { target: t, .. }
            | Op::JumpIfNullish { target: t, .. }
            | Op::JumpIfNotUndefined { target: t, .. }
            | Op::JumpIfStrictEq { target: t, .. }
            | Op::JumpIfInt { target: t, .. }
            | Op::JumpUnless { target: t, .. }
            | Op::JumpUnlessInt { target: t, .. }
            | Op::Loop { target: t }
            | Op::ForInNext { done: t, .. }
            | Op::IteratorStep { done: t, .. } => *t = target,
            other => unreachable!("not a jump: {other:?}"),
        }
    }

    fn patch_here(&mut self, at: usize) {
        let here = self.here();
        self.patch(at, here);
    }

    fn jump(&mut self) -> usize {
        self.emit(Op::Jump { target: 0 })
    }

    fn temp(&mut self) -> CResult<Reg> {
        let r = self.next_reg;
        self.next_reg += 1;
        self.max_reg = self.max_reg.max(self.next_reg);
        u16::try_from(r).map_err(|_| CompileError::TooLarge)
    }

    fn temps(&mut self, n: usize) -> CResult<Reg> {
        let first = self.next_reg;
        self.next_reg += n as u32;
        self.max_reg = self.max_reg.max(self.next_reg);
        if self.next_reg > u32::from(u16::MAX) {
            return Err(CompileError::TooLarge);
        }
        Ok(first as Reg)
    }

    fn mark(&self) -> u32 {
        self.next_reg
    }

    fn release(&mut self, mark: u32) {
        self.next_reg = mark;
    }

    // ---- constants ---------------------------------------------------

    fn push_constant(&mut self, v: Value) -> u32 {
        self.constants.push(v);
        (self.constants.len() - 1) as u32
    }

    fn number(&mut self, n: f64) -> u32 {
        let bits = n.to_bits();
        if let Some(&i) = self.numbers.get(&bits) {
            return i;
        }
        let i = self.push_constant(Value::number(n));
        self.numbers.insert(bits, i);
        i
    }

    fn string(&mut self, units: &[u16]) -> CResult<u32> {
        if let Some(&i) = self.strings.get(units) {
            return Ok(i);
        }
        let id = self.agent.heap.alloc_string(JsStr::from_units(units))?;
        let i = self.push_constant(Value::string(id));
        self.strings.insert(units.to_vec(), i);
        Ok(i)
    }

    fn atom_id(&mut self, name: &str) -> CResult<StringId> {
        Ok(self.agent.heap.intern_str(name)?)
    }

    fn atom(&mut self, name: &str) -> CResult<u32> {
        let id = self.atom_id(name)?;
        Ok(self.atom_const(id))
    }

    fn atom_const(&mut self, id: StringId) -> u32 {
        if let Some(&i) = self.atoms.get(&id) {
            return i;
        }
        let i = self.push_constant(Value::string(id));
        self.atoms.insert(id, i);
        i
    }

    fn load_number(&mut self, dst: Reg, n: f64) {
        if n.fract() == 0.0 && n.abs() <= f64::from(i32::MAX) && !(n == 0.0 && n.is_sign_negative())
        {
            self.emit(Op::Int {
                dst,
                value: n as i32,
            });
        } else {
            let index = self.number(n);
            self.emit(Op::Const { dst, index });
        }
    }

    fn load_bool(&mut self, dst: Reg, b: bool) {
        let index = self.push_constant(Value::boolean(b));
        self.emit(Op::Const { dst, index });
    }

    fn site(&mut self, name: &str) -> CResult<Option<u16>> {
        let name = self.atom_id(name)?;
        if self.sites.len() >= usize::from(u16::MAX) {
            return Ok(None);
        }
        self.sites.push(NamedSite {
            name,
            cache: std::cell::Cell::default(),
        });
        Ok(Some((self.sites.len() - 1) as u16))
    }

    fn get_named(&mut self, dst: Reg, object: Reg, name: &str) -> CResult<()> {
        match self.site(name)? {
            Some(site) => {
                self.emit(Op::GetNamed { dst, object, site });
            }
            None => {
                let key = self.temp()?;
                let index = self.atom(name)?;
                self.emit(Op::Const { dst: key, index });
                self.emit(Op::GetProp { dst, object, key });
                self.next_reg -= 1;
            }
        }
        Ok(())
    }

    fn set_named(&mut self, object: Reg, name: &str, value: Reg) -> CResult<()> {
        match self.site(name)? {
            Some(site) => {
                self.emit(Op::SetNamed {
                    object,
                    site,
                    value,
                });
            }
            None => {
                let key = self.temp()?;
                let index = self.atom(name)?;
                self.emit(Op::Const { dst: key, index });
                self.emit(Op::SetProp { object, key, value });
                self.next_reg -= 1;
            }
        }
        Ok(())
    }

    // ---- scopes and bindings -----------------------------------------

    /// Allocate registers or slots for a scope's bindings.
    fn layout_scope(&mut self, scope: ScopeId) -> CResult<()> {
        let s = &self.tree.scopes[scope];
        if s.kind == ScopeKind::Script || s.kind == ScopeKind::With {
            return Ok(());
        }
        let mut names = Vec::new();
        for (i, b) in s.bindings.iter().enumerate() {
            if b.captured {
                let id = self.agent.heap.intern_str(&b.name)?;
                let flags = match b.kind {
                    BindingKind::Const => BINDING_STRICT,
                    BindingKind::SelfName => 0,
                    _ => BINDING_MUTABLE,
                };
                names.push((id, flags));
            } else if !self.locals.contains_key(&(scope, i)) {
                let r = self.temp()?;
                self.locals.insert((scope, i), r);
            }
        }
        if s.has_env() {
            let index = self.scopes.len() as u32;
            names.shrink_to_fit();
            self.scopes.push(Rc::new(names));
            self.scope_index.insert(scope, index);
        }
        Ok(())
    }

    /// Enter a block scope: environment, TDZ markers, hoisted functions.
    fn enter_block(&mut self, scope: ScopeId, body: &[Statement]) -> CResult<()> {
        self.layout_scope(scope)?;
        self.scope = scope;
        if let Some(&index) = self.scope_index.get(&scope) {
            self.emit(Op::PushScope { scope: index });
            self.depth += 1;
        }
        let bindings = self.tree.scopes[scope].bindings.clone();
        for (i, b) in bindings.iter().enumerate() {
            if !b.captured && b.kind.has_tdz() {
                let r = self.locals[&(scope, i)];
                self.emit(Op::Uninit { dst: r });
            }
        }
        self.hoist_functions(body, false)?;
        Ok(())
    }

    fn leave_block(&mut self, scope: ScopeId) {
        if self.scope_index.contains_key(&scope) {
            self.emit(Op::PopScope);
            self.depth -= 1;
        }
        self.scope = self.tree.scopes[scope].parent.unwrap_or(scope);
    }

    /// Instantiate function declarations of a statement list.
    fn hoist_functions(&mut self, body: &[Statement], top: bool) -> CResult<()> {
        for s in body {
            if let StatementKind::Function(f) = &s.kind {
                let Some(id) = &f.id else { continue };
                if f.is_async || f.is_generator {
                    return Err(CompileError::Unsupported(
                        "generator and async functions",
                        f.span,
                    ));
                }
                let mark = self.mark();
                let r = self.temp()?;
                self.function_expression(f, r, None, FunctionKind::Normal)?;
                if self.kind == FunctionKind::Script && top {
                    let name = self.atom(&id.name)?;
                    self.emit(Op::SetName { src: r, name });
                } else {
                    self.store_binding(&id.name, r, true)?;
                }
                self.release(mark);
            }
        }
        Ok(())
    }

    /// The slot of a captured binding in its scope's environment: its
    /// rank among the scope's captured bindings. Enclosing functions'
    /// scopes are laid out the same way, so a nested function finds their
    /// slots without sharing state with their compiler.
    fn slot_of(&self, scope: ScopeId, index: usize) -> u16 {
        let before = self.tree.scopes[scope].bindings[..index]
            .iter()
            .filter(|b| b.captured)
            .count();
        // Scopes with more bindings than registers fail to compile.
        before as u16
    }

    fn resolve(&self, name: &str) -> Resolution {
        self.tree.resolve(self.scope, name)
    }

    /// Whether a `with` body encloses the current scope, so a name no
    /// scope declares may still be a property of the `with` object.
    fn under_with(&self) -> bool {
        let mut current = Some(self.scope);
        while let Some(id) = current {
            if self.tree.scopes[id].kind == ScopeKind::With {
                return true;
            }
            current = self.tree.scopes[id].parent;
        }
        false
    }

    /// A cache site for a global name, unless a `with` is in the way or
    /// the function has run out of sites.
    fn global_site(&mut self, name: &str) -> CResult<Option<u16>> {
        if self.under_with() {
            return Ok(None);
        }
        self.site(name)
    }

    /// Environments between the current scope and `target` (exclusive).
    fn hops(&self, target: ScopeId) -> u16 {
        let mut hops = 0;
        let mut current = Some(self.scope);
        while let Some(id) = current {
            if id == target {
                return hops;
            }
            if self.tree.scopes[id].has_env() {
                hops += 1;
            }
            current = self.tree.scopes[id].parent;
        }
        unreachable!("binding scope encloses the reference")
    }

    fn binding_kind(&self, scope: ScopeId, index: usize) -> BindingKind {
        self.tree.scopes[scope].bindings[index].kind
    }

    /// Read a binding into `dst`.
    fn load_binding(&mut self, name: &str, dst: Reg) -> CResult<()> {
        match self.resolve(name) {
            Resolution::Dynamic => match self.global_site(name)? {
                Some(site) => {
                    self.emit(Op::GetGlobal { dst, site });
                }
                None => {
                    let name = self.atom(name)?;
                    self.emit(Op::GetName { dst, name });
                }
            },
            Resolution::Binding { scope, index } => {
                self.load_binding_at(scope, index, name, dst)?
            }
        }
        Ok(())
    }

    /// Read the binding `index` of `scope` into `dst`, with its TDZ check.
    fn load_binding_at(
        &mut self,
        scope: ScopeId,
        index: usize,
        name: &str,
        dst: Reg,
    ) -> CResult<()> {
        if let Some(&r) = self.locals.get(&(scope, index)) {
            if r != dst {
                self.emit(Op::Move { dst, src: r });
            }
            if self.binding_kind(scope, index).has_tdz() {
                let name = self.atom(name)?;
                self.emit(Op::CheckTdz { src: dst, name });
            }
        } else {
            let slot = self.slot_of(scope, index);
            let hops = self.hops(scope);
            self.emit(Op::GetSlot { dst, hops, slot });
        }
        Ok(())
    }

    /// `dst = this`. In a derived class constructor and the arrow
    /// functions inside it, `this` is a binding that `super()` fills.
    fn load_this(&mut self, dst: Reg) -> CResult<()> {
        match self.tree.resolve_special(self.scope, THIS) {
            Some((scope, index)) => self.load_binding_at(scope, index, THIS, dst),
            None => {
                self.emit(Op::This { dst });
                Ok(())
            }
        }
    }

    /// `dst = new.target`. An arrow function reads the binding its
    /// enclosing function keeps for it.
    fn load_new_target(&mut self, dst: Reg) -> CResult<()> {
        if self.kind == FunctionKind::Arrow {
            if let Some((scope, index)) = self.tree.resolve_special(self.scope, NEW_TARGET) {
                return self.load_binding_at(scope, index, NEW_TARGET, dst);
            }
        }
        self.emit(Op::NewTarget { dst });
        Ok(())
    }

    /// `dst = the enclosing class constructor`.
    fn load_class(&mut self, dst: Reg, span: Span) -> CResult<()> {
        match self.tree.resolve_special(self.scope, CLASS) {
            Some((scope, index)) => self.load_binding_at(scope, index, CLASS, dst),
            None => Err(CompileError::Unsupported("super outside a class", span)),
        }
    }

    /// `dst = the object a `super.x` lookup starts from`: the prototype
    /// of the home object.
    fn load_super_base(&mut self, dst: Reg, span: Span) -> CResult<()> {
        let Some(is_static) = self.home_static else {
            return Err(CompileError::Unsupported("super outside a class", span));
        };
        self.load_class(dst, span)?;
        if !is_static {
            self.get_named(dst, dst, "prototype")?;
        }
        self.emit(Op::GetPrototypeOf { dst, src: dst });
        Ok(())
    }

    /// `dst = the key of a `super` member`.
    fn super_key(&mut self, property: &MemberProperty, dst: Reg, span: Span) -> CResult<()> {
        match property {
            MemberProperty::Name(id) => {
                let index = self.atom(&id.name)?;
                self.emit(Op::Const { dst, index });
                Ok(())
            }
            MemberProperty::Computed(k) => self.expr(k, dst),
            MemberProperty::Private(_) => Err(CompileError::Unsupported("private names", span)),
        }
    }

    /// `dst = super.property`, with the receiver left in `this`.
    fn super_member(
        &mut self,
        property: &MemberProperty,
        dst: Reg,
        this: Reg,
        span: Span,
    ) -> CResult<()> {
        let mark = self.mark();
        self.load_this(this)?;
        let (base, key) = (self.temp()?, self.temp()?);
        self.super_key(property, key, span)?;
        self.load_super_base(base, span)?;
        self.emit(Op::GetSuper {
            dst,
            object: base,
            key,
            this,
        });
        self.release(mark);
        Ok(())
    }

    /// `dst = super(arguments)`, which also binds `this`.
    fn super_call(&mut self, arguments: &[Argument], dst: Reg, span: Span) -> CResult<()> {
        let Some((this_scope, this_index)) = self.tree.resolve_special(self.scope, THIS) else {
            return Err(CompileError::Unsupported(
                "super call outside a derived constructor",
                span,
            ));
        };
        let mark = self.mark();
        let (callee, new_target) = (self.temp()?, self.temp()?);
        self.load_class(callee, span)?;
        self.emit(Op::GetPrototypeOf {
            dst: callee,
            src: callee,
        });
        self.load_new_target(new_target)?;
        if arguments.iter().any(|a| matches!(a, Argument::Spread(_))) {
            let array = self.arguments_array(arguments)?;
            self.emit(Op::SuperCallSpread {
                dst,
                callee,
                new_target,
                array,
            });
        } else {
            let (args, argc) = self.arguments(arguments)?;
            self.emit(Op::SuperCall {
                dst,
                callee,
                new_target,
                args,
                argc,
            });
        }
        // Bind `this`, unless an earlier `super()` already did.
        let current = self.temp()?;
        if let Some(&r) = self.locals.get(&(this_scope, this_index)) {
            self.emit(Op::Move {
                dst: current,
                src: r,
            });
        } else {
            let slot = self.slot_of(this_scope, this_index);
            let hops = self.hops(this_scope);
            self.emit(Op::GetSlotRaw {
                dst: current,
                hops,
                slot,
            });
        }
        self.emit(Op::ThrowIfInit { src: current });
        self.store_binding_at(this_scope, this_index, dst)?;
        self.initialize_fields(dst)?;
        self.release(mark);
        Ok(())
    }

    /// Evaluate a class definition into `dst`.
    fn class_definition(&mut self, c: &Class, dst: Reg, name_hint: Option<&str>) -> CResult<()> {
        self.guard()?;
        let scope = self.tree.scope_of(c).expect("classes are analyzed");
        let outer_scope = self.scope;
        let derived = c.super_class.is_some();
        let name: Box<str> = match (&c.id, name_hint) {
            (Some(id), _) => id.name.clone(),
            (None, Some(hint)) => hint.into(),
            (None, None) => "".into(),
        };
        // The class scope: the class's own name stays in its TDZ until
        // the definition is complete.
        self.layout_scope(scope)?;
        self.scope = scope;
        if let Some(&index) = self.scope_index.get(&scope) {
            self.emit(Op::PushScope { scope: index });
            self.depth += 1;
        }
        let bindings = self.tree.scopes[scope].bindings.clone();
        for (i, b) in bindings.iter().enumerate() {
            if !b.captured && b.kind.has_tdz() {
                let r = self.locals[&(scope, i)];
                self.emit(Op::Uninit { dst: r });
            }
        }
        let mark = self.mark();
        let (heritage, f, proto) = (self.temp()?, self.temp()?, self.temp()?);
        if let Some(h) = &c.super_class {
            self.expr(h, heritage)?;
        }
        let kind = if derived {
            FunctionKind::DerivedConstructor
        } else {
            FunctionKind::ClassConstructor
        };
        let constructor = c.body.iter().find_map(|element| match element {
            ClassElement::Method {
                function,
                kind: MethodKind::Constructor,
                ..
            } => Some(function),
            _ => None,
        });
        match constructor {
            Some(function) => {
                self.next_home = Some(false);
                self.function_expression(function, f, Some(&name), kind)?;
            }
            None => self.default_constructor(scope, f, &name, kind, c.span)?,
        }
        self.emit(Op::ClassSetup {
            ctor: f,
            heritage,
            proto,
            derived,
        });
        let class_index = self.tree.scopes[scope]
            .find(CLASS)
            .expect("declared by the resolver");
        self.store_binding_at(scope, class_index, f)?;
        // Static fields and blocks run after every method is defined;
        // their computed keys are evaluated in order with the rest and
        // wait in registers.
        let mut static_keys: Vec<Option<Reg>> = vec![None; c.body.len()];
        for (element_index, element) in c.body.iter().enumerate() {
            match element {
                ClassElement::Method {
                    kind: MethodKind::Constructor,
                    ..
                } => {}
                ClassElement::Method {
                    key,
                    function,
                    kind: method_kind,
                    is_static,
                    span,
                } => {
                    if matches!(key, PropertyKey::Private(_)) {
                        return Err(CompileError::Unsupported("private names", *span));
                    }
                    let inner = self.mark();
                    let (k, v) = (self.temp()?, self.temp()?);
                    let target = if *is_static { f } else { proto };
                    self.property_key(key, k)?;
                    let key_name = Self::static_key_name(key);
                    let hint = match method_kind {
                        MethodKind::Get => key_name.map(|n| format!("get {n}")),
                        MethodKind::Set => key_name.map(|n| format!("set {n}")),
                        _ => key_name,
                    };
                    self.next_home = Some(*is_static);
                    self.function_expression(function, v, hint.as_deref(), FunctionKind::Method)?;
                    match method_kind {
                        MethodKind::Get | MethodKind::Set => {
                            self.emit(Op::DefineAccessor {
                                object: target,
                                key: k,
                                function: v,
                                setter: *method_kind == MethodKind::Set,
                                enumerable: false,
                            });
                        }
                        _ => {
                            self.emit(Op::DefineMethod {
                                object: target,
                                key: k,
                                value: v,
                            });
                        }
                    }
                    self.release(inner);
                }
                ClassElement::Field {
                    key,
                    is_static,
                    span,
                    ..
                } => match key {
                    PropertyKey::Private(_) => {
                        return Err(CompileError::Unsupported("private names", *span));
                    }
                    PropertyKey::Computed(x) => {
                        let k = self.temp()?;
                        self.expr(x, k)?;
                        self.emit(Op::Unary {
                            op: UnOp::ToPropertyKey,
                            dst: k,
                            src: k,
                        });
                        if *is_static {
                            static_keys[element_index] = Some(k);
                        } else {
                            let name = field_key_name(element_index);
                            let index = self.tree.scopes[scope]
                                .find(&name)
                                .expect("declared by the resolver");
                            self.store_binding_at(scope, index, k)?;
                        }
                    }
                    _ => {}
                },
                ClassElement::StaticBlock(..) => {}
            }
        }
        let init_index = self.tree.scopes[scope]
            .find(FIELD_INIT)
            .expect("declared by the resolver");
        if self.tree.scopes[scope].bindings[init_index].used {
            let init = self.temp()?;
            self.field_initializer(c, scope, init)?;
            self.store_binding_at(scope, init_index, init)?;
        }
        if let Some(id) = &c.id {
            let index = self.tree.scopes[scope]
                .find(&id.name)
                .expect("declared by the resolver");
            self.store_binding_at(scope, index, f)?;
        }
        for (element_index, element) in c.body.iter().enumerate() {
            match element {
                ClassElement::Field {
                    key,
                    value,
                    is_static: true,
                    ..
                } => {
                    let inner = self.mark();
                    let (k, v) = (self.temp()?, self.temp()?);
                    match static_keys[element_index] {
                        Some(held) => {
                            self.emit(Op::Move { dst: k, src: held });
                        }
                        None => self.property_key(key, k)?,
                    }
                    match value {
                        Some(value) => {
                            let function_scope = self
                                .tree
                                .scope_of(element)
                                .expect("static fields are analyzed");
                            let hint = Self::static_key_name(key);
                            let code = {
                                let mut g = self.class_member_gen(function_scope, true);
                                let r = g.temp()?;
                                g.layout_scope(function_scope)?;
                                g.expr_named(value, r, hint.as_deref())?;
                                g.emit(Op::Return { src: r });
                                g.finish(0, 0)?
                            };
                            self.call_member_code(code, f, v)?;
                        }
                        None => {
                            self.emit(Op::Undefined { dst: v });
                        }
                    }
                    self.emit(Op::DefineProp {
                        object: f,
                        key: k,
                        value: v,
                    });
                    self.release(inner);
                }
                ClassElement::StaticBlock(body, _) => {
                    let inner = self.mark();
                    let function_scope = self
                        .tree
                        .scope_of(element)
                        .expect("static blocks are analyzed");
                    let code = {
                        let mut g = self.class_member_gen(function_scope, true);
                        g.parameters(&[], None)?;
                        g.function_prologue(body)?;
                        g.statements(body)?;
                        g.finish(0, 0)?
                    };
                    let v = self.temp()?;
                    self.call_member_code(code, f, v)?;
                    self.release(inner);
                }
                _ => {}
            }
        }
        self.emit(Op::Move { dst, src: f });
        self.release(mark);
        if self.scope_index.contains_key(&scope) {
            self.emit(Op::PopScope);
            self.depth -= 1;
        }
        self.scope = outer_scope;
        Ok(())
    }

    /// A generator for code the class itself needs (field initializers,
    /// static blocks): a strict method whose `super` starts at the class
    /// (`is_static`) or at its prototype.
    fn class_member_gen(&mut self, scope: ScopeId, is_static: bool) -> Gen<'_, 't> {
        let name = self.agent.names.empty;
        let mut g = Gen::new(
            self.agent,
            self.tree,
            scope,
            FunctionKind::Method,
            true,
            name,
            self.stack_base,
        );
        g.home_static = Some(is_static);
        g
    }

    /// `dst = code.call(this)` for code made by [`Self::class_member_gen`].
    fn call_member_code(&mut self, code: CodeId, this: Reg, dst: Reg) -> CResult<()> {
        let index = self.functions.len() as u32;
        self.functions.push(code);
        let callee = self.temp()?;
        self.emit(Op::Closure { dst: callee, index });
        self.emit(Op::Call {
            dst,
            callee,
            this,
            args: callee,
            argc: 0,
        });
        Ok(())
    }

    /// `dst = a function that defines the instance fields of `c` on its
    /// `this``, in order, each with `CreateDataPropertyOrThrow`.
    fn field_initializer(&mut self, c: &Class, class_scope: ScopeId, dst: Reg) -> CResult<()> {
        let function_scope = self
            .tree
            .scope_of_role(c, ROLE_FIELDS)
            .expect("instance fields are analyzed");
        let code = {
            let mut g = self.class_member_gen(function_scope, false);
            g.layout_scope(function_scope)?;
            let (this, k, v) = (g.temp()?, g.temp()?, g.temp()?);
            g.emit(Op::This { dst: this });
            for (element_index, element) in c.body.iter().enumerate() {
                let ClassElement::Field {
                    key,
                    value,
                    is_static: false,
                    ..
                } = element
                else {
                    continue;
                };
                if matches!(key, PropertyKey::Computed(_)) {
                    let name = field_key_name(element_index);
                    let index = g.tree.scopes[class_scope]
                        .find(&name)
                        .expect("declared by the resolver");
                    g.load_binding_at(class_scope, index, &name, k)?;
                } else {
                    g.property_key(key, k)?;
                }
                match value {
                    Some(value) => {
                        let hint = Self::static_key_name(key);
                        g.expr_named(value, v, hint.as_deref())?;
                    }
                    None => {
                        g.emit(Op::Undefined { dst: v });
                    }
                }
                g.emit(Op::DefineProp {
                    object: this,
                    key: k,
                    value: v,
                });
            }
            g.finish(0, 0)?
        };
        let index = self.functions.len() as u32;
        self.functions.push(code);
        self.emit(Op::Closure { dst, index });
        Ok(())
    }

    /// Run the enclosing class's field initializer, if it has one, on
    /// the object in `this`.
    fn initialize_fields(&mut self, this: Reg) -> CResult<()> {
        let Some((scope, index)) = self.tree.resolve_special(self.scope, FIELD_INIT) else {
            return Ok(());
        };
        if !self.tree.scopes[scope].bindings[index].used {
            return Ok(());
        }
        let mark = self.mark();
        let (init, result) = (self.temp()?, self.temp()?);
        self.load_binding_at(scope, index, FIELD_INIT, init)?;
        self.emit(Op::Call {
            dst: result,
            callee: init,
            this,
            args: init,
            argc: 0,
        });
        self.release(mark);
        Ok(())
    }

    /// The constructor of a class that declares none: empty, or
    /// `constructor(...args) { super(...args) }` for a derived class.
    fn default_constructor(
        &mut self,
        scope: ScopeId,
        dst: Reg,
        name: &str,
        kind: FunctionKind,
        span: Span,
    ) -> CResult<()> {
        let name = self.atom_id(name)?;
        let code = {
            // The code runs in the class scope: it has no scope of its own.
            let mut g = Gen::new(
                self.agent,
                self.tree,
                scope,
                kind,
                true,
                name,
                self.stack_base,
            );
            if kind == FunctionKind::DerivedConstructor {
                let (args, callee, new_target, result) =
                    (g.temp()?, g.temp()?, g.temp()?, g.temp()?);
                g.emit(Op::Rest { dst: args, from: 0 });
                g.load_class(callee, span)?;
                g.emit(Op::GetPrototypeOf {
                    dst: callee,
                    src: callee,
                });
                g.emit(Op::NewTarget { dst: new_target });
                g.emit(Op::SuperCallSpread {
                    dst: result,
                    callee,
                    new_target,
                    array: args,
                });
                g.initialize_fields(result)?;
                g.emit(Op::Return { src: result });
            } else {
                let this = g.temp()?;
                g.emit(Op::This { dst: this });
                g.initialize_fields(this)?;
            }
            g.finish(0, 0)?
        };
        let index = self.functions.len() as u32;
        self.functions.push(code);
        self.emit(Op::Closure { dst, index });
        Ok(())
    }

    /// A register already holding the binding's value, when reading it
    /// needs no instruction (an uncaptured var or parameter).
    fn binding_register(&self, name: &str) -> Option<Reg> {
        match self.resolve(name) {
            Resolution::Binding { scope, index } if !self.binding_kind(scope, index).has_tdz() => {
                self.locals.get(&(scope, index)).copied()
            }
            _ => None,
        }
    }

    /// Store `src` into a binding: `init` for declarations (no TDZ or
    /// constness checks), otherwise an assignment.
    fn store_binding(&mut self, name: &str, src: Reg, init: bool) -> CResult<()> {
        match self.resolve(name) {
            Resolution::Dynamic => {
                if init {
                    let name = self.atom(name)?;
                    self.emit(Op::InitName { src, name });
                } else if let Some(site) = self.global_site(name)? {
                    self.emit(Op::SetGlobal { src, site });
                } else {
                    let name = self.atom(name)?;
                    self.emit(Op::SetName { src, name });
                }
            }
            Resolution::Binding { scope, index } => {
                let kind = self.binding_kind(scope, index);
                if let Some(&r) = self.locals.get(&(scope, index)) {
                    if !init {
                        if kind.has_tdz() {
                            let atom = self.atom(name)?;
                            self.emit(Op::CheckTdz { src: r, name: atom });
                        }
                        if kind == BindingKind::Const {
                            let atom = self.atom(name)?;
                            self.emit(Op::ConstAssign { name: atom });
                            return Ok(());
                        }
                        if kind == BindingKind::SelfName {
                            if self.strict {
                                let atom = self.atom(name)?;
                                self.emit(Op::ConstAssign { name: atom });
                            }
                            return Ok(());
                        }
                    }
                    if r != src {
                        self.emit(Op::Move { dst: r, src });
                    }
                } else {
                    let slot = self.slot_of(scope, index);
                    let hops = self.hops(scope);
                    if init {
                        self.emit(Op::InitSlot { src, hops, slot });
                    } else {
                        self.emit(Op::SetSlot { src, hops, slot });
                    }
                }
            }
        }
        Ok(())
    }

    // ---- programs and functions --------------------------------------

    fn script(&mut self, body: &[Statement]) -> CResult<()> {
        // GlobalDeclarationInstantiation: lexical names, then vars and
        // functions, all by name on the global environment.
        for s in body {
            match &s.kind {
                StatementKind::Variable(d) => {
                    let kind = match d.kind {
                        VariableKind::Var => declare::VAR,
                        VariableKind::Let => declare::LET,
                        VariableKind::Const => declare::CONST,
                    };
                    for decl in &d.declarations {
                        let mut names = Vec::new();
                        pattern_names(&decl.target, &mut names);
                        for n in names {
                            let name = self.atom(&n)?;
                            self.emit(Op::DeclareGlobal { name, kind });
                        }
                    }
                }
                StatementKind::Class(c) => {
                    if let Some(id) = &c.id {
                        let name = self.atom(&id.name)?;
                        self.emit(Op::DeclareGlobal {
                            name,
                            kind: declare::LET,
                        });
                    }
                }
                _ => {}
            }
        }
        let mut vars = Vec::new();
        for s in body {
            collect_nested_vars(s, &mut vars);
        }
        for n in vars {
            let name = self.atom(&n)?;
            self.emit(Op::DeclareGlobal {
                name,
                kind: declare::VAR,
            });
        }
        for s in body {
            if let StatementKind::Function(f) = &s.kind {
                if let Some(id) = &f.id {
                    let name = self.atom(&id.name)?;
                    self.emit(Op::DeclareGlobal {
                        name,
                        kind: declare::VAR,
                    });
                }
            }
        }
        let completion = self.temp()?;
        self.emit(Op::Undefined { dst: completion });
        self.completion = Some(completion);
        self.hoist_functions(body, true)?;
        self.statements(body)?;
        self.emit(Op::Return { src: completion });
        Ok(())
    }

    /// Compile a nested function and load a closure over it into `dst`.
    fn function_expression(
        &mut self,
        f: &Function,
        dst: Reg,
        name_hint: Option<&str>,
        kind: FunctionKind,
    ) -> CResult<()> {
        if f.is_async || f.is_generator {
            return Err(CompileError::Unsupported(
                "generator and async functions",
                f.span,
            ));
        }
        let scope = self.tree.scope_of(f).expect("functions are analyzed");
        let name = match (&f.id, name_hint) {
            (Some(id), _) => self.atom_id(&id.name)?,
            (None, Some(hint)) => self.atom_id(hint)?,
            (None, None) => self.agent.names.empty,
        };
        let code = {
            let mut g = Gen::new(
                self.agent,
                self.tree,
                scope,
                kind,
                f.strict || self.strict || kind.is_class_constructor(),
                name,
                self.stack_base,
            );
            g.home_static = match kind {
                FunctionKind::Normal => None,
                _ => self.next_home.take(),
            };
            g.guard()?;
            if kind == FunctionKind::ClassConstructor {
                // Fields are defined before the parameters are bound. The
                // incoming arguments own the first registers, so work in
                // the ones above them.
                // The function's own environment is not entered yet, so
                // resolve from the scope outside it.
                g.next_reg = f.params.len() as u32;
                g.scope = g.tree.scopes[scope].parent.unwrap_or(scope);
                let this = g.temp()?;
                g.emit(Op::This { dst: this });
                g.initialize_fields(this)?;
                g.scope = scope;
                g.next_reg = 0;
            }
            let (params, length) = g.parameters(&f.params, f.rest.as_ref())?;
            g.function_prologue(&f.body)?;
            g.statements(&f.body)?;
            g.finish(params, length)?
        };
        let index = self.functions.len() as u32;
        self.functions.push(code);
        self.emit(Op::Closure { dst, index });
        Ok(())
    }

    fn arrow_expression(
        &mut self,
        f: &ArrowFunction,
        dst: Reg,
        name_hint: Option<&str>,
    ) -> CResult<()> {
        if f.is_async {
            return Err(CompileError::Unsupported("async functions", f.span));
        }
        let scope = self.tree.scope_of(f).expect("arrows are analyzed");
        let name = match name_hint {
            Some(hint) => self.atom_id(hint)?,
            None => self.agent.names.empty,
        };
        let code = {
            let mut g = Gen::new(
                self.agent,
                self.tree,
                scope,
                FunctionKind::Arrow,
                f.strict || self.strict,
                name,
                self.stack_base,
            );
            g.home_static = self.home_static;
            g.guard()?;
            let (params, length) = g.parameters(&f.params, f.rest.as_ref())?;
            match &f.body {
                ArrowBody::Block(body) => {
                    g.function_prologue(body)?;
                    g.statements(body)?;
                }
                ArrowBody::Expression(e) => {
                    g.function_prologue(&[])?;
                    let r = g.temp()?;
                    g.expr(e, r)?;
                    g.emit(Op::Return { src: r });
                }
            }
            g.finish(params, length)?
        };
        let index = self.functions.len() as u32;
        self.functions.push(code);
        self.emit(Op::Closure { dst, index });
        Ok(())
    }

    /// Lay out the function scope with parameters in registers
    /// `0 .. params`, then bind complex parameters. Returns the parameter
    /// count and the function's `length`.
    fn parameters(&mut self, params: &[Pattern], rest: Option<&Pattern>) -> CResult<(u16, u16)> {
        let count = u16::try_from(params.len()).map_err(|_| CompileError::TooLarge)?;
        // Incoming arguments occupy the first registers.
        self.temps(params.len())?;
        let length = params
            .iter()
            .position(|p| matches!(p.kind, PatternKind::Assignment { .. }))
            .unwrap_or(params.len()) as u16;
        // Simple parameters that stay in registers are the incoming
        // registers themselves.
        let scope = self.function_scope;
        for (i, p) in params.iter().enumerate() {
            if let PatternKind::Identifier(id) = &p.kind {
                if let Some(index) = self.tree.scopes[scope].find(&id.name) {
                    let b = &self.tree.scopes[scope].bindings[index];
                    if !b.captured && b.kind == BindingKind::Param {
                        // A later duplicate parameter wins (sloppy mode).
                        self.locals.insert((scope, index), i as u16);
                    }
                }
            }
        }
        self.layout_scope(scope)?;
        if let Some(&index) = self.scope_index.get(&scope) {
            self.emit(Op::PushScope { scope: index });
            self.depth += 1;
        }
        // Captured vars start undefined; captured lets stay in the TDZ.
        let bindings = self.tree.scopes[scope].bindings.clone();
        let undefined = self.temp()?;
        self.emit(Op::Undefined { dst: undefined });
        for (i, b) in bindings.iter().enumerate() {
            if b.captured && matches!(b.kind, BindingKind::Var | BindingKind::Param) {
                let slot = self.slot_of(scope, i);
                self.emit(Op::InitSlot {
                    src: undefined,
                    hops: 0,
                    slot,
                });
            }
            if !b.captured && b.kind.has_tdz() {
                let r = self.locals[&(scope, i)];
                self.emit(Op::Uninit { dst: r });
            }
            if b.used && b.kind == BindingKind::Arguments {
                let r = self.temp()?;
                self.emit(Op::Arguments { dst: r });
                self.store_binding_at(scope, i, r)?;
            }
            if b.used && b.kind == BindingKind::NewTarget {
                let r = self.temp()?;
                self.emit(Op::NewTarget { dst: r });
                self.store_binding_at(scope, i, r)?;
            }
            if b.used && b.kind == BindingKind::SelfName {
                let r = self.temp()?;
                self.emit(Op::Callee { dst: r });
                self.store_binding_at(scope, i, r)?;
            }
        }
        // Bind parameters left to right.
        for (i, p) in params.iter().enumerate() {
            let incoming = i as Reg;
            match &p.kind {
                PatternKind::Identifier(id)
                    if self.locals.get(&(
                        scope,
                        self.tree.scopes[scope].find(&id.name).unwrap_or(usize::MAX),
                    )) == Some(&incoming) => {}
                _ => self.bind_pattern(p, incoming, true)?,
            }
        }
        if let Some(rest) = rest {
            let r = self.temp()?;
            self.emit(Op::Rest {
                dst: r,
                from: count,
            });
            self.bind_pattern(rest, r, true)?;
        }
        Ok((count, length))
    }

    fn store_binding_at(&mut self, scope: ScopeId, index: usize, src: Reg) -> CResult<()> {
        if let Some(&r) = self.locals.get(&(scope, index)) {
            self.emit(Op::Move { dst: r, src });
        } else {
            let slot = self.slot_of(scope, index);
            let hops = self.hops(scope);
            self.emit(Op::InitSlot { src, hops, slot });
        }
        Ok(())
    }

    fn function_prologue(&mut self, body: &[Statement]) -> CResult<()> {
        self.hoist_functions(body, true)
    }

    // ---- statements --------------------------------------------------

    fn statements(&mut self, body: &[Statement]) -> CResult<()> {
        for s in body {
            self.statement(s)?;
        }
        Ok(())
    }

    fn statement(&mut self, s: &Statement) -> CResult<()> {
        self.guard()?;
        let mark = self.mark();
        let labels = std::mem::take(&mut self.labels);
        match &s.kind {
            StatementKind::Expression { expression, .. } => match self.completion {
                Some(c) => self.expr(expression, c)?,
                None => self.effect(expression)?,
            },
            StatementKind::Block(body) => {
                let scope = self.tree.scope_of(s).expect("blocks are analyzed");
                self.with_breakable(labels, |g| {
                    g.enter_block(scope, body)?;
                    g.statements(body)?;
                    g.leave_block(scope);
                    Ok(())
                })?;
            }
            StatementKind::Empty => {}
            StatementKind::Debugger => {
                self.emit(Op::Nop);
            }
            StatementKind::Variable(d) => self.variable_declaration(d)?,
            StatementKind::Function(_) => {} // hoisted
            StatementKind::Class(c) => {
                let r = self.temp()?;
                self.class_definition(c, r, None)?;
                if let Some(id) = &c.id {
                    self.store_binding(&id.name, r, true)?;
                }
            }
            StatementKind::If {
                test,
                consequent,
                alternate,
            } => {
                self.with_breakable(labels, |g| {
                    let to_else = g.jump_unless(test)?;
                    g.statement(consequent)?;
                    match alternate {
                        Some(a) => {
                            let to_end = g.jump();
                            g.patch_here(to_else);
                            g.statement(a)?;
                            g.patch_here(to_end);
                        }
                        None => g.patch_here(to_else),
                    }
                    Ok(())
                })?;
            }
            StatementKind::For {
                init,
                test,
                update,
                body,
            } => self.for_statement(
                s,
                labels,
                init.as_ref(),
                test.as_ref(),
                update.as_ref(),
                body,
            )?,
            StatementKind::ForIn { left, right, body } => {
                self.for_in_of(s, labels, left, right, body, false)?;
            }
            StatementKind::ForOf {
                left,
                right,
                body,
                is_await,
            } => {
                if *is_await {
                    return Err(CompileError::Unsupported("for await", s.span));
                }
                self.for_in_of(s, labels, left, right, body, true)?;
            }
            StatementKind::While { test, body } => {
                self.push_control(ControlKind::Loop, labels, true);
                let top = self.here();
                self.controls.last_mut().expect("loop").continue_target = Some(top);
                let exit = self.jump_unless(test)?;
                self.statement(body)?;
                self.emit(Op::Loop { target: top });
                self.patch_here(exit);
                self.pop_control();
            }
            StatementKind::DoWhile { body, test } => {
                self.push_control(ControlKind::Loop, labels, true);
                let top = self.here();
                self.statement(body)?;
                let cont = self.here();
                self.controls.last_mut().expect("loop").continue_target = Some(cont);
                let back = self.jump_unless(test)?;
                self.emit(Op::Loop { target: top });
                self.patch_here(back);
                self.pop_control();
            }
            StatementKind::Continue(label) => {
                let target = self.find_control(label.as_ref().map(|l| &*l.name), true, s.span)?;
                self.emit_exit(Exit::Continue(target), None)?;
            }
            StatementKind::Break(label) => {
                let target = self.find_control(label.as_ref().map(|l| &*l.name), false, s.span)?;
                self.emit_exit(Exit::Break(target), None)?;
            }
            StatementKind::Return(e) => {
                let r = self.temp()?;
                match e {
                    Some(e) => self.expr(e, r)?,
                    None => {
                        self.emit(Op::Undefined { dst: r });
                    }
                }
                if self.kind == FunctionKind::DerivedConstructor {
                    // An object is returned as is; `undefined` means
                    // `this`; anything else is a TypeError.
                    self.emit(Op::DerivedResult { src: r });
                    let done = self.emit(Op::JumpIfNotUndefined { src: r, target: 0 });
                    self.load_this(r)?;
                    self.patch_here(done);
                }
                self.emit_exit(Exit::Return, Some(r))?;
            }
            StatementKind::With { object, body } => {
                let r = self.temp()?;
                self.expr(object, r)?;
                let scope = self.tree.scope_of(s).expect("with is analyzed");
                self.with_breakable(labels, |g| {
                    g.emit(Op::PushWith { object: r });
                    g.depth += 1;
                    let outer = g.scope;
                    g.scope = scope;
                    g.statement(body)?;
                    g.scope = outer;
                    g.emit(Op::PopScope);
                    g.depth -= 1;
                    Ok(())
                })?;
            }
            StatementKind::Switch {
                discriminant,
                cases,
            } => self.switch_statement(s, labels, discriminant, cases)?,
            StatementKind::Labeled { label, body } => {
                let mut labels = labels;
                labels.push(label.name.clone());
                self.labels = labels;
                self.statement(body)?;
                self.labels.clear();
            }
            StatementKind::Throw(e) => {
                let r = self.temp()?;
                self.expr(e, r)?;
                self.emit(Op::Throw { src: r });
            }
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => self.try_statement(s, labels, block, handler.as_ref(), finalizer.as_deref())?,
            StatementKind::Import(_) | StatementKind::Export(_) => {
                return Err(CompileError::Unsupported("modules", s.span));
            }
        }
        self.release(mark);
        Ok(())
    }

    /// Run `body` inside a breakable control when the statement has
    /// labels (so `break label` can leave it).
    fn with_breakable(
        &mut self,
        labels: Vec<Box<str>>,
        body: impl FnOnce(&mut Self) -> CResult<()>,
    ) -> CResult<()> {
        if labels.is_empty() {
            return body(self);
        }
        self.push_control(ControlKind::Breakable, labels, false);
        body(self)?;
        self.pop_control();
        Ok(())
    }

    fn push_control(&mut self, kind: ControlKind, labels: Vec<Box<str>>, unlabeled_break: bool) {
        self.controls.push(Control {
            kind,
            labels,
            unlabeled_break,
            break_depth: self.depth,
            continue_depth: self.depth,
            breaks: Vec::new(),
            continues: Vec::new(),
            continue_target: None,
        });
    }

    /// Close the innermost control: patch its breaks here and its
    /// continues to its continue target.
    fn pop_control(&mut self) {
        let c = self.controls.pop().expect("control to pop");
        let here = self.here();
        for b in c.breaks {
            self.patch(b, here);
        }
        if let Some(target) = c.continue_target {
            for at in c.continues {
                self.patch(at, target);
            }
        }
    }

    fn find_control(&self, label: Option<&str>, is_continue: bool, span: Span) -> CResult<usize> {
        for (i, c) in self.controls.iter().enumerate().rev() {
            let matches = match label {
                Some(l) => c.labels.iter().any(|x| &**x == l),
                None => c.unlabeled_break,
            };
            if matches && (!is_continue || matches!(c.kind, ControlKind::Loop)) {
                return Ok(i);
            }
        }
        // The parser's early errors make this unreachable for valid code.
        Err(CompileError::Unsupported("jump target", span))
    }

    /// Leave controls for `exit`, running finally blocks and closing
    /// iterators on the way. `value` is the return value.
    fn emit_exit(&mut self, exit: Exit, value: Option<Reg>) -> CResult<()> {
        let floor = match exit {
            Exit::Break(t) | Exit::Continue(t) => t + 1,
            Exit::Return => 0,
        };
        for i in (floor..self.controls.len()).rev() {
            match &self.controls[i].kind {
                ControlKind::Finally { .. } => {
                    // Route through the finally block: record the exit,
                    // set the dispatch registers and jump to its entry.
                    let ControlKind::Finally {
                        kind,
                        value: value_reg,
                        exits,
                        ..
                    } = &mut self.controls[i].kind
                    else {
                        unreachable!()
                    };
                    let (kind, value_reg) = (*kind, *value_reg);
                    let id = match exits.iter().position(|e| *e == exit) {
                        Some(p) => p,
                        None => {
                            exits.push(exit);
                            exits.len() - 1
                        }
                    };
                    if let Some(v) = value {
                        self.emit(Op::Move {
                            dst: value_reg,
                            src: v,
                        });
                    }
                    let pops = self.depth - self.controls[i].break_depth;
                    for _ in 0..pops {
                        self.emit(Op::PopScope);
                    }
                    self.emit(Op::Int {
                        dst: kind,
                        value: 2 + id as i32,
                    });
                    let j = self.jump();
                    if let ControlKind::Finally { entry_jumps, .. } = &mut self.controls[i].kind {
                        entry_jumps.push(j);
                    }
                    return Ok(());
                }
                ControlKind::Iterator { iterator, owner } => {
                    if exit != Exit::Continue(*owner) {
                        let iterator = *iterator;
                        self.emit(Op::IteratorClose {
                            iterator,
                            silent: false,
                        });
                    }
                }
                ControlKind::Loop | ControlKind::Breakable => {}
            }
        }
        match exit {
            Exit::Return => {
                self.emit(Op::Return {
                    src: value.expect("return value"),
                });
            }
            Exit::Break(t) => {
                let pops = self.depth - self.controls[t].break_depth;
                for _ in 0..pops {
                    self.emit(Op::PopScope);
                }
                let j = self.jump();
                self.controls[t].breaks.push(j);
            }
            Exit::Continue(t) => {
                let pops = self.depth - self.controls[t].continue_depth;
                for _ in 0..pops {
                    self.emit(Op::PopScope);
                }
                // A continue can jump backwards (while, for-in, for-of), and
                // every backward jump must be a safe point, or a loop made
                // only of continues could never be cancelled.
                let j = self.emit(Op::Loop { target: 0 });
                self.controls[t].continues.push(j);
            }
        }
        Ok(())
    }

    fn variable_declaration(&mut self, d: &VariableDeclaration) -> CResult<()> {
        for decl in &d.declarations {
            let mark = self.mark();
            if let (PatternKind::Identifier(id), Some(init)) = (&decl.target.kind, &decl.init) {
                // A declaration cannot be a constant assignment, and its
                // TDZ ends here.
                if let Some(local) = self.local_register(&id.name) {
                    self.expr_into_local(init, local, Some(&id.name))?;
                    self.release(mark);
                    continue;
                }
            }
            let r = self.temp()?;
            match &decl.init {
                Some(init) => {
                    let hint = match &decl.target.kind {
                        PatternKind::Identifier(id) => Some(&*id.name),
                        _ => None,
                    };
                    self.expr_named(init, r, hint)?;
                }
                None if d.kind == VariableKind::Var => continue,
                None => {
                    self.emit(Op::Undefined { dst: r });
                }
            }
            // `var` assigns; `let`/`const` initialize.
            let init = d.kind != VariableKind::Var;
            self.bind_pattern(&decl.target, r, init)?;
            self.release(mark);
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the for statement's parts"
    )]
    fn for_statement(
        &mut self,
        s: &Statement,
        labels: Vec<Box<str>>,
        init: Option<&ForInit>,
        test: Option<&Expression>,
        update: Option<&Expression>,
        body: &Statement,
    ) -> CResult<()> {
        let scope = self.tree.scope_of(s).expect("for is analyzed");
        let outer_depth = self.depth;
        self.enter_block(scope, &[])?;
        let per_iteration = self.scope_index.contains_key(&scope);
        match init {
            Some(ForInit::Variable(d)) => self.variable_declaration(d)?,
            Some(ForInit::Expression(e)) => self.effect(e)?,
            None => {}
        }
        if per_iteration {
            self.emit(Op::CopyScope);
        }
        self.push_control(ControlKind::Loop, labels, true);
        self.controls.last_mut().expect("loop").break_depth = outer_depth;
        let top = self.here();
        let exit = match test {
            Some(t) => Some(self.jump_unless(t)?),
            None => None,
        };
        self.statement(body)?;
        let cont = self.here();
        self.controls.last_mut().expect("loop").continue_target = Some(cont);
        if per_iteration {
            self.emit(Op::CopyScope);
        }
        if let Some(u) = update {
            self.effect(u)?;
        }
        self.emit(Op::Loop { target: top });
        if let Some(e) = exit {
            self.patch_here(e);
        }
        // Breaks pop the loop scope themselves; the normal exit does here.
        let c = self.controls.pop().expect("loop");
        let normal_exit_pop = self.scope_index.contains_key(&scope);
        if normal_exit_pop {
            self.emit(Op::PopScope);
        }
        let here = self.here();
        for b in c.breaks {
            self.patch(b, here);
        }
        for at in c.continues {
            self.patch(at, cont);
        }
        self.depth = outer_depth;
        self.scope = self.tree.scopes[scope].parent.unwrap_or(scope);
        Ok(())
    }

    fn for_in_of(
        &mut self,
        s: &Statement,
        labels: Vec<Box<str>>,
        left: &ForHead,
        right: &Expression,
        body: &Statement,
        of: bool,
    ) -> CResult<()> {
        let scope = self.tree.scope_of(s).expect("for-in/of is analyzed");
        // The right side sees the loop's lexical names in their TDZ; this
        // engine evaluates it outside the loop scope (TDZ violations there
        // read the outer binding instead of throwing).
        let src = self.temp()?;
        self.expr(right, src)?;
        // Three consecutive registers: iterator and `next` for for-of,
        // the for-in state otherwise.
        let iterator = self.temps(3)?;
        let next = iterator + 1;
        if of {
            self.emit(Op::GetIterator {
                iterator,
                next,
                src,
            });
        } else {
            self.emit(Op::ForInStart {
                state: iterator,
                src,
            });
        }
        let outer_depth = self.depth;
        self.layout_scope(scope)?;
        let has_env = self.scope_index.contains_key(&scope);
        // For-of needs both: a loop for continue, and iterator closing on
        // exits. Model it as a loop whose exits close the iterator.
        self.push_control(ControlKind::Loop, labels, true);
        let loop_index = self.controls.len() - 1;
        let kind = ControlKind::Iterator {
            iterator,
            owner: loop_index,
        };
        let top = self.here();
        self.controls[loop_index].continue_target = Some(top);
        let value = self.temp()?;
        let step = if of {
            self.emit(Op::IteratorStep {
                dst: value,
                iterator,
                next,
                done: 0,
            })
        } else {
            self.emit(Op::ForInNext {
                dst: value,
                state: iterator,
                done: 0,
            })
        };
        // Per-iteration scope.
        let outer_scope = self.scope;
        self.scope = scope;
        if has_env {
            let index = self.scope_index[&scope];
            self.emit(Op::PushScope { scope: index });
            self.depth += 1;
        }
        self.controls[loop_index].continue_depth = outer_depth;
        self.controls[loop_index].break_depth = outer_depth;
        if of {
            self.push_control(kind, Vec::new(), false);
            // Exits through this control close the iterator; it never
            // matches a break or continue itself.
        }
        // Bind the head, with a handler that closes the iterator if the
        // body throws.
        let guard_start = self.here();
        match left {
            ForHead::Variable(d) => {
                let init = d.kind != VariableKind::Var;
                let decl = &d.declarations[0];
                if d.kind != VariableKind::Var {
                    let bindings = self.tree.scopes[scope].bindings.clone();
                    for (i, b) in bindings.iter().enumerate() {
                        if !b.captured && b.kind.has_tdz() {
                            let r = self.locals[&(scope, i)];
                            self.emit(Op::Uninit { dst: r });
                        }
                    }
                }
                self.bind_pattern(&decl.target, value, init)?;
            }
            ForHead::Pattern(p) => self.bind_pattern(p, value, false)?,
        }
        self.statement(body)?;
        let guard_end = self.here();
        if of {
            self.controls.pop();
        }
        if has_env {
            self.emit(Op::PopScope);
            self.depth -= 1;
        }
        self.emit(Op::Loop { target: top });
        let close_landing = if of {
            let skip = self.jump();
            let landing = self.here();
            let exception = self.temp()?;
            self.handlers.push(Handler {
                start: guard_start,
                end: guard_end,
                target: landing,
                exception,
                scope_depth: outer_depth,
            });
            self.emit(Op::IteratorClose {
                iterator,
                silent: true,
            });
            self.emit(Op::Throw { src: exception });
            Some(skip)
        } else {
            None
        };
        self.patch_here(step);
        if let Some(skip) = close_landing {
            self.patch_here(skip);
        }
        self.scope = outer_scope;
        self.depth = outer_depth;
        self.pop_control();
        Ok(())
    }

    fn switch_statement(
        &mut self,
        s: &Statement,
        labels: Vec<Box<str>>,
        discriminant: &Expression,
        cases: &[crate::ast::SwitchCase],
    ) -> CResult<()> {
        let d = self.temp()?;
        self.expr(discriminant, d)?;
        let scope = self.tree.scope_of(s).expect("switch is analyzed");
        let outer_depth = self.depth;
        let all: Vec<Statement> = Vec::new();
        self.enter_block(scope, &all)?;
        for c in cases {
            self.hoist_functions(&c.body, false)?;
        }
        self.push_control(ControlKind::Breakable, labels, true);
        self.controls.last_mut().expect("switch").break_depth = outer_depth;
        let mut case_jumps = Vec::new();
        let t = self.temp()?;
        for c in cases {
            if let Some(test) = &c.test {
                self.expr(test, t)?;
                case_jumps.push(Some(self.emit(Op::JumpIfStrictEq {
                    a: d,
                    b: t,
                    target: 0,
                })));
            } else {
                case_jumps.push(None);
            }
        }
        let to_default = self.jump();
        let mut default_target = None;
        for (c, j) in cases.iter().zip(case_jumps) {
            let here = self.here();
            match j {
                Some(j) => self.patch(j, here),
                None => default_target = Some(here),
            }
            self.statements(&c.body)?;
        }
        let end_of_bodies = self.here();
        self.patch(to_default, default_target.unwrap_or(end_of_bodies));
        let c = self.controls.pop().expect("switch");
        if self.scope_index.contains_key(&scope) {
            self.emit(Op::PopScope);
        }
        let here = self.here();
        for b in c.breaks {
            self.patch(b, here);
        }
        self.depth = outer_depth;
        self.scope = self.tree.scopes[scope].parent.unwrap_or(scope);
        Ok(())
    }

    fn try_statement(
        &mut self,
        s: &Statement,
        labels: Vec<Box<str>>,
        block: &[Statement],
        handler: Option<&crate::ast::CatchClause>,
        finalizer: Option<&[Statement]>,
    ) -> CResult<()> {
        self.with_breakable(labels, |g| {
            let depth = g.depth;
            let finally_regs = match finalizer {
                Some(_) => {
                    let kind = g.temp()?;
                    let value = g.temp()?;
                    g.push_control(
                        ControlKind::Finally {
                            kind,
                            value,
                            entry_jumps: Vec::new(),
                            exits: Vec::new(),
                        },
                        Vec::new(),
                        false,
                    );
                    Some((kind, value))
                }
                None => None,
            };
            let outer_start = g.here();
            let block_scope = g
                .tree
                .scope_of_role(s, ROLE_BODY)
                .expect("try blocks are analyzed");
            let start = g.here();
            g.enter_block(block_scope, block)?;
            g.statements(block)?;
            g.leave_block(block_scope);
            let end = g.here();
            let mut to_finally = Vec::new();
            match finally_regs {
                Some((kind, _)) => {
                    g.emit(Op::Int {
                        dst: kind,
                        value: 0,
                    });
                    to_finally.push(g.jump());
                }
                None => to_finally.push(g.jump()),
            }
            if let Some(h) = handler {
                let landing = g.here();
                let exception = g.temp()?;
                g.handlers.push(Handler {
                    start,
                    end,
                    target: landing,
                    exception,
                    scope_depth: depth,
                });
                let catch_scope = g.tree.scope_of(h).expect("catch is analyzed");
                g.enter_block(catch_scope, &[])?;
                if let Some(p) = &h.param {
                    g.bind_pattern(p, exception, true)?;
                }
                let body_scope = g
                    .tree
                    .scope_of_role(h, ROLE_BODY)
                    .expect("catch body is analyzed");
                g.enter_block(body_scope, &h.body)?;
                g.statements(&h.body)?;
                g.leave_block(body_scope);
                g.leave_block(catch_scope);
                if let Some((kind, _)) = finally_regs {
                    g.emit(Op::Int {
                        dst: kind,
                        value: 0,
                    });
                }
                to_finally.push(g.jump());
            }
            let outer_end = g.here();
            match (finalizer, finally_regs) {
                (Some(fin), Some((kind, value))) => {
                    // Throws anywhere in try or catch land here.
                    let landing = g.here();
                    g.handlers.push(Handler {
                        start: outer_start,
                        end: outer_end,
                        target: landing,
                        exception: value,
                        scope_depth: depth,
                    });
                    g.emit(Op::Int {
                        dst: kind,
                        value: -1,
                    });
                    let entry = g.here();
                    for j in to_finally {
                        g.patch(j, entry);
                    }
                    let control = g.controls.pop().expect("finally control");
                    let ControlKind::Finally {
                        entry_jumps, exits, ..
                    } = control.kind
                    else {
                        unreachable!()
                    };
                    for j in entry_jumps {
                        g.patch(j, entry);
                    }
                    let fin_scope = g
                        .tree
                        .scope_of_role(s, ROLE_FINALLY)
                        .expect("finally is analyzed");
                    g.enter_block(fin_scope, fin)?;
                    g.statements(fin)?;
                    g.leave_block(fin_scope);
                    // Dispatch on how the protected code finished.
                    let normal = g.emit(Op::JumpIfInt {
                        a: kind,
                        value: 0,
                        target: 0,
                    });
                    let rethrow = g.emit(Op::JumpIfInt {
                        a: kind,
                        value: -1,
                        target: 0,
                    });
                    let mut routes = Vec::new();
                    for (id, _) in exits.iter().enumerate() {
                        routes.push(g.emit(Op::JumpIfInt {
                            a: kind,
                            value: 2 + id as i32,
                            target: 0,
                        }));
                    }
                    g.patch_here(rethrow);
                    g.emit(Op::Throw { src: value });
                    for (exit, at) in exits.into_iter().zip(routes) {
                        g.patch_here(at);
                        g.emit_exit(exit, Some(value))?;
                    }
                    g.patch_here(normal);
                }
                _ => {
                    let here = g.here();
                    for j in to_finally {
                        g.patch(j, here);
                    }
                }
            }
            Ok(())
        })
    }

    // ---- patterns ----------------------------------------------------

    /// Bind or assign `value` to a pattern.
    fn bind_pattern(&mut self, p: &Pattern, value: Reg, init: bool) -> CResult<()> {
        self.guard()?;
        match &p.kind {
            PatternKind::Identifier(id) => self.store_binding(&id.name, value, init),
            PatternKind::Member(target) => {
                let mark = self.mark();
                self.assign_member(target, value)?;
                self.release(mark);
                Ok(())
            }
            PatternKind::Assignment { target, default } => {
                let mark = self.mark();
                let v = self.temp()?;
                self.emit(Op::Move { dst: v, src: value });
                let skip = self.emit(Op::JumpIfNotUndefined { src: v, target: 0 });
                let hint = match &target.kind {
                    PatternKind::Identifier(id) => Some(&*id.name),
                    _ => None,
                };
                self.expr_named(default, v, hint)?;
                self.patch_here(skip);
                self.bind_pattern(target, v, init)?;
                self.release(mark);
                Ok(())
            }
            PatternKind::Object { properties, rest } => {
                let mark = self.mark();
                let object = self.temp()?;
                // RequireObjectCoercible.
                self.emit(Op::Unary {
                    op: UnOp::ToObject,
                    dst: object,
                    src: value,
                });
                let keys = if rest.is_some() {
                    Some(self.temps(properties.len())?)
                } else {
                    None
                };
                for (i, prop) in properties.iter().enumerate() {
                    let inner = self.mark();
                    let v = self.temp()?;
                    let key_reg = keys.map(|k| k + i as Reg);
                    match (&prop.key, key_reg) {
                        (PropertyKey::Identifier(id), None) => {
                            self.get_named(v, value, &id.name)?;
                        }
                        (key, _) => {
                            let k = match key_reg {
                                Some(k) => k,
                                None => self.temp()?,
                            };
                            self.property_key(key, k)?;
                            self.emit(Op::GetProp {
                                dst: v,
                                object: value,
                                key: k,
                            });
                        }
                    }
                    self.bind_pattern(&prop.value, v, init)?;
                    self.release(inner.max(self.mark().min(inner)));
                    self.next_reg = inner;
                }
                if let Some(rest) = rest {
                    let r = self.temp()?;
                    self.emit(Op::NewObject { dst: r });
                    self.emit(Op::CopyProps {
                        target: r,
                        source: value,
                        excluded: keys.unwrap_or(0),
                        count: properties.len() as u16,
                    });
                    self.bind_pattern(rest, r, init)?;
                }
                self.release(mark);
                Ok(())
            }
            PatternKind::Array { elements, rest } => {
                let mark = self.mark();
                let iterator = self.temp()?;
                let next = self.temp()?;
                let done = self.temp()?;
                let v = self.temp()?;
                self.emit(Op::GetIterator {
                    iterator,
                    next,
                    src: value,
                });
                self.load_bool(done, false);
                for element in elements {
                    let skip = self.emit(Op::JumpIfTrue {
                        cond: done,
                        target: 0,
                    });
                    let step = self.emit(Op::IteratorStep {
                        dst: v,
                        iterator,
                        next,
                        done: 0,
                    });
                    let have = self.jump();
                    self.patch_here(step);
                    self.load_bool(done, true);
                    self.patch_here(skip);
                    self.emit(Op::Undefined { dst: v });
                    self.patch_here(have);
                    if let Some(p) = element {
                        self.bind_pattern(p, v, init)?;
                    }
                }
                if let Some(rest) = rest {
                    let array = self.temp()?;
                    self.emit(Op::NewArray { dst: array });
                    let skip = self.emit(Op::JumpIfTrue {
                        cond: done,
                        target: 0,
                    });
                    let top = self.here();
                    let step = self.emit(Op::IteratorStep {
                        dst: v,
                        iterator,
                        next,
                        done: 0,
                    });
                    self.emit(Op::ArrayPush { array, value: v });
                    self.emit(Op::Jump { target: top });
                    self.patch_here(step);
                    self.load_bool(done, true);
                    self.patch_here(skip);
                    self.bind_pattern(rest, array, init)?;
                }
                let finished = self.emit(Op::JumpIfTrue {
                    cond: done,
                    target: 0,
                });
                self.emit(Op::IteratorClose {
                    iterator,
                    silent: false,
                });
                self.patch_here(finished);
                self.release(mark);
                Ok(())
            }
        }
    }

    /// `target = value` where target is a member expression.
    fn assign_member(&mut self, target: &Expression, value: Reg) -> CResult<()> {
        match &target.kind {
            ExpressionKind::Member {
                object, property, ..
            } => {
                let o = self.temp()?;
                self.expr(object, o)?;
                match property {
                    MemberProperty::Name(id) => self.set_named(o, &id.name, value)?,
                    MemberProperty::Computed(k) => {
                        let kr = self.temp()?;
                        self.expr(k, kr)?;
                        self.emit(Op::SetProp {
                            object: o,
                            key: kr,
                            value,
                        });
                    }
                    MemberProperty::Private(_) => {
                        return Err(CompileError::Unsupported("private names", target.span));
                    }
                }
                Ok(())
            }
            ExpressionKind::SuperMember(property) => {
                let mark = self.mark();
                let (this, base, key) = (self.temp()?, self.temp()?, self.temp()?);
                self.load_this(this)?;
                self.super_key(property, key, target.span)?;
                self.load_super_base(base, target.span)?;
                self.emit(Op::SetSuper {
                    object: base,
                    key,
                    value,
                    this,
                });
                self.release(mark);
                Ok(())
            }
            ExpressionKind::Parenthesized(inner) => self.assign_member(inner, value),
            _ => Err(CompileError::Unsupported("assignment target", target.span)),
        }
    }

    // ---- expressions -------------------------------------------------

    fn expr_named(&mut self, e: &Expression, dst: Reg, name: Option<&str>) -> CResult<()> {
        match (&e.kind, name) {
            (ExpressionKind::Function(f), Some(n)) if f.id.is_none() => {
                self.function_expression(f, dst, Some(n), FunctionKind::Normal)
            }
            (ExpressionKind::Arrow(f), Some(n)) => self.arrow_expression(f, dst, Some(n)),
            (ExpressionKind::Class(c), Some(n)) if c.id.is_none() => {
                self.class_definition(c, dst, Some(n))
            }
            _ => self.expr(e, dst),
        }
    }

    /// Evaluate `e` into a register: the binding's own register when that
    /// is safe (an uncaptured var or parameter read right away), else a
    /// fresh temporary.
    fn operand(&mut self, e: &Expression) -> CResult<Reg> {
        if let ExpressionKind::Identifier(id) = &e.kind {
            if let Some(r) = self.binding_register(&id.name) {
                return Ok(r);
            }
        }
        let r = self.temp()?;
        self.expr(e, r)?;
        Ok(r)
    }

    /// The register of an uncaptured binding, whatever its kind.
    fn local_register(&self, name: &str) -> Option<Reg> {
        match self.resolve(name) {
            Resolution::Binding { scope, index } => self.locals.get(&(scope, index)).copied(),
            Resolution::Dynamic => None,
        }
    }

    /// The register of an uncaptured binding that plain assignment may
    /// write without checks (not `let`/`const`, not a function's own
    /// name).
    fn assignable_register(&self, name: &str) -> Option<Reg> {
        match self.resolve(name) {
            Resolution::Binding { scope, index } => {
                let kind = self.binding_kind(scope, index);
                if kind.has_tdz() || kind == BindingKind::SelfName {
                    return None;
                }
                self.locals.get(&(scope, index)).copied()
            }
            Resolution::Dynamic => None,
        }
    }

    /// Whether evaluating `e` can run no code and write no binding, so a
    /// binding's register read before it and after it holds the same
    /// value.
    fn is_inert(e: &Expression) -> bool {
        match &e.kind {
            ExpressionKind::Number(_)
            | ExpressionKind::String(_)
            | ExpressionKind::Boolean(_)
            | ExpressionKind::Null
            | ExpressionKind::This => true,
            // Reading a global or `with` name may run a getter, but that
            // code cannot reach an uncaptured binding of this frame.
            ExpressionKind::Identifier(_) => true,
            ExpressionKind::Parenthesized(inner) => Self::is_inert(inner),
            _ => false,
        }
    }

    /// A small integer literal, for immediate operands.
    fn small_int(e: &Expression) -> Option<i32> {
        match &e.kind {
            ExpressionKind::Number(n)
                if n.fract() == 0.0
                    && n.abs() <= f64::from(i32::MAX)
                    && !(*n == 0.0 && n.is_sign_negative()) =>
            {
                Some(*n as i32)
            }
            ExpressionKind::Parenthesized(inner) => Self::small_int(inner),
            _ => None,
        }
    }

    /// Evaluate `e` and store it in a binding's own register. The
    /// register is written only by the last instruction, after every
    /// read of the old value, as an assignment must.
    fn expr_into_local(&mut self, e: &Expression, local: Reg, name: Option<&str>) -> CResult<()> {
        match &e.kind {
            ExpressionKind::Binary { .. } => self.binary_chain(e, local),
            _ => {
                let mark = self.mark();
                let v = self.temp()?;
                self.expr_named(e, v, name)?;
                self.emit(Op::Move { dst: local, src: v });
                self.release(mark);
                Ok(())
            }
        }
    }

    /// Emit a jump taken when `test` is falsy; returns the jump to patch.
    /// A comparison is fused with its branch.
    fn jump_unless(&mut self, test: &Expression) -> CResult<usize> {
        let mark = self.mark();
        let mut t = test;
        while let ExpressionKind::Parenthesized(inner) = &t.kind {
            t = inner;
        }
        if let ExpressionKind::Binary { op, left, right } = &t.kind {
            let op = Self::binop(*op);
            if matches!(
                op,
                BinOp::Lt
                    | BinOp::LtEq
                    | BinOp::Gt
                    | BinOp::GtEq
                    | BinOp::Eq
                    | BinOp::NotEq
                    | BinOp::StrictEq
                    | BinOp::StrictNotEq
            ) {
                // The left side is read in place only when the right side
                // cannot change it.
                let a = match &left.kind {
                    ExpressionKind::Identifier(id) if Self::is_inert(right) => {
                        self.binding_register(&id.name)
                    }
                    _ => None,
                };
                let a = match a {
                    Some(a) => a,
                    None => {
                        let r = self.temp()?;
                        self.expr(left, r)?;
                        r
                    }
                };
                let at = match Self::small_int(right) {
                    Some(value) => self.emit(Op::JumpUnlessInt {
                        op,
                        a,
                        value,
                        target: 0,
                    }),
                    None => {
                        let b = self.operand(right)?;
                        self.emit(Op::JumpUnless {
                            op,
                            a,
                            b,
                            target: 0,
                        })
                    }
                };
                self.release(mark);
                return Ok(at);
            }
        }
        let r = self.operand(t)?;
        let at = self.emit(Op::JumpIfFalse { cond: r, target: 0 });
        self.release(mark);
        Ok(at)
    }

    /// Evaluate `e` for its effects only.
    fn effect(&mut self, e: &Expression) -> CResult<()> {
        let mark = self.mark();
        match &e.kind {
            ExpressionKind::Parenthesized(inner) => self.effect(inner)?,
            ExpressionKind::Update { op, target, .. } => {
                let local = match &target.kind {
                    PatternKind::Identifier(id) => self.assignable_register(&id.name),
                    _ => None,
                };
                match local {
                    // The old value is not needed, so the conversion and
                    // the step are one instruction.
                    Some(r) => {
                        let op = match op {
                            UpdateOp::Increment => UnOp::Inc,
                            UpdateOp::Decrement => UnOp::Dec,
                        };
                        self.emit(Op::Unary { op, dst: r, src: r });
                    }
                    None => {
                        let r = self.temp()?;
                        self.expr(e, r)?;
                    }
                }
            }
            ExpressionKind::Assignment { op, target, value } => {
                let local = match &target.kind {
                    PatternKind::Identifier(id) => {
                        self.assignable_register(&id.name).map(|r| (r, &*id.name))
                    }
                    _ => None,
                };
                match (local, Self::assign_op(*op)) {
                    (Some((r, name)), None) if *op == AssignOp::Assign => {
                        self.expr_into_local(value, r, Some(name))?;
                    }
                    (Some((r, _)), Some(bin)) if Self::is_inert(value) => {
                        match Self::small_int(value) {
                            Some(imm) => {
                                self.emit(Op::BinaryInt {
                                    op: bin,
                                    dst: r,
                                    a: r,
                                    value: imm,
                                });
                            }
                            None => {
                                let b = self.operand(value)?;
                                self.emit(Op::Binary {
                                    op: bin,
                                    dst: r,
                                    a: r,
                                    b,
                                });
                            }
                        }
                    }
                    _ => {
                        let r = self.temp()?;
                        self.expr(e, r)?;
                    }
                }
            }
            _ => {
                let r = self.temp()?;
                self.expr(e, r)?;
            }
        }
        self.release(mark);
        Ok(())
    }

    fn property_key(&mut self, key: &PropertyKey, dst: Reg) -> CResult<()> {
        match key {
            PropertyKey::Identifier(id) | PropertyKey::Private(id) => {
                let index = self.atom(&id.name)?;
                self.emit(Op::Const { dst, index });
            }
            PropertyKey::String(s, _) => {
                let index = self.string(s)?;
                self.emit(Op::Const { dst, index });
            }
            PropertyKey::Number(n, _) => self.load_number(dst, *n),
            PropertyKey::BigInt(..) => {
                return Err(CompileError::Unsupported(
                    "BigInt property keys",
                    Span::default(),
                ));
            }
            PropertyKey::Computed(e) => {
                self.expr(e, dst)?;
                self.emit(Op::Unary {
                    op: UnOp::ToPropertyKey,
                    dst,
                    src: dst,
                });
            }
        }
        Ok(())
    }

    fn static_key_name(key: &PropertyKey) -> Option<String> {
        match key {
            PropertyKey::Identifier(id) => Some(id.name.to_string()),
            PropertyKey::String(s, _) => String::from_utf16(s).ok(),
            _ => None,
        }
    }

    fn expr(&mut self, e: &Expression, dst: Reg) -> CResult<()> {
        self.guard()?;
        let mark = self.mark();
        match &e.kind {
            ExpressionKind::This => self.load_this(dst)?,
            ExpressionKind::Identifier(id) => {
                if &*id.name == "undefined"
                    && self.resolve("undefined") == Resolution::Dynamic
                    && !self.under_with()
                {
                    // The global `undefined` is read-only and not
                    // configurable, and global lexical declarations may
                    // not shadow it.
                    self.emit(Op::Undefined { dst });
                } else {
                    self.load_binding(&id.name, dst)?;
                }
            }
            ExpressionKind::Null => {
                let index = self.push_constant(Value::NULL);
                self.emit(Op::Const { dst, index });
            }
            ExpressionKind::Boolean(b) => self.load_bool(dst, *b),
            ExpressionKind::Number(n) => self.load_number(dst, *n),
            ExpressionKind::String(s) => {
                let index = self.string(s)?;
                self.emit(Op::Const { dst, index });
            }
            ExpressionKind::BigInt(radix, digits) => {
                let big = crate::runtime::BigInt::from_digits(digits, *radix, false)
                    .ok_or(CompileError::Unsupported("BigInt literal", e.span))?;
                let id = self.agent.heap.alloc_bigint(big)?;
                let index = self.push_constant(Value::bigint(id));
                self.emit(Op::Const { dst, index });
            }
            ExpressionKind::RegExp { .. } => {
                return Err(CompileError::Unsupported("regular expressions", e.span));
            }
            ExpressionKind::Template(t) => {
                let first = t.quasis[0].cooked.clone().unwrap_or_default();
                let index = self.string(&first)?;
                self.emit(Op::Const { dst, index });
                for (expression, quasi) in t.expressions.iter().zip(&t.quasis[1..]) {
                    let r = self.temp()?;
                    self.expr(expression, r)?;
                    self.emit(Op::Unary {
                        op: UnOp::ToString,
                        dst: r,
                        src: r,
                    });
                    self.emit(Op::Binary {
                        op: BinOp::Add,
                        dst,
                        a: dst,
                        b: r,
                    });
                    let text = quasi.cooked.clone().unwrap_or_default();
                    if !text.is_empty() {
                        let index = self.string(&text)?;
                        self.emit(Op::Const { dst: r, index });
                        self.emit(Op::Binary {
                            op: BinOp::Add,
                            dst,
                            a: dst,
                            b: r,
                        });
                    }
                }
            }
            ExpressionKind::TaggedTemplate { .. } => {
                return Err(CompileError::Unsupported("tagged templates", e.span));
            }
            ExpressionKind::Array(elements) => {
                let array = self.temp()?;
                self.emit(Op::NewArray { dst: array });
                let v = self.temp()?;
                for el in elements {
                    match el {
                        None => {
                            self.emit(Op::ArrayHole { array });
                        }
                        Some(ArrayElement::Expression(x)) => {
                            self.expr(x, v)?;
                            self.emit(Op::ArrayPush { array, value: v });
                        }
                        Some(ArrayElement::Spread(x)) => self.spread_into(array, x)?,
                    }
                }
                self.emit(Op::Move { dst, src: array });
            }
            ExpressionKind::Object(props) => self.object_literal(props, dst)?,
            ExpressionKind::Function(f) => {
                self.function_expression(f, dst, None, FunctionKind::Normal)?;
            }
            ExpressionKind::Arrow(f) => self.arrow_expression(f, dst, None)?,
            ExpressionKind::Class(c) => self.class_definition(c, dst, None)?,
            ExpressionKind::Unary { op, argument } => self.unary(*op, argument, dst)?,
            ExpressionKind::Update { op, prefix, target } => {
                self.update(*op, *prefix, target, dst)?
            }
            ExpressionKind::Binary { .. } => self.binary_chain(e, dst)?,
            ExpressionKind::PrivateIn { .. } => {
                return Err(CompileError::Unsupported("private names", e.span));
            }
            ExpressionKind::Logical { op, left, right } => {
                self.expr(left, dst)?;
                let skip = match op {
                    LogicalOp::And => self.emit(Op::JumpIfFalse {
                        cond: dst,
                        target: 0,
                    }),
                    LogicalOp::Or => self.emit(Op::JumpIfTrue {
                        cond: dst,
                        target: 0,
                    }),
                    LogicalOp::Coalesce => {
                        let go = self.emit(Op::JumpIfNullish {
                            src: dst,
                            target: 0,
                        });
                        let skip = self.jump();
                        self.patch_here(go);
                        self.expr(right, dst)?;
                        self.patch_here(skip);
                        self.release(mark);
                        return Ok(());
                    }
                };
                self.expr(right, dst)?;
                self.patch_here(skip);
            }
            ExpressionKind::Conditional {
                test,
                consequent,
                alternate,
            } => {
                let to_else = self.jump_unless(test)?;
                self.expr(consequent, dst)?;
                let to_end = self.jump();
                self.patch_here(to_else);
                self.expr(alternate, dst)?;
                self.patch_here(to_end);
            }
            ExpressionKind::Assignment { op, target, value } => {
                self.assignment(*op, target, value, dst)?;
            }
            ExpressionKind::Sequence(items) => {
                if let Some((last, rest)) = items.split_last() {
                    for item in rest {
                        self.effect(item)?;
                    }
                    self.expr(last, dst)?;
                }
            }
            ExpressionKind::Member { .. } => self.member_chain(e, dst, None)?,
            ExpressionKind::SuperMember(property) => {
                let this = self.temp()?;
                self.super_member(property, dst, this, e.span)?;
            }
            ExpressionKind::SuperCall(arguments) => self.super_call(arguments, dst, e.span)?,
            ExpressionKind::Call {
                callee,
                arguments,
                optional,
            } => self.call(callee, arguments, *optional, dst, None)?,
            ExpressionKind::New { callee, arguments } => {
                let c = self.temp()?;
                self.expr(callee, c)?;
                if arguments.iter().any(|a| matches!(a, Argument::Spread(_))) {
                    let array = self.arguments_array(arguments)?;
                    self.emit(Op::NewSpread {
                        dst,
                        callee: c,
                        array,
                    });
                } else {
                    let (args, argc) = self.arguments(arguments)?;
                    self.emit(Op::New {
                        dst,
                        callee: c,
                        args,
                        argc,
                    });
                }
            }
            ExpressionKind::OptionalChain(inner) => {
                let mut exits = Vec::new();
                self.chain_expr(inner, dst, &mut exits)?;
                if !exits.is_empty() {
                    let end = self.jump();
                    for j in exits {
                        self.patch_here(j);
                    }
                    self.emit(Op::Undefined { dst });
                    self.patch_here(end);
                }
            }
            ExpressionKind::Parenthesized(inner) => self.expr(inner, dst)?,
            ExpressionKind::NewTarget => self.load_new_target(dst)?,
            ExpressionKind::ImportMeta | ExpressionKind::ImportCall { .. } => {
                return Err(CompileError::Unsupported("modules", e.span));
            }
            ExpressionKind::Yield { .. } | ExpressionKind::Await(_) => {
                return Err(CompileError::Unsupported(
                    "generator and async functions",
                    e.span,
                ));
            }
        }
        self.release(mark);
        Ok(())
    }

    /// Compile inside an optional chain: optional links jump to `exits`
    /// when their base is nullish.
    fn chain_expr(&mut self, e: &Expression, dst: Reg, exits: &mut Vec<usize>) -> CResult<()> {
        match &e.kind {
            ExpressionKind::Member { .. } => self.member_chain(e, dst, Some(exits)),
            ExpressionKind::Call {
                callee,
                arguments,
                optional,
            } => self.call(callee, arguments, *optional, dst, Some(exits)),
            _ => self.expr(e, dst),
        }
    }

    /// `a.b.c[d]...`: evaluate the base, then each link, iteratively.
    fn member_chain(
        &mut self,
        e: &Expression,
        dst: Reg,
        mut exits: Option<&mut Vec<usize>>,
    ) -> CResult<()> {
        let mut links = Vec::new();
        let mut base = e;
        while let ExpressionKind::Member {
            object,
            property,
            optional,
        } = &base.kind
        {
            links.push((property, *optional, base.span));
            base = object;
        }
        match &mut exits {
            Some(x) => self.chain_expr(base, dst, x)?,
            None => self.expr(base, dst)?,
        }
        for (property, optional, span) in links.into_iter().rev() {
            if optional {
                let j = self.emit(Op::JumpIfNullish {
                    src: dst,
                    target: 0,
                });
                match &mut exits {
                    Some(x) => x.push(j),
                    None => return Err(CompileError::Unsupported("optional chain", span)),
                }
            }
            self.member_link(dst, property, dst, span)?;
        }
        Ok(())
    }

    fn member_link(
        &mut self,
        object: Reg,
        property: &MemberProperty,
        dst: Reg,
        span: Span,
    ) -> CResult<()> {
        match property {
            MemberProperty::Name(id) => self.get_named(dst, object, &id.name),
            MemberProperty::Computed(k) => {
                let mark = self.mark();
                let kr = self.operand(k)?;
                self.emit(Op::GetProp {
                    dst,
                    object,
                    key: kr,
                });
                self.release(mark);
                Ok(())
            }
            MemberProperty::Private(_) => Err(CompileError::Unsupported("private names", span)),
        }
    }

    fn call(
        &mut self,
        callee: &Expression,
        arguments: &[Argument],
        optional: bool,
        dst: Reg,
        mut exits: Option<&mut Vec<usize>>,
    ) -> CResult<()> {
        let mark = self.mark();
        let this = self.temp()?;
        let f = self.temp()?;
        // Unwrap parentheses only for the receiver rule: `(a.b)()` keeps
        // `this`.
        let mut c = callee;
        while let ExpressionKind::Parenthesized(inner) = &c.kind {
            c = inner;
        }
        match &c.kind {
            ExpressionKind::Member {
                object,
                property,
                optional: member_optional,
            } => {
                match &mut exits {
                    Some(x) => self.chain_expr(object, this, x)?,
                    None => self.expr(object, this)?,
                }
                if *member_optional {
                    let j = self.emit(Op::JumpIfNullish {
                        src: this,
                        target: 0,
                    });
                    match &mut exits {
                        Some(x) => x.push(j),
                        None => {
                            return Err(CompileError::Unsupported("optional chain", c.span));
                        }
                    }
                }
                self.member_link(this, property, f, c.span)?;
            }
            ExpressionKind::SuperMember(property) => {
                self.super_member(property, f, this, c.span)?
            }
            ExpressionKind::Identifier(id) => {
                self.load_binding(&id.name, f)?;
                if self.resolve(&id.name) == Resolution::Dynamic && self.under_with() {
                    let name = self.atom(&id.name)?;
                    self.emit(Op::WithBase { dst: this, name });
                } else {
                    self.emit(Op::Undefined { dst: this });
                }
            }
            _ => {
                match &mut exits {
                    Some(x) => self.chain_expr(callee, f, x)?,
                    None => self.expr(callee, f)?,
                }
                self.emit(Op::Undefined { dst: this });
            }
        }
        if optional {
            let j = self.emit(Op::JumpIfNullish { src: f, target: 0 });
            match &mut exits {
                Some(x) => x.push(j),
                None => return Err(CompileError::Unsupported("optional call", callee.span)),
            }
        }
        if arguments.iter().any(|a| matches!(a, Argument::Spread(_))) {
            let array = self.arguments_array(arguments)?;
            self.emit(Op::CallSpread {
                dst,
                callee: f,
                this,
                array,
            });
        } else {
            let (args, argc) = self.arguments(arguments)?;
            self.emit(Op::Call {
                dst,
                callee: f,
                this,
                args,
                argc,
            });
        }
        self.release(mark);
        Ok(())
    }

    /// Evaluate arguments into consecutive registers.
    fn arguments(&mut self, arguments: &[Argument]) -> CResult<(Reg, u16)> {
        let argc = u16::try_from(arguments.len()).map_err(|_| CompileError::TooLarge)?;
        let first = self.temps(arguments.len())?;
        for (i, a) in arguments.iter().enumerate() {
            let Argument::Expression(x) = a else {
                unreachable!("spread handled by the caller")
            };
            self.expr(x, first + i as Reg)?;
        }
        Ok((first, argc))
    }

    fn arguments_array(&mut self, arguments: &[Argument]) -> CResult<Reg> {
        let array = self.temp()?;
        self.emit(Op::NewArray { dst: array });
        let v = self.temp()?;
        for a in arguments {
            match a {
                Argument::Expression(x) => {
                    self.expr(x, v)?;
                    self.emit(Op::ArrayPush { array, value: v });
                }
                Argument::Spread(x) => self.spread_into(array, x)?,
            }
        }
        Ok(array)
    }

    fn spread_into(&mut self, array: Reg, x: &Expression) -> CResult<()> {
        let mark = self.mark();
        let src = self.temp()?;
        self.expr(x, src)?;
        let iterator = self.temp()?;
        let next = self.temp()?;
        let v = self.temp()?;
        self.emit(Op::GetIterator {
            iterator,
            next,
            src,
        });
        let top = self.here();
        let step = self.emit(Op::IteratorStep {
            dst: v,
            iterator,
            next,
            done: 0,
        });
        self.emit(Op::ArrayPush { array, value: v });
        self.emit(Op::Jump { target: top });
        self.patch_here(step);
        self.release(mark);
        Ok(())
    }

    fn object_literal(&mut self, props: &[ObjectProperty], dst: Reg) -> CResult<()> {
        let object = self.temp()?;
        self.emit(Op::NewObject { dst: object });
        let k = self.temp()?;
        let v = self.temp()?;
        for p in props {
            match p {
                ObjectProperty::Property {
                    key,
                    value,
                    shorthand,
                    ..
                } => {
                    let name = Self::static_key_name(key);
                    if !shorthand
                        && name.as_deref() == Some("__proto__")
                        && !matches!(key, PropertyKey::Computed(_))
                    {
                        self.expr(value, v)?;
                        self.emit(Op::SetPrototype { object, value: v });
                        continue;
                    }
                    self.property_key(key, k)?;
                    self.expr_named(value, v, name.as_deref())?;
                    self.emit(Op::DefineProp {
                        object,
                        key: k,
                        value: v,
                    });
                }
                ObjectProperty::Method {
                    key,
                    function,
                    kind,
                    ..
                } => {
                    self.property_key(key, k)?;
                    let name = Self::static_key_name(key);
                    let hint = match kind {
                        MethodKind::Get => name.map(|n| format!("get {n}")),
                        MethodKind::Set => name.map(|n| format!("set {n}")),
                        _ => name,
                    };
                    self.function_expression(function, v, hint.as_deref(), FunctionKind::Method)?;
                    match kind {
                        MethodKind::Get | MethodKind::Set => {
                            self.emit(Op::DefineAccessor {
                                object,
                                key: k,
                                function: v,
                                setter: *kind == MethodKind::Set,
                                enumerable: true,
                            });
                        }
                        _ => {
                            self.emit(Op::SetHome {
                                function: v,
                                home: object,
                            });
                            self.emit(Op::DefineProp {
                                object,
                                key: k,
                                value: v,
                            });
                        }
                    }
                }
                ObjectProperty::Spread(x) => {
                    self.expr(x, v)?;
                    self.emit(Op::CopyProps {
                        target: object,
                        source: v,
                        excluded: 0,
                        count: 0,
                    });
                }
            }
        }
        self.emit(Op::Move { dst, src: object });
        Ok(())
    }

    fn unary(&mut self, op: UnaryOp, argument: &Expression, dst: Reg) -> CResult<()> {
        let mut arg = argument;
        while let ExpressionKind::Parenthesized(inner) = &arg.kind {
            arg = inner;
        }
        match op {
            UnaryOp::Delete => {
                match &arg.kind {
                    ExpressionKind::Member {
                        object, property, ..
                    } => {
                        let o = self.temp()?;
                        self.expr(object, o)?;
                        let k = self.temp()?;
                        match property {
                            MemberProperty::Name(id) => {
                                let index = self.atom(&id.name)?;
                                self.emit(Op::Const { dst: k, index });
                            }
                            MemberProperty::Computed(x) => self.expr(x, k)?,
                            MemberProperty::Private(_) => {
                                return Err(CompileError::Unsupported("private names", arg.span));
                            }
                        }
                        self.emit(Op::DeleteProp {
                            dst,
                            object: o,
                            key: k,
                        });
                    }
                    ExpressionKind::Identifier(id) => match self.resolve(&id.name) {
                        Resolution::Dynamic => {
                            let name = self.atom(&id.name)?;
                            self.emit(Op::DeleteName { dst, name });
                        }
                        Resolution::Binding { .. } => self.load_bool(dst, false),
                    },
                    ExpressionKind::OptionalChain(_) => {
                        return Err(CompileError::Unsupported(
                            "delete of an optional chain",
                            arg.span,
                        ));
                    }
                    _ => {
                        self.expr(arg, dst)?;
                        self.load_bool(dst, true);
                    }
                }
                return Ok(());
            }
            UnaryOp::Typeof => {
                if let ExpressionKind::Identifier(id) = &arg.kind {
                    if self.resolve(&id.name) == Resolution::Dynamic {
                        let name = self.atom(&id.name)?;
                        self.emit(Op::TypeofName { dst, name });
                        return Ok(());
                    }
                }
            }
            _ => {}
        }
        let src = self.operand(arg)?;
        match op {
            UnaryOp::Void => {
                self.emit(Op::Undefined { dst });
            }
            UnaryOp::Minus | UnaryOp::Plus | UnaryOp::Not | UnaryOp::BitNot | UnaryOp::Typeof => {
                let op = match op {
                    UnaryOp::Minus => UnOp::Neg,
                    UnaryOp::Plus => UnOp::Plus,
                    UnaryOp::Not => UnOp::Not,
                    UnaryOp::BitNot => UnOp::BitNot,
                    _ => UnOp::Typeof,
                };
                self.emit(Op::Unary { op, dst, src });
            }
            UnaryOp::Delete => unreachable!("handled above"),
        }
        Ok(())
    }

    fn binop(op: BinaryOp) -> BinOp {
        match op {
            BinaryOp::Eq => BinOp::Eq,
            BinaryOp::NotEq => BinOp::NotEq,
            BinaryOp::StrictEq => BinOp::StrictEq,
            BinaryOp::StrictNotEq => BinOp::StrictNotEq,
            BinaryOp::Lt => BinOp::Lt,
            BinaryOp::LtEq => BinOp::LtEq,
            BinaryOp::Gt => BinOp::Gt,
            BinaryOp::GtEq => BinOp::GtEq,
            BinaryOp::Shl => BinOp::Shl,
            BinaryOp::Sar => BinOp::Sar,
            BinaryOp::Shr => BinOp::Shr,
            BinaryOp::Add => BinOp::Add,
            BinaryOp::Sub => BinOp::Sub,
            BinaryOp::Mul => BinOp::Mul,
            BinaryOp::Div => BinOp::Div,
            BinaryOp::Rem => BinOp::Rem,
            BinaryOp::Exp => BinOp::Exp,
            BinaryOp::BitOr => BinOp::BitOr,
            BinaryOp::BitXor => BinOp::BitXor,
            BinaryOp::BitAnd => BinOp::BitAnd,
            BinaryOp::In => BinOp::In,
            BinaryOp::Instanceof => BinOp::Instanceof,
        }
    }

    /// A left-nested chain of binary operators, folded iteratively.
    /// Intermediate results live in a temporary; only the last operator
    /// writes `dst`, so `dst` may be a binding's own register.
    fn binary_chain(&mut self, e: &Expression, dst: Reg) -> CResult<()> {
        let mut rights = Vec::new();
        let mut base = e;
        while let ExpressionKind::Binary { op, left, right } = &base.kind {
            rights.push((*op, &**right));
            base = left;
        }
        let mark = self.mark();
        let count = rights.len();
        let mut acc: Option<Reg> = None;
        // The leftmost operand can be read from a binding's register in
        // place when nothing evaluated before the operator can change it.
        let mut left = match &base.kind {
            ExpressionKind::Identifier(id)
                if rights.last().is_some_and(|(_, r)| Self::is_inert(r)) =>
            {
                self.binding_register(&id.name)
            }
            _ => None,
        };
        if left.is_none() {
            let r = self.temp()?;
            self.expr(base, r)?;
            acc = Some(r);
            left = Some(r);
        }
        let mut a = left.expect("set above");
        for (i, (op, right)) in rights.into_iter().rev().enumerate() {
            let out = if i + 1 == count {
                dst
            } else {
                match acc {
                    Some(r) => r,
                    None => {
                        let r = self.temp()?;
                        acc = Some(r);
                        r
                    }
                }
            };
            let inner = self.mark();
            let op = Self::binop(op);
            match Self::small_int(right) {
                Some(value) if !matches!(op, BinOp::In | BinOp::Instanceof) => {
                    self.emit(Op::BinaryInt {
                        op,
                        dst: out,
                        a,
                        value,
                    });
                }
                _ => {
                    let b = self.operand(right)?;
                    self.emit(Op::Binary { op, dst: out, a, b });
                }
            }
            self.release(inner);
            a = out;
        }
        self.release(mark);
        Ok(())
    }

    fn assign_op(op: AssignOp) -> Option<BinOp> {
        Some(match op {
            AssignOp::Add => BinOp::Add,
            AssignOp::Sub => BinOp::Sub,
            AssignOp::Mul => BinOp::Mul,
            AssignOp::Div => BinOp::Div,
            AssignOp::Rem => BinOp::Rem,
            AssignOp::Exp => BinOp::Exp,
            AssignOp::Shl => BinOp::Shl,
            AssignOp::Sar => BinOp::Sar,
            AssignOp::Shr => BinOp::Shr,
            AssignOp::BitOr => BinOp::BitOr,
            AssignOp::BitXor => BinOp::BitXor,
            AssignOp::BitAnd => BinOp::BitAnd,
            AssignOp::Assign | AssignOp::And | AssignOp::Or | AssignOp::Coalesce => return None,
        })
    }

    fn assignment(
        &mut self,
        op: AssignOp,
        target: &Pattern,
        value: &Expression,
        dst: Reg,
    ) -> CResult<()> {
        if op == AssignOp::Assign {
            let hint = match &target.kind {
                PatternKind::Identifier(id) => Some(&*id.name),
                _ => None,
            };
            match &target.kind {
                PatternKind::Member(m) => {
                    // Evaluate the reference (object, key) before the value.
                    return self.member_update(m, dst, |g, _old, out| g.expr(value, out), false);
                }
                _ => {
                    let v = self.temp()?;
                    self.expr_named(value, v, hint)?;
                    self.bind_pattern(target, v, false)?;
                    self.emit(Op::Move { dst, src: v });
                    return Ok(());
                }
            }
        }
        let logical = matches!(op, AssignOp::And | AssignOp::Or | AssignOp::Coalesce);
        match &target.kind {
            PatternKind::Identifier(id) => {
                let name = id.name.clone();
                self.load_binding(&name, dst)?;
                if logical {
                    let skip = match op {
                        AssignOp::And => self.emit(Op::JumpIfFalse {
                            cond: dst,
                            target: 0,
                        }),
                        AssignOp::Or => self.emit(Op::JumpIfTrue {
                            cond: dst,
                            target: 0,
                        }),
                        _ => {
                            let go = self.emit(Op::JumpIfNullish {
                                src: dst,
                                target: 0,
                            });
                            let skip = self.jump();
                            self.patch_here(go);
                            self.expr_named(value, dst, Some(&name))?;
                            self.store_binding(&name, dst, false)?;
                            self.patch_here(skip);
                            return Ok(());
                        }
                    };
                    self.expr_named(value, dst, Some(&name))?;
                    self.store_binding(&name, dst, false)?;
                    self.patch_here(skip);
                } else {
                    let r = self.operand(value)?;
                    self.emit(Op::Binary {
                        op: Self::assign_op(op).expect("arithmetic assignment"),
                        dst,
                        a: dst,
                        b: r,
                    });
                    self.store_binding(&name, dst, false)?;
                }
                Ok(())
            }
            PatternKind::Member(m) => {
                if logical {
                    self.member_update(
                        m,
                        dst,
                        |g, old, out| {
                            g.emit(Op::Move { dst: out, src: old });
                            let skip = match op {
                                AssignOp::And => g.emit(Op::JumpIfFalse {
                                    cond: out,
                                    target: 0,
                                }),
                                AssignOp::Or => g.emit(Op::JumpIfTrue {
                                    cond: out,
                                    target: 0,
                                }),
                                _ => {
                                    let go = g.emit(Op::JumpIfNullish {
                                        src: out,
                                        target: 0,
                                    });
                                    let skip = g.jump();
                                    g.patch_here(go);
                                    skip
                                }
                            };
                            g.expr(value, out)?;
                            Ok(skip).map(|s| {
                                // Signal the caller to skip the store.
                                g.pending_skip = Some(s);
                            })
                        },
                        true,
                    )
                } else {
                    let bin = Self::assign_op(op).expect("arithmetic assignment");
                    self.member_update(
                        m,
                        dst,
                        |g, old, out| {
                            let r = g.operand(value)?;
                            g.emit(Op::Binary {
                                op: bin,
                                dst: out,
                                a: old,
                                b: r,
                            });
                            Ok(())
                        },
                        true,
                    )
                }
            }
            _ => Err(CompileError::Unsupported("assignment target", target.span)),
        }
    }

    /// `object.key op= ...`: evaluate the reference once, optionally read
    /// the old value, compute the new one with `compute(old, out)`, store
    /// it and leave it in `dst`.
    fn member_update(
        &mut self,
        m: &Expression,
        dst: Reg,
        compute: impl FnOnce(&mut Self, Reg, Reg) -> CResult<()>,
        read_old: bool,
    ) -> CResult<()> {
        let mut m = m;
        while let ExpressionKind::Parenthesized(inner) = &m.kind {
            m = inner;
        }
        if let ExpressionKind::SuperMember(property) = &m.kind {
            let (this, base, key) = (self.temp()?, self.temp()?, self.temp()?);
            self.load_this(this)?;
            self.super_key(property, key, m.span)?;
            if read_old && matches!(property, MemberProperty::Computed(_)) {
                self.emit(Op::Unary {
                    op: UnOp::ToPropertyKey,
                    dst: key,
                    src: key,
                });
            }
            self.load_super_base(base, m.span)?;
            let old = self.temp()?;
            if read_old {
                self.emit(Op::GetSuper {
                    dst: old,
                    object: base,
                    key,
                    this,
                });
            }
            let out = self.temp()?;
            self.pending_skip = None;
            compute(self, old, out)?;
            let skip = self.pending_skip.take();
            self.emit(Op::SetSuper {
                object: base,
                key,
                value: out,
                this,
            });
            if let Some(s) = skip {
                self.patch_here(s);
            }
            self.emit(Op::Move { dst, src: out });
            return Ok(());
        }
        let ExpressionKind::Member {
            object, property, ..
        } = &m.kind
        else {
            return Err(CompileError::Unsupported("assignment target", m.span));
        };
        let o = self.temp()?;
        self.expr(object, o)?;
        // A named property goes through cache sites; a computed one keeps
        // its key in a register.
        let name = match property {
            MemberProperty::Name(id) => Some(&*id.name),
            _ => None,
        };
        let k = self.temp()?;
        match property {
            MemberProperty::Name(_) => {}
            MemberProperty::Computed(x) => {
                self.expr(x, k)?;
                if read_old {
                    self.emit(Op::Unary {
                        op: UnOp::ToPropertyKey,
                        dst: k,
                        src: k,
                    });
                }
            }
            MemberProperty::Private(_) => {
                return Err(CompileError::Unsupported("private names", m.span));
            }
        }
        let old = self.temp()?;
        if read_old {
            match name {
                Some(name) => self.get_named(old, o, name)?,
                None => {
                    self.emit(Op::GetProp {
                        dst: old,
                        object: o,
                        key: k,
                    });
                }
            }
        }
        let out = self.temp()?;
        self.pending_skip = None;
        compute(self, old, out)?;
        let skip = self.pending_skip.take();
        match name {
            Some(name) => self.set_named(o, name, out)?,
            None => {
                self.emit(Op::SetProp {
                    object: o,
                    key: k,
                    value: out,
                });
            }
        }
        if let Some(s) = skip {
            self.patch_here(s);
        }
        self.emit(Op::Move { dst, src: out });
        Ok(())
    }

    fn update(&mut self, op: UpdateOp, prefix: bool, target: &Pattern, dst: Reg) -> CResult<()> {
        let un = match op {
            UpdateOp::Increment => UnOp::Inc,
            UpdateOp::Decrement => UnOp::Dec,
        };
        match &target.kind {
            PatternKind::Identifier(id) => {
                let old = self.temp()?;
                self.load_binding(&id.name, old)?;
                self.emit(Op::Unary {
                    op: UnOp::ToNumeric,
                    dst: old,
                    src: old,
                });
                let new = self.temp()?;
                self.emit(Op::Unary {
                    op: un,
                    dst: new,
                    src: old,
                });
                self.store_binding(&id.name, new, false)?;
                self.emit(Op::Move {
                    dst,
                    src: if prefix { new } else { old },
                });
                Ok(())
            }
            PatternKind::Member(m) => {
                let saved = self.temp()?;
                self.member_update(
                    m,
                    dst,
                    |g, old, out| {
                        g.emit(Op::Unary {
                            op: UnOp::ToNumeric,
                            dst: old,
                            src: old,
                        });
                        g.emit(Op::Move {
                            dst: saved,
                            src: old,
                        });
                        g.emit(Op::Unary {
                            op: un,
                            dst: out,
                            src: old,
                        });
                        Ok(())
                    },
                    true,
                )?;
                if !prefix {
                    self.emit(Op::Move { dst, src: saved });
                }
                Ok(())
            }
            _ => Err(CompileError::Unsupported("update target", target.span)),
        }
    }
}

fn pattern_names(p: &Pattern, out: &mut Vec<String>) {
    match &p.kind {
        PatternKind::Identifier(id) => out.push(id.name.to_string()),
        PatternKind::Member(_) => {}
        PatternKind::Object { properties, rest } => {
            for prop in properties {
                pattern_names(&prop.value, out);
            }
            if let Some(r) = rest {
                pattern_names(r, out);
            }
        }
        PatternKind::Array { elements, rest } => {
            for e in elements.iter().flatten() {
                pattern_names(e, out);
            }
            if let Some(r) = rest {
                pattern_names(r, out);
            }
        }
        PatternKind::Assignment { target, .. } => pattern_names(target, out),
    }
}

/// `var` names nested inside statements of a script (not top-level
/// declarations, which the caller already declared).
fn collect_nested_vars(s: &Statement, out: &mut Vec<String>) {
    let var_decl = |d: &VariableDeclaration, out: &mut Vec<String>| {
        if d.kind == VariableKind::Var {
            for decl in &d.declarations {
                pattern_names(&decl.target, out);
            }
        }
    };
    match &s.kind {
        StatementKind::Block(b) => b.iter().for_each(|s| collect_nested_vars(s, out)),
        StatementKind::If {
            consequent,
            alternate,
            ..
        } => {
            collect_nested_vars(consequent, out);
            if let Some(a) = alternate {
                collect_nested_vars(a, out);
            }
        }
        StatementKind::For { init, body, .. } => {
            if let Some(ForInit::Variable(d)) = init {
                var_decl(d, out);
            }
            collect_nested_vars(body, out);
        }
        StatementKind::ForIn { left, body, .. } | StatementKind::ForOf { left, body, .. } => {
            if let ForHead::Variable(d) = left {
                var_decl(d, out);
            }
            collect_nested_vars(body, out);
        }
        StatementKind::While { body, .. }
        | StatementKind::DoWhile { body, .. }
        | StatementKind::Labeled { body, .. }
        | StatementKind::With { body, .. } => collect_nested_vars(body, out),
        StatementKind::Switch { cases, .. } => {
            for c in cases {
                c.body.iter().for_each(|s| collect_nested_vars(s, out));
            }
        }
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            block.iter().for_each(|s| collect_nested_vars(s, out));
            if let Some(h) = handler {
                h.body.iter().for_each(|s| collect_nested_vars(s, out));
            }
            if let Some(f) = finalizer {
                f.iter().for_each(|s| collect_nested_vars(s, out));
            }
        }
        _ => {}
    }
}
