use crate::control::{Commander, PlayerCmd};
use failure::{self, format_err, Fail};
use gpio_cdev::{Chip, LineRequestFlags, MultiLineHandle};
use log::{debug, error, info};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Fail)]
pub enum Error {
    #[fail(display = "GPIO device error: {}", _0)]
    GpioDev(#[fail(cause)] failure::Error),
}

impl From<gpio_cdev::errors::Error> for Error {
    fn from(e: gpio_cdev::errors::Error) -> Self {
        Error::GpioDev(format_err!("{:?}", e))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct PinMap {
    /// Pin's line offset.
    offset: u32,
    /// True means on raising edge. Otherwse on falling edge.
    edge_high: bool,
    /// Default state
    default_high: bool,
    /// Command to execute.
    cmd: PlayerCmd,
}

impl PinMap {
    pub fn new(offset: u32, edge_high: bool, default_high: bool, cmd: PlayerCmd) -> Self {
        PinMap {
            offset,
            edge_high,
            default_high,
            cmd,
        }
    }
}

pub struct GpioCommander {
    pin_mapping: HashMap<(u32, bool), PlayerCmd>,
    offsets: Vec<u32>,
    pin_state: Vec<u8>,
    lines_handle: MultiLineHandle,
}

impl GpioCommander {
    pub fn create<P: AsRef<Path>>(dev_path: P, pin_mapping: Vec<PinMap>) -> Result<Self> {
        let mut pinmap = HashMap::new();
        let mut default_states = HashMap::new();
        for map in pin_mapping {
            pinmap.insert((map.offset, map.edge_high), map.cmd);
            default_states.insert(map.offset, map.default_high);
        }
        let offsets: Vec<_> = pinmap
            .keys()
            .map(|(off, _)| *off)
            // Make distinct list of line offsets
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        info!(
            "Opening GPIO {:?} for offsets {:?}",
            dev_path.as_ref(),
            offsets
        );
        let mut chip = Chip::new(dev_path.as_ref())?;
        let lines = chip.get_lines(&offsets)?;
        // Despite of the document says the `default` argument is used to supply
        // default values for `OUTPUT` pins, it returns error when the length isn't
        // consistent to the number of lines even for `INPUT`.
        let defaults = vec![0; offsets.len()];
        let lines_handle = lines.request(LineRequestFlags::INPUT, &defaults, "phoseum")?;

        let pin_state = offsets
            .iter()
            .map(|off| if default_states[off] { 1 } else { 0 })
            .collect();
        debug!(
            "Initial GPIO pins state: offsets={:?}, states={:?}",
            offsets, pin_state
        );
        Ok(GpioCommander {
            pin_mapping: pinmap,
            offsets,
            pin_state,
            lines_handle,
        })
    }
}

impl Commander<PlayerCmd> for GpioCommander {
    fn run(&mut self, sender: mpsc::Sender<PlayerCmd>, terminate: Arc<AtomicBool>) {
        while !terminate.load(Ordering::Relaxed) {
            let inputs = match self.lines_handle.get_values() {
                Ok(inputs) => inputs,
                Err(e) => {
                    error!("Failed reading GPIO input: {}", e);
                    continue;
                }
            };

            for (i, current) in inputs.into_iter().enumerate() {
                let prev = self.pin_state[i];
                if current == prev {
                    continue;
                }
                let key = (self.offsets[i], current > prev);
                debug!("Detect GPIO event: {:?}", key);
                if let Some(cmd) = self.pin_mapping.get(&key) {
                    if let Err(e) = sender.send(*cmd) {
                        debug!("Breaking out loop facing error: {:?}", e);
                        break;
                    }
                }
                self.pin_state[i] = current;
            }

            std::thread::sleep(POLL_INTERVAL);
        }
    }
}
