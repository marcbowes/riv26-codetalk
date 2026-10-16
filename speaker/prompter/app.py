"""The interactive teleprompter: state, keys, and the curses loop."""
from __future__ import annotations

import curses
import locale
import os
import re
import signal
import sys
import time
from typing import Callable, Dict, List, Optional, Tuple, Union

from . import clipboard, parser, timing
from .layout import layout_step
from .model import PROMPT, RACE, RUN, Beat, Step, Talk

Key = Union[str, int]

# ------------------------------------------------------------------ state


class App:
    """Everything the screen shows, and what each key does. No curses
    calls here, so tests can drive it directly."""

    def __init__(self, talk: Talk, path: str = "", all_parts: bool = False,
                 now: Callable[[], float] = time.monotonic,
                 copier: Callable[[str], Tuple[bool, str]] = clipboard.copy):
        self.talk = talk
        self.path = path
        self.all_parts = all_parts
        self.now = now
        self.copier = copier
        self.step_i = talk.flow[0] if talk.flow else 0
        self.beat = 0
        self.scroll_delta = 0
        self.show_reader = False
        self.mode = "normal"            # normal | help | goto
        self.query = ""
        self.sel = 0
        self.piece: Optional[int] = None
        self.clock_acc = 0.0
        self.clock_t0: Optional[float] = None
        self.flash: Optional[Tuple[str, str, float]] = None
        self.changed_on_disk = False
        self._mtime = self._stat()
        self._last_check = 0.0
        self._layout_cache: Dict[tuple, tuple] = {}

    # -------------------------------------------------------- basics

    @property
    def steps(self) -> List[Step]:
        return self.talk.steps

    @property
    def step(self) -> Step:
        return self.steps[self.step_i]

    @property
    def current(self) -> Beat:
        return self.step.beats[self.beat]

    def say(self, text: str, kind: str = "ok", secs: float = 3.0) -> None:
        self.flash = (text, kind, self.now() + secs)

    def current_flash(self) -> Optional[Tuple[str, str]]:
        if self.flash and self.now() < self.flash[2]:
            return self.flash[0], self.flash[1]
        return None

    # -------------------------------------------------------- clock

    @property
    def running(self) -> bool:
        return self.clock_t0 is not None

    def elapsed(self) -> float:
        e = self.clock_acc
        if self.clock_t0 is not None:
            e += self.now() - self.clock_t0
        return e

    def toggle_clock(self) -> None:
        if self.clock_t0 is None:
            self.clock_t0 = self.now()
            self.say("Clock running from %s" % timing.fmt(self.elapsed()))
        else:
            self.clock_acc = self.elapsed()
            self.clock_t0 = None
            self.say("Clock paused at %s" % timing.fmt(self.clock_acc), "warn")

    def reset_clock(self) -> None:
        self.clock_acc = 0.0
        self.clock_t0 = None
        self.say("Clock reset to 0:00 (s starts it)", "warn")

    def set_clock(self, seconds: float) -> None:
        self.clock_acc = max(0.0, seconds)
        self.clock_t0 = None

    @property
    def started(self) -> bool:
        return self.running or self.clock_acc > 0

    # -------------------------------------------------------- pacing

    def planned(self) -> Optional[float]:
        """Where the talk clock should be at the current beat."""
        st = self.step
        if not st.talk:
            return None
        return timing.planned_at(st.start, st.end, self.beat, len(st.beats))

    def status(self) -> dict:
        el = self.elapsed()
        st = self.step
        t0, t1 = self.talk.span
        planned = self.planned()
        pos = planned if planned is not None else self._fallback_position()
        return {
            "elapsed": el,
            "running": self.running,
            "started": self.started,
            "total": t1 - t0,
            "planned": planned,
            "position": pos,
            "pace": timing.pace(el, st.start, st.end) if st.talk else ("none", "not in talk"),
            "delta": timing.schedule(el, self.started, planned),
        }

    def _fallback_position(self) -> float:
        """For steps outside the flow: the start of the next talk step."""
        t0, t1 = self.talk.span
        for k in self.talk.flow:
            if k > self.step_i and self.steps[k].start is not None:
                return float(self.steps[k].start)
        return float(t1 if self.step_i > (self.talk.flow[-1] if self.talk.flow else 0) else t0)

    # -------------------------------------------------------- navigation

    def _order(self) -> List[int]:
        if self.all_parts or not self.step.talk:
            return list(range(len(self.steps)))
        return self.talk.flow

    def _moved(self) -> None:
        self.scroll_delta = 0
        self.piece = None

    def goto(self, step_i: int, beat: int = 0) -> None:
        self.step_i = max(0, min(step_i, len(self.steps) - 1))
        n = len(self.step.beats)
        self.beat = max(0, min(beat if beat >= 0 else n + beat, n - 1))
        self._moved()

    def next_beat(self) -> None:
        if self.beat < len(self.step.beats) - 1:
            self.beat += 1
            self._moved()
            return
        self.next_step(at_end=True)

    def prev_beat(self) -> None:
        if self.beat > 0:
            self.beat -= 1
            self._moved()
            return
        order = self._order()
        before = [k for k in order if k < self.step_i]
        if before:
            self.goto(before[-1], -1)
        else:
            self.say("Start of the talk", "warn", 1.5)

    def next_step(self, at_end: bool = False) -> None:
        order = self._order()
        after = [k for k in order if k > self.step_i]
        if after:
            self.goto(after[0], 0)
        elif self.step.talk and not self.all_parts:
            self.say("End of the talk. g to jump to wrap-up, cleaning up, troubleshooting.", "warn", 4)
        else:
            self.say("End of the document", "warn", 2)

    def prev_step(self) -> None:
        order = self._order()
        before = [k for k in order if k < self.step_i]
        if before:
            self.goto(before[-1], 0)
        elif self.beat:
            self.goto(self.step_i, 0)
        else:
            self.say("Start of the talk", "warn", 1.5)

    # -------------------------------------------------------- layout & scroll

    def layout(self, width: int):
        key = (self.step_i, self.beat, width, self.show_reader, self.piece, id(self.talk))
        hit = self._layout_cache.get(key)
        if hit is None:
            if len(self._layout_cache) > 64:
                self._layout_cache.clear()
            hit = layout_step(self.step, self.beat, width, self.show_reader, self.piece)
            self._layout_cache[key] = hit
        return hit

    def scroll_to(self, rng: Tuple[int, int], total: int, view_h: int, first_beat_line: int = 0,
                  group: Optional[Tuple[int, int]] = None) -> int:
        """Top line of the body: the current beat in view with some earlier
        context, plus any j/k offset. `group` is a larger range (a race
        table) to keep in view if it fits."""
        self.view_h = view_h
        a, b = rng
        if group is not None and group[1] - group[0] + 1 <= view_h:
            a, b = min(a, group[0]), max(b, group[1])
        if total <= view_h:
            auto = 0
        else:
            ctx = max(2, view_h // 4)
            height = b - a + 1
            if height + ctx > view_h:
                ctx = max(0, view_h - height)
            auto = a - min(ctx, view_h - 1)
            if self.beat == 0:
                auto = 0 if b < view_h else min(first_beat_line, a)
            auto = max(0, min(auto, total - view_h))
        top = auto + self.scroll_delta
        top = max(0, min(top, max(0, total - view_h)))
        self.scroll_delta = top - auto
        return top

    def scroll(self, n: int) -> None:
        self.scroll_delta += n

    # -------------------------------------------------------- copy

    def copy(self, piece: bool = False) -> None:
        beat = self.current
        if not beat.copyable:
            self.say("Nothing to copy in this %s beat (n moves on)" % beat.kind, "warn", 2)
            return
        what = beat.target or ("AI agent" if beat.kind == PROMPT else "psql")
        if piece:
            pieces = parser.command_pieces(beat)
            if not pieces:
                pieces = [beat.command]
            self.piece = 0 if self.piece is None else (self.piece + 1) % len(pieces)
            text = pieces[self.piece]
            label = "piece %d/%d" % (self.piece + 1, len(pieces))
        else:
            self.piece = None
            text = beat.command
            n = len(text.strip("\n").split("\n"))
            label = "%d line%s" % (n, "" if n == 1 else "s")
        ok, how = self.copier(text)
        if not ok:
            self.say("Copy failed: %s" % how, "err", 4)
            return
        first = " ".join(text.split())
        msg = "✓ COPIED %s for %s: %s" % (label, what, first)
        warn = []
        ph = parser.placeholders(text)
        if ph:
            warn.append("fill in " + " ".join(ph))
        if not piece and beat.kind == RUN and re.search(r"\\gset", text) and len(parser.command_pieces(beat)) > 1:
            warn.append("has \\gset: paste one statement at a time (C)")
        if warn:
            msg = "✓ COPIED %s for %s · %s · %s" % (label, what, "; ".join(warn), first)
        self.say(msg, "warn" if warn else "ok", 4 if warn else 3)

    # -------------------------------------------------------- reload

    def _stat(self) -> float:
        try:
            return os.stat(self.path).st_mtime if self.path else 0.0
        except OSError:
            return 0.0

    def tick(self) -> None:
        """Called by the loop: notice TALK.md changing on disk."""
        t = self.now()
        if t - self._last_check < 1.0:
            return
        self._last_check = t
        m = self._stat()
        if m and m != self._mtime:
            self.changed_on_disk = True

    def reload(self) -> None:
        if not self.path:
            self.say("Nothing to reload", "warn")
            return
        try:
            talk = parser.load(self.path)
        except OSError as exc:
            self.say("Reload failed: %s" % exc, "err", 4)
            return
        old = self.step
        key, title, beat = old.key, old.title, self.beat
        idx = None
        for k, s in enumerate(talk.steps):
            if s.key == key:
                idx = k
                break
        if idx is None:
            for k, s in enumerate(talk.steps):
                if s.title == title:
                    idx = k
                    break
        if idx is None:
            idx = min(self.step_i, len(talk.steps) - 1)
        self.talk = talk
        self._layout_cache.clear()
        self._mtime = self._stat()
        self.changed_on_disk = False
        self.goto(idx, min(beat, len(talk.steps[idx].beats) - 1))
        nw = len(talk.warnings)
        self.say("Reloaded %s: %d steps%s" % (os.path.basename(self.path), len(talk.steps),
                                               (", %d markup warnings (--check)" % nw) if nw else ""))

    # -------------------------------------------------------- goto

    def goto_matches(self) -> List[int]:
        return search(self.steps, self.query)

    # -------------------------------------------------------- misc

    def hidden_readers(self) -> int:
        return sum(len(b.readers) for b in self.step.beats)

    def peek_next(self):
        """The next beat, or a string naming the next step."""
        if self.beat < len(self.step.beats) - 1:
            return self.step.beats[self.beat + 1]
        after = [k for k in self._order() if k > self.step_i]
        if not after:
            return None
        s = self.steps[after[0]]
        when = (" at %s" % timing.fmt(s.start)) if s.start is not None else ""
        return "next step: %s%s" % (s.name, when)

    # -------------------------------------------------------- keys

    def handle_key(self, key: Key) -> bool:
        """Returns False to quit."""
        if self.mode == "help":
            self.mode = "normal"
            return True
        if self.mode == "goto":
            return self._goto_key(key)
        k = key
        if k == "q":
            return False
        if k in ("n", " ", curses.KEY_RIGHT, curses.KEY_NPAGE, "\n", "\r", curses.KEY_ENTER):
            self.next_beat()
        elif k in ("p", curses.KEY_LEFT, curses.KEY_PPAGE, curses.KEY_BACKSPACE, "\x7f"):
            self.prev_beat()
        elif k == "N":
            self.next_step()
        elif k == "P":
            self.prev_step()
        elif k == "g":
            self.mode = "goto"
            self.query = ""
            self.sel = 0
        elif k == "c":
            self.copy()
        elif k == "C":
            self.copy(piece=True)
        elif k == "s":
            self.toggle_clock()
        elif k == "t":
            self.reset_clock()
        elif k == "r":
            self.reload()
        elif k == "f":
            self.show_reader = not self.show_reader
            self.say("Reader-only notes %s" % ("shown" if self.show_reader else "hidden"), "ok", 1.5)
        elif k == "?" or k == "h":
            self.mode = "help"
        elif k in ("j", curses.KEY_DOWN):
            self.scroll(1)
        elif k in ("k", curses.KEY_UP):
            self.scroll(-1)
        elif k == "J":
            self.scroll(max(1, getattr(self, "view_h", 10) // 2))
        elif k == "K":
            self.scroll(-max(1, getattr(self, "view_h", 10) // 2))
        elif k == curses.KEY_HOME:
            self.goto(self.step_i, 0)
        elif k == curses.KEY_END:
            self.goto(self.step_i, -1)
        return True

    def _goto_key(self, key: Key) -> bool:
        if key in ("\x1b",):
            self.mode = "normal"
        elif key in ("\n", "\r", curses.KEY_ENTER):
            m = self.goto_matches()
            if m:
                self.goto(m[max(0, min(self.sel, len(m) - 1))], 0)
                self.say("→ %s" % self.step.name, "ok", 1.5)
            self.mode = "normal"
        elif key in (curses.KEY_BACKSPACE, "\x7f", "\b", "\x08"):
            self.query = self.query[:-1]
            self.sel = 0
        elif key in (curses.KEY_DOWN, "\x0e"):
            self.sel = min(self.sel + 1, max(0, len(self.goto_matches()) - 1))
        elif key in (curses.KEY_UP, "\x10"):
            self.sel = max(0, self.sel - 1)
        elif isinstance(key, str) and key.isprintable():
            self.query += key
            self.sel = 0
        return True


# ------------------------------------------------------------------ search


def search(steps: List[Step], query: str) -> List[int]:
    """Rank steps for the goto box: step numbers, then talk times, then
    fuzzy title matches. An empty query lists every step."""
    q = query.strip().lower()
    if not q:
        return list(range(len(steps)))
    scored: List[Tuple[float, int]] = []
    num = int(q) if re.fullmatch(r"\d{1,3}", q) else None
    t = timing.parse_time(q)
    for i, s in enumerate(steps):
        hay = ("%s %s" % (s.label, s.title)).lower()
        score = None
        if num is not None and num in s.numbers:
            score = 1000 - len(s.numbers)
        elif t is not None and s.talk and s.start is not None and s.end is not None \
                and s.start <= t < s.end:
            score = 900
        else:
            sc = _fuzzy(q, hay)
            if sc is not None:
                score = sc + (5 if s.talk else 0)
        if score is not None:
            scored.append((-score, i))
    scored.sort()
    return [i for _, i in scored]


def _fuzzy(q: str, hay: str) -> Optional[float]:
    if q in hay:
        word_start = re.search(r"(^|\W)" + re.escape(q), hay) is not None
        return 500 - hay.index(q) + (50 if word_start else 0)
    words = q.split()
    if len(words) > 1 and all(w in hay for w in words):
        return 400
    # subsequence
    pos, gaps, last = 0, 0, -1
    for ch in q:
        if ch == " ":
            continue
        j = hay.find(ch, pos)
        if j == -1:
            return None
        if last >= 0:
            gaps += j - last - 1
        last, pos = j, j + 1
    return max(1.0, 300 - gaps * 3)


# ------------------------------------------------------------------ theme

A_ITALIC = getattr(curses, "A_ITALIC", 0)

# role -> (fg256, bg256, fg8, bg8, attrs, mono attrs)
# -1 is the terminal's default color.
_P = {
    "bg":            (-1, -1, -1, -1, "", ""),
    "say":           (-1, -1, -1, -1, "", ""),
    "say.bullet":    (75, -1, 4, -1, "b", "b"),
    "say.tag":       (16, 250, 0, 7, "b", "r"),
    "head":          (-1, -1, -1, -1, "b", "b"),
    "header":        (231, 24, 7, 4, "b", "r"),
    "header.part":   (153, 24, 6, 4, "", "r"),
    "header2":       (252, 236, 7, 0, "", ""),
    "header2.clock": (231, 236, 7, 0, "b", "b"),
    "header2.warn":  (214, 236, 3, 0, "b", "b"),
    "run.label":     (16, 41, 0, 2, "b", "rb"),
    "run.target":    (120, 235, 2, 0, "b", "b"),
    "run.code":      (157, 235, 2, 0, "", "b"),
    "run.code.wrap": (242, 235, 2, 0, "", "d"),
    "run.mark":      (16, 220, 0, 3, "b", "r"),
    "prompt.label":  (16, 177, 0, 5, "b", "rb"),
    "prompt.target": (213, 235, 5, 0, "b", "b"),
    "prompt.code":   (225, 235, 5, 0, "", "b"),
    "prompt.code.wrap": (242, 235, 5, 0, "", "d"),
    "prompt.mark":   (16, 220, 0, 3, "b", "r"),
    "ph":            (214, 235, 3, 0, "bu", "bu"),
    "expect.label":  (246, -1, 7, -1, "b", "d"),
    "expect":        (248, -1, 7, -1, "", "d"),
    "expect.bar":    (240, -1, 7, -1, "", "d"),
    "key":           (229, 58, 3, 0, "", "b"),
    "key.title":     (230, 58, 3, 0, "b", "b"),
    "key.label":     (16, 220, 0, 3, "b", "rb"),
    "note":          (195, 23, 6, 0, "", ""),
    "note.label":    (16, 80, 0, 6, "b", "ru"),
    "reader":        (110, -1, 4, -1, "", "d"),
    "reader.label":  (67, -1, 4, -1, "b", "d"),
    "reader.bar":    (67, -1, 4, -1, "", "d"),
    "table.head":    (-1, -1, -1, -1, "b", "b"),
    "table.cell":    (-1, -1, -1, -1, "", ""),
    "table.grid":    (240, -1, 7, -1, "", "d"),
    "table.cur":     (231, 238, 7, 4, "b", "r"),
    "gutter":        (39, -1, 6, -1, "b", "b"),
    "hint":          (244, -1, 7, -1, "", "d"),
    "hint.bar":      (250, 236, 7, 0, "", ""),
    "hint.key":      (231, 236, 7, 0, "b", "b"),
    "next":          (252, 235, 7, 0, "", ""),
    "next.label":    (16, 252, 0, 7, "b", "r"),
    "next.code":     (157, 235, 2, 0, "", "b"),
    "flash.ok":      (16, 41, 0, 2, "b", "r"),
    "flash.warn":    (16, 214, 0, 3, "b", "r"),
    "flash.err":     (231, 160, 7, 1, "b", "rb"),
    "ind.ok":        (16, 41, 0, 2, "b", "r"),
    "ind.ahead":     (16, 80, 0, 6, "b", "r"),
    "ind.warn":      (16, 214, 0, 3, "b", "r"),
    "ind.over":      (231, 160, 7, 1, "b", "rb"),
    "ind.none":      (250, 238, 7, 0, "", ""),
    "delta.ok":      (16, 41, 0, 2, "b", "r"),
    "delta.ahead":   (16, 41, 0, 2, "b", "r"),
    "delta.warn":    (16, 214, 0, 3, "b", "r"),
    "delta.over":    (231, 196, 7, 1, "b", "rb"),
    "delta.none":    (250, 238, 7, 0, "", "r"),
    "prog.fill":     (39, 236, 4, 0, "", "b"),
    "prog.empty":    (240, 236, 7, 0, "", "d"),
    "prog.tick":     (250, 236, 7, 0, "", ""),
    "prog.clock.ok":    (46, 236, 2, 0, "b", "b"),
    "prog.clock.ahead": (46, 236, 2, 0, "b", "b"),
    "prog.clock.warn":  (214, 236, 3, 0, "b", "b"),
    "prog.clock.over":  (196, 236, 1, 0, "b", "b"),
    "prog.clock.none":  (250, 236, 7, 0, "b", "b"),
    "sep":           (238, -1, 7, -1, "", "d"),
    "ros":           (250, -1, 7, -1, "", ""),
    "ros.title":     (244, -1, 7, -1, "b", "b"),
    "ros.past":      (242, -1, 7, -1, "", "d"),
    "ros.cur":       (231, 24, 7, 4, "b", "r"),
    "overlay":       (252, 236, 7, 0, "", ""),
    "overlay.title": (231, 236, 7, 0, "b", "b"),
    "overlay.key":   (120, 236, 2, 0, "b", "b"),
    "overlay.input": (231, 236, 7, 0, "b", "b"),
    "overlay.dim":   (244, 236, 7, 0, "", "d"),
    "overlay.sel":   (231, 24, 7, 4, "b", "r"),
}
# Past beats: everything drops to grey (boxes keep a dark background so
# their shape still shows).
_PAST_FG = 243
_CODE_FG = (172, 3)   # inline `code` in prose


class Theme:
    """Maps (role, state, flags) styles to curses attributes."""

    def __init__(self, depth: int):
        self.depth = depth          # 256, 8 or 0 (no color)
        self._cache: Dict[tuple, int] = {}
        self._pairs: Dict[Tuple[int, int], int] = {}
        self._max_pairs = getattr(curses, "COLOR_PAIRS", 0) if depth else 0

    @staticmethod
    def detect(no_color: bool = False) -> "Theme":
        if no_color or os.environ.get("NO_COLOR"):
            return Theme(0)
        try:
            if not curses.has_colors():
                return Theme(0)
            curses.start_color()
            try:
                curses.use_default_colors()
            except curses.error:
                pass
            return Theme(256 if curses.COLORS >= 256 else 8)
        except curses.error:
            return Theme(0)

    def _pair(self, fg: int, bg: int) -> int:
        if fg == -1 and bg == -1:
            return 0
        key = (fg, bg)
        if key in self._pairs:
            return curses.color_pair(self._pairs[key])
        n = len(self._pairs) + 1
        if n >= max(1, self._max_pairs):
            return 0
        try:
            curses.init_pair(n, fg, bg)
        except curses.error:
            try:  # terminals without default-color support
                curses.init_pair(n, fg if fg >= 0 else curses.COLOR_WHITE,
                                 bg if bg >= 0 else curses.COLOR_BLACK)
            except curses.error:
                return 0
        self._pairs[key] = n
        return curses.color_pair(n)

    def attr(self, style) -> int:
        a = self._cache.get(style)
        if a is not None:
            return a
        try:
            a = self._attr(style)
        except Exception:
            a = 0
        self._cache[style] = a
        return a

    def _attr(self, style) -> int:
        role, state, flags = style
        spec = _P.get(role)
        if spec is None:
            spec = _P.get(role.split(".")[0], _P["say"])
        fg256, bg256, fg8, bg8, attrs, mono = spec
        a = 0
        if self.depth == 0:
            for ch in mono:
                a |= {"b": curses.A_BOLD, "r": curses.A_REVERSE, "u": curses.A_UNDERLINE,
                      "d": curses.A_DIM}.get(ch, 0)
            if state == "past" and role not in ("gutter",):
                a = (a & ~curses.A_BOLD & ~curses.A_REVERSE) | curses.A_DIM
                if role in ("key", "key.title", "key.label"):
                    a = curses.A_DIM | curses.A_UNDERLINE
            if state == "cur" and role in ("say", "say.bullet", "table.cell", "key", "key.title"):
                a |= curses.A_BOLD
            if state == "cur" and role.startswith("key"):
                a |= curses.A_REVERSE
            if "b" in flags:
                a |= curses.A_BOLD if state != "past" else 0
            if "c" in flags and role in ("say", "table.cell", "note", "reader", "key"):
                a |= curses.A_UNDERLINE
            if "i" in flags:
                a |= A_ITALIC
            return a
        fg, bg = (fg256, bg256) if self.depth == 256 else (fg8, bg8)
        if "c" in flags and role in ("say", "table.cell", "say.bullet", "reader"):
            fg = _CODE_FG[0] if self.depth == 256 else _CODE_FG[1]
        if "c" in flags and role in ("note",):
            fg = 229 if self.depth == 256 else 3
        if "c" in flags and role in ("key", "key.title"):
            fg = (94 if state == "cur" else 222) if self.depth == 256 else 3
        if state == "cur" and role in ("key", "key.title"):
            fg, bg = (16, 220) if self.depth == 256 else (0, 3)
            if "c" in flags:
                fg = 94 if self.depth == 256 else 1
        if state == "cur" and role == "key.label":
            fg, bg = (220, 16) if self.depth == 256 else (3, 0)
        if state == "past":
            if self.depth == 256:
                fg = _PAST_FG
                if bg not in (-1,):
                    bg = 235 if role not in ("table.cur",) else -1
                if role in ("run.label", "prompt.label", "key.label", "run.mark", "prompt.mark"):
                    fg, bg = 250, 239
            else:
                a |= curses.A_DIM
                if role in ("run.label", "prompt.label", "key.label"):
                    fg, bg = 7, 0
                elif bg != -1:
                    bg = 0
        a |= self._pair(fg, bg)
        for ch in attrs:
            if ch == "b" and state != "past":
                a |= curses.A_BOLD
            elif ch == "u":
                a |= curses.A_UNDERLINE
        if state == "cur" and role in ("say", "say.bullet", "table.cell", "head"):
            a |= curses.A_BOLD
        if "b" in flags and state != "past":
            a |= curses.A_BOLD
        if "i" in flags:
            a |= A_ITALIC
        return a


# ------------------------------------------------------------------ curses


def blit(stdscr, canvas, theme: Theme) -> None:
    stdscr.erase()
    h, w = canvas.h, canvas.w
    for y in range(h):
        for x, text, style in canvas.runs(y):
            if y == h - 1 and x + len(text) >= w:
                text = text[: max(0, w - x - 1)]
                if not text:
                    continue
            try:
                stdscr.addstr(y, x, text, theme.attr(style))
            except curses.error:
                pass
    stdscr.refresh()


def _dump_screen(canvas) -> None:
    """Test hook: SPEAKER_SCREEN_DUMP=path writes the screen text after
    every draw."""
    path = os.environ.get("SPEAKER_SCREEN_DUMP")
    if not path:
        return
    try:
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as fh:
            fh.write("\n".join(canvas.text()) + "\n")
        os.replace(tmp, path)
    except OSError:
        pass


_ESC_FINAL = {"A": curses.KEY_UP, "B": curses.KEY_DOWN, "C": curses.KEY_RIGHT,
              "D": curses.KEY_LEFT, "H": curses.KEY_HOME, "F": curses.KEY_END}
_ESC_TILDE = {"1": curses.KEY_HOME, "4": curses.KEY_END, "5": curses.KEY_PPAGE,
              "6": curses.KEY_NPAGE, "7": curses.KEY_HOME, "8": curses.KEY_END}


def decode_escape(read) -> Key:
    """After an ESC, read the rest of a raw arrow / page key sequence that
    curses didn't translate (ESC [ C, ESC O C, ESC [ 6 ~ ...). `read`
    returns the next char or None on timeout. Unknown sequences become a
    plain ESC, and their bytes are dropped so they can't trigger commands."""
    ch = read()
    if ch is None:
        return "\x1b"
    if ch not in ("[", "O"):
        return "\x1b"
    params = ""
    for _ in range(8):
        c = read()
        if c is None:
            return "\x1b"
        if c.isdigit() or c == ";":
            params += c
            continue
        if c == "~":
            return _ESC_TILDE.get(params.split(";")[0], "\x1b")
        return _ESC_FINAL.get(c, "\x1b")
    return "\x1b"


def _loop(stdscr, app: App, no_color: bool) -> None:
    from .screen import compose
    theme = Theme.detect(no_color)
    try:
        curses.curs_set(0)
    except curses.error:
        pass
    stdscr.keypad(True)
    stdscr.timeout(250)
    while True:
        h, w = stdscr.getmaxyx()
        canvas = compose(app, h, w)
        blit(stdscr, canvas, theme)
        _dump_screen(canvas)
        try:
            key = stdscr.get_wch()
        except curses.error:
            app.tick()
            continue
        if key == "\x1b":
            key = _read_escape(stdscr)
        if key == curses.KEY_RESIZE:
            try:
                curses.update_lines_cols()
            except AttributeError:
                pass
            continue
        if not app.handle_key(key):
            return


def _read_escape(stdscr) -> Key:
    stdscr.timeout(40)

    def read():
        try:
            c = stdscr.get_wch()
        except curses.error:
            return None
        return c if isinstance(c, str) else None

    try:
        return decode_escape(read)
    finally:
        stdscr.timeout(250)


def _goto_arg(app: App, q: str) -> None:
    m = search(app.steps, q)
    if m:
        app.goto(m[0], 0)


def run(path: str, talk: Talk, args) -> int:
    try:
        locale.setlocale(locale.LC_ALL, "")
    except locale.Error:
        pass
    os.environ.setdefault("ESCDELAY", "25")
    app = App(talk, path, all_parts=getattr(args, "all", False))
    if getattr(args, "goto", None):
        _goto_arg(app, args.goto)
    if getattr(args, "clock", None):
        t = timing.parse_time(args.clock)
        if t is not None:
            app.set_clock(t)

    def _term(signum, frame):
        raise SystemExit(0)

    for sig in (signal.SIGTERM, signal.SIGHUP):
        try:
            signal.signal(sig, _term)
        except (ValueError, OSError):
            pass
    try:
        curses.wrapper(_loop, app, getattr(args, "no_color", False))
    except KeyboardInterrupt:
        pass
    except curses.error as exc:
        sys.stderr.write("speaker: this terminal can't run the UI (%s; TERM=%s). "
                         "Try TERM=xterm-256color, or --dump for text.\n"
                         % (exc, os.environ.get("TERM", "")))
        return 2
    # curses.wrapper restores the terminal; say where we stopped
    sys.stdout.write("speaker: stopped at %s, beat %d/%d, clock %s\n"
                     % (app.step.name, app.beat + 1, len(app.step.beats), timing.fmt(app.elapsed())))
    return 0
