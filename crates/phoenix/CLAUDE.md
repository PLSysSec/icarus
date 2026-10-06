# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

phoenix translates SpiderMonkey's CacheIR stub generators and instructions from C++
into Cachet, so that what gets verified is derived from the engine rather than
written by hand. `README.md` covers setup, env overrides and the full CLI; this file
covers what is hard to rediscover.

## Commands

```sh
cargo build -p phoenix          # -p is required; the workspace has several binaries
cargo test -q -p phoenix
cargo test -p phoenix a_claim   # one test, by substring

# translate: a generator, one instruction, a free function
cargo run -p phoenix -- cachet 'CompareIRGenerator::tryAttachInt32'
cargo run -p phoenix -- cachet --unit instruction \
  --source js/src/jit/CacheIRCompiler.cpp 'CacheIRCompiler::emitGuardIsNull'
cargo run -p phoenix -- cachet --unit function 'CanConvertToInt32ForToNumber'

# inspect: the modeled C++ subset, the raw clang AST, the call graph
cargo run -p phoenix -- subset 'CompareIRGenerator::tryAttachInt32'
cargo run -p phoenix -- ast 'CompareIRGenerator::tryAttachInt32'

# translate, compile and verify; one line per stub
./scripts/translate-verify.sh CompareIRGenerator
```

To check generated Cachet by hand, copy `notes/*.cachet` somewhere writable, write
the module beside them with `--imports .`, and run the compiler directly:

```sh
cargo run --bin cachet-compiler -- gen.cachet \
  --cpp-decls g.h --cpp-defs g.inc --bpl g.bpl
```

## Pipeline

clang AST → `cpp_subset` (a small, explicitly modeled C++) → `cpp_to_cachet` →
`cachet_lang::parser` AST → printed as text.

Build arguments with `cachet_utils::to_arg`, never `Arg::Expr` directly. A bare name
and a field access in argument position are *ambiguous* in Cachet — either could be a
label — and the grammar defers the choice to name resolution, which knows the callee's
signature. phoenix does not, since it never reads the model. `to_arg` reproduces that
normalization, so the AST phoenix builds is the one the parser would have built, and
nothing depends on the output being re-read.

For the same reason, mind which expression statement you build. `Stmt::Semi(expr)`
carries a trailing semicolon and discards the value; `Stmt::Expr(expr)` carries none,
which the grammar permits only for a block or an `if`. A call wants `Semi` — without
the semicolon it does not parse. The two hold the same payload, so a mix-up is invisible
to the compiler and shows up as generated Cachet that `cachet-compiler` cannot read.

Two failure vocabularies, and the messages say which:

| | means | where |
|---|---|---|
| `Unsupported` | the C++ never reached the subset | `cpp_subset` |
| `Unhandled` | it did, and the model has no counterpart | `cpp_to_cachet` |

`Fidelity` grades a gap: `Elided` (sound to drop), `Failed` (phoenix cannot express
it), `Invalid` (the C++ breaks an invariant both languages rely on — likelier a
misreading on our side, which is why it is reported rather than asserted). Anything
but `Elided` blocks verification.

## Modules

- `cacheir_ops` — `CacheIROps.yaml` is the authority on what a CacheIR op *is*:
  operand names, types, which are stub fields, the writer-method naming rules, and
  wrapper synthesis. `OpIr` picks the `ir` generated ops live in (`CacheIROps`) or
  the hand-written one (`CacheIR`, via `--model-ops`).
- `masm_ops` — the MacroAssembler correspondence. No yaml exists for masm, so this
  is a hand-written table; `MasmStmt` lets one C++ call become several statements, or
  a `bind` rather than an `emit`.
- `scopes` — per-block facts the C++ does not state: what kind of value a register
  holds, and outstanding failure paths as `Obligation`s discharged on return or at
  scope end.
- `names` — every C++ name to its Cachet identifier, decided per unit so a
  keyword-mangled name cannot land on one already in use.
- `cachet_utils` — walks phoenix's *own output*; `emitted_ops` reads the op set a
  generator needs off the emits it produced.
- `cpp_to_cachet` — the translator. `translate_known_expr`, `translate_known_stmt` and
  `translate_known_block` are the special-case boundary: an idiom lives in one of the
  three, a plain C++ form lives in the ordinary match. Keep it that way.

A generator and its instructions are in **different translation units** —
`CacheIR.cpp` lands in `Unified_cpp_js_src_jit2.cpp`, `CacheIRCompiler.cpp` in
`..jit3.cpp` — so the emitters are parsed separately (`--instruction-source`).

## Working rules

**The C++ is ground truth.** `notes/*.cachet` is a careful hand-written model, but it
was written against an older Firefox and can be dated. When the two disagree, suspect
the model first and check the C++.

**Refuse rather than guess.** A wrong mapping verifies something other than the code
that runs, which is worse than not verifying. Every refusal becomes a gap with a
span, and the output carries a `DO NOT VERIFY` header.

**Ask whether a mistake would be loud.** The recurring hazard is a name resolving to
the wrong thing rather than to nothing: Cachet accepts shadowing silently, so a
rename that collides is invisible, while an unknown qualified name (`JSOp::NotAThing`)
is rejected outright. A derived rule is fine where a miss is loud; where it is silent,
use a table and check the collision.

**Measure instead of asserting.** Several design decisions here rest on counts taken
from the source — 18/18 stub-field arg types end in `Field`, 514 of 522
`allocator.use*`/`define*` calls are declaration initializers, 27 arg types map to 27 distinct
Cachet types. Write the number and where it came from into the doc comment.

**A `complete` verdict does not mean it compiles.** The fidelity report measures what
the walk handled, not whether the result type checks. Run `cachet-compiler` before
claiming a translation works.

`notes/` — the hand-written model at the repo root — is not to be edited.

`docs/` holds the design record: what is deferred and why, with the measurements
behind each decision. `docs/README.md` indexes it. Read `docs/next-steps.md` first
after a compaction — it is a dated snapshot of what works, what blocks it, and what is
left. Add to these rather than re-deriving, and say so when one goes stale.

## How this is developed

**Discuss before changing.** Most work here happens a construct at a time, with the
design settled in conversation first. Expect to explain what a change does and why,
and to be asked about it — "is that actually allowed?", "what does the C++ do?" —
before anything is written. A change to a table's shape, a function signature, or
where a decision lives is worth raising rather than just making.

**Settle questions by experiment.** Several designs here were decided by running a
three-line `.cachet` file through `cachet-compiler`: whether an `ir` can be declared
twice, whether an `impl` can extend one, whether a reserved word parses as a parameter
name, whether shadowing is accepted. That is faster and more reliable than reasoning
about the language, and the answer belongs in a doc comment afterwards.

Prefer the `Edit` tool over `sed`/`python` for file changes, unless auto mode is on
and asks for the shell.

**Propose fixes when this file drifts.** When the code no longer matches this file or
`README.md` — a renamed flag, a module whose job has moved, an invariant that no
longer holds — say so and offer the edit. Both are read fresh after a compaction, so a
stale claim here is worse than a missing one.
