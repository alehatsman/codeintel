# Implementation plan

Seven milestones, strict order. Each is independently shippable, leaves `main`
green, and has a **done-when** that is checkable by running something — not by
reading code.

The ordering is a real dependency chain, not a preference. M1 before M2 because
the engine must be provably correct before code-intel facts can hide a bug in
it. M3 before M4 because tier B anchors against tier A's identifier index.

```
M0 ──► M1 ──► M2 ──► M3 ──► M4 ──► M5 ──► M6
skel   engine  tier A  tier B  surface  langs  perf
```

---

## M0 — Skeleton

Workspace, crate seams, CI.

```
crates/datalog  crates/facts  crates/extract  crates/codeintel
queries/  rules/  tests/fixtures/
```

**Done when**
- `cargo build --workspace` and `cargo test --workspace` are green.
- `cargo clippy --workspace -- -D warnings` is green.
- CI runs build, test, clippy, `cargo fmt --check` on every push.
- A test asserts `crates/datalog/Cargo.toml` depends on neither `facts` nor
  `extract`. This seam is the thing that keeps the engine honest; guard it
  mechanically from commit one.

---

## M1 — Datalog engine

[03-datalog.md](../specs/03-datalog.md), end to end, with **no code-intel
concepts**. This is the highest-risk milestone; do it first and do it properly.

1. Interner (`facts` crate): reserved integer range, append-only dict, mmap load.
2. Lexer + parser → AST. Fuzz it.
3. Safety checks: range restriction, negation safety, comparison safety,
   aggregate safety, arity consistency. Each with its diagnostic.
4. Stratification: dependency graph, negative-edge cycle detection, topo order.
5. `Relation`: flat `Vec<u32>`, sorted, deduplicated, binary search on prefix.
6. Semi-naive evaluation with the most-bound-first literal ordering.
7. Builtins: comparisons, arithmetic, `match`/`prefix`/`suffix`/`contains`, the
   four aggregates.
8. Limits, truncation reporting, `Stats`.

**Done when**
- The conformance suite passes: transitive closure, same-generation, stratified
  negation, all four aggregates, arithmetic.
- The `#[cfg(test)]` naive evaluator agrees with semi-naive on every conformance
  program. *This is the acceptance gate* — a semi-naive bug that drops rows is
  otherwise invisible until it corrupts a real answer.
- Every safety rule has a test asserting its exact diagnostic message.
- Every limit has a test provoking it and asserting `status` and `cap`.
- 100 runs of each conformance query are byte-identical.
- `cargo fuzz run parser` survives 10 minutes with no panic.
- No dependencies in `crates/datalog/Cargo.toml`.

---

## M2 — Tier A, one language

Rust only. Walk → parse → extract → segment → load → query.

1. `ignore`-based walk, binary/size filtering.
2. Vendor `queries/rust/{tags,imports}.scm`; write the kind-mapping table.
3. Span-nesting containment sweep. **Write this once, in a shared function** —
   tier B calls the same code in M4.
4. Emit `file`, `def`, `def_span`, `def_name`, `def_sig`, `def_doc`, `parent`,
   `exported`, `import`, and `"name"`-provenance `ref`.
5. Segment writer + manifest, per [04-storage.md](../specs/04-storage.md).
6. `codeintel index`, `codeintel query`.

**Done when**
- `codeintel index` on this repo, then
  `codeintel query '?- def(S, F, "function", N).'` returns real rows.
- Golden fact file for `tests/fixtures/rust/` matches exactly.
- Span exactness test passes: for every `def`, `src[start_byte..end_byte]`
  reparses to the same kind and `src` at `def_name` equals `Name`.
- Indexing twice produces byte-identical segments.
- Indexing with shuffled file order produces identical facts.
- Deleting a file and reindexing removes its facts.

---

## M3 — Tier B, one language

`rust-analyzer scip .` → facts → the anchor join.

1. `scip` crate; read `index.scip`.
2. Position normalization, including the UTF-16 case and its skip-and-report
   path.
3. `local N` → `local <path> N` rewrite. **Test this specifically** — the bug it
   prevents is silent and repo-wide.
4. `SymbolInformation.kind` → our 16 kinds.
5. Reference `From` attribution via the M2 containment sweep.
6. The anchor join: match on `def_name` position, rewrite the tier-A atom to the
   SCIP symbol in the interner, emit `resolved(S)`.
7. `implements`, `has_type`, `extern`.

**Done when**
- On `tests/fixtures/rust/` with both tiers, ≥ 95% of tier-A definitions carry
  `resolved(S)`.
- Tier-A precision test passes: every `"name"` ref either matches an `"exact"`
  ref at the same position or is listed in `known-imprecise.txt` with a reason.
- `?- calls_exact(A, B).` returns edges that `?- calls(A, B).` misses, and a
  hand-audited sample of 20 is correct in both directions.
- Deleting `index.scip` and reindexing degrades cleanly to `"name"` only, with
  `status: "no-scip"` on a query that needs precision.

---

## M4 — Surface and stdlib

1. `rules/stdlib.dl` — every rule in [01-facts.md](../specs/01-facts.md) §
   Derived relations, each with a doc comment and a fixture test.
2. `codeintel rules`, `schema`, `status`.
3. `codeintel mcp` — one tool, the full status taxonomy, hints on every
   non-`ok`.
4. The surface-budget CI test: assert 1 MCP tool, 6 CLI verbs, ≤ 16 base
   relations. A PR that adds a seventh verb fails CI and has to argue in a spec
   change.

**Done when**
- Every rule in `stdlib.dl` has a fixture test with a hand-verified answer.
- Every question in [research.md](research.md) §1a's 34-method list is
  answerable in ≤ 5 lines of Datalog. Write them all down in
  `docs/cookbook.md` — that file is the proof, and the artifact users actually
  read.
- `codeintel schema` output is under 1500 tokens, measured.
- **The agent test.** 10 held-out structural questions, phrased in English, in a
  repo none of the fixtures come from. An agent given only `codeintel schema`
  output must write a correct query for ≥ 8 on the first attempt. Record the
  failures verbatim in `docs/agent-eval.md` — a failure is a `schema` wording
  bug far more often than an agent bug, and the transcript is the evidence.
- Surface-budget test is in CI.

---

## M5 — Languages

Python, TypeScript, Go. Each is: vendor two `.scm` files, add a grammar crate,
add a `lang.rs` row, add a fixture with golden facts.

**Done when**
- Each language has a fixture passing golden-fact, span-exactness, anchor-rate,
  and tier-A-precision tests.
- A polyglot fixture (Rust + TS + Python in one tree, one SCIP index each)
  indexes and queries correctly across language boundaries.
- **Adding a language required no changes outside `queries/`, `lang.rs`, and
  `tests/fixtures/`.** If it did, the extractor is wrong; fix the extractor
  rather than special-casing the language. This criterion is the whole reason
  tier A is query-driven.

---

## M6 — Performance and incremental

Only now, with correctness fixed and something to measure.

1. Benchmark corpus: 3 pinned public repos, one per size decade (~10k, ~100k,
   ~1M LOC).
2. `codeintel bench` reporting cold index, warm load, single-file reindex, and
   query p50/p95 over a fixed query set.
3. Profile and fix what the numbers say — **not what seems slow.**

**Done when** (on the ~100k-LOC corpus entry)
- Cold tier-A index < 30 s.
- Warm load < 200 ms. If it is not, *then* build the merged-array cache in
  [04-storage.md](../specs/04-storage.md) § Loading, and not before.
- Single-file reindex < 1 s.
- Query p50 < 20 ms, p95 < 100 ms.
- Baselines committed as JSON; CI fails on a > 20% regression.

---

## Explicitly deferred

Recorded so a future agent knows these were considered, not overlooked.

| Deferred | Revisit when |
|---|---|
| Merged-array load cache | M6 measures warm load > 200 ms |
| Watch mode | single-file reindex measures > 1 s |
| Live LSP probe (`codeintel probe file:line`) | a real query needs a type at a position that SCIP does not carry |
| Multi-repo / cross-repo indexes | someone has the problem |
| More languages (Java, Ruby, C++, C#) | M5's "no changes outside three places" holds |
| Query result caching | a profile shows repeated identical queries |

## Explicitly never

From [00-overview.md](../specs/00-overview.md) § Scope. Listed again because a
plan document is where scope creep enters.

Embeddings. Vector search. LLM calls. Summarization. Text search as a lane.
Ranking or centrality scores of any kind. Git-history mining. Agent memory.
Context packing. An HTTP API.
