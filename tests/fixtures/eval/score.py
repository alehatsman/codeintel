#!/usr/bin/env python3
"""Score agent-written Datalog against the reference answers.

Usage:
  score.py expected                 -> recompute expected answers from the
                                       reference queries in questions.tsv
  score.py answers.tsv              -> score a file of `id<TAB>query` rows

An answer is correct when the SET of values in the question's answer variables
matches the reference. Extra join columns are fine and extra rows are not:
different correct queries expose different join variables, and scoring on the
whole row would be scoring style rather than truth.
"""
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
QUESTIONS = HERE / "questions.tsv"
EXPECTED = HERE / "expected.tsv"


def run(query):
    """Return (columns, rows) or ('error', message)."""
    out = subprocess.run(
        ["cargo", "run", "-q", "-p", "codeintel", "--example", "eval", "--", query],
        cwd=ROOT, capture_output=True, text=True,
    )
    lines = [l for l in out.stdout.splitlines() if l]
    if not lines:
        return None, (out.stderr.strip() or "no output")
    head = lines[0].split("\t")
    if head[0] == "status":
        return None, "\t".join(lines[1].split("\t")[1:])
    columns = head[1:]
    rows = [l.split("\t") for l in lines[1:] if not l.startswith(("rows\t", "truncated\t"))]
    return columns, rows


def project(columns, rows, wanted):
    """The set of tuples over the named columns, as a sorted list."""
    missing = [w for w in wanted if w not in columns]
    if missing:
        return None
    index = [columns.index(w) for w in wanted]
    return sorted({tuple(r[i] for i in index) for r in rows})


def questions():
    for line in QUESTIONS.read_text().splitlines():
        if not line.strip():
            continue
        qid, answer_vars, text, reference = line.split("\t")
        yield qid, answer_vars.split(","), text, reference


def write_expected():
    out = []
    for qid, answer_vars, _text, reference in questions():
        columns, rows = run(reference)
        if columns is None:
            sys.exit(f"question {qid}: reference query failed: {rows}")
        projected = project(columns, rows, answer_vars)
        if projected is None:
            sys.exit(f"question {qid}: reference query does not bind {answer_vars}")
        payload = "|".join(" ".join(t) for t in projected)
        out.append(f"{qid}\t{len(projected)}\t{payload}")
    EXPECTED.write_text("\n".join(out) + "\n")
    print(f"wrote {len(out)} expected answers")


def score(path):
    expected = {}
    for line in EXPECTED.read_text().splitlines():
        qid, _count, payload = line.split("\t", 2)
        expected[qid] = payload
    wanted = {qid: (vars_, text) for qid, vars_, text, _ in questions()}

    passed, failures = 0, []
    for line in Path(path).read_text().splitlines():
        if not line.strip():
            continue
        qid, query = line.split("\t", 1)
        answer_vars = wanted[qid][0]
        columns, rows = run(query)
        if columns is None:
            failures.append((qid, query, f"rejected: {rows}"))
            continue
        projected = project(columns, rows, answer_vars)
        if projected is None:
            failures.append((qid, query, f"does not bind {answer_vars}"))
            continue
        got = "|".join(" ".join(t) for t in projected)
        if got == expected[qid]:
            passed += 1
        else:
            failures.append((qid, query, f"got {got or '<empty>'}"))

    total = passed + len(failures)
    print(f"{passed}/{total}")
    for qid, query, why in failures:
        print(f"  q{qid}: {why}\n        {query}")
    return 0 if not failures else 1


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "expected":
        write_expected()
    elif len(sys.argv) > 1:
        sys.exit(score(sys.argv[1]))
    else:
        sys.exit(__doc__)
