# queries

Tree-sitter queries, `queries/<lang>/{tags,imports}.scm`, **authored here, not
vendored**. Upstream `tags.scm` is a reference: it collapses distinct Rust kinds
into one capture, omits `@reference.call` in TypeScript, C and C++, and captures
only ambient forms in TypeScript. See `specs/02-extraction.md` § Adding a
language and `docs/plan.md` M2.

Empty until M2.
