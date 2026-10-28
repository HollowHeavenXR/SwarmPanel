import Foundation

@MainActor
final class AccountsAdminViewModel: ObservableObject {
    @Published var accounts: [SwarmAccountSummary] = []
    @Published var query = ""
    @Published var isLoading = true
    @Published var errorMessage: String?
    @Published var statusMessage: String?
    @Published var isSelecting = false
    @Published var selectedIds: Set<Int> = []
    @Published var isBulkWorking = false

    private let core = Core.shared

    func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            accounts = try await core.swarmAccounts(query: query).data.users ?? []
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load accounts.")
        }
    }

    func toggleVerified(_ account: SwarmAccountSummary) async {
        await run("Failed to update verification.") {
            _ = try await self.core.setAccountVerified(accountId: account.id, verified: !(account.verificationVerified ?? false))
            await self.load()
        }
    }

    func toggleModerator(_ account: SwarmAccountSummary) async {
        await run("Failed to update moderator status.") {
            _ = try await self.core.setAccountModerator(accountId: account.id, moderator: !account.isModerator)
            self.statusMessage = account.isModerator ? "Moderator access revoked." : "Moderator access granted."
            await self.load()
        }
    }

    func resetPassword(_ account: SwarmAccountSummary, newPassword: String) async {
        await run("Failed to reset password.") {
            _ = try await self.core.resetAccountPassword(accountId: account.id, newPassword: newPassword)
            self.statusMessage = "Password reset for \(account.username)."
        }
    }

    func update(_ account: SwarmAccountSummary, username: String, displayName: String, email: String, guildId: String, serverName: String, publicProfile: Bool) async {
        await run("Failed to update account.") {
            _ = try await self.core.updateAccount(accountId: account.id, username: username, displayName: displayName, email: email,
                                                  guildId: guildId, serverName: serverName, publicProfile: publicProfile)
            self.statusMessage = "\(username) updated."
            await self.load()
        }
    }

    func delete(_ account: SwarmAccountSummary) async {
        await run("Failed to delete account.", failHaptic: true) {
            _ = try await self.core.deleteAccount(accountId: account.id)
            Haptics.warning()
            await self.load()
        }
    }

    func resendVerification(_ account: SwarmAccountSummary) async {
        await run("Failed to resend verification.") {
            let response = try await self.core.resendAccountVerification(accountId: account.id)
            if response.alreadyVerified == true {
                self.statusMessage = "\(account.username) is already verified."
            } else if response.verificationSent == true {
                self.statusMessage = "Verification code sent to \(account.username)."
            } else {
                self.errorMessage = "Could not send a verification code to \(account.username)."
            }
        }
    }

    func toggleSelection(_ id: Int) {
        if selectedIds.contains(id) { selectedIds.remove(id) } else { selectedIds.insert(id) }
    }

    func exitSelectionMode() {
        isSelecting = false
        selectedIds.removeAll()
    }

    func bulkVerify(_ verified: Bool) async {
        guard !selectedIds.isEmpty else { return }
        isBulkWorking = true
        defer { isBulkWorking = false }
        await run("Bulk verify failed.") {
            let result = try await self.core.bulkVerifyAccounts(ids: Array(self.selectedIds), verified: verified)
            self.statusMessage = "Bulk \(verified ? "verify" : "unverify"): \(bulkSummary(result: result))"
            self.exitSelectionMode()
            await self.load()
        }
    }

    func bulkDelete() async {
        guard !selectedIds.isEmpty else { return }
        isBulkWorking = true
        defer { isBulkWorking = false }
        await run("Bulk delete failed.", failHaptic: true) {
            let result = try await self.core.bulkDeleteAccounts(ids: Array(self.selectedIds))
            Haptics.warning()
            self.statusMessage = "Bulk delete: \(bulkSummary(result: result))"
            self.exitSelectionMode()
            await self.load()
        }
    }

    private func run(_ fallback: String, failHaptic: Bool = false, _ work: () async throws -> Void) async {
        do {
            try await work()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message(fallback)
            if failHaptic { Haptics.error() }
        }
    }
}
