//! The blog API as the load generator sees it: bootstrap calls, pagination and
//! cheap response parsing.

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use memchr::memmem::FinderRev;
use reqwest::{Client, Url, Version};
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// Page size used while collecting ids.
pub const BOOTSTRAP_PAGE: u32 = 100;

#[derive(Deserialize)]
struct HealthBody {
    #[serde(default)]
    region: Option<String>,
}

#[derive(Deserialize)]
struct UsersBody {
    users: Vec<IdOnly>,
}

#[derive(Deserialize)]
struct IdOnly {
    id: String,
}

#[derive(Deserialize)]
struct CategoriesBody {
    categories: Vec<String>,
}

#[derive(Deserialize)]
struct PostsBody {
    posts: Vec<PostRef>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PostRef {
    pub id: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Deserialize)]
struct CreatedPost {
    post: IdOnly,
}

pub struct Health {
    pub region: Option<String>,
    pub version: Version,
    pub latency: Duration,
}

pub fn url(base: &str, path: &str) -> Result<Url> {
    Url::parse(&format!("{base}{path}")).with_context(|| format!("bad URL {base}{path}"))
}

pub fn posts_url(
    base: &str,
    limit: u32,
    category: Option<&str>,
    before: Option<&str>,
) -> Result<Url> {
    let mut u = url(base, "posts")?;
    {
        let mut q = u.query_pairs_mut();
        q.append_pair("limit", &limit.to_string());
        if let Some(c) = category {
            q.append_pair("category", c);
        }
        if let Some(b) = before {
            q.append_pair("before", b);
        }
    }
    Ok(u)
}

async fn get_json<T: DeserializeOwned>(client: &Client, u: Url) -> Result<(T, Version)> {
    let shown = u.to_string();
    let resp = client
        .get(u)
        .send()
        .await
        .with_context(|| format!("GET {shown}"))?;
    let status = resp.status();
    let version = resp.version();
    let body = resp
        .bytes()
        .await
        .with_context(|| format!("GET {shown}: reading body"))?;
    if !status.is_success() {
        let snippet = String::from_utf8_lossy(&body[..body.len().min(200)]).into_owned();
        bail!("GET {shown}: HTTP {status}: {snippet}");
    }
    let parsed = serde_json::from_slice(&body).with_context(|| format!("GET {shown}: bad JSON"))?;
    Ok((parsed, version))
}

pub async fn health(client: &Client, base: &str) -> Result<Health> {
    let started = Instant::now();
    let (body, version): (HealthBody, _) = get_json(client, url(base, "health")?).await?;
    Ok(Health {
        region: body.region,
        version,
        latency: started.elapsed(),
    })
}

pub async fn users(client: &Client, base: &str, limit: u32) -> Result<Vec<String>> {
    let mut u = url(base, "users")?;
    u.query_pairs_mut().append_pair("limit", &limit.to_string());
    let (body, _): (UsersBody, _) = get_json(client, u).await?;
    Ok(body.users.into_iter().map(|u| u.id).collect())
}

pub async fn categories(client: &Client, base: &str) -> Result<Vec<String>> {
    let (body, _): (CategoriesBody, _) = get_json(client, url(base, "categories")?).await?;
    Ok(body.categories)
}

pub async fn posts_page(
    client: &Client,
    base: &str,
    limit: u32,
    category: Option<&str>,
    before: Option<&str>,
) -> Result<Vec<PostRef>> {
    let (body, _): (PostsBody, _) =
        get_json(client, posts_url(base, limit, category, before)?).await?;
    Ok(body.posts)
}

/// Walk a feed (all posts, or one category) newest first with `?before=`,
/// adding ids to `seen`/`out` until `want` ids or `max_pages` pages. Stops early
/// when a page brings nothing new: posts loaded in one transaction share a
/// `created_at`, and paging by timestamp skips the rest of such a batch.
pub async fn walk_feed(
    client: &Client,
    base: &str,
    category: Option<&str>,
    want: usize,
    max_pages: usize,
    seen: &mut HashSet<String>,
    out: &mut Vec<PostRef>,
) -> Result<()> {
    let mut before: Option<String> = None;
    for _ in 0..max_pages {
        if out.len() >= want {
            break;
        }
        let page = posts_page(client, base, BOOTSTRAP_PAGE, category, before.as_deref()).await?;
        let next = page.last().and_then(|p| p.created_at.clone());
        let mut new = 0;
        for p in page {
            if seen.insert(p.id.clone()) {
                out.push(p);
                new += 1;
            }
        }
        if new == 0 || next.is_none() || next == before {
            break;
        }
        before = next;
    }
    Ok(())
}

/// Collect up to `want` post ids: the main feed first, then a couple of pages
/// per category to reach older posts.
pub async fn collect_posts(
    client: &Client,
    base: &str,
    want: usize,
    categories: &[String],
) -> Result<Vec<PostRef>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    walk_feed(client, base, None, want, 1000, &mut seen, &mut out).await?;
    for c in categories {
        let target = out.len() + 2 * BOOTSTRAP_PAGE as usize;
        if let Err(e) = walk_feed(client, base, Some(c), target, 2, &mut seen, &mut out).await {
            eprintln!("  warning: category {c:?}: {e:#}");
        }
    }
    Ok(out)
}

static DB_MS: LazyLock<FinderRev<'static>> = LazyLock::new(|| FinderRev::new(b"\"dbMs\""));

fn skip_ws(b: &[u8]) -> &[u8] {
    let n = b.iter().take_while(|c| c.is_ascii_whitespace()).count();
    &b[n..]
}

/// Pull `timing.dbMs` out of a response body without parsing the JSON. The
/// last occurrence wins; inside a JSON string the quotes would be escaped, so
/// user content cannot fake it.
pub fn extract_db_ms(body: &[u8]) -> Option<f64> {
    let pos = DB_MS.rfind(body)?;
    let rest = skip_ws(&body[pos + 6..]);
    let rest = skip_ws(rest.strip_prefix(b":")?);
    let end = rest
        .iter()
        .position(|c| !(c.is_ascii_digit() || matches!(c, b'.' | b'-' | b'+' | b'e' | b'E')))
        .unwrap_or(rest.len());
    let v: f64 = std::str::from_utf8(&rest[..end]).ok()?.parse().ok()?;
    (v.is_finite() && v >= 0.0).then_some(v)
}

/// The id from a `201 { post }` response.
pub fn created_post_id(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<CreatedPost>(body)
        .ok()
        .map(|c| c.post.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_db_ms() {
        assert_eq!(
            extract_db_ms(br#"{"posts":[],"region":"us-east-1","timing":{"dbMs":3.25}}"#),
            Some(3.25)
        );
        assert_eq!(extract_db_ms(br#"{"timing": {"dbMs" : 12 }}"#), Some(12.0));
        assert_eq!(extract_db_ms(br#"{"timing":{"dbMs":1.5e1}}"#), Some(15.0));
        assert_eq!(extract_db_ms(br#"{"timing":{}}"#), None);
        assert_eq!(extract_db_ms(br#"{"timing":{"dbMs":null}}"#), None);
        assert_eq!(extract_db_ms(br#"{"timing":{"dbMs":-1}}"#), None);
        assert_eq!(extract_db_ms(b""), None);
    }

    #[test]
    fn db_ms_cannot_be_faked_by_content() {
        // A comment body containing the text "dbMs": 99 is escaped in JSON.
        let body = br#"{"timing":{"dbMs":4},"comments":[{"body":"x \"dbMs\": 99"}]}"#;
        assert_eq!(extract_db_ms(body), Some(4.0));
    }

    #[test]
    fn parses_created_post() {
        let body = br#"{"post":{"id":"abc","slug":"x"},"region":"local","timing":{"dbMs":2}}"#;
        assert_eq!(created_post_id(body), Some("abc".to_string()));
        assert_eq!(created_post_id(b"{}"), None);
    }

    #[test]
    fn builds_query_urls() {
        let u = posts_url(
            "https://x.lambda-url.us-east-1.on.aws/",
            20,
            Some("distributed systems"),
            Some("2026-10-14T19:06:43.123+00:00"),
        )
        .unwrap();
        assert_eq!(
            u.as_str(),
            "https://x.lambda-url.us-east-1.on.aws/posts?limit=20&category=distributed+systems&before=2026-10-14T19%3A06%3A43.123%2B00%3A00"
        );
    }
}
