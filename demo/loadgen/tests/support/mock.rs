//! In-memory mock of the blog API (the spec's HTTP contract), with random
//! latency and an optional Lambda-style concurrency limit that answers 429.
//!
//! Used by the integration tests and by `cargo run --example mock_server`.
//! Several "Regions" can share one dataset, like a multi-Region cluster.
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

pub const CATEGORIES: &[&str] = &[
    "databases",
    "distributed-systems",
    "serverless",
    "postgres",
    "rust",
    "performance",
    "architecture",
    "devops",
];

#[derive(Clone, Debug)]
pub struct MockConfig {
    pub region: String,
    /// Simulated handler time, uniform in this range (ms).
    pub latency_ms: (u64, u64),
    /// Like Lambda's concurrency limit: beyond this many in flight, 429.
    pub max_concurrency: Option<usize>,
}

impl MockConfig {
    pub fn region(region: &str) -> Self {
        Self {
            region: region.into(),
            latency_ms: (1, 5),
            max_concurrency: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SeedConfig {
    pub users: usize,
    pub posts: usize,
    /// Posts per identical `created_at`, like rows bulk-loaded in one transaction.
    pub same_timestamp_batch: usize,
}

impl Default for SeedConfig {
    fn default() -> Self {
        Self {
            users: 200,
            posts: 5000,
            same_timestamp_batch: 1,
        }
    }
}

struct User {
    id: String,
    username: String,
    display_name: String,
}

struct Post {
    id: String,
    author: usize,
    slug: String,
    title: String,
    body: String,
    category: String,
    tags: Vec<String>,
    reading_minutes: u32,
    created_ms: i64,
}

struct Comment {
    id: String,
    user: usize,
    body: String,
    created_ms: i64,
}

struct Posts {
    /// Ascending by created_ms.
    list: Vec<Post>,
    index: HashMap<String, usize>,
}

/// The "cluster": shared by every Region's server.
pub struct Shared {
    users: Vec<User>,
    user_index: HashMap<String, usize>,
    posts: RwLock<Posts>,
    comments: Mutex<HashMap<String, Vec<Comment>>>,
    pub comments_created: AtomicU64,
    pub posts_created: AtomicU64,
}

#[derive(Default)]
pub struct Counters {
    pub requests: AtomicU64,
    pub throttled: AtomicU64,
    pub list: AtomicU64,
    pub list_category: AtomicU64,
    pub get_post: AtomicU64,
    pub not_found: AtomicU64,
    pub bad_request: AtomicU64,
    pub create_comment: AtomicU64,
    pub create_post: AtomicU64,
    pub in_flight: AtomicUsize,
    pub max_in_flight: AtomicUsize,
}

pub struct MockState {
    cfg: MockConfig,
    shared: Arc<Shared>,
    pub counters: Counters,
}

pub struct MockServer {
    pub addr: SocketAddr,
    pub state: Arc<MockState>,
}

impl MockServer {
    pub fn url(&self) -> String {
        format!("http://{}/", self.addr)
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn uuid(rng: &mut fastrand::Rng) -> String {
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        rng.u32(..),
        rng.u16(..),
        rng.u16(..0x1000),
        0x8000 | rng.u16(..0x4000),
        rng.u64(..1 << 48)
    )
}

const WORDS: &[&str] = &[
    "aurora", "dsql", "scaling", "postgres", "regions", "latency", "commit", "snapshot", "lambda",
    "quorum", "journal", "storage", "index", "query", "cluster", "witness",
];

fn words(rng: &mut fastrand::Rng, n: usize) -> String {
    (0..n)
        .map(|_| WORDS[rng.usize(..WORDS.len())])
        .collect::<Vec<_>>()
        .join(" ")
}

impl Shared {
    pub fn seed(seed: SeedConfig) -> Arc<Shared> {
        let mut rng = fastrand::Rng::with_seed(2026);
        let users: Vec<User> = (0..seed.users)
            .map(|i| User {
                id: uuid(&mut rng),
                username: format!("user{i:03}"),
                display_name: format!("User {i}"),
            })
            .collect();
        let user_index = users
            .iter()
            .enumerate()
            .map(|(i, u)| (u.id.clone(), i))
            .collect();
        let step = 37 * 60 * 1000;
        let batch = seed.same_timestamp_batch.max(1);
        let base = now_ms() - (seed.posts as i64 + 1) * step;
        let list: Vec<Post> = (0..seed.posts)
            .map(|i| {
                let title = format!("{} {i}", words(&mut rng, 4));
                Post {
                    id: uuid(&mut rng),
                    author: rng.usize(..seed.users.max(1)),
                    slug: title.replace(' ', "-"),
                    body: words(&mut rng, 120),
                    title,
                    category: CATEGORIES[rng.usize(..CATEGORIES.len())].to_string(),
                    tags: vec![words(&mut rng, 1), words(&mut rng, 1)],
                    reading_minutes: rng.u32(2..15),
                    created_ms: base + (i / batch) as i64 * step,
                }
            })
            .collect();
        let index = list
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.clone(), i))
            .collect();
        Arc::new(Shared {
            users,
            user_index,
            posts: RwLock::new(Posts { list, index }),
            comments: Mutex::new(HashMap::new()),
            comments_created: AtomicU64::new(0),
            posts_created: AtomicU64::new(0),
        })
    }

    fn user_json(&self, i: usize) -> Value {
        let u = &self.users[i];
        json!({ "id": u.id, "username": u.username, "displayName": u.display_name })
    }

    fn summary(&self, p: &Post) -> Value {
        json!({
            "id": p.id,
            "slug": p.slug,
            "title": p.title,
            "excerpt": p.body.chars().take(140).collect::<String>(),
            "category": p.category,
            "tags": p.tags,
            "readingMinutes": p.reading_minutes,
            "createdAt": iso(p.created_ms),
            "author": self.user_json(p.author),
        })
    }

    fn detail(&self, p: &Post) -> Value {
        let mut v = self.summary(p);
        v["body"] = json!(p.body);
        v["metadata"] = json!({
            "category": p.category,
            "tags": p.tags,
            "readingMinutes": p.reading_minutes,
            "seo": { "description": p.title },
        });
        v
    }
}

// --- ISO-8601 without a date crate (Howard Hinnant's civil algorithms) ---

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn iso(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        (rem / 60_000) % 60,
        (rem / 1000) % 60,
        rem % 1000
    )
}

pub fn parse_iso(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, sec) = (
        n(0..4)?,
        n(5..7)?,
        n(8..10)?,
        n(11..13)?,
        n(14..16)?,
        n(17..19)?,
    );
    let mut ms = 0;
    let rest = &s[19..];
    let rest = if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(|c| c.is_ascii_digit()).collect();
        let padded = format!("{digits:0<3}");
        ms = padded[..3].parse().ok()?;
        &frac[digits.len()..]
    } else {
        rest
    };
    if !(rest == "Z" || rest == "+00:00" || rest.is_empty()) {
        return None;
    }
    let days = days_from_civil(y, mo as u32, d as u32);
    Some(days * 86_400_000 + h * 3_600_000 + mi * 60_000 + sec * 1000 + ms)
}

// --- server ---

pub async fn start(
    cfg: MockConfig,
    shared: Arc<Shared>,
    addr: SocketAddr,
) -> std::io::Result<MockServer> {
    let state = Arc::new(MockState {
        cfg,
        shared,
        counters: Counters::default(),
    });
    let app = Router::new()
        .route("/health", get(health))
        .route("/users", get(users))
        .route("/categories", get(categories))
        .route("/posts", get(list_posts).post(create_post))
        .route("/posts/{id}", get(get_post))
        .route("/posts/{id}/comments", axum::routing::post(create_comment))
        .layer(middleware::from_fn_with_state(state.clone(), gate))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(MockServer { addr, state })
}

struct InFlight<'a>(&'a AtomicUsize);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Relaxed);
    }
}

/// Counts requests, enforces the concurrency limit, adds `x-region`.
async fn gate(State(state): State<Arc<MockState>>, req: Request, next: Next) -> Response {
    let c = &state.counters;
    c.requests.fetch_add(1, Relaxed);
    let n = c.in_flight.fetch_add(1, Relaxed) + 1;
    let _guard = InFlight(&c.in_flight);
    if let Some(max) = state.cfg.max_concurrency
        && n > max
    {
        c.throttled.fetch_add(1, Relaxed);
        let mut r = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "Message": "Rate Exceeded." })),
        )
            .into_response();
        r.headers_mut().insert(
            "x-amzn-errortype",
            HeaderValue::from_static("TooManyRequestsException"),
        );
        return r;
    }
    c.max_in_flight.fetch_max(n, Relaxed);
    let mut resp = next.run(req).await;
    if let Ok(v) = HeaderValue::from_str(&state.cfg.region) {
        resp.headers_mut().insert("x-region", v);
    }
    resp
}

/// Sleep for the simulated handler time; return the "DB" share of it.
async fn simulate(state: &MockState) -> f64 {
    let (lo, hi) = state.cfg.latency_ms;
    let total = if hi > lo { fastrand::u64(lo..=hi) } else { lo };
    if total > 0 {
        tokio::time::sleep(Duration::from_millis(total)).await;
    }
    let db = total as f64 * (0.2 + fastrand::f64() * 0.3);
    (db * 100.0).round() / 100.0
}

fn reply(state: &MockState, status: StatusCode, mut body: Value, db_ms: f64) -> Response {
    body["region"] = json!(state.cfg.region);
    body["timing"] = json!({ "dbMs": db_ms });
    (status, Json(body)).into_response()
}

fn error(state: &MockState, status: StatusCode, msg: &str, db_ms: f64) -> Response {
    reply(state, status, json!({ "error": msg }), db_ms)
}

async fn health(State(state): State<Arc<MockState>>) -> Response {
    let db = simulate(&state).await;
    reply(&state, StatusCode::OK, json!({ "ok": true }), db)
}

#[derive(Deserialize)]
struct LimitQuery {
    limit: Option<usize>,
}

async fn users(State(state): State<Arc<MockState>>, Query(q): Query<LimitQuery>) -> Response {
    let db = simulate(&state).await;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let users: Vec<Value> = (0..state.shared.users.len().min(limit))
        .map(|i| state.shared.user_json(i))
        .collect();
    reply(&state, StatusCode::OK, json!({ "users": users }), db)
}

async fn categories(State(state): State<Arc<MockState>>) -> Response {
    let db = simulate(&state).await;
    reply(
        &state,
        StatusCode::OK,
        json!({ "categories": CATEGORIES }),
        db,
    )
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<usize>,
    category: Option<String>,
    before: Option<String>,
}

async fn list_posts(State(state): State<Arc<MockState>>, Query(q): Query<ListQuery>) -> Response {
    if q.category.is_some() {
        state.counters.list_category.fetch_add(1, Relaxed);
    } else {
        state.counters.list.fetch_add(1, Relaxed);
    }
    let db = simulate(&state).await;
    let before = match q.before.as_deref() {
        Some(b) => match parse_iso(b) {
            Some(ms) => Some(ms),
            None => return error(&state, StatusCode::BAD_REQUEST, "bad before", db),
        },
        None => None,
    };
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let shared = &state.shared;
    let posts = shared.posts.read().unwrap();
    let end = match before {
        Some(b) => posts.list.partition_point(|p| p.created_ms < b),
        None => posts.list.len(),
    };
    let page: Vec<Value> = posts.list[..end]
        .iter()
        .rev()
        .filter(|p| q.category.as_deref().is_none_or(|c| p.category == c))
        .take(limit)
        .map(|p| shared.summary(p))
        .collect();
    drop(posts);
    reply(&state, StatusCode::OK, json!({ "posts": page }), db)
}

async fn get_post(State(state): State<Arc<MockState>>, Path(id): Path<String>) -> Response {
    state.counters.get_post.fetch_add(1, Relaxed);
    let db = simulate(&state).await;
    let shared = &state.shared;
    let post = {
        let posts = shared.posts.read().unwrap();
        match posts.index.get(&id) {
            Some(i) => shared.detail(&posts.list[*i]),
            None => {
                state.counters.not_found.fetch_add(1, Relaxed);
                return error(&state, StatusCode::NOT_FOUND, "post not found", db);
            }
        }
    };
    let comments: Vec<Value> = shared
        .comments
        .lock()
        .unwrap()
        .get(&id)
        .map(|cs| {
            cs.iter()
                .map(|c| {
                    json!({
                        "id": c.id, "body": c.body, "createdAt": iso(c.created_ms),
                        "user": shared.user_json(c.user),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    reply(
        &state,
        StatusCode::OK,
        json!({ "post": post, "comments": comments }),
        db,
    )
}

fn str_field<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

async fn create_post(State(state): State<Arc<MockState>>, Json(body): Json<Value>) -> Response {
    state.counters.create_post.fetch_add(1, Relaxed);
    let db = simulate(&state).await;
    let shared = &state.shared;
    let (Some(author), Some(title), Some(text), Some(category)) = (
        str_field(&body, "authorId"),
        str_field(&body, "title"),
        str_field(&body, "body"),
        str_field(&body, "category"),
    ) else {
        state.counters.bad_request.fetch_add(1, Relaxed);
        return error(
            &state,
            StatusCode::BAD_REQUEST,
            "authorId, title, body and category are required",
            db,
        );
    };
    let Some(&author) = shared.user_index.get(author) else {
        state.counters.bad_request.fetch_add(1, Relaxed);
        return error(&state, StatusCode::BAD_REQUEST, "unknown author", db);
    };
    let tags = body
        .get("tags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let mut rng = fastrand::Rng::new();
    let id = uuid(&mut rng);
    let post = Post {
        slug: format!(
            "{}-{}",
            title
                .to_lowercase()
                .replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
            &id[..8]
        ),
        id: id.clone(),
        author,
        title: title.to_string(),
        body: text.to_string(),
        category: category.to_string(),
        tags,
        reading_minutes: 3,
        created_ms: now_ms(),
    };
    let detail = shared.detail(&post);
    {
        let mut posts = shared.posts.write().unwrap();
        let created = post
            .created_ms
            .max(posts.list.last().map_or(0, |p| p.created_ms));
        let i = posts.list.len();
        posts.index.insert(id, i);
        posts.list.push(Post {
            created_ms: created,
            ..post
        });
    }
    shared.posts_created.fetch_add(1, Relaxed);
    reply(&state, StatusCode::CREATED, json!({ "post": detail }), db)
}

async fn create_comment(
    State(state): State<Arc<MockState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    state.counters.create_comment.fetch_add(1, Relaxed);
    let db = simulate(&state).await;
    let shared = &state.shared;
    if !shared.posts.read().unwrap().index.contains_key(&id) {
        state.counters.not_found.fetch_add(1, Relaxed);
        return error(&state, StatusCode::NOT_FOUND, "post not found", db);
    }
    let (Some(user), Some(text)) = (str_field(&body, "userId"), str_field(&body, "body")) else {
        state.counters.bad_request.fetch_add(1, Relaxed);
        return error(
            &state,
            StatusCode::BAD_REQUEST,
            "userId and body are required",
            db,
        );
    };
    let Some(&user) = shared.user_index.get(user) else {
        state.counters.bad_request.fetch_add(1, Relaxed);
        return error(&state, StatusCode::BAD_REQUEST, "unknown user", db);
    };
    let comment = Comment {
        id: uuid(&mut fastrand::Rng::new()),
        user,
        body: text.to_string(),
        created_ms: now_ms(),
    };
    let out = json!({
        "id": comment.id, "body": comment.body, "createdAt": iso(comment.created_ms),
        "user": shared.user_json(user),
    });
    {
        let mut comments = shared.comments.lock().unwrap();
        let list = comments.entry(id).or_default();
        // Like the API's `limit 200`: keep memory bounded on long runs.
        if list.len() < 200 {
            list.push(comment);
        }
    }
    shared.comments_created.fetch_add(1, Relaxed);
    reply(&state, StatusCode::CREATED, json!({ "comment": out }), db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trips() {
        for ms in [0, 1_760_000_000_123, 951_782_400_000, 4_102_444_799_999] {
            assert_eq!(parse_iso(&iso(ms)), Some(ms), "{}", iso(ms));
        }
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            parse_iso("2026-10-14T19:06:43.1Z"),
            parse_iso("2026-10-14T19:06:43.100Z")
        );
        assert_eq!(parse_iso("nope"), None);
    }
}
