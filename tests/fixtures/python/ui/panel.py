"""The UI layer. This file deliberately violates the layering rule, and
docs/plan.md M5 requires that a conformance query over import/3 finds it and
that removing the import returns zero rows with status: ok.
"""

from db import conn
import net.conn as _net


def draw() -> bool:
    """Draw the panel."""
    return conn.open("sqlite://memory")
