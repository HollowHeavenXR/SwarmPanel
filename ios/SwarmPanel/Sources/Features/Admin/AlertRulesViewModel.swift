import Foundation

@MainActor
final class AlertRulesViewModel: ObservableObject {
    @Published var rules: [AlertRule] = []
    @Published var isLoading = true
    @Published var isSaving = false
    @Published var errorMessage: String?

    private let core = Core.shared

    func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            rules = try await core.alertRules().rules ?? []
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load alert rules.")
        }
    }

    func create(ruleType: String, thresholdMinutes: Int, escalationMinutes: Int? = nil, escalateEmail: Bool = false) async {
        isSaving = true
        defer { isSaving = false }
        do {
            _ = try await core.createAlertRule(ruleType: ruleType, thresholdMinutes: thresholdMinutes, enabled: true,
                                               escalationMinutes: escalationMinutes, escalateEmail: escalationMinutes != nil ? escalateEmail : nil)
            await load()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to create alert rule.")
        }
    }

    func toggle(_ rule: AlertRule) async {
        do {
            _ = try await core.setAlertRuleEnabled(ruleId: rule.id, enabled: !rule.enabled)
            await load()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to update alert rule.")
        }
    }

    func delete(_ rule: AlertRule) async {
        do {
            _ = try await core.deleteAlertRule(ruleId: rule.id)
            await load()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to delete alert rule.")
        }
    }
}
