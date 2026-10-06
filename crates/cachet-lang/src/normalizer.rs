// vim: set tw=99 ts=4 sts=4 sw=4 et:

mod ast;

use std::ops::{Deref, DerefMut};

use lazy_static::lazy_static;
use typed_index_collections::{TiSlice, TiVec};

use crate::type_checker;

use crate::ast::{
    BinOper, BlockKind, Ident, LogicalBinOper, NegateKind, Span, Spanned, VarParamKind,
};
use crate::built_in::{BuiltInType, BuiltInVar};

pub use crate::normalizer::ast::*;

lazy_static! {
    static ref TMP_VAR_IDENT: Ident = "tmp".into();
}

pub fn normalize(env: type_checker::Env) -> Env {
    let global_var_items = &env.global_var_items;

    let fn_items: TiVec<FnIndex, _> = env
        .fn_items
        .into_iter()
        .map(|fn_item| normalize_callable_item(global_var_items, fn_item))
        .collect();

    let op_items: TiVec<OpIndex, _> = env
        .op_items
        .into_iter()
        .map(|op_item| normalize_callable_item(global_var_items, op_item))
        .collect();

    let global_var_items: TiVec<GlobalVarIndex, _> = env
        .global_var_items
        .iter()
        .map(|global_var_item| normalize_global_var_item(global_var_items, global_var_item))
        .collect();

    Env {
        enum_items: env.enum_items,
        struct_items: env.struct_items,
        ir_items: env.ir_items,
        global_var_items,
        fn_items,
        op_items,
        decl_order: env.decl_order,
    }
}

fn normalize_global_var_item(
    global_var_items: &TiSlice<GlobalVarIndex, type_checker::GlobalVarItem>,
    global_var_item: &type_checker::GlobalVarItem,
) -> GlobalVarItem {
    let value = global_var_item.value.as_ref().map(|value| {
        let mut local_vars = TiVec::new();
        let mut normalizer =
            Normalizer::new(global_var_items, TiSlice::from_ref(&[]), &mut local_vars);

        value
            .clone()
            .map(|value| normalizer.normalize_const_expr(value))
    });

    GlobalVarItem {
        path: global_var_item.path,
        parent: global_var_item.parent,
        is_mut: global_var_item.is_mut,
        type_: global_var_item.type_,
        value,
        attrs: global_var_item.attrs,
    }
}

fn normalize_callable_item(
    global_var_items: &TiSlice<GlobalVarIndex, type_checker::GlobalVarItem>,
    callable_item: type_checker::CallableItem,
) -> CallableItem {
    let ret = if callable_item.ret == BuiltInType::Unit.into() {
        None
    } else {
        Some(callable_item.ret)
    };

    let body = callable_item.body.map(|mut body| {
        let mut normalizer = Normalizer::new(
            global_var_items,
            &callable_item.params.var_params,
            &mut body.locals.local_vars,
        );
        let stmts = normalizer.normalize_body_block(body.block);
        Body {
            locals: body.locals,
            stmts,
        }
    });

    CallableItem {
        path: callable_item.path,
        parent: callable_item.parent,
        is_unsafe: callable_item.is_unsafe,
        params: callable_item.params,
        param_order: callable_item.param_order,
        ret,
        interprets: callable_item.interprets,
        emits: callable_item.emits,
        body,
        attrs: callable_item.attrs,
    }
}

struct Normalizer<'a> {
    global_var_items: &'a TiSlice<GlobalVarIndex, type_checker::GlobalVarItem>,
    var_params: &'a TiSlice<VarParamIndex, VarParam>,
    local_vars: &'a mut TiVec<LocalVarIndex, LocalVar>,
}

impl<'a> Normalizer<'a> {
    fn new(
        global_var_items: &'a TiSlice<GlobalVarIndex, type_checker::GlobalVarItem>,
        var_params: &'a TiSlice<VarParamIndex, VarParam>,
        local_vars: &'a mut TiVec<LocalVarIndex, LocalVar>,
    ) -> Self {
        Normalizer {
            global_var_items,
            var_params,
            local_vars,
        }
    }

    fn normalize_const_expr(&mut self, expr: type_checker::Expr) -> Expr {
        let mut stmts = vec![];
        let mut scoped_normalizer = ScopedNormalizer::new(self, &mut stmts);

        let expr = scoped_normalizer.normalize_expr(expr);
        debug_assert!(
            stmts.len() == 0,
            "const expressions should not require preceding stmts"
        );

        expr
    }

    fn normalize_body_block(&mut self, block: type_checker::Block) -> Vec<Stmt> {
        // If the body block always exits early, any value expression at the end
        // of the block will be unused. We should also treat the value
        // expression as unused if it's unit-typed, since unit-return-types get
        // erased past the normalization stage.
        if block.exits_early || block.type_() == BuiltInType::Unit.into() {
            let block_exits_early = block.exits_early;
            let mut stmts = self.normalize_unused_block(block);
            // Passes downstream of the normalizer should be able to rely on the
            // property that callables only return at explicit return
            // statements. If the body block doesn't always exit early, we
            // should demarcate the return point at the end of the block with an
            // explicit early return (expressionless, because if this is true
            // here then we know the block is unit-typed).
            if !block_exits_early {
                stmts.push(RetStmt { value: None }.into());
            }
            return stmts;
        }

        let mut stmts = self.normalize_block_stmts(block.stmts);
        let mut scoped_normalizer = ScopedNormalizer::new(self, &mut stmts);
        let value = scoped_normalizer.normalize_used_expr(block.value);
        stmts.push(RetStmt { value: Some(value) }.into());
        stmts
    }

    fn normalize_unused_block(&mut self, block: type_checker::Block) -> Vec<Stmt> {
        let mut stmts = self.normalize_block_stmts(block.stmts);
        let mut scoped_normalizer = ScopedNormalizer::new(self, &mut stmts);
        scoped_normalizer.normalize_unused_expr(block.value);
        stmts
    }

    fn normalize_block_stmts(&mut self, stmts: Vec<type_checker::Stmt>) -> Vec<Stmt> {
        let mut normalized_stmts = Vec::with_capacity(stmts.len());
        let mut scoped_normalizer = ScopedNormalizer::new(self, &mut normalized_stmts);

        for stmt in stmts {
            scoped_normalizer.normalize_stmt(stmt);
        }

        normalized_stmts
    }

    fn is_mut_var(&self, var_index: VarIndex) -> bool {
        match var_index {
            VarIndex::BuiltIn(_) | VarIndex::EnumVariant(_) => false,
            VarIndex::Global(global_var_index) => self.global_var_items[global_var_index].is_mut,
            VarIndex::Param(var_param_index) => {
                self.var_params[var_param_index].kind == VarParamKind::Mut
            }
            VarIndex::Local(local_var_index) => self.local_vars[local_var_index].is_mut,
        }
    }
}

struct ScopedNormalizer<'a, 'b> {
    normalizer: &'b mut Normalizer<'a>,
    stmts: &'b mut Vec<Stmt>,
    exported_stmts: Option<&'b mut Vec<Stmt>>,
}

impl<'a> Deref for ScopedNormalizer<'a, '_> {
    type Target = Normalizer<'a>;

    fn deref(&self) -> &Self::Target {
        self.normalizer
    }
}

impl<'a> DerefMut for ScopedNormalizer<'a, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.normalizer
    }
}

impl<'a, 'b> ScopedNormalizer<'a, 'b> {
    fn new(normalizer: &'b mut Normalizer<'a>, stmts: &'b mut Vec<Stmt>) -> Self {
        ScopedNormalizer {
            normalizer,
            stmts,
            exported_stmts: None,
        }
    }

    fn recurse<'c>(&'c mut self, stmts: &'c mut Vec<Stmt>) -> ScopedNormalizer<'a, 'c> {
        ScopedNormalizer {
            normalizer: self.normalizer,
            stmts,
            exported_stmts: Some(
                self.exported_stmts
                    .as_deref_mut()
                    .unwrap_or(&mut self.stmts),
            ),
        }
    }

    fn nest<'c>(&'c mut self, stmts: &'c mut Vec<Stmt>) -> ScopedNormalizer<'a, 'c> {
        ScopedNormalizer {
            normalizer: self.normalizer,
            stmts,
            exported_stmts: None,
        }
    }

    fn normalize_arg(&mut self, arg: type_checker::Arg) -> Arg {
        match arg {
            type_checker::Arg::Expr(expr) => self.normalize_pure_expr(expr).into(),
            type_checker::Arg::OutVar(out_var_arg) => OutVarArg {
                var: match out_var_arg.out_var {
                    type_checker::OutVar::Free(var_index) => var_index.value,
                    type_checker::OutVar::Fresh(local_var_index) => {
                        self.export_stmt(
                            LetStmt {
                                lhs: local_var_index,
                                type_: out_var_arg.type_,
                                rhs: None,
                            }
                            .into(),
                        );
                        local_var_index.into()
                    }
                },
                type_: out_var_arg.type_,
                upcast_route: out_var_arg.upcast_route,
            }
            .into(),
            type_checker::Arg::Label(label_arg) => LabelArg {
                label: label_arg.label.value,
                is_out: false,
                ir: label_arg.ir,
            }
            .into(),
            type_checker::Arg::LabelField(label_field_arg) => {
                let parent = self.normalize_pure_expr(label_field_arg.parent);
                LabelFieldArg {
                    parent,
                    field: label_field_arg.field,
                    ir: label_field_arg.ir,
                }
                .into()
            }
            type_checker::Arg::OutLabel(out_label_arg) => LabelArg {
                label: match out_label_arg.out_label {
                    type_checker::OutLabel::Free(label_index) => label_index.value,
                    type_checker::OutLabel::Fresh(local_label_index) => {
                        self.export_stmt(
                            LabelStmt {
                                label: local_label_index,
                            }
                            .into(),
                        );
                        local_label_index.into()
                    }
                },
                is_out: true,
                ir: out_label_arg.ir,
            }
            .into(),
        }
    }

    fn normalize_call(&mut self, call: type_checker::Call) -> (Vec<Stmt>, Call) {
        let mut stmts = Vec::new();
        let mut scoped_normalizer = self.recurse(&mut stmts);

        let args = call
            .args
            .into_iter()
            .map(|arg| scoped_normalizer.normalize_arg(arg))
            .collect();

        (
            stmts,
            Call {
                target: call.target,
                is_unsafe: call.is_unsafe,
                args,
            },
        )
    }

    fn normalize_used_call(
        &mut self,
        call: type_checker::Call,
        expr_ctor: impl FnOnce(Call) -> Expr,
    ) -> Expr {
        let (stmts, call) = self.normalize_call(call);
        let value = expr_ctor(call);
        block_if_necessary(None, stmts, value)
    }

    fn normalize_unused_call(
        &mut self,
        call: type_checker::Call,
        stmt_ctor: impl FnOnce(Call) -> Stmt,
    ) {
        let (mut stmts, call) = self.normalize_call(call);
        let stmt = stmt_ctor(call);

        stmts.push(stmt);
        self.stmts.append(&mut stmts);
    }

    fn normalize_stmt(&mut self, stmt: type_checker::Stmt) {
        match stmt {
            type_checker::Stmt::Let(let_stmt) => self.normalize_let_stmt(let_stmt),
            type_checker::Stmt::Label(label_stmt) => self.stmts.push(label_stmt.into()),
            type_checker::Stmt::ForIn(for_in_stmt) => self.normalize_for_in_stmt(for_in_stmt),
            type_checker::Stmt::Check(check_stmt) => self.normalize_check_stmt(check_stmt),
            type_checker::Stmt::Goto(goto_stmt) => self.stmts.push(goto_stmt.into()),
            type_checker::Stmt::Bind(bind_stmt) => self.stmts.push(bind_stmt.into()),
            type_checker::Stmt::Emit(emit_stmt) => self.normalize_emit_stmt(emit_stmt),
            type_checker::Stmt::Expr(expr) => self.normalize_unused_expr(expr),
            type_checker::Stmt::Ret(ret_stmt) => self.normalize_ret_stmt(ret_stmt),
            type_checker::Stmt::Unreachable => self.stmts.push(Stmt::Unreachable),
        }
    }

    fn normalize_let_stmt(&mut self, let_stmt: type_checker::LetStmt) {
        let rhs = self.normalize_expr(let_stmt.rhs);

        self.stmts.push(
            LetStmt {
                lhs: let_stmt.lhs,
                type_: rhs.type_(),
                rhs: Some(rhs),
            }
            .into(),
        );
    }

    /// Lowers an `if` to the statement the backends understand, with each arm
    /// assigning its value to `target` where there is one.
    ///
    /// The condition's own statements go to `self.stmts`, ahead of the `if`, the
    /// condition being evaluated on every path.
    fn normalize_if_expr_recurse(
        &mut self,
        if_expr: type_checker::IfExpr,
        target: Option<VarIndex>,
    ) -> IfStmt {
        let cond = self.normalize_expr(if_expr.cond);
        let then = self.normalize_arm(if_expr.then, target);
        let else_ = if_expr.else_.map(|else_| match else_ {
            type_checker::ElseClause::Else(else_block) => {
                ast::ElseClause::Else(self.normalize_arm(else_block, target))
            }
            type_checker::ElseClause::ElseIf(else_if) => {
                ast::ElseClause::ElseIf(Box::new(self.normalize_if_expr_recurse(*else_if, target)))
            }
        });

        IfStmt { cond, then, else_ }
    }

    /// One arm of an `if`, assigning its value to `target` where there is one.
    ///
    /// `None` is the statement-position case, which discards the arms' values. An arm
    /// that exits early discards its value either way: the value is never reached --
    /// it is the `unit` standing in for a missing one -- so assigning it would be
    /// ill-typed as well as unreachable.
    ///
    /// The arm's work rides along inside a block expression, which the flattener
    /// hoists into the arm. That keeps it conditional, the same way the `&&` lowering
    /// in [`ScopedNormalizer::normalize_used_bin_oper_expr`] keeps its right operand
    /// conditional.
    fn normalize_arm(
        &mut self,
        block: type_checker::Block,
        target: Option<VarIndex>,
    ) -> Vec<Stmt> {
        let Some(target) = target.filter(|_| !block.exits_early) else {
            return self.normalize_unused_block(block);
        };

        let value = self.normalize_used_block_expr(block.into());
        vec![
            AssignStmt {
                lhs: target,
                rhs: value,
            }
            .into(),
        ]
    }

    /// An `if` in statement position, where the arms' values go nowhere.
    fn normalize_unused_if_expr(&mut self, if_expr: type_checker::IfExpr) {
        let if_ = self.normalize_if_expr_recurse(if_expr, None);
        self.stmts.push(if_.into());
    }

    /// An `if` whose value is wanted. Neither backend has a conditional expression,
    /// so it becomes a local the arms assign to, and the local is the value.
    fn normalize_used_if_expr(&mut self, if_expr: type_checker::IfExpr) -> Expr {
        let output_var_expr = self.push_mut_tmp_expr(if_expr.type_());

        let if_ = self.normalize_if_expr_recurse(if_expr, Some(output_var_expr.var.into()));
        self.stmts.push(if_.into());

        output_var_expr.into()
    }

    fn normalize_for_in_stmt(&mut self, for_in_stmt: type_checker::ForInStmt) {
        let body = self.normalize_unused_block(for_in_stmt.body);

        self.stmts.push(
            ForInStmt {
                var: for_in_stmt.var,
                target: for_in_stmt.target,
                order: for_in_stmt.order,
                body,
            }
            .into(),
        );
    }

    fn normalize_check_stmt(&mut self, check_stmt: type_checker::CheckStmt) {
        let cond = self.normalize_expr(check_stmt.cond);

        self.stmts.push(
            CheckStmt {
                kind: check_stmt.kind,
                cond,
            }
            .into(),
        );
    }

    fn normalize_ret_stmt(&mut self, ret_stmt: type_checker::RetStmt) {
        let value = if ret_stmt.value.type_() == BuiltInType::Unit.into() {
            // Unit return types are erased by normalization, so we should treat the
            // return value expression as unused and emit an expressionless
            // return statement.
            self.normalize_unused_expr(ret_stmt.value);
            None
        } else {
            Some(self.normalize_used_expr(ret_stmt.value))
        };

        self.stmts.push(RetStmt { value }.into());
    }

    fn normalize_emit_stmt(&mut self, emit_stmt: type_checker::EmitStmt) {
        self.normalize_unused_call(emit_stmt.call, |call| {
            EmitStmt {
                call,
                ir: emit_stmt.ir,
            }
            .into()
        });
    }

    fn normalize_expr(&mut self, expr: type_checker::Expr) -> Expr {
        if expr.type_() == BuiltInType::Unit.into() {
            self.normalize_unused_expr(expr);
            BuiltInVar::Unit.into()
        } else {
            self.normalize_used_expr(expr)
        }
    }

    fn normalize_pure_expr(&mut self, expr: type_checker::Expr) -> PureExpr {
        match self.normalize_expr(expr).try_into() {
            Ok(pure_expr) => match pure_expr {
                // If the variable is mutable, we need to save its value at the
                // correct point in evaluation.
                PureExpr::Var(var_expr) if self.is_mut_var(var_expr.var) => {
                    self.push_tmp_expr(var_expr.into()).into()
                }
                pure_expr @ _ => pure_expr,
            },
            Err(expr) => {
                debug_assert_ne!(expr.type_(), BuiltInType::Unit.into());
                self.push_tmp_expr(expr).into()
            }
        }
    }

    fn normalize_used_expr(&mut self, expr: type_checker::Expr) -> Expr {
        debug_assert_ne!(expr.type_(), BuiltInType::Unit.into());

        match expr {
            type_checker::Expr::Block(block) => self.normalize_used_block_expr(*block),
            type_checker::Expr::If(if_expr) => self.normalize_used_if_expr(*if_expr),
            type_checker::Expr::Literal(literal) => literal.into(),
            type_checker::Expr::Var(var_expr) => var_expr.into(),
            type_checker::Expr::Invoke(invoke_expr) => {
                self.normalize_used_invoke_expr(invoke_expr)
            }
            type_checker::Expr::FieldAccess(field_access_expr) => self
                .normalize_used_field_access_expr(*field_access_expr)
                .into(),
            type_checker::Expr::Negate(negate_expr) => {
                self.normalize_used_negate_expr(*negate_expr).into()
            }
            type_checker::Expr::Cast(cast_expr) => {
                self.normalize_used_cast_expr(*cast_expr).into()
            }
            type_checker::Expr::BinOper(bin_oper_expr) => {
                self.normalize_used_bin_oper_expr(*bin_oper_expr).into()
            }
            type_checker::Expr::Assign(_) => {
                unreachable!("assignment expressions should be `Unit`-typed")
            }
        }
    }

    fn normalize_unused_expr(&mut self, expr: type_checker::Expr) {
        match expr {
            type_checker::Expr::Block(block) => self.normalize_unused_block_expr(*block),
            type_checker::Expr::If(if_expr) => self.normalize_unused_if_expr(*if_expr),
            type_checker::Expr::Literal(_) => (),
            type_checker::Expr::Var(_) => (),
            type_checker::Expr::Invoke(invoke_expr) => {
                self.normalize_unused_invoke_expr(invoke_expr)
            }
            type_checker::Expr::FieldAccess(field_access_expr) => {
                self.normalize_unused_field_access_expr(*field_access_expr)
            }
            type_checker::Expr::Negate(negate_expr) => {
                self.normalize_unused_negate_expr(*negate_expr)
            }
            type_checker::Expr::Cast(cast_expr) => self.normalize_unused_cast_expr(*cast_expr),
            type_checker::Expr::BinOper(bin_oper_expr) => {
                self.normalize_unused_bin_oper_expr(*bin_oper_expr)
            }
            type_checker::Expr::Assign(assign_expr) => {
                self.normalize_unused_assign_expr(*assign_expr)
            }
        }
    }

    fn normalize_used_block_expr(&mut self, kinded_block: type_checker::KindedBlock) -> Expr {
        let mut stmts = self.normalize_block_stmts(kinded_block.block.stmts);

        let mut scoped_normalizer = self.nest(&mut stmts);
        let value = scoped_normalizer.normalize_expr(kinded_block.block.value);

        block_if_necessary(kinded_block.kind, stmts, value)
    }

    fn normalize_unused_block_expr(&mut self, kinded_block: type_checker::KindedBlock) {
        let mut stmts = self.normalize_block_stmts(kinded_block.block.stmts);

        let mut scoped_normalizer = self.nest(&mut stmts);
        scoped_normalizer.normalize_unused_expr(kinded_block.block.value);
        self.stmts.extend(stmts);
    }

    fn normalize_used_invoke_expr(&mut self, invoke_expr: type_checker::InvokeExpr) -> Expr {
        self.normalize_used_call(invoke_expr.call, |call| {
            InvokeExpr {
                call,
                ret: invoke_expr.ret,
            }
            .into()
        })
    }

    fn normalize_unused_invoke_expr(&mut self, invoke_expr: type_checker::InvokeExpr) {
        self.normalize_unused_call(invoke_expr.call, |call| {
            InvokeStmt {
                call,
                ret: invoke_expr.ret,
            }
            .into()
        });
    }

    fn normalize_used_field_access_expr(
        &mut self,
        field_access_expr: type_checker::FieldAccessExpr,
    ) -> FieldAccessExpr {
        let parent = self.normalize_used_expr(field_access_expr.parent);

        FieldAccessExpr {
            parent,
            field: field_access_expr.field,
            type_: field_access_expr.type_,
        }
    }

    fn normalize_unused_field_access_expr(
        &mut self,
        field_access_expr: type_checker::FieldAccessExpr,
    ) {
        self.normalize_unused_expr(field_access_expr.parent);
    }

    fn normalize_used_negate_expr(&mut self, negate_expr: type_checker::NegateExpr) -> NegateExpr {
        let expr = self.normalize_used_expr(negate_expr.expr);

        NegateExpr {
            kind: negate_expr.kind,
            expr,
        }
    }

    fn normalize_unused_negate_expr(&mut self, negate_expr: type_checker::NegateExpr) {
        self.normalize_unused_expr(negate_expr.expr);
    }

    fn normalize_used_cast_expr(&mut self, cast_expr: type_checker::CastExpr) -> CastExpr {
        let expr = self.normalize_used_expr(cast_expr.expr);

        CastExpr {
            kind: cast_expr.kind,
            expr,
            type_: cast_expr.type_,
        }
    }

    fn normalize_unused_cast_expr(&mut self, cast_expr: type_checker::CastExpr) {
        self.normalize_unused_expr(cast_expr.expr);
    }

    fn normalize_used_bin_oper_expr(&mut self, bin_oper_expr: type_checker::BinOperExpr) -> Expr {
        let mut lhs_stmts = Vec::new();
        let mut lhs_normalizer = self.recurse(&mut lhs_stmts);
        let lhs = lhs_normalizer.normalize_pure_expr(bin_oper_expr.lhs);

        let mut rhs_stmts = Vec::new();
        let mut rhs_normalizer = self.recurse(&mut rhs_stmts);
        let rhs = rhs_normalizer.normalize_pure_expr(bin_oper_expr.rhs);

        match bin_oper_expr.oper {
            BinOper::Logical(logical_bin_oper) => match logical_bin_oper {
                // v = false; if (lhs) { v = rhs }; v
                LogicalBinOper::And => {
                    let output_var_expr = self.push_tmp_expr(BuiltInVar::False.into());

                    self.stmts.push(
                        IfStmt {
                            cond: block_if_necessary(None, lhs_stmts, lhs.into()),
                            then: vec![
                                AssignStmt {
                                    lhs: output_var_expr.var,
                                    rhs: block_if_necessary(None, rhs_stmts, rhs.into()),
                                }
                                .into(),
                            ],
                            else_: None,
                        }
                        .into(),
                    );

                    output_var_expr.into()
                }

                // v = true; if (!lhs) { v = rhs }; v
                LogicalBinOper::Or => {
                    let output_var_expr = self.push_tmp_expr(BuiltInVar::True.into());

                    self.stmts.push(
                        IfStmt {
                            cond: NegateExpr {
                                kind: NegateKind::Logical,
                                expr: block_if_necessary(None, lhs_stmts, lhs.into()),
                            }
                            .into(),
                            then: vec![
                                AssignStmt {
                                    lhs: output_var_expr.var,
                                    rhs: block_if_necessary(None, rhs_stmts, rhs.into()),
                                }
                                .into(),
                            ],
                            else_: None,
                        }
                        .into(),
                    );

                    output_var_expr.into()
                }
            },

            _ => {
                let mut stmts = lhs_stmts;
                stmts.append(&mut rhs_stmts);

                let value = BinOperExpr {
                    oper: bin_oper_expr.oper,
                    type_: bin_oper_expr.type_,
                    lhs,
                    rhs,
                }
                .into();

                block_if_necessary(None, stmts, value)
            }
        }
    }

    fn normalize_unused_bin_oper_expr(&mut self, bin_oper_expr: type_checker::BinOperExpr) {
        match bin_oper_expr.oper {
            BinOper::Logical(logical_bin_oper) => {
                let lhs = self.normalize_used_expr(bin_oper_expr.lhs);

                let mut rhs_stmts = Vec::new();
                let mut scoped_normalizer = self.recurse(&mut rhs_stmts);
                scoped_normalizer.normalize_unused_expr(bin_oper_expr.rhs);

                let cond = match logical_bin_oper {
                    // if (lhs) { rhs }
                    LogicalBinOper::And => lhs,
                    // if (!lhs) { rhs }
                    LogicalBinOper::Or => NegateExpr {
                        kind: NegateKind::Logical,
                        expr: lhs,
                    }
                    .into(),
                };

                self.stmts.push(
                    IfStmt {
                        cond,
                        then: rhs_stmts,
                        else_: None,
                    }
                    .into(),
                );
            }
            _ => {
                self.normalize_unused_expr(bin_oper_expr.lhs);
                self.normalize_unused_expr(bin_oper_expr.rhs);
            }
        };
    }

    fn normalize_unused_assign_expr(&mut self, assign_expr: type_checker::AssignExpr) {
        let rhs = self.normalize_expr(assign_expr.rhs);

        self.stmts.push(
            AssignStmt {
                lhs: assign_expr.lhs.value,
                rhs,
            }
            .into(),
        );
    }

    fn push_tmp_expr(&mut self, expr: Expr) -> VarExpr {
        let tmp_local_var_type_index = expr.type_();

        let tmp_local_var = LocalVar {
            ident: Spanned::new(Span::Internal, *TMP_VAR_IDENT),
            is_mut: false,
            type_: tmp_local_var_type_index,
        };

        let tmp_local_var_index = self.local_vars.push_and_get_key(tmp_local_var);

        self.stmts.push(
            LetStmt {
                lhs: tmp_local_var_index,
                type_: tmp_local_var_type_index,
                rhs: Some(expr),
            }
            .into(),
        );

        VarExpr {
            var: tmp_local_var_index.into(),
            type_: tmp_local_var_type_index,
        }
    }

    /// A mutable local with no initial value, to hold a result several paths assign.
    ///
    /// [`ScopedNormalizer::push_tmp_expr`]'s temp is immutable and initialized, which
    /// a result computed by branches cannot be: each branch supplies the value, and
    /// there is nothing to initialize it with beforehand -- nor any way to invent a
    /// value for an arbitrary type. An absent initializer is already what a fresh
    /// `out` argument's declaration relies on; see [`ScopedNormalizer::normalize_arg`].
    fn push_mut_tmp_expr(&mut self, type_: TypeIndex) -> VarExpr {
        let tmp_local_var_index = self.local_vars.push_and_get_key(LocalVar {
            ident: Spanned::new(Span::Internal, *TMP_VAR_IDENT),
            is_mut: true,
            type_,
        });

        self.stmts.push(
            LetStmt {
                lhs: tmp_local_var_index,
                type_,
                rhs: None,
            }
            .into(),
        );

        VarExpr {
            var: tmp_local_var_index.into(),
            type_,
        }
    }

    fn export_stmt(&mut self, stmt: Stmt) {
        self.exported_stmts
            .as_deref_mut()
            .unwrap_or(self.stmts)
            .push(stmt);
    }
}

fn block_if_necessary(kind: Option<BlockKind>, stmts: Vec<Stmt>, value: Expr) -> Expr {
    if stmts.is_empty() {
        value
    } else {
        BlockExpr { kind, stmts, value }.into()
    }
}
