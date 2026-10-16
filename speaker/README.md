# speaker

A terminal teleprompter that plays [`../TALK.md`](../TALK.md). It shows what to say
next, which command to run, and which key point you must not skip, and it tells you
whether you're ahead of or behind the plan.

There's nothing to install. It needs Python 3.9 or newer, using only the standard
library (`curses`). The python3 that ships with macOS works, and so does Homebrew's.

```sh
speaker/speaker                    # plays ../TALK.md (found relative to this folder)
speaker/speaker path/to/TALK.md    # or any file that follows the same conventions
python3 speaker/speaker            # the same thing, if the file isn't executable

speaker/speaker --dump             # print parts → steps → beats, then exit
speaker/speaker --check            # list markup speaker had to guess about (exit 1 if any)
speaker/speaker --json             # the parsed talk as JSON
speaker/speaker --goto 7           # start at step 7 (a number, a time like 21:30, or title words)
speaker/speaker --clock 21:30      # start with the clock at 21:30, paused (handy after a restart)
speaker/speaker --goto 3 --clock plan   # start at step 3 with the clock on plan (10:30), paused
speaker/speaker --all              # let n/N walk the non-talk parts too
speaker/speaker --no-color         # or set NO_COLOR=1
```

## Keys

| Key | Does |
|---|---|
| `n`, space, `→`, PgDn, Enter | next beat (it moves on to the next step at the end of one) |
| `p`, `←`, PgUp | previous beat |
| `N` / `P` | next / previous step |
| `g` | go to a step by number (`6`), by talk time (`17:30`), or by fuzzy title (`race`). ↑↓ selects, Enter jumps, Esc cancels |
| `c` | copy this beat's command or prompt to the clipboard, and flash a confirmation |
| `C` | copy the next single statement of the command, cycling (for `\gset` lines, which must be pasted one at a time) |
| `s` | start or pause the talk clock |
| `t` | reset the clock to 0:00 |
| `T` or `=` | set the clock to the plan for this beat, so you're exactly on time. For after a restart: `g` to your step, then `T`. Keeps running if it was running |
| `r` | reload TALK.md, keeping your place (the footer tells you when the file changed on disk) |
| `f` | show or hide reader-only follow-along notes |
| `j` / `k`, `↓` / `↑` (`J` / `K` half a page) | scroll, if a beat is taller than the screen |
| `?` | help: keys and color legend |
| `q`, Ctrl-C | quit (the terminal is restored, and speaker prints where you stopped) |

PgDn and PgUp step through beats because most presentation clickers send those keys.

## The screen

```
 Build it ▸ Step 6 · An index, without downtime         ▄  █▀▀ █▀█  SEC
 beat 3/12 │ step 17:00–20:00  ON TIME · 1:40 left      ▀█▀ ▀▀█ █ █  BEHIND
 clock 18:20 / 45:00  ██████████▏███████░░◆░░░│░░░ 39%  ▀▀▀ ▀▀▀  vs plan
   (earlier beats, dimmed)                        │ ✎ PRESENTER NOTES
▌  the current beat, bold, with a bar on the left │ • ...
   (later beats)                                  │ RUN OF SHOW
 NEXT ▸ ▶ RUN psql: SELECT job_id, status ...     │ ▶17:00 Step 6 · ...
 n/␣ next  p back  N/P step  g go to  c copy ...
```

- **Header:** the part and step; beat *x*/*y*; the step's time budget and how the
  clock sits inside it; the talk clock against the planned total (`18:20 / 45:00`);
  and a progress bar. The bar fills to where you are in the script, `◆` marks the
  clock, and the ticks are part boundaries.
- **Delta badge**, in big digits on the right (or a reverse-video badge on small
  terminals): how far you are from the plan.
- **Body:** the whole step. The current beat is scrolled into view, earlier beats
  are dimmed, and `▲`/`▼` mean there's more above or below.
- **Sidebar**, on terminals 110 or more columns wide: the step's presenter notes and the
  run of show. On narrower terminals the notes sit at the top of the step.
- **NEXT line:** the next beat, or the next step and its start time.
- **Footer:** key hints, or a flash after copy, reload and so on.

### Pace

- **Delta badge.** Each beat gets an equal slice of its step's time budget. That
  slice is the window in which you're on time. A one-beat step, like the intro, is
  on time for its whole budget.
  - Inside the current beat's window: on time.
  - Before it starts: ahead by the gap.
  - After it ends: behind by the overrun.

  The badge shows:
  - `--`: the clock hasn't started (press `s`), or you're outside the talk flow.
  - Green `±0` **ON TIME**: inside the window, or within 30 s of it.
  - Green `-1m` **AHEAD**: more than 30 s early.
  - Amber `+45s` **BEHIND**: 30–90 s late.
  - Red `+2m` **BEHIND**: 90 s or more late.

  Minutes are rounded down, so `+2m` means at least two minutes behind.
- **Step indicator**, next to the step's window. Green while the clock is inside the
  window. Amber in its last minute, or its last half for steps shorter than two minutes.
  Red once you're past the step's end. Cyan if the clock hasn't reached the step yet.

### What the colors mean

| Looks like | Means |
|---|---|
| plain text (bold, with `▌`, when current) | **SAY**: what you say |
| ` ▶ RUN ` on green, then the target (`psql`, `psql · pane B`, `shell · second terminal`), then the code on a dark block | a **command** to run. Press `c` to copy it |
| ` ✦ PROMPT ` on magenta, target `AI agent` | an **AI prompt** to paste into the agent |
| grey text behind a `│` bar, labelled `EXPECT` | **expected output**: what the audience will see |
| ` ★ KEY POINT ` on yellow. The whole block turns black-on-yellow when it's current | a **key message that must be said** |
| ` ✎ PRESENTER NOTES ` on teal | **private notes** for the speaker |
| `┆ READER NOTE` in blue (hidden until `f`) | follow-along material for readers, not for the stage |
| ` ⇄ RACE ` table with one row highlighted, plus a `▶ NOW` line | a two-pane race; each row is a beat |
| orange underlined `<placeholder>` in code | edit it before running (the copy flash reminds you too) |

Without colors (`NO_COLOR`, `--no-color`, or a terminal without color), the same
labels are shown with bold, reverse video and dim, so nothing depends on color alone.

## How TALK.md becomes beats

`n` always moves to the next thing to do. Here is how the markdown maps onto that:

| In TALK.md | In speaker |
|---|---|
| `# Title` | the talk title |
| `## 0:00–5:00 · Intro` | a **part**. A time range (`–`, `-` or `—`) puts it in the talk flow |
| `## Cleaning up` (no time) | a part outside the talk flow. `g` reaches it; `n` skips it |
| `### Step 6 · Title · 17:00–20:00` | a **step** with a budget. `Steps 4 and 5 · …` answers to `g 4` and `g 5` |
| a `##` with no `###` under it | one step, with the part's budget |
| plain paragraphs and lists | **SAY**. Consecutive paragraphs and lists form one beat |
| a paragraph that starts with `**bold lead-in.**` | starts a new SAY beat |
| `<!-- beat -->` on its own line | forces a beat break (invisible on GitHub) |
| ` ```sql ` / ` ```sh ` | **RUN**, one beat each. Target is `psql` or `shell` |
| ` ```prompt ` | **PROMPT**, one beat; the target is the AI agent. `c` unwraps hard-wrapped lines so the paste doesn't send early |
| a plain ` ``` ` right after a command block | that command's **EXPECT** output, on the same beat |
| a plain ` ``` ` anywhere else | a standalone **EXPECT · on screen** beat |
| `> **★ DSQL difference: …**` | **KEY POINT**, one beat; the bold sentence is its title |
| any other `>` quote (`> No agent? …`) | a reader note, hidden; attached to the beat before it |
| `<details><summary>Presenter notes</summary>` | **presenter notes** for the step (sidebar, or top of the step) |
| `<details>` with any other summary (`Following along: …`) | a reader note, hidden |
| a table whose header has `Pane A` and `Pane B` | a **race**: one beat per row. `c` copies the row's `` `code` ``; text after `→` is its expected result |
| any other table | one **TABLE** beat |
| `**bold**`, `` `code` ``, `[text](url)`, `*italic*` | rendered. Links keep their text only |

### Where a command runs

The target label comes from the language: `sql` means psql, `sh` means shell, and
`prompt` means the AI agent. speaker refines it when the hint is unambiguous:

- **Explicit, which is best:** put words after the language, e.g. ` ```sql pane B `.
  GitHub ignores them, and speaker shows `psql · pane B`.
- **From the command:** a first line like `-- both panes, one line at a time` gives
  `psql · both panes`.
- **From the lead-in:** the last sentence of the paragraph just before the block.
  "From a second psql session (pane B):" gives `psql · pane B`, and "In a second
  terminal:" gives `shell · second terminal`. If that sentence names more than one
  pane, no hint is used.

### Markup tips

- Put each `\gset` query on its own line. `C` copies one statement at a time, and
  the flash after `c` warns when a block contains `\gset`.
- Write race-table cells out in full (`` `INSERT INTO comments (...) VALUES (...);` ``
  rather than `` `INSERT INTO comments ...;` ``), so `c` copies something runnable.
- On-screen output belongs in a plain ` ``` ` block, not a `>` quote. Quotes are
  hidden as reader notes.
- `speaker/speaker --check` lists everything speaker had to guess about, with line
  numbers.

speaker never crashes on markdown it doesn't understand. It renders it as plain text.

## Environment

| Variable | Effect |
|---|---|
| `NO_COLOR=1` | no colors |
| `SPEAKER_COPY_CMD` | shell command that receives copied text on stdin, instead of `pbcopy` (then wl-copy, xclip, xsel, then the terminal's OSC 52) |
| `SPEAKER_SCREEN_DUMP=path` | write the screen text to `path` after every draw (used by the tests) |

## Tests

```sh
python3 -m unittest discover -s speaker/tests      # about 15 s; the pty tests drive the real UI
```

- `test_parser.py`: the fixture (`tests/fixtures/mini.md`) beat by beat, plus generic
  checks against the live TALK.md: every `### Step` found, ★ quotes are key points,
  sql/sh/prompt fences are commands, and plain fences are expected output.
- `test_timing.py`: time parsing, the step window, and the schedule delta.
- `test_render.py`: layout at many widths and screen sizes, plus the curses blit
  against a fake screen.
- `test_app.py`: keys, navigation, goto, copy, clock and reload.
- `test_pty.py`: launches `speaker` in a pseudo-terminal. It navigates, copies,
  resizes and quits (also with Ctrl-C and without colors), and checks that the
  terminal is restored.

## Files

```
speaker/
  speaker            entry point (Python script)
  prompter/
    parser.py        TALK.md → parts, steps, beats
    inline.py        **bold**, `code`, [links](...) → styled spans
    model.py         Talk / Part / Step / Beat / Elem
    timing.py        times, step windows, schedule delta
    layout.py        a step → wrapped, styled lines (pure)
    screen.py        the whole screen → a canvas of cells (pure)
    app.py           state, keys, theme, the curses loop
    clipboard.py     pbcopy and friends
    cli.py           arguments, --dump, --json, --check
  tests/
```
