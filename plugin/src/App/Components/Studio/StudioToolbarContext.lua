local Roxo = script:FindFirstAncestor("Roxo")
local Packages = Roxo.Packages

local Roact = require(Packages.Roact)

local StudioToolbarContext = Roact.createContext(nil)

return StudioToolbarContext
