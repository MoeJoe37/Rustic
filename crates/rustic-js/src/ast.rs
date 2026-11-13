//! The abstract syntax tree the parser produces (ECMA-262 §13–§16).
//!
//! Every node carries the [`Span`] of the source it covers. Strings are
//! UTF-16 code units, as ECMAScript string values are; identifier names are
//! Rust strings because they can never contain lone surrogates.
//!
//! Left-nested chains (`a + b + c ...`, `a.b.c ...`) are built without
//! recursion and may be arbitrarily deep, so [`Expression`] drops its
//! children iteratively and [`dump`] has an explicit depth limit.

use std::fmt::Write;

use crate::source::Span;
use crate::token::TemplatePart;

/// A parsed script or module.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    /// Whether this is a script or a module.
    pub kind: ProgramKind,
    /// The top-level statements, including the directive prologue.
    pub body: Vec<Statement>,
    /// Whether the code is strict (modules always are).
    pub strict: bool,
    /// The whole source.
    pub span: Span,
}

/// The goal symbol a program was parsed with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramKind {
    /// `Script`.
    Script,
    /// `Module`.
    Module,
}

/// A name in the source.
#[derive(Clone, Debug, PartialEq)]
pub struct Identifier {
    /// The name with escapes decoded.
    pub name: Box<str>,
    /// Where it is.
    pub span: Span,
}

/// A statement or declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    /// What it is.
    pub kind: StatementKind,
    /// Where it is.
    pub span: Span,
}

/// The kinds of statements and declarations.
#[derive(Clone, Debug, PartialEq)]
pub enum StatementKind {
    /// An expression statement; `directive` is set for members of a
    /// directive prologue such as `"use strict"`.
    Expression {
        /// The expression.
        expression: Expression,
        /// Whether it is part of a directive prologue.
        directive: bool,
    },
    /// `{ ... }`.
    Block(Vec<Statement>),
    /// `;`.
    Empty,
    /// `debugger;`.
    Debugger,
    /// `var`, `let` or `const`.
    Variable(VariableDeclaration),
    /// A function, generator or async function declaration.
    Function(Box<Function>),
    /// A class declaration.
    Class(Box<Class>),
    /// `if (test) consequent else alternate`.
    If {
        /// The condition.
        test: Expression,
        /// The statement run when it holds.
        consequent: Box<Statement>,
        /// The `else` statement.
        alternate: Option<Box<Statement>>,
    },
    /// `for (init; test; update) body`.
    For {
        /// The initializer.
        init: Option<ForInit>,
        /// The condition.
        test: Option<Expression>,
        /// The update expression.
        update: Option<Expression>,
        /// The loop body.
        body: Box<Statement>,
    },
    /// `for (left in right) body`.
    ForIn {
        /// The binding or assignment target.
        left: ForHead,
        /// The object whose keys are enumerated.
        right: Expression,
        /// The loop body.
        body: Box<Statement>,
    },
    /// `for (left of right) body` or `for await (...)`.
    ForOf {
        /// The binding or assignment target.
        left: ForHead,
        /// The iterable.
        right: Expression,
        /// The loop body.
        body: Box<Statement>,
        /// `for await`.
        is_await: bool,
    },
    /// `while (test) body`.
    While {
        /// The condition.
        test: Expression,
        /// The loop body.
        body: Box<Statement>,
    },
    /// `do body while (test)`.
    DoWhile {
        /// The loop body.
        body: Box<Statement>,
        /// The condition.
        test: Expression,
    },
    /// `continue label;`.
    Continue(Option<Identifier>),
    /// `break label;`.
    Break(Option<Identifier>),
    /// `return argument;`.
    Return(Option<Expression>),
    /// `with (object) body` (sloppy mode only).
    With {
        /// The object.
        object: Expression,
        /// The body.
        body: Box<Statement>,
    },
    /// `switch (discriminant) { cases }`.
    Switch {
        /// The value switched on.
        discriminant: Expression,
        /// The clauses in source order.
        cases: Vec<SwitchCase>,
    },
    /// `label: body`.
    Labeled {
        /// The label.
        label: Identifier,
        /// The labelled statement.
        body: Box<Statement>,
    },
    /// `throw argument;`.
    Throw(Expression),
    /// `try { } catch (param) { } finally { }`.
    Try {
        /// The protected block.
        block: Vec<Statement>,
        /// The `catch` clause.
        handler: Option<CatchClause>,
        /// The `finally` block.
        finalizer: Option<Vec<Statement>>,
    },
    /// An `import` declaration (modules only).
    Import(ImportDeclaration),
    /// An `export` declaration (modules only).
    Export(ExportDeclaration),
}

/// `var`, `let` or `const` declarations.
#[derive(Clone, Debug, PartialEq)]
pub struct VariableDeclaration {
    /// Which keyword.
    pub kind: VariableKind,
    /// The declarators.
    pub declarations: Vec<VariableDeclarator>,
}

/// The declaration keyword.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariableKind {
    /// `var`.
    Var,
    /// `let`.
    Let,
    /// `const`.
    Const,
}

/// One `target = init` in a variable declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct VariableDeclarator {
    /// The binding.
    pub target: Pattern,
    /// The initializer.
    pub init: Option<Expression>,
    /// Where it is.
    pub span: Span,
}

/// The first clause of `for (;;)`.
#[derive(Clone, Debug, PartialEq)]
pub enum ForInit {
    /// A declaration.
    Variable(VariableDeclaration),
    /// An expression.
    Expression(Expression),
}

/// The left side of `for-in` and `for-of`.
#[derive(Clone, Debug, PartialEq)]
pub enum ForHead {
    /// `var x`, `let x` or `const x` (one declarator; an initializer only
    /// for the Annex B `for (var x = 1 in o)` form).
    Variable(VariableDeclaration),
    /// An assignment target.
    Pattern(Pattern),
}

/// A `case` or `default` clause.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitchCase {
    /// The `case` expression, or `None` for `default`.
    pub test: Option<Expression>,
    /// The statements.
    pub body: Vec<Statement>,
    /// Where it is.
    pub span: Span,
}

/// A `catch` clause.
#[derive(Clone, Debug, PartialEq)]
pub struct CatchClause {
    /// The binding, absent for `catch { }`.
    pub param: Option<Pattern>,
    /// The handler block.
    pub body: Vec<Statement>,
    /// Where it is.
    pub span: Span,
}

/// A binding pattern or assignment target.
#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    /// What it is.
    pub kind: PatternKind,
    /// Where it is.
    pub span: Span,
}

impl Pattern {
    pub(crate) fn bound_identifiers<'a>(&'a self, out: &mut Vec<&'a Identifier>) {
        let mut stack = vec![self];
        let start = out.len();
        while let Some(p) = stack.pop() {
            match &p.kind {
                PatternKind::Identifier(id) => out.push(id),
                PatternKind::Member(_) => {}
                PatternKind::Object { properties, rest } => {
                    stack.extend(rest.as_deref());
                    stack.extend(properties.iter().rev().map(|prop| &prop.value));
                }
                PatternKind::Array { elements, rest } => {
                    stack.extend(rest.as_deref());
                    stack.extend(elements.iter().rev().flatten());
                }
                PatternKind::Assignment { target, .. } => stack.push(target),
            }
        }
        out[start..].sort_by_key(|id| id.span.start);
    }
}

/// The kinds of patterns.
#[derive(Clone, Debug, PartialEq)]
pub enum PatternKind {
    /// A name.
    Identifier(Identifier),
    /// A member expression (assignment targets only).
    Member(Box<Expression>),
    /// `{ a, b: c, ...rest }`.
    Object {
        /// The properties.
        properties: Vec<PatternProperty>,
        /// The rest element.
        rest: Option<Box<Pattern>>,
    },
    /// `[a, , b, ...rest]`.
    Array {
        /// The elements; `None` is an elision.
        elements: Vec<Option<Pattern>>,
        /// The rest element.
        rest: Option<Box<Pattern>>,
    },
    /// `target = default`.
    Assignment {
        /// The target.
        target: Box<Pattern>,
        /// The default value.
        default: Box<Expression>,
    },
}

/// One property of an object pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternProperty {
    /// The property key.
    pub key: PropertyKey,
    /// The target (with its default, if any).
    pub value: Pattern,
    /// Whether it was written `{ a }` or `{ a = 1 }`.
    pub shorthand: bool,
    /// Where it is.
    pub span: Span,
}

/// A property name in an object literal, pattern or class.
#[derive(Clone, Debug, PartialEq)]
pub enum PropertyKey {
    /// An identifier name (including reserved words).
    Identifier(Identifier),
    /// A string literal key.
    String(Vec<u16>, Span),
    /// A numeric literal key.
    Number(f64, Span),
    /// A BigInt literal key (radix and digits).
    BigInt(u32, Box<str>, Span),
    /// `[expression]`.
    Computed(Box<Expression>),
    /// A class element's `#name`.
    Private(Identifier),
}

impl PropertyKey {
    /// The static name of a non-computed key as UTF-16 code units, for
    /// early errors such as duplicate `__proto__`. Numeric keys are not
    /// converted (they never equal the names those rules look for).
    pub fn static_name(&self) -> Option<Vec<u16>> {
        match self {
            PropertyKey::Identifier(id) => Some(id.name.encode_utf16().collect()),
            PropertyKey::String(s, _) => Some(s.clone()),
            _ => None,
        }
    }
}

/// An expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Expression {
    /// What it is.
    pub kind: ExpressionKind,
    /// Where it is.
    pub span: Span,
}

impl Expression {
    /// A node with no children, used to take children out while dropping.
    const fn leaf(span: Span) -> Self {
        Self {
            kind: ExpressionKind::Null,
            span,
        }
    }
}

impl Drop for Expression {
    /// Drops left-nested chains iteratively so their depth cannot overflow
    /// the stack.
    fn drop(&mut self) {
        let mut stack: Vec<Expression> = Vec::new();
        take_chain_children(&mut self.kind, &mut stack);
        while let Some(mut e) = stack.pop() {
            take_chain_children(&mut e.kind, &mut stack);
        }
    }
}

fn take_chain_children(kind: &mut ExpressionKind, stack: &mut Vec<Expression>) {
    let mut take = |e: &mut Box<Expression>| {
        let span = e.span;
        stack.push(std::mem::replace(&mut **e, Expression::leaf(span)));
    };
    match kind {
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            take(left);
            take(right);
        }
        ExpressionKind::Member { object, .. } => take(object),
        ExpressionKind::Call { callee, .. } | ExpressionKind::New { callee, .. } => take(callee),
        ExpressionKind::TaggedTemplate { tag, .. } => take(tag),
        ExpressionKind::OptionalChain(inner)
        | ExpressionKind::Parenthesized(inner)
        | ExpressionKind::Await(inner)
        | ExpressionKind::Unary {
            argument: inner, ..
        } => take(inner),
        ExpressionKind::Assignment { value, .. } => take(value),
        _ => {}
    }
}

/// The kinds of expressions.
#[derive(Clone, Debug, PartialEq)]
pub enum ExpressionKind {
    /// `this`.
    This,
    /// A name reference.
    Identifier(Identifier),
    /// `null`.
    Null,
    /// `true` or `false`.
    Boolean(bool),
    /// A Number literal.
    Number(f64),
    /// A BigInt literal (radix and digits).
    BigInt(u32, Box<str>),
    /// A string literal.
    String(Vec<u16>),
    /// A regular expression literal.
    RegExp {
        /// The pattern source.
        body: Vec<u16>,
        /// The flags.
        flags: Box<str>,
    },
    /// An untagged template literal.
    Template(Template),
    /// `` tag`...` ``.
    TaggedTemplate {
        /// The tag function expression.
        tag: Box<Expression>,
        /// The template.
        template: Template,
    },
    /// `[a, , ...b]`.
    Array(Vec<Option<ArrayElement>>),
    /// `{ ... }`.
    Object(Vec<ObjectProperty>),
    /// A function, generator or async function expression.
    Function(Box<Function>),
    /// An arrow function.
    Arrow(Box<ArrowFunction>),
    /// A class expression.
    Class(Box<Class>),
    /// A prefix unary operator.
    Unary {
        /// The operator.
        op: UnaryOp,
        /// The operand.
        argument: Box<Expression>,
    },
    /// `++` or `--`.
    Update {
        /// The operator.
        op: UpdateOp,
        /// Prefix (`++a`) or postfix (`a++`).
        prefix: bool,
        /// The target.
        target: Box<Pattern>,
    },
    /// A binary operator other than the logical ones.
    Binary {
        /// The operator.
        op: BinaryOp,
        /// The left operand.
        left: Box<Expression>,
        /// The right operand.
        right: Box<Expression>,
    },
    /// `#x in object`.
    PrivateIn {
        /// The private name.
        name: Identifier,
        /// The object.
        right: Box<Expression>,
    },
    /// `&&`, `||` or `??`.
    Logical {
        /// The operator.
        op: LogicalOp,
        /// The left operand.
        left: Box<Expression>,
        /// The right operand.
        right: Box<Expression>,
    },
    /// `test ? consequent : alternate`.
    Conditional {
        /// The condition.
        test: Box<Expression>,
        /// The value when it holds.
        consequent: Box<Expression>,
        /// The value otherwise.
        alternate: Box<Expression>,
    },
    /// An assignment.
    Assignment {
        /// The operator.
        op: AssignOp,
        /// The target.
        target: Box<Pattern>,
        /// The value.
        value: Box<Expression>,
    },
    /// `a, b, c`.
    Sequence(Vec<Expression>),
    /// `object.name`, `object[expr]`, `object.#name`, or the same after `?.`.
    Member {
        /// The object.
        object: Box<Expression>,
        /// The property.
        property: MemberProperty,
        /// Whether `?.` precedes this access.
        optional: bool,
    },
    /// `super.name` or `super[expr]`.
    SuperMember(MemberProperty),
    /// A call.
    Call {
        /// The function.
        callee: Box<Expression>,
        /// The arguments.
        arguments: Vec<Argument>,
        /// Whether `?.` precedes the call.
        optional: bool,
    },
    /// `super(...)`.
    SuperCall(Vec<Argument>),
    /// `new callee(arguments)`.
    New {
        /// The constructor.
        callee: Box<Expression>,
        /// The arguments (empty for `new C`).
        arguments: Vec<Argument>,
    },
    /// The end of an optional chain (`a?.b.c`): short-circuiting stops here.
    OptionalChain(Box<Expression>),
    /// `(expression)`, kept so early errors and tools can see it.
    Parenthesized(Box<Expression>),
    /// `new.target`.
    NewTarget,
    /// `import.meta` (modules only).
    ImportMeta,
    /// `import(source, options)`.
    ImportCall {
        /// The specifier.
        source: Box<Expression>,
        /// The options argument.
        options: Option<Box<Expression>>,
    },
    /// `yield`, `yield argument` or `yield* argument`.
    Yield {
        /// The operand.
        argument: Option<Box<Expression>>,
        /// `yield*`.
        delegate: bool,
    },
    /// `await argument`.
    Await(Box<Expression>),
}

/// A template literal's pieces and substitutions (`quasis.len() ==
/// expressions.len() + 1`).
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    /// The text pieces.
    pub quasis: Vec<TemplatePart>,
    /// The substitutions.
    pub expressions: Vec<Expression>,
}

/// An array literal element.
#[derive(Clone, Debug, PartialEq)]
pub enum ArrayElement {
    /// A value.
    Expression(Expression),
    /// `...value`.
    Spread(Expression),
}

/// A call or `new` argument.
#[derive(Clone, Debug, PartialEq)]
pub enum Argument {
    /// A value.
    Expression(Expression),
    /// `...value`.
    Spread(Expression),
}

/// The property of a member expression.
#[derive(Clone, Debug, PartialEq)]
pub enum MemberProperty {
    /// `.name`.
    Name(Identifier),
    /// `[expression]`.
    Computed(Box<Expression>),
    /// `.#name`.
    Private(Identifier),
}

/// An object literal member.
#[derive(Clone, Debug, PartialEq)]
pub enum ObjectProperty {
    /// `key: value` or shorthand `key`.
    Property {
        /// The key.
        key: PropertyKey,
        /// The value.
        value: Expression,
        /// Whether it was written `{ key }`.
        shorthand: bool,
        /// Where it is.
        span: Span,
    },
    /// A method, getter or setter.
    Method {
        /// The key.
        key: PropertyKey,
        /// The function.
        function: Box<Function>,
        /// Which kind.
        kind: MethodKind,
        /// Where it is.
        span: Span,
    },
    /// `...value`.
    Spread(Expression),
}

/// Methods, getters and setters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodKind {
    /// A plain, generator or async method.
    Method,
    /// `get`.
    Get,
    /// `set`.
    Set,
    /// A class `constructor`.
    Constructor,
}

/// A function, generator, async function or method.
#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    /// The name (absent for anonymous expressions and methods).
    pub id: Option<Identifier>,
    /// The formal parameters (with defaults as assignment patterns).
    pub params: Vec<Pattern>,
    /// The rest parameter.
    pub rest: Option<Pattern>,
    /// The body statements, including any directive prologue.
    pub body: Vec<Statement>,
    /// `async`.
    pub is_async: bool,
    /// `function*`.
    pub is_generator: bool,
    /// Whether the body is strict code.
    pub strict: bool,
    /// Where it is.
    pub span: Span,
}

/// An arrow function.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrowFunction {
    /// The parameters.
    pub params: Vec<Pattern>,
    /// The rest parameter.
    pub rest: Option<Pattern>,
    /// The body.
    pub body: ArrowBody,
    /// `async`.
    pub is_async: bool,
    /// Whether the body is strict code.
    pub strict: bool,
    /// Where it is.
    pub span: Span,
}

/// An arrow function body.
#[derive(Clone, Debug, PartialEq)]
pub enum ArrowBody {
    /// `=> expression`.
    Expression(Box<Expression>),
    /// `=> { statements }`.
    Block(Vec<Statement>),
}

/// A class declaration or expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Class {
    /// The name.
    pub id: Option<Identifier>,
    /// The `extends` expression.
    pub super_class: Option<Box<Expression>>,
    /// The members.
    pub body: Vec<ClassElement>,
    /// Where it is.
    pub span: Span,
}

/// A class member.
#[derive(Clone, Debug, PartialEq)]
pub enum ClassElement {
    /// A method, getter, setter or the constructor.
    Method {
        /// The key (possibly private).
        key: PropertyKey,
        /// The function.
        function: Box<Function>,
        /// Which kind.
        kind: MethodKind,
        /// `static`.
        is_static: bool,
        /// Where it is.
        span: Span,
    },
    /// A field.
    Field {
        /// The key (possibly private).
        key: PropertyKey,
        /// The initializer.
        value: Option<Expression>,
        /// `static`.
        is_static: bool,
        /// Where it is.
        span: Span,
    },
    /// `static { ... }`.
    StaticBlock(Vec<Statement>, Span),
}

/// The operand-taking prefix operators.
#[allow(missing_docs, reason = "each variant is named after its operator")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Minus,
    Plus,
    Not,
    BitNot,
    Typeof,
    Void,
    Delete,
}

/// `++` and `--`.
#[allow(missing_docs, reason = "each variant is named after its operator")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateOp {
    Increment,
    Decrement,
}

/// Binary operators other than `&&`, `||` and `??`.
#[allow(missing_docs, reason = "each variant is named after its operator")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Eq,
    NotEq,
    StrictEq,
    StrictNotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Shl,
    Sar,
    Shr,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Exp,
    BitOr,
    BitXor,
    BitAnd,
    In,
    Instanceof,
}

/// `&&`, `||` and `??`.
#[allow(missing_docs, reason = "each variant is named after its operator")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicalOp {
    And,
    Or,
    Coalesce,
}

/// Assignment operators.
#[allow(missing_docs, reason = "each variant is named after its operator")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Exp,
    Shl,
    Sar,
    Shr,
    BitOr,
    BitXor,
    BitAnd,
    And,
    Or,
    Coalesce,
}

/// An `import` declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportDeclaration {
    /// The imported bindings (empty for `import "m"`).
    pub specifiers: Vec<ImportSpecifier>,
    /// The module specifier.
    pub source: Vec<u16>,
    /// `with { type: "json" }` import attributes.
    pub attributes: Vec<ImportAttribute>,
}

/// One imported binding.
#[derive(Clone, Debug, PartialEq)]
pub enum ImportSpecifier {
    /// `import local from`.
    Default(Identifier),
    /// `import * as local from`.
    Namespace(Identifier),
    /// `import { imported as local } from`.
    Named {
        /// The exported name in the other module.
        imported: ModuleExportName,
        /// The local binding.
        local: Identifier,
    },
}

/// An export name: an identifier name or a string literal.
#[derive(Clone, Debug, PartialEq)]
pub enum ModuleExportName {
    /// An identifier name.
    Identifier(Identifier),
    /// A string literal (well-formed UTF-16).
    String(Vec<u16>, Span),
}

/// `key: "value"` in an import attribute list.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportAttribute {
    /// The key (identifier name or string).
    pub key: Vec<u16>,
    /// The value.
    pub value: Vec<u16>,
    /// Where it is.
    pub span: Span,
}

/// An `export` declaration.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportDeclaration {
    /// `export { a as b }` or `export { a } from "m"`.
    Named {
        /// The specifiers.
        specifiers: Vec<ExportSpecifier>,
        /// The module re-exported from.
        source: Option<Vec<u16>>,
        /// Import attributes of the re-export.
        attributes: Vec<ImportAttribute>,
    },
    /// `export * from "m"` or `export * as name from "m"`.
    All {
        /// The namespace export name.
        exported: Option<ModuleExportName>,
        /// The module.
        source: Vec<u16>,
        /// Import attributes.
        attributes: Vec<ImportAttribute>,
    },
    /// `export var/let/const/function/class ...`.
    Declaration(Box<Statement>),
    /// `export default function/class` or `export default expression;`.
    Default(DefaultExport),
}

/// One `local as exported` export specifier.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportSpecifier {
    /// The local binding (or the other module's name for re-exports).
    pub local: ModuleExportName,
    /// The exported name.
    pub exported: ModuleExportName,
}

/// What `export default` exports.
#[derive(Clone, Debug, PartialEq)]
pub enum DefaultExport {
    /// A (possibly anonymous) function declaration.
    Function(Box<Function>),
    /// A (possibly anonymous) class declaration.
    Class(Box<Class>),
    /// An expression.
    Expression(Expression),
}

/// [`dump`] reached its depth limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DumpTooDeep;

/// Most nested nodes [`dump`] prints before failing.
pub const MAX_DUMP_DEPTH: usize = 512;

/// A compact S-expression rendering of `program` for tests and tools,
/// such as `(script (var (let x 1)))`. Fails rather than truncating when
/// the tree is nested more than [`MAX_DUMP_DEPTH`] deep.
pub fn dump(program: &Program) -> Result<String, DumpTooDeep> {
    let mut out = Dumper {
        out: String::new(),
        depth: 0,
    };
    out.open(match program.kind {
        ProgramKind::Script => "script",
        ProgramKind::Module => "module",
    })?;
    if program.strict {
        out.out.push_str(" strict");
    }
    for s in &program.body {
        out.statement(s)?;
    }
    out.close();
    Ok(out.out)
}

struct Dumper {
    out: String,
    depth: usize,
}

type DumpResult = Result<(), DumpTooDeep>;

fn utf16(units: &[u16]) -> String {
    let mut s = String::from('"');
    for c in char::decode_utf16(units.iter().copied()) {
        match c {
            Ok('"') => s.push_str("\\\""),
            Ok('\\') => s.push_str("\\\\"),
            Ok(c) if c.is_control() => {
                let _ = write!(s, "\\u{{{:x}}}", u32::from(c));
            }
            Ok(c) => s.push(c),
            Err(e) => {
                let _ = write!(s, "\\u{{{:x}}}", e.unpaired_surrogate());
            }
        }
    }
    s.push('"');
    s
}

fn number(value: f64) -> String {
    if value.is_finite() && value == value.trunc() && value.abs() < 1e21 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

impl Dumper {
    fn open(&mut self, head: &str) -> DumpResult {
        self.depth += 1;
        if self.depth > MAX_DUMP_DEPTH {
            return Err(DumpTooDeep);
        }
        if !self.out.is_empty() {
            self.out.push(' ');
        }
        self.out.push('(');
        self.out.push_str(head);
        Ok(())
    }

    fn close(&mut self) {
        self.depth -= 1;
        self.out.push(')');
    }

    fn atom(&mut self, text: &str) {
        self.out.push(' ');
        self.out.push_str(text);
    }

    fn list<T>(
        &mut self,
        head: &str,
        items: &[T],
        mut each: impl FnMut(&mut Self, &T) -> DumpResult,
    ) -> DumpResult {
        self.open(head)?;
        for item in items {
            each(self, item)?;
        }
        self.close();
        Ok(())
    }

    fn statements(&mut self, head: &str, body: &[Statement]) -> DumpResult {
        self.list(head, body, Self::statement)
    }

    fn statement(&mut self, s: &Statement) -> DumpResult {
        use StatementKind as S;
        match &s.kind {
            S::Expression {
                expression,
                directive,
            } => {
                self.open(if *directive { "directive" } else { "expr" })?;
                self.expression(expression)?;
                self.close();
            }
            S::Block(body) => self.statements("block", body)?,
            S::Empty => self.atom("(empty)"),
            S::Debugger => self.atom("(debugger)"),
            S::Variable(v) => self.variable(v)?,
            S::Function(f) => self.function("function", f)?,
            S::Class(c) => self.class(c)?,
            S::If {
                test,
                consequent,
                alternate,
            } => {
                self.open("if")?;
                self.expression(test)?;
                self.statement(consequent)?;
                if let Some(a) = alternate {
                    self.statement(a)?;
                }
                self.close();
            }
            S::For {
                init,
                test,
                update,
                body,
            } => {
                self.open("for")?;
                match init {
                    Some(ForInit::Variable(v)) => self.variable(v)?,
                    Some(ForInit::Expression(e)) => self.expression(e)?,
                    None => self.atom("_"),
                }
                self.opt_expression(test.as_ref())?;
                self.opt_expression(update.as_ref())?;
                self.statement(body)?;
                self.close();
            }
            S::ForIn { left, right, body } => {
                self.open("for-in")?;
                self.for_head(left)?;
                self.expression(right)?;
                self.statement(body)?;
                self.close();
            }
            S::ForOf {
                left,
                right,
                body,
                is_await,
            } => {
                self.open(if *is_await { "for-await-of" } else { "for-of" })?;
                self.for_head(left)?;
                self.expression(right)?;
                self.statement(body)?;
                self.close();
            }
            S::While { test, body } => {
                self.open("while")?;
                self.expression(test)?;
                self.statement(body)?;
                self.close();
            }
            S::DoWhile { body, test } => {
                self.open("do-while")?;
                self.statement(body)?;
                self.expression(test)?;
                self.close();
            }
            S::Continue(label) | S::Break(label) => {
                self.open(if matches!(s.kind, S::Continue(_)) {
                    "continue"
                } else {
                    "break"
                })?;
                if let Some(l) = label {
                    self.atom(&l.name);
                }
                self.close();
            }
            S::Return(e) => {
                self.open("return")?;
                if let Some(e) = e {
                    self.expression(e)?;
                }
                self.close();
            }
            S::With { object, body } => {
                self.open("with")?;
                self.expression(object)?;
                self.statement(body)?;
                self.close();
            }
            S::Switch {
                discriminant,
                cases,
            } => {
                self.open("switch")?;
                self.expression(discriminant)?;
                for case in cases {
                    self.open(if case.test.is_some() {
                        "case"
                    } else {
                        "default"
                    })?;
                    self.opt_expression(case.test.as_ref())?;
                    for s in &case.body {
                        self.statement(s)?;
                    }
                    self.close();
                }
                self.close();
            }
            S::Labeled { label, body } => {
                self.open("label")?;
                self.atom(&label.name);
                self.statement(body)?;
                self.close();
            }
            S::Throw(e) => {
                self.open("throw")?;
                self.expression(e)?;
                self.close();
            }
            S::Try {
                block,
                handler,
                finalizer,
            } => {
                self.open("try")?;
                self.statements("block", block)?;
                if let Some(h) = handler {
                    self.open("catch")?;
                    match &h.param {
                        Some(p) => self.pattern(p)?,
                        None => self.atom("_"),
                    }
                    self.statements("block", &h.body)?;
                    self.close();
                }
                if let Some(f) = finalizer {
                    self.statements("finally", f)?;
                }
                self.close();
            }
            S::Import(i) => {
                self.open("import")?;
                for spec in &i.specifiers {
                    match spec {
                        ImportSpecifier::Default(id) => {
                            self.open("default")?;
                            self.atom(&id.name);
                            self.close();
                        }
                        ImportSpecifier::Namespace(id) => {
                            self.open("namespace")?;
                            self.atom(&id.name);
                            self.close();
                        }
                        ImportSpecifier::Named { imported, local } => {
                            self.open("named")?;
                            self.export_name(imported);
                            self.atom(&local.name);
                            self.close();
                        }
                    }
                }
                self.atom(&utf16(&i.source));
                self.attributes(&i.attributes)?;
                self.close();
            }
            S::Export(e) => self.export(e)?,
        }
        Ok(())
    }

    fn export_name(&mut self, name: &ModuleExportName) {
        match name {
            ModuleExportName::Identifier(id) => self.atom(&id.name),
            ModuleExportName::String(s, _) => self.atom(&utf16(s)),
        }
    }

    fn attributes(&mut self, attributes: &[ImportAttribute]) -> DumpResult {
        if attributes.is_empty() {
            return Ok(());
        }
        self.open("with")?;
        for a in attributes {
            self.atom(&format!("{}:{}", utf16(&a.key), utf16(&a.value)));
        }
        self.close();
        Ok(())
    }

    fn export(&mut self, e: &ExportDeclaration) -> DumpResult {
        self.open("export")?;
        match e {
            ExportDeclaration::Named {
                specifiers,
                source,
                attributes,
            } => {
                for s in specifiers {
                    self.open("as")?;
                    self.export_name(&s.local);
                    self.export_name(&s.exported);
                    self.close();
                }
                if let Some(source) = source {
                    self.atom(&utf16(source));
                }
                self.attributes(attributes)?;
            }
            ExportDeclaration::All {
                exported,
                source,
                attributes,
            } => {
                self.atom("*");
                if let Some(name) = exported {
                    self.export_name(name);
                }
                self.atom(&utf16(source));
                self.attributes(attributes)?;
            }
            ExportDeclaration::Declaration(s) => self.statement(s)?,
            ExportDeclaration::Default(d) => {
                self.atom("default");
                match d {
                    DefaultExport::Function(f) => self.function("function", f)?,
                    DefaultExport::Class(c) => self.class(c)?,
                    DefaultExport::Expression(e) => self.expression(e)?,
                }
            }
        }
        self.close();
        Ok(())
    }

    fn for_head(&mut self, head: &ForHead) -> DumpResult {
        match head {
            ForHead::Variable(v) => self.variable(v),
            ForHead::Pattern(p) => self.pattern(p),
        }
    }

    fn variable(&mut self, v: &VariableDeclaration) -> DumpResult {
        self.open(match v.kind {
            VariableKind::Var => "var",
            VariableKind::Let => "let",
            VariableKind::Const => "const",
        })?;
        for d in &v.declarations {
            if let Some(init) = &d.init {
                self.open("=")?;
                self.pattern(&d.target)?;
                self.expression(init)?;
                self.close();
            } else {
                self.pattern(&d.target)?;
            }
        }
        self.close();
        Ok(())
    }

    fn params(&mut self, params: &[Pattern], rest: Option<&Pattern>) -> DumpResult {
        self.open("params")?;
        for p in params {
            self.pattern(p)?;
        }
        if let Some(r) = rest {
            self.open("...")?;
            self.pattern(r)?;
            self.close();
        }
        self.close();
        Ok(())
    }

    fn function(&mut self, head: &str, f: &Function) -> DumpResult {
        let mut head = head.to_owned();
        if f.is_async {
            head.insert_str(0, "async-");
        }
        if f.is_generator {
            head.push('*');
        }
        self.open(&head)?;
        if let Some(id) = &f.id {
            self.atom(&id.name);
        }
        if f.strict {
            self.atom("strict");
        }
        self.params(&f.params, f.rest.as_ref())?;
        self.statements("body", &f.body)?;
        self.close();
        Ok(())
    }

    fn class(&mut self, c: &Class) -> DumpResult {
        self.open("class")?;
        if let Some(id) = &c.id {
            self.atom(&id.name);
        }
        if let Some(s) = &c.super_class {
            self.open("extends")?;
            self.expression(s)?;
            self.close();
        }
        for element in &c.body {
            match element {
                ClassElement::Method {
                    key,
                    function,
                    kind,
                    is_static,
                    ..
                } => {
                    self.open(if *is_static {
                        "static-method"
                    } else {
                        "method"
                    })?;
                    self.method_kind(*kind);
                    self.key(key)?;
                    self.function("function", function)?;
                    self.close();
                }
                ClassElement::Field {
                    key,
                    value,
                    is_static,
                    ..
                } => {
                    self.open(if *is_static { "static-field" } else { "field" })?;
                    self.key(key)?;
                    self.opt_expression(value.as_ref())?;
                    self.close();
                }
                ClassElement::StaticBlock(body, _) => self.statements("static-block", body)?,
            }
        }
        self.close();
        Ok(())
    }

    fn method_kind(&mut self, kind: MethodKind) {
        match kind {
            MethodKind::Method => {}
            MethodKind::Get => self.atom("get"),
            MethodKind::Set => self.atom("set"),
            MethodKind::Constructor => self.atom("constructor"),
        }
    }

    fn key(&mut self, key: &PropertyKey) -> DumpResult {
        match key {
            PropertyKey::Identifier(id) => self.atom(&id.name),
            PropertyKey::String(s, _) => self.atom(&utf16(s)),
            PropertyKey::Number(n, _) => self.atom(&number(*n)),
            PropertyKey::BigInt(radix, digits, _) => self.atom(&bigint(*radix, digits)),
            PropertyKey::Computed(e) => {
                self.open("computed")?;
                self.expression(e)?;
                self.close();
            }
            PropertyKey::Private(id) => self.atom(&format!("#{}", id.name)),
        }
        Ok(())
    }

    fn pattern(&mut self, p: &Pattern) -> DumpResult {
        match &p.kind {
            PatternKind::Identifier(id) => self.atom(&id.name),
            PatternKind::Member(e) => self.expression(e)?,
            PatternKind::Object { properties, rest } => {
                self.open("object-pattern")?;
                for prop in properties {
                    if prop.shorthand {
                        self.pattern(&prop.value)?;
                    } else {
                        self.open(":")?;
                        self.key(&prop.key)?;
                        self.pattern(&prop.value)?;
                        self.close();
                    }
                }
                if let Some(r) = rest {
                    self.open("...")?;
                    self.pattern(r)?;
                    self.close();
                }
                self.close();
            }
            PatternKind::Array { elements, rest } => {
                self.open("array-pattern")?;
                for element in elements {
                    match element {
                        Some(p) => self.pattern(p)?,
                        None => self.atom("_"),
                    }
                }
                if let Some(r) = rest {
                    self.open("...")?;
                    self.pattern(r)?;
                    self.close();
                }
                self.close();
            }
            PatternKind::Assignment { target, default } => {
                self.open("default")?;
                self.pattern(target)?;
                self.expression(default)?;
                self.close();
            }
        }
        Ok(())
    }

    fn opt_expression(&mut self, e: Option<&Expression>) -> DumpResult {
        match e {
            Some(e) => self.expression(e),
            None => {
                self.atom("_");
                Ok(())
            }
        }
    }

    fn arguments(&mut self, arguments: &[Argument]) -> DumpResult {
        for a in arguments {
            match a {
                Argument::Expression(e) => self.expression(e)?,
                Argument::Spread(e) => {
                    self.open("...")?;
                    self.expression(e)?;
                    self.close();
                }
            }
        }
        Ok(())
    }

    fn member_property(&mut self, property: &MemberProperty) -> DumpResult {
        match property {
            MemberProperty::Name(id) => self.atom(&id.name),
            MemberProperty::Private(id) => self.atom(&format!("#{}", id.name)),
            MemberProperty::Computed(e) => {
                self.open("computed")?;
                self.expression(e)?;
                self.close();
            }
        }
        Ok(())
    }

    fn template(&mut self, head: &str, t: &Template) -> DumpResult {
        self.open(head)?;
        for (i, quasi) in t.quasis.iter().enumerate() {
            match &quasi.cooked {
                Some(cooked) => self.atom(&utf16(cooked)),
                None => self.atom("undefined"),
            }
            if let Some(e) = t.expressions.get(i) {
                self.expression(e)?;
            }
        }
        Ok(())
    }

    fn expression(&mut self, e: &Expression) -> DumpResult {
        use ExpressionKind as E;
        match &e.kind {
            E::This => self.atom("this"),
            E::Identifier(id) => self.atom(&id.name),
            E::Null => self.atom("null"),
            E::Boolean(b) => self.atom(if *b { "true" } else { "false" }),
            E::Number(n) => self.atom(&number(*n)),
            E::BigInt(radix, digits) => self.atom(&bigint(*radix, digits)),
            E::String(s) => self.atom(&utf16(s)),
            E::RegExp { body, flags } => {
                self.atom(&format!("/{}/{flags}", String::from_utf16_lossy(body)));
            }
            E::Template(t) => {
                self.template("template", t)?;
                self.close();
            }
            E::TaggedTemplate { tag, template } => {
                self.open("tagged")?;
                self.expression(tag)?;
                self.template("template", template)?;
                self.close();
                self.close();
            }
            E::Array(elements) => {
                self.open("array")?;
                for element in elements {
                    match element {
                        None => self.atom("_"),
                        Some(ArrayElement::Expression(e)) => self.expression(e)?,
                        Some(ArrayElement::Spread(e)) => {
                            self.open("...")?;
                            self.expression(e)?;
                            self.close();
                        }
                    }
                }
                self.close();
            }
            E::Object(properties) => {
                self.open("object")?;
                for p in properties {
                    match p {
                        ObjectProperty::Property {
                            key,
                            value,
                            shorthand,
                            ..
                        } => {
                            if *shorthand {
                                self.expression(value)?;
                            } else {
                                self.open(":")?;
                                self.key(key)?;
                                self.expression(value)?;
                                self.close();
                            }
                        }
                        ObjectProperty::Method {
                            key,
                            function,
                            kind,
                            ..
                        } => {
                            self.open("method")?;
                            self.method_kind(*kind);
                            self.key(key)?;
                            self.function("function", function)?;
                            self.close();
                        }
                        ObjectProperty::Spread(e) => {
                            self.open("...")?;
                            self.expression(e)?;
                            self.close();
                        }
                    }
                }
                self.close();
            }
            E::Function(f) => self.function("function", f)?,
            E::Arrow(a) => {
                self.open(if a.is_async { "async-arrow" } else { "arrow" })?;
                self.params(&a.params, a.rest.as_ref())?;
                match &a.body {
                    ArrowBody::Expression(e) => self.expression(e)?,
                    ArrowBody::Block(body) => self.statements("body", body)?,
                }
                self.close();
            }
            E::Class(c) => self.class(c)?,
            E::Unary { op, argument } => {
                self.open(match op {
                    UnaryOp::Minus => "-",
                    UnaryOp::Plus => "+",
                    UnaryOp::Not => "!",
                    UnaryOp::BitNot => "~",
                    UnaryOp::Typeof => "typeof",
                    UnaryOp::Void => "void",
                    UnaryOp::Delete => "delete",
                })?;
                self.expression(argument)?;
                self.close();
            }
            E::Update { op, prefix, target } => {
                let op = match op {
                    UpdateOp::Increment => "++",
                    UpdateOp::Decrement => "--",
                };
                self.open(&if *prefix {
                    format!("{op}prefix")
                } else {
                    format!("{op}postfix")
                })?;
                self.pattern(target)?;
                self.close();
            }
            E::Binary { op, left, right } => {
                self.open(binary_name(*op))?;
                self.expression(left)?;
                self.expression(right)?;
                self.close();
            }
            E::PrivateIn { name, right } => {
                self.open("in")?;
                self.atom(&format!("#{}", name.name));
                self.expression(right)?;
                self.close();
            }
            E::Logical { op, left, right } => {
                self.open(match op {
                    LogicalOp::And => "&&",
                    LogicalOp::Or => "||",
                    LogicalOp::Coalesce => "??",
                })?;
                self.expression(left)?;
                self.expression(right)?;
                self.close();
            }
            E::Conditional {
                test,
                consequent,
                alternate,
            } => {
                self.open("?")?;
                self.expression(test)?;
                self.expression(consequent)?;
                self.expression(alternate)?;
                self.close();
            }
            E::Assignment { op, target, value } => {
                self.open(assign_name(*op))?;
                self.pattern(target)?;
                self.expression(value)?;
                self.close();
            }
            E::Sequence(items) => {
                self.open(",")?;
                for item in items {
                    self.expression(item)?;
                }
                self.close();
            }
            E::Member {
                object,
                property,
                optional,
            } => {
                self.open(if *optional { "?." } else { "." })?;
                self.expression(object)?;
                self.member_property(property)?;
                self.close();
            }
            E::SuperMember(property) => {
                self.open(".")?;
                self.atom("super");
                self.member_property(property)?;
                self.close();
            }
            E::Call {
                callee,
                arguments,
                optional,
            } => {
                self.open(if *optional { "?.call" } else { "call" })?;
                self.expression(callee)?;
                self.arguments(arguments)?;
                self.close();
            }
            E::SuperCall(arguments) => {
                self.open("call")?;
                self.atom("super");
                self.arguments(arguments)?;
                self.close();
            }
            E::New { callee, arguments } => {
                self.open("new")?;
                self.expression(callee)?;
                self.arguments(arguments)?;
                self.close();
            }
            E::OptionalChain(inner) => {
                self.open("chain")?;
                self.expression(inner)?;
                self.close();
            }
            E::Parenthesized(inner) => {
                self.open("paren")?;
                self.expression(inner)?;
                self.close();
            }
            E::NewTarget => self.atom("new.target"),
            E::ImportMeta => self.atom("import.meta"),
            E::ImportCall { source, options } => {
                self.open("import")?;
                self.expression(source)?;
                if let Some(o) = options {
                    self.expression(o)?;
                }
                self.close();
            }
            E::Yield { argument, delegate } => {
                self.open(if *delegate { "yield*" } else { "yield" })?;
                if let Some(a) = argument {
                    self.expression(a)?;
                }
                self.close();
            }
            E::Await(argument) => {
                self.open("await")?;
                self.expression(argument)?;
                self.close();
            }
        }
        Ok(())
    }
}

fn bigint(radix: u32, digits: &str) -> String {
    match radix {
        16 => format!("0x{digits}n"),
        8 => format!("0o{digits}n"),
        2 => format!("0b{digits}n"),
        _ => format!("{digits}n"),
    }
}

fn binary_name(op: BinaryOp) -> &'static str {
    use BinaryOp as B;
    match op {
        B::Eq => "==",
        B::NotEq => "!=",
        B::StrictEq => "===",
        B::StrictNotEq => "!==",
        B::Lt => "<",
        B::LtEq => "<=",
        B::Gt => ">",
        B::GtEq => ">=",
        B::Shl => "<<",
        B::Sar => ">>",
        B::Shr => ">>>",
        B::Add => "+",
        B::Sub => "-",
        B::Mul => "*",
        B::Div => "/",
        B::Rem => "%",
        B::Exp => "**",
        B::BitOr => "|",
        B::BitXor => "^",
        B::BitAnd => "&",
        B::In => "in",
        B::Instanceof => "instanceof",
    }
}

fn assign_name(op: AssignOp) -> &'static str {
    use AssignOp as A;
    match op {
        A::Assign => "=",
        A::Add => "+=",
        A::Sub => "-=",
        A::Mul => "*=",
        A::Div => "/=",
        A::Rem => "%=",
        A::Exp => "**=",
        A::Shl => "<<=",
        A::Sar => ">>=",
        A::Shr => ">>>=",
        A::BitOr => "|=",
        A::BitXor => "^=",
        A::BitAnd => "&=",
        A::And => "&&=",
        A::Or => "||=",
        A::Coalesce => "??=",
    }
}
