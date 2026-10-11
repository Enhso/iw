"""Tests for iw_research.sources.bing_news: RSS parsing, bundling, full text."""

import logging
import re
import time
from pathlib import Path
from urllib.parse import quote

import httpx
import pytest
from pytest_httpx import HTTPXMock

from iw_research.sources import bing_news
from iw_research.sources.bing_news import (
    ARTICLE_TIMEOUT,
    MAX_ARTICLE_CHARS,
    MAX_ITEMS,
    MIN_ARTICLE_CHARS,
    SEARCH_URL,
    fetch,
    parse_items,
)

RETRIEVED_AT = "2026-10-09T00:00:00Z"
QUESTION = (
    "Will Donald Trump and Vladimir Putin meet in person before December 1, 2026?"
)
KEYWORDS = "donald trump vladimir putin meet person before december 1 2026"
FIRST_ARTICLE_URL = (
    "https://www.msn.com/en-us/news/other/"
    "trump-invites-putin-to-miami-g20-why-that-matters/ar-AA2cUQzn"
)
LINE_RE = re.compile(r"^(\d{4}-\d{2}-\d{2}|unknown) \| [^|]+ \| .+$")
EMPTY_PAGE = b"<html><head><title>x</title></head><body></body></html>"


def _search_url(query: str) -> httpx.URL:
    return httpx.URL(SEARCH_URL, params={"q": query, "format": "rss", "mkt": "en-US"})


def _feed(*items: str) -> bytes:
    body = "".join(items)
    rss = f'<?xml version="1.0"?><rss version="2.0"><channel>{body}</channel></rss>'
    return rss.encode()


def _item(
    title: str,
    real_url: str | None,
    pub_date: str = "Thu, 24 Sep 2026 04:13:11 GMT",
    description: str = "",
) -> str:
    link = "http://www.bing.com/news/apiclick.aspx?ref=FexRss&amp;aid=&amp;tid=1"
    if real_url is not None:
        link += f"&amp;url={real_url}&amp;c=1&amp;mkt=en-us"
    return (
        f"<item><title>{title}</title><link>{link}</link>"
        f"<description>{description}</description><pubDate>{pub_date}</pubDate></item>"
    )


def _feed_of(*urls: str) -> bytes:
    """A feed with one item per url, titled `T1`, `T2`, ... in order."""
    return _feed(
        *(_item(f"T{n}", quote(url, safe="")) for n, url in enumerate(urls, 1))
    )


def _captured_feed(http_fixture_dir: Path) -> bytes:
    return (http_fixture_dir / "bing_news_rss.xml").read_bytes()


def test_parse_items_takes_the_real_url_from_the_url_parameter(
    http_fixture_dir: Path,
) -> None:
    items = parse_items(_captured_feed(http_fixture_dir))

    assert len(items) == 9
    first = items[0]
    assert first.url == FIRST_ARTICLE_URL
    assert first.domain == "msn.com"
    assert first.date == "2026-09-24"
    assert first.title == "Trump invites Putin to Miami G20: Why that matters"
    assert first.snippet.startswith("US Secretary of State Rubio")
    assert all("bing.com" not in item.url for item in items)
    assert [item.domain for item in items][1:3] == ["theweek.in", "usnews.com"]


def test_parse_items_decodes_the_percent_encoded_url() -> None:
    feed = _feed(_item("T", "https%3a%2f%2fexample.com%2fa%3fid%3d1%26x%3d2"))
    assert parse_items(feed)[0].url == "https://example.com/a?id=1&x=2"


def test_parse_items_skips_unusable_and_repeated_items() -> None:
    feed = _feed(
        _item("No url parameter", None),
        _item("Not http", "ftp%3a%2f%2fexample.com%2fa"),
        _item("", "https%3a%2f%2fexample.com%2fno-title"),
        _item("Kept", "https%3a%2f%2fexample.com%2fa"),
        _item("Repeat of kept", "https%3a%2f%2fexample.com%2fa"),
    )
    assert [item.title for item in parse_items(feed)] == ["Kept"]


def test_parse_items_leaves_the_date_empty_when_unparseable() -> None:
    feed = _feed(_item("T", "https%3a%2f%2fexample.com%2fa", pub_date="soon"))
    assert parse_items(feed)[0].date == ""


def test_fetch_bundles_headlines_and_keeps_the_first_three_articles_that_extract(
    httpx_mock: HTTPXMock, http_fixture_dir: Path
) -> None:
    feed = _captured_feed(http_fixture_dir)
    urls = [item.url for item in parse_items(feed)]
    httpx_mock.add_response(url=_search_url(KEYWORDS), content=feed)
    # Four of the nine results are msn.com and are never requested. Of the five
    # candidates left, the first answers 200 with a captured bot-wall page and
    # the second refuses the request; the captured article page, served by the
    # other three, fills the three slots.
    httpx_mock.add_response(
        url=urls[1],
        content=(http_fixture_dir / "article_access_denied.html").read_bytes(),
    )
    httpx_mock.add_response(url=urls[2], status_code=403)
    for url in urls[6:]:
        httpx_mock.add_response(
            url=url, content=(http_fixture_dir / "article.html").read_bytes()
        )

    with httpx.Client() as client:
        docs = fetch(client, QUESTION, RETRIEVED_AT)

    bundle, *articles = docs
    assert bundle.provider == "bing_news"
    assert bundle.id.startswith("src:bing_news:")
    assert bundle.title == f"Bing News headlines: {KEYWORDS}"
    assert bundle.url == str(_search_url(KEYWORDS))
    assert bundle.published == "2026-09-26"
    lines = bundle.text.split("\n")
    assert len(lines) == 9
    assert all(LINE_RE.match(line) for line in lines)
    assert lines[0].startswith(
        "2026-09-24 | msn.com | Trump invites Putin to Miami G20: Why that matters | "
        "US Secretary of State Rubio"
    )

    assert [a.url for a in articles] == urls[6:]
    article = articles[0]
    assert article.provider == "bing_news"
    assert article.title == "U.S. Invites Putin to G20, Attendance Uncertain"
    assert article.published == "2026-09-26"
    assert "G20" in article.text
    assert "<" not in article.text
    assert MIN_ARTICLE_CHARS < len(article.text) <= MAX_ARTICLE_CHARS
    assert article.id != bundle.id


def test_fetch_requests_the_first_six_non_msn_items_with_a_15_second_timeout(
    httpx_mock: HTTPXMock,
) -> None:
    skipped = ["https://www.msn.com/a", "https://uk.msn.com/b"]
    # `notmsn.com` only ends in "msn.com": it is not an msn.com subdomain.
    candidates = [
        "https://example.com/1",
        "https://example.com/2",
        "https://notmsn.com/3",
        "https://example.com/4",
        "https://example.com/5",
        "https://example.com/6",
    ]
    beyond_the_sixth = "https://example.com/7"
    feed = _feed_of(
        skipped[0], candidates[0], skipped[1], *candidates[1:], beyond_the_sixth
    )
    httpx_mock.add_response(url=_search_url("q"), content=feed)
    for url in candidates:
        httpx_mock.add_response(url=url, content=EMPTY_PAGE)

    with httpx.Client() as client:
        fetch(client, "q", RETRIEVED_AT)

    requests = httpx_mock.get_requests()
    # The articles are fetched in parallel, so only the set of urls is fixed.
    assert sorted(str(r.url) for r in requests[1:]) == sorted(candidates)
    for request in requests:
        assert request.headers["User-Agent"] == "Mozilla/5.0"
    for request in requests[1:]:
        assert request.extensions["timeout"]["read"] == ARTICLE_TIMEOUT == 15.0


def test_fetch_keeps_the_first_three_extractable_articles_in_feed_order(
    httpx_mock: HTTPXMock, http_fixture_dir: Path
) -> None:
    article_page = (http_fixture_dir / "article.html").read_bytes()
    urls = [f"https://example.com/{n}" for n in range(1, 7)]
    httpx_mock.add_response(url=_search_url("q"), content=_feed_of(*urls))

    def slow_response(request: httpx.Request) -> httpx.Response:
        time.sleep(0.2)
        return httpx.Response(200, content=article_page)

    # The first candidate finishes last but still comes first.
    httpx_mock.add_callback(slow_response, url=urls[0])
    for url in urls[1:]:
        httpx_mock.add_response(url=url, content=article_page)

    with httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT)

    assert [d.url for d in docs[1:]] == urls[:3]
    # All six extract, so the last three were fetched and then dropped.
    assert len(httpx_mock.get_requests()) == 1 + len(urls)


def test_fetch_skips_failed_and_short_candidates_without_looking_past_the_sixth(
    httpx_mock: HTTPXMock,
    caplog: pytest.LogCaptureFixture,
    http_fixture_dir: Path,
) -> None:
    article_page = (http_fixture_dir / "article.html").read_bytes()
    bot_wall = (http_fixture_dir / "article_access_denied.html").read_bytes()
    stub = b"<html><body><p>Short stub.</p></body></html>"
    urls = [f"https://example.com/{n}" for n in range(1, 8)]
    httpx_mock.add_response(url=_search_url("q"), content=_feed_of(*urls))
    httpx_mock.add_response(url=urls[0], status_code=403)
    httpx_mock.add_exception(httpx.ReadTimeout("slow article"), url=urls[1])
    httpx_mock.add_response(url=urls[2], content=bot_wall)
    httpx_mock.add_response(url=urls[3], content=article_page)
    httpx_mock.add_response(url=urls[4], content=stub)
    httpx_mock.add_response(url=urls[5], content=article_page)
    # The seventh item is never tried, even though fewer than three extracted.
    with caplog.at_level(logging.WARNING), httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT)

    assert [d.url for d in docs[1:]] == [urls[3], urls[5]]
    assert urls[6] not in {str(r.url) for r in httpx_mock.get_requests()}
    for url in (urls[0], urls[1], urls[2], urls[4]):
        assert any(url in r.getMessage() for r in caplog.records)


def test_fetch_follows_redirects_when_fetching_an_article(
    httpx_mock: HTTPXMock, http_fixture_dir: Path
) -> None:
    feed = _feed(_item("T", "https%3a%2f%2fexample.com%2fold"))
    httpx_mock.add_response(url=_search_url("q"), content=feed)
    httpx_mock.add_response(
        url="https://example.com/old",
        status_code=301,
        headers={"Location": "https://example.com/new"},
    )
    httpx_mock.add_response(
        url="https://example.com/new",
        content=(http_fixture_dir / "article.html").read_bytes(),
    )
    with httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT)

    assert [d.url for d in docs[1:]] == ["https://example.com/old"]
    assert "G20" in docs[1].text


def test_fetch_caps_article_text(httpx_mock: HTTPXMock) -> None:
    paragraphs = "".join(
        f"<p>Paragraph {n}: the summit between the two presidents is discussed "
        "at length, with analysts weighing the odds of an in-person meeting.</p>"
        for n in range(300)
    )
    httpx_mock.add_response(
        url=_search_url("q"), content=_feed(_item("T", "https%3a%2f%2fexample.com%2fa"))
    )
    httpx_mock.add_response(
        url="https://example.com/a",
        content=f"<html><body><article>{paragraphs}</article></body></html>".encode(),
    )
    with httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT)

    assert len(docs[1].text) == MAX_ARTICLE_CHARS


def test_fetch_bundle_holds_at_most_ten_items(httpx_mock: HTTPXMock) -> None:
    items = [_item(f"T{n}", f"https%3a%2f%2fexample.com%2f{n}") for n in range(12)]
    httpx_mock.add_response(url=_search_url("q"), content=_feed(*items))
    httpx_mock.add_response(is_reusable=True, content=EMPTY_PAGE)
    with httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT)

    assert len(docs[0].text.split("\n")) == MAX_ITEMS == 10


def test_fetch_omits_an_empty_snippet_field(httpx_mock: HTTPXMock) -> None:
    feed = _feed(_item("Bare", "https%3a%2f%2fwww.example.com%2fa"))
    httpx_mock.add_response(url=_search_url("q"), content=feed)
    httpx_mock.add_response(url="https://www.example.com/a", content=EMPTY_PAGE)
    with httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT)

    assert docs[0].text == "2026-09-24 | example.com | Bare"


@pytest.mark.parametrize(
    "failure",
    [
        httpx.ReadTimeout("slow article"),
        httpx.ConnectError("refused"),
    ],
)
def test_fetch_skips_an_article_whose_request_fails(
    httpx_mock: HTTPXMock,
    caplog: pytest.LogCaptureFixture,
    failure: httpx.HTTPError,
) -> None:
    feed = _feed(_item("T", "https%3a%2f%2fexample.com%2fa"))
    httpx_mock.add_response(url=_search_url("q"), content=feed)
    httpx_mock.add_exception(failure, url="https://example.com/a")
    with caplog.at_level(logging.WARNING), httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT)

    assert [d.title for d in docs] == ["Bing News headlines: q"]
    assert any("https://example.com/a" in r.getMessage() for r in caplog.records)


def test_fetch_skips_an_article_when_extraction_raises(
    httpx_mock: HTTPXMock,
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
) -> None:
    def explode(*args: object, **kwargs: object) -> str:
        raise ValueError("unparseable page")

    monkeypatch.setattr(bing_news.trafilatura, "extract", explode)
    feed = _feed(_item("T", "https%3a%2f%2fexample.com%2fa"))
    httpx_mock.add_response(url=_search_url("q"), content=feed)
    httpx_mock.add_response(url="https://example.com/a", content=EMPTY_PAGE)
    with caplog.at_level(logging.WARNING), httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT)

    assert [d.title for d in docs] == ["Bing News headlines: q"]
    assert any("extraction failed" in r.getMessage() for r in caplog.records)


@pytest.mark.parametrize(
    "page",
    [EMPTY_PAGE, b"<html><body><p>Short stub.</p></body></html>", None],
    ids=["empty", "stub", "captured-bot-wall"],
)
def test_fetch_skips_an_article_with_too_little_text(
    httpx_mock: HTTPXMock,
    caplog: pytest.LogCaptureFixture,
    http_fixture_dir: Path,
    page: bytes | None,
) -> None:
    if page is None:
        page = (http_fixture_dir / "article_access_denied.html").read_bytes()
    feed = _feed(_item("T", "https%3a%2f%2fexample.com%2fa"))
    httpx_mock.add_response(url=_search_url("q"), content=feed)
    httpx_mock.add_response(url="https://example.com/a", content=page)
    with caplog.at_level(logging.WARNING), httpx.Client() as client:
        docs = fetch(client, "q", RETRIEVED_AT)

    assert len(docs) == 1
    assert any("too little extractable text" in r.getMessage() for r in caplog.records)


def test_fetch_feed_http_error_returns_empty_and_logs(
    httpx_mock: HTTPXMock, caplog: pytest.LogCaptureFixture
) -> None:
    httpx_mock.add_response(url=_search_url("q"), status_code=429)
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


def test_fetch_returns_nothing_for_a_feed_without_items(httpx_mock: HTTPXMock) -> None:
    httpx_mock.add_response(url=_search_url("q"), content=_feed())
    with httpx.Client() as client:
        assert fetch(client, "q", RETRIEVED_AT) == []
    assert len(httpx_mock.get_requests()) == 1
