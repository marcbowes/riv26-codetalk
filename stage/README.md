# stage

The talk's terminal demos, run from Emacs with
[demo-it](https://github.com/howardabrams/demo-it). Each press of F12 (or a
clicker's PgDn) runs the next command from [TALK.md](../TALK.md), typed into
the right pane the way a person types. You talk; it types.

It's meant to look live, not like a slideshow: there are no titles, no
banners, nothing that says "step 3". Just terminals and files.

- **Typing** has a person's rhythm, about 7 characters a second, unevenly:
  - each line has its own tempo;
  - some words come in a quick burst, others more slowly;
  - there are gaps between words and longer ones after punctuation;
  - Shift and symbols cost a moment;
  - now and then there's a pause to think, or a hiccup mid-word.

  SQL is typed in lower case and without indentation, as people do. TALK.md
  keeps its formatting for readers.
- **IDs are never typed.** A command that needs one (`<cluster-id>` in step
  1, every `<job_id>`) acts out copying it from the output above:
  1. Enter vterm's copy mode.
  2. Move up to the ID's line: a couple of taps, then key-repeat speed.
  3. Hop along that line a word at a time.
  4. Set the mark and select the ID.
  5. Copy, go back to the prompt, and paste.

  The clipboard isn't touched: the value comes from a variable. An ID that
  isn't on screen is pasted, and a short readable value is typed.
- **Pasting a block:** a ```` ```sh pane B paste ```` block is pasted whole
  (a bracketed paste, so the shell shows it as pasted), then run.

- **Panes** are terminal buffers (Ghostel, or vterm): `psql A`, `psql B`, `shell`, `events` and
  `load`. Each one starts in the repo root with `AWS_PROFILE` and
  `AWS_REGION` set. Once the cluster's endpoint is known, they also get
  `PGHOST` and the other psql settings.
- **Layouts** follow the commands. Most steps show psql A on its own. The
  foreign key race shows A and B side by side. The loader shows A with a
  shell along the bottom.
- **Highlights** on terminal output, one F12 each:
  - `<!-- stage: highlight identifier -->` highlights a JSON field's value.
  - `highlight dpu` highlights an EXPLAIN's DPU total.
  - `highlight index` highlights the plan's index scan.

  Highlights survive the terminal redrawing. They clear at the next command,
  or at `<!-- stage: clear -->`. A command that pastes a value keeps them, and
  highlights the pasted value too.
- **Files** open full screen, named like the file (`db.ts`, `prompt.md`). Code
  is narrowed to the excerpt TALK.md shows, with its lines highlighted and the
  lines between them dimmed. Step 4 does this for `demo/api` and its prompt.
- **★ DSQL difference** callouts aren't shown. You say them; speaker has them.

`speaker/` still works as the teleprompter on a second screen. This replaces
copying and pasting from it.

## Setup (once)

demo-it is already in `~/.doom.d/packages.el`, and `doom sync` has been run.
**Restart Emacs** so it loads.

The panes are **Ghostel** terminals: [ghostel](https://github.com/dakra/ghostel)
runs libghostty-vt, the VT engine behind the Ghostty terminal. It's in
`~/.doom.d/packages.el` as `(package! ghostel)`. Its native module is a
prebuilt download from ghostel's releases. If it's missing (a `doom sync`
that rebuilt ghostel deletes it), fetch it again:

```sh
G=~/.emacs.d/.local/straight/build-*/ghostel
emacs -Q --batch -L $G --eval '(progn (require (quote ghostel)) (require (quote ghostel-module-install)) (ghostel--download-module (ghostel--module-directory)))'
```

vterm still works as a fallback: `(setq dat401-terminal 'vterm)`. If you use
it, build its module **with reflow off**. libvterm's reflow rewraps lines
when a terminal resizes, and with a full-screen program running (the load
generator) it can abort Emacs ("screen_resize failed to update cursor
position"):

```sh
cd ~/.emacs.d/.local/straight/build-*/vterm && rm -rf build && mkdir build && cd build \
  && cmake -DUSE_SYSTEM_LIBVTERM=no -DCMAKE_C_FLAGS=-DVTermScreenEnableReflowNotExists .. \
  && make
```

The panes aren't text-scaled. A scaled terminal can be told the wrong size,
which mangles full-screen programs. `dat401-start` makes the whole frame's
font big instead (Doom's `doom-big-font-mode`), and `dat401-stop` puts it
back.

## Running it

```
M-x load-file RET ~/code/riv26-codetalk/stage/dat401-stage.el RET
M-x dat401-start
```

| Key | Does |
|---|---|
| F12 or PgDn | Run the next step |
| any key while it's typing | Finish the current command instantly (the key is swallowed) |
| `C-u N F12` | Run step N |
| `C-F12` or `M-x dat401-goto` | Jump to a talk step by name. The next F12 runs its first command |
| `M-x dat401-stop` | Leave demo mode. The panes stay open |
| `M-x dat401-reset` | Close every pane and forget the cluster, for a clean `dat401-start` (rehearsals) |

You never copy anything by hand:
- Step 1's first F12 types `aws dsql create-cluster`. `<cluster-endpoint>`
  (and `<cluster-id>`) come from the endpoint it prints, copied from that
  line.
- `<job_id>` comes from the `CREATE INDEX ASYNC` output on screen.
- `<kinesis-stream-arn>` comes from the `describe-stream-summary` output in
  step 10.
- Anything else is asked for once in the minibuffer, then remembered.

Step 1 goes like this:
1. Type `create-cluster`.
2. Highlight the identifier, then highlight the endpoint.
3. Type the `PGHOST` export, copying the endpoint in, which is highlighted too.
4. Clear the highlights.
5. Type the token export and `psql` in pane A, full screen.
6. Open pane B, paste its exports, then type `psql` there.

Typing waits for each statement or shell command to finish before it types
the next line. It never waits after the last line, so a step that starts
something long-running (the events viewer, the load generator) hands control
back to you.

Settings (`M-x customize-group RET dat401`):

- `dat401-typing-speed`: 1.0 is a person at a keyboard, 0.5 is twice as fast, 0 is instant.
- `dat401-text-scale`: how large files (code) are. Terminals use `dat401-big-font`.
- `dat401-aws-profile` and `dat401-aws-region`: bowes and us-west-2.
- `dat401-wait-timeout`: how long to wait for psql, 180 s.

## Changing the demo

Don't edit `dat401-talk.el`. Edit TALK.md, then regenerate:

```sh
python3 stage/generate.py           # rewrites stage/dat401-talk.el
python3 stage/generate.py --check   # exits 1 if it's out of date
```

It reads TALK.md through `speaker --json`, so both tools agree on what's a
command and where it runs. A fence's info string picks the pane:

- ```` ```sql ```` goes to psql A.
- ```` ```sql pane B ```` goes to psql B, and ```` ```sh pane A ```` to pane A.
- ```` ```sh both panes ```` and ```` ```sql both panes ```` go to A, then B.
- ```` ```sh pane B paste ```` is pasted into B in one go, not typed.
- ```` ```sh ```` goes to the shell. Commands that start with `./events/` go
  to the events pane, and commands that run `loadgen` go to the load pane.
- ```` ```ts demo/api/src/db.ts ```` (any language, then a path) is a code
  excerpt. It's shown, not typed. Its lines must appear in the file verbatim
  and in order. A `...` line skips ahead. The generator fails if an excerpt
  no longer matches its file.
- ```` ```text ```` is expected output. It's never typed.

Stage directions are HTML comments, hidden on GitHub. Each takes one F12:
- `<!-- stage: highlight <json field | dpu | index> -->`
- `<!-- stage: clear -->`
- `<!-- stage: open pane B -->`

`<!-- stage: close pane shell -->` has no F12 of its own: it happens with the
next one.

Comment lines, trailing comments and indentation are dropped before typing,
and SQL is lowercased outside quotes. A literal `wait_for_job('...')` id is
replaced with `<job_id>`.

## Tests

```sh
B=~/.emacs.d/.local/straight/build-32.0.50
emacs -Q --batch -L $B/demo-it -L $B/vterm -L stage -l stage/test/dat401-test.el -f ert-run-tests-batch-and-exit
python3 -m unittest discover -s stage/test

# End to end: real Doom Emacs in a pty. Runs the intro through step 6:
# step 1 creates a real cluster (deleted afterwards), the highlights, pane B's
# paste, the schema, step 4's code, the loader, EXPLAIN, both async indexes.
# Then it checks what the panes show.
AWS_PROFILE=bowes python3 stage/test/e2e.py
```
