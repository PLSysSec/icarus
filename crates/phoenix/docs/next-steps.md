# Where `tryAttachInt32` stands, and what is left

Status: written 2026-10-01, updated 2026-10-02 when the first stub verified end to end.
Measured by running the commands below, not from memory.

## Confirmed state

```sh
./scripts/translate-verify.sh CompareIRGenerator
```

`CompareIRGenerator::tryAttachInt32` reports **`PASS … verified`**: phoenix says
`complete, 12 elided`, `cachet-compiler` accepts it, and Corral says *"Program has no
bugs."* The module holds, in order: imports, the ops' helpers (`JSOpToCondition`,
`EmitStoreBoolean`), `ir CacheIROps` with the five ops it emits, the generator's helpers
and wrappers, then `ir CompareIRGenerator`.

Across the class: **1 verified, 9 partial, 4 broken** — and every failure is now in
*translation*, phoenix's own gaps. Nothing fails to compile or verify. 22 phoenix tests
pass.

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

## Smaller things, roughly by value

- **4 elided `MOZ_ASSERT`s** remain in the output. Dropping an assertion is sound but
  weakens the proof. One of them cannot be translated faithfully as-is:
  `MOZ_ASSERT(output.type() == JSVAL_TYPE_BOOLEAN)` in `EmitStoreBoolean`, because
  `AutoOutputRegister::type()` is `ValueTypeFromMIRType(output_.type())` and nothing
  models that conversion. The hand-written model compares a raw `MIRType` instead,
  which is likely *stale* rather than clever — remember the C++ is ground truth.
- **`left_` / `right_` are renamed for nothing.** `left` and `right` are in
  `names::RESERVED`, but measurement showed `cachet-compiler` accepts them in both
  parameter and `let` position; the other 27 names are refused in both. Only those two
  positions were probed, which is why they are still listed. Probing label/field/op
  positions would settle it, and removing them tidies real output.
- **Register content kinds are seeded but never updated.** `scopes` learns a register's
  kind from the `use*Id`/`define*Id` that bound it, which is enough for `move32`. It
  does not track an op *changing* a register's contents — `CastBoolToInt32` leaves an
  int32 where a bool was, and `scopes` still says bool.
- **`masm_ops`' `_` shape rows don't discriminate.** `branchTestNull` is four C++
  overloads against two model ops and every emitter so far passes the `ValueOperand`
  one. `moveValue` and `movePtr` are keyed properly; the rest are right by luck of
  which overload is reached.
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

- **`releaseReg` for the `Auto*` scratch registers.** `scopes::Obligation` exists and
  carries failure paths; scratch registers are the other half and are not done. See
  `scope-end-effects.md`. Note these differ from failure paths: C++ *does*
  release them, from a destructor, so they are relocated rather than invented.
- **Splitting the model** into a file holding only the modeled `fn`s and `var`s, no
  CacheIR ops. Kyle wants this "soon". Not blocking: `ir CacheIROps` coexists with the
  model's `ir CacheIR`, and the model's own ops simply go unused. Two facts settled by
  experiment and worth not re-deriving: an `ir` cannot be declared twice (`duplicate
  definition`), and an `impl` cannot extend one (`expected type, found IR`).
- **Scaling past this one stub.** `tryAttachNumber` needs `newNumberId` — the model has
  no allocator for `NumberId`, so `create_op_wrapper` refuses — and `DoubleField` /
  `writeDoubleField`, which `translate_arg_type` has no row for.

## The other notes here

All three are current as of this writing: `masm-op-overloads.md` (the operand column
landed, the `_` rows remain), `scope-end-effects.md` (failure paths done, `Auto*`
register releases not), and `nonlocal-transformations.md` (the cases are implemented;
it records why there is no second IR).
