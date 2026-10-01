# Where `tryAttachInt32` stands, and what is left

Status: written 2026-10-01, at the point where a generator's module holds its own
translated CacheIR ops. Verified by running the commands below, not from memory.

## Confirmed state

```sh
cargo run -p phoenix -- cachet 'CompareIRGenerator::tryAttachInt32' \
  --imports . --out <dir>/gen.cachet      # with notes/*.cachet copied into <dir>
cargo run --bin cachet-compiler -- <dir>/gen.cachet \
  --cpp-decls <dir>/g.h --cpp-defs <dir>/g.inc --bpl <dir>/g.bpl
```

phoenix says `complete, 12 elided`. The module holds, in order: imports, the ops'
helpers (`JSOpToCondition`, `EmitStoreBoolean`), `ir CacheIROps` with the five ops it
emits, the generator's helpers and wrappers, then `ir CompareIRGenerator`.

`cachet-compiler` reports **one** error, in `fn JSOpToCondition`. Every one of the five
ops type checks. 22 tests pass.

## The blocker

`JSOpToCondition`'s `switch` has a `default: MOZ_CRASH(..)`, which translates to
`assert false`. That is a check, not a return, so the function can fall off its end and
the return type is unsatisfied.

Cachet has no diverging construct — nothing matching `unreachable`, `never`, `noreturn`
in `built_in.rs`, and no use of the idea anywhere in `notes/*.cachet`. So this wants
either:

- **a Cachet change**, e.g. an expression or statement that type checks as diverging.
  In Boogie terms it would lower to `assert false; assume false;` — the assert keeps
  the unreachability an obligation, the assume makes the path dead. This is the
  direction Kyle preferred thinking about.
- **the default-arm fold**, which I implemented and then reverted at his request: when
  a switch's `default` crashes, move the last case into the final `else` as
  `assert <that case's condition>; <its body>`. Sound, and exactly what the
  hand-written `Condition::fromJSOp` does by hand (notes/masm.cachet:624). Rejected as
  too clever for now, not as wrong.

**Unknown beyond that: whether the module verifies.** Compilation has never succeeded,
so Boogie has never seen it.

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
