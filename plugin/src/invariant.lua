local Packages = script.Parent.Parent.Packages
local Fmt = require(Packages.Fmt)

local Config = require(script.Parent.Config)

local invariant

if Config.isDevBuild then
	function invariant(message, ...)
		message = Fmt.fmt(message, ...)

		error("Invariant violation: " .. message, 2)
	end
else
	function invariant(message, ...)
		message = Fmt.fmt(message, ...)

		local fullMessage = string.format(
			"Roxo detected an invariant violation within itself:\n"
				.. "%s\n\n"
				.. "This is a bug in Roxo. Please file an issue:\n"
				.. "https://github.com/rbxrootx/roxo-mcp/issues",
			message
		)

		error(fullMessage, 2)
	end
end

return invariant
