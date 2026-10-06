# phoenix

Dumps the type-resolved clang AST (or call graph) of a CacheIR stub generator,
as a first step toward compiling stub generators to Icarus automatically.

## Setup

```sh
cargo build -p phoenix
```

That's it. `build.rs` does the SpiderMonkey setup on first build and skips it
afterwards:

1. shallow-clones mozilla-central at a pinned revision into
   `../mozilla-central` (sibling of this repo; ~2 GB)
2. `mach bootstrap` — downloads Mozilla's vendored clang + libclang into
   `~/.mozbuild/clang` (~1 GB)
3. `mach configure` — debug JS-shell-only build into `obj-js`
4. `mach build pre-export export` — generates headers `CacheIR.cpp` includes
5. `mach build-backend -b CompileDB` — writes `obj-js/compile_commands.json`
6. `mach build-backend -b Clangd` — writes `obj-js/clangd/`, and a
   `mozilla-central/.clangd` pointing at it (see [Editor setup](#editor-setup))

No C++ is compiled. First build takes ~10 minutes, nearly all download.
Cargo hides build-script output; progress goes to `cargo:warning` lines and the
full log is in `target/debug/build/phoenix-*/out/setup.log`.

Requirements: `git`, and a Python 3.10–3.14 on `PATH` for `mach`.

Overrides:

| variable | effect |
|---|---|
| `PHOENIX_MOZ_CENTRAL` | checkout location |
| `PHOENIX_MOZ_REV` | mozilla-central revision (default pinned in `build.rs`) |
| `PHOENIX_SKIP_SETUP=1` | skip setup entirely; pass `--db` at run time |
| `MOZBUILD_STATE_PATH` | mach's toolchain dir (default `~/.mozbuild`) |
| `LIBCLANG_PATH` | use a different libclang at run time |

### Editor setup

Setup leaves the checkout ready for clangd (go-to-definition, hover types, and
so on in the SpiderMonkey sources). Any clangd-based editor picks up
`mozilla-central/.clangd` on its own, because clangd finds it by walking up from
the file it opens, so it also works with `icarus/` open as the workspace root.
No editor config is written; for VS Code:

1. Install the clangd extension (`llvm-vs-code-extensions.vscode-clangd`), and
   disable Microsoft C/C++ IntelliSense if you have it
   (`"C_Cpp.intelliSenseEngine": "disabled"`).
2. In `icarus/.vscode/settings.json` (gitignored), point it at Mozilla's
   clangd, which matches the vendored clang the compile commands were written
   for, and keep VS Code from crawling the objdir:

   ```jsonc
   {
       "clangd.path": "<home>/.mozbuild/clang-tools/clang-tidy/bin/clangd",
       "files.watcherExclude": { "mozilla-central/obj-js/**": true },
       "search.exclude": { "mozilla-central/obj-js/**": true }
   }
   ```

Don't use `mach ide vscode` here: it writes `mozilla-central/.vscode/`, which VS
Code ignores unless that directory is the workspace root, and without
`MOZCONFIG=mozconfig-phoenix` it configures a full Firefox build instead.

The vendored toolchain for the Firefox-93-era `icarus-firefox` tree has expired
upstream, which is why a current mozilla-central is used as the parsing target.

## Run

Four subcommands, each taking a `Class::method` (or a bare function name).
Note the `-p phoenix`: the workspace has several binaries, so a bare
`cargo run --` can't tell which one you mean.

```sh
# translate a stub generator to Cachet -- the main path
cargo run -p phoenix -- cachet 'CompareIRGenerator::tryAttachInt32'

# ...and write it to a file, with imports relative to that file
cargo run -p phoenix -- cachet 'CompareIRGenerator::tryAttachInt32' \
  --out out/compare-int32.cachet --imports ../notes

# translate a helper the generators call
cargo run -p phoenix -- cachet 'CanConvertToInt32ForToNumber'

# the generator lowered into the modeled C++ subset
cargo run -p phoenix -- subset 'CompareIRGenerator::tryAttachInt32'

# the raw, fully type-resolved clang AST
cargo run -p phoenix -- ast 'InlinableNativeIRGenerator::tryAttachArrayPush'

# call graph, following definitions inside the CacheIR sources 3 levels deep
cargo run -p phoenix -- calls 'SetPropIRGenerator::tryAttachNativeSetSlot' --depth 3
```

`cachet` dispatches on the symbol: a generator method becomes an `ir`, a free
function becomes a `fn`. Its `--imports` is where the generated `import`s point,
relative to `--out`; the default suits `notes/stubs/`.

A generator's module also holds the CacheIR ops it emits, translated into an
`ir CacheIROps` beside the imported `ir CacheIR` — the model's `fn`s are still
imported, only the ops are generated. `--model-ops` leaves them to the
hand-written model instead, so the generator emits into `ir CacheIR` and nothing
below it is translated:

```sh
# the generator, its helpers, and the ops it emits
cargo run -p phoenix -- cachet 'CompareIRGenerator::tryAttachInt32'

# ...against the hand-written ops in notes/cacheir.cachet
cargo run -p phoenix -- cachet 'CompareIRGenerator::tryAttachInt32' --model-ops
```

The ops come from their own translation unit, named by `--instruction-source`
(default `js/src/jit/CacheIRCompiler.cpp`): a generator's unit does not contain
them, and the unified build does not put them together either.

`--source` and `--db` are global and may go before or after the subcommand.
`--source` is matched as a path suffix against the compile database, and
defaults to `js/src/jit/CacheIR.cpp`; reach for it when the symbol lives
elsewhere, such as the machine-code side of an op:

```sh
cargo run -p phoenix -- subset 'CacheIRCompiler::emitCompareDoubleResult' \
  --source js/src/jit/CacheIRCompiler.cpp

cargo run -p phoenix -- ast 'CompareIRGenerator::tryAttachInt32' \
  --db path/to/compile_commands.json
```

`cargo run -p phoenix -- help` lists the subcommands; `help <subcommand>` shows
one in detail.

## Profile

`scripts/translate-verify.sh` translates, compiles and verifies, one line per
stub as it finishes. A bare class name means every `tryAttach*` on it, so one
run says how much translates and how much of that verifies.

```sh
# the whole class
./scripts/translate-verify.sh CompareIRGenerator

# one stub
./scripts/translate-verify.sh 'CompareIRGenerator::tryAttachInt32'

# ...with the compiler and verifier output as it goes
./scripts/translate-verify.sh -v 'CompareIRGenerator::tryAttachInt32'
```

Output lands in `out/phoenix/`, which is gitignored: a partial translation is an
artifact, not source. `PASS` means verified; `PARTIAL` means phoenix marked the
module `DO NOT VERIFY` and it was not compiled; `TRANSLATE`, `COMPILE` and
`VERIFY` name the stage that failed. Exits 1 unless everything passed.

phoenix loads libclang at run time from `~/.mozbuild/clang`, so the parser and
the compile flags come from the same toolchain. `PHOENIX_CLANG_ARGS` appends
extra flags to the parse (e.g. `-v` to print the header search path).
