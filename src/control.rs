use crate::album::Album;
use crate::player::Player;
use crate::slideshow::{self, Slideshow};
use failure::Error;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub enum PlaylistCmd {
    /// Check and add new items in album into the current playlist
    Update,
    /// Regenerate playlist and replace the current one
    Refresh,
}

#[derive(Debug, Clone, Copy)]
pub enum PlayerCmd {
    /// Play next item in the playlist
    PlayNext,
    /// Play previous item in the playlist
    PlayBack,
    /// Pause sliding photo or playing video
    Pause,
    /// Resume sliding photo or playing video
    Resume,
    /// Mute video volume
    Mute,
    /// Unmute video volume
    Unmute,
}

impl PlayerCmd {
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "play_next" => Some(Self::PlayNext),
            "play_back" => Some(Self::PlayBack),
            "pause" => Some(Self::Pause),
            "resume" => Some(Self::Resume),
            "mute" => Some(Self::Mute),
            "unmute" => Some(Self::Unmute),
            _ => None,
        }
    }
}

pub fn handle_playlist_cmd<P: Player, A: Album>(
    slideshow: &mut Slideshow<P, A>,
    cmd: PlaylistCmd,
) -> Result<(), slideshow::Error> {
    match cmd {
        PlaylistCmd::Update => slideshow.update_playlist(),
        PlaylistCmd::Refresh => slideshow.refresh_playlist(),
    }
}

pub fn handle_player_cmd<P: Player>(player: &mut P, cmd: PlayerCmd) -> Result<(), Error> {
    match cmd {
        PlayerCmd::PlayNext => player.play_next(),
        PlayerCmd::PlayBack => player.play_back(),
        PlayerCmd::Pause => player.pause(),
        PlayerCmd::Resume => player.resume(),
        PlayerCmd::Mute => player.mute(),
        PlayerCmd::Unmute => player.unmute(),
    }
}

pub trait Commander<C> {
    /// Return if this commander's run() method terminates and returns by given flag
    fn run_and_forget(&self) -> bool {
        false
    }
    /// Loop over inputs and send commands through passed channel
    fn run(&mut self, sender: mpsc::Sender<C>, terminate: Arc<AtomicBool>);
}
