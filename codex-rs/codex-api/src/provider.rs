use codex_client::Request;
use codex_client::RequestCompression;
use codex_client::RetryOn;
use codex_client::RetryPolicy;
use http::Method;
use http::header::HeaderMap;
use std::collections::HashMap;
use std::time::Duration;
use url::Url;

/// High-level retry configuration for a provider.
///
/// This is converted into a `RetryPolicy` used by `codex-client` to drive
/// transport-level retries for both unary and streaming calls.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_attempts: u64,
    pub base_delay: Duration,
    pub retry_429: bool,
    pub retry_5xx: bool,
    pub retry_transport: bool,
}

impl RetryConfig {
    pub fn to_policy(&self) -> RetryPolicy {
        RetryPolicy {
            max_attempts: self.max_attempts,
            base_delay: self.base_delay,
            retry_on: RetryOn {
                retry_429: self.retry_429,
                retry_5xx: self.retry_5xx,
                retry_transport: self.retry_transport,
            },
        }
    }
}

/// HTTP endpoint configuration used to talk to a concrete API deployment.
///
/// Encapsulates base URL, default headers, query params, retry policy, and
/// stream idle timeout, plus helper methods for building requests.
#[derive(Debug, Clone)]
pub struct Provider {
    pub provider_id: Option<String>,
    pub name: String,
    pub base_url: String,
    pub query_params: Option<HashMap<String, String>>,
    pub headers: HeaderMap,
    pub retry: RetryConfig,
    pub stream_idle_timeout: Duration,
}

impl Provider {
    pub fn url_for_path(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        let mut url = if path.is_empty() {
            base.to_string()
        } else {
            format!("{base}/{path}")
        };

        if let Some(params) = &self.query_params
            && !params.is_empty()
        {
            // Percent-encode keys and values so a param containing `&`, `=`, or
            // whitespace cannot corrupt the query string. Sort for deterministic
            // output (HashMap iteration order is otherwise unspecified).
            let mut pairs: Vec<(&String, &String)> = params.iter().collect();
            pairs.sort();
            let qs = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(pairs)
                .finish();
            url.push('?');
            url.push_str(&qs);
        }

        url
    }

    pub fn build_request(&self, method: Method, path: &str) -> Request {
        Request {
            method,
            url: self.url_for_path(path),
            headers: self.headers.clone(),
            body: None,
            compression: RequestCompression::None,
            timeout: None,
        }
    }

    pub fn is_azure_responses_endpoint(&self) -> bool {
        is_azure_responses_provider(&self.name, Some(&self.base_url))
    }

    pub fn is_openrouter_endpoint(&self) -> bool {
        self.provider_id.as_deref() == Some("openrouter")
            || is_openrouter_provider(&self.name, Some(&self.base_url))
    }

    pub fn is_nvidia_endpoint(&self) -> bool {
        self.provider_id.as_deref() == Some("nvidia") || self.name.eq_ignore_ascii_case("nvidia")
    }

    pub fn is_cerebras_endpoint(&self) -> bool {
        self.provider_id.as_deref() == Some("cerebras")
            || self.name.eq_ignore_ascii_case("cerebras")
    }

    pub fn is_groq_endpoint(&self) -> bool {
        self.provider_id.as_deref() == Some("groq") || self.name.eq_ignore_ascii_case("groq")
    }

    pub fn websocket_url_for_path(&self, path: &str) -> Result<Url, url::ParseError> {
        let mut url = Url::parse(&self.url_for_path(path))?;

        let scheme = match url.scheme() {
            "http" => "ws",
            "https" => "wss",
            "ws" | "wss" => return Ok(url),
            _ => return Ok(url),
        };
        let _ = url.set_scheme(scheme);
        Ok(url)
    }
}

pub fn is_openrouter_provider(name: &str, base_url: Option<&str>) -> bool {
    name.eq_ignore_ascii_case("openrouter")
        || base_url
            .map(matches_openrouter_base_url)
            .unwrap_or_default()
}

fn matches_openrouter_base_url(base_url: &str) -> bool {
    let base_url = base_url.to_ascii_lowercase();
    base_url.contains("openrouter.ai/api/v1")
}

pub fn is_azure_responses_provider(name: &str, base_url: Option<&str>) -> bool {
    if name.eq_ignore_ascii_case("azure") {
        true
    } else if let Some(base_url) = base_url {
        matches_azure_responses_base_url(base_url)
    } else {
        false
    }
}

fn matches_azure_responses_base_url(base_url: &str) -> bool {
    let base_url = base_url.to_ascii_lowercase();
    const AZURE_MARKERS: [&str; 6] = [
        "openai.azure.",
        "cognitiveservices.azure.",
        "aoai.azure.",
        "azure-api.",
        "azurefd.",
        "windows.net/openai",
    ];
    AZURE_MARKERS.iter().any(|marker| base_url.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_azure_responses_base_urls() {
        let positive_cases = [
            "https://foo.openai.azure.com/openai",
            "https://foo.openai.azure.us/openai/deployments/bar",
            "https://foo.cognitiveservices.azure.cn/openai",
            "https://foo.aoai.azure.com/openai",
            "https://foo.openai.azure-api.net/openai",
            "https://foo.z01.azurefd.net/",
        ];

        for base_url in positive_cases {
            assert!(
                is_azure_responses_provider("test", Some(base_url)),
                "expected {base_url} to be detected as Azure"
            );
        }

        assert!(is_azure_responses_provider(
            "Azure",
            Some("https://example.com")
        ));

        let negative_cases = [
            "https://api.openai.com/v1",
            "https://example.com/openai",
            "https://myproxy.azurewebsites.net/openai",
        ];

        for base_url in negative_cases {
            assert!(
                !is_azure_responses_provider("test", Some(base_url)),
                "expected {base_url} not to be detected as Azure"
            );
        }
    }

    #[test]
    fn url_for_path_percent_encodes_query_params() {
        let mut query_params = HashMap::new();
        query_params.insert("api version".to_string(), "a&b=c".to_string());
        let provider = Provider {
            provider_id: None,
            name: "test".to_string(),
            base_url: "https://example.com/v1".to_string(),
            query_params: Some(query_params),
            headers: HeaderMap::new(),
            retry: RetryConfig {
                max_attempts: 1,
                base_delay: Duration::from_millis(1),
                retry_429: false,
                retry_5xx: false,
                retry_transport: false,
            },
            stream_idle_timeout: Duration::from_secs(1),
        };

        // Special characters in keys/values must be percent-encoded so they
        // cannot break out of their query-parameter slot.
        assert_eq!(
            "https://example.com/v1/models?api+version=a%26b%3Dc",
            provider.url_for_path("/models")
        );
    }

    #[test]
    fn detects_openrouter_base_urls() {
        assert!(is_openrouter_provider(
            "OpenRouter",
            Some("https://example.com/v1")
        ));
        assert!(is_openrouter_provider(
            "test",
            Some("https://openrouter.ai/api/v1")
        ));
        assert!(is_openrouter_provider(
            "test",
            Some("https://openrouter.ai/api/v1/")
        ));

        assert!(!is_openrouter_provider(
            "test",
            Some("https://example.com/api/v1")
        ));
    }

    #[test]
    fn openrouter_endpoint_detection_uses_provider_id_for_mirrors() {
        let provider = Provider {
            provider_id: Some("openrouter".to_string()),
            name: "OpenRouter Mirror".to_string(),
            base_url: "https://openrouter-mirror.example.test/v1".to_string(),
            query_params: None,
            headers: HeaderMap::new(),
            retry: RetryConfig {
                max_attempts: 1,
                base_delay: Duration::from_millis(1),
                retry_429: false,
                retry_5xx: false,
                retry_transport: false,
            },
            stream_idle_timeout: Duration::from_secs(1),
        };

        assert!(provider.is_openrouter_endpoint());
    }

    #[test]
    fn provider_family_detection_uses_provider_id_for_display_name_overrides() {
        let provider = |provider_id: &str, name: &str| Provider {
            provider_id: Some(provider_id.to_string()),
            name: name.to_string(),
            base_url: "https://provider-mirror.example.test/v1".to_string(),
            query_params: None,
            headers: HeaderMap::new(),
            retry: RetryConfig {
                max_attempts: 1,
                base_delay: Duration::from_millis(1),
                retry_429: false,
                retry_5xx: false,
                retry_transport: false,
            },
            stream_idle_timeout: Duration::from_secs(1),
        };

        assert!(provider("nvidia", "NVIDIA Mirror").is_nvidia_endpoint());
        assert!(provider("cerebras", "Cerebras Mirror").is_cerebras_endpoint());
        assert!(provider("groq", "Groq Mirror").is_groq_endpoint());
    }
}
