pub mod store;

use failure::{self, Fail};
use log::debug;
use oauth2::basic::BasicClient;
use oauth2::reqwest::http_client;
use oauth2::{
    self, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, TokenResponse, TokenUrl,
};
use std::cell::RefCell;
use std::path::PathBuf;
use store::TokenStore;

#[derive(Debug, Fail)]
pub enum Error {
    /// Passed input contains invalid value
    #[fail(display = "Invalid argument {}: {}", name, reason)]
    InvalidArgument { name: &'static str, reason: String },
    /// Token request got error response
    #[fail(display = "Token request failed: {}", _0)]
    TokenRequest(failure::Error),
    /// No token is available locally now
    #[fail(display = "No token configured in local store")]
    NoAvailableToken,
    /// Attempted to complete authorization before initiating
    #[fail(display = "Authentication process has not started")]
    AuthNotStarted,
    /// Failed to access locally stored secrets
    #[fail(display = "{}", _0)]
    StoredSecret(#[fail(cause)] store::Error),
}

impl From<store::Error> for Error {
    fn from(e: store::Error) -> Self {
        Error::StoredSecret(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct TokenService {
    store: TokenStore,
    oauth2_client: BasicClient,
    auth_scopes: Vec<String>,

    authing_context: RefCell<Option<AuthenticatingContext>>,
}

#[derive(PartialEq, Clone, Debug)]
pub struct AuthConfig {
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scopes: Vec<String>,
    pub token_store: PathBuf,
}

impl TokenService {
    pub fn new(config: AuthConfig) -> Result<TokenService> {
        let auth_url = AuthUrl::new(config.auth_url).map_err(|e| Error::InvalidArgument {
            name: "auth_url",
            reason: e.to_string(),
        })?;
        let token_url = TokenUrl::new(config.token_url).map_err(|e| Error::InvalidArgument {
            name: "token_url",
            reason: e.to_string(),
        })?;
        let client_id = ClientId::new(config.client_id);
        let client_secret = ClientSecret::new(config.client_secret);

        let oauth2_client =
            BasicClient::new(client_id, Some(client_secret), auth_url, Some(token_url))
                .set_redirect_url(
                    RedirectUrl::new("urn:ietf:wg:oauth:2.0:oob".to_string())
                        .expect("set redirect url"),
                );

        let store = TokenStore::open(config.token_store)?;

        Ok(TokenService {
            store,
            oauth2_client,
            auth_scopes: config.scopes.into_iter().map(Into::into).collect(),

            authing_context: RefCell::new(None),
        })
    }

    pub fn start_new_authorization(&self) -> String {
        let (pkce_code_challenge, pkce_code_verifier) = PkceCodeChallenge::new_random_sha256();

        let mut req = self
            .oauth2_client
            .authorize_url(CsrfToken::new_random)
            .set_pkce_challenge(pkce_code_challenge);
        for scope in &self.auth_scopes {
            req = req.add_scope(Scope::new(scope.clone()));
        }

        let (authorize_url, _) = req.url();
        *self.authing_context.borrow_mut() = Some(AuthenticatingContext {
            pkce_verifier: pkce_code_verifier,
        });

        debug!("Started new authorization; url={}", authorize_url);
        authorize_url.to_string()
    }

    pub fn complete_authorization(&self, auth_code: String) -> Result<()> {
        let pkce_verifier = self
            .authing_context
            .borrow_mut()
            .take()
            .ok_or(Error::AuthNotStarted)?
            .pkce_verifier;

        let resp = self
            .oauth2_client
            .exchange_code(AuthorizationCode::new(auth_code))
            .set_pkce_verifier(pkce_verifier)
            .request(http_client)
            .map_err(|e| Error::TokenRequest(e.into()))?;

        let access_token = resp.access_token().secret();
        let refresh_token = resp.refresh_token().map(|t| t.secret().clone());
        debug!("Exchange token response: {:?}", resp);

        self.store.update_tokens(
            Some(access_token.clone()),
            resp.expires_in().map(|d| d.as_millis()),
            refresh_token,
        )?;

        Ok(())
    }

    pub fn obtain_access_token(&self) -> Result<String> {
        // Return if one is available in local cache
        if let Some(access_token) = self.store.valid_access_token() {
            return Ok(access_token);
        }

        // If refresh token is available, try refreshing token with it
        if let Some(refresh_token) = self.store.refresh_token() {
            let resp = self
                .oauth2_client
                .exchange_refresh_token(&RefreshToken::new(refresh_token))
                .request(http_client)
                .map_err(|e| Error::TokenRequest(e.into()))?;

            let access_token = resp.access_token().secret();
            debug!("Refresh token response: {:?}", resp);

            self.store.update_access_token(
                Some(access_token.clone()),
                resp.expires_in().map(|d| d.as_millis()),
            )?;
            return Ok(resp.access_token().secret().clone());
        }

        Err(Error::NoAvailableToken)
    }

    pub fn expire_current(&self) -> Result<()> {
        self.store.update_access_token(None, None)?;
        Ok(())
    }
}

struct AuthenticatingContext {
    pkce_verifier: PkceCodeVerifier,
}
