"""LLM-driven knowledge-graph extraction from fetched source documents."""

from .llm import Completer
from .schema import ExtractionPayload, Source, content_sha256
from .sources import SourceDocument

SYSTEM_PROMPT = """You are an intelligence analyst extracting a structured knowledge \
graph from a set of source documents, to help answer a research question.

Return a single JSON object with exactly these top-level keys: `entities`, `events`, \
`claims`, `evidence`, `causal_links`, `temporal_relations`. Do not include \
`schema_version`, `question`, or `sources` in your output — the caller supplies those.

Shape of each item:
- entities[]: {"id": "ent:<slug>", "name": str, "kind": one of "person | organization \
| country | technology | policy", "description": str}
- events[]: {"id": "evt:<slug>", "name": str, "occurred_at": "YYYY-MM-DD" or "", \
"description": str, "actor_ids": [entity id, ...]}
- claims[]: {"id": "clm:<slug>", "text": str, "kind": one of "hypothesis | fact | \
assumption", "subject_ids": [entity or event id, ...]}
- evidence[]: {"id": "evd:<slug>", "claim_id": <clm id>, "source_id": <src id>, \
"stance": one of "supports | contradicts", "excerpt": str, "quality": float in \
0.0..1.0}
- causal_links[]: {"cause_id": <clm or evt id>, "effect_id": <clm or evt id>, \
"mechanism": str, "confidence": one of "low | medium | high"}
- temporal_relations[]: {"before_id": <evt id>, "after_id": <evt id>, "relation": one \
of "before | during | after"}

Id rules: every id is a lowercase, hyphen-separated slug prefixed with its list's tag \
(`ent:`, `evt:`, `clm:`, `evd:`), and every id is unique across the whole payload.

Every `evidence[].source_id` must be one of the source ids given to you in the user \
message (the `SOURCE <id> | ...` headers) — never invent a source id. Every `excerpt` \
must be a verbatim quotation copied from the cited source's text, never a paraphrase.

Phrase every `hypothesis` claim so it could turn out to be true or false — never as a \
settled fact. Deliberately search the sources for evidence that both supports and \
contradicts each hypothesis; do not report only confirming evidence.

Never output probabilities, percentages, or predictions."""


def build_user_prompt(question: str, documents: list[SourceDocument]) -> str:
    """Build the user prompt listing the research question and its sources.

    Args:
        question: The research question to answer.
        documents: The normalized source documents available for extraction.

    Returns:
        A prompt with the question followed by each document rendered as
        `=== SOURCE <id> | <provider> | <title> | <url> ===` and its text.
    """
    lines = [f"Question: {question}", ""]
    for doc in documents:
        lines.append(
            f"=== SOURCE {doc.id} | {doc.provider} | {doc.title} | {doc.url} ==="
        )
        lines.append(doc.text)
        lines.append("")
    return "\n".join(lines)


def extract(
    question: str, documents: list[SourceDocument], completer: Completer
) -> ExtractionPayload:
    """Extract a validated, normalized `ExtractionPayload` via the LLM.

    Args:
        question: The research question to answer.
        documents: The normalized source documents available for extraction.
        completer: The chat-completions client (live or fixture-backed).

    Returns:
        A normalized `ExtractionPayload` that has passed `check_integrity`.
        `schema_version`, `question`, and `sources` are always the caller's
        own values; any the model returned are overwritten. Items the
        model returned that violate the contract are repaired or dropped
        by `ExtractionPayload.from_untrusted` rather than failing the
        whole extraction.

    Raises:
        LlmError: If the completer's request or JSON parsing fails.
        ValueError: If `check_integrity` finds a duplicate id or dangling
            reference after normalization.
    """
    raw = completer.complete_json(SYSTEM_PROMPT, build_user_prompt(question, documents))
    sources = [
        Source(
            id=doc.id,
            title=doc.title,
            url=doc.url,
            provider=doc.provider,
            published=doc.published,
            retrieved_at=doc.retrieved_at,
            content=doc.text,
            content_hash=content_sha256(doc.text),
        )
        for doc in documents
    ]
    normalized_payload = ExtractionPayload.from_untrusted(
        {**raw, "schema_version": 2, "question": question, "sources": sources}
    )
    normalized_payload.check_integrity()
    return normalized_payload
