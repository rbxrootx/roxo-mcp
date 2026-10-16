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

	describe("peer ports", function()
		it("should collect advertised ports that were not already probed", function()
			local seen = { [34375] = true }
			local results = {
				{ port = "34375", serverInfo = serverInfo({ peerPorts = { 34374, 34375, 40000 } }) },
				{ port = "34374", serverInfo = serverInfo({ peerPorts = { 40000 } }) },
			}

			local ports = AutoConnect.collectPeerPorts(results, seen)

			expect(#ports).to.equal(2)
			expect(ports[1]).to.equal(34374)
			expect(ports[2]).to.equal(40000)
		end)

		it("should ignore malformed and out-of-range ports", function()
			local ports = AutoConnect.extraPorts({ "x", -1, 0, 70000, 34375 }, {})

			expect(#ports).to.equal(1)
			expect(ports[1]).to.equal(34375)
		end)

		it("should cap how many peer ports one poll probes", function()
			local many = {}
			for port = 40000, 40000 + Config.maxPeerPorts * 2 do
				table.insert(many, port)
			end

			local ports = AutoConnect.collectPeerPorts({ { serverInfo = serverInfo({ peerPorts = many }) } }, {})

			expect(#ports).to.equal(Config.maxPeerPorts)
		end)

		it("should judge a server found through a peer by the same rules", function()
			-- Finding a server through another one's peer list is only a new
			-- place to look. It must not make that server any easier to accept.
			local results = {}
			results[3] = {
				port = "40000",
				serverInfo = serverInfo({ expectedPlaceIds = { 999 }, peerPorts = { 34375 } }),
			}

			local result = AutoConnect.classify(results, { placeId = 123 })

			expect(#result.candidates).to.equal(0)
			expect(#result.rejected).to.equal(1)
			expect(result.peerPorts[1]).to.equal(34375)
		end)
	end)

	describe("reject codes", function()
		it("should give a stable code with every rejection", function()
			local cases = {
				{ serverInfo({ expectedPlaceIds = { 1 } }), { placeId = 2 }, "NotInServePlaceIds" },
				{ serverInfo({ unexpectedPlaceIds = { 2 } }), { placeId = 2 }, "BlockedPlace" },
				{ serverInfo({ autoConnect = "off" }), { placeId = 2 }, "AutoConnectOff" },
				{ serverInfo(), { placeId = 0 }, "UnpublishedPlace" },
				{ serverInfo({ autoConnect = "always" }), { placeId = 2 }, "AlwaysOnPublishedPlace" },
				{ serverInfo(), { placeId = 2 }, "NoIdentityMatch" },
				{ serverInfo({ protocolVersion = -1 }), { placeId = 2 }, "ProtocolMismatch" },
			}

			for _, case in cases do
				local match, why, code = AutoConnect.evaluate(case[1], case[2])
				expect(match).to.equal(nil)
				expect(type(why)).to.equal("string")
				expect(code).to.equal(AutoConnect.RejectCode[case[3]])
			end
		end)
	end)

	describe("probe reports", function()
		local reporting = { "auto-connect", "probe-report" }
		local context = { placeId = 99657347679962, gameId = 42 }

		it("should identify the place in the probe", function()
			local query = AutoConnect.probeQuery(context)

			expect(string.find(query, "client=roxo%-plugin%-probe")).to.be.ok()
			expect(string.find(query, "placeId=99657347679962", 1, true)).to.be.ok()
			expect(string.find(query, "gameId=42", 1, true)).to.be.ok()
		end)

		it("should only report to servers that accept reports", function()
			local rejected = {
				{
					baseUrl = "http://localhost:1",
					serverInfo = serverInfo({ capabilities = reporting }),
					reason = "r",
					code = AutoConnect.RejectCode.NotInServePlaceIds,
				},
				-- An upstream Rojo server has no report route.
				{ baseUrl = "http://localhost:2", serverInfo = serverInfo(), reason = "r", code = "x" },
			}

			local verdicts = AutoConnect.buildVerdicts(rejected, {}, context)

			expect(#verdicts).to.equal(1)
			expect(verdicts[1].baseUrl).to.equal("http://localhost:1")
			expect(verdicts[1].body.outcome).to.equal("rejected")
			expect(verdicts[1].body.code).to.equal("not-in-serve-place-ids")
			-- Strings, so no JSON encoder can turn the ID into exponent form.
			expect(verdicts[1].body.placeId).to.equal("99657347679962")
		end)

		it("should tell every tied server about the conflict", function()
			local tied = {
				{ port = "34374", baseUrl = "a", serverInfo = serverInfo({ capabilities = reporting }) },
				{ port = "34375", baseUrl = "b", serverInfo = serverInfo({ capabilities = reporting }) },
			}

			local verdicts = AutoConnect.buildVerdicts({}, tied, context)

			expect(#verdicts).to.equal(2)
			for _, verdict in verdicts do
				expect(verdict.body.outcome).to.equal("ambiguous")
				expect(#verdict.body.conflictingPorts).to.equal(2)
			end
		end)
	end)

	describe("parsePortRanges", function()
		it("should parse several ranges and single ports", function()
			local ranges = AutoConnect.parsePortRanges("34370-34379, 34872-34881,40000")

			expect(#ranges).to.equal(3)
			expect(ranges[1].low).to.equal(34370)
			expect(ranges[1].high).to.equal(34379)
			expect(ranges[2].low).to.equal(34872)
			expect(ranges[3].low).to.equal(40000)
			expect(ranges[3].high).to.equal(40000)
		end)

		it("should upgrade the old single-range default", function()
			local ranges = AutoConnect.parsePortRanges("34872-34881")

			expect(#ranges).to.equal(#Config.defaultPortRanges)
			expect(ranges[1].low).to.equal(Config.defaultPortRanges[1].min)
		end)

		it("should fall back to the defaults when given nothing", function()
			local ranges = AutoConnect.parsePortRanges(nil)

			expect(#ranges).to.equal(#Config.defaultPortRanges)
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
