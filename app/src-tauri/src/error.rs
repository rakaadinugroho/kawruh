use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Msg(String),
    #[error(transparent)]
    Store(#[from] kawruh_core::store::StoreError),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error("cannot reach server: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{message}")]
    Remote {
        status: u16,
        code: String,
        message: String,
    },
    #[error(transparent)]
    Tree(#[from] kawruh_core::workspace::TreeError),
}

impl Error {
    pub fn msg(m: impl Into<String>) -> Error {
        Error::Msg(m.into())
    }

    fn code(&self) -> &str {
        match self {
            Error::Remote { code, .. } => code,
            Error::Http(_) => "unreachable",
            Error::Tree(_) => "tree",
            _ => "error",
        }
    }
}

/// Commands reject with `{ message, code }` on the JS side.
impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("Error", 2)?;
        st.serialize_field("message", &self.to_string())?;
        st.serialize_field("code", self.code())?;
        st.end()
    }
}

pub type Result<T> = std::result::Result<T, Error>;
