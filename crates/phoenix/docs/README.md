# phoenix design notes

What is deferred and why, with the measurements behind each decision. Add to these
rather than re-deriving, and say so when one goes stale.

| file | |
|---|---|
| `next-steps.md` | dated snapshot: what verifies today, every remaining gap ordered by leverage, and the blockers already resolved |
| `masm-op-overloads.md` | how a `MacroAssembler` method picks a model op, and the rows that still don't discriminate between overloads |
| `scope-end-effects.md` | statements with no C++ line to translate from, because C++ runs them at a scope's end — how obligations are tracked, and what the register allocator actually guarantees |
| `nonlocal-transformations.md` | the cases that aren't name-to-name mappings, and why there is no second IR |
