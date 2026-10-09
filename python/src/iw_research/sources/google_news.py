"""Google News RSS search provider: one bundled headline document per query."""

import logging
import xml.etree.ElementTree as ET
from collections.abc import Sequence
from dataclasses import dataclass

import httpx

from ..schema import make_source_id
from . import SourceDocument
from .query import keyword_query
from .rss import USER_AGENT, rss_date

logger = logging.getLogger(__name__)

RSS_URL = "https://news.google.com/rss/search"
LOOKBACK = "30d"
MAX_HEADLINES = 20


@dataclass(frozen=True)
class Headline:
    """One Google News result.

    Attributes:
        date: UTC `YYYY-MM-DD`, or `""` if the item carried no usable date.
        source: The publisher's name, e.g. `"Reuters"`.
        title: The headline, without the trailing ` - <source>` Google appends.
    """

    date: str
    source: str
    title: str


def parse_headlines(content: bytes) -> list[Headline]:
    """Parse a Google News RSS search response into headlines.

    Args:
        content: Raw RSS XML bytes.

    Returns:
        One `Headline` per `<item>` that has a title, in feed order. The
        publisher comes from the item's `<source>` element; if that is
        missing it is taken from the title's trailing ` - <source>`.

    Raises:
        xml.etree.ElementTree.ParseError: If `content` is not valid XML.
    """
    root = ET.fromstring(content)
    headlines = []
    for item in root.iterfind("./channel/item"):
        raw_title = " ".join((item.findtext("title") or "").split())
        if not raw_title:
            continue
        source = " ".join((item.findtext("source") or "").split())
        suffix = f" - {source}"
        if source and raw_title.endswith(suffix):
            title = raw_title[: -len(suffix)]
        elif not source and " - " in raw_title:
            title, _, source = raw_title.rpartition(" - ")
        else:
            title = raw_title
        headlines.append(
            Headline(rss_date(item.findtext("pubDate")), source or "unknown", title)
        )
    return headlines


def _bundle(
    query: str, url: str, headlines: list[Headline], retrieved_at: str
) -> SourceDocument:
    newest_first = sorted(headlines[:MAX_HEADLINES], key=lambda h: h.date, reverse=True)
    text = "\n".join(
        f"{h.date or 'unknown'} | {h.source} | {h.title}" for h in newest_first
    )
    return SourceDocument(
        id=make_source_id("google_news", url),
        provider="google_news",
        title=f"Google News headlines: {query}",
        url=url,
        published=newest_first[0].date,
        retrieved_at=retrieved_at,
        text=text,
    )


def fetch(
    client: httpx.Client,
    question: str,
    retrieved_at: str,
    extra_queries: Sequence[str] = (),
) -> list[SourceDocument]:
    """Fetch Google News headlines for `question` (last `LOOKBACK`).

    Runs the keyword query built from `question` plus one query per entry of
    `extra_queries` (verbatim). Each query yields at most one document that
    bundles its first `MAX_HEADLINES` results, one headline per line as
    `YYYY-MM-DD | <source> | <headline>`, newest first. One bundle per query
    keeps the LLM extraction cost flat however many headlines come back.

    Args:
        client: An `httpx.Client` used for every request.
        question: The research question.
        retrieved_at: RFC 3339 UTC timestamp to stamp onto each document.
        extra_queries: Additional targeted search queries (contracts.md A1).

    Returns:
        One `SourceDocument` per query that returned at least one headline;
        its url is the RSS request URL. A query that fails or returns nothing
        contributes no document, and a failure is logged without affecting
        the other queries.
    """
    documents = []
    for query in dict.fromkeys((keyword_query(question), *extra_queries)):
        url = httpx.URL(
            RSS_URL,
            params={
                "q": f"{query} when:{LOOKBACK}",
                "hl": "en-US",
                "gl": "US",
                "ceid": "US:en",
            },
        )
        try:
            response = client.get(url, headers={"User-Agent": USER_AGENT})
            response.raise_for_status()
            headlines = parse_headlines(response.content)
        except httpx.HTTPError as exc:
            logger.error(
                "google news search failed for query %r: %s", query, exc, exc_info=True
            )
            continue
        except ET.ParseError as exc:
            logger.error(
                "google news response parse failed for query %r: %s",
                query,
                exc,
                exc_info=True,
            )
            continue
        if headlines:
            documents.append(_bundle(query, str(url), headlines, retrieved_at))
    return documents
