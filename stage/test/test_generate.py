"""Tests for stage/generate.py: python3 -m unittest discover -s stage/test"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
import generate  # noqa: E402


class Clean(unittest.TestCase):
    def test_strips_sql_comments(self):
        text = "-- both panes, one line at a time\nSELECT 1;   -- note\nSELECT '--not a comment';"
        self.assertEqual(generate.clean_command(text, "sql"), "SELECT 1;\nSELECT '--not a comment';")

    def test_strips_shell_comments(self):
        self.assertEqual(generate.clean_command("cd demo   # go\n# only a comment\nls", "sh"), "cd demo\nls")

    def test_rehearsal_job_ids_become_placeholders(self):
        self.assertEqual(generate.clean_command("CALL sys.wait_for_job('sdtn7gjobfeglgjbysgk3pog4i');", "sql"),
                         "CALL sys.wait_for_job('<job_id>');")


class Panes(unittest.TestCase):
    def test_targets(self):
        self.assertEqual(generate.panes_for("psql", "SELECT 1;"), ["a"])
        self.assertEqual(generate.panes_for("psql · Pane B", "COMMIT;"), ["b"])
        self.assertEqual(generate.panes_for("psql · both panes", "\\c"), ["a", "b"])
        self.assertEqual(generate.panes_for("shell · both panes", "psql"), ["a", "b"])
        self.assertEqual(generate.panes_for("shell · second terminal", "./sql/05-load-sample-data.sh"), ["shell"])
        self.assertEqual(generate.panes_for("shell", "./events/target/release/events --stream x"), ["events"])
        self.assertEqual(generate.panes_for("shell", "cd demo/loadgen\n./target/release/loadgen --rps 1"), ["load"])

    def test_layouts(self):
        self.assertEqual(generate.layout_for(["a", "b", "a"]), ["a", "b"])
        self.assertEqual(generate.layout_for(["shell", "a"]), ["a", "shell"])
        self.assertEqual(generate.layout_for(["load"]), ["load"])


class Elisp(unittest.TestCase):
    def test_string_escaping(self):
        self.assertEqual(generate.el('a "b" \\gset'), '"a \\"b\\" \\\\gset"')


class RealTalk(unittest.TestCase):
    def test_every_talk_command_is_a_step(self):
        talk = generate.load_talk()
        out = generate.generate(talk)
        cmds = [b for p in talk["parts"] if p["talk"] for s in p["steps"] if s["talk"]
                for b in s["beats"] if b["kind"] in ("RUN", "RACE", "PROMPT")]
        self.assertEqual(out.count("(dat401-run ") + out.count("(dat401-prompt "), len(cmds))
        self.assertIn("(dat401-layout 'a 'b)", out)          # the race
        self.assertNotIn("sdtn7gjobfeglgjbysgk3pog4i", out)  # rehearsal job id replaced

    def test_generated_file_is_current(self):
        self.assertEqual(generate.main(["--check"]), 0)


if __name__ == "__main__":
    unittest.main()
