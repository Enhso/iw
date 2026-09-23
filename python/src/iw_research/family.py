"""Family routing and minting for the `classify-family` subcommand (A3/A4, B4.1)."""

import logging
from collections.abc import Callable

from .jev import ChoiceQuestion, Jev, JevError
from .llm import Completer, LlmError
from .request import ClassifyFamilyRequest
from .schema import GateLogEntry

logger = logging.getLogger(__name__)

NONE_OPTION = "none"
MATCH_PROBABILITY_THRESHOLD = 0.5  # contracts.md A4/s2

_ROUTING_INSTRUCTIONS = "Which family (if any) does `question` belong to?"
_NONE_DESCRIPTION = "None of the other options describe what this question is about."

_MINT_SYSTEM_PROMPT = """You label recurring forecasting-question families.

Given a question, respond with a single JSON object: {"label": str, "description": \
str}. `label` is 2-6 words, topic-level, e.g. "ECB rate decisions" -- never phrased as \
the specific question itself, e.g. never "ECB October 2026 cut". `description` is one \
sentence describing the family of questions this label covers.

Never output probabilities, percentages, or predictions."""


def _routing_state(request: ClassifyFamilyRequest) -> dict[str, object]:
    state: dict[str, object] = {"question": request.question}
    if request.context is not None:
        context = request.context.model_dump(exclude_none=True)
        if context:
            state["context"] = context
    return state


def _mint_label(
    request: ClassifyFamilyRequest, make_completer: Callable[[], Completer]
) -> tuple[str, str] | None:
    """Mint a new family label + description with the cheap-tier LLM chain.

    Args:
        request: The classify-family request being minted for.
        make_completer: Builds the chat-completions client on demand, so a
            request that matches via Jev never has to construct one (and
            never fails on missing `LLM_API_KEY`/`LLM_MODEL` or a
            denylisted model just to check routing).

    Returns:
        `(label, description)`, or `None` if the client could not be
        built, the request failed, or it returned an empty label or
        description.
    """
    user_prompt = f"Question: {request.question}"
    if request.context is not None and request.context.background:
        user_prompt += f"\nBackground: {request.context.background}"
    try:
        completer = make_completer()
        raw = completer.complete_json(_MINT_SYSTEM_PROMPT, user_prompt)
        label = str(raw["label"]).strip()
        description = str(raw["description"]).strip()
    except (LlmError, KeyError, TypeError) as exc:
        logger.warning("family mint failed: %s", exc)
        return None
    if not label or not description:
        logger.warning("family mint returned an empty label or description")
        return None
    return label, description


def run_classify_family(
    request: ClassifyFamilyRequest,
    jev: Jev | None,
    make_completer: Callable[[], Completer],
) -> dict[str, object]:
    """Classify `request.question` against `request.families` (A4).

    Rule (spec s2): a Jev Choice over the families plus a `none` option.
    Match iff the top option is not `none` and its probability >= 0.5.
    Otherwise mint a new label with the cheap-tier LLM chain. If Jev is
    unavailable, has no families to route against, or errors, skip
    straight to minting (`method: "mint"`). If minting also fails, return
    `{"decision": "none", "method": "failed", ...}` (fail open).

    Args:
        request: The parsed `classify-family` request.
        jev: A configured `Jev`, or `None` if `TYPESAFE_API_KEY` is unset.
        make_completer: Builds the chat-completions client used for
            minting, called only if minting is actually attempted.

    Returns:
        The A4 response dict (JSON-serializable).
    """
    gate_log: list[GateLogEntry] = []
    choice: str | None = None
    probability: float | None = None
    confidence: float | None = None

    if jev is None:
        gate_log.append(
            GateLogEntry(
                gate="family_routing", status="skipped", detail="no TYPESAFE_API_KEY"
            )
        )
    elif not request.families:
        gate_log.append(
            GateLogEntry(
                gate="family_routing", status="skipped", detail="no live families"
            )
        )
    else:
        criteria: dict[str, object | None] = {
            family.id: family.description for family in request.families
        }
        criteria[NONE_OPTION] = _NONE_DESCRIPTION
        try:
            answers = jev.ask(
                _routing_state(request),
                {
                    "family": ChoiceQuestion(
                        instructions=_ROUTING_INSTRUCTIONS, criteria=criteria
                    )
                },
            )
            answer = answers["family"]
            probabilities_raw = answer["probabilities"]
            if not isinstance(probabilities_raw, dict):
                raise TypeError("jev family answer 'probabilities' is not an object")
            choice = str(answer["choice"])
            probability = float(probabilities_raw[choice])
            confidence = float(answer["confidence"])  # type: ignore[arg-type]
            gate_log.append(
                GateLogEntry(
                    gate="family_routing",
                    status="ok",
                    detail=f"top={choice} probability={probability:.2f}",
                )
            )
        except (JevError, KeyError, TypeError, ValueError) as exc:
            gate_log.append(
                GateLogEntry(gate="family_routing", status="failed", detail=str(exc))
            )
            choice = None

    matched = (
        choice is not None
        and choice != NONE_OPTION
        and probability is not None
        and probability >= MATCH_PROBABILITY_THRESHOLD
    )
    if matched:
        return {
            "decision": "matched",
            "family_id": choice,
            "probability": probability,
            "jev_confidence": confidence,
            "method": "jev",
            "gate_log": [entry.model_dump() for entry in gate_log],
        }

    mint_method = "jev+mint" if choice is not None else "mint"
    minted = _mint_label(request, make_completer)
    if minted is None:
        gate_log.append(
            GateLogEntry(gate="family_mint", status="failed", detail="mint failed")
        )
        return {
            "decision": "none",
            "method": "failed",
            "gate_log": [entry.model_dump() for entry in gate_log],
        }

    label, description = minted
    gate_log.append(GateLogEntry(gate="family_mint", status="ok", detail=label))
    return {
        "decision": "minted",
        "label": label,
        "description": description,
        "probability": probability,
        "jev_confidence": confidence,
        "method": mint_method,
        "gate_log": [entry.model_dump() for entry in gate_log],
    }
