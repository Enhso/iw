"""Helpers shared by the RSS-backed news providers."""

from datetime import UTC
from email.utils import parsedate_to_datetime

# Both feeds answer a bare `Mozilla/5.0` (verified live 2026-10-09); Google News
# serves nothing useful to the default `python-httpx` agent string.
USER_AGENT = "Mozilla/5.0"


def rss_date(pub_date: str | None) -> str:
    """Convert an RSS `<pubDate>` (RFC 822) to a UTC `YYYY-MM-DD` date.

    Args:
        pub_date: The `<pubDate>` text, e.g. `"Wed, 23 Sep 2026 07:00:00 GMT"`,
            or `None` if the item has none.

    Returns:
        The date in UTC, or `""` if `pub_date` is missing or unparseable.
    """
    if not pub_date:
        return ""
    try:
        parsed = parsedate_to_datetime(pub_date)
    except (TypeError, ValueError):
        return ""
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=UTC)
    return parsed.astimezone(UTC).date().isoformat()
