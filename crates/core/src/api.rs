//! Requests crunchyroll-rs doesn't make (logos and ratings in bulk, playheads, up next, marking as watched), sent
//! with its client and its session's token. The token lives 5 minutes and the library only renews it on its own
//! requests, so a 401 here renews it with a cheap library call and tries once more.

use anyhow::{Context, Result, bail};
use crunchyroll_rs::Crunchyroll;
use reqwest::{Method, StatusCode};
use serde_json::Value;

pub const BASE: &str = "https://www.crunchyroll.com";

pub async fn get(crunchy: &Crunchyroll, path: &str, query: &[(&str, String)]) -> Result<Value> {
    send(crunchy, Method::GET, path, query, None).await
}

pub async fn post(crunchy: &Crunchyroll, path: &str, query: &[(&str, String)], body: Value) -> Result<Value> {
    send(crunchy, Method::POST, path, query, Some(body)).await
}

pub async fn patch(crunchy: &Crunchyroll, path: &str, query: &[(&str, String)], body: Value) -> Result<Value> {
    send(crunchy, Method::PATCH, path, query, Some(body)).await
}

pub async fn put(crunchy: &Crunchyroll, path: &str, query: &[(&str, String)], body: Value) -> Result<Value> {
    send(crunchy, Method::PUT, path, query, Some(body)).await
}

pub async fn delete(crunchy: &Crunchyroll, path: &str, query: &[(&str, String)]) -> Result<Value> {
    send(crunchy, Method::DELETE, path, query, None).await
}

async fn send(crunchy: &Crunchyroll, method: Method, path: &str, query: &[(&str, String)], body: Option<Value>) -> Result<Value> {
    let mut renewed = false;
    loop {
        let mut request = crunchy
            .client()
            .request(method.clone(), format!("{BASE}{path}"))
            .bearer_auth(crunchy.access_token().await)
            .query(query);
        if let Some(body) = &body {
            request = request.json(body);
        }
        let response = request.send().await.with_context(|| format!("{method} {path}"))?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED && !renewed {
            renewed = true;
            // any library request past the expiry renews the session first
            let _ = crunchy.account().await;
            continue;
        }
        let text = response.text().await.with_context(|| format!("{method} {path}"))?;
        if !status.is_success() {
            let detail: String = text.chars().take(160).collect();
            bail!("{method} {path}: {status} {detail}");
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        return serde_json::from_str(&text).with_context(|| format!("{method} {path}: unreadable answer"));
    }
}

/// `data` of a list answer, or nothing.
pub fn data(value: &Value) -> &[Value] {
    value.get("data").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

pub fn str<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}
