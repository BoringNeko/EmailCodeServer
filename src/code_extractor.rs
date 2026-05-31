use crate::config::LlmFallbackConfig;
use regex::Regex;

pub fn extract(subject: &str, body: &str, patterns: &[Regex]) -> Option<String> {
    for p in patterns {
        if let Some(m) = p.find(subject) {
            return Some(m.as_str().to_string());
        }
    }
    for p in patterns {
        if let Some(m) = p.find(body) {
            return Some(m.as_str().to_string());
        }
    }
    None
}

pub async fn extract_with_llm(
    subject: &str,
    body: &str,
    patterns: &[Regex],
    llm: &LlmFallbackConfig,
) -> Option<String> {
    if let Some(code) = extract(subject, body, patterns) {
        return Some(code);
    }
    if !llm.enabled || llm.api_key.is_empty() {
        return None;
    }
    #[cfg(feature = "llm")]
    {
        call_llm(subject, body, llm).await
    }
    #[cfg(not(feature = "llm"))]
    {
        None
    }
}

#[cfg(feature = "llm")]
async fn call_llm(subject: &str, body: &str, config: &LlmFallbackConfig) -> Option<String> {
    let text = format!(
        "Subject: {}\n\nBody: {}",
        &subject[..subject.len().min(200)],
        &body[..body.len().min(500)]
    );

    let payload = serde_json::json!({
        "model": config.model,
        "messages": [
            {
                "role": "system",
                "content": "Extract the verification code from this email. Output ONLY the code, no other text. If there is no verification code, output exactly: NONE"
            },
            {"role": "user", "content": text}
        ],
        "max_tokens": 10,
        "temperature": 0.0
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "{}/chat/completions",
            config.base_url.trim_end_matches('/')
        ))
        .header("Authorization", format!("Bearer {}", config.api_key))
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?;

    let json: serde_json::Value = resp.json().await.ok()?;
    let code = json["choices"][0]["message"]["content"]
        .as_str()?
        .trim()
        .to_string();

    if code.eq_ignore_ascii_case("NONE") || code.is_empty() {
        None
    } else {
        Some(code)
    }
}
