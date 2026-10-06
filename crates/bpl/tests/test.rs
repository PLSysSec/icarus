// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::fmt::{self, Display};
use std::fs;
use std::io::Cursor;
use std::path::{Component, PathBuf};
use std::str;

use anyhow::Error;
use codespan_reporting::files::SimpleFile;
use codespan_reporting::term;
use codespan_reporting::term::termcolor::NoColor;
use insta::assert_snapshot;
use joinery::JoinableIterator;
use lazy_static::lazy_static;
use regex::Regex;
use similar_asserts::assert_eq;

use bpl::ast::BoogieProgram;
use bpl::parser::{ParseError, parse_boogie_program};

include!(concat!(env!("OUT_DIR"), "/test.rs"));

fn test_program(name: &str, path: &str) -> Result<(), Error> {
    let src = fs::read_to_string(path)?;
    let trimmed_src = src.trim();

    let (report, printed_program) = try_parse_and_print(path, &src)?;
    println!("{report}\n\n[Input - {path}]\n{}", trimmed_src);
    if let Some(printed_program) = printed_program {
        let (new_report, _) = try_parse_and_print(path, &printed_program)?;
        assert_eq!(report, new_report);
    }

    // For the snapshot, when printing the path, make sure, e.g., directory
    // separators get printed consistently between Linux and Windows.
    let snapshot_report = format!(
        "{report}\n\n[Input - {}]\n{}",
        OsIndependentPath(path),
        trimmed_src
    );
    assert_snapshot!(name, snapshot_report);
    Ok(())
}

fn try_parse_and_print(path: &str, src: &str) -> Result<(String, Option<String>), Error> {
    let parsed_program = match parse_boogie_program(&src) {
        Ok(parsed_program) => parsed_program,
        Err(parse_error) => {
            let report = format_parse_error(path, &src, &parse_error)?;
            return Ok((report, None));
        }
    };

    let printed_program = format!("{parsed_program}");
    let report = format!(
        "[Parsed]\n{}\n\n[Printed]\n{printed_program}",
        format_program(&parsed_program)
    );
    Ok((report, Some(printed_program)))
}

fn format_parse_error(path: &str, src: &str, parse_error: &ParseError) -> Result<String, Error> {
    let file = SimpleFile::new(path, src);
    let config = term::Config::default();
    let diagnostic = parse_error.build_diagnostic();

    let mut buf = Cursor::new(Vec::<u8>::new());
    term::emit(&mut NoColor::new(&mut buf), &config, &file, &diagnostic)?;
    let buf = buf.into_inner();

    let parse_error_str = format!("[Parse Error]\n{}", str::from_utf8(&buf)?.trim());
    Ok(parse_error_str.to_owned())
}

fn format_program(program: &BoogieProgram) -> String {
    let program_str = format!("{program:#?}");
    lazy_static! {
        static ref SPANNED_RE: Regex = Regex::new(r" @ \[[0-9]+, [0-9]+\)").unwrap();
    }
    SPANNED_RE.replace_all(&program_str, "").into_owned()
}

struct OsIndependentPath<'a>(&'a str);

impl Display for OsIndependentPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "{}",
            PathBuf::from(&self.0)
                .components()
                .map(OsIndependentComponent)
                .join_with("/")
        )
    }
}

struct OsIndependentComponent<'a>(Component<'a>);

impl Display for OsIndependentComponent<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match &self.0 {
            Component::Prefix(prefix) => write!(f, "{}", prefix.as_os_str().to_string_lossy())?,
            Component::RootDir => (),
            Component::CurDir => write!(f, ".")?,
            Component::ParentDir => write!(f, "..")?,
            Component::Normal(segment) => write!(f, "{}", segment.to_string_lossy())?,
        }
        Ok(())
    }
}
