; Rust definitions, scopes and call sites.
;
; AUTHORED, not vendored. Upstream's tree-sitter-rust/queries/tags.scm collapses
; `struct_item`, `enum_item`, `union_item` and `type_item` into one
; `@definition.class`, so every enum and type alias would be emitted as a
; struct — an extractor guessing wrong, which invariant 1 forbids. It also
; double-captures a method in a `declaration_list` as both `@definition.method`
; and `@definition.function`, has no `@definition.constant`, and treats
; `impl_item` as a reference, which leaves every method parented to the file.
; See docs/plan.md § "What the 2026-09-12 review changed", finding 1.
;
; Conventions this file must hold to, checked by tests:
;   * the capture suffix after `definition.` IS the Kind, after the remap in
;     lang.rs (`union` -> `type`).
;   * `@scope.*` marks a container that contributes a descriptor but is NOT a
;     definition. An `impl` block defines nothing; it owns things.
;   * every node gets at most one `@definition.*` or `@scope.*` capture.
;   * every pattern here matches at least once on tests/fixtures/rust.

; --- types -----------------------------------------------------------------

(struct_item name: (type_identifier) @name) @definition.struct
(enum_item   name: (type_identifier) @name) @definition.enum
(union_item  name: (type_identifier) @name) @definition.union
(type_item   name: (type_identifier) @name) @definition.typealias
(trait_item  name: (type_identifier) @name) @definition.trait
(associated_type name: (type_identifier) @name) @definition.typealias

; --- namespaces ------------------------------------------------------------

(mod_item name: (identifier) @name) @definition.module
(macro_definition name: (identifier) @name) @definition.macro

; --- terms -----------------------------------------------------------------
;
; `function_signature_item` is a trait method with no body. Upstream has no
; pattern for it, so a trait's contract is invisible.

(function_item name: (identifier) @name) @definition.function
(function_signature_item name: (identifier) @name) @definition.function
(const_item  name: (identifier) @name) @definition.constant
(static_item name: (identifier) @name) @definition.variable
(enum_variant name: (identifier) @name) @definition.constant
(field_declaration name: (field_identifier) @name) @definition.field

; --- scopes ----------------------------------------------------------------
;
; An `impl` block is not a definition — nothing is declared by `impl Store`
; that is not already declared by `struct Store`. It IS the owner of everything
; in its body, so it contributes the `Store#` descriptor and no `def` row, and
; `parent` for a method becomes the type rather than the file.

(impl_item type: (type_identifier) @name) @scope.type
(impl_item type: (generic_type type: (type_identifier) @name)) @scope.type
(impl_item type: (scoped_type_identifier name: (type_identifier) @name)) @scope.type

; --- call sites ------------------------------------------------------------
;
; These become `name_ref` — the text, its position, and its enclosing
; definition. Tier A resolves nothing (02-extraction.md § Tier A does not
; resolve names); `x.method()` is recorded by its field name and which symbol
; that denotes is the rule layer's problem.

(call_expression function: (identifier) @name) @reference.call
(call_expression function: (scoped_identifier name: (identifier) @name)) @reference.call
(call_expression function: (field_expression field: (field_identifier) @name)) @reference.call
(call_expression function: (generic_function function: (identifier) @name)) @reference.call
(macro_invocation macro: (identifier) @name) @reference.call
