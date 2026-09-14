"""arXiv Atom feed search and parsing, live and fixture-backed."""

import logging
import xml.etree.ElementTree as ET
from pathlib import Path

import httpx

from ..schema import make_id
from . import SourceDocument

logger = logging.getLogger(__name__)

ARXIV_API_URL = "https://export.arxiv.org/api/query"
ATOM_NS = "{http://www.w3.org/2005/Atom}"


def parse_atom(content: bytes, retrieved_at: str) -> list[SourceDocument]:
    """Parse an arXiv Atom feed into source documents.

    Args:
        content: Raw Atom XML bytes.
        retrieved_at: RFC 3339 UTC timestamp to stamp onto each document.

    Returns:
        One `SourceDocument` per `<entry>` with a title and id. Entries
        missing either are skipped.

    Raises:
        xml.etree.ElementTree.ParseError: If `content` is not valid XML.
    """
    root = ET.fromstring(content)
    documents = []
    for entry in root.findall(f"{ATOM_NS}entry"):
        title_el = entry.find(f"{ATOM_NS}title")
        id_el = entry.find(f"{ATOM_NS}id")
        if title_el is None or title_el.text is None:
            continue
        if id_el is None or id_el.text is None:
            continue

        summary_el = entry.find(f"{ATOM_NS}summary")
        published_el = entry.find(f"{ATOM_NS}published")

        title = " ".join(title_el.text.split())
        text = ""
        if summary_el is not None and summary_el.text:
            text = " ".join(summary_el.text.split())
        url = id_el.text.strip()
        published = ""
        if published_el is not None and published_el.text:
            published = published_el.text.strip()[:10]

        last_segment = url.rstrip("/").rsplit("/", 1)[-1]
        documents.append(
            SourceDocument(
                id=make_id("src", f"arxiv-{last_segment}"),
                provider="arxiv",
                title=title,
                url=url,
                published=published,
                retrieved_at=retrieved_at,
                text=text,
            )
        )
    return documents


def search(
    client: httpx.Client, query: str, limit: int, retrieved_at: str
) -> list[SourceDocument]:
    """Search arXiv and parse the resulting Atom feed.

    Args:
        client: An `httpx.Client` used for the search request.
        query: The search query string, embedded as `all:<query>`.
        limit: Maximum number of results to request.
        retrieved_at: RFC 3339 UTC timestamp to stamp onto each document.

    Returns:
        One `SourceDocument` per feed entry. Returns `[]` and logs an
        error if the request fails or the response is not valid XML.
    """
    try:
        response = client.get(
            ARXIV_API_URL,
            params={"search_query": f"all:{query}", "start": 0, "max_results": limit},
        )
        response.raise_for_status()
        return parse_atom(response.content, retrieved_at)
    except httpx.HTTPError as exc:
        logger.error("arxiv search failed: %s", exc, exc_info=True)
        return []
    except ET.ParseError as exc:
        logger.error("arxiv response parse failed: %s", exc, exc_info=True)
        return []


def load_fixture(fixture_dir: Path, retrieved_at: str) -> list[SourceDocument]:
    """Load arXiv documents from a fixture Atom feed file.

    Args:
        fixture_dir: Directory containing `arxiv.xml`.
        retrieved_at: RFC 3339 UTC timestamp to stamp onto each document.

    Returns:
        One `SourceDocument` per feed entry, parsed with the same parser
        the live path uses.
    """
    content = (fixture_dir / "arxiv.xml").read_bytes()
    return parse_atom(content, retrieved_at)
