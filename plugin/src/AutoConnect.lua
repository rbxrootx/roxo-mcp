--[[
	Finds a serve session this place is allowed to attach to without a human
	pressing Connect.

	Rojo requires that click, which makes it unusable from an AI agent: the
	agent can start a server but cannot reach into Studio to accept it. Removing
	the click naively would be worse than the problem, because a plugin that
	connects to whatever server it finds will happily overwrite one game with
	another game's source.

	So the rule here is that the *server* has to prove it belongs to this place,
	and the proof has to be unambiguous. A candidate is only auto-connectable if
	it identifies this place explicitly, or a human previously paired the two.
	When two servers both qualify, neither is chosen; ambiguity is reported to
	the user instead of guessed at.
]]

local Packages = script.Parent.Parent.Packages
local Http = require(Packages.Http)
local Log = require(Packages.Log)
local Promise = require(Packages.Promise)

local Config = require(script.Parent.Config)
local Types = require(script.Parent.Types)

local validateApiInfo = Types.ifEnabled(Types.ApiInfoResponse)

local AutoConnect = {}

--[[
	How a candidate proved it belongs to this place, strongest first.

	The ordering exists to explain a choice to the user, not to break ties. Two
	candidates that both qualify are always ambiguous, however strong their
	proofs are, because the whole point of the check is that exactly one project
	should claim a given place.
]]
AutoConnect.MatchReason = {
	-- The project lists this place in `servePlaceIds`.
	ServePlaceIds = "servePlaceIds",
	-- The project's `gameId` matches this place's universe.
	GameId = "gameId",
	-- A human connected this place to this project before.
	Paired = "paired",
	-- The project opted in with `"autoConnect": "always"`. This is the only
	-- reason available to an unpublished place, which has no ID to match.
	Declared = "declared",
}

local REASON_STRENGTH = {
	[AutoConnect.MatchReason.ServePlaceIds] = 4,
	[AutoConnect.MatchReason.GameId] = 3,
	[AutoConnect.MatchReason.Paired] = 2,
	[AutoConnect.MatchReason.Declared] = 1,
}

--[[
	Decides whether a place may attach to a server without confirmation.

	`context` carries the place's identity and its saved pairings, rather than
	reading them from the DataModel, so this stays a pure function that can be
	tested against cases that are painful to reproduce in Studio.

	Returns a match table, or nil plus a reason the candidate was rejected.
]]
function AutoConnect.evaluate(serverInfo, context)
	if serverInfo == nil then
		return nil, "no server info"
	end

	if serverInfo.protocolVersion ~= Config.protocolVersion then
		return nil, "incompatible protocol version"
	end

	local placeId = context.placeId or 0
	local gameId = context.gameId or 0

	-- A blocked place is never eligible, whatever else matches. This check
	-- comes first so an explicit exclusion cannot be overridden by a pairing.
	if serverInfo.unexpectedPlaceIds and table.find(serverInfo.unexpectedPlaceIds, placeId) then
		return nil, "this place is listed in blockedPlaceIds"
	end

	-- A server that names the places it serves is making an exhaustive claim,
	-- so a place outside the list is disqualified even if it was paired before.
	if serverInfo.expectedPlaceIds and not table.find(serverInfo.expectedPlaceIds, placeId) then
		return nil, "this place is not listed in servePlaceIds"
	end

	-- Upstream Rojo servers do not report a policy. Treating that as the
	-- default rather than as a refusal means Roxo's plugin can still
	-- auto-connect to a stock Rojo server whose project names this place.
	local policy = serverInfo.autoConnect or "matching"

	if policy == "off" then
		return nil, "the project has auto-connect turned off"
	end

	if placeId ~= 0 and serverInfo.expectedPlaceIds and table.find(serverInfo.expectedPlaceIds, placeId) then
		return {
			reason = AutoConnect.MatchReason.ServePlaceIds,
			strength = REASON_STRENGTH[AutoConnect.MatchReason.ServePlaceIds],
		}
	end

	if gameId ~= 0 and serverInfo.gameId ~= nil and serverInfo.gameId == gameId then
		return {
			reason = AutoConnect.MatchReason.GameId,
			strength = REASON_STRENGTH[AutoConnect.MatchReason.GameId],
		}
	end

	-- Pairings are keyed by project identity rather than by name, because two
	-- unrelated projects are quite likely to both be called "Game".
	local pairing = context.pairings and context.pairings[tostring(placeId)]
	if
		placeId ~= 0
		and pairing ~= nil
		and serverInfo.projectId ~= nil
		and pairing.projectId == serverInfo.projectId
	then
		return {
			reason = AutoConnect.MatchReason.Paired,
			strength = REASON_STRENGTH[AutoConnect.MatchReason.Paired],
		}
	end

	-- `always` exists solely because an unpublished place reports a PlaceId of
	-- 0 and so has nothing to match on. A published place has an identity, and
	-- the checks above can use it, so `always` is not allowed to override them
	-- there: otherwise a scratch project left on `always` will attach itself to
	-- whatever real game the developer happens to open next.
	if policy == "always" and placeId == 0 then
		return {
			reason = AutoConnect.MatchReason.Declared,
			strength = REASON_STRENGTH[AutoConnect.MatchReason.Declared],
		}
	end

	if placeId == 0 then
		return nil,
			"this place is unpublished, so it has no ID to match against. "
				.. 'Set "autoConnect": "always" in the project file, or connect once by hand to pair them.'
	end

	if policy == "always" then
		return nil,
			"the project is set to 'always', which only applies to unpublished places. "
				.. "Add this place to servePlaceIds, or connect once by hand to pair them."
	end

	return nil, "nothing identifies this place as belonging to the project"
end

--[[
	Chooses at most one candidate to connect to.

	Returns the chosen candidate, or nil and a list of the candidates that tied.
	Ambiguity deliberately produces no connection: picking the "best" of several
	claims is how a project ends up synced into the wrong place, and a wrong
	sync is far more expensive to undo than a prompt is to answer.
]]
function AutoConnect.choose(candidates)
	if #candidates == 0 then
		return nil, {}
	end

	if #candidates == 1 then
		return candidates[1], {}
	end

	return nil, candidates
end

--[[
	Parses a comma-separated list of ranges like "34370-34379, 34872-34881".

	A single range cannot cover both Rojo's historical ports and the ports
	people actually serve on, and a server outside the scan is invisible no
	matter how well its project identifies the place.
]]
function AutoConnect.parsePortRanges(text)
	-- The old single-range default was saved by earlier versions; treat it as
	-- unset so those installs pick up the wider default.
	if type(text) == "string" and string.gsub(text, "%s", "") == Config.legacyPortRangeText then
		text = Config.defaultPortRangeText
	end

	local ranges = {}
	if type(text) == "string" then
		for part in string.gmatch(text, "[^,]+") do
			local low, high = AutoConnect.parsePortRange(part)
			table.insert(ranges, { low = low, high = high })
		end
	end
	if #ranges == 0 then
		for _, range in Config.defaultPortRanges do
			table.insert(ranges, { low = range.min, high = range.max })
		end
	end
	return ranges
end

--[[
	Parses a port range like "34872-34881" into its bounds.

	Falls back to the default range rather than erroring, because a typo in a
	setting should not be able to disable discovery outright.
]]
function AutoConnect.parsePortRange(text)
	if type(text) ~= "string" then
		return Config.defaultPortRange.min, Config.defaultPortRange.max
	end

	local low, high = string.match(text, "^%s*(%d+)%s*%-%s*(%d+)%s*$")
	low, high = tonumber(low), tonumber(high)

	if low == nil or high == nil or low > high then
		local single = tonumber(string.match(text, "^%s*(%d+)%s*$"))
		if single then
			return single, single
		end

		Log.warn("Invalid auto-connect port range '{}', using the default.", text)
		return Config.defaultPortRange.min, Config.defaultPortRange.max
	end

	-- A wide range means a burst of requests on every poll, which Studio's HTTP
	-- limits will eventually push back on. Cap it rather than let a stray digit
	-- turn discovery into a port scan of the whole machine.
	if high - low > Config.maxPortRangeSize then
		high = low + Config.maxPortRangeSize
		Log.warn("Auto-connect port range is too wide; limiting it to {}-{}.", low, high)
	end

	return low, high
end

local function baseUrlFor(host, port)
	if string.find(host, "^https?://") then
		return string.format("%s:%s", host, port)
	end

	return string.format("http://%s:%s", host, port)
end

--[[
	Asks a single address what it is, if anything.

	Always resolves. A closed port is the common case during discovery, not an
	error worth propagating, so it resolves to nil like any other non-candidate.
]]
function AutoConnect.probe(host, port)
	local baseUrl = baseUrlFor(host, port)

	-- Identify the request as a probe. Discovery contacts every candidate port
	-- and walks away from all but one, so without this the server records a
	-- string of anonymous clients and `roxo status` shows phantom connections
	-- next to the real one.
	return Http.get(baseUrl .. "/api/rojo?client=roxo-plugin-probe")
		:andThen(Http.Response.msgpack)
		:andThen(function(body)
			if not validateApiInfo(body) then
				return nil
			end

			return {
				host = host,
				port = tostring(port),
				baseUrl = baseUrl,
				serverInfo = body,
			}
		end)
		:catch(function()
			return nil
		end)
end

--[[
	Probes every port in the range and returns those this place may attach to.

	Probes run concurrently because a sequential scan across ten closed ports
	takes long enough that an agent's `wait-for-studio` would time out before
	the plugin had finished looking.
]]
function AutoConnect.discover(options)
	local host = options.host or Config.defaultHost

	local ports = {}
	local seen = {}

	-- The user's configured port goes first so that it is probed even when it
	-- sits outside the scanned range.
	--
	-- Converted before the check rather than after: the setting is a string,
	-- and an empty one is truthy in Lua while `tonumber("")` is nil, so testing
	-- the raw value would try to insert nil into the list.
	local preferredPort = tonumber(options.preferredPort)

	if preferredPort then
		table.insert(ports, preferredPort)
		seen[preferredPort] = true
	end

	for _, range in AutoConnect.parsePortRanges(options.portRange) do
		for port = range.low, range.high do
			if not seen[port] then
				seen[port] = true
				table.insert(ports, port)
			end
		end
	end

	-- Peers learned on an earlier poll, so a session outside the ranges stays
	-- found even if the server that pointed at it has since stopped.
	for _, port in AutoConnect.extraPorts(options.knownPeerPorts, seen) do
		table.insert(ports, port)
	end

	local function probeAll(list)
		local probes = {}
		for _, port in list do
			table.insert(probes, AutoConnect.probe(host, port))
		end
		return Promise.all(probes)
	end

	return probeAll(ports)
		:andThen(function(results)
			-- Every Roxo server lists the other sessions on its machine, so one
			-- extra round reaches all of them. Servers found this way are judged
			-- by exactly the same rules as any other.
			local peerPorts = AutoConnect.collectPeerPorts(results, seen)
			if #peerPorts == 0 then
				return results
			end

			return probeAll(peerPorts):andThen(function(peerResults)
				-- Both lists have holes where ports were closed, so they are
				-- merged into a fresh list rather than appended in place.
				local merged = {}
				for _, list in { results, peerResults } do
					for _, result in list do
						table.insert(merged, result)
					end
				end
				return merged
			end)
		end)
		:andThen(function(results)
			return AutoConnect.classify(results, options.context)
		end)
end

--[[
	Returns the ports in `list` not already marked in `seen`, marking them.
]]
function AutoConnect.extraPorts(list, seen)
	local ports = {}
	if type(list) ~= "table" then
		return ports
	end

	for _, port in list do
		port = tonumber(port)
		if port and port % 1 == 0 and port >= 1 and port <= 65535 and not seen[port] then
			seen[port] = true
			table.insert(ports, port)
		end
	end
	return ports
end

--[[
	Gathers the `peerPorts` advertised by every server that answered, skipping
	ports already probed. Capped, because the list comes from a server and a
	poll should never turn into an unbounded burst of requests.
]]
function AutoConnect.collectPeerPorts(results, seen)
	local ports = {}
	for _, result in results do
		if result ~= nil and type(result.serverInfo.peerPorts) == "table" then
			for _, port in AutoConnect.extraPorts(result.serverInfo.peerPorts, seen) do
				if #ports >= Config.maxPeerPorts then
					return ports
				end
				table.insert(ports, port)
			end
		end
	end
	return ports
end

--[[
	Splits probe results into servers this place may attach to and servers it
	may not, with the reason for each rejection. Also returns every peer port
	the servers advertised, so the caller can keep probing them on later polls.
]]
function AutoConnect.classify(results, context)
	local candidates = {}
	local rejected = {}
	local peerPorts = {}
	local seenPeers = {}

	-- Iterated with pairs semantics on purpose: a closed port resolves to nil,
	-- so the result list has holes.
	for _, result in results do
		if result == nil then
			continue
		end

		for _, port in AutoConnect.extraPorts(result.serverInfo.peerPorts, seenPeers) do
			table.insert(peerPorts, port)
		end

		local match, why = AutoConnect.evaluate(result.serverInfo, context)

		if match then
			result.match = match
			table.insert(candidates, result)
		else
			table.insert(rejected, {
				projectName = result.serverInfo.projectName,
				port = result.port,
				reason = why,
			})
		end
	end

	return {
		candidates = candidates,
		rejected = rejected,
		peerPorts = peerPorts,
	}
end

return AutoConnect
