//! The compile database comes from a configured SpiderMonkey objdir
//! (`mach configure && mach build pre-export export && mach build-backend -b CompileDB`).
//! We reuse the exact flags mozbuild would compile the file with, so every
//! `writer.foo(...)` call resolves to its `CacheIRWriter` method declaration and
//! every operand id carries its concrete `ObjOperandId` / `ValOperandId` type.
//!
//! libclang is loaded at runtime from the toolchain `mach bootstrap` installs
//! (`~/.mozbuild/clang`), so the compile flags and the parser always come from
//! the same self-consistent toolchain. Set `LIBCLANG_PATH` to override.

use clang::{Clang, Entity, EntityKind, Index, TranslationUnit, diagnostic::Diagnostic};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct CompileCommand {
    directory: PathBuf,
    file: PathBuf,
    command: String,
}

/// Find the compile command for `source` and turn it into a flag list libclang
/// will accept: drop the compiler binary, output/dep-file options and the input.
fn compile_args(db: &Path, source: &Path) -> (PathBuf, PathBuf, Vec<String>) {
    let text = std::fs::read_to_string(db).expect("read compile_commands.json");
    let entries: Vec<CompileCommand> = serde_json::from_str(&text).expect("parse compile db");
    let entry = entries
        .into_iter()
        .find(|e| e.file.ends_with(source) || e.file == source)
        .unwrap_or_else(|| panic!("no compile command for {}", source.display()));

    let words = shlex::split(&entry.command).expect("shlex compile command");
    let mut args = Vec::new();
    let mut skip_next = false;
    for (i, w) in words.iter().enumerate() {
        if i == 0 || skip_next {
            skip_next = false;
            continue;
        }
        match w.as_str() {
            "-o" | "-MF" | "-MT" | "-MQ" => skip_next = true,
            "-c" | "-MD" | "-MP" | "-MMD" => {}
            w if w.ends_with(".cpp") || w.ends_with(".cc") => {}
            w if w.starts_with("-Werror") => {}
            _ => args.push(w.clone()),
        }
    }
    // libclang doesn't run the driver's language inference; be explicit.
    args.splice(0..0, ["-x".to_string(), "c++".to_string()]);
    // Unlike the `clang++` driver, libclang doesn't know where its own
    // toolchain lives, so it neither finds its builtin headers nor the libc++
    // shipped next to it and falls back to the (possibly newer) SDK libc++.
    // Point both at the libclang we're actually loading.
    if let Some(root) = libclang_root() {
        let libcxx = root.join("include/c++/v1");
        if libcxx.is_dir() {
            // Drop the SDK's libc++ entirely, else `#include_next` chains from
            // our wrappers into its (incompatible) wrappers.
            args.push("-nostdinc++".into());
            args.push("-isystem".into());
            args.push(libcxx.to_string_lossy().into_owned());
        }
        if let Some(res) = resource_dir(&root) {
            args.push(format!("-resource-dir={}", res.display()));
        }
    }
    // Only diagnose real errors; the tree's warning set is noisy and irrelevant here.
    args.push("-w".into());
    // Escape hatch for toolchain quirks (e.g. `-v`, `-resource-dir`, `-isystem`).
    if let Ok(extra) = std::env::var("PHOENIX_CLANG_ARGS") {
        args.extend(shlex::split(&extra).unwrap_or_default());
    }
    (entry.directory, entry.file, args)
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

/// Prefix of the LLVM install that `LIBCLANG_PATH` points into:
/// `<root>/lib/libclang.dylib`.
fn libclang_root() -> Option<PathBuf> {
    let lib = std::env::var_os("LIBCLANG_PATH").map(PathBuf::from)?;
    let lib = if lib.is_file() {
        lib.parent()?.to_path_buf()
    } else {
        lib
    };
    let lib = std::fs::canonicalize(lib).ok()?;
    lib.parent().map(Path::to_path_buf)
}

/// `<root>/lib/clang/<version>` — the directory holding clang's builtin headers.
fn resource_dir(root: &Path) -> Option<PathBuf> {
    let dir = std::fs::read_dir(root.join("lib/clang")).ok()?;
    let mut versions: Vec<PathBuf> = dir.flatten().map(|e| e.path()).collect();
    versions.sort();
    versions.pop()
}

/// Walk top-level declarations (including those nested in namespaces) looking
/// for the method *definition* whose qualified name matches.
fn find_definition<'tu>(root: Entity<'tu>, qualified: &str) -> Option<Entity<'tu>> {
    let mut found = None;
    root.visit_children(|e, _| {
        use clang::EntityVisitResult::*;
        match e.get_kind() {
            EntityKind::Namespace => return Recurse,
            EntityKind::Method | EntityKind::FunctionDecl if e.is_definition() => {
                if qualified_name(e) == qualified {
                    found = Some(e);
                    return Break;
                }
            }
            _ => {}
        }
        Continue
    });
    found
}

/// `Class::method` form, ignoring namespaces (they're all `js::jit` here).
fn qualified_name(e: Entity) -> String {
    let name = e.get_name().unwrap_or_default();
    match e.get_semantic_parent() {
        Some(p) if matches!(p.get_kind(), EntityKind::ClassDecl | EntityKind::StructDecl) => {
            format!("{}::{}", p.get_name().unwrap_or_default(), name)
        }
        _ => name,
    }
}

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

fn parse<'i>(index: &'i Index<'i>, source: &Path, flags: &Vec<String>) -> TranslationUnit<'i> {
    let tu = index
        .parser(&source)
        .arguments(flags)
        .skip_function_bodies(false)
        .parse()
        .expect("parse translation unit");
    tu
}

fn errors<'a>(tu: &'a TranslationUnit<'a>) -> Vec<Diagnostic<'a>> {
    let errors: Vec<_> = tu
        .get_diagnostics()
        .into_iter()
        .filter(|d| d.get_severity() >= clang::diagnostic::Severity::Error)
        .collect();
    errors
}

fn main() {
    // phoenix <Class::method> [--db compile_commands.json] [--source file.cpp]
    //         [--calls] [--depth N]
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut take_flag = |name: &str| -> bool {
        args.iter()
            .position(|a| a == name)
            .map(|i| args.remove(i))
            .is_some()
    };
    let calls = take_flag("--calls");
    let mut take_opt = |name: &str| -> Option<String> {
        let i = args.iter().position(|a| a == name)?;
        args.remove(i);
        Some(args.remove(i))
    };
    let max_depth = take_opt("--depth")
        .map(|d| d.parse::<usize>().expect("--depth N"))
        .unwrap_or(3);
    let db = take_opt("--db").or_else(|| option_env!("PHOENIX_COMPILE_DB").map(String::from));
    let source = take_opt("--source").unwrap_or_else(|| "js/src/jit/CacheIR.cpp".into());
    let (Some(db), [wanted]) = (db, args.as_slice()) else {
        eprintln!(
            "usage: phoenix <Class::method> [--db compile_commands.json] [--source file.cpp] [--calls] [--depth N]\n\
             --db defaults to the objdir build.rs set up ({})",
            option_env!("PHOENIX_COMPILE_DB").unwrap_or("none; built with PHOENIX_SKIP_SETUP")
        );
        std::process::exit(2);
    };
    let db = Path::new(&db);
    let source = Path::new(&source);

    select_libclang();
    let (dir, source, flags) = compile_args(db, source);
    std::env::set_current_dir(&dir).expect("chdir to compile directory");

    let clang = Clang::new().expect("load libclang");
    let index = Index::new(
        &clang, /* exclude_pch_decls */ false, /* diagnostics */ true,
    );

    let tu = parse(&index, &source, &flags);
    let errors = errors(&tu);

    for d in &errors {
        eprintln!("{}", d);
    }
    if !errors.is_empty() {
        eprintln!(
            "{} error(s) while parsing; AST may be incomplete",
            errors.len()
        );
    }

    let Some(def) = find_definition(tu.get_entity(), wanted) else {
        eprintln!("definition of {wanted} not found");
        std::process::exit(1);
    };
    if let Some(loc) = def.get_location() {
        let l = loc.get_file_location();
        println!("// {wanted} at {}:{}", source.display(), l.line);
    }
    if calls {
        call_graph(def, 0, max_depth, &mut Vec::new());
    } else {
        dump(def, 0);
    }
}
