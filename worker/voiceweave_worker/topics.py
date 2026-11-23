"""Hints from the topic of what was just said (a second recognition pass).

The app sends the field glossary as topics: one list of terms per glossary line (planet
phenomena, sky coordinates, constellations, planetarium words, ...). When the first
transcript of a part already contains a term of some topics, the other terms of those
topics are likely in the same speech, misheard as common words (赤経 written as 設計).
Recognizing the part again with them as hints fixed some of these: on synthetic
astronomy speech the terms written right went from 62.1% to 69.7%
(scripts/eval/eval-terms.py, hints "topic120", 2026-10-13). No topic, no second pass.
"""
from __future__ import annotations

# Characters of topic terms added to the hints; 120 did as well as 200 in the measurement.
LIMIT = 120


def parse(value: object) -> list[list[str]]:
    """The topics from a request: a list of lists of non-empty strings, else nothing."""
    if not isinstance(value, list):
        return []
    topics = []
    for line in value[:64]:
        if isinstance(line, list):
            terms = [term.strip() for term in line[:200] if isinstance(term, str) and term.strip()]
            if terms:
                topics.append(terms)
    return topics


def topic_terms(text: str, topics: list[list[str]], known: str = "", limit: int = LIMIT) -> list[str]:
    """Terms to add as hints for `text`: the terms of every topic with a term (two or more
    characters) in `text`, topics with more such terms first, leaving out terms already in
    `text` or in `known` (the hints given already), within `limit` characters."""
    scored = []
    for index, terms in enumerate(topics):
        found = sum(1 for term in terms if len(term) >= 2 and term in text)
        if found:
            scored.append((-found, index))
    out: list[str] = []
    used = 0
    for _, index in sorted(scored):
        for term in topics[index]:
            if term in out or term in text or term in known:
                continue
            if used + len(term) + 1 > limit:
                continue
            out.append(term)
            used += len(term) + 1
    return out


def with_topic(hints: str, terms: list[str], context: str) -> str:
    """The second pass's prompt: the given hints, the topic terms, then the earlier text."""
    head = "、".join(part for part in (hints, "、".join(terms)) if part)
    return head + ("、" if head and context else "") + context
