// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::collections::HashMap;
use std::iter;
use std::ops::{Deref, Index};

use derive_more::{Display, From};
use enum_iterator::IntoEnumIterator;
use enum_map::EnumMap;
use enumset::EnumSet;
use fix_hidden_lifetime_bug::Captures;
use iterate::iterate;
use typed_index_collections::{TiSlice, TiVec};

use cachet_lang::ast::{BinOper, CheckKind, ForInOrder, Ident, NegateKind, Path, VarParamKind};
use cachet_lang::built_in::{BuiltInType, IdentEnum};
use cachet_lang::normalizer::{self, HasAttrs, Typed};
use cachet_util::MaybeOwned;

use crate::cpp::ast::*;

mod ast;

#[derive(Clone, Debug)]
pub struct CppCompilerOutput {
    pub decls: CppCode,
    pub defs: CppCode,
}

#[derive(Clone, Debug, Display)]
pub struct CppCode(Code);

pub fn compile(env: &normalizer::Env) -> CppCompilerOutput {
    let mut compiler = Compiler::new(env);

    for enum_index in env.enum_items.keys() {
        compiler.compile_enum_item(enum_index);
    }

    for struct_index in env.struct_items.keys() {
        compiler.compile_struct_item(struct_index);
    }

    for global_var_index in env.global_var_items.keys() {
        compiler.compile_global_var_item(global_var_index);
    }

    for fn_index in env.fn_items.keys() {
        compiler.compile_callable_item(fn_index.into());
    }

    for op_index in env.op_items.keys() {
        compiler.compile_callable_item(op_index.into());
    }

    let decls = iterate![
        CommentItem {
            text: "External forward declarations.".to_owned(),
        }
        .into(),
        ..compiler.external_decls,
        CommentItem {
            text: "Internal forward declarations.".to_owned(),
        }
        .into(),
        ..compiler.internal_decls,
    ]
    .collect();

    let defs = iterate![
        PragmaItem {
            content: "GCC diagnostic push".to_owned(),
        }
        .into(),
        PragmaItem {
            content: "GCC diagnostic ignored \"-Wunused-function\"".to_owned(),
        }
        .into(),
        PragmaItem {
            content: "GCC diagnostic ignored \"-Wunused-parameter\"".to_owned(),
        }
        .into(),
        PragmaItem {
            content: "GCC diagnostic ignored \"-Wunused-variable\"".to_owned(),
        }
        .into(),
        CommentItem {
            text: "Internal definitions.".to_owned(),
        }
        .into(),
        ..compiler.internal_defs,
        PragmaItem {
            content: "GCC diagnostic pop".to_owned(),
        }
        .into(),
    ]
    .collect();

    CppCompilerOutput {
        decls: CppCode(decls),
        defs: CppCode(defs),
    }
}

#[derive(Clone, Copy, From)]
enum ItemBucketIndex {
    #[from(types(BuiltInType, normalizer::EnumIndex, normalizer::StructIndex))]
    Type(normalizer::TypeIndex),
    #[from(types(normalizer::IrIndex))]
    Ir(IrItemBucketIndex),
    TopLevel,
}

impl From<normalizer::ParentIndex> for ItemBucketIndex {
    fn from(parent_index: normalizer::ParentIndex) -> Self {
        match parent_index {
            normalizer::ParentIndex::Type(type_index) => type_index.into(),
            normalizer::ParentIndex::Ir(ir_index) => ir_index.into(),
        }
    }
}

impl From<Option<normalizer::ParentIndex>> for ItemBucketIndex {
    fn from(parent_index: Option<normalizer::ParentIndex>) -> Self {
        match parent_index {
            Some(parent_index) => parent_index.into(),
            None => Self::TopLevel,
        }
    }
}

#[derive(Clone, Copy)]
struct IrItemBucketIndex {
    ir_index: normalizer::IrIndex,
    selector: Option<IrMemberFlagSelector>,
}

impl From<normalizer::IrIndex> for IrItemBucketIndex {
    fn from(ir_index: normalizer::IrIndex) -> Self {
        Self {
            ir_index,
            selector: None,
        }
    }
}

#[derive(Clone)]
struct ItemBuckets {
    built_in_type_namespace_items: HashMap<BuiltInType, NamespaceItem>,
    enum_namespace_items: TiVec<normalizer::EnumIndex, NamespaceItem>,
    struct_namespace_items: TiVec<normalizer::StructIndex, NamespaceItem>,
    ir_item_buckets: TiVec<normalizer::IrIndex, IrItemBuckets>,
    top_level_items: Vec<Item>,
}

impl ItemBuckets {
    fn new(
        enum_items: &TiSlice<normalizer::EnumIndex, normalizer::EnumItem>,
        struct_items: &TiSlice<normalizer::StructIndex, normalizer::StructItem>,
        ir_items: &TiSlice<normalizer::IrIndex, normalizer::IrItem>,
    ) -> Self {
        let init_namespace_item = |ident: Ident| NamespaceItem {
            ident: NamespaceIdent {
                kind: NamespaceKind::Impl,
                ident,
            },
            items: Vec::new(),
        };

        ItemBuckets {
            built_in_type_namespace_items: BuiltInType::into_enum_iter()
                .map(|built_in_type| (built_in_type, init_namespace_item(built_in_type.ident())))
                .collect(),
            enum_namespace_items: enum_items
                .iter()
                .map(|enum_item| init_namespace_item(enum_item.ident.value))
                .collect(),
            struct_namespace_items: struct_items
                .iter()
                .map(|struct_item| init_namespace_item(struct_item.ident.value))
                .collect(),
            ir_item_buckets: ir_items
                .iter()
                .map(|ir_item| IrItemBuckets::new(ir_item.ident.value))
                .collect(),
            top_level_items: Vec::new(),
        }
    }

    fn bucket_for(&mut self, item_bucket_index: ItemBucketIndex) -> &mut Vec<Item> {
        match item_bucket_index {
            ItemBucketIndex::Type(type_index) => self.bucket_for_type(type_index),
            ItemBucketIndex::Ir(ir_item_bucket_index) => self.bucket_for_ir(ir_item_bucket_index),
            ItemBucketIndex::TopLevel => &mut self.top_level_items,
        }
    }

    fn bucket_for_type(&mut self, type_index: normalizer::TypeIndex) -> &mut Vec<Item> {
        match type_index {
            normalizer::TypeIndex::BuiltIn(built_in_type) => {
                self.bucket_for_built_in_type(built_in_type)
            }
            normalizer::TypeIndex::Enum(enum_index) => self.bucket_for_enum(enum_index),
            normalizer::TypeIndex::Struct(struct_index) => self.bucket_for_struct(struct_index),
        }
    }

    fn bucket_for_built_in_type(&mut self, built_in_type: BuiltInType) -> &mut Vec<Item> {
        &mut self
            .built_in_type_namespace_items
            .get_mut(&built_in_type)
            .unwrap()
            .items
    }

    fn bucket_for_enum(&mut self, enum_index: normalizer::EnumIndex) -> &mut Vec<Item> {
        &mut self.enum_namespace_items[enum_index].items
    }

    fn bucket_for_struct(&mut self, struct_index: normalizer::StructIndex) -> &mut Vec<Item> {
        &mut self.struct_namespace_items[struct_index].items
    }

    fn bucket_for_ir(&mut self, ir_item_bucket_index: IrItemBucketIndex) -> &mut Vec<Item> {
        self.ir_item_buckets[ir_item_bucket_index.ir_index]
            .bucket_for(ir_item_bucket_index.selector)
    }
}

impl IntoIterator for ItemBuckets {
    type Item = Item;
    type IntoIter = std::vec::IntoIter<Item>;

    fn into_iter(self) -> Self::IntoIter {
        let ItemBuckets {
            built_in_type_namespace_items,
            enum_namespace_items,
            struct_namespace_items,
            ir_item_buckets,
            top_level_items,
        } = self;

        iterate![
            ..iterate![
                ..built_in_type_namespace_items
                    .into_iter()
                    .map(|(_, items)| items),
                ..enum_namespace_items,
                ..struct_namespace_items,
            ]
            .filter(|namespace_item| !namespace_item.items.is_empty())
            .map(Into::into),
            ..ir_item_buckets.into_iter().flatten(),
            ..top_level_items
        ]
        .collect::<Vec<_>>()
        .into_iter()
    }
}

#[derive(Clone)]
struct IrItemBuckets {
    ident: Ident,
    items_for_selectors: EnumMap<IrMemberFlagSelector, Vec<Item>>,
    misc_items: Vec<Item>,
}

impl IrItemBuckets {
    fn new(ident: Ident) -> Self {
        IrItemBuckets {
            ident,
            items_for_selectors: EnumMap::default(),
            misc_items: Vec::new(),
        }
    }

    fn bucket_for(&mut self, selector: Option<IrMemberFlagSelector>) -> &mut Vec<Item> {
        match selector {
            Some(selector) => &mut self.items_for_selectors[selector],
            None => &mut self.misc_items,
        }
    }
}

impl IntoIterator for IrItemBuckets {
    type Item = Item;
    type IntoIter = std::vec::IntoIter<Item>;

    fn into_iter(self) -> Self::IntoIter {
        let IrItemBuckets {
            ident,
            items_for_selectors,
            misc_items,
        } = self;

        let impl_namespace_ident = NamespaceIdent {
            kind: NamespaceKind::Impl,
            ident,
        };

        let if_def_items = items_for_selectors
            .into_iter()
            .filter_map(move |(selector, items)| {
                if items.is_empty() {
                    None
                } else {
                    Some(
                        IfDefItem {
                            cond: IrMemberFlagIdent { ident, selector }.into(),
                            then: vec![
                                NamespaceItem {
                                    ident: impl_namespace_ident,
                                    items,
                                }
                                .into(),
                            ]
                            .into(),
                        }
                        .into(),
                    )
                }
            });

        let misc_namespace_item = if misc_items.is_empty() {
            None
        } else {
            Some(
                NamespaceItem {
                    ident: impl_namespace_ident,
                    items: misc_items,
                }
                .into(),
            )
        };

        iterate![..if_def_items, ..misc_namespace_item]
            .collect::<Vec<_>>()
            .into_iter()
    }
}

const CONTEXT_PARAM: Param = Param {
    ident: ParamVarIdent::Context,
    type_: Type::Path(TypePath::Helper(HelperTypeIdent::ContextRef)),
};
const CONTEXT_ARG: Expr = Expr::Var(VarIdent::Param(CONTEXT_PARAM.ident));

struct Compiler<'a> {
    env: &'a normalizer::Env,
    external_decls: ItemBuckets,
    internal_decls: ItemBuckets,
    internal_defs: ItemBuckets,
}

impl<'a> Compiler<'a> {
    fn new(env: &'a normalizer::Env) -> Self {
        let external_decls = ItemBuckets::new(&env.enum_items, &env.struct_items, &env.ir_items);
        let internal_decls = external_decls.clone();
        let internal_defs = external_decls.clone();

        Compiler {
            env,
            external_decls,
            internal_decls,
            internal_defs,
        }
    }

    fn compile_enum_item(&mut self, enum_index: normalizer::EnumIndex) {
        let enum_item = &self.env[enum_index];
        if enum_item.is_prelude() || enum_item.is_spec() {
            return;
        }

        let variant_decls = enum_item.variants.iter().map(|variant_path| {
            let path = TypeMemberFnPath {
                parent: enum_item.ident.value,
                ident: VariantTypeMemberFnIdent::from(variant_path.value.ident()).into(),
            }
            .into();

            let ret = TypeMemberTypePath {
                parent: enum_item.ident.value,
                ident: ExprTag::Ref.into(),
            }
            .into();

            FnItem {
                path,
                is_fully_qualified: false,
                is_inline: true,
                params: vec![CONTEXT_PARAM],
                ret,
                body: None,
            }
            .into()
        });

        self.external_decls
            .bucket_for_enum(enum_index)
            .extend(variant_decls);
    }

    fn compile_struct_item(&mut self, struct_index: normalizer::StructIndex) {
        let struct_item = &self.env[struct_index];
        if struct_item.is_prelude() || struct_item.is_spec() {
            return;
        }

        if let Some(supertype_index) = struct_item.supertype {
            let supertype_ident = self.get_type_ident(supertype_index);

            self.external_decls
                .bucket_for_type(supertype_index)
                .extend([
                    generate_cast_fn_decl(supertype_ident, struct_item.ident.value, ExprTag::Val)
                        .into(),
                    generate_cast_fn_decl(supertype_ident, struct_item.ident.value, ExprTag::Ref)
                        .into(),
                ]);

            self.external_decls.bucket_for_struct(struct_index).extend([
                generate_cast_fn_decl(struct_item.ident.value, supertype_ident, ExprTag::Val)
                    .into(),
                generate_cast_fn_decl(struct_item.ident.value, supertype_ident, ExprTag::Ref)
                    .into(),
            ]);
        }

        // Add declarations for struct field accessor functions.
        let field_decls: Vec<_> = struct_item
            .fields
            .iter()
            .map(|field| {
                let path = TypeMemberFnPath {
                    parent: struct_item.ident.value,
                    ident: FieldAccessTypeMemberFnIdent {
                        field: field.ident().value,
                    }
                    .into(),
                }
                .into();

                let ret = match field {
                    normalizer::Field::Var(var_field) => TypeMemberTypePath {
                        parent: self.get_type_ident(var_field.type_),
                        ident: ExprTag::Ref.into(),
                    }
                    .into(),
                    normalizer::Field::Label(label) => IrMemberTypePath {
                        parent: self.env[label.ir].ident.value,
                        ident: IrMemberTypeIdent::LabelRef,
                    }
                    .into(),
                };

                let parent_param = Param {
                    ident: ParamVarIdent::Parent,
                    type_: TypeMemberTypePath {
                        parent: struct_item.ident.value,
                        ident: ExprTag::Ref.into(),
                    }
                    .into(),
                };

                FnItem {
                    path,
                    is_fully_qualified: false,
                    // TODO(abhishekc-sharma): Can/should this be inline?
                    is_inline: false,
                    params: vec![CONTEXT_PARAM, parent_param],
                    ret,
                    body: None,
                }
                .into()
            })
            .collect();

        self.external_decls
            .bucket_for_struct(struct_index)
            .extend(field_decls);
    }

    fn compile_global_var_item(&mut self, global_var_index: normalizer::GlobalVarIndex) {
        let global_var_item = &self.env[global_var_index];
        if global_var_item.is_prelude() || global_var_item.is_spec() {
            return;
        }

        let global_var_parent_ident = global_var_item.path.value.parent().map(Path::ident);
        let global_var_ident = global_var_item.path.value.ident();
        let path = GlobalVarFnPath {
            parent: global_var_parent_ident,
            ident: global_var_ident.into(),
        }
        .into();

        let type_ident = self.get_type_ident(global_var_item.type_);

        let ret = TypeMemberTypePath {
            parent: type_ident,
            ident: global_var_tag(global_var_item.is_mut).into(),
        }
        .into();

        let fn_decl = FnItem {
            path,
            is_fully_qualified: false,
            is_inline: true,
            params: vec![CONTEXT_PARAM],
            ret,
            body: None,
        };

        let item_bucket_index = global_var_item.parent.into();

        match &global_var_item.value {
            Some(value) => {
                let mut scoped_compiler = ScopedCompiler::new(self, None, Scope::Empty);
                let value_tagged_expr = scoped_compiler.compile_expr(&value.value);
                let value_expr = scoped_compiler.use_expr(value_tagged_expr, ExprTag::Val.into());
                let mut body = Block::from(scoped_compiler.stmts);
                body.stmts.push(
                    RetStmt {
                        value: Some(value_expr),
                    }
                    .into(),
                );

                let mut fn_def = fn_decl.clone();
                fn_def.body = Some(body);

                self.internal_decls
                    .bucket_for(item_bucket_index)
                    .push(fn_decl.into());
                self.internal_defs
                    .bucket_for(item_bucket_index)
                    .push(fn_def.into());
            }
            None => {
                self.external_decls
                    .bucket_for(item_bucket_index)
                    .push(fn_decl.into());
            }
        }
    }

    fn compile_callable_item(&mut self, callable_index: normalizer::CallableIndex) {
        let callable_item = &self.env[callable_index];
        if callable_item.is_prelude() || callable_item.is_spec() {
            return;
        }

        if let normalizer::CallableIndex::Fn(_) = callable_index {
            match callable_item.parent {
                Some(normalizer::ParentIndex::Type(normalizer::TypeIndex::Enum(enum_index))) => {
                    let enum_item = &self.env[enum_index];
                    if enum_item.is_spec() {
                        return;
                    }
                }
                Some(normalizer::ParentIndex::Type(normalizer::TypeIndex::Struct(
                    struct_index,
                ))) => {
                    let struct_item = &self.env[struct_index];
                    if struct_item.is_spec() {
                        return;
                    }
                }
                _ => (),
            }
        }

        let callable_parent_ident = callable_item.path.value.parent().map(Path::ident);
        let callable_ident = callable_item.path.value.ident();
        let path = UserFnPath {
            parent: callable_parent_ident,
            ident: match callable_index {
                normalizer::CallableIndex::Fn(_) => FnUserFnIdent::from(callable_ident).into(),
                normalizer::CallableIndex::Op(_) => OpUserFnIdent::from(callable_ident).into(),
            },
        }
        .into();

        let params = iter::once(CONTEXT_PARAM)
            .chain(callable_item.emits.map(|ir_index| {
                Param {
                    ident: ParamVarIdent::Ops,
                    type_: IrMemberTypePath {
                        parent: self.env[ir_index].ident.value,
                        ident: IrMemberTypeIdent::OpsRef,
                    }
                    .into(),
                }
            }))
            .chain(self.compile_params(&callable_item.params, &callable_item.param_order))
            .collect();

        let ret = match callable_item.ret {
            Some(ret) => TypeMemberTypePath {
                parent: self.get_type_ident(ret),
                ident: ExprTag::Val.into(),
            }
            .into(),
            None => Type::Void,
        };

        let fn_decl = FnItem {
            path,
            is_fully_qualified: false,
            is_inline: false,
            params,
            ret,
            body: None,
        };

        let item_bucket_index = match callable_index {
            normalizer::CallableIndex::Fn(_) => callable_item.parent.into(),
            normalizer::CallableIndex::Op(_) => {
                let ir_index = match callable_item.parent {
                    Some(normalizer::ParentIndex::Ir(ir_index)) => ir_index,
                    _ => panic!("op with no parent IR"),
                };
                let ir_item = &self.env[ir_index];
                let ir_ident = ir_item.ident.value;

                // Declare emit helper functions for ops.

                let mut emit_fn_decl = fn_decl.clone();

                emit_fn_decl.path = IrMemberFnPath {
                    parent: ir_ident,
                    ident: EmitIrMemberFnIdent::from(callable_ident).into(),
                }
                .into();

                emit_fn_decl
                    .params
                    .retain(|param| !matches!(param.ident, ParamVarIdent::Ops));

                emit_fn_decl.params.insert(
                    1,
                    Param {
                        ident: ParamVarIdent::Ops,
                        type_: IrMemberTypePath {
                            parent: ir_ident,
                            ident: IrMemberTypeIdent::OpsRef,
                        }
                        .into(),
                    },
                );

                self.external_decls
                    .bucket_for_ir(IrItemBucketIndex {
                        ir_index,
                        selector: Some(IrMemberFlagSelector::Emit),
                    })
                    .push(emit_fn_decl.into());

                // Put the rest of the compiled items in either the interpreter
                // or compiler bucket.

                IrItemBucketIndex {
                    ir_index,
                    selector: Some(match ir_item.emits {
                        None => IrMemberFlagSelector::Interpreter,
                        Some(_) => IrMemberFlagSelector::Compiler,
                    }),
                }
                .into()
            }
        };

        if let normalizer::CallableIndex::Fn(_) = callable_index {
            if callable_item.is_refined() {
                self.external_decls
                    .bucket_for(item_bucket_index)
                    .push(fn_decl.into());

                return;
            }
        }

        match &callable_item.body {
            None => {
                self.external_decls
                    .bucket_for(item_bucket_index)
                    .push(fn_decl.into());
            }
            Some(body) => {
                let mut fn_def = fn_decl.clone();
                fn_def.is_fully_qualified = true;
                fn_def.body = Some(self.compile_body(&callable_item, body));

                self.internal_decls
                    .bucket_for(item_bucket_index)
                    .push(fn_decl.into());
                self.internal_defs
                    .bucket_for(item_bucket_index)
                    .push(fn_def.into());
            }
        }
    }

    fn compile_params<'b>(
        &'b mut self,
        params: &'b normalizer::Params,
        param_order: &'b [normalizer::ParamIndex],
    ) -> impl 'b + Iterator<Item = Param> + Captures<'a> {
        param_order
            .iter()
            .map(|param_index| self.compile_param(params, *param_index))
    }

    fn compile_param(
        &mut self,
        params: &normalizer::Params,
        param_index: normalizer::ParamIndex,
    ) -> Param {
        match param_index {
            normalizer::ParamIndex::Var(var_param_index) => {
                self.compile_var_param(&params[var_param_index])
            }
            normalizer::ParamIndex::Label(label_param_index) => {
                self.compile_label_param(&params[label_param_index])
            }
        }
    }

    fn compile_var_param(&mut self, var_param: &normalizer::VarParam) -> Param {
        let ident = UserParamVarIdent::from(var_param.ident.value).into();

        let type_ = TypeMemberTypePath {
            parent: self.get_type_ident(var_param.type_),
            ident: match var_param.kind {
                VarParamKind::In | VarParamKind::Mut => ExprTag::Ref,
                VarParamKind::Out => ExprTag::MutRef,
            }
            .into(),
        }
        .into();

        Param { ident, type_ }
    }

    fn compile_label_param(&mut self, label_param: &normalizer::LabelParam) -> Param {
        let ident = UserParamVarIdent::from(label_param.label.ident.value).into();

        let type_ = IrMemberTypePath {
            parent: self.env[label_param.label.ir].ident.value,
            ident: if label_param.is_out {
                IrMemberTypeIdent::LabelMutRef
            } else {
                IrMemberTypeIdent::LabelRef
            },
        }
        .into();

        Param { ident, type_ }
    }

    fn compile_body(
        &mut self,
        callable: &normalizer::CallableItem,
        body: &normalizer::Body,
    ) -> Block {
        let mut scoped_compiler = ScopedCompiler::new(self, callable.parent, callable.into());
        scoped_compiler.init_mut_var_params(&callable.param_order);
        scoped_compiler.compile_stmts(&body.stmts);
        scoped_compiler.stmts.into()
    }

    fn get_type_ident(&self, type_index: normalizer::TypeIndex) -> Ident {
        match type_index {
            normalizer::TypeIndex::BuiltIn(built_in_type) => built_in_type.ident(),
            normalizer::TypeIndex::Enum(enum_index) => self.env[enum_index].ident.value,
            normalizer::TypeIndex::Struct(struct_index) => self.env[struct_index].ident.value,
        }
    }
}

fn generate_cast_fn_decl(
    source_type_ident: Ident,
    target_type_ident: Ident,
    tag: ExprTag,
) -> FnItem {
    let source_type_path = TypeMemberTypePath {
        parent: source_type_ident,
        ident: tag.into(),
    };
    let target_type_path = TypeMemberTypePath {
        parent: target_type_ident,
        ident: tag.into(),
    };

    let mut param_type = source_type_path.into();
    if let ExprTag::Val = tag {
        param_type = RefType {
            inner: param_type,
            value_category: ValueCategory::RValue,
        }
        .into();
    }

    FnItem {
        path: TypeMemberFnPath {
            parent: source_type_ident,
            ident: CastTypeMemberFnIdent {
                target_type_ident,
                tag,
            }
            .into(),
        }
        .into(),
        is_fully_qualified: false,
        is_inline: true,
        params: vec![Param {
            ident: ParamVarIdent::In,
            type_: param_type,
        }],
        ret: target_type_path.into(),
        body: None,
    }
}

#[derive(Copy, Clone)]
enum Scope<'b> {
    Empty,
    Body {
        params: &'b normalizer::Params,
        locals: &'b normalizer::Locals,
    },
}

impl<'b> Scope<'b> {
    pub fn local<I>(&self, index: I) -> &<normalizer::Locals as Index<I>>::Output
    where
        normalizer::Locals: Index<I>,
    {
        match self {
            Scope::Empty => panic!("Attempted access a local var within an empty scope"),
            Scope::Body { locals, .. } => &locals[index],
        }
    }

    pub fn param<I>(&self, index: I) -> &<normalizer::Params as Index<I>>::Output
    where
        normalizer::Params: Index<I>,
    {
        match self {
            Scope::Empty => panic!("Attempted access a param var within an empty scope"),
            Scope::Body { params, .. } => &params[index],
        }
    }
}

impl<'b> From<&'b normalizer::CallableItem> for Scope<'b> {
    fn from(callable: &'b normalizer::CallableItem) -> Self {
        match &callable.body {
            Some(body) => Scope::Body {
                params: &callable.params,
                locals: &body.locals,
            },
            None => Self::Empty,
        }
    }
}

struct ScopedCompiler<'a, 'b> {
    compiler: &'b Compiler<'a>,
    parent_index: Option<normalizer::ParentIndex>,
    scope: Scope<'b>,
    next_synthetic_var_indexes: MaybeOwned<'b, EnumMap<SyntheticVarKind, usize>>,
    stmts: Vec<Stmt>,
}

impl<'a> Deref for ScopedCompiler<'a, '_> {
    type Target = Compiler<'a>;

    fn deref(&self) -> &Self::Target {
        self.compiler
    }
}

impl<'a, 'b> ScopedCompiler<'a, 'b> {
    fn new(
        compiler: &'b Compiler<'a>,
        parent_index: Option<normalizer::ParentIndex>,
        scope: Scope<'b>,
    ) -> Self {
        ScopedCompiler {
            compiler,
            parent_index,
            scope,
            next_synthetic_var_indexes: MaybeOwned::Owned(EnumMap::default()),
            stmts: Vec::new(),
        }
    }

    fn recurse<'c>(&'c mut self) -> ScopedCompiler<'a, 'c> {
        ScopedCompiler {
            compiler: self.compiler,
            parent_index: self.parent_index,
            scope: self.scope,
            next_synthetic_var_indexes: MaybeOwned::Borrowed(&mut self.next_synthetic_var_indexes),
            stmts: Vec::new(),
        }
    }

    fn init_mut_var_params(&mut self, param_order: &[normalizer::ParamIndex]) {
        for param_index in param_order {
            if let normalizer::ParamIndex::Var(var_param_index) = param_index {
                let var_param = &self.scope.param(var_param_index);

                if var_param.kind == VarParamKind::Mut {
                    // Variable parameters are passed in as immutable `Ref`s. To
                    // let one be mutable within the function body, we store it
                    // in a local variable up front.

                    let lhs = UserParamVarIdent::from(var_param.ident.value).into();

                    let rhs = TaggedExpr {
                        expr: Expr::from(lhs),
                        type_: var_param.type_,
                        tags: ExprTag::Ref.into(),
                    };

                    self.init_local_var_with_rhs(lhs, rhs);
                }
            }
        }
    }

    fn compile_args<'c>(
        &'c mut self,
        args: &'c [normalizer::Arg],
    ) -> impl 'c + Iterator<Item = Expr> + Captures<'a> + Captures<'b> {
        args.iter().map(|arg| self.compile_arg(arg))
    }

    fn compile_arg(&mut self, arg: &normalizer::Arg) -> Expr {
        match arg {
            normalizer::Arg::Expr(pure_expr) => self.compile_expr_arg(pure_expr),
            normalizer::Arg::OutVar(out_var_arg) => self.compile_out_var_arg(out_var_arg),
            normalizer::Arg::Label(label_arg) => self.compile_label_arg(label_arg),
            normalizer::Arg::LabelField(label_field_arg) => {
                self.compile_label_field_arg(label_field_arg)
            }
        }
    }

    fn compile_expr_arg(&mut self, pure_expr: &normalizer::PureExpr) -> Expr {
        let tagged_expr = self.compile_pure_expr(pure_expr);
        self.use_expr(tagged_expr, ExprTag::Ref.into())
    }

    fn compile_out_var_arg(&self, out_var_arg: &normalizer::OutVarArg) -> Expr {
        // TODO(spinda): Handle out-parameter upcasting.
        // TODO(spinda): If a variable is referenced to fill both an in- and
        // out-parameter in the same call, we need to set up a separate space
        // for the out-parameter value to be written while the value is still
        // being used. This can reuse the machinery for out-parameter upcasting.

        self.use_expr(
            self.compile_var_access(out_var_arg.var),
            ExprTag::MutRef.into(),
        )
    }

    fn compile_label_arg(&self, label_arg: &normalizer::LabelArg) -> Expr {
        match label_arg.label {
            normalizer::LabelIndex::Param(label_param_index) => {
                UserParamVarIdent::from(self.scope.param(label_param_index).label.ident.value)
                    .into()
            }
            normalizer::LabelIndex::Local(local_label_index) => CallExpr {
                target: IrMemberFnPath {
                    parent: self.env[label_arg.ir].ident.value,
                    ident: if label_arg.is_out {
                        IrMemberFnIdent::ToLabelMutRef
                    } else {
                        IrMemberFnIdent::ToLabelRef
                    },
                }
                .into(),
                args: vec![
                    LocalLabelVarIdent {
                        ident: self.scope.local(local_label_index).ident.value,
                        index: local_label_index,
                    }
                    .into(),
                ],
            }
            .into(),
        }
    }

    fn compile_label_field_arg(&mut self, label_field_arg: &normalizer::LabelFieldArg) -> Expr {
        self.compile_field_access(&label_field_arg.parent, label_field_arg.field)
            .into()
    }

    fn compile_internal_label_arg(
        &self,
        label_index: normalizer::LabelIndex,
        is_out: bool,
    ) -> Expr {
        self.compile_label_arg(&normalizer::LabelArg {
            label: label_index,
            is_out,
            ir: self.get_label_ir(label_index),
        })
    }

    fn compile_block(&mut self, stmts: &[normalizer::Stmt]) -> Block {
        let mut scoped_compiler = self.recurse();
        scoped_compiler.compile_stmts(stmts);
        scoped_compiler.stmts.into()
    }

    fn compile_stmts(&mut self, stmts: &[normalizer::Stmt]) {
        self.stmts.reserve(stmts.len());
        for stmt in stmts {
            self.compile_stmt(stmt);
        }
    }

    fn compile_stmt(&mut self, stmt: &normalizer::Stmt) {
        match stmt {
            normalizer::Stmt::Let(let_stmt) => self.compile_let_stmt(let_stmt),
            normalizer::Stmt::Label(label_stmt) => self.compile_label_stmt(label_stmt),
            normalizer::Stmt::If(if_stmt) => self.compile_if_stmt(if_stmt),
            normalizer::Stmt::ForIn(for_in_stmt) => self.compile_for_in_stmt(for_in_stmt),
            normalizer::Stmt::Check(check_stmt) => self.compile_check_stmt(check_stmt),
            normalizer::Stmt::Goto(goto_stmt) => self.compile_goto_stmt(goto_stmt),
            normalizer::Stmt::Bind(bind_stmt) => self.compile_bind_stmt(bind_stmt),
            normalizer::Stmt::Emit(call) => self.compile_emit_stmt(call),
            normalizer::Stmt::Invoke(invoke_stmt) => self.compile_invoke_stmt(invoke_stmt),
            normalizer::Stmt::Assign(assign_stmt) => self.compile_assign_stmt(assign_stmt),
            normalizer::Stmt::Ret(ret_stmt) => self.compile_ret_stmt(ret_stmt),
            normalizer::Stmt::Unreachable => self.compile_unreachable_stmt(),
        }
    }

    fn compile_let_stmt(&mut self, let_stmt: &normalizer::LetStmt) {
        let lhs = LocalVarIdent {
            ident: self.scope.local(let_stmt.lhs).ident.value,
            index: let_stmt.lhs,
        }
        .into();

        let rhs = let_stmt.rhs.as_ref().map(|rhs| self.compile_expr(rhs));

        self.init_local_var(lhs, let_stmt.type_, rhs);
    }

    fn compile_label_stmt(&mut self, label_stmt: &normalizer::LabelStmt) {
        let local_label = &self.scope.local(label_stmt.label);
        let ir_ident = self.env[local_label.ir].ident.value;

        let lhs = LocalLabelVarIdent {
            ident: local_label.ident.value,
            index: label_stmt.label,
        }
        .into();

        let type_ = IrMemberTypePath {
            parent: ir_ident,
            ident: IrMemberTypeIdent::LabelLocal,
        }
        .into();

        let rhs = Some(
            CallExpr {
                target: IrMemberFnPath {
                    parent: ir_ident,
                    ident: IrMemberFnIdent::NewLabel,
                }
                .into(),
                args: vec![CONTEXT_ARG],
            }
            .into(),
        );

        self.stmts.push(LetStmt { lhs, type_, rhs }.into());
    }

    fn init_local_var(
        &mut self,
        lhs: VarIdent,
        type_index: normalizer::TypeIndex,
        rhs: Option<TaggedExpr>,
    ) {
        let type_ident = self.get_type_ident(type_index);

        let type_ = TypeMemberTypePath {
            parent: type_ident,
            ident: ExprTag::Local.into(),
        }
        .into();

        let rhs = match rhs {
            Some(rhs) => {
                assert!(rhs.type_ == type_index);
                self.use_expr(rhs, ExprTag::Local.into())
            }
            None => CallExpr {
                target: TypeMemberFnPath {
                    parent: type_ident,
                    ident: TypeMemberFnIdent::EmptyLocal,
                }
                .into(),
                args: vec![CONTEXT_ARG],
            }
            .into(),
        };

        self.stmts.push(
            LetStmt {
                lhs,
                type_,
                rhs: Some(rhs),
            }
            .into(),
        );
    }

    fn init_local_var_with_rhs(&mut self, lhs: VarIdent, rhs: TaggedExpr) {
        self.init_local_var(lhs, rhs.type_, Some(rhs));
    }

    fn compile_if_stmt_recurse(&mut self, if_stmt: &normalizer::IfStmt) -> IfStmt {
        let tagged_cond = self.compile_expr(&if_stmt.cond);
        let cond = self.use_expr(tagged_cond, ExprTag::Ref | ExprTag::Val);

        let then = self.compile_block(&if_stmt.then);

        let else_ = if_stmt.else_.as_ref().map(|else_| match else_ {
            normalizer::ElseClause::ElseIf(else_if) => {
                ast::ElseClause::ElseIf(Box::new(self.compile_if_stmt_recurse(&*else_if)))
            }
            normalizer::ElseClause::Else(else_block) => {
                ast::ElseClause::Else(self.compile_block(else_block))
            }
        });

        IfStmt { cond, then, else_ }.into()
    }

    fn compile_if_stmt(&mut self, if_stmt: &normalizer::IfStmt) {
        let if_ = self.compile_if_stmt_recurse(if_stmt);
        self.stmts.push(if_.into());
    }

    fn compile_for_in_stmt(&mut self, for_in_stmt: &normalizer::ForInStmt) {
        let enum_item = &self.env[for_in_stmt.target];

        let body = self.compile_block(&for_in_stmt.body);

        let iteration_stmts: Vec<_> = {
            let loop_var_index = for_in_stmt.var;
            let loop_var_ident = LocalVarIdent {
                ident: self.scope.local(loop_var_index).ident.value,
                index: loop_var_index,
            }
            .into();
            let loop_var_type = Type::from(TypeMemberTypePath {
                parent: self.get_type_ident(for_in_stmt.target.into()),
                ident: ExprTag::Local.into(),
            });

            enum_item
                .variants
                .iter_enumerated()
                .map(|(variant_index, _)| {
                    let rhs = Some(self.use_expr(
                        self.compile_enum_variant_access(normalizer::EnumVariantIndex {
                            enum_index: for_in_stmt.target,
                            variant_index,
                        }),
                        ExprTag::Local.into(),
                    ));
                    let loop_var_let_stmt = LetStmt {
                        lhs: loop_var_ident,
                        type_: loop_var_type.clone(),
                        rhs,
                    };

                    let body_stmts = body.stmts.iter().cloned();
                    Stmt::from(BlockStmt::from_iter(iterate![
                        loop_var_let_stmt.into(),
                        ..body_stmts,
                    ]))
                })
                .collect()
        };

        match for_in_stmt.order {
            ForInOrder::Ascending => self.stmts.extend(iteration_stmts),
            ForInOrder::Descending => self.stmts.extend(iteration_stmts.into_iter().rev()),
        };
    }

    fn compile_check_stmt(&mut self, check_stmt: &normalizer::CheckStmt) {
        match check_stmt.kind {
            CheckKind::Assert => {
                let tagged_cond = self.compile_expr(&check_stmt.cond);
                let cond = self.use_expr(tagged_cond, ExprTag::Ref | ExprTag::Val);

                self.stmts.push(
                    Expr::from(CallExpr {
                        target: HelperFnIdent::Assert.into(),
                        args: vec![cond],
                    })
                    .into(),
                );
            }
            CheckKind::Assume => (),
        }
    }

    /// Lowers to `Cachet_Unreachable()`, which the embedder defines as a
    /// `[[noreturn]]` crash (`MOZ_CRASH` in SpiderMonkey). It stands in for the
    /// valueless return C++ cannot express in a value-returning function, and it
    /// has to carry the check itself, since `Cachet_Assert` is `MOZ_ASSERT` and
    /// compiles away in release builds.
    fn compile_unreachable_stmt(&mut self) {
        self.stmts.push(
            Expr::from(CallExpr {
                target: HelperFnIdent::Unreachable.into(),
                args: Vec::new(),
            })
            .into(),
        );
    }

    fn compile_goto_stmt(&mut self, goto_stmt: &normalizer::GotoStmt) {
        let ir_ident = self.env[goto_stmt.ir].ident.value;
        let target = IrMemberFnPath {
            parent: ir_ident,
            ident: IrMemberFnIdent::GotoLabel,
        }
        .into();

        let args = vec![
            CONTEXT_ARG,
            self.compile_internal_label_arg(goto_stmt.label, false),
        ];

        self.stmts.extend([
            Expr::from(CallExpr { target, args }).into(),
            RetStmt { value: None }.into(),
        ]);
    }

    fn compile_bind_stmt(&mut self, bind_stmt: &normalizer::BindStmt) {
        let ir_ident = self.env[bind_stmt.ir].ident.value;
        let target = IrMemberFnPath {
            parent: ir_ident,
            ident: IrMemberFnIdent::BindLabel,
        }
        .into();

        let args = vec![
            CONTEXT_ARG,
            ParamVarIdent::Ops.into(),
            self.compile_internal_label_arg(bind_stmt.label, true),
        ];

        self.stmts
            .push(Expr::from(CallExpr { target, args }).into());
    }

    fn compile_emit_stmt(&mut self, emit_stmt: &normalizer::EmitStmt) {
        let ir_ident = self.env[emit_stmt.ir].ident.value;
        let op_ident = self.env[emit_stmt.call.target].path.value.ident();
        let target = IrMemberFnPath {
            parent: ir_ident,
            ident: EmitIrMemberFnIdent::from(op_ident).into(),
        }
        .into();

        let args = [CONTEXT_ARG, ParamVarIdent::Ops.into()]
            .into_iter()
            .chain(self.compile_args(&emit_stmt.call.args))
            .collect();

        self.stmts
            .push(Expr::from(CallExpr { target, args }).into());
    }

    fn compile_invoke_stmt(&mut self, invoke_stmt: &normalizer::InvokeStmt) {
        let invoke_expr = self.compile_invoke_expr(invoke_stmt);
        // We can discard any tag information because any return value would be
        // unused.
        self.stmts.push(Expr::from(invoke_expr.expr).into());
    }

    fn compile_assign_stmt(&mut self, assign_stmt: &normalizer::AssignStmt) {
        let lhs = self.use_expr(
            self.compile_var_access(assign_stmt.lhs),
            ExprTag::MutRef.into(),
        );

        let tagged_rhs = self.compile_expr(&assign_stmt.rhs);
        let rhs = self.use_expr(
            tagged_rhs,
            ExprTag::MutRef | ExprTag::Ref | ExprTag::Local | ExprTag::Val,
        );

        self.stmts.push(match assign_stmt.lhs {
            normalizer::VarIndex::Param(var_param_index)
                if self.scope.param(var_param_index).kind == VarParamKind::Out =>
            {
                Expr::from(CallExpr {
                    target: TypeMemberFnPath {
                        parent: self.get_type_ident(assign_stmt.rhs.type_()),
                        ident: TypeMemberFnIdent::SetMutRef,
                    }
                    .into(),
                    args: vec![lhs, rhs],
                })
                .into()
            }
            // TODO(spinda): Review this.
            _ => Expr::from(AssignExpr { lhs, rhs }).into(),
        });
    }

    fn compile_ret_stmt(&mut self, ret_stmt: &normalizer::RetStmt) {
        let value = ret_stmt.value.as_ref().map(|value| {
            let tagged_value = self.compile_expr(value);
            self.use_expr(tagged_value, ExprTag::Val.into())
        });

        self.stmts.push(RetStmt { value }.into());
    }

    fn compile_expr(&mut self, expr: &normalizer::Expr) -> TaggedExpr {
        match expr {
            normalizer::Expr::Block(_, block_expr) => {
                self.compile_block_expr(&block_expr).map(Expr::from)
            }
            normalizer::Expr::Literal(literal) => self.compile_literal(literal).map(Expr::from),
            normalizer::Expr::Var(var_expr) => self.compile_var_expr(var_expr),
            normalizer::Expr::Invoke(invoke_expr) => {
                self.compile_invoke_expr(invoke_expr).map(Expr::from)
            }
            normalizer::Expr::FieldAccess(field_access_expr) => self
                .compile_field_access_expr(&field_access_expr)
                .map(Expr::from),
            normalizer::Expr::Negate(negate_expr) => {
                self.compile_negate_expr(&negate_expr).map(Expr::from)
            }
            normalizer::Expr::Cast(cast_expr) => {
                self.compile_cast_expr(&cast_expr).map(Expr::from)
            }
            normalizer::Expr::BinOper(bin_oper_expr) => {
                self.compile_bin_oper_expr(&bin_oper_expr).map(Expr::from)
            }
        }
    }

    fn compile_pure_expr(&mut self, pure_expr: &normalizer::PureExpr) -> TaggedExpr {
        match pure_expr {
            normalizer::PureExpr::Literal(literal) => {
                self.compile_literal(literal).map(Expr::from)
            }
            normalizer::PureExpr::Var(var_expr) => self.compile_var_expr(var_expr),
            normalizer::PureExpr::FieldAccess(field_access_expr) => self
                .compile_field_access_expr(&field_access_expr)
                .map(Expr::from),
            normalizer::PureExpr::Negate(negate_expr) => {
                self.compile_negate_expr(&negate_expr).map(Expr::from)
            }
            normalizer::PureExpr::Cast(cast_expr) => {
                self.compile_cast_expr(&cast_expr).map(Expr::from)
            }
            normalizer::PureExpr::BinOper(bin_oper_expr) => {
                self.compile_bin_oper_expr(&bin_oper_expr).map(Expr::from)
            }
        }
    }

    // TODO(spinda): Switch from a bottom-up to a top-down approach to
    // expression tagging.
    fn use_expr(&self, expr: TaggedExpr, tags: EnumSet<ExprTag>) -> Expr {
        // TODO(spinda): Auto-expand tags for built-in types.
        if expr.tags.is_disjoint(tags) {
            if let Some(preferred_tag) = tags.iter().nth(0) {
                // TODO(spinda): Check tag conversion validity.
                return CallExpr {
                    target: TypeMemberFnPath {
                        parent: self.get_type_ident(expr.type_),
                        ident: ToTagTypeMemberFnIdent::from(preferred_tag).into(),
                    }
                    .into(),
                    args: iterate![
                        ..if preferred_tag == ExprTag::Local {
                            Some(CONTEXT_ARG)
                        } else {
                            None
                        },
                        expr.expr,
                    ]
                    .collect(),
                }
                .into();
            }
        }

        expr.expr
    }

    fn compile_block_expr(&mut self, block_expr: &normalizer::BlockExpr) -> TaggedExpr<BlockExpr> {
        let mut block = self.compile_block(&block_expr.stmts);
        let value_tagged_expr = self.compile_expr(&block_expr.value);
        let value_expr = self.use_expr(value_tagged_expr, ExprTag::Val.into());
        block.stmts.push(value_expr.into());
        TaggedExpr {
            expr: block.into(),
            type_: block_expr.type_(),
            tags: ExprTag::Val.into(),
        }
    }

    fn compile_literal(&self, literal: &normalizer::Literal) -> TaggedExpr<Literal> {
        match literal {
            normalizer::Literal::Int8(n) => TaggedExpr {
                expr: Literal::Int8(*n),
                type_: BuiltInType::INT8.into(),
                tags: ExprTag::Val.into(),
            },
            normalizer::Literal::Int16(n) => TaggedExpr {
                expr: Literal::Int16(*n),
                type_: BuiltInType::INT16.into(),
                tags: ExprTag::Val.into(),
            },
            normalizer::Literal::Int32(n) => TaggedExpr {
                expr: Literal::Int32(*n),
                type_: BuiltInType::INT32.into(),
                tags: ExprTag::Val.into(),
            },
            normalizer::Literal::Int64(n) => TaggedExpr {
                expr: Literal::Int64(*n),
                type_: BuiltInType::INT64.into(),
                tags: ExprTag::Val.into(),
            },
            normalizer::Literal::UInt8(n) => TaggedExpr {
                expr: Literal::UInt8(*n),
                type_: BuiltInType::UINT8.into(),
                tags: ExprTag::Val.into(),
            },
            normalizer::Literal::UInt16(n) => TaggedExpr {
                expr: Literal::UInt16(*n),
                type_: BuiltInType::UINT16.into(),
                tags: ExprTag::Val.into(),
            },
            normalizer::Literal::UInt32(n) => TaggedExpr {
                expr: Literal::UInt32(*n),
                type_: BuiltInType::UINT32.into(),
                tags: ExprTag::Val.into(),
            },
            normalizer::Literal::UInt64(n) => TaggedExpr {
                expr: Literal::UInt64(*n),
                type_: BuiltInType::UINT64.into(),
                tags: ExprTag::Val.into(),
            },
            normalizer::Literal::Double(n) => TaggedExpr {
                expr: Literal::Double(*n),
                type_: BuiltInType::Double.into(),
                tags: ExprTag::Val.into(),
            },
        }
    }

    fn compile_var_expr(&self, var_expr: &normalizer::VarExpr) -> TaggedExpr {
        self.compile_var_access(var_expr.var)
    }

    fn compile_var_access(&self, var_index: normalizer::VarIndex) -> TaggedExpr {
        match var_index {
            normalizer::VarIndex::BuiltIn(built_in_var) => TaggedExpr {
                expr: CallExpr {
                    target: GlobalVarFnPath::from(GlobalVarFnIdent::from(built_in_var.ident()))
                        .into(),
                    args: vec![CONTEXT_ARG],
                }
                .into(),
                type_: built_in_var.type_(),
                tags: ExprTag::Ref.into(),
            },
            normalizer::VarIndex::EnumVariant(enum_variant_index) => {
                self.compile_enum_variant_access(enum_variant_index)
            }
            normalizer::VarIndex::Global(global_var_index) => {
                let global_var_item = &self.env[global_var_index];

                TaggedExpr {
                    expr: CallExpr {
                        target: GlobalVarFnPath {
                            parent: global_var_item.path.value.parent().map(Path::ident),
                            ident: global_var_item.path.value.ident().into(),
                        }
                        .into(),
                        args: vec![CONTEXT_ARG],
                    }
                    .into(),
                    type_: global_var_item.type_,
                    tags: global_var_tag(global_var_item.is_mut).into(),
                }
            }
            normalizer::VarIndex::Param(var_param_index) => {
                let var_param = &self.scope.param(var_param_index);

                TaggedExpr {
                    expr: UserParamVarIdent::from(var_param.ident.value).into(),
                    type_: var_param.type_,
                    tags: var_param_tag(var_param.kind).into(),
                }
            }
            normalizer::VarIndex::Local(local_var_index) => {
                let local_var = &self.scope.local(local_var_index);

                TaggedExpr {
                    expr: LocalVarIdent {
                        ident: local_var.ident.value,
                        index: local_var_index,
                    }
                    .into(),
                    type_: local_var.type_,
                    tags: ExprTag::Local.into(),
                }
            }
        }
    }

    fn compile_enum_variant_access(
        &self,
        enum_variant_index: normalizer::EnumVariantIndex,
    ) -> TaggedExpr {
        let enum_item = &self.env[enum_variant_index.enum_index];
        let variant_path = enum_item[enum_variant_index.variant_index];

        TaggedExpr {
            expr: CallExpr {
                target: TypeMemberFnPath {
                    parent: enum_item.ident.value,
                    ident: VariantTypeMemberFnIdent::from(variant_path.value.ident()).into(),
                }
                .into(),
                args: vec![CONTEXT_ARG],
            }
            .into(),
            type_: enum_variant_index.enum_index.into(),
            tags: ExprTag::Ref.into(),
        }
    }

    fn compile_invoke_expr(
        &mut self,
        invoke_expr: &normalizer::InvokeExpr,
    ) -> TaggedExpr<CallExpr> {
        let callable_item = &self.env[invoke_expr.call.target];

        let callable_parent_ident = callable_item.path.value.parent().map(Path::ident);
        let callable_ident = callable_item.path.value.ident();
        let target = UserFnPath {
            parent: callable_parent_ident,
            ident: FnUserFnIdent::from(callable_ident).into(),
        }
        .into();

        let args = self.compile_args(&invoke_expr.call.args);
        let args = iterate![
            CONTEXT_ARG,
            ..callable_item.emits.map(|_| ParamVarIdent::Ops.into()),
            ..args
        ]
        .collect();

        TaggedExpr {
            expr: CallExpr { target, args },
            type_: invoke_expr.type_(),
            tags: ExprTag::Val.into(),
        }
    }

    fn compile_field_access_expr<E: CompileExpr + Typed>(
        &mut self,
        field_access_expr: &normalizer::FieldAccessExpr<E>,
    ) -> TaggedExpr<Expr> {
        TaggedExpr {
            expr: self
                .compile_field_access(&field_access_expr.parent, field_access_expr.field)
                .into(),
            type_: field_access_expr.type_(),
            tags: ExprTag::Ref.into(),
        }
    }

    fn compile_field_access<E: CompileExpr + Typed>(
        &mut self,
        parent_expr: &E,
        struct_field_index: normalizer::StructFieldIndex,
    ) -> CallExpr {
        let parent_type = parent_expr.type_();

        let parent_tagged_expr = parent_expr.compile(self);
        // If the parent expression is `Val`-tagged, we need to hoist it up to
        // a temporary local variable to be able to take a reference to it.
        let parent_tagged_expr = if parent_tagged_expr.tags == ExprTag::Val {
            // This should be safe to do based on the laws of `Expr`/`PureExpr`
            // post-normalization: either we're dealing with a `PureExpr`
            // parent, in which case evaluation order doesn't matter, or we're
            // inside something unary, in which case hoisting out the
            // sub-expression won't change the order.

            let hoist_var_ident = self.generate_synthetic_var(SyntheticVarKind::Hoist).into();
            self.init_local_var_with_rhs(hoist_var_ident, parent_tagged_expr);

            TaggedExpr {
                expr: hoist_var_ident.into(),
                type_: parent_type,
                tags: ExprTag::Local.into(),
            }
        } else {
            parent_tagged_expr
        };
        let parent_expr = self.use_expr(parent_tagged_expr, ExprTag::Ref.into());

        let target = TypeMemberFnPath {
            parent: self.get_type_ident(parent_type),
            ident: FieldAccessTypeMemberFnIdent {
                field: self.env[struct_field_index].ident().value,
            }
            .into(),
        }
        .into();

        CallExpr {
            target,
            args: vec![CONTEXT_ARG, parent_expr],
        }
    }

    fn compile_negate_expr<E: CompileExpr + Typed>(
        &mut self,
        negate_expr: &normalizer::NegateExpr<E>,
    ) -> TaggedExpr<Expr> {
        let tagged_expr = negate_expr.expr.compile(self);
        let expr = self.use_expr(tagged_expr, ExprTag::Ref | ExprTag::Val);

        let ident = TypeMemberFnIdent::Negate(match negate_expr.kind {
            NegateKind::Arith | NegateKind::Bitwise => negate_expr.kind.into(),
            NegateKind::Logical => {
                return TaggedExpr {
                    expr: NegateExpr {
                        kind: negate_expr.kind,
                        expr,
                    }
                    .into(),
                    type_: negate_expr.type_(),
                    tags: ExprTag::Ref | ExprTag::Val,
                };
            }
        });

        let expr_type = negate_expr.expr.type_();
        let target = TypeMemberFnPath {
            parent: self.get_type_ident(expr_type),
            ident,
        }
        .into();

        TaggedExpr {
            expr: CallExpr {
                target,
                args: vec![expr],
            }
            .into(),
            type_: negate_expr.type_(),
            tags: ExprTag::Ref | ExprTag::Val,
        }
    }

    fn compile_cast_expr<E: CompileExpr + Typed>(
        &mut self,
        cast_expr: &normalizer::CastExpr<E>,
    ) -> TaggedExpr<Expr> {
        let tagged_expr = cast_expr.expr.compile(self);
        let expr_tags = tagged_expr.tags;
        let expr = self.use_expr(tagged_expr, ExprTag::Ref | ExprTag::Val);

        let source_type_index = cast_expr.expr.type_();
        let target_type_index = cast_expr.type_;

        let source_type_ident = self.get_type_ident(source_type_index);
        let target_type_ident = self.get_type_ident(target_type_index);

        match (source_type_index, target_type_index) {
            // Rather than specifying functions for each builtin cast, we'll just use C++
            // static_casts
            (normalizer::TypeIndex::BuiltIn(_), normalizer::TypeIndex::BuiltIn(_)) => TaggedExpr {
                expr: CastExpr {
                    kind: CastStyle::Functional(FunctionalCastStyle::Static),
                    type_: TypeMemberTypePath {
                        parent: target_type_ident,
                        ident: TypeMemberTypeIdent::ExprTag(ExprTag::Val),
                    }
                    .into(),
                    expr,
                }
                .into(),
                type_: cast_expr.type_(),
                tags: ExprTag::Ref | ExprTag::Val,
            },
            _ => {
                debug_assert_eq!(expr_tags.len(), 1);
                let tag = if expr_tags == ExprTag::Val {
                    ExprTag::Val
                } else {
                    ExprTag::Ref
                };

                TaggedExpr {
                    expr: CallExpr {
                        target: TypeMemberFnPath {
                            parent: source_type_ident,
                            ident: CastTypeMemberFnIdent {
                                target_type_ident,
                                tag,
                            }
                            .into(),
                        }
                        .into(),
                        args: vec![expr],
                    }
                    .into(),
                    type_: cast_expr.type_(),
                    tags: tag.into(),
                }
            }
        }
    }

    fn compile_bin_oper_expr(&mut self, bin_oper_expr: &normalizer::BinOperExpr) -> TaggedExpr {
        let tagged_lhs = self.compile_pure_expr(&bin_oper_expr.lhs);
        let lhs = self.use_expr(tagged_lhs, ExprTag::Ref.into());

        let tagged_rhs = self.compile_pure_expr(&bin_oper_expr.rhs);
        let rhs = self.use_expr(tagged_rhs, ExprTag::Ref.into());

        let ident = TypeMemberFnIdent::BinOper(match bin_oper_expr.oper {
            BinOper::Arith(arith_bin_oper) => arith_bin_oper.into(),
            BinOper::Bitwise(bitwise_bin_oper) => bitwise_bin_oper.into(),
            BinOper::Compare(compare_bin_oper) => compare_bin_oper.into(),
            BinOper::Logical(logical_bin_oper) => {
                return TaggedExpr {
                    expr: BinOperExpr {
                        oper: logical_bin_oper.into(),
                        lhs,
                        rhs,
                    }
                    .into(),
                    type_: bin_oper_expr.type_(),
                    tags: ExprTag::Ref | ExprTag::Val,
                };
            }
        });

        // Note that the type namespace for the helper function comes from the
        // *operand* type, not the output type, so we take the type of the
        // left-hand side rather than the operator expression itself.
        let lhs_type = bin_oper_expr.lhs.type_();
        let rhs_type = bin_oper_expr.rhs.type_();
        debug_assert_eq!(lhs_type, rhs_type);
        let target = TypeMemberFnPath {
            parent: self.get_type_ident(lhs_type),
            ident,
        }
        .into();

        TaggedExpr {
            expr: CallExpr {
                target,
                args: vec![lhs, rhs],
            }
            .into(),
            type_: bin_oper_expr.type_(),
            tags: ExprTag::Ref | ExprTag::Val,
        }
    }

    fn generate_synthetic_var(&mut self, kind: SyntheticVarKind) -> SyntheticVarIdent {
        let index = &mut self.next_synthetic_var_indexes[kind];
        let ident = SyntheticVarIdent {
            kind,
            index: *index,
        };
        *index += 1;
        ident
    }

    fn get_label_ir(&self, label_index: normalizer::LabelIndex) -> normalizer::IrIndex {
        match label_index {
            normalizer::LabelIndex::Param(label_param_index) => {
                self.scope.param(label_param_index).label.ir
            }
            normalizer::LabelIndex::Local(local_label_index) => {
                self.scope.local(local_label_index).ir
            }
        }
    }
}

fn global_var_tag(is_mut: bool) -> ExprTag {
    if is_mut {
        ExprTag::MutRef
    } else {
        ExprTag::Ref
    }
}

fn var_param_tag(kind: VarParamKind) -> ExprTag {
    match kind {
        VarParamKind::In => ExprTag::Ref,
        // Mutable variable parameters are stored in local variables at the
        // beginning of a function body.
        VarParamKind::Mut => ExprTag::Local,
        VarParamKind::Out => ExprTag::MutRef,
    }
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "{}", expr)]
struct TaggedExpr<T = Expr> {
    expr: T,
    type_: normalizer::TypeIndex,
    tags: EnumSet<ExprTag>,
}

impl<T> TaggedExpr<T> {
    fn map<U>(self, f: impl FnOnce(T) -> U) -> TaggedExpr<U> {
        TaggedExpr {
            expr: f(self.expr),
            type_: self.type_,
            tags: self.tags,
        }
    }
}

trait CompileExpr {
    fn compile<'a, 'b>(&self, scoped_compiler: &mut ScopedCompiler<'a, 'b>) -> TaggedExpr;
}

impl CompileExpr for normalizer::Expr {
    fn compile<'a, 'b>(&self, scoped_compiler: &mut ScopedCompiler<'a, 'b>) -> TaggedExpr {
        scoped_compiler.compile_expr(self)
    }
}

impl CompileExpr for normalizer::PureExpr {
    fn compile<'a, 'b>(&self, scoped_compiler: &mut ScopedCompiler<'a, 'b>) -> TaggedExpr {
        scoped_compiler.compile_pure_expr(self)
    }
}
