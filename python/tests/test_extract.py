"""Tests for iw_research.extract: LLM-driven payload construction."""

from pathlib import Path

import orjson

from iw_research.extract import build_user_prompt, extract
from iw_research.llm import Completer
from iw_research.normalize import normalize, to_documents
from iw_research.sources import SourceDocument, arxiv, wikipedia

QUESTION = (
    "Can export controls durably slow China's access to advanced semiconductor "
    "manufacturing capability?"
)
RETRIEVED_AT = "2026-09-14T00:00:00Z"


class _StubCompleter:
    """A `Completer` that always returns the same pre-loaded JSON object."""

    def __init__(self, data: dict[str, object]) -> None:
        self._data = data

    def complete_json(self, system: str, user: str) -> dict[str, object]:
        return self._data


def _fixture_documents(fixture_dir: Path) -> list[SourceDocument]:
    docs = wikipedia.load_fixture(fixture_dir, RETRIEVED_AT) + arxiv.load_fixture(
        fixture_dir, RETRIEVED_AT
    )
    return to_documents(normalize(docs))


def test_extract_builds_payload_with_expected_counts_and_sources(
    fixture_dir: Path,
) -> None:
    raw = orjson.loads((fixture_dir / "llm_extraction.json").read_bytes())
    documents = _fixture_documents(fixture_dir)
    completer: Completer = _StubCompleter(raw)

    payload = extract(QUESTION, documents, completer)

    payload.check_integrity()
    assert len(payload.sources) == len(documents)
    assert {s.id for s in payload.sources} == {d.id for d in documents}
    assert len(payload.entities) == 10
    assert len(payload.events) == 5
    assert len(payload.claims) == 7
    assert len(payload.evidence) == 12
    assert len(payload.causal_links) == 6
    assert len(payload.temporal_relations) == 3


def test_extract_drops_evidence_with_dangling_claim_id(fixture_dir: Path) -> None:
    documents = _fixture_documents(fixture_dir)
    source_id = documents[0].id
    raw: dict[str, object] = {
        "entities": [],
        "events": [],
        "claims": [
            {"id": "clm:real", "text": "A claim", "kind": "fact", "subject_ids": []}
        ],
        "evidence": [
            {
                "id": "evd:orphan",
                "claim_id": "clm:does-not-exist",
                "source_id": source_id,
                "stance": "supports",
                "excerpt": "x",
                "quality": 0.5,
            }
        ],
        "causal_links": [],
        "temporal_relations": [],
    }
    completer: Completer = _StubCompleter(raw)

    payload = extract(QUESTION, documents, completer)

    assert payload.evidence == []
    assert len(payload.claims) == 1


def test_build_user_prompt_lists_each_document_with_a_header(fixture_dir: Path) -> None:
    documents = _fixture_documents(fixture_dir)[:1]
    prompt = build_user_prompt(QUESTION, documents)
    doc = documents[0]
    assert QUESTION in prompt
    assert (
        f"=== SOURCE {doc.id} | {doc.provider} | {doc.title} | {doc.url} ===" in prompt
    )
    assert doc.text in prompt
