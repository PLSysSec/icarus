// vim: set tw=99 ts=4 sts=4 sw=4 et:

use derive_more::Display;
use std::error;
use std::fmt::{self, Debug, Display};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Display, Eq, PartialEq, Ord, PartialOrd)]
pub enum Span {
    #[display(fmt = "[internal]")]
    Internal,
    #[display(fmt = "{}", _0)]
    External(codespan::Span),
}

impl<T> From<Span> for Range<T>
where
    Range<T>: From<codespan::Span> + Default,
{
    fn from(span: Span) -> Self {
        match span {
            Span::Internal => Default::default(),
            Span::External(csp) => Range::<T>::from(csp),
        }
    }
}

impl<T> From<T> for Span
where
    codespan::Span: From<T>,
{
    fn from(t: T) -> Self {
        Span::External(codespan::Span::from(t))
    }
}

#[derive(Clone, Copy)]
pub struct Spanned<T> {
    pub span: Span,
    pub value: T,
}

impl<T> Spanned<T> {
    pub const fn new(span: Span, value: T) -> Self {
        Self { span, value }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Spanned<U> {
        Spanned {
            span: self.span,
            value: f(self.value),
        }
    }

    pub const fn as_ref(&self) -> Spanned<&T> {
        Spanned {
            span: self.span,
            value: &self.value,
        }
    }

    pub fn as_mut(&mut self) -> Spanned<&mut T> {
        Spanned {
            span: self.span,
            value: &mut self.value,
        }
    }
}

impl<T: Clone> Spanned<&T> {
    pub fn cloned(self) -> Spanned<T> {
        self.map(|value| value.clone())
    }
}

impl<T: Copy> Spanned<&T> {
    pub fn copied(self) -> Spanned<T> {
        self.map(|value| *value)
    }
}

impl<T: Debug> Debug for Spanned<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        Debug::fmt(&self.value, f)?;
        write!(f, " @ {}", self.span)
    }
}

impl<T: Display> Display for Spanned<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        Display::fmt(&self.value, f)
    }
}

impl<T: error::Error> error::Error for Spanned<T> {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        self.value.source()
    }

    fn description(&self) -> &str {
        #[allow(deprecated)]
        self.value.description()
    }

    fn cause(&self) -> Option<&dyn error::Error> {
        #[allow(deprecated)]
        self.value.cause()
    }
}

#[derive(Clone, Copy)]
pub struct MaybeSpanned<T> {
    pub span: Option<Span>,
    pub value: T,
}

impl<T> MaybeSpanned<T> {
    pub const fn new(span: Option<Span>, value: T) -> Self {
        Self { span, value }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> MaybeSpanned<U> {
        MaybeSpanned {
            span: self.span,
            value: f(self.value),
        }
    }

    pub const fn as_ref(&self) -> MaybeSpanned<&T> {
        MaybeSpanned {
            span: self.span,
            value: &self.value,
        }
    }

    pub fn as_mut(&mut self) -> MaybeSpanned<&mut T> {
        MaybeSpanned {
            span: self.span,
            value: &mut self.value,
        }
    }
}

impl<T: Clone> MaybeSpanned<&T> {
    pub fn cloned(self) -> MaybeSpanned<T> {
        self.map(|value| value.clone())
    }
}

impl<T: Copy> MaybeSpanned<&T> {
    pub fn copied(self) -> MaybeSpanned<T> {
        self.map(|value| *value)
    }
}

impl<T: Debug> Debug for MaybeSpanned<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self.span {
            Some(span) => Debug::fmt(&Spanned::new(span, &self.value), f),
            None => Debug::fmt(&self.value, f),
        }
    }
}

impl<T: Display> Display for MaybeSpanned<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        Display::fmt(&self.value, f)
    }
}

impl<T: error::Error> error::Error for MaybeSpanned<T> {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        self.value.source()
    }

    fn description(&self) -> &str {
        #[allow(deprecated)]
        self.value.description()
    }

    fn cause(&self) -> Option<&dyn error::Error> {
        #[allow(deprecated)]
        self.value.cause()
    }
}

impl<T> From<T> for MaybeSpanned<T> {
    fn from(value: T) -> Self {
        MaybeSpanned::new(None, value)
    }
}

impl<T> From<Spanned<T>> for MaybeSpanned<T> {
    fn from(spanned: Spanned<T>) -> Self {
        MaybeSpanned::new(Some(spanned.span), spanned.value)
    }
}
