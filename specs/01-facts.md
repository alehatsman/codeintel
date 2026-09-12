---
id: facts
status: proposed
schema_version: 1
binding: yes
---
# 01 — Fact schema

This is the project's ABI. Extractors write it, the engine reads it, agents
query it. Adding a relation is cheap; changing or removing one is a breaking
change that bumps `schema_version` and gets an entry in § Changelog.

## Universal conventions

| Thing | Convention |
|---|---|
| Every column | `u32`, an index into the string interner ([04-storage.md](04-storage.md)) — **including integers**. See § Integers. |
| File identity | The interned **repo-relative path** with `/` separators. There is no separate file id; `F` prints as a path. |
| Line numbers | **1-based**, so output pastes into `path:line` directly. |
| Column numbers | **0-based UTF-8 byte offset from line start.** |
| Byte offsets | **0-based, end-exclusive**, into the file's raw bytes. `src[B1..B2]` is the exact text. |
| Missing string | The `EMPTY` atom (the empty string). Never a sentinel like `"-"`. |
| Ordering | Relations are stored sorted by column order. Query output order is defined in [03-datalog.md](03-datalog.md). |

### Integers

Line, column, and byte columns are interned like everything else, but the id
space is split so integers need no decode step:

```
0x0000_0000 ..= 0x0FFF_FFFF   small non-negative integers. id n IS the integer n.
0x1000_0000 ..= 0xFFFF_FFFF   string-table entries. 0x1000_0000 is EMPTY ("").
```

So `Line = 42` is literally `42` in the tuple, and `<`/`>` work directly on the
raw `u32` with no lookup. Integer `0` is atom `0`; the empty string is atom
`0x1000_0000`. Files larger than 268M lines or bytes are not supported.

Consequence an implementer must not miss: the interned string `"42"` and the
integer `42` are **different atoms**. `Line = "42"` never matches.

## Atom vocabularies

Closed sets. An extractor that cannot map to one of these emits `unknown`; it
does not invent a value.

**`Kind`** — 16 values:
```
module  type  interface  struct  enum  trait  class
function  method  constructor  field  constant  variable
macro  typealias  unknown
```

**`Role`** — how an occurrence uses the symbol:
```
def  read  write  import  test  generated  forward
```

**`Prov`** — resolution provenance. Exactly two values, forever:
```
exact    from SCIP. type-resolved by a real compiler front-end.
name     from tree-sitter. matched by identifier text. may be wrong, may be missing.
```

**`Lang`** — lowercase SCIP `Language` enum names: `rust`, `python`,
`typescript`, `typescriptreact`, `javascript`, `go`, `java`, `ruby`, `cpp`, `c`,
`csharp`, ...

## Symbol identity

`SymId` is the interned **symbol string**. Two schemes:

**Resolved** — the SCIP symbol string verbatim:
```
scip-go gomod github.com/org/proj v1.4.0 internal/store/Store#Get().
rust-analyzer cargo codeintel 0.1.0 facts/intern/Interner#intern().
```

**Unresolved** — synthesized by the tree-sitter tier using the same SCIP
descriptor grammar, under a `local` scheme keyed by path:
```
local src/store.rs Store#get().
local app/main.py `__main__`/run().
```

Descriptor suffixes follow SCIP: `/` namespace, `#` type, `.` term, `().` method,
`:` meta, `[...]` type parameter. Keying unresolved symbols by path is mandatory
— SCIP's own `local N` symbols are document-scoped, and two files both containing
`local 4` must not collide.

When both tiers cover a definition, **ingest unifies them**: the tree-sitter def
adopts the SCIP symbol string as its `SymId` and `resolved(S)` is emitted. There
is exactly one `def` row per definition, carrying tree-sitter's span and
signature together with SCIP's global identity. See
[02-extraction.md](02-extraction.md) § The anchor join.

---

## Base relations

14 relations. Ceiling is 16 ([00-overview.md](00-overview.md) § Surface budget).

### `file(F, Lang)`
One row per indexed source file. `F` is the repo-relative path.
```
file("src/store.rs", "rust").
```

### `def(S, F, Kind, Name)`
One row per definition. `Name` is the display name as written (no
qualification); `S` carries the qualification.
```
def("local src/store.rs Store#get().", "src/store.rs", "method", "get").
```

### `def_span(S, StartLine, EndLine, StartByte, EndByte)`
The **full** definition including body, attributes, and doc comment. Byte range
is exact and sliceable.
```
def_span("local src/store.rs Store#get().", 42, 57, 1180, 1604).
```

### `def_name(S, Line, Col)`
Position of the **identifier token** — where the name is written. This is what a
rename edits and what a jump-to-definition targets. Distinct from
`def_span`'s start, which includes decorators and doc comments.

### `def_sig(S, Sig)`
The declaration header with the body stripped, normalized to one line.
```
def_sig("local src/store.rs Store#get().", "pub fn get(&self, k: &str) -> Option<Entry>").
```

### `def_doc(S, Doc)`
The attached doc comment, marker prefixes stripped, newlines preserved. Absent
if there is none — **no empty-string rows**, so `!def_doc(S, _)` means undocumented.

### `parent(Child, Parent)`
Lexical containment, one hop. `Parent` is a `SymId` for nested definitions or a
file path `F` for top-level ones — both are atoms, so the column is uniform.
Every `def` has exactly one `parent` row.
```
parent("local src/store.rs Store#get().", "local src/store.rs Store#").
parent("local src/store.rs Store#",       "src/store.rs").
```

### `exported(S)`
Present iff the symbol is visible outside its defining module, per that
language's rules (Rust `pub`, Go capitalization, TS `export`, Python
`__all__`/leading-underscore convention). Best-effort; absence is not proof of
privacy in dynamic languages.

### `resolved(S)`
Present iff `S` has SCIP-grade identity. The gate for precision-critical
queries: `def(S,...), resolved(S)` restricts to symbols a compiler agreed exist.

### `import(F, Module, Alias)`
One row per import statement. `Module` is the module specifier **as written**
(`"./util"`, `"github.com/x/y"`, `"std::collections"`), not resolved to a path —
resolution is the SCIP tier's job and shows up in `ref`/`extern`. `Alias` is the
local binding, or `""`.
```
import("src/api.rs", "crate::store", "").
import("web/app.ts", "./util/retry", "retry").
```

### `ref(S, F, Line, Col, From, Role, Prov)`
**The load-bearing relation.** One row per occurrence of symbol `S` at
`F:Line:Col`, lexically inside definition `From`, used as `Role`, resolved with
provenance `Prov`. `From` is the innermost enclosing definition, or `F` if the
occurrence is at file scope.
```
ref("... Store#get().", "src/api.rs", 88, 12, "... handler#read().", "read", "exact").
```
Definition sites also appear here with `Role = "def"`, so "every place this name
occurs" is one relation, not two.

### `implements(S, T, Prov)`
`S` implements, satisfies, or overrides `T`. From SCIP
`Relationship.is_implementation`. Covers interface impls, trait impls, method
overrides, and protocol conformance uniformly.

### `has_type(S, T, Prov)`
The type of `S` is `T`. From SCIP `Relationship.is_type_definition`. Present
only for languages whose indexer supplies it; do not assume coverage.

### `extern(S, Manager, Pkg, Version)`
`S` belongs to a third-party package, parsed from the SCIP symbol string's
`Package`. Lets a query separate "our code" from "our dependencies" without a
path heuristic.
```
extern("scip-go gomod github.com/gin-gonic/gin v1.9.1 gin/Context#JSON().",
       "gomod", "github.com/gin-gonic/gin", "v1.9.1").
```

---

## Derived relations (`rules/stdlib.dl`)

Shipped as Datalog, not Rust. Users can read them, override them, or ignore
them. This is invariant 3 in [00-overview.md](00-overview.md), and the reason
`calls` is not an extractor output.

```prolog
% --- callability -------------------------------------------------------
callable("function"). callable("method"). callable("constructor"). callable("macro").

% --- calls: a reference to a callable, attributed to its enclosing def --
calls_at(From, S, F, L, Prov) :-
    ref(S, F, L, _, From, Role, Prov), Role != "def",
    def(S, _, K, _), callable(K).
calls(From, S) :- calls_at(From, S, _, _, _).
calls_exact(From, S) :- calls_at(From, S, _, _, "exact").

callers(C, S) :- calls(C, S).
callees(S, C) :- calls(S, C).

% name-taking variants, so `codeintel rules` stays a dumb translation
callers_by_name(C, N) :- calls(C, S), def(S, _, _, N).
impact_by_name(C, N)  :- impact(S, C), def(S, _, _, N).

% --- transitive closure ------------------------------------------------
reaches(A, B) :- calls(A, B).
reaches(A, C) :- reaches(A, B), calls(B, C).

impact(S, C)  :- reaches(C, S).          % everything that transitively calls S
recursive(S)  :- reaches(S, S).

reaches_exact(A, B) :- calls_exact(A, B).
reaches_exact(A, C) :- reaches_exact(A, B), calls_exact(B, C).
impact_exact(S, C)  :- reaches_exact(C, S).

% --- location helpers --------------------------------------------------
file_of(S, F)  :- def(S, F, _, _).
defines(F, S)  :- def(S, F, _, _).
at(S, F, L)    :- def(S, F, _, _), def_span(S, L, _, _, _).

% --- containment closure -----------------------------------------------
within(C, P) :- parent(C, P).
within(C, A) :- parent(C, P), within(P, A).

% --- file-level dependency ---------------------------------------------
depends(F, G) :- ref(S, F, _, _, _, _, _), def(S, G, _, _), F != G.

% --- the dex "34 methods", as rules ------------------------------------
dead_export(S)   :- exported(S), def(S, F, _, _),
                    !ref_outside(S, F).
ref_outside(S, F):- ref(S, G, _, _, _, _, _), G != F.

entrypoint(S)    :- def(S, _, K, _), callable(K), !calls(_, S).

long_def(S, N)   :- def_span(S, A, B, _, _), N = B - A, N > 80.

undocumented_export(S) :- exported(S), !def_doc(S, _).

uses_package(F, P) :- ref(S, F, _, _, _, _, _), extern(S, _, P, _).
```

Note `dead_export` and `entrypoint` use negation — both are stratified, which
the engine verifies before evaluating ([03-datalog.md](03-datalog.md) §
Stratification). Note also that `dead_export` over a `name`-provenance graph
**over-reports**: an unresolved call is an invisible call. The honest precise
form gates on provenance:

```prolog
dead_export_exact(S) :- exported(S), resolved(S), def(S, F, _, _),
                        !ref_outside_exact(S, F).
ref_outside_exact(S, F) :- ref(S, G, _, _, _, _, "exact"), G != F.
```

That distinction is the entire point of the `Prov` column, and every rule in
`stdlib.dl` that can be contaminated by it ships in both forms.

---

## Worked queries

```prolog
% Transitive blast radius of a rename, precise only, with locations.
?- impact_exact(S, C), def(S, _, _, "get"), def(C, F, _, N), at(C, F, L).

% Exported symbols in src/store/ that nothing outside the file references.
?- dead_export(S), def(S, F, _, _), match(F, "^src/store/").

% Which third-party packages does the API layer touch?
?- uses_package(F, P), match(F, "^src/api/").

% Methods on Store, with signatures, ordered by line.
?- parent(M, "local src/store.rs Store#"), def(M, _, "method", N), def_sig(M, Sig).

% Functions over 80 lines with no doc comment.
?- long_def(S, N), !def_doc(S, _), def(S, F, _, Name), at(S, F, L).

% Everything implementing a trait, and where.
?- implements(S, T, _), def(T, _, "trait", "Handler"), def(S, F, _, N), at(S, F, L).

% Count callers per function, top 20.
?- def(S, _, "function", N), C = count{ X : calls(X, S) }, C > 0.
```

---

## Scale

For a 1M-symbol repository, measured against dex's graph statistics as the
reference point:

| Relation | Rows (order) | Cols | Bytes |
|---|---:|---:|---:|
| `ref` | 5M | 7 | 140 MB |
| `def` | 1M | 4 | 16 MB |
| `def_span` | 1M | 5 | 20 MB |
| everything else | ~2M | ~3 | ~24 MB |
| **total** | | | **~200 MB** |

Comfortably resident. This is why the engine is in-memory and why fixed-width
`u32` tuples are the right representation ([04-storage.md](04-storage.md)).

---

## Changelog

| schema_version | Change |
|---|---|
| 1 | Initial. 14 relations. |
