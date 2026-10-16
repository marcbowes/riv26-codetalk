"""Inline markdown -> styled spans.

A span is (text, flags) where flags is a string containing any of
  'b' bold, 'i' italic, 'c' code.
Links keep their text and drop the URL. Unknown or unbalanced markup is
kept literally: this never raises.
"""
from __future__ import annotations

import html
import re
import unicodedata
from typing import List, Tuple

Span = Tuple[str, str]

_ESCAPABLE = set("\\`*_{}[]()#+-.!|<>~\"'")
# Inline HTML we strip (tags only). Anything else in <...> stays, so
# placeholders like <cluster-A-id> survive.
_TAGS = (
    "b|i|em|strong|br|sub|sup|kbd|code|span|a|img|summary|details|p|small|u|s|del|ins|mark|abbr"
)
_TAG_RE = re.compile(r"</?(?:%s)(?:\s[^<>]*)?/?>" % _TAGS, re.I)
_AUTOLINK_RE = re.compile(r"<((?:https?|mailto):[^<>\s]+)>")


def _flags(base: str, add: str) -> str:
    return "".join(sorted(set(base) | set(add)))


def _skip_code(s: str, i: int) -> int:
    """If s[i] starts a backtick run with a matching closer, return the index
    just past the closer; otherwise return i + len(run)."""
    j = i
    while j < len(s) and s[j] == "`":
        j += 1
    run = j - i
    k = j
    while True:
        k = s.find("`" * run, k)
        if k == -1:
            return i + run
        end = k + run
        if end < len(s) and s[end] == "`":  # longer run, keep looking
            while end < len(s) and s[end] == "`":
                end += 1
            k = end
            continue
        if k > 0 and s[k - 1] == "`":
            k = end
            continue
        return end


def _find_closer(s: str, delim: str, start: int) -> int:
    """Index of the next delim in s at or after start, skipping code spans
    and escapes. For single-char delims, a closer must not be preceded by
    whitespace and not be part of a double delimiter."""
    i = start
    n = len(s)
    while i < n:
        ch = s[i]
        if ch == "\\":
            i += 2
            continue
        if ch == "`":
            i = _skip_code(s, i)
            continue
        if s.startswith(delim, i):
            if len(delim) == 1:
                prev = s[i - 1] if i > 0 else " "
                nxt = s[i + 1] if i + 1 < n else " "
                if nxt == delim:  # part of ** / __, skip both
                    i += 2
                    continue
                if prev.isspace():
                    i += 1
                    continue
                if delim == "_" and nxt.isalnum():
                    i += 1
                    continue
                return i
            if i > start:
                return i
        i += 1
    return -1


def _match_bracket(s: str, i: int, open_ch: str, close_ch: str) -> int:
    """s[i] == open_ch. Return the index of the matching close_ch or -1."""
    depth = 0
    j = i
    n = len(s)
    while j < n:
        ch = s[j]
        if ch == "\\":
            j += 2
            continue
        if ch == "`":
            j = _skip_code(s, j)
            continue
        if ch == open_ch:
            depth += 1
        elif ch == close_ch:
            depth -= 1
            if depth == 0:
                return j
        j += 1
    return -1


def parse(s: str, flags: str = "") -> List[Span]:
    """Parse inline markdown into a list of (text, flags) spans."""
    try:
        return _merge(_parse(s or "", flags, 0))
    except Exception:  # never crash on odd markup
        return [(s or "", flags)]


def _parse(s: str, flags: str, depth: int) -> List[Span]:
    if depth > 20:
        return [(s, flags)]
    out: List[Span] = []
    buf: List[str] = []
    n = len(s)
    i = 0

    def flush() -> None:
        if buf:
            out.append((html.unescape("".join(buf)), flags))
            buf.clear()

    while i < n:
        ch = s[i]
        if ch == "\\" and i + 1 < n and s[i + 1] in _ESCAPABLE:
            buf.append(s[i + 1])
            i += 2
            continue
        if ch == "`":
            end = _skip_code(s, i)
            j = i
            while j < n and s[j] == "`":
                j += 1
            run = j - i
            if end - i > run:  # matched
                code = s[j:end - run]
                if code.startswith(" ") and code.endswith(" ") and code.strip():
                    code = code[1:-1]
                flush()
                out.append((code, _flags(flags, "c")))
                i = end
                continue
            buf.append("`" * run)
            i = j
            continue
        if ch == "!" and i + 1 < n and s[i + 1] == "[":
            res = _try_link(s, i + 1, flags, depth)
            if res:
                spans, i = res
                flush()
                out.extend(spans)
                continue
        if ch == "[":
            res = _try_link(s, i, flags, depth)
            if res:
                spans, i = res
                flush()
                out.extend(spans)
                continue
        if ch == "<":
            m = _AUTOLINK_RE.match(s, i)
            if m:
                buf.append(m.group(1))
                i = m.end()
                continue
            m = _TAG_RE.match(s, i)
            if m:
                i = m.end()
                continue
        if s.startswith("**", i) or s.startswith("__", i):
            delim = s[i:i + 2]
            ok_open = i + 2 < n and not s[i + 2].isspace()
            if delim == "__" and i > 0 and s[i - 1].isalnum():
                ok_open = False
            if ok_open:
                j = _find_closer(s, delim, i + 2)
                if j != -1:
                    flush()
                    out.extend(_parse(s[i + 2:j], _flags(flags, "b"), depth + 1))
                    i = j + 2
                    continue
            buf.append(delim)
            i += 2
            continue
        if ch in "*_":
            ok_open = i + 1 < n and not s[i + 1].isspace()
            if ch == "_" and i > 0 and s[i - 1].isalnum():
                ok_open = False
            if ok_open:
                j = _find_closer(s, ch, i + 1)
                if j != -1:
                    flush()
                    out.extend(_parse(s[i + 1:j], _flags(flags, "i"), depth + 1))
                    i = j + 1
                    continue
        buf.append(ch)
        i += 1
    flush()
    return out


def _try_link(s: str, i: int, flags: str, depth: int):
    """s[i] == '['. Returns (spans, next_index) for [text](url), [text][ref]
    or None if this is not a link."""
    close = _match_bracket(s, i, "[", "]")
    if close == -1:
        return None
    text = s[i + 1:close]
    nxt = close + 1
    if nxt < len(s) and s[nxt] == "(":
        end = _match_bracket(s, nxt, "(", ")")
        if end == -1:
            return None
        return _parse(text, flags, depth + 1), end + 1
    if nxt < len(s) and s[nxt] == "[":
        end = s.find("]", nxt)
        if end == -1:
            return None
        return _parse(text, flags, depth + 1), end + 1
    return None


def _merge(spans: List[Span]) -> List[Span]:
    out: List[Span] = []
    for text, fl in spans:
        if not text:
            continue
        if out and out[-1][1] == fl:
            out[-1] = (out[-1][0] + text, fl)
        else:
            out.append((text, fl))
    return out


def plain(s: str) -> str:
    """Inline markdown -> plain text."""
    return "".join(t for t, _ in parse(s))


def spans_text(spans: List[Span]) -> str:
    return "".join(t for t, _ in spans)


def char_width(ch: str) -> int:
    if not ch:
        return 0
    o = ord(ch)
    if o < 32 or 0x7F <= o < 0xA0:
        return 0
    cat = unicodedata.category(ch)
    if cat in ("Mn", "Me", "Cf"):
        return 0
    if unicodedata.east_asian_width(ch) in ("W", "F"):
        return 2
    return 1


def width(s: str) -> int:
    """Display width of a string in terminal cells."""
    if s.isascii():
        return len(s)
    return sum(char_width(c) for c in s)
