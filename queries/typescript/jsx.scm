; JSX component uses, for the TSX grammar only. lang.rs appends this file to
; typescript/tags.scm, because the TypeScript grammar has no JSX nodes and a
; query naming one does not compile against it.
;
; A tag starting with an upper-case letter is a component: a value in scope,
; used at that position, as a call is. A lower-case tag (`div`) is an intrinsic
; element and names nothing in scope. That is the rule the TypeScript compiler
; applies to JSX, read from the name exactly as Go's export rule is. A dotted
; tag is always a value, and is recorded by its final name as `ui.row()` is.
; Only the opening and self-closing tags count; a closing tag is the same use.

(jsx_opening_element name: (identifier) @name (#match? @name "^[A-Z]")) @reference.call
(jsx_self_closing_element name: (identifier) @name (#match? @name "^[A-Z]")) @reference.call
(jsx_opening_element
  name: (member_expression property: (property_identifier) @name)) @reference.call
(jsx_self_closing_element
  name: (member_expression property: (property_identifier) @name)) @reference.call
