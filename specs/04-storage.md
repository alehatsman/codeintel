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
  seg/<blake3-of-bytes>.bin  one segment per source file, named by its content
```

**There is no `seg/_scip.bin`.** An earlier draft put facts from SCIP documents
with no tier-A counterpart into one blob. They get an ordinary per-file segment
instead, because the blob buys nothing and costs three things: a second load
path, a manifest field to name it, and the loss of per-file forgetting — a
tier-B-only file that is deleted can no longer be dropped on its own. Every
argument for per-file segments applies to them unchanged.

Add `.codeintel/` to `.gitignore` on `codeintel index` if a `.gitignore` exists
and does not already ignore it. Announce it; do not do it silently.

## Interner

Maps `&str` ↔ `Atom(u32)`. Append-only, so an id is stable for the life of the
index and segments never need rewriting when the dictionary grows.

**Id space** ([01-facts.md](01-facts.md) § Integers):

```
0x0000_0000 ..= 0x0FFF_FFFF   small non-negative integers, encoded as themselves
0x1000_0000 ..= 0xFFFF_FFFF   deduplicated strings; 0x1000_0000 is EMPTY ("")
```

`dict.bin` is concatenated UTF-8 with no separators. `dict.idx` is a `Vec<u64>`
of start offsets; entry `i` spans `idx[i]..idx[i+1]`. **u64, not u32** — u32
offsets silently cap `dict.bin` at 4 GiB with no stated limit and wrap to
garbage strings on breach. The extra four bytes per entry costs ~10 MB on a
2.5M-string dictionary. Both are `mmap`-ed;
resolving an atom to a string is two loads and a slice.

In-memory, the indexer additionally holds a `HashMap<&str, Atom>` borrowing from
the mmap for dedup during a run. The query path never needs it — queries compare
atoms, and only materialize strings when formatting output.

**Every string goes through the dedup map, including doc comments.** An earlier
draft bypassed it for strings over 256 bytes. Measured, hashing ~120 MB of doc
comments with blake3 costs ~0.12 s against a 30 s index budget — 0.4% — in
exchange for a third id partition, a safety rule, and an `invalid-query` class
an agent cannot guess. Not worth it, and it violated this document's own rule at
§ Loading: do not build it until a benchmark says so.

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
--- trailer ---
checksum    blake3 of every byte above       32
```

The trailer is what § Concurrency's "every segment carries a body checksum"
means concretely. It sits at the end rather than in the header so the writer
streams the body once and stamps it, and so the layout above stays the layout a
reader walks front-to-back.

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
  "roots": ["/Users/x/proj"],
  "writer_version": "codeintel 0.1.0",
  "extractor_fingerprint": "blake3:7d1a...",
  "dict_bin_len": 40213884,
  "dict_idx_len": 2011240,
  "dict_generation": 3,
  "scip": [
    { "path": "index.scip", "tool": "scip-typescript 0.4.0",
      "mtime": 1757000000, "size": 40218811, "documents": 812,
      "ambiguous": 0 }
  ],
  "scip_collisions": 0,
  "files": {
    "src/store.rs": {
      "seg": "a3f1...bin", "mtime": 1757000000, "size": 4021,
      "hash": "blake3:9c2e...", "lang": "rust", "tiers": ["ts", "scip"],
      "scip_hash": "blake3:9c2e..."
    }
  }
}
```

`hash` is content, `mtime`+`size` is the fast path. A file is unchanged iff
mtime and size both match; otherwise hash before deciding to re-extract.

**`scip_hash` is the content the SCIP inputs last saw.** At ingest it becomes
`hash` when the file was not newer than the newest input, or when `hash`
equals what the previous ingest of the same inputs recorded; otherwise the
recorded value is kept, or `""` when the inputs never saw any version. A file
is `scip-stale` iff `scip_hash` differs from `hash`, so a file that is touched,
or edited and restored, clears itself without rerunning the indexer.
Staleness by mtime alone could not: the touched-but-unchanged path refreshes
`mtime`, and a restored file stayed `scip-stale` until the indexer reran. An
index written before the field existed has none and falls back to mtime until
its next ingest.

**A SCIP input carries `mtime` and `size` for the same reason, and `tool`,
`documents` and `ambiguous` are *not* compared.** Those are only
known after the index is parsed, and a refresh that changes nothing must not parse it — a large
`index.scip` on the query path would put a protobuf decode in front of every
answer. Comparing them would make every refresh look like a changed SCIP input
and re-extract the whole tree. `ambiguous` counts the occurrences placed
nowhere because the index declares no position encoding and the text before
the column is not ASCII ([02-extraction.md](02-extraction.md) § Position
normalization). All three are that input's own, read from it alone, never the
merge: every input carrying a merged number reports the total once per input,
and a reader that sums counts it once per input.

**`scip_collisions` is top-level because it has no per-input value.** It counts
symbols defined in more than one document across the merge of every input
([02-extraction.md](02-extraction.md) § The anchor join), and a collision can
span two inputs. It is carried forward on a refresh that does not parse, like
the per-input counts, and is `0` with no inputs. A manifest written before the
field existed reads `0` until its next ingest.

**`extractor_fingerprint` is blake3 over the binary version, the source of
`crates/extract/src`, the locked versions of the grammar and SCIP crates, every
vendored and authored `.scm` byte, every field of every `lang.rs` row, and the
kind-mapping table. A mismatch forces a full re-extract.** The binary version
alone is not enough: it has been `0.1.0` through every extractor fix, and a
`cargo update` moves a caret-pinned grammar without touching it. Hashing the
source over-invalidates on a comment edit, and that is the cheap direction. The
field list is destructured, so a new `Lang` field does not compile until it is
hashed. Without it, incremental indexing means an
extractor fix reaches only the files a user happens to edit afterwards: half the
index is built by the old query and half by the new one, `status` says `ok`, and
nothing will ever reconcile them. For the same reason the fingerprint is
recorded only by a run that walked every language and finished: a `--lang`
run, or one `max_refresh_ms` stopped, leaves the old value in place, so the
next full run still re-extracts what it skipped. It also makes every bug report irreproducible,
because the reporter's first move is `rm -rf .codeintel`, which destroys the
evidence. Fifteen lines.

**`roots` is an array from day one** even though v1 writes one element.
Retrofitting it after segment paths are committed is an invasive change; doing
it now is one character.

**`seg` is exactly what the writer names it: 64 lowercase hex digits and
`.bin`, and nothing else parses.** It is joined onto `seg/` to load a segment
and to unlink a retired one, and the manifest is a file in a directory a cloned
repository can ship. An unchecked `"/home/x/.ssh/id_ed25519"` or `"../../src/main.rs"`
survives `Path::join` as a path outside the index, and the first refresh that
retires that entry — a file the tree no longer has is enough — deletes it. A
manifest naming any other shape is `corrupt`, never read and never followed.

**`dict_bin_len` / `dict_idx_len` bound what a reader may trust.** The manifest
names segments but does not name dictionary extents, so a reader that loads a
new manifest and mmaps a dictionary mid-append can read an offset past the end
of its mapping. Recovery from a torn append is truncation to the recorded
length: a reader bounds its view to the recorded extents, and a writer
truncates both files to them before it appends.

The rule runs in both directions:

- **Longer than recorded is a torn append; shorter or missing is corrupt.**
  Non-zero extents with `dict.bin` or `dict.idx` absent, or either file shorter
  than its extent, is `corrupt`. Reading that as "no dictionary yet" starts the
  id space over at `""`, and every segment atom past the surviving prefix is
  then handed to a new string: old rows resolve to the wrong names, and the
  next commit makes it permanent. Extents of zero still mean no dictionary,
  whatever is on disk.
- **A commit records the extents the interner vouches for, not the file
  sizes.** A refresh that interns nothing appends nothing, so it leaves a
  crashed run's torn tail where it is — past the extents, where no reader looks
  and the next append cuts it — and records the extents it read through.
  Recording `stat` sizes would stamp those torn bytes as trusted, and every
  later open would fail validation until `rm -rf`.
- **A flush that fails leaves the interner as it was.** The strings it did not
  write are still pending, the view is still the one it had, and a retry cuts
  back to where the failed append began.

**`dict_generation` increments on `--rebuild`.** `--rebuild` renumbers every
atom, and `query --raw` hands raw atom ids to the caller. Without a generation
stamp, a saved raw id resolves after a rebuild to a *different string* rather
than to an error.

## Incremental reindex

```
walk the tree (ignore crate, gitignore-aware)
  new file          -> extract, write segment, add manifest entry
  changed file      -> extract, overwrite segment, update entry
  unchanged file    -> skip
  vanished file     -> delete segment, drop entry
if any SCIP input changed (mtime or size):
  re-extract every file with the new ingest -> the anchor join runs inline
```

The anchor join is not a pass over written facts. Tier A consults what SCIP
knows about a file *while it emits*, so a definition adopts its SCIP symbol
before any row exists and nothing is rewritten twice
([02-extraction.md](02-extraction.md) § The anchor join). That is why a changed
SCIP input re-extracts rather than rewrites: there is nothing to rewrite.

Single-file change: one parse, one segment write, one manifest write. Target
< 1 s, dominated by the tree walk.

**A refresh that changes nothing writes nothing.** No dictionary flush, no
manifest write, no `fsync`. `query` refreshes before every answer, and
committing an unchanged manifest cost 8 ms of a ~40 ms MCP call on a 799-file
tree (#18), which is a durable write per read. "Changed" means the manifest
after the run differs from the manifest before it. Every segment written, every
file forgotten, every touched file's refreshed `mtime` and every dictionary
extent lands in the manifest, so equality is the whole test and no separate
dirty flag can disagree with it. The one exception: **a store with no index yet
always commits**, so `codeintel index` on a tree with nothing to extract leaves
an empty index rather than `no-index`.

**SCIP is all-or-nothing.** A SCIP index carries cross-file references, so a
changed `index.scip` invalidates the reference graph globally. Attempting
per-document SCIP incrementality is a correctness trap: a reference that
*disappeared* from a document leaves no evidence in any other document, so
there is nothing to drive its removal. Re-ingest the whole thing.

Corollary worth surfacing to the user: after editing source, the file is
re-extracted **tier A only** — tier B emits nothing for a file the inputs did
not see ([02-extraction.md](02-extraction.md) § The anchor join) — so its
definitions are `local` and its `scip_ref` rows are gone until the indexer
reruns, while every other file keeps its exact rows. `status` reports the
files. A query whose answer depends on `"exact"` provenance is then answering
about the past for those files, and the agent must be able to see that.

## Concurrency

**One writer, many readers, no locking protocol.**

- `codeintel index` writes each new or changed segment to `seg/<hash>.bin.tmp`,
  **`fsync`s it**, and renames it into place; then writes `manifest.json.tmp`,
  `fsync`s it, and renames it **last**. `<hash>` is blake3 of the segment's
  own bytes, not of the source path: a changed file gets a *new* segment file,
  the one the old manifest names is untouched until the new manifest is in
  place, and it is unlinked only after. A vanished file's segment is likewise
  unlinked after the commit that drops its entry. Every rename is followed by
  an `fsync` of the directory, without which a crash can keep the file and
  lose its name. The `fsync` is not optional: without it
  a power loss can land the manifest rename while segment data is still in page
  cache, and the result is a manifest naming a **zero-filled** segment. Atom `0`
  is the *integer* zero ([01-facts.md](01-facts.md) § Integers), so those bytes
  decode as well-formed facts asserting that every symbol lives at line 0,
  column 0 — no error, no status, a silently wrong answer. The manifest is the only thing that names segments, so a reader sees
  either the old index or the new one, never a mix. A crash before the
  manifest rename leaves the old manifest naming files that all still exist,
  plus orphans — `.tmp` files and segments no manifest names — which the next
  writer removes under the lock before it writes anything.
- Readers `mmap` segments named by the manifest they loaded. A concurrent
  reindex may unlink those segments; the mapping stays valid on POSIX until the
  reader closes it.
- Two concurrent writers are undefined behaviour. Take an advisory lock on
  `.codeintel/lock` (`flock`, `LOCK_EX | LOCK_NB`) and fail rather than racing.
  **`query`'s auto-refresh is a writer and takes the same lock** — an MCP server
  serving two concurrent queries is two writers, and `dict.bin` is append-only
  with no tmp+rename, so interleaved appends put `dict.idx` offsets out of step
  with `dict.bin` bytes and every atom past that point resolves to the wrong
  string, with no checksum to notice.
- `--rebuild` takes the writer lock first and discards under it, and
  `.codeintel/lock` survives the discard: a second writer opening a fresh lock
  file would not contend with the first. A refresh re-reads the manifest after
  taking the lock — one read to decide whether to refresh, one under the lock
  to build on — so a refresh that lost a race never commits a manifest built
  from a copy another writer has already replaced.
- A **reader** that cannot take the lock does not fail. It reads the current
  manifest and returns `status: "stale"`. `locked` is for a second writer;
  telling a reader "locked" is not actionable.
- Every segment carries a body checksum, validated on open, and `n_rows` is
  bounds-checked against the file length. A truncated or zero-filled segment
  returns `status: "corrupt"` with hint `rm -rf .codeintel && codeintel index .`
  — never a panic and never silent rows of atom 0.

Windows has no unlink-open-file semantics and is out of scope for v1. Say so in
the README rather than half-supporting it.

## What is not here, and why

- **No SQLite.** A C dependency, schema migrations, and a parse/load step, for a
  store that is derived, single-writer, and rebuildable. dex paid all three
  costs; [research.md](../docs/research.md) §5.
- **No WAL, no transactions.** Crash recovery is `fsync` + rename ordering +
  per-segment checksums + dictionary extents in the manifest, above. A crashed
  index leaves the previous manifest pointing at valid, checksummed segments,
  plus orphans — `.tmp` files and unnamed segments — that the next run removes.
- **`.codeintel/` must be excluded from file sync.** mmap over a Dropbox/iCloud
  partial write or an SMB/NFS truncation raises `SIGBUS`, which no status code
  can report. One line in the README.
- **No compression.** `u32` columns of interned ids are already dense. Adding
  compression would put a decode step on the load path to save disk nobody is
  short of.
- **No network format, no export.** `codeintel query --format json` is the
  export. If someone needs the raw facts, they query for them.
