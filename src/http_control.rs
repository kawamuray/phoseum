use crate::control::{Commander, PlaylistCmd};
use rouille;
use rouille::router;
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;

pub struct HttpCommander {
    http_port: u32,
}

impl HttpCommander {
    pub fn new(http_port: u32) -> Self {
        HttpCommander { http_port }
    }
}

impl Commander<PlaylistCmd> for HttpCommander {
    fn run_and_forget(&self) -> bool {
        true
    }

    fn run(&mut self, send: mpsc::Sender<PlaylistCmd>, _: Arc<AtomicBool>) {
        let sender = Mutex::new(send);

        let listen_addr = format!("localhost:{}", self.http_port);
        rouille::start_server(listen_addr, move |request| {
            let sender = sender.lock().unwrap().clone();
            rouille::log(&request, io::stdout(), || {
                router!(
                    request,
                    (POST) (/playlist/update) => {
                        sender.send(PlaylistCmd::Update).expect("playlist update");
                        rouille::Response::text("Update requested")
                    },
                    (POST) (/playlist/refresh) => {
                        sender.send(PlaylistCmd::Refresh).expect("playlist refresh");
                        rouille::Response::text("Refresh requested")
                    },
                    _ => rouille::Response::empty_404()
                )
            })
        });
    }
}
