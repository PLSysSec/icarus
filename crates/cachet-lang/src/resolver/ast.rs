// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::ops::{Index, IndexMut};

use derive_more::From;
use enumset::EnumSet;
use typed_index_collections::TiVec;

use cachet_util::{box_from, deref_from, deref_index, field_index, typed_field_index};

use crate::ast::{
    BinOper, BlockKind, CheckKind, ForInOrder, Ident, MaybeSpanned, NegateKind, Path, Spanned,
    VarParamKind,
};
use crate::built_in::{BuiltInAttr, BuiltInType, BuiltInVar};
pub use crate::parser::{FieldIndex, Literal, VariantIndex};

#[derive(Clone, Debug)]
pub struct Env {
    pub enum_items: TiVec<EnumIndex, EnumItem>,
    pub struct_items: TiVec<StructIndex, StructItem>,
    pub ir_items: TiVec<IrIndex, IrItem>,
    pub global_var_items: TiVec<GlobalVarIndex, GlobalVarItem>,
    pub fn_items: TiVec<FnIndex, CallableItem>,
    pub op_items: TiVec<OpIndex, CallableItem>,
}

typed_field_index!(Env:enum_items[pub EnumIndex] => EnumItem);
typed_field_index!(Env:struct_items[pub StructIndex] => StructItem);
typed_field_index!(Env:ir_items[pub IrIndex] => IrItem);
typed_field_index!(Env:global_var_items[pub GlobalVarIndex] => GlobalVarItem);
typed_field_index!(Env:fn_items[pub FnIndex] => CallableItem);
typed_field_index!(Env:op_items[pub OpIndex] => CallableItem);

#[derive(Clone, Copy, Debug, Eq, From, Hash, PartialEq)]
pub enum TypeIndex {
    BuiltIn(BuiltInType),
    Enum(EnumIndex),
    Struct(StructIndex),
}

impl TypeIndex {
    pub fn is_numeric(&self) -> bool {
        match self {
            Self::BuiltIn(built_in_type) => built_in_type.is_numeric(),
            Self::Enum(_) | Self::Struct(_) => false,
        }
    }

    pub fn is_signed_numeric(&self) -> bool {
        match self {
            Self::BuiltIn(built_in_type) => built_in_type.is_signed_numeric(),
            Self::Enum(_) | Self::Struct(_) => false,
        }
    }

    pub fn is_integral(&self) -> bool {
        match self {
            Self::BuiltIn(built_in_type) => built_in_type.is_integral(),
            Self::Enum(_) | Self::Struct(_) => false,
        }
    }
}

deref_from!(&BuiltInType => TypeIndex);
deref_from!(&EnumIndex => TypeIndex);
deref_from!(&StructIndex => TypeIndex);

#[derive(Clone, Copy, Debug, Eq, From, Hash, PartialEq)]
pub enum VarIndex {
    BuiltIn(BuiltInVar),
    EnumVariant(EnumVariantIndex),
    Global(GlobalVarIndex),
    Param(VarParamIndex),
    Local(LocalVarIndex),
}

deref_from!(&BuiltInVar => VarIndex);
deref_from!(&EnumVariantIndex => VarIndex);
deref_from!(&GlobalVarIndex => VarIndex);
deref_from!(&VarParamIndex => VarIndex);
deref_from!(&LocalVarIndex => VarIndex);

#[derive(Clone, Copy, Debug, Eq, From, Hash, PartialEq)]
pub enum CallableIndex {
    Fn(FnIndex),
    Op(OpIndex),
}

deref_from!(&FnIndex => CallableIndex);
deref_from!(&OpIndex => CallableIndex);

impl Index<CallableIndex> for Env {
    type Output = CallableItem;

    fn index(&self, index: CallableIndex) -> &Self::Output {
        match index {
            CallableIndex::Fn(fn_index) => &self.fn_items[fn_index],
            CallableIndex::Op(op_index) => &self.op_items[op_index],
        }
    }
}

impl IndexMut<CallableIndex> for Env {
    fn index_mut(&mut self, index: CallableIndex) -> &mut Self::Output {
        match index {
            CallableIndex::Fn(fn_index) => &mut self.fn_items[fn_index],
            CallableIndex::Op(op_index) => &mut self.op_items[op_index],
        }
    }
}

deref_index!(Env[&CallableIndex] => CallableItem);

#[derive(Clone, Copy, Debug, Eq, From, Hash, PartialEq)]
pub enum ParentIndex {
    #[from(types(
        BuiltInType,
        "&BuiltInType",
        EnumIndex,
        "&EnumIndex",
        StructIndex,
        "&StructIndex"
    ))]
    Type(TypeIndex),
    #[from]
    Ir(IrIndex),
}

deref_from!(&TypeIndex => ParentIndex);
deref_from!(&IrIndex => ParentIndex);

#[derive(Clone, Copy, Debug, Eq, From, Hash, PartialEq)]
pub enum LabelIndex {
    Param(LabelParamIndex),
    Local(LocalLabelIndex),
}

deref_from!(&LabelParamIndex => LabelIndex);
deref_from!(&LocalLabelIndex => LabelIndex);

pub trait HasAttrs {
    fn attrs(&self) -> &EnumSet<BuiltInAttr>;

    fn is_inline(&self) -> bool {
        self.attrs().contains(BuiltInAttr::Inline)
    }

    fn is_prelude(&self) -> bool {
        self.attrs().contains(BuiltInAttr::Prelude)
    }

    fn is_refined(&self) -> bool {
        self.attrs().contains(BuiltInAttr::Refined)
    }

    fn is_spec(&self) -> bool {
        self.attrs().contains(BuiltInAttr::Spec)
    }
}

pub trait Typed {
    fn type_(&self) -> TypeIndex;
}

impl<T: Typed> Typed for Box<T> {
    fn type_(&self) -> TypeIndex {
        (**self).type_()
    }
}

impl<T: Typed> Typed for Spanned<T> {
    fn type_(&self) -> TypeIndex {
        self.value.type_()
    }
}

impl<T: Typed> Typed for MaybeSpanned<T> {
    fn type_(&self) -> TypeIndex {
        self.value.type_()
    }
}

impl Typed for BuiltInType {
    fn type_(&self) -> TypeIndex {
        self.into()
    }
}

impl Typed for BuiltInVar {
    fn type_(&self) -> TypeIndex {
        self.built_in_type().into()
    }
}

impl Typed for TypeIndex {
    fn type_(&self) -> TypeIndex {
        *self
    }
}

impl Typed for Literal {
    fn type_(&self) -> TypeIndex {
        match self {
            Self::Int8(_) => BuiltInType::INT8.into(),
            Self::Int16(_) => BuiltInType::INT16.into(),
            Self::Int32(_) => BuiltInType::INT32.into(),
            Self::Int64(_) => BuiltInType::INT64.into(),
            Self::UInt8(_) => BuiltInType::UINT8.into(),
            Self::UInt16(_) => BuiltInType::UINT16.into(),
            Self::UInt32(_) => BuiltInType::UINT32.into(),
            Self::UInt64(_) => BuiltInType::UINT64.into(),
            Self::Double(_) => BuiltInType::Double.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct EnumItem {
    pub ident: Spanned<Ident>,
    pub attrs: EnumSet<BuiltInAttr>,
    pub variants: TiVec<VariantIndex, Spanned<Path>>,
}

impl HasAttrs for EnumItem {
    fn attrs(&self) -> &EnumSet<BuiltInAttr> {
        &self.attrs
    }
}

field_index!(EnumItem:variants[VariantIndex] => Spanned<Path>);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EnumVariantIndex {
    pub enum_index: EnumIndex,
    pub variant_index: VariantIndex,
}

impl Typed for EnumVariantIndex {
    fn type_(&self) -> TypeIndex {
        TypeIndex::Enum(self.enum_index)
    }
}

impl Index<EnumVariantIndex> for Env {
    type Output = Spanned<Path>;

    fn index(&self, index: EnumVariantIndex) -> &Self::Output {
        &self.enum_items[index.enum_index][index.variant_index]
    }
}

impl IndexMut<EnumVariantIndex> for Env {
    fn index_mut(&mut self, index: EnumVariantIndex) -> &mut Self::Output {
        &mut self.enum_items[index.enum_index][index.variant_index]
    }
}

deref_index!(Env[&EnumVariantIndex] => Spanned<Path>);

#[derive(Clone, Debug)]
pub struct StructItem {
    pub ident: Spanned<Ident>,
    pub attrs: EnumSet<BuiltInAttr>,
    pub supertype: Option<TypeIndex>,
    pub fields: TiVec<FieldIndex, Field>,
}

impl HasAttrs for StructItem {
    fn attrs(&self) -> &EnumSet<BuiltInAttr> {
        &self.attrs
    }
}

field_index!(StructItem:fields[FieldIndex] => Field);

#[derive(Clone, Copy, Debug, Eq, From, Hash, PartialEq)]
pub struct StructFieldIndex {
    pub struct_index: StructIndex,
    pub field_index: FieldIndex,
}

impl Index<StructFieldIndex> for Env {
    type Output = Field;

    fn index(&self, index: StructFieldIndex) -> &Self::Output {
        &self.struct_items[index.struct_index][index.field_index]
    }
}

impl IndexMut<StructFieldIndex> for Env {
    fn index_mut(&mut self, index: StructFieldIndex) -> &mut Self::Output {
        &mut self.struct_items[index.struct_index][index.field_index]
    }
}

deref_index!(Env[&StructFieldIndex] => Field);

#[derive(Clone, Debug, From)]
pub enum Field {
    Var(VarField),
    Label(Label),
}

impl Field {
    pub fn ident(&self) -> Spanned<Ident> {
        match self {
            Self::Var(var_field) => var_field.ident,
            Self::Label(label) => label.ident,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VarField {
    pub ident: Spanned<Ident>,
    pub type_: TypeIndex,
}

#[derive(Clone, Debug)]
pub struct IrItem {
    pub ident: Spanned<Ident>,
    pub emits: Option<Spanned<IrIndex>>,
}

#[derive(Clone, Debug)]
pub struct GlobalVarItem {
    pub path: Spanned<Path>,
    pub parent: Option<ParentIndex>,
    pub attrs: EnumSet<BuiltInAttr>,
    pub is_mut: bool,
    pub type_: TypeIndex,
    pub value: Option<Spanned<Expr>>,
}

impl HasAttrs for GlobalVarItem {
    fn attrs(&self) -> &EnumSet<BuiltInAttr> {
        &self.attrs
    }
}

impl Typed for GlobalVarItem {
    fn type_(&self) -> TypeIndex {
        self.type_
    }
}

#[derive(Clone, Debug)]
pub struct CallableItem {
    pub path: Spanned<Path>,
    pub parent: Option<ParentIndex>,
    pub attrs: EnumSet<BuiltInAttr>,
    pub is_unsafe: bool,
    pub params: Params,
    pub param_order: Vec<ParamIndex>,
    pub emits: Option<Spanned<IrIndex>>,
    pub ret: Option<Spanned<TypeIndex>>,
    pub body: Spanned<Option<Body>>,
}

impl HasAttrs for CallableItem {
    fn attrs(&self) -> &EnumSet<BuiltInAttr> {
        &self.attrs
    }
}

impl Typed for CallableItem {
    fn type_(&self) -> TypeIndex {
        match self.ret {
            Some(ret) => ret.value,
            None => BuiltInType::Unit.into(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Params {
    pub var_params: TiVec<VarParamIndex, VarParam>,
    pub label_params: TiVec<LabelParamIndex, LabelParam>,
}

typed_field_index!(Params:var_params[pub VarParamIndex] => VarParam);
typed_field_index!(Params:label_params[pub LabelParamIndex] => LabelParam);

#[derive(Clone, Copy, Debug, Eq, From, Hash, PartialEq)]
pub enum ParamIndex {
    Var(VarParamIndex),
    Label(LabelParamIndex),
}

deref_from!(&VarParamIndex => ParamIndex);
deref_from!(&LabelParamIndex => ParamIndex);

#[derive(Clone, Debug)]
pub struct VarParam {
    pub ident: Spanned<Ident>,
    pub kind: VarParamKind,
    pub type_: TypeIndex,
}

impl Typed for VarParam {
    fn type_(&self) -> TypeIndex {
        self.type_
    }
}

#[derive(Clone, Debug)]
pub struct LabelParam {
    pub label: Label,
    pub is_out: bool,
}

impl From<Label> for LabelParam {
    fn from(label: Label) -> Self {
        Self {
            label,
            is_out: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Label {
    pub ident: Spanned<Ident>,
    pub ir: IrIndex,
}

#[derive(Clone, Debug, From)]
pub enum Arg {
    Expr(Expr),
    /// Field accesses directly in the argument position can't be disambiguated
    /// between accesses to variable fields or accesses to label fields until
    /// type-checking.
    FieldAccess(FieldAccess),
    OutVar(OutVar),
    Label(LabelIndex),
    OutLabel(OutLabel),
}

deref_from!(&LabelIndex => Arg);

#[derive(Clone, Debug)]
pub struct FieldAccess {
    pub parent: Spanned<Expr>,
    pub field: Spanned<Ident>,
}

#[derive(Clone, Copy, Debug)]
pub enum OutVar {
    Free(Spanned<VarIndex>),
    Fresh(LocalVarIndex),
}

#[derive(Clone, Copy, Debug)]
pub enum OutLabel {
    Free(Spanned<LabelIndex>),
    Fresh(LocalLabelIndex),
}

#[derive(Clone, Debug)]
pub struct Call {
    pub target: Spanned<CallableIndex>,
    pub args: Spanned<Vec<Spanned<Arg>>>,
}

#[derive(Clone, Debug)]
pub struct Body {
    pub locals: Locals,
    pub block: Block,
}

field_index!(Body:locals[LocalVarIndex] => LocalVar);
field_index!(Body:locals[LocalLabelIndex] => LocalLabel);

#[derive(Clone, Debug, Default)]
pub struct Locals {
    pub local_vars: TiVec<LocalVarIndex, LocalVar>,
    pub local_labels: TiVec<LocalLabelIndex, LocalLabel>,
}

typed_field_index!(Locals:local_vars[pub LocalVarIndex] => LocalVar);
typed_field_index!(Locals:local_labels[pub LocalLabelIndex] => LocalLabel);

#[derive(Clone, Debug)]
pub struct LocalVar {
    pub ident: Spanned<Ident>,
    pub is_mut: bool,
    pub type_: Option<TypeIndex>,
}

#[derive(Clone, Debug)]
pub struct LocalLabel {
    pub ident: Spanned<Ident>,
    pub ir: Option<IrIndex>,
}

impl From<Label> for LocalLabel {
    fn from(label: Label) -> Self {
        LocalLabel {
            ident: label.ident,
            ir: Some(label.ir),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Spanned<Stmt>>,
    pub value: Spanned<Option<Expr>>,
}

#[derive(Clone, Debug)]
pub struct KindedBlock {
    pub kind: Option<BlockKind>,
    pub block: Block,
}

impl From<Block> for KindedBlock {
    fn from(block: Block) -> KindedBlock {
        KindedBlock { kind: None, block }
    }
}

#[derive(Clone, Debug, From)]
pub enum Stmt {
    #[from]
    Let(LetStmt),
    #[from]
    Label(LabelStmt),
    #[from]
    ForIn(ForInStmt),
    #[from]
    Check(CheckStmt),
    #[from]
    Goto(GotoStmt),
    #[from]
    Bind(BindStmt),
    Emit(Call),
    #[from]
    Ret(RetStmt),
    Unreachable,
    /// An expression in statement position with no trailing semicolon, which
    /// requires it to be unit-typed.
    Expr(Expr),
    /// An expression in statement position with a trailing semicolon, which
    /// discards its value whatever the type.
    Semi(Expr),
}

#[derive(Clone, Debug)]
pub struct LetStmt {
    pub lhs: LocalVarIndex,
    pub rhs: Spanned<Expr>,
}

impl Typed for LetStmt {
    fn type_(&self) -> TypeIndex {
        BuiltInType::Unit.into()
    }
}

#[derive(Clone, Debug, From)]
pub struct LabelStmt {
    pub label: LocalLabelIndex,
}

impl Typed for LabelStmt {
    fn type_(&self) -> TypeIndex {
        BuiltInType::Unit.into()
    }
}

#[derive(Clone, Debug)]
pub struct IfExpr {
    pub cond: Spanned<Expr>,
    pub then: Block,
    pub else_: Option<ElseClause>,
}

#[derive(Clone, Debug, From)]
pub enum ElseClause {
    #[from]
    ElseIf(Box<IfExpr>),
    #[from]
    Else(Block),
}

#[derive(Clone, Debug, From)]
pub struct ForInStmt {
    pub var: LocalVarIndex,
    pub target: TypeIndex,
    pub order: ForInOrder,
    pub body: Block,
}

impl Typed for ForInStmt {
    fn type_(&self) -> TypeIndex {
        BuiltInType::Unit.into()
    }
}

#[derive(Clone, Debug)]
pub struct CheckStmt {
    pub kind: CheckKind,
    pub cond: Spanned<Expr>,
}

impl Typed for CheckStmt {
    fn type_(&self) -> TypeIndex {
        BuiltInType::Unit.into()
    }
}

#[derive(Clone, Debug)]
pub struct GotoStmt {
    pub label: Spanned<LabelIndex>,
}

impl Typed for GotoStmt {
    fn type_(&self) -> TypeIndex {
        BuiltInType::Unit.into()
    }
}

#[derive(Clone, Debug)]
pub struct BindStmt {
    pub label: Spanned<LabelIndex>,
}

impl Typed for BindStmt {
    fn type_(&self) -> TypeIndex {
        BuiltInType::Unit.into()
    }
}

#[derive(Clone, Debug)]
pub struct RetStmt {
    pub value: Spanned<Option<Expr>>,
}

impl Typed for RetStmt {
    fn type_(&self) -> TypeIndex {
        BuiltInType::Unit.into()
    }
}

#[derive(Clone, Debug, From)]
pub enum Expr {
    #[from]
    Block(Box<KindedBlock>),
    #[from]
    If(Box<IfExpr>),
    #[from]
    Literal(Literal),
    #[from]
    Var(Spanned<VarIndex>),
    Invoke(Call),
    #[from]
    FieldAccess(Box<FieldAccess>),
    #[from]
    Negate(Box<NegateExpr>),
    #[from]
    Cast(Box<CastExpr>),
    #[from]
    BinOper(Box<BinOperExpr>),
    #[from]
    Assign(Box<AssignExpr>),
}

box_from!(KindedBlock => Expr);
box_from!(IfExpr => Expr);
box_from!(FieldAccess => Expr);
box_from!(NegateExpr => Expr);
box_from!(CastExpr => Expr);
box_from!(BinOperExpr => Expr);
box_from!(AssignExpr => Expr);

impl From<Block> for Expr {
    fn from(block: Block) -> Self {
        KindedBlock::from(block).into()
    }
}

deref_from!(&Literal => Expr);
deref_from!(&Spanned<VarIndex> => Expr);

impl From<Spanned<&VarIndex>> for Expr {
    fn from(var_index: Spanned<&VarIndex>) -> Self {
        var_index.copied().into()
    }
}

#[derive(Clone, Debug)]
pub struct NegateExpr {
    pub kind: Spanned<NegateKind>,
    pub expr: Spanned<Expr>,
}

#[derive(Clone, Debug)]
pub struct CastExpr {
    pub expr: Spanned<Expr>,
    pub type_: Spanned<TypeIndex>,
}

impl Typed for CastExpr {
    fn type_(&self) -> TypeIndex {
        self.type_.value
    }
}

#[derive(Clone, Debug)]
pub struct BinOperExpr {
    pub oper: Spanned<BinOper>,
    pub lhs: Spanned<Expr>,
    pub rhs: Spanned<Expr>,
}

#[derive(Clone, Debug)]
pub struct AssignExpr {
    pub lhs: Spanned<VarIndex>,
    pub rhs: Spanned<Expr>,
}

impl AssignExpr {
    pub const TYPE: BuiltInType = BuiltInType::Unit;
}

impl Typed for AssignExpr {
    fn type_(&self) -> TypeIndex {
        AssignExpr::TYPE.into()
    }
}
