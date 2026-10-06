# The MASM op table discriminates on two operands, and not yet on every method

Status: mostly fixed. Noted 2026-09-28 as "keyed by name alone"; the operand column
landed 2026-10-01 when `moveValue` needed it, and widened to two operands 2026-10-02
when the `branchTest*` family needed it. What is left is the rows that still ignore it.

## What the table does now

`crates/phoenix/src/masm_ops.rs`'s `translate_op` keys on the method name *and* the
shapes of its first two operands, which is what C++ overloads on:

```rust
("moveValue", Some("Value"), _)            => "MoveValueImm",
("moveValue", Some("ValueReg"), _)         => "MoveValue",
("branchTestNull", _, Some("ValueReg"))    => "BranchTestNull",
("branchTestNull", _, Some("Reg"))         => "BranchTestNullTag",
```

Two positions rather than one because the `branchTest*` family overloads on its
*second*: the first is always the `Condition`. `operand_shape` reads an argument's type
through `translate_type`, so the column speaks the model's vocabulary and keeps no
second table of C++ paths.

Three methods need more than that lookup and sit in `masm_call` instead, for different
reasons:

- **`move32`** — both C++ overloads are `(Register, Register)`, so the call says
  nothing. Which op applies depends on what the register *holds*, which only
  `scopes` knows. See the register-content notes in `next-steps.md`.
- **`movePtr`** — the model has no generic `movePtr`; its ops name the immediate kind
  and its payload (`MovePtrBoolImmWord`), so choosing the op also rewrites an
  argument, dropping the `ImmWord(..)` construction.
- **`assumeUnreachable`** — its op takes no message, so choosing it drops the argument.
  Not the `unreachable` statement: `MOZ_CRASH` is the generator giving up as it runs,
  while this *emits* an instruction that traps when the generated code runs.

## What is left

These `_` rows still ignore the operand column, and are right only because of which
overload the emitters happen to reach:

| row | C++ overloads | model ops |
|---|---|---|
| `branchTestInt32` | 4 | `BranchTestInt32`, `BranchTestInt32Tag` |
| `branch32` | several | `Branch32`, `Branch32Tag`, `Branch32Imm`, `Branch32AddressImm32` |
| `fallibleUnboxBoolean` | templated on the source | the `ValueOperand` one only |

`branchTestNull` left this list 2026-10-02, along with `branchTestUndefined` and
`branchTestObject`: `emitCompareNullUndefinedResult` reaches both forms of the same
method in one function -- the tag form on an extracted tag, the value form in
`emitGuardToUndefined` -- so the column had to discriminate rather than guess. Getting
it wrong there would have been the first case where the *same* method name needed two
different ops in one translated module.

Every other emitter so far passes the `ValueOperand` form, so the name alone picks
correctly.

How bad: `ValueReg` and `Reg` are distinct Cachet types, so mapping a tag call to the
value op emits an argument of the wrong type and `cachet-compiler` rejects it. The
failure is caught — it just points at the argument rather than at the op choice.

Fill a row in when an emitter first reaches its other overload. Guessing the whole
family now would mean inventing correspondences for ops no translated code uses, and
only 45 of the model's 106 ops are even the lowercased method name, so the table stays
hand-written either way.
