import Foundation

@MainActor
final class DiagnosticsViewModel: ObservableObject {
    @Published var metricsText = ""
    @Published var stabilityText = ""
    @Published var events: [FeedEvent] = []
    @Published var isLoading = true
    @Published var errorMessage: String?

    private let core = Core.shared

    func load() async {
        isLoading = true
        defer { isLoading = false }
        async let metricsDoc = core.metrics()
        async let stabilityDoc = core.stability()
        async let feed = core.events()
        do { metricsText = try await metricsDoc.pretty } catch { metricsText = Self.unavailable(error) }
        do { stabilityText = try await stabilityDoc.pretty } catch { stabilityText = Self.unavailable(error) }
        events = (try? await feed.events) ?? []
    }

    private static func unavailable(_ error: Error) -> String {
        error.isCancellation ? "" : "Unavailable: \(error.message("request failed"))"
    }
}
