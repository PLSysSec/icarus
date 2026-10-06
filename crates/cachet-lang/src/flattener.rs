// vim: set tw=99 ts=4 sts=4 sw=4 et:

mod ast;

use typed_index_collections::TiVec;

use crate::normalizer;

pub use crate::flattener::ast::*;

pub fn flatten(env: normalizer::Env) -> Env {
    let fn_items: TiVec<FnIndex, _> = env
        .fn_items
        .into_iter()
        .map(|fn_item| flatten_callable_item(fn_item))
        .collect();
    let op_items: TiVec<OpIndex, _> = env
        .op_items
        .into_iter()
        .map(|op_item| flatten_callable_item(op_item))
        .collect();
    let global_var_items: TiVec<GlobalVarIndex, _> = env
        .global_var_items
        .into_iter()
        .map(|op_item| flatten_global_var_item(op_item))
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

fn flatten_global_var_item(global_var_item: normalizer::GlobalVarItem) -> GlobalVarItem {
    let value = global_var_item
        .value
        .map(|value| value.map(flatten_const_expr));

    GlobalVarItem {
        path: global_var_item.path,
        parent: global_var_item.parent,
        is_mut: global_var_item.is_mut,
        type_: global_var_item.type_,
        value,
        attrs: global_var_item.attrs,
    }
}

fn flatten_const_expr(expr: normalizer::Expr) -> Expr {
    let mut flattener = Flattener::new();
    let expr = flattener.flatten_expr(expr);

    debug_assert!(
        flattener.stmts.len() == 0,
        "const exprs shouldn't yield stmts during flattening"
    );

    expr
}

fn flatten_callable_item(callable_item: normalizer::CallableItem) -> CallableItem {
    let body = callable_item.body.map(flatten_body);

    CallableItem {
        path: callable_item.path,
        parent: callable_item.parent,
        is_unsafe: callable_item.is_unsafe,
        params: callable_item.params,
        param_order: callable_item.param_order,
        ret: callable_item.ret,
        interprets: callable_item.interprets,
        emits: callable_item.emits,
        body,
        attrs: callable_item.attrs,
    }
}

fn flatten_body(body: normalizer::Body) -> Body {
    let stmts = flatten_block(body.stmts);

    Body {
        locals: body.locals,
        stmts,
    }
}

fn flatten_block(stmts: Vec<normalizer::Stmt>) -> Vec<Stmt> {
    let mut flattener = Flattener::new();
    flattener.flatten_stmts(stmts);
    flattener.stmts
}

struct Flattener {
    stmts: Vec<Stmt>,
}

impl Flattener {
    fn new() -> Self {
        Flattener { stmts: Vec::new() }
    }

    fn flatten_stmts(&mut self, stmts: Vec<normalizer::Stmt>) {
        self.stmts.reserve(stmts.len());
        for stmt in stmts {
            self.flatten_stmt(stmt);
        }
    }

    fn flatten_stmt(&mut self, stmt: normalizer::Stmt) {
        match stmt {
            normalizer::Stmt::Let(let_stmt) => {
                self.flatten_let_stmt(let_stmt);
            }
            normalizer::Stmt::Label(label_stmt) => {
                self.stmts.push(label_stmt.into());
            }
            normalizer::Stmt::If(if_stmt) => {
                self.flatten_if_stmt(if_stmt);
            }
            normalizer::Stmt::ForIn(for_in_stmt) => {
                self.flatten_for_in_stmt(for_in_stmt);
            }
            normalizer::Stmt::Check(check_stmt) => {
                self.flatten_check_stmt(check_stmt);
            }
            normalizer::Stmt::Goto(goto_stmt) => {
                self.stmts.push(goto_stmt.into());
            }
            normalizer::Stmt::Bind(bind_stmt) => {
                self.stmts.push(bind_stmt.into());
            }
            normalizer::Stmt::Emit(emit_stmt) => {
                self.stmts.push(emit_stmt.into());
            }
            normalizer::Stmt::Invoke(invoke_stmt) => {
                self.stmts.push(invoke_stmt.into());
            }
            normalizer::Stmt::Assign(assign_stmt) => {
                self.flatten_assign_stmt(assign_stmt);
            }
            normalizer::Stmt::Ret(ret_stmt) => {
                self.flatten_ret_stmt(ret_stmt);
            }
            normalizer::Stmt::Unreachable => {
                self.stmts.push(Stmt::Unreachable);
            }
        }
    }

    fn flatten_let_stmt(&mut self, let_stmt: normalizer::LetStmt) {
        let rhs = let_stmt.rhs.map(|rhs| self.flatten_expr(rhs));

        self.stmts.push(
            LetStmt {
                lhs: let_stmt.lhs,
                type_: let_stmt.type_,
                rhs,
            }
            .into(),
        );
    }

    fn flatten_if_stmt_recurse(&mut self, if_stmt: normalizer::IfStmt) -> IfStmt {
        let cond = self.flatten_expr(if_stmt.cond);

        let then = flatten_block(if_stmt.then);

        let else_ = if_stmt.else_.map(|else_| match else_ {
            normalizer::ElseClause::ElseIf(else_if) => {
                ast::ElseClause::ElseIf(Box::new(self.flatten_if_stmt_recurse(*else_if)))
            }
            normalizer::ElseClause::Else(else_block) => {
                ast::ElseClause::Else(flatten_block(else_block))
            }
        });

        IfStmt { cond, then, else_ }
    }

    fn flatten_if_stmt(&mut self, if_stmt: normalizer::IfStmt) {
        let if_ = self.flatten_if_stmt_recurse(if_stmt);
        self.stmts.push(if_.into());
    }

    fn flatten_for_in_stmt(&mut self, for_in_stmt: normalizer::ForInStmt) {
        let body = flatten_block(for_in_stmt.body);
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

    fn flatten_check_stmt(&mut self, check_stmt: normalizer::CheckStmt) {
        let cond = self.flatten_expr(check_stmt.cond);

        self.stmts.push(
            CheckStmt {
                kind: check_stmt.kind,
                cond,
            }
            .into(),
        );
    }

    fn flatten_assign_stmt(&mut self, assign_stmt: normalizer::AssignStmt) {
        let rhs = self.flatten_expr(assign_stmt.rhs);

        self.stmts.push(
            AssignStmt {
                lhs: assign_stmt.lhs,
                rhs,
            }
            .into(),
        );
    }

    fn flatten_ret_stmt(&mut self, ret_stmt: normalizer::RetStmt) {
        let value = ret_stmt.value.map(|value| self.flatten_expr(value));

        self.stmts.push(RetStmt { value }.into());
    }

    fn flatten_expr(&mut self, expr: normalizer::Expr) -> Expr {
        match expr {
            normalizer::Expr::Block(_, block_expr) => self.flatten_block_expr(*block_expr),
            normalizer::Expr::Literal(literal) => literal.into(),
            normalizer::Expr::Var(var_expr) => var_expr.into(),
            normalizer::Expr::Invoke(invoke_expr) => invoke_expr.into(),
            normalizer::Expr::FieldAccess(field_access_expr) => {
                self.flatten_field_access_expr(*field_access_expr).into()
            }
            normalizer::Expr::Negate(negate_expr) => self.flatten_negate_expr(*negate_expr).into(),
            normalizer::Expr::Cast(cast_expr) => self.flatten_cast_expr(*cast_expr).into(),
            normalizer::Expr::BinOper(bin_oper_expr) => bin_oper_expr.into(),
        }
    }

    fn flatten_block_expr(&mut self, block_expr: normalizer::BlockExpr) -> Expr {
        self.flatten_stmts(block_expr.stmts);

        self.flatten_expr(block_expr.value)
    }

    fn flatten_field_access_expr(
        &mut self,
        field_access_expr: normalizer::FieldAccessExpr,
    ) -> FieldAccessExpr {
        let parent = self.flatten_expr(field_access_expr.parent);

        FieldAccessExpr {
            parent,
            field: field_access_expr.field,
            type_: field_access_expr.type_,
        }
    }

    fn flatten_negate_expr(&mut self, negate_expr: normalizer::NegateExpr) -> NegateExpr {
        let expr = self.flatten_expr(negate_expr.expr);

        NegateExpr {
            kind: negate_expr.kind,
            expr,
        }
    }

    fn flatten_cast_expr(&mut self, cast_expr: normalizer::CastExpr) -> CastExpr {
        let expr = self.flatten_expr(cast_expr.expr);

        CastExpr {
            kind: cast_expr.kind,
            expr,
            type_: cast_expr.type_,
        }
    }
}
