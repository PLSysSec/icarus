// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::collections::{HashMap, HashSet};
use std::iter;
use std::ops::ControlFlow::{self, Break, Continue};

use derive_more::From;
use typed_index_collections::{TiSlice, TiVec};

use cachet_lang::flattener;
use cachet_util::{MaybeOwned, typed_field_index};

use crate::bpl::ast::*;

#[derive(Clone, Debug, Default)]
pub struct FlowGraph {
    pub emit_nodes: TiVec<EmitNodeIndex, EmitNode>,
    pub label_nodes: TiVec<LabelNodeIndex, LabelNode>,
}

impl FlowGraph {
    pub fn exit_emit_node_index(&self) -> EmitNodeIndex {
        let exit_emit_node_index = self.emit_nodes.first_key().expect("missing exit emit node");
        debug_assert!(
            self[exit_emit_node_index].is_exit(),
            "first emit node does not correspond to an exit point"
        );
        exit_emit_node_index
    }

    pub fn entry_emit_node_index(&self) -> EmitNodeIndex {
        let entry_emit_node_index =
            EmitNodeIndex::from(1 + usize::from(self.exit_emit_node_index()));
        self.emit_nodes
            .get(entry_emit_node_index)
            .expect("missing entry emit node");
        debug_assert!(
            self[entry_emit_node_index].is_entry(),
            "exit emit node is not followed by an entry emit node"
        );
        entry_emit_node_index
    }

    fn exit_label_node_index(&self) -> LabelNodeIndex {
        let exit_label_node_index = self
            .label_nodes
            .first_key()
            .expect("missing exit label node");
        debug_assert!(
            self[exit_label_node_index]
                .bound_to
                .contains(&self.exit_emit_node_index()),
            "exit label node not bound to the exit emit node"
        );
        debug_assert!(
            self[exit_label_node_index].bound_to.len() == 1,
            "exit label node bound to more than one emit node"
        );
        exit_label_node_index
    }

    fn link_label_to_emit(
        &mut self,
        label_node_index: LabelNodeIndex,
        emit_node_index: EmitNodeIndex,
    ) {
        self[label_node_index].bound_to.insert(emit_node_index);
    }
}

typed_field_index!(FlowGraph:emit_nodes[pub EmitNodeIndex] => EmitNode);
typed_field_index!(FlowGraph:label_nodes[pub LabelNodeIndex] => LabelNode);

#[derive(Clone, Debug)]
pub struct EmitNode {
    pub label_ident: Option<EmitLabelIdent>,
    pub succs: HashSet<EmitSucc>,
    pub target: Option<flattener::CallableIndex>,
}

impl EmitNode {
    pub fn is_exit(&self) -> bool {
        self.label_ident.is_some() && self.target.is_none()
    }

    pub fn is_entry(&self) -> bool {
        self.label_ident.is_none()
    }
}

#[derive(Clone, Copy, Debug, Eq, From, Hash, PartialEq)]
pub enum EmitSucc {
    Emit(EmitNodeIndex),
    Label(LabelNodeIndex),
}

pub type LabelScope = HashMap<flattener::LabelIndex, LabelNodeIndex>;

#[derive(Clone, Debug, Default)]
pub struct LabelNode {
    pub bound_to: HashSet<EmitNodeIndex>,
}

pub fn trace_entry_point(
    env: &flattener::Env,
    top_op_item: &flattener::CallableItem,
    bottom_ir_index: flattener::IrIndex,
) -> FlowGraph {
    let mut flow_tracer = FlowTracer::new(env, bottom_ir_index);
    flow_tracer.insert_exit_emit_node();
    flow_tracer.insert_exit_label_node();
    flow_tracer.insert_entry_emit_node();
    flow_tracer.init_top_label_params(&top_op_item.params, &top_op_item.param_order);
    flow_tracer.trace_callable_item(top_op_item);
    flow_tracer.link_exit_emit_node();

    let flow_graph = flow_tracer.graph.into_owned();
    debug_assert_eq!(
        flow_graph
            .emit_nodes
            .iter()
            .filter(|emit_node| emit_node.is_exit())
            .count(),
        1,
        "wrong number of exit emit nodes"
    );
    debug_assert_eq!(
        flow_graph
            .emit_nodes
            .iter()
            .filter(|emit_node| emit_node.is_entry())
            .count(),
        1,
        "wrong number of entry emit nodes"
    );
    debug_assert_eq!(
        flow_graph
            .emit_nodes
            .iter_enumerated()
            .find(|(_, emit_node)| emit_node.succs.is_empty() && !emit_node.is_exit())
            .map(|(emit_node_index, _)| emit_node_index),
        None,
        "non-exit emit node has no successors"
    );
    debug_assert_eq!(
        flow_graph
            .label_nodes
            .iter_enumerated()
            .find(|(_, label_node)| label_node.bound_to.is_empty())
            .map(|(label_node_index, _)| label_node_index),
        None,
        "unbound label node",
    );
    flow_graph
}

#[derive(Clone, Default)]
struct FlowState {
    pred_emits: HashSet<EmitNodeIndex>,
    bound_labels: HashSet<LabelNodeIndex>,
}

impl FlowState {
    fn is_empty(&self) -> bool {
        self.pred_emits.is_empty() && self.bound_labels.is_empty()
    }

    fn clear(&mut self) {
        self.pred_emits.clear();
        self.bound_labels.clear();
    }

    fn append(&mut self, other: &mut FlowState) {
        self.pred_emits.extend(other.pred_emits.drain());
        self.bound_labels.extend(other.bound_labels.drain());
    }
}

struct FlowTracer<'a> {
    env: &'a flattener::Env,
    bottom_ir_index: flattener::IrIndex,
    graph: MaybeOwned<'a, FlowGraph>,
    curr_emit_label_ident: &'a EmitLabelIdent,
    next_local_emit_index: MaybeOwned<'a, LocalEmitIndex>,
    label_scope: MaybeOwned<'a, LabelScope>,
    curr_state: MaybeOwned<'a, FlowState>,
    exit_state: MaybeOwned<'a, FlowState>,
}

impl<'a> FlowTracer<'a> {
    fn new(env: &'a flattener::Env, bottom_ir_index: flattener::IrIndex) -> Self {
        static ROOT_EMIT_LABEL_IDENT: EmitLabelIdent = EmitLabelIdent {
            segments: Vec::new(),
        };

        FlowTracer {
            env,
            bottom_ir_index,
            graph: MaybeOwned::Owned(FlowGraph::default()),
            curr_emit_label_ident: &ROOT_EMIT_LABEL_IDENT,
            next_local_emit_index: MaybeOwned::Owned(LocalEmitIndex::from(0)),
            label_scope: MaybeOwned::Owned(LabelScope::new()),
            curr_state: MaybeOwned::Owned(FlowState::default()),
            exit_state: MaybeOwned::Owned(FlowState::default()),
        }
    }

    fn init_top_label_params(
        &mut self,
        params: &flattener::Params,
        param_order: &[flattener::ParamIndex],
    ) {
        let exit_label_node_index = self.graph.exit_label_node_index();
        for param_index in param_order {
            if let flattener::ParamIndex::Label(label_param_index) = param_index
                && params[label_param_index].label.ir == self.bottom_ir_index
            {
                // Top-level input label parameters are all represented by the
                // shared global exit label.
                self.label_scope
                    .insert(label_param_index.into(), exit_label_node_index);
            }
        }
    }

    fn init_label_params_with_args(
        &mut self,
        params: &flattener::Params,
        param_order: &[flattener::ParamIndex],
        args: &[flattener::Arg],
        caller_label_scope: &LabelScope,
    ) {
        for (param_index, arg) in param_order.iter().zip(args) {
            if let flattener::ParamIndex::Label(label_param_index) = param_index
                && params[label_param_index].label.ir == self.bottom_ir_index
            {
                let arg_label_node_index = match arg {
                    flattener::Arg::Expr(_) | flattener::Arg::OutVar(_) => continue,
                    flattener::Arg::Label(label_arg) => caller_label_scope[&label_arg.label],
                    // Label fields are represented with the shared global exit
                    // label.
                    flattener::Arg::LabelField(_) => self.graph.exit_label_node_index(),
                };
                self.label_scope
                    .insert(label_param_index.into(), arg_label_node_index);
            }
        }
    }

    fn trace_callable_item(&mut self, callable_item: &flattener::CallableItem) {
        if let Some(body) = &callable_item.body {
            self.trace_body(body);
        }
    }

    fn trace_body(&mut self, body: &flattener::Body) {
        self.trace_local_labels(&body.locals.local_labels);
        assert_eq!(
            self.trace_stmts(&body.stmts),
            Break(()),
            "tracing a body block should always terminate at a return or an `unreachable`"
        );
        assert!(
            self.curr_state.is_empty(),
            "tracing a body block should always end with an empty current working state"
        );
        self.curr_state.append(&mut self.exit_state);
    }

    fn trace_local_labels(&mut self, local_labels: &TiSlice<LocalLabelIndex, flattener::Label>) {
        for (local_label_index, local_label) in local_labels.iter_enumerated() {
            if local_label.ir == self.bottom_ir_index {
                self.insert_label_node(local_label_index.into());
            }
        }
    }

    fn trace_stmts(&mut self, stmts: &[flattener::Stmt]) -> ControlFlow<()> {
        stmts.iter().try_for_each(|stmt| self.trace_stmt(stmt))
    }

    fn trace_branches<'b>(
        &mut self,
        branches: impl Iterator<Item = &'b [flattener::Stmt]>,
    ) -> ControlFlow<()> {
        // First, back up the state going into the branches.
        let init_state = self.curr_state.clone();
        self.curr_state.clear();

        branches
            .fold(None, |accum_outcome, branch| {
                // Trace each branch from the starting predecessor emits and bound
                // labels.
                let mut branch_flow_tracer = FlowTracer {
                    env: self.env,
                    bottom_ir_index: self.bottom_ir_index,
                    graph: MaybeOwned::Borrowed(&mut self.graph),
                    curr_emit_label_ident: self.curr_emit_label_ident,
                    next_local_emit_index: MaybeOwned::Borrowed(&mut self.next_local_emit_index),
                    label_scope: MaybeOwned::Borrowed(&mut self.label_scope),
                    // Each branch starts with a fresh copy of the backed-up
                    // state at the branch point.
                    curr_state: MaybeOwned::Owned(init_state.clone()),
                    // State at return points should be accumulated into our
                    // overall exit state accumulator across all branches.
                    exit_state: MaybeOwned::Borrowed(&mut self.exit_state),
                };
                let branch_outcome = branch_flow_tracer.trace_stmts(branch);

                // Accumulate the state at the end of each branch into the
                // current working state. Note that if the branch hit an early
                // return, this state will be empty.
                self.curr_state.append(&mut branch_flow_tracer.curr_state);

                // If all branches hit an early return, then we short-circuit
                // after tracing all of them. We continue tracing past the
                // branches if even one branch completes normally.
                match (accum_outcome, branch_outcome) {
                    (Some(Continue(())), _) | (_, Continue(())) => Some(Continue(())),
                    (_, Break(())) => Some(Break(())),
                }
            })
            // We also continue tracing if there were no branches.
            .unwrap_or_else(|| Continue(()))
    }

    fn trace_stmt(&mut self, stmt: &flattener::Stmt) -> ControlFlow<()> {
        match stmt {
            flattener::Stmt::Label(_)
            | flattener::Stmt::Let(flattener::LetStmt { rhs: None, .. }) => (),
            flattener::Stmt::Let(flattener::LetStmt {
                rhs: Some(expr), ..
            })
            | flattener::Stmt::Assign(flattener::AssignStmt { rhs: expr, .. })
            | flattener::Stmt::Check(flattener::CheckStmt { cond: expr, .. }) => {
                self.trace_expr(expr);
            }
            flattener::Stmt::Ret(flattener::RetStmt { value }) => {
                if let Some(expr) = value {
                    // return value expression might be an emitting function
                    // so we need to trace into it.
                    self.trace_expr(expr);
                }

                // We encountered a return statement. The current working state at this point
                // is the state upon exiting the containing callable (or at
                // least one potential state, in the case of branches). Drain
                // the current state into the exit state accumulator, clearing
                // the current state in the process.
                self.exit_state.append(&mut self.curr_state);

                // Short-circuit flow-tracing of this callable: since we hit an
                // return statement, nothing else along this path will run.
                return Break(());
            }
            flattener::Stmt::Unreachable => {
                // Control flow never gets past an `unreachable`, so treat it as
                // a return. Any emits on-deck here become predecessors of the
                // exit node, along an edge taken only if the unreachability
                // obligation fails.
                self.exit_state.append(&mut self.curr_state);
                return Break(());
            }
            flattener::Stmt::If(if_stmt) => self.trace_if_stmt(if_stmt)?,
            // TODO: assume right now that for-in loop body
            // does not contain any labels, binds or emits.
            flattener::Stmt::ForIn(_) => (),
            flattener::Stmt::Goto(goto_stmt) => self.trace_goto_stmt(goto_stmt),
            flattener::Stmt::Bind(bind_stmt) => self.trace_bind_stmt(bind_stmt),
            flattener::Stmt::Emit(emit_stmt) => self.trace_emit_stmt(emit_stmt),
            flattener::Stmt::Invoke(invoke_stmt) => self.trace_invoke_stmt(invoke_stmt),
        }

        Continue(())
    }

    fn trace_if_stmt(&mut self, if_stmt: &flattener::IfStmt) -> ControlFlow<()> {
        static EMPTY_BRANCH: Vec<flattener::Stmt> = Vec::new();
        let mut branches = vec![];
        let mut if_option = Some(if_stmt);
        while let Some(if_) = if_option {
            branches.push(if_.then.as_slice());
            match &if_.else_ {
                Some(flattener::ElseClause::ElseIf(if_next)) => if_option = Some(&*if_next),
                Some(flattener::ElseClause::Else(b)) => {
                    branches.push(&b);
                    if_option = None;
                }
                None => {
                    branches.push(&EMPTY_BRANCH);
                    if_option = None;
                }
            }
        }
        self.trace_branches(branches.into_iter())
    }

    fn trace_goto_stmt(&mut self, goto_stmt: &flattener::GotoStmt) {
        let label_node_index = self.label_scope[&goto_stmt.label];
        self.link_pred_emits_to_succ(label_node_index.into());
    }

    fn trace_bind_stmt(&mut self, bind_stmt: &flattener::BindStmt) {
        let label_node_index = self.label_scope[&bind_stmt.label];
        self.curr_state.bound_labels.insert(label_node_index);
    }

    fn trace_emit_stmt(&mut self, emit_stmt: &flattener::EmitStmt) {
        let ir_item = &self.env[emit_stmt.ir];
        let op_item = &self.env[emit_stmt.call.target];

        let new_emit_label_segment = EmitLabelSegment {
            local_emit_index: self.take_local_emit_index(),
            ident: EmitLabelSegmentIdent::Op(EmitOpLabelSegment {
                ir_ident: ir_item.ident.value.into(),
                op_selector: UserOpSelector::from(op_item.path.value.ident()).into(),
            }),
        };

        // Extend the emit label identifier with the emit we're currently
        // looking at.
        let new_emit_label_ident: EmitLabelIdent = self
            .curr_emit_label_ident
            .segments
            .iter()
            .copied()
            .chain(iter::once(new_emit_label_segment))
            .collect();

        // If the op we're emitting is at the interpreter level, create a
        // corresponding emit node and set it as the new predecessor on-deck.
        if ir_item.emits.is_none() {
            self.insert_and_link_emit_node(
                Some(new_emit_label_ident.clone()),
                Some(emit_stmt.call.target),
            );
        }

        // Trace inside the emitted op. It's important that we do this *after*
        // potentially creating a new emit node, so that, e.g., any goto
        // statements inside the op link their labels as successors of the new
        // emit node.

        let mut op_flow_tracer = FlowTracer {
            env: self.env,
            bottom_ir_index: self.bottom_ir_index,
            graph: MaybeOwned::Borrowed(&mut self.graph),
            curr_emit_label_ident: &new_emit_label_ident,
            next_local_emit_index: MaybeOwned::Owned(LocalEmitIndex::from(0)),
            label_scope: MaybeOwned::Owned(LabelScope::new()),
            // Tracing inside the emitted op starts from our current working
            // state and mutates it directly. After tracing the emitted op, our
            // current working state will be the state on control flow exiting
            // the op.
            curr_state: MaybeOwned::Borrowed(&mut self.curr_state),
            // On the other hand, the exit state accumulator starts with
            // a blank slate, since it should reflect the state when control
            // flow exits the emitted op, not the containing callable.
            exit_state: MaybeOwned::Owned(FlowState::default()),
        };
        op_flow_tracer.init_label_params_with_args(
            &op_item.params,
            &op_item.param_order,
            &emit_stmt.call.args,
            &self.label_scope,
        );
        op_flow_tracer.trace_callable_item(op_item);
    }

    fn trace_invoke_stmt(&mut self, invoke_stmt: &flattener::InvokeStmt) {
        self.trace_invoke_expr(invoke_stmt)
    }

    fn trace_expr(&mut self, expr: &flattener::Expr) {
        // TODO(abhishekc-sharma): Handle all the expression variants.
        match expr {
            flattener::Expr::Invoke(invoke_expr) => self.trace_invoke_expr(invoke_expr),
            _ => (),
        }
    }

    fn trace_invoke_expr(&mut self, invoke_expr: &flattener::InvokeExpr) {
        let fn_item = &self.env[invoke_expr.call.target];

        let mut out_label_args = invoke_expr.call.args.iter().filter_map(|arg| match arg {
            flattener::Arg::Label(label_arg)
                if label_arg.is_out && label_arg.ir == self.bottom_ir_index =>
            {
                Some(label_arg)
            }
            _ => None,
        });

        if fn_item.body.is_none() {
            // Labels bound through out-arguments to external functions point
            // into unknown territory.

            let exit_emit_node_index = self.graph.exit_emit_node_index();
            for out_label_arg in out_label_args {
                let label_node_index = self.label_scope[&out_label_arg.label];
                self.graph
                    .link_label_to_emit(label_node_index, exit_emit_node_index);
            }
        } else if fn_item.emits.is_some() || out_label_args.next().is_some() {
            // Otherwise, invoked functions only influence interpreter control
            // flow if they either emit ops or bind labels via passed-in label
            // out-arguments.

            let trace_inside_target = |flow_tracer: &mut Self, curr_emit_label_ident| {
                let mut target_flow_tracer = FlowTracer {
                    env: flow_tracer.env,
                    bottom_ir_index: flow_tracer.bottom_ir_index,
                    graph: MaybeOwned::Borrowed(&mut flow_tracer.graph),
                    curr_emit_label_ident,
                    next_local_emit_index: MaybeOwned::Owned(LocalEmitIndex::from(0)),
                    label_scope: MaybeOwned::Owned(LabelScope::new()),
                    // Tracing inside the invoked function starts from our current working
                    // state and mutates it directly. After tracing the function, our
                    // current working state will be the state on control flow exiting
                    // the function.
                    curr_state: MaybeOwned::Borrowed(&mut flow_tracer.curr_state),
                    // On the other hand, the exit state accumulator starts with
                    // a blank slate, since it should reflect the state when control
                    // flow exits the invoked function, not the containing callable.
                    exit_state: MaybeOwned::Owned(FlowState::default()),
                };

                target_flow_tracer.init_label_params_with_args(
                    &fn_item.params,
                    &fn_item.param_order,
                    &invoke_expr.call.args,
                    &flow_tracer.label_scope,
                );

                target_flow_tracer.trace_callable_item(fn_item);
            };

            if fn_item.emits.is_some() {
                // Extend the emit label identifier with the function we're currently
                // looking at.

                let new_emit_label_segment = EmitLabelSegment {
                    local_emit_index: self.take_local_emit_index(),
                    ident: EmitLabelSegmentIdent::Fn(
                        UserFnIdent {
                            parent_ident: fn_item
                                .path
                                .value
                                .parent()
                                .map(|parent_path| parent_path.ident()),
                            fn_ident: fn_item.path.value.ident(),
                        }
                        .into(),
                    ),
                };

                let new_emit_label_ident = self
                    .curr_emit_label_ident
                    .segments
                    .iter()
                    .copied()
                    .chain(iter::once(new_emit_label_segment))
                    .collect();

                trace_inside_target(self, &new_emit_label_ident);
            } else {
                trace_inside_target(self, self.curr_emit_label_ident);
            }
        }
    }

    fn insert_entry_emit_node(&mut self) {
        self.insert_and_link_emit_node(None, None);
    }

    fn insert_exit_emit_node(&mut self) {
        debug_assert!(self.graph.emit_nodes.is_empty());
        debug_assert!(self.curr_state.pred_emits.is_empty());
        self.insert_emit_node(Some(EmitLabelIdent::default()), None);
    }

    fn link_exit_emit_node(&mut self) {
        self.link_emit_node(self.graph.exit_emit_node_index());
    }

    fn insert_emit_node(
        &mut self,
        label_ident: Option<EmitLabelIdent>,
        target: Option<flattener::CallableIndex>,
    ) -> EmitNodeIndex {
        self.graph.emit_nodes.push_and_get_key(EmitNode {
            label_ident,
            succs: HashSet::new(),
            target,
        })
    }

    fn insert_and_link_emit_node(
        &mut self,
        label_ident: Option<EmitLabelIdent>,
        target: Option<flattener::CallableIndex>,
    ) {
        let emit_node_index = self.insert_emit_node(label_ident, target);
        self.link_emit_node(emit_node_index);
        self.curr_state.pred_emits.insert(emit_node_index);
    }

    fn link_emit_node(&mut self, emit_node_index: EmitNodeIndex) {
        self.link_pred_emits_to_succ(emit_node_index.into());
        self.link_bound_labels_to_emit(emit_node_index);
    }

    fn link_pred_emits_to_succ(&mut self, succ: EmitSucc) {
        for pred_emit in self.curr_state.pred_emits.drain() {
            self.graph[pred_emit].succs.insert(succ);
        }
    }

    fn link_bound_labels_to_emit(&mut self, emit_node_index: EmitNodeIndex) {
        for bound_label_index in self.curr_state.bound_labels.drain() {
            self.graph
                .link_label_to_emit(bound_label_index, emit_node_index);
        }
    }

    fn take_local_emit_index(&mut self) -> LocalEmitIndex {
        let local_emit_index = *self.next_local_emit_index;
        *self.next_local_emit_index = LocalEmitIndex::from(usize::from(local_emit_index) + 1);
        local_emit_index
    }

    fn insert_label_node(&mut self, label_index: flattener::LabelIndex) -> LabelNodeIndex {
        let label_node_index = self
            .graph
            .label_nodes
            .push_and_get_key(LabelNode::default());
        self.label_scope.insert(label_index, label_node_index);
        label_node_index
    }

    fn insert_exit_label_node(&mut self) {
        debug_assert!(self.graph.label_nodes.is_empty());
        let exit_emit_node_index = self.graph.exit_emit_node_index();
        self.graph.label_nodes.push_and_get_key(LabelNode {
            bound_to: HashSet::from([exit_emit_node_index]),
        });
    }
}
