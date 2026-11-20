-- init.lua: F12 (fn-F12 on a Mac keyboard), or a clicker's Page Down, runs the
-- next press of the DAT401 stage in Rex. Load it from your own Rex config (~/.config/rex/init.lua),
-- with this repo's path:
--
--   dofile(os.getenv("HOME") .. "/code/riv26-codetalk/stage/rex/init.lua")
--
-- It binds nothing unless a stage is running: Rex has no per-session key
-- bindings, so dat401.lua's cmd=start and cmd=reset reload the config, and
-- the keys come and go with the stage. While it runs, they act only in the
-- "DAT401 stage" session; anywhere else they do what they always do. Each
-- press is logged to keys.log, beside the stage's state.
--
-- A key press only leaves a request beside the stage's state: the stage's
-- driver (dat401.lua cmd=serve, started by cmd=start) runs the press, so a
-- long one (typing, waiting for psql) never holds up the server. A press made
-- while one is still typing makes it finish its typing at once.

local state_dir = os.getenv("DAT401_REX_STATE") or ((os.getenv("HOME") or "/tmp") .. "/.cache/dat401-rex")
local state_file = state_dir .. "/state.json"

local function log(s)
  pcall(function()
    local f = io.open(state_dir .. "/keys.log", "a")
    if f then f:write(os.date("%H:%M:%S ") .. s .. "\n"); f:close() end
  end)
end

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

local function write(path, s)
  local f = io.open(path, "w")
  if f then f:write(s); f:close() end
end

-- A press still running? It touches this lock as it goes.
local function busy()
  local f = io.open(state_dir .. "/running", "r")
  if not f then return false end
  local stamp = tonumber(f:read("*a")) or 0
  f:close()
  return os.time() - stamp < 20
end

rex.action{
  name = "dat401_next",
  title = "DAT401: next stage press",
  args = { key = "string" },
  run = function(ctx, args)
    local stage = stage_session()
    local session = type(ctx) == "table" and ctx.session_id or nil
    local ok, seen = pcall(rex.json.encode, ctx)
    log(string.format("%s: ctx %s, stage %s", tostring(args.key), ok and seen or "?", tostring(stage)))
    -- A key with no session can only have come from the app in front of you.
    if stage and (session == nil or session == stage) then
      if busy() then
        write(state_dir .. "/hurry", "1")
        log("  busy: hurry")
      else
        write(state_dir .. "/requests", "next\n")
        log("  next press requested")
      end
    else
      rex.client.queue("pane.send_key", { key = args.key })
    end
  end,
}

rex.bind("f12", "dat401_next", { key = "f12" })
rex.bind("pagedown", "dat401_next", { key = "pagedown" })
