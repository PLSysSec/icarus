use std::error;
use std::fmt::{self, Debug, Display};

pub use codespan::{ByteIndex, RawIndex, Span};

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
        write!(f, " @ {}", self.span)?;
        Ok(())
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
