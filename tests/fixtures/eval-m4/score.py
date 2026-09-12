#!/usr/bin/env python3
"""Score agent-written Datalog against reference answers, using the real binary.

The M1 scorer ran against `cargo run --example eval` over hand-written facts.
This one runs `codeintel query --raw` against a real index, which is the whole
point of the M4 re-run: the facts are extracted, the symbol ids are SCIP's, and
the schema the agent read was generated from the index rather than hand-written.

Usage:
  score.py expected SET ROOT        recompute expected answers from the
                                    reference queries
  score.py score SET ROOT ANSWERS   score a file of `id<TAB>query` rows

An answer is correct when the SET of values in the question's answer variables
matches the reference. Extra join columns are fine and extra or missing rows are
not: different correct queries expose different join variables, and scoring the
whole row would score style rather than truth.
"""
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
BINARY = HERE.parents[2] / "target" / "release" / "codeintel"


def run(query, root):
    """Return (columns, rows) or (None, message)."""
    out = subprocess.run(
        [str(BINARY), "query", query, "--raw", "--no-refresh", "--limit", "5000"],
        cwd=root, capture_output=True, text=True,
    )
    status = ""
    for line in out.stderr.splitlines():
        if line.startswith("status="):
            status = line[len("status="):]
    if status in ("invalid-query", "unstratified", "timeout", "budget-exceeded", ""):
        return None, (status or "no status") + ": " + out.stderr.strip().replace("\n", " ")[:200]
    rows = [l.split("\t") for l in out.stdout.splitlines() if l]
    return columns_of(query), rows


def columns_of(query):
    """The goal's variables, in order of first appearance — same rule the
    engine uses to name result columns."""
    goal = query[query.index("?-") + 2:] if "?-" in query else query
    seen, out = set(), []
    token = ""
    in_string = False
    for ch in goal:
        if ch == '"':
            in_string = not in_string
            token = ""
            continue
        if in_string:
            continue
        if ch.isalnum() or ch == "_":
            token += ch
        else:
            if token and token[0].isupper() and token not in seen:
                seen.add(token)
                out.append(token)
            token = ""
    if token and token[0].isupper() and token not in seen:
        out.append(token)
    return out


def values(columns, rows, wanted):
    """The set of tuples over `wanted`, as `|`-joined strings."""
    if columns is None:
        return None
    index = []
    for name in wanted:
        if name not in columns:
            return None
        index.append(columns.index(name))
    out = set()
    for row in rows:
        if max(index) >= len(row):
            return None
        out.add("\x1f".join(row[i] for i in index))
    return out


def questions(path):
    for line in Path(path).read_text().splitlines():
        if not line.strip():
            continue
        qid, vars_, text, reference = line.split("\t")
        yield qid, vars_.split(","), text, reference


# Questions whose correct answer genuinely IS "no rows", by set. Everything
# else must have a non-empty reference answer: a question answered by nothing
# is passed by any query that returns nothing, including a broken one, and it
# scores itself correct for free. Ten of set A were silently in this state
# because the symbol ids had been written by hand and were wrong.
KNOWN_EMPTY = {
    # `implements/3` and `extern/4` are zero on this fixture even with SCIP —
    # no external dependency, and rust-analyzer emits no implementation
    # relationship for `impl Handler for Config`. Same gap
    # `the_rules_with_no_positive_coverage_are_named` pins in tests/stdlib.rs.
    "A": {"16", "17"},
    "B": set(),
}


def check_not_empty(which, expected):
    """Fail loudly if a question that should have an answer has none."""
    allowed = KNOWN_EMPTY.get(which, set())
    empty = {qid for qid, rows in expected.items() if not rows}
    unexpected = empty - allowed
    if unexpected:
        raise SystemExit(
            f"questions with an EMPTY reference answer: {sorted(unexpected, key=int)}\n"
            "A question answered by no rows is passed by any query that returns "
            "nothing. Fix the question, or add it to KNOWN_EMPTY with a reason."
        )
    stale = allowed - empty
    if stale:
        raise SystemExit(
            f"KNOWN_EMPTY lists {sorted(stale, key=int)} but they now return rows — "
            "give them a real assertion and remove them from the list."
        )


def main():
    mode, which, root = sys.argv[1], sys.argv[2], sys.argv[3]
    qfile = HERE / f"set{which}.tsv"
    efile = HERE / f"expected{which}.tsv"

    if mode == "expected":
        out = []
        for qid, vars_, _text, reference in questions(qfile):
            columns, rows = run(reference, root)
            got = values(columns, rows, vars_)
            if got is None:
                print(f"Q{qid}: REFERENCE FAILED: {rows}", file=sys.stderr)
                got = set()
            out.append(f"{qid}\t{len(got)}\t" + "|".join(sorted(got)))
        efile.write_text("\n".join(out) + "\n")
        check_not_empty(which, {row.split("\t")[0]: row.split("\t", 2)[2] for row in out})
        print(f"wrote {efile}")
        return

    expected = {}
    for line in efile.read_text().splitlines():
        qid, _n, payload = line.split("\t", 2)
        expected[qid] = set(payload.split("|")) if payload else set()

    check_not_empty(which, expected)
    wanted = {qid: v for qid, v, _t, _r in questions(qfile)}
    answers = {}
    for line in Path(sys.argv[4]).read_text().splitlines():
        if not line.strip():
            continue
        qid, query = line.split("\t", 1)
        answers.setdefault(qid, query)

    correct = 0
    for qid in sorted(expected, key=int):
        if qid not in answers:
            print(f"Q{qid}\tMISSING")
            continue
        columns, rows = run(answers[qid], root)
        got = values(columns, rows, wanted[qid])
        if got == expected[qid]:
            correct += 1
            print(f"Q{qid}\tok")
        elif got is None:
            print(f"Q{qid}\tFAIL\t{rows}\t{answers[qid]}")
        else:
            missing = sorted(expected[qid] - got)[:3]
            extra = sorted(got - expected[qid])[:3]
            print(f"Q{qid}\tFAIL\tmissing={missing} extra={extra}\t{answers[qid]}")
    print(f"\n{correct}/{len(expected)}")


if __name__ == "__main__":
    main()
