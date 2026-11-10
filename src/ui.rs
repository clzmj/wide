//! Terminal form only; discovery, planning, validation, and application belong to the caller.
use crate::layout::{LayoutChoice, Source};
use dialoguer::{Confirm, Select};
use serde_json::{json, Value};
use std::io::{self, IsTerminal};

pub struct UiContext {
    pub monitor_name: String,
    pub current_input: String,
    pub current_layout: String,
    pub current_choice: LayoutChoice,
}

pub const RECONNECT_HINT: &str =
    "Unplug and reconnect this Mac’s video cable, then run wide again.";

fn transport_error(error: &str) -> bool {
    [
        "DDC read timed out",
        "DDC request timed out",
        "DDC bus busy",
        "worker still stopping",
        "(transport_error)",
        ": transport_error",
        "The monitor has not reconnected",
        "re-enumerated during a read (NoDevice",
        "IORegistry changed during discovery",
        "IOAVServiceCreateWithService failed",
    ]
    .iter()
    .any(|marker| error.contains(marker))
        || (error.starts_with("Expected exactly one external AV proxy with registry ID ")
            && error.contains("; found 0."))
}

fn contains_transport_error(value: &Value) -> bool {
    match value {
        Value::Object(fields) => {
            fields
                .get("error")
                .and_then(Value::as_str)
                .is_some_and(transport_error)
                || fields.get("status").and_then(Value::as_str) == Some("transport_error")
                || ["write_status", "read_status"].iter().any(|key| {
                    fields
                        .get(*key)
                        .and_then(Value::as_i64)
                        .is_some_and(|status| status != 0)
                })
                || fields.values().any(contains_transport_error)
        }
        Value::Array(values) => values.iter().any(contains_transport_error),
        _ => false,
    }
}

/// Preserve the original failure and raw evidence; add a customer action only
/// for a transport failure, never for an unsupported feature/profile/selection.
pub fn annotate_failure(value: &mut Value) {
    if value["ok"] != true && contains_transport_error(value) {
        value["reconnect_required"] = json!(true);
        value["action_hint"] = json!(RECONNECT_HINT);
    }
}

pub fn failure_message(error: &str, after_mutation: bool) -> String {
    if transport_error(error) || error == RECONNECT_HINT {
        if after_mutation {
            format!("The requested layout may be incomplete. {RECONNECT_HINT}")
        } else {
            RECONNECT_HINT.into()
        }
    } else {
        error.into()
    }
}

/// Call before discovery when entering the default form, so redirected runs fail immediately.
pub fn require_terminal() -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err("The layout form needs an interactive terminal. Run wide in a terminal, or use wide status / wide advanced --help.".into());
    }
    Ok(())
}

pub fn choose(context: &UiContext) -> Result<Option<LayoutChoice>, String> {
    require_terminal()?;
    eprintln!("\nwide · {}", context.monitor_name);
    eprintln!("This Mac: {}", context.current_input);
    eprintln!("Current layout: {}\n", context.current_layout);

    let layout = Select::new()
        .with_prompt("Layout")
        .items(["Full-screen", "Split view", "Picture in picture", "Cancel"])
        .default(match context.current_choice {
            LayoutChoice::Fullscreen => 0,
            LayoutChoice::SplitLeft | LayoutChoice::SplitRight => 1,
            _ => 2,
        })
        .interact_opt()
        .map_err(|error| format!("Could not read your layout choice: {error}"))?;
    let choice = match layout {
        Some(0) => Some(LayoutChoice::Fullscreen),
        Some(1) => Select::new()
            .with_prompt("Where should this Mac appear?")
            .items(["Left half", "Right half"])
            .default(usize::from(
                context.current_choice == LayoutChoice::SplitRight,
            ))
            .interact_opt()
            .map_err(|error| format!("Could not read your side choice: {error}"))?
            .map(|side| {
                if side == 0 {
                    LayoutChoice::SplitLeft
                } else {
                    LayoutChoice::SplitRight
                }
            }),
        Some(2) => Select::new()
            .with_prompt("Where should this Mac appear?")
            .items(["Big picture (background)", "Small picture (inset)"])
            .default(usize::from(
                context.current_choice == LayoutChoice::PipSmall,
            ))
            .interact_opt()
            .map_err(|error| format!("Could not read your picture choice: {error}"))?
            .map(|role| {
                if role == 0 {
                    LayoutChoice::PipBig
                } else {
                    LayoutChoice::PipSmall
                }
            }),
        _ => None,
    };
    Ok(choice)
}

pub fn choose_source() -> Result<Option<Source>, String> {
    require_terminal()?;
    Select::new()
        .with_prompt("Which cable connects this Mac to the monitor?")
        .items(["HDMI 1", "DisplayPort"])
        .default(0)
        .interact_opt()
        .map(|v| {
            v.map(|i| {
                if i == 0 {
                    Source::Hdmi1
                } else {
                    Source::DisplayPort
                }
            })
        })
        .map_err(|e| format!("Could not read the connection choice: {e}"))
}

/// Confirm only after the caller has validated the plan and rendered its resolution preview.
pub fn confirm(summary: &str) -> Result<bool, String> {
    require_terminal()?;
    eprintln!("\nReview layout\n{summary}\n");
    Confirm::new()
        .with_prompt("Apply this layout?")
        .default(false)
        .wait_for_newline(true)
        .interact_opt()
        .map(|answer| answer.unwrap_or(false))
        .map_err(|error| format!("Could not read your confirmation: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_warning_preserves_details_and_does_not_claim_an_initial_change() {
        let error = "DDC read timed out after 1500ms; worker still stopping";
        let mut value = json!({"error":error});
        annotate_failure(&mut value);
        assert_eq!(value["error"], error);
        assert_eq!(value["reconnect_required"], true);
        assert_eq!(value["action_hint"], RECONNECT_HINT);
        assert_eq!(failure_message(error, false), RECONNECT_HINT);
        assert!(failure_message(error, true).contains("may be incomplete"));
    }
    #[test]
    fn unsupported_profiles_and_ambiguous_connections_do_not_suggest_replugging() {
        for value in [
            json!({"error":"Monitor layout reply does not match the supported G93SC profile"}),
            json!({"error":"Found 2 supported monitor connections"}),
            json!({"status":"unsupported","attempts":[{"write_status":0,"read_status":0}]}),
            json!({"ok":true,"old_error":"DDC read timed out"}),
        ] {
            let mut result = value;
            annotate_failure(&mut result);
            assert!(result["reconnect_required"].is_null());
        }
        let mut result = json!({"ok":false,"error":"The monitor write failed","steps":[{"write":{"write_status":-1}}]});
        annotate_failure(&mut result);
        assert_eq!(result["reconnect_required"], true);
    }
}
