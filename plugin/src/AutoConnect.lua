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

	if policy == "always" then
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

	return Http.get(baseUrl .. "/api/rojo")
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
	local low, high = AutoConnect.parsePortRange(options.portRange)

	local ports = {}

	-- The user's configured port goes first so that it is probed even when it
	-- sits outside the scanned range.
	if options.preferredPort then
		table.insert(ports, tonumber(options.preferredPort))
	end

	for port = low, high do
		if port ~= tonumber(options.preferredPort) then
			table.insert(ports, port)
		end
	end

	local probes = {}
	for _, port in ports do
		table.insert(probes, AutoConnect.probe(host, port))
	end

	return Promise.all(probes):andThen(function(results)
		local candidates = {}
		local rejected = {}

		for _, result in results do
			if result == nil then
				continue
			end

			local match, why = AutoConnect.evaluate(result.serverInfo, options.context)

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
		}
	end)
end

return AutoConnect
