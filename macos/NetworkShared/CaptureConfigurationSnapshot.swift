import Foundation

public struct CaptureConfigurationSnapshot: Codable, Hashable, Sendable {
    public static let currentSchemaVersion: UInt16 = 1

    public let schemaVersion: UInt16
    public let revision: UInt64
    public let generationID: UUID
    public let createdAt: Date
    public let rules: [CaptureRule]

    public init(
        schemaVersion: UInt16 = Self.currentSchemaVersion,
        revision: UInt64,
        generationID: UUID = UUID(),
        createdAt: Date = Date(),
        rules: [CaptureRule]
    ) throws {
        self.schemaVersion = schemaVersion
        self.revision = revision
        self.generationID = generationID
        self.createdAt = createdAt
        self.rules = rules
        try validate()
    }

    public func validate() throws {
        guard schemaVersion == Self.currentSchemaVersion else {
            throw NetworkRuleValidationError.unsupportedSchemaVersion(schemaVersion)
        }
        var identifiers = Set<String>()
        for rule in rules {
            try rule.validate()
            guard identifiers.insert(rule.id).inserted else {
                throw NetworkRuleValidationError.duplicateRuleIdentifier(rule.id)
            }
        }
    }

    private enum CodingKeys: String, CodingKey {
        case schemaVersion
        case revision
        case generationID
        case createdAt
        case rules
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try container.decode(UInt16.self, forKey: .schemaVersion)
        revision = try container.decode(UInt64.self, forKey: .revision)
        generationID = try container.decode(UUID.self, forKey: .generationID)
        createdAt = try container.decode(Date.self, forKey: .createdAt)
        rules = try container.decode([CaptureRule].self, forKey: .rules)
        do {
            try validate()
        } catch {
            throw DecodingError.dataCorruptedError(
                forKey: .schemaVersion,
                in: container,
                debugDescription: String(describing: error)
            )
        }
    }
}
