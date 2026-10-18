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

// anything the code did not anticipate still arrives as a code the UI can
// name, with the system's own words kept as the detail
macro_rules! wrap {
    ($($t:ty => $code:literal),*) => {$(
        impl From<$t> for Error {
            fn from(e: $t) -> Self {
                Error(format!("{}:{e}", $code))
            }
        }
    )*};
}

wrap!(rusqlite::Error => "E_DB", serde_json::Error => "E_INTERNAL", std::io::Error => "E_IO", zip::result::ZipError => "E_IO");

pub fn fail<T>(code: impl Into<String>) -> Result<T> {
    Err(Error(code.into()))
}
