//! `js/src/jit/MacroAssembler.h`: the machine a stub emits code to.
//!
//! Unlike CacheIR there is no table to read from. The MacroAssembler is
//! hand-written C++, with no yaml and no generated op list -- `moz.build`
//! generates headers from `CacheIROps.yaml`, `MIROps.yaml` and `LIROps.yaml`,
//! but nothing for masm. So what a method *means* is whatever
//! `notes/masm.cachet` says it means, and this file is the correspondence
//! between the two.
//!
//! One simplification falls out of that: masm methods return nothing (1385 of
//! 1527 declarations in `MacroAssembler.h` are `void`, and the rest are wasm
//! trap ranges and patchable-jump offsets, which no stub emitter calls). So a
//! masm call is always a statement, and there is no counterpart to the
//! result-bearing CacheIR ops that need a synthesized wrapper.

use cachet_lang::ast::Path as CachetPath;

use crate::cpp_subset::{
    Callee as CppCallee, Expr as CppExpr, Spanned as CppSpanned, Stmt as CppStmt, Type as CppType,
};
use crate::scopes::Scopes;

/// `js::jit::StackMacroAssembler`, the `masm` field code is emitted through.
pub const MASM: [&str; 3] = ["js", "jit", "StackMacroAssembler"];

pub fn is_masm(ty: &CppType) -> bool {
    ty.scope == MASM
}

/// One op to emit, with the C++ arguments to pass it.
///
/// The arguments stay C++ here: which op to emit is this module's business, while
/// turning an expression into Cachet is the translator's.
#[derive(Debug)]
pub struct Emit<'e> {
    pub op: &'static str,
    pub args: Vec<&'e CppSpanned<CppExpr>>,
}

/// The ops a `masm.<method>(..)` statement emits.
///
/// Usually one, but not always: a single masm call can mean a sequence in the
/// model, because the model tracks what kind of value a register holds where the
/// machine just moves bits. `move32` into a register declared int32 from one
/// holding a bool is `Move32Bool` followed by `CastBoolToInt32`, the second having
/// no counterpart in the C++ at all.
///
/// `None` where the statement isn't a masm call at all, and where it is one the
/// model has no op for -- the two are told apart by the error the caller reports,
/// not here.
pub fn masm_call<'e>(stmt: &'e CppStmt, scopes: &Scopes) -> Option<Vec<Emit<'e>>> {
    let CppStmt::Expr(CppExpr::Call(call)) = stmt else {
        return None;
    };
    let CppCallee::Method {
        recv: Some(recv),
        callee,
    } = &call.callee
    else {
        return None;
    };
    if !matches!(&recv.value, CppExpr::Ref(r) if is_masm(&r.ty)) {
        return None;
    }

    let op = match callee.name.as_str() {
        // The one method whose op depends on what a register holds rather than on
        // the call, both C++ overloads being `(Register, Register)`.
        "move32" => move32_op(&call.args, scopes)?,
        method => translate_op(method)?,
    };
    let args = call.args.iter().collect();

    let mut emits = vec![Emit { op, args }];
    emits.extend(coerce_written(op, &call.args, scopes));
    Some(emits)
}

/// `Move32Bool` or `Move32Int32`, by what the source register holds.
///
/// Both C++ overloads take `(Register, Register)`, so the call says nothing; the
/// model's two ops read the source with `getBool` and `getInt32` respectively, and
/// which is right depends on the operand id the source was bound from.
fn move32_op(args: &[CppSpanned<CppExpr>], scopes: &Scopes) -> Option<&'static str> {
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
    args: &'e [CppSpanned<CppExpr>],
    scopes: &Scopes,
) -> Option<Emit<'e>> {
    let (index, written) = writes(op)?;
    let dst = args.get(index)?;
    let declared = reg_kind(dst, scopes)?;
    Some(Emit {
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
fn reg_kind<'s>(arg: &CppSpanned<CppExpr>, scopes: &'s Scopes) -> Option<&'s CachetPath> {
    let CppExpr::Ref(r) = &arg.value else {
        return None;
    };
    scopes.register(&r.name)
}

/// A `MacroAssembler` method to the op modeling it in `notes/masm.cachet`.
///
/// Keyed on the method name alone, which is not enough in general: masm methods
/// are overloaded on operand shape, and the model disambiguates in the op name.
/// `branchTestNull` is four C++ overloads, of which the model has two --
/// `BranchTestNull` for a `ValueOperand` and `BranchTestNullTag` for a tag
/// `Register` -- and `load32` splits into `Load32Address` and friends. Only 45
/// of the model's 106 ops are even the lowercased method name, so this stays a
/// table, and the key grows an operand-type column when a second overload is
/// first reached.
fn translate_op(method: &str) -> Option<&'static str> {
    Some(match method {
        "branchTestNull" => "BranchTestNull",
        "branchTestInt32" => "BranchTestInt32",
        // Templated on the source, `fallibleUnboxBoolean(const T&, Register,
        // Label*)`. The model has the `ValueOperand` instantiation only, which is
        // the one the emitters reach.
        "fallibleUnboxBoolean" => "FallibleUnboxBoolean",
        _ => return None,
    })
}
