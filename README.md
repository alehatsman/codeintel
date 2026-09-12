# codeintel

A code graph for agents, queryable in Datalog.

`codeintel` extracts structural facts from a repository — definitions, spans,
containment, imports, references, implementations — interns them into a compact
fact store, and exposes them through a Datalog query engine. One index, one
query language, no inference, no ranking, no magic.

```sh
codeintel index .

# you have a location — from ripgrep, a stack trace, a compiler error, a diff.
# turn it into a symbol, then ask what it touches.
codeintel query '?- innermost_at("src/store.rs", 142, S), impact_of(S, C),
                    def(C, F, _, N), at(C, F, L), !is_test(F).'
```

```
handle_read     src/api/handler.rs:42
warm_entry      src/cache/warm.rs:118
flush_pending   src/cache/warm.rs:203
```

## Why

An agent arriving at an unfamiliar repo has ripgrep (lexical) and its own model
(semantic). What it lacks is **structural**: who calls this, what breaks if I
change it, what does this file actually depend on, which of these 200 exports is
dead. Those are relational queries over a graph, and the honest interface to a
graph is a relational query language.

The bridge matters as much as the graph. Every other tool an agent uses speaks
`path:line` — ripgrep, `git diff`, stack traces, compiler errors. `innermost_at`
lifts any of them into the graph, so `codeintel` composes with the agent's
existing habits instead of asking it to start over.

Every hardcoded graph endpoint — `callers`, `impact`, `dead_exports`,
`entrypoints` — is a two-line Datalog rule. Shipping the engine instead of the
endpoints means the surface stops growing when the questions do.

## What it is not

No embeddings. No vector search. No LLM calls. No chunking, summarizing, or
reranking. No quality scores or smell heuristics. No agent memory. No text
search — you have ripgrep, and `innermost_at` is the bridge back.

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

### You want the SCIP tier

Be clear about what each tier buys, because the difference is large:

| | tree-sitter alone | + SCIP |
|---|---|---|
| where symbols are, spans, signatures, docs | ✅ | ✅ |
| containment, imports, `innermost_at` | ✅ | ✅ |
| **call graph, `impact_of`, find-references** | ⚠️ sparse | ✅ |

Tier A matches identifiers as text. It does not resolve `x.method()` — and in
Rust, Python, TypeScript, and Java that is the *dominant* call form. Tier A
alone gives you a good symbol map and a thin, unrepresentative call graph.

So `codeintel index` always prints the exact indexer command for the languages
it found, and `--run-indexers` runs them for you:

```sh
codeintel index . --run-indexers     # rust-analyzer scip . | scip-typescript index | ...
```

Never implicit — an indexer runs your build. If it is missing or fails, you get
the command, its stderr, and a tier-A index; it is never fatal.

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
