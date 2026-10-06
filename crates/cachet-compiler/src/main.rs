// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::fs::File;
use std::io::prelude::*;
use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use codespan_reporting::term;
use codespan_reporting::term::termcolor::{ColorChoice, StandardStream};
use iterate::iterate;
use structopt::StructOpt;
use structopt::clap::AppSettings;

use cachet_lang::FrontendError;
use cachet_lang::flattener::flatten;
use cachet_lang::normalizer::normalize;
use cachet_lang::parser::{Files, ParseError, Parser};
use cachet_lang::resolver::{ResolveErrors, resolve};
use cachet_lang::type_checker::{TypeCheckErrors, type_check};

use cachet_compiler::{compile_bpl, compile_cpp};

// TODO(spinda): Change to this format for specifying compiler outputs:
//
//   cachet-compiler foo.cachet \
//      --cpp-decls foo.h       \
//      --cpp-defs foo.inc      \
//      --bpl foo.bpl

/// The Cachet compiler.
#[derive(StructOpt)]
#[structopt(global_settings = &[AppSettings::DeriveDisplayOrder])]
struct Opt {
    /// Input source file (typically ending in .cachet).
    #[structopt(parse(from_os_str))]
    input: PathBuf,
    /// Output file for C++ declarations (typically ending in .h or .hpp).
    #[structopt(long, parse(from_os_str))]
    cpp_decls: Option<PathBuf>,
    /// Output file for C++ definitions (typically ending in .inc or .cpp).
    #[structopt(long, parse(from_os_str))]
    cpp_defs: Option<PathBuf>,
    /// Output file for Boogie verification code (typically ending in .bpl).
    #[structopt(long, parse(from_os_str))]
    bpl: Option<PathBuf>,

    /// Skip writing output files.
    #[structopt(long)]
    dry_run: bool,
    /// Dump parser output.
    #[structopt(long)]
    dump_parser: bool,
    /// Dump name resolver output.
    #[structopt(long)]
    dump_resolver: bool,
    /// Dump type checker output.
    #[structopt(long)]
    dump_type_checker: bool,
    /// Dump normalizer output.
    #[structopt(long)]
    dump_normalizer: bool,
    /// Dump flattener output.
    #[structopt(long)]
    dump_flattener: bool,
}

fn main() -> Result<(), Error> {
    let opt = Opt::from_args();

    let mut parser = Parser::new();
    let prelude = parser.parse_str("<prelude>", cachet_lang::PRELUDE);
    let main = parser.parse_file(&opt.input);
    let main_err = match main.err() {
        Some(e) => match e.downcast::<ParseError>() {
            Ok(parse_error) => Some(parse_error),
            Err(e) => Err(e)?,
        },
        None => None,
    };

    if prelude.is_err() || main_err.is_some() {
        let errors: Vec<_> =
            iterate![..prelude.err().into_iter(), ..main_err.into_iter()].collect();
        report_all(&parser.files, errors.iter());

        let first_error_file_name = Path::new(parser.files.name(errors[0].file_id()));
        return Err(Error::msg(format!(
            "Failed to parse {}",
            first_error_file_name.display()
        )));
    }

    let mod_ = parser.mod_;
    if opt.dump_parser {
        println!("=== PARSER ===\n\n{:#?}\n\n", mod_);
    }

    let env = match resolve(mod_) {
        Ok(env) => env,
        Err(ResolveErrors(errors)) => {
            report_all(&parser.files, errors.iter());
            return Err(Error::msg(format!(
                "Failed to resolve names in {}",
                opt.input.display()
            )));
        }
    };
    if opt.dump_resolver {
        println!("=== RESOLVER ===\n\n{:#?}\n\n", env);
    }

    let env = match type_check(env) {
        Ok(env) => env,
        Err(TypeCheckErrors(errors)) => {
            report_all(&parser.files, errors.iter());
            return Err(Error::msg(format!(
                "Failed to type check {}",
                opt.input.display()
            )));
        }
    };
    if opt.dump_type_checker {
        println!("=== TYPE CHECKER ===\n\n{:#?}\n\n", env);
    }

    let env = normalize(env);
    if opt.dump_normalizer {
        println!("=== NORMALIZER ===\n\n{:#?}\n\n", env);
    }

    if opt.cpp_decls.is_some() || opt.cpp_defs.is_some() {
        let cpp_compiler_output = compile_cpp(&env);

        if !opt.dry_run {
            if let Some(cpp_decls_path) = opt.cpp_decls {
                write!(
                    File::create(&cpp_decls_path)
                        .with_context(|| format!("Failed to open {}", cpp_decls_path.display()))?,
                    "{}",
                    cpp_compiler_output.decls
                )
                .with_context(|| format!("Failed to write {}", cpp_decls_path.display()))?;
            }

            if let Some(cpp_defs_path) = opt.cpp_defs {
                write!(
                    File::create(&cpp_defs_path)
                        .with_context(|| format!("Failed to open {}", cpp_defs_path.display()))?,
                    "{}",
                    cpp_compiler_output.defs
                )
                .with_context(|| format!("Failed to write {}", cpp_defs_path.display()))?;
            }
        }
    }

    let env = flatten(env);
    if opt.dump_flattener {
        println!("=== FLATTENER ===\n\n{:#?}\n\n", env);
    }

    if let Some(bpl_path) = opt.bpl {
        let bpl_code = compile_bpl(&env);

        if !opt.dry_run {
            write!(
                File::create(&bpl_path)
                    .with_context(|| format!("Failed to open {}", bpl_path.display()))?,
                "{}",
                bpl_code
            )
            .with_context(|| format!("Failed to write {}", bpl_path.display()))?;
        }
    }

    Ok(())
}

fn report_all<'a, E: 'a + FrontendError>(files: &Files, errors: impl Iterator<Item = &'a E>) {
    let writer = StandardStream::stderr(ColorChoice::Auto);
    let config = term::Config::default();

    for error in errors {
        let diagnostic = error.build_diagnostic();
        term::emit(&mut writer.lock(), &config, files, &diagnostic).unwrap();
    }
}
