import BackgroundTasks
import UserNotifications

/// Best-effort background notification delivery — the workaround for not
/// having a paid Apple Developer Program account, which real APNs push
/// requires. iOS decides if/when a BGAppRefreshTask actually runs based on
/// usage patterns and battery, with no guaranteed schedule, and it does NOT
/// run at all once the user force-quits the app from the app switcher —
/// only while it's backgrounded (not killed) does this help. Foreground
/// polling (NotificationsViewModel, every 20s) stays the reliable path;
/// this only extends coverage a little further, into the background.
enum BackgroundRefreshManager {
    static let refreshTaskIdentifier = "com.swarmpanel.ios.refresh"

    static func register() {
        BGTaskScheduler.shared.register(forTaskWithIdentifier: refreshTaskIdentifier, using: nil) { task in
            guard let refreshTask = task as? BGAppRefreshTask else { return }
            handle(task: refreshTask)
        }
    }

    static func scheduleNext() {
        let request = BGAppRefreshTaskRequest(identifier: refreshTaskIdentifier)
        request.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
        try? BGTaskScheduler.shared.submit(request)
    }

    private static func handle(task: BGAppRefreshTask) {
        scheduleNext()
        let work = Task {
            await checkForNewNotifications()
            task.setTaskCompleted(success: true)
        }
        task.expirationHandler = { work.cancel() }
    }

    /// The core returns unread notifications it hasn't surfaced before.
    private static func checkForNewNotifications() async {
        guard Core.shared.hasToken() else { return }
        guard let fresh = try? await Core.shared.freshNotifications() else { return }
        for notification in fresh {
            await deliver(notification)
        }
    }

    private static func deliver(_ notification: PanelNotification) async {
        let content = UNMutableNotificationContent()
        content.title = notification.title
        if let body = notification.body, !body.isEmpty { content.body = body }
        content.sound = .default
        let request = UNNotificationRequest(
            identifier: "swarmpanel.notification.\(notification.id)",
            content: content,
            trigger: nil
        )
        try? await UNUserNotificationCenter.current().add(request)
    }
}
