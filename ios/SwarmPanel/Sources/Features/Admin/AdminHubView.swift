import SwiftUI

/// Admin tools hub — the iOS counterpart of the web panel's /admin overview
/// (lua/src/nav.lua's "admin" section). Every admin/moderator screen used to
/// be a flat list in the middle of the Profile form; they now live here,
/// grouped by what they're for, one tap from the Account tab. Each group
/// only shows the tools this session's role can use, with the same gates
/// as the web panel (admin / moderator / gallery owner).
struct AdminHubView: View {
    @EnvironmentObject private var appState: AppState
    @State private var stats: AdminOverviewStats?

    private var canModerate: Bool { appState.isAdmin || appState.isModerator }

    var body: some View {
        List {
            Section {
                ResScreenHeader(eyebrow: "Command center", title: "Admin", subtitle: "Monitoring, moderation and data for the whole swarm.")
                    .padding(.horizontal, -20)
                    .listRowInsets(EdgeInsets(top: 8, leading: 20, bottom: 8, trailing: 20))
                    .listRowBackground(Color.clear)
            }

            // Same stats as the web /admin overview.
            if let stats {
                Section {
                    if let online = stats.botsOnline, let total = stats.botsTotal {
                        AdminStatRow(label: "Bots online", value: "\(online) / \(total)", attention: online < total)
                    }
                    if let rules = stats.alertRulesEnabled {
                        AdminStatRow(label: "Alert rules on", value: "\(rules)", attention: false)
                    }
                    if let audit = stats.auditEntriesRecent {
                        AdminStatRow(label: "Audit entries (24h)", value: "\(audit)", attention: false)
                    }
                    if let reports = stats.openReports {
                        AdminStatRow(label: "Open gallery reports", value: "\(reports)", attention: reports > 0)
                    }
                } header: {
                    SectionLabel(title: "At a Glance")
                }
                .listRowBackground(ResRowBackground())
            }

            if appState.isAdmin {
                Section {
                    NavigationLink { DiagnosticsView() } label: {
                        IconRow(icon: "heart.text.square", tint: .green, title: "Fleet Health", subtitle: "Stability and live metrics")
                    }
                    NavigationLink { FleetTopologyView() } label: {
                        IconRow(icon: "point.3.filled.connected.trianglepath.dotted", tint: .cyan, title: "Fleet Topology", subtitle: "Bots, guilds, and audio nodes")
                    }
                    NavigationLink { AlertRulesView() } label: {
                        IconRow(icon: "bell.badge", tint: .red, title: "Alert Rules", subtitle: "When the fleet pages you")
                    }
                    NavigationLink { ExportsView() } label: {
                        IconRow(icon: "square.and.arrow.down", tint: .orange, title: "Scheduled Exports", subtitle: "Recurring CSV reports")
                    }
                } header: {
                    SectionLabel(title: "Monitoring")
                }
                .listRowBackground(ResRowBackground())
            } else if appState.isModerator {
                // Moderators could already open Alert Rules from Profile
                // before the restructure — keep that access.
                Section {
                    NavigationLink { AlertRulesView() } label: {
                        IconRow(icon: "bell.badge", tint: .red, title: "Alert Rules", subtitle: "When the fleet pages you")
                    }
                } header: {
                    SectionLabel(title: "Monitoring")
                }
                .listRowBackground(ResRowBackground())
            }

            if canModerate || appState.canGallery {
                Section {
                    if canModerate {
                        NavigationLink { AuditLogView() } label: {
                            IconRow(icon: "list.bullet.clipboard", tint: .indigo, title: "Audit Log", subtitle: "Every recorded admin action")
                        }
                        NavigationLink { LumisoundAdminView() } label: {
                            IconRow(icon: "waveform", tint: .pink, title: "Lumisound", subtitle: "Accounts, library, and uploads")
                        }
                    }
                    if appState.canGallery {
                        NavigationLink { GalleryModerationView() } label: {
                            IconRow(icon: "photo.on.rectangle", tint: .teal, title: "Gallery", subtitle: "Users, media, comments, and reports")
                        }
                    }
                } header: {
                    SectionLabel(title: "Moderation")
                }
                .listRowBackground(ResRowBackground())
            }

            if appState.isAdmin {
                Section {
                    NavigationLink { AccountsAdminView() } label: {
                        IconRow(icon: "person.2.badge.gearshape", tint: .blue, title: "Accounts", subtitle: "Recover and manage swarm accounts")
                    }
                    NavigationLink { DatabasesView() } label: {
                        IconRow(icon: "cylinder.split.1x2", tint: .brown, title: "Databases", subtitle: "Browse raw schema tables")
                    }
                } header: {
                    SectionLabel(title: "Data")
                }
                .listRowBackground(ResRowBackground())
            }
        }
        .scrollContentBackground(.hidden)
        .background(ResonanceBackdrop().ignoresSafeArea())
        .navigationTitle("Admin")
        .navigationBarTitleDisplayMode(.inline)
        .task { await loadStats() }
        .refreshable { await loadStats() }
    }

    private func loadStats() async {
        // Silent on failure: the tool list below still works without stats.
        stats = try? await Core.shared.adminOverview()
    }
}

private struct AdminStatRow: View {
    let label: String
    let value: String
    let attention: Bool

    var body: some View {
        LabeledContent(label) {
            Text(value)
                .font(.body.monospacedDigit().bold())
                .foregroundStyle(attention ? SwarmTheme.warn : SwarmTheme.textPrimary)
        }
    }
}

#Preview {
    NavigationStack { AdminHubView() }
        .environmentObject(AppState())
}
