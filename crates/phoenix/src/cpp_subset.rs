use cachet_lang::parser::Item;
use clang::{Entity, EntityKind, TypeKind};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug)]
pub struct Pos {
    pub line: u32,
    pub column: u32,
    pub offset: u32,
}

/// Where a node came from, as the half-open region `[start, end)`. `file` is
/// absolute, so a span outlives the chdir in `parse`.
#[derive(Clone, Debug)]
pub enum Span {
    /// clang reports no range, which happens for some implicit nodes.
    Unknown,
    Known {
        file: PathBuf,
        start: Pos,
        end: Pos,
    },
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Span::Unknown => write!(f, "<unknown>"),
            Span::Known { file, start, .. } => {
                let name = file.file_name().unwrap_or(file.as_os_str());
                write!(f, "{}:{}:{}", name.to_string_lossy(), start.line, start.column)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Spanned<T> {
    pub span: Span,
    pub value: T,
}

impl<T> Spanned<T> {
    pub fn new(span: Span, value: T) -> Self {
        Spanned { span, value }
    }
}

/// An expression, with the span and the type clang gave it.
///
/// The type sits on the node rather than on the variants because that is how clang
/// hands it over -- `clang_getCursorType` answers for any expression cursor, not
/// just a name. Per-variant fields would make "does this have a type" an accident of
/// which shapes someone happened to need, which is how a chained receiver like
/// `output.typedReg().gpr()` came to have no type to look up.
#[derive(Clone, Debug)]
pub struct TypedExpr {
    pub span: Span,
    pub value: Expr,
    /// `None` only when the type is outside the subset, never because nothing
    /// recorded it. `failure->label()` is a `Label*`, which the subset declines to
    /// model, and translates fine all the same.
    pub ty: Option<Type>,
}

/// A "small C++": the subset of the clang AST a CacheIR stub generator body
/// actually uses, with the implicit-conversion scaffolding already stripped.
/// Everything here is reachable from inside a body, so there are no
/// declaration nodes yet.
#[derive(Clone, Debug)]
pub struct CompoundStmt {
    pub stmts: Vec<Spanned<Stmt>>,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    If(IfStmt),
    Let(LetStmt),
    Return(ReturnStmt),
    /// `MOZ_ASSERT(cond)`, recovered from its `do { ... } while (0)` expansion.
    /// Kept rather than dropped as macro noise: this is what becomes `assume`.
    Assert(AssertStmt),
    Switch(SwitchStmt),
    /// `MOZ_CRASH("reason")`: this point is not reached.
    Crash(CrashStmt),
    /// An expression in statement position. libclang emits no wrapper node for
    /// these; the `CallExpr` hangs directly off the enclosing `CompoundStmt`.
    ///
    /// The whole node, not a bare [`Expr`]: a statement expression has a type like
    /// any other, and discarding it would mean rebuilding one at the use site.
    Expr(TypedExpr),
}

/// `switch (op) { case Eq: case StrictEq: return Equal; .. default: .. }`.
///
/// Cachet has no switch, so this becomes an if-chain -- faithful only because
/// [`extract_switch`] refuses a case body that would fall through to the next.
#[derive(Clone, Debug)]
pub struct SwitchStmt {
    pub scrutinee: TypedExpr,
    pub cases: Vec<Case>,
    /// `None` when the switch has no `default:`.
    pub default: Option<CompoundStmt>,
}

/// One case body, with every label that shares it.
#[derive(Clone, Debug)]
pub struct Case {
    /// `case Eq: case StrictEq:` is two values against one body.
    pub values: Vec<TypedExpr>,
    /// Not a C++ block -- a case body without braces shares the switch's scope --
    /// but the same list of statements, and what the translator already consumes.
    pub body: CompoundStmt,
}

/// `MOZ_CRASH("Unrecognized comparison operation")`.
#[derive(Clone, Debug)]
pub struct CrashStmt {
    /// The message, kept for the comment it becomes. `None` if the expansion held
    /// no string literal.
    pub reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct IfStmt {
    pub cond: TypedExpr,
    pub then: CompoundStmt,
    pub els: Option<CompoundStmt>,
}

/// A `DeclStmt` wrapping a single `VarDecl`: `NumberOperandId lhs = ...`.
#[derive(Clone, Debug)]
pub struct LetStmt {
    pub name: String,
    pub ty: Type,
    pub init: Option<TypedExpr>,
}

#[derive(Clone, Debug)]
pub struct ReturnStmt {
    pub value: Option<TypedExpr>,
}

#[derive(Clone, Debug)]
pub struct AssertStmt {
    /// `MOZ_ASSERT_IF`'s first argument: the assertion holds only where this
    /// does. `None` for a plain `MOZ_ASSERT`.
    pub guard: Option<TypedExpr>,
    pub cond: TypedExpr,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Call(Call),
    Construct(Construct),
    Unary(UnaryOp),
    Binary(BinaryOp),
    Ref(Ref),
    EnumConst(EnumConst),
    Lit(Lit),
    /// `this`. Its type is a pointer to the enclosing class, so it usually
    /// appears under a dereference: `*this`.
    This,
}

/// Construction of an object: `AutoAvailableFloatRegister(*this, FloatReg0)`.
///
/// Not a [`Call`]: there is no callee, only a type, and libclang gives a
/// constructor's `CallExpr` no callee child -- its children are the arguments.
/// Clang's own AST agrees, modelling this as `CXXConstructExpr`.
///
/// Compiler-inserted copies never reach here; [`is_implicit_conversion`] peels
/// them first.
#[derive(Clone, Debug)]
pub struct Construct {
    pub ty: Type,
    pub args: Vec<TypedExpr>,
}

#[derive(Clone, Debug)]
pub struct Call {
    pub callee: Callee,
    pub args: Vec<TypedExpr>,
}

/// clang's Unified Symbol Resolution. Separates overloads, and encodes the file
/// for a `static`, so same-named statics don't collide.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FnId(pub String);

/// What a call resolved to. `name` keys a translation table, `id` finds the
/// definition in [`Callees`].
#[derive(Clone, Debug)]
pub struct FnRef {
    pub name: String,
    pub id: FnId,
}

#[derive(Clone, Debug)]
pub enum Callee {
    /// A free function, e.g. `EmitGuardToDoubleForToNumber`.
    Free(FnRef),
    /// A method, with what it was called on. `recv` is `None` for an implicit
    /// `this`, as in `trackAttached("..")`.
    ///
    /// LIBCLANG: an implicit `this` receiver is absent from the tree, hence the
    /// `Option` -- clang's own AST has a `CXXThisExpr` there.
    Method {
        recv: Option<Box<TypedExpr>>,
        callee: FnRef,
    },
}

#[derive(Clone, Debug)]
pub struct UnaryOp {
    pub op: String,
    pub operand: Box<TypedExpr>,
}

#[derive(Clone, Debug)]
pub struct BinaryOp {
    pub op: String,
    pub lhs: Box<TypedExpr>,
    pub rhs: Box<TypedExpr>,
}

/// A name in expression position, resolved to what it refers to.
#[derive(Clone, Debug)]
pub struct Ref {
    pub kind: RefKind,
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// A field on the generator: `op_`, `lhsVal_`, `rhsVal_`, `writer`.
    Field,
    /// A parameter of the generator method: `lhsId`, `rhsId`.
    Param,
    /// A local introduced by a `LetStmt`: `lhs`, `rhs`.
    Local,
    /// A variable declared outside the callable: a namespace-scope global, or a
    /// class-scope `static` like `Assembler::NotEqual`.
    ///
    /// Distinguished from [`RefKind::Local`] because the name alone doesn't
    /// carry the scope it came from. Emitting one unqualified would put it in
    /// whatever scope the generated code has, where it means nothing -- or,
    /// worse, binds to a local that happens to share the name.
    Global,
}

/// A C++ type, with the structure the name only renders.
///
/// `HandleValue`, the type of `lhsVal_`, is:
///
/// ```text
/// Type {
///     scope: ["JS", "Handle"],
///     args: [Type { scope: ["JS", "Value"], .. }],
///     indirection: Value,
///     is_const: false,
///     spelled: "HandleValue",
/// }
/// ```
///
/// `scope` and `args` come from the canonical type, so typedefs are seen
/// through and namespaces are explicit -- a translation table keys on those.
/// `spelled` keeps the source's own words for dumps.
#[derive(Clone, Debug)]
pub struct Type {
    /// Namespace path and the type's own name: `["JS", "Handle"]`. A builtin
    /// has no declaration, so it is a single element: `["bool"]`.
    pub scope: Vec<String>,
    /// Template arguments, empty for a non-generic type.
    pub args: Vec<Type>,
    pub indirection: Indirection,
    /// Constness of the value, or of the pointee for a reference: `true` for
    /// both `const Value` and `const Value &`.
    pub is_const: bool,
    /// The type as written, typedef intact: `HandleValue`.
    pub spelled: String,
}

/// Whether a name denotes a value or stands in for one. Constness is separate,
/// so `const T&` is `Ref` with `is_const`, not a kind of its own. Rvalue
/// references are not modeled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indirection {
    Value,
    Ref,
    /// `T*`. Unlike a reference it can be null and can be reseated.
    Ptr,
}

/// `JSOp::StrictEq`, `AttachDecision::Attach`.
#[derive(Clone, Debug)]
pub struct EnumConst {
    pub ty: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub enum Lit {
    Str(String),
    Int(i64),
    Double(f64),
    Bool(bool),
}

/// A read-only traversal of a body. Each method defaults to descending, so an
/// implementation overrides only the nodes it cares about; an override that
/// still wants to descend calls the matching `walk_*` itself.
pub trait Visit {
    fn visit_block(&mut self, b: &CompoundStmt) {
        walk_block(self, b);
    }
    fn visit_stmt(&mut self, s: &Stmt) {
        walk_stmt(self, s);
    }
    fn visit_expr(&mut self, e: &Expr) {
        walk_expr(self, e);
    }
    fn visit_call(&mut self, c: &Call) {
        walk_call(self, c);
    }
    /// A leaf, so the default does nothing rather than descending.
    fn visit_ref(&mut self, _r: &Ref) {}
}

// `?Sized` so a default method can pass its `&mut Self` here: inside a trait,
// `Self` is not known to be sized (it could be `dyn Visit`).

pub fn walk_block<V: Visit + ?Sized>(v: &mut V, block: &CompoundStmt) {
    for stmt in &block.stmts {
        v.visit_stmt(&stmt.value);
    }
}

pub fn walk_stmt<V: Visit + ?Sized>(v: &mut V, stmt: &Stmt) {
    match stmt {
        Stmt::If(s) => {
            v.visit_expr(&s.cond.value);
            v.visit_block(&s.then);
            if let Some(els) = &s.els {
                v.visit_block(els);
            }
        }
        Stmt::Let(s) => {
            if let Some(init) = &s.init {
                v.visit_expr(&init.value);
            }
        }
        Stmt::Return(s) => {
            if let Some(value) = &s.value {
                v.visit_expr(&value.value);
            }
        }
        Stmt::Assert(s) => {
            if let Some(guard) = &s.guard {
                v.visit_expr(&guard.value);
            }
            v.visit_expr(&s.cond.value);
        }
        Stmt::Switch(s) => {
            v.visit_expr(&s.scrutinee.value);
            for case in &s.cases {
                for value in &case.values {
                    v.visit_expr(&value.value);
                }
                v.visit_block(&case.body);
            }
            if let Some(default) = &s.default {
                v.visit_block(default);
            }
        }
        // No subexpressions: the message is a string literal, not modeled.
        Stmt::Crash(_) => {}
        Stmt::Expr(e) => v.visit_expr(&e.value),
    }
}

pub fn walk_expr<V: Visit + ?Sized>(v: &mut V, expr: &Expr) {
    match expr {
        Expr::Call(c) => v.visit_call(c),
        Expr::Construct(c) => {
            for arg in &c.args {
                v.visit_expr(&arg.value);
            }
        }
        Expr::Unary(u) => v.visit_expr(&u.operand.value),
        Expr::Binary(b) => {
            v.visit_expr(&b.lhs.value);
            v.visit_expr(&b.rhs.value);
        }
        Expr::Ref(r) => v.visit_ref(r),
        // Leaves.
        Expr::EnumConst(_) | Expr::Lit(_) | Expr::This => {}
    }
}

pub fn walk_call<V: Visit + ?Sized>(v: &mut V, call: &Call) {
    // The receiver is an expression like any other: `v.isNumber()` reads `v`.
    if let Callee::Method { recv: Some(recv), .. } = &call.callee {
        v.visit_expr(&recv.value);
    }
    for arg in &call.args {
        v.visit_expr(&arg.value);
    }
}

/// Where an unsupported construct was found, for error reporting.
#[derive(Clone, Debug)]
pub struct Loc {
    pub file: String,
    pub line: u32,
}

impl fmt::Display for Loc {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

/// A C++ construct outside the subset we model, found while extracting it.
///
/// The first of the two ways translation can stop, and the messages say which:
/// this one means the C++ never made it into [`Stmt`]/[`Expr`] at all, where an
/// "unhandled" [`Unhandled`](crate::cpp_to_cachet::Unhandled) means it did and has
/// no Cachet counterpart. Every variant names what was found and where, so a
/// failure reports the line that defeated it rather than failing anonymously.
#[derive(Clone, Debug)]
pub enum Unsupported {
    Stmt {
        kind: EntityKind,
        loc: Loc,
    },
    Expr {
        kind: EntityKind,
        loc: Loc,
    },
    /// A macro expansion with no policy entry. Its expansion is in the AST but
    /// its meaning isn't, so translating it would be guesswork.
    Macro {
        name: String,
        loc: Loc,
    },
    /// A call whose callee is neither a plain function nor a method.
    Callee {
        name: String,
        loc: Loc,
    },
    /// A type outside the subset: a pointer or an rvalue reference.
    Type {
        spelled: String,
        loc: Loc,
    },
    /// A node of a modeled kind whose children aren't shaped as expected.
    Malformed {
        what: String,
        loc: Loc,
    },
}

impl fmt::Display for Unsupported {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Unsupported::Stmt { kind, loc } => {
                write!(f, "{loc}: statement {kind:?} is outside the C++ subset")
            }
            Unsupported::Expr { kind, loc } => {
                write!(f, "{loc}: expression {kind:?} is outside the C++ subset")
            }
            Unsupported::Macro { name, loc } => {
                write!(f, "{loc}: macro `{name}` is outside the C++ subset")
            }
            Unsupported::Callee { name, loc } => {
                write!(f, "{loc}: callee `{name}` is outside the C++ subset")
            }
            Unsupported::Type { spelled, loc } => {
                write!(f, "{loc}: type `{spelled}` is outside the C++ subset")
            }
            Unsupported::Malformed { what, loc } => write!(f, "{loc}: {what}"),
        }
    }
}

impl std::error::Error for Unsupported {}

pub type Result<T> = std::result::Result<T, Unsupported>;

/// Why a definition couldn't be extracted. Separates "this isn't something we
/// extract" from "this uses C++ we don't model": the first means the caller
/// pointed at the wrong entity, the second is a gap in the subset. `Body`
/// names the unit it came from so a sweep over many of them stays readable.
#[derive(Clone, Debug)]
pub enum Error {
    Signature {
        what: String,
        loc: Loc,
    },
    Body {
        /// `CompareIRGenerator::tryAttachNumber`, or a bare function name.
        unit: String,
        cause: Unsupported,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Signature { what, loc } => write!(f, "{loc}: {what}"),
            Error::Body { unit, cause } => write!(f, "{unit}: {cause}"),
        }
    }
}

impl Error {
    /// The message for a failure the caller asked about by name.
    ///
    /// The unit that failed is not always the one requested -- a generator's
    /// translation descends into its callees -- so it is worth naming, except when
    /// it *is* the one requested and naming it twice says nothing.
    pub fn report(&self, requested: &str) -> String {
        match self {
            Error::Body { unit, cause } if unit == requested => {
                format!("cannot translate {unit}: {cause}")
            }
            _ => format!("cannot translate {requested}: {self}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Signature { .. } => None,
            Error::Body { cause, .. } => Some(cause),
        }
    }
}

/// The C++ an entity came from.
///
/// The path is canonicalized so it stays meaningful after the process leaves
/// the compile directory: a compile database records the main file relative to
/// the build directory, and `parse` chdirs there.
fn span_of(e: Entity) -> Span {
    let Some(range) = e.get_range() else {
        return Span::Unknown;
    };
    let start = range.get_start().get_file_location();
    let end = range.get_end().get_file_location();
    let Some(file) = start.file.map(|f| f.get_path()) else {
        return Span::Unknown;
    };

    let pos = |l: clang::source::Location| Pos {
        line: l.line,
        column: l.column,
        offset: l.offset,
    };
    Span::Known {
        file: std::fs::canonicalize(&file).unwrap_or(file),
        start: pos(start),
        end: pos(end),
    }
}

fn loc(e: Entity) -> Loc {
    let Some(l) = e.get_location() else {
        return Loc {
            file: String::from("<unknown>"),
            line: 0,
        };
    };
    let l = l.get_file_location();
    Loc {
        file: l
            .file
            .map(|f| f.get_path())
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_default(),
        line: l.line,
    }
}

/// A node that came from a macro has no tokens of its own: its source range
/// lies inside the expansion, so `tokenize` yields nothing.
///
/// LIBCLANG: no `is_in_macro_expansion`, so emptiness of the token range stands
/// in for it.
fn is_macro_expansion(e: Entity) -> bool {
    e.get_range()
        .map(|r| r.tokenize().is_empty())
        .unwrap_or(false)
}

/// Read source text at a location, `len` characters wide.
fn text_at(l: clang::source::Location, len: usize) -> Option<String> {
    let text = l.file?.get_contents()?;
    let line = text.lines().nth(l.line.checked_sub(1)? as usize)?;
    Some(
        line.chars()
            .skip(l.column.checked_sub(1)? as usize)
            .take(len)
            .collect(),
    )
}

/// The macro that produced `e`, read from the invocation site. A cursor's
/// file location is its *expansion* location, i.e. where the macro was written.
///
/// LIBCLANG: macro expansions carry no name, so the source text is re-read.
fn macro_name(e: Entity) -> Option<String> {
    let name: String = text_at(e.get_location()?.get_file_location(), 64)?
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// LIBCLANG: `UnaryOperator` carries no spelling (unlike `BinaryOperator`), so
/// it is read from source -- tokens normally, and inside a macro expansion the
/// spelling location, which points at the operator in the macro definition.
fn operator_spelling(e: Entity) -> Option<String> {
    if let Some(tok) = e
        .get_range()
        .map(|r| r.tokenize())
        .and_then(|t| t.first().map(|t| t.get_spelling()))
    {
        return Some(tok);
    }
    text_at(e.get_location()?.get_spelling_location(), 1)
}

/// Peel off the implicit nodes clang inserts: casts (`UnexposedExpr`),
/// parentheses, implicit copy constructors and conversion operators. Reading
/// `lhsVal_` costs four such wrappers before reaching the `FieldDecl`.
///
/// LIBCLANG: every implicit cast is an anonymous `UnexposedExpr` with no cast
/// kind, so they are peeled by shape rather than by what they do.
fn strip(mut e: Entity) -> Entity {
    loop {
        let peel = match e.get_kind() {
            EntityKind::UnexposedExpr | EntityKind::ParenExpr => true,
            EntityKind::CallExpr | EntityKind::MemberRefExpr => is_implicit_conversion(e),
            _ => false,
        };
        match e.get_children().as_slice() {
            [only] if peel => e = *only,
            _ => return e,
        }
    }
}

/// Whether a constructor or conversion call was inserted by the compiler
/// rather than written down.
///
/// LIBCLANG: nothing marks a node as implicit, so source extent decides.
///
/// Both look the same structurally -- a `CallExpr` resolving to a `Constructor`
/// with one argument -- so kind alone can't tell `AutoOutputRegister output(*this)`
/// (meaningful) from the implicit copy around `lhsId` (noise). What separates
/// them is source extent: an implicit call spans exactly its argument, while a
/// written one also covers the type name and parentheses.
fn is_implicit_conversion(e: Entity) -> bool {
    if !matches!(
        e.get_reference().map(|r| r.get_kind()),
        Some(EntityKind::Constructor | EntityKind::ConversionFunction)
    ) {
        return false;
    }
    let (Some(outer), Some(child)) = (
        e.get_range(),
        e.get_children().first().and_then(|c| c.get_range()),
    ) else {
        return false;
    };
    let extent = |r: clang::source::SourceRange| {
        let (s, e) = (
            r.get_start().get_file_location(),
            r.get_end().get_file_location(),
        );
        (s.line, s.column, e.line, e.column)
    };
    extent(outer) == extent(child)
}

/// A declaration's type.
fn extract_type(decl: Entity) -> Result<Type> {
    let ty = decl.get_type().ok_or_else(|| Unsupported::Malformed {
        what: format!("`{}` has no type", decl.get_name().unwrap_or_default()),
        loc: loc(decl),
    })?;
    type_of(ty, decl)
}

/// Pointers and rvalue references are refused rather than approximated: `T*`
/// adds nullability and `T&&` move semantics, neither of which we model.
fn type_of(ty: clang::Type, at: Entity) -> Result<Type> {
    let spelled = ty.get_display_name();
    let canonical = ty.get_canonical_type();

    // Look through a reference to describe what it refers to; the indirection
    // is recorded separately.
    let (indirection, referent) = match canonical.get_pointee_type() {
        None => (Indirection::Value, canonical),
        Some(pointee) => match canonical.get_kind() {
            TypeKind::LValueReference => (Indirection::Ref, pointee),
            TypeKind::Pointer => (Indirection::Ptr, pointee),
            // RValueReference and the exotic pointer kinds.
            _ => {
                return Err(Unsupported::Type {
                    spelled,
                    loc: loc(at),
                });
            }
        },
    };

    // A builtin (`bool`, `int`) has no declaration to take a scope from, so its
    // own name stands in for one.
    let scope = match referent.get_declaration() {
        Some(decl) => scope_path(decl),
        None => vec![
            referent
                .get_display_name()
                .trim_start_matches("const ")
                .to_string(),
        ],
    };

    let args = referent
        .get_template_argument_types()
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .map(|arg| type_of(arg, at))
        .collect::<Result<Vec<_>>>()?;

    Ok(Type {
        scope,
        args,
        indirection,
        is_const: referent.is_const_qualified(),
        spelled,
    })
}

/// Enclosing namespaces and classes, outermost first, ending with the entity's
/// own name: `["JS", "Handle"]`.
fn scope_path(mut e: Entity) -> Vec<String> {
    let mut path = vec![e.get_name().unwrap_or_default()];
    while let Some(parent) = e.get_semantic_parent() {
        match parent.get_kind() {
            EntityKind::Namespace | EntityKind::ClassDecl | EntityKind::StructDecl => {
                path.push(parent.get_name().unwrap_or_default());
                e = parent;
            }
            _ => break,
        }
    }
    path.reverse();
    path
}

fn is_type_ref(e: Entity) -> bool {
    matches!(
        e.get_kind(),
        EntityKind::TypeRef | EntityKind::NamespaceRef | EntityKind::TemplateRef
    )
}

/// Extract a generator body into the modeled subset. The kind is checked
/// here so this is safe to call on any entity.
pub fn extract_compound_stmt(body: Entity) -> Result<CompoundStmt> {
    if body.get_kind() != EntityKind::CompoundStmt {
        return Err(Unsupported::Malformed {
            what: format!("expected a CompoundStmt, found {:?}", body.get_kind()),
            loc: loc(body),
        });
    }
    extract_block(body)
}

fn extract_block(block: Entity) -> Result<CompoundStmt> {
    let mut stmts = Vec::new();
    for child in block.get_children() {
        stmts.extend(extract_stmt(child)?);
    }
    Ok(CompoundStmt { stmts })
}

/// One C++ statement can yield several: a `DeclStmt` declaring more than one
/// variable becomes one [`Stmt::Let`] per declarator.
fn extract_stmt(e: Entity) -> Result<Vec<Spanned<Stmt>>> {
    // Statements split from one `DeclStmt` share its span; there is no narrower
    // region to attribute each declarator to.
    let span = span_of(e);
    let stmts = extract_stmt_values(e)?;
    Ok(stmts
        .into_iter()
        .map(|stmt| Spanned::new(span.clone(), stmt))
        .collect())
}

fn extract_stmt_values(e: Entity) -> Result<Vec<Stmt>> {
    if is_macro_expansion(e) {
        return Ok(vec![extract_macro(e)?]);
    }

    match e.get_kind() {
        EntityKind::IfStmt => Ok(vec![Stmt::If(extract_if(e)?)]),
        EntityKind::DeclStmt => Ok(extract_decls(e)?.into_iter().map(Stmt::Let).collect()),
        EntityKind::ReturnStmt => {
            let value = match e.get_children().as_slice() {
                [] => None,
                [v] => Some(extract_expr(*v)?),
                _ => {
                    return Err(Unsupported::Malformed {
                        what: String::from("return with multiple children"),
                        loc: loc(e),
                    });
                }
            };
            Ok(vec![Stmt::Return(ReturnStmt { value })])
        }
        EntityKind::SwitchStmt => Ok(vec![Stmt::Switch(extract_switch(e)?)]),
        // Loops and jumps are outside the subset. Naming them as statements is
        // clearer than letting them fall to the expression path.
        EntityKind::CompoundStmt
        | EntityKind::ForStmt
        | EntityKind::WhileStmt
        | EntityKind::DoStmt
        | EntityKind::BreakStmt
        | EntityKind::ContinueStmt
        | EntityKind::GotoStmt => Err(Unsupported::Stmt {
            kind: e.get_kind(),
            loc: loc(e),
        }),
        // Anything else in statement position is an expression statement.
        _ => Ok(vec![Stmt::Expr(extract_expr(e)?)]),
    }
}

/// `switch (op) { .. }` into the subset.
///
/// Two things about clang's shape drive this. Labels sharing a body nest --
/// `case Eq: case StrictEq: return x;` is `CaseStmt(Eq, CaseStmt(StrictEq, Ret))`
/// -- so grouping is a matter of peeling, and no notion of fall-through is needed
/// for it. And only a body's *first* statement hangs under its label; the rest are
/// siblings in the switch's block, so a body runs until the next label.
///
/// Fall-through proper is refused: every case body must end in a `return` or a
/// crash, since an if-chain would not fall into the next case. That is stricter
/// than C++ requires -- the last case may fall out of the switch harmlessly -- but
/// being wrong here would change what the code does silently.
fn extract_switch(e: Entity) -> Result<SwitchStmt> {
    let kids = e.get_children();
    let [scrutinee, block] = kids.as_slice() else {
        return Err(Unsupported::Malformed {
            what: format!("switch with {} children", kids.len()),
            loc: loc(e),
        });
    };
    if block.get_kind() != EntityKind::CompoundStmt {
        return Err(Unsupported::Malformed {
            what: format!("switch body is a {:?}", block.get_kind()),
            loc: loc(*block),
        });
    }
    let scrutinee = extract_expr(*scrutinee)?;

    let mut cases: Vec<Case> = Vec::new();
    let mut default: Option<CompoundStmt> = None;
    // Which body the statements being read belong to: the label most recently
    // opened.
    let mut in_default = false;

    for child in block.get_children() {
        match child.get_kind() {
            EntityKind::CaseStmt => {
                in_default = false;
                let (values, first) = peel_case(child)?;
                let mut body = CompoundStmt { stmts: Vec::new() };
                if let Some(first) = first {
                    body.stmts.extend(extract_stmt(first)?);
                }
                cases.push(Case { values, body });
            }
            EntityKind::DefaultStmt => {
                in_default = true;
                if default.is_some() {
                    return Err(Unsupported::Malformed {
                        what: String::from("switch with two defaults"),
                        loc: loc(child),
                    });
                }
                let mut body = CompoundStmt { stmts: Vec::new() };
                for inner in child.get_children() {
                    body.stmts.extend(extract_stmt(inner)?);
                }
                default = Some(body);
            }
            // Not a label, so it continues the body of the last one opened.
            _ => {
                let stmts = extract_stmt(child)?;
                let body = if in_default {
                    default.as_mut().map(|body| &mut body.stmts)
                } else {
                    cases.last_mut().map(|case| &mut case.body.stmts)
                };
                match body {
                    Some(body) => body.extend(stmts),
                    None => {
                        return Err(Unsupported::Malformed {
                            what: String::from("statement before the switch's first label"),
                            loc: loc(child),
                        });
                    }
                }
            }
        }
    }

    for case in &cases {
        if !ends_execution(&case.body.stmts) {
            return Err(Unsupported::Malformed {
                what: String::from("case body falls through, which an if-chain would not"),
                loc: loc(e),
            });
        }
    }

    Ok(SwitchStmt {
        scrutinee,
        cases,
        default,
    })
}

/// The labels sharing one body, and the first statement of that body.
fn peel_case(case: Entity) -> Result<(Vec<TypedExpr>, Option<Entity>)> {
    let mut case = case;
    let mut values = Vec::new();
    loop {
        let kids = case.get_children();
        let (value, rest) = match kids.as_slice() {
            [value] => (*value, None),
            [value, inner] => (*value, Some(*inner)),
            _ => {
                return Err(Unsupported::Malformed {
                    what: format!("case with {} children", kids.len()),
                    loc: loc(case),
                });
            }
        };
        values.push(extract_expr(value)?);
        match rest {
            // The next label of a shared body.
            Some(inner) if inner.get_kind() == EntityKind::CaseStmt => case = inner,
            other => return Ok((values, other)),
        }
    }
}

/// Whether a body cannot run off its end, so nothing follows it.
fn ends_execution(body: &[Spanned<Stmt>]) -> bool {
    matches!(
        body.last().map(|stmt| &stmt.value),
        Some(Stmt::Return(_) | Stmt::Crash(_))
    )
}

/// Macro expansions are recognized by name and translated per policy. Anything
/// unlisted is an error: its expansion is present but its intent isn't, and
/// guessing from the expansion's shape is how `TRY_ATTACH` would get silently
/// mistaken for an assertion.
fn extract_macro(e: Entity) -> Result<Stmt> {
    let name = macro_name(e).unwrap_or_else(|| String::from("<unknown>"));
    match name.as_str() {
        "MOZ_ASSERT" | "MOZ_RELEASE_ASSERT" | "MOZ_DIAGNOSTIC_ASSERT" => {
            Ok(Stmt::Assert(AssertStmt {
                guard: None,
                cond: assert_cond(e)?,
            }))
        }
        // `do { if (cond) { MOZ_ASSERT(expr); } } while (false)`
        "MOZ_ASSERT_IF" | "MOZ_DIAGNOSTIC_ASSERT_IF" => {
            let if_stmt = do_body(e)?
                .into_iter()
                .find(|c| c.get_kind() == EntityKind::IfStmt)
                .ok_or_else(|| Unsupported::Malformed {
                    what: format!("{name} without an inner if"),
                    loc: loc(e),
                })?;
            let kids = if_stmt.get_children();
            let [cond, then, ..] = kids.as_slice() else {
                return Err(Unsupported::Malformed {
                    what: format!("{name}'s if has {} children", kids.len()),
                    loc: loc(e),
                });
            };
            let inner = then
                .get_children()
                .into_iter()
                .find(|c| c.get_kind() == EntityKind::DoStmt)
                .ok_or_else(|| Unsupported::Malformed {
                    what: format!("{name} without an inner assertion"),
                    loc: loc(e),
                })?;
            Ok(Stmt::Assert(AssertStmt {
                guard: Some(extract_expr(*cond)?),
                cond: assert_cond(inner)?,
            }))
        }
        // `MOZ_CRASH("reason")` says control never gets here. Kept rather than
        // dropped, because "unreachable" is a claim the model can carry and check.
        "MOZ_CRASH" => Ok(Stmt::Crash(CrashStmt {
            reason: crash_reason(e),
        })),
        _ => Err(Unsupported::Macro { name, loc: loc(e) }),
    }
}

/// The message a `MOZ_CRASH` was given, from the first string literal in its
/// expansion. Best effort: it is only ever a comment.
fn crash_reason(e: Entity) -> Option<String> {
    let mut found = None;
    e.visit_children(|child, _| {
        if child.get_kind() == EntityKind::StringLiteral {
            found = child.get_display_name().map(|s| s.trim_matches('"').to_owned());
            return clang::EntityVisitResult::Break;
        }
        clang::EntityVisitResult::Recurse
    });
    found
}

/// The statements inside a macro's `do { ... } while (false)`.
fn do_body(e: Entity) -> Result<Vec<Entity>> {
    e.get_children()
        .into_iter()
        .find(|c| c.get_kind() == EntityKind::CompoundStmt)
        .map(|c| c.get_children())
        .ok_or_else(|| Unsupported::Malformed {
            what: String::from("macro expansion is not a do-block"),
            loc: loc(e),
        })
}

/// Recover `expr` from a `MOZ_ASSERT(expr)` expansion. The assertion's
/// condition is `MOZ_UNLIKELY(!MOZ_CHECK_ASSERT_ASSIGNMENT(expr))`
/// (`Assertions.h:535`), so the asserted expression sits under a chain of
/// macro-written negations and parens.
fn assert_cond(do_stmt: Entity) -> Result<TypedExpr> {
    let if_stmt = do_body(do_stmt)?
        .into_iter()
        .find(|c| c.get_kind() == EntityKind::IfStmt)
        .ok_or_else(|| Unsupported::Malformed {
            what: String::from("assertion without a check"),
            loc: loc(do_stmt),
        })?;
    let cond =
        if_stmt
            .get_children()
            .into_iter()
            .next()
            .ok_or_else(|| Unsupported::Malformed {
                what: String::from("assertion check without a condition"),
                loc: loc(do_stmt),
            })?;
    extract_expr(peel_assert_glue(cond, &loc(do_stmt).file))
}

/// Descend through the assertion macro's wrappers to the asserted expression.
///
/// LIBCLANG: macros leave no node of their own, so the expansion is walked.
///
/// Counting negations doesn't work: the glue contributes an odd number of `!`,
/// but `MOZ_ASSERT_IF(.., !IsEqualityOp(op_))` starts with a `!` of its own,
/// and peeling that one inverts the assertion. The discriminator is *where the
/// token was written*: glue is spelled inside `Assertions.h` / `Likely.h`,
/// while the asserted expression is spelled at the call site.
fn peel_assert_glue<'tu>(mut e: Entity<'tu>, user_file: &str) -> Entity<'tu> {
    loop {
        if spelling_file(e).as_deref() == Some(user_file) {
            return e;
        }
        let kids = e.get_children();
        match e.get_kind() {
            EntityKind::UnexposedExpr | EntityKind::ParenExpr | EntityKind::UnaryOperator => {
                match kids.as_slice() {
                    [only] => e = *only,
                    _ => return e,
                }
            }
            // MOZ_UNLIKELY(x) is __builtin_expect(!!(x), 0); the first child is
            // the callee, the second the condition.
            EntityKind::CallExpr
                if e.get_name().as_deref() == Some("__builtin_expect") && kids.len() >= 2 =>
            {
                e = kids[1];
            }
            _ => return e,
        }
    }
}

/// The file a node's tokens are physically written in, which for a macro
/// expansion is the macro's own definition rather than the call site.
fn spelling_file(e: Entity) -> Option<String> {
    let l = e.get_location()?.get_spelling_location();
    l.file?
        .get_path()
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
}

fn extract_if(e: Entity) -> Result<IfStmt> {
    let kids = e.get_children();
    let (cond, then, els) = match kids.as_slice() {
        [cond, then] => (*cond, *then, None),
        [cond, then, els] => (*cond, *then, Some(*els)),
        _ => {
            return Err(Unsupported::Malformed {
                what: format!("if with {} children", kids.len()),
                loc: loc(e),
            });
        }
    };
    Ok(IfStmt {
        cond: extract_expr(cond)?,
        then: extract_branch(then)?,
        els: els.map(extract_branch).transpose()?,
    })
}

/// A branch is a block, or a single statement we wrap into one.
fn extract_branch(e: Entity) -> Result<CompoundStmt> {
    if e.get_kind() == EntityKind::CompoundStmt {
        return extract_block(e);
    }
    Ok(CompoundStmt {
        stmts: extract_stmt(e)?,
    })
}

/// One `DeclStmt` can declare several variables: `Label done, ifTrue;`. Each
/// becomes its own [`LetStmt`], which is exact -- the declarators share a scope
/// and keep their order, so splitting them changes nothing.
fn extract_decls(e: Entity) -> Result<Vec<LetStmt>> {
    let kids = e.get_children();
    if kids.is_empty() {
        return Err(Unsupported::Malformed {
            what: String::from("declaration of nothing"),
            loc: loc(e),
        });
    }

    kids.into_iter()
        .map(|var| {
            if var.get_kind() != EntityKind::VarDecl {
                return Err(Unsupported::Stmt {
                    kind: var.get_kind(),
                    loc: loc(var),
                });
            }
            let name = var.get_name().ok_or_else(|| Unsupported::Malformed {
                what: String::from("unnamed variable"),
                loc: loc(var),
            })?;
            let ty = extract_type(var)?;
            let init = var
                .get_children()
                .into_iter()
                .find(|c| !is_type_ref(*c))
                .map(extract_expr)
                .transpose()?;
            Ok(LetStmt { name, ty, init })
        })
        .collect()
}

fn extract_expr(e: Entity) -> Result<TypedExpr> {
    // The span and type of the stripped node: the implicit wrappers cover the same
    // text and the same type, and this is the node the value describes.
    let e = strip(e);
    Ok(TypedExpr {
        span: span_of(e),
        value: extract_expr_value(e)?,
        // A type the subset cannot model is not a reason to refuse the expression.
        ty: extract_type(e).ok(),
    })
}

fn extract_expr_value(e: Entity) -> Result<Expr> {
    match e.get_kind() {
        // A constructor's `CallExpr` has no callee child, so it must not go
        // through `extract_call`, which would mistake its first argument for
        // one.
        EntityKind::CallExpr if is_construction(e) => Ok(Expr::Construct(extract_construct(e)?)),
        EntityKind::CallExpr => Ok(Expr::Call(extract_call(e)?)),
        // `T(x)` where `T` is a class, as in `Int32OperandId(input.id())`: the
        // cast wraps the construction it names, both covering the same text, so
        // it carries nothing the construction doesn't and is transparent.
        //
        // Only for a construction, though. A functional cast to a non-class type
        // -- `uint16_t(x)` -- is a value conversion, and dropping it would lose a
        // narrowing, so that stays unsupported.
        EntityKind::FunctionalCastExpr => {
            let constructed = e
                .get_children()
                .into_iter()
                .find(|c| !is_type_ref(*c))
                .map(strip)
                .filter(|c| is_construction(*c))
                .ok_or(Unsupported::Expr {
                    kind: e.get_kind(),
                    loc: loc(e),
                })?;
            Ok(Expr::Construct(extract_construct(constructed)?))
        }
        EntityKind::ThisExpr => Ok(Expr::This),
        EntityKind::DeclRefExpr | EntityKind::MemberRefExpr => extract_ref(e),
        EntityKind::UnaryOperator => {
            let operand =
                e.get_children()
                    .into_iter()
                    .next()
                    .ok_or_else(|| Unsupported::Malformed {
                        what: String::from("unary operator without operand"),
                        loc: loc(e),
                    })?;
            let op = operator_spelling(e).ok_or_else(|| Unsupported::Malformed {
                what: String::from("unary operator with no readable spelling"),
                loc: loc(e),
            })?;
            Ok(Expr::Unary(UnaryOp {
                op,
                operand: Box::new(extract_expr(operand)?),
            }))
        }
        EntityKind::BinaryOperator => {
            let kids = e.get_children();
            let [lhs, rhs] = kids.as_slice() else {
                return Err(Unsupported::Malformed {
                    what: format!("binary operator with {} operands", kids.len()),
                    loc: loc(e),
                });
            };
            let op = e.get_name().ok_or_else(|| Unsupported::Malformed {
                what: String::from("binary operator with no spelling"),
                loc: loc(e),
            })?;
            Ok(Expr::Binary(BinaryOp {
                op,
                lhs: Box::new(extract_expr(*lhs)?),
                rhs: Box::new(extract_expr(*rhs)?),
            }))
        }
        EntityKind::IntegerLiteral
        | EntityKind::FloatingLiteral
        | EntityKind::StringLiteral
        | EntityKind::BoolLiteralExpr => extract_lit(e),
        kind => Err(Unsupported::Expr { kind, loc: loc(e) }),
    }
}

/// LIBCLANG: construction is flattened into `CallExpr`, so it is identified by
/// what the call resolves to rather than by node kind (`CXXConstructExpr`).
fn is_construction(e: Entity) -> bool {
    e.get_reference().map(|r| r.get_kind()) == Some(EntityKind::Constructor)
}

fn extract_construct(e: Entity) -> Result<Construct> {
    let ty = e.get_type().ok_or_else(|| Unsupported::Malformed {
        what: format!(
            "construction of `{}` has no type",
            e.get_name().unwrap_or_default()
        ),
        loc: loc(e),
    })?;
    Ok(Construct {
        ty: type_of(ty, e)?,
        // Every child is an argument; there is no callee to skip.
        args: e
            .get_children()
            .into_iter()
            .map(extract_expr)
            .collect::<Result<Vec<_>>>()?,
    })
}

fn fn_id(decl: Entity) -> Option<FnId> {
    decl.get_usr().map(|usr| FnId(usr.0))
}

/// Definitions of the functions a body calls, by id.
///
/// Absence means no definition in this translation unit, so the caller has to
/// model it rather than descend. What to descend into is not decided here.
pub type Callees<'tu> = HashMap<FnId, Entity<'tu>>;

/// Every call in a body, paired with its definition where there is one.
fn collect_callees<'tu>(body: Entity<'tu>) -> Callees<'tu> {
    let mut callees = Callees::new();
    body.visit_children(|e, _| {
        if e.get_kind() == EntityKind::CallExpr {
            if let Some(target) = e.get_reference() {
                if let (Some(id), Some(def)) = (fn_id(target), target.get_definition()) {
                    callees.insert(id, def);
                }
            }
        }
        clang::EntityVisitResult::Recurse
    });
    callees
}

fn extract_call(e: Entity) -> Result<Call> {
    let name = e.get_name().unwrap_or_default();
    let target = e.get_reference().ok_or_else(|| Unsupported::Callee {
        name: name.clone(),
        loc: loc(e),
    })?;
    let fn_ref = FnRef {
        name: name.clone(),
        id: fn_id(target).ok_or_else(|| Unsupported::Callee {
            name: name.clone(),
            loc: loc(e),
        })?,
    };

    let kids = e.get_children();
    let (callee_expr, args) = kids.split_first().ok_or_else(|| Unsupported::Malformed {
        what: format!("call to {name} with no callee"),
        loc: loc(e),
    })?;

    let callee = match target.get_kind() {
        EntityKind::FunctionDecl | EntityKind::FunctionTemplate => Callee::Free(fn_ref),
        EntityKind::Method => {
            // For `v.isNumber()` the callee is a `MemberRefExpr` whose own
            // child is the receiver. An implicit `this` leaves it childless.
            let recv = callee_expr
                .get_children()
                .into_iter()
                .next()
                .map(extract_expr)
                .transpose()?
                .map(Box::new);
            Callee::Method {
                recv,
                callee: fn_ref,
            }
        }
        _ => return Err(Unsupported::Callee { name, loc: loc(e) }),
    };

    let args = args
        .iter()
        .map(|a| extract_expr(*a))
        .collect::<Result<Vec<_>>>()?;
    Ok(Call { callee, args })
}

/// Whether a declaration sits inside a function or method body.
///
/// A local's semantic parent is the callable that declares it; a global's is a
/// namespace or the translation unit, and a class-scope `static`'s is the class.
fn declared_in_callable(decl: Entity) -> bool {
    matches!(
        decl.get_semantic_parent().map(|p| p.get_kind()),
        Some(EntityKind::FunctionDecl | EntityKind::Method | EntityKind::FunctionTemplate)
    )
}

fn extract_ref(e: Entity) -> Result<Expr> {
    let name = e.get_name().unwrap_or_default();
    let target = e.get_reference().ok_or_else(|| Unsupported::Expr {
        kind: e.get_kind(),
        loc: loc(e),
    })?;
    match target.get_kind() {
        EntityKind::EnumConstantDecl => {
            let ty = target
                .get_semantic_parent()
                .and_then(|p| p.get_name())
                .ok_or_else(|| Unsupported::Malformed {
                    what: format!("enum constant {name} has no enum"),
                    loc: loc(e),
                })?;
            Ok(Expr::EnumConst(EnumConst { ty, name }))
        }
        EntityKind::ParmDecl => Ok(Expr::Ref(Ref {
            kind: RefKind::Param,
            name,
            ty: extract_type(target)?,
        })),
        // A variable, which is a local only when the callable being translated
        // is the one that declared it. `Assembler::NotEqual` is a `VarDecl` too
        // -- a class-scope `static const` -- and nothing in the name says so.
        EntityKind::VarDecl => Ok(Expr::Ref(Ref {
            kind: if declared_in_callable(target) {
                RefKind::Local
            } else {
                RefKind::Global
            },
            name,
            ty: extract_type(target)?,
        })),
        EntityKind::FieldDecl => Ok(Expr::Ref(Ref {
            kind: RefKind::Field,
            name,
            ty: extract_type(target)?,
        })),
        _ => Err(Unsupported::Expr {
            kind: e.get_kind(),
            loc: loc(e),
        }),
    }
}

/// Literals prefer `clang_Cursor_Evaluate`, which yields typed values and
/// works inside macro expansions where there are no tokens. It doesn't cover
/// every literal kind, so plain token text is the fallback.
fn extract_lit(e: Entity) -> Result<Expr> {
    use clang::EvaluationResult::*;
    let malformed = || Unsupported::Malformed {
        what: format!("unreadable {:?}", e.get_kind()),
        loc: loc(e),
    };
    let is_bool = e.get_kind() == EntityKind::BoolLiteralExpr;

    match e.evaluate() {
        Some(SignedInteger(n)) if is_bool => return Ok(Expr::Lit(Lit::Bool(n != 0))),
        Some(SignedInteger(n)) => return Ok(Expr::Lit(Lit::Int(n))),
        Some(UnsignedInteger(n)) if is_bool => return Ok(Expr::Lit(Lit::Bool(n != 0))),
        Some(UnsignedInteger(n)) => {
            return Ok(Expr::Lit(Lit::Int(
                i64::try_from(n).map_err(|_| malformed())?,
            )));
        }
        Some(Float(x)) => return Ok(Expr::Lit(Lit::Double(x))),
        Some(String(s)) | Some(CFString(s)) | Some(ObjCString(s)) | Some(Other(s)) => {
            return Ok(Expr::Lit(Lit::Str(s.to_string_lossy().into_owned())));
        }
        _ => {}
    }

    let text = e
        .get_range()
        .map(|r| r.tokenize())
        .unwrap_or_default()
        .iter()
        .map(|t| t.get_spelling())
        .collect::<Vec<_>>()
        .join("");
    match e.get_kind() {
        // LIBCLANG: a `StringLiteral`'s name is its value but its tokens are as
        // written, so `__FUNCTION__` tokenizes to itself, not the function name.
        EntityKind::StringLiteral => {
            let text = e.get_name().unwrap_or(text);
            Ok(Expr::Lit(Lit::Str(text.trim_matches('"').to_string())))
        }
        EntityKind::BoolLiteralExpr => match text.as_str() {
            "true" => Ok(Expr::Lit(Lit::Bool(true))),
            "false" => Ok(Expr::Lit(Lit::Bool(false))),
            _ => Err(malformed()),
        },
        EntityKind::IntegerLiteral => text
            .trim_end_matches(['u', 'U', 'l', 'L'])
            .parse()
            .map(|n| Expr::Lit(Lit::Int(n)))
            .map_err(|_| malformed()),
        EntityKind::FloatingLiteral => text
            .trim_end_matches(['f', 'F', 'l', 'L'])
            .parse()
            .map(|x| Expr::Lit(Lit::Double(x)))
            .map_err(|_| malformed()),
        kind => Err(Unsupported::Expr { kind, loc: loc(e) }),
    }
}

/// A parameter of a generator or a function.
#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

/// Parameters of a definition, with their types. `unit` only names the
/// definition in error messages.
fn extract_params(def: Entity, unit: &str) -> std::result::Result<Vec<Param>, Error> {
    def.get_arguments()
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let name = p.get_name().ok_or_else(|| Error::Signature {
                what: format!("unnamed parameter in {unit}"),
                loc: loc(p),
            })?;
            let ty = extract_type(p).map_err(|e| Error::Signature {
                what: format!("parameter {name}: {e}"),
                loc: loc(p),
            })?;
            Ok(Param { name, ty })
        })
        .collect()
}

/// The `CompoundStmt` child of a definition, extracted into the subset.
fn extract_body(def: Entity, unit: &str) -> std::result::Result<CompoundStmt, Error> {
    let body = def
        .get_children()
        .into_iter()
        .find(|c| c.get_kind() == EntityKind::CompoundStmt)
        .ok_or_else(|| Error::Signature {
            what: format!("{unit} has no body"),
            loc: loc(def),
        })?;
    extract_compound_stmt(body).map_err(|cause| Error::Body {
        unit: unit.to_string(),
        cause,
    })
}

/// The class a method is defined on.
///
/// Qualified rather than a bare name because the class is what decides how a
/// method is translated: a `CacheIRWriter` method is a wrapper over an op, a
/// `*IRGenerator` method is a stub generator, a `CacheIRCompiler` method is an
/// instruction. That policy lives in the translator, which needs to be able to
/// tell them apart exactly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassRef {
    /// Outermost first, ending with the class: `["js", "jit", "CacheIRWriter"]`.
    pub scope: Vec<String>,
}

impl ClassRef {
    pub fn name(&self) -> &str {
        self.scope.last().map(String::as_str).unwrap_or_default()
    }
}

impl fmt::Display for ClassRef {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.scope.join("::"))
    }
}

/// A method definition: a function definition plus the class it is on.
///
/// A stub generator is one of these, and so is a `CacheIRWriter` wrapper. What
/// distinguishes them is [`ClassRef`], not their shape, so they extract the
/// same way and the translator decides what each becomes.
#[derive(Clone, Debug)]
pub struct MethodDef<'tu> {
    pub class: ClassRef,
    pub def: FnDef<'tu>,
}

/// Finds a method's shape and extracts its body into the modeled subset.
/// Everything downstream works on the result, never on clang entities.
pub fn get_method_def<'tu>(method: &Entity<'tu>) -> std::result::Result<MethodDef<'tu>, Error> {
    let method = *method;

    if method.get_kind() != EntityKind::Method {
        return Err(Error::Signature {
            what: format!("expected a method, found {:?}", method.get_kind()),
            loc: loc(method),
        });
    }

    // Semantic, not lexical: a generator's body lives in CacheIR.cpp while its
    // class is declared in CacheIRGenerator.h, so the lexical parent is the
    // namespace.
    let class = method
        .get_semantic_parent()
        .filter(|p| matches!(p.get_kind(), EntityKind::ClassDecl | EntityKind::StructDecl))
        .ok_or_else(|| Error::Signature {
            what: String::from("method has no owning class"),
            loc: loc(method),
        })?;
    let class = ClassRef {
        scope: scope_path(class),
    };

    let def = get_callable_def(method, &class.to_string())?;
    Ok(MethodDef { class, def })
}

/// A free function defined in the CacheIR sources, e.g.
/// `CanConvertToDoubleForToNumber` or `EmitGuardToDoubleForToNumber`.
#[derive(Clone, Debug)]
pub struct FnDef<'tu> {
    /// Its name and its own identity, so a definition can be matched against
    /// the call that reached it.
    pub name: FnRef,
    pub params: Vec<Param>,
    pub ret: Type,
    pub body: CompoundStmt,
    pub callees: Callees<'tu>,
}

/// Extracts a free function's definition into the modeled subset.
pub fn get_fn_def<'tu>(function: &Entity<'tu>) -> std::result::Result<FnDef<'tu>, Error> {
    let function = *function;

    if function.get_kind() != EntityKind::FunctionDecl {
        return Err(Error::Signature {
            what: format!("expected a function, found {:?}", function.get_kind()),
            loc: loc(function),
        });
    }
    get_callable_def(function, "function")
}

/// The part that is the same for a free function and a method: the name and
/// identity, the signature, the body, and what it calls.
///
/// `what` names the kind of callable, for error messages raised before the
/// name is known.
fn get_callable_def<'tu>(
    callable: Entity<'tu>,
    what: &str,
) -> std::result::Result<FnDef<'tu>, Error> {
    // A declaration has no body to translate; we need the out-of-line
    // definition, not the one in the header.
    if !callable.is_definition() {
        return Err(Error::Signature {
            what: format!("expected a {what} definition, found a declaration"),
            loc: loc(callable),
        });
    }

    let name = callable.get_name().ok_or_else(|| Error::Signature {
        what: format!("{what} has no name"),
        loc: loc(callable),
    })?;

    // Its own identity, so a worklist can recognize the definition it asked for
    // and a callable that reaches itself can be caught.
    let id = fn_id(callable).ok_or_else(|| Error::Signature {
        what: format!("{name} has no USR"),
        loc: loc(callable),
    })?;

    let ret = callable.get_result_type().ok_or_else(|| Error::Signature {
        what: format!("{name} has no return type"),
        loc: loc(callable),
    })?;
    let ret = type_of(ret, callable).map_err(|e| Error::Signature {
        what: format!("return type: {e}"),
        loc: loc(callable),
    })?;

    let params = extract_params(callable, &name)?;
    let body = extract_body(callable, &name)?;

    Ok(FnDef {
        name: FnRef { name, id },
        params,
        ret,
        body,
        callees: collect_callees(callable),
    })
}

// Dumping ____________________________________________________________________
//
// Mirrors the raw clang dump in `main.rs`: one node per line, two spaces per
// level, names in backticks. Reading the two side by side is the quickest way
// to see what the translation dropped.

fn indent(f: &mut fmt::Formatter, depth: usize) -> fmt::Result {
    write!(f, "{:indent$}", "", indent = depth * 2)
}

impl fmt::Display for MethodDef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(
            f,
            "Method `{}` [{}] -> {}",
            self.def.name.name, self.class, self.def.ret.spelled
        )?;
        for p in &self.def.params {
            writeln!(f, "  ParmDecl `{}` : {}", p.name, p.ty.spelled)?;
        }
        fmt_block(f, &self.def.body, 1)
    }
}

impl fmt::Display for FnDef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "Function `{}` -> {}", self.name.name, self.ret.spelled)?;
        for p in &self.params {
            writeln!(f, "  ParmDecl `{}` : {}", p.name, p.ty.spelled)?;
        }
        fmt_block(f, &self.body, 1)
    }
}

impl fmt::Display for CompoundStmt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt_block(f, self, 0)
    }
}

fn fmt_block(f: &mut fmt::Formatter, block: &CompoundStmt, depth: usize) -> fmt::Result {
    indent(f, depth)?;
    writeln!(f, "CompoundStmt")?;
    for stmt in &block.stmts {
        fmt_stmt(f, &stmt.value, depth + 1)?;
    }
    Ok(())
}

/// A labelled sub-tree, for the places where position alone is ambiguous
/// (an `if`'s else arm, an assertion's guard).
fn fmt_labelled(f: &mut fmt::Formatter, label: &str, expr: &Expr, depth: usize) -> fmt::Result {
    indent(f, depth)?;
    writeln!(f, "{label}")?;
    fmt_expr(f, expr, depth + 1)
}

fn fmt_stmt(f: &mut fmt::Formatter, stmt: &Stmt, depth: usize) -> fmt::Result {
    match stmt {
        Stmt::If(s) => {
            indent(f, depth)?;
            writeln!(f, "IfStmt")?;
            fmt_expr(f, &s.cond.value, depth + 1)?;
            fmt_block(f, &s.then, depth + 1)?;
            if let Some(els) = &s.els {
                indent(f, depth + 1)?;
                writeln!(f, "Else")?;
                fmt_block(f, els, depth + 2)?;
            }
            Ok(())
        }
        Stmt::Let(s) => {
            indent(f, depth)?;
            writeln!(f, "VarDecl `{}` : {}", s.name, s.ty.spelled)?;
            match &s.init {
                Some(init) => fmt_expr(f, &init.value, depth + 1),
                None => Ok(()),
            }
        }
        Stmt::Return(s) => {
            indent(f, depth)?;
            writeln!(f, "ReturnStmt")?;
            match &s.value {
                Some(v) => fmt_expr(f, &v.value, depth + 1),
                None => Ok(()),
            }
        }
        Stmt::Assert(s) => {
            indent(f, depth)?;
            writeln!(f, "AssertStmt")?;
            if let Some(guard) = &s.guard {
                fmt_labelled(f, "Guard", &guard.value, depth + 1)?;
            }
            fmt_labelled(f, "Cond", &s.cond.value, depth + 1)
        }
        Stmt::Switch(s) => {
            indent(f, depth)?;
            writeln!(f, "SwitchStmt")?;
            fmt_labelled(f, "Scrutinee", &s.scrutinee.value, depth + 1)?;
            for case in &s.cases {
                indent(f, depth + 1)?;
                writeln!(f, "Case")?;
                for value in &case.values {
                    fmt_labelled(f, "Value", &value.value, depth + 2)?;
                }
                fmt_block(f, &case.body, depth + 2)?;
            }
            if let Some(default) = &s.default {
                indent(f, depth + 1)?;
                writeln!(f, "Default")?;
                fmt_block(f, default, depth + 2)?;
            }
            Ok(())
        }
        Stmt::Crash(s) => {
            indent(f, depth)?;
            match &s.reason {
                Some(reason) => writeln!(f, "Crash `{reason}`"),
                None => writeln!(f, "Crash"),
            }
        }
        Stmt::Expr(e) => fmt_expr(f, &e.value, depth),
    }
}

fn fmt_expr(f: &mut fmt::Formatter, expr: &Expr, depth: usize) -> fmt::Result {
    indent(f, depth)?;
    match expr {
        Expr::Call(c) => {
            match &c.callee {
                Callee::Free(callee) => writeln!(f, "Call Free `{}`", callee.name)?,
                Callee::Method { recv, callee } => {
                    writeln!(f, "Call Method `{}`", callee.name)?;
                    // The receiver is printed first, labelled, so it can't be
                    // mistaken for an argument.
                    if let Some(recv) = recv {
                        indent(f, depth + 1)?;
                        writeln!(f, "Recv")?;
                        fmt_expr(f, &recv.value, depth + 2)?;
                    }
                }
            }
            for arg in &c.args {
                fmt_expr(f, &arg.value, depth + 1)?;
            }
            Ok(())
        }
        Expr::Construct(c) => {
            writeln!(f, "Construct `{}`", c.ty.spelled)?;
            for arg in &c.args {
                fmt_expr(f, &arg.value, depth + 1)?;
            }
            Ok(())
        }
        Expr::Unary(u) => {
            writeln!(f, "Unary `{}`", u.op)?;
            fmt_expr(f, &u.operand.value, depth + 1)
        }
        Expr::Binary(b) => {
            writeln!(f, "Binary `{}`", b.op)?;
            fmt_expr(f, &b.lhs.value, depth + 1)?;
            fmt_expr(f, &b.rhs.value, depth + 1)
        }
        Expr::Ref(r) => {
            let kind = match r.kind {
                RefKind::Field => "Field",
                RefKind::Param => "Param",
                RefKind::Local => "Local",
                RefKind::Global => "Global",
            };
            writeln!(f, "{kind} `{}` : {}", r.name, r.ty.spelled)
        }
        Expr::This => writeln!(f, "This"),
        Expr::EnumConst(e) => writeln!(f, "EnumConst `{}::{}`", e.ty, e.name),
        Expr::Lit(l) => match l {
            Lit::Str(s) => writeln!(f, "Lit `{s:?}`"),
            Lit::Int(n) => writeln!(f, "Lit `{n}`"),
            Lit::Double(x) => writeln!(f, "Lit `{x:?}`"),
            Lit::Bool(b) => writeln!(f, "Lit `{b}`"),
        },
    }
}
