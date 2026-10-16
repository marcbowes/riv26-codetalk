"""TALK.md -> Talk.

Two passes:
  1. parse_blocks: markdown lines -> raw blocks (paragraph, list, fence,
     quote, details, table, heading, rule, beat-break). Quotes and details
     are parsed recursively.
  2. build: headings split blocks into parts (##) and steps (###); each
     step's blocks become beats.

The parser is deliberately forgiving: anything it doesn't recognise becomes
a plain paragraph, and it never raises on odd input.
"""
from __future__ import annotations

import html
import re
from typing import List, Optional, Tuple

from . import inline
from .model import (EXPECT, KEY, PROMPT, RACE, RUN, SAY, TABLE, Beat, Elem,
                    Part, Step, Talk)
from .timing import RANGE_FULL_RE, parse_range

# ---------------------------------------------------------------- languages

# fence language -> target shown next to RUN
LANG_TARGET = {
    "sql": "psql", "psql": "psql", "pgsql": "psql", "postgres": "psql",
    "postgresql": "psql", "plpgsql": "psql",
    "sh": "shell", "bash": "shell", "shell": "shell", "zsh": "shell",
    "console": "shell", "shell-session": "shell", "fish": "shell",
    "prompt": "AI agent", "ai": "AI agent", "agent": "AI agent",
}
PROMPT_LANGS = {"prompt", "ai", "agent"}
OUTPUT_LANGS = {"", "text", "txt", "output", "out", "plain", "plaintext", "expected"}

# ---------------------------------------------------------------- regexes

FENCE_RE = re.compile(r"^(?P<indent>\s*)(?P<fence>`{3,}|~{3,})\s*(?P<info>[^`]*)$")
HEADING_RE = re.compile(r"^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$")
RULE_RE = re.compile(r"^\s{0,3}(?:(?:\*\s*){3,}|(?:-\s*){3,}|(?:_\s*){3,})$")
LIST_RE = re.compile(r"^(?P<indent>\s*)(?P<marker>[-*+]|\d{1,9}[.)])(?P<space>\s+|$)(?P<text>.*)$")
QUOTE_RE = re.compile(r"^\s*>")
DETAILS_OPEN_RE = re.compile(r"<details\b[^>]*>", re.I)
DETAILS_CLOSE_RE = re.compile(r"</details\s*>", re.I)
SUMMARY_RE = re.compile(r"<summary\b[^>]*>(.*?)</summary\s*>", re.I | re.S)
TABLE_DELIM_RE = re.compile(r"^\s*\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$")
HTML_LINE_RE = re.compile(
    r"^\s*</?(?:div|p|br|hr|img|summary|picture|source|table|thead|tbody|tr|td|th|center|"
    r"section|figure|figcaption|span|b|i|em|strong|a|kbd|sub|sup)\b[^>]*>", re.I)
BEAT_MARK_RE = re.compile(r"\bbeat\b", re.I)
STEP_LABEL_RE = re.compile(r"^(steps?|part|section|act)\b\s*(.*)$", re.I)
PANE_RE = re.compile(r"\bpane\s+([A-Z0-9])\b", re.I)
BOTH_PANES_RE = re.compile(r"\b(both|each) panes?\b", re.I)
NEW_TERMINAL_RE = re.compile(r"\b(second|another|new|other) terminal\b", re.I)
PLACEHOLDER_RE = re.compile(r"<[A-Za-z][\w.-]*>")


class Block:
    """A raw markdown block."""

    def __init__(self, kind: str, line: int, **data):
        self.kind = kind
        self.line = line
        self.text = data.get("text", "")
        self.level = data.get("level", 0)
        self.lang = data.get("lang", "")
        self.items = data.get("items", [])          # list blocks
        self.children = data.get("children", [])    # quote / details
        self.summary = data.get("summary", "")      # details
        self.rows = data.get("rows", [])            # table: header + rows
        self.unclosed = data.get("unclosed", False)
        self.info = data.get("info", "")            # fence info string

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return "Block(%s@%d %r)" % (self.kind, self.line, (self.text or self.summary)[:30])


# ---------------------------------------------------------------- pass 1


def _indent(line: str) -> int:
    return len(line) - len(line.lstrip(" \t"))


def _starts_block(line: str) -> bool:
    """Does this line begin a non-paragraph block (so it ends a paragraph)?"""
    s = line.strip()
    if not s:
        return True
    if FENCE_RE.match(line) or HEADING_RE.match(line) or RULE_RE.match(line):
        return True
    if QUOTE_RE.match(line) or DETAILS_OPEN_RE.match(s) or DETAILS_CLOSE_RE.match(s):
        return True
    if s.startswith("<!--") or s.startswith("|"):
        return True
    m = LIST_RE.match(line)
    if m and m.group("text").strip():
        marker = m.group("marker")
        # like CommonMark, only "1." may interrupt a paragraph
        return not marker[0].isdigit() or int(marker[:-1]) == 1
    return False


def split_cells(line: str) -> List[str]:
    """Split a table row on '|' that aren't escaped or inside code spans."""
    s = line.strip()
    if s.startswith("|"):
        s = s[1:]
    if s.endswith("|") and not s.endswith("\\|"):
        s = s[:-1]
    cells, cur, i, in_code = [], [], 0, 0
    while i < len(s):
        ch = s[i]
        if ch == "\\" and i + 1 < len(s) and s[i + 1] == "|":
            cur.append("|")
            i += 2
            continue
        if ch == "`":
            j = i
            while j < len(s) and s[j] == "`":
                j += 1
            run = j - i
            if in_code == 0:
                # only enter code if there's a closer on this row
                if s.find("`" * run, j) != -1:
                    in_code = run
            elif in_code == run:
                in_code = 0
            cur.append(s[i:j])
            i = j
            continue
        if ch == "|" and not in_code:
            cells.append("".join(cur).strip())
            cur = []
            i += 1
            continue
        cur.append(ch)
        i += 1
    cells.append("".join(cur).strip())
    return cells


def parse_blocks(lines: List[str], base: int = 1) -> List[Block]:
    """Markdown lines -> blocks. base is the 1-based line number of lines[0]."""
    out: List[Block] = []
    i, n = 0, len(lines)
    while i < n:
        line = lines[i]
        s = line.strip()
        ln = base + i
        if not s:
            i += 1
            continue

        m = FENCE_RE.match(line)
        if m:
            fence, info, ind = m.group("fence"), m.group("info").strip(), len(m.group("indent"))
            lang = info.split()[0].lower() if info else ""
            body, j, closed = [], i + 1, False
            close_re = re.compile(r"^\s*" + re.escape(fence[0]) + "{%d,}\\s*$" % len(fence))
            while j < n:
                if close_re.match(lines[j]):
                    closed = True
                    break
                raw = lines[j]
                # strip up to the opening fence's indentation
                k = 0
                while k < ind and k < len(raw) and raw[k] in " \t":
                    k += 1
                body.append(raw[k:].rstrip("\n"))
                j += 1
            out.append(Block("fence", ln, lang=lang, info=info, text="\n".join(body),
                             unclosed=not closed))
            i = j + 1
            continue

        m = HEADING_RE.match(line)
        if m:
            out.append(Block("heading", ln, level=len(m.group(1)), text=m.group(2)))
            i += 1
            continue

        if RULE_RE.match(line):
            out.append(Block("rule", ln))
            i += 1
            continue

        if QUOTE_RE.match(line):
            inner, j = [], i
            while j < n and QUOTE_RE.match(lines[j]):
                inner.append(re.sub(r"^\s*> ?", "", lines[j], count=1))
                j += 1
            out.append(Block("quote", ln, children=parse_blocks(inner, ln),
                             text="\n".join(inner)))
            i = j
            continue

        if DETAILS_OPEN_RE.match(s):
            blk, i = _parse_details(lines, i, base)
            out.append(blk)
            continue

        if DETAILS_CLOSE_RE.match(s) or SUMMARY_RE.match(s):  # stray tags
            i += 1
            continue

        if s.startswith("<!--"):
            j, text = i, []
            while j < n:
                text.append(lines[j])
                if "-->" in lines[j]:
                    break
                j += 1
            comment = "\n".join(text)
            if BEAT_MARK_RE.search(comment):
                out.append(Block("beatbreak", ln))
            i = j + 1
            continue

        if "|" in s and i + 1 < n and TABLE_DELIM_RE.match(lines[i + 1]) and "-" in lines[i + 1]:
            rows = [split_cells(line)]
            j = i + 2
            while j < n and lines[j].strip() and "|" in lines[j]:
                rows.append(split_cells(lines[j]))
                j += 1
            out.append(Block("table", ln, rows=rows))
            i = j
            continue

        m = LIST_RE.match(line)
        if m and m.group("text").strip():
            blk, i = _parse_list(lines, i, base)
            out.append(blk)
            continue

        if HTML_LINE_RE.match(s):
            text = inline.plain(s).strip()
            if text:
                out.append(Block("para", ln, text=text))
            i += 1
            continue

        # paragraph
        para, j = [s], i + 1
        while j < n and not _starts_block(lines[j]):
            para.append(lines[j].strip())
            j += 1
        out.append(Block("para", ln, text=" ".join(para)))
        i = j
    return out


def _parse_details(lines: List[str], i: int, base: int) -> Tuple[Block, int]:
    """lines[i] contains <details>. Returns the block and the next index."""
    n = len(lines)
    ln = base + i
    # Gather everything up to the matching </details>.
    depth, j, chunk = 0, i, []
    while j < n:
        line = lines[j]
        depth += len(DETAILS_OPEN_RE.findall(line))
        closes = len(DETAILS_CLOSE_RE.findall(line))
        chunk.append(line)
        depth -= closes
        j += 1
        if depth <= 0:
            break
    text = "\n".join(chunk)
    # strip the outer <details ...> and its matching (last) </details>
    m = DETAILS_OPEN_RE.search(text)
    body = text[m.end():] if m else text
    closes = list(DETAILS_CLOSE_RE.finditer(body))
    if closes:
        body = body[: closes[-1].start()] + body[closes[-1].end():]
    summary = ""
    sm = SUMMARY_RE.search(body)
    if sm and not body[: sm.start()].strip():
        summary = " ".join(inline.plain(sm.group(1)).split())
        body = body[sm.end():]
    children = parse_blocks(body.split("\n"), ln)
    return Block("details", ln, summary=summary, children=children), j


def _parse_list(lines: List[str], i: int, base: int) -> Tuple[Block, int]:
    """Collect a list starting at lines[i]. Items are (level, marker, text);
    a continuation paragraph inside an item has marker ''."""
    n = len(lines)
    ln = base + i
    items: List[list] = []
    stack: List[Tuple[int, int]] = []   # (indent, content indent)
    base_indent = _indent(lines[i])
    j = i
    after_blank = False
    while j < n:
        line = lines[j]
        if not line.strip():
            # a blank line continues the list only if what follows is
            # indented or another item
            k = j + 1
            while k < n and not lines[k].strip():
                k += 1
            if k < n and (_indent(lines[k]) > base_indent or
                          (LIST_RE.match(lines[k]) and _indent(lines[k]) >= base_indent
                           and LIST_RE.match(lines[k]).group("text").strip())):
                after_blank = True
                j = k
                continue
            break
        s = line.strip()
        if FENCE_RE.match(line) or QUOTE_RE.match(line) or DETAILS_OPEN_RE.match(s) \
                or HEADING_RE.match(line) or RULE_RE.match(line) or s.startswith("|"):
            break
        m = LIST_RE.match(line)
        ind = _indent(line)
        if m and m.group("text").strip():
            while stack and ind < stack[-1][1]:
                stack.pop()
            level = len(stack)
            content = ind + len(m.group("marker")) + max(1, len(m.group("space")))
            stack.append((ind, content))
            items.append([level, m.group("marker"), m.group("text").strip()])
            after_blank = False
        elif after_blank:
            if ind <= base_indent:
                break
            while stack and ind < stack[-1][1]:
                stack.pop()
            level = max(0, len(stack) - 1)
            items.append([level, "", s])
            after_blank = False
        else:
            if items:
                items[-1][2] += " " + s
            else:
                break
        j += 1
    return Block("list", ln, items=[tuple(it) for it in items]), j


# ---------------------------------------------------------------- pass 2


def _elem_from_block(b: Block) -> Optional[Elem]:
    """Convert a raw block to a renderable element (no beat semantics)."""
    if b.kind == "para":
        return Elem("para", spans=inline.parse(b.text), line=b.line)
    if b.kind == "heading":
        return Elem("heading", spans=inline.parse(b.text, "b"), line=b.line)
    if b.kind == "list":
        return Elem("list", items=[(lv, mk, inline.parse(t)) for lv, mk, t in b.items], line=b.line)
    if b.kind == "fence":
        if b.lang in OUTPUT_LANGS:
            return Elem("output", text=b.text, line=b.line)
        return Elem("code", lang=b.lang, text=b.text, line=b.line)
    if b.kind == "table":
        header = [inline.parse(c) for c in b.rows[0]] if b.rows else []
        rows = [[inline.parse(c) for c in r] for r in b.rows[1:]]
        return Elem("table", header=header, rows=rows, race=_is_race(b.rows[0] if b.rows else []),
                    line=b.line)
    if b.kind == "quote":
        return Elem("reader", children=_elems(b.children), line=b.line)
    if b.kind == "details":
        kids = _elems(b.children)
        if b.summary:
            kids.insert(0, Elem("heading", spans=[(b.summary, "b")], line=b.line))
        return Elem("reader", children=kids, line=b.line)
    return None


def _elems(blocks: List[Block]) -> List[Elem]:
    out = []
    for b in blocks:
        e = _elem_from_block(b)
        if e is not None:
            out.append(e)
    return out


def _is_race(header: List[str]) -> bool:
    panes = [h for h in header if re.search(r"\bpane\b", inline.plain(h), re.I)]
    return len(panes) >= 2


def is_key_quote(b: Block) -> bool:
    text = inline.plain(b.text).lstrip()
    return text.startswith("★")


def is_presenter_details(b: Block) -> bool:
    return "presenter" in b.summary.lower()


def _key_elem(b: Block) -> Elem:
    """A ★ blockquote -> key elem with the leading bold sentence as title."""
    kids = list(b.children)
    title: List[inline.Span] = []
    children: List[Elem] = []
    if kids and kids[0].kind == "para":
        spans = inline.parse(kids[0].text)
        if spans and "b" in spans[0][1]:
            title = [(spans[0][0].strip().lstrip("★").strip(), "b")]
            rest = spans[1:]
            if rest:
                rest[0] = (rest[0][0].lstrip(), rest[0][1])
                children.append(Elem("para", spans=rest, line=kids[0].line))
        else:
            # no bold lead: drop the star, keep the text
            text = kids[0].text.lstrip().lstrip("*").lstrip()
            if text.startswith("★"):
                text = text[1:].lstrip()
            children.append(Elem("para", spans=inline.parse(text), line=kids[0].line))
        kids = kids[1:]
    children.extend(_elems(kids))
    return Elem("key", title=title, children=children, line=b.line)


def _pane_hints(text: str) -> List[str]:
    hints = []
    if BOTH_PANES_RE.search(text):
        hints.append("both panes")
    for m in PANE_RE.finditer(text):
        h = "pane %s" % m.group(1).upper()
        if h not in hints:
            hints.append(h)
    return hints


def _last_sentence(text: str) -> str:
    plain = inline.plain(text).strip()
    parts = re.split(r"(?<=[.!?])\s+", plain)
    return parts[-1] if parts else plain


def _code_target(lang: str, info: str, text: str, prev: Optional[Block]) -> str:
    """Where a command runs. An explicit target after the language wins
    (```sql pane B); otherwise a hint in the command's first comment line or
    in the last sentence of the paragraph just before it, if unambiguous."""
    target = LANG_TARGET.get(lang, lang or "code")
    extra = info.split(None, 1)[1].strip() if len(info.split(None, 1)) > 1 else ""
    extra = re.sub(r"^[{(\[]|[})\]]$", "", extra).strip()
    if extra:
        return "%s · %s" % (target, extra)
    first = text.lstrip().split("\n", 1)[0] if text.strip() else ""
    sources = []
    if first.startswith("--") or first.startswith("#"):
        sources.append(first)
    if prev is not None and prev.kind == "para":
        sources.append(_last_sentence(prev.text))
    for src in sources:
        if target == "psql":
            hints = _pane_hints(src)
            if len(hints) == 1:
                return "psql · " + hints[0]
            if hints:
                return target
        if target == "shell":
            m = NEW_TERMINAL_RE.search(src)
            if m:
                return "shell · %s terminal" % m.group(1).lower()
    return target


def unwrap_prompt(text: str) -> str:
    """Join hard-wrapped lines inside each paragraph of a prompt, so pasting
    it into an agent doesn't send early. Blank lines are kept."""
    paras = re.split(r"\n\s*\n", text.strip("\n"))
    out = []
    for p in paras:
        lines = [ln.strip() for ln in p.split("\n")]
        # keep list-looking lines on their own line
        merged: List[str] = []
        for ln in lines:
            if merged and not LIST_RE.match(ln):
                merged[-1] += " " + ln
            else:
                merged.append(ln)
        out.append("\n".join(merged))
    return "\n\n".join(out)


def _race_beats(tbl: Elem, line: int) -> List[Beat]:
    beats = []
    header = [inline.spans_text(h).strip() for h in tbl.header]
    for r, row in enumerate(tbl.rows):
        cmd, target, exp = "", "", ""
        for c, cell in enumerate(row):
            if c == 0 and (not header or not header[0] or header[0].lower() in ("#", "step", "t", "n")):
                continue
            if not inline.spans_text(cell).strip():
                continue
            target = "psql · " + (header[c] if c < len(header) and header[c] else "column %d" % c)
            before, after, seen_arrow = [], [], False
            for t, fl in cell:
                if not seen_arrow and "→" in t and "c" not in fl:
                    a, _, b = t.partition("→")
                    before.append((a, fl))
                    after.append((b, fl))
                    seen_arrow = True
                elif seen_arrow:
                    after.append((t, fl))
                else:
                    before.append((t, fl))
            code = [t for t, fl in before if "c" in fl]
            cmd = " ".join(x.strip() for x in code) if code else inline.spans_text(before).strip()
            exp = inline.spans_text(after).strip()
            break
        beats.append(Beat(RACE, elems=[tbl], target=target, command=cmd, expect=exp,
                          row=r, line=line + 2 + r))
    return beats


def build_beats(blocks: List[Block], warnings: List[str], where: str) -> Tuple[List[Beat], List[Elem]]:
    """A step's blocks -> (beats, presenter notes)."""
    beats: List[Beat] = []
    notes: List[Elem] = []
    pending_readers: List[Elem] = []
    say_open = False      # last beat is a SAY that can take more paragraphs
    prev: Optional[Block] = None

    def add(beat: Beat) -> Beat:
        nonlocal pending_readers
        if pending_readers:
            beat.readers.extend(pending_readers)
            pending_readers = []
        beats.append(beat)
        return beat

    def attach_reader(e: Elem) -> None:
        if beats:
            beats[-1].readers.append(e)
        else:
            pending_readers.append(e)

    for b in blocks:
        if b.kind in ("rule",):
            say_open = False
            prev = b
            continue
        if b.kind == "beatbreak":
            say_open = False
            prev = b
            continue

        if b.kind in ("para", "list", "heading"):
            e = _elem_from_block(b)
            bold_lead = b.kind == "para" and b.text.lstrip().startswith("**")
            if b.kind == "heading":
                say_open = False
            if say_open and beats and beats[-1].kind == SAY and not bold_lead:
                beats[-1].elems.append(e)
            else:
                add(Beat(SAY, elems=[e], line=b.line))
            say_open = True
            prev = b
            continue

        say_open = False

        if b.kind == "fence":
            if b.unclosed:
                warnings.append("%s line %d: unclosed ``` fence" % (where, b.line))
            if b.lang in OUTPUT_LANGS:
                last = beats[-1] if beats else None
                if (prev is not None and prev.kind == "fence" and prev.lang not in OUTPUT_LANGS
                        and last is not None and last.kind in (RUN, PROMPT) and not last.expect):
                    last.expect = b.text
                    last.elems.append(Elem("output", text=b.text, line=b.line))
                else:
                    add(Beat(EXPECT, elems=[Elem("output", text=b.text, line=b.line)], line=b.line))
                    warnings.append(
                        "%s line %d: plain ``` block that doesn't follow a command: shown as "
                        "standalone EXPECT" % (where, b.line))
            else:
                kind = PROMPT if b.lang in PROMPT_LANGS else RUN
                if b.lang not in LANG_TARGET:
                    warnings.append("%s line %d: unknown fence language %r: treated as a command"
                                    % (where, b.line, b.lang))
                cmd = unwrap_prompt(b.text) if kind == PROMPT else b.text
                add(Beat(kind, elems=[Elem("code", lang=b.lang, text=b.text, line=b.line)],
                         target=_code_target(b.lang, b.info, b.text, prev), command=cmd, line=b.line))
            prev = b
            continue

        if b.kind == "quote":
            if is_key_quote(b):
                add(Beat(KEY, elems=[_key_elem(b)], line=b.line))
            else:
                e = _elem_from_block(b)
                attach_reader(e)
                first = inline.plain(b.text).strip()
                if not re.match(r"(no agent|following along|if you|reader)", first, re.I):
                    warnings.append(
                        "%s line %d: blockquote that isn't a ★ key point is hidden as a reader "
                        "note: %r" % (where, b.line, first[:60]))
            prev = b
            continue

        if b.kind == "details":
            if is_presenter_details(b):
                notes.extend(_elems(b.children))
                if re.search(r"follow", b.summary, re.I):
                    warnings.append("%s line %d: <details> %r mixes presenter notes and "
                                    "following-along; shown as presenter notes"
                                    % (where, b.line, b.summary))
            else:
                attach_reader(_elem_from_block(b))
            prev = b
            continue

        if b.kind == "table":
            t = _elem_from_block(b)
            if t.race and t.rows:
                for rb in _race_beats(t, b.line):
                    add(rb)
                    if "..." in rb.command or "…" in rb.command:
                        warnings.append("%s line %d: race row %d command is abbreviated (%r); "
                                        "`c` will copy it as written"
                                        % (where, rb.line, rb.row + 1, rb.command))
            else:
                add(Beat(TABLE, elems=[t], line=b.line))
            prev = b
            continue

        prev = b

    if pending_readers:  # step with only reader notes
        add(Beat(SAY, elems=[], line=0))
    return beats, notes


# ---------------------------------------------------------------- headings


def parse_part_heading(text: str) -> Tuple[str, Optional[int], Optional[int]]:
    """'0:00–5:00 · Intro' -> ('Intro', 0, 300). Untimed -> (text, None, None)."""
    segs = [s.strip() for s in re.split(r"\s+·\s+|\s*·\s*", inline.plain(text)) if s.strip()]
    start = end = None
    rest = []
    for s in segs:
        if start is None and RANGE_FULL_RE.match(s):
            start, end = parse_range(s, whole=True) or (None, None)
            if start is not None:
                continue
        rest.append(s)
    return " · ".join(rest) or inline.plain(text), start, end


def parse_step_heading(text: str) -> Tuple[str, str, List[int], Optional[int], Optional[int]]:
    """'Step 6 · An index, without downtime · 17:00–20:00'
    -> ('An index, without downtime', 'Step 6', [6], 1020, 1200)."""
    title, start, end = parse_part_heading(text)
    segs = [s.strip() for s in title.split(" · ")]
    label, numbers = "", []
    m = STEP_LABEL_RE.match(segs[0]) if segs else None
    if m and re.search(r"\d", m.group(2) or ""):
        label = segs[0]
        numbers = [int(x) for x in re.findall(r"\d+", m.group(2))]
        segs = segs[1:]
    else:
        # "Step 6: Title" style
        m2 = re.match(r"^(steps?\s+[\d\s,&and–-]+?)\s*[:.]\s+(.*)$", title, re.I)
        if m2:
            label = m2.group(1).strip()
            numbers = [int(x) for x in re.findall(r"\d+", label)]
            segs = [m2.group(2)]
    return " · ".join(segs), label, numbers, start, end


# ---------------------------------------------------------------- build


def parse_text(text: str, path: str = "") -> Talk:
    try:
        return _parse_text(text, path)
    except Exception as exc:  # pragma: no cover - last-resort safety net
        talk = Talk(title="(could not parse %s)" % (path or "talk"), path=path)
        part = Part(title="Parse error")
        step = Step(title="Parse error", beats=[
            Beat(SAY, elems=[Elem("para", spans=[("speaker could not parse this file: %r" % exc, "")])])])
        part.steps.append(step)
        talk.parts.append(part)
        talk.warnings.append("parse error: %r" % exc)
        return talk


def _parse_text(text: str, path: str) -> Talk:
    text = text.replace("\r\n", "\n").replace("\r", "\n").replace("\t", "    ")
    lines = text.split("\n")
    blocks = parse_blocks(lines)
    warnings: List[str] = []

    title = ""
    # sections: (heading block or None, level, blocks)
    parts_raw: List[Tuple[Optional[Block], List[Tuple[Optional[Block], List[Block]]]]] = []
    pre: List[Block] = []
    for b in blocks:
        if b.kind == "heading" and b.level == 1 and not title and not parts_raw:
            title = inline.plain(b.text)
            continue
        if b.kind == "heading" and b.level == 2:
            parts_raw.append((b, [(None, [])]))
            continue
        if b.kind == "heading" and b.level == 3:
            if not parts_raw:
                parts_raw.append((None, [(None, [])]))
            parts_raw[-1][1].append((b, []))
            continue
        if not parts_raw:
            pre.append(b)
        else:
            parts_raw[-1][1][-1][1].append(b)

    talk = Talk(title=title or "Talk", path=path, warnings=warnings)

    if any(b.kind not in ("rule",) for b in pre):
        part = Part(title=title or "Preamble", heading="(preamble)", line=pre[0].line)
        beats, notes = build_beats(pre, warnings, "preamble")
        if beats or notes:
            part.steps.append(Step(title=part.title, beats=beats, notes=notes, line=pre[0].line))
            talk.parts.append(part)

    for head, sections in parts_raw:
        if head is not None:
            ptitle, pstart, pend = parse_part_heading(head.text)
            part = Part(title=ptitle, heading=inline.plain(head.text), start=pstart, end=pend,
                        talk=pstart is not None, line=head.line)
        else:
            part = Part(title="(untitled)", line=sections[0][1][0].line if sections[0][1] else 0)
        pi = len(talk.parts)
        where_p = "part %r" % part.title
        intro_head, intro_blocks = sections[0]
        has_children = len(sections) > 1
        intro_beats, intro_notes = build_beats(intro_blocks, warnings, where_p)
        if intro_beats or intro_notes or not has_children:
            part.steps.append(Step(title=part.title, start=part.start, end=part.end,
                                   beats=intro_beats, notes=intro_notes, part_index=pi,
                                   talk=part.talk, line=part.line))
        for sh, sblocks in sections[1:]:
            stitle, label, nums, sstart, send = parse_step_heading(sh.text)
            where = "%s%s" % (label or "step", (" %r" % stitle) if not label else "")
            beats, notes = build_beats(sblocks, warnings, where)
            if not beats:
                beats = [Beat(SAY, elems=[], line=sh.line)]
            if part.talk and sstart is None:
                warnings.append("%s (line %d): no time budget in heading; estimated" % (where, sh.line))
            part.steps.append(Step(title=stitle, label=label, numbers=nums, start=sstart, end=send,
                                   beats=beats, notes=notes, part_index=pi, talk=part.talk,
                                   line=sh.line))
        for st in part.steps:
            if not st.beats:
                st.beats = [Beat(SAY, elems=[], line=st.line)]
        talk.parts.append(part)

    for st in talk.steps:  # every step has at least one beat to stand on
        if not st.beats:
            st.beats = [Beat(SAY, elems=[], line=st.line)]
    _fill_windows(talk)
    _check_times(talk, warnings)
    return talk


def _fill_windows(talk: Talk) -> None:
    """Give untimed steps in timed parts a window between their neighbours.
    A part-intro step before timed steps gets [part start, first step start)."""
    for part in talk.parts:
        if not part.talk:
            continue
        steps = part.steps
        timed_children = [s for s in steps[1:] if s.start is not None]
        if steps and steps[0].title == part.title and not steps[0].label and timed_children:
            first = timed_children[0]
            if first.start is not None and part.start is not None and first.start > part.start:
                steps[0].start, steps[0].end = part.start, first.start
                steps[0].estimated = True
            else:  # no room before the first step: no budget of its own
                steps[0].start = steps[0].end = None
        for k, st in enumerate(steps):
            if st.start is not None and st.end is not None:
                continue
            if k == 0 and st.title == part.title and not st.label and timed_children:
                continue
            prev_end = next((s.end for s in reversed(steps[:k]) if s.end is not None), part.start)
            next_start = next((s.start for s in steps[k + 1:] if s.start is not None), part.end)
            st.start, st.end, st.estimated = prev_end, next_start, True


def _check_times(talk: Talk, warnings: List[str]) -> None:
    flow = [s for s in talk.steps if s.talk and s.start is not None and s.end is not None
            and s.end > s.start]
    for a, b in zip(flow, flow[1:]):
        if b.start != a.end:
            warnings.append("timing: %r ends %s but %r starts %s"
                            % (a.name, _f(a.end), b.name, _f(b.start)))
    for part in talk.parts:
        if not part.talk or part.start is None:
            continue
        for st in part.steps:
            if st.start is not None and (st.start < part.start or st.end > part.end):
                warnings.append("timing: %r (%s) is outside its part %r (%s)"
                                % (st.name, st.window, part.title, part.window))


def _f(x: Optional[int]) -> str:
    from .timing import fmt
    return fmt(x)


def load(path: str) -> Talk:
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        return parse_text(fh.read(), path)


# ---------------------------------------------------------------- helpers for copy


def command_pieces(beat: Beat) -> List[str]:
    """Split a command into the pieces you'd paste one at a time:
    SQL statements (ending in ';' or a psql backslash command) or shell
    lines (joining '\\' continuations). Comment-only lines are dropped."""
    text = beat.command or ""
    if beat.kind == PROMPT or beat.kind == RACE:
        return [text] if text.strip() else []
    lang = beat.elems[0].lang if beat.elems else ""
    target = LANG_TARGET.get(lang, "")
    pieces: List[str] = []
    cur: List[str] = []
    if target == "psql":
        for ln in text.split("\n"):
            s = ln.strip()
            if not cur and (not s or s.startswith("--")):
                continue
            cur.append(ln)
            code = re.sub(r"\s--\s.*$", "", s)
            if s.startswith("\\") or re.search(r"\\g\w*\b", code) or code.rstrip().endswith(";"):
                pieces.append("\n".join(cur).strip("\n"))
                cur = []
        if cur and "".join(cur).strip():
            pieces.append("\n".join(cur).strip("\n"))
        return pieces
    if target == "shell":
        for ln in text.split("\n"):
            s = ln.strip()
            if not cur and (not s or s.startswith("#")):
                continue
            cur.append(ln)
            if not s.endswith("\\"):
                pieces.append("\n".join(cur))
                cur = []
        if cur:
            pieces.append("\n".join(cur))
        return pieces
    return [text] if text.strip() else []


def placeholders(text: str) -> List[str]:
    seen: List[str] = []
    for m in PLACEHOLDER_RE.finditer(text or ""):
        if m.group(0) not in seen:
            seen.append(m.group(0))
    return seen


def html_unescape(s: str) -> str:
    return html.unescape(s)
