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

Workspace, crate seams, and the quality gate. **The gate comes first** — it is
far cheaper to adopt the lint block on an empty workspace than to retrofit it
across four crates.

```
crates/datalog  crates/facts  crates/extract  crates/codeintel
queries/  rules/  tests/fixtures/  tasks/
```

### The quality gate is not ours to invent

This repo is a consumer of [rust-quality](https://github.com/alehatsman/rust-quality)
— the shared lint block, clippy/rustfmt/cargo-deny config, cargo aliases, and
the two-mode gate — driven by [provision](https://github.com/alehatsman/provision).

**`provision` is also the worked example.** Its `tasks/` directory is the
reference consumer wiring; copy that shape rather than deriving a new one.

Wire it exactly as `provision` does:

```
tasks/
  tools.yml         pins the rust-quality ref, clones it via the `git` action to
                    ~/.cache/provision/tools/rust-quality, installs cargo-nextest
                    / cargo-deny / cargo-machete + the clippy & rustfmt components.
                    THE PIN LIVES HERE AND NOWHERE ELSE.
  sync-config.yml   copies clippy.toml / rustfmt.toml / deny.toml / .cargo/config.toml
                    into this repo and prints the canonical lint block
  ci.yml            use: ~/.cache/provision/tools/rust-quality/ci.yml   (full gate)
  ci-fast.yml       use: ~/.cache/provision/tools/rust-quality/fast.yml (pre-commit)
  lints-check.yml   lint-block drift — the one thing cargo cannot check itself
  findings.yml      every finding as JSONL -> .gate/findings.jsonl
```

Notes that matter:

- Every preset defaults to what a single-crate-workspace repo wants, so
  `tasks/ci.yml` is a `use:` and nothing else. Do not pass `dir` — this is one
  workspace.
- Nothing fetches at gate time. The checkout is a step in `tasks/tools.yml`,
  `creates`-gated, so the gate works offline and a bump is a one-line ref change.
- The root `Cargo.toml` carries the canonical `[workspace.lints]` block verbatim
  from `rust-quality/lints.toml`; every member declares `[lints] workspace =
  true`. `tasks/lints-check.yml` fails if either drifts.
- `.gate/findings.jsonl` is the machine-readable output. An agent working in this
  repo reads that, not scraped terminal text.

### Read before writing any Rust

- `rust-quality/docs/RUST.md` — the rules, gate markers, the 2026 trap list, and
  the review checklist.
- `rust-quality/docs/STACK.md` — the de-facto crate picks, with versions and
  documented deviation triggers.

**STACK.md outranks [research.md](research.md) §6 on crate choice.** Where this
project deviates — the zero-dependency `crates/datalog`, `memmap2` for segment
loading — record the deviation and its trigger in §6 rather than letting the two
documents disagree silently. Reconcile them during M0, before any code depends
on either.

**Done when**
- `provision apply tasks/tools.yml` converges from a clean machine.
- `provision apply tasks/sync-config.yml` lands the config, and the root
  `Cargo.toml` carries the canonical lint block with every member inheriting it.
- `provision apply tasks/ci.yml` is green: fmt, clippy, test + doctests, rustdoc,
  cargo-deny, cargo-machete, lint drift, soft caps.
- `provision apply tasks/ci-fast.yml` is green and needs no network.
- `tasks/findings.yml` produces `.gate/findings.jsonl`.
- CI (moongit or GitHub Actions) invokes **the same** `provision apply
  tasks/ci.yml`, so local and CI run one gate, not two that drift.
- `research.md` §6 is reconciled against `STACK.md`, with any deviation and its
  trigger written down.
- A test asserts `crates/datalog/Cargo.toml` depends on neither `facts` nor
  `extract`. This seam is what keeps the engine honest; guard it mechanically
  from commit one.

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
7. **Demand transformation (magic sets).** Not optional and not deferrable —
   without it `impact_of` computes all-pairs reachability and the product's most
   valuable query returns `budget-exceeded` on any real repo
   ([03-datalog.md](../specs/03-datalog.md) § Demand transformation). If this
   milestone runs long, ship the `reach/4` builtin as a stopgap **and open an
   issue**; do not ship recursion that does not scale and call it done.
8. Builtins: comparisons, arithmetic, `match`/`prefix`/`suffix`/`contains`, the
   four aggregates.
9. Limits (rows **and bytes**), truncation reporting, `Stats`.
10. **The cheap agent eval.** See below — this is the point of the milestone as
    much as the engine is.

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
- Opaque atoms: a string over `OPAQUE_THRESHOLD` gets an id in the opaque range,
  and `=`/`!=` against it is rejected with a diagnostic naming the variable.
- **Demand transformation demonstrably applies:** for each seeded traversal, a
  constant-bound goal derives strictly fewer tuples than the same goal with the
  transformation disabled, and both return identical results. Equal counts mean
  it silently did not apply — the failure mode that otherwise surfaces as an
  unexplained timeout at M6.
- **The agent eval passes at ≥ 8/10 — against hand-written facts.**

### Why the agent eval belongs here, not at M4

The load-bearing bet of this entire project is that an agent can write correct
Datalog over this schema. If that is false, the schema or the surface is wrong,
and every milestone after this one is built on top of the mistake.

Testing it needs **no extractors**: hand-write a fact file for a small,
realistic repo, write `rules/stdlib.dl` against it, generate `schema` output,
and give an agent nothing but that plus 10 English questions. A day's work that
de-risks four milestones.

Record every failure verbatim in `docs/agent-eval.md`. A failure is a `schema`
wording bug far more often than an agent bug, and the transcript is the
evidence. M4 re-runs the same eval against real extracted facts.

---

## M2 — Tier A, one language

Rust only. Walk → parse → extract → segment → load → query.

1. `ignore`-based walk, binary/size filtering.
2. Vendor `queries/rust/{tags,imports}.scm`; write the kind-mapping table.
3. Span-nesting containment sweep. **Write this once, in a shared function** —
   tier B calls the same code in M4.
4. Emit `file`, `def`, `def_span`, `def_name`, `def_sig`, `def_doc`, `parent`,
   `exported`, `import`, and `name_ref`. **Tier A resolves nothing** — see
   invariant 3b. Resolution is `stdlib.dl`'s job.
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
- **Locality test passes:** every fact for file X is reproducible by extracting
  X alone with nothing else indexed. This is the mechanical guard against
  reintroducing cross-file lookups into the extractor.
- **Incremental equivalence passes:** index, mutate one file, reindex
  incrementally → byte-identical to a cold reindex.
- `?- innermost_at("<some file>", <some line>, S).` returns the right symbol.
  The cold-start path works before anything else does.
- Auto-refresh on `query`: edit a file, query without reindexing, get the new
  answer. Delete a file, query, its facts are gone. Both with `stats.refreshed`
  naming what moved, and `status: "stale"` when `max_refresh_ms` is exceeded.

---

## M3 — Tier B, one language

`rust-analyzer scip .` → facts → the anchor join.

1. `scip` crate; read `index.scip`. Then `--run-indexers`: the language→command
   table, the `PATH` check, the subprocess, the timeout, and the
   never-fatal failure path ([02-extraction.md](../specs/02-extraction.md)
   § Acquisition).
2. Position normalization, including the UTF-16 case and its skip-and-report
   path.
3. `local N` → `local <path> N` rewrite. **Test this specifically** — the bug it
   prevents is silent and repo-wide.
4. `SymbolInformation.kind` → our 16 kinds.
5. Reference `From` attribution via the M2 containment sweep.
6. The anchor join: match on `def_name` position, rewrite the tier-A atom to the
   SCIP symbol in the interner, emit `resolved(S)`.
7. `implements`, `has_type`, `extern`, and `scip_ref`.
8. Parent precedence: descriptor prefix → `enclosing_symbol` → tier A span
   nesting, replacing tier A's row after the anchor join
   ([02-extraction.md](../specs/02-extraction.md) § Parent precedence).

**Done when**
- On `tests/fixtures/rust/` with both tiers, ≥ 95% of tier-A definitions carry
  `resolved(S)`.
- Tier-A precision test passes: every `"name"` ref either matches an `"exact"`
  ref at the same position or is listed in `known-imprecise.txt` with a reason.
- `?- calls_exact(A, B).` returns edges that `?- calls(A, B).` misses, and a
  hand-audited sample of 20 is correct in both directions.
- Deleting `index.scip` and reindexing degrades cleanly to `"name"` only, with
  `status: "no-scip"` on a query that needs precision.
- `codeintel index` **without** `--run-indexers` prints the exact indexer
  command for every detected language.
- **Go methods resolve to their type**, not to the file: `?- parent(M, T),
  def(T, _, _, "Store").` returns the methods. This is the case where lexical
  and semantic containment visibly disagree, so it is the test that proves the
  precedence rule is wired.
- `codeintel index --run-indexers` produces a usable `index.scip` on the fixture,
  and with the indexer binary removed from `PATH` it reports the failure and
  completes with tier A only.

---

## M4 — Surface and stdlib

1. `rules/stdlib.dl` — every rule in [01-facts.md](../specs/01-facts.md) §
   Derived relations, each with a doc comment and a fixture test. Includes the
   resolution rules for `ref`, `symbol_at`/`innermost_at`, `about`, `is_test`,
   and the seeded traversals.
1b. Output rendering: symbol columns as `Name` + `path:line`, `--raw` for the
   joinable form, and the byte cap wired through `truncated`/`cap`.
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
- **The agent test, full version.** The M1 eval re-run against real extracted
  facts, plus 10 *new* held-out questions in a repo none of the fixtures come
  from. ≥ 8/10 first-attempt on both sets. A regression against the M1 numbers
  means extraction quality, not schema wording, and points at M2/M3.
- A no-SCIP run of the eval, recorded separately. The gap between the two
  numbers *is* the honest measure of what tier A alone is worth, and it belongs
  in the README rather than in a footnote.
- Surface-budget test is in CI.

---

## M5 — Languages

Nine languages ship out of the box. Rust lands at M2/M3 as the bring-up
language; this milestone is the other eight.

Every one of the nine has a published, current grammar crate that **already
ships `queries/tags.scm` upstream** (verified 2026-09-12,
[research.md](research.md) §6). So each language is: one crate, two vendored
`.scm` files, one `lang.rs` row, one fixture. No per-language Rust.

### M5a — the core five

Go, Python, JavaScript, TypeScript (+TSX). With Rust from M2, these are the
languages we actually work in, and they get the full treatment.

| Language | Grammar | SCIP indexer |
|---|---|---|
| Go | `tree-sitter-go` 0.25.0 | `scip-go` |
| Python | `tree-sitter-python` 0.25.0 | `scip-python` |
| JavaScript | `tree-sitter-javascript` 0.25.0 | `scip-typescript` |
| TypeScript + TSX | `tree-sitter-typescript` 0.23.2 | `scip-typescript` |

**Done when**
- Each passes the full suite: golden facts, span exactness, anchor rate ≥ 95%,
  tier-A precision, locality, incremental equivalence.
- Each is wired into the `--run-indexers` table and produces a working
  `index.scip` on its fixture.
- A polyglot fixture (Rust + Go + TS + Python in one tree, one SCIP index per
  language) indexes and queries correctly across language boundaries.
- **Go methods resolve to their type** via parent precedence — the case where
  lexical and semantic containment disagree.

### M5b — the extended four

C, C++, Ruby, Java. Shipped and enabled, smoke-tested rather than fully
fixtured; the full suite follows as fixtures get written. The tier says how much
we have **proven**, not what is switched on.

| Language | Grammar | SCIP indexer | tier-B bootstrap |
|---|---|---|---|
| C | `tree-sitter-c` 0.24.2 | `scip-clang` | needs `compile_commands.json` |
| C++ | `tree-sitter-cpp` 0.23.4 | `scip-clang` | needs `compile_commands.json` |
| Ruby | `tree-sitter-ruby` 0.23.1 | `scip-ruby` | needs Sorbet |
| Java | `tree-sitter-java` 0.23.5 | `scip-java` | needs a working build |

**Done when**
- Each parses its smoke fixture and emits `def`/`def_span`/`import`/`name_ref`
  without panicking, with span exactness asserted.
- `codeintel status` lists them as supported, and their indexer commands appear
  in `--run-indexers` and in the no-SCIP hint.
- Grammar ABI is verified: these four are the oldest crates (0.23.x against a
  0.27 runtime). **Smoke-load all nine grammars in one test before building on
  them** — an ABI mismatch is a loud failure at load time and a confusing one
  later.

### The criterion that matters for both

**Adding a language required no changes outside `queries/`, `lang.rs`, and
`tests/fixtures/`.** If it did, the extractor is wrong — fix the extractor
rather than special-casing the language. Eight languages in one milestone is
only sane if this holds, so it is the first thing to check, not the last.

---

## M6 — Performance and incremental

Only now, with correctness fixed and something to measure.

1. Benchmark corpus: 3 pinned public repos, one per size decade (~10k, ~100k,
   ~1M LOC).
2. A **test-only** bench harness (`cargo bench`) reporting cold index, warm
   load, single-file reindex, and query p50/p95 over a fixed query set. Not a
   CLI verb — the budget is 6 and it is full
   ([00-overview.md](../specs/00-overview.md) § Surface budget).
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
| Caching the derived `ref` relation | a profile shows materializing it dominates query time |
| Watch mode | single-file reindex measures > 1 s |
| Live LSP probe (`codeintel probe file:line`) | a real query needs a type at a position that SCIP does not carry |
| Multi-repo / cross-repo indexes | someone has the problem |
| Full fixture suites for the extended four | M5b smoke tests are green and someone hits a real gap |
| More languages (C#, Kotlin, Scala, PHP) | M5's "no changes outside three places" holds |
| Query result caching | a profile shows repeated identical queries |

## Explicitly never

From [00-overview.md](../specs/00-overview.md) § Scope. Listed again because a
plan document is where scope creep enters.

Embeddings. Vector search. LLM calls. Summarization. Text search as a lane.
Ranking or centrality scores of any kind. Git-history mining. Agent memory.
Context packing. An HTTP API.
