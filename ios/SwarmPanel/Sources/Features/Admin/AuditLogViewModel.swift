import Foundation

@MainActor
final class AuditLogViewModel: ObservableObject {
    @Published var entries: [AuditLogEntry] = []
    @Published var total = 0
    @Published var actionFilter = ""
    @Published var isLoading = true
    @Published var errorMessage: String?
    @Published var statusMessage: String?
    @Published var revertingId: Int?

    private let core = Core.shared

    func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            let data = try await core.auditLog(limit: 100, action: actionFilter).data
            entries = data.entries ?? []
            total = data.total ?? entries.count
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load audit log.")
        }
    }

    func revert(_ entry: AuditLogEntry) async {
        revertingId = entry.id
        defer { revertingId = nil }
        do {
            _ = try await core.revertAuditEntry(entryId: entry.id)
            Haptics.success()
            statusMessage = "Reverted \(entry.action)."
            await load()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to revert this entry.")
            Haptics.error()
        }
    }
}
