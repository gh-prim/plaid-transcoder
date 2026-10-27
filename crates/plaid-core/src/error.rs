use std::fmt;
use std::io;

/// How far an error propagates. Drives the fallback ladder described in
/// `docs/DESIGN.md` §4: callers decide whether to skip, drop a stream, or
/// hand the whole session to the fallback engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Damaged data. Skip it and keep reading.
    Corrupt,
    /// One stream is unusable; the others can continue.
    StreamFatal,
    /// The whole session must stop (or fall back).
    SessionFatal,
    /// A configured resource limit was hit (memory cap, element size, ...).
    Resource,
    /// Valid input that we deliberately do not handle; route to the fallback.
    Unsupported,
}

#[derive(Debug)]
pub struct Error {
    class: ErrorClass,
    message: String,
    io: Option<io::Error>,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(class: ErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
            io: None,
        }
    }

    pub fn corrupt(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Corrupt, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Unsupported, message)
    }

    pub fn resource(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Resource, message)
    }

    pub fn fatal(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::SessionFatal, message)
    }

    /// A truncated input: the data ended in the middle of a structure.
    pub fn truncated(message: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Corrupt,
            message: message.into(),
            io: Some(io::ErrorKind::UnexpectedEof.into()),
        }
    }

    pub fn class(&self) -> ErrorClass {
        self.class
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// True when the input ended early. Demuxers treat this as end of stream.
    pub fn is_truncated(&self) -> bool {
        self.io
            .as_ref()
            .is_some_and(|e| e.kind() == io::ErrorKind::UnexpectedEof)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.class, self.message)?;
        if let Some(io) = &self.io {
            write!(f, " ({io})")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.io.as_ref().map(|e| e as _)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        let class = if e.kind() == io::ErrorKind::UnexpectedEof {
            ErrorClass::Corrupt
        } else {
            ErrorClass::SessionFatal
        };
        Self {
            class,
            message: "I/O error".into(),
            io: Some(e),
        }
    }
}
