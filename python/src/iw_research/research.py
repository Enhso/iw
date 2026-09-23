"""Orchestration for the `research` subcommand (contracts.md A1/A2).

Pipeline: fetch (per requested provider, live or fixture) -> normalize ->
relevance/injection filter -> LLM extraction (on the kept documents only)
-> claim support scoring -> assembled `ExtractionPayload` with every
fetched document restored to `sources[]` (contracts.md A2: dropped
passages are still returned, they just do not feed extraction).
"""

import logging
import os
from datetime import UTC, datetime
from pathlib import Path

import httpx

from .extract import extract_batched
from .gates import filter_sources, score_claim_support
from .jev import Jev, jev_client_from_env, jev_fixture_client_from_dir
from .llm import Completer, FixtureChatClient, client_from_env
from .normalize import normalize, to_documents, to_sources
from .request import ResearchRequest, default_providers
from .schema import ExtractionPayload
from .sources import SourceDocument, arxiv, asknews, wikipedia

logger = logging.getLogger(__name__)

# Matches the fixed timestamp the v1 worker used in fixture mode, so fixture
# output stays deterministic across runs.
FIXTURE_RETRIEVED_AT = "2026-09-14T00:00:00Z"


def _fetch_live(
    request: ResearchRequest, providers: list[str], retrieved_at: str
) -> list[SourceDocument]:
    documents: list[SourceDocument] = []
    with httpx.Client(timeout=30.0) as client:
        for provider in providers:
            if provider == "wikipedia":
                documents += wikipedia.search_and_fetch(
                    client, request.question, request.max_wiki, retrieved_at
                )
            elif provider == "arxiv":
                # No dedicated max_arxiv field in the request contract (A1):
                # arxiv is the no-key default's second provider, so it shares
                # max_wiki's bound with wikipedia.
                documents += arxiv.search(
                    client, request.question, request.max_wiki, retrieved_at
                )
            elif provider in ("asknews_news", "asknews_wiki"):
                api_key = os.environ.get("ASKNEWS_API_KEY")
                if not api_key:
                    logger.error(
                        "provider %s requested but ASKNEWS_API_KEY is unset", provider
                    )
                    continue
                if provider == "asknews_news":
                    documents += asknews.fetch_news(
                        client,
                        api_key,
                        request.question,
                        request.news_since,
                        request.max_news,
                        retrieved_at,
                    )
                else:
                    documents += asknews.fetch_wiki(
                        client,
                        api_key,
                        request.question,
                        request.max_wiki,
                        retrieved_at,
                    )
            else:
                logger.error("unknown provider %r", provider)
    return documents


def _fetch_fixture(
    providers: list[str], fixture_dir: Path, retrieved_at: str
) -> list[SourceDocument]:
    documents: list[SourceDocument] = []
    for provider in providers:
        if provider == "wikipedia":
            documents += wikipedia.load_fixture(fixture_dir, retrieved_at)
        elif provider == "arxiv":
            documents += arxiv.load_fixture(fixture_dir, retrieved_at)
        elif provider == "asknews_news":
            documents += asknews.load_news_fixture(fixture_dir, retrieved_at)
        elif provider == "asknews_wiki":
            documents += asknews.load_wiki_fixture(fixture_dir, retrieved_at)
        else:
            logger.error("unknown provider %r", provider)
    return documents


def run_research(
    request: ResearchRequest, fixture_dir: Path | None
) -> ExtractionPayload:
    """Run the full `research` pipeline for one question (contracts.md A1/A2).

    Args:
        request: The parsed `research` request.
        fixture_dir: An offline fixture directory, or `None` for live mode.

    Returns:
        A validated `ExtractionPayload` (schema_version 2).

    Raises:
        RuntimeError: If every requested provider returned zero documents.
        LlmError: If the LLM extraction request or JSON parsing fails
            (live mode; the fixture chat client never raises this).
        ValueError: If the final payload fails `check_integrity`.
    """
    # `is not None`, not truthiness: an explicit empty list means "no providers",
    # distinct from an omitted field, which falls back to the default (A1).
    providers: list[str] = (
        list(request.providers)
        if request.providers is not None
        else default_providers()
    )

    completer: Completer
    jev: Jev | None
    if fixture_dir is not None:
        retrieved_at = FIXTURE_RETRIEVED_AT
        documents = _fetch_fixture(providers, fixture_dir, retrieved_at)
        completer = FixtureChatClient(fixture_dir / "llm_extraction.json")
        jev = jev_fixture_client_from_dir(fixture_dir)
    else:
        now = datetime.now(UTC).isoformat(timespec="seconds")
        retrieved_at = now.replace("+00:00", "Z")
        documents = _fetch_live(request, providers, retrieved_at)
        completer = client_from_env()
        jev = jev_client_from_env()

    if not documents:
        raise RuntimeError("every requested provider returned zero documents")

    frame = normalize(documents)
    all_documents = to_documents(frame)
    all_sources = to_sources(frame)

    kept_documents, dropped_sources, filter_gate_log = filter_sources(
        all_documents, jev, request.question
    )

    payload = extract_batched(request.question, kept_documents, completer)
    # Restore every fetched document, including ones the relevance filter
    # dropped: the corpus keeps everything fetched, it just doesn't feed
    # extraction (contracts.md A2).
    payload = payload.model_copy(update={"sources": all_sources})

    payload, support_gate_log = score_claim_support(payload, jev)

    payload = payload.model_copy(
        update={
            "gate_log": filter_gate_log + support_gate_log,
            "dropped_sources": dropped_sources,
        }
    )
    payload.check_integrity()
    return payload
