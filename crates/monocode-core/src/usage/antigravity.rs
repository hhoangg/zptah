//! Antigravity subscription usage from `agy -p /usage --output-format json`.
//! The slash command runs locally without a model turn and returns structured quota data.
use super::{FetchError, Snapshot, Window};
use crate::accounts::Profile;
use crate::provider::{self, Kind};
use serde::{Deserialize, Deserializer};

/// `--print-timeout` bounds agy itself; `cli_output` also kills the process tree after 15 s.
const ARGS: [&str; 6] = [
    "-p",
    "/usage",
    "--output-format",
    "json",
    "--print-timeout",
    "10s",
];

/// The model family of an Antigravity usage group, read from its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GroupFamily {
    /// Antigravity's own Gemini models.
    Gemini,
    /// Claude and GPT models.
    Claude,
}
pub fn group_family(name: &str) -> Option<GroupFamily> {
    let name = name.to_lowercase();
    if name.contains("gemini") {
        Some(GroupFamily::Gemini)
    } else if name.contains("claude") || name.contains("gpt") {
        Some(GroupFamily::Claude)
    } else {
        None
    }
}

pub(super) fn fetch(profile: &Profile) -> Result<Snapshot, FetchError> {
    let binary = provider::resolve(Kind::Antigravity).ok_or_else(|| {
        FetchError::unavailable("Install the agy CLI to check Antigravity usage.")
    })?;
    if provider::is_legacy_antigravity_server(&binary) {
        return Err(FetchError::unavailable(
            "The legacy Antigravity ACP server has no usage command. Install the agy CLI to check usage.",
        ));
    }
    let output = crate::accounts::login::binary_output(Kind::Antigravity, binary, &ARGS, profile)
        .map_err(FetchError::failed)?;
    parse(&output, super::now_ms())
}

/// agy is written in Go, which encodes a nil slice or empty value as `null`; treat that as
/// the default instead of rejecting the whole output.
fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Deserialize)]
struct Output {
    #[serde(default, deserialize_with = "nullable")]
    status: String,
    command: Option<Command>,
}
#[derive(Deserialize)]
struct Command {
    #[serde(default, deserialize_with = "nullable")]
    name: String,
    data: Option<Data>,
}
#[derive(Deserialize)]
struct Data {
    #[serde(default, deserialize_with = "nullable")]
    groups: Vec<Group>,
}
#[derive(Deserialize)]
struct Group {
    #[serde(default, deserialize_with = "nullable")]
    name: String,
    #[serde(default, deserialize_with = "nullable")]
    buckets: Vec<Bucket>,
}
#[derive(Deserialize)]
struct Bucket {
    #[serde(default, deserialize_with = "nullable")]
    id: String,
    #[serde(default, deserialize_with = "nullable")]
    window: String,
    remaining_fraction: Option<f64>,
    reset_time: Option<String>,
}

/// Parses only `command.data`; the human-readable `response` text is ignored.
fn parse(output: &str, now: i64) -> Result<Snapshot, FetchError> {
    if output.trim().is_empty() {
        return Err(FetchError::unavailable(
            "agy returned no output. Sign in with agy, then refresh usage.",
        ));
    }
    let output: Output = serde_json::from_str(output.trim()).map_err(|_| {
        FetchError::unavailable("agy did not return usage as JSON. Update the agy CLI.")
    })?;
    if output.status != "SUCCESS" {
        return Err(FetchError::unavailable(format!(
            "agy could not read usage (status {}). Sign in with agy, then refresh usage.",
            if output.status.is_empty() {
                "missing"
            } else {
                &output.status
            }
        )));
    }
    let data = output
        .command
        .filter(|command| command.name == "usage")
        .and_then(|command| command.data)
        .ok_or_else(|| FetchError::unavailable("agy returned no usage data."))?;
    let mut groups = data.groups;
    // Gemini (Antigravity's own models) first, then Claude and GPT, then anything new.
    groups.sort_by_key(|group| group_family(&group.name).map_or(2, |family| family as u8));
    let mut windows = Vec::new();
    for group in groups {
        let mut group_windows: Vec<Window> = group
            .buckets
            .iter()
            .filter_map(|bucket| window(bucket, &group.name))
            .collect();
        group_windows.sort_by_key(|w| w.minutes);
        windows.extend(group_windows);
    }
    if windows.is_empty() {
        return Err(FetchError::unavailable(
            "agy reported no Antigravity usage limits.",
        ));
    }
    Ok(Snapshot {
        windows,
        updated_at: now,
        ..Default::default()
    })
}

/// Maps a bucket by its machine-readable `window` (or the `id` suffix), never its display name.
fn window(bucket: &Bucket, group: &str) -> Option<Window> {
    let kind = if bucket.window.is_empty() {
        bucket.id.rsplit('-').next().unwrap_or("")
    } else {
        bucket.window.as_str()
    };
    let minutes = match kind {
        "5h" => 300,
        "weekly" => 10_080,
        _ => return None,
    };
    let remaining = bucket.remaining_fraction.filter(|n| n.is_finite())?;
    Some(Window {
        used_percent: ((1. - remaining) * 100.).clamp(0., 100.),
        minutes,
        resets_at: bucket.reset_time.as_deref().and_then(reset_ms),
        group: (!group.is_empty()).then(|| group.to_string()),
    })
}

/// agy reports `1970-01-01T00:00:00Z` for a window that has not started; that is no reset time.
fn reset_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.timestamp_millis())
        .filter(|ms| *ms > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_764_000_000_000;
    const TWO_GROUPS: &str = r#"{"conversation_id":"","status":"SUCCESS","response":"Gemini 99% left","duration_seconds":0,"num_turns":0,
 "command":{"name":"usage","data":{"groups":[
  {"name":"Gemini Models","buckets":[
    {"id":"gemini-weekly","window":"weekly","remaining_fraction":0.9947,"reset_time":"2026-12-01T07:01:59Z"},
    {"id":"gemini-5h","window":"5h","remaining_fraction":1,"reset_time":"2026-11-25T19:26:33Z"}]},
  {"name":"Claude and GPT models","buckets":[
    {"id":"3p-weekly","window":"weekly","remaining_fraction":0.8348,"reset_time":"2026-12-01T13:31:31Z"},
    {"id":"3p-5h","window":"5h","remaining_fraction":1,"reset_time":"1970-01-01T00:00:00Z"}]}]}}}"#;

    fn ms(rfc3339: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn parses_every_group_with_its_windows() {
        let snapshot = parse(TWO_GROUPS, NOW).unwrap();
        assert_eq!(snapshot.updated_at, NOW);
        let summary: Vec<_> = snapshot
            .windows
            .iter()
            .map(|w| (w.group.as_deref().unwrap(), w.minutes))
            .collect();
        assert_eq!(
            summary,
            [
                ("Gemini Models", 300),
                ("Gemini Models", 10_080),
                ("Claude and GPT models", 300),
                ("Claude and GPT models", 10_080),
            ]
        );
        let gemini_week = &snapshot.windows[1];
        assert!((gemini_week.used_percent - 0.53).abs() < 1e-9);
        assert_eq!(gemini_week.resets_at, Some(ms("2026-12-01T07:01:59Z")));
        assert_eq!(snapshot.windows[0].used_percent, 0.);
        assert_eq!(
            snapshot.windows[0].resets_at,
            Some(ms("2026-11-25T19:26:33Z"))
        );
        assert!((snapshot.windows[3].used_percent - 16.52).abs() < 1e-9);
    }

    #[test]
    fn gemini_comes_before_claude_whatever_the_output_order() {
        let snapshot = parse(
            r#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[
              {"name":"New models","buckets":[{"id":"n-5h","window":"5h","remaining_fraction":1}]},
              {"name":"Claude and GPT models","buckets":[{"id":"3p-5h","window":"5h","remaining_fraction":1}]},
              {"name":"Gemini Models","buckets":[{"id":"gemini-5h","window":"5h","remaining_fraction":1}]}]}}}"#,
            NOW,
        )
        .unwrap();
        let order: Vec<_> = snapshot
            .windows
            .iter()
            .map(|w| w.group.as_deref().unwrap())
            .collect();
        assert_eq!(
            order,
            ["Gemini Models", "Claude and GPT models", "New models"]
        );
    }

    #[test]
    fn the_1970_sentinel_means_no_reset_time() {
        let snapshot = parse(TWO_GROUPS, NOW).unwrap();
        let unstarted = &snapshot.windows[2];
        assert_eq!(unstarted.group.as_deref(), Some("Claude and GPT models"));
        assert_eq!(unstarted.minutes, 300);
        assert_eq!(unstarted.resets_at, None);
        assert_eq!(reset_ms("1970-01-01T00:00:00Z"), None);
        assert_eq!(reset_ms("not a time"), None);
    }

    #[test]
    fn a_status_other_than_success_is_unavailable() {
        let error = parse(
            r#"{"status":"ERROR","response":"Please sign in","command":{"name":"usage","data":{"groups":[]}}}"#,
            NOW,
        )
        .unwrap_err();
        assert!(error.unavailable);
        assert!(error.message.contains("status ERROR"), "{}", error.message);
        let missing = parse(r#"{"response":"x"}"#, NOW).unwrap_err();
        assert!(missing.unavailable);
        assert!(
            missing.message.contains("status missing"),
            "{}",
            missing.message
        );
    }

    #[test]
    fn missing_or_unexpected_data_is_unavailable() {
        for output in [
            r#"{"status":"SUCCESS"}"#,
            r#"{"status":"SUCCESS","command":{"name":"usage"}}"#,
            r#"{"status":"SUCCESS","command":{"name":"help","data":{"groups":[]}}}"#,
        ] {
            let error = parse(output, NOW).unwrap_err();
            assert!(error.unavailable, "{output}");
            assert_eq!(error.message, "agy returned no usage data.", "{output}");
        }
        let empty = parse(
            r#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[{"name":"G","buckets":[{"id":"g-daily","window":"daily","remaining_fraction":1}]}]}}}"#,
            NOW,
        )
        .unwrap_err();
        assert!(empty.unavailable);
        assert!(empty.message.contains("no Antigravity usage limits"));
        let text = parse("Models & Quota\n  Weekly Limit Remaining 96%", NOW).unwrap_err();
        assert!(text.unavailable);
        assert!(text.message.contains("JSON"));
    }

    #[test]
    fn null_values_are_defaults_and_empty_output_is_unavailable() {
        let snapshot = parse(
            r#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[
              {"name":null,"buckets":null},
              {"name":"G","buckets":[{"id":"g-weekly","window":"weekly","remaining_fraction":0.5,"reset_time":null}]}]}}}"#,
            NOW,
        )
        .unwrap();
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].group.as_deref(), Some("G"));
        assert_eq!(snapshot.windows[0].used_percent, 50.);
        let error = parse(
            r#"{"status":"ERROR","command":{"name":"usage","data":{"groups":null}}}"#,
            NOW,
        )
        .unwrap_err();
        assert!(error.message.contains("status ERROR"), "{}", error.message);
        let empty = parse("  \n", NOW).unwrap_err();
        assert!(empty.unavailable);
        assert!(empty.message.contains("no output"), "{}", empty.message);
    }

    #[test]
    fn a_bucket_without_a_window_falls_back_to_its_id() {
        let snapshot = parse(
            r#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[{"name":"","buckets":[{"id":"x-5h","remaining_fraction":0.25}]}]}}}"#,
            NOW,
        )
        .unwrap();
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].minutes, 300);
        assert_eq!(snapshot.windows[0].used_percent, 75.);
        assert_eq!(snapshot.windows[0].group, None);
        assert_eq!(snapshot.windows[0].resets_at, None);
    }

    /// Points `MONOCODE_ANTIGRAVITY_BINARY` at `path` for one test and restores it after.
    /// Only Antigravity resolution reads this variable.
    struct Binary(Option<std::ffi::OsString>);
    const BINARY_KEY: &str = "MONOCODE_ANTIGRAVITY_BINARY";
    impl Binary {
        fn set(path: &std::path::Path) -> Self {
            let previous = std::env::var_os(BINARY_KEY);
            unsafe { std::env::set_var(BINARY_KEY, path) };
            Self(previous)
        }
    }
    impl Drop for Binary {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => unsafe { std::env::set_var(BINARY_KEY, value) },
                None => unsafe { std::env::remove_var(BINARY_KEY) },
            }
        }
    }

    /// One test owns the variable so parallel test threads never see another test's value.
    #[test]
    fn fetch_with_dispatches_to_antigravity() {
        let result = {
            let _binary = Binary::set(std::path::Path::new("/nonexistent/agy_acp_server.par"));
            super::super::fetch_with(Kind::Antigravity, &Profile::default(), None, false)
        };
        let error = result.unwrap_err();
        assert!(error.unavailable);
        assert!(error.message.contains("legacy"), "{}", error.message);

        // A fixture agy that records its arguments and prints the usage JSON.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let binary = dir.path().join("agy");
            let args = dir.path().join("args");
            let json = dir.path().join("usage.json");
            std::fs::write(&json, TWO_GROUPS.replace('\n', " ")).unwrap();
            std::fs::write(
                &binary,
                format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat '{}'\n",
                    args.display(),
                    json.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
            let result = {
                let _binary = Binary::set(&binary);
                super::super::fetch_with(Kind::Antigravity, &Profile::default(), None, false)
            };
            let snapshot = result.unwrap();
            assert_eq!(snapshot.windows.len(), 4);
            assert_eq!(
                snapshot.windows[2].group.as_deref(),
                Some("Claude and GPT models")
            );
            assert_eq!(
                std::fs::read_to_string(&args)
                    .unwrap()
                    .lines()
                    .collect::<Vec<_>>(),
                ARGS
            );
        }
    }
}
