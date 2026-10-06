// vim: set tw=99 ts=4 sts=4 sw=4 et:

#![allow(dead_code)]

use std::fmt::{self, Display, Write};
use std::iter::FromIterator;

use derive_more::{Display, From};
use enum_map::Enum;
use enumset::EnumSetType;

use cachet_lang::ast::{
    ArithBinOper, BinOper, BitwiseBinOper, CompareBinOper, Ident, NegateKind,
    NumericCompareBinOper,
};
pub use cachet_lang::normalizer::{LocalLabelIndex, LocalVarIndex};

use cachet_util::{AffixWriter, box_from, chain_from, deref_from, fmt_join, fmt_join_trailing};

#[derive(Clone, Copy, Debug, Display)]
#[display(fmt = "{}_{}", kind, ident)]
pub struct NamespaceIdent {
    pub kind: NamespaceKind,
    pub ident: Ident,
}

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum NamespaceKind {
    #[display(fmt = "Type")]
    Type,
    #[display(fmt = "IR")]
    Ir,
    #[display(fmt = "Impl")]
    Impl,
}

#[derive(Clone, Debug, Display, From)]
pub enum Type {
    #[display(fmt = "void")]
    Void,
    #[from(types(HelperTypeIdent, TypeMemberTypePath, IrMemberTypePath))]
    Path(TypePath),
    #[from]
    Template(Box<TemplateType>),
    #[from]
    Const(Box<ConstType>),
    #[from]
    Ref(Box<RefType>),
}

box_from!(TemplateType => Type);
box_from!(ConstType => Type);
box_from!(RefType => Type);

#[derive(Clone, Copy, Debug, Display, From)]
pub enum TypePath {
    Helper(HelperTypeIdent),
    TypeMember(TypeMemberTypePath),
    IrMember(IrMemberTypePath),
}

impl TypePath {
    pub const fn ident(self) -> TypeIdent {
        match self {
            TypePath::Helper(helper_type_ident) => TypeIdent::Helper(helper_type_ident),
            TypePath::TypeMember(type_member_type_path) => {
                TypeIdent::TypeMember(type_member_type_path.ident)
            }
            TypePath::IrMember(ir_member_type_path) => {
                TypeIdent::IrMember(ir_member_type_path.ident)
            }
        }
    }

    pub const fn parent_namespace(self) -> Option<NamespaceIdent> {
        match self {
            TypePath::Helper(_) => None,
            TypePath::TypeMember(type_member_type_path) => {
                Some(type_member_type_path.parent_namespace())
            }
            TypePath::IrMember(ir_member_type_path) => {
                Some(ir_member_type_path.parent_namespace())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Display, From)]
pub enum TypeIdent {
    Helper(HelperTypeIdent),
    TypeMember(TypeMemberTypeIdent),
    IrMember(IrMemberTypeIdent),
}

impl TypeIdent {
    pub const fn parent_namespace_kind(self) -> Option<NamespaceKind> {
        match self {
            TypeIdent::Helper(_) => None,
            TypeIdent::TypeMember(_) => Some(TypeMemberTypeIdent::PARENT_NAMESPACE_KIND),
            TypeIdent::IrMember(_) => Some(IrMemberTypeIdent::PARENT_NAMESPACE_KIND),
        }
    }
}

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum HelperTypeIdent {
    #[display(fmt = "Cachet_ContextRef")]
    ContextRef,
}

#[derive(Clone, Copy, Debug, Display)]
#[display(fmt = "{}::{}", "self.parent_namespace()", ident)]
pub struct TypeMemberTypePath {
    pub parent: Ident,
    pub ident: TypeMemberTypeIdent,
}

impl TypeMemberTypePath {
    pub const fn parent_namespace(self) -> NamespaceIdent {
        NamespaceIdent {
            kind: TypeMemberTypeIdent::PARENT_NAMESPACE_KIND,
            ident: self.parent,
        }
    }
}

#[derive(Clone, Copy, Debug, Display, Eq, From, Hash, PartialEq)]
pub enum TypeMemberTypeIdent {
    ExprTag(ExprTag),
}

impl TypeMemberTypeIdent {
    pub const PARENT_NAMESPACE_KIND: NamespaceKind = NamespaceKind::Type;
}

#[derive(Clone, Copy, Debug, Display)]
#[display(fmt = "{}::{}", "self.parent_namespace()", ident)]
pub struct IrMemberTypePath {
    pub parent: Ident,
    pub ident: IrMemberTypeIdent,
}

impl IrMemberTypePath {
    pub const fn parent_namespace(self) -> NamespaceIdent {
        NamespaceIdent {
            kind: IrMemberTypeIdent::PARENT_NAMESPACE_KIND,
            ident: self.parent,
        }
    }
}

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum IrMemberTypeIdent {
    #[display(fmt = "LabelLocal")]
    LabelLocal,
    #[display(fmt = "LabelRef")]
    LabelRef,
    #[display(fmt = "LabelMutRef")]
    LabelMutRef,
    #[display(fmt = "OpsRef")]
    OpsRef,
}

impl IrMemberTypeIdent {
    pub const PARENT_NAMESPACE_KIND: NamespaceKind = NamespaceKind::Ir;
}

#[derive(Clone, Debug)]
pub struct TemplateType {
    pub inner: Type,
    pub args: Vec<Type>,
}

impl Display for TemplateType {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}<", self.inner)?;
        fmt_join(f, ", ", self.args.iter())?;
        write!(f, ">")
    }
}

#[derive(Clone, Debug, Display, From)]
#[display(fmt = "{} const", inner)]
pub struct ConstType {
    inner: Type,
}

#[derive(Clone, Debug, Display, From)]
#[display(fmt = "{}{}", inner, value_category)]
pub struct RefType {
    pub inner: Type,
    pub value_category: ValueCategory,
}

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum ValueCategory {
    #[display(fmt = "&")]
    LValue,
    #[display(fmt = "&&")]
    RValue,
}

#[derive(Clone, Copy, Debug, Display, From)]
pub enum VarIdent {
    #[from(types(UserParamVarIdent))]
    Param(ParamVarIdent),
    #[from]
    Local(LocalVarIdent),
    #[from]
    LocalLabel(LocalLabelVarIdent),
    #[from]
    Synthetic(SyntheticVarIdent),
}

#[derive(Clone, Copy, Debug, Display, From)]
pub enum ParamVarIdent {
    #[display(fmt = "cx")]
    Context,
    #[display(fmt = "in")]
    In,
    #[display(fmt = "parent")]
    Parent,
    #[display(fmt = "ops")]
    Ops,
    User(UserParamVarIdent),
}

#[derive(Clone, Copy, Debug, Display, From)]
#[display(fmt = "param_{}", ident)]
pub struct UserParamVarIdent {
    pub ident: Ident,
}

#[derive(Clone, Copy, Debug, Display)]
#[display(fmt = "local_{}_{}", ident, index)]
pub struct LocalVarIdent {
    pub ident: Ident,
    pub index: LocalVarIndex,
}

#[derive(Clone, Copy, Debug, Display)]
#[display(fmt = "label_{}_{}", ident, index)]
pub struct LocalLabelVarIdent {
    pub ident: Ident,
    pub index: LocalLabelIndex,
}

#[derive(Clone, Copy, Debug, Display)]
#[display(fmt = "{}_{}", kind, index)]
pub struct SyntheticVarIdent {
    pub kind: SyntheticVarKind,
    pub index: usize,
}

#[derive(Clone, Copy, Debug, Display, Enum)]
pub enum SyntheticVarKind {
    #[display(fmt = "hoist")]
    Hoist,
}

#[derive(Clone, Copy, Debug, Display, From)]
pub enum FnPath {
    Helper(HelperFnIdent),
    TypeMember(TypeMemberFnPath),
    IrMember(IrMemberFnPath),
    GlobalVar(GlobalVarFnPath),
    User(UserFnPath),
}

impl FnPath {
    pub const fn ident(self) -> FnIdent {
        match self {
            FnPath::Helper(helper_fn_ident) => FnIdent::Helper(helper_fn_ident),
            FnPath::TypeMember(type_member_fn_path) => {
                FnIdent::TypeMember(type_member_fn_path.ident)
            }
            FnPath::IrMember(ir_member_fn_path) => FnIdent::IrMember(ir_member_fn_path.ident),
            FnPath::GlobalVar(global_var_fn_path) => FnIdent::GlobalVar(global_var_fn_path.ident),
            FnPath::User(user_fn_path) => FnIdent::User(user_fn_path.ident),
        }
    }

    pub const fn parent_namespace(self) -> Option<NamespaceIdent> {
        match self {
            FnPath::Helper(_) => None,
            FnPath::TypeMember(type_member_fn_path) => {
                Some(type_member_fn_path.parent_namespace())
            }
            FnPath::IrMember(ir_member_fn_path) => Some(ir_member_fn_path.parent_namespace()),
            FnPath::GlobalVar(global_var_fn_path) => global_var_fn_path.parent_namespace(),
            FnPath::User(user_fn_path) => user_fn_path.parent_namespace(),
        }
    }
}

#[derive(Clone, Copy, Debug, Display, From)]
pub enum FnIdent {
    Helper(HelperFnIdent),
    TypeMember(TypeMemberFnIdent),
    IrMember(IrMemberFnIdent),
    GlobalVar(GlobalVarFnIdent),
    User(UserFnIdent),
}

impl FnIdent {
    pub const fn parent_namespace_kind(self) -> Option<NamespaceKind> {
        match self {
            FnIdent::Helper(_) => None,
            FnIdent::TypeMember(type_member_fn_ident) => {
                Some(type_member_fn_ident.parent_namespace_kind())
            }
            FnIdent::IrMember(ir_member_fn_ident) => {
                Some(ir_member_fn_ident.parent_namespace_kind())
            }
            FnIdent::GlobalVar(_) => Some(GlobalVarFnIdent::PARENT_NAMESPACE_KIND),
            FnIdent::User(_) => Some(UserFnIdent::PARENT_NAMESPACE_KIND),
        }
    }
}

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum HelperFnIdent {
    #[display(fmt = "Cachet_Assert")]
    Assert,
    /// Must be declared `[[noreturn]]` by the embedder: it stands in for the
    /// return a value-returning function would otherwise need.
    #[display(fmt = "Cachet_Unreachable")]
    Unreachable,
}

#[derive(Clone, Copy, Debug, Display, From)]
#[display(fmt = "{}::{}", "self.parent_namespace()", ident)]
pub struct TypeMemberFnPath {
    pub parent: Ident,
    pub ident: TypeMemberFnIdent,
}

impl TypeMemberFnPath {
    pub const fn parent_namespace(self) -> NamespaceIdent {
        NamespaceIdent {
            kind: self.ident.parent_namespace_kind(),
            ident: self.parent,
        }
    }
}

#[derive(Clone, Copy, Debug, Display, From)]
pub enum TypeMemberFnIdent {
    #[display(fmt = "EmptyLocal")]
    EmptyLocal,
    ToTag(ToTagTypeMemberFnIdent),
    #[display(fmt = "SetMutRef")]
    SetMutRef,
    FieldAccess(FieldAccessTypeMemberFnIdent),
    Negate(NegateTypeMemberFnIdent),
    Cast(CastTypeMemberFnIdent),
    Variant(VariantTypeMemberFnIdent),
    BinOper(BinOperTypeMemberFnIdent),
}

impl TypeMemberFnIdent {
    pub const fn parent_namespace_kind(self) -> NamespaceKind {
        match self {
            TypeMemberFnIdent::EmptyLocal | TypeMemberFnIdent::SetMutRef => NamespaceKind::Type,
            TypeMemberFnIdent::FieldAccess(_) => {
                FieldAccessTypeMemberFnIdent::PARENT_NAMESPACE_KIND
            }
            TypeMemberFnIdent::ToTag(_) => ToTagTypeMemberFnIdent::PARENT_NAMESPACE_KIND,
            TypeMemberFnIdent::Negate(_) => NegateTypeMemberFnIdent::PARENT_NAMESPACE_KIND,
            TypeMemberFnIdent::Cast(_) => CastTypeMemberFnIdent::PARENT_NAMESPACE_KIND,
            TypeMemberFnIdent::Variant(_) => VariantTypeMemberFnIdent::PARENT_NAMESPACE_KIND,
            TypeMemberFnIdent::BinOper(_) => BinOperTypeMemberFnIdent::PARENT_NAMESPACE_KIND,
        }
    }
}

#[derive(Clone, Copy, Debug, Display, From)]
#[display(fmt = "To{}", tag)]
pub struct ToTagTypeMemberFnIdent {
    pub tag: ExprTag,
}

impl ToTagTypeMemberFnIdent {
    pub const PARENT_NAMESPACE_KIND: NamespaceKind = NamespaceKind::Type;
}

#[derive(Clone, Copy, Debug, Display, From)]
#[display(fmt = "Field_{}", field)]
pub struct FieldAccessTypeMemberFnIdent {
    pub field: Ident,
}

impl FieldAccessTypeMemberFnIdent {
    pub const PARENT_NAMESPACE_KIND: NamespaceKind = NamespaceKind::Impl;
}

#[derive(Clone, Copy, Debug, From)]
pub struct NegateTypeMemberFnIdent {
    pub kind: NegateKind,
}

impl NegateTypeMemberFnIdent {
    pub const PARENT_NAMESPACE_KIND: NamespaceKind = NamespaceKind::Type;
}

impl Display for NegateTypeMemberFnIdent {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "{}",
            match self.kind {
                NegateKind::Arith => "Negate",
                NegateKind::Bitwise => "BitNot",
                NegateKind::Logical => panic!("logical negate doesn't use member function"),
            }
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CastTypeMemberFnIdent {
    pub target_type_ident: Ident,
    pub tag: ExprTag,
}

impl CastTypeMemberFnIdent {
    pub const PARENT_NAMESPACE_KIND: NamespaceKind = NamespaceKind::Impl;
}

impl Display for CastTypeMemberFnIdent {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "To_{}_{}", self.target_type_ident, self.tag)
    }
}

#[derive(Clone, Copy, Debug, From)]
pub enum BinOperTypeMemberFnIdent {
    #[from]
    Arith(ArithBinOper),
    #[from]
    Bitwise(BitwiseBinOper),
    #[from(types(NumericCompareBinOper))]
    Compare(CompareBinOper),
}

impl BinOperTypeMemberFnIdent {
    pub const PARENT_NAMESPACE_KIND: NamespaceKind = NamespaceKind::Type;
}

impl Display for BinOperTypeMemberFnIdent {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "{}",
            match self {
                BinOperTypeMemberFnIdent::Arith(arith_bin_oper) => match arith_bin_oper {
                    ArithBinOper::Mul => "Mul",
                    ArithBinOper::Div => "Div",
                    ArithBinOper::Mod => "Mod",
                    ArithBinOper::Add => "Add",
                    ArithBinOper::Sub => "Sub",
                },
                BinOperTypeMemberFnIdent::Bitwise(bitwise_bin_oper) => match bitwise_bin_oper {
                    BitwiseBinOper::Shl => "Shl",
                    BitwiseBinOper::Shr => "Shr",
                    BitwiseBinOper::And => "BitAnd",
                    BitwiseBinOper::Xor => "Xor",
                    BitwiseBinOper::Or => "BitOr",
                },
                BinOperTypeMemberFnIdent::Compare(compare_bin_oper) => match compare_bin_oper {
                    CompareBinOper::Eq => "Eq",
                    CompareBinOper::Neq => "Neq",
                    CompareBinOper::Numeric(numeric_compare_bin_oper) =>
                        match numeric_compare_bin_oper {
                            NumericCompareBinOper::Lt => "Lt",
                            NumericCompareBinOper::Gt => "Gt",
                            NumericCompareBinOper::Lte => "Lte",
                            NumericCompareBinOper::Gte => "Gte",
                        },
                },
            }
        )?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Display, From)]
#[display(fmt = "Variant_{}", ident)]
pub struct VariantTypeMemberFnIdent {
    pub ident: Ident,
}

impl VariantTypeMemberFnIdent {
    pub const PARENT_NAMESPACE_KIND: NamespaceKind = NamespaceKind::Impl;
}

#[derive(Clone, Copy, Debug, Display, From)]
#[display(fmt = "{}::{}", "self.parent_namespace()", ident)]
pub struct IrMemberFnPath {
    pub parent: Ident,
    pub ident: IrMemberFnIdent,
}

impl IrMemberFnPath {
    pub const fn parent_namespace(self) -> NamespaceIdent {
        NamespaceIdent {
            kind: self.ident.parent_namespace_kind(),
            ident: self.parent,
        }
    }
}

#[derive(Clone, Copy, Debug, Display, From)]
pub enum IrMemberFnIdent {
    #[display(fmt = "NewLabel")]
    NewLabel,
    #[display(fmt = "ToLabelRef")]
    ToLabelRef,
    #[display(fmt = "ToLabelMutRef")]
    ToLabelMutRef,
    #[display(fmt = "GotoLabel")]
    GotoLabel,
    #[display(fmt = "BindLabel")]
    BindLabel,
    Emit(EmitIrMemberFnIdent),
}

impl IrMemberFnIdent {
    pub const fn parent_namespace_kind(self) -> NamespaceKind {
        match self {
            IrMemberFnIdent::NewLabel
            | IrMemberFnIdent::ToLabelRef
            | IrMemberFnIdent::ToLabelMutRef
            | IrMemberFnIdent::GotoLabel
            | IrMemberFnIdent::BindLabel => NamespaceKind::Ir,
            IrMemberFnIdent::Emit(_) => EmitIrMemberFnIdent::PARENT_NAMESPACE_KIND,
        }
    }
}

#[derive(Clone, Copy, Debug, Display, From)]
#[display(fmt = "Emit{}", ident)]
pub struct EmitIrMemberFnIdent {
    pub ident: OpUserFnIdent,
}

impl EmitIrMemberFnIdent {
    pub const PARENT_NAMESPACE_KIND: NamespaceKind = NamespaceKind::Impl;
}

chain_from!(Ident => OpUserFnIdent => EmitIrMemberFnIdent);

#[derive(Clone, Copy, Debug)]
pub struct GlobalVarFnPath {
    pub parent: Option<Ident>,
    pub ident: GlobalVarFnIdent,
}

impl GlobalVarFnPath {
    pub const fn parent_namespace(self) -> Option<NamespaceIdent> {
        match self.parent {
            Some(parent_ident) => Some(NamespaceIdent {
                kind: GlobalVarFnIdent::PARENT_NAMESPACE_KIND,
                ident: parent_ident,
            }),
            None => None,
        }
    }
}

impl Display for GlobalVarFnPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if let Some(parent_namespace_ident) = self.parent_namespace() {
            write!(f, "{}::", parent_namespace_ident)?;
        }
        write!(f, "{}", self.ident)
    }
}

impl From<GlobalVarFnIdent> for GlobalVarFnPath {
    fn from(global_var_fn_ident: GlobalVarFnIdent) -> Self {
        GlobalVarFnPath {
            parent: None,
            ident: global_var_fn_ident,
        }
    }
}

#[derive(Clone, Copy, Debug, Display, From)]
#[display(fmt = "Var_{}", ident)]
pub struct GlobalVarFnIdent {
    pub ident: Ident,
}

impl GlobalVarFnIdent {
    pub const PARENT_NAMESPACE_KIND: NamespaceKind = NamespaceKind::Impl;
}

#[derive(Clone, Copy, Debug)]
pub struct UserFnPath {
    pub parent: Option<Ident>,
    pub ident: UserFnIdent,
}

impl UserFnPath {
    pub const fn parent_namespace(self) -> Option<NamespaceIdent> {
        match self.parent {
            Some(parent_ident) => Some(NamespaceIdent {
                kind: UserFnIdent::PARENT_NAMESPACE_KIND,
                ident: parent_ident,
            }),
            None => None,
        }
    }
}

impl Display for UserFnPath {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if let Some(parent_namespace_ident) = self.parent_namespace() {
            write!(f, "{}::", parent_namespace_ident)?;
        }
        write!(f, "{}", self.ident)
    }
}

impl From<UserFnIdent> for UserFnPath {
    fn from(user_fn_ident: UserFnIdent) -> Self {
        UserFnPath {
            parent: None,
            ident: user_fn_ident,
        }
    }
}

chain_from!(FnUserFnIdent => UserFnIdent => UserFnPath);

#[derive(Clone, Copy, Debug, Display, From)]
pub enum UserFnIdent {
    Fn(FnUserFnIdent),
    Op(OpUserFnIdent),
}

impl UserFnIdent {
    pub const PARENT_NAMESPACE_KIND: NamespaceKind = NamespaceKind::Impl;
}

#[derive(Clone, Copy, Debug, Display, From)]
#[display(fmt = "Fn_{}", ident)]
pub struct FnUserFnIdent {
    pub ident: Ident,
}

#[derive(Clone, Copy, Debug, Display, From)]
#[display(fmt = "Op_{}", ident)]
pub struct OpUserFnIdent {
    pub ident: Ident,
}

#[derive(Clone, Copy, Debug, Display, From)]
pub enum FlagIdent {
    IrMember(IrMemberFlagIdent),
}

#[derive(Clone, Copy, Debug, Display)]
#[display(fmt = "CACHET_{}_{}", ident, selector)]
pub struct IrMemberFlagIdent {
    pub ident: Ident,
    pub selector: IrMemberFlagSelector,
}

#[derive(Clone, Copy, Debug, Display, Enum)]
pub enum IrMemberFlagSelector {
    #[display(fmt = "EMIT")]
    Emit,
    #[display(fmt = "INTERPRETER")]
    Interpreter,
    #[display(fmt = "COMPILER")]
    Compiler,
}

#[derive(Clone, Debug, Default, From)]
pub struct Code {
    pub items: Vec<Item>,
}

impl Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt_join(f, "\n\n", self.items.iter())
    }
}

impl FromIterator<Item> for Code {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Item>,
    {
        Code {
            items: iter.into_iter().collect(),
        }
    }
}

#[derive(Clone, Debug, Display, From)]
pub enum Item {
    PragmaItem(PragmaItem),
    IfDef(IfDefItem),
    Comment(CommentItem),
    Namespace(NamespaceItem),
    Fn(FnItem),
}

#[derive(Clone, Debug)]
pub struct PragmaItem {
    pub content: String,
}

impl Display for PragmaItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "#pragma {}", self.content)
    }
}

#[derive(Clone, Debug)]
pub struct IfDefItem {
    pub cond: FlagIdent,
    pub then: Code,
}

impl Display for IfDefItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "#ifdef {}\n\n{}\n\n#endif  // {}",
            self.cond, self.then, self.cond
        )?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct CommentItem {
    pub text: String,
}

impl Display for CommentItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(AffixWriter::new(f, "// ", ""), "{}", self.text)
    }
}

#[derive(Clone, Debug)]
pub struct NamespaceItem {
    pub ident: NamespaceIdent,
    pub items: Vec<Item>,
}

impl Display for NamespaceItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "namespace {} {{\n\n", self.ident)?;
        fmt_join_trailing(f, "\n\n", self.items.iter())?;
        write!(f, "}};  // namespace {}", self.ident)
    }
}

#[derive(Clone, Debug)]
pub struct FnItem {
    pub path: FnPath,
    pub is_fully_qualified: bool,
    pub is_inline: bool,
    pub params: Vec<Param>,
    pub ret: Type,
    pub body: Option<Block>,
}

impl Display for FnItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if self.is_inline {
            write!(f, "inline ")?;
        }
        write!(f, "{} ", self.ret)?;

        if self.is_fully_qualified {
            write!(f, "{}", self.path)?;
        } else {
            write!(f, "{}", self.path.ident())?;
        }

        write!(f, "(")?;
        fmt_join(f, ", ", self.params.iter())?;
        write!(f, ")")?;

        match &self.body {
            None => write!(f, ";"),
            Some(body) => write!(f, " {}", body),
        }
    }
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "{} {}", type_, ident)]
pub struct Param {
    pub ident: ParamVarIdent,
    pub type_: Type,
}

#[derive(Clone, Debug, Default, From)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

impl Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{{\n")?;

        let mut indented = AffixWriter::new(f, "  ", "");
        fmt_join_trailing(&mut indented, "\n", self.stmts.iter())?;
        let f = indented.into_inner();

        write!(f, "}}")?;
        Ok(())
    }
}

impl FromIterator<Stmt> for Block {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Stmt>,
    {
        Block {
            stmts: iter.into_iter().collect(),
        }
    }
}

#[derive(Clone, Debug, Display, From)]
pub enum Stmt {
    #[from(types(Block, "Vec<Stmt>"))]
    Block(BlockStmt),
    #[from]
    Let(LetStmt),
    #[from]
    If(IfStmt),
    #[from]
    Ret(RetStmt),
    #[from(types(Expr))]
    Expr(ExprStmt),
}

impl FromIterator<Stmt> for Stmt {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Stmt>,
    {
        Block::from_iter(iter).into()
    }
}

#[derive(Clone, Debug, Default, Display, From)]
#[from(types("Vec<Stmt>"))]
pub struct BlockStmt {
    pub block: Block,
}

impl FromIterator<Stmt> for BlockStmt {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Stmt>,
    {
        Block::from_iter(iter).into()
    }
}

#[derive(Clone, Debug)]
pub struct LetStmt {
    pub lhs: VarIdent,
    pub type_: Type,
    pub rhs: Option<Expr>,
}

impl Display for LetStmt {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{} {}", self.type_, self.lhs)?;
        if let Some(rhs) = &self.rhs {
            write!(f, "({})", rhs)?;
        }
        write!(f, ";")
    }
}

#[derive(Clone, Debug)]
pub struct IfStmt {
    pub cond: Expr,
    pub then: Block,
    pub else_: Option<ElseClause>,
}

impl Display for IfStmt {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "if ({}) {}", self.cond, self.then)?;
        if let Some(else_) = &self.else_ {
            write!(f, "{}", else_)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, From)]
pub enum ElseClause {
    #[from]
    ElseIf(Box<IfStmt>),
    #[from]
    Else(Block),
}

impl Display for ElseClause {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self {
            ElseClause::ElseIf(if_stmt) => {
                write!(f, " else {}", *if_stmt)?;
            }
            ElseClause::Else(block) => {
                write!(f, " else {}", block)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct RetStmt {
    pub value: Option<Expr>,
}

impl Display for RetStmt {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "return")?;
        if let Some(value) = &self.value {
            write!(f, " {}", value)?;
        }
        write!(f, ";")
    }
}

#[derive(Clone, Debug, Display, From)]
#[display(fmt = "{};", expr)]
pub struct ExprStmt {
    pub expr: Expr,
}

#[derive(Clone, Debug, Display, From)]
pub enum Expr {
    #[from(types(Block, "Vec<Stmt>"))]
    Block(BlockExpr),
    #[from]
    Literal(Literal),
    #[from(types(
        ParamVarIdent,
        UserParamVarIdent,
        LocalVarIdent,
        LocalLabelVarIdent,
        SyntheticVarIdent
    ))]
    Var(VarIdent),
    #[from(types(
        HelperFnIdent,
        TypeMemberFnPath,
        IrMemberFnPath,
        GlobalVarFnPath,
        UserFnPath,
    ))]
    Fn(FnPath),
    #[from]
    Template(Box<TemplateExpr>),
    #[from]
    Member(Box<MemberExpr>),
    #[from]
    Arrow(Box<ArrowExpr>),
    #[from]
    Call(Box<CallExpr>),
    #[from]
    Cast(Box<CastExpr>),
    #[from]
    Negate(Box<NegateExpr>),
    #[from]
    Comma(Box<CommaExpr>),
    #[from]
    BinOper(Box<BinOperExpr>),
    #[from]
    Assign(Box<AssignExpr>),
}

box_from!(TemplateExpr => Expr);
box_from!(MemberExpr => Expr);
box_from!(ArrowExpr => Expr);
box_from!(CallExpr => Expr);
box_from!(CastExpr => Expr);
box_from!(NegateExpr => Expr);
box_from!(CommaExpr => Expr);
box_from!(BinOperExpr => Expr);
box_from!(AssignExpr => Expr);

deref_from!(&Literal => Expr);

impl FromIterator<Stmt> for Expr {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Stmt>,
    {
        Block::from_iter(iter).into()
    }
}

struct MaybeGrouped<'a>(&'a Expr);

impl Display for MaybeGrouped<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        let needs_group = match self.0 {
            // Note: BlockExpr and CommaExpr are always inherently grouped for
            // C++ syntax reasons, so there's no need for us to group them again
            // here.
            Expr::Block(_)
            | Expr::Literal(_)
            | Expr::Var(_)
            | Expr::Fn(_)
            | Expr::Template(_)
            | Expr::Member(_)
            | Expr::Arrow(_)
            | Expr::Call(_)
            | Expr::Comma(_) => false,
            Expr::Negate(_) | Expr::BinOper(_) | Expr::Assign(_) => true,
            Expr::Cast(cast_expr) => match cast_expr.kind {
                CastStyle::Functional(_) => false,
                CastStyle::C => true,
            },
        };

        if needs_group {
            write!(f, "({})", self.0)
        } else {
            Display::fmt(self.0, f)
        }
    }
}

#[derive(Debug, Display, EnumSetType, Hash, Ord, PartialOrd)]
pub enum ExprTag {
    #[display(fmt = "MutRef")]
    MutRef,
    #[display(fmt = "Ref")]
    Ref,
    #[display(fmt = "Local")]
    Local,
    #[display(fmt = "Val")]
    Val,
}

#[derive(Clone, Debug, Default, Display, From)]
#[display(fmt = "({})", block)]
#[from(types("Vec<Stmt>"))]
pub struct BlockExpr {
    pub block: Block,
}

impl FromIterator<Stmt> for BlockExpr {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Stmt>,
    {
        Block::from_iter(iter).into()
    }
}

// TODO(spinda): Wrap these in, e.g., `int32_t(<n>)`. Double-check if any
// special namespace handling is required (e.g., should that be
// `::std::int32_t(<n>)`?).
#[derive(Clone, Copy, Debug, Display)]
pub enum Literal {
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    UInt8(u8),
    UInt16(u16),
    UInt32(u32),
    UInt64(u64),
    Double(f64),
}

#[derive(Clone, Debug)]
pub struct TemplateExpr {
    pub inner: Expr,
    pub args: Vec<Type>,
}

impl Display for TemplateExpr {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}<", self.inner)?;
        fmt_join(f, ", ", self.args.iter())?;
        write!(f, ">")
    }
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "{}.{}", "MaybeGrouped(&self.parent)", member)]
pub struct MemberExpr {
    pub parent: Expr,
    pub member: Ident,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "{}->{}", "MaybeGrouped(&self.parent)", member)]
pub struct ArrowExpr {
    pub parent: Expr,
    pub member: Ident,
}

#[derive(Clone, Debug)]
pub struct CallExpr {
    pub target: Expr,
    pub args: Vec<Expr>,
}

impl Display for CallExpr {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}(", MaybeGrouped(&self.target))?;
        fmt_join(f, ", ", self.args.iter())?;
        write!(f, ")")
    }
}

#[derive(Clone, Debug)]
pub struct CastExpr {
    pub kind: CastStyle,
    pub expr: Expr,
    pub type_: Type,
}

impl Display for CastExpr {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self.kind {
            CastStyle::Functional(kind) => write!(f, "{}<{}>({})", kind, self.type_, self.expr),
            CastStyle::C => write!(f, "({}) {}", self.type_, MaybeGrouped(&self.expr)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, From, Hash, PartialEq)]
pub enum CastStyle {
    Functional(FunctionalCastStyle),
    C,
}

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum FunctionalCastStyle {
    #[display(fmt = "static_cast")]
    Static,
    #[display(fmt = "dynamic_cast")]
    Dynamic,
    #[display(fmt = "const_cast")]
    Const,
    #[display(fmt = "reinterpret_cast")]
    Reinterpret,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "{}{}", kind, "MaybeGrouped(&self.expr)")]
pub struct NegateExpr {
    pub kind: NegateKind,
    pub expr: Expr,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "({}, {})", "MaybeGrouped(&self.lhs)", "MaybeGrouped(&self.rhs)")]
pub struct CommaExpr {
    pub lhs: Expr,
    pub rhs: Expr,
}

#[derive(Clone, Debug, Display)]
#[display(
    fmt = "{} {} {}",
    "MaybeGrouped(&self.lhs)",
    oper,
    "MaybeGrouped(&self.rhs)"
)]
pub struct BinOperExpr {
    pub oper: BinOper,
    pub lhs: Expr,
    pub rhs: Expr,
}

#[derive(Clone, Debug, Display)]
#[display(fmt = "{} = {}", "MaybeGrouped(&self.lhs)", "MaybeGrouped(&self.rhs)")]
pub struct AssignExpr {
    pub lhs: Expr,
    pub rhs: Expr,
}
