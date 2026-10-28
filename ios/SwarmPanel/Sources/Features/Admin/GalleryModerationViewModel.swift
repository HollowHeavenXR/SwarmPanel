import Foundation

@MainActor
final class GalleryModerationViewModel: ObservableObject {
    @Published var comments: [GalleryComment] = []
    @Published var reports: [GalleryReport] = []
    // Previously undecoded/unused entirely -- see the matching note on
    // GalleryAdminData. Media moderation and user management (verify,
    // reset password, delete, bulk actions) had no iOS UI at all before
    // this; the web panel's Gallery Admin has always had both.
    @Published var media: [GalleryMedia] = []
    @Published var users: [GalleryUser] = []
    @Published var isLoading = true
    @Published var errorMessage: String?
    @Published var statusMessage: String?
    @Published var selectedCommentIds: Set<Int> = []
    @Published var selectedMediaIds: Set<Int> = []
    @Published var selectedUserIds: Set<Int> = []
    @Published var isBulkDeleting = false
    @Published var isExporting = false

    private let core = Core.shared

    func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            let data = try await core.galleryAdmin().data
            comments = data.comments ?? []
            reports = data.reports ?? []
            media = data.media ?? []
            users = data.users ?? []
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load gallery admin data.")
        }
    }

    private func act(_ fallback: String, reload: Bool = true, failHaptic: Bool = false, _ work: () async throws -> Void) async {
        do {
            try await work()
            if reload { await load() }
        } catch {
            guard !error.isCancellation else { return }
            if failHaptic { Haptics.error() }
            errorMessage = error.message(fallback)
        }
    }

    func deleteComment(_ comment: GalleryComment) async {
        await act("Failed to delete comment.") { _ = try await self.core.deleteGalleryComment(commentId: comment.id) }
    }

    func toggleSelection(_ comment: GalleryComment) {
        if selectedCommentIds.contains(comment.id) { selectedCommentIds.remove(comment.id) } else { selectedCommentIds.insert(comment.id) }
    }

    func bulkDeleteSelectedComments() async {
        await bulkDelete(kind: "comments", ids: selectedCommentIds, fallback: "Failed to delete comments.") { self.selectedCommentIds.removeAll() }
    }

    func setReportStatus(_ report: GalleryReport, status: String) async {
        await act("Failed to update report.") { _ = try await self.core.setGalleryReportStatus(reportId: report.id, status: status) }
    }

    func setModerationStatus(_ item: GalleryMedia, status: String) async {
        await act("Failed to update media.") {
            _ = try await self.core.updateGalleryMedia(mediaId: item.id, moderationStatus: status, moderationReason: nil, isAdult: nil)
        }
    }

    func toggleAdult(_ item: GalleryMedia) async {
        await act("Failed to update media.") {
            _ = try await self.core.updateGalleryMedia(mediaId: item.id, moderationStatus: nil, moderationReason: nil, isAdult: !(item.isAdult ?? false))
        }
    }

    func deleteMedia(_ item: GalleryMedia) async {
        await act("Failed to delete media.", failHaptic: true) {
            _ = try await self.core.deleteGalleryMedia(mediaId: item.id)
            Haptics.warning()
        }
    }

    func toggleMediaSelection(_ item: GalleryMedia) {
        if selectedMediaIds.contains(item.id) { selectedMediaIds.remove(item.id) } else { selectedMediaIds.insert(item.id) }
    }

    func bulkDeleteSelectedMedia() async {
        await bulkDelete(kind: "media", ids: selectedMediaIds, fallback: "Failed to delete media.") { self.selectedMediaIds.removeAll() }
    }

    func toggleEmailVerified(_ user: GalleryUser) async {
        await act("Failed to update verification.") { _ = try await self.core.setGalleryUserEmailVerified(userId: user.id, verified: !user.isEmailVerified) }
    }

    func toggleAgeVerified(_ user: GalleryUser) async {
        await act("Failed to update age verification.") { _ = try await self.core.setGalleryUserAgeVerified(userId: user.id, verified: !user.isAgeVerified) }
    }

    func resendVerification(_ user: GalleryUser) async {
        await act("Failed to resend verification.", reload: false) {
            let response = try await self.core.resendGalleryUserVerification(userId: user.id)
            if response.alreadyVerified == true {
                self.statusMessage = "\(user.username) is already verified."
            } else if response.emailVerificationSent == true {
                self.statusMessage = "Verification email sent to \(user.username)."
            } else {
                self.errorMessage = "Could not send a verification email to \(user.username)."
            }
        }
    }

    func resetPassword(_ user: GalleryUser, newPassword: String) async {
        await act("Failed to reset password.", reload: false) {
            _ = try await self.core.resetGalleryUserPassword(userId: user.id, newPassword: newPassword)
            self.statusMessage = "Password reset for \(user.username)."
        }
    }

    func update(_ user: GalleryUser, username: String, displayName: String, email: String, publicProfile: Bool) async {
        await act("Failed to update user.") {
            _ = try await self.core.updateGalleryUser(userId: user.id, username: username, displayName: displayName, email: email, publicProfile: publicProfile)
            self.statusMessage = "\(username) updated."
        }
    }

    func deleteUser(_ user: GalleryUser) async {
        await act("Failed to delete user.", failHaptic: true) {
            _ = try await self.core.deleteGalleryUser(userId: user.id)
            Haptics.warning()
        }
    }

    func toggleUserSelection(_ user: GalleryUser) {
        if selectedUserIds.contains(user.id) { selectedUserIds.remove(user.id) } else { selectedUserIds.insert(user.id) }
    }

    func bulkDeleteSelectedUsers() async {
        await bulkDelete(kind: "users", ids: selectedUserIds, fallback: "Failed to delete users.") { self.selectedUserIds.removeAll() }
    }

    private func bulkDelete(kind: String, ids: Set<Int>, fallback: String, clear: () -> Void) async {
        guard !ids.isEmpty else { return }
        isBulkDeleting = true
        defer { isBulkDeleting = false }
        await act(fallback, failHaptic: true) {
            _ = try await self.core.galleryBulkDelete(kind: kind, ids: Array(ids))
            clear()
            Haptics.success()
        }
    }

    func exportMediaCSV() async -> URL? {
        await downloadCSV(kind: "media", filename: "gallery_media.csv")
    }

    func exportUsersCSV() async -> URL? {
        await downloadCSV(kind: "users", filename: "gallery_users.csv")
    }

    private func downloadCSV(kind: String, filename: String) async -> URL? {
        isExporting = true
        defer { isExporting = false }
        do {
            let data = try await core.galleryExportCsv(kind: kind)
            let url = FileManager.default.temporaryDirectory.appendingPathComponent(filename)
            try data.write(to: url, options: .atomic)
            return url
        } catch {
            guard !error.isCancellation else { return nil }
            errorMessage = error.message("Failed to export CSV.")
            return nil
        }
    }
}
