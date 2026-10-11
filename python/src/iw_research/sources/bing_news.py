"""Bing News RSS search provider: a headline bundle plus a few articles in full."""

import logging
import xml.etree.ElementTree as ET
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from urllib.parse import parse_qs, urlsplit

import httpx
import trafilatura

from ..schema import make_source_id
from . import SourceDocument
from .query import keyword_query
from .rss import USER_AGENT, rss_date

logger = logging.getLogger(__name__)

SEARCH_URL = "https://www.bing.com/news/search"
MAX_ITEMS = 10
MAX_ARTICLES = 3
# Articles tried in parallel to find `MAX_ARTICLES` extractable ones.
MAX_CANDIDATES = 6
# JS-rendered aggregator: its pages never yield article text.
UNEXTRACTABLE_DOMAIN = "msn.com"
ARTICLE_TIMEOUT = 15.0
MAX_ARTICLE_CHARS = 8000
# Bot walls often answer HTTP 200 with a short "Access Denied" page that
# trafilatura happily extracts (~300 characters); real articles run longer.
MIN_ARTICLE_CHARS = 500


@dataclass(frozen=True)
class NewsItem:
    """One Bing News result.

    Attributes:
        url: The article's real url (not Bing's click-tracking redirect).
        date: UTC `YYYY-MM-DD`, or `""` if the item carried no usable date.
        domain: The article url's host without a leading `www.`.
        title: The headline.
        snippet: Bing's description snippet, possibly empty.
    """

    url: str
    date: str
    domain: str
    title: str
    snippet: str


def _real_url(link: str) -> str | None:
    """The article url inside a `bing.com/news/apiclick.aspx?...&url=` link."""
    values = parse_qs(urlsplit(link).query).get("url")
    if not values or urlsplit(values[0]).scheme not in ("http", "https"):
        return None
    return values[0]


def parse_items(content: bytes) -> list[NewsItem]:
    """Parse a Bing News RSS search response into news items.

    Args:
        content: Raw RSS XML bytes.

    Returns:
        One `NewsItem` per distinct article, in feed (relevance) order. The
        article url is the `url` query parameter of the item's tracking link;
        items without one, or whose title is empty, are skipped.

    Raises:
        xml.etree.ElementTree.ParseError: If `content` is not valid XML.
    """
    root = ET.fromstring(content)
    items: list[NewsItem] = []
    seen: set[str] = set()
    for item in root.iterfind("./channel/item"):
        url = _real_url(item.findtext("link") or "")
        title = " ".join((item.findtext("title") or "").split())
        if url is None or url in seen or not title:
            continue
        seen.add(url)
        host = urlsplit(url).hostname or ""
        items.append(
            NewsItem(
                url=url,
                date=rss_date(item.findtext("pubDate")),
                domain=host.removeprefix("www."),
                title=title,
                snippet=" ".join((item.findtext("description") or "").split()),
            )
        )
    return items


def _bundle(
    query: str, url: str, items: list[NewsItem], retrieved_at: str
) -> SourceDocument:
    lines = []
    for item in items[:MAX_ITEMS]:
        fields = [item.date or "unknown", item.domain, item.title, item.snippet]
        lines.append(" | ".join(field for field in fields if field))
    return SourceDocument(
        id=make_source_id("bing_news", url),
        provider="bing_news",
        title=f"Bing News headlines: {query}",
        url=url,
        published=max(item.date for item in items[:MAX_ITEMS]),
        retrieved_at=retrieved_at,
        text="\n".join(lines),
    )


def _article_document(
    client: httpx.Client, item: NewsItem, retrieved_at: str
) -> SourceDocument | None:
    try:
        response = client.get(
            item.url,
            headers={"User-Agent": USER_AGENT},
            timeout=ARTICLE_TIMEOUT,
            follow_redirects=True,
        )
        response.raise_for_status()
    except httpx.HTTPError as exc:
        logger.warning("bing news article fetch failed for %s: %s", item.url, exc)
        return None
    try:
        text = trafilatura.extract(response.content, include_comments=False)
    except Exception as exc:  # trafilatura/lxml raise assorted types on bad pages
        logger.warning("bing news article extraction failed for %s: %s", item.url, exc)
        return None
    if text is None or len(text) < MIN_ARTICLE_CHARS:
        logger.warning(
            "bing news article has too little extractable text (%d chars): %s",
            len(text or ""),
            item.url,
        )
        return None
    return SourceDocument(
        id=make_source_id("bing_news", item.url),
        provider="bing_news",
        title=item.title,
        url=item.url,
        published=item.date,
        retrieved_at=retrieved_at,
        text=text[:MAX_ARTICLE_CHARS],
    )


def _is_extractable_domain(domain: str) -> bool:
    return domain != UNEXTRACTABLE_DOMAIN and not domain.endswith(
        f".{UNEXTRACTABLE_DOMAIN}"
    )


def fetch(
    client: httpx.Client, question: str, retrieved_at: str
) -> list[SourceDocument]:
    """Fetch Bing News results for `question`, with full text for a few.

    Runs the keyword query built from `question`. The result is one document
    bundling the first `MAX_ITEMS` items, one per line as
    `YYYY-MM-DD | <domain> | <title> | <snippet>`, plus up to `MAX_ARTICLES`
    full-text documents extracted with trafilatura (text capped at
    `MAX_ARTICLE_CHARS`).

    The full-text candidates are the feed's first `MAX_CANDIDATES` items in
    Bing's order, skipping `UNEXTRACTABLE_DOMAIN` and its subdomains. They are
    fetched in parallel, each with an `ARTICLE_TIMEOUT` second timeout, so the
    slowest case costs one timeout rather than one per article. The first
    `MAX_ARTICLES` candidates in feed order that yield at least
    `MIN_ARTICLE_CHARS` characters of text are kept; one that cannot be
    fetched or yields less (an error or bot-wall page) is skipped, so fewer
    than `MAX_ARTICLES` may come back.

    Args:
        client: An `httpx.Client` used for the feed and the article requests.
        question: The research question.
        retrieved_at: RFC 3339 UTC timestamp to stamp onto each document.

    Returns:
        The bundle document (url = the RSS request URL) followed by the
        article documents (url = the real article url). Returns `[]` and
        logs an error if the feed request fails or is not valid XML, and
        also if the feed has no items.
    """
    query = keyword_query(question)
    url = httpx.URL(SEARCH_URL, params={"q": query, "format": "rss", "mkt": "en-US"})
    try:
        response = client.get(url, headers={"User-Agent": USER_AGENT})
        response.raise_for_status()
        items = parse_items(response.content)
    except httpx.HTTPError as exc:
        logger.error("bing news search failed: %s", exc, exc_info=True)
        return []
    except ET.ParseError as exc:
        logger.error("bing news response parse failed: %s", exc, exc_info=True)
        return []
    if not items:
        return []

    candidates = [i for i in items if _is_extractable_domain(i.domain)]
    candidates = candidates[:MAX_CANDIDATES]
    with ThreadPoolExecutor(max_workers=max(1, len(candidates))) as pool:
        articles = pool.map(
            lambda item: _article_document(client, item, retrieved_at), candidates
        )
        extracted = [article for article in articles if article is not None]
    return [_bundle(query, str(url), items, retrieved_at), *extracted[:MAX_ARTICLES]]
