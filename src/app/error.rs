use std::{error::Error, fmt};

#[derive(Clone, Copy)]
enum ErrorKind {
    Input,
    Operational,
}

pub(crate) struct AppError {
    kind: ErrorKind,
    message: String,
}

impl AppError {
    pub(crate) fn input(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Input,
            message: message.into(),
        }
    }

    pub(crate) fn operational(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Operational,
            message: message.into(),
        }
    }

    pub(crate) fn exit_code(&self) -> i32 {
        match self.kind {
            ErrorKind::Input => 2,
            ErrorKind::Operational => 1,
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl fmt::Debug for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for AppError {}
