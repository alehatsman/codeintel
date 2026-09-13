; TypeScript definitions and call sites. The TSX grammar loads this file with
; jsx.scm appended (lang.rs), so nothing here may name a JSX node: a query
; naming one does not compile against the TypeScript grammar.
;
; AUTHORED, not vendored. Upstream's tree-sitter-typescript/queries/tags.scm is
; 23 lines and captures only ambient `.d.ts` forms (`function_signature`,
; `method_signature`, `abstract_class_declaration`, `interface_declaration`),
; so an ordinary `class X {}`, `function f() {}` or `const f = () => {}`
; produces nothing, and it has no `@reference.call` at all.
; See specs/02-extraction.md § TypeScript and TSX.
;
; Conventions this file must hold to, checked by tests:
;   * the capture suffix after `definition.` IS the Kind, unless `@value` makes
;     a binding a `function` (lang.rs `function_values`).
;   * every node gets at most one `@definition.*` or `@scope.*` capture.
;   * every Kind here is emitted at least once on tests/fixtures/typescript.
;
; There is deliberately no file-level `module`. A TypeScript module IS the
; file, which `file/2` already states. `namespace X {}` and `declare module
; "m" {}` are modules written inside one, and those are captured.

; --- types -----------------------------------------------------------------

(class_declaration name: (type_identifier) @name) @definition.class
(abstract_class_declaration name: (type_identifier) @name) @definition.class
(interface_declaration name: (type_identifier) @name) @definition.interface
(enum_declaration name: (identifier) @name) @definition.enum
(type_alias_declaration name: (type_identifier) @name) @definition.typealias

; --- namespaces ------------------------------------------------------------
;
; `declare module "m"` is named by its string, quotes and all: that is how it
; is written, and how scip-typescript names it (`"m"`/).

(internal_module name: [(identifier) (nested_identifier)] @name) @definition.module
(module name: (string) @name) @definition.module

; --- functions -------------------------------------------------------------
;
; An overload signature and its implementation are two declarations of one
; symbol (specs/01-facts.md § `def`). A function declared inside a function is
; a definition owned by the outer one, as in Python.

(function_declaration name: (identifier) @name) @definition.function
(generator_function_declaration name: (identifier) @name) @definition.function
(function_signature name: (identifier) @name) @definition.function

; --- members ---------------------------------------------------------------
;
; Anchored to the body that owns them, so a method in an object literal and a
; property in a type literal are not definitions. A name written as a string,
; a number or a computed expression (`["x"]() {}`) is not captured: there is
; no identifier token for `def_name` to point at.
;
; `get x()` and `set x(v)` are two `method_definition`s named `x`, which is
; two declarations of one tier-A symbol.

(class_body
  (method_definition
    name: (property_identifier) @name
    (#eq? @name "constructor")) @definition.constructor)
(class_body
  (method_definition
    name: [(property_identifier) (private_property_identifier)] @name
    (#not-eq? @name "constructor")) @definition.method)
(class_body
  (method_signature
    name: [(property_identifier) (private_property_identifier)] @name) @definition.method)
(class_body
  (abstract_method_signature
    name: [(property_identifier) (private_property_identifier)] @name) @definition.method)
(class_body
  (public_field_definition
    name: [(property_identifier) (private_property_identifier)] @name) @definition.field)

(interface_body (method_signature name: (property_identifier) @name) @definition.method)
(interface_body (property_signature name: (property_identifier) @name) @definition.field)

; An enum member is its bare name, or a name with an initializer. The bare
; form has no node but the identifier, so it is both the definition and its
; name.
(enum_body name: (property_identifier) @name @definition.constant)
(enum_body (enum_assignment name: (property_identifier) @name) @definition.constant)

; --- bindings --------------------------------------------------------------
;
; A binding is a definition at module or namespace scope only. A `const` in a
; function body is a local, as an assignment inside a Python `def` is. The Kind
; is what the keyword states, `constant` under `const` and `variable` under
; `let` and `var`, unless the `@value` node is a function, in which case it is
; `function` (specs/02-extraction.md § `@value`). Destructuring
; (`const { a, b } = o`) binds several names with no single name node, and is
; not captured.
;
; The same three declarator shapes, repeated for each statement they can sit
; in: bare, under `export`, under `declare`, and under `export declare`. A query
; cannot name "any of these parents" once and share the child.

(program [
  (lexical_declaration kind: "const" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.constant)
  (lexical_declaration kind: "let" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
  (variable_declaration (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
  (export_statement declaration: [
    (lexical_declaration kind: "const" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.constant)
    (lexical_declaration kind: "let" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
    (variable_declaration (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
  ])
  (ambient_declaration [
    (lexical_declaration kind: "const" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.constant)
    (lexical_declaration kind: "let" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
    (variable_declaration (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
  ])
  (export_statement (ambient_declaration [
    (lexical_declaration kind: "const" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.constant)
    (lexical_declaration kind: "let" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
    (variable_declaration (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
  ]))
])

(internal_module body: (statement_block [
  (lexical_declaration kind: "const" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.constant)
  (lexical_declaration kind: "let" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
  (variable_declaration (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
  (export_statement declaration: [
    (lexical_declaration kind: "const" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.constant)
    (lexical_declaration kind: "let" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
    (variable_declaration (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
  ])
]))

(module body: (statement_block [
  (lexical_declaration kind: "const" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.constant)
  (lexical_declaration kind: "let" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
  (variable_declaration (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
  (export_statement declaration: [
    (lexical_declaration kind: "const" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.constant)
    (lexical_declaration kind: "let" (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
    (variable_declaration (variable_declarator name: (identifier) @name value: (_)? @value) @definition.variable)
  ])
]))

; --- call sites ------------------------------------------------------------
;
; These become `name_ref`: the text, its position, and its enclosing
; definition. Tier A resolves nothing (02-extraction.md § Tier A does not
; resolve names). `store.get()` is recorded by its property name, and
; `new Store()` is a use of `Store` at that position, which is true.

(call_expression function: (identifier) @name) @reference.call
(call_expression
  function: (member_expression
    property: [(property_identifier) (private_property_identifier)] @name)) @reference.call
(new_expression constructor: (identifier) @name) @reference.call
(new_expression
  constructor: (member_expression property: (property_identifier) @name)) @reference.call
