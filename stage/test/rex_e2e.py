#!/usr/bin/env python3
"""End-to-end for the Rex stage (stage/rex): start a "DAT401 stage" session,
press F12 (`rex do stage/rex/dat401.lua cmd=next`) through part of the talk,
waiting by eye where a presenter would, then check what the panes show.

    AWS_PROFILE=bowes python3 stage/test/rex_e2e.py            # intro to step 7
    AWS_PROFILE=bowes python3 stage/test/rex_e2e.py --finale   # the Two Regions segment

The default run makes a cluster (deleted afterwards). --finale jumps to the
Two Regions segment against the deployed app (demo/outputs.json), with `open`
a visible no-op and a local 400 req/s load test, but the real FIS experiment;
afterwards it stops any experiment left running and takes the title edit off.
Its own state directory, so a real stage's state is never touched.
"""
import json
import os
import re
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
RUNNER = os.path.join(REPO, "stage", "rex", "dat401.lua")
sys.path.insert(0, HERE)
import e2e  # noqa: E402  (finale_cleanup, delete_created_cluster)

STATE = tempfile.mkdtemp(prefix="dat401-rex-e2e-")
ENV = dict(os.environ, DAT401_REX_STATE=STATE, DAT401_REX_TEST="1", DAT401_AGENT="cat",
           DAT401_AGENT_READY=".", DAT401_TYPING_SPEED="0.05")


def rex(*args, timeout=600):
    r = subprocess.run(["rex", *args], env=ENV, capture_output=True, text=True, timeout=timeout)
    return (r.stdout + r.stderr).strip()


def stage(cmd, **kw):
    return rex("do", RUNNER, "cmd=" + cmd, *("%s=%s" % kv for kv in kw.items()))


def state():
    with open(os.path.join(STATE, "state.json")) as f:
        return json.load(f)


def pane(name):
    st = state()
    block = st["panes"].get(name)
    if not block:
        return ""
    return rex("capture", "-s", st["session_id"], "-b", block, "--trim", "--unwrap")


def east_rate(load):
    """us-east-1's sending rate in the load generator's Regions table, or None.
    (Read from the table: in a pane nobody's looking at, 80x24, the Events
    panel is one line, and the failover's event soon scrolls out of it.)"""
    m = re.findall(r"us-eas\S*\s+\S+\s+([\d.,]+)/s", load)
    return float(m[-1].replace(",", "")) if m else None


def wait_moved(timeout):
    """Wait for us-east-1's traffic to have moved: its rate down to the probes."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        r = east_rate(pane("load"))
        if r is not None and r < 20:
            return True
        time.sleep(2)
    return False


def wait_text(name, pattern, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if re.search(pattern, pane(name)):
            return True
        time.sleep(2)
    return False


def main():
    args = sys.argv[1:]
    finale = "--finale" in args
    os.environ.setdefault("AWS_PROFILE", "bowes")
    ENV.setdefault("AWS_PROFILE", os.environ["AWS_PROFILE"])
    log = []
    notes = {}
    try:
        print(stage("start")[:200])
        st = state()
        notes["agents prepared"] = sorted((st.get("agents") or {}).keys())
        if finale:
            print(stage("goto", title="Multi-Region, in a minute"))
            last = None
        else:
            last = 51           # the end of step 7
        while True:
            out = stage("next")
            log.append(out)
            print("  " + out[:150])
            if "Error" in out or "the talk is over" in out:
                break
            n = state()["step"]
            text = out
            # What a presenter waits for by eye.
            if "press 19 " in text:     # the loader, in the shell
                wait_text("shell", r"Done in", 240)
            if "Next: close agent" in text or "Next: quit_agent" in text:
                notes.setdefault("agent screens", []).append(pane(state().get("current_agent") or "claude"))
            if "press 72 " in text:     # the load test
                wait_text("load", r"STEADY", 180)
                time.sleep(5)
            if "press 73 " in text:     # start-experiment: wait for it to bite
                notes["failover"] = wait_moved(150)
                notes["load during"] = pane("load")
                with open(os.path.join(STATE, "load-during.txt"), "w") as f:
                    f.write(notes["load during"])
            if "press 74 " in text:     # stop-experiment
                notes["back"] = wait_text("load", r"answering again", 150)
                notes["load after"] = pane("load")
                with open(os.path.join(STATE, "load-after.txt"), "w") as f:
                    f.write(notes["load after"])
                time.sleep(12)
                st = state()
                rex("send", "-s", st["session_id"], "-b", st["panes"]["load"], "q")
                wait_text("load", r"SUMMARY", 60)
                break
            if last and n >= last:
                break
        panes = {p: pane(p) for p in ("a", "b", "shell", "load", "us-west-2", "us-east-1")}
        for p, t in panes.items():
            with open(os.path.join(STATE, "pane-%s.txt" % p), "w") as f:
                f.write(t)
        print("panes saved in", STATE)
    finally:
        if not finale:
            print("deleted the cluster step 1 made:", e2e.delete_created_cluster(pane("a")))
        else:
            print("finale cleanup:", e2e.finale_cleanup())
        print(stage("reset"))

    a, b, shell, west, east = panes["a"], panes["b"], panes["shell"], panes["us-west-2"], panes["us-east-1"]
    load = panes["load"]
    no_errors = not any("Error" in l for l in log)
    if finale:
        agents = "\n".join(notes.get("agent screens", []))
        checks = [
            ("every press ran", no_errors),
            ("both agents started, prompts in, at start", notes["agents prepared"] == ["claude-1", "claude-2"]),
            ("both prompts shown whole, never submitted",
             agents.count("build a typescript crud api") == 1 and agents.count("now a web app for the blog api") == 1),
            ("the site opened from the shell, the console only cued",
             "console.aws.amazon.com" not in shell and bool(re.search(r": open https://\S+\.cloudfront\.net", shell))),
            ("a psql in each Region", "dsql.us-west-2.on.aws" in west and "dsql.us-east-1.on.aws" in east
             and "postgres=>" in west and "postgres=>" in east),
            ("timed in both", all("Timing is on." in p and "Timing is off." in p for p in (west, east))),
            ("the edit by the pasted id", bool(re.search(r"where id = '[0-9a-f]{8}-[0-9a-f-]{27}';\s*UPDATE 1", west))),
            ("the edit read back in us-east-1", "(live from re:Invent)" in east),
            ("FIS started, then stopped", bool(re.search(r"\nEXP[A-Za-z0-9]{8,}", shell))
             and bool(re.search(r"\n(stopping|stopped)\b", shell))),
            ("traffic moved off us-east-1, then back", bool(notes.get("failover")) and bool(notes.get("back"))
             and "Failovers: 1" in load),
            ("no errors in the shell", not re.search(r"no matches found|command not found|error", shell, re.I)),
            ("no unresolved placeholders",
             not re.search(r"<(blog-url|fis-template-id|fis-experiment-id|post-id|us-[a-z]+-\d-endpoint)>", shell + west + east)),
        ]
    else:
        checks = [
            ("every press ran", no_errors),
            ("both agents started at start", notes["agents prepared"] == ["claude-1", "claude-2"]),
            ("cluster created, its endpoint pasted in both panes",
             '"status": "CREATING"' in a and bool(re.search(r"export PGHOST=[a-z0-9]{26}\.dsql", a))
             and bool(re.search(r"export PGHOST=[a-z0-9]{26}\.dsql", b))),
            ("both panes connected", "postgres=>" in a and "postgres=>" in b),
            ("tables created", a.count("CREATE TABLE") >= 3),
            ("loader ran in the shell", "Done in" in shell),
            ("<job_id> pasted", bool(re.search(r"wait_for_job\('[a-z0-9]{26}'\)", a))),
            ("covering index used", "Index Only Scan Backward using posts_recent_idx" in a),
            ("race 1 orphans; validation fails", "violates foreign key constraint" in a),
            ("race 2 ends in OC000", "OC000" in b),
            ("jsonb: filter, then the expression index",
             "Rows Removed by Filter" in a and "Index Scan Backward using posts_category_idx" in a),
            ("no unresolved placeholders", not re.search(r"<(job_id|cluster-endpoint|cluster-id)>", a + b)),
        ]
    width = max(len(c) for c, _ in checks)
    ok = True
    for name, passed in checks:
        ok &= bool(passed)
        print("  %-*s  %s" % (width, name, "ok" if passed else "FAIL"))
    if not ok:
        print("log:", open(os.path.join(STATE, "log.txt")).read()[-1500:] if os.path.exists(os.path.join(STATE, "log.txt")) else "")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
