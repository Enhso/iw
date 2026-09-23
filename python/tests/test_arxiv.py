"""Tests for iw_research.sources.arxiv: live search and fixture loading."""

import logging
from pathlib import Path

import httpx
import pytest
from pytest_httpx import HTTPXMock

from iw_research.sources.arxiv import ARXIV_API_URL, load_fixture, parse_atom, search

RETRIEVED_AT = "2026-09-14T00:00:00Z"

_ATOM_FEED = b"""<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <id>http://arxiv.org/abs/9999.00001v1</id>
    <updated>2024-03-12T00:00:00Z</updated>
    <published>2024-03-11T00:00:00Z</published>
    <title>
      A Test Paper Title
    </title>
    <summary>
      A   summary   with   extra   whitespace.
    </summary>
  </entry>
</feed>
"""


def test_parse_atom_extracts_fields_and_collapses_whitespace() -> None:
    docs = parse_atom(_ATOM_FEED, RETRIEVED_AT)
    assert len(docs) == 1
    doc = docs[0]
    assert doc.id == "src:arxiv:b457a59b56dd11ef"
    assert doc.provider == "arxiv"
    assert doc.title == "A Test Paper Title"
    assert doc.text == "A summary with extra whitespace."
    assert doc.url == "http://arxiv.org/abs/9999.00001v1"
    assert doc.retrieved_at == RETRIEVED_AT


def test_parse_atom_slices_published_to_ten_chars() -> None:
    docs = parse_atom(_ATOM_FEED, RETRIEVED_AT)
    assert docs[0].published == "2024-03-11"


def test_parse_atom_skips_entries_missing_title_or_id() -> None:
    feed = b"""<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <id>http://arxiv.org/abs/1v1</id>
  </entry>
  <entry>
    <title>No id here</title>
  </entry>
</feed>
"""
    assert parse_atom(feed, RETRIEVED_AT) == []


def test_search_returns_documents(httpx_mock: HTTPXMock) -> None:
    httpx_mock.add_response(
        url=ARXIV_API_URL,
        match_params={
            "search_query": "all:export controls",
            "start": "0",
            "max_results": "1",
        },
        content=_ATOM_FEED,
    )
    with httpx.Client() as client:
        docs = search(client, "export controls", 1, RETRIEVED_AT)
    assert len(docs) == 1
    assert docs[0].id == "src:arxiv:b457a59b56dd11ef"


def test_search_returns_empty_and_logs_on_http_error(
    httpx_mock: HTTPXMock, caplog: pytest.LogCaptureFixture
) -> None:
    httpx_mock.add_response(
        url=ARXIV_API_URL,
        match_params={"search_query": "all:q", "start": "0", "max_results": "1"},
        status_code=500,
    )
    with caplog.at_level(logging.ERROR), httpx.Client() as client:
        docs = search(client, "q", 1, RETRIEVED_AT)
    assert docs == []
    assert any(record.levelno == logging.ERROR for record in caplog.records)


def test_search_returns_empty_and_logs_on_malformed_xml(
    httpx_mock: HTTPXMock, caplog: pytest.LogCaptureFixture
) -> None:
    httpx_mock.add_response(
        url=ARXIV_API_URL,
        match_params={"search_query": "all:q", "start": "0", "max_results": "1"},
        content=b"<not valid xml",
    )
    with caplog.at_level(logging.ERROR), httpx.Client() as client:
        docs = search(client, "q", 1, RETRIEVED_AT)
    assert docs == []
    assert any(record.levelno == logging.ERROR for record in caplog.records)


def test_load_fixture_reuses_the_live_parser(fixture_dir: Path) -> None:
    docs = load_fixture(fixture_dir, RETRIEVED_AT)
    assert len(docs) == 3
    ids = [doc.id for doc in docs]
    assert ids == [
        "src:arxiv:b457a59b56dd11ef",
        "src:arxiv:a781fa3f3036193a",
        "src:arxiv:65697ea6b9cc8d14",
    ]
    for doc in docs:
        assert doc.provider == "arxiv"
        assert doc.retrieved_at == RETRIEVED_AT
        assert doc.published.count("-") == 2
