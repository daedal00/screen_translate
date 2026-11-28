use std::time::Duration;

use serde::Deserialize;

use super::Translation;

/// Shared HTTP client: bounded timeouts so a stalled connection can't hang a translation.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
}

/// Free keys (`:fx` suffix) use a different host than Pro keys.
pub fn base_for_key(key: &str) -> &'static str {
    if key.ends_with(":fx") {
        "https://api-free.deepl.com"
    } else {
        "https://api.deepl.com"
    }
}

pub struct DeepL {
    key: String,
    base: String,
    client: reqwest::Client,
}

#[derive(Deserialize)]
struct Response {
    translations: Vec<Item>,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: Option<String>,
}

#[derive(Deserialize)]
struct Item {
    text: String,
    detected_source_language: Option<String>,
}

impl DeepL {
    pub fn new(key: String) -> Self {
        let base = base_for_key(&key).into();
        Self::with_base_url(key, base)
    }

    pub fn with_base_url(key: String, base: String) -> Self {
        Self::with_client(key, base, http_client())
    }

    pub fn with_client(key: String, base: String, client: reqwest::Client) -> Self {
        Self { key, base, client }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub async fn translate(&self, text: &str, target: &str) -> Result<Translation, String> {
        if text.trim().is_empty() {
            return Ok(Translation {
                text: String::new(),
                detected_source: None,
            });
        }
        self.post(&[text.to_owned()], target)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| "DeepL returned no translation".into())
    }

    /// Translate all `texts` in one request, in order. Empty input makes no request.
    pub async fn translate_batch(
        &self,
        texts: &[String],
        target: &str,
    ) -> Result<Vec<Translation>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let items = self.post(texts, target).await?;
        if items.len() != texts.len() {
            return Err(format!(
                "DeepL returned {} translations for {} texts",
                items.len(),
                texts.len()
            ));
        }
        Ok(items)
    }

    async fn post(&self, texts: &[String], target: &str) -> Result<Vec<Translation>, String> {
        let resp = self
            .client
            .post(format!("{}/v2/translate", self.base))
            .header("Authorization", format!("DeepL-Auth-Key {}", self.key))
            .json(&serde_json::json!({ "text": texts, "target_lang": target }))
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    "DeepL request timed out".to_string()
                } else {
                    format!("DeepL request failed: {}", e.without_url())
                }
            })?;
        let status = resp.status();
        match status.as_u16() {
            200..=299 => {}
            403 => return Err("DeepL rejected the API key (403). Check it in Settings.".into()),
            456 => return Err("DeepL quota exceeded (456).".into()),
            429 => return Err("DeepL rate limit hit (429), try again shortly.".into()),
            code => {
                let message = resp.json::<ErrorBody>().await.ok().and_then(|b| b.message);
                return Err(match message {
                    Some(m) => format!("DeepL returned HTTP {code}: {m}"),
                    None => format!("DeepL returned HTTP {code}"),
                });
            }
        }
        let body: Response = resp.json().await.map_err(|e| {
            if e.is_timeout() {
                "DeepL request timed out".to_string()
            } else {
                format!("bad DeepL response: {}", e.without_url())
            }
        })?;
        Ok(body
            .translations
            .into_iter()
            .map(|item| Translation {
                text: item.text,
                detected_source: item.detected_source_language,
            })
            .collect())
    }
}
