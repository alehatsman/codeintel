"""The database layer. Nothing here may import from ui/."""


def open(url: str) -> bool:
    """Open a connection."""
    return url != ""
