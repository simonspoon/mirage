// Mock data seeder
#![allow(dead_code)]

use std::collections::HashMap;

use fake::Fake;
use fake::faker::address::en::*;
use fake::faker::company::en::{Buzzword, CatchPhrase, CompanyName};
use fake::faker::creditcard::en::CreditCardNumber;
use fake::faker::internet::en::*;
use fake::faker::lorem::en::*;
use fake::faker::name::en::{FirstName, LastName};
use fake::faker::phone_number::en::PhoneNumber;
use rand::RngExt;
use rusqlite::Connection;
use serde::Deserialize;

use crate::parser::{SchemaObject, SwaggerSpec};

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FakerStrategy {
    Auto,
    Word,
    Name,
    Email,
    Phone,
    Url,
    Sentence,
    Paragraph,
    Uuid,
    Date,
    Integer,
    Float,
    Boolean,
    FirstName,
    LastName,
    FullName,
    Username,
    Password,
    City,
    State,
    ZipCode,
    StreetAddress,
    Country,
    CompanyName,
    JobTitle,
    CreditCard,
    #[serde(rename = "ipv4")]
    IPv4,
    #[serde(rename = "ipv6")]
    IPv6,
    UserAgent,
    HexColor,
    Latitude,
    Longitude,
    FilePath,
    MimeType,
    CurrencyCode,
    CurrencyName,
    Ssn,
    Birthday,
    Sku,
    DomainName,
    FreeEmail,
    SafeEmail,
    Buzzword,
    CatchPhrase,
    Barcode,
    PhoneNumber,
    Title,
    Suffix,
}

fn generate_for_strategy(strategy: &FakerStrategy) -> serde_json::Value {
    let mut rng = rand::rng();

    match strategy {
        FakerStrategy::Auto => {
            // Should not be called directly; handled by caller
            let w: String = Word().fake();
            serde_json::Value::String(w)
        }
        FakerStrategy::Word => {
            let w: String = Word().fake();
            serde_json::Value::String(w)
        }
        FakerStrategy::Name | FakerStrategy::FullName => {
            let n: String = fake::faker::name::en::Name().fake();
            serde_json::Value::String(n)
        }
        FakerStrategy::FirstName => {
            let n: String = FirstName().fake();
            serde_json::Value::String(n)
        }
        FakerStrategy::LastName => {
            let n: String = LastName().fake();
            serde_json::Value::String(n)
        }
        FakerStrategy::Email => {
            let e: String = SafeEmail().fake();
            serde_json::Value::String(e)
        }
        FakerStrategy::FreeEmail => {
            let e: String = FreeEmail().fake();
            serde_json::Value::String(e)
        }
        FakerStrategy::SafeEmail => {
            let e: String = SafeEmail().fake();
            serde_json::Value::String(e)
        }
        FakerStrategy::Phone | FakerStrategy::PhoneNumber => {
            let p: String = PhoneNumber().fake();
            serde_json::Value::String(p)
        }
        FakerStrategy::Url => {
            let domain: String = DomainSuffix().fake();
            let word: String = Word().fake();
            serde_json::Value::String(format!("https://{word}.{domain}"))
        }
        FakerStrategy::Sentence => {
            let s: String = Sentence(3..8).fake();
            serde_json::Value::String(s)
        }
        FakerStrategy::Paragraph => {
            let p: String = Paragraph(2..5).fake();
            serde_json::Value::String(p)
        }
        FakerStrategy::Uuid => serde_json::Value::String(uuid::Uuid::new_v4().to_string()),
        FakerStrategy::Date => {
            let year = rng.random_range(2020..=2025);
            let month = rng.random_range(1..=12u32);
            let day = rng.random_range(1..=28u32);
            serde_json::Value::String(format!("{year:04}-{month:02}-{day:02}"))
        }
        FakerStrategy::Integer => {
            let n: i64 = rng.random_range(1..10000);
            serde_json::Value::Number(serde_json::Number::from(n))
        }
        FakerStrategy::Float => {
            let n: f64 = rng.random_range(0.0..10000.0);
            serde_json::json!(n)
        }
        FakerStrategy::Boolean => serde_json::Value::Bool(rng.random::<bool>()),
        FakerStrategy::Username => {
            let u: String = Username().fake();
            serde_json::Value::String(u)
        }
        FakerStrategy::Password => {
            let p: String = Password(8..16).fake();
            serde_json::Value::String(p)
        }
        FakerStrategy::City => {
            let c: String = CityName().fake();
            serde_json::Value::String(c)
        }
        FakerStrategy::State => {
            let s: String = StateName().fake();
            serde_json::Value::String(s)
        }
        FakerStrategy::ZipCode => {
            let z: String = ZipCode().fake();
            serde_json::Value::String(z)
        }
        FakerStrategy::StreetAddress => {
            let s: String = StreetName().fake();
            let n: String = BuildingNumber().fake();
            serde_json::Value::String(format!("{n} {s}"))
        }
        FakerStrategy::Country => {
            let c: String = CountryName().fake();
            serde_json::Value::String(c)
        }
        FakerStrategy::CompanyName => {
            let c: String = CompanyName().fake();
            serde_json::Value::String(c)
        }
        FakerStrategy::JobTitle => {
            let t: String = fake::faker::job::en::Title().fake();
            serde_json::Value::String(t)
        }
        FakerStrategy::CreditCard => {
            let c: String = CreditCardNumber().fake();
            serde_json::Value::String(c)
        }
        FakerStrategy::IPv4 => {
            let ip: String = IPv4().fake();
            serde_json::Value::String(ip)
        }
        FakerStrategy::IPv6 => {
            let ip: String = IPv6().fake();
            serde_json::Value::String(ip)
        }
        FakerStrategy::UserAgent => {
            let ua: String = UserAgent().fake();
            serde_json::Value::String(ua)
        }
        FakerStrategy::HexColor => {
            let r = rng.random_range(0..=255u32);
            let g = rng.random_range(0..=255u32);
            let b = rng.random_range(0..=255u32);
            serde_json::Value::String(format!("#{r:02x}{g:02x}{b:02x}"))
        }
        FakerStrategy::Latitude => {
            let lat: f64 = Latitude().fake();
            serde_json::json!(lat)
        }
        FakerStrategy::Longitude => {
            let lon: f64 = Longitude().fake();
            serde_json::json!(lon)
        }
        FakerStrategy::FilePath => {
            let p: String = fake::faker::filesystem::en::FilePath().fake();
            serde_json::Value::String(p)
        }
        FakerStrategy::MimeType => {
            let m: String = fake::faker::filesystem::en::MimeType().fake();
            serde_json::Value::String(m)
        }
        FakerStrategy::CurrencyCode => {
            let c: String = fake::faker::currency::en::CurrencyCode().fake();
            serde_json::Value::String(c)
        }
        FakerStrategy::CurrencyName => {
            let c: String = fake::faker::currency::en::CurrencyName().fake();
            serde_json::Value::String(c)
        }
        FakerStrategy::Ssn => {
            let a = rng.random_range(100..999u32);
            let b = rng.random_range(10..99u32);
            let c = rng.random_range(1000..9999u32);
            serde_json::Value::String(format!("{a:03}-{b:02}-{c:04}"))
        }
        FakerStrategy::Birthday => {
            let year = rng.random_range(1950..=2005);
            let month = rng.random_range(1..=12u32);
            let day = rng.random_range(1..=28u32);
            serde_json::Value::String(format!("{year:04}-{month:02}-{day:02}"))
        }
        FakerStrategy::Sku => {
            let n = rng.random_range(100000..999999u32);
            serde_json::Value::String(format!("SKU-{n:06}"))
        }
        FakerStrategy::DomainName => {
            let word: String = Word().fake();
            let suffix: String = DomainSuffix().fake();
            serde_json::Value::String(format!("{word}.{suffix}"))
        }
        FakerStrategy::Buzzword => {
            let b: String = Buzzword().fake();
            serde_json::Value::String(b)
        }
        FakerStrategy::CatchPhrase => {
            let c: String = CatchPhrase().fake();
            serde_json::Value::String(c)
        }
        FakerStrategy::Barcode => {
            let b: String = fake::faker::barcode::en::Isbn().fake();
            serde_json::Value::String(b)
        }
        FakerStrategy::Title => {
            let t: String = fake::faker::name::en::Title().fake();
            serde_json::Value::String(t)
        }
        FakerStrategy::Suffix => {
            let s: String = fake::faker::name::en::Suffix().fake();
            serde_json::Value::String(s)
        }
    }
}

/// Parse a strategy string (as used in x-faker) into a FakerStrategy.
fn parse_strategy_string(s: &str) -> Option<FakerStrategy> {
    serde_json::from_value(serde_json::json!(s)).ok()
}

/// Resolve format string to a FakerStrategy.
fn strategy_from_format(format: &str) -> Option<FakerStrategy> {
    match format {
        "date-time" => Some(FakerStrategy::Date),
        "date" => Some(FakerStrategy::Date),
        "email" => Some(FakerStrategy::Email),
        "uri" | "url" => Some(FakerStrategy::Url),
        "uuid" => Some(FakerStrategy::Uuid),
        "ipv4" => Some(FakerStrategy::IPv4),
        "ipv6" => Some(FakerStrategy::IPv6),
        "int32" | "int64" => Some(FakerStrategy::Integer),
        "float" | "double" => Some(FakerStrategy::Float),
        "password" => Some(FakerStrategy::Password),
        _ => None,
    }
}

/// Generate a value based on format string, returning None if no mapping exists.
fn generate_for_format(format: &str) -> Option<serde_json::Value> {
    match format {
        "byte" => {
            // base64-ish string: generate random alphanumeric
            let mut rng = rand::rng();
            let chars: String = (0..16)
                .map(|_| {
                    let idx = rng.random_range(0..62u32);
                    match idx {
                        0..=25 => (b'A' + idx as u8) as char,
                        26..=51 => (b'a' + (idx - 26) as u8) as char,
                        _ => (b'0' + (idx - 52) as u8) as char,
                    }
                })
                .collect();
            Some(serde_json::Value::String(chars))
        }
        "binary" => {
            let mut rng = rand::rng();
            let hex: String = (0..16)
                .map(|_| format!("{:02x}", rng.random_range(0..=255u8)))
                .collect();
            Some(serde_json::Value::String(hex))
        }
        other => strategy_from_format(other).map(|s| generate_for_strategy(&s)),
    }
}

/// Match field name heuristics to a FakerStrategy or generate a value directly.
fn generate_for_name_heuristic(name: &str) -> Option<serde_json::Value> {
    let lower = name.to_lowercase();

    // Date/time patterns
    if lower.ends_with("_at")
        || lower.ends_with("_date")
        || lower.ends_with("_time")
        || lower.starts_with("created")
        || lower.starts_with("updated")
        || lower.starts_with("born")
        || lower == "dob"
        || lower == "birthday"
    {
        // Use ISO datetime for _at patterns, date for others
        if lower.ends_with("_at") || lower.ends_with("_time") {
            let mut rng = rand::rng();
            let year = rng.random_range(2020..=2025);
            let month = rng.random_range(1..=12u32);
            let day = rng.random_range(1..=28u32);
            let hour = rng.random_range(0..=23u32);
            let min = rng.random_range(0..=59u32);
            let sec = rng.random_range(0..=59u32);
            return Some(serde_json::Value::String(format!(
                "{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z"
            )));
        }
        return Some(generate_for_strategy(&FakerStrategy::Date));
    }

    // First name (check before generic *name*)
    if lower == "first_name"
        || lower == "given_name"
        || lower == "firstname"
        || lower == "givenname"
    {
        return Some(generate_for_strategy(&FakerStrategy::FirstName));
    }

    // Last name (check before generic *name*)
    if lower == "last_name"
        || lower == "surname"
        || lower == "family_name"
        || lower == "lastname"
        || lower == "familyname"
    {
        return Some(generate_for_strategy(&FakerStrategy::LastName));
    }

    // Username (check before generic *name*)
    if lower.contains("username") || lower.contains("login") || lower.contains("handle") {
        return Some(generate_for_strategy(&FakerStrategy::Username));
    }

    // Generic name
    if lower.contains("name") {
        return Some(generate_for_strategy(&FakerStrategy::FullName));
    }

    // Email
    if lower.contains("email") {
        return Some(generate_for_strategy(&FakerStrategy::Email));
    }

    // Phone
    if lower.contains("phone")
        || lower.contains("mobile")
        || lower.contains("fax")
        || lower.contains("cell")
    {
        return Some(generate_for_strategy(&FakerStrategy::PhoneNumber));
    }

    // Address
    if lower.contains("address") || lower.starts_with("street") || lower.contains("address_line") {
        return Some(generate_for_strategy(&FakerStrategy::StreetAddress));
    }

    // City
    if lower.contains("city") {
        return Some(generate_for_strategy(&FakerStrategy::City));
    }

    // State/Province
    if lower.contains("state") || lower.contains("province") {
        return Some(generate_for_strategy(&FakerStrategy::State));
    }

    // Zip/Postal
    if lower.contains("zip") || lower.contains("postal") {
        return Some(generate_for_strategy(&FakerStrategy::ZipCode));
    }

    // Country
    if lower.contains("country") {
        return Some(generate_for_strategy(&FakerStrategy::Country));
    }

    // Company/Organization
    if lower.contains("company") || lower.contains("organization") || lower.contains("org_name") {
        return Some(generate_for_strategy(&FakerStrategy::CompanyName));
    }

    // Price/Amount (before generic patterns)
    if lower.contains("price")
        || lower.contains("amount")
        || lower.contains("cost")
        || lower.contains("fee")
        || lower.contains("total")
    {
        let mut rng = rand::rng();
        let n: f64 = rng.random_range(1.0..999.99);
        let rounded = (n * 100.0).round() / 100.0;
        return Some(serde_json::json!(rounded));
    }

    // Title
    if lower.contains("title") {
        // If it looks person-related, use job title; otherwise sentence
        if lower.contains("job") || lower.contains("position") || lower.contains("role") {
            return Some(generate_for_strategy(&FakerStrategy::JobTitle));
        }
        return Some(generate_for_strategy(&FakerStrategy::Sentence));
    }

    // Latitude
    if lower.contains("latitude") || lower.ends_with("_lat") || lower == "lat" {
        return Some(generate_for_strategy(&FakerStrategy::Latitude));
    }

    // Longitude
    if lower.contains("longitude")
        || lower.ends_with("_lng")
        || lower.ends_with("_lon")
        || lower == "lng"
        || lower == "lon"
    {
        return Some(generate_for_strategy(&FakerStrategy::Longitude));
    }

    // Avatar/Image
    if lower.contains("avatar")
        || lower.contains("image")
        || lower.contains("photo")
        || lower.contains("picture")
    {
        let mut rng = rand::rng();
        let id = rng.random_range(1..1000u32);
        return Some(serde_json::Value::String(format!(
            "https://picsum.photos/seed/{id}/200/200"
        )));
    }

    // IP
    if lower.contains("ip_address") || lower == "ip" || lower.ends_with("_ip") {
        return Some(generate_for_strategy(&FakerStrategy::IPv4));
    }

    // User Agent
    if lower.contains("user_agent") || lower == "useragent" {
        return Some(generate_for_strategy(&FakerStrategy::UserAgent));
    }

    // Password/Secret/Token
    if lower.contains("password") || lower.contains("secret") || lower.contains("token") {
        return Some(generate_for_strategy(&FakerStrategy::Password));
    }

    // SSN
    if lower.contains("ssn") || lower.contains("social_security") {
        return Some(generate_for_strategy(&FakerStrategy::Ssn));
    }

    // Color
    if lower.contains("color") || lower.contains("colour") {
        return Some(generate_for_strategy(&FakerStrategy::HexColor));
    }

    // Currency
    if lower.contains("currency") {
        return Some(generate_for_strategy(&FakerStrategy::CurrencyCode));
    }

    // SKU/Product Code
    if lower.contains("sku") || lower.contains("product_code") || lower.contains("item_code") {
        return Some(generate_for_strategy(&FakerStrategy::Sku));
    }

    // Domain
    if lower.contains("domain") {
        return Some(generate_for_strategy(&FakerStrategy::DomainName));
    }

    // URL/Website/Link
    if lower.contains("url")
        || lower.contains("website")
        || lower.contains("link")
        || lower.contains("href")
    {
        return Some(generate_for_strategy(&FakerStrategy::Url));
    }

    // Description/Body/Content
    if lower.contains("description")
        || lower.contains("body")
        || lower.contains("content")
        || lower.contains("summary")
        || lower.contains("bio")
        || lower.contains("about")
    {
        return Some(generate_for_strategy(&FakerStrategy::Sentence));
    }

    // Credit Card
    if lower.contains("credit_card") || lower.contains("card_number") {
        return Some(generate_for_strategy(&FakerStrategy::CreditCard));
    }

    // UUID/GUID
    if lower.contains("uuid") || lower.contains("guid") {
        return Some(generate_for_strategy(&FakerStrategy::Uuid));
    }

    // *id (ends with "id" but not uuid -- small integer)
    if lower.ends_with("id") || lower.ends_with("_id") {
        let mut rng = rand::rng();
        let n: i64 = rng.random_range(1..10000);
        return Some(serde_json::Value::Number(serde_json::Number::from(n)));
    }

    None
}

pub fn fake_value_for_field(name: &str, schema: &SchemaObject) -> serde_json::Value {
    let mut rng = rand::rng();

    // Check enum_values first
    if let Some(ref enums) = schema.enum_values
        && !enums.is_empty()
    {
        let idx = rng.random_range(0..enums.len());
        return enums[idx].clone();
    }

    // Match on schema_type for structural types (object, array) first
    match schema.schema_type.as_deref() {
        Some("object") => {
            if let Some(ref props) = schema.properties {
                let mut map = serde_json::Map::new();
                for (prop_name, prop_schema) in props {
                    map.insert(
                        prop_name.clone(),
                        fake_value_for_field(prop_name, prop_schema),
                    );
                }
                return serde_json::Value::Object(map);
            } else {
                return serde_json::Value::Object(serde_json::Map::new());
            }
        }
        Some("array") => {
            if let Some(ref items) = schema.items {
                let count = rng.random_range(1..=3);
                let arr: Vec<serde_json::Value> = (0..count)
                    .map(|_| fake_value_for_field(name, items))
                    .collect();
                return serde_json::Value::Array(arr);
            } else {
                return serde_json::Value::Array(vec![]);
            }
        }
        _ => {}
    }

    // Layer 1: x-faker
    if let Some(ref faker_hint) = schema.x_faker
        && let Some(strategy) = parse_strategy_string(faker_hint)
    {
        return generate_for_strategy(&strategy);
    }

    // Layer 2: format
    if let Some(ref fmt) = schema.format
        && let Some(value) = generate_for_format(fmt)
    {
        return value;
    }

    // Layer 3: name heuristic (only for string types or untyped)
    match schema.schema_type.as_deref() {
        Some("string") | None => {
            if let Some(value) = generate_for_name_heuristic(name) {
                return value;
            }
        }
        _ => {}
    }

    // Layer 4: type fallback
    match schema.schema_type.as_deref() {
        Some("integer") => {
            let n: i64 = rng.random_range(1..10000);
            serde_json::Value::Number(serde_json::Number::from(n))
        }
        Some("number") => {
            let n: f64 = rng.random_range(0.0..10000.0);
            serde_json::json!(n)
        }
        Some("boolean") => serde_json::Value::Bool(rng.random::<bool>()),
        _ => {
            let w: String = Word().fake();
            serde_json::Value::String(w)
        }
    }
}

pub fn fake_value_for_field_with_rule(
    name: &str,
    schema: &SchemaObject,
    rule: Option<&FakerStrategy>,
) -> serde_json::Value {
    match rule {
        None | Some(FakerStrategy::Auto) => fake_value_for_field(name, schema),
        Some(strategy) => generate_for_strategy(strategy),
    }
}

fn map_type(schema: &SchemaObject) -> &str {
    match schema.schema_type.as_deref() {
        Some("integer") => "INTEGER",
        Some("number") => "REAL",
        Some("boolean") => "INTEGER",
        Some("string") => "TEXT",
        Some("object") => "TEXT",
        Some("array") => "TEXT",
        _ => "TEXT",
    }
}

/// Get the effective properties for a schema, handling array-typed definitions.
fn effective_props(
    schema: &SchemaObject,
) -> Option<&std::collections::HashMap<String, SchemaObject>> {
    if let Some(ref props) = schema.properties
        && !props.is_empty()
    {
        return Some(props);
    }
    if schema.schema_type.as_deref() == Some("array")
        && let Some(ref items) = schema.items
        && let Some(ref props) = items.properties
        && !props.is_empty()
    {
        return Some(props);
    }
    None
}

pub fn seed_table(
    conn: &Connection,
    table_name: &str,
    schema: &SchemaObject,
    count: usize,
    faker_rules: Option<&HashMap<String, HashMap<String, FakerStrategy>>>,
) -> Result<(), rusqlite::Error> {
    let props = match effective_props(schema) {
        Some(p) => p,
        None => return Ok(()),
    };

    // Sort column names alphabetically (same as schema.rs)
    let mut col_names: Vec<&String> = props.keys().collect();
    col_names.sort();

    let columns_str = col_names
        .iter()
        .map(|c| format!("\"{}\"", c))
        .collect::<Vec<_>>()
        .join(", ");

    let placeholders: Vec<String> = (1..=col_names.len()).map(|i| format!("?{i}")).collect();
    let placeholders_str = placeholders.join(", ");

    let sql = format!("INSERT INTO \"{table_name}\" ({columns_str}) VALUES ({placeholders_str})");

    for _ in 0..count {
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        for col_name in &col_names {
            let col_schema = &props[col_name.as_str()];
            let rule = faker_rules
                .and_then(|r| r.get(table_name))
                .and_then(|m| m.get(col_name.as_str()));
            let value = fake_value_for_field_with_rule(col_name, col_schema, rule);
            let sqlite_type = map_type(col_schema);

            match sqlite_type {
                "INTEGER" => {
                    let n = match &value {
                        serde_json::Value::Number(num) => num.as_i64().unwrap_or(0),
                        serde_json::Value::Bool(b) => {
                            if *b {
                                1
                            } else {
                                0
                            }
                        }
                        _ => 0,
                    };
                    params.push(Box::new(n));
                }
                "REAL" => {
                    let n = match &value {
                        serde_json::Value::Number(num) => num.as_f64().unwrap_or(0.0),
                        _ => 0.0,
                    };
                    params.push(Box::new(n));
                }
                _ => {
                    // TEXT: for objects/arrays, serialize to JSON string
                    let s = match &value {
                        serde_json::Value::String(s) => s.clone(),
                        other => serde_json::to_string(other).unwrap_or_default(),
                    };
                    params.push(Box::new(s));
                }
            }
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        conn.execute(&sql, param_refs.as_slice())?;
    }

    Ok(())
}

pub fn seed_tables(
    conn: &Connection,
    spec: &SwaggerSpec,
    rows_per_table: usize,
) -> Result<(), rusqlite::Error> {
    seed_tables_filtered(conn, spec, rows_per_table, None, None)
}

pub fn seed_tables_filtered(
    conn: &Connection,
    spec: &SwaggerSpec,
    rows_per_table: usize,
    only: Option<&std::collections::HashSet<String>>,
    faker_rules: Option<&HashMap<String, HashMap<String, FakerStrategy>>>,
) -> Result<(), rusqlite::Error> {
    if let Some(ref definitions) = spec.definitions {
        for (name, schema) in definitions {
            if let Some(filter) = only
                && !filter.contains(name)
            {
                continue;
            }
            seed_table(conn, name, schema, rows_per_table, faker_rules)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::SwaggerSpec;
    use crate::schema::create_tables;
    use rusqlite::Connection;

    fn setup() -> (Connection, SwaggerSpec) {
        let mut spec = SwaggerSpec::from_file("tests/fixtures/petstore.yaml").unwrap();
        spec.resolve_refs();
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn, &spec).unwrap();
        (conn, spec)
    }

    #[test]
    fn test_seed_counts() {
        let (conn, spec) = setup();
        seed_tables(&conn, &spec, 10).unwrap();

        let defs = spec.definitions.as_ref().unwrap();
        for table_name in defs.keys() {
            let sql = format!("SELECT COUNT(*) FROM \"{table_name}\"");
            let count: i64 = conn.query_row(&sql, [], |row| row.get(0)).unwrap();
            assert_eq!(count, 10, "Table {table_name} should have 10 rows");
        }
    }

    #[test]
    fn test_json_columns() {
        let (conn, spec) = setup();
        seed_tables(&conn, &spec, 5).unwrap();

        // Pet.category is an object -> stored as JSON TEXT
        let rows: Vec<String> = {
            let mut stmt = conn.prepare("SELECT \"category\" FROM \"Pet\"").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        for row in &rows {
            let val: serde_json::Value = serde_json::from_str(row)
                .unwrap_or_else(|_| panic!("category should be valid JSON, got: {row}"));
            assert!(
                val.is_object(),
                "category should be a JSON object, got: {val}"
            );
        }

        // Pet.tags is an array -> stored as JSON TEXT
        let rows: Vec<String> = {
            let mut stmt = conn.prepare("SELECT \"tags\" FROM \"Pet\"").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        for row in &rows {
            let val: serde_json::Value = serde_json::from_str(row)
                .unwrap_or_else(|_| panic!("tags should be valid JSON, got: {row}"));
            assert!(val.is_array(), "tags should be a JSON array, got: {val}");
        }
    }

    #[test]
    fn test_enum_values() {
        let (conn, spec) = setup();
        seed_tables(&conn, &spec, 20).unwrap();

        let valid = ["available", "pending", "sold"];
        let mut stmt = conn.prepare("SELECT \"status\" FROM \"Pet\"").unwrap();
        let statuses: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(statuses.len(), 20);
        for status in &statuses {
            assert!(
                valid.contains(&status.as_str()),
                "status '{status}' should be one of {valid:?}"
            );
        }
    }
}
