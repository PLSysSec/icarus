//! What the translator knows about the locals in scope.
//!
//! Its own module because both sides need it: `cpp_to_cachet` records what a
//! declaration establishes, and `masm_ops` reads it back to decide which op a
//! masm call means.

use std::collections::HashMap;

use cachet_lang::ast::{Ident, Path as CachetPath};

/// Something the translator must emit on leaving a point in the walk.
///
/// Held by the block that took it on, which is what says who discharges it: a
/// `return` leaves every enclosing block and so owes all of them, while falling
/// out of a block owes only its own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Obligation {
    /// `CacheIR::releaseFailurePath()`, which has no C++ line behind it at all.
    ///
    /// The model requires `addFailurePath` and `releaseFailurePath` to balance --
    /// each asserts the other's state (notes/cacheir.cachet:1806-1820) -- while C++
    /// needs no release, `addedFailurePath_` being cleared by `nextOp()` between
    /// instructions. So every release is inserted.
    ReleaseFailurePath,
    /// `CacheIR::releaseReg(_)` for a scratch register whose `Auto*` wrapper is
    /// going out of scope.
    ///
    /// Unlike a failure path, C++ *does* emit this one, from the wrapper's
    /// destructor, so it is relocated rather than invented. Which is why it carries
    /// a name: the destructor knows which register it holds, and the statement it
    /// becomes has to say so.
    ReleaseReg(Ident),
    /// `CacheIR::releaseScratchReg()`: give the tag register back, owed by a
    /// `ScratchTagScope`.
    ///
    /// Carries no name, unlike [`Obligation::ReleaseReg`]: the model fixes the tag
    /// register at R11 rather than binding one (notes/cacheir.cachet:1848).
    ReleaseScratchReg,
    /// `CacheIR::allocateScratchReg()`: take the tag register *back*, owed by a
    /// `ScratchTagScopeRelease`.
    ///
    /// That wrapper lends the tag register out for the length of a block, by
    /// releasing it in its constructor and calling `reacquire()` in its destructor
    /// (MacroAssembler-arm64.h:2211). So its obligation is the inverse of the one
    /// above, and it is the only obligation whose C++ counterpart *takes* a
    /// resource rather than giving one up.
    ReacquireScratchReg,
}

/// The locals declared in one C++ block.
#[derive(Debug, Default)]
pub struct Scope {
    /// Register-valued local -> the operand id it was bound from, which is what
    /// says *what kind of value* the register holds.
    ///
    /// C++ has only `Register` there, while the model's `Reg` holds a typed JS
    /// value, so `masm.move32(a, b)` is `Move32Bool` or `Move32Int32` depending on
    /// a fact the C++ never states.
    registers: HashMap<String, CachetPath>,
    /// What the block owes on the way out, in the order it took them on.
    /// Discharged in reverse, C++ destroying in reverse of construction.
    owed: Vec<Obligation>,
}

/// The blocks enclosing the statement being translated, innermost last.
///
/// A frame per block, because a C++ declaration is scoped to its block and a
/// sibling block may reuse the name for a register of another kind.
#[derive(Debug, Default)]
pub struct Scopes(Vec<Scope>);

impl Scopes {
    pub fn push(&mut self) {
        self.0.push(Scope::default());
    }

    pub fn pop(&mut self) {
        self.0.pop();
    }

    /// Records that the innermost block has taken an obligation on. Dropped when
    /// there is no block, which cannot happen while translating one.
    fn owe(&mut self, obligation: Obligation) {
        if let Some(scope) = self.0.last_mut() {
            scope.owed.push(obligation);
        }
    }

    /// Records that this block took out a failure path.
    ///
    /// `false` when one is already outstanding *anywhere* enclosing, which the model
    /// forbids and C++ asserts against -- "multiple failure paths for instruction".
    /// The caller reports it, since it is a fault in the C++ rather than a gap in the
    /// translation.
    pub fn acquire_failure_path(&mut self) -> bool {
        if self
            .0
            .iter()
            .any(|scope| scope.owed.contains(&Obligation::ReleaseFailurePath))
        {
            return false;
        }
        self.owe(Obligation::ReleaseFailurePath);
        true
    }

    /// Records that this block holds a scratch register, to be released when it
    /// goes out of scope.
    pub fn acquire_reg(&mut self, reg: Ident) {
        self.owe(Obligation::ReleaseReg(reg));
    }

    /// Records that this block holds the tag register.
    ///
    /// `false` when one is already outstanding. Unlike a second failure path, this
    /// is a limit of the model rather than a fault in the C++: nesting is fine on
    /// 64-bit, where vixl hands out a second and different register, while the model
    /// fixes the tag register at R11, so the second `allocateScratchReg` would trip
    /// `allocateKnownReg`'s "register should not already be allocated"
    /// (notes/support.bpl:434). None of the 5 declarations in CacheIRCompiler.cpp
    /// nests, so this is defensive.
    pub fn acquire_tag_scope(&mut self) -> bool {
        if self
            .0
            .iter()
            .any(|scope| scope.owed.contains(&Obligation::ReleaseScratchReg))
        {
            return false;
        }
        self.owe(Obligation::ReleaseScratchReg);
        true
    }

    /// Records that this block has lent the tag register out and takes it back at
    /// its end.
    pub fn lend_tag_scope(&mut self) {
        self.owe(Obligation::ReacquireScratchReg);
    }


    /// Discharges what a `return` here owes, and says what discharges it.
    ///
    /// A `return` leaves every enclosing block, so it owes all of them -- innermost
    /// first, and within each in reverse of the order taken on, which is the order
    /// C++ runs the destructors in.
    ///
    /// One call rather than a query and a separate update: the obligation is gone
    /// once it has been reported, so there is no state in which a caller can see it
    /// and fail to emit it, or see it twice.
    ///
    /// Only this block is marked discharged. An enclosing block's fall-through path
    /// still owes its own, being a different path.
    pub fn on_return(&mut self) -> Vec<Obligation> {
        let obligations = self
            .0
            .iter()
            .rev()
            .flat_map(|scope| scope.owed.iter().rev().copied())
            .collect();
        if let Some(scope) = self.0.last_mut() {
            scope.owed.clear();
        }
        obligations
    }

    /// Discharges what falling out of this block owes: its own obligations only.
    ///
    /// An enclosing block's are not discharged here -- a failure path taken out
    /// further up is still in use afterwards, and a register further up is still
    /// held.
    pub fn on_scope_end(&mut self) -> Vec<Obligation> {
        match self.0.last_mut() {
            Some(scope) => scope.owed.drain(..).rev().collect(),
            None => Vec::new(),
        }
    }

    /// Records a register-valued local in the innermost block. Dropped when there
    /// is no block, which cannot happen while translating one.
    pub fn insert_register(&mut self, name: String, id_ty: CachetPath) {
        if let Some(scope) = self.0.last_mut() {
            scope.registers.insert(name, id_ty);
        }
    }

    /// The operand id `name` was bound from, searching inward blocks first so a
    /// shadowing declaration wins, as it does in C++.
    pub fn register(&self, name: &str) -> Option<&CachetPath> {
        self.0
            .iter()
            .rev()
            .find_map(|scope| scope.registers.get(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `emitGuardClass` shape: acquired in the op's block, with a `return`
    /// inside a branch and more code after it still using the path.
    ///
    /// ```text
    /// addFailurePath(&failure);
    /// if (kind == JSFunction) { ..; return true; }
    /// masm.branchTestObjClass(.., failure->label());
    /// return true;
    /// ```
    #[test]
    fn a_return_in_a_branch_leaves_the_enclosing_block_owing() {
        let mut scopes = Scopes::default();
        scopes.push();
        assert!(scopes.acquire_failure_path());

        scopes.push();
        assert_eq!(
            scopes.on_return(),
            [Obligation::ReleaseFailurePath],
            "the branch's return releases"
        );
        assert!(
            scopes.on_scope_end().is_empty(),
            "and having released, the branch's end owes nothing"
        );
        scopes.pop();

        assert_eq!(
            scopes.on_return(),
            [Obligation::ReleaseFailurePath],
            "the fall-through path still owes its own release"
        );
        assert!(
            scopes.on_scope_end().is_empty(),
            "which the return discharged"
        );
    }

    /// A block that only inherits the path must not release it: the block that
    /// acquired it goes on using it afterwards.
    ///
    /// ```text
    /// addFailurePath(&failure);
    /// if (x) { masm.foo(); }
    /// masm.bar(failure->label());
    /// ```
    #[test]
    fn an_inheriting_block_does_not_release_early() {
        let mut scopes = Scopes::default();
        scopes.push();
        assert!(scopes.acquire_failure_path());

        scopes.push();
        assert!(scopes.on_scope_end().is_empty());
        scopes.pop();

        assert_eq!(
            scopes.on_scope_end(),
            [Obligation::ReleaseFailurePath],
            "the acquirer releases"
        );
    }

    /// What `setAddedFailurePath` asserts: "multiple failure paths for
    /// instruction".
    #[test]
    fn a_second_failure_path_is_refused() {
        let mut scopes = Scopes::default();
        scopes.push();
        assert!(scopes.acquire_failure_path());
        assert!(!scopes.acquire_failure_path());

        scopes.push();
        assert!(
            !scopes.acquire_failure_path(),
            "including from a block that only inherited it"
        );
    }

    /// Two branches may each take one out, the first having released at its end.
    #[test]
    fn sibling_blocks_may_each_acquire() {
        let mut scopes = Scopes::default();
        scopes.push();

        scopes.push();
        assert!(scopes.acquire_failure_path());
        assert_eq!(scopes.on_scope_end(), [Obligation::ReleaseFailurePath]);
        scopes.pop();

        scopes.push();
        assert!(scopes.acquire_failure_path());
        scopes.pop();
    }

    #[test]
    fn an_inner_register_shadows_an_outer_one() {
        let int32 = CachetPath::from_ident("Int32Id");
        let bool_ = CachetPath::from_ident("BoolId");

        let mut scopes = Scopes::default();
        scopes.push();
        scopes.insert_register(String::from("input"), int32.clone());
        assert_eq!(scopes.register("input"), Some(&int32));

        scopes.push();
        scopes.insert_register(String::from("input"), bool_.clone());
        assert_eq!(scopes.register("input"), Some(&bool_));
        scopes.pop();

        assert_eq!(scopes.register("input"), Some(&int32));
    }
}
