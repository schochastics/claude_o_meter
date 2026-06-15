//! Anthropic OAuth usage API client.
//!
//! Endpoint: `GET https://api.anthropic.com/api/oauth/usage`
//! Headers:  `Authorization: Bearer <oauth-token>`
//!           `anthropic-beta: oauth-2025-04-20`
//!
//! The response schema is undocumented and may change. We model the
//! known fields (`five_hour`, `seven_day`) and use `#[serde(flatten)]`
//! to capture every other `seven_day_*` field for per-model display.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use thiserror::Error;

pub const BASE_URL: &str = "https://api.anthropic.com";
const PATH: &str = "/api/oauth/usage";
const BETA: &str = "oauth-2025-04-20";

/// Above this, a utilization value can only be a 0..=100 percentage — no
/// fraction-scale window legitimately reports >150% of quota.
const PERCENTAGE_THRESHOLD: f64 = 1.5;

/// Sticky detector for the API's utilization scale.
///
/// `/api/oauth/usage` reports utilization as either a 0..=1 fraction or a
/// 0..=100 percentage depending on the account/API version. A *single* low
/// reading is ambiguous — `2.0` could mean 2% (percentage) or 200% (fraction).
/// Deciding per-response (the old heuristic) makes the displayed number flip
/// non-monotonically as true usage crosses 1.5: e.g. 1.0 → shown as 100%,
/// then 2.0 → shown as 2%.
///
/// The scale is a property of the account, not of an individual reading, so we
/// latch it: the first time any window exceeds [`PERCENTAGE_THRESHOLD`] we lock
/// into percentage mode and divide *every* subsequent reading by 100. The latch
/// is shared (cheap to clone) so one detector spans all polls, and its state is
/// persisted so a restart doesn't reopen the ambiguity window.
#[derive(Debug, Clone, Default)]
pub struct ScaleLatch(Arc<AtomicBool>);

impl ScaleLatch {
    /// Seed the latch — pass `true` to start already locked into percentage
    /// mode (e.g. restored from persisted settings).
    pub fn new(percentage: bool) -> Self {
        Self(Arc::new(AtomicBool::new(percentage)))
    }

    /// Whether we've decided the API reports percentages (0..=100).
    pub fn is_percentage(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn latch_percentage(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Window {
    pub utilization: f64,
    /// `None` when the API returns `null` (e.g. a window that hasn't been
    /// activated yet — fresh login with no Claude Code usage in the session).
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UsageResponse {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
    /// Captures `seven_day_sonnet`, `seven_day_opus`, etc., and any future
    /// per-model fields. We render whatever shows up here.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl UsageResponse {
    /// Normalize utilization to a 0..=1 fraction in place, using `latch` to
    /// remember the API's scale across calls.
    ///
    /// While the scale is undecided, a reading whose max is within the
    /// ambiguous `[0, 1.5]` band is assumed to already be a fraction and left
    /// alone. The first reading that exceeds [`PERCENTAGE_THRESHOLD`] proves the
    /// API is on the percentage scale: we latch that decision and, from then on,
    /// divide *every* reading by 100 — including later low readings that would
    /// otherwise be misread as near-full fractions. See [`ScaleLatch`].
    pub(crate) fn normalize_scale(&mut self, latch: &ScaleLatch) {
        if !latch.is_percentage() {
            if self.max_utilization() > PERCENTAGE_THRESHOLD {
                latch.latch_percentage();
            } else {
                // Still ambiguous (or NaN) — treat as an already-fraction value.
                return;
            }
        }
        self.scale_down_by_100();
    }

    /// Divide every utilization (top-level windows and per-model `extra`
    /// entries) by 100, converting a percentage reading to a fraction.
    fn scale_down_by_100(&mut self) {
        if let Some(w) = self.five_hour.as_mut() {
            w.utilization /= 100.0;
        }
        if let Some(w) = self.seven_day.as_mut() {
            w.utilization /= 100.0;
        }
        for v in self.extra.values_mut() {
            let Some(obj) = v.as_object_mut() else {
                continue;
            };
            let Some(num) = obj.get("utilization").and_then(|x| x.as_f64()) else {
                continue;
            };
            if let Some(scaled) = serde_json::Number::from_f64(num / 100.0) {
                obj.insert("utilization".into(), serde_json::Value::Number(scaled));
            }
        }
    }

    fn max_utilization(&self) -> f64 {
        let mut m = 0.0_f64;
        for w in self.five_hour.iter().chain(self.seven_day.iter()) {
            m = m.max(w.utilization);
        }
        for v in self.extra.values() {
            if let Some(u) = v.get("utilization").and_then(|x| x.as_f64()) {
                m = m.max(u);
            }
        }
        m
    }

    /// Per-model windows derived from any `seven_day_*` keys in `extra`.
    /// Returns (humanized_label, Window) pairs sorted by label.
    pub fn per_model(&self) -> Vec<(String, Window)> {
        let mut out = Vec::new();
        for (k, v) in &self.extra {
            let Some(suffix) = k.strip_prefix("seven_day_") else {
                continue;
            };
            if let Ok(w) = serde_json::from_value::<Window>(v.clone()) {
                out.push((humanize_model(suffix), w));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Higher of session and weekly utilization, for the menu bar number.
    pub fn headline_fraction(&self) -> f64 {
        let s = self
            .five_hour
            .as_ref()
            .map(|w| w.utilization)
            .unwrap_or(0.0);
        let w = self
            .seven_day
            .as_ref()
            .map(|w| w.utilization)
            .unwrap_or(0.0);
        s.max(w)
    }
}

fn humanize_model(suffix: &str) -> String {
    // "sonnet" -> "Sonnet", "claude_design" -> "Claude Design"
    let mut out = String::with_capacity(suffix.len());
    for (i, part) in suffix.split('_').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.extend(chars);
        }
    }
    out
}

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("unauthorized — token expired or revoked")]
    Unauthorized,
    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("decode error: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("server error: HTTP {0}")]
    Server(u16),
}

pub struct ApiClient {
    http: reqwest::Client,
    token: String,
    base_url: String,
    scale: ScaleLatch,
}

impl ApiClient {
    pub fn new(token: String) -> Result<Self, FetchError> {
        Self::new_with_base(token, BASE_URL.to_string())
    }

    pub fn new_with_base(token: String, base_url: String) -> Result<Self, FetchError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("claude_o_meter/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            token,
            base_url,
            scale: ScaleLatch::default(),
        })
    }

    /// Share a [`ScaleLatch`] across clients so scale detection sticks between
    /// polls. Without this each client starts fresh in the ambiguous state.
    pub fn with_scale_latch(mut self, scale: ScaleLatch) -> Self {
        self.scale = scale;
        self
    }

    pub async fn fetch(&self) -> Result<UsageResponse, FetchError> {
        let url = format!("{}{PATH}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .header("anthropic-beta", BETA)
            .send()
            .await?;

        let status = resp.status();
        if status.is_success() {
            let bytes = resp.bytes().await?;
            let mut parsed: UsageResponse = serde_json::from_slice(&bytes)?;
            parsed.normalize_scale(&self.scale);
            return Ok(parsed);
        }

        if status.as_u16() == 401 {
            return Err(FetchError::Unauthorized);
        }
        if status.as_u16() == 429 {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(Duration::from_secs);
            return Err(FetchError::RateLimited { retry_after });
        }
        Err(FetchError::Server(status.as_u16()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("tests/fixtures/{name}")).expect("fixture")
    }

    #[test]
    fn decodes_nominal() {
        let body = fixture("usage_nominal.json");
        let r: UsageResponse = serde_json::from_str(&body).unwrap();
        assert!(r.five_hour.is_some());
        assert!(r.seven_day.is_some());
        let per_model = r.per_model();
        assert!(!per_model.is_empty());
    }

    #[test]
    fn decodes_missing_per_model() {
        let body = fixture("usage_minimal.json");
        let r: UsageResponse = serde_json::from_str(&body).unwrap();
        assert!(r.five_hour.is_some());
        assert_eq!(r.per_model().len(), 0);
    }

    #[test]
    fn decodes_over_one_utilization() {
        let body = fixture("usage_over.json");
        let r: UsageResponse = serde_json::from_str(&body).unwrap();
        assert!(r.five_hour.unwrap().utilization > 1.0);
    }

    #[test]
    fn headline_is_max_of_session_and_weekly() {
        let body = fixture("usage_nominal.json");
        let r: UsageResponse = serde_json::from_str(&body).unwrap();
        let s = r.five_hour.as_ref().unwrap().utilization;
        let w = r.seven_day.as_ref().unwrap().utilization;
        assert_eq!(r.headline_fraction(), s.max(w));
    }

    #[test]
    fn humanize_model_capitalizes_underscored_names() {
        assert_eq!(humanize_model("sonnet"), "Sonnet");
        assert_eq!(humanize_model("claude_design"), "Claude Design");
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn normalizes_percentage_scale() {
        let body = fixture("usage_percentage.json");
        let mut r: UsageResponse = serde_json::from_str(&body).unwrap();
        r.normalize_scale(&ScaleLatch::default());
        assert!(close(r.five_hour.as_ref().unwrap().utilization, 0.21));
        assert!(close(r.seven_day.as_ref().unwrap().utilization, 0.07));
        let per_model = r.per_model();
        let sonnet = per_model.iter().find(|(k, _)| k == "Sonnet").unwrap();
        let opus = per_model.iter().find(|(k, _)| k == "Opus").unwrap();
        assert!(close(sonnet.1.utilization, 0.04));
        assert!(close(opus.1.utilization, 0.03));
    }

    #[test]
    fn leaves_fraction_scale_alone() {
        let body = fixture("usage_nominal.json");
        let mut r: UsageResponse = serde_json::from_str(&body).unwrap();
        let before_session = r.five_hour.as_ref().unwrap().utilization;
        let before_weekly = r.seven_day.as_ref().unwrap().utilization;
        r.normalize_scale(&ScaleLatch::default());
        assert_eq!(r.five_hour.unwrap().utilization, before_session);
        assert_eq!(r.seven_day.unwrap().utilization, before_weekly);
    }

    fn synth(util: f64) -> UsageResponse {
        let resets_at: chrono::DateTime<chrono::Utc> = "2026-05-19T17:00:00Z".parse().unwrap();
        UsageResponse {
            five_hour: Some(Window {
                utilization: util,
                resets_at: Some(resets_at),
            }),
            seven_day: None,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn boundary_at_1_5_stays_fraction() {
        let mut r = synth(1.5);
        r.normalize_scale(&ScaleLatch::default());
        assert_eq!(r.five_hour.unwrap().utilization, 1.5);
    }

    #[test]
    fn boundary_above_1_5_is_percentage() {
        let mut r = synth(1.51);
        r.normalize_scale(&ScaleLatch::default());
        assert!(close(r.five_hour.unwrap().utilization, 0.0151));
    }

    #[test]
    fn normalizes_extra_per_model_fields_too() {
        let body = fixture("usage_percentage.json");
        let mut r: UsageResponse = serde_json::from_str(&body).unwrap();
        r.normalize_scale(&ScaleLatch::default());
        let sonnet_raw = r.extra.get("seven_day_sonnet").unwrap();
        let u = sonnet_raw.get("utilization").unwrap().as_f64().unwrap();
        assert!(close(u, 0.04));
    }

    #[test]
    fn nan_utilization_is_left_untouched() {
        // NaN > 1.5 is false, so normalize_scale leaves the response alone
        // rather than producing NaN/0.0 from arithmetic on NaN — and does not
        // latch percentage mode off a garbage reading.
        let latch = ScaleLatch::default();
        let mut r = synth(f64::NAN);
        r.normalize_scale(&latch);
        assert!(r.five_hour.unwrap().utilization.is_nan());
        assert!(!latch.is_percentage(), "NaN must not latch percentage mode");
    }

    #[test]
    fn latch_makes_scale_detection_sticky() {
        // Regression for the threshold flip-flop: a high reading latches
        // percentage mode, and a later LOW reading (max <= 1.5) is still
        // divided by 100 instead of being misread as a near-full fraction.
        let latch = ScaleLatch::default();

        // First poll: 2.0 (= 2%) exceeds the threshold, so we latch.
        let mut high = synth(2.0);
        high.normalize_scale(&latch);
        assert!(latch.is_percentage());
        assert!(close(high.five_hour.unwrap().utilization, 0.02));

        // Second poll: 1.0 (= 1%). Without the latch this was shown as 100%.
        let mut low = synth(1.0);
        low.normalize_scale(&latch);
        assert!(close(low.five_hour.unwrap().utilization, 0.01));
    }

    #[test]
    fn preseeded_latch_normalizes_first_low_reading() {
        // A latch restored from persisted settings treats even the very first
        // low reading as a percentage — no ambiguity window after restart.
        let latch = ScaleLatch::new(true);
        let mut r = synth(1.0);
        r.normalize_scale(&latch);
        assert!(close(r.five_hour.unwrap().utilization, 0.01));
    }

    #[test]
    fn decodes_null_resets_at() {
        // Regression: after `claude login` a freshly-issued response can
        // include `"resets_at": null` for windows that haven't started yet.
        let body = r#"{
            "five_hour": {"utilization": 0.0, "resets_at": null},
            "seven_day": {"utilization": 0.1, "resets_at": "2026-05-26T12:00:00Z"}
        }"#;
        let r: UsageResponse = serde_json::from_str(body).unwrap();
        assert!(r.five_hour.as_ref().unwrap().resets_at.is_none());
        assert!(r.seven_day.as_ref().unwrap().resets_at.is_some());
    }

    #[test]
    fn decodes_missing_resets_at_field() {
        // Defensive: the field may be omitted entirely, not just null.
        let body = r#"{"five_hour": {"utilization": 0.5}}"#;
        let r: UsageResponse = serde_json::from_str(body).unwrap();
        assert!(r.five_hour.as_ref().unwrap().resets_at.is_none());
    }

    #[test]
    fn per_model_skips_malformed_extra_entries() {
        let mut r = synth(0.5);
        // Valid per-model entry.
        r.extra.insert(
            "seven_day_sonnet".into(),
            serde_json::json!({
                "utilization": 0.04,
                "resets_at": "2026-05-19T17:00:00Z",
            }),
        );
        // Valid per-model entry with null resets_at — should still parse,
        // since the field is now optional.
        r.extra.insert(
            "seven_day_opus".into(),
            serde_json::json!({"utilization": 0.10, "resets_at": null}),
        );
        // Malformed — utilization is a string, not a number. Skip.
        r.extra.insert(
            "seven_day_borked".into(),
            serde_json::json!({"utilization": "nope"}),
        );
        // Wrong-shape entry — string instead of object; should also be skipped.
        r.extra.insert(
            "seven_day_string".into(),
            serde_json::Value::String("nope".into()),
        );
        let per_model = r.per_model();
        let labels: Vec<&str> = per_model.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(labels, vec!["Opus", "Sonnet"]);
    }
}
