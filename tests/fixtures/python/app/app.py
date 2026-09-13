"""A caller in a third file. `open` is defined twice repo-wide, so tier A's
!ambiguous(N) guard refuses the edge and `calls` has nothing here. Tier B
resolves it exactly.
"""

from db.conn import open


def start() -> bool:
    """Start up."""
    return open("sqlite://memory")
