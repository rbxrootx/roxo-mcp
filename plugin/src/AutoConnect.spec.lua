return function()
	local AutoConnect = require(script.Parent.AutoConnect)
	local Config = require(script.Parent.Config)

	local function serverInfo(overrides)
		local info = {
			protocolVersion = Config.protocolVersion,
			projectName = "Test",
			projectId = "roxo-aaaa",
			autoConnect = "matching",
		}

		for key, value in pairs(overrides or {}) do
			info[key] = value
		end

		return info
	end

	describe("evaluate", function()
		it("should match a place listed in servePlaceIds", function()
			local match = AutoConnect.evaluate(serverInfo({ expectedPlaceIds = { 123 } }), { placeId = 123 })

			expect(match).to.be.ok()
			expect(match.reason).to.equal(AutoConnect.MatchReason.ServePlaceIds)
		end)

		it("should refuse a place that is not listed in servePlaceIds", function()
			local match = AutoConnect.evaluate(serverInfo({ expectedPlaceIds = { 123 } }), { placeId = 999 })

			expect(match).to.equal(nil)
		end)

		it("should refuse a place listed in blockedPlaceIds even when it is also allowed", function()
			local info = serverInfo({
				expectedPlaceIds = { 123 },
				unexpectedPlaceIds = { 123 },
			})

			expect(AutoConnect.evaluate(info, { placeId = 123 })).to.equal(nil)
		end)

		it("should match a place whose universe matches gameId", function()
			local match = AutoConnect.evaluate(serverInfo({ gameId = 55 }), { placeId = 123, gameId = 55 })

			expect(match).to.be.ok()
			expect(match.reason).to.equal(AutoConnect.MatchReason.GameId)
		end)

		it("should match a place that was previously paired with the project", function()
			local match = AutoConnect.evaluate(serverInfo(), {
				placeId = 123,
				pairings = { ["123"] = { projectId = "roxo-aaaa" } },
			})

			expect(match).to.be.ok()
			expect(match.reason).to.equal(AutoConnect.MatchReason.Paired)
		end)

		it("should not match a pairing that belongs to a different project", function()
			local match = AutoConnect.evaluate(serverInfo({ projectId = "roxo-bbbb" }), {
				placeId = 123,
				pairings = { ["123"] = { projectId = "roxo-aaaa" } },
			})

			expect(match).to.equal(nil)
		end)

		it("should refuse an unidentified place", function()
			local match, why = AutoConnect.evaluate(serverInfo(), { placeId = 123 })

			expect(match).to.equal(nil)
			expect(type(why)).to.equal("string")
		end)

		it("should refuse an unpublished place unless the project opts in", function()
			expect(AutoConnect.evaluate(serverInfo(), { placeId = 0 })).to.equal(nil)

			local match = AutoConnect.evaluate(serverInfo({ autoConnect = "always" }), { placeId = 0 })
			expect(match).to.be.ok()
			expect(match.reason).to.equal(AutoConnect.MatchReason.Declared)
		end)

		it("should not let 'always' attach to a published place", function()
			-- A scratch project left on "always" must not follow the developer
			-- into whatever real game they open next. Once a place has an ID,
			-- it has to qualify on its own merits.
			local info = serverInfo({ autoConnect = "always" })

			expect(AutoConnect.evaluate(info, { placeId = 113452345799420 })).to.equal(nil)
		end)

		it("should still match a published place that 'always' also identifies", function()
			-- Narrowing "always" must not disqualify a place that would have
			-- matched anyway through a strict tier.
			local info = serverInfo({ autoConnect = "always", expectedPlaceIds = { 123 } })

			local match = AutoConnect.evaluate(info, { placeId = 123 })

			expect(match).to.be.ok()
			expect(match.reason).to.equal(AutoConnect.MatchReason.ServePlaceIds)
		end)

		it("should honor a project that turns auto-connect off", function()
			local info = serverInfo({ autoConnect = "off", expectedPlaceIds = { 123 } })

			expect(AutoConnect.evaluate(info, { placeId = 123 })).to.equal(nil)
		end)

		it("should treat a server with no policy as using the default", function()
			-- Upstream Rojo servers report no policy at all. One whose project
			-- names this place should still be connectable.
			local info = serverInfo({ autoConnect = nil, expectedPlaceIds = { 123 } })

			expect(AutoConnect.evaluate(info, { placeId = 123 })).to.be.ok()
		end)

		it("should refuse a server speaking a different protocol version", function()
			local info = serverInfo({
				protocolVersion = Config.protocolVersion + 1,
				expectedPlaceIds = { 123 },
			})

			expect(AutoConnect.evaluate(info, { placeId = 123 })).to.equal(nil)
		end)
	end)

	describe("choose", function()
		it("should choose the only candidate", function()
			local chosen = AutoConnect.choose({ { port = "34872" } })

			expect(chosen).to.be.ok()
			expect(chosen.port).to.equal("34872")
		end)

		it("should choose nothing when there are no candidates", function()
			expect(AutoConnect.choose({})).to.equal(nil)
		end)

		it("should refuse to guess between several candidates", function()
			-- Even a strictly stronger claim does not win. Two projects both
			-- claiming one place is a misconfiguration, and connecting to
			-- either could overwrite the wrong game.
			local strong = { match = { reason = AutoConnect.MatchReason.ServePlaceIds, strength = 4 } }
			local weak = { match = { reason = AutoConnect.MatchReason.Declared, strength = 1 } }

			local chosen, tied = AutoConnect.choose({ strong, weak })

			expect(chosen).to.equal(nil)
			expect(#tied).to.equal(2)
		end)
	end)

	describe("parsePortRange", function()
		it("should parse a range", function()
			local low, high = AutoConnect.parsePortRange("34872-34881")

			expect(low).to.equal(34872)
			expect(high).to.equal(34881)
		end)

		it("should accept a single port", function()
			local low, high = AutoConnect.parsePortRange("34872")

			expect(low).to.equal(34872)
			expect(high).to.equal(34872)
		end)

		it("should fall back to the default range when the text is nonsense", function()
			local low, high = AutoConnect.parsePortRange("not a range")

			expect(low).to.equal(Config.defaultPortRange.min)
			expect(high).to.equal(Config.defaultPortRange.max)
		end)

		it("should clamp a range that is too wide to scan", function()
			local low, high = AutoConnect.parsePortRange("30000-60000")

			expect(low).to.equal(30000)
			expect(high).to.equal(30000 + Config.maxPortRangeSize)
		end)
	end)
end
