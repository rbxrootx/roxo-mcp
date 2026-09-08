return function(TestEZ)
	local Roxo = script.Parent.Parent
	local Packages = Roxo.Packages

	TestEZ.TestBootstrap:run({ Roxo.Plugin, Packages.Http, Packages.Log, Packages.RbxDom })
end
