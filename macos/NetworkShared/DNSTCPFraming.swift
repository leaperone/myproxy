import Foundation

public enum DNSTCPFramingError: Error { case invalidLength, bufferLimit }

/// DNS TCP transport framing; the DNS payload remains unchanged.
public struct DNSTCPFraming: Sendable {
    private var bytes = Data()
    public init() {}

    public static func encode(_ payload: Data) throws -> Data {
        guard (12...65_535).contains(payload.count) else { throw DNSTCPFramingError.invalidLength }
        var frame = Data([UInt8(payload.count >> 8), UInt8(payload.count & 255)])
        frame.append(payload)
        return frame
    }

    public mutating func append(_ chunk: Data) throws {
        guard bytes.count + chunk.count <= 2 * 65_537 else { throw DNSTCPFramingError.bufferLimit }
        bytes.append(chunk)
    }

    public mutating func next() throws -> Data? {
        guard bytes.count >= 2 else { return nil }
        let length = Int(bytes[bytes.startIndex]) * 256 + Int(bytes[bytes.startIndex + 1])
        guard length >= 12 else { throw DNSTCPFramingError.invalidLength }
        guard bytes.count >= length + 2 else { return nil }
        let payload = Data(bytes.dropFirst(2).prefix(length))
        bytes = Data(bytes.dropFirst(length + 2))
        return payload
    }
}
