use chrono::DateTime;
use chrono::Local;
use chrono::Utc;
use codex_model_provider_info::ModelProviderInfo;
use serde::Deserialize;
use tokio::time::Duration;
use tokio::time::timeout;

const MINIMAX_TOKEN_PLAN_REMAINS_URL: &str = "https://www.minimax.io/v1/token_plan/remains";
const MINIMAX_USAGE_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MiniMaxUsageSnapshot {
    pub(crate) buckets: Vec<MiniMaxUsageBucket>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MiniMaxUsageBucket {
    pub(crate) name: String,
    pub(crate) interval: MiniMaxUsageWindow,
    pub(crate) weekly: MiniMaxUsageWindow,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MiniMaxUsageWindow {
    pub(crate) remaining_percent: f64,
    pub(crate) used_count: Option<i64>,
    pub(crate) total_count: Option<i64>,
    pub(crate) resets_at: Option<DateTime<Local>>,
}

impl MiniMaxUsageSnapshot {
    pub(crate) fn primary_bucket(&self) -> Option<&MiniMaxUsageBucket> {
        self.buckets
            .iter()
            .find(|bucket| bucket.name.eq_ignore_ascii_case("general"))
            .or_else(|| self.buckets.first())
    }
}

pub(crate) async fn fetch_minimax_usage(
    provider: &ModelProviderInfo,
) -> Result<MiniMaxUsageSnapshot, String> {
    let api_key = provider
        .api_key_if_available()
        .ok_or_else(|| "MINIMAX_API_KEY is not configured".to_string())?;
    let client = reqwest::Client::new();
    let response = timeout(
        MINIMAX_USAGE_TIMEOUT,
        client
            .get(MINIMAX_TOKEN_PLAN_REMAINS_URL)
            .bearer_auth(api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .send(),
    )
    .await
    .map_err(|_| "MiniMax usage request timed out".to_string())?
    .map_err(|err| format!("MiniMax usage request failed: {err}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("MiniMax usage request failed with HTTP {status}"));
    }

    let raw = response
        .json::<RawMiniMaxUsageResponse>()
        .await
        .map_err(|err| format!("MiniMax usage response was not valid JSON: {err}"))?;
    MiniMaxUsageSnapshot::try_from(raw)
}

#[derive(Debug, Deserialize)]
struct RawMiniMaxUsageResponse {
    base_resp: Option<RawMiniMaxBaseResponse>,
    #[serde(default)]
    model_remains: Vec<RawMiniMaxUsageBucket>,
}

#[derive(Debug, Deserialize)]
struct RawMiniMaxBaseResponse {
    status_code: i64,
    status_msg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawMiniMaxUsageBucket {
    model_name: String,
    current_interval_remaining_percent: f64,
    current_interval_total_count: Option<i64>,
    current_interval_usage_count: Option<i64>,
    end_time: Option<i64>,
    current_weekly_remaining_percent: f64,
    current_weekly_total_count: Option<i64>,
    current_weekly_usage_count: Option<i64>,
    weekly_end_time: Option<i64>,
}

impl TryFrom<RawMiniMaxUsageResponse> for MiniMaxUsageSnapshot {
    type Error = String;

    fn try_from(raw: RawMiniMaxUsageResponse) -> Result<Self, Self::Error> {
        if let Some(base_resp) = raw.base_resp
            && base_resp.status_code != 0
        {
            let message = base_resp
                .status_msg
                .unwrap_or_else(|| "unknown error".to_string());
            return Err(format!("MiniMax usage request failed: {message}"));
        }

        let buckets = raw
            .model_remains
            .into_iter()
            .map(MiniMaxUsageBucket::from)
            .collect::<Vec<_>>();
        if buckets.is_empty() {
            return Err("MiniMax usage response did not include Token Plan data".to_string());
        }

        Ok(Self { buckets })
    }
}

impl From<RawMiniMaxUsageBucket> for MiniMaxUsageBucket {
    fn from(raw: RawMiniMaxUsageBucket) -> Self {
        Self {
            name: raw.model_name,
            interval: MiniMaxUsageWindow {
                remaining_percent: raw.current_interval_remaining_percent,
                used_count: raw.current_interval_usage_count,
                total_count: raw.current_interval_total_count,
                resets_at: millis_to_local(raw.end_time),
            },
            weekly: MiniMaxUsageWindow {
                remaining_percent: raw.current_weekly_remaining_percent,
                used_count: raw.current_weekly_usage_count,
                total_count: raw.current_weekly_total_count,
                resets_at: millis_to_local(raw.weekly_end_time),
            },
        }
    }
}

fn millis_to_local(millis: Option<i64>) -> Option<DateTime<Local>> {
    let millis = millis?;
    let seconds = millis.div_euclid(1_000);
    let nanos = u32::try_from(millis.rem_euclid(1_000) * 1_000_000).ok()?;
    DateTime::<Utc>::from_timestamp(seconds, nanos).map(|timestamp| timestamp.with_timezone(&Local))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parses_token_plan_remains_response() {
        let raw: RawMiniMaxUsageResponse = serde_json::from_value(serde_json::json!({
            "base_resp": { "status_code": 0, "status_msg": "success" },
            "model_remains": [
                {
                    "model_name": "general",
                    "current_interval_remaining_percent": 72.5,
                    "current_interval_total_count": 0,
                    "current_interval_usage_count": 0,
                    "end_time": 1781258400000_i64,
                    "current_weekly_remaining_percent": 80,
                    "current_weekly_total_count": 100,
                    "current_weekly_usage_count": 20,
                    "weekly_end_time": 1781481600000_i64
                }
            ]
        }))
        .expect("raw response should parse");

        let snapshot = MiniMaxUsageSnapshot::try_from(raw).expect("snapshot should parse");

        assert_eq!(snapshot.buckets.len(), 1);
        let bucket = snapshot.primary_bucket().expect("general bucket");
        assert_eq!(bucket.name, "general");
        assert_eq!(bucket.interval.remaining_percent, 72.5);
        assert_eq!(bucket.weekly.used_count, Some(20));
        assert_eq!(bucket.weekly.total_count, Some(100));
        assert!(bucket.interval.resets_at.is_some());
    }

    #[test]
    fn rejects_error_response() {
        let raw = RawMiniMaxUsageResponse {
            base_resp: Some(RawMiniMaxBaseResponse {
                status_code: 1001,
                status_msg: Some("bad key".to_string()),
            }),
            model_remains: Vec::new(),
        };

        assert_eq!(
            MiniMaxUsageSnapshot::try_from(raw),
            Err("MiniMax usage request failed: bad key".to_string())
        );
    }
}
