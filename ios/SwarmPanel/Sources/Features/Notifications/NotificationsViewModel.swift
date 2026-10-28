import Foundation
import UserNotifications

/// Owned once by AppShellView so the unread badge stays live regardless of
/// which tab is currently selected — mirrors the web bell's always-polling
/// behavior (frontend/src/components/Shell.jsx's NotificationsBell).
@MainActor
final class NotificationsViewModel: ObservableObject {
    @Published var notifications: [PanelNotification] = []
    @Published var unreadCount = 0 {
        didSet { updateAppIconBadge() }
    }
    @Published var isLoading = false
    @Published var errorMessage: String?
    /// Community tab badge: unread direct messages + pending incoming
    /// friend requests (the "community_counts" live key, same counts the
    /// web panel shows on its Community nav).
    @Published var unreadMessages = 0
    @Published var pendingFriendRequests = 0
    var communityBadge: Int { unreadMessages + pendingFriendRequests }

    private let core = Core.shared
    private let socket = SwarmLiveSocket.shared
    private var pollTask: Task<Void, Never>?
    private var watching = false

    func startPolling() {
        guard !watching else { return }
        watching = true
        socket.watch("notifications", as: NotificationsSnapshot.self) { [weak self] result in
            guard let self, case .success(let snapshot) = result else { return }
            self.unreadCount = snapshot.unreadCount ?? 0
            self.notifications = snapshot.notifications ?? self.notifications
        }
        socket.watch("community_counts", as: CommunityCounts.self) { [weak self] result in
            guard let self, case .success(let counts) = result else { return }
            self.applyCommunityCounts(counts)
        }
        socket.connect()

        // Fallback poll only kicks in while the socket is actually down --
        // same pattern as DashboardViewModel -- so the badge still updates
        // if the connection drops and hasn't reconnected yet.
        pollTask = Task { [weak self] in
            guard let self else { return }
            await self.refreshUnreadCount()
            await self.refreshCommunityCounts()
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(20))
                if Task.isCancelled { break }
                if !self.socket.isConnected {
                    await self.refreshUnreadCount()
                    await self.refreshCommunityCounts()
                }
            }
        }
    }

    func stopPolling() {
        pollTask?.cancel()
        pollTask = nil
        watching = false
        socket.unwatch("notifications")
        socket.unwatch("community_counts")
    }

    func refreshCommunityCounts() async {
        do {
            let counts = try await core.communityCounts()
            applyCommunityCounts(counts)
        } catch {
            // Silent, like the unread-count poll above -- a badge is non-critical.
        }
    }

    private func applyCommunityCounts(_ counts: CommunityCounts) {
        unreadMessages = counts.unreadMessages ?? 0
        pendingFriendRequests = counts.pendingFriendRequests ?? 0
    }

    func refreshUnreadCount() async {
        do {
            let response = try await core.unreadCount()
            unreadCount = response.unreadCount ?? 0
        } catch {
            // Silent — notifications are non-critical, don't surface a toast for a failed poll.
        }
    }

    func loadNotifications() async {
        isLoading = true
        defer { isLoading = false }
        do {
            let response = try await core.notifications()
            notifications = response.notifications ?? []
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load notifications.")
        }
    }

    func markRead(_ notification: PanelNotification) async {
        guard notification.isUnread else { return }
        do {
            _ = try await core.markNotificationRead(notificationId: notification.id)
            unreadCount = max(0, unreadCount - 1)
            if let index = notifications.firstIndex(where: { $0.id == notification.id }) {
                notifications[index] = PanelNotification(
                    id: notification.id, kind: notification.kind, title: notification.title,
                    body: notification.body, linkPath: notification.linkPath,
                    readAt: "1970-01-01T00:00:00Z", createdAt: notification.createdAt
                )
            }
        } catch {
            // Best-effort — next poll/reload reconciles state.
        }
    }

    func markAllRead() async {
        do {
            _ = try await core.markAllNotificationsRead()
            unreadCount = 0
            await loadNotifications()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to mark all read.")
        }
    }

    /// Best-effort — if badge authorization was never granted (or the user
    /// declined it), this just silently doesn't update the home screen badge;
    /// it never affects in-app behavior either way.
    private func updateAppIconBadge() {
        let count = unreadCount
        Task {
            try? await UNUserNotificationCenter.current().setBadgeCount(count)
        }
    }
}
