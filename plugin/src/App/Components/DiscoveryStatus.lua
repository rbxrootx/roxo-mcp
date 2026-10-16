--[[
	Shows what auto-connect is doing while nothing is connected: searching,
	found servers it may not use and why, or found several that claim this
	place. Without it, those reasons only reach the Output window, and a
	panel that just says "Connect" looks the same as a broken plugin.
]]

local Roxo = script:FindFirstAncestor("Roxo")
local Plugin = Roxo.Plugin
local Packages = Roxo.Packages

local Roact = require(Packages.Roact)

local Theme = require(Plugin.App.Theme)
local Assets = require(Plugin.Assets)

local SlicedImage = require(Plugin.App.Components.SlicedImage)
local Tooltip = require(Plugin.App.Components.Tooltip)

local e = Roact.createElement

-- Enough to show the usual one or two servers; the panel is short, and the
-- full list is still logged.
local MAX_SERVER_ROWS = 2

-- Project names come from project files, so they may contain markup.
local function escapeRichText(text)
	return (
		string.gsub(tostring(text or ""), "[&<>\"']", {
			["&"] = "&amp;",
			["<"] = "&lt;",
			[">"] = "&gt;",
			['"'] = "&quot;",
			["'"] = "&apos;",
		})
	)
end

local function headline(discovery)
	local count = #(discovery.servers or {})

	if discovery.status == "off" then
		return "Auto-connect is off", "Turn it on in Settings, or press Connect."
	elseif discovery.status == "declined" then
		return if count == 1
			then "Found a server, but it can't auto-connect"
			else string.format("Found %d servers, but none can auto-connect", count),
			"Press Connect to sync anyway, or fix the project below."
	elseif discovery.status == "ambiguous" then
		return string.format("%d projects claim this place", count),
			"Roxo won't guess between them. Stop one, or press Connect."
	end

	return "Looking for a Roxo server", discovery.scanning or ""
end

local function ServerRow(props)
	return Theme.with(function(theme)
		return e("Frame", {
			Size = UDim2.new(1, 0, 0, 0),
			AutomaticSize = Enum.AutomaticSize.Y,
			BackgroundTransparency = 1,
			LayoutOrder = props.layoutOrder,
		}, {
			Layout = e("UIListLayout", {
				FillDirection = Enum.FillDirection.Vertical,
				SortOrder = Enum.SortOrder.LayoutOrder,
				Padding = UDim.new(0, 2),
			}),

			Name = e("TextLabel", {
				Text = string.format(
					'%s  <font transparency="0.4">:%s</font>',
					escapeRichText(props.projectName),
					escapeRichText(props.port)
				),
				RichText = true,
				FontFace = theme.Font.Main,
				TextSize = theme.TextSize.Body,
				TextColor3 = theme.TextColor,
				TextTransparency = props.transparency,
				TextXAlignment = Enum.TextXAlignment.Left,
				TextTruncate = Enum.TextTruncate.AtEnd,
				Size = UDim2.new(1, 0, 0, theme.TextSize.Body + 2),
				BackgroundTransparency = 1,
				LayoutOrder = 1,
			}),

			Reason = if props.reason
				then e("TextLabel", {
					Text = props.reason,
					FontFace = theme.Font.Thin,
					TextSize = theme.TextSize.Small,
					TextColor3 = theme.SubTextColor,
					TextTransparency = props.transparency,
					TextXAlignment = Enum.TextXAlignment.Left,
					TextWrapped = true,
					Size = UDim2.new(1, 0, 0, 0),
					AutomaticSize = Enum.AutomaticSize.Y,
					BackgroundTransparency = 1,
					LayoutOrder = 2,
				})
				else nil,
		})
	end)
end

local function DiscoveryStatus(props)
	local discovery = props.discovery
	if discovery == nil then
		return nil
	end

	return Theme.with(function(theme)
		local title, subtitle = headline(discovery)

		local attention = discovery.status == "declined" or discovery.status == "ambiguous"
		local dotColor = if attention then theme.Diff.Warning else theme.BrandColor

		local rows = {}
		local servers = discovery.servers or {}
		for index, server in servers do
			if index > MAX_SERVER_ROWS then
				break
			end
			rows["Server" .. index] = e(ServerRow, {
				projectName = server.projectName,
				port = server.port,
				-- Every tied server has the same reason, so it is said once in
				-- the subtitle rather than under each row.
				reason = if discovery.status == "declined" then server.reason else nil,
				transparency = props.transparency,
				layoutOrder = 10 + index,
			})
		end

		if #servers > MAX_SERVER_ROWS then
			rows.More = e("TextLabel", {
				Text = string.format("+%d more in the Output window", #servers - MAX_SERVER_ROWS),
				FontFace = theme.Font.Thin,
				TextSize = theme.TextSize.Small,
				TextColor3 = theme.SubTextColor,
				TextTransparency = props.transparency,
				TextXAlignment = Enum.TextXAlignment.Left,
				Size = UDim2.new(1, 0, 0, theme.TextSize.Small + 2),
				BackgroundTransparency = 1,
				LayoutOrder = 100,
			})
		end

		return e(SlicedImage, {
			slice = Assets.Slices.RoundedBackground,
			color = theme.BorderedContainer.BackgroundColor,
			transparency = props.transparency,
			size = UDim2.new(1, 0, 0, 0),
			automaticSize = Enum.AutomaticSize.Y,
			layoutOrder = props.layoutOrder,
		}, {
			Border = e(SlicedImage, {
				slice = Assets.Slices.RoundedBorder,
				color = theme.BorderedContainer.BorderColor,
				transparency = props.transparency,
				size = UDim2.new(1, 0, 1, 0),
			}),

			-- A separate frame, so the list layout does not also arrange the
			-- border above.
			Content = e("Frame", {
				Size = UDim2.new(1, 0, 0, 0),
				AutomaticSize = Enum.AutomaticSize.Y,
				BackgroundTransparency = 1,
				ZIndex = 2,
			}, {
				Padding = e("UIPadding", {
					PaddingLeft = UDim.new(0, 12),
					PaddingRight = UDim.new(0, 12),
					PaddingTop = UDim.new(0, 10),
					PaddingBottom = UDim.new(0, 10),
				}),

				Layout = e("UIListLayout", {
					FillDirection = Enum.FillDirection.Vertical,
					SortOrder = Enum.SortOrder.LayoutOrder,
					Padding = UDim.new(0, 6),
				}),

				Title = e("Frame", {
					Size = UDim2.new(1, 0, 0, theme.TextSize.Body + 2),
					BackgroundTransparency = 1,
					LayoutOrder = 1,
				}, {
					Dot = e("ImageLabel", {
						Image = Assets.Images.Circles[16],
						ImageColor3 = dotColor,
						ImageTransparency = props.transparency,
						ScaleType = Enum.ScaleType.Fit,
						Size = UDim2.new(0, 8, 0, 8),
						Position = UDim2.new(0, 0, 0.5, 0),
						AnchorPoint = Vector2.new(0, 0.5),
						BackgroundTransparency = 1,
					}),

					Text = e("TextLabel", {
						Text = title,
						FontFace = theme.Font.Bold,
						TextSize = theme.TextSize.Body,
						TextColor3 = theme.TextColor,
						TextTransparency = props.transparency,
						TextXAlignment = Enum.TextXAlignment.Left,
						TextTruncate = Enum.TextTruncate.AtEnd,
						Size = UDim2.new(1, -16, 1, 0),
						Position = UDim2.new(0, 16, 0, 0),
						BackgroundTransparency = 1,
					}, {
						Tip = if discovery.scanning
							then e(Tooltip.Trigger, {
								text = "Scanning " .. discovery.scanning,
							})
							else nil,
					}),
				}),

				Subtitle = if subtitle ~= ""
					then e("TextLabel", {
						Text = subtitle,
						FontFace = theme.Font.Thin,
						TextSize = theme.TextSize.Small,
						TextColor3 = theme.SubTextColor,
						TextTransparency = props.transparency,
						TextXAlignment = Enum.TextXAlignment.Left,
						TextWrapped = true,
						Size = UDim2.new(1, 0, 0, 0),
						AutomaticSize = Enum.AutomaticSize.Y,
						BackgroundTransparency = 1,
						LayoutOrder = 2,
					})
					else nil,

				Rows = Roact.createFragment(rows),
			}),
		})
	end)
end

return DiscoveryStatus
