"""Tests for iw_research.research: the `research` subcommand's orchestration."""

import logging
from pathlib import Path

import pytest

from iw_research.request import ResearchRequest
from iw_research.research import _fetch_fixture, run_research

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
