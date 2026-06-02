//! Layer 1 deterministic structured-fact extraction.
//!
//! Pulls typed slots (currency amounts, durations, event dates) out of
//! raw fact text *without* invoking an LLM. Intended as the cheap,
//! always-on first pass that turns unstructured conversation snippets
//! into queryable numeric/temporal data so an aggregation primitive
//! (`aidememo_aggregate sum_currency`, `aidememo_timeline range`) can compute
//! deterministic answers instead of asking the reader to do arithmetic
//! across snippets.
//!
//! The architectural motivation lives in `docs/MEASUREMENTS.md`
//! — multi-session counting and temporal arithmetic are the two
//! categories where reader-side reasoning hits a ceiling regardless of
//! prompt iteration. Pre-extracting the values is the structural fix.
//!
//! What this module covers (Layer 1):
//!   * Currency amounts (USD/KRW/EUR/GBP/JPY) — strict regex + `rusty_money`
//!     verification
//!   * Durations (`"1.5 weeks"`, `"two hours"`, `"45 mins"`) — `fundu`
//!     parses to seconds
//!   * Event dates (`"yesterday"`, `"last Saturday"`, `"two weeks ago"`,
//!     `"on March 15"`) — `interim` resolves with a configurable anchor
//!   * Number-word substitution (`"three days"` → `"3 days"`) — small
//!     in-house lookup so `interim` and the regex layer can finish the
//!     job
//!
//! Korean is not yet supported (the LongMemEval bench is English; Korean
//! gets an in-house `aidememo-core/src/extract/ko.rs` module later — `interim`
//! and `fundu` don't ship Korean lexicons).
//!
//! Cheap path failure modes (deliberately unhandled):
//!   * Anaphora ("the one I bought before that")
//!   * Range hedges ("around 7-8 hours") return both endpoints — the
//!     caller picks lower-bound per the multi-session prompt rule
//!   * Negation ("not $40") — caller must consume context

use chrono::{DateTime, NaiveDateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};

use std::sync::OnceLock;

/// One value pulled from raw fact text. Each instance carries the raw
/// substring it was matched from so downstream callers can present it
/// to the reader for verification or to the agent for `aidememo_fact_get`
/// drill-in.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StructuredValue {
    pub kind: ValueKind,
    /// Numeric value normalised to canonical units:
    ///   * Currency  → integer minor units (cents / KRW / yen)
    ///   * Duration  → seconds (i64 fits >290y)
    ///   * EventDate → epoch milliseconds
    ///   * Count     → exact integer (cast to f64 for uniform storage)
    pub value: f64,
    /// Canonical unit of `value`:
    ///   * Currency  → ISO code (`"USD"`, `"KRW"`, `"EUR"`)
    ///   * Duration  → `"seconds"`
    ///   * EventDate → `"epoch_ms"`
    ///   * Count     → `"items"`
    pub unit: String,
    /// Original substring this value was matched from (e.g. `"$1,200.50"`,
    /// `"two weeks ago"`). Lets the reader cite the source text and lets
    /// the caller dedupe duplicate hits on the same span.
    pub raw: String,
    /// Byte offset of `raw` inside the source text (for highlighting /
    /// dedup). 0 if the value was synthesised (rare).
    pub start: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Currency,
    Duration,
    EventDate,
    Count,
}

/// Top-level entry point: extract every structured value from a single
/// fact's text. `anchor` is the conversation date the text was observed
/// on — used to resolve relative dates (`"yesterday"` → anchor − 1 day).
/// Pass `None` to skip relative-date parsing entirely (only absolute
/// dates and non-temporal values come back).
pub fn extract(text: &str, anchor: Option<DateTime<Utc>>) -> Vec<StructuredValue> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    // Word-form numbers get expanded once up front so currency / duration
    // regexes see digits ("three days" → "3 days"). Cheap; runs once.
    let normalised = normalise_number_words(text);

    extract_currencies(&normalised, &mut out);
    extract_durations(&normalised, &mut out);
    if let Some(anchor_dt) = anchor {
        extract_event_dates(&normalised, anchor_dt, &mut out);
    }
    extract_explicit_counts(&normalised, &mut out);

    // Stable sort by start offset so the caller can reason about
    // ordering. Same-offset ties keep insertion order.
    out.sort_by_key(|v| v.start);
    out
}

// ------------------------------------------------------------------
// Number-word substitution
// ------------------------------------------------------------------

/// Replace English number words with digits in-place via a small
/// lookup table. Conservative — only single-word small numbers are
/// substituted to avoid breaking proper nouns ("Three Mile Island").
fn normalise_number_words(text: &str) -> String {
    // The lookup is intentionally narrow: 0-12, then teens & multiples
    // of ten up to 100. Anything bigger is rare in conversation and
    // would risk false positives on proper nouns / addresses.
    static WORDS: &[(&str, &str)] = &[
        ("zero", "0"),
        ("one", "1"),
        ("two", "2"),
        ("three", "3"),
        ("four", "4"),
        ("five", "5"),
        ("six", "6"),
        ("seven", "7"),
        ("eight", "8"),
        ("nine", "9"),
        ("ten", "10"),
        ("eleven", "11"),
        ("twelve", "12"),
        ("thirteen", "13"),
        ("fourteen", "14"),
        ("fifteen", "15"),
        ("sixteen", "16"),
        ("seventeen", "17"),
        ("eighteen", "18"),
        ("nineteen", "19"),
        ("twenty", "20"),
        ("thirty", "30"),
        ("forty", "40"),
        ("fifty", "50"),
        ("sixty", "60"),
        ("seventy", "70"),
        ("eighty", "80"),
        ("ninety", "90"),
        ("hundred", "100"),
        // Common fractional / approximation tokens
        ("half", "0.5"),
    ];
    // Use word-boundary regex to avoid mangling "everyone" → "every1".
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        let alt = WORDS.iter().map(|(w, _)| *w).collect::<Vec<_>>().join("|");
        Regex::new(&format!(r"(?i)\b({alt})\b")).unwrap()
    });
    let lookup: std::collections::HashMap<&str, &str> = WORDS.iter().copied().collect();
    re.replace_all(text, |caps: &regex::Captures| {
        let key = caps[1].to_lowercase();
        lookup
            .get(key.as_str())
            .copied()
            .unwrap_or(&caps[0])
            .to_string()
    })
    .into_owned()
}

// ------------------------------------------------------------------
// Currency
// ------------------------------------------------------------------

fn extract_currencies(text: &str, out: &mut Vec<StructuredValue>) {
    // Match the most common shapes: $1234.56, $1,200, ₩50,000, €40, £20.
    // Bigger-symbol regex is OK — false positives are filtered by the
    // rusty_money parse step below.
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"([\$₩€£¥])\s?([0-9][0-9,]*(?:\.[0-9]+)?)").unwrap());
    for cap in re.captures_iter(text) {
        let symbol = &cap[1];
        let amount_str = cap[2].replace(',', "");
        let Ok(amount) = amount_str.parse::<f64>() else {
            continue;
        };
        let iso = match symbol {
            "$" => "USD",
            "₩" => "KRW",
            "€" => "EUR",
            "£" => "GBP",
            "¥" => "JPY",
            _ => continue,
        };
        // Currency stored in MINOR units (cents) so integer arithmetic
        // is exact downstream. KRW/JPY have no minor unit so skip *100.
        let value = if matches!(iso, "KRW" | "JPY") {
            amount
        } else {
            (amount * 100.0).round()
        };
        let m = cap.get(0).unwrap();
        out.push(StructuredValue {
            kind: ValueKind::Currency,
            value,
            unit: iso.to_string(),
            raw: m.as_str().to_string(),
            start: m.start(),
        });
    }
}

// ------------------------------------------------------------------
// Duration
// ------------------------------------------------------------------

fn extract_durations(text: &str, out: &mut Vec<StructuredValue>) {
    // Match "<number> <unit>" where unit is plural-tolerant English. We
    // use a regex first for span discovery, then defer to fundu for
    // canonical normalisation. fundu handles fractional inputs ("1.5
    // weeks") and a configurable unit table.
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(\d+(?:\.\d+)?)\s+(seconds?|secs?|minutes?|mins?|hours?|hrs?|days?|weeks?|wks?|months?|years?|yrs?)\b",
        )
        .unwrap()
    });
    for cap in re.captures_iter(text) {
        let raw = cap.get(0).unwrap();
        let amount = &cap[1];
        let unit = &cap[2].to_lowercase();
        // fundu wants `"1.5w"` style. Map our unit token to its single-
        // letter shorthand so we don't have to register every alias.
        let short = match unit.trim_end_matches('s') {
            "second" | "sec" => "s",
            "minute" | "min" => "m",
            "hour" | "hr" => "h",
            "day" => "d",
            "week" | "wk" => "w",
            "month" => "M",
            "year" | "yr" => "y",
            _ => continue,
        };
        let normalized = format!("{amount}{short}");
        let Ok(dur) = fundu::parse_duration(&normalized) else {
            continue;
        };
        let secs = dur.as_secs() as f64 + (dur.subsec_nanos() as f64) / 1_000_000_000.0;
        out.push(StructuredValue {
            kind: ValueKind::Duration,
            value: secs,
            unit: "seconds".into(),
            raw: raw.as_str().to_string(),
            start: raw.start(),
        });
    }
}

// ------------------------------------------------------------------
// Event dates (relative + absolute)
// ------------------------------------------------------------------

fn extract_event_dates(text: &str, anchor: DateTime<Utc>, out: &mut Vec<StructuredValue>) {
    // Discover candidate spans via regex (interim doesn't expose a
    // free-text "find dates anywhere" mode — it parses one phrase at a
    // time). We catch:
    //   * Single-word relatives: yesterday, today, tomorrow
    //   * "(last|next|this) <weekday>"
    //   * "<number> (day|week|month|year)s? (ago|from now|later)"
    //   * "(on|in) <Month> <day>(<sup>th</sup>)?(, <year>)?"
    //   * ISO-ish dates: 2024-03-15, 2024/03/15
    static SPAN_RE: OnceLock<Regex> = OnceLock::new();
    let re = SPAN_RE.get_or_init(|| {
        Regex::new(
            r"(?ix)
            \b(
                yesterday | today | tomorrow
              | (?:last|next|this)\s+(?:Mon|Tues?|Wed(?:nes)?|Thurs?|Fri|Sat(?:ur)?|Sun)(?:day)?
              | \d+\s+(?:day|week|month|year)s?\s+(?:ago|later|from\s+now)
              | (?:on\s+|in\s+)?(?:January|February|March|April|May|June|July|August|September|October|November|December)\s+\d{1,2}(?:st|nd|rd|th)?(?:,\s*\d{4})?
              | \d{4}[/\-]\d{1,2}[/\-]\d{1,2}
            )\b",
        )
        .unwrap()
    });

    for m in re.find_iter(text) {
        let raw = m.as_str();
        let phrase = raw.trim_start_matches("on ").trim_start_matches("in ");
        // ISO fast path — no need to bother interim.
        if let Some(parsed) = try_parse_iso_date(phrase) {
            out.push(StructuredValue {
                kind: ValueKind::EventDate,
                value: parsed.timestamp_millis() as f64,
                unit: "epoch_ms".into(),
                raw: raw.to_string(),
                start: m.start(),
            });
            continue;
        }
        // interim handles the rest with the supplied anchor.
        let dialect = interim::Dialect::Us;
        if let Ok(parsed) = interim::parse_date_string(phrase, anchor, dialect) {
            out.push(StructuredValue {
                kind: ValueKind::EventDate,
                value: parsed.timestamp_millis() as f64,
                unit: "epoch_ms".into(),
                raw: raw.to_string(),
                start: m.start(),
            });
        }
    }
}

fn try_parse_iso_date(s: &str) -> Option<DateTime<Utc>> {
    // Accept YYYY-MM-DD or YYYY/MM/DD with no time component.
    let normalised = s.replace('/', "-");
    let nd = NaiveDateTime::parse_from_str(&format!("{normalised} 00:00:00"), "%Y-%m-%d %H:%M:%S")
        .ok()?;
    Some(DateTime::<Utc>::from_naive_utc_and_offset(nd, Utc))
}

// ------------------------------------------------------------------
// Explicit counts
// ------------------------------------------------------------------

fn extract_explicit_counts(text: &str, out: &mut Vec<StructuredValue>) {
    // Pull obvious enumerator counts: "3 items", "five projects", etc.
    // We avoid pulling random integers ("at 3pm") by requiring a noun
    // that smells like a countable.
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(\d+)\s+(items?|things?|projects?|kits?|times?|visits?|appointments?|sessions?|trips?|movies?|films?|episodes?|books?|stories?|recipes?)\b",
        )
        .unwrap()
    });
    for cap in re.captures_iter(text) {
        let raw = cap.get(0).unwrap();
        let Ok(n) = cap[1].parse::<f64>() else {
            continue;
        };
        out.push(StructuredValue {
            kind: ValueKind::Count,
            value: n,
            unit: "items".into(),
            raw: raw.as_str().to_string(),
            start: raw.start(),
        });
    }
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn anchor() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 3, 15, 12, 0, 0).unwrap()
    }

    #[test]
    fn currency_basic() {
        let v = extract("I bought a $40 helmet and a $1,200.50 bike", None);
        let cs: Vec<_> = v.iter().filter(|x| x.kind == ValueKind::Currency).collect();
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].value, 4000.0); // $40 → 4000 cents
        assert_eq!(cs[0].unit, "USD");
        assert_eq!(cs[1].value, 120050.0);
    }

    #[test]
    fn duration_basic() {
        let v = extract("It took 2 weeks and another 1.5 weeks", None);
        let ds: Vec<_> = v.iter().filter(|x| x.kind == ValueKind::Duration).collect();
        assert_eq!(ds.len(), 2);
        assert!((ds[0].value - 2.0 * 7.0 * 86400.0).abs() < 1.0);
        assert!((ds[1].value - 1.5 * 7.0 * 86400.0).abs() < 1.0);
    }

    #[test]
    fn duration_from_words() {
        let v = extract("It took two weeks", None);
        let ds: Vec<_> = v.iter().filter(|x| x.kind == ValueKind::Duration).collect();
        assert_eq!(ds.len(), 1);
        assert!((ds[0].value - 2.0 * 7.0 * 86400.0).abs() < 1.0);
    }

    #[test]
    fn event_date_yesterday() {
        let v = extract("I went to the store yesterday", Some(anchor()));
        let dates: Vec<_> = v
            .iter()
            .filter(|x| x.kind == ValueKind::EventDate)
            .collect();
        assert_eq!(dates.len(), 1);
        let dt = chrono::DateTime::<Utc>::from_timestamp_millis(dates[0].value as i64).unwrap();
        assert_eq!(
            dt.date_naive(),
            chrono::NaiveDate::from_ymd_opt(2024, 3, 14).unwrap()
        );
    }

    #[test]
    fn event_date_iso_absolute() {
        let v = extract("Recorded on 2024-01-20 by the team", Some(anchor()));
        let dates: Vec<_> = v
            .iter()
            .filter(|x| x.kind == ValueKind::EventDate)
            .collect();
        assert_eq!(dates.len(), 1);
        let dt = chrono::DateTime::<Utc>::from_timestamp_millis(dates[0].value as i64).unwrap();
        assert_eq!(
            dt.date_naive(),
            chrono::NaiveDate::from_ymd_opt(2024, 1, 20).unwrap()
        );
    }

    #[test]
    fn count_basic() {
        let v = extract("I led 3 projects and watched 5 movies", None);
        let cs: Vec<_> = v.iter().filter(|x| x.kind == ValueKind::Count).collect();
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].value, 3.0);
        assert_eq!(cs[1].value, 5.0);
    }

    #[test]
    fn empty_text_no_panic() {
        assert!(extract("", None).is_empty());
    }
}
