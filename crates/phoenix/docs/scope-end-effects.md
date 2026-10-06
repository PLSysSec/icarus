# Scope-end effects have to be inserted on every exit path

Status: done for failure paths (2026-09-30) and for the two scratch-register wrappers
(2026-10-02), in `crates/phoenix/src/scopes.rs`. `ScratchTagScope` and
`ScratchTagScopeRelease` remain; see `next-steps.md`. Noted 2026-09-28.

## The problem

Some Cachet statements have no C++ line to translate from, because in C++ they happen
implicitly at the end of a scope. Translating statement by statement misses them.

**`nextOp()` — done.** The C++ clears `addedFailurePath_` between CacheIR instructions
(`CacheIRCompiler.h:441`), so nothing in an emitter releases the failure path, while the
model requires `addFailurePath`/`releaseFailurePath` to balance (each asserts the
other's state, notes/cacheir.cachet:1806-1820).

**RAII destructors — done for the scratch registers.** `AutoScratchRegister` acquires in
its constructor and releases in its destructor (`CacheIRCompiler.h:536-552`), so

```cpp
AutoScratchRegisterMaybeOutput scratch(allocator, masm, output);
```

is *two* effects: `CacheIR::allocateReg()` at the declaration and
`CacheIR::releaseReg(scratch)` at the closing brace. Both of those are now emitted, for
`AutoScratchRegister` and `AutoScratchRegisterMaybeOutput`. Still outstanding in the
family: `AutoScratchRegister64`, `AutoAvailableFloatRegister`,
`AutoScratchFloatRegister`, and the tag-scope pair.

`MaybeOutput` reuses the output register when one is free rather than allocating, which
the model's allocator cannot express. Translating it as an allocation is still sound,
because the output register is assumed *un*allocated (notes/utils.cachet:64) and so
`allocateReg` may return it — the aliasing case the C++ exploits is inside what the
verifier considers. The cost is register pressure, not correctness: the model consumes
a register where the C++ reuses one, so `allocateReg`'s `assert hasAvailableReg()` could
fail on a starved stub where the real code succeeds.

`AutoOutputRegister` is a special case already handled differently: its declaration is
*dropped*, because the model has no allocator state for the reservation to touch, and a
reference to it becomes `CacheIR::outputReg`.

## How obligations work, since the sketch did not anticipate it

Each block carries an ordered list of the `Obligation`s it took on, and *which block
holds one* is what says who discharges it. Three rules fall out, and they are the same
for a failure path and for a register:

- a `return` leaves every enclosing block, so it discharges all of them, innermost
  first and within each in reverse of the order taken on — the order C++ runs
  destructors in
- a `return` clears only *its own* block; an enclosing block's fall-through path still
  owes its own, being a different path
- falling out of a block discharges only that block's own list, so a path acquired
  further up is not released while its acquirer is still using it

An earlier version carried a three-state `Held` per block — `No`, `Inherited`, `Owned`
— to get the same effect. The list subsumes it: "inherited" was an encoding of
"visible to a return from an inner block but not released at that block's end", which
walking the scopes expresses directly.

`Scopes::on_return` and `on_scope_end` both discharge and report in one call, so there
is no state in which a caller can see an obligation and fail to emit it. The cases are
pinned by tests in `scopes.rs`, written against `emitGuardClass`'s shape — acquire in
the op's block, `return` inside a branch, more code after it still using the path.

Taking a second path out is refused as `Fidelity::Invalid`, matching
`setAddedFailurePath`'s own assertion, "multiple failure paths for instruction".

## What the register half turned out to need

`is_scratch_register_decl` recognizes the declaration, `Scopes::acquire_reg` records it,
and the releases are emitted at each `return` and at the block's end in reverse order of
acquisition.

Order does not in fact matter for register release: `releaseReg` asserts only that the
register *is* allocated (notes/support.bpl:446), nothing about failure paths. Pairing is
observable either way, so getting it wrong shows up as a verification failure rather
than silently.

Two facts about the allocator, hard-won and easy to assume wrongly. It is **not**
uninterpreted: `notes/support.bpl` implements it, and `translate-verify.sh` prepends
that file. And it is **stateful** — `allocateReg` does `assume !isAllocatedReg(ret)` and
then adds the result to `allocatedRegs` — so two `allocateReg()` calls yield *distinct*
registers. That is load-bearing: `emitCompareBigIntInt32Result` needs two, and the
hand-written model passes one register twice where two were available. It also rests on
`CacheIR::allocateReg` being pinned to a Boogie *procedure* in `bpl.rs`'s
`BLOCKED_PATHS`; as a Boogie function it would be a constant, and the two scratch
registers would silently collapse into one.

Note these differ from failure paths in an important way: C++ *does* emit the register
releases, from a destructor, so they are relocated rather than invented. A failure path
release has no C++ counterpart at all.

## The distinction that is easy to get wrong

These are **compile-time** effects of the emitter function, not runtime paths of the
generated code. `failure->label()` is a branch in the *emitted machine code*; the
destructor runs while the IC compiler is building that code. So "every exit path" means
every path through the C++ emitter, and has nothing to do with whether the guard fails
at run time.
