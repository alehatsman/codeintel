# codeintel

A code graph for agents, queryable in Datalog.

`codeintel` extracts structural facts from a repository — definitions, spans,
containment, imports, references, implementations — interns them into a compact
fact store, and exposes them through a Datalog query engine. One index, one
query language, no inference, no ranking, no magic.

```sh
codeintel index .

# architecture conformance: no module under ui/ may import from db/.
# works on a fresh index, no build, no language server, every language.
codeintel query '?- import(F, M, _), prefix(F, "ui/"), contains(M, "db/").'
```

```
ui/panel.ts     ../db/pool
ui/table.ts     ../db/query
```

With a SCIP index present, the same store answers reachability:

```sh
# you have a location — from ripgrep, a stack trace, a compiler error, a diff.
codeintel query '?- innermost_at("src/store.rs", 142, S), impact_of(S, C),
                    def(C, F, _, N), at(C, F, L), !is_test(F).'
```

## Install

Built from source, native to the machine that runs it. Needs a Rust toolchain
(`rust-toolchain.toml` pins it) and nothing else — no build of the repository
you point it at, no language server.

```sh
provision apply tasks/install.yml                    # -> ~/.local/bin/codeintel
provision apply tasks/install.yml --prop dest=/usr/local/bin/codeintel
```

`tasks/build.yml` is the build alone, if you only want the binary in
`target/release/`. Both are idempotent: a second `install` run reports `ok` and
copies nothing.

Without [provision](https://github.com/alehatsman/provision), the same two
steps by hand:

```sh
cargo build --locked --release -p codeintel
mkdir -p ~/.local/bin
install -m 0755 target/release/codeintel ~/.local/bin/
```

To serve it to an agent, point the agent's MCP config at `codeintel mcp
--path /your/repo`. Registering it is the agent's configuration, not this
repository's.

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
it found, e.g. `rust-analyzer scip .` or `scip-typescript index --infer-tsconfig`.
You run it: an indexer runs your build, and `codeintel` never does
([docs/plan.md](docs/plan.md) M3). Then `codeintel index .` again picks up
`./index.scip`.

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
| [docs/cookbook.md](docs/cookbook.md) | Conformance rules, orientation, dex's 34 methods as queries |
| [docs/plan.md](docs/plan.md) | Ordered milestones with done-when criteria |
| [docs/research.md](docs/research.md) | Findings, rejected options, evidence |
| [docs/agent-eval.md](docs/agent-eval.md) | Can an agent actually write this Datalog? Measured. |
| [CLAUDE.md](CLAUDE.md) | Rules for agents implementing this |

## Can an agent actually use it?

That is the load-bearing bet, so it is measured rather than asserted. An agent
reads `codeintel schema` and nothing else, writes one query per question, and is
scored on the set of values its query returns. Three samples per question, a
fresh agent each time.

| Set | Score | Index |
|---|---:|---|
| 30 questions, real extracted facts | **89/90** | `tests/fixtures/rust/` with SCIP |
| 15 held-out questions, a repo no fixture comes from | **45/45** | this repository pinned, **no SCIP** |

The second row is also the no-SCIP number: 45/45 with tier A alone. The first
was withdrawn on 2026-09-13. Schema 2 deleted two rules that recorded answers
call, and `tests/eval_rot.rs`, which now recomputes set A on every commit,
caught it.

Every failure so far has been a *signature reading as something it is not*, and
each one is a fix to the schema rather than a complaint about the agent. The
first pass found `long_def(S, N)` reading as parameterised by `N` when `N` was
an output; `def_lines(S, N)` replaced it and every later round used it
correctly. What is left is `def(S, F, "type", ...)` for a struct, and
`local_def(F, Name)` read as `(file, symbol)`.
[docs/agent-eval.md](docs/agent-eval.md) records every failure verbatim, and why
the earlier 88/90 was withdrawn rather than kept.

## Languages

Four in v1. A language costs one crate, two **authored** query files, one table
row, and a fixture — call it ~1,000 lines and a day, not a table row.

| | Language | SCIP indexer |
|---|---|---|
| **v1** | Rust, Go, Python, TypeScript (+TSX) | `rust-analyzer scip` · `scip-go` · `scip-python` · `scip-typescript` |
| **deferred** | C, C++, Java | `scip-clang` · `scip-java` |
| **dropped** | Ruby | — |

Four, not nine. We **author** the queries rather than vendoring them, because
upstream `tags.scm` files are not consistent enough to build on: TypeScript's
covers only ambient `.d.ts` forms and has no call references at all, Python has
no method capture, and Rust maps structs, enums, unions and type aliases onto a
single capture. Ruby is dropped outright — it has no import node, since `require`
is an ordinary method call. C and C++ are strong candidates next, because they
are import-complete, which is all conformance needs.

## Status

M0–M5 shipped: the Datalog engine, the fact store, the tree-sitter tier, the
SCIP tier and the anchor join, all five verbs and the MCP tool. **Rust, Go,
Python and TypeScript** (with TSX) index today, each with a committed fixture
index. Anchor rates on those fixtures are 80% for Rust, where every miss is one
of its 9 modules and modules are exempt
([02-extraction.md](specs/02-extraction.md) § Validation), 100% for Go, 96.7%
for Python and 98.4% for TypeScript. The one miss in each of the last two is an
occurrence whose column #21 refuses to guess.

Not built yet: M6's performance work, beyond the bench harness and counter
goldens that were pulled forward.
See [docs/plan.md](docs/plan.md) for where the line is.

Both examples above run today. The second one needs a SCIP index — without one
it answers `status: "no-scip"` and prints the command that would build it,
rather than `ok` with zero rows.
