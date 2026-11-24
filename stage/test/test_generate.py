"""Tests for stage/generate.py: python3 -m unittest discover -s stage/test"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
import generate  # noqa: E402


class Clean(unittest.TestCase):
    def test_strips_sql_comments(self):
        text = "-- both panes, one line at a time\nSELECT 1;   -- note\nSELECT '--not a comment';"
        self.assertEqual(generate.clean_command(text, "sql"), "select 1;\nselect '--not a comment';")

    def test_strips_shell_comments(self):
        self.assertEqual(generate.clean_command("cd demo   # go\n# only a comment\nls", "sh"), "cd demo\nls")

    def test_sql_is_typed_in_lower_case_except_quotes(self):
        self.assertEqual(generate.clean_command(
            "INSERT INTO posts (title) VALUES ('Hello, DSQL') RETURNING \"Id\";", "sql"),
            "insert into posts (title) values ('Hello, DSQL') returning \"Id\";")
        self.assertEqual(generate.clean_command("CALL sys.wait_for_job('<job_id>');", "sql"),
                         "call sys.wait_for_job('<job_id>');")
        self.assertEqual(generate.clean_command("SELECT * FROM t WHERE m->>'Category' = 'Rust' \\gset", "sql"),
                         "select * from t where m->>'Category' = 'Rust' \\gset")
        # Shell keeps its case.
        self.assertEqual(generate.clean_command("export PGHOST=X", "sh"), "export PGHOST=X")

    def test_typed_lines_lose_their_indentation(self):
        self.assertEqual(generate.clean_command("CREATE TABLE t (\n  id uuid PRIMARY KEY,\n  x text\n);", "sql"),
                         "create table t (\nid uuid primary key,\nx text\n);")

    def test_rehearsal_job_ids_become_placeholders(self):
        self.assertEqual(generate.clean_command("CALL sys.wait_for_job('sdtn7gjobfeglgjbysgk3pog4i');", "sql"),
                         "call sys.wait_for_job('<job_id>');")


class Panes(unittest.TestCase):
    def test_targets(self):
        self.assertEqual(generate.panes_for("psql", "SELECT 1;"), ["a"])
        self.assertEqual(generate.panes_for("psql · Pane B", "COMMIT;"), ["b"])
        self.assertEqual(generate.panes_for("shell · pane A", "aws dsql create-cluster"), ["a"])
        self.assertEqual(generate.panes_for("shell · pane events", "aws kinesis create-stream"), ["events"])
        self.assertEqual(generate.panes_for("psql · both panes", "\\c"), ["a", "b"])
        self.assertEqual(generate.panes_for("shell · both panes", "psql"), ["a", "b"])
        self.assertEqual(generate.panes_for("shell · second terminal", "./sql/05-load-sample-data.sh"), ["shell"])
        self.assertEqual(generate.panes_for("shell", "./events/target/release/events --stream x"), ["events"])
        self.assertEqual(generate.panes_for("shell", "cd demo/loadgen\n./target/release/loadgen --rps 1"), ["load"])

    def test_layouts(self):
        self.assertEqual(generate.layout_for(["a", "b", "a"]), ["a", "b"])
        self.assertEqual(generate.layout_for(["shell", "a"]), ["a", "shell"])
        self.assertEqual(generate.layout_for(["load"]), ["load"])


class Excerpt(unittest.TestCase):
    SRC = "a\nimport x;\nimport y;\n\nfunction f() {\n  body;\n  return 1;\n}\n"

    def test_consecutive_lines(self):
        self.assertEqual(generate.excerpt_ranges("import x;\nimport y;", self.SRC), [(2, 3)])

    def test_elision_skips_ahead(self):
        self.assertEqual(generate.excerpt_ranges("import x;\n// ...\n  return 1;\n}", self.SRC),
                         [(2, 2), (7, 8)])

    def test_drift_fails(self):
        with self.assertRaises(generate.ExcerptError):
            generate.excerpt_ranges("import z;", self.SRC)
        with self.assertRaises(generate.ExcerptError):   # out of order
            generate.excerpt_ranges("import y;\n...\nimport x;", self.SRC)
        with self.assertRaises(generate.ExcerptError):   # not consecutive without an elision
            generate.excerpt_ranges("import x;\nfunction f() {", self.SRC)


class Cues(unittest.TestCase):
    def test_directions(self):
        self.assertEqual(generate.cue_form("highlight identifier", "a", 1), "(dat401-highlight 'a \"identifier\")")
        self.assertEqual(generate.cue_form("clear", "a", 1), "(dat401-unhighlight)")
        self.assertEqual(generate.cue_form("open pane B", "a", 1), "(dat401-open 'b)")
        self.assertEqual(generate.cue_form("close pane shell", "a", 1), "(dat401-close 'shell)")
        self.assertEqual(generate.cue_form("highlight dpu", "a", 1), "(dat401-highlight 'a \"dpu\")")
        self.assertEqual(generate.cue_form("highlight time dpu", "a", 1),
                         "(dat401-highlight 'a \"time\" \"dpu\")")

    def test_recall_runs_from_history(self):
        step = {"beats": [{"kind": "RUN", "target": "psql · recall: desc limit", "line": 1,
                           "command": "EXPLAIN ANALYZE VERBOSE\nSELECT 1 ORDER BY x DESC LIMIT 20;",
                           "elems": [{"kind": "code", "lang": "sql"}]}]}
        self.assertEqual(generate.step_forms(step)[0][2],
                         "(dat401-recall 'a \"explain analyze verbose\nselect 1 order by x desc limit 20;\" "
                         "\"desc limit\")")

    def test_close_rides_along_with_the_next_command(self):
        step = {"beats": [
            {"kind": "RUN", "target": "shell", "command": "./sql/05-load-sample-data.sh",
             "elems": [{"kind": "code", "lang": "sh"}], "line": 1},
            {"kind": "CUE", "target": "close pane shell", "line": 2},
            {"kind": "RUN", "target": "psql", "command": "EXPLAIN SELECT 1;",
             "elems": [{"kind": "code", "lang": "sql"}], "line": 3},
        ]}
        forms = generate.step_forms(step)
        self.assertEqual(len(forms), 2)
        self.assertEqual(forms[1][2], "(progn (dat401-close 'shell) (dat401-run 'a \"explain select 1;\"))")
        with self.assertRaises(SystemExit):
            generate.cue_form("highlight", "a", 1)


class Elisp(unittest.TestCase):
    def test_string_escaping(self):
        self.assertEqual(generate.el('a "b" \\gset'), '"a \\"b\\" \\\\gset"')


class RealTalk(unittest.TestCase):
    def test_every_talk_command_is_a_step(self):
        talk = generate.load_talk()
        out = generate.generate(talk)
        cmds = [b for p in talk["parts"] if p["talk"] for s in p["steps"] if s["talk"]
                for b in s["beats"] if b["kind"] in ("RUN", "RACE", "PROMPT", "SHOW", "CUE")]
        self.assertEqual(out.count("(dat401-run ") + out.count("(dat401-paste ") + out.count("(dat401-agent ")
                         + out.count("(dat401-code ") + out.count("(dat401-highlight ")
                         + out.count("(dat401-unhighlight)") + out.count("(dat401-open ")
                         + out.count("(dat401-close ") + out.count("(dat401-recall ")
                         + out.count("(dat401-cue ") + out.count("(dat401-send-keys "),
                         len(cmds))
        self.assertGreaterEqual(out.count("(dat401-recall 'a "), 2)       # re-runs come from history
        self.assertNotIn("(dat401-key ", out)                # no banners: it should look live
        self.assertEqual(out.count("(dat401-agent "), out.count("(dat401-quit-agent)"))  # always closed
        self.assertNotIn("\\n", out[out.index("(dat401-agent "):].split(")", 1)[0])  # no newline: never submitted
        self.assertIn("(dat401-paste 'b \"export PGHOST=<cluster-endpoint>", out)
        self.assertIn("(dat401-highlight 'a \"identifier\")", out)   # step 1
        self.assertIn("(dat401-copy-into 'a 'b ", out)       # the race: B copies from A
        # Two Regions: a pane each, the second copies from the first, a re-run
        # from history, the shell joins what's on screen, and cues for the browser.
        self.assertIn("(dat401-layout 'us-west-2 'us-east-1)", out)
        self.assertIn("(dat401-paste 'us-east-1 \"export PGHOST=<us-east-1-endpoint>", out)
        self.assertIn("(dat401-copy-into 'us-west-2 'us-east-1 ", out)
        self.assertIn("(dat401-recall 'us-east-1 ", out)
        self.assertIn("(dat401--show 'shell) (dat401-run 'shell \"aws fis start-experiment", out)
        self.assertIn("<fis-experiment-id>", out)
        self.assertEqual(out.count('(dat401-cue "browser")'), 2)
        self.assertIn("(dat401-send-keys 'load \"f\")", out)    # 50/50, then hit the target
        self.assertNotIn("sdtn7gjobfeglgjbysgk3pog4i", out)  # rehearsal job id replaced

    def test_rex_presses(self):
        talk = generate.load_talk()
        el = generate.generate(talk)
        lua = generate.generate_rex(talk)
        # Highlights are the presenter's (the mouse) in Rex: no presses for them.
        self.assertNotIn('op = "highlight"', lua)
        highlight_steps = el.count("\n (dat401-highlight ") + el.count("\n (dat401-unhighlight)")
        self.assertEqual(lua.count("\n    -- "), el.count("\n ;; ") - highlight_steps)
        # The console's address is a press of its own; the browser cues ride
        # along with the FIS commands.
        self.assertIn('{op = "cue", text = "console: us-west-2.console.aws.amazon.com/dsql/create-cluster/home"}', lua)
        self.assertEqual(lua.count('{op = "cue", text = "browser"}'), 2)
        # Both prompts are listed, for cmd=start to prepare.
        self.assertEqual(len(generate.talk_prompts(talk)), 2)
        self.assertEqual(lua.count('{op = "agent", '), 2)
        self.assertNotIn("\\u", lua)       # Lua 5.1 has no \u escapes

    def test_generated_file_is_current(self):
        self.assertEqual(generate.main(["--check"]), 0)


if __name__ == "__main__":
    unittest.main()
