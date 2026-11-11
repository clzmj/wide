use std::process::Command;
fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_wide"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn ordinary_help_keeps_diagnostics_under_advanced() {
    let result = cli(&["--help"]);
    assert!(result.status.success());
    let help = String::from_utf8(result.stdout).unwrap().to_lowercase();
    assert!(help.contains("wide"));
    assert!(help.contains("status"));
    assert!(help.contains("advanced"));
    for internal in [
        "ddc",
        "vcp",
        "--confirm",
        "--allow-undocumented",
        "--mock",
        "--display",
        "--json",
    ] {
        assert!(!help.contains(internal), "ordinary help exposed {internal}");
    }
    let advanced = cli(&["advanced", "--help"]);
    assert!(advanced.status.success());
    assert!(String::from_utf8_lossy(&advanced.stdout).contains("ddc"));
    assert!(!cli(&["ddc", "read", "0x60"]).status.success());
}

#[test]
fn friendly_plans_are_available_without_writes_and_conflicting_ports_are_rejected() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/layout-pip-enabled.json"
    );
    for choice in [
        "fullscreen",
        "split-left",
        "split-right",
        "pip-big",
        "pip-small",
    ] {
        let result = cli(&["--json", "--mock", fixture, "advanced", "plan", choice]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["dry_run"], true);
        assert_eq!(value["plan"]["this_source"], "hdmi1");
    }
    let result = cli(&[
        "--json",
        "--mock",
        fixture,
        "advanced",
        "plan",
        "split-left",
        "--port",
        "dp",
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("conflicts"));
    let result = cli(&[
        "--json",
        "--mock",
        "/does/not/exist",
        "advanced",
        "apply",
        "fullscreen",
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("requires --confirm"));
}

#[test]
fn the_hdmi_mac_stays_hdmi_when_the_monitors_main_input_is_displayport() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/layout-split-right-hdmi.json"
    );
    let result = cli(&["--json", "--mock", fixture, "status"]);
    assert!(result.status.success());
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["connection"], "HDMI1");
    assert_eq!(value["state"]["input"], "display_port");
    assert!(value["layout"].as_str().unwrap().contains("right"));
    let result = cli(&[
        "--json",
        "--mock",
        fixture,
        "advanced",
        "plan",
        "fullscreen",
    ]);
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["plan"]["this_source"], "hdmi1");
    assert_eq!(value["plan"]["target_main"], "hdmi1");
    assert_eq!(
        value["plan"]["steps"],
        serde_json::json!(["swap_sources", "disable"])
    );
}

#[test]
fn audio_only_sync_never_changes_layout_or_resolution() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/layout-pip-enabled.json"
    );
    for confirm in [false, true] {
        let mut args = vec!["--json", "--mock", fixture, "advanced", "sound"];
        if confirm {
            args.push("--confirm");
        }
        let result = cli(&args);
        assert!(result.status.success());
        let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["layout_untouched"], true);
        assert_eq!(value["resolution_untouched"], true);
        if confirm {
            assert_eq!(value["audio"]["logical_role"], "secondary");
        } else {
            assert_eq!(value["dry_run"], true);
            assert_eq!(value["setting_writes"], 0);
        }
    }
}

#[test]
fn default_form_requires_a_terminal_and_json_requires_a_command() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mock.json");
    // Command::output closes stdin and pipes stderr; the form must return instead of waiting.
    let result = cli(&["--mock", fixture]);
    assert!(!result.status.success());
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(message.contains("interactive terminal"));
    assert!(message.contains("wide advanced --help"));
    let result = cli(&["--json", "--mock", "/does/not/exist"]);
    assert!(!result.status.success());
    let data: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(data["error"].as_str().unwrap().contains("explicit command"));
}
#[test]
fn offline_cli_and_mutation_boundary() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mock.json");
    let result = cli(&[
        "--json", "--mock", fixture, "advanced", "ddc", "read", "0x60",
    ]);
    assert!(result.status.success());
    let data: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(data["current"], 5);
    assert_eq!(data["status"], "ok");
    let result = cli(&[
        "--json",
        "--mock",
        fixture,
        "--display",
        "93",
        "advanced",
        "info",
    ]);
    assert!(!result.status.success()); // Numeric selectors never fall through to the G93 name.
                                       // Gate operates even with a nonexistent fixture: no discovery or FFI before refusal.
    for args in [
        vec!["ddc", "write", "0xe2", "3"],
        vec!["mode", "pbp"],
        vec!["preset", "apply", "work"],
    ] {
        let mut call = vec!["--json", "--mock", "/does/not/exist", "advanced"];
        call.extend(args);
        let result = cli(&call);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stdout).contains("No monitor command was sent"));
    }
    let result = cli(&["--mock", fixture, "advanced", "ddc", "scan"]);
    assert!(!result.status.success()); // explicit --read-only is mandatory
}

#[test]
fn pip_enable_requires_both_flags_before_loading_any_fixture() {
    for command in [vec!["mode", "pip"], vec!["ddc", "write", "0xE2", "1"]] {
        for flags in [vec![], vec!["--allow-undocumented"], vec!["--confirm"]] {
            let mut call = vec!["--json", "--mock", "/does/not/exist", "advanced"];
            call.extend(command.iter().copied());
            call.extend(flags);
            let result = cli(&call);
            assert!(!result.status.success());
            assert!(String::from_utf8_lossy(&result.stdout).contains("requires both"));
        }
    }
}

#[test]
fn only_verified_enable_operation_is_available() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/pip-disabled.json"
    );
    for command in [vec!["mode", "pip"], vec!["ddc", "write", "0xe2", "1"]] {
        let mut call = vec!["--json", "--mock", fixture, "advanced"];
        call.extend(command);
        call.extend(["--allow-undocumented", "--confirm"]);
        let result = cli(&call);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        let data: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(data["status"], "verified_readback");
        assert_eq!(data["setting_writes"], 1);
        assert_eq!(data["write"]["request_hex"], "8403e200015b");
        assert_eq!(data["write"]["read_status"], serde_json::Value::Null);
        assert_eq!(data["readback"]["current"], 0x35cf);
        assert_eq!(data["requires_visual_verification"], true);
        assert_eq!(data["mock"], true);
    }
    for command in [
        vec!["mode", "fullscreen"],
        vec!["mode", "pbp"],
        vec!["ddc", "write", "0xe2", "0"],
        vec!["ddc", "write", "0xe2", "0x10"],
        vec!["ddc", "write", "0xe3", "0x10"],
        vec!["ddc", "write", "0x60", "0x11"],
    ] {
        let mut call = vec!["--json", "--mock", "/does/not/exist", "advanced"];
        call.extend(command);
        call.extend(["--allow-undocumented", "--confirm"]);
        let result = cli(&call);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stdout).contains("No monitor command was sent"));
    }
}

#[test]
fn pip_enable_noops_when_enabled_and_rejects_other_saved_layouts() {
    let enabled = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/pip-enabled.json"
    );
    let result = cli(&[
        "--json",
        "--mock",
        enabled,
        "advanced",
        "mode",
        "pip",
        "--allow-undocumented",
        "--confirm",
    ]);
    assert!(result.status.success());
    let data: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(data["status"], "already_enabled");
    assert_eq!(data["setting_writes"], 0);
    assert!(data["write"].is_null());
    for fixture in [
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/pip-wrong-layout.json"
        ),
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mock.json"),
    ] {
        let result = cli(&[
            "--json",
            "--mock",
            fixture,
            "advanced",
            "mode",
            "pip",
            "--allow-undocumented",
            "--confirm",
        ]);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stdout)
            .to_lowercase()
            .contains("no write sent"));
    }
}

#[test]
fn displayport_identity_is_selected_and_scopes_friendly_plans() {
    // Sanitized observed DP identity; register replies are synthetic fixture state.
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/layout-displayport.json"
    );
    let status = cli(&["--json", "--mock", fixture, "status"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stdout)
    );
    let value: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(value["connection"], "DisplayPort");
    for (choice, main) in [
        ("fullscreen", "display_port"),
        ("split-left", "display_port"),
        ("split-right", "hdmi1"),
        ("pip-big", "display_port"),
        ("pip-small", "hdmi1"),
    ] {
        let result = cli(&["--json", "--mock", fixture, "advanced", "plan", choice]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["dry_run"], true);
        assert_eq!(value["plan"]["this_source"], "display_port");
        assert_eq!(value["plan"]["target_main"], main);
    }
    let conflict = cli(&[
        "--json",
        "--mock",
        fixture,
        "advanced",
        "plan",
        "split-left",
        "--port",
        "hdmi1",
    ]);
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stdout).contains("conflicts"));
}
