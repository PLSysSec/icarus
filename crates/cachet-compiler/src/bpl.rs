// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::collections::{BTreeSet, HashMap};
use std::fmt::{self, Display};
use std::iter;
use std::ops::{Deref, DerefMut, Index};

use enum_map::EnumMap;
use fix_hidden_lifetime_bug::Captures;
use iterate::iterate;
use lazy_static::lazy_static;
use typed_index_collections::TiSlice;
use void::unreachable;

use cachet_lang::ast::{
    BinOper, CheckKind, CompareBinOper, ForInOrder, Ident, NegateKind, Path, Spanned, VarParamKind,
};
use cachet_lang::built_in::{BuiltInVar, IdentEnum};
use cachet_lang::flattener::{self, HasAttrs, Typed};
use cachet_util::MaybeOwned;

use crate::bpl::ast::*;
use crate::bpl::flow::{EmitNode, EmitSucc, FlowGraph, trace_entry_point};

mod ast;
mod flow;

#[derive(Clone, Debug)]
pub struct BplCode(Code);

impl Display for BplCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}\n\n{}", include_str!("bpl/prelude.bpl"), self.0)
    }
}

pub fn compile(env: &flattener::Env) -> BplCode {
    let mut compiler = Compiler::new(env);

    for enum_item in &env.enum_items {
        compiler.compile_enum_item(enum_item);
    }

    for struct_item in &env.struct_items {
        compiler.compile_struct_item(struct_item);
    }

    for ir_item in &env.ir_items {
        compiler.compile_ir_item(ir_item);
    }

    for global_var_item in env.global_var_items.keys() {
        compiler.compile_global_var_item(global_var_item);
    }

    for fn_index in env.fn_items.keys() {
        compiler.compile_callable_item(fn_index.into());
    }

    for op_index in env.op_items.keys() {
        compiler.compile_callable_item(op_index.into());
    }

    if let Some(top_op_item) = env.op_items.last() {
        if let Some(flattener::ParentIndex::Ir(top_ir_index)) = top_op_item.parent {
            let mut bottom_ir_index = top_ir_index;
            while let Some(next_ir_index) = env[bottom_ir_index].emits {
                bottom_ir_index = next_ir_index.value;
            }

            let flow_graph = trace_entry_point(&env, top_op_item, bottom_ir_index);
            compiler.compile_entry_point(top_op_item, top_ir_index, bottom_ir_index, &flow_graph);
        }
    }

    BplCode(compiler.items.into())
}

struct Compiler<'a> {
    env: &'a flattener::Env,
    items: Vec<Item>,
}

impl<'a> Compiler<'a> {
    fn new(env: &'a flattener::Env) -> Self {
        Compiler {
            env,
            items: Vec::new(),
        }
    }

    fn compile_enum_item(&mut self, enum_item: &flattener::EnumItem) {
        let type_ident = UserTypeIdent::from(enum_item.ident.value);

        if BLOCKED_PATHS.contains_key(&enum_item.ident.value.into()) {
            return;
        }

        let type_item = TypeItem {
            ident: type_ident.into(),
            attr: Some(TypeAttr::DataType),
            type_: None,
        }
        .into();

        let variant_ctor_fn_items = enum_item.variants.iter().map(|variant_path| {
            FnItem {
                ident: TypeMemberFnIdent {
                    type_ident,
                    selector: VariantCtorTypeMemberFnSelector::from(variant_path.value.ident())
                        .into(),
                }
                .into(),
                attr: Some(FnAttr::Ctor),
                param_vars: TypedVars::default(),
                ret: type_ident.into(),
                value: None,
            }
            .into()
        });

        self.items
            .extend(iterate![type_item, ..variant_ctor_fn_items]);
    }

    fn compile_struct_item(&mut self, struct_item: &flattener::StructItem) {
        if BLOCKED_PATHS.contains_key(&struct_item.ident.value.into()) {
            return;
        }

        let type_ident = UserTypeIdent::from(struct_item.ident.value);

        // Struct types with supertypes compile to type aliases of the root
        // supertype, so we don't have to worry about conversions.
        let mut root_supertype_type_index = None;
        let mut curr_struct_item = struct_item;
        while let Some(supertype_type_index) = curr_struct_item.supertype {
            root_supertype_type_index = Some(supertype_type_index);
            match supertype_type_index {
                flattener::TypeIndex::Struct(supertype_struct_index) => {
                    curr_struct_item = &self.env[supertype_struct_index];
                }
                _ => break,
            }
        }
        let root_supertype =
            root_supertype_type_index.map(|type_index| self.get_type_ident(type_index).into());

        self.items.push(
            TypeItem {
                ident: type_ident.into(),
                attr: None,
                type_: root_supertype,
            }
            .into(),
        );

        for field in &struct_item.fields {
            let ident = TypeMemberFnIdent {
                type_ident,
                selector: TypeMemberFnSelector::Field(field.ident().value.into()),
            }
            .into();

            let (attr, ret, value) = match field {
                flattener::Field::Var(var_field) => {
                    let ret = self.get_type_ident(var_field.type_).into();
                    (None, ret, None)
                }
                // For label fields, generate an inline function that returns
                // the well-known exit label bound in the entry-point.
                flattener::Field::Label(_) => {
                    let attr = FnAttr::Inline;
                    let ret = PreludeTypeIdent::Label.into();
                    let value = CallExpr {
                        target: PreludeFnIdent::ExitLabel.into(),
                        arg_exprs: vec![],
                    }
                    .into();
                    (Some(attr), ret, Some(value))
                }
            };

            self.items.push(
                FnItem {
                    ident,
                    param_vars: vec![
                        TypedVar {
                            ident: ParamVarIdent::Instance.into(),
                            type_: type_ident.into(),
                        }
                        .into(),
                    ]
                    .into(),
                    attr,
                    ret,
                    value,
                }
                .into(),
            )
        }
    }

    fn compile_ir_item(&mut self, ir_item: &flattener::IrItem) {
        // TODO(spinda): Always generate label infrastructure for IRs.

        if ir_item.emits.is_some() || BLOCKED_PATHS.contains_key(&ir_item.ident.value.into()) {
            return;
        }

        let ir_ident = IrIdent::from(ir_item.ident.value);

        let op_type_item = TypeItem {
            ident: IrMemberTypeIdent {
                ir_ident,
                selector: IrMemberTypeSelector::Op,
            }
            .into(),
            attr: Some(TypeAttr::DataType),
            type_: None,
        };

        let exit_op_ctor_fn_item = FnItem {
            ident: IrMemberFnIdent {
                ir_ident,
                selector: OpCtorIrMemberFnSelector::from(OpSelector::Exit).into(),
            }
            .into(),
            attr: Some(FnAttr::Ctor),
            param_vars: TypedVars::default(),
            ret: op_type_item.ident.into(),
            value: None,
        };

        let pc_global_var_item = GlobalVarItem::from(TypedVar {
            ident: IrMemberGlobalVarIdent {
                ir_ident,
                selector: IrMemberGlobalVarSelector::Pc,
            }
            .into(),
            type_: PreludeTypeIdent::Pc.into(),
        });

        let op_at_fn_item = FnItem {
            ident: IrMemberFnIdent {
                ir_ident,
                selector: IrMemberFnSelector::OpAt,
            }
            .into(),
            attr: None,
            param_vars: vec![TypedVar {
                ident: ParamVarIdent::Pc.into(),
                type_: PreludeTypeIdent::Pc.into(),
            }]
            .into(),
            ret: op_type_item.ident.into(),
            value: None,
        };

        let next_pc_fn_item = FnItem {
            ident: IrMemberFnIdent {
                ir_ident,
                selector: IrMemberFnSelector::NextPc,
            }
            .into(),
            attr: None,
            param_vars: vec![TypedVar {
                ident: ParamVarIdent::Pc.into(),
                type_: PreludeTypeIdent::Pc.into(),
            }]
            .into(),
            ret: PreludeTypeIdent::Pc.into(),
            value: None,
        };

        let step_proc_item = ProcItem {
            ident: IrMemberFnIdent {
                ir_ident,
                selector: IrMemberFnSelector::Step,
            }
            .into(),
            attr: Some(InlineProcAttr { depth: 1 }.into()),
            param_vars: TypedVars::default(),
            ret_vars: TypedVars::default(),
            body: Some(Body {
                local_vars: Vec::new(),
                stmts: vec![
                    AssignStmt {
                        lhs: pc_global_var_item.var.ident.into(),
                        rhs: CallExpr {
                            target: next_pc_fn_item.ident,
                            arg_exprs: vec![pc_global_var_item.var.ident.into()],
                        }
                        .into(),
                    }
                    .into(),
                ],
            }),
        };

        let emit_proc_item = ProcItem {
            ident: IrMemberFnIdent {
                ir_ident,
                selector: IrMemberFnSelector::Emit,
            }
            .into(),
            attr: None,
            param_vars: vec![
                TypedVar {
                    ident: ParamVarIdent::Op.into(),
                    type_: op_type_item.ident.into(),
                },
                TypedVar {
                    ident: ParamVarIdent::Pc.into(),
                    type_: PreludeTypeIdent::Pc.into(),
                },
            ]
            .into(),
            ret_vars: TypedVars::default(),
            body: Some(Body {
                local_vars: Vec::new(),
                stmts: vec![
                    CheckStmt {
                        kind: CheckKind::Assume,
                        attr: None,
                        cond: BinOperExpr {
                            oper: CompareBinOper::Eq.into(),
                            lhs: CallExpr {
                                target: op_at_fn_item.ident,
                                arg_exprs: vec![ParamVarIdent::Pc.into()],
                            }
                            .into(),
                            rhs: ParamVarIdent::Op.into(),
                        }
                        .into(),
                    }
                    .into(),
                    CheckStmt {
                        kind: CheckKind::Assume,
                        attr: None,
                        cond: BinOperExpr {
                            oper: CompareBinOper::Eq.into(),
                            lhs: CallExpr {
                                target: next_pc_fn_item.ident,
                                arg_exprs: vec![pc_global_var_item.var.ident.into()],
                            }
                            .into(),
                            rhs: ParamVarIdent::Pc.into(),
                        }
                        .into(),
                    }
                    .into(),
                    AssignStmt {
                        lhs: pc_global_var_item.var.ident.into(),
                        rhs: ParamVarIdent::Pc.into(),
                    }
                    .into(),
                ],
            }),
        };

        let label_bound_pc_fn_item = FnItem {
            ident: IrMemberFnIdent {
                ir_ident,
                selector: IrMemberFnSelector::LabelBoundPc,
            }
            .into(),
            attr: None,
            param_vars: vec![TypedVar {
                ident: ParamVarIdent::Label.into(),
                type_: PreludeTypeIdent::Label.into(),
            }]
            .into(),
            ret: PreludeTypeIdent::Pc.into(),
            value: None,
        };

        let bind_proc_item = ProcItem {
            ident: IrMemberFnIdent {
                ir_ident,
                selector: IrMemberFnSelector::Bind,
            }
            .into(),
            attr: Some(InlineProcAttr { depth: 1 }.into()),
            param_vars: vec![TypedVar {
                ident: ParamVarIdent::Label.into(),
                type_: PreludeTypeIdent::Label.into(),
            }]
            .into(),
            ret_vars: TypedVars::default(),
            body: Some(Body {
                local_vars: Vec::new(),
                stmts: vec![
                    CheckStmt {
                        kind: CheckKind::Assume,
                        attr: None,
                        cond: BinOperExpr {
                            oper: CompareBinOper::Eq.into(),
                            lhs: CallExpr {
                                target: label_bound_pc_fn_item.ident,
                                arg_exprs: vec![ParamVarIdent::Label.into()],
                            }
                            .into(),
                            rhs: CallExpr {
                                target: next_pc_fn_item.ident,
                                arg_exprs: vec![pc_global_var_item.var.ident.into()],
                            }
                            .into(),
                        }
                        .into(),
                    }
                    .into(),
                ],
            }),
        };

        let bind_exit_proc_item = ProcItem {
            ident: IrMemberFnIdent {
                ir_ident,
                selector: IrMemberFnSelector::BindExit,
            }
            .into(),
            attr: Some(InlineProcAttr { depth: 1 }.into()),
            param_vars: vec![TypedVar {
                ident: ParamVarIdent::Label.into(),
                type_: PreludeTypeIdent::Label.into(),
            }]
            .into(),
            ret_vars: TypedVars::default(),
            body: Some(Body {
                local_vars: Vec::new(),
                stmts: vec![
                    CheckStmt {
                        kind: CheckKind::Assume,
                        attr: None,
                        cond: BinOperExpr {
                            oper: CompareBinOper::Eq.into(),
                            lhs: CallExpr {
                                target: label_bound_pc_fn_item.ident,
                                arg_exprs: vec![ParamVarIdent::Label.into()],
                            }
                            .into(),
                            rhs: CallExpr {
                                target: PreludeFnIdent::ExitPc.into(),
                                arg_exprs: vec![],
                            }
                            .into(),
                        }
                        .into(),
                    }
                    .into(),
                ],
            }),
        };

        let goto_proc_item = ProcItem {
            ident: IrMemberFnIdent {
                ir_ident,
                selector: IrMemberFnSelector::Goto,
            }
            .into(),
            attr: Some(InlineProcAttr { depth: 1 }.into()),
            param_vars: vec![TypedVar {
                ident: ParamVarIdent::Label.into(),
                type_: PreludeTypeIdent::Label.into(),
            }]
            .into(),
            ret_vars: TypedVars::default(),
            body: Some(Body {
                local_vars: Vec::new(),
                stmts: vec![
                    AssignStmt {
                        lhs: pc_global_var_item.var.ident.into(),
                        rhs: CallExpr {
                            target: label_bound_pc_fn_item.ident,
                            arg_exprs: vec![ParamVarIdent::Label.into()],
                        }
                        .into(),
                    }
                    .into(),
                ],
            }),
        };

        self.items.extend([
            op_type_item.into(),
            exit_op_ctor_fn_item.into(),
            pc_global_var_item.into(),
            op_at_fn_item.into(),
            next_pc_fn_item.into(),
            step_proc_item.into(),
            emit_proc_item.into(),
            label_bound_pc_fn_item.into(),
            bind_proc_item.into(),
            bind_exit_proc_item.into(),
            goto_proc_item.into(),
        ]);
    }

    fn compile_global_var_item(&mut self, global_var_index: flattener::GlobalVarIndex) {
        let global_var_item = &self.env[global_var_index];

        if global_var_item.is_prelude() {
            return;
        }

        let parent_ident = global_var_item
            .path
            .value
            .parent()
            .map(|parent_path| parent_path.ident());
        let var_ident = global_var_item.path.value.ident();
        let type_ = self.get_type_ident(global_var_item.type_).into();

        // If there's a value, associated with the global var then it is a const
        // We model these as functions whose body is the value expression.
        if let Some(expr) = &global_var_item.value {
            let ident = UserFnIdent {
                parent_ident,
                fn_ident: var_ident,
            }
            .into();

            let mut local_vars = vec![];
            let mut scoped_compiler = ScopedCompiler::new(self, ItemContext::Var, &mut local_vars);

            let value = scoped_compiler.compile_expr(expr.as_ref().value).into();

            self.items.push(
                FnItem {
                    ident,
                    attr: Some(FnAttr::Inline),
                    param_vars: TypedVars::default(),
                    ret: type_,
                    value,
                }
                .into(),
            )
        } else {
            let ident = UserGlobalVarIdent {
                parent_ident,
                var_ident,
            }
            .into();

            let var = TypedVar { ident, type_ };

            self.items.push(if global_var_item.is_mut {
                GlobalVarItem::from(var).into()
            } else {
                ConstItem::from(var).into()
            });
        }
    }

    fn compile_callable_item(&mut self, callable_index: flattener::CallableIndex) {
        let callable_item = &self.env[callable_index];
        if callable_item.is_prelude() || BLOCKED_PATHS.contains_key(&callable_item.path.value) {
            return;
        }

        let callable_parent_ident = callable_item
            .path
            .value
            .parent()
            .map(|parent_path| parent_path.ident());
        let callable_ident = callable_item.path.value.ident();
        let fn_ident = UserFnIdent {
            parent_ident: callable_parent_ident,
            fn_ident: callable_ident,
        };

        let (mut param_vars, mut ret_vars) =
            self.compile_params(&callable_item.params, &callable_item.param_order);

        if let flattener::CallableIndex::Op(_) = callable_index {
            // Define datatype constructors for interpreter ops.
            if let Some(ir_index) = callable_item.interprets {
                let ir_ident = self.env[ir_index].ident.value.into();
                let ident = IrMemberFnIdent {
                    ir_ident,
                    selector: OpCtorIrMemberFnSelector {
                        op_selector: UserOpSelector::from(callable_ident).into(),
                    }
                    .into(),
                }
                .into();

                let ret = IrMemberTypeIdent {
                    ir_ident,
                    selector: IrMemberTypeSelector::Op,
                }
                .into();

                self.items.push(
                    FnItem {
                        ident,
                        attr: Some(FnAttr::Ctor),
                        param_vars: param_vars.clone(),
                        ret,
                        value: None,
                    }
                    .into(),
                );
            }
        }

        // Add an extra `pc` parameter to compiler ops and emitting functions.
        if let Some(_) = callable_item.emits {
            param_vars.push(TypedVar {
                ident: ParamVarIdent::Pc.into(),
                type_: PreludeTypeIdent::Pc.into(),
            });
        }

        let body = callable_item
            .body
            .as_ref()
            .map(|body| self.compile_body(callable_index, &callable_item, body));

        let mut is_inline = match callable_index {
            flattener::CallableIndex::Fn(_) => callable_item.is_inline(),
            flattener::CallableIndex::Op(_) => false,
        };
        match CallableRepr::for_callable(callable_item) {
            CallableRepr::Fn => {
                self.items.push(
                    FnItem {
                        ident: fn_ident.into(),
                        attr: if is_inline {
                            Some(FnAttr::Inline)
                        } else {
                            None
                        },
                        param_vars,
                        ret: self.get_type_ident(callable_item.type_()).into(),
                        value: None,
                    }
                    .into(),
                );
            }
            CallableRepr::Proc => {
                if let Some(ret) = callable_item.ret {
                    ret_vars.push(TypedVar {
                        ident: VarIdent::Ret,
                        type_: self.get_type_ident(ret).into(),
                    });
                }

                let body = body.unwrap_or_else(|| {
                    // For external functions represented as procedures with
                    // return varlabies (for, e.g., out-parameters), set up a
                    // procedure body where each return variable is assigned to
                    // the result of delegating to a unique uninterpreted
                    // function.

                    let arg_exprs: Vec<_> =
                        param_vars.iter().map(|param| param.ident.into()).collect();

                    let ret_var_assign_stmts = ret_vars.iter().map(|ret_var| {
                        let ident = ExternalUserFnHelperFnIdent {
                            fn_ident,
                            ret_var_ident: ret_var.ident,
                        }
                        .into();

                        self.items.push(
                            FnItem {
                                ident,
                                attr: None,
                                param_vars: param_vars.clone(),
                                ret: ret_var.type_.clone(),
                                value: None,
                            }
                            .into(),
                        );

                        AssignStmt {
                            lhs: ret_var.ident.into(),
                            rhs: CallExpr {
                                target: ident,
                                arg_exprs: arg_exprs.clone(),
                            }
                            .into(),
                        }
                        .into()
                    });

                    // Also bind all label out-parameters to an exit point.

                    let out_label_param_bind_stmts = callable_item
                        .param_order
                        .iter()
                        .filter_map(|param_index| match param_index {
                            flattener::ParamIndex::Label(label_param_index) => {
                                Some(label_param_index)
                            }
                            _ => None,
                        })
                        .map(|label_param_index| &callable_item.params[label_param_index])
                        .filter(|label_param| label_param.is_out)
                        .map(|label_param| {
                            let ir_index = label_param.label.ir;
                            let ir_ident = self.env[ir_index].ident.value.into();

                            CallExpr {
                                target: IrMemberFnIdent {
                                    ir_ident,
                                    selector: IrMemberFnSelector::BindExit,
                                }
                                .into(),
                                arg_exprs: vec![
                                    UserParamVarIdent::from(label_param.label.ident.value).into(),
                                ],
                            }
                            .into()
                        });

                    is_inline = true;
                    iterate![..ret_var_assign_stmts, ..out_label_param_bind_stmts].collect()
                });

                let attr = if is_inline {
                    Some(InlineProcAttr { depth: 1 }.into())
                } else {
                    None
                };

                self.items.push(
                    ProcItem {
                        ident: fn_ident.into(),
                        attr,
                        param_vars,
                        ret_vars,
                        body: Some(body),
                    }
                    .into(),
                );
            }
        };
    }

    fn compile_entry_point(
        &mut self,
        top_op_item: &flattener::CallableItem,
        top_ir_index: flattener::IrIndex,
        bottom_ir_index: flattener::IrIndex,
        flow_graph: &FlowGraph,
    ) {
        let top_op_ident = top_op_item.path.value.ident();

        let top_ir_ident = self.env[top_ir_index].ident.value.into();
        let bottom_ir_ident = self.env[bottom_ir_index].ident.value.into();

        let ident = EntryPointFnIdent {
            ir_ident: top_ir_ident,
            user_op_selector: top_op_ident.into(),
        }
        .into();

        // The parameters of the entry point are the variable parameters of the
        // top-level op. These will be considered by the solver to have
        // arbitrary initial values.
        let param_vars: TypedVars = top_op_item
            .param_order
            .iter()
            .filter_map(|param_index| match param_index {
                flattener::ParamIndex::Var(var_param_index) => {
                    let var_param = &top_op_item.params[var_param_index];
                    match var_param.kind {
                        VarParamKind::In | VarParamKind::Mut => {
                            Some(self.compile_var_param(var_param))
                        }
                        VarParamKind::Out => None,
                    }
                }
                _ => None,
            })
            .collect();

        // Label parameters to the top-level op are reflected as local
        // variables.
        // TODO(spinda): Instead of using actual variables for these, just pass
        // `ExitPc()` directly into the top-level label parameters.
        let label_params =
            top_op_item
                .param_order
                .iter()
                .filter_map(|param_index| match param_index {
                    flattener::ParamIndex::Label(label_param_index) => {
                        Some(&top_op_item.params[label_param_index])
                    }
                    _ => None,
                });
        let label_param_local_vars: Vec<LocalVar> = label_params
            .map(|label_param| self.compile_label_param(label_param).into())
            .collect();

        // First, some label-related bookkeeping...

        // Bind the well-known exit label to the well-known exit point PC. This
        // is used for top-level label parameters and struct label fields.
        let exit_label_expr: Expr = CallExpr {
            target: PreludeFnIdent::ExitLabel.into(),
            arg_exprs: vec![],
        }
        .into();
        let bind_exit_label_stmt = CallExpr {
            target: IrMemberFnIdent {
                ir_ident: bottom_ir_ident,
                selector: IrMemberFnSelector::BindExit,
            }
            .into(),
            arg_exprs: vec![exit_label_expr.clone()],
        }
        .into();

        let mut stmts: Vec<Stmt> = vec![bind_exit_label_stmt];

        // Point all of the label parameter local variables to the exit label we
        // just bound.
        stmts.extend(label_param_local_vars.iter().map(|label_param_local_var| {
            AssignStmt {
                lhs: label_param_local_var.var.ident.into(),
                rhs: exit_label_expr.clone(),
            }
            .into()
        }));

        // Now we're entering into op emission...

        let pc_var_ident: VarIdent = IrMemberGlobalVarIdent {
            ir_ident: bottom_ir_ident,
            selector: IrMemberGlobalVarSelector::Pc,
        }
        .into();

        // Initialize the program counter to `NilPc()` for op emission.
        let nil_pc_expr: Expr = CallExpr {
            target: PreludeFnIdent::NilPc.into(),
            arg_exprs: vec![],
        }
        .into();
        let init_emit_pc_stmt: Stmt = AssignStmt {
            lhs: pc_var_ident.into(),
            rhs: nil_pc_expr.clone(),
        }
        .into();

        // Call function corresponding to the top-level op, passing along the
        // root emit path, variable parameters, and labels from the local
        // variables declared for its labels parameters.
        let top_op_var_arg_exprs = param_vars.iter().map(|param_var| param_var.ident.into());
        let top_op_label_arg_exprs = label_param_local_vars
            .iter()
            .map(|label_param_local_var| label_param_local_var.var.ident.into());
        let top_op_arg_exprs = top_op_var_arg_exprs
            .chain(top_op_label_arg_exprs)
            .chain(iter::once(nil_pc_expr.clone()))
            .collect();
        let call_top_op_fn_stmt = CallExpr {
            target: UserFnIdent {
                parent_ident: Some(top_ir_ident.ident),
                fn_ident: top_op_ident,
            }
            .into(),
            arg_exprs: top_op_arg_exprs,
        }
        .into();

        // Emit a trailing "exit" op after everything else, to represent
        // control flow being transferred to some point outside the program,
        // either via normal termination (i.e., control flow reaching the end of
        // the generated program) or otherwise (e.g., jumping to an exit label).
        // We use a well-known fixed PC value so that the exit point has
        // a predictable PC (e.g., for binding exit labels) regardless of how
        // many ops are emitted.
        let emit_exit_op_stmt = CallExpr {
            target: IrMemberFnIdent {
                ir_ident: bottom_ir_ident,
                selector: IrMemberFnSelector::Emit,
            }
            .into(),
            arg_exprs: vec![
                CallExpr {
                    target: IrMemberFnIdent {
                        ir_ident: bottom_ir_ident,
                        selector: OpCtorIrMemberFnSelector::from(OpSelector::Exit).into(),
                    }
                    .into(),
                    arg_exprs: Vec::new(),
                }
                .into(),
                CallExpr {
                    target: PreludeFnIdent::ExitPc.into(),
                    arg_exprs: vec![],
                }
                .into(),
            ],
        }
        .into();

        // And now, the interpreter...

        // Reset the program counter before we run the interpreter. The first op
        // in the program is emitted at the PC following `NilPc()`, as that's
        // the initial PC value passed into the top-level op emitter.
        let init_interpret_pc_stmt: Stmt = AssignStmt {
            lhs: pc_var_ident.into(),
            rhs: CallExpr {
                target: IrMemberFnIdent {
                    ir_ident: bottom_ir_ident,
                    selector: IrMemberFnSelector::NextPc,
                }
                .into(),
                arg_exprs: vec![nil_pc_expr],
            }
            .into(),
        }
        .into();

        stmts.extend([
            init_emit_pc_stmt,
            call_top_op_fn_stmt,
            emit_exit_op_stmt,
            init_interpret_pc_stmt,
        ]);

        // The successors of the artifically-inserted "entry" emit node gives
        // us all the possible ops at PC 1 from which the program execution can
        // begin. We emit a jump to all the labels corresponding to all the ops.
        let entry_emit_node = &flow_graph[flow_graph.entry_emit_node_index()];
        let entry_emit_node_goto_succs_stmt =
            generate_emit_node_goto_succs_stmt(entry_emit_node, flow_graph);
        stmts.extend([entry_emit_node_goto_succs_stmt]);

        // For each emit node in the control-flow graph, emit a labeled section
        // which interprets the corresponding op and jumps to labels
        // corresponding to the possible subsequent emits. We do this for all
        // the regular emit nodes first, and then for the artificially-inserted
        // "exit" emit node at the end.
        let emit_nodes = iterate![
            ..flow_graph
                .emit_nodes
                .iter()
                .filter(|emit_node| !emit_node.is_entry() && !emit_node.is_exit()),
            &flow_graph[flow_graph.exit_emit_node_index()],
        ];
        for emit_node in emit_nodes {
            // All nodes other than entry node have a `label_ident`.
            debug_assert!(emit_node.label_ident.is_some());

            let emit_label_stmt = LabelStmt {
                label_ident: emit_node.label_ident.as_ref().unwrap().clone().into(),
            }
            .into();

            let assume_emit_path_stmt = CheckStmt {
                kind: CheckKind::Assume,
                attr: Some(CheckAttr::Partition),
                cond: BinOperExpr {
                    oper: CompareBinOper::Eq.into(),
                    lhs: CallExpr {
                        target: PreludeFnIdent::EmitPathPcField.into(),
                        arg_exprs: vec![pc_var_ident.into()],
                    }
                    .into(),
                    rhs: generate_emit_path_expr(emit_node.label_ident.as_ref().unwrap()).into(),
                }
                .into(),
            }
            .into();

            stmts.extend([emit_label_stmt, assume_emit_path_stmt]);

            if let Some(target) = emit_node.target {
                let op_item = &self.env[target];
                let op_ident = op_item.path.value.ident();

                let assign_op_stmt = AssignStmt {
                    lhs: VarIdent::Op.into(),
                    rhs: CallExpr {
                        target: IrMemberFnIdent {
                            ir_ident: bottom_ir_ident,
                            selector: IrMemberFnSelector::OpAt,
                        }
                        .into(),
                        arg_exprs: vec![pc_var_ident.into()],
                    }
                    .into(),
                }
                .into();

                let call_op_fn_stmt = CallExpr {
                    target: UserFnIdent {
                        parent_ident: Some(bottom_ir_ident.ident),
                        fn_ident: op_ident,
                    }
                    .into(),
                    arg_exprs: op_item
                        .param_order
                        .iter()
                        .filter_map(|param_index| match param_index {
                            flattener::ParamIndex::Var(var_param_index) => {
                                let var_param = &op_item.params[var_param_index];
                                match var_param.kind {
                                    VarParamKind::In | VarParamKind::Mut => {
                                        Some(var_param.ident.value)
                                    }
                                    VarParamKind::Out => None,
                                }
                            }
                            flattener::ParamIndex::Label(label_param_index) => {
                                Some(op_item.params[label_param_index].label.ident.value)
                            }
                        })
                        .map(|param_ident| {
                            CallExpr {
                                target: OpCtorFieldFnIdent {
                                    param_var_ident: UserParamVarIdent::from(param_ident).into(),
                                    op_ctor_ident: IrMemberFnIdent {
                                        ir_ident: bottom_ir_ident,
                                        selector: OpCtorIrMemberFnSelector {
                                            op_selector: UserOpSelector::from(op_ident).into(),
                                        }
                                        .into(),
                                    },
                                }
                                .into(),
                                arg_exprs: vec![VarIdent::Op.into()],
                            }
                            .into()
                        })
                        .collect(),
                }
                .into();

                let goto_succs_stmt = generate_emit_node_goto_succs_stmt(emit_node, flow_graph);

                stmts.extend([assign_op_stmt, call_op_fn_stmt, goto_succs_stmt]);
            }
        }

        // Tack on an additional local variable to hold the op being
        // interpreted.
        let mut local_vars = label_param_local_vars;
        local_vars.push(
            TypedVar {
                ident: VarIdent::Op,
                type_: IrMemberTypeIdent {
                    ir_ident: bottom_ir_ident,
                    selector: IrMemberTypeSelector::Op,
                }
                .into(),
            }
            .into(),
        );

        self.items.push(
            ProcItem {
                ident,
                attr: Some(ProcAttr::EntryPoint),
                param_vars,
                ret_vars: TypedVars::default(),
                body: Some(Body { local_vars, stmts }),
            }
            .into(),
        );
    }

    fn compile_params(
        &self,
        params: &flattener::Params,
        param_order: &[flattener::ParamIndex],
    ) -> (TypedVars, TypedVars) {
        let mut param_vars = TypedVars::default();
        let mut out_param_ret_vars = TypedVars::default();

        for param_index in param_order {
            match param_index {
                flattener::ParamIndex::Var(var_param_index) => {
                    let var_param = &params[var_param_index];
                    // Regular variable parameters become regular parameters;
                    // variable out-parameters become return variables.
                    match var_param.kind {
                        VarParamKind::In | VarParamKind::Mut => &mut param_vars,
                        VarParamKind::Out => &mut out_param_ret_vars,
                    }
                    .push(self.compile_var_param(var_param));
                }
                flattener::ParamIndex::Label(label_param_index) => {
                    param_vars.push(self.compile_label_param(&params[label_param_index]));
                }
            }
        }

        (param_vars, out_param_ret_vars)
    }

    fn compile_var_param(&self, var_param: &flattener::VarParam) -> TypedVar {
        TypedVar {
            ident: UserParamVarIdent::from(var_param.ident.value).into(),
            type_: self.get_type_ident(var_param.type_).into(),
        }
    }

    fn compile_label_param(&self, label_param: &flattener::LabelParam) -> TypedVar {
        // Both label parameters and out-parameters should be compiled to
        // regular Boogie parameters. Unlike with variable out-parameters, which
        // get translated to return variables, we don't actually *assign*
        // through label out-parameters, we just bind the label IDs passed in.
        TypedVar {
            ident: UserParamVarIdent::from(label_param.label.ident.value).into(),
            type_: PreludeTypeIdent::Label.into(),
        }
    }

    fn compile_body(
        &mut self,
        callable_index: flattener::CallableIndex,
        callable_item: &flattener::CallableItem,
        body: &flattener::Body,
    ) -> Body {
        let mut local_vars = self.compile_locals(&body.locals).collect();

        let mut scoped_compiler = ScopedCompiler::new(
            self,
            ItemContext::Callable {
                index: callable_index,
                item: callable_item,
            },
            &mut local_vars,
        );

        scoped_compiler.compile_stmts(&body.stmts);
        let stmts = scoped_compiler.stmts;

        Body { local_vars, stmts }
    }

    fn compile_locals<'b>(
        &'b self,
        locals: &'b flattener::Locals,
    ) -> impl 'b + Iterator<Item = LocalVar> + Captures<'a> {
        self.compile_local_vars(&locals.local_vars)
    }

    fn compile_local_vars<'b>(
        &'b self,
        local_vars: &'b TiSlice<LocalVarIndex, flattener::LocalVar>,
    ) -> impl 'b + Iterator<Item = LocalVar> + Captures<'a> {
        local_vars
            .iter_enumerated()
            .map(|(local_var_index, local_var)| self.compile_local_var(local_var_index, local_var))
    }

    fn compile_local_var(
        &self,
        local_var_index: LocalVarIndex,
        local_var: &flattener::LocalVar,
    ) -> LocalVar {
        TypedVar {
            ident: LocalVarIdent {
                ident: local_var.ident.value,
                index: local_var_index,
            }
            .into(),
            type_: self.get_type_ident(local_var.type_).into(),
        }
        .into()
    }

    fn get_type_ident(&self, type_index: flattener::TypeIndex) -> UserTypeIdent {
        let ident = match type_index {
            flattener::TypeIndex::BuiltIn(built_in_type) => built_in_type.ident(),
            flattener::TypeIndex::Enum(enum_index) => self.env[enum_index].ident.value,
            flattener::TypeIndex::Struct(struct_index) => self.env[struct_index].ident.value,
        };

        UserTypeIdent::from(ident)
    }
}

fn generate_emit_path_expr(emit_label_ident: &EmitLabelIdent) -> CallExpr {
    let nil_emit_path_ctor_expr = CallExpr {
        target: PreludeFnIdent::NilEmitPath.into(),
        arg_exprs: Vec::new(),
    };

    emit_label_ident
        .segments
        .iter()
        .fold(nil_emit_path_ctor_expr, |accum, emit_label_segment| {
            CallExpr {
                target: PreludeFnIdent::ConsEmitPath.into(),
                arg_exprs: vec![
                    accum.into(),
                    Literal::Int(emit_label_segment.local_emit_index.into()).into(),
                ],
            }
        })
}

fn generate_emit_node_goto_succs_stmt(emit_node: &EmitNode, flow_graph: &FlowGraph) -> Stmt {
    // Use a `BTreeSet` (as opposed to a `HashSet`) so the order of
    // the indexes is consistent for the same input every time the
    // compiler is run. The order of the indexes determines the
    // order of the labels in the goto statement generated below.
    let mut succ_emit_node_indexes = BTreeSet::new();
    for succ in &emit_node.succs {
        match succ {
            EmitSucc::Emit(emit_node_index) => {
                succ_emit_node_indexes.insert(*emit_node_index);
            }
            EmitSucc::Label(label_node_index) => {
                succ_emit_node_indexes
                    .extend(flow_graph[label_node_index].bound_to.iter().copied());
            }
        }
    }
    debug_assert!(!succ_emit_node_indexes.is_empty());

    // TODO(spinda): Elide goto on fallthrough.
    GotoStmt {
        labels: succ_emit_node_indexes
            .iter()
            .map(|succ_emit_node_index| {
                flow_graph[succ_emit_node_index]
                    .label_ident
                    .as_ref()
                    .unwrap()
                    .clone()
                    .into()
            })
            .collect(),
    }
    .into()
}

#[derive(Clone, Copy)]
enum CallableRepr {
    Fn,
    Proc,
}

impl CallableRepr {
    fn for_callable(callable_item: &flattener::CallableItem) -> Self {
        if let Some(callable_repr) = BLOCKED_PATHS.get(&callable_item.path.value) {
            return *callable_repr;
        };

        let has_out_params =
            callable_item
                .param_order
                .iter()
                .any(|param_index| match param_index {
                    flattener::ParamIndex::Var(var_param_index) => {
                        callable_item.params[var_param_index].kind == VarParamKind::Out
                    }
                    flattener::ParamIndex::Label(label_param_index) => {
                        callable_item.params[label_param_index].is_out
                    }
                });

        match (has_out_params, callable_item.ret, &callable_item.body) {
            (false, Some(_), None) => Self::Fn,
            _ => Self::Proc,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ItemContext<'b> {
    Callable {
        index: flattener::CallableIndex,
        item: &'b flattener::CallableItem,
    },
    Var,
}

impl<'b> ItemContext<'b> {
    fn param<I>(&self, index: I) -> &<flattener::Params as Index<I>>::Output
    where
        flattener::Params: Index<I>,
    {
        match self {
            ItemContext::Callable { item, .. } => &item.params[index],
            ItemContext::Var => {
                panic!("attempted to look up param in non-callable context");
            }
        }
    }

    fn local<I>(&self, index: I) -> &<flattener::Locals as Index<I>>::Output
    where
        flattener::Locals: Index<I>,
    {
        match self {
            ItemContext::Callable { item, .. } => &item.body.as_ref().unwrap().locals[index],
            ItemContext::Var => {
                panic!("attempted to look up local in non-callable context");
            }
        }
    }
}

struct ScopedCompiler<'a, 'b> {
    compiler: &'b mut Compiler<'a>,
    context: ItemContext<'b>,
    local_vars: &'b mut Vec<LocalVar>,
    next_local_emit_index: MaybeOwned<'b, LocalEmitIndex>,
    next_synthetic_var_indexes: MaybeOwned<'b, EnumMap<SyntheticVarKind, usize>>,
    stmts: Vec<Stmt>,
}

impl<'a> Deref for ScopedCompiler<'a, '_> {
    type Target = Compiler<'a>;

    fn deref(&self) -> &Self::Target {
        self.compiler
    }
}

impl<'a> DerefMut for ScopedCompiler<'a, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.compiler
    }
}

impl<'a, 'b> ScopedCompiler<'a, 'b> {
    fn new(
        compiler: &'b mut Compiler<'a>,
        context: ItemContext<'b>,
        local_vars: &'b mut Vec<LocalVar>,
    ) -> Self {
        ScopedCompiler {
            compiler,
            context,
            local_vars,
            next_local_emit_index: MaybeOwned::Owned(LocalEmitIndex::from(0)),
            next_synthetic_var_indexes: MaybeOwned::Owned(EnumMap::default()),
            stmts: Vec::new(),
        }
    }

    fn recurse<'c>(&'c mut self) -> ScopedCompiler<'a, 'c> {
        ScopedCompiler {
            compiler: self.compiler,
            context: self.context,
            local_vars: self.local_vars,
            next_local_emit_index: MaybeOwned::Borrowed(&mut self.next_local_emit_index),
            next_synthetic_var_indexes: MaybeOwned::Borrowed(&mut self.next_synthetic_var_indexes),
            stmts: Vec::new(),
        }
    }

    fn compile_args(&mut self, args: &[flattener::Arg]) -> (Vec<Expr>, Vec<VarIdent>) {
        let mut arg_exprs = Vec::new();
        let mut ret_var_idents = Vec::new();

        for arg in args {
            match arg {
                flattener::Arg::Expr(pure_expr) => {
                    arg_exprs.push(self.compile_expr_arg(pure_expr))
                }
                flattener::Arg::OutVar(out_var_arg) => {
                    ret_var_idents.push(self.compile_out_var_arg(out_var_arg))
                }
                flattener::Arg::Label(label_arg) => {
                    arg_exprs.push(self.compile_label_arg(label_arg))
                }
                flattener::Arg::LabelField(label_field_arg) => {
                    arg_exprs.push(self.compile_label_field_arg(label_field_arg))
                }
            }
        }

        (arg_exprs, ret_var_idents)
    }

    fn compile_expr_arg(&mut self, pure_expr: &flattener::PureExpr) -> Expr {
        self.compile_pure_expr(pure_expr)
    }

    fn compile_out_var_arg(&mut self, out_var_arg: &flattener::OutVarArg) -> VarIdent {
        // TODO(spinda): Handle out-parameter upcasting.
        self.get_var_ident(out_var_arg.var)
            .expect("invalid out-variable argument")
    }

    fn compile_label_arg(&mut self, label_arg: &flattener::LabelArg) -> Expr {
        self.compile_internal_label_arg(label_arg.label)
    }

    fn compile_label_field_arg(&mut self, label_field_arg: &flattener::LabelFieldArg) -> Expr {
        let struct_item = &self.env[label_field_arg.field.struct_index];
        let field = &struct_item.fields[label_field_arg.field.field_index];
        let field_fn_ident = TypeMemberFnIdent {
            type_ident: UserTypeIdent::from(struct_item.ident.value),
            selector: TypeMemberFnSelector::Field(field.ident().value.into()),
        }
        .into();

        let parent_expr = label_field_arg.parent.compile(self);

        CallExpr {
            target: field_fn_ident,
            arg_exprs: vec![parent_expr],
        }
        .into()
    }

    fn compile_internal_label_arg(&mut self, label_index: flattener::LabelIndex) -> Expr {
        match label_index {
            // As with compiling label parameters and out-parameters, we compile
            // label arguments and label out-arguments the same way when
            // generating Boogie.
            flattener::LabelIndex::Param(label_param_index) => {
                UserParamVarIdent::from(self.context.param(label_param_index).label.ident.value)
                    .into()
            }
            // Local labels don't actually need Boogie local variables: we can
            // simply identify them by index.
            flattener::LabelIndex::Local(local_label_index) => {
                let local_label = self.context.local(local_label_index);
                CallExpr {
                    target: PreludeFnIdent::Label.into(),
                    arg_exprs: vec![
                        ParamVarIdent::Pc.into(),
                        CommentedExpr {
                            comment: local_label.ident.value.into(),
                            expr: Literal::Int(local_label_index.into()).into(),
                        }
                        .into(),
                    ],
                }
                .into()
            }
        }
    }

    fn compile_invocation(
        &mut self,
        invoke_expr: &flattener::InvokeExpr,
        generate_proc_ret_var_ident: impl FnOnce(&mut Self, flattener::TypeIndex) -> VarIdent,
    ) -> CompiledInvocation {
        let callable_item = &self.env[invoke_expr.call.target];

        let callable_parent_ident = callable_item.path.value.parent().map(Path::ident);
        let callable_ident = callable_item.path.value.ident();
        let target = UserFnIdent {
            parent_ident: callable_parent_ident,
            fn_ident: callable_ident,
        }
        .into();

        let (mut arg_exprs, mut ret_var_idents) = self.compile_args(&invoke_expr.call.args);

        if callable_item.emits.is_some() {
            let local_emit_index = *self.next_local_emit_index;
            *self.next_local_emit_index = LocalEmitIndex::from(usize::from(local_emit_index) + 1);

            let pc_expr = CallExpr {
                target: PreludeFnIdent::ConsPcEmitPath.into(),
                arg_exprs: vec![
                    ParamVarIdent::Pc.into(),
                    Literal::Int(local_emit_index.into()).into(),
                ],
            }
            .into();

            arg_exprs.push(pc_expr);
        }

        let call_expr = CallExpr { target, arg_exprs };

        match CallableRepr::for_callable(callable_item) {
            CallableRepr::Fn => CompiledInvocation::FnCall(call_expr),
            CallableRepr::Proc => {
                let ret_var_ident = match callable_item.ret {
                    Some(ret) => {
                        let ret_var_ident = generate_proc_ret_var_ident(self, ret);
                        ret_var_idents.push(ret_var_ident);
                        ret_var_ident
                    }
                    None => built_in_var_ident(BuiltInVar::Unit).into(),
                };

                self.stmts.push(
                    CallStmt {
                        call: call_expr,
                        ret_var_idents,
                    }
                    .into(),
                );

                CompiledInvocation::ProcCall(ret_var_ident)
            }
        }
    }

    fn compile_block(&mut self, stmts: &[flattener::Stmt]) -> Block {
        let mut scoped_compiler = self.recurse();
        scoped_compiler.compile_stmts(stmts);
        scoped_compiler.stmts.into()
    }

    fn compile_stmts(&mut self, stmts: &[flattener::Stmt]) {
        self.stmts.reserve(stmts.len());
        for stmt in stmts {
            self.compile_stmt(stmt);
        }
    }

    fn compile_stmt(&mut self, stmt: &flattener::Stmt) {
        match stmt {
            flattener::Stmt::Let(let_stmt) => self.compile_let_stmt(let_stmt),
            flattener::Stmt::Label(_) => (),
            flattener::Stmt::If(if_stmt) => self.compile_if_stmt(if_stmt),
            flattener::Stmt::ForIn(for_in_stmt) => self.compile_for_in_stmt(for_in_stmt),
            flattener::Stmt::Check(check_stmt) => self.compile_check_stmt(check_stmt),
            flattener::Stmt::Goto(goto_stmt) => self.compile_goto_stmt(goto_stmt),
            flattener::Stmt::Bind(bind_stmt) => self.compile_bind_stmt(bind_stmt),
            flattener::Stmt::Emit(call) => self.compile_emit_stmt(call),
            flattener::Stmt::Invoke(invoke_stmt) => self.compile_invoke_stmt(invoke_stmt),
            flattener::Stmt::Assign(assign_stmt) => self.compile_assign_stmt(assign_stmt),
            flattener::Stmt::Ret(ret_stmt) => self.compile_ret_stmt(ret_stmt),
            flattener::Stmt::Unreachable => self.compile_unreachable_stmt(),
        }
    }

    /// `unreachable` is a claim to be discharged, so it lowers to the obligation
    /// plus a return. The return carries no value: a value-returning
    /// procedure's `ret` is left unconstrained, which is sound because the only
    /// path reaching it is the one the assertion rules out.
    fn compile_unreachable_stmt(&mut self) {
        self.stmts.extend([
            CheckStmt {
                kind: CheckKind::Assert,
                attr: None,
                cond: false.into(),
            }
            .into(),
            Stmt::Ret,
        ]);
    }

    fn compile_let_stmt(&mut self, let_stmt: &flattener::LetStmt) {
        // Local variables are pre-declared at the top of the block in Boogie,
        // so no need to insert a separate assign statement if there's nothing
        // to assign. This comes up in, e.g., compiling `out let foo` arguments.
        let Some(rhs) = let_stmt.rhs.as_ref() else {
            return;
        };

        let lhs_var_ident = LocalVarIdent {
            ident: self.context.local(let_stmt.lhs).ident.value,
            index: let_stmt.lhs,
        };

        match rhs {
            flattener::Expr::Invoke(invoke_expr) => {
                // Same optimization as in `compile_assign_stmt`.
                self.compile_invoke_stmt_impl(invoke_expr, |_, _| lhs_var_ident.into());
            }
            _ => {
                let rhs = self.compile_expr(rhs);

                self.stmts.push(
                    AssignStmt {
                        lhs: lhs_var_ident.into(),
                        rhs,
                    }
                    .into(),
                );
            }
        }
    }

    fn compile_if_stmt_recurse(&mut self, if_stmt: &flattener::IfStmt) -> IfStmt {
        let cond = self.compile_expr(&if_stmt.cond);

        let then = self.compile_block(&if_stmt.then);

        let else_ = if_stmt.else_.as_ref().map(|else_| match else_ {
            flattener::ElseClause::ElseIf(else_if) => {
                ast::ElseClause::ElseIf(Box::new(self.compile_if_stmt_recurse(&*else_if)))
            }
            flattener::ElseClause::Else(else_block) => {
                ast::ElseClause::Else(self.compile_block(else_block))
            }
        });

        IfStmt { cond, then, else_ }
    }

    fn compile_if_stmt(&mut self, if_stmt: &flattener::IfStmt) {
        let if_ = self.compile_if_stmt_recurse(if_stmt);
        self.stmts.push(if_.into());
    }

    fn compile_for_in_stmt(&mut self, for_in_stmt: &flattener::ForInStmt) {
        let enum_item = &self.env[for_in_stmt.target];

        let body = self.compile_block(&for_in_stmt.body);

        let compile_iteration = |variant_path: &Spanned<Path>| {
            let loop_var_expr = Expr::from(self.get_local_var_ident(for_in_stmt.var));

            let variant_expr: Expr = CallExpr {
                target: TypeMemberFnIdent {
                    type_ident: enum_item.ident.value.into(),
                    selector: VariantCtorTypeMemberFnSelector::from(variant_path.value.ident())
                        .into(),
                }
                .into(),
                arg_exprs: vec![],
            }
            .into();

            let loop_var_update: Stmt = AssignStmt {
                lhs: loop_var_expr,
                rhs: variant_expr,
            }
            .into();

            let body_stmts = body.stmts.iter().cloned();
            iterate![loop_var_update, ..body_stmts]
        };

        let stmts: Vec<_> = match for_in_stmt.order {
            ForInOrder::Ascending => enum_item
                .variants
                .iter()
                .flat_map(compile_iteration)
                .collect(),
            ForInOrder::Descending => enum_item
                .variants
                .iter()
                .rev()
                .flat_map(compile_iteration)
                .collect(),
        };
        self.stmts.extend(stmts);
    }

    fn compile_check_stmt(&mut self, check_stmt: &flattener::CheckStmt) {
        let cond = self.compile_expr(&check_stmt.cond);

        self.stmts.push(
            CheckStmt {
                kind: check_stmt.kind,
                attr: None,
                cond,
            }
            .into(),
        );
    }

    fn compile_goto_stmt(&mut self, goto_stmt: &flattener::GotoStmt) {
        let ir_ident = self.env[goto_stmt.ir].ident.value.into();
        let target = IrMemberFnIdent {
            ir_ident,
            selector: IrMemberFnSelector::Goto,
        }
        .into();

        let arg_exprs = vec![self.compile_internal_label_arg(goto_stmt.label)];

        self.stmts
            .extend([CallExpr { target, arg_exprs }.into(), Stmt::Ret]);
    }

    fn compile_bind_stmt(&mut self, bind_stmt: &flattener::BindStmt) {
        let ir_ident = self.env[bind_stmt.ir].ident.value.into();
        let target = IrMemberFnIdent {
            ir_ident,
            selector: IrMemberFnSelector::Bind,
        }
        .into();

        let arg_exprs = vec![self.compile_internal_label_arg(bind_stmt.label)];

        self.stmts.extend([CallExpr { target, arg_exprs }.into()]);
    }

    fn compile_emit_stmt(&mut self, emit_stmt: &flattener::EmitStmt) {
        let ir_item = &self.env[emit_stmt.ir];

        let ir_ident = IrIdent::from(ir_item.ident.value);
        let op_ident = self.env[emit_stmt.call.target].path.value.ident();

        let (mut op_arg_exprs, _) = self.compile_args(&emit_stmt.call.args);

        let local_emit_index = *self.next_local_emit_index;
        *self.next_local_emit_index = LocalEmitIndex::from(usize::from(local_emit_index) + 1);

        let pc_expr = CallExpr {
            target: PreludeFnIdent::ConsPcEmitPath.into(),
            arg_exprs: vec![
                ParamVarIdent::Pc.into(),
                Literal::Int(local_emit_index.into()).into(),
            ],
        }
        .into();

        match ir_item.emits {
            Some(_) => {
                // If the IR of the emitted op describes a compiler, call
                // directly into the other op.

                let target = UserFnIdent {
                    parent_ident: Some(ir_ident.ident),
                    fn_ident: op_ident,
                }
                .into();

                op_arg_exprs.push(pc_expr);

                self.stmts.push(
                    CallExpr {
                        target,
                        arg_exprs: op_arg_exprs,
                    }
                    .into(),
                );
            }
            None => {
                // Otherwise, build the op via the corresponding datatype
                // constructor and pass it to the emit helper function.

                let op_ctor_target = IrMemberFnIdent {
                    ir_ident,
                    selector: OpCtorIrMemberFnSelector {
                        op_selector: UserOpSelector::from(op_ident).into(),
                    }
                    .into(),
                }
                .into();
                let op_ctor_call_expr = CallExpr {
                    target: op_ctor_target,
                    arg_exprs: op_arg_exprs,
                }
                .into();

                let emit_target = IrMemberFnIdent {
                    ir_ident,
                    selector: IrMemberFnSelector::Emit,
                }
                .into();

                let emit_args = vec![op_ctor_call_expr, pc_expr];

                self.stmts.push(
                    CallExpr {
                        target: emit_target,
                        arg_exprs: emit_args,
                    }
                    .into(),
                );
            }
        }
    }

    fn compile_invoke_stmt(&mut self, invoke_stmt: &flattener::InvokeStmt) {
        self.compile_invoke_stmt_impl(invoke_stmt, |scoped_compiler: &mut Self, ret| {
            scoped_compiler
                .generate_synthetic_var(
                    SyntheticVarKind::Out,
                    scoped_compiler.get_type_ident(ret).into(),
                )
                .into()
        })
    }

    fn compile_invoke_stmt_impl(
        &mut self,
        invoke_stmt: &flattener::InvokeStmt,
        generate_ret_var_ident: impl Fn(&mut Self, flattener::TypeIndex) -> VarIdent,
    ) {
        match self.compile_invocation(invoke_stmt, &generate_ret_var_ident) {
            CompiledInvocation::FnCall(call_expr) => {
                // The invocation compiles down to a Boogie function call. We can't
                // directly insert this as a statement, but we can wrap it up in an
                // assign statement and insert that.

                let callable_item = &self.env[invoke_stmt.call.target];

                let lhs = generate_ret_var_ident(self, callable_item.type_()).into();

                self.stmts.push(
                    AssignStmt {
                        lhs,
                        rhs: call_expr.into(),
                    }
                    .into(),
                );
            }
            CompiledInvocation::ProcCall(_) => (),
        }
    }

    fn compile_assign_stmt(&mut self, assign_stmt: &flattener::AssignStmt) {
        let lhs = self.compile_var_access(assign_stmt.lhs);

        match (lhs, &assign_stmt.rhs) {
            (Expr::Var(lhs_var_ident), flattener::Expr::Invoke(invoke_expr)) => {
                // If the assignment is to the return value of an invocation,
                // use the LHS variable directly as the return variable of any
                // call statement.
                self.compile_invoke_stmt_impl(invoke_expr, |_, _| lhs_var_ident);
            }
            (lhs, _) => {
                let rhs = self.compile_expr(&assign_stmt.rhs);

                self.stmts.push(AssignStmt { lhs, rhs }.into());
            }
        }
    }

    fn compile_ret_stmt(&mut self, ret_stmt: &flattener::RetStmt) {
        let assign_stmt = ret_stmt.value.as_ref().and_then(|value| {
            let rhs = match value {
                flattener::Expr::Invoke(invoke_expr) => {
                    // If the return value is an invoke expression, we can have
                    // it assign directly to the containing function's return
                    // variable when the invoke expression compiles down to a
                    // Boogie procedure call.
                    match self.compile_invocation(invoke_expr, |_, _| VarIdent::Ret) {
                        // If the result is a Boogie function call, we still
                        // need to handle the assignment ourselves.
                        CompiledInvocation::FnCall(call_expr) => call_expr.into(),
                        CompiledInvocation::ProcCall(_) => return None,
                    }
                }
                value => self.compile_expr(value),
            };

            Some(
                AssignStmt {
                    lhs: VarIdent::Ret.into(),
                    rhs,
                }
                .into(),
            )
        });

        let step_call = match self.context {
            ItemContext::Callable {
                index: flattener::CallableIndex::Op(_),
                item:
                    &flattener::CallableItem {
                        interprets: Some(ir_index),
                        ..
                    },
            } => {
                let ir_ident = self.env[ir_index].ident.value.into();

                Some(
                    CallExpr {
                        target: IrMemberFnIdent {
                            ir_ident,
                            selector: IrMemberFnSelector::Step,
                        }
                        .into(),
                        arg_exprs: Vec::new(),
                    }
                    .into(),
                )
            }
            _ => None,
        };

        self.stmts
            .extend(iterate![..assign_stmt, ..step_call, Stmt::Ret]);
    }

    fn compile_expr(&mut self, expr: &flattener::Expr) -> Expr {
        match expr {
            flattener::Expr::Block(void, _) => unreachable(*void),
            flattener::Expr::Literal(literal) => compile_literal(*literal).into(),
            flattener::Expr::Var(var_expr) => self.compile_var_expr(var_expr),
            flattener::Expr::Invoke(invoke_expr) => self.compile_invoke_expr(invoke_expr),
            flattener::Expr::FieldAccess(field_access_expr) => {
                self.compile_field_access_expr(&field_access_expr).into()
            }
            flattener::Expr::Negate(negate_expr) => self.compile_negate_expr(&negate_expr),
            flattener::Expr::Cast(cast_expr) => self.compile_cast_expr(&cast_expr).into(),
            flattener::Expr::BinOper(bin_oper_expr) => {
                self.compile_bin_oper_expr(&bin_oper_expr).into()
            }
        }
    }

    fn compile_pure_expr(&mut self, pure_expr: &flattener::PureExpr) -> Expr {
        match pure_expr {
            flattener::PureExpr::Literal(literal) => compile_literal(*literal).into(),
            flattener::PureExpr::Var(var_expr) => self.compile_var_expr(var_expr),
            flattener::PureExpr::FieldAccess(field_access_expr) => {
                self.compile_field_access_expr(&field_access_expr).into()
            }
            flattener::PureExpr::Negate(negate_expr) => self.compile_negate_expr(&negate_expr),
            flattener::PureExpr::Cast(cast_expr) => self.compile_cast_expr(&cast_expr).into(),
            flattener::PureExpr::BinOper(bin_oper_expr) => {
                self.compile_bin_oper_expr(&bin_oper_expr).into()
            }
        }
    }

    fn compile_var_expr(&self, var_expr: &flattener::VarExpr) -> Expr {
        self.compile_var_access(var_expr.var)
    }

    fn compile_var_access(&self, var_index: flattener::VarIndex) -> Expr {
        match var_index {
            flattener::VarIndex::BuiltIn(BuiltInVar::True) => true.into(),
            flattener::VarIndex::BuiltIn(BuiltInVar::False) => false.into(),
            flattener::VarIndex::EnumVariant(enum_variant_index) => {
                let enum_item = &self.env[enum_variant_index.enum_index];
                let variant_path = enum_item[enum_variant_index.variant_index];

                CallExpr {
                    target: TypeMemberFnIdent {
                        type_ident: enum_item.ident.value.into(),
                        selector: VariantCtorTypeMemberFnSelector::from(
                            variant_path.value.ident(),
                        )
                        .into(),
                    }
                    .into(),
                    arg_exprs: vec![],
                }
                .into()
            }

            // Global variables with values are modeled as functions so we must emit a call
            // expression rather than a var expression.
            flattener::VarIndex::Global(global_var_index)
                if self.env[global_var_index].value.is_some() =>
            {
                let global_var_item = &self.env[global_var_index];
                let parent_ident = global_var_item
                    .path
                    .value
                    .parent()
                    .map(|parent_path| parent_path.ident());
                let var_ident = global_var_item.path.value.ident();

                CallExpr {
                    target: UserFnIdent {
                        parent_ident,
                        fn_ident: var_ident,
                    }
                    .into(),
                    arg_exprs: vec![],
                }
                .into()
            }
            var_index => self.get_var_ident(var_index).unwrap().into(),
        }
    }

    fn compile_invoke_expr(&mut self, invoke_expr: &flattener::InvokeExpr) -> Expr {
        let compiled_invocation = self.compile_invocation(invoke_expr, |scoped_compiler, ret| {
            scoped_compiler
                .generate_synthetic_var(
                    SyntheticVarKind::Out,
                    scoped_compiler.get_type_ident(ret).into(),
                )
                .into()
        });

        match compiled_invocation {
            CompiledInvocation::FnCall(call_expr) => call_expr.into(),
            CompiledInvocation::ProcCall(ret_var_ident) => ret_var_ident.into(),
        }
    }

    fn compile_field_access_expr<E: CompileExpr + Typed>(
        &mut self,
        field_access_expr: &flattener::FieldAccessExpr<E>,
    ) -> CallExpr {
        let struct_item = &self.env[field_access_expr.field.struct_index];
        let field = &struct_item.fields[field_access_expr.field.field_index];
        let field_fn_ident = TypeMemberFnIdent {
            type_ident: UserTypeIdent::from(struct_item.ident.value),
            selector: TypeMemberFnSelector::Field(field.ident().value.into()),
        }
        .into();

        let parent_expr = field_access_expr.parent.compile(self);

        CallExpr {
            target: field_fn_ident,
            arg_exprs: vec![parent_expr],
        }
    }

    fn compile_negate_expr<E: CompileExpr + Typed>(
        &mut self,
        negate_expr: &flattener::NegateExpr<E>,
    ) -> Expr {
        let expr = negate_expr.expr.compile(self);

        match negate_expr.kind {
            NegateKind::Arith | NegateKind::Bitwise => {
                let type_ident = self.get_type_ident(negate_expr.type_()).into();
                let selector: NegateTypeMemberFnSelector = negate_expr.kind.into();
                CallExpr {
                    target: TypeMemberFnIdent {
                        type_ident,
                        selector: selector.into(),
                    }
                    .into(),
                    arg_exprs: vec![expr],
                }
                .into()
            }
            kind => NegateExpr { kind, expr }.into(),
        }
    }

    fn compile_cast_expr<E: CompileExpr + Typed>(
        &mut self,
        cast_expr: &flattener::CastExpr<E>,
    ) -> Expr {
        let expr = cast_expr.expr.compile(self);

        let source_type_index = cast_expr.expr.type_();
        let target_type_index = cast_expr.type_;

        if let (flattener::TypeIndex::Struct(_), flattener::TypeIndex::Struct(_)) =
            (source_type_index, target_type_index)
        {
            // Casts between struct types should be no-ops on the Boogie side,
            // owing to the way types with supertypes get compiled.
            return expr;
        }

        let source_type_ident = self.get_type_ident(source_type_index);
        let target_type_ident = self.get_type_ident(target_type_index);

        CallExpr {
            target: TypeMemberFnIdent {
                type_ident: source_type_ident,
                selector: CastTypeMemberFnSelector { target_type_ident }.into(),
            }
            .into(),
            arg_exprs: vec![expr],
        }
        .into()
    }

    fn compile_bin_oper_expr(&mut self, bin_oper_expr: &flattener::BinOperExpr) -> Expr {
        let lhs = self.compile_pure_expr(&bin_oper_expr.lhs);
        let rhs = self.compile_pure_expr(&bin_oper_expr.rhs);

        let selector = TypeMemberFnSelector::BinOper(match bin_oper_expr.oper {
            BinOper::Arith(arith_bin_oper) => arith_bin_oper.into(),
            BinOper::Bitwise(bitwise_bin_oper) => bitwise_bin_oper.into(),
            BinOper::Compare(compare_bin_oper) => match compare_bin_oper {
                CompareBinOper::Numeric(numeric_compare_bin_oper) => {
                    numeric_compare_bin_oper.into()
                }
                CompareBinOper::Eq | CompareBinOper::Neq => {
                    return BinOperExpr {
                        oper: compare_bin_oper.into(),
                        lhs,
                        rhs,
                    }
                    .into();
                }
            },
            BinOper::Logical(logical_bin_oper) => {
                return BinOperExpr {
                    oper: logical_bin_oper.into(),
                    lhs,
                    rhs,
                }
                .into();
            }
        });

        // Note that the type prefix for the helper function is the *operand*
        // type, not the output type, so we take the type of the left-hand side
        // rather than the operator expression itself.
        let lhs_type = bin_oper_expr.lhs.type_();
        let rhs_type = bin_oper_expr.rhs.type_();
        debug_assert_eq!(lhs_type, rhs_type);
        let type_ident = self.get_type_ident(lhs_type).into();

        CallExpr {
            target: TypeMemberFnIdent {
                type_ident,
                selector,
            }
            .into(),
            arg_exprs: vec![lhs, rhs],
        }
        .into()
    }

    fn generate_synthetic_var(
        &mut self,
        kind: SyntheticVarKind,
        type_: Type,
    ) -> SyntheticVarIdent {
        let index = &mut self.next_synthetic_var_indexes[kind];
        let ident = SyntheticVarIdent {
            kind,
            index: *index,
        };
        *index += 1;

        self.local_vars.push(
            TypedVar {
                ident: ident.into(),
                type_,
            }
            .into(),
        );

        ident
    }

    fn get_var_ident(&self, var_index: flattener::VarIndex) -> Option<VarIdent> {
        match var_index {
            flattener::VarIndex::BuiltIn(built_in_var) => {
                Some(built_in_var_ident(built_in_var).into())
            }
            flattener::VarIndex::EnumVariant(_) => None,
            flattener::VarIndex::Global(global_var_index) => {
                let global_var_item = &self.env[global_var_index];
                Some(
                    UserGlobalVarIdent {
                        parent_ident: global_var_item.path.value.parent().map(Path::ident),
                        var_ident: global_var_item.path.value.ident().into(),
                    }
                    .into(),
                )
            }
            flattener::VarIndex::Param(var_param_index) => {
                let var_param = &self.context.param(var_param_index);
                Some(UserParamVarIdent::from(var_param.ident.value).into())
            }
            flattener::VarIndex::Local(local_var_index) => {
                Some(self.get_local_var_ident(local_var_index))
            }
        }
    }

    fn get_local_var_ident(&self, local_var_index: flattener::LocalVarIndex) -> VarIdent {
        let local_var = &self.context.local(local_var_index);
        LocalVarIdent {
            ident: local_var.ident.value,
            index: local_var_index,
        }
        .into()
    }
}

fn compile_literal(literal: flattener::Literal) -> Literal {
    match literal {
        flattener::Literal::Int8(n) => Literal::Bv8(n as u8),
        flattener::Literal::Int16(n) => Literal::Bv16(n as u16),
        flattener::Literal::Int32(n) => Literal::Bv32(n as u32),
        flattener::Literal::Int64(n) => Literal::Bv64(n as u64),
        flattener::Literal::UInt8(n) => Literal::Bv8(n),
        flattener::Literal::UInt16(n) => Literal::Bv16(n),
        flattener::Literal::UInt32(n) => Literal::Bv32(n),
        flattener::Literal::UInt64(n) => Literal::Bv64(n),
        flattener::Literal::Double(n) => Literal::Float64(n),
    }
}

fn built_in_var_ident(built_in_var: BuiltInVar) -> UserGlobalVarIdent {
    UserGlobalVarIdent {
        parent_ident: None,
        var_ident: built_in_var.ident(),
    }
}

trait CompileExpr {
    fn compile<'a, 'b>(&self, scoped_compiler: &mut ScopedCompiler<'a, 'b>) -> Expr;
}

impl CompileExpr for flattener::Expr {
    fn compile<'a, 'b>(&self, scoped_compiler: &mut ScopedCompiler<'a, 'b>) -> Expr {
        scoped_compiler.compile_expr(self)
    }
}

impl CompileExpr for flattener::PureExpr {
    fn compile<'a, 'b>(&self, scoped_compiler: &mut ScopedCompiler<'a, 'b>) -> Expr {
        scoped_compiler.compile_pure_expr(self)
    }
}

enum CompiledInvocation {
    FnCall(CallExpr),
    ProcCall(VarIdent),
}

lazy_static! {
    // Temporary hack to implement a few functions in Boogie, until we implement
    // support for conditional compilation and polymorphism in Cachet.
    static ref BLOCKED_PATHS: HashMap<Path, CallableRepr> = {
        use CallableRepr::*;
        HashMap::from([
            (Ident::from("ValueReg").into(), Proc),
            (Ident::from("ValueReg").nest("scratchReg".into()), Proc),
            (Ident::from("GeneralRegSet").nest("empty".into()), Proc),
            (Ident::from("GeneralRegSet").nest("volatile".into()), Proc),
            (Ident::from("GeneralRegSet").nest("intersect".into()), Proc),
            (Ident::from("GeneralRegSet").nest("difference".into()), Proc),
            (Ident::from("GeneralRegSet").nest("contains".into()), Fn),
            (Ident::from("GeneralRegSet").nest("add".into()), Proc),
            (Ident::from("GeneralRegSet").nest("take".into()), Proc),
            (Ident::from("FloatRegSet").nest("empty".into()), Proc),
            (Ident::from("FloatRegSet").nest("volatile".into()), Proc),
            (Ident::from("FloatRegSet").nest("intersect".into()), Proc),
            (Ident::from("FloatRegSet").nest("difference".into()), Proc),
            (Ident::from("FloatRegSet").nest("contains".into()), Fn),
            (Ident::from("FloatRegSet").nest("add".into()), Proc),
            (Ident::from("FloatRegSet").nest("take".into()), Proc),
            (Ident::from("MoveResolver").nest("reset".into()), Proc),
            (Ident::from("MoveResolver").nest("addRegMove".into()), Proc),
            (Ident::from("MoveResolver").nest("addFloatMove".into()), Proc),
            (Ident::from("MoveResolver").nest("resolve".into()), Proc),
            (Ident::from("ABIFunction").nest("clobberVolatileRegs".into()), Proc),
            (Ident::from("MASM").nest("getValue".into()), Proc),
            (Ident::from("MASM").nest("setValue".into()), Proc),
            (Ident::from("MASM").nest("getData".into()), Proc),
            (Ident::from("MASM").nest("setData".into()), Proc),
            (Ident::from("MASM").nest("getFloatData".into()), Proc),
            (Ident::from("MASM").nest("setFloatData".into()), Proc),
            (Ident::from("MASM").nest("stackPush".into()), Proc),
            (Ident::from("MASM").nest("stackPop".into()), Proc),
            (Ident::from("MASM").nest("stackStore".into()), Proc),
            (Ident::from("MASM").nest("stackLoad".into()), Proc),
            (Ident::from("MASM").nest("stackPushLiveGeneralReg".into()), Proc),
            (Ident::from("MASM").nest("stackPopLiveGeneralReg".into()), Proc),
            (Ident::from("MASM").nest("stackPushLiveFloatReg".into()), Proc),
            (Ident::from("MASM").nest("stackPopLiveFloatReg".into()), Proc),
            (Ident::from("CacheIR").nest("hasAvailableReg".into()), Proc),
            (Ident::from("CacheIR").nest("isAllocatedValueReg".into()), Proc),
            (Ident::from("CacheIR").nest("isAllocatedReg".into()), Proc),
            (Ident::from("CacheIR").nest("allocateValueReg".into()), Proc),
            (Ident::from("CacheIR").nest("releaseValueReg".into()), Proc),
            (Ident::from("CacheIR").nest("allocateReg".into()), Proc),
            (Ident::from("CacheIR").nest("allocateKnownReg".into()), Proc),
            (Ident::from("CacheIR").nest("releaseReg".into()), Proc),
            (Ident::from("CacheIR").nest("allocateAvailableFloatRegUnchecked".into()), Proc),
            (Ident::from("CacheIR").nest("releaseFloatReg".into()), Proc),
            (Ident::from("CacheIR").nest("defineTypedId".into()), Proc),
            (Ident::from("CacheIR").nest("defineValueId".into()), Proc),
            (Ident::from("CacheIR").nest("getOperandLocation".into()), Proc),
            (Ident::from("CacheIR").nest("setOperandLocation".into()), Proc),
            (Ident::from("initRegState").into(), Proc),
            (Ident::from("initOperandId").into(), Proc),
            (Ident::from("initInputValueId").into(), Proc),
            (Ident::from("addLiveFloatReg").into(), Proc),
        ])
    };
}
