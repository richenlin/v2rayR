import Foundation
import Network

enum ProxyError: Error {
    case configuration, unsupportedEndpoint, invalidReply, authentication, rejected, closed, limit
}

struct SocksAddress: Hashable {
    let host: NWEndpoint.Host
    let port: NWEndpoint.Port

    init(_ endpoint: NWEndpoint) throws {
        guard case let .hostPort(host, port) = endpoint else { throw ProxyError.unsupportedEndpoint }
        self.host = host
        self.port = port
    }

    var endpoint: NWEndpoint { .hostPort(host: host, port: port) }

    var isLoopback: Bool {
        switch host {
        case .ipv4(let ip): return ip.rawValue.first == 127
        case .ipv6(let ip):
            let bytes = Array(ip.rawValue)
            return ip == .loopback || (bytes.prefix(10).allSatisfy { $0 == 0 } &&
                bytes[10] == 255 && bytes[11] == 255 && bytes[12] == 127)
        case .name(let name, _): return name.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: ".")) == "localhost"
        @unknown default: return false
        }
    }

    func encode() throws -> Data {
        var bytes = Data()
        switch host {
        case .ipv4(let ip): bytes = Data([1]) + ip.rawValue
        case .ipv6(let ip): bytes = Data([4]) + ip.rawValue
        case .name(let name, _):
            let value = Data(name.utf8)
            guard (1...255).contains(value.count) else { throw ProxyError.unsupportedEndpoint }
            bytes = Data([3, UInt8(value.count)]) + value
        @unknown default: throw ProxyError.unsupportedEndpoint
        }
        bytes.append(UInt8(port.rawValue >> 8))
        bytes.append(UInt8(port.rawValue & 255))
        return bytes
    }

    static func decode(_ data: Data) throws -> (SocksAddress, Int) {
        let bytes = Array(data)
        guard let type = bytes.first else { throw ProxyError.invalidReply }
        let start = type == 3 ? 2 : 1
        let length: Int
        switch type {
        case 1: length = 4
        case 4: length = 16
        case 3:
            guard bytes.count > 1, bytes[1] > 0 else { throw ProxyError.invalidReply }
            length = Int(bytes[1])
        default: throw ProxyError.invalidReply
        }
        let end = start + length
        guard bytes.count >= end + 2 else { throw ProxyError.invalidReply }
        let value = Data(bytes[start..<end])
        let host: NWEndpoint.Host
        switch type {
        case 1:
            guard let ip = IPv4Address(value) else { throw ProxyError.invalidReply }
            host = .ipv4(ip)
        case 4:
            guard let ip = IPv6Address(value) else { throw ProxyError.invalidReply }
            host = .ipv6(ip)
        default:
            guard let name = String(data: value, encoding: .utf8) else { throw ProxyError.invalidReply }
            host = .name(name, nil)
        }
        let port = NWEndpoint.Port(rawValue: UInt16(bytes[end]) << 8 | UInt16(bytes[end + 1]))!
        return (try SocksAddress(.hostPort(host: host, port: port)), end + 2)
    }
}

struct SocksClient {
    let port: NWEndpoint.Port
    let username: String
    let password: String

    static let queue = DispatchQueue(label: "v2rayr.application-proxy.network")
    static let local = NWEndpoint.Host.ipv4(.loopback)

    func connection() -> NWConnection {
        NWConnection(host: Self.local, port: port, using: .tcp)
    }

    func authenticate(_ connection: NWConnection) async throws {
        connection.start(queue: Self.queue)
        let username = Data(username.utf8)
        let password = Data(password.utf8)
        guard (1...255).contains(username.count), (1...255).contains(password.count) else {
            throw ProxyError.configuration
        }
        try await send(connection, Data([5, 1, 2]))
        guard try await exact(connection, 2) == Data([5, 2]) else { throw ProxyError.authentication }
        try await send(connection, Data([1, UInt8(username.count)]) + username + Data([UInt8(password.count)]) + password)
        guard try await exact(connection, 2) == Data([1, 0]) else { throw ProxyError.authentication }
    }

    func request(_ command: UInt8, to target: SocksAddress, over connection: NWConnection) async throws -> SocksAddress {
        try await send(connection, Data([5, command, 0]) + target.encode())
        let reply = Array(try await exact(connection, 4))
        guard reply[0] == 5, reply[2] == 0 else { throw ProxyError.invalidReply }
        guard reply[1] == 0 else { throw ProxyError.rejected }
        var address = Data([reply[3]])
        switch reply[3] {
        case 1: address += try await exact(connection, 6)
        case 4: address += try await exact(connection, 18)
        case 3:
            let size = try await exact(connection, 1)
            address += size
            address += try await exact(connection, Int(size.first!) + 2)
        default: throw ProxyError.invalidReply
        }
        return try SocksAddress.decode(address).0
    }

    func associate(_ control: NWConnection) async throws -> NWConnection {
        try await authenticate(control)
        let unspecified = try SocksAddress(.hostPort(host: "0.0.0.0", port: .any))
        let relay = try await request(3, to: unspecified, over: control)
        guard relay.port != .any, relay.host == Self.local || relay.host == unspecified.host else {
            throw ProxyError.invalidReply
        }
        let udp = NWConnection(host: Self.local, port: relay.port, using: .udp)
        udp.start(queue: Self.queue)
        return udp
    }
}

func send(_ connection: NWConnection, _ data: Data?, complete: Bool = false) async throws {
    try await withCheckedThrowingContinuation { (result: CheckedContinuation<Void, Error>) in
        connection.send(content: data, contentContext: .defaultMessage, isComplete: complete,
                        completion: .contentProcessed { error in
            if let error { result.resume(throwing: error) } else { result.resume() }
        })
    }
}

func receive(_ connection: NWConnection, min: Int = 1, max: Int = 65536) async throws -> Data? {
    try await withCheckedThrowingContinuation { result in
        connection.receive(minimumIncompleteLength: min, maximumLength: max) { data, _, _, error in
            if let error { result.resume(throwing: error) } else { result.resume(returning: data) }
        }
    }
}

func exact(_ connection: NWConnection, _ size: Int) async throws -> Data {
    guard let data = try await receive(connection, min: size, max: size), data.count == size else {
        throw ProxyError.closed
    }
    return data
}

func receiveDatagram(_ connection: NWConnection) async throws -> Data {
    try await withCheckedThrowingContinuation { result in
        connection.receiveMessage { data, _, _, error in
            if let error { result.resume(throwing: error) }
            else if let data { result.resume(returning: data) }
            else { result.resume(throwing: ProxyError.closed) }
        }
    }
}

func udpPacket(_ payload: Data, to target: SocksAddress) throws -> Data {
    Data([0, 0, 0]) + (try target.encode()) + payload
}

func udpPayload(_ packet: Data) throws -> (SocksAddress, Data) {
    guard packet.prefix(3) == Data([0, 0, 0]) else { throw ProxyError.invalidReply }
    let (sender, size) = try SocksAddress.decode(Data(packet.dropFirst(3)))
    return (sender, Data(packet.dropFirst(3 + size)))
}
