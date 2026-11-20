# stage/rex

The same stage as [`stage/`](..) (Emacs and demo-it), run in the Rex
terminal instead. It's an alternative, not a
replacement: both read the talk from TALK.md, through `stage/generate.py`.
Rex needs nothing else installed, so it's the easier one for someone else to
run.

Each F12 runs the next **press**: commands typed into terminal panes the way
a person types, blocks of exports pasted, Ctrl-R re-runs, panes laid out. As
in Emacs, there are no titles or banners: just terminals.

What's different from Emacs:
- **Highlights are yours.** Select with the mouse while you talk: the stage
  has no highlight presses, so the talk is 74 presses here, not 92.
- **Ids are pasted**, not copied in an acted-out copy mode.
- **A jump doesn't run the steps before it** (Emacs offers to). Jump to a
  step that doesn't need them, such as the finale.
- **Cues** (switch to the browser, the console's address) show as the stage
  window's label until the next press, not in an echo area.

## Files

| File | |
|---|---|
| `dat401.lua` | The runtime: `rex do stage/rex/dat401.lua cmd=...` |
| `talk.lua` | The presses, **generated** from TALK.md (`python3 stage/generate.py`). Don't edit it. |
| `init.lua` | F12 and Page Down, for your Rex config |

## Setup (once)

Add this line to your Rex config, `~/.config/rex/init.lua`, with this repo's
path, then run `rex config reload`:

```lua
dofile(os.getenv("HOME") .. "/code/riv26-codetalk/stage/rex/init.lua")
```

That line binds nothing until a stage is running. Rex has no per-session key
bindings, so `cmd=start` reloads the config to add F12 and Page Down (a
clicker's next), and `cmd=reset` reloads it again to take them away. While the
stage runs, they act only in the "DAT401 stage" session: anywhere else they do
what they always did.

Also, as for the Emacs stage: `stage/setup-cdc-role.sh` (step 9's role), the
finished app deployed (`demo/deploy.sh`), and `demo/preflight.sh` before the
talk.

## Running it

From the repo root, in a terminal with your AWS settings (they're kept for the
panes):

```sh
AWS_PROFILE=bowes rex do stage/rex/dat401.lua cmd=start
```

That makes a session called **DAT401 stage**, with psql A's pane. Select it in
Rex's sidebar, make the font big (⌘+), and hide the sidebar. It also starts a
Claude Code pane for each of the talk's prompts, off screen, with its prompt
already in it, so no agent starts (or updates) on stage.

| | |
|---|---|
| F12 or Page Down | the next press. Pressed while one is still typing, it finishes the typing at once. |
| `rex do stage/rex/dat401.lua cmd=next` | the same, from any terminal |
| `rex do stage/rex/dat401.lua cmd=goto title="Losing a Region"` | the next press starts that talk step (any part of its title) |
| `rex do stage/rex/dat401.lua cmd=status` | where you are, and what the next press does |
| `rex do stage/rex/dat401.lua cmd=reset` | close the session and forget everything, for a clean `cmd=start` |

What `cmd=start` keeps, from its environment: `AWS_PROFILE` (or
`DAT401_AWS_PROFILE`), `DAT401_AWS_REGION` (us-west-2), `SHELL`,
`DAT401_TYPING_SPEED` (1.0 is a person; 0.5 twice as fast), `DAT401_AGENT`
(`claude`), `DAT401_WAIT_TIMEOUT` (180 s). State lives in
`~/.cache/dat401-rex/` (`DAT401_REX_STATE` to move it), with a log of every
press in `log.txt`.

## How it works

A press is its own `rex do` process. The F12 binding starts one, so a long
press (typing, waiting for psql) never holds up the Rex server. Everything goes
through Rex's server API:

- **Panes** are terminal blocks in one window, created as they're needed with
  their own environment, laid out by moving them in and out of the window.
  The layouts follow the Emacs ones: psql A alone, A and B side by side, A with
  a shell below, the load generator with a strip along the bottom...
- **Typing** writes a character at a time, with the Emacs stage's timing
  model (uneven, like a person).
- **Waiting** reads the pane's text for a prompt that stays put, as the Emacs
  stage does, before typing the next statement.
- **Values** like `<cluster-endpoint>` and `<job_id>` come from the screen;
  `<blog-url>`, `<fis-template-id>` and the Regions' endpoints from
  `demo/outputs.json`; anything else from `DAT401_<NAME>`.

## Tests

```sh
AWS_PROFILE=bowes python3 stage/test/rex_e2e.py            # intro to step 7, on a new cluster (deleted after)
AWS_PROFILE=bowes python3 stage/test/rex_e2e.py --finale   # the Two Regions segment, against the deployed app
```

They run in their own session and state directory, with `cat` for the agent
and fast typing. `--finale` makes `open` a no-op and runs a local 400 req/s
load test, but the FIS experiment is real.
