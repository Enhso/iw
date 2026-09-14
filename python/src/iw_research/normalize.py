"""Turn fetched source documents into a normalized polars frame."""

import polars as pl

from .schema import Source
from .sources import SourceDocument

MAX_DOC_CHARS = 6000

_FRAME_SCHEMA = {
    "id": pl.Utf8,
    "provider": pl.Utf8,
    "title": pl.Utf8,
    "url": pl.Utf8,
    "published": pl.Utf8,
    "retrieved_at": pl.Utf8,
    "text": pl.Utf8,
}


def normalize(documents: list[SourceDocument]) -> pl.DataFrame:
    """Clean and de-duplicate fetched documents.

    Args:
        documents: Raw documents from one or more sources.

    Returns:
        A frame with columns `id, provider, title, url, published,
        retrieved_at, text`: rows with empty (post-strip) text dropped,
        de-duplicated on `url` (keeping the first), `text` truncated to
        `MAX_DOC_CHARS`, sorted by `provider, title`.
    """
    frame = pl.DataFrame(
        {
            "id": [d.id for d in documents],
            "provider": [d.provider for d in documents],
            "title": [d.title for d in documents],
            "url": [d.url for d in documents],
            "published": [d.published for d in documents],
            "retrieved_at": [d.retrieved_at for d in documents],
            "text": [d.text for d in documents],
        },
        schema=_FRAME_SCHEMA,
    )
    frame = frame.filter(pl.col("text").str.strip_chars().str.len_chars() > 0)
    frame = frame.unique(subset=["url"], keep="first", maintain_order=True)
    frame = frame.with_columns(pl.col("text").str.slice(0, MAX_DOC_CHARS))
    return frame.sort(["provider", "title"])


def to_documents(frame: pl.DataFrame) -> list[SourceDocument]:
    """Convert a normalized frame back into `SourceDocument` objects.

    Args:
        frame: A frame produced by `normalize`.

    Returns:
        One `SourceDocument` per row, in frame order.
    """
    return [
        SourceDocument(
            id=row["id"],
            provider=row["provider"],
            title=row["title"],
            url=row["url"],
            published=row["published"],
            retrieved_at=row["retrieved_at"],
            text=row["text"],
        )
        for row in frame.iter_rows(named=True)
    ]


def to_sources(frame: pl.DataFrame) -> list[Source]:
    """Build the payload's `Source` rows from a normalized frame.

    Args:
        frame: A frame produced by `normalize`.

    Returns:
        One `Source` per row, in frame order.
    """
    return [
        Source(
            id=row["id"],
            title=row["title"],
            url=row["url"],
            provider=row["provider"],
            published=row["published"],
            retrieved_at=row["retrieved_at"],
        )
        for row in frame.iter_rows(named=True)
    ]
