"""Tests for iw_research.sources.bing_news: RSS parsing, bundling, full text."""

import logging
import re
from pathlib import Path

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


def test_fetch_bundles_headlines_and_extracts_the_top_three_articles(
    httpx_mock: HTTPXMock, http_fixture_dir: Path
) -> None:
    feed = _captured_feed(http_fixture_dir)
    urls = [item.url for item in parse_items(feed)[:3]]
    httpx_mock.add_response(url=_search_url(KEYWORDS), content=feed)
    # The captured article page is served as the first result's body; the
    # second result answers 200 with a captured bot-wall page, the third
    # refuses the request.
    httpx_mock.add_response(
        url=urls[0], content=(http_fixture_dir / "article.html").read_bytes()
    )
    httpx_mock.add_response(
        url=urls[1],
        content=(http_fixture_dir / "article_access_denied.html").read_bytes(),
    )
    httpx_mock.add_response(url=urls[2], status_code=403)

    with httpx.Client() as client:
        docs = fetch(client, QUESTION, RETRIEVED_AT)

    bundle, article = docs
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

    assert article.provider == "bing_news"
    assert article.url == urls[0] == FIRST_ARTICLE_URL
    assert article.title == "Trump invites Putin to Miami G20: Why that matters"
    assert article.published == "2026-09-24"
    assert "G20" in article.text
    assert "<" not in article.text
    assert MIN_ARTICLE_CHARS < len(article.text) <= MAX_ARTICLE_CHARS
    assert article.id != bundle.id


def test_fetch_requests_only_the_top_three_articles_with_a_15_second_timeout(
    httpx_mock: HTTPXMock, http_fixture_dir: Path
) -> None:
    feed = _captured_feed(http_fixture_dir)
    items = parse_items(feed)
    assert len(items) > 3
    httpx_mock.add_response(url=_search_url(KEYWORDS), content=feed)
    for item in items[:3]:
        httpx_mock.add_response(url=item.url, content=EMPTY_PAGE)

    with httpx.Client() as client:
        fetch(client, QUESTION, RETRIEVED_AT)

    requests = httpx_mock.get_requests()
    assert [str(r.url) for r in requests[1:]] == [item.url for item in items[:3]]
    for request in requests:
        assert request.headers["User-Agent"] == "Mozilla/5.0"
    for request in requests[1:]:
        assert request.extensions["timeout"]["read"] == ARTICLE_TIMEOUT == 15.0


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
