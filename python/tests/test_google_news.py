"""Tests for iw_research.sources.google_news: RSS parsing and bundling."""

import logging
import re
from pathlib import Path

import httpx
import pytest
from pytest_httpx import HTTPXMock

from iw_research.sources.google_news import (
    MAX_HEADLINES,
    RSS_URL,
    fetch,
    parse_headlines,
)

RETRIEVED_AT = "2026-10-09T00:00:00Z"
QUESTION = (
    "Will Donald Trump and Vladimir Putin meet in person before December 1, 2026?"
)
KEYWORDS = "donald trump vladimir putin meet person before december 1 2026"
LINE_RE = re.compile(r"^(\d{4}-\d{2}-\d{2}|unknown) \| [^|]+ \| .+$")


def _search_url(query: str) -> httpx.URL:
    return httpx.URL(
        RSS_URL,
        params={
            "q": f"{query} when:30d",
            "hl": "en-US",
            "gl": "US",
            "ceid": "US:en",
        },
    )


def _feed(*items: str) -> bytes:
    body = "".join(items)
    rss = f'<?xml version="1.0"?><rss version="2.0"><channel>{body}</channel></rss>'
    return rss.encode()


def _item(title: str, source: str | None, pub_date: str | None) -> str:
    source_xml = f'<source url="https://x.example">{source}</source>' if source else ""
    date_xml = f"<pubDate>{pub_date}</pubDate>" if pub_date else ""
    return f"<item><title>{title}</title>{date_xml}{source_xml}</item>"


def _numbered_feed(count: int) -> bytes:
    return _feed(
        *(
            _item(
                f"Headline {n} - Pub{n}",
                f"Pub{n}",
                f"Mon, {n:02d} Sep 2026 07:00:00 GMT",
            )
            for n in range(1, count + 1)
        )
    )


def test_parse_headlines_reads_the_captured_feed(http_fixture_dir: Path) -> None:
    content = (http_fixture_dir / "google_news_rss.xml").read_bytes()
    headlines = parse_headlines(content)

    assert len(headlines) == content.count(b"<item>")
    first = headlines[0]
    assert (
        first.title == "US invites Putin to December G20 meeting, possible Trump summit"
    )
    assert first.source == "Reuters"
    assert first.date == "2026-09-23"
    for headline in headlines:
        assert not headline.title.endswith(f" - {headline.source}")
        assert re.fullmatch(r"\d{4}-\d{2}-\d{2}", headline.date)


def test_parse_headlines_strips_only_the_trailing_source_suffix() -> None:
    feed = _feed(_item("Trump - Putin summit: what we know - Reuters", "Reuters", None))
    assert parse_headlines(feed)[0].title == "Trump - Putin summit: what we know"


def test_parse_headlines_without_a_source_element_splits_the_title() -> None:
    feed = _feed(_item("Trump - Putin summit: what we know - Reuters", None, None))
    headline = parse_headlines(feed)[0]
    assert (headline.title, headline.source) == (
        "Trump - Putin summit: what we know",
        "Reuters",
    )


def test_parse_headlines_keeps_a_title_that_lacks_the_source_suffix() -> None:
    headline = parse_headlines(_feed(_item("Plain headline", "Reuters", None)))[0]
    assert (headline.title, headline.source) == ("Plain headline", "Reuters")


def test_parse_headlines_labels_a_missing_source_unknown() -> None:
    headline = parse_headlines(_feed(_item("Plain headline", None, None)))[0]
    assert (headline.title, headline.source) == ("Plain headline", "unknown")


def test_parse_headlines_skips_items_without_a_title() -> None:
    feed = _feed("<item><pubDate>Mon, 01 Sep 2026 07:00:00 GMT</pubDate></item>")
    assert parse_headlines(feed) == []


def test_fetch_bundles_the_captured_feed_into_one_document(
    httpx_mock: HTTPXMock, http_fixture_dir: Path
) -> None:
    httpx_mock.add_response(
        url=_search_url(KEYWORDS),
        content=(http_fixture_dir / "google_news_rss.xml").read_bytes(),
    )
    with httpx.Client() as client:
        docs = fetch(client, QUESTION, RETRIEVED_AT)

    assert len(docs) == 1
    doc = docs[0]
    assert doc.provider == "google_news"
    assert doc.id.startswith("src:google_news:")
    assert doc.title == f"Google News headlines: {KEYWORDS}"
    assert doc.url == str(_search_url(KEYWORDS))
    assert doc.retrieved_at == RETRIEVED_AT
    lines = doc.text.split("\n")
    assert len(lines) == MAX_HEADLINES
    assert all(LINE_RE.match(line) for line in lines)
    dates = [line.split(" | ")[0] for line in lines]
    assert dates == sorted(dates, reverse=True)
    assert doc.published == dates[0]


def test_fetch_sends_a_browser_like_user_agent(httpx_mock: HTTPXMock) -> None:
    httpx_mock.add_response(
        url=_search_url(KEYWORDS),
        match_headers={"User-Agent": "Mozilla/5.0"},
        content=_numbered_feed(1),
    )
    with httpx.Client() as client:
        assert len(fetch(client, QUESTION, RETRIEVED_AT)) == 1


def test_fetch_takes_the_first_twenty_results_then_orders_newest_first(
    httpx_mock: HTTPXMock,
) -> None:
    httpx_mock.add_response(url=_search_url("q"), content=_numbered_feed(25))
    with httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT)

    assert docs[0].text.split("\n") == [
        f"2026-09-{n:02d} | Pub{n} | Headline {n}" for n in range(20, 0, -1)
    ]


def test_fetch_labels_an_undated_headline_unknown_and_sorts_it_last(
    httpx_mock: HTTPXMock,
) -> None:
    feed = _feed(
        _item("No date - Pub", "Pub", None),
        _item("Dated - Pub", "Pub", "Mon, 07 Sep 2026 07:00:00 GMT"),
    )
    httpx_mock.add_response(url=_search_url("q"), content=feed)
    with httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT)

    assert docs[0].text == "2026-09-07 | Pub | Dated\nunknown | Pub | No date"
    assert docs[0].published == "2026-09-07"


def test_fetch_returns_no_document_for_a_feed_without_items(
    httpx_mock: HTTPXMock,
) -> None:
    httpx_mock.add_response(url=_search_url("q"), content=_feed())
    with httpx.Client() as client:
        assert fetch(client, "q", RETRIEVED_AT) == []


def test_fetch_runs_one_query_per_extra_and_skips_repeats(
    httpx_mock: HTTPXMock,
) -> None:
    for query in ("ecb cut rates", "ecb october statement"):
        httpx_mock.add_response(url=_search_url(query), content=_numbered_feed(2))
    with httpx.Client() as client:
        docs = fetch(
            client,
            "Will the ECB cut rates?",
            RETRIEVED_AT,
            extra_queries=["ecb october statement", "ecb cut rates"],
        )

    assert [d.title for d in docs] == [
        "Google News headlines: ecb cut rates",
        "Google News headlines: ecb october statement",
    ]
    assert len({d.id for d in docs}) == 2
    assert len(httpx_mock.get_requests()) == 2


def test_fetch_http_error_returns_empty_and_logs(
    httpx_mock: HTTPXMock, caplog: pytest.LogCaptureFixture
) -> None:
    httpx_mock.add_response(url=_search_url("q"), status_code=503)
    with caplog.at_level(logging.ERROR), httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT)
    assert docs == []
    assert any(record.levelno == logging.ERROR for record in caplog.records)


def test_fetch_malformed_xml_returns_empty_and_logs(
    httpx_mock: HTTPXMock, caplog: pytest.LogCaptureFixture
) -> None:
    httpx_mock.add_response(url=_search_url("q"), content=b"<rss><channel>")
    with caplog.at_level(logging.ERROR), httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT)
    assert docs == []
    assert any("parse failed" in record.getMessage() for record in caplog.records)


def test_fetch_failing_extra_query_keeps_the_other_queries(
    httpx_mock: HTTPXMock, caplog: pytest.LogCaptureFixture
) -> None:
    httpx_mock.add_response(url=_search_url("q"), content=_numbered_feed(1))
    httpx_mock.add_response(url=_search_url("broken"), status_code=500)
    httpx_mock.add_response(url=_search_url("working"), content=_numbered_feed(1))
    with caplog.at_level(logging.ERROR), httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT, extra_queries=["broken", "working"])

    assert [d.title for d in docs] == [
        "Google News headlines: q",
        "Google News headlines: working",
    ]
    errors = [r for r in caplog.records if r.levelno == logging.ERROR]
    assert len(errors) == 1
    assert "broken" in errors[0].getMessage()
