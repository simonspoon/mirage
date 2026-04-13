// Recipe rule data model: bounded constraints + cross-field compares.
//
// A `Rule` is a tagged union of five kinds:
//   - Range   { field, min, max }       -- numeric bounds [min, max] (inclusive)
//   - Choice  { field, options }        -- pick one value from a list
//   - Const   { field, value }          -- always emit this exact value
//   - Pattern { field, regex }          -- generate a string matching the regex
//   - Compare { left, op, right }       -- cross-field predicate (numeric or string)
//
// Field-level rules (Range / Choice / Const / Pattern) override per-field
// generation BEFORE the existing x-faker / format / heuristic / type layers.
// Compare rules run AFTER row generation, in a row-repair pass, against
// already-generated values.
//
// Rules are scoped to a definition (table/document type). The `field` is a
// dotted path of the form "DefName.propName" (matching the existing
// faker_rules convention used by composer::parse_faker_rules). For Compare,
// both `left` and `right` may be field paths; `right` may also be a literal
// value (number, string, or boolean).
//
// Validation happens at create_recipe / update_recipe time and rejects:
//   - Two field-level rules on the same field
//   - Compare rules that form a dependency cycle
//   - Compare rules with a self-loop (left == right field)
//   - Pattern rules with an invalid or unparseable regex
//   - Rules that target a field which does not exist in the spec
//   - Rules that target a field of an incompatible type (e.g. Range on a
//     string field, or Compare gt on a boolean)

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use rand::RngExt;
use serde::{Deserialize, Serialize};

use crate::parser::{SchemaObject, SwaggerSpec};

/// Compare operators. Apply to numeric AND string fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
}

/// A single rule. Tagged union via `kind` discriminator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Rule {
    Range {
        field: String,
        min: f64,
        max: f64,
    },
    Choice {
        field: String,
        options: Vec<serde_json::Value>,
    },
    Const {
        field: String,
        value: serde_json::Value,
    },
    Pattern {
        field: String,
        regex: String,
    },
    Compare {
        left: String,
        op: CompareOp,
        /// Either a field path (string matching a known field) or a literal
        /// value. Resolved at apply time.
        right: serde_json::Value,
    },
}

impl Rule {
    /// For field-level rules, the (def_name, prop_name) tuple this rule targets.
    /// Returns None for Compare rules.
    pub fn target_field(&self) -> Option<(&str, &str)> {
        let path = match self {
            Rule::Range { field, .. }
            | Rule::Choice { field, .. }
            | Rule::Const { field, .. }
            | Rule::Pattern { field, .. } => field,
            Rule::Compare { .. } => return None,
        };
        split_field_path(path)
    }

    pub fn is_field_level(&self) -> bool {
        !matches!(self, Rule::Compare { .. })
    }

    pub fn is_compare(&self) -> bool {
        matches!(self, Rule::Compare { .. })
    }
}

/// Split a "DefName.propName" path. Returns None if the path is not in the
/// expected form.
pub fn split_field_path(path: &str) -> Option<(&str, &str)> {
    let dot = path.find('.')?;
    let def = &path[..dot];
    let prop = &path[dot + 1..];
    if def.is_empty() || prop.is_empty() {
        None
    } else {
        Some((def, prop))
    }
}

/// Lookup keyed by (def_name, prop_name) -> rule. Used by the seeder/composer
/// to find a field-level rule for a given column.
pub type FieldRuleMap = HashMap<(String, String), Rule>;

/// Build a FieldRuleMap from a slice of rules. Compare rules are ignored.
pub fn build_field_rule_map(rules: &[Rule]) -> FieldRuleMap {
    let mut map = FieldRuleMap::new();
    for rule in rules {
        if let Some((def, prop)) = rule.target_field() {
            map.insert((def.to_string(), prop.to_string()), rule.clone());
        }
    }
    map
}

/// Compare rules grouped by definition name. The seeder/composer applies these
/// per-row after generation.
pub type CompareRulesByDef = HashMap<String, Vec<Rule>>;

/// Group compare rules by the definition name of their LEFT operand. Returns
/// only well-formed compare rules whose left side is a "DefName.propName"
/// path.
pub fn build_compare_rules_by_def(rules: &[Rule]) -> CompareRulesByDef {
    let mut map: CompareRulesByDef = HashMap::new();
    for rule in rules {
        if let Rule::Compare { left, .. } = rule
            && let Some((def, _)) = split_field_path(left)
        {
            map.entry(def.to_string()).or_default().push(rule.clone());
        }
    }
    map
}

/// Parse a JSON string into a Vec<Rule>. Returns Ok(empty) on null/empty
/// input. Returns Err on invalid JSON or invalid rule shape.
pub fn parse_rules(json_str: &str) -> Result<Vec<Rule>, String> {
    let trimmed = json_str.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(Vec::new());
    }
    let parsed: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| format!("Invalid rules JSON: {e}"))?;
    if parsed.is_null() {
        return Ok(Vec::new());
    }
    let arr = parsed
        .as_array()
        .ok_or_else(|| "rules must be a JSON array".to_string())?;
    if arr.is_empty() {
        return Ok(Vec::new());
    }
    let rules: Vec<Rule> = serde_json::from_value(serde_json::Value::Array(arr.clone()))
        .map_err(|e| format!("Invalid rule shape: {e}"))?;
    Ok(rules)
}

/// Validate a slice of rules. If `spec` is provided, also check that all
/// referenced fields exist and have compatible types. Returns Ok(()) on
/// success or a human-readable error.
pub fn validate_rules(rules: &[Rule], spec: Option<&SwaggerSpec>) -> Result<(), String> {
    // 1. Pattern rules: parse the regex.
    for rule in rules {
        if let Rule::Pattern { field, regex } = rule {
            regex_syntax::parse(regex)
                .map_err(|e| format!("Invalid regex for field {field}: {e}"))?;
        }
    }

    // 2. Conflict: two field-level rules targeting the same (def, field).
    let mut seen_field_targets: HashSet<(String, String)> = HashSet::new();
    for rule in rules {
        if let Some((def, prop)) = rule.target_field() {
            let key = (def.to_string(), prop.to_string());
            if !seen_field_targets.insert(key.clone()) {
                return Err(format!(
                    "Conflicting field-level rules: multiple rules target {}.{}",
                    key.0, key.1
                ));
            }
        }
    }

    // 3. Compare rules: validate left is a field path, no self-loop.
    for rule in rules {
        if let Rule::Compare { left, right, .. } = rule {
            let (left_def, _left_prop) = split_field_path(left).ok_or_else(|| {
                format!("Compare rule left side must be a 'DefName.field' path, got: {left}")
            })?;
            // If right is a string, it MAY be a field path — in that case
            // require it to be in the same def AND not equal to left.
            if let Some(right_str) = right.as_str()
                && let Some((right_def, _right_prop)) = split_field_path(right_str)
            {
                if right_str == left {
                    return Err(format!("Compare rule self-loop: {left} compared to itself"));
                }
                if right_def != left_def {
                    return Err(format!(
                        "Compare rule cross-definition not supported: {left_def} vs {right_def}"
                    ));
                }
            }
        }
    }

    // 4. Cycle detection on compare rule graph.
    detect_compare_cycles(rules)?;

    // 5. Spec-aware validation: field existence and type compatibility.
    if let Some(spec) = spec {
        validate_against_spec(rules, spec)?;
    }

    Ok(())
}

/// Detect cycles in the dependency graph induced by Compare rules.
/// An edge goes from `left` -> `right` whenever right is a field path.
fn detect_compare_cycles(rules: &[Rule]) -> Result<(), String> {
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();

    for rule in rules {
        if let Rule::Compare { left, right, .. } = rule {
            graph.entry(left.clone()).or_default();
            if let Some(right_str) = right.as_str()
                && split_field_path(right_str).is_some()
            {
                graph
                    .entry(left.clone())
                    .or_default()
                    .push(right_str.to_string());
                graph.entry(right_str.to_string()).or_default();
            }
        }
    }

    // DFS with three-color marking.
    enum Color {
        White,
        Gray,
        Black,
    }
    let mut color: HashMap<String, Color> =
        graph.keys().map(|k| (k.clone(), Color::White)).collect();

    for start in graph.keys() {
        if matches!(color.get(start), Some(Color::White)) {
            let mut stack: Vec<(String, usize)> = vec![(start.clone(), 0)];
            color.insert(start.clone(), Color::Gray);
            while let Some((node, child_idx)) = stack.last().cloned() {
                let children = graph.get(&node).cloned().unwrap_or_default();
                if child_idx >= children.len() {
                    color.insert(node.clone(), Color::Black);
                    stack.pop();
                    continue;
                }
                if let Some(last) = stack.last_mut() {
                    last.1 += 1;
                }
                let child = &children[child_idx];
                match color.get(child) {
                    Some(Color::Gray) => {
                        return Err(format!("Compare rule cycle detected involving {child}"));
                    }
                    Some(Color::White) => {
                        color.insert(child.clone(), Color::Gray);
                        stack.push((child.clone(), 0));
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

/// Walk each rule's referenced fields and check they exist in the spec's
/// definitions, and that their types are compatible with the rule kind.
fn validate_against_spec(rules: &[Rule], spec: &SwaggerSpec) -> Result<(), String> {
    let defs = match spec.definitions.as_ref() {
        Some(d) => d,
        // No definitions means we cannot validate; treat as empty (rules will
        // either match nothing or be caught at apply time). Be lenient.
        None => return Ok(()),
    };

    let lookup_field = |path: &str| -> Result<&SchemaObject, String> {
        let (def, prop) = split_field_path(path)
            .ok_or_else(|| format!("Field path must be DefName.field: {path}"))?;
        let schema = defs
            .get(def)
            .ok_or_else(|| format!("Unknown definition '{def}' in rule"))?;
        // Walk into the schema for the property. Support array-typed defs that
        // wrap items (mirror seeder::effective_props).
        let props = if let Some(p) = schema.properties.as_ref() {
            p
        } else if schema.schema_type.as_deref() == Some("array")
            && let Some(items) = schema.items.as_ref()
            && let Some(p) = items.properties.as_ref()
        {
            p
        } else {
            return Err(format!("Definition '{def}' has no properties"));
        };
        props
            .get(prop)
            .ok_or_else(|| format!("Unknown field '{prop}' on definition '{def}'"))
    };

    let is_numeric = |schema: &SchemaObject| -> bool {
        matches!(
            schema.schema_type.as_deref(),
            Some("integer") | Some("number")
        )
    };
    let is_string = |schema: &SchemaObject| -> bool {
        matches!(schema.schema_type.as_deref(), Some("string") | None)
    };

    for rule in rules {
        match rule {
            Rule::Range { field, min, max } => {
                let schema = lookup_field(field)?;
                if !is_numeric(schema) {
                    return Err(format!(
                        "Range rule on non-numeric field '{field}' (type {:?})",
                        schema.schema_type
                    ));
                }
                if min > max {
                    return Err(format!(
                        "Range rule has min > max for field '{field}': {min} > {max}"
                    ));
                }
            }
            Rule::Choice { field, options } => {
                lookup_field(field)?;
                if options.is_empty() {
                    return Err(format!(
                        "Choice rule for field '{field}' has empty options list"
                    ));
                }
            }
            Rule::Const { field, .. } => {
                lookup_field(field)?;
            }
            Rule::Pattern { field, regex: _ } => {
                let schema = lookup_field(field)?;
                if !is_string(schema) {
                    return Err(format!(
                        "Pattern rule on non-string field '{field}' (type {:?})",
                        schema.schema_type
                    ));
                }
            }
            Rule::Compare { left, op: _, right } => {
                let left_schema = lookup_field(left)?;
                if let Some(right_str) = right.as_str()
                    && split_field_path(right_str).is_some()
                {
                    let right_schema = lookup_field(right_str)?;
                    // Both sides must be the same broad type family.
                    let left_num = is_numeric(left_schema);
                    let right_num = is_numeric(right_schema);
                    let left_str = is_string(left_schema);
                    let right_str_t = is_string(right_schema);
                    if !((left_num && right_num) || (left_str && right_str_t)) {
                        return Err(format!(
                            "Compare rule type mismatch: {left} ({:?}) vs {right_str} ({:?})",
                            left_schema.schema_type, right_schema.schema_type
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}

// -------------------------------------------------------------------------
// Field-level resolution (used by seeder + composer)
// -------------------------------------------------------------------------

/// If a field-level rule applies, generate a value for it. Returns None if
/// the rule kind is Compare (handled in repair pass) or unsupported.
pub fn generate_for_field_rule(rule: &Rule) -> Option<serde_json::Value> {
    let mut rng = rand::rng();
    match rule {
        Rule::Range { min, max, .. } => {
            let lo = min.min(*max);
            let hi = min.max(*max);
            // Decide integer vs float: if both are whole numbers and lo == hi,
            // emit an integer; if both are whole, randomly pick within the
            // integer range; otherwise float.
            if lo.fract() == 0.0 && hi.fract() == 0.0 {
                let lo_i = lo as i64;
                let hi_i = hi as i64;
                let n = if lo_i == hi_i {
                    lo_i
                } else {
                    rng.random_range(lo_i..=hi_i)
                };
                Some(serde_json::Value::Number(serde_json::Number::from(n)))
            } else {
                let n: f64 = rng.random_range(lo..=hi);
                Some(serde_json::json!(n))
            }
        }
        Rule::Choice { options, .. } => {
            if options.is_empty() {
                None
            } else {
                let idx = rng.random_range(0..options.len());
                Some(options[idx].clone())
            }
        }
        Rule::Const { value, .. } => Some(value.clone()),
        Rule::Pattern { regex, .. } => generate_for_pattern(regex).ok(),
        Rule::Compare { .. } => None,
    }
}

/// Generate a string matching the given regex via rand_regex. Returns Err
/// with a human-readable message if the regex is invalid.
pub fn generate_for_pattern(regex: &str) -> Result<serde_json::Value, String> {
    let hir = regex_syntax::parse(regex).map_err(|e| format!("Invalid regex '{regex}': {e}"))?;
    let generator = rand_regex::Regex::with_hir(hir, 100)
        .map_err(|e| format!("rand_regex failed for '{regex}': {e}"))?;
    let mut rng = rand::rng();
    let s: String = rng.sample(&generator);
    Ok(serde_json::Value::String(s))
}

// -------------------------------------------------------------------------
// Compare repair pass
// -------------------------------------------------------------------------

/// Apply compare rules to a generated row in place. For each rule, if the
/// predicate is not satisfied, repair the LEFT field by setting it to a value
/// that satisfies the predicate (preserving type).
///
/// Best-effort: rules are processed in declaration order. If a rule cannot be
/// repaired (e.g. left field missing), it is skipped.
pub fn apply_compare_rules(row: &mut serde_json::Map<String, serde_json::Value>, rules: &[Rule]) {
    for rule in rules {
        let Rule::Compare { left, op, right } = rule else {
            continue;
        };
        let Some((_def, left_prop)) = split_field_path(left) else {
            continue;
        };

        // Resolve the right-hand value: either a literal or a sibling field.
        let right_val: serde_json::Value = if let Some(right_str) = right.as_str() {
            if let Some((_rdef, right_prop)) = split_field_path(right_str) {
                match row.get(right_prop) {
                    Some(v) => v.clone(),
                    None => continue,
                }
            } else {
                serde_json::Value::String(right_str.to_string())
            }
        } else {
            right.clone()
        };

        let Some(left_val) = row.get(left_prop).cloned() else {
            continue;
        };

        if compare_holds(&left_val, *op, &right_val) {
            continue;
        }

        // Repair: produce a left value that satisfies the predicate.
        if let Some(repaired) = repair_left(&left_val, *op, &right_val) {
            row.insert(left_prop.to_string(), repaired);
        }
    }
}

/// Evaluate a compare predicate. Returns false if values are not comparable.
pub fn compare_holds(left: &serde_json::Value, op: CompareOp, right: &serde_json::Value) -> bool {
    // Numeric comparison if both are numbers.
    if let (Some(l), Some(r)) = (left.as_f64(), right.as_f64()) {
        return apply_op_num(l, op, r);
    }
    // String comparison if both are strings.
    if let (Some(l), Some(r)) = (left.as_str(), right.as_str()) {
        return apply_op_str(l, op, r);
    }
    // Boolean equality only.
    if let (Some(l), Some(r)) = (left.as_bool(), right.as_bool()) {
        return match op {
            CompareOp::Eq => l == r,
            CompareOp::Neq => l != r,
            _ => false,
        };
    }
    false
}

fn apply_op_num(l: f64, op: CompareOp, r: f64) -> bool {
    match op {
        CompareOp::Eq => (l - r).abs() < f64::EPSILON,
        CompareOp::Neq => (l - r).abs() >= f64::EPSILON,
        CompareOp::Gt => l > r,
        CompareOp::Gte => l >= r,
        CompareOp::Lt => l < r,
        CompareOp::Lte => l <= r,
    }
}

fn apply_op_str(l: &str, op: CompareOp, r: &str) -> bool {
    match op {
        CompareOp::Eq => l == r,
        CompareOp::Neq => l != r,
        CompareOp::Gt => l > r,
        CompareOp::Gte => l >= r,
        CompareOp::Lt => l < r,
        CompareOp::Lte => l <= r,
    }
}

/// Produce a left value that satisfies the predicate. Preserves the original
/// type of `left` when possible.
fn repair_left(
    left: &serde_json::Value,
    op: CompareOp,
    right: &serde_json::Value,
) -> Option<serde_json::Value> {
    // Numeric repair.
    if let (Some(_), Some(r)) = (left.as_f64(), right.as_f64()) {
        let new_val = match op {
            CompareOp::Eq => r,
            CompareOp::Neq => r + 1.0,
            CompareOp::Gt => r + 1.0,
            CompareOp::Gte => r,
            CompareOp::Lt => r - 1.0,
            CompareOp::Lte => r,
        };
        // Preserve integer-ness when possible.
        if left.is_i64() && new_val.fract() == 0.0 {
            return Some(serde_json::Value::Number(serde_json::Number::from(
                new_val as i64,
            )));
        }
        return Some(serde_json::json!(new_val));
    }

    // String repair.
    if let (Some(_), Some(r)) = (left.as_str(), right.as_str()) {
        let new_val = match op {
            CompareOp::Eq => r.to_string(),
            CompareOp::Neq => format!("{r}_x"),
            CompareOp::Gt => format!("{r}_z"),
            CompareOp::Gte => r.to_string(),
            CompareOp::Lt => {
                // Produce a string strictly less than r. Easy choice: empty
                // string is always <= r; if r is empty we cannot satisfy.
                if r.is_empty() {
                    return None;
                }
                String::new()
            }
            CompareOp::Lte => r.to_string(),
        };
        return Some(serde_json::Value::String(new_val));
    }

    // Boolean repair (only Eq / Neq make sense).
    if let (Some(_), Some(r)) = (left.as_bool(), right.as_bool()) {
        return match op {
            CompareOp::Eq => Some(serde_json::Value::Bool(r)),
            CompareOp::Neq => Some(serde_json::Value::Bool(!r)),
            _ => None,
        };
    }

    None
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(rule: &Rule) -> Rule {
        let json = serde_json::to_string(rule).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn test_rule_serde_roundtrip_range() {
        let r = Rule::Range {
            field: "Pet.age".to_string(),
            min: 1.0,
            max: 20.0,
        };
        assert_eq!(roundtrip(&r), r);
    }

    #[test]
    fn test_rule_serde_roundtrip_choice() {
        let r = Rule::Choice {
            field: "Pet.status".to_string(),
            options: vec![
                serde_json::json!("available"),
                serde_json::json!("pending"),
                serde_json::json!("sold"),
            ],
        };
        assert_eq!(roundtrip(&r), r);
    }

    #[test]
    fn test_rule_serde_roundtrip_const() {
        let r = Rule::Const {
            field: "Pet.id".to_string(),
            value: serde_json::json!(42),
        };
        assert_eq!(roundtrip(&r), r);
    }

    #[test]
    fn test_rule_serde_roundtrip_pattern() {
        let r = Rule::Pattern {
            field: "Pet.name".to_string(),
            regex: r"[A-Z][a-z]{2,5}".to_string(),
        };
        assert_eq!(roundtrip(&r), r);
    }

    #[test]
    fn test_rule_serde_roundtrip_compare() {
        let r = Rule::Compare {
            left: "Pet.age".to_string(),
            op: CompareOp::Gt,
            right: serde_json::json!("Pet.min_age"),
        };
        assert_eq!(roundtrip(&r), r);
    }

    #[test]
    fn test_parse_rules_array() {
        let json = r#"[
            {"kind": "range", "field": "Pet.age", "min": 1, "max": 20},
            {"kind": "choice", "field": "Pet.status", "options": ["a", "b"]}
        ]"#;
        let rules = parse_rules(json).unwrap();
        assert_eq!(rules.len(), 2);
        assert!(matches!(rules[0], Rule::Range { .. }));
        assert!(matches!(rules[1], Rule::Choice { .. }));
    }

    #[test]
    fn test_parse_rules_empty() {
        assert_eq!(parse_rules("").unwrap().len(), 0);
        assert_eq!(parse_rules("[]").unwrap().len(), 0);
        assert_eq!(parse_rules("null").unwrap().len(), 0);
    }

    #[test]
    fn test_parse_rules_invalid() {
        assert!(parse_rules("not json").is_err());
        assert!(parse_rules("{}").is_err()); // not an array
    }

    #[test]
    fn test_validate_conflict_const_range() {
        let rules = vec![
            Rule::Const {
                field: "Pet.age".to_string(),
                value: serde_json::json!(5),
            },
            Rule::Range {
                field: "Pet.age".to_string(),
                min: 1.0,
                max: 10.0,
            },
        ];
        let err = validate_rules(&rules, None).unwrap_err();
        assert!(err.contains("Conflicting"));
        assert!(err.contains("Pet.age"));
    }

    #[test]
    fn test_validate_conflict_two_choices() {
        let rules = vec![
            Rule::Choice {
                field: "Pet.status".to_string(),
                options: vec![serde_json::json!("a")],
            },
            Rule::Choice {
                field: "Pet.status".to_string(),
                options: vec![serde_json::json!("b")],
            },
        ];
        assert!(validate_rules(&rules, None).is_err());
    }

    #[test]
    fn test_validate_compare_self_loop() {
        let rules = vec![Rule::Compare {
            left: "Pet.age".to_string(),
            op: CompareOp::Gt,
            right: serde_json::json!("Pet.age"),
        }];
        let err = validate_rules(&rules, None).unwrap_err();
        assert!(err.contains("self-loop"), "got: {err}");
    }

    #[test]
    fn test_validate_compare_cycle() {
        let rules = vec![
            Rule::Compare {
                left: "Pet.a".to_string(),
                op: CompareOp::Gt,
                right: serde_json::json!("Pet.b"),
            },
            Rule::Compare {
                left: "Pet.b".to_string(),
                op: CompareOp::Gt,
                right: serde_json::json!("Pet.a"),
            },
        ];
        let err = validate_rules(&rules, None).unwrap_err();
        assert!(err.contains("cycle"), "got: {err}");
    }

    #[test]
    fn test_validate_compare_no_cycle_chain() {
        // a -> b -> c is fine.
        let rules = vec![
            Rule::Compare {
                left: "Pet.a".to_string(),
                op: CompareOp::Gt,
                right: serde_json::json!("Pet.b"),
            },
            Rule::Compare {
                left: "Pet.b".to_string(),
                op: CompareOp::Gt,
                right: serde_json::json!("Pet.c"),
            },
        ];
        validate_rules(&rules, None).unwrap();
    }

    #[test]
    fn test_validate_pattern_bad_regex() {
        let rules = vec![Rule::Pattern {
            field: "Pet.name".to_string(),
            regex: "[invalid".to_string(),
        }];
        assert!(validate_rules(&rules, None).is_err());
    }

    #[test]
    fn test_validate_pattern_good_regex() {
        let rules = vec![Rule::Pattern {
            field: "Pet.name".to_string(),
            regex: r"[a-z]{3,5}".to_string(),
        }];
        validate_rules(&rules, None).unwrap();
    }

    #[test]
    fn test_validate_choice_empty_options() {
        let rules = vec![Rule::Choice {
            field: "Pet.status".to_string(),
            options: vec![],
        }];
        // Spec is None so we won't validate options-against-spec, but the
        // generic validator should still catch empty options when a spec is
        // present. Without spec, we accept it (it'll be a runtime no-op).
        // Add an explicit check below with spec.
        validate_rules(&rules, None).unwrap();
    }

    #[test]
    fn test_split_field_path() {
        assert_eq!(split_field_path("Pet.age"), Some(("Pet", "age")));
        assert_eq!(split_field_path("a.b.c"), Some(("a", "b.c")));
        assert_eq!(split_field_path("noDot"), None);
        assert_eq!(split_field_path(".missingDef"), None);
        assert_eq!(split_field_path("missingProp."), None);
    }

    #[test]
    fn test_generate_for_field_rule_const() {
        let r = Rule::Const {
            field: "Pet.x".to_string(),
            value: serde_json::json!("hello"),
        };
        assert_eq!(
            generate_for_field_rule(&r),
            Some(serde_json::json!("hello"))
        );
    }

    #[test]
    fn test_generate_for_field_rule_choice() {
        let opts = vec![serde_json::json!("a"), serde_json::json!("b")];
        let r = Rule::Choice {
            field: "Pet.x".to_string(),
            options: opts.clone(),
        };
        for _ in 0..50 {
            let v = generate_for_field_rule(&r).unwrap();
            assert!(opts.contains(&v));
        }
    }

    #[test]
    fn test_generate_for_field_rule_range_int() {
        let r = Rule::Range {
            field: "Pet.x".to_string(),
            min: 5.0,
            max: 10.0,
        };
        for _ in 0..100 {
            let v = generate_for_field_rule(&r).unwrap();
            let n = v.as_i64().unwrap();
            assert!((5..=10).contains(&n), "value {n} outside [5,10]");
        }
    }

    #[test]
    fn test_generate_for_field_rule_range_float() {
        let r = Rule::Range {
            field: "Pet.x".to_string(),
            min: 1.5,
            max: 2.5,
        };
        for _ in 0..100 {
            let v = generate_for_field_rule(&r).unwrap();
            let n = v.as_f64().unwrap();
            assert!((1.5..=2.5).contains(&n), "value {n} outside [1.5,2.5]");
        }
    }

    #[test]
    fn test_generate_for_pattern_matches() {
        let regex_str = r"[a-z]{5,10}";
        let compiled = regex::Regex::new(&format!("^{regex_str}$")).unwrap();
        for _ in 0..50 {
            let v = generate_for_pattern(regex_str).unwrap();
            let s = v.as_str().unwrap();
            assert!(compiled.is_match(s), "'{s}' does not match {regex_str}");
        }
    }

    #[test]
    fn test_generate_for_pattern_invalid_regex() {
        assert!(generate_for_pattern("[invalid").is_err());
    }

    #[test]
    fn test_compare_holds_numeric() {
        let l = serde_json::json!(5);
        let r = serde_json::json!(3);
        assert!(compare_holds(&l, CompareOp::Gt, &r));
        assert!(compare_holds(&l, CompareOp::Gte, &r));
        assert!(!compare_holds(&l, CompareOp::Lt, &r));
        assert!(!compare_holds(&l, CompareOp::Eq, &r));
        assert!(compare_holds(&l, CompareOp::Neq, &r));
    }

    #[test]
    fn test_compare_holds_string() {
        let l = serde_json::json!("banana");
        let r = serde_json::json!("apple");
        assert!(compare_holds(&l, CompareOp::Gt, &r));
        assert!(!compare_holds(&l, CompareOp::Lt, &r));
        assert!(compare_holds(&l, CompareOp::Neq, &r));
    }

    #[test]
    fn test_compare_holds_eq_string() {
        let l = serde_json::json!("hi");
        let r = serde_json::json!("hi");
        assert!(compare_holds(&l, CompareOp::Eq, &r));
        assert!(!compare_holds(&l, CompareOp::Neq, &r));
    }

    #[test]
    fn test_apply_compare_repair_numeric_gt() {
        // age must be > min_age. Set up a row that violates.
        let mut row = serde_json::Map::new();
        row.insert("age".to_string(), serde_json::json!(2));
        row.insert("min_age".to_string(), serde_json::json!(10));

        let rules = vec![Rule::Compare {
            left: "Pet.age".to_string(),
            op: CompareOp::Gt,
            right: serde_json::json!("Pet.min_age"),
        }];
        apply_compare_rules(&mut row, &rules);

        let age = row["age"].as_i64().unwrap();
        let min_age = row["min_age"].as_i64().unwrap();
        assert!(age > min_age, "age {age} should be > min_age {min_age}");
    }

    #[test]
    fn test_apply_compare_repair_numeric_lt() {
        let mut row = serde_json::Map::new();
        row.insert("a".to_string(), serde_json::json!(50));
        row.insert("b".to_string(), serde_json::json!(10));

        let rules = vec![Rule::Compare {
            left: "Pet.a".to_string(),
            op: CompareOp::Lt,
            right: serde_json::json!("Pet.b"),
        }];
        apply_compare_rules(&mut row, &rules);

        let a = row["a"].as_i64().unwrap();
        let b = row["b"].as_i64().unwrap();
        assert!(a < b, "a {a} should be < b {b}");
    }

    #[test]
    fn test_apply_compare_repair_string_eq() {
        let mut row = serde_json::Map::new();
        row.insert("city".to_string(), serde_json::json!("Paris"));
        row.insert("home_city".to_string(), serde_json::json!("Tokyo"));

        let rules = vec![Rule::Compare {
            left: "Person.city".to_string(),
            op: CompareOp::Eq,
            right: serde_json::json!("Person.home_city"),
        }];
        apply_compare_rules(&mut row, &rules);

        assert_eq!(row["city"].as_str().unwrap(), "Tokyo");
    }

    #[test]
    fn test_apply_compare_with_literal_right() {
        let mut row = serde_json::Map::new();
        row.insert("age".to_string(), serde_json::json!(2));

        let rules = vec![Rule::Compare {
            left: "Pet.age".to_string(),
            op: CompareOp::Gte,
            right: serde_json::json!(18),
        }];
        apply_compare_rules(&mut row, &rules);

        assert!(row["age"].as_i64().unwrap() >= 18);
    }

    #[test]
    fn test_apply_compare_already_satisfied_no_change() {
        let mut row = serde_json::Map::new();
        row.insert("a".to_string(), serde_json::json!(100));
        row.insert("b".to_string(), serde_json::json!(10));

        let rules = vec![Rule::Compare {
            left: "Pet.a".to_string(),
            op: CompareOp::Gt,
            right: serde_json::json!("Pet.b"),
        }];
        apply_compare_rules(&mut row, &rules);

        // a was already > b, should remain 100.
        assert_eq!(row["a"].as_i64().unwrap(), 100);
    }

    #[test]
    fn test_build_field_rule_map() {
        let rules = vec![
            Rule::Range {
                field: "Pet.age".to_string(),
                min: 1.0,
                max: 10.0,
            },
            Rule::Compare {
                left: "Pet.a".to_string(),
                op: CompareOp::Gt,
                right: serde_json::json!("Pet.b"),
            },
        ];
        let map = build_field_rule_map(&rules);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key(&("Pet".to_string(), "age".to_string())));
    }

    #[test]
    fn test_build_compare_rules_by_def() {
        let rules = vec![
            Rule::Compare {
                left: "Pet.a".to_string(),
                op: CompareOp::Gt,
                right: serde_json::json!("Pet.b"),
            },
            Rule::Compare {
                left: "Person.x".to_string(),
                op: CompareOp::Eq,
                right: serde_json::json!("Person.y"),
            },
            Rule::Range {
                field: "Pet.age".to_string(),
                min: 1.0,
                max: 10.0,
            },
        ];
        let by_def = build_compare_rules_by_def(&rules);
        assert_eq!(by_def.len(), 2);
        assert_eq!(by_def.get("Pet").unwrap().len(), 1);
        assert_eq!(by_def.get("Person").unwrap().len(), 1);
    }
}
