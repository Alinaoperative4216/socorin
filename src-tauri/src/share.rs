//! "Upload & copy link": sharing a capture on socorin.com.
//!
//! One click uploads the annotated PNG (or the recording) to the share
//! server named in Settings (`upload_server`, socorin.com by default) and
//! puts the link it answers with on the clipboard. The server keeps a file
//! for a limited time and takes a size limit; both come from the server
//! (`Limits`), nothing is hard-coded here. Every link comes with a delete
//! token, which is kept with the last `HISTORY_LIMIT` links in
//! `shares.json` next to the settings, so a file can be taken down again
//! from Settings → Share or from the popover that announces the link.
//!
//! Nothing is uploaded unless the user asks for it. The only thing the
//! server learns about the install is a random id it hands out itself
//! (`register`), its spam control; there are no accounts.
//!
//! The protocol (v1, fixed on both sides):
//!
//! ```text
//! GET    /api/upload/limits                        → Limits (cached per run)
//! POST   /api/app/register  {platform, appVersion} → {installId}, once per install
//! POST   /api/upload/init   {kind, mime, size, sha256, filename}
//!                                                  → {uploadId, chunkBytes, totalChunks, uploadToken, expiresAt}
//! PUT    /api/upload/{uploadId}/chunks/{index}     Bearer uploadToken, octet-stream (idempotent)
//! POST   /api/upload/{uploadId}/complete           Bearer uploadToken → ShareResult
//! DELETE /api/media/{id}                           Bearer deleteToken → 204
//! ```
//!
//! Every request carries `X-Socorin-Client: socorin-desktop/<version> (<os>)`
//! and, once registered, `X-Socorin-Install: <installId>`. Failures are an
//! HTTP 4xx / 5xx with `{error, message, …}`; a proxy in front of the server
//! may answer with something that is not JSON, so the status code alone has
//! to be enough to say what went wrong (`describe_failure`).

use std::{
    path::Path,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, Runtime};
#[cfg(not(test))]
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::{settings, windows};

/// Every request gives up after this long (the chunks are a few MB at most).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// A chunk is sent up to this many times when the network or the server
/// (5xx) fails; the PUT is idempotent, so a repeat is always safe.
const CHUNK_ATTEMPTS: u32 = 3;
/// Wait before the second attempt; doubled for the third.
const RETRY_BACKOFF: Duration = Duration::from_millis(300);
/// How many links `shares.json` remembers.
pub const HISTORY_LIMIT: usize = 20;
/// Event carrying the `Notice` to the popover.
pub const NOTICE_EVENT: &str = "share:notice";
/// Event telling the settings window the history changed.
pub const HISTORY_EVENT: &str = "share:history";
/// A file name sent with `init` is cut here (the server takes 200).
const MAX_FILENAME: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Image,
    Video,
}

/// What the server accepts (`GET /api/upload/limits`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub max_file_bytes: u64,
    pub chunk_bytes: u64,
    pub accepted_mimes: Vec<String>,
    pub retention_days: u32,
}

/// The answer to a finished upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareResult {
    pub id: String,
    pub share_url: String,
    pub delete_token: String,
    /// ISO 8601, when the server removes the file.
    pub expires_at: String,
    pub kind: Kind,
    pub mime: String,
    pub size: u64,
}

/// One line of `shares.json`: a `ShareResult` plus when it was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedLink {
    pub id: String,
    pub share_url: String,
    pub delete_token: String,
    pub expires_at: String,
    pub kind: Kind,
    pub mime: String,
    pub size: u64,
    /// Unix time (seconds) of the upload.
    pub created_at: u64,
}

impl SharedLink {
    fn from_result(result: ShareResult, created_at: u64) -> Self {
        Self {
            id: result.id,
            share_url: result.share_url,
            delete_token: result.delete_token,
            expires_at: result.expires_at,
            kind: result.kind,
            mime: result.mime,
            size: result.size,
            created_at,
        }
    }

    /// The link as the settings window and the popover see it: without the
    /// delete token, which never leaves the Rust side (deleting goes by id).
    pub fn public(&self) -> PublicLink {
        PublicLink {
            id: self.id.clone(),
            share_url: self.share_url.clone(),
            expires_at: self.expires_at.clone(),
            kind: self.kind,
            mime: self.mime.clone(),
            size: self.size,
            created_at: self.created_at,
        }
    }
}

/// A remembered link, minus its delete token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicLink {
    pub id: String,
    pub share_url: String,
    pub expires_at: String,
    pub kind: Kind,
    pub mime: String,
    pub size: u64,
    pub created_at: u64,
}

/// Why an upload (or a delete) did not happen, in the words the pages show.
/// `code` is the server's error code, or one of the app's own: `network`
/// (no answer at all), `server` (an HTTP 5xx or a non-JSON failure),
/// `unsupported_type` and `file_too_large` for the checks made before
/// anything is sent, `not_found` for a link the app no longer knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
    /// The HTTP status the server answered with (none for a network
    /// failure or a check made before sending). Decides whether a chunk is
    /// sent again (`retryable`); the pages do not need it.
    #[serde(skip)]
    pub status: Option<u16>,
}

impl ShareError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            max_bytes: None,
            retry_after_seconds: None,
            status: None,
        }
    }

    /// Worth another try: no answer at all, or a 5xx whatever its body says
    /// (the server's own 500 is `{"error":"internal"}`; a proxy's is HTML).
    /// A 4xx is the server's decision and stays.
    pub fn retryable(&self) -> bool {
        self.code == "network" || self.status.map_or(false, |s| s >= 500)
    }

    /// The pre-flight size check, and a 413 from the server.
    pub fn too_large(size: u64, max_bytes: u64) -> Self {
        Self {
            max_bytes: Some(max_bytes),
            ..Self::new(
                "file_too_large",
                format!("This capture is {}; the share limit is {}.", megabytes(size), megabytes(max_bytes)),
            )
        }
    }
}

impl std::fmt::Display for ShareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// What the popover shows: the link that was just made, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind")]
pub enum Notice {
    Shared { link: PublicLink, retention_days: u32 },
    Failed { error: ShareError },
}

#[derive(Default)]
pub struct ShareState {
    /// The limits fetched this run, and the server they came from.
    limits: Mutex<Option<(String, Limits)>>,
    notice: Mutex<Option<Notice>>,
    /// What would have gone to the clipboard (the tests never write the
    /// real one).
    #[cfg(test)]
    pub(crate) copied: Mutex<Vec<String>>,
}

// ---- formatting ----

/// "7.3 MB", "5 MB" (binary megabytes: the server's 5242880 is 5 MB).
pub fn megabytes(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if (mb - mb.round()).abs() < 0.05 {
        format!("{} MB", mb.round() as u64)
    } else {
        format!("{mb:.1} MB")
    }
}

/// "45 seconds", "1 minute", "3 minutes".
fn wait_time(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds} second{}", if seconds == 1 { "" } else { "s" })
    } else {
        let minutes = seconds.div_ceil(60);
        format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
    }
}

/// The host the messages name ("socorin.com", "localhost:3000").
fn host_of(server: &str) -> String {
    url::Url::parse(server)
        .ok()
        .and_then(|u| {
            u.host_str().map(|h| match u.port() {
                Some(p) => format!("{h}:{p}"),
                None => h.to_string(),
            })
        })
        .unwrap_or_else(|| server.to_string())
}

/// `X-Socorin-Client`.
pub fn client_header() -> String {
    format!("socorin-desktop/{} ({})", env!("CARGO_PKG_VERSION"), std::env::consts::OS)
}

/// The mime type the server is told for a video file, by its extension.
/// Only what the server accepts; a `.mov` (macOS' own recording format) is
/// converted first, see `record::uploadable_video`.
pub fn video_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "mp4" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        _ => None,
    }
}

/// Lower-case hex SHA-256, as `init` wants it.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---- the failure vocabulary ----

/// The error body the server sends; every field is optional because a
/// proxy may answer instead of the server.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    error: Option<String>,
    message: Option<String>,
    max_bytes: Option<u64>,
    retry_after_seconds: Option<u64>,
}

/// Turn a failed response into words. `size` is what was being uploaded
/// (for the too-large message); `retry_after` is the header, when present.
pub fn describe_failure(host: &str, status: u16, retry_after: Option<u64>, body: &[u8], size: u64) -> ShareError {
    let parsed: ErrorBody = serde_json::from_slice(body).unwrap_or_default();
    let code = parsed.error.filter(|c| !c.is_empty()).unwrap_or_else(|| {
        match status {
            413 => "file_too_large",
            429 => "rate_limited",
            507 => "storage_full",
            415 => "unsupported_type",
            401 | 403 => "forbidden",
            404 => "session_not_found",
            400 | 422 => "invalid_request",
            500..=599 => "server",
            _ => "server",
        }
        .into()
    });
    let retry = parsed.retry_after_seconds.or(retry_after);
    let server_says = parsed.message.filter(|m| !m.trim().is_empty());
    let with_reason = |text: String| match &server_says {
        Some(m) => format!("{text} ({m})"),
        None => text,
    };
    let message = match code.as_str() {
        "file_too_large" => match parsed.max_bytes {
            Some(max) => return ShareError { status: Some(status), ..ShareError::too_large(size, max) },
            None => format!("This capture is {}, more than {host} accepts.", megabytes(size)),
        },
        "rate_limited" => match retry {
            Some(s) => format!("Too many uploads for now. Try again in {}.", wait_time(s)),
            None => "Too many uploads for now. Try again in a minute.".into(),
        },
        "storage_full" => format!("{host} has no room for new uploads right now. Try again later."),
        "unsupported_type" => format!("{host} does not accept this kind of file."),
        "session_expired" | "session_not_found" => format!("The upload session on {host} expired. Try again."),
        "chunk_invalid" | "chunk_missing" | "checksum_mismatch" => {
            format!("The upload arrived damaged at {host}. Try again.")
        }
        "forbidden" | "blocked" => with_reason(format!("{host} refused this upload.")),
        "invalid_request" => with_reason(format!("{host} rejected the request.")),
        _ if status >= 500 => format!("{host} is not available right now (HTTP {status}). Try again later."),
        _ => with_reason(format!("{host} answered HTTP {status}.")),
    };
    ShareError {
        code,
        message,
        max_bytes: parsed.max_bytes,
        retry_after_seconds: retry,
        status: Some(status),
    }
}

fn network_error(host: &str, e: &reqwest::Error) -> ShareError {
    let what = if e.is_timeout() {
        format!("{host} did not answer in time.")
    } else if e.is_connect() {
        format!("Cannot reach {host}.")
    } else {
        format!("Cannot reach {host}: {e}")
    };
    ShareError::new("network", what)
}

// ---- the client ----

struct Client {
    http: reqwest::Client,
    server: String,
    host: String,
    install_id: Option<String>,
}

impl Client {
    fn new(server: &str, install_id: Option<String>) -> Result<Self, ShareError> {
        crate::update::ensure_crypto_provider();
        let http = reqwest::Client::builder()
            .user_agent(concat!("Socorin/", env!("CARGO_PKG_VERSION")))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| ShareError::new("client", e.to_string()))?;
        Ok(Self {
            http,
            server: server.to_string(),
            host: host_of(server),
            install_id,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut builder = self
            .http
            .request(method, format!("{}{path}", self.server))
            .header("X-Socorin-Client", client_header())
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(id) = &self.install_id {
            builder = builder.header("X-Socorin-Install", id.as_str());
        }
        builder
    }

    /// Send, and turn anything but a 2xx into a `ShareError`. Returns the
    /// status and the body.
    async fn send(&self, builder: reqwest::RequestBuilder, size: u64) -> Result<(u16, Vec<u8>), ShareError> {
        let response = builder.send().await.map_err(|e| network_error(&self.host, &e))?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let body = response
            .bytes()
            .await
            .map_err(|e| network_error(&self.host, &e))?
            .to_vec();
        if status.is_success() {
            Ok((status.as_u16(), body))
        } else {
            Err(describe_failure(&self.host, status.as_u16(), retry_after, &body, size))
        }
    }

    fn json<T: serde::de::DeserializeOwned>(&self, body: &[u8], what: &str) -> Result<T, ShareError> {
        serde_json::from_slice(body).map_err(|e| {
            ShareError::new("invalid_response", format!("{} sent an unexpected answer to {what}: {e}", self.host))
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registered {
    install_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Initiated {
    upload_id: String,
    chunk_bytes: u64,
    total_chunks: u64,
    upload_token: String,
    #[allow(dead_code)]
    expires_at: Option<String>,
}

/// The server's limits, fetched once per run (per server).
pub async fn limits<R: Runtime>(app: &AppHandle<R>) -> Result<Limits, ShareError> {
    let server = settings::current(app).upload_server;
    if let Some((from, cached)) = app.state::<ShareState>().limits.lock().unwrap().clone() {
        if from == server {
            return Ok(cached);
        }
    }
    let client = Client::new(&server, None)?;
    let limits = fetch_limits(&client).await?;
    *app.state::<ShareState>().limits.lock().unwrap() = Some((server, limits.clone()));
    Ok(limits)
}

async fn fetch_limits(client: &Client) -> Result<Limits, ShareError> {
    let (_, body) = client.send(client.request(reqwest::Method::GET, "/api/upload/limits"), 0).await?;
    let limits: Limits = client.json(&body, "limits")?;
    if limits.chunk_bytes == 0 || limits.max_file_bytes == 0 {
        return Err(ShareError::new("invalid_response", format!("{} reports no upload limits.", client.host)));
    }
    Ok(limits)
}

/// The install id from the settings, or a fresh one from the server
/// (stored right away).
async fn install_id<R: Runtime>(app: &AppHandle<R>, client: &Client) -> Result<String, ShareError> {
    let known = settings::current(app).install_id;
    if !known.is_empty() {
        return Ok(known);
    }
    let body = serde_json::json!({ "platform": std::env::consts::OS, "appVersion": env!("CARGO_PKG_VERSION") });
    let (_, answer) = client
        .send(client.request(reqwest::Method::POST, "/api/app/register").json(&body), 0)
        .await?;
    let registered: Registered = client.json(&answer, "register")?;
    let id = settings::sanitise_install_id(&registered.install_id);
    if id.is_empty() {
        return Err(ShareError::new("invalid_response", format!("{} sent an unusable install id.", client.host)));
    }
    let mut s = settings::current(app);
    s.install_id = id.clone();
    settings::store(app, s).map_err(|e| ShareError::new("settings", e))?;
    Ok(id)
}

/// Checks made before anything is sent: the type must be one the server
/// takes and the size within its limit.
pub fn check(limits: &Limits, mime: &str, size: u64, host: &str) -> Result<(), ShareError> {
    if !limits.accepted_mimes.iter().any(|m| m.eq_ignore_ascii_case(mime)) {
        return Err(ShareError::new("unsupported_type", format!("{host} does not accept {mime} files.")));
    }
    if size > limits.max_file_bytes {
        return Err(ShareError::too_large(size, limits.max_file_bytes));
    }
    Ok(())
}

/// Upload `bytes` and return the link. The whole protocol, in order: the
/// limits (cached), the install id (registered once), `init`, the chunks
/// (each retried on network / server trouble), `complete`. The link is
/// remembered in the history.
pub async fn upload<R: Runtime>(
    app: &AppHandle<R>,
    bytes: Vec<u8>,
    kind: Kind,
    mime: &str,
    filename: Option<String>,
) -> Result<SharedLink, ShareError> {
    let server = settings::current(app).upload_server;
    let limits = limits(app).await?;
    let size = bytes.len() as u64;
    check(&limits, mime, size, &host_of(&server))?;

    let mut client = Client::new(&server, None)?;
    let id = install_id(app, &client).await?;
    client.install_id = Some(id);

    let filename = filename.map(|f| f.chars().take(MAX_FILENAME).collect::<String>());
    let init = serde_json::json!({
        "kind": kind,
        "mime": mime,
        "size": size,
        "sha256": sha256_hex(&bytes),
        "filename": filename,
    });
    let (_, body) = client
        .send(client.request(reqwest::Method::POST, "/api/upload/init").json(&init), size)
        .await?;
    let session: Initiated = client.json(&body, "init")?;
    if session.chunk_bytes == 0 {
        return Err(ShareError::new("invalid_response", format!("{} sent no chunk size.", client.host)));
    }

    let chunk_bytes = session.chunk_bytes as usize;
    let chunks: Vec<&[u8]> = if bytes.is_empty() { vec![&bytes[..]] } else { bytes.chunks(chunk_bytes).collect() };
    if session.total_chunks != 0 && session.total_chunks != chunks.len() as u64 {
        return Err(ShareError::new(
            "invalid_response",
            format!("{} expects {} chunks, this file makes {}.", client.host, session.total_chunks, chunks.len()),
        ));
    }
    for (index, chunk) in chunks.iter().enumerate() {
        put_chunk(&client, &session, index, chunk, size).await?;
    }

    let (_, body) = client
        .send(
            client
                .request(reqwest::Method::POST, &format!("/api/upload/{}/complete", session.upload_id))
                .bearer_auth(&session.upload_token),
            size,
        )
        .await?;
    let result: ShareResult = client.json(&body, "complete")?;
    let link = SharedLink::from_result(result, now_secs());
    remember(app, &link);
    Ok(link)
}

/// One chunk, sent again after a network failure or any 5xx (the server
/// takes repeats of the same index), never after a 4xx.
async fn put_chunk(client: &Client, session: &Initiated, index: usize, chunk: &[u8], size: u64) -> Result<(), ShareError> {
    let path = format!("/api/upload/{}/chunks/{index}", session.upload_id);
    let mut attempt = 0;
    loop {
        attempt += 1;
        let request = client
            .request(reqwest::Method::PUT, &path)
            .bearer_auth(&session.upload_token)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(reqwest::header::CONTENT_LENGTH, chunk.len())
            .body(chunk.to_vec());
        match client.send(request, size).await {
            Ok(_) => return Ok(()),
            Err(e) if attempt < CHUNK_ATTEMPTS && e.retryable() => {
                eprintln!("[share] chunk {index} attempt {attempt}: {}", e.message);
                tokio::time::sleep(RETRY_BACKOFF * (1 << (attempt - 1))).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Take a shared file down again. A link the server no longer has (404) is
/// gone as well; either way it leaves the history.
pub async fn delete<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<(), ShareError> {
    let link = history(app)
        .into_iter()
        .find(|l| l.id == id)
        .ok_or_else(|| ShareError::new("not_found", "This link is no longer in the list."))?;
    let server = settings::current(app).upload_server;
    let install = settings::current(app).install_id;
    let client = Client::new(&server, (!install.is_empty()).then_some(install))?;
    let request = client
        .request(reqwest::Method::DELETE, &format!("/api/media/{}", link.id))
        .bearer_auth(&link.delete_token);
    match client.send(request, link.size).await {
        Ok(_) => {}
        Err(e) if e.code == "session_not_found" || e.code == "not_found" => {}
        Err(e) => return Err(e),
    }
    forget(app, id);
    Ok(())
}

// ---- the history (shares.json) ----

fn history_file<R: Runtime>(app: &AppHandle<R>) -> Option<std::path::PathBuf> {
    settings::config_path(app, "shares.json")
}

/// The remembered links, newest first.
pub fn history<R: Runtime>(app: &AppHandle<R>) -> Vec<SharedLink> {
    history_file(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_history<R: Runtime>(app: &AppHandle<R>, links: &[SharedLink]) {
    let Some(path) = history_file(app) else {
        return;
    };
    match serde_json::to_string_pretty(links) {
        Ok(json) => {
            if let Err(e) = settings::write_atomically(&path, json.as_bytes()) {
                eprintln!("[share] {e}");
            }
        }
        Err(e) => eprintln!("[share] {e}"),
    }
    let _ = app.emit(HISTORY_EVENT, ());
}

fn remember<R: Runtime>(app: &AppHandle<R>, link: &SharedLink) {
    let mut links = history(app);
    links.retain(|l| l.id != link.id);
    links.insert(0, link.clone());
    links.truncate(HISTORY_LIMIT);
    save_history(app, &links);
}

fn forget<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let mut links = history(app);
    let before = links.len();
    links.retain(|l| l.id != id);
    if links.len() != before {
        save_history(app, &links);
    }
}

/// Put a remembered link on the clipboard again.
pub fn copy_link<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<(), String> {
    let link = history(app)
        .into_iter()
        .find(|l| l.id == id)
        .ok_or("This link is no longer in the list.")?;
    copy_text(app, &link.share_url)
}

/// The link goes to the clipboard as text. The tests record it instead:
/// nothing they do may touch the real clipboard.
pub fn copy_text<R: Runtime>(app: &AppHandle<R>, text: &str) -> Result<(), String> {
    #[cfg(test)]
    {
        app.state::<ShareState>().copied.lock().unwrap().push(text.to_string());
        Ok(())
    }
    #[cfg(not(test))]
    {
        app.clipboard()
            .write_text(text)
            .map_err(|e| format!("clipboard write failed: {e}"))
    }
}

// ---- the popover ----

/// Upload, copy the link and announce it: the whole "Upload & copy link"
/// action for a PNG, from any page. The pages get the outcome back as
/// well, for their own toast.
pub async fn share_png<R: Runtime>(app: &AppHandle<R>, png: Vec<u8>) -> Result<ShareResult, ShareError> {
    let name = format!("{}_{}.png", settings::current(app).file_prefix, crate::capture::time_stamp());
    let outcome = upload(app, png, Kind::Image, "image/png", Some(name)).await;
    announce(app, outcome)
}

/// Copy the link and show the popover (or show the failure), and hand the
/// outcome back for the caller. Copying can fail on its own; then the
/// upload still happened and the popover offers the link.
pub fn announce<R: Runtime>(app: &AppHandle<R>, outcome: Result<SharedLink, ShareError>) -> Result<ShareResult, ShareError> {
    let notice = match &outcome {
        Ok(link) => {
            if let Err(e) = copy_text(app, &link.share_url) {
                eprintln!("[share] {e}");
            }
            let retention_days = app
                .state::<ShareState>()
                .limits
                .lock()
                .unwrap()
                .as_ref()
                .map(|(_, l)| l.retention_days)
                .unwrap_or(0);
            Notice::Shared { link: link.public(), retention_days }
        }
        Err(e) => {
            eprintln!("[share] {}: {}", e.code, e.message);
            Notice::Failed { error: e.clone() }
        }
    };
    *app.state::<ShareState>().notice.lock().unwrap() = Some(notice.clone());
    let _ = app.emit(NOTICE_EVENT, &notice);
    if let Err(e) = windows::show_share_notice(app) {
        eprintln!("[share] {e}");
    }
    outcome.map(|link| ShareResult {
        id: link.id,
        share_url: link.share_url,
        delete_token: link.delete_token,
        expires_at: link.expires_at,
        kind: link.kind,
        mime: link.mime,
        size: link.size,
    })
}

/// "After capture: upload" — the PNG goes up on a worker thread; the
/// popover reports the outcome.
pub fn share_png_async<R: Runtime>(app: &AppHandle<R>, png: Vec<u8>) {
    let app = app.clone();
    std::thread::spawn(move || {
        let outcome = tauri::async_runtime::block_on(share_png(&app, png));
        if outcome.is_ok() {
            let _ = app.emit("capture-done", "link");
        }
    });
}

/// What the popover shows right now.
pub fn notice<R: Runtime>(app: &AppHandle<R>) -> Option<Notice> {
    app.state::<ShareState>().notice.lock().unwrap().clone()
}

/// Close, Escape, a click elsewhere or the timer: hide the popover.
pub fn dismiss<R: Runtime>(app: &AppHandle<R>) {
    windows::hide_share_notice(app);
}

#[cfg(test)]
pub(crate) fn reset<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<ShareState>();
    *state.limits.lock().unwrap() = None;
    *state.notice.lock().unwrap() = None;
    state.copied.lock().unwrap().clear();
    if let Some(path) = history_file(app) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        settings::Settings,
        test_support::{app_with, settings_in, temp_dir},
    };
    use std::{
        collections::HashMap,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{
            atomic::{AtomicU32, Ordering},
            Arc,
        },
    };

    /// One request as the fake server saw it.
    #[derive(Debug, Clone)]
    pub struct Req {
        pub method: String,
        pub path: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Req {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    pub struct Resp {
        pub status: u16,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    pub fn json(status: u16, value: serde_json::Value) -> Resp {
        Resp {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: value.to_string().into_bytes(),
        }
    }

    fn read_request(stream: &mut TcpStream) -> Option<Req> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        let header_end = loop {
            let n = stream.read(&mut chunk).ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
        let mut lines = head.lines();
        let request_line = lines.next()?;
        let mut parts = request_line.split_whitespace();
        let method = parts.next()?.to_string();
        let path = parts.next()?.to_string();
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        let length: usize = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);
        let mut body = buf[header_end..].to_vec();
        while body.len() < length {
            let n = stream.read(&mut chunk).ok()?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
        body.truncate(length);
        Some(Req { method, path, headers, body })
    }

    /// A tiny HTTP/1.1 server answering every request with `handler`, for
    /// the rest of the test process. Returns its base URL.
    pub fn serve(handler: impl Fn(&Req) -> Resp + Send + Sync + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handler = Arc::new(handler);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let handler = handler.clone();
                std::thread::spawn(move || {
                    let Some(request) = read_request(&mut stream) else { return };
                    let resp = handler(&request);
                    let mut head = format!("HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n", resp.status, resp.body.len());
                    for (k, v) in &resp.headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&resp.body);
                    let _ = stream.flush();
                });
            }
        });
        base
    }

    /// A share server that speaks the protocol: it hands out limits and
    /// install ids, takes chunks (checking the token, order-independent),
    /// verifies the checksum on `complete`, deletes by token, and logs
    /// every request. `fail_chunk` makes the first attempt at that chunk
    /// index answer with the given status.
    pub struct Fake {
        pub chunk_bytes: u64,
        pub max_bytes: u64,
        pub fail_chunk: Option<(usize, u16)>,
        /// The failure answers like the real server (`{"error":"internal"}`)
        /// instead of like a proxy (HTML).
        pub fail_chunk_json: bool,
        pub log: Mutex<Vec<Req>>,
        registered: AtomicU32,
        chunks: Mutex<HashMap<u32, Vec<u8>>>,
        expected_sha: Mutex<String>,
        expected_size: Mutex<u64>,
        failed_once: AtomicU32,
        deleted: Mutex<Vec<String>>,
    }

    impl Fake {
        pub fn new(chunk_bytes: u64, max_bytes: u64) -> Arc<Self> {
            Arc::new(Self {
                chunk_bytes,
                max_bytes,
                fail_chunk: None,
                fail_chunk_json: false,
                log: Mutex::new(Vec::new()),
                registered: AtomicU32::new(0),
                chunks: Mutex::new(HashMap::new()),
                expected_sha: Mutex::new(String::new()),
                expected_size: Mutex::new(0),
                failed_once: AtomicU32::new(0),
                deleted: Mutex::new(Vec::new()),
            })
        }

        pub fn registrations(&self) -> u32 {
            self.registered.load(Ordering::SeqCst)
        }

        pub fn deleted(&self) -> Vec<String> {
            self.deleted.lock().unwrap().clone()
        }

        pub fn requests(&self, method: &str, prefix: &str) -> Vec<Req> {
            self.log
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.method == method && r.path.starts_with(prefix))
                .cloned()
                .collect()
        }

        pub fn handle(&self, req: &Req) -> Resp {
            self.log.lock().unwrap().push(req.clone());
            if req.header("X-Socorin-Client").map_or(true, |c| !c.starts_with("socorin-desktop/")) {
                return json(400, serde_json::json!({ "error": "invalid_request", "message": "no client header" }));
            }
            match (req.method.as_str(), req.path.as_str()) {
                ("GET", "/api/upload/limits") => json(
                    200,
                    serde_json::json!({
                        "maxFileBytes": self.max_bytes,
                        "chunkBytes": self.chunk_bytes,
                        "acceptedMimes": ["image/png", "image/jpeg", "image/webp", "image/gif", "video/mp4", "video/webm"],
                        "retentionDays": 60
                    }),
                ),
                ("POST", "/api/app/register") => {
                    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                    assert!(matches!(body["platform"].as_str(), Some("macos" | "windows" | "linux")));
                    assert_eq!(body["appVersion"], env!("CARGO_PKG_VERSION"));
                    let n = self.registered.fetch_add(1, Ordering::SeqCst) + 1;
                    json(201, serde_json::json!({ "installId": format!("inst-{n:0>17}") }))
                }
                ("POST", "/api/upload/init") => {
                    if req.header("X-Socorin-Install").is_none() {
                        return json(403, serde_json::json!({ "error": "forbidden", "message": "register first" }));
                    }
                    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                    let size = body["size"].as_u64().unwrap();
                    if size > self.max_bytes {
                        return json(413, serde_json::json!({ "error": "file_too_large", "message": "too big", "maxBytes": self.max_bytes }));
                    }
                    *self.expected_sha.lock().unwrap() = body["sha256"].as_str().unwrap().to_string();
                    *self.expected_size.lock().unwrap() = size;
                    self.chunks.lock().unwrap().clear();
                    let total = size.div_ceil(self.chunk_bytes).max(1);
                    json(
                        201,
                        serde_json::json!({
                            "uploadId": "up-0123456789abcdefghij",
                            "chunkBytes": self.chunk_bytes,
                            "totalChunks": total,
                            "uploadToken": "tok-secret",
                            "expiresAt": "2026-09-18T12:00:00Z"
                        }),
                    )
                }
                ("PUT", path) if path.starts_with("/api/upload/up-0123456789abcdefghij/chunks/") => {
                    if req.header("Authorization") != Some("Bearer tok-secret") {
                        return json(403, serde_json::json!({ "error": "forbidden" }));
                    }
                    assert_eq!(req.header("Content-Type"), Some("application/octet-stream"));
                    assert_eq!(req.header("Content-Length").and_then(|v| v.parse::<usize>().ok()), Some(req.body.len()));
                    let index: u32 = path.rsplit('/').next().unwrap().parse().unwrap();
                    if let Some((at, status)) = self.fail_chunk {
                        if index as usize == at && self.failed_once.fetch_add(1, Ordering::SeqCst) == 0 {
                            if self.fail_chunk_json {
                                return json(status, serde_json::json!({ "error": "internal", "message": "Internal server error" }));
                            }
                            return Resp { status, headers: vec![], body: b"<html>bad gateway</html>".to_vec() };
                        }
                    }
                    if req.body.len() as u64 > self.chunk_bytes {
                        return json(400, serde_json::json!({ "error": "chunk_invalid", "message": "chunk too long" }));
                    }
                    let mut chunks = self.chunks.lock().unwrap();
                    chunks.insert(index, req.body.clone());
                    let total = self.expected_size.lock().unwrap().div_ceil(self.chunk_bytes).max(1);
                    json(200, serde_json::json!({ "received": chunks.len(), "totalChunks": total }))
                }
                ("POST", "/api/upload/up-0123456789abcdefghij/complete") => {
                    if req.header("Authorization") != Some("Bearer tok-secret") {
                        return json(403, serde_json::json!({ "error": "forbidden" }));
                    }
                    let chunks = self.chunks.lock().unwrap();
                    let mut all = Vec::new();
                    for i in 0..chunks.len() as u32 {
                        match chunks.get(&i) {
                            Some(c) => all.extend_from_slice(c),
                            None => return json(409, serde_json::json!({ "error": "chunk_missing" })),
                        }
                    }
                    if all.len() as u64 != *self.expected_size.lock().unwrap() {
                        return json(409, serde_json::json!({ "error": "chunk_missing", "message": "short" }));
                    }
                    if sha256_hex(&all) != *self.expected_sha.lock().unwrap() {
                        return json(422, serde_json::json!({ "error": "checksum_mismatch" }));
                    }
                    json(
                        201,
                        serde_json::json!({
                            "id": "med-0123456789abcdefgh",
                            "shareUrl": "https://socorin.com/s/med-0123456789abcdefgh",
                            "deleteToken": "del-".to_string() + &"x".repeat(39),
                            "expiresAt": "2026-11-17T00:00:00Z",
                            "kind": "image",
                            "mime": "image/png",
                            "size": all.len()
                        }),
                    )
                }
                ("DELETE", path) if path.starts_with("/api/media/") => {
                    let id = path.trim_start_matches("/api/media/").to_string();
                    let token = req.header("Authorization").unwrap_or("").to_string();
                    if token != format!("Bearer del-{}", "x".repeat(39)) {
                        return json(403, serde_json::json!({ "error": "forbidden" }));
                    }
                    if self.deleted.lock().unwrap().contains(&id) {
                        return json(404, serde_json::json!({ "error": "session_not_found" }));
                    }
                    self.deleted.lock().unwrap().push(id);
                    Resp { status: 204, headers: vec![], body: vec![] }
                }
                _ => json(404, serde_json::json!({ "error": "not_found" })),
            }
        }
    }

    pub fn serve_fake(fake: &Arc<Fake>) -> String {
        let fake = fake.clone();
        serve(move |req| fake.handle(req))
    }

    fn run<F: std::future::Future>(f: F) -> F::Output {
        tauri::async_runtime::block_on(f)
    }

    fn png(len: usize) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend((0..len.saturating_sub(8)).map(|i| (i * 7 % 251) as u8));
        bytes
    }

    fn app_for(server: &str, name: &str) -> crate::test_support::TestApp {
        let app = app_with(Settings { upload_server: server.into(), ..settings_in(&temp_dir(name)) });
        reset(app.handle());
        app
    }

    #[test]
    fn checksums_sizes_and_words() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(megabytes(5_242_880), "5 MB");
        assert_eq!(megabytes(7_654_321), "7.3 MB");
        assert_eq!(megabytes(0), "0 MB");
        assert_eq!(megabytes(1_100_000), "1 MB");
        assert_eq!(megabytes(1_200_000), "1.1 MB");
        assert_eq!(wait_time(1), "1 second");
        assert_eq!(wait_time(45), "45 seconds");
        assert_eq!(wait_time(60), "1 minute");
        assert_eq!(wait_time(61), "2 minutes");
        assert_eq!(host_of("https://socorin.com"), "socorin.com");
        assert_eq!(host_of("http://localhost:3000"), "localhost:3000");
        assert_eq!(host_of("nope"), "nope");
        assert!(client_header().starts_with("socorin-desktop/"));
        assert!(client_header().ends_with(&format!("({})", std::env::consts::OS)));
        assert_eq!(video_mime(Path::new("/x/a.MP4")), Some("video/mp4"));
        assert_eq!(video_mime(Path::new("/x/a.webm")), Some("video/webm"));
        assert_eq!(video_mime(Path::new("/x/a.mov")), None);
        assert_eq!(video_mime(Path::new("/x/noext")), None);
        assert!(now_secs() > 1_700_000_000);
        let e = ShareError::too_large(7_654_321, 5_242_880);
        assert_eq!(e.to_string(), "This capture is 7.3 MB; the share limit is 5 MB.");
        assert_eq!(e.max_bytes, Some(5_242_880));
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["code"], "file_too_large");
        assert_eq!(json["maxBytes"], 5_242_880);
        assert!(json.get("retryAfterSeconds").is_none());
        assert!(json.get("status").is_none(), "the status stays on the Rust side");
        assert!(!e.retryable());
        assert!(ShareError::new("network", "x").retryable());
        assert!(ShareError { status: Some(500), ..ShareError::new("internal", "x") }.retryable());
        assert!(ShareError { status: Some(503), ..ShareError::new("server", "x") }.retryable());
        assert!(!ShareError { status: Some(429), ..ShareError::new("rate_limited", "x") }.retryable());
        assert!(!ShareError { status: Some(403), ..ShareError::new("forbidden", "x") }.retryable());
    }

    #[test]
    fn failures_are_described_from_the_body_or_the_status_alone() {
        let host = "socorin.com";
        let e = describe_failure(host, 413, None, br#"{"error":"file_too_large","message":"x","maxBytes":5242880}"#, 7_654_321);
        assert_eq!((e.code.as_str(), e.max_bytes), ("file_too_large", Some(5_242_880)));
        assert_eq!(e.message, "This capture is 7.3 MB; the share limit is 5 MB.");
        let e = describe_failure(host, 413, None, b"<html>Request Entity Too Large</html>", 7_654_321);
        assert_eq!(e.code, "file_too_large");
        assert_eq!(e.message, "This capture is 7.3 MB, more than socorin.com accepts.");

        let e = describe_failure(host, 429, Some(90), br#"{"error":"rate_limited","message":"slow down","retryAfterSeconds":30}"#, 1);
        assert_eq!((e.code.as_str(), e.retry_after_seconds), ("rate_limited", Some(30)));
        assert_eq!(e.message, "Too many uploads for now. Try again in 30 seconds.");
        let e = describe_failure(host, 429, Some(90), b"", 1);
        assert_eq!((e.code.as_str(), e.retry_after_seconds), ("rate_limited", Some(90)));
        assert_eq!(e.message, "Too many uploads for now. Try again in 2 minutes.");
        let e = describe_failure(host, 429, None, b"not json", 1);
        assert_eq!(e.message, "Too many uploads for now. Try again in a minute.");

        let e = describe_failure(host, 507, None, br#"{"error":"storage_full"}"#, 1);
        assert_eq!(e.message, "socorin.com has no room for new uploads right now. Try again later.");
        assert_eq!(describe_failure(host, 507, None, b"", 1).code, "storage_full");
        let e = describe_failure(host, 415, None, br#"{"error":"unsupported_type"}"#, 1);
        assert_eq!(e.message, "socorin.com does not accept this kind of file.");
        assert_eq!(describe_failure(host, 415, None, b"", 1).code, "unsupported_type");
        for code in ["session_expired", "session_not_found"] {
            let e = describe_failure(host, 410, None, format!(r#"{{"error":"{code}"}}"#).as_bytes(), 1);
            assert_eq!(e.code, code);
            assert!(e.message.contains("session on socorin.com expired"), "{}", e.message);
        }
        for code in ["chunk_invalid", "chunk_missing", "checksum_mismatch"] {
            let e = describe_failure(host, 409, None, format!(r#"{{"error":"{code}"}}"#).as_bytes(), 1);
            assert!(e.message.contains("arrived damaged"), "{code}: {}", e.message);
        }
        let e = describe_failure(host, 403, None, br#"{"error":"blocked","message":"abuse"}"#, 1);
        assert_eq!(e.message, "socorin.com refused this upload. (abuse)");
        assert_eq!(describe_failure(host, 401, None, b"", 1).message, "socorin.com refused this upload.");
        assert_eq!(describe_failure(host, 404, None, b"", 1).code, "session_not_found");
        let e = describe_failure(host, 400, None, br#"{"error":"invalid_request","message":"bad mime"}"#, 1);
        assert_eq!(e.message, "socorin.com rejected the request. (bad mime)");
        assert_eq!(describe_failure(host, 422, None, b"", 1).code, "invalid_request");
        let e = describe_failure(host, 502, None, b"<html>Bad gateway</html>", 1);
        assert_eq!((e.code.as_str(), e.status), ("server", Some(502)));
        assert_eq!(e.message, "socorin.com is not available right now (HTTP 502). Try again later.");
        assert!(e.retryable());
        // The real server's only 500: a JSON body with its own code.
        let e = describe_failure(host, 500, None, br#"{"error":"internal","message":"Internal server error"}"#, 1);
        assert_eq!((e.code.as_str(), e.status), ("internal", Some(500)));
        assert_eq!(e.message, "socorin.com is not available right now (HTTP 500). Try again later.");
        assert!(e.retryable());
        let e = describe_failure(host, 500, None, br#"{"error":"boom","message":"db down"}"#, 1);
        assert_eq!(e.code, "boom");
        assert!(e.message.contains("HTTP 500"), "{}", e.message);
        assert_eq!(describe_failure(host, 413, None, br#"{"error":"file_too_large","maxBytes":1}"#, 2).status, Some(413));
        assert!(!describe_failure(host, 403, None, b"", 1).retryable());
        let e = describe_failure(host, 418, None, br#"{"error":"teapot","message":"short and stout"}"#, 1);
        assert_eq!(e.message, "socorin.com answered HTTP 418. (short and stout)");
        assert_eq!(describe_failure(host, 418, None, b"", 1).message, "socorin.com answered HTTP 418.");
        assert_eq!(describe_failure(host, 418, None, br#"{"error":""}"#, 1).code, "server");
    }

    #[test]
    fn the_preflight_checks_type_and_size() {
        let limits = Limits {
            max_file_bytes: 100,
            chunk_bytes: 10,
            accepted_mimes: vec!["image/png".into(), "video/mp4".into()],
            retention_days: 60,
        };
        assert_eq!(check(&limits, "image/png", 100, "h"), Ok(()));
        assert_eq!(check(&limits, "IMAGE/PNG", 1, "h"), Ok(()));
        let e = check(&limits, "image/png", 101, "h").unwrap_err();
        assert_eq!((e.code.as_str(), e.max_bytes), ("file_too_large", Some(100)));
        let e = check(&limits, "video/quicktime", 1, "h").unwrap_err();
        assert_eq!(e.code, "unsupported_type");
        assert_eq!(e.message, "h does not accept video/quicktime files.");
    }

    #[test]
    fn uploads_in_one_chunk_registering_once_and_remembering_the_link() {
        let fake = Fake::new(8 * 1024 * 1024, 5 * 1024 * 1024);
        let base = serve_fake(&fake);
        let app = app_for(&base, "share-one");
        let handle = app.handle().clone();
        let bytes = png(1000);

        let link = run(upload(&handle, bytes.clone(), Kind::Image, "image/png", Some("a.png".into()))).unwrap();
        assert_eq!(link.share_url, "https://socorin.com/s/med-0123456789abcdefgh");
        assert_eq!(link.id, "med-0123456789abcdefgh");
        assert_eq!(link.size, 1000);
        assert_eq!(link.kind, Kind::Image);
        assert!(link.created_at > 1_700_000_000);
        assert_eq!(fake.registrations(), 1);
        assert_eq!(settings::current(&handle).install_id, "inst-00000000000000001");

        // Headers: the client header everywhere, the install header once known.
        let limits_req = &fake.requests("GET", "/api/upload/limits")[0];
        assert!(limits_req.header("X-Socorin-Client").unwrap().starts_with("socorin-desktop/"));
        assert_eq!(limits_req.header("X-Socorin-Install"), None);
        let init = &fake.requests("POST", "/api/upload/init")[0];
        assert_eq!(init.header("X-Socorin-Install"), Some("inst-00000000000000001"));
        let body: serde_json::Value = serde_json::from_slice(&init.body).unwrap();
        assert_eq!(body["kind"], "image");
        assert_eq!(body["mime"], "image/png");
        assert_eq!(body["size"], 1000);
        assert_eq!(body["sha256"], sha256_hex(&bytes));
        assert_eq!(body["filename"], "a.png");
        let puts = fake.requests("PUT", "/api/upload/");
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].body, bytes);
        assert_eq!(fake.requests("POST", "/api/upload/up-0123456789abcdefghij/complete").len(), 1);

        // Remembered, newest first, without the token in the public view.
        let remembered = history(&handle);
        assert_eq!(remembered.len(), 1);
        assert_eq!(remembered[0], link);
        let public = serde_json::to_value(link.public()).unwrap();
        assert!(public.get("deleteToken").is_none());
        assert_eq!(public["shareUrl"], link.share_url);

        // A second upload: no new registration, the limits are cached.
        run(upload(&handle, png(20), Kind::Image, "image/png", None)).unwrap();
        assert_eq!(fake.registrations(), 1);
        assert_eq!(fake.requests("GET", "/api/upload/limits").len(), 1);
        assert_eq!(history(&handle).len(), 1, "the same id is one line");
        let init = &fake.requests("POST", "/api/upload/init")[1];
        let body: serde_json::Value = serde_json::from_slice(&init.body).unwrap();
        assert!(body["filename"].is_null());
    }

    #[test]
    fn uploads_in_several_chunks_and_retries_a_failed_one() {
        let mut fake = Fake::new(300, 5 * 1024 * 1024);
        Arc::get_mut(&mut fake).unwrap().fail_chunk = Some((1, 502));
        let base = serve_fake(&fake);
        let app = app_for(&base, "share-chunks");
        let handle = app.handle().clone();
        let bytes = png(1000); // 4 chunks of 300, the last one 100

        let link = run(upload(&handle, bytes.clone(), Kind::Image, "image/png", None)).unwrap();
        assert_eq!(link.size, 1000);
        let puts = fake.requests("PUT", "/api/upload/");
        // 4 chunks plus the repeat of chunk 1.
        assert_eq!(puts.len(), 5);
        let indexes: Vec<&str> = puts.iter().map(|p| p.path.rsplit('/').next().unwrap()).collect();
        assert_eq!(indexes, ["0", "1", "1", "2", "3"]);
        assert_eq!(puts[0].body, &bytes[..300]);
        assert_eq!(puts[4].body, &bytes[900..]);
        assert_eq!(puts[4].header("Content-Length"), Some("100"));
    }

    /// The real server's 500 is JSON with its own code (`internal`): the
    /// chunk is still sent again.
    #[test]
    fn a_chunk_answered_with_the_servers_own_500_is_retried_too() {
        let mut fake = Fake::new(300, 5 * 1024 * 1024);
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.fail_chunk = Some((0, 500));
            f.fail_chunk_json = true;
        }
        let base = serve_fake(&fake);
        let app = app_for(&base, "share-500-json");
        let bytes = png(1000);
        let link = run(upload(app.handle(), bytes.clone(), Kind::Image, "image/png", None)).unwrap();
        assert_eq!(link.size, 1000);
        let puts = fake.requests("PUT", "/api/upload/");
        assert_eq!(puts.len(), 5);
        let indexes: Vec<&str> = puts.iter().map(|p| p.path.rsplit('/').next().unwrap()).collect();
        assert_eq!(indexes, ["0", "0", "1", "2", "3"]);
        assert_eq!(puts[0].body, puts[1].body);
        assert_eq!(history(app.handle()).len(), 1);
    }

    #[test]
    fn a_chunk_refused_by_the_server_is_not_retried() {
        let mut fake = Fake::new(300, 5 * 1024 * 1024);
        Arc::get_mut(&mut fake).unwrap().fail_chunk = Some((0, 403));
        let base = serve_fake(&fake);
        let app = app_for(&base, "share-4xx");
        let e = run(upload(app.handle(), png(500), Kind::Image, "image/png", None)).unwrap_err();
        assert_eq!(e.code, "forbidden");
        assert_eq!(fake.requests("PUT", "/api/upload/").len(), 1);
        assert!(history(app.handle()).is_empty());
    }

    #[test]
    fn a_chunk_that_keeps_failing_gives_up_after_three_attempts() {
        let puts = Arc::new(AtomicU32::new(0));
        let counter = puts.clone();
        let base = serve(move |req| {
            if req.method == "PUT" {
                counter.fetch_add(1, Ordering::SeqCst);
                return Resp { status: 503, headers: vec![], body: b"unavailable".to_vec() };
            }
            Fake::new(100, 1000).handle(req)
        });
        let app = app_for(&base, "share-503");
        let handle = app.handle().clone();
        let e = run(upload(&handle, png(50), Kind::Image, "image/png", None)).unwrap_err();
        assert_eq!((e.code.as_str(), e.status), ("server", Some(503)));
        assert!(e.message.contains("HTTP 503"), "{}", e.message);
        assert_eq!(puts.load(Ordering::SeqCst), CHUNK_ATTEMPTS);

        // The same with the real server's JSON 500, three times over.
        let puts = Arc::new(AtomicU32::new(0));
        let counter = puts.clone();
        let base = serve(move |req| {
            if req.method == "PUT" {
                counter.fetch_add(1, Ordering::SeqCst);
                return json(500, serde_json::json!({ "error": "internal", "message": "Internal server error" }));
            }
            Fake::new(100, 1000).handle(req)
        });
        point_at(&handle, &base);
        let e = run(upload(&handle, png(50), Kind::Image, "image/png", None)).unwrap_err();
        assert_eq!((e.code.as_str(), e.status), ("internal", Some(500)));
        assert!(e.message.contains("not available right now"), "{}", e.message);
        assert_eq!(puts.load(Ordering::SeqCst), CHUNK_ATTEMPTS);
        assert!(history(&handle).is_empty());
    }

    #[test]
    fn too_large_files_never_reach_init_and_a_413_is_understood_too() {
        let fake = Fake::new(1000, 500);
        let base = serve_fake(&fake);
        let app = app_for(&base, "share-large");
        let handle = app.handle().clone();
        let e = run(upload(&handle, png(600), Kind::Image, "image/png", None)).unwrap_err();
        assert_eq!((e.code.as_str(), e.max_bytes), ("file_too_large", Some(500)));
        assert_eq!(e.message, "This capture is 0 MB; the share limit is 0 MB.");
        assert!(fake.requests("POST", "/api/upload/init").is_empty());
        assert_eq!(fake.registrations(), 0, "nothing is registered for a file that cannot go");

        // The server disagrees with its own limits: its 413 is read.
        let fake2 = Fake::new(1000, 5000);
        let base2 = serve(move |req| {
            if req.path == "/api/upload/init" {
                return json(413, serde_json::json!({ "error": "file_too_large", "maxBytes": 100 }));
            }
            fake2.handle(req)
        });
        point_at(&handle, &base2);
        let e = run(upload(&handle, png(600), Kind::Image, "image/png", None)).unwrap_err();
        assert_eq!((e.code.as_str(), e.max_bytes), ("file_too_large", Some(100)));

        // A type the server does not list is refused before anything is sent.
        let e = run(upload(&handle, png(10), Kind::Video, "video/quicktime", None)).unwrap_err();
        assert_eq!(e.code, "unsupported_type");
    }

    #[test]
    fn rate_limits_and_unreachable_servers_are_reported() {
        let base = serve(|req| match req.path.as_str() {
            "/api/upload/limits" => Fake::new(100, 1000).handle(req),
            "/api/app/register" => Resp {
                status: 429,
                headers: vec![("Retry-After".into(), "45".into())],
                body: br#"{"error":"rate_limited","message":"x","retryAfterSeconds":45}"#.to_vec(),
            },
            _ => json(500, serde_json::json!({})),
        });
        let app = app_for(&base, "share-429");
        let handle = app.handle().clone();
        let e = run(upload(&handle, png(10), Kind::Image, "image/png", None)).unwrap_err();
        assert_eq!((e.code.as_str(), e.retry_after_seconds), ("rate_limited", Some(45)));
        assert_eq!(e.message, "Too many uploads for now. Try again in 45 seconds.");
        assert_eq!(settings::current(&handle).install_id, "");

        // Nothing listening: a network error, before anything else.
        point_at(&handle, "http://127.0.0.1:9");
        let e = run(upload(&handle, png(10), Kind::Image, "image/png", None)).unwrap_err();
        assert_eq!(e.code, "network");
        assert!(e.message.contains("127.0.0.1:9"), "{}", e.message);
        let e = run(delete(&handle, "nope")).unwrap_err();
        assert_eq!(e.code, "not_found");
    }

    /// Point the shared app at another fake server (the app is one per
    /// process and locked by the test: it is never created twice).
    fn point_at(handle: &AppHandle<tauri::test::MockRuntime>, server: &str) {
        *handle.state::<crate::settings::SettingsState>().0.lock().unwrap() =
            Settings { upload_server: server.into(), ..settings::current(handle) };
        reset(handle);
    }

    #[test]
    fn unexpected_answers_are_invalid_responses() {
        let base = serve(|req| match req.path.as_str() {
            "/api/upload/limits" => json(200, serde_json::json!({ "maxFileBytes": 0, "chunkBytes": 0, "acceptedMimes": [], "retentionDays": 1 })),
            _ => json(200, serde_json::json!({})),
        });
        let app = app_for(&base, "share-invalid");
        let handle = app.handle().clone();
        let e = run(upload(&handle, png(10), Kind::Image, "image/png", None)).unwrap_err();
        assert_eq!(e.code, "invalid_response");
        assert!(e.message.contains("no upload limits"), "{}", e.message);

        let base = serve(|req| match req.path.as_str() {
            "/api/upload/limits" => json(200, serde_json::json!({ "maxFileBytes": 10 })),
            _ => json(200, serde_json::json!({})),
        });
        point_at(&handle, &base);
        let e = run(upload(&handle, png(10), Kind::Image, "image/png", None)).unwrap_err();
        assert_eq!(e.code, "invalid_response");
        assert!(e.message.contains("unexpected answer to limits"), "{}", e.message);

        // Registration answers with a bad id.
        let base = serve(|req| match req.path.as_str() {
            "/api/upload/limits" => Fake::new(100, 1000).handle(req),
            "/api/app/register" => json(201, serde_json::json!({ "installId": "bad id!" })),
            _ => json(200, serde_json::json!({})),
        });
        point_at(&handle, &base);
        let e = run(upload(&handle, png(10), Kind::Image, "image/png", None)).unwrap_err();
        assert!(e.message.contains("unusable install id"), "{}", e.message);
        assert_eq!(settings::current(&handle).install_id, "");

        // init without a chunk size, then with a chunk count that does not fit.
        let base = serve(|req| match req.path.as_str() {
            "/api/upload/init" => json(201, serde_json::json!({ "uploadId": "u", "chunkBytes": 0, "totalChunks": 1, "uploadToken": "t" })),
            _ => Fake::new(100, 1000).handle(req),
        });
        point_at(&handle, &base);
        let e = run(upload(&handle, png(10), Kind::Image, "image/png", None)).unwrap_err();
        assert!(e.message.contains("no chunk size"), "{}", e.message);
        let base = serve(|req| match req.path.as_str() {
            "/api/upload/init" => json(201, serde_json::json!({ "uploadId": "u", "chunkBytes": 100, "totalChunks": 7, "uploadToken": "t" })),
            _ => Fake::new(100, 1000).handle(req),
        });
        point_at(&handle, &base);
        let e = run(upload(&handle, png(10), Kind::Image, "image/png", None)).unwrap_err();
        assert!(e.message.contains("expects 7 chunks"), "{}", e.message);
    }

    #[test]
    fn links_can_be_deleted_copied_and_are_capped_at_twenty() {
        let fake = Fake::new(1000, 5000);
        let base = serve_fake(&fake);
        let app = app_for(&base, "share-delete");
        let handle = app.handle().clone();
        let link = run(upload(&handle, png(10), Kind::Image, "image/png", None)).unwrap();
        assert_eq!(copy_link(&handle, &link.id), Ok(()));
        assert_eq!(handle.state::<ShareState>().copied.lock().unwrap().last().unwrap(), &link.share_url);
        assert!(copy_link(&handle, "unknown").is_err());

        run(delete(&handle, &link.id)).unwrap();
        assert_eq!(fake.deleted(), vec![link.id.clone()]);
        let del = &fake.requests("DELETE", "/api/media/")[0];
        assert_eq!(del.header("X-Socorin-Install"), Some("inst-00000000000000001"));
        assert!(history(&handle).is_empty());
        // Gone from the list: nothing more to do.
        assert_eq!(run(delete(&handle, &link.id)).unwrap_err().code, "not_found");

        // Deleting a link the server already forgot (404) still clears it.
        let link = run(upload(&handle, png(10), Kind::Image, "image/png", None)).unwrap();
        run(delete(&handle, &link.id)).unwrap();
        let mut again = link.clone();
        again.id = "med-0123456789abcdefgh".into();
        remember(&handle, &again);
        run(delete(&handle, &again.id)).unwrap();
        assert!(history(&handle).is_empty());
        // A refused delete (wrong token) keeps the line.
        let mut wrong = link.clone();
        wrong.delete_token = "del-nope".into();
        remember(&handle, &wrong);
        assert_eq!(run(delete(&handle, &wrong.id)).unwrap_err().code, "forbidden");
        assert_eq!(history(&handle).len(), 1);
        forget(&handle, &wrong.id);
        forget(&handle, &wrong.id); // nothing to forget: nothing written

        for i in 0..25 {
            remember(&handle, &SharedLink { id: format!("id-{i}"), ..link.clone() });
        }
        let history = history(&handle);
        assert_eq!(history.len(), HISTORY_LIMIT);
        assert_eq!(history[0].id, "id-24");
        assert_eq!(history[19].id, "id-5");
        // A broken file reads as empty.
        std::fs::write(history_file(&handle).unwrap(), "{ nope").unwrap();
        assert!(super::history(&handle).is_empty());
    }

    #[test]
    fn share_png_copies_the_link_and_announces_it() {
        let fake = Fake::new(1000, 5000);
        let base = serve_fake(&fake);
        let app = app_for(&base, "share-announce");
        let handle = app.handle().clone();
        assert_eq!(notice(&handle), None);

        let result = run(share_png(&handle, png(10))).unwrap();
        assert_eq!(result.share_url, "https://socorin.com/s/med-0123456789abcdefgh");
        assert_eq!(result.delete_token.len(), 43);
        assert_eq!(handle.state::<ShareState>().copied.lock().unwrap().as_slice(), &[result.share_url.clone()]);
        let Some(Notice::Shared { link, retention_days }) = notice(&handle) else { panic!() };
        assert_eq!(link.share_url, result.share_url);
        assert_eq!(retention_days, 60);
        assert!(handle.get_webview_window(windows::SHARE).is_some(), "the popover window exists");
        let json = serde_json::to_value(notice(&handle).unwrap()).unwrap();
        assert_eq!(json["kind"], "shared");
        assert_eq!(json["retentionDays"], 60);
        let init = &fake.requests("POST", "/api/upload/init")[0];
        let body: serde_json::Value = serde_json::from_slice(&init.body).unwrap();
        assert!(body["filename"].as_str().unwrap().starts_with("Socorin_"));
        dismiss(&handle);

        // A failure is announced as well, and nothing is copied.
        point_at(&handle, "http://127.0.0.1:9");
        let e = run(share_png(&handle, png(10))).unwrap_err();
        assert_eq!(e.code, "network");
        let Some(Notice::Failed { error }) = notice(&handle) else { panic!() };
        assert_eq!(error, e);
        assert!(handle.state::<ShareState>().copied.lock().unwrap().is_empty());
        let json = serde_json::to_value(notice(&handle).unwrap()).unwrap();
        assert_eq!(json["kind"], "failed");
        assert_eq!(json["error"]["code"], "network");

        // The asynchronous flavour ("after capture: upload") reports the same way.
        point_at(&handle, &base);
        share_png_async(&handle, png(10));
        for _ in 0..300 {
            if matches!(notice(&handle), Some(Notice::Shared { .. })) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(notice(&handle), Some(Notice::Shared { .. })));
        dismiss(&handle);
    }
}
