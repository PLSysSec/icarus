# Transformations that aren't local name-to-name mappings

Status: the three cases below are implemented; the conclusion against a second IR
stands. Noted 2026-09-28, trimmed 2026-10-01 to keep the decision and drop the
problem statement, which the code now answers.

Most of the translator is tables — `translate_type`, `translate_method`,
`translate_free`, `translate_enum_const`, `masm_ops` — each mapping one name to
another, which works because the two languages mostly agree on structure. The cases
that don't were handled as follows:

- **The out-parameter idiom.** `FailurePath* failure; if (!addFailurePath(&failure)) {
  return false; }` is three statements meaning one `let`. Matched as a statement prefix
  by `translate_known_block`.
- **`emitLoadStubField`.** The callee's identity depends on a `StubField::Type` literal
  in an *earlier* statement, because the model has one loader per kind and Cachet has no
  overloading. Done as this note recommended — a local environment in `State`, not an
  adjacency match, so the construction and its use need not be neighbours and a
  non-literal tag is refused cleanly.
- **Scope-end effects.** Half done; see `scope-end-effects.md`.

## Why there is no second IR

Tempting, and worth stating why not. These transformations need *C++* facts (scopes,
declaration order, literal arguments) but produce *Cachet* changes, and after lowering
the facts are gone. So passes would have to run over `cpp_subset`, normalizing C++ into
a shape whose lowering is purely local, rather than over the Cachet AST. A staging
would look like: normalize (collapse out-parameter idioms, resolve stub-field uses, drop
logging) → annotate (attach scope-end obligations to declarations) → lower.

**A second IR is too expensive for what it buys.** `translate_known_block` and
`translate_known_stmt` are proto-passes fused into the walk, and local environments in
`State` — `stub_fields`, and `Scopes` for register kinds and obligations — cover every
case met so far without new machinery.

Revisit only if the non-local cases keep multiplying. The signal to watch is whether new
ones need facts a single walk cannot carry; so far each has been satisfiable by
recording something at one statement and discharging it at another.
