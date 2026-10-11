"""Tests for iw_research.sources.rss: shared RSS helpers."""

import pytest

from iw_research.sources.rss import rss_date


def test_rss_date_formats_a_gmt_pub_date() -> None:
    assert rss_date("Wed, 23 Sep 2026 07:00:00 GMT") == "2026-09-23"


def test_rss_date_converts_other_offsets_to_utc() -> None:
    assert rss_date("Wed, 23 Sep 2026 01:30:00 +0200") == "2026-09-22"


def test_rss_date_treats_a_naive_date_as_utc() -> None:
    assert rss_date("Wed, 23 Sep 2026 07:00:00 -0000") == "2026-09-23"


@pytest.mark.parametrize("value", [None, "", "yesterday-ish", "not a date at all"])
def test_rss_date_returns_empty_for_missing_or_unparseable(value: str | None) -> None:
    assert rss_date(value) == ""
