//! Sending requests: shared context, per-Region targets, the open-loop pacer,
//! closed-loop workers and the post-id refresher.

use std::collections::VecDeque;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use reqwest::header::{CONTENT_TYPE, HeaderName, HeaderValue};
use reqwest::{Client, RequestBuilder, Url, Version};
use tokio::time::MissedTickBehavior;

use crate::api;
use crate::control::{Controller, Pacer};
use crate::mix::{Available, Mix, Op, PPM, fallback};
use crate::pool::IdPool;
use crate::stats::{Class, Stats, classify_status};
use crate::text;
use crate::util::fmt_clock;

const ERROR_TYPE: HeaderName = HeaderName::from_static("x-amzn-errortype");
const JSON: HeaderValue = HeaderValue::from_static("application/json");
const CONTENT_SHA256: HeaderName = HeaderName::from_static("x-amz-content-sha256");

/// Hex SHA-256 of a request body. CloudFront's origin access control signs the
/// request to the Lambda function URL but can't hash the body, so POSTs carry
/// the hash in x-amz-content-sha256 (harmless when calling an API directly).
pub fn sha256_hex(body: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(body);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn json_post<U: reqwest::IntoUrl>(client: &reqwest::Client, url: U, body: String) -> reqwest::RequestBuilder {
    let hash = sha256_hex(body.as_bytes());
    client
        .post(url)
        .header(CONTENT_TYPE, JSON)
        .header(CONTENT_SHA256, hash)
        .body(body)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proto {
    Auto,
    Http1,
    H2c,
}

#[derive(Clone, Debug)]
pub struct HttpSettings {
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub proto: Proto,
}

impl HttpSettings {
    pub fn client(&self, max_idle_per_host: usize) -> Result<Client> {
        let b = Client::builder()
            .user_agent(concat!("dat401-loadgen/", env!("CARGO_PKG_VERSION")))
            .timeout(self.timeout)
            .connect_timeout(self.connect_timeout)
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(max_idle_per_host)
            .tcp_nodelay(true)
            .no_proxy();
        let b = match self.proto {
            Proto::Auto => b,
            Proto::Http1 => b.http1_only(),
            Proto::H2c => b.http2_prior_knowledge(),
        };
        Ok(b.build()?)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Open loop: requests/s.
    Rate,
    /// Closed loop: workers.
    Concurrency,
}

/// Keeps hot atomics on their own cache line.
#[repr(align(128))]
#[derive(Default)]
pub struct Padded<T>(pub T);

pub struct Target {
    pub label: String,
    pub base: String,
    pub version: Version,
    clients: Vec<Client>,
    list_url: Url,
    category_urls: Vec<Url>,
    create_post_url: Url,
    /// Max in-flight requests (= connections with HTTP/1.1).
    pub cap: usize,
    pub in_flight: Padded<AtomicUsize>,
    /// Open-loop sends skipped because `cap` was reached.
    pub skipped: AtomicU64,
}

impl Target {
    pub fn new(
        label: String,
        base: String,
        version: Version,
        http: &HttpSettings,
        cap: usize,
        categories: &[String],
        page_size: u32,
    ) -> Result<Self> {
        // HTTP/1.1: one request per connection, and a single pool per Region
        // so every idle connection can be reused. (Spreading requests over
        // several clients was measured to leave idle connections stranded in
        // one pool while another opened new ones: connection churn, which
        // means extra TLS handshakes against real function URLs.)
        // HTTP/2: hyper never opens a second connection to a host, so use one
        // client per ~100 streams.
        let (n_clients, idle) = if version == Version::HTTP_2 {
            (cap.div_ceil(100).max(1), 1)
        } else {
            (1, cap)
        };
        let clients = (0..n_clients)
            .map(|_| http.client(idle))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            list_url: api::posts_url(&base, page_size, None, None)?,
            category_urls: categories
                .iter()
                .map(|c| api::posts_url(&base, page_size, Some(c), None))
                .collect::<Result<_>>()?,
            create_post_url: api::url(&base, "posts")?,
            label,
            base,
            version,
            clients,
            cap,
            in_flight: Padded(AtomicUsize::new(0)),
            skipped: AtomicU64::new(0),
        })
    }

    fn client(&self) -> &Client {
        if self.clients.len() == 1 {
            &self.clients[0]
        } else {
            &self.clients[fastrand::usize(..self.clients.len())]
        }
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.0.load(Relaxed)
    }
}

/// Things worth telling the presenter: back-offs, warnings, new posts.
pub struct EventLog {
    start: Instant,
    plain: bool,
    lines: Mutex<VecDeque<String>>,
}

impl EventLog {
    pub fn new(start: Instant, plain: bool) -> Self {
        Self {
            start,
            plain,
            lines: Mutex::new(VecDeque::new()),
        }
    }

    pub fn push(&self, msg: impl Into<String>) {
        let secs = Instant::now()
            .saturating_duration_since(self.start)
            .as_secs_f64();
        let line = format!("{} {}", fmt_clock(secs), msg.into());
        if self.plain {
            println!("# {line}");
        }
        let mut lines = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        lines.push_back(line);
        while lines.len() > 200 {
            lines.pop_front();
        }
    }

    pub fn recent(&self, n: usize) -> Vec<String> {
        let lines = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        lines
            .iter()
            .skip(lines.len().saturating_sub(n))
            .cloned()
            .collect()
    }
}

pub struct Ctx {
    pub targets: Vec<Target>,
    pub pool: IdPool,
    pub categories: Vec<String>,
    pub mix: Mix,
    pub stats: Stats,
    pub controllers: Vec<Mutex<Controller>>,
    /// Closed loop: workers spawned per target.
    pub workers: Vec<usize>,
    /// Closed loop: workers currently allowed to send, per target.
    pub active: Vec<Padded<AtomicUsize>>,
    pub mode: Mode,
    pub start: Instant,
    pub stop: AtomicBool,
    pub events: EventLog,
}

impl Ctx {
    pub fn secs(&self, now: Instant) -> f64 {
        now.saturating_duration_since(self.start).as_secs_f64()
    }

    pub fn stopping(&self) -> bool {
        self.stop.load(Relaxed)
    }

    pub fn in_flight(&self) -> usize {
        self.targets.iter().map(Target::in_flight).sum()
    }

    pub fn controller(&self, t: usize) -> std::sync::MutexGuard<'_, Controller> {
        self.controllers[t]
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

fn post_url(base: &str, id: &str, suffix: &str) -> String {
    let mut s = String::with_capacity(base.len() + 6 + id.len() + suffix.len());
    s.push_str(base);
    s.push_str("posts/");
    s.push_str(id);
    s.push_str(suffix);
    s
}

fn classify_error(e: &reqwest::Error) -> Class {
    if e.is_timeout() {
        Class::Timeout
    } else {
        Class::Connect
    }
}

/// Build the request for `op`, falling back to an op that the id pool can
/// support. Returns the op actually used.
fn build(ctx: &Ctx, target: &Target, op: Op) -> (Op, RequestBuilder) {
    let have = Available {
        posts: true,
        users: ctx.pool.user_count() > 0,
        categories: !ctx.categories.is_empty(),
    };
    let mut op = fallback(op, have);
    let post = if matches!(op, Op::GetPost | Op::CreateComment) {
        let p = ctx.pool.random_post();
        if p.is_none() {
            op = fallback(
                op,
                Available {
                    posts: false,
                    ..have
                },
            );
        }
        p
    } else {
        None
    };
    let client = target.client();
    let user = ctx.pool.random_user().unwrap_or("");
    match (op, post.as_deref()) {
        (Op::GetPost, Some(id)) => (op, client.get(post_url(&target.base, id, ""))),
        (Op::CreateComment, Some(id)) => {
            let mut body = String::with_capacity(192);
            text::comment_json(&mut body, user);
            (op, json_post(client, post_url(&target.base, id, "/comments"), body))
        }
        (Op::CreatePost, _) => {
            let category = if ctx.categories.is_empty() {
                "general"
            } else {
                &ctx.categories[fastrand::usize(..ctx.categories.len())]
            };
            let body = text::post_json(user, category);
            (op, json_post(client, target.create_post_url.clone(), body))
        }
        (Op::ListByCategory, _) => {
            let u = &target.category_urls[fastrand::usize(..target.category_urls.len())];
            (op, client.get(u.clone()))
        }
        _ => (Op::ListPosts, client.get(target.list_url.clone())),
    }
}

/// Send one request to target `t` and record the outcome.
pub async fn run_one(ctx: &Ctx, t: usize, op: Op) -> Class {
    let target = &ctx.targets[t];
    let (op, req) = build(ctx, target, op);
    let started = Instant::now();
    let (class, db_us) = match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let class = if (200..300).contains(&status) {
                Class::Ok
            } else {
                classify_status(status, resp.headers().get(ERROR_TYPE).map(|v| v.as_bytes()))
            };
            match resp.bytes().await {
                Ok(body) => {
                    let db = if class == Class::Ok {
                        api::extract_db_ms(&body)
                    } else {
                        None
                    };
                    if class == Class::Ok
                        && op == Op::CreatePost
                        && let Some(id) = api::created_post_id(&body)
                    {
                        ctx.pool.add_post(id);
                    }
                    (class, db.map(|ms| (ms * 1000.0).round() as u64))
                }
                Err(e) => (classify_error(&e), None),
            }
        }
        Err(e) => (classify_error(&e), None),
    };
    let latency_us = started.elapsed().as_micros() as u64;
    ctx.stats.record(t, op, class, latency_us, db_us);
    class
}

/// Open loop: send at the controller's rate, whatever the latency.
pub async fn pacer(ctx: Arc<Ctx>, t: usize) {
    let mut tick = tokio::time::interval(Duration::from_millis(1));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut pacer = Pacer::default();
    let mut last = Instant::now();
    let target = &ctx.targets[t];
    while !ctx.stopping() {
        tick.tick().await;
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f64();
        last = now;
        let rate = ctx.controller(t).rate(ctx.secs(now) - dt / 2.0);
        let n = pacer.advance(rate, dt);
        let mut skipped = 0;
        for _ in 0..n {
            if target.in_flight() >= target.cap {
                skipped += 1;
                continue;
            }
            target.in_flight.0.fetch_add(1, Relaxed);
            let op = ctx.mix.pick(fastrand::u32(..PPM));
            let ctx = ctx.clone();
            tokio::spawn(async move {
                run_one(&ctx, t, op).await;
                ctx.targets[t].in_flight.0.fetch_sub(1, Relaxed);
            });
        }
        if skipped > 0 {
            target.skipped.fetch_add(skipped, Relaxed);
        }
    }
}

/// Closed loop: one request at a time, while this worker is active.
pub async fn worker(ctx: Arc<Ctx>, t: usize, id: usize) {
    let target = &ctx.targets[t];
    while !ctx.stopping() {
        if id >= ctx.active[t].0.load(Relaxed) {
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        }
        target.in_flight.0.fetch_add(1, Relaxed);
        let class = run_one(&ctx, t, ctx.mix.pick(fastrand::u32(..PPM))).await;
        target.in_flight.0.fetch_sub(1, Relaxed);
        if class == Class::Throttled {
            // Don't spin on fast 429s; the controller handles the rest.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// Re-read the newest posts now and then so posts created during the run
/// (by us, or by people using the web UI) get read too.
pub async fn refresher(ctx: Arc<Ctx>, client: Client, every: Duration) {
    let mut i = 0usize;
    loop {
        tokio::time::sleep(every).await;
        if ctx.stopping() {
            return;
        }
        let target = &ctx.targets[i % ctx.targets.len()];
        i += 1;
        match api::posts_page(&client, &target.base, api::BOOTSTRAP_PAGE, None, None).await {
            Ok(posts) => {
                let added = ctx.pool.add_posts(posts.into_iter().map(|p| p.id));
                if added > 0 {
                    ctx.events.push(format!(
                        "{}: picked up {added} new post(s) from the feed (pool {})",
                        target.label,
                        ctx.pool.post_count()
                    ));
                }
            }
            Err(e) => ctx
                .events
                .push(format!("{}: refreshing posts failed: {e:#}", target.label)),
        }
    }
}

#[cfg(test)]
mod sha_tests {
    #[test]
    fn sha256_hex_matches_known_vectors() {
        assert_eq!(
            super::sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            super::sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
