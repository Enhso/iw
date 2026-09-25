"""Tests for iw_research.extract: LLM-driven payload construction."""

from pathlib import Path

import orjson
import pytest

from iw_research.extract import build_user_prompt, extract, extract_batched
from iw_research.llm import Completer, LlmError
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


class _BatchCompleter:
    """A `Completer` dispatching by which document's marker is in the prompt.

    Each document built by `_marker_doc` carries a unique marker in its
    text; the response for a batch is chosen by which marker its prompt
    contains, so each batch can be made to succeed or fail independently
    even though `extract_batched` runs them concurrently.
    """

    def __init__(self, responses: dict[str, dict[str, object] | LlmError]) -> None:
        self._responses = responses

    def complete_json(self, system: str, user: str) -> dict[str, object]:
        for marker, response in self._responses.items():
            if marker in user:
                if isinstance(response, LlmError):
                    raise response
                return response
        raise AssertionError(f"no stub response configured for prompt: {user!r}")


def _marker_doc(doc_id: str, marker: str) -> SourceDocument:
    return SourceDocument(
        id=doc_id,
        provider="wikipedia",
        title=f"Title {marker}",
        url=f"https://example.com/{marker.lower()}",
        published="",
        retrieved_at=RETRIEVED_AT,
        text=f"Some passage text about {marker}.",
    )


def _claim_payload(claim_id: str, source_id: str) -> dict[str, object]:
    return {
        "entities": [],
        "events": [],
        "claims": [
            {"id": claim_id, "text": "A claim", "kind": "fact", "subject_ids": []}
        ],
        "evidence": [
            {
                "id": f"evd:{claim_id.split(':')[1]}",
                "claim_id": claim_id,
                "source_id": source_id,
                "stance": "supports",
                "excerpt": "x",
                "quality": 0.5,
            }
        ],
        "causal_links": [],
        "temporal_relations": [],
    }


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


def test_extract_batched_drops_failing_batch_and_merges_the_rest() -> None:
    docs = [
        _marker_doc("src:wikipedia:0000000000000001", "ALPHA"),
        _marker_doc("src:wikipedia:0000000000000002", "BETA"),
        _marker_doc("src:wikipedia:0000000000000003", "GAMMA"),
    ]
    completer: Completer = _BatchCompleter(
        {
            "ALPHA": _claim_payload("clm:alpha", docs[0].id),
            "BETA": LlmError("chat completion request failed: read timed out"),
            "GAMMA": _claim_payload("clm:gamma", docs[2].id),
        }
    )

    payload = extract_batched(QUESTION, docs, completer, batch_size=1)

    payload.check_integrity()
    assert {c.id for c in payload.claims} == {"clm:alpha", "clm:gamma"}
    assert {s.id for s in payload.sources} == {docs[0].id, docs[2].id}


def test_extract_batched_reraises_first_error_when_all_batches_fail() -> None:
    docs = [
        _marker_doc("src:wikipedia:0000000000000001", "ALPHA"),
        _marker_doc("src:wikipedia:0000000000000002", "BETA"),
        _marker_doc("src:wikipedia:0000000000000003", "GAMMA"),
    ]
    completer: Completer = _BatchCompleter(
        {
            "ALPHA": LlmError("alpha batch failure"),
            "BETA": LlmError("beta batch failure"),
            "GAMMA": LlmError("gamma batch failure"),
        }
    )

    with pytest.raises(LlmError, match="alpha batch failure"):
        extract_batched(QUESTION, docs, completer, batch_size=1)


def test_extract_batched_single_batch_path_still_raises() -> None:
    docs = [_marker_doc("src:wikipedia:0000000000000001", "ALPHA")]
    completer: Completer = _BatchCompleter({"ALPHA": LlmError("single batch failure")})

    with pytest.raises(LlmError, match="single batch failure"):
        extract_batched(QUESTION, docs, completer)
