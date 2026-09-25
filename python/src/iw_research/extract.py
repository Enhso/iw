"""LLM-driven knowledge-graph extraction from fetched source documents."""

import logging

from .gates import bounded_map
from .llm import Completer, LlmError
from .schema import (
    CausalLink,
    ExtractionPayload,
    Source,
    TemporalRelation,
    content_sha256,
)
from .sources import SourceDocument

logger = logging.getLogger(__name__)

# ~4 documents per extraction call (contracts.md B4 / build log C2b): a free-tier
# model's output token limit caps how much JSON one call can return, so a question
# with many kept sources is split into batches and merged rather than starved down
# to a handful of claims.
BATCH_SIZE = 4

SYSTEM_PROMPT = """You are a forecasting research analyst extracting a structured \
knowledge graph from a set of source documents, to help a human forecaster answer a \
research question. Your job is to surface concrete, evidence-bearing material, not to \
summarize the question.

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
must be a verbatim quotation copied word-for-word from the cited source's text, never \
a paraphrase or summary — copy the exact substring.

What makes a good claim: a claim is a single, concrete, evidence-bearing statement --
a dated fact, a figure or statistic, a scheduled event, historical base-rate data \
(e.g. how often a comparable past event occurred, or what happened the last several \
times), or a stated position or action of a named actor. A claim is never a \
restatement or paraphrase of the research question itself, and never a vague summary \
("there is uncertainty about X") -- extract the specific facts underneath that \
uncertainty instead. Prefer several narrow claims over one broad one. Extract roughly \
8 to 25 claims in total (fewer only if the sources genuinely do not support that \
many), drawing on as many of the given sources as they support. Every claim must have \
at least one evidence[] item citing it (matching `claim_id`); do not emit a claim you \
cannot back with a verbatim excerpt.

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


def _dedupe_causal_links(links: list[CausalLink]) -> list[CausalLink]:
    """Drop exact-duplicate causal links, keeping the first occurrence.

    `CausalLink` carries no `id` field, so `ExtractionPayload.normalized()`'s
    id-based dedup never touches it; overlapping batches can otherwise derive
    the identical `(cause_id, effect_id)` link twice.
    """
    seen: set[tuple[str, str, str, str]] = set()
    kept: list[CausalLink] = []
    for link in links:
        key = (link.cause_id, link.effect_id, link.mechanism, link.confidence)
        if key not in seen:
            seen.add(key)
            kept.append(link)
    return kept


def _dedupe_temporal_relations(
    relations: list[TemporalRelation],
) -> list[TemporalRelation]:
    """Drop exact-duplicate temporal relations, keeping the first occurrence.

    Same rationale as `_dedupe_causal_links`: `TemporalRelation` has no `id`.
    """
    seen: set[tuple[str, str, str]] = set()
    kept: list[TemporalRelation] = []
    for relation in relations:
        key = (relation.before_id, relation.after_id, relation.relation)
        if key not in seen:
            seen.add(key)
            kept.append(relation)
    return kept


def _merge_payloads(
    question: str, payloads: list[ExtractionPayload]
) -> ExtractionPayload:
    """Concatenate several payloads' lists and re-normalize as one payload.

    Each input payload was already produced by `extract` from its own batch of
    documents, so it is already internally consistent (its own ids repaired, its
    own dangling references dropped). Concatenating and re-running `normalized()`
    de-duplicates ids that recur across batches (an LLM routinely re-derives the
    same slug for the same real-world entity or a re-cited source) and drops any
    reference that still dangles once the lists are combined. `causal_links` and
    `temporal_relations` carry no `id` for `normalized()` to dedupe by, so exact
    duplicates across batches are removed separately, by full content.

    Args:
        question: The research question, carried onto the merged payload.
        payloads: One `ExtractionPayload` per batch, in any order.

    Returns:
        A single normalized `ExtractionPayload` combining every batch.
    """
    merged = ExtractionPayload(
        question=question,
        entities=[e for p in payloads for e in p.entities],
        events=[e for p in payloads for e in p.events],
        sources=[s for p in payloads for s in p.sources],
        claims=[c for p in payloads for c in p.claims],
        evidence=[e for p in payloads for e in p.evidence],
        causal_links=_dedupe_causal_links(
            [link for p in payloads for link in p.causal_links]
        ),
        temporal_relations=_dedupe_temporal_relations(
            [t for p in payloads for t in p.temporal_relations]
        ),
    )
    return merged.normalized()


def _extract_batch_or_error(
    question: str, batch: list[SourceDocument], completer: Completer, index: int
) -> ExtractionPayload | LlmError | ValueError:
    """Run `extract` on one batch, catching its documented exceptions.

    Args:
        question: The research question to answer.
        batch: This batch's documents.
        completer: The chat-completions client (live or fixture-backed).
        index: This batch's position among `extract_batched`'s batches,
            used only for the failure log line.

    Returns:
        The batch's `ExtractionPayload` on success, or the caught
        `LlmError`/`ValueError` on failure (never raised here, so a
        `bounded_map` over this function cannot fail the whole call).
    """
    try:
        return extract(question, batch, completer)
    except (LlmError, ValueError) as exc:
        logger.warning(
            "extract_batched: batch %d (%d documents) failed: %s: %s",
            index,
            len(batch),
            type(exc).__name__,
            exc,
        )
        return exc


def extract_batched(
    question: str,
    documents: list[SourceDocument],
    completer: Completer,
    batch_size: int = BATCH_SIZE,
) -> ExtractionPayload:
    """Extract a payload from `documents`, splitting into concurrent batches.

    A free-tier model's completion has a limited output token budget, which
    caps how many entities/claims/evidence one `extract` call can return
    regardless of how much source material it is given. Splitting `documents`
    into batches of `batch_size` and running one `extract` call per batch
    (concurrently) gives each batch's material its own output budget; the
    per-batch payloads are then merged and re-normalized by `_merge_payloads`.

    A batch that raises `LlmError` or `ValueError` is logged (batch index,
    document count, exception class and message -- never document content)
    and dropped rather than failing the whole call, so one batch's LLM
    timeout or malformed output does not starve the other batches' claims.
    The single-batch path (`len(documents) <= batch_size`) is unaffected and
    still raises on failure.

    Args:
        question: The research question to answer.
        documents: The normalized source documents available for extraction.
        completer: The chat-completions client (live or fixture-backed).
        batch_size: Maximum documents per `extract` call.

    Returns:
        A single normalized `ExtractionPayload` merging every batch that
        succeeded (see `_merge_payloads`), or the empty payload if
        `documents` is empty.

    Raises:
        LlmError: If `len(documents) <= batch_size` and the single `extract`
            call's completer request or JSON parsing fails, or if every
            batch failed with `LlmError` (the first batch's error is
            re-raised).
        ValueError: If `len(documents) <= batch_size` and the single
            `extract` call's `check_integrity` fails, or if every batch
            failed with `ValueError` (the first batch's error is
            re-raised).
    """
    if not documents:
        return ExtractionPayload(question=question)
    if len(documents) <= batch_size:
        return extract(question, documents, completer)

    batches = [
        documents[i : i + batch_size] for i in range(0, len(documents), batch_size)
    ]
    results = bounded_map(
        lambda pair: _extract_batch_or_error(question, pair[1], completer, pair[0]),
        list(enumerate(batches)),
    )
    payloads: list[ExtractionPayload] = []
    first_error: LlmError | ValueError | None = None
    for result in results:
        if isinstance(result, ExtractionPayload):
            payloads.append(result)
        elif first_error is None:
            first_error = result
    if not payloads:
        assert first_error is not None  # every batch failed -> at least one error
        raise first_error
    return _merge_payloads(question, payloads)
