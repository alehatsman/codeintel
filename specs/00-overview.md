---
id: overview
status: proposed
supersedes: none
binding: yes
---
# 00 — Overview

This spec is **binding**. Where another spec or a code change conflicts with it,
this one wins or the conflict gets escalated. Its job is to be the thing a
future agent reads when it is about to add a feature, so that it doesn't.

## Goal

Give an agent working in an unfamiliar repository a **structural** map it can
query relationally, with honest provenance and no inference.

One sentence, and every word is load-bearing:

> `codeintel` extracts structural facts from source code and answers Datalog
> queries over them.

## The three-lane premise

An agent has three ways to find code. They are not competitors and we only own
one of them.

| Question shape | Right tool | Ours? |
|---|---|---|
| "where does the string `retry_backoff` appear" | ripgrep | no |
| "what code is *about* rate limiting" | the agent's own model, reading | no |
| "who calls `Store::get`, transitively, and which of them are tests" | **codeintel** | **yes** |

We lose the first two lanes on purpose. Every code-intel tool that tried to own
all three became a search engine with a graph bolted on, and the graph is the
part nobody else can give you.

## Scope

In:

- Extracting definitions, spans, signatures, containment, imports, references,
  and implementations from a source tree.
- Two extraction tiers with explicit provenance: tree-sitter (universal,
  name-grade) and SCIP (precise, requires a build).
- A persistent, incrementally-updatable fact store.
- A Datalog engine that evaluates queries supplied at runtime.
- A CLI and a single-tool MCP server over that engine.
- A small standard library of rules (`callers`, `impact`, `reaches`, ...)
  written in Datalog, not in Rust.

Out — and these are **non-goals, not backlog items**:

- **Embeddings, vector search, semantic ranking, reranking.** Different product.
- **LLM calls of any kind.** No summarization, no synthesis, no natural-language
  answers. `codeintel` returns tuples. The agent is the language model.
- **Text/regex search as a lane.** `match/2` exists as a *filter inside* a
  query; there is no `codeintel grep`.
- **Scores.** No PageRank, betweenness, community detection, centrality,
  "importance", or smell rankings. See [research.md](../docs/research.md) §1c:
  any score over a mixed-provenance graph launders resolution quality into a
  number that looks objective. If a user wants a ranking, they write a Datalog
  aggregate and own the definition.
- **Git history mining.** Co-change coupling is measurably non-structural
  (research.md §1c). Out of scope permanently.
- **Agent memory, notes, sessions, or any write verb over user content.** The
  only writer is the indexer.
- **Chunking, compression, token budgeting, context packing.** The harness owns
  context.
- **A server, a daemon beyond `codeintel mcp`, a database, or a network
  protocol of our own.**

## Invariants

These hold at every commit. A change that breaks one is a spec change first.

1. **Facts are extracted, never inferred.** Every tuple in the store traces to a
   byte range in a source file or to a field in a SCIP index. Nothing is
   guessed, smoothed, or scored.
2. **Provenance is explicit and never averaged.** Relations whose accuracy
   depends on resolution quality carry a `Prov` column with exactly two values:
   `exact` (SCIP, type-resolved) and `name` (tree-sitter, name-matched). No
   third value, no confidence float, no blending.
3. **Derivation lives in Datalog, not Rust.** If a fact can be derived from
   other facts, it is a rule in `stdlib.dl`, visible and overridable. Rust
   extracts; Datalog derives. `calls` is a rule; so is `ref`, because resolving
   a name to a symbol needs whole-repo knowledge.
3b. **Every extracted fact is a function of its own file.** An extractor may not
   consult another file. Cross-file knowledge enters only through rules, which
   are re-evaluated per query. Violating this makes incremental indexing
   unsound — a fact derived from file B cannot be invalidated by a change to
   file B if it is stored against file A.
4. **The fact schema is the ABI.** Adding a relation is cheap. Changing or
   removing one is a breaking change with a version bump and a migration note in
   [01-facts.md](01-facts.md).
5. **The index is 100% derived.** `rm -rf .codeintel` is always a valid repair.
   Nothing user-authored is ever stored there.
6. **Degradation is a status, never an error and never a silent empty.** The
   taxonomy in [05-surface.md](05-surface.md) (`ok` / `no-index` / `stale` /
   `no-scip` / `unsupported-language` / `truncated`) is returned on every
   response, each with an actionable hint. An empty result and an unbuilt index
   must be distinguishable by a consumer without reading prose.
7. **Truncation is reported.** Any cap that drops rows sets `truncated: true`
   and reports the cap that fired. A silently-capped result reads as a complete
   answer and is worse than an error.
8. **Determinism.** The same repo state and the same query produce
   byte-identical output. Result ordering is total and explicit.
9. **Output is bounded in bytes, not just rows.** The consumer is a context
   window and rows are not uniformly sized.

## Surface budget

A hard ceiling, checked in CI (see [docs/plan.md](../docs/plan.md) M4):

- **MCP tools: 1.** Named `code_query`. Growing to 2 requires deleting one.
- **CLI verbs: 5.** `index`, `query`, `schema`, `status`, `mcp`.
  Benchmarking is a test-only harness (`cargo bench`), **not** a sixth verb.
- **Base relations: ≤ 16.** Currently 14 ([01-facts.md](01-facts.md)).
- **Named predicates in `rules/stdlib.dl`: ≤ 24.** This is the one that grows.
  The other three cannot, which is why asserting only those three would make the
  founding thesis unfalsifiable. Flags do not count — `--rules`, `--raw`,
  `--expect-empty` cost no verb — but a flag that needs a paragraph in `schema`
  output is spending the budget that actually binds: **`codeintel schema` must
  fit in ~1500 tokens with the rule list complete.**

If a new capability cannot be expressed as a Datalog rule over the existing
relations, that is the signal to think hard — not the signal to add a verb.

## Layout

```
crates/
  datalog/     the engine. no knowledge of code. ~3000 LOC, zero deps.
  facts/       interner, relations, segments, manifest. ~700 LOC.
  extract/     tree-sitter tier + SCIP tier + the anchor join. ~1000 LOC per
               language, plus ~600 shared. Four languages -> ~4600.
  codeintel/   CLI + MCP. thin. ~500 LOC.
queries/       *.scm per language (tags, imports) -- AUTHORED, not vendored
rules/         stdlib.dl — the shipped rule library
specs/         these documents
```

These numbers were revised after the 2026-09-12 review. The engine was budgeted
at ~1200 LOC, which describes `datafrog` plus a parser and omits eight safety
checks with named diagnostics, stratification, demand transformation, limits and
truncation. The extractor was budgeted at ~900 LOC for nine languages, against a
same-author precedent — `~/projects/dex/internal/graph/` — spending ~6,177 LOC
on five. **The extractor, not the engine, was the under-estimate**, and it is
why the language set is four rather than nine ([docs/plan.md](../docs/plan.md)
M5).

`datalog/` must not depend on `facts/` or know what a symbol is. It is a generic
engine over interned tuples. This is the seam that keeps the engine testable and
keeps code-intel concepts from leaking into evaluation.

## Success criteria

v1 is done when, on a 100k-LOC polyglot repository:

- `codeintel index` completes the tree-sitter tier in **< 30 s** cold, **< 1 s**
  for a single-file change.
- Median query latency is **< 20 ms**, p95 **< 100 ms**, over a warm store **in
  the MCP process**. The CLI pays process start plus a warm load per invocation
  and is budgeted separately at **< 250 ms**.
- **These are 100k-LOC numbers.** The 1M-symbol table in
  [01-facts.md](01-facts.md) § Scale is a sizing note for the storage layout,
  **not a latency promise**, and the two are ~200x apart. Four independent
  reviewers built performance objections by pairing them; say it once, here,
  rather than letting the next reader do it again.
- An agent given only the output of `codeintel schema` writes a correct,
  non-trivial query on the first attempt in **≥ 24 of 30** held-out tasks —
  n=10 cannot separate "works" from "coin flip" (the 95% interval on 8/10 spans
  roughly 0.49–0.94), and five milestones were gated on it
  ([docs/plan.md](../docs/plan.md) M4 validation).
- Every question in the "34 methods" list from [research.md](../docs/research.md)
  §1a is answerable, and each answer is **≤ 5 lines of Datalog**.
