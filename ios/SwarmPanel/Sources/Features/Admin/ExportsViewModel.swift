import Foundation

@MainActor
final class ExportsViewModel: ObservableObject {
    @Published var snapshots: [ExportSnapshot] = []
    @Published var enabled = true
    @Published var isLoading = true
    @Published var downloadingFileId: String?
    @Published var errorMessage: String?

    private let core = Core.shared

    func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            let response = try await core.exports()
            snapshots = response.snapshots ?? []
            enabled = response.enabled ?? false
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load exports.")
        }
    }

    func download(date: String, file: ExportFile) async -> URL? {
        downloadingFileId = file.id
        defer { downloadingFileId = nil }
        do {
            let data = try await core.downloadExport(date: date, fileName: file.name)
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("\(date)_\(file.name)")
            try data.write(to: url, options: .atomic)
            return url
        } catch {
            guard !error.isCancellation else { return nil }
            errorMessage = error.message("Failed to download file.")
            return nil
        }
    }
}
