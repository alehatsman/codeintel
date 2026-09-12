# Cookbook

Every question below is Datalog over the relations in
[01-facts.md](../specs/01-facts.md). None of them is a CLI verb, an MCP tool, or
a function in this binary — that is the whole design, and this file is the
evidence for it.

Read [`codeintel schema`](../specs/05-surface.md) first; it is generated from
your index and tells you which values actually occur in it.

---

## 1. Conformance — start here

**This section needs no SCIP index.** `import/3` comes from
`queries/<lang>/imports.scm`, which is ours, so it works on the first run after
`codeintel index .` with no language indexer installed. Everything in §3 that
touches `calls` or `ref` does not.

The shape is always the same: **write the rule that describes the violation,
then assert that it finds nothing.**

```dl
%% ui_imports_db(F, M)  no module under ui/ may import from db/
ui_imports_db(F, M) :- import(F, M, _), prefix(F, "src/ui/"), contains(M, "db").
```

```sh
codeintel query '?- ui_imports_db(F, M).' --rules conformance.dl --expect-empty
```

`--expect-empty` exits **1** when a row comes back and 0 when none does. It is 1
and not 2 because 2 means the query never ran: "your code violates this" and "I
could not tell you" are different results in CI.

### Layering

One rule per forbidden direction. Naming each one separately is what makes the
CI failure legible.

```dl
%% layer_violation(F, M)  the whole layering policy, one clause per edge
layer_violation(F, M) :- import(F, M, _), prefix(F, "src/ui/"),    contains(M, "db").
layer_violation(F, M) :- import(F, M, _), prefix(F, "src/db/"),    contains(M, "ui").
layer_violation(F, M) :- import(F, M, _), prefix(F, "src/domain/"), contains(M, "infra").
```

### A banned dependency

```dl
%% banned(F, M)  nothing may reach for the old client
banned(F, M) :- import(F, M, _), contains(M, "legacy_http").
```

### An allowed-direction rule (the inverse shape)

Sometimes the policy is a whitelist. State it as "an import that is not one of
the allowed kinds".

```dl
%% allowed(F, M)  what src/core/ is permitted to import
allowed(F, M) :- import(F, M, _), prefix(M, "std").
allowed(F, M) :- import(F, M, _), prefix(M, "crate::core").

%% core_reaches_out(F, M)  src/core/ importing anything else
core_reaches_out(F, M) :- import(F, M, _), prefix(F, "src/core/"), !allowed(F, M).
```

### Layer-crossing at file granularity, once `calls` is available

```dl
%% crosses(F, G)  a file depending on one it should not
crosses(F, G) :- depends(F, G), prefix(F, "src/ui/"), prefix(G, "src/db/").
```

`depends` reads `ref`, so it degrades with provenance — see §4.

---

## 2. Orientation

You have a location and need a symbol. This is how anything gets *into* the
graph.

```dl
?- innermost_at("src/store.rs", 23, S).          % from a stack trace, a diff
?- def(S, F, _, N), contains(N, "auth").         % from a word
?- about(S, Rel, A, B, L).                       % everything about S, one trip
```

`about` is ten questions in one relation, discriminated by `Rel`:
`sig doc defined parent caller callee implements implementor test extern`.

The impact of a change, excluding tests:

```dl
?- innermost_at("src/store.rs", 23, S), impact_of(S, C),
   def(C, F, _, N), !is_test(F).
```

**Seed `impact_of` and `reach_of` with a constant.** Unseeded they compute
all-pairs reachability and hit the budget. `reaches/2` and `recursive/1` are the
deliberately unseeded pair; use them only on a small tree.

---

## 3. The dex 34, as rules

`~/projects/dex`'s `internal/store/store_graph.go` exposes **34 methods**.
[research.md](research.md) §1a names 17 of them and elides the rest; the counts
below were taken from the file itself, because a claim about 34 methods that
only ever shows 17 is not checkable.

**Of the 34: 17 are not questions at all, 13 are Datalog rules, 4 are refused.**

The 17 that are not questions are the write path (`GraphUpsertNodes`,
`GraphUpsertEdges`, `GraphPruneUnseen`, `SetNodeVecs`, `GraphSetCentrality`),
epoch and maintenance bookkeeping (`GraphMaxEpoch`, `GraphSeenTime`,
`GraphStats`, `GraphScale`), bulk dump (`GraphAllNodes`, `GraphAllEdges`),
embeddings (`NodeKNN`, `NodeVecCount`, `NodesNeedingEmbed`), chunk retrieval
(`ChunksByPaths`) and edit spans (`SymbolEditSpanByID`,
`SymbolEditSpansByName`). Here the write path is `codeintel index` plus the
manifest, the bookkeeping is `codeintel status`, and embeddings and chunk
retrieval are a different product — `NodeKNN` takes a query vector.
**Out of scope is a third category, distinct from refused.**

Several of those are one-liners here anyway, which is worth saying because it
makes the table shorter rather than longer: `SymbolEditSpansByName` is
`?- def(S, _, _, "get"), def_span(S, L1, C1, L2, C2).`, `GraphAllNodes` is
`?- def(S, F, K, N).`, and `GraphStats` is `codeintel status`.

The 17 that *are* questions:

| dex method | here |
|---|---|
| `SymbolsByFile` | `?- defines("src/store.rs", S), def(S, _, K, N).` |
| `SelectSymbols` | `?- def(S, F, K, N), contains(N, "auth").` |
| `ExportedSymbolsByDir` | `?- exported(S), def(S, F, _, N), prefix(F, "src/api/").` |
| `GraphQualifiedNameAt` | `?- innermost_at(F, L, S), within(S, A), def(A, _, _, N).` |
| `ImportsForFile` | `?- import("src/store.rs", M, Alias).` |
| `ImportsForDir` | `?- import(F, M, _), prefix(F, "src/db/").` |
| `InternalPackageImports` | `?- import(F, M, _), prefix(M, "crate::").` |
| `ExternalImports` | `?- uses_package(F, P).` |
| `UsedByPackages` | `?- uses_package(F, "tokio").` |
| `MainEntrypoints` | `?- entrypoint(S), def(S, F, _, N).` |
| `CallerFiles` | `?- def(S, _, _, "get"), calls(C, S), def(C, F, _, _).` |
| `UnresolvedInboundForFile` | `?- name_ref(N, "src/store.rs", L, _, _), !def(_, _, _, N).` |
| `Smells.dead_exports` | `?- dead_export(S), def(S, F, _, N).` |
| `Smells.long_functions` | `?- long_def(S, Lines), def(S, F, _, N).` |
| `Smells.undocumented` | `?- undocumented_export(S), def(S, F, _, N).` |
| `TopCentralByDir` | **refused** — see below |
| `PackageCentrality` | **refused** |
| `FileCentrality` | **refused** |
| `GraphCommunities` | **refused** |
| `Smells` *as a ranked list* | **refused** as a ranking; the three above are the facts under it |

Twenty rows, thirteen distinct dex methods — `Smells` is one method that appears
four times, once per thing it bundles. Every answered row is one to five lines
and the longest is three literals. Variation by directory, by package or by file
is a `prefix/2` on a column, not a new method, which is why dex's
`ImportsForDir` and `ImportsForFile` are one row each here rather than two
implementations.

### What is refused, and why

**No centrality, no communities, no importance score, no smell ranking.** This
is invariant 4 in [00-overview.md](../specs/00-overview.md), and it is not
squeamishness: [research.md](research.md) §1c measured what such a score does
over a mixed-provenance graph. A `name`-provenance edge set is missing most
method calls, so "centrality" computed over it ranks the code whose *identifiers
happen to be unique*, not the code that matters. The number would look
authoritative and be wrong, and nothing in the output would say so.

What you can have instead is the fact under the question. `entrypoint/1` and
`dead_export/1` are the two that people usually want centrality *for*, and both
are exact statements about the graph rather than positions in a ranking.

---

## 4. Provenance — read this before trusting §3

Every rule that reads `calls` or `ref` is contaminated by resolution quality,
which is why each ships in two forms.

On `tests/fixtures/rust/`, with a SCIP index present:

```
?- calls(C, S).          4 edges
?- calls_exact(C, S).    3 edges
```

The extra edge is `get -> get` in `src/store.rs`. `Store::get` calls
`self.entries.get(key)`, tier A matches the identifier `get` to the only `get`
it knows, and invents a self-call that does not exist. Its consequence is
visible one rule further on: `?- recursive(S).` reports `get`, and `get` is not
recursive.

That is one false edge in a four-edge graph, and on this repository M3 measured
tier A over-reporting **252 call edges, 17% of what it claims**.

- Use `calls_exact`, `impact_of_exact`, `dead_export_exact`, `entrypoint_exact`,
  `depends_exact` and `uses_package_exact` when precision matters.
- Check `codeintel status` for whether a SCIP index is present and fresh. A
  query whose dependency closure reaches a SCIP-only relation returns
  `status: no-scip` when there is none — it never silently returns zero rows.
- `dead_export` **over**-reports on `name` provenance: an unresolved call is an
  invisible call, so a symbol can look unreferenced when it is not.
- `entrypoint` **inverts**: a callable looks like an entrypoint precisely when
  its callers were not resolved.

### Two artifacts this fixture shows, and what they mean

`?- within(C, P).` returns a row whose child has an empty name, and
`?- depends(F, G).` claims `src/app.rs` depends on `tests/store_test.rs`. Both
trace to one thing: the crate-root module symbol. SCIP places a module's
definition in a document that is not where the `mod` statement is
([plan.md](plan.md) M3 records the same effect on the anchor rate), so
file-granularity rules inherit that placement. Read a `depends` row involving a
module definition as "these two files are in one module graph", not as "this
file uses that file".

`?- ambiguous(N).` lists `self`, `key` and `store` alongside real collisions.
SCIP emits a `local` definition for every parameter and binding, the collapse
table has no word for `Parameter`, and they land in `def` as `unknown`. Filter
with `def(S, _, K, _), K != "unknown"` when that matters.

---

## 5. Writing your own

The rule is the unit of extension. If you are about to ask for a feature, write
the rule first — it is usually shorter than the feature request.

```dl
%% my_question(A, B)  what this is for, one line
my_question(A, B) :- ...
```

The `%%` line is not decoration: `codeintel schema` generates its rule list from
it, and a test asserts every rule has one and that the signature matches the
clause. A rule without a `%%` line is a rule an agent will never find.

Load it with `--rules`. **Clauses are additive** — a file defining `is_test`
*widens* it, because a predicate is the union of its clauses. To *replace* a
stdlib rule, put your version in the query program itself; that shadows the
loaded one, and the shadowing is reported rather than silent.
