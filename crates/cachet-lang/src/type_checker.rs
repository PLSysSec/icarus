// vim: set tw=99 ts=4 sts=4 sw=4 et:

mod ast;
mod error;
mod graphs;

use std::iter;
use std::ops::{Deref, DerefMut, Index, IndexMut};

use enumset::EnumSet;
use iterate::iterate;
use lazy_static::lazy_static;
use typed_index_collections::TiVec;

use crate::FrontendError;
use crate::ast::{
    ArithBinOper, BinOper, BlockKind, CastSafety, CheckKind, CompareBinOper, Ident, MaybeSpanned,
    NegateKind, Path, Span, Spanned, VarParamKind,
};
use crate::built_in::{BuiltInType, BuiltInVar, IdentEnum, Signedness, Width};
use crate::resolver;

pub use crate::type_checker::ast::*;
pub use crate::type_checker::error::*;
use crate::type_checker::graphs::{CallGraph, TypeGraph, VarGraph};

// * Type-Checker Entry Point

pub fn type_check(mut env: resolver::Env) -> Result<Env, TypeCheckErrors> {
    // Set up an internal placeholder type used to "turn off" type-checking for
    // variables whose types can't be inferred. This type shouldn't escape the
    // type-checking phase. We do the same for IRs as well.

    let unknown_struct_index = env.struct_items.push_and_get_key(StructItem {
        ident: Spanned::new(Span::Internal, *UNKNOWN_IDENT),
        attrs: EnumSet::empty(),
        supertype: None,
        fields: TiVec::new(),
    });

    let unknown_enum_index = env.enum_items.push_and_get_key(EnumItem {
        ident: Spanned::new(Span::Internal, *UNKNOWN_IDENT),
        attrs: EnumSet::empty(),
        variants: TiVec::new(),
    });

    let unknown_ir_index = env
        .ir_items
        .push_and_get_key(IrItem {
            ident: Spanned::new(Span::Internal, *UNKNOWN_IDENT),
            emits: None,
        })
        .into();

    let mut type_checker = TypeChecker::new(
        &env,
        unknown_struct_index,
        unknown_enum_index,
        unknown_ir_index,
    );

    // TODO(spinda): Forbid subtyping built-in types.

    let fn_items = type_checker.type_check_fn_items();
    let op_items = type_checker.type_check_op_items();
    let global_var_items = type_checker.type_check_global_var_items();

    let decl_order = type_checker.finish()?;

    // Strip out the internal "unknown" type and IR, so they doesn't escape the
    // type-checking phase.
    let popped_type_index = env
        .struct_items
        .pop_key_value()
        .map(|(struct_index, _)| struct_index);
    let popped_enum_index = env
        .enum_items
        .pop_key_value()
        .map(|(enum_index, _)| enum_index);
    let popped_ir_index = env
        .ir_items
        .pop_key_value()
        .map(|(ir_index, _)| ir_index.into());
    debug_assert_eq!(popped_type_index, Some(unknown_struct_index));
    debug_assert_eq!(popped_enum_index, Some(unknown_enum_index));
    debug_assert_eq!(popped_ir_index, Some(unknown_ir_index));

    Ok(Env {
        enum_items: env.enum_items,
        struct_items: env.struct_items,
        ir_items: env.ir_items,
        global_var_items,
        fn_items,
        op_items,
        decl_order,
    })
}

lazy_static! {
    static ref UNKNOWN_IDENT: Ident = Ident::from("<Unknown>");
}

// * Top-Level Type Checker

struct TypeChecker<'a> {
    errors: Vec<TypeCheckError>,
    env: &'a resolver::Env,
    call_graph: CallGraph,
    var_graph: VarGraph,
    unknown_struct: StructIndex,
    unknown_enum: EnumIndex,
    unknown_ir: IrIndex,
}

impl<'a> TypeChecker<'a> {
    fn new(
        env: &'a resolver::Env,
        unknown_struct_index: StructIndex,
        unknown_enum_index: EnumIndex,
        unknown_ir_index: IrIndex,
    ) -> Self {
        TypeChecker {
            errors: Vec::new(),
            env,
            call_graph: CallGraph::new(env.fn_items.len(), env.op_items.len()),
            var_graph: VarGraph::new(env.global_var_items.len()),
            unknown_struct: unknown_struct_index,
            unknown_enum: unknown_enum_index,
            unknown_ir: unknown_ir_index,
        }
    }

    fn finish(mut self) -> Result<Vec<DeclIndex>, TypeCheckErrors> {
        if !self.errors.is_empty() {
            self.errors.sort_by_key(|error| error.span());
            return Err(TypeCheckErrors(self.errors));
        }

        let unknown_type_index = self.unknown_type();
        let type_decl_order = self
            .flatten_type_graph()
            .into_iter()
            .filter_map(|type_index| {
                if type_index == unknown_type_index {
                    None
                } else {
                    DeclIndex::try_from(type_index).ok()
                }
            });

        let global_var_decl_order = self
            .flatten_global_var_graph()
            .into_iter()
            .map(DeclIndex::from);

        let callable_decl_order = self.flatten_call_graph().into_iter().map(DeclIndex::from);

        let decl_order = iterate![
            ..type_decl_order,
            ..global_var_decl_order,
            ..callable_decl_order,
        ]
        .collect();

        Ok(decl_order)
    }

    fn flatten_type_graph(&mut self) -> Vec<TypeIndex> {
        let type_graph = TypeGraph::new(&self.env.enum_items, &self.env.struct_items);
        let type_sccs = type_graph.sccs();

        for cycle_type_indexes in type_sccs.iter_cycles() {
            let mut cycle_types: Vec<_> = cycle_type_indexes
                .map(|cycle_type_index| match cycle_type_index {
                    TypeIndex::BuiltIn(_) => {
                        unreachable!("built-in types can't participate in cycles")
                    }
                    TypeIndex::Enum(enum_index) => self.env[enum_index].ident,
                    TypeIndex::Struct(struct_index) => self.env[struct_index].ident,
                })
                .collect();

            let first_cycle_type = cycle_types.remove(0);
            self.errors.push(TypeCheckError::SubtypeCycle {
                first_cycle_type,
                other_cycle_types: cycle_types,
            });
        }

        type_sccs.iter_post_order().collect()
    }

    fn flatten_call_graph(&mut self) -> Vec<CallableIndex> {
        let callable_sccs = self.call_graph.sccs();

        for cycle_callable_indexes in callable_sccs.iter_cycles() {
            let mut cycle_callables: Vec<_> = cycle_callable_indexes
                .map(|cycle_callable_index| {
                    cycle_callable_index
                        .map(|cycle_callable_index| self.env[cycle_callable_index].path.value)
                })
                .collect();

            // Rotate cycle_callables so that the first element corresponds to
            // the call that appears first in the source code.
            let first_cycle_callable_index = cycle_callables
                .iter()
                .enumerate()
                .min_by_key(|(_, cycle_callable)| cycle_callable.span)
                .map(|(cycle_callable_index, _)| cycle_callable_index)
                .unwrap();
            cycle_callables.rotate_left(first_cycle_callable_index);

            let first_cycle_callable = cycle_callables.remove(0);
            self.errors.push(TypeCheckError::CallCycle {
                first_cycle_callable,
                other_cycle_callables: cycle_callables,
            });
        }

        callable_sccs.iter_post_order().collect()
    }

    fn flatten_global_var_graph(&mut self) -> Vec<GlobalVarIndex> {
        let var_sccs = self.var_graph.sccs();

        for cycle_var_indexes in var_sccs.iter_cycles() {
            let mut cycle_vars: Vec<_> = cycle_var_indexes
                .map(|cycle_var_index| {
                    cycle_var_index.map(|cycle_var_index| self.env[cycle_var_index].path.value)
                })
                .collect();

            // Rotate cycle_callables so that the first element corresponds to
            // the call that appears first in the source code.
            let first_cycle_var_index = cycle_vars
                .iter()
                .enumerate()
                .min_by_key(|(_, cycle_callable)| cycle_callable.span)
                .map(|(cycle_var_index, _)| cycle_var_index)
                .unwrap();
            cycle_vars.rotate_left(first_cycle_var_index);

            let first_cycle_callable = cycle_vars.remove(0);
            self.errors.push(TypeCheckError::CallCycle {
                first_cycle_callable,
                other_cycle_callables: cycle_vars,
            });
        }

        var_sccs.iter_post_order().collect()
    }

    fn type_check_fn_items(&mut self) -> TiVec<FnIndex, CallableItem> {
        self.type_check_callable_items(self.env.fn_items.keys())
    }

    fn type_check_op_items(&mut self) -> TiVec<OpIndex, CallableItem> {
        self.type_check_callable_items(self.env.op_items.keys())
    }

    fn type_check_callable_items<I: Into<CallableIndex>>(
        &mut self,
        callable_indexes: impl Iterator<Item = I>,
    ) -> TiVec<I, CallableItem> {
        callable_indexes
            .map(|callable_index| self.type_check_callable_item(callable_index.into()))
            .collect()
    }

    fn type_check_callable_item(&mut self, callable_index: CallableIndex) -> CallableItem {
        let callable_item = &self.env[callable_index];

        let ret = callable_item.type_();

        let (interprets, emits) = match (callable_index, callable_item.parent, callable_item.emits)
        {
            (CallableIndex::Op(_), Some(ParentIndex::Ir(ir_index)), explicit_emits) => {
                if explicit_emits.is_some() {
                    self.errors.push(TypeCheckError::OpHasExplicitEmits {
                        op: callable_item.path,
                    });
                }

                let ir_item = &self.env[ir_index];
                let emits = ir_item.emits;
                let interprets = match emits {
                    Some(_) => None,
                    None => Some(Spanned::new(ir_item.ident.span, ir_index)),
                };
                (interprets, emits)
            }
            (CallableIndex::Fn(_), _, Some(ir_index)) => {
                if callable_item.body.value.is_none() {
                    self.errors.push(TypeCheckError::MissingEmitsFnBody {
                        fn_: callable_item.path,
                    });
                }

                (None, Some(ir_index))
            }
            _ => (None, None),
        };

        let body = callable_item.body.value.as_ref().map(|body| {
            let mut locals = body.locals.clone();
            let mut scoped_type_checker = ScopedTypeChecker::for_callable(
                self,
                callable_index,
                interprets,
                emits,
                &mut locals,
            );

            let body_block_has_explicit_value = body.block.value.value.is_some();
            let mut block = scoped_type_checker.type_check_block(&body.block);
            // If the callable's body block doesn't exit early, we need to check
            // that the type of its value matches the return type of the
            // callable, possibly inserting casts. Even if the callable body
            // *does* exit early, we still do this check if the user explicitly
            // wrote out a value for the body block.
            if !block.exits_early || body_block_has_explicit_value {
                block.value = scoped_type_checker
                    .expect_expr_type(Spanned::new(callable_item.body.span, block.value), ret);
            }

            let locals = Locals {
                local_vars: locals
                    .local_vars
                    .into_iter()
                    .map(|local_var| LocalVar {
                        ident: local_var.ident,
                        is_mut: local_var.is_mut,
                        type_: local_var.type_.expect(
                            "the types of all local variables should be inferred by this point",
                        ),
                    })
                    .collect(),
                local_labels: locals
                    .local_labels
                    .into_iter()
                    .map(|local_label| Label {
                        ident: local_label.ident,
                        ir: local_label.ir.expect(
                            "the IRs of all local labels should be inferred by this point",
                        ),
                    })
                    .collect(),
            };

            Body { locals, block }
        });

        CallableItem {
            path: callable_item.path.into(),
            parent: callable_item.parent,
            attrs: callable_item.attrs,
            is_unsafe: callable_item.is_unsafe,
            params: callable_item.params.clone(),
            param_order: callable_item.param_order.clone(),
            ret,
            interprets: interprets.map(|interprets| interprets.value),
            emits: emits.map(|emits| emits.value),
            body,
        }
    }

    fn type_check_global_var_items(&mut self) -> TiVec<GlobalVarIndex, GlobalVarItem> {
        self.env
            .global_var_items
            .keys()
            .map(|gvidx| self.type_check_global_var_item(gvidx))
            .collect()
    }

    fn type_check_global_var_item(&mut self, global_var_index: GlobalVarIndex) -> GlobalVarItem {
        let global_var_item = &self.env[global_var_index];

        let value = global_var_item.value.as_ref().map(|value| {
            if global_var_item.is_mut {
                self.errors.push(TypeCheckError::MutGlobalVarWithValue {
                    var: global_var_item.path,
                });
            }

            let mut checker = ScopedTypeChecker::for_global_var(self, global_var_index);

            let value = Spanned::new(
                value.span,
                checker.type_check_expr_expecting_type(value.as_ref(), global_var_item.type_()),
            );

            if !is_const(&value.value) {
                self.errors
                    .push(TypeCheckError::NonConstGlobalVarItem { expr: value.span });
            }

            value
        });

        GlobalVarItem {
            path: global_var_item.path,
            parent: global_var_item.parent,
            is_mut: global_var_item.is_mut,
            type_: global_var_item.type_(),
            attrs: global_var_item.attrs,
            value,
        }
    }

    fn expect_expr_type(&mut self, expr: Spanned<Expr>, type_index: TypeIndex) -> Expr {
        let expr_span = expr.span;

        match self.try_build_cast_chain(CastSafety::Lossless, expr.value, type_index) {
            Ok(expr) => expr,
            Err(expr) => {
                self.errors.push(TypeCheckError::TypeMismatch {
                    expected_type: self.get_type_ident(type_index),
                    found_type: self.get_type_ident(expr.type_()),
                    span: expr_span,
                });
                expr
            }
        }
    }

    fn try_build_cast_chain(
        &self,
        min_safety: CastSafety,
        expr: Expr,
        type_: TypeIndex,
    ) -> Result<Expr, Expr> {
        if expr.type_() == type_ {
            return Ok(expr);
        }

        match (expr.type_(), type_) {
            (TypeIndex::BuiltIn(src_built_in), TypeIndex::BuiltIn(target_built_in)) => {
                let safety = src_built_in.casts_to(target_built_in);
                if safety >= min_safety {
                    Ok(build_cast_chain(safety, expr, iter::once(type_)))
                } else {
                    Err(expr)
                }
            }
            _ => match self.find_upcast_route(type_, expr.type_()) {
                None => Err(expr),
                Some(route) => Ok(build_cast_chain(
                    CastSafety::Lossless,
                    expr,
                    route.into_iter(),
                )),
            },
        }
    }

    fn find_upcast_route(
        &self,
        supertype_index: TypeIndex,
        subtype_index: TypeIndex,
    ) -> Option<Vec<TypeIndex>> {
        if self.is_same_type(supertype_index, subtype_index) {
            return Some(Vec::new());
        }

        let mut upcast_route = Vec::new();
        let mut curr_type_index = subtype_index;
        loop {
            curr_type_index = match curr_type_index {
                TypeIndex::BuiltIn(_) | TypeIndex::Enum(_) => return None,
                TypeIndex::Struct(struct_index) => self.env[struct_index].supertype?,
            };
            debug_assert_ne!(curr_type_index, self.unknown_type());

            if curr_type_index == subtype_index {
                // We went all the way 'round a cycle. Though type cycles are
                // forbidden in Cachet, they can still exist during
                // type-checking. Break out here to prevent infinite loops.
                return None;
            }

            upcast_route.push(curr_type_index);

            if curr_type_index == supertype_index {
                return Some(upcast_route);
            }
        }
    }

    const fn unknown_type(&self) -> TypeIndex {
        TypeIndex::Struct(self.unknown_struct)
    }

    fn is_unknown_type(&self, type_index: TypeIndex) -> bool {
        type_index == self.unknown_type()
    }

    fn is_same_type(&self, lhs: TypeIndex, rhs: TypeIndex) -> bool {
        // The internal "unknown" type unifies with everything. No values of
        // this type will ever actually be produced.
        lhs == rhs || self.is_unknown_type(lhs) || self.is_unknown_type(rhs)
    }

    // Consider the internal "unknown" type to be numeric, signed, and integral for
    // the purposes of the type-checking phase.

    fn is_numeric_type(&self, type_index: TypeIndex) -> bool {
        type_index.is_numeric() || self.is_unknown_type(type_index)
    }

    fn is_signed_numeric_type(&self, type_index: TypeIndex) -> bool {
        type_index.is_signed_numeric() || self.is_unknown_type(type_index)
    }

    fn is_integral_type(&self, type_index: TypeIndex) -> bool {
        type_index.is_integral() || self.is_unknown_type(type_index)
    }

    fn get_type_ident(&self, type_index: TypeIndex) -> Ident {
        match type_index {
            TypeIndex::BuiltIn(built_in_type) => built_in_type.ident(),
            TypeIndex::Enum(enum_index) => self.env[enum_index].ident.value,
            TypeIndex::Struct(struct_index) => self.env[struct_index].ident.value,
        }
    }

    fn is_same_ir(&self, found_ir_index: IrIndex, expected_ir_index: IrIndex) -> bool {
        return found_ir_index == expected_ir_index
            || found_ir_index == self.unknown_ir
            || expected_ir_index == self.unknown_ir;
    }
}

fn build_cast_chain(
    kind: CastSafety,
    expr: Expr,
    cast_route: impl Iterator<Item = TypeIndex>,
) -> Expr {
    cast_route.fold(expr, |expr, type_index| {
        // If we're casting a literal, let's try to produce a new literal rather
        // than wrap the literal in a cast expression. It's important for
        // correctness that this conversion act the same as the C++ and Boogie
        // backends.
        if let (Expr::Literal(l), TypeIndex::BuiltIn(built_in)) = (&expr, type_index) {
            if let Some(literal) = cast_literal(l, built_in) {
                return literal.into();
            }
        }

        CastExpr {
            kind,
            expr,
            type_: type_index,
        }
        .into()
    })
}

#[derive(Debug)]
enum ItemContext<'b> {
    Callable {
        index: CallableIndex,
        item: &'b resolver::CallableItem,
        locals: &'b mut resolver::Locals,
    },
    Var {
        index: GlobalVarIndex,
    },
}

impl<'b> ItemContext<'b> {
    fn param<I>(&self, index: I) -> &<resolver::Params as Index<I>>::Output
    where
        resolver::Params: Index<I>,
    {
        match self {
            ItemContext::Callable { item, .. } => &item.params[index],
            ItemContext::Var { .. } => {
                panic!("attempted to look up param in non-callable context");
            }
        }
    }

    fn local<I>(&self, index: I) -> &<resolver::Locals as Index<I>>::Output
    where
        resolver::Locals: Index<I>,
    {
        match self {
            ItemContext::Callable { locals, .. } => &locals[index],
            ItemContext::Var { .. } => {
                panic!("attempted to look up param in non-callable context");
            }
        }
    }

    fn local_mut<I>(&mut self, index: I) -> &mut <resolver::Locals as Index<I>>::Output
    where
        resolver::Locals: IndexMut<I>,
    {
        match self {
            ItemContext::Callable { locals, .. } => &mut locals[index],
            ItemContext::Var { .. } => {
                panic!("attempted to look up param in non-callable context");
            }
        }
    }
}

// Scoped Type Checker
struct ScopedTypeChecker<'a, 'b> {
    type_checker: &'b mut TypeChecker<'a>,
    context: ItemContext<'b>,
    is_unsafe: bool,
    interprets: Option<Spanned<IrIndex>>,
    emits: Option<Spanned<IrIndex>>,
}

impl<'a, 'b> Deref for ScopedTypeChecker<'a, 'b> {
    type Target = TypeChecker<'a>;

    fn deref(&self) -> &Self::Target {
        self.type_checker
    }
}

impl<'a, 'b> DerefMut for ScopedTypeChecker<'a, 'b> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.type_checker
    }
}

impl<'a, 'b> ScopedTypeChecker<'a, 'b> {
    fn for_global_var(type_checker: &'b mut TypeChecker<'a>, index: GlobalVarIndex) -> Self {
        ScopedTypeChecker {
            type_checker,
            context: ItemContext::Var { index },
            is_unsafe: false,
            interprets: None,
            emits: None,
        }
    }
    fn for_callable(
        type_checker: &'b mut TypeChecker<'a>,
        index: CallableIndex,
        interprets: Option<Spanned<IrIndex>>,
        emits: Option<Spanned<IrIndex>>,
        locals: &'b mut resolver::Locals,
    ) -> Self {
        let callable = &type_checker.env[index];
        let context = ItemContext::Callable {
            index,
            item: callable,
            locals,
        };

        let is_unsafe = callable.is_unsafe;

        ScopedTypeChecker {
            type_checker,
            context,
            is_unsafe,
            interprets,
            emits,
        }
    }

    fn recurse<'c>(&'c mut self, is_unsafe: bool) -> ScopedTypeChecker<'a, 'c> {
        ScopedTypeChecker {
            type_checker: self.type_checker,
            context: match &mut self.context {
                ItemContext::Callable {
                    index,
                    item,
                    locals,
                } => ItemContext::Callable {
                    index: *index,
                    item,
                    locals,
                },
                ItemContext::Var { index } => ItemContext::Var { index: *index },
            },
            is_unsafe: self.is_unsafe || is_unsafe,
            interprets: self.interprets,
            emits: self.emits,
        }
    }

    fn type_check_arg(
        &mut self,
        target: CallableIndex,
        param_index: Option<resolver::ParamIndex>,
        arg: Spanned<&resolver::Arg>,
    ) -> Arg {
        let callable_item = &self.env[target];

        let param_summary = param_index.map(|param_index| match param_index {
            resolver::ParamIndex::Var(var_param_index) => {
                let var_param = &callable_item.params[var_param_index];
                ParamSummary {
                    ident: var_param.ident,
                    type_: Some(var_param.type_),
                    ir: None,
                    expected_arg_kind: match var_param.kind {
                        VarParamKind::In | VarParamKind::Mut => ArgKind::Expr,
                        VarParamKind::Out => ArgKind::OutVar,
                    },
                }
            }
            resolver::ParamIndex::Label(label_param_index) => {
                let label_param = &callable_item.params[label_param_index];
                ParamSummary {
                    ident: label_param.label.ident,
                    type_: None,
                    ir: Some(label_param.label.ir),
                    expected_arg_kind: if label_param.is_out {
                        ArgKind::OutLabel
                    } else {
                        ArgKind::Label
                    },
                }
            }
        });

        let arg_span = arg.span;
        let (found_arg_kinds, arg) = match &arg.value {
            resolver::Arg::Expr(expr) => (
                ArgKind::Expr.into(),
                self.type_check_expr_arg(
                    callable_item.path.value,
                    param_summary.as_ref(),
                    Spanned::new(arg_span, expr),
                )
                .into(),
            ),
            resolver::Arg::FieldAccess(field_access) => self.type_check_field_access_arg(
                callable_item.path.value,
                param_summary.as_ref(),
                Spanned::new(arg_span, field_access),
            ),
            resolver::Arg::OutVar(out_var) => (
                ArgKind::OutVar.into(),
                self.type_check_out_var_arg(
                    callable_item.path.value,
                    param_summary.as_ref(),
                    Spanned::new(arg_span, *out_var),
                )
                .into(),
            ),
            resolver::Arg::Label(label_index) => (
                ArgKind::Label.into(),
                self.type_check_label_arg(
                    callable_item.path.value,
                    param_summary.as_ref(),
                    Spanned::new(arg_span, *label_index),
                )
                .into(),
            ),
            resolver::Arg::OutLabel(out_label) => (
                ArgKind::OutLabel.into(),
                self.type_check_out_label_arg(
                    callable_item.path.value,
                    param_summary.as_ref(),
                    Spanned::new(arg_span, *out_label),
                )
                .into(),
            ),
        };

        if let Some(ParamSummary {
            ident: param_ident,
            expected_arg_kind,
            ..
        }) = param_summary
        {
            if !found_arg_kinds.contains(expected_arg_kind) {
                self.errors.push(TypeCheckError::ArgKindMismatch {
                    expected_arg_kind,
                    found_arg_kinds,
                    arg_span,
                    target: callable_item.path.value,
                    param: param_ident,
                });
            }
        }

        arg
    }

    fn type_check_expr_arg(
        &mut self,
        target: Path,
        param_summary: Option<&ParamSummary>,
        arg: Spanned<&resolver::Expr>,
    ) -> Expr {
        let arg = arg.map(|expr| self.type_check_expr(expr));
        self.ensure_expr_arg_matches_param_type(target, param_summary, arg)
    }

    fn type_check_field_access_arg(
        &mut self,
        target: Path,
        param_summary: Option<&ParamSummary>,
        arg: Spanned<&resolver::FieldAccess>,
    ) -> (EnumSet<ArgKind>, Arg) {
        let (parent, struct_field_index, field) = self.type_check_field_access(&arg.value);
        let Some(field) = field else {
            return (
                ArgKind::Expr | ArgKind::Label,
                Expr::from(FieldAccessExpr {
                    parent,
                    type_: self.unknown_type(),
                    field: struct_field_index,
                })
                .into(),
            );
        };

        match field {
            resolver::Field::Var(resolver::VarField { type_, .. }) => {
                let field_access_expr = FieldAccessExpr {
                    parent,
                    type_: *type_,
                    field: struct_field_index,
                };
                let expr = self.ensure_expr_arg_matches_param_type(
                    target,
                    param_summary,
                    Spanned::new(arg.span, field_access_expr.into()),
                );
                (ArgKind::Expr.into(), expr.into())
            }
            resolver::Field::Label(resolver::Label {
                ir: arg_ir_index, ..
            }) => {
                self.ensure_label_arg_matches_param_ir(
                    target,
                    param_summary,
                    arg.span,
                    *arg_ir_index,
                );
                let label_field_arg = LabelFieldArg {
                    parent,
                    field: struct_field_index,
                    ir: *arg_ir_index,
                };
                (ArgKind::Label.into(), label_field_arg.into())
            }
        }
    }

    fn type_check_field_access(
        &mut self,
        field_access: &resolver::FieldAccess,
    ) -> (Expr, StructFieldIndex, Option<&'a resolver::Field>) {
        let mut parent_expr = self.type_check_expr(&field_access.parent.value);

        let orig_parent_type_index = parent_expr.type_();
        let mut parent_struct_index = match orig_parent_type_index {
            TypeIndex::Struct(parent_struct_index) => parent_struct_index,
            _ => {
                self.type_checker
                    .errors
                    .push(TypeCheckError::NonStructFieldAccess {
                        field: field_access.field,
                        parent_span: field_access.parent.span,
                        parent_type: self.get_type_ident(orig_parent_type_index),
                    });
                self.unknown_struct
            }
        };

        // This field index is potentially invalid if resolution fails. However,
        // it shouldn't be used inside the type-checker, and shouldn't escape,
        // since the failure will raise an error.
        let mut field_index = FieldIndex::from(0);
        let mut field = None;

        // Note that, in addition to the non-struct-type case above, an
        // unknown-struct here could also result from a parent expression whose
        // type couldn't be resolved.
        if parent_struct_index != self.unknown_struct {
            let mut parent_upcast_route = Vec::new();
            let mut curr_parent_struct_index = parent_struct_index;

            loop {
                let curr_parent_struct_item = &self.env[curr_parent_struct_index];
                match curr_parent_struct_item
                    .fields
                    .iter_enumerated()
                    .find(|(_, field)| field.ident().value == field_access.field.value)
                {
                    Some((found_field_index, found_field)) => {
                        field_index = found_field_index;
                        field = Some(found_field);

                        parent_expr = build_cast_chain(
                            CastSafety::Lossless,
                            parent_expr,
                            parent_upcast_route.into_iter(),
                        );
                        parent_struct_index = curr_parent_struct_index;
                    }
                    None => {
                        if let Some(next_parent_type_index) = curr_parent_struct_item.supertype {
                            // Supertype cycles are forbidden in Cachet, but
                            // they're still technically possible in the AST at
                            // this stage, so we need to guard against that here
                            // to avoid a potential infinite loop.
                            if next_parent_type_index != orig_parent_type_index {
                                if let TypeIndex::Struct(next_parent_struct_index) =
                                    next_parent_type_index
                                {
                                    debug_assert_ne!(
                                        next_parent_struct_index,
                                        self.unknown_struct
                                    );
                                    parent_upcast_route.push(next_parent_type_index);
                                    curr_parent_struct_index = next_parent_struct_index;
                                    continue;
                                }
                            }
                        }

                        self.type_checker
                            .errors
                            .push(TypeCheckError::FieldNotFound {
                                field: field_access.field,
                                parent_type: self.env[parent_struct_index].ident,
                            });
                    }
                }
                break;
            }
        }

        let struct_field_index = StructFieldIndex {
            struct_index: parent_struct_index,
            field_index,
        };
        (parent_expr, struct_field_index, field)
    }

    fn type_check_out_var_arg(
        &mut self,
        target: Path,
        param_summary: Option<&ParamSummary>,
        arg: Spanned<OutVar>,
    ) -> OutVarArg {
        // Assign the internal "unknown" type if we don't have a parameter type
        // to cross-reference against. It will unify with whatever the argument
        // has for its type. An error should be reported elsewhere if this
        // happens (e.g., too many arguments or wrong argument kind), preventing
        // the internal "unknown" type from escaping the type-checking phase.
        let param_type_index = param_summary
            .and_then(|param_summary| param_summary.type_)
            .unwrap_or(self.unknown_type());

        let arg_type_index = match arg.value {
            OutVar::Free(var_index) => self.get_var_type(var_index),
            // If this is an `out let foo`-style argument with no type
            // ascription, infer the type from the parameter.
            OutVar::Fresh(local_var_index) => *self
                .context
                .local_mut(local_var_index)
                .type_
                .get_or_insert(param_type_index),
        };

        // Upcasting may be needed to convert from the type the out-parameter
        // yields to the type of the variable being written.
        let upcast_route = match self.find_upcast_route(arg_type_index, param_type_index) {
            Some(upcast_route) => upcast_route,
            None => {
                self.type_checker
                    .errors
                    .push(TypeCheckError::ArgTypeMismatch {
                        expected_type: self.get_type_ident(param_type_index),
                        found_type: self.get_type_ident(arg_type_index),
                        arg_span: arg.span,
                        target,
                        param: param_summary.unwrap().ident,
                    });
                Vec::new()
            }
        };

        OutVarArg {
            out_var: arg.value,
            type_: param_type_index,
            upcast_route,
        }
    }

    fn type_check_label_arg(
        &mut self,
        target: Path,
        param_summary: Option<&ParamSummary>,
        arg: Spanned<LabelIndex>,
    ) -> LabelArg {
        let arg_ir_index = self.get_label_ir(arg);
        self.ensure_label_arg_matches_param_ir(target, param_summary, arg.span, arg_ir_index);

        LabelArg {
            label: arg,
            ir: arg_ir_index,
        }
    }

    fn type_check_out_label_arg(
        &mut self,
        target: Path,
        param_summary: Option<&ParamSummary>,
        arg: Spanned<OutLabel>,
    ) -> OutLabelArg {
        let arg_label_index = match arg.value {
            OutLabel::Free(label_index) => label_index.value,
            OutLabel::Fresh(local_label_index) => {
                // Infer the label IR from the parameter if one isn't specified
                // explicitly.
                let unknown_ir_index = self.unknown_ir;
                let local_label_ir = &mut self.context.local_mut(local_label_index).ir;
                if local_label_ir.is_none() {
                    // As with variable out-parameters, we assign the internal "unknown" IR
                    // if we don't have a corresponding parameter IR.
                    let param_ir_index = param_summary
                        .and_then(|param_summary| param_summary.ir)
                        .unwrap_or(unknown_ir_index);
                    *local_label_ir = Some(param_ir_index);
                }

                local_label_index.into()
            }
        };

        let arg_ir_index = self.get_label_ir(Spanned::new(arg.span, arg_label_index));
        self.ensure_label_arg_matches_param_ir(target, param_summary, arg.span, arg_ir_index);

        OutLabelArg {
            out_label: arg.value,
            ir: arg_ir_index,
        }
    }

    fn ensure_expr_arg_matches_param_type(
        &mut self,
        target: Path,
        param_summary: Option<&ParamSummary>,
        arg: Spanned<Expr>,
    ) -> Expr {
        match param_summary {
            Some(&ParamSummary {
                ident: param_ident,
                type_: Some(param_type_index),
                ir: None,
                ..
            }) => {
                match self.try_build_cast_chain(CastSafety::Lossless, arg.value, param_type_index)
                {
                    Ok(expr) => expr,
                    Err(expr) => {
                        self.type_checker
                            .errors
                            .push(TypeCheckError::ArgTypeMismatch {
                                expected_type: self.get_type_ident(param_type_index),
                                found_type: self.get_type_ident(expr.type_()),
                                arg_span: arg.span,
                                target,
                                param: param_ident,
                            });
                        expr
                    }
                }
            }
            _ => arg.value,
        }
    }

    fn ensure_label_arg_matches_param_ir(
        &mut self,
        target: Path,
        param_summary: Option<&ParamSummary>,
        arg_span: Span,
        arg_ir_index: IrIndex,
    ) {
        if let Some(&ParamSummary {
            ident: param_ident,
            ir: Some(param_ir_index),
            ..
        }) = param_summary
        {
            if !self.is_same_ir(arg_ir_index, param_ir_index) {
                self.type_checker
                    .errors
                    .push(TypeCheckError::ArgIrMismatch {
                        expected_ir: self.env[param_ir_index].ident.value,
                        found_ir: self.env[arg_ir_index].ident.value,
                        arg_span,
                        target,
                        param: param_ident,
                    });
            }
        }
    }

    fn type_check_call(&mut self, call: &resolver::Call) -> Call {
        if let ItemContext::Callable { index, .. } = self.context {
            self.type_checker.call_graph.record_call(index, call.target);
        }
        let callable_item = &self.env[call.target.value];

        if callable_item.is_unsafe && !self.is_unsafe {
            self.type_checker
                .errors
                .push(TypeCheckError::UnsafeCallInSafeContext {
                    target: Spanned::new(call.target.span, callable_item.path.value),
                    target_defined_at: callable_item.path.span,
                });
        }

        let expected_arg_count = callable_item.param_order.len();
        let found_arg_count = call.args.value.len();
        if found_arg_count != expected_arg_count {
            self.type_checker
                .errors
                .push(TypeCheckError::ArgCountMismatch {
                    expected_arg_count,
                    found_arg_count,
                    target: Spanned::new(call.target.span, callable_item.path.value),
                    target_defined_at: callable_item.path.span,
                    args_span: call.args.span,
                });
        }

        // Extend the parameter order past the end so that excess arguments
        // still get type-checked.
        let param_order = callable_item
            .param_order
            .iter()
            .copied()
            .map(Some)
            .chain(iter::repeat(None));

        let args: Vec<_> = param_order
            .zip(call.args.value.iter())
            .map(|(param_index, arg)| {
                self.type_check_arg(call.target.value, param_index, arg.as_ref())
            })
            .collect();

        Call {
            target: call.target.value,
            is_unsafe: callable_item.is_unsafe,
            args,
        }
    }

    fn type_check_block(&mut self, block: &resolver::Block) -> Block {
        let stmts = Vec::from_iter(
            block
                .stmts
                .iter()
                .filter_map(|stmt| self.type_check_stmt(stmt.as_ref())),
        );

        let value = match &block.value.value {
            None => BuiltInVar::Unit.into(),
            Some(value) => self.type_check_expr(value),
        };

        let exits_early = stmts.iter().any(does_stmt_exit_early) || does_expr_exit_early(&value);

        Block {
            stmts,
            value,
            exits_early,
        }
    }

    fn type_check_kinded_block(&mut self, kinded_block: &resolver::KindedBlock) -> KindedBlock {
        let block = match kinded_block.kind {
            None => self.type_check_block(&kinded_block.block),
            Some(kind) => self
                .recurse(kind == BlockKind::Unsafe)
                .type_check_block(&kinded_block.block),
        };

        KindedBlock {
            kind: kinded_block.kind,
            block,
        }
    }

    fn type_check_stmt(&mut self, stmt: Spanned<&resolver::Stmt>) -> Option<Stmt> {
        let stmt_span = stmt.span;

        let stmt: Option<Stmt> = match stmt.value {
            resolver::Stmt::Let(let_stmt) => Some(self.type_check_let_stmt(let_stmt).into()),
            resolver::Stmt::Label(LabelStmt { label }) => Some(LabelStmt { label: *label }.into()),
            resolver::Stmt::If(if_stmt) => Some(self.type_check_if_stmt(if_stmt).into()),
            resolver::Stmt::ForIn(for_in_stmt) => {
                Some(self.type_check_for_in_stmt(for_in_stmt).into())
            }
            resolver::Stmt::Check(check_stmt) => {
                Some(self.type_check_check_stmt(check_stmt).into())
            }
            resolver::Stmt::Goto(goto_stmt) => Some(self.type_check_goto_stmt(goto_stmt).into()),
            resolver::Stmt::Bind(bind_stmt) => Some(self.type_check_bind_stmt(bind_stmt).into()),
            resolver::Stmt::Emit(call) => self.type_check_emit_stmt(call).map(Into::into),
            resolver::Stmt::Ret(ret_stmt) => Some(self.type_check_ret_stmt(ret_stmt).into()),
            resolver::Stmt::Expr(expr) => Some(self.type_check_expr(expr).into()),
            resolver::Stmt::Block(kinded_block) => {
                Some(self.type_check_block_stmt(kinded_block).into())
            }
        };

        if let Some(stmt) = &stmt {
            let stmt_type = stmt.type_();
            if !self.is_same_type(stmt_type, BuiltInType::Unit.into()) {
                let stmt_type_ident = self.get_type_ident(stmt_type);
                self.errors.push(TypeCheckError::TypeMismatch {
                    expected_type: BuiltInVar::Unit.ident(),
                    found_type: stmt_type_ident,
                    span: stmt_span,
                });
            }
        }

        stmt
    }

    fn type_check_block_stmt(&mut self, kinded_block: &resolver::KindedBlock) -> Expr {
        let typed_kinded_block = self.type_check_kinded_block(kinded_block);
        let type_ = typed_kinded_block.type_();
        if type_ != BuiltInType::Unit.into() {
            let expected_type = self.get_type_ident(BuiltInType::Unit.into());
            let found_type = self.get_type_ident(type_);
            self.errors.push(TypeCheckError::TypeMismatch {
                expected_type,
                found_type,
                span: kinded_block.block.value.span,
            });
        }

        typed_kinded_block.into()
    }

    fn type_check_let_stmt(&mut self, let_stmt: &resolver::LetStmt) -> LetStmt {
        let rhs = let_stmt.rhs.as_ref().map(|rhs| self.type_check_expr(rhs));

        let local_var = &mut self.context.local_mut(let_stmt.lhs);

        let rhs = match local_var.type_ {
            Some(lhs_type_index) => self.expect_expr_type(rhs, lhs_type_index),
            None => {
                local_var.type_ = Some(rhs.value.type_());
                rhs.value
            }
        };

        LetStmt {
            lhs: let_stmt.lhs,
            rhs,
        }
    }

    fn type_check_if_stmt(&mut self, if_stmt: &resolver::IfStmt) -> IfStmt {
        let cond =
            self.type_check_expr_expecting_type(if_stmt.cond.as_ref(), BuiltInType::Bool.into());

        // TODO(spinda): Check that all branches have the same type.

        let then = self.type_check_block(&if_stmt.then);

        let else_ = if_stmt.else_.as_ref().map(|else_| match else_ {
            resolver::ElseClause::ElseIf(else_if) => {
                ast::ElseClause::ElseIf(Box::new(self.type_check_if_stmt(&*else_if)))
            }
            resolver::ElseClause::Else(else_block) => {
                ast::ElseClause::Else(self.type_check_block(else_block))
            }
        });

        IfStmt { cond, then, else_ }
    }

    fn type_check_for_in_stmt(&mut self, for_in_stmt: &resolver::ForInStmt) -> ForInStmt {
        let enum_index = match for_in_stmt.target {
            TypeIndex::BuiltIn(_) | TypeIndex::Struct(_) => {
                //TODO: push an error
                self.unknown_enum
            }
            TypeIndex::Enum(enum_index) => enum_index,
        };

        let body = self.type_check_block(&for_in_stmt.body);

        ForInStmt {
            var: for_in_stmt.var,
            target: enum_index,
            order: for_in_stmt.order,
            body,
        }
    }

    fn type_check_check_stmt(&mut self, check_stmt: &resolver::CheckStmt) -> CheckStmt {
        let cond = self
            .type_check_expr_expecting_type(check_stmt.cond.as_ref(), BuiltInType::Bool.into());

        CheckStmt {
            kind: check_stmt.kind,
            cond,
        }
    }

    fn type_check_ret_stmt(&mut self, ret_stmt: &resolver::RetStmt) -> RetStmt {
        // Treat `return;` as `return unit;`. We do a little finagling here to
        // ensure the span for the value expression always points to a sensible
        // location.

        let default = Spanned::new(ret_stmt.value.span, VarIndex::from(BuiltInVar::Unit)).into();
        let value_or_default = ret_stmt
            .value
            .as_ref()
            .map(|value| value.as_ref().unwrap_or(&default));

        let value = match self.context {
            ItemContext::Callable { item, .. } => {
                self.type_check_expr_expecting_type(value_or_default, item.type_())
            }

            ItemContext::Var { .. } => {
                self.errors.push(TypeCheckError::ReturnOutsideCallable {
                    span: value_or_default.span,
                });
                self.type_check_expr(&value_or_default.value)
            }
        };

        RetStmt { value }
    }

    fn type_check_goto_stmt(&mut self, goto_stmt: &resolver::GotoStmt) -> GotoStmt {
        let ir_index = self.get_label_ir(goto_stmt.label);

        if !self
            .interprets
            .is_some_and(|interprets| self.is_same_ir(ir_index, interprets.value))
        {
            self.type_checker
                .errors
                .push(TypeCheckError::GotoIrMismatch {
                    label: Spanned::new(
                        goto_stmt.label.span,
                        self.get_label_ident(goto_stmt.label.value).value,
                    ),

                    callable: match self.context {
                        ItemContext::Callable { item, .. } => Some(item.path.value),
                        ItemContext::Var { .. } => None,
                    },

                    expected_ir: self.interprets.map(|interprets| {
                        interprets.map(|interprets| self.env[interprets].ident.value)
                    }),
                    found_ir: self.env[ir_index].ident.value,
                });
        }

        GotoStmt {
            label: goto_stmt.label.value,
            ir: ir_index,
        }
    }

    fn type_check_bind_stmt(&mut self, bind_stmt: &resolver::BindStmt) -> BindStmt {
        let ir_index = self.get_label_ir(bind_stmt.label);

        if !self
            .emits
            .is_some_and(|emits| self.is_same_ir(ir_index, emits.value))
        {
            self.type_checker
                .errors
                .push(TypeCheckError::BindIrMismatch {
                    label: Spanned::new(
                        bind_stmt.label.span,
                        self.get_label_ident(bind_stmt.label.value).value,
                    ),
                    callable: match self.context {
                        ItemContext::Callable { item, .. } => Some(item.path.value),
                        ItemContext::Var { .. } => None,
                    },
                    expected_ir: self
                        .emits
                        .map(|emits| emits.map(|emits| self.env[emits].ident.value)),
                    found_ir: self.env[ir_index].ident.value,
                });
        }

        BindStmt {
            label: bind_stmt.label.value,
            ir: ir_index,
        }
    }

    fn type_check_emit_stmt(&mut self, call: &resolver::Call) -> Option<EmitStmt> {
        let callable_item = &self.env[call.target.value];
        let ir_index = match callable_item.parent {
            Some(ParentIndex::Ir(ir_index)) => {
                if self
                    .emits
                    .is_some_and(|emits| self.is_same_ir(emits.value, ir_index))
                {
                    Some(ir_index)
                } else {
                    self.type_checker
                        .errors
                        .push(TypeCheckError::EmitIrMismatch {
                            op: Spanned::new(call.target.span, callable_item.path.value),
                            callable: match self.context {
                                ItemContext::Callable { item, .. } => Some(item.path.value),
                                ItemContext::Var { .. } => None,
                            },
                            expected_ir: self
                                .emits
                                .map(|emits| emits.map(|emits| self.env[emits].ident.value)),
                            found_ir: self.env[ir_index].ident.value,
                        });
                    None
                }
            }
            _ => panic!("emitted op with no parent IR"),
        };

        let call = self.type_check_call(call);

        Some(EmitStmt {
            call,
            ir: ir_index?,
        })
    }

    fn type_check_expr_expecting_type(
        &mut self,
        expr: Spanned<&resolver::Expr>,
        expected_type_index: TypeIndex,
    ) -> Expr {
        let expr = expr.map(|expr| self.type_check_expr(expr));
        self.expect_expr_type(expr, expected_type_index)
    }

    fn type_check_expr(&mut self, expr: &resolver::Expr) -> Expr {
        match expr {
            resolver::Expr::Block(block) => self.type_check_kinded_block(block).into(),
            resolver::Expr::Literal(literal) => literal.into(),
            resolver::Expr::Var(var_index) => self.type_check_var_expr(*var_index).into(),
            resolver::Expr::Invoke(call) => self.type_check_invoke_expr(call).into(),
            resolver::Expr::FieldAccess(field_access_expr) => {
                self.type_check_field_access_expr(field_access_expr).into()
            }
            resolver::Expr::Negate(negate_expr) => self.type_check_negate_expr(negate_expr).into(),
            resolver::Expr::Cast(cast_expr) => self.type_check_cast_expr(cast_expr),
            resolver::Expr::BinOper(bin_oper_expr) => {
                self.type_check_bin_oper_expr(bin_oper_expr).into()
            }
            resolver::Expr::Assign(assign_expr) => self.type_check_assign_expr(assign_expr).into(),
        }
    }

    fn type_check_var_expr(&mut self, var_index: Spanned<VarIndex>) -> VarExpr {
        let var_type_index = self.get_var_type(var_index);
        if let VarIndex::Global(target) = var_index.value {
            if let ItemContext::Var { index: source } = self.context {
                self.var_graph
                    .record_ref(source, Spanned::new(var_index.span, target));
            }
        }

        VarExpr {
            var: var_index.value,
            type_: var_type_index,
        }
    }

    fn type_check_invoke_expr(&mut self, call: &resolver::Call) -> InvokeExpr {
        let ret = self.env[call.target.value].type_();

        // if the function being called has an emits annotation then
        // the emitted IR should match the one being emitted in the
        // current callable context.
        let callable_item = &self.env[call.target.value];
        match callable_item.emits {
            Some(ir_index) => {
                if !self
                    .emits
                    .is_some_and(|emits| self.is_same_ir(emits.value, ir_index.value))
                {
                    self.type_checker
                        .errors
                        .push(TypeCheckError::EmitsFnIrMismatch {
                            fn_: Spanned::new(call.target.span, callable_item.path.value),
                            callable: match self.context {
                                ItemContext::Callable { item, .. } => Some(item.path.value),
                                ItemContext::Var { .. } => None,
                            },
                            expected_ir: self
                                .emits
                                .map(|emits| emits.map(|emits| self.env[emits].ident.value)),
                            found_ir: self.env[ir_index.value].ident.value,
                        });
                }
            }
            None => (),
        }

        let call = self.type_check_call(call);
        InvokeExpr { call, ret }
    }

    fn type_check_field_access_expr(
        &mut self,
        field_access: &resolver::FieldAccess,
    ) -> FieldAccessExpr {
        let (parent, struct_field_index, field) = self.type_check_field_access(field_access);

        let type_ = match field {
            Some(resolver::Field::Var(resolver::VarField { type_, .. })) => *type_,
            Some(resolver::Field::Label(_)) => {
                self.type_checker
                    .errors
                    .push(TypeCheckError::LabelFieldUsedAsVarField {
                        field: field_access.field,
                        parent_type: self.env[struct_field_index.struct_index].ident,
                    });
                self.unknown_type()
            }
            None => self.unknown_type(),
        };

        FieldAccessExpr {
            parent,
            type_,
            field: struct_field_index,
        }
    }

    fn type_check_negate_expr(&mut self, negate_expr: &resolver::NegateExpr) -> NegateExpr {
        let expr = negate_expr
            .expr
            .as_ref()
            .map(|expr| self.type_check_expr(expr));
        let expr_type_index = expr.value.type_();

        let expr = match negate_expr.kind.value {
            NegateKind::Arith => {
                if !self.is_signed_numeric_type(expr_type_index) {
                    self.type_checker
                        .errors
                        .push(TypeCheckError::NumericOperatorTypeMismatch {
                            operand_span: expr.span,
                            operand_type: self.get_type_ident(expr_type_index),
                            operator_span: negate_expr.kind.span,
                        });
                }

                match expr_type_index {
                    TypeIndex::BuiltIn(BuiltInType::Bool) => CastExpr {
                        kind: CastSafety::Lossless,
                        expr: expr.value,
                        type_: BuiltInType::INT32.into(),
                    }
                    .into(),
                    _ => expr.value,
                }
            }
            NegateKind::Bitwise => {
                if !self.is_integral_type(expr_type_index) {
                    self.type_checker
                        .errors
                        .push(TypeCheckError::NumericOperatorTypeMismatch {
                            operand_span: expr.span,
                            operand_type: self.get_type_ident(expr_type_index),
                            operator_span: negate_expr.kind.span,
                        });
                }

                expr.value
            }
            NegateKind::Logical => self.expect_expr_type(expr, BuiltInType::Bool.into()),
        };

        NegateExpr {
            kind: negate_expr.kind.value,
            expr,
        }
    }

    fn type_check_cast_expr(&mut self, cast_expr: &resolver::CastExpr) -> Expr {
        let expr = self.type_check_expr(&cast_expr.expr.value);
        let source_type_index = expr.type_();
        let target_type_index = cast_expr.type_;
        match self.try_build_cast_chain(CastSafety::Truncating, expr, target_type_index.value) {
            Ok(expr) => expr,
            Err(expr) => {
                // Let's try downcasting
                if let Some(route) = self.find_upcast_route(expr.type_(), target_type_index.value)
                {
                    // Okay, we found a downcast route, but let's make sure we're allowed to use it
                    if !self.is_unsafe {
                        self.type_checker
                            .errors
                            .push(TypeCheckError::UnsafeCastInSafeContext {
                                source_type: self.get_type_ident(source_type_index),
                                target_type: target_type_index.map(|target_type_index| {
                                    self.get_type_ident(target_type_index)
                                }),
                            });
                    }

                    // We now have a route from destination type to expr_type: (dest, expr]
                    // but we want a route from expr_type to destination: (expr, dest]
                    // so we reverse the route, skip the first entry (the expr type) and append the
                    // dest type
                    return build_cast_chain(
                        CastSafety::Unsafe,
                        expr,
                        iterate![..route.into_iter().rev().skip(1), target_type_index.value],
                    );
                }

                // Okay, we tried out best but no possible chain exists
                self.type_checker.errors.push(TypeCheckError::InvalidCast {
                    source_type: self.get_type_ident(source_type_index),
                    target_type: self.get_type_ident(target_type_index.value),
                    expr_span: cast_expr.expr.span,
                });

                // Emit an impossible, but correctly-typed cast expression so that anything
                // depending on its type for inference lines up correctly.
                CastExpr {
                    kind: CastSafety::Lossless,
                    expr,
                    type_: target_type_index.value,
                }
                .into()
            }
        }
    }

    fn type_check_bin_oper_expr(&mut self, bin_oper_expr: &resolver::BinOperExpr) -> BinOperExpr {
        let lhs = self.type_check_expr(&bin_oper_expr.lhs.value);
        let rhs = self.type_check_expr(&bin_oper_expr.rhs.value);

        let lhs_type_index = lhs.type_();
        let rhs_type_index = rhs.type_();

        // Check that the types of the operands are compatible with the
        // operator.
        let (lhs_type_matches, rhs_type_matches) = match bin_oper_expr.oper.value {
            BinOper::Arith(ArithBinOper::Mod) | BinOper::Bitwise(_) => (
                self.is_integral_type(lhs_type_index),
                self.is_integral_type(rhs_type_index),
            ),
            BinOper::Arith(_) | BinOper::Compare(CompareBinOper::Numeric(_)) => (
                self.is_numeric_type(lhs_type_index),
                self.is_numeric_type(rhs_type_index),
            ),
            BinOper::Compare(_) => (true, true),
            BinOper::Logical(_) => (
                self.is_same_type(lhs_type_index, BuiltInType::Bool.into()),
                self.is_same_type(rhs_type_index, BuiltInType::Bool.into()),
            ),
        };

        if !lhs_type_matches {
            self.type_checker
                .errors
                .push(TypeCheckError::NumericOperatorTypeMismatch {
                    operand_span: bin_oper_expr.lhs.span,
                    operand_type: self.get_type_ident(lhs_type_index),
                    operator_span: bin_oper_expr.oper.span,
                });
        }

        if !rhs_type_matches {
            self.type_checker
                .errors
                .push(TypeCheckError::NumericOperatorTypeMismatch {
                    operand_span: bin_oper_expr.rhs.span,
                    operand_type: self.get_type_ident(rhs_type_index),
                    operator_span: bin_oper_expr.oper.span,
                });
        }

        // Ensure the types of the left- and right-hand operands match,
        // upcasting as necessary.
        let (lhs, rhs) = match self.try_build_cast_chain(CastSafety::Lossless, rhs, lhs_type_index)
        {
            Ok(rhs) => (lhs, rhs),
            Err(rhs) => match self.try_build_cast_chain(CastSafety::Lossless, lhs, rhs_type_index)
            {
                Ok(lhs) => (lhs, rhs),
                Err(lhs) => {
                    self.type_checker
                        .errors
                        .push(TypeCheckError::BinaryOperatorTypeMismatch {
                            operator_span: bin_oper_expr.oper.span,
                            lhs_span: bin_oper_expr.lhs.span,
                            lhs_type: self.get_type_ident(lhs_type_index),
                            rhs_span: bin_oper_expr.rhs.span,
                            rhs_type: self.get_type_ident(rhs_type_index),
                        });

                    (lhs, rhs)
                }
            },
        };

        // Compute the output type.
        let type_ = match bin_oper_expr.oper.value {
            // Make sure to use the operand type post-upcasting. If the types
            // didn't match, we'll assume the type of the left-hand side.
            BinOper::Arith(_) | BinOper::Bitwise(_) => lhs.type_(),
            BinOper::Compare(_) | BinOper::Logical(_) => BuiltInType::Bool.into(),
        };

        BinOperExpr {
            oper: bin_oper_expr.oper.value,
            lhs,
            rhs,
            type_,
        }
    }

    fn type_check_assign_expr(&mut self, assign_expr: &resolver::AssignExpr) -> AssignExpr {
        let lhs_type_index = self.get_var_type(assign_expr.lhs);

        let rhs = self.type_check_expr(&assign_expr.rhs.value);
        let rhs = match self.try_build_cast_chain(CastSafety::Lossless, rhs, lhs_type_index) {
            Ok(rhs) => rhs,
            Err(rhs) => {
                let lhs_path = self.get_var_path(assign_expr.lhs.value);
                self.type_checker
                    .errors
                    .push(TypeCheckError::AssignTypeMismatch {
                        expected_type: self.get_type_ident(lhs_type_index),
                        found_type: self.get_type_ident(rhs.type_()),
                        lhs: lhs_path.value,
                        lhs_defined_at: lhs_path.span,
                        rhs_span: assign_expr.rhs.span,
                    });
                rhs
            }
        };

        AssignExpr {
            lhs: assign_expr.lhs,
            rhs,
        }
    }

    fn get_var_path(&self, var_index: VarIndex) -> MaybeSpanned<Path> {
        match var_index {
            VarIndex::BuiltIn(built_in_var) => Path::from(built_in_var.ident()).into(),
            VarIndex::EnumVariant(enum_variant_index) => self.env[enum_variant_index].into(),
            VarIndex::Global(global_var_index) => self.env[global_var_index].path.into(),
            VarIndex::Param(var_param_index) => self
                .context
                .param(var_param_index)
                .ident
                .map(Path::from)
                .into(),
            VarIndex::Local(local_var_index) => self
                .context
                .local(local_var_index)
                .ident
                .map(Path::from)
                .into(),
        }
    }

    fn get_var_type(&mut self, var_index: Spanned<VarIndex>) -> TypeIndex {
        match var_index.value {
            VarIndex::BuiltIn(built_in_var) => built_in_var.type_(),
            VarIndex::EnumVariant(enum_variant_index) => enum_variant_index.type_(),
            VarIndex::Global(global_var_index) => self.env[global_var_index].type_,
            VarIndex::Param(var_param_index) => self.context.param(var_param_index).type_,
            VarIndex::Local(local_var_index) => {
                let local_var = &self.context.local(local_var_index);
                match local_var.type_ {
                    Some(type_) => type_,
                    None => panic!(
                        "local variable {} used at {} before its declaration at {}",
                        local_var.ident, var_index.span, local_var.ident.span
                    ),
                }
            }
        }
    }

    fn get_label_ident(&self, label_index: LabelIndex) -> Spanned<Ident> {
        match label_index {
            LabelIndex::Param(label_param_index) => {
                self.context.param(label_param_index).label.ident
            }
            LabelIndex::Local(local_label_index) => self.context.local(local_label_index).ident,
        }
    }

    fn get_label_ir(&self, label_index: Spanned<LabelIndex>) -> IrIndex {
        match label_index.value {
            LabelIndex::Param(label_param_index) => self.context.param(label_param_index).label.ir,
            LabelIndex::Local(local_label_index) => {
                let local_label = &self.context.local(local_label_index);
                match local_label.ir {
                    Some(ir) => ir,
                    None => panic!(
                        "local label {} used at {} before its declaration at {}",
                        local_label.ident, label_index.span, local_label.ident.span,
                    ),
                }
            }
        }
    }
}

struct ParamSummary {
    ident: Spanned<Ident>,
    type_: Option<TypeIndex>,
    ir: Option<IrIndex>,
    expected_arg_kind: ArgKind,
}

fn does_stmt_exit_early(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Let(LetStmt { rhs: expr, .. }) | Stmt::Expr(expr) => does_expr_exit_early(expr),
        Stmt::If(if_stmt) => does_if_stmt_exit_early(if_stmt),
        Stmt::ForIn(for_in_stmt) => for_in_stmt.body.exits_early,
        Stmt::Ret(_) => true,
        Stmt::Check(check_stmt) => does_check_stmt_exit_early(check_stmt),
        Stmt::Label(_) | Stmt::Goto(_) | Stmt::Bind(_) | Stmt::Emit(_) => false,
    }
}

// Treat `assert false` as an early exit
fn does_check_stmt_exit_early(check_stmt: &CheckStmt) -> bool {
    check_stmt.kind == CheckKind::Assert && is_false(&check_stmt.cond)
}

fn is_false(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Var(VarExpr {
            var: VarIndex::BuiltIn(BuiltInVar::False),
            ..
        })
    )
}

fn does_if_stmt_exit_early(if_stmt: &IfStmt) -> bool {
    if does_expr_exit_early(&if_stmt.cond) {
        return true;
    }

    if_stmt.then.exits_early
        && match &if_stmt.else_ {
            Some(ElseClause::ElseIf(else_if)) => does_if_stmt_exit_early(else_if),
            Some(ElseClause::Else(else_block)) => else_block.exits_early,
            None => false,
        }
}

fn does_expr_exit_early(expr: &Expr) -> bool {
    match expr {
        Expr::Block(kinded_block) => kinded_block.block.exits_early,
        Expr::Literal(_) | Expr::Var(_) | Expr::Invoke(_) => false,
        Expr::FieldAccess(field_access_expr) => does_expr_exit_early(&field_access_expr.parent),
        Expr::Negate(negate_expr) => does_expr_exit_early(&negate_expr.expr),
        Expr::Cast(cast_expr) => does_expr_exit_early(&cast_expr.expr),
        Expr::BinOper(bin_oper_expr) => {
            if bin_oper_expr.oper.is_short_circuiting() {
                does_expr_exit_early(&bin_oper_expr.lhs)
            } else {
                does_expr_exit_early(&bin_oper_expr.lhs)
                    || does_expr_exit_early(&bin_oper_expr.rhs)
            }
        }
        Expr::Assign(assign_expr) => does_expr_exit_early(&assign_expr.rhs),
    }
}

fn is_const(expr: &Expr) -> bool {
    match expr {
        Expr::Var(_) | Expr::Literal(_) => true,
        Expr::BinOper(e) => match e.oper {
            BinOper::Arith(_) | BinOper::Bitwise(_) | BinOper::Compare(_) => {
                is_const(&e.lhs) && is_const(&e.rhs)
            }

            // Don't allow logical operators because the normalizer will introduce if/else
            // statements.
            BinOper::Logical(_) => false,
        },
        Expr::Negate(e) => is_const(&e.expr),
        Expr::Invoke(_) | Expr::FieldAccess(_) | Expr::Block(_) | Expr::Assign(_) => false,
        Expr::Cast(cast_expr) => {
            is_const(&cast_expr.expr) && matches!(cast_expr.type_, TypeIndex::BuiltIn(_))
        }
    }
}

fn cast_literal(l: &Literal, t: BuiltInType) -> Option<Literal> {
    match l {
        Literal::Int8(v) => try_construct_lit(t, *v),
        Literal::Int16(v) => try_construct_lit(t, *v),
        Literal::Int32(v) => try_construct_lit(t, *v),
        Literal::Int64(v) => try_construct_lit(t, *v),
        Literal::UInt8(v) => try_construct_lit(t, *v),
        Literal::UInt16(v) => try_construct_lit(t, *v),
        Literal::UInt32(v) => try_construct_lit(t, *v),
        Literal::UInt64(v) => try_construct_lit(t, *v),
        Literal::Double(_) => None,
    }
}

fn try_construct_lit<V>(t: BuiltInType, v: V) -> Option<Literal>
where
    i8: TryFrom<V>,
    i16: TryFrom<V>,
    i32: TryFrom<V>,
    i64: TryFrom<V>,
    u8: TryFrom<V>,
    u16: TryFrom<V>,
    u32: TryFrom<V>,
    u64: TryFrom<V>,
{
    use Signedness::*;
    use Width::*;

    match t {
        BuiltInType::Unit => None?,
        BuiltInType::Bool => None?,
        BuiltInType::Double => None?,
        BuiltInType::Integral(Signed, W8) => Literal::Int8(v.try_into().ok()?),
        BuiltInType::Integral(Signed, W16) => Literal::Int16(v.try_into().ok()?),
        BuiltInType::Integral(Signed, W32) => Literal::Int32(v.try_into().ok()?),
        BuiltInType::Integral(Signed, W64) => Literal::Int64(v.try_into().ok()?),
        BuiltInType::Integral(Unsigned, W8) => Literal::UInt8(v.try_into().ok()?),
        BuiltInType::Integral(Unsigned, W16) => Literal::UInt16(v.try_into().ok()?),
        BuiltInType::Integral(Unsigned, W32) => Literal::UInt32(v.try_into().ok()?),
        BuiltInType::Integral(Unsigned, W64) => Literal::UInt64(v.try_into().ok()?),
    }
    .into()
}
