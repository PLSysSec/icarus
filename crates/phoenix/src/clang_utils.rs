use clang::{Entity, EntityKind, Index, TranslationUnit, diagnostic::Diagnostic};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// `<root>/lib/clang/<version>` — the directory holding clang's builtin headers.
fn resource_dir(root: &Path) -> Option<PathBuf> {
    let dir = std::fs::read_dir(root.join("lib/clang")).ok()?;
    let mut versions: Vec<PathBuf> = dir.flatten().map(|e| e.path()).collect();
    versions.sort();
    versions.pop()
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

/// `Class::method` form, ignoring namespaces (they're all `js::jit` here).
pub fn qualified_name(e: Entity) -> String {
    let name = e.get_name().unwrap_or_default();
    match e.get_semantic_parent() {
        Some(p) if matches!(p.get_kind(), EntityKind::ClassDecl | EntityKind::StructDecl) => {
            format!("{}::{}", p.get_name().unwrap_or_default(), name)
        }
        _ => name,
    }
}

/// The file a definition was written in, if it has one.
fn file_of(e: Entity) -> Option<PathBuf> {
    Some(e.get_location()?.get_file_location().file?.get_path())
}

/// Walk top-level declarations (including those nested in namespaces and
/// classes) looking for the method *definition* whose qualified name matches.
///
/// Classes are descended into because a method can be defined inside its class
/// rather than out of line: every `CacheIRWriter` method is, so without this the
/// whole header is invisible.
///
/// A name can have several definitions: `CacheIRCompiler::emitGuardIsNull` is
/// both the real one in `CacheIRCompiler.cpp` and the generated shim taking a
/// `CacheIRReader` that decodes the operands and calls it. So a definition in
/// `source` wins, that being the file the caller asked about; anything else is a
/// fallback for when the symbol lives somewhere unexpected.
pub fn find_definition<'tu>(
    root: Entity<'tu>,
    qualified: &str,
    source: &Path,
) -> Option<Entity<'tu>> {
    let mut preferred = None;
    let mut fallback = None;
    root.visit_children(|e, _| {
        use clang::EntityVisitResult::*;
        match e.get_kind() {
            EntityKind::Namespace | EntityKind::ClassDecl | EntityKind::StructDecl => {
                return Recurse;
            }
            EntityKind::Method | EntityKind::FunctionDecl if e.is_definition() => {
                if qualified_name(e) == qualified {
                    if file_of(e).is_some_and(|path| path.ends_with(source)) {
                        preferred = Some(e);
                        return Break;
                    }
                    fallback.get_or_insert(e);
                }
            }
            _ => {}
        }
        Continue
    });
    preferred.or(fallback)
}

// TODO: Fail gracefully
pub fn parse_file<'a>(index: &'a Index, source: &Path, db: &Path) -> TranslationUnit<'a> {
    let (_, source, flags) = compile_args(db, source);
    index
        .parser(&source)
        .arguments(&flags)
        .skip_function_bodies(false)
        .parse()
        .expect("parse translation unit")
}

pub fn get_errors<'tu>(translation_unit: &'tu TranslationUnit) -> Vec<Diagnostic<'tu>> {
    translation_unit
        .get_diagnostics()
        .into_iter()
        .filter(|d| d.get_severity() >= clang::diagnostic::Severity::Error)
        .collect()
}
