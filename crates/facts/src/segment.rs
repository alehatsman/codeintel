//! One segment per source file: every fact that file produced.
//!
//! The format is `specs/04-storage.md` § Segment format. Rows are sorted and
//! deduplicated at write time, so loading is concatenate and merge rather than
//! sort-from-scratch, and the on-disk `u32`s are the in-memory representation.
//!
//! Reading is total: a truncated, zero-filled, or wrong-version segment is
//! reported, never read through. Atom `0` is the *integer* zero
//! (`specs/01-facts.md` § Integers), so a zero-filled segment decodes as
//! well-formed facts claiming every symbol lives at line 0 — a wrong answer
//! with no error, which is why the checksum is not optional.

use std::collections::BTreeMap;
use std::io::{Error, ErrorKind, Result};

use datalog::atom::Atom;
use datalog::relation::Relation;

use crate::schema::{self, SCHEMA_VERSION};

/// Leading bytes of every segment.
const MAGIC: [u8; 4] = *b"CIF1";

/// Bytes before the first relation: magic, `schema_ver`, `n_relations`.
const HEADER: usize = 12;

/// Bytes per relation before its rows: `rel_id`, `arity`, `n_rows`.
const REL_HEADER: usize = 12;

/// Bytes of blake3 after the body.
const CHECKSUM: usize = 32;

/// The facts of one file, grouped by relation.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Segment {
    rels: BTreeMap<u32, Relation>,
}

impl Segment {
    /// An empty segment.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a row to a relation, by name.
    ///
    /// Returns false for an unknown relation or a row of the wrong width. Both
    /// are bugs in the caller, and a reshaped row would hide the defect in the
    /// answer instead of surfacing it here.
    pub fn push(&mut self, relation: &str, row: &[Atom]) -> bool {
        let Some(rel) = schema::by_name(relation) else {
            return false;
        };
        self.rels
            .entry(rel.id)
            .or_insert_with(|| Relation::new(rel.arity))
            .push(row)
    }

    /// How many rows it holds, across every relation.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rels.values().map(Relation::len).sum()
    }

    /// True when it holds no rows at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rels.values().all(Relation::is_empty)
    }

    /// The rows of one relation, by name.
    #[must_use]
    pub fn relation(&self, name: &str) -> Option<&Relation> {
        self.rels.get(&schema::by_name(name)?.id)
    }

    /// Every non-empty relation, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (&'static schema::Rel, &Relation)> {
        self.rels
            .iter()
            .filter(|(_, rows)| !rows.is_empty())
            .filter_map(|(id, rows)| Some((schema::by_id(*id)?, rows)))
    }

    /// Sort and deduplicate every relation. Idempotent.
    pub fn settle(&mut self) {
        for rows in self.rels.values_mut() {
            rows.settle();
        }
    }

    /// The on-disk bytes, settled first so the file is canonical.
    ///
    /// Canonical matters: `docs/plan.md` M2 requires that indexing twice
    /// produces byte-identical segments, and that shuffling the file order
    /// changes nothing.
    ///
    /// # Errors
    /// `InvalidData` when a relation holds more rows than the `u32` row count
    /// can name. Unreachable in practice — it is 4 billion facts from one
    /// source file — and refused rather than truncated, because a truncated
    /// count writes a file that decodes cleanly into the wrong answer.
    pub fn encode(&mut self) -> Result<Vec<u8>> {
        self.settle();
        let live: Vec<(&schema::Rel, &Relation)> = self.iter().collect();
        let body_len = HEADER
            + live
                .iter()
                .map(|(rel, rows)| REL_HEADER + 4 * rel.arity * rows.len())
                .sum::<usize>();
        let mut out = Vec::with_capacity(body_len + CHECKSUM);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&SCHEMA_VERSION.to_le_bytes());
        out.extend_from_slice(&count(live.len())?.to_le_bytes());
        for (rel, rows) in live {
            out.extend_from_slice(&rel.id.to_le_bytes());
            out.extend_from_slice(&count(rel.arity)?.to_le_bytes());
            out.extend_from_slice(&count(rows.len())?.to_le_bytes());
            for row in rows.iter() {
                for atom in row {
                    out.extend_from_slice(&atom.to_le_bytes());
                }
            }
        }
        let checksum = blake3::hash(&out);
        out.extend_from_slice(checksum.as_bytes());
        Ok(out)
    }

    /// Read a segment back.
    ///
    /// # Errors
    /// `InvalidData` for a wrong magic, a `schema_version` this build does not
    /// speak, a failed checksum, an unknown relation id, an arity that
    /// disagrees with the relation table, or a row count that does not fit the
    /// bytes present.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let body = bytes
            .len()
            .checked_sub(CHECKSUM)
            .and_then(|n| bytes.get(..n))
            .ok_or_else(|| corrupt(format!("{} bytes is shorter than a header", bytes.len())))?;
        let stamped = bytes.get(body.len()..).unwrap_or_default();
        if blake3::hash(body).as_bytes() != stamped {
            return Err(corrupt("the body checksum does not match"));
        }
        let mut r = Reader { bytes: body, at: 0 };
        if r.take(4)? != MAGIC {
            return Err(corrupt("wrong magic; this is not a segment"));
        }
        let version = r.u32()?;
        if version != SCHEMA_VERSION {
            return Err(corrupt(format!(
                "schema_version {version}, but this build writes {SCHEMA_VERSION}"
            )));
        }
        let n_relations = r.u32()?;
        let mut rels = BTreeMap::new();
        for _ in 0..n_relations {
            let id = r.u32()?;
            let arity = index(r.u32()?)?;
            let n_rows = index(r.u32()?)?;
            let Some(rel) = schema::by_id(id) else {
                return Err(corrupt(format!("relation id {id} is not in this schema")));
            };
            if arity != rel.arity {
                return Err(corrupt(format!(
                    "`{}` has arity {arity} here and {} in this schema",
                    rel.name, rel.arity
                )));
            }
            let wanted = arity
                .checked_mul(n_rows)
                .and_then(|n| n.checked_mul(4))
                .ok_or_else(|| corrupt(format!("`{}` claims {n_rows} rows", rel.name)))?;
            let data = r.take(wanted).map_err(|cause| {
                corrupt(format!(
                    "`{}` claims {n_rows} rows, which is more than the file holds ({cause})",
                    rel.name
                ))
            })?;
            let mut rows = Relation::new(arity);
            let (words, _) = data.as_chunks::<4>();
            for row in words
                .iter()
                .copied()
                .map(u32::from_le_bytes)
                .collect::<Vec<Atom>>()
                .chunks_exact(arity)
            {
                rows.push(row);
            }
            rows.settle();
            rels.insert(id, rows);
        }
        if r.at != body.len() {
            return Err(corrupt(format!(
                "{} trailing bytes after the last relation",
                body.len() - r.at
            )));
        }
        Ok(Self { rels })
    }
}

/// A cursor over the body, refusing to read past the end.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .ok_or_else(|| corrupt("a length overflowed"))?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| corrupt("the segment ends mid-record"))?;
        self.at = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32> {
        let bytes = self.take(4)?;
        let (words, _) = bytes.as_chunks::<4>();
        words
            .first()
            .copied()
            .map(u32::from_le_bytes)
            .ok_or_else(|| corrupt("the segment ends mid-record"))
    }
}

/// A length as a `u32` row or relation count.
fn count(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|cause| corrupt(format!("{n} does not fit a u32 count: {cause}")))
}

/// A `u32` from the file as an index into it.
fn index(n: u32) -> Result<usize> {
    usize::try_from(n)
        .map_err(|cause| corrupt(format!("{n} does not fit this machine's usize: {cause}")))
}

fn corrupt(message: impl AsRef<str>) -> Error {
    Error::new(
        ErrorKind::InvalidData,
        format!(
            "a segment is corrupt: {}. Repair with `rm -rf .codeintel && codeintel index .`",
            message.as_ref()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Segment {
        let mut seg = Segment::new();
        assert!(seg.push("file", &[100, 101]));
        assert!(seg.push("def", &[200, 100, 201, 202]));
        assert!(seg.push("def", &[203, 100, 201, 204]));
        assert!(seg.push("def_span", &[200, 1, 9, 0, 120]));
        seg
    }

    #[test]
    fn a_segment_round_trips() {
        let mut seg = sample();
        let bytes = seg.encode().expect("encodes");
        let back = Segment::decode(&bytes).expect("decodes");
        assert_eq!(back, seg);
        assert_eq!(back.len(), 4);
    }

    #[test]
    fn encoding_is_canonical_across_push_order() {
        let mut forward = sample();
        let mut backward = Segment::new();
        assert!(backward.push("def_span", &[200, 1, 9, 0, 120]));
        assert!(backward.push("def", &[203, 100, 201, 204]));
        assert!(backward.push("def", &[200, 100, 201, 202]));
        assert!(backward.push("file", &[100, 101]));
        assert_eq!(
            forward.encode().expect("encodes"),
            backward.encode().expect("encodes")
        );
    }

    #[test]
    fn duplicate_rows_collapse() {
        let mut seg = Segment::new();
        assert!(seg.push("file", &[100, 101]));
        assert!(seg.push("file", &[100, 101]));
        seg.settle();
        assert_eq!(seg.len(), 1);
    }

    #[test]
    fn an_unknown_relation_is_refused() {
        let mut seg = Segment::new();
        assert!(!seg.push("has_type", &[1, 2, 3]));
        assert!(!seg.push("file", &[1]));
        assert!(seg.is_empty());
    }

    #[test]
    fn a_flipped_byte_is_caught() {
        let mut seg = sample();
        let mut bytes = seg.encode().expect("encodes");
        let last = bytes.len() - CHECKSUM - 1;
        if let Some(b) = bytes.get_mut(last) {
            *b ^= 0xff;
        }
        let err = Segment::decode(&bytes).expect_err("a flipped row byte is corruption");
        assert!(err.to_string().contains("checksum"), "{err}");
    }

    #[test]
    fn a_zero_filled_segment_is_not_facts() {
        // The failure 04-storage.md § Concurrency describes: atom 0 is the
        // integer zero, so zeroed bytes would otherwise decode as rows saying
        // every symbol lives at line 0.
        let err = Segment::decode(&[0u8; 64]).expect_err("zeroes are not a segment");
        assert!(err.to_string().contains("corrupt"), "{err}");
    }

    #[test]
    fn truncation_is_caught() {
        let mut seg = sample();
        let bytes = seg.encode().expect("encodes");
        for n in [0, 1, HEADER, HEADER + 4, bytes.len() - 1] {
            assert!(
                Segment::decode(bytes.get(..n).unwrap_or_default()).is_err(),
                "{n} bytes decoded"
            );
        }
    }

    #[test]
    fn a_lying_row_count_is_caught() {
        let mut seg = sample();
        let mut bytes = seg.encode().expect("encodes");
        // The first relation's n_rows sits at HEADER + 8.
        let at = HEADER + 8;
        if let Some(slot) = bytes.get_mut(at..at + 4) {
            slot.copy_from_slice(&9_999u32.to_le_bytes());
        }
        let body = bytes.len() - CHECKSUM;
        let checksum = blake3::hash(bytes.get(..body).unwrap_or_default());
        if let Some(slot) = bytes.get_mut(body..) {
            slot.copy_from_slice(checksum.as_bytes());
        }
        let err = Segment::decode(&bytes).expect_err("a lying row count is corruption");
        assert!(
            err.to_string().contains("more than the file holds"),
            "{err}"
        );
    }
}
