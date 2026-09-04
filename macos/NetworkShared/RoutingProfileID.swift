import Foundation

/// A module-independent profile identity used on the App/NetworkExtension
/// boundary.
///
/// The app owns its richer `ProfileID` model. The shared routing layer carries
/// only this canonical UUID string so provider payloads do not depend on app
/// model types or on UUID's keyed Codable representation.
public struct RoutingProfileID: Hashable, Sendable, CustomStringConvertible {
    public let rawValue: String

    public init(_ uuid: UUID) {
        rawValue = uuid.uuidString.lowercased()
    }

    public init(rawValue: String) throws {
        let candidate = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let uuid = UUID(uuidString: candidate) else {
            throw RoutingProfileIDError.invalidUUID(rawValue)
        }
        self.init(uuid)
    }

    public var uuid: UUID {
        // Construction and decoding validate `rawValue`, so this cannot fail.
        UUID(uuidString: rawValue)!
    }

    public var description: String {
        rawValue
    }
}

extension RoutingProfileID: Codable {
    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        let rawValue = try container.decode(String.self)
        do {
            try self.init(rawValue: rawValue)
        } catch {
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription: "Routing profile identifier must be a valid UUID string."
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

public enum RoutingProfileIDError: Error, Equatable, Sendable {
    case invalidUUID(String)
}

extension RoutingProfileIDError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case let .invalidUUID(value):
            "Routing profile identifier is not a valid UUID: \(value)"
        }
    }
}
