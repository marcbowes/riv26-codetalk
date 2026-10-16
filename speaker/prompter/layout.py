"""Step -> styled, wrapped lines. Pure functions, no curses.

A style is (role, state, flags):
  role   what it is: say, run.code, key, note, expect, ...
  state  past / cur / fut for beats, '' for chrome
  flags  inline markup: any of b (bold), i (italic), c (code)
The curses layer maps styles to colors; tests read the text.
"""
from __future__ import annotations

import re
from typing import Dict, List, Optional, Sequence, Tuple

from . import inline
from .model import EXPECT, KEY, PROMPT, RACE, RUN, SAY, TABLE, Beat, Elem, Step
from .parser import PLACEHOLDER_RE, command_pieces

Style = Tuple[str, str, str]
Seg = Tuple[str, Style]


class Line:
    __slots__ = ("segs", "beat", "fill")

    def __init__(self, segs: List[Seg], beat: Optional[int] = None, fill: Optional[Style] = None):
        self.segs = segs
        self.beat = beat
        self.fill = fill

    @property
    def text(self) -> str:
        return "".join(t for t, _ in self.segs)

    def __repr__(self) -> str:  # pragma: no cover
        return "Line(%r, beat=%r)" % (self.text, self.beat)


def S(role: str, state: str = "", flags: str = "") -> Style:
    return (role, state, flags)


# ------------------------------------------------------------------ wrapping


def _cut(text: str, cells: int) -> Tuple[str, str]:
    """Split text so the head is at most `cells` wide (at least 1 char)."""
    w = 0
    for i, ch in enumerate(text):
        cw = inline.char_width(ch)
        if w + cw > cells and i > 0:
            return text[:i], text[i:]
        w += cw
    return text, ""


def seg_width(segs: Sequence[Seg]) -> int:
    return sum(inline.width(t) for t, _ in segs)


def wrap(segs: Sequence[Seg], width: int, first: Sequence[Seg] = (),
         rest: Sequence[Seg] = ()) -> List[List[Seg]]:
    """Word-wrap styled segments to width cells. first/rest are prefixes
    for the first and following lines (e.g. bullets, hanging indents)."""
    width = max(4, width)
    if seg_width(first) > width - 2:
        first = ()
    if seg_width(rest) > width - 2:
        rest = ()
    # group consecutive non-space pieces so "**bold**," doesn't split
    groups: List[Tuple[bool, List[Seg]]] = []
    cur: List[Seg] = []
    for text, st in segs:
        for m in re.finditer(r"\s+|\S+", text):
            t = m.group()
            if t.isspace():
                if cur:
                    groups.append((False, cur))
                    cur = []
                if groups and not groups[-1][0]:
                    groups.append((True, [(" ", st)]))
            else:
                cur.append((t, st))
    if cur:
        groups.append((False, cur))
    while groups and groups[-1][0]:
        groups.pop()

    lines: List[List[Seg]] = []
    line: List[Seg] = list(first)
    lw = seg_width(line)
    base = lw
    pending: Optional[Seg] = None
    rest_w = seg_width(rest)
    for is_space, pieces in groups:
        if is_space:
            if lw > base:
                pending = pieces[0]
            continue
        gw = seg_width(pieces)
        need = gw + (1 if pending else 0)
        if lw > base and lw + need > width:
            lines.append(line)
            line, lw, base, pending = list(rest), rest_w, rest_w, None
        if pending:
            line.append(pending)
            lw += 1
            pending = None
        for text, st in pieces:
            while lw + inline.width(text) > width:
                room = width - lw
                if room <= 0 or (lw > base and room < 1):
                    lines.append(line)
                    line, lw, base = list(rest), rest_w, rest_w
                    continue
                head, text = _cut(text, room)
                line.append((head, st))
                lines.append(line)
                line, lw, base = list(rest), rest_w, rest_w
            if text:
                line.append((text, st))
                lw += inline.width(text)
    lines.append(line)
    return lines


def hard_wrap(segs: Sequence[Seg], width: int, cont: Sequence[Seg] = ()) -> List[List[Seg]]:
    """Wrap at exactly width cells (for code), prefixing continuation lines."""
    width = max(4, width)
    if seg_width(cont) > width - 2:
        cont = ()
    out: List[List[Seg]] = []
    line: List[Seg] = []
    lw = 0
    cw = seg_width(cont)
    for text, st in segs:
        while text:
            room = width - lw
            if room <= 0:
                out.append(line)
                line, lw = list(cont), cw
                continue
            head, text = _cut(text, room)
            line.append((head, st))
            lw += inline.width(head)
    out.append(line)
    return out


def clip(segs: Sequence[Seg], width: int) -> List[Seg]:
    """Truncate segments to width cells (for label lines that don't wrap)."""
    out: List[Seg] = []
    w = 0
    for text, st in segs:
        tw = inline.width(text)
        if w + tw <= width:
            out.append((text, st))
            w += tw
            continue
        head, _ = _cut(text, width - w) if width - w > 0 else ("", "")
        if head and inline.width(head) <= width - w:
            out.append((head, st))
        break
    return out


def spans_to_segs(spans: Sequence[inline.Span], role: str, state: str, extra: str = "") -> List[Seg]:
    return [(t, (role, state, "".join(sorted(set(fl + extra))))) for t, fl in spans]


def code_segs(text: str, role: str, state: str, flags: str = "") -> List[Seg]:
    """A code line, with <placeholders> picked out."""
    out: List[Seg] = []
    pos = 0
    for m in PLACEHOLDER_RE.finditer(text):
        if m.start() > pos:
            out.append((text[pos:m.start()], (role, state, flags)))
        out.append((m.group(0), ("ph", state, "")))
        pos = m.end()
    if pos < len(text):
        out.append((text[pos:], (role, state, flags)))
    return out or [("", (role, state, flags))]


# ------------------------------------------------------------------ elements


def render_elems(elems: Sequence[Elem], role: str, state: str, width: int,
                 margin: int = 0) -> List[List[Seg]]:
    """Generic rendering of elements (for notes, reader notes, SAY)."""
    out: List[List[Seg]] = []
    pad = [(" " * margin, (role, state, ""))] if margin else []
    w = width
    prev_kind = None
    for e in elems:
        if out and not (e.kind == "list" and prev_kind == "para"):
            out.append(list(pad))
        prev_kind = e.kind
        if e.kind == "para":
            out.extend(wrap(spans_to_segs(e.spans, role, state), w, pad, pad))
        elif e.kind == "heading":
            out.extend(wrap(spans_to_segs(e.spans, role, state, "b"), w, pad, pad))
        elif e.kind == "list":
            out.extend(_list_lines(e, role, state, w, pad))
        elif e.kind in ("code", "output"):
            flags = "c" if e.kind == "code" else ""
            for ln in (e.text.split("\n") if e.text else [""]):
                segs = pad + [("  ", (role, state, ""))] + code_segs(ln, role, state, flags)
                out.extend(hard_wrap(segs, w, pad + [("    ", (role, state, ""))]))
        elif e.kind == "table":
            states = [state] * len(e.rows)
            for ln, _ in render_table(e, w - margin, states, state, role_override=role):
                out.append(pad + ln)
        elif e.kind in ("key", "reader", "note"):
            kids = e.children
            if e.kind == "key" and e.title:
                kids = [Elem("para", spans=[("★ ", "b")] + list(e.title))] + list(kids)
            out.extend(render_elems(kids, role, state, w, margin))
    return out


def _list_lines(e: Elem, role: str, state: str, width: int, pad: List[Seg]) -> List[List[Seg]]:
    out: List[List[Seg]] = []
    for level, marker, spans in e.items:
        indent = "  " * level
        if marker == "":
            bullet = "  "
        elif marker in "-*+":
            bullet = "• " if level == 0 else "◦ "
        else:
            bullet = marker + " "
        first = pad + [(indent + bullet, (role + ".bullet" if role == "say" else role, state, ""))]
        rest = pad + [(" " * (len(indent) + inline.width(bullet)), (role, state, ""))]
        out.extend(wrap(spans_to_segs(spans, role, state), width, first, rest))
    return out


# ------------------------------------------------------------------ tables


def _table_widths(widths_nat: List[int], widths_min: List[int], avail: int) -> Optional[List[int]]:
    if sum(widths_nat) <= avail:
        return list(widths_nat)
    if sum(widths_min) > avail:
        return None
    spare = avail - sum(widths_min)
    grow = [n - m for n, m in zip(widths_nat, widths_min)]
    total = sum(grow) or 1
    out = [m + (spare * g) // total for m, g in zip(widths_min, grow)]
    # hand out rounding leftovers to the widest-growing columns
    left = avail - sum(out)
    for i in sorted(range(len(out)), key=lambda k: -grow[k]):
        if left <= 0:
            break
        if out[i] < widths_nat[i]:
            out[i] += 1
            left -= 1
    return out


def render_table(e: Elem, width: int, row_states: List[str], head_state: str,
                 role_override: Optional[str] = None) -> List[Tuple[List[Seg], int]]:
    """A table as (segments, row index) pairs; row index -1 is the header."""
    header = list(e.header)
    rows = [list(r) for r in e.rows]
    ncols = max([len(header)] + [len(r) for r in rows] + [1])
    header += [[] for _ in range(ncols - len(header))]
    for r in rows:
        r += [[] for _ in range(ncols - len(r))]
    if e.race and header and not inline.spans_text(header[0]).strip():
        header[0] = [("#", "")]
    role_head = role_override or "table.head"
    role_cell = role_override or "table.cell"
    role_grid = role_override or "table.grid"

    def cell_w(spans):
        return inline.width(inline.spans_text(spans))

    def longest_word(spans):
        words = inline.spans_text(spans).split()
        return max([inline.width(w) for w in words] + [1])

    nat = [max([cell_w(header[c])] + [cell_w(r[c]) for r in rows] + [1]) for c in range(ncols)]
    mins = [min(nat[c], max(3, min(14, max([longest_word(header[c])] +
                                             [longest_word(r[c]) for r in rows]))))
            for c in range(ncols)]
    sep = " │ "
    avail = width - inline.width(sep) * (ncols - 1)
    widths = _table_widths(nat, mins, avail)
    out: List[Tuple[List[Seg], int]] = []

    if widths is None:  # too narrow for a grid: stack "Header: value" lines
        names = [inline.spans_text(h).strip() for h in header]
        for ri, r in enumerate(rows):
            st = row_states[ri] if ri < len(row_states) else head_state
            role = "table.cur" if (st == "cur" and not role_override) else role_cell
            label = "#%d" % (ri + 1)
            if e.race and inline.spans_text(r[0]).strip():
                label = "#" + inline.spans_text(r[0]).strip()
            out.append(([("── %s " % label, (role_grid, st, ""))], ri))
            for c in range(ncols):
                if not inline.spans_text(r[c]).strip():
                    continue
                if e.race and c == 0:
                    continue
                name = names[c] or ("col %d" % (c + 1))
                first = [(name + ": ", (role_head, st, "b"))]
                for ln in wrap(spans_to_segs(r[c], role, st), width, first, [("  ", (role, st, ""))]):
                    out.append((ln, ri))
        return out

    def row_lines(cells, role, st, flags=""):
        wrapped = [wrap(spans_to_segs(cells[c], role, st, flags), widths[c]) for c in range(ncols)]
        height = max(len(w) for w in wrapped)
        lines = []
        for k in range(height):
            segs: List[Seg] = []
            for c in range(ncols):
                part = wrapped[c][k] if k < len(wrapped[c]) else []
                segs.extend(part)
                padn = widths[c] - seg_width(part)
                if c < ncols - 1:
                    segs.append((" " * padn, (role, st, "")))
                    segs.append((sep, (role_grid, st, "")))
            lines.append(segs)
        return lines

    for ln in row_lines(header, role_head, head_state, "b"):
        out.append((ln, -1))
    rule = "─┼─".join("─" * w for w in widths)
    out.append(([(rule, (role_grid, head_state, ""))], -1))
    for ri, r in enumerate(rows):
        st = row_states[ri] if ri < len(row_states) else head_state
        role = "table.cur" if (st == "cur" and not role_override) else role_cell
        for ln in row_lines(r, role, st):
            out.append((ln, ri))
    return out


# ------------------------------------------------------------------ beats

LABELS = {
    RUN: (" ▶ RUN ", "run"),
    PROMPT: (" ✦ PROMPT ", "prompt"),
}


def _state(i: int, cur: int) -> str:
    return "past" if i < cur else ("cur" if i == cur else "fut")


def _code_block(beat: Beat, state: str, width: int, piece: Optional[int]) -> List[Line]:
    label, fam = LABELS[beat.kind]
    lines: List[Line] = []
    head: List[Seg] = [(label, S(fam + ".label", state)),
                       (" %s " % (beat.target or fam), S(fam + ".target", state))]
    if state == "cur":
        hint = "c copy" + ("  C next piece" if beat.kind == RUN and len(command_pieces(beat)) > 1 else "")
        room = width - seg_width(head) - 1
        if room > inline.width(hint) + 2:
            head.append((" " * (room - inline.width(hint)), S("say", state)))
            head.append((hint, S("hint", state)))
    lines.append(Line(head))
    text = beat.elems[0].text if beat.elems else beat.command
    marked = _piece_lines(beat, piece) if (state == "cur" and piece is not None) else set()
    role = fam + ".code"
    for n, src in enumerate(text.split("\n")):
        mark = ("▸", S(fam + ".mark", state)) if n in marked else (" ", S(role, state))
        for k, segs in enumerate(hard_wrap([mark, (" ", S(role, state))] + code_segs(src, role, state),
                                           width, [("  ↪ ", S(role + ".wrap", state))])):
            lines.append(Line(segs, fill=S(role, state)))
    if beat.expect:
        lines.extend(_expect_lines(beat.expect, state, width, "EXPECT"))
    return lines


def _piece_lines(beat: Beat, piece: int) -> set:
    """Line numbers (in the code block) covered by command piece `piece`."""
    pieces = command_pieces(beat)
    if not (0 <= piece < len(pieces)):
        return set()
    text = beat.elems[0].text if beat.elems else beat.command
    src_lines = text.split("\n")
    # find the piece by walking pieces in order
    pos = 0
    for k, p in enumerate(pieces):
        plines = p.split("\n")
        for start in range(pos, len(src_lines)):
            if src_lines[start:start + len(plines)] == plines:
                if k == piece:
                    return set(range(start, start + len(plines)))
                pos = start + len(plines)
                break
    return set()


def _expect_lines(text: str, state: str, width: int, label: str) -> List[Line]:
    lines = [Line([("  " + label, S("expect.label", state))])]
    for src in text.split("\n"):
        for segs in hard_wrap([("  │ ", S("expect.bar", state)), (src, S("expect", state))], width,
                              [("  │   ", S("expect.bar", state))]):
            lines.append(Line(segs))
    return lines


def _key_block(beat: Beat, state: str, width: int) -> List[Line]:
    e = beat.elems[0]
    fill = S("key", state)
    lines: List[Line] = []
    if state == "cur":
        lines.append(Line([], fill=fill))
    first = [(" ★ KEY POINT ", S("key.label", state)), (" ", fill)]
    title = spans_to_segs(e.title, "key.title", state, "b") if e.title else []
    body: List[List[Seg]] = []
    if title and seg_width(first) + 12 <= width:
        body = wrap(title, width - 1, first, [(" ", fill)])
    elif title:  # narrow: the label gets its own line
        body = [first] + wrap(title, width - 1, [(" ", fill)], [(" ", fill)])
    else:
        body = [first]
    for seg_line in body:
        lines.append(Line(seg_line, fill=fill))
    for ln in render_elems(e.children, "key", state, width - 1, margin=1):
        lines.append(Line(ln, fill=fill))
    if state == "cur":
        lines.append(Line([], fill=fill))
    return lines


def _say_block(beat: Beat, state: str, width: int) -> List[Line]:
    if not beat.elems:
        return [Line([("(nothing to say in this step)", S("say", "past"))])]
    return [Line(ln) for ln in render_elems(beat.elems, "say", state, width)]


def _reader_block(beat: Beat, state: str, width: int) -> List[Line]:
    out: List[Line] = []
    for r in beat.readers:
        out.append(Line([("  ┆ READER NOTE (f hides)", S("reader.label", state))]))
        bar = [("  ┆ ", S("reader.bar", state))]
        for ln in render_elems(r.children, "reader", state, width - 4):
            out.append(Line(bar + ln))
    return out


def layout_step(step: Step, cur: int, width: int, show_reader: bool = False,
                piece: Optional[int] = None) -> Tuple[List[Line], Dict[int, Tuple[int, int]]]:
    """All of a step's beats as lines. Returns (lines, {beat: (first, last)}).
    Race rows also get an entry under ("table", beat): the whole table, which
    the scroller keeps in view when it fits."""
    width = max(12, width)
    gutter = 2
    cw = width - gutter
    lines: List[Line] = []
    ranges: Dict[int, Tuple[int, int]] = {}
    beats = step.beats
    done_tables = set()
    race_spans: List[Tuple[int, int, List[int]]] = []

    def emit(blines: List[Line], idx: int, state: str) -> None:
        g = ("▌ ", S("gutter", "cur")) if state == "cur" else ("  ", S("say", state))
        for ln in blines:
            ln.segs = [g] + ln.segs
            ln.beat = idx if ln.beat is None else ln.beat
            lines.append(ln)

    for i, beat in enumerate(beats):
        state = _state(i, cur)
        if beat.kind == RACE:
            tbl = beat.elems[0]
            if id(tbl) in done_tables:
                continue
            done_tables.add(id(tbl))
            members = [(j, b) for j, b in enumerate(beats) if b.kind == RACE and b.elems[0] is tbl]
            row_beat = {b.row: j for j, b in members}
            row_states = [_state(row_beat.get(r, i), cur) for r in range(len(tbl.rows))]
            if lines:
                lines.append(Line([]))
            span_start = len(lines)
            head_state = "past" if all(s == "past" for s in row_states) else "fut"
            label = Line([("  ", S("say", head_state)), (" ⇄ RACE ", S("run.label", head_state)),
                          (" two psql panes, one row at a time ", S("run.target", head_state))])
            label.beat = members[0][0]
            for ln in [label]:
                lines.append(ln)
            for segs, r in render_table(tbl, cw, row_states, head_state):
                j = row_beat.get(r, members[0][0]) if r >= 0 else members[0][0]
                st = _state(j, cur) if r >= 0 else head_state
                g = ("▌ ", S("gutter", "cur")) if (r >= 0 and st == "cur") else ("  ", S("say", st))
                fill = S("table.cur", "cur") if (r >= 0 and st == "cur") else None
                lines.append(Line([g] + segs, beat=j, fill=fill))
            cur_member = next((b for j, b in members if j == cur), None)
            if cur_member is not None:
                now: List[Seg] = [(" ▶ NOW ", S("run.label", "cur")),
                                  (" %s " % cur_member.target.replace("psql · ", ""), S("run.target", "cur")),
                                  (" ", S("say", "cur"))]
                now += code_segs(cur_member.command, "run.code", "cur")
                if cur_member.expect:
                    now += [("  → " + cur_member.expect, S("expect", "cur"))]
                for segs in wrap(now, cw, (), [("   ", S("say", "cur"))]):
                    lines.append(Line([("▌ ", S("gutter", "cur"))] + segs, beat=cur))
            race_spans.append((span_start, len(lines) - 1, [j for j, _ in members]))
            for j, _ in members:
                ranges[("table", j)] = (span_start, len(lines) - 1)
            for j, b in members:
                if b.readers and show_reader:
                    emit(_reader_block(b, _state(j, cur), cw), j, _state(j, cur))
            continue

        if lines:
            lines.append(Line([]))
        if beat.kind == SAY:
            blines = _say_block(beat, state, cw)
        elif beat.kind in (RUN, PROMPT):
            blines = _code_block(beat, state, cw, piece if i == cur else None)
        elif beat.kind == EXPECT:
            text = beat.elems[0].text if beat.elems else ""
            blines = _expect_lines(text, state, cw, "EXPECT · on screen")
        elif beat.kind == KEY:
            blines = _key_block(beat, state, cw)
        elif beat.kind == TABLE:
            tbl = beat.elems[0]
            blines = [Line(segs) for segs, _ in
                      render_table(tbl, cw, [state] * len(tbl.rows), state)]
        else:
            blines = _say_block(beat, state, cw)
        if show_reader and beat.readers:
            blines.extend(_reader_block(beat, state, cw))
        emit(blines, i, state)

    for n, ln in enumerate(lines):
        if seg_width(ln.segs) > width:  # safety net for labels
            ln.segs = clip(ln.segs, width)
        if ln.beat is None:
            continue
        a, b = ranges.get(ln.beat, (n, n))
        ranges[ln.beat] = (min(a, n), max(b, n))
    return lines, ranges


def layout_notes(step: Step, width: int) -> List[Line]:
    """Presenter notes as a filled box."""
    if not step.notes:
        return []
    fill = S("note", "")
    out = [Line(clip([(" ✎ PRESENTER NOTES ", S("note.label", ""))], width), fill=fill)]
    for ln in render_elems(step.notes, "note", "", width - 1, margin=1):
        out.append(Line(ln, fill=fill))
    out.append(Line([], fill=fill))
    return out


def beat_headline(beat: Beat, width: int = 200) -> List[Seg]:
    """One-line description of a beat, for the NEXT line."""
    kind = beat.kind
    if kind in (RUN, PROMPT):
        fam = "run" if kind == RUN else "prompt"
        first = next((ln.strip() for ln in beat.command.split("\n")
                      if ln.strip() and not ln.strip().startswith(("--", "#"))), beat.command.strip())
        return [(LABELS[kind][0].strip() + " ", S(fam + ".target", "fut")),
                (beat.target + ": ", S("next", "")), (first, S("next.code", ""))]
    if kind == RACE:
        return [("RACE ", S("run.target", "fut")),
                (beat.target.replace("psql · ", "") + ": ", S("next", "")),
                (beat.command, S("next.code", ""))]
    if kind == KEY:
        return [("★ KEY POINT ", S("key.label", "fut")), (" " + beat.summary(width), S("next", ""))]
    if kind == EXPECT:
        return [("EXPECT ", S("expect.label", "fut")), (beat.summary(width), S("next", ""))]
    if kind == TABLE:
        return [("TABLE ", S("table.head", "fut")), (beat.summary(width), S("next", ""))]
    return [("SAY ", S("say.tag", "")), (beat.summary(width), S("next", ""))]
