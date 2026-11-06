#!/usr/bin/env python3
"""End-to-end: run generated demo-it steps in a real (terminal) Emacs with vterm
against a real cluster, then check what the panes show.

    AWS_PROFILE=bowes python3 stage/test/e2e.py [FROM-TO]

Uses your normal Emacs config (Doom), so vterm and demo-it come from there.
By default it runs the talk from the intro through step 6: creating the
cluster (its endpoint read from the output), the schema, the step 4 code
views, the loader, EXPLAIN, and both async indexes (with <job_id> read from
the output). Afterwards it deletes the cluster step 1 created.
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
    while time.time() < deadline:
        r, _, _ = select.select([fd], [], [], 0.5)
        if r:
            try:
                os.read(fd, 65536)
            except OSError:
                pass
        done, st = os.waitpid(pid, os.WNOHANG)
        if done:
            status = st
            break
    if status is None:
        os.kill(pid, 9)
        os.waitpid(pid, 0)
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
    checks = [
        ("finished", bool(read(out, "done.txt"))),
        ("cluster created in pane A", '"status": "CREATING"' in a),
        ("its endpoint typed in both panes", bool(re.search(r"export PGHOST=[a-z0-9]{26}\.dsql", a))
         and bool(re.search(r"export PGHOST=[a-z0-9]{26}\.dsql", b))),
        ("both panes connected to psql", "postgres=>" in a and "postgres=>" in b),
        ("tables created", a.count("CREATE TABLE") >= 3),
        ("step 4 code shown", "db.transactionWithRetry(" in read(out, "code.txt")),
        ("identity-column error shown", "explicit cache size" in a),
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
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
