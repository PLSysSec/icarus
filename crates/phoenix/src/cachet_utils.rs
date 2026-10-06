//! Working with the Cachet AST phoenix writes and reads back.
//!
//! `cachet-lang` has no traversal to borrow: each compiler pass descends by hand over
//! its own stage's AST (`type_checker.rs`, `normalizer.rs`, `flattener.rs`), and none
//! of them walks the parser's. So this is the parser-side walk phoenix needs, in the
//! same shape as [`crate::cpp_subset`]'s: a [`Visit`] trait whose default methods
//! delegate to free `walk_*` functions, so the accumulator lives in the implementor
//! and one traversal serves any number of questions.

use std::collections::BTreeSet;

use cachet_lang::ast::{Ident, Path as CachetPath, Spanned};
use cachet_lang::parser::{Arg, Block, Call, ElseClause, Expr, FreeArg, IfExpr, Item, Stmt};

/// An argument, normalized as the parser normalizes one.
///
/// Why this exists rather than `Arg::Expr` everywhere: a bare name and a field access
/// in argument position are *ambiguous* in Cachet -- either could be a label -- and
/// the grammar (grammar.lalrpop:177) defers the choice to name resolution, which knows
/// the callee's signature. phoenix does not: it never reads the model, so it cannot
/// tell which parameters are labels.
///
/// Building `Arg::Expr(Expr::Var(..))` instead would assert "variable" in a position
/// the language leaves open. It happens to work only because phoenix emits text that
/// the parser re-reads and re-normalizes; producing the same nodes the parser would
/// makes that round-trip incidental rather than load-bearing.
pub fn to_arg(expr: Expr) -> Arg {
    match expr {
        Expr::Var(path) => Arg::Free(FreeArg {
            path,
            is_out: false,
        }),
        Expr::FieldAccess(field_access) => Arg::FieldAccess(*field_access),
        expr => Arg::Expr(expr),
    }
}

pub trait Visit {
    fn visit_item(&mut self, item: &Item) {
        walk_item(self, item);
    }
    fn visit_block(&mut self, block: &Block) {
        walk_block(self, block);
    }
    fn visit_stmt(&mut self, stmt: &Stmt) {
        walk_stmt(self, stmt);
    }
    fn visit_if(&mut self, s: &IfExpr) {
        walk_if(self, s);
    }
    /// Walked because a block and an `if` are *expressions*, so an `emit` does hide
    /// in one: `if c { emit Foo(); }` is an expression statement.
    fn visit_expr(&mut self, expr: &Expr) {
        walk_expr(self, expr);
    }
    /// A leaf, so the default does nothing rather than descending.
    fn visit_emit(&mut self, _call: &Call) {}
}

// `?Sized` so a default method can pass its `&mut Self` here: inside a trait, `Self`
// is not known to be sized (it could be `dyn Visit`).

pub fn walk_items<V: Visit + ?Sized>(v: &mut V, items: &[Spanned<Item>]) {
    for item in items {
        v.visit_item(&item.value);
    }
}

pub fn walk_item<V: Visit + ?Sized>(v: &mut V, item: &Item) {
    match item {
        // An `ir` or an `impl` holds items of its own -- a generator's `op` lives
        // inside its `ir`.
        Item::Ir(i) => walk_items(v, &i.items),
        Item::Impl(i) => walk_items(v, &i.items),
        Item::Fn(c) | Item::Op(c) => {
            if let Some(body) = &c.body.value {
                v.visit_block(body);
            }
        }
        // Nothing else holds statements.
        Item::Comment(_)
        | Item::Enum(_)
        | Item::Import(_)
        | Item::Struct(_)
        | Item::GlobalVar(_) => {}
    }
}

pub fn walk_block<V: Visit + ?Sized>(v: &mut V, block: &Block) {
    for stmt in &block.stmts {
        v.visit_stmt(&stmt.value);
    }
    // A trailing block or `if` is the block's *value* rather than a statement -- the
    // grammar prefers that reading, which is what lets `{ .. if c { 1 } else { 2 } }`
    // produce a value. Walking the statements alone would miss whatever it contains.
    if let Some(value) = &block.value.value {
        v.visit_expr(value);
    }
}

pub fn walk_stmt<V: Visit + ?Sized>(v: &mut V, stmt: &Stmt) {
    match stmt {
        Stmt::Emit(call) => v.visit_emit(call),
        Stmt::ForIn(s) => v.visit_block(&s.body),
        // Where a block or an `if` lives, with or without a trailing semicolon.
        Stmt::Expr(expr) | Stmt::Semi(expr) => v.visit_expr(expr),
        // No statements inside. A `let`'s initializer and a `check`'s condition are
        // expressions, which could in principle hold a block -- see [`walk_expr`].
        Stmt::Comment(_)
        | Stmt::Let(_)
        | Stmt::Label(_)
        | Stmt::Check(_)
        | Stmt::Goto(_)
        | Stmt::Bind(_)
        | Stmt::Ret(_)
        | Stmt::Unreachable => {}
    }
}

pub fn walk_expr<V: Visit + ?Sized>(v: &mut V, expr: &Expr) {
    match expr {
        // The two that hold statements, which is where an `emit` can be.
        Expr::Block(b) => v.visit_block(&b.block),
        Expr::If(s) => v.visit_if(s),
        // Hold expressions rather than statements. A block nested inside one --
        // `f({ emit Foo(); 1 })` -- would be missed, which no generated module
        // produces: phoenix emits only in statement position. Listed rather than
        // wildcarded so a new variant has to be considered here.
        Expr::Literal(_)
        | Expr::Var(_)
        | Expr::Invoke(_)
        | Expr::FieldAccess(_)
        | Expr::Negate(_)
        | Expr::Cast(_)
        | Expr::BinOper(_)
        | Expr::Assign(_) => {}
    }
}

pub fn walk_if<V: Visit + ?Sized>(v: &mut V, s: &IfExpr) {
    v.visit_block(&s.then);
    match &s.else_ {
        Some(ElseClause::ElseIf(s)) => v.visit_if(s),
        Some(ElseClause::Else(block)) => v.visit_block(block),
        None => {}
    }
}

/// The ops a translated module emits into `ir`, as `emit <ir>::<Op>` names it.
///
/// This decides which CacheIR instructions a stub generator needs: the module is the
/// artifact handed to the verifier, so what it references is exactly what has to be
/// defined beside it. Reading the output rather than the C++ also catches an op
/// reached only through a wrapper phoenix synthesized, which no C++ statement
/// mentions.
///
/// One pass suffices, and that is a fact about the model rather than luck:
/// `ir CacheIR emits MASM` (notes/cacheir.cachet:443), so a CacheIR op can only emit
/// MASM. Translating the ops this finds cannot turn up more of them.
///
/// Sorted, so the generated module's order does not depend on a hash seed.
pub fn emitted_ops(items: &[Spanned<Item>], ir: &str) -> BTreeSet<Ident> {
    let mut collector = EmittedOps {
        ir,
        ops: BTreeSet::new(),
    };
    walk_items(&mut collector, items);
    collector.ops
}

struct EmittedOps<'a> {
    ir: &'a str,
    ops: BTreeSet<Ident>,
}

impl Visit for EmittedOps<'_> {
    fn visit_emit(&mut self, call: &Call) {
        if let Some(op) = qualified_by(&call.target.value, self.ir) {
            self.ops.insert(op);
        }
    }
}

/// The last segment of `path`, if `path` is exactly `<ir>::<segment>`.
///
/// Qualification is what tells an op from a function: `emit CacheIR::GuardIsNull` is
/// an op, while `CacheIR::newInt32Id(..)` is a `fn` on the same `ir` and belongs to
/// the imported model, not to the generated subset.
fn qualified_by(path: &CachetPath, ir: &str) -> Option<Ident> {
    let parent = path.parent()?;
    (!parent.has_parent() && parent.ident().to_string() == ir).then(|| path.ident())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cachet_lang::parser::{Files, parse};

    fn ops_of(src: &str) -> Vec<String> {
        // Parsing needs a `FileId` only so errors can point somewhere.
        let mut files = Files::new();
        let file_id = files.add("test.cachet", src.to_owned());
        let items = parse(file_id, src).expect("test source should parse").items;
        emitted_ops(&items, "CacheIR")
            .into_iter()
            .map(|op| op.to_string())
            .collect()
    }

    #[test]
    fn an_emit_inside_a_generator_op_is_found() {
        assert_eq!(
            ops_of(
                "ir CompareIRGenerator emits CacheIR {
                     op TryAttachInt32() {
                         emit CacheIR::GuardToInt32(lhsId);
                     }
                 }"
            ),
            ["GuardToInt32"]
        );
    }

    /// A wrapper is a top-level `fn`, and it is the only place a result-bearing op is
    /// emitted from.
    #[test]
    fn an_emit_inside_a_helper_fn_is_found() {
        assert_eq!(
            ops_of(
                "fn loadInt32Constant(val: Int32) emits CacheIR -> Int32Id {
                     emit CacheIR::LoadInt32Constant(valField, result);
                     return result;
                 }"
            ),
            ["LoadInt32Constant"]
        );
    }

    /// `EmitGuardToInt32ForToNumber` emits from inside an `if`, and an `else if` chain
    /// nests, so a shallow walk would miss the later rungs.
    #[test]
    fn emits_nested_in_branches_are_found() {
        assert_eq!(
            ops_of(
                "fn f(v: Value) emits CacheIR {
                     if Value::isInt32(v) {
                         emit CacheIR::GuardToInt32(id);
                     } else if Value::isNull(v) {
                         emit CacheIR::GuardIsNull(id);
                     } else {
                         emit CacheIR::GuardBooleanToInt32(id, result);
                     }
                 }"
            ),
            ["GuardBooleanToInt32", "GuardIsNull", "GuardToInt32"]
        );
    }

    /// A `fn` on the same `ir` is not an op: `CacheIR::newInt32Id` is part of the
    /// imported model, and only `emit` reaches an op.
    #[test]
    fn a_call_to_an_ir_function_is_not_an_op() {
        assert_eq!(
            ops_of(
                "fn f() emits CacheIR -> Int32Id {
                     let result = CacheIR::newInt32Id();
                     CacheIR::emitLoadInt32StubField(val, reg);
                     return result;
                 }"
            ),
            Vec::<String>::new()
        );
    }

    /// MASM ops are emitted by the instructions, not by the generator, and they are
    /// not what the generated `ir CacheIR` has to hold.
    #[test]
    fn an_emit_into_another_ir_is_ignored() {
        assert_eq!(
            ops_of(
                "fn f() emits MASM {
                     emit MASM::Jump(done);
                 }"
            ),
            Vec::<String>::new()
        );
    }
}
