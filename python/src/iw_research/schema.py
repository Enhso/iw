"""The shared `ExtractionPayload` contract and its id/normalization helpers.

Mirrors the Rust `ExtractionPayload` in `src/model.rs` field-for-field so the
JSON this worker prints deserializes directly on the Rust side.
"""

import hashlib
import logging
import re
from collections.abc import Mapping
from typing import Literal, Protocol

from pydantic import BaseModel, Field, ValidationError, model_validator

logger = logging.getLogger(__name__)

_SLUG_COLLAPSE_RE = re.compile(r"[^a-z0-9]+")

_ID_PREFIXES: tuple[str, ...] = ("ent", "evt", "clm", "evd", "src")

_ID_BODY_RE = re.compile(r"[a-z0-9]+(-[a-z0-9]+)*")

# v2 (contracts.md A2): `src:<provider>:<first 16 hex of sha256(url)>`. This is a
# distinct, narrower shape from the Global Constraint 10 slug grammar above, so it
# is checked by `is_valid_source_id`/`make_source_id` rather than folded into
# `is_valid_id`/`make_id` (which stay exactly as Rust's shared contract vectors
# expect for entities, events, claims, and evidence).
_SOURCE_ID_RE = re.compile(r"src:[a-z][a-z0-9_]*:[0-9a-f]{16}")

_FORECASTING_RE = re.compile(
    r"\b(probability|probabilities|likelihood|odds)\b"
    r"|\d+(\.\d+)?\s*%\s*(chance|probability|likelihood|likely)",
    re.IGNORECASE,
)


def is_valid_id(value: str) -> bool:
    """Check `value` against the Global Constraint 10 id shape.

    Mirrors Rust's private `is_valid_id` in `src/model.rs`, checked
    against any of the five known list prefixes (`ent`, `evt`, `clm`,
    `evd`, `src`) rather than one specific list's prefix. A caller that
    cares which list an id belongs to checks
    `value.startswith(f"{prefix}:")` separately.

    Args:
        value: The candidate id.

    Returns:
        True if `value` is `f"{prefix}:{body}"` for one of the five known
        prefixes, at most 80 characters total, where `body` is one or
        more lowercase-alphanumeric segments joined by single hyphens,
        with no leading, trailing, or doubled hyphen.
    """
    if len(value) > 80:
        return False
    for prefix in _ID_PREFIXES:
        body = value.removeprefix(f"{prefix}:")
        if body != value and _ID_BODY_RE.fullmatch(body):
            return True
    return False


def is_valid_date(value: str) -> bool:
    """Check whether `value` is `""` or a `YYYY-MM-DD` date.

    Mirrors Rust's private `validate_date` in `src/model.rs`.

    Args:
        value: The candidate date string.

    Returns:
        True if `value` is `""`, or exactly 10 characters with ASCII
        digits at every position except a literal `-` at positions 4 and
        7.
    """
    if value == "":
        return True
    if len(value) != 10 or value[4] != "-" or value[7] != "-":
        return False
    return all(value[i] in "0123456789" for i in range(10) if i not in (4, 7))


def is_valid_quality(value: float) -> bool:
    """Check whether `value` is a valid evidence quality.

    Mirrors Rust's inline `(0.0..=1.0).contains(&quality)` check in
    `src/model.rs`. `NaN` is never valid, since every comparison against
    `NaN` is false.

    Args:
        value: The candidate quality score.

    Returns:
        True if `0.0 <= value <= 1.0`.
    """
    return 0.0 <= value <= 1.0


def slugify(text: str) -> str:
    """Lowercase `text`, collapse non-alphanumeric runs to `-`, and trim edges.

    Args:
        text: Arbitrary input text.

    Returns:
        A slug matching `[a-z0-9]+(-[a-z0-9]+)*`, or `""` if `text` has no
        alphanumeric characters.
    """
    return _SLUG_COLLAPSE_RE.sub("-", text.lower()).strip("-")


def make_id(prefix: str, text: str) -> str:
    """Build a Global Constraint 10 id from a prefix and free text.

    Args:
        prefix: The id namespace, e.g. `"src"`.
        text: Free text to slugify into the id body.

    Returns:
        `f"{prefix}:{slugify(text)[:60].rstrip('-')}"`.
    """
    return f"{prefix}:{slugify(text)[:60].rstrip('-')}"


def is_valid_source_id(value: str) -> bool:
    """Check `value` against the v2 deterministic source id shape (Item A2).

    Args:
        value: The candidate source id.

    Returns:
        True if `value` is `src:<provider>:<16 lowercase hex digits>`, where
        `provider` is one or more lowercase alphanumeric/underscore
        characters starting with a letter.
    """
    return bool(_SOURCE_ID_RE.fullmatch(value))


def content_sha256(content: str) -> str:
    """Compute the lowercase hex sha256 digest of `content`'s UTF-8 bytes.

    Args:
        content: The exact source content string.

    Returns:
        The 64-character lowercase hex sha256 digest.
    """
    return hashlib.sha256(content.encode("utf-8")).hexdigest()


def make_source_id(provider: str, url: str) -> str:
    """Build a v2 deterministic source id from a provider and url (Item A2).

    Args:
        provider: The source's provider, e.g. `"wikipedia"`.
        url: The source document's url.

    Returns:
        `f"src:{provider}:{first 16 hex digits of sha256(url)}"`.
    """
    digest = hashlib.sha256(url.encode("utf-8")).hexdigest()[:16]
    return f"src:{provider}:{digest}"


class Entity(BaseModel):
    """A named entity, identified by an `ent:` id."""

    id: str
    name: str
    kind: Literal["person", "organization", "country", "technology", "policy"]
    description: str


class Event(BaseModel):
    """A dated occurrence, identified by an `evt:` id."""

    id: str
    name: str
    occurred_at: str
    description: str
    actor_ids: list[str] = Field(default_factory=list)


class Source(BaseModel):
    """A citable source document, identified by a `src:` id.

    `content_hash` must equal `content_sha256(content)`; this is enforced on
    construction so a mismatched pair never survives validation on either
    side of the contract (Item A2).
    """

    id: str
    title: str
    url: str
    provider: Literal["wikipedia", "arxiv", "asknews_news", "asknews_wiki"]
    published: str
    retrieved_at: str
    content: str
    content_hash: str

    @model_validator(mode="after")
    def _check_content_hash(self) -> "Source":
        expected = content_sha256(self.content)
        if self.content_hash != expected:
            raise ValueError(
                f"content_hash {self.content_hash!r} does not match "
                f"sha256(content) {expected!r}"
            )
        return self


class Claim(BaseModel):
    """A proposition about one or more subjects, identified by a `clm:` id."""

    id: str
    text: str
    kind: Literal["hypothesis", "fact", "assumption"]
    subject_ids: list[str] = Field(default_factory=list)
    support: float | None = Field(default=None, ge=0.0, le=1.0)
    support_method: Literal["jev", "none"] = "none"


class Evidence(BaseModel):
    """A source excerpt bearing on a claim, identified by an `evd:` id."""

    id: str
    claim_id: str
    source_id: str
    stance: Literal["supports", "contradicts"]
    excerpt: str
    quality: float


class CausalLink(BaseModel):
    """A causal relationship between a cause and an effect (claim or event)."""

    cause_id: str
    effect_id: str
    mechanism: str
    confidence: Literal["low", "medium", "high"]


class TemporalRelation(BaseModel):
    """A temporal ordering between two events."""

    before_id: str
    after_id: str
    relation: Literal["before", "during", "after"]


class GateLogEntry(BaseModel):
    """One Jev gate invocation's outcome (Item A2/B4)."""

    gate: str
    status: Literal["ok", "failed", "skipped"]
    detail: str


class DroppedSource(BaseModel):
    """A fetched passage the relevance/injection gate removed (Item A2/B4)."""

    url: str
    reason: Literal["irrelevant", "injection"]
    score: float


class _HasId(Protocol):
    """Structural type for the payload's id-bearing list items."""

    @property
    def id(self) -> str: ...


def _dedupe_by_id[T: _HasId](list_name: str, items: list[T]) -> list[T]:
    """Drop items whose id has already been seen, keeping the first.

    Args:
        list_name: The field name `items` came from, used only for the
            duplicate warning.
        items: The items to de-duplicate by `id`.

    Returns:
        `items` with every id after its first occurrence dropped. Each
        drop is logged via `logger.warning`.
    """
    seen: set[str] = set()
    kept: list[T] = []
    for item in items:
        if item.id in seen:
            logger.warning("dropping duplicate %s id %s", list_name, item.id)
            continue
        seen.add(item.id)
        kept.append(item)
    return kept


def _validate_list[M: BaseModel](
    list_name: str, raw_items: object, model: type[M]
) -> list[M]:
    """Validate each item of `raw_items` on its own against `model`.

    Used by `ExtractionPayload.from_untrusted` so one malformed item (an
    unknown enum value, a wrong type) drops only that item instead of
    failing validation of the whole payload.

    Args:
        list_name: The field name `raw_items` came from, used only for
            logging.
        raw_items: The raw value of that field, expected to be a list of
            item mappings.
        model: The pydantic model to validate each item against.

    Returns:
        Every item that validated successfully, in order. A `raw_items`
        that is `None` becomes `[]` silently (an absent field); one that
        is present but not a list becomes `[]` with a warning; an item
        that raises `ValidationError` is dropped with a warning naming
        its id (if it has one) and the validation error.
    """
    if raw_items is None:
        return []
    if not isinstance(raw_items, list):
        logger.warning(
            "dropping non-list field %s (got %s), treating as empty",
            list_name,
            type(raw_items).__name__,
        )
        return []
    kept: list[M] = []
    for item in raw_items:
        try:
            kept.append(model.model_validate(item))
        except ValidationError as exc:
            label = item.get("id", item) if isinstance(item, dict) else item
            logger.warning("dropping invalid %s item %s: %s", list_name, label, exc)
    return kept


def _repair_id(
    list_name: str, prefix: str, old_id: str, remap: dict[str, str]
) -> str | None:
    """Repair one item's own id against Global Constraint 10 (Item C2 step 2).

    Args:
        list_name: The field this id belongs to, used only for logging.
        prefix: That list's id prefix, e.g. `"ent"` for entities.
        old_id: The item's id as received.
        remap: Updated in place with `old_id -> new_id` when a repair
            happens, so reference fields elsewhere in the payload can be
            fixed up to match.

    Returns:
        `old_id` unchanged if it already passes `is_valid_id` and carries
        `prefix`; otherwise a freshly built `make_id(prefix, value)`
        (`value` being the text after `old_id`'s first `:`, or the whole
        string if there is none), or `None` if that repaired slug is
        empty, meaning the caller should drop the item. Every repair or
        drop is logged via `logger.warning`.
    """
    if is_valid_id(old_id) and old_id.startswith(f"{prefix}:"):
        return old_id
    value = old_id.split(":", 1)[1] if ":" in old_id else old_id
    new_id = make_id(prefix, value)
    if new_id == f"{prefix}:":
        logger.warning("dropping %s id %s: repaired slug is empty", list_name, old_id)
        return None
    logger.warning("repairing %s id %s -> %s", list_name, old_id, new_id)
    remap[old_id] = new_id
    return new_id


class ExtractionPayload(BaseModel):
    """A research extraction: one dossier's worth of graph data.

    Mirrors the Rust `ExtractionPayload` in `src/model.rs`.
    """

    schema_version: Literal[2] = 2
    question: str
    entities: list[Entity] = Field(default_factory=list)
    events: list[Event] = Field(default_factory=list)
    sources: list[Source] = Field(default_factory=list)
    claims: list[Claim] = Field(default_factory=list)
    evidence: list[Evidence] = Field(default_factory=list)
    causal_links: list[CausalLink] = Field(default_factory=list)
    temporal_relations: list[TemporalRelation] = Field(default_factory=list)
    gate_log: list[GateLogEntry] = Field(default_factory=list)
    dropped_sources: list[DroppedSource] = Field(default_factory=list)

    @classmethod
    def from_untrusted(cls, raw: Mapping[str, object]) -> "ExtractionPayload":
        """Build a payload from untrusted (LLM-authored) data.

        Repairs or drops, item by item, whatever Rust's
        `ExtractionPayload::validate` would otherwise reject outright, so
        one malformed LLM item never fails a whole live run. Every repair
        or drop is logged via `logger.warning`, naming the list, the id,
        and the reason.

        Runs, in order: (1) per-item model validation for every list
        field, dropping items that raise `ValidationError` (this is also
        where a `Source` with a mismatched `content_hash` is dropped, since
        that check runs in `Source`'s own validator); (2) id repair for
        entities, events, claims, and evidence, re-slugifying under each
        list's own prefix and remapping every `subject_ids`/`actor_ids`/
        `claim_id`/`source_id`/`cause_id`/`effect_id`/`before_id`/
        `after_id` reference to match (sources are worker-authored, not
        LLM-authored, so a source with an invalid v2 id is dropped rather
        than repaired: there is no slug to repair it into); (3) blanking
        `occurred_at`/`published` values that are not `""` or `YYYY-MM-DD`;
        (4) dropping evidence whose `quality` is outside `0.0..=1.0`
        (including `NaN`); (5) dropping claims and causal links whose
        `text`/`mechanism` uses forecasting language, since evidence
        excerpts quote sources verbatim and are exempt; (6) `normalized()`,
        which drops dangling references and de-duplicates by id.

        Args:
            raw: The untrusted payload mapping, e.g. an LLM's parsed JSON
                response merged with the caller's own `schema_version`,
                `question`, `sources`, `gate_log`, and `dropped_sources`
                overrides.

        Returns:
            A normalized `ExtractionPayload` satisfying the same
            contract Rust's `ExtractionPayload::validate` enforces.
        """
        entities = _validate_list("entities", raw.get("entities"), Entity)
        events = _validate_list("events", raw.get("events"), Event)
        sources = _validate_list("sources", raw.get("sources"), Source)
        claims = _validate_list("claims", raw.get("claims"), Claim)
        evidence = _validate_list("evidence", raw.get("evidence"), Evidence)
        causal_links = _validate_list(
            "causal_links", raw.get("causal_links"), CausalLink
        )
        temporal_relations = _validate_list(
            "temporal_relations", raw.get("temporal_relations"), TemporalRelation
        )
        gate_log = _validate_list("gate_log", raw.get("gate_log"), GateLogEntry)
        dropped_sources = _validate_list(
            "dropped_sources", raw.get("dropped_sources"), DroppedSource
        )

        remap: dict[str, str] = {}

        repaired_entities: list[Entity] = []
        for entity in entities:
            new_id = _repair_id("entities", "ent", entity.id, remap)
            if new_id is not None:
                repaired_entities.append(entity.model_copy(update={"id": new_id}))
        entities = repaired_entities

        repaired_events: list[Event] = []
        for event in events:
            new_id = _repair_id("events", "evt", event.id, remap)
            if new_id is not None:
                repaired_events.append(event.model_copy(update={"id": new_id}))
        events = repaired_events

        valid_sources: list[Source] = []
        for source in sources:
            if is_valid_source_id(source.id):
                valid_sources.append(source)
            else:
                logger.warning("dropping sources %s: invalid source id", source.id)
        sources = valid_sources

        repaired_claims: list[Claim] = []
        for claim in claims:
            new_id = _repair_id("claims", "clm", claim.id, remap)
            if new_id is not None:
                repaired_claims.append(claim.model_copy(update={"id": new_id}))
        claims = repaired_claims

        repaired_evidence: list[Evidence] = []
        for item in evidence:
            new_id = _repair_id("evidence", "evd", item.id, remap)
            if new_id is not None:
                repaired_evidence.append(item.model_copy(update={"id": new_id}))
        evidence = repaired_evidence

        events = [
            event.model_copy(
                update={"actor_ids": [remap.get(a, a) for a in event.actor_ids]}
            )
            for event in events
        ]
        claims = [
            claim.model_copy(
                update={"subject_ids": [remap.get(s, s) for s in claim.subject_ids]}
            )
            for claim in claims
        ]
        evidence = [
            item.model_copy(
                update={
                    "claim_id": remap.get(item.claim_id, item.claim_id),
                    "source_id": remap.get(item.source_id, item.source_id),
                }
            )
            for item in evidence
        ]
        causal_links = [
            link.model_copy(
                update={
                    "cause_id": remap.get(link.cause_id, link.cause_id),
                    "effect_id": remap.get(link.effect_id, link.effect_id),
                }
            )
            for link in causal_links
        ]
        temporal_relations = [
            relation.model_copy(
                update={
                    "before_id": remap.get(relation.before_id, relation.before_id),
                    "after_id": remap.get(relation.after_id, relation.after_id),
                }
            )
            for relation in temporal_relations
        ]

        dated_events: list[Event] = []
        for event in events:
            if is_valid_date(event.occurred_at):
                dated_events.append(event)
            else:
                logger.warning(
                    "blanking events %s occurred_at: %r", event.id, event.occurred_at
                )
                dated_events.append(event.model_copy(update={"occurred_at": ""}))
        events = dated_events

        dated_sources: list[Source] = []
        for source in sources:
            if is_valid_date(source.published):
                dated_sources.append(source)
            else:
                logger.warning(
                    "blanking sources %s published: %r", source.id, source.published
                )
                dated_sources.append(source.model_copy(update={"published": ""}))
        sources = dated_sources

        quality_checked_evidence: list[Evidence] = []
        for item in evidence:
            if is_valid_quality(item.quality):
                quality_checked_evidence.append(item)
            else:
                logger.warning(
                    "dropping evidence %s: invalid quality %r", item.id, item.quality
                )
        evidence = quality_checked_evidence

        non_forecasting_claims: list[Claim] = []
        for claim in claims:
            if _FORECASTING_RE.search(claim.text):
                logger.warning(
                    "dropping claims %s: forecasting language in text", claim.id
                )
            else:
                non_forecasting_claims.append(claim)
        claims = non_forecasting_claims

        non_forecasting_links: list[CausalLink] = []
        for link in causal_links:
            if _FORECASTING_RE.search(link.mechanism):
                logger.warning(
                    "dropping causal_links %s->%s: forecasting language in mechanism",
                    link.cause_id,
                    link.effect_id,
                )
            else:
                non_forecasting_links.append(link)
        causal_links = non_forecasting_links

        payload = cls(
            schema_version=raw.get("schema_version", 2),  # type: ignore[arg-type]
            question=raw.get("question", ""),  # type: ignore[arg-type]
            entities=entities,
            events=events,
            sources=sources,
            claims=claims,
            evidence=evidence,
            causal_links=causal_links,
            temporal_relations=temporal_relations,
            gate_log=gate_log,
            dropped_sources=dropped_sources,
        )
        return payload.normalized()

    def normalized(self) -> "ExtractionPayload":
        """De-duplicate by id and drop every dangling reference.

        Returns:
            A new `ExtractionPayload` with every list de-duplicated by id
            (keeping the first occurrence) and every dangling
            `subject_ids`/`actor_ids` entry, `Evidence`, `CausalLink`, and
            `TemporalRelation` referencing an unknown id removed. Each drop
            is logged via `logger.warning`.
        """
        entities = _dedupe_by_id("entities", self.entities)
        events_deduped = _dedupe_by_id("events", self.events)
        sources = _dedupe_by_id("sources", self.sources)
        claims_deduped = _dedupe_by_id("claims", self.claims)
        evidence_deduped = _dedupe_by_id("evidence", self.evidence)

        entity_ids = {item.id for item in entities}
        event_ids = {item.id for item in events_deduped}
        claim_ids = {item.id for item in claims_deduped}
        source_ids = {item.id for item in sources}

        events: list[Event] = []
        for event in events_deduped:
            kept_actor_ids = []
            for actor_id in event.actor_ids:
                if actor_id in entity_ids:
                    kept_actor_ids.append(actor_id)
                else:
                    logger.warning(
                        "dropping actor_id %s: unknown reference %s", event.id, actor_id
                    )
            events.append(event.model_copy(update={"actor_ids": kept_actor_ids}))

        claims: list[Claim] = []
        for claim in claims_deduped:
            kept_subject_ids = []
            for subject_id in claim.subject_ids:
                if subject_id in entity_ids or subject_id in event_ids:
                    kept_subject_ids.append(subject_id)
                else:
                    logger.warning(
                        "dropping subject_id %s: unknown reference %s",
                        claim.id,
                        subject_id,
                    )
            claims.append(claim.model_copy(update={"subject_ids": kept_subject_ids}))

        evidence: list[Evidence] = []
        for item in evidence_deduped:
            if item.claim_id not in claim_ids:
                logger.warning(
                    "dropping evidence %s: unknown reference %s", item.id, item.claim_id
                )
                continue
            if item.source_id not in source_ids:
                logger.warning(
                    "dropping evidence %s: unknown reference %s",
                    item.id,
                    item.source_id,
                )
                continue
            evidence.append(item)

        claim_or_event_ids = claim_ids | event_ids
        causal_links: list[CausalLink] = []
        for link in self.causal_links:
            if link.cause_id not in claim_or_event_ids:
                logger.warning(
                    "dropping causal_link %s->%s: unknown reference %s",
                    link.cause_id,
                    link.effect_id,
                    link.cause_id,
                )
                continue
            if link.effect_id not in claim_or_event_ids:
                logger.warning(
                    "dropping causal_link %s->%s: unknown reference %s",
                    link.cause_id,
                    link.effect_id,
                    link.effect_id,
                )
                continue
            causal_links.append(link)

        temporal_relations: list[TemporalRelation] = []
        for relation in self.temporal_relations:
            if relation.before_id not in event_ids:
                logger.warning(
                    "dropping temporal_relation %s->%s: unknown reference %s",
                    relation.before_id,
                    relation.after_id,
                    relation.before_id,
                )
                continue
            if relation.after_id not in event_ids:
                logger.warning(
                    "dropping temporal_relation %s->%s: unknown reference %s",
                    relation.before_id,
                    relation.after_id,
                    relation.after_id,
                )
                continue
            temporal_relations.append(relation)

        return self.model_copy(
            update={
                "entities": entities,
                "events": events,
                "sources": sources,
                "claims": claims,
                "evidence": evidence,
                "causal_links": causal_links,
                "temporal_relations": temporal_relations,
            }
        )

    def check_integrity(self) -> None:
        """Assert every id is unique and every reference resolves.

        Raises:
            ValueError: On the first duplicate id (checked across
                entities, events, sources, claims, and evidence) or the
                first dangling reference encountered.
        """
        all_ids = (
            [item.id for item in self.entities]
            + [item.id for item in self.events]
            + [item.id for item in self.sources]
            + [item.id for item in self.claims]
            + [item.id for item in self.evidence]
        )
        seen: set[str] = set()
        for id_ in all_ids:
            if id_ in seen:
                raise ValueError(f"duplicate id: {id_}")
            seen.add(id_)

        entity_ids = {item.id for item in self.entities}
        event_ids = {item.id for item in self.events}
        source_ids = {item.id for item in self.sources}
        claim_ids = {item.id for item in self.claims}
        claim_or_event_ids = claim_ids | event_ids

        for event in self.events:
            for actor_id in event.actor_ids:
                if actor_id not in entity_ids:
                    raise ValueError(
                        f"unknown reference in events[].actor_ids: {actor_id}"
                    )

        for claim in self.claims:
            for subject_id in claim.subject_ids:
                if subject_id not in entity_ids and subject_id not in event_ids:
                    raise ValueError(
                        f"unknown reference in claims[].subject_ids: {subject_id}"
                    )

        for item in self.evidence:
            if item.claim_id not in claim_ids:
                raise ValueError(
                    f"unknown reference in evidence[].claim_id: {item.claim_id}"
                )
            if item.source_id not in source_ids:
                raise ValueError(
                    f"unknown reference in evidence[].source_id: {item.source_id}"
                )

        for link in self.causal_links:
            if link.cause_id not in claim_or_event_ids:
                raise ValueError(
                    f"unknown reference in causal_links[].cause_id: {link.cause_id}"
                )
            if link.effect_id not in claim_or_event_ids:
                raise ValueError(
                    f"unknown reference in causal_links[].effect_id: {link.effect_id}"
                )

        for relation in self.temporal_relations:
            if relation.before_id not in event_ids:
                raise ValueError(
                    "unknown reference in temporal_relations[].before_id: "
                    f"{relation.before_id}"
                )
            if relation.after_id not in event_ids:
                raise ValueError(
                    "unknown reference in temporal_relations[].after_id: "
                    f"{relation.after_id}"
                )
