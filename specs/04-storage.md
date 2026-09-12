---
id: storage
status: proposed
binding: no
---
# 04 — Storage

The fact store is **100% derived**. `rm -rf .codeintel` is always a valid
repair, nothing user-authored is ever written there, and there is no migration
path — a schema bump means reindex.

That property is what buys the simplicity below. We are not a database.

## Layout

```
.codeintel/
  manifest.json          index metadata + per-file segment table
  dict.bin               string interner, append-only
  dict.idx               offset table into dict.bin
  seg/<blake3-of-path>.bin   one segment per source file
  seg/_scip.bin          facts from SCIP documents with no tier-A counterpart
```

Add `.codeintel/` to `.gitignore` on `codeintel index` if a `.gitignore` exists
and does not already ignore it. Announce it; do not do it silently.

## Interner

Maps `&str` ↔ `Atom(u32)`. Append-only, so an id is stable for the life of the
index and segments never need rewriting when the dictionary grows.

**Id space** ([01-facts.md](01-facts.md) § Integers):

```
0                        the empty string
1 ..= 0x0FFF_FFFF        small non-negative integers, encoded as themselves
0x1000_0000 ..= u32::MAX string-table entries
```

`dict.bin` is concatenated UTF-8 with no separators. `dict.idx` is a `Vec<u32>`
of start offsets; entry `i` spans `idx[i]..idx[i+1]`. Both are `mmap`-ed;
resolving an atom to a string is two loads and a slice.

In-memory, the indexer additionally holds a `HashMap<&str, Atom>` borrowing from
the mmap for dedup during a run. The query path never needs it — queries compare
atoms, and only materialize strings when formatting output.

**Growth.** Atoms are never reused and never renumbered. A long-lived index on a
churning repo accumulates dead strings. Reclaim is `codeintel index --rebuild`,
which rewrites the dictionary and every segment from scratch. There is no
incremental GC and there will not be one — a dictionary big enough to matter is
a repo big enough that a 30-second rebuild is acceptable.

## Segment format

One file per indexed source file, holding every fact that file produced.

```
magic       "CIF1"                          4 bytes
schema_ver  u32 le                          4
n_relations u32 le                          4
--- per relation ---
rel_id      u32 le                          4   index into the relation table
arity       u32 le                          4
n_rows      u32 le                          4
rows        [u32 le; arity * n_rows]        4 * arity * n_rows
```

Rows are sorted by column order and deduplicated **at write time**, so loading
is concatenate + merge, never sort-from-scratch.

The relation table (id → name, arity) lives in `crates/facts` as a compile-time
constant, not in the file. Schema changes bump `schema_ver`; a mismatch on open
is `status: "stale"` with the reindex command, never a silent read.

Endianness is fixed little-endian and the loader validates alignment. An index
is not portable across architectures with different `u32` alignment
requirements — it is a cache, so this is stated, not solved.

## Loading

```
mmap every segment
for each relation: concatenate its rows across segments, merge-sort, dedup
```

No parsing. The on-disk bytes are the in-memory representation. For a 1M-symbol
repo (~200 MB, [01-facts.md](01-facts.md) § Scale) the merge is the only real
cost and it is linear in row count.

Optimization, only if measurement demands it: cache the merged per-relation
arrays in `.codeintel/merged/` keyed by the manifest hash, so an unchanged index
loads with zero merging. **Do not build this until a benchmark shows load time
above 200 ms.** It is listed here so a future agent knows it was considered and
deliberately deferred.

## Manifest

```json
{
  "schema_version": 1,
  "created_at": "2026-09-12T10:00:00Z",
  "root": "/Users/x/proj",
  "scip": [
    { "path": "index.scip", "tool": "scip-typescript 0.4.0",
      "mtime": 1757000000, "documents": 812 }
  ],
  "files": {
    "src/store.rs": {
      "seg": "a3f1...bin", "mtime": 1757000000, "size": 4021,
      "hash": "blake3:9c2e...", "lang": "rust", "tiers": ["ts", "scip"]
    }
  }
}
```

`hash` is content, `mtime`+`size` is the fast path. A file is unchanged iff
mtime and size both match; otherwise hash before deciding to re-extract.

## Incremental reindex

```
walk the tree (ignore crate, gitignore-aware)
  new file          -> extract, write segment, add manifest entry
  changed file      -> extract, overwrite segment, update entry
  unchanged file    -> skip
  vanished file     -> delete segment, drop entry
if any SCIP input changed (mtime or size):
  re-ingest all SCIP -> rewrite _scip.bin -> re-run the anchor join
```

Single-file change: one parse, one segment write, one manifest write. Target
< 1 s, dominated by the tree walk.

**SCIP is all-or-nothing.** A SCIP index carries cross-file references, so a
changed `index.scip` invalidates the reference graph globally. Attempting
per-document SCIP incrementality is a correctness trap: a reference that
*disappeared* from a document leaves no evidence in any other document, so
there is nothing to drive its removal. Re-ingest the whole thing.

Corollary worth surfacing to the user: after editing source, the tier-A facts
are fresh and the tier-B facts are stale until the indexer reruns. `status`
reports this per tier. A query whose answer depends on `"exact"` provenance
against a stale SCIP index is answering about the past, and the agent must be
able to see that.

## Concurrency

**One writer, many readers, no locking protocol.**

- `codeintel index` writes each new or changed segment to `seg/<hash>.bin.tmp`
  and renames it into place, then writes `manifest.json.tmp` and renames it
  **last**. The manifest is the only thing that names segments, so a reader sees
  either the old index or the new one, never a mix. Orphaned `.tmp` files from a
  crashed run are removed at the start of the next one.
- Readers `mmap` segments named by the manifest they loaded. A concurrent
  reindex may unlink those segments; the mapping stays valid on POSIX until the
  reader closes it.
- Two concurrent `index` runs are undefined behaviour. Take an advisory lock on
  `.codeintel/lock` (`flock`, `LOCK_EX | LOCK_NB`) and fail with
  `status: "locked"` rather than racing.

Windows has no unlink-open-file semantics and is out of scope for v1. Say so in
the README rather than half-supporting it.

## What is not here, and why

- **No SQLite.** A C dependency, schema migrations, and a parse/load step, for a
  store that is derived, single-writer, and rebuildable. dex paid all three
  costs; [research.md](../docs/research.md) §5.
- **No WAL, no transactions, no crash recovery.** A crashed index leaves the
  previous manifest pointing at valid segments, plus orphaned `.tmp` files that
  the next run removes.
- **No compression.** `u32` columns of interned ids are already dense. Adding
  compression would put a decode step on the load path to save disk nobody is
  short of.
- **No network format, no export.** `codeintel query --format json` is the
  export. If someone needs the raw facts, they query for them.
