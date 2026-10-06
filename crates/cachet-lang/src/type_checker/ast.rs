// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::ops::{Index, IndexMut};

use derive_more::From;
use enumset::EnumSet;
use thiserror::Error;
use typed_index_collections::TiVec;

use cachet_util::{box_from, deref_from, deref_index, field_index};

use crate::ast::{
    BinOper, BlockKind, CastSafety, CheckKind, ForInOrder, Ident, NegateKind, Path, Spanned,
};
use crate::built_in::{BuiltInAttr, BuiltInType, BuiltInVar};
use crate::resolver;
pub use crate::resolver::{
    CallableIndex, EnumIndex, EnumItem, EnumVariantIndex, Field, FieldIndex, FnIndex,
    GlobalVarIndex, HasAttrs, IrIndex, IrItem, Label, LabelIndex, LabelParam, LabelParamIndex,
    LabelStmt, Literal, LocalLabelIndex, LocalVarIndex, OpIndex, OutLabel, OutVar, ParamIndex,
    Params, ParentIndex, StructFieldIndex, StructIndex, StructItem, TypeIndex, Typed, VarField,
    VarIndex, VarParam, VarParamIndex, VariantIndex,
};

#[derive(Clone, Debug)]
pub struct Env {
    pub enum_items: TiVec<EnumIndex, EnumItem>,
    pub struct_items: TiVec<StructIndex, StructItem>,
    pub ir_items: TiVec<IrIndex, IrItem>,
    pub global_var_items: TiVec<GlobalVarIndex, GlobalVarItem>,
    pub fn_items: TiVec<FnIndex, CallableItem>,
    pub op_items: TiVec<OpIndex, CallableItem>,
    pub decl_order: Vec<DeclIndex>,
}

field_index!(Env:enum_items[EnumIndex] => EnumItem);
field_index!(Env:struct_items[StructIndex] => StructItem);
field_index!(Env:ir_items[IrIndex] => IrItem);
field_index!(Env:global_var_items[GlobalVarIndex] => GlobalVarItem);
field_index!(Env:fn_items[FnIndex] => CallableItem);
field_index!(Env:op_items[OpIndex] => CallableItem);

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
pub enum DeclIndex {
    Enum(EnumIndex),
    Struct(StructIndex),
    GlobalVar(GlobalVarIndex),
    Fn(FnIndex),
    Op(OpIndex),
}

deref_from!(&EnumIndex => DeclIndex);
deref_from!(&StructIndex => DeclIndex);
deref_from!(&OpIndex => DeclIndex);
deref_from!(&FnIndex => DeclIndex);

impl TryFrom<TypeIndex> for DeclIndex {
    type Error = NotPartOfDeclOrderError;

    fn try_from(type_index: TypeIndex) -> Result<Self, Self::Error> {
        match type_index {
            TypeIndex::BuiltIn(_) => Err(NotPartOfDeclOrderError),
            TypeIndex::Enum(enum_index) => Ok(enum_index.into()),
            TypeIndex::Struct(struct_index) => Ok(struct_index.into()),
        }
    }
}

impl TryFrom<&TypeIndex> for DeclIndex {
    type Error = NotPartOfDeclOrderError;

    fn try_from(src: &TypeIndex) -> Result<Self, Self::Error> {
        TryFrom::try_from(*src)
    }
}

impl TryFrom<VarIndex> for DeclIndex {
    type Error = NotPartOfDeclOrderError;

    fn try_from(var_index: VarIndex) -> Result<Self, Self::Error> {
        match var_index {
            VarIndex::BuiltIn(_)
            | VarIndex::EnumVariant(_)
            | VarIndex::Param(_)
            | VarIndex::Local(_) => Err(NotPartOfDeclOrderError),
            VarIndex::Global(global_var_index) => Ok(global_var_index.into()),
        }
    }
}

impl TryFrom<&VarIndex> for DeclIndex {
    type Error = NotPartOfDeclOrderError;

    fn try_from(src: &VarIndex) -> Result<Self, Self::Error> {
        TryFrom::try_from(*src)
    }
}

impl From<CallableIndex> for DeclIndex {
    fn from(callable_index: CallableIndex) -> DeclIndex {
        match callable_index {
            CallableIndex::Op(op_index) => DeclIndex::Op(op_index),
            CallableIndex::Fn(fn_index) => DeclIndex::Fn(fn_index),
        }
    }
}

deref_from!(&CallableIndex => DeclIndex);

#[derive(Clone, Copy, Debug, Default, Error)]
#[error("item is not part of the declaration order")]
pub struct NotPartOfDeclOrderError;

#[derive(Clone, Debug)]
pub struct GlobalVarItem {
    pub path: Spanned<Path>,
    pub parent: Option<ParentIndex>,
    pub is_mut: bool,
    pub type_: TypeIndex,
    pub value: Option<Spanned<Expr>>,
    pub attrs: EnumSet<BuiltInAttr>,
}

impl HasAttrs for GlobalVarItem {
    fn attrs(&self) -> &EnumSet<BuiltInAttr> {
        &self.attrs
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
    pub ret: TypeIndex,
    pub interprets: Option<IrIndex>,
    pub emits: Option<IrIndex>,
    pub body: Option<Body>,
}

impl HasAttrs for CallableItem {
    fn attrs(&self) -> &EnumSet<BuiltInAttr> {
        &self.attrs
    }
}

impl Typed for CallableItem {
    fn type_(&self) -> TypeIndex {
        self.ret
    }
}

#[derive(Clone, Debug, From)]
pub enum Arg {
    Expr(Expr),
    OutVar(OutVarArg),
    Label(LabelArg),
    LabelField(LabelFieldArg),
    OutLabel(OutLabelArg),
}

#[derive(Clone, Debug)]
pub struct OutVarArg {
    pub out_var: OutVar,
    pub type_: TypeIndex,
    pub upcast_route: Vec<TypeIndex>,
}

impl Typed for OutVarArg {
    fn type_(&self) -> TypeIndex {
        self.type_
    }
}

#[derive(Clone, Debug)]
pub struct LabelArg {
    pub label: Spanned<LabelIndex>,
    pub ir: IrIndex,
}

#[derive(Clone, Debug)]
pub struct LabelFieldArg {
    pub parent: Expr,
    pub field: StructFieldIndex,
    pub ir: IrIndex,
}

#[derive(Clone, Debug)]
pub struct OutLabelArg {
    pub out_label: OutLabel,
    pub ir: IrIndex,
}

#[derive(Clone, Debug)]
pub struct Call {
    pub target: CallableIndex,
    pub is_unsafe: bool,
    pub args: Vec<Arg>,
}

#[derive(Clone, Debug)]
pub struct Body {
    pub locals: Locals,
    pub block: Block,
}

impl Typed for Body {
    fn type_(&self) -> TypeIndex {
        self.block.value.type_()
    }
}

field_index!(Body:locals[LocalVarIndex] => LocalVar);
field_index!(Body:locals[LocalLabelIndex] => Label);

#[derive(Clone, Debug, Default)]
pub struct Locals {
    pub local_vars: TiVec<LocalVarIndex, LocalVar>,
    pub local_labels: TiVec<LocalLabelIndex, Label>,
}

field_index!(Locals:local_vars[LocalVarIndex] => LocalVar);
field_index!(Locals:local_labels[LocalLabelIndex] => Label);

#[derive(Clone, Debug)]
pub struct LocalVar {
    pub ident: Spanned<Ident>,
    pub is_mut: bool,
    pub type_: TypeIndex,
}

impl Typed for LocalVar {
    fn type_(&self) -> TypeIndex {
        self.type_
    }
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub value: Expr,
    pub exits_early: bool,
}

impl Typed for Block {
    fn type_(&self) -> TypeIndex {
        self.value.type_()
    }
}

#[derive(Clone, Debug)]
pub struct KindedBlock {
    pub kind: Option<BlockKind>,
    pub block: Block,
}

impl Typed for KindedBlock {
    fn type_(&self) -> TypeIndex {
        self.block.type_()
    }
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
    #[from]
    Emit(EmitStmt),
    #[from]
    Ret(RetStmt),
    Unreachable,
    #[from]
    Expr(Expr),
}

impl Typed for Stmt {
    fn type_(&self) -> TypeIndex {
        match self {
            Self::Let(let_stmt) => let_stmt.type_(),
            Self::Label(label_stmt) => label_stmt.type_(),
            Self::ForIn(for_in_stmt) => for_in_stmt.type_(),
            Self::Check(check_stmt) => check_stmt.type_(),
            Self::Goto(goto_stmt) => goto_stmt.type_(),
            Self::Bind(bind_stmt) => bind_stmt.type_(),
            Self::Emit(emit_stmt) => emit_stmt.type_(),
            Self::Ret(ret_stmt) => ret_stmt.type_(),
            Self::Unreachable => BuiltInType::Unit.into(),
            // The final value of an expression statement is ignored, so the
            // statement itself is inherently unit-typed.
            Self::Expr(_) => BuiltInType::Unit.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LetStmt {
    pub lhs: LocalVarIndex,
    pub rhs: Expr,
}

impl Typed for LetStmt {
    fn type_(&self) -> TypeIndex {
        BuiltInType::Unit.into()
    }
}

#[derive(Clone, Debug)]
pub struct IfExpr {
    pub cond: Expr,
    pub then: Block,
    pub else_: Option<ElseClause>,
}

impl Typed for IfExpr {
    /// An arm that never reaches its value contributes no type, so the type is
    /// whichever arm does contribute one -- which is what makes
    /// `if c { unreachable; } else { 1_i32 }` an `Int32`. With no `else`, the other
    /// path falls through producing nothing, so neither does the `if`.
    ///
    /// `type_check_if_expr` decides from the same rule which arms have to agree, and
    /// the two must stay in step.
    fn type_(&self) -> TypeIndex {
        if !self.then.exits_early {
            return self.then.type_();
        }
        match &self.else_ {
            Some(else_) => else_.type_(),
            None => BuiltInType::Unit.into(),
        }
    }
}

#[derive(Clone, Debug, From)]
pub enum ElseClause {
    #[from]
    ElseIf(Box<IfExpr>),
    #[from]
    Else(Block),
}

impl Typed for ElseClause {
    fn type_(&self) -> TypeIndex {
        match self {
            Self::ElseIf(if_expr) => if_expr.type_(),
            Self::Else(block) => block.type_(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ForInStmt {
    pub var: LocalVarIndex,
    pub target: EnumIndex,
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
    pub cond: Expr,
}

impl Typed for CheckStmt {
    fn type_(&self) -> TypeIndex {
        BuiltInType::Unit.into()
    }
}

#[derive(Clone, Debug)]
pub struct GotoStmt {
    pub label: LabelIndex,
    pub ir: IrIndex,
}

impl Typed for GotoStmt {
    fn type_(&self) -> TypeIndex {
        BuiltInType::Unit.into()
    }
}

#[derive(Clone, Debug)]
pub struct BindStmt {
    pub label: LabelIndex,
    pub ir: IrIndex,
}

impl Typed for BindStmt {
    fn type_(&self) -> TypeIndex {
        BuiltInType::Unit.into()
    }
}

#[derive(Clone, Debug)]
pub struct EmitStmt {
    pub call: Call,
    pub ir: IrIndex,
}

impl Typed for EmitStmt {
    fn type_(&self) -> TypeIndex {
        BuiltInType::Unit.into()
    }
}

#[derive(Clone, Debug)]
pub struct RetStmt {
    pub value: Expr,
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
    #[from(types(BuiltInVar, "&BuiltInVar"))]
    Var(VarExpr),
    #[from]
    Invoke(InvokeExpr),
    #[from]
    FieldAccess(Box<FieldAccessExpr>),
    #[from]
    Negate(Box<NegateExpr>),
    #[from]
    Cast(Box<CastExpr>),
    #[from]
    BinOper(Box<BinOperExpr>),
    #[from]
    Assign(Box<AssignExpr>),
}

impl Typed for Expr {
    fn type_(&self) -> TypeIndex {
        match self {
            Expr::Block(block_expr) => block_expr.type_(),
            Expr::If(if_expr) => if_expr.type_(),
            Expr::Literal(literal) => literal.type_(),
            Expr::Var(var_expr) => var_expr.type_(),
            Expr::Invoke(call_expr) => call_expr.type_(),
            Expr::FieldAccess(field_access_expr) => field_access_expr.type_(),
            Expr::Negate(negate_expr) => negate_expr.type_(),
            Expr::Cast(cast_expr) => cast_expr.type_(),
            Expr::Assign(assign_expr) => assign_expr.type_(),
            Expr::BinOper(bin_oper_expr) => bin_oper_expr.type_(),
        }
    }
}

box_from!(KindedBlock => Expr);
box_from!(IfExpr => Expr);
box_from!(FieldAccessExpr => Expr);
box_from!(NegateExpr => Expr);
box_from!(CastExpr => Expr);
box_from!(BinOperExpr => Expr);
box_from!(AssignExpr => Expr);

deref_from!(&Literal => Expr);

impl From<Block> for Expr {
    fn from(block: Block) -> Self {
        KindedBlock::from(block).into()
    }
}

#[derive(Clone, Debug)]
pub struct VarExpr {
    pub var: VarIndex,
    pub type_: TypeIndex,
}

impl Typed for VarExpr {
    fn type_(&self) -> TypeIndex {
        self.type_
    }
}

impl From<BuiltInVar> for VarExpr {
    fn from(built_in_var: BuiltInVar) -> Self {
        VarExpr {
            var: built_in_var.into(),
            type_: built_in_var.type_().into(),
        }
    }
}

deref_from!(&BuiltInVar => VarExpr);

#[derive(Clone, Debug)]
pub struct InvokeExpr {
    pub call: Call,
    pub ret: TypeIndex,
}

impl Typed for InvokeExpr {
    fn type_(&self) -> TypeIndex {
        self.ret
    }
}

#[derive(Clone, Debug)]
pub struct FieldAccessExpr {
    pub parent: Expr,
    pub field: StructFieldIndex,
    pub type_: TypeIndex,
}

impl Typed for FieldAccessExpr {
    fn type_(&self) -> TypeIndex {
        self.type_
    }
}

#[derive(Clone, Debug)]
pub struct NegateExpr {
    pub kind: NegateKind,
    pub expr: Expr,
}

impl Typed for NegateExpr {
    fn type_(&self) -> TypeIndex {
        self.expr.type_()
    }
}

#[derive(Clone, Debug)]
pub struct CastExpr {
    pub kind: CastSafety,
    pub expr: Expr,
    pub type_: TypeIndex,
}

impl Typed for CastExpr {
    fn type_(&self) -> TypeIndex {
        self.type_
    }
}

#[derive(Clone, Debug)]
pub struct BinOperExpr {
    pub oper: BinOper,
    pub lhs: Expr,
    pub rhs: Expr,
    pub type_: TypeIndex,
}

impl Typed for BinOperExpr {
    fn type_(&self) -> TypeIndex {
        self.type_
    }
}

#[derive(Clone, Debug)]
pub struct AssignExpr {
    pub lhs: Spanned<VarIndex>,
    pub rhs: Expr,
}

impl AssignExpr {
    pub const TYPE: BuiltInType = resolver::AssignExpr::TYPE;
}

impl Typed for AssignExpr {
    fn type_(&self) -> TypeIndex {
        AssignExpr::TYPE.into()
    }
}
