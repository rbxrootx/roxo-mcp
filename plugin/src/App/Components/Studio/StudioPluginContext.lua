local Roxo = script:FindFirstAncestor("Roxo")
local Packages = Roxo.Packages

local Roact = require(Packages.Roact)

local StudioPluginContext = Roact.createContext(nil)

return StudioPluginContext
