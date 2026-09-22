import Foundation
import MyproxyNetworkShared
import Darwin
import Dispatch
import CommonCrypto

#if MYPROXY_XRAY
final class AppAdmissionClient: @unchecked Sendable {
    static let maximumFrame = 32 * 1024
    private let bootstrap: AppAdmissionBootstrap
    private let key: Data
    var activation: String { bootstrap.activation }

    init?(data: Data?) {
        guard let data,
              data.count <= 2048,
              let bootstrap = try? JSONDecoder().decode(AppAdmissionBootstrap.self, from: data),
              bootstrap.version == AppAdmissionBootstrap.version,
              bootstrap.port > 0, UUID(uuidString: bootstrap.activation) != nil,
              let key = Data(base64Encoded: bootstrap.key), key.count == 32
        else { return nil }
        self.bootstrap = bootstrap
        self.key = key
    }

    func request(_ request: AppAdmissionRequest) -> Result<AppAdmissionReply, AppAdmissionProtocolError> {
        guard request.version == AppAdmissionBootstrap.version,
              request.activation == bootstrap.activation,
              !request.host.isEmpty, request.port > 0 else {
            return .failure(.invalidRequest)
        }
        do {
            let deadline = DispatchTime.now().uptimeNanoseconds + 1_000_000_000
            let payload = try JSONEncoder().encode(request)
            guard payload.count <= 16 * 1024 else { throw AppAdmissionProtocolError.frameTooLarge }
            let envelope = try Self.envelope(payload: payload, key: key)
            let fd = try connect(deadline: deadline)
            defer { close(fd) }
            try Self.writeFrame(envelope, fd: fd, deadline: deadline)
            let response = try Self.readFrame(fd: fd, deadline: deadline)
            let decoded = try Self.decodeEnvelope(response, key: key)
            let reply = try JSONDecoder().decode(AppAdmissionReply.self, from: decoded)
            guard reply.version == AppAdmissionBootstrap.version,
                  reply.activation == request.activation,
                  reply.nonce == request.nonce,
                  reply.host.count <= 253, reply.port > 0 || reply.action == "reject",
                  ["direct", "reject", "proxy"].contains(reply.action)
            else { throw AppAdmissionProtocolError.invalidReply }
            return .success(reply)
        } catch let error as AppAdmissionProtocolError {
            return .failure(error)
        } catch is DecodingError {
            return .failure(.invalidReply)
        } catch {
            return .failure(.unavailable)
        }
    }

    private func connect(deadline: UInt64) throws -> Int32 {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { throw AppAdmissionProtocolError.unavailable }
        var connected = false
        defer { if !connected { close(fd) } }
        var noSigPipe: Int32 = 1
        _ = setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &noSigPipe, socklen_t(MemoryLayout<Int32>.size))
        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = bootstrap.port.bigEndian
        address.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))
        let flags = fcntl(fd, F_GETFL, 0)
        _ = fcntl(fd, F_SETFL, flags | O_NONBLOCK)
        let result = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        if result < 0 && errno != EINPROGRESS { throw AppAdmissionProtocolError.unavailable }
        if result < 0 { try Self.wait(fd: fd, events: Int16(POLLOUT), deadline: deadline) }
        var error = 0; var length = socklen_t(MemoryLayout<Int32>.size)
        guard getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &length) == 0, error == 0 else {
            throw AppAdmissionProtocolError.unavailable
        }
        connected = true
        return fd
    }

    private static func envelope(payload: Data, key: Data) throws -> Data {
        let mac = hmac(key: key, data: payload).map { String(format: "%02x", $0) }.joined()
        let object: [String: Any] = ["payload": String(decoding: payload, as: UTF8.self), "mac": mac]
        let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        guard data.count <= maximumFrame else { throw AppAdmissionProtocolError.frameTooLarge }
        return data
    }

    private static func decodeEnvelope(_ data: Data, key: Data) throws -> Data {
        guard data.count <= maximumFrame,
              let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let payload = object["payload"] as? String,
              let mac = object["mac"] as? String,
              let payloadData = payload.data(using: .utf8),
              constantTimeEqual(mac, hmac(key: key, data: payloadData).map({ String(format: "%02x", $0) }).joined())
        else { throw AppAdmissionProtocolError.authenticationFailed }
        guard payloadData.count <= 16 * 1024 else { throw AppAdmissionProtocolError.frameTooLarge }
        return payloadData
    }

    private static func constantTimeEqual(_ left: String, _ right: String) -> Bool {
        let a = Array(left.utf8), b = Array(right.utf8)
        guard a.count == b.count else { return false }
        return zip(a, b).reduce(UInt8(0)) { $0 | ($1.0 ^ $1.1) } == 0
    }

    private static func writeFrame(_ data: Data, fd: Int32, deadline: UInt64) throws {
        var size = UInt32(data.count).bigEndian
        var bytes = Data(bytes: &size, count: 4); bytes.append(data)
        try bytes.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                try wait(fd: fd, events: Int16(POLLOUT), deadline: deadline)
                let count = Darwin.write(fd, raw.baseAddress!.advanced(by: offset), raw.count - offset)
                if count <= 0 { throw errno == EAGAIN ? AppAdmissionProtocolError.timeout : .unavailable }
                offset += count
            }
        }
    }

    private static func readFrame(fd: Int32, deadline: UInt64) throws -> Data {
        var header = Data(count: 4); try readExact(&header, fd: fd, deadline: deadline)
        let size = header.withUnsafeBytes { UInt32(bigEndian: $0.loadUnaligned(as: UInt32.self)) }
        guard size > 0 && size <= maximumFrame else { throw AppAdmissionProtocolError.frameTooLarge }
        var body = Data(count: Int(size)); try readExact(&body, fd: fd, deadline: deadline); return body
    }

    private static func readExact(_ data: inout Data, fd: Int32, deadline: UInt64) throws {
        try data.withUnsafeMutableBytes { raw in
            var offset = 0
            while offset < raw.count {
                try wait(fd: fd, events: Int16(POLLIN), deadline: deadline)
                let count = Darwin.read(fd, raw.baseAddress!.advanced(by: offset), raw.count - offset)
                if count <= 0 { throw count == 0 ? AppAdmissionProtocolError.unavailable : .timeout }
                offset += count
            }
        }
    }

    private static func wait(fd: Int32, events: Int16, deadline: UInt64) throws {
        var pollfd = Darwin.pollfd(fd: fd, events: events, revents: 0)
        let now = DispatchTime.now().uptimeNanoseconds
        guard now < deadline else { throw AppAdmissionProtocolError.timeout }
        let remaining = (deadline - now + 999_999) / 1_000_000
        let result = Darwin.poll(&pollfd, 1, Int32(min(remaining, 1000)))
        guard result > 0 else { throw result == 0 ? AppAdmissionProtocolError.timeout : .unavailable }
        guard pollfd.revents & Int16(POLLERR | POLLNVAL) == 0 else { throw AppAdmissionProtocolError.unavailable }
    }

    private static func hmac(key: Data, data: Data) -> Data {
        var output = Data(count: 32)
        output.withUnsafeMutableBytes { out in
            key.withUnsafeBytes { keyRaw in
                data.withUnsafeBytes { dataRaw in
                    CCHmac(CCHmacAlgorithm(kCCHmacAlgSHA256), keyRaw.baseAddress, key.count, dataRaw.baseAddress, data.count, out.baseAddress)
                }
            }
        }
        return output
    }
}
#endif
