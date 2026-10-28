import Foundation

@MainActor
final class MetricsChartViewModel: ObservableObject {
    @Published var queuedPoints: [MetricPoint] = []
    @Published var queuedAnomalies: [MetricAnomaly] = []
    @Published var activeBotsPoints: [MetricPoint] = []
    @Published var activeBotsAnomalies: [MetricAnomaly] = []
    @Published var isLoading = true

    private let core = Core.shared

    func load() async {
        isLoading = true
        defer { isLoading = false }
        async let qp = core.metricHistory(metric: "queued_tracks", hours: 24)
        async let qa = core.metricAnomalies(metric: "queued_tracks", hours: 24)
        async let ap = core.metricHistory(metric: "active_bots", hours: 24)
        async let aa = core.metricAnomalies(metric: "active_bots", hours: 24)
        if let r = try? await qp { queuedPoints = r.points ?? [] }
        if let r = try? await qa { queuedAnomalies = r.anomalies ?? [] }
        if let r = try? await ap { activeBotsPoints = r.points ?? [] }
        if let r = try? await aa { activeBotsAnomalies = r.anomalies ?? [] }
    }
}
