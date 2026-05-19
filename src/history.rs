//! Aggregates historic Claude Code token usage from local transcript JSONL.
//!
//! Walks `~/.claude/projects/**/*.jsonl`, parses assistant messages,
//! and rolls up token counts per local-time day and per project (by cwd).
//!
//! Incremental: only files whose mtime changed since the last scan are
//! re-parsed. Result is persisted as JSON so restarts populate instantly.

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate, Utc};
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

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
struct FileEntry {
    mtime_unix_ns: i128,
    by_day: BTreeMap<NaiveDate, Totals>,
    by_project: BTreeMap<String, ProjectTotals>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Aggregates {
    file_totals: BTreeMap<PathBuf, FileEntry>,
    pub by_day: BTreeMap<NaiveDate, Totals>,
    pub by_project: BTreeMap<String, ProjectTotals>,
    pub scanned_files: usize,
    pub last_scanned_at: Option<DateTime<Utc>>,
}

impl Aggregates {
    pub fn load_or_default(path: &Path) -> Self {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "history cache malformed; starting fresh");
                Aggregates::default()
            }),
            Err(_) => Aggregates::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("mkdir {parent:?}"))?;
        }
        let json = serde_json::to_vec(self)?;
        fs::write(path, json).with_context(|| format!("write {path:?}"))?;
        Ok(())
    }

    /// Walk `projects_dir`, reparse changed files, drop entries for files
    /// that no longer exist, rebuild rollups. Returns `true` if anything
    /// changed.
    pub fn refresh(&mut self, projects_dir: &Path) -> Result<bool> {
        if !projects_dir.exists() {
            return Ok(false);
        }
        let mut changed = false;
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

        for entry in walk_jsonl(projects_dir) {
            seen.insert(entry.clone());
            let mtime = mtime_ns(&entry);
            let stale = self
                .file_totals
                .get(&entry)
                .map(|fe| fe.mtime_unix_ns != mtime)
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

        // Drop entries for files that have disappeared.
        let stale_keys: Vec<PathBuf> = self
            .file_totals
            .keys()
            .filter(|k| !seen.contains(*k))
            .cloned()
            .collect();
        if !stale_keys.is_empty() {
            changed = true;
            for k in stale_keys {
                self.file_totals.remove(&k);
            }
        }

        if changed {
            self.rebuild_rollups();
            self.scanned_files = self.file_totals.len();
            self.last_scanned_at = Some(Utc::now());
        }
        Ok(changed)
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
        out.sort_by(|a, b| b.1.cmp(&a.1));
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
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    usage: Option<UsageBlock>,
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

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if line.is_empty() {
            continue;
        }
        let Ok(parsed): std::result::Result<Line, _> = serde_json::from_str(&line) else {
            continue;
        };
        if parsed.kind.as_deref() != Some("assistant") {
            continue;
        }
        let Some(message) = parsed.message else {
            continue;
        };
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
        let ts = parsed.timestamp.unwrap_or_else(Utc::now);
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
    }

    Ok(FileEntry {
        mtime_unix_ns: mtime,
        by_day,
        by_project,
    })
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
    fn removed_files_are_dropped() {
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

        fs::remove_file(&f1).unwrap();
        agg.refresh(td.path()).unwrap();
        assert_eq!(agg.scanned_files, 1);
        assert!(!agg.by_project.contains_key("/x"));
        assert!(agg.by_project.contains_key("/y"));
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

    #[test]
    fn short_name_disambiguates_on_collision() {
        assert_eq!(short_name("/Users/a/proj", &["/Users/a/proj"]), "proj");
        assert_eq!(
            short_name("/Users/a/proj", &["/Users/a/proj", "/Users/b/proj"]),
            "a/proj"
        );
    }
}
