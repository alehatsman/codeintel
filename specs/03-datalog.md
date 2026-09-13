---
id: datalog
status: proposed
binding: no
---
# 03 — Datalog: language and engine

`crates/datalog` is a generic Datalog engine over interned `u32` tuples. It has
**no knowledge of code, symbols, files, or SCIP**. That seam is enforced: the
crate's `Cargo.toml` must not depend on `facts` or `extract`.

Target: ~1200 lines, zero dependencies. See
[research.md](../docs/research.md) §4 for why we build rather than vendor.

---

## Language

Prolog/Souffle-flavoured, chosen because it is the Datalog dialect with the most
public documentation and therefore the one a language model is most likely to
write correctly without examples.

### Grammar

```
program    := (rule | fact | query)*
fact       := atom "."
rule       := atom ":-" body "."
query      := "?-" body "."
body       := literal ("," literal)*
literal    := atom | "!" atom | comparison | assignment
atom       := ident "(" term ("," term)* ")"
comparison := term op term                 op := = | != | < | <= | > | >=
assignment := var "=" expr
expr       := term | term arith term | aggregate
arith      := + | - | * | /
aggregate  := "count" "{" term ":" body "}"
generator  := "between" "(" term "," term "," var ")"
term       := var | string | int | "_"
var        := upper (alnum | "_")*
string     := '"' ... '"'                  escapes: \" \\ \n \t
int        := "-"? digit+
comment    := "%" ... eol
```

Whitespace insensitive. `%` comments. No modules, no functors, no lists, no
arithmetic on strings, no cut. Deliberately small.

### Example

```prolog
% rules
hot(S, N) :- def(S, _, "function", _), N = count{ C : calls(C, S) }, N > 10.

% query
?- hot(S, N), def(S, F, _, Name), at(S, F, L).
```

### Builtins

| Builtin | Meaning |
|---|---|
| `X = Y`, `X != Y` | atom identity. `"42"` and `42` are different atoms ([01-facts.md](01-facts.md) § Integers). |
| `X < Y`, `<=`, `>`, `>=` | **integers only**, checked at runtime: both operands must fall in the integer id range ([01-facts.md](01-facts.md) § Integers), else `invalid-query`. Comparing string atoms is an error, not a byte comparison — string atom ids are allocation-ordered, so comparing them would give results that change between runs. |
| `X = A + B` (`-`, `*`, `/`) | integer arithmetic. Division by zero → error. Overflow → error. |
| `between(Lo, Hi, X)` | **generator.** `Lo` and `Hi` must be bound integers; binds `X` to each integer in `Lo..=Hi` inclusive, ascending. If `X` is already bound it degrades to a range check. This is the only builtin that produces bindings rather than filtering them, and it is what makes `symbol_at` range-restricted without flow-sensitive safety analysis ([01-facts.md](01-facts.md) § The location bridge). `Lo > Hi` yields nothing. `X` must be a variable, per the grammar above: `between(Lo, Hi, _)` is `invalid-query`, not a silently empty generator. |
| `match(S, "re")` | regex over the string behind atom `S`. Second argument must be a literal. |
| `prefix(S, "p")`, `suffix(S, "s")`, `contains(S, "c")` | cheaper string tests; prefer these over `match` where they suffice. |
| `count{ X : goal }` | number of distinct bindings of `X` satisfying `goal` |
| ~~`sum` / `min` / `max`~~ | **deferred.** No `stdlib.dl` rule uses them, and each is an overflow, negative-value and determinism surface in a hand-written engine. Add when a cookbook entry needs one. |

Regex flavour: **the `regex` crate**, injected by the host as a builtin so
`crates/datalog` keeps its zero-dependency property. An earlier draft specified
a hand-rolled backtracking matcher to avoid the dependency. It avoids nothing —
`regex` is already a non-optional transitive dependency of `tree-sitter` itself
— and a backtracker gives up the linear-time guarantee, so `max_regex_steps`
would abort on patterns `regex` runs in microseconds. Inline flags such as
`(?i)` work, which the hand-rolled flavour did not, while the `schema` output
taught `match(N, "(?i)auth")` anyway.

The superseded flavour was: a minimal backtracking matcher supporting `^ $ . * + ? [] | ()`
and escapes. No backreferences, no lookaround. Compiled once per query, with a
step budget. We do not take a regex dependency for this.

### Safety rules — checked before evaluation, reported as errors

1. **Range restriction.** Every variable in a rule head appears in a positive
   body literal of that rule. A `_` in a rule head is `invalid-query` for the
   same reason: the column has no value to emit, so the rule derives nothing at
   all rather than deriving rows with a hole. `_` stays legal in a body literal
   and in a query head position, where it means "any value, do not bind it".
2. **Negation safety.** Every variable inside `!atom(...)` appears in a positive
   literal earlier in the same body.
3. **Comparison safety.** Both sides of a comparison are bound by an earlier
   positive literal, or are literals.
4. **Aggregate safety.** Every variable free in the aggregate's goal but used
   outside it is bound outside it. "Outside" is every enclosing body, not just
   the nearest: in `M = count{ Y : e(Y), K = count{ Z : g(Z) }, K > 3 }, h(Z)`
   the inner `Z` is used by `h(Z)` two levels out and nothing binds it first.
   Checking only the nearest body accepted that query while the planner, which
   does see nested variables, scheduled `h(Z)` first and grouped the inner
   count by `Z` — a different answer from the one written, and the naive twin
   shares the planner, so the differential test agreed with it.
5. **Stratification.** See below.
6. **Arity consistency.** A relation's arity is fixed by its first use; a later
   use with different arity is an error naming both sites.
7. **Binding by assignment and by generator.** For range restriction (rule 1)
   and negation safety (rule 2), a variable counts as bound if it appears in a
   positive literal, **or** is the target of an assignment `X = expr` whose
   every variable is already bound, **or** is the output of a generator builtin
   whose inputs are already bound (`between/3`). This is what makes
   `def_lines(S, N) :- def_span(S,A,B,_,_), N = B - A.` legal, and
   `symbol_at` with it.

   **A literal naming a predicate nothing defines is `invalid-query`.** Not a
   safety rule — an existence one, checked after the seven so that a more
   specific complaint wins. A predicate must be a base relation or a head
   somewhere in the same program; forward references stay legal because heads
   are registered first. Without it an undeclared predicate evaluates as an
   empty derived relation, so `?- nosuchrelation(X).` answers `ok` with zero
   rows — indistinguishable from a correct query about code that has none of
   the thing asked for, which is invariant 6's failure exactly.

   Safety is checked on the program **as written**, before any demand
   transformation. A rule that is safe only after the transformation would be
   un-evaluable by the naive evaluator, and the naive evaluator is the M1
   acceptance gate — so the transformation could no longer be differentially
   tested against anything. Legality and performance stay separate concerns:
   demand transformation makes safe rules fast, never unsafe rules legal.
8. **Base/derived exclusivity.** A relation is either supplied by the fact store
   or defined by rules, never both. Writing a rule whose head is a base relation
   is an error naming the relation. This keeps "where did this tuple come from"
   answerable — the reason `ref` was made purely derived and the extractors
   write `scip_ref` / `name_ref` instead ([01-facts.md](01-facts.md)).

Every violation returns `status: "invalid-query"` with the offending rule, the
variable, and which rule was violated. An agent must be able to fix its query
from the error text alone without reading this spec.

### Stratification

Negation and aggregation are **stratified**: build a dependency graph over
relations where an edge is labelled negative if the dependency is through `!` or
an aggregate. If any cycle contains a negative edge, reject with
`status: "unstratified"` naming the cycle. Otherwise evaluate strata in
topological order; within a stratum, semi-naive to fixpoint.

This rejects `p(X) :- q(X), !p(X).` and accepts everything in `stdlib.dl`.

---

## Engine

### Representation

```rust
type Atom = u32;

/// A relation is a set of fixed-arity tuples, kept sorted and deduplicated.
struct Relation {
    arity: u8,
    data:  Vec<Atom>,   // row-major, len == arity * rows
    index: per-column row order, built lazily, dropped on write
}
```

Flat `Vec<Atom>` rather than `Vec<[Atom; N]>`: arity is runtime-determined, and
a flat buffer keeps one code path instead of a macro-generated family. Row `i`
is `data[i*arity .. (i+1)*arity]`.

Sorted + deduplicated is the invariant every operation preserves. It gives
sort-merge joins, binary-search lookup, and free set semantics.

The sort order serves a bound *prefix*. A lookup that binds a later column with
the prefix free — `def(S, _, _, N)` with `N` bound, `!scip_ref(_, F, L, C, _, _)`
— has nothing to binary-search, and a scan per outer row is what made
`ambiguous` cost 650 ms on a 5,000-definition repository (issue #3). So a
relation also carries a **secondary index per column**: the row ids ordered by
that column, ties in row order. Built the first time a lookup asks for that
column, kept for the relation's lifetime, dropped by any write. Nothing is
precomputed for columns no query binds, and a derived relation that is settled
every fixpoint round pays only for the columns its rules actually look up.

### Demand transformation (magic sets)

**Required for v1. Without it the product's most valuable query is unusable.**

Bottom-up evaluation ignores the query's bindings. Given

```prolog
impact_of(Seed, C) :- calls(C, Seed).
impact_of(Seed, C) :- impact_of(Seed, B), calls(C, B).
?- impact_of("Store::get", C).
```

naive bottom-up computes `impact_of` for **every** seed — the full all-pairs
transitive closure of the call graph — and then filters to one. On a 1M-symbol
repo that is on the order of 10^10 tuples: `max_derived_tuples` fires, and the
agent gets `budget-exceeded` on the one question it most wanted answered.

The fix is the standard magic-set / demand transformation (Bancilhon et al.;
Beeri–Ramakrishnan). Before evaluation, for each recursive predicate whose query
binds some argument positions, synthesize a `magic_` seed relation and guard the
recursive rules with it, so evaluation grows outward from the seed:

```prolog
magic_impact_of("Store::get").
magic_impact_of(B)  :- magic_impact_of(S), impact_of(S, B).
impact_of(S, C)     :- magic_impact_of(S), calls(C, S).
impact_of(S, C)     :- magic_impact_of(S), impact_of(S, B), calls(C, B).
```

Now the work is proportional to the reachable subgraph, not the whole graph.

Scope for v1:

- Applies to **any binding available at a call site** — a constant in the goal,
  or a variable bound by an earlier body literal — propagated through both
  recursive **and non-recursive** predicates. Adornment is on bound/free
  argument positions.
- **Non-recursive predicates are the majority of the win and are not optional.**
  `ref`, `calls`, `symbol_at`, `is_test`, `about`, `ambiguous` and `depends` are
  all non-recursive. An earlier draft excluded them, which left `symbol_at` not
  range-restricted — so the standard library did not load, and the README's own
  headline query was rejected by the engine shipped beside it.
- The headline pattern binds the seed from an **earlier body literal**, not from
  a goal constant:
  `?- innermost_at("src/store.rs", 142, S), impact_of(S, C).`
  Sideways information passing handles this; a transformation scoped to goal
  constants does not. Test this shape explicitly, not just the constant-goal one.
- Not applied to fully-free goals, where there is nothing to propagate.
- **Adornment walks each body most-bound-first, not as written.** The planner
  reorders literals at run time, but adornment is fixed before that, so a
  binder placed *after* the derived literal it binds would otherwise forfeit
  the rewrite: `callers(C, S), def(S, _, _, "evaluate")` and its reverse must
  rewrite identically. The walk mirrors the planner's heuristic without
  relation sizes — a literal sharing no bound variable goes after one that
  shares one, a constant or an already-bound variable counts as bound,
  filters run as soon as their inputs exist, ties keep written order. Safety
  is still checked on the program as written (rule 7).
- **`stats.demand` says why `stats.transformed` holds what it holds**:
  `applied`, or the reason the program ran as written — nothing to seed, the
  rewrite failed the safety check, did not stratify with nothing left to back
  off, backed off too many times, or exceeded the planning limits. An empty
  `transformed` alone conflated the first with the rest, and the rest are the
  ones that surface as an unexplained timeout (invariant 5).
**`stats.depends` names the base relations the goal's dependency closure
reaches**, taken from the plain program rather than the demand rewrite, so the
names are relations and not adornments. The engine has no opinion about what
that means; it is what lets the layer above distinguish "your code does not do
this" from "this index could not see it" without parsing the query text
([05-surface.md](05-surface.md) § Status, `no-scip`).

- The transformation is reported in `stats.transformed`, so an unexpectedly slow
  query can be diagnosed as "demand transformation did not apply here" rather
  than guessed at.

`stdlib.dl` is written to cooperate: every traversal ships in a **seeded** form
whose first argument is the seed (`impact_of`, `reach_of`), with the unseeded
whole-graph forms (`reaches`, `recursive`) kept separate and documented as
expensive. A rule written so the seed cannot propagate is a rule that will be
slow, and that is a property of the rule, not a bug in the engine.

**Cheaper fallback, if M1 runs long:** a `reach(EdgeRel, Seed, Out, MaxDepth)`
builtin doing a seeded BFS over one relation — ~50 lines, covers most real
traversals, and honestly a special case rather than general evaluation. Ship it
only as a stopgap with an issue open for the real transformation; a query
language whose recursion is a builtin is a query language with an asterisk.

#### Negated literals, and backing off

A negated literal whose arguments are **all bound** is a ground membership test,
and a demanded relation answers it exactly: its seed is that one tuple, so it
holds the tuple iff the full relation does. Seeding it matters —
`innermost_at` is `symbol_at(F, Line, S), !tighter_at(F, Line, S)`, and without
it `tighter_at` is materialized over every line of every file.

Seeding a negated predicate also adds a magic rule whose body is the *call
site*. Magic sets always place a predicate, its magic relation and its callers
in one strongly connected component; when the edge into that component is
negative, the component is unstratifiable. That is exactly what happens to
`!ambiguous(N)` inside `ref` once the query also reaches `impact_of`, because
`impact_of`'s recursion closes the loop.

So the decision is **per predicate**: rewrite, ask which cycle broke, stop
seeding the predicates named in it, rewrite again — bounded by the number of
seeded negations. `?- innermost_at(F, L, S), impact_of(S, C).` ends up with
`tighter_at` seeded and `ambiguous` not, which is the most demand that query can
carry. An all-or-nothing fallback gets this query wrong in the expensive
direction: measured at M2, whole-program fallback left it running past 120 s on
a 1,070-definition repository, and per-predicate back-off answers it in 168 ms.

A predicate inside an **aggregate** goal is never seeded. Counting reads the
whole relation, so restricting it would change the answer rather than the cost.

### Evaluation

**Only the goal's dependency closure is evaluated.** Before the loop below
runs, the predicates reachable from the query body — through rule bodies,
negations and aggregates alike — are collected, and every stratum outside that
set is dropped. This is not an optimization, it is what makes a shipped rule
library usable: `rules/stdlib.dl` contains `reaches/2`, an unseeded all-pairs
transitive closure, so evaluating every stratum would make
`?- def(S, F, "function", N).` — a single scan of a base relation — cost
all-pairs reachability over the whole repository. Measured at M2 on this repo
(1,070 defs, 4,173 `name_ref`s) that query hit `max_time_ms` at 5,000 ms; with
the closure it answers in single-digit milliseconds. The rows are identical
either way: a relation the goal cannot reach cannot change the goal's answer.

Semi-naive, the standard algorithm, the one `datafrog` implements in ~500 lines:

```
for each stratum in topological order:
    for each relation R in stratum:  delta[R] = R
    loop:
        for each rule in stratum:
            evaluate the body with at least one literal bound to its delta
            new = results - R
        if all new are empty: break
        for each R: R += new[R];  delta[R] = new[R]
```

Joining a rule body: order literals by a two-key cost heuristic, then for each
literal pick the narrowest access path the bindings allow.

The order:

- **Connected first.** Once any variable is bound, a relation literal that
  shares none of the bound variables is a cross product — every row it matches
  multiplies the rest of the join — and it ranks after every literal that
  shares one. The second key cannot see this, because a constant counts as a
  bound argument exactly as a join variable does. `exported`'s recursive body
  tied `visibility(S, "inherited")` with `parent(S, P)` at `11380 >> 1` on
  tokio, took the written one, and paired 1,788 seeds with 2,126 candidates:
  868 ms, against 1 ms connected (#24). `stats.derived` counts derivations,
  not rows visited, so it does not move when this breaks; the plan does.
- **Then most-bound-first.** A literal with `k` bound arguments — constants or
  already-bound variables — costs `|R| / 2^k`. Filters are free and run as
  soon as their inputs exist. Ties keep written order.

The access path:

1. **prefix** — the leading column is bound: binary-search the sorted relation
   on the whole bound prefix.
2. **column** — the leading column is free and some later column is bound: look
   the bound columns up in their secondary indexes and take the one with the
   fewest matching rows. The other bound columns are filtered per row, as
   before.
3. **scan** — nothing is bound.

Candidates are visited in row order on every path, so the same rows come out in
the same order whichever path served them. No query optimizer beyond this, and
none is warranted at our scale. `stats.plan` marks a literal on the column path
as `i#c,d` — literal `i`, its bound non-leading columns — so a slow query shows
which lookups went through an index and which did not; the narrowest of the
listed columns is chosen per lookup, not per plan.

Joins produce tuples into a scratch buffer; sort + dedup once per iteration
rather than per tuple.

### Determinism

Non-negotiable (invariant 8, [00-overview.md](00-overview.md)).

- All relations sorted by column order; iteration follows storage order. A
  lookup through a secondary index yields rows in that same order.
- Query results sorted by the goal's term order before output. **Term order is
  atom order, which is dictionary order — insertion order, not lexicographic.**
  It is total and deterministic for a given index, but a cold index and an
  incrementally-updated one assign different ids to the same string
  ([docs/plan.md](../docs/plan.md) M2, incremental equivalence), so the same
  repo state can print the same rows in a different order. **The surface layer
  therefore sorts rendered rows by their printed text** before output
  ([05-surface.md](05-surface.md)); the engine's atom order is the cheap,
  stable internal one.
- Literal reordering by the cost heuristic is a **pure function of the rule and
  the current relation sizes**, and relation sizes are deterministic, so the
  plan is deterministic.
- No hash-map iteration order ever reaches output. Hash maps are permitted
  internally only where the result is subsequently sorted.

### Limits

Every limit is configurable, has a documented default, and **sets `truncated`
with the name of the cap that fired** when it bites (invariant 7).

| Limit | Default | On breach |
|---|---:|---|
| `max_result_rows` | 1,000 | truncate output, `truncated: true`, `cap: "max_result_rows"` |
| `max_result_bytes` | 262,144 | truncate output at a row boundary, `cap: "max_result_bytes"` |
| `max_derived_tuples` | 10,000,000 | abort, `status: "budget-exceeded"` |
| `max_time_ms` | 5,000 | abort, `status: "timeout"` |
| `max_strata` | 64 | reject at planning |
| `max_body_literals` | 32 | reject at planning |

`max_body_literals` counts the literals inside a body's `count{}` goals too, at
every depth: the solver recurses once per literal whether or not it sits in an
aggregate, so a bound that skipped them bounded nothing. It is checked
**before** the safety rules, on the parsed program; the safety check and the
planner cost more than linear time in body length, and a limit enforced after
them let an 8,000-literal query spend seconds past `max_time_ms` just to be
told it was too long. `max_strata` needs stratification to measure and is
checked after it.

**Aggregate nesting is capped at 32 by the parser, and that cap is not a
`Limits` field.** Parsing, the safety check and demand transformation each
recurse once per `count{}` level, and a query nested ten thousand deep (~150 KB
of text) overflowed the stack — an abort, not a diagnostic, which takes the MCP
process with it. The bound guards the thread's stack, which is a property of the
host rather than of the query, and parsing happens before any `Limits` is in
hand (`load_rules` has none at all). 32 matches `max_body_literals`' default:
each level needs a literal, so a deeper query could not pass planning anyway.
Past it the parser returns `invalid-query` naming the cap.

Both planning limits are checked on the program as written and again on the
demand-rewritten one, which has a guard literal more per body and more strata.
A rewrite over the limit is dropped and the program as written runs: the
rewrite is a performance step, and may not turn a query within budget into a
rejected one.

`max_strata` was 32 in an earlier draft. `rules/stdlib.dl` stratifies into 37,
so that default rejected every query against the shipped standard library: the
limit had been set against an imagined rule set rather than the real one.

**`max_regex_steps` is gone with the hand-rolled matcher.** A step budget is
what a backtracker needs to bound catastrophic patterns; `regex` is linear-time
by construction, so the only bound that means anything is `max_time_ms`, and a
cap the host cannot measure would be a limit in name only.

`max_result_bytes` exists because rows are not uniformly sized and the consumer
is a context window. A SCIP symbol string runs ~68 characters ≈ 17 tokens; 1,000
rows with two symbol columns is ~34,000 tokens of output reported as
`status: ok`. A row cap alone does not bound that. Both caps are checked, and
whichever fires first is the one named in `cap`.

A **ground** goal — `?- calls("a", "b").` — has no variables and therefore no
columns. Truth is one empty row and falsehood is no rows; without the
distinction an answered "yes" would be indistinguishable from "no".

A truncated result is still sorted, so the first N rows are a stable prefix, not
an arbitrary sample. This matters: an agent paging through results must get the
same prefix each time.

### Public API

```rust
pub struct Engine { /* relations, dictionary, rules */ }

impl Engine {
    // Not `load(facts: &FactStore)`: that signature is the seam this crate
    // exists to keep. The host owns the store and the dictionary and installs
    // both, so `crates/datalog` never learns what a symbol is.
    pub fn new(syms: Box<dyn Symbols>) -> Engine;
    pub fn with_regexes(self, regexes: Box<dyn Regexes>) -> Engine;
    pub fn insert_relation(&mut self, name: impl Into<String>, relation: Relation);

    pub fn load_rules(&mut self, src: &str) -> Result<(), Diagnostic>;
    // `&mut self`, not `&self`: a query's string literals are interned, and a
    // query may legitimately name a string the corpus does not hold.
    pub fn query(&mut self, src: &str, limits: &Limits) -> Result<QueryResult, Diagnostic>;
}

pub struct QueryResult {
    pub columns:   Vec<String>,   // variable names from the goal, in order
    pub rows:      Vec<Vec<Atom>>,
    pub truncated: bool,
    pub cap:       Option<&'static str>,
    pub stats:     Stats,         // derived tuples, elapsed, strata, plan,
                                  // transformed, shadowed, depends
}

pub struct Diagnostic {
    pub status:  Status,          // invalid-query | unstratified | timeout | budget-exceeded
    pub message: String,          // actionable, names the rule and variable
    pub span:    Option<(usize, usize)>, // byte range in the submitted source
}
```

`load_rules` is separate from `query` so `stdlib.dl` is parsed and stratified
once at startup and a user query is checked against an already-built rule set.
A user query may define rules; those are layered on top for that call only and
may shadow stdlib rules — shadowing is reported in `stats`, never silent.

---

## Testing

1. **Engine conformance, code-free.** A suite of synthetic programs
   (transitive closure, same-generation, stratified negation, aggregates,
   arithmetic) with hand-computed expected results. `crates/datalog` must pass
   these with no code-intel facts in sight — that is how the seam stays honest.
2. **Safety rejection.** Every rule in § Safety rules has a test asserting the
   specific diagnostic, including its message text.
3. **Determinism.** Each query runs 100× with shuffled internal capacities;
   output must be byte-identical.
4. **Limits.** Each limit has a program that provokes it and asserts the exact
   `status` / `cap`.
5. **Differential.** A naive (non-semi-naive) evaluator ships in the engine,
   public and doc-hidden, never used to answer a question. Every conformance
   program is evaluated by both and the results compared, and the host runs
   `stdlib.dl` through the same comparison from its own crate, so the engine's
   tests read no rule library. This is the only practical defence against a subtle
   semi-naive bug, which otherwise manifests as quietly missing rows. The
   naive side runs the program **as written**, with no demand transformation:
   a twin that shared the rewrite would agree with a rewrite that drops a
   seed. The stdlib's seeded traversals, and every combination of aggregate,
   negation, recursion and demand, go through the same comparison.
6. **stdlib.dl.** Every rule in `rules/stdlib.dl` has a fixture-based test with
   a hand-verified expected answer.
7. **Fuzzing.** The parser is fuzzed for panics. Parsing untrusted input must
   produce a `Diagnostic`, never an abort.
8. **Demand transformation.** For each seeded stdlib traversal, assert that a
   constant-bound goal derives **strictly fewer** tuples than the same goal run
   with the transformation disabled, and that both produce identical results.
   Equal counts mean the transformation silently failed to apply, which is the
   failure mode that only shows up as an unexplained timeout on a big repo.
