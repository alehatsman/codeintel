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
