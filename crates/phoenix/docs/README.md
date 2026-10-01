# phoenix design notes

What is deferred and why, with the measurements behind each decision. Add to these
rather than re-deriving, and say so when one goes stale.

| file | |
|---|---|
| `next-steps.md` | dated snapshot: what translates today, the one error blocking compilation, and everything still outstanding |
| `masm-op-overloads.md` | how a `MacroAssembler` method picks a model op, and the rows that still don't discriminate between overloads |
| `scope-end-effects.md` | statements with no C++ line to translate from, because C++ runs them at a scope's end — failure paths done, `Auto*` register releases not |
| `nonlocal-transformations.md` | the cases that aren't name-to-name mappings, and why there is no second IR |
