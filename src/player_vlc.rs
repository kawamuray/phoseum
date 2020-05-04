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
const VLC_REQUEST_TIMEOUT: u64 = 30;

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

pub trait HttpClient {
    fn send_get(
        &self,
        port: u32,
        path: &str,
        params: &[(&str, &str)],
    ) -> std::result::Result<String, VlcError>;
}

pub struct ReqwestClient(reqwest::Client);

impl HttpClient for ReqwestClient {
    fn send_get(
        &self,
        port: u32,
        path: &str,
        params: &[(&str, &str)],
    ) -> std::result::Result<String, VlcError> {
        let url = Url::parse_with_params(
            &format!("http://{}:{}/{}", VLC_HTTP_HOST, port, path),
            params,
        )
        .expect("parse vlc url");

        debug!("Sending GET to {}", url);

        let mut resp = self
            .0
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
}

pub struct VlcPlayer<C: HttpClient = ReqwestClient> {
    vlc_config: VlcConfig,

    config: Option<SlideshowConfig>,
    process: Option<Child>,
    client: C,

    pausing: bool,
    sleeping: bool,
    muting: bool,
}

impl VlcPlayer {
    pub fn new(config: VlcConfig) -> Self {
        Self::new_with_client(
            config,
            ReqwestClient(
                reqwest::Client::builder()
                    .timeout(Some(Duration::from_secs(VLC_REQUEST_TIMEOUT)))
                    .build()
                    .expect("reqwest client"),
            ),
        )
    }
}

impl<C: HttpClient> VlcPlayer<C> {
    fn new_with_client(config: VlcConfig, client: C) -> Self {
        Self {
            vlc_config: config,
            config: None,
            process: None,
            client,
            pausing: false,
            sleeping: false,
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

    fn http_port(&self) -> u32 {
        self.vlc_config.http_port.unwrap_or(VLC_DEFAULT_HTTP_PORT)
    }

    fn send_get(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> std::result::Result<String, VlcError> {
        self.client.send_get(self.http_port(), path, params)
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

        self.send_get("requests/status.xml", &params)
    }

    fn get_playlist(&self) -> std::result::Result<Element, VlcError> {
        let xml = self.send_get("requests/playlist.xml", &[])?;
        debug!("Playlist XML from VLC: {}", xml);
        let element = Element::from_reader(xml.into_bytes().as_slice())?;
        Ok(element)
    }

    fn wait_on_http_interface(&self) -> std::result::Result<(), VlcError> {
        let start_time = Instant::now();

        while Instant::now() - start_time < VLC_STARTUP_TIMEOUT {
            if self.is_ok() {
                return Ok(());
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

    fn maybe_restore_pause(&self) -> std::result::Result<(), VlcError> {
        // Moving resets the pausing state
        if self.pausing {
            // Pausing before play starts causes blackscreen
            std::thread::sleep(Duration::from_secs(1));
            self.send_status_cmd("pl_pause", &[])?;
        }
        Ok(())
    }

    fn maybe_pause(&self) -> std::result::Result<(), VlcError> {
        if !self.pausing && !self.sleeping {
            self.send_status_cmd("pl_pause", &[])?;
        }
        Ok(())
    }

    fn maybe_resume(&mut self, resume: bool) -> std::result::Result<(), VlcError> {
        if (self.pausing && resume) || (self.sleeping && !self.pausing) {
            self.send_status_cmd("pl_play", &[])?;
            self.pausing = false;
            self.sleeping = false;
        }
        Ok(())
    }
}

impl<C: HttpClient> Player for VlcPlayer<C> {
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
        self.maybe_restore_pause()?;
        Ok(())
    }

    fn play_back(&mut self) -> Result<()> {
        self.send_status_cmd("pl_previous", &[])?;
        self.maybe_restore_pause()?;
        Ok(())
    }

    fn sleep(&mut self) -> Result<()> {
        self.maybe_pause()?;
        self.sleeping = true;
        Ok(())
    }

    fn wakeup(&mut self) -> Result<()> {
        self.maybe_resume(false)?;
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        self.maybe_pause()?;
        self.pausing = true;
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        self.maybe_resume(true)?;
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

    fn locked(&self) -> bool {
        self.pausing || self.sleeping
    }

    fn is_ok(&self) -> bool {
        match self.send_status_cmd("", &[]) {
            Ok(_) => true,
            Err(e) => {
                debug!("Got error response while checking health of VLC: {}", e);
                false
            }
        }
    }
}

impl<C: HttpClient> Drop for VlcPlayer<C> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use tempfile;

    impl<F: Fn(&str, &HashMap<&str, &str>) -> std::result::Result<String, VlcError>> HttpClient for F {
        fn send_get(
            &self,
            _port: u32,
            path: &str,
            params: &[(&str, &str)],
        ) -> std::result::Result<String, VlcError> {
            self(path, &params.into_iter().map(|v| *v).collect())
        }
    }

    fn dummy_bin_player<
        C: Fn(&str, &HashMap<&str, &str>) -> std::result::Result<String, VlcError>,
    >(
        client: C,
    ) -> (tempfile::NamedTempFile, VlcPlayer<C>) {
        let mut dummy_bin = tempfile::NamedTempFile::new().unwrap();
        let file = dummy_bin.as_file_mut();
        writeln!(file, "#!/bin/sh").unwrap();
        writeln!(file, "sleep 60").unwrap();
        file.flush().unwrap();
        let mut perm = file.metadata().expect("metadata").permissions();
        perm.set_mode(0o775);
        fs::set_permissions(&dummy_bin, perm).unwrap();

        let player = VlcPlayer::new_with_client(
            VlcConfig {
                vlc_bin: Some(dummy_bin.path().to_str().unwrap().to_string()),
                ..VlcConfig::default()
            },
            client,
        );
        (dummy_bin, player)
    }

    #[test]
    fn test_is_ok() {
        let shutdown = Cell::new(false);
        let (_dummy_bin, mut player) = dummy_bin_player(|_, _| {
            if shutdown.get() {
                Err(VlcError::BadResponse(format_err!("")))
            } else {
                Ok("".to_string())
            }
        });

        player.start(SlideshowConfig::default()).unwrap();

        // Player health's good while it's running
        assert!(player.is_ok());

        // Now process exits and health should not be okay
        shutdown.set(true);
        assert!(!player.is_ok());
    }

    #[test]
    fn test_pause() {
        let req = RefCell::new(None);
        let (_dummy_bin, mut player) = dummy_bin_player(|_, p| {
            req.borrow_mut()
                .replace(p.get("command").unwrap_or(&"").to_string());
            Ok("".to_string())
        });

        player.start(SlideshowConfig::default()).unwrap();

        player.pause().unwrap();
        assert_eq!(Some("pl_pause".to_string()), req.borrow_mut().take());
        // Calling pause twice should be no-op
        player.pause().unwrap();
        assert_eq!(None, req.borrow_mut().take());

        player.resume().unwrap();
        assert_eq!(Some("pl_play".to_string()), req.borrow_mut().take());
        player.resume().unwrap();
        assert_eq!(None, req.borrow_mut().take());

        // Do not send pause again if its alredy sleeping
        player.sleep().unwrap();
        req.borrow_mut().take();
        player.pause().unwrap();
        assert_eq!(None, req.borrow_mut().take());

        // Resume can ignore sleep
        player.resume().unwrap();
        assert_eq!(Some("pl_play".to_string()), req.borrow_mut().take());

        // Resume should reset sleep flag
        player.sleep().unwrap();
        assert_eq!(Some("pl_pause".to_string()), req.borrow_mut().take());
    }

    #[test]
    fn test_sleep() {
        let req = RefCell::new(None);
        let (_dummy_bin, mut player) = dummy_bin_player(|_, p| {
            req.borrow_mut()
                .replace(p.get("command").unwrap_or(&"").to_string());
            Ok("".to_string())
        });

        player.start(SlideshowConfig::default()).unwrap();

        player.sleep().unwrap();
        assert_eq!(Some("pl_pause".to_string()), req.borrow_mut().take());
        // Calling sleep twice should be no-op
        player.sleep().unwrap();
        assert_eq!(None, req.borrow_mut().take());

        player.wakeup().unwrap();
        assert_eq!(Some("pl_play".to_string()), req.borrow_mut().take());
        // Calling wakeup twice should be no-op
        player.wakeup().unwrap();
        assert_eq!(None, req.borrow_mut().take());

        // Do not send pause again if its alredy sleeping
        player.pause().unwrap();
        req.borrow_mut().take();
        player.sleep().unwrap();
        assert_eq!(None, req.borrow_mut().take());

        // Wakeup should not resume if it's pausing
        player.wakeup().unwrap();
        assert_eq!(None, req.borrow_mut().take());
    }
}
