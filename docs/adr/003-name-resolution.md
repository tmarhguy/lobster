# 003 — Single-file name resolution with span-keyed globals

- Status: accepted
- Date: 2026-10-01

## Context

The checker needs to answer "what does this path mean" for every name
occurrence. Resolution must be order-independent within a module, report
duplicates/unknowns/ambiguity, and cover locals, params, items, imports,
types, variants, and the `println` builtin — without a module system yet.

## Decision

`lobster-resolve` collects items first (order-independent), then walks bodies
with lexical ribs. Locals and params stay in ribs that the checker rebuilds
identically; only globals are exported, keyed by source span:

- value paths → `Local` (checker resolves by name) or `Fn` / `Variant` /
  `BuiltinPrintln`;
- a bare Uppercase name resolving to exactly one enum variant records its
  parent enum; a struct name records a struct head for struct patterns;
- a two-segment path with an enum/struct head records the pair unchecked —
  the checker verifies variant/field membership (E218), so genuinely
  unknown members are checker errors, not resolution errors;
- genuinely unknown names are E200, duplicate bindings E201, every import
  E202 (no modules exist), unknown types E203.

Type lowering (`Resolved::lower_type`) lives here too, returning
`Unknown` (already reported, stay silent) or `Generic` (checker reports
E217) instead of diagnosing inline.

## Alternatives

- Resolve locals into exported ids: rejected — rib parity between two
  crates is fragile; name-based local lookup in the checker is simpler
  and total.
- Resolve imports against a fake std: rejected — dishonest; E202 says
  plainly that modules are deferred.

## Tradeoffs

Span keys assume distinct nodes never share a span (true: parens collapse
in the parser, blocks are single nodes). A future rename of AST spans
must keep that property.

## Consequences

`lobster check` runs `lobster_resolve::resolve` before `lobster_sema::check_file`;
the checker never re-walks scopes for globals. Multi-file modules will
extend the item tables, not replace the design.
