use crate::player::{Player, Result, SlideshowConfig};
use elementtree::Element;
use failure::{format_err, Fail};
use libc;
use log::{debug, info, warn};
use reqwest;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::time::Duration;
use std::time::Instant;
use url::Url;

const VLC_VOLUME_MAX: u32 = 512;
const VLC_HTTP_PASSWORD: &str = "cherry";
const VLC_HTTP_HOST: &str = "localhost";
const VLC_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const VLC_STARTUP_CHECK_BACKOFF: Duration = Duration::from_millis(500);

const VLC_DEFAULT_BIN: &str = "vlc";
const VLC_DEFAULT_HTTP_PORT: u32 = 9843;

#[derive(Debug, Fail)]
pub enum VlcError {
    #[fail(display = "Player not started")]
    NotStarted,
    #[fail(display = "Timed out in waiting player to start")]
    StartTimeout,
    #[fail(display = "Failed to send request to player: {}", _0)]
    BadResponse(#[fail(cause)] failure::Error),
}

impl From<reqwest::Error> for VlcError {
    fn from(e: reqwest::Error) -> Self {
        VlcError::BadResponse(e.into())
    }
}

impl From<elementtree::Error> for VlcError {
    fn from(e: elementtree::Error) -> Self {
        VlcError::BadResponse(e.into())
    }
}

pub struct VlcConfig {
    pub vlc_bin: Option<String>,
    pub http_port: Option<u32>,
}

impl Default for VlcConfig {
    fn default() -> Self {
        VlcConfig {
            vlc_bin: None,
            http_port: None,
        }
    }
}

pub struct VlcPlayer {
    vlc_config: VlcConfig,

    config: Option<SlideshowConfig>,
    process: Option<Child>,
    client: reqwest::Client,

    pausing: bool,
    muting: bool,
}

impl VlcPlayer {
    pub fn new(config: VlcConfig) -> VlcPlayer {
        VlcPlayer {
            vlc_config: config,
            config: None,
            process: None,
            client: reqwest::Client::new(),
            pausing: false,
            muting: false,
        }
    }

    fn config(&self) -> std::result::Result<&SlideshowConfig, VlcError> {
        self.config.as_ref().ok_or(VlcError::NotStarted)
    }

    /// Convert `audio_volume` set in config into the value
    /// range used in VLC player
    fn audio_volume(&self) -> std::result::Result<u32, VlcError> {
        Ok((VLC_VOLUME_MAX as f32 * self.config()?.audio_volume).round() as u32)
    }

    fn send_get<U: AsRef<str>>(&self, url: U) -> std::result::Result<String, VlcError> {
        debug!("Sending GET to {}", url.as_ref());

        let mut resp = self
            .client
            .get(url.as_ref())
            .basic_auth("", Some(VLC_HTTP_PASSWORD))
            .send()?;
        if !resp.status().is_success() {
            return Err(VlcError::BadResponse(format_err!(
                "Bad HTTP status {} : {}",
                resp.status(),
                resp.text().unwrap_or_else(|_| "N/A".to_string())
            )));
        }

        Ok(resp.text()?)
    }

    fn http_port(&self) -> u32 {
        self.vlc_config.http_port.unwrap_or(VLC_DEFAULT_HTTP_PORT)
    }

    fn build_url(&self, path: &str) -> String {
        format!("http://{}:{}/{}", VLC_HTTP_HOST, self.http_port(), path)
    }

    fn send_status_cmd(
        &self,
        cmd: &str,
        args: &[(&str, &str)],
    ) -> std::result::Result<String, VlcError> {
        let mut params = Vec::with_capacity(args.len() + 1);
        if !cmd.is_empty() {
            params.push(("command", cmd));
            params.extend(args);
        }

        let url = Url::parse_with_params(&self.build_url("requests/status.xml"), params)
            .expect("parse vlc url");
        self.send_get(&url)
    }

    fn get_playlist(&self) -> std::result::Result<Element, VlcError> {
        let xml = self.send_get(&self.build_url("requests/playlist.xml"))?;
        debug!("Playlist XML from VLC: {}", xml);
        let element = Element::from_reader(xml.into_bytes().as_slice())?;
        Ok(element)
    }

    fn wait_on_http_interface(&self) -> std::result::Result<(), VlcError> {
        let start_time = Instant::now();

        while Instant::now() - start_time < VLC_STARTUP_TIMEOUT {
            match self.send_status_cmd("", &[]) {
                Ok(_) => return Ok(()),
                Err(e) => debug!("Still waiting VLC to boot: {}", e),
            }
            std::thread::sleep(VLC_STARTUP_CHECK_BACKOFF);
        }
        Err(VlcError::StartTimeout)
    }

    fn set_volume(&self, volume: u32) -> std::result::Result<(), VlcError> {
        info!("Setting audio volume to {}", volume);
        self.send_status_cmd("volume", &[("val", &volume.to_string())])?;
        Ok(())
    }

    fn playlist_ids(element: Element) -> std::result::Result<Vec<u64>, VlcError> {
        for node in element.find_all("node") {
            if node
                .get_attr("name")
                .map(|name| name == "Playlist")
                .unwrap_or(false)
            {
                let mut ids = Vec::new();
                for leaf in node.find_all("leaf") {
                    let id_s = leaf.get_attr("id").ok_or_else(|| {
                        VlcError::BadResponse(format_err!("missing id attribute"))
                    })?;
                    let id: u64 = id_s.parse().map_err(|_| {
                        VlcError::BadResponse(format_err!("cannot parse id: {}", id_s))
                    })?;
                    ids.push(id);
                }
                return Ok(ids);
            }
        }

        Err(VlcError::BadResponse(format_err!(
            "no playlist found in XML"
        )))
    }

    fn may_restore_pause(&self) -> std::result::Result<(), VlcError> {
        // Moving resets the pausing state
        if self.pausing {
            // Pausing before play starts causes blackscreen
            std::thread::sleep(Duration::from_secs(1));
            self.send_status_cmd("pl_pause", &[])?;
        }
        Ok(())
    }
}

impl Player for VlcPlayer {
    fn start(&mut self, config: SlideshowConfig) -> Result<()> {
        let vlc_bin = self
            .vlc_config
            .vlc_bin
            .as_ref()
            .map(|s| s.as_ref())
            .unwrap_or(VLC_DEFAULT_BIN);

        let mut cmd = Command::new(vlc_bin);
        cmd.arg("--loop")
            .arg("--no-video-title-show")
            // Don't show popup for asking whether to fetch media metadata through network
            .arg("--no-qt-privacy-ask")
            .arg("--no-qt-video-autoresize")
            // https://wiki.videolan.org/index.php/VLC_command-line_help
            .args(&[
                "--image-duration",
                &config.show_duration.as_secs().to_string(),
            ])
            .args(&["--extraintf", "http"])
            .args(&["--http-password", VLC_HTTP_PASSWORD])
            .args(&["--http-host", VLC_HTTP_HOST])
            .args(&["--http-port", &self.http_port().to_string()]);

        if config.fullscreen {
            cmd.arg("--fullscreen");
        }

        self.process = Some(cmd.spawn()?);
        self.wait_on_http_interface()?;

        self.config = Some(config);
        self.set_volume(self.audio_volume()?)?;

        Ok(())
    }

    fn play_next(&mut self) -> Result<()> {
        self.send_status_cmd("pl_next", &[])?;
        self.may_restore_pause()?;
        Ok(())
    }

    fn play_back(&mut self) -> Result<()> {
        self.send_status_cmd("pl_previous", &[])?;
        self.may_restore_pause()?;
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        if !self.pausing {
            self.send_status_cmd("pl_pause", &[])?;
        }
        self.pausing = true;
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if self.pausing {
            self.send_status_cmd("pl_play", &[])?;
        }
        self.pausing = false;
        Ok(())
    }

    fn mute(&mut self) -> Result<()> {
        if !self.muting {
            self.set_volume(0)?;
        }
        self.muting = true;
        Ok(())
    }

    fn unmute(&mut self) -> Result<()> {
        if self.muting {
            self.set_volume(self.audio_volume()?)?;
        }
        self.muting = false;
        Ok(())
    }

    fn update_playlist(&mut self, playlist: Vec<PathBuf>) -> Result<()> {
        debug!("Start updating playlist");
        // 1. get current playlist
        let old_ids = Self::playlist_ids(self.get_playlist()?)?;

        // 2. enqueue all new items
        for path in playlist {
            debug!("Adding new item to playlist: {}", path.display());
            self.send_status_cmd("in_enqueue", &[("input", path.to_str().unwrap())])?;
        }

        // 3. move to the head of new items
        let cur_ids = Self::playlist_ids(self.get_playlist()?)?;
        let head_id = cur_ids[old_ids.len()];

        debug!("Jumping to playlist ID: {}", head_id);
        self.send_status_cmd("pl_play", &[("id", &head_id.to_string())])?;
        std::thread::sleep(Duration::from_secs(1));

        // 4. Remove old items from playlist (assuming current media won't come up so soon)
        for id in old_ids {
            debug!("Removing old item from playlist: {}", id);
            self.send_status_cmd("pl_delete", &[("id", &id.to_string())])?;
        }

        debug!("Update playlist complete");
        Ok(())
    }

    fn pausing(&self) -> bool {
        self.pausing
    }
}

impl Drop for VlcPlayer {
    fn drop(&mut self) {
        if let Some(mut proc) = self.process.take() {
            // Rust's Command doesn't support other than SIGKILL in portable interface
            unsafe {
                libc::kill(proc.id() as i32, libc::SIGTERM);
            }
            match proc.wait() {
                Ok(status) => debug!("VLC process exit with {}", status.code().unwrap_or(-1)),
                Err(e) => warn!("Failed to stop VLC process gracefully: {}", e),
            }
        }
    }
}
