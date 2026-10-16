import os
import re
import unittest

import helpers
from prompter import inline, parser
from prompter.model import EXPECT, KEY, PROMPT, RACE, RUN, SAY, TABLE


def kinds(step):
    return [b.kind for b in step.beats]


def find(talk, name):
    for s in talk.steps:
        if s.name == name or s.title == name:
            return s
    raise AssertionError("no step %r in %r" % (name, [s.name for s in talk.steps]))


class Fixture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.talk = parser.load(helpers.FIXTURE)

    def test_title_and_parts(self):
        t = self.talk
        self.assertEqual(t.title, "Mini talk (fixture)")
        self.assertEqual([p.title for p in t.parts],
                         ["Mini talk (fixture)", "Takeaways", "Run of show", "Opening", "Build",
                          "Cleaning up"])
        self.assertEqual([p.talk for p in t.parts], [False, False, False, True, True, False])
        self.assertEqual(t.span, (0, 600))

    def test_flow_is_timed_steps_only(self):
        names = [self.talk.steps[i].name for i in self.talk.flow]
        self.assertEqual(names, ["Opening", "Step 1 · Make a table", "Steps 2 and 3 · Race and load",
                                 "Step 4 · Wrap the build"])

    def test_part_without_steps_is_one_step(self):
        st = find(self.talk, "Opening")
        self.assertEqual((st.start, st.end), (0, 120))
        self.assertEqual(kinds(st), [SAY])
        self.assertEqual(len(st.beats[0].elems), 2)   # two paragraphs, one beat
        self.assertTrue(st.notes)                     # presenter notes kept
        self.assertIn("Have the terminal open.", st.notes[0].plain())

    def test_step_headings(self):
        st = find(self.talk, "Step 1 · Make a table")
        self.assertEqual((st.label, st.numbers, st.start, st.end), ("Step 1", [1], 120, 240))
        st = find(self.talk, "Steps 2 and 3 · Race and load")
        self.assertEqual(st.numbers, [2, 3])

    def test_beats_of_step_1(self):
        st = find(self.talk, "Step 1 · Make a table")
        self.assertEqual(kinds(st), [SAY, SAY, RUN, KEY, SAY, PROMPT, SAY])
        say1, say2, run, key, _, prompt, last = st.beats
        # bold lead-in starts a new beat; the plain paragraph after it doesn't
        self.assertEqual(len(say1.elems), 2)
        self.assertTrue(say2.summary().startswith("Now in psql."))
        # sql is a command with its output attached; the pane hint is picked up
        self.assertEqual(run.command, "CREATE TABLE t (id int PRIMARY KEY);")
        self.assertEqual(run.expect, "CREATE TABLE")
        self.assertEqual(run.target, "psql · pane B")
        # the ★ quote is a key point with its bold lead as the title
        self.assertEqual(inline.spans_text(key.elems[0].title), "DSQL difference: keys matter.")
        # "No agent?" quote is a hidden reader note, not a beat
        self.assertEqual(len(key.readers), 1)
        self.assertIn("aws thing create --name x", key.readers[0].plain())
        # prompts are unwrapped for pasting
        self.assertEqual(prompt.kind, PROMPT)
        self.assertEqual(prompt.target, "AI agent")
        self.assertEqual(prompt.command, "Build me an API for table t using the cluster <cluster-endpoint>.")
        # <!-- beat --> forces a break
        self.assertEqual(last.summary(), "...and that was the wrong cluster.")

    def test_race_table_is_one_beat_per_row(self):
        st = find(self.talk, "Steps 2 and 3 · Race and load")
        self.assertEqual(kinds(st), [RUN, RACE, RACE, RACE, RACE, SAY, EXPECT, SAY, RUN])
        races = [b for b in st.beats if b.kind == RACE]
        self.assertEqual([(b.target, b.command, b.expect) for b in races], [
            ("psql · Pane A", "BEGIN;", ""),
            ("psql · Pane B", "BEGIN;", ""),
            ("psql · Pane A", "COMMIT;", "ok"),
            ("psql · Pane B", "COMMIT;", "ERROR: conflict (OC000)"),
        ])
        self.assertTrue(all(b.elems[0] is races[0].elems[0] for b in races))  # one shared table
        self.assertEqual(st.beats[0].target, "psql · both panes")
        self.assertEqual(st.beats[8].target, "shell · second terminal")
        # the following-along <details> is a reader note on the last beat
        self.assertEqual(len(st.beats[8].readers), 1)

    def test_plain_table(self):
        st = find(self.talk, "Step 4 · Wrap the build")
        self.assertEqual(kinds(st), [TABLE])
        tbl = st.beats[0].elems[0]
        self.assertFalse(tbl.race)
        self.assertEqual(len(tbl.rows), 2)

    def test_inline_markup_rendered(self):
        st = find(self.talk, "Opening")
        spans = st.beats[0].elems[0].spans
        text = inline.spans_text(spans)
        self.assertNotIn("https://", text)
        self.assertNotIn("**", text)
        self.assertIn(("the opening", "b"), spans)
        self.assertIn(("inline code", "c"), spans)

    def test_nested_list(self):
        st = find(self.talk, "Takeaways")
        items = st.beats[0].elems[0].items
        self.assertEqual([(lv, mk) for lv, mk, _ in items],
                         [(0, "1."), (0, "2."), (1, "-"), (1, "-"), (0, "")])

    def test_warnings_flag_standalone_output(self):
        self.assertTrue(any("standalone EXPECT" in w for w in self.talk.warnings))


class RealTalk(unittest.TestCase):
    """Generic checks against the live TALK.md, which is being edited: count
    things with simple regexes and compare with the parser."""

    @classmethod
    def setUpClass(cls):
        if not os.path.exists(helpers.TALK):
            raise unittest.SkipTest("no TALK.md")
        with open(helpers.TALK, encoding="utf-8") as fh:
            cls.text = fh.read()
        cls.talk = parser.parse_text(cls.text, helpers.TALK)

    def _outside_quotes_and_details(self):
        """Lines that aren't inside a blockquote or a <details> block."""
        depth, out = 0, []
        for line in self.text.split("\n"):
            depth += len(re.findall(r"<details\b", line))
            if depth == 0 and not line.lstrip().startswith(">"):
                out.append(line)
            depth -= len(re.findall(r"</details>", line))
        return out

    def test_every_step_heading_found(self):
        headings = [ln[4:].strip() for ln in self.text.split("\n") if ln.startswith("### ")]
        self.assertTrue(headings)
        labelled = [s for s in self.talk.steps if s.label]
        self.assertEqual(len(labelled), len([h for h in headings if re.match(r"Steps? \d", h)]))
        numbers = sorted(n for s in self.talk.steps for n in s.numbers)
        expected = sorted(int(n) for h in headings if re.match(r"Steps? \d", h)
                          for n in re.findall(r"\d+", h.split("·")[0]))
        self.assertEqual(numbers, expected)
        for s in labelled:
            self.assertTrue(s.talk, s.name)
            self.assertIsNotNone(s.start, s.name)
            self.assertGreater(len(s.beats), 0, s.name)

    def test_star_callouts_are_key_points(self):
        n_star = len(re.findall(r"^> \*\*★", self.text, re.M))
        keys = [b for s in self.talk.steps for b in s.beats if b.kind == KEY]
        self.assertEqual(len(keys), n_star)
        self.assertGreater(n_star, 5)
        for b in keys:
            self.assertTrue(b.summary().startswith("★ DSQL difference"), b.summary())

    def test_fences_typed(self):
        lines = self._outside_quotes_and_details()
        # An opening fence may carry a target after the language ("```sql pane A").
        fences = [m.group(1) for m in (re.match(r"^```(\w*)(?:[ \t].*)?$", ln) for ln in lines) if m]
        opening = fences[0::2]   # fences alternate open / close
        cmds = [f for f in opening if f in ("sql", "sh", "prompt")]
        plain = [f for f in opening if f in parser.OUTPUT_LANGS]   # ``` or ```text etc.
        beats = [b for s in self.talk.steps for b in s.beats]
        run = [b for b in beats if b.kind == RUN]
        prompt = [b for b in beats if b.kind == PROMPT]
        self.assertEqual(len(run) + len(prompt), len(cmds))
        self.assertEqual(len(prompt), opening.count("prompt"))
        for b in run:
            self.assertIn(b.elems[0].lang, ("sql", "sh"))
            self.assertTrue(b.target.startswith(("psql", "shell")), b.target)
            self.assertTrue(b.copyable)
        for b in prompt:
            self.assertEqual(b.target, "AI agent")
        attached = [b for b in beats if b.kind in (RUN, PROMPT) and b.expect]
        standalone = [b for b in beats if b.kind == EXPECT]
        self.assertEqual(len(attached) + len(standalone), len(plain))

    def test_starts_at_intro_and_flow_is_contiguous(self):
        flow = [self.talk.steps[i] for i in self.talk.flow]
        self.assertEqual(flow[0].start, 0)
        self.assertIn("Intro", flow[0].name)
        for a, b in zip(flow, flow[1:]):
            self.assertEqual(a.end, b.start, (a.name, b.name))
        non_talk = [s.name for s in self.talk.steps if not s.talk]
        for name in ("Following along", "Cleaning up", "Troubleshooting"):
            self.assertTrue(any(name in n for n in non_talk), name)

    def test_presenter_notes_not_beats(self):
        n_notes = len(re.findall(r"<summary>(?:<b>)?Presenter notes", self.text))
        with_notes = [s for s in self.talk.steps if s.notes]
        self.assertEqual(len(with_notes), n_notes)
        for s in self.talk.steps:
            for b in s.beats:
                self.assertNotIn("Presenter notes", b.summary())

    def test_race_tables(self):
        races = [b for s in self.talk.steps for b in s.beats if b.kind == RACE]
        n_tables = len(re.findall(r"^\| *\| *Pane A *\| *Pane B *\|", self.text, re.M))
        self.assertEqual(len({id(b.elems[0]) for b in races}), n_tables)
        for b in races:
            self.assertRegex(b.target, r"Pane [AB]")
            self.assertTrue(b.command)


class Robustness(unittest.TestCase):
    def test_garbage_never_raises(self):
        samples = [
            "", "\n\n", "```", "```sql\nSELECT 1;", "> ", "<details>", "</details>",
            "| a |\n|---|", "### Step x · · ·", "## ·", "## 9:00–1:00 · Backwards",
            "<details><summary>Presenter notes</summary>inline</details>",
            "<details>\n<summary>Presenter\nnotes</summary>\n\n<details>\n<summary>x</summary>\nin\n</details>\n</details>",
            "**unclosed bold and `unclosed code and [link(",
            "- a\n  - b\n    - c\n- d\n\n   orphan",
            "> **★** \n> ```\n> unclosed in quote",
            "\t\ttabs\r\nand CRLF\r\n",
            "| `a|b` | c |\n|---|---|\n| x |",
            "### Step 1 · Only time · 1:00–2:00\n### Step 2 · No time",
        ]
        for s in samples:
            talk = parser.parse_text(s)
            for st in talk.steps:
                self.assertTrue(st.beats, repr(s))

    def test_unclosed_fence_warned(self):
        talk = parser.parse_text("## 0:00–1:00 · A\n\n```sql\nSELECT 1;\n")
        self.assertEqual(talk.steps[0].beats[0].command, "SELECT 1;\n")
        self.assertTrue(any("unclosed" in w for w in talk.warnings))

    def test_untimed_step_in_timed_part_gets_estimate(self):
        talk = parser.parse_text("## 0:00–10:00 · P\n\n### Step 1 · A · 0:00–4:00\n\nx\n\n"
                                 "### Step 2 · B\n\ny\n")
        s2 = talk.steps[1]
        self.assertEqual((s2.start, s2.end, s2.estimated), (240, 600, True))
        self.assertTrue(s2.window.startswith("≈"))

    def test_explicit_target_in_info_string(self):
        talk = parser.parse_text("## 0:00–1:00 · A\n\n```sql pane B\nSELECT 1;\n```\n")
        self.assertEqual(talk.steps[0].beats[0].target, "psql · pane B")

    def test_ambiguous_pane_hint_ignored(self):
        talk = parser.parse_text("## 0:00–1:00 · A\n\nReconnect pane B and re-run it in both panes:\n\n"
                                 "```sql\nSELECT 1;\n```\n")
        self.assertEqual(talk.steps[0].beats[1].target, "psql")


class Inline(unittest.TestCase):
    def test_spans(self):
        self.assertEqual(inline.parse("a **b** `c` [d](http://x) *e*"),
                         [("a ", ""), ("b", "b"), (" ", ""), ("c", "c"), (" d ", ""), ("e", "i")])

    def test_link_with_code_text(self):
        self.assertEqual(inline.parse("[`sql/x.sql`](sql/x.sql)"), [("sql/x.sql", "c")])

    def test_literal_when_unbalanced(self):
        self.assertEqual(inline.plain("2 * 3 and a_b_c and **open"), "2 * 3 and a_b_c and **open")

    def test_placeholders_and_tags(self):
        self.assertEqual(inline.plain("<b>Bold</b> to <cluster-A-id>"), "Bold to <cluster-A-id>")
        self.assertEqual(inline.plain("a &amp; b"), "a & b")

    def test_width(self):
        self.assertEqual(inline.width("abc"), 3)
        self.assertEqual(inline.width("日本"), 4)


class Commands(unittest.TestCase):
    def _beat(self, lang, text):
        talk = parser.parse_text("## 0:00–1:00 · A\n\n```%s\n%s\n```\n" % (lang, text))
        return talk.steps[0].beats[0]

    def test_sql_pieces(self):
        b = self._beat("sql", "-- both panes\nSELECT 1 AS x \\gset\nEXPLAIN ANALYZE\nSELECT 2;\n"
                              "\\i file.sql\nINSERT INTO t VALUES (1);   -- 1\n--  trailing comment")
        self.assertEqual(parser.command_pieces(b),
                         ["SELECT 1 AS x \\gset", "EXPLAIN ANALYZE\nSELECT 2;", "\\i file.sql",
                          "INSERT INTO t VALUES (1);   -- 1"])

    def test_shell_pieces(self):
        b = self._beat("sh", "# comment\nexport A=1\naws thing \\\n  --flag\ncd x")
        self.assertEqual(parser.command_pieces(b), ["export A=1", "aws thing \\\n  --flag", "cd x"])

    def test_unwrap_prompt(self):
        self.assertEqual(parser.unwrap_prompt("one\ntwo\n\nthree\n- a\n- b"), "one two\n\nthree\n- a\n- b")

    def test_placeholders(self):
        self.assertEqual(parser.placeholders("x <job_id> y <job_id> metadata->>'c' <cluster-A-id>"),
                         ["<job_id>", "<cluster-A-id>"])


if __name__ == "__main__":
    unittest.main()
