//! ETS CSV group address export parser.

use std::collections::HashMap;

use crate::error::{ConfigurationError, Result};
use crate::protocol::address::GroupAddress;
use crate::protocol::dpt::DptType;

use crate::log_config;
use crate::logging::LogLevel;

/// Configuration for a group address loaded from ETS.
#[derive(Debug, Clone)]
pub struct GroupAddressConfig {
    /// The DPT type for this group address
    pub dpt: DptType,
    /// Human-readable name from ETS
    pub name: String,
    /// Optional description
    pub description: Option<String>,
}

/// Parse an ETS CSV export into a group address map.
///
/// Supports:
/// - ETS5 format (semicolon-delimited)
/// - ETS6 format (tab-delimited)
/// - DPT formats: "DPST-9-1", "DPT-9", "9.001", "9.1"
/// - UTF-8 BOM handling
///
/// # Errors
///
/// Returns [`ConfigurationError::ParseError`] if `data` is empty or its
/// header row has no recognizable Address column. Rows with an unparseable
/// address or DPT are skipped rather than treated as an error.
pub fn parse_ets_csv(data: &str) -> Result<HashMap<GroupAddress, GroupAddressConfig>> {
    let data = data.strip_prefix('\u{FEFF}').unwrap_or(data);
    let mut lines = data.lines();

    let header = lines.next().ok_or_else(|| ConfigurationError::ParseError {
        file: "ETS CSV".to_string(),
        reason: "empty file".to_string(),
    })?;

    let delimiter = if header.contains('\t') { '\t' } else { ';' };

    let columns: Vec<&str> = header.split(delimiter).collect();
    let name_col = find_column(&columns, &["Group name", "Name", "Gruppenname", "Sub"]);
    let addr_col = find_column(&columns, &["Address", "Adresse", "Group Address"]);
    let dpt_col = find_column(
        &columns,
        &["DatapointType", "Datapoint Type", "DPT", "Datenpunkttyp"],
    );
    let desc_col = find_column(&columns, &["Description", "Beschreibung"]);

    if addr_col.is_none() {
        log_config!(LogLevel::Warn, "ETS CSV: no Address column found in header");
    }
    let addr_col = addr_col.ok_or_else(|| ConfigurationError::ParseError {
        file: "ETS CSV".to_string(),
        reason: "no Address column found".to_string(),
    })?;

    let mut map = HashMap::new();

    for line in lines {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }

        let cols: Vec<&str> = line.split(delimiter).collect();

        let addr_str = cols
            .get(addr_col)
            .map_or("", |s| s.trim_matches('"').trim());
        let Some(address) = parse_group_address(addr_str) else {
            continue;
        };

        let dpt_str = dpt_col
            .and_then(|i| cols.get(i))
            .map_or("", |s| s.trim_matches('"').trim());

        let name = name_col
            .and_then(|i| cols.get(i))
            .map(|s| s.trim_matches('"').trim().to_string())
            .unwrap_or_default();

        let Some(dpt) = parse_dpt_string(dpt_str) else {
            if !dpt_str.is_empty() {
                log_config!(
                    LogLevel::Trace,
                    "ETS CSV: skipping '{}' ({}): unknown DPT '{}'",
                    name,
                    addr_str,
                    dpt_str
                );
            }
            continue;
        };

        let description = desc_col
            .and_then(|i| cols.get(i))
            .map(|s| s.trim_matches('"').trim())
            .filter(|s| !s.is_empty())
            .map(std::string::ToString::to_string);

        map.insert(
            address,
            GroupAddressConfig {
                dpt,
                name,
                description,
            },
        );
    }

    log_config!(
        LogLevel::Info,
        "ETS CSV loaded: {} group addresses parsed",
        map.len()
    );
    Ok(map)
}

fn find_column(columns: &[&str], names: &[&str]) -> Option<usize> {
    for name in names {
        for (i, col) in columns.iter().enumerate() {
            if col.trim_matches('"').trim().eq_ignore_ascii_case(name) {
                return Some(i);
            }
        }
    }
    None
}

fn parse_group_address(s: &str) -> Option<GroupAddress> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() == 3 {
        let main: u8 = parts[0].parse().ok()?;
        let middle: u8 = parts[1].parse().ok()?;
        let sub: u8 = parts[2].parse().ok()?;
        GroupAddress::from_parts(main, middle, sub).ok()
    } else {
        None
    }
}

/// Parse DPT from various ETS formats:
/// - "DPST-9-1" -> `DptType` `from_number(9`, 1)
/// - "DPT-9" -> `DptType` `from_number(9`, 1) (default sub=1)
/// - "9.001" or "9.1" -> `DptType` `from_str`
fn parse_dpt_string(s: &str) -> Option<DptType> {
    if s.is_empty() {
        return None;
    }

    if let Some(rest) = s.strip_prefix("DPST-") {
        let parts: Vec<&str> = rest.split('-').collect();
        let main: u16 = parts.first()?.parse().ok()?;
        let sub: u16 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
        return DptType::from_number(main, sub);
    }

    if let Some(rest) = s.strip_prefix("DPT-") {
        let main: u16 = rest.parse().ok()?;
        return DptType::from_number(main, 1);
    }

    DptType::from_str(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ets5_semicolon_format() {
        let csv = "\"Group name\";\"Address\";\"Central\";\"Unfiltered\";\"Description\";\"DatapointType\";\"Security\"\n\
                   \"Living Room Temp\";\"1/2/3\";\"\";\"\";\"\";\"DPST-9-1\";\"\"\n\
                   \"Hall Light\";\"1/2/4\";\"\";\"\";\"\";\"DPST-1-1\";\"\"\n";
        let map = parse_ets_csv(csv).unwrap();
        assert_eq!(map.len(), 2);
        let temp = map
            .get(&GroupAddress::from_parts(1, 2, 3).unwrap())
            .unwrap();
        assert_eq!(temp.dpt, DptType::Temperature);
        assert_eq!(temp.name, "Living Room Temp");
    }

    #[test]
    fn test_parse_ets5_with_description() {
        let csv = "\"Group name\";\"Address\";\"Description\";\"DatapointType\"\n\
                   \"Living Room Temp\";\"1/2/3\";\"Temperature sensor\";\"DPST-9-1\"\n";
        let map = parse_ets_csv(csv).unwrap();
        let temp = map
            .get(&GroupAddress::from_parts(1, 2, 3).unwrap())
            .unwrap();
        assert_eq!(temp.description.as_deref(), Some("Temperature sensor"));
    }

    #[test]
    fn test_parse_ets6_tab_format() {
        let csv = "Name\tAddress\tDatapointType\nKitchen Light\t2/1/0\t9.001\n";
        let map = parse_ets_csv(csv).unwrap();
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn test_parse_dpt_formats() {
        assert_eq!(parse_dpt_string("DPST-9-1"), Some(DptType::Temperature));
        assert_eq!(parse_dpt_string("9.001"), Some(DptType::Temperature));
        assert_eq!(parse_dpt_string("9.1"), Some(DptType::Temperature));
        assert_eq!(parse_dpt_string("DPST-1-1"), Some(DptType::Switch));
        assert_eq!(parse_dpt_string("DPT-9"), Some(DptType::Temperature));
        assert_eq!(parse_dpt_string(""), None);
    }

    #[test]
    fn test_bom_handling() {
        let csv = "\u{FEFF}\"Group name\";\"Address\";\"DatapointType\"\n\"Test\";\"1/0/1\";\"DPST-1-1\"\n";
        let map = parse_ets_csv(csv).unwrap();
        assert_eq!(map.len(), 1);
    }
}
