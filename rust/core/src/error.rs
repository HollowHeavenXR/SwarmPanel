//! Errors surfaced to Swift (mirrors the old Swift `APIError`).

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ApiError {
    #[error("The server returned an unexpected response.")]
    InvalidResponse,
    #[error("{message}")]
    Network { message: String, code: i32 },
    #[error("Couldn't read the server's response.")]
    Decoding { message: String },
    #[error("{message}")]
    Server { status_code: u16, message: String },
    #[error("Your session expired. Please log in again.")]
    Unauthorized,
    #[error("{message}")]
    Invalid { message: String },
}

impl ApiError {
    /// Short machine label used in telemetry ("server_500", "network_-1", ...).
    pub fn kind(&self) -> String {
        match self {
            ApiError::InvalidResponse => "invalid_response".into(),
            ApiError::Network { code, .. } => format!("network_{code}"),
            ApiError::Decoding { .. } => "decoding".into(),
            ApiError::Server { status_code, .. } => format!("server_{status_code}"),
            ApiError::Unauthorized => "unauthorized".into(),
            ApiError::Invalid { .. } => "invalid".into(),
        }
    }
    pub fn invalid(m: impl Into<String>) -> Self {
        ApiError::Invalid { message: m.into() }
    }
}

/// Message text for an error (Swift: `error.localizedDescription` equivalent).
#[uniffi::export]
pub fn api_error_message(error: &ApiError) -> String {
    error.to_string()
}

/// Telemetry label for an error.
#[uniffi::export]
pub fn api_error_kind(error: &ApiError) -> String {
    error.kind()
}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        let code = if e.is_timeout() { -1001 } else if e.is_connect() { -1004 } else { -1 };
        ApiError::Network { message: if e.is_timeout() { "The request timed out.".into() } else if e.is_connect() { "Could not connect to the server.".into() } else { "A network error occurred.".into() }, code }
    }
}
