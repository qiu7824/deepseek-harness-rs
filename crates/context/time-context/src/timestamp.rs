//! ISO-shaped time-context timestamp formatting shared by production and
//! replay validation. Rust port of
//! `packages/context/time-context/src/timestamp.ts`.

/// Why a requested display zone cannot produce the durable formatter. The
/// messages mirror the TS `TypeError`s raised by `browserTimeZone` and
/// `createTimestampFormatter` (the system-zone failure is the TS `RangeError`
/// path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeZoneError {
    /// The value is neither `UTC` nor IANA `Area/Location`-shaped.
    InvalidFormat(String),
    /// The value is well-shaped but absent from the tz database.
    Unsupported(String),
    /// The value resolves to a different canonical zone.
    NotCanonical(String),
    /// The process zone itself cannot be resolved.
    SystemUnresolvable,
}

impl TimeZoneError {
    /// The exact message the TS runtime raises (JSON-quoted values).
    pub fn message(&self) -> String {
        let quoted =
            |value: &str| serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
        match self {
            TimeZoneError::InvalidFormat(value) => format!(
                "browser time zone must be canonical UTC or IANA Area/Location: {}",
                quoted(value)
            ),
            TimeZoneError::Unsupported(value) => {
                format!("browser time zone is unsupported: {}", quoted(value))
            }
            TimeZoneError::NotCanonical(value) => {
                format!("browser time zone must be canonical: {}", quoted(value))
            }
            TimeZoneError::SystemUnresolvable => {
                "failed to resolve the system time zone".to_string()
            }
        }
    }
}

impl std::fmt::Display for TimeZoneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for TimeZoneError {}

/// The TS `IANA_TIME_ZONE` shape (Area/Location with at least one slash).
const IANA_TIME_ZONE: &str = r"^[A-Za-z][A-Za-z0-9_+.-]*(?:/[A-Za-z0-9_+.-]+)+$";

/// CLDR bcp47 aliases the ICU formatter resolves to `UTC` (tzdb keeps the
/// Etc/UTC family as named zones/links).
fn collapse_cldr_alias(name: &str) -> &str {
    match name {
        "Etc/UTC" | "Etc/UCT" | "Etc/Universal" | "Etc/Zulu" => "UTC",
        other => other,
    }
}

/// Validate one display zone exactly like the TS runtime: exempt `UTC`,
/// require IANA `Area/Location` shape, then demand the canonical name (the
/// jiff tz database resolves links like the ICU formatter does). CLDR-style
/// `Etc/UTC`-family aliases collapse to `UTC`, so they can never be canonical
/// input.
pub fn canonical_time_zone(value: &str) -> Result<String, TimeZoneError> {
    if value == "UTC" {
        return Ok("UTC".to_string());
    }
    let shape = regex::Regex::new(IANA_TIME_ZONE).expect("static pattern");
    if !shape.is_match(value) {
        return Err(TimeZoneError::InvalidFormat(value.to_string()));
    }
    let parsed = jiff::tz::TimeZone::get(value)
        .map_err(|_| TimeZoneError::Unsupported(value.to_string()))?;
    // tzdb links resolve elsewhere (the ICU formatter does the same), so a
    // link spelling is never canonical input.
    if crate::tz_links::TZ_LINKS
        .iter()
        .any(|(link, _)| *link == value)
    {
        return Err(TimeZoneError::NotCanonical(value.to_string()));
    }
    let canonical = collapse_cldr_alias(parsed.iana_name().expect("IANA zone"));
    if canonical != value {
        return Err(TimeZoneError::NotCanonical(value.to_string()));
    }
    Ok(canonical.to_string())
}

/// The exact formatter used by durable time-context readings (TS
/// `createTimestampFormatter`): stable numeric local fields and a long
/// numeric offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimestampFormatter {
    tz: jiff::tz::TimeZone,
    /// Canonical display label (TS `resolvedOptions().timeZone`).
    zone_name: String,
}

impl TimestampFormatter {
    /// Build for an explicit display zone, or the process zone when `None`.
    pub fn create(time_zone: Option<&str>) -> Result<Self, TimeZoneError> {
        let canonical = match time_zone {
            Some(zone) => canonical_time_zone(zone)?,
            None => {
                let system = iana_time_zone::get_timezone()
                    .map_err(|_| TimeZoneError::SystemUnresolvable)?;
                canonical_time_zone(&system)?
            }
        };
        let tz = jiff::tz::TimeZone::get(&canonical)
            .map_err(|_| TimeZoneError::Unsupported(canonical.clone()))?;
        Ok(Self {
            tz,
            zone_name: canonical,
        })
    }

    /// Canonical zone label this formatter displays.
    pub fn time_zone(&self) -> &str {
        &self.zone_name
    }
}

/// Format an epoch millisecond value as an ISO-shaped timestamp with offset
/// and IANA zone (TS `formatTimestamp`). The bracket label is the caller's
/// canonical display zone; the offset comes from the formatter's zone.
pub fn format_timestamp(now: i64, formatter: &TimestampFormatter, time_zone: &str) -> String {
    let zoned = jiff::Timestamp::from_millisecond(now)
        .expect("epoch millis")
        .to_zoned(formatter.tz.clone());
    let local = zoned.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string();
    format!("{local}[{time_zone}]")
}
