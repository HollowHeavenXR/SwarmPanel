import Foundation

@MainActor
final class ProfileViewModel: ObservableObject {
    @Published var displayName = ""
    @Published var bio = ""
    @Published var isPublic = true
    @Published var isSaving = false
    @Published var errorMessage: String?
    @Published var statusMessage: String?
    @Published var isVerified = false
    @Published var hasAvatar = false

    private let core = Core.shared

    func load() async {
        do {
            let response = try await core.me()
            displayName = response.profile.displayName ?? ""
            bio = response.profile.bio ?? ""
            isPublic = response.profile.publicProfile ?? true
            isVerified = response.profile.verificationVerified ?? false
            hasAvatar = !(response.profile.avatarUrl ?? "").isEmpty
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load profile.")
        }
    }

    func save(themeAccentHex: String? = nil) async {
        isSaving = true
        statusMessage = nil
        defer { isSaving = false }
        do {
            _ = try await core.updateProfile(displayName: displayName, bio: bio, publicProfile: isPublic, themeAccent: themeAccentHex)
            statusMessage = "Profile saved."
            errorMessage = nil
            Haptics.success()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to save profile.")
            Haptics.error()
        }
    }
}
