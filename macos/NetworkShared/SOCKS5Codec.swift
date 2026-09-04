import Darwin
import Foundation

/// Hard limits used by the SOCKS5 protocol layer. The stream limit bounds data retained while
/// decoding a handshake. The datagram limit is the largest possible IPv4 UDP payload.
public enum SOCKS5Limits: Sendable {
    public static let maximumStreamInputBytes = 1_048_576
    public static let maximumDomainBytes = 255
    public static let maximumCredentialBytes = 255
    public static let maximumUDPDatagramBytes = 65_507
}

public enum SOCKS5CodecError: Error, Equatable, Sendable {
    case inputTooLarge(limit: Int, actual: Int)
    case truncatedFrame(minimumExpected: Int, actual: Int)
    case trailingData(Int)
    case invalidVersion(UInt8)
    case invalidReservedByte(UInt8)
    case invalidAddressType(UInt8)
    case invalidDomainLength(Int)
    case invalidDomainEncoding
    case invalidDomain(String)
    case invalidPort(UInt16)
    case invalidAuthenticationMethod(UInt8)
    case noAcceptableAuthenticationMethods
    case authenticationMethodNotOffered(UInt8)
    case unsupportedAuthenticationMethod(UInt8)
    case invalidUsernameLength(Int)
    case invalidPasswordLength(Int)
    case invalidUsernamePasswordVersion(UInt8)
    case usernamePasswordRejected(UInt8)
    case invalidReplyCode(UInt8)
    case serverRejected(SOCKS5ReplyCode)
    case fragmentedUDPDatagram(UInt8)
    case decoderAlreadyCompleted
}

extension SOCKS5CodecError: CustomStringConvertible {
    public var description: String {
        switch self {
        case let .inputTooLarge(limit, actual):
            return "SOCKS5 input is \(actual) bytes; limit is \(limit)"
        case let .truncatedFrame(expected, actual):
            return "Truncated SOCKS5 frame: expected at least \(expected) bytes, got \(actual)"
        case let .trailingData(count):
            return "SOCKS5 frame has \(count) trailing bytes"
        case let .invalidVersion(version):
            return "Invalid SOCKS version: \(version)"
        case let .invalidReservedByte(value):
            return "Invalid SOCKS5 reserved byte: \(value)"
        case let .invalidAddressType(value):
            return "Unsupported SOCKS5 address type: \(value)"
        case let .invalidDomainLength(length):
            return "Invalid SOCKS5 domain length: \(length)"
        case .invalidDomainEncoding:
            return "SOCKS5 domain is not valid UTF-8"
        case let .invalidDomain(domain):
            return "Invalid SOCKS5 domain: \(domain)"
        case let .invalidPort(port):
            return "Invalid SOCKS5 destination port: \(port)"
        case let .invalidAuthenticationMethod(method):
            return "Unknown SOCKS5 authentication method: \(method)"
        case .noAcceptableAuthenticationMethods:
            return "SOCKS5 server accepted no authentication method"
        case let .authenticationMethodNotOffered(method):
            return "SOCKS5 server selected unoffered authentication method: \(method)"
        case let .unsupportedAuthenticationMethod(method):
            return "SOCKS5 authentication method is unsupported: \(method)"
        case let .invalidUsernameLength(length):
            return "Invalid SOCKS5 username length: \(length)"
        case let .invalidPasswordLength(length):
            return "Invalid SOCKS5 password length: \(length)"
        case let .invalidUsernamePasswordVersion(version):
            return "Invalid SOCKS5 username/password version: \(version)"
        case let .usernamePasswordRejected(status):
            return "SOCKS5 username/password authentication failed with status \(status)"
        case let .invalidReplyCode(code):
            return "Unknown SOCKS5 reply code: \(code)"
        case let .serverRejected(code):
            return "SOCKS5 server rejected the command: \(code.description)"
        case let .fragmentedUDPDatagram(fragment):
            return "SOCKS5 UDP fragmentation is unsupported (fragment \(fragment))"
        case .decoderAlreadyCompleted:
            return "SOCKS5 incremental decoder has already completed"
        }
    }
}

public enum SOCKS5AuthenticationMethod: UInt8, CaseIterable, Hashable, Sendable {
    case noAuthenticationRequired = 0x00
    case gssAPI = 0x01
    case usernamePassword = 0x02
    case noAcceptableMethods = 0xFF
}

public struct SOCKS5MethodSelection: Equatable, Sendable {
    public let method: SOCKS5AuthenticationMethod

    public init(method: SOCKS5AuthenticationMethod) {
        self.method = method
    }
}

public struct SOCKS5UsernamePasswordCredentials: Equatable, Sendable {
    public let username: Data
    public let password: Data

    public init(username: Data, password: Data) throws {
        guard (1 ... SOCKS5Limits.maximumCredentialBytes).contains(username.count) else {
            throw SOCKS5CodecError.invalidUsernameLength(username.count)
        }
        guard (1 ... SOCKS5Limits.maximumCredentialBytes).contains(password.count) else {
            throw SOCKS5CodecError.invalidPasswordLength(password.count)
        }
        self.username = username
        self.password = password
    }

    public init(username: String, password: String) throws {
        try self.init(username: Data(username.utf8), password: Data(password.utf8))
    }
}

public struct SOCKS5UsernamePasswordResponse: Equatable, Sendable {
    public let status: UInt8

    public init(status: UInt8) {
        self.status = status
    }

    public func requireSuccess() throws {
        guard status == 0 else {
            throw SOCKS5CodecError.usernamePasswordRejected(status)
        }
    }
}

public enum SOCKS5Command: UInt8, Hashable, Sendable {
    case connect = 0x01
    case bind = 0x02
    case udpAssociate = 0x03
}

public enum SOCKS5ReplyCode: UInt8, CaseIterable, Hashable, Sendable {
    case succeeded = 0x00
    case generalFailure = 0x01
    case connectionNotAllowed = 0x02
    case networkUnreachable = 0x03
    case hostUnreachable = 0x04
    case connectionRefused = 0x05
    case ttlExpired = 0x06
    case commandNotSupported = 0x07
    case addressTypeNotSupported = 0x08

    public var description: String {
        switch self {
        case .succeeded: "succeeded"
        case .generalFailure: "general failure"
        case .connectionNotAllowed: "connection not allowed"
        case .networkUnreachable: "network unreachable"
        case .hostUnreachable: "host unreachable"
        case .connectionRefused: "connection refused"
        case .ttlExpired: "TTL expired"
        case .commandNotSupported: "command not supported"
        case .addressTypeNotSupported: "address type not supported"
        }
    }
}

public struct SOCKS5Address: Hashable, Sendable {
    public enum Kind: UInt8, Hashable, Sendable {
        case ipv4 = 0x01
        case domain = 0x03
        case ipv6 = 0x04
    }

    public let kind: Kind
    public let ipAddress: IPAddress?
    public let domain: String?

    public init(ipAddress: IPAddress) {
        kind = ipAddress.family == .ipv4 ? .ipv4 : .ipv6
        self.ipAddress = ipAddress
        domain = nil
    }

    public init(domain: String) throws {
        let bytes = Array(domain.utf8)
        guard (1 ... SOCKS5Limits.maximumDomainBytes).contains(bytes.count),
              !bytes.contains(where: { $0 == 0 || $0 < 0x20 || $0 == 0x7F })
        else {
            if bytes.isEmpty || bytes.count > SOCKS5Limits.maximumDomainBytes {
                throw SOCKS5CodecError.invalidDomainLength(bytes.count)
            }
            throw SOCKS5CodecError.invalidDomain(domain)
        }
        kind = .domain
        ipAddress = nil
        self.domain = domain
    }
}

public struct SOCKS5Endpoint: Hashable, Sendable {
    public let address: SOCKS5Address
    public let port: UInt16

    /// A zero port is valid in a UDP ASSOCIATE request and in some server bound-address replies.
    public init(address: SOCKS5Address, port: UInt16) {
        self.address = address
        self.port = port
    }
}

public struct SOCKS5CommandRequest: Equatable, Sendable {
    public let command: SOCKS5Command
    public let endpoint: SOCKS5Endpoint

    public init(command: SOCKS5Command, endpoint: SOCKS5Endpoint) throws {
        if command != .udpAssociate, endpoint.port == 0 {
            throw SOCKS5CodecError.invalidPort(endpoint.port)
        }
        self.command = command
        self.endpoint = endpoint
    }
}

public struct SOCKS5CommandReply: Equatable, Sendable {
    public let code: SOCKS5ReplyCode
    public let boundEndpoint: SOCKS5Endpoint

    public init(code: SOCKS5ReplyCode, boundEndpoint: SOCKS5Endpoint) {
        self.code = code
        self.boundEndpoint = boundEndpoint
    }

    @discardableResult
    public func requireSuccess() throws -> SOCKS5Endpoint {
        guard code == .succeeded else {
            throw SOCKS5CodecError.serverRejected(code)
        }
        return boundEndpoint
    }
}

public struct SOCKS5UDPDatagram: Equatable, Sendable {
    public let destination: SOCKS5Endpoint
    public let payload: Data

    public init(destination: SOCKS5Endpoint, payload: Data) throws {
        guard destination.port > 0 else {
            throw SOCKS5CodecError.invalidPort(destination.port)
        }
        self.destination = destination
        self.payload = payload
    }
}

/// Pure SOCKS5 wire codec. It performs no I/O and is safe to use from any executor.
public enum SOCKS5Codec: Sendable {
    private static let version: UInt8 = 0x05
    private static let usernamePasswordVersion: UInt8 = 0x01

    public static func encodeGreeting(methods: [SOCKS5AuthenticationMethod]) throws -> Data {
        let uniqueMethods = methods.reduce(into: [SOCKS5AuthenticationMethod]()) { result, method in
            if !result.contains(method) { result.append(method) }
        }
        guard !uniqueMethods.isEmpty, uniqueMethods.count <= UInt8.max else {
            throw SOCKS5CodecError.invalidAuthenticationMethod(0xFF)
        }
        guard !uniqueMethods.contains(.noAcceptableMethods) else {
            throw SOCKS5CodecError.invalidAuthenticationMethod(SOCKS5AuthenticationMethod.noAcceptableMethods.rawValue)
        }
        return Data([version, UInt8(uniqueMethods.count)] + uniqueMethods.map(\.rawValue))
    }

    public static func decodeMethodSelection(_ data: Data) throws -> SOCKS5MethodSelection {
        try enforceStreamLimit(data.count)
        guard data.count >= 2 else {
            throw SOCKS5CodecError.truncatedFrame(minimumExpected: 2, actual: data.count)
        }
        guard data.count == 2 else {
            throw SOCKS5CodecError.trailingData(data.count - 2)
        }
        return try parseMethodSelection(Array(data))
    }

    public static func encodeUsernamePasswordRequest(
        credentials: SOCKS5UsernamePasswordCredentials
    ) -> Data {
        var result = Data([usernamePasswordVersion, UInt8(credentials.username.count)])
        result.append(credentials.username)
        result.append(UInt8(credentials.password.count))
        result.append(credentials.password)
        return result
    }

    public static func decodeUsernamePasswordResponse(
        _ data: Data
    ) throws -> SOCKS5UsernamePasswordResponse {
        try enforceStreamLimit(data.count)
        guard data.count >= 2 else {
            throw SOCKS5CodecError.truncatedFrame(minimumExpected: 2, actual: data.count)
        }
        guard data.count == 2 else {
            throw SOCKS5CodecError.trailingData(data.count - 2)
        }
        return try parseUsernamePasswordResponse(Array(data))
    }

    public static func encodeCommandRequest(_ request: SOCKS5CommandRequest) throws -> Data {
        var result = Data([version, request.command.rawValue, 0x00])
        try append(request.endpoint, to: &result)
        try enforceStreamLimit(result.count)
        return result
    }

    public static func decodeCommandReply(_ data: Data) throws -> SOCKS5CommandReply {
        try enforceStreamLimit(data.count)
        let bytes = Array(data)
        guard let frameLength = try commandReplyFrameLength(bytes) else {
            throw SOCKS5CodecError.truncatedFrame(
                minimumExpected: commandReplyMinimumExpected(bytes),
                actual: bytes.count
            )
        }
        guard bytes.count == frameLength else {
            throw SOCKS5CodecError.trailingData(bytes.count - frameLength)
        }
        return try parseCommandReply(Array(bytes.prefix(frameLength)))
    }

    public static func encodeUDPDatagram(_ datagram: SOCKS5UDPDatagram) throws -> Data {
        var result = Data([0x00, 0x00, 0x00])
        try append(datagram.destination, to: &result)
        result.append(datagram.payload)
        guard result.count <= SOCKS5Limits.maximumUDPDatagramBytes else {
            throw SOCKS5CodecError.inputTooLarge(
                limit: SOCKS5Limits.maximumUDPDatagramBytes,
                actual: result.count
            )
        }
        return result
    }

    public static func decodeUDPDatagram(_ data: Data) throws -> SOCKS5UDPDatagram {
        guard data.count <= SOCKS5Limits.maximumUDPDatagramBytes else {
            throw SOCKS5CodecError.inputTooLarge(
                limit: SOCKS5Limits.maximumUDPDatagramBytes,
                actual: data.count
            )
        }
        let bytes = Array(data)
        guard bytes.count >= 4 else {
            throw SOCKS5CodecError.truncatedFrame(minimumExpected: 4, actual: bytes.count)
        }
        guard bytes[0] == 0, bytes[1] == 0 else {
            throw SOCKS5CodecError.invalidReservedByte(bytes[0] != 0 ? bytes[0] : bytes[1])
        }
        guard bytes[2] == 0 else {
            throw SOCKS5CodecError.fragmentedUDPDatagram(bytes[2])
        }
        guard let endpointLength = try endpointWireLength(bytes, addressTypeOffset: 3) else {
            throw SOCKS5CodecError.truncatedFrame(
                minimumExpected: endpointMinimumExpected(bytes, addressTypeOffset: 3),
                actual: bytes.count
            )
        }
        let frameHeaderLength = 3 + endpointLength
        let endpoint = try parseEndpoint(bytes, addressTypeOffset: 3).endpoint
        guard endpoint.port > 0 else {
            throw SOCKS5CodecError.invalidPort(endpoint.port)
        }
        return try SOCKS5UDPDatagram(
            destination: endpoint,
            payload: Data(bytes[frameHeaderLength...])
        )
    }

    static func parseMethodSelection(_ bytes: [UInt8]) throws -> SOCKS5MethodSelection {
        guard bytes[0] == version else { throw SOCKS5CodecError.invalidVersion(bytes[0]) }
        guard let method = SOCKS5AuthenticationMethod(rawValue: bytes[1]) else {
            throw SOCKS5CodecError.invalidAuthenticationMethod(bytes[1])
        }
        return SOCKS5MethodSelection(method: method)
    }

    static func parseUsernamePasswordResponse(
        _ bytes: [UInt8]
    ) throws -> SOCKS5UsernamePasswordResponse {
        guard bytes[0] == usernamePasswordVersion else {
            throw SOCKS5CodecError.invalidUsernamePasswordVersion(bytes[0])
        }
        return SOCKS5UsernamePasswordResponse(status: bytes[1])
    }

    static func commandReplyFrameLength(_ bytes: [UInt8]) throws -> Int? {
        if let first = bytes.first, first != version {
            throw SOCKS5CodecError.invalidVersion(first)
        }
        if bytes.count >= 2, SOCKS5ReplyCode(rawValue: bytes[1]) == nil {
            throw SOCKS5CodecError.invalidReplyCode(bytes[1])
        }
        if bytes.count >= 3, bytes[2] != 0 {
            throw SOCKS5CodecError.invalidReservedByte(bytes[2])
        }
        guard bytes.count >= 4 else { return nil }
        guard let endpointLength = try endpointWireLength(bytes, addressTypeOffset: 3) else {
            return nil
        }
        return 3 + endpointLength
    }

    static func parseCommandReply(_ bytes: [UInt8]) throws -> SOCKS5CommandReply {
        guard bytes[0] == version else { throw SOCKS5CodecError.invalidVersion(bytes[0]) }
        guard let code = SOCKS5ReplyCode(rawValue: bytes[1]) else {
            throw SOCKS5CodecError.invalidReplyCode(bytes[1])
        }
        guard bytes[2] == 0 else { throw SOCKS5CodecError.invalidReservedByte(bytes[2]) }
        let endpoint = try parseEndpoint(bytes, addressTypeOffset: 3).endpoint
        return SOCKS5CommandReply(code: code, boundEndpoint: endpoint)
    }

    static func commandReplyMinimumExpected(_ bytes: [UInt8]) -> Int {
        guard bytes.count >= 4 else { return 4 }
        return 3 + endpointMinimumExpected(bytes, addressTypeOffset: 3)
    }

    private static func append(_ endpoint: SOCKS5Endpoint, to data: inout Data) throws {
        switch endpoint.address.kind {
        case .ipv4, .ipv6:
            guard let ipAddress = endpoint.address.ipAddress else {
                throw SOCKS5CodecError.invalidAddressType(endpoint.address.kind.rawValue)
            }
            data.append(endpoint.address.kind.rawValue)
            data.append(contentsOf: ipAddress.bytes)
        case .domain:
            guard let domain = endpoint.address.domain else {
                throw SOCKS5CodecError.invalidAddressType(endpoint.address.kind.rawValue)
            }
            let bytes = Array(domain.utf8)
            guard (1 ... SOCKS5Limits.maximumDomainBytes).contains(bytes.count) else {
                throw SOCKS5CodecError.invalidDomainLength(bytes.count)
            }
            data.append(SOCKS5Address.Kind.domain.rawValue)
            data.append(UInt8(bytes.count))
            data.append(contentsOf: bytes)
        }
        data.append(UInt8(endpoint.port >> 8))
        data.append(UInt8(endpoint.port & 0xFF))
    }

    private static func endpointWireLength(
        _ bytes: [UInt8],
        addressTypeOffset: Int
    ) throws -> Int? {
        guard bytes.count > addressTypeOffset else { return nil }
        let addressType = bytes[addressTypeOffset]
        switch addressType {
        case SOCKS5Address.Kind.ipv4.rawValue:
            let length = 1 + 4 + 2
            return bytes.count >= addressTypeOffset + length ? length : nil
        case SOCKS5Address.Kind.ipv6.rawValue:
            let length = 1 + 16 + 2
            return bytes.count >= addressTypeOffset + length ? length : nil
        case SOCKS5Address.Kind.domain.rawValue:
            guard bytes.count > addressTypeOffset + 1 else { return nil }
            let domainLength = Int(bytes[addressTypeOffset + 1])
            guard domainLength > 0 else { throw SOCKS5CodecError.invalidDomainLength(0) }
            let length = 1 + 1 + domainLength + 2
            return bytes.count >= addressTypeOffset + length ? length : nil
        default:
            throw SOCKS5CodecError.invalidAddressType(addressType)
        }
    }

    private static func endpointMinimumExpected(_ bytes: [UInt8], addressTypeOffset: Int) -> Int {
        guard bytes.count > addressTypeOffset else { return 1 }
        switch bytes[addressTypeOffset] {
        case SOCKS5Address.Kind.ipv4.rawValue: return 7
        case SOCKS5Address.Kind.ipv6.rawValue: return 19
        case SOCKS5Address.Kind.domain.rawValue:
            guard bytes.count > addressTypeOffset + 1 else { return 2 }
            return 4 + Int(bytes[addressTypeOffset + 1])
        default: return 1
        }
    }

    private static func parseEndpoint(
        _ bytes: [UInt8],
        addressTypeOffset: Int
    ) throws -> (endpoint: SOCKS5Endpoint, nextOffset: Int) {
        guard let wireLength = try endpointWireLength(bytes, addressTypeOffset: addressTypeOffset) else {
            throw SOCKS5CodecError.truncatedFrame(
                minimumExpected: addressTypeOffset + endpointMinimumExpected(bytes, addressTypeOffset: addressTypeOffset),
                actual: bytes.count
            )
        }
        let kind = bytes[addressTypeOffset]
        let address: SOCKS5Address
        let portOffset: Int

        switch kind {
        case SOCKS5Address.Kind.ipv4.rawValue:
            let raw = Array(bytes[(addressTypeOffset + 1) ..< (addressTypeOffset + 5)])
            address = SOCKS5Address(ipAddress: try ipAddress(family: .ipv4, bytes: raw))
            portOffset = addressTypeOffset + 5
        case SOCKS5Address.Kind.ipv6.rawValue:
            let raw = Array(bytes[(addressTypeOffset + 1) ..< (addressTypeOffset + 17)])
            address = SOCKS5Address(ipAddress: try ipAddress(family: .ipv6, bytes: raw))
            portOffset = addressTypeOffset + 17
        case SOCKS5Address.Kind.domain.rawValue:
            let count = Int(bytes[addressTypeOffset + 1])
            let start = addressTypeOffset + 2
            let raw = bytes[start ..< (start + count)]
            guard let domain = String(bytes: raw, encoding: .utf8) else {
                throw SOCKS5CodecError.invalidDomainEncoding
            }
            address = try SOCKS5Address(domain: domain)
            portOffset = start + count
        default:
            throw SOCKS5CodecError.invalidAddressType(kind)
        }

        let port = UInt16(bytes[portOffset]) << 8 | UInt16(bytes[portOffset + 1])
        return (SOCKS5Endpoint(address: address, port: port), addressTypeOffset + wireLength)
    }

    private static func ipAddress(family: IPAddress.Family, bytes: [UInt8]) throws -> IPAddress {
        var buffer = [CChar](repeating: 0, count: family == .ipv4 ? Int(INET_ADDRSTRLEN) : Int(INET6_ADDRSTRLEN))
        let addressFamily = family == .ipv4 ? AF_INET : AF_INET6
        let converted = bytes.withUnsafeBytes { rawBuffer in
            inet_ntop(addressFamily, rawBuffer.baseAddress, &buffer, socklen_t(buffer.count))
        }
        guard converted != nil else {
            throw SOCKS5CodecError.invalidAddressType(family == .ipv4 ? 0x01 : 0x04)
        }
        let presentation = String(
            decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) },
            as: UTF8.self
        )
        return try IPAddress(presentation)
    }

    private static func enforceStreamLimit(_ count: Int) throws {
        guard count <= SOCKS5Limits.maximumStreamInputBytes else {
            throw SOCKS5CodecError.inputTooLarge(
                limit: SOCKS5Limits.maximumStreamInputBytes,
                actual: count
            )
        }
    }
}

/// Stateless helper that validates a server's method choice and creates RFC 1929 auth frames.
public struct SOCKS5ClientAuthenticationNegotiator: Sendable {
    public enum NextStep: Equatable, Sendable {
        case authenticated
        case sendUsernamePassword(Data)
    }

    public let offeredMethods: [SOCKS5AuthenticationMethod]
    public let credentials: SOCKS5UsernamePasswordCredentials?

    /// When credentials are present, username/password is required by default. Callers must opt in
    /// to an unauthenticated fallback explicitly, avoiding an accidental authentication downgrade.
    public init(
        credentials: SOCKS5UsernamePasswordCredentials? = nil,
        allowsNoAuthenticationFallback: Bool = false
    ) {
        self.credentials = credentials
        if credentials == nil {
            offeredMethods = [.noAuthenticationRequired]
        } else if allowsNoAuthenticationFallback {
            offeredMethods = [.usernamePassword, .noAuthenticationRequired]
        } else {
            offeredMethods = [.usernamePassword]
        }
    }

    public func greeting() throws -> Data {
        try SOCKS5Codec.encodeGreeting(methods: offeredMethods)
    }

    public func handle(_ selection: SOCKS5MethodSelection) throws -> NextStep {
        guard selection.method != .noAcceptableMethods else {
            throw SOCKS5CodecError.noAcceptableAuthenticationMethods
        }
        guard offeredMethods.contains(selection.method) else {
            throw SOCKS5CodecError.authenticationMethodNotOffered(selection.method.rawValue)
        }
        switch selection.method {
        case .noAuthenticationRequired:
            return .authenticated
        case .usernamePassword:
            guard let credentials else {
                throw SOCKS5CodecError.unsupportedAuthenticationMethod(selection.method.rawValue)
            }
            return .sendUsernamePassword(
                SOCKS5Codec.encodeUsernamePasswordRequest(credentials: credentials)
            )
        case .gssAPI, .noAcceptableMethods:
            throw SOCKS5CodecError.unsupportedAuthenticationMethod(selection.method.rawValue)
        }
    }
}
