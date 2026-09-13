# Research

Date: 2026-09-12. Every claim below was verified against a live source on that
date; versions and dates are recorded so a future agent can tell staleness from
error.

## 1. The prior art we are reacting to: `dex`

`~/projects/dex` is a Go implementation of roughly this idea. It works. It is
also **84,887 lines of non-test Go**, and the shape of that number is the whole
reason this project exists.

Package sizes (`internal/`, non-test LOC):

| Package | LOC | What it is |
|---|---:|---|
| `mcp` | 17,763 | tool surface |
| `graph` | 10,146 | extraction (Go via `go/types`, 5 languages via tree-sitter) |
| `compress` | 7,388 | output compression |
| `store` | 6,725 | SQLite |
| `retrieve` | 5,126 | fusion ranking |
| `eval` | 4,745 | benchmark harness |
| `bench` | 2,922 | more benchmark harness |
| ...31 more | | embeddings, rerank, summarize, cohesion, feedback, shadow, throttle, redact, gotcha, rehearse |

### 1a. The accretion is legible in the source

`internal/store/store_graph.go` exposes **34 methods**, and most are a single
frozen graph question:

```
ExportedSymbolsByDir   TopCentralByDir      SelectSymbols        SymbolsByFile
GraphQualifiedNameAt   ExternalImports      UnresolvedInboundForFile
InternalPackageImports MainEntrypoints      PackageCentrality    ImportsForDir
ImportsForFile         UsedByPackages       FileCentrality       Smells
GraphCommunities       CallerFiles          ...
```

Each is 20–60 lines of hand-written SQL plus a Go result struct plus an MCP
wrapper plus a CLI mirror plus a spec paragraph. Each is also **two to five
lines of Datalog.** `MainEntrypoints`, `UsedByPackages`, and `Smells.dead_exports`
are rules, not features. That is the single strongest argument for the design in
[specs/03-datalog.md](../specs/03-datalog.md): the endpoint count grows with the
question count until someone stops it, and a query language stops it structurally.

dex's own `specs/roadmap.md` says the quiet part out loud — it describes dex as
having accreted "**three separate products** in one binary" and plans to delete
two of them, including an 8,160-line LLM API proxy. `specs/anti-accretion-lint.md`
is a *lint that counts how often tool descriptions argue with each other*. When
you need a lint for that, the surface has already lost.

### 1b. What dex got right — steal these

- **Structural facts are the load-bearing part.** Nodes (package, file,
  function, method, type, interface, struct, field, import) and typed edges
  (`contains`, `imports`, `has_method`, `has_field`, `embeds`, `implements`,
  `calls`) cover nearly every real question. Our schema is a direct descendant.
- **Byte spans on every symbol.** `graph_nodes` carries `signature`,
  `start_byte`, `end_byte`, `declaration_hash` so a consumer can slice an exact
  edit out of a file without reading it. Cheap to record, expensive to retrofit.
  We record it from day one.
- **Stable IDs.** dex keys nodes `<module>::<pkg>::<kind>::<qualified-name>` so
  reruns are idempotent. We use SCIP symbol strings for the same reason, with
  a better guarantee (see §2).
- **A status taxonomy, not errors.** `no-index` vs `no-graph` vs `not-found` vs
  `ok`, each with an actionable hint. A consumer branches on status instead of
  catching failures. We copy this verbatim.
- **`last_seen_at` + prune.** Re-index marks rows, then drops anything not seen
  this pass. Deleted files leave no ghosts.

### 1c. What dex got wrong — the expensive lesson

From `docs/architecture.md`, dex's own words:

> **Known limitation (v1): edge resolution is language-tiered.** Go `calls`
> edges are *type-resolved* via `go/packages` + `go/types` [...] The other
> languages are name-resolved by the tree-sitter extractors with no type info
> [...] receiver/method calls on typed values (Python `x.method()`, Rust
> `value.method()`, Java calls on typed receivers) are best-effort and
> frequently dropped.

And the consequence:

> Go nodes accrue more edges from better resolution, which systematically skews
> PageRank/betweenness centrality and Louvain clusters toward Go symbols in
> polyglot repos — an artifact of resolution accuracy, not real importance.

They measured it: `dex bench skew` on a pinned polyglot corpus shows Go at
**1.08** PageRank-mass-to-node-count ratio versus TypeScript at **0.69** — TS
holds ~19% of call-graph nodes but only ~13% of centrality mass.

**Three lessons, all of which shape our design:**

1. **Do not write your own cross-file name resolver per language.** dex wrote
   one per language (`sitter_python.go`, `sitter_ts.go`, `sitter_java.go`,
   `sitter_rust.go`, plus a TypeScript constructor-DI special case) and it is
   still wrong. The compilers already solved this; consume their output (§2).
2. **Never average across provenance.** Any score computed over a mixed-quality
   graph inherits the quality gradient and launders it into a number that looks
   objective. This is why `codeintel` ships **no scores at all** — no PageRank,
   no betweenness, no communities, no smell rankings. Facts carry an explicit
   `exact`/`name` provenance column and the query decides.
3. **The failure mode is silent.** A dropped edge looks exactly like "no
   caller". Our `ref` relation is provenance-tagged precisely so an agent
   asking a precision-critical question (rename impact, dead-code deletion) can
   write `Prov = "exact"` and get a *smaller, honest* answer rather than a
   larger, contaminated one.

dex also measured a second ceiling worth knowing: git co-change coupling is
**mostly non-structural**. Two-hop reachability from an anchor to its co-changing
files through calls/imports edges is ~43% on flask, ~21% on react, ~0% on
ripgrep and zod. History reveals coupling the structural graph does not. That is
a hard ceiling on what any code graph can tell you, and it is why "what changes
together" is explicitly out of scope rather than a tempting future feature.

---


### §1e — dex adopted this thesis, shipped it, and the surface did not collapse

Measured 2026-09-12, read-only, against `~/projects/dex`.

`specs/roadmap.md:1-10` states dex's own active plan: *"dex collapses to one
composable verb… The surface should be as small as that identity: one verb,
`query`, that composes."* Dated 2026-08-26. It shipped — `internal/mcp/query_pipe.go`
(639 LOC), `query.go` (630), `query_select.go` (255), and a git log showing
`feat: #206 query pipes MVP`.

Today:

| | |
|---|---:|
| MCP tools registered in `internal/mcp/server_register.go` | **20**, with `query` as the twentieth |
| `func (s *Store)` methods in `store_graph.go` | **34** |
| non-test Go LOC | **84,887**, unchanged after ~9,500 lines of deliberate deletion |
| pipe grammar | already accreting: `#210` selector seeds, `#219` `since:`/`diff:` seeds |

The composable verb was added **alongside** the nineteen endpoints, not instead
of them. Its stage vocabulary is a fixed `switch` over eleven cases, so question
N+1 is a new `case` in a Go parser needing a release.

**This is the strongest available evidence for codeintel, and it is evidence
about a mechanism rather than a line count.** A query language bolted onto an
existing endpoint surface does not remove the endpoints; someone has to, and in
a year nobody did. The defensible claim is therefore *not* "26x smaller" (a
ratio this document never stated and should never state) and *not* "Datalog
deletes 84,887 lines" (it plausibly addresses 5–13%, with a further ~34% removed
by scope decisions that needed no query language at all).

The claim is: **question N+1 is a line of data in a text file the user can edit,
not a branch in a parser that needs a release** — asserted in CI from commit
one, before there is anything to delete. That is testable, and
[plan.md](plan.md) M4 puts a rule-count assertion in the budget test so it can
fail.

### §1f — corrections to this document

- **§3's `tags.scm` verification checked existence, not sufficiency.** All nine
  upstream files were re-fetched 2026-09-12; the results and their consequences
  are in [02-extraction.md](../specs/02-extraction.md) § Adding a language.
  "Ships `tags.scm` upstream" does not imply "costs one table row", and for
  TypeScript, Python, Rust and Go it does not.
- **§6's maintenance signal used `pushed_at`, which bot branches defeat.**
  Default-branch human commits: `scip-python` **none since 2025-09-05**, with
  open bugs on `kind=UnspecifiedKind` and silently-dropped cross-package
  references; `scip-typescript` shipped symbol-kind emission on 2026-09-11.
  Python ships in v1 for its tier-A story; do not promise `calls_exact` quality
  there.
- **§6 omitted `blake3` and `regex`.** Fixed in M0, along with a full
  reconciliation against `rust-quality/docs/STACK.md`. Both are used —
  segments are named by a blake3 hash, and `regex` is a non-optional
  transitive dependency of `tree-sitter` itself, which also removes any
  reason to hand-roll a matcher.
  A budget `CLAUDE.md` calls "the entire budget" was wrong at spec time.
- **§4 answered "which Datalog?" and never asked "why Datalog?"** SQL over the
  same facts was never evaluated — SQLite was rejected as *storage* in §5 and
  DataFusion dismissed in one clause. The decision stands (a code-shaped
  recursive query language with stratified negation, honest provenance, and
  first-party diagnostics is the differentiator, and byte-exact truncation over
  arbitrary user SQL would require parsing it anyway) but it stands as a
  **decision**, not as a survey result.
- **Invariant 4's evidence is n=1.** `benchmark/skew/baseline-corpus.json` holds
  one repo — gotify, 485 nodes, 94 of them TypeScript. The conclusion is sound;
  the phrase "backed by a measurement" is carrying one small repo and should say so.


## 2. Resolution: consume SCIP, do not build a resolver

**SCIP** (SCIP Code Intelligence Protocol) is the successor to LSIF: a protobuf
format emitted by language-specific indexers that carries globally-unique symbol
identity plus every occurrence of every symbol.

Verified 2026-09-12:

| Fact | Evidence |
|---|---|
| `scip` Rust crate is live | crates.io 0.10.0, updated 2026-09-03, 2.52M downloads |
| Spec repo is active | `github.com/scip-code/scip`, pushed 2026-09-12, 796 stars |
| Now independently governed | Sourcegraph announced (March 2026) a Core Steering Committee with engineers from Uber and Meta; repos moved `sourcegraph/*` → `scip-code/*` |
| Indexers maintained | `scip-typescript` pushed 2026-09-11, `scip-python` 2026-09-11, `scip-clang` 2026-09-08 |
| Consumed in production | Sourcegraph, Mozilla Searchfox, Meta Glean, rust-analyzer |

Indexer coverage: TypeScript/JavaScript (`scip-typescript`), Python
(`scip-python`, pyright-based), Java/Scala/Kotlin (`scip-java`), Go (`scip-go`),
Ruby (`scip-ruby`, Sorbet), C/C++ (`scip-clang`), Rust (`rust-analyzer scip .`),
C# (`scip-dotnet`).

### What SCIP actually gives us

Fetched from `scip.proto` (2026-09-12). The load-bearing pieces:

- `Document { relative_path, language, occurrences[], symbols[] }`
- `Occurrence { typed_range, symbol, symbol_roles, syntax_kind, typed_enclosing_range }`
- `SymbolRole` bitmask: `Definition=0x1, Import=0x2, WriteAccess=0x4,
  ReadAccess=0x8, Generated=0x10, Test=0x20, ForwardDefinition=0x40`
- `SymbolInformation { symbol, kind, display_name, documentation[],
  signature_documentation, enclosing_symbol, relationships[] }`
- `Relationship { symbol, is_reference, is_implementation, is_type_definition,
  is_definition }`
- Symbol strings are structured: `<scheme> <manager> <package> <version>
  <descriptors...>`, e.g. `scip-go gomod github.com/x/y v1.2.0 pkg/Foo#Bar().`
  Descriptor suffixes: `Namespace Type Term Method TypeParameter Parameter Meta
  Local Macro`.

**Three consequences that shape the schema:**

1. **Symbol identity is free and global.** A SCIP symbol string is stable across
   files, across repos, and across the package boundary. We intern it and it is
   our `SymId`. No `<module>::<pkg>::<kind>::<name>` scheme to invent and no
   collision policy to get wrong.
2. **SCIP has no `calls` relation — and that is fine.** It has references.
   `calls(A,B)` is derivable: a non-definition occurrence of `B` whose position
   falls inside the `enclosing_range` of `A`'s definition, where `B` is callable.
   A per-file interval-containment sweep, one algorithm, ~60 lines. The
   derivation is a *Datalog rule*, which means an agent can see it, change it,
   or bypass it. That is strictly better than a baked-in extractor decision.
3. **`enclosing_symbol` and `relationships` come free.** Lexical parent and
   interface-implementation, two of the most expensive things to compute by
   hand, are fields on the wire.

Caveat to encode in the ingest: SCIP `local N` symbols are **document-scoped**.
They must be namespaced by path (`local 4` in `a.ts` ≠ `local 4` in `b.ts`) or
the graph silently cross-links unrelated locals.

### Rejected: live LSP

Running `rust-analyzer` / `gopls` / `tsserver` as a subprocess and asking
`textDocument/references` per symbol. Rejected:

- Stateful, slow to warm (rust-analyzer on a large workspace: minutes), and
  holds gigabytes resident.
- Per-symbol round-trips. Building a whole-repo graph means N requests where
  SCIP is one file read.
- No batch export in the protocol. LSP is designed for a cursor, not a corpus.
- Every server has different readiness semantics and different failure modes.

SCIP *is* the batch export of exactly this information, produced by the same
engines. An optional `codeintel probe <file>:<line>` that shells out to an LSP
for a single live answer is a reasonable **post-v1** escape hatch, and nothing in
the design forecloses it. It is not v1.

### Rejected: stack-graphs

GitHub's `stack-graphs` — incremental, compiler-free name resolution driven by
a per-language DSL over tree-sitter — is conceptually the ideal middle tier.
**The repository was archived (last push 2025-09-09).** Writing scope-graph DSLs
per language is also precisely the per-language-resolver trap dex fell into.
Rejected on both counts.

---

## 3. The universal tier: tree-sitter

Verified 2026-09-12: `tree-sitter` 0.27.0 (2026-08-30, 38M downloads),
`tree-sitter-tags` 0.27.0, grammars current (`tree-sitter-rust` 0.24.2,
`tree-sitter-python` 0.25.0, `tree-sitter-go` 0.25.0, `tree-sitter-typescript`
0.23.2).

The finding that matters: **upstream grammars already ship
`queries/tags.scm`** using a stable capture convention. Fetched from
`tree-sitter-rust/queries/tags.scm`:

```scheme
(struct_item   name: (type_identifier) @name) @definition.class
(function_item name: (identifier)      @name) @definition.function
(trait_item    name: (type_identifier) @name) @definition.interface
(mod_item      name: (identifier)      @name) @definition.module
(macro_definition name: (identifier)   @name) @definition.macro
```

and `tree-sitter-python/queries/tags.scm` adds `@reference.call`.

So adding a language is: vendor two `.scm` files, add one grammar crate, add one
row to a table. **No per-language Rust code.** That is the difference between
dex's 10,146-line `internal/graph` and our ~400-line extractor.

`tags.scm` gives definitions and (sometimes) call references. It does not give
imports or containment. We add:

- A ~10-line per-language `imports.scm` of our own.
- Containment for free: `@definition.*` captures are *nodes*, so we have their
  byte spans, and parent = innermost enclosing definition span. **The same
  interval-containment sweep that attributes SCIP references to their caller.**
  One algorithm, two uses.

---

## 4. Datalog engine: build it

The requirement is runtime-evaluated query strings — an agent composes a query
it was not compiled with. That eliminates most of the field.

Verified 2026-09-12:

| Candidate | Status | Verdict |
|---|---|---|
| **CozoDB** | repo last push **2024-12-04**; crate `cozo` 0.7.6, **updated 2023-12-11**, 111k downloads | **Dead.** Also: RocksDB dependency tree, and CozoScript is a non-standard dialect we would have to teach every agent. |
| **Ascent** | crate 0.8.1, 2026-08-29; repo active 2026-08-29, 582★ | Healthy, but a **proc-macro**: rules are fixed at compile time. Powers parts of rustc's borrow checker. Cannot evaluate an agent-authored query string. |
| **crepe** | crate 0.2.0, 2025-12-14; repo 2025-12-14, 530★ | Same proc-macro limitation. |
| **datafrog** | crate 2.0.1 **published 2019-01-02**; repo `rust-lang/datafrog` pushed 2026-08-19, 896★ | Not an engine — a join library. No parser, no rules, no runtime. Its `Variable<Tuple>` API is monomorphic, which is hostile to runtime-determined arity. **But its algorithm is exactly right.** |
| **DDlog** | archived by VMware | Dead. |
| **DataFusion** | healthy | SQL, not Datalog. Recursive CTEs would technically work; the dialect is wrong for the stated goal and the dependency is enormous. |

**Decision: write our own, ~1200 LOC, zero dependencies.** Rationale:

- Runtime query strings are non-negotiable; only a hand-rolled evaluator or Cozo
  provides them, and Cozo is dead.
- Our problem is unusually easy for a Datalog engine: fixed small arity, all
  columns are `u32` after interning, the whole fact set fits in memory (a
  1M-symbol repo is roughly 5M `ref` tuples × 7 columns × 4 bytes ≈ 140 MB), no
  transactions, no concurrent writers, no durability requirement beyond "rebuild
  it".
- We control determinism, row/time budgets, and truncation reporting — all of
  which matter for an agent-facing tool and none of which a general DB gives us.
- Semi-naive evaluation with stratified negation over sorted `Vec<[u32; N]>` and
  sort-merge joins is a well-understood, ~1200-line program. `datafrog` is the
  reference implementation of the inner loop, at ~500 lines, MIT/Apache, and we
  copy its approach without taking the dependency.

Syntax: Prolog/Souffle-flavoured (`head :- body.`), because that is the dialect
with the most public documentation and therefore the one an LLM is most likely
to write correctly on the first try. See [specs/03-datalog.md](../specs/03-datalog.md).

---

## 5. Storage: own columnar segments

**Decision: per-file fixed-width `u32` segments plus a global append-only string
dictionary, `mmap`-ed on load.** Rejected SQLite (a C dependency, schema
migrations, and a parse/load cost on a store that is 100% derived and trivially
rebuildable) and in-memory-only (unusable for a CLI on a large repo).

Why it is small: after interning, every fact is a fixed-width row of `u32`.
There is nothing to serialize — the on-disk bytes *are* the in-memory
representation. Loading is `mmap` + concatenate. Incremental reindex is "rewrite
one file's segment". Corruption recovery is `rm -rf .codeintel && codeintel index`.

`memmap2` 0.9.11 (2026-06-22) is the one dependency this requires.

---

## 6. Dependency budget

Reconciled against `rust-quality/docs/STACK.md` at v0.5.0 during M0.
**STACK.md outranks this table on crate choice**; every deviation below names
its trigger, because deviating is fine and deviating silently is not.

The whole binary, as specified:

| Crate | Why | Source |
|---|---|---|
| `tree-sitter` | parsing | 0.27.0, 2026-08-30 — no STACK entry |
| 4 grammar crates (below) | grammars | no STACK entry |
| `scip` | SCIP bindings and the descriptor parser | 0.10.0, 2026-09-03 — **deviation**, see below |
| `protobuf` | required by `scip`, and by us directly | 3.7 — see below |
| `memmap2` | segment loading | 0.9.11 — **deviation**, see below |
| `blake3` | segments are named by a content hash (`04-storage.md`) | STACK default (`sha2`/`blake3`) |
| `regex` | the `match`/`prefix`/`suffix`/`contains` builtins | STACK default; already transitive via `tree-sitter` |
| `serde` + `serde_json` | JSON output, manifest | STACK default |
| `clap` | CLI | STACK default, `features = ["derive", "env"]` |
| `anyhow` | application top level | STACK default |
| `thiserror` | one error enum per library boundary | STACK default |
| `ignore` | gitignore-aware walking | no STACK entry |
| `fd-lock` | the advisory writer lock (`04-storage.md` § Concurrency) | 4.0 — **deviation**, see below |

Dev-dependencies, all STACK defaults: `insta` (golden facts and rendered
output), `assert_cmd` + `predicates` (CLI), `rstest`, `proptest` (engine
invariants), `tempfile`. Benchmarks are a test-only harness, never a CLI verb.

**No `divan` and no `criterion`, against STACK's benchmark row.** Decided when
the harness was built ([plan.md](plan.md) M6 § Pulled forward). Both answer "how
long does this function take" with statistics over repeated calls. M6's
question is different: p50/p95 over a whole pipeline — index, load, query, the
CLI — on a pinned corpus, compared against a baseline **committed as JSON**.
Neither crate emits that format, and both would bring a dependency tree to
produce numbers that `std::time::Instant` plus the `serde_json` already in the
tree produce directly. The regression gate CI actually runs is not a timer at
all: it is exact counters in a nextest test. Trigger: the first
*microbenchmark* of an engine internal — `Relation::select`, the interner's
lookup — is where `divan` earns its place.

The Datalog engine, the fact store, the interner, the extractors, and the MCP
server are all first-party. No async runtime, no database, no HTTP client, no
model backend.

### Deviations from STACK.md, and their triggers

- **`crates/datalog` takes no dependencies at all.** Trigger: it is the seam
  that keeps code-intel concepts out of evaluation (`00-overview.md`
  invariant 6, asserted by `crates/datalog/tests/seam.rs`), and it is the one
  crate we might publish standalone. Consequence: `match/2` delegates to
  `regex`, but as a **host builtin injected by `crates/codeintel`** — the engine
  names an interface, not a crate. Hand-rolling a backtracker would buy no
  dependency reduction and would lose linear-time matching.
- **`memmap2` has no STACK entry.** Trigger: after interning, a fact is a
  fixed-width row of `u32`, so the on-disk bytes *are* the in-memory
  representation (§5). Loading is `mmap` plus concatenate; a serialization
  library would be a parse step over data that needs none.
- **`scip` deviates from STACK.md's `prost`.** Trigger: the protobuf here is
  not a schema we own, it is SCIP, and the `scip` crate is that schema's
  canonical Rust binding. It also carries `parse_symbol`, which is what makes
  `02-extraction.md` § Parent precedence correct rather than approximately
  correct: a backtick-quoted descriptor name may contain `#`, `.` and `/`, so
  truncating a symbol by `rsplit` corrupts it. Taking `prost` instead would mean
  vendoring `scip.proto`, putting `protoc` on the build path, and hand-writing
  that parser. Cost: `protobuf` 3.7 as a second protobuf runtime alongside
  nothing — we have no other. STACK.md now carries a SCIP row of its own.
- **`scip` costs one duplicate build-time crate.** `protobuf` 3.7 pins
  `thiserror` 1.x, which pins `syn` 2 while the rest of the stack is on `syn` 3.
  `.gate/findings.jsonl` reports it as `duplicate-dep`, and it is accepted: both
  are proc-macro crates, nothing is duplicated at runtime, and the alternative is
  the `prost` path above. Revisit when `rust-protobuf` moves to `thiserror` 2.
- **`protobuf` is a direct dependency, not only a transitive one.** Trigger:
  `scip` deliberately does not re-export its runtime, and parsing an index or
  reading a `symbol_roles` bitset needs it by name. Same version `scip`
  resolves, so it is one runtime and not two.
- **`fd-lock` has no STACK entry.** Trigger: `04-storage.md` § Concurrency
  requires an advisory `flock` on `.codeintel/lock`, `query`'s auto-refresh is a
  writer and must take the same one, and std has no file locking at all. It was
  chosen over `rustix` because it keeps Windows reachable — that spec puts
  Windows out of scope for v1, and a lock that forecloses it would make the
  decision permanent for no gain.
- **`tree-sitter`, the grammar crates, `scip`, `protobuf` and `ignore` have no
  STACK entry.** They are the domain, not a stack choice. Recorded here so the
  next reader does not go looking for a ruling that does not exist.
- **No `tracing` / `tracing-subscriber`, against STACK's focused-CLI default.**
  Trigger: degradation is a `status` field on the response, never a log line
  (invariant 6), and stdout is the tool's output. Revisit at M4, when
  `codeintel mcp` becomes a long-running process with no stdout to spare — that
  is the first time a log has somewhere to go.
- **`blake3` and `regex` were missing from this budget at spec time** and are
  now listed, per §1f. A budget `CLAUDE.md` calls "the entire budget" has to be
  complete to mean anything.
- **No MCP SDK — the transport is hand-written.** Decided at M4, when
  `codeintel mcp` was built. STACK.md has no entry, so this is a first ruling
  rather than a deviation. The stdio transport is newline-delimited JSON-RPC
  2.0; the surface this server needs is `initialize`, `tools/list`, `tools/call`
  and `ping`; `serde_json` is already a dependency for the response contract;
  and there is exactly **one tool**, so none of what an SDK offers — routing
  across many tools, schema derivation, capability negotiation beyond a single
  flag — is load-bearing here. The whole transport is ~150 lines in
  `crates/codeintel/src/mcp.rs`. Revisit if a second transport is needed, or if
  the protocol revision this server pins (`2024-11-05`) starts costing more to
  track than an SDK would.
- **Still no `tracing`, re-checked at M4.** The revisit trigger recorded above
  was "when `codeintel mcp` becomes a long-running process with no stdout to
  spare". It is now that process, and the answer is still no: stdout carries
  JSON-RPC frames, stderr is free, and every degradation an operator needs is a
  `status` plus a `hint` on the response itself. A log would be a *second*
  channel saying what the response already says, which is how the two start
  disagreeing.

### Language coverage

**Four languages, not nine** ([plan.md](plan.md) M5). The nine-language table
this section carried was built on "every grammar ships `queries/tags.scm`
upstream, so each costs one crate, two vendored `.scm` files and one `lang.rs`
row". §1f retracts that: existence is not sufficiency, the `.scm` files are
**authored here**, and the same-author precedent — `~/projects/dex`, ~6,177 LOC
of extraction for five languages — prices a language at ~1,000 LOC plus fixture
work.

| Language | Grammar crate | SCIP indexer | Lands |
|---|---|---|---|
| Rust | `tree-sitter-rust` 0.24.2 | `rust-analyzer scip .` | M2 / M3 |
| Go | `tree-sitter-go` 0.25.0 | `scip-go` | M5a — **landed** |
| Python | `tree-sitter-python` 0.25.0 | `scip-python` | M5a — **landed** |
| TypeScript (+TSX) | `tree-sitter-typescript` 0.23.2 | `scip-typescript` | M5b |

Python's tier B is on notice: `scip-python` has had no human commit on its
default branch since 2025-09-05 and three open correctness bugs, one of which
silently drops cross-package references. It ships for its tier-A story and its
`imports.scm`; `calls_exact` quality is not promised there.

What `scip-python` 0.6.6 actually emitted on the fixture, 2026-09-13: 100%
anchor rate and every `calls` edge exact, so the on-notice status is about
maintenance, not about what it produces today. Two shapes are its own and are
recorded, not corrected. It writes no `display_name` and no `kind` for a
parameter, an attribute bound in `__init__`, or a module, so those `def` rows
are named from the symbol's own descriptor (02-extraction.md § Ingest) and
their kind is `unknown`. And it records the binding `from db.conn import open`
makes as a plain read of `open` at module scope, with no `Import` role bit, so
`calls` carries an edge whose caller is the file. The row is what the indexer
stated. rust-analyzer, for comparison, sets no role bits at all. It also declares
no position encoding and counts columns in UTF-16, as `scip-typescript` 0.4.0
does, and ingest had read those columns as bytes (#21); an undeclared column
is now kept only over an ASCII prefix ([02-extraction.md](../specs/02-extraction.md)
§ Position normalization).

**Ruby is dropped**, not deferred: `tree-sitter-ruby` has no import node —
`require` is an ordinary method call and idiomatic Rails autoloads without one —
so it fails at tier A, at tier B, and at the conformance capability that is the
headline. **C, C++ and Java are deferred**; C and C++ are import-complete and
call-empty, which makes them the strongest candidates for the language after
M5b, added for conformance with `calls` declared unsupported in `status` rather
than silently empty.

One risk to check at M5: the grammar crates span tree-sitter ABI versions
(0.23.x through 0.25.x against a 0.27 runtime). tree-sitter maintains ABI
compatibility across a range, but a grammar should be smoke-loaded before
anything is built on it. Checked for Go and Python: `tree-sitter-go` 0.25.0 and
`tree-sitter-python` 0.25.0 load and their queries compile against the 0.27
runtime, both asserted by tests over every registered language rather than over
Rust alone.

`scip-go` has **moved out of the Sourcegraph org**. It installs from
`github.com/scip-code/scip-go/cmd/scip-go`; the old `sourcegraph` path fails
`go install` with a module-path conflict, not a 404, so the error names
neither the move nor the replacement. Recorded in
[02-extraction.md](../specs/02-extraction.md) § Acquisition, which is where
someone hits it.

---

## Sources

- [dex — local source at `~/projects/dex`](file:///Users/aatsman/projects/dex) (`docs/architecture.md`, `specs/graph.md`, `specs/storage.md`, `specs/roadmap.md`, `specs/anti-accretion-lint.md`, `internal/store/store_graph.go`, `internal/graph/graph.go`)
- [SCIP Code Intelligence Protocol](https://scip-code.org/)
- [github.com/scip-code/scip](https://github.com/scip-code/scip) — `scip.proto`
- [The future of SCIP — Sourcegraph](https://sourcegraph.com/blog/the-future-of-scip)
- [scip crate — crates.io](https://crates.io/crates/scip)
- [github.com/cozodb/cozo](https://github.com/cozodb/cozo)
- [github.com/rust-lang/datafrog](https://github.com/rust-lang/datafrog)
- [github.com/s-arash/ascent](https://github.com/s-arash/ascent)
- [github.com/ekzhang/crepe](https://github.com/ekzhang/crepe)
- [github.com/github/stack-graphs](https://github.com/github/stack-graphs) (archived)
- [tree-sitter crate — crates.io](https://crates.io/crates/tree-sitter)
