---
id: surface
status: proposed
binding: yes
---
# 05 — Surface

The surface budget from [00-overview.md](00-overview.md) is binding: **5 CLI
verbs, 1 MCP tool, <= 16 base relations, and `schema` output <= 5,700
characters with the rule list complete.** Growth requires deletion.

There is **no cap on the rule count**. There was, twice, and both numbers were
set rather than derived; 00-overview.md § Surface budget records why they were
dropped. Rules are the growth path the design wants — a new structural question
is meant to become a rule, not a verb or a Rust helper — so the thing to bound
is what they cost an agent, which is `schema` bytes. `tests/surface.rs` asserts
all four.

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
| `scip-stale` | an indexed file holds bytes `index.scip` did not see ([04-storage.md](04-storage.md) § Manifest, `scip_hash`) | which files, and the indexer command |
| `invalid-query` | parse or safety failure | the rule, the variable, the violated rule |
| `unstratified` | negation cycle | the cycle |
| `timeout` / `budget-exceeded` | a limit aborted evaluation | the limit and its value |
| `locked` | another index run holds the lock | its pid |
| `corrupt` | a segment or the dictionary would not load | `run: codeintel index . --rebuild` |

**There was an `unsupported-language` row here, and nothing ever emitted it.**
Deciding it per query costs a full tree walk to find files no grammar covers,
which is `codeintel status`'s job and is why the walk lives there. A status the
taxonomy advertises and the code cannot produce is the same defect as a status
the code produces and the taxonomy omits — `the_status_taxonomy_matches_the_spec`
now fails on either. `status` reports unsupported extensions, biggest first.

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
codeintel query  <PROGRAM|-> [--path DIR] [--format text|json] [--limit N]
                 [--rules FILE]... [--raw] [--no-refresh] [--expect-empty]
codeintel schema [--path DIR] [--format text|json]
codeintel status [PATH] [--format text|json]
codeintel mcp    [--path DIR]
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
restricts grammars: files of every other language are neither refreshed nor
dropped, they are carried forward unchanged.

`index` prints the exact indexer command for every language it found.
Copy-pasteable, not a description of one. It never runs one: `--run-indexers`
is not built ([docs/plan.md](../docs/plan.md) M3, § Explicitly deferred).

Prints a summary to stderr: files indexed/skipped/unchanged, facts per relation,
SCIP coverage, anchor rate, SCIP occurrences skipped because their column is
ambiguous ([02-extraction.md](02-extraction.md) § Position normalization),
elapsed. Skipped files are summarized **by reason**
— "3 too large, 88 unsupported (`.scala`)" — because a silently unindexed
subtree is the single most confusing failure this tool can have.

**There is no `ignored` count.** The `ignore` crate does not report the entries
its matchers drop, so the number costs a second full traversal with the matchers
off — which on a repo with a populated `target/` is the most expensive thing in
the walk, spent on a figure that changes no decision. An earlier draft of this
line read "412 ignored, 3 too large, ...".

### `query`

```sh
codeintel query '?- calls(C, S), def(S, _, _, "get").'
codeintel query - < investigation.dl
```

**Auto-refresh, on by default.** Before evaluating, `query` stats the files in
the manifest and re-extracts any that changed. An agent's normal loop is edit →
ask, and answering that from a stale index produces a confidently wrong answer —
the worst failure this tool has, because it looks like a right one.

- Tier A only. SCIP cannot be refreshed incrementally
  ([04-storage.md](04-storage.md) § Incremental reindex), so a file edited since
  the SCIP index was built is re-extracted with tier A alone: fresh `name_ref`,
  no `scip_ref`, `local` definitions
  ([02-extraction.md](02-extraction.md) § The anchor join). That is reported as
  `scip-stale`, which auto-refresh makes **more** likely, not less — so the
  status matters more now, not less.
- **Auto-refresh carries the manifest's SCIP inputs forward.** What it must
  never do is refresh with no SCIP at all: that looks like "the index
  disappeared", re-extracts the tree tier-A-only, and deletes every tier-B fact
  — the store would degrade a little more with each query, silently. Rerunning
  the indexer and `codeintel index` is what reconciles a `scip-stale` index.
- Bounded by `max_refresh_ms` (default 2,000). If the refresh would exceed it,
  it is abandoned, the query runs against the index as it stands, and the
  response carries `status: "stale"` naming the files it could not refresh.
  Never a silent slow path and never a silent stale answer.
- `stats.refreshed` **counts** the files re-extracted before the answer, as an
  integer. It was specified as a list, and a list has no bound: one
  `git checkout` can re-extract hundreds of files, and every response would
  carry every path (#19). A refresh that runs out of time already names the
  files it left behind, five at most, in the `stale` hint. Zero is the common
  case. It
  costs a gitignore-aware walk of the tree, one `stat` per file (which is how
  new files are found), and **no write**
  ([04-storage.md](04-storage.md) § Incremental reindex). ~5 ms for 799 files.
- `--no-refresh` skips it, for benchmarking or for querying a deliberately
  pinned index.
- New files are picked up; **deleted files are pruned.** A refresh that only
  added would leave a deleted file's facts answering queries.

**`--rules FILE` is where a repository keeps its own conformance rules** —
layering, banned dependencies, allowed directions — so they live in the
repository under review rather than in this binary. Repeatable, loaded after
`stdlib.dl`.

**Clauses in a rule file are additive, not replacements.** A predicate is the
union of its clauses, so a file defining `is_test` *widens* it. Only a rule
written in the query program itself shadows a loaded one, and that shadowing is
reported on stderr and in `stats.shadowed` — a repository rule quietly replacing
`is_test` would change every answer that reads it. A repository that means to
replace a stdlib rule puts it in the program, not in a rule file.

**`--expect-empty` exits 1 if any row comes back.** A conformance check states
the violation it looks for, so finding none is the passing case. The exit code
is 1 and not 2 because "your code violates this" and "I could not tell you" are
different results in CI: 2 stays reserved for a query that never ran.

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

For that to be true the engine's own cap must not be the one that fires. The
rendered form is **shorter** than the raw one — a ~68-character `SymId` becomes
`name path:line` — so an engine holding the printed budget would truncate rows
that fit it, and the caller would silently receive a fraction of the answer the
budget allows. The host therefore gives the engine the same budget with
headroom, where it serves as a bound on materialisation rather than as the
answer's size limit.

**Both notations of a row are produced together and ordered once**, by the
display text. Rendering each separately and sorting each gives two arrays whose
`i`th entries are different tuples, so a consumer that shows the display form
and feeds the raw form into its next query binds the wrong symbol. One
consequence worth stating: `--raw` prints the same rows in the same order as the
default, because it is the same answer in a different notation.

**A goal with no variables prints `true` or nothing.** Truth is one empty row
and falsehood is none ([03-datalog.md](03-datalog.md) § Evaluation), so printed
literally the two differ by one invisible newline while both carry `status: ok`.
The word is the answer. JSON is unchanged — one empty row versus none, which is
already unambiguous there.

**A symbol whose `Name` is empty renders as its location alone.** SCIP gives a
crate-root module an empty `display_name`; holding the column open prefixes the
location with a space and reads as a glitch.

Values are carried structurally — one string per column — until the moment of
printing. Tab-joining and splitting back apart loses the column boundaries of
any value containing a tab, which doc comments do.

**At the moment of printing, a value's own control characters are escaped:**
newline as `\n`, carriage return as `\r`, tab as `\t`, backslash as `\\`. Text
output promises one row per line and `columns.len()` tab-separated fields; a
raw newline in a `def_doc` breaks both, and it breaks them *silently* — the
consumer sees more rows than the query matched, with ragged field counts, and
nothing in the status or the row count says so. One multi-line doc comment
turns a single row into seven lines of which six are not rows. That is a
malformed answer wearing `status: ok`, which is invariant 5, and no amount of
"read the notes" fixes a format that lies about its own shape.

Backslash is escaped too, so the transformation is reversible: without it, a
literal `\n` in source text and an escaped newline are the same two characters
and a consumer cannot tell them apart. JSON is unaffected — it has always
escaped these — which is why the defect was invisible to anything that checked
the structured format.

**A truncation hint names a knob that exists.** `--limit` moves
`max_result_rows` and nothing else, so it is offered only when that is the cap
that fired; a byte cap says "narrow the query".

**When both caps fire, `cap` names the one that shaped the output.** The
engine can stop at `max_result_rows` and the printed budget can then cut
further: on deno, `--limit 5000` kept 5,000 rows and the byte budget printed
2,757 (#27). The rows missing from the answer were dropped by the byte cap,
so that is the cap reported, with its advice. Naming the row cap there told
the consumer to raise `--limit`, which returns the same 2,757 rows every time.
A consumer that follows the hint loops, and invariant 7 is not satisfied by a
cap that fired but did not decide the answer.

### `schema`

Prints the relation catalog, atom vocabularies, and stdlib rule signatures.
**Must fit in ~1500 tokens** — this is the text an agent reads to learn the
system, and it is the highest-leverage output in the project.

**The rule list is generated from `rules/stdlib.dl`, not written here.** One
`%%` line above a predicate's first clause carries its signature and its
one-line doc:

```
%% within(Child, Ancestor)  transitive containment. Child is FIRST.
within(C, P) :- parent(C, P).
```

The signature is written out rather than lifted from the clause because heads
carry constants — `ref(..., "exact")`, `about(S, "sig", ...)` — and the
*argument names* are what an agent needs. A test asserts both directions: every
advertised head names a real predicate at its real arity, and every predicate in
the file is advertised. A rule cannot ship unlisted, and a renamed one cannot
leave a stale catalogue behind.

**The 1500 is a band, not a number, and the output sits at the top of it.**
No tokenizer is available offline and adding one is a dependency for a figure
that gates copy length. Estimates of this text span **~1,400 tokens** at four
characters per token — prose-like, and what an earlier draft of this paragraph
claimed — to **~1,750** under a BPE-shaped count that charges per punctuation
mark and per identifier fragment, which is what dense tabular output actually
looks like. Today: **5,599 characters, 38 rules**, and the honest statement is
that `schema` is *at* its budget rather than inside it.

That matters because it is now the *only* constraint the standard library
spends against. The RULES section is ~2,900 of those characters — **over half
the output** — so every rule added spends copy that is genuinely not there, and
it spends it visibly. The CI assertion is therefore on
characters, which can be measured, at a ceiling the current output meets: it
catches growth, which is what it is for. If it fires, the standard library is
too big — cut rules, not the catalogue.

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

`--format json` carries the same catalogue **structurally**: `relations` with
argument names and row counts, `kinds` and `roles` as the closed vocabulary with
this index's count against every value including the zeros, `rules` with their
signatures, and `text` for the rendered form. A consumer must never regex the
prose to learn that a kind is at zero — that is the same defect as handing
`query` a `display` column and no raw one.

### `status`

Index freshness, per-tier. Reports the counts a user needs to trust or distrust
an answer: files indexed, files changed since index, SCIP tool/coverage/
staleness/ambiguous columns, anchor rate, unsupported languages, fact counts per relation.

`status: ok` when nothing has changed since the index was written, `stale` when
something has, `no-index` when there is nothing here. A missing index **exits
0**: "there is no index in this directory" is an answer, not a failure of the
command that reported it.

The unsupported-extension count costs a full walk, which is why it lives here
and not on every query. It is printed biggest-first, because the number that
matters is the one that turns out to be a whole unindexed subtree.

`--format json` is the bug-report artifact for a tool with no telemetry. It
carries the extractor fingerprint, the dictionary generation, the SCIP inputs,
per-relation row counts for **every** relation including the zeros — an absent
key would read as "not measured" rather than "empty" — and per-language
`{files, defs, refs, imports}`. "python: 1,204 files, 11 defs" is visibly absurd
to a human in one second; `status: ok` is not.

Per-language attribution goes through the relations that carry a file column
(`def`, `scip_ref`, `name_ref`, `import`) and the manifest's file→language
table. `exported`, `resolved` and the rest have no file column and are reported
as totals only, rather than joined through `def` to invent a language for them.

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
      "schema": { "type": "boolean", "description": "Return the relation catalog and exit." },
      "limit":  { "type": "integer", "default": 200 },
      "format": { "type": "string", "enum": ["text", "json"], "default": "text" },
      "raw":    { "type": "boolean", "description": "SymIds instead of `Name path:line`." }
    }
  }
}
```

Exactly one of `query` or `schema` per call; both is an error naming the two.

**`rule` and `args` are not in this schema.** They were, and they carried the
defect that deleted the `rules` CLI verb: `args` is an array of strings, and an
integer and its string form are different atoms ([01-facts.md](01-facts.md)
§ Integers), so `{"rule": "innermost_at", "args": ["src/store.rs", "142"]}` bound
the *string* `"142"`, matched nothing, and returned `ok` with zero rows. Coercing
digit-only arguments would be the extractor guessing, which invariant 1 forbids,
and would make a symbol genuinely named `142` unaddressable. An agent writing
`?- innermost_at("src/store.rs", 142, S).` gets the integer right because it is
writing Datalog rather than filling positional slots.

**The server stays warm between calls.** `codeintel mcp` is one long-lived
process. Reloading the index on every call cost ~18 ms of a ~40 ms answer on a
799-file tree (#18). The server keeps the loaded engine (relations, the standard
library, symbol sites) and reuses it for as long as the manifest *after refresh*
equals the manifest the engine was built from. A refresh that commits anything,
or another process's `codeintel index`, moves the manifest, and the next call
rebuilds. **Every call still refreshes first.** Warmth skips the load, never the
refresh that keeps an answer current. A call that ends in `corrupt`, `no-index`
or a schema mismatch keeps nothing warm.

**Query literals do not outlive their call.** A query interns constants the
corpus does not contain. A long-lived engine would keep them, so atom ids would
depend on what was asked earlier — and so would which rows a truncated answer's
stable prefix holds ([03-datalog.md](03-datalog.md) § Determinism). The host
therefore gives the engine an **overlay dictionary**: a corpus string resolves to
its stored atom, any other string gets a scratch atom above the dictionary, and
the scratch range is emptied after every call. The CLI builds the same engine
through the same code, once. A warm server and the CLI give byte-identical
answers to the same query on the same index. `crates/datalog` is unchanged: the
overlay is a `Symbols` implementation in this crate.

**Transport is newline-delimited JSON-RPC 2.0 on stdin and stdout, hand-written.**
A message with no `id` is a notification and gets no reply. There is no MCP SDK
dependency: the protocol surface this server needs is `initialize`, `tools/list`,
`tools/call` and `ping`, `serde_json` is already present for the response
contract, and the whole transport is ~150 lines. Recorded in
[research.md](../docs/research.md) §6.

### Response contract

```json
{
  "status": "ok",
  "columns": ["C", "F", "L"],
  "rows": [["handle_read", "src/api/handler.rs", 42]],
  "truncated": false,
  "hint": null,
  "stats": { "derived": 18422, "elapsed_ms": 7, "refreshed": 0, "transformed": ["impact_of"],
             "demand": "applied", "depends": ["def", "name_ref", "scip_ref"] }
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
- **A constant in a typed column is diagnosed against that column, not against
  the relation.** "the index holds 1448 `def` rows" is true and useless: the
  agent's error was the *value*, and the relation total says nothing about it.
  Two column types carry enough information to say more, and both are the
  subject of a recorded eval failure:
  - **Closed vocabulary** (`Kind`, `Role`, `Prov`, `Lang`) — report that
    column's values with this index's counts. An agent that writes
    `Kind="type"` for a Rust struct needs to see `struct 79` beside `type 1`;
    an agent that writes `Kind="klass"` needs to see that no such value exists.
    Listing the vocabulary is a fact about the index, not a guess about intent,
    so it stays inside invariant 1 — **do not suggest a replacement value.**
  - **Path** (`F`, `G`) — the constant an agent gets wrong most often, because
    paths arrive from `ripgrep`, `git diff` and stack traces, and those emit
    absolute and `./`-prefixed forms while the index keys on repo-relative
    ones. Report, in order: that stripping the repo root or a leading `./`
    yields a path that *is* indexed; else that a file with the same basename is
    indexed elsewhere; else that neither the path nor the basename is known,
    with the file count and a pointer to `status` for what was skipped. All
    three are facts about the index. The basename case in particular separates
    "you named the wrong directory" from "this file is not indexed at all",
    which are different problems with different fixes.
  - **Integer** (`Line`, `Col`, `StartLine`, `EndLine`, `StartByte`, `EndByte`)
    — a quoted constant in one of these can *never* match, because an integer
    and its string form are different atoms. Say so and show the unquoted form.
    This is the footgun the `NOTES` block already warns about, which is
    evidence a warning in the preamble is not where an agent reads it.

  This stays a `hint` on an `ok` result rather than becoming `invalid-query`.
  The query is well-formed and the answer — no rows — is true. Promoting it to
  an error would make the taxonomy claim a malformed program where there is
  only an empty one, and the taxonomy's whole job is keeping those apart.
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
