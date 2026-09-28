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

use crate::cpp_subset::{
    Callee as CppCallee, Expr as CppExpr, Spanned as CppSpanned, Stmt as CppStmt, Type as CppType,
};

/// `js::jit::StackMacroAssembler`, the `masm` field code is emitted through.
pub const MASM: [&str; 3] = ["js", "jit", "StackMacroAssembler"];

pub fn is_masm(ty: &CppType) -> bool {
    ty.scope == MASM
}

/// The op a `masm.<method>(..)` statement emits, and the arguments it is given.
///
/// `None` where the statement isn't a masm call at all, and where it is one the
/// model has no op for -- the two are told apart by the error the caller reports,
/// not here.
pub fn masm_call(stmt: &CppStmt) -> Option<(&'static str, &[CppSpanned<CppExpr>])> {
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
    Some((translate_op(&callee.name)?, &call.args))
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
        _ => return None,
    })
}
