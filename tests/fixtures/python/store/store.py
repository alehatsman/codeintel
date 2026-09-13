"""A class with methods, so that `parent` has something to disagree with the
file about. Unlike Go, the class encloses them: span nesting is enough here.
"""

import re as _re
from typing import Dict


class Entry:
    """One entry in the store."""

    def __init__(self, value: str) -> None:
        self.value = value

    def describe(self) -> str:
        """Render an entry."""
        return _re.sub(r"\s+", " ", self.value.upper())


class Store:
    """A tiny key-value store."""

    def __init__(self) -> None:
        self.entries: Dict[str, Entry] = {}

    def get(self, key: str) -> Entry | None:
        """Read one entry.

        Two lines of documentation, to prove newlines survive.
        """
        return self.entries.get(key)

    def put(self, key: str, value: str) -> None:
        """Write one entry."""
        self.entries[key] = Entry(value)

    def _evict(self) -> None:
        self.entries.clear()


def warm(store: Store) -> Entry | None:
    """A free function that calls a method, so `calls` has an edge to find."""
    return store.get("warm")
