// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::fs;
use std::io;
use std::io::prelude::*;
use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use atomicwrites::{AllowOverwrite, AtomicFile};
use codespan_reporting::files::SimpleFile;
use codespan_reporting::term;
use codespan_reporting::term::termcolor::{ColorChoice, StandardStream};
use lazy_static::lazy_static;
use num_bigint::BigUint;
use structopt::StructOpt;
use structopt::clap::AppSettings;

use bpl_tools::{ParseError, inline_program};

/// Preprocess code in the Boogie intermediate verification language to eagerly
/// inline declarations.
#[derive(StructOpt)]
#[structopt(global_settings = &[AppSettings::DeriveDisplayOrder])]
struct Opt {
    /// Input Boogie source file. Specify `-` for stdin.
    #[structopt(parse(from_os_str))]
    input: PathBuf,
    /// Output Boogie source file, after preprocessing. Specify `-` for stdout.
    #[structopt(short, long, parse(from_os_str), required_unless("in-place"))]
    output: Option<PathBuf>,
    /// Overwrite the input file with the preprocessed output. This command will
    /// attempt to make this write atomic, so if parsing of the input fails, it
    /// won't be overwritten. If the input is stdin, this will write to stdout.
    #[structopt(short, long, conflicts_with("output"))]
    in_place: bool,
    /// Inline all procedures and implementations to at least the specified
    /// level.
    #[structopt(short("p"), long("proc"), value_name("level"))]
    procs_level: Option<BigUint>,
}

fn main() -> Result<(), Error> {
    lazy_static! {
        static ref STD_STREAM_PATH: &'static Path = Path::new("-");
    }

    let opt = Opt::from_args();

    let input_is_stdin = opt.input == *STD_STREAM_PATH;
    let src = if input_is_stdin {
        io::read_to_string(io::stdin())
    } else {
        fs::read_to_string(&opt.input)
    }
    .with_context(|| format!("Failed to read {}", opt.input.display()))?;

    let preprocessed_src = match inline_program(&src, opt.procs_level.as_ref()) {
        Ok(preprocessed_src) => preprocessed_src,
        Err(parse_error) => {
            report_parse_error(&opt.input, &src, &parse_error)?;
            return Err(Error::msg(format!(
                "Failed to parse {}",
                opt.input.display()
            )));
        }
    };

    let output_is_stdout = opt
        .output
        .as_ref()
        .is_some_and(|output_path| output_path == *STD_STREAM_PATH);
    if output_is_stdout || (input_is_stdin && opt.in_place) {
        print!("{}", preprocessed_src);
    } else {
        let output_path = if opt.in_place {
            &opt.input
        } else {
            &opt.output.as_ref().expect("missing output path")
        };
        AtomicFile::new(output_path, AllowOverwrite)
            .write(|output_file| write!(output_file, "{}", preprocessed_src))
            .with_context(|| format!("Failed to write {}", output_path.display()))?;
    }

    Ok(())
}

fn report_parse_error(
    path: impl AsRef<Path>,
    src: &str,
    parse_error: &ParseError,
) -> Result<(), Error> {
    let file = SimpleFile::new(path.as_ref().to_string_lossy(), src);
    let diagnostic = parse_error.build_diagnostic();
    let config = term::Config::default();
    let writer = StandardStream::stderr(ColorChoice::Auto);
    term::emit(&mut writer.lock(), &config, &file, &diagnostic)?;
    Ok(())
}
