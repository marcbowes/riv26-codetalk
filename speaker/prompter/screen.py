"""Compose the whole screen into a Canvas of (char, style) cells.

Pure: no curses. The app blits the canvas; tests read canvas.text().
"""
from __future__ import annotations

from typing import List, Optional, Sequence, Tuple

from . import inline, timing
from .layout import Line, S, Seg, Style, beat_headline, layout_notes, layout_step, seg_width, wrap
from .model import RUN, PROMPT, RACE

BG: Style = ("bg", "", "")


class Canvas:
    def __init__(self, h: int, w: int):
        self.h, self.w = max(0, h), max(0, w)
        self.cells: List[List[Tuple[str, Style]]] = [[(" ", BG)] * self.w for _ in range(self.h)]

    def put(self, y: int, x: int, text: str, style: Style, maxx: Optional[int] = None) -> int:
        """Write text at (y, x), clipped at maxx. Returns the next x."""
        if not (0 <= y < self.h):
            return x
        maxx = self.w if maxx is None else min(maxx, self.w)
        row = self.cells[y]
        for ch in text:
            if ch in "\n\r\t":
                ch = " "
            cw = inline.char_width(ch)
            if cw == 0:
                continue
            if x + cw > maxx:
                if x < maxx and cw == 2:
                    row[x] = (" ", style)
                return maxx
            if x >= 0:
                row[x] = (ch, style)
                if cw == 2 and x + 1 < self.w:
                    row[x + 1] = ("", style)
            x += cw
        return x

    def segs(self, y: int, x: int, segs: Sequence[Seg], maxx: Optional[int] = None) -> int:
        for text, st in segs:
            x = self.put(y, x, text, st, maxx)
        return x

    def fill(self, y: int, x0: int, x1: int, style: Style, ch: str = " ") -> None:
        if not (0 <= y < self.h):
            return
        for x in range(max(0, x0), min(self.w, x1)):
            self.cells[y][x] = (ch, style)

    def text(self) -> List[str]:
        return ["".join(c for c, _ in row) for row in self.cells]

    def runs(self, y: int) -> List[Tuple[int, str, Style]]:
        """Consecutive cells with the same style, for blitting."""
        out: List[Tuple[int, str, Style]] = []
        row = self.cells[y]
        start, buf, cur = 0, [], None
        for x, (ch, st) in enumerate(row):
            if st != cur:
                if buf or cur is not None:
                    out.append((start, "".join(buf), cur))
                start, buf, cur = x, [], st
            buf.append(ch)
        if cur is not None:
            out.append((start, "".join(buf), cur))
        return [r for r in out if r[1]]


# ------------------------------------------------------------------ big digits

_GLYPHS = {
    "0": ["###", "#.#", "#.#", "#.#", "###"],
    "1": [".#.", "##.", ".#.", ".#.", "###"],
    "2": ["###", "..#", "###", "#..", "###"],
    "3": ["###", "..#", ".##", "..#", "###"],
    "4": ["#.#", "#.#", "###", "..#", "..#"],
    "5": ["###", "#..", "###", "..#", "###"],
    "6": ["###", "#..", "###", "#.#", "###"],
    "7": ["###", "..#", "..#", ".#.", ".#."],
    "8": ["###", "#.#", "###", "#.#", "###"],
    "9": ["###", "#.#", "###", "..#", "###"],
    "+": ["...", ".#.", "###", ".#.", "..."],
    "-": ["...", "...", "###", "...", "..."],
    "±": [".#.", "###", ".#.", "...", "###"],
    " ": ["..", "..", "..", "..", ".."],
}


def big(text: str) -> List[str]:
    """Render text 3 rows tall with half blocks (unknown chars skipped)."""
    rows = ["", "", ""]
    for ch in text:
        g = _GLYPHS.get(ch)
        if g is None:
            continue
        g = g + ["." * len(g[0])]
        for r in range(3):
            top, bot = g[2 * r], g[2 * r + 1]
            for a, b in zip(top, bot):
                rows[r] += "█" if (a == "#" and b == "#") else "▀" if a == "#" else "▄" if b == "#" else " "
            rows[r] += " "
    return [r.rstrip(" ") for r in rows]


def badge_parts(big_text: str, word: str) -> Tuple[str, str]:
    """('+2m', 'BEHIND') -> big '+2', small lines ('MIN', 'BEHIND')."""
    if big_text.endswith("m"):
        return big_text[:-1], "MIN"
    if big_text.endswith("s"):
        return big_text[:-1], "SEC"
    return big_text, ""


# ------------------------------------------------------------------ compose


def compose(app, h: int, w: int) -> Canvas:
    cv = Canvas(h, w)
    if h < 4 or w < 20:
        cv.put(0, 0, "speaker: terminal too small", S("flash.err"))
        return cv
    step = app.step
    st = app.status()

    big_header = w >= 72 and h >= 16
    header_h = 3 if big_header else 2
    if big_header:
        _header_big(cv, app, st, w)
    else:
        _header_small(cv, app, st, w)

    footer_h = 2 if h >= 10 else 1
    body_top = header_h + (1 if h >= 24 else 0)
    body_bot = h - footer_h          # exclusive
    view_h = max(1, body_bot - body_top)

    sidebar_w = 0
    if w >= 110:
        sidebar_w = max(34, min(56, int(w * 0.32)))
    body_w = w - sidebar_w - (1 if sidebar_w else 0)

    # body
    lines, ranges = app.layout(body_w - 1)
    notes_inline = []
    if not sidebar_w and step.notes:
        notes_inline = layout_notes(step, body_w - 3)
        notes_inline = [Line([("  ", BG)] + ln.segs, None, ln.fill) for ln in notes_inline]
        notes_inline.append(Line([]))
    shift = len(notes_inline)
    all_lines = notes_inline + lines
    ranges = {k: (a + shift, b + shift) for k, (a, b) in ranges.items()}
    top = app.scroll_to(ranges.get(app.beat, (shift, shift)), len(all_lines), view_h,
                        first_beat_line=shift, group=ranges.get(("table", app.beat)))
    for row in range(view_h):
        n = top + row
        if n >= len(all_lines):
            break
        ln = all_lines[n]
        y = body_top + row
        if ln.fill is not None:  # boxes start after the 2-cell gutter
            cv.fill(y, 2, body_w - 1, ln.fill)
        cv.segs(y, 0, ln.segs, body_w - 1)
    # scroll indicators
    if top > 0:
        cv.put(body_top, body_w - 1, "▲", S("hint"))
    if top + view_h < len(all_lines):
        cv.put(body_bot - 1, body_w - 1, "▼", S("hint"))

    if sidebar_w:
        x0 = body_w
        for y in range(body_top, body_bot):
            cv.put(y, x0, "│", S("sep"))
        _sidebar(cv, app, x0 + 2, body_top, w - 1, body_bot)

    _footer(cv, app, st, h, w, footer_h)

    if app.mode == "help":
        _help(cv, app, h, w)
    elif app.mode == "goto":
        _goto(cv, app, h, w)
    return cv


def _header_big(cv: Canvas, app, st: dict, w: int) -> None:
    """Three rows: title / beat + step window / clock + progress, with the
    schedule delta in big digits on the right."""
    dstate, dbig, dword = st["delta"]
    num, unit = badge_parts(dbig, dword)
    glyph = big(num)
    words = [unit, dword] if unit else (dword.split(" ", 1) + [""])[:2]
    small_w = max(inline.width(x) for x in words + ["vs plan"])
    gw = max(inline.width(g) for g in glyph) if glyph else 0
    bw = gw + small_w + 6
    lw = w - bw
    for y in range(3):
        cv.fill(y, 0, lw, S("header") if y == 0 else S("header2"))
    title = _title(app)
    x = cv.put(0, 0, " " + title[0], S("header.part"), lw - 1)
    cv.put(0, x, title[1], S("header", "", "b"), lw - 1)
    _beat_row(cv, app, st, 1, 0, lw - 1)
    x = _clock(cv, st, 2, 0, lw - 1)
    _progress(cv, app, st, 2, x + 2, lw - 1)
    ds = "delta." + dstate
    for y in range(3):
        cv.fill(y, lw, w, S(ds))
        cv.put(y, lw + 2, glyph[y] if y < len(glyph) else "", S(ds, "", "b"), w)
    sx = lw + 2 + gw + 2
    cv.put(0, sx, words[0], S(ds, "", "b"), w)
    cv.put(1, sx, words[1], S(ds, "", "b"), w)
    cv.put(2, sx, "vs plan", S(ds), w)


def _header_small(cv: Canvas, app, st: dict, w: int) -> None:
    """Two rows for small terminals; the delta is a reverse-video badge."""
    dstate, dbig, dword = st["delta"]
    badge = " %s %s " % (dbig, dword) if dstate != "none" else " -- "
    if dstate == "ok":
        badge = " ON TIME "
    bw = inline.width(badge)
    cv.fill(0, 0, w, S("header"))
    cv.fill(1, 0, w, S("header2"))
    title = _title(app)
    x = cv.put(0, 0, " " + title[0], S("header.part"), w - bw - 1)
    cv.put(0, x, title[1], S("header", "", "b"), w - bw - 1)
    cv.put(0, w - bw, badge, S("delta." + dstate, "", "b"))
    x = _beat_row(cv, app, st, 1, 0, w, compact=True)
    x = cv.put(1, x, " ", S("header2"), w)
    x = _clock(cv, st, 1, x, w, compact=True)
    if w - x > 16:
        _progress(cv, app, st, 1, x + 2, w - 1)


def _title(app) -> Tuple[str, str]:
    step = app.step
    part = app.talk.parts[step.part_index] if step.part_index < len(app.talk.parts) else None
    ptitle = part.title if part is not None else ""
    if ptitle and ptitle != step.name:
        return ptitle + " ▸ ", step.name
    return "", step.name + ("" if step.talk else "   (not in the talk flow)")


def _beat_row(cv: Canvas, app, st: dict, y: int, x0: int, x1: int, compact: bool = False) -> int:
    hs = S("header2")
    fmt = " %d/%d " if compact else " beat %d/%d "
    x = cv.put(y, x0, fmt % (app.beat + 1, len(app.step.beats)), S("header2", "", "b"), x1)
    win = app.step.window
    if win:
        x = cv.put(y, x, "│ " + ("" if compact else "step ") + win + " ", hs, x1)
        pstate, ptext = st["pace"]
        if st["started"]:
            if compact:
                ptext = ptext.replace("ON TIME · ", "")
            x = cv.put(y, x, " " + ptext + " ", S("ind." + pstate, "", "b"), x1)
    elif not app.step.talk:
        x = cv.put(y, x, "│ not in the talk flow ", hs, x1)
    return x


def _clock(cv: Canvas, st: dict, y: int, x0: int, x1: int, compact: bool = False) -> int:
    hs = S("header2")
    x = cv.put(y, x0, "" if compact else " clock ", hs, x1)
    x = cv.put(y, x, "%s / %s" % (timing.fmt(st["elapsed"]), timing.fmt(st["total"])),
               S("header2.clock", "", "b"), x1)
    if not st["running"]:
        note = ("  s starts" if not st["started"] else "  PAUSED") if not compact else \
            (" (s)" if not st["started"] else " PAUSED")
        x = cv.put(y, x, note, S("header2.warn", "", "b"), x1)
    return x


def _progress(cv: Canvas, app, st: dict, y: int, x0: int, x1: int) -> None:
    t0, t1 = app.talk.span
    total = max(1, t1 - t0)
    pct = " %3d%% " % int(round(100 * max(0.0, min(1.0, (st["position"] - t0) / total))))
    bar_w = x1 - x0 - len(pct)
    if bar_w < 6:
        return
    pos = st["position"]
    fill_n = int(round(bar_w * max(0.0, min(1.0, (pos - t0) / total))))
    for i in range(bar_w):
        cv.put(y, x0 + i, "█" if i < fill_n else "░", S("prog.fill" if i < fill_n else "prog.empty"))
    # part boundaries
    for part in app.talk.parts:
        if part.talk and part.start is not None and part.start > t0:
            px = x0 + int(round(bar_w * (part.start - t0) / total))
            if x0 <= px < x0 + bar_w:
                cv.put(y, px, "▏" if px < x0 + fill_n else "│", S("prog.tick"))
    if st["started"]:
        cx = x0 + min(bar_w - 1, int(round(bar_w * max(0.0, min(1.0, (st["elapsed"] - t0) / total)))))
        cv.put(y, cx, "◆", S("prog.clock." + st["delta"][0], "", "b"))
    cv.put(y, x0 + bar_w, pct, S("header2"))


def _sidebar(cv: Canvas, app, x0: int, y0: int, x1: int, y1: int) -> None:
    width = x1 - x0
    y = y0
    notes = layout_notes(app.step, width)
    if notes:
        for ln in notes:
            if y >= y1:
                break
            if ln.fill:
                cv.fill(y, x0, x1, ln.fill)
            cv.segs(y, x0, ln.segs, x1)
            y += 1
    else:
        cv.put(y, x0, "No presenter notes for this step.", S("hint"), x1)
        y += 1
    y += 1
    if y + 3 >= y1:
        return
    cv.put(y, x0, "RUN OF SHOW", S("ros.title", "", "b"), x1)
    y += 1
    steps = app.talk.steps
    flow = app.talk.flow
    show = flow if app.step.talk else list(range(len(steps)))
    room = y1 - y
    try:
        ci = show.index(app.step_i)
    except ValueError:
        ci = 0
    start = max(0, min(ci - room // 3, len(show) - room))
    for k in show[start:start + room]:
        s = steps[k]
        role = "ros.cur" if k == app.step_i else ("ros.past" if k < app.step_i else "ros")
        start_t = timing.fmt(s.start) if s.start is not None else "  -  "
        label = "%5s  %s" % (start_t, s.name)
        if role == "ros.cur":
            cv.fill(y, x0, x1, S(role))
        cv.put(y, x0, ("▶" if role == "ros.cur" else " ") + label, S(role), x1)
        y += 1


def _footer(cv: Canvas, app, st: dict, h: int, w: int, footer_h: int) -> None:
    y = h - footer_h
    if footer_h == 2:
        cv.fill(y, 0, w, S("next"))
        x = cv.put(y, 0, " NEXT ▸ ", S("next.label", "", "b"))
        nxt = app.peek_next()
        if nxt is None:
            cv.put(y, x + 1, "end of the talk", S("next"))
        elif isinstance(nxt, str):
            cv.put(y, x + 1, nxt, S("next", "", "b"), w - 1)
        else:
            cv.segs(y, x + 1, [(t, s) for t, s in beat_headline(nxt, w)], w - 1)
        y += 1
    flash = app.current_flash()
    if flash:
        text, kind = flash
        cv.fill(y, 0, w, S("flash." + kind))
        cv.put(y, 0, " " + text, S("flash." + kind, "", "b"), w)
        return
    cv.fill(y, 0, w, S("hint.bar"))
    x = 0
    if app.changed_on_disk:
        x = cv.put(y, 0, " TALK.md changed: r reloads ", S("flash.warn", "", "b"))
    hidden = app.hidden_readers()
    keys = [("n/␣", "next"), ("p", "back"), ("N/P", "step"), ("g", "go to"), ("c", "copy"),
            ("s", "clock"), ("f", "reader notes%s" % (" (%d hidden)" % hidden if hidden and not app.show_reader
                                                     else (" on" if app.show_reader else ""))),
            ("?", "help"), ("q", "quit")]
    for k, label in keys:
        if x + len(k) + len(label) + 3 > w:
            break
        x = cv.put(y, x, " " + k, S("hint.key", "", "b"))
        x = cv.put(y, x, " " + label + " ", S("hint.bar"))


def _box(cv: Canvas, y0: int, x0: int, y1: int, x1: int, title: str) -> None:
    for y in range(y0, y1):
        cv.fill(y, x0, x1, S("overlay"))
    cv.put(y0, x0 + 2, " %s " % title, S("overlay.title", "", "b"), x1 - 1)


HELP = [
    ("n  space  →  PgDn", "next beat"),
    ("p  ←  PgUp", "previous beat"),
    ("N / P", "next / previous step"),
    ("g", "go to a step: number, time (17:30) or title words"),
    ("c", "copy this beat's command or prompt (pbcopy)"),
    ("C", "copy the next single statement/line of it (for \\gset)"),
    ("s / t", "start-pause / reset the talk clock to 0:00"),
    ("T or =", "set the clock to the plan for this beat (after a restart + g)"),
    ("j k  ↓ ↑", "scroll"),
    ("f", "show / hide reader-only follow-along notes"),
    ("r", "reload TALK.md, keeping your place"),
    ("?  /  q", "this help  /  quit"),
]
LEGEND = [
    ("say", "SAY", "plain text: what you say (current beat bold, with ▌)"),
    ("run.label", " ▶ RUN ", "a command, with its target (psql, shell)"),
    ("prompt.label", " ✦ PROMPT ", "paste into the AI agent"),
    ("expect.label", "EXPECT", "what the audience will see"),
    ("key.label", " ★ KEY POINT ", "must be said"),
    ("note.label", " ✎ PRESENTER NOTES ", "private notes for you"),
    ("reader.label", "┆ READER NOTE", "follow-along only (hidden; f)"),
    ("ph", "<placeholder>", "edit before running"),
]


def _help(cv: Canvas, app, h: int, w: int) -> None:
    bw = min(w - 2, 78) if w >= 80 else w
    bh = min(h - 2, len(HELP) + len(LEGEND) + 7) if h >= 12 else h
    kw = 19 if bw >= 70 else max(8, bw // 3)
    x0 = max(0, (w - bw) // 2)
    y0 = max(0, (h - bh) // 2)
    _box(cv, y0, x0, y0 + bh, x0 + bw, "speaker · keys (any key closes)")
    y = y0 + 2
    for k, d in HELP:
        if y >= y0 + bh - 1:
            return
        x = cv.put(y, x0 + 2, k.ljust(kw)[:kw] + " ", S("overlay.key", "", "b"), x0 + bw - 1)
        cv.put(y, x, d, S("overlay"), x0 + bw - 1)
        y += 1
    y += 1
    for role, label, d in LEGEND:
        if y >= y0 + bh - 1:
            return
        x = cv.put(y, x0 + 2, label, S(role, "fut"), x0 + bw - 1)
        cv.put(y, max(x + 1, x0 + kw + 3), d, S("overlay"), x0 + bw - 1)
        y += 1
    if y < y0 + bh - 1:
        cv.put(y, x0 + 2, "Delta badge: green ahead/on time, amber 30-90 s behind, red more.",
               S("overlay.dim"), x0 + bw - 1)


def _goto(cv: Canvas, app, h: int, w: int) -> None:
    bw = min(w - 2, 90)
    bh = max(6, min(h - 2, 24))
    x0 = max(0, (w - bw) // 2)
    y0 = max(0, (h - bh) // 2)
    _box(cv, y0, x0, y0 + bh, x0 + bw, "go to step (Enter jumps, Esc cancels, ↑↓ select)")
    x = cv.put(y0 + 1, x0 + 2, "› ", S("overlay.key", "", "b"))
    x = cv.put(y0 + 1, x, app.query, S("overlay.input", "", "b"), x0 + bw - 2)
    cv.put(y0 + 1, x, "█", S("overlay.key"))
    matches = app.goto_matches()
    rows = bh - 3
    sel = app.sel
    start = max(0, min(sel - rows + 1, len(matches) - rows)) if sel >= rows else 0
    steps = app.talk.steps
    y = y0 + 2
    if not matches:
        cv.put(y, x0 + 2, "no match", S("overlay.dim"), x0 + bw - 1)
    for n, k in enumerate(matches[start:start + rows]):
        s = steps[k]
        role = "overlay.sel" if start + n == sel else ("overlay" if s.talk else "overlay.dim")
        if role == "overlay.sel":
            cv.fill(y, x0 + 1, x0 + bw - 1, S(role))
        win = s.window or ("not in talk" if not s.talk else "")
        cv.put(y, x0 + 2, "%-12s %s" % (win, s.name), S(role), x0 + bw - 2)
        y += 1
