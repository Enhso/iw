"""Command-line entry point: subcommands `research` and `classify-family`.

Each subcommand reads one JSON request from stdin and writes one JSON
document to stdout; logs go to stderr. Exit 0 on success, 1 on failure
(contracts.md A).
"""

import argparse
import logging
import sys
from collections.abc import Callable
from pathlib import Path

import orjson

from .family import run_classify_family
from .jev import jev_client_from_env, jev_fixture_client_from_dir
from .llm import Completer, FixtureChatClient, client_from_env
from .request import ClassifyFamilyRequest, ResearchRequest
from .research import run_research

logger = logging.getLogger(__name__)

# The fixture mint-response file for classify-family's minting path, read
# lazily only if minting is actually attempted.
_MINT_FIXTURE_NAME = "llm_family_mint.json"


def _read_stdin_json() -> object:
    """Read and parse stdin as a single JSON document."""
    return orjson.loads(sys.stdin.buffer.read())


def _run_research(fixture_dir: Path | None) -> int:
    try:
        request = ResearchRequest.model_validate(_read_stdin_json())
        payload = run_research(request, fixture_dir)
        sys.stdout.write(
            orjson.dumps(payload.model_dump(), option=orjson.OPT_INDENT_2).decode()
        )
        sys.stdout.write("\n")
        return 0
    except Exception as exc:
        logger.error("research run failed: %s", exc, exc_info=True)
        return 1


def _mint_completer_factory(fixture_dir: Path | None) -> Callable[[], Completer]:
    """Build the thunk `run_classify_family` calls only if minting is attempted."""
    if fixture_dir is not None:
        return lambda: FixtureChatClient(fixture_dir / _MINT_FIXTURE_NAME)
    return client_from_env


def _run_classify_family(fixture_dir: Path | None) -> int:
    try:
        request = ClassifyFamilyRequest.model_validate(_read_stdin_json())
        jev = (
            jev_fixture_client_from_dir(fixture_dir)
            if fixture_dir is not None
            else jev_client_from_env()
        )
        result = run_classify_family(request, jev, _mint_completer_factory(fixture_dir))
        sys.stdout.write(orjson.dumps(result, option=orjson.OPT_INDENT_2).decode())
        sys.stdout.write("\n")
        return 0
    except Exception as exc:
        logger.error("classify-family run failed: %s", exc, exc_info=True)
        return 1


def main(argv: list[str] | None = None) -> int:
    """Parse args, dispatch to the requested subcommand, return its exit code.

    Args:
        argv: Command-line arguments, excluding the program name. `None`
            uses `sys.argv[1:]`.

    Returns:
        `0` on success, `1` on any failure (logged to stderr).
    """
    logging.basicConfig(level=logging.INFO, stream=sys.stderr)

    parser = argparse.ArgumentParser(prog="iw-research")
    subparsers = parser.add_subparsers(dest="command", required=True)

    research_parser = subparsers.add_parser("research")
    research_parser.add_argument("--fixture-dir", type=Path, default=None)

    classify_parser = subparsers.add_parser("classify-family")
    classify_parser.add_argument("--fixture-dir", type=Path, default=None)

    args = parser.parse_args(argv)

    if args.command == "research":
        return _run_research(args.fixture_dir)
    return _run_classify_family(args.fixture_dir)


if __name__ == "__main__":
    raise SystemExit(main())
