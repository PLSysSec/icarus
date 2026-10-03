use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::Path;

use cachet_lang::ast::{
    BinOper, CompareBinOper, Ident, LogicalBinOper, Path as CachetPath, Spanned,
};
use cachet_lang::ast::{CheckKind, NegateKind};
use cachet_lang::parser::{
    BinOperExpr, BindStmt, Block, Call, CallableItem, CheckStmt, Comment, ElseClause, Expr,
    FieldAccess, GlobalVarItem, IfStmt as CachetIfStmt, ImportItem, IrItem, Item,
    Label as CachetLabel, LabelStmt, LetStmt, Literal, LocalVar, Mod, NegateExpr, RetStmt, Stmt,
};
use clang::{Entity, EntityKind};

use crate::cacheir_ops::{
    Op as CacheIrOp, OpIr, Ops, SigParam, create_op_wrapper, field_loader, helper_sig, id_definer,
    id_user, is_operand_id, is_stub_field, op_path, translate_arg_type, writer_arity,
    writer_method,
};
use crate::cachet_utils::{emitted_ops, to_arg};
use crate::clang_utils::find_definition;
use crate::cpp_subset::{
    Call as CppCall, Callee as CppCallee, Callees, CompoundStmt as CppCompoundStmt,
    Construct as CppConstruct, Error as SubsetError, Expr as CppExpr, FnDef, FnId, FnRef,
    IfStmt as CppIfStmt, Indirection, LetStmt as CppLetStmt, Lit as CppLit, Param, RefKind,
    Span as CppSpan, Spanned as CppSpanned, Stmt as CppStmt, Type as CppType,
    TypedExpr as CppTypedExpr, get_fn_def, walk_block,
};
use crate::cpp_subset::{ClassRef, MethodDef, Ref, Visit, get_method_def};
use crate::masm_ops::{LABEL, MACRO_ASSEMBLER, MASM, MasmStmt, is_masm, masm_call};
use crate::names::{NameMap, declared_names};
use crate::scopes::{Obligation, Scopes};

/// Whether leaving a construct out still leaves a model of the C++.
///
/// The question a generated module has to answer is not "was anything left out"
/// but "does what came out mean what the C++ means". Omitting an assertion
/// weakens the proof; omitting a statement changes the behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fidelity {
    /// Left out on purpose, and sound to leave out: the model says less than the
    /// C++ but nothing it says disagrees.
    Elided,
    /// Not translatable. Whatever came out is not what the C++ does, so
    /// verifying it would verify the wrong program.
    Failed,
    /// The C++ breaks an invariant the model and the C++ both rely on, so there is
    /// nothing faithful to produce.
    ///
    /// Distinct from [`Fidelity::Failed`] because it points somewhere else: not at
    /// a construct phoenix cannot express, but at a fault in the source -- or in
    /// our reading of it, which is the likelier of the two and the reason it is
    /// reported rather than asserted.
    Invalid,
}

/// A place where the output falls short of the C++.
#[derive(Clone, Debug)]
pub struct Gap {
    pub fidelity: Fidelity,
    pub what: String,
    pub span: CppSpan,
}

impl fmt::Display for Gap {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.span {
            CppSpan::Unknown => write!(f, "{}", self.what),
            span => write!(f, "{span}: {}", self.what),
        }
    }
}

/// A construct that reached the modeled C++ subset but has no Cachet counterpart.
///
/// The second of the two ways translation can stop. An
/// [`Unsupported`](crate::cpp_subset::Unsupported) means the C++ never got as far
/// as the subset; this means it did, and the model has nothing to say it with.
///
/// Translation refuses rather than guesses: a type mapped wrongly would verify
/// something other than the code that runs, which is worse than not verifying.
#[derive(Clone, Debug)]
pub struct Unhandled {
    pub what: String,
    /// `Unknown` where the construct has no span to point at -- types and
    /// parameters aren't spanned, only statements and expressions.
    pub span: CppSpan,
    pub fidelity: Fidelity,
}

impl Unhandled {
    /// `Failed` is the default, so a construct nobody has considered lands in
    /// the safe category: it takes an explicit [`Unhandled::elided`] to claim
    /// that dropping something is harmless.
    pub fn new(what: impl Into<String>) -> Self {
        Unhandled {
            what: what.into(),
            span: CppSpan::Unknown,
            fidelity: Fidelity::Failed,
        }
    }

    /// For a construct the model deliberately does without.
    pub fn elided(what: impl Into<String>) -> Self {
        Unhandled {
            fidelity: Fidelity::Elided,
            ..Unhandled::new(what)
        }
    }

    /// For C++ that breaks an invariant the translation depends on.
    pub fn invalid(what: impl Into<String>) -> Self {
        Unhandled {
            fidelity: Fidelity::Invalid,
            ..Unhandled::new(what)
        }
    }

    fn into_gap(self, span: &CppSpan) -> Gap {
        Gap {
            fidelity: self.fidelity,
            what: self.what,
            // The statement's span, which covers the whole construct, rather
            // than the inner one the error points at.
            span: span.clone(),
        }
    }

    /// Attach a location, for errors raised where one is in scope.
    fn at(mut self, span: &CppSpan) -> Self {
        self.span = span.clone();
        self
    }
}

impl fmt::Display for Unhandled {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.span {
            CppSpan::Unknown => write!(f, "unhandled {}", self.what),
            span => write!(f, "{span}: unhandled {}", self.what),
        }
    }
}

impl std::error::Error for Unhandled {}

/// C++ type to Cachet type.
///
/// Deliberately a short, explicit table. Every entry asserts that the two
/// types denote the same values, which has to be argued case by case, so
/// entries are added one at a time and anything absent is [`Unhandled`].
pub fn translate_type(ty: &CppType) -> Result<CachetPath, Unhandled> {
    match ty.indirection {
        Indirection::Value => {}
        // `const Value&` is pass-by-reference only to avoid a copy; it denotes
        // the value it refers to and cannot change it, so it translates as that
        // value.
        Indirection::Ref if ty.is_const => {}
        // A mutable reference or a pointer is aliased state: the callee can
        // write through it, which a Cachet value cannot express.
        Indirection::Ref | Indirection::Ptr => {
            return Err(Unhandled::new(format!(
                "type `{}`: {:?} indirection",
                ty.spelled, ty.indirection
            )));
        }
    }

    let scope: Vec<&str> = ty.scope.iter().map(String::as_str).collect();
    let args: Vec<Vec<&str>> = ty
        .args
        .iter()
        .map(|arg| arg.scope.iter().map(String::as_str).collect())
        .collect();

    match (scope.as_slice(), args.as_slice()) {
        // `HandleValue` is `JS::Handle<JS::Value>`, a rooted reference to a
        // `Value`. Rooting exists to tell the GC where live pointers are
        // (RootingAPI.h, "[SMDOC] Stack Rooting"); Cachet models no GC, so the
        // wrapper carries no meaning and a handle denotes exactly its `Value`.
        (["JS", "Handle"], [inner]) if inner.as_slice() == ["JS", "Value"] => {
            Ok(CachetPath::from_ident("Value"))
        }

        // `JS::Value` itself, however it is spelled at the use site: by value,
        // or as the `const Value&` a helper takes.
        (["JS", "Value"], []) => Ok(CachetPath::from_ident("Value")),

        // `enum class JSOp` (Opcodes.h) against `enum JSOp` (notes/jsop.cachet).
        // Same name, same role: the bytecode op a generator is attaching for.
        (["JSOp"], []) => Ok(CachetPath::from_ident("JSOp")),

        // The type tag a boxed value carries (Value.h:158, notes/js.cachet:349).
        // Both are plain enums at the top level; only the variant spellings differ,
        // which [`translate_enum_const`] already maps.
        (["JSValueType"], []) => Ok(CachetPath::from_ident("JSValueType")),

        // The operand id family (notes/cacheir.cachet:89-263). Each names a slot
        // and the static type that slot carries; the C++ and Cachet spellings
        // differ only by convention. `CacheIR::defineInputValueId` returns
        // `ValueId` (:459), which is what a `ValOperandId` denotes.
        (["js", "jit", "OperandId"], []) => Ok(CachetPath::from_ident("OperandId")),
        (["js", "jit", "ValOperandId"], []) => Ok(CachetPath::from_ident("ValueId")),
        (["js", "jit", "ObjOperandId"], []) => Ok(CachetPath::from_ident("ObjectId")),
        (["js", "jit", "StringOperandId"], []) => Ok(CachetPath::from_ident("StringId")),
        (["js", "jit", "SymbolOperandId"], []) => Ok(CachetPath::from_ident("SymbolId")),
        (["js", "jit", "BooleanOperandId"], []) => Ok(CachetPath::from_ident("BoolId")),
        (["js", "jit", "Int32OperandId"], []) => Ok(CachetPath::from_ident("Int32Id")),
        (["js", "jit", "NumberOperandId"], []) => Ok(CachetPath::from_ident("NumberId")),
        (["js", "jit", "BigIntOperandId"], []) => Ok(CachetPath::from_ident("BigIntId")),
        (["js", "jit", "ValueTagOperandId"], []) => Ok(CachetPath::from_ident("ValueTagId")),
        (["js", "jit", "IntPtrOperandId"], []) => Ok(CachetPath::from_ident("IntPtrId")),

        // `bool` against Cachet's `Bool`.
        (["bool"], []) => Ok(CachetPath::from_ident("Bool")),

        // The integer types, matched on the canonical spelling clang reports -- a
        // `uint32_t` arrives as `unsigned int`. Cachet has the same ladder, so these
        // are name changes only.
        (["signed char"], []) => Ok(CachetPath::from_ident("Int8")),
        (["short"], []) => Ok(CachetPath::from_ident("Int16")),
        (["int"], []) => Ok(CachetPath::from_ident("Int32")),
        (["long"] | ["long long"], []) => Ok(CachetPath::from_ident("Int64")),
        (["unsigned char"], []) => Ok(CachetPath::from_ident("UInt8")),
        (["unsigned short"], []) => Ok(CachetPath::from_ident("UInt16")),
        (["unsigned int"], []) => Ok(CachetPath::from_ident("UInt32")),
        (["unsigned long"] | ["unsigned long long"], []) => Ok(CachetPath::from_ident("UInt64")),

        // A register holding either a boxed value or an unboxed one of a known type
        // (notes/masm.cachet:172). `AutoOutputRegister` denotes one too: the RAII
        // part has nothing to do in a model with no allocator state, and
        // `operator TypedOrValueRegister()` is what a helper taking one receives --
        // which is how `CacheIR::emitStoreBool` takes it.
        (["js", "jit", "TypedOrValueRegister"], [])
        | (["js", "jit", "AutoOutputRegister"], []) => {
            Ok(CachetPath::from_ident("TypedOrValueReg"))
        }

        // The condition a branch tests (notes/masm.cachet:575). Matched on the
        // trailing name because the path is platform-dependent: `Assembler::
        // Condition` is a typedef, canonically `vixl::Condition` on arm64 and an
        // enum in `js::jit` on x86 -- the same split that makes its *constants*
        // globals on one and enumerators on the other (see
        // [`translate_enum_const`]).
        ([.., "Condition"], []) => Ok(CachetPath::from_ident("Condition")),

        // The register a boxed value lives in (RegisterSets.h:201 -- "Registers
        // to hold a boxed value"), which the model calls `ValueReg`
        // (notes/masm.cachet:65).
        (["js", "jit", "ValueOperand"], []) => Ok(CachetPath::from_ident("ValueReg")),

        // Either a general-purpose or a float register, tagged by which
        // (RegisterSets.h:132 -- `code_ >= Registers::Total`). `AnyReg` in the model
        // (notes/masm.cachet:130).
        (["js", "jit", "AnyRegister"], []) => Ok(CachetPath::from_ident("AnyReg")),

        // A general-purpose register. The model enumerates the sixteen x86-64 ones
        // (notes/masm.cachet:6), where C++ carries an encoding, so the two agree on
        // what the type denotes and not on how it is represented.
        //
        // The RAII wrappers denote one too, each by its `operator Register()` --
        // CacheIRCompiler.h:560, :1085 and MacroAssembler-arm64.h:2187. A wrapper
        // contributes no value of its own -- which is what that operator says, and
        // how the C++ uses it; what it contributes is a *lifetime*, carried by
        // [`scopes::Obligation`] rather than by a type.
        //
        // `ScratchTagScopeRelease` is deliberately absent: it holds a pointer to a
        // scope rather than a register, and denotes nothing.
        (["js", "jit", "Register"], [])
        | (["js", "jit", "AutoScratchRegister"], [])
        | (["js", "jit", "AutoScratchRegisterMaybeOutput"], [])
        | (["js", "jit", "ScratchTagScope"], []) => Ok(CachetPath::from_ident("Reg")),

        _ => Err(Unhandled::new(format!(
            "type `{}` (canonically `{}`)",
            ty.spelled,
            ty.scope.join("::")
        ))),
    }
}

/// What translating one definition accumulates: the names it referred to that
/// still have to be defined, and the places it fell short of the C++.
///
/// State, not context: it flows back up, where [`Ctx`] flows down.
#[derive(Default)]
pub struct State {
    pub needed: Vec<Needed>,
    pub gaps: Vec<Gap>,
    /// `StubFieldOffset` local -> the op field parameter it stands for: `val` ->
    /// `val`. Recorded rather than matched against the next statement, so the
    /// construction and its use need not be adjacent. Admits one use, as
    /// `emitLoadStubField`'s argument.
    stub_fields: HashMap<String, Ident>,
    /// The locals in scope, for the facts a C++ type doesn't carry.
    scopes: Scopes,
}

/// Something a translated body referred to that still has to be defined.
///
/// Two ways to get one: translate the C++ definition, or synthesize it from a
/// `CacheIROps.yaml` entry. Which it is follows from how the call resolved, so it
/// is recorded here rather than rediscovered by the worklist.
#[derive(Clone, Debug)]
pub enum Needed {
    /// A C++ definition to translate, found by identity.
    Cpp(FnRef),
    /// A wrapper over a CacheIR op, named by the op.
    Wrapper(String),
}

/// The op a writer call emits.
fn resolve_op<'a>(ops: &'a Ops, method: &str) -> Result<&'a CacheIrOp, Unhandled> {
    ops.by_writer_method(method).ok_or_else(|| {
        Unhandled::new(format!(
            "`writer.{method}`: no matching op in CacheIROps.yaml"
        ))
    })
}

/// C++ passes every operand the writer method takes, so a mismatch means the
/// call isn't the one the yaml describes.
fn check_arity(op: &CacheIrOp, method: &str, args: usize) -> Result<(), Unhandled> {
    let expected = writer_arity(op);
    if args != expected {
        return Err(Unhandled::new(format!(
            "`writer.{method}` takes {expected} operands, called with {args}"
        )));
    }
    Ok(())
}

#[derive(Default)]
struct Fields(Vec<Ref>);

impl Visit for Fields {
    fn visit_ref(&mut self, r: &Ref) {
        if r.kind == RefKind::Field && !self.0.iter().any(|f| f.name == r.name) {
            self.0.push(r.clone());
        }
    }
}

fn get_method_def_fields(method_def: &MethodDef) -> Vec<Ref> {
    let mut fields = Fields::default();
    walk_block(&mut fields, &method_def.def.body);
    fields.0
}

/// `js::jit::CacheIRWriter`, the type of a generator's `writer` field.
const CACHE_IR_WRITER: [&str; 3] = ["js", "jit", "CacheIRWriter"];

/// The writer is how C++ emits CacheIR, which Cachet makes implicit. So a
/// writer is never a value: it is dropped as an argument and as a parameter,
/// and a call on it becomes an `emit`.
fn is_writer(ty: &CppType) -> bool {
    ty.scope == CACHE_IR_WRITER
}

/// Whether an expression is the writer itself, for dropping it as an argument.
fn is_writer_expr(expr: &CppTypedExpr) -> bool {
    matches!(&expr.value, CppExpr::Ref(r) if is_writer(&r.ty))
}

/// The types a unit carries implicitly, which Cachet makes ambient rather than
/// modeling as values: the CacheIR sink, the register allocator, and the machine
/// the code is emitted to.
///
/// None of them can be passed, so where C++ hands one along -- as
/// `useValueRegister(masm, inputId)` does -- the argument is dropped.
/// The machine appears in two spellings: the compiler's `masm` field is a
/// `StackMacroAssembler`, while a helper takes the base `MacroAssembler&`.
const AMBIENT: [[&str; 3]; 4] = [CACHE_IR_WRITER, ALLOCATOR, MASM, MACRO_ASSEMBLER];

/// Whether a value of this type is one the unit carries implicitly, so it is
/// neither passed nor declared.
fn is_ambient(ty: &CppType) -> bool {
    AMBIENT.iter().any(|ambient| ty.scope == *ambient)
}

/// `js::jit::CacheRegisterAllocator`, which an instruction reaches registers
/// through. The model holds no value for it, so its operations live on
/// `ir CacheIR` instead.
const ALLOCATOR: [&str; 3] = ["js", "jit", "CacheRegisterAllocator"];

/// `js::jit::AutoOutputRegister`, which reserves the register an instruction's
/// result goes into and releases it on the way out.
///
/// The model has the register -- `var outputReg: TypedOrValueReg` -- but not the
/// reservation: `allocateReg` and `isAllocatedReg` are uninterpreted and
/// `releaseReg` has no body, so the whole allocator is unmodeled (a refinement
/// that would have tracked it is commented out, notes/cacheir.cachet:2196-2223).
/// So the declaration is dropped and the local denotes the register, which is what
/// `operator TypedOrValueRegister()` makes it mean anyway.
const AUTO_OUTPUT_REGISTER: [&str; 3] = ["js", "jit", "AutoOutputRegister"];

fn is_ambient_expr(expr: &CppTypedExpr) -> bool {
    matches!(&expr.value, CppExpr::Ref(r) if is_ambient(&r.ty))
}

/// The method, if this is a call on the writer: either through a `writer` field,
/// or on an implicit `this` inside a `CacheIRWriter` method.
fn writer_call<'a>(ctx: &Ctx<'_>, call: &'a CppCall) -> Option<&'a FnRef> {
    match &call.callee {
        CppCallee::Method {
            recv: Some(recv),
            callee,
        } if is_writer_expr(&**recv) => Some(callee),
        CppCallee::Method { recv: None, callee } if ctx.recv_is_writer() => Some(callee),
        _ => None,
    }
}

/// `js::jit::ValOperandId`, a generator's input operand.
const VAL_OPERAND_ID: [&str; 3] = ["js", "jit", "ValOperandId"];

/// `f()`, with no arguments.
fn invoke(target: CachetPath) -> Expr {
    Expr::Invoke(Call {
        target: Spanned::internal(target),
        args: Spanned::internal(Vec::new()),
    })
}

/// The statements a `CompareIRGenerator` stub generator opens with:
///
/// ```text
/// initRegState();
/// let lhsId = CacheIR::defineInputValueId();
/// let rhsId = CacheIR::defineInputValueId();
/// initValueOutput();
/// assume JSOp::isEqualityOp(CompareIRGenerator::op_)
///     || JSOp::isRelationalOp(CompareIRGenerator::op_);
/// ```
///
/// None of this comes from the generator's body. It is the contract of
/// `CompareIRGenerator::tryAttachStub` (CacheIR.cpp:15291), which every
/// `tryAttach*` inherits from its caller:
///
/// ```cpp
/// MOZ_ASSERT(cacheKind_ == CacheKind::Compare);
/// MOZ_ASSERT(IsEqualityOp(op_) || IsRelationalOp(op_));
/// ...
/// ValOperandId lhsId(writer.setInputOperandId(lhsIndex));
/// ValOperandId rhsId(writer.setInputOperandId(rhsIndex));
/// ```
///
/// The two `setInputOperandId` calls are the two `defineInputValueId` `let`s,
/// which also stand in for the method's parameters -- one each, named as C++
/// names them -- so the body can refer to a parameter as an ordinary local. The
/// `MOZ_ASSERT` on `op_` is the `assume`: unlike an assertion inside a generator,
/// this one states what the caller guarantees, and without it `op_` is an
/// arbitrary `JSOp` and `Condition::fromJSOp` has no case for it.
///
/// Every dispatcher has its own contract, so this is specific to
/// `CompareIRGenerator`; another generator's preamble has to be read off its own
/// `tryAttachStub` rather than assumed to match.
fn translate_preamble(
    class: &ClassRef,
    params: &[Param],
) -> Result<Vec<Spanned<Stmt>>, Unhandled> {
    if class.name() != "CompareIRGenerator" {
        return Err(Unhandled::new(format!(
            "`{class}`: no preamble modeled -- it has to come from that class's \
             own `tryAttachStub`"
        )));
    }

    // `tryAttachStub` declares exactly these two operands, both values.
    if let Some(other) = params.iter().find(|p| p.ty.scope != VAL_OPERAND_ID) {
        return Err(Unhandled::new(format!(
            "parameter `{}`: expected a ValOperandId, found `{}`",
            other.name, other.ty.spelled
        )));
    }
    if params.len() != 2 {
        return Err(Unhandled::new(format!(
            "generator takes {} operands, expected 2",
            params.len()
        )));
    }

    let mut stmts = vec![Spanned::internal(Stmt::Expr(invoke(
        CachetPath::from_ident("initRegState"),
    )))];

    stmts.extend(params.iter().map(|param| {
        Spanned::internal(Stmt::Let(LetStmt {
            lhs: LocalVar {
                ident: Spanned::internal(Ident::from(param.name.clone())),
                is_mut: false,
                // Inferred from the initializer.
                type_: None,
            },
            rhs: Spanned::internal(invoke(
                CachetPath::from_ident("CacheIR").nest(Ident::from("defineInputValueId")),
            )),
        }))
    }));

    stmts.push(Spanned::internal(Stmt::Expr(invoke(
        CachetPath::from_ident("initValueOutput"),
    ))));

    // `MOZ_ASSERT(IsEqualityOp(op_) || IsRelationalOp(op_))`. The field is
    // qualified, as a field reached from inside an `op` has to be.
    let op_field = Expr::Var(Spanned::internal(
        CachetPath::from_ident(Ident::from(class.name().to_owned())).nest(Ident::from("op_")),
    ));
    let is_op = |predicate: &str| {
        Expr::Invoke(Call {
            target: Spanned::internal(
                CachetPath::from_ident("JSOp").nest(Ident::from(predicate.to_owned())),
            ),
            args: Spanned::internal(vec![Spanned::internal(to_arg(op_field.clone()))]),
        })
    };
    stmts.push(Spanned::internal(Stmt::Check(CheckStmt {
        kind: CheckKind::Assume,
        cond: Spanned::internal(Expr::BinOper(Box::new(BinOperExpr {
            oper: Spanned::internal(BinOper::Logical(LogicalBinOper::Or)),
            lhs: Spanned::internal(is_op("isEqualityOp")),
            rhs: Spanned::internal(is_op("isRelationalOp")),
        }))),
    })));

    Ok(stmts)
}

/// `var lhsVal_: Value;` for each of the generator's fields.
///
/// Names are kept exactly as C++ spells them -- `lhsVal_`, not `lhsValue` --
/// so a generated name always traces back to its source.
fn create_field_var_items(fields: &[Ref]) -> Result<Vec<Spanned<Item>>, Unhandled> {
    fields
        .iter()
        .map(|field| {
            let type_ = translate_type(&field.ty)
                .map_err(|e| Unhandled::new(format!("field `{}`: {}", field.name, e.what)))?;
            Ok(Spanned::internal(Item::GlobalVar(GlobalVarItem {
                ident: Spanned::internal(Ident::from(field.name.clone())),
                attrs: Vec::new(),
                is_mut: false,
                type_: Spanned::internal(type_),
                value: None,
            })))
        })
        .collect()
}

/// `js::jit::CacheIRCompiler`, whose `emit*` methods are the op semantics.
const CACHE_IR_COMPILER: [&str; 3] = ["js", "jit", "CacheIRCompiler"];

/// The classes in `CacheIRGenerator.h` whose methods are stub generators.
///
/// Enumerated rather than matched on the `IRGenerator` suffix, which would also
/// catch `LIRGenerator` and `MIRGenerator` -- unrelated Ion classes -- and the
/// `IRGenerator` base class, whose methods are shared helpers rather than
/// generators. A class missing from here is refused rather than guessed at, so
/// an omission shows up as a translation failure and never as a wrong `ir`.
const STUB_GENERATORS: &[&str] = &[
    "BinaryArithIRGenerator",
    "BindNameIRGenerator",
    "CallIRGenerator",
    "CheckPrivateFieldIRGenerator",
    "CloseIterIRGenerator",
    "CompareIRGenerator",
    "GetImportIRGenerator",
    "GetIteratorIRGenerator",
    "GetNameIRGenerator",
    "GetPropIRGenerator",
    "HasPropIRGenerator",
    "InlinableNativeIRGenerator",
    "InstanceOfIRGenerator",
    "LambdaIRGenerator",
    "LazyConstantIRGenerator",
    "NewArrayIRGenerator",
    "NewObjectIRGenerator",
    "OptimizeGetIteratorIRGenerator",
    "OptimizeSpreadCallIRGenerator",
    "SetPropIRGenerator",
    "ToBoolIRGenerator",
    "ToPropertyKeyIRGenerator",
    "TypeOfEqIRGenerator",
    "TypeOfIRGenerator",
    "UnaryArithIRGenerator",
];

/// What is being translated, and the op table its `writer` calls resolve
/// against. Flows down; immutable.
struct Ctx<'a> {
    /// The class the callable is defined on, `None` for a free function.
    ///
    /// The class decides what is ambient -- entities the code carries
    /// implicitly, which translation reinterprets rather than translates. A
    /// generator's `writer` is the CacheIR sink and its `AttachDecision` is
    /// protocol with the dispatcher; a `CacheIRWriter` method *is* the writer,
    /// so its implicit `this` is one; an instruction's `masm` and `allocator`
    /// are the machine. A free function carries nothing.
    class: Option<ClassRef>,
    /// `CacheIROps.yaml`, which decides what a `writer` call becomes.
    ops: &'a Ops,
    /// Which `ir` an emitted op belongs to. Only op paths and the `emits` clauses
    /// aimed at them depend on this; every modeled function stays in `CacheIR`
    /// whichever way it is set.
    op_ir: OpIr,
    /// The parameters of the C++ definition, and of the Cachet item it becomes.
    ///
    /// Both, because they disagree: the body is written against the first, the
    /// signature is the second. An instruction's op says `result` where its
    /// hand-written definition says `resultId` -- only the declaration in
    /// `CacheIROpsGenerated.h` is generated from the yaml, so the definition is
    /// free to spell its parameters however it likes.
    cpp_sig: Option<Vec<Param>>,
    cachet_sig: Option<Vec<SigParam>>,
    /// What each name the unit declares is called in Cachet, which is the C++ name
    /// unless it is a Cachet keyword. Decided for the whole unit at once so a
    /// mangled name cannot land on one already in use.
    names: NameMap,
    /// The op being given meaning, when this is an instruction, and `None`
    /// otherwise -- only [`translate_cacheir_op`] sets it.
    instruction: Option<Instruction>,
}

/// What an instruction's body needs to know about the op it implements.
#[derive(Clone, Debug)]
struct Instruction {
    /// The op's stub-field operands, keyed by the name the C++ gives the offset
    /// it receives in their place.
    ///
    /// The yaml says `val: RawInt32Field` and the model's op takes that field;
    /// the generated C++ takes `uint32_t valOffset`, since `arg_reader_info`
    /// turns every field into a stub-data offset. The model has no notion of an
    /// offset at all, so `valOffset` has no counterpart -- it is not renamed to
    /// the field, it is eliminated, and the `StubFieldOffset` construction is
    /// the only place that may consume it.
    ///
    /// Names only: the field's type is in [`Ctx::cachet_sig`] under the name this
    /// maps to.
    offsets: HashMap<String, Ident>,
}

impl Ctx<'_> {
    /// The type the Cachet signature binds `name` at.
    fn cachet_param_ty(&self, name: &Ident) -> Option<&CachetPath> {
        let sig = self.cachet_sig.as_ref()?;
        sig.iter().find(|p| &p.name == name).map(|p| &p.ty)
    }

    /// The Cachet parameter a C++ parameter became, found by position.
    ///
    /// Position is all that relates them, so this answers only when the two
    /// signatures have the same length -- true of an instruction, whose
    /// declaration is generated from the yaml the op's signature comes from, and
    /// false of a generator, whose op takes no parameters at all.
    fn cachet_param(&self, cpp_name: &str) -> Option<&SigParam> {
        let (cpp, cachet) = (self.cpp_sig.as_ref()?, self.cachet_sig.as_ref()?);
        if cpp.len() != cachet.len() {
            return None;
        }
        let i = cpp.iter().position(|p| p.name == cpp_name)?;
        cachet.get(i)
    }

    /// Whether `this` is the CacheIR writer, which makes a call on an implicit
    /// receiver -- `guardToInt32_(input)` inside a wrapper -- a writer call.
    fn recv_is_writer(&self) -> bool {
        self.class
            .as_ref()
            .is_some_and(|class| class.scope == CACHE_IR_WRITER)
    }

    /// Whether this is a stub generator, whose `AttachDecision` return is
    /// protocol with the dispatcher rather than a value.
    fn is_stub_generator(&self) -> bool {
        let Some(class) = &self.class else {
            return false;
        };
        let scope: Vec<&str> = class.scope.iter().map(String::as_str).collect();
        matches!(scope.as_slice(), ["js", "jit", name] if STUB_GENERATORS.contains(name))
    }

    /// The item a field qualifies against, since Cachet won't resolve a bare
    /// field name inside an `op`.
    ///
    /// `None` for anything translated as a top-level `fn` -- a free function, or
    /// a writer wrapper, which reads no writer state -- and for a class we don't
    /// recognize, so a field is refused rather than qualified against something
    /// that doesn't hold it.
    fn parent(&self) -> Option<CachetPath> {
        let class = self.class.as_ref()?;
        if class.scope == CACHE_IR_COMPILER {
            // An instruction is an `op` in whichever `ir` holds the ops, not in its
            // own class.
            return Some(self.op_ir.path());
        }
        self.is_stub_generator()
            .then(|| CachetPath::from_ident(Ident::from(class.name().to_owned())))
    }
}

/// A C++ method to the Cachet function that models it.
///
/// Keyed on the receiver's *translated* type, so `lhsVal_.isNumber()` (whose
/// receiver is a `HandleValue`) and `v.isNumber()` (a `const Value&`) reach the
/// same entry: both receivers translate to `Value`.
///
/// The names do not always match, which is why this is a table and not a rule:
/// C++ spells it `isBoolean`, the model spells it `isBool`.
fn translate_method(recv: &CppType, method: &str) -> Option<(&'static str, &'static str)> {
    let cpp_ty = recv.scope.join("::");
    let cachet_ty = translate_type(recv).ok().map(|ty| ty.to_string());

    match (cpp_ty.as_str(), cachet_ty.as_deref(), method) {
        // Keyed on the Cachet type, so `lhsVal_.isNumber()` (whose receiver is a
        // `JS::Handle<JS::Value>`) and `v.isNumber()` (a `const JS::Value&`)
        // reach one entry rather than one per wrapper. `impl Value` in
        // notes/js.cachet.
        (_, Some("Value"), "isNumber") => Some(("Value", "isNumber")),
        (_, Some("Value"), "isInt32") => Some(("Value", "isInt32")),
        (_, Some("Value"), "isBoolean") => Some(("Value", "isBool")),
        (_, Some("Value"), "isNull") => Some(("Value", "isNull")),
        (_, Some("Value"), "isNullOrUndefined") => Some(("Value", "isNullOrUndefined")),
        (_, Some("Value"), "isBigInt") => Some(("Value", "isBigInt")),

        // Whether the result register holds a boxed value rather than an unboxed one
        // of a known type (notes/masm.cachet:182). Keyed on the Cachet type so an
        // `AutoOutputRegister` receiver and a `TypedOrValueRegister` one reach the
        // same row, both denoting the register.
        (_, Some("TypedOrValueReg"), "hasValue") => Some(("TypedOrValueReg", "hasValue")),
        // The boxed half of the register. `assert TypedOrValueReg::hasValue(reg)`
        // opens the model's version (notes/masm.cachet:196), which is the obligation
        // C++ leaves to the `if (output.hasValue())` around the call site.
        (_, Some("TypedOrValueReg"), "valueReg") => Some(("TypedOrValueReg", "toValueReg")),
        // The unboxed half. `MOZ_ASSERT(hasTyped())` opens the C++
        // (RegisterSets.h:300) and `assert TypedOrValueReg::hasTyped(reg)` the
        // model's, so the precondition carries over as an obligation.
        (_, Some("TypedOrValueReg"), "typedReg") => Some(("TypedOrValueReg", "toTypedReg")),
        // The general-purpose half of an `AnyRegister`, which is a projection with a
        // tag check: `MOZ_ASSERT(!isFloat())` in C++ (RegisterSets.h:137), the same
        // assertion in `AnyReg::toReg`.
        (_, Some("AnyReg"), "gpr") => Some(("AnyReg", "toReg")),
        // The float half, guarded the other way round: `MOZ_ASSERT(isFloat())`
        // (RegisterSets.h:140) against the same assertion in `AnyReg::toFloatReg`.
        // `TypedOrValueRegister` has an `fpu()` too (RegisterSets.h:1348), which
        // keying on the Cachet type keeps separate from this one.
        (_, Some("AnyReg"), "fpu") => Some(("AnyReg", "toFloatReg")),
        // The tag those projections check, uninterpreted on both sides: C++ reads it
        // off the register's encoding (`code_ >= Registers::Total`), the model leaves
        // `AnyReg::isFloat` without a body (notes/masm.cachet:133).
        (_, Some("AnyReg"), "isFloat") => Some(("AnyReg", "isFloat")),

        // Keyed on the C++ type, there being no Cachet type: an instruction
        // reaches registers *through* `allocator`, which the model holds no
        // value for, keeping those operations on `ir CacheIR` instead.
        // `useValueRegister` and the rest of the register family are not here:
        // their callee depends on the operand's id type, so they go through
        // [`translate_allocator_register`] instead.
        ("js::jit::CacheRegisterAllocator", _, "knownType") => Some(("CacheIR", "knownType")),

        _ => None,
    }
}

/// A C++ enum constant to the Cachet one, as
/// `(cpp type, cpp const) -> (cachet type, cachet const)`.
///
/// A table rather than a rule: the model spells the value types as
/// `JS::ValueType` does (Value.h:176), while the code being translated uses the
/// older `JSVAL_TYPE_*` constants, and stripping the prefix isn't enough --
/// `BOOLEAN` is `Bool` and `PRIVATE_GCTHING` is `PrivateGCThing`. Other enums
/// don't even agree on the type's name.
///
/// The type is keyed by its bare name, because whether a constant is even an
/// enumerator depends on the architecture. `Assembler::NotEqual` is an
/// enumerator of `Assembler::Condition` on x86-shared, and on arm64 a
/// `static const Condition` aliasing an enumerator of `vixl::Condition`
/// (`COPYENUM`, Assembler-vixl.h:110) -- so it arrives as an enum constant on
/// one and a global on the other, with different scopes, for one Cachet target.
/// Two `Condition` types in different namespaces would collide here; within the
/// JIT there is only the one.
fn translate_enum_const<'n>(ty: &str, name: &'n str) -> Option<(&'static str, &'n str)> {
    match (ty, name) {
        // `enum JSValueType` (notes/js.cachet:349) against Value.h:158. The two
        // agree variant for variant, in the same order.
        ("JSValueType", "JSVAL_TYPE_DOUBLE") => Some(("JSValueType", "Double")),
        ("JSValueType", "JSVAL_TYPE_INT32") => Some(("JSValueType", "Int32")),
        ("JSValueType", "JSVAL_TYPE_BOOLEAN") => Some(("JSValueType", "Bool")),
        ("JSValueType", "JSVAL_TYPE_UNDEFINED") => Some(("JSValueType", "Undefined")),
        ("JSValueType", "JSVAL_TYPE_NULL") => Some(("JSValueType", "Null")),
        ("JSValueType", "JSVAL_TYPE_MAGIC") => Some(("JSValueType", "Magic")),
        ("JSValueType", "JSVAL_TYPE_STRING") => Some(("JSValueType", "String")),
        ("JSValueType", "JSVAL_TYPE_SYMBOL") => Some(("JSValueType", "Symbol")),
        ("JSValueType", "JSVAL_TYPE_PRIVATE_GCTHING") => Some(("JSValueType", "PrivateGCThing")),
        ("JSValueType", "JSVAL_TYPE_BIGINT") => Some(("JSValueType", "BigInt")),
        ("JSValueType", "JSVAL_TYPE_OBJECT") => Some(("JSValueType", "Object")),
        ("JSValueType", "JSVAL_TYPE_UNKNOWN") => Some(("JSValueType", "Unknown")),

        // `enum JSOp` (notes/jsop.cachet) and `enum Condition` (notes/masm.cachet)
        // keep the engine's variant names, so a constant passes through by name.
        //
        // A rule rather than a table because the model declares 227 of the engine's
        // 257 opcodes -- too many to list -- and it is safe here where it was not
        // for a bare global: a variant is qualified by its enum, so an unmodeled one
        // cannot bind to something else. Cachet refuses it outright, "undefined
        // variable `JSOp::NotARealOpcode`", which makes a miss loud.
        ("JSOp", name) => Some(("JSOp", name)),
        ("Condition", name) => Some(("Condition", name)),

        _ => None,
    }
}

/// A C++ free function to the Cachet function that models it.
///
/// A miss means the definition is translated instead. The CacheIR machinery --
/// stub generators, their helpers, and the instruction semantics in
/// `CacheIRCompiler` -- is meant to be translated, so entries here are for
/// engine functions outside it, where translation should stop.
fn translate_free(name: &str) -> Option<CachetPath> {
    let (ty, f) = match name {
        // `BooleanValue(b)` is `{ Value v; v.setBoolean(b); return v; }`
        // (Value.h:1183) -- bit work on the tagged representation, which is what the
        // model replaces wholesale rather than describes. `Value::fromBool` says the
        // same thing abstractly: the result `isBool` and reads back as `b`
        // (notes/js.cachet:505).
        "BooleanValue" => ("Value", "fromBool"),
        _ => return None,
    };
    Some(CachetPath::from_ident(ty).nest(Ident::from(f)))
}

/// A C++ binary operator to Cachet's.
fn translate_bin_oper(op: &str) -> Option<BinOper> {
    Some(match op {
        "||" => BinOper::Logical(LogicalBinOper::Or),
        "&&" => BinOper::Logical(LogicalBinOper::And),
        "==" => BinOper::Compare(CompareBinOper::Eq),
        "!=" => BinOper::Compare(CompareBinOper::Neq),
        _ => return None,
    })
}

/// A C++ unary operator to Cachet's.
///
/// Cachet's unary operators are all negations, so `&` and `*` have no
/// counterpart -- taking an address or dereferencing is aliasing, which a value
/// language cannot express.
fn translate_unary_oper(op: &str) -> Option<NegateKind> {
    Some(match op {
        "!" => NegateKind::Logical,
        "-" => NegateKind::Arith,
        "~" => NegateKind::Bitwise,
        _ => return None,
    })
}

/// `Int32OperandId(input.id())`: an operand id rebuilt from another one's slot
/// number.
///
/// Not really a construction. Nothing is allocated and no CacheIR is emitted:
/// the slot is the same slot, and only the static type it carries changes. The
/// model spells that `OperandId::toInt32Id(input)`
/// (notes/cacheir.cachet:102-138), which is why this is the one construction
/// shape that translates.
fn translate_retype(
    ctx: &Ctx<'_>,
    state: &mut State,
    c: &CppConstruct,
) -> Result<Expr, Unhandled> {
    let unmodeled = || Unhandled::new(format!("construction of `{}`", c.ty.spelled));

    let ty = translate_type(&c.ty).map_err(|_| unmodeled())?;
    if !is_operand_id(&ty) {
        return Err(unmodeled());
    }

    // The sole argument has to be another operand id's `id()`, since that is
    // what makes this a retyping of an existing slot rather than a fresh value.
    let [arg] = c.args.as_slice() else {
        return Err(unmodeled());
    };
    let CppExpr::Call(call) = &arg.value else {
        return Err(unmodeled());
    };
    let CppCallee::Method {
        recv: Some(recv),
        callee,
    } = &call.callee
    else {
        return Err(unmodeled());
    };
    if callee.name != "id" || !call.args.is_empty() {
        return Err(unmodeled());
    }
    let recv_ty = translate_type(expr_type(recv)?)?;
    if !is_operand_id(&recv_ty) {
        return Err(unmodeled());
    }

    // `to*Id` is declared on `OperandId`, and every id type is one, so the
    // receiver is passed as-is and Cachet upcasts it.
    Ok(Expr::Invoke(Call {
        target: Spanned::internal(
            CachetPath::from_ident("OperandId").nest(Ident::from(format!("to{ty}"))),
        ),
        args: Spanned::internal(vec![Spanned::internal(to_arg(translate_expr(
            ctx, state, recv,
        )?))]),
    }))
}

/// The local a `StubFieldOffset` construction binds, and the op field it stands
/// for, if this declaration is one.
///
/// Matched narrowly: the construction has to name an offset parameter of the op
/// being translated. Anything else built from a `StubFieldOffset` is refused,
/// there being nothing in the model to stand for it.
fn stub_field_binding(ctx: &Ctx<'_>, l: &CppLetStmt) -> Option<(String, Ident)> {
    let instruction = ctx.instruction.as_ref()?;
    let init = l.init.as_ref()?;
    let CppExpr::Construct(c) = &init.value else {
        return None;
    };
    if c.ty.scope != ["js", "jit", "StubFieldOffset"] {
        return None;
    }
    let CppExpr::Ref(offset) = &c.args.first()?.value else {
        return None;
    };
    let field = instruction.offsets.get(&offset.name)?;
    Some((l.name.clone(), field.clone()))
}

/// `Label ifTrue;` -- a default-constructed `js::jit::Label`. The C++ declares it
/// to take its address later; the model declares it to `bind` and `goto` it.
fn is_label_decl(l: &CppLetStmt) -> bool {
    matches!(
        l.init.as_ref().map(|init| &init.value),
        Some(CppExpr::Construct(c)) if c.ty.scope == LABEL && c.args.is_empty()
    )
}

/// `AutoOutputRegister output(*this);` -- the declaration whose only effect is the
/// reservation the model does without.
fn is_output_register_decl(l: &CppLetStmt) -> bool {
    matches!(
        l.init.as_ref().map(|init| &init.value),
        Some(CppExpr::Construct(c)) if c.ty.scope == AUTO_OUTPUT_REGISTER
    )
}

/// `AutoScratchRegister scratch(allocator, masm);` and the `MaybeOutput` flavour --
/// a declaration that takes a register out of the allocator.
///
/// Both become `CacheIR::allocateReg()`. `MaybeOutput` reuses the output register
/// when one is free rather than allocating, which the model's allocator does not
/// express; translating it as an allocation is still sound, because the output
/// register is assumed *un*allocated (notes/utils.cachet:64) and so
/// `allocateReg` may return it. The cost is register pressure, not correctness:
/// the model consumes one where the C++ reuses.
///
/// Only the two-argument form, which is what makes the defaulted argument worth
/// dropping in `cpp_subset`. The three-argument form asks for a *fixed* register,
/// and the model's counterpart for that is `unsafe fn allocateKnownReg`, so it is
/// left to be refused.
fn is_scratch_register_decl(l: &CppLetStmt) -> bool {
    const SCRATCH: [&str; 3] = ["js", "jit", "AutoScratchRegister"];
    const SCRATCH_MAYBE_OUTPUT: [&str; 3] = ["js", "jit", "AutoScratchRegisterMaybeOutput"];

    matches!(
        l.init.as_ref().map(|init| &init.value),
        Some(CppExpr::Construct(c))
            if (c.ty.scope == SCRATCH && c.args.len() == 2)
                || (c.ty.scope == SCRATCH_MAYBE_OUTPUT && c.args.len() == 3)
    )
}

/// `ScratchTagScope tag(masm, input);` -- a declaration that takes the register the
/// Value's type tag will be extracted into.
///
/// A scope type because where the tag lives is platform-dependent: on 64-bit a Value
/// is one register, so its tag needs another one allocated, while on 32-bit a Value
/// is already a (type, payload) pair and the tag is `value.typeReg()`. The model
/// takes the 64-bit reading and fixes the register at R11
/// (notes/cacheir.cachet:1848), which is why `allocateScratchReg` takes no argument.
fn is_tag_scope_decl(l: &CppLetStmt) -> bool {
    matches!(
        l.init.as_ref().map(|init| &init.value),
        Some(CppExpr::Construct(c)) if c.ty.scope == ["js", "jit", "ScratchTagScope"]
    )
}

/// `ScratchTagScopeRelease _(&tag);` -- a declaration that *gives the tag register
/// back* for the length of its block, and takes it again at the end.
///
/// The inverse of the wrapper above, and the one RAII type here whose constructor
/// gives up a resource rather than taking one: its body is `ts_->release()`, its
/// destructor `ts_->reacquire()` (MacroAssembler-arm64.h:2211). Emitters use it to
/// free a register up once the tag has been read for the last time.
fn is_tag_scope_release_decl(l: &CppLetStmt) -> bool {
    matches!(
        l.init.as_ref().map(|init| &init.value),
        Some(CppExpr::Construct(c)) if c.ty.scope == ["js", "jit", "ScratchTagScopeRelease"]
    )
}

/// `emitLoadStubField(..)` on the compiler's implicit `this`.
fn is_load_stub_field(call: &CppCall) -> bool {
    matches!(
        &call.callee,
        CppCallee::Method { recv: None, callee } if callee.name == "emitLoadStubField"
    )
}

/// `trackAttached(..)` on a generator's implicit `this`.
fn is_track_attached(call: &CppCall) -> bool {
    matches!(
        &call.callee,
        CppCallee::Method { recv: None, callee } if callee.name == "trackAttached"
    )
}

/// A call that records nothing about the machine, and so is dropped rather than
/// translated. The name is for the tally; `None` means keep translating.
///
/// Both of these feed the IC spewer, which is a debugging aid: `JitSpew` has no
/// effect outside a `JS_JITSPEW` build at all (JitSpewer.h:41 -- "None of the
/// global functions have effect on non-debug builds"), and `trackAttached` sets
/// a stub name for it to print. Neither emits CacheIR.
fn dropped_call(ctx: &Ctx<'_>, call: &CppCall) -> Option<&'static str> {
    if matches!(&call.callee, CppCallee::Free(callee) if callee.name == "JitSpew") {
        return Some("JitSpew");
    }
    if ctx.is_stub_generator() && is_track_attached(call) {
        return Some("trackAttached");
    }
    None
}

/// The C++ type of an expression, which keys [`translate_method`] and the operand
/// families.
///
/// One function rather than one per call site: this used to be two, a receiver
/// version covering names and an operand version covering names and constructions,
/// and which shapes each covered was an accident of what had been needed. That is
/// how `output.typedReg().gpr()` came to fail -- a chained receiver is a call, and
/// the receiver version knew nothing about calls. Now every expression carries its
/// type, so there is nothing to be partial about.
fn expr_type(expr: &CppTypedExpr) -> Result<&CppType, Unhandled> {
    expr.ty
        .as_ref()
        .ok_or_else(|| Unhandled::new(String::from("expression has no type the subset models")))
}

/// A C++ integer literal, at the width its type says.
///
/// The value has to fit that width, and a mismatch is refused rather than
/// truncated: a `1 << 40` typed `int` means the source is doing something this
/// doesn't understand.
fn int_literal(ty: Option<&CppType>, n: i64) -> Result<Expr, Unhandled> {
    let ty = ty.ok_or_else(|| Unhandled::new(format!("integer literal {n} has no type")))?;
    let cachet = translate_type(ty)?;
    let too_wide = || Unhandled::new(format!("integer literal {n} does not fit a `{cachet}`"));
    let literal = match cachet.to_string().as_str() {
        "Int8" => Literal::Int8(i8::try_from(n).map_err(|_| too_wide())?),
        "Int16" => Literal::Int16(i16::try_from(n).map_err(|_| too_wide())?),
        "Int32" => Literal::Int32(i32::try_from(n).map_err(|_| too_wide())?),
        "Int64" => Literal::Int64(n),
        "UInt8" => Literal::UInt8(u8::try_from(n).map_err(|_| too_wide())?),
        "UInt16" => Literal::UInt16(u16::try_from(n).map_err(|_| too_wide())?),
        "UInt32" => Literal::UInt32(u32::try_from(n).map_err(|_| too_wide())?),
        "UInt64" => Literal::UInt64(u64::try_from(n).map_err(|_| too_wide())?),
        _ => {
            return Err(Unhandled::new(format!(
                "integer literal {n} typed `{}`, which is not an integer in the model",
                ty.spelled
            )));
        }
    };
    Ok(Expr::Literal(literal))
}

/// A C++ expression whose Cachet counterpart isn't the same shape, matched
/// whole rather than mapped piece by piece.
///
/// The expression counterpart of [`translate_known_block`], and needed for the
/// same reason: the tables map a call to a call and a name to a name, so an
/// idiom that crosses those categories has nowhere to live. One helper per
/// pattern.
fn translate_known_expr(
    ctx: &Ctx<'_>,
    state: &mut State,
    expr: &CppTypedExpr,
) -> Result<Option<Expr>, Unhandled> {
    if let Some(label) = translate_failure_label(ctx, expr) {
        return Ok(Some(label));
    }
    if let Some(label) = translate_label_ref(ctx, expr) {
        return Ok(Some(label));
    }
    if let Some(ty) = translate_output_type(ctx, state, expr)? {
        return Ok(Some(ty));
    }
    Ok(None)
}

/// `output.type()` is two calls in the model.
///
/// C++ folds the conversion into the method: `AutoOutputRegister::type()` is
/// `ValueTypeFromMIRType(output_.type())` (CacheIRCompiler.h:1012), while the model
/// keeps the register's `MIRType` and the conversion to a `JSValueType`
/// (notes/js.cachet:393) apart. A table keyed on the receiver yields one callee, so
/// the composition lives here.
///
/// Keyed on the *C++* type, not the Cachet one: `TypedOrValueRegister::type()`
/// returns the `MIRType` unconverted, so the two spell one method name and mean
/// different things, and both receivers translate to `TypedOrValueReg`.
///
/// Drops the method's `MOZ_ASSERT(!hasValue())` along with the fold -- sound, but
/// weaker than descending into the method would be. See docs/next-steps.md.
fn translate_output_type(
    ctx: &Ctx<'_>,
    state: &mut State,
    expr: &CppTypedExpr,
) -> Result<Option<Expr>, Unhandled> {
    let CppExpr::Call(call) = &expr.value else {
        return Ok(None);
    };
    let CppCallee::Method {
        recv: Some(recv),
        callee,
    } = &call.callee
    else {
        return Ok(None);
    };
    if callee.name != "type" || !call.args.is_empty() {
        return Ok(None);
    }
    if !recv
        .ty
        .as_ref()
        .is_some_and(|ty| ty.scope == AUTO_OUTPUT_REGISTER)
    {
        return Ok(None);
    }

    let call = |target: &str, name: &str, arg: Expr| {
        Expr::Invoke(Call {
            target: Spanned::internal(CachetPath::from_ident(target).nest(Ident::from(name))),
            args: Spanned::internal(vec![Spanned::internal(to_arg(arg))]),
        })
    };
    let reg = translate_expr(ctx, state, recv)?;
    Ok(Some(call(
        "JSValueType",
        "fromMIRType",
        call("TypedOrValueReg", "type", reg),
    )))
}

/// `&ifTrue` is the label `ifTrue`.
///
/// C++ passes a label by address because masm records patch sites in it; the model
/// passes it by name, a label not being a value there. So the `&` has no
/// counterpart and is dropped.
///
/// Narrowly, though: only for a label. An address of anything else is either an
/// out-parameter -- which Cachet spells `out`, and where dropping the `&` would
/// lose the write with nothing to notice it -- or aliasing the model has no
/// counterpart for. Telling those apart needs the callee's parameter constness,
/// which is not extracted, so they stay refused.
fn translate_label_ref(ctx: &Ctx<'_>, expr: &CppTypedExpr) -> Option<Expr> {
    let CppExpr::Unary(u) = &expr.value else {
        return None;
    };
    if u.op != "&" {
        return None;
    }
    let CppExpr::Ref(r) = &u.operand.value else {
        return None;
    };
    if r.ty.scope != LABEL {
        return None;
    }
    // A bare name in argument position, which the parser resolves to a label or a
    // variable (grammar.lalrpop:181) -- the same route `failure.label_` takes.
    Some(Expr::Var(Spanned::internal(CachetPath::from_ident(
        ctx.names.ident(&r.name),
    ))))
}

/// `allocator.defineRegister(masm, resultId)` is `CacheIR::defineInt32Id(resultId)`,
/// and `useRegister` is `use*Id` the same way.
///
/// The callee depends on the *argument's* id type, which no table keyed on the
/// receiver and the method name can express: C++ carries the operand's type as a
/// value -- these take a `TypedOperandId` and read `typedId.type()` back out of it
/// -- where the model, having no overloading, carries it in the name.
/// [`id_definer`] and [`id_user`] hold the correspondence; this finds the argument
/// to key them on.
///
/// The `*ValueRegister` spellings join their families: `ValueId` is the one id that
/// is not a `TypedOperandId`, so C++ needs a second name for it where the model
/// does not.
///
/// Note what the model drops on the defining side: `defineRegister` also allocates
/// a physical register, possibly spilling, and asserts both that the slot was still
/// undefined and that no failure path has been added yet. `defineTypedId` has no
/// body, so none of that is checked. `useTypedId` does have one.
/// Only reached as a declaration's initializer, so the register it yields always
/// gets a name to record against. Of the 522 `allocator.use*`/`define*` calls in
/// `CacheIRCompiler.cpp` that is all but 8, and those 8 are
/// `mozilla::Maybe::emplace`, an idiom outside the subset anyway.
fn translate_allocator_register(
    ctx: &Ctx<'_>,
    state: &mut State,
    expr: &CppTypedExpr,
) -> Result<(Expr, CachetPath), Unhandled> {
    let (family, callee, args) = allocator_register(expr)
        .ok_or_else(|| Unhandled::new(String::from("not a register from `allocator`")))?;

    // `masm` is ambient, leaving the operand id as the only argument that is a
    // value in the model.
    let mut operands = args.iter().filter(|arg| !is_ambient_expr(arg));
    let (Some(id), None) = (operands.next(), operands.next()) else {
        return Err(Unhandled::new(format!(
            "`{}` takes one operand id besides `masm`",
            callee.name
        )));
    };

    let id_ty = translate_type(expr_type(id)?)?;
    let target = family(&id_ty).ok_or_else(|| {
        Unhandled::new(format!("no `{}` of a `{id_ty}` in the model", callee.name))
    })?;
    let call = Expr::Invoke(Call {
        target: Spanned::internal(
            CachetPath::from_ident("CacheIR").nest(Ident::from(target.to_owned())),
        ),
        args: Spanned::internal(vec![Spanned::internal(to_arg(translate_expr(
            ctx, state, id,
        )?))]),
    });
    Ok((call, id_ty))
}

/// The `CacheIR` family a call on `allocator` belongs to, with the callee and its
/// arguments. Syntax only, so it can guard a match arm without translating.
fn allocator_register<'e>(
    expr: &'e CppTypedExpr,
) -> Option<(
    fn(&CachetPath) -> Option<&'static str>,
    &'e FnRef,
    &'e [CppTypedExpr],
)> {
    let CppExpr::Call(call) = &expr.value else {
        return None;
    };
    let CppCallee::Method {
        recv: Some(recv),
        callee,
    } = &call.callee
    else {
        return None;
    };
    if expr_type(recv).ok()?.scope != ALLOCATOR {
        return None;
    }
    let family: fn(&CachetPath) -> Option<&'static str> = match callee.name.as_str() {
        "defineRegister" | "defineValueRegister" => id_definer,
        "useRegister" | "useValueRegister" => id_user,
        _ => return None,
    };
    Some((family, callee, &call.args))
}

/// `failure->label()` is the field access `failure.label_`.
///
/// The model keeps the label as a field, `struct FailurePath { label label_:
/// MASM }`, because it has no choice: a label's kind is the `ir` it belongs to,
/// no Cachet *type* denotes one, and a `fn`'s return is a type. So the C++
/// accessor has no counterpart call to map to.
///
/// A label field can only be read in argument position -- `let l =
/// failure.label_;` is rejected -- but that distinction is the parser's to make,
/// since it rewrites a field access in argument position into a label-or-variable
/// argument (grammar.lalrpop:181). Emitting the field access is enough.
fn translate_failure_label(ctx: &Ctx<'_>, expr: &CppTypedExpr) -> Option<Expr> {
    let CppExpr::Call(call) = &expr.value else {
        return None;
    };
    let CppCallee::Method {
        recv: Some(recv),
        callee,
    } = &call.callee
    else {
        return None;
    };
    if callee.name != "label" || !call.args.is_empty() {
        return None;
    }
    let CppExpr::Ref(r) = &recv.value else {
        return None;
    };
    if r.ty.scope != ["js", "jit", "FailurePath"] {
        return None;
    }

    Some(Expr::from(FieldAccess {
        parent: Spanned::internal(Expr::Var(Spanned::internal(CachetPath::from_ident(
            ctx.names.ident(&r.name),
        )))),
        field: Spanned::internal(Ident::from("label_")),
    }))
}

/// Errors from a sub-expression already point at the narrowest construct that
/// failed, so a span is filled in only where none was set.
fn translate_expr(
    ctx: &Ctx<'_>,
    state: &mut State,
    expr: &CppTypedExpr,
) -> Result<Expr, Unhandled> {
    translate_expr_value(ctx, state, expr).map_err(|e| match e.span {
        CppSpan::Unknown => e.at(&expr.span),
        _ => e,
    })
}

fn translate_expr_value(
    ctx: &Ctx<'_>,
    state: &mut State,
    expr: &CppTypedExpr,
) -> Result<Expr, Unhandled> {
    if let Some(known) = translate_known_expr(ctx, state, expr)? {
        return Ok(known);
    }
    match &expr.value {
        // An emitter's `AutoOutputRegister output(*this);` is a local whose
        // declaration is dropped, so a reference to it is the result register
        // itself, which the model keeps in a `var`.
        //
        // Only a local. A *parameter* of that type is a helper being handed the
        // register -- `EmitStoreBoolean(masm, b, const AutoOutputRegister& output)`
        // -- and that parameter survives into the signature, so the name carries
        // over like any other.
        CppExpr::Ref(r) if r.kind == RefKind::Local && r.ty.scope == AUTO_OUTPUT_REGISTER => {
            Ok(Expr::Var(Spanned::internal(
                CachetPath::from_ident("CacheIR").nest(Ident::from("outputReg")),
            )))
        }
        CppExpr::Ref(r) => match r.kind {
            // An instruction's offset parameter, which stands for a field the
            // model already has typed. The model has no notion of an offset, so
            // there is nothing to translate this to; the `StubFieldOffset`
            // construction consumes it, and any other use is refused.
            RefKind::Param
                if ctx
                    .instruction
                    .as_ref()
                    .is_some_and(|i| i.offsets.contains_key(&r.name)) =>
            {
                Err(Unhandled::new(format!(
                    "stub field offset `{}`: the model has no offsets",
                    r.name
                )))
            }
            // A local standing for a dropped `StubFieldOffset` construction.
            // Its only legitimate use is as `emitLoadStubField`'s first
            // argument, which reads the name directly rather than translating
            // it, so reaching here means the C++ used the offset for something
            // the model can't express.
            //
            // Letting it through would be worse than dangling: the op's field
            // parameter often carries the very same name the C++ gave the local
            // -- `val` in `LoadInt32Constant` -- so the reference would
            // silently resolve to the field itself and type check.
            RefKind::Local if state.stub_fields.contains_key(&r.name) => Err(Unhandled::new(
                format!("stub field offset `{}`: the model has no offsets", r.name),
            )),
            // A parameter the generated signature binds under another name, the
            // two being written independently: the yaml says `result` where the
            // definition says `resultId`.
            RefKind::Param if ctx.cachet_param(&r.name).is_some() => {
                let param = ctx.cachet_param(&r.name).unwrap();
                Ok(Expr::Var(Spanned::internal(CachetPath::from_ident(
                    param.name.clone(),
                ))))
            }
            // In scope in the translated body, so the name carries over -- under
            // whatever the unit decided to call it, which differs only for a Cachet
            // keyword.
            RefKind::Param | RefKind::Local => Ok(Expr::Var(Spanned::internal(
                CachetPath::from_ident(ctx.names.ident(&r.name)),
            ))),
            // Declared elsewhere, so the name alone doesn't say what it refers
            // to. Emitting it unqualified would either dangle or -- if a local
            // happened to share the name -- capture it, so it takes a mapping.
            //
            // A named constant is what these turn out to be, so they go through
            // the same table as enum constants: on another architecture the very
            // same constant *is* an enumerator.
            RefKind::Global => {
                let ty = r.ty.scope.last().map(String::as_str).unwrap_or_default();
                let (ty, name) = translate_enum_const(ty, &r.name).ok_or_else(|| {
                    Unhandled::new(format!("global `{}`: {}", r.name, r.ty.spelled))
                })?;
                Ok(Expr::Var(Spanned::internal(
                    CachetPath::from_ident(ty).nest(Ident::from(name)),
                )))
            }
            // A field is a `var` on the enclosing item, and Cachet requires it
            // qualified: `field` alone doesn't resolve inside an `op`.
            RefKind::Field => {
                let parent = ctx
                    .parent()
                    .ok_or_else(|| Unhandled::new(format!("field `{}` outside an ir", r.name)))?;
                Ok(Expr::Var(Spanned::internal(
                    parent.nest(ctx.names.ident(&r.name)),
                )))
            }
        },

        // A writer call used as a value has to yield an operand id, and only
        // the wrapper can: `emit` is a statement, so the op alone cannot stand
        // in an expression.
        CppExpr::Call(call) if writer_call(ctx, call).is_some() => {
            let callee = writer_call(ctx, call).unwrap();
            let method = callee.name.as_str();
            let op = resolve_op(ctx.ops, method)?;

            let target = if op.custom_writer {
                // `custom_writer` means the public method is hand-written
                // (CacheIRWriter.h), so it is translated like any other helper
                // rather than derived from the yaml -- its arity and return type
                // are its own, not the op's. Translating it as a free function is
                // sound because it reads no writer state: only its parameters and
                // the generated method behind it, which the yaml names `<Op>_`.
                state.needed.push(Needed::Cpp(callee.clone()));
                CachetPath::from_ident(Ident::from(method.to_owned()))
            } else {
                check_arity(op, method, call.args.len())?;
                let Some(helper) = helper_sig(op)? else {
                    return Err(Unhandled::new(format!(
                        "`writer.{method}`: op `{}` has no result operand, so the \
                         call yields nothing to use as a value",
                        op.name
                    )));
                };
                // The wrapper doesn't exist in C++ -- it stands in for the
                // generated writer method -- so it is synthesized from the yaml.
                state.needed.push(Needed::Wrapper(op.name.clone()));
                CachetPath::from_ident(helper.ident)
            };

            let args = call
                .args
                .iter()
                .map(|arg| Ok(Spanned::internal(to_arg(translate_expr(ctx, state, arg)?))))
                .collect::<Result<Vec<_>, Unhandled>>()?;
            Ok(Expr::Invoke(Call {
                target: Spanned::internal(target),
                args: Spanned::internal(args),
            }))
        }

        CppExpr::Call(call) => match &call.callee {
            CppCallee::Method {
                recv: Some(recv),
                callee,
            } => {
                let recv_ty = expr_type(recv)?;
                let (ty, name) = translate_method(recv_ty, &callee.name).ok_or_else(|| {
                    Unhandled::new(format!("method `{}` on `{}`", callee.name, recv_ty.spelled))
                })?;
                let target = CachetPath::from_ident(ty).nest(Ident::from(name));

                // A value receiver leads, as `impl Value { fn isNumber(value:
                // Value) }` takes its subject as the first argument. An ambient
                // one is no value at all, so there is nothing to pass.
                let mut args = Vec::new();
                if translate_type(recv_ty).is_ok() {
                    args.push(Spanned::internal(to_arg(translate_expr(ctx, state, recv)?)));
                }
                // An ambient entity is no value in the model, so where C++ hands
                // one along -- `useValueRegister(masm, inputId)` -- it is dropped.
                for arg in call.args.iter().filter(|arg| !is_ambient_expr(arg)) {
                    args.push(Spanned::internal(to_arg(translate_expr(ctx, state, arg)?)));
                }
                Ok(Expr::Invoke(Call {
                    target: Spanned::internal(target),
                    args: Spanned::internal(args),
                }))
            }
            CppCallee::Method { recv: None, callee } => Err(Unhandled::new(format!(
                "method `{}` on an implicit `this`",
                callee.name
            ))),
            CppCallee::Free(callee) => {
                // Modelled, or translated: on a miss the C++ definition is
                // recorded for the caller to translate, and the call is emitted
                // against its own name.
                let target = translate_free(&callee.name).unwrap_or_else(|| {
                    state.needed.push(Needed::Cpp(callee.clone()));
                    CachetPath::from_ident(Ident::from(callee.name.clone()))
                });
                let args = call
                    .args
                    .iter()
                    // An ambient entity isn't a value, so it isn't passed.
                    .filter(|arg| !is_ambient_expr(arg))
                    .map(|arg| Ok(Spanned::internal(to_arg(translate_expr(ctx, state, arg)?))))
                    .collect::<Result<Vec<_>, Unhandled>>()?;
                Ok(Expr::Invoke(Call {
                    target: Spanned::internal(target),
                    args: Spanned::internal(args),
                }))
            }
        },

        CppExpr::Unary(unary) => {
            let kind = translate_unary_oper(&unary.op)
                .ok_or_else(|| Unhandled::new(format!("unary `{}`", unary.op)))?;
            Ok(Expr::Negate(Box::new(NegateExpr {
                kind: Spanned::internal(kind),
                expr: Spanned::internal(translate_expr(ctx, state, &unary.operand)?),
            })))
        }

        CppExpr::Binary(binary) => {
            let oper = translate_bin_oper(&binary.op)
                .ok_or_else(|| Unhandled::new(format!("binary `{}`", binary.op)))?;
            Ok(Expr::BinOper(Box::new(BinOperExpr {
                oper: Spanned::internal(oper),
                lhs: Spanned::internal(translate_expr(ctx, state, &binary.lhs)?),
                rhs: Spanned::internal(translate_expr(ctx, state, &binary.rhs)?),
            })))
        }

        CppExpr::Construct(c) => translate_retype(ctx, state, c),
        CppExpr::EnumConst(e) => {
            let (ty, name) = translate_enum_const(&e.ty, &e.name)
                .ok_or_else(|| Unhandled::new(format!("enum constant `{}::{}`", e.ty, e.name)))?;
            Ok(Expr::Var(Spanned::internal(
                CachetPath::from_ident(ty).nest(Ident::from(name)),
            )))
        }
        // Typed by what the C++ says the literal is, not by how big the value
        // happens to be. clang gives every expression a type, so a `uint32_t` 1 is a
        // `UInt32` rather than an `Int32` that fits -- which is the difference
        // between matching the parameter it is passed to and failing to type check.
        CppExpr::Lit(CppLit::Int(n)) => int_literal(expr.ty.as_ref(), *n),
        CppExpr::Lit(CppLit::Double(d)) => Ok(Expr::Literal(Literal::Double(*d))),
        // `true` and `false` are built-in variables, not literals
        // (built_in.rs:206).
        CppExpr::Lit(CppLit::Bool(b)) => Ok(Expr::Var(Spanned::internal(CachetPath::from_ident(
            if *b { "true" } else { "false" },
        )))),
        // Cachet's literals are numeric, and it models no string type.
        CppExpr::Lit(CppLit::Str(s)) => Err(Unhandled::new(format!("string literal {s:?}"))),
        CppExpr::This => Err(Unhandled::new(String::from("`this`"))),
    }
}

/// One C++ statement can yield several, so this returns a list.
fn translate_stmt(
    ctx: &Ctx<'_>,
    state: &mut State,
    stmt: &CppSpanned<CppStmt>,
) -> Result<Vec<Spanned<Stmt>>, Unhandled> {
    translate_stmt_values(ctx, state, stmt).map_err(|e| match e.span {
        CppSpan::Unknown => e.at(&stmt.span),
        _ => e,
    })
}

fn translate_stmt_values(
    ctx: &Ctx<'_>,
    state: &mut State,
    stmt: &CppSpanned<CppStmt>,
) -> Result<Vec<Spanned<Stmt>>, Unhandled> {
    // An idiom first: the plain forms below would translate its pieces literally.
    if let Some(known) = translate_known_stmt(ctx, state, stmt)? {
        return Ok(known);
    }
    match &stmt.value {
        CppStmt::Return(ret) => {
            // A stub generator returns an `AttachDecision`, which is protocol
            // with the dispatcher rather than a value: whether it attached is
            // the dispatcher's business, and `ReturnFromIC` comes from a
            // `writer.returnFromIC()` call, not from returning `Attach`.
            let value = if ctx.is_stub_generator() {
                None
            } else if ctx.instruction.is_some() {
                // An instruction's `bool` is protocol with the compiler loop --
                // `true` means the code was emitted, `false` that it gave up -- and
                // the model has neither notion, so the value goes while the `return`
                // stays, an early one being real control flow.
                //
                // Keyed on `instruction` rather than on the class, which would also
                // catch a `CacheIRCompiler` method translated as a plain helper.
                //
                // `false` is refused rather than read as success: the out-of-memory
                // path from `addFailurePath` is consumed by its own idiom, so a
                // `false` arriving here is something the model cannot express, and
                // calling it success would verify a path that does not exist.
                match ret.value.as_ref().map(|v| &v.value) {
                    None | Some(CppExpr::Lit(CppLit::Bool(true))) => None,
                    Some(_) => {
                        return Err(Unhandled::new(String::from(
                            "an instruction returning anything but `true`",
                        )));
                    }
                }
            } else {
                ret.value
                    .as_ref()
                    .map(|v| translate_expr(ctx, state, v))
                    .transpose()?
            };
            // Leaving the op, so anything outstanding is discharged first, whether
            // this block took it out or inherited it.
            let mut stmts = discharge(state.scopes.on_return());
            stmts.push(Spanned::internal(Stmt::Ret(RetStmt {
                value: Spanned::internal(value),
            })));
            Ok(stmts)
        }
        CppStmt::If(s) => Ok(vec![Spanned::internal(Stmt::If(translate_if(
            ctx, state, s,
        )?))]),
        // Cachet has no switch, so each case becomes a rung of an `else if` chain
        // testing the scrutinee against every value sharing that case's body, and
        // `default:` becomes the final `else`.
        //
        // A chain rather than a run of separate `if`s, so the lowering does not
        // depend on case bodies ending in a `return`. That leaves
        // `extract_switch`'s termination check doing only its real job: refusing
        // fall-through, the one thing a chain cannot express.
        CppStmt::Switch(s) => {
            // Forward, so gaps are recorded in source order; the chain itself has
            // to be assembled from the inside out.
            let mut arms = Vec::new();
            for case in &s.cases {
                arms.push((
                    switch_case_cond(ctx, state, &s.scrutinee, &case.values)?,
                    translate_block(ctx, state, &case.body)?,
                ));
            }
            let default = s
                .default
                .as_ref()
                .map(|default| translate_block(ctx, state, default))
                .transpose()?;

            let mut else_ = default.map(ElseClause::Else);
            for (cond, then) in arms.into_iter().rev() {
                else_ = Some(ElseClause::ElseIf(Box::new(CachetIfStmt {
                    cond: Spanned::internal(cond),
                    then,
                    else_,
                })));
            }
            // The outermost rung is nobody's else clause, so it comes back out.
            Ok(match else_ {
                Some(ElseClause::ElseIf(if_stmt)) => vec![Spanned::internal(Stmt::If(*if_stmt))],
                // A `default:` and no cases at all.
                Some(ElseClause::Else(block)) => block.stmts,
                None => Vec::new(),
            })
        }
        // A freestanding block, which Cachet has too. `translate_block` pushes a
        // scope, so what the block's declarations owe is discharged at its closing
        // brace rather than the enclosing one.
        CppStmt::Block(body) => Ok(vec![Spanned::internal(Stmt::from(translate_block(
            ctx, state, body,
        )?))]),
        // `MOZ_CRASH("..")` says control never reaches here, which is exactly
        // `unreachable`. The reason string has no counterpart, so it is kept as a
        // comment.
        CppStmt::Crash(c) => {
            let mut stmts = Vec::new();
            if let Some(reason) = &c.reason {
                stmts.push(Spanned::internal(Stmt::Comment(Comment {
                    text: format!("MOZ_CRASH({reason:?})"),
                })));
            }
            stmts.push(Spanned::internal(Stmt::Unreachable));
            Ok(stmts)
        }
        // Cachet's `let` always binds a value, so a C++ declaration without an
        // initializer -- `Label done;` -- has no counterpart.
        CppStmt::Let(l) => {
            let init = l.init.as_ref().ok_or_else(|| {
                Unhandled::new(format!(
                    "declaration of `{}` without an initializer",
                    l.name
                ))
            })?;
            Ok(vec![Spanned::internal(Stmt::Let(LetStmt {
                lhs: LocalVar {
                    ident: Spanned::internal(ctx.names.ident(&l.name)),
                    is_mut: false,
                    // Inferred, as the hand-written models leave it.
                    type_: None,
                },
                rhs: Spanned::internal(translate_expr(ctx, state, init)?),
            }))])
        }
        // An assertion states what the code believes about itself. Dropping one
        // gives up a proof obligation, so the model is weaker than the C++ but
        // never disagrees with it.
        CppStmt::Assert(_) => Err(Unhandled::elided(String::from("assertion"))),
        // `EmitStoreBoolean(masm, false, output);` -- a call whose `void` the C++
        // discards, which Cachet spells the same way. Only a free function: a call
        // on a receiver in statement position is one of the idioms above, or
        // nothing.
        CppStmt::Expr(e)
            if matches!(&e.value, CppExpr::Call(call)
                if matches!(call.callee, CppCallee::Free(_))) =>
        {
            Ok(vec![Spanned::internal(Stmt::Expr(translate_expr(
                ctx, state, e,
            )?))])
        }
        CppStmt::Expr(_) => Err(Unhandled::new(String::from("expression statement"))),
    }
}

/// A C++ statement whose Cachet counterpart is not the same shape.
///
/// The statement counterpart of [`translate_known_expr`] and
/// [`translate_known_block`], and the boundary the three of them draw: an idiom
/// lives here, a plain C++ form lives in [`translate_stmt_values`]. Nothing
/// outside these three is a special case.
///
/// `Ok(None)` when the statement is none of these shapes. An `Err` means it *is*
/// one and could not be translated, which propagates rather than falling through:
/// the refusal names the idiom instead of whatever the general path would have
/// made of it.
fn translate_known_stmt(
    ctx: &Ctx<'_>,
    state: &mut State,
    stmt: &CppSpanned<CppStmt>,
) -> Result<Option<Vec<Spanned<Stmt>>>, Unhandled> {
    Ok(Some(match &stmt.value {
        // `StubFieldOffset val(valOffset, StubField::Type::RawInt32);` pairs the
        // offset with the kind the C++ lost. The model never lost it -- the op's
        // parameter is already an `Int32Field` -- so the construction says
        // nothing and is dropped, leaving `val` standing for that parameter.
        CppStmt::Let(l) if stub_field_binding(ctx, l).is_some() => {
            let (local, field) = stub_field_binding(ctx, l).unwrap();
            state.stub_fields.insert(local, field);
            state.gaps.push(Gap {
                fidelity: Fidelity::Elided,
                what: String::from("StubFieldOffset"),
                span: stmt.span.clone(),
            });
            Vec::new()
        }

        // `Label ifTrue, done;` -- default-constructed, and clang gives one
        // declaration per declarator, so two statements. The model declares a label
        // rather than constructing one, and names the `ir` it belongs to: every
        // label a stub jumps to is a position in the code it emits.
        CppStmt::Let(l) if is_label_decl(l) => {
            vec![Spanned::internal(Stmt::Label(LabelStmt {
                label: CachetLabel {
                    ident: Spanned::internal(ctx.names.ident(&l.name)),
                    ir: Spanned::internal(CachetPath::from_ident("MASM")),
                },
            }))]
        }

        // `AutoOutputRegister output(*this);` reserves the result register. The
        // model has no allocator state to reserve in, so the declaration says
        // nothing and is dropped; references to `output` become the register
        // itself.
        CppStmt::Let(l) if is_output_register_decl(l) => {
            state.gaps.push(Gap {
                fidelity: Fidelity::Elided,
                what: String::from("AutoOutputRegister: the model has no allocator state"),
                span: stmt.span.clone(),
            });
            Vec::new()
        }

        // `Register output = allocator.defineRegister(masm, resultId);` binds a
        // register, and the operand id it came from is the only record of what kind
        // of value it holds -- a fact the C++ `Register` type does not carry and
        // that later masm calls need.
        CppStmt::Let(l)
            if l.init
                .as_ref()
                .is_some_and(|init| allocator_register(init).is_some()) =>
        {
            let init = l.init.as_ref().unwrap();
            let (rhs, id_ty) = translate_allocator_register(ctx, state, init)?;
            state.scopes.insert_register(l.name.clone(), id_ty);
            vec![Spanned::internal(Stmt::Let(LetStmt {
                lhs: LocalVar {
                    ident: Spanned::internal(ctx.names.ident(&l.name)),
                    is_mut: false,
                    type_: None,
                },
                rhs: Spanned::internal(rhs),
            }))]
        }

        // `AutoScratchRegister scratch2(allocator, masm);` takes a register out of
        // the allocator, and its destructor puts it back. The model spells the first
        // half `CacheIR::allocateReg()`; the second has no C++ line of its own, so it
        // becomes an obligation on the enclosing block.
        //
        // The wrapper itself is not represented: it denotes the register
        // (`operator Register()`), which is why `translate_type` sends it to `Reg`
        // and why references to `scratch2` need nothing special.
        CppStmt::Let(l) if is_scratch_register_decl(l) => {
            let reg = ctx.names.ident(&l.name);
            state.scopes.acquire_reg(reg);
            vec![Spanned::internal(Stmt::Let(LetStmt {
                lhs: LocalVar {
                    ident: Spanned::internal(reg),
                    is_mut: false,
                    type_: None,
                },
                rhs: Spanned::internal(invoke(
                    CachetPath::from_ident("CacheIR").nest(Ident::from("allocateReg")),
                )),
            }))]
        }

        // `ScratchTagScope tag(masm, input);` takes the tag register, and its
        // destructor gives it back. Shaped like the scratch declaration above, with
        // a different pair of model calls, and refusing a nested one: the model has
        // only the one tag register to give.
        CppStmt::Let(l) if is_tag_scope_decl(l) => {
            if !state.scopes.acquire_tag_scope() {
                return Err(Unhandled::new(String::from(
                    "a second `ScratchTagScope`: the model has one tag register, R11",
                )));
            }
            vec![Spanned::internal(Stmt::Let(LetStmt {
                lhs: LocalVar {
                    ident: Spanned::internal(ctx.names.ident(&l.name)),
                    is_mut: false,
                    type_: None,
                },
                rhs: Spanned::internal(invoke(
                    CachetPath::from_ident("CacheIR").nest(Ident::from("allocateScratchReg")),
                )),
            }))]
        }

        // `ScratchTagScopeRelease _(&tag);` gives the tag register back for the
        // length of its block. Binds nothing: the declared variable is never named
        // again, the construction itself being the effect.
        CppStmt::Let(l) if is_tag_scope_release_decl(l) => {
            state.scopes.lend_tag_scope();
            vec![Spanned::internal(Stmt::Expr(invoke(
                CachetPath::from_ident("CacheIR").nest(Ident::from("releaseScratchReg")),
            )))]
        }

        // `writer.compareDoubleResult(..)` records a CacheIR op, which Cachet
        // spells `emit CacheIR::CompareDoubleResult(..)`. Arguments carry over
        // unchanged.
        //
        // The op's *semantics* live in `CacheIRCompiler::emit<Op>`, which is a
        // separate unit to translate; the call is not chased into it.
        CppStmt::Expr(CppTypedExpr {
            value: CppExpr::Call(call),
            ..
        }) if writer_call(ctx, call).is_some() => {
            let method = writer_call(ctx, call).unwrap().name.as_str();
            let op = resolve_op(ctx.ops, method)?;
            check_arity(op, method, call.args.len())?;

            // An op that allocates a result is reachable only through its
            // wrapper, and here the wrapper's value would be dropped -- which
            // Cachet has no way to spell, since a statement must have type
            // `Unit` (type_checker.rs:1218) and there is no name to bind to.
            if let Some(helper) = helper_sig(op)? {
                return Err(Unhandled::new(format!(
                    "`writer.{method}` allocates a `{}` that C++ discards",
                    helper.ret
                )));
            }

            let args = call
                .args
                .iter()
                .map(|arg| Ok(Spanned::internal(to_arg(translate_expr(ctx, state, arg)?))))
                .collect::<Result<Vec<_>, Unhandled>>()?;
            vec![Spanned::internal(Stmt::Emit(Call {
                target: Spanned::internal(op_path(op, ctx.op_ir)),
                args: Spanned::internal(args),
            }))]
        }
        // `emitLoadStubField(val, reg)` reads the field into a register. The C++
        // is generic and switches on the kind; the model has one function per
        // kind, Cachet having no overloading, so the kind recorded above picks it.
        CppStmt::Expr(CppTypedExpr {
            value: CppExpr::Call(call),
            ..
        }) if is_load_stub_field(call) => {
            let [field, dst] = call.args.as_slice() else {
                return Err(Unhandled::new(format!(
                    "`emitLoadStubField` takes 2 arguments, called with {}",
                    call.args.len()
                )));
            };
            let CppExpr::Ref(r) = &field.value else {
                return Err(Unhandled::new(String::from(
                    "`emitLoadStubField`: first argument is not a name",
                )));
            };
            let field = state.stub_fields.get(&r.name).cloned().ok_or_else(|| {
                Unhandled::new(format!(
                    "`emitLoadStubField`: `{}` is not a known stub field",
                    r.name
                ))
            })?;
            let ty = ctx
                .cachet_param_ty(&field)
                .ok_or_else(|| Unhandled::new(format!("`{field}` is not an operand of the op")))?;
            let loader = field_loader(ty)
                .ok_or_else(|| Unhandled::new(format!("no loader for a `{ty}` in the model")))?;

            vec![Spanned::internal(Stmt::Expr(Expr::Invoke(Call {
                target: Spanned::internal(
                    CachetPath::from_ident("CacheIR").nest(Ident::from(loader.to_owned())),
                ),
                args: Spanned::internal(vec![
                    Spanned::internal(to_arg(Expr::Var(Spanned::internal(
                        CachetPath::from_ident(field),
                    )))),
                    Spanned::internal(to_arg(translate_expr(ctx, state, dst)?)),
                ]),
            })))]
        }

        CppStmt::Expr(CppTypedExpr {
            value: CppExpr::Call(call),
            ..
        }) if dropped_call(ctx, call).is_some() => {
            // Recorded even though nothing is emitted, so the tally of what the
            // module leaves out stays complete.
            state.gaps.push(Gap {
                fidelity: Fidelity::Elided,
                what: String::from(dropped_call(ctx, call).unwrap()),
                span: stmt.span.clone(),
            });
            Vec::new()
        }
        // `masm.branchTestNull(..)` emits machine code, which Cachet spells
        // `emit MASM::BranchTestNull(..)`. The receiver is ambient, so only the
        // arguments carry over.
        CppStmt::Expr(CppTypedExpr {
            value: CppExpr::Call(_),
            ..
        }) if masm_call(&stmt.value, &state.scopes).is_some() => {
            // One call can mean several statements, so this is a loop: see
            // `masm_call`.
            let masm_stmts = masm_call(&stmt.value, &state.scopes).unwrap();
            let mut stmts = Vec::new();
            for masm_stmt in masm_stmts {
                stmts.push(Spanned::internal(match masm_stmt {
                    MasmStmt::Emit { op, args } => {
                        let args = args
                            .iter()
                            .map(|arg| {
                                Ok(Spanned::internal(to_arg(translate_expr(ctx, state, arg)?)))
                            })
                            .collect::<Result<Vec<_>, Unhandled>>()?;
                        Stmt::Emit(Call {
                            target: Spanned::internal(
                                CachetPath::from_ident("MASM").nest(Ident::from(op.to_owned())),
                            ),
                            args: Spanned::internal(args),
                        })
                    }
                    // `translate_label_ref` strips the `&` and checks the target
                    // really is a label, so anything else is refused here rather
                    // than bound as if it were one.
                    MasmStmt::Bind { label } => {
                        let Expr::Var(label) = translate_expr(ctx, state, label)? else {
                            return Err(Unhandled::new(String::from(
                                "`bind` of something that is not a label",
                            )));
                        };
                        Stmt::Bind(BindStmt { label })
                    }
                }));
            }
            stmts
        }
        _ => return Ok(None),
    }))
}

/// The C++ lines a span covers.
///
/// Whole lines rather than an exact slice: a clang range ends at the *start* of
/// its last token, so `[start.offset, end.offset)` would cut it short.
fn quote(span: &CppSpan) -> Option<String> {
    let CppSpan::Known { file, start, end } = span else {
        return None;
    };
    let text = std::fs::read_to_string(file).ok()?;
    let lines: Vec<&str> = text
        .lines()
        .skip(start.line.checked_sub(1)? as usize)
        .take((end.line.checked_sub(start.line)? + 1) as usize)
        .collect();
    // Drop the common indentation, which is the C++ nesting, not the statement's.
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    Some(
        lines
            .iter()
            .map(|l| l.get(indent..).unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// An untranslated statement, kept as a comment holding the C++ it stood for.
///
/// The quote spans the whole statement, while the reason names the innermost
/// construct that failed -- quoting that would cut the statement mid-expression.
fn unhandled_comment(e: &Unhandled, stmt: &CppSpan) -> Comment {
    let text = match quote(stmt) {
        Some(cpp) => format!("unhandled {}:\n{cpp}", e.what),
        None => format!("unhandled {}", e.what),
    };
    Comment { text }
}

/// An `if`, keeping an `else if` chain as a chain.
///
/// C++ spells `else if` as an else branch holding a single `if`, which
/// [`extract_branch`](crate::cpp_subset) wraps into a one-statement block. Cachet
/// has the same form, so recognizing that shape keeps the output a chain instead of
/// nesting a block per rung -- and skips a scope the C++ never opened.
fn translate_if(
    ctx: &Ctx<'_>,
    state: &mut State,
    s: &CppIfStmt,
) -> Result<CachetIfStmt, Unhandled> {
    let cond = Spanned::internal(translate_expr(ctx, state, &s.cond)?);
    let then = translate_block(ctx, state, &s.then)?;
    let else_ = match &s.els {
        None => None,
        Some(els) => match els.stmts.as_slice() {
            [only] => match &only.value {
                CppStmt::If(inner) => Some(ElseClause::ElseIf(Box::new(translate_if(
                    ctx, state, inner,
                )?))),
                _ => Some(ElseClause::Else(translate_block(ctx, state, els)?)),
            },
            _ => Some(ElseClause::Else(translate_block(ctx, state, els)?)),
        },
    };
    Ok(CachetIfStmt { cond, then, else_ })
}

/// `scrutinee == v1 || scrutinee == v2 || ..`: one switch case's test, over every
/// label sharing its body.
fn switch_case_cond(
    ctx: &Ctx<'_>,
    state: &mut State,
    scrutinee: &CppTypedExpr,
    values: &[CppTypedExpr],
) -> Result<Expr, Unhandled> {
    let mut cond: Option<Expr> = None;
    for value in values {
        let matches = Expr::BinOper(Box::new(BinOperExpr {
            oper: Spanned::internal(BinOper::Compare(CompareBinOper::Eq)),
            lhs: Spanned::internal(translate_expr(ctx, state, scrutinee)?),
            rhs: Spanned::internal(translate_expr(ctx, state, value)?),
        }));
        cond = Some(match cond {
            None => matches,
            Some(prev) => Expr::BinOper(Box::new(BinOperExpr {
                oper: Spanned::internal(BinOper::Logical(LogicalBinOper::Or)),
                lhs: Spanned::internal(prev),
                rhs: Spanned::internal(matches),
            })),
        });
    }
    // `case:` with no value is not a thing clang produces, but the type allows it.
    cond.ok_or_else(|| Unhandled::new(String::from("switch case with no value to match")))
}

/// A block of statements. Cachet blocks can also end in a bare tail expression;
/// C++ always returns explicitly, so `value` is always `None`.
///
/// A statement that can't be translated becomes a comment rather than failing
/// the whole body.
/// A run of C++ statements that says one thing together, translated as a unit.
///
/// Some C++ idioms don't survive being taken a statement at a time: an
/// out-parameter is a declaration, a call, and a test of that call, which in
/// Cachet is one `let`. Matching on the sequence is the only way to see it.
///
/// Returns how many statements were consumed and what they became, or `None`
/// where no pattern applies and the statements translate one by one. One helper
/// per pattern, each matching a prefix of `stmts`.
/// The statements consumed, and what they become. An `Err` consumed them too, so
/// the caller advances either way.
fn translate_known_block(
    state: &mut State,
    stmts: &[CppSpanned<CppStmt>],
) -> Option<(usize, Result<Vec<Spanned<Stmt>>, Unhandled>)> {
    let (consumed, translated) = translate_add_failure_path(stmts)?;
    // Outstanding from here until this block releases it or a `return` does: the
    // model has no `nextOp()` to clear the flag between instructions.
    if !state.scopes.acquire_failure_path() {
        return Some((
            consumed,
            Err(Unhandled::invalid(
                "a second failure path while one is outstanding, which \
                 `setAddedFailurePath` asserts against: \"multiple failure paths \
                 for instruction\"",
            )),
        ));
    }
    Some((consumed, Ok(translated)))
}

/// ```text
/// FailurePath* failure;
/// if (!addFailurePath(&failure)) {
///   return false;
/// }
/// ```
///
/// becomes `let failure = CacheIR::addFailurePath();`.
///
/// The `bool` is whether the path could be allocated, and the early return
/// propagates OOM: the stub is abandoned and none is attached, which is always
/// safe. The model has no OOM, so what is left is the declaration and the call,
/// which together are one binding.
///
/// Matched verbatim rather than in pieces. The shape is boilerplate repeated
/// across every emitter, and recognizing it exactly means an emitter that does
/// something else with the result still refuses rather than being mistranslated.
fn translate_add_failure_path(
    stmts: &[CppSpanned<CppStmt>],
) -> Option<(usize, Vec<Spanned<Stmt>>)> {
    let [decl, test, ..] = stmts else {
        return None;
    };

    // `FailurePath* failure;` -- a pointer, and no initializer.
    let CppStmt::Let(decl) = &decl.value else {
        return None;
    };
    if decl.ty.scope != ["js", "jit", "FailurePath"]
        || decl.ty.indirection != Indirection::Ptr
        || decl.init.is_some()
    {
        return None;
    }

    // `if (!addFailurePath(&failure)) { return false; }`
    let CppStmt::If(test) = &test.value else {
        return None;
    };
    if test.els.is_some() || !returns_false(&test.then) {
        return None;
    }
    let CppExpr::Unary(negated) = &test.cond.value else {
        return None;
    };
    if negated.op != "!" {
        return None;
    }
    let CppExpr::Call(call) = &negated.operand.value else {
        return None;
    };
    let CppCallee::Method { recv: None, callee } = &call.callee else {
        return None;
    };
    if callee.name != "addFailurePath" || !is_address_of(&call.args, &decl.name) {
        return None;
    }

    Some((
        2,
        vec![Spanned::internal(Stmt::Let(LetStmt {
            lhs: LocalVar {
                ident: Spanned::internal(Ident::from(decl.name.clone())),
                is_mut: false,
                type_: None,
            },
            rhs: Spanned::internal(invoke(
                CachetPath::from_ident("CacheIR").nest(Ident::from("addFailurePath")),
            )),
        }))],
    ))
}

/// A block that is exactly `return false;`.
fn returns_false(block: &CppCompoundStmt) -> bool {
    matches!(
        block.stmts.as_slice(),
        [stmt] if matches!(&stmt.value,
            CppStmt::Return(ret) if matches!(
                ret.value.as_ref().map(|v| &v.value),
                Some(CppExpr::Lit(CppLit::Bool(false)))
            )
        )
    )
}

/// A lone `&name` argument.
fn is_address_of(args: &[CppTypedExpr], name: &str) -> bool {
    matches!(
        args,
        [arg] if matches!(&arg.value,
            CppExpr::Unary(unary) if unary.op == "&" && matches!(
                &unary.operand.value,
                CppExpr::Ref(r) if r.name == name
            )
        )
    )
}

fn translate_block(
    ctx: &Ctx<'_>,
    state: &mut State,
    body: &CppCompoundStmt,
) -> Result<Block, Unhandled> {
    // A C++ declaration is scoped to its block, so what the walk learns about a
    // local has to go out of scope with it.
    state.scopes.push();
    let block = translate_block_stmts(ctx, state, body);
    state.scopes.pop();
    block
}

fn translate_block_stmts(
    ctx: &Ctx<'_>,
    state: &mut State,
    body: &CppCompoundStmt,
) -> Result<Block, Unhandled> {
    let mut stmts = Vec::new();
    let mut rest = body.stmts.as_slice();
    while let [stmt, tail @ ..] = rest {
        // A multi-statement idiom takes precedence, since its statements don't
        // translate on their own.
        if let Some((consumed, translated)) = translate_known_block(state, rest) {
            match translated {
                Ok(translated) => stmts.extend(translated),
                Err(e) => {
                    stmts.push(Spanned::internal(Stmt::from(unhandled_comment(
                        &e, &stmt.span,
                    ))));
                    state.gaps.push(e.into_gap(&stmt.span));
                }
            }
            rest = &rest[consumed..];
            continue;
        }
        match translate_stmt(ctx, state, stmt) {
            Ok(translated) => stmts.extend(translated),
            Err(e) => {
                stmts.push(Spanned::internal(Stmt::from(unhandled_comment(
                    &e, &stmt.span,
                ))));
                state.gaps.push(e.into_gap(&stmt.span));
            }
        }
        rest = tail;
    }
    // Falling out of the block. A `return` discharged its own path already, so this
    // fires only where the block ends by running off the end.
    stmts.extend(discharge(state.scopes.on_scope_end()));
    Ok(Block {
        stmts,
        value: Spanned::internal(None),
    })
}

/// The statements that discharge what leaving a scope owes.
///
/// Inserted, never translated: no C++ line corresponds to any of them.
fn discharge(obligations: Vec<Obligation>) -> Vec<Spanned<Stmt>> {
    obligations
        .into_iter()
        .map(|obligation| match obligation {
            Obligation::ReleaseFailurePath => Spanned::internal(Stmt::Expr(invoke(
                CachetPath::from_ident("CacheIR").nest(Ident::from("releaseFailurePath")),
            ))),
            Obligation::ReleaseScratchReg => Spanned::internal(Stmt::Expr(invoke(
                CachetPath::from_ident("CacheIR").nest(Ident::from("releaseScratchReg")),
            ))),
            // Takes the tag register back. The register it returns is discarded, an
            // expression statement being unit-typed whatever its expression's type
            // (type_checker/ast.rs) -- which is how the model writes it too
            // (notes/cacheir.cachet:1424).
            Obligation::ReacquireScratchReg => Spanned::internal(Stmt::Expr(invoke(
                CachetPath::from_ident("CacheIR").nest(Ident::from("allocateScratchReg")),
            ))),
            Obligation::ReleaseReg(reg) => Spanned::internal(Stmt::Expr(Expr::Invoke(Call {
                target: Spanned::internal(
                    CachetPath::from_ident("CacheIR").nest(Ident::from("releaseReg")),
                ),
                args: Spanned::internal(vec![Spanned::internal(to_arg(Expr::Var(
                    Spanned::internal(CachetPath::from_ident(reg)),
                )))]),
            }))),
        })
        .collect()
}

/// A helper the generators call:
///
/// ```text
/// static bool CanConvertToDoubleForToNumber(const Value& v) {
///   return v.isNumber() || v.isBoolean() || v.isNullOrUndefined();
/// }
/// ```
///
/// becomes
///
/// ```text
/// fn CanConvertToDoubleForToNumber(v: Value) -> Bool {
///   return Value::isNumber(v) || Value::isBool(v) || Value::isNullOrUndefined(v);
/// }
/// ```
///
/// Translated rather than modelled: it has no entry in [`translate_method`], so
/// the translation descends into its definition. A helper that *does* have an
/// entry bottoms out there instead, and never needs translating.
pub fn translate_fn_def(
    ops: &Ops,
    op_ir: OpIr,
    class: Option<ClassRef>,
    fn_def: &FnDef,
) -> Result<(CallableItem, State), Unhandled> {
    // The writer is ambient, so it is dropped rather than translated. The rest keep
    // their C++ names, a helper having no second source of names to reconcile
    // against -- except where the name is a Cachet keyword, since `JSOpToCondition`
    // takes a parameter called `op`, which does not parse.
    let params: Vec<&Param> = fn_def
        .params
        .iter()
        .filter(|param| !is_ambient(&param.ty))
        .collect();
    let names = NameMap::build(
        declared_names(&fn_def.params, &fn_def.body)
            .iter()
            .map(String::as_str),
    );
    let sig = params
        .iter()
        .map(|param| {
            Ok(SigParam {
                name: names.ident(&param.name),
                ty: translate_type(&param.ty).map_err(|e| {
                    Unhandled::new(format!("parameter `{}`: {}", param.name, e.what))
                })?,
            })
        })
        .collect::<Result<Vec<_>, Unhandled>>()?;

    // A helper is top-level, so it has no parent to qualify names against.
    // `state` is created here and returned: its scope is this one definition.
    let ctx = Ctx {
        class,
        ops,
        op_ir,
        // The parameters that survive, so the two lists correspond position by
        // position and a keyword-mangled name can be found from the C++ one.
        cpp_sig: Some(params.iter().map(|param| (*param).clone()).collect()),
        cachet_sig: Some(sig.clone()),
        names,
        instruction: None,
    };

    // Taking a writer is what makes a function emit, so dropping the parameter
    // is what the `emits` clause replaces. A `CacheIRWriter` method takes no
    // such parameter -- it is the writer -- and emits all the same.
    // Which ambient thing a helper is handed says which `ir` it emits to, since that
    // is the only reason to hand it one. A `CacheIRWriter` means CacheIR, a
    // `MacroAssembler` means MASM -- `EmitStoreBoolean(masm, ..)` moves a value into
    // a register, so it emits machine code.
    let takes = |p: fn(&CppType) -> bool| fn_def.params.iter().any(|param| p(&param.ty));
    let emits = if ctx.recv_is_writer() || takes(is_writer) {
        // Taking the writer means emitting ops, so it follows wherever they are.
        Some(op_ir.path())
    } else if takes(is_masm) {
        Some(CachetPath::from_ident("MASM"))
    } else {
        None
    }
    .map(Spanned::internal);

    let params = sig.iter().map(SigParam::to_param).collect();

    // `void` is not a type Cachet has: a callable that returns nothing leaves the
    // return clause off entirely.
    let ret = if fn_def.ret.scope == ["void"] {
        None
    } else {
        Some(Spanned::internal(translate_type(&fn_def.ret).map_err(
            |e| Unhandled::new(format!("return type: {}", e.what)),
        )?))
    };

    let mut state = State::default();
    let body = translate_block(&ctx, &mut state, &fn_def.body)?;

    let item = CallableItem {
        // Kept verbatim, as field and local names are.
        ident: Spanned::internal(Ident::from(fn_def.name.name.clone())),
        attrs: Vec::new(),
        is_unsafe: false,
        params,
        emits,
        ret,
        body: Spanned::internal(Some(body)),
    };
    Ok((item, state))
}

/// `tryAttachNumber` becomes `TryAttachNumber`: Cachet spells ops capitalized,
/// as `emit CacheIR::CompareDoubleResult` in the models does.
fn op_ident(method: &str) -> String {
    let mut chars = method.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `op TryAttachNumber() { <preamble> <body> }`, with the helpers it needs.
fn create_generator_op(
    ctx: &Ctx<'_>,
    gen_def: &MethodDef,
) -> Result<(CallableItem, State), Unhandled> {
    let preamble = translate_preamble(&gen_def.class, &gen_def.def.params)?;
    let mut state = State::default();
    let body = translate_block(ctx, &mut state, &gen_def.def.body)?;

    let item = CallableItem {
        ident: Spanned::internal(Ident::from(op_ident(&gen_def.def.name.name))),
        attrs: Vec::new(),
        is_unsafe: false,
        // No parameters: the operands arrive through the preamble's
        // `defineInputValueId` calls rather than being passed in.
        params: Vec::new(),
        // Inherited from the enclosing `ir`, which already says `emits CacheIR`.
        emits: None,
        // The C++ returns `AttachDecision`, which is the dispatcher's business;
        // an op yields nothing.
        ret: None,
        body: Spanned::internal(Some(Block {
            stmts: preamble.into_iter().chain(body.stmts).collect(),
            value: Spanned::internal(None),
        })),
    };
    Ok((item, state))
}

/// `CompareIRGenerator::tryAttachNumber` becomes
/// `ir CompareIRGenerator emits CacheIR { .. }`: the generator class names the
/// `ir`, and every stub generator emits CacheIR.
pub fn translate_gen_def(
    ops: &Ops,
    op_ir: OpIr,
    gen_def: &MethodDef,
) -> Result<(IrItem, State), Unhandled> {
    // `writer` is how the C++ emits, not state the generator holds: each
    // `writer.foo(..)` becomes an `emit`, so the field itself has no
    // counterpart in the `ir` and is dropped before translating the rest.
    let fields: Vec<Ref> = get_method_def_fields(gen_def)
        .into_iter()
        .filter(|field| field.ty.scope != CACHE_IR_WRITER)
        .collect();
    let var_items = create_field_var_items(&fields)?;
    let ctx = Ctx {
        class: Some(gen_def.class.clone()),
        ops,
        op_ir,
        names: NameMap::build(
            declared_names(&gen_def.def.params, &gen_def.def.body)
                .iter()
                .map(String::as_str),
        ),
        cpp_sig: Some(gen_def.def.params.clone()),
        // The generator's op takes nothing: its `ValOperandId` parameters become
        // `defineInputValueId()` bindings in the body, under the very names the
        // C++ gave the parameters. So the lists have nothing to say to each
        // other here, and the differing arity is what keeps them apart.
        cachet_sig: Some(Vec::new()),
        instruction: None,
    };

    // The `ir` is named after the class, so it has to be a class we know is a
    // stub generator rather than any method's owner.
    if !ctx.is_stub_generator() {
        return Err(Unhandled::new(format!(
            "`{}`: not a known stub generator class",
            gen_def.class
        )));
    }
    let (op, state) = create_generator_op(&ctx, gen_def)?;
    let generator_op = Item::Op(op);

    // The `var`s first, then the single `op`, as the hand-written models order
    // them: state before the code that reads it.
    let items = var_items
        .into_iter()
        .chain([Spanned::internal(generator_op)])
        .collect();

    Ok((
        IrItem {
            // Spans are `internal` throughout: these nodes are synthesized, so
            // there is no Cachet source location to point at.
            ident: Spanned::internal(Ident::from(gen_def.class.name().to_owned())),
            // Whichever `ir` the ops it emits are in.
            emits: Some(Spanned::internal(op_ir.path())),
            items,
        },
        state,
    ))
}

/// A translated module and how far it fell short of the C++.
pub struct Translation {
    /// `js::jit::CompareIRGenerator::tryAttachInt32`.
    pub unit: String,
    pub module: Mod,
    pub gaps: Vec<Gap>,
}

impl Translation {
    /// Whether the module models the C++, and so may be verified.
    ///
    /// Elided gaps don't count: they weaken the proof without changing what the
    /// model says. Anything else does, anywhere, including in a helper -- the
    /// generator's own body may be perfect and still call into a lie.
    pub fn is_faithful(&self) -> bool {
        !self.gaps.iter().any(|g| g.fidelity != Fidelity::Elided)
    }

    pub fn failures(&self) -> impl Iterator<Item = &Gap> {
        self.gaps.iter().filter(|g| g.fidelity == Fidelity::Failed)
    }

    /// Reported apart from [`Translation::failures`]: the same verdict, a different
    /// place to go looking.
    pub fn invalid(&self) -> impl Iterator<Item = &Gap> {
        self.gaps.iter().filter(|g| g.fidelity == Fidelity::Invalid)
    }

    pub fn elisions(&self) -> impl Iterator<Item = &Gap> {
        self.gaps.iter().filter(|g| g.fidelity == Fidelity::Elided)
    }

    /// One line, for a caller reporting the outcome.
    pub fn summary(&self) -> String {
        let elided = self.elisions().count();
        let failed = self.failures().count();
        let invalid = self.invalid().count();
        let mut counts = Vec::new();
        if failed > 0 {
            counts.push(format!("{failed} failed"));
        }
        if invalid > 0 {
            counts.push(format!("{invalid} invalid"));
        }
        counts.push(format!("{elided} elided"));
        let counts = counts.join(", ");
        if self.is_faithful() {
            format!("{}: complete, {counts}", self.unit)
        } else {
            format!("{}: PARTIAL, {counts}", self.unit)
        }
    }
}

/// The verdict, as a comment at the top of the module.
///
/// The exit status says the same thing, but it is gone as soon as the shell moves
/// on, while the file stays on disk and will eventually be handed to the verifier
/// by someone who didn't generate it. The file has to speak for itself.
fn verdict_items(unit: &str, gaps: &[Gap]) -> Vec<Spanned<Item>> {
    // Both blocking fidelities together: the header's job is to say whether the
    // module may be verified, and neither may.
    let failures: Vec<&Gap> = gaps
        .iter()
        .filter(|g| g.fidelity != Fidelity::Elided)
        .collect();
    let elided = gaps.len() - failures.len();

    let mut text = if failures.is_empty() {
        format!("phoenix: complete translation of {unit}.")
    } else {
        format!(
            "phoenix: PARTIAL translation of {unit} -- DO NOT VERIFY.\n\
             {} construct(s) could not be translated, so this module does not \
             model the C++:",
            failures.len()
        )
    };
    for gap in &failures {
        text.push_str(&format!("\n  {gap}"));
    }
    if elided > 0 {
        text.push_str(&format!("\n{elided} construct(s) deliberately elided."));
    }
    vec![note(text)]
}

/// Extraction or translation failed.
#[derive(Debug)]
pub enum Error {
    Extract(SubsetError),
    Unhandled(Unhandled),
    /// `CacheIROps.yaml` could not be read.
    Ops(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Extract(e) => write!(f, "{e}"),
            Error::Unhandled(e) => write!(f, "{e}"),
            Error::Ops(e) => write!(f, "{e}"),
        }
    }
}

impl Error {
    /// The message for a failure the caller asked about by name, deferring to the
    /// extraction error so the unit it failed in is not named twice.
    pub fn report(&self, requested: &str) -> String {
        match self {
            Error::Extract(e) => e.report(requested),
            _ => format!("cannot translate {requested}: {self}"),
        }
    }
}

impl From<SubsetError> for Error {
    fn from(e: SubsetError) -> Self {
        Error::Extract(e)
    }
}

impl From<Unhandled> for Error {
    fn from(e: Unhandled) -> Self {
        Error::Unhandled(e)
    }
}

/// A top-level comment, for recording what didn't translate.
fn note(text: String) -> Spanned<Item> {
    Spanned::internal(Item::from(Comment { text }))
}

/// The models a generated module is written against: the CacheIR ops it emits,
/// the JS value types it reasons about, and the calling-convention helpers the
/// preamble uses.
///
/// `codegen.cachet` is deliberately absent -- it holds hand-written counterparts
/// of the helpers phoenix generates -- though `js.cachet` imports it anyway.
const IMPORTS: [&str; 3] = ["cacheir.cachet", "js.cachet", "utils.cachet"];

/// Cachet resolves an import against the importing file, so the prefix depends
/// on where the generated module is written: `..` to sit in `notes/stubs/`,
/// something longer to sit outside the tree entirely.
fn import_items(prefix: &Path) -> Vec<Spanned<Item>> {
    IMPORTS
        .iter()
        .map(|name| {
            Spanned::internal(Item::from(ImportItem {
                file_path: Spanned::internal(prefix.join(name)),
            }))
        })
        .collect()
}

/// A helper's C++ signature, using the types as C++ spells them.
fn cpp_signature(fn_def: &FnDef) -> String {
    let params: Vec<String> = fn_def
        .params
        .iter()
        .map(|p| format!("{} {}", p.ty.spelled, p.name))
        .collect();
    format!(
        "{} {}({})",
        fn_def.ret.spelled,
        fn_def.name.name,
        params.join(", ")
    )
}

/// A stub generator and every helper it calls, as one module.
///
/// Helpers come first, then the `ir`. Translation drives the descent: a helper
/// is only translated because emitted code calls it, so a statement that fell
/// back to a comment pulls nothing in.
pub fn load_ops() -> Result<Ops, Error> {
    let path = Ops::default_path().ok_or_else(|| {
        Error::Ops(String::from(
            "no CacheIROps.yaml: this binary was built with PHOENIX_SKIP_SETUP",
        ))
    })?;
    Ops::load(path).map_err(|e| Error::Ops(e.to_string()))
}

/// A callee's definition, as the top-level `fn` the translator will make of it.
///
/// A method is accepted only where its class makes it a helper. A
/// `CacheIRWriter` wrapper qualifies: it reads no writer state, only its
/// parameters and the op behind it, so nothing is lost by dropping the receiver.
/// A method on any other class carries state or ambient entities that a
/// top-level `fn` cannot, and needs a unit of its own.
fn extract_helper<'tu>(entity: &Entity<'tu>) -> Result<(Option<ClassRef>, FnDef<'tu>), Error> {
    match entity.get_kind() {
        EntityKind::Method => {
            let method = get_method_def(entity)?;
            // The class comes along, because what it makes ambient is exactly
            // what a bare `FnDef` would have lost: inside a wrapper, `this` is
            // the writer, so `guardToInt32_(input)` is a writer call.
            if method.class.scope != CACHE_IR_WRITER {
                return Err(Unhandled::new(format!(
                    "`{}::{}`: a method on a class other than CacheIRWriter",
                    method.class, method.def.name.name
                ))
                .into());
            }
            Ok((Some(method.class), method.def))
        }
        _ => Ok((None, get_fn_def(entity)?)),
    }
}

/// Everything a translated definition referred to, and everything those refer to
/// in turn.
///
/// `seed` is what the entry definition produced -- the names it needs, and the
/// gaps it already has -- and `seen` the ids already translated, so a callee that
/// reaches back into one of them isn't translated twice.
///
/// Nothing here is fatal. A callee that can't be extracted or translated becomes
/// a note and a `Failed` gap, so the rest of the module still comes out; what it
/// leaves behind is a call to a name nothing defines, which the note accounts
/// for.
fn translate_transitive_callees<'tu>(
    ops: &Ops,
    op_ir: OpIr,
    mut callees: Callees<'tu>,
    seed: State,
    // Borrowed, so several passes over one module share it: a generator's helpers and
    // its instructions' helpers overlap -- `JSOpToCondition` is reachable from
    // several -- and Cachet has no overloading, so a second definition is an error.
    seen: &mut HashSet<FnId>,
) -> (Vec<Spanned<Item>>, Vec<Gap>) {
    let mut gaps = seed.gaps;
    let mut queue: VecDeque<Needed> = seed.needed.into();
    // Wrappers are identified by op name rather than by a C++ symbol, since
    // there is no C++ definition behind them.
    let mut wrapped: HashSet<String> = HashSet::new();
    let mut helpers = Vec::new();

    while let Some(next) = queue.pop_front() {
        let fn_ref = match next {
            Needed::Cpp(fn_ref) => fn_ref,
            Needed::Wrapper(op_name) => {
                if !wrapped.insert(op_name.clone()) {
                    continue;
                }
                // The op is in the table -- resolving the call is what put this
                // on the queue -- so only synthesis can fail from here.
                let op = ops.get(&op_name).expect("op resolved earlier");
                match create_op_wrapper(op, op_ir) {
                    Ok(item) => helpers.push(Spanned::internal(Item::Fn(item))),
                    Err(e) => {
                        let what = format!("no wrapper for `{}`: {}", writer_method(op), e.what);
                        helpers.push(note(what.clone()));
                        gaps.push(Gap {
                            fidelity: e.fidelity,
                            what,
                            span: CppSpan::Unknown,
                        });
                    }
                }
                continue;
            }
        };

        // Insert before translating, so a cycle terminates instead of looping.
        if !seen.insert(fn_ref.id.clone()) {
            continue;
        }

        // A helper that can't be translated becomes a note rather than failing
        // the module: the generator is still worth seeing. What it leaves behind
        // is a call to a name nothing defines, which the comment accounts for.
        let Some(entity) = callees.get(&fn_ref.id).copied() else {
            let what = format!(
                "`{}` has no definition in this translation unit",
                fn_ref.name
            );
            helpers.push(note(what.clone()));
            // A call to a name nothing defines. `cachet-compiler` would catch
            // this one, but the verdict shouldn't depend on that.
            gaps.push(Gap {
                fidelity: Fidelity::Failed,
                what,
                span: CppSpan::Unknown,
            });
            continue;
        };
        let (class, fn_def) = match extract_helper(&entity) {
            Ok(extracted) => extracted,
            Err(e) => {
                let what = format!("cannot extract `{}`: {e}", fn_ref.name);
                helpers.push(note(what.clone()));
                gaps.push(Gap {
                    fidelity: Fidelity::Failed,
                    what,
                    span: CppSpan::Unknown,
                });
                continue;
            }
        };
        match translate_fn_def(ops, op_ir, class, &fn_def) {
            Ok((item, more)) => {
                helpers.push(Spanned::internal(Item::Fn(item)));
                callees.extend(fn_def.callees);
                gaps.extend(more.gaps);
                queue.extend(more.needed);
            }
            Err(e) => {
                helpers.push(note(format!(
                    "cannot translate:\n{}\n{e}",
                    cpp_signature(&fn_def)
                )));
                gaps.push(Gap {
                    fidelity: e.fidelity,
                    what: format!("cannot translate `{}`: {}", fn_def.name.name, e.what),
                    span: e.span,
                });
            }
        }
    }

    (helpers, gaps)
}

/// A definition and everything it calls, with the definition last, as the
/// helpers it depends on have to be defined before it reads.
pub fn translate_fn_and_transitive_callees<'tu>(
    ops: &Ops,
    op_ir: OpIr,
    class: Option<ClassRef>,
    fn_def: FnDef<'tu>,
) -> Result<(Vec<Spanned<Item>>, Vec<Gap>), Unhandled> {
    // Seeded with the definition itself, so one that reaches back into itself
    // isn't translated a second time.
    let mut seen = HashSet::from([fn_def.name.id.clone()]);
    let (item, state) = translate_fn_def(ops, op_ir, class, &fn_def)?;
    let (mut items, gaps) =
        translate_transitive_callees(ops, op_ir, fn_def.callees, state, &mut seen);
    items.push(Spanned::internal(Item::Fn(item)));
    Ok((items, gaps))
}

/// A `CacheIRCompiler::emit*` method as the `op` it gives meaning to.
///
/// The signature comes from `CacheIROps.yaml`, not from the C++. It has to: the
/// C++ parameters are generated from the yaml by `GenerateCacheIRFiles.py`, and
/// generating them *loses* information, since `arg_reader_info` turns every
/// field kind into a bare `uint32_t` offset -- `RawInt32Field`,
/// `RawPointerField`, `ICScriptField` and `IdField` all arrive as `uint32_t`.
/// The yaml still has the distinction, so it is read rather than reconstructed.
///
/// The op sits in `ir CacheIR`, which is what says `emits MASM`, so the op
/// itself declares neither that nor a return.
///
/// Scaffolding: the signature only. Reconciling it with the body -- whose
/// statements refer to the C++ parameter names, `valOffset` where the yaml says
/// `val` -- comes next.
pub fn translate_cacheir_op(
    ops: &Ops,
    op_ir: OpIr,
    method: &MethodDef,
) -> Result<(CallableItem, State), Unhandled> {
    let name = method
        .def
        .name
        .name
        .strip_prefix("emit")
        .ok_or_else(|| {
            Unhandled::new(format!(
                "`{}`: an instruction is named `emit<Op>`",
                method.def.name.name
            ))
        })?
        .to_owned();
    let op = ops
        .get(&name)
        .ok_or_else(|| Unhandled::new(format!("no op `{name}` in CacheIROps.yaml")))?;

    // The declaration in `CacheIROpsGenerated.h` is generated from the yaml args
    // in order, and the definition implements that declaration, so the two lists
    // correspond position by position.
    if op.args.len() != method.def.params.len() {
        return Err(Unhandled::new(format!(
            "`{name}`: the yaml gives {} operand(s), the definition takes {}",
            op.args.len(),
            method.def.params.len()
        )));
    }

    // Types from the yaml, which is where a field is a field rather than an
    // offset. Names from the C++, so the body refers to its parameters as it
    // already does and cannot come to shadow them -- except a field's, the one
    // operand whose meaning changes, and which it would be a lie to call
    // `valOffset`.
    let mut names = NameMap::build(
        declared_names(&method.def.params, &method.def.body)
            .iter()
            .map(String::as_str),
    );
    let sig = op
        .args
        .iter()
        .zip(&method.def.params)
        .map(|((arg, ty), param)| {
            Ok(SigParam {
                // A field's name comes from the yaml, not from the C++, so it is the
                // one name phoenix invents -- and the one that has to be claimed
                // against the unit, or a local of the same name would shadow it. The
                // rest are the C++'s own.
                name: if is_stub_field(ty) {
                    names.claim(arg)
                } else {
                    names.ident(&param.name)
                },
                ty: translate_arg_type(ty)?,
            })
        })
        .collect::<Result<Vec<_>, Unhandled>>()?;

    let offsets = op
        .args
        .iter()
        .zip(&sig)
        .zip(&method.def.params)
        .filter(|(((_, ty), _), _)| is_stub_field(ty))
        .map(|((_, arg), param)| (param.name.clone(), arg.name.clone()))
        .collect();

    let ctx = Ctx {
        class: Some(method.class.clone()),
        ops,
        op_ir,
        names,
        cpp_sig: Some(method.def.params.clone()),
        cachet_sig: Some(sig.clone()),
        instruction: Some(Instruction { offsets }),
    };
    let mut state = State::default();
    let body = translate_block(&ctx, &mut state, &method.def.body)?;

    Ok((
        CallableItem {
            ident: Spanned::internal(Ident::from(name)),
            attrs: Vec::new(),
            is_unsafe: false,
            params: sig.iter().map(SigParam::to_param).collect(),
            // Both are the enclosing `ir CacheIR`'s to declare.
            emits: None,
            ret: None,
            body: Spanned::internal(Some(body)),
        },
        state,
    ))
}

/// An instruction's op, the helpers it calls, and every gap in the lot.
///
/// The op and the helpers stay apart because they land in different places: the op
/// belongs inside `ir CacheIR`, the helpers are top-level `fn`s beside it.
///
/// One worklist covers the whole closure, which is what keeps it free of
/// duplicates: emitters share helpers -- several reach `JSOpToCondition` -- and
/// Cachet has no overloading, so emitting one twice is a `duplicate definition`
/// error. A generator translating several ops will want one worklist across all of
/// them for the same reason.
pub fn translate_cacheir_op_and_helpers(
    instruction: &Entity<'_>,
    op_ir: OpIr,
) -> Result<(CallableItem, Vec<Spanned<Item>>, Vec<Gap>), Error> {
    let ops = load_ops()?;
    let method = get_method_def(instruction)?;
    let mut seen = HashSet::new();
    Ok(translate_op_sharing(&ops, op_ir, method, &mut seen)?)
}

/// One instruction, into a module that holds others.
///
/// `seen` is shared so the helpers do not repeat what a sibling op or the generator
/// already defined.
fn translate_op_sharing<'tu>(
    ops: &Ops,
    op_ir: OpIr,
    method: MethodDef<'tu>,
    seen: &mut HashSet<FnId>,
) -> Result<(CallableItem, Vec<Spanned<Item>>, Vec<Gap>), Unhandled> {
    // Inserted before translating, so an emitter that reaches back into itself is not
    // translated a second time.
    seen.insert(method.def.name.id.clone());
    let (op, state) = translate_cacheir_op(ops, op_ir, &method)?;
    let (helpers, gaps) =
        translate_transitive_callees(ops, op_ir, method.def.callees, state, seen);
    Ok((op, helpers, gaps))
}

/// Where a generator's CacheIR instructions are translated from.
///
/// A separate translation unit, because the generator's does not contain them: the
/// unified build puts `CacheIR.cpp` in `Unified_cpp_js_src_jit2.cpp` and
/// `CacheIRCompiler.cpp` in `..jit3.cpp`, so the emitters have to be parsed on their
/// own.
pub struct Instructions<'tu> {
    /// The root of the translation unit holding `CacheIRCompiler::emit*`.
    pub root: Entity<'tu>,
    /// The file the real definitions are in, so the macro-generated
    /// `emit<Op>(CacheIRReader&)` shim in `CacheIRCompiler.h` is not taken instead.
    pub source: &'tu Path,
}

/// A stub generator as a module: the `ir` for the generator, every helper it calls,
/// and -- given somewhere to translate them from -- the CacheIR ops it emits.
///
/// `instructions` is what decides between the two: with it, the ops are translated
/// into an `ir CacheIROps` beside the model; without it, the generator emits into the
/// hand-written `ir CacheIR` and the module leans on that.
pub fn translate_generator(
    generator: &Entity<'_>,
    instructions: Option<Instructions<'_>>,
    imports: &Path,
) -> Result<Translation, Error> {
    let op_ir = match instructions {
        Some(_) => OpIr::Generated,
        None => OpIr::Model,
    };
    let ops = load_ops()?;
    let gen_def = get_method_def(generator)?;
    let unit = format!("{}::{}", gen_def.class, gen_def.def.name.name);
    let (ir, state) = translate_gen_def(&ops, op_ir, &gen_def)?;

    // One `seen` across every pass over this module, so a helper two of them reach is
    // defined once.
    let mut seen = HashSet::from([gen_def.def.name.id.clone()]);
    // Each definition brings its own callees, so deeper helpers stay resolvable.
    let (gen_helpers, mut gaps) =
        translate_transitive_callees(&ops, op_ir, gen_def.def.callees, state, &mut seen);

    // After the generator's pass, not before: the ops reached through a wrapper are
    // only visible once `translate_transitive_callees` has synthesized it, and
    // `LoadInt32Constant` is emitted nowhere else.
    let mut op_items = Vec::new();
    let mut op_helpers = Vec::new();
    if let Some(instructions) = instructions {
        let emitted = {
            let items: Vec<_> = gen_helpers
                .iter()
                .cloned()
                .chain([Spanned::internal(Item::Ir(ir.clone()))])
                .collect();
            emitted_ops(&items, op_ir.name())
        };
        for op in emitted {
            match translate_instruction(&ops, op_ir, &instructions, &op, &mut seen) {
                Ok((item, helpers, more)) => {
                    op_items.push(Spanned::internal(Item::Op(item)));
                    op_helpers.extend(helpers);
                    gaps.extend(more);
                }
                Err(e) => {
                    let what = format!("`{op}`: {}", e.what);
                    op_helpers.push(note(what.clone()));
                    gaps.push(Gap {
                        fidelity: e.fidelity,
                        what,
                        span: e.span,
                    });
                }
            }
        }
    }

    let op_ir_item = (!op_items.is_empty()).then(|| {
        Spanned::internal(Item::Ir(IrItem {
            ident: Spanned::internal(Ident::from(op_ir.name())),
            // An op emits machine code, as `ir CacheIR emits MASM` has it.
            emits: Some(Spanned::internal(CachetPath::from_ident("MASM"))),
            items: op_items,
        }))
    });

    // Helpers before what needs them, and the ops before the generator that emits
    // them.
    let module = verdict_items(&unit, &gaps)
        .into_iter()
        .chain(import_items(imports))
        .chain(op_helpers)
        .chain(op_ir_item)
        .chain(gen_helpers)
        .chain([Spanned::internal(Item::Ir(ir))])
        .collect();
    Ok(Translation { unit, module, gaps })
}

/// One `CacheIRCompiler::emit<Op>`, found by name in the instructions' unit.
fn translate_instruction(
    ops: &Ops,
    op_ir: OpIr,
    instructions: &Instructions<'_>,
    op: &Ident,
    seen: &mut HashSet<FnId>,
) -> Result<(CallableItem, Vec<Spanned<Item>>, Vec<Gap>), Unhandled> {
    let qualified = format!("CacheIRCompiler::emit{op}");
    let entity = find_definition(instructions.root, &qualified, instructions.source)
        .ok_or_else(|| Unhandled::new(format!("no definition of `{qualified}`")))?;
    let method = get_method_def(&entity).map_err(|e| Unhandled::new(e.to_string()))?;
    translate_op_sharing(ops, op_ir, method, seen)
}
