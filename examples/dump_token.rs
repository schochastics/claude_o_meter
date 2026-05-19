//! Diagnostic: print parsed credentials (token redacted) and expiry status.
//! Run with: cargo run --example dump_token

use claude_o_meter::credentials::read_credentials;

fn main() {
    let creds = match read_credentials() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("read failed: {e}");
            std::process::exit(1);
        }
    };
    let now = chrono::Utc::now();
    let token_preview = if creds.access_token.len() > 12 {
        format!(
            "{}…{}",
            &creds.access_token[..6],
            &creds.access_token[creds.access_token.len() - 4..]
        )
    } else {
        "<redacted>".to_string()
    };
    println!("access_token : {token_preview}");
    println!("expires_at   : {}", creds.expires_at);
    println!("now          : {now}");
    println!(
        "status       : {}",
        if creds.is_expired(now) {
            "EXPIRED"
        } else {
            "valid"
        }
    );
}
