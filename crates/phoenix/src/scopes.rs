//! What the translator knows about the locals in scope.
//!
//! Its own module because both sides need it: `cpp_to_cachet` records what a
//! declaration establishes, and `masm_ops` reads it back to decide which op a
//! masm call means.

use std::collections::HashMap;

use cachet_lang::ast::Path as CachetPath;

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
