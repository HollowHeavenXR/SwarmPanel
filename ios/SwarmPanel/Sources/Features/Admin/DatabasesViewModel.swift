import Foundation

@MainActor
final class DatabasesViewModel: ObservableObject {
    @Published var schemas: [SchemaEntry] = []
    @Published var selectedSchema = ""
    @Published var selectedTable = ""
    /// column -> display text (formatted by the core)
    @Published var rows: [[String: String]] = []
    @Published var isLoadingSchemas = true
    @Published var isLoadingRows = false
    @Published var errorMessage: String?
    @Published var isTruncating = false

    private let core = Core.shared

    var tables: [String] {
        schemas.first(where: { $0.schema == selectedSchema })?.tableNames ?? []
    }

    func loadSchemas() async {
        isLoadingSchemas = true
        defer { isLoadingSchemas = false }
        do {
            schemas = try await core.schemas().schemas ?? []
            if selectedSchema.isEmpty, let first = schemas.first {
                selectedSchema = first.schema
                selectedTable = first.tableNames.first ?? ""
            }
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load schemas.")
        }
    }

    func loadRows() async {
        guard !selectedSchema.isEmpty, !selectedTable.isEmpty else { return }
        isLoadingRows = true
        defer { isLoadingRows = false }
        do {
            let data = try await core.tableData(schema: selectedSchema, table: selectedTable, limit: 100)
            rows = data.rows.map { row in
                Dictionary(uniqueKeysWithValues: zip(data.columns, row.cells))
            }
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            rows = []
            errorMessage = error.message("Failed to load table data.")
        }
    }

    func expectedTableConfirmText(schema: String, table: String) -> String {
        "TRUNCATE \(schema).\(table)"
    }

    func expectedSchemaConfirmText(schema: String) -> String {
        "TRUNCATE ALL \(schema)"
    }

    @discardableResult
    func truncateTable(schema: String, table: String, confirmText: String, ownerConfirmText: String) async -> Bool {
        isTruncating = true
        defer { isTruncating = false }
        do {
            let response = try await core.truncateTable(schema: schema, table: table, confirmText: confirmText, ownerConfirmText: ownerConfirmText)
            errorMessage = nil
            Haptics.warning()
            if response.ok == true { await loadRows() }
            return response.ok == true
        } catch {
            guard !error.isCancellation else { return false }
            errorMessage = error.message("Failed to truncate table.")
            Haptics.error()
            return false
        }
    }

    @discardableResult
    func truncateSchema(schema: String, confirmText: String, ownerConfirmText: String) async -> Bool {
        isTruncating = true
        defer { isTruncating = false }
        do {
            let response = try await core.truncateSchema(schema: schema, confirmText: confirmText, ownerConfirmText: ownerConfirmText)
            errorMessage = nil
            Haptics.warning()
            if response.ok == true { await loadSchemas() }
            return response.ok == true
        } catch {
            guard !error.isCancellation else { return false }
            errorMessage = error.message("Failed to truncate schema.")
            Haptics.error()
            return false
        }
    }

    func exportCSV() async -> URL? {
        guard !selectedSchema.isEmpty, !selectedTable.isEmpty else { return nil }
        do {
            let data = try await core.tableCsv(schema: selectedSchema, table: selectedTable)
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("\(selectedSchema)_\(selectedTable).csv")
            try data.write(to: url, options: .atomic)
            return url
        } catch {
            guard !error.isCancellation else { return nil }
            errorMessage = error.message("Failed to export CSV.")
            return nil
        }
    }
}
