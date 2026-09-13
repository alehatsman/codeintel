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
