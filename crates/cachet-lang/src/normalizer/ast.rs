// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::ops::{Index, IndexMut};

use derive_more::From;
use typed_index_collections::TiVec;

use crate::ast::{
    ArithKind, BlockKind, CastKind, CheckKind, CompareKind, NegateKind, Path, Spanned,
};
use crate::built_in::{BuiltInType, BuiltInVar};
use crate::type_checker;
pub use crate::type_checker::{
    BindStmt, CallableIndex, DeclIndex, EnumIndex, EnumItem, EnumVariantIndex, FieldIndex,
    FnIndex, GlobalVarIndex, GlobalVarItem, GotoStmt, IrIndex, IrItem, Label, LabelIndex,
    LabelParam, LabelParamIndex, LabelStmt, Literal, LocalLabelIndex, LocalVar, LocalVarIndex,
    Locals, NotPartOfDeclOrderError, OpIndex, OutVar, OutVarArg, ParamIndex, Params, ParentIndex,
    StructField, StructIndex, StructItem, TypeIndex, Typed, VarExpr, VarIndex, VarParam,
    VarParamIndex, VariantIndex,
};
use cachet_util::{box_from, deref_from, deref_index, field_index};

#[derive(Clone, Debug)]
pub struct Env<B = ()> {
    pub enum_items: TiVec<EnumIndex, EnumItem>,
    pub struct_items: TiVec<StructIndex, StructItem>,
    pub ir_items: TiVec<IrIndex, IrItem>,
    pub global_var_items: TiVec<GlobalVarIndex, GlobalVarItem>,
    pub fn_items: TiVec<FnIndex, CallableItem<B>>,
    pub op_items: TiVec<OpIndex, CallableItem<B>>,
    pub decl_order: Vec<DeclIndex>,
}

field_index!(Env<B>:enum_items[EnumIndex] => EnumItem | <B>);
field_index!(Env<B>:struct_items[StructIndex] => StructItem | <B>);
field_index!(Env<B>:ir_items[IrIndex] => IrItem | <B>);
field_index!(Env<B>:global_var_items[GlobalVarIndex] => GlobalVarItem | <B>);
field_index!(Env<B>:fn_items[FnIndex] => CallableItem<B> | <B>);
field_index!(Env<B>:op_items[OpIndex] => CallableItem<B> | <B>);

impl<B> Index<EnumVariantIndex> for Env<B> {
    type Output = Spanned<Path>;

    fn index(&self, index: EnumVariantIndex) -> &Self::Output {
        &self.enum_items[index.enum_index][index.variant_index]
    }
}

impl<B> IndexMut<EnumVariantIndex> for Env<B> {
    fn index_mut(&mut self, index: EnumVariantIndex) -> &mut Self::Output {
        &mut self.enum_items[index.enum_index][index.variant_index]
    }
}

impl<B> Index<FieldIndex> for Env<B> {
    type Output = StructField;

    fn index(&self, index: FieldIndex) -> &Self::Output {
        &self[index.struct_].fields[&index.ident]
    }
}

deref_index!(Env<B>[&EnumVariantIndex] => Spanned<Path> | <B>);

impl<B> Index<CallableIndex> for Env<B> {
    type Output = CallableItem<B>;

    fn index(&self, index: CallableIndex) -> &Self::Output {
        match index {
            CallableIndex::Fn(fn_index) => &self.fn_items[fn_index],
            CallableIndex::Op(op_index) => &self.op_items[op_index],
        }
    }
}

impl<B> IndexMut<CallableIndex> for Env<B> {
    fn index_mut(&mut self, index: CallableIndex) -> &mut Self::Output {
        match index {
            CallableIndex::Fn(fn_index) => &mut self.fn_items[fn_index],
            CallableIndex::Op(op_index) => &mut self.op_items[op_index],
        }
    }
}

deref_index!(Env<B>[&CallableIndex] => CallableItem<B> | <B>);

#[derive(Clone, Debug)]
pub struct CallableItem<B = ()> {
    pub path: Spanned<Path>,
    pub parent: Option<ParentIndex>,
    pub is_unsafe: bool,
    pub params: Params,
    pub param_order: Vec<ParamIndex>,
    pub ret: Option<TypeIndex>,
    pub interprets: Option<IrIndex>,
    pub emits: Option<IrIndex>,
    pub body: Option<Body<B>>,
}

impl<B> Typed for CallableItem<B> {
    fn type_(&self) -> TypeIndex {
        self.ret.unwrap_or_else(|| BuiltInType::Unit.into())
    }
}

#[derive(Clone, Debug, From)]
pub enum Arg {
    Expr(PureExpr),
    OutVar(OutVarArg),
    Label(LabelArg),
}

#[derive(Clone, Debug)]
pub struct LabelArg {
    pub label: LabelIndex,
    pub is_out: bool,
    pub ir: IrIndex,
}

#[derive(Clone, Debug)]
pub struct Call {
    pub target: CallableIndex,
    pub is_unsafe: bool,
    pub args: Vec<Arg>,
}

#[derive(Clone, Debug)]
pub struct Body<B = ()> {
    pub locals: Locals,
    pub stmts: Vec<Stmt<B>>,
}

field_index!(Body<B>:locals[LocalVarIndex] => LocalVar | <B>);
field_index!(Body<B>:locals[LocalLabelIndex] => Label | <B>);

#[derive(Clone, Debug, From)]
pub enum Stmt<B = ()> {
    #[from]
    Let(LetStmt<B>),
    #[from]
    Label(LabelStmt),
    #[from]
    If(IfStmt<B>),
    #[from]
    Check(CheckStmt<B>),
    #[from]
    Goto(GotoStmt),
    #[from]
    Bind(BindStmt),
    #[from]
    Emit(EmitStmt),
    Block(B, BlockStmt<B>),
    #[from]
    Invoke(InvokeStmt),
    #[from]
    Assign(AssignStmt<B>),
    #[from]
    Ret(RetStmt<B>),
}

impl<B: Default> From<BlockStmt<B>> for Stmt<B> {
    fn from(block_stmt: BlockStmt<B>) -> Self {
        Stmt::Block(B::default(), block_stmt)
    }
}

#[derive(Clone, Debug)]
pub struct LetStmt<B = ()> {
    pub lhs: LocalVarIndex,
    pub rhs: Expr<B>,
}

#[derive(Clone, Debug)]
pub struct IfStmt<B = ()> {
    pub cond: Expr<B>,
    pub then: Vec<Stmt<B>>,
    pub else_: Option<ElseClause<B>>,
}

#[derive(Clone, Debug, From)]
pub enum ElseClause<B = ()> {
    #[from]
    ElseIf(Box<IfStmt<B>>),
    #[from]
    Else(Vec<Stmt<B>>),
}

#[derive(Clone, Debug)]
pub struct CheckStmt<B = ()> {
    pub kind: CheckKind,
    pub cond: Expr<B>,
}

#[derive(Clone, Debug)]
pub struct EmitStmt {
    pub call: Call,
    pub ir: IrIndex,
}

#[derive(Clone, Debug)]
pub struct BlockStmt<B = ()> {
    pub kind: Option<BlockKind>,
    pub stmts: Vec<Stmt<B>>,
}

pub type InvokeStmt = InvokeExpr;

#[derive(Clone, Debug)]
pub struct AssignStmt<B = ()> {
    pub lhs: VarIndex,
    pub rhs: Expr<B>,
}

#[derive(Clone, Debug)]
pub struct RetStmt<B = ()> {
    // TODO(spinda): Record when this statement is at an end of control flow
    // for a body. Do the same for goto statements as well.
    pub value: Option<Expr<B>>,
}

#[derive(Clone, Debug, From)]
pub enum Expr<B = ()> {
    Block(B, Box<BlockExpr<B>>),
    #[from]
    Literal(Literal),
    #[from(types(BuiltInVar, "&BuiltInVar"))]
    Var(VarExpr),
    #[from]
    Invoke(InvokeExpr),
    #[from]
    FieldAccess(Box<FieldAccessExpr<Expr<B>>>),
    #[from]
    Negate(Box<NegateExpr<Expr<B>>>),
    #[from]
    Cast(Box<CastExpr<Expr<B>>>),
    #[from]
    Compare(CompareExpr),
    #[from]
    Arith(ArithExpr),
}

impl<B> Typed for Expr<B> {
    fn type_(&self) -> TypeIndex {
        match self {
            Expr::Block(_, block_expr) => block_expr.type_(),
            Expr::Literal(literal) => literal.type_(),
            Expr::Var(var_expr) => var_expr.type_(),
            Expr::Invoke(invoke_expr) => invoke_expr.type_(),
            Expr::FieldAccess(field_access_expr) => field_access_expr.type_(),
            Expr::Negate(negate_expr) => negate_expr.type_(),
            Expr::Cast(cast_expr) => cast_expr.type_(),
            Expr::Compare(compare_expr) => compare_expr.type_(),
            Expr::Arith(arith_expr) => arith_expr.type_(),
        }
    }
}

impl<B: Default> From<Box<BlockExpr<B>>> for Expr<B> {
    fn from(block_expr: Box<BlockExpr<B>>) -> Expr<B> {
        Expr::Block(B::default(), block_expr)
    }
}

box_from!(BlockExpr<B> => Expr<B> | <B> where B: Default);
box_from!(FieldAccessExpr<Expr<B>> => Expr<B> | <B>);
box_from!(NegateExpr<Expr<B>> => Expr<B> | <B>);
box_from!(CastExpr<Expr<B>> => Expr<B> | <B>);

deref_from!(&Literal => Expr);

impl<B> From<PureExpr> for Expr<B> {
    fn from(pure_expr: PureExpr) -> Self {
        match pure_expr {
            PureExpr::Var(var_expr) => var_expr.into(),
            PureExpr::Literal(literal) => literal.into(),
            PureExpr::FieldAccess(field_access_expr) => (*field_access_expr).into(),
            PureExpr::Negate(negate_expr) => (*negate_expr).into(),
            PureExpr::Cast(cast_expr) => (*cast_expr).into(),
            PureExpr::Compare(compare_expr) => (*compare_expr).into(),
            PureExpr::Arith(arith_expr) => (*arith_expr).into(),
        }
    }
}

impl<B> From<FieldAccessExpr<PureExpr>> for Expr<B> {
    fn from(field_access_expr: FieldAccessExpr<PureExpr>) -> Self {
        Expr::FieldAccess(FieldAccessExpr::<Expr<B>>::from(field_access_expr).into())
    }
}

impl<B> From<NegateExpr<PureExpr>> for Expr<B> {
    fn from(negate_expr: NegateExpr<PureExpr>) -> Self {
        Expr::from(NegateExpr::<Expr<B>>::from(negate_expr))
    }
}

impl<B> From<CastExpr<PureExpr>> for Expr<B> {
    fn from(cast_expr: CastExpr<PureExpr>) -> Self {
        Expr::from(CastExpr::<Expr<B>>::from(cast_expr))
    }
}

#[derive(Clone, Debug, From)]
pub enum PureExpr {
    #[from]
    Literal(Literal),
    #[from(types(BuiltInVar, "&BuiltInVar"))]
    Var(VarExpr),
    #[from]
    FieldAccess(Box<FieldAccessExpr<PureExpr>>),
    #[from]
    Negate(Box<NegateExpr<PureExpr>>),
    #[from]
    Cast(Box<CastExpr<PureExpr>>),
    #[from]
    Compare(Box<CompareExpr>),
    #[from]
    Arith(Box<ArithExpr>),
}

impl Typed for PureExpr {
    fn type_(&self) -> TypeIndex {
        match self {
            PureExpr::Literal(literal) => literal.type_(),
            PureExpr::Var(var_expr) => var_expr.type_(),
            PureExpr::FieldAccess(field_access_expr) => field_access_expr.type_(),
            PureExpr::Negate(negate_expr) => negate_expr.type_(),
            PureExpr::Cast(cast_expr) => cast_expr.type_(),
            PureExpr::Compare(compare_expr) => compare_expr.type_(),
            PureExpr::Arith(arith_expr) => arith_expr.type_(),
        }
    }
}

box_from!(FieldAccessExpr<PureExpr> => PureExpr);
box_from!(NegateExpr<PureExpr> => PureExpr);
box_from!(CastExpr<PureExpr> => PureExpr);
box_from!(CompareExpr => PureExpr);
box_from!(ArithExpr => PureExpr);

deref_from!(&Literal => PureExpr);

impl<B> TryFrom<Expr<B>> for PureExpr {
    type Error = Expr<B>;

    fn try_from(expr: Expr<B>) -> Result<Self, Self::Error> {
        match expr {
            expr @ (Expr::Block(_, _) | Expr::Invoke(_)) => Err(expr),
            Expr::Literal(literal) => Ok(literal.into()),
            Expr::Var(var_expr) => Ok(var_expr.into()),
            Expr::FieldAccess(field_access_expr) => Ok((*field_access_expr).try_into()?),
            Expr::Negate(negate_expr) => Ok((*negate_expr).try_into()?),
            Expr::Cast(cast_expr) => Ok((*cast_expr).try_into()?),
            Expr::Compare(compare_expr) => Ok(compare_expr.into()),
            Expr::Arith(arith_expr) => Ok(arith_expr.into()),
        }
    }
}

impl<B> TryFrom<FieldAccessExpr<Expr<B>>> for PureExpr {
    type Error = FieldAccessExpr<Expr<B>>;

    fn try_from(field_access_expr: FieldAccessExpr<Expr<B>>) -> Result<Self, Self::Error> {
        Ok(PureExpr::from(FieldAccessExpr::<PureExpr>::try_from(
            field_access_expr,
        )?))
    }
}

impl<B> TryFrom<NegateExpr<Expr<B>>> for PureExpr {
    type Error = NegateExpr<Expr<B>>;

    fn try_from(negate_expr: NegateExpr<Expr<B>>) -> Result<Self, Self::Error> {
        Ok(PureExpr::from(NegateExpr::<PureExpr>::try_from(
            negate_expr,
        )?))
    }
}

impl<B> TryFrom<CastExpr<Expr<B>>> for PureExpr {
    type Error = CastExpr<Expr<B>>;

    fn try_from(cast_expr: CastExpr<Expr<B>>) -> Result<Self, Self::Error> {
        Ok(PureExpr::from(CastExpr::<PureExpr>::try_from(cast_expr)?))
    }
}

#[derive(Clone, Debug)]
pub struct BlockExpr<B = ()> {
    pub kind: Option<BlockKind>,
    pub stmts: Vec<Stmt<B>>,
    pub value: Expr<B>,
}

impl<B> Typed for BlockExpr<B> {
    fn type_(&self) -> TypeIndex {
        self.value.type_()
    }
}

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
pub struct NegateExpr<E = Expr> {
    pub kind: NegateKind,
    pub expr: E,
}

impl<E: Typed> Typed for NegateExpr<E> {
    fn type_(&self) -> TypeIndex {
        self.expr.type_()
    }
}

impl<B> From<NegateExpr<PureExpr>> for NegateExpr<Expr<B>> {
    fn from(negate_expr: NegateExpr<PureExpr>) -> Self {
        NegateExpr {
            kind: negate_expr.kind,
            expr: negate_expr.expr.into(),
        }
    }
}

impl<B> TryFrom<NegateExpr<Expr<B>>> for NegateExpr<PureExpr> {
    type Error = NegateExpr<Expr<B>>;

    fn try_from(negate_expr: NegateExpr<Expr<B>>) -> Result<Self, Self::Error> {
        match negate_expr.expr.try_into() {
            Ok(expr) => Ok(NegateExpr {
                kind: negate_expr.kind,
                expr,
            }),
            Err(expr) => Err(NegateExpr {
                kind: negate_expr.kind,
                expr,
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FieldAccessExpr<E = Expr> {
    pub parent: E,
    pub field: FieldIndex,
    pub type_: TypeIndex,
}

impl<E> Typed for FieldAccessExpr<E> {
    fn type_(&self) -> TypeIndex {
        self.type_
    }
}

impl<B> From<FieldAccessExpr<PureExpr>> for FieldAccessExpr<Expr<B>> {
    fn from(field_access_expr: FieldAccessExpr<PureExpr>) -> Self {
        FieldAccessExpr {
            parent: field_access_expr.parent.into(),
            field: field_access_expr.field,
            type_: field_access_expr.type_,
        }
    }
}

impl<B> TryFrom<FieldAccessExpr<Expr<B>>> for FieldAccessExpr<PureExpr> {
    type Error = FieldAccessExpr<Expr<B>>;

    fn try_from(field_access_expr: FieldAccessExpr<Expr<B>>) -> Result<Self, Self::Error> {
        match field_access_expr.parent.try_into() {
            Ok(expr) => Ok(FieldAccessExpr {
                parent: expr,
                field: field_access_expr.field,
                type_: field_access_expr.type_,
            }),
            Err(expr) => Err(FieldAccessExpr {
                parent: expr,
                field: field_access_expr.field,
                type_: field_access_expr.type_,
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CastExpr<E = Expr> {
    pub kind: CastKind,
    pub expr: E,
    pub type_: TypeIndex,
}

impl<E> Typed for CastExpr<E> {
    fn type_(&self) -> TypeIndex {
        self.type_
    }
}

impl<B> From<CastExpr<PureExpr>> for CastExpr<Expr<B>> {
    fn from(cast_expr: CastExpr<PureExpr>) -> Self {
        CastExpr {
            kind: cast_expr.kind,
            expr: cast_expr.expr.into(),
            type_: cast_expr.type_,
        }
    }
}

impl<B> TryFrom<CastExpr<Expr<B>>> for CastExpr<PureExpr> {
    type Error = CastExpr<Expr<B>>;

    fn try_from(cast_expr: CastExpr<Expr<B>>) -> Result<Self, Self::Error> {
        match cast_expr.expr.try_into() {
            Ok(expr) => Ok(CastExpr {
                kind: cast_expr.kind,
                expr,
                type_: cast_expr.type_,
            }),
            Err(expr) => Err(CastExpr {
                kind: cast_expr.kind,
                expr,
                type_: cast_expr.type_,
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompareExpr {
    pub kind: CompareKind,
    pub lhs: PureExpr,
    pub rhs: PureExpr,
}

impl CompareExpr {
    pub const TYPE: BuiltInType = type_checker::CompareExpr::TYPE;
}

impl Typed for CompareExpr {
    fn type_(&self) -> TypeIndex {
        CompareExpr::TYPE.into()
    }
}

#[derive(Clone, Debug)]
pub struct ArithExpr {
    pub kind: ArithKind,
    pub lhs: PureExpr,
    pub rhs: PureExpr,
}

impl Typed for ArithExpr {
    fn type_(&self) -> TypeIndex {
        self.lhs.type_()
    }
}
