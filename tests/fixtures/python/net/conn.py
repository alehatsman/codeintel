"""A second connection layer, exporting a function with the same name as
db.conn.open. That ambiguity is the whole point: tier A cannot decide which
open a caller in a third file means, and gives up rather than guessing. A
type checker can.
"""


def open(url: str) -> bool:
    """Open a socket."""
    return url.startswith("tcp://")
