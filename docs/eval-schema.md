# codeintel schema

Everything below is what `codeintel schema` prints. It is the whole contract:
relations, vocabularies, rules, builtins. A question you cannot answer from this
page is a question the schema is failing to support.

## Language

Datalog, Prolog-flavoured. A query is a body introduced by `?-` and ended by `.`

```prolog
?- def(S, F, "function", N), at(S, F, L).
```

Every variable the goal mentions becomes an output column, in order of first
appearance. `_` matches anything and is **not** a column — use it for arguments
you do not want back. A query may also define its own rules before the `?-`.

Literals are joined by `,` (conjunction). `!p(..)` is negation and every
variable inside it must already be bound by an earlier positive literal.

## Atoms

Every column is an atom: either a **non-negative integer** or an **interned
string**. They are different atoms — `Line = 42` matches, `Line = "42"` never
does. Lines are 1-based; columns are 0-based byte offsets; byte ranges are
0-based and end-exclusive.

## Base relations

Extracted, never inferred. 14 of them.

```
file(F, Lang)                          one row per indexed file. F is a repo-relative path.
def(S, F, Kind, Name)                  one row per definition. S is the symbol id.
def_span(S, StartLine, EndLine, StartByte, EndByte)   the full definition, body included
def_name(S, Line, Col)                 where the identifier itself is written
def_sig(S, Sig)                        declaration header, body stripped
def_doc(S, Doc)                        attached doc comment. Absent if undocumented.
parent(Child, Parent)                  semantic owner, one hop. Parent is a symbol id
                                       or a file path. Exactly one row per def.
exported(S)                            present iff visible outside its module
resolved(S)                            present iff S has SCIP-grade identity
import(F, Module, Alias)               Module as written. Alias is "" when absent.
scip_ref(S, F, Line, Col, From, Role)  compiler-resolved occurrence inside definition From
name_ref(Name, F, Line, Col, From)     unresolved identifier occurrence. Names no symbol.
implements(S, T, Prov)                 S implements, overrides or satisfies T
extern(S, Manager, Pkg, Version)       S belongs to a third-party package
```

Observed counts in this index: `file` 8 · `def` 23 · `def_span` 23 ·
`def_name` 23 · `def_sig` 14 · `def_doc` 5 · `parent` 23 · `exported` 15 ·
`resolved` 14 · `import` 9 · `scip_ref` 17 · `name_ref` 6 · `implements` 2 ·
`extern` 2.

## Vocabularies

```
Kind   module type interface struct enum trait class function method
       constructor field constant variable macro typealias unknown
       observed here: struct 4 · method 3 · function 12 · class 1 · trait 1

Role   def read write import test generated forward
Prov   exact (SCIP, type-resolved)  |  name (tree-sitter, matched by identifier text)
Lang   rust go typescript javascript python java ruby c cpp ...
       observed here: rust 5 · go 1 · typescript 2
```

## Derived relations (rules)

```
ref(S, F, L, C, From, Role, Prov)   every occurrence, both tiers
local_def(F, N)                     N is defined somewhere in F
ambiguous(N)                        more than one definition carries the name N

symbol_at(F, Line, S)               every definition whose span covers Line
innermost_at(F, Line, S)            the tightest one. The way in from a file:line.
tighter_at(F, Line, S)              some definition at Line is strictly narrower than S

callable(Kind)                      function, method, constructor, macro
calls_at(From, S, F, L, Prov)       a call, with its position and provenance
calls(From, S)                      From calls S
calls_exact(From, S)                ...with SCIP evidence only
callers(C, S)   callees(S, C)       the two directions, spelled out
callers_by_name(C, N)               callers of anything named N
impact_by_name(C, N)                transitive impact of anything named N

impact_of(Seed, C)                  who reaches Seed, transitively. SEEDED — bind Seed.
reach_of(Seed, C)                   what Seed reaches, transitively. SEEDED.
impact_of_exact(Seed, C)            ...SCIP evidence only
reaches(A, B)   recursive(S)        whole-graph closure. Expensive, unseeded.

file_of(S, F)   defines(F, S)       the two directions
at(S, F, L)                         S is defined in F at line L
within(Child, Ancestor)             containment closure, up to the file
is_test(F)                          F is a test file

about(S, Rel, A, B, L)              everything about S in one query. Rel is one of
                                    sig doc defined parent caller callee implements
                                    implementor test extern
depends(F, G)                       F references something defined in G
dead_export(S)                      exported, never referenced outside its own file
ref_outside(S, F)                   S is referenced outside F
entrypoint(S)                       callable, and nothing calls it
def_lines(S, N)                     how many lines S spans
undocumented_export(S)              exported with no doc comment
uses_package(F, P)                  F references a symbol from package P

dead_export_exact(S)   ref_outside_exact(S, F)
entrypoint_exact(S)    depends_exact(F, G)    uses_package_exact(F, P)
```

**Provenance matters.** Anything reading `calls` or `ref` inherits resolution
quality. Over a `name`-provenance graph `dead_export` over-reports and
`entrypoint` **inverts** — an unresolved call is an invisible call. The `_exact`
forms gate on SCIP evidence; prefer them when the answer must be trusted.

**Seeded traversals.** `impact_of` and `reach_of` take the seed first, and the
engine pushes that binding into the recursion. `reaches` does not, and on a
large repo it computes every pair.

## Builtins

```
X = Y      X != Y            atom identity, any atom
X < Y      <=   >   >=       INTEGERS ONLY. Comparing strings is an error.
X = A + B  -  *  /           integer arithmetic. Overflow and /0 are errors.
between(Lo, Hi, X)           generator: binds X to each integer in Lo..=Hi
match(S, "re")               regex over the string behind S. Pattern is a literal.
prefix(S, "p")  suffix(S, "s")  contains(S, "c")    cheaper string tests
N = count{ X : goal }        distinct bindings of X satisfying goal
```

## Worked queries

```prolog
% You have a file:line from a stack trace and no symbol. Lift it into the graph.
?- innermost_at("src/store.rs", 142, S), about(S, Rel, A, B, L).

% Blast radius of a change, precise evidence only, with locations.
?- def(S, _, _, "get"), impact_of_exact(S, C), def(C, F, _, N), at(C, F, L).

% Exported symbols under src/store/ that nothing outside their file references.
?- dead_export(S), def(S, F, _, _), prefix(F, "src/store/").

% Callers per function, as a count.
?- def(S, _, "function", N), C = count{ X : calls(X, S) }.
```
