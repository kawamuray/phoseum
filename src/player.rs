use failure::Error;
use std::path::PathBuf;
use std::time::Duration;

pub struct SlideshowConfig {
    /// Duration to keep showing single photo
    pub show_duration: Duration,
    /// Fullscreen mode. On by default and disabled only for debugging
    pub fullscreen: bool,
    /// Audio volume in percent when playing videos
    pub audio_volume: f32,
}

impl Default for SlideshowConfig {
    fn default() -> Self {
        SlideshowConfig {
            show_duration: Duration::from_secs(10),
            fullscreen: true,
            audio_volume: 0.5,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub trait Player {
    /// Launch player
    ///
    /// At the time this method returns, player must be ready to start slideshow
    /// immediately by upcoming `update_playlist` call.
    fn start(&mut self, config: SlideshowConfig) -> Result<()>;
    /// Move to the next item in the playlist
    fn play_next(&mut self) -> Result<()>;
    /// Move to the previous item in the playlist
    fn play_back(&mut self) -> Result<()>;
    /// Pause slideshow
    ///
    /// If photo is currently displayed, it should stop sliding to the next one.
    /// If video is currently playing, it should pause its play.
    fn pause(&mut self) -> Result<()>;
    /// Resume slideshow
    fn resume(&mut self) -> Result<()>;
    /// Mute volume
    fn mute(&mut self) -> Result<()>;
    /// Unmute volume
    fn unmute(&mut self) -> Result<()>;
    /// Update by replacing the current playlist with newly given playlist
    fn update_playlist(&mut self, playlist: Vec<PathBuf>) -> Result<()>;
    /// Return whether the player is pausing
    fn pausing(&self) -> bool;
}
