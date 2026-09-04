import Foundation

private struct SOCKS5BoundedStreamBuffer: Sendable {
    private(set) var bytes: [UInt8] = []

    mutating func append(_ data: Data) throws {
        guard data.count <= SOCKS5Limits.maximumStreamInputBytes - bytes.count else {
            let actual = bytes.count.addingReportingOverflow(data.count)
            throw SOCKS5CodecError.inputTooLarge(
                limit: SOCKS5Limits.maximumStreamInputBytes,
                actual: actual.overflow ? Int.max : actual.partialValue
            )
        }
        bytes.append(contentsOf: data)
    }

    mutating func consume(_ count: Int) -> Data {
        let remainder = Data(bytes.dropFirst(count))
        bytes.removeAll(keepingCapacity: false)
        return remainder
    }
}

/// Incrementally parses the two-byte server method-selection response.
public struct SOCKS5MethodSelectionDecoder: Sendable {
    private var buffer = SOCKS5BoundedStreamBuffer()
    public private(set) var remainingData = Data()
    public private(set) var isComplete = false

    public init() {}

    public mutating func append(_ data: Data) throws -> SOCKS5MethodSelection? {
        guard !isComplete else { throw SOCKS5CodecError.decoderAlreadyCompleted }
        try buffer.append(data)
        if let first = buffer.bytes.first, first != 0x05 {
            throw SOCKS5CodecError.invalidVersion(first)
        }
        guard buffer.bytes.count >= 2 else { return nil }
        let result = try SOCKS5Codec.parseMethodSelection(Array(buffer.bytes.prefix(2)))
        remainingData = buffer.consume(2)
        isComplete = true
        return result
    }
}

/// Incrementally parses the two-byte RFC 1929 username/password response.
public struct SOCKS5UsernamePasswordResponseDecoder: Sendable {
    private var buffer = SOCKS5BoundedStreamBuffer()
    public private(set) var remainingData = Data()
    public private(set) var isComplete = false

    public init() {}

    public mutating func append(_ data: Data) throws -> SOCKS5UsernamePasswordResponse? {
        guard !isComplete else { throw SOCKS5CodecError.decoderAlreadyCompleted }
        try buffer.append(data)
        if let first = buffer.bytes.first, first != 0x01 {
            throw SOCKS5CodecError.invalidUsernamePasswordVersion(first)
        }
        guard buffer.bytes.count >= 2 else { return nil }
        let result = try SOCKS5Codec.parseUsernamePasswordResponse(Array(buffer.bytes.prefix(2)))
        remainingData = buffer.consume(2)
        isComplete = true
        return result
    }
}

/// Incrementally parses a variable-length CONNECT or UDP ASSOCIATE server response. Bytes read
/// after the reply are retained in `remainingData`, which is important when a server coalesces the
/// first proxied payload with its success response.
public struct SOCKS5CommandReplyDecoder: Sendable {
    private var buffer = SOCKS5BoundedStreamBuffer()
    public private(set) var remainingData = Data()
    public private(set) var isComplete = false

    public init() {}

    public mutating func append(_ data: Data) throws -> SOCKS5CommandReply? {
        guard !isComplete else { throw SOCKS5CodecError.decoderAlreadyCompleted }
        try buffer.append(data)
        guard let frameLength = try SOCKS5Codec.commandReplyFrameLength(buffer.bytes) else {
            return nil
        }
        let result = try SOCKS5Codec.parseCommandReply(Array(buffer.bytes.prefix(frameLength)))
        remainingData = buffer.consume(frameLength)
        isComplete = true
        return result
    }
}
