# Where `CompareIRGenerator` stands, and what is left

Status: written 2026-10-01; updated 2026-10-02 when the first stub verified, and
2026-10-06 after the `if`-expression work. Measured by running the command below, not
from memory.

## Confirmed state

```sh
./scripts/translate-verify.sh CompareIRGenerator
```

**14 stubs: 2 verified, 10 partial, 2 broken.** `tryAttachInt32` and
`tryAttachAnyNullUndefined` both report `PASS … verified` — phoenix translates them
completely, `cachet-compiler` accepts them, and Corral says *"Program has no bugs."* 22
phoenix tests pass.

A verified module holds, in order: imports, the ops' helpers (`JSOpToCondition`,
`EmitStoreBoolean`), `ir CacheIROps` with the ops it emits, the generator's helpers and
wrappers, then `ir CompareIRGenerator`.

Nothing fails to compile or verify any more; every one of the 39 remaining gaps is in
*translation*. The two `broken` stubs fail outright rather than partially, both on
`LambdaExpr` (CacheIR.cpp:15134, :15156).

## The blockers, by leverage

Counting gaps across all 14 stubs. The first two groups are table rows against model ops
that already exist, so they are the cheap ones.

**Five masm rows would clear 13 gaps.** Each is a `branchTest*` on a `ValueOperand` in
an `emitGuardTo*`, and the model has every op:

| C++ | model op | gaps |
|---|---|---|
| `branchTestBigInt` (CacheIRCompiler.cpp:1948) | `BranchTestBigInt` | 4 |
| `branchTestNumber` (:1777, :5419, :5420) | `BranchTestNumber` | 3 |
| `branchTestString` (:1918) | `BranchTestString` | 2 |
| `branchTestBoolean` (:1964) | `BranchTestBoolean` | 2 |
| `branchTestSymbol` (:1933) | `BranchTestSymbol` | 1 |

Key them on the operand shape, not `_`: each has a `*Tag` sibling, and
`emitCompareNullUndefinedResult` already reaches both forms of three such methods. See
`masm-op-overloads.md`.

**Three `translate_method` rows would clear 3 gaps** — `isString`, `isObject`,
`isSymbol` on a `Value`, all present in the model beside the `isBigInt` row added
2026-10-02.

**The rest, roughly by how often it recurs:**

- **The statement-position ternary** (CacheIR.cpp:15112, :15114) — 2 gaps, and the only
  thing between `tryAttachNullUndefined` and a complete translation. Needs the arms to
  become blocks of *statements*; see `nonlocal-transformations.md`.
- **`TypeAliasDecl`** in the three BigInt emitters (CacheIRCompiler.cpp:8894, :8974,
  :9043) — 3 gaps. A `using` declaration inside a function body.
- **`DoubleField` has no writer in the model**, and **no allocator for `NumberId`** — 4
  gaps between them, blocking `tryAttachNumber` and `tryAttachBigIntNumber`. Both are
  model gaps rather than translator gaps.
- **`return emitComparePointerResultShared(..)`** — 2 gaps, refused as "an instruction
  returning anything but `true`". The emitter delegates its whole body to a shared
  helper.
- **`AutoAvailableFloatRegister`** — 2 gaps, the float half of the `Auto*` family; see
  `scope-end-effects.md`.
- **`LambdaExpr`** — the 2 broken stubs. `tryAttachStringNumber` and
  `tryAttachPrimitiveSymbol` pass a lambda to a helper.
- **One-offs:** `masm.compareBigIntAndInt32` (the one needed masm op the model
  *lacks*), `allocator.ensureDoubleRegister`, `emitLoadValueTag`'s `DeclRefExpr`,
  `SameType`'s unreadable `IntegerLiteral` (Value.h:1270), and
  `emitCompareStringResult` having no definition in `CacheIRCompiler.cpp` — it is a
  per-backend emitter.

## How the divergence blocker was resolved

`JSOpToCondition`'s `switch` has a `default: MOZ_CRASH(..)`. That used to translate to
`assert false`, which is a check rather than a return, so a value-returning function
could fall off its end and its return type went unsatisfied.

Cachet now has **`unreachable`** as a statement, and `MOZ_CRASH` translates to it. The
reason string is kept as a comment. It is one statement in every AST from the parser
down, and each pass gives it the obvious meaning:

| pass | behavior |
|---|---|
| type checker | sets `Block::exits_early`, like `return` |
| Boogie | `assert false; return;` — the assert keeps the obligation, the valueless return leaves `ret` unconstrained |
| C++ | `Cachet_Unreachable()`, which the embedder defines as a `[[noreturn]]` crash |
| flow tracer | drains into the exit state and `Break`s, like `Stmt::Ret` |

Two alternatives were considered and rejected. **Reinterpreting `assert false`** as
diverging works, but it reads the control-flow fact out of a syntactic accident, and it
changes the lowering of every existing `assert false` in `notes/` — measured: doing so
makes `op AssumeUnreachable` trip `trace_body`'s assertion. **The default-arm fold** —
moving a switch's last case into the final `else` under an `assert` — was implemented and
reverted earlier; sound, but it hides the obligation in a rewrite.

Things settled along the way, worth not re-deriving:

- `assert false` alone does *not* make a Boogie path dead, but it does not need to.
  If the path is reachable Corral reports the failed assertion; if not, it contributes
  nothing. So no `assume false` is emitted.
- `RetStmt { value: None }` already means "returns unit" (`normalizer.rs:428`), which is
  why `unreachable` is its own statement rather than a valueless return.
- A reachable `unreachable` *does* fail verification — `tests/verifier/fail/unreachable.cachet`
  pins that, so the construct cannot be used as an escape hatch.

## How the ternary blocker was resolved

`?:` had no counterpart: Cachet's `if` was a statement, so there was nowhere for a
conditional *expression* to go. The workaround that looks like it should work is a
silent trap — `let foo = { if c { 1 } else { 2 } };` parses, type checks, and binds
`foo` to **`Unit`**, the arms' values discarded without a word, because `IfStmt::type_`
returned a hardcoded `Unit` instead of reading the arms.

So `if` became an expression (2026-10-06, `a5ec277`), following Rust:

| | |
|---|---|
| `Expr::If` | added; `Stmt::If` removed |
| `Stmt::Block` | removed, generalized into `Stmt::Expr` |
| `Stmt::Expr` | **no** trailing semicolon, must be unit-typed |
| `Stmt::Semi` | **with** a semicolon, value discarded |
| arms | must agree on a type, which discharges `// TODO(spinda)` at type_checker.rs:1271 |
| an arm that diverges | contributes no type, so `if c { 1_i32 } else { unreachable; }` is an `Int32` |
| normalizer | unused → `Stmt::If` as before; used → `let mut tmp; if c { tmp = a } else { tmp = b }; tmp` |

`normalizer::Stmt::If` is unchanged, so the flattener and both backends were untouched.

Three things that bit, worth not rediscovering:

- **`Stmt::Expr` reversed meaning.** It used to be the semicolon form. Nine phoenix
  sites constructing it silently became semicolon-less; the compiler cannot catch this,
  since the variant name and payload type are identical. It surfaced as generated
  Cachet that would not parse.
- **`Stmt::Expr(Expr::Block(..))` printed `({ .. })`**, which is unparseable in
  statement position — `Expr::Block`'s own `Display` parenthesizes, where the old
  `Stmt::Block` held a `KindedBlock` and printed it bare. Fixed with an
  `Unparenthesized` wrapper beside `MaybeGrouped`.
- **A *trailing* `if` is the block's value, not a statement** — the grammar prefers that
  reading, which is what makes `{ .. if c { 1 } else { 2 } }` produce a value. phoenix's
  `walk_block` only iterated `stmts`, so `emitted_ops` found nothing in such a block.
  Caught by an existing test.

## Smaller things, roughly by value

- **4 elided `MOZ_ASSERT`s** remain in the output. Dropping an assertion is sound but
  weakens the proof. None of them is now known to be untranslatable: this entry used
  to claim `MOZ_ASSERT(output.type() == JSVAL_TYPE_BOOLEAN)` in `EmitStoreBoolean` was,
  on the grounds that nothing modelled `ValueTypeFromMIRType`. Something does —
  `JSValueType::fromMIRType` (notes/js.cachet:393), checked case for case against
  IonTypes.h:563 — and `output.type()` translates as of 2026-10-02.
- **`JSValueType::fromMIRType` is modelled where it could be translated.**
  `ValueTypeFromMIRType` is a `static inline` in the TU and its `switch` is within the
  subset, so phoenix could descend into it instead of trusting the model. Worth doing:
  the model's version ends `assert type == MIRType::Object; JSValueType::Object` where
  the C++ ends `default: MOZ_CRASH("bad type")` — the default-arm fold, by hand — so a
  translation would now be *more* faithful than what it is being mapped to. Same goes
  for `AutoOutputRegister::type()` itself, whose `MOZ_ASSERT(!hasValue())`
  `translate_output_type` drops.
- **`left_` / `right_` are renamed for nothing.** `left` and `right` are in
  `names::RESERVED`, but measurement showed `cachet-compiler` accepts them in both
  parameter and `let` position; the other 27 names are refused in both. Only those two
  positions were probed, which is why they are still listed. Probing label/field/op
  positions would settle it, and removing them tidies real output.
- **Register content kinds are seeded but never updated.** `scopes` learns a register's
  kind from the `use*Id`/`define*Id` that bound it, which is enough for `move32`. It
  does not track an op *changing* a register's contents — `CastBoolToInt32` leaves an
  int32 where a bool was, and `scopes` still says bool.
- **`masm_ops`' remaining `_` shape rows don't discriminate.** `branchTestInt32`,
  `branch32` and `fallibleUnboxBoolean` are right only by luck of which overload is
  reached. The `branchTestNull`/`Undefined`/`Object` rows were keyed 2026-10-02 when
  one emitter needed both forms at once; see `masm-op-overloads.md`.
- **`translate_label_ref` and `translate_failure_label` produce expressions**, with
  `to_arg` normalizing afterwards. Argument-level would be tighter, since neither is
  legal anywhere else. Fails loudly if misused, so not urgent.
- **`--unit instruction` output is not valid Cachet standalone** — an `op` needs an
  enclosing `ir`, and that path emits bare items. Fine for inspection.
- **`op_sig` / `op_params` are test-only**, dead on the translation path since
  `translate_cacheir_op` started building its own signature.
- **`scripts/test.sh` does not run as-is**, which is easy to mistake for a passing suite:
  it needs GNU `parallel` (absent on this machine — it reports "0 tests passed, 9 tests
  skipped" rather than failing), and it invokes `cachet-compiler` with positional output
  paths, which the CLI replaced with `--cpp-decls`/`--cpp-defs`/`--bpl`. Run the stages
  directly until it's fixed. Baseline when last measured: 6 of 21 cases fail
  (`cpp/numerics`, `dual/structs`, `frontend/pass/imports`, `verifier/fail/early_return`,
  `verifier/fail/structs`, `verifier/pass/numerics`), all pre-existing.
- **The integer-literal change is stricter than before.** A literal whose C++ type has
  no `translate_type` row is now refused rather than defaulted to `Int32`. A sweep over
  emitters for newly-refused literals was started and abandoned; no known instances.

## Bigger pieces, already discussed

- **The tag register is modelled as 64-bit only.** `ScratchTagScope` exists because the
  tag of a `Value` lives somewhere different per platform: on 64-bit a Value is one
  register, so extracting its tag needs *another* register to be allocated; on 32-bit a
  Value is already a (type, payload) register pair, so the tag is `value.typeReg()` and
  nothing is allocated. The model takes the 64-bit story and goes further, fixing the
  tag register at R11: `CacheIR::allocateScratchReg()` returns `Reg::scratchReg()`
  (notes/cacheir.cachet:1848), which is why `releaseScratchReg()` needs no argument.
  Worth returning to if 32-bit is ever in scope, and worth knowing meanwhile that
  `ScratchTagScopeRelease` — which lends the tag register out for an inner block and
  takes it back — is a *no-op* on 32-bit, so a translation that drops it is right there
  and wrong here.
- **A bottom type (`!`) for Cachet, to replace `Block::exits_early`.** Deferred
  2026-10-05 after being costed; recorded because it is a replacement rather than an
  addition, which is easy to forget.

  An expression that never produces a value has no type today, so the fact is carried
  beside the type as `Block::exits_early: bool`. A `!` that is a *subtype of every
  type* carries it in the type instead, and three things follow:

  - `let x = if c { 1_i32 } else { unreachable; }` gives `x: Int32` with no special
    case — it is ordinary subtyping. The interim measure, landed 2026-10-06, is an
    exemption in `expect_expr_type` for an expression that exits early, plus the
    matching rule in `type_check_if_expr` and `IfExpr::type_`; `!` would delete all
    three. Those last two must stay in step, and each says so — getting them out of
    step is how `if c { unreachable; } else { 1_i32 }` would be rejected by the check
    while `type_` claimed `Int32`.
  - It subsumes the way `unreachable` is encoded. It is a *statement* that sets
    `exits_early` so a value-returning body needs no trailing value; with `!` it would
    be an expression, and the trailing value would simply coerce. That is how Rust
    types `panic!`.
  - `return` could become an expression, as in Rust, where `return e` is `!`. Today it
    is statement-only, which is why `{ return "hi" }` does not parse without a `;`.

  What it replaces, measured: the `exits_early` field (type_checker/ast.rs:299) and both
  its consumers — the body's return-type check (type_checker.rs:322) and the
  normalizer's implicit trailing return (normalizer.rs:153-162). The four producers
  (`does_stmt_exit_early` and friends) stay, rebadged as the rule computing a block's
  type. It does **not** subsume `flow.rs`'s `trace_body` assertion, which reads the
  lowered statement structure and asks a different question.

  What makes it tractable: `!` is uninhabited, so neither backend has a type to emit
  for it. Allow it on expressions and in coercion but **refuse a local or parameter
  typed `!`**, and a backend never sees one. Rust permits them and leans on MIR to drop
  the dead code; nothing here needs them, and refusing turns
  `let x = if c { return 1; } else { return 2; };` into a clean error rather than a
  silent `Unit`.

  Cost: a `BuiltInType` variant against 93 `BuiltInType::` references to audit, a
  subtyping rule in `is_same_type`/`find_upcast_route` — which today handle struct
  supertypes and numeric casts — and the refusal above.
- **Splitting the model** into a file holding only the modeled `fn`s and `var`s, no
  CacheIR ops. Kyle wants this "soon". Not blocking: `ir CacheIROps` coexists with the
  model's `ir CacheIR`, and the model's own ops simply go unused. Two facts settled by
  experiment and worth not re-deriving: an `ir` cannot be declared twice (`duplicate
  definition`), and an `impl` cannot extend one (`expected type, found IR`).
- **Scaling past this one stub.** `tryAttachNumber` needs `newNumberId` — the model has
  no allocator for `NumberId`, so `create_op_wrapper` refuses — and `DoubleField` /
  `writeDoubleField`, which `translate_arg_type` has no row for.

## The other notes here

All current as of 2026-10-06. `masm-op-overloads.md` was updated when the operand
column widened to two positions; `nonlocal-transformations.md` when the ternary turned
out to need two rules; `scope-end-effects.md` when the register half landed.
