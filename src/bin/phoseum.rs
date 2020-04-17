use clap::{App, Arg, ArgMatches};
use env_logger;
use failure::{Error, Fail};
use log::error;
use phoseum::console_control;
use phoseum::control::PlayerCmd;
use phoseum::googlephotos::{self, GPhotosAlbum};
use phoseum::gpio_control;
use phoseum::http_control;
use phoseum::oauth::{self, TokenService};
use phoseum::player::SlideshowConfig;
use phoseum::player_vlc::{VlcConfig, VlcPlayer};
use phoseum::playlist;
use phoseum::slideshow::Slideshow;
use phoseum::storage::Storage;
use phoseum::Phoseum;
use signal_hook;
use std::fmt::Debug;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Fail)]
#[fail(display = "Invalid argument {} - {}", name, reason)]
struct InvalidArgError {
    name: &'static str,
    reason: String,
}

type Result<T> = std::result::Result<T, Error>;

fn register_for_signal() -> Arc<AtomicBool> {
    let term = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::SIGTERM, Arc::clone(&term)).unwrap();
    signal_hook::flag::register(signal_hook::SIGINT, Arc::clone(&term)).unwrap();
    term
}

fn parse_value<T>(matches: &ArgMatches, name: &'static str) -> Result<Option<T>>
where
    T: FromStr,
    <T as FromStr>::Err: std::fmt::Display,
{
    if let Some(val) = matches.value_of(name) {
        return match val.parse::<T>() {
            Ok(v) => Ok(Some(v)),
            Err(e) => Err(InvalidArgError {
                name,
                reason: e.to_string(),
            }
            .into()),
        };
    }
    Ok(None)
}

fn create_album(matches: &ArgMatches) -> GPhotosAlbum {
    let album_id = matches.value_of("googlephotos.album_id").expect("album_id");
    let client_id = matches
        .value_of("googlephotos.oauth_client_id")
        .expect("oauth id");
    let client_secret = matches
        .value_of("googlephotos.oauth_client_secret")
        .expect("oauth secret");
    let auth_config = googlephotos::api::auth_config(client_id, client_secret);
    let token_service = TokenService::new(oauth::store::default_store_path(), auth_config)
        .expect("error loading token servie");

    googlephotos::new_gphotos_album(album_id, token_service)
}

fn create_pl_builder(matches: &ArgMatches) -> Result<playlist::PlaylistBuilder> {
    let mut builder = playlist::PlaylistBuilder::new();
    if let Some(min_size) = parse_value(matches, "playlist.min_size")? {
        builder = builder.min_size(min_size);
    }
    if let Some(max_size) = parse_value(matches, "playlist.max_size")? {
        builder = builder.max_size(max_size);
    }
    if let Some(fresh_retention) = parse_value(matches, "playlist.fresh_retention")? {
        builder = builder.fresh_retention(Duration::from_secs(fresh_retention));
    }
    Ok(builder)
}

fn create_player(matches: &ArgMatches) -> Result<VlcPlayer> {
    let http_port = parse_value(matches, "vlc.http_port")?;
    let vlc_bin = matches.value_of("vlc.bin").map(String::from);

    Ok(VlcPlayer::new(VlcConfig { http_port, vlc_bin }))
}

fn create_storage(matches: &ArgMatches) -> Result<Storage> {
    let media_dir = matches
        .value_of("storage.media_dir")
        .expect("storage.media_dir");
    let capacity: u64 = parse_value(matches, "storage.capacity")?.expect("storage.capacity");
    Ok(Storage::open(media_dir, capacity)?)
}

fn create_slideshow_config(matches: &ArgMatches) -> Result<SlideshowConfig> {
    let mut conf = SlideshowConfig::default();
    if let Some(seconds) = parse_value(matches, "slideshow.show_duration")? {
        conf.show_duration = Duration::from_secs(seconds);
    }
    if let Some(volume) = parse_value::<f32>(matches, "slideshow.audio_volume")? {
        if volume < 0.0 || volume > 1.0 {
            return Err(InvalidArgError {
                name: "slideshow.audio_volume",
                reason: "value must be in range between 0.0 and 1.0".to_string(),
            }
            .into());
        }
        conf.audio_volume = volume;
    }
    if matches.is_present("slideshow.no-fullscreen") {
        conf.fullscreen = false;
    }

    Ok(conf)
}

fn parse_pin_state(s: &str) -> Result<bool> {
    match s {
        "H" => Ok(true),
        "L" => Ok(false),
        unknown => Err(InvalidArgError {
            name: "control.gpio_map",
            reason: format!(
                "pin's state must be either 'H'(high) or 'L'(low): {}",
                unknown
            ),
        }
        .into()),
    }
}

fn create_gpio_commander(matches: &ArgMatches) -> Result<gpio_control::GpioCommander> {
    let mut pin_mapping = Vec::new();
    for map in matches.values_of("control.gpio_map").into_iter().flatten() {
        match map.splitn(4, ':').collect::<Vec<_>>().as_slice() {
            [offset, high_low, cmd_name, default] => {
                let offset = offset.parse::<u32>().map_err(|e| InvalidArgError {
                    name: "control.gpio_map",
                    reason: e.to_string(),
                })?;
                let edge_high = parse_pin_state(high_low)?;
                let cmd = PlayerCmd::from_name(cmd_name).ok_or_else(|| InvalidArgError {
                    name: "control.gpio_map",
                    reason: format!("no such command: {}", cmd_name),
                })?;
                let default_state = parse_pin_state(default)?;
                pin_mapping.push(gpio_control::PinMap::new(
                    offset,
                    edge_high,
                    default_state,
                    cmd,
                ));
            }
            _ => {
                return Err(InvalidArgError {
                    name: "control.gpio_map",
                    reason: "not in form of OFFSET:[HL]:COMMAND:[HL]".to_string(),
                }
                .into())
            }
        }
    }

    let gpio_dev = matches
        .value_of("control.gpio_dev")
        .ok_or_else(|| InvalidArgError {
            name: "control.gpio_dev",
            reason: "is missing".to_string(),
        })?;
    Ok(gpio_control::GpioCommander::create(gpio_dev, pin_mapping)?)
}

fn create_http_commander(matches: &ArgMatches) -> Result<http_control::HttpCommander> {
    let http_port: u32 = parse_value(matches, "control.http_port")?.expect("control.http_port");
    Ok(http_control::HttpCommander::new(http_port))
}

fn run(matches: ArgMatches<'_>) -> Result<()> {
    let slideshow = Slideshow::new(
        create_album(&matches),
        create_player(&matches)?,
        create_pl_builder(&matches)?,
        create_storage(&matches)?,
        create_slideshow_config(&matches)?,
    );

    let mut app = Phoseum::new(slideshow);
    app.add_playlist_commander(create_http_commander(&matches)?);
    match matches.value_of("control.player").expect("control.player") {
        "gpio" => {
            app.add_player_commander(create_gpio_commander(&matches)?);
        }
        "console" => {
            app.add_player_commander(console_control::ConsoleCommander::default());
        }
        unknown => panic!("unknown player control: {}", unknown),
    };

    let terminate = register_for_signal();
    app.run(terminate)?;
    Ok(())
}

fn main() {
    env_logger::init();

    let matches = App::new("Photo Museum")
        .version("0.1")
        .arg(
            Arg::with_name("storage.media_dir")
                .long("storage.media-dir")
                .required(true)
                .takes_value(true)
                .help("Path to directory that used as local storage of media files"),
        )
        .arg(
            Arg::with_name("storage.capacity")
                .long("storage.capacity")
                .takes_value(true)
                .default_value("10737418240")
                .help("Size in bytes to limit total size of files kept in local filesystem"),
        )
        .arg(
            Arg::with_name("googlephotos.album_id")
                .long("googlephotos.album-id")
                .required(true)
                .takes_value(true)
                .help("Album ID of Google Photos"),
        )
        .arg(
            Arg::with_name("googlephotos.oauth_client_id")
                .long("googlephotos.oauth-client-id")
                .required(true)
                .takes_value(true)
                .help("OAuth client ID to access API"),
        )
        .arg(
            Arg::with_name("googlephotos.oauth_client_secret")
                .long("googlephotos.oauth-client-secret")
                .required(true)
                .takes_value(true)
                .help("OAuth client secret to access API"),
        )
        .arg(
            Arg::with_name("playlist.min_size")
                .long("playlist.min-size")
                .takes_value(true)
                .help(
                    "Minimum size of the playlist. Old items are filled up to reach this size when there's no enough fresh items",
                ),
        )
        .arg(
            Arg::with_name("playlist.max_size")
                .long("playlist.max-size")
                .takes_value(true)
                .help(
                    "Maximum size of the playlist. Even though there're more new items, this is the hardlimit",
                ),
        )
        .arg(
            Arg::with_name("playlist.fresh_retention")
                .long("playlist.fresh-retention")
                .takes_value(true)
                .help(
                    "Retention in seconds to decide whether an item is new or not. Items created since this retention ago are considered as fresh",
                ),
        )
        .arg(
            Arg::with_name("slideshow.show_duration")
                .long("slideshow.show-duration")
                .takes_value(true)
                .help("Duration in seconds to set the time to keep showing one photo"),
        )
        .arg(
            Arg::with_name("slideshow.audio_volume")
                .long("slideshow.audio-volume")
                .takes_value(true)
                .help("Audio volume when playing video expressed as value between 0.0 (min) and 1.0 (max)"),
        )
        .arg(
            Arg::with_name("slideshow.no_fullscreen")
                .long("slideshow.no-fullscreen")
                .help("Turn off fullscreen (debug)"),
        )
        .arg(
            Arg::with_name("vlc.http_port")
                .long("vlc.http-port")
                .takes_value(true)
                .help("Http port for VLC player to listen for controlling it"),
        )
        .arg(
            Arg::with_name("vlc.bin")
                .long("vlc.bin")
                .takes_value(true)
                .help("VLC player executable path"),
        )
        .arg(
            Arg::with_name("control.player")
                .long("control.player")
                .takes_value(true)
                .possible_values(&["gpio", "console"])
                .default_value("gpio")
                .help("Player controlling interface (debug)"),
        )
        .arg(
            Arg::with_name("control.gpio_dev")
                .long("control.gpio-dev")
                .takes_value(true)
                .help("GPIO device name (required when enabling GPIO control)"),
        )
        .arg(
            Arg::with_name("control.gpio_map")
                .long("control.gpio-map")
                .takes_value(true)
                .multiple(true)
                .help("Mapping from each pin's state to command to produce. Format: PIN_OFFSET:[HL]:COMMAND:[HL](default)"),
        )
        .arg(
            Arg::with_name("control.http_port")
                .long("control.http-port")
                .takes_value(true)
                .default_value("8000")
                .help("HTTP port to listen and expose playlist controlling API"),
        )
        .get_matches();

    if let Err(e) = run(matches) {
        if let Some(e) = e.downcast_ref::<InvalidArgError>() {
            eprintln!("Invalid argument {} - {}", e.name, e.reason);
        } else {
            error!("Error happend, shutting down: {}", e);
        }
        std::process::exit(1);
    }
}
