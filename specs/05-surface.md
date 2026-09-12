---
id: surface
status: proposed
binding: yes
---
# 05 — Surface

The surface budget from [00-overview.md](00-overview.md) is binding: **5 CLI
verbs, 1 MCP tool, <= 24 named predicates in `stdlib.dl`.** Growth requires
deletion. The rule count is in the budget because it is the dimension that
actually grows; asserting only the ones that do not makes the thesis
unfalsifiable.

## Status taxonomy

Every response on every surface carries a `status`. Adopted from dex, which got
this right: a consumer branches on status instead of catching failures or
guessing what an empty result means.

| status | Meaning | Hint returned |
|---|---|---|
| `ok` | query ran, results are complete | — |
| `truncated` | query ran, a cap fired | which cap (`max_result_rows` / `max_result_bytes`), and its value |
| `no-index` | no `.codeintel/` here | `run: codeintel index .` |
| `stale` | sources changed and auto-refresh was skipped, exceeded `max_refresh_ms`, or `schema_version` mismatched | `run: codeintel index .` — names the N files |
| `no-scip` | the goal's **relation-dependency closure** reaches a relation only SCIP populates, and none was ingested | which relations, and the indexer command for the languages present |
| `scip-stale` | an indexed file is newer than `index.scip` | which files, and the indexer command |
| `unsupported-language` | files present in a language with no grammar | which languages, how many files |
| `invalid-query` | parse or safety failure | the rule, the variable, the violated rule |
| `unstratified` | negation cycle | the cycle |
| `timeout` / `budget-exceeded` | a limit aborted evaluation | the limit and its value |
| `locked` | another index run holds the lock | its pid |

**`ok` with zero rows means "this is not true of your code."** That must be
distinguishable from every failure above without reading prose. This is
invariant 6 and the most common way a tool like this lies to an agent.

`no-scip` is that invariant at its sharpest, and it fires on the **closure, not
the syntax**. `?- impact_of(S, C).` mentions no base relation at all; its
closure reaches `scip_ref`. Without the closure test the README's own headline
query returns `ok` with zero rows on a fresh install.

**`stale`, `truncated`, `no-scip` and `scip-stale` all carry rows and all exit
0.** They are answers about a known-imperfect index, not failures: the caller
gets what there is and is told exactly what is wrong with it. Only `no-index`,
`invalid-query`, `unstratified`, `timeout`, `budget-exceeded`, `locked` and
`corrupt` exit 2.

---

## CLI

```
codeintel index  [PATH] [--scip FILE]... [--rebuild] [--lang L]...
codeintel query  <PROGRAM|-> [--format text|json] [--limit N] [--rules FILE]...
                 [--raw] [--no-refresh] [--expect-empty]
codeintel schema [--format text|json]
codeintel status [PATH] [--format text|json]
codeintel mcp
```

**Five verbs.** `PATH` defaults to `.`.

`rules` is gone. It translated `rules NAME A B` into `?- NAME(A, B, Vars...)`
with args "bound positionally as atoms, no type coercion" — and integers and
their string forms are different atoms ([01-facts.md](01-facts.md) § Integers),
so `codeintel rules innermost_at src/store.rs 142` bound the *string* `"142"`,
matched nothing, and returned `status: ok` with zero rows. Broken for every rule
taking a line number, including the headline one, and lying while doing it. The
`*_by_name` rules it needed stay in `stdlib.dl`; they are useful on their own.

`--format tsv` is gone: text is already TSV-ish and there were three formats for
two consumers.

`--run-indexers` is not built — see [02-extraction.md](02-extraction.md)
§ Acquisition. `index` prints the exact command.

### `index`

Builds or updates the store. `--scip` may repeat; defaults to `./index.scip` if
present. `--rebuild` discards and rewrites, including the dictionary. `--lang`
restricts grammars.

`--run-indexers` shells out to the canonical SCIP indexer for each detected
language before indexing ([02-extraction.md](02-extraction.md) § Acquisition).
Never implicit — an indexer runs arbitrary build code, so it takes an explicit
flag every time. A missing binary or a failed indexer prints the command and its
stderr and **continues with tier A only**; it is never fatal.

Without the flag, `index` prints the exact indexer command for every language it
found. Copy-pasteable, not a description of one.

Prints a summary to stderr: files indexed/skipped/unchanged, facts per relation,
SCIP coverage, anchor rate, elapsed. Skipped files are summarized **by reason**
— "3 too large, 88 unsupported (`.scala`)" — because a silently unindexed
subtree is the single most confusing failure this tool can have.

**There is no `ignored` count.** The `ignore` crate does not report the entries
its matchers drop, so the number costs a second full traversal with the matchers
off — which on a repo with a populated `target/` is the most expensive thing in
the walk, spent on a figure that changes no decision. An earlier draft of this
line read "412 ignored, 3 too large, ...".

### `query`

```sh
codeintel query '?- callers(C, S), def(S, _, _, "get").'
codeintel query - < investigation.dl
```

**Auto-refresh, on by default.** Before evaluating, `query` stats the files in
the manifest and re-extracts any that changed. An agent's normal loop is edit →
ask, and answering that from a stale index produces a confidently wrong answer —
the worst failure this tool has, because it looks like a right one.

- Tier A only. SCIP cannot be refreshed incrementally
  ([04-storage.md](04-storage.md) § Incremental reindex), so a file edited since
  the SCIP index was built has fresh `name_ref` and stale `scip_ref`. That
  mixture is reported as `scip-stale`, which auto-refresh makes **more** likely,
  not less — so the status matters more now, not less.
- **Auto-refresh carries the manifest's SCIP inputs forward.** It re-extracts a
  changed file against the *existing* ingest, so the file keeps whatever
  resolved identity still matches at the old positions and loses the rest. What
  it must never do is refresh with no SCIP at all: that looks like "the index
  disappeared", re-extracts the tree tier-A-only, and deletes every tier-B fact
  — the store would degrade a little more with each query, silently. A full
  `codeintel index` is what reconciles a `scip-stale` index.
- Bounded by `max_refresh_ms` (default 2,000). If the refresh would exceed it,
  it is abandoned, the query runs against the index as it stands, and the
  response carries `status: "stale"` naming the files it could not refresh.
  Never a silent slow path and never a silent stale answer.
- `stats.refreshed` lists what was re-extracted. Zero is the common case and
  costs one `stat` per indexed file.
- `--no-refresh` skips it, for benchmarking or for querying a deliberately
  pinned index.
- New files are picked up; **deleted files are pruned.** A refresh that only
  added would leave a deleted file's facts answering queries.

MCP always refreshes; the flag is not exposed there. An agent editing files is
the assumed case, not the exception.

Text format is TSV-ish, one row per line, aligned, designed to be read by a
human and grepped by an agent. JSON is
`{ status, columns, rows, truncated, cap, stats }`.

**Row order is lexicographic over the rendered row**, applied here rather than
in the engine. The engine's own order is by atom, which is dictionary order —
insertion order ([03-datalog.md](03-datalog.md) § Determinism). That is total
and deterministic for one index, but a cold index and an incrementally-updated
one assign different atom ids to the same string, so the same repo state would
print the same rows in a different order depending on how the index was built.
Sorting the rendered text costs one sort per query and makes invariant 8 hold
across index paths, which is the property a consumer actually relies on. The
sort runs **after** the row cap, so a truncated result is still the engine's
stable prefix rather than a re-sorted sample of it.

**Symbol rendering.** A raw `SymId` is a ~68-character SCIP string — 17 tokens,
unreadable, and impossible for an agent to retype correctly. In text format a
column holding a symbol atom renders as `Name path:line`, with the name and the
location joined by a **space**, so the column stays one printed field:

```
?- calls(C, S).
start src/app.rs:7      open src/db/conn.rs:3
warm src/store.rs:38    get src/store.rs:19
```

**One printed field per column, always.** An earlier draft of this line read
"where the row also carries a file and a line they render adjacent" — which, on
the reading where a symbol expands into two tab-separated fields, makes the text
form wider than `columns` says it is and hands a consumer that splits on tabs a
ragged table. The space join costs nothing and keeps the TSV honest.

Symbol-ness is **extracted, not guessed**: an atom is a symbol exactly when
`def` has a row for it, and the location comes from `def_span`. A symbol with no
`def_span` prints `Name path`; an atom with no `def` row prints as itself.
Nothing here infers from the shape of a string.

Because the renderer resolves the location itself, `?- calls(C, S).` is already
a useful answer — a `def(S, F, _, N), at(S, F, L)` tail is for *filtering* on
those values or binding them, not for seeing where something is. `schema` says
so, because every join an agent does not have to write is a join it cannot get
wrong.

`--raw` prints the underlying `SymId` instead. Use it when piping one query's
output into another query's literal. JSON carries both: `rows` is always the raw
atoms, one value per column, and `display` is the same rows expanded — so a
programmatic consumer never parses the pretty form. This is presentation only —
the tuple is unchanged, and `--raw` is what round-trips.

**The byte cap is measured on the printed text**, not on the engine's estimate
over raw atoms. Symbol expansion is what the consumer's context window actually
pays for, and `crates/datalog` cannot account for it without learning what a
symbol is (invariant 6). It is applied before the sort, so a truncated answer is
still the engine's stable prefix.

### `schema`

Prints the relation catalog, atom vocabularies, and stdlib rule signatures.
**Must fit in ~1500 tokens** — this is the text an agent reads to learn the
system, and it is the highest-leverage output in the project.

```
START HERE — you have a location, you need a symbol
  ?- innermost_at("src/store.rs", 142, S).     from ripgrep / git diff /
                                                a stack trace / a compiler error
  ?- def(S, F, _, N), contains(N, "auth").      from a word
  ?- about(S, Rel, A, B, L).                    everything about S, one round trip

RELATIONS
  file(F, Lang)
  def(S, F, Kind, Name)
  def_span(S, StartLine, EndLine, StartByte, EndByte)
  scip_ref(S, F, Line, Col, From, Role)     compiler-resolved occurrence
  name_ref(Name, F, Line, Col, From)        unresolved identifier (tier A)
  ...
VALUES   closed sets, with counts from THIS index. A kind at 0 is a kind you
         will get no rows for -- do not query it.                        <gen>
  Kind   function 4,201  class 812  module 96  interface 41
         method 0  struct 0  enum 0  trait 0  constant 0  field 0
         constructor 0  variable 0  macro 0  typealias 0  type 0  unknown 1,340
  Role   def 5,150  read 0  write 0  import 402  test 0
  Prov   name 18,904  exact 0
Generated from the manifest, not printed as a constant. A static list
advertising sixteen kinds when the index holds four is this document's own
invariant 5 broken by its own onboarding text.
RULES
  innermost_at(F, Line, S)       location -> tightest enclosing symbol
  about(S, Rel, A, B, L)         sig|doc|defined|caller|callee|implements|test
  ref(S,F,L,C,From,Role,Prov)    resolved occurrence, either tier
  calls(Caller, Callee)          callable reference, either provenance
  calls_exact(Caller, Callee)    SCIP-resolved only
  impact_of(S, Caller)           transitive callers of S -- SEED THE 1st ARG
  reach_of(S, Callee)            transitive callees of S -- SEED THE 1st ARG
  is_test(F)                     test file, by path or SCIP role
  ...
BUILTINS = != < <= > >= + - * / between/3 match/2 prefix/2 suffix/2 contains/2
           count{X:g}
NOTES
  lines are 1-based; columns are 0-based UTF-8 bytes
  integers and their string forms are different atoms: Line = "42" never matches
  < and > are integers only
  seed recursive rules (impact_of/reach_of) with a constant, or they compute
    all-pairs reachability and hit the budget
  Prov "name" = tree-sitter text matching: method calls x.f() are mostly
    MISSING. For precision use calls_exact / impact_of_exact, and check
    `codeintel status` for whether a SCIP index is present and fresh.
EXAMPLES
  what does this diff hunk affect?
  ?- innermost_at("src/store.rs", 142, S), impact_of(S, C),
     def(C, F, _, N), at(C, F, L), !is_test(F).

  blast radius of a rename, precise only
  ?- def(S, _, _, "get"), impact_of_exact(S, C), def(C, F, _, N), at(C, F, L).

  exported and unreferenced outside its own file
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
  "stats": { "derived": 18422, "elapsed_ms": 7, "refreshed": [], "transformed": ["impact_of"],
             "depends": ["def", "name_ref", "scip_ref"] }
}
```

- **`hint` is non-null whenever `status != "ok"` OR the result is empty.** On an
  empty result it names the first body literal that matched nothing:
  `"def/4 with Kind=\"method\" matched 0 rows; this index contains 0 method
  definitions"`. As originally specified, `hint` was guaranteed null in exactly
  the case the taxonomy exists to disambiguate — a valid query returning zero
  rows, which is indistinguishable from a typo, a wrong `Kind`, a path that is
  not indexed, and a missing SCIP index. The engine already evaluates
  literal-by-literal, so the first-zero literal is free.
- Everything else carries a command the agent can run. `no-index` returns `run: codeintel index .`, not "index not
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
