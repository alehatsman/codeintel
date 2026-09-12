# codeintel

A code graph for agents, queryable in Datalog.

`codeintel` extracts structural facts from a repository — definitions, spans,
containment, imports, references, implementations — interns them into a compact
fact store, and exposes them through a Datalog query engine. One index, one
query language, no inference, no ranking, no magic.

```
codeintel index .
codeintel query '?- callers(C, "Store::get").'
```

```
src/api/handler.rs:42   handle_read
src/cache/warm.rs:118   warm_entry
```

## Why

An agent arriving at an unfamiliar repo has ripgrep (lexical) and its own model
(semantic). What it lacks is **structural**: who calls this, what breaks if I
change it, what does this file actually depend on, which of these 200 exports is
dead. Those are relational queries over a graph, and the honest interface to a
graph is a relational query language.

Every hardcoded graph endpoint — `callers`, `impact`, `dead_exports`,
`entrypoints` — is a two-line Datalog rule. Shipping the engine instead of the
endpoints means the surface stops growing when the questions do.

## What it is not

No embeddings. No vector search. No LLM calls. No chunking, summarizing, or
reranking. No quality scores or smell heuristics. No agent memory. No text
search — you have ripgrep.

`codeintel` answers structural questions about code. That is the whole product.
See [specs/00-overview.md](specs/00-overview.md) § Non-goals, which is binding.

## Architecture

Two extraction tiers feed one fact store:

```
                  ┌──────────────────────────────────┐
  source files ──►│ tree-sitter tier   (always)      │ defs, spans, signatures,
                  │ queries/*.scm, no build needed   │ containment, imports
                  └──────────────┬───────────────────┘
                                 │  anchored by span containment
                  ┌──────────────▼───────────────────┐
  index.scip   ──►│ SCIP tier          (when present)│ global symbol identity,
                  │ scip-* / rust-analyzer output    │ references, implements
                  └──────────────┬───────────────────┘
                                 ▼
                     .codeintel/  interned u32 facts, per-file segments
                                 ▼
                     Datalog engine — semi-naive, stratified negation
                                 ▼
                            CLI  ·  MCP (one tool)
```

The tree-sitter tier is fast, universal, and imprecise. The SCIP tier is
precise and requires a working build. They are joined, not merged: facts carry
an explicit `exact` / `name` provenance, so a query can demand precision or
accept reach, and degradation is visible rather than silent.

## Docs

Read in this order.

| Doc | Contents |
|---|---|
| [specs/00-overview.md](specs/00-overview.md) | Goal, scope, non-goals, invariants. **Binding.** |
| [specs/01-facts.md](specs/01-facts.md) | The fact schema. The project's ABI. |
| [specs/02-extraction.md](specs/02-extraction.md) | tree-sitter tier, SCIP tier, the anchor join |
| [specs/03-datalog.md](specs/03-datalog.md) | Query language, engine, evaluation, limits |
| [specs/04-storage.md](specs/04-storage.md) | Interner, segments, incremental reindex |
| [specs/05-surface.md](specs/05-surface.md) | CLI and MCP contracts |
| [docs/plan.md](docs/plan.md) | Ordered milestones with done-when criteria |
| [docs/research.md](docs/research.md) | Findings, rejected options, evidence |
| [CLAUDE.md](CLAUDE.md) | Rules for agents implementing this |

## Status

Specification. No code yet. Start at [docs/plan.md](docs/plan.md) § M0.
