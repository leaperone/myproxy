import Darwin
import Foundation
import MyproxyNetworkShared
import Security

/// Resolves `NEFlowMetaData.sourceAppAuditToken` without consulting a reusable PID.
/// All process and code-signing lookups remain tied to the original audit token.
public struct ProcessIdentityResolver: Sendable {
    public init() {}

    /// Returns `.unavailable` rather than a partially trusted identity. A transparent
    /// proxy provider must respond to that case by bypassing the flow (`return false`).
    public func resolve(sourceAppAuditToken data: Data) -> ProcessIdentityResolution {
        do {
            return .resolved(try resolveOrThrow(sourceAppAuditToken: data))
        } catch let failure as ProcessIdentityResolutionFailure {
            return .unavailable(failure)
        } catch {
            // Every operation below maps to a typed failure. This is a defensive
            // fallback that preserves fail-open behavior if that invariant changes.
            return .unavailable(.malformedSigningInformation)
        }
    }

    public func resolveOrThrow(sourceAppAuditToken data: Data) throws -> ResolvedProcessIdentity {
        let sdkTokenSize = MemoryLayout<audit_token_t>.size
        guard sdkTokenSize == SourceAppAuditToken.byteCount else {
            throw ProcessIdentityResolutionFailure.unsupportedAuditTokenLayout(
                expected: SourceAppAuditToken.byteCount,
                actual: sdkTokenSize
            )
        }
        let tokenValue = try SourceAppAuditToken(data)
        var auditToken = audit_token_t()
        _ = withUnsafeMutableBytes(of: &auditToken) { destination in
            data.copyBytes(to: destination)
        }

        let pid = audit_token_to_pid(auditToken)
        guard pid > 0 else {
            throw ProcessIdentityResolutionFailure.invalidProcessIdentifier(pid)
        }
        let effectiveUserID = audit_token_to_euid(auditToken)
        let auditUserID = audit_token_to_auid(auditToken)
        let processVersion = audit_token_to_pidversion(auditToken)
        let processStartTime = processStartTime(for: pid)
        let executablePath = try executablePath(for: &auditToken)
        let codeSigning = try codeSigningIdentity(auditTokenData: data)

        return ResolvedProcessIdentity(
            auditToken: tokenValue,
            processIdentifier: pid,
            processVersion: processVersion,
            processStartTime: processStartTime,
            effectiveUserID: effectiveUserID,
            auditUserID: auditUserID,
            executablePath: executablePath,
            codeSigning: codeSigning
        )
    }

    private func processStartTime(for processIdentifier: pid_t) -> ProcessStartTime? {
        var info = proc_bsdinfo()
        let expectedSize = MemoryLayout<proc_bsdinfo>.size
        let actualSize = withUnsafeMutablePointer(to: &info) { pointer in
            proc_pidinfo(
                processIdentifier,
                PROC_PIDTBSDINFO,
                0,
                pointer,
                Int32(expectedSize)
            )
        }
        guard actualSize == expectedSize else { return nil }
        return try? ProcessStartTime(
            seconds: info.pbi_start_tvsec,
            microseconds: UInt32(info.pbi_start_tvusec)
        )
    }

    private func executablePath(for auditToken: inout audit_token_t) throws -> String {
        var buffer = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
        let byteCount = buffer.withUnsafeMutableBufferPointer { pointer in
            proc_pidpath_audittoken(&auditToken, pointer.baseAddress, UInt32(pointer.count))
        }
        let capturedErrno = errno
        guard byteCount > 0 else {
            switch capturedErrno {
            case ESRCH:
                throw ProcessIdentityResolutionFailure.processNoLongerExists
            case EPERM, EACCES:
                throw ProcessIdentityResolutionFailure.executablePathPermissionDenied(errno: capturedErrno)
            default:
                throw ProcessIdentityResolutionFailure.executablePathUnavailable(errno: capturedErrno)
            }
        }

        let bytes = buffer.prefix(min(Int(byteCount), buffer.count))
            .prefix { $0 != 0 }
            .map { UInt8(bitPattern: $0) }
        let path = String(decoding: bytes, as: UTF8.self)
        guard !path.isEmpty else {
            throw ProcessIdentityResolutionFailure.emptyExecutablePath
        }
        return path
    }

    private func codeSigningIdentity(auditTokenData: Data) throws -> ProcessCodeSigningIdentity {
        let attributes = [kSecGuestAttributeAudit: auditTokenData as CFData] as CFDictionary
        var dynamicCode: SecCode?
        let lookupStatus = SecCodeCopyGuestWithAttributes(
            nil,
            attributes,
            SecCSFlags(),
            &dynamicCode
        )
        guard lookupStatus == errSecSuccess, let dynamicCode else {
            throw ProcessIdentityResolutionFailure.codeObjectLookupFailed(status: lookupStatus)
        }

        var staticCode: SecStaticCode?
        let staticStatus = SecCodeCopyStaticCode(dynamicCode, SecCSFlags(), &staticCode)
        guard staticStatus == errSecSuccess, let staticCode else {
            throw ProcessIdentityResolutionFailure.staticCodeLookupFailed(status: staticStatus)
        }

        // `SecCodeCheckValidity` validates a running (dynamic) code object and only
        // accepts its documented default flags. The strict/resource flags belong to
        // `SecStaticCodeCheckValidity`; passing them here returns errSecCSInvalidFlags
        // (-67070) for every flow and forces the provider to fail open.
        let validityStatus = SecCodeCheckValidity(dynamicCode, SecCSFlags(), nil)
        if validityStatus != errSecSuccess, validityStatus != errSecCSUnsigned {
            throw ProcessIdentityResolutionFailure.codeSignatureInvalid(status: validityStatus)
        }

        var signingInformation: CFDictionary?
        let signingFlags = SecCSFlags(
            rawValue: kSecCSSigningInformation | kSecCSRequirementInformation
        )
        let informationStatus = SecCodeCopySigningInformation(
            staticCode,
            signingFlags,
            &signingInformation
        )
        guard informationStatus == errSecSuccess, let signingInformation else {
            throw ProcessIdentityResolutionFailure.signingInformationFailed(status: informationStatus)
        }
        let information = signingInformation as NSDictionary

        guard let signingIdentifier = information[kSecCodeInfoIdentifier] as? String else {
            if validityStatus == errSecCSUnsigned {
                return .unsigned
            }
            throw ProcessIdentityResolutionFailure.malformedSigningInformation
        }

        var requirement: SecRequirement?
        let requirementStatus = SecCodeCopyDesignatedRequirement(
            staticCode,
            SecCSFlags(),
            &requirement
        )
        guard requirementStatus == errSecSuccess, let requirement else {
            throw ProcessIdentityResolutionFailure.designatedRequirementFailed(status: requirementStatus)
        }

        var requirementText: CFString?
        let requirementStringStatus = SecRequirementCopyString(
            requirement,
            SecCSFlags(),
            &requirementText
        )
        guard requirementStringStatus == errSecSuccess,
              let designatedRequirement = requirementText as String?,
              !designatedRequirement.isEmpty
        else {
            throw ProcessIdentityResolutionFailure.requirementStringFailed(status: requirementStringStatus)
        }

        let securedInfo = information[kSecCodeInfoPList] as? NSDictionary
        let securedBundleIdentifier = securedInfo?["CFBundleIdentifier"] as? String
        let mainExecutablePath = (information[kSecCodeInfoMainExecutable] as? URL)?.path
        let teamIdentifier = information[kSecCodeInfoTeamIdentifier] as? String
        let codeDirectoryHash = information[kSecCodeInfoUnique] as? Data
        let isApplePlatformCode = information[kSecCodeInfoPlatformIdentifier] != nil

        return .signed(SignedCodeIdentity(
            signingIdentifier: signingIdentifier,
            teamIdentifier: teamIdentifier,
            designatedRequirement: designatedRequirement,
            codeDirectoryHash: codeDirectoryHash,
            securedBundleIdentifier: securedBundleIdentifier,
            mainExecutablePath: mainExecutablePath,
            isApplePlatformCode: isApplePlatformCode
        ))
    }
}

/// Small FIFO cache for identities whose audit tokens were fully verified.
/// Audit tokens are process-instance identities (PID plus PID version), not
/// reusable bare PIDs. The fixed capacity keeps paths and signing metadata
/// bounded for the lifetime of the Network Extension.
final class ProcessIdentityResolutionCache: @unchecked Sendable {
    let capacity: Int

    // Permission and Security failures can repeat for every short-lived flow
    // from the same process. Keep the suppression short so a transient failure
    // can recover without turning the cache into a trust decision.
    private static let failureCacheTTLNanoseconds: UInt64 = 2_000_000_000

    private let lock = NSLock()
    private var identities: [SourceAppAuditToken: ResolvedProcessIdentity] = [:]
    private var insertionOrder: [SourceAppAuditToken] = []
    private struct FailureRecord {
        let resolution: ProcessIdentityResolution
        let expiresAt: UInt64
    }
    // Concurrent misses still resolve independently. This marker only keeps a
    // late failure from surviving a success in the same token wave.
    private struct ActiveResolution {
        var count: Int
        var observedSuccess: Bool
    }
    private var failures: [SourceAppAuditToken: FailureRecord] = [:]
    private var failureOrder: [SourceAppAuditToken] = []
    private var activeResolutions: [SourceAppAuditToken: ActiveResolution] = [:]

    init(capacity: Int) {
        self.capacity = max(0, capacity)
        identities.reserveCapacity(self.capacity)
        insertionOrder.reserveCapacity(self.capacity)
        failures.reserveCapacity(self.capacity)
        failureOrder.reserveCapacity(self.capacity)
    }

    func identity(for token: SourceAppAuditToken) -> ResolvedProcessIdentity? {
        withLock { identities[token] }
    }

    /// Code-signing inspection is one of the most expensive operations on the
    /// new-flow path. An audit token includes the PID version, so a successfully
    /// resolved identity can be reused safely without confusing a recycled PID.
    /// Failures are cached only briefly because permission and Security
    /// framework errors can be transient.
    func resolve(
        sourceAppAuditToken data: Data,
        using resolver: ProcessIdentityResolver
    ) -> ProcessIdentityResolution {
        resolve(
            sourceAppAuditToken: data,
            clock: { DispatchTime.now().uptimeNanoseconds },
            resolver: resolver.resolve(sourceAppAuditToken:)
        )
    }

    func resolve(
        sourceAppAuditToken data: Data,
        clock: () -> UInt64,
        resolver: (Data) -> ProcessIdentityResolution
    ) -> ProcessIdentityResolution {
        guard let token = try? SourceAppAuditToken(data) else {
            return resolver(data)
        }
        guard capacity > 0 else { return resolver(data) }
        if let cached = cachedResolutionOrBegin(
            for: token,
            clock: clock
        ) { return cached }

        let resolution = resolver(data)
        completeResolution(
            resolution,
            for: token,
            clock: clock
        )
        return resolution
    }

    func insert(_ identity: ResolvedProcessIdentity) {
        withLock {
            if var active = activeResolutions[identity.auditToken] {
                active.observedSuccess = true
                activeResolutions[identity.auditToken] = active
            }
            insertIdentityLocked(identity)
        }
    }

    private func cachedResolutionOrBegin(
        for token: SourceAppAuditToken,
        clock: () -> UInt64
    ) -> ProcessIdentityResolution? {
        withLock {
            if let identity = identities[token] {
                return .resolved(identity)
            }
            if let record = failures[token], record.expiresAt > clock() {
                return record.resolution
            }
            if failures[token] != nil {
                removeFailureLocked(for: token)
            }
            var active = activeResolutions[token] ?? ActiveResolution(
                count: 0,
                observedSuccess: false
            )
            active.count += 1
            activeResolutions[token] = active
            return nil
        }
    }

    private func completeResolution(
        _ resolution: ProcessIdentityResolution,
        for token: SourceAppAuditToken,
        clock: () -> UInt64
    ) {
        withLock {
            guard var active = activeResolutions[token] else { return }
            if case let .resolved(identity) = resolution {
                active.observedSuccess = true
                insertIdentityLocked(identity)
            } else if !active.observedSuccess {
                insertFailureLocked(
                    resolution,
                    for: token,
                    nowNanoseconds: clock()
                )
            }

            active.count -= 1
            if active.count == 0 {
                activeResolutions.removeValue(forKey: token)
            } else {
                activeResolutions[token] = active
            }
        }
    }

    private func insertIdentityLocked(_ identity: ResolvedProcessIdentity) {
        removeFailureLocked(for: identity.auditToken)
        guard capacity > 0,
              identities[identity.auditToken] == nil else { return }
        while insertionOrder.count >= capacity {
            identities.removeValue(forKey: insertionOrder.removeFirst())
        }
        identities[identity.auditToken] = identity
        insertionOrder.append(identity.auditToken)
    }

    private func insertFailureLocked(
        _ resolution: ProcessIdentityResolution,
        for token: SourceAppAuditToken,
        nowNanoseconds: UInt64
    ) {
        let (expiresAt, overflow) = nowNanoseconds.addingReportingOverflow(
            Self.failureCacheTTLNanoseconds
        )
        let record = FailureRecord(
            resolution: resolution,
            expiresAt: overflow ? .max : expiresAt
        )
        guard identities[token] == nil else { return }
        if failures[token] == nil {
            while failureOrder.count >= capacity {
                guard let oldest = failureOrder.first else { break }
                failureOrder.removeFirst()
                failures.removeValue(forKey: oldest)
            }
            failureOrder.append(token)
        }
        failures[token] = record
    }

    private func removeFailureLocked(for token: SourceAppAuditToken) {
        guard failures.removeValue(forKey: token) != nil else { return }
        if let index = failureOrder.firstIndex(of: token) {
            failureOrder.remove(at: index)
        }
    }

    private func withLock<T>(_ operation: () throws -> T) rethrows -> T {
        lock.lock()
        defer { lock.unlock() }
        return try operation()
    }
}
