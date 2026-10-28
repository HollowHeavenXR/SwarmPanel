import Foundation

/// SwiftUI-facing wrapper around the core's live socket (/ws). The socket
/// itself (auth, watches, reconnects, watchdog, typed decoding) is in Rust;
/// this only republishes connection state and hops callbacks to the main actor.
@MainActor
final class SwarmLiveSocket: ObservableObject {
    static let shared = SwarmLiveSocket()

    @Published private(set) var isConnected = false

    private let live = Core.shared.live()
    private var handlers: [String: (Result<LiveSnapshot, Error>) -> Void] = [:]
    private lazy var bridge = Bridge(owner: self)

    private init() {
        live.addConnectionObserver(observer: bridge)
    }

    func connect() { live.connect() }
    func disconnect() { live.disconnect() }

    func watch(_ key: String, params: [String: String]? = nil, handler: @escaping (Result<LiveSnapshot, Error>) -> Void) {
        handlers[key] = handler
        live.watch(key: key, params: params, listener: bridge)
    }

    /// Typed watch: `watch("dashboard", as: DashboardResponse.self) { result in ... }`.
    func watch<T>(_ key: String, params: [String: String]? = nil, as type: T.Type, onUpdate: @escaping (Result<T, Error>) -> Void) {
        watch(key, params: params) { result in
            switch result {
            case .success(let snapshot):
                if let value = snapshot.payload as? T {
                    onUpdate(.success(value))
                } else {
                    onUpdate(.failure(SwarmLiveError.snapshotError("Unexpected live update for \(key).")))
                }
            case .failure(let error):
                onUpdate(.failure(error))
            }
        }
    }

    func resubscribe(_ key: String, params: [String: String]? = nil) {
        live.resubscribe(key: key, params: params)
    }

    func unwatch(_ key: String) {
        handlers.removeValue(forKey: key)
        live.unwatch(key: key)
    }

    fileprivate func deliver(_ key: String, _ result: Result<LiveSnapshot, Error>) {
        handlers[key]?(result)
    }

    fileprivate func setConnected(_ connected: Bool) {
        if isConnected != connected { isConnected = connected }
    }

    private final class Bridge: LiveListener, LiveConnectionObserver {
        weak var owner: SwarmLiveSocket?
        init(owner: SwarmLiveSocket) { self.owner = owner }

        func onSnapshot(key: String, snapshot: LiveSnapshot) {
            Task { @MainActor [weak owner] in owner?.deliver(key, .success(snapshot)) }
        }

        func onError(key: String, message: String) {
            Task { @MainActor [weak owner] in owner?.deliver(key, .failure(SwarmLiveError.snapshotError(message))) }
        }

        func connectionChanged(connected: Bool) {
            Task { @MainActor [weak owner] in owner?.setConnected(connected) }
        }
    }
}

enum SwarmLiveError: LocalizedError {
    case snapshotError(String)
    var errorDescription: String? {
        switch self {
        case .snapshotError(let message): return message
        }
    }
}
