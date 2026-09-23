"""AskNews news + wiki search providers (contracts.md B1).

AskNews' response field names are not part of the cross-repo contract
(only the request shape and "use full text if present, else summary" are);
the dict keys read here (`as_dicts`/`article_url`/`eng_title`/`full_text`/
`summary`/`pub_date` for news, `documents`/`title`/`url`/`content`/`summary`
for wiki, checked against the live API on 2026-09-22) are kept in
one place so it is easy to adjust if AskNews changes them.
"""

import logging
import re
from collections.abc import Iterable
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import httpx
import orjson

from ..schema import make_source_id
from . import SourceDocument

logger = logging.getLogger(__name__)

NEWS_URL = "https://api.asknews.app/v1/news/search"
WIKI_URL = "https://api.asknews.app/v1/wiki/search"

# contracts.md A1: "news_since null means the provider default look-back
# (AskNews: last 30 days of news)".
DEFAULT_HOURS_BACK = 720

_STOPWORDS = frozenset(
    """
    a an the is are was were will would can could should of in on at to for and
    or but with by from that this these those it its as be been being do does
    did if than then so such not no yes has have had which who whom what when
    where why how will
    """.split()
)
_WORD_RE = re.compile(r"[A-Za-z0-9']+")


def _keyword_query(question: str) -> str:
    """Build a keyword query from `question` (contracts.md B1's second query).

    Args:
        question: The research question.

    Returns:
        `question`'s words, lowercased, with common stopwords removed and
        order preserved. Falls back to `question` itself if every word is
        a stopword.
    """
    words = [w for w in _WORD_RE.findall(question.lower()) if w not in _STOPWORDS]
    return " ".join(words) if words else question


def _auth_headers(api_key: str) -> dict[str, str]:
    return {"Authorization": f"Bearer {api_key}"}


def _news_document(article: dict[str, Any], retrieved_at: str) -> SourceDocument | None:
    url = article.get("article_url")
    if not url:
        return None
    title = str(article.get("eng_title") or url)
    text = str(article.get("full_text") or article.get("summary") or "")
    published = str(article.get("pub_date") or "")[:10]
    return SourceDocument(
        id=make_source_id("asknews_news", str(url)),
        provider="asknews_news",
        title=title,
        url=str(url),
        published=published,
        retrieved_at=retrieved_at,
        text=text,
    )


def _wiki_document(item: dict[str, Any], retrieved_at: str) -> SourceDocument | None:
    url = item.get("url")
    if not url:
        return None
    title = str(item.get("title") or url)
    text = str(item.get("content") or item.get("summary") or "")
    return SourceDocument(
        id=make_source_id("asknews_wiki", str(url)),
        provider="asknews_wiki",
        title=title,
        url=str(url),
        published="",
        retrieved_at=retrieved_at,
        text=text,
    )


def _dedupe(candidates: Iterable[SourceDocument | None]) -> list[SourceDocument]:
    seen: set[str] = set()
    documents: list[SourceDocument] = []
    for doc in candidates:
        if doc is None or doc.url in seen:
            continue
        seen.add(doc.url)
        documents.append(doc)
    return documents


def fetch_news(
    client: httpx.Client,
    api_key: str,
    question: str,
    news_since: str | None,
    max_articles: int,
    retrieved_at: str,
) -> list[SourceDocument]:
    """Fetch AskNews news articles for `question` (contracts.md B1).

    Runs two queries -- the question title verbatim, and a keyword query
    built from it -- and deduplicates the combined results by url. Each
    article's full text is used if present, else its summary.

    Args:
        client: An `httpx.Client` used for both requests.
        api_key: `ASKNEWS_API_KEY`, sent as a bearer token.
        question: The research question.
        news_since: RFC 3339 UTC timestamp to start from, or `None` to use
            `hours_back=DEFAULT_HOURS_BACK`.
        max_articles: `n_articles` for each query.
        retrieved_at: RFC 3339 UTC timestamp to stamp onto each document.

    Returns:
        Deduplicated documents from both queries. Returns `[]` and logs an
        error for whichever query's request fails; the other query's
        results (if any) are still returned.
    """
    params: dict[str, Any] = {
        "n_articles": max_articles,
        "return_type": "dicts",
        "method": "nl",
        "strategy": "default",
    }
    if news_since is not None:
        params["start_timestamp"] = int(
            datetime.fromisoformat(news_since.replace("Z", "+00:00"))
            .astimezone(UTC)
            .timestamp()
        )
    else:
        params["hours_back"] = DEFAULT_HOURS_BACK

    candidates: list[SourceDocument | None] = []
    for query in (question, _keyword_query(question)):
        try:
            response = client.get(
                NEWS_URL,
                params={**params, "query": query},
                headers=_auth_headers(api_key),
            )
            response.raise_for_status()
            articles = response.json().get("as_dicts", [])
        except httpx.HTTPError as exc:
            logger.error(
                "asknews news search failed for query %r: %s", query, exc, exc_info=True
            )
            continue
        candidates.extend(_news_document(article, retrieved_at) for article in articles)
    return _dedupe(candidates)


def fetch_wiki(
    client: httpx.Client,
    api_key: str,
    question: str,
    max_results: int,
    retrieved_at: str,
) -> list[SourceDocument]:
    """Fetch AskNews wiki results for `question` (contracts.md B1).

    Args:
        client: An `httpx.Client` used for the request.
        api_key: `ASKNEWS_API_KEY`, sent as a bearer token.
        question: The research question, used as the wiki search query.
        max_results: `n_results` for the query.
        retrieved_at: RFC 3339 UTC timestamp to stamp onto each document.

    Returns:
        One `SourceDocument` per deduplicated result. Returns `[]` and
        logs an error if the request fails.
    """
    try:
        response = client.get(
            WIKI_URL,
            params={"query": question, "n_results": max_results},
            headers=_auth_headers(api_key),
        )
        response.raise_for_status()
        results = response.json().get("documents", [])
    except httpx.HTTPError as exc:
        logger.error("asknews wiki search failed: %s", exc, exc_info=True)
        return []
    return _dedupe(_wiki_document(item, retrieved_at) for item in results)


def load_news_fixture(fixture_dir: Path, retrieved_at: str) -> list[SourceDocument]:
    """Load AskNews news documents from `<fixture_dir>/asknews_news.json`.

    The fixture holds one already-merged `{"as_dicts": [...]}` response
    body, standing in for the live path's two deduplicated queries.

    Args:
        fixture_dir: Directory containing `asknews_news.json`.
        retrieved_at: RFC 3339 UTC timestamp to stamp onto each document.

    Returns:
        One `SourceDocument` per deduplicated fixture article.
    """
    data = orjson.loads((fixture_dir / "asknews_news.json").read_bytes())
    articles = data.get("as_dicts", [])
    return _dedupe(_news_document(article, retrieved_at) for article in articles)


def load_wiki_fixture(fixture_dir: Path, retrieved_at: str) -> list[SourceDocument]:
    """Load AskNews wiki documents from `<fixture_dir>/asknews_wiki.json`.

    Args:
        fixture_dir: Directory containing `asknews_wiki.json`.
        retrieved_at: RFC 3339 UTC timestamp to stamp onto each document.

    Returns:
        One `SourceDocument` per deduplicated fixture result.
    """
    data = orjson.loads((fixture_dir / "asknews_wiki.json").read_bytes())
    results = data.get("documents", [])
    return _dedupe(_wiki_document(item, retrieved_at) for item in results)
