#!/usr/bin/env python3
"""Generate stage/dat401-talk.el (the demo-it steps) from TALK.md.

    python3 stage/generate.py           # write stage/dat401-talk.el
    python3 stage/generate.py --check   # exit 1 if it's out of date

It reads TALK.md through speaker's parser (speaker --json), so both tools
agree on what a command is and where it runs. Each talk step becomes:

    a section + layout change, merged into its first action;
    one demo-it step per command (typed into its pane), race row, AI prompt
    (shown in a side window), code excerpt (the real file, with the
    excerpt's lines highlighted) and ★ key point (a banner).

A code excerpt is a fence with a path after the language (```ts
demo/api/src/db.ts). Its lines must appear in that file verbatim and in
order; a `...` line (or `// ...`, `# ...`, `-- ...`) skips ahead. Generation
fails if an excerpt no longer matches its file.

Spoken text and expected output don't become steps: you say those.
"""
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
OUT = os.path.join(HERE, "dat401-talk.el")

JOB_ID_LITERAL = re.compile(r"(wait_for_job\(')[a-z0-9]{26}('\))")


class ExcerptError(Exception):
    pass


ELISION = {"...", "…", "// ...", "# ...", "-- ...", "/* ... */"}


def excerpt_ranges(excerpt, source):
    """1-based (first, last) line ranges of SOURCE that EXCERPT shows. Lines
    between elisions must be consecutive in SOURCE; an elision searches ahead."""
    src = [l.rstrip() for l in source.splitlines()]
    chunks, cur = [], []
    for line in excerpt.splitlines():
        if line.strip() in ELISION:
            if cur:
                chunks.append(cur)
            cur = []
        else:
            cur.append(line.rstrip())
    if cur:
        chunks.append(cur)
    if not chunks:
        raise ExcerptError("the excerpt is empty")
    ranges, at = [], 0
    for chunk in chunks:
        n = len(chunk)
        for i in range(at, len(src) - n + 1):
            if src[i:i + n] == chunk:
                ranges.append((i + 1, i + n))
                at = i + n
                break
        else:
            raise ExcerptError("not found (in order) in the file: %r" % chunk[0])
    return ranges


def spans_text(spans):
    return "".join(s[0] for s in spans)


def elem_text(e):
    if e.get("spans"):
        return spans_text(e["spans"])
    if e.get("items"):
        return "\n".join("• " + spans_text(i[2]) for i in e["items"])
    return e.get("text", "")


def strip_comment(line, marker):
    """Drop a trailing `marker` comment outside quotes (needs whitespace before it)."""
    quote = None
    i = 0
    while i < len(line):
        c = line[i]
        if quote:
            if c == quote:
                quote = None
        elif c in ("'", '"'):
            quote = c
        elif line.startswith(marker, i) and (i == 0 or line[i - 1] in " \t"):
            return line[:i].rstrip()
        i += 1
    return line.rstrip()


def clean_command(text, lang):
    """What gets typed: no comment lines, no trailing comments, real placeholders."""
    marker = "--" if lang in ("sql", "psql") else "#"
    out = []
    for line in text.splitlines():
        if line.strip().startswith(marker):
            continue
        line = strip_comment(line, marker)
        line = JOB_ID_LITERAL.sub(r"\1<job_id>\2", line)
        if line.strip():
            out.append(line)
    return "\n".join(out)


def panes_for(target, command):
    """speaker's target label -> the panes to type into."""
    t = (target or "").lower()
    if "both panes" in t:
        return ["a", "b"]
    if "pane b" in t:
        return ["b"]
    if "pane a" in t:
        return ["a"]
    if t.startswith("psql"):
        return ["a"]
    first = command.strip().splitlines()[0] if command.strip() else ""
    if first.startswith("./events/") or "events/target" in first:
        return ["events"]
    if "loadgen" in command:
        return ["load"]
    return ["shell"]


def layout_for(panes):
    s = set(panes)
    if "load" in s:
        return ["load"]
    if "b" in s:
        return ["a", "b"]
    if "events" in s:
        return ["a", "events"]
    if "shell" in s:
        return ["a", "shell"]
    return ["a"]


def el(s):
    """An Emacs Lisp string literal."""
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def el_panes(panes):
    return "'" + panes[0] if len(panes) == 1 else "'(" + " ".join(panes) + ")"


def step_forms(step):
    """(comment, form) pairs for one talk step's beats."""
    forms = []
    for b in step["beats"]:
        kind = b["kind"]
        if kind in ("RUN", "RACE"):
            lang = (b["elems"][0].get("lang") or "sql") if b["elems"] and b["elems"][0].get("kind") == "code" else "sql"
            if kind == "RACE":
                lang = "sql"
            text = clean_command(b.get("command") or "", lang)
            if not text:
                continue
            panes = panes_for(b.get("target"), text)
            forms.append((panes, "%s %s" % (kind.lower(), b.get("target") or ""),
                          "(dat401-run %s %s)" % (el_panes(panes), el(text))))
        elif kind == "PROMPT":
            produced = b.get("expect") or ""
            forms.append(([], "prompt", "(dat401-prompt %s%s)" % (
                el(b.get("command") or ""), " " + el(produced) if produced else "")))
        elif kind == "SHOW":
            path = b["target"]
            try:
                with open(os.path.join(REPO, path)) as f:
                    ranges = excerpt_ranges(b["elems"][0]["text"], f.read())
            except (OSError, ExcerptError) as e:
                raise SystemExit("TALK.md line %s: excerpt of %s: %s" % (b["line"], path, e))
            forms.append(([], "code %s" % path, "(dat401-code %s '(%s))" % (
                el(path), " ".join("(%d . %d)" % r for r in ranges))))
        elif kind == "KEY":
            e = b["elems"][0]
            title = "★ " + spans_text(e.get("title") or [])
            body = "\n".join(elem_text(c) for c in e.get("children") or [])
            forms.append(([], "key point", "(dat401-key %s %s)" % (el(title), el(body))))
    return forms


def generate(talk):
    lines = [
        ";;; dat401-talk.el --- The DAT401 demo-it steps  -*- lexical-binding: t; -*-",
        ";;",
        ";; GENERATED by stage/generate.py from TALK.md. Don't edit: change TALK.md and",
        ";; run `python3 stage/generate.py`.",
        "",
        ";;; Code:",
        "",
        "(require 'dat401-stage)",
        "",
    ]
    steps = []      # (comment, form)
    index = []      # (title, 1-based step number)
    first = True
    for part in talk["parts"]:
        if not part["talk"]:
            continue
        for s in part["steps"]:
            if not s["talk"]:
                continue
            title = s["title"] if not s.get("label") else "%s · %s" % (s["label"], s["title"])
            forms = step_forms(s)
            if first:
                # demo-it runs its first step on start: just set the scene.
                steps.append(("start: %s" % title,
                              "(progn (dat401-section %s) (dat401-layout 'a))" % el(title)))
                index.append((title, len(steps)))
                first = False
            if not forms:
                continue
            # The layout the first command needs. Later commands add panes as
            # they need them (step 1: create the cluster in A, then connect
            # in A and B).
            p0, c0, f0 = forms[0]
            layout = layout_for(p0) if p0 else ["a"]
            index.append((title, len(steps) + 1))
            head = "(progn (dat401-section %s) (dat401-layout %s) %s)" % (
                el(title), " ".join("'" + p for p in layout), f0)
            steps.append(("%s (TALK.md line %s): %s" % (title, s["line"], c0), head))
            for _, c, f in forms[1:]:
                steps.append((c, f))
    lines.append("(setq dat401--steps-index")
    lines.append("      '(" + "\n        ".join("(%s . %d)" % (el(t), n) for t, n in index) + "))")
    lines.append("")
    lines.append("(demo-it-create")
    lines.append(" :advanced-mode")
    for n, (comment, form) in enumerate(steps, 1):
        lines.append(" ;; %d. %s" % (n, comment.replace("\n", " ")))
        lines.append(" " + form)
    lines.append(" )")
    lines.append("")
    lines.append(";;; dat401-talk.el ends here")
    return "\n".join(lines) + "\n"


def load_talk():
    out = subprocess.run([sys.executable, os.path.join(REPO, "speaker", "speaker"), "--json",
                          os.path.join(REPO, "TALK.md")],
                         check=True, capture_output=True, text=True).stdout
    return json.loads(out)


def main(argv):
    text = generate(load_talk())
    if "--check" in argv:
        current = open(OUT).read() if os.path.exists(OUT) else ""
        if current != text:
            print("stage/dat401-talk.el is out of date: run python3 stage/generate.py")
            return 1
        print("stage/dat401-talk.el is up to date")
        return 0
    with open(OUT, "w") as f:
        f.write(text)
    print("wrote %s (%d steps)" % (os.path.relpath(OUT, REPO), text.count("\n ;; ")))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
