// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::error::Error;
use std::fmt::{self, Display};

use codespan::RawIndex;
use codespan_reporting::diagnostic::{Diagnostic, LabelStyle};
use phf::phf_map;

use cachet_util::fmt_join_or;

use crate::FrontendError;
use crate::ast::{FileId, Span};

use crate::parser::helpers::RawParseError;

#[derive(Clone, Debug)]
pub struct ParseError {
    file_id: FileId,
    error: RawParseError<String>,
}

impl ParseError {
    pub(super) fn new<T: ToString>(file_id: FileId, error: RawParseError<T>) -> Self {
        Self {
            file_id,
            error: error.map_token(|token| token.to_string()),
        }
    }

    pub fn file_id(&self) -> FileId {
        self.file_id
    }
}

impl Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match &self.error {
            RawParseError::InvalidToken { .. } => write!(f, "invalid token")?,
            RawParseError::UnrecognizedEOF { expected, .. } => {
                write!(f, "unexpected end of input")?;
                fmt_expected(f, expected)?;
            }
            RawParseError::UnrecognizedToken {
                token: (_, token, _),
                expected,
                ..
            } => {
                write!(f, "unexpected token `{}`", token)?;
                fmt_expected(f, expected)?;
            }
            RawParseError::ExtraToken {
                token: (_, token, _),
                ..
            } => {
                write!(f, "extra token `{}`", token)?;
            }
            RawParseError::User { error } => write!(f, "{}", error.value)?,
        }
        Ok(())
    }
}

impl Error for ParseError {}

impl FrontendError for ParseError {
    fn span(&self) -> Span {
        match &self.error {
            RawParseError::InvalidToken { location } => {
                Span::new(self.file_id, *location as RawIndex..*location as RawIndex)
            }
            RawParseError::UnrecognizedEOF { location, .. } => {
                Span::new(self.file_id, *location as RawIndex..*location as RawIndex)
            }
            RawParseError::UnrecognizedToken {
                token: (start, _, end),
                ..
            } => Span::new(self.file_id, *start as RawIndex..*end as RawIndex),
            RawParseError::ExtraToken {
                token: (start, _, end),
            } => Span::new(self.file_id, *start as RawIndex..*end as RawIndex),
            RawParseError::User { error } => return error.span,
        }
    }

    fn build_diagnostic(&self) -> Diagnostic<FileId> {
        let diagnostic = Diagnostic::error().with_message(self.to_string());

        match self.span() {
            Span::Internal => diagnostic,
            Span::External(external_span) => {
                diagnostic.with_labels(vec![external_span.label(LabelStyle::Primary)])
            }
        }
    }
}

static TOKEN_NAMES: phf::Map<&'static str, &'static str> = phf_map! {
    "r#\"[A-Za-z_][A-Za-z0-9_]*\"#" => "identiifer",
    "r#\"[0-9]+_u16\"#" => "UInt16 literal",
    "r#\"[0-9]+_i32\"#" => "Int32 literal",
    "r#\"[0-9]+_i64\"#" => "Int64 literal",
    "r#\"[0-9]+.[0-9]+\"#" => "Double literal",
};

fn fmt_expected(f: &mut fmt::Formatter, expected: &[String]) -> Result<(), fmt::Error> {
    if !expected.is_empty() {
        write!(f, "; expected ")?;
        fmt_join_or(f, expected.iter(), |f, item| {
            match TOKEN_NAMES.get(item.as_str()) {
                Some(token_name) => write!(f, "{}", token_name),
                None => write!(f, "{}", item.replace("\"", "`")),
            }
        })?;
    }
    Ok(())
}
