use env_logger;

use googlephotos::api::{GPhotosApi, RetryConfig};
use phoseum::googlephotos;
use phoseum::oauth::TokenService;
use std::env;
use std::io;
use std::io::BufRead;

fn main() {
    env_logger::init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} OAUTH_CLIENT_ID OAUTH_CLIENT_SECRET", args[0]);
        std::process::exit(1);
    }

    let auth_config = googlephotos::api::auth_config(args[1].clone(), args[2].clone());
    let tokens = TokenService::new(auth_config).expect("oauth loading");

    let auth_url = tokens.start_new_authorization();

    println!("Open this URL in your browser:\n{}\n", auth_url);
    eprint!("Paste auth code: ");

    let auth_code = io::stdin()
        .lock()
        .lines()
        .next()
        .expect("read auth code (absent)")
        .expect("read auth code (error)");
    tokens
        .complete_authorization(auth_code)
        .expect("finalize authorization");

    println!("Auth OK");

    let gapi = GPhotosApi::new(tokens, RetryConfig::default());

    let mut page_token: Option<String> = None;
    println!("Albums (private):");
    loop {
        let resp = gapi.albums(page_token.as_deref()).expect("listing albums");
        if resp.albums.is_none() {
            break;
        }

        for album in resp.albums.unwrap() {
            println!(
                "* {} - {}",
                album.title.as_deref().unwrap_or("NO TITLE"),
                album.id
            );
            if let Some(url) = album.product_url {
                println!("  - {}", url);
            }
        }

        page_token = resp.next_page_token;
        if page_token.is_none() {
            break;
        }
    }

    let mut page_token: Option<String> = None;
    println!("Shared Albums:");
    loop {
        let resp = gapi
            .shared_albums(page_token.as_deref())
            .expect("listing albums");
        if resp.shared_albums.is_none() {
            break;
        }

        for album in resp.shared_albums.unwrap() {
            println!(
                "* {} - {}",
                album.title.as_deref().unwrap_or("NO TITLE"),
                album.id
            );
            if let Some(url) = album.product_url {
                println!("  - {}", url);
            }
        }

        page_token = resp.next_page_token;
        if page_token.is_none() {
            break;
        }
    }

    println!("Select one ID from the above albums and pass it to phoseum");
}
