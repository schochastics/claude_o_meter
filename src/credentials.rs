//! Read the Claude Code OAuth token from the macOS Keychain.
//!
//! On macOS, Claude Code stores its credentials as a generic password
//! under service name `Claude Code-credentials`. The secret blob is JSON
//! shaped like:
//!
//! ```json
//! {
//!   "claudeAiOauth": {
//!     "accessToken": "...",
//!     "expiresAt": 1740000000000,
//!     "refreshToken": "..."
//!   }
//! }
//! ```
//!
//! First read of this entry from a new binary triggers a Keychain ACL
//! prompt. The user must click "Always Allow" for unattended operation.

use chrono::{DateTime, TimeZone, Utc};
use security_framework::passwords::get_generic_password;
use serde::Deserialize;
use thiserror::Error;

const SERVICE: &str = "Claude Code-credentials";

#[derive(Debug, Clone)]
pub struct StoredCreds {
    pub access_token: String,
    pub expires_at: DateTime<Utc>,
}

impl StoredCreds {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        let margin = chrono::Duration::seconds(60);
        self.expires_at - margin <= now
    }
}

#[derive(Debug, Error)]
pub enum CredError {
    #[error("Keychain entry not found — run `claude login`")]
    NotFound,
    #[error("Keychain access denied")]
    AccessDenied,
    #[error("Keychain returned malformed JSON: {0}")]
    Malformed(String),
    #[error("Keychain error: {0}")]
    Other(String),
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: OauthBlob,
}

#[derive(Deserialize)]
struct OauthBlob {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: i64, // epoch millis
}

/// Look up the current Claude Code user account by reading the Keychain entry.
/// The account name is the logged-in macOS user.
pub fn read_credentials() -> Result<StoredCreds, CredError> {
    let account = std::env::var("USER").unwrap_or_default();
    read_for_account(&account)
}

fn read_for_account(account: &str) -> Result<StoredCreds, CredError> {
    let bytes = get_generic_password(SERVICE, account).map_err(|e| {
        // security-framework wraps OSStatus codes; -25300 = errSecItemNotFound,
        // -128 = errUserCanceled, -25293 = errSecAuthFailed.
        let code = e.code();
        match code {
            -25300 => CredError::NotFound,
            -128 | -25293 => CredError::AccessDenied,
            _ => CredError::Other(format!("{e} (code={code})")),
        }
    })?;
    parse_blob(&bytes)
}

fn parse_blob(bytes: &[u8]) -> Result<StoredCreds, CredError> {
    let env: Envelope =
        serde_json::from_slice(bytes).map_err(|e| CredError::Malformed(e.to_string()))?;
    let expires_at = Utc
        .timestamp_millis_opt(env.claude_ai_oauth.expires_at)
        .single()
        .ok_or_else(|| CredError::Malformed("expiresAt out of range".into()))?;
    Ok(StoredCreds {
        access_token: env.claude_ai_oauth.access_token,
        expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nominal_blob() {
        let blob = br#"{
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-test",
                "expiresAt": 1779984000000,
                "refreshToken": "sk-ant-ort01-test"
            }
        }"#;
        let creds = parse_blob(blob).unwrap();
        assert_eq!(creds.access_token, "sk-ant-oat01-test");
        assert_eq!(creds.expires_at.timestamp_millis(), 1779984000000);
    }

    #[test]
    fn ignores_extra_fields() {
        let blob = br#"{
            "claudeAiOauth": {
                "accessToken": "tok",
                "expiresAt": 1779984000000,
                "scopes": ["user:inference"]
            },
            "version": 2
        }"#;
        assert!(parse_blob(blob).is_ok());
    }

    #[test]
    fn malformed_returns_error() {
        assert!(matches!(parse_blob(b"not json"), Err(CredError::Malformed(_))));
    }

    #[test]
    fn missing_field_returns_error() {
        let blob = br#"{"claudeAiOauth": {"accessToken": "x"}}"#;
        assert!(matches!(parse_blob(blob), Err(CredError::Malformed(_))));
    }

    #[test]
    fn expiry_check_with_margin() {
        let creds = StoredCreds {
            access_token: "x".into(),
            expires_at: Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap(),
        };
        // Inside the 60s margin (30s before expiry) — treated as expired.
        let inside_margin = Utc.with_ymd_and_hms(2026, 5, 19, 11, 59, 30).unwrap();
        // Outside the margin (2 minutes before expiry) — still valid.
        let outside_margin = Utc.with_ymd_and_hms(2026, 5, 19, 11, 58, 0).unwrap();
        // After expiry.
        let after = Utc.with_ymd_and_hms(2026, 5, 19, 12, 5, 0).unwrap();
        assert!(creds.is_expired(inside_margin), "inside 60s margin -> expired");
        assert!(!creds.is_expired(outside_margin), "outside margin -> valid");
        assert!(creds.is_expired(after));
    }
}
