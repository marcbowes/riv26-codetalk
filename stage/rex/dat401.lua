-- dat401.lua: the DAT401 stage, for Rex. The same talk as stage/dat401-stage.el
-- (Emacs demo-it), read from stage/rex/talk.lua, which stage/generate.py writes
-- from TALK.md. Each F12 runs the next press: commands typed into terminal
-- panes the way a person types, pastes, Ctrl-R re-runs, layouts.
--
--   rex do stage/rex/dat401.lua cmd=start            a "DAT401 stage" session, press 1
--   rex do stage/rex/dat401.lua cmd=next             the next press (what F12 does)
--   rex do stage/rex/dat401.lua cmd=goto title="Losing a Region"
--   rex do stage/rex/dat401.lua cmd=status
--   rex do stage/rex/dat401.lua cmd=reset            close the session, forget everything
--
-- Unlike Emacs: highlights are yours (the mouse), ids are pasted rather than
-- copied in copy mode, and a jump doesn't run the steps before it. A press made
-- while another is still typing makes that one finish its typing at once.
--
-- Lua 5.1. Every Rex call goes through the server API (rex.call).

local SOURCE = debug.getinfo(1, "S").source:gsub("^@", "")
if SOURCE:sub(1, 1) ~= "/" then SOURCE = (os.getenv("PWD") or ".") .. "/" .. SOURCE end
local HERE = SOURCE:match("^(.*)/[^/]*$")
local REPO = HERE:gsub("/stage/rex$", "")
local STATE_DIR = os.getenv("DAT401_REX_STATE") or ((os.getenv("HOME") or "/tmp") .. "/.cache/dat401-rex")
local STATE_FILE = STATE_DIR .. "/state.json"
local LOCK = STATE_DIR .. "/running"
local HURRY = STATE_DIR .. "/hurry"
local LOG = STATE_DIR .. "/log.txt"

local TITLES = {
  a = "psql A", b = "psql B", shell = "shell", events = "events", load = "load",
  claude = "claude", view = "view",
}
local REGIONS = { "us-west-2", "us-east-1" }   -- the finished app's two Regions, left to right
local WINDOW_LABEL = "DAT401"
local FLAVOR = "com.superlogical.terminal.shell"

math.randomseed(os.time())

-- --------------------------------------------------------------------------
-- Files and state

local function read_file(path)
  local f = io.open(path, "r")
  if not f then return nil end
  local s = f:read("*a")
  f:close()
  return s
end

local function write_file(path, s)
  local f = assert(io.open(path, "w"))
  f:write(s)
  f:close()
end

local function exists(path)
  local f = io.open(path, "r")
  if f then f:close() return true end
  return false
end

rex.exec({ "mkdir", "-p", STATE_DIR })

local function log(fmt, ...)
  local f = io.open(LOG, "a")
  if f then
    f:write(os.date("%H:%M:%S ") .. string.format(fmt, ...) .. "\n")
    f:close()
  end
end

local state

local function load_state()
  local s = read_file(STATE_FILE)
  state = s and rex.json.decode(s) or nil
  if state then
    state.panes = state.panes or {}
    state.vars = state.vars or {}
    state.fresh = state.fresh or {}
    state.layout = state.layout or {}
    state.stale = state.stale or {}
  end
  return state
end

local function save_state()
  write_file(STATE_FILE, rex.json.encode(state))
end

-- One press at a time. The running press touches the lock as it goes; a lock
-- that hasn't been touched for a while was left by a press that died.
local function heartbeat()
  write_file(LOCK, tostring(os.time()))
end

local function locked()
  local s = read_file(LOCK)
  return s ~= nil and (os.time() - (tonumber(s) or 0)) < 20
end

local function hurried() return exists(HURRY) end

-- --------------------------------------------------------------------------
-- The server

local S   -- the stage session

local function call(method, params)
  local r, err = rex.call(method, params)
  if r == nil and err ~= nil then error(method .. ": " .. tostring(err), 2) end
  return r
end

local function block_method(block, method, args)
  return call("block.method", { session_id = S, block_id = block, method = method, args = args or {} })
end

local function attach(session)
  S = session
  call("session.attach", { session_id = S })
end

local function session_alive(session)
  local list = rex.call("session.list", {}) or {}
  for _, s in ipairs(list.sessions or list) do
    if s.session_id == session then return true end
  end
  return false
end

-- --------------------------------------------------------------------------
-- Settings, from the environment of `cmd=start` (a press started by F12 runs
-- with the Rex server's environment, so they're kept in the state file).

local function settings_from_env()
  return {
    profile = os.getenv("DAT401_AWS_PROFILE") or os.getenv("AWS_PROFILE") or "",
    region = os.getenv("DAT401_AWS_REGION") or "us-west-2",
    shell = os.getenv("SHELL") or "/bin/zsh",
    speed = tonumber(os.getenv("DAT401_TYPING_SPEED") or "") or 1.0,
    agent = os.getenv("DAT401_AGENT") or "claude",
    agent_ready = os.getenv("DAT401_AGENT_READY") or "shift%+tab to cycle",
    timeout = tonumber(os.getenv("DAT401_WAIT_TIMEOUT") or "") or 180,
    -- Tests: `open` is a no-op you can still see, and the load test is a small
    -- local one instead of 20,000 req/s from EC2.
    test = os.getenv("DAT401_REX_TEST") == "1",
  }
end

local function cfg() return state.config end

-- --------------------------------------------------------------------------
-- Waiting

local function now() return os.time() end

-- A pause that a second press (hurry) cuts short. For typing, not for waits.
local function pause(secs)
  if secs <= 0 or hurried() then return end
  heartbeat()
  rex.sleep(secs)
end

-- A wait for the terminal: never cut short.
local function sleep(secs)
  heartbeat()
  rex.sleep(secs)
end

local function wait_for(pred, timeout)
  local deadline = now() + timeout
  while true do
    local v = pred()
    if v then return v end
    if now() >= deadline then return nil end
    sleep(0.1)
  end
end

-- --------------------------------------------------------------------------
-- Panes

local function is_region(name)
  return type(name) == "string" and name:match("^%a%a%-%a+%-%d$") ~= nil
end

local function title(name) return TITLES[name] or name end

local function pane_command(name)
  local c = cfg()
  local cmd = { "/usr/bin/env", "-u", "CLAUDECODE", "-u", "CLAUDE_CODE_ENTRYPOINT" }
  local function add(s) cmd[#cmd + 1] = s end
  if c.profile ~= "" then add("AWS_PROFILE=" .. c.profile) end
  -- A Region's pane signs for that Region and pastes its own PGHOST.
  add("AWS_REGION=" .. (is_region(name) and name or c.region))
  add("PSQL_PAGER=cat"); add("PAGER=cat"); add("PROMPT_EOL_MARK=")
  local endpoint = state.vars["cluster-endpoint"]
  if endpoint and not is_region(name) then
    add("PGHOST=" .. endpoint); add("PGUSER=admin"); add("PGDATABASE=postgres"); add("PGSSLMODE=require")
  end
  add(c.shell); add("-il")
  return cmd
end

local live_blocks   -- block id -> true, read once per press

local function block_alive(id)
  if not live_blocks then
    live_blocks = {}
    local r = call("session.list_blocks", { session_id = S }) or {}
    for _, b in ipairs(r.blocks or r) do live_blocks[b.block_id] = true end
  end
  return live_blocks[id] == true
end

local function block_options(name, command)
  return { shell = "none", command = command or pane_command(name), cwd = REPO }
end

-- PANE's terminal, made (not yet on screen) the first time it's needed.
local function pane(name, command)
  local id = state.panes[name]
  if id and block_alive(id) then return id end
  local r = call("session.new_block", {
    session_id = S, flavor = FLAVOR, label = title(name), options = block_options(name, command),
  })
  id = r.block_id
  -- The app sizes a pane it shows, not the client that made it.
  rex.call("session.disown_block", { session_id = S, block_ids = { id } })
  state.panes[name] = id
  state.fresh[name] = true
  live_blocks = live_blocks or {}
  live_blocks[id] = true
  save_state()
  return id
end

local function text(name)
  local id = state.panes[name]
  if not id then return "" end
  local r = rex.call("block.method", {
    session_id = S, block_id = id, method = "format", args = { format = "text", unwrap = true, trim = true },
  })
  if not r then return "" end
  local out, more = r.content or "", r.id
  while more do
    local part = block_method(id, "blob_read", { id = more, offset = #out })
    out = out .. (part.content or "")
    more = part.id
  end
  return out
end

local function tail(s) return (s:gsub("%s+$", "")) end

local function at_prompt(name)
  local t = tail(text(name))
  return t:match("=[%*!]?[>#]$") ~= nil or t:match("[%$%%#>]$") ~= nil or t:sub(-3) == "\226\157\175"   -- ❯
end

local function shell_prompt(name)
  local t = tail(text(name))
  return t:match("[%$%%]$") ~= nil or t:sub(-3) == "\226\157\175"
end

local function write(name, data) block_method(pane(name), "write", { data = data }) end
local function enter(name) write(name, "\r") end

local function paste(name, s)
  call("block.event", { session_id = S, block_id = pane(name), event = { type = "paste", payload = { text = s } } })
end

-- Wait until PANE has finished what it was given: right after Enter the screen
-- can still show the old prompt, so give it a moment (SETTLE), then wait for a
-- prompt that stays put.
local function wait_settled(name, timeout, settle)
  local deadline = now() + (timeout or cfg().timeout)
  sleep(settle or 0.3)
  while now() < deadline do
    wait_for(function() return at_prompt(name) end, math.max(1, deadline - now()))
    local before = text(name)
    sleep(0.25)
    if at_prompt(name) and text(name) == before then return true end
  end
  return false
end

local function wait_prompt(name) return wait_settled(name) end
local function ready(name) return wait_settled(name, cfg().timeout, 0.05) end

-- --------------------------------------------------------------------------
-- Layouts: the panes in the stage window, arranged as stage/dat401-stage.el
-- arranges its windows.

local function contains(list, x)
  for _, v in ipairs(list) do if v == x then return true end end
  return false
end

local function normalize(panes)
  local want = panes[1]
  local region = false
  for _, p in ipairs(panes) do if is_region(p) then region = true end end
  if #panes == 1 and type(want) == "string" and want:match("^claude") then return { want } end
  if #panes == 1 and want == "view" then return { "view" } end
  if contains(panes, "load") then
    return contains(panes, "shell") and { "load", "shell" } or { "load" }
  end
  if region then
    local out = { REGIONS[1], REGIONS[2] }
    if contains(panes, "shell") then out[3] = "shell" end
    return out
  end
  if contains(panes, "b") then return { "a", "b" } end
  if contains(panes, "claude") then return { "a", "claude" } end
  if want == "shell" and contains(panes, "shell") then return { "a", "shell" } end
  if contains(panes, "events") then return { "a", "events" } end
  if contains(panes, "shell") then return { "a", "shell" } end
  return { "a" }
end

local function same(x, y)
  if #x ~= #y then return false end
  for i = 1, #x do if x[i] ~= y[i] then return false end end
  return true
end

local function window_blocks()
  local v = call("session.view", { session_id = S })
  local out = {}
  for _, w in ipairs(v.windows or {}) do
    if w.window_id == state.window_id then
      for _, layer in ipairs(w.layers or {}) do
        for _, b in ipairs(layer.blocks or {}) do out[b.block_id] = true end
      end
    end
  end
  return out
end

local function place(id, anchor, direction, ratio)
  call("session.move_block", {
    session_id = S, block_id = id, anchor_block_id = anchor, direction = direction, side = "after", ratio = ratio,
  })
end

-- New panes: once the shell has drawn its first prompt at the size it's shown
-- at, start from a clean screen.
local function tidy(panes)
  for _, p in ipairs(panes) do
    if state.fresh[p] then
      wait_for(function() return shell_prompt(p) end, 8)
      sleep(0.3)
      write(p, "\12")   -- Ctrl-L
      state.fresh[p] = nil
    end
  end
  save_state()
end

local function layout(panes)
  panes = normalize(panes)
  local ids = {}
  for i, p in ipairs(panes) do ids[i] = pane(p) end
  if not same(panes, state.layout) then
    local placed = window_blocks()
    if not placed[ids[1]] then
      local any = next(placed)
      if any then place(ids[1], any, "horizontal", 0.5) end
    end
    for id in pairs(window_blocks()) do
      if id ~= ids[1] then call("session.detach_block", { session_id = S, block_id = id }) end
    end
    local shape = table.concat(panes, " ")
    if is_region(panes[1]) then
      if panes[3] == "shell" then place(ids[3], ids[1], "vertical", 0.8) end
      place(ids[2], ids[1], "horizontal", 0.5)
    elseif shape == "a shell" then
      place(ids[2], ids[1], "vertical", 2 / 3)
    elseif shape == "load shell" then
      place(ids[2], ids[1], "vertical", 0.8)
    elseif #panes == 2 then
      place(ids[2], ids[1], "horizontal", 0.5)
    end
    state.layout = panes
    save_state()
  end
  tidy(panes)
end

local function show(name)
  if contains(state.layout, name) then return end
  local panes = { name }
  for _, p in ipairs(state.layout) do
    if p ~= "view" and p ~= name then panes[#panes + 1] = p end
  end
  layout(panes)
end

local function focus(name)
  rex.call("session.focus_block", { session_id = S, block_id = pane(name) })
end

-- --------------------------------------------------------------------------
-- Typing like a person (the same model as dat401--delays in Emacs)

local function gauss()
  local s = 0
  for _ = 1, 12 do s = s + math.random() end
  return s - 6
end

local function jitter(ms, spread) return ms * math.exp(spread * gauss()) end

local function pause_ms(ms, spread) return jitter(ms, spread) / 1000 * cfg().speed end

local SHIFTED = {}
for ch in ("_()\"'<>{}*$:|&!?%+~^@#"):gmatch(".") do SHIFTED[ch] = true end
local AFTER_WORD = { [" "] = true, ["("] = true, ["."] = true, [","] = true, ["="] = true, [";"] = true }
local PUNCT = { [","] = true, [";"] = true, ["("] = true, [")"] = true, ["."] = true, ["="] = true, [":"] = true }

local function chars(s)
  local out = {}
  for ch in s:gmatch("[%z\1-\127\194-\244][\128-\191]*") do out[#out + 1] = ch end
  return out
end

-- LINE typed by a person: {char, seconds} pairs. The line has its own tempo;
-- each word is a quick burst or typed steadily; there's a gap between words
-- and a longer one after punctuation; Shift costs a moment; now and then a
-- pause to think before a word, or a hiccup mid-word.
local function delays(line)
  local tempo = jitter(1.0, 0.18)
  local out, prev, burst = {}, nil, false
  for _, ch in ipairs(chars(line)) do
    if prev == nil or AFTER_WORD[prev] then burst = math.random(100) <= 20 end
    local ms = burst and jitter(45, 0.25) or jitter(78, 0.4)
    if prev == " " then
      ms = ms + jitter(125, 0.45)
      if math.random(100) <= 8 then ms = ms + jitter(650, 0.45) end
    end
    if PUNCT[prev] then ms = ms + jitter(135, 0.5) end
    if SHIFTED[ch] or (ch:match("^%u$") and not (prev and prev:match("^%u$"))) then
      ms = ms + jitter(60, 0.4)
    end
    if math.random(1000) <= 15 then ms = ms + jitter(300, 0.4) end
    out[#out + 1] = { ch, ms * tempo / 1000 * cfg().speed }
    prev = ch
  end
  return out
end

local function type_text(name, s)
  local ds = delays(s)
  for i, cd in ipairs(ds) do
    if hurried() then
      local rest = {}
      for j = i, #ds do rest[#rest + 1] = ds[j][1] end
      write(name, table.concat(rest))
      return
    end
    write(name, cd[1])
    pause(cd[2])
  end
end

-- --------------------------------------------------------------------------
-- Placeholders: <cluster-endpoint>, <job_id>, ... filled from the screen, the
-- finished app's demo/outputs.json, or DAT401_<NAME> in the environment.

local function placeholders(s)
  local out = {}
  for name in s:gmatch("<([%a][%w_%-]*)>") do out[#out + 1] = name end
  return out
end

local function segments(line)
  local out, pos = {}, 1
  while true do
    local a, b, name = line:find("<([%a][%w_%-]*)>", pos)
    if not a then break end
    if a > pos then out[#out + 1] = { line:sub(pos, a - 1), false } end
    out[#out + 1] = { name, true }
    pos = b + 1
  end
  if pos <= #line then out[#out + 1] = { line:sub(pos), false } end
  return out
end

local function last_match(s, pattern)
  local found
  for m in s:gmatch(pattern) do found = m end
  return found
end

local function endpoints(s)
  local out = {}
  for e in s:gmatch('"endpoint": "([a-z0-9]+%.dsql%.[a-z0-9%-]+%.on%.aws)"') do out[#out + 1] = e end
  return out
end

local function new_endpoint()
  for _, p in ipairs({ "a", "b", "shell" }) do
    local found
    for _, e in ipairs(endpoints(text(p))) do
      if not contains(state.stale, e) then found = e end
    end
    if found then return found end
  end
end

local function last_job_id(s)
  local found
  for id in s:gmatch("job_id[ \t]*\n[%-%+ \t]+\n[ \t]*([a-z0-9]+)") do
    if #id == 26 then found = id end
  end
  return found
end

local SCRAPED = {
  ["kinesis-stream-arn"] = '"StreamARN": "([^"\n]+)"',
  ["cdc-role-arn"] = '"(arn:aws:iam::%d+:role/[^"\n]+)"',
  ["cdc-stream-id"] = '"streamIdentifier": "([a-z0-9]+)"',
  ["fis-experiment-id"] = "(EXP%w+)",
  ["post-id"] = "(%x%x%x%x%x%x%x%x%-%x%x%x%x%-%x%x%x%x%-%x%x%x%x%-%x%x%x%x%x%x%x%x%x%x%x%x)",
}

local function outputs()
  local s = read_file(REPO .. "/demo/outputs.json")
  return s and rex.json.decode(s) or nil
end

local function output_value(name)
  local o = outputs()
  if not o then return nil end
  if name == "blog-url" then return o.webUrl end
  if name == "fis-template-id" then return o.fis and o.fis.templateId end
  local region = name:match("^(%a%a%-%a+%-%d)%-endpoint$")
  if region then
    for _, r in ipairs(o.regions or {}) do
      if r.region == region then return r.clusterEndpoint end
    end
  end
end

local function value(name, where)
  if SCRAPED[name] then
    local v = wait_for(function() return last_match(text(where), SCRAPED[name]) end, 15)
    if v then return v end
  elseif name == "job_id" then
    local v = wait_for(function() return last_job_id(text(where)) end, 30)
    if v then return v end
  elseif name == "cluster-endpoint" then
    if not state.vars[name] then
      state.vars[name] = wait_for(new_endpoint, 30)
      save_state()
    end
    if state.vars[name] then return state.vars[name] end
  elseif name == "cluster-id" then
    return (value("cluster-endpoint", where):match("^([^.]+)"))
  else
    local v = state.vars[name] or output_value(name)
    if v then return v end
  end
  local env = os.getenv("DAT401_" .. name:upper():gsub("[^%w]", "_"))
  if env then return env end
  error("no value for <" .. name .. ">")
end

local function resolve(s, where)
  for _, name in ipairs(placeholders(s)) do
    local v = value(name, where)
    s = s:gsub("<" .. name:gsub("%-", "%%-") .. ">", (v:gsub("%%", "%%%%")))
  end
  return s
end

-- Nobody types an id, a URL, an ARN or an endpoint by hand: it's pasted.
local function id_like(v)
  return v:match("^[a-z0-9]+$") and #v >= 20
      or v:match("^EX[PT]%w+$") and #v >= 10
      or v:match("^%x+%-%x+%-%x+%-%x+%-%x+$")
      or v:match("^https?://")
      or v:match("^arn:")
      or v:match("%.on%.aws$")
end

local function type_line(name, line)
  pause(pause_ms(420, 0.35))
  for _, seg in ipairs(segments(line)) do
    if seg[2] then
      local v = value(seg[1], name)
      if id_like(v) then
        pause(pause_ms(450, 0.35))
        paste(name, v)
        pause(pause_ms(250, 0.35))
      else
        type_text(name, v)
      end
    else
      type_text(name, seg[1])
    end
  end
  pause(pause_ms(300, 0.4))
  enter(name)
end

local function statement_end(line)
  local l = line:gsub("^%s+", ""):gsub("%s+$", "")
  return l:sub(-1) == ";" or l:sub(1, 1) == "\\" or l:match("\\gset$") ~= nil
end

local function new_cluster_coming()
  state.stale = {}
  for _, p in ipairs({ "a", "b", "shell" }) do
    if state.panes[p] then
      for _, e in ipairs(endpoints(text(p))) do state.stale[#state.stale + 1] = e end
    end
  end
  state.vars["cluster-endpoint"] = nil
  save_state()
end

-- Type TEXT into PANE line by line. Between lines, wait for a shell command or
-- a psql statement to finish; never after the last line (it may start
-- something that keeps running).
local function type_lines(name, s)
  ready(name)
  local lines = {}
  for l in (s .. "\n"):gmatch("([^\n]*)\n") do if l ~= "" then lines[#lines + 1] = l end end
  for i, line in ipairs(lines) do
    local shell = shell_prompt(name)
    if line:find("aws dsql create-cluster", 1, true) then new_cluster_coming() end
    type_line(name, line)
    if i < #lines and (shell or statement_end(line)) then wait_prompt(name) end
  end
end

-- --------------------------------------------------------------------------
-- Ops: what a press does

local ops = {}
local talk_prompts   -- the talk's prompts, from talk.lua

function ops.section(op) state.section = op.title end

function ops.layout(op) layout(op.panes) end

function ops.show(op) show(op.pane) end

function ops.open(op) show(op.pane); focus(op.pane) end

function ops.close(op)
  if contains(state.layout, op.pane) then
    local rest = {}
    for _, p in ipairs(state.layout) do if p ~= op.pane then rest[#rest + 1] = p end end
    layout(#rest > 0 and rest or { "a" })
  end
end

local function for_tests(s)
  if not cfg().test then return s end
  s = s:gsub("^open ", ": open "):gsub("\nopen ", "\n: open ")
  return (s:gsub("%-%-run%-on%-ec2 %-%-rps 20000", "--rps 400 --ramp 5"))
end

function ops.run(op)
  local s = for_tests(op.text)
  for _, p in ipairs(op.panes) do show(p) end
  for _, p in ipairs(op.panes) do
    focus(p)
    type_lines(p, s)
  end
end

-- A block of exports, pasted in one go (a bracketed paste at a shell prompt),
-- then Enter.
function ops.paste(op)
  show(op.pane); focus(op.pane); ready(op.pane)
  local s = resolve(op.text, op.pane):gsub("%s+$", "")
  pause(pause_ms(600, 0.3))
  paste(op.pane, s)
  pause(pause_ms(500, 0.3))
  enter(op.pane)
  wait_prompt(op.pane)
end

-- The same statements in TO, as if copied from FROM's screen: each line pasted,
-- then Enter.
function ops.copy_into(op)
  show(op.to); focus(op.to); ready(op.to)
  local lines = {}
  for l in (op.text .. "\n"):gmatch("([^\n]*)\n") do if l ~= "" then lines[#lines + 1] = l end end
  for _, line in ipairs(lines) do
    pause(pause_ms(900, 0.3))
    paste(op.to, resolve(line, op.from))
    pause(pause_ms(450, 0.35))
    enter(op.to)
    if statement_end(line) then wait_prompt(op.to) end
  end
end

-- Run TEXT again from the shell's or psql's history: Ctrl-R, a few characters,
-- a look, Enter. If the search fails, give up on it (Ctrl-G) and type TEXT.
function ops.recall(op)
  local p = op.pane
  show(p); focus(p); ready(p)
  pause(pause_ms(600, 0.3))
  write(p, "\18")   -- Ctrl-R
  pause(pause_ms(400, 0.3))
  type_text(p, op.search)
  sleep(0.8)
  local t = text(p)
  if t:sub(-400):find("failed reverse-i-search", 1, true) then
    write(p, "\7")  -- Ctrl-G
    sleep(0.3)
    type_lines(p, op.text)
  else
    enter(p)
    wait_prompt(p)
  end
end

-- Claude Code panes, one per prompt (claude-1, claude-2...), each started at
-- cmd=start with its prompt already in it (pasted, never submitted: no Enter,
-- no newline), so no agent starts (or updates) on stage. A press shows one
-- full screen; quit_agent closes it and puts back what was on screen.

local function one_line(s) return (s:gsub("^%s+", ""):gsub("%s+$", ""):gsub("[ \t]*\n[ \t\n]*", " ")) end

local function agent_pane_for(prompt)
  for i, p in ipairs(talk_prompts or {}) do
    if one_line(p) == one_line(prompt) then return "claude-" .. i end
  end
  return "claude"
end

local function start_agent(name, prompt)
  pane(name)
  wait_for(function() return shell_prompt(name) end, 15)
  state.fresh[name] = nil
  write(name, cfg().agent .. "\r")
  wait_for(function() return text(name):find(cfg().agent_ready) ~= nil end, 45)
  sleep(0.6)
  paste(name, one_line(prompt))
  state.agents = state.agents or {}
  state.agents[name] = one_line(prompt)
  save_state()
end

local function agent_ready(name, prompt)
  return state.agents and state.agents[name] == one_line(prompt)
    and state.panes[name] and block_alive(state.panes[name])
end

function ops.agent(op)
  local name = agent_pane_for(op.text)
  local before = {}
  for _, p in ipairs(state.layout) do
    if not p:match("^claude") and p ~= "view" then before[#before + 1] = p end
  end
  state.before_agent = before
  state.current_agent = name
  if not agent_ready(name, op.text) then start_agent(name, op.text) end
  layout({ name })
  focus(name)
end

function ops.quit_agent()
  local back = state.before_agent
  if not back or #back == 0 then back = { "a" } end
  layout(back)
  local name = state.current_agent or "claude"
  local id = state.panes[name]
  if id then
    rex.call("block.close", { session_id = S, block_id = id })
    state.panes[name] = nil
    state.fresh[name] = nil
    if state.agents then state.agents[name] = nil end
  end
  state.before_agent = nil
  state.current_agent = nil
  save_state()
end

-- A reminder for a move only the presenter can make (switch to the browser):
-- shown as the window's label until the next press.
function ops.cue(op)
  rex.call("session.set_window_label", { session_id = S, window_id = state.window_id, label = "\226\134\146 " .. op.text })
end

-- A file, read-only, from its first excerpted line.
function ops.code(op)
  local first = (op.ranges[1] or { 1 })[1]
  if state.panes.view then
    rex.call("block.close", { session_id = S, block_id = state.panes.view })
    state.panes.view = nil
  end
  pane("view", { "less", "-R", "-N", "+" .. first, REPO .. "/" .. op.path })
  state.fresh.view = nil
  layout({ "view" })
end

-- --------------------------------------------------------------------------
-- Commands

local talk_cache
local function talk()
  talk_cache = talk_cache or dofile(HERE .. "/talk.lua")
  talk_prompts = talk_cache.prompts
  return talk_cache
end

local function run_press(ops_list)
  live_blocks = nil
  rex.call("session.set_window_label", { session_id = S, window_id = state.window_id, label = WINDOW_LABEL })
  for _, op in ipairs(ops_list) do
    local f = ops[op.op]
    if not f then error("unknown op " .. tostring(op.op)) end
    f(op)
  end
end

local function describe(press)
  local parts = {}
  for _, op in ipairs(press or {}) do
    if op.text then
      parts[#parts + 1] = op.op .. ": " .. (op.text:match("^[^\n]*") or ""):sub(1, 60)
    elseif op.op ~= "section" then
      parts[#parts + 1] = op.op .. (op.pane and (" " .. op.pane) or (op.panes and (" " .. table.concat(op.panes, "+")) or ""))
    end
  end
  return table.concat(parts, "; ")
end

local function need_state()
  if not load_state() or not session_alive(state.session_id) then
    error("no stage running: rex do " .. SOURCE .. " cmd=start")
  end
  attach(state.session_id)
end

local function press(n)
  local t = talk()
  local p = t.presses[n]
  if not p then return "the talk is over (" .. #t.presses .. " presses)" end
  heartbeat()
  os.remove(HURRY)
  state.step = n
  save_state()
  log("press %d: %s", n, describe(p))
  local ok, err = pcall(run_press, p)
  os.remove(LOCK)
  os.remove(HURRY)
  if not ok then
    log("press %d failed: %s", n, tostring(err))
    error(err, 0)
  end
  local nxt = t.presses[n + 1]
  return string.format("press %d of %d done (%s). Next: %s", n, #t.presses, state.section or "",
    nxt and describe(nxt) or "the end")
end

local commands = {}

function commands.start()
  if load_state() and session_alive(state.session_id) then
    error("a stage is already running (" .. state.session_id .. "): cmd=reset first")
  end
  state = { config = settings_from_env(), panes = {}, vars = {}, fresh = {}, layout = {}, stale = {}, step = 0 }
  local s = call("session.create", {
    label = "DAT401 stage",
    initial_windows = { { window_label = WINDOW_LABEL, layout = { block = {
      flavor = FLAVOR, label = title("a"), options = block_options("a"),
    } } } },
  })
  state.session_id = s.session_id
  state.window_id = s.initial_windows[1].window_id
  state.panes.a = s.initial_windows[1].block_ids[1]
  state.fresh.a = true
  state.layout = { "a" }
  attach(state.session_id)
  rex.call("session.disown_block", { session_id = S, block_ids = { state.panes.a } })
  save_state()
  os.remove(LOCK)
  -- The agents for the talk's prompts, off screen: Rex keeps drawing a
  -- terminal nobody's looking at.
  live_blocks = nil
  for i, prompt in ipairs(talk().prompts or {}) do start_agent("claude-" .. i, prompt) end
  return press(1) .. "\nShow the \"DAT401 stage\" session in Rex. F12 (with stage/rex/init.lua) or cmd=next runs each press."
end

function commands.next()
  if locked() then
    write_file(HURRY, "1")
    return "busy: the current press finishes its typing now"
  end
  need_state()
  return press((state.step or 0) + 1)
end

commands["goto"] = function()
  need_state()
  local want = rex.args.title or ""
  for _, e in ipairs(talk().index) do
    if e[1] == want or e[1]:find(want, 1, true) then
      state.step = e[2] - 1
      state.section = e[1]
      save_state()
      return "next press (" .. e[2] .. ") runs: " .. e[1]
    end
  end
  error("no talk step called " .. want)
end

function commands.status()
  if not load_state() then return "no stage" end
  local t = talk()
  local n = (state.step or 0) + 1
  return string.format("session %s%s, press %d of %d next (%s): %s", state.session_id,
    session_alive(state.session_id) and "" or " (gone)", n, #t.presses, state.section or "", describe(t.presses[n]))
end

function commands.reset()
  if load_state() and state.session_id and session_alive(state.session_id) then
    attach(state.session_id)
    call("session.destroy", { session_id = S })
  end
  os.remove(STATE_FILE); os.remove(LOCK); os.remove(HURRY)
  return "reset: the stage session is closed"
end

local cmd = rex.args.cmd or "next"
if not commands[cmd] then error("cmd= start, next, goto, status or reset") end
return commands[cmd]()
