use crate::control::{Commander, PlayerCmd, PlaylistCmd};
use rouille;
use rouille::router;
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct HttpCommander {
    http_port: u32,
    playlist_sender: Arc<Mutex<Option<mpsc::Sender<PlaylistCmd>>>>,
    player_sender: Arc<Mutex<Option<mpsc::Sender<PlayerCmd>>>>,
    started: bool,
}

impl HttpCommander {
    pub fn new(http_port: u32) -> Self {
        Self {
            http_port,
            playlist_sender: Arc::new(Mutex::new(None)),
            player_sender: Arc::new(Mutex::new(None)),
            started: false,
        }
    }

    fn with_sender<C, F>(sender: &Mutex<Option<mpsc::Sender<C>>>, handler: F) -> rouille::Response
    where
        F: Fn(&mpsc::Sender<C>) -> rouille::Response,
    {
        if let Some(sender_locked) = sender.lock().expect("lock sender").as_ref() {
            handler(sender_locked)
        } else {
            rouille::Response::empty_404()
        }
    }

    fn run(&mut self) {
        if self.started {
            return;
        }
        self.started = true;

        let listen_addr = format!("localhost:{}", self.http_port);
        let playlist_sender = Arc::clone(&self.playlist_sender);
        let player_sender = Arc::clone(&self.player_sender);

        rouille::start_server(listen_addr, move |request| {
            rouille::log(&request, io::stdout(), || {
                router!(
                    request,
                    // Playlist commands
                    (POST) (/playlist/update) => {
                        Self::with_sender(&playlist_sender, |sender| {
                            sender.send(PlaylistCmd::Update).expect("Sender::send playlist");
                            rouille::Response::text("Update requested")
                        })
                    },
                    (POST) (/playlist/refresh) => {
                        Self::with_sender(&playlist_sender, |sender| {
                        sender.send(PlaylistCmd::Refresh).expect("Sender::send playlist");
                            rouille::Response::text("Refresh requested")
                        })
                    },
                    // Player commands
                    (POST) (/player/pause) => {
                        Self::with_sender(&player_sender, |sender| {
                            sender.send(PlayerCmd::Pause).expect("Sender::send player");
                            rouille::Response::text("Player paused")
                        })
                    },
                    (POST) (/player/resume) => {
                        Self::with_sender(&player_sender, |sender| {
                            sender.send(PlayerCmd::Resume).expect("Sender::send player");
                            rouille::Response::text("Player resumed")
                        })
                    },
                    _ => rouille::Response::empty_404()
                )
            })
        });
    }
}

impl Commander<PlaylistCmd> for HttpCommander {
    fn run_and_forget(&self) -> bool {
        true
    }

    fn run(&mut self, sender: mpsc::Sender<PlaylistCmd>, _: Arc<AtomicBool>) {
        self.playlist_sender
            .lock()
            .expect("lock sender")
            .replace(sender);
        self.run();
    }
}

impl Commander<PlayerCmd> for HttpCommander {
    fn run_and_forget(&self) -> bool {
        true
    }

    fn run(&mut self, sender: mpsc::Sender<PlayerCmd>, _: Arc<AtomicBool>) {
        self.player_sender
            .lock()
            .expect("lock sender")
            .replace(sender);
        self.run();
    }
}
