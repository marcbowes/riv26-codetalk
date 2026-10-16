"""Command line: speaker [TALK.md] [--dump | --json | --check] ..."""
from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import asdict
from typing import List, Optional

from . import parser
from .model import RUN, PROMPT, RACE, Talk

HERE = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))


def find_talk(arg: Optional[str]) -> str:
    """An explicit path, else TALK.md next to the speaker/ folder, else in
    the current directory."""
    if arg:
        return os.path.abspath(os.path.expanduser(arg))
    for cand in (os.path.join(os.path.dirname(HERE), "TALK.md"),
                 os.path.join(HERE, "TALK.md"),
                 os.path.join(os.getcwd(), "TALK.md")):
        if os.path.isfile(cand):
            return cand
    return os.path.join(os.path.dirname(HERE), "TALK.md")


def dump(talk: Talk, out=sys.stdout, width: int = 72) -> None:
    w = out.write
    w("TALK  %s\n" % talk.title)
    w("      %s\n" % talk.path)
    a, b = talk.span
    flow = set(talk.flow)
    n_beats = sum(len(talk.steps[i].beats) for i in flow)
    w("      %d parts, %d steps (%d in the talk flow, %s–%s), %d beats in flow\n"
      % (len(talk.parts), len(talk.steps), len(flow), _f(a), _f(b), n_beats))
    for part in talk.parts:
        tag = part.window if part.talk else "not in talk flow"
        w("\nPART  %s  [%s]\n" % (part.title, tag))
        for st in part.steps:
            win = st.window or "-"
            w("  STEP  %s  [%s]%s\n" % (st.name, win, "" if st.talk else "  (not in flow)"))
            if st.notes:
                n_items = sum(len(e.items) if e.kind == "list" else 1 for e in st.notes)
                first = " ".join(st.notes[0].plain().split())
                w("        NOTES   %s  (%d item%s)\n" % (_cut(first, width - 12), n_items,
                                                     "" if n_items == 1 else "s"))
            for k, bt in enumerate(st.beats, 1):
                tgt = ("[%s] " % bt.target) if bt.target else ""
                extra = ""
                if bt.kind == "SAY":
                    np = len([e for e in bt.elems if e.kind != "heading"])
                    extra = "  (%d blocks)" % np if np > 1 else ""
                elif bt.kind in (RUN, PROMPT):
                    nl = len(bt.command.strip("\n").split("\n")) if bt.command else 0
                    extra = "  (%d line%s)" % (nl, "" if nl == 1 else "s") if nl > 1 else ""
                w("     %3d %-7s %s%s%s\n" % (k, bt.kind, tgt, bt.summary(max(20, width - len(tgt) - 4)),
                                            extra))
                if bt.expect:
                    first = next((ln for ln in bt.expect.split("\n") if ln.strip()), "")
                    if bt.kind == RACE:
                        w("             → expect  %s\n" % _cut(first.strip(), width - 10))
                    else:
                        nl = len(bt.expect.split("\n"))
                        w("             ↳ EXPECT  %s%s\n" % (_cut(first.strip(), width - 18),
                                                         "  (%d lines)" % nl if nl > 1 else ""))
                for r in bt.readers:
                    w("             · reader  %s\n" % _cut(" ".join(r.plain().split()), width - 10))
    if talk.warnings:
        w("\nWARNINGS (%d): things in the markdown speaker had to guess about\n" % len(talk.warnings))
        for msg in talk.warnings:
            w("  - %s\n" % msg)


def _f(x):
    from .timing import fmt
    return fmt(x)


def _cut(s: str, n: int) -> str:
    n = max(8, n)
    return s if len(s) <= n else s[: n - 1] + "…"


def to_json(talk: Talk) -> str:
    def clean(o):
        if isinstance(o, dict):
            return {k: clean(v) for k, v in o.items() if v not in ("", [], None, -1, False) or k in ("talk",)}
        if isinstance(o, list):
            return [clean(v) for v in o]
        if isinstance(o, tuple):
            return [clean(v) for v in o]
        return o
    d = asdict(talk)
    d["flow"] = talk.flow
    return json.dumps(clean(d), indent=1, ensure_ascii=False)


def main(argv: Optional[List[str]] = None) -> int:
    ap = argparse.ArgumentParser(
        prog="speaker",
        description="A terminal teleprompter for TALK.md. Keys: n next, p back, N/P step, "
                    "g go to, c copy, s clock, ? help, q quit.")
    ap.add_argument("talk", nargs="?", help="path to TALK.md (default: ../TALK.md next to speaker/)")
    ap.add_argument("--dump", action="store_true", help="print parts → steps → beats and exit")
    ap.add_argument("--json", action="store_true", help="print the parsed talk as JSON and exit")
    ap.add_argument("--check", action="store_true",
                    help="print markup warnings; exit 1 if there are any")
    ap.add_argument("--goto", metavar="STEP", help="start at a step (number, time or title search)")
    ap.add_argument("--all", action="store_true",
                    help="include non-talk parts (takeaways, cleanup, ...) in n/N navigation")
    ap.add_argument("--no-color", action="store_true", help="don't use colors (also: NO_COLOR=1)")
    ap.add_argument("--clock", metavar="MM:SS", help="start with the talk clock at this time (paused)")
    args = ap.parse_args(argv)

    path = find_talk(args.talk)
    if not os.path.isfile(path):
        sys.stderr.write("speaker: can't find %s (pass the path to TALK.md)\n" % path)
        return 2
    talk = parser.load(path)

    if args.dump:
        try:
            dump(talk)
        except BrokenPipeError:
            pass
        return 0
    if args.json:
        print(to_json(talk))
        return 0
    if args.check:
        for msg in talk.warnings:
            print(msg)
        return 1 if talk.warnings else 0

    if not sys.stdin.isatty() or not sys.stdout.isatty():
        sys.stderr.write("speaker: needs a terminal (use --dump for text output)\n")
        return 2
    from . import app
    return app.run(path, talk, args)
