import Foundation
import UIKit
import WidgetKit

/// Writes the account's own-guild Now Playing state + fleet totals to the
/// shared App Group so SwarmPanelWidget can read them, then triggers a
/// widget timeline reload. Called from DashboardView whenever
/// DashboardViewModel's response updates.
@MainActor
final class WidgetDataService {
    static let shared = WidgetDataService()

    private var defaults: UserDefaults? { UserDefaults(suiteName: WidgetShared.appGroupID) }
    private var lastReloadTime: Date = .distantPast
    private var lastThumbnailURL: String?

    private init() {}

    /// Unlike `UserDefaults(suiteName:)` (which returns a usable but
    /// sandbox-local object even without the entitlement), this reliably
    /// returns `nil` when the App Group isn't actually provisioned for this
    /// build — most relevant on a sideloaded build whose resigning
    /// certificate/provisioning profile doesn't include the App Group.
    var isAppGroupAvailable: Bool {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: WidgetShared.appGroupID) != nil
    }

    /// Publishes the widget/Live Activity state. What to show is computed
    /// by the core (widgetSnapshot); this writes it where WidgetKit reads it.
    func update(dashboard: DashboardResponse, guildId: String?) {
        guard let ud = defaults else { return }
        let w = widgetSnapshot(dashboard: dashboard, guildId: guildId)
        ud.set(w.botsCount, forKey: WidgetShared.Keys.botsCount)
        ud.set(w.liveCount, forKey: WidgetShared.Keys.liveCount)
        ud.set(w.queuedCount, forKey: WidgetShared.Keys.queuedCount)
        ud.set(w.title, forKey: WidgetShared.Keys.title)
        ud.set(w.subtitle, forKey: WidgetShared.Keys.subtitle)
        ud.set(w.isPlaying, forKey: WidgetShared.Keys.isPlaying)
        ud.set(w.isPaused, forKey: WidgetShared.Keys.isPaused)
        ud.set(w.position, forKey: WidgetShared.Keys.position)
        ud.set(w.duration, forKey: WidgetShared.Keys.duration)
        ud.set(Date().timeIntervalSinceReferenceDate, forKey: WidgetShared.Keys.anchorDate)

        reloadTimelinesThrottled()

        if #available(iOS 16.1, *) {
            LiveActivityManager.update(
                title: w.title, subtitle: w.subtitle, isPlaying: w.isPlaying, isPaused: w.isPaused,
                position: w.position, duration: w.duration
            )
        }

        Task { await downloadThumbnailIfNeeded(w.thumbnail) }
    }

    private func downloadThumbnailIfNeeded(_ urlString: String?) async {
        guard let urlString, !urlString.isEmpty else {
            clearThumbnail()
            return
        }
        guard urlString != lastThumbnailURL, let url = URL(string: urlString) else { return }
        guard let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: WidgetShared.appGroupID) else { return }
        do {
            let (data, _) = try await URLSession.shared.data(from: url)
            guard let image = UIImage(data: data), let resized = Self.widgetSizedJPEG(image) else { return }
            let path = container.appendingPathComponent(WidgetShared.thumbnailFilename)
            try resized.write(to: path, options: .atomic)
            defaults?.set(WidgetShared.thumbnailFilename, forKey: WidgetShared.Keys.thumbnailPath)
            lastThumbnailURL = urlString
            reloadTimelinesThrottled()
        } catch {
            // Best-effort — the widget just shows a placeholder icon instead.
        }
    }

    private func clearThumbnail() {
        guard lastThumbnailURL != nil else { return }
        defaults?.removeObject(forKey: WidgetShared.Keys.thumbnailPath)
        if let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: WidgetShared.appGroupID) {
            try? FileManager.default.removeItem(at: container.appendingPathComponent(WidgetShared.thumbnailFilename))
        }
        lastThumbnailURL = nil
    }

    /// Widget extensions run under a tight memory cap and decode this file
    /// with `UIImage(contentsOfFile:)` directly — handing them a full-size
    /// YouTube thumbnail risks the widget process getting killed mid-render.
    private static func widgetSizedJPEG(_ image: UIImage, maxPixelSize: CGFloat = 300) -> Data? {
        let scale = min(1, maxPixelSize / max(image.size.width, image.size.height))
        guard scale < 1 else { return image.jpegData(compressionQuality: 0.85) }
        let newSize = CGSize(width: image.size.width * scale, height: image.size.height * scale)
        let renderer = UIGraphicsImageRenderer(size: newSize)
        let resized = renderer.image { _ in image.draw(in: CGRect(origin: .zero, size: newSize)) }
        return resized.jpegData(compressionQuality: 0.85)
    }

    /// Calls `WidgetCenter.shared.reloadAllTimelines()` at most once every 2
    /// seconds — avoids redundant WidgetKit refreshes when several fields
    /// change in quick succession (e.g. a fresh dashboard snapshot).
    private func reloadTimelinesThrottled() {
        guard Date().timeIntervalSince(lastReloadTime) >= 2.0 else { return }
        lastReloadTime = Date()
        WidgetCenter.shared.reloadAllTimelines()
    }
}
