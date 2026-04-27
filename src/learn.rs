// Recipe learn — synth deterministic rules from sample data.
//
// Inputs: a swagger spec, a target definition name, and a slice of JSON sample
// rows. Outputs: a `LearnPlan` that names per-field decisions (apply faker
// strategy, apply custom list, apply field-level rule, or skip + reason).
//
// Pure module — no I/O except `read_samples` (a small reader helper for the
// CLI). All detection is regex + simple stats; no LLM, no network, no fs.
//
// Field-path convention matches `composer::parse_faker_rules`:
//   "<DefName>.<propName>"
//
// Custom-list naming convention:
//   learned_<def_snake>_<prop_snake>
// where `<x_snake>` is the original name with '-' converted to '_' and any
// uppercase letter prefixed by '_'. The convention is internal to learn — once
// written, the name lives in custom_lists like any other list.
//
// Detection priority per field (apply first match):
//   1. non-null sample count < min_samples       → skip "low_samples"
//   2. all values identical (1 distinct)          → Const rule
//   3. all string samples match a format regex    → faker_rule (uuid/email/…)
//   4. distinct count <= max_choice               → Choice rule
//   5. all string and distinct <= max_list        → custom_list + faker_rule
//   6. all numeric and distinct > max_choice      → Range rule
//   7. distinct > max_list                        → skip "too_distinct"

#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};

use serde_json::Value;

use crate::parser::{SchemaObject, SwaggerSpec};
use crate::rules::{Rule, split_field_path};

// ---------------------------------------------------------------------------
// Config + policy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LearnConfig {
    pub max_choice: usize,
    pub max_list: usize,
    pub min_samples: usize,
    /// Cap how many JSON-array elements / JSONL lines are read. `None` means
    /// no cap (caller is responsible for bounded input).
    pub max_samples: Option<usize>,
}

impl Default for LearnConfig {
    fn default() -> Self {
        Self {
            max_choice: 20,
            max_list: 200,
            min_samples: 5,
            max_samples: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Only fill empty slots. Existing entries are preserved; overlapping
    /// proposals are skipped + reported.
    Merge,
    /// Replace any existing entry the proposal collides with.
    Overwrite,
    /// Error on first collision (no writes performed).
    Fail,
}

// ---------------------------------------------------------------------------
// Existing config view (input to apply_plan)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct CurrentConfig {
    /// Map of `"DefName.propName" -> "strategy_str"`. Strings only — matches
    /// `composer::parse_faker_rules` wire shape.
    pub faker_rules: HashMap<String, String>,
    /// Existing recipe rules (field-level + compare).
    pub rules: Vec<Rule>,
    /// Map of `list_name -> values`.
    pub custom_lists: HashMap<String, Vec<String>>,
    /// Other config slots passed through unchanged.
    pub quantity_configs: Value,
    pub frozen_rows: Value,
}

#[derive(Debug, Clone, Default)]
pub struct NewConfig {
    pub faker_rules: HashMap<String, String>,
    pub rules: Vec<Rule>,
    pub custom_lists: HashMap<String, Vec<String>>,
    pub quantity_configs: Value,
    pub frozen_rows: Value,
}

// ---------------------------------------------------------------------------
// Plan + report data
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::enum_variant_names)]
pub enum ProposedAction {
    /// Set faker_rules[field_path] to a strategy string (built-in or list name).
    SetFaker { strategy: String },
    /// Define a custom_list AND set faker_rules[field_path] = list_name.
    SetCustomList {
        list_name: String,
        values: Vec<String>,
    },
    /// Set a field-level rule (Const / Choice / Range).
    SetRule { rule: Rule },
}

#[derive(Debug, Clone)]
pub struct ProposedRule {
    /// `<DefName>.<propName>`
    pub field: String,
    pub action: ProposedAction,
}

#[derive(Debug, Clone)]
pub struct SkippedField {
    pub field: String,
    pub reason: String,
    pub detail: Value,
}

#[derive(Debug, Clone, Default)]
pub struct LearnPlan {
    pub def_name: String,
    pub proposed: Vec<ProposedRule>,
    pub skipped: Vec<SkippedField>,
    pub warnings: Vec<String>,
    pub collisions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AppliedEntry {
    pub field: String,
    pub kind: String,
    pub detail: Value,
}

#[derive(Debug, Clone, Default)]
pub struct ApplyReport {
    pub applied: Vec<AppliedEntry>,
    pub skipped: Vec<SkippedField>,
    pub warnings: Vec<String>,
    pub collisions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ConflictError {
    pub field: String,
    pub slot: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Field-name normalize
// ---------------------------------------------------------------------------

/// Normalize a field name for cross-form matching.
///
/// Lowercase + drop `_` and `-`. So `userEmail`, `user_email`, `user-email`,
/// `USER_EMAIL` all collapse to `useremail`.
pub fn normalize_field_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '_' || c == '-' {
            continue;
        }
        for lc in c.to_lowercase() {
            out.push(lc);
        }
    }
    out
}

/// snake_case slug for list-name synthesis.
fn to_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev_lower = false;
    for c in s.chars() {
        if c == '-' || c == '_' {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() {
            if prev_lower {
                out.push('_');
            }
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            prev_lower = false;
        } else {
            out.push(c);
            prev_lower = c.is_lowercase() || c.is_ascii_digit();
        }
    }
    out
}

/// Build the conventional custom-list name for a (def, prop) pair.
pub fn list_name_for(def: &str, prop: &str) -> String {
    format!("learned_{}_{}", to_snake(def), to_snake(prop))
}

// ---------------------------------------------------------------------------
// Format detectors
// ---------------------------------------------------------------------------

/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` (case-insensitive hex).
pub fn is_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, c) in bytes.iter().enumerate() {
        match i {
            8 | 13 | 18 | 23 => {
                if *c != b'-' {
                    return false;
                }
            }
            _ => {
                if !c.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }
    true
}

/// Conservative email shape: `<local>@<domain>.<tld>` with no whitespace and
/// at least one dot in the host.
pub fn is_email(s: &str) -> bool {
    if s.is_empty() || s.len() > 320 {
        return false;
    }
    let at = match s.find('@') {
        Some(i) => i,
        None => return false,
    };
    if s[at + 1..].find('@').is_some() {
        return false;
    }
    let local = &s[..at];
    let host = &s[at + 1..];
    if local.is_empty() || host.is_empty() {
        return false;
    }
    if local.chars().any(|c| c.is_whitespace()) || host.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    if !host.contains('.') {
        return false;
    }
    let last_dot = host.rfind('.').unwrap();
    let tld = &host[last_dot + 1..];
    if tld.len() < 2 || !tld.chars().all(|c| c.is_alphanumeric()) {
        return false;
    }
    true
}

/// Dotted-quad IPv4 address.
pub fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    for p in parts {
        if p.is_empty() || p.len() > 3 {
            return false;
        }
        if !p.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        match p.parse::<u32>() {
            Ok(n) if n <= 255 => {}
            _ => return false,
        }
    }
    true
}

/// RFC 3339 / ISO 8601 date (date-only `YYYY-MM-DD` or full datetime).
pub fn is_date(s: &str) -> bool {
    if s.len() < 10 {
        return false;
    }
    // Accept date-only YYYY-MM-DD.
    if s.len() == 10 {
        return chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok();
    }
    // Accept date-time forms via chrono::DateTime parsing.
    if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
        return true;
    }
    if chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").is_ok() {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Sample reader
// ---------------------------------------------------------------------------

/// Read sample rows from a reader. Auto-detects JSON array vs JSONL by the
/// first non-whitespace byte (`[` → array; otherwise JSONL).
///
/// Returns `Err` on malformed input. Empty or whitespace-only input returns
/// an empty vector. JSONL blank lines are skipped; comment lines are not
/// supported.
pub fn read_samples(reader: impl Read) -> Result<Vec<Value>, String> {
    let mut buffered = BufReader::new(reader);

    // Peek first non-whitespace byte.
    let mut first: Option<u8> = None;
    let mut leading: Vec<u8> = Vec::new();
    loop {
        let buf = buffered
            .fill_buf()
            .map_err(|e| format!("read error: {e}"))?;
        if buf.is_empty() {
            break;
        }
        let mut idx = 0;
        let mut found = false;
        for (i, b) in buf.iter().enumerate() {
            if !b.is_ascii_whitespace() {
                first = Some(*b);
                idx = i;
                found = true;
                break;
            }
        }
        if found {
            // Stash the leading whitespace + the non-ws byte and beyond.
            leading.extend_from_slice(buf);
            let consumed = buf.len();
            buffered.consume(consumed);
            // Strip just the prefix whitespace so we hand the rest to the parser.
            leading.drain(..idx);
            break;
        }
        // All whitespace — consume and continue.
        let consumed = buf.len();
        buffered.consume(consumed);
    }

    let Some(first_byte) = first else {
        return Ok(Vec::new());
    };

    // Reattach `leading` ahead of `buffered` for downstream readers.
    let combined = std::io::Cursor::new(leading).chain(buffered);

    if first_byte == b'[' {
        // JSON array: read whole thing.
        let mut s = String::new();
        let mut combined = combined;
        combined
            .read_to_string(&mut s)
            .map_err(|e| format!("read error: {e}"))?;
        let parsed: Value =
            serde_json::from_str(&s).map_err(|e| format!("invalid JSON array: {e}"))?;
        let arr = parsed
            .as_array()
            .ok_or_else(|| "expected JSON array".to_string())?
            .clone();
        return Ok(arr);
    }

    // JSONL: one JSON value per non-blank line.
    let buffered = BufReader::new(combined);
    let mut out = Vec::new();
    for (lineno, line) in buffered.lines().enumerate() {
        let line = line.map_err(|e| format!("read error: {e}"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(trimmed)
            .map_err(|e| format!("invalid JSONL on line {}: {}", lineno + 1, e))?;
        out.push(v);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Plan computation
// ---------------------------------------------------------------------------

/// Walk a definition's properties + the supplied sample rows and produce a
/// `LearnPlan` of proposed rules + per-field skip reasons.
///
/// `samples` is bounded by caller. `config.max_samples` is informational here
/// (already enforced by `read_samples`).
pub fn plan_learn(
    spec: &SwaggerSpec,
    def_name: &str,
    samples: &[Value],
    config: &LearnConfig,
) -> LearnPlan {
    let mut plan = LearnPlan {
        def_name: def_name.to_string(),
        ..Default::default()
    };

    let defs = match spec.definitions.as_ref() {
        Some(d) => d,
        None => {
            plan.warnings.push(format!(
                "spec has no definitions (looking for '{def_name}')"
            ));
            return plan;
        }
    };

    let schema = match defs.get(def_name) {
        Some(s) => s,
        None => {
            plan.warnings
                .push(format!("definition '{def_name}' not found in spec"));
            return plan;
        }
    };

    // Effective property map: prefer direct .properties, else array-wrapped
    // items.properties (mirrors seeder::effective_props).
    let props = if let Some(p) = schema.properties.as_ref() {
        p
    } else if schema.schema_type.as_deref() == Some("array")
        && let Some(items) = schema.items.as_ref()
        && let Some(p) = items.properties.as_ref()
    {
        p
    } else {
        plan.warnings
            .push(format!("definition '{def_name}' has no properties"));
        return plan;
    };

    // Detect spec-side normalize collisions: two distinct prop names that
    // collapse to the same normalized form.
    let mut norm_to_orig: HashMap<String, Vec<String>> = HashMap::new();
    for prop_name in props.keys() {
        norm_to_orig
            .entry(normalize_field_name(prop_name))
            .or_default()
            .push(prop_name.clone());
    }
    let mut colliding: HashSet<String> = HashSet::new();
    for (norm, originals) in &norm_to_orig {
        if originals.len() > 1 {
            let mut sorted = originals.clone();
            sorted.sort();
            plan.collisions.push(format!(
                "spec normalize collision: [{}] all collapse to '{}'",
                sorted.join(", "),
                norm
            ));
            for o in originals {
                colliding.insert(o.clone());
            }
        }
    }

    // Index samples by normalized key. Do not pre-flatten: walk each sample,
    // gather its top-level object entries, and bucket them.
    let mut sample_keys_seen: HashSet<String> = HashSet::new();
    let mut field_samples: HashMap<String, Vec<Value>> = HashMap::new();
    for sample in samples {
        let Some(obj) = sample.as_object() else {
            continue;
        };
        for (k, v) in obj {
            sample_keys_seen.insert(k.clone());
            let norm = normalize_field_name(k);
            field_samples.entry(norm).or_default().push(v.clone());
        }
    }

    // Report sample keys that match no spec property (after normalize).
    let spec_norms: HashSet<String> = props.keys().map(|k| normalize_field_name(k)).collect();
    let mut unmatched: Vec<String> = sample_keys_seen
        .iter()
        .filter(|k| !spec_norms.contains(&normalize_field_name(k)))
        .cloned()
        .collect();
    unmatched.sort();
    for k in unmatched {
        plan.skipped.push(SkippedField {
            field: k.clone(),
            reason: "sample_key_not_in_spec".to_string(),
            detail: Value::Null,
        });
    }

    // Walk spec properties in deterministic (sorted) order.
    let mut prop_names: Vec<&String> = props.keys().collect();
    prop_names.sort();
    for prop_name in prop_names {
        let field_path = format!("{def_name}.{prop_name}");
        if colliding.contains(prop_name) {
            plan.skipped.push(SkippedField {
                field: field_path,
                reason: "normalize_collision".to_string(),
                detail: Value::Null,
            });
            continue;
        }
        let prop_schema = &props[prop_name];

        // Skip $ref properties (out of scope: nested refs handled by the
        // implicit pool path, not by sample-driven learn).
        if prop_schema.ref_path.is_some() {
            plan.skipped.push(SkippedField {
                field: field_path,
                reason: "nested_ref".to_string(),
                detail: Value::Null,
            });
            continue;
        }

        // Skip array properties — learn handles top-level scalars only.
        if prop_schema.schema_type.as_deref() == Some("array") {
            plan.skipped.push(SkippedField {
                field: field_path,
                reason: "array_property".to_string(),
                detail: Value::Null,
            });
            continue;
        }

        // Object subschemas without ref also out of scope.
        if prop_schema.schema_type.as_deref() == Some("object")
            || prop_schema.properties.is_some()
        {
            plan.skipped.push(SkippedField {
                field: field_path,
                reason: "object_property".to_string(),
                detail: Value::Null,
            });
            continue;
        }

        let norm = normalize_field_name(prop_name);
        let bucket = match field_samples.get(&norm) {
            Some(v) => v.as_slice(),
            None => {
                // Spec field absent from samples → leave untouched (per spec).
                continue;
            }
        };

        match decide_field(&field_path, prop_schema, bucket, config) {
            Decision::Apply(action) => plan.proposed.push(ProposedRule {
                field: field_path,
                action,
            }),
            Decision::Skip(skip) => plan.skipped.push(skip),
        }
    }

    plan
}

enum Decision {
    Apply(ProposedAction),
    Skip(SkippedField),
}

fn decide_field(
    field_path: &str,
    prop_schema: &SchemaObject,
    samples: &[Value],
    config: &LearnConfig,
) -> Decision {
    // Filter non-null samples.
    let non_null: Vec<&Value> = samples.iter().filter(|v| !v.is_null()).collect();
    if non_null.len() < config.min_samples {
        return Decision::Skip(SkippedField {
            field: field_path.to_string(),
            reason: "low_samples".to_string(),
            detail: serde_json::json!({
                "non_null_count": non_null.len(),
                "min_samples": config.min_samples,
            }),
        });
    }

    // Type homogeneity check. Allow ints + floats together (numeric).
    let mut all_numeric = true;
    let mut all_string = true;
    let mut all_bool = true;
    for v in &non_null {
        if !v.is_number() {
            all_numeric = false;
        }
        if !v.is_string() {
            all_string = false;
        }
        if !v.is_boolean() {
            all_bool = false;
        }
    }

    if !all_numeric && !all_string && !all_bool {
        return Decision::Skip(SkippedField {
            field: field_path.to_string(),
            reason: "mixed_types".to_string(),
            detail: Value::Null,
        });
    }

    // Distinct-value count. Use string-form keys for a stable hash.
    let mut distinct: BTreeSet<String> = BTreeSet::new();
    for v in &non_null {
        distinct.insert(value_key(v));
    }
    let distinct_count = distinct.len();

    // 1. Const branch.
    if distinct_count == 1 {
        let v = non_null[0].clone();
        return Decision::Apply(ProposedAction::SetRule {
            rule: Rule::Const {
                field: field_path.to_string(),
                value: v,
            },
        });
    }

    // 2. Format branch — only when all values are strings.
    if all_string {
        let strs: Vec<&str> = non_null.iter().filter_map(|v| v.as_str()).collect();
        // UUID → "uuid"
        if strs.iter().all(|s| is_uuid(s)) {
            return Decision::Apply(ProposedAction::SetFaker {
                strategy: "uuid".to_string(),
            });
        }
        // Email → "email"
        if strs.iter().all(|s| is_email(s)) {
            return Decision::Apply(ProposedAction::SetFaker {
                strategy: "email".to_string(),
            });
        }
        // IPv4 → "ipv4"
        if strs.iter().all(|s| is_ipv4(s)) {
            return Decision::Apply(ProposedAction::SetFaker {
                strategy: "ipv4".to_string(),
            });
        }
        // Date → "date"
        if strs.iter().all(|s| is_date(s)) {
            return Decision::Apply(ProposedAction::SetFaker {
                strategy: "date".to_string(),
            });
        }
    }

    // 3. Choice branch.
    if distinct_count <= config.max_choice {
        let mut options: Vec<Value> = non_null
            .iter()
            .map(|v| (*v).clone())
            .collect::<Vec<_>>()
            .into_iter()
            .collect();
        // Dedup while preserving sort by key.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        options.retain(|v| seen.insert(value_key(v)));
        // Stable order for determinism.
        options.sort_by_key(value_key);
        return Decision::Apply(ProposedAction::SetRule {
            rule: Rule::Choice {
                field: field_path.to_string(),
                options,
            },
        });
    }

    // 4. Custom-list branch (string only).
    if all_string && distinct_count <= config.max_list {
        let mut values: Vec<String> = non_null
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        let mut seen: HashSet<String> = HashSet::new();
        values.retain(|s| seen.insert(s.clone()));
        values.sort();
        let (def, prop) = match split_field_path(field_path) {
            Some(t) => t,
            None => {
                return Decision::Skip(SkippedField {
                    field: field_path.to_string(),
                    reason: "bad_field_path".to_string(),
                    detail: Value::Null,
                });
            }
        };
        let list_name = list_name_for(def, prop);
        return Decision::Apply(ProposedAction::SetCustomList { list_name, values });
    }

    // 5. Range branch (numeric, > max_choice distinct).
    if all_numeric {
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for v in &non_null {
            if let Some(n) = v.as_f64() {
                if n < min {
                    min = n;
                }
                if n > max {
                    max = n;
                }
            }
        }
        // Preserve integer-ness when every sample is an integer.
        let all_int = non_null.iter().all(|v| v.is_i64() || v.is_u64());
        let (rmin, rmax) = if all_int {
            (min.round(), max.round())
        } else {
            (min, max)
        };
        let _ = prop_schema; // schema_type informs nothing extra here
        return Decision::Apply(ProposedAction::SetRule {
            rule: Rule::Range {
                field: field_path.to_string(),
                min: rmin,
                max: rmax,
            },
        });
    }

    // 6. Too-distinct string fallback.
    Decision::Skip(SkippedField {
        field: field_path.to_string(),
        reason: "too_distinct".to_string(),
        detail: serde_json::json!({
            "distinct": distinct_count,
            "max_list": config.max_list,
        }),
    })
}

fn value_key(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Apply plan vs current config
// ---------------------------------------------------------------------------

/// Apply a `LearnPlan` to a `CurrentConfig` under the requested policy.
/// Returns the merged `NewConfig` plus an `ApplyReport` of what was applied
/// vs. skipped.
///
/// On `ConflictPolicy::Fail`, the FIRST collision short-circuits and returns
/// `Err(ConflictError)` — caller is expected to surface the error and write
/// nothing.
pub fn apply_plan(
    plan: &LearnPlan,
    existing: &CurrentConfig,
    policy: ConflictPolicy,
) -> Result<(NewConfig, ApplyReport), ConflictError> {
    let mut new_cfg = NewConfig {
        faker_rules: existing.faker_rules.clone(),
        rules: existing.rules.clone(),
        custom_lists: existing.custom_lists.clone(),
        quantity_configs: existing.quantity_configs.clone(),
        frozen_rows: existing.frozen_rows.clone(),
    };
    let mut report = ApplyReport {
        skipped: plan.skipped.clone(),
        warnings: plan.warnings.clone(),
        collisions: plan.collisions.clone(),
        ..Default::default()
    };

    for proposed in &plan.proposed {
        let field = &proposed.field;
        match &proposed.action {
            ProposedAction::SetFaker { strategy } => {
                let slot = format!("faker_rules[{field}]");
                if let Some(existing_val) = existing.faker_rules.get(field) {
                    match policy {
                        ConflictPolicy::Merge => {
                            report.skipped.push(SkippedField {
                                field: field.clone(),
                                reason: "conflict_existing_faker_rule".to_string(),
                                detail: serde_json::json!({
                                    "existing": existing_val,
                                    "proposed": strategy,
                                }),
                            });
                            continue;
                        }
                        ConflictPolicy::Fail => {
                            return Err(ConflictError {
                                field: field.clone(),
                                slot,
                                message: format!(
                                    "faker_rules['{field}'] already set to '{existing_val}'"
                                ),
                            });
                        }
                        ConflictPolicy::Overwrite => {}
                    }
                }
                new_cfg.faker_rules.insert(field.clone(), strategy.clone());
                report.applied.push(AppliedEntry {
                    field: field.clone(),
                    kind: "faker".to_string(),
                    detail: serde_json::json!({ "strategy": strategy }),
                });
            }
            ProposedAction::SetCustomList { list_name, values } => {
                // Two slots: faker_rules[field] AND custom_lists[list_name].
                let faker_taken = existing.faker_rules.contains_key(field);
                let list_taken = existing.custom_lists.contains_key(list_name);
                if faker_taken || list_taken {
                    match policy {
                        ConflictPolicy::Merge => {
                            report.skipped.push(SkippedField {
                                field: field.clone(),
                                reason: "conflict_existing_custom_list".to_string(),
                                detail: serde_json::json!({
                                    "list_name": list_name,
                                    "faker_taken": faker_taken,
                                    "list_taken": list_taken,
                                }),
                            });
                            continue;
                        }
                        ConflictPolicy::Fail => {
                            return Err(ConflictError {
                                field: field.clone(),
                                slot: if list_taken {
                                    format!("custom_lists[{list_name}]")
                                } else {
                                    format!("faker_rules[{field}]")
                                },
                                message: format!(
                                    "slot occupied: faker_taken={faker_taken} list_taken={list_taken}"
                                ),
                            });
                        }
                        ConflictPolicy::Overwrite => {}
                    }
                }
                new_cfg
                    .custom_lists
                    .insert(list_name.clone(), values.clone());
                new_cfg
                    .faker_rules
                    .insert(field.clone(), list_name.clone());
                report.applied.push(AppliedEntry {
                    field: field.clone(),
                    kind: "custom_list".to_string(),
                    detail: serde_json::json!({
                        "list_name": list_name,
                        "values": values,
                    }),
                });
            }
            ProposedAction::SetRule { rule } => {
                let occupant = field_level_rule_index(&existing.rules, field);
                if let Some(idx) = occupant {
                    match policy {
                        ConflictPolicy::Merge => {
                            report.skipped.push(SkippedField {
                                field: field.clone(),
                                reason: "conflict_existing_rule".to_string(),
                                detail: rule_kind_value(&existing.rules[idx]),
                            });
                            continue;
                        }
                        ConflictPolicy::Fail => {
                            return Err(ConflictError {
                                field: field.clone(),
                                slot: format!("rules[{field}]"),
                                message: format!("field-level rule already set on '{field}'"),
                            });
                        }
                        ConflictPolicy::Overwrite => {
                            // Replace at index.
                            new_cfg.rules[idx] = rule.clone();
                            report.applied.push(applied_for_rule(field, rule));
                            continue;
                        }
                    }
                }
                new_cfg.rules.push(rule.clone());
                report.applied.push(applied_for_rule(field, rule));
            }
        }
    }

    Ok((new_cfg, report))
}

fn field_level_rule_index(rules: &[Rule], field_path: &str) -> Option<usize> {
    let (def, prop) = split_field_path(field_path)?;
    rules.iter().position(|r| match r.target_field() {
        Some((d, p)) => d == def && p == prop,
        None => false,
    })
}

fn applied_for_rule(field: &str, rule: &Rule) -> AppliedEntry {
    let (kind, detail) = match rule {
        Rule::Const { value, .. } => ("const", serde_json::json!({ "value": value })),
        Rule::Choice { options, .. } => ("choice", serde_json::json!({ "options": options })),
        Rule::Range { min, max, .. } => ("range", serde_json::json!({ "min": min, "max": max })),
        Rule::Pattern { regex, .. } => ("pattern", serde_json::json!({ "regex": regex })),
        Rule::Compare { .. } => ("compare", serde_json::json!({})),
    };
    AppliedEntry {
        field: field.to_string(),
        kind: kind.to_string(),
        detail,
    }
}

fn rule_kind_value(rule: &Rule) -> Value {
    let kind = match rule {
        Rule::Const { .. } => "const",
        Rule::Choice { .. } => "choice",
        Rule::Range { .. } => "range",
        Rule::Pattern { .. } => "pattern",
        Rule::Compare { .. } => "compare",
    };
    serde_json::json!({ "existing_kind": kind })
}

// ---------------------------------------------------------------------------
// Report serialization helpers
// ---------------------------------------------------------------------------

/// Render an `ApplyReport` to a JSON object. Adds `id` and `wrote` fields.
pub fn report_json(report: &ApplyReport, id: i64, wrote: bool) -> Value {
    let applied: Vec<Value> = report
        .applied
        .iter()
        .map(|a| {
            let mut obj = serde_json::Map::new();
            obj.insert("field".to_string(), Value::String(a.field.clone()));
            obj.insert("kind".to_string(), Value::String(a.kind.clone()));
            if let Value::Object(m) = &a.detail {
                for (k, v) in m {
                    obj.insert(k.clone(), v.clone());
                }
            }
            Value::Object(obj)
        })
        .collect();
    let skipped: Vec<Value> = report
        .skipped
        .iter()
        .map(|s| {
            let mut obj = serde_json::Map::new();
            obj.insert("field".to_string(), Value::String(s.field.clone()));
            obj.insert("reason".to_string(), Value::String(s.reason.clone()));
            if let Value::Object(m) = &s.detail {
                for (k, v) in m {
                    obj.insert(k.clone(), v.clone());
                }
            }
            Value::Object(obj)
        })
        .collect();
    serde_json::json!({
        "id": id,
        "applied": applied,
        "skipped": skipped,
        "warnings": report.warnings,
        "collisions": report.collisions,
        "wrote": wrote,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse_yaml(yaml: &str) -> SwaggerSpec {
        serde_yaml::from_str(yaml).unwrap()
    }

    // ---- normalize_field_name ----

    #[test]
    fn normalize_camel_snake_kebab_match() {
        assert_eq!(normalize_field_name("userEmail"), "useremail");
        assert_eq!(normalize_field_name("user_email"), "useremail");
        assert_eq!(normalize_field_name("user-email"), "useremail");
        assert_eq!(normalize_field_name("USER_EMAIL"), "useremail");
        assert_eq!(normalize_field_name("UserEmail"), "useremail");
    }

    // ---- format detectors ----

    #[test]
    fn uuid_positive() {
        assert!(is_uuid("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_uuid("550E8400-E29B-41D4-A716-446655440000"));
    }

    #[test]
    fn uuid_negative() {
        assert!(!is_uuid("not-a-uuid"));
        assert!(!is_uuid("550e8400-e29b-41d4-a716"));
        assert!(!is_uuid("550e8400e29b41d4a716446655440000"));
        assert!(!is_uuid(""));
    }

    #[test]
    fn email_positive() {
        assert!(is_email("a@b.co"));
        assert!(is_email("alice@example.com"));
        assert!(is_email("a.b+c@x.io"));
    }

    #[test]
    fn email_negative() {
        assert!(!is_email("no-at-sign.com"));
        assert!(!is_email("a@b"));
        assert!(!is_email("a @b.co"));
        assert!(!is_email("a@@b.co"));
        assert!(!is_email(""));
    }

    #[test]
    fn ipv4_positive() {
        assert!(is_ipv4("0.0.0.0"));
        assert!(is_ipv4("127.0.0.1"));
        assert!(is_ipv4("255.255.255.255"));
    }

    #[test]
    fn ipv4_negative() {
        assert!(!is_ipv4("256.0.0.0"));
        assert!(!is_ipv4("1.2.3"));
        assert!(!is_ipv4("1.2.3.4.5"));
        assert!(!is_ipv4("a.b.c.d"));
        assert!(!is_ipv4(""));
    }

    #[test]
    fn date_positive() {
        assert!(is_date("2025-01-02"));
        assert!(is_date("2025-01-02T03:04:05Z"));
        assert!(is_date("2025-01-02T03:04:05+00:00"));
        assert!(is_date("2025-01-02T03:04:05"));
    }

    #[test]
    fn date_negative() {
        assert!(!is_date(""));
        assert!(!is_date("not a date"));
        assert!(!is_date("2025/01/02"));
    }

    // ---- read_samples ----

    #[test]
    fn read_samples_jsonl() {
        let data = b"{\"a\":1}\n{\"a\":2}\n\n{\"a\":3}\n";
        let v = read_samples(Cursor::new(data)).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0]["a"], 1);
        assert_eq!(v[2]["a"], 3);
    }

    #[test]
    fn read_samples_array() {
        let data = b"[{\"a\":1},{\"a\":2}]";
        let v = read_samples(Cursor::new(data)).unwrap();
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn read_samples_array_with_leading_ws() {
        let data = b"   \n  [{\"a\":1}]";
        let v = read_samples(Cursor::new(data)).unwrap();
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn read_samples_empty() {
        let v = read_samples(Cursor::new(b"")).unwrap();
        assert!(v.is_empty());
        let v = read_samples(Cursor::new(b"   \n  ")).unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn read_samples_malformed_array() {
        let err = read_samples(Cursor::new(b"[not json")).unwrap_err();
        assert!(err.contains("invalid"));
    }

    #[test]
    fn read_samples_malformed_jsonl() {
        let err = read_samples(Cursor::new(b"{\"a\":1}\nnot json\n")).unwrap_err();
        assert!(err.contains("line 2"));
    }

    // ---- plan_learn ----

    fn pet_spec() -> SwaggerSpec {
        parse_yaml(
            r#"
swagger: "2.0"
info: { title: t, version: "1" }
paths: {}
definitions:
  Pet:
    type: object
    properties:
      id: { type: integer }
      name: { type: string }
      status: { type: string }
      breed: { type: string }
      tagId: { type: string }
      email: { type: string }
      kind:  { type: string }
"#,
        )
    }

    #[test]
    fn plan_const_branch() {
        let spec = pet_spec();
        let samples: Vec<Value> = (0..6)
            .map(|_| serde_json::json!({ "kind": "dog" }))
            .collect();
        let plan = plan_learn(&spec, "Pet", &samples, &LearnConfig::default());
        let p = plan
            .proposed
            .iter()
            .find(|p| p.field == "Pet.kind")
            .expect("Pet.kind proposed");
        match &p.action {
            ProposedAction::SetRule {
                rule: Rule::Const { value, .. },
            } => assert_eq!(value, &serde_json::json!("dog")),
            other => panic!("expected Const, got {other:?}"),
        }
    }

    #[test]
    fn plan_choice_branch() {
        let spec = pet_spec();
        let mut samples: Vec<Value> = Vec::new();
        for s in ["available", "pending", "sold"].iter().cycle().take(15) {
            samples.push(serde_json::json!({ "status": s }));
        }
        let plan = plan_learn(&spec, "Pet", &samples, &LearnConfig::default());
        let p = plan
            .proposed
            .iter()
            .find(|p| p.field == "Pet.status")
            .expect("Pet.status proposed");
        match &p.action {
            ProposedAction::SetRule {
                rule: Rule::Choice { options, .. },
            } => assert_eq!(options.len(), 3),
            other => panic!("expected Choice, got {other:?}"),
        }
    }

    #[test]
    fn plan_format_uuid() {
        let spec = pet_spec();
        let uuids = [
            "550e8400-e29b-41d4-a716-446655440000",
            "550e8400-e29b-41d4-a716-446655440001",
            "550e8400-e29b-41d4-a716-446655440002",
            "550e8400-e29b-41d4-a716-446655440003",
            "550e8400-e29b-41d4-a716-446655440004",
            "550e8400-e29b-41d4-a716-446655440005",
        ];
        let samples: Vec<Value> = uuids
            .iter()
            .map(|u| serde_json::json!({ "tagId": u }))
            .collect();
        let plan = plan_learn(&spec, "Pet", &samples, &LearnConfig::default());
        let p = plan
            .proposed
            .iter()
            .find(|p| p.field == "Pet.tagId")
            .expect("Pet.tagId proposed");
        match &p.action {
            ProposedAction::SetFaker { strategy } => assert_eq!(strategy, "uuid"),
            other => panic!("expected uuid faker, got {other:?}"),
        }
    }

    #[test]
    fn plan_custom_list_branch() {
        let spec = pet_spec();
        // 25 distinct breeds → above default max_choice (20), below max_list (200).
        let breeds: Vec<String> = (0..25).map(|i| format!("breed_{i}")).collect();
        let mut samples: Vec<Value> = Vec::new();
        for b in &breeds {
            samples.push(serde_json::json!({ "breed": b }));
            samples.push(serde_json::json!({ "breed": b }));
        }
        let plan = plan_learn(&spec, "Pet", &samples, &LearnConfig::default());
        let p = plan
            .proposed
            .iter()
            .find(|p| p.field == "Pet.breed")
            .expect("Pet.breed proposed");
        match &p.action {
            ProposedAction::SetCustomList { list_name, values } => {
                assert_eq!(list_name, "learned_pet_breed");
                assert_eq!(values.len(), 25);
            }
            other => panic!("expected custom_list, got {other:?}"),
        }
    }

    #[test]
    fn plan_range_branch() {
        let spec = pet_spec();
        let samples: Vec<Value> = (1..=30)
            .map(|i| serde_json::json!({ "id": i }))
            .collect();
        let plan = plan_learn(&spec, "Pet", &samples, &LearnConfig::default());
        let p = plan
            .proposed
            .iter()
            .find(|p| p.field == "Pet.id")
            .expect("Pet.id proposed");
        match &p.action {
            ProposedAction::SetRule {
                rule: Rule::Range { min, max, .. },
            } => {
                assert_eq!(*min, 1.0);
                assert_eq!(*max, 30.0);
            }
            other => panic!("expected Range, got {other:?}"),
        }
    }

    #[test]
    fn plan_low_samples_skip() {
        let spec = pet_spec();
        // Only 3 samples for `name` (default min_samples = 5).
        let samples = vec![
            serde_json::json!({ "name": "x" }),
            serde_json::json!({ "name": "y" }),
            serde_json::json!({ "name": "z" }),
        ];
        let plan = plan_learn(&spec, "Pet", &samples, &LearnConfig::default());
        assert!(plan.proposed.iter().all(|p| p.field != "Pet.name"));
        assert!(
            plan.skipped
                .iter()
                .any(|s| s.field == "Pet.name" && s.reason == "low_samples")
        );
    }

    #[test]
    fn plan_too_distinct_skip() {
        let spec = pet_spec();
        // 250 distinct strings → above default max_list (200).
        let mut samples: Vec<Value> = Vec::new();
        for i in 0..250 {
            samples.push(serde_json::json!({ "breed": format!("b_{i}") }));
        }
        let plan = plan_learn(&spec, "Pet", &samples, &LearnConfig::default());
        assert!(plan.proposed.iter().all(|p| p.field != "Pet.breed"));
        let s = plan
            .skipped
            .iter()
            .find(|s| s.field == "Pet.breed")
            .expect("breed skipped");
        assert_eq!(s.reason, "too_distinct");
    }

    #[test]
    fn plan_sample_key_not_in_spec() {
        let spec = pet_spec();
        let samples: Vec<Value> = (0..6)
            .map(|_| serde_json::json!({ "what_is_this": "??" }))
            .collect();
        let plan = plan_learn(&spec, "Pet", &samples, &LearnConfig::default());
        assert!(
            plan.skipped
                .iter()
                .any(|s| s.field == "what_is_this" && s.reason == "sample_key_not_in_spec")
        );
    }

    #[test]
    fn plan_normalize_collision_refuses() {
        // Two spec props that collide on normalize.
        let spec = parse_yaml(
            r#"
swagger: "2.0"
info: { title: t, version: "1" }
paths: {}
definitions:
  Pet:
    type: object
    properties:
      userEmail: { type: string }
      user_email: { type: string }
"#,
        );
        let samples: Vec<Value> = (0..6)
            .map(|i| serde_json::json!({ "userEmail": format!("a{i}@b.co") }))
            .collect();
        let plan = plan_learn(&spec, "Pet", &samples, &LearnConfig::default());
        assert!(!plan.collisions.is_empty(), "expected collision report");
        assert!(
            plan.skipped
                .iter()
                .any(|s| s.field == "Pet.userEmail" && s.reason == "normalize_collision")
        );
        assert!(
            plan.skipped
                .iter()
                .any(|s| s.field == "Pet.user_email" && s.reason == "normalize_collision")
        );
        assert!(plan.proposed.is_empty());
    }

    // ---- apply_plan ----

    fn one_field_plan(field: &str, action: ProposedAction) -> LearnPlan {
        LearnPlan {
            def_name: "Pet".to_string(),
            proposed: vec![ProposedRule {
                field: field.to_string(),
                action,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn apply_merge_keeps_existing_faker() {
        let plan = one_field_plan(
            "Pet.id",
            ProposedAction::SetFaker {
                strategy: "uuid".to_string(),
            },
        );
        let mut existing = CurrentConfig::default();
        existing
            .faker_rules
            .insert("Pet.id".to_string(), "integer".to_string());
        let (new_cfg, report) = apply_plan(&plan, &existing, ConflictPolicy::Merge).unwrap();
        assert_eq!(new_cfg.faker_rules.get("Pet.id"), Some(&"integer".to_string()));
        assert!(report.applied.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].reason, "conflict_existing_faker_rule");
    }

    #[test]
    fn apply_overwrite_replaces_faker() {
        let plan = one_field_plan(
            "Pet.id",
            ProposedAction::SetFaker {
                strategy: "uuid".to_string(),
            },
        );
        let mut existing = CurrentConfig::default();
        existing
            .faker_rules
            .insert("Pet.id".to_string(), "integer".to_string());
        let (new_cfg, _report) = apply_plan(&plan, &existing, ConflictPolicy::Overwrite).unwrap();
        assert_eq!(new_cfg.faker_rules.get("Pet.id"), Some(&"uuid".to_string()));
    }

    #[test]
    fn apply_fail_returns_err_on_conflict() {
        let plan = one_field_plan(
            "Pet.id",
            ProposedAction::SetFaker {
                strategy: "uuid".to_string(),
            },
        );
        let mut existing = CurrentConfig::default();
        existing
            .faker_rules
            .insert("Pet.id".to_string(), "integer".to_string());
        let err = apply_plan(&plan, &existing, ConflictPolicy::Fail).unwrap_err();
        assert_eq!(err.field, "Pet.id");
    }

    #[test]
    fn apply_merge_rule_collision_skipped() {
        let plan = one_field_plan(
            "Pet.status",
            ProposedAction::SetRule {
                rule: Rule::Choice {
                    field: "Pet.status".to_string(),
                    options: vec![serde_json::json!("a")],
                },
            },
        );
        let existing = CurrentConfig {
            rules: vec![Rule::Const {
                field: "Pet.status".to_string(),
                value: serde_json::json!("locked"),
            }],
            ..Default::default()
        };
        let (new_cfg, report) = apply_plan(&plan, &existing, ConflictPolicy::Merge).unwrap();
        // Existing const preserved.
        assert_eq!(new_cfg.rules.len(), 1);
        assert!(matches!(new_cfg.rules[0], Rule::Const { .. }));
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].reason, "conflict_existing_rule");
    }

    #[test]
    fn apply_overwrite_rule_replaces_in_place() {
        let plan = one_field_plan(
            "Pet.status",
            ProposedAction::SetRule {
                rule: Rule::Choice {
                    field: "Pet.status".to_string(),
                    options: vec![serde_json::json!("a")],
                },
            },
        );
        let existing = CurrentConfig {
            rules: vec![Rule::Const {
                field: "Pet.status".to_string(),
                value: serde_json::json!("locked"),
            }],
            ..Default::default()
        };
        let (new_cfg, report) =
            apply_plan(&plan, &existing, ConflictPolicy::Overwrite).unwrap();
        assert_eq!(new_cfg.rules.len(), 1);
        assert!(matches!(new_cfg.rules[0], Rule::Choice { .. }));
        assert_eq!(report.applied.len(), 1);
    }

    #[test]
    fn apply_custom_list_writes_both_slots() {
        let plan = one_field_plan(
            "Pet.breed",
            ProposedAction::SetCustomList {
                list_name: "learned_pet_breed".to_string(),
                values: vec!["a".to_string(), "b".to_string()],
            },
        );
        let existing = CurrentConfig::default();
        let (new_cfg, _) = apply_plan(&plan, &existing, ConflictPolicy::Merge).unwrap();
        assert_eq!(
            new_cfg.faker_rules.get("Pet.breed"),
            Some(&"learned_pet_breed".to_string())
        );
        assert_eq!(
            new_cfg.custom_lists.get("learned_pet_breed").map(Vec::len),
            Some(2)
        );
    }
}
