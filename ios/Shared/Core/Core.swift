import Foundation
import Security
#if canImport(UIKit)
import UIKit
#endif

/// The app's Rust core (rust/core, module SwarmPanelCore): networking,
/// session, live socket, telemetry, models and app logic. Shared by the iOS
/// app, its widget extension and the Apple TV app. Swift only provides
/// storage (below) and the UI.
enum Core {
    static let appGroupID = "group.com.swarmpanel.ios"

    #if os(tvOS)
    static let clientName = "tvos"
    #else
    static let clientName = "ios"
    #endif

    static let shared: SwarmCore = SwarmCore(
        config: CoreConfig(client: clientName, appVersion: appVersion, osVersion: osVersion, deviceModel: deviceModel),
        store: AppleStore()
    )

    static var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
    }

    static var osVersion: String {
        #if canImport(UIKit)
        UIDevice.current.systemVersion
        #else
        ProcessInfo.processInfo.operatingSystemVersionString
        #endif
    }

    static var deviceModel: String {
        var info = utsname()
        uname(&info)
        let id = Mirror(reflecting: info.machine).children.reduce(into: "") { result, element in
            if let value = element.value as? Int8, value != 0 { result.append(Character(UnicodeScalar(UInt8(value)))) }
        }
        #if canImport(UIKit)
        return id.isEmpty ? UIDevice.current.model : id
        #else
        return id
        #endif
    }
}

/// Where the core keeps things: the bearer token in the Keychain (same item
/// the app always used, so existing installs stay signed in), everything
/// else in UserDefaults (standard, or the App Group suite when `shared`).
final class AppleStore: PlatformStore {
    private static let service = "com.swarmpanel.ios.token"
    private static let account = "bearer-token"

    private var baseQuery: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: Self.service, kSecAttrAccount as String: Self.account]
    }

    func loadToken() -> String? {
        var query = baseQuery
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let data = result as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    func saveToken(token: String?) {
        SecItemDelete(baseQuery as CFDictionary)
        guard let token, !token.isEmpty else { return }
        var attributes = baseQuery
        attributes[kSecValueData as String] = Data(token.utf8)
        attributes[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        SecItemAdd(attributes as CFDictionary, nil)
    }

    private func defaults(_ shared: Bool) -> UserDefaults? {
        shared ? UserDefaults(suiteName: Core.appGroupID) : .standard
    }

    func get(key: String, shared: Bool) -> String? {
        defaults(shared)?.string(forKey: key)
    }

    func set(key: String, value: String?, shared: Bool) {
        if let value { defaults(shared)?.set(value, forKey: key) } else { defaults(shared)?.removeObject(forKey: key) }
    }
}
