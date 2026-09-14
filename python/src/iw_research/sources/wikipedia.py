"""Wikipedia search and extract fetching, live and fixture-backed."""

import logging
from pathlib import Path
from typing import Any

import httpx
import orjson

from ..schema import make_id, slugify
from . import SourceDocument

logger = logging.getLogger(__name__)

WIKI_API_URL = "https://en.wikipedia.org/w/api.php"
USER_AGENT = "IntelligenceWorkbench/0.1 (research worker; httpx)"


def parse_search_response(data: dict[str, Any]) -> list[str]:
    """Extract result titles from a MediaWiki search response.

    Args:
        data: Decoded JSON body of a `list=search` MediaWiki API response.

    Returns:
        The `title` of each search hit, in response order.
    """
    hits = data.get("query", {}).get("search", [])
    return [str(hit["title"]) for hit in hits]


def parse_extract_response(data: dict[str, Any], title: str) -> tuple[str, str]:
    """Extract page text and url from a MediaWiki extract response.

    Args:
        data: Decoded JSON body of a `prop=extracts|info` MediaWiki API
            response for a single page.
        title: The page title requested, used to build a fallback url.

    Returns:
        A `(text, url)` tuple. `url` falls back to the canonical
        `en.wikipedia.org/wiki/<title>` form when `fullurl` is absent.
    """
    pages = data.get("query", {}).get("pages", {})
    page: dict[str, Any] = next(iter(pages.values()), {})
    text = str(page.get("extract", ""))
    fullurl = page.get("fullurl")
    url = (
        str(fullurl)
        if fullurl
        else f"https://en.wikipedia.org/wiki/{title.replace(' ', '_')}"
    )
    return text, url


def _document_for(title: str, text: str, url: str, retrieved_at: str) -> SourceDocument:
    return SourceDocument(
        id=make_id("src", f"wikipedia-{title}"),
        provider="wikipedia",
        title=title,
        url=url,
        published="",
        retrieved_at=retrieved_at,
        text=text,
    )


def search_and_fetch(
    client: httpx.Client, query: str, limit: int, retrieved_at: str
) -> list[SourceDocument]:
    """Search Wikipedia and fetch a plain-text extract for each hit.

    Args:
        client: An `httpx.Client` used for both the search and extract
            requests.
        query: The search query string.
        limit: Maximum number of search hits to fetch extracts for.
        retrieved_at: RFC 3339 UTC timestamp to stamp onto each document.

    Returns:
        One `SourceDocument` per successfully fetched hit. Returns `[]` and
        logs an error if any request in this source fails.
    """
    headers = {"User-Agent": USER_AGENT}
    try:
        search_response = client.get(
            WIKI_API_URL,
            params={
                "action": "query",
                "list": "search",
                "srsearch": query,
                "srlimit": limit,
                "format": "json",
            },
            headers=headers,
        )
        search_response.raise_for_status()
        titles = parse_search_response(search_response.json())

        documents = []
        for title in titles:
            extract_response = client.get(
                WIKI_API_URL,
                params={
                    "action": "query",
                    "prop": "extracts|info",
                    "explaintext": 1,
                    "exsectionformat": "plain",
                    "inprop": "url",
                    "titles": title,
                    "format": "json",
                },
                headers=headers,
            )
            extract_response.raise_for_status()
            text, url = parse_extract_response(extract_response.json(), title)
            documents.append(_document_for(title, text, url, retrieved_at))
        return documents
    except httpx.HTTPError as exc:
        logger.error("wikipedia search_and_fetch failed: %s", exc, exc_info=True)
        return []


def load_fixture(fixture_dir: Path, retrieved_at: str) -> list[SourceDocument]:
    """Load Wikipedia documents from fixture JSON files.

    Args:
        fixture_dir: Directory containing `wikipedia_search.json` and a
            `wikipedia_extracts/` subdirectory.
        retrieved_at: RFC 3339 UTC timestamp to stamp onto each document.

    Returns:
        One `SourceDocument` per fixture search hit, parsed with the same
        functions the live path uses.
    """
    search_data = orjson.loads((fixture_dir / "wikipedia_search.json").read_bytes())
    titles = parse_search_response(search_data)

    documents = []
    for title in titles:
        extract_path = fixture_dir / "wikipedia_extracts" / f"{slugify(title)}.json"
        extract_data = orjson.loads(extract_path.read_bytes())
        text, url = parse_extract_response(extract_data, title)
        documents.append(_document_for(title, text, url, retrieved_at))
    return documents
