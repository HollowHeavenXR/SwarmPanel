import SwiftUI

/// Mirrors the web panel's SWARM_MONO_COLUMN_RE (static/app.js) -- id/hash/
/// token/uuid-shaped columns get a monospaced, accent-tinted treatment so
/// they read as identifiers rather than ordinary text at a glance.
private func isIdLikeColumn(_ name: String) -> Bool {
    let lower = name.lowercased()
    return lower == "id" || lower == "uuid"
        || lower.hasSuffix("_id") || lower.hasSuffix("_hash")
        || lower.hasSuffix("_token") || lower.hasSuffix("_uuid")
}

struct DatabasesView: View {
    @StateObject private var viewModel = DatabasesViewModel()
    @State private var shareURL: IdentifiableURL?
    @State private var isExporting = false
    @State private var truncateTarget: TruncateTarget?

    private enum TruncateTarget: Identifiable {
        case table(schema: String, table: String)
        case schema(schema: String)
        var id: String {
            switch self {
            case .table(let schema, let table): return "table:\(schema).\(table)"
            case .schema(let schema): return "schema:\(schema)"
            }
        }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                if let error = viewModel.errorMessage {
                    ErrorBanner(message: error).padding(.horizontal)
                }

                PanelCard {
                    Picker("Schema", selection: $viewModel.selectedSchema) {
                        ForEach(viewModel.schemas) { schema in
                            Text(schema.schema).tag(schema.schema)
                        }
                    }
                    Picker("Table", selection: $viewModel.selectedTable) {
                        ForEach(viewModel.tables, id: \.self) { table in
                            Text(table).tag(table)
                        }
                    }
                    HStack {
                        Button {
                            Task { await viewModel.loadRows() }
                        } label: {
                            if viewModel.isLoadingRows {
                                ProgressView()
                            } else {
                                Label("Load", systemImage: "tablecells")
                            }
                        }
                        .disabled(viewModel.isLoadingRows || viewModel.selectedTable.isEmpty)

                        Spacer()

                        Button {
                            Task {
                                isExporting = true
                                if let url = await viewModel.exportCSV() { shareURL = IdentifiableURL(url: url) }
                                isExporting = false
                            }
                        } label: {
                            if isExporting {
                                ProgressView()
                            } else {
                                Label("Export CSV", systemImage: "square.and.arrow.down")
                            }
                        }
                        .disabled(isExporting || viewModel.selectedTable.isEmpty)
                    }
                    .tint(SwarmTheme.accent)

                    Divider().overlay(SwarmTheme.line)

                    HStack {
                        Button(role: .destructive) {
                            guard !viewModel.selectedSchema.isEmpty, !viewModel.selectedTable.isEmpty else { return }
                            truncateTarget = .table(schema: viewModel.selectedSchema, table: viewModel.selectedTable)
                        } label: {
                            Label("Truncate Table", systemImage: "trash")
                        }
                        .disabled(viewModel.selectedTable.isEmpty)

                        Spacer()

                        Button(role: .destructive) {
                            guard !viewModel.selectedSchema.isEmpty else { return }
                            truncateTarget = .schema(schema: viewModel.selectedSchema)
                        } label: {
                            Label("Truncate Schema", systemImage: "trash.fill")
                        }
                        .disabled(viewModel.selectedSchema.isEmpty)
                    }
                    .font(.caption)
                    .buttonStyle(.borderless)
                }
                .padding(.horizontal)

                if viewModel.rows.isEmpty {
                    PanelCard { EmptyStateView(icon: "cylinder.split.1x2", title: "No rows loaded yet.") }
                        .padding(.horizontal)
                } else {
                    ForEach(Array(viewModel.rows.enumerated()), id: \.offset) { _, row in
                        PanelCard {
                            ForEach(Array(row.keys.sorted()), id: \.self) { key in
                                HStack(alignment: .top) {
                                    Text(key).font(.caption.bold()).foregroundStyle(SwarmTheme.textMuted)
                                    Spacer()
                                    Text(row[key] ?? "—")
                                        .font(isIdLikeColumn(key) ? .caption.monospaced() : .caption)
                                        .foregroundStyle(isIdLikeColumn(key) ? SwarmTheme.accent : SwarmTheme.textPrimary)
                                        .multilineTextAlignment(.trailing)
                                }
                            }
                        }
                        .padding(.horizontal)
                    }
                }
            }
            .padding(.vertical)
        }
        .background(ResonanceBackdrop().ignoresSafeArea())
        .navigationTitle("Database Viewer")
        .task { await viewModel.loadSchemas() }
        .onChange(of: viewModel.selectedSchema) { _ in
            viewModel.selectedTable = viewModel.tables.first ?? ""
            viewModel.rows = []
        }
        .refreshable {
            Haptics.light()
            await viewModel.loadSchemas()
        }
        .refreshOnForeground { await viewModel.loadSchemas() }
        .sheet(item: $shareURL) { item in
            ActivityShareSheet(activityItems: [item.url])
        }
        .sheet(item: $truncateTarget) { target in
            NavigationStack {
                switch target {
                case .table(let schema, let table):
                    TruncateConfirmSheet(
                        title: "Truncate Table",
                        expectedText: viewModel.expectedTableConfirmText(schema: schema, table: table),
                        isWorking: viewModel.isTruncating
                    ) { confirmText, ownerText in
                        let ok = await viewModel.truncateTable(schema: schema, table: table, confirmText: confirmText, ownerConfirmText: ownerText)
                        if ok { truncateTarget = nil }
                        return ok
                    }
                case .schema(let schema):
                    TruncateConfirmSheet(
                        title: "Truncate Schema",
                        expectedText: viewModel.expectedSchemaConfirmText(schema: schema),
                        isWorking: viewModel.isTruncating
                    ) { confirmText, ownerText in
                        let ok = await viewModel.truncateSchema(schema: schema, confirmText: confirmText, ownerConfirmText: ownerText)
                        if ok { truncateTarget = nil }
                        return ok
                    }
                }
            }
        }
    }
}

/// Double-typed-confirmation gate for a destructive database action —
/// mirrors routes.lua's server-side check exactly (both fields are
/// re-validated server-side regardless of what's typed here; this is only
/// a UX nicety that keeps the button disabled until the text matches, not
/// the actual security boundary). `expectedText` is the exact
/// "TRUNCATE schema.table" / "TRUNCATE ALL schema" string the backend
/// requires; the second field is whatever phrase this SwarmPanel deployment
/// has configured as its owner/site confirmation phrase (PANEL_DESTRUCTIVE_
/// CONFIRMATION_PHRASE) — the admin has to know it, it isn't shown here.
private struct TruncateConfirmSheet: View {
    let title: String
    let expectedText: String
    let isWorking: Bool
    /// Returns true on success so the caller can dismiss the sheet.
    let onConfirm: (String, String) async -> Bool

    @Environment(\.dismiss) private var dismiss
    @State private var confirmText = ""
    @State private var ownerConfirmText = ""
    @State private var errorMessage: String?

    private var canConfirm: Bool {
        confirmText.trimmingCharacters(in: .whitespacesAndNewlines) == expectedText && !isWorking
    }

    var body: some View {
        Form {
            Section {
                Label("This permanently deletes data. It cannot be undone.", systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(SwarmTheme.danger)
                    .font(.subheadline.bold())
            }
            .listRowBackground(ResRowBackground())

            Section {
                Text(expectedText)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(SwarmTheme.textPrimary)
                    .textSelection(.enabled)
                TextField("Type the text above exactly", text: $confirmText)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
            } header: {
                SectionLabel(title: "Type to confirm")
            }
            .listRowBackground(ResRowBackground())

            Section {
                SecureField("Site owner confirmation phrase", text: $ownerConfirmText)
            } header: {
                SectionLabel(title: "Owner phrase")
            } footer: {
                Text("The confirmation phrase this SwarmPanel deployment is configured with. Leave blank if none is configured — the server will reject this if one is required.")
            }
            .listRowBackground(ResRowBackground())

            if let errorMessage {
                Section { ErrorBanner(message: errorMessage) }
                    .listRowBackground(ResRowBackground())
            }

            Section {
                Button(role: .destructive) {
                    Task {
                        errorMessage = nil
                        let ok = await onConfirm(confirmText, ownerConfirmText)
                        if !ok { errorMessage = "Confirmation rejected by the server." }
                    }
                } label: {
                    HStack {
                        Spacer()
                        if isWorking { ProgressView() } else { Text(title).bold() }
                        Spacer()
                    }
                }
                .disabled(!canConfirm)
            }
            .listRowBackground(SwarmTheme.danger.opacity(canConfirm ? 0.85 : 0.3))
        }
        .scrollContentBackground(.hidden)
        .background(ResonanceBackdrop().ignoresSafeArea())
        .navigationTitle(title)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { dismiss() }
            }
        }
    }
}

#Preview {
    NavigationStack { DatabasesView() }
        .environmentObject(ToastCenter())
}
