use crate::album::{Album, AlbumItem};
use crate::player::Player;
use crate::player::SlideshowConfig;
use crate::playlist::PlaylistBuilder;
use crate::storage::Storage;
pub use failure::Error;
use log::{debug, error, info, warn};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const TMPFILE_NAME: &str = ".downloading.tmp";

pub type Result<T> = std::result::Result<T, Error>;

pub struct Slideshow<P: Player, A: Album> {
    album: A,
    player: Arc<Mutex<P>>,
    pl_builder: PlaylistBuilder,
    storage: Storage,
    config: Option<SlideshowConfig>,
    playlist: Option<Vec<A::Item>>,
}

impl<P: Player, A: Album> Slideshow<P, A> {
    pub fn new(
        album: A,
        player: P,
        pl_builder: PlaylistBuilder,
        storage: Storage,
        slideshow_config: SlideshowConfig,
    ) -> Self {
        Slideshow {
            album,
            player: Arc::new(Mutex::new(player)),
            pl_builder,
            storage,
            config: Some(slideshow_config),
            playlist: None,
        }
    }

    pub fn start(&mut self) -> Result<()> {
        if let Some(config) = self.config.take() {
            self.player.lock().expect("lock player").start(config)?;
            self.refresh_playlist()?;
        }
        Ok(())
    }

    fn prepare_items(&mut self, playlist: &[A::Item]) -> Result<Vec<PathBuf>> {
        info!("Preparing {} items locally", playlist.len());

        let reserved_paths: HashSet<_> = playlist.iter().map(|item| item.path()).collect();
        let tmpfile = self.storage.filepath(TMPFILE_NAME).expect("filepath");
        let mut paths = Vec::with_capacity(playlist.len());
        for item in playlist {
            let path = self.storage.filepath(item.path())?;
            // Error handling rule:
            // * album.prepare_item => return error because it could make all items in list to fail prepare
            // * fs::* : io::Error => return error because they are not supposed to happen in normal situation
            // * storage.acquire : io::Error => same as the above
            // * storage.acquire failure => skip because other smaller size media might succeeds to acquire
            let size = if path.exists() {
                debug!(
                    "Media already exists, skipping download: {}",
                    item.path().display()
                );
                fs::metadata(&path)?.len()
            } else {
                info!("Downloading {}", item.path().display());
                self.album.prepare_item(&item, &tmpfile)?;
                fs::metadata(&tmpfile)?.len()
            };

            if !self.storage.acquire(item.path(), size, &reserved_paths)? {
                warn!(
                    "Failed to acquire storage for media: {}",
                    item.path().display()
                );
                continue;
            }

            if !path.exists() {
                fs::rename(&tmpfile, &path)?;
            }
            paths.push(path);
        }

        Ok(paths)
    }

    fn replace_playlist(&mut self, playlist: Vec<A::Item>) -> Result<()> {
        let paths = self.prepare_items(&playlist)?;

        if paths.is_empty() {
            info!("Not updating playlist because it has no items");
            return Ok(());
        }

        let mut player = self.player.lock().unwrap();
        if player.pausing() {
            info!("Player is pausing, not replacing playlist");
            return Ok(());
        }

        info!("Updating playlist on player...");
        player.update_playlist(paths)?;

        if let Some(old_playlist) = self.playlist.replace(playlist) {
            for item in old_playlist {
                if let Err(e) = self.storage.release(item.path()) {
                    error!(
                        "Failed to release {} from storage: {:?}",
                        item.path().display(),
                        e
                    );
                }
            }
        }
        info!("Finish updating playlist");
        Ok(())
    }

    pub fn refresh_playlist(&mut self) -> Result<()> {
        info!("Start refreshing playlist");
        if self.player.lock().unwrap().pausing() {
            info!("Player is pausing, not refreshing playlist");
            return Ok(());
        }
        let playlist = self.pl_builder.build(&self.album)?;
        self.replace_playlist(playlist)?;
        Ok(())
    }

    pub fn update_playlist(&mut self) -> Result<()> {
        info!(
            "Start updating playlist, currently {} items",
            self.playlist.as_ref().map(|p| p.len()).unwrap_or(0)
        );
        if self.player.lock().unwrap().pausing() {
            info!("Player is pausing, not updating playlist");
            return Ok(());
        }
        if let Some(new_pl) = self
            .pl_builder
            .updated(&self.album, self.playlist.as_ref().expect("playlist"))?
        {
            info!("Playlist updated, new list contains {} items", new_pl.len());
            self.replace_playlist(new_pl)?;
        } else {
            info!("No new updates for playlist");
        }
        Ok(())
    }

    pub fn player(&mut self) -> Arc<Mutex<P>> {
        Arc::clone(&self.player)
    }

    pub fn is_player_ok(&self) -> bool {
        self.player.lock().is_ok()
    }
}
