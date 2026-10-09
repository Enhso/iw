"""Tests for iw_research.research: the `research` subcommand's orchestration."""

import logging
import re
from collections.abc import Sequence
from pathlib import Path
from typing import Literal

import httpx
import pytest
from pytest_httpx import HTTPXMock

from iw_research import research
from iw_research.request import ResearchRequest
from iw_research.research import _fetch_fixture, _fetch_live, run_research
from iw_research.sources import SourceDocument, asknews, bing_news, google_news
from iw_research.sources.asknews import NEWS_URL
from iw_research.sources.google_news import RSS_URL

ASKNEWS_NEWS_SEARCH = re.compile(re.escape(NEWS_URL) + r"\?.*")
GOOGLE_NEWS_SEARCH = re.compile(re.escape(RSS_URL) + r"\?.*")

QUESTION = (
    "Can export controls durably slow China's access to advanced semiconductor "
    "manufacturing capability?"
)


def test_run_research_fixture_mode_default_providers(fixture_dir: Path) -> None:
    request = ResearchRequest.model_validate({"question": QUESTION})
    payload = run_research(request, fixture_dir)
    assert payload.schema_version == 2
    assert len(payload.sources) == 6
    assert {s.provider for s in payload.sources} == {"wikipedia", "arxiv"}
    assert payload.dropped_sources == []
    assert any(entry.gate == "relevance_filter" for entry in payload.gate_log)
    assert any(entry.gate == "claim_support" for entry in payload.gate_log)


def test_run_research_fixture_mode_explicit_asknews_providers(
    fixture_dir: Path,
) -> None:
    request = ResearchRequest.model_validate(
        {"question": QUESTION, "providers": ["asknews_news", "asknews_wiki"]}
    )
    payload = run_research(request, fixture_dir)
    assert {s.provider for s in payload.sources} == {"asknews_news", "asknews_wiki"}


def test_run_research_every_source_has_matching_content_hash(fixture_dir: Path) -> None:
    request = ResearchRequest.model_validate({"question": QUESTION})
    payload = run_research(request, fixture_dir)
    for source in payload.sources:
        assert source.content  # Source's own validator already checked the hash.


def test_run_research_raises_when_no_documents_are_fetched(fixture_dir: Path) -> None:
    request = ResearchRequest.model_validate({"question": QUESTION, "providers": []})
    with pytest.raises(RuntimeError, match="zero documents"):
        run_research(request, fixture_dir)


def test_fetch_fixture_logs_error_and_skips_unknown_provider(
    fixture_dir: Path, caplog: pytest.LogCaptureFixture
) -> None:
    # `providers` at this internal layer is a plain `list[str]`, not the request
    # model's `Literal`-constrained field, so this path is reachable in practice
    # only by a bug elsewhere; it is tested directly here.
    with caplog.at_level(logging.ERROR):
        documents = _fetch_fixture(
            ["wikipedia", "bing"], fixture_dir, "2026-09-14T00:00:00Z"
        )
    assert {d.provider for d in documents} == {"wikipedia"}
    assert any("unknown provider" in record.message for record in caplog.records)


def test_fetch_live_passes_extra_queries_to_asknews_news(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("ASKNEWS_API_KEY", "key")
    received: list[Sequence[str]] = []

    def fake_fetch_news(
        client: httpx.Client,
        api_key: str,
        question: str,
        max_articles: int,
        retrieved_at: str,
        extra_queries: Sequence[str] = (),
    ) -> list[SourceDocument]:
        received.append(extra_queries)
        return []

    monkeypatch.setattr(asknews, "fetch_news", fake_fetch_news)
    request = ResearchRequest.model_validate(
        {
            "question": QUESTION,
            "providers": ["asknews_news"],
            "extra_queries": ["targeted one", "targeted two"],
        }
    )

    _fetch_live(request, ["asknews_news"], "2026-09-14T00:00:00Z")

    assert received == [["targeted one", "targeted two"]]


def _doc(provider: Literal["google_news", "bing_news"], url: str) -> SourceDocument:
    return SourceDocument(
        id=f"src:{provider}:0123456789abcdef",
        provider=provider,
        title=f"{provider} document",
        url=url,
        published="2026-09-24",
        retrieved_at="2026-09-14T00:00:00Z",
        text=f"Text from {provider}.",
    )


def test_fetch_live_runs_the_free_news_providers(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    received: dict[str, object] = {}

    def fake_google(
        client: httpx.Client,
        question: str,
        retrieved_at: str,
        extra_queries: Sequence[str] = (),
    ) -> list[SourceDocument]:
        received["google"] = (question, list(extra_queries))
        return [_doc("google_news", "https://news.google.com/rss/search?q=a")]

    def fake_bing(
        client: httpx.Client, question: str, retrieved_at: str
    ) -> list[SourceDocument]:
        received["bing"] = question
        return [_doc("bing_news", "https://www.bing.com/news/search?q=a")]

    monkeypatch.setattr(google_news, "fetch", fake_google)
    monkeypatch.setattr(bing_news, "fetch", fake_bing)
    request = ResearchRequest.model_validate(
        {"question": QUESTION, "extra_queries": ["targeted one"]}
    )

    documents = _fetch_live(
        request, ["google_news", "bing_news"], "2026-09-14T00:00:00Z"
    )

    assert [d.provider for d in documents] == ["google_news", "bing_news"]
    assert received == {"google": (QUESTION, ["targeted one"]), "bing": QUESTION}


def test_fetch_fixture_skips_the_news_providers_without_an_error(
    fixture_dir: Path, caplog: pytest.LogCaptureFixture
) -> None:
    with caplog.at_level(logging.INFO):
        documents = _fetch_fixture(
            ["google_news", "bing_news", "wikipedia"],
            fixture_dir,
            "2026-09-14T00:00:00Z",
        )
    assert {d.provider for d in documents} == {"wikipedia"}
    assert not [r for r in caplog.records if r.levelno >= logging.ERROR]


class _RecordingCompleter:
    """A `Completer` that returns an empty extraction and records each prompt."""

    def __init__(self) -> None:
        self.prompts: list[str] = []

    def complete_json(self, system: str, user: str) -> dict[str, object]:
        self.prompts.append(user)
        return {
            "entities": [],
            "events": [],
            "claims": [],
            "evidence": [],
            "causal_links": [],
            "temporal_relations": [],
        }


def _live_research_setup(monkeypatch: pytest.MonkeyPatch) -> _RecordingCompleter:
    completer = _RecordingCompleter()
    monkeypatch.setenv("ASKNEWS_API_KEY", "key")
    monkeypatch.delenv("TYPESAFE_API_KEY", raising=False)
    monkeypatch.setattr(research, "client_from_env", lambda: completer)
    return completer


def test_run_research_succeeds_when_asknews_is_over_quota_but_google_answers(
    monkeypatch: pytest.MonkeyPatch, httpx_mock: HTTPXMock, http_fixture_dir: Path
) -> None:
    completer = _live_research_setup(monkeypatch)
    httpx_mock.add_response(
        url=ASKNEWS_NEWS_SEARCH,
        status_code=402,
        is_reusable=True,
        match_headers={"Authorization": "Bearer key"},
    )
    httpx_mock.add_response(
        url=GOOGLE_NEWS_SEARCH,
        content=(http_fixture_dir / "google_news_rss.xml").read_bytes(),
    )
    request = ResearchRequest.model_validate(
        {"question": "q", "providers": ["asknews_news", "google_news"]}
    )

    payload = run_research(request, None)

    assert any(r.url.host == "api.asknews.app" for r in httpx_mock.get_requests())
    assert {s.provider for s in payload.sources} == {"google_news"}
    assert len(completer.prompts) == 1
    assert "Google News headlines: q" in completer.prompts[0]


def test_run_research_fails_only_when_every_provider_returns_nothing(
    monkeypatch: pytest.MonkeyPatch, httpx_mock: HTTPXMock
) -> None:
    _live_research_setup(monkeypatch)
    httpx_mock.add_response(url=ASKNEWS_NEWS_SEARCH, status_code=402, is_reusable=True)
    httpx_mock.add_response(
        url=GOOGLE_NEWS_SEARCH, content=b'<rss version="2.0"><channel></channel></rss>'
    )
    request = ResearchRequest.model_validate(
        {"question": "q", "providers": ["asknews_news", "google_news"]}
    )

    with pytest.raises(RuntimeError, match="zero documents"):
        run_research(request, None)
