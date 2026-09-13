"""One of every kind our Python tags.scm can emit.

This is the fixture behind the kind-fidelity test in docs/plan.md M5:
upstream's tags.scm has no @definition.method, so every method arrives as a
function, and it tags a module-level assignment @definition.constant, which
is a thing Python cannot state.
"""

from functools import lru_cache

BANNER = "codeintel"
_retries: int = 3


class Config:
    """A class, with a documented field and a documented method."""

    limit: int = 64
    _quiet = False

    def __init__(self, limit: int = 64) -> None:
        """A dunder is magic, not private: PEP 8 says so."""
        self.limit = limit

    def handle(self, key: str) -> bool:
        """Handle one key.

        Two lines of documentation, to prove newlines survive.
        """
        return self.limit > 0 and len(key) > 0

    def _describe(self) -> str:
        return BANNER

    @property
    def quiet(self) -> bool:
        """A decorated method: the decorator is in the span."""
        return self._quiet


class Handler(Config):
    """A subclass. The superclass list is not an owner and is not captured."""

    def handle(self, key: str) -> bool:
        return not super().handle(key)


@lru_cache(maxsize=None)
def describe(config: Config) -> str:
    """A decorated function."""
    return config._describe()


def outer() -> int:
    """A nested function: `inner` is owned by `outer`, not by the file."""

    def inner() -> int:
        return _retries

    return inner()


# A non-ASCII character before a definition on its own line. `scip-python`
# counts that column in UTF-16 and declares no encoding, so the second
# definition's SCIP occurrence is ambiguous and skipped (#21).
WIDE = "é"; AFTER_WIDE = 1
