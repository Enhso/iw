"""Command-line entry point: fetch, normalize, extract, print one payload."""

import argparse
import logging
import sys
from datetime import UTC, datetime
from pathlib import Path

import httpx
import orjson

from .extract import extract
from .llm import Completer, FixtureChatClient, client_from_env
from .normalize import normalize, to_documents
from .sources import arxiv, wikipedia

logger = logging.getLogger(__name__)


def main(argv: list[str] | None = None) -> int:
    """Run the research pipeline and print one `ExtractionPayload` to stdout.

    Args:
        argv: Command-line arguments, excluding the program name. `None`
            uses `sys.argv[1:]`.

    Returns:
        `0` on success. `1` if both sources returned no documents, or if
        any step raised an exception (logged to stderr in either case).
    """
    logging.basicConfig(level=logging.INFO, stream=sys.stderr)

    parser = argparse.ArgumentParser(prog="iw-research")
    parser.add_argument("--question", required=True)
    parser.add_argument("--fixture-dir", type=Path, default=None)
    parser.add_argument("--max-wikipedia", type=int, default=3)
    parser.add_argument("--max-arxiv", type=int, default=3)
    args = parser.parse_args(argv)

    try:
        completer: Completer
        if args.fixture_dir is not None:
            retrieved_at = "2026-09-14T00:00:00Z"
            wiki_docs = wikipedia.load_fixture(args.fixture_dir, retrieved_at)
            arxiv_docs = arxiv.load_fixture(args.fixture_dir, retrieved_at)
            completer = FixtureChatClient(args.fixture_dir / "llm_extraction.json")
        else:
            retrieved_at = datetime.now(UTC).isoformat(timespec="seconds")
            retrieved_at = retrieved_at.replace("+00:00", "Z")
            with httpx.Client(timeout=30.0) as client:
                wiki_docs = wikipedia.search_and_fetch(
                    client, args.question, args.max_wikipedia, retrieved_at
                )
                arxiv_docs = arxiv.search(
                    client, args.question, args.max_arxiv, retrieved_at
                )
            completer = client_from_env()

        if not wiki_docs and not arxiv_docs:
            logger.error("both sources returned no documents")
            return 1

        frame = normalize(wiki_docs + arxiv_docs)
        documents = to_documents(frame)
        payload = extract(args.question, documents, completer)

        sys.stdout.write(
            orjson.dumps(payload.model_dump(), option=orjson.OPT_INDENT_2).decode()
        )
        sys.stdout.write("\n")
        return 0
    except Exception as exc:
        logger.error("research run failed: %s", exc, exc_info=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
