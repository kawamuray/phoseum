use crate::control::{Commander, PlayerCmd};
use log::{debug, info, warn};
use std::io;
use std::io::BufRead;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;

#[derive(Default)]
pub struct ConsoleCommander {}

impl Commander<PlayerCmd> for ConsoleCommander {
    fn run_and_forget(&self) -> bool {
        true
    }

    fn run(&mut self, sender: mpsc::Sender<PlayerCmd>, _terminate: Arc<AtomicBool>) {
        let stdin = io::stdin();
        let mut lines = stdin.lock().lines();
        loop {
            eprint!("cmd> ");
            let line = if let Some(line) = lines.next() {
                line
            } else {
                info!("Console - EOF");
                break;
            };
            let cmd_name = line.expect("stdin");
            let cmd = if let Some(cmd) = PlayerCmd::from_name(&cmd_name) {
                cmd
            } else {
                warn!("Unknown command: {}", cmd_name);
                continue;
            };
            if let Err(e) = sender.send(cmd) {
                debug!("Breaking out loop facing error: {:?}", e);
                break;
            }
        }
    }
}
