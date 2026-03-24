use std::fmt;
use std::io;

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidArgument,
    PermissionDenied,
    NotFound,
    UnsupportedDevice,
    UnsupportedPlatform,
    IoError,
    InternalError,
}

#[derive(Debug, Error)]
pub enum GetSmartError {
    #[error("{0}")]
    InvalidArgument(String),
    #[error("{0}")]
    PermissionDenied(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    UnsupportedDevice(String),
    #[error("{0}")]
    UnsupportedPlatform(String),
    #[error("{0}")]
    IoError(String),
    #[error("{0}")]
    Internal(String),
}

impl GetSmartError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidArgument(_) => ErrorCode::InvalidArgument,
            Self::PermissionDenied(_) => ErrorCode::PermissionDenied,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::UnsupportedDevice(_) => ErrorCode::UnsupportedDevice,
            Self::UnsupportedPlatform(_) => ErrorCode::UnsupportedPlatform,
            Self::IoError(_) => ErrorCode::IoError,
            Self::Internal(_) => ErrorCode::InternalError,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

impl From<io::Error> for GetSmartError {
    fn from(value: io::Error) -> Self {
        match value.kind() {
            io::ErrorKind::NotFound => Self::NotFound(value.to_string()),
            io::ErrorKind::PermissionDenied => Self::PermissionDenied(value.to_string()),
            _ => Self::IoError(value.to_string()),
        }
    }
}

impl From<std::str::Utf8Error> for GetSmartError {
    fn from(value: std::str::Utf8Error) -> Self {
        Self::InvalidArgument(value.to_string())
    }
}

impl From<std::ffi::NulError> for GetSmartError {
    fn from(value: std::ffi::NulError) -> Self {
        Self::Internal(value.to_string())
    }
}

impl From<time::error::Format> for GetSmartError {
    fn from(value: time::error::Format) -> Self {
        Self::Internal(value.to_string())
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::InvalidArgument => "invalid_argument",
            Self::PermissionDenied => "permission_denied",
            Self::NotFound => "not_found",
            Self::UnsupportedDevice => "unsupported_device",
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::IoError => "io_error",
            Self::InternalError => "internal_error",
        };

        f.write_str(value)
    }
}
