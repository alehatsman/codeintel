---
id: extraction
status: proposed
binding: no
---
# 02 — Extraction

Two tiers produce the facts in [01-facts.md](01-facts.md). This spec defines
what each emits, how they are joined, and what each is allowed to guess (almost
nothing).

```
                tier A — tree-sitter                 tier B — SCIP
                ────────────────────                 ─────────────
  runs          always, no build required            when index.scip exists
  cost          ~30s / 100k LOC                      whatever the indexer costs
  gives         defs, spans, signatures, docs,       global symbol identity,
                containment, imports                 references, implements, types
  precision     identifier text                      compiler front-end
  Prov          "name"                               "exact"
```

Neither tier is optional-in-principle: tier A is the skeleton (where things are),
tier B is the nervous system (what refers to what). Tier A alone gives a usable
map. Tier B alone gives references with no spans, no signatures, and no docs.

---

## Tier A — tree-sitter

### Adding a language is data — but the data is ours to write

**Superseded 2026-09-12.** The original claim was that upstream grammars ship
`queries/tags.scm` with a stable capture convention, so vendoring it plus a
small `imports.scm` was the entire per-language cost. All nine upstream files
were fetched and checked. The convention is not stable and vendoring does not
work:

| grammar | `@reference.call` | what breaks |
|---|---:|---|
| rust | 3 | `struct_item`, `enum_item`, `union_item` **and** `type_item` all capture as `@definition.class`, so every enum and type alias is emitted `Kind = "struct"` — an extractor guessing **wrong**, which invariant 1 forbids. `impl_item` is `@reference.implementation`, not a definition, so tier-A `parent` for every method is the file. Methods in a `declaration_list` match both `@definition.method` and `@definition.function` and emit two `def` rows. No `@definition.constant`. |
| go | 1 | emits `@definition.type`, which is absent from the kind table below; five patterns carry a bare `@name` with no tag at all, and emission is undefined for them |
| python | 1 | **no `@definition.method`** — every method is `Kind = "function"`, so `?- def(M,_,"method",N).` returns zero rows with `status: ok` |
| javascript | 2 | uses `#strip!`, `#select-adjacent!` — `tree-sitter-tags` directives, not core `Query` predicates; silently no-ops |
| **typescript** | **0** | captures only ambient `.d.ts` forms. `class X {}`, `function f() {}`, `const f = () => {}` produce **nothing** |
| **c / c++** | **0** | import-complete, call-empty |
| ruby | 2 | **no import node at all** — `require` is a method call, and Rails autoloads with none |
| java | 1 | `@reference.call` attaches to the *arguments* node, so recorded positions point at `(args)` |

So: **we author `tags.scm` and `imports.scm` per language.** Upstream is a
reference to start from, not a dependency to vendor. Budget ~1,000 LOC of query
and mapping per language — the same order as
`~/projects/dex/internal/graph/sitter_rust_tags.go` and its siblings, which
spend ~6,177 LOC across five languages doing exactly this. That reprices M5 and
is why the language set is four, not nine.

`imports.scm` was always ours, which is why none of the above touches
`import/3` — and why architecture conformance is the capability that works on a
first run with no SCIP index.

The per-language cost is therefore:

```
queries/
  rust/tags.scm        vendored from tree-sitter-rust
  rust/imports.scm     ours, ~10 lines
  python/tags.scm
  python/imports.scm
  ...
```

`tags.scm` capture convention, already honoured upstream:

| Capture | Meaning |
|---|---|
| `@definition.function` `.method` `.class` `.interface` `.module` `.macro` `.constant` | the node is a definition; the capture suffix is the kind |
| `@name` | the identifier token within that definition |
| `@reference.call` | a call site |
| `@doc` | attached documentation (where the grammar provides it) |

Example, `tree-sitter-rust/queries/tags.scm`:
```scheme
(struct_item      name: (type_identifier) @name) @definition.class
(function_item    name: (identifier)      @name) @definition.function
(trait_item       name: (type_identifier) @name) @definition.interface
(mod_item         name: (identifier)      @name) @definition.module
(macro_definition name: (identifier)      @name) @definition.macro
```

Our `imports.scm` for the same grammar:
```scheme
(use_declaration argument: (_) @module) @import
(extern_crate_declaration name: (identifier) @module) @import
```

Kind mapping is a table, not a `match` arm per language:

| tags.scm capture | our `Kind` |
|---|---|
| `definition.function` | `function` |
| `definition.method` | `method` |
| `definition.class` | `class` (Rust/Go: `struct`, see § per-language overrides) |
| `definition.interface` | `interface` (Rust: `trait`) |
| `definition.module` | `module` |
| `definition.macro` | `macro` |
| `definition.constant` | `constant` |
| anything else | `unknown` |

Per-language overrides live in one `lang.rs` table: grammar, query paths, kind
remaps, export predicate, doc-comment prefix. ~20 lines per language.

### Emission

Per file, one parse, then:

1. Run `tags.scm`. Every `@definition.*` capture yields a `def` + `def_span`
   (the capture node's byte range) + `def_name` (the `@name` node's position).
2. **Containment by span nesting.** Sort definition spans by `(start_byte,
   -end_byte)`. Sweep with a stack: a definition's `parent` is the innermost
   enclosing definition, or the file path if none. This is the one non-trivial
   algorithm in tier A, it is ~30 lines, and **tier B reuses it verbatim** (§ Tier B).
3. `def_sig` = the definition's source text from `start_byte` up to the first
   body delimiter (`{`, `:` + newline, `=`, or end of line), whitespace-collapsed,
   capped at 512 bytes. Crude and honest — it is a display string, not a parse.
4. `def_doc` = contiguous comment lines immediately preceding `def_span.start`,
   marker prefixes stripped. Emitted only if non-empty.
5. `exported` per the language's export predicate.
6. Run `imports.scm` → `import` rows. The module specifier is captured **as
   written**; no resolution.
7. `@reference.call` captures → `name_ref(Name, F, Line, Col, From)` rows, with
   `From` from the same span sweep. **Tier A does not resolve names to symbols.**

### Tier A does not resolve names

Every tier-A fact is a function of **its own file and nothing else**. A name
occurrence becomes `name_ref(Name, F, Line, Col, From)` — the text, its
position, and its enclosing definition. Which symbol that name denotes is
decided by rules in `stdlib.dl` ([01-facts.md](01-facts.md) § Derived
relations), evaluated over the whole repo at query time.

This is not a stylistic choice. A resolution decision depends on what exists in
*other* files, so freezing it into a per-file fact makes incremental indexing
unsound: adding a second `foo` in file B invalidates a fact in file A, and file
A will never be re-extracted because file A did not change. Per-file facts must
be per-file functions.

The secondary benefit is that the resolution policy is four readable rules a
user can tighten, loosen, or delete, rather than a buried Rust decision.

**We do not compete with compilers.** dex tried — per-language import tables, a
TypeScript constructor-dependency-injection special case — and still produced a
measurably skewed graph ([research.md](../docs/research.md) §1c). Tier A records
what the grammar saw. Tier B records what the compiler knows. The `Prov` column
on the derived `ref` says which one answered.

Explicitly *not* attempted anywhere in tier A: receiver/method dispatch
(`x.method()`), interface dispatch, generic instantiation, re-export chains,
dynamic dispatch, aliased imports crossing files. In Rust, Python, TypeScript,
and Java `x.method()` is the dominant call form, so **tier A alone yields a
sparse and unrepresentative call graph.** That is stated plainly here, in
`status` output, and in the README, because a sparse call graph reporting
`status: ok` is indistinguishable from code that has no callers.

### Budget

Skip a file if: binary, > 2 MB, matched by `.gitignore`/`.ignore` (via the
`ignore` crate), or its grammar is unregistered. Skipped files produce **zero**
facts — not a `file` row — so `!file(F, _)` means "not indexed", which is
different from "indexed and empty".

---

## Tier B — SCIP

### Acquisition

By default `codeintel` reads `index.scip` if present, at `--scip <path>` or the
default `./index.scip`. It does not produce one.

**`codeintel index --run-indexers`** opts into producing one. It detects the
languages present, looks up the canonical indexer in a static table, checks the
binary is on `PATH`, and shells out. That is the whole feature — a table, a
`which`, and a subprocess.

| Language | Command | Needs |
|---|---|---|
| Rust | `rust-analyzer scip .` | a cargo workspace |
| TypeScript/JS | `scip-typescript index --infer-tsconfig` | `node_modules` installed |
| Python | `scip-python index . --output index.scip` | an environment with deps |
| Go | `scip-go` | a buildable module |
| Java/Scala/Kotlin | `scip-java index` | a working build |
| C / C++ | `scip-clang --compdb-path compile_commands.json` | a compilation database |
| Ruby | `scip-ruby` | a Sorbet-typechecked project |

Rules that keep this from becoming a build system:

- **Never implicit.** Without the flag, nothing is executed. An indexer can run
  arbitrary build code; that needs an explicit opt-in every time.
- **No environment management.** We do not install indexers, create virtualenvs,
  run `npm install`, or repair builds. A missing binary or a failing indexer is
  reported with its stderr and the exact command, and indexing continues with
  tier A only. It is never fatal.
- **The table is data.** Adding a language adds a row, not a code path.
- **Timeout, default 600 s**, after which the child is killed and reported.

Without the flag, `index` and `status` still **print the exact command** for
each detected language. Not "no SCIP index found" — the literal, copy-pasteable
line. The gap between tier A and tier B is the difference between a symbol map
and a call graph, and a user must never have to go looking for how to close it.

`codeintel status` reports SCIP presence, the emitting tool from
`Metadata.tool_info`, file coverage (`documents` matched against indexed files),
and staleness (index mtime vs. newest source mtime).

Multiple `index.scip` files (polyglot repos, one per language) are supported:
`--scip a.scip --scip b.scip`. They are ingested independently; symbol strings
are globally unique by construction so no merge logic is needed.

### Ingest

From `scip.proto` (field names verbatim):

| SCIP | Our facts |
|---|---|
| `Document.relative_path`, `.language` | `file(F, Lang)` — `Lang` lowercased from the `Language` enum |
| `Occurrence` with `symbol_roles & Definition` | a definition site; `symbol` → `SymId` |
| `SymbolInformation.kind` | `Kind`, via the mapping table below |
| `SymbolInformation.display_name` | `Name` |
| symbol-string descriptor prefix, else `SymbolInformation.enclosing_symbol` | `parent(S, Owner)` — replaces tier A's row, see § Parent precedence |
| `SymbolInformation.documentation[]` | `def_doc` (if tier A did not supply one) |
| `SymbolInformation.signature_documentation.text` | `def_sig` (if tier A did not supply one) |
| `Occurrence` without `Definition` role | `scip_ref(S, F, L, C, From, Role)` |
| `symbol_roles` bits | `Role`: `WriteAccess`→`write`, `ReadAccess`→`read`, `Import`→`import`, `Test`→`test`, `Generated`→`generated`, `ForwardDefinition`→`forward`, `Definition`→`def` |
| `Relationship.is_implementation` | `implements(S, T, "exact")` |
| `Relationship.is_type_definition` | `has_type(S, T, "exact")` |
| symbol string `Package{manager,name,version}` ≠ project package | `extern(S, Manager, Pkg, Version)` |
| every ingested definition | `resolved(S)` |

`SymbolInformation.kind` has **80+ values**; we collapse them:

```
Function Macro                                   -> function / macro
Method AbstractMethod StaticMethod SingletonMethod
  TraitMethod ProtocolMethod PureVirtualMethod
  MethodSpecification TypeClassMethod Getter
  Setter Accessor MethodAlias                    -> method
Constructor                                      -> constructor
Class SingletonClass                             -> class
Struct                                           -> struct
Interface Protocol                               -> interface
Trait TypeClass Concept                          -> trait
Enum Union                                       -> enum
Field Property StaticField StaticProperty
  StaticDataMember Key                           -> field
Constant EnumMember                              -> constant
Variable StaticVariable Value Object Instance    -> variable
Module Namespace Package PackageObject File      -> module
Type TypeAlias TypeFamily DataFamily
  AssociatedType                                 -> typealias
everything else                                  -> unknown
```

### Position normalization

SCIP ranges carry a `PositionEncoding` (`UTF8` / `UTF16` / `UTF32` code units
from line start), and `Document.position_encoding` may differ per document.
**Normalize on ingest** to our convention (1-based line, 0-based UTF-8 byte
column) using the document's `text` when present, or the file on disk otherwise.

If a document specifies UTF-16 and its source is unavailable, **skip that
document and report it** in `status` — do not emit approximate columns. A
half-byte-wrong column silently breaks every edit built on it.

Prefer `single_line_range` / `multi_line_range` over the deprecated
`repeated int32 range` field; support both, since indexer versions vary.

### Deriving `From` for references

SCIP does not say which function a reference sits inside. We compute it with
**the same span sweep tier A uses for containment**:

1. Collect every definition's span in the file. Tier A supplies it for files it
   parsed. For files only tier B covers, use the definition occurrence's
   `typed_enclosing_range` (SCIP's full-definition range).
2. Sort by `(start, -end)`, sweep with a stack, and for each reference position
   take the innermost enclosing definition. No enclosing definition → `From = F`.

Both tiers, one algorithm, one test suite. This is also why `calls` is a Datalog
rule rather than an extractor output: the extractor emits *where a reference is*,
and the rule decides what counts as a call.

### Parent precedence

Both tiers can supply a parent and they disagree in real cases — most visibly a
Go method, which is lexically at file scope but semantically owned by its type.
`parent` carries the **semantic owner**, resolved by a fixed order
([01-facts.md](01-facts.md) § `parent`):

1. Drop the last descriptor from the SCIP symbol string. `pkg/Store#Get().` →
   `pkg/Store#`. If that names an indexed definition, it is the parent.
   Preferred because the descriptor grammar is mandatory, so every indexer
   supplies it.
2. Else `SymbolInformation.enclosing_symbol`, which is how SCIP `local` symbols
   get an owner.
3. Else tier A's span nesting.

Tier B **replaces** tier A's row rather than adding one, so the
one-row-per-definition guarantee holds. Replacement happens after the anchor
join, when the tier-A symbol already carries its SCIP identity.

Descriptor truncation must be done on the parsed descriptor list, not by string
surgery — descriptor names can contain `#`, `.`, and `/` when backtick-quoted
(SCIP escapes them as `` `name with spaces` ``), so a naive `rsplit` on the
suffix character corrupts them.

### Local symbols

SCIP `local N` symbols are **document-scoped**. Rewrite them on ingest to
`local <relative_path> N` before interning. Skipping this silently cross-links
unrelated locals across every file in the repo.

---

## The anchor join

When both tiers cover a file, identity is unified so there is exactly one `def`
row per definition.

```
for each SCIP definition occurrence D in file F:
    find the tier-A definition whose def_name position == D's range start
    if found:  that def adopts D.symbol as its SymId; emit resolved(S)
    if absent: emit a tier-B-only def from SymbolInformation (no span/sig)
```

Matching on the **identifier position** (`def_name`), not the full span, is what
makes this robust: the two tiers disagree about whether a span includes
decorators, attributes, and doc comments, but they always agree on where the
name token starts.

Rewriting is done in the interner: the tier-A synthetic symbol string is mapped
to the SCIP symbol's atom id before any facts are written, so every tier-A fact
referencing it (`def_span`, `def_sig`, `parent`, `exported`, `ref.From`, ...)
lands on the resolved identity for free. Nothing is rewritten twice.

Ordering requirement: **tier A runs first, entirely, then tier B.** Tier B needs
tier A's `def_name` index to anchor against.

### Duplicate suppression

A call site visible to both tiers contributes a `name_ref` *and* a `scip_ref`,
so the derived `ref` relation carries two rows for it, `"name"` and `"exact"`.
**Keep both.** They are different facts. `calls(A,B)` deduplicates naturally
because it projects `Prov` away; `calls_exact` selects the precise one.

Collapsing them would destroy the ability to ask "what does tier A see that
tier B missed" — which is the diagnostic for a broken, partial, or stale SCIP
index, and is expressible as a query:

```prolog
?- name_ref(N, F, L, C, From), !scip_ref(_, F, L, C, _, _).
```

Rows here are either tier-A false positives or genuine tier-B gaps. Either way
the number should be small and stable; a jump means one of the tiers changed
behaviour.

---

## Validation

Per-language fixture repos under `tests/fixtures/<lang>/`, each with a
hand-written golden fact file.

1. **Golden facts.** `cargo test` extracts the fixture and diffs against
   `expected.facts` (sorted, text). Any extraction change shows up as a legible
   diff.
2. **Span exactness.** For every `def`, assert `src[start_byte..end_byte]`
   re-parses as a definition of the same kind, and `src` at `def_name` equals
   `Name`. Catches off-by-one and encoding bugs, which are otherwise invisible
   until someone applies an edit.
3. **Anchor coverage.** On a fixture with both tiers, assert ≥ 95% of tier-A
   definitions get `resolved(S)`. A drop means the anchor join is drifting.
4. **Tier-A precision.** On a fixture with both tiers, every derived `"name"`
   `ref` must either match an `"exact"` `ref` at the same position or be listed
   in `known-imprecise.txt` with a reason. This bounds tier A's false-positive
   rate with a number instead of a hope.
5. **Locality.** Every tier-A fact for file X must be reproducible by extracting
   file X *alone*, with no other file indexed. This is the mechanical guard on
   incremental soundness — it fails loudly the moment someone reintroduces a
   cross-file lookup into the extractor.
6. **Idempotence.** Index twice → byte-identical segments.
7. **Determinism across order.** Index with files shuffled → identical facts.
8. **Incremental equivalence.** Index the fixture; mutate one file; reindex
   incrementally. The resulting fact set must equal a full cold reindex, byte
   for byte. This is the test that would have caught the bug `name_ref` fixes.
