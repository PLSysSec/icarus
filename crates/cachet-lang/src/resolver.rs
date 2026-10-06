// vim: set tw=99 ts=4 sts=4 sw=4 et:

mod ast;
mod error;
mod registry;

use std::borrow::Cow;
use std::mem;
use std::ops::{Deref, DerefMut};

use derive_more::{From, Into};
use enum_iterator::IntoEnumIterator;
use enumset::EnumSet;
use typed_index_collections::{TiSlice, TiVec};

use cachet_util::{MaybeOwned, collect_eager, deref_from};

use crate::FrontendError;
use crate::ast::{Ident, Path, Spanned};
use crate::built_in::{BuiltInAttr, BuiltInType, BuiltInVar, IdentEnum};
use crate::parser;
use crate::util::map_spanned;

pub use crate::resolver::ast::*;
pub use crate::resolver::error::*;
use crate::resolver::registry::{LookupError, LookupResult, Registrable, Registry};

// * Name Resolution Entry Point

pub fn resolve(mod_: parser::Mod) -> Result<Env, ResolveErrors> {
    let mut item_catalog = ItemCatalog::new();
    item_catalog.catalog_items(mod_.items);

    let mut resolver = Resolver::new(
        item_catalog.errors,
        item_catalog.global_registry,
        item_catalog.impl_item_parents,
    );

    let enum_items: Option<_> = collect_eager(
        item_catalog
            .enum_items
            .into_iter()
            .map(|enum_item| resolver.resolve_enum_item(enum_item)),
    );

    let ir_items: Option<TiVec<_, _>> = collect_eager(
        item_catalog
            .ir_items
            .into_iter()
            .map(|ir_item| resolver.resolve_ir_item(ir_item)),
    );

    // We'll need early access to the name-resolved IR items in order to resolve
    // label IRs.
    resolver.ir_items = ir_items.as_ref().map(|ir_items| ir_items.as_slice());

    let struct_items: Option<_> = collect_eager(
        item_catalog
            .struct_items
            .into_iter()
            .map(|struct_item| resolver.resolve_struct_item(struct_item)),
    );

    let global_var_items: Option<_> = collect_eager(
        item_catalog
            .global_var_items
            .into_iter()
            .map(|global_var_item| resolver.resolve_global_var_item(global_var_item)),
    );

    let fn_items: Option<_> = collect_eager(
        item_catalog
            .fn_items
            .into_iter()
            .map(|fn_item| resolver.resolve_callable_item(fn_item)),
    );

    let op_items: Option<_> = collect_eager(
        item_catalog
            .op_items
            .into_iter()
            .map(|op_item| resolver.resolve_callable_item(op_item)),
    );

    if resolver.errors.is_empty() {
        Ok(Env {
            enum_items: enum_items.unwrap(),
            struct_items: struct_items.unwrap(),
            ir_items: ir_items.unwrap(),
            global_var_items: global_var_items.unwrap(),
            fn_items: fn_items.unwrap(),
            op_items: op_items.unwrap(),
        })
    } else {
        resolver.errors.sort_by_key(|error| error.span());
        Err(ResolveErrors(resolver.errors))
    }
}

// * Initial Item Cataloging/Index Assignment Step

struct ItemCatalog {
    errors: Vec<ResolveError>,
    global_registry: GlobalRegistry,
    enum_items: TiVec<EnumIndex, parser::EnumItem>,
    struct_items: TiVec<StructIndex, parser::StructItem>,
    ir_items: TiVec<IrIndex, parser::IrItem>,
    impl_item_parents: TiVec<ImplIndex, Spanned<Path>>,
    global_var_items: TiVec<GlobalVarIndex, NestableItem<parser::GlobalVarItem>>,
    fn_items: TiVec<FnIndex, NestableItem<parser::CallableItem>>,
    op_items: TiVec<OpIndex, NestableItem<parser::CallableItem>>,
}

impl ItemCatalog {
    fn new() -> Self {
        let mut global_registry =
            GlobalRegistry::with_capacity(BuiltInType::ITEM_COUNT + BuiltInVar::ITEM_COUNT);

        for built_in_type in BuiltInType::into_enum_iter() {
            global_registry
                .register(
                    Path::from(built_in_type.ident().to_owned()).into(),
                    built_in_type.into(),
                )
                .expect("built-in types shouldn't shadow other built-ins");
        }

        for built_in_var in BuiltInVar::into_enum_iter() {
            global_registry
                .register(
                    Path::from(built_in_var.ident().to_owned()).into(),
                    built_in_var.into(),
                )
                .expect("built-in variables shouldn't shadow other built-ins");
        }

        ItemCatalog {
            errors: Vec::new(),
            global_registry,
            enum_items: TiVec::new(),
            struct_items: TiVec::new(),
            ir_items: TiVec::new(),
            impl_item_parents: TiVec::new(),
            fn_items: TiVec::new(),
            global_var_items: TiVec::new(),
            op_items: TiVec::new(),
        }
    }

    fn catalog_items(&mut self, items: Vec<Spanned<parser::Item>>) {
        for item in items {
            self.catalog_item(None, item);
        }
    }

    fn catalog_item(
        &mut self,
        parent: Option<(Spanned<Path>, UnresolvedParentIndex)>,
        item: Spanned<parser::Item>,
    ) {
        match item.value {
            parser::Item::Comment(_) => (),
            parser::Item::Enum(enum_item) => {
                self.catalog_enum_item(parent, Spanned::new(item.span, enum_item))
            }
            parser::Item::Struct(struct_item) => {
                self.catalog_struct_item(parent, Spanned::new(item.span, struct_item))
            }
            parser::Item::Ir(ir_item) => {
                self.catalog_ir_item(parent, Spanned::new(item.span, ir_item))
            }
            parser::Item::Impl(impl_item) => {
                self.catalog_impl_item(parent, Spanned::new(item.span, impl_item))
            }
            parser::Item::Import(_) => {} // Imports are handled in the parser time
            parser::Item::GlobalVar(global_var_item) => {
                self.catalog_global_var_item(parent, global_var_item)
            }
            parser::Item::Fn(fn_item) => self.catalog_callable_item(
                parent,
                Spanned::new(item.span, fn_item),
                |item_catalog, fn_item| item_catalog.fn_items.push_and_get_key(fn_item),
            ),
            parser::Item::Op(op_item) => self.catalog_callable_item(
                parent,
                Spanned::new(item.span, op_item),
                |item_catalog, op_item| item_catalog.op_items.push_and_get_key(op_item),
            ),
        }
    }

    fn catalog_enum_item(
        &mut self,
        parent: Option<(Spanned<Path>, UnresolvedParentIndex)>,
        enum_item: Spanned<parser::EnumItem>,
    ) {
        let parent_path = parent.map(|(parent_path, parent_index)| {
            self.errors.push(
                BadNestingError {
                    ident: Some(enum_item.value.ident.value),
                    kind: NameKind::Type,
                    span: enum_item.span,
                    parent: parent_path,
                    parent_kind: parent_index.name_kind(),
                }
                .into(),
            );
            parent_path.value
        });
        let enum_path = enum_item
            .value
            .ident
            .map(|enum_ident| Path::new(parent_path, enum_ident));
        let enum_index = self.enum_items.next_key();

        for (variant_index, variant_ident) in enum_item.value.variants.iter_enumerated() {
            let enum_variant_index = EnumVariantIndex {
                enum_index,
                variant_index,
            };
            self.register_global(
                (*variant_ident).map(|variant_ident| enum_path.value.nest(variant_ident)),
                enum_variant_index.into(),
            );
        }

        let actual_enum_index = self.enum_items.push_and_get_key(enum_item.value);
        debug_assert_eq!(enum_index, actual_enum_index);
        self.register_global(enum_path, enum_index.into());
    }

    fn catalog_struct_item(
        &mut self,
        parent: Option<(Spanned<Path>, UnresolvedParentIndex)>,
        struct_item: Spanned<parser::StructItem>,
    ) {
        let parent_path = parent.map(|(parent_path, parent_index)| {
            self.errors.push(
                BadNestingError {
                    ident: Some(struct_item.value.ident.value),
                    kind: NameKind::Type,
                    span: struct_item.span,
                    parent: parent_path,
                    parent_kind: parent_index.name_kind(),
                }
                .into(),
            );
            parent_path.value
        });
        let struct_path = struct_item
            .value
            .ident
            .map(|struct_ident| Path::new(parent_path, struct_ident));
        let struct_index = self.struct_items.push_and_get_key(struct_item.value);
        self.register_global(struct_path, struct_index.into());
    }

    fn catalog_ir_item(
        &mut self,
        parent: Option<(Spanned<Path>, UnresolvedParentIndex)>,
        mut ir_item: Spanned<parser::IrItem>,
    ) {
        let parent_path = parent.map(|(parent_path, parent_index)| {
            self.errors.push(
                BadNestingError {
                    ident: Some(ir_item.value.ident.value),
                    kind: NameKind::Ir,
                    span: ir_item.span,
                    parent: parent_path,
                    parent_kind: parent_index.name_kind(),
                }
                .into(),
            );
            parent_path.value
        });
        let ir_path = ir_item
            .value
            .ident
            .map(|ir_ident| Path::new(parent_path, ir_ident));
        let ir_sub_items = mem::replace(&mut ir_item.value.items, Vec::new());
        let ir_index = self.ir_items.push_and_get_key(ir_item.value);
        self.register_global(ir_path, ir_index.into());

        for item in ir_sub_items {
            self.catalog_item(Some((ir_path, ir_index.into())), item);
        }
    }

    fn catalog_impl_item(
        &mut self,
        parent: Option<(Spanned<Path>, UnresolvedParentIndex)>,
        impl_item: Spanned<parser::ImplItem>,
    ) {
        if let Some((parent_path, parent_index)) = parent {
            self.errors.push(
                BadNestingError {
                    ident: None,
                    kind: NameKind::Type,
                    span: impl_item.span,
                    parent: parent_path,
                    parent_kind: parent_index.name_kind(),
                }
                .into(),
            );
        }

        let impl_index = self
            .impl_item_parents
            .push_and_get_key(impl_item.value.parent);

        for item in impl_item.value.items {
            self.catalog_item(Some((impl_item.value.parent, impl_index.into())), item);
        }
    }

    fn catalog_global_var_item(
        &mut self,
        parent: Option<(Spanned<Path>, UnresolvedParentIndex)>,
        global_var_item: parser::GlobalVarItem,
    ) {
        let parent_path = parent.map(|(parent_path, _)| parent_path.value);
        let global_var_path = global_var_item
            .ident
            .map(|global_var_ident| Path::new(parent_path, global_var_ident));
        let global_var_index = self.global_var_items.push_and_get_key(NestableItem {
            parent,
            path: global_var_path,
            item: global_var_item,
        });
        self.register_global(global_var_path, global_var_index.into());
    }

    fn catalog_callable_item<I: Into<GlobalIndex>>(
        &mut self,
        parent: Option<(Spanned<Path>, UnresolvedParentIndex)>,
        callable_item: Spanned<parser::CallableItem>,
        push_callable_item: impl FnOnce(&mut Self, NestableItem<parser::CallableItem>) -> I,
    ) {
        let parent_path = parent.map(|(parent_path, _)| parent_path.value);
        let callable_path = callable_item
            .value
            .ident
            .map(|callable_ident| Path::new(parent_path, callable_ident));
        let callable_index = push_callable_item(
            self,
            NestableItem {
                parent,
                path: callable_path,
                item: callable_item.value,
            },
        )
        .into();
        self.register_global(callable_path, callable_index);

        if let GlobalIndex::Op(_) = callable_index {
            match parent {
                Some((parent_path, parent_index)) => match parent_index {
                    UnresolvedParentIndex::Ir(_) => (),
                    _ => {
                        self.errors.push(
                            BadNestingError {
                                ident: None,
                                kind: NameKind::Op,
                                span: callable_item.span,
                                parent: parent_path,
                                parent_kind: parent_index.name_kind(),
                            }
                            .into(),
                        );
                    }
                },
                None => {
                    self.errors.push(
                        OrphanItemError {
                            ident: callable_path.value.ident(),
                            kind: NameKind::Op,
                            span: callable_item.span,
                        }
                        .into(),
                    );
                }
            }
        }
    }

    fn register_global(&mut self, path: Spanned<Path>, global_index: GlobalIndex) {
        if let Err(error) = self.global_registry.register(path.into(), global_index) {
            self.errors.push(error.into());
        }
    }
}

// * Top-Level Name Resolver

struct Resolver<'a> {
    errors: Vec<ResolveError>,
    global_registry: GlobalRegistry,
    impl_item_parents: TiVec<ImplIndex, Option<TypeIndex>>,
    ir_items: Option<&'a TiSlice<IrIndex, IrItem>>,
}

impl<'a> Resolver<'a> {
    fn new(
        errors: Vec<ResolveError>,
        global_registry: GlobalRegistry,
        impl_item_parents: TiVec<ImplIndex, Spanned<Path>>,
    ) -> Self {
        let mut resolver = Resolver {
            errors,
            global_registry,
            impl_item_parents: TiVec::with_capacity(impl_item_parents.len()),
            ir_items: None,
        };

        for parent_index in impl_item_parents {
            let parent_index = resolver.lookup_type_global(parent_index);
            resolver
                .impl_item_parents
                .push(parent_index.found(&mut resolver.errors));
        }

        resolver
    }

    fn resolve_attrs(&mut self, attrs: Vec<parser::Attr>) -> Option<EnumSet<BuiltInAttr>> {
        collect_eager(attrs.into_iter().map(|attr| self.resolve_attr(attr)))
    }

    fn resolve_attr(&mut self, attr: parser::Attr) -> Option<BuiltInAttr> {
        let resolved_attr = BuiltInAttr::from_path(attr.path.value);

        if resolved_attr.is_none() {
            self.errors.push(
                UndefinedError {
                    path: attr.path,
                    expected: NameKind::Attr.into(),
                }
                .into(),
            );
        }

        resolved_attr
    }

    fn resolve_enum_item(&mut self, enum_item: parser::EnumItem) -> Option<EnumItem> {
        let attrs = self.resolve_attrs(enum_item.attrs);

        let variants = enum_item
            .variants
            .into_iter()
            .map(|variant_ident| {
                variant_ident.map(|variant_ident| enum_item.ident.value.nest(variant_ident))
            })
            .collect();

        Some(EnumItem {
            ident: enum_item.ident,
            attrs: attrs?,
            variants,
        })
    }

    fn resolve_struct_item(&mut self, struct_item: parser::StructItem) -> Option<StructItem> {
        let attrs = self.resolve_attrs(struct_item.attrs);

        let supertype = match struct_item.supertype {
            None => Some(None),
            Some(supertype) => self
                .lookup_type_global(supertype)
                .found(&mut self.errors)
                .map(Some),
        };

        // TODO(spinda): Raise an error on duplicate field names.
        // TODO(spinda): Raise an error on shadowing supertype field names.
        // TODO(spinda): Interaction between struct fields and supertypes is in
        // a weird spot right now. Sort that out down the road.
        let fields: Option<_> =
            collect_eager(struct_item.fields.into_iter().map(|field| match field {
                parser::Field::Var(var_field) => {
                    let type_ = self
                        .lookup_type_global(var_field.type_)
                        .found(&mut self.errors);
                    Some(
                        VarField {
                            ident: var_field.ident,
                            type_: type_?,
                        }
                        .into(),
                    )
                }
                parser::Field::Label(label) => {
                    let ir = self.lookup_ir_global(label.ir).found(&mut self.errors);
                    Some(
                        Label {
                            ident: label.ident,
                            ir: ir?,
                        }
                        .into(),
                    )
                }
            }));

        Some(StructItem {
            ident: struct_item.ident,
            attrs: attrs?,
            supertype: supertype?,
            fields: fields?,
        })
    }

    fn resolve_ir_item(&mut self, ir_item: parser::IrItem) -> Option<IrItem> {
        let emits = match ir_item.emits {
            Some(emits) => map_spanned(emits, |emits| {
                self.lookup_ir_global(emits).found(&mut self.errors)
            })
            .map(Some),
            None => Some(None),
        };

        Some(IrItem {
            ident: ir_item.ident,
            emits: emits?,
        })
    }

    fn resolve_global_var_item(
        &mut self,
        global_var_item: NestableItem<parser::GlobalVarItem>,
    ) -> Option<GlobalVarItem> {
        let parent_index = match global_var_item.parent {
            Some((_, parent_index)) => self.resolve_parent_index(parent_index).map(Some),
            None => Some(None),
        };

        let type_ = self
            .lookup_type_global(global_var_item.item.type_)
            .found(&mut self.errors);

        let attrs = self.resolve_attrs(global_var_item.item.attrs);
        let mut resolver = ScopedResolver::new(self);
        let value = global_var_item
            .item
            .value
            .and_then(|value| map_spanned(value, |value| resolver.resolve_expr(value.value)));

        Some(GlobalVarItem {
            path: global_var_item.path,
            parent: parent_index?,
            attrs: attrs?,
            is_mut: global_var_item.item.is_mut,
            type_: type_?,
            value,
        })
    }

    fn resolve_callable_item(
        &mut self,
        callable_item: NestableItem<parser::CallableItem>,
    ) -> Option<CallableItem> {
        let parent_index = match callable_item.parent {
            Some((_, parent_index)) => self.resolve_parent_index(parent_index).map(Some),
            None => Some(None),
        };

        let attrs = self.resolve_attrs(callable_item.item.attrs);

        let emits = match callable_item.item.emits {
            Some(emits) => map_spanned(emits, |emits| {
                self.lookup_ir_global(emits).found(&mut self.errors)
            })
            .map(Some),
            None => Some(None),
        };

        let ret = match callable_item.item.ret {
            Some(ret) => map_spanned(ret, |ret| {
                self.lookup_type_global(ret).found(&mut self.errors)
            })
            .map(Some),
            None => Some(None),
        };

        let mut scoped_resolver = ScopedResolver::new(self);
        let (params, param_order) = scoped_resolver.resolve_params(callable_item.item.params);
        let body = map_spanned(callable_item.item.body, move |body| match body.value {
            Some(block) => scoped_resolver.resolve_body_block(block).map(move |block| {
                Some(
                    Body {
                        locals: scoped_resolver.locals.into_owned(),
                        block,
                    }
                    .into(),
                )
            }),
            None => Some(None),
        });

        Some(CallableItem {
            path: callable_item.path.into(),
            parent: parent_index?,
            attrs: attrs?,
            is_unsafe: callable_item.item.is_unsafe,
            params,
            param_order,
            emits: emits?,
            ret: ret?,
            body: body?,
        })
    }

    fn resolve_parent_index(&self, parent_index: UnresolvedParentIndex) -> Option<ParentIndex> {
        match parent_index {
            UnresolvedParentIndex::Impl(impl_index) => {
                self.impl_item_parents[impl_index].map(ParentIndex::from)
            }
            UnresolvedParentIndex::Ir(ir_index) => Some(ir_index.into()),
        }
    }

    fn lookup_type_global(&mut self, path: Spanned<Path>) -> LookupResult<TypeIndex> {
        self.lookup_global(path, NameKind::Type.into()).map_found(
            |global_index| match global_index {
                GlobalIndex::Type(type_index) => type_index,
                _ => unreachable!(),
            },
        )
    }

    fn lookup_ir_global(&mut self, path: Spanned<Path>) -> LookupResult<IrIndex> {
        self.lookup_global(path, NameKind::Ir.into()).map_found(
            |global_index| match global_index {
                GlobalIndex::Ir(ir_index) => ir_index,
                _ => unreachable!(),
            },
        )
    }

    fn lookup_op_global(&mut self, path: Spanned<Path>) -> LookupResult<OpIndex> {
        self.lookup_global(path, NameKind::Op.into()).map_found(
            |global_index| match global_index {
                GlobalIndex::Op(op_index) => op_index,
                _ => unreachable!(),
            },
        )
    }

    fn lookup_var_global(&mut self, path: Spanned<Path>) -> LookupResult<VarIndex> {
        self.lookup_global(path, NameKind::Var.into())
            .map_found(|global_index| match global_index {
                GlobalIndex::Var(var_index) => var_index,
                _ => unreachable!(),
            })
    }

    fn lookup_fn_global(&mut self, path: Spanned<Path>) -> LookupResult<FnIndex> {
        self.lookup_global(path, NameKind::Fn.into()).map_found(
            |global_index| match global_index {
                GlobalIndex::Fn(fn_index) => fn_index,
                _ => unreachable!(),
            },
        )
    }

    fn lookup_global(
        &mut self,
        path: Spanned<Path>,
        expected: EnumSet<NameKind>,
    ) -> LookupResult<GlobalIndex> {
        self.global_registry
            .lookup(path, expected)
            .map_found(|global_index| *global_index)
    }
}

// * Scoped Name Resolver

struct ScopedResolver<'a, 'b> {
    resolver: &'b mut Resolver<'a>,
    scoped_registry: Cow<'b, ScopedRegistry>,
    locals: MaybeOwned<'b, Locals>,
}

impl<'a, 'b> Deref for ScopedResolver<'a, 'b> {
    type Target = Resolver<'a>;

    fn deref(&self) -> &Self::Target {
        self.resolver
    }
}

impl<'a, 'b> DerefMut for ScopedResolver<'a, 'b> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.resolver
    }
}

impl<'a, 'b> ScopedResolver<'a, 'b> {
    fn new(resolver: &'b mut Resolver<'a>) -> Self {
        ScopedResolver {
            resolver,
            scoped_registry: Cow::Owned(ScopedRegistry::new()),
            locals: MaybeOwned::Owned(Locals::default()),
        }
    }

    fn recurse<'c>(&'c mut self) -> ScopedResolver<'a, 'c> {
        ScopedResolver {
            resolver: self.resolver,
            scoped_registry: Cow::Borrowed(&self.scoped_registry),
            locals: MaybeOwned::Borrowed(&mut self.locals),
        }
    }

    fn resolve_params(&mut self, params: Vec<parser::Param>) -> (Params, Vec<ParamIndex>) {
        let mut resolved_params = Params::default();
        let param_order = params
            .into_iter()
            .map(|param| self.resolve_param(param, &mut resolved_params))
            .collect();
        (resolved_params, param_order)
    }

    fn resolve_param(&mut self, param: parser::Param, params: &mut Params) -> ParamIndex {
        match param {
            parser::Param::Var(var_param) => {
                let var_param = self.resolve_var_param(var_param);
                let var_param_ident = var_param.ident;
                let var_param_index = params.var_params.push_and_get_key(var_param);
                self.register_scoped(var_param_ident, var_param_index.into());
                var_param_index.into()
            }
            parser::Param::Label(label_param) => {
                let label_param = self.resolve_label_param(label_param);
                let label_param_ident = label_param.label.ident;
                let label_param_index = params.label_params.push_and_get_key(label_param);
                self.register_scoped(label_param_ident, label_param_index.into());
                label_param_index.into()
            }
        }
    }

    fn resolve_var_param(&mut self, var_param: parser::VarParam) -> VarParam {
        let type_ = self
            .lookup_type_scoped(var_param.type_)
            .found(&mut self.errors)
            .unwrap_or_else(|| BuiltInType::Unit.into());

        VarParam {
            ident: var_param.ident,
            kind: var_param.kind,
            type_,
        }
    }

    fn resolve_label_param(&mut self, label_param: parser::LabelParam) -> LabelParam {
        let label = self.resolve_label(label_param.label);

        LabelParam {
            label,
            is_out: label_param.is_out,
        }
    }

    fn resolve_label(&mut self, label: parser::Label) -> Label {
        // If the IR lookup fails, we fill in the `ir` field with a bogus zero
        // index value. This could be an invalid index value if there are no IRs
        // defined in the code we're processing. It'll be fine as long as
        // nothing in name resolution actually uses these index values. The
        // generation of an error on lookup failure will stop them from escaping
        // the resolver.
        let ir = self
            .resolve_label_ir(label.ir)
            .unwrap_or_else(|| IrIndex::from(0));

        Label {
            ident: label.ident,
            ir,
        }
    }

    fn resolve_args(&mut self, args: Vec<Spanned<parser::Arg>>) -> Option<Vec<Spanned<Arg>>> {
        let mut deferred_local_registrations = Vec::new();

        let args = collect_eager(args.into_iter().map(|arg| {
            map_spanned(arg, |arg| {
                self.resolve_arg(arg.value, &mut deferred_local_registrations)
            })
        }));

        for (ident, scoped_index) in deferred_local_registrations {
            self.register_scoped(ident, scoped_index);
        }

        args
    }

    fn resolve_arg(
        &mut self,
        arg: parser::Arg,
        deferred_local_registrations: &mut Vec<(Spanned<Ident>, ScopedIndex)>,
    ) -> Option<Arg> {
        match arg {
            parser::Arg::Expr(expr) => self.resolve_expr(expr).map(Arg::from),
            parser::Arg::Free(free_arg) => self
                .lookup_var_or_label_scoped(free_arg.path)
                .found(&mut self.errors)
                .map(|scoped_index| match scoped_index {
                    ScopedIndex::Var(var_index) => {
                        if free_arg.is_out {
                            OutVar::Free(Spanned::new(free_arg.path.span, var_index)).into()
                        } else {
                            Expr::from(Spanned::new(free_arg.path.span, var_index)).into()
                        }
                    }
                    ScopedIndex::Label(label_index) => {
                        if free_arg.is_out {
                            OutLabel::Free(Spanned::new(free_arg.path.span, label_index)).into()
                        } else {
                            label_index.into()
                        }
                    }
                }),
            parser::Arg::FieldAccess(field_access) => {
                self.resolve_field_access(field_access).map(Arg::from)
            }
            parser::Arg::OutFreshVar(local_var) => {
                let local_var = self.resolve_local_var(local_var);
                let local_var_ident = local_var.ident;
                let local_var_index = self.locals.local_vars.push_and_get_key(local_var);
                deferred_local_registrations.push((local_var_ident, local_var_index.into()));
                Some(OutVar::Fresh(local_var_index).into())
            }
            parser::Arg::OutFreshLabel(local_label) => {
                let local_label = self.resolve_local_label(local_label);
                let label_ident = local_label.ident;
                let local_label_index = self.locals.local_labels.push_and_get_key(local_label);
                deferred_local_registrations.push((label_ident, local_label_index.into()));
                Some(OutLabel::Fresh(local_label_index).into())
            }
        }
    }

    fn resolve_field_access(&mut self, field_access: parser::FieldAccess) -> Option<FieldAccess> {
        let parent = map_spanned(field_access.parent, |parent| {
            self.resolve_expr(parent.value)
        });

        Some(FieldAccess {
            parent: parent?,
            field: field_access.field,
        })
    }

    fn resolve_call(
        &mut self,
        call: parser::Call,
        lookup_target: impl FnOnce(&mut Self, Spanned<Path>) -> LookupResult<CallableIndex>,
    ) -> Option<Call> {
        let target = map_spanned(call.target, |target| {
            lookup_target(self, target).found(&mut self.errors)
        });

        let args = map_spanned(call.args, |args| self.resolve_args(args.value));

        Some(Call {
            target: target?,
            args: args?,
        })
    }

    fn resolve_local_var(&mut self, local_var: parser::LocalVar) -> LocalVar {
        let type_ = local_var
            .type_
            .and_then(|type_| self.lookup_type_scoped(type_).found(&mut self.errors));

        LocalVar {
            ident: local_var.ident,
            is_mut: local_var.is_mut,
            type_,
        }
    }

    fn resolve_local_label(&mut self, local_label: parser::LocalLabel) -> LocalLabel {
        let ir = local_label.ir.and_then(|ir| self.resolve_label_ir(ir));

        LocalLabel {
            ident: local_label.ident,
            ir,
        }
    }

    fn resolve_body_block(&mut self, block: parser::Block) -> Option<Block> {
        self.resolve_block_impl(block)
    }

    fn resolve_nested_block(&mut self, block: parser::Block) -> Option<Block> {
        self.recurse().resolve_block_impl(block)
    }

    fn resolve_nested_elseif(&mut self, if_expr: Box<parser::IfExpr>) -> Option<Box<IfExpr>> {
        self.recurse().resolve_if_expr(*if_expr).map(Box::new)
    }

    /// This is deliberately *not* named `resolve_block`, to force explicit
    /// disambiguation between calls to `resolve_body_block` and
    /// `resolve_nested_block`. `resolve_block_impl` should not generally be
    /// called directly.
    fn resolve_block_impl(&mut self, block: parser::Block) -> Option<Block> {
        let stmts: Option<_> = collect_eager(block.stmts.into_iter().filter_map(|stmt| {
            let stmt_span = stmt.span;
            match self.resolve_stmt(stmt.value) {
                None => Some(None),
                Some(maybe_stmt) => maybe_stmt.map(|stmt| Some(Spanned::new(stmt_span, stmt))),
            }
        }));

        let value = Spanned::new(
            block.value.span,
            match block.value.value {
                Some(expr) => self.resolve_expr(expr)?.into(),
                None => None,
            },
        );

        Some(Block {
            stmts: stmts?,
            value: value,
        })
    }

    fn resolve_kinded_block(&mut self, kinded_block: parser::KindedBlock) -> Option<KindedBlock> {
        let block = self.resolve_nested_block(kinded_block.block);

        Some(KindedBlock {
            kind: kinded_block.kind,
            block: block?,
        })
    }

    fn resolve_stmt(&mut self, stmt: parser::Stmt) -> Option<Option<Stmt>> {
        match stmt {
            parser::Stmt::Comment(_) => Some(None),
            parser::Stmt::Let(let_stmt) => {
                self.resolve_let_stmt(let_stmt).map(Stmt::from).map(Some)
            }
            parser::Stmt::Label(label_stmt) => self
                .resolve_label_stmt(label_stmt)
                .map(Stmt::from)
                .map(Some),
            parser::Stmt::ForIn(for_in_stmt) => self
                .resolve_for_in_stmt(for_in_stmt)
                .map(Stmt::from)
                .map(Some),
            parser::Stmt::Check(check_stmt) => self
                .resolve_check_stmt(check_stmt)
                .map(Stmt::from)
                .map(Some),
            parser::Stmt::Goto(goto_stmt) => {
                self.resolve_goto_stmt(goto_stmt).map(Stmt::from).map(Some)
            }
            parser::Stmt::Bind(bind_stmt) => {
                self.resolve_bind_stmt(bind_stmt).map(Stmt::from).map(Some)
            }
            parser::Stmt::Emit(call) => self
                .resolve_call(call, |scoped_resolver, target| {
                    scoped_resolver
                        .lookup_op_scoped(target)
                        .map_found(Into::into)
                })
                .map(Stmt::Emit)
                .map(Some),
            parser::Stmt::Ret(ret_stmt) => {
                self.resolve_ret_stmt(ret_stmt).map(Stmt::from).map(Some)
            }
            parser::Stmt::Unreachable => Some(Some(Stmt::Unreachable)),
            parser::Stmt::Expr(expr) => self.resolve_expr(expr).map(Stmt::Expr).map(Some),
            parser::Stmt::Semi(expr) => self.resolve_expr(expr).map(Stmt::Semi).map(Some),
        }
    }

    fn resolve_let_stmt(&mut self, let_stmt: parser::LetStmt) -> Option<LetStmt> {
        let rhs = map_spanned(let_stmt.rhs, |rhs| self.resolve_expr(rhs.value));

        let local_var = self.resolve_local_var(let_stmt.lhs);
        let local_var_ident = local_var.ident;
        let local_var_index = self.locals.local_vars.push_and_get_key(local_var);
        self.register_scoped(local_var_ident, local_var_index.into());

        Some(LetStmt {
            lhs: local_var_index,
            rhs: rhs?,
        })
    }

    fn resolve_label_stmt(&mut self, label_stmt: parser::LabelStmt) -> Option<LabelStmt> {
        let local_label = self.resolve_local_label(label_stmt.label.into());
        let local_label_ident = local_label.ident;
        let local_label_index = self.locals.local_labels.push_and_get_key(local_label);
        self.register_scoped(local_label_ident, local_label_index.into());

        Some(LabelStmt {
            label: local_label_index,
        })
    }

    fn resolve_if_expr(&mut self, if_expr: parser::IfExpr) -> Option<IfExpr> {
        let cond = map_spanned(if_expr.cond, |cond| self.resolve_expr(cond.value));

        let then = self.resolve_nested_block(if_expr.then);

        let else_ = match if_expr.else_ {
            Some(parser::ElseClause::ElseIf(elseif_)) => self
                // TODO(spinda): What's this doing? Should we really be
                // recursing here?
                .resolve_nested_elseif(elseif_)
                .map(ElseClause::ElseIf)
                .map(Some),
            Some(parser::ElseClause::Else(else_)) => self
                .resolve_nested_block(else_)
                .map(ElseClause::Else)
                .map(Some),
            None => Some(None),
        };

        Some(IfExpr {
            cond: cond?,
            then: then?,
            else_: else_?,
        })
    }

    fn resolve_for_in_stmt(&mut self, for_in_stmt: parser::ForInStmt) -> Option<ForInStmt> {
        // Lookup the target type that we are iterating over.
        let target_type_index = self
            .lookup_type_scoped(for_in_stmt.target)
            .found(&mut self.errors);

        // Create a local variable with the type of the target type that we are
        // iterating over, scoped to the loop body.
        let mut scoped_resolver = self.recurse();
        let loop_var = LocalVar {
            ident: for_in_stmt.var,
            type_: target_type_index,
            is_mut: false,
        };
        let loop_var_ident = loop_var.ident;
        let loop_var_index = scoped_resolver.locals.local_vars.push_and_get_key(loop_var);
        scoped_resolver.register_scoped(loop_var_ident, loop_var_index.into());

        let body = scoped_resolver.resolve_block_impl(for_in_stmt.body);

        Some(ForInStmt {
            var: loop_var_index,
            target: target_type_index?,
            order: for_in_stmt.order,
            body: body?,
        })
    }

    fn resolve_check_stmt(&mut self, check_stmt: parser::CheckStmt) -> Option<CheckStmt> {
        let cond = map_spanned(check_stmt.cond, |cond| self.resolve_expr(cond.value));

        Some(CheckStmt {
            kind: check_stmt.kind,
            cond: cond?,
        })
    }

    fn resolve_goto_stmt(&mut self, goto_stmt: parser::GotoStmt) -> Option<GotoStmt> {
        let label_index = map_spanned(goto_stmt.label, |label_ident| {
            self.lookup_label_scoped(label_ident)
                .found(&mut self.errors)
        });

        Some(GotoStmt {
            label: label_index?,
        })
    }

    fn resolve_bind_stmt(&mut self, bind_stmt: parser::BindStmt) -> Option<BindStmt> {
        let label_index = map_spanned(bind_stmt.label, |label_ident| {
            self.lookup_label_scoped(label_ident)
                .found(&mut self.errors)
        });

        Some(BindStmt {
            label: label_index?,
        })
    }

    fn resolve_ret_stmt(&mut self, ret_stmt: parser::RetStmt) -> Option<RetStmt> {
        let value = map_spanned(ret_stmt.value, |value| match value.value {
            Some(value) => self.resolve_expr(value).map(Some),
            None => Some(None),
        });

        Some(RetStmt { value: value? })
    }

    fn resolve_expr(&mut self, expr: parser::Expr) -> Option<Expr> {
        match expr {
            parser::Expr::Block(block) => self.resolve_kinded_block(*block).map(Expr::from),
            parser::Expr::If(if_expr) => self.resolve_if_expr(*if_expr).map(Expr::from),
            parser::Expr::Literal(literal) => Some(literal.into()),
            parser::Expr::Var(var_path) => self.resolve_var_expr(var_path).map(Expr::from),
            parser::Expr::Invoke(call) => self
                .resolve_call(call, |scoped_resolver, target| {
                    scoped_resolver
                        .lookup_fn_scoped(target)
                        .map_found(Into::into)
                })
                .map(Expr::Invoke),
            parser::Expr::FieldAccess(field_access) => {
                self.resolve_field_access(*field_access).map(Expr::from)
            }
            parser::Expr::Negate(negate_expr) => {
                self.resolve_negate_expr(*negate_expr).map(Expr::from)
            }
            parser::Expr::Cast(cast_expr) => self.resolve_cast_expr(*cast_expr).map(Expr::from),
            parser::Expr::BinOper(bin_oper_expr) => {
                self.resolve_bin_oper_expr(*bin_oper_expr).map(Expr::from)
            }
            parser::Expr::Assign(assign_expr) => {
                self.resolve_assign_expr(*assign_expr).map(Expr::from)
            }
        }
    }

    fn resolve_var_expr(&mut self, var_path: Spanned<Path>) -> Option<Spanned<VarIndex>> {
        map_spanned(var_path, |var_path| {
            self.lookup_var_scoped(var_path).found(&mut self.errors)
        })
    }

    fn resolve_negate_expr(&mut self, negate_expr: parser::NegateExpr) -> Option<NegateExpr> {
        let expr = map_spanned(negate_expr.expr, |expr| self.resolve_expr(expr.value));

        Some(NegateExpr {
            kind: negate_expr.kind,
            expr: expr?,
        })
    }

    fn resolve_cast_expr(&mut self, cast_expr: parser::CastExpr) -> Option<CastExpr> {
        let expr = map_spanned(cast_expr.expr, |expr| self.resolve_expr(expr.value));

        let type_ = map_spanned(cast_expr.type_, |type_| {
            self.lookup_type_scoped(type_).found(&mut self.errors)
        });

        Some(CastExpr {
            expr: expr?,
            type_: type_?,
        })
    }

    fn resolve_bin_oper_expr(
        &mut self,
        bin_oper_expr: parser::BinOperExpr,
    ) -> Option<BinOperExpr> {
        let lhs = map_spanned(bin_oper_expr.lhs, |lhs| self.resolve_expr(lhs.value));

        let rhs = map_spanned(bin_oper_expr.rhs, |rhs| self.resolve_expr(rhs.value));

        Some(BinOperExpr {
            oper: bin_oper_expr.oper,
            lhs: lhs?,
            rhs: rhs?,
        })
    }

    fn resolve_assign_expr(&mut self, assign_expr: parser::AssignExpr) -> Option<AssignExpr> {
        let lhs = map_spanned(assign_expr.lhs, |lhs| {
            self.lookup_var_scoped(lhs).found(&mut self.errors)
        });

        let rhs = map_spanned(assign_expr.rhs, |rhs| self.resolve_expr(rhs.value));

        Some(AssignExpr {
            lhs: lhs?,
            rhs: rhs?,
        })
    }

    fn resolve_label_ir(&mut self, path: Spanned<Path>) -> Option<IrIndex> {
        let ir_index = self.lookup_ir_scoped(path).found(&mut self.errors)?;
        let ir_item = &self.ir_items?[ir_index];
        match ir_item.emits {
            None => Some(ir_index),
            Some(_) => {
                self.errors.push(
                    InvalidLabelIrError {
                        ir: path,
                        ir_defined_at: ir_item.ident.span,
                    }
                    .into(),
                );
                None
            }
        }
    }

    fn register_scoped(&mut self, ident: Spanned<Ident>, scoped_index: ScopedIndex) {
        if let Err(error) = self
            .scoped_registry
            .to_mut()
            .register(ident.into(), scoped_index)
        {
            self.errors.push(error.into());
        }
    }

    fn lookup_type_scoped(&mut self, path: Spanned<Path>) -> LookupResult<TypeIndex> {
        let path = match self.lookup_scoped_only(path, NameKind::Type.into()) {
            Ok(_) => unreachable!(),
            Err(LookupError::Undefined(error)) => error.path,
            Err(error @ LookupError::WrongKind(_)) => return Err(error),
        };

        self.lookup_type_global(path)
    }

    fn lookup_ir_scoped(&mut self, path: Spanned<Path>) -> LookupResult<IrIndex> {
        let path = match self.lookup_scoped_only(path, NameKind::Ir.into()) {
            Ok(_) => unreachable!(),
            Err(LookupError::Undefined(error)) => error.path,
            Err(error @ LookupError::WrongKind(_)) => return Err(error),
        };

        self.lookup_ir_global(path)
    }

    fn lookup_op_scoped(&mut self, path: Spanned<Path>) -> LookupResult<OpIndex> {
        let path = match self.lookup_scoped_only(path, NameKind::Op.into()) {
            Ok(_) => unreachable!(),
            Err(LookupError::Undefined(error)) => error.path,
            Err(error @ LookupError::WrongKind(_)) => return Err(error),
        };

        self.lookup_op_global(path)
    }

    fn lookup_var_scoped(&mut self, path: Spanned<Path>) -> LookupResult<VarIndex> {
        let path = match self
            .lookup_scoped_only(path, NameKind::Var.into())
            .map_found(|scoped_index| match scoped_index {
                ScopedIndex::Var(var_index) => var_index,
                _ => unreachable!(),
            }) {
            Ok(found) => return Ok(found),
            Err(LookupError::Undefined(error)) => error.path,
            Err(error @ LookupError::WrongKind(_)) => return Err(error),
        };

        self.lookup_var_global(path)
    }

    fn lookup_var_or_label_scoped(&mut self, path: Spanned<Path>) -> LookupResult<ScopedIndex> {
        let path = match self.lookup_scoped_only(path, NameKind::Var | NameKind::Label) {
            Ok(found) => return Ok(found),
            Err(LookupError::Undefined(error)) => error.path,
            Err(error @ LookupError::WrongKind(_)) => return Err(error),
        };

        self.lookup_global(path, NameKind::Var | NameKind::Label)
            .map_found(|global_index| match global_index {
                GlobalIndex::Var(var_index) => ScopedIndex::Var(var_index),
                _ => unreachable!(),
            })
    }

    fn lookup_fn_scoped(&mut self, path: Spanned<Path>) -> LookupResult<FnIndex> {
        let path = match self.lookup_scoped_only(path, NameKind::Fn.into()) {
            Ok(_) => unreachable!(),
            Err(LookupError::Undefined(error)) => error.path,
            Err(error @ LookupError::WrongKind(_)) => return Err(error),
        };

        self.lookup_fn_global(path)
    }

    fn lookup_label_scoped(&mut self, path: Spanned<Path>) -> LookupResult<LabelIndex> {
        let path = match self
            .lookup_scoped_only(path, NameKind::Label.into())
            .map_found(|scoped_index| match scoped_index {
                ScopedIndex::Label(label_index) => label_index,
                _ => unreachable!(),
            }) {
            Ok(found) => return Ok(found),
            Err(LookupError::Undefined(error)) => error.path,
            Err(error @ LookupError::WrongKind(_)) => return Err(error),
        };

        match self.lookup_global(path, NameKind::Label.into()) {
            Ok(_) => unreachable!(),
            Err(error) => Err(error),
        }
    }

    fn lookup_scoped_only(
        &mut self,
        path: Spanned<Path>,
        expected: EnumSet<NameKind>,
    ) -> LookupResult<ScopedIndex> {
        if path.value.has_parent() {
            Err(LookupError::Undefined(UndefinedError { path, expected }))
        } else {
            self.scoped_registry
                .lookup(path.map(|path| path.ident()), expected)
                .map_found(|scoped_index| *scoped_index)
        }
    }
}

// * Global/Scoped Indexes and Registration

#[derive(Clone, Copy, From)]
enum GlobalIndex {
    #[from(types(BuiltInType, EnumIndex, StructIndex))]
    Type(TypeIndex),
    #[from]
    Ir(IrIndex),
    #[from(types(
        BuiltInVar,
        EnumVariantIndex,
        GlobalVarIndex,
        VarParamIndex,
        LocalVarIndex
    ))]
    Var(VarIndex),
    #[from]
    Fn(FnIndex),
    #[from]
    Op(OpIndex),
}

impl Registrable for GlobalIndex {
    fn name_kind(&self) -> NameKind {
        match self {
            GlobalIndex::Type(_) => NameKind::Type,
            GlobalIndex::Ir(_) => NameKind::Ir,
            GlobalIndex::Var(_) => NameKind::Var,
            GlobalIndex::Fn(_) => NameKind::Fn,
            GlobalIndex::Op(_) => NameKind::Op,
        }
    }

    fn shadows(&self) -> bool {
        false
    }
}

type GlobalRegistry = Registry<Path, GlobalIndex>;

#[derive(Clone, Copy, From)]
enum ScopedIndex {
    #[from(types(VarParamIndex, LocalVarIndex))]
    Var(VarIndex),
    #[from(types(LabelParamIndex, LocalLabelIndex))]
    Label(LabelIndex),
}

impl Registrable for ScopedIndex {
    fn name_kind(&self) -> NameKind {
        match self {
            ScopedIndex::Var(_) => NameKind::Var,
            ScopedIndex::Label(_) => NameKind::Label,
        }
    }

    fn shadows(&self) -> bool {
        match self {
            ScopedIndex::Var(VarIndex::Local(_)) => true,
            ScopedIndex::Label(LabelIndex::Local(_)) => true,
            _ => false,
        }
    }
}

type ScopedRegistry = Registry<Ident, ScopedIndex>;

// * Nesting Relationship Representation

#[derive(Clone, Copy, From, Into)]
struct ImplIndex(usize);

#[derive(Clone, Copy, From)]
enum UnresolvedParentIndex {
    Impl(ImplIndex),
    Ir(IrIndex),
}

impl UnresolvedParentIndex {
    fn name_kind(&self) -> NameKind {
        match self {
            UnresolvedParentIndex::Impl(_) => NameKind::Type,
            UnresolvedParentIndex::Ir(_) => NameKind::Ir,
        }
    }
}

deref_from!(&ImplIndex => UnresolvedParentIndex);
deref_from!(&IrIndex => UnresolvedParentIndex);

struct NestableItem<T> {
    parent: Option<(Spanned<Path>, UnresolvedParentIndex)>,
    path: Spanned<Path>,
    item: T,
}

// * Lookup Result Helper Functions

trait LookupResultExt<T> {
    fn found(self, errors: &mut Vec<ResolveError>) -> Option<T>;

    fn map_found<U>(self, f: impl FnOnce(T) -> U) -> LookupResult<U>;
}

impl<T> LookupResultExt<T> for LookupResult<T> {
    fn found(self, errors: &mut Vec<ResolveError>) -> Option<T> {
        match self {
            Ok((value, _)) => value,
            Err(error) => {
                errors.push(error.into());
                None
            }
        }
    }

    fn map_found<U>(self, f: impl FnOnce(T) -> U) -> LookupResult<U> {
        match self {
            Ok((value, defined_at)) => Ok((value.map(f), defined_at)),
            Err(error) => Err(error),
        }
    }
}
