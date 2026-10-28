import Foundation

/// Password change, email change, and account verification -- entirely
/// absent from the app before this (web's ProfilePage.jsx has all three;
/// the native Profile screen only ever showed a read-only "Account
/// verified" checkbox with no way to actually verify, and no path to
/// change a password or email at all).
@MainActor
final class AccountSecurityViewModel: ObservableObject {
    @Published var email = ""
    @Published var hasPassword = false

    @Published var isVerified = false
    @Published var verificationPending = false
    @Published var verificationWebhookUrl = ""
    @Published var discordUserId = ""

    @Published var currentPassword = ""
    @Published var newPassword = ""
    @Published var verificationCode = ""

    @Published var isLoading = true
    @Published var isSaving = false
    @Published var errorMessage: String?
    @Published var statusMessage: String?

    private let core = Core.shared

    func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            let response = try await core.me()
            apply(response.profile)
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load account details.")
        }
    }

    private func apply(_ profile: MeProfile) {
        email = profile.email ?? ""
        hasPassword = profile.hasPassword ?? false
        isVerified = profile.verificationVerified ?? false
        verificationPending = profile.verificationPending ?? false
        verificationWebhookUrl = profile.verificationWebhookUrl ?? ""
        discordUserId = profile.discordUserId ?? ""
    }

    func changePassword() async {
        guard !currentPassword.isEmpty, newPassword.count >= 8 else {
            errorMessage = "Enter your current password and a new password (8+ characters)."
            return
        }
        await run("Password changed.") { [self] in
            let envelope = try await core.changePassword(currentPassword: currentPassword, newPassword: newPassword)
            apply(envelope.profile)
            currentPassword = ""
            newPassword = ""
        }
    }

    func changeEmail() async {
        guard email.contains("@") else {
            errorMessage = "Enter a valid email address."
            return
        }
        await run("Email updated.") { [self] in
            let envelope = try await core.changeEmail(email: email)
            apply(envelope.profile)
        }
    }

    /// Empty *verificationWebhookUrl* is a valid, intentional "clear it"
    /// request server-side (see routes.lua) -- only guard against a code
    /// never being sent for a non-empty one.
    func saveWebhook() async {
        await run(nil) { [self] in
            let response = try await core.setVerificationWebhook(url: verificationWebhookUrl)
            apply(response.profile)
            statusMessage = response.verificationSent == true ? "Code sent to your webhook." : "Saved."
        }
    }

    func saveDiscordUserId() async {
        await run(nil) { [self] in
            let response = try await core.setVerificationDiscord(discordUserId: discordUserId)
            apply(response.profile)
            statusMessage = response.verificationSent == true ? "Code sent via Discord DM." : "Saved."
        }
    }

    func submitCode() async {
        guard !verificationCode.trimmingCharacters(in: .whitespaces).isEmpty else {
            errorMessage = "Enter the verification code you received."
            return
        }
        await run("Verified.") { [self] in
            let envelope = try await core.verifyCode(code: verificationCode)
            apply(envelope.profile)
            verificationCode = ""
        }
    }

    func resendCode() async {
        await run(nil) { [self] in
            let response = try await core.resendVerification()
            if response.alreadyVerified == true {
                statusMessage = "Already verified."
            } else if response.verificationSent == true {
                statusMessage = "Code resent."
            } else {
                errorMessage = "Could not resend a code — set a webhook or Discord User ID above first."
            }
        }
    }

    private func run(_ successMessage: String?, _ action: () async throws -> Void) async {
        isSaving = true
        errorMessage = nil
        statusMessage = nil
        defer { isSaving = false }
        do {
            try await action()
            if let successMessage { statusMessage = successMessage }
            Haptics.success()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Request failed.")
            Haptics.error()
        }
    }
}
