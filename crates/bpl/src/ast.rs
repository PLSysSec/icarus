// vim: set tw=99 ts=4 sts=4 sw=4 et:

//! Structured after the
//! [official Boogie grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar).
//!
//! [BoogieProgram] is the top-level entry point.

mod ident;
mod span;

use std::borrow::{Borrow, BorrowMut};
use std::fmt::{self, Debug, Display, Write};
use std::iter::{self, FromIterator, IntoIterator};
use std::ops::{Deref, DerefMut};

use derive_more::{Display, From};
use indent_write::fmt::IndentWriter;
use joinery::JoinableIterator;

pub use crate::ast::ident::Ident;
pub use crate::ast::span::{ByteIndex, RawIndex, Span, Spanned};

// * Helper Macros

macro_rules! box_from {
    (
        $src:ty => $dst:ty
    ) => {
        impl From<$src> for $dst {
            fn from(src: $src) -> Self {
                From::from(Box::new(src))
            }
        }
    };
}

macro_rules! chain_from {
    (
        $src:ty => $mid:ty => $dst:ty
    ) => {
        impl From<$src> for $dst {
            fn from(src: $src) -> Self {
                From::from(Into::<$mid>::into(src))
            }
        }
    };
}

macro_rules! wrapper_impls {
    (
        $wrapper:ty:$field:ident => $wrapped:ty
    ) => {
        impl Debug for $wrapper {
            fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
                Debug::fmt(&self.$field, f)
            }
        }

        impl AsRef<$wrapped> for $wrapper {
            fn as_ref(&self) -> &$wrapped {
                &*self
            }
        }

        impl AsMut<$wrapped> for $wrapper {
            fn as_mut(&mut self) -> &mut $wrapped {
                &mut *self
            }
        }

        impl Borrow<$wrapped> for $wrapper {
            fn borrow(&self) -> &$wrapped {
                &*self
            }
        }

        impl BorrowMut<$wrapped> for $wrapper {
            fn borrow_mut(&mut self) -> &mut $wrapped {
                &mut *self
            }
        }

        impl Deref for $wrapper {
            type Target = $wrapped;

            fn deref(&self) -> &Self::Target {
                &self.$field
            }
        }

        impl DerefMut for $wrapper {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.$field
            }
        }

        impl From<$wrapped> for $wrapper {
            fn from(items: $wrapped) -> Self {
                Self { $field: items }
            }
        }
    };
}

macro_rules! collection_wrapper_impls {
    (
        $collection:ty:$field:ident => $item:ty | $wrapped:ty
    ) => {
        wrapper_impls!($collection:$field => $wrapped);

        impl From<$item> for $collection {
            fn from(item: $item) -> Self {
                iter::once(item).collect()
            }
        }

        impl FromIterator<$item> for $collection {
            fn from_iter<T>(iter: T) -> Self
            where
                T: IntoIterator<Item = $item>,
            {
                iter.into_iter().collect::<$wrapped>().into()
            }
        }

        impl<'a> IntoIterator for &'a $collection {
            type Item = &'a $item;
            type IntoIter = <&'a $wrapped as IntoIterator>::IntoIter;

            fn into_iter(self) -> Self::IntoIter {
                (&self.$field).into_iter()
            }
        }

        impl<'a> IntoIterator for &'a mut $collection {
            type Item = &'a mut $item;
            type IntoIter = <&'a mut $wrapped as IntoIterator>::IntoIter;

            fn into_iter(self) -> Self::IntoIter {
                (&mut self.$field).into_iter()
            }
        }

        impl IntoIterator for $collection {
            type Item = $item;
            type IntoIter = <$wrapped as IntoIterator>::IntoIter;

            fn into_iter(self) -> Self::IntoIter {
                self.$field.into_iter()
            }
        }
    };
    (
        $collection:ty:$field:ident => $item:ty
    ) => {
        collection_wrapper_impls!($collection:$field => $item | Vec<$item>);
    };
}

macro_rules! from_expr_impls {
    (
        $dst:tt
    ) => {
        chain_from!(Box<EquivExpr> => Expr => $dst);
        chain_from!(EquivExpr => Expr => $dst);
        chain_from!(Box<ImpliesExpr> => Expr => $dst);
        chain_from!(ImpliesExpr => Expr => $dst);
        chain_from!(Box<ExpliesExpr> => Expr => $dst);
        chain_from!(ExpliesExpr => Expr => $dst);
        chain_from!(Box<LogicalExpr> => Expr => $dst);
        chain_from!(LogicalExpr => Expr => $dst);
        chain_from!(Box<RelExpr> => Expr => $dst);
        chain_from!(RelExpr => Expr => $dst);
        chain_from!(Box<BvTerm> => Expr => $dst);
        chain_from!(BvTerm => Expr => $dst);
        chain_from!(Box<Term> => Expr => $dst);
        chain_from!(Term => Expr => $dst);
        chain_from!(Box<Factor> => Expr => $dst);
        chain_from!(Factor => Expr => $dst);
        chain_from!(Box<Power> => Expr => $dst);
        chain_from!(Power => Expr => $dst);
        chain_from!(Box<NegExpr> => Expr => $dst);
        chain_from!(NegExpr => Expr => $dst);
        chain_from!(Box<CoercionExpr> => Expr => $dst);
        chain_from!(CoercionExpr => Expr => $dst);
        chain_from!(Box<ArrayExpr> => Expr => $dst);
        chain_from!(ArrayExpr => Expr => $dst);
        chain_from!(bool => Expr => $dst);
        chain_from!(Dec => Expr => $dst);
        chain_from!(Float => Expr => $dst);
        chain_from!(BvLit => Expr => $dst);
        chain_from!(FuncCall => Expr => $dst);
        chain_from!(Box<OldExpr> => Expr => $dst);
        chain_from!(OldExpr => Expr => $dst);
        chain_from!(Box<ArithCoercionExpr> => Expr => $dst);
        chain_from!(ArithCoercionExpr => Expr => $dst);
        chain_from!(Box<QuantExpr> => Expr => $dst);
        chain_from!(QuantExpr => Expr => $dst);
        chain_from!(Box<IfThenElseExpr> => Expr => $dst);
        chain_from!(IfThenElseExpr => Expr => $dst);
        chain_from!(Box<CodeExpr> => Expr => $dst);
        chain_from!(CodeExpr => Expr => $dst);
    };
}

// * Boogie AST and Printing Instances

const INDENT: &str = "  ";

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-boogie_program):
///
/// ```grammar
/// boogie_program ::= { axiom_decl | const_decl | func_decl | impl_decl | proc_decl | type_decl | var_decl }
/// ```
#[derive(Clone, Default)]
pub struct BoogieProgram {
    pub decls: Vec<Spanned<Decl>>,
}

impl Display for BoogieProgram {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.decls.iter().join_with("\n\n"))
    }
}

collection_wrapper_impls!(BoogieProgram:decls => Spanned<Decl>);

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-boogie_program):
///
/// ```grammar
/// boogie_program ::= { axiom_decl | const_decl | func_decl | impl_decl | proc_decl | type_decl | var_decl }
/// ```
#[derive(Clone, Debug, Display, From)]
pub enum Decl {
    Axiom(AxiomDecl),
    Const(ConstDecl),
    Func(FuncDecl),
    Impl(ImplDecl),
    Proc(ProcDecl),
    Type(TypeDecls),
    Var(VarDecl),
}

chain_from!(Spanned<TypeDecl> => TypeDecls => Decl);
chain_from!(Vec<Spanned<TypeDecl>> => TypeDecls => Decl);

impl FromIterator<Spanned<TypeDecl>> for Decl {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Spanned<TypeDecl>>,
    {
        TypeDecls::from_iter(iter).into()
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-axiom_decl):
///
/// ```grammar
/// axiom_decl ::= "axiom" { attr } proposition ";"
/// ```
#[derive(Clone, Debug)]
pub struct AxiomDecl {
    pub attrs: Vec<Attr>,
    pub proposition: Proposition,
}

impl Display for AxiomDecl {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "axiom ")?;
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        write!(f, "{};", self.proposition)
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-const_decl):
///
/// ```grammar
/// const_decl ::= "const" { attr } [ "unique" ] typed_idents [ order_spec ] ";"
/// ```
#[derive(Clone, Debug)]
pub struct ConstDecl {
    pub attrs: Vec<Attr>,
    pub is_unique: bool,
    pub consts: TypedIdents,
    pub order_spec: Option<OrderSpec>,
}

impl Display for ConstDecl {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "const ")?;
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        if self.is_unique {
            write!(f, "unique ")?;
        }
        write!(f, "{}", self.consts)?;
        if let Some(order_spec) = &self.order_spec {
            write!(f, " {order_spec}")?;
        }
        write!(f, ";")
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-func_decl):
///
/// ```grammar
/// func_decl ::= "function" { attr } ident [ type_params ] "(" [ var_or_type { "," var_or_type } ] ")" ( "returns" "(" var_or_type ")" | ":" type ) ( "{" expr "}" | ";" )
/// ```
#[derive(Clone, Debug)]
pub struct FuncDecl {
    pub attrs: Vec<Attr>,
    pub ident: Ident,
    pub type_params: TypeParams,
    pub var_params: Vec<VarOrType>,
    pub returns: VarOrType,
    pub body: Option<Expr>,
}

impl Display for FuncDecl {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "function ")?;
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        write!(
            f,
            "{}{}({})",
            self.ident,
            self.type_params,
            self.var_params.iter().join_with(", ")
        )?;
        if self.returns.attrs.is_empty() && self.returns.var.is_none() {
            write!(f, ": {}", self.returns.type_)
        } else {
            write!(f, " returns ({})", self.returns)
        }?;
        match &self.body {
            Some(body) => {
                write!(f, " {{\n")?;

                let mut indented = IndentWriter::new(INDENT, f);
                write!(indented, "{body}")?;
                let f = indented.into_inner();

                write!(f, "\n}}")?;
            }
            None => write!(f, ";")?,
        }
        Ok(())
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-impl_decl):
///
/// ```grammar
/// impl_decl ::= "implementation" proc_sign impl_body
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "implementation {proc_sign} {impl_body}")]
pub struct ImplDecl {
    pub proc_sign: ProcSign,
    pub impl_body: ImplBody,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-proc_decl):
///
/// ```grammar
/// proc_decl ::= "procedure" proc_sign ( ";" { spec } | { spec } impl_body )
/// ```
#[derive(Clone, Debug)]
pub struct ProcDecl {
    pub proc_sign: ProcSign,
    pub specs: Vec<Spec>,
    pub impl_body: Option<ImplBody>,
}

impl Display for ProcDecl {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "procedure {}", self.proc_sign)?;
        if self.impl_body.is_none() {
            write!(f, ";")?;
        }

        let mut indented = IndentWriter::new(INDENT, f);
        for spec in &self.specs {
            write!(indented, "\n{spec}")?;
        }
        let f = indented.into_inner();

        if let Some(impl_body) = &self.impl_body {
            write!(
                f,
                "{}{impl_body}",
                if self.specs.is_empty() { ' ' } else { '\n' }
            )?;
        }
        Ok(())
    }
}

/// Multiple type declarations can be combined with commas into a single
/// top-level declaration.
///
/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-type_decl):
///
/// ```grammar
/// type_decl ::= "type" { attr } ident { ident } [ "=" type ] { "," ident { ident } [ "=" type ] } ";"
/// ```
#[derive(Clone, Debug)]
pub struct TypeDecls {
    pub attrs: Vec<Attr>,
    pub decls: Vec<Spanned<TypeDecl>>,
}

impl Display for TypeDecls {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "type ")?;
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }

        if self.decls.len() == 1 {
            write!(f, "{};", self.decls[0])
        } else {
            let mut indented = IndentWriter::new_skip_initial(INDENT, f);
            write!(indented, "{};", self.decls.iter().join_with(",\n"))
        }
    }
}

impl From<Spanned<TypeDecl>> for TypeDecls {
    fn from(type_decl: Spanned<TypeDecl>) -> Self {
        iter::once(type_decl).collect()
    }
}

impl From<Vec<Spanned<TypeDecl>>> for TypeDecls {
    fn from(type_decls: Vec<Spanned<TypeDecl>>) -> Self {
        TypeDecls {
            attrs: Vec::new(),
            decls: type_decls,
        }
    }
}

impl FromIterator<Spanned<TypeDecl>> for TypeDecls {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Spanned<TypeDecl>>,
    {
        TypeDecls {
            attrs: Vec::new(),
            decls: iter.into_iter().collect(),
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-type_decl):
///
/// ```grammar
/// type_decl ::= "type" { attr } ident { ident } [ "=" type ] { "," ident { ident } [ "=" type ] } ";"
/// ```
#[derive(Clone, Debug)]
pub struct TypeDecl {
    pub ident: Ident,
    pub type_params: Vec<Ident>,
    pub type_: Option<Type>,
}

impl Display for TypeDecl {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.ident)?;
        for type_param in &self.type_params {
            write!(f, " {type_param}")?;
        }
        if let Some(type_) = &self.type_ {
            write!(f, " = {type_}")?;
        }
        Ok(())
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-var_decl):
///
/// ```grammar
/// var_decl ::= "var" { attr } typed_idents_wheres ";"
/// ```
#[derive(Clone, Debug)]
pub struct VarDecl {
    pub attrs: Vec<Attr>,
    pub vars: TypedIdentsWheres,
}

impl Display for VarDecl {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "var ")?;
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        write!(f, "{};", self.vars)
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-order_spec):
///
/// ```grammar
/// order_spec ::= "extends" [ [ "unique" ] ident { "," [ "unique" ] ident } ] [ "complete" ]
/// ```
#[derive(Clone, Debug)]
pub struct OrderSpec {
    pub parents: Vec<OrderSpecParent>,
    pub is_complete: bool,
}

impl Display for OrderSpec {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "extends")?;
        if !self.parents.is_empty() {
            write!(f, " {}", self.parents.iter().join_with(", "))?;
        }
        if self.is_complete {
            write!(f, " complete")?;
        }
        Ok(())
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-order_spec):
///
/// ```grammar
/// order_spec ::= "extends" [ [ "unique" ] ident { "," [ "unique" ] ident } ] [ "complete" ]
/// ```
#[derive(Clone, Debug)]
pub struct OrderSpecParent {
    pub parent: Ident,
    pub is_unique: bool,
}

impl Display for OrderSpecParent {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if self.is_unique {
            write!(f, "unique ")?;
        }
        write!(f, "{}", self.parent)
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-var_or_type):
///
/// ```grammar
/// var_or_type ::= { attr } ( type | ident [ ":" type ] )
/// ```
///
/// Note that the official grammar as stated is ambiguous. "foo" could be parsed
/// as either a type or a variable identifier in the above definition. In truth,
/// the `":" type` signature is not actually optional if the identifier is to be
/// interpreted as a variable.
#[derive(Clone, Debug)]
pub struct VarOrType {
    pub attrs: Vec<Attr>,
    pub var: Option<Ident>,
    pub type_: Type,
}

impl Display for VarOrType {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        if let Some(var) = &self.var {
            write!(f, "{var}: ")?;
        }
        write!(f, "{}", self.type_)
    }
}

impl From<Type> for VarOrType {
    fn from(type_: Type) -> Self {
        Self {
            attrs: Vec::new(),
            var: None,
            type_,
        }
    }
}

impl From<TypeAtom> for VarOrType {
    fn from(type_atom: TypeAtom) -> Self {
        Type::from(type_atom).into()
    }
}

impl From<TypeApp> for VarOrType {
    fn from(type_app: TypeApp) -> Self {
        Type::from(type_app).into()
    }
}

impl From<Ident> for VarOrType {
    fn from(ident: Ident) -> Self {
        Type::from(ident).into()
    }
}

impl From<MapType> for VarOrType {
    fn from(map_type: MapType) -> Self {
        Type::from(map_type).into()
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-proc_sign):
///
/// ```grammar
/// proc_sign ::= { attr } ident [ type_params ] "(" [ attr_typed_idents_wheres ] ")" [ "returns" "(" [ attr_typed_idents_wheres ] ")" ]
/// ```
#[derive(Clone, Debug)]
pub struct ProcSign {
    pub attrs: Vec<Spanned<Attr>>,
    pub ident: Spanned<Ident>,
    pub type_params: TypeParams,
    pub var_params: AttrTypedIdentsWheres,
    pub returns: AttrTypedIdentsWheres,
}

impl Display for ProcSign {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        write!(f, "{}{}({})", self.ident, self.type_params, self.var_params)?;
        if !self.returns.is_empty() {
            write!(f, " returns ({})", self.returns)?;
        }
        Ok(())
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-impl_body):
///
/// ```grammar
/// impl_body ::= "{" { local_vars } stmt_list "}"
/// ```
#[derive(Clone, Debug)]
pub struct ImplBody {
    pub local_vars: Vec<LocalVars>,
    pub stmt_list: StmtList,
}

impl Display for ImplBody {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{{\n")?;

        let mut indented = IndentWriter::new(INDENT, f);
        write!(indented, "{}", self.local_vars.iter().join_with("\n"))?;
        if !self.local_vars.is_empty() && !self.stmt_list.is_empty() {
            write!(indented, "\n\n")?;
        }
        write!(indented, "{}", self.stmt_list)?;
        let f = indented.into_inner();

        if !self.stmt_list.is_empty() {
            write!(f, "\n")?;
        }
        write!(f, "}}")
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-stmt_list):
///
/// ```grammar
/// stmt_list ::= { ( label_or_cmd | transfer_cmd | structured_cmd ) }
/// ```
#[derive(Clone, Default)]
pub struct StmtList {
    pub stmts: Vec<Stmt>,
}

impl Display for StmtList {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.stmts.iter().join_with("\n"))
    }
}

collection_wrapper_impls!(StmtList:stmts => Stmt);

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-stmt_list):
///
/// ```grammar
/// stmt_list ::= { ( label_or_cmd | transfer_cmd | structured_cmd ) }
/// ```
#[derive(Clone, Debug, Display, From)]
pub enum Stmt {
    #[from(types(AssignCmd, CallCmd, ClaimCmd, HavocCmd, Label, ParCallCmd, YieldCmd))]
    LabelOrCmd(LabelOrCmd),
    #[from(types(GotoCmd, ReturnCmd))]
    TransferCmd(TransferCmd),
    #[from(types(BreakCmd, IfCmd, WhileCmd))]
    StructuredCmd(StructuredCmd),
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-local_vars):
///
/// ```grammar
/// local_vars ::= "var" { attr } typed_idents_wheres ";"
/// ```
#[derive(Clone, Debug)]
pub struct LocalVars {
    pub attrs: Vec<Attr>,
    pub vars: TypedIdentsWheres,
}

impl Display for LocalVars {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "var ")?;
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        write!(f, "{};", self.vars)
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-spec):
///
/// ```grammar
/// spec ::= ( modifies_spec | requires_spec | ensures_spec )
/// ```
#[derive(Clone, Debug, Display, From)]
pub enum Spec {
    Modifies(ModifiesSpec),
    Contract(ContractSpec),
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-modifies_spec):
///
/// ```grammar
/// modifies_spec ::= "modifies" [ idents ] ";"
/// ```
#[derive(Clone, Debug)]
pub struct ModifiesSpec {
    pub vars: Idents,
}

impl Display for ModifiesSpec {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "modifies")?;
        if !self.vars.is_empty() {
            write!(f, " {}", self.vars)?;
        }
        write!(f, ";")
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-requires_spec):
///
/// ```grammar
/// requires_spec ::= [ "free" ] "requires" { attr } proposition ";"
/// ensures_spec  ::= [ "free" ] "ensures" { attr } proposition ";"
/// ```
#[derive(Clone, Debug)]
pub struct ContractSpec {
    pub kind: ContractKind,
    pub attrs: Vec<Attr>,
    pub proposition: Proposition,
    pub is_free: bool,
}

impl Display for ContractSpec {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if self.is_free {
            write!(f, "free ")?;
        }
        write!(f, "{} ", self.kind)?;
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        write!(f, "{};", self.proposition)
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-requires_spec):
///
/// ```grammar
/// requires_spec ::= [ "free" ] "requires" { attr } proposition ";"
/// ensures_spec  ::= [ "free" ] "ensures" { attr } proposition ";"
/// ```
#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum ContractKind {
    #[display(fmt = "requires")]
    Requires,
    #[display(fmt = "ensures")]
    Ensures,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-label_or_cmd):
///
/// ```grammar
/// label_or_cmd ::= ( assert_cmd | assign_cmd | assume_cmd | call_cmd | havoc_cmd | label | par_call_cmd | yield_cmd )
/// ```
#[derive(Clone, Debug, Display, From)]
pub enum LabelOrCmd {
    Assign(AssignCmd),
    Call(CallCmd),
    Claim(ClaimCmd),
    Havoc(HavocCmd),
    Label(Label),
    ParCall(ParCallCmd),
    Yield(YieldCmd),
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-transfer_cmd):
///
/// ```grammar
/// transfer_cmd ::= ( goto_cmd | return_cmd )
/// ```
#[derive(Clone, Debug, Display, From)]
pub enum TransferCmd {
    Goto(GotoCmd),
    Return(ReturnCmd),
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-structured_cmd):
///
/// ```grammar
/// structured_cmd ::= ( break_cmd | if_cmd | while_cmd)
/// ```
#[derive(Clone, Debug, Display, From)]
pub enum StructuredCmd {
    Break(BreakCmd),
    If(IfCmd),
    While(WhileCmd),
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-assign_cmd):
///
/// ```grammar
/// assign_cmd ::= ident { "[" [ exprs ] "]" } { "," ident { "[" [ exprs ] "]" } } ":=" exprs ";"
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{} := {rhs};", "lhs.iter().join_with(\", \")")]
pub struct AssignCmd {
    pub lhs: Vec<AssignLhs>,
    pub rhs: Exprs,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-assign_cmd):
///
/// ```grammar
/// assign_cmd ::= ident { "[" [ exprs ] "]" } { "," ident { "[" [ exprs ] "]" } } ":=" exprs ";"
/// ```
#[derive(Clone, Debug)]
pub struct AssignLhs {
    pub ident: Ident,
    pub subscripts: Vec<Exprs>,
}

impl Display for AssignLhs {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.ident)?;
        for subscript in &self.subscripts {
            write!(f, "[{subscript}]")?;
        }
        Ok(())
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-break_cmd):
///
/// ```grammar
/// break_cmd ::= "break" [ ident ] ";"
/// ```
#[derive(Clone, Debug)]
pub struct BreakCmd {
    pub label: Option<Ident>,
}

impl Display for BreakCmd {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "break")?;
        if let Some(label) = &self.label {
            write!(f, " {label}")?;
        }
        write!(f, ";")
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-call_cmd):
///
/// ```grammar
/// call_cmd ::= [ "async" ] [ "free" ] "call" { attr } call_params ";"
/// ```
#[derive(Clone, Debug)]
pub struct CallCmd {
    pub attrs: Vec<Attr>,
    pub call_params: CallParams,
    pub is_async: bool,
    pub is_free: bool,
}

impl Display for CallCmd {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if self.is_async {
            write!(f, "async ")?;
        }
        if self.is_free {
            write!(f, "free ")?;
        }
        write!(f, "call ")?;
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        write!(f, "{};", self.call_params)
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-assert_cmd):
///
/// ```grammar
/// assert_cmd ::= "assert" { attr } proposition ";"
/// assume_cmd ::= "assume" { attr } proposition ";"
/// ```
#[derive(Clone, Debug)]
pub struct ClaimCmd {
    pub kind: ClaimKind,
    pub attrs: Vec<Attr>,
    pub proposition: Proposition,
}

impl Display for ClaimCmd {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{} ", self.kind)?;
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        write!(f, "{};", self.proposition)
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-assert_cmd):
///
/// ```grammar
/// assert_cmd ::= "assert" { attr } proposition ";"
/// assume_cmd ::= "assume" { attr } proposition ";"
/// ```
#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum ClaimKind {
    #[display(fmt = "assert")]
    Assert,
    #[display(fmt = "assume")]
    Assume,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-goto_cmd):
///
/// ```grammar
/// goto_cmd ::= "goto" idents ";"
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "goto {labels};")]
pub struct GotoCmd {
    pub labels: Idents,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-havoc_cmd):
///
/// ```grammar
/// havoc_cmd ::= "havoc" idents ";"
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "havoc {vars};")]
pub struct HavocCmd {
    pub vars: Idents,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-if_cmd):
///
/// ```grammar
/// if_cmd ::= "if" guard "{" [ "else" ( if_cmd | "{" stmt_list "}" ) ]
/// ```
///
/// Note that else-clauses are incorrectly defined in the official grammar, as
/// reproduced above.
#[derive(Clone, Debug)]
pub struct IfCmd {
    pub guard: Guard,
    pub then: StmtList,
    pub else_: Option<ElseClause>,
}

impl Display for IfCmd {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "if {} {{\n", self.guard)?;

        let mut indented = IndentWriter::new(INDENT, f);
        write!(indented, "{}", self.then)?;
        let f = indented.into_inner();

        if !self.then.is_empty() {
            write!(f, "\n")?;
        }
        write!(f, "}}")?;
        if let Some(else_) = &self.else_ {
            write!(f, " {else_}")?;
        }
        Ok(())
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-if_cmd):
///
/// ```grammar
/// if_cmd ::= "if" guard "{" [ "else" ( if_cmd | "{" stmt_list "}" ) ]
/// ```
///
/// Note that else-clauses are incorrectly defined in the official grammar, as
/// reproduced above.
#[derive(Clone, Debug, From)]
pub enum ElseClause {
    ElseIf(Box<IfCmd>),
    Else(StmtList),
}

impl Display for ElseClause {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "else ")?;
        match self {
            ElseClause::ElseIf(if_cmd) => write!(f, "{if_cmd}"),
            ElseClause::Else(stmt_list) => {
                write!(f, "{{\n")?;

                let mut indented = IndentWriter::new(INDENT, f);
                write!(indented, "{stmt_list}")?;
                let f = indented.into_inner();

                if !stmt_list.is_empty() {
                    write!(f, "\n")?;
                }
                write!(f, "}}")
            }
        }
    }
}

box_from!(IfCmd => ElseClause);

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-label_cmd):
///
/// ```grammar
/// label ::= ident ":"
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{ident}:")]
pub struct Label {
    pub ident: Ident,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-par_call_cmd):
///
/// ```grammar
/// par_call_cmd ::= "par" { attr } call_params { "|" call_params } ";"
/// ```
#[derive(Clone, Debug)]
pub struct ParCallCmd {
    pub attrs: Vec<Attr>,
    pub calls: Vec<CallParams>,
}

impl Display for ParCallCmd {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "par ")?;
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        write!(f, "{};", self.calls.iter().join_with(" | "))
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-return_cmd):
///
/// ```grammar
/// return_cmd ::= "return" ";"
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "return;")]
pub struct ReturnCmd;

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-while_cmd):
///
/// ```grammar
/// while_cmd ::= "while" guard { [ "free" ] "invariant" { attr } expr ";" } "{" stmt_list "}"
/// ```
#[derive(Clone, Debug)]
pub struct WhileCmd {
    pub guard: Guard,
    pub invariants: Vec<Invariant>,
    pub body: StmtList,
}

impl Display for WhileCmd {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "while {}", self.guard)?;
        let f = if self.invariants.is_empty() {
            write!(f, " ")?;
            f
        } else {
            let mut indented = IndentWriter::new(INDENT, f);
            for invariant in &self.invariants {
                write!(indented, "\n{invariant}")?;
            }
            write!(indented, "\n")?;
            indented.into_inner()
        };
        write!(f, "{{\n")?;

        let mut indented = IndentWriter::new(INDENT, f);
        write!(indented, "{}", self.body)?;
        let f = indented.into_inner();

        if !self.body.is_empty() {
            write!(f, "\n")?;
        }
        write!(f, "}}")
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-while_cmd):
///
/// ```grammar
/// while_cmd ::= "while" guard { [ "free" ] "invariant" { attr } expr ";" } "{" stmt_list "}"
/// ```
#[derive(Clone, Debug)]
pub struct Invariant {
    pub attrs: Vec<Attr>,
    pub expr: Expr,
    pub is_free: bool,
}

impl Display for Invariant {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if self.is_free {
            write!(f, "free ")?;
        }
        write!(f, "invariant ")?;
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        write!(f, "{};", self.expr)
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-yield_cmd):
///
/// ```grammar
/// yield_cmd ::= "yield" ";"
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "yield;")]
pub struct YieldCmd;

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-call_params):
///
/// ```grammar
/// call_params ::= ident ( "(" [ exprs ] ")" | [ "," idents ] ":=" ident [ exprs ] ")" )
/// ```
///
/// Note that the argument list is incorrectly defined in the official grammar,
/// as reproduced above.
#[derive(Clone, Debug)]
pub struct CallParams {
    pub returns: Idents,
    pub target: Ident,
    pub params: Exprs,
}

impl Display for CallParams {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if !self.returns.is_empty() {
            write!(f, "{} := ", self.returns)?;
        }
        write!(f, "{}({})", self.target, self.params)
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-guard):
///
/// ```grammar
/// guard ::= "(" ( "*" | expr ) ")"
/// ```
#[derive(Clone, Debug, Display, From)]
#[display(fmt = "({})")]
pub enum Guard {
    /// I would give this a better name, if I knew what it meant.
    #[display(fmt = "*")]
    Asterisk,
    Expr(Expr),
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-type):
///
/// ```grammar
/// type ::= ( type_atom | ident [ type_args ] | map_type )
/// ```
#[derive(Clone, Debug, Display, From)]
pub enum Type {
    #[from]
    Atom(TypeAtom),
    #[from(types(Ident))]
    App(TypeApp),
    #[from]
    Map(MapType),
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-type):
///
/// ```grammar
/// type      ::= ( type_atom | ident [ type_args ] | map_type )
/// type_args ::= ( type_atom [ type_args ] | ident [ type_args ] | map_type )
/// ```
#[derive(Clone, Debug)]
pub struct TypeApp<T = Ident> {
    pub head: T,
    pub tail: Option<Box<TypeArgs>>,
}

impl<T: Display> Display for TypeApp<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.head)?;
        if let Some(tail) = &self.tail {
            write!(f, " {tail}")?;
        }
        Ok(())
    }
}

impl<T> From<T> for TypeApp<T> {
    fn from(head: T) -> Self {
        Self { head, tail: None }
    }
}

chain_from!(Type => TypeAtom => TypeApp<TypeAtom>);

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-type_args):
///
/// ```grammar
/// type_args ::= ( type_atom [ type_args ] | ident [ type_args ] | map_type )
/// ```
#[derive(Clone, Debug, Display, From)]
pub enum TypeArgs {
    #[from(types(Type, TypeAtom))]
    AtomApp(TypeApp<TypeAtom>),
    #[from(types(Ident))]
    App(TypeApp),
    #[from]
    Map(MapType),
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-type_atom):
///
/// ```grammar
/// type_atom ::= ( "int" | "real" | "bool" | "(" type ")" )
/// ```
#[derive(Clone, Debug, Display, From)]
pub enum TypeAtom {
    #[display(fmt = "int")]
    Int,
    #[display(fmt = "real")]
    Real,
    #[display(fmt = "bool")]
    Bool,
    #[display(fmt = "({})", _0)]
    Paren(Box<Type>),
}

box_from!(Type => TypeAtom);

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-map_type):
///
/// ```grammar
/// map_type ::= [ type_params ] "[" [ type { "," type } ] "]" type
/// ```
#[derive(Clone, Debug)]
pub struct MapType {
    pub type_params: TypeParams,
    pub keys: Vec<Type>,
    pub value: Box<Type>,
}

impl Display for MapType {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "{}[{}]{}",
            self.type_params,
            self.keys.iter().join_with(", "),
            self.value
        )
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-exprs):
///
/// ```grammar
/// exprs ::= expr { "," expr }
/// ```
#[derive(Clone, Default, Display)]
#[display(fmt = "{}", "self.iter().join_with(\", \")")]
pub struct Exprs {
    pub exprs: Vec<Expr>,
}

collection_wrapper_impls!(Exprs:exprs => Expr);
from_expr_impls!(Exprs);

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-proposition):
///
/// ```grammar
/// proposition ::= expr
/// ```
pub type Proposition = Expr;

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-expr):
///
/// ```grammar
/// expr          ::= implies_expr { equiv_op implies_expr }
/// implies_expr  ::= logical_expr [ implies_op implies_expr | explies_op logical_expr { explies_op logical_expr } ]
/// logical_expr  ::= rel_expr [ and_op rel_expr { and_op rel_expr } | or_op rel_expr { or_op rel_expr } ]
/// rel_expr      ::= bv_term [ rel_op bv_term ]
/// bv_term       ::= term { "++" term }
/// term          ::= factor { add_op factor }
/// factor        ::= power { mul_op power }
/// power         ::= unary_expr [ "**" power ]
/// unary_expr    ::= ( "-" unary_expr | neg_op unary_expr | coercion_expr )
/// coercion_expr ::= array_expr { ":" ( type | nat ) }
/// array_expr    ::= atom_expr { "[" [ exprs [ ":=" expr ] | ":=" expr ] "]" }
/// atom_expr     :: ( bool_lit | nat | dec | float | bv_lit | ident [ "(" ( expr | ε ) ")" ] | old_expr | arith_coercion_expr | paren_expr | forall_expr | exists_expr | lambda_expr | if_then_else_expr | code_expr )
/// paren_expr    ::= "(" expr ")"
/// ```
///
/// They're not in the official grammar, but expressions also include [Float]s.
#[derive(Clone, Debug, Display, From)]
pub enum Expr {
    #[from]
    Equiv(Box<EquivExpr>),
    #[from]
    Implies(Box<ImpliesExpr>),
    #[from]
    Explies(Box<ExpliesExpr>),
    #[from]
    Logical(Box<LogicalExpr>),
    #[from]
    Rel(Box<RelExpr>),
    #[from]
    BvTerm(Box<BvTerm>),
    #[from]
    Term(Box<Term>),
    #[from]
    Factor(Box<Factor>),
    #[from]
    Power(Box<Power>),
    #[from]
    Neg(Box<NegExpr>),
    #[from]
    Coercion(Box<CoercionExpr>),
    #[from]
    Array(Box<ArrayExpr>),
    #[from]
    BoolLit(bool),
    Nat(Nat),
    #[from]
    Dec(Dec),
    #[from]
    Float(Float),
    #[from]
    BvLit(BvLit),
    Var(Ident),
    #[from]
    FuncCall(FuncCall),
    #[from]
    Old(Box<OldExpr>),
    #[from]
    ArithCoercion(Box<ArithCoercionExpr>),
    #[from]
    Quant(Box<QuantExpr>),
    #[from]
    IfThenElse(Box<IfThenElseExpr>),
    #[from]
    Code(Box<CodeExpr>),
}

box_from!(EquivExpr => Expr);
box_from!(ImpliesExpr => Expr);
box_from!(ExpliesExpr => Expr);
box_from!(LogicalExpr => Expr);
box_from!(RelExpr => Expr);
box_from!(BvTerm => Expr);
box_from!(Term => Expr);
box_from!(Factor => Expr);
box_from!(Power => Expr);
box_from!(NegExpr => Expr);
box_from!(CoercionExpr => Expr);
box_from!(ArrayExpr => Expr);
box_from!(OldExpr => Expr);
box_from!(ArithCoercionExpr => Expr);
box_from!(QuantExpr => Expr);
box_from!(IfThenElseExpr => Expr);
box_from!(CodeExpr => Expr);

trait ParentExpr {
    fn child_expr(&self, index: usize) -> Option<&Expr>;
    fn child_needs_parens(&self, expr: &Expr, index: usize) -> bool;
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-expr):
///
/// ```grammar
/// expr     ::= implies_expr { equiv_op implies_expr }
/// equiv_op ::= ( "<==>" | "⇔" )
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{} <==> {}", "ChildExpr(self, 0)", "ChildExpr(self, 1)")]
pub struct EquivExpr {
    pub lhs: Expr,
    pub rhs: Expr,
}

impl ParentExpr for EquivExpr {
    fn child_expr(&self, index: usize) -> Option<&Expr> {
        match index {
            0 => Some(&self.lhs),
            1 => Some(&self.rhs),
            _ => None,
        }
    }

    fn child_needs_parens(&self, expr: &Expr, index: usize) -> bool {
        match expr {
            Expr::Equiv(_) => index == 1,
            _ => false,
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-implies_expr):
///
/// ```grammar
/// implies_expr ::= logical_expr [ implies_op implies_expr | explies_op logical_expr { explies_op logical_expr } ]
/// implies_op   ::= ( "==>" | "⇒" )
/// explies_op   ::= ( "<==" | "⇐" )
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{} ==> {}", "ChildExpr(self, 0)", "ChildExpr(self, 1)")]
pub struct ImpliesExpr {
    pub lhs: Expr,
    pub rhs: Expr,
}

impl ParentExpr for ImpliesExpr {
    fn child_expr(&self, index: usize) -> Option<&Expr> {
        match index {
            0 => Some(&self.lhs),
            1 => Some(&self.rhs),
            _ => None,
        }
    }

    fn child_needs_parens(&self, expr: &Expr, index: usize) -> bool {
        match expr {
            Expr::Equiv(_) => true,
            Expr::Implies(_) | Expr::Explies(_) => index == 0,
            _ => false,
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-implies_expr):
///
/// ```grammar
/// implies_expr ::= logical_expr [ implies_op implies_expr | explies_op logical_expr { explies_op logical_expr } ]
/// explies_op   ::= ( "<==" | "⇐" )
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{} <== {}", "ChildExpr(self, 0)", "ChildExpr(self, 1)")]
pub struct ExpliesExpr {
    pub lhs: Expr,
    pub rhs: Expr,
}

impl ParentExpr for ExpliesExpr {
    fn child_expr(&self, index: usize) -> Option<&Expr> {
        match index {
            0 => Some(&self.lhs),
            1 => Some(&self.rhs),
            _ => None,
        }
    }

    fn child_needs_parens(&self, expr: &Expr, index: usize) -> bool {
        match expr {
            Expr::Equiv(_) | Expr::Implies(_) => true,
            Expr::Explies(_) => index == 1,
            _ => false,
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-logical_expr):
///
/// ```grammar
/// logical_expr ::= rel_expr [ and_op rel_expr { and_op rel_expr } | or_op rel_expr { or_op rel_expr } ]
/// and_op       ::= ( "&&" | "∧" )
/// or_op        ::= ( "||" | "∨" )
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{} {op} {}", "ChildExpr(self, 0)", "ChildExpr(self, 1)")]
pub struct LogicalExpr {
    pub lhs: Expr,
    pub op: LogicalOp,
    pub rhs: Expr,
}

impl ParentExpr for LogicalExpr {
    fn child_expr(&self, index: usize) -> Option<&Expr> {
        match index {
            0 => Some(&self.lhs),
            1 => Some(&self.rhs),
            _ => None,
        }
    }

    fn child_needs_parens(&self, expr: &Expr, index: usize) -> bool {
        match expr {
            Expr::Equiv(_) | Expr::Implies(_) | Expr::Explies(_) => true,
            Expr::Logical(logical_expr) => {
                if self.op == logical_expr.op {
                    index == 1
                } else {
                    true
                }
            }
            _ => false,
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-and_op):
///
/// ```grammar
/// and_op ::= ( "&&" | "∧" )
/// or_op  ::= ( "||" | "∨" )
/// ```
#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum LogicalOp {
    #[display(fmt = "&&")]
    And,
    #[display(fmt = "||")]
    Or,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-rel_expr):
///
/// ```grammar
/// rel_expr ::= bv_term [ rel_op bv_term ]
/// rel_op   ::= ( "==" | "<" | ">" | "<=" | ">=" | "!=" | "<:" | "≠" | "≤" | "≥" )
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{} {op} {}", "ChildExpr(self, 0)", "ChildExpr(self, 1)")]
pub struct RelExpr {
    pub lhs: Expr,
    pub op: RelOp,
    pub rhs: Expr,
}

impl ParentExpr for RelExpr {
    fn child_expr(&self, index: usize) -> Option<&Expr> {
        match index {
            0 => Some(&self.lhs),
            1 => Some(&self.rhs),
            _ => None,
        }
    }

    fn child_needs_parens(&self, expr: &Expr, _index: usize) -> bool {
        match expr {
            Expr::Equiv(_)
            | Expr::Implies(_)
            | Expr::Explies(_)
            | Expr::Logical(_)
            | Expr::Rel(_) => true,
            _ => false,
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-rel_op):
///
/// ```grammar
/// rel_op ::= ( "==" | "<" | ">" | "<=" | ">=" | "!=" | "<:" | "≠" | "≤" | "≥" )
/// ```
#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum RelOp {
    #[display(fmt = "==")]
    Eq,
    #[display(fmt = "<")]
    Lt,
    #[display(fmt = ">")]
    Gt,
    #[display(fmt = "<=")]
    Le,
    #[display(fmt = ">=")]
    Ge,
    #[display(fmt = "!=")]
    Neq,
    #[display(fmt = "<:")]
    Subtype,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-bv_term):
///
/// ```grammar
/// bv_term ::= term { "++" term }
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{} ++ {}", "ChildExpr(self, 0)", "ChildExpr(self, 1)")]
pub struct BvTerm {
    pub lhs: Expr,
    pub rhs: Expr,
}

impl ParentExpr for BvTerm {
    fn child_expr(&self, index: usize) -> Option<&Expr> {
        match index {
            0 => Some(&self.lhs),
            1 => Some(&self.rhs),
            _ => None,
        }
    }

    fn child_needs_parens(&self, expr: &Expr, index: usize) -> bool {
        match expr {
            Expr::Equiv(_)
            | Expr::Implies(_)
            | Expr::Explies(_)
            | Expr::Logical(_)
            | Expr::Rel(_) => true,
            Expr::BvTerm(_) => index == 1,
            _ => false,
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-term):
///
/// ```grammar
/// term   ::= factor { add_op factor }
/// add_op ::= ( "+" | "-" )
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{} {op} {}", "ChildExpr(self, 0)", "ChildExpr(self, 1)")]
pub struct Term {
    pub lhs: Expr,
    pub op: AddOp,
    pub rhs: Expr,
}

impl ParentExpr for Term {
    fn child_expr(&self, index: usize) -> Option<&Expr> {
        match index {
            0 => Some(&self.lhs),
            1 => Some(&self.rhs),
            _ => None,
        }
    }

    fn child_needs_parens(&self, expr: &Expr, index: usize) -> bool {
        match expr {
            Expr::Equiv(_)
            | Expr::Implies(_)
            | Expr::Explies(_)
            | Expr::Logical(_)
            | Expr::Rel(_)
            | Expr::BvTerm(_) => true,
            Expr::Term(_) => index == 1,
            _ => false,
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-add_op):
///
/// ```grammar
/// add_op ::= ( "+" | "-" )
/// ```
#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum AddOp {
    #[display(fmt = "+")]
    Add,
    #[display(fmt = "-")]
    Sub,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-factor):
///
/// ```grammar
/// factor ::= power { mul_op power }
/// mul_op ::= ( "*" | "div" | "mod" | "/" )
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{} {op} {}", "ChildExpr(self, 0)", "ChildExpr(self, 1)")]
pub struct Factor {
    pub lhs: Expr,
    pub op: MulOp,
    pub rhs: Expr,
}

impl ParentExpr for Factor {
    fn child_expr(&self, index: usize) -> Option<&Expr> {
        match index {
            0 => Some(&self.lhs),
            1 => Some(&self.rhs),
            _ => None,
        }
    }

    fn child_needs_parens(&self, expr: &Expr, index: usize) -> bool {
        match expr {
            Expr::Equiv(_)
            | Expr::Implies(_)
            | Expr::Explies(_)
            | Expr::Logical(_)
            | Expr::Rel(_)
            | Expr::BvTerm(_)
            | Expr::Term(_) => true,
            Expr::Factor(_) => index == 1,
            _ => false,
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-mul_op):
///
/// ```grammar
/// mul_op ::= ( "*" | "div" | "mod" | "/" )
/// ```
#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum MulOp {
    #[display(fmt = "*")]
    Mul,
    #[display(fmt = "div")]
    Div,
    #[display(fmt = "mod")]
    Mod,
    #[display(fmt = "/")]
    RealDiv,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-power):
///
/// ```grammar
/// power ::= unary_expr [ "**" power ]
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{} ** {}", "ChildExpr(self, 0)", "ChildExpr(self, 1)")]
pub struct Power {
    pub lhs: Expr,
    pub rhs: Expr,
}

impl ParentExpr for Power {
    fn child_expr(&self, index: usize) -> Option<&Expr> {
        match index {
            0 => Some(&self.lhs),
            1 => Some(&self.rhs),
            _ => None,
        }
    }

    fn child_needs_parens(&self, expr: &Expr, index: usize) -> bool {
        match expr {
            Expr::Equiv(_)
            | Expr::Implies(_)
            | Expr::Explies(_)
            | Expr::Logical(_)
            | Expr::Rel(_)
            | Expr::BvTerm(_)
            | Expr::Term(_)
            | Expr::Factor(_) => true,
            Expr::Power(_) => index == 0,
            _ => false,
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-unary_expr):
///
/// ```grammar
/// unary_expr ::= ( "-" unary_expr | neg_op unary_expr | coercion_expr )
/// neg_op     ::= ( "!" | "¬" )
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{op}{}", "ChildExpr(self, 0)")]
pub struct NegExpr {
    pub op: NegOp,
    pub expr: Expr,
}

impl ParentExpr for NegExpr {
    fn child_expr(&self, index: usize) -> Option<&Expr> {
        match index {
            0 => Some(&self.expr),
            _ => None,
        }
    }

    fn child_needs_parens(&self, expr: &Expr, _index: usize) -> bool {
        match expr {
            Expr::Equiv(_)
            | Expr::Implies(_)
            | Expr::Explies(_)
            | Expr::Logical(_)
            | Expr::Rel(_)
            | Expr::BvTerm(_)
            | Expr::Term(_)
            | Expr::Factor(_)
            | Expr::Power(_) => true,
            _ => false,
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-unary_expr):
///
/// ```grammar
/// unary_expr ::= ( "-" unary_expr | neg_op unary_expr | coercion_expr )
/// neg_op     ::= ( "!" | "¬" )
/// ```
#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum NegOp {
    #[display(fmt = "-")]
    Arith,
    #[display(fmt = "!")]
    Logical,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-coercion_expr):
///
/// ```grammar
/// coercion_expr ::= array_expr { ":" ( type | nat ) }
/// ```
///
/// Note that this introduces an unfortunate ambiguity. Quoting the
/// [source](https://github.com/boogie-org/boogie/blob/a938997b52b53e5a67a8d54b128847da3b434a44/Source/Core/BoogiePL.atg#L1258-L1263):
///
/// > This production creates ambiguities, because types can start with "<"
/// > (polymorphic map types), but can also be followed by "<" (inequalities).
/// > Coco deals with these ambiguities in a reasonable way by preferring to
/// > read further types (type arguments) over relational symbols. E.g.,
/// > "5 : C < 0" will cause a parse error because "<" is treated as the
/// > beginning of a map type.
#[derive(Clone, Debug)]
pub struct CoercionExpr {
    pub expr: Expr,
    pub coercions: Vec<Coercion>,
}

impl Display for CoercionExpr {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", ChildExpr(self, 0))?;
        for coercion in &self.coercions {
            write!(f, " : {coercion}")?;
        }
        Ok(())
    }
}

impl ParentExpr for CoercionExpr {
    fn child_expr(&self, index: usize) -> Option<&Expr> {
        match index {
            0 => Some(&self.expr),
            _ => None,
        }
    }

    fn child_needs_parens(&self, expr: &Expr, _index: usize) -> bool {
        match expr {
            Expr::Equiv(_)
            | Expr::Implies(_)
            | Expr::Explies(_)
            | Expr::Logical(_)
            | Expr::Rel(_)
            | Expr::BvTerm(_)
            | Expr::Term(_)
            | Expr::Factor(_)
            | Expr::Power(_)
            | Expr::Neg(_)
            | Expr::Coercion(_) => true,
            _ => false,
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-coercion_expr):
///
/// ```grammar
/// coercion_expr ::= array_expr { ":" ( type | nat ) }
/// ```
#[derive(Clone, Debug, Display, From)]
pub enum Coercion {
    Type(Type),
    Nat(Nat),
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-array_expr):
///
/// ```grammar
/// array_expr ::= atom_expr { "[" [ exprs [ ":=" expr ] | ":=" expr ] "]" }
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{}{}", "ChildExpr(self, 0)", "subscripts.iter().join_concat()")]
pub struct ArrayExpr {
    pub expr: Expr,
    pub subscripts: Vec<ArraySubscript>,
}

impl ParentExpr for ArrayExpr {
    fn child_expr(&self, index: usize) -> Option<&Expr> {
        match index {
            0 => Some(&self.expr),
            _ => None,
        }
    }

    fn child_needs_parens(&self, expr: &Expr, _index: usize) -> bool {
        match expr {
            Expr::Equiv(_)
            | Expr::Implies(_)
            | Expr::Explies(_)
            | Expr::Logical(_)
            | Expr::Rel(_)
            | Expr::BvTerm(_)
            | Expr::Term(_)
            | Expr::Factor(_)
            | Expr::Power(_)
            | Expr::Neg(_)
            | Expr::Coercion(_)
            | Expr::Array(_) => true,
            _ => false,
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-array_expr):
///
/// ```grammar
/// array_expr ::= atom_expr { "[" [ exprs [ ":=" expr ] | ":=" expr ] "]" }
/// ```
#[derive(Clone, Debug)]
pub struct ArraySubscript {
    pub keys: Exprs,
    pub value: Option<Expr>,
}

impl Display for ArraySubscript {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "[{}", self.keys)?;
        if let Some(value) = &self.value {
            if !self.keys.is_empty() {
                write!(f, " ")?;
            }
            write!(f, ":= {value}")?;
        }
        write!(f, "]")?;
        Ok(())
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-atom_expr):
///
/// ```grammar
/// atom_expr :: ( bool_lit | nat | dec | float | bv_lit | ident [ "(" ( expr | ε ) ")" ] | old_expr | arith_coercion_expr | paren_expr | forall_expr | exists_expr | lambda_expr | if_then_else_expr | code_expr )
/// ```
///
/// Note that function arguments are incorrectly defined in the official
/// grammar, as reproduced above.
#[derive(Clone, Debug, Display)]
#[display(fmt = "{target}({args})")]
pub struct FuncCall {
    pub target: Ident,
    pub args: Exprs,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-old_expr):
///
/// ```grammar
/// old_expr ::= "old" "(" expr ")"
/// ```
#[derive(Clone, Debug, Display, From)]
#[display(fmt = "old ({expr})")]
pub struct OldExpr {
    pub expr: Expr,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-arith_coercion_expr):
///
/// ```grammar
/// arith_coercion_expr ::= ( "int" "(" expr ")" | "real" "(" expr ")" )
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{kind}({expr})")]
pub struct ArithCoercionExpr {
    pub kind: ArithCoercionKind,
    pub expr: Expr,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-arith_coercion_expr):
///
/// ```grammar
/// arith_coercion_expr ::= ( "int" "(" expr ")" | "real" "(" expr ")" )
/// ```
#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum ArithCoercionKind {
    #[display(fmt = "int")]
    Int,
    #[display(fmt = "real")]
    Real,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-forall_expr):
///
/// ```grammar
/// forall_expr ::= "(" forall quant_body ")"
/// exists_expr ::= "(" exists quant_body ")"
/// lambda_expr ::= "(" lambda quant_body ")"
/// forall      ::= ( "forall" | "∀" )
/// exists      ::= ( "exists" | "∃" )
/// lambda      ::= ( "lambda" | "λ" )
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "({kind} {body})")]
pub struct QuantExpr {
    pub kind: QuantKind,
    pub body: QuantBody,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-forall):
///
/// ```grammar
/// forall ::= ( "forall" | "∀" )
/// exists ::= ( "exists" | "∃" )
/// lambda ::= ( "lambda" | "λ" )
/// ```
#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum QuantKind {
    #[display(fmt = "forall")]
    ForAll,
    #[display(fmt = "exists")]
    Exists,
    #[display(fmt = "lambda")]
    Lambda,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-quant_body):
///
/// ```grammar
/// quant_body ::= ( type_params [ bound_vars ] | bound_vars ) qsep { attr_or_trigger } expr
/// qsep       ::= ( "::" | "•" )
/// ```
#[derive(Clone, Debug)]
pub struct QuantBody {
    pub type_params: TypeParams,
    pub bound_vars: BoundVars,
    // TODO(spinda): Rename this to be more accurate.
    pub attrs: Vec<AttrOrTrigger>,
    pub expr: Expr,
}

impl Display for QuantBody {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if !self.type_params.is_empty() {
            write!(f, "{} ", self.type_params)?;
        }
        if !self.bound_vars.is_empty() {
            write!(f, "{} ", self.bound_vars)?;
        }
        write!(f, ":: ")?;
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        write!(f, "{}", self.expr)
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-bound_vars):
///
/// ```grammar
/// bound_vars ::= attr_typed_idents_wheres
/// ```
pub type BoundVars = AttrTypedIdentsWheres;

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-if_then_else_expr):
///
/// ```grammar
/// if_then_else_expr ::= "if" expr "then" expr "else" expr
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "if {cond} then {then} else {else_}")]
pub struct IfThenElseExpr {
    pub cond: Expr,
    pub then: Expr,
    pub else_: Expr,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-code_expr):
///
/// ```grammar
/// code_expr ::= "|{" { local_vars } spec_block { speck_block } "}|"
/// ```
///
/// Note: `spec[k]_block` is misspelled in the official grammar, as reproduced
/// above.
#[derive(Clone, Debug)]
pub struct CodeExpr {
    pub local_vars: Vec<LocalVars>,
    pub spec_blocks: Vec<SpecBlock>,
}

impl Display for CodeExpr {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "|{{\n")?;

        let mut indented = IndentWriter::new(INDENT, f);
        write!(indented, "{}", self.local_vars.iter().join_with("\n"))?;
        if !self.local_vars.is_empty() && !self.spec_blocks.is_empty() {
            write!(indented, "\n\n")?;
        }
        write!(indented, "{}", self.spec_blocks.iter().join_with("\n"))?;
        let f = indented.into_inner();

        write!(f, "\n}}|")
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-spec_block):
///
/// ```grammar
/// spec_block ::= ident ":" { label_or_cmd } ( "goto" idents | "return" expr ) ";"
/// ```
#[derive(Clone, Debug)]
pub struct SpecBlock {
    pub label: Ident,
    pub cmds: Vec<LabelOrCmd>,
    pub transfer: SpecTransfer,
}

impl Display for SpecBlock {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}:\n", self.label)?;

        let mut indented = IndentWriter::new(INDENT, f);
        for cmd in &self.cmds {
            write!(indented, "{cmd}\n")?;
        }
        write!(indented, "{};", self.transfer)
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-spec_block):
///
/// ```grammar
/// spec_block ::= ident ":" { label_or_cmd } ( "goto" idents | "return" expr ) ";"
/// ```
#[derive(Clone, Debug, Display, From)]
pub enum SpecTransfer {
    Goto(SpecGoto),
    Return(SpecReturn),
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-spec_block):
///
/// ```grammar
/// spec_block ::= ident ":" { label_or_cmd } ( "goto" idents | "return" expr ) ";"
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "goto {labels}")]
pub struct SpecGoto {
    pub labels: Idents,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-spec_block):
///
/// ```grammar
/// spec_block ::= ident ":" { label_or_cmd } ( "goto" idents | "return" expr ) ";"
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "return {value}")]
pub struct SpecReturn {
    pub value: Expr,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-nat):
///
/// ```grammar
/// nat ::= digits
/// ```
pub type Nat = Digits;

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-dec):
///
/// ```grammar
/// dec       ::= ( decimal | dec_float )
/// decimal   ::= digits "e" [ "-" ] digits
/// dec_float ::= digits "." digits [ "e" [ "-" ] digits ]
/// ```
///
/// Note that unlike the rest of the grammar, the tokens that make up a [Dec]
/// *must not* be separated by whitespace.
#[derive(Clone, Debug)]
pub struct Dec {
    pub whole: Digits,
    pub fract: Option<Digits>,
    pub exp: Option<Exp>,
}

impl Display for Dec {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.whole)?;
        if let Some(fract) = &self.fract {
            write!(f, ".{fract}")?;
        }
        if let Some(exp) = &self.exp {
            write!(f, "{exp}")?;
        }
        Ok(())
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-dec):
///
/// ```grammar
/// dec       ::= ( decimal | dec_float )
/// decimal   ::= digits "e" [ "-" ] digits
/// dec_float ::= digits "." digits [ "e" [ "-" ] digits ]
/// ```
#[derive(Clone, Debug)]
pub struct Exp {
    pub is_neg: bool,
    pub digits: Digits,
}

impl Display for Exp {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "e")?;
        if self.is_neg {
            write!(f, "-")?;
        }
        write!(f, "{}", self.digits)
    }
}

/// Not in the official grammar.
///
/// [Reference](https://github.com/boogie-org/boogie/blob/a938997b52b53e5a67a8d54b128847da3b434a44/Source/Core/BoogiePL.atg#L164-L168):
///
/// ```coco
/// float = [ '-' ] '0' 'x' hexdigit {hexdigit} '.' hexdigit {hexdigit} 'e' [ '-' ] digit {digit} 'f' digit {digit} 'e' digit {digit}
///       | '0' 'N' 'a' 'N' digit {digit} 'e' digit {digit}
///       | '0' 'n' 'a' 'n' digit {digit} 'e' digit {digit}
///       | '0' '+' 'o' 'o' digit {digit} 'e' digit {digit}
///       | '0' '-' 'o' 'o' digit {digit} 'e' digit {digit} .
/// ```
#[derive(Clone, Debug, From)]
pub struct Float {
    pub value: FloatValue,
    pub sig_size: Digits,
    pub exp_size: Digits,
}

impl Display for Float {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.value)?;
        if let FloatValue::Plain(_) = &self.value {
            write!(f, "f")?;
        }
        write!(f, "{}e{}", self.sig_size, self.exp_size)
    }
}

/// Not in the official grammar.
///
/// [Reference](https://github.com/boogie-org/boogie/blob/a938997b52b53e5a67a8d54b128847da3b434a44/Source/Core/BoogiePL.atg#L164-L168):
///
/// ```coco
/// float = [ '-' ] '0' 'x' hexdigit {hexdigit} '.' hexdigit {hexdigit} 'e' [ '-' ] digit {digit} 'f' digit {digit} 'e' digit {digit}
///       | '0' 'N' 'a' 'N' digit {digit} 'e' digit {digit}
///       | '0' 'n' 'a' 'n' digit {digit} 'e' digit {digit}
///       | '0' '+' 'o' 'o' digit {digit} 'e' digit {digit}
///       | '0' '-' 'o' 'o' digit {digit} 'e' digit {digit} .
/// ```
#[derive(Clone, Debug, Display, From)]
pub enum FloatValue {
    Plain(PlainFloatValue),
    #[display(fmt = "0NaN")]
    NotANumber,
    #[display(fmt = "0+oo")]
    PosInfinity,
    #[display(fmt = "0-oo")]
    NegInfinity,
}

/// Not in the official grammar.
///
/// [Reference](https://github.com/boogie-org/boogie/blob/a938997b52b53e5a67a8d54b128847da3b434a44/Source/Core/BoogiePL.atg#L164-L168):
///
/// ```coco
/// float = [ '-' ] '0' 'x' hexdigit {hexdigit} '.' hexdigit {hexdigit} 'e' [ '-' ] digit {digit} 'f' digit {digit} 'e' digit {digit}
///       | '0' 'N' 'a' 'N' digit {digit} 'e' digit {digit}
///       | '0' 'n' 'a' 'n' digit {digit} 'e' digit {digit}
///       | '0' '+' 'o' 'o' digit {digit} 'e' digit {digit}
///       | '0' '-' 'o' 'o' digit {digit} 'e' digit {digit} .
/// ```
#[derive(Clone, Debug)]
pub struct PlainFloatValue {
    pub is_neg: bool,
    pub whole: HexDigits,
    pub fract: HexDigits,
    pub exp: Exp,
}

impl Display for PlainFloatValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if self.is_neg {
            write!(f, "-")?;
        }
        write!(f, "0x{}.{}{}", self.whole, self.fract, self.exp)
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-bv_lit):
///
/// ```grammar
/// bv_lit ::= digits "bv" digits
/// ```
///
/// Note that unlike the rest of the grammar, the tokens that make up a [BvLit]
/// *must not* be separated by whitespace.
#[derive(Clone, Debug, Display)]
#[display(fmt = "{n}bv{width}")]
pub struct BvLit {
    pub n: Digits,
    pub width: Digits,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-attr_typed_idents_wheres):
///
/// ```grammar
/// attr_typed_idents_wheres ::= attr_typed_idents_where { "," attr_typed_idents_where }
/// ```
#[derive(Clone, Default, Display)]
#[display(fmt = "{}", "self.iter().join_with(\", \")")]
pub struct AttrTypedIdentsWheres {
    pub items: Vec<Spanned<AttrTypedIdentsWhere>>,
}

collection_wrapper_impls!(AttrTypedIdentsWheres:items => Spanned<AttrTypedIdentsWhere>);

impl From<TypedIdentsWheres> for AttrTypedIdentsWheres {
    fn from(typed_idents_wheres: TypedIdentsWheres) -> Self {
        AttrTypedIdentsWheres::from_iter(typed_idents_wheres)
    }
}

impl From<Spanned<TypedIdentsWhere>> for AttrTypedIdentsWheres {
    fn from(typed_idents_where: Spanned<TypedIdentsWhere>) -> Self {
        typed_idents_where.map(AttrTypedIdentsWhere::from).into()
    }
}

impl From<Spanned<TypedIdents>> for AttrTypedIdentsWheres {
    fn from(typed_idents: Spanned<TypedIdents>) -> Self {
        typed_idents.map(TypedIdentsWhere::from).into()
    }
}

impl FromIterator<Spanned<TypedIdentsWhere>> for AttrTypedIdentsWheres {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Spanned<TypedIdentsWhere>>,
    {
        iter.into_iter()
            .map(|typed_idents_where| typed_idents_where.map(AttrTypedIdentsWhere::from))
            .collect()
    }
}

impl FromIterator<Spanned<TypedIdents>> for AttrTypedIdentsWheres {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Spanned<TypedIdents>>,
    {
        iter.into_iter()
            .map(|typed_idents| typed_idents.map(AttrTypedIdentsWhere::from))
            .collect()
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-attr_typed_idents_where):
///
/// ```grammar
/// attr_typed_idents_where ::= { attr } typed_idents_where
/// ```
#[derive(Clone, Debug)]
pub struct AttrTypedIdentsWhere {
    pub attrs: Vec<Attr>,
    pub typed_idents_where: TypedIdentsWhere,
}

impl Display for AttrTypedIdentsWhere {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        for attr in &self.attrs {
            write!(f, "{attr} ")?;
        }
        write!(f, "{}", self.typed_idents_where)
    }
}

impl From<TypedIdentsWhere> for AttrTypedIdentsWhere {
    fn from(typed_idents_where: TypedIdentsWhere) -> Self {
        Self {
            attrs: Vec::new(),
            typed_idents_where,
        }
    }
}

impl From<TypedIdents> for AttrTypedIdentsWhere {
    fn from(typed_idents: TypedIdents) -> Self {
        TypedIdentsWhere::from(typed_idents).into()
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-typed_idents_wheres):
///
/// ```grammar
/// typed_idents_wheres ::= typed_idents_where { "," typed_idents_where }
/// ```
#[derive(Clone, Default, Display)]
#[display(fmt = "{}", "self.iter().join_with(\", \")")]
pub struct TypedIdentsWheres {
    pub items: Vec<Spanned<TypedIdentsWhere>>,
}

collection_wrapper_impls!(TypedIdentsWheres:items => Spanned<TypedIdentsWhere>);

impl From<Spanned<TypedIdents>> for TypedIdentsWheres {
    fn from(typed_idents: Spanned<TypedIdents>) -> Self {
        typed_idents.map(TypedIdentsWhere::from).into()
    }
}

impl FromIterator<Spanned<TypedIdents>> for TypedIdentsWheres {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Spanned<TypedIdents>>,
    {
        iter.into_iter()
            .map(|typed_idents| typed_idents.map(TypedIdentsWhere::from))
            .collect()
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-typed_idents_where):
///
/// ```grammar
/// typed_idents_where ::= typed_idents [ "where" expr ]
/// ```
#[derive(Clone, Debug)]
pub struct TypedIdentsWhere {
    pub typed_idents: TypedIdents,
    pub where_: Option<Expr>,
}

impl Display for TypedIdentsWhere {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.typed_idents)?;
        if let Some(where_) = &self.where_ {
            write!(f, " where {where_}")?;
        }
        Ok(())
    }
}

impl From<TypedIdents> for TypedIdentsWhere {
    fn from(typed_idents: TypedIdents) -> Self {
        Self {
            typed_idents,
            where_: None,
        }
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-typed_idents):
///
/// ```grammar
/// typed_idents ::= idents ":" type
/// ```
#[derive(Clone, Debug, Display)]
#[display(fmt = "{idents}: {type_}")]
pub struct TypedIdents {
    pub idents: Idents,
    pub type_: Type,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-idents):
///
/// ```grammar
/// idents ::= ident { "," ident }
/// ```
#[derive(Clone, Default, Display)]
#[display(fmt = "{}", "self.iter().join_with(\", \")")]
pub struct Idents {
    pub idents: Vec<Spanned<Ident>>,
}

collection_wrapper_impls!(Idents:idents => Spanned<Ident>);

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-type_params):
///
/// ```grammar
/// type_params ::= "<" idents ">"
/// ```
#[derive(Clone, Default)]
pub struct TypeParams {
    pub params: Idents,
}

impl Display for TypeParams {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if !self.params.is_empty() {
            write!(f, "<{}>", self.params)?;
        }
        Ok(())
    }
}

collection_wrapper_impls!(TypeParams:params => Spanned<Ident> | Idents);

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-attr):
///
/// ```grammar
/// attr ::= attr_or_trigger
/// ```
pub type Attr = AttrOrTrigger;

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-attr_or_trigger):
///
/// ```grammar
/// attr_or_trigger ::= "{" ( ":" ident [ attr_param { "," attr_param } ] | exprs ) "}"
/// ```
#[derive(Clone, Debug, Display, From)]
#[display(fmt = "{{{}}}")]
pub enum AttrOrTrigger {
    #[from]
    Attr(AttrContent),
    Trigger(Exprs),
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-attr_or_trigger):
///
/// ```grammar
/// attr_or_trigger ::= "{" ( ":" ident [ attr_param { "," attr_param } ] | exprs ) "}"
/// ```
#[derive(Clone, Debug)]
pub struct AttrContent {
    pub ident: Ident,
    pub params: Vec<AttrParam>,
}

impl Display for AttrContent {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, ":{}", self.ident)?;
        if !self.params.is_empty() {
            write!(f, " {}", self.params.iter().join_with(", "))?;
        }
        Ok(())
    }
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-attr_param):
///
/// ```grammar
/// attr_param ::= ( string | expr )
/// ```
#[derive(Clone, Debug, Display, From)]
pub enum AttrParam {
    String(StringLit),
    Expr(Expr),
}

from_expr_impls!(AttrParam);

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-string):
///
/// ```grammar
/// string      ::= quote { string_char | "\\\"" } quote
/// quote       ::= "\""
/// string_char ::= any character, except newline or quote
/// ```
///
/// Note that unlike the rest of the grammar, whitespace *must not* be thrown
/// away between the `quote` tokens on either end of a [StringLit].
#[derive(Clone, Debug, Display)]
#[display(fmt = "\"{text}\"")]
pub struct StringLit {
    /// This contains the ***raw*** text between the quotes of the string
    /// literal, so quotes must be escaped!
    pub text: String,
}

/// [Grammar](https://boogie-docs.readthedocs.io/en/latest/LangRef.html#grammar-token-digits):
///
/// ```grammar
/// digits ::= digit { digit }
/// digit  ::= "0…9"
/// ```
pub type Digits = String;

pub type HexDigits = String;

// * Expr-Printing Helpers

struct ChildExpr<'a, T>(&'a T, usize);

impl<T: Display + ParentExpr> Display for ChildExpr<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        let ChildExpr(parent_expr, index) = self;
        let child_expr = parent_expr
            .child_expr(*index)
            .expect("missing child expression");

        let child_needs_parens = match child_expr {
            // Thanks to the trailing `Expr`, the rules for whether
            // `IfThenElseExpr` needs to be surrounded by parentheses as a child
            // expression are complex. Err on the side of caution when printing
            // and prefer wrapping these in parentheses.
            Expr::IfThenElse(_) => true,
            _ => parent_expr.child_needs_parens(child_expr, *index),
        };
        if child_needs_parens {
            write!(f, "({})", child_expr)
        } else {
            write!(f, "{}", child_expr)
        }
    }
}
