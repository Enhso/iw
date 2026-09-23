"""Source document fetching for the research worker."""

from dataclasses import dataclass
from typing import Literal


@dataclass(frozen=True)
class SourceDocument:
    """A fetched or fixture-loaded document from a research provider.

    Attributes:
        id: Global Constraint 10 id, prefixed `src:`.
        provider: The provider this document came from.
        title: Document title.
        url: Document URL.
        published: `YYYY-MM-DD`, or `""` if unknown.
        retrieved_at: RFC 3339 UTC timestamp of when this document was fetched.
        text: The document's full text content.
    """

    id: str
    provider: Literal["wikipedia", "arxiv", "asknews_news", "asknews_wiki"]
    title: str
    url: str
    published: str
    retrieved_at: str
    text: str
