//! API-equivalent cost estimates for Claude Code token usage.
//!
//! Claude Code on a subscription isn't billed per token, but pricing each
//! transcript message at public API list rates gives a feel for how much
//! work the plan is absorbing. Rates are USD per million tokens, from
//! <https://platform.claude.com/docs/en/about-claude/pricing> (Sep 2026).
//! Batch, data-residency and negotiated discounts are ignored.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
    pub output: f64,
    /// Price multiplier for `speed: "fast"` responses (1.0 when the model has
    /// no fast mode).
    pub fast_multiplier: f64,
}

const fn rates(input: f64, cache_read: f64, output: f64, fast_multiplier: f64) -> Rates {
    Rates {
        input,
        cache_write_5m: input * 1.25,
        cache_write_1h: input * 2.0,
        cache_read,
        output,
        fast_multiplier,
    }
}

const FABLE_5_1: Rates = rates(10.0, 0.25, 50.0, 1.0);
const FABLE_5: Rates = rates(10.0, 1.0, 50.0, 1.0);
const OPUS_5_5: Rates = rates(4.0, 0.20, 20.0, 2.0);
const OPUS_5: Rates = rates(5.0, 0.50, 25.0, 2.0);
const OPUS_4_8: Rates = rates(5.0, 0.50, 25.0, 2.0);
const OPUS_4_5: Rates = rates(5.0, 0.50, 25.0, 1.0);
const OPUS_4: Rates = rates(15.0, 1.50, 75.0, 1.0);
const SONNET_5: Rates = rates(2.0, 0.20, 10.0, 1.0);
const SONNET_4: Rates = rates(3.0, 0.30, 15.0, 1.0);
const HAIKU_4_5: Rates = rates(1.0, 0.10, 5.0, 1.0);
const HAIKU_3_5: Rates = rates(0.80, 0.08, 4.0, 1.0);

/// Model-id prefixes, most specific first so `claude-opus-5-5` wins over
/// `claude-opus-5`.
const TABLE: &[(&str, Rates)] = &[
    ("claude-fable-5-1", FABLE_5_1),
    ("claude-mythos-5-1", FABLE_5_1),
    ("claude-fable-5", FABLE_5),
    ("claude-mythos-5", FABLE_5),
    ("claude-opus-5-5", OPUS_5_5),
    ("claude-opus-5", OPUS_5),
    ("claude-opus-4-8", OPUS_4_8),
    ("claude-opus-4-7", OPUS_4_5),
    ("claude-opus-4-6", OPUS_4_5),
    ("claude-opus-4-5", OPUS_4_5),
    ("claude-opus-4", OPUS_4),
    ("claude-sonnet-5", SONNET_5),
    ("claude-sonnet-4", SONNET_4),
    ("claude-3-7-sonnet", SONNET_4),
    ("claude-haiku-4-5", HAIKU_4_5),
    ("claude-3-5-haiku", HAIKU_3_5),
];

/// Web search is billed per request on top of tokens.
const WEB_SEARCH_USD: f64 = 10.0 / 1000.0;

/// Look up list rates for a transcript `model` id. Handles date suffixes
/// (`claude-haiku-4-5-20251001`) and Claude Code's context tag
/// (`claude-opus-5-5[1m]`). Unknown ids in a known family fall back to that
/// family's current model so a new release isn't priced at $0; anything else
/// (e.g. `<synthetic>`) returns `None`.
pub fn rates_for(model: &str) -> Option<Rates> {
    let id = model.split('[').next().unwrap_or(model);
    if let Some((_, r)) = TABLE.iter().find(|(prefix, _)| id.starts_with(prefix)) {
        return Some(*r);
    }
    if id.contains("fable") || id.contains("mythos") {
        Some(FABLE_5_1)
    } else if id.contains("opus") {
        Some(OPUS_5_5)
    } else if id.contains("sonnet") {
        Some(SONNET_5)
    } else if id.contains("haiku") {
        Some(HAIKU_4_5)
    } else {
        None
    }
}

/// Token counts of one assistant message, as they appear in the transcript
/// `usage` block.
#[derive(Debug, Default, Clone, Copy)]
pub struct MessageUsage {
    pub input: u64,
    pub output: u64,
    /// Total cache writes (`cache_creation_input_tokens`).
    pub cache_creation: u64,
    /// The 1-hour-TTL share of `cache_creation`; the rest is priced as 5m.
    pub cache_creation_1h: u64,
    pub cache_read: u64,
    pub web_search_requests: u64,
    pub fast: bool,
}

/// Estimated API-equivalent cost of one message in micro-dollars (1e-6 USD).
/// Integer micros keep the rollups exact and `Eq`-comparable.
pub fn cost_micros(model: &str, u: &MessageUsage) -> u64 {
    let Some(r) = rates_for(model) else {
        return 0;
    };
    let write_1h = u.cache_creation_1h.min(u.cache_creation);
    let write_5m = u.cache_creation - write_1h;
    // Rates are $/MTok, so tokens × rate is already in micro-dollars.
    let tokens = u.input as f64 * r.input
        + write_5m as f64 * r.cache_write_5m
        + write_1h as f64 * r.cache_write_1h
        + u.cache_read as f64 * r.cache_read
        + u.output as f64 * r.output;
    let mult = if u.fast { r.fast_multiplier } else { 1.0 };
    let search = u.web_search_requests as f64 * WEB_SEARCH_USD * 1e6;
    (tokens * mult + search).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_specific_prefix_wins() {
        assert_eq!(rates_for("claude-opus-5-5"), Some(OPUS_5_5));
        assert_eq!(rates_for("claude-opus-5"), Some(OPUS_5));
        assert_eq!(rates_for("claude-fable-5-1"), Some(FABLE_5_1));
        assert_eq!(rates_for("claude-fable-5"), Some(FABLE_5));
    }

    #[test]
    fn strips_context_tag_and_date_suffix() {
        assert_eq!(rates_for("claude-opus-5-5[1m]"), Some(OPUS_5_5));
        assert_eq!(rates_for("claude-haiku-4-5-20251001"), Some(HAIKU_4_5));
    }

    #[test]
    fn unknown_family_member_falls_back() {
        assert_eq!(rates_for("claude-sonnet-6"), Some(SONNET_5));
        assert_eq!(rates_for("<synthetic>"), None);
    }

    #[test]
    fn opus_5_5_cache_read_is_0_05x() {
        assert_eq!(OPUS_5_5.cache_read, OPUS_5_5.input * 0.05);
    }

    #[test]
    fn cost_splits_cache_ttl() {
        // Opus 5: 1M input $5 + 1M output $25 + 1M 5m-write $6.25 +
        // 1M 1h-write $10 + 1M read $0.50 = $46.75.
        let u = MessageUsage {
            input: 1_000_000,
            output: 1_000_000,
            cache_creation: 2_000_000,
            cache_creation_1h: 1_000_000,
            cache_read: 1_000_000,
            ..Default::default()
        };
        assert_eq!(cost_micros("claude-opus-5", &u), 46_750_000);
    }

    #[test]
    fn fast_mode_and_web_search() {
        let u = MessageUsage {
            output: 1_000_000,
            web_search_requests: 3,
            fast: true,
            ..Default::default()
        };
        // $20 × 2 fast + 3 × $0.01 search.
        assert_eq!(cost_micros("claude-opus-5-5", &u), 40_030_000);
    }

    #[test]
    fn unpriced_model_costs_nothing() {
        let u = MessageUsage {
            input: 10,
            ..Default::default()
        };
        assert_eq!(cost_micros("<synthetic>", &u), 0);
    }
}
