; Python definitions and call sites.
;
; AUTHORED, not vendored. Upstream's tree-sitter-python/queries/tags.scm has
; no `@definition.method` — every method arrives as a function, so
; `?- def(M,_,"method",N).` returns zero rows with `status: ok` — and it tags a
; module-level assignment `@definition.constant`, which is a thing Python
; cannot state: there is no `const`, and an upper-case name is a convention
; the extractor would be guessing from.
; See specs/02-extraction.md § Adding a language.
;
; Conventions this file must hold to, checked by tests:
;   * the capture suffix after `definition.` IS the Kind.
;   * every node gets at most one `@definition.*` or `@scope.*` capture.
;   * every pattern here matches at least once on tests/fixtures/python.
;
; There is deliberately no `module` here. A Python module IS the file, so a
; `def` for it would be a second row for something `file/2` already states,
; and there is no node for it to span but the root.
;
; A method is a `function_definition` whose enclosing definition is a class.
; The query does not say `method`: it cannot see its own nesting without
; double-capturing the node, which is the upstream Rust defect. The extractor
; promotes a `function` under a type-like owner to `method` (lang.rs
; `TYPE_LIKE`), so the same pattern serves free functions, methods, and
; functions nested in functions, each parented by span nesting.
;
; `@doc` names the docstring. Python's documentation is INSIDE the definition
; — the first statement of its body — so the preamble walk over preceding
; comments cannot see it (specs/02-extraction.md § `@doc`). The capture is the
; `string_content`, so the delimiters are never part of the text.

; --- definitions -----------------------------------------------------------

(class_definition
  name: (identifier) @name
  body: (block . (expression_statement (string (string_content) @doc))?))
  @definition.class

(function_definition
  name: (identifier) @name
  body: (block . (expression_statement (string (string_content) @doc))?))
  @definition.function

; A module-level binding, `x = 1` or `x: int = 1` or the bare `x: int`. The
; pattern is anchored to the module so that a local inside a function body
; is not a definition, and it takes a single identifier: an unpacking
; `a, b = pair` binds two names at once and has no one name node to point at.
; Kind is `variable`, never `constant` — see the header.

(module
  (expression_statement
    (assignment left: (identifier) @name) @definition.variable))

; The same shape directly under a class body is a class attribute.

(class_definition
  body: (block
    (expression_statement
      (assignment left: (identifier) @name) @definition.field)))

; --- call sites ------------------------------------------------------------
;
; These become `name_ref` — the text, its position, and its enclosing
; definition. Tier A resolves nothing (02-extraction.md § Tier A does not
; resolve names); `store.get()` is recorded by its attribute name, and which
; symbol that denotes is the rule layer's problem. `Entry(value)` is a call
; to a class and is recorded as one: it is a use of `Entry` at that position,
; which is true.

(call function: (identifier) @name) @reference.call
(call function: (attribute attribute: (identifier) @name)) @reference.call
