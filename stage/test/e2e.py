#!/usr/bin/env python3
"""End-to-end: run generated demo-it steps in a real (terminal) Emacs with vterm
against a real cluster, then check what the panes show.

    AWS_PROFILE=bowes python3 stage/test/e2e.py [FROM-TO]

Uses your normal Emacs config (Doom), so vterm and demo-it come from there.
By default it runs the talk from the intro through step 7: creating the
cluster (its endpoint read from the output), the schema, the step 4 code
views, the loader, EXPLAIN, both async indexes (with <job_id> copied from
the output), the foreign key race in two panes, and step 8's JSONB index,
with every EXPLAIN highlight and Ctrl-R re-run. Afterwards it deletes the cluster step 1 created.
"""
import fcntl
import json
import os
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))


def run(steps, timeout=900):
    out = tempfile.mkdtemp(prefix="dat401-e2e-")
    env = dict(os.environ, DAT401_E2E_STEPS=steps, DAT401_E2E_OUT=out, TERM="xterm-256color")
    if not steps:
        env.pop("DAT401_E2E_STEPS")
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(REPO)
        os.execvpe("emacs", ["emacs", "-nw", "--eval",
                             '(progn (add-to-list (quote load-path) "%s/stage") (load "%s/e2e.el"))'
                             % (REPO, HERE)], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 200, 0, 0))
    deadline = time.time() + timeout
    status = None
    screen = b""
    while time.time() < deadline:
        r, _, _ = select.select([fd], [], [], 0.5)
        if r:
            try:
                screen = (screen + os.read(fd, 65536))[-20000:]
            except OSError:
                pass
        done, st = os.waitpid(pid, os.WNOHANG)
        if done:
            status = st
            break
    if status is None:
        os.kill(pid, 9)
        os.waitpid(pid, 0)
    # What Emacs last drew (escape codes stripped): shows a stuck prompt.
    text = re.sub(rb"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07|\x1b[()][0-9A-B]|\x1b[=>]", b"", screen)
    with open(os.path.join(out, "screen.txt"), "wb") as f:
        f.write(text)
    return out, status


def read(out, name):
    p = os.path.join(out, name)
    return open(p, errors="replace").read() if os.path.exists(p) else ""


def delete_created_cluster(pane_a):
    """Step 1's create-cluster turns deletion protection on: turn it off, delete."""
    m = re.findall(r'"identifier": "([a-z0-9]{26})"', pane_a)
    if not m:
        return None
    ident = m[-1]
    aws = ["aws", "dsql", "--region", os.environ.get("AWS_REGION", "us-west-2")]
    subprocess.run(aws + ["update-cluster", "--identifier", ident, "--no-deletion-protection-enabled"],
                   check=False, capture_output=True)
    r = subprocess.run(aws + ["delete-cluster", "--identifier", ident], capture_output=True, text=True)
    status = json.loads(r.stdout).get("status") if r.returncode == 0 else r.stderr.strip()
    return "%s: %s" % (ident, status)


def main():
    steps = sys.argv[1] if len(sys.argv) > 1 else ""
    os.environ.setdefault("AWS_PROFILE", "bowes")
    out, status = run(steps)
    print("output in", out, "exit", status)
    print("deleted the cluster step 1 made:", delete_created_cluster(read(out, "pane-a.txt")))
    if read(out, "error.txt"):
        print("ERROR:", read(out, "error.txt"))
    a, b, shell = read(out, "pane-a.txt"), read(out, "pane-b.txt"), read(out, "pane-shell.txt")
    marks = [line.split(" ", 1)[1] for line in read(out, "marks.txt").splitlines() if " " in line]
    counts = [m.count('"') // 2 for m in marks]
    checks = [
        ("finished", bool(read(out, "done.txt"))),
        ("cluster created in pane A", '"status": "CREATING"' in a),
        ("its endpoint typed in both panes", bool(re.search(r"export PGHOST=[a-z0-9]{26}\.dsql", a))
         and bool(re.search(r"export PGHOST=[a-z0-9]{26}\.dsql", b))),
        ("both panes connected to psql", "postgres=>" in a and "postgres=>" in b),
        ("highlights: identifier, + endpoint, + pasted id",
         [1, 2, 3] == [c for c in counts if c][:3]),
        ("highlights cleared", bool(counts) and counts[-1] == 0 and 3 in counts),
        ("DPU totals highlighted", sum(1 for m in marks if "DPU" in m) >= 3),
        ("index highlighted after adding it", any("Index Only Scan Backward using posts_recent_idx" in m
                                                  for m in marks)),
        ("ids and B's statements copied from the screen, not typed",
         int(read(out, "copies.txt").strip() or 0) >= 9),
        ("step 7: B's lookups copied in from A", b.count("select id as post_id from posts where slug = 'hello-dsql'") >= 2),
        ("step 7: race 1 leaves orphans; validation fails", "violates foreign key constraint" in a),
        ("step 7: race 2 ends in OC000", "OC000" in b),
        ("re-runs come from history (Ctrl-R), 3 of them",
         a.count("reverse-i-search") >= 3 and "failed reverse-i-search" not in a),
        ("step 6: the ignored index (same plan, re-run)",
         a.count("Index Only Scan using posts_pkey on public.posts") >= 2),
        ("step 8: filter, then the expression index",
         "Rows Removed by Filter" in a and "Index Scan Backward using posts_category_idx" in a),
        ("plan highlights: scan, sort, filter, cond, time",
         all(any(k in m for m in marks) for k in ("Scan using", "Sort Method", "Rows Removed", "Index Cond",
                                                   "Execution Time"))),
        ("pane B: exports pasted, then psql", "export PGPASSWORD" in b and "\n❯ psql" in b),
        ("DDL typed without indentation, in lower case", "(> id uuid primary key" in a),
        ("step 2: comment inserted via :'post_id'", "values (:'post_id', 'First!');\nINSERT 0 1" in a),
        ("tables created", a.count("CREATE TABLE") >= 3),
        ("step 4 code shown", "db.transactionWithRetry(" in read(out, "code.txt")),
        ("step 4 prompt typed into the agent, never submitted",
         read(out, "agent.txt").count("build a TypeScript API for my blog") == 1),
        ("real schema loaded with \\i", "INSERT 0 1" in a),
        ("loader ran in the shell pane", "Done in" in shell),
        ("bad plan shown (full scan + sort)", "top-N heapsort" in a),
        ("<job_id> filled from the pane", bool(re.search(r"wait_for_job\('[a-z0-9]{26}'\)", a))),
        ("index jobs waited for", "succeeded" in a and " t" in a),
        ("covering index used", "Index Only Scan Backward using posts_recent_idx" in a),
        ("no unresolved placeholders typed", not re.search(r"<(job_id|cluster-endpoint|cluster-id)>", a + b)),
    ]
    width = max(len(c) for c, _ in checks)
    ok = True
    for name, passed in checks:
        ok &= passed
        print("  %-*s  %s" % (width, name, "ok" if passed else "FAIL"))
    if not ok:
        print("progress:", read(out, "progress.txt")[:300])
        print("last screen:", " ".join(read(out, "screen.txt").split())[-600:])
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
