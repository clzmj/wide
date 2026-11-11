mod app;
mod audio;
mod cli;
mod diagnostics;
mod display;
mod layout;
mod presets;
mod protocol;
mod recovery;
mod resolution;
mod transport;
mod ui;

use clap::Parser;
use cli::{AdvancedCommand, Cli, Command, Ddc, Preset};
use layout::{LayoutChoice, MonitorState, Plan, Source};
use serde_json::{json, Value};
use std::{fs, path::Path};
use transport::Transport;

fn journal_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".journal.jsonl");
    name.into()
}

fn save(path: &Path, data: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Do not silently overwrite previous experimental evidence.
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(
        serde_json::to_string_pretty(data)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )
    .map_err(|e| e.to_string())
}

/// Refuse every unverified mutation before fixture access, discovery, or native calls.
fn pip_write_requested(command: &AdvancedCommand) -> Result<bool, String> {
    let flags = match command {
        AdvancedCommand::Mode {
            mode,
            allow_undocumented,
            confirm,
        } if mode == "pip" => Some((*allow_undocumented, *confirm)),
        AdvancedCommand::Ddc {
            command:
                Ddc::Write {
                    vcp,
                    value,
                    allow_undocumented,
                    confirm,
                },
        } => {
            let requested = protocol::number(vcp)
                .and_then(|code| {
                    let code =
                        u8::try_from(code).map_err(|_| "VCP code must be 0..255".to_string())?;
                    Ok((code, protocol::number(value)?))
                })
                .map_err(|error| format!("{error}. No monitor command was sent."))?;
            if !protocol::WRITE_ALLOWLIST.contains(&requested) {
                return Err("Only E2=1 for an already selected PiP layout is verified; this write is disabled. No monitor command was sent.".into());
            }
            Some((*allow_undocumented, *confirm))
        }
        AdvancedCommand::Input { .. }
        | AdvancedCommand::Mode { .. }
        | AdvancedCommand::Pbp { .. }
        | AdvancedCommand::Pip { .. }
        | AdvancedCommand::Preset {
            command: Preset::Apply { .. },
        } => {
            return Err("This hardware-changing command is unverified and disabled. No monitor command was sent.".into());
        }
        _ => None,
    };
    match flags {
        Some((true, true)) => Ok(true),
        Some(_) => Err("The narrowly verified PiP enable write requires both --allow-undocumented and --confirm. No monitor command was sent.".into()),
        None => Ok(false),
    }
}

fn check_pip_display(display: &display::Display) -> Result<(), String> {
    if display.manufacturer.as_deref() != Some("SAM")
        || !display.name.to_ascii_uppercase().contains("G93SC")
        || !display.product_id.is_some_and(display::is_g93sc_product_id)
        || display.location != "External"
    {
        return Err("PiP enable is restricted to the verified external Samsung G93SC identity; no write sent".into());
    }
    Ok(())
}

fn enable_existing_pip(
    bus: &mut dyn Transport,
    display: &display::Display,
    mock: bool,
) -> Result<Value, String> {
    check_pip_display(display)?;
    let preflight = diagnostics::read(bus, 0xe2)?;
    if protocol::pip_enabled(&preflight.reading)? {
        return Ok(
            json!({"operation":"enable_existing_pip","status":"already_enabled","display":display,
            "mock":mock,"setting_writes":0,"preflight":preflight,"write":null,"readback":null,
            "requires_visual_verification":true}),
        );
    }
    if !mock {
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    // This operation sends only the verified enable value, never a layout/size/source command.
    let write = bus.write_once(&protocol::set_request(0xe2, 1))?;
    let mut status = if write.write_status == 0 {
        "submitted"
    } else {
        "write_failed"
    };
    let mut readback = None;
    let mut readback_error = None;
    if write.write_status == 0 {
        if !mock {
            std::thread::sleep(std::time::Duration::from_millis(1000));
        }
        match diagnostics::read(bus, 0xe2) {
            Ok(record) => {
                if protocol::pip_enabled(&record.reading) == Ok(true) {
                    status = "verified_readback";
                }
                readback = Some(record);
            }
            Err(error) => readback_error = Some(error),
        }
    }
    Ok(
        json!({"operation":"enable_existing_pip","status":status,"display":display,"mock":mock,
        "setting_writes":1,"preflight":preflight,"write":write,"readback":readback,"readback_error":readback_error,
        "requires_visual_verification":true,"note":"Readback confirms the enable bit only; verify the physical PiP picture. No automatic rollback or further setting write is sent."}),
    )
}

fn run_advanced(args: &Cli, command: &AdvancedCommand) -> Result<Value, String> {
    let enable_pip = pip_write_requested(command)?;
    let fixture = if let Some(path) = &args.mock {
        Some(
            serde_json::from_str::<diagnostics::Fixture>(
                &fs::read_to_string(path).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let displays = if let Some(f) = &fixture {
        vec![f.display.clone()]
    } else {
        display::enumerate()?
    };
    if matches!(command, AdvancedCommand::List) {
        return Ok(json!({"displays":displays}));
    }
    let selected: Vec<_> = displays
        .into_iter()
        .filter(|d| {
            args.display
                .as_ref()
                .map(|s| {
                    let id = s
                        .strip_prefix("0x")
                        .or_else(|| s.strip_prefix("0X"))
                        .and_then(|n| u64::from_str_radix(n, 16).ok())
                        .or_else(|| s.parse::<u64>().ok());
                    match id {
                        Some(id) => id == d.registry_id,
                        None if s.starts_with("0x")
                            || s.starts_with("0X")
                            || s.bytes().all(|c| c.is_ascii_digit()) =>
                        {
                            false
                        }
                        None => d.name.to_lowercase().contains(&s.to_lowercase()),
                    }
                })
                .unwrap_or(d.location == "External")
        })
        .collect();
    if selected.len() != 1 {
        return Err(format!(
            "selection matched {} interfaces; use list and --display <registry-id>",
            selected.len()
        ));
    }
    let display = &selected[0];
    if matches!(command, AdvancedCommand::Info) {
        return Ok(json!(display));
    }
    if enable_pip {
        check_pip_display(display)?;
    }
    let mut bus: Box<dyn Transport> = if let Some(f) = fixture {
        Box::new(diagnostics::MockTransport::from_fixture(f))
    } else {
        Box::new(transport::NativeTransport::open(display.registry_id)?)
    };
    if enable_pip {
        return enable_existing_pip(bus.as_mut(), display, args.mock.is_some());
    }
    match command {
        AdvancedCommand::Capabilities => diagnostics::capabilities(bus.as_mut()),
        AdvancedCommand::Ddc {
            command: Ddc::Read { vcp },
        } => {
            let value = protocol::number(vcp)?;
            let code = u8::try_from(value).map_err(|_| "VCP code must be 0..255")?;
            Ok(json!(diagnostics::read(bus.as_mut(), code)?))
        }
        AdvancedCommand::Ddc {
            command: Ddc::Scan { all, .. },
        } => {
            let result = diagnostics::scan(bus.as_mut(), *all, None)?;
            Ok(
                json!({"read_only":true,"display":display,"reads":result.reads,"complete":result.complete,"expected_count":result.expected_count,"stopped_reason":result.stopped_reason}),
            )
        }
        AdvancedCommand::Diagnose { output, label, all } => {
            if output.exists() {
                return Err("output already exists; choose a fresh capture path".into());
            }
            let value =
                diagnostics::capture(bus.as_mut(), display, label, *all, &journal_path(output))?;
            save(output, &value)?;
            Ok(json!({"saved":output,"capture":value}))
        }
        AdvancedCommand::Preset {
            command: Preset::Save { name },
        } => {
            let path = presets::path(name)?;
            if path.exists() {
                return Err("preset snapshot already exists".into());
            }
            let value =
                diagnostics::capture(bus.as_mut(), display, name, false, &journal_path(&path))?;
            save(&path, &value)?;
            Ok(
                json!({"saved":path,"executable":false,"note":"Observed snapshot only; apply disabled"}),
            )
        }
        _ => unreachable!(),
    }
}

fn selected_layout_device(args: &Cli) -> Result<(display::Display, Box<dyn app::Device>), String> {
    let fixture = if let Some(path) = &args.mock {
        Some(
            serde_json::from_slice::<diagnostics::Fixture>(
                &fs::read(path).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let displays = if let Some(f) = &fixture {
        vec![f.display.clone()]
    } else {
        display::enumerate()?
    };
    let matches: Vec<_> = displays
        .into_iter()
        .filter(app::supported_display)
        .filter(|d| {
            args.display
                .as_ref()
                .map(|value| {
                    let id = value
                        .strip_prefix("0x")
                        .or_else(|| value.strip_prefix("0X"))
                        .and_then(|s| u64::from_str_radix(s, 16).ok())
                        .or_else(|| value.parse().ok());
                    match id {
                        Some(id) => d.registry_id == id,
                        None => {
                            !value.starts_with("0x")
                                && !value.bytes().all(|b| b.is_ascii_digit())
                                && d.name.to_lowercase().contains(&value.to_lowercase())
                        }
                    }
                })
                .unwrap_or(true)
        })
        .collect();
    if matches.len() != 1 {
        return Err(format!("Found {} supported monitor connections. Use wide advanced list to choose a connection.",matches.len()));
    }
    let display = matches.into_iter().next().unwrap();
    let device: Box<dyn app::Device> = if let Some(f) = fixture {
        let reading = |code: u8| -> Result<protocol::Reading, String> {
            let key = format!("0x{code:02X}");
            let raw = f
                .replies
                .get(&key)
                .ok_or_else(|| format!("Mock fixture has no {key} reply"))?;
            Ok(protocol::parse(code, &protocol::unhex(raw)?))
        };
        Box::new(app::MockDevice::new(MonitorState::decode(
            &reading(0xe2)?,
            &reading(0xe3)?,
            &reading(0x60)?,
        )?))
    } else {
        Box::new(app::NativeDevice::new(display.clone())?)
    };
    Ok((display, device))
}

fn layout_description(state: &MonitorState, source: Option<Source>) -> String {
    let role = source.map(|s| {
        if state.main == s {
            if state.mode == 7 {
                "big picture"
            } else {
                "left"
            }
        } else if state.mode == 7 {
            "small picture"
        } else {
            "right"
        }
    });
    if state.enabled {
        format!(
            "{}{}",
            state.label(),
            role.map(|r| format!(" · this Mac: {r}"))
                .unwrap_or_default()
        )
    } else {
        state.label().into()
    }
}

fn run_status(args: &Cli) -> Result<Value, String> {
    let (display, mut device) = selected_layout_device(args)?;
    let state = device.state()?;
    let display = device.display().cloned().unwrap_or(display);
    let source = app::detected_source(&display);
    let mode = if args.mock.is_none() {
        display
            .cg_display_id
            .and_then(|id| resolution::current(id).ok())
    } else {
        None
    };
    Ok(
        json!({"monitor":display.name,"connection":source.map(|s|s.label()),"layout":layout_description(&state,source),"state":state,"display":display,"local_mode":mode,"mock":args.mock.is_some()}),
    )
}

fn run_form(args: &Cli) -> Result<Value, String> {
    if args.json {
        return Err("JSON output requires an explicit command, such as wide --json status.".into());
    }
    ui::require_terminal()?;
    let (display, mut device) = selected_layout_device(args)?;
    // A failed initial read exits before asking for a source/layout or confirmation.
    let state = device.state()?;
    let source = match app::detected_source(&display) {
        Some(source) => source,
        None => match ui::choose_source()? {
            Some(s) => s,
            None => return Ok(json!({"cancelled":true})),
        },
    };
    device.bind_source(source)?;
    let context = ui::UiContext {
        monitor_name: display.name,
        current_input: source.label().into(),
        current_layout: layout_description(&state, Some(source)),
        current_choice: if !state.enabled {
            LayoutChoice::Fullscreen
        } else if state.mode == 7 {
            if state.main == source {
                LayoutChoice::PipBig
            } else {
                LayoutChoice::PipSmall
            }
        } else if state.main == source {
            LayoutChoice::SplitLeft
        } else {
            LayoutChoice::SplitRight
        },
    };
    let Some(choice) = ui::choose(&context)? else {
        return Ok(json!({"cancelled":true}));
    };
    let plan = Plan::build(choice, source, &state)?;
    app::check_plan(&plan, false)?;
    let target_main = match choice {
        LayoutChoice::Fullscreen | LayoutChoice::SplitLeft | LayoutChoice::PipBig => source,
        _ => source.other(),
    };
    let sound = match choice {
        LayoutChoice::PipBig | LayoutChoice::PipSmall => {
            format!("{} (small picture)", target_main.other())
        }
        LayoutChoice::SplitLeft | LayoutChoice::SplitRight => {
            format!("{} (left picture)", target_main)
        }
        LayoutChoice::Fullscreen => "unchanged".into(),
    };
    let picture = plan.summary();
    let summary=format!("{}\nThis Mac's resolution: {}\nSound: {}\nThe other machine manages its own resolution.\nThe display may briefly reconnect.",picture,app::resolution_preview(choice.resolution_role()),sound);
    if !ui::confirm(&summary)? {
        return Ok(json!({"cancelled":true}));
    }
    eprintln!("Applying layout…");
    app::apply(device.as_mut(), &plan, false, None)
}

fn run_layout_advanced(args: &Cli, command: &AdvancedCommand) -> Result<Value, String> {
    if let AdvancedCommand::Apply { confirm: false, .. } = command {
        return Err("Applying a layout requires --confirm. No monitor command was sent.".into());
    }
    let (display, mut device) = selected_layout_device(args)?;
    if let AdvancedCommand::Reconnect {
        layout,
        confirm,
        cycle,
    } = command
    {
        if args.mock.is_some() {
            return Err("A fixture has no native display connection to refresh.".into());
        }
        if *cycle {
            return recovery::cycle(&display, *confirm);
        }
        let role = layout
            .as_deref()
            .map(|layout| {
                layout
                    .parse::<LayoutChoice>()
                    .map(LayoutChoice::resolution_role)
            })
            .transpose()?;
        return recovery::retime(&display, role, *confirm);
    }
    if let AdvancedCommand::Sound { confirm } = command {
        let state = device.state()?;
        let role = if !state.enabled {
            None
        } else if state.mode == 7 {
            Some(audio::Role::Secondary)
        } else {
            Some(audio::Role::Main)
        };
        if !confirm || role.is_none() {
            return Ok(
                json!({"ok":true,"dry_run":!confirm,"display":display,"policy":role,"setting_writes":0,"layout_untouched":true,"resolution_untouched":true}),
            );
        }
        let result = device.fit_audio(role.unwrap())?;
        return Ok(
            json!({"ok":result["ok"],"audio":result,"layout_untouched":true,"resolution_untouched":true}),
        );
    }
    if matches!(command, AdvancedCommand::Modes) {
        if args.mock.is_some() {
            return Err("A fixture has no native display modes.".into());
        }
        let id = display
            .cg_display_id
            .ok_or("No current macOS display mode available")?;
        return Ok(
            json!({"display":display,"current":resolution::current(id)?,"modes":resolution::enumerate(id)?}),
        );
    }
    let (requested, port) = match command {
        AdvancedCommand::Plan { layout, port } | AdvancedCommand::Apply { layout, port, .. } => {
            (layout, port)
        }
        _ => unreachable!(),
    };
    let choice: LayoutChoice = requested.parse()?;
    let source = match port {
        Some(value) => value.parse()?,
        None => app::detected_source(&display)
            .ok_or("Could not detect this Mac's connection. Set --port hdmi1 or --port dp.")?,
    };
    if app::detected_source(&display).is_some_and(|detected| detected != source) {
        return Err(
            "The chosen port conflicts with this Mac's detected cable. No layout command was sent."
                .into(),
        );
    }
    device.bind_source(source)?;
    let state = device.state()?;
    let plan = Plan::build(choice, source, &state)?;
    if matches!(command, AdvancedCommand::Plan { .. }) {
        return Ok(
            json!({"dry_run":true,"display":display,"plan":plan,"resolution_policy":app::resolution_preview(plan.resolution_role),"mock":args.mock.is_some()}),
        );
    }
    let experimental = {
        #[cfg(feature = "experimental-writes")]
        {
            matches!(
                command,
                AdvancedCommand::Apply {
                    experimental_layout: true,
                    ..
                }
            )
        }
        #[cfg(not(feature = "experimental-writes"))]
        {
            false
        }
    };
    let log = match command {
        AdvancedCommand::Apply { log, .. } => log.as_deref(),
        _ => None,
    };
    app::apply(device.as_mut(), &plan, experimental, log)
}

fn run(args: &Cli) -> Result<Value, String> {
    match &args.command {
        None => run_form(args),
        Some(Command::Status) => run_status(args),
        Some(Command::Advanced { command }) => match command {
            AdvancedCommand::Sound { .. }
            | AdvancedCommand::Reconnect { .. }
            | AdvancedCommand::Plan { .. }
            | AdvancedCommand::Apply { .. }
            | AdvancedCommand::Modes => run_layout_advanced(args, command),
            _ => run_advanced(args, command),
        },
    }
}

fn main() {
    if let Some(code) = transport::maybe_run_read_worker() {
        std::process::exit(code);
    }
    let args = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    match run(&args) {
        Ok(mut value) => {
            ui::annotate_failure(&mut value);
            if args.command.is_none() && !args.json {
                if value["cancelled"] == true {
                    println!("No changes made.");
                } else if value["ok"] == true {
                    println!(
                        "{}",
                        if value["layout_changed"] == false
                            && value["resolution"]["changed"] != true
                            && value["audio"]["changed"] != true
                        {
                            "Already set.".to_string()
                        } else {
                            value["plan"]["choice"]
                                .as_str()
                                .and_then(|s| s.parse::<LayoutChoice>().ok())
                                .map(|choice| format!("Applied: {}.", choice.label()))
                                .unwrap_or_else(|| "Layout applied.".into())
                        }
                    );
                    if let Some(mode) = value["resolution"]["after"].as_object() {
                        println!(
                            "This Mac: {} × {}",
                            mode["pixel_width"], mode["pixel_height"]
                        );
                    }
                } else {
                    eprintln!(
                        "{}",
                        ui::failure_message(
                            if value["reconnect_required"] == true {
                                ui::RECONNECT_HINT
                            } else {
                                value["error"]
                                    .as_str()
                                    .unwrap_or("Could not verify the layout.")
                            },
                            value["partial"] == true,
                        )
                    );
                }
            } else if matches!(args.command, Some(Command::Status)) && !args.json {
                println!(
                    "{} · this Mac via {}",
                    value["monitor"].as_str().unwrap_or("Monitor"),
                    value["connection"].as_str().unwrap_or("unknown connection")
                );
                println!("{}", value["layout"].as_str().unwrap_or("Unknown layout"));
                if let Some(mode) = value["local_mode"].as_object() {
                    println!("{} × {}", mode["pixel_width"], mode["pixel_height"]);
                }
            } else if args.json {
                println!("{}", serde_json::to_string(&value).unwrap());
            } else {
                println!("{}", serde_json::to_string_pretty(&value).unwrap());
            }
            if value["ok"] == false {
                std::process::exit(1);
            }
        }
        Err(error) => {
            if args.json {
                let mut value = json!({"error":error});
                ui::annotate_failure(&mut value);
                println!("{value}");
            } else {
                let message =
                    if args.command.is_none() || matches!(args.command, Some(Command::Status)) {
                        ui::failure_message(&error, false)
                    } else {
                        error
                    };
                eprintln!("error: {message}");
            }
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jsonl_output_cannot_collide_with_its_journal() {
        let output = Path::new("captures/state.jsonl");
        assert_ne!(journal_path(output), output);
        assert_eq!(
            journal_path(output),
            Path::new("captures/state.jsonl.journal.jsonl")
        );
    }

    fn mock_pip() -> (display::Display, diagnostics::MockTransport) {
        let fixture: diagnostics::Fixture =
            serde_json::from_str(include_str!("../tests/fixtures/pip-disabled.json")).unwrap();
        (
            fixture.display.clone(),
            diagnostics::MockTransport::from_fixture(fixture),
        )
    }

    #[test]
    fn pip_write_is_single_attempt_and_failure_never_retries_or_changes_state() {
        let (display, mut bus) = mock_pip();
        let result = enable_existing_pip(&mut bus, &display, true).unwrap();
        assert_eq!(result["status"], "verified_readback");
        assert_eq!(bus.writes, ["8403e200015b"]);
        assert!(bus.write_once(&protocol::set_request(0xe2, 1)).is_err());
        assert_eq!(bus.writes.len(), 1);

        let (display, mut bus) = mock_pip();
        bus.write_status = -1;
        let result = enable_existing_pip(&mut bus, &display, true).unwrap();
        assert_eq!(result["status"], "write_failed");
        assert_eq!(result["write"]["write_status"], -1);
        assert!(result["readback"].is_null());
        assert_eq!(bus.writes.len(), 1);
        assert_eq!(bus.replies["0xE2"], "6e880200e200007f35ced2");
        assert!(bus.write_once(&protocol::set_request(0xe2, 1)).is_err());
    }

    #[test]
    fn matching_identity_and_protocol_profile_are_both_required() {
        let (mut display, mut bus) = mock_pip();
        display.manufacturer = Some("APP".into());
        assert!(enable_existing_pip(&mut bus, &display, true).is_err());
        assert!(bus.writes.is_empty());
        let (display, mut bus) = mock_pip();
        bus.replies
            .insert("0xE2".into(), "6e880200e200007f35cfd3".into());
        assert_eq!(
            enable_existing_pip(&mut bus, &display, true).unwrap()["status"],
            "already_enabled"
        );
        assert!(bus.writes.is_empty());
    }
}
