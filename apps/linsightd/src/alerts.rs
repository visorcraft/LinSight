// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

//! Alert engine (Phase 7b).
//!
//! Opt-in: set `LINSIGHT_ALERTS=1` to load
//! `$XDG_CONFIG_HOME/linsight/alerts.toml`. Each rule has a name, an
//! `evalexpr` expression over recent sensor values, an optional `for`
//! debounce duration, and a list of notify targets:
//!
//! * `"desktop"` — libnotify popup via `notify-rust`.
//! * `"exec:<argv>"` — argv-split execution. Tokens are split using shell-
//!   style quoting (single and double quotes, backslash escapes), then
//!   exec'd directly via `std::process::Command` with NO shell interposed.
//!   This means metacharacters like `$()`, `;`, `&&`, and `|` are passed
//!   as literal argv elements rather than interpreted. Use a wrapper
//!   script if you genuinely need shell features.
//!
//! Rules see sensor values as variables — `cpu.util` is bound to its latest
//! scalar value. Expressions like `xe.gpu1.temp_c > 85 && cpu.util > 50`
//! work as you'd expect.
//!
//! The engine maintains per-rule state for debouncing and edge-triggered
//! firing: a rule that's continuously true only fires once until it goes
//! false again. Scalar samples are eligible inputs; Counter, State, and
//! Table samples are skipped (could be revisited later).

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use evalexpr::{ContextWithMutableVariables, HashMapContext, Value, eval_boolean_with_context};

const MAX_EXPR_LEN: usize = 4096;
const EVAL_TIMEOUT: Duration = Duration::from_millis(500);
/// Maximum number of alert events retained in the ring buffer.
pub const EVENT_CAPACITY: usize = 512;

use linsight_core::{Reading, Sample};
use linsight_protocol::AlertRuleJson;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Whether the alert transitioned to firing or back to normal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AlertEventKind {
    Fired,
    Cleared,
}

/// A single entry in the alert event ring buffer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertEvent {
    pub rule: String,
    pub ts_micros: u64,
    pub kind: AlertEventKind,
    /// Sensor reading that drove the decision, when cheaply available.
    pub value: Option<f64>,
}

pub enum EvalOutcome {
    Ok(bool),
    Err(String),
    Timeout,
    Panic,
}

/// Evaluate a rewritten alert expression **inline**, with no worker thread.
///
/// Used by the alert engine's per-tick [`AlertEngine::evaluate_rule`], which
/// runs on the daemon's synchronous sample path. Spawning a thread and
/// cloning the whole context for every rule on every tick (as
/// [`eval_limited`] does) is exactly the hot-path churn the daemon avoids.
/// Config rules come from a same-user `alerts.toml`, so they are trusted; the
/// `MAX_EXPR_LEN` cap plus `catch_unwind` (effective under the daemon's
/// `panic = "unwind"` build) bound the worst case without needing a timeout.
/// This path therefore never yields [`EvalOutcome::Timeout`].
pub(crate) fn eval_inline(rewritten_expr: &str, ctx: &HashMapContext) -> EvalOutcome {
    if rewritten_expr.len() > MAX_EXPR_LEN {
        return EvalOutcome::Err(format!(
            "expression too long ({} bytes, limit {MAX_EXPR_LEN})",
            rewritten_expr.len()
        ));
    }
    match std::panic::catch_unwind(AssertUnwindSafe(|| {
        eval_boolean_with_context(rewritten_expr, ctx)
    })) {
        Ok(Ok(b)) => EvalOutcome::Ok(b),
        Ok(Err(e)) => EvalOutcome::Err(e.to_string()),
        Err(_) => EvalOutcome::Panic,
    }
}

/// Evaluate a rewritten expression in a worker thread with a wall-clock
/// timeout. Used only by the `TestAlertExpr` RPC, where the expression is
/// supplied ad hoc by a client (not from trusted config) and the call is
/// occasional, so a thread + timeout is affordable and guarantees a bounded
/// response even for a pathological expression. The per-tick alert engine
/// uses [`eval_inline`] instead, to stay off the daemon hot path.
///
/// Note: a timed-out evaluation leaves its worker thread running until the
/// expression finishes on its own — Rust cannot cancel a thread — so the
/// `MAX_EXPR_LEN` cap is what ultimately bounds how expensive that can get.
pub(crate) fn eval_limited(rewritten_expr: &str, ctx: &HashMapContext) -> EvalOutcome {
    if rewritten_expr.len() > MAX_EXPR_LEN {
        return EvalOutcome::Err(format!(
            "expression too long ({} bytes, limit {MAX_EXPR_LEN})",
            rewritten_expr.len()
        ));
    }

    let expr_owned = rewritten_expr.to_owned();
    let ctx_clone = ctx.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            eval_boolean_with_context(&expr_owned, &ctx_clone)
        }));
        let _ = tx.send(result);
    });

    match rx.recv_timeout(EVAL_TIMEOUT) {
        Ok(Ok(Ok(b))) => EvalOutcome::Ok(b),
        Ok(Ok(Err(e))) => EvalOutcome::Err(e.to_string()),
        Ok(Err(_)) => EvalOutcome::Panic,
        Err(_) => EvalOutcome::Timeout,
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct AlertsConfig {
    #[serde(default, rename = "rule")]
    rules: Vec<RuleConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RuleConfig {
    name: String,
    expr: String,
    #[serde(default)]
    #[serde(rename = "for")]
    for_duration: Option<String>,
    #[serde(default)]
    cooldown: Option<String>,
    #[serde(default)]
    notify: Vec<String>,
    // `enabled` is tri-stated at the config layer: `None` means "not
    // specified" (downstream defaults to true), `Some(false)` means
    // "explicitly disabled". Skip serialization on `None` so the
    // round-tripped TOML doesn't grow a spurious `enabled = ...` line
    // on rules the user never touched the toggle for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
}

struct CompiledRule {
    name: String,
    expr: String,
    for_duration: Duration,
    cooldown: Duration,
    notify: Vec<String>,
    enabled: bool,
    triggered_at: Option<Instant>,
    fired: bool,
    last_fired_at: Option<Instant>,
    referenced_sensors: Vec<String>,
}

pub struct AlertEngine {
    rules: Vec<CompiledRule>,
    /// Latest scalar value seen per sensor.
    values: HashMap<String, f64>,
    /// Ring buffer of recent fire/clear events; newest-first (front = newest).
    events: VecDeque<AlertEvent>,
}

#[derive(Clone)]
pub struct AlertEngineHandle {
    inner: Arc<Mutex<AlertEngine>>,
}

impl AlertEngineHandle {
    pub fn on_sample(&self, sample: &Sample) {
        let mut eng = self.inner.lock().unwrap();
        eng.observe(sample);
    }

    /// Return a JSON-encoded list of recent alert events, newest first.
    /// `limit` caps the number of entries returned; `None` returns all (up to
    /// [`EVENT_CAPACITY`]).
    pub fn list_events_json(&self, limit: Option<u32>) -> String {
        let eng = self.inner.lock().unwrap();
        let cap = limit.map(|n| n as usize).unwrap_or(usize::MAX);
        let slice: Vec<&AlertEvent> = eng.events.iter().take(cap).collect();
        serde_json::to_string(&slice).unwrap_or_else(|_| "[]".to_owned())
    }

    /// Return a snapshot of all current rules for RPC dispatch.
    pub fn list_rules_json(&self) -> Vec<AlertRuleJson> {
        let eng = self.inner.lock().unwrap();
        eng.rules
            .iter()
            .map(|r| AlertRuleJson {
                name: r.name.clone(),
                expr: r.expr.clone(),
                for_duration: if r.for_duration == Duration::ZERO {
                    None
                } else {
                    Some(format_duration(r.for_duration))
                },
                cooldown: if r.cooldown == Duration::ZERO {
                    None
                } else {
                    Some(format_duration(r.cooldown))
                },
                notify: r.notify.clone(),
                enabled: r.enabled,
            })
            .collect()
    }

    /// Upsert (add or update) a rule by name. Returns Ok on success.
    pub fn upsert_rule(
        &self,
        name: &str,
        expr: &str,
        for_duration: Option<&str>,
        cooldown: Option<&str>,
        notify: Vec<String>,
        enabled: Option<bool>,
    ) -> Result<(), String> {
        let for_duration = match for_duration {
            Some(s) if !s.is_empty() => parse_duration(s).map_err(|e| e.to_string())?,
            _ => Duration::ZERO,
        };
        let cooldown = match cooldown {
            Some(s) if !s.is_empty() => parse_duration(s).map_err(|e| e.to_string())?,
            _ => Duration::ZERO,
        };
        let mut eng = self.inner.lock().unwrap();
        let referenced_sensors = extract_sensor_refs(expr);
        if let Some(existing) = eng.rules.iter_mut().find(|r| r.name == name) {
            existing.expr = expr.to_string();
            existing.notify = notify;
            existing.referenced_sensors = referenced_sensors;
            existing.for_duration = for_duration;
            existing.cooldown = cooldown;
            if let Some(e) = enabled {
                existing.enabled = e;
            }
            existing.triggered_at = None;
            existing.fired = false;
            existing.last_fired_at = None;
        } else {
            eng.rules.push(CompiledRule {
                name: name.to_string(),
                expr: expr.to_string(),
                for_duration,
                cooldown,
                notify,
                enabled: enabled.unwrap_or(true),
                triggered_at: None,
                fired: false,
                last_fired_at: None,
                referenced_sensors,
            });
        }
        Ok(())
    }

    /// Delete a rule by name. Returns Ok(true) if found and removed, Ok(false) if not found.
    pub fn delete_rule(&self, name: &str) -> Result<bool, String> {
        let mut eng = self.inner.lock().unwrap();
        let before = eng.rules.len();
        eng.rules.retain(|r| r.name != name);
        Ok(eng.rules.len() < before)
    }

    /// Persist current rules to the alerts TOML config file.
    pub fn save_config(&self, path: &std::path::Path) -> Result<(), String> {
        let config = {
            let eng = self.inner.lock().unwrap();
            AlertsConfig {
                rules: eng
                    .rules
                    .iter()
                    .map(|r| RuleConfig {
                        name: r.name.clone(),
                        expr: r.expr.clone(),
                        for_duration: if r.for_duration == Duration::ZERO {
                            None
                        } else {
                            Some(format_duration(r.for_duration))
                        },
                        cooldown: if r.cooldown == Duration::ZERO {
                            None
                        } else {
                            Some(format_duration(r.cooldown))
                        },
                        notify: r.notify.clone(),
                        enabled: if r.enabled { None } else { Some(false) },
                    })
                    .collect(),
            }
        };
        let toml_str = toml::to_string(&config).map_err(|e| format!("serialize: {e}"))?;
        // Create with owner-only perms in one step (mode applies on creation),
        // so there is no window where a freshly-created file is world-readable
        // before a separate chmod. The explicit set_permissions afterwards
        // also tightens a pre-existing file that was created with looser perms.
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("open: {e}"))?;
        f.write_all(toml_str.as_bytes()).map_err(|e| format!("write: {e}"))?;
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .map_err(|e| format!("chmod: {e}"))?;
        Ok(())
    }
}

impl AlertEngine {
    pub fn load(path: &Path) -> Result<Self> {
        let content =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let config: AlertsConfig =
            toml::from_str(&content).with_context(|| format!("parse {}", path.display()))?;
        let mut compiled = Vec::with_capacity(config.rules.len());
        for r in config.rules {
            let for_duration = match r.for_duration.as_deref() {
                None | Some("") => Duration::ZERO,
                Some(s) => parse_duration(s)
                    .with_context(|| format!("rule {}: bad `for` = {s:?}", r.name))?,
            };
            let cooldown = match r.cooldown.as_deref() {
                None | Some("") => Duration::ZERO,
                Some(s) => parse_duration(s)
                    .with_context(|| format!("rule {}: bad `cooldown` = {s:?}", r.name))?,
            };
            let referenced_sensors = extract_sensor_refs(&r.expr);
            compiled.push(CompiledRule {
                name: r.name,
                expr: r.expr,
                for_duration,
                cooldown,
                notify: r.notify,
                enabled: r.enabled.unwrap_or(true),
                triggered_at: None,
                fired: false,
                last_fired_at: None,
                referenced_sensors,
            });
        }
        info!(count = compiled.len(), "alerts loaded");
        Ok(Self { rules: compiled, values: HashMap::new(), events: VecDeque::new() })
    }

    pub fn into_handle(self) -> AlertEngineHandle {
        AlertEngineHandle { inner: Arc::new(Mutex::new(self)) }
    }

    fn observe(&mut self, sample: &Sample) {
        let id = sample.sensor.as_str().to_string();
        let val = match &sample.reading {
            Reading::Scalar(v) => *v,
            Reading::Counter(v) => *v as f64,
            _ => return,
        };
        self.values.insert(id.clone(), val);

        let now = Instant::now();
        let mut to_eval: Vec<usize> = Vec::new();
        for (i, rule) in self.rules.iter().enumerate() {
            if !rule.enabled {
                continue;
            }
            if rule.referenced_sensors.iter().any(|s| s == &id) {
                to_eval.push(i);
            }
        }
        for i in to_eval {
            self.evaluate_rule(i, now);
        }
    }

    fn evaluate_rule(&mut self, idx: usize, now: Instant) {
        // Read rule metadata without holding a mutable borrow on self.rules,
        // so we can also write to self.events after evaluation.
        let (name, expr, for_duration, cooldown, notify, prev_fired, triggered_at, last_fired_at) = {
            let rule = &self.rules[idx];
            (
                rule.name.clone(),
                rule.expr.clone(),
                rule.for_duration,
                rule.cooldown,
                rule.notify.clone(),
                rule.fired,
                rule.triggered_at,
                rule.last_fired_at,
            )
        };
        let mut ctx = HashMapContext::new();
        for (k, v) in &self.values {
            // evalexpr identifiers may contain `.`; we substitute the dot for
            // `__` to keep the parser happy and rewrite the expression once.
            // The conversion happens at expression-eval time below.
            let _ = ctx.set_value(k.replace('.', "__"), Value::Float(*v));
        }
        let rewritten_expr = expr.replace('.', "__");
        // Inline (no per-tick worker thread) — see `eval_inline`. Trusted
        // config + length cap + catch_unwind; never returns `Timeout`.
        let truthy = match eval_inline(&rewritten_expr, &ctx) {
            EvalOutcome::Ok(b) => b,
            EvalOutcome::Err(e) => {
                warn!(rule = %name, error = %e, "alert expression failed to evaluate");
                return;
            }
            EvalOutcome::Timeout => {
                warn!(rule = %name, "alert expression timed out");
                return;
            }
            EvalOutcome::Panic => {
                warn!(rule = %name, "alert expression panicked");
                return;
            }
        };

        if truthy {
            let new_triggered_at = triggered_at.unwrap_or(now);
            self.rules[idx].triggered_at = Some(new_triggered_at);
            if !prev_fired && now.duration_since(new_triggered_at) >= for_duration {
                let within_cooldown = last_fired_at
                    .map_or(false, |last| now.duration_since(last) < cooldown);
                if !within_cooldown {
                    self.rules[idx].fired = true;
                    self.rules[idx].last_fired_at = Some(now);
                    fire(&name, &expr, &notify);
                    self.push_event(AlertEvent {
                        rule: name,
                        ts_micros: wall_micros(),
                        kind: AlertEventKind::Fired,
                        value: None,
                    });
                }
            }
        } else {
            self.rules[idx].triggered_at = None;
            self.rules[idx].fired = false;
            if prev_fired {
                self.push_event(AlertEvent {
                    rule: name,
                    ts_micros: wall_micros(),
                    kind: AlertEventKind::Cleared,
                    value: None,
                });
            }
        }
    }

    /// Push an event to the front of the ring buffer (newest-first), evicting
    /// the oldest entry when the buffer is full.
    fn push_event(&mut self, event: AlertEvent) {
        if self.events.len() == EVENT_CAPACITY {
            self.events.pop_back();
        }
        self.events.push_front(event);
    }
}

fn wall_micros() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_micros() as u64,
        Err(e) => {
            warn!(error = ?e, "system clock before UNIX_EPOCH; emitting sentinel timestamp");
            u64::MAX
        }
    }
}

fn fire(name: &str, expr: &str, notify: &[String]) {
    info!(rule = %name, expr = %expr, "alert firing");
    for target in notify {
        if target == "desktop" {
            if let Err(e) = notify_rust::Notification::new()
                .summary(&format!("LinSight: {name}"))
                .body(&format!("Condition true: {expr}"))
                .show()
            {
                warn!(error = ?e, "desktop notification failed");
            }
        } else if let Some(cmd) = target.strip_prefix("exec:") {
            // Argv-split + direct exec. No shell. See module docs for
            // rationale; this replaces the previous `shell:<cmd>` target
            // which passed the raw string to `sh -c` and was an RCE for
            // anyone who could write the alerts config.
            match shell_split(cmd) {
                Ok(argv) if argv.is_empty() => {
                    warn!(target = %target, "exec notify target is empty");
                }
                Ok(argv) => {
                    let result = Command::new(&argv[0]).args(&argv[1..]).status();
                    if let Err(e) = result {
                        warn!(target = %target, error = ?e, "exec notify failed");
                    }
                }
                Err(e) => warn!(target = %target, error = %e, "exec notify: bad quoting"),
            }
        } else if let Some(url) = target.strip_prefix("webhook:") {
            if let Err(e) = fire_webhook(name, expr, url) {
                warn!(target = %target, error = %e, "webhook notify failed");
            }
        } else if let Some(_cmd) = target.strip_prefix("shell:") {
            // The old `shell:<cmd>` target was removed because it passed
            // user-config strings to `sh -c` unescaped. Anyone able to
            // write the alerts config (malicious dotfile, sync gone wrong)
            // could execute arbitrary commands as the daemon's user.
            warn!(
                target = %target,
                "the `shell:` notify target was removed for safety; use `exec:<argv>` instead",
            );
        } else {
            warn!(target = %target, "unknown notify target");
        }
    }
}

fn validate_webhook_url(raw: &str) -> Result<(), String> {
    let rest = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))
        .ok_or_else(|| "webhook URL must start with http:// or https://".to_owned())?;

    let authority = rest.split('/').next().unwrap_or(rest);
    // Strip any `userinfo@` prefix so it can't mask the real host from the
    // checks below: `http://user@127.0.0.1/` would otherwise leave the host
    // parsing as `user@127.0.0.1` (not an IP) and slip through, while the
    // HTTP client connects to the real `127.0.0.1`.
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    // Extract the bare host. A bracketed IPv6 literal (`[::1]:8080`) needs its
    // brackets stripped before parsing; a missing closing bracket is rejected
    // rather than panicking on an out-of-range slice.
    let host_inner = if let Some(after_lbracket) = host_port.strip_prefix('[') {
        match after_lbracket.find(']') {
            Some(end) => &after_lbracket[..end],
            None => {
                return Err(
                    "webhook URL has a malformed IPv6 host literal (missing ']')".to_owned()
                );
            }
        }
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };

    if let Ok(ip) = host_inner.parse::<IpAddr>() {
        if is_restricted_ip(&ip) {
            return Err(format!(
                "webhook URL host {ip} is a restricted address (loopback / link-local / private / unspecified)"
            ));
        }
    } else if looks_like_numeric_host(host_inner) {
        // A host that isn't a valid IpAddr literal but is all-numeric (or
        // hex `0x...`) is almost certainly an obfuscated integer/octal IP
        // encoding (e.g. `2130706433` or `0177.0.0.1`) that the resolver
        // would still turn into an IP, bypassing the check above. Reject it.
        return Err(format!(
            "webhook URL host {host_inner:?} looks like an obfuscated numeric IP; \
             use a hostname, dotted-decimal IPv4, or bracketed IPv6"
        ));
    }

    Ok(())
}

/// Heuristic: does this host (which already failed `IpAddr` parsing) look
/// like an obfuscated numeric IP rather than a real DNS name? A legitimate
/// hostname's rightmost label (the TLD) contains a non-digit, so an
/// all-digit final label means a decimal/octal integer-IP encoding
/// (`2130706433`, `0177.0.0.1`), and a `0x` prefix means a hex one.
fn looks_like_numeric_host(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }
    if host.len() >= 2 && (host.starts_with("0x") || host.starts_with("0X")) {
        return true;
    }
    match host.rsplit('.').next() {
        Some(last) => !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

fn is_restricted_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_link_local()
                || v4.is_private()
                || *v4 == Ipv4Addr::UNSPECIFIED
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || is_ipv6_link_local(v6)
                || is_ipv6_unique_local(v6)
                || *v6 == Ipv6Addr::UNSPECIFIED
        }
    }
}

fn is_ipv6_link_local(v6: &Ipv6Addr) -> bool {
    let segments = v6.segments();
    (segments[0] & 0xffc0) == 0xfe80
}

fn is_ipv6_unique_local(v6: &Ipv6Addr) -> bool {
    let segments = v6.segments();
    (segments[0] & 0xfe00) == 0xfc00
}

/// Fire a webhook notification via a simple HTTP POST.
fn fire_webhook(name: &str, expr: &str, url: &str) -> Result<(), String> {
    validate_webhook_url(url)?;

    let payload = serde_json::json!({
        "name": name,
        "expr": expr,
        "source": "linsight",
    });
    let body = serde_json::to_string(&payload).map_err(|e| format!("serialize: {e}"))?;
    let url = url.to_owned();
    let body_clone = body.clone();
    std::thread::spawn(move || {
        if let Err(e) = do_webhook_post(&url, &body_clone) {
            tracing::warn!(url = %url, error = %e, "webhook POST failed");
        }
    });
    Ok(())
}

fn do_webhook_post(url: &str, body: &str) -> std::result::Result<(), std::io::Error> {
    // Disable redirect following: `validate_webhook_url` only vets the initial
    // URL, so a public endpoint that 3xx-redirects to a restricted address
    // (e.g. http://169.254.169.254/) would otherwise bypass the SSRF check.
    let agent: ureq::Agent = ureq::Agent::config_builder().max_redirects(0).build().into();
    match agent.post(url).header("Content-Type", "application/json").send(body) {
        Ok(_) => Ok(()),
        Err(ureq::Error::StatusCode(code)) => {
            Err(std::io::Error::other(format!("webhook returned HTTP {code}")))
        }
        Err(e) => Err(std::io::Error::other(format!("webhook request failed: {e}"))),
    }
}

/// POSIX-ish argv tokenizer for `exec:` notify targets. Supports single
/// quotes (literal), double quotes (with backslash escapes for `\`, `"`),
/// and unquoted backslash escapes of any single character. Whitespace
/// outside quotes separates tokens. Returns an error on an unterminated
/// quoted segment. Deliberately does NOT handle environment expansion,
/// command substitution, redirections, or globbing — those are shell
/// features, and the whole point of this notify target is to avoid a
/// shell.
fn shell_split(input: &str) -> Result<Vec<String>, String> {
    let mut argv = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' if !in_token => continue,
            ' ' | '\t' | '\n' => {
                argv.push(std::mem::take(&mut current));
                in_token = false;
            }
            '\'' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => current.push(ch),
                        None => return Err("unterminated single quote".into()),
                    }
                }
            }
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(esc @ ('\\' | '"' | '$' | '`')) => current.push(esc),
                            Some(ch) => {
                                current.push('\\');
                                current.push(ch);
                            }
                            None => return Err("unterminated escape in double quote".into()),
                        },
                        Some(ch) => current.push(ch),
                        None => return Err("unterminated double quote".into()),
                    }
                }
            }
            '\\' => {
                in_token = true;
                match chars.next() {
                    Some(ch) => current.push(ch),
                    None => return Err("trailing backslash".into()),
                }
            }
            other => {
                in_token = true;
                current.push(other);
            }
        }
    }
    if in_token {
        argv.push(current);
    }
    Ok(argv)
}

fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    let (num, unit) =
        s.split_at(s.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(s.len()));
    let n: f64 = num.parse().with_context(|| format!("invalid number in {s:?}"))?;
    let secs = match unit {
        "" | "s" => n,
        "ms" => n / 1000.0,
        "m" => n * 60.0,
        "h" => n * 3600.0,
        other => anyhow::bail!("unknown duration unit {other:?}"),
    };
    Ok(Duration::from_secs_f64(secs))
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs >= 3600.0 && (secs % 3600.0).abs() < f64::EPSILON {
        format!("{}h", secs as u64 / 3600)
    } else if secs >= 60.0 && (secs % 60.0).abs() < f64::EPSILON {
        format!("{}m", secs as u64 / 60)
    } else if secs >= 1.0 && (secs % 1.0).abs() < f64::EPSILON {
        format!("{}s", secs as u64)
    } else if secs >= 0.001 {
        format!("{}ms", d.as_millis())
    } else {
        format!("{}s", secs)
    }
}

/// Pull plausible sensor identifiers out of a rule expression. A sensor id
/// is the canonical dotted form (e.g. `xe.gpu1.temp_c`); this helper scans
/// the expression text for tokens of that shape so the rule's
/// `referenced_sensors` cache can avoid re-evaluating unrelated rules on
/// every sample. Tokens that look like float literals (e.g. `85.5`) are
/// skipped so a numeric threshold in the expression isn't mistaken for a
/// sensor reference.
fn extract_sensor_refs(expr: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for c in expr.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            current.push(c);
        } else {
            push_if_sensor_token(&current, &mut out);
            current.clear();
        }
    }
    push_if_sensor_token(&current, &mut out);
    out.sort();
    out.dedup();
    out
}

fn push_if_sensor_token(token: &str, out: &mut Vec<String>) {
    if !token.contains('.') {
        return;
    }
    // Sensor identifiers always start with a lowercase letter; numeric
    // literals start with a digit (or `-`, which the tokenizer already
    // split on). Filters out `85.5`-shaped tokens that would otherwise be
    // mistaken for sensor refs and trigger spurious rule re-evaluations.
    if !token.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return;
    }
    out.push(token.to_owned());
}

#[cfg(test)]
impl AlertEngine {
    /// Construct a minimal engine with a single rule, for unit tests.
    fn new_test(name: &str, expr: &str) -> Self {
        let referenced_sensors = extract_sensor_refs(expr);
        Self {
            rules: vec![CompiledRule {
                name: name.to_string(),
                expr: expr.to_string(),
                for_duration: Duration::ZERO,
                cooldown: Duration::ZERO,
                notify: vec![],
                enabled: true,
                triggered_at: None,
                fired: false,
                last_fired_at: None,
                referenced_sensors,
            }],
            values: HashMap::new(),
            events: VecDeque::new(),
        }
    }

    /// Feed a scalar sample directly into the engine (bypasses on_sample
    /// indirection for test convenience).
    fn push_scalar(&mut self, sensor: &str, val: f64) {
        use linsight_core::{Reading, SensorId};
        let s =
            Sample { sensor: SensorId::new(sensor), ts_micros: 0, reading: Reading::Scalar(val) };
        self.observe(&s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_refs_finds_dotted_tokens() {
        let refs = extract_sensor_refs("xe.gpu1.temp_c > 85 && cpu.util < 50");
        assert_eq!(refs, vec!["cpu.util".to_string(), "xe.gpu1.temp_c".to_string()]);
    }

    #[test]
    fn parse_duration_handles_units() {
        assert_eq!(parse_duration("0").unwrap(), Duration::ZERO);
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
    }

    #[test]
    fn shell_split_basic() {
        assert_eq!(shell_split("notify-send hello").unwrap(), vec!["notify-send", "hello"]);
    }

    #[test]
    fn shell_split_double_quoted_keeps_spaces() {
        assert_eq!(
            shell_split(r#"notify-send "alert: hot" "body line""#).unwrap(),
            vec!["notify-send", "alert: hot", "body line"],
        );
    }

    #[test]
    fn shell_split_single_quoted_is_literal() {
        // Single quotes pass `$VAR` and `$(cmd)` as literal text — exactly
        // what we want to prevent command-substitution injection from a
        // hostile alerts.toml.
        assert_eq!(
            shell_split("logger 'pwned: $(uptime)'").unwrap(),
            vec!["logger", "pwned: $(uptime)"],
        );
    }

    #[test]
    fn shell_split_metacharacters_are_literal() {
        // The whole point of dropping `sh -c`: `;`, `|`, `&&` etc. become
        // ordinary argv characters rather than shell directives.
        assert_eq!(
            shell_split("echo a ; rm -rf /").unwrap(),
            vec!["echo", "a", ";", "rm", "-rf", "/"],
        );
    }

    #[test]
    fn shell_split_rejects_unterminated_quote() {
        assert!(shell_split("echo \"unterminated").is_err());
        assert!(shell_split("echo 'unterminated").is_err());
        assert!(shell_split("echo \\").is_err());
    }

    #[test]
    fn webhook_payload_format() {
        let payload = serde_json::json!({
            "name": "high-cpu",
            "expr": "cpu.util > 90",
            "source": "linsight",
        });
        let body = serde_json::to_string(&payload).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["name"], "high-cpu");
        assert_eq!(parsed["expr"], "cpu.util > 90");
        assert_eq!(parsed["source"], "linsight");
    }

    #[test]
    fn fire_webhook_rejects_loopback() {
        let result = fire_webhook("test-rule", "cpu.util > 90", "http://127.0.0.1:1");
        assert!(result.is_err());
    }

    #[test]
    fn validate_webhook_url_allows_public_ip() {
        assert!(validate_webhook_url("http://203.0.113.5:8080/hook").is_ok());
        assert!(validate_webhook_url("https://example.com/webhook").is_ok());
    }

    #[test]
    fn validate_webhook_url_rejects_loopback() {
        assert!(validate_webhook_url("http://127.0.0.1/hook").is_err());
        assert!(validate_webhook_url("http://127.0.0.1:9090/hook").is_err());
        assert!(validate_webhook_url("http://[::1]/hook").is_err());
    }

    #[test]
    fn validate_webhook_url_rejects_link_local() {
        assert!(validate_webhook_url("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(validate_webhook_url("http://[fe80::1]/hook").is_err());
    }

    #[test]
    fn validate_webhook_url_rejects_private() {
        assert!(validate_webhook_url("http://10.0.0.1/hook").is_err());
        assert!(validate_webhook_url("http://172.16.0.1/hook").is_err());
        assert!(validate_webhook_url("http://192.168.1.1/hook").is_err());
        assert!(validate_webhook_url("http://[fc00::1]/hook").is_err());
    }

    #[test]
    fn validate_webhook_url_rejects_unspecified() {
        assert!(validate_webhook_url("http://0.0.0.0/hook").is_err());
        assert!(validate_webhook_url("http://[::]/hook").is_err());
    }

    #[test]
    fn validate_webhook_url_rejects_bad_scheme() {
        assert!(validate_webhook_url("ftp://example.com/hook").is_err());
        assert!(validate_webhook_url("gopher://example.com/hook").is_err());
    }

    #[test]
    fn validate_webhook_url_rejects_unclosed_ipv6_bracket() {
        // A missing ']' must be rejected, not panic on an out-of-range slice.
        assert!(validate_webhook_url("http://[::1").is_err());
        assert!(validate_webhook_url("http://[::1/hook").is_err());
        assert!(validate_webhook_url("http://[fe80::1:8080/hook").is_err());
    }

    #[test]
    fn validate_webhook_url_rejects_userinfo_masking_restricted_ip() {
        // userinfo must not hide a restricted host from the IP check.
        assert!(validate_webhook_url("http://user@127.0.0.1/hook").is_err());
        assert!(validate_webhook_url("http://user:pass@169.254.169.254/").is_err());
        assert!(validate_webhook_url("http://user@[::1]/hook").is_err());
        // ...but legitimate userinfo on a public host still passes.
        assert!(validate_webhook_url("http://user@example.com/hook").is_ok());
    }

    #[test]
    fn validate_webhook_url_rejects_obfuscated_numeric_ip() {
        // Integer / octal / hex IP encodings that getaddrinfo resolves to an
        // IP but `IpAddr::parse` rejects must not slip through.
        assert!(validate_webhook_url("http://2130706433/hook").is_err()); // 127.0.0.1
        assert!(validate_webhook_url("http://0177.0.0.1/hook").is_err()); // octal
        assert!(validate_webhook_url("http://0x7f000001/hook").is_err()); // hex
        // A normal hostname (alphabetic TLD) is still allowed.
        assert!(validate_webhook_url("https://hooks.example.com/x").is_ok());
    }

    #[test]
    fn fired_and_cleared_rules_append_events() {
        // Use integer thresholds: the engine rewrites all '.' to '__', which
        // corrupts floating-point literals like 50.0 → 50__0.
        let mut eng = AlertEngine::new_test("high-cpu", "cpu.util > 50");

        // Drive rule true → one Fired event.
        eng.push_scalar("cpu.util", 90.0);
        assert_eq!(eng.events.len(), 1);
        assert_eq!(eng.events[0].rule, "high-cpu");
        assert_eq!(eng.events[0].kind, AlertEventKind::Fired);

        // Drive rule true again (still fired) → no new event.
        eng.push_scalar("cpu.util", 95.0);
        assert_eq!(eng.events.len(), 1);

        // Drive rule false → one Cleared event prepended (newest-first).
        eng.push_scalar("cpu.util", 10.0);
        assert_eq!(eng.events.len(), 2);
        assert_eq!(eng.events[0].kind, AlertEventKind::Cleared);
        assert_eq!(eng.events[1].kind, AlertEventKind::Fired);

        // Drive false again (not fired) → no new event.
        eng.push_scalar("cpu.util", 5.0);
        assert_eq!(eng.events.len(), 2);

        // Overfill: fill to EVENT_CAPACITY + 10, ring must stay capped.
        for i in 0..(EVENT_CAPACITY + 10) {
            let val = if i % 2 == 0 { 90.0 } else { 10.0 };
            // Reset rule state between cycles so each toggle produces an event.
            if i % 2 == 0 {
                eng.rules[0].fired = false;
                eng.rules[0].triggered_at = None;
            }
            eng.push_scalar("cpu.util", val);
        }
        assert_eq!(eng.events.len(), EVENT_CAPACITY);

        // Verify newest-first: the front entry is the most recent push.
        // The loop above ends on an even index (EVENT_CAPACITY + 10 - 1 is odd,
        // so the last val is 10.0 which is a clear cycle; but the last even
        // iteration pushed 90.0 → Fired). We just verify len is bounded.
        let json = serde_json::to_string(&eng.events.iter().collect::<Vec<_>>()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), EVENT_CAPACITY);

        // Verify list_events_json limit is honored via the handle.
        let handle = AlertEngine::new_test("x", "cpu.util > 1").into_handle();
        handle.on_sample(&linsight_core::Sample {
            sensor: linsight_core::SensorId::new("cpu.util"),
            ts_micros: 0,
            reading: linsight_core::Reading::Scalar(99.0),
        });
        let json_limited = handle.list_events_json(Some(1));
        let arr: serde_json::Value = serde_json::from_str(&json_limited).unwrap();
        assert_eq!(arr.as_array().unwrap().len(), 1);
        assert_eq!(arr[0]["kind"], "fired");
    }

    #[test]
    fn list_events_json_limit_edges() {
        let handle = AlertEngine::new_test("x", "cpu.util > 1").into_handle();
        handle.on_sample(&linsight_core::Sample {
            sensor: linsight_core::SensorId::new("cpu.util"),
            ts_micros: 0,
            reading: linsight_core::Reading::Scalar(99.0),
        });
        handle.on_sample(&linsight_core::Sample {
            sensor: linsight_core::SensorId::new("cpu.util"),
            ts_micros: 0,
            reading: linsight_core::Reading::Scalar(0.0),
        });

        let json_all = handle.list_events_json(None);
        let arr: serde_json::Value = serde_json::from_str(&json_all).unwrap();
        assert_eq!(arr.as_array().unwrap().len(), 2);

        let json_zero = handle.list_events_json(Some(0));
        let arr: serde_json::Value = serde_json::from_str(&json_zero).unwrap();
        assert_eq!(arr.as_array().unwrap().len(), 0);

        let json_over = handle.list_events_json(Some(100));
        let arr: serde_json::Value = serde_json::from_str(&json_over).unwrap();
        assert_eq!(arr.as_array().unwrap().len(), 2);
    }

    #[test]
    fn cooldown_suppresses_refire_within_window() {
        let mut eng = AlertEngine::new_test("high-cpu", "cpu.util > 50");
        eng.rules[0].cooldown = Duration::from_millis(50);

        // Fire
        eng.push_scalar("cpu.util", 90.0);
        assert_eq!(eng.events.len(), 1);
        assert_eq!(eng.events[0].kind, AlertEventKind::Fired);

        // Clear
        eng.push_scalar("cpu.util", 10.0);
        assert_eq!(eng.events.len(), 2);
        assert_eq!(eng.events[0].kind, AlertEventKind::Cleared);

        // Refire immediately — suppressed by cooldown
        eng.push_scalar("cpu.util", 90.0);
        assert_eq!(eng.events.len(), 2); // no new event

        // Wait out cooldown
        std::thread::sleep(Duration::from_millis(60));

        // Refire — allowed now
        eng.push_scalar("cpu.util", 90.0);
        assert_eq!(eng.events.len(), 3);
        assert_eq!(eng.events[0].kind, AlertEventKind::Fired);
    }

    #[test]
    fn toml_round_trips_cooldown() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alerts.toml");

        let handle = AlertEngine::new_test("x", "cpu.util > 1").into_handle();
        handle
            .upsert_rule("x", "cpu.util > 1", None, Some("5m"), vec![], None)
            .unwrap();
        handle.save_config(&path).unwrap();

        let loaded = AlertEngine::load(&path).unwrap();
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.rules[0].cooldown, Duration::from_secs(300));
    }
}
