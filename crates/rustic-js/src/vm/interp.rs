//! The bytecode interpreter (JS-04).
//!
//! Frames live in the agent ([`VmState`]): registers on one value stack,
//! call records on another. A call from script to script pushes a frame
//! and keeps running the same Rust loop, so script recursion never grows
//! the Rust stack; only a call through native code (a getter, a
//! conversion method, a built-in that calls back) nests a Rust frame, and
//! that nesting is bounded.
//!
//! Collection happens only at safe points of a run the host started
//! (loop back edges and calls), where every live value is in a register,
//! a frame or an environment. Runs nested inside native code never
//! collect, because that code holds handles in Rust locals.

use std::rc::Rc;

use rustic_base::CancellationToken;

use super::bytecode::{
    BinOp, CacheKind, CodeId, FunctionCode, FunctionKind, Op, Reg, SiteCache, UnOp, declare,
};
use crate::runtime::{
    Abrupt, Agent, BINDING_MUTABLE, BINDING_STRICT, Class, EnvId, ErrorKind, FLAG_EXTENSIBLE,
    FLAG_PROTOTYPE, JsResult, Kind, NativeFn, ObjectId, PropertyDescriptor, PropertyKey, Reference,
    ReferenceBase, ShapeId, StringId, Tracer, Value,
};

/// Bounds on what running script may use beyond the heap.
#[derive(Clone, Copy, Debug)]
pub struct VmLimits {
    /// Most call frames. A deeper call throws a RangeError.
    pub max_frames: usize,
    /// Most values on the register stack (8 bytes each). A call that
    /// would exceed it throws a RangeError.
    pub max_stack_values: usize,
    /// Most script runs nested through native code (getters, conversion
    /// methods, callbacks). These nest Rust frames, so the bound is small.
    pub max_native_depth: u32,
    /// Most bytes of Rust stack nested runs may use beyond the outermost
    /// one. Unoptimized builds use far more stack per level, so this is
    /// what keeps [`Self::max_native_depth`] safe in both.
    pub max_native_stack_bytes: usize,
}

impl Default for VmLimits {
    fn default() -> Self {
        Self {
            max_frames: 10_000,
            max_stack_values: 1 << 20,
            max_native_depth: 96,
            max_native_stack_bytes: 1024 * 1024,
        }
    }
}

/// One activation of a compiled function.
#[derive(Debug)]
struct Frame {
    code: Rc<FunctionCode>,
    code_id: CodeId,
    /// The next op to run.
    pc: u32,
    /// Where the frame's registers start on the value stack.
    base: u32,
    /// Where to cut the value stack when the frame returns (below its
    /// registers, and below its arguments when the host pushed them).
    floor: u32,
    /// The actual arguments, wherever the caller left them.
    args_at: u32,
    argc: u32,
    /// The running lexical environment.
    env: EnvId,
    /// Environments this frame has pushed.
    depth: u16,
    /// The caller's register for the result.
    ret: Reg,
    /// A `new` call: a non-object result is replaced by `this`.
    construct: bool,
    this: Value,
    /// The function object, or undefined for a script.
    function: Value,
    new_target: Value,
}

/// The interpreter's state in an agent.
#[derive(Debug, Default)]
pub(crate) struct VmState {
    stack: Vec<Value>,
    frames: Vec<Frame>,
    /// Native calls and nested runs in progress. Collection needs zero.
    native_depth: u32,
    cancel: Option<CancellationToken>,
    /// Bumped when a global lexical binding is created, which can shadow
    /// a property of the global object that a site has cached.
    lexical_generation: u32,
    limits: VmLimits,
    /// Where the Rust stack was when the outermost run started.
    stack_base: usize,
    /// Safe points left before the run is cancelled, if budgeted.
    steps_left: Option<u64>,
    /// When set, every cache epoch is unique, so no site ever hits.
    caches_off: Option<std::cell::Cell<u32>>,
}

fn stack_address() -> usize {
    let marker = 0u8;
    std::ptr::from_ref(&marker) as usize
}

impl VmState {
    /// Report everything the interpreter holds to the collector.
    pub(crate) fn trace(&self, tracer: &mut impl Tracer) {
        for &v in &self.stack {
            tracer.value(v);
        }
        for frame in &self.frames {
            tracer.code(frame.code_id);
            tracer.env(frame.env);
            tracer.value(frame.this);
            tracer.value(frame.function);
            tracer.value(frame.new_target);
        }
    }
}

/// What a callee is.
enum Callee {
    Closure(ObjectId),
    Native(NativeFn),
    NotCallable,
}

/// How the dispatch loop stopped.
enum Flow {
    /// The frame stack changed: reload the top frame.
    Switch,
    /// The run's first frame returned.
    Done(Value),
}

/// Arguments of a native call are copied out of the value stack (the
/// callee gets the whole agent); this many fit without allocating.
const INLINE_ARGS: usize = 8;

impl Agent {
    // ---- embedder API -------------------------------------------------

    /// Let `token` stop running script: loops and calls poll it, and a
    /// cancelled run ends with [`Abrupt::Cancelled`], which script cannot
    /// catch.
    pub fn set_cancellation(&mut self, token: Option<CancellationToken>) {
        self.vm.cancel = token;
    }

    /// Cancel running script after `steps` more safe points (loop back
    /// edges and calls), or never with `None`. Unlike a token this is
    /// deterministic, which tests and fuzzing need. The budget is shared
    /// by every run until it is set again.
    pub fn set_step_budget(&mut self, steps: Option<u64>) {
        self.vm.steps_left = steps;
    }

    /// Turn the inline caches of property and global sites off or on.
    /// Results must not depend on it: this exists so tests can compare
    /// cached runs against the uncached lookups.
    pub fn set_inline_caches(&mut self, enabled: bool) {
        self.vm.caches_off = if enabled {
            None
        } else {
            Some(std::cell::Cell::new(0))
        };
    }

    /// The cancellation token running script polls, if any.
    pub fn cancellation(&self) -> Option<CancellationToken> {
        self.vm.cancel.clone()
    }

    /// Whether another run nested inside native code still fits; the
    /// outermost run records where the Rust stack starts.
    fn enter_nested_run(&mut self) -> JsResult<()> {
        if self.vm.frames.is_empty() {
            self.vm.stack_base = stack_address();
            return Ok(());
        }
        if self.vm.native_depth >= self.vm.limits.max_native_depth
            || stack_address().abs_diff(self.vm.stack_base) > self.vm.limits.max_native_stack_bytes
        {
            return Err(self.throw(ErrorKind::RangeError, "Maximum call stack size exceeded"));
        }
        Ok(())
    }

    /// Replace the interpreter's limits.
    pub fn set_vm_limits(&mut self, limits: VmLimits) {
        self.vm.limits = limits;
    }

    /// Run a compiled script and return its completion value. The run
    /// collects garbage at its safe points: every handle the caller keeps
    /// across this call must be rooted ([`Agent::root`]).
    pub fn run_script(&mut self, code: CodeId) -> JsResult<Value> {
        self.enter_nested_run()?;
        let floor = self.vm.frames.len();
        let stack_floor = self.vm.stack.len();
        let global = Value::object(self.intrinsics.global);
        let env = self.global_env;
        self.push_frame_for(
            code,
            env,
            global,
            Value::UNDEFINED,
            Value::UNDEFINED,
            stack_floor,
            0,
            0,
            false,
            stack_floor,
        )?;
        self.run(floor)
    }

    /// `Call(f, this, args)` from the host, collecting garbage at safe
    /// points like [`Agent::run_script`]. Native code that is itself
    /// running inside script calls [`Agent::call`] instead.
    pub fn call_from_host(&mut self, f: Value, this: Value, args: &[Value]) -> JsResult<Value> {
        match self.callee(f) {
            Callee::Closure(o) => {
                self.enter_nested_run()?;
                let floor = self.vm.frames.len();
                let args_at = self.vm.stack.len();
                self.vm.stack.extend_from_slice(args);
                if let Err(e) = self.push_closure_frame(
                    o,
                    this,
                    Value::UNDEFINED,
                    args_at,
                    args.len(),
                    0,
                    false,
                    args_at,
                ) {
                    self.vm.stack.truncate(args_at);
                    return Err(e);
                }
                self.run(floor)
            }
            _ => self.call(f, this, args),
        }
    }

    // ---- calls from native code ----------------------------------------

    pub(crate) fn call_native(
        &mut self,
        behavior: NativeFn,
        this: Value,
        args: &[Value],
    ) -> JsResult<Value> {
        self.vm.native_depth += 1;
        let result = behavior(self, this, args);
        self.vm.native_depth -= 1;
        result
    }

    /// Run a closure to completion inside native code. No collection
    /// happens in the nested run.
    pub(crate) fn call_closure(
        &mut self,
        f: ObjectId,
        this: Value,
        args: &[Value],
    ) -> JsResult<Value> {
        self.enter_nested_run()?;
        let floor = self.vm.frames.len();
        let args_at = self.vm.stack.len();
        self.vm.stack.extend_from_slice(args);
        if let Err(e) = self.push_closure_frame(
            f,
            this,
            Value::UNDEFINED,
            args_at,
            args.len(),
            0,
            false,
            args_at,
        ) {
            self.vm.stack.truncate(args_at);
            return Err(e);
        }
        self.vm.native_depth += 1;
        let result = self.run(floor);
        self.vm.native_depth -= 1;
        result
    }

    /// `Construct(f, args)` with `f` as the new target.
    pub fn construct(&mut self, f: Value, args: &[Value]) -> JsResult<Value> {
        let Callee::Closure(o) = self.callee(f) else {
            return self.type_error("value is not a constructor");
        };
        let this = self.construct_this(o, f)?;
        self.enter_nested_run()?;
        let floor = self.vm.frames.len();
        let args_at = self.vm.stack.len();
        self.vm.stack.extend_from_slice(args);
        if let Err(e) = self.push_closure_frame(o, this, f, args_at, args.len(), 0, true, args_at) {
            self.vm.stack.truncate(args_at);
            return Err(e);
        }
        self.vm.native_depth += 1;
        let result = self.run(floor);
        self.vm.native_depth -= 1;
        result
    }

    // ---- functions ------------------------------------------------------

    fn callee(&self, f: Value) -> Callee {
        let Some(o) = f.as_object() else {
            return Callee::NotCallable;
        };
        let object = self.heap.object(o);
        if object.closure_index().is_some() {
            Callee::Closure(o)
        } else if let Some(native) = object.native() {
            Callee::Native(native)
        } else {
            Callee::NotCallable
        }
    }

    /// A function object for `code` closing over `env`.
    pub(crate) fn new_closure(
        &mut self,
        code_id: CodeId,
        env: EnvId,
        this: Value,
    ) -> JsResult<ObjectId> {
        use crate::runtime::{Closure, FLAG_CLOSURE, Object};
        let code = Rc::clone(self.heap.code(code_id));
        let this = if code.kind == FunctionKind::Arrow {
            this
        } else {
            Value::UNINITIALIZED
        };
        self.heap.charge(std::mem::size_of::<Closure>())?;
        let closure = self.heap.closures.alloc(Closure {
            code: code_id,
            env,
            this,
            home: Value::UNDEFINED,
        })?;
        let mut object = Object::new(Class::Function, Some(self.intrinsics.function_prototype));
        object.set_flag(FLAG_CLOSURE, true);
        object.length = closure;
        let f = self.heap.alloc_object(object)?;
        let length_key = PropertyKey::String(self.names.length);
        self.define_property_or_throw(
            f,
            length_key,
            PropertyDescriptor::data_with(
                Value::number(f64::from(code.length)),
                false,
                false,
                true,
            ),
        )?;
        let name_key = PropertyKey::String(self.names.name);
        self.define_property_or_throw(
            f,
            name_key,
            PropertyDescriptor::data_with(Value::string(code.name), false, false, true),
        )?;
        if code.kind == FunctionKind::Normal {
            // The object itself is created on first use (see
            // `realize_function_prototype`); most functions never need it.
            let prototype_key = PropertyKey::String(self.names.prototype);
            self.define_property_or_throw(
                f,
                prototype_key,
                PropertyDescriptor::data_with(Value::UNINITIALIZED, true, false, false),
            )?;
        }
        Ok(f)
    }

    /// The object and private name of a private field access.
    fn private_operands(
        &mut self,
        object: Value,
        key: Value,
    ) -> JsResult<(ObjectId, crate::runtime::SymbolId)> {
        let Kind::Symbol(name) = key.kind() else {
            unreachable!("the compiler loads a private name")
        };
        match object.as_object() {
            Some(o) => Ok((o, name)),
            None => self.type_error("cannot use a private member on a non-object"),
        }
    }

    /// Whether `o` carries `brand`: a private name it must own, or the
    /// class it must be (static private methods).
    fn has_brand(&self, o: ObjectId, brand: Value) -> bool {
        match brand.kind() {
            Kind::Symbol(name) => self.private_has(o, name),
            _ => brand.as_object() == Some(o),
        }
    }

    /// Whether `new` accepts `o`.
    fn is_constructor(&self, o: ObjectId) -> bool {
        self.heap
            .closure(o)
            .is_some_and(|closure| self.heap.code(closure.code).kind.is_constructor())
    }

    /// The parent constructor a `super()` call constructs.
    fn super_constructor(&mut self, f: Value) -> JsResult<ObjectId> {
        match f.as_object().filter(|o| self.is_constructor(*o)) {
            Some(o) => Ok(o),
            None => self.type_error("Super constructor is not a constructor"),
        }
    }

    /// `ClassDefinitionEvaluation` steps 5 to 14: create the prototype
    /// object and link constructor and prototype to the heritage (absent
    /// for a class without `extends`). Returns the prototype object.
    fn class_setup(&mut self, f: ObjectId, heritage: Option<Value>) -> JsResult<ObjectId> {
        let prototype_key = PropertyKey::String(self.names.prototype);
        let (proto_parent, constructor_parent) = match heritage {
            None => (
                Some(self.intrinsics.object_prototype),
                self.intrinsics.function_prototype,
            ),
            Some(Value::NULL) => (None, self.intrinsics.function_prototype),
            Some(value) => {
                let Some(parent) = value.as_object().filter(|o| self.is_constructor(*o)) else {
                    return self.type_error("Class extends value is not a constructor or null");
                };
                let parent_prototype = self.get(parent, prototype_key)?;
                let proto_parent = match parent_prototype.as_object() {
                    Some(o) => Some(o),
                    None if parent_prototype == Value::NULL => None,
                    None => {
                        return self.type_error(
                            "Class extends value does not have a valid prototype property",
                        );
                    }
                };
                (proto_parent, parent)
            }
        };
        let proto = self.new_object_with(proto_parent)?;
        self.set_prototype_of(f, Some(constructor_parent));
        self.define_property_or_throw(
            f,
            prototype_key,
            PropertyDescriptor::data_with(Value::object(proto), false, false, false),
        )?;
        let constructor_key = PropertyKey::String(self.names.constructor);
        self.define_property_or_throw(
            proto,
            constructor_key,
            PropertyDescriptor::data_with(Value::object(f), true, false, true),
        )?;
        Ok(proto)
    }

    /// `OrdinaryCreateFromConstructor(newTarget, "%Object.prototype%")`.
    /// A derived class constructor gets no object: `super()` binds its
    /// `this`.
    fn construct_this(&mut self, f: ObjectId, new_target: Value) -> JsResult<Value> {
        let closure = self.heap.closure(f).expect("caller checked for a closure");
        let kind = self.heap.code(closure.code).kind;
        if !kind.is_constructor() {
            return self.type_error("value is not a constructor");
        }
        if kind == FunctionKind::DerivedConstructor {
            return Ok(Value::UNINITIALIZED);
        }
        let source = new_target.as_object().unwrap_or(f);
        let prototype = self.get(source, PropertyKey::String(self.names.prototype))?;
        let prototype = prototype
            .as_object()
            .unwrap_or(self.intrinsics.object_prototype);
        Ok(Value::object(self.new_object_with(Some(prototype))?))
    }

    #[allow(clippy::too_many_arguments, reason = "one frame's whole state")]
    fn push_closure_frame(
        &mut self,
        f: ObjectId,
        this: Value,
        new_target: Value,
        args_at: usize,
        argc: usize,
        ret: Reg,
        construct: bool,
        floor: usize,
    ) -> JsResult<()> {
        let closure = self.heap.closure(f).expect("caller checked for a closure");
        let code = self.heap.code(closure.code);
        if !construct && code.kind.is_class_constructor() {
            return self.type_error("Class constructor cannot be invoked without 'new'");
        }
        let this = if code.kind == FunctionKind::Arrow {
            closure.this
        } else if code.strict || this.as_object().is_some() {
            this
        } else if this.is_nullish() {
            Value::object(self.intrinsics.global)
        } else {
            Value::object(self.to_object(this)?)
        };
        self.push_frame_for(
            closure.code,
            closure.env,
            this,
            Value::object(f),
            new_target,
            args_at,
            argc,
            ret,
            construct,
            floor,
        )
    }

    #[allow(clippy::too_many_arguments, reason = "one frame's whole state")]
    fn push_frame_for(
        &mut self,
        code_id: CodeId,
        env: EnvId,
        this: Value,
        function: Value,
        new_target: Value,
        args_at: usize,
        argc: usize,
        ret: Reg,
        construct: bool,
        floor: usize,
    ) -> JsResult<()> {
        let code = Rc::clone(self.heap.code(code_id));
        let base = self.vm.stack.len();
        let registers = usize::from(code.registers);
        if self.vm.frames.len() >= self.vm.limits.max_frames
            || base + registers > self.vm.limits.max_stack_values
        {
            return Err(self.throw(ErrorKind::RangeError, "Maximum call stack size exceeded"));
        }
        self.vm.stack.resize(base + registers, Value::UNDEFINED);
        let copied = argc.min(usize::from(code.params));
        self.vm.stack.copy_within(args_at..args_at + copied, base);
        // The stack limit keeps every index within `u32`.
        self.vm.frames.push(Frame {
            code,
            code_id,
            pc: 0,
            base: base as u32,
            floor: floor as u32,
            args_at: args_at as u32,
            argc: argc as u32,
            env,
            depth: 0,
            ret,
            construct,
            this,
            function,
            new_target,
        });
        Ok(())
    }

    // ---- running --------------------------------------------------------

    /// Run until the frame stack is back to `floor` frames.
    fn run(&mut self, floor: usize) -> JsResult<Value> {
        loop {
            let frame = self.vm.frames.last().expect("a frame to run");
            let code = Rc::clone(&frame.code);
            let base = frame.base as usize;
            let mut pc = frame.pc as usize;
            match self.dispatch(&code, base, &mut pc, floor) {
                Ok(Flow::Switch) => {}
                Ok(Flow::Done(value)) => return Ok(value),
                Err(abrupt) => {
                    // Frames above the faulting one (a callee whose frame
                    // could not finish setting up) do not exist: the
                    // faulting frame is the top one.
                    self.vm.frames.last_mut().expect("the faulting frame").pc = pc as u32;
                    self.unwind(floor, abrupt)?;
                }
            }
        }
    }

    /// Find a handler for `abrupt` in the frames above `floor`, dropping
    /// the frames that have none.
    fn unwind(&mut self, floor: usize, abrupt: Abrupt) -> JsResult<()> {
        let Abrupt::Throw(value) = abrupt else {
            // Out of memory and cancellation are not catchable.
            self.pop_frames_to(floor);
            return Err(abrupt);
        };
        while self.vm.frames.len() > floor {
            let frame = self.vm.frames.last().expect("checked above");
            // `pc` is past the faulting op.
            let at = frame.pc.saturating_sub(1);
            let handler = frame
                .code
                .handlers
                .iter()
                .find(|h| h.start <= at && at < h.end)
                .copied();
            match handler {
                Some(h) => {
                    let mut env = frame.env;
                    let mut depth = frame.depth;
                    while depth > h.scope_depth {
                        env = self.env_outer_n(env, 1);
                        depth -= 1;
                    }
                    let frame = self.vm.frames.last_mut().expect("checked above");
                    frame.env = env;
                    frame.depth = depth;
                    frame.pc = h.target;
                    let slot = frame.base as usize + usize::from(h.exception);
                    self.vm.stack[slot] = value;
                    return Ok(());
                }
                None => {
                    let frame = self.vm.frames.pop().expect("checked above");
                    self.vm.stack.truncate(frame.floor as usize);
                }
            }
        }
        Err(Abrupt::Throw(value))
    }

    fn pop_frames_to(&mut self, floor: usize) {
        while self.vm.frames.len() > floor {
            let frame = self.vm.frames.pop().expect("checked above");
            self.vm.stack.truncate(frame.floor as usize);
        }
    }

    /// Poll for cancellation, and collect if the heap asks for it and no
    /// native code is on the stack.
    fn safepoint(&mut self) -> JsResult<()> {
        if self
            .vm
            .cancel
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(Abrupt::Cancelled);
        }
        if let Some(left) = &mut self.vm.steps_left {
            if *left == 0 {
                return Err(Abrupt::Cancelled);
            }
            *left -= 1;
        }
        if self.vm.native_depth == 0 && self.should_collect() {
            self.collect_garbage(crate::runtime::Roots::default());
        }
        Ok(())
    }

    /// The key that makes a cache entry trustworthy: collections reuse
    /// shape and object ids, a change to a prototype object changes what
    /// its heirs inherit, and a new global lexical binding can shadow a
    /// cached property of the global object.
    fn cache_epoch(&self) -> u32 {
        if let Some(counter) = &self.vm.caches_off {
            counter.set(counter.get().wrapping_add(1));
            return counter.get();
        }
        // Wrapping is fine: a stale entry would need 2³² events between
        // two uses of one site to be mistaken for a fresh one.
        (self.heap.collections() as u32)
            .wrapping_add(self.vm.lexical_generation)
            .wrapping_add(self.heap.proto_epoch)
            .wrapping_add(1)
    }

    fn name_of(&self, code: &FunctionCode, index: u32) -> StringId {
        code.constants[index as usize]
            .as_string()
            .expect("name operands are strings")
    }

    #[allow(clippy::too_many_lines, reason = "the dispatch loop is one match")]
    fn dispatch(
        &mut self,
        code: &FunctionCode,
        base: usize,
        pc_out: &mut usize,
        floor: usize,
    ) -> JsResult<Flow> {
        let mut pc = *pc_out;
        let mut env = self.vm.frames.last().expect("a frame to run").env;

        macro_rules! reg {
            ($r:expr) => {
                self.vm.stack[base + usize::from($r)]
            };
        }
        // Evaluate a fallible operation, recording where it failed.
        macro_rules! t {
            ($e:expr) => {
                match $e {
                    Ok(v) => v,
                    Err(e) => {
                        *pc_out = pc;
                        return Err(Abrupt::from(e));
                    }
                }
            };
        }
        macro_rules! jump {
            ($target:expr) => {
                pc = $target as usize
            };
        }
        macro_rules! frame {
            () => {
                self.vm.frames.last_mut().expect("a frame to run")
            };
        }

        loop {
            let op = code.ops[pc];
            pc += 1;
            match op {
                Op::Const { dst, index } => reg!(dst) = code.constants[index as usize],
                Op::Undefined { dst } => reg!(dst) = Value::UNDEFINED,
                Op::Uninit { dst } => reg!(dst) = Value::UNINITIALIZED,
                Op::Int { dst, value } => reg!(dst) = Value::number(f64::from(value)),
                Op::Move { dst, src } => reg!(dst) = reg!(src),
                Op::This { dst } => reg!(dst) = frame!().this,
                Op::NewTarget { dst } => reg!(dst) = frame!().new_target,
                Op::Callee { dst } => reg!(dst) = frame!().function,
                Op::CheckTdz { src, name } => {
                    if reg!(src) == Value::UNINITIALIZED {
                        let name = self.name_of(code, name);
                        t!(Err(self.tdz_error(name)));
                    }
                }
                Op::ConstAssign { name } => {
                    let name = self.name_of(code, name);
                    let text = format!(
                        "assignment to constant variable {}",
                        self.heap.string(name).to_text()
                    );
                    t!(Err(self.throw(ErrorKind::TypeError, &text)));
                }

                Op::GetSlot { dst, hops, slot } => {
                    let scope = self.env_outer_n(env, hops);
                    let value = self.env_slot(scope, slot);
                    if value == Value::UNINITIALIZED {
                        let (name, _) = self.env_slot_binding(scope, slot);
                        t!(Err(self.tdz_error(name)));
                    }
                    reg!(dst) = value;
                }
                Op::SetSlot { src, hops, slot } => {
                    let scope = self.env_outer_n(env, hops);
                    let (name, flags) = self.env_slot_binding(scope, slot);
                    if self.env_slot(scope, slot) == Value::UNINITIALIZED {
                        t!(Err(self.tdz_error(name)));
                    }
                    if flags & BINDING_MUTABLE != 0 {
                        let value = reg!(src);
                        self.set_env_slot(scope, slot, value);
                    } else if code.strict || flags & BINDING_STRICT != 0 {
                        let text = format!(
                            "assignment to constant variable {}",
                            self.heap.string(name).to_text()
                        );
                        t!(Err(self.throw(ErrorKind::TypeError, &text)));
                    }
                }
                Op::InitSlot { src, hops, slot } => {
                    let scope = self.env_outer_n(env, hops);
                    let value = reg!(src);
                    self.set_env_slot(scope, slot, value);
                }
                Op::PushScope { scope } => {
                    let names = &code.scopes[scope as usize];
                    env = t!(self.new_scope_environment(Some(env), names, Value::UNINITIALIZED));
                    let frame = frame!();
                    frame.env = env;
                    frame.depth += 1;
                }
                Op::PopScope => {
                    env = self.env_outer_n(env, 1);
                    let frame = frame!();
                    frame.env = env;
                    frame.depth -= 1;
                }
                Op::CopyScope => {
                    env = t!(self.copy_scope_environment(env));
                    frame!().env = env;
                }

                Op::GetName { dst, name } => {
                    let name = self.name_of(code, name);
                    let reference = t!(self.resolve_binding(env, name, code.strict));
                    reg!(dst) = t!(self.get_value(&reference));
                }
                Op::TypeofName { dst, name } => {
                    let name = self.name_of(code, name);
                    let reference = t!(self.resolve_binding(env, name, code.strict));
                    let value = if reference.base == ReferenceBase::Unresolvable {
                        Value::UNDEFINED
                    } else {
                        t!(self.get_value(&reference))
                    };
                    reg!(dst) = t!(self.type_of_value(value));
                }
                Op::SetName { src, name } => {
                    let name = self.name_of(code, name);
                    let reference = t!(self.resolve_binding(env, name, code.strict));
                    let value = reg!(src);
                    t!(self.put_value(&reference, value));
                }
                Op::GetGlobal { dst, site } => {
                    reg!(dst) = t!(self.get_global(code, site));
                }
                Op::SetGlobal { src, site } => {
                    let value = reg!(src);
                    t!(self.set_global(code, site, value));
                }
                Op::DeclareGlobal { name, kind } => {
                    let name = self.name_of(code, name);
                    t!(self.declare_global(name, kind));
                }
                Op::InitName { src, name } => {
                    let name = self.name_of(code, name);
                    let value = reg!(src);
                    let global = self.global_env;
                    t!(self.initialize_binding(global, name, value));
                }
                Op::DeleteName { dst, name } => {
                    let name = self.name_of(code, name);
                    let reference = t!(self.resolve_binding(env, name, code.strict));
                    let deleted = match reference.base {
                        ReferenceBase::Environment(e) => t!(self.delete_binding(e, name)),
                        _ => true,
                    };
                    reg!(dst) = Value::boolean(deleted);
                }
                Op::PushWith { object } => {
                    let o = t!(self.to_object(reg!(object)));
                    env = t!(self.new_object_environment(o, true, Some(env)));
                    let frame = frame!();
                    frame.env = env;
                    frame.depth += 1;
                }
                Op::WithBase { dst, name } => {
                    let name = self.name_of(code, name);
                    let reference = t!(self.resolve_binding(env, name, code.strict));
                    reg!(dst) = match reference.base {
                        ReferenceBase::Environment(e) => self.with_base_object(e),
                        _ => Value::UNDEFINED,
                    };
                }

                Op::GetProp { dst, object, key } => {
                    let (o, k) = (reg!(object), reg!(key));
                    reg!(dst) = match self.element_fast(o, k) {
                        Some(v) => v,
                        None => t!(self.get_property(o, k)),
                    };
                }
                Op::GetNamed { dst, object, site } => {
                    let o = reg!(object);
                    reg!(dst) = t!(self.get_named(code, site, o));
                }
                Op::SetProp { object, key, value } => {
                    let (o, k, v) = (reg!(object), reg!(key), reg!(value));
                    if !self.set_element_fast(o, k, v) {
                        t!(self.set_property(o, k, v, code.strict));
                    }
                }
                Op::SetNamed {
                    object,
                    site,
                    value,
                } => {
                    let (o, v) = (reg!(object), reg!(value));
                    t!(self.set_named(code, site, o, v));
                }
                Op::DeleteProp { dst, object, key } => {
                    let (o, k) = (reg!(object), reg!(key));
                    let o = t!(self.to_object(o));
                    let key = t!(self.to_property_key(k));
                    let deleted = t!(self.delete(o, key));
                    if !deleted && code.strict {
                        t!(self.type_error::<()>("cannot delete property"));
                    }
                    reg!(dst) = Value::boolean(deleted);
                }
                Op::DefineProp { object, key, value } => {
                    let (o, k, v) = (reg!(object), reg!(key), reg!(value));
                    let o = o.as_object().expect("literals are objects");
                    let key = t!(self.to_property_key(k));
                    t!(self.create_data_property_or_throw(o, key, v));
                }
                Op::DefineAccessor {
                    object,
                    key,
                    function,
                    setter,
                    enumerable,
                } => {
                    let (o, k, f) = (reg!(object), reg!(key), reg!(function));
                    let o = o.as_object().expect("literals are objects");
                    let key = t!(self.to_property_key(k));
                    let desc = PropertyDescriptor {
                        get: (!setter).then_some(f),
                        set: setter.then_some(f),
                        enumerable: Some(enumerable),
                        configurable: Some(true),
                        ..PropertyDescriptor::default()
                    };
                    t!(self.define_property_or_throw(o, key, desc));
                }
                Op::DefineMethod { object, key, value } => {
                    let (o, k, v) = (reg!(object), reg!(key), reg!(value));
                    let o = o.as_object().expect("class objects are objects");
                    let key = t!(self.to_property_key(k));
                    let desc = PropertyDescriptor::data_with(v, true, false, true);
                    t!(self.define_property_or_throw(o, key, desc));
                }
                Op::ClassSetup {
                    ctor,
                    heritage,
                    proto,
                    derived,
                } => {
                    let f = reg!(ctor).as_object().expect("a class constructor");
                    let heritage = reg!(heritage);
                    reg!(proto) =
                        Value::object(t!(self.class_setup(f, derived.then_some(heritage))));
                }
                Op::GetPrototypeOf { dst, src } => {
                    let Some(o) = reg!(src).as_object() else {
                        t!(self.type_error::<()>("value has no prototype"));
                        unreachable!("the error returned");
                    };
                    reg!(dst) = self.get_prototype_of(o).map_or(Value::NULL, Value::object);
                }
                Op::GetSuper {
                    dst,
                    object,
                    key,
                    this,
                } => {
                    let (home, k, this) = (reg!(object), reg!(key), reg!(this));
                    let Some(home) = home.as_object() else {
                        t!(self.type_error::<()>("super has no properties: the prototype is null"));
                        unreachable!("the error returned");
                    };
                    let key = t!(self.to_property_key(k));
                    reg!(dst) = t!(self.get_with_receiver(home, key, this));
                }
                Op::SetSuper {
                    object,
                    key,
                    value,
                    this,
                } => {
                    let (home, k, v, this) = (reg!(object), reg!(key), reg!(value), reg!(this));
                    let Some(home) = home.as_object() else {
                        t!(self.type_error::<()>("super has no properties: the prototype is null"));
                        unreachable!("the error returned");
                    };
                    let key = t!(self.to_property_key(k));
                    if !t!(self.set_with_receiver(home, key, v, this)) {
                        t!(self.type_error::<()>(
                            "cannot assign to a read-only property through super"
                        ));
                    }
                }
                Op::SuperCall {
                    dst,
                    callee,
                    new_target,
                    args,
                    argc,
                } => {
                    t!(self.safepoint());
                    let (f, nt) = (reg!(callee), reg!(new_target));
                    let o = t!(self.super_constructor(f));
                    let this = t!(self.construct_this(o, nt));
                    let top = self.vm.stack.len();
                    t!(self.push_closure_frame(
                        o,
                        this,
                        nt,
                        base + usize::from(args),
                        usize::from(argc),
                        dst,
                        true,
                        top
                    ));
                    let caller = self.vm.frames.len() - 2;
                    self.vm.frames[caller].pc = pc as u32;
                    return Ok(Flow::Switch);
                }
                Op::SuperCallSpread {
                    dst,
                    callee,
                    new_target,
                    array,
                } => {
                    t!(self.safepoint());
                    let (f, nt, array) = (reg!(callee), reg!(new_target), reg!(array));
                    let o = t!(self.super_constructor(f));
                    let this = t!(self.construct_this(o, nt));
                    let args_at = self.vm.stack.len();
                    let argc = self.push_array_elements(array);
                    if let Err(e) =
                        self.push_closure_frame(o, this, nt, args_at, argc, dst, true, args_at)
                    {
                        self.vm.stack.truncate(args_at);
                        t!(Err(e));
                    }
                    let caller = self.vm.frames.len() - 2;
                    self.vm.frames[caller].pc = pc as u32;
                    return Ok(Flow::Switch);
                }
                Op::NewPrivateName { dst, name } => {
                    let description = self.name_of(code, name);
                    let symbol = t!(self.heap.alloc_private_name(description));
                    reg!(dst) = Value::symbol(symbol);
                }
                Op::PrivateGet { dst, object, key } => {
                    let (o, k) = (reg!(object), reg!(key));
                    let (o, name) = t!(self.private_operands(o, k));
                    reg!(dst) = t!(self.private_get(o, name));
                }
                Op::PrivateSet { object, key, value } => {
                    let (o, k, v) = (reg!(object), reg!(key), reg!(value));
                    let (o, name) = t!(self.private_operands(o, k));
                    t!(self.private_set(o, name, v));
                }
                Op::PrivateAdd { object, key, value } => {
                    let (o, k, v) = (reg!(object), reg!(key), reg!(value));
                    let (o, name) = t!(self.private_operands(o, k));
                    t!(self.private_add(o, name, v));
                }
                Op::PrivateHas { dst, object, key } => {
                    let (o, k) = (reg!(object), reg!(key));
                    let Some(o) = o.as_object() else {
                        t!(self.type_error::<()>(
                            "cannot use 'in' for a private name on a non-object"
                        ));
                        unreachable!("the error returned");
                    };
                    reg!(dst) = Value::boolean(self.has_brand(o, k));
                }
                Op::PrivateBrand { object, brand } => {
                    let (o, b) = (reg!(object), reg!(brand));
                    if !o.as_object().is_some_and(|o| self.has_brand(o, b)) {
                        t!(self.type_error::<()>(
                            "cannot use a private method on an object whose class did not declare it"
                        ));
                    }
                }
                Op::TypeError { message } => {
                    let message = self.name_of(code, message);
                    let text = self.heap.string(message).to_text();
                    t!(self.type_error::<()>(&text));
                }
                Op::GetSlotRaw { dst, hops, slot } => {
                    let scope = self.env_outer_n(env, hops);
                    reg!(dst) = self.env_slot(scope, slot);
                }
                Op::ThrowIfInit { src } => {
                    if reg!(src) != Value::UNINITIALIZED {
                        t!(Err(self.throw(
                            ErrorKind::ReferenceError,
                            "Super constructor may only be called once"
                        )));
                    }
                }
                Op::DerivedResult { src } => {
                    let v = reg!(src);
                    if v != Value::UNDEFINED && v.as_object().is_none() {
                        t!(self.type_error::<()>(
                            "Derived constructors may only return an object or undefined"
                        ));
                    }
                }
                Op::NewObject { dst } => {
                    reg!(dst) = Value::object(t!(self.new_object()));
                }
                Op::NewArray { dst } => {
                    reg!(dst) = Value::object(t!(self.new_array()));
                }
                Op::ArrayPush { array, value } => {
                    let (a, v) = (reg!(array), reg!(value));
                    t!(self.array_push(a, v));
                }
                Op::ArrayHole { array } => {
                    let a = reg!(array).as_object().expect("literals are arrays");
                    let length = self.heap.object(a).length;
                    let key = PropertyKey::String(self.names.length);
                    t!(self.set(a, key, Value::number(f64::from(length) + 1.0), true));
                }
                Op::CopyProps {
                    target,
                    source,
                    excluded,
                    count,
                } => {
                    let (target, source) = (reg!(target), reg!(source));
                    let mut skip = Vec::with_capacity(usize::from(count));
                    for i in 0..count {
                        let k = reg!(excluded + i);
                        skip.push(t!(self.to_property_key(k)));
                    }
                    t!(self.copy_data_properties(target, source, &skip));
                }
                Op::Closure { dst, index } => {
                    let this = frame!().this;
                    let f = t!(self.new_closure(code.functions[index as usize], env, this));
                    reg!(dst) = Value::object(f);
                }
                Op::SetHome { function, home } => {
                    let (f, home) = (reg!(function), reg!(home));
                    if let Some(closure) = f.as_object().and_then(|f| self.heap.closure_mut(f)) {
                        closure.home = home;
                    }
                }
                Op::Arguments { dst } => {
                    let frame = frame!();
                    let (at, n) = (frame.args_at as usize, frame.argc as usize);
                    let args = self.vm.stack[at..at + n].to_vec();
                    reg!(dst) = t!(self.new_arguments_object(&args));
                }
                Op::Rest { dst, from } => {
                    let frame = frame!();
                    let (at, n) = (frame.args_at as usize, frame.argc as usize);
                    let from = usize::from(from).min(n);
                    let args = self.vm.stack[at + from..at + n].to_vec();
                    reg!(dst) = Value::object(t!(self.create_array_from_list(&args)));
                }

                Op::Unary { op, dst, src } => {
                    let v = reg!(src);
                    reg!(dst) = match (op, v.as_number()) {
                        (UnOp::Neg, Some(n)) => Value::number(-n),
                        (UnOp::Inc, Some(n)) => Value::number(n + 1.0),
                        (UnOp::Dec, Some(n)) => Value::number(n - 1.0),
                        (UnOp::Plus | UnOp::ToNumeric, Some(_)) => v,
                        (UnOp::Not, _) => Value::boolean(!self.to_boolean(v)),
                        _ => t!(self.unary(op, v)),
                    };
                }
                Op::Binary { op, dst, a, b } => {
                    let (x, y) = (reg!(a), reg!(b));
                    reg!(dst) = match (x.as_number(), y.as_number()) {
                        (Some(x), Some(y)) if !matches!(op, BinOp::In | BinOp::Instanceof) => {
                            number_binary(op, x, y)
                        }
                        _ => t!(self.binary(op, x, y)),
                    };
                }

                Op::BinaryInt { op, dst, a, value } => {
                    let x = reg!(a);
                    let y = f64::from(value);
                    reg!(dst) = match x.as_number() {
                        Some(x) => number_binary(op, x, y),
                        None => t!(self.binary(op, x, Value::number(y))),
                    };
                }
                Op::Jump { target } => jump!(target),
                Op::JumpIfTrue { cond, target } => {
                    if self.truthy(reg!(cond)) {
                        jump!(target);
                    }
                }
                Op::JumpIfFalse { cond, target } => {
                    if !self.truthy(reg!(cond)) {
                        jump!(target);
                    }
                }
                Op::JumpUnless { op, a, b, target } => {
                    let (x, y) = (reg!(a), reg!(b));
                    let holds = match (x.as_number(), y.as_number()) {
                        (Some(x), Some(y)) => number_compare(op, x, y),
                        _ => t!(self.binary(op, x, y)) == Value::TRUE,
                    };
                    if !holds {
                        jump!(target);
                    }
                }
                Op::JumpUnlessInt {
                    op,
                    a,
                    value,
                    target,
                } => {
                    let x = reg!(a);
                    let y = f64::from(value);
                    let holds = match x.as_number() {
                        Some(x) => number_compare(op, x, y),
                        None => t!(self.binary(op, x, Value::number(y))) == Value::TRUE,
                    };
                    if !holds {
                        jump!(target);
                    }
                }
                Op::JumpIfNullish { src, target } => {
                    if reg!(src).is_nullish() {
                        jump!(target);
                    }
                }
                Op::JumpIfNotUndefined { src, target } => {
                    if reg!(src) != Value::UNDEFINED {
                        jump!(target);
                    }
                }
                Op::JumpIfStrictEq { a, b, target } => {
                    if self.is_strictly_equal(reg!(a), reg!(b)) {
                        jump!(target);
                    }
                }
                Op::JumpIfInt { a, value, target } => {
                    if reg!(a).as_number() == Some(f64::from(value)) {
                        jump!(target);
                    }
                }

                Op::Call {
                    dst,
                    callee,
                    this,
                    args,
                    argc,
                } => {
                    t!(self.safepoint());
                    let (f, this) = (reg!(callee), reg!(this));
                    let args_at = base + usize::from(args);
                    let argc = usize::from(argc);
                    match self.callee(f) {
                        Callee::Closure(o) => {
                            let top = self.vm.stack.len();
                            t!(self.push_closure_frame(
                                o,
                                this,
                                Value::UNDEFINED,
                                args_at,
                                argc,
                                dst,
                                false,
                                top
                            ));
                            let caller = self.vm.frames.len() - 2;
                            self.vm.frames[caller].pc = pc as u32;
                            return Ok(Flow::Switch);
                        }
                        Callee::Native(behavior) => {
                            reg!(dst) = t!(self.call_native_at(behavior, this, args_at, argc));
                        }
                        Callee::NotCallable => {
                            t!(self.type_error::<()>("value is not a function"));
                        }
                    }
                }
                Op::New {
                    dst,
                    callee,
                    args,
                    argc,
                } => {
                    t!(self.safepoint());
                    let f = reg!(callee);
                    let Callee::Closure(o) = self.callee(f) else {
                        t!(self.type_error::<()>("value is not a constructor"));
                        unreachable!("the error returned");
                    };
                    let this = t!(self.construct_this(o, f));
                    let top = self.vm.stack.len();
                    t!(self.push_closure_frame(
                        o,
                        this,
                        f,
                        base + usize::from(args),
                        usize::from(argc),
                        dst,
                        true,
                        top
                    ));
                    let caller = self.vm.frames.len() - 2;
                    self.vm.frames[caller].pc = pc as u32;
                    return Ok(Flow::Switch);
                }
                Op::CallSpread {
                    dst,
                    callee,
                    this,
                    array,
                } => {
                    t!(self.safepoint());
                    let (f, this, array) = (reg!(callee), reg!(this), reg!(array));
                    let args_at = self.vm.stack.len();
                    let argc = self.push_array_elements(array);
                    match self.callee(f) {
                        Callee::Closure(o) => {
                            if let Err(e) = self.push_closure_frame(
                                o,
                                this,
                                Value::UNDEFINED,
                                args_at,
                                argc,
                                dst,
                                false,
                                args_at,
                            ) {
                                self.vm.stack.truncate(args_at);
                                t!(Err(e));
                            }
                            let caller = self.vm.frames.len() - 2;
                            self.vm.frames[caller].pc = pc as u32;
                            return Ok(Flow::Switch);
                        }
                        Callee::Native(behavior) => {
                            let result = self.call_native_at(behavior, this, args_at, argc);
                            self.vm.stack.truncate(args_at);
                            reg!(dst) = t!(result);
                        }
                        Callee::NotCallable => {
                            self.vm.stack.truncate(args_at);
                            t!(self.type_error::<()>("value is not a function"));
                        }
                    }
                }
                Op::NewSpread { dst, callee, array } => {
                    t!(self.safepoint());
                    let (f, array) = (reg!(callee), reg!(array));
                    let Callee::Closure(o) = self.callee(f) else {
                        t!(self.type_error::<()>("value is not a constructor"));
                        unreachable!("the error returned");
                    };
                    let this = t!(self.construct_this(o, f));
                    let args_at = self.vm.stack.len();
                    let argc = self.push_array_elements(array);
                    if let Err(e) =
                        self.push_closure_frame(o, this, f, args_at, argc, dst, true, args_at)
                    {
                        self.vm.stack.truncate(args_at);
                        t!(Err(e));
                    }
                    let caller = self.vm.frames.len() - 2;
                    self.vm.frames[caller].pc = pc as u32;
                    return Ok(Flow::Switch);
                }
                Op::Return { src } => {
                    let mut value = reg!(src);
                    let frame = self.vm.frames.pop().expect("a frame to return from");
                    self.vm.stack.truncate(frame.floor as usize);
                    if frame.construct && value.as_object().is_none() {
                        value = frame.this;
                    }
                    if self.vm.frames.len() == floor {
                        return Ok(Flow::Done(value));
                    }
                    let caller = self.vm.frames.last().expect("a caller above the floor");
                    let slot = caller.base as usize + usize::from(frame.ret);
                    self.vm.stack[slot] = value;
                    return Ok(Flow::Switch);
                }
                Op::Throw { src } => {
                    t!(Err(Abrupt::Throw(reg!(src))));
                }

                Op::ForInStart { state, src } => {
                    let source = reg!(src);
                    let (object, keys) = t!(self.for_in_keys(source));
                    reg!(state) = object;
                    reg!(state + 1) = keys;
                    reg!(state + 2) = Value::number(0.0);
                }
                Op::ForInNext { dst, state, done } => {
                    let (object, keys, at) = (reg!(state), reg!(state + 1), reg!(state + 2));
                    let (key, next) = t!(self.for_in_next(object, keys, at));
                    reg!(state + 2) = next;
                    match key {
                        Some(key) => reg!(dst) = key,
                        None => jump!(done),
                    }
                }
                Op::GetIterator {
                    iterator,
                    next,
                    src,
                } => {
                    let source = reg!(src);
                    let (it, step) = t!(self.get_iterator(source));
                    reg!(iterator) = it;
                    reg!(next) = step;
                }
                Op::IteratorStep {
                    dst,
                    iterator,
                    next,
                    done,
                } => {
                    let (it, step) = (reg!(iterator), reg!(next));
                    if let Some(index) = step.as_number() {
                        // An array walked in place.
                        let array = it.as_object().expect("fast iteration is over an array");
                        if index < f64::from(self.heap.object(array).length) {
                            reg!(next) = Value::number(index + 1.0);
                            // Positions are below the array's `u32` length.
                            let key = PropertyKey::Index(index as u32);
                            reg!(dst) = match self.element_fast(it, Value::number(index)) {
                                Some(v) => v,
                                None => t!(self.get(array, key)),
                            };
                        } else {
                            reg!(iterator) = Value::UNDEFINED;
                            jump!(done);
                        }
                    } else {
                        match t!(self.iterator_step(it, step)) {
                            Some(value) => reg!(dst) = value,
                            None => {
                                reg!(iterator) = Value::UNDEFINED;
                                jump!(done);
                            }
                        }
                    }
                }
                Op::IteratorClose { iterator, silent } => {
                    let (it, step) = (reg!(iterator), reg!(iterator + 1));
                    reg!(iterator) = Value::UNDEFINED;
                    if it != Value::UNDEFINED && step.as_number().is_none() {
                        match self.iterator_close(it) {
                            Ok(()) => {}
                            Err(Abrupt::Throw(_)) if silent => {}
                            Err(e) => t!(Err(e)),
                        }
                    }
                }
                Op::SetPrototype { object, value } => {
                    let (o, v) = (reg!(object), reg!(value));
                    let o = o.as_object().expect("literals are objects");
                    if let Some(proto) = v.as_object() {
                        self.set_prototype_of(o, Some(proto));
                    } else if v == Value::NULL {
                        self.set_prototype_of(o, None);
                    }
                }
                Op::Loop { target } => {
                    t!(self.safepoint());
                    jump!(target);
                }
                Op::Nop => {}
            }
        }
    }

    /// Call a native function with the `argc` values at `args_at` on the
    /// value stack.
    fn call_native_at(
        &mut self,
        behavior: NativeFn,
        this: Value,
        args_at: usize,
        argc: usize,
    ) -> JsResult<Value> {
        if argc <= INLINE_ARGS {
            let mut inline = [Value::UNDEFINED; INLINE_ARGS];
            inline[..argc].copy_from_slice(&self.vm.stack[args_at..args_at + argc]);
            self.call_native(behavior, this, &inline[..argc])
        } else {
            let args = self.vm.stack[args_at..args_at + argc].to_vec();
            self.call_native(behavior, this, &args)
        }
    }

    /// Push the elements of an argument array (built by the compiler, so
    /// dense and plain) onto the value stack; returns how many.
    fn push_array_elements(&mut self, array: Value) -> usize {
        let a = array.as_object().expect("spread arguments are an array");
        let elements = &self.heap.object(a).elements;
        let n = elements.len();
        self.vm.stack.extend(elements.iter().map(|&v| {
            if v == Value::UNINITIALIZED {
                Value::UNDEFINED
            } else {
                v
            }
        }));
        n
    }

    // ---- globals --------------------------------------------------------

    fn get_global(&mut self, code: &FunctionCode, site: u16) -> JsResult<Value> {
        let site = &code.sites[usize::from(site)];
        let cache = site.cache.get();
        let global = self.intrinsics.global;
        match cache.kind {
            CacheKind::GlobalLexical => {
                let (value, _) = self.global_lexical(cache.slot);
                if value == Value::UNINITIALIZED {
                    return Err(self.tdz_error(site.name));
                }
                return Ok(value);
            }
            CacheKind::Own
                if cache.epoch == self.cache_epoch()
                    && self.heap.object(global).shape.0 == cache.shape =>
            {
                return Ok(self.heap.object(global).slots[cache.slot as usize]);
            }
            _ => {}
        }
        if let Some(slot) = self.global_lexical_slot(site.name) {
            site.cache.set(SiteCache {
                kind: CacheKind::GlobalLexical,
                slot,
                ..SiteCache::default()
            });
            let (value, _) = self.global_lexical(slot);
            if value == Value::UNINITIALIZED {
                return Err(self.tdz_error(site.name));
            }
            return Ok(value);
        }
        let key = PropertyKey::String(site.name);
        if let Some(slot) = self.own_data_slot(global, key, false) {
            let shape = self.heap.object(global).shape.0;
            if shape != 0 {
                site.cache.set(SiteCache {
                    kind: CacheKind::Own,
                    epoch: self.cache_epoch(),
                    shape,
                    slot,
                    ..SiteCache::default()
                });
            }
            return Ok(self.heap.object(global).slots[slot as usize]);
        }
        if !self.has_property(global, key)? {
            return Err(self.not_defined_error(site.name));
        }
        self.get(global, key)
    }

    fn set_global(&mut self, code: &FunctionCode, site: u16, value: Value) -> JsResult<()> {
        // A write site only ever caches a mutable lexical binding or a
        // writable data property of a global object that is nobody's
        // prototype.
        let site = &code.sites[usize::from(site)];
        let cache = site.cache.get();
        let global = self.intrinsics.global;
        match cache.kind {
            CacheKind::GlobalLexical => {
                if self.global_lexical(cache.slot).0 == Value::UNINITIALIZED {
                    return Err(self.tdz_error(site.name));
                }
                self.set_global_lexical(cache.slot, value);
                return Ok(());
            }
            CacheKind::Own if cache.epoch == self.cache_epoch() => {
                let object = self.heap.object_mut(global);
                if object.shape.0 == cache.shape && !object.has_flag(FLAG_PROTOTYPE) {
                    object.slots[cache.slot as usize] = value;
                    return Ok(());
                }
            }
            _ => {}
        }
        if let Some(slot) = self.global_lexical_slot(site.name) {
            let (current, flags) = self.global_lexical(slot);
            if current != Value::UNINITIALIZED && flags & BINDING_MUTABLE != 0 {
                site.cache.set(SiteCache {
                    kind: CacheKind::GlobalLexical,
                    slot,
                    ..SiteCache::default()
                });
                self.set_global_lexical(slot, value);
                return Ok(());
            }
        } else if let Some(slot) = self.own_data_slot(global, PropertyKey::String(site.name), true)
        {
            let object = self.heap.object(global);
            let shape = object.shape.0;
            if shape != 0 && !object.has_flag(FLAG_PROTOTYPE) {
                site.cache.set(SiteCache {
                    kind: CacheKind::Own,
                    epoch: self.cache_epoch(),
                    shape,
                    slot,
                    ..SiteCache::default()
                });
                self.heap.object_mut(global).slots[slot as usize] = value;
                return Ok(());
            }
        }
        let env = self.global_env;
        let reference = self.resolve_binding(env, site.name, code.strict)?;
        self.put_value(&reference, value)
    }

    fn declare_global(&mut self, name: StringId, kind: u8) -> JsResult<()> {
        let env = self.global_env;
        let global = self.intrinsics.global;
        if kind == declare::VAR {
            if self.has_lexical_declaration(env, name) {
                return Err(self.redeclaration(name));
            }
            if !self.can_declare_global_var(global, name)? {
                return self.type_error("cannot declare a global variable");
            }
            return self.create_global_var_binding(env, name, false);
        }
        if self.has_lexical_declaration(env, name)
            || self.has_var_declaration(env, name)
            || self.has_restricted_global_property(env, name)?
        {
            return Err(self.redeclaration(name));
        }
        self.vm.lexical_generation = self.vm.lexical_generation.wrapping_add(1);
        if kind == declare::CONST {
            self.create_immutable_binding(env, name, true)
        } else {
            self.create_mutable_binding(env, name, false)
        }
    }

    fn redeclaration(&mut self, name: StringId) -> Abrupt {
        let text = format!("redeclaration of {}", self.heap.string(name).to_text());
        self.throw(ErrorKind::SyntaxError, &text)
    }

    // ---- properties -----------------------------------------------------

    /// The slot of an own data property kept in the object's shape (not a
    /// dictionary entry, an accessor, or an array's `length`).
    fn own_data_slot(&self, o: ObjectId, key: PropertyKey, writable: bool) -> Option<u32> {
        use crate::runtime::Attributes;
        let object = self.heap.object(o);
        if object.dictionary().is_some() {
            return None;
        }
        let slot = self.heap.shapes.lookup(object.shape, key)?;
        if slot.attributes.is_accessor() || (writable && !slot.attributes.has(Attributes::WRITABLE))
        {
            return None;
        }
        Some(slot.slot)
    }

    /// Whether a site may cache lookups of `name` on `o` by shape. Shape 0
    /// is shared by empty objects and every dictionary-mode object, and an
    /// array's `length` lives outside its shape, so neither is cached.
    fn cacheable(&self, o: ObjectId, name: StringId) -> bool {
        self.heap.object(o).shape.0 != 0 && name != self.names.length
    }

    fn get_named(&mut self, code: &FunctionCode, site: u16, object: Value) -> JsResult<Value> {
        let site = &code.sites[usize::from(site)];
        let key = PropertyKey::String(site.name);
        let Some(o) = object.as_object() else {
            if object.is_nullish() {
                return Err(self.nullish_access(site.name));
            }
            return self.get_v(object, key);
        };
        let cache = site.cache.get();
        let receiver = self.heap.object(o);
        if cache.shape == receiver.shape.0 && cache.epoch == self.cache_epoch() {
            match cache.kind {
                CacheKind::Own => return Ok(receiver.slots[cache.slot as usize]),
                CacheKind::Inherited if cache.proto == receiver.proto_raw() => {
                    let holder = self.heap.object(ObjectId(cache.other));
                    return Ok(holder.slots[cache.slot as usize]);
                }
                _ => {}
            }
        }
        // A function's `prototype` slot can hold a not-yet-created marker
        // that only the full lookup understands.
        if !self.cacheable(o, site.name) || site.name == self.names.prototype {
            return self.get(o, key);
        }
        let (shape, proto) = (receiver.shape.0, receiver.proto_raw());
        if let Some(slot) = self.own_data_slot(o, key, false) {
            site.cache.set(SiteCache {
                kind: CacheKind::Own,
                epoch: self.cache_epoch(),
                shape,
                slot,
                ..SiteCache::default()
            });
            return Ok(self.heap.object(o).slots[slot as usize]);
        }
        if self
            .heap
            .shapes
            .lookup(self.heap.object(o).shape, key)
            .is_none()
        {
            // Not an own property: look for a plain data property up the
            // chain. Every object on the chain is flagged as a prototype,
            // so any later change to one moves the cache epoch.
            let mut current = self.heap.object(o).proto();
            while let Some(holder) = current {
                if let Some(slot) = self.own_data_slot(holder, key, false) {
                    site.cache.set(SiteCache {
                        kind: CacheKind::Inherited,
                        epoch: self.cache_epoch(),
                        shape,
                        slot,
                        other: holder.0,
                        proto,
                    });
                    return Ok(self.heap.object(holder).slots[slot as usize]);
                }
                if self.chain_link_has(holder, key) {
                    break;
                }
                current = self.heap.object(holder).proto();
            }
        }
        self.get(o, key)
    }

    /// Whether a prototype-chain object has `key` in a form a cache cannot
    /// describe (a dictionary entry, an accessor, an array's `length`), or
    /// cannot be searched by shape at all.
    fn chain_link_has(&self, o: ObjectId, key: PropertyKey) -> bool {
        let object = self.heap.object(o);
        object.dictionary().is_some()
            || self.heap.shapes.lookup(object.shape, key).is_some()
            || (object.class == Class::Array && key == PropertyKey::String(self.names.length))
    }

    fn set_named(
        &mut self,
        code: &FunctionCode,
        site: u16,
        object: Value,
        value: Value,
    ) -> JsResult<()> {
        let site = &code.sites[usize::from(site)];
        let key = PropertyKey::String(site.name);
        let Some(o) = object.as_object() else {
            if object.is_nullish() {
                return Err(self.nullish_access(site.name));
            }
            return self.set_on_primitive(object, key, value, code.strict);
        };
        let cache = site.cache.get();
        let epoch = self.cache_epoch();
        let receiver = self.heap.object_mut(o);
        // A prototype's own writes go the slow way, which moves the epoch.
        if cache.shape == receiver.shape.0
            && cache.epoch == epoch
            && !receiver.has_flag(FLAG_PROTOTYPE)
        {
            match cache.kind {
                CacheKind::Own => {
                    receiver.slots[cache.slot as usize] = value;
                    return Ok(());
                }
                CacheKind::Add
                    if cache.proto == receiver.proto_raw()
                        && receiver.has_flag(FLAG_EXTENSIBLE)
                        && receiver.dictionary().is_none() =>
                {
                    receiver.shape = ShapeId(cache.other);
                    receiver.slots.push(value);
                    return Ok(self.heap.charge(8)?);
                }
                _ => {}
            }
        }
        if site.name == self.names.length || self.heap.object(o).has_flag(FLAG_PROTOTYPE) {
            return self.set(o, key, value, code.strict);
        }
        if let Some(slot) = self.own_data_slot(o, key, true) {
            let shape = self.heap.object(o).shape.0;
            if shape != 0 {
                site.cache.set(SiteCache {
                    kind: CacheKind::Own,
                    epoch,
                    shape,
                    slot,
                    ..SiteCache::default()
                });
            }
            self.heap.object_mut(o).slots[slot as usize] = value;
            return Ok(());
        }
        // Remember a plain add: the receiver has no such property, nothing
        // on its chain has one either (so no setter or read-only property
        // intercepts), and the write moved it along one shape transition.
        let before = self.heap.object(o);
        let (shape, proto, slots) = (before.shape, before.proto_raw(), before.slots.len());
        let plain = before.dictionary().is_none()
            && self.heap.shapes.lookup(shape, key).is_none()
            && self.chain_lacks(before.proto(), key);
        self.set(o, key, value, code.strict)?;
        if plain {
            let after = self.heap.object(o);
            if after.dictionary().is_none()
                && after.slots.len() == slots + 1
                && after.shape != shape
                && self.cache_epoch() == epoch
            {
                site.cache.set(SiteCache {
                    kind: CacheKind::Add,
                    epoch,
                    shape: shape.0,
                    slot: slots as u32,
                    other: after.shape.0,
                    proto,
                });
            }
        }
        Ok(())
    }

    /// Whether no object from `start` up the chain has `key` at all.
    fn chain_lacks(&self, start: Option<ObjectId>, key: PropertyKey) -> bool {
        let mut current = start;
        while let Some(o) = current {
            if self.chain_link_has(o, key) {
                return false;
            }
            current = self.heap.object(o).proto();
        }
        true
    }

    fn nullish_access(&mut self, name: StringId) -> Abrupt {
        let text = format!(
            "cannot access property {} of undefined or null",
            self.heap.string(name).to_text()
        );
        self.throw(ErrorKind::TypeError, &text)
    }

    /// A dense element read that needs no conversion.
    fn element_fast(&self, object: Value, key: Value) -> Option<Value> {
        let o = object.as_object()?;
        let n = key.as_number()?;
        let i = n as usize;
        if i as f64 != n {
            return None;
        }
        self.heap
            .object(o)
            .elements
            .get(i)
            .copied()
            .filter(|v| *v != Value::UNINITIALIZED)
    }

    /// Overwrite an existing dense element (a writable data property by
    /// construction).
    fn set_element_fast(&mut self, object: Value, key: Value, value: Value) -> bool {
        let (Some(o), Some(n)) = (object.as_object(), key.as_number()) else {
            return false;
        };
        let i = n as usize;
        if i as f64 != n {
            return false;
        }
        match self.heap.object_mut(o).elements.get_mut(i) {
            Some(slot) if *slot != Value::UNINITIALIZED => {
                *slot = value;
                true
            }
            _ => false,
        }
    }

    fn get_property(&mut self, object: Value, key: Value) -> JsResult<Value> {
        if object.is_nullish() {
            return self.type_error("cannot access a property of undefined or null");
        }
        let key = self.to_property_key(key)?;
        self.get_v(object, key)
    }

    fn set_property(
        &mut self,
        object: Value,
        key: Value,
        value: Value,
        strict: bool,
    ) -> JsResult<()> {
        if object.is_nullish() {
            return self.type_error("cannot set a property of undefined or null");
        }
        let key = self.to_property_key(key)?;
        match object.as_object() {
            Some(o) => self.set(o, key, value, strict),
            None => self.set_on_primitive(object, key, value, strict),
        }
    }

    fn set_on_primitive(
        &mut self,
        base: Value,
        key: PropertyKey,
        value: Value,
        strict: bool,
    ) -> JsResult<()> {
        let reference = Reference {
            base: ReferenceBase::Value(base),
            name: key,
            strict,
            this_value: None,
        };
        self.put_value(&reference, value)
    }

    fn array_push(&mut self, array: Value, value: Value) -> JsResult<()> {
        let a = array.as_object().expect("literals are arrays");
        let length = self.heap.object(a).length;
        self.create_data_property_or_throw(a, PropertyKey::Index(length), value)
    }

    /// `CopyDataProperties(target, source, excluded)`.
    fn copy_data_properties(
        &mut self,
        target: Value,
        source: Value,
        excluded: &[PropertyKey],
    ) -> JsResult<()> {
        if source.is_nullish() {
            return Ok(());
        }
        let target = target.as_object().expect("spread targets are objects");
        let from = self.to_object(source)?;
        for key in self.own_property_keys(from) {
            if excluded.contains(&key) {
                continue;
            }
            let Some(desc) = self.get_own_property(from, key)? else {
                continue;
            };
            if desc.enumerable == Some(true) {
                let value = self.get(from, key)?;
                self.create_data_property_or_throw(target, key, value)?;
            }
        }
        Ok(())
    }

    /// An unmapped arguments object (§10.4.4.6).
    fn new_arguments_object(&mut self, args: &[Value]) -> JsResult<Value> {
        let object = self.new_object()?;
        for (i, &v) in args.iter().enumerate() {
            // Argument counts are bounded by the value stack limit.
            self.create_data_property_or_throw(object, PropertyKey::Index(i as u32), v)?;
        }
        let length_key = PropertyKey::String(self.names.length);
        self.define_property_or_throw(
            object,
            length_key,
            PropertyDescriptor::data_with(Value::number(args.len() as f64), true, false, true),
        )?;
        let iterator_key = PropertyKey::Symbol(self.symbols.iterator);
        let values = Value::object(self.intrinsics.array_values);
        self.define_property_or_throw(
            object,
            iterator_key,
            PropertyDescriptor::data_with(values, true, false, true),
        )?;
        Ok(Value::object(object))
    }

    // ---- iteration ------------------------------------------------------

    /// The object and the array of keys a for-in walks.
    fn for_in_keys(&mut self, source: Value) -> JsResult<(Value, Value)> {
        if source.is_nullish() {
            let empty = self.new_array()?;
            return Ok((Value::UNDEFINED, Value::object(empty)));
        }
        let object = self.to_object(source)?;
        let mut keys = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut current = Some(object);
        while let Some(o) = current {
            for key in self.own_property_keys(o) {
                if matches!(key, PropertyKey::Symbol(_)) || !seen.insert(key) {
                    continue;
                }
                // A non-enumerable property still shadows one further up.
                let enumerable = self
                    .get_own_property(o, key)?
                    .is_some_and(|d| d.enumerable == Some(true));
                if enumerable {
                    keys.push(self.key_to_value(key)?);
                }
            }
            current = self.get_prototype_of(o);
        }
        let keys = self.create_array_from_list(&keys)?;
        Ok((Value::object(object), Value::object(keys)))
    }

    /// The next for-in key that is still a property of the object, and
    /// the position after it.
    fn for_in_next(
        &mut self,
        object: Value,
        keys: Value,
        at: Value,
    ) -> JsResult<(Option<Value>, Value)> {
        let keys = keys.as_object().expect("for-in keys are an array");
        let mut index = at.as_number().expect("for-in position is a number") as usize;
        loop {
            let Some(&key) = self.heap.object(keys).elements.get(index) else {
                return Ok((None, at));
            };
            index += 1;
            let object = object.as_object().expect("keys imply an object");
            let property = self.to_property_key(key)?;
            // Keys deleted during the walk are skipped (§14.7.5.9).
            if self.has_property(object, property)? {
                return Ok((Some(key), Value::number(index as f64)));
            }
        }
    }

    /// `GetIterator(source, sync)`: the iterator and its `next` method,
    /// or the array itself and position 0 when nothing could observe the
    /// difference.
    fn get_iterator(&mut self, source: Value) -> JsResult<(Value, Value)> {
        let key = PropertyKey::Symbol(self.symbols.iterator);
        let Some(method) = self.get_method(source, key)? else {
            return self.type_error("value is not iterable");
        };
        if method == Value::object(self.intrinsics.array_values)
            && source
                .as_object()
                .is_some_and(|o| self.heap.object(o).class == Class::Array)
            && self.array_iteration_is_pristine()?
        {
            return Ok((source, Value::number(0.0)));
        }
        let iterator = self.call(method, source, &[])?;
        let Some(object) = iterator.as_object() else {
            return self.type_error("iterator is not an object");
        };
        let next = self.get(object, PropertyKey::String(self.names.next))?;
        Ok((iterator, next))
    }

    /// Whether `%ArrayIteratorPrototype%` still has its own `next` and no
    /// `return`, so walking an array by index is unobservable.
    fn array_iteration_is_pristine(&mut self) -> JsResult<bool> {
        let prototype = self.intrinsics.array_iterator_prototype;
        let next = self.get(prototype, PropertyKey::String(self.names.next))?;
        if next != Value::object(self.intrinsics.array_iterator_next) {
            return Ok(false);
        }
        let close = self.get(prototype, PropertyKey::String(self.names.return_))?;
        Ok(close == Value::UNDEFINED)
    }

    /// `IteratorStepValue`: the next value, or `None` when done.
    fn iterator_step(&mut self, iterator: Value, next: Value) -> JsResult<Option<Value>> {
        let result = self.call(next, iterator, &[])?;
        let Some(result) = result.as_object() else {
            return self.type_error("iterator result is not an object");
        };
        let done = self.get(result, PropertyKey::String(self.names.done))?;
        if self.to_boolean(done) {
            return Ok(None);
        }
        self.get(result, PropertyKey::String(self.names.value))
            .map(Some)
    }

    /// `IteratorClose` for a normal completion.
    fn iterator_close(&mut self, iterator: Value) -> JsResult<()> {
        let key = PropertyKey::String(self.names.return_);
        let Some(method) = self.get_method(iterator, key)? else {
            return Ok(());
        };
        let result = self.call(method, iterator, &[])?;
        if result.as_object().is_none() {
            return self.type_error("iterator result is not an object");
        }
        Ok(())
    }

    // ---- operators ------------------------------------------------------

    /// `ToBoolean`, with the Boolean case inline.
    #[inline]
    fn truthy(&self, value: Value) -> bool {
        if value == Value::TRUE {
            true
        } else if value == Value::FALSE {
            false
        } else {
            self.to_boolean(value)
        }
    }

    fn type_of_value(&mut self, value: Value) -> JsResult<Value> {
        let text = self.type_of(value);
        Ok(Value::string(self.heap.intern_str(text)?))
    }

    fn unary(&mut self, op: UnOp, v: Value) -> JsResult<Value> {
        use crate::runtime::Numeric;
        Ok(match op {
            UnOp::Not => Value::boolean(!self.to_boolean(v)),
            UnOp::Typeof => self.type_of_value(v)?,
            UnOp::Plus => Value::number(self.to_number(v)?),
            UnOp::ToPropertyKey => {
                let key = self.to_property_key(v)?;
                self.key_to_value(key)?
            }
            UnOp::ToString => Value::string(self.to_string(v)?),
            UnOp::ToObject => Value::object(self.to_object(v)?),
            UnOp::BitNot => match self.to_numeric(v)? {
                Numeric::Number(n) => Value::number(f64::from(!crate::runtime::to_int32(n))),
                Numeric::BigInt(_) => return self.bigint_unsupported(),
            },
            UnOp::ToNumeric => match self.to_numeric(v)? {
                Numeric::Number(n) => Value::number(n),
                Numeric::BigInt(b) => Value::bigint(self.heap.alloc_bigint(b)?),
            },
            UnOp::Neg | UnOp::Inc | UnOp::Dec => match self.to_numeric(v)? {
                Numeric::Number(n) => Value::number(match op {
                    UnOp::Neg => -n,
                    UnOp::Inc => n + 1.0,
                    _ => n - 1.0,
                }),
                Numeric::BigInt(_) => return self.bigint_unsupported(),
            },
        })
    }

    fn bigint_unsupported<T>(&mut self) -> JsResult<T> {
        self.type_error("BigInt arithmetic is not supported yet")
    }

    fn binary(&mut self, op: BinOp, x: Value, y: Value) -> JsResult<Value> {
        use crate::runtime::Numeric;
        Ok(match op {
            BinOp::StrictEq => Value::boolean(self.is_strictly_equal(x, y)),
            BinOp::StrictNotEq => Value::boolean(!self.is_strictly_equal(x, y)),
            BinOp::Eq => Value::boolean(self.is_loosely_equal(x, y)?),
            BinOp::NotEq => Value::boolean(!self.is_loosely_equal(x, y)?),
            BinOp::Lt => Value::boolean(self.is_less_than(x, y, true)? == Some(true)),
            BinOp::Gt => Value::boolean(self.is_less_than(y, x, false)? == Some(true)),
            BinOp::LtEq => Value::boolean(self.is_less_than(y, x, false)? == Some(false)),
            BinOp::GtEq => Value::boolean(self.is_less_than(x, y, true)? == Some(false)),
            BinOp::In => {
                let Some(o) = y.as_object() else {
                    return self.type_error("right side of 'in' is not an object");
                };
                let key = self.to_property_key(x)?;
                Value::boolean(self.has_property(o, key)?)
            }
            BinOp::Instanceof => Value::boolean(self.instance_of(x, y)?),
            BinOp::Add => {
                let a = self.to_primitive(x, None)?;
                let b = self.to_primitive(y, None)?;
                if a.as_string().is_some() || b.as_string().is_some() {
                    let a = self.to_string(a)?;
                    let b = self.to_string(b)?;
                    let joined = self.heap.string(a).concat(self.heap.string(b));
                    return self.string_from(joined);
                }
                match (self.to_numeric(a)?, self.to_numeric(b)?) {
                    (Numeric::Number(a), Numeric::Number(b)) => Value::number(a + b),
                    _ => return self.bigint_unsupported(),
                }
            }
            _ => {
                let a = self.to_numeric(x)?;
                let b = self.to_numeric(y)?;
                match (a, b) {
                    (Numeric::Number(a), Numeric::Number(b)) => number_binary(op, a, b),
                    _ => return self.bigint_unsupported(),
                }
            }
        })
    }

    /// `InstanceofOperator(value, target)`.
    fn instance_of(&mut self, value: Value, target: Value) -> JsResult<bool> {
        let Some(target_object) = target.as_object() else {
            return self.type_error("right side of 'instanceof' is not an object");
        };
        let key = PropertyKey::Symbol(self.symbols.has_instance);
        if let Some(method) = self.get_method(target, key)? {
            let result = self.call(method, target, &[value])?;
            return Ok(self.to_boolean(result));
        }
        if !self.is_callable(target) {
            return self.type_error("right side of 'instanceof' is not callable");
        }
        // OrdinaryHasInstance.
        let Some(mut object) = value.as_object() else {
            return Ok(false);
        };
        let prototype = self.get(target_object, PropertyKey::String(self.names.prototype))?;
        let Some(prototype) = prototype.as_object() else {
            return self.type_error("function has a non-object prototype");
        };
        while let Some(parent) = self.get_prototype_of(object) {
            if parent == prototype {
                return Ok(true);
            }
            object = parent;
        }
        Ok(false)
    }
}

/// `Number::exponentiate` (§6.1.6.1.3), where it differs from `powf`.
fn exponentiate(base: f64, exponent: f64) -> f64 {
    if exponent.is_nan() {
        return f64::NAN;
    }
    if base.abs() == 1.0 && exponent.is_infinite() {
        return f64::NAN;
    }
    base.powf(exponent)
}

/// `Number::remainder` (§6.1.6.1.6). Non-negative integers, the common
/// case, take an integer division instead of the much slower `fmod`.
#[inline]
fn remainder(x: f64, y: f64) -> f64 {
    const LIMIT: f64 = 9_007_199_254_740_992.0;
    if x >= 0.0 && y >= 1.0 && x < LIMIT && y < LIMIT {
        let (a, b) = (x as u64, y as u64);
        // Both are integers below 2⁵³, so the conversions are exact; a
        // zero result keeps the dividend's (positive) sign.
        if a as f64 == x && b as f64 == y && !(x == 0.0 && x.is_sign_negative()) {
            return (a % b) as f64;
        }
    }
    x % y
}

/// A comparison of two Numbers.
#[inline(always)]
fn number_compare(op: BinOp, x: f64, y: f64) -> bool {
    match op {
        BinOp::Lt => x < y,
        BinOp::LtEq => x <= y,
        BinOp::Gt => x > y,
        BinOp::GtEq => x >= y,
        BinOp::Eq | BinOp::StrictEq => x == y,
        _ => x != y,
    }
}

/// A binary operator on two Numbers.
#[inline(always)]
fn number_binary(op: BinOp, x: f64, y: f64) -> Value {
    use crate::runtime::{to_int32, to_uint32};
    let shift = || to_uint32(y) & 31;
    match op {
        BinOp::Add => Value::number(x + y),
        BinOp::Sub => Value::number(x - y),
        BinOp::Mul => Value::number(x * y),
        BinOp::Div => Value::number(x / y),
        BinOp::Rem => Value::number(remainder(x, y)),
        BinOp::Exp => Value::number(exponentiate(x, y)),
        BinOp::Shl => Value::number(f64::from(to_int32(x).wrapping_shl(shift()))),
        BinOp::Sar => Value::number(f64::from(to_int32(x) >> shift())),
        BinOp::Shr => Value::number(f64::from(to_uint32(x) >> shift())),
        BinOp::BitAnd => Value::number(f64::from(to_int32(x) & to_int32(y))),
        BinOp::BitOr => Value::number(f64::from(to_int32(x) | to_int32(y))),
        BinOp::BitXor => Value::number(f64::from(to_int32(x) ^ to_int32(y))),
        BinOp::Eq | BinOp::StrictEq => Value::boolean(x == y),
        BinOp::NotEq | BinOp::StrictNotEq => Value::boolean(x != y),
        BinOp::Lt => Value::boolean(x < y),
        BinOp::LtEq => Value::boolean(x <= y),
        BinOp::Gt => Value::boolean(x > y),
        BinOp::GtEq => Value::boolean(x >= y),
        // `in` and `instanceof` need an object on the right; the caller
        // routes them to the slow path, which throws.
        BinOp::In | BinOp::Instanceof => Value::UNDEFINED,
    }
}
