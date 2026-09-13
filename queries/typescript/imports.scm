; TypeScript imports, for both grammars. `@module` is the string's content
; without its quotes; nothing is resolved to a path (01-facts.md § `import`).
; `@alias` is set only where the statement binds a local name for the module
; itself: `import * as ns` and `import x = require(...)`. A default or named
; import binds members, as Python's `from m import a` does, and has none.
;
; The namespace alias is an optional capture inside the one pattern rather than
; a second pattern, which would emit two rows for `import * as ns from "m"`.
; `import type` has the same shape and is a row.

(import_statement
  (import_clause (namespace_import (identifier) @alias))?
  source: (string (string_fragment) @module)) @import

(import_statement
  (import_require_clause
    (identifier) @alias
    source: (string (string_fragment) @module))) @import

; A re-export (`export { x } from "m"`, `export * from "m"`) depends on its
; source exactly as an import does, and a layering rule that missed one would
; pass a violation.
(export_statement source: (string (string_fragment) @module)) @import

; An export clause that is not the declaration: `export { x, f as g }`,
; `export default f`, `export = f`. Each local name is a `name_export` row
; (01-facts.md § `name_export`); the join to its definition is `exported`'s
; third clause. `@export` is the specifier's `name`, never its `alias`: the
; question is which declaration is reachable, and `f` is what the clause says.
; `!source` keeps `export { x } from "m"` out, which is the `@import` above.
(export_statement
  !source
  (export_clause (export_specifier name: (identifier) @export)))
(export_statement value: (identifier) @export)
(export_statement "=" (identifier) @export)
