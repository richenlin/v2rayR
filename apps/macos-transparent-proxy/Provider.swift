import CryptoKit
import Foundation
import Network
import NetworkExtension
import Security

struct ProviderConfiguration: Decodable {
    struct Application: Decodable {
        struct Identity: Decodable {
            let path: String
            let sha256: String
        }
        let identity: Identity
        let username: String
    }

    let port: UInt16
    let password: String
    let applications: [Application]
    let bypassExecutables: [String]

    func client(for token: Data?) -> SocksClient? {
        // An unresolved audit identity cannot be distinguished from the kernel
        // itself. Let the OS handle that flow instead of risking a proxy loop.
        guard let token, let code = AuditExecutable(token),
              !bypassExecutables.contains(code.path),
              let port = Network.NWEndpoint.Port(rawValue: port) else { return nil }
        let candidates = applications.filter { $0.identity.path == code.path }
        var username = "unknown"
        if !candidates.isEmpty, let digest = code.verifiedDigest(),
           let match = candidates.first(where: { $0.identity.sha256 == digest }) {
            username = match.username
        }
        return SocksClient(port: port, username: username, password: password)
    }
}

struct AuditExecutable {
    let code: SecCode
    let staticCode: SecStaticCode
    let path: String

    init?(_ token: Data) {
        let attributes = [kSecGuestAttributeAudit as String: token] as CFDictionary
        var code: SecCode?
        var staticCode: SecStaticCode?
        var information: CFDictionary?
        guard SecCodeCopyGuestWithAttributes(nil, attributes, [], &code) == errSecSuccess,
              let code, SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess,
              let staticCode,
              SecCodeCopySigningInformation(staticCode, [], &information) == errSecSuccess,
              let fields = information as? [String: Any],
              let executable = fields[kSecCodeInfoMainExecutable as String] as? URL else { return nil }
        self.code = code
        self.staticCode = staticCode
        self.path = executable.resolvingSymlinksInPath().path
    }

    func verifiedDigest() -> String? {
        // Verify both the live guest and the on-disk code against the exact
        // running code requirement. A replaced executable cannot inherit a rule.
        var information: CFDictionary?
        var requirement: SecRequirement?
        guard SecCodeCopySigningInformation(staticCode, SecCSFlags(rawValue: kSecCSSigningInformation), &information) == errSecSuccess,
              let fields = information as? [String: Any],
              let cdhash = fields[kSecCodeInfoUnique as String] as? Data,
              cdhash.count == 20 else { return nil }
        let hex = cdhash.map { String(format: "%02x", $0) }.joined()
        guard SecRequirementCreateWithString("cdhash H\"\(hex)\"" as CFString, [], &requirement) == errSecSuccess,
              let requirement,
              SecCodeCheckValidity(code, [], requirement) == errSecSuccess,
              SecStaticCodeCheckValidity(staticCode, [], requirement) == errSecSuccess,
              let bytes = try? Data(contentsOf: URL(fileURLWithPath: path), options: .mappedIfSafe) else { return nil }
        let digest = SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
        guard SecStaticCodeCheckValidity(staticCode, [], requirement) == errSecSuccess else { return nil }
        return digest
    }
}

@available(macOS 15.0, *)
final class TransparentProxyProvider: NETransparentProxyProvider, NEAppProxyUDPFlowHandling {
    private let lock = NSLock()
    private var configuration: ProviderConfiguration?
    private var sessions: [UUID: Task<Void, Never>] = [:]

    override func startProxy(options: [String: Any]? = nil) async throws {
        guard let bytes = options?["applicationIngress"] as? Data else {
            throw ProxyError.configuration
        }
        let config = try JSONDecoder().decode(ProviderConfiguration.self, from: bytes)
        guard config.port >= 1024, (24...255).contains(config.password.utf8.count),
              !config.bypassExecutables.isEmpty else { throw ProxyError.configuration }
        let settings = NETransparentProxyNetworkSettings(tunnelRemoteAddress: "127.0.0.1")
        settings.includedNetworkRules = [
            NENetworkRule(remoteNetworkEndpoint: nil, remotePrefix: 0,
                          localNetworkEndpoint: nil, localPrefix: 0,
                          protocol: .any, direction: .outbound),
        ]
        settings.excludedNetworkRules = [
            NENetworkRule(destinationNetworkEndpoint: .hostPort(host: "127.0.0.0", port: .any), prefix: 8, protocol: .any),
            NENetworkRule(destinationNetworkEndpoint: .hostPort(host: "::1", port: .any), prefix: 128, protocol: .any),
        ]
        lock.withLock { configuration = config }
        do {
            try await setTunnelNetworkSettings(settings)
        } catch {
            lock.withLock { configuration = nil }
            throw error
        }
    }

    override func stopProxy(with reason: NEProviderStopReason) async {
        let tasks = lock.withLock {
            configuration = nil
            let tasks = Array(sessions.values)
            sessions.removeAll()
            return tasks
        }
        tasks.forEach { $0.cancel() }
        for task in tasks { await task.value }
    }

    override func handleNewFlow(_ flow: NEAppProxyFlow) -> Bool {
        guard let tcp = flow as? NEAppProxyTCPFlow,
              let target = try? SocksAddress(tcp.remoteFlowEndpoint), !target.isLoopback,
              let client = client(for: flow) else { return false }
        return track(flow) { try await relayTCP(tcp, to: target, via: client) }
    }

    func handleNewUDPFlow(_ flow: NEAppProxyUDPFlow, initialRemoteFlowEndpoint: Network.NWEndpoint) -> Bool {
        guard let target = try? SocksAddress(initialRemoteFlowEndpoint), !target.isLoopback,
              let client = client(for: flow) else { return false }
        return track(flow) { try await relayUDP(flow, via: client) }
    }

    private func client(for flow: NEAppProxyFlow) -> SocksClient? {
        lock.withLock { configuration }?.client(for: flow.metaData.sourceAppAuditToken)
    }

    private func track(_ flow: NEAppProxyFlow, _ operation: @escaping () async throws -> Void) -> Bool {
        lock.withLock {
            guard configuration != nil else { return false }
            guard sessions.count < 256 else {
                flow.closeReadWithError(ProxyError.limit)
                flow.closeWriteWithError(ProxyError.limit)
                return true
            }
            let id = UUID()
            sessions[id] = Task { [weak self] in
                defer {
                    if let self { _ = self.lock.withLock { self.sessions.removeValue(forKey: id) } }
                }
                try? await operation()
            }
            return true
        }
    }
}
