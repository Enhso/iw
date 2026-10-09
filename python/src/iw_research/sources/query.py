"""Keyword-query construction shared by the news providers."""

import re

_STOPWORDS = frozenset(
    """
    a an the is are was were will would can could should of in on at to for and
    or but with by from that this these those it its as be been being do does
    did if than then so such not no yes has have had which who whom what when
    where why how will
    """.split()
)
_WORD_RE = re.compile(r"[A-Za-z0-9']+")


def keyword_query(question: str) -> str:
    """Build a keyword query from `question` (contracts.md B1's second query).

    Args:
        question: The research question.

    Returns:
        `question`'s words, lowercased, with common stopwords removed and
        order preserved. Falls back to `question` itself if every word is
        a stopword.
    """
    words = [w for w in _WORD_RE.findall(question.lower()) if w not in _STOPWORDS]
    return " ".join(words) if words else question
