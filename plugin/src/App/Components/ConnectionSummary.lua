--[[
	The connected page's description of the session: which project, where, and
	how this place came to be attached to it.

	The "how" matters most when nobody pressed Connect. An unattended sync that
	does not say why it happened is indistinguishable from one that happened by
	mistake, so the match reason is always on screen.
]]

local Roxo = script:FindFirstAncestor("Roxo")
local Plugin = Roxo.Plugin
local Packages = Roxo.Packages

local Roact = require(Packages.Roact)

local Theme = require(Plugin.App.Theme)

local BorderedContainer = require(Plugin.App.Components.BorderedContainer)
local Tag = require(Plugin.App.Components.Tag)
local Tooltip = require(Plugin.App.Components.Tooltip)

local e = Roact.createElement

local MATCH_LABELS = {
	servePlaceIds = { "servePlaceIds", "The project lists this place in servePlaceIds." },
	gameId = { "gameId", "The project's gameId matches this place's universe." },
	paired = { "paired", "This place was connected to this project by hand before." },
	declared = { "always", 'The project sets "autoConnect": "always", and this place is unpublished.' },
}

local function describeMatch(autoConnected, matchReason)
	if not autoConnected then
		return "manual", "Not auto-connected: someone pressed Connect, or the plugin reconnected to the last project."
	end

	local label = MATCH_LABELS[matchReason]
	if label then
		return "auto: " .. label[1], label[2]
	end

	return "auto", "Connected automatically."
end

local function describePlace(placeId)
	if placeId == nil then
		return ""
	elseif placeId == 0 then
		return "Unpublished place"
	end
	return "Place " .. tostring(placeId)
end

local function ConnectionSummary(props)
	return Theme.with(function(theme)
		local matchText, matchTip = describeMatch(props.autoConnected, props.matchReason)

		return e(BorderedContainer, {
			transparency = props.transparency,
			size = UDim2.new(1, 0, 0, 84),
			layoutOrder = props.layoutOrder,
		}, {
			Padding = e("UIPadding", {
				PaddingLeft = UDim.new(0, 15),
				PaddingRight = UDim.new(0, 15),
				PaddingTop = UDim.new(0, 10),
				PaddingBottom = UDim.new(0, 10),
			}),

			Layout = e("UIListLayout", {
				VerticalAlignment = Enum.VerticalAlignment.Center,
				FillDirection = Enum.FillDirection.Vertical,
				SortOrder = Enum.SortOrder.LayoutOrder,
				Padding = UDim.new(0, 5),
			}),

			ProjectName = e("TextLabel", {
				Text = props.projectName or "",
				FontFace = theme.Font.Bold,
				TextSize = theme.TextSize.Large,
				TextColor3 = theme.ConnectionDetails.ProjectNameColor,
				TextTransparency = props.transparency,
				TextXAlignment = Enum.TextXAlignment.Left,
				TextTruncate = Enum.TextTruncate.AtEnd,
				Size = UDim2.new(1, 0, 0, theme.TextSize.Large),
				BackgroundTransparency = 1,
				LayoutOrder = 1,
			}, {
				Tip = if props.projectPath
					then e(Tooltip.Trigger, {
						text = props.projectPath,
					})
					else nil,
			}),

			Address = e("TextLabel", {
				Text = props.address or "",
				FontFace = theme.Font.Code,
				TextSize = theme.TextSize.Medium,
				TextColor3 = theme.ConnectionDetails.AddressColor,
				TextTransparency = props.transparency,
				TextXAlignment = Enum.TextXAlignment.Left,
				TextTruncate = Enum.TextTruncate.AtEnd,
				Size = UDim2.new(1, 0, 0, theme.TextSize.Medium),
				BackgroundTransparency = 1,
				LayoutOrder = 2,
			}),

			Footer = e("Frame", {
				Size = UDim2.new(1, 0, 0, theme.TextSize.Medium),
				BackgroundTransparency = 1,
				LayoutOrder = 3,
			}, {
				Layout = e("UIListLayout", {
					FillDirection = Enum.FillDirection.Horizontal,
					VerticalAlignment = Enum.VerticalAlignment.Center,
					SortOrder = Enum.SortOrder.LayoutOrder,
					Padding = UDim.new(0, 8),
				}),

				-- Tag does not take children, so the tooltip hangs off a wrapper.
				Match = e("Frame", {
					Size = UDim2.new(0, 0, 1, 0),
					AutomaticSize = Enum.AutomaticSize.X,
					BackgroundTransparency = 1,
					LayoutOrder = 1,
				}, {
					Tag = e(Tag, {
						text = matchText,
						color = if props.autoConnected then theme.BrandColor else theme.SubTextColor,
						transparency = props.transparency,
					}),
					Tip = e(Tooltip.Trigger, {
						text = matchTip,
					}),
				}),

				Place = e("TextLabel", {
					Text = describePlace(props.placeId),
					FontFace = theme.Font.Thin,
					TextSize = theme.TextSize.Small,
					TextColor3 = theme.SubTextColor,
					TextTransparency = props.transparency,
					TextXAlignment = Enum.TextXAlignment.Left,
					Size = UDim2.new(0, 0, 1, 0),
					AutomaticSize = Enum.AutomaticSize.X,
					BackgroundTransparency = 1,
					LayoutOrder = 2,
				}),
			}),
		})
	end)
end

return ConnectionSummary
