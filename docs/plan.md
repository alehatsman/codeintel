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

## What the 2026-09-12 review changed

An eight-reviewer adversarial review of the specification, before any code.
Findings are archived; the ones that moved this plan:

**1. `tags.scm` vendoring does not work. We author our own queries.**
Verified by fetching all nine upstream files. `@reference.call` is absent from
TypeScript, C, and C++. Python emits no `@definition.method`. Rust collapses
`struct_item`, `enum_item`, `union_item` **and** `type_item` into one
`@definition.class` capture, so every Rust enum and type alias would be emitted
as `Kind = "struct"` — an extractor guessing wrong, which invariant 1 forbids.
TypeScript's file captures only ambient `.d.ts` forms, so ordinary TS yields
nothing. `docs/research.md` verified that `tags.scm` *exists* and concluded each
language *costs one table row*. Existence is not sufficiency.

**2. Therefore the extraction budget, not the engine budget, was the fiction.**
`~/projects/dex/internal/graph/` spends ~6,177 LOC on tree-sitter extraction for
five languages — hand-written per-language queries plus per-language scope
recovery. This plan budgeted ~900 LOC for nine. **Languages narrowed to four.**

**3. The founding argument is stronger than `research.md` states it.** dex
adopted the one-composable-verb thesis in 2026-08 and shipped it. Today dex has
**20 registered MCP tools with `query` as the twentieth**, 34 store methods, and
84,887 non-test LOC — unchanged after ~9,500 lines of deliberate deletion, with
its pipe grammar already accreting new stages. dex is not a strawman and not a
refutation: **dex is the experiment, and the surface did not collapse.** The
claim that survives is narrower and testable — *question N+1 is a line of data,
not a branch in a parser* — and it is asserted in CI from commit one.

**4. The stdlib does not typecheck against the engine.** `symbol_at` puts `Line`
in the head and binds it only in comparisons, violating safety rules 1 and 3.
`innermost_at`, `tighter_at`, and `lexical_parent` all sit on it. Four of five
`is_test` clauses fail the same rule.

Fixed with a **`between(Lo, Hi, X)` generator builtin**, not with mode
declarations. The alternative — declare the rule's bound arguments and check
safety after demand transformation — makes `symbol_at` legal *only* under the
transformation, which means the naive evaluator cannot run it. The naive
evaluator is this milestone's acceptance gate, so that would leave the
transformation with nothing to be differentially tested against, and "it
silently did not apply" is precisely the failure mode being guarded. `between`
keeps legality and performance separate: the rule is unconditionally safe,
demand transformation only makes it fast. `is_test` gains a `file(F, _)`
literal; assignment- and generator-binding are stated explicitly in safety
rule 7.

**5. Performance objections did not survive measurement.** Claims that auto-
refresh and `ambiguous/1` break the latency budget were built by pairing the
100k-LOC success criteria against the 1M-symbol scale note — two numbers ~200x
apart. Measured: `stat` is 1.5 us warm (1.5 ms for 1,000 files, not 5-20 ms);
an 8M-row sort is 669 ms, not 2.8 s; `Sigma k^2` for `ambiguous` is **8,486** on
dex and 11,330 on `tracing`, not the asserted ~25M. The `ambiguous` rewrite is
still worth doing because it is free. The architecture change is not.

**6. Conformance is the capability that works on first run.** `import/3` is
extracted by `queries/<lang>/imports.scm`, which is **ours**, so none of finding
1 touches it. "No module under `ui/` may import from `db/`" needs no SCIP, no
`calls`, no `ref`. It is now the headline.

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
  trigger written down. **§6 must list `blake3` and `regex`** — both are used
  (`04-storage.md` names segments by blake3; `regex` is a non-optional transitive
  dependency of `tree-sitter` itself) and neither appeared in the budget that
  `CLAUDE.md` calls "the entire budget."
- A test asserts `crates/datalog/Cargo.toml` depends on neither `facts` nor
  `extract`. This seam is what keeps the engine honest; guard it mechanically
  from commit one.

---

## M1 — Datalog engine

[03-datalog.md](../specs/03-datalog.md), end to end, with **no code-intel
concepts**. This is the highest-risk milestone; do it first and do it properly.

1. Interner (`facts` crate): reserved integer range, append-only dict, mmap load.
   **`dict.idx` holds u64 offsets** — u32 silently caps `dict.bin` at 4 GiB.
2. Lexer + parser → AST. Fuzz it.
3. Safety checks: range restriction, negation safety, comparison safety,
   aggregate safety, arity consistency, base/derived exclusivity. Each with its
   diagnostic. **Seven rules, not eight** — the opaque-atom rule is gone with
   opaque atoms.
4. Stratification: dependency graph, negative-edge cycle detection, topo order.
5. `Relation`: flat `Vec<u32>`, sorted, deduplicated, binary search on prefix.
6. Semi-naive evaluation with the most-bound-first literal ordering.
7. **Demand transformation, applied to non-recursive predicates as well as
   recursive ones.** A performance requirement, not a correctness one — every
   stdlib rule is safe as written (see the `between` builtin). With `Line`
   bound, `symbol_at` is a binary search into `def_span` sorted by `(F, L1)`
   instead of a materialisation of ~LOC x nesting-depth rows; unbound it is
   ~250k rows at 100k LOC, which is wasteful and correct. The same
   transformation stops `impact_of` computing all-pairs reachability, and
   `ref`, `calls`, `about`, `is_test` and `depends` are all non-recursive, so
   excluding non-recursive predicates would forfeit most of the win.
8. Builtins: comparisons, arithmetic, `between/3`, `match`/`prefix`/`suffix`/
   `contains`, and `count`. **`match` delegates to the `regex` crate** — hand-rolling a
   backtracker buys no dependency reduction, since `tree-sitter` already depends
   on `regex`, and it loses linear-time guarantees. Inject it as a host builtin
   so `crates/datalog` keeps its zero-dependency property.
9. Limits (rows **and bytes**), truncation reporting, `Stats`.
10. **The cheap agent eval.** See below.

**Done when**
- The conformance suite passes: transitive closure, same-generation, stratified
  negation, `count`, arithmetic.
- The `#[cfg(test)]` naive evaluator agrees with semi-naive on every conformance
  program. *This is the acceptance gate* — a semi-naive bug that drops rows is
  otherwise invisible until it corrupts a real answer.
- Every safety rule has a test asserting its exact diagnostic message.
- Every limit has a test provoking it and asserting `status` and `cap`.
- 100 runs of each conformance query are byte-identical.
- `cargo fuzz run parser` survives 10 minutes with no panic. **Run: 33,197,154
  executions in 601 s, zero crashes.** `tests/robustness.rs` covers the same
  property on every commit from a seeded generator, for machines with no
  nightly toolchain.
- `crates/datalog` has no dependencies. `match` arrives as an injected builtin.
- **Demand transformation demonstrably applies, to both kinds of predicate:**
  for each seeded traversal *and* for `symbol_at`, a bound goal derives strictly
  fewer tuples than the same goal with the transformation disabled, and both
  return identical results. Equal counts mean it silently did not apply.
- **`load_rules(include_str!("../../rules/stdlib.dl"))` returns `Ok` — and so
  does the naive evaluator.** The engine must accept its own standard library,
  and it must accept it *as written*, with no transformation applied. If a rule
  is legal only after demand transformation, the differential test has nothing
  to compare against. This one line is the guard for
  the whole class of defect the review found by reading two specs against each
  other.
- **The agent eval passes at >= 24/30 — against hand-written facts.**

### Why the agent eval belongs here, not at M4

The load-bearing bet of this project is that an agent can write correct Datalog
over this schema. If that is false, the schema or the surface is wrong, and
every milestone after this one is built on top of the mistake.

Testing it needs **no extractors**: hand-write a fact file for a small,
realistic repo, write `rules/stdlib.dl` against it, generate `schema` output,
and give an agent nothing but that plus the questions.

**n = 30, not 10.** Ten trials cannot separate "works" from "coin flip" — the
95% interval on 8/10 spans roughly 0.49 to 0.94. Thirty questions, written
before the schema is frozen, three samples each.

Record every failure verbatim in `docs/agent-eval.md`. **Do not pre-commit to a
diagnosis.** The previous wording ("a failure is a `schema` wording bug far more
often than an agent bug") is a confirmation-bias trap: an agent reading it will
revise the copy and re-run forever. Three hypotheses are live — the copy, the
schema, and the language — and the transcript decides which.

**If the eval comes in below 24/30, stop and raise it.** That is a human
decision, not an implementer's.

### What M1 actually cost, and what it found

Shipped: `crates/datalog` (parser, eight safety checks, stratification,
relations, semi-naive evaluation, demand transformation, builtins, limits),
`crates/facts`' interner, `rules/stdlib.dl`, and the regex builtin injected from
`crates/codeintel`.

Four defects, each caught by a gate this plan asked for rather than by review:

1. **The planner hoisted an aggregate above the literal that bound its group
   variable**, so `N = count{ C : calls(C, S) }` counted over every `S` at once.
   Wrong, not slow, and silent. Found by the conformance suite.
2. **`ref_outside/2` in [01-facts.md](../specs/01-facts.md) is not
   range-restricted** — `F` appears in the head and only in a comparison. The
   engine rejected its own standard library. Found by the one-line
   `load_rules(include_str!(...))` gate, which is exactly the class of defect it
   was added for.
3. **`max_strata` defaulted to 32 and the stdlib stratifies into 37**, so every
   query against the shipped rules was rejected at planning.
4. **The interner's append path rewrote the trailing offset**, so `alpha` and
   `beta` resolved to `alphabeta`.

The agent eval came in at **88/90 first-attempt** against the 24/30 gate, with
both failures recorded verbatim in [agent-eval.md](agent-eval.md). One of the
two was a genuine schema-copy ambiguity (`within(C, A)` never said which
position is the child); the fix is unmeasured and M4 re-tests it.

Open at the end of M1: `rules/stdlib.dl` holds 37 named predicates against a cap
of 24 (see M4 below).

---

## M2 — Tier A, one language

Rust only. Walk → parse → extract → segment → load → query.

1. `ignore`-based walk, binary/size filtering.
2. **Author `queries/rust/{tags,imports}.scm`.** Not vendored. Upstream's
   `tags.scm` maps `struct_item`, `enum_item`, `union_item` and `type_item` all
   to `@definition.class`, has no `@definition.constant`, treats `impl_item` as
   `@reference.implementation` rather than a definition, and double-captures
   methods in a `declaration_list` as both `@definition.method` and
   `@definition.function`. Upstream is a **reference**, not a dependency.
   Budget ~1,000 LOC of query + mapping per language, per the dex precedent.
3. Span-nesting containment sweep. **Write this once, in a shared function** —
   tier B calls the same code in M3.
4. Emit `file`, `def`, `def_span`, `def_name`, `def_sig`, `def_doc`, `parent`,
   `exported`, `import`, and `name_ref`. **Tier A resolves nothing** — see
   invariant 3b. Resolution is `stdlib.dl`'s job.
5. Segment writer + manifest, per [04-storage.md](../specs/04-storage.md).
   Includes `writer_version`, `extractor_fingerprint`, `dict_bin_len`,
   `dict_idx_len`, a per-segment body checksum, and `fsync` before the manifest
   rename.
6. `codeintel index`, `codeintel query`.

**Done when**
- `codeintel index` on this repo, then
  `codeintel query '?- def(S, F, "function", N).'` returns real rows.
- Golden fact file for `tests/fixtures/rust/` matches exactly.
- Span exactness test passes: for every `def`, `src[start_byte..end_byte]`
  reparses to the same kind and `src` at `def_name` equals `Name`.
- **Kind fidelity: a fixture containing a struct, an enum, a union, a type alias,
  a trait, a const and an impl block emits seven distinct correct `Kind` values.**
  This is the test upstream's query fails.
- **Query liveness: every pattern in every `.scm` matches at least once on its
  fixture, and no node receives two conflicting `@definition.*` captures.**
  A dead pattern is the signature of a grammar node rename, and it otherwise
  surfaces as `status: ok` with zero rows.
- Indexing twice produces byte-identical segments.
- Indexing with shuffled file order produces identical facts.
- Deleting a file and reindexing removes its facts.
- **Locality test passes:** every fact for file X is reproducible by extracting
  X alone with nothing else indexed.
- **Incremental equivalence passes:** index, mutate one file, reindex
  incrementally → **equal as resolved fact sets** to a cold reindex. *Not*
  byte-identical: an append-only interner assigns atom ids in encounter order,
  so a symbol added to an early file lands after every later file's atoms
  incrementally and inside its own block cold. The byte-identical form is
  unsatisfiable and was specified twice.
- `?- innermost_at("<some file>", <some line>, S).` returns the right symbol.
- **A conformance rule runs and fails correctly:** a fixture with a deliberate
  `ui/ -> db/` import returns the violating row; removing the import returns
  zero rows with `status: ok`. This is the headline capability and it lands at
  M2, before any SCIP exists.
- Auto-refresh on `query`: edit a file, query without reindexing, get the new
  answer. Delete a file, query, its facts are gone. Refresh **commits each
  file's segment as it completes** — an all-or-nothing refresh that abandons its
  work at `max_refresh_ms` never converges, so a large `git checkout` would
  leave every subsequent query paying the full budget and returning `stale`
  forever. Partial progress is committed; only the unfinished files are reported
  stale.
- `query` takes the same `flock` as `index`. On contention it reads the current
  manifest and returns `stale`, never `locked` — `locked` is for a second writer
  and is not actionable by a reader.

### What M2 actually cost, and what it found

Shipped: `crates/facts`' relation table, segment format, manifest, store and
writer lock; `crates/extract`'s language table, walk, containment sweep, symbol
synthesis and Rust tier A; authored `queries/rust/{tags,imports}.scm`; and the
`index` and `query` verbs. `tests/fixtures/rust/` and its golden fact file.

**Two of the four M2 questions that mattered were about the engine, not the
extractor** — and both were found by pointing the finished tier A at this
repository (52 files, 1,070 definitions, 4,173 `name_ref`s), which is the first
time anything larger than a hand-written fixture existed.

1. **Every stratum was evaluated, whatever the goal asked for.** `stdlib.dl`
   ships `reaches/2`, an unseeded all-pairs closure, so `?- def(S, F,
   "function", N).` — one scan of one base relation — cost whole-graph
   reachability and hit `max_time_ms`. Fixed by restricting evaluation to the
   goal's dependency closure ([03-datalog.md](../specs/03-datalog.md)
   § Evaluation).
2. **Demand transformation silently stopped applying.** It is validated after
   rewriting and falls back to the plain program when the rewrite does not
   stratify, so when seeding `!tighter_at` made `impact_of` unstratifiable, the
   *whole* query fell back — and the spec's own headline,
   `?- innermost_at(F, L, S), impact_of(S, C), ..., !is_test(F).`, ran past
   120 s. The fall-back is now per predicate: 168 ms. M1's gate proved the
   transformation applied to a two-rule test program; it did not prove it
   applied to the shipped rule library, and that is now asserted.
3. **`//!` is not an item's doc comment.** Treating it as a doc marker attached
   every file header to whichever definition came first.
4. **`ignore` does not apply `.gitignore` outside a git checkout** unless
   `require_git(false)` is set, so a worktree indexed all of `target/`.

Two spec conflicts were surfaced rather than reconciled silently: `Union`
mapping (resolved to `type` in both tiers, [02-extraction.md](../specs/02-extraction.md)
§ Ingest) and the `ignored` skip count, which is deleted because the `ignore`
crate does not report what its matchers drop and recovering the figure costs a
second full traversal ([05-surface.md](../specs/05-surface.md) § `index`).

Deferred to M4 on purpose, and not silently: `--raw`, `--rules`,
`--expect-empty`, symbol rendering as `Name` + `path:line`, and the
first-zero-literal `hint` on an empty result. `--scip` is M3. `index` and
`query` carry `--format`, `--limit` and `--no-refresh` today.

---

## M3 — Tier B, one language

`rust-analyzer scip .` → facts → the anchor join.

1. `scip` crate; read `index.scip`.
2. Position normalization, including the UTF-16 case and its skip-and-report
   path.
3. `local N` → `local <path> N` rewrite. **Test this specifically** — the bug it
   prevents is silent and repo-wide.
4. `SymbolInformation.kind` → our kinds.
5. Reference `From` attribution via the M2 containment sweep.
6. The anchor join: match on `def_name` position, rewrite the tier-A atom to the
   SCIP symbol in the interner, emit `resolved(S)`.
7. `implements`, `extern`, and `scip_ref`.
8. Parent precedence: descriptor prefix → `enclosing_symbol` → tier A span
   nesting, replacing tier A's row after the anchor join.

**`--run-indexers` is not built.** `index` prints the exact command for every
detected language, which is where the value is. Running another project's build
system inside this tool buys a 600 s subprocess, seven environment-failure
matrices, and the only path by which this binary executes arbitrary code. The
printed command is copy-pasteable and the user runs it.

**Done when**
- On `tests/fixtures/rust/` with both tiers, >= 95% of tier-A definitions carry
  `resolved(S)`, **counting definitions a position join can reach**. Modules
  are exempt and the exemption is pinned by count
  ([02-extraction.md](02-extraction.md) § Validation): tier A places module
  `conn` at `mod conn;` in one file and rust-analyzer places it at line 1 of
  another, so no join on identifier position can match them. Measured: 33/33
  non-module, 0/9 module.
- Tier-A precision test passes: every `"name"` ref either matches an `"exact"`
  ref at the same position or is listed in `known-imprecise.txt` with a reason.
- `?- calls_exact(A, B).` returns edges that a **tier-A-only** `?- calls(A, B).`
  misses, and the set is hand-audited in both directions. The original wording
  was unsatisfiable: `calls_exact` is `calls_at` with `Prov` pinned to
  `"exact"`, so against a full index it is a subset of `calls` by construction.
  What it buys there is precision, not reach. The fixture carries `open`
  exported twice so that tier A's `!ambiguous(N)` guard correctly refuses the
  edge and tier B supplies it; "a sample of 20" is also more edges than a
  fixture this size should have, and the whole set is audited instead.
- Deleting `index.scip` and reindexing degrades cleanly to `"name"` only, with
  `status: "no-scip"` on a query that needs precision.
- **`no-scip` fires on the relation-dependency closure, not on literal syntax.**
  A query reaching `calls`/`ref`/`impact_of` with no SCIP present must say so.
  Otherwise the README's own headline query returns `status: ok` with zero rows
  on a fresh install — the exact failure invariant 6 exists to prevent.
- `codeintel index` prints the exact indexer command for every detected language.
- **A file edited after `index.scip` was built reports `scip-stale`, and the
  spec states which `SymId` its tier-A rows carry.** Settled: auto-refresh
  carries the manifest's SCIP inputs forward and re-extracts the changed file
  against the *existing* ingest, so a definition whose name token has not moved
  keeps its resolved identity and one that has moved reverts to the synthesized
  `local <path> ...` form. A full `codeintel index` reconciles it. What
  auto-refresh must never do is refresh with no SCIP at all — that silently
  deletes every tier-B fact ([05-surface.md](../specs/05-surface.md) § `query`).

### What M3 actually cost, and what it found

Shipped: `crates/extract`'s `scip.rs` (normalization) and `tier_b.rs` (the join
and tier-B facts); anchors threaded through tier A; `--scip`; the `no-scip` and
`scip-stale` statuses; `Stats.depends` in the engine. The fixture became a real
cargo crate with a committed `index.scip` from `rust-analyzer scip .`.

**The anchor join was the easy half.** The join itself is one `BTreeMap` keyed
by identifier position, and doing it *before* tier A writes — rather than as a
rewrite pass afterwards — made `def_span`, `parent` and every reference's `From`
land on the resolved identity for free. That is what the spec said; it is
cheaper than it sounds.

What actually cost time:

1. **`query`'s auto-refresh deleted tier B.** The refresh built its `Plan` with
   no SCIP inputs, which reads as "the SCIP index disappeared" — all-or-nothing
   invalidation then re-extracted the tree tier-A-only and dropped every
   `scip_ref`, `resolved`, `implements` and `extern` row. Three queries in a row
   and the index had silently lost half of itself, with `status: ok` throughout.
   Found by querying the fixture after indexing it, not by any test that existed.
   Auto-refresh now carries the manifest's inputs forward, and
   `auto_refresh_does_not_delete_tier_b` pins it.
2. **Two of the milestone's own done-whens were wrong.** The 95% anchor rate is
   unreachable while tier A treats `mod x;` as a definition — the two tiers put
   a module in different *files*. And `calls_exact` is a subset of `calls` by
   construction, so "edges `calls` misses" only means anything against a
   tier-A-only index. Both are corrected above with the reasoning, not quietly
   relaxed.
3. **Parsing `index.scip` on every query is not free.** The ingest is now
   stat-gated: the inputs are `stat`ed, and the protobuf is decoded only when a
   file actually needs re-extracting. A refresh that changes nothing reads no
   protobuf at all.
4. **SCIP's `extern` test had to be inverted.** "Package ≠ project package" is
   not checkable — SCIP never says what packages a project is. It says exactly
   what symbols the project defines, so `extern` is the complement.

Two spec deviations were surfaced rather than reconciled silently: `seg/_scip.bin`
is gone in favour of ordinary per-file segments for tier-B-only files, and
`ScipInput` compares `mtime`+`size` while ignoring `tool` and `documents`, which
are only known after a parse. Both are written into
[04-storage.md](../specs/04-storage.md).

Measurements worth keeping. On `tests/fixtures/rust/`, tier A alone finds two
call edges, one of which is a false positive; with SCIP, `calls_exact` finds
three and all three are correct. On **this repository** — 61 files, 1,262 tier-A
definitions, `rust-analyzer scip .` in 4.3 s, index in 717 ms:

| | |
|---|---|
| anchored | 1,185/1,262 (93.9%); 97.4% of non-module definitions |
| `?- calls(A, B).` | 1,473 edges, 355 ms |
| `?- calls_exact(A, B).` | 1,221 edges, 364 ms |
| the headline query | 21 rows, 376 ms |

**Tier A over-reports 252 call edges here, 17% of what it claims.** That number
is the entire argument for tier B, and it is also the argument for keeping both
provenances rather than collapsing them: the 252 are inspectable, and a jump in
that figure means one of the tiers changed behaviour.

Still open, and named rather than hidden: SCIP emits a `local` definition for
every parameter and binding — 16 in this fixture — and the collapse table has no
word for `Parameter`, so they land in `def` as `unknown`. That is the spec as
written and invariant 1 forbids the extractor deciding a parameter is not a
definition. A `stdlib.dl` rule naming the set is the cheap fix if it bites.

---

## M4 — Surface and stdlib

1. `rules/stdlib.dl` — every rule in [01-facts.md](../specs/01-facts.md) §
   Derived relations, each with a doc comment and a fixture test.
2. Output rendering: symbol columns as `Name` + `path:line`, `--raw` for the
   joinable form, the byte cap wired through `truncated`/`cap`. `--raw` output
   carries the dictionary generation from the manifest, and a mismatched
   generation is rejected — `--rebuild` renumbers every atom, so a stale raw id
   otherwise resolves to a *different string* rather than to an error.
3. `schema`, `status`. **`status --format json`** — it is the bug-report artifact
   for a tool with no telemetry, and it must carry the extractor fingerprint and
   per-relation, per-language fact counts. "python: 1,204 files, 11 defs" is
   visibly absurd to a human in one second; `status: ok` is not.
4. `codeintel mcp` — one tool, the full status taxonomy, hints on every
   non-`ok`.
5. The surface-budget CI test: assert 1 MCP tool, **5 CLI verbs**, <= 16 base
   relations, **and <= 24 named predicates in `stdlib.dl`**. The rule count is
   the dimension that actually grows; asserting only the three that do not makes
   the founding thesis unfalsifiable.

   **Open conflict, to be resolved here.** `rules/stdlib.dl` shipped at M1 with
   **37** named predicates, and every one of them is specified in
   [01-facts.md](../specs/01-facts.md) § Derived relations. So the cap and the
   fact schema disagree, and they have disagreed since both were written. M4
   either cuts rules or amends the cap with a written reason — but it does not
   get to leave the assertion out, because that is the only budget the founding
   thesis is falsifiable against.

6. Render rows in lexicographic order of their printed text
   ([05-surface.md](../specs/05-surface.md) § `query`), after the row cap. The
   engine sorts by atom, which is insertion order, and a cold index and an
   incremental one number the same string differently — so engine order alone
   would make output depend on how the index was built.

**Done when**
- Every rule in `stdlib.dl` has a fixture test with a hand-verified answer.
- Every question in [research.md](research.md) §1a's 34-method list is
  answerable in <= 5 lines of Datalog, written down in `docs/cookbook.md`.
- **`docs/cookbook.md` opens with the conformance pack** — layering rules,
  banned-dependency rules, allowed-direction rules — because that section works
  with no SCIP index and is what a first-run user can actually use.
- `codeintel schema` output is under 1500 tokens, **measured, with the rule list
  complete**. If it does not fit, the stdlib is too big; cut rules, not the
  catalogue.
- **`schema` prints observed counts, generated from the manifest**, not a static
  closed list: `Kind function 4,201 · class 812 · method 0 …`. A kind at zero is
  a kind the agent must not query. A static list advertising sixteen kinds when
  the index holds five is the onboarding text violating invariant 5.
- **`hint` is non-null whenever `status != "ok"` OR the result is empty**, naming
  the first body literal that matched nothing. As specified, `hint` is
  guaranteed null in exactly the failure mode the taxonomy exists to
  disambiguate.
- **The agent test, full version.** The M1 eval re-run against real extracted
  facts, plus 15 *new* held-out questions in a repo none of the fixtures come
  from. >= 24/30 first-attempt on both sets.
- A no-SCIP run of the eval, recorded separately, in the README.
- Surface-budget test is in CI.

---

## M5 — Languages

**Three more, not eight.** Rust lands at M2/M3. Adding a language is one
authored `tags.scm`, one authored `imports.scm`, one `lang.rs` row, one fixture
and a golden file — call it ~1,000 LOC and a day of fixture work, not a table
row. The prior estimate assumed vendoring, and vendoring does not work.

### M5a — Go, Python

| Language | Grammar | SCIP indexer | Notes |
|---|---|---|---|
| Go | `tree-sitter-go` 0.25.0 | `scip-go` | upstream emits `@definition.type` and five bare `@name` captures with no tag at all |
| Python | `tree-sitter-python` 0.25.0 | `scip-python` | upstream has **no** `@definition.method`; `scip-python` has had no human commit on its default branch since 2025-09-05 |

Python's tier B is on notice. `scip-python` has three open correctness bugs, one
of which silently drops cross-package references. Python ships because its
tier-A story and its `imports.scm` are clean; do not promise `calls_exact`
quality there.

### M5b — TypeScript + TSX

The most expensive of the four and the last. Upstream `tags.scm` is 23 lines and
captures only ambient `.d.ts` forms — `function_signature`, `method_signature`,
`abstract_class_declaration`, `interface_declaration`. Ordinary
`class X {}` / `function f() {}` / `const f = () => {}` produce **nothing**, and
there is no `@reference.call` at all. The whole query is ours. Two grammars
(TS and TSX).

**Done when (each language)**
- Golden facts, span exactness, kind fidelity, query liveness, locality,
  incremental equivalence — the full M2 suite.
- Anchor rate >= 95% against its SCIP index.
- Its `imports.scm` produces usable `import/3` rows, and a conformance rule over
  them passes and fails correctly on a fixture.
- A polyglot fixture (Rust + Go + Python + TS in one tree) indexes and queries
  across language boundaries.
- **Go methods resolve to their type**, not to the file — the case where lexical
  and semantic containment visibly disagree.

### Dropped

**Ruby is not shipping.** `tree-sitter-ruby` has no import node at all — `require`
is an ordinary method call, and idiomatic Rails autoloads with no `require` at
any point. Its `@reference.call` pattern needs `(#is-not? local)`, i.e. a third
`locals.scm` and scope tracking. It fails at both tiers and at the headline
capability.

**C, C++, Java are deferred, not rejected.** C and C++ have `preproc_include`
and zero `@reference.call` — import-complete and call-empty, which is precisely
the profile where conformance is the whole product and no polyglot incumbent
exists. They are the strongest candidates for the next language after M5b, and
they should be added for conformance alone, with `calls` declared unsupported in
`status` rather than silently empty.

---

## M6 — Performance and incremental

Only now, with correctness fixed and something to measure.

1. Benchmark corpus: 3 pinned public repos, one per size decade (~10k, ~100k,
   ~1M LOC).
2. A **test-only** bench harness (`cargo bench`) reporting cold index, warm
   load, single-file reindex, and query p50/p95 over a fixed query set. Not a
   CLI verb.
3. Profile and fix what the numbers say — **not what seems slow.**

**Done when** (on the ~100k-LOC corpus entry, which is what the success criteria
are stated against — the 1M-symbol table in [01-facts.md](../specs/01-facts.md)
§ Scale is a sizing note, not a latency promise)
- Cold tier-A index < 30 s.
- Warm load < 200 ms. If it is not, *then* build the merged-array cache in
  [04-storage.md](../specs/04-storage.md) § Loading, and not before.
- Single-file reindex < 1 s.
- Query p50 < 20 ms, p95 < 100 ms — **in the MCP warm process.** The CLI pays a
  process start plus a warm load per invocation and cannot meet a 20 ms median;
  state the two numbers separately rather than implying one.
- Cold-page-cache numbers recorded alongside warm ones. The agent's first query
  after a checkout is the one that matters and it is 10-50x the warm cost.
- Baselines committed as JSON; CI fails on a > 20% regression.

---

## Explicitly deferred

Recorded so a future agent knows these were considered, not overlooked.

| Deferred | Revisit when |
|---|---|
| Merged-array load cache | M6 measures warm load > 200 ms |
| Caching the derived `ref` relation | a profile shows materializing it dominates query time |
| `--run-indexers` | users report that printing the command is not enough |
| Watch mode | single-file reindex measures > 1 s |
| C, C++, Java | M5 is green and conformance has a user |
| `query --facts FILE` (ephemeral injection) | a real consumer — coverage, a stack trace — asks for it |
| Multi-root / cross-repo indexes | someone has the problem. `manifest.roots` is an array from M2 so this stays cheap |
| Depth-bounded traversals `impact_of_d/3` | truncation on a traversal proves misleading in practice |
| Query result caching | a profile shows repeated identical queries |
| Publishing `crates/datalog` standalone | M1 is green and the seam test holds |

## Explicitly never

From [00-overview.md](../specs/00-overview.md) § Scope.

Embeddings. Vector search. LLM calls. Summarization. Text search as a lane.
Ranking or centrality scores of any kind. Git-history mining. Agent memory.
Context packing. An HTTP API.
