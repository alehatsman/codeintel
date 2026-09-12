; Go definitions, owners and call sites.
;
; AUTHORED, not vendored. Upstream's tree-sitter-go/queries/tags.scm emits
; `@definition.type` for a struct, an interface, a defined type and an alias
; alike — and `type` is not even in this schema's Kind vocabulary, so all four
; would arrive as one guess. Five of its patterns carry a bare `@name` with no
; tag at all, which leaves emission undefined for them. It has no receiver
; handling, so every method parents to the file.
; See docs/plan.md § "What the 2026-09-12 review changed", finding 1.
;
; Conventions this file must hold to, checked by tests:
;   * the capture suffix after `definition.` IS the Kind.
;   * every node gets at most one `@definition.*` or `@scope.*` capture.
;   * every pattern here matches at least once on tests/fixtures/go.
;
; Definitions capture the *spec* node, not the enclosing declaration, so that a
; grouped `const ( A = 1 \n B = 2 )` yields two definitions with two spans
; rather than two definitions sharing one. The `const`/`var`/`type` keyword is
; recovered by `attribute_kinds` in lang.rs, which is what makes the span read
; `const Limit = 64` and not `Limit = 64`.
;
; There is deliberately no `module` here. Go's `package_clause` names a package
; that spans files, so a per-file `def` for it would be one definition claimed
; N times, and it encloses nothing — the decls are its siblings, not its
; children. Package identity comes from tier B, which has it exactly.

; --- types -----------------------------------------------------------------
;
; The discrimination upstream does not do. A query has no negation, so the
; fall-through kind enumerates the rest of the grammar's `_type` union rather
; than matching `(_)` and double-capturing struct and interface. The one gap is
; `type X (struct{...})`, whose `parenthesized_type` lands on `type`; nobody
; writes it, and the honest alternative is unbounded recursion through
; parentheses in a query language that cannot express it.

(type_spec name: (type_identifier) @name type: (struct_type)) @definition.struct
(type_spec name: (type_identifier) @name type: (interface_type)) @definition.interface
(type_alias name: (type_identifier) @name) @definition.typealias

(type_spec
  name: (type_identifier) @name
  type: [
    (array_type)
    (channel_type)
    (function_type)
    (generic_type)
    (map_type)
    (negated_type)
    (parenthesized_type)
    (pointer_type)
    (qualified_type)
    (slice_type)
    (type_identifier)
  ]) @definition.type

; --- terms -----------------------------------------------------------------

(function_declaration name: (identifier) @name) @definition.function
(const_spec name: (identifier) @name) @definition.constant
(var_spec name: (identifier) @name) @definition.variable
(field_declaration name: (field_identifier) @name) @definition.field

; A method declared in an interface. It is enclosed by the `interface_type`,
; which is enclosed by the `type_spec`, so span nesting already owns it.

(method_elem name: (field_identifier) @name) @definition.method

; --- owners ----------------------------------------------------------------
;
; The case docs/plan.md M5 names by hand. A Go method sits at file scope and
; its owner is named in its own receiver, so span nesting parents it to the
; file — and worse, synthesizes `local store.go Get().`, which two types with
; a `Get` method in one file would both claim. `@owner` is the receiver's type
; name, resolved against the top-level type-like definitions of this same file
; (specs/02-extraction.md § `@owner`). Unresolved, nothing is invented: the
; parent stays what the sweep said.

(method_declaration
  receiver: (parameter_list
    (parameter_declaration
      type: [
        (type_identifier) @owner
        (pointer_type (type_identifier) @owner)
        (generic_type type: (type_identifier) @owner)
        (pointer_type (generic_type type: (type_identifier) @owner))
      ]))
  name: (field_identifier) @name) @definition.method

; --- call sites ------------------------------------------------------------
;
; These become `name_ref` — the text, its position, and its enclosing
; definition. Tier A resolves nothing (02-extraction.md § Tier A does not
; resolve names); `s.Get()` is recorded by its field name and which symbol
; that denotes is the rule layer's problem. A conversion `Mode(x)` is
; indistinguishable from a call here, and is recorded as one: it is a use of
; `Mode` at that position, which is true.

(call_expression function: (identifier) @name) @reference.call
(call_expression function: (selector_expression field: (field_identifier) @name)) @reference.call
(call_expression function: (index_expression operand: (identifier) @name)) @reference.call
(call_expression
  function: (index_expression operand: (selector_expression field: (field_identifier) @name)))
  @reference.call
