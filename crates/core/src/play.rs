//! What playing needs from Crunchyroll besides crunchyroll-rs' stream: the Widevine license, posted to Crunchyroll's
//! license proxy with the stream's token, and that server's service certificate (requests are encrypted to it).

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use crunchyroll_rs::Crunchyroll;

const LICENSE_URL: &str = "https://cr-license-proxy.prd.crunchyrollsvc.com/v1/license/widevine";

/// The license server's Widevine service certificate (public: Crunchyroll's web players ship it).
const SERVICE_CERTIFICATE: &str = concat!(
    "CrsCCAMSEKDc0WAwLAQT1SB2ogyBJEwYv4Tx7gUijgIwggEKAoIBAQC8Xc/GTRwZDtlnBThq8V382D1oJAM0F/YgCQtNDLz7vTWJ+QskNGi5",
    "Dd2qzO4s48Cnx5BLvL4H0xCRSw2Ed6ekHSdrRUwyoYOE+M/t1oIbccwlTQ7o+BpV1X6TB7fxFyx1jsBtRsBWphU65w121zqmSiwzZzJ4xsXV",
    "QCJpQnNI61gzHO42XZOMuxytMm0F6puNHTTqhyY3Z290YqvSDdOB+UY5QJuXJgjhvOUD9+oaLlvT+vwmV2/NJWxKqHBKdL9JqvOnNiQUF0hD",
    "I7Wf8Wb63RYSXKE27Ky31hKgx1wuq7TTWkA+kHnJTUrTEfQxfPR4dJTquE+IDLAi5yeVVxzbAgMBAAE6DGNhc3RsYWJzLmNvbUABEoADMmGX",
    "pXg/0qxUuwokpsqVIHZrJfu62ar+BF8UVUKdK5oYQoiTZd9OzK3kr29kqGGk3lSgM0/p499p/FUL8oHHzgsJ7Hajdsyzn0Vs3+VysAgaJAkX",
    "Z+k+N6Ka0WBiZlCtcunVJDiHQbz1sF9GvcePUUi2fM/h7hyskG5ZLAyJMzTvgnV3D8/I5Y6mCFBPb/+/Ri+9bEvquPF3Ff9ip3yEHu9mcQeE",
    "YCeGe9zR/27eI5MATX39gYtCnn7dDXVxo4/rCYK0A4VemC3HRai2X3pSGcsKY7+6we7h4IycjqtuGtYg8AbaigovcoURAZcr1d/G0rpREjLd",
    "VLG0Gjqk63Gx688W5gh3TKemsK3R1jV0dOfj3e6uV/kTpsNRL9KsD0v7ysBQVdUXEbJotcFz71tI5qc3jwr6GjYIPA3VzusD17PN6AGQniMw",
    "xJV12z/EgnUopcFB13osydpD2AaDsgWo5RWJcNf+fzCgtUQx/0Au9+xVm5LQBdv8Ja4f2oiHN3dw",
);

pub fn service_certificate() -> Vec<u8> {
    B64.decode(SERVICE_CERTIFICATE).expect("the service certificate is valid base64")
}

/// Posts the CDM's license request for `content_id`'s stream (`token` from the stream) and returns the license.
pub async fn widevine_license(
    crunchy: &Crunchyroll,
    url: &str,
    content_id: &str,
    token: &str,
    challenge: Vec<u8>,
) -> Result<Vec<u8>> {
    let url = if url.is_empty() { LICENSE_URL } else { url };
    let response = crunchy
        .client()
        .post(url)
        .bearer_auth(crunchy.access_token().await)
        .header("Content-Type", "application/octet-stream")
        .header("X-Cr-Content-Id", content_id)
        .header("X-Cr-Video-Token", token)
        .body(challenge)
        .send()
        .await
        .context("requesting the license")?;
    let status = response.status();
    let body = response.bytes().await.context("reading the license")?;
    if !status.is_success() {
        let text: String = String::from_utf8_lossy(&body).chars().take(200).collect();
        bail!("license server {status}: {text}");
    }
    // Crunchyroll wraps Widevine licenses in JSON
    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body)
        && let Some(license) = json.get("license").and_then(|license| license.as_str())
    {
        return B64.decode(license).context("decoding the license");
    }
    Ok(body.to_vec())
}
