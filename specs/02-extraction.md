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

### Adding a language is data, not code

Upstream grammars ship `queries/tags.scm` with a stable capture convention. We
vendor it, add a small `imports.scm` of our own, and register the grammar. That
is the entire per-language cost.

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
7. `@reference.call` captures → `ref` rows with `Role = "read"`, `Prov = "name"`,
   `From` = enclosing definition by the same span sweep, and `S` resolved by the
   name-matching rule below.

### Name matching, and its honest limits

A tier-A reference resolves to a `SymId` by this rule and no other:

1. A definition with the same `Name` in the same file → that symbol.
2. Otherwise, if exactly one indexed definition anywhere in the repo has that
   `Name` and is `exported` → that symbol.
3. Otherwise → **no `ref` row is emitted.**

Rule 3 is deliberate. dex tried harder — per-language import tables, a
TypeScript constructor-dependency-injection special case — and still produced a
measurably skewed graph ([research.md](../docs/research.md) §1c). **We do not
compete with compilers.** An ambiguous name produces nothing, tier B produces
the truth, and the `Prov` column tells the query which it got.

Explicitly *not* attempted in tier A: receiver/method dispatch (`x.method()`),
interface dispatch, generic instantiation, re-export chains, dynamic dispatch,
aliased imports crossing files.

### Budget

Skip a file if: binary, > 2 MB, matched by `.gitignore`/`.ignore` (via the
`ignore` crate), or its grammar is unregistered. Skipped files produce **zero**
facts — not a `file` row — so `!file(F, _)` means "not indexed", which is
different from "indexed and empty".

---

## Tier B — SCIP

### Acquisition

`codeintel` does **not** run indexers. It reads `index.scip` if present, at
`--scip <path>` or the default `./index.scip`. Running `scip-typescript index`
or `rust-analyzer scip .` is the user's job — those commands need the project's
own toolchain, dependencies, and build config, and owning that is a second
product.

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
| `SymbolInformation.enclosing_symbol` | `parent(S, Enclosing)` |
| `SymbolInformation.documentation[]` | `def_doc` (if tier A did not supply one) |
| `SymbolInformation.signature_documentation.text` | `def_sig` (if tier A did not supply one) |
| `Occurrence` without `Definition` role | `ref(S, F, L, C, From, Role, "exact")` |
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

A call site visible to both tiers produces two `ref` rows, `"name"` and
`"exact"`. **Keep both.** They are different facts. `calls(A,B)` deduplicates
naturally because it projects `Prov` away; `calls_exact` selects the precise one.
Collapsing them at ingest would destroy the ability to ask "what does tier A see
that tier B missed" — which is the diagnostic for a broken or stale SCIP index.

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
4. **Tier-A precision.** On a fixture with both tiers, every `"name"` `ref` must
   either match an `"exact"` `ref` at the same position or be listed in a
   `known-imprecise.txt` with a reason. This bounds tier A's false-positive rate
   with a number instead of a hope.
5. **Idempotence.** Index twice → byte-identical segments.
6. **Determinism across order.** Index with files shuffled → identical facts.
