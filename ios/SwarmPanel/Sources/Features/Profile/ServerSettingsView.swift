import SwiftUI

/// Lets the base URL be overridden for local development against the Docker
/// container (e.g. http://127.0.0.1:8003) instead of the production domain.
/// Changing it logs the current session out — a token from one backend is
/// meaningless against another, so there is no safe way to keep it signed in.
struct ServerSettingsView: View {
    @EnvironmentObject private var appState: AppState
    @Environment(\.dismiss) private var dismiss
    @State private var urlText = Core.shared.baseUrl()
    @State private var errorMessage: String?

    var body: some View {
        Form {
            Section {
                HStack {
                    IconChip(systemName: "server.rack", tint: .gray)
                    TextField("https://your-backend", text: $urlText)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                }
            } header: {
                SectionLabel(title: "Backend URL")
            } footer: {
                Text("Default is the production SwarmPanel backend. Only change this for local development against a Docker instance (e.g. http://127.0.0.1:8003). Saving logs you out.")
            }
            .listRowBackground(ResRowBackground())

            if let errorMessage {
                Section { Text(errorMessage).foregroundStyle(SwarmTheme.danger) }
                    .listRowBackground(ResRowBackground())
            }

            Section {
                Button("Reset to Default") {
                    urlText = Core.shared.defaultBaseUrl()
                }
            }
            .listRowBackground(ResRowBackground())

            Section {
                Button {
                    save()
                } label: {
                    HStack {
                        Spacer()
                        Text("Save & Log Out").bold()
                        Spacer()
                    }
                    .foregroundStyle(.white)
                    .padding(.vertical, 4)
                }
            }
            .listRowBackground(Rectangle().fill(SwarmTheme.accent.gradient))
        }
        .scrollContentBackground(.hidden)
        .background(ResonanceBackdrop().ignoresSafeArea())
        .navigationTitle("Server")
    }

    private func save() {
        let trimmed = urlText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: trimmed), let scheme = url.scheme, ["http", "https"].contains(scheme), url.host != nil else {
            errorMessage = "Enter a valid http:// or https:// URL."
            return
        }
        Core.shared.setBaseUrl(url: url.absoluteString)
        appState.logout()
        dismiss()
    }
}

#Preview {
    NavigationStack { ServerSettingsView() }
        .environmentObject(AppState())
}
