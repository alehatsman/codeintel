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
aggregate  := ("count"|"sum"|"min"|"max") "{" term ":" body "}"
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
| `X = Y`, `X != Y` | atom identity. `"42"` and `42` are different atoms, and **opaque atoms are rejected** ([01-facts.md](01-facts.md) § Integers). |
| `X < Y`, `<=`, `>`, `>=` | **integers only**, checked at runtime: both operands must fall in the integer id range ([01-facts.md](01-facts.md) § Integers), else `invalid-query`. Comparing string atoms is an error, not a byte comparison — string atom ids are allocation-ordered, so comparing them would give results that change between runs. |
| `X = A + B` (`-`, `*`, `/`) | integer arithmetic. Division by zero → error. Overflow → error. |
| `match(S, "re")` | regex over the string behind atom `S`. Second argument must be a literal. |
| `prefix(S, "p")`, `suffix(S, "s")`, `contains(S, "c")` | cheaper string tests; prefer these over `match` where they suffice. |
| `count{ X : goal }` | number of distinct bindings of `X` satisfying `goal` |
| `sum{ X : goal }`, `min{...}`, `max{...}` | integer aggregates over distinct bindings |

Regex flavour: a minimal backtracking matcher supporting `^ $ . * + ? [] | ()`
and escapes. No backreferences, no lookaround. Compiled once per query, with a
step budget. We do not take a regex dependency for this.

### Safety rules — checked before evaluation, reported as errors

1. **Range restriction.** Every variable in a rule head appears in a positive
   body literal of that rule.
2. **Negation safety.** Every variable inside `!atom(...)` appears in a positive
   literal earlier in the same body.
3. **Comparison safety.** Both sides of a comparison are bound by an earlier
   positive literal, or are literals.
4. **Aggregate safety.** Every variable free in the aggregate's goal but used
   outside it is bound outside it.
5. **Stratification.** See below.
6. **Arity consistency.** A relation's arity is fixed by its first use; a later
   use with different arity is an error naming both sites.
7. **Opaque atom comparison.** `=` and `!=` where either side is an opaque
   atom ([01-facts.md](01-facts.md) § Integers) is an error naming the variable.
   Opaque atoms are not deduplicated, so identity comparison would silently
   return false for equal strings. `match`, `contains`, `prefix`, and `suffix`
   are allowed and work on the underlying bytes.
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
}
```

Flat `Vec<Atom>` rather than `Vec<[Atom; N]>`: arity is runtime-determined, and
a flat buffer keeps one code path instead of a macro-generated family. Row `i`
is `data[i*arity .. (i+1)*arity]`.

Sorted + deduplicated is the invariant every operation preserves. It gives
sort-merge joins, binary-search lookup, and free set semantics.

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

Scope for v1, kept deliberately narrow:

- Applies to **constant bindings in a query goal**, propagated through recursive
  predicates. Adornment is on bound/free argument positions only.
- Not applied where it cannot help (non-recursive predicates, fully-free goals).
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

### Evaluation

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

Joining a rule body: order literals by a simple cost heuristic (most-bound-first
— a literal with `k` already-bound variables costs `|R| / 2^k`), then for each
literal either **binary-search** the sorted relation on its bound prefix, or
**scan** if nothing is bound. No query optimizer beyond this, and none is
warranted at our scale.

Joins produce tuples into a scratch buffer; sort + dedup once per iteration
rather than per tuple.

### Determinism

Non-negotiable (invariant 8, [00-overview.md](00-overview.md)).

- All relations sorted by column order; iteration follows storage order.
- Query results sorted by the goal's term order before output.
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
| `max_strata` | 32 | reject at planning |
| `max_body_literals` | 32 | reject at planning |
| `max_regex_steps` | 100,000 | abort, `status: "budget-exceeded"` |

`max_result_bytes` exists because rows are not uniformly sized and the consumer
is a context window. A SCIP symbol string runs ~68 characters ≈ 17 tokens; 1,000
rows with two symbol columns is ~34,000 tokens of output reported as
`status: ok`. A row cap alone does not bound that. Both caps are checked, and
whichever fires first is the one named in `cap`.

A truncated result is still sorted, so the first N rows are a stable prefix, not
an arbitrary sample. This matters: an agent paging through results must get the
same prefix each time.

### Public API

```rust
pub struct Engine { /* relations, interner handle */ }

impl Engine {
    pub fn load(facts: &FactStore) -> Engine;
    pub fn load_rules(&mut self, src: &str) -> Result<(), Diagnostic>;
    pub fn query(&self, src: &str, limits: &Limits) -> Result<QueryResult, Diagnostic>;
}

pub struct QueryResult {
    pub columns:   Vec<String>,   // variable names from the goal, in order
    pub rows:      Vec<Vec<Atom>>,
    pub truncated: bool,
    pub cap:       Option<&'static str>,
    pub stats:     Stats,         // derived tuples, elapsed, strata, plan
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
5. **Differential.** A naive (non-semi-naive) evaluator lives in
   `#[cfg(test)]`. Every conformance program is evaluated by both and the
   results compared. This is the only practical defence against a subtle
   semi-naive bug, which otherwise manifests as quietly missing rows.
6. **stdlib.dl.** Every rule in `rules/stdlib.dl` has a fixture-based test with
   a hand-verified expected answer.
7. **Fuzzing.** The parser is fuzzed for panics. Parsing untrusted input must
   produce a `Diagnostic`, never an abort.
8. **Demand transformation.** For each seeded stdlib traversal, assert that a
   constant-bound goal derives **strictly fewer** tuples than the same goal run
   with the transformation disabled, and that both produce identical results.
   Equal counts mean the transformation silently failed to apply, which is the
   failure mode that only shows up as an unexplained timeout on a big repo.
