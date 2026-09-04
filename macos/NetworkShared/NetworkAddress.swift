import Darwin
import Foundation

public enum NetworkRuleValidationError: Error, Equatable, Sendable {
    case invalidIPAddress(String)
    case invalidCIDR(String)
    case invalidPrefixLength(Int, IPAddress.Family)
    case invalidPortRange(UInt16, UInt16)
    case invalidDestinationPort(UInt16)
    case invalidDomain(String)
    case invalidSourceMatcher(String)
    case invalidRuleIdentifier(String)
    case invalidMihomoGroup(String)
    case duplicateRuleIdentifier(String)
    case unsupportedSchemaVersion(UInt16)
}

extension NetworkRuleValidationError: CustomStringConvertible {
    public var description: String {
        switch self {
        case let .invalidIPAddress(value):
            return "Invalid IP address: \(value)"
        case let .invalidCIDR(value):
            return "Invalid CIDR: \(value)"
        case let .invalidPrefixLength(length, family):
            return "Invalid prefix length \(length) for \(family.rawValue)"
        case let .invalidPortRange(lower, upper):
            return "Invalid port range: \(lower)-\(upper)"
        case let .invalidDestinationPort(port):
            return "Invalid destination port: \(port)"
        case let .invalidDomain(value):
            return "Invalid domain matcher: \(value)"
        case let .invalidSourceMatcher(value):
            return "Invalid source matcher: \(value)"
        case let .invalidRuleIdentifier(value):
            return "Invalid rule identifier: \(value)"
        case let .invalidMihomoGroup(value):
            return "Invalid Mihomo group: \(value)"
        case let .duplicateRuleIdentifier(value):
            return "Duplicate rule identifier: \(value)"
        case let .unsupportedSchemaVersion(version):
            return "Unsupported capture configuration schema version: \(version)"
        }
    }
}

public struct IPAddress: Hashable, Sendable {
    public enum Family: String, Codable, Hashable, Sendable {
        case ipv4
        case ipv6

        public var bitCount: Int {
            switch self {
            case .ipv4: 32
            case .ipv6: 128
            }
        }
    }

    public let family: Family
    public let bytes: [UInt8]

    public init(_ presentation: String) throws {
        if let bytes = Self.parseIPv4(presentation) {
            family = .ipv4
            self.bytes = bytes
            return
        }
        if let bytes = Self.parseIPv6(presentation) {
            family = .ipv6
            self.bytes = bytes
            return
        }
        throw NetworkRuleValidationError.invalidIPAddress(presentation)
    }

    fileprivate init(family: Family, bytes: [UInt8]) {
        self.family = family
        self.bytes = bytes
    }

    public var presentation: String {
        switch family {
        case .ipv4:
            var address = in_addr()
            withUnsafeMutableBytes(of: &address) { destination in
                destination.copyBytes(from: bytes)
            }
            var buffer = [CChar](repeating: 0, count: Int(INET_ADDRSTRLEN))
            guard inet_ntop(AF_INET, &address, &buffer, socklen_t(buffer.count)) != nil else {
                return bytes.map(String.init).joined(separator: ".")
            }
            return String(decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
        case .ipv6:
            var address = in6_addr()
            withUnsafeMutableBytes(of: &address) { destination in
                destination.copyBytes(from: bytes)
            }
            var buffer = [CChar](repeating: 0, count: Int(INET6_ADDRSTRLEN))
            guard inet_ntop(AF_INET6, &address, &buffer, socklen_t(buffer.count)) != nil else {
                return "::"
            }
            return String(decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
        }
    }

    public var isLoopback: Bool {
        switch family {
        case .ipv4:
            return bytes[0] == 127
        case .ipv6:
            return bytes.dropLast().allSatisfy { $0 == 0 } && bytes.last == 1
        }
    }

    public var isLinkLocal: Bool {
        switch family {
        case .ipv4:
            return bytes[0] == 169 && bytes[1] == 254
        case .ipv6:
            return bytes[0] == 0xFE && (bytes[1] & 0xC0) == 0x80
        }
    }

    /// Addresses that are intended to stay inside a private network.
    ///
    /// This deliberately excludes the shared CGNAT range (`100.64.0.0/10`):
    /// it is not an RFC 1918 private address and may be meaningful to a user's
    /// routing profile.
    public var isPrivate: Bool {
        switch family {
        case .ipv4:
            return bytes[0] == 10
                || (bytes[0] == 172 && (16 ... 31).contains(bytes[1]))
                || (bytes[0] == 192 && bytes[1] == 168)
        case .ipv6:
            if bytes[0] & 0xFE == 0xFC {
                return true
            }
            guard bytes.prefix(10).allSatisfy({ $0 == 0 }),
                  bytes[10] == 0xFF,
                  bytes[11] == 0xFF else {
                return false
            }
            let mappedIPv4 = Array(bytes.suffix(4))
            return mappedIPv4[0] == 10
                || (mappedIPv4[0] == 172 && (16 ... 31).contains(mappedIPv4[1]))
                || (mappedIPv4[0] == 192 && mappedIPv4[1] == 168)
        }
    }

    public var isLocalNetwork: Bool {
        isPrivate || isLoopback || isLinkLocal
    }

    public var isMulticast: Bool {
        switch family {
        case .ipv4:
            return (224 ... 239).contains(bytes[0])
        case .ipv6:
            return bytes[0] == 0xFF
        }
    }

    public var isUnspecified: Bool {
        bytes.allSatisfy { $0 == 0 }
    }

    private static func parseIPv4(_ presentation: String) -> [UInt8]? {
        var address = in_addr()
        let result = presentation.withCString { pointer in
            inet_pton(AF_INET, pointer, &address)
        }
        guard result == 1 else { return nil }
        return withUnsafeBytes(of: &address) { Array($0) }
    }

    private static func parseIPv6(_ presentation: String) -> [UInt8]? {
        var address = in6_addr()
        let result = presentation.withCString { pointer in
            inet_pton(AF_INET6, pointer, &address)
        }
        guard result == 1 else { return nil }
        return withUnsafeBytes(of: &address) { Array($0) }
    }
}

extension IPAddress: Codable {
    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        let presentation = try container.decode(String.self)
        do {
            try self.init(presentation)
        } catch {
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription: String(describing: error)
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(presentation)
    }
}

public struct IPNetwork: Codable, Hashable, Sendable {
    public let address: IPAddress
    public let prefixLength: Int

    public init(address: IPAddress, prefixLength: Int) throws {
        guard (0 ... address.family.bitCount).contains(prefixLength) else {
            throw NetworkRuleValidationError.invalidPrefixLength(prefixLength, address.family)
        }
        self.address = Self.masked(address, prefixLength: prefixLength)
        self.prefixLength = prefixLength
    }

    public init(_ cidr: String) throws {
        let components = cidr.split(separator: "/", omittingEmptySubsequences: false)
        guard components.count == 2,
              let prefixLength = Int(components[1]),
              !components[0].isEmpty
        else {
            throw NetworkRuleValidationError.invalidCIDR(cidr)
        }
        do {
            try self.init(address: IPAddress(String(components[0])), prefixLength: prefixLength)
        } catch let error as NetworkRuleValidationError {
            throw error
        } catch {
            throw NetworkRuleValidationError.invalidCIDR(cidr)
        }
    }

    public var presentation: String {
        "\(address.presentation)/\(prefixLength)"
    }

    public func contains(_ candidate: IPAddress) -> Bool {
        guard candidate.family == address.family else { return false }
        let fullBytes = prefixLength / 8
        let remainingBits = prefixLength % 8

        if fullBytes > 0,
           candidate.bytes.prefix(fullBytes) != address.bytes.prefix(fullBytes) {
            return false
        }
        guard remainingBits > 0 else { return true }
        let mask = UInt8.max << UInt8(8 - remainingBits)
        return (candidate.bytes[fullBytes] & mask) == (address.bytes[fullBytes] & mask)
    }

    private static func masked(_ address: IPAddress, prefixLength: Int) -> IPAddress {
        var bytes = address.bytes
        let fullBytes = prefixLength / 8
        let remainingBits = prefixLength % 8

        if remainingBits > 0 {
            bytes[fullBytes] &= UInt8.max << UInt8(8 - remainingBits)
        }
        let firstZeroByte = fullBytes + (remainingBits > 0 ? 1 : 0)
        if firstZeroByte < bytes.count {
            for index in firstZeroByte ..< bytes.count {
                bytes[index] = 0
            }
        }
        return IPAddress(family: address.family, bytes: bytes)
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        let cidr = try container.decode(String.self)
        do {
            try self.init(cidr)
        } catch {
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription: String(describing: error)
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(presentation)
    }
}

public struct PortRange: Codable, Hashable, Sendable {
    public let lowerBound: UInt16
    public let upperBound: UInt16

    public init(_ port: UInt16) throws {
        try self.init(lowerBound: port, upperBound: port)
    }

    public init(lowerBound: UInt16, upperBound: UInt16) throws {
        guard lowerBound > 0, upperBound >= lowerBound else {
            throw NetworkRuleValidationError.invalidPortRange(lowerBound, upperBound)
        }
        self.lowerBound = lowerBound
        self.upperBound = upperBound
    }

    public func contains(_ port: UInt16) -> Bool {
        port >= lowerBound && port <= upperBound
    }

    private enum CodingKeys: String, CodingKey {
        case lowerBound
        case upperBound
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let lowerBound = try container.decode(UInt16.self, forKey: .lowerBound)
        let upperBound = try container.decode(UInt16.self, forKey: .upperBound)
        do {
            try self.init(lowerBound: lowerBound, upperBound: upperBound)
        } catch {
            throw DecodingError.dataCorruptedError(
                forKey: .upperBound,
                in: container,
                debugDescription: String(describing: error)
            )
        }
    }
}
