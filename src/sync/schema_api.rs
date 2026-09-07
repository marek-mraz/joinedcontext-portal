//! Reading another organisation's schema surface over HTTP (DM-48, DM-49).
//!
//! `jcctl::foreign_models` makes every judgement about what a peer publishes and refuses a
//! plaintext base, a model name that is not a label and a document whose digest does not match
//! the peer's own `index.json`. This is the hop it is handed, and its whole job is to not open
//! a way round any of that:
//!
//! - `https://` only, checked here as well as there, because a redirect is a second URL the
//!   module above never sees;
//! - a redirect is followed only to another `https://` address, and only a few times;
//! - a timeout per request, so one slow partner cannot hold a reconcile open;
//! - a ceiling on the body, because a peer's `index.json` is a few kilobytes and a mirror that
//!   streams a gigabyte into memory is a peer's denial of service on us.
//!
//! No credential goes out with these requests and none is logged: a schema surface is the
//! public half of an endpoint (EP-46).

use std::io::Read;
use std::time::Duration;

use jcctl::foreign_models::{FetchError, SchemaApi};

/// How long one document may take. A peer that cannot answer in this is a peer whose mirror
/// waits until the next run.
const TIMEOUT: Duration = Duration::from_secs(15);

/// The largest document a mirror reads. The seven artifacts of a schema surface are text;
/// four megabytes is a generous LinkML model and nowhere near a memory problem.
const MAX_BODY: u64 = 4 * 1024 * 1024;

/// How many `https` redirects a document may hide behind.
const MAX_REDIRECTS: usize = 3;

/// The peer half of [`jcctl::foreign_models::mirror`].
pub struct PeerSchemaApi {
    http: reqwest::blocking::Client,
}

impl PeerSchemaApi {
    /// A client that refuses to leave `https`, times out and follows a few redirects.
    ///
    /// Blocking on purpose: `SchemaApi` is a synchronous trait, because the module above it is
    /// a decision and not a process, and the reconciler runs the whole mirror on a blocking
    /// task. An async client bridged with `block_on` would be the same wait with a way to
    /// deadlock the runtime in it.
    pub fn new() -> Result<Self, reqwest::Error> {
        let redirect = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.url().scheme() != "https" {
                // Not an error the peer can distinguish from a 404, and deliberately so: the
                // mirror says the surface could not be read, not what the redirect pointed at.
                return attempt.stop();
            }
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.stop();
            }
            attempt.follow()
        });
        let http = reqwest::blocking::Client::builder()
            .redirect(redirect)
            .timeout(TIMEOUT)
            .build()?;
        Ok(Self { http })
    }
}

impl SchemaApi for PeerSchemaApi {
    fn get(&self, url: &str) -> Result<Option<Vec<u8>>, FetchError> {
        if !url.starts_with("https://") {
            return Err(FetchError::Unavailable(format!(
                "{url} is not https, and a schema surface is not fetched in the clear"
            )));
        }

        let response = self
            .http
            .get(url)
            .send()
            .map_err(|err| FetchError::Unavailable(reason(err)))?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(FetchError::Refused {
                url: url.to_owned(),
                status: status.as_u16(),
            });
        }
        if response.content_length().is_some_and(|len| len > MAX_BODY) {
            return Err(FetchError::Unavailable(format!(
                "{url} is larger than the {MAX_BODY} bytes a schema document may have"
            )));
        }

        // Read one byte past the ceiling rather than trusting Content-Length: a chunked
        // response declares no length at all, and that is the shape a slow drip arrives in.
        let mut body = Vec::new();
        response
            .take(MAX_BODY + 1)
            .read_to_end(&mut body)
            .map_err(|err| FetchError::Unavailable(format!("{url}: {err}")))?;
        if body.len() as u64 > MAX_BODY {
            return Err(FetchError::Unavailable(format!(
                "{url} is larger than the {MAX_BODY} bytes a schema document may have"
            )));
        }
        Ok(Some(body))
    }
}

/// What went wrong, without the URL's credentials — there are none on this hop, and this keeps
/// it that way if one is ever added.
fn reason(err: reqwest::Error) -> String {
    if err.is_timeout() {
        return "the peer did not answer in time".to_owned();
    }
    if err.is_redirect() {
        return "the peer redirected somewhere a mirror does not follow".to_owned();
    }
    if err.is_connect() {
        return "the peer could not be reached".to_owned();
    }
    err.without_url().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plaintext_surface_is_refused_before_anything_is_requested() {
        let api = PeerSchemaApi::new().expect("a client");
        let error = api
            .get("http://peer.example/schema/index.json")
            .unwrap_err();
        assert!(
            matches!(error, FetchError::Unavailable(ref why) if why.contains("not https")),
            "{error}"
        );
    }

    #[test]
    fn the_ceiling_is_small_enough_to_be_a_ceiling() {
        // A schema surface is seven text documents. If this ever grows past a few megabytes,
        // it has stopped being a ceiling and the growth was not deliberate.
        const { assert!(MAX_BODY <= 8 * 1024 * 1024) };
    }
}
