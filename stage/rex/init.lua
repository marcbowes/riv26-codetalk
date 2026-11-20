-- init.lua: F12, or a clicker's Page Down, runs the next press of the DAT401
-- stage in Rex. Load it from your own Rex config (~/.config/rex/init.lua),
-- with this repo's path:
--
--   dofile(os.getenv("HOME") .. "/code/riv26-codetalk/stage/rex/init.lua")
--
-- It binds nothing unless a stage is running: Rex has no per-session key
-- bindings, so dat401.lua's cmd=start and cmd=reset reload the config, and
-- the keys come and go with the stage. While it runs, they act only in the
-- "DAT401 stage" session; anywhere else they do what they always do.
--
-- A press runs as its own `rex do` process, so a long one (typing, waiting for
-- psql) never holds up the server. A press made while one is still typing
-- makes it finish its typing at once.

local here = debug.getinfo(1, "S").source:gsub("^@", ""):match("^(.*)/[^/]*$")
local runner = here .. "/dat401.lua"
local state_file = (os.getenv("DAT401_REX_STATE")
  or ((os.getenv("HOME") or "/tmp") .. "/.cache/dat401-rex")) .. "/state.json"

-- The stage's session, as dat401.lua cmd=start recorded it.
local function stage_session()
  local f = io.open(state_file, "r")
  if not f then return nil end
  local ok, s = pcall(rex.json.decode, f:read("*a"))
  f:close()
  return ok and type(s) == "table" and s.session_id or nil
end

-- No stage, no keys.
if not stage_session() then return end

-- `rex` is on the PATH in Rex's own terminals; the server's is a fallback.
local launch = [[
for r in rex "$HOME/Library/Application Support/com.superlogical.rex/RexServer/Versions/Current/Rex Server.app/Contents/MacOS/rex"; do
  if command -v "$r" >/dev/null 2>&1; then exec "$r" do "$0" cmd=next; fi
done
exit 127]]

rex.action{
  name = "dat401_next",
  title = "DAT401: next stage press",
  args = { key = "string" },
  run = function(ctx, args)
    if ctx and ctx.session_id and ctx.session_id == stage_session() then
      rex.exec.start({ "/bin/sh", "-c", launch, runner })
    else
      rex.client.queue("pane.send_key", { key = args.key })
    end
  end,
}

rex.bind("f12", "dat401_next", { key = "f12" })
rex.bind("pagedown", "dat401_next", { key = "pagedown" })
