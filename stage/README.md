# stage

The talk's terminal demos, run from Emacs with
[demo-it](https://github.com/howardabrams/demo-it). Each press of F12 (or a
clicker's PgDn) runs the next command from [TALK.md](../TALK.md), typed into
the right pane at a human pace: quick bursts inside words, pauses between
them and after punctuation, the odd hesitation, then a beat before Enter. You
talk; it types.

- **Panes** are vterm buffers: `psql A`, `psql B`, `shell`, `events` and
  `load`. Each one starts in the repo root with `AWS_PROFILE` and
  `AWS_REGION` set. Once cluster A is known, they also get `PGHOST` and the
  other psql settings.
- **Layouts** follow the commands. Most steps show psql A on its own. The
  foreign key race shows A and B side by side. The loader shows A with a
  shell along the bottom.
- **★ DSQL difference** callouts become a yellow banner across the top.
- **Code** fills the screen: the real file, narrowed to the excerpt TALK.md
  shows, with the excerpt's lines highlighted and the lines between them
  dimmed. Step 4 does this for `demo/api`.
- **The AI prompt** in step 4 opens in a side window.
- **The header line** names the current talk step.

`speaker/` still works as the teleprompter on a second screen. This replaces
copying and pasting from it.

## Setup (once)

demo-it is already in `~/.doom.d/packages.el`, and `doom sync` has been run.
**Restart Emacs** so it loads. vterm needs its module built for your Emacs. If
`M-x vterm` asks to compile it, say yes, or build it by hand:

```sh
cd ~/.emacs.d/.local/straight/build-*/vterm && mkdir -p build && cd build \
  && cmake -DUSE_SYSTEM_LIBVTERM=no .. && make
```

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

The first time a command needs `<cluster-A-id>`, `<kinesis-stream>` or
another placeholder, the minibuffer asks for it once and remembers it.
`<job_id>` is never asked for: it's read from the `CREATE INDEX ASYNC` output
on screen. To use a new cluster, set it before you start:
`(puthash "cluster-A-id" "abc..." dat401--vars)`.

Typing waits for each statement or shell command to finish before it types
the next line. It never waits after the last line, so a step that starts
something long-running (the events viewer, the load generator) hands control
back to you.

Settings (`M-x customize-group RET dat401`):

- `dat401-typing-speed`: 1.0 is a confident typist, 0.5 is twice as fast, 0 is instant.
- `dat401-text-scale`: how large the text is.
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
- ```` ```sql pane B ```` goes to psql B.
- ```` ```sh both panes ```` and ```` ```sql both panes ```` go to A, then B.
- ```` ```sh ```` goes to the shell. Commands that start with `./events/` go
  to the events pane, and commands that run `loadgen` go to the load pane.
- ```` ```ts demo/api/src/db.ts ```` (any language, then a path) is a code
  excerpt. It's shown, not typed. Its lines must appear in the file verbatim
  and in order. A `...` line skips ahead. The generator fails if an excerpt
  no longer matches its file.
- ```` ```text ```` is expected output. It's never typed.

Comment lines and trailing comments are dropped before typing. A literal
`wait_for_job('...')` id is replaced with `<job_id>`.

## Tests

```sh
B=~/.emacs.d/.local/straight/build-32.0.50
emacs -Q --batch -L $B/demo-it -L $B/vterm -L stage -l stage/test/dat401-test.el -f ert-run-tests-batch-and-exit
python3 -m unittest discover -s stage/test

# End to end: real Doom Emacs in a pty, against an empty scratch cluster.
# Runs the intro through step 6 (schema, step 4's code, loader, EXPLAIN,
# both async indexes) and checks what the panes show.
DAT401_E2E_CLUSTER=<id> python3 stage/test/e2e.py
```
