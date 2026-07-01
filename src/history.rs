//! Aggregates historic Claude Code token usage from local transcript JSONL.
//!
//! Walks `~/.claude/projects/**/*.jsonl`, parses assistant messages,
//! and rolls up token counts per local-time day and per project (by cwd).
//!
//! Incremental: only files whose mtime changed since the last scan are
//! re-parsed. Result is persisted as JSON so restarts populate instantly.

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Totals {
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
}

impl Totals {
    pub fn sum(&self) -> u64 {
        self.input + self.output + self.cache_creation + self.cache_read
    }

    fn add(&mut self, other: &Totals) {
        self.input += other.input;
        self.output += other.output;
        self.cache_creation += other.cache_creation;
        self.cache_read += other.cache_read;
    }
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct ProjectTotals {
    pub total: u64,
    pub last_used: Option<DateTime<Utc>>,
}

/// A single user prompt and the total tokens attributed to it (summed across
/// the assistant messages in its turn). `text` is a whitespace-collapsed,
/// truncated preview of the prompt for display in the menu.
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct PromptStat {
    pub text: String,
    pub tokens: u64,
    pub project: String,
    pub timestamp: DateTime<Utc>,
}

/// Max prompts retained per file. Generous so a global top-3 in either the
/// 7-day or all-time window is virtually always present in some file's list.
const PER_FILE_PROMPTS: usize = 10;

/// Max prompt preview length stored in the cache, in characters.
const PROMPT_TEXT_MAX: usize = 120;

/// Bump when the parse/cache schema changes in a way that requires re-parsing
/// existing transcripts. See `load_or_default` for the migration.
const HISTORY_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
struct FileEntry {
    mtime_unix_ns: i128,
    by_day: BTreeMap<NaiveDate, Totals>,
    by_project: BTreeMap<String, ProjectTotals>,
    /// This file's own top prompts by token usage, capped at
    /// `PER_FILE_PROMPTS`. Rolled up on demand by `Aggregates::top_prompts`.
    #[serde(default)]
    top_prompts: Vec<PromptStat>,
    /// True when the source .jsonl is no longer on disk (Claude Code's
    /// 30-day retention sweep removed it). We keep the rolled-up totals
    /// so "all-time" aggregates survive transcript deletion. Purged
    /// explicitly via `purge_tombstoned`.
    #[serde(default)]
    tombstoned: bool,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Aggregates {
    file_totals: BTreeMap<PathBuf, FileEntry>,
    pub by_day: BTreeMap<NaiveDate, Totals>,
    pub by_project: BTreeMap<String, ProjectTotals>,
    pub scanned_files: usize,
    pub last_scanned_at: Option<DateTime<Utc>>,
    /// Schema version of the cached data. See `HISTORY_VERSION`.
    #[serde(default)]
    version: u32,
}

impl Aggregates {
    pub fn load_or_default(path: &Path) -> Self {
        let mut agg: Aggregates = match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "history cache malformed; starting fresh");
                Aggregates::default()
            }),
            Err(_) => Aggregates::default(),
        };
        agg.migrate();
        agg
    }

    /// Handle cache-schema upgrades. When the persisted `version` is behind
    /// `HISTORY_VERSION`, invalidate every live entry's mtime so the next
    /// `refresh` re-parses it and populates the newer fields (e.g. prompts).
    /// Tombstoned entries are left as-is — their source transcript is gone, so
    /// they keep their existing day/project totals but contribute no prompts.
    fn migrate(&mut self) {
        if self.version == HISTORY_VERSION {
            return;
        }
        for fe in self.file_totals.values_mut() {
            if !fe.tombstoned {
                fe.mtime_unix_ns = i128::MIN;
            }
        }
        self.version = HISTORY_VERSION;
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("mkdir {parent:?}"))?;
        }
        let json = serde_json::to_vec(self)?;
        fs::write(path, json).with_context(|| format!("write {path:?}"))?;
        Ok(())
    }

    /// Walk `projects_dir`, reparse changed files, tombstone entries for
    /// files that no longer exist, rebuild rollups. Returns `true` if
    /// anything changed. Tombstoned entries keep contributing to the
    /// rollups so the "all-time" total survives Claude Code's transcript
    /// retention sweep; call `purge_tombstoned` to forget them.
    pub fn refresh(&mut self, projects_dir: &Path) -> Result<bool> {
        if !projects_dir.exists() {
            return Ok(false);
        }
        let mut changed = false;
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

        for entry in walk_jsonl(projects_dir) {
            seen.insert(entry.clone());
            let mtime = mtime_ns(&entry);
            // A tombstoned-but-now-back-on-disk file is treated as stale so
            // we re-parse and clear the tombstone in one step.
            let stale = self
                .file_totals
                .get(&entry)
                .map(|fe| fe.mtime_unix_ns != mtime || fe.tombstoned)
                .unwrap_or(true);
            if !stale {
                continue;
            }
            match parse_file(&entry, mtime) {
                Ok(fe) => {
                    self.file_totals.insert(entry, fe);
                    changed = true;
                }
                Err(e) => {
                    tracing::debug!(file = %entry.display(), error = %e, "skip bad transcript");
                }
            }
        }

        // Mark entries whose source file has disappeared. Don't drop them:
        // their totals remain in the rollups so all-time history persists
        // even after Claude Code prunes the transcript.
        for (k, fe) in self.file_totals.iter_mut() {
            if !seen.contains(k) && !fe.tombstoned {
                fe.tombstoned = true;
                changed = true;
            }
        }

        if changed {
            self.rebuild_rollups();
            self.scanned_files = self
                .file_totals
                .values()
                .filter(|fe| !fe.tombstoned)
                .count();
            self.last_scanned_at = Some(Utc::now());
        }
        Ok(changed)
    }

    /// Number of cached files whose source transcript has been deleted
    /// from disk. Their token totals still count toward the rollups
    /// until `purge_tombstoned` runs.
    pub fn tombstoned_count(&self) -> usize {
        self.file_totals.values().filter(|fe| fe.tombstoned).count()
    }

    /// Drop tombstoned entries and refresh the rollups. Returns the
    /// number of entries removed.
    pub fn purge_tombstoned(&mut self) -> usize {
        let before = self.file_totals.len();
        self.file_totals.retain(|_, fe| !fe.tombstoned);
        let removed = before - self.file_totals.len();
        if removed > 0 {
            self.rebuild_rollups();
            self.scanned_files = self.file_totals.len();
        }
        removed
    }

    fn rebuild_rollups(&mut self) {
        let mut by_day: BTreeMap<NaiveDate, Totals> = BTreeMap::new();
        let mut by_project: BTreeMap<String, ProjectTotals> = BTreeMap::new();

        for fe in self.file_totals.values() {
            for (date, t) in &fe.by_day {
                by_day.entry(*date).or_default().add(t);
            }
            for (proj, pt) in &fe.by_project {
                let agg = by_project.entry(proj.clone()).or_default();
                agg.total += pt.total;
                match (agg.last_used, pt.last_used) {
                    (None, x) => agg.last_used = x,
                    (Some(a), Some(b)) if b > a => agg.last_used = Some(b),
                    _ => {}
                }
            }
        }
        self.by_day = by_day;
        self.by_project = by_project;
    }

    /// Tokens for each of the last `n` days, oldest first. Missing days are
    /// included as zero-`Totals` so the bar chart shows the gap.
    pub fn last_n_days(&self, n: usize, today: NaiveDate) -> Vec<(NaiveDate, Totals)> {
        (0..n)
            .rev()
            .map(|offset| {
                let d = today - chrono::Duration::days(offset as i64);
                let t = self.by_day.get(&d).copied().unwrap_or_default();
                (d, t)
            })
            .collect()
    }

    /// Total tokens in the current calendar month plus a linear projection
    /// to month-end. Returns `(current_total, projected_total)`. When no
    /// days have elapsed yet (impossible in practice), the projection equals
    /// the current total.
    pub fn current_month_total_and_projection(&self, today: NaiveDate) -> (u64, u64) {
        let Some(month_start) = today.with_day(1) else {
            return (0, 0);
        };
        let next_month_start = if today.month() == 12 {
            NaiveDate::from_ymd_opt(today.year() + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1)
        };
        let Some(next_month_start) = next_month_start else {
            return (0, 0);
        };
        let days_in_month = (next_month_start - month_start).num_days().max(1) as u64;
        let days_elapsed = ((today - month_start).num_days() + 1).max(1) as u64;

        let current: u64 = self
            .by_day
            .range(month_start..next_month_start)
            .map(|(_, t)| t.sum())
            .sum();
        let projected = current.saturating_mul(days_in_month) / days_elapsed;
        (current, projected)
    }

    /// Top projects by total tokens, optionally restricted to activity on
    /// or after `since`. For the windowed case we approximate per-project
    /// recent tokens as `(project's share of its file) × (recent share of
    /// that file)` since we don't store per-day-per-project breakdowns.
    /// Returns at most `n` entries.
    pub fn top_projects(&self, n: usize, since: Option<NaiveDate>) -> Vec<(String, u64)> {
        let mut acc: BTreeMap<String, u64> = BTreeMap::new();
        match since {
            None => {
                for (proj, pt) in &self.by_project {
                    acc.insert(proj.clone(), pt.total);
                }
            }
            Some(since_date) => {
                for fe in self.file_totals.values() {
                    let file_total: u64 = fe.by_day.values().map(|t| t.sum()).sum();
                    if file_total == 0 {
                        continue;
                    }
                    let recent: u64 = fe
                        .by_day
                        .iter()
                        .filter(|(d, _)| **d >= since_date)
                        .map(|(_, t)| t.sum())
                        .sum();
                    if recent == 0 {
                        continue;
                    }
                    let frac = recent as f64 / file_total as f64;
                    for (proj, pt) in &fe.by_project {
                        let portion = (pt.total as f64 * frac).round() as u64;
                        if portion > 0 {
                            *acc.entry(proj.clone()).or_default() += portion;
                        }
                    }
                }
            }
        }
        let mut out: Vec<(String, u64)> = acc.into_iter().collect();
        out.sort_by_key(|(_, total)| std::cmp::Reverse(*total));
        out.truncate(n);
        out
    }

    /// Top prompts by token usage across all live files, optionally restricted
    /// to prompts on or after `since` (by the prompt's local date). Returns at
    /// most `n` entries, highest tokens first.
    ///
    /// Because each file only retains its own top `PER_FILE_PROMPTS`, a very
    /// small recent prompt could in theory fall outside its file's list and be
    /// missed by the `since` window — negligible for a global top-N, and
    /// mirrors the per-file approximation used by `top_projects`.
    pub fn top_prompts(&self, n: usize, since: Option<NaiveDate>) -> Vec<PromptStat> {
        let mut out: Vec<PromptStat> = Vec::new();
        for fe in self.file_totals.values() {
            if fe.tombstoned {
                continue;
            }
            for p in &fe.top_prompts {
                if let Some(since_date) = since
                    && p.timestamp.with_timezone(&Local).date_naive() < since_date
                {
                    continue;
                }
                out.push(p.clone());
            }
        }
        out.sort_by_key(|p| std::cmp::Reverse(p.tokens));
        out.truncate(n);
        out
    }
}

fn walk_jsonl(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(p);
            }
        }
    }
    out
}

fn mtime_ns(p: &Path) -> i128 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0)
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    cwd: Option<String>,
    #[serde(default, rename = "isSidechain")]
    is_sidechain: bool,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    usage: Option<UsageBlock>,
    content: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct UsageBlock {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

fn parse_file(path: &Path, mtime: i128) -> Result<FileEntry> {
    let f = fs::File::open(path).with_context(|| format!("open {path:?}"))?;
    let reader = BufReader::new(f);
    let mut by_day: BTreeMap<NaiveDate, Totals> = BTreeMap::new();
    let mut by_project: BTreeMap<String, ProjectTotals> = BTreeMap::new();

    // Prompt attribution: `current` accumulates assistant tokens for the most
    // recent top-level user prompt; a new real prompt flushes it. Sidechain
    // (subagent) lines are excluded from prompt ranking but still counted in
    // the day/project totals to preserve existing behavior.
    let mut prompts: Vec<PromptStat> = Vec::new();
    let mut current: Option<PromptStat> = None;

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if line.is_empty() {
            continue;
        }
        let Ok(parsed): std::result::Result<Line, _> = serde_json::from_str(&line) else {
            continue;
        };
        let ts = parsed.timestamp.unwrap_or_else(Utc::now);

        match parsed.kind.as_deref() {
            Some("assistant") => {
                let Some(message) = parsed.message else { continue };
                let Some(usage) = message.usage else { continue };
                let totals = Totals {
                    input: usage.input_tokens.unwrap_or(0),
                    output: usage.output_tokens.unwrap_or(0),
                    cache_creation: usage.cache_creation_input_tokens.unwrap_or(0),
                    cache_read: usage.cache_read_input_tokens.unwrap_or(0),
                };
                if totals.sum() == 0 {
                    continue;
                }
                let local_date = ts.with_timezone(&Local).date_naive();
                by_day.entry(local_date).or_default().add(&totals);

                if let Some(cwd) = parsed.cwd {
                    let pt = by_project.entry(cwd).or_default();
                    pt.total += totals.sum();
                    pt.last_used = Some(match pt.last_used {
                        Some(prev) if prev > ts => prev,
                        _ => ts,
                    });
                }

                if !parsed.is_sidechain
                    && let Some(cur) = current.as_mut()
                {
                    cur.tokens += totals.sum();
                }
            }
            Some("user") => {
                if parsed.is_sidechain {
                    continue;
                }
                let Some(message) = parsed.message else { continue };
                let Some(text) = extract_prompt_text(message.content.as_ref()) else {
                    continue;
                };
                if let Some(prev) = current.take()
                    && prev.tokens > 0
                {
                    prompts.push(prev);
                }
                current = Some(PromptStat {
                    text,
                    tokens: 0,
                    project: parsed.cwd.unwrap_or_default(),
                    timestamp: ts,
                });
            }
            _ => {}
        }
    }
    if let Some(prev) = current.take()
        && prev.tokens > 0
    {
        prompts.push(prev);
    }
    prompts.sort_by_key(|p| std::cmp::Reverse(p.tokens));
    prompts.truncate(PER_FILE_PROMPTS);

    Ok(FileEntry {
        mtime_unix_ns: mtime,
        by_day,
        by_project,
        top_prompts: prompts,
        tombstoned: false,
    })
}

/// Extract a display preview from a user message's `content`, or `None` if this
/// isn't a real user prompt. A `type:"user"` line that carries a `tool_result`
/// block is a tool response, not something the user typed, so it's rejected.
fn extract_prompt_text(content: Option<&serde_json::Value>) -> Option<String> {
    let raw = match content? {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => {
            let is_tool_result = |b: &serde_json::Value| {
                b.get("type").and_then(|t| t.as_str()) == Some("tool_result")
            };
            if blocks.iter().any(is_tool_result) {
                return None;
            }
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(" ")
        }
        _ => return None,
    };
    let cleaned: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        return None;
    }
    Some(truncate_chars(&cleaned, PROMPT_TEXT_MAX))
}

/// Truncate to at most `max` characters, appending an ellipsis when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Just the basename of a project path, plus the parent dir if there's a
/// collision with another project in the aggregate. Caller passes the full
/// set of project paths so we can disambiguate.
pub fn short_name(full: &str, all: &[&str]) -> String {
    let base = full.rsplit('/').next().unwrap_or(full).to_string();
    let collides = all
        .iter()
        .filter(|other| **other != full && other.rsplit('/').next() == Some(&base))
        .count()
        > 0;
    if !collides {
        return base;
    }
    let mut parts = full.rsplit('/');
    let last = parts.next().unwrap_or("");
    let parent = parts.next().unwrap_or("");
    if parent.is_empty() {
        last.to_string()
    } else {
        format!("{parent}/{last}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_jsonl(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn parses_assistant_token_counts() {
        let td = TempDir::new().unwrap();
        write_jsonl(
            td.path(),
            "s1.jsonl",
            &[
                r#"{"type":"user","message":{"content":"hi"}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:00Z","cwd":"/Users/a/proj","message":{"usage":{"input_tokens":100,"output_tokens":200,"cache_creation_input_tokens":10,"cache_read_input_tokens":50}}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-19T09:00:00Z","cwd":"/Users/a/proj","message":{"usage":{"input_tokens":50,"output_tokens":100}}}"#,
            ],
        );
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        assert_eq!(agg.scanned_files, 1);
        let project_total = agg.by_project.get("/Users/a/proj").unwrap().total;
        assert_eq!(project_total, 100 + 200 + 10 + 50 + 50 + 100);
    }

    #[test]
    fn skips_user_messages_and_malformed_lines() {
        let td = TempDir::new().unwrap();
        write_jsonl(
            td.path(),
            "s.jsonl",
            &[
                "not json at all",
                r#"{"type":"user"}"#,
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:00Z","cwd":"/p","message":{"usage":{"input_tokens":1,"output_tokens":2}}}"#,
            ],
        );
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        assert_eq!(agg.by_project.get("/p").unwrap().total, 3);
    }

    #[test]
    fn incremental_refresh_replaces_changed_file() {
        let td = TempDir::new().unwrap();
        let f = write_jsonl(
            td.path(),
            "s.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:00Z","cwd":"/p","message":{"usage":{"input_tokens":100}}}"#,
            ],
        );
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        assert_eq!(agg.by_project.get("/p").unwrap().total, 100);

        // Wait a hair, then overwrite with different contents + bump mtime.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::remove_file(&f).unwrap();
        write_jsonl(
            td.path(),
            "s.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:00Z","cwd":"/p","message":{"usage":{"input_tokens":500}}}"#,
            ],
        );
        agg.refresh(td.path()).unwrap();
        assert_eq!(agg.by_project.get("/p").unwrap().total, 500);
    }

    #[test]
    fn removed_files_are_tombstoned_not_dropped() {
        let td = TempDir::new().unwrap();
        let f1 = write_jsonl(
            td.path(),
            "a.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:00Z","cwd":"/x","message":{"usage":{"input_tokens":10}}}"#,
            ],
        );
        write_jsonl(
            td.path(),
            "b.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:00Z","cwd":"/y","message":{"usage":{"input_tokens":20}}}"#,
            ],
        );
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        assert_eq!(agg.scanned_files, 2);
        assert_eq!(agg.tombstoned_count(), 0);

        // Simulate Claude Code's retention sweep deleting the transcript.
        fs::remove_file(&f1).unwrap();
        agg.refresh(td.path()).unwrap();
        // Live count drops, but tombstoned totals are preserved.
        assert_eq!(agg.scanned_files, 1);
        assert_eq!(agg.tombstoned_count(), 1);
        assert!(agg.by_project.contains_key("/x"));
        assert!(agg.by_project.contains_key("/y"));
        assert_eq!(agg.by_project.get("/x").unwrap().total, 10);
    }

    #[test]
    fn purge_tombstoned_drops_only_dead_entries() {
        let td = TempDir::new().unwrap();
        let f1 = write_jsonl(
            td.path(),
            "a.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:00Z","cwd":"/x","message":{"usage":{"input_tokens":10}}}"#,
            ],
        );
        write_jsonl(
            td.path(),
            "b.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:00Z","cwd":"/y","message":{"usage":{"input_tokens":20}}}"#,
            ],
        );
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        fs::remove_file(&f1).unwrap();
        agg.refresh(td.path()).unwrap();

        let purged = agg.purge_tombstoned();
        assert_eq!(purged, 1);
        assert_eq!(agg.tombstoned_count(), 0);
        assert!(!agg.by_project.contains_key("/x"));
        assert!(agg.by_project.contains_key("/y"));
        assert_eq!(agg.scanned_files, 1);
    }

    #[test]
    fn re_appearing_file_clears_tombstone() {
        let td = TempDir::new().unwrap();
        let f1 = write_jsonl(
            td.path(),
            "a.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:00Z","cwd":"/x","message":{"usage":{"input_tokens":10}}}"#,
            ],
        );
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        fs::remove_file(&f1).unwrap();
        agg.refresh(td.path()).unwrap();
        assert_eq!(agg.tombstoned_count(), 1);

        // Same path written back with different contents: tombstone clears,
        // totals reflect the new content (not the old plus new).
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_jsonl(
            td.path(),
            "a.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-05-20T08:00:00Z","cwd":"/x","message":{"usage":{"input_tokens":500}}}"#,
            ],
        );
        agg.refresh(td.path()).unwrap();
        assert_eq!(agg.tombstoned_count(), 0);
        assert_eq!(agg.by_project.get("/x").unwrap().total, 500);
    }

    #[test]
    fn top_projects_returns_n_sorted_desc() {
        let td = TempDir::new().unwrap();
        write_jsonl(
            td.path(),
            "a.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:00Z","cwd":"/small","message":{"usage":{"input_tokens":10}}}"#,
            ],
        );
        write_jsonl(
            td.path(),
            "b.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:00Z","cwd":"/big","message":{"usage":{"input_tokens":1000}}}"#,
            ],
        );
        write_jsonl(
            td.path(),
            "c.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:00Z","cwd":"/medium","message":{"usage":{"input_tokens":100}}}"#,
            ],
        );
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        let top = agg.top_projects(2, None);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, "/big");
        assert_eq!(top[1].0, "/medium");
    }

    #[test]
    fn last_n_days_pads_missing_days_with_zero() {
        let td = TempDir::new().unwrap();
        write_jsonl(
            td.path(),
            "s.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:00Z","cwd":"/p","message":{"usage":{"input_tokens":1}}}"#,
            ],
        );
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 5, 22).unwrap();
        let days = agg.last_n_days(7, today);
        assert_eq!(days.len(), 7);
        // Most recent day is at the end; the day with data isn't today.
        assert_eq!(days.last().unwrap().0, today);
        assert_eq!(days.last().unwrap().1.sum(), 0);
    }

    fn agg_with_days(days: &[(NaiveDate, u64)]) -> Aggregates {
        let mut agg = Aggregates::default();
        for (d, n) in days {
            agg.by_day.insert(
                *d,
                Totals {
                    input: *n,
                    ..Default::default()
                },
            );
        }
        agg
    }

    #[test]
    fn monthly_projection_extrapolates_to_month_end() {
        // Two days into May (the 1st and the 2nd), 100 tokens each → 200 over 2 days.
        let agg = agg_with_days(&[
            (NaiveDate::from_ymd_opt(2026, 5, 1).unwrap(), 100),
            (NaiveDate::from_ymd_opt(2026, 5, 2).unwrap(), 100),
        ]);
        let today = NaiveDate::from_ymd_opt(2026, 5, 2).unwrap();
        let (current, projected) = agg.current_month_total_and_projection(today);
        assert_eq!(current, 200);
        assert_eq!(projected, 200 * 31 / 2);
    }

    #[test]
    fn monthly_projection_excludes_prior_months() {
        let agg = agg_with_days(&[
            // April 30 — should NOT be counted toward May's total.
            (NaiveDate::from_ymd_opt(2026, 4, 30).unwrap(), 99_999),
            (NaiveDate::from_ymd_opt(2026, 5, 1).unwrap(), 50),
        ]);
        let today = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
        let (current, _) = agg.current_month_total_and_projection(today);
        assert_eq!(current, 50);
    }

    #[test]
    fn monthly_projection_zero_when_no_data() {
        let agg = Aggregates::default();
        let today = NaiveDate::from_ymd_opt(2026, 5, 15).unwrap();
        assert_eq!(agg.current_month_total_and_projection(today), (0, 0));
    }

    #[test]
    fn monthly_projection_handles_december_rollover() {
        // December → next month is January of next year; must not panic.
        let agg = agg_with_days(&[(NaiveDate::from_ymd_opt(2026, 12, 1).unwrap(), 100)]);
        let today = NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
        let (current, projected) = agg.current_month_total_and_projection(today);
        assert_eq!(current, 100);
        assert_eq!(projected, 100 * 31);
    }

    #[test]
    fn monthly_projection_february_non_leap() {
        let agg = agg_with_days(&[(NaiveDate::from_ymd_opt(2026, 2, 14).unwrap(), 140)]);
        let today = NaiveDate::from_ymd_opt(2026, 2, 14).unwrap();
        // 2026 is non-leap, so Feb has 28 days; 14 days elapsed → projection = 140 * 28 / 14 = 280.
        let (current, projected) = agg.current_month_total_and_projection(today);
        assert_eq!(current, 140);
        assert_eq!(projected, 280);
    }

    #[test]
    fn short_name_disambiguates_on_collision() {
        assert_eq!(short_name("/Users/a/proj", &["/Users/a/proj"]), "proj");
        assert_eq!(
            short_name("/Users/a/proj", &["/Users/a/proj", "/Users/b/proj"]),
            "a/proj"
        );
    }

    #[test]
    fn prompt_attribution_sums_assistant_tokens_per_turn() {
        let td = TempDir::new().unwrap();
        write_jsonl(
            td.path(),
            "s.jsonl",
            &[
                r#"{"type":"user","timestamp":"2026-05-19T08:00:00Z","cwd":"/p","message":{"content":"cheap prompt"}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:01Z","cwd":"/p","message":{"usage":{"output_tokens":30}}}"#,
                r#"{"type":"user","timestamp":"2026-05-19T09:00:00Z","cwd":"/p","message":{"content":"expensive prompt"}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-19T09:00:01Z","cwd":"/p","message":{"usage":{"input_tokens":100,"output_tokens":200}}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-19T09:00:02Z","cwd":"/p","message":{"usage":{"output_tokens":50}}}"#,
            ],
        );
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        let top = agg.top_prompts(5, None);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].text, "expensive prompt");
        assert_eq!(top[0].tokens, 100 + 200 + 50);
        assert_eq!(top[0].project, "/p");
        assert_eq!(top[1].text, "cheap prompt");
        assert_eq!(top[1].tokens, 30);
    }

    #[test]
    fn tool_result_user_lines_are_not_prompts() {
        let td = TempDir::new().unwrap();
        write_jsonl(
            td.path(),
            "s.jsonl",
            &[
                r#"{"type":"user","timestamp":"2026-05-19T08:00:00Z","cwd":"/p","message":{"content":"real prompt"}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:01Z","cwd":"/p","message":{"usage":{"output_tokens":10}}}"#,
                r#"{"type":"user","timestamp":"2026-05-19T08:00:02Z","cwd":"/p","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:03Z","cwd":"/p","message":{"usage":{"output_tokens":90}}}"#,
            ],
        );
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        let top = agg.top_prompts(5, None);
        // Both assistant turns attribute to the single real prompt; the
        // tool_result line does not start a new prompt.
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].text, "real prompt");
        assert_eq!(top[0].tokens, 100);
    }

    #[test]
    fn sidechain_lines_are_skipped_for_prompts() {
        let td = TempDir::new().unwrap();
        write_jsonl(
            td.path(),
            "s.jsonl",
            &[
                r#"{"type":"user","timestamp":"2026-05-19T08:00:00Z","cwd":"/p","message":{"content":"top-level prompt"}}"#,
                r#"{"type":"user","isSidechain":true,"timestamp":"2026-05-19T08:00:01Z","cwd":"/p","message":{"content":"subagent prompt"}}"#,
                r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-05-19T08:00:02Z","cwd":"/p","message":{"usage":{"output_tokens":500}}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:03Z","cwd":"/p","message":{"usage":{"output_tokens":10}}}"#,
            ],
        );
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        let top = agg.top_prompts(5, None);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].text, "top-level prompt");
        // Sidechain assistant tokens are not attributed to the prompt...
        assert_eq!(top[0].tokens, 10);
        // ...but still count toward project totals (existing behavior).
        assert_eq!(agg.by_project.get("/p").unwrap().total, 510);
    }

    #[test]
    fn multi_block_text_and_image_prompt_is_captured() {
        let td = TempDir::new().unwrap();
        write_jsonl(
            td.path(),
            "s.jsonl",
            &[
                r#"{"type":"user","timestamp":"2026-05-19T08:00:00Z","cwd":"/p","message":{"content":[{"type":"image","source":{}},{"type":"text","text":"describe this"}]}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:01Z","cwd":"/p","message":{"usage":{"output_tokens":42}}}"#,
            ],
        );
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        let top = agg.top_prompts(5, None);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].text, "describe this");
        assert_eq!(top[0].tokens, 42);
    }

    #[test]
    fn top_prompts_respects_since_window() {
        let td = TempDir::new().unwrap();
        write_jsonl(
            td.path(),
            "s.jsonl",
            &[
                r#"{"type":"user","timestamp":"2026-05-10T08:00:00Z","cwd":"/p","message":{"content":"old big prompt"}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-10T08:00:01Z","cwd":"/p","message":{"usage":{"output_tokens":1000}}}"#,
                r#"{"type":"user","timestamp":"2026-05-20T08:00:00Z","cwd":"/p","message":{"content":"recent small prompt"}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-20T08:00:01Z","cwd":"/p","message":{"usage":{"output_tokens":5}}}"#,
            ],
        );
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        let since = NaiveDate::from_ymd_opt(2026, 5, 15).unwrap();
        let recent = agg.top_prompts(5, Some(since));
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].text, "recent small prompt");
        // All-time still sees both, biggest first.
        assert_eq!(agg.top_prompts(5, None)[0].text, "old big prompt");
    }

    #[test]
    fn per_file_prompt_list_is_capped() {
        let td = TempDir::new().unwrap();
        let mut lines: Vec<String> = Vec::new();
        for i in 0..(PER_FILE_PROMPTS + 5) {
            lines.push(format!(
                r#"{{"type":"user","timestamp":"2026-05-19T08:00:00Z","cwd":"/p","message":{{"content":"prompt {i}"}}}}"#
            ));
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"2026-05-19T08:00:01Z","cwd":"/p","message":{{"usage":{{"output_tokens":{}}}}}}}"#,
                i + 1
            ));
        }
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        write_jsonl(td.path(), "s.jsonl", &refs);
        let mut agg = Aggregates::default();
        agg.refresh(td.path()).unwrap();
        let all = agg.top_prompts(1000, None);
        assert_eq!(all.len(), PER_FILE_PROMPTS);
        // The largest-token prompt is retained.
        assert_eq!(all[0].tokens, (PER_FILE_PROMPTS + 5) as u64);
    }

    #[test]
    fn version_migration_reparses_live_files_for_prompts() {
        let td = TempDir::new().unwrap();
        let cache = td.path().join("history.json");
        write_jsonl(
            td.path(),
            "s.jsonl",
            &[
                r#"{"type":"user","timestamp":"2026-05-19T08:00:00Z","cwd":"/p","message":{"content":"a prompt"}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-19T08:00:01Z","cwd":"/p","message":{"usage":{"output_tokens":7}}}"#,
            ],
        );
        // Simulate an old cache: correct day/project totals but no prompts and
        // a stale version.
        let mut old = Aggregates::default();
        old.refresh(td.path()).unwrap();
        for fe in old.file_totals.values_mut() {
            fe.top_prompts.clear();
        }
        old.version = 0;
        old.save(&cache).unwrap();

        // Loading migrates (invalidates mtimes); the next refresh repopulates.
        let mut reloaded = Aggregates::load_or_default(&cache);
        assert!(reloaded.top_prompts(5, None).is_empty());
        reloaded.refresh(td.path()).unwrap();
        let top = reloaded.top_prompts(5, None);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].text, "a prompt");
        assert_eq!(top[0].tokens, 7);
    }
}
