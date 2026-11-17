#!/usr/bin/env python3
"""End-to-end: run generated demo-it steps in a real (terminal) Emacs with vterm
against a real cluster, then check what the panes show.

    AWS_PROFILE=bowes python3 stage/test/e2e.py [FROM-TO]

Uses your normal Emacs config (Doom), so vterm and demo-it come from there.
By default it runs the talk from the intro through step 7: creating the
cluster (its endpoint read from the output), both schemas, the loader,
EXPLAIN, the covering index (job ids copied from the output), the foreign
key race in two panes, and step 7's JSONB index, with every highlight and
Ctrl-R re-run. Afterwards it deletes the cluster step 1 created.

--finale runs the Two Regions segment on its own, against the deployed app
(demo/outputs.json): `open` is a no-op, and the load test is a local 400
req/s. It does run the real FIS experiment, so us-east-1 refuses connections
for a minute or so. Afterwards it stops any experiment left running and takes
the title edit back off.
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


E2E_STREAM = "dat401-cdc-e2e"   # the test's own Kinesis stream (the role allows dat401-cdc*)


def run(steps, timeout=900, jump=None, until=None, finale=False):
    out = tempfile.mkdtemp(prefix="dat401-e2e-")
    env = dict(os.environ, DAT401_E2E_STEPS=steps, DAT401_E2E_OUT=out, TERM="xterm-256color",
               DAT401_E2E_STREAM=E2E_STREAM)
    if finale:
        env.update(DAT401_E2E_FINALE="1", DAT401_E2E_COLD="1")
    if jump:
        env["DAT401_E2E_JUMP"] = jump
    if until:
        env["DAT401_E2E_UNTIL"] = until
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


JUMP_CHECKS = {
    "finished", "copies select exactly what's pasted", "cluster created in pane A", "both panes connected to psql",
    "race: B's lookups copied in from A", "race: race 1 leaves orphans; validation fails",
    "race: race 2 ends in OC000", "jsonb: filter, then the expression index",
    "no unresolved placeholders typed",
}


def option(args, name):
    if name in args:
        i = args.index(name)
        value = args[i + 1]
        del args[i:i + 2]
        return value
    return None


FINALE = "Multi-Region, in a minute"
LIVE_EDIT = " (live from re:Invent)"


def finale_cleanup():
    """Stop any experiment the run left going, and take the title edit back off
    (as demo/preflight.sh does)."""
    outputs = json.load(open(os.path.join(REPO, "demo", "outputs.json")))
    fis = outputs.get("fis") or {}
    stopped = []
    if fis.get("templateId"):
        r = subprocess.run(["aws", "fis", "list-experiments", "--region", fis["region"], "--output", "text",
                            "--query", "experiments[?experimentTemplateId=='%s' && (state.status=='running' "
                            "|| state.status=='initiating')].id" % fis["templateId"]],
                           capture_output=True, text=True)
        for exp in r.stdout.split():
            subprocess.run(["aws", "fis", "stop-experiment", "--region", fis["region"], "--id", exp],
                           capture_output=True)
            stopped.append(exp)
    region = outputs["regions"][0]
    token = subprocess.run(["aws", "dsql", "generate-db-connect-admin-auth-token", "--region", region["region"],
                            "--hostname", region["clusterEndpoint"], "--expires-in", "300"],
                           capture_output=True, text=True).stdout.strip()
    env = dict(os.environ, PGHOST=region["clusterEndpoint"], PGUSER="admin", PGDATABASE="postgres",
               PGSSLMODE="require", PGPASSWORD=token)
    r = subprocess.run(["psql", "-X", "-At", "-c", "update posts set title = replace(title, '%s', '') "
                        "where title like '%%%s'" % (LIVE_EDIT, LIVE_EDIT)],
                       env=env, capture_output=True, text=True)
    return "experiments stopped: %s; titles: %s" % (stopped or "none", (r.stdout or r.stderr).strip())


def delete_e2e_stream():
    r = subprocess.run(["aws", "kinesis", "delete-stream", "--stream-name", E2E_STREAM],
                       capture_output=True, text=True)
    return "deleted" if r.returncode == 0 else "none"


def main():
    args = sys.argv[1:]
    # --jump "Step 7 · ...": what C-F12 does from a fresh start (fast-forward),
    # then the rest as usual. --until "Scale": stop before that talk step.
    jump = option(args, "--jump")
    until = option(args, "--until")
    # --finale: the Two Regions segment on its own, against the deployed app.
    finale = "--finale" in args
    if finale:
        args.remove("--finale")
        jump = FINALE
    steps = args[0] if args else ""
    os.environ.setdefault("AWS_PROFILE", "bowes")
    out, status = run(steps, jump=jump, until=until, finale=finale)
    if finale:
        print("finale cleanup:", finale_cleanup())
    print("test Kinesis stream:", delete_e2e_stream())
    print("output in", out, "exit", status)
    print("deleted the cluster step 1 made:", delete_created_cluster(read(out, "pane-a.txt")))
    if read(out, "error.txt"):
        print("ERROR:", read(out, "error.txt"))
    a, b, shell = read(out, "pane-a.txt"), read(out, "pane-b.txt"), read(out, "pane-shell.txt")
    events = read(out, "pane-events.txt")
    west, east, load = read(out, "pane-us-west-2.txt"), read(out, "pane-us-east-1.txt"), read(out, "pane-load.txt")
    agent = read(out, "agent.txt")

    def count_of(pane):
        m = re.search(r"select count\(\*\) from posts;\s*count\s*-+\s*(\d+)", pane)
        return m and m.group(1)
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
        ("highlights cleared after the 3", 3 in counts and 0 in counts[counts.index(3):]),
        ("DPU totals highlighted", sum(1 for m in marks if "DPU" in m) >= 3),
        ("index highlighted after adding it", any("Index Only Scan Backward using posts_recent_idx" in m
                                                  for m in marks)),
        ("ids and B's statements copied from the screen, not typed",
         int(read(out, "copies.txt").strip() or 0) >= 9),
        ("copies select exactly what's pasted", os.path.exists(os.path.join(out, "copy-misses.txt"))
         and not read(out, "copy-misses.txt").strip()),
        ("race: B's lookups copied in from A", b.count("select id as post_id from posts where slug = 'hello-dsql'") >= 2),
        ("race: race 1 leaves orphans; validation fails", "violates foreign key constraint" in a),
        ("race: race 2 ends in OC000", "OC000" in b),
        ("re-runs come from history (Ctrl-R), 3 of them, none typed instead",
         (read(out, "recalls.txt").split() or ["0", "0"]) == ["3", "0"]),
        ("jsonb: filter, then the expression index",
         "Rows Removed by Filter" in a and "Index Scan Backward using posts_category_idx" in a),
        ("plan highlights: scan, sort, filter, cond, time",
         all(any(k in m for m in marks) for k in ("Scan using", "Sort Method", "Rows Removed", "Index Cond",
                                                   "Execution Time"))),
        ("pane B: exports pasted, then psql", "export PGPASSWORD" in b and "\n❯ psql" in b),
        ("DDL typed without indentation, in lower case", "(> id uuid primary key" in a),
        ("rows: comment inserted via :'post_id'", "values (:'post_id', 'First!');\nINSERT 0 1" in a),
        ("tables created", a.count("CREATE TABLE") >= 3),
        ("finale: console and site opened from the shell",
         ": open https://us-west-2.console.aws.amazon.com/dsql/create-cluster/home" in shell
         and bool(re.search(r": open https://\S+\.cloudfront\.net", shell))),
        ("finale: both prompts shown whole, never submitted",
         agent.count("build a typescript crud api") == 1 and agent.count("now a web app for the blog api") == 1
         and "then call every op in both regions to check it works" in agent),
        ("finale: a psql in each Region",
         "dsql.us-west-2.on.aws" in west and "dsql.us-east-1.on.aws" in east
         and "postgres=>" in west and "postgres=>" in east),
        ("finale: the same count in both Regions", bool(count_of(west)) and count_of(west) == count_of(east)),
        ("finale: us-east-1's statements copied from us-west-2", "select count(*) from posts;" in east),
        ("finale: the edit in us-west-2, read straight back in us-east-1",
         "UPDATE 1" in west and LIVE_EDIT.strip() in east.split("UPDATE")[-1]),
        ("finale: us-east-1's re-read came from history",
         (read(out, "recalls.txt").split() or ["0", "0"]) == ["1", "0"]),
        ("finale: FIS experiment started, then stopped",
         bool(re.search(r"\nEXP[A-Za-z0-9]{8,}", shell)) and bool(re.search(r"stop-experiment .*--id EXP", shell))
         and bool(re.search(r"\n(stopping|stopped)\b", shell))),
        ("finale: copies select exactly what's pasted", os.path.exists(os.path.join(out, "copy-misses.txt"))
         and not read(out, "copy-misses.txt").strip()),
        ("finale: no errors in the shell", not re.search(r"no matches found|command not found|error", shell, re.I)),
        ("finale: traffic moved off us-east-1, then back",
         "sending its traffic to us-west-2" in read(out, "load-fis.txt")
         and "answering again" in read(out, "load-fis.txt") and "Failovers: 1" in load),
        ("finale: no unresolved placeholders typed",
         not re.search(r"<(blog-url|fis-template-id|fis-experiment-id|us-[a-z]+-\d-endpoint)>", shell + west + east)),
        ("real schema loaded with \\i", "INSERT 0 1" in a),
        ("loader ran in the shell pane", "Done in" in shell),
        ("bad plan shown (full scan + sort)", "top-N heapsort" in a),
        ("<job_id> filled from the pane", bool(re.search(r"wait_for_job\('[a-z0-9]{26}'\)", a))),
        ("index jobs waited for", "succeeded" in a and " t" in a),
        ("covering index used", "Index Only Scan Backward using posts_recent_idx" in a),
        ("cdc: stream created, then active", '"status": "CREATING"' in events
         and "wait stream-active" in events),
        ("cdc: the comment shows up in the CDC viewer", "Hello from the stage" in events),
        ("no unresolved placeholders typed", not re.search(r"<(job_id|cluster-endpoint|cluster-id)>", a + b)),
    ]
    if finale:
        checks = [c for c in checks if c[0].startswith("finale:") or c[0] == "finished"]
    elif jump and "Step 9" in jump:
        checks = [c for c in checks if c[0].startswith("cdc:") or c[0] in
                  ("finished", "no unresolved placeholders typed")]
    elif jump:
        checks = [c for c in checks if c[0] in JUMP_CHECKS]
    else:
        checks = [c for c in checks if not c[0].startswith(("cdc:", "finale:"))]

    width = max(len(c) for c, _ in checks)
    ok = True
    for name, passed in checks:
        ok &= bool(passed)
        print("  %-*s  %s" % (width, name, "ok" if passed else "FAIL"))
    if not ok:
        print("progress:", read(out, "progress.txt")[:300])
        print("last screen:", " ".join(read(out, "screen.txt").split())[-600:])
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
