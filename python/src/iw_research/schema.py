"""The shared `ExtractionPayload` contract and its id/normalization helpers.

Mirrors the Rust `ExtractionPayload` in `src/model.rs` field-for-field so the
JSON this worker prints deserializes directly on the Rust side.
"""

import logging
import re
from typing import Literal, Protocol

from pydantic import BaseModel, Field

logger = logging.getLogger(__name__)

_SLUG_COLLAPSE_RE = re.compile(r"[^a-z0-9]+")


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
    """A citable source document, identified by a `src:` id."""

    id: str
    title: str
    url: str
    provider: Literal["wikipedia", "arxiv"]
    published: str
    retrieved_at: str


class Claim(BaseModel):
    """A proposition about one or more subjects, identified by a `clm:` id."""

    id: str
    text: str
    kind: Literal["hypothesis", "fact", "assumption"]
    subject_ids: list[str] = Field(default_factory=list)


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


class _HasId(Protocol):
    """Structural type for the payload's id-bearing list items."""

    @property
    def id(self) -> str: ...


def _dedupe_by_id[T: _HasId](items: list[T]) -> list[T]:
    """Drop items whose id has already been seen, keeping the first."""
    seen: set[str] = set()
    kept: list[T] = []
    for item in items:
        if item.id in seen:
            continue
        seen.add(item.id)
        kept.append(item)
    return kept


class ExtractionPayload(BaseModel):
    """A research extraction: one dossier's worth of graph data.

    Mirrors the Rust `ExtractionPayload` in `src/model.rs`.
    """

    schema_version: Literal[1] = 1
    question: str
    entities: list[Entity] = Field(default_factory=list)
    events: list[Event] = Field(default_factory=list)
    sources: list[Source] = Field(default_factory=list)
    claims: list[Claim] = Field(default_factory=list)
    evidence: list[Evidence] = Field(default_factory=list)
    causal_links: list[CausalLink] = Field(default_factory=list)
    temporal_relations: list[TemporalRelation] = Field(default_factory=list)

    def normalized(self) -> "ExtractionPayload":
        """De-duplicate by id and drop every dangling reference.

        Returns:
            A new `ExtractionPayload` with every list de-duplicated by id
            (keeping the first occurrence) and every dangling
            `subject_ids`/`actor_ids` entry, `Evidence`, `CausalLink`, and
            `TemporalRelation` referencing an unknown id removed. Each drop
            is logged via `logger.warning`.
        """
        entities = _dedupe_by_id(self.entities)
        events_deduped = _dedupe_by_id(self.events)
        sources = _dedupe_by_id(self.sources)
        claims_deduped = _dedupe_by_id(self.claims)
        evidence_deduped = _dedupe_by_id(self.evidence)

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
