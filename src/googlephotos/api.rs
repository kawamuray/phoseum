use crate::oauth::{self, TokenService};
use dirs;
use failure::{self, format_err, Fail};
use log::{debug, warn};
use reqwest;
use reqwest::Client;
use reqwest::Method;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde::Serialize;
use std::fs::File;
use std::io;
use std::path::Path;
use std::thread;
use std::time::Duration;
use url::Url;

const API_ENDPOINT: &str = "https://photoslibrary.googleapis.com";

const PATH_LIST_ALBUMS: &str = "v1/albums";
const PATH_LIST_SHARED_ALBUMS: &str = "v1/sharedAlbums";
const PATH_MEDIA_ITEMS_SEARCH: &str = "v1/mediaItems:search";

#[derive(Debug, Fail)]
pub enum Error {
    /// Error by remote server is failing
    #[fail(display = "Error response returned: {}", _0)]
    Request(#[fail(cause)] failure::Error),
    /// Authentication error that never recovers with current config
    #[fail(display = "Access unauthorized by status: {}", _0)]
    Unauthorized(u16),
    /// Error in managing OAuth token
    #[fail(display = "Error in managing OAuth token: {}", _0)]
    OAuthToken(#[fail(cause)] oauth::Error),
    /// IO error
    #[fail(display = "IO error in processing request: {}", _0)]
    IO(#[fail(cause)] io::Error),
}

impl From<oauth::Error> for Error {
    fn from(e: oauth::Error) -> Self {
        Error::OAuthToken(e)
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Request(e.into())
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::IO(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn auth_config<I: Into<String>, S: Into<String>>(
    client_id: I,
    client_secret: S,
) -> oauth::AuthConfig {
    oauth::AuthConfig {
        auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
        token_url: "https://www.googleapis.com/oauth2/v3/token".to_string(),
        scopes: vec!["https://www.googleapis.com/auth/photoslibrary.readonly".to_string()],
        client_id: client_id.into(),
        client_secret: client_secret.into(),
        token_store: dirs::home_dir()
            .expect("HOME dir is not set")
            .join(".phoseum-googleapis-secret.json"),
    }
}

pub struct RetryConfig {
    max_retries: usize,
    backoff: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        RetryConfig {
            max_retries: 3,
            backoff: Duration::from_secs(1),
        }
    }
}

pub struct GPhotosApi {
    tokens: TokenService,
    retry: RetryConfig,
    client: Client,
}

impl GPhotosApi {
    pub fn new(tokens: TokenService, retry_config: RetryConfig) -> GPhotosApi {
        GPhotosApi {
            tokens,
            retry: retry_config,
            client: reqwest::Client::new(),
        }
    }

    fn request<Req, Res>(&self, method: Method, url: &str, data: Option<&Req>) -> Result<Res>
    where
        Req: Serialize,
        Res: DeserializeOwned,
    {
        let mut retry_count = 0;
        loop {
            let access_token = self.tokens.obtain_access_token()?;

            let mut builder = self
                .client
                .request(method.clone(), url)
                .bearer_auth(access_token);
            if let Some(req) = data {
                builder = builder.json(req);
            }

            let err = match builder.send() {
                Ok(mut resp) => {
                    let status = resp.status();

                    if status.is_success() {
                        let res = resp.json()?;
                        return Ok(res);
                    }

                    if status == StatusCode::UNAUTHORIZED {
                        if let Err(e) = self.tokens.expire_current() {
                            warn!("Failed to clear local tokens: {:?}", e);
                        }
                        Error::Unauthorized(StatusCode::UNAUTHORIZED.as_u16())
                    } else if status.is_client_error() {
                        debug!("Got {} response for {}, aborting", status, url);
                        return Err(Error::Unauthorized(status.as_u16()));
                    } else {
                        Error::Request(format_err!("bad status code: {}", status))
                    }
                }
                Err(e) => {
                    if !e.is_http() && !e.is_timeout() {
                        return Err(Error::Request(e.into()));
                    }
                    Error::Request(e.into())
                }
            };

            debug!("Retrying request for {} with error: {}", url, err);
            retry_count += 1;
            if retry_count > self.retry.max_retries {
                return Err(err);
            }
            thread::sleep(self.retry.backoff)
        }
    }

    pub fn albums(&self, page_token: Option<&str>) -> Result<AlbumListResponse> {
        let mut params = Vec::with_capacity(1);
        if let Some(token) = page_token {
            params.push(("pageToken", token));
        }
        let url =
            Url::parse_with_params(&format!("{}/{}", API_ENDPOINT, PATH_LIST_ALBUMS), &params)
                .expect("url parse");

        self.request(Method::GET, url.as_str(), None as Option<&()>)
    }

    pub fn shared_albums(&self, page_token: Option<&str>) -> Result<SharedAlbumListResponse> {
        let mut params = Vec::with_capacity(1);
        if let Some(token) = page_token {
            params.push(("pageToken", token));
        }
        let url = Url::parse_with_params(
            &format!("{}/{}", API_ENDPOINT, PATH_LIST_SHARED_ALBUMS),
            &params,
        )
        .expect("url parse");

        self.request(Method::GET, url.as_str(), None as Option<&()>)
    }

    pub fn media_items_search(
        &self,
        req: &MediaItemsSearchRequest,
    ) -> Result<MediaItemsSearchResponse> {
        self.request(
            Method::POST,
            &format!("{}/{}", API_ENDPOINT, PATH_MEDIA_ITEMS_SEARCH),
            Some(req),
        )
    }

    /// Download the content of given media item and save it into specified path.
    ///
    /// This is a simple HTTP access rather than Google Photos API access,
    /// so it doesn't require oauth but putting here for ease of access.
    pub fn download_media_item(
        &self,
        dest_path: &Path,
        base_url: &str,
        is_video: bool,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let url = if is_video {
            format!("{}=dv", base_url)
        } else {
            format!("{}=w{}-h{}", base_url, width, height)
        };

        let mut resp = self.client.get(&url).send()?;
        if !resp.status().is_success() {
            return Err(Error::Request(format_err!(
                "bad status code: {}",
                resp.status()
            )));
        }

        let mut file = File::create(dest_path)?;
        io::copy(&mut resp, &mut file)?;

        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AlbumListResponse {
    pub albums: Option<Vec<Album>>,
    pub next_page_token: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SharedAlbumListResponse {
    pub shared_albums: Option<Vec<Album>>,
    pub next_page_token: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub title: Option<String>,
    pub product_url: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaItemsSearchRequest {
    pub album_id: String,
    pub page_size: Option<i64>,
    pub page_token: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaItemsSearchResponse {
    pub media_items: Vec<MediaItem>,
    pub next_page_token: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    pub id: Option<String>,
    pub description: Option<String>,
    pub product_url: Option<String>,
    pub base_url: Option<String>,
    pub mime_type: Option<String>,
    pub media_metadata: Option<MediaMetadata>,
    pub filename: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaMetadata {
    pub creation_time: Option<String>,
    pub width: Option<String>,
    pub height: Option<String>,
}
