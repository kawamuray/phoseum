use dirs;
use failure::Fail;
use log::debug;
use serde::Deserialize;
use serde::Serialize;
use serde_json;
use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;

#[derive(Debug, Fail)]
pub enum Error {
    #[fail(display = "IO error in secret store: {}", _0)]
    IO(#[fail(cause)] io::Error),
    #[fail(display = "Error in secret serialization: {}", _0)]
    Serde(#[fail(cause)] serde_json::Error),
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Serde(e)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::IO(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn default_store_path() -> PathBuf {
    dirs::home_dir()
        .expect("HOME dir is not set")
        .join(".phoseum-googleapis-secret.json")
}

pub struct TokenStore {
    path: PathBuf,
    entry: RefCell<StoreEntry>,
}

impl TokenStore {
    pub fn open<T: Into<PathBuf>>(path: T) -> Result<TokenStore> {
        let path = path.into();

        let entry = match fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json)?,
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Fallback to initial creation
                StoreEntry {
                    access_token: None,
                    refresh_token: None,
                }
            }
            Err(e) => return Err(Error::IO(e)),
        };

        Ok(TokenStore {
            path,
            entry: RefCell::new(entry),
        })
    }

    fn current_time() -> Duration {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("error getting system clock")
    }

    pub fn save(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.entry)?;
        std::fs::write(&self.path, json)?;
        Ok(())
    }

    pub fn valid_access_token(&self) -> Option<String> {
        if let Some(token) = &self.entry.borrow().access_token {
            if token.expire_date.unwrap_or(std::u128::MAX) > Self::current_time().as_millis() {
                return Some(token.secret.clone());
            }
        }
        None
    }

    pub fn refresh_token(&self) -> Option<String> {
        self.entry
            .borrow()
            .refresh_token
            .as_ref()
            .map(|t| t.secret.clone())
    }

    pub fn update_access_token(
        &self,
        access_token: Option<String>,
        expires_in: Option<u128>,
    ) -> Result<()> {
        self.update_tokens(access_token, expires_in, self.refresh_token())
    }

    pub fn update_tokens(
        &self,
        access_token: Option<String>,
        expires_in: Option<u128>,
        refresh_token: Option<String>,
    ) -> Result<()> {
        let now = Self::current_time().as_millis();
        let expire_date = expires_in.map(|t| now + t);

        self.entry.borrow_mut().access_token = access_token.map(|t| Token {
            secret: t,
            created_date: now,
            expire_date,
        });
        self.entry.borrow_mut().refresh_token = refresh_token.map(|t| Token {
            secret: t,
            created_date: Self::current_time().as_millis(),
            expire_date: None,
        });

        debug!(
            "Local tokens {:?} updated to expire at {:?}",
            self.path, expire_date
        );

        self.save()
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct StoreEntry {
    access_token: Option<Token>,
    refresh_token: Option<Token>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Token {
    secret: String,
    created_date: u128,
    expire_date: Option<u128>,
}
