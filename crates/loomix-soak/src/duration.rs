//! Parses the `--duration` flag's value (e.g. `30m`, `1800s`, `2h`,
//! `0.5h`) -- pure string parsing, no I/O. Kept in its own file rather
//! than inline in `main.rs`: everything else in this crate calls real
//! hardware APIs and is excluded from the `cargo llvm-cov` gate (see
//! `main.rs`'s doc comment), but this one function is ordinary,
//! deterministic logic with no excuse not to be tested.

use std::time::Duration;

pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let split_at = s.find(|c: char| c.is_alphabetic()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split_at);
    let value: f64 = num
        .parse()
        .map_err(|_| format!("bad duration '{s}', expected e.g. 30m, 1800s, 2h, 0.5h"))?;
    let seconds = match unit {
        "" | "s" => value,
        "m" => value * 60.0,
        "h" => value * 3600.0,
        other => {
            return Err(format!(
                "unknown duration unit '{other}', expected s, m or h"
            ))
        }
    };
    Ok(Duration::from_secs_f64(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_or_explicit_seconds() {
        assert_eq!(parse_duration("5").unwrap(), Duration::from_secs(5));
        assert_eq!(parse_duration("5s").unwrap(), Duration::from_secs(5));
    }

    #[test]
    fn parses_minutes_and_hours() {
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
    }

    #[test]
    fn parses_a_fractional_value() {
        assert_eq!(parse_duration("0.5h").unwrap(), Duration::from_secs(1800));
    }

    #[test]
    fn rejects_an_unknown_unit() {
        assert!(parse_duration("5x").is_err());
    }

    #[test]
    fn rejects_a_non_numeric_value() {
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn rejects_an_empty_string() {
        assert!(parse_duration("").is_err());
    }
}
