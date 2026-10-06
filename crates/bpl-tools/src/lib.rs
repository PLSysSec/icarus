// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ops::Index;
use std::str::FromStr;

use lazy_static::lazy_static;
use num_bigint::BigUint;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{Dfs, VisitMap, Visitable};

use bpl::ast::*;
pub use bpl::parser::ParseError;
use bpl::parser::parse_boogie_program;

lazy_static! {
    static ref INLINE_ATTR_IDENT: Ident = Ident::from("inline");
}

pub fn inline_program<'a>(
    src: &'a str,
    procs_level: Option<&BigUint>,
) -> Result<Cow<'a, str>, ParseError> {
    let program = parse_boogie_program(src)?;

    let mut spans_to_replace = Vec::new();
    for decl in &program.decls {
        match &decl.value {
            Decl::Proc(proc_decl) => {
                if let Some(procs_level) = procs_level {
                    inline_proc_sign(&procs_level, &proc_decl.proc_sign, &mut spans_to_replace);
                }
            }
            Decl::Impl(impl_decl) => {
                if let Some(procs_level) = procs_level {
                    inline_proc_sign(&procs_level, &impl_decl.proc_sign, &mut spans_to_replace);
                }
            }
            _ => (),
        }
    }

    Ok(replace_spans(src, spans_to_replace))
}

fn inline_proc_sign(
    inline_level: &BigUint,
    proc_sign: &ProcSign,
    spans_to_replace: &mut Vec<(Span, String)>,
) {
    let mut max_inline_level = Cow::Borrowed(inline_level);

    let mut attr_or_trigger_iter = proc_sign.attrs.iter().peekable();
    while let Some(attr_or_trigger) = attr_or_trigger_iter.next() {
        if let AttrOrTrigger::Attr(attr) = &attr_or_trigger.value {
            if attr.ident == *INLINE_ATTR_IDENT {
                if attr.params.len() == 1 {
                    let found_inline_level_param = &attr.params[0];
                    if let AttrParam::Expr(Expr::Nat(found_inline_level_str)) =
                        found_inline_level_param
                    {
                        if let Ok(found_inline_level) = BigUint::from_str(found_inline_level_str) {
                            if &found_inline_level >= inline_level {
                                max_inline_level = Cow::Owned(found_inline_level);
                            }
                        }
                    }
                }

                let span_start = attr_or_trigger.span.start();
                let span_end = match attr_or_trigger_iter.peek() {
                    Some(next_attr_or_trigger) => next_attr_or_trigger.span.start(),
                    None => proc_sign.ident.span.start(),
                };
                spans_to_replace.push((Span::new(span_start, span_end), String::new()));
            }
        }
    }

    spans_to_replace.push((
        Span::new(proc_sign.ident.span.start(), proc_sign.ident.span.start()),
        format!(
            "{} ",
            Attr::from(AttrContent {
                ident: *INLINE_ATTR_IDENT,
                params: vec![AttrParam::Expr(Expr::Nat(max_inline_level.to_string()))],
            })
        ),
    ));
}

fn replace_spans<'a>(src: &'a str, spans_to_replace: Vec<(Span, String)>) -> Cow<'a, str> {
    if spans_to_replace.is_empty() {
        return Cow::Borrowed(src);
    }

    // We should never be handling overlapping spans.
    for span_index in 1..spans_to_replace.len() {
        assert!(
            spans_to_replace[span_index - 1].0.end() <= spans_to_replace[span_index].0.start()
        );
    }

    let src_bytes = src.as_bytes();
    let mut new_src_bytes = Vec::new();
    let mut last_span_end = ByteIndex::default();
    for (span, replacement) in spans_to_replace {
        new_src_bytes.extend(&src_bytes[last_span_end.to_usize()..span.start().to_usize()]);
        new_src_bytes.extend(replacement.into_bytes());
        last_span_end = span.end();
    }
    new_src_bytes.extend(&src_bytes[last_span_end.to_usize()..]);
    Cow::Owned(
        String::from_utf8(new_src_bytes)
            .expect("encoding error replacing spans in Boogie source code"),
    )
}

pub fn shake_tree<'a>(
    src: &'a str,
    retain_idents: impl IntoIterator<Item = NamespacedIdent>,
    can_prune_data_type_ident: impl Fn(Ident) -> bool,
) -> Result<Cow<'a, str>, ParseError> {
    let program = parse_boogie_program(src)?;

    let mut reachability_visitor = ReachabilityVisitor::new(can_prune_data_type_ident);
    reachability_visitor.visit_program(&program);
    let reachability_graph = reachability_visitor.graph;

    let mut reachability_map = ReachabilityMap::new(&reachability_graph);
    for ident in retain_idents.into_iter() {
        if let Some(ident_node_index) = reachability_graph.lookup_node_with_ident(ident) {
            reachability_map.mark_reachable(&reachability_graph, ident_node_index);
        }
    }
    for (decl_index, decl) in program.iter().enumerate() {
        // TODO(spinda): For now, we assume all axioms are required. A future
        // improvement might be to figure out how to prune those.
        if decl_is_entry_point(&decl.value) || decl_is_axiom(&decl.value) {
            let decl_node_index = reachability_graph.decl_node_indexes[decl_index];
            reachability_map.mark_reachable(&reachability_graph, decl_node_index);
        }
    }

    let spans_to_replace =
        find_spans_to_replace_for_reachability(&reachability_graph, &reachability_map);
    Ok(replace_spans(src, spans_to_replace))
}

fn decl_is_entry_point(decl: &Decl) -> bool {
    lazy_static! {
        static ref ENTRY_POINT_ATTR_IDENT: Ident = Ident::from("entrypoint");
    }

    let proc_decl = match decl {
        Decl::Proc(proc_decl) => proc_decl,
        _ => return false,
    };

    has_attr(
        proc_decl.proc_sign.attrs.iter().map(|attr| &attr.value),
        *ENTRY_POINT_ATTR_IDENT,
    )
}

fn decl_is_axiom(decl: &Decl) -> bool {
    match decl {
        Decl::Axiom(_) => true,
        _ => false,
    }
}

fn has_attr<'a>(attrs: impl IntoIterator<Item = &'a Attr>, attr_ident: Ident) -> bool {
    attrs.into_iter().any(|attr| match attr {
        AttrOrTrigger::Attr(attr_content) => attr_content.ident == attr_ident,
        AttrOrTrigger::Trigger(_) => false,
    })
}

fn find_spans_to_replace_for_reachability(
    reachability_graph: &ReachabilityGraph,
    reachability_map: &ReachabilityMap,
) -> Vec<(Span, String)> {
    let mut spans_to_replace = Vec::new();
    find_spans_to_replace_for_reachability_nodes(
        &reachability_graph.decl_node_indexes,
        reachability_graph,
        reachability_map,
        &mut spans_to_replace,
    );
    spans_to_replace.sort();
    spans_to_replace
}

fn find_spans_to_replace_for_reachability_nodes(
    node_indexes: &[NodeIndex],
    reachability_graph: &ReachabilityGraph,
    reachability_map: &ReachabilityMap,
    spans_to_replace: &mut Vec<(Span, String)>,
) {
    let mut prev_span_end = None;
    let mut node_indexes_iter = node_indexes.iter().copied().peekable();
    while let Some(node_index) = node_indexes_iter.next() {
        let reachability_node = &reachability_graph[node_index];
        if reachability_map.is_reachable(node_index) {
            // If a node has no roots, then we're done. If it has exactly one
            // root, then that root *must* be reachable if the parent node is
            // reachable. Otherwise, we have to check each root for
            // reachability.
            find_spans_to_replace_for_reachability_nodes(
                &reachability_node.root_node_indexes,
                reachability_graph,
                reachability_map,
                spans_to_replace,
            );
        } else {
            let span_to_eliminate = match prev_span_end {
                None => match node_indexes_iter.peek() {
                    // If this is the only node on this level, just eliminate
                    // its span.
                    None => reachability_node.span,
                    // If this is the first node and it's followed by at least
                    // one other node, eliminate everything from the start of
                    // this node's span to the start of the next node's span.
                    Some(next_node_index) => {
                        let next_reachability_node = &reachability_graph[*next_node_index];
                        Span::new(
                            reachability_node.span.start(),
                            next_reachability_node.span.start(),
                        )
                    }
                },
                // If there's at least one node preceding this one, eliminate
                // everything from the end of the previous node's span to the
                // end of the current node's span.
                Some(prev_span_end) => Span::new(prev_span_end, reachability_node.span.end()),
            };
            spans_to_replace.push((span_to_eliminate, String::new()));
        }

        prev_span_end = Some(reachability_node.span.end());
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Namespace {
    Func,
    Proc,
    Type,
    /// Note: `const` and `var` declarations share the same namespace.
    Var,
}

#[derive(Clone, Copy)]
enum Scope {
    Global,
    Local,
}

#[derive(Clone, Copy)]
enum OccurrenceKind {
    Decl(Scope),
    Ref,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NamespacedIdent(pub Ident, pub Namespace);

struct ReachabilityNode {
    span: Span,
    root_node_indexes: Vec<NodeIndex>,
}

type RawReachabilityGraph = DiGraph<ReachabilityNode, ()>;

#[derive(Default)]
struct ReachabilityGraph {
    graph: RawReachabilityGraph,
    ident_indexes: HashMap<NamespacedIdent, NodeIndex>,
    decl_node_indexes: Vec<NodeIndex>,
}

impl ReachabilityGraph {
    fn reserve_decls(&mut self, additional: usize) {
        self.graph.reserve_nodes(additional);
        self.decl_node_indexes.reserve(additional);
    }

    fn lookup_node_with_ident(&self, ident: NamespacedIdent) -> Option<NodeIndex> {
        self.ident_indexes.get(&ident).copied()
    }

    fn get_node_with_ident(&mut self, ident: Spanned<NamespacedIdent>) -> NodeIndex {
        *self
            .ident_indexes
            .entry(ident.value)
            // NOTE(spinda): We can't use `ReachabilityGraph::add_node` here in
            // current Rust because calling it requires a mutable reference to
            // the entire `self`, even though it only actually modifies
            // `self.graph`.
            .or_insert_with(|| {
                self.graph.add_node(ReachabilityNode {
                    span: ident.span,
                    root_node_indexes: Vec::new(),
                })
            })
    }

    fn add_node(&mut self, span: Span) -> NodeIndex {
        self.graph.add_node(ReachabilityNode {
            span,
            root_node_indexes: Vec::new(),
        })
    }

    fn add_decl_node(&mut self, span: Span) -> NodeIndex {
        let decl_node_index = self.add_node(span);
        self.decl_node_indexes.push(decl_node_index);
        decl_node_index
    }

    fn add_edge(&mut self, src_node_index: NodeIndex, dst_node_index: NodeIndex) {
        self.graph.add_edge(src_node_index, dst_node_index, ());
    }

    fn attach_root_to_node(&mut self, parent_node_index: NodeIndex, root_node_index: NodeIndex) {
        self.add_edge(root_node_index, parent_node_index);
        self.graph[parent_node_index]
            .root_node_indexes
            .push(root_node_index);
    }

    fn attach_ident_to_node(
        &mut self,
        parent_node_index: NodeIndex,
        ident: Spanned<NamespacedIdent>,
    ) {
        let ident_node_index = self.get_node_with_ident(ident);
        self.attach_root_to_node(parent_node_index, ident_node_index);
    }
}

impl Index<NodeIndex> for ReachabilityGraph {
    type Output = ReachabilityNode;

    fn index(&self, index: NodeIndex) -> &Self::Output {
        &self.graph[index]
    }
}

struct ReachabilityVisitor<F> {
    graph: ReachabilityGraph,
    can_prune_data_type_ident: F,
}

impl<F: Fn(Ident) -> bool> ReachabilityVisitor<F> {
    fn new(can_prune_data_type_ident: F) -> Self {
        Self {
            graph: ReachabilityGraph::default(),
            can_prune_data_type_ident,
        }
    }

    fn nest_decl(&mut self, span: Span) -> NestedReachabilityVisitor<'_, F> {
        let decl_node_index = self.graph.add_decl_node(span);
        self.nest_with_parent(decl_node_index)
    }

    fn nest_decl_with_ident(
        &mut self,
        span: Span,
        ident: Ident,
        namespace: Namespace,
    ) -> NestedReachabilityVisitor<'_, F> {
        let decl_node_index = self.graph.add_decl_node(span);
        self.graph.attach_ident_to_node(
            decl_node_index,
            // For single-identifier declarations, we don't need to attach real
            // position information, because it won't be used.
            Spanned::new(Span::default(), NamespacedIdent(ident, namespace)),
        );
        self.nest_with_parent(decl_node_index)
    }

    fn nest_with_parent(&mut self, parent_index: NodeIndex) -> NestedReachabilityVisitor<'_, F> {
        NestedReachabilityVisitor {
            visitor: self,
            parent_node_index: parent_index,
            local_idents: Cow::Owned(HashSet::new()),
        }
    }

    fn visit_program(&mut self, program: &BoogieProgram) {
        self.graph.reserve_decls(program.len());
        for decl in program {
            self.visit_decl(decl);
        }
    }

    fn visit_decl(&mut self, decl: &Spanned<Decl>) {
        match &decl.value {
            Decl::Axiom(axiom_decl) => self.visit_axiom_decl(Spanned::new(decl.span, axiom_decl)),
            Decl::Const(const_decl) => self.visit_const_decl(Spanned::new(decl.span, const_decl)),
            Decl::Func(func_decl) => self.visit_func_decl(Spanned::new(decl.span, func_decl)),
            Decl::Impl(impl_decl) => self.visit_impl_decl(Spanned::new(decl.span, impl_decl)),
            Decl::Proc(proc_decl) => self.visit_proc_decl(Spanned::new(decl.span, proc_decl)),
            Decl::Type(type_decls) => self.visit_type_decls(Spanned::new(decl.span, type_decls)),
            Decl::Var(var_decl) => self.visit_var_decl(Spanned::new(decl.span, var_decl)),
        }
    }

    fn visit_axiom_decl(&mut self, axiom_decl: Spanned<&AxiomDecl>) {
        let mut axiom_decl_visitor = self.nest_decl(axiom_decl.span);
        axiom_decl_visitor.visit_attrs(&axiom_decl.value.attrs);
        axiom_decl_visitor.visit_proposition(&axiom_decl.value.proposition);
    }

    fn visit_const_decl(&mut self, const_decl: Spanned<&ConstDecl>) {
        let mut const_decl_visitor = self.nest_decl(const_decl.span);
        const_decl_visitor.visit_attrs(&const_decl.value.attrs);
        const_decl_visitor.visit_typed_idents(&const_decl.value.consts, Scope::Global);
        if let Some(order_spec) = &const_decl.value.order_spec {
            const_decl_visitor.visit_order_spec(order_spec);
        }
    }

    fn visit_func_decl(&mut self, func_decl: Spanned<&FuncDecl>) {
        let mut func_decl_visitor =
            self.nest_decl_with_ident(func_decl.span, func_decl.value.ident, Namespace::Func);

        func_decl_visitor.visit_type_params(&func_decl.value.type_params);

        for var_param in &func_decl.value.var_params {
            func_decl_visitor.visit_var_or_type(var_param);
        }

        func_decl_visitor.visit_var_or_type(&func_decl.value.returns);
        if func_decl_is_ctor(&func_decl.value) {
            if let Some(data_type_ident) =
                extract_data_type_ident_from_type(&func_decl.value.returns.type_)
            {
                let data_type_namespaced_ident = NamespacedIdent(data_type_ident, Namespace::Type);
                if !func_decl_visitor
                    .local_idents
                    .contains(&data_type_namespaced_ident)
                {
                    // TODO(spinda): Link in is#... and <param>#... functions.
                    if !(func_decl_visitor.visitor.can_prune_data_type_ident)(data_type_ident) {
                        // Special handling for constructors: we additionally
                        // draw an edge from the parent data type to the
                        // constructor function.
                        let data_type_node_index = func_decl_visitor
                            .visitor
                            .graph
                            .get_node_with_ident(Spanned::new(
                                Span::default(),
                                data_type_namespaced_ident,
                            ));
                        func_decl_visitor
                            .visitor
                            .graph
                            .add_edge(data_type_node_index, func_decl_visitor.parent_node_index);
                    }
                }
            }
        }

        func_decl_visitor.visit_attrs(&func_decl.value.attrs);

        if let Some(body) = &func_decl.value.body {
            func_decl_visitor.visit_expr(body);
        }
    }

    fn visit_impl_decl(&mut self, impl_decl: Spanned<&ImplDecl>) {
        let mut impl_decl_visitor = self.nest_decl_with_ident(
            impl_decl.span,
            impl_decl.value.proc_sign.ident.value,
            Namespace::Proc,
        );
        impl_decl_visitor.visit_type_params(&impl_decl.value.proc_sign.type_params);
        impl_decl_visitor
            .visit_attr_typed_idents_wheres(&impl_decl.value.proc_sign.var_params, Scope::Local);
        impl_decl_visitor
            .visit_attr_typed_idents_wheres(&impl_decl.value.proc_sign.returns, Scope::Local);
        impl_decl_visitor.visit_impl_body(&impl_decl.value.impl_body);
        impl_decl_visitor.visit_attrs(
            impl_decl
                .value
                .proc_sign
                .attrs
                .iter()
                .map(|attr| &attr.value),
        );
    }

    fn visit_proc_decl(&mut self, proc_decl: Spanned<&ProcDecl>) {
        let mut proc_decl_visitor = self.nest_decl_with_ident(
            proc_decl.span,
            proc_decl.value.proc_sign.ident.value,
            Namespace::Proc,
        );
        proc_decl_visitor.visit_type_params(&proc_decl.value.proc_sign.type_params);
        proc_decl_visitor.visit_modifies_specs(&proc_decl.value.specs);
        proc_decl_visitor
            .visit_attr_typed_idents_wheres(&proc_decl.value.proc_sign.var_params, Scope::Local);
        proc_decl_visitor.visit_contract_specs(&proc_decl.value.specs, ContractKind::Requires);
        proc_decl_visitor
            .visit_attr_typed_idents_wheres(&proc_decl.value.proc_sign.returns, Scope::Local);
        proc_decl_visitor.visit_contract_specs(&proc_decl.value.specs, ContractKind::Ensures);
        if let Some(impl_body) = &proc_decl.value.impl_body {
            proc_decl_visitor.visit_impl_body(impl_body);
        }
        proc_decl_visitor.visit_attrs(
            proc_decl
                .value
                .proc_sign
                .attrs
                .iter()
                .map(|attr| &attr.value),
        );
    }

    fn visit_type_decls(&mut self, type_decls: Spanned<&TypeDecls>) {
        let mut type_decls_visitor = self.nest_decl(type_decls.span);
        for type_decl in &type_decls.value.decls {
            type_decls_visitor.visit_type_decl(type_decl);
        }
        type_decls_visitor.visit_attrs(&type_decls.value.attrs);
    }

    fn visit_var_decl(&mut self, var_decl: Spanned<&VarDecl>) {
        let mut var_decl_visitor = self.nest_decl(var_decl.span);

        for typed_idents_where in &var_decl.value.vars {
            let mut typed_idents_where_visitor =
                var_decl_visitor.nest_node(typed_idents_where.span);
            typed_idents_where_visitor
                .visit_typed_idents(&typed_idents_where.value.typed_idents, Scope::Global);
        }

        // TODO(spinda): A future improvement might be to eliminate
        // where-clauses that only referenced unused things. I don't know if
        // that's actually sound, though - haven't put too much thought into it
        // yet.
        let typed_idents_where_node_indexes = &var_decl_visitor.visitor.graph
            [var_decl_visitor.parent_node_index]
            .root_node_indexes
            .clone();
        for (typed_idents_where, typed_idents_where_node_index) in var_decl
            .value
            .vars
            .iter()
            .zip(typed_idents_where_node_indexes)
        {
            if let Some(where_) = &typed_idents_where.value.where_ {
                let mut typed_idents_where_visitor =
                    var_decl_visitor.nest_with_parent(*typed_idents_where_node_index);
                typed_idents_where_visitor.visit_expr(where_);
            }
        }

        var_decl_visitor.visit_attrs(&var_decl.value.attrs);
    }
}

struct NestedReachabilityVisitor<'a, F> {
    visitor: &'a mut ReachabilityVisitor<F>,
    parent_node_index: NodeIndex,
    local_idents: Cow<'a, HashSet<NamespacedIdent>>,
}

impl<'a, F> NestedReachabilityVisitor<'a, F> {
    fn nest(&mut self) -> NestedReachabilityVisitor<'_, F> {
        self.nest_with_parent(self.parent_node_index)
    }

    fn nest_node(&mut self, span: Span) -> NestedReachabilityVisitor<'_, F> {
        let node_index = self.visitor.graph.add_node(span);
        self.visitor
            .graph
            .attach_root_to_node(self.parent_node_index, node_index);
        self.nest_with_parent(node_index)
    }

    fn nest_node_with_ident(
        &mut self,
        span: Span,
        ident: Ident,
        namespace: Namespace,
    ) -> NestedReachabilityVisitor<'_, F> {
        let nested_visitor = self.nest_node(span);
        nested_visitor.visitor.graph.attach_ident_to_node(
            nested_visitor.parent_node_index,
            // As with `nest_decl_with_ident`, whatever position we attach here
            // won't be used.
            Spanned::new(Span::default(), NamespacedIdent(ident, namespace)),
        );
        nested_visitor
    }

    fn nest_with_parent(&mut self, parent_index: NodeIndex) -> NestedReachabilityVisitor<'_, F> {
        NestedReachabilityVisitor {
            visitor: &mut self.visitor,
            parent_node_index: parent_index,
            local_idents: Cow::Borrowed(&self.local_idents),
        }
    }

    fn visit_type_decl(&mut self, type_decl: &Spanned<TypeDecl>) {
        let mut type_decl_visitor =
            self.nest_node_with_ident(type_decl.span, type_decl.value.ident, Namespace::Type);
        for type_param in &type_decl.value.type_params {
            type_decl_visitor.declare_local_ident(*type_param, Namespace::Type);
        }
        if let Some(type_) = &type_decl.value.type_ {
            type_decl_visitor.visit_type(type_);
        }
    }

    fn visit_order_spec(&mut self, order_spec: &OrderSpec) {
        for parent in &order_spec.parents {
            self.visit_order_spec_parent(parent);
        }
    }

    fn visit_order_spec_parent(&mut self, order_spec_parent: &OrderSpecParent) {
        self.reference_ident(order_spec_parent.parent, Namespace::Var);
    }

    fn visit_var_or_type(&mut self, var_or_type: &VarOrType) {
        if let Some(var_ident) = &var_or_type.var {
            self.declare_local_ident(*var_ident, Namespace::Var);
        }
        self.visit_type(&var_or_type.type_);
        self.visit_attrs(&var_or_type.attrs);
    }

    fn visit_impl_body(&mut self, impl_body: &ImplBody) {
        self.visit_local_vars_list(&impl_body.local_vars);
        self.visit_stmt_list(&impl_body.stmt_list);
    }

    fn visit_stmt_list(&mut self, stmt_list: &StmtList) {
        for stmt in stmt_list {
            self.visit_stmt(stmt);
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::LabelOrCmd(label_or_cmd) => self.visit_label_or_cmd(label_or_cmd),
            Stmt::TransferCmd(transfer_cmd) => self.visit_transfer_cmd(transfer_cmd),
            Stmt::StructuredCmd(structured_cmd) => self.visit_structured_cmd(structured_cmd),
        }
    }

    fn visit_local_vars_list(&mut self, local_vars_list: &[LocalVars]) {
        for local_vars in local_vars_list {
            for typed_idents_where in &local_vars.vars {
                self.visit_typed_idents(&typed_idents_where.value.typed_idents, Scope::Local);
            }
            self.visit_attrs(&local_vars.attrs);
        }

        for local_vars in local_vars_list {
            for typed_idents_where in &local_vars.vars {
                if let Some(where_) = &typed_idents_where.value.where_ {
                    self.visit_expr(where_);
                }
            }
        }
    }

    fn visit_modifies_specs(&mut self, specs: &[Spec]) {
        for spec in specs {
            if let Spec::Modifies(modifies_spec) = spec {
                self.visit_modifies_spec(modifies_spec);
            }
        }
    }

    fn visit_modifies_spec(&mut self, modifies_spec: &ModifiesSpec) {
        self.visit_idents(&modifies_spec.vars, Namespace::Var, OccurrenceKind::Ref);
    }

    fn visit_contract_specs(&mut self, specs: &[Spec], kind: ContractKind) {
        for spec in specs {
            if let Spec::Contract(contract_spec) = spec
                && contract_spec.kind == kind
            {
                self.visit_contract_spec(contract_spec);
            }
        }
    }

    fn visit_contract_spec(&mut self, contract_spec: &ContractSpec) {
        self.visit_attrs(&contract_spec.attrs);
        self.visit_proposition(&contract_spec.proposition);
    }

    fn visit_label_or_cmd(&mut self, label_or_cmd: &LabelOrCmd) {
        match label_or_cmd {
            LabelOrCmd::Assign(assign_cmd) => self.visit_assign_cmd(assign_cmd),
            LabelOrCmd::Call(call_cmd) => self.visit_call_cmd(call_cmd),
            LabelOrCmd::Claim(claim_cmd) => self.visit_claim_cmd(claim_cmd),
            LabelOrCmd::Havoc(havoc_cmd) => self.visit_havoc_cmd(havoc_cmd),
            LabelOrCmd::Label(label) => self.visit_label(label),
            LabelOrCmd::ParCall(par_call_cmd) => self.visit_par_call_cmd(par_call_cmd),
            LabelOrCmd::Yield(yield_cmd) => self.visit_yield_cmd(yield_cmd),
        }
    }

    fn visit_transfer_cmd(&mut self, transfer_cmd: &TransferCmd) {
        match transfer_cmd {
            TransferCmd::Goto(goto_cmd) => self.visit_goto_cmd(goto_cmd),
            TransferCmd::Return(return_cmd) => self.visit_return_cmd(return_cmd),
        }
    }

    fn visit_structured_cmd(&mut self, structured_cmd: &StructuredCmd) {
        match structured_cmd {
            StructuredCmd::Break(break_cmd) => self.visit_break_cmd(break_cmd),
            StructuredCmd::If(if_cmd) => self.visit_if_cmd(if_cmd),
            StructuredCmd::While(while_cmd) => self.visit_while_cmd(while_cmd),
        }
    }

    fn visit_assign_cmd(&mut self, assign_cmd: &AssignCmd) {
        for lhs in &assign_cmd.lhs {
            self.visit_assign_lhs(lhs);
        }
        self.visit_exprs(&assign_cmd.rhs);
    }

    fn visit_assign_lhs(&mut self, assign_lhs: &AssignLhs) {
        self.reference_ident(assign_lhs.ident, Namespace::Var);
        for subscript in &assign_lhs.subscripts {
            self.visit_exprs(subscript);
        }
    }

    fn visit_break_cmd(&mut self, BreakCmd { label: _label }: &BreakCmd) {
        // This only references labels, which we don't care about.
    }

    fn visit_call_cmd(&mut self, call_cmd: &CallCmd) {
        self.visit_attrs(&call_cmd.attrs);
        self.visit_call_params(&call_cmd.call_params);
    }

    fn visit_claim_cmd(&mut self, claim_cmd: &ClaimCmd) {
        self.visit_attrs(&claim_cmd.attrs);
        self.visit_proposition(&claim_cmd.proposition);
    }

    fn visit_goto_cmd(&mut self, GotoCmd { labels: _labels }: &GotoCmd) {
        // This only references labels, which we don't care about.
    }

    fn visit_havoc_cmd(&mut self, havoc_cmd: &HavocCmd) {
        self.visit_idents(&havoc_cmd.vars, Namespace::Var, OccurrenceKind::Ref);
    }

    fn visit_if_cmd(&mut self, if_cmd: &IfCmd) {
        self.visit_guard(&if_cmd.guard);
        self.visit_stmt_list(&if_cmd.then);
        if let Some(else_) = &if_cmd.else_ {
            self.visit_else_clause(else_);
        }
    }

    fn visit_else_clause(&mut self, else_clause: &ElseClause) {
        match else_clause {
            ElseClause::ElseIf(if_cmd) => self.visit_if_cmd(&*if_cmd),
            ElseClause::Else(stmt_list) => self.visit_stmt_list(stmt_list),
        }
    }

    fn visit_label(&mut self, _label: &Label) {
        // We don't care about label declarations.
    }

    fn visit_par_call_cmd(&mut self, par_call_cmd: &ParCallCmd) {
        self.visit_attrs(&par_call_cmd.attrs);
        for call in &par_call_cmd.calls {
            self.visit_call_params(call);
        }
    }

    fn visit_return_cmd(&mut self, ReturnCmd: &ReturnCmd) {
        // Nothing to do here.
    }

    fn visit_while_cmd(&mut self, while_cmd: &WhileCmd) {
        self.visit_guard(&while_cmd.guard);
        for invariant in &while_cmd.invariants {
            self.visit_invariant(invariant);
        }
        self.visit_stmt_list(&while_cmd.body);
    }

    fn visit_invariant(&mut self, invariant: &Invariant) {
        self.visit_attrs(&invariant.attrs);
        self.visit_expr(&invariant.expr);
    }

    fn visit_yield_cmd(&mut self, YieldCmd: &YieldCmd) {
        // Nothing to do here.
    }

    fn visit_call_params(&mut self, call_params: &CallParams) {
        self.visit_idents(&call_params.returns, Namespace::Var, OccurrenceKind::Ref);
        self.reference_ident(call_params.target, Namespace::Proc);
        self.visit_exprs(&call_params.params);
    }

    fn visit_guard(&mut self, guard: &Guard) {
        match guard {
            Guard::Asterisk => (),
            Guard::Expr(expr) => self.visit_expr(expr),
        }
    }

    fn visit_type(&mut self, type_: &Type) {
        match type_ {
            Type::Atom(type_atom) => self.visit_type_atom(type_atom),
            Type::App(type_app) => self.visit_ident_type_app(type_app),
            Type::Map(map_type) => self.visit_map_type(map_type),
        }
    }

    fn visit_ident_type_app(&mut self, type_app: &TypeApp<Ident>) {
        self.visit_type_app(type_app, |visitor, head| {
            visitor.reference_ident(*head, Namespace::Type)
        });
    }

    fn visit_atom_type_app(&mut self, type_app: &TypeApp<TypeAtom>) {
        self.visit_type_app(type_app, |visitor, head| visitor.visit_type_atom(head));
    }

    fn visit_type_app<T>(
        &mut self,
        type_app: &TypeApp<T>,
        visit_head: impl FnOnce(&mut Self, &T),
    ) {
        visit_head(self, &type_app.head);
        if let Some(tail) = &type_app.tail {
            self.visit_type_args(tail);
        }
    }

    fn visit_type_args(&mut self, type_args: &TypeArgs) {
        match type_args {
            TypeArgs::AtomApp(atom_type_app) => self.visit_atom_type_app(atom_type_app),
            TypeArgs::App(type_app) => self.visit_ident_type_app(type_app),
            TypeArgs::Map(map_type) => self.visit_map_type(map_type),
        }
    }

    fn visit_type_atom(&mut self, type_atom: &TypeAtom) {
        match type_atom {
            TypeAtom::Int | TypeAtom::Real | TypeAtom::Bool => (),
            TypeAtom::Paren(type_) => self.visit_type(&*type_),
        }
    }

    fn visit_map_type(&mut self, map_type: &MapType) {
        let mut map_type_visitor = self.nest();
        map_type_visitor.visit_type_params(&map_type.type_params);
        for key in &map_type.keys {
            map_type_visitor.visit_type(key);
        }
        map_type_visitor.visit_type(&*map_type.value);
    }

    fn visit_exprs(&mut self, exprs: &Exprs) {
        for expr in exprs {
            self.visit_expr(expr);
        }
    }

    fn visit_proposition(&mut self, proposition: &Proposition) {
        self.visit_expr(proposition);
    }

    fn visit_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Equiv(equiv_expr) => self.visit_equiv_expr(&*equiv_expr),
            Expr::Implies(implies_expr) => self.visit_implies_expr(&*implies_expr),
            Expr::Explies(explies_expr) => self.visit_explies_expr(&*explies_expr),
            Expr::Logical(logical_expr) => self.visit_logical_expr(&*logical_expr),
            Expr::Rel(rel_expr) => self.visit_rel_expr(&*rel_expr),
            Expr::BvTerm(bv_term) => self.visit_bv_term(&*bv_term),
            Expr::Term(term) => self.visit_term(&*term),
            Expr::Factor(factor) => self.visit_factor(&*factor),
            Expr::Power(power) => self.visit_power(&*power),
            Expr::Neg(neg_expr) => self.visit_neg_expr(&*neg_expr),
            Expr::Coercion(coercion_expr) => self.visit_coercion_expr(&*coercion_expr),
            Expr::Array(array_expr) => self.visit_array_expr(&*array_expr),
            Expr::BoolLit(_) | Expr::Nat(_) | Expr::Dec(_) | Expr::Float(_) | Expr::BvLit(_) => (),
            Expr::Var(var_ident) => self.reference_ident(*var_ident, Namespace::Var),
            Expr::FuncCall(func_call) => self.visit_func_call(&func_call),
            Expr::Old(old_expr) => self.visit_old_expr(&*old_expr),
            Expr::ArithCoercion(arith_coercion_expr) => {
                self.visit_arith_coercion_expr(&*arith_coercion_expr)
            }
            Expr::Quant(quant_expr) => self.visit_quant_expr(&*quant_expr),
            Expr::IfThenElse(if_then_else_expr) => {
                self.visit_if_then_else_expr(&*if_then_else_expr)
            }
            Expr::Code(code_expr) => self.visit_code_expr(&*code_expr),
        }
    }

    fn visit_equiv_expr(&mut self, equiv_expr: &EquivExpr) {
        self.visit_expr(&equiv_expr.lhs);
        self.visit_expr(&equiv_expr.rhs);
    }

    fn visit_implies_expr(&mut self, implies_expr: &ImpliesExpr) {
        self.visit_expr(&implies_expr.lhs);
        self.visit_expr(&implies_expr.rhs);
    }

    fn visit_explies_expr(&mut self, explies_expr: &ExpliesExpr) {
        self.visit_expr(&explies_expr.lhs);
        self.visit_expr(&explies_expr.rhs);
    }

    fn visit_logical_expr(&mut self, logical_expr: &LogicalExpr) {
        self.visit_expr(&logical_expr.lhs);
        self.visit_expr(&logical_expr.rhs);
    }

    fn visit_rel_expr(&mut self, rel_expr: &RelExpr) {
        self.visit_expr(&rel_expr.lhs);
        self.visit_expr(&rel_expr.rhs);
    }

    fn visit_bv_term(&mut self, bv_term: &BvTerm) {
        self.visit_expr(&bv_term.lhs);
        self.visit_expr(&bv_term.rhs);
    }

    fn visit_term(&mut self, term: &Term) {
        self.visit_expr(&term.lhs);
        self.visit_expr(&term.rhs);
    }

    fn visit_factor(&mut self, factor: &Factor) {
        self.visit_expr(&factor.lhs);
        self.visit_expr(&factor.rhs);
    }

    fn visit_power(&mut self, power: &Power) {
        self.visit_expr(&power.lhs);
        self.visit_expr(&power.rhs);
    }

    fn visit_neg_expr(&mut self, neg_expr: &NegExpr) {
        self.visit_expr(&neg_expr.expr);
    }

    fn visit_coercion_expr(&mut self, coercion_expr: &CoercionExpr) {
        self.visit_expr(&coercion_expr.expr);
        for coercion in &coercion_expr.coercions {
            self.visit_coercion(coercion);
        }
    }

    fn visit_coercion(&mut self, coercion: &Coercion) {
        match coercion {
            Coercion::Type(type_) => self.visit_type(type_),
            Coercion::Nat(_) => (),
        }
    }

    fn visit_array_expr(&mut self, array_expr: &ArrayExpr) {
        self.visit_expr(&array_expr.expr);
        for subscript in &array_expr.subscripts {
            self.visit_array_subscript(subscript);
        }
    }

    fn visit_array_subscript(&mut self, array_subscript: &ArraySubscript) {
        self.visit_exprs(&array_subscript.keys);
        if let Some(value) = &array_subscript.value {
            self.visit_expr(&value);
        }
    }

    fn visit_func_call(&mut self, func_call: &FuncCall) {
        // TODO(spinda): It would be cool if we could do something like, drop
        // uninterpreted functions that are only referenced in assumes with
        // expressions that don't put them in relation to anything else. That'd
        // require modeling the expression structure in the reference graph,
        // then doing a pattern-matching optimization pass over the graph. Or
        // perhaps this would be better suited for an optimization inside Boogie
        // itself. Generically optimizing Boogie programs for verification time
        // would be an interesting research project in itself...
        self.reference_ident(func_call.target, Namespace::Func);
        self.visit_exprs(&func_call.args);
    }

    fn visit_old_expr(&mut self, old_expr: &OldExpr) {
        self.visit_expr(&old_expr.expr);
    }

    fn visit_arith_coercion_expr(&mut self, arith_coercion_expr: &ArithCoercionExpr) {
        self.visit_expr(&arith_coercion_expr.expr);
    }

    fn visit_quant_expr(&mut self, quant_expr: &QuantExpr) {
        self.visit_quant_body(&quant_expr.body);
    }

    fn visit_quant_body(&mut self, quant_body: &QuantBody) {
        let mut quant_body_visitor = self.nest();
        quant_body_visitor.visit_type_params(&quant_body.type_params);
        quant_body_visitor.visit_bound_vars(&quant_body.bound_vars);
        quant_body_visitor.visit_attrs(&quant_body.attrs);
        quant_body_visitor.visit_expr(&quant_body.expr);
    }

    fn visit_bound_vars(&mut self, bound_vars: &BoundVars) {
        self.visit_attr_typed_idents_wheres(bound_vars, Scope::Local);
    }

    fn visit_if_then_else_expr(&mut self, if_then_else_expr: &IfThenElseExpr) {
        self.visit_expr(&if_then_else_expr.cond);
        self.visit_expr(&if_then_else_expr.then);
        self.visit_expr(&if_then_else_expr.else_);
    }

    fn visit_code_expr(&mut self, code_expr: &CodeExpr) {
        let mut code_expr_visitor = self.nest();
        code_expr_visitor.visit_local_vars_list(&code_expr.local_vars);
        for spec_block in &code_expr.spec_blocks {
            code_expr_visitor.visit_spec_block(spec_block);
        }
    }

    fn visit_spec_block(&mut self, spec_block: &SpecBlock) {
        for cmd in &spec_block.cmds {
            self.visit_label_or_cmd(cmd);
        }
        self.visit_spec_transfer(&spec_block.transfer);
    }

    fn visit_spec_transfer(&mut self, spec_transfer: &SpecTransfer) {
        match spec_transfer {
            SpecTransfer::Goto(spec_goto) => self.visit_spec_goto(spec_goto),
            SpecTransfer::Return(spec_return) => self.visit_spec_return(spec_return),
        }
    }

    fn visit_spec_goto(&mut self, SpecGoto { labels: _labels }: &SpecGoto) {
        // This only references labels, which we don't care about.
    }

    fn visit_spec_return(&mut self, spec_return: &SpecReturn) {
        self.visit_expr(&spec_return.value);
    }

    fn visit_attr_typed_idents_wheres(
        &mut self,
        attr_typed_idents_wheres: &AttrTypedIdentsWheres,
        scope: Scope,
    ) {
        for attr_typed_idents_where in attr_typed_idents_wheres {
            self.visit_typed_idents(
                &attr_typed_idents_where
                    .value
                    .typed_idents_where
                    .typed_idents,
                scope,
            );
            self.visit_attrs(&attr_typed_idents_where.value.attrs);
        }

        for attr_typed_idents_where in attr_typed_idents_wheres {
            if let Some(where_) = &attr_typed_idents_where.value.typed_idents_where.where_ {
                self.visit_expr(where_);
            }
        }
    }

    #[allow(dead_code)]
    fn visit_attr_typed_idents_where(
        &mut self,
        attr_typed_idents_where: &AttrTypedIdentsWhere,
        scope: Scope,
    ) {
        self.visit_typed_idents_where(&attr_typed_idents_where.typed_idents_where, scope);
        self.visit_attrs(&attr_typed_idents_where.attrs);
    }

    #[allow(dead_code)]
    fn visit_typed_idents_wheres(
        &mut self,
        typed_idents_wheres: &TypedIdentsWheres,
        scope: Scope,
    ) {
        for typed_idents_where in typed_idents_wheres {
            self.visit_typed_idents(&typed_idents_where.value.typed_idents, scope);
        }

        for typed_idents_where in typed_idents_wheres {
            if let Some(where_) = &typed_idents_where.value.where_ {
                self.visit_expr(where_);
            }
        }
    }

    fn visit_typed_idents_where(&mut self, typed_idents_where: &TypedIdentsWhere, scope: Scope) {
        self.visit_typed_idents(&typed_idents_where.typed_idents, scope);
        if let Some(where_) = &typed_idents_where.where_ {
            self.visit_expr(where_);
        }
    }

    fn visit_typed_idents(&mut self, typed_idents: &TypedIdents, scope: Scope) {
        self.visit_idents(
            &typed_idents.idents,
            Namespace::Var,
            OccurrenceKind::Decl(scope),
        );
        self.visit_type(&typed_idents.type_);
    }

    fn visit_idents(
        &mut self,
        idents: &Idents,
        namespace: Namespace,
        occurrence_kind: OccurrenceKind,
    ) {
        for ident in idents {
            self.declare_or_reference_ident(*ident, namespace, occurrence_kind);
        }
    }

    fn visit_type_params(&mut self, type_params: &TypeParams) {
        self.visit_idents(
            &type_params.params,
            Namespace::Type,
            OccurrenceKind::Decl(Scope::Local),
        );
    }

    fn visit_attrs<'b>(&mut self, attrs: impl IntoIterator<Item = &'b Attr>) {
        for attr in attrs.into_iter() {
            self.visit_attr(attr);
        }
    }

    fn visit_attr(&mut self, attr: &Attr) {
        self.visit_attr_or_trigger(attr);
    }

    fn visit_attr_or_trigger(&mut self, attr_or_trigger: &AttrOrTrigger) {
        match attr_or_trigger {
            AttrOrTrigger::Attr(attr_content) => self.visit_attr_content(attr_content),
            AttrOrTrigger::Trigger(exprs) => self.visit_exprs(exprs),
        }
    }

    fn visit_attr_content(&mut self, attr_content: &AttrContent) {
        for attr_param in &attr_content.params {
            self.visit_attr_param(attr_param);
        }
    }

    fn visit_attr_param(&mut self, attr_param: &AttrParam) {
        match attr_param {
            AttrParam::String(_) => (),
            AttrParam::Expr(expr) => self.visit_expr(expr),
        }
    }

    fn declare_or_reference_ident(
        &mut self,
        ident: Spanned<Ident>,
        namespace: Namespace,
        occurrence_kind: OccurrenceKind,
    ) {
        match occurrence_kind {
            OccurrenceKind::Decl(scope) => self.declare_ident(ident, namespace, scope),
            OccurrenceKind::Ref => self.reference_ident(ident.value, namespace),
        }
    }

    fn declare_ident(&mut self, ident: Spanned<Ident>, namespace: Namespace, scope: Scope) {
        match scope {
            Scope::Global => self.declare_global_ident(ident, namespace),
            Scope::Local => self.declare_local_ident(ident.value, namespace),
        }
    }

    fn declare_global_ident(&mut self, ident: Spanned<Ident>, namespace: Namespace) {
        let node_index = self
            .visitor
            .graph
            .get_node_with_ident(ident.map(|ident| NamespacedIdent(ident, namespace)));
        self.visitor
            .graph
            .attach_root_to_node(self.parent_node_index, node_index);
    }

    fn declare_local_ident(&mut self, ident: Ident, namespace: Namespace) {
        self.local_idents
            .to_mut()
            .insert(NamespacedIdent(ident, namespace));
    }

    fn reference_ident(&mut self, ident: Ident, namespace: Namespace) {
        let ident = NamespacedIdent(ident, namespace);
        if self.local_idents.contains(&ident) {
            return;
        }
        let referenced_index = self
            .visitor
            .graph
            .get_node_with_ident(Spanned::new(Span::default(), ident));
        self.visitor
            .graph
            .add_edge(self.parent_node_index, referenced_index);
    }
}

fn func_decl_is_ctor(func_decl: &FuncDecl) -> bool {
    lazy_static! {
        static ref CTOR_ATTR_IDENT: Ident = Ident::from("constructor");
    }

    has_attr(&func_decl.attrs, *CTOR_ATTR_IDENT)
}

fn extract_data_type_ident_from_type(type_: &Type) -> Option<Ident> {
    match type_ {
        Type::Atom(type_atom) => match type_atom {
            TypeAtom::Int | TypeAtom::Real | TypeAtom::Bool => None,
            TypeAtom::Paren(inner_type) => extract_data_type_ident_from_type(&*inner_type),
        },
        Type::App(type_app) => Some(type_app.head),
        Type::Map(_) => None,
    }
}

struct ReachabilityMap {
    dfs: Dfs<NodeIndex, <RawReachabilityGraph as Visitable>::Map>,
}

impl ReachabilityMap {
    fn new(graph: &ReachabilityGraph) -> Self {
        Self {
            dfs: Dfs::empty(&graph.graph),
        }
    }

    fn mark_reachable(&mut self, graph: &ReachabilityGraph, node_index: NodeIndex) {
        self.dfs.move_to(node_index);
        while let Some(_) = self.dfs.next(&graph.graph) {}
    }

    fn is_reachable(&self, node_index: NodeIndex) -> bool {
        self.dfs.discovered.is_visited(&node_index)
    }
}
