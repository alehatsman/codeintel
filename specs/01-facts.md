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
0x1000_0000 ..= 0xFFFF_FFFF   deduplicated strings. 0x1000_0000 is EMPTY ("").
```

So `Line = 42` is literally `42` in the tuple, and `<`/`>` work directly on the
raw `u32` with no lookup. Integer `0` is atom `0`; the empty string is atom
`0x1000_0000`. Files larger than 268M lines or bytes are not supported.

Consequence an implementer must not miss: the interned string `"42"` and the
integer `42` are **different atoms**. `Line = "42"` never matches.

**Everything is interned the same way, including doc comments.** An earlier
draft gave long strings an un-deduplicated "opaque" id range to skip a hash
lookup at index time. Measured, that saves ~0.12 s on a 30 s index budget —
0.4% — and costs a third id partition, an extra safety rule, and a class of
`invalid-query` an agent cannot guess from the schema. Deleted. Doc comments
are ordinary interned strings and `=` works on them.

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

15 relations. Ceiling is 16 ([00-overview.md](00-overview.md) § Surface budget).

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
if there is none — **no empty-string rows**, so `!def_doc(S, _)` means
undocumented. `Doc` is an ordinary interned string (§ Integers): displayable and
`match`-able, but not comparable with `=`.

### `parent(Child, Parent)`
The **semantic owner**, one hop. `Parent` is a `SymId` for owned definitions or
a file path `F` for top-level ones — both are atoms, so the column is uniform.
**Exactly one row per `def`**, guaranteed by the precedence below.
```
parent("local src/store.rs Store#entries.", "local src/store.rs Store#").
parent("local src/store.rs Store#",         "src/store.rs").
```

Semantic, not lexical — and the two genuinely differ. A Go method is *not*
lexically inside its type:

```go
type Store struct { ... }
func (s *Store) Get(k string) (Entry, bool) { ... }   // lexically at file scope
```

Span nesting says `parent(Get, "store.go")`. Everyone asking "what are Store's
methods" means `parent(Get, Store#)`. The second is the useful answer, so
`parent` carries it.

**Precedence — first source that yields a value wins, and they are tried in
this fixed order:**

1. **Descriptor prefix of the SCIP symbol.** Drop the last descriptor from the
   symbol string; if the result names an indexed definition, that is the parent.
   `pkg/Store#Get().` → `pkg/Store#`. Universally available, because the
   descriptor grammar is mandatory in SCIP — unlike `enclosing_symbol`, which
   not every indexer populates.
2. **`SymbolInformation.enclosing_symbol`**, when descriptor truncation yields
   nothing indexed. This is the path for SCIP `local` symbols.
3. **Tier A span nesting** — the innermost enclosing definition, or the file.
   The only source for unresolved symbols, and the fallback everywhere else.
   *Definition*, not *span*: a Rust `impl` block encloses its methods and is no
   `def`, so the walk continues past it. See
   [02-extraction.md](02-extraction.md) § Parent precedence for what shipping
   the span version cost.

The example above is a field, not a method, because a method in a Rust `impl`
block does **not** reach `Store#` — and that is a known limit, not an accident.
`rust-analyzer` names the method `store/impl#[Store]get().`, whose descriptor
prefix is `store/impl#[Store]`, which nothing defines; rule 1 declines, and the
walk lands on the file. Getting `Store#` would mean teaching the extractor that
`impl#[T]` means `T` — per-language descriptor knowledge, and a guess invariant
1 forbids. So for Rust inherent impls this fact is *coarse but true* rather than
precise and invented. Go, whose methods carry `Store#Get().` directly, is
unaffected and still resolves to the type.

Deterministic, total, and exactly one row. The tradeoff is deliberate: lexical
containment is no longer recoverable from `parent` for the cases where the two
disagree. Nothing in `stdlib.dl` wants it, and `def_span` containment recovers
it for anything that does:

```prolog
% Query def_span containment directly. Do NOT try to express this via
% innermost_at: L binds to C's own start line, the tightest definition
% enclosing C's start line IS C, and the `P != C` filter then removes the
% only binding. An earlier draft shipped exactly that rule and it is always
% empty.
lexical_parent(C, P) :- def(C, F, _, _), def_span(C, A, B, _, _),
                        def(P, F, _, _), def_span(P, X, Y, _, _),
                        P != C, X <= A, B <= Y.
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

### `scip_ref(S, F, Line, Col, From, Role)`
A compiler-resolved occurrence of symbol `S` at `F:Line:Col`, lexically inside
definition `From`, used as `Role`. No provenance column — everything here is
`exact` by construction. `From` is the innermost enclosing definition, or `F` if
the occurrence is at file scope.
```
scip_ref("... Store#get().", "src/api.rs", 88, 12, "... handler#read().", "read").
```
Definition sites appear here too with `Role = "def"`, so "every place this
symbol occurs" is one relation, not two.

### `name_ref(Name, F, Line, Col, From)`
An **unresolved** identifier occurrence from tier A: the text `Name` appears at
`F:Line:Col` inside definition `From`, in a position the grammar calls a
reference. It names no symbol.
```
name_ref("get", "src/api.rs", 88, 12, "local src/api.rs handler#read().").
```
This relation is deliberately *local* — every column is derivable from that one
file. Resolving a name to a symbol needs whole-repo knowledge, so it happens in
the rule layer (§ Derived relations), not at extraction. Two reasons, and both
are load-bearing:

1. **Incremental correctness.** Resolution depends on what exists elsewhere in
   the repo. Baking it into a per-file fact means adding a second `foo` in file
   B silently invalidates a fact in file A, which incremental indexing will
   never revisit because file A did not change. Facts must be functions of their
   own file, or incremental indexing is unsound.
2. **Visibility.** Name resolution is a *policy*, and a policy an agent may
   reasonably want to tighten or loosen. In `stdlib.dl` it is four readable
   rules; in Rust it is a decision nobody can see.

### `implements(S, T, Prov)`
`S` implements, satisfies, or overrides `T`. From SCIP
`Relationship.is_implementation`. Covers interface impls, trait impls, method
overrides, and protocol conformance uniformly.

### `has_type(S, T, Prov)` — DEFERRED, not in v1

The type of `S` is `T`, from SCIP `Relationship.is_type_definition`. Cut from
v1: no `stdlib.dl` rule consumes it, it is present only for languages whose
indexer supplies it — so it is a zero-row trap for most users — and its `Prov`
column can only ever hold `"exact"`, since tier A cannot produce types. Re-add
when a query needs it, at which point it costs one of the two free base-relation
slots.

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
% === reference resolution ==============================================
% `ref` is DERIVED. Tier B contributes resolved occurrences directly; tier A
% contributes names that these rules resolve against the whole-repo def set.

ref(S, F, L, C, From, Role, "exact") :- scip_ref(S, F, L, C, From, Role).

% Name matching is a FALLBACK, not a parallel claim: it fires only where tier B
% never resolved this occurrence. See § Tier precedence below.
% same file wins
ref(S, F, L, C, From, "read", "name") :-
    name_ref(N, F, L, C, From), !scip_ref(_, F, L, C, _, _), def(S, F, _, N).
% else a unique exported definition repo-wide
ref(S, F, L, C, From, "read", "name") :-
    name_ref(N, F, L, C, From), !scip_ref(_, F, L, C, _, _), !local_def(F, N),
    def(S, _, _, N), exported(S), !ambiguous(N).

local_def(F, N) :- def(S, F, _, N).
% Grouped, not a self-join. The self-join form emits k*k candidate pairs for a
% name with k definitions before dedup; measured on real repos that is ~8.5k
% pairs (dex) and ~11k (tracing) — harmless — but it is O(k^2) on generated
% bindings and the aggregate form is free.
ambiguous(N)    :- def(_, _, _, N), C = count{S : def(S, _, _, N)}, C > 1.

% Everything below is unchanged by which tier supplied the evidence.
% To tighten resolution, edit the two `name` rules above. To disable tier-A
% resolution entirely, delete them: `ref` degrades to `exact` only.
```

### Tier precedence

**At an occurrence tier B resolved, tier A does not also guess.** Both `name`
rules carry `!scip_ref(_, F, L, C, _, _)`, so the tiers partition the
occurrences rather than both claiming them.

Without the guard the two tiers made independent claims about the same byte
position and `ref` was their union, so a name guess the compiler directly
contradicts survived into `calls`, `depends`, `impact_of`, `dead_export` and
`entrypoint`. Measured on this repository with `rust-analyzer scip .`:

| name-provenance `ref` rows | 1,629 |
| ...at a position `scip_ref` also covers | 1,628 |
| ...that `scip_ref` **contradicts** — it saw the position and never names that symbol | 1,448 |
| ...at a position SCIP never saw — the genuine fallback | 4 |

So 89% of tier-A resolution was noise the compiler had already refuted, and the
real fallback was four rows in a nested fixture crate outside the SCIP index.
The artifacts are characteristic rather than exotic: `Relation::new` claimed at
a site SCIP resolves to `alloc`'s `Vec::new`, and at another it resolves to a
different local `Db::new`. Name matching cannot see a receiver type, so every
`x.f()` is a coin flip among every `f` in scope.

**The guard is a no-op without SCIP.** With no `scip_ref` rows the negation is
vacuously true and every name rule fires exactly as before, so a tier-A-only
index is bit-identical. It is also per-occurrence, not per-file, so a file SCIP
did not cover keeps full name resolution while its neighbours use the compiler's
answer — which is what the four surviving rows are.

This is invariant 1 applied to derivation rather than extraction. A name guess
is an inference; keeping one the compiler has already refuted is inferring over
evidence, which is the failure `exact` exists to make impossible.

```prolog

% === the location bridge ===============================================
% Turns a file:line from ripgrep, git diff, a stack trace, or a compiler
% error into a symbol. This is how anything gets INTO the graph.

% `between` is what makes Line range-restricted: it GENERATES when its first
% two arguments are bound, so Line appears in a positive body literal and the
% rule is unconditionally safe. Written with bare comparisons instead, Line
% would be bound by nothing and the engine would reject its own stdlib.
% Demand transformation makes this fast (Line bound -> `between` degrades to a
% filter, and `def_span` sorted by (F, L1) is a binary search), but the rule
% does not depend on it for LEGALITY -- which is what keeps the naive
% evaluator able to run it, and therefore keeps the M1 differential test
% honest about the transformation itself.
symbol_at(F, Line, S) :- def(S, F, _, _), def_span(S, L1, L2, _, _),
                         between(L1, L2, Line).

% the tightest enclosing definition — usually what you want
innermost_at(F, Line, S) :- symbol_at(F, Line, S), !tighter_at(F, Line, S).

% T is strictly tighter than S: compare widths, not endpoints, so two
% definitions sharing an identical span do not cancel each other out and
% leave `innermost_at` empty.
tighter_at(F, Line, S)   :- symbol_at(F, Line, S), symbol_at(F, Line, T), T != S,
                            def_span(S, A, B, _, _), def_span(T, C, D, _, _),
                            V = B - A, W = D - C, W < V.

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

% name-taking variants, because a SymId is unwieldy to type by hand
callers_by_name(C, N) :- calls(C, S), def(S, _, _, N).
impact_by_name(C, N)  :- def(S, _, _, N), impact_of(S, C).

% --- transitive closure, SEEDED ----------------------------------------
% Always prefer these. The first argument is a seed the engine pushes into
% the recursion (see 03-datalog.md § Demand transformation), so evaluation
% grows outward from the seed instead of computing all-pairs reachability.

impact_of(Seed, C) :- calls(C, Seed).                     % who calls Seed
impact_of(Seed, C) :- impact_of(Seed, B), calls(C, B).    % ...transitively

reach_of(Seed, C)  :- calls(Seed, C).                     % what Seed calls
reach_of(Seed, C)  :- reach_of(Seed, B), calls(B, C).

impact_of_exact(Seed, C) :- calls_exact(C, Seed).
impact_of_exact(Seed, C) :- impact_of_exact(Seed, B), calls_exact(C, B).

% --- transitive closure, UNSEEDED --------------------------------------
% Whole-graph questions only. On a large repo these are O(n^2) and will hit
% `max_derived_tuples`; that is the honest cost of asking about every pair.

reaches(A, B) :- calls(A, B).
reaches(A, C) :- reaches(A, B), calls(B, C).
recursive(S)  :- reaches(S, S).

% --- location helpers --------------------------------------------------
file_of(S, F)  :- def(S, F, _, _).
defines(F, S)  :- def(S, F, _, _).
at(S, F, L)    :- def(S, F, _, _), def_span(S, L, _, _, _).

% --- containment closure -----------------------------------------------
within(C, P) :- parent(C, P).
within(C, A) :- parent(C, P), within(P, A).

% --- tests -------------------------------------------------------------
% `file(F, _)` is not decoration: without it F is a head variable bound only
% by a filter, which violates range restriction exactly as symbol_at did.
is_test(F) :- file(F, _), match(F, "(^|/)tests?/").
is_test(F) :- file(F, _), match(F, "_test\\.(go|py|rs)$").
is_test(F) :- file(F, _), match(F, "(^|/)test_[^/]*\\.py$").
is_test(F) :- file(F, _), match(F, "\\.(test|spec)\\.(ts|tsx|js|jsx)$").
is_test(F) :- scip_ref(_, F, _, _, _, "test").

% --- orientation: everything about one symbol, in one round trip -------
% Heterogeneous rows sharing a discriminator column. One query instead of
% six, and the definition of "orientation" stays editable.

about(S, "sig",        Sig, "", 0) :- def_sig(S, Sig).
about(S, "doc",        Doc, "", 0) :- def_doc(S, Doc).
about(S, "defined",    F,   N,  L) :- def(S, F, _, N), at(S, F, L).
about(S, "parent",     F,   N,  L) :- parent(S, P), def(P, F, _, N), at(P, F, L).
about(S, "caller",     F,   N,  L) :- calls(C, S), def(C, F, _, N), at(C, F, L).
about(S, "callee",     F,   N,  L) :- calls(S, C), def(C, F, _, N), at(C, F, L).
about(S, "implements", F,   N,  L) :- implements(S, T, _), def(T, F, _, N), at(T, F, L).
about(S, "implementor",F,   N,  L) :- implements(T, S, _), def(T, F, _, N), at(T, F, L).
about(S, "test",       F,   N,  L) :- calls(C, S), def(C, F, _, N), is_test(F), at(C, F, L).
about(S, "extern",     P,   V,  0) :- extern(S, _, P, V).

% --- file-level dependency ---------------------------------------------
depends(F, G) :- ref(S, F, _, _, _, _, _), def(S, G, _, _), F != G.

% --- the dex "34 methods", as rules ------------------------------------
dead_export(S)   :- exported(S), def(S, F, _, _),
                    !ref_outside(S, F).
% `def(S, F, _, _)` is what binds F. Without it F appears in the head and only
% in a comparison, so the rule is not range-restricted and the engine rejects
% its own standard library -- found at M1 by the one-line gate that loads this
% file. F is S's defining file at every call site, so naming it is the honest
% reading as well as the fix.
ref_outside(S, F):- def(S, F, _, _), ref(S, G, _, _, _, _, _), G != F.

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
ref_outside_exact(S, F) :- def(S, F, _, _),
                           ref(S, G, _, _, _, _, "exact"), G != F.
```

That distinction is the entire point of the `Prov` column. **Every rule that
reads `calls` or `ref` is contaminated by it, and every one of them ships in
both forms** — `dead_export`, `entrypoint`, `depends`, `uses_package`, and
`about`'s `caller`/`callee`/`test` rows:

```prolog
entrypoint_exact(S)    :- def(S, _, K, _), callable(K), resolved(S),
                          !calls_exact(_, S).
depends_exact(F, G)    :- ref(S, F, _, _, _, _, "exact"), def(S, G, _, _), F != G.
uses_package_exact(F,P):- ref(S, F, _, _, _, _, "exact"), extern(S, _, P, _).
```

`entrypoint` is the sharpest case and the reason this list is exhaustive rather
than illustrative. Over a `name`-provenance graph it does not degrade, it
**inverts**: an unresolved call is an invisible call, so almost every function
in the repo reports as an entrypoint. An earlier draft claimed every
contaminated rule shipped in both forms while shipping only `dead_export` that
way.

---

## Worked queries

```prolog
% COLD START. You have a location (ripgrep, git diff, a stack trace) and
% no symbol. Lift it into the graph, then traverse.
?- innermost_at("src/store.rs", 142, S), about(S, Rel, A, B, L).

% Blast radius of changing a symbol, precise only, with locations.
?- def(S, _, _, "get"), impact_of_exact(S, C), def(C, F, _, N), at(C, F, L).

% What does this diff hunk affect? (seed from `git diff --unified=0`)
?- innermost_at("src/store.rs", 142, S), impact_of(S, C),
   def(C, F, _, N), at(C, F, L), !is_test(F).

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
| `scip_ref` | 5M | 6 | 120 MB |
| `name_ref` | 5M | 5 | 100 MB |
| `def` | 1M | 4 | 16 MB |
| `def_span` | 1M | 5 | 20 MB |
| everything else | ~2M | ~3 | ~24 MB |
| **stored total** | | | **~280 MB** |
| derived `ref` (in memory, per query session) | ~8M | 7 | ~220 MB |

Comfortably resident. This is why the engine is in-memory and why fixed-width
`u32` tuples are the right representation ([04-storage.md](04-storage.md)).

Note that `ref` is now derived, so it costs memory at query time rather than
disk at index time. If that materialization ever dominates a profile, the fix is
to cache the derived relation alongside the segments — not to move resolution
back into the extractor, which would reintroduce the incremental-soundness bug
that `name_ref` exists to prevent.

---

## Changelog

| schema_version | Change |
|---|---|
| 1 | Initial. 15 relations. `ref` is derived from `scip_ref` + `name_ref`; tier A does not resolve names. |
