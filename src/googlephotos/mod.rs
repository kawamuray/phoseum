pub mod api;

use crate::album::{self, Album, AlbumItem, MediaType};
use crate::oauth::TokenService;
use api::{GPhotosApi, MediaItem, MediaItemsSearchRequest, RetryConfig};
use chrono::DateTime;
use failure::{self, Fail};
use log::debug;
use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::SystemTime;

const MEDIA_ITEMS_SEARCH_PAGE_SIZE: i64 = 100;
const PHOTO_WIDTH: u32 = 1280;
const PHOTO_HEIGHT: u32 = 800;

pub fn new_gphotos_album<S: Into<String>>(album_id: S, tokens: TokenService) -> GPhotosAlbum {
    let api = GPhotosApi::new(tokens, RetryConfig::default());
    GPhotosAlbum::new(album_id, api)
}

#[derive(Debug, Fail)]
pub enum Error {
    /// Metadata returned from Google Photos API was broken or unexpected
    #[fail(display = "Metadata corrupted: {}", _0)]
    CorruptedMetadata(String),
    /// Returned media type is not known (so should be skipped)
    #[fail(display = "Unknown media type: {:?}", media_type)]
    UnknownMediaType { media_type: Option<String> },
    /// Any IO failure that indicates permanent fail not likely to recover
    #[fail(display = "IO error {}", _0)]
    IO(#[fail(cause)] io::Error),
    /// Remote API failure
    #[fail(display = "Remote endpoint failed: {}", _0)]
    RemoteFail(#[fail(cause)] api::Error),
    /// Invalid configuration
    #[fail(display = "Invalid auth configuration: {}", _0)]
    InvalidAuthConfig(#[fail(cause)] failure::Error),
}

impl album::Error for Error {
    fn is_fatal(&self) -> bool {
        match self {
            Error::IO(_) | Error::InvalidAuthConfig(_) => true,
            Error::RemoteFail(_) | Error::CorruptedMetadata(_) | Error::UnknownMediaType { .. } => {
                false
            }
        }
    }
}

impl From<api::Error> for Error {
    fn from(e: api::Error) -> Self {
        match e {
            api::Error::IO(e) => Error::IO(e),
            e @ api::Error::Request(_) => Error::RemoteFail(e),
            e @ api::Error::Unauthorized(_) => Error::InvalidAuthConfig(e.into()),
            e @ api::Error::OAuthToken(_) => Error::InvalidAuthConfig(e.into()),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct GPhotosAlbum {
    album_id: Rc<String>,
    api: Rc<GPhotosApi>,
}

impl GPhotosAlbum {
    pub fn new<S: Into<String>>(album_id: S, api: GPhotosApi) -> GPhotosAlbum {
        GPhotosAlbum {
            album_id: Rc::new(album_id.into()),
            api: Rc::new(api),
        }
    }
}

impl Album for GPhotosAlbum {
    type E = Error;
    type Item = GPhotosAlbumItem;
    type Items = GPhotosAlbumItems;

    fn items(&self) -> Self::Items {
        GPhotosAlbumItems {
            api: Rc::clone(&self.api),
            album_id: Rc::clone(&self.album_id),
            cur_batch: VecDeque::new(),
            next_token: None,
            end_of_stream: false,
        }
    }

    fn prepare_item<P: AsRef<Path>>(&self, item: &Self::Item, path: P) -> Result<()> {
        let is_video = item.media_type() == MediaType::VIDEO;
        self.api.download_media_item(
            path.as_ref(),
            &item.mitem.base_url.as_ref().expect("base_url is missing"),
            is_video,
            PHOTO_WIDTH,
            PHOTO_HEIGHT,
        )?;

        Ok(())
    }
}

pub struct GPhotosAlbumItems {
    api: Rc<GPhotosApi>,
    album_id: Rc<String>,
    cur_batch: VecDeque<MediaItem>,
    next_token: Option<String>,
    end_of_stream: bool,
}

impl Iterator for GPhotosAlbumItems {
    type Item = Result<GPhotosAlbumItem>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.end_of_stream && self.cur_batch.is_empty() {
            let req = MediaItemsSearchRequest {
                album_id: self.album_id.to_string(),
                page_size: Some(MEDIA_ITEMS_SEARCH_PAGE_SIZE),
                page_token: self.next_token.take(),
            };
            let resp = match self.api.media_items_search(&req) {
                Ok(resp) => resp,
                Err(e) => {
                    self.end_of_stream = true;
                    return Some(Err(e.into()));
                }
            };
            self.cur_batch.extend(resp.media_items);
            self.next_token = resp.next_page_token;
            self.end_of_stream = self.next_token.is_none();
        }

        self.cur_batch.pop_front().map(GPhotosAlbumItem::new)
    }
}

#[derive(Eq, PartialEq, Debug)]
pub struct GPhotosAlbumItem {
    path: PathBuf,
    media_type: MediaType,
    created_time: SystemTime,
    mitem: MediaItem,
}

impl GPhotosAlbumItem {
    fn new(mitem: MediaItem) -> Result<GPhotosAlbumItem> {
        let (media_type, file_ext) =
            Self::media_info(&mitem).ok_or_else(|| Error::UnknownMediaType {
                media_type: mitem.mime_type.clone(),
            })?;
        let id = mitem
            .id
            .as_ref()
            .ok_or_else(|| Error::CorruptedMetadata("missing id".to_string()))?;
        let created_time = mitem
            .media_metadata
            .as_ref()
            .and_then(|meta| meta.creation_time.as_ref())
            .ok_or_else(|| Error::CorruptedMetadata("missing creation_time".to_string()))?;
        let created_time = DateTime::parse_from_rfc3339(&created_time).map_err(|e| {
            Error::CorruptedMetadata(format!("invalid creation_time {}: {}", created_time, e))
        })?;

        let path = PathBuf::from(format!("{}.{}", id, file_ext));

        Ok(GPhotosAlbumItem {
            path,
            media_type,
            created_time: created_time.into(),
            mitem,
        })
    }

    /// Return media information from its MIME type
    ///
    /// The return type is (MediaType, FILE_EXTENSION)
    fn media_info(mitem: &MediaItem) -> Option<(MediaType, &'static str)> {
        mitem.mime_type.as_ref().and_then(|mt| match mt.as_ref() {
            "image/jpeg" => Some((MediaType::PHOTO, "jpg")),
            "image/png" => Some((MediaType::PHOTO, "png")),
            "image/apng" => Some((MediaType::PHOTO, "apng")),
            "image/gif" => Some((MediaType::PHOTO, "gif")),
            "image/svg+xml" => Some((MediaType::PHOTO, "svg")),
            "image/heif" => Some((MediaType::PHOTO, "heif")),
            "video/webm" => Some((MediaType::VIDEO, "webm")),
            "video/ogg" => Some((MediaType::VIDEO, "ogg")),
            "video/mp4" => Some((MediaType::VIDEO, "mp4")),
            _ => {
                debug!("Unknown MIME for {:?}: {}", mitem.id, mt);
                None
            }
        })
    }
}

impl AlbumItem for GPhotosAlbumItem {
    fn id(&self) -> &str {
        &self.mitem.id.as_ref().expect("mitem.id")
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn media_type(&self) -> MediaType {
        self.media_type
    }

    fn created_time(&self) -> SystemTime {
        self.created_time
    }
}
