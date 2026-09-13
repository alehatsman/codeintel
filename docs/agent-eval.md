# Agent eval — M1

The load-bearing bet of this project is that an agent can write correct Datalog
over this schema. If that is false, the schema or the surface is wrong and every
milestone after M1 is built on top of the mistake — which is why this runs here,
against hand-written facts, and not at M4 against extractor output.

## Result

**88/90 first-attempt correct. 30/30 questions passed at least two of three
samples.** The gate is ≥ 24/30; the worst single round was 28/30.

| Round | Score |
|---|---:|
| 1 | 30/30 |
| 2 | 28/30 |
| 3 | 30/30 |

## Method

- **Fixture:** `tests/fixtures/eval/facts.dl` — a hand-written fact set for a
  small polyglot repo: 8 files, 23 definitions, Rust with SCIP identity, Go and
  TypeScript with tree-sitter only. Mixed provenance, one ambiguous name, two
  dead exports, an extern package, a trait with two implementors, and tests that
  call the code under test.
- **Rules:** the shipped `rules/stdlib.dl`, unmodified.
- **What the agent saw:** `docs/eval-schema.md` and nothing else — 968 words, a
  hand-written stand-in for `codeintel schema` until that verb exists at M4.
  Each agent was instructed to read that one file and use no other tool, so no
  agent could see the fixture, the expected answers, or this document.
- **Questions:** 30, in `tests/fixtures/eval/questions.tsv`, written before the
  schema page was written. Three samples each, in three rounds, with the
  questions re-partitioned between rounds so no agent context held the same
  grouping twice. Every agent was fresh: 30 agents, 3 questions each.
- **Scoring:** `tests/fixtures/eval/score.py`. An answer is correct when the
  **set of values in the question's answer variables** matches the reference
  query's. Extra join columns are fine; extra or missing rows are not. Scoring
  whole rows would score style rather than truth, because different correct
  queries expose different join variables.
- **Answers as submitted:** `tests/fixtures/eval/runs/round{1,2,3}.tsv`.

## The two failures, verbatim

> **Vocabulary note, schema 2.** The transcripts below are reproduced exactly as
> the agents typed them, and two of them call `callers/2`, which no longer
> exists — it was `calls/2` with the variables renamed and was cut to pay for
> `exported` and `implements` becoming rules. Read `callers(A, S)` as
> `calls(A, S)`. The eval is **not** re-run here: editing a recorded answer to
> match a later vocabulary would make the measurement say something it did not
> say.


**Q5, round 2** — *Who calls the symbol `"scip demo store/Store#get()."`?*

```prolog
?- def(S, _, _, "scip demo store/Store#get()."), callers(A, S).
```

The symbol id was passed in `def`'s **fourth** argument, which is `Name`. The
schema documents `def(S, F, Kind, Name)` and the question quoted a symbol id, so
this is a positional mix-up rather than a missing concept. The same agent's
other two answers were correct, and the other two samples of Q5 were correct —
one of them `?- callers(A, "scip demo store/Store#get().").`, which is the
minimal form.

**Q14, round 2** — *What contains `"scip demo store/Store#get()."`, directly or
transitively, up to the file?*

```prolog
?- within(A, "scip demo store/Store#get().").
```

Arguments reversed. This one is **the schema's fault, not the agent's**: the
page documents

```
within(C, A)                        containment closure, up to the file
```

and never says which position is the child. `parent(Child, Parent)` names its
arguments; `within` does not. An agent reading only that line has a coin flip,
and one of three samples lost it.

## What was changed, and what was not

`within`'s line in the schema now reads `within(Child, Ancestor)`. That is a
copy fix to an ambiguity the transcript identified, and **it is unmeasured** —
this run's 88/90 was scored against the page as it stood. M4 re-runs the eval
against real extracted facts plus 15 held-out questions, which is where the fix
gets tested.

Nothing else was changed. In particular the `def` argument order was left alone:
one sample in ninety mis-positioned an argument that the page spells out and the
other eighty-nine did not, which is an agent slip and not a schema defect. The
plan warns against the confirmation-bias trap of revising the copy after every
failure until the number comes up; two failures out of ninety, one of them with
an identified cause in the copy, is not evidence that the schema is wrong.

## Caveats worth stating

- **Same model family as the author.** Every sample came from the model writing
  this repo. That is the population the tool is built for, and it is also the
  population most likely to share its blind spots.
- **The no-other-tool rule was an instruction, not a sandbox.** Agents had file
  access and were told to read one path. Nothing in the transcripts suggests
  otherwise, but it was not enforced.
- **The fixture is small.** 23 definitions. It exercises the schema, not scale;
  M6 is where scale gets measured.
- **The questions name symbol ids verbatim.** A real agent would have to find
  them first — usually with `innermost_at` or a `def` lookup, which Q21 does
  cover end to end.

---

# Agent eval — M4

The M1 eval ran against hand-written facts and a hand-written schema page. This
one runs against **real extracted facts** and the **generated** `codeintel
schema`, which is the thing M4 built. Two sets, because a re-run alone would
only show that nothing regressed.

## Result

**Set A's total is withdrawn. See § Rescored, 2026-09-13.** The table below
is kept as it was measured.

| Set | Round 1 | Round 2 | Round 3 | Total | Gate |
|---|---:|---:|---:|---:|---|
| **A** — the M1 30, retargeted to real facts | 29/30 | 28/30 | 28/30 | **85/90** | >= 24/30 ✅ |
| **B** — 15 held-out, on a repo no fixture comes from | 15/15 | 15/15 | 15/15 | **45/45** | >= 12/15 ✅ |

Set A is `tests/fixtures/rust/` with its committed `index.scip`, so SCIP symbol
ids and both provenances are in play. Set B is **this repository pinned at
`8471824`**, tier A only — which doubles as the no-SCIP run: 45/45 with no SCIP
index at all.

**These numbers are a re-run, and the first ones are withdrawn.** An earlier
pass scored 88/90 and 43/45 against a schema that no longer exists: the eval
found `long_def`'s copy defect, the fix changed `long_def`'s signature, and
scoring old transcripts against the new standard library measures nothing about
the agents. Everything below was produced by fresh agents reading the shipped
schema.

## Rescored, 2026-09-13 — set A withdrawn

`crates/codeintel/tests/eval_rot.rs` (#17) now recomputes set A's reference
answers and rescores the recorded runs on every commit. On its first run, at
`0574a1b`, the table above did not reproduce.

| Set | Round 1 | Round 2 | Round 3 | Total |
|---|---:|---:|---:|---:|
| **A**, recorded runs against current references | 28/30 | 27/30 | 26/30 | 81/90 |
| **B**, recorded runs against `8471824`, rescored with `score.py` | 15/15 | 15/15 | 15/15 | **45/45** |

**Set A's 85/90 is withdrawn, and 81/90 does not replace it.** Four of the new
failures are not agents answering wrongly. Q5 in all three rounds calls
`callers/2`, and round 3's Q10 calls `defines/2`, and schema 2 (`9e5dd5e`)
removed both. This page's rule is that a signature change invalidates a run,
because the agent could not have written the query that now exists. Schema 2 was
that change, and nobody applied the rule when it landed. So 81/90 is what old
transcripts earn against a new vocabulary, not a score. **A fresh set-A run is
owed.** The other failures are the recorded ones: Q13 in every round and Q28 in
rounds 2 and 3.

**Set B stands.** Its tree is pinned. Its one moved reference answer, Q8 at 16
→ 23 rows, moved together with the recorded queries, and all three rounds still
match it.

**Fourteen set-A reference answers moved.** The reason is the facts, not the
questions:

- **Q5, Q21, Q25 — checked by hand.** `Store::get` no longer calls itself. Its
  body calls `HashMap::get`, and tier A's name matching resolved that to
  `Store::get` until `ad49523` deferred to the compiler. The old reference
  answer recorded a false edge.
- **Q23.** `src/app.rs` no longer "depends on" `tests/store_test.rs`. That is
  the crate-root-module artifact M4 documented and `cookbook.md` § 4 warned
  about.
- **Q10, Q20, Q30.** `crate/` is gone, and three `local src/kinds.rs N`
  definitions appear. This fits `5ca3f68` refusing a symbol that SCIP defines in
  two documents.
- **Q11, Q12, Q22.** Enum variants and trait items are now exported (#13).
- **Q16, Q26, Q27, Q28.** A trait-impl method is qualified by its trait
  (`5ca3f68`), so `fmt` and `f` become distinct definitions and the per-file
  counts move.

Only the first bullet was traced end to end. The others are attributed to the
commit whose stated behaviour matches the diff. The regenerated `expectedA.tsv`
is `score.py expected A`'s output, and `eval_rot.rs` reproduces it independently
through the library. The two agree.


Fresh agent per group, three questions each, questions re-partitioned between
rounds so no context saw the same grouping twice. Each agent read one file — the
generated `schema` output — and was told to use no other tool. Scoring is
`tests/fixtures/eval-m4/score.py`, which runs the real binary with `--raw` and
compares the **set of values in the question's answer variables**. Answers as
submitted are in `runs/`.

**Set B is pinned, and this is not optional.** Its reference answers are facts
about this repository's own source, so *any* commit invalidates them — including
the commit that fixes something the eval found. `setB.commit` records the sha;
reproduce with `git archive <sha> | tar -x -C <dir>`, index that directory, and
score against it. Scoring set B against a live working tree silently measures a
moving target, which is how a 45/45 and a 42/45 can both be "true" on the same
afternoon.

**If you reproduce from a copied tree, force the rebuild.** `rsync -a` and
`cp -a` preserve mtimes, so cargo sees a binary newer than its sources, prints
`Finished in 0.19s`, and you score with whatever build the copy carried. The
symptom is a plausible score from the wrong code. `touch` the sources — or copy
without `-a` — and confirm you saw a real compile before trusting a number.

**Two of set B's questions were replaced before this run.** Q11 and Q13 had
**empty** reference answers — `depends` on a file with no resolvable references,
and a module with no children — so they were passable by any query returning
nothing. They were rewritten against targets that have answers. Every question
in the table above was answered by a fresh agent against the current schema; no
answer in `runs/` was authored or edited by hand. The scorer now refuses to run
if any reference answer is empty outside a named list, so this class of hole
fails loudly instead of being found by accident.

## The failures

**Set A, Q13 — all three rounds.** *Which fields belong to the type named
"Store"?*

```prolog
?- def(T, _, "type", "Store"), parent(A, T), def(A, _, "field", _).
```

`Store` is a `struct`, not a `type`, and `schema` lists both kinds with their
counts from this very index. Three independent agents made the same choice, so
this is worth treating as copy rather than carelessness: `type` is both a Kind
value *and* the English word for what `Store` is. The form that works leaves
Kind unbound — `def(P, _, _, "Store")` — and no rule advertises that as the
robust shape.

**Set A, Q28 — rounds 2 and 3.** *How many definitions does each file hold?*

```prolog
?- file(A, _), B = count{S : local_def(A, S)}.
```

`local_def(F, N)` is `(file, **name**)`, so this counts distinct names rather
than definitions — off by the number of same-named definitions in a file. The
`%%` line reads "S is defined in F. Helper.", which does not say which of the
two arguments is the name.

**`local_def`'s doc line has since been corrected**, and the number above has
not. It read "S is defined in F" for `local_def(F, N)` — naming a variable
absent from its own signature and calling a name a symbol — which is a plainly
false line and was fixed as a correctness matter. That makes this page carry a
milder version of the property it withdrew the first pass for: the copy an agent
would read today is not quite the copy these agents read, and the difference is
the line behind Q28.

The score stands as measured. Whether it would now be 87/90 is **untested, on
purpose**: re-running after every copy change is how a score stops measuring the
tool and starts measuring how many times you re-rolled. The rule for this page
is that a *signature* change invalidates a run — the agent could not have
written the query that now exists — while a corrected doc line does not, because
every recorded query is still legal and still scores identically. `long_def`
crossed that line and forced a re-run; `local_def` did not.

**Both failures are a signature reading as something it is not**, which is the
same shape as the `long_def` defect this eval found on its first pass. That one
was fixed — `def_lines(S, N)` now states a length with no threshold, and all
three rounds used it correctly for the question that used to fail. The score
went *down* from 88/90 to 85/90 because the questions got harder to pass by
accident, not because the tool got worse.

### Q14's reference answer was wrong, and the agents were right

Set A's Q14 — "what contains `store/impl#[Store]get().`, directly or
transitively, **up to the file**?" — recorded `local src/store.rs Store#` as the
answer. That symbol is the `impl` block's stale tier-A id, which after anchoring
has no `def` row, and the recorded answer never reached the file the question
explicitly asks for. The reference answer was wrong when it was written; the
scorer compared a wrong answer against a wrong expectation and called it a pass.

All three set-A rounds wrote the identical, correct query. So fixing the
extractor (`specs/02-extraction.md` § Parent precedence, rule 3's existence
check) changed only what that correct query *returns*: now `src/store.rs`.
**85/90 and 45/45 both stand** — recomputing every set-A reference answer from
its query moved exactly one line, Q14's, and rescoring all six recorded rounds
reproduced both totals unchanged. Set B is untouched because it runs tier A only,
where the `impl` block and its type share a symbol and the parent resolves.

This is the third reference-answer defect this eval has found in itself, after
the ten empty answers and the moving set-B target. The pattern is consistent: the
eval is better at finding bugs in the eval than the eval is at finding bugs in
the tool, and every one of them was invisible until something forced a
recomputation. A reference answer nobody recomputes is an assertion nobody
checks.

## What building the eval found

Two things, before a single agent ran.

**Ten of thirty expected answers came back empty**, because the retargeted
questions quoted symbol ids I had written by hand and got wrong — the real one is
`store/impl#[Store]get().`, not `store/Store#get().`. A question whose correct
answer is "no rows" is passed by any query that returns nothing, so a third of
set A was scoring itself as correct for free. Now two are empty, both the known
`implements`/`extern` coverage gap.

**A real defect, in tier B rather than M4.** `parent(S)` for a method inside an
`impl` block points at `local src/store.rs Store#`, which has **no `def` row**.
Every other parent edge in the index carries a resolved SCIP id. So `within/2`
and `about(S, "parent", ...)` join to nothing for any method in an `impl`, and
`tests/stdlib.rs`'s containment test sailed past it because it used module
nesting. Recorded in [plan.md](plan.md); not fixed inside M4, because parent
precedence is tier-B ingest and has blast radius.
