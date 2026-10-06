//! `js/src/jit/MacroAssembler.h`: the machine a stub emits code to.
//!
//! Unlike CacheIR there is no table to read from. The MacroAssembler is
//! hand-written C++, with no yaml and no generated op list -- `moz.build`
//! generates headers from `CacheIROps.yaml`, `MIROps.yaml` and `LIROps.yaml`,
//! but nothing for masm. So what a method *means* is whatever
//! `notes/masm.cachet` says it means, and this file is the correspondence
//! between the two.
//!
//! Nearly every masm method returns nothing (1385 of 1527 declarations in
//! `MacroAssembler.h` are `void`), so a masm call is nearly always a statement,
//! and those are what this table maps. The exceptions a stub emitter calls, such
//! as `extractTag`, return a register: their definitions are translated as helpers
//! instead, the way a `custom_writer` CacheIR method is.

use cachet_lang::ast::Path as CachetPath;

use crate::cpp_subset::{
    Callee as CppCallee, Expr as CppExpr, Stmt as CppStmt, Type as CppType,
    TypedExpr as CppTypedExpr,
};
use crate::cpp_to_cachet::translate_type;
use crate::scopes::Scopes;

/// `js::jit::StackMacroAssembler`, the `masm` field code is emitted through.
pub const MASM: [&str; 3] = ["js", "jit", "StackMacroAssembler"];

/// `js::jit::MacroAssembler`, the base the `masm` field is a `StackMacroAssembler`
/// of. A helper takes it in this spelling -- `EmitStoreBoolean(MacroAssembler&, ..)`
/// -- so both name the same machine.
pub const MACRO_ASSEMBLER: [&str; 3] = ["js", "jit", "MacroAssembler"];

pub fn is_masm(ty: &CppType) -> bool {
    ty.scope == MASM || ty.scope == MACRO_ASSEMBLER
}

/// The classes whose methods run with `this` as the machine: `MacroAssembler` and
/// the platform bases it inherits from on x64, where most of the methods a stub
/// calls are declared (`extractTag` is `MacroAssemblerX64`'s, :978).
///
/// x64 only, as is the translation unit: arm64's bases are other classes, with
/// other definitions (its `extractTag` calls `splitSignExtTag`).
const MASM_FAMILY: [[&str; 3]; 4] = [
    MASM,
    MACRO_ASSEMBLER,
    ["js", "jit", "MacroAssemblerX64"],
    ["js", "jit", "MacroAssemblerX86Shared"],
];

pub fn is_masm_class(scope: &[String]) -> bool {
    MASM_FAMILY.iter().any(|class| scope == class)
}

/// `js::jit::Label`, a position in the code being emitted. Masm's own type, and
/// the one thing a stub declares that Cachet has a statement form for rather than
/// a value.
pub const LABEL: [&str; 3] = ["js", "jit", "Label"];

/// What one `masm.<method>(..)` call becomes in the model.
///
/// Arguments stay C++ here: which statement a call means is this module's business,
/// turning an expression into Cachet is the translator's.
#[derive(Debug)]
pub enum MasmStmt<'e> {
    Emit {
        op: &'static str,
        args: Vec<&'e CppTypedExpr>,
    },
    /// `bind ifTrue;`. A statement of its own rather than a call, so it names its
    /// label instead of passing one.
    ///
    /// A label in *argument* position needs no such treatment: it rides through as an
    /// expression and the Cachet parser resolves it against the op's signature, which
    /// phoenix cannot do because it never reads the model. A `bind` has no argument
    /// position to hide a label in.
    Bind { label: &'e CppTypedExpr },
}

/// The statements a `masm.<method>(..)` call becomes.
///
/// Usually one, but not always: the model tracks what kind of value a register holds
/// where the machine just moves bits, so a `move32` into a register declared int32
/// from one holding a bool is `Move32Bool` followed by a `CastBoolToInt32` that has
/// no counterpart in the C++ at all.
///
/// `None` where the statement isn't a masm call at all, and where it is one the
/// model has no op for -- the two are told apart by the error the caller reports,
/// not here.
///
/// `this_is_masm` says an implicit `this` is the machine, as inside a masm helper,
/// where `splitTag(value, scratch)` is `masm.splitTag(value, scratch)`.
pub fn masm_call<'e>(
    stmt: &'e CppStmt,
    scopes: &Scopes,
    this_is_masm: bool,
) -> Option<Vec<MasmStmt<'e>>> {
    let CppStmt::Expr(CppTypedExpr {
        value: CppExpr::Call(call),
        ..
    }) = stmt
    else {
        return None;
    };
    let CppCallee::Method { recv, callee } = &call.callee else {
        return None;
    };
    let on_masm = match recv {
        Some(recv) => matches!(&recv.value, CppExpr::Ref(r) if is_masm(&r.ty)),
        None => this_is_masm,
    };
    if !on_masm {
        return None;
    }

    // `bind` is not a call in the model at all, so it leaves before the op
    // machinery: there is no op to name and no coercion to owe.
    if callee.name == "bind" {
        let [label] = call.args.as_slice() else {
            return None;
        };
        return Some(vec![MasmStmt::Bind { label }]);
    }

    // Three methods need more than a name-and-shape lookup, for different reasons, so
    // they sit here rather than in the table.
    let (op, args) = match callee.name.as_str() {
        // Its op depends on what a register *holds*, which is nowhere in the call:
        // both C++ overloads are `(Register, Register)`.
        "move32" => (move32_op(&call.args, scopes)?, call.args.iter().collect()),
        // Its op absorbs the immediate, so choosing it also rewrites an argument.
        "movePtr" => move_ptr(&call.args)?,
        // Its message is dropped: the model's op takes none, and the string carries
        // no semantics -- the body is `assert false` with or without it. The model
        // records wanting it anyway (notes/masm.cachet:794).
        //
        // This is not the `unreachable` statement. `MOZ_CRASH` is the generator
        // giving up as it runs, while this *emits* an instruction that traps when
        // the generated code runs, so it stays an emit.
        "assumeUnreachable" => ("AssumeUnreachable", Vec::new()),
        method => (
            translate_op(method, &call.args)?,
            call.args.iter().collect(),
        ),
    };

    let mut stmts = vec![MasmStmt::Emit { op, args }];
    stmts.extend(coerce_written(op, &call.args, scopes));
    Some(stmts)
}

/// `Move32Bool` or `Move32Int32`, by what the source register holds.
///
/// Both C++ overloads take `(Register, Register)`, so the call says nothing; the
/// model's two ops read the source with `getBool` and `getInt32` respectively, and
/// which is right depends on the operand id the source was bound from.
fn move32_op(args: &[CppTypedExpr], scopes: &Scopes) -> Option<&'static str> {
    let [src, _dst] = args else {
        return None;
    };
    match reg_kind(src, scopes)?.to_string().as_str() {
        "BoolId" => Some("Move32Bool"),
        "Int32Id" => Some("Move32Int32"),
        _ => None,
    }
}

/// The coercion an emit owes, when it writes a kind of value into a register
/// declared to hold another.
///
/// `emitGuardBooleanToInt32` is the case: the result register is defined for an
/// `Int32Id`, so its consumers read it with `getInt32`, but both branches write a
/// bool into it. On the machine that is free, a JS boolean payload already being 0
/// or 1; in the model it takes an op.
fn coerce_written<'e>(
    op: &'static str,
    args: &'e [CppTypedExpr],
    scopes: &Scopes,
) -> Option<MasmStmt<'e>> {
    let (index, written) = writes(op)?;
    let dst = args.get(index)?;
    let declared = reg_kind(dst, scopes)?;
    Some(MasmStmt::Emit {
        op: coercion(written, &declared.to_string())?,
        args: vec![dst],
    })
}

/// The argument an op writes to, and the kind of value it puts there. `None` for
/// an op that writes no register, which is most of them.
fn writes(op: &str) -> Option<(usize, &'static str)> {
    Some(match op {
        "Move32Bool" => (1, "BoolId"),
        "Move32Int32" => (1, "Int32Id"),
        "FallibleUnboxBoolean" => (1, "BoolId"),
        _ => return None,
    })
}

/// The op that turns a register holding `from` into one holding `to`, or `None`
/// when they agree and nothing is owed.
fn coercion(from: &str, to: &str) -> Option<&'static str> {
    Some(match (from, to) {
        // notes/masm.cachet:1080.
        ("BoolId", "Int32Id") => "CastBoolToInt32",
        _ => return None,
    })
}

/// The operand id a register-valued argument was bound from.
fn reg_kind<'s>(arg: &CppTypedExpr, scopes: &'s Scopes) -> Option<&'s CachetPath> {
    let CppExpr::Ref(r) = &arg.value else {
        return None;
    };
    scopes.register(&r.name)
}

/// A `MacroAssembler` method to the op modeling it in `notes/masm.cachet`.
///
/// Keyed on the method name and, where the model splits a method into several ops,
/// the shapes of its first two operands -- which is what C++ overloads on.
/// `moveValue` was the first to need a shape at all: `MoveValue` takes a register
/// source, `MoveValueImm` a `Value`, and the call says which. Two positions rather
/// than one because the `branchTest*` family overloads on its *second*: the first is
/// always the `Condition`.
///
/// A `_` in a shape column does not mean the method has one overload, only that the
/// model has one op for the overloads reached so far. Those rows want filling in as
/// they are reached rather than guessed at. Where the model has nothing for an
/// overload -- the `Address` and `BaseIndex` forms, which want an address in the
/// model -- no row matches and the call is refused.
///
/// Only 45 of the model's 106 ops are even the lowercased method name, so this stays
/// a table either way.
fn translate_op(method: &str, args: &[CppTypedExpr]) -> Option<&'static str> {
    let shape = |i: usize| operand_shape(args.get(i));
    Some(match (method, shape(0).as_deref(), shape(1).as_deref()) {
        ("branchTestInt32", _, _) => "BranchTestInt32",
        // The tag forms take an already-extracted tag register, where the plain ones
        // take a whole `Value` and extract it themselves -- so they ask the same
        // question of different things and are separate ops. Four C++ overloads
        // apiece (MacroAssembler.h:1960-2055); the `Address` and `BaseIndex` two are
        // refused.
        ("branchTestNull", _, Some("ValueReg")) => "BranchTestNull",
        ("branchTestNull", _, Some("Reg")) => "BranchTestNullTag",
        ("branchTestUndefined", _, Some("ValueReg")) => "BranchTestUndefined",
        ("branchTestUndefined", _, Some("Reg")) => "BranchTestUndefinedTag",
        ("branchTestObject", _, Some("ValueReg")) => "BranchTestObject",
        ("branchTestObject", _, Some("Reg")) => "BranchTestObjectTag",
        // Two overloads only, both modelled (MacroAssembler.h:1966, :2012): a number
        // is either of two tags, so there is no single-word memory test to offer.
        ("branchTestNumber", _, Some("ValueReg")) => "BranchTestNumber",
        ("branchTestNumber", _, Some("Reg")) => "BranchTestNumberTag",
        // Extracts a `Value`'s type tag into a register (notes/masm.cachet:2161).
        // One signature, `(const ValueOperand&, ScratchTagScope&)`.
        ("splitTagForTest", _, _) => "SplitTagForTest",
        // The same extraction into a plain register, reached from inside the
        // `extractTag` helper (MacroAssembler-x64.h:715). Keyed: the `Register` and
        // `Operand` sources (:709, :718) are raw bits, which the model has no op for.
        ("splitTag", Some("ValueReg"), _) => "SplitTagForTest",
        // Keyed because the model has only the `ValueOperand` source
        // (notes/masm.cachet:1022) of four overloads -- the others take a `Register`,
        // an `Address` or a `BaseIndex` (MacroAssembler-arm64.h:1447-1457).
        ("unboxObject", Some("ValueReg"), _) => "UnboxObject",
        // One signature, `(Register, Register, Label*, Label*)`
        // (MacroAssembler.h:1849), against notes/masm.cachet:1512.
        ("branchIfObjectEmulatesUndefined", _, _) => "BranchIfObjectEmulatesUndefined",
        // `Branch32Tag`, `Branch32Imm` and `Branch32AddressImm32` are the model's
        // other three.
        ("branch32", _, _) => "Branch32",
        // Templated on the source, `fallibleUnboxBoolean(const T&, Register,
        // Label*)`. The model has the `ValueOperand` instantiation only.
        ("fallibleUnboxBoolean", _, _) => "FallibleUnboxBoolean",
        // An unconditional branch to a label, which is an ordinary op: the label is
        // its argument, unlike `bind`'s.
        ("jump", _, _) => "Jump",
        ("moveValue", Some("Value"), _) => "MoveValueImm",
        ("moveValue", Some("ValueReg"), _) => "MoveValue",
        // Boxes `payload` as a `valTy` into `dest` (notes/masm.cachet:964). Not a
        // provisional `_`: every one of the nine platform headers declares the
        // single signature `tagValue(JSValueType, Register, ValueOperand)`.
        ("tagValue", _, _) => "TagValue",
        // Sets `dest` from whether the value's tag matches, under `condition`
        // (notes/masm.cachet:1468, :1529). Also settled rather than provisional:
        // nine platform headers, one signature each,
        // `(Condition, const ValueOperand&, Register)`.
        ("testNullSet", _, _) => "TestNullSet",
        ("testUndefinedSet", _, _) => "TestUndefinedSet",
        // A whole-register copy: the model's `Move` reads and writes the register's
        // data untyped (notes/masm.cachet:812), where the `Move32*` ops read a typed
        // 32-bit payload, so this needs none of `move32`'s guessing at contents.
        // Keyed because only one of arm64's five `mov` overloads is modelled -- the
        // rest take `ImmWord`, `ImmPtr`, `SymbolicAddress` or `CodeLabel*`
        // (MacroAssembler-arm64.h:739-743).
        ("mov", Some("Reg"), _) => "Move",
        // Keyed rather than `_`: the source is overloaded three ways on arm64 --
        // `Register`, `Address`, `BaseIndex` (MacroAssembler-arm64.h:457-468), and
        // four on x86 -- while the model has only the register form
        // (notes/masm.cachet:1285). The memory forms want an address in the model
        // before they can be translated, so they fall through and are refused.
        ("convertInt32ToDouble", Some("Reg"), _) => "ConvertInt32ToDouble",
        _ => return None,
    })
}

/// `movePtr(ImmWord(b), reg)` is `MovePtrBoolImmWord(b, reg)`.
///
/// The model has no generic `movePtr`. Its ops name both the immediate kind and what
/// it carries -- `MovePtrBoolImmWord(b: Bool, ..)`, `MovePtrImmGCPtrObject(object:
/// Object, ..)` -- so the C++'s `ImmWord` construction has nowhere to go and the
/// value inside it is passed instead. The only masm mapping so far where choosing
/// the op also rewrites an argument.
fn move_ptr<'e>(args: &'e [CppTypedExpr]) -> Option<(&'static str, Vec<&'e CppTypedExpr>)> {
    let [imm, dst] = args else {
        return None;
    };
    let CppExpr::Construct(c) = &imm.value else {
        return None;
    };
    let [carried] = c.args.as_slice() else {
        return None;
    };
    let op = match (
        c.ty.scope.last()?.as_str(),
        operand_shape(Some(carried))?.as_str(),
    ) {
        ("ImmWord", "Bool") => "MovePtrBoolImmWord",
        _ => return None,
    };
    Some((op, vec![carried, dst]))
}

/// What the model calls the type of a masm operand, for the methods it splits by
/// operand shape.
///
/// The Cachet spelling, since it is the model's own distinctions this has to line up
/// with -- so it goes through [`translate_type`] rather than keeping a second table
/// of the same C++ paths. `None` where the argument carries no readable type, or one
/// the model has no name for.
fn operand_shape(arg: Option<&CppTypedExpr>) -> Option<String> {
    translate_type(arg?.ty.as_ref()?)
        .ok()
        .map(|ty| ty.to_string())
}
