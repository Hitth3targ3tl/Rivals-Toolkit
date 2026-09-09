//! INI parsing and CVar edit application for pak-embedded config files.

use super::{PakCvar, PakTweakEdit};

#[derive(Clone, Copy)]
pub(super) enum IniType {
    Engine,
    DeviceProfiles,
}

/// Apply edits to INI content.
pub(super) fn apply_edits_to_ini(
    content: &str,
    edits: &[PakTweakEdit],
    ini_type: IniType,
) -> String {
    let mut lines: Vec<String> = content.lines().map(String::from).collect();

    match ini_type {
        IniType::DeviceProfiles => {
            apply_device_profiles_edits(&mut lines, edits);
        }
        IniType::Engine => {
            apply_engine_edits(&mut lines, edits);
        }
    }

    let mut result = lines.join("\r\n");
    if !result.ends_with("\r\n") {
        result.push_str("\r\n");
    }
    result
}

/// Parse CVar key/value lines from Engine or DeviceProfiles INI content.
pub(super) fn parse_console_vars(content: &str, source: &str) -> Vec<PakCvar> {
    let mut vars = Vec::new();
    let is_device_profiles = source.contains("DeviceProfiles");

    if is_device_profiles {
        let mut in_section = false;
        for line in content.lines() {
            let trimmed = line.trim();

            if trimmed.starts_with('[') {
                in_section = is_windows_device_profile_header(trimmed);
                continue;
            }

            if !in_section || trimmed.is_empty() || trimmed.starts_with(';') {
                continue;
            }

            if let Some(kv) = parse_cvar_line(trimmed) {
                vars.push(PakCvar {
                    key: kv.0,
                    value: kv.1,
                    source: source.to_string(),
                });
            }
        }
    } else {
        // Engine.ini keys can be outside [ConsoleVariables], so scan all sections.
        let mut in_any_section = false;
        for line in content.lines() {
            let trimmed = line.trim();

            if trimmed.starts_with('[') {
                in_any_section = true;
                continue;
            }

            if !in_any_section || trimmed.is_empty() || trimmed.starts_with(';') {
                continue;
            }

            if let Some(kv) = parse_cvar_line(trimmed) {
                vars.push(PakCvar {
                    key: kv.0,
                    value: kv.1,
                    source: source.to_string(),
                });
            }
        }
    }
    vars
}

/// Parse one CVar line, supporting optional `+CVars=` prefix.
fn parse_cvar_line(line: &str) -> Option<(String, String)> {
    let inner = if line.to_ascii_lowercase().starts_with("+cvars=") {
        &line["+CVars=".len()..]
    } else {
        line
    };

    let (key, value) = inner.split_once('=')?;
    let key = key.trim();
    let value = value.trim();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), value.to_string()))
}

/// Check whether a section header is a Windows device profile, i.e. `[Windows DeviceProfile]`
/// or one of the profiles that inherit from it (`WindowsClient`, `WindowsNoEditor`, ...).
///
/// The shipping client runs the `Windows` profile, but a config mod can park a CVar in any of
/// its siblings and it still applies, so all of them have to be visible to reads and edits.
fn is_windows_device_profile_header(header: &str) -> bool {
    let Some(inner) = header
        .trim()
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return false;
    };
    let lower = inner.trim().to_ascii_lowercase();
    lower
        .strip_suffix(" deviceprofile")
        .is_some_and(|name| name.starts_with("windows"))
}

/// Check whether a section header is the plain `[Windows DeviceProfile]` section, the one a
/// brand-new CVar belongs in.
fn is_primary_device_profile_header(header: &str) -> bool {
    header
        .trim()
        .eq_ignore_ascii_case("[Windows DeviceProfile]")
}

/// Remove non-comment CVar lines whose key matches `key_lower`.
fn remove_cvar_key(lines: &mut Vec<String>, key_lower: &str) {
    lines.retain(|line| {
        let t = line.trim();
        if t.starts_with(';') {
            return true;
        }
        match parse_cvar_line(t) {
            Some((k, _)) => k.to_ascii_lowercase() != key_lower,
            None => true,
        }
    });
}

/// Format a CVar assignment line.
fn format_cvar_line(key: &str, val: &str, preserve_prefix: bool) -> String {
    if preserve_prefix {
        format!("+CVars={}={}", key, val)
    } else {
        format!("{}={}", key, val)
    }
}

/// Find the end of a section (next header or EOF).
fn find_section_end(lines: &[String], section_start: usize) -> usize {
    for (i, line) in lines.iter().enumerate().skip(section_start + 1) {
        if line.trim().starts_with('[') {
            return i;
        }
    }
    lines.len()
}

/// Find an insert point near the end of a section, before trailing blank lines.
fn find_section_insert_point(lines: &[String], section_start: usize) -> usize {
    let end = find_section_end(lines, section_start);
    let mut insert = end;
    while insert > section_start + 1 && lines[insert - 1].trim().is_empty() {
        insert -= 1;
    }
    insert
}

/// Line ranges of every Windows device profile section, as `(header index, end)`.
///
/// Combo paks concatenate several config mods, so a header can appear more than once, and the
/// same key can sit under `[Windows DeviceProfile]` and `[WindowsClient DeviceProfile]` at once.
fn device_profile_sections(lines: &[String]) -> Vec<(usize, usize)> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_windows_device_profile_header(line.trim()))
        .map(|(i, _)| (i, find_section_end(lines, i)))
        .collect()
}

/// Header index of the last plain `[Windows DeviceProfile]` section.
fn primary_section_start(lines: &[String]) -> Option<usize> {
    lines
        .iter()
        .rposition(|line| is_primary_device_profile_header(line))
}

/// Header index of the device profile section containing `line_index`.
fn enclosing_section_start(sections: &[(usize, usize)], line_index: usize) -> Option<usize> {
    sections
        .iter()
        .find(|(start, end)| line_index > *start && line_index < *end)
        .map(|(start, _)| *start)
}

/// Every line under a Windows device profile header that sets `key_lower`, in file order.
fn device_profile_key_hits(lines: &[String], key_lower: &str) -> Vec<usize> {
    let mut hits = Vec::new();
    for (start, end) in device_profile_sections(lines) {
        for (offset, line) in lines[start + 1..end].iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with(';') {
                continue;
            }
            if let Some((k, _)) = parse_cvar_line(trimmed)
                && k.to_ascii_lowercase() == key_lower
            {
                hits.push(start + 1 + offset);
            }
        }
    }
    hits
}

/// Apply edits across every Windows device profile section.
///
/// A removal clears every occurrence: a copy left behind in `[WindowsClient DeviceProfile]` or
/// in a second `[Windows DeviceProfile]` block still applies at runtime, so one survivor makes
/// the tweak a no-op. A set collapses the occurrences into a single line in the plain
/// `[Windows DeviceProfile]` section that the others inherit from, so repeated saves cannot
/// grow the file.
fn apply_device_profiles_edits(lines: &mut Vec<String>, edits: &[PakTweakEdit]) {
    for edit in edits {
        let key_lower = edit.key.to_ascii_lowercase();
        let hits = device_profile_key_hits(lines, &key_lower);

        // A pure removal never creates a section.
        let Some(val) = edit.value.as_deref() else {
            for &i in hits.iter().rev() {
                lines.remove(i);
            }
            continue;
        };

        let sections = device_profile_sections(lines);
        let anchor_start = primary_section_start(lines).or_else(|| {
            hits.last()
                .and_then(|&i| enclosing_section_start(&sections, i))
        });
        let anchor_hit = anchor_start.and_then(|start| {
            let end = find_section_end(lines, start);
            hits.iter().rev().copied().find(|&i| i > start && i < end)
        });

        match anchor_hit {
            // Rewrite in place so a set never reorders the file, then drop the copies that
            // would shadow it.
            Some(keep) => {
                let has_prefix = lines[keep]
                    .trim()
                    .to_ascii_lowercase()
                    .starts_with("+cvars=");
                lines[keep] = format_cvar_line(&edit.key, val, has_prefix);
                for &i in hits.iter().rev() {
                    if i != keep {
                        lines.remove(i);
                    }
                }
            }
            None => {
                let anchor_header = anchor_start.map(|start| lines[start].trim().to_string());
                for &i in hits.iter().rev() {
                    lines.remove(i);
                }
                let existing = anchor_header.and_then(|header| {
                    lines
                        .iter()
                        .rposition(|line| line.trim().eq_ignore_ascii_case(&header))
                });
                let start = match existing {
                    Some(start) => start,
                    None => {
                        if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                            lines.push(String::new());
                        }
                        lines.push("[Windows DeviceProfile]".to_string());
                        lines.len() - 1
                    }
                };
                let insert_at = find_section_insert_point(lines, start);
                lines.insert(insert_at, format_cvar_line(&edit.key, val, true));
            }
        }
    }
}

/// Apply edits to Engine.ini.
///
/// Existing keys are updated in place. New keys are inserted into `engine_section`
/// when provided, otherwise into `[ConsoleVariables]`.
fn apply_engine_edits(lines: &mut Vec<String>, edits: &[PakTweakEdit]) {
    for edit in edits {
        let key_lower = edit.key.to_ascii_lowercase();

        let mut in_section = false;
        let mut found_idx: Option<usize> = None;
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_section = true;
                continue;
            }
            if !in_section || trimmed.is_empty() || trimmed.starts_with(';') {
                continue;
            }
            if let Some((k, _)) = parse_cvar_line(trimmed)
                && k.to_ascii_lowercase() == key_lower
            {
                found_idx = Some(i);
                break;
            }
        }

        match (&edit.value, found_idx) {
            (Some(val), Some(_)) => {
                let new_line = format_cvar_line(&edit.key, val, false);
                for line in lines.iter_mut() {
                    let t = line.trim();
                    if t.starts_with(';') {
                        continue;
                    }
                    if let Some((k, _)) = parse_cvar_line(t)
                        && k.to_ascii_lowercase() == key_lower
                    {
                        *line = new_line.clone();
                    }
                }
            }
            (None, Some(_)) => {
                remove_cvar_key(lines, &key_lower);
            }
            (None, None) => {}
            (Some(val), None) => {
                let target_header = edit
                    .engine_section
                    .as_deref()
                    .map(|s| format!("[{}]", s))
                    .unwrap_or_else(|| "[ConsoleVariables]".to_string());

                let section_start = lines
                    .iter()
                    .rposition(|l| l.trim().eq_ignore_ascii_case(&target_header));

                let section_start = match section_start {
                    Some(idx) => idx,
                    None => {
                        if !lines.last().is_some_and(|l| l.trim().is_empty()) {
                            lines.push(String::new());
                        }
                        lines.push(target_header);
                        lines.len() - 1
                    }
                };

                let insert_at = find_section_insert_point(lines, section_start);
                lines.insert(insert_at, format_cvar_line(&edit.key, val, false));
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod device_profile_tests {
    use super::*;

    /// A combo pak: several mods' configs concatenated, so the profile header and the keys under it
    /// both appear more than once.
    fn combo() -> String {
        [
            "[Windows DeviceProfile]",
            "+CVars=r.PostProcessing.DisableMaterials=1",
            "+CVars=r.CustomDepth=0",
            "",
            "[SomeOtherSection]",
            "Unrelated=1",
            "",
            "[Windows DeviceProfile]",
            "+CVars=r.PostProcessing.DisableMaterials=1",
            "+CVars=r.CustomDepth=0",
            "",
        ]
        .join("\r\n")
    }

    fn edit(key: &str, value: Option<&str>) -> PakTweakEdit {
        PakTweakEdit {
            key: key.to_string(),
            value: value.map(str::to_string),
            engine_section: None,
        }
    }

    fn value_of(content: &str, key: &str) -> Option<String> {
        parse_console_vars(content, "DefaultDeviceProfiles.ini")
            .into_iter()
            .rfind(|v| v.key.eq_ignore_ascii_case(key))
            .map(|v| v.value)
    }

    #[test]
    fn removal_clears_the_key_in_every_section() {
        let out = apply_edits_to_ini(
            &combo(),
            &[edit("r.PostProcessing.DisableMaterials", None)],
            IniType::DeviceProfiles,
        );
        assert_eq!(value_of(&out, "r.PostProcessing.DisableMaterials"), None);
        assert!(!out.to_ascii_lowercase().contains("disablematerials"));
        // The untouched key survives in both sections.
        assert_eq!(out.matches("r.CustomDepth=0").count(), 2);
    }

    #[test]
    fn set_collapses_duplicates_to_a_single_line() {
        let out = apply_edits_to_ini(
            &combo(),
            &[edit("r.CustomDepth", Some("3"))],
            IniType::DeviceProfiles,
        );
        assert_eq!(value_of(&out, "r.CustomDepth").as_deref(), Some("3"));
        assert_eq!(out.matches("r.CustomDepth=").count(), 1);
        assert!(!out.contains("r.CustomDepth=0"));
    }

    /// The bug that grew the user's INI: with the key never found, every save appended another line.
    #[test]
    fn repeated_sets_do_not_grow_the_file() {
        let once = apply_edits_to_ini(
            &combo(),
            &[edit("r.CustomDepth", Some("3"))],
            IniType::DeviceProfiles,
        );
        let twice = apply_edits_to_ini(
            &once,
            &[edit("r.CustomDepth", Some("3"))],
            IniType::DeviceProfiles,
        );
        assert_eq!(once, twice);
    }

    #[test]
    fn a_set_lands_where_a_read_will_find_it() {
        let out = apply_edits_to_ini(
            &combo(),
            &[edit("r.NewKey", Some("7"))],
            IniType::DeviceProfiles,
        );
        assert_eq!(value_of(&out, "r.NewKey").as_deref(), Some("7"));
    }

    #[test]
    fn a_removal_never_creates_a_section() {
        let out = apply_edits_to_ini(
            "[SomeOtherSection]\r\nUnrelated=1\r\n",
            &[edit("r.CustomDepth", None)],
            IniType::DeviceProfiles,
        );
        assert!(!out.contains("[Windows DeviceProfile]"));
    }
}
