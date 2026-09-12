; Rust imports. `@module` is the specifier AS WRITTEN; nothing is resolved to a
; path (01-facts.md § `import`). `@alias` is the local binding, if any.
;
; This file was always ours — no upstream grammar ships an imports query — which
; is why architecture conformance is the one capability that works on a first
; run with no SCIP index.

(use_declaration
  argument: (use_as_clause path: (_) @module alias: (_) @alias)) @import

(use_declaration
  argument: [
    (crate)
    (identifier)
    (metavariable)
    (scoped_identifier)
    (scoped_use_list)
    (self)
    (super)
    (use_list)
    (use_wildcard)
  ] @module) @import

(extern_crate_declaration name: (identifier) @module) @import
