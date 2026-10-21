-- Minimal TCP round-robin reverse proxy in front of N SwarmPanel backend
-- worker processes (see main.lua), each listening on 127.0.0.1 at its own
-- port. Direct copy of Nyxframe's proxy.lua (lua/proxy.lua there) -- same
-- deployment model, same reasoning: a byte-level TCP relay rather than an
-- HTTP-aware proxy, so WebSocket framing and chunked/streaming responses
-- pass through untouched instead of needing a second implementation here.
-- See that file's own header for the full design writeup; nothing below is
-- SwarmPanel-specific except the GALLERY_PROXY_* env var names, which were
-- kept as-is (not renamed) specifically so this stays a verbatim, easy-to-
-- diff copy rather than a near-copy with incidental naming drift.
--
-- Round-robin is per TCP CONNECTION, not per request -- this is actually
-- the right behavior for SwarmPanel specifically (unlike Nyxframe, where
-- it's just incidentally equivalent to per-request): SwarmPanel holds
-- long-lived WebSocket connections for live dashboard updates, and
-- per-connection round-robin is exactly "pin this WebSocket to one backend
-- for its whole lifetime," which is what a stateful WS connection needs.
-- Regular page/API requests get Connection: close from httpd.lua same as
-- Nyxframe, so those are per-request-equivalent too. Auth is a stateless
-- signed token (verify_api_token against a shared session_secret, not an
-- in-memory session table), so it needs no sticky routing either -- any
-- worker can verify a token any other worker issued.
--
-- Run with: luajit proxy.lua   (see swarmpanel-proxy.service)

package.path = "./lib/?.lua;./lib/?/init.lua;./src/?.lua;" .. package.path

local socket = require("socket")
local copas = require("copas")

local LISTEN_HOST = os.getenv("GALLERY_PROXY_HOST") or "0.0.0.0"
local LISTEN_PORT = tonumber(os.getenv("GALLERY_PROXY_PORT")) or 8007
local RELAY_CHUNK_BYTES = 8192

local function parse_backends(raw)
  local backends = {}
  for entry in tostring(raw or ""):gmatch("[^,]+") do
    local host, port = entry:match("^%s*([^:]+):(%d+)%s*$")
    if host and port then backends[#backends + 1] = { host = host, port = tonumber(port) } end
  end
  return backends
end

local backends = parse_backends(os.getenv("GALLERY_PROXY_BACKENDS"))
if #backends == 0 then
  backends = { { host = "127.0.0.1", port = tonumber(os.getenv("GALLERY_HTTP_PORT")) or 8003 } }
end

do
  local labels = {}
  for _, b in ipairs(backends) do labels[#labels + 1] = b.host .. ":" .. b.port end
  print(string.format("[swarmpanel-proxy] listening on %s:%d -> backends: %s",
    LISTEN_HOST, LISTEN_PORT, table.concat(labels, ", ")))
end

local next_backend = 0

local function connect_backend()
  for _ = 1, #backends do
    next_backend = (next_backend % #backends) + 1
    local b = backends[next_backend]
    local upstream = socket.tcp()
    local ok = copas.connect(upstream, b.host, b.port)
    if ok then return upstream, b end
    pcall(function() upstream:close() end)
  end
  return nil
end

local function pump(from_sock, to_sock)
  while true do
    local data, err, partial = copas.receivepartial(from_sock, RELAY_CHUNK_BYTES)
    local payload = data or partial
    if payload and #payload > 0 then
      if not copas.send(to_sock, payload) then break end
    end
    if not data and err ~= "timeout" then break end
  end
  pcall(function() from_sock:close() end)
  pcall(function() to_sock:close() end)
end

local function handle_connection(client)
  local upstream = connect_backend()
  if not upstream then
    pcall(function() client:close() end)
    return
  end
  copas.addthread(pump, upstream, client)
  pump(client, upstream)
end

local server = assert(socket.bind(LISTEN_HOST, LISTEN_PORT))
copas.addserver(server, handle_connection)
print(string.format("[swarmpanel-proxy] ready"))
copas.loop()
