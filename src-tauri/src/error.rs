use std::fmt;

// Errors cross the IPC boundary as strings. Known failures use a code
// ("E_AUTH", "E_PRINT:<os message>") that the UI translates.
#[derive(Debug)]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

macro_rules! wrap {
    ($($t:ty),*) => {$(
        impl From<$t> for Error {
            fn from(e: $t) -> Self {
                Error(e.to_string())
            }
        }
    )*};
}

wrap!(rusqlite::Error, serde_json::Error, std::io::Error);

pub fn fail<T>(code: impl Into<String>) -> Result<T> {
    Err(Error(code.into()))
}
