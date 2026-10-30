//! Hermes: configuration and the [`HermesSource`] price source.
//!
//! The API key is read only from the environment, never from a file and never printed: `Debug`
//! and every error redact it, and the base URL too (a provider may put the key in its path or
//! query).
//!
//! **UNVERIFIED: the authentication scheme.** Hermes' price endpoints answer 401 without a key
//! (`docs/PLAN.md` verified facts), but Pyth's documentation does not yet state how the key is
//! sent. The only source found (Mysten Labs' deepbook SDK documentation) uses
//! `Authorization: Bearer <key>`, so that is the default; [`AuthScheme::Header`] sends it in a
//! named header instead (`VARSWAP_HERMES_AUTH=header:<name>`). Confirm on the first live call
//! and update this note and the plan.
//!
//! Endpoint semantics, verified in the Hermes source (pyth-crosschain
//! `apps/hermes/server/src/api/rest/v2/timestamp_price_updates.rs`, `RequestTime::FirstAfter`):
//! `GET /v2/updates/price/{t}` returns, per feed, the **first** update with
//! `publish_time >= t`. Its parsed form carries `price` and `conf` as decimal strings, `expo`,
//! `publish_time` and `metadata.prev_publish_time` (`apps/hermes/server/src/api/types.rs`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

use base64::Engine;
use serde::Deserialize;

use crate::{KeeperError, PriceSource, PrintMsg};

/// Environment variable that holds the Hermes API key.
pub const HERMES_API_KEY_VAR: &str = "VARSWAP_HERMES_API_KEY";

/// Environment variable that overrides the Hermes base URL.
pub const HERMES_URL_VAR: &str = "VARSWAP_HERMES_URL";

/// Environment variable that selects the authentication scheme: `bearer` (default) or
/// `header:<name>`.
pub const HERMES_AUTH_VAR: &str = "VARSWAP_HERMES_AUTH";

/// Default Hermes endpoint.
pub const DEFAULT_HERMES_URL: &str = "https://hermes.pyth.network";

/// How the API key is sent. **UNVERIFIED** (see the module documentation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthScheme {
    /// `Authorization: Bearer <key>` (default).
    Bearer,
    /// `<name>: <key>`.
    Header(String),
}

/// How to reach Hermes.
#[derive(Clone, PartialEq, Eq)]
pub struct HermesConfig {
    base_url: String,
    api_key: String,
    auth: AuthScheme,
}

impl HermesConfig {
    /// Reads the key from [`HERMES_API_KEY_VAR`], the URL from [`HERMES_URL_VAR`] (default
    /// [`DEFAULT_HERMES_URL`]) and the scheme from [`HERMES_AUTH_VAR`] (default bearer).
    ///
    /// # Errors
    ///
    /// `KeeperError::Config` if the key is missing or empty (Hermes price endpoints return 401
    /// without one), or the scheme is not `bearer` or `header:<name>`.
    pub fn from_env() -> Result<Self, KeeperError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// As [`HermesConfig::from_env`], with an explicit variable lookup (for tests).
    ///
    /// # Errors
    ///
    /// As [`HermesConfig::from_env`].
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, KeeperError> {
        let api_key = lookup(HERMES_API_KEY_VAR)
            .filter(|k| !k.trim().is_empty())
            .ok_or(KeeperError::Config("VARSWAP_HERMES_API_KEY is not set"))?;
        let base_url = lookup(HERMES_URL_VAR).unwrap_or_else(|| DEFAULT_HERMES_URL.to_string());
        let auth = match lookup(HERMES_AUTH_VAR).as_deref().map(str::trim) {
            None | Some("") | Some("bearer") => AuthScheme::Bearer,
            Some(other) => match other.strip_prefix("header:") {
                Some(name) if !name.trim().is_empty() => {
                    AuthScheme::Header(name.trim().to_string())
                }
                _ => {
                    return Err(KeeperError::Config(
                        "VARSWAP_HERMES_AUTH must be bearer or header:<name>",
                    ))
                }
            },
        };
        Ok(Self {
            base_url,
            api_key,
            auth,
        })
    }

    /// Base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The API key, for the request header only.
    #[must_use]
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// The authentication scheme.
    #[must_use]
    pub fn auth(&self) -> &AuthScheme {
        &self.auth
    }

    /// The `(name, value)` header that carries the key.
    #[must_use]
    pub fn auth_header(&self) -> (String, String) {
        match &self.auth {
            AuthScheme::Bearer => (
                "Authorization".to_string(),
                format!("Bearer {}", self.api_key),
            ),
            AuthScheme::Header(name) => (name.clone(), self.api_key.clone()),
        }
    }
}

impl fmt::Debug for HermesConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HermesConfig")
            .field(
                "base_url",
                &scrub(&self.base_url, &self.base_url, &self.api_key),
            )
            .field("api_key", &"<redacted>")
            .field("auth", &self.auth)
            .finish()
    }
}

/// An HTTP response: status and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body.
    pub body: String,
}

/// A blocking HTTP GET, so tests can replay fixtures instead of calling Hermes.
pub trait HttpGet {
    /// GETs `url` with `headers`. Any status is a response; only transport failures are errors.
    ///
    /// # Errors
    ///
    /// `KeeperError::Source` on a transport failure.
    fn get(&self, url: &str, headers: &[(String, String)]) -> Result<HttpResponse, KeeperError>;
}

/// [`HttpGet`] over `ureq` (rustls, no native TLS).
#[derive(Debug)]
pub struct UreqHttp {
    agent: ureq::Agent,
}

impl UreqHttp {
    /// An agent that returns every status as a response, with a 10-second timeout.
    #[must_use]
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(std::time::Duration::from_secs(10)))
            .build()
            .into();
        Self { agent }
    }
}

impl Default for UreqHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpGet for UreqHttp {
    fn get(&self, url: &str, headers: &[(String, String)]) -> Result<HttpResponse, KeeperError> {
        let mut request = self.agent.get(url);
        for (name, value) in headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let mut response = request
            .call()
            .map_err(|e| KeeperError::Source(format!("GET failed: {e}")))?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| KeeperError::Source(format!("reading body failed: {e}")))?;
        Ok(HttpResponse { status, body })
    }
}

/// One Hermes update for one feed: the parsed print and the raw accumulator update (what the
/// Pyth receiver posts; the production `Chain` uses it, milestone 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HermesUpdate {
    /// The parsed price message.
    pub print: PrintMsg,
    /// The binary accumulator update, decoded from base64.
    pub update_data: Vec<u8>,
}

#[derive(Deserialize)]
struct WirePriceUpdate {
    binary: WireBinary,
    parsed: Option<Vec<WireParsed>>,
}

#[derive(Deserialize)]
struct WireBinary {
    encoding: String,
    data: Vec<String>,
}

#[derive(Deserialize)]
struct WireParsed {
    id: String,
    price: WirePrice,
    metadata: WireMetadata,
}

#[derive(Deserialize)]
struct WirePrice {
    price: String,
    conf: String,
    expo: i32,
    publish_time: i64,
}

#[derive(Deserialize)]
struct WireMetadata {
    prev_publish_time: Option<i64>,
}

/// Parses a `/v2/updates/price/{t}?encoding=base64&parsed=true` body for `feed`.
///
/// # Errors
///
/// `KeeperError::MalformedResponse` (status, length and a fixed category, never the body) for
/// malformed JSON, a non-base64 encoding, a missing feed, or a price or confidence that is not
/// an integer. A missing `prev_publish_time` is read as 0, which the keeper and the program reject.
pub fn parse_update(body: &str, feed: &[u8; 32]) -> Result<HermesUpdate, KeeperError> {
    // No error quotes the response: serde's messages can echo field values (`invalid type:
    // string "…"`), so every failure is a fixed category with the status and length only.
    let bad = |what: &'static str| KeeperError::MalformedResponse {
        status: 200,
        byte_len: body.len(),
        what,
    };
    let wire: WirePriceUpdate = serde_json::from_str(body).map_err(|e| {
        bad(match e.classify() {
            serde_json::error::Category::Syntax => "not JSON",
            serde_json::error::Category::Eof => "truncated JSON",
            serde_json::error::Category::Data => "JSON of the wrong shape",
            serde_json::error::Category::Io => "unreadable body",
        })
    })?;
    if wire.binary.encoding != "base64" {
        return Err(bad("unexpected binary encoding"));
    }
    let want = hex(feed);
    let parsed = wire
        .parsed
        .unwrap_or_default()
        .into_iter()
        .find(|p| p.id.trim_start_matches("0x").eq_ignore_ascii_case(&want))
        .ok_or_else(|| bad("requested feed missing"))?;
    let price = parsed
        .price
        .price
        .parse::<i64>()
        .map_err(|_| bad("price is not an integer"))?;
    let conf = parsed
        .price
        .conf
        .parse::<u64>()
        .map_err(|_| bad("confidence is not an integer"))?;
    let raw = wire
        .binary
        .data
        .first()
        .ok_or_else(|| bad("no binary update"))?;
    let update_data = base64::engine::general_purpose::STANDARD
        .decode(raw)
        .map_err(|_| bad("binary update is not valid base64"))?;
    Ok(HermesUpdate {
        print: PrintMsg {
            price,
            conf,
            expo: parsed.price.expo,
            publish_time: parsed.price.publish_time,
            prev_publish_time: parsed.metadata.prev_publish_time.unwrap_or(0),
        },
        update_data,
    })
}

/// Placeholder for a redacted Hermes URL.
const HIDDEN_URL: &str = "<hermes url>";

fn scrub(text: &str, url: &str, key: &str) -> String {
    pyth_post::redact::scrub(text, url, key, HIDDEN_URL)
}

/// Scrubs an error message with [`scrub`], as a last line of defence: no error built in this
/// module quotes a response, but a transport error quotes the request URL and could carry text
/// from elsewhere.
fn redact(e: KeeperError, config: &HermesConfig) -> KeeperError {
    let scrub = |msg: String| scrub(&msg, config.base_url(), config.api_key());
    match e {
        KeeperError::Source(msg) => KeeperError::Source(scrub(msg)),
        KeeperError::Chain(msg) => KeeperError::Chain(scrub(msg)),
        other => other,
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    use fmt::Write as _;
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// A [`PriceSource`] backed by Hermes (`/v2/updates/price/{t}`, the first update with
/// `publish_time >= t`): at most one request per `(feed, t)`, every 200 answer cached (history
/// is immutable, and the key's rate limit is shared), a 429 reported as
/// [`KeeperError::RateLimited`]. Errors never carry response content,
/// and the API key is scrubbed from any error text. Ported from metalbook-keeper (#17).
#[derive(Debug)]
pub struct HermesSource<H: HttpGet> {
    config: HermesConfig,
    http: H,
    /// Every 200 answer of `first_at_or_after`, by `(feed, t)`: history is immutable, and the
    /// key's timestamp endpoint allows about 10 requests a minute (shared between keepers).
    cache: RefCell<HashMap<([u8; 32], i64), HermesUpdate>>,
}

impl<H: HttpGet> HermesSource<H> {
    fn request(&self, url: &str, feed: &[u8; 32]) -> Result<Option<HermesUpdate>, KeeperError> {
        self.request_unredacted(url, feed)
            .map_err(|e| redact(e, &self.config))
    }

    fn request_unredacted(
        &self,
        url: &str,
        feed: &[u8; 32],
    ) -> Result<Option<HermesUpdate>, KeeperError> {
        let response = self.http.get(url, &[self.config.auth_header()])?;
        match response.status {
            200 => parse_update(&response.body, feed).map(Some),
            404 => Ok(None),
            429 => Err(KeeperError::RateLimited),
            401 | 403 => Err(KeeperError::Source(format!(
                "Hermes rejected the API key ({}); the auth scheme is unverified, see VARSWAP_HERMES_AUTH",
                response.status
            ))),
            other => Err(KeeperError::Source(format!("Hermes returned HTTP {other}"))),
        }
    }

    /// A source using `config` over `http`.
    #[must_use]
    pub fn new(config: HermesConfig, http: H) -> Self {
        Self {
            config,
            http,
            cache: RefCell::new(HashMap::new()),
        }
    }

    /// The first update of `feed` with `publish_time >= t`, or `None` if Hermes has none yet
    /// (404).
    ///
    /// # Errors
    ///
    /// `KeeperError::Source` for a 401 or 403 (the key is missing, wrong or sent the wrong way:
    /// see the module note), any other non-success status, or a malformed body.
    pub fn first_at_or_after(
        &self,
        feed: &[u8; 32],
        t: i64,
    ) -> Result<Option<HermesUpdate>, KeeperError> {
        if let Some(u) = self.cache.borrow().get(&(*feed, t)) {
            return Ok(Some(u.clone()));
        }
        let url = format!(
            "{}/v2/updates/price/{t}?ids%5B%5D={}&encoding=base64&parsed=true",
            self.config.base_url().trim_end_matches('/'),
            hex(feed)
        );
        let update = self.request(&url, feed)?;
        if let Some(u) = &update {
            let mut cache = self.cache.borrow_mut();
            cache.insert((*feed, t), u.clone());
            // The same update answers a query at its own publish time (the keeper's second
            // request for a canonical print).
            if u.print.publish_time >= t {
                cache.insert((*feed, u.print.publish_time), u.clone());
            }
        }
        Ok(update)
    }

    /// The latest update of `feed` (`/v2/updates/price/latest`), or `None` if Hermes has none.
    ///
    /// # Errors
    ///
    /// As [`HermesSource::first_at_or_after`].
    pub fn latest(&self, feed: &[u8; 32]) -> Result<Option<PrintMsg>, KeeperError> {
        let url = format!(
            "{}/v2/updates/price/latest?ids%5B%5D={}&encoding=base64&parsed=true",
            self.config.base_url().trim_end_matches('/'),
            hex(feed)
        );
        self.request(&url, feed).map(|u| u.map(|u| u.print))
    }

    fn published(
        &self,
        feed: &[u8; 32],
        t: i64,
        now: i64,
    ) -> Result<Option<PrintMsg>, KeeperError> {
        Ok(self
            .first_at_or_after(feed, t)?
            .map(|u| u.print)
            .filter(|m| m.publish_time <= now))
    }
}

/// The binary update of a print published at `publish_time`: the first update at or after it.
/// For a canonical print that is the print itself (nothing is published between its boundary
/// and it); `ReceiverChain` checks the message before posting.
impl<H: HttpGet> pyth_post::UpdateSource for HermesSource<H> {
    fn update(&self, feed: &[u8; 32], publish_time: i64) -> Result<Vec<u8>, pyth_post::PostError> {
        let u = self
            .first_at_or_after(feed, publish_time)
            .map_err(|e| match e {
                KeeperError::RateLimited => pyth_post::PostError::RateLimited,
                other => pyth_post::PostError::Source(other.to_string()),
            })?
            .ok_or_else(|| {
                pyth_post::PostError::Source("Hermes has no update at that time".into())
            })?;
        Ok(u.update_data)
    }
}

impl<H: HttpGet> PriceSource for HermesSource<H> {
    fn canonical_at(
        &self,
        feed: &[u8; 32],
        t: i64,
        tolerance: i64,
        now: i64,
    ) -> Result<Option<PrintMsg>, KeeperError> {
        let latest = t
            .checked_add(tolerance)
            .ok_or(KeeperError::Config("tolerance overflow"))?;
        // The first update at or after t is the canonical one when it is within tolerance.
        Ok(self.published(feed, t, now)?.filter(|m| {
            m.prev_publish_time > 0 && m.prev_publish_time < t && m.publish_time <= latest
        }))
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::disallowed_macros
)]
mod tests {
    use super::*;

    #[test]
    fn test_key_comes_from_the_environment_and_is_redacted() {
        let missing = HermesConfig::from_lookup(|_| None);
        assert!(matches!(missing, Err(KeeperError::Config(_))));
        let empty =
            HermesConfig::from_lookup(|n| (n == HERMES_API_KEY_VAR).then(|| " ".to_string()));
        assert!(matches!(empty, Err(KeeperError::Config(_))));
        let c =
            HermesConfig::from_lookup(|n| (n == HERMES_API_KEY_VAR).then(|| "s3cr3t".to_string()))
                .unwrap();
        assert_eq!((c.base_url(), c.api_key()), (DEFAULT_HERMES_URL, "s3cr3t"));
        assert_eq!(c.auth(), &AuthScheme::Bearer);
        assert_eq!(
            c.auth_header(),
            ("Authorization".to_string(), "Bearer s3cr3t".to_string())
        );
        let shown = format!("{c:?}");
        assert!(!shown.contains("s3cr3t") && shown.contains("<redacted>"));
    }

    #[test]
    fn test_auth_scheme_from_the_environment() {
        let lookup = |auth: &'static str| {
            move |n: &str| match n {
                HERMES_API_KEY_VAR => Some("k".to_string()),
                HERMES_AUTH_VAR => Some(auth.to_string()),
                _ => None,
            }
        };
        let c = HermesConfig::from_lookup(lookup("header:x-api-key")).unwrap();
        assert_eq!(c.auth_header(), ("x-api-key".to_string(), "k".to_string()));
        assert_eq!(
            HermesConfig::from_lookup(lookup("bearer")).unwrap().auth(),
            &AuthScheme::Bearer
        );
        assert!(matches!(
            HermesConfig::from_lookup(lookup("basic")),
            Err(KeeperError::Config(_))
        ));
        assert!(matches!(
            HermesConfig::from_lookup(lookup("header:")),
            Err(KeeperError::Config(_))
        ));
    }
}
