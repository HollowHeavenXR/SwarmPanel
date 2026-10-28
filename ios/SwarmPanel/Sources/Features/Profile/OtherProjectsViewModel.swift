import Foundation

@MainActor
final class OtherProjectsViewModel: ObservableObject {
    @Published var isDownloading = false
    @Published var errorMessage: String?

    private let core = Core.shared

    func downloadLumisound() async -> URL? {
        isDownloading = true
        defer { isDownloading = false }
        do {
            let data = try await core.lumisoundDownload()
            let tempURL = FileManager.default.temporaryDirectory.appendingPathComponent("Lumisound.ipa")
            try data.write(to: tempURL, options: .atomic)
            return tempURL
        } catch {
            guard !error.isCancellation else { return nil }
            errorMessage = error.message("Failed to download Lumisound.")
            return nil
        }
    }
}
