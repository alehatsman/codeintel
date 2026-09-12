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

; An INHERENT impl: `impl Store`. The `!trait` guard is what keeps this pattern
; and the trait-impl one below mutually exclusive, so the "at most one capture
; per node" convention above still holds.
(impl_item
  !trait
  type: [
    (type_identifier) @name
    (generic_type type: (type_identifier) @name)
    (scoped_type_identifier name: (type_identifier) @name)
  ]) @scope.type

; A TRAIT impl: `impl Handler for Config`. Captured apart from the inherent case
; for two reasons. It is the source of `name_impl`, which needs both names. And
; Rust rejects `pub` on a method inside one, so a missing modifier there is
; silence rather than privacy: `visibility` emits `inherited` and the rule layer
; resolves it through `parent` (specs/01-facts.md § `visibility`).
;
; Both names are the grammar's FINAL identifier, so `impl fmt::Display for Key`
; yields `Display` and `Key`. Reading that segment off the tree is what keeps it
; from being a string split in a rule, which no rule could do.
(impl_item
  trait: [
    (type_identifier) @trait
    (generic_type type: (type_identifier) @trait)
    (scoped_type_identifier name: (type_identifier) @trait)
  ]
  type: [
    (type_identifier) @name @target
    (generic_type type: (type_identifier) @name) @target
    (scoped_type_identifier name: (type_identifier) @name) @target
    (reference_type type: (type_identifier) @name) @target
  ]) @scope.impl

; `@target` is the implemented type as written. A trait impl qualifies its
; members' symbols with `[Trait]` — `Config#[Display]fmt().`, the shape
; rust-analyzer uses — and with the target appended when it says more than the
; bare name (`[Handler for &Key]`), so `impl T for A` and `impl T for &A` give
; their methods different symbols (specs/01-facts.md § Symbol identity).

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
