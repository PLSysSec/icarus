// vim: set tw=99 ts=4 sts=4 sw=4 et:

use derive_more::Display;

pub use crate::ast::built_ins::*;
pub use crate::ast::ident::*;
pub use crate::ast::span::*;

mod built_ins;
mod ident;
mod span;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CheckKind {
    Assert,
    Assume,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BlockKind {
    Unsafe,
}

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum NegateKind {
    #[display(fmt = "-")]
    Arithmetic,
    #[display(fmt = "!")]
    Logical,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CastKind {
    Downcast,
    Upcast,
}

impl CastKind {
    pub const fn is_unsafe(self) -> bool {
        match self {
            CastKind::Downcast => true,
            CastKind::Upcast => false,
        }
    }

    pub const fn reverse(self) -> Self {
        match self {
            CastKind::Downcast => CastKind::Upcast,
            CastKind::Upcast => CastKind::Downcast,
        }
    }
}

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub enum CompareKind {
    #[display(fmt = "==")]
    Eq,
    #[display(fmt = "!=")]
    Neq,
    #[display(fmt = "<=")]
    Lte,
    #[display(fmt = ">=")]
    Gte,
    #[display(fmt = ">")]
    Lt,
    #[display(fmt = "<")]
    Gt,
}

impl CompareKind {
    pub fn is_numeric(&self) -> bool {
        match self {
            CompareKind::Lte | CompareKind::Gte | CompareKind::Lt | CompareKind::Gt => true,
            CompareKind::Eq | CompareKind::Neq => false,
        }
    }
}
