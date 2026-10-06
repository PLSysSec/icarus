// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::error;
use std::fmt::{self, Debug, Display};

pub use codespan::{ByteIndex, FileId, Span as RawSpan};
use codespan_reporting::diagnostic::{Label, LabelStyle};
use derive_more::{Display, From};

use cachet_util::deref_from;

#[derive(Clone, Copy, Debug, Display, Eq, From, PartialEq, Ord, PartialOrd)]
pub enum Span {
    #[display(fmt = "<internal>")]
    Internal,
    External(ExternalSpan),
}

impl Span {
    pub fn new<T>(file_id: FileId, span: T) -> Self
    where
        RawSpan: From<T>,
    {
        ExternalSpan::new(file_id, span).into()
    }

    pub fn label(&self, style: LabelStyle) -> Option<Label<FileId>> {
        match self {
            Span::Internal => None,
            Span::External(external_span) => Some(external_span.label(style)),
        }
    }
}

deref_from!(&ExternalSpan => Span);

macro_rules! labels {
    ($($style:ident ($span:expr) => $string:expr),*) => {
        ::std::iter::empty()$(
            .chain(
                $span
                    .label(codespan_reporting::diagnostic::LabelStyle::$style)
                    .map(|label| label.with_message($string))
                    .into_iter()
            )
        )*
    }
}

pub(crate) use labels;

#[derive(Clone, Copy, Debug, Display, Eq, PartialEq, Ord, PartialOrd)]
#[display(fmt = "{}", span)]
pub struct ExternalSpan {
    pub file_id: FileId,
    pub span: RawSpan,
}

impl ExternalSpan {
    pub fn new<T>(file_id: FileId, span: T) -> Self
    where
        RawSpan: From<T>,
    {
        Self {
            file_id,
            span: span.into(),
        }
    }

    pub fn label(&self, style: LabelStyle) -> Label<FileId> {
        Label::new(style, self.file_id, self.span)
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

    pub const fn internal(value: T) -> Self {
        Self {
            span: Span::Internal,
            value,
        }
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
        if let Span::External(external_span) = self.span {
            write!(f, " @ {}", external_span)?;
        }
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
