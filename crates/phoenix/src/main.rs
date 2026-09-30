//! Dump the fully type-resolved clang AST of a single CacheIR stub generator.
//!
//! Usage:
//!   phoenix <Class::method> [--db compile_commands.json] [--source file.cpp]
//!           [--calls] [--depth N] [--subset]
//!
//! `--subset` dumps the generator lowered into the modeled C++ subset
//! (`cpp_subset::MethodDef`) instead of the raw clang AST.
//!
//! `--db` defaults to the compile database that build.rs generated, and
//! `--source` to js/src/jit/CacheIR.cpp.
//!
//! The compile database comes from a configured SpiderMonkey objdir
//! (`mach configure && mach build pre-export export && mach build-backend -b CompileDB`).
//! We reuse the exact flags mozbuild would compile the file with, so every
//! `writer.foo(...)` call resolves to its `CacheIRWriter` method declaration and
//! every operand id carries its concrete `ObjOperandId` / `ValOperandId` type.
//!
//! libclang is loaded at runtime from the toolchain `mach bootstrap` installs
//! (`~/.mozbuild/clang`), so the compile flags and the parser always come from
//! the same self-consistent toolchain. Set `LIBCLANG_PATH` to override.

use clang::{Clang, Entity, EntityKind, Index};
use clap::{Parser, Subcommand, ValueEnum};
use phoenix::clang_utils::{find_definition, get_errors, parse_file, qualified_name};
use std::path::{Path, PathBuf};

fn dump(e: Entity, depth: usize) {
    let indent = "  ".repeat(depth);
    let mut line = format!("{indent}{:?}", e.get_kind());
    if let Some(n) = e.get_name() {
        line.push_str(&format!(" `{n}`"));
    }
    if let Some(t) = e.get_type() {
        line.push_str(&format!(" : {}", t.get_display_name()));
    }
    // The resolved declaration behind a reference or call — this is the
    // information a syntax-only parser can't give us.
    if let Some(r) = e.get_reference() {
        if r != e {
            let loc = r
                .get_location()
                .and_then(|l| l.get_file_location().file.map(|f| f.get_path()))
                .map(|p| {
                    p.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            line.push_str(&format!(
                "  => {:?} {} [{loc}]",
                r.get_kind(),
                r.get_display_name().unwrap_or_default()
            ));
        }
    }
    if let Some(lit) = literal_text(e) {
        line.push_str(&format!(" = {lit}"));
    }
    println!("{line}");
    for c in e.get_children() {
        dump(c, depth + 1);
    }
}

/// Spell out literal tokens so numbers/strings are readable in the dump.
fn literal_text(e: Entity) -> Option<String> {
    match e.get_kind() {
        EntityKind::IntegerLiteral
        | EntityKind::FloatingLiteral
        | EntityKind::StringLiteral
        | EntityKind::CharacterLiteral
        | EntityKind::BoolLiteralExpr => {
            let range = e.get_range()?;
            let toks = range.tokenize();
            Some(
                toks.iter()
                    .map(|t| t.get_spelling())
                    .collect::<Vec<_>>()
                    .join(""),
            )
        }
        _ => None,
    }
}

/// `file:line` of an entity, with just the basename.
fn short_loc(e: Entity) -> String {
    e.get_location()
        .map(|l| {
            let f = l.get_file_location();
            let name = f
                .file
                .map(|f| f.get_path())
                .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
                .unwrap_or_default();
            format!("{name}:{}", f.line)
        })
        .unwrap_or_default()
}

/// `ret name(params)` with fully resolved parameter types. Note this is the
/// *canonical* type for each parameter (typedefs/aliases stripped), which is
/// what a translator needs to key on.
fn signature(f: Entity) -> String {
    let params: Vec<String> = f
        .get_arguments()
        .unwrap_or_default()
        .iter()
        .map(|p| {
            let ty = p
                .get_type()
                .map(|t| t.get_canonical_type().get_display_name())
                .unwrap_or_default();
            format!("{ty} {}", p.get_name().unwrap_or_default())
        })
        .collect();
    let ret = f
        .get_result_type()
        .map(|t| t.get_canonical_type().get_display_name())
        .unwrap_or_default();
    format!("{} {}({})", ret, qualified_name(f), params.join(", "))
}

/// Everything called from `f`'s body, in source order, deduplicated:
/// free functions, methods and template method instantiations. Constructors
/// and operators are elided — they are implicit-conversion noise here, not
/// semantics a stub generator expresses.
fn direct_callees<'tu>(f: Entity<'tu>) -> Vec<Entity<'tu>> {
    let mut out: Vec<Entity<'tu>> = Vec::new();
    f.visit_children(|e, _| {
        if e.get_kind() == EntityKind::CallExpr {
            if let Some(callee) = e.get_reference() {
                let is_fn = matches!(
                    callee.get_kind(),
                    EntityKind::FunctionDecl | EntityKind::Method | EntityKind::FunctionTemplate
                );
                let name = callee.get_name().unwrap_or_default();
                if is_fn && !name.starts_with("operator") && !out.contains(&callee) {
                    out.push(callee);
                }
            }
        }
        clang::EntityVisitResult::Recurse
    });
    out
}

/// Only follow definitions that live in the CacheIR machinery itself; the
/// rest of the engine (NativeObject, Shape, ...) is the boundary a stub
/// generator queries but doesn't implement.
fn is_cacheir_file(e: Entity) -> bool {
    e.get_location()
        .and_then(|l| l.get_file_location().file.map(|f| f.get_path()))
        .map(|p| {
            let p = p.to_string_lossy();
            p.contains("/js/src/jit/CacheIR")
        })
        .unwrap_or(false)
}

/// Print the call graph rooted at `f`, expanding callees defined in CacheIR
/// sources up to `max_depth` levels.
fn call_graph(f: Entity, depth: usize, max_depth: usize, seen: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    let sig = signature(f);
    let def = f.get_definition();
    let status = match def {
        Some(d) if is_cacheir_file(d) => "",
        Some(_) => "  [defined outside CacheIR]",
        None => "  [declaration only]",
    };
    let already = seen.contains(&sig);
    println!(
        "{indent}{sig}  @ {}{status}{}",
        short_loc(def.unwrap_or(f)),
        if already { "  (see above)" } else { "" }
    );
    if already || depth >= max_depth {
        return;
    }
    seen.push(sig);
    let Some(def) = def.filter(|d| is_cacheir_file(*d)) else {
        return;
    };
    for callee in direct_callees(def) {
        call_graph(callee, depth + 1, max_depth, seen);
    }
}

/// Default to the vendored clang that `mach bootstrap` installs unless the
/// caller pointed us elsewhere. clang-sys reads `LIBCLANG_PATH` when loading.
fn select_libclang() {
    if std::env::var_os("LIBCLANG_PATH").is_some() {
        return;
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mozbuild = std::env::var_os("MOZBUILD_STATE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".mozbuild"));
    let lib = mozbuild.join("clang/lib");
    if lib.is_dir() {
        // SAFETY: single-threaded, before any thread is spawned.
        unsafe { std::env::set_var("LIBCLANG_PATH", &lib) };
    }
}

/// Every mode needs the same plumbing, so `--db` and `--source` are global.
#[derive(Parser)]
#[command(
    name = "phoenix",
    about = "Translate a CacheIR stub generator to Cachet, or inspect it on the way."
)]
struct Opt {
    /// Compile database to take the parse flags from. Defaults to the objdir
    /// build.rs set up.
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// Source file the symbol is defined in, matched as a path suffix.
    #[arg(long, global = true, default_value = "js/src/jit/CacheIR.cpp")]
    source: PathBuf,

    #[command(subcommand)]
    cmd: Cmd,
}

/// What kind of C++ a symbol is, and so which Cachet item it becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Unit {
    /// A `*IRGenerator::tryAttach*` method, which becomes an `ir` holding one
    /// `op`, plus every helper it calls.
    Generator,
    /// A free function the generators call, which becomes a top-level `fn`,
    /// plus every helper it calls.
    Function,
    /// A method translated as a top-level `fn`, its class deciding what is
    /// ambient -- a `CacheIRWriter` wrapper being the case that works today.
    Method,
    /// A `CacheIRCompiler::emit*` method, which becomes an `op` in `ir CacheIR`
    /// whose signature comes from `CacheIROps.yaml` rather than from the C++.
    Instruction,
}

#[derive(Subcommand)]
enum Cmd {
    /// Translate to Cachet.
    Cachet {
        /// `Class::method`, or a bare function name.
        symbol: String,
        /// What the symbol is, which decides what it becomes.
        ///
        /// Stated rather than sniffed: the entity kind distinguishes a method
        /// from a function, but a stub generator and an instruction are both
        /// methods and become different things.
        #[arg(long, value_enum, default_value_t = Unit::Generator)]
        unit: Unit,
        /// Write here instead of standard output.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Directory the generated `import`s point at, relative to where the
        /// module is written. The default suits `notes/stubs/`; output kept
        /// elsewhere needs its own, e.g. `../../notes`.
        #[arg(long, default_value = "..")]
        imports: PathBuf,
    },
    /// Dump the generator lowered into the modeled C++ subset.
    Subset { symbol: String },
    /// Dump the raw, fully type-resolved clang AST.
    Ast { symbol: String },
    /// Dump the call graph, following definitions inside the CacheIR sources.
    Calls {
        symbol: String,
        #[arg(long, default_value_t = 3)]
        depth: usize,
    },
}

impl Cmd {
    fn symbol(&self) -> &str {
        match self {
            Cmd::Cachet { symbol, .. }
            | Cmd::Subset { symbol }
            | Cmd::Ast { symbol }
            | Cmd::Calls { symbol, .. } => symbol,
        }
    }
}

fn main() {
    let opt = Opt::parse();
    let symbol = opt.cmd.symbol();

    let Some(db) = opt
        .db
        .clone()
        .or_else(|| option_env!("PHOENIX_COMPILE_DB").map(PathBuf::from))
    else {
        eprintln!(
            "no compile database: pass --db, or build with the objdir setup \
             (this binary was built with PHOENIX_SKIP_SETUP)"
        );
        std::process::exit(2);
    };

    select_libclang();
    let clang = Clang::new().expect("load libclang");
    let index = Index::new(
        &clang, /* exclude_pch_decls */ false, /* diagnostics */ true,
    );
    let tu = parse_file(&index, &opt.source, &db);
    let errors = get_errors(&tu);

    for d in &errors {
        eprintln!("{}", d);
    }
    if !errors.is_empty() {
        eprintln!(
            "{} error(s) while parsing; AST may be incomplete",
            errors.len()
        );
    }

    let Some(def) = find_definition(tu.get_entity(), symbol, &opt.source) else {
        eprintln!("definition of {symbol} not found");
        std::process::exit(1);
    };

    match &opt.cmd {
        Cmd::Cachet {
            out,
            imports,
            unit,
            ..
        } => {
            if *unit == Unit::Instruction {
                match phoenix::cpp_to_cachet::translate_cacheir_op_and_helpers(&def) {
                    Ok((op, helpers, gaps)) => {
                        // Helpers first, as the generator path orders them: what a
                        // definition needs comes before the definition.
                        let mut text: String = helpers
                            .iter()
                            .map(|helper| format!("{}\n", helper.value))
                            .collect();
                        text.push_str(&format!("{}\n", cachet_lang::parser::Item::Op(op)));
                        write_out(out.as_deref(), &text);
                        report_gaps(symbol, &gaps);
                    }
                    Err(e) => {
                        eprintln!("{}", e.report(&symbol));
                        std::process::exit(1);
                    }
                }
            } else if *unit != Unit::Generator {
                // A free function has no class, so nothing is ambient; a method
                // carries its class, which decides what is.
                // Each arm reports for itself, since extraction knows which unit
                // failed and the translator does not.
                let translated = phoenix::cpp_to_cachet::load_ops()
                    .map_err(|e| format!("cannot translate {symbol}: {e}"))
                    .and_then(|ops| {
                        let (class, fn_def) = match unit {
                            Unit::Function => (
                                None,
                                phoenix::cpp_subset::get_fn_def(&def)
                                    .map_err(|e| e.report(symbol))?,
                            ),
                            _ => {
                                let m = phoenix::cpp_subset::get_method_def(&def)
                                    .map_err(|e| e.report(symbol))?;
                                (Some(m.class), m.def)
                            }
                        };
                        phoenix::cpp_to_cachet::translate_fn_and_transitive_callees(
                            &ops, class, fn_def,
                        )
                        .map_err(|e| format!("cannot translate {symbol}: {e}"))
                    });
                match translated {
                    Ok((items, gaps)) => {
                        let text: String = items
                            .iter()
                            .map(|item| format!("{}\n", item.value))
                            .collect();
                        write_out(out.as_deref(), &text);
                        report_gaps(symbol, &gaps);
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
            } else {
                // The generator plus every helper it calls.
                match phoenix::cpp_to_cachet::translate_generator(&def, imports) {
                    Ok(translation) => {
                        write_out(out.as_deref(), &format!("{}\n", translation.module));
                        // Nonzero on a partial translation, so a shell chain
                        // can't go on to compile and verify one by accident.
                        eprintln!("{}", translation.summary());
                        if !translation.is_faithful() {
                            for gap in translation.failures() {
                                eprintln!("  failed: {gap}");
                            }
                            std::process::exit(1);
                        }
                    }
                    Err(e) => {
                        eprintln!("{}", e.report(&symbol));
                        std::process::exit(1);
                    }
                }
            }
        }
        Cmd::Subset { .. } => {
            print_location(&def, symbol, &opt.source);
            // A method is a stub generator; a free function is a helper the
            // generators call.
            let extracted = if def.get_kind() == EntityKind::FunctionDecl {
                phoenix::cpp_subset::get_fn_def(&def).map(|f| f.to_string())
            } else {
                phoenix::cpp_subset::get_method_def(&def).map(|m| m.to_string())
            };
            match extracted {
                Ok(text) => print!("{text}"),
                Err(e) => {
                    // `cpp_subset::Error` already names the unit or the location.
                    eprintln!("cannot extract subset: {e}");
                    std::process::exit(1);
                }
            }
        }
        Cmd::Ast { .. } => {
            print_location(&def, symbol, &opt.source);
            dump(def, 0);
        }
        Cmd::Calls { depth, .. } => {
            print_location(&def, symbol, &opt.source);
            call_graph(def, 0, *depth, &mut Vec::new());
        }
    }
}

/// The `// <symbol> at <file>:<line>` header the dumps carry. Omitted for
/// `cachet`, whose output has to stay compilable.
fn print_location(def: &Entity, symbol: &str, source: &Path) {
    if let Some(loc) = def.get_location() {
        let l = loc.get_file_location();
        println!("// {symbol} at {}:{}", source.display(), l.line);
    }
}

/// The verdict for a single `fn`, which has no module header to carry it.
fn report_gaps(symbol: &str, gaps: &[phoenix::cpp_to_cachet::Gap]) {
    use phoenix::cpp_to_cachet::Fidelity;
    let failed: Vec<_> = gaps
        .iter()
        .filter(|g| g.fidelity == Fidelity::Failed)
        .collect();
    let elided = gaps.len() - failed.len();
    if failed.is_empty() {
        eprintln!("{symbol}: complete, {elided} elided");
        return;
    }
    eprintln!("{symbol}: PARTIAL, {} failed, {elided} elided", failed.len());
    for gap in failed {
        eprintln!("  failed: {gap}");
    }
    std::process::exit(1);
}

fn write_out(out: Option<&Path>, text: &str) {
    match out {
        Some(path) => std::fs::write(path, text)
            .unwrap_or_else(|e| panic!("write {}: {e}", path.display())),
        None => print!("{text}"),
    }
}
