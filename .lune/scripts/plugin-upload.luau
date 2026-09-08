local args: any = ...
assert(args, "no arguments passed to script")

local input: buffer = args.BinaryInput

local AssetService = game:GetService("AssetService")
local SerializationService = game:GetService("SerializationService")
local EncodingService = game:GetService("EncodingService")

local input_hash: buffer = EncodingService:ComputeBufferHash(input, Enum.HashAlgorithm.Sha256)
local hex_hash: { string } = table.create(buffer.len(input_hash))
for i = 0, buffer.len(input_hash) - 1 do
	table.insert(hex_hash, string.format("%02x", buffer.readu8(input_hash, i)))
end

print(`Deserializing plugin file (size: {buffer.len(input)} bytes, hash: {table.concat(hex_hash, "")})`)
local plugin = SerializationService:DeserializeInstancesAsync(input)[1]

local UploadDetails = require(plugin.UploadDetails) :: any
local PLUGIN_ID = UploadDetails.assetId
local PLUGIN_NAME = UploadDetails.name
local PLUGIN_DESCRIPTION = UploadDetails.description
local PLUGIN_CREATOR_ID = UploadDetails.creatorId
local PLUGIN_CREATOR_TYPE = UploadDetails.creatorType

assert(typeof(PLUGIN_ID) == "number", "UploadDetails did not contain a number field 'assetId'")
-- Roxo ships placeholder IDs so that a release cut before the plugin has been
-- published cannot silently upload over whatever asset the fork inherited.
assert(
	PLUGIN_ID ~= 0,
	"UploadDetails 'assetId' is still the placeholder 0. Publish the Roxo plugin once by hand, then put its asset ID in plugin/UploadDetails.json."
)
assert(
	PLUGIN_CREATOR_ID ~= 0,
	"UploadDetails 'creatorId' is still the placeholder 0. Set it to the group or user that owns the Roxo plugin."
)
assert(typeof(PLUGIN_NAME) == "string", "UploadDetails did not contain a string field 'name'")
assert(typeof(PLUGIN_DESCRIPTION) == "string", "UploadDetails did not contain a string field 'description'")
assert(typeof(PLUGIN_CREATOR_ID) == "number", "UploadDetails did not contain a number field 'creatorId'")
assert(typeof(PLUGIN_CREATOR_TYPE) == "string", "UploadDetails did not contain a string field 'creatorType'")
assert(
	Enum.AssetCreatorType:FromName(PLUGIN_CREATOR_TYPE) ~= nil,
	"UploadDetails field 'creatorType' was not a valid member of Enum.AssetCreatorType"
)

print(`Uploading to {PLUGIN_ID}`)
print(`Plugin Name: {PLUGIN_NAME}`)
print(`Plugin Description: {PLUGIN_DESCRIPTION}`)

local result, version_or_err = AssetService:CreateAssetVersionAsync(plugin, Enum.AssetType.Plugin, PLUGIN_ID, {
	["Name"] = PLUGIN_NAME,
	["Description"] = PLUGIN_DESCRIPTION,
	["CreatorId"] = PLUGIN_CREATOR_ID,
	["CreatorType"] = Enum.AssetCreatorType:FromName(PLUGIN_CREATOR_TYPE),
})

if result ~= Enum.CreateAssetResult.Success then
	error(`Plugin failed to upload because: {result.Name} - {version_or_err}`)
end

print(`Plugin uploaded successfully. New version is {version_or_err}.`)
