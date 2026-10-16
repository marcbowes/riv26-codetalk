"""The parsed talk: Talk -> Part -> Step -> Beat -> Elem."""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import List, Optional, Tuple

from . import inline
from .timing import fmt_range

# Beat kinds
SAY = "SAY"          # spoken paragraphs and lists
RUN = "RUN"          # a sql / sh command block
PROMPT = "PROMPT"    # an AI prompt to paste into the agent
EXPECT = "EXPECT"    # expected output that doesn't follow a command
KEY = "KEY"          # a ★ key message that must be said
TABLE = "TABLE"      # a table
RACE = "RACE"        # one row of a Pane A / Pane B table

BEAT_KINDS = (SAY, RUN, PROMPT, EXPECT, KEY, TABLE, RACE)


@dataclass
class Elem:
    """A renderable piece of markdown.

    kind is one of:
      para     spans
      heading  spans
      list     items: [(level, marker, spans)]
      code     lang, text           (a command or prompt)
      output   text                 (expected output)
      key      title (spans), children
      reader   children             (reader-only follow-along note)
      note     children             (presenter notes)
      table    header, rows, race
    """
    kind: str
    spans: List[inline.Span] = field(default_factory=list)
    items: List[Tuple[int, str, List[inline.Span]]] = field(default_factory=list)
    lang: str = ""
    text: str = ""
    title: List[inline.Span] = field(default_factory=list)
    children: List["Elem"] = field(default_factory=list)
    header: List[List[inline.Span]] = field(default_factory=list)
    rows: List[List[List[inline.Span]]] = field(default_factory=list)
    race: bool = False
    line: int = 0

    def plain(self) -> str:
        if self.kind in ("para", "heading"):
            return inline.spans_text(self.spans)
        if self.kind == "list":
            return " ".join(inline.spans_text(s) for _, _, s in self.items)
        if self.kind in ("code", "output"):
            return self.text
        if self.kind == "key":
            return " ".join(
                [inline.spans_text(self.title)] + [c.plain() for c in self.children]
            ).strip()
        if self.kind in ("reader", "note"):
            return " ".join(c.plain() for c in self.children)
        if self.kind == "table":
            cells = [inline.spans_text(c) for c in self.header]
            for r in self.rows:
                cells.extend(inline.spans_text(c) for c in r)
            return " | ".join(c for c in cells if c)
        return ""


@dataclass
class Beat:
    kind: str
    elems: List[Elem] = field(default_factory=list)
    target: str = ""          # psql, shell, AI agent, psql · Pane B ...
    command: str = ""         # what `c` copies
    expect: str = ""          # expected output attached to a command
    readers: List[Elem] = field(default_factory=list)  # hidden follow-along notes
    row: int = -1             # RACE: index into the table's rows
    line: int = 0

    @property
    def copyable(self) -> bool:
        return bool(self.command.strip())

    def summary(self, width: int = 70) -> str:
        if self.kind in (RUN, PROMPT):
            text = self.command
        elif self.kind == RACE:
            text = self.command or self.elems[0].plain()
        elif self.kind == KEY:
            text = "★ " + (self.elems[0].plain() if self.elems else "")
        elif self.kind == EXPECT:
            text = self.elems[0].text if self.elems else ""
        elif self.kind == TABLE:
            t = self.elems[0] if self.elems else None
            text = " | ".join(inline.spans_text(c) for c in t.header) if t else ""
        else:
            text = " ".join(e.plain() for e in self.elems)
        text = " ".join(text.split())
        if len(text) > width:
            text = text[: max(1, width - 1)].rstrip() + "…"
        return text


@dataclass
class Step:
    title: str
    label: str = ""                 # "Step 6", "Steps 4 and 5", or ""
    numbers: List[int] = field(default_factory=list)
    start: Optional[int] = None     # seconds into the talk
    end: Optional[int] = None
    estimated: bool = False         # window inferred from neighbours
    beats: List[Beat] = field(default_factory=list)
    notes: List[Elem] = field(default_factory=list)   # presenter notes
    part_index: int = 0
    talk: bool = False              # part of the timed talk flow
    line: int = 0

    @property
    def name(self) -> str:
        if self.label and self.title:
            return "%s · %s" % (self.label, self.title)
        return self.label or self.title

    @property
    def window(self) -> str:
        r = fmt_range(self.start, self.end)
        return ("≈" + r) if (r and self.estimated) else r

    @property
    def key(self) -> Tuple:
        """Stable identity across reloads."""
        if self.numbers:
            return ("n",) + tuple(self.numbers)
        return ("t", self.title)


@dataclass
class Part:
    title: str
    heading: str = ""
    start: Optional[int] = None
    end: Optional[int] = None
    talk: bool = False
    steps: List[Step] = field(default_factory=list)
    line: int = 0

    @property
    def window(self) -> str:
        return fmt_range(self.start, self.end)


@dataclass
class Talk:
    title: str
    parts: List[Part] = field(default_factory=list)
    path: str = ""
    warnings: List[str] = field(default_factory=list)

    @property
    def steps(self) -> List[Step]:
        return [s for p in self.parts for s in p.steps]

    @property
    def flow(self) -> List[int]:
        """Indices into steps that make up the timed talk. If nothing is
        timed, every step is in the flow."""
        steps = self.steps
        idx = [i for i, s in enumerate(steps) if s.talk]
        return idx or list(range(len(steps)))

    @property
    def span(self) -> Tuple[int, int]:
        steps = [s for s in self.steps if s.talk and s.start is not None and s.end is not None]
        if not steps:
            return 0, 0
        return min(s.start for s in steps), max(s.end for s in steps)
