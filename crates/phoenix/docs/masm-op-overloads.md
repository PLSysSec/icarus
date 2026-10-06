# The MASM op table discriminates on one operand, and not yet on every method

Status: partly fixed. Noted 2026-09-28 as "keyed by name alone"; the operand column
landed 2026-10-01 when `moveValue` needed it. What is left is the rows that still
ignore it.

## What the table does now

`crates/phoenix/src/masm_ops.rs`'s `translate_op` keys on the method name *and* the
shape of its first operand, which is what C++ overloads on:

```rust
("moveValue", Some("Value"))    => "MoveValueImm",
("moveValue", Some("ValueReg")) => "MoveValue",
```

`operand_shape` reads the argument's type through `translate_type`, so it speaks the
model's vocabulary and keeps no second table of C++ paths.

Two methods need more than that lookup and sit in `masm_call` instead, for opposite
reasons:

- **`move32`** — both C++ overloads are `(Register, Register)`, so the call says
  nothing. Which op applies depends on what the register *holds*, which only
  `scopes` knows. See the register-content notes in `next-steps.md`.
- **`movePtr`** — the model has no generic `movePtr`; its ops name the immediate kind
  and its payload (`MovePtrBoolImmWord`), so choosing the op also rewrites an
  argument, dropping the `ImmWord(..)` construction.

## What is left

The `_` rows still ignore the operand column, and are right only because of which
overload the emitters happen to reach:

| row | C++ overloads | model ops |
|---|---|---|
| `branchTestNull` | 4 | `BranchTestNull`, `BranchTestNullTag` |
| `branchTestInt32` | 4 | `BranchTestInt32`, `BranchTestInt32Tag` |
| `branch32` | several | `Branch32`, `Branch32Tag`, `Branch32Imm`, `Branch32AddressImm32` |
| `fallibleUnboxBoolean` | templated on the source | the `ValueOperand` one only |

Every emitter so far passes the `ValueOperand` form, so the name alone picks correctly.

How bad: `ValueReg` and `Reg` are distinct Cachet types, so mapping a tag call to the
value op emits an argument of the wrong type and `cachet-compiler` rejects it. The
failure is caught — it just points at the argument rather than at the op choice.

Fill a row in when an emitter first reaches its other overload. Guessing the whole
family now would mean inventing correspondences for ops no translated code uses, and
only 45 of the model's 106 ops are even the lowercased method name, so the table stays
hand-written either way.
