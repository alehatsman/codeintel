"""A test file, so is_test(F) has something to match on by path."""

from store.store import Store, warm


def test_warm() -> None:
    assert warm(Store()) is None
