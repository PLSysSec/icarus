# Transformations that aren't local name-to-name mappings

Status: the cases below are implemented bar the last; the conclusion against a second
IR stands. Noted 2026-09-28, trimmed 2026-10-01, extended 2026-10-06 with the ternary.

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
- **Scope-end effects.** Done for failure paths and the register wrappers; see
  `scope-end-effects.md`.
- **The ternary operator**, which needs *two* rules rather than one, and not because of
  position. `c ? a : b` maps directly onto `if c { a } else { b }` now that Cachet's
  `if` is an expression -- `writer.loadBooleanResult(op_ == JSOp::StrictNe ? true :
  false)` translates in argument position with no reshaping at all. But a `void` arm
  has no expression form: `writer.guardIsNull(id)` becomes `emit CacheIROps::
  GuardIsNull(id)`, and an `emit` is a *statement* in Cachet. So
  `lhsVal_.isNull() ? writer.guardIsNull(lhsId) : writer.guardIsUndefined(lhsId);`
  (CacheIR.cpp:15112) needs the arms to become blocks of statements instead, which is a
  `translate_known_stmt` idiom. The split is along "do the arms produce values", which
  is a fact about the *model* -- that emitting is a statement there -- rather than about
  C++.

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
