"""Tests for iw_research.sources.asknews: live fetch and fixture loading."""

import logging
from pathlib import Path

import httpx
import pytest
from pytest_httpx import HTTPXMock

from iw_research.sources.asknews import (
    MAX_N_ARTICLES,
    NEWS_URL,
    WIKI_URL,
    _keyword_query,
    fetch_news,
    fetch_wiki,
    load_news_fixture,
    load_wiki_fixture,
)

RETRIEVED_AT = "2026-09-14T00:00:00Z"


def test_keyword_query_drops_stopwords_and_preserves_order() -> None:
    assert (
        _keyword_query("Will the ECB cut its deposit rate at the October meeting?")
        == "ecb cut deposit rate october meeting"
    )


def test_keyword_query_falls_back_to_question_when_all_stopwords() -> None:
    assert _keyword_query("is it") == "is it"


def test_fetch_news_runs_two_queries_and_dedupes_by_url(httpx_mock: HTTPXMock) -> None:
    httpx_mock.add_response(
        url=httpx.URL(NEWS_URL, params={}).copy_merge_params(
            {
                "query": "export controls",
                "n_articles": "5",
                "return_type": "dicts",
                "method": "nl",
                "strategy": "default",
                "hours_back": "720",
            }
        ),
        json={
            "as_dicts": [
                {
                    "article_url": "https://news.example.com/a",
                    "eng_title": "Article A",
                    "full_text": "Full text A",
                    "pub_date": "2026-09-01T00:00:00Z",
                }
            ]
        },
    )
    httpx_mock.add_response(
        url=httpx.URL(NEWS_URL, params={}).copy_merge_params(
            {
                "query": "export controls",
                "n_articles": "5",
                "return_type": "dicts",
                "method": "nl",
                "strategy": "default",
                "hours_back": "720",
            }
        ),
        json={
            "as_dicts": [
                {
                    "article_url": "https://news.example.com/a",
                    "eng_title": "Duplicate of A",
                    "summary": "Should not appear",
                },
                {
                    "article_url": "https://news.example.com/b",
                    "eng_title": "Article B",
                    "summary": "Summary B only",
                },
            ]
        },
    )
    with httpx.Client() as client:
        docs = fetch_news(client, "key", "export controls", None, 5, RETRIEVED_AT)

    assert {d.url for d in docs} == {
        "https://news.example.com/a",
        "https://news.example.com/b",
    }
    by_url = {d.url: d for d in docs}
    assert by_url["https://news.example.com/a"].text == "Full text A"
    assert by_url["https://news.example.com/a"].title == "Article A"
    assert by_url["https://news.example.com/b"].text == "Summary B only"
    assert all(d.provider == "asknews_news" for d in docs)


def test_fetch_news_clamps_n_articles_to_the_plan_maximum(
    httpx_mock: HTTPXMock,
) -> None:
    # contracts.md A1's default `max_news` (12) exceeds what AskNews' plan
    # accepts (verified live: a 400 above 10), so a request above the plan
    # maximum must still be clamped down to it rather than sent as-is.
    for _ in range(2):  # both queries (literal + keyword) must be clamped
        httpx_mock.add_response(
            url=httpx.URL(NEWS_URL, params={}).copy_merge_params(
                {
                    "query": "export controls",
                    "n_articles": str(MAX_N_ARTICLES),
                    "return_type": "dicts",
                    "method": "nl",
                    "strategy": "default",
                    "hours_back": "720",
                }
            ),
            json={"as_dicts": []},
        )
    with httpx.Client() as client:
        fetch_news(client, "key", "export controls", None, 12, RETRIEVED_AT)
    # `httpx_mock` raises on teardown if a registered response was never matched,
    # so reaching here already proves both requests used the clamped value.


def test_fetch_news_uses_start_timestamp_when_news_since_given(
    httpx_mock: HTTPXMock,
) -> None:
    httpx_mock.add_response(
        url=httpx.URL(NEWS_URL).copy_merge_params(
            {
                "query": "q",
                "n_articles": "3",
                "return_type": "dicts",
                "method": "nl",
                "strategy": "default",
                "start_timestamp": "1767225600",
            }
        ),
        json={"as_dicts": []},
        is_reusable=True,
    )
    with httpx.Client() as client:
        docs = fetch_news(client, "key", "q", "2026-01-01T00:00:00Z", 3, RETRIEVED_AT)
    assert docs == []


def test_fetch_news_sends_bearer_auth(httpx_mock: HTTPXMock) -> None:
    httpx_mock.add_response(
        url=httpx.URL(NEWS_URL).copy_merge_params(
            {
                "query": "q",
                "n_articles": "1",
                "return_type": "dicts",
                "method": "nl",
                "strategy": "default",
                "hours_back": "720",
            }
        ),
        match_headers={"Authorization": "Bearer my-key"},
        json={"as_dicts": []},
        is_reusable=True,
    )
    with httpx.Client() as client:
        fetch_news(client, "my-key", "q", None, 1, RETRIEVED_AT)


def test_fetch_news_one_query_failing_still_returns_the_other(
    httpx_mock: HTTPXMock, caplog: pytest.LogCaptureFixture
) -> None:
    # "q" -> the literal query; keyword query for "q" is also "q" (no stopwords to
    # strip), so both requests hit the same url/params: the first response (error)
    # is consumed by query 1, the second (success) by query 2.
    httpx_mock.add_response(
        url=httpx.URL(NEWS_URL).copy_merge_params(
            {
                "query": "q",
                "n_articles": "1",
                "return_type": "dicts",
                "method": "nl",
                "strategy": "default",
                "hours_back": "720",
            }
        ),
        status_code=500,
    )
    httpx_mock.add_response(
        url=httpx.URL(NEWS_URL).copy_merge_params(
            {
                "query": "q",
                "n_articles": "1",
                "return_type": "dicts",
                "method": "nl",
                "strategy": "default",
                "hours_back": "720",
            }
        ),
        json={
            "as_dicts": [
                {"article_url": "https://news.example.com/x", "eng_title": "X"}
            ]
        },
    )
    with caplog.at_level(logging.ERROR), httpx.Client() as client:
        docs = fetch_news(client, "key", "q", None, 1, RETRIEVED_AT)
    assert len(docs) == 1
    assert any(record.levelno == logging.ERROR for record in caplog.records)


def _news_url(query: str, n_articles: int = 5) -> httpx.URL:
    """The default-window (`hours_back`) news search URL for `query`."""
    return httpx.URL(NEWS_URL).copy_merge_params(
        {
            "query": query,
            "n_articles": str(n_articles),
            "return_type": "dicts",
            "method": "nl",
            "strategy": "default",
            "hours_back": "720",
        }
    )


def _articles(*names: str) -> dict[str, list[dict[str, str]]]:
    """An AskNews `as_dicts` body with one article per name (url `.../<name>`)."""
    return {
        "as_dicts": [
            {
                "article_url": f"https://news.example.com/{name}",
                "eng_title": f"Title {name}",
                "summary": f"Summary {name}",
            }
            for name in names
        ]
    }


def test_fetch_news_runs_extra_queries_and_dedupes_across_all_queries(
    httpx_mock: HTTPXMock,
) -> None:
    # The keyword query for this question is "ecb cut rates".
    httpx_mock.add_response(
        url=_news_url("Will the ECB cut rates?"), json=_articles("a")
    )
    httpx_mock.add_response(url=_news_url("ecb cut rates"), json=_articles("a", "b"))
    httpx_mock.add_response(
        url=_news_url("ecb october statement"), json=_articles("b", "c")
    )
    httpx_mock.add_response(
        url=_news_url("deposit facility rate"), json=_articles("c", "d")
    )
    with httpx.Client() as client:
        docs = fetch_news(
            client,
            "key",
            "Will the ECB cut rates?",
            None,
            5,
            RETRIEVED_AT,
            extra_queries=["ecb october statement", "deposit facility rate"],
        )

    assert [d.url for d in docs] == [
        f"https://news.example.com/{name}" for name in ("a", "b", "c", "d")
    ]
    sent_queries = [r.url.params["query"] for r in httpx_mock.get_requests()]
    assert sent_queries == [
        "Will the ECB cut rates?",
        "ecb cut rates",
        "ecb october statement",
        "deposit facility rate",
    ]


def test_fetch_news_without_extra_queries_runs_only_the_two_default_queries(
    httpx_mock: HTTPXMock,
) -> None:
    httpx_mock.add_response(
        url=_news_url("Will the ECB cut rates?"), json=_articles("a")
    )
    httpx_mock.add_response(url=_news_url("ecb cut rates"), json=_articles("b"))
    with httpx.Client() as client:
        docs = fetch_news(
            client, "key", "Will the ECB cut rates?", None, 5, RETRIEVED_AT, []
        )

    assert len(docs) == 2
    assert len(httpx_mock.get_requests()) == 2


def test_fetch_news_clamps_n_articles_for_extra_queries_too(
    httpx_mock: HTTPXMock,
) -> None:
    for query in ("Will the ECB cut rates?", "ecb cut rates", "targeted follow-up"):
        httpx_mock.add_response(
            url=_news_url(query, n_articles=MAX_N_ARTICLES), json=_articles()
        )
    with httpx.Client() as client:
        fetch_news(
            client,
            "key",
            "Will the ECB cut rates?",
            None,
            12,
            RETRIEVED_AT,
            extra_queries=["targeted follow-up"],
        )
    # `httpx_mock` raises on teardown if a registered response was never matched,
    # so all three requests (both defaults and the extra) used the clamped value.


def test_fetch_news_extra_queries_use_start_timestamp_when_news_since_given(
    httpx_mock: HTTPXMock,
) -> None:
    for query in ("q", "targeted follow-up"):
        httpx_mock.add_response(
            url=httpx.URL(NEWS_URL).copy_merge_params(
                {
                    "query": query,
                    "n_articles": "3",
                    "return_type": "dicts",
                    "method": "nl",
                    "strategy": "default",
                    "start_timestamp": "1767225600",
                }
            ),
            json=_articles(),
            is_reusable=True,
        )
    with httpx.Client() as client:
        fetch_news(
            client,
            "key",
            "q",
            "2026-01-01T00:00:00Z",
            3,
            RETRIEVED_AT,
            extra_queries=["targeted follow-up"],
        )
    extra_request = httpx_mock.get_requests()[-1]
    assert extra_request.url.params["query"] == "targeted follow-up"
    assert "hours_back" not in extra_request.url.params


def test_fetch_news_failing_extra_query_logs_and_keeps_the_other_queries(
    httpx_mock: HTTPXMock, caplog: pytest.LogCaptureFixture
) -> None:
    httpx_mock.add_response(
        url=_news_url("Will the ECB cut rates?"), json=_articles("a")
    )
    httpx_mock.add_response(url=_news_url("ecb cut rates"), json=_articles("a"))
    httpx_mock.add_response(url=_news_url("broken extra"), status_code=500)
    httpx_mock.add_response(url=_news_url("working extra"), json=_articles("w"))
    with caplog.at_level(logging.ERROR), httpx.Client() as client:
        docs = fetch_news(
            client,
            "key",
            "Will the ECB cut rates?",
            None,
            5,
            RETRIEVED_AT,
            extra_queries=["broken extra", "working extra"],
        )

    assert [d.url for d in docs] == [
        "https://news.example.com/a",
        "https://news.example.com/w",
    ]
    errors = [r for r in caplog.records if r.levelno == logging.ERROR]
    assert len(errors) == 1
    assert "broken extra" in errors[0].getMessage()


def test_fetch_wiki_returns_documents(httpx_mock: HTTPXMock) -> None:
    httpx_mock.add_response(
        url=httpx.URL(WIKI_URL).copy_merge_params(
            {"query": "export controls", "n_results": "2"}
        ),
        match_headers={"Authorization": "Bearer key"},
        json={
            "documents": [
                {
                    "title": "Export control",
                    "url": "https://wiki.example.com/export-control",
                    "content": "Wiki content",
                }
            ]
        },
    )
    with httpx.Client() as client:
        docs = fetch_wiki(client, "key", "export controls", 2, RETRIEVED_AT)
    assert len(docs) == 1
    assert docs[0].provider == "asknews_wiki"
    assert docs[0].text == "Wiki content"


def test_fetch_wiki_returns_empty_and_logs_on_http_error(
    httpx_mock: HTTPXMock, caplog: pytest.LogCaptureFixture
) -> None:
    httpx_mock.add_response(
        url=httpx.URL(WIKI_URL).copy_merge_params({"query": "q", "n_results": "1"}),
        status_code=500,
    )
    with caplog.at_level(logging.ERROR), httpx.Client() as client:
        docs = fetch_wiki(client, "key", "q", 1, RETRIEVED_AT)
    assert docs == []
    assert any(record.levelno == logging.ERROR for record in caplog.records)


def test_load_news_fixture_reuses_the_live_parser(fixture_dir: Path) -> None:
    docs = load_news_fixture(fixture_dir, RETRIEVED_AT)
    urls = [d.url for d in docs]
    assert urls == [
        "https://news.example.com/export-controls-update",
        "https://news.example.com/duplicate-url",
    ]
    expected_text = (
        "The full article text about export controls and semiconductor policy updates."
    )
    assert docs[0].text == expected_text
    assert docs[1].text == "Summary only, no content field."
    assert all(d.provider == "asknews_news" for d in docs)


def test_load_wiki_fixture_reuses_the_live_parser(fixture_dir: Path) -> None:
    docs = load_wiki_fixture(fixture_dir, RETRIEVED_AT)
    assert len(docs) == 2
    assert docs[0].text == "Wiki content about export control policy."
    assert docs[1].text == "Only a summary is present here."
    assert all(d.provider == "asknews_wiki" for d in docs)
