pub mod album;
pub mod console_control;
pub mod control;
pub mod googlephotos;
pub mod gpio_control;
pub mod http_control;
pub mod oauth;
pub mod player;
pub mod player_vlc;
pub mod playlist;
pub mod slideshow;
pub mod storage;

use album::Album;
use control::{Commander, PlayerCmd, PlaylistCmd};
use log::{debug, error, info};
use player::Player;
use slideshow::Slideshow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const POLL_TIMEOUT: Duration = Duration::from_millis(300);

pub struct Phoseum<P: Player + Send + 'static, A: Album> {
    slideshow: Slideshow<P, A>,
    pl_commanders: Vec<Box<dyn Commander<PlaylistCmd> + Send + 'static>>,
    player_commanders: Vec<Box<dyn Commander<PlayerCmd> + Send + 'static>>,
}

impl<P: Player + Send + 'static, A: Album> Phoseum<P, A> {
    pub fn new(slideshow: Slideshow<P, A>) -> Self {
        Self {
            slideshow,
            pl_commanders: Vec::new(),
            player_commanders: Vec::new(),
        }
    }

    pub fn add_playlist_commander<C>(&mut self, commander: C)
    where
        C: Commander<PlaylistCmd> + Send + 'static,
    {
        self.pl_commanders.push(Box::new(commander));
    }

    pub fn add_player_commander<C>(&mut self, commander: C)
    where
        C: Commander<PlayerCmd> + Send + 'static,
    {
        self.player_commanders.push(Box::new(commander));
    }

    pub fn run(mut self, terminate: Arc<AtomicBool>) -> Result<(), slideshow::Error> {
        self.slideshow.start()?;

        let mut threads = Vec::new();

        let (player_send, player_recv) = mpsc::channel();
        for mut commander in self.player_commanders {
            let forget = commander.run_and_forget();
            let send_copy = player_send.clone();
            let term_copy = Arc::clone(&terminate);
            let th = thread::spawn(move || {
                commander.run(send_copy, term_copy);
            });
            if !forget {
                threads.push(th);
            }
        }
        // Handle commands for players in separate threads to keep it responsive
        // even if heavy playlist commands such as refresh is being processed.
        let term_copy = Arc::clone(&terminate);
        let player = self.slideshow.player();
        let th = thread::spawn(move || {
            while !term_copy.load(Ordering::Relaxed) {
                match player_recv.recv_timeout(POLL_TIMEOUT) {
                    Ok(cmd) => {
                        if let Err(e) = control::handle_player_cmd(
                            &mut *player.lock().expect("lock player"),
                            cmd,
                        ) {
                            error!("Error handling player command {:?}: {:?}", cmd, e);
                        }
                    }
                    Err(e) => match e {
                        mpsc::RecvTimeoutError::Timeout => {}
                        _ => {
                            debug!("Player command sender closed, breaking out loop");
                            break;
                        }
                    },
                }
            }
        });
        threads.push(th);

        let (pl_send, pl_recv) = mpsc::channel();
        for mut commander in self.pl_commanders {
            let forget = commander.run_and_forget();
            let send_copy = pl_send.clone();
            let term_copy = Arc::clone(&terminate);
            let th = thread::spawn(move || {
                commander.run(send_copy, term_copy);
            });
            if !forget {
                threads.push(th);
            }
        }

        while !terminate.load(Ordering::Relaxed) {
            match pl_recv.recv_timeout(POLL_TIMEOUT) {
                Ok(cmd) => {
                    if let Err(e) = control::handle_playlist_cmd(&mut self.slideshow, cmd) {
                        error!("Error handling playlist command {:?}: {:?}", cmd, e);
                    }
                }
                Err(e) => match e {
                    mpsc::RecvTimeoutError::Timeout => {}
                    _ => {
                        debug!("Playlist command sender closed, breaking out loop");
                        break;
                    }
                },
            }
        }

        info!("Waiting all threads to terminate...");
        for th in threads {
            th.join().expect("thread join");
        }

        Ok(())
    }
}
