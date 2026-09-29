//! What the translator knows about the locals in scope.
//!
//! Its own module because both sides need it: `cpp_to_cachet` records what a
//! declaration establishes, and `masm_ops` reads it back to decide which op a
//! masm call means.

use std::collections::HashMap;

use cachet_lang::ast::Path as CachetPath;

/// Whether a failure path is outstanding at a point in the walk, and what the
/// current block therefore owes.
///
/// The model requires `addFailurePath` and `releaseFailurePath` to balance --
/// each asserts the other's state (notes/cacheir.cachet:1806-1820) -- while C++
/// needs no release at all, `addedFailurePath_` being cleared by `nextOp()`
/// between instructions. So every release is inserted, with no C++ line behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Held {
    #[default]
    No,
    /// Responsibility: release on return.
    Inherited,
    /// Responsibility: release on return & scope end.
    Owned,
}

/// Something the translator must emit on leaving a point in the walk, with no C++
/// line behind it.
///
/// A list rather than a single value because the kinds will multiply: an
/// `AutoScratchRegister` going out of scope owes a `releaseReg` in just the same
/// way, differing only in that C++ does emit that one, from a destructor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Obligation {
    ReleaseFailurePath,
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
    failure_path: Held,
}

/// The blocks enclosing the statement being translated, innermost last.
///
/// A frame per block, because a C++ declaration is scoped to its block and a
/// sibling block may reuse the name for a register of another kind.
#[derive(Debug, Default)]
pub struct Scopes(Vec<Scope>);

impl Scopes {
    /// A block inherits an outstanding failure path from the one enclosing it, so
    /// a `return` anywhere inside releases it.
    pub fn push(&mut self) {
        let failure_path = match self.failure_path() {
            Held::No => Held::No,
            Held::Inherited | Held::Owned => Held::Inherited,
        };
        self.0.push(Scope {
            failure_path,
            ..Scope::default()
        });
    }

    pub fn pop(&mut self) {
        self.0.pop();
    }

    fn failure_path(&self) -> Held {
        self.0.last().map_or(Held::No, |scope| scope.failure_path)
    }

    fn set_failure_path(&mut self, held: Held) {
        if let Some(scope) = self.0.last_mut() {
            scope.failure_path = held;
        }
    }

    /// Records that this block took out a failure path.
    ///
    /// `false` when one is already outstanding, which the model forbids and C++
    /// asserts against -- "multiple failure paths for instruction". The caller
    /// reports it, since it is a fault in the C++ rather than a gap in the
    /// translation.
    pub fn acquire_failure_path(&mut self) -> bool {
        if self.failure_path() != Held::No {
            return false;
        }
        self.set_failure_path(Held::Owned);
        true
    }

    /// Discharges what a `return` here owes, and says what discharges it.
    ///
    /// One call rather than a query and a separate update: the obligation is gone
    /// once it has been reported, so there is no state in which a caller can see it
    /// and fail to emit it, or see it twice.
    ///
    /// Only this block is marked discharged. An enclosing block's fall-through path
    /// still owes its own, being a different path.
    pub fn on_return(&mut self) -> Vec<Obligation> {
        if self.failure_path() == Held::No {
            return Vec::new();
        }
        self.set_failure_path(Held::No);
        vec![Obligation::ReleaseFailurePath]
    }

    /// Discharges what falling out of this block owes.
    ///
    /// Only the block that acquired the failure path owes one; an inheriting block
    /// would be releasing while the acquirer is still using it.
    pub fn on_scope_end(&mut self) -> Vec<Obligation> {
        if self.failure_path() != Held::Owned {
            return Vec::new();
        }
        self.set_failure_path(Held::No);
        vec![Obligation::ReleaseFailurePath]
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
