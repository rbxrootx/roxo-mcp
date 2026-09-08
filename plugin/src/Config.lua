local strict = require(script.Parent.strict)

local isDevBuild = script.Parent.Parent:FindFirstChild("ROJO_DEV_BUILD") ~= nil

local Version = script.Parent.Parent.Version
local trimmedVersionValue = Version.Value:gsub("^%s+", ""):gsub("%s+$", "")
local major, minor, patch, metadata = trimmedVersionValue:match("^(%d+)%.(%d+)%.(%d+)(.*)$")

local realVersion = { major, minor, patch, metadata }
for i = 1, 3 do
	local num = tonumber(realVersion[i])
	if num then
		realVersion[i] = num
	else
		error(("invalid version `%s` (field %d)"):format(realVersion[i], i))
	end
end

return strict("Config", {
	isDevBuild = isDevBuild,
	codename = "Epiphany",
	version = realVersion,
	expectedServerVersionString = ("%d.%d or newer"):format(realVersion[1], realVersion[2]),
	protocolVersion = 5,
	defaultHost = "localhost",
	defaultPort = "34872",

	-- The span of ports auto-connect looks at when hunting for a server. Ten
	-- is enough for the handful of projects anyone serves at once, and small
	-- enough that repeated polling stays well inside Studio's HTTP limits.
	defaultPortRange = { min = 34872, max = 34881 },
	maxPortRangeSize = 32,

	-- Discovery starts eager, because the common case is an agent that just
	-- started a server and is waiting on the connection, then backs off so an
	-- editor left open for hours is not polling at full rate.
	autoConnectInitialInterval = 2,
	autoConnectMaxInterval = 15,
})
