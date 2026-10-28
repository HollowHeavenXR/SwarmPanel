import Foundation

@MainActor
final class LumisoundAdminViewModel: ObservableObject {
    @Published var users: [LumisoundUser] = []
    @Published var uploads: [LumisoundUpload] = []
    @Published var bugReports: [LumisoundBugReport] = []
    @Published var isLoading = true
    @Published var errorMessage: String?

    private let core = Core.shared

    func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            let data = try await core.lumisoundAdmin().data
            users = data.users ?? []
            uploads = data.uploads ?? []
            bugReports = data.bugReports ?? []
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load Lumisound admin data.")
        }
    }

    func setUserActive(_ user: LumisoundUser, active: Bool) async {
        await act("Failed to update user.") { _ = try await self.core.setLumisoundUserActive(userId: user.id, active: active) }
    }

    func deleteUpload(_ upload: LumisoundUpload) async {
        await act("Failed to delete upload.") { _ = try await self.core.deleteLumisoundUpload(uploadId: upload.id) }
    }

    func setBugReportStatus(_ report: LumisoundBugReport, status: String) async {
        await act("Failed to update bug report.") { _ = try await self.core.setLumisoundBugStatus(reportId: report.id, status: status) }
    }

    private func act(_ fallback: String, _ work: () async throws -> Void) async {
        do {
            try await work()
            await load()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message(fallback)
        }
    }
}
