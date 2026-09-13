; Python imports. `@module` is the specifier AS WRITTEN — dotted, relative
; dots included — nothing is resolved to a path (01-facts.md § `import`).
; `@alias` is the local binding a top-level `as` introduces.
;
; `import a, b` is two rows. Each name in the list is its own specifier and
; carries its own `as`, so one row could not hold both aliases.
;
; `from m import a, b as c` is ONE row, `Module = "m"`, no alias: the names it
; binds are members of `m`, not local names for `m`, and 01-facts.md § `import`
; sets `Alias` only for a top-level `as`. The dots of a relative import are
; part of the specifier as written, which is why `module_name` is `(_)`.
;
; `from __future__ import annotations` is not here. It is a compiler directive,
; `__future__` is a bare token in the grammar rather than a name node, and no
; conformance question is asked of it.

(import_statement
  name: (dotted_name) @module) @import

(import_statement
  name: (aliased_import
    name: (dotted_name) @module
    alias: (identifier) @alias)) @import

(import_from_statement
  module_name: (_) @module) @import
