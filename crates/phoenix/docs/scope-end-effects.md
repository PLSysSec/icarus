# Scope-end effects have to be inserted on every exit path

Status: half done. Noted 2026-09-28; failure paths implemented 2026-09-30 in
`crates/phoenix/src/scopes.rs`. The `Auto*` register releases are still missing.

## The problem

Some Cachet statements have no C++ line to translate from, because in C++ they happen
implicitly at the end of a scope. Translating statement by statement misses them.

**`nextOp()` — done.** The C++ clears `addedFailurePath_` between CacheIR instructions
(`CacheIRCompiler.h:441`), so nothing in an emitter releases the failure path, while the
model requires `addFailurePath`/`releaseFailurePath` to balance (each asserts the
other's state, notes/cacheir.cachet:1806-1820).

**RAII destructors — not done.** `AutoScratchRegister` acquires in its constructor and
releases in its destructor (`CacheIRCompiler.h:536-552`), so

```cpp
AutoScratchRegisterMaybeOutput scratch(allocator, masm, output);
```

is *two* effects: `CacheIR::allocateReg()` at the declaration and
`CacheIR::releaseReg(scratchReg)` at the closing brace. The family is
`AutoScratchRegister`, `AutoScratchRegister64`, `AutoScratchRegisterMaybeOutput`,
`AutoOutputRegister`, `AutoAvailableFloatRegister`, `AutoScratchFloatRegister`.

`AutoOutputRegister` is a special case already handled differently: its declaration is
*dropped*, because the model has no allocator state for the reservation to touch, and a
reference to it becomes `CacheIR::outputReg`.

## How the failure-path half works, since the sketch did not anticipate it

`Scopes` carries a `Held` per block — `No`, `Inherited`, or `Owned` — and reports
`Obligation`s to discharge. The three-state part is the subtle bit, and the reason a
bool is not enough:

- a block **inherits** an outstanding path from the one enclosing it, so a `return`
  anywhere inside releases it
- a `return` marks only *its own* block discharged; the enclosing block's fall-through
  path still owes its own release, being a different path
- only the block that **acquired** it releases at scope end; an inheriting block doing
  so would release while the acquirer is still using it

`Scopes::on_return` and `on_scope_end` both discharge and report in one call, so there
is no state in which a caller can see an obligation and fail to emit it. The cases are
pinned by tests in `scopes.rs`, written against `emitGuardClass`'s shape — acquire in
the op's block, `return` inside a branch, more code after it still using the path.

Taking a second path out is refused as `Fidelity::Invalid`, matching
`setAddedFailurePath`'s own assertion, "multiple failure paths for instruction".

## What the register half still needs

- recognize an acquiring `Auto*` declaration and record what its scope end owes
- emit the owed releases at each `return` and at the end of the block, in reverse
  order of acquisition, as C++ destroys in reverse declaration order

Order probably does not matter for register release, but pairing is observable —
`releaseFailurePath` asserts its flag is set — so getting it wrong shows up as a
verification failure rather than silently.

Note these differ from failure paths in an important way: C++ *does* emit the register
releases, from a destructor, so they are relocated rather than invented. A failure path
release has no C++ counterpart at all.

## The distinction that is easy to get wrong

These are **compile-time** effects of the emitter function, not runtime paths of the
generated code. `failure->label()` is a branch in the *emitted machine code*; the
destructor runs while the IC compiler is building that code. So "every exit path" means
every path through the C++ emitter, and has nothing to do with whether the guard fails
at run time.
