import Foundation
import Network
import NetworkExtension

@available(macOS 15.0, *)
func relayTCP(_ flow: NEAppProxyTCPFlow, to target: SocksAddress, via client: SocksClient) async throws {
    let connection = client.connection()
    try await withTaskCancellationHandler {
        do {
            try await handshake(connection) {
                try await client.authenticate(connection)
                _ = try await client.request(1, to: target, over: connection)
            }
            try await open(flow)
            try await withThrowingTaskGroup(of: Void.self) { group in
                group.addTask {
                    while let data = try await readTCP(flow), !data.isEmpty {
                        try await send(connection, data)
                    }
                    connection.send(content: nil, contentContext: .finalMessage, isComplete: true, completion: .idempotent)
                    flow.closeReadWithError(nil)
                }
                group.addTask {
                    while let data = try await receive(connection), !data.isEmpty {
                        try await writeTCP(flow, data)
                    }
                    flow.closeWriteWithError(nil)
                }
                do {
                    while try await group.next() != nil {}
                } catch {
                    close(flow, error)
                    connection.cancel()
                    throw error
                }
            }
        } catch {
            close(flow, error)
            connection.cancel()
            throw error
        }
        connection.cancel()
    } onCancel: {
        close(flow, CancellationError())
        connection.cancel()
    }
}

private struct UDPReply {
    let data: Data
    let endpoint: Network.NWEndpoint
}

@available(macOS 15.0, *)
func relayUDP(_ flow: NEAppProxyUDPFlow, via client: SocksClient) async throws {
    let (replies, continuation) = AsyncStream<UDPReply>.makeStream(bufferingPolicy: .bufferingNewest(128))
    try await withTaskCancellationHandler {
        do {
            try await open(flow)
            try await withThrowingTaskGroup(of: Void.self) { group in
                group.addTask {
                    for await reply in replies {
                        try await writeUDP(flow, reply.data, reply.endpoint)
                    }
                }
                group.addTask {
                    try await sendDatagrams(flow, via: client, replies: continuation)
                }
                do {
                    _ = try await group.next()
                } catch {
                    close(flow, error)
                    group.cancelAll()
                    throw error
                }
                close(flow, nil)
                group.cancelAll()
            }
        } catch {
            close(flow, error)
            throw error
        }
    } onCancel: {
        continuation.finish()
        close(flow, CancellationError())
    }
}

@available(macOS 15.0, *)
private func sendDatagrams(
    _ flow: NEAppProxyUDPFlow, via client: SocksClient, replies: AsyncStream<UDPReply>.Continuation
) async throws {
    // Separate associations let the kernel independently evaluate each
    // destination even when an app uses one unconnected UDP socket.
    try await withThrowingTaskGroup(of: Void.self) { group in
        var channels: [SocksAddress: NWConnection] = [:]
        defer {
            replies.finish()
            channels.values.forEach { $0.cancel() }
            group.cancelAll()
        }
        while !Task.isCancelled {
            let datagrams = try await readUDP(flow)
            if datagrams.isEmpty { return }
            for (data, endpoint) in datagrams {
                let target = try SocksAddress(endpoint)
                guard !target.isLoopback else { continue }
                let relay: NWConnection
                if let existing = channels[target] {
                    relay = existing
                } else {
                    guard channels.count < 16 else { throw ProxyError.limit }
                    let control = client.connection()
                    relay = try await withTaskCancellationHandler {
                        try await handshake(control) { try await client.associate(control) }
                    } onCancel: { control.cancel() }
                    channels[target] = relay
                    group.addTask {
                        try await withTaskCancellationHandler {
                            defer { control.cancel() }
                            _ = try await receive(control)
                            close(flow, ProxyError.closed)
                        } onCancel: { control.cancel() }
                    }
                    group.addTask {
                        do {
                            while !Task.isCancelled {
                                let (sender, payload) = try udpPayload(try await receiveDatagram(relay))
                                replies.yield(UDPReply(data: payload, endpoint: sender.endpoint))
                            }
                        } catch {
                            close(flow, error)
                            throw error
                        }
                    }
                }
                try await send(relay, udpPacket(data, to: target), complete: true)
            }
        }
    }
}

func handshake<T>(_ connection: NWConnection, _ operation: () async throws -> T) async throws -> T {
    let timeout = Task {
        try await Task.sleep(for: .seconds(8))
        connection.cancel()
    }
    defer { timeout.cancel() }
    do { return try await operation() }
    catch {
        connection.cancel()
        throw error
    }
}

private func close(_ flow: NEAppProxyFlow, _ error: Error?) {
    flow.closeReadWithError(error)
    flow.closeWriteWithError(error)
}

@available(macOS 15.0, *)
private func open(_ flow: NEAppProxyFlow) async throws {
    try await flow.open(withLocalFlowEndpoint: nil)
}

private func readTCP(_ flow: NEAppProxyTCPFlow) async throws -> Data? {
    try await withCheckedThrowingContinuation { result in
        flow.readData { data, error in
            if let error { result.resume(throwing: error) } else { result.resume(returning: data) }
        }
    }
}

private func writeTCP(_ flow: NEAppProxyTCPFlow, _ data: Data) async throws {
    try await flow.write(data)
}

@available(macOS 15.0, *)
private func readUDP(_ flow: NEAppProxyUDPFlow) async throws -> [(Data, Network.NWEndpoint)] {
    try await withCheckedThrowingContinuation { result in
        flow.readDatagrams { datagrams, error in
            if let error { result.resume(throwing: error) } else { result.resume(returning: datagrams ?? []) }
        }
    }
}

@available(macOS 15.0, *)
private func writeUDP(_ flow: NEAppProxyUDPFlow, _ data: Data, _ endpoint: Network.NWEndpoint) async throws {
    try await flow.writeDatagrams([(data, endpoint)])
}
