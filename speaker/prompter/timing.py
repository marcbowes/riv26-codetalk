"""Talk-clock times: parsing "17:00–20:00" and formatting seconds."""
from __future__ import annotations

import re
from typing import Optional, Tuple

# "17:00" is minutes:seconds into the talk; "1:02:03" is h:mm:ss.
TIME = r"\d{1,3}:\d{2}(?::\d{2})?"
_RANGE_SEP = r"\s*(?:-{1,2}|–|—|‒|−|to)\s*"
RANGE_RE = re.compile(r"(?P<a>" + TIME + r")" + _RANGE_SEP + r"(?P<b>" + TIME + r")")
RANGE_FULL_RE = re.compile(r"^\s*" + RANGE_RE.pattern + r"\s*$")


def parse_time(text: str) -> Optional[int]:
    """'17:00' -> 1020 seconds. '1:02:03' -> 3723. Anything else -> None."""
    m = re.fullmatch(r"\s*(\d{1,3}):(\d{2})(?::(\d{2}))?\s*", text or "")
    if not m:
        return None
    a, b, c = m.group(1), m.group(2), m.group(3)
    if c is None:
        mins, secs = int(a), int(b)
        if secs >= 60:
            return None
        return mins * 60 + secs
    hours, mins, secs = int(a), int(b), int(c)
    if mins >= 60 or secs >= 60:
        return None
    return hours * 3600 + mins * 60 + secs


def parse_range(text: str, whole: bool = False) -> Optional[Tuple[int, int]]:
    """Find a time range like '17:00–20:00' in text (or require the whole
    text to be one if whole=True). Returns (start, end) seconds or None.
    A range whose end is before its start is rejected."""
    m = (RANGE_FULL_RE if whole else RANGE_RE).search(text or "")
    if not m:
        return None
    a, b = parse_time(m.group("a")), parse_time(m.group("b"))
    if a is None or b is None or b < a:
        return None
    return a, b


def fmt(seconds: Optional[float], signed: bool = False) -> str:
    """1020 -> '17:00'; 3723 -> '1:02:03'; with signed=True, 83 -> '+1:23'."""
    if seconds is None:
        return "--:--"
    s = int(seconds) if seconds >= 0 else -int(-seconds)
    sign = ""
    if s < 0:
        sign, s = "-", -s
    elif signed:
        sign = "+"
    h, rem = divmod(s, 3600)
    m, sec = divmod(rem, 60)
    if h:
        return "%s%d:%02d:%02d" % (sign, h, m, sec)
    return "%s%d:%02d" % (sign, m, sec)


def fmt_range(start: Optional[int], end: Optional[int]) -> str:
    if start is None or end is None:
        return ""
    return "%s–%s" % (fmt(start), fmt(end))


def warn_window(start: int, end: int) -> int:
    """How long before a step's end the indicator turns amber: the last
    minute, or the last half of the step if it is shorter than two minutes."""
    return int(min(60, max(0, end - start) / 2))


def pace(elapsed: float, start: Optional[int], end: Optional[int]) -> Tuple[str, str]:
    """Where the clock is relative to a step's window.

    Returns (state, text) where state is one of
      'ahead'  clock is before the step's start
      'ok'     inside the window, more than the warn window left
      'warn'   inside the last minute (amber)
      'over'   past the step's end (red)
      'none'   the step has no time budget
    """
    if start is None or end is None:
        return "none", "no budget"
    if elapsed < start:
        return "ahead", "AHEAD %s" % fmt(start - elapsed)
    if elapsed >= end:
        return "over", "OVER %s" % fmt(elapsed - end, signed=True)
    left = end - elapsed
    if left <= warn_window(start, end):
        return "warn", "%s LEFT" % fmt(left)
    return "ok", "ON TIME · %s left" % fmt(left)


# ---------------------------------------------------------------- schedule delta

ON_TIME = 30      # within ±30 s counts as on time
LATE = 90         # 30–90 s behind is amber; more is red


def planned_at(start: Optional[int], end: Optional[int], beat: int, beats: int) -> Optional[float]:
    """Where the talk clock should be when the presenter is at `beat`
    (0-based) of a step with `beats` beats in [start, end]: the step's start
    plus its duration times the fraction of beats already passed."""
    if start is None or end is None:
        return None
    beats = max(1, beats)
    beat = min(max(0, beat), beats)
    return start + (end - start) * beat / beats


def planned_window(start: Optional[int], end: Optional[int], beat: int, beats: int) -> Optional[Tuple[float, float]]:
    """When the presenter should be on `beat` (0-based) of a step with `beats`
    beats in [start, end]: the beat's slice of the step, [planned_at(beat),
    planned_at(beat + 1)]. Anywhere inside it is on time."""
    lo = planned_at(start, end, beat, beats)
    hi = planned_at(start, end, beat + 1, beats)
    if lo is None or hi is None:
        return None
    return lo, hi


def fmt_delta(delta: float) -> str:
    """+150 -> '+2m' (whole minutes, rounded down), -45 -> '-45s', 0 -> '±0'."""
    s = int(abs(delta))
    if s < ON_TIME:
        return "±0"
    sign = "+" if delta > 0 else "-"
    if s < 60:
        return "%s%ds" % (sign, s)
    return "%s%dm" % (sign, s // 60)


def schedule(elapsed: float, started: bool, planned) -> Tuple[str, str, str]:
    """The overall ahead/behind badge.

    `planned` is the current beat's window (lo, hi) from planned_window, or a
    single planned time. Inside the window you're on time; before it you're
    ahead by lo - elapsed; after it you're behind by elapsed - hi. Positive
    delta means behind.
    Returns (state, big, word):
      ('none',  '--',   'CLOCK STOPPED' / 'OFF SCRIPT')  before `s`, or no plan here
      ('ok',    '±0',   'ON TIME')    within ±30 s
      ('ahead', '-1m',  'AHEAD')      more than 30 s ahead (green)
      ('warn',  '+45s', 'BEHIND')     30–90 s behind (amber)
      ('over',  '+2m',  'BEHIND')     90 s or more behind (red)
    """
    if not started:
        return "none", "--", "CLOCK STOPPED"
    if planned is None:
        return "none", "--", "OFF SCRIPT"
    lo, hi = planned if isinstance(planned, tuple) else (planned, planned)
    delta = elapsed - lo if elapsed < lo else (elapsed - hi if elapsed > hi else 0.0)
    if abs(delta) < ON_TIME:
        return "ok", "±0", "ON TIME"
    if delta < 0:
        return "ahead", fmt_delta(delta), "AHEAD"
    if delta < LATE:
        return "warn", fmt_delta(delta), "BEHIND"
    return "over", fmt_delta(delta), "BEHIND"
