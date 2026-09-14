"""Tests for iw_research.sources.wikipedia: live fetch and fixture loading."""

import logging
from pathlib import Path

import httpx
import pytest
from pytest_httpx import HTTPXMock

from iw_research.sources.wikipedia import (
    WIKI_API_URL,
    load_fixture,
    parse_extract_response,
    parse_search_response,
    search_and_fetch,
)

RETRIEVED_AT = "2026-09-14T00:00:00Z"

_SEARCH_PARAMS = {
    "action": "query",
    "list": "search",
    "srsearch": "export controls",
    "srlimit": "1",
    "format": "json",
}
_EXTRACT_PARAMS = {
    "action": "query",
    "prop": "extracts|info",
    "explaintext": "1",
    "exsectionformat": "plain",
    "inprop": "url",
    "titles": "Foo Bar",
    "format": "json",
}


def test_parse_search_response_extracts_titles() -> None:
    data = {"query": {"search": [{"title": "Foo"}, {"title": "Bar"}]}}
    assert parse_search_response(data) == ["Foo", "Bar"]


def test_parse_extract_response_uses_fullurl_when_present() -> None:
    data = {
        "query": {
            "pages": {
                "1": {
                    "title": "Foo",
                    "extract": "Text here",
                    "fullurl": "https://x/Foo",
                }
            }
        }
    }
    text, url = parse_extract_response(data, "Foo")
    assert text == "Text here"
    assert url == "https://x/Foo"


def test_parse_extract_response_falls_back_to_canonical_url() -> None:
    data = {"query": {"pages": {"1": {"title": "Foo Bar", "extract": "Text"}}}}
    text, url = parse_extract_response(data, "Foo Bar")
    assert text == "Text"
    assert url == "https://en.wikipedia.org/wiki/Foo_Bar"


def test_search_and_fetch_returns_documents_with_expected_fields(
    httpx_mock: HTTPXMock,
) -> None:
    httpx_mock.add_response(
        url=WIKI_API_URL,
        match_params=_SEARCH_PARAMS,
        json={"query": {"search": [{"title": "Foo Bar"}]}},
    )
    httpx_mock.add_response(
        url=WIKI_API_URL,
        match_params=_EXTRACT_PARAMS,
        json={
            "query": {
                "pages": {
                    "1": {
                        "title": "Foo Bar",
                        "extract": "Some extract text.",
                        "fullurl": "https://en.wikipedia.org/wiki/Foo_Bar",
                    }
                }
            }
        },
    )
    with httpx.Client() as client:
        docs = search_and_fetch(client, "export controls", 1, RETRIEVED_AT)

    assert len(docs) == 1
    doc = docs[0]
    assert doc.id == "src:wikipedia-foo-bar"
    assert doc.provider == "wikipedia"
    assert doc.title == "Foo Bar"
    assert doc.url == "https://en.wikipedia.org/wiki/Foo_Bar"
    assert doc.published == ""
    assert doc.retrieved_at == RETRIEVED_AT
    assert doc.text == "Some extract text."


def test_search_and_fetch_returns_empty_and_logs_on_extract_500(
    httpx_mock: HTTPXMock, caplog: pytest.LogCaptureFixture
) -> None:
    httpx_mock.add_response(
        url=WIKI_API_URL,
        match_params=_SEARCH_PARAMS,
        json={"query": {"search": [{"title": "Foo Bar"}]}},
    )
    httpx_mock.add_response(
        url=WIKI_API_URL,
        match_params=_EXTRACT_PARAMS,
        status_code=500,
    )
    with caplog.at_level(logging.ERROR), httpx.Client() as client:
        docs = search_and_fetch(client, "export controls", 1, RETRIEVED_AT)

    assert docs == []
    assert any(record.levelno == logging.ERROR for record in caplog.records)


def test_load_fixture_reuses_the_live_parsers(fixture_dir: Path) -> None:
    docs = load_fixture(fixture_dir, RETRIEVED_AT)
    assert len(docs) == 3
    ids = {doc.id for doc in docs}
    assert ids == {
        "src:wikipedia-semiconductor-industry-in-china",
        "src:wikipedia-united-states-export-controls-on-semiconductors",
        "src:wikipedia-extreme-ultraviolet-lithography",
    }
    for doc in docs:
        assert doc.provider == "wikipedia"
        assert doc.retrieved_at == RETRIEVED_AT
        assert doc.text
