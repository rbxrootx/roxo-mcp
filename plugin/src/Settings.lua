--[[
	Persistent plugin settings.
]]

local plugin = plugin or script:FindFirstAncestorWhichIsA("Plugin")
local Roxo = script:FindFirstAncestor("Roxo")
local Packages = Roxo.Packages

local Log = require(Packages.Log)
local Roact = require(Packages.Roact)

local defaultSettings = {
	openScriptsExternally = false,
	twoWaySync = false,
	autoReconnect = false,

	-- Lets a serve session attach without a human pressing Connect, but only
	-- when exactly one server proves it belongs to this place. This is what
	-- makes Roxo usable from an AI agent, which can start a server but cannot
	-- click a button in Studio.
	autoConnect = true,
	autoConnectPortRange = "34872-34881",
	-- Places paired with a project by a previous connection, keyed by place ID.
	pairedProjects = {},
	showNotifications = true,
	enableSyncFallback = true,
	syncReminderMode = "Notify" :: "None" | "Notify" | "Fullscreen",
	syncReminderPolling = true,
	checkForUpdates = true,
	checkForPrereleases = false,
	autoConnectPlaytestServer = false,
	confirmationBehavior = "Initial" :: "Never" | "Initial" | "Large Changes" | "Unlisted PlaceId",
	largeChangesConfirmationThreshold = 5,
	playSounds = true,
	typecheckingEnabled = false,
	logLevel = "Info",
	timingLogsEnabled = false,
	priorEndpoints = {},
}

local Settings = {}

Settings._values = table.clone(defaultSettings)
Settings._updateListeners = {}
Settings._bindings = {}

if plugin then
	for name, defaultValue in pairs(Settings._values) do
		local savedValue = plugin:GetSetting("Roxo_" .. name)

		-- Roxo is a drop-in replacement for Rojo, so someone installing it over
		-- an existing Rojo setup should keep the preferences they already had
		-- rather than silently reverting to defaults.
		if savedValue == nil then
			savedValue = plugin:GetSetting("Rojo_" .. name)

			if savedValue ~= nil then
				Log.trace("Migrated setting '{}' from Rojo", name)
			end
		end

		if savedValue == nil then
			-- plugin:SetSetting hits disc instead of memory, so it can be slow. Spawn so we don't hang.
			task.spawn(plugin.SetSetting, plugin, "Roxo_" .. name, defaultValue)
			Settings._values[name] = defaultValue
		else
			Settings._values[name] = savedValue
		end
	end
	Log.trace("Loaded settings from plugin store")
end

function Settings:get(name)
	if defaultSettings[name] == nil then
		error("Invalid setings name " .. tostring(name), 2)
	end

	return self._values[name]
end

function Settings:set(name, value)
	self._values[name] = value
	if self._bindings[name] then
		self._bindings[name].set(value)
	end

	if plugin then
		-- plugin:SetSetting hits disc instead of memory, so it can be slow. Spawn so we don't hang.
		task.spawn(plugin.SetSetting, plugin, "Roxo_" .. name, value)
	end

	if self._updateListeners[name] then
		for callback in pairs(self._updateListeners[name]) do
			task.spawn(callback, value)
		end
	end

	Log.trace(string.format("Set setting '%s' to '%s'", name, tostring(value)))
end

function Settings:onChanged(name, callback)
	local listeners = self._updateListeners[name]
	if listeners == nil then
		listeners = {}
		self._updateListeners[name] = listeners
	end
	listeners[callback] = true

	Log.trace(string.format("Added listener for setting '%s' changes", name))

	return function()
		listeners[callback] = nil
		Log.trace(string.format("Removed listener for setting '%s' changes", name))
	end
end

function Settings:getBinding(name)
	local cached = self._bindings[name]
	if cached then
		return cached.bind
	end

	local bind, set = Roact.createBinding(self._values[name])
	self._bindings[name] = {
		bind = bind,
		set = set,
	}

	Log.trace(string.format("Created binding for setting '%s'", name))

	return bind
end

function Settings:getBindings(...: string)
	local bindings = {}
	for i = 1, select("#", ...) do
		local source = select(i, ...)
		bindings[source] = self:getBinding(source)
	end

	return Roact.joinBindings(bindings)
end

return Settings
