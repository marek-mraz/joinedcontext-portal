//! The `hsl-transport` reference app (AP-34, AP-37, AP-38, AP-41).
//!
//! It shows the 30 buses of one Context Space on a map and keeps them moving. Everything it
//! serves comes from a single Endpoint whose URL it is handed at run time; it opens no other
//! connection, holds no credential and contains no authorization logic. The Endpoint it is
//! bound to is public, so the app never sees a user and asks nobody to sign in (AP-28).
//!
//! An Endpoint has no subscription or streaming surface and this app does not add one. One
//! poll loop reads the Endpoint's GeoJSON, keeps the last known position of each bus in
//! memory and pushes what moved to every connected browser as Server-Sent Events. A hundred
//! open maps are still one query per interval, and the browser never learns the Endpoint URL.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{json, Value};
use tokio_stream::{Stream, StreamExt};

pub mod assets;

/// The entity type this app was built for; it is also what the grant names.
const TYPE: &str = "Vehicle";
/// The fleet the demo runs, and the cap the Endpoint is asked for (AP-38).
const FLEET: usize = 30;
/// How often the poll loop asks the Endpoint. The manifest allows 1200 requests a minute, so
/// two seconds leaves the budget almost untouched however many browsers are watching.
const POLL_SECONDS: u64 = 2;
/// A slow browser must not hold the poll loop back, so the channel drops the oldest batch
/// instead of blocking. A viewer that falls this far behind gets the next full poll anyway.
const BROADCAST_DEPTH: usize = 16;

/// What the reconciler hands the container (Architecture/16 §5).
#[derive(Clone, Debug)]
pub struct Config {
    /// `/apps/{name}/`, the path the app is served under.
    pub base_path: String,
    /// `https://{host}/api/endpoint/{slug}/`, the only data surface it may call (AP-04).
    pub endpoint_url: String,
    /// Seconds between two polls of the Endpoint.
    pub poll_seconds: u64,
}

impl Config {
    /// Reads the variables of Architecture/16 §5. A missing endpoint URL is fatal: an app
    /// with nowhere to read from has nothing to serve, and guessing a default would be
    /// guessing which data a user gets.
    pub fn from_env() -> Result<Self, String> {
        let endpoint_url = std::env::var("JC_ENDPOINT_URL")
            .map_err(|_| "JC_ENDPOINT_URL is not set".to_owned())?;
        Ok(Self {
            base_path: std::env::var("JC_BASE_PATH").unwrap_or_else(|_| "/".to_owned()),
            endpoint_url: with_trailing_slash(&endpoint_url),
            poll_seconds: std::env::var("JC_POLL_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .filter(|seconds| *seconds > 0)
                .unwrap_or(POLL_SECONDS),
        })
    }
}

fn with_trailing_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_owned()
    } else {
        format!("{url}/")
    }
}

/// One bus, as the browser draws it. Every field but the position is optional because a
/// Policy may hide an attribute, and a hidden attribute is absent rather than defaulted.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Vehicle {
    pub id: String,
    /// `[longitude, latitude]`, the GeoJSON order.
    pub coordinates: [f64; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearing: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    #[serde(rename = "refLine", skip_serializing_if = "Option::is_none")]
    pub ref_line: Option<String>,
}

/// The application state: its configuration, the one client it makes requests with, the last
/// known fleet and the channel every open map listens on.
pub struct App {
    pub config: Config,
    http: reqwest::Client,
    fleet: RwLock<BTreeMap<String, Vehicle>>,
    changes: tokio::sync::broadcast::Sender<Vec<Vehicle>>,
}

impl App {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
            fleet: RwLock::new(BTreeMap::new()),
            changes: tokio::sync::broadcast::channel(BROADCAST_DEPTH).0,
        }
    }

    /// The fleet as it stands, ordered by id so two viewers see the same list.
    pub fn snapshot(&self) -> Vec<Vehicle> {
        self.fleet
            .read()
            .map(|fleet| fleet.values().cloned().collect())
            .unwrap_or_default()
    }

    /// One poll of the Endpoint: read the fleet, keep what moved, tell the browsers.
    ///
    /// Returns the buses that changed, which is what the tests assert on and what goes out
    /// over SSE. A bus that has not moved since the last poll is not sent again.
    pub async fn poll_once(&self) -> Result<Vec<Vehicle>, PollError> {
        let fetched = self.fetch().await?;
        let mut changed = Vec::new();
        {
            let mut fleet = self.fleet.write().map_err(|_| PollError::Unreachable)?;
            for vehicle in fetched {
                if fleet.get(&vehicle.id) != Some(&vehicle) {
                    fleet.insert(vehicle.id.clone(), vehicle.clone());
                    changed.push(vehicle);
                }
            }
        }
        if !changed.is_empty() {
            // An error here means nobody is watching, which is not a failure.
            let _ = self.changes.send(changed.clone());
        }
        Ok(changed)
    }

    /// The poll loop, for the process. Nothing else in the app makes an outbound call.
    pub async fn run(self: Arc<Self>) {
        let interval = Duration::from_secs(self.config.poll_seconds);
        loop {
            if let Err(error) = self.poll_once().await {
                // A poll that fails leaves the last known fleet on every open map, which is
                // the honest thing to show: the buses stop moving rather than disappearing.
                tracing::warn!(%error, "poll failed, keeping the last known fleet");
            }
            tokio::time::sleep(interval).await;
        }
    }

    /// The Endpoint's GeoJSON representation of the fleet (EP-05), capped at the fleet size.
    ///
    /// Anonymous on purpose: the Endpoint is public, one poll loop serves every viewer, and a
    /// per-viewer token could not be used for a shared read anyway (AP-28).
    async fn fetch(&self) -> Result<Vec<Vehicle>, PollError> {
        let response = self
            .http
            .get(format!("{}ngsi-ld/v1/entities", self.config.endpoint_url))
            .query(&[("type", TYPE), ("limit", &FLEET.to_string())])
            .header(axum::http::header::ACCEPT, "application/geo+json")
            .send()
            .await
            .map_err(|_| PollError::Unreachable)?;
        if !response.status().is_success() {
            return Err(PollError::Refused(response.status().as_u16()));
        }
        let body: Value = response.json().await.map_err(|_| PollError::Unreadable)?;
        Ok(vehicles(&body))
    }
}

/// Why a poll did not produce a fleet. None of it reaches the browser: the map keeps the last
/// known positions and the reason goes to the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollError {
    /// The Endpoint could not be reached.
    Unreachable,
    /// The Endpoint answered with a status that is not a fleet.
    Refused(u16),
    /// The Endpoint answered with something that is not GeoJSON.
    Unreadable,
}

impl std::fmt::Display for PollError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable => write!(f, "the endpoint did not answer"),
            Self::Refused(status) => write!(f, "the endpoint answered {status}"),
            Self::Unreadable => write!(f, "the endpoint's answer is not GeoJSON"),
        }
    }
}

/// The buses of one GeoJSON `FeatureCollection`.
///
/// A feature with no point geometry is dropped rather than defaulted to a coordinate: a bus
/// the map cannot place is a bus the map must not draw somewhere wrong.
pub fn vehicles(collection: &Value) -> Vec<Vehicle> {
    let features = collection
        .get("features")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    features.iter().filter_map(vehicle).take(FLEET).collect()
}

fn vehicle(feature: &Value) -> Option<Vehicle> {
    let geometry = feature.get("geometry")?;
    if geometry.get("type").and_then(Value::as_str) != Some("Point") {
        return None;
    }
    let coordinates = geometry.get("coordinates")?.as_array()?;
    let [longitude, latitude] = [
        coordinates.first()?.as_f64()?,
        coordinates.get(1)?.as_f64()?,
    ];
    // NGSI-LD GeoJSON keeps the entity id at the top level and the attributes in `properties`.
    let properties = feature.get("properties");
    let id = feature
        .get("id")
        .or_else(|| properties.and_then(|properties| properties.get("id")))?
        .as_str()?
        .to_owned();
    let number = |name: &str| properties?.get(name).and_then(simple_value)?.as_f64();
    let text = |name: &str| {
        properties?
            .get(name)
            .and_then(simple_value)?
            .as_str()
            .map(str::to_owned)
    };
    Some(Vehicle {
        id,
        coordinates: [longitude, latitude],
        bearing: number("bearing"),
        speed: number("speed"),
        ref_line: text("refLine"),
    })
}

/// An attribute is a bare value in the simplified representation and `{"value": …}` in the
/// normalized one; the Endpoint may serve either, so both are read.
fn simple_value(attribute: &Value) -> Option<&Value> {
    match attribute {
        Value::Object(fields) => fields.get("value"),
        other => Some(other),
    }
}

/// The app's routes under its own base path, with the embedded UI behind them.
///
/// The prefix is spelled into every route rather than nested. `Router::nest` does not match
/// the prefix with a trailing slash, and `https://{host}/apps/hsl-transport/` with that slash
/// is exactly what a browser follows from the Portal, so nesting would 404 the front page.
pub fn router(app: Arc<App>) -> Router {
    let base = app.config.base_path.trim_end_matches('/').to_owned();
    let at = |tail: &str| format!("{base}{tail}");
    Router::new()
        // The readiness probe of the pod, at the root and outside the base path
        // (Architecture/16 §5).
        .route("/healthz", get(|| async { "ok" }))
        .route(&at("/api/vehicles"), get(fleet))
        .route(&at("/api/stream"), get(stream))
        // Both spellings of the front page: the edge routes the prefix, and a person who
        // types it without the slash is not a different visitor.
        .route(&base, get(assets::static_handler))
        .route(&at("/"), get(assets::static_handler))
        .route(&at("/{*path}"), get(assets::static_handler))
        .with_state(app)
}

/// The fleet as the app last saw it. A map that has just loaded draws from this, so it is not
/// blank until the first bus moves.
async fn fleet(State(app): State<Arc<App>>) -> Response {
    let fleet = app.snapshot();
    if fleet.is_empty() {
        // Nothing polled yet is not the same as an empty fleet, and a map should say so.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
            json!({
                "type": "https://joinedcontext.com/errors/app",
                "title": "Service Unavailable",
                "status": 503,
                "detail": "the first poll of the endpoint has not produced a fleet yet",
            })
            .to_string(),
        )
            .into_response();
    }
    Json(fleet).into_response()
}

/// Every bus that moves, as Server-Sent Events.
///
/// The first event is the whole fleet, so a browser that connects here alone is drawing
/// immediately and every later event is a difference.
async fn stream(State(app): State<Arc<App>>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = app.changes.subscribe();
    let first = tokio_stream::iter([app.snapshot()]);
    let rest = tokio_stream::wrappers::BroadcastStream::new(receiver).filter_map(Result::ok);
    let events = first.chain(rest).map(|vehicles| {
        Ok(Event::default()
            .event("vehicles")
            .data(serde_json::to_string(&vehicles).unwrap_or_else(|_| "[]".to_owned())))
    });
    Sse::new(events).keep_alive(KeepAlive::default())
}
