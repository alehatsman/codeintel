---
id: surface
status: proposed
binding: yes
---
# 05 — Surface

The surface budget from [00-overview.md](00-overview.md) is binding: **6 CLI
verbs, 1 MCP tool.** Growth requires deletion.

## Status taxonomy

Every response on every surface carries a `status`. Adopted from dex, which got
this right: a consumer branches on status instead of catching failures or
guessing what an empty result means.

| status | Meaning | Hint returned |
|---|---|---|
| `ok` | query ran, results are complete | — |
| `truncated` | query ran, a cap fired | which cap, and its current value |
| `no-index` | no `.codeintel/` here | `run: codeintel index .` |
| `stale` | index older than sources, or `schema_version` mismatch | `run: codeintel index .` — names the N changed files |
| `no-scip` | query needs `"exact"` provenance, none available | the indexer command for the languages present |
| `scip-stale` | `index.scip` older than sources | the indexer command |
| `unsupported-language` | files present in a language with no grammar | which languages, how many files |
| `invalid-query` | parse or safety failure | the rule, the variable, the violated rule |
| `unstratified` | negation cycle | the cycle |
| `timeout` / `budget-exceeded` | a limit aborted evaluation | the limit and its value |
| `locked` | another index run holds the lock | its pid |

**`ok` with zero rows means "this is not true of your code."** That must be
distinguishable from every failure above without reading prose. This is
invariant 6 and the most common way a tool like this lies to an agent.

---

## CLI

```
codeintel index  [PATH] [--scip FILE]... [--rebuild] [--lang L]...
codeintel query  <PROGRAM|-> [--format text|json|tsv] [--limit N] [--rules FILE]
codeintel rules  [NAME] [ARG]...
codeintel schema [--format text|json]
codeintel status [PATH]
codeintel mcp
```

Six verbs. `PATH` defaults to `.`.

### `index`

Builds or updates the store. `--scip` may repeat; defaults to `./index.scip` if
present. `--rebuild` discards and rewrites, including the dictionary. `--lang`
restricts grammars.

Prints a summary to stderr: files indexed/skipped/unchanged, facts per relation,
SCIP coverage, anchor rate, elapsed. Skipped files are summarized **by reason**
— "412 ignored, 3 too large, 88 unsupported (`.scala`)" — because a silently
unindexed subtree is the single most confusing failure this tool can have.

### `query`

```sh
codeintel query '?- callers(C, S), def(S, _, _, "get").'
codeintel query - < investigation.dl
```

Text format is TSV-ish, one row per line, aligned, atoms rendered as their
strings, designed to be read by a human and grepped by an agent. JSON is
`{ status, columns, rows, truncated, cap, stats }`. Row-order is the engine's
deterministic order ([03-datalog.md](03-datalog.md) § Determinism).

Where a result row contains a file atom and a line integer, text format renders
them adjacent as `path:line` so the output is clickable and pasteable. This is a
formatting affordance only — it does not change the tuple.

### `rules`

Runs a named rule from `rules/stdlib.dl` with positional arguments. The
convenience layer, so common questions do not require writing Datalog.

```sh
codeintel rules                      # list rules with arities and doc comments
codeintel rules callers_by_name get  # -> ?- callers_by_name(A, "get").
codeintel rules impact_by_name get
codeintel rules dead_export
```

`rules` **must not** accumulate special cases. It is one fixed translation:
`codeintel rules NAME A B` → `?- NAME(A, B, Vars...)`, args bound positionally
as atoms, trailing variables left unbound. No name lookup, no type coercion, no
argument inspection.

Ergonomics belong in Datalog, not here. Because a `SymId` is an unwieldy thing
to type, `stdlib.dl` ships `*_by_name` variants that take the display name:

```prolog
callers_by_name(C, N) :- calls(C, S), def(S, _, _, N).
impact_by_name(C, N)  :- impact(S, C), def(S, _, _, N).
```

That is the pattern for every ergonomic affordance: a rule, in the file users
can read and extend. Every special case added to `rules` instead is the first
step back toward dex's 34 store methods.

### `schema`

Prints the relation catalog, atom vocabularies, and stdlib rule signatures.
**Must fit in ~1500 tokens** — this is the text an agent reads to learn the
system, and it is the highest-leverage output in the project.

```
RELATIONS
  file(F, Lang)
  def(S, F, Kind, Name)
  def_span(S, StartLine, EndLine, StartByte, EndByte)
  ...
KINDS    module type interface struct enum trait class function method
         constructor field constant variable macro typealias unknown
ROLES    def read write import test generated forward
PROV     exact (SCIP, compiler-resolved) | name (tree-sitter, text-matched)
RULES
  calls(Caller, Callee)          callable reference, either provenance
  calls_exact(Caller, Callee)    SCIP-resolved only
  impact(S, Caller)              transitive callers of S
  ...
BUILTINS = != < <= > >= + - * / match/2 prefix/2 suffix/2 contains/2
           count{X:g} sum{X:g} min{X:g} max{X:g}
NOTES
  lines are 1-based; columns are 0-based UTF-8 bytes
  integers and their string forms are different atoms: Line = "42" never matches
  < and > are integers only
EXAMPLES
  ?- impact(S, C), def(S, _, _, "get"), def(C, F, _, N), at(C, F, L).
  ?- dead_export(S), def(S, F, _, _), match(F, "^src/").
```

### `status`

Index freshness, per-tier. Reports the counts a user needs to trust or distrust
an answer: files indexed, files changed since index, SCIP tool/coverage/
staleness, anchor rate, unsupported languages, fact counts per relation.

---

## MCP

**One tool.** Named `code_query`.

```json
{
  "name": "code_query",
  "description": "Query the structural code graph in Datalog. Facts: definitions, spans, containment, imports, references, implements. Call with {\"schema\":true} first to get the relation catalog and examples.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query":  { "type": "string", "description": "Datalog program ending in a ?- goal." },
      "rule":   { "type": "string", "description": "Named stdlib rule instead of a program." },
      "args":   { "type": "array", "items": { "type": "string" } },
      "schema": { "type": "boolean", "description": "Return the relation catalog and exit." },
      "limit":  { "type": "integer", "default": 200 },
      "format": { "type": "string", "enum": ["text", "json"], "default": "text" }
    }
  }
}
```

Exactly one of `query`, `rule`, `schema` per call; two is `invalid-query`.

### Response contract

```json
{
  "status": "ok",
  "columns": ["C", "F", "L"],
  "rows": [["handle_read", "src/api/handler.rs", 42]],
  "truncated": false,
  "hint": null,
  "stats": { "derived": 18422, "elapsed_ms": 7 }
}
```

- **`hint` is non-null whenever `status != "ok"`** and contains a command the
  agent can run. `no-index` returns `run: codeintel index .`, not "index not
  found".
- On `invalid-query`, `hint` contains the corrected shape where it can be
  inferred — an arity mismatch reports the expected arity. An agent should be
  able to self-correct in one retry.
- Text format wins by default. It is ~4× denser than JSON for tabular results
  and every consumer is a language model.

### Why one tool

dex's `internal/mcp` is 17,763 lines and it needed a lint to stop its tool
descriptions from arguing with each other ([research.md](../docs/research.md)
§1a). The failure mode is structural: N tools means N descriptions competing for
the router's attention, and each new question is tempted to become tool N+1.
One tool plus a query language means new questions cost zero surface.

The cost is real and should be stated: the agent must learn Datalog. That is
what `schema: true` is for, and the ≥ 8/10 first-attempt success criterion in
[00-overview.md](00-overview.md) is the test of whether that cost was paid down.

---

## Non-surface

No HTTP server. No daemon. No watch mode in v1 — `index` on a single changed
file is under a second, so the harness can call it from a hook and a file
watcher is a process-lifecycle problem we do not need to own.

No `--explain`, no query plan output beyond `stats`. No interactive REPL. No
output templating. Each of these is a plausible half-day of work that permanently
widens the surface.
