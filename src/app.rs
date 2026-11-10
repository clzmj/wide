//! The friendly workflow: inspect, plan, confirm elsewhere, apply once, verify, then fit this Mac.
use crate::{
    audio::{self, Role as AudioRole},
    diagnostics,
    display::{self, Display},
    layout::{LayoutChoice, MonitorState, Plan, Source, Step},
    protocol,
    resolution::{self, Mode, ResolutionRole},
    transport::{Exchange, NativeTransport, Transport},
};
use serde_json::{json, Value};
use std::{
    fs::OpenOptions,
    io::Write,
    path::Path,
    thread,
    time::{Duration, Instant},
};

// Validated against the owner's G93SC1011.0 in W03/W04; other profiles still fail preflight.
const COMPLETE_PROFILE_VERIFIED: bool = true;

pub fn supported_display(d: &Display) -> bool {
    d.manufacturer.as_deref() == Some("SAM")
        && d.name.to_ascii_uppercase().contains("G93SC")
        && matches!(d.product_id, Some(0x749b | 0x745e | 0x7462 | 0x7461))
        && d.location == "External"
        && d.serial.is_some_and(|s| s != 0)
}

pub fn detected_source(d: &Display) -> Option<Source> {
    match d.connection.as_deref() {
        Some("HDMI") => Some(Source::Hdmi1),
        Some("DisplayPort") => Some(Source::DisplayPort),
        _ => None,
    }
}

pub fn resolution_preview(role: ResolutionRole) -> &'static str {
    match role {
        ResolutionRole::Fullscreen => "5120 × 1440",
        ResolutionRole::PipMain => {
            "Ultrawide, up to 5120 × 1440 (3840 × 1080 when PiP requires it)"
        }
        ResolutionRole::SplitHalf => "2560 × 1440",
        ResolutionRole::PipInset => {
            "16:9 inset, up to 1920 × 1080 (1280 × 720 when PiP requires it)"
        }
    }
}

pub trait Device {
    fn bind_source(&mut self, _source: Source) -> Result<(), String> {
        Ok(())
    }
    fn display(&self) -> Option<&Display> {
        None
    }
    fn read_evidence(&self) -> Value {
        Value::Null
    }
    fn state(&mut self) -> Result<MonitorState, String>;
    fn send(&mut self, step: Step) -> Result<Exchange, String>;
    fn fit_resolution(&mut self, role: ResolutionRole) -> Result<Value, String>;
    fn fit_audio(&mut self, role: AudioRole) -> Result<Value, String>;
    fn wait(&mut self) {
        thread::sleep(Duration::from_millis(250));
    }
}

#[derive(Debug)]
enum StateReadFailure {
    Reenumerating(String),
    Failed(String),
}

fn reenumeration_error(error: &str) -> bool {
    error.starts_with("IORegistry changed during discovery")
        || (error.starts_with("Expected exactly one external AV proxy with registry ID ")
            && error.contains("; found 0."))
}

fn no_device_record(record: &diagnostics::Record) -> bool {
    // SDK kIOReturnNoDevice; unlike a timeout, this says the selected service left.
    const NO_DEVICE: i32 = 0xe00002c0u32 as i32;
    record.reading.status == "transport_error"
        && record.attempts.last().is_some_and(|exchange| {
            (exchange.write_status == NO_DEVICE && exchange.read_status.is_none())
                || (exchange.write_status == 0 && exchange.read_status == Some(NO_DEVICE))
        })
}

fn read_state_feature(
    bus: &mut dyn Transport,
    evidence: &mut Vec<diagnostics::Record>,
    code: u8,
) -> Result<usize, StateReadFailure> {
    let record = diagnostics::read(bus, code).map_err(|error| {
        if reenumeration_error(&error) {
            StateReadFailure::Reenumerating(error)
        } else {
            StateReadFailure::Failed(error)
        }
    })?;
    let transient = no_device_record(&record);
    let ok = record.reading.status == "ok";
    let status = record.reading.status.clone();
    let index = evidence.len();
    evidence.push(record);
    if transient {
        return Err(StateReadFailure::Reenumerating(format!(
            "The monitor re-enumerated during a read (NoDevice, feature {code:02X})."
        )));
    }
    if !ok {
        return Err(StateReadFailure::Failed(format!(
            "The monitor's feature {code:02X} reply is unavailable ({status})."
        )));
    }
    Ok(index)
}

fn read_state_once(
    bus: &mut dyn Transport,
    evidence: &mut Vec<diagnostics::Record>,
    mock: bool,
) -> Result<MonitorState, StateReadFailure> {
    let e2 = read_state_feature(bus, evidence, 0xe2)?;
    if !mock {
        thread::sleep(Duration::from_millis(150));
    }
    let e3 = read_state_feature(bus, evidence, 0xe3)?;
    MonitorState::decode(
        &evidence[e2].reading,
        &evidence[e3].reading,
        &protocol::Reading::failed("not_queried"),
    )
    .map_err(StateReadFailure::Failed)
}

pub struct NativeDevice {
    anchor: Display,
    requested_source: Option<Source>,
    pub latest: Display,
    pub last_reads: Vec<diagnostics::Record>,
}
impl NativeDevice {
    pub fn new(display: Display) -> Result<Self, String> {
        if !supported_display(&display) {
            return Err("wide supports the Samsung G93SC on its verified firmware profile.".into());
        }
        Ok(Self {
            anchor: display.clone(),
            requested_source: None,
            latest: display,
            last_reads: Vec::new(),
        })
    }
    fn rebind(&mut self) -> Result<(), String> {
        self.rebind_until(Instant::now() + Duration::from_secs(5))
    }
    fn rebind_until(&mut self, deadline: Instant) -> Result<(), String> {
        while Instant::now() < deadline {
            let observed = match display::enumerate() {
                Ok(value) => value,
                Err(error) if reenumeration_error(&error) => {
                    thread::sleep(Duration::from_millis(150));
                    continue;
                }
                Err(error) => return Err(error),
            };
            let matches = self.rebind_candidates(&observed);
            if matches.len() == 1 {
                let next = matches.into_iter().next().unwrap();
                if self.latest.registry_id != next.registry_id {
                    tracing::info!(old_registry_id=self.latest.registry_id,new_registry_id=next.registry_id,
                        cg_display_id=?next.cg_display_id,"Rebound monitor after re-enumeration");
                }
                self.latest = next;
                return Ok(());
            }
            if matches.len() > 1 {
                return Err("More than one connection matches this monitor. Choose one with wide advanced --help.".into());
            }
            thread::sleep(Duration::from_millis(250));
        }
        Err("The monitor has not reconnected. No further setting command was sent.".into())
    }

    fn rebind_candidates(&self, observed: &[Display]) -> Vec<Display> {
        let unique_external = observed
            .iter()
            .filter(|display| display.location == "External")
            .count()
            == 1;
        observed
            .iter()
            .filter(|d| {
                supported_display(d)
                    && d.manufacturer == self.anchor.manufacturer
                    && d.serial == self.anchor.serial
                    && !self.connection_conflicts(d)
                    && (self.connection_matches(d)
                        || (unique_external
                            && self
                                .anchor
                                .cg_display_id
                                .is_some_and(|id| id != 0 && d.cg_display_id == Some(id))))
            })
            .cloned()
            .collect()
    }

    fn connection_matches(&self, d: &Display) -> bool {
        if self.connection_conflicts(d) {
            return false;
        }
        d.registry_id == self.latest.registry_id
    }
    fn expected_connection(&self) -> Option<&str> {
        self.requested_source
            .map(|source| match source {
                Source::Hdmi1 => "HDMI",
                Source::DisplayPort => "DisplayPort",
            })
            .or(self.anchor.connection.as_deref())
    }
    fn connection_conflicts(&self, d: &Display) -> bool {
        self.expected_connection()
            .zip(d.connection.as_deref())
            .is_some_and(|(expected, actual)| expected != actual)
    }
}
impl Device for NativeDevice {
    fn bind_source(&mut self, source: Source) -> Result<(), String> {
        if detected_source(&self.anchor).is_some_and(|detected| detected != source) {
            return Err("The chosen port conflicts with this Mac's detected cable. No layout command was sent.".into());
        }
        self.requested_source = Some(source);
        Ok(())
    }
    fn display(&self) -> Option<&Display> {
        Some(&self.latest)
    }
    fn read_evidence(&self) -> Value {
        json!(self.last_reads)
    }
    fn state(&mut self) -> Result<MonitorState, String> {
        self.last_reads.clear();
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            self.rebind_until(deadline)?;
            let mut bus = NativeTransport::open(self.latest.registry_id)?;
            match read_state_once(&mut bus, &mut self.last_reads, false) {
                Ok(state) => return Ok(state),
                Err(StateReadFailure::Reenumerating(error)) if Instant::now() < deadline => {
                    tracing::info!(registry_id=self.latest.registry_id,reason=%error,"Retrying read-only state after monitor re-enumeration");
                    thread::sleep(Duration::from_millis(150));
                }
                Err(StateReadFailure::Reenumerating(error) | StateReadFailure::Failed(error)) => {
                    return Err(error)
                }
            }
        }
    }
    fn send(&mut self, step: Step) -> Result<Exchange, String> {
        self.rebind()?;
        // A fresh handle consumes exactly one attempt. Never retry a setting command.
        let mut bus = NativeTransport::open(self.latest.registry_id)?;
        let (code, value) = step.command();
        thread::sleep(Duration::from_millis(150));
        bus.write_once(&protocol::set_request(code, value))
    }
    fn fit_resolution(&mut self, role: ResolutionRole) -> Result<Value, String> {
        let started = Instant::now();
        let (id, before, selected) = loop {
            self.rebind()?;
            let candidate = (|| {
                let id = self
                    .latest
                    .cg_display_id
                    .ok_or("macOS has not exposed this monitor's current display modes.")?;
                let before = resolution::current(id)?;
                let modes = resolution::enumerate(id)?;
                let selected = resolution::choose(role, &modes, &before)?;
                Ok::<_, String>((id, before, selected))
            })();
            match candidate {
                Ok(value) => break value,
                Err(_) if started.elapsed() < Duration::from_secs(8) => {
                    thread::sleep(Duration::from_millis(250))
                }
                Err(error) => return Err(error),
            }
        };
        let changed = selected.id != before.id;
        let after = if changed {
            resolution::apply(id, selected.id)?
        } else {
            before.clone()
        };
        Ok(
            json!({"scope":"this_mac_only","changed":changed,"before":before,"after":after,"verified":true}),
        )
    }
    fn fit_audio(&mut self, role: AudioRole) -> Result<Value, String> {
        self.rebind()?;
        // Audio is last: reacquire this physical connection after layout/resolution changes.
        // A fresh handle has exactly one Set attempt, including any kernel failure.
        let mut bus = NativeTransport::open(self.latest.registry_id)?;
        audio::apply(&mut bus, &role, false)
    }
}

fn same_layout(a: &MonitorState, b: &MonitorState) -> bool {
    a.enabled == b.enabled && a.mode == b.mode && a.main == b.main && a.secondary == b.secondary
}

pub fn check_plan(plan: &Plan, experimental: bool) -> Result<(), String> {
    if COMPLETE_PROFILE_VERIFIED || experimental {
        return Ok(());
    }
    if plan.steps.iter().all(|s| *s == Step::Enable) && plan.before.mode == 7 {
        return Ok(());
    }
    Err("This layout is still being validated for your monitor. Its current picture has not been changed.".into())
}

/// All setting effects require a confirmed plan. A failure stops the sequence; no rollback is sent.
pub fn apply(
    device: &mut dyn Device,
    plan: &Plan,
    experimental: bool,
    log: Option<&Path>,
) -> Result<Value, String> {
    check_plan(plan, experimental)?;
    let before = device.state()?;
    // The confirmed intent is the requested layout, not an obsolete step list.
    // Replan from a fresh verified state before sending any settings.
    let fresh_plan = Plan::build(plan.choice, plan.this_source, &before)?;
    let plan = &fresh_plan;
    check_plan(plan, experimental)?;
    let mut journal = log
        .map(|p| {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(p)
                .map_err(|e| e.to_string())
        })
        .transpose()?;
    let mut observed = before;
    let mut steps = Vec::new();
    for step in &plan.steps {
        if let Some(file) = &mut journal {
            writeln!(
                file,
                "{}",
                json!({"phase":"intent","step":step,"before":observed,"command":step.command()})
            )
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        }
        let exchange = match device.send(*step) {
            Ok(value) => value,
            Err(error) => {
                return Ok(
                    json!({"ok":false,"partial":true,"error":error,"plan":plan,"steps":steps,"resolution":null,"automatic_rollback":false}),
                )
            }
        };
        if let Some(file) = &mut journal {
            writeln!(
                file,
                "{}",
                json!({"phase":"write_return","step":step,"exchange":exchange})
            )
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        }
        if exchange.write_status != 0 {
            steps.push(json!({"step":step,"write":exchange,"verified":false}));
            return Ok(
                json!({"ok":false,"partial":true,"error":"The monitor write failed. The sequence stopped without a retry.","plan":plan,"steps":steps,"resolution":null,"automatic_rollback":false}),
            );
        }
        let mut last_error = None;
        let mut readbacks = Vec::new();
        let mut settled = None;
        // Valid replies may need transition settling. A transport failure stops
        // immediately; NativeDevice already handles only normal NoDevice rebinding.
        for _ in 0..4 {
            device.wait();
            match device.state() {
                Ok(after) => {
                    readbacks.push(json!({"state":after}));
                    if step.verify(&observed, &after) {
                        settled = Some(after);
                        break;
                    }
                    last_error = Some("The monitor did not report the expected change within the settling window.".to_string());
                }
                Err(error) => {
                    readbacks.push(json!({"error":error,"evidence":device.read_evidence()}));
                    last_error = Some(error);
                    break;
                }
            }
        }
        let after = match settled.ok_or_else(|| {
            last_error.unwrap_or_else(|| "Layout verification did not complete.".into())
        }) {
            Ok(state) => state,
            Err(error) => {
                steps.push(
                    json!({"step":step,"write":exchange,"verified":false,"readbacks":readbacks}),
                );
                return Ok(
                    json!({"ok":false,"partial":true,"error":error,"plan":plan,"steps":steps,"recoveries":[],"failed_readback":device.read_evidence(),"resolution":null,"automatic_rollback":false}),
                );
            }
        };
        let verified = step.verify(&observed, &after);
        let item = json!({"step":step,"write":exchange,"after":after,"verified":verified,"readbacks":readbacks});
        if let Some(file) = &mut journal {
            writeln!(file, "{}", json!({"phase":"result","result":item}))
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
        }
        steps.push(item);
        if !verified {
            return Ok(
                json!({"ok":false,"error":"The monitor did not report the expected change. The sequence stopped; no retry or rollback was sent.","plan":plan,"steps":steps,"resolution":null,"automatic_rollback":false}),
            );
        }
        observed = after;
    }
    let expected_enabled = plan.choice != LayoutChoice::Fullscreen;
    let expected_mode = match plan.choice {
        LayoutChoice::Fullscreen => observed.mode,
        LayoutChoice::SplitLeft | LayoutChoice::SplitRight => 0,
        _ => 7,
    };
    if observed.enabled != expected_enabled
        || observed.mode != expected_mode
        || observed.main != plan.target_main
        || observed.secondary != plan.target_secondary
    {
        return Ok(
            json!({"ok":false,"error":"Final layout verification did not match your selection.","plan":plan,"steps":steps,"after":observed,"resolution":null}),
        );
    }
    let resolution = match device.fit_resolution(plan.resolution_role) {
        Ok(value) => value,
        Err(error) => {
            return Ok(
                json!({"ok":false,"error":format!("Monitor layout applied, but this Mac's resolution could not be verified: {error}"),"plan":plan,"steps":steps,"after":observed,"resolution":null,"audio":null,"automatic_rollback":false}),
            )
        }
    };
    // Never let an audio failure prevent this Mac's resolution correction.
    let audio = audio::policy(plan.choice).map(|role| match device.fit_audio(role) {
        Ok(value) => value,
        Err(error) => json!({"ok":false,"error":error,"logical_role":role,"changed":false,
            "status":"failed","preflight":null,"write":null,"readback":null,"audible_verification_required":true}),
    });
    let audio_ok = audio.as_ref().is_none_or(|value| value["ok"] == true);
    let mut result = json!({"ok":audio_ok,"layout_changed":!same_layout(&before,&observed),"plan":plan,"steps":steps,
        "after":observed,"resolution":resolution,"audio":audio,"recoveries":[],"automatic_rollback":false,
        "other_machine":"Resolution is managed by the other machine; run wide there if it needs adjustment."});
    if !audio_ok {
        result["partial"] = json!(true);
        result["layout_applied"] = json!(true);
        result["resolution_applied"] = json!(true);
        result["error"] = json!("Monitor layout and this Mac's resolution were applied, but monitor audio could not be verified. No rollback was sent.");
    }
    Ok(result)
}

/// Offline form/planner testing uses the same typed steps, never IOKit.
pub struct MockDevice {
    pub state: MonitorState,
    pub writes: Vec<Step>,
    pub fail_step: Option<usize>,
    pub audio_role: AudioRole,
    pub audio_writes: Vec<AudioRole>,
    pub fail_audio: bool,
}
impl MockDevice {
    pub fn new(state: MonitorState) -> Self {
        Self {
            state,
            writes: vec![],
            fail_step: None,
            audio_role: AudioRole::Main,
            audio_writes: vec![],
            fail_audio: false,
        }
    }
}
impl Device for MockDevice {
    fn state(&mut self) -> Result<MonitorState, String> {
        Ok(self.state)
    }
    fn send(&mut self, step: Step) -> Result<Exchange, String> {
        self.writes.push(step);
        let fail = self.fail_step == Some(self.writes.len());
        if !fail {
            match step {
                Step::Enable => self.state.raw_e2 |= 1,
                Step::Disable => self.state.raw_e2 &= !1,
                Step::RestorePbp | Step::EqualSplit => self.state.raw_e2 &= !0x0e,
                Step::SelectPip => self.state.raw_e2 |= 0x0e,
                Step::SwapSources => {
                    std::mem::swap(&mut self.state.main, &mut self.state.secondary)
                }
            }
            self.state.enabled = self.state.raw_e2 & 1 != 0;
            self.state.mode = ((self.state.raw_e2 >> 1) & 7) as u8;
            self.state.input = Some(self.state.main);
        }
        let (code, value) = step.command();
        Ok(Exchange {
            request_hex: crate::transport::hex(&protocol::set_request(code, value)),
            response_hex: String::new(),
            write_status: if fail { -1 } else { 0 },
            read_status: None,
            elapsed_ms: 0.0,
        })
    }
    fn fit_resolution(&mut self, role: ResolutionRole) -> Result<Value, String> {
        let (w, h) = match role {
            ResolutionRole::Fullscreen | ResolutionRole::PipMain => (5120, 1440),
            ResolutionRole::SplitHalf => (2560, 1440),
            ResolutionRole::PipInset => (1920, 1080),
        };
        let mode = Mode {
            id: 1,
            width: w,
            height: h,
            pixel_width: w,
            pixel_height: h,
            refresh_hz: 60.0,
        };
        Ok(json!({"scope":"this_mac_only","mock":true,"after":mode,"verified":true}))
    }
    fn fit_audio(&mut self, role: AudioRole) -> Result<Value, String> {
        fn reply(role: AudioRole) -> String {
            let mut bytes = vec![0x6e, 0x88, 2, 0, 0xe8, 0, 0, 2, 0, role.value() as u8];
            bytes.push(bytes.iter().fold(0x50, |a, b| a ^ b));
            crate::transport::hex(&bytes)
        }
        let mut bus = diagnostics::MockTransport::default();
        bus.replies.insert("0xE8".into(), reply(self.audio_role));
        bus.after_write.insert("0xE8".into(), reply(role));
        if self.fail_audio {
            bus.write_status = -1;
        }
        let result = audio::apply(&mut bus, &role, true)?;
        if !bus.writes.is_empty() {
            self.audio_writes.push(role);
        }
        if result["ok"] == true {
            self.audio_role = role;
        }
        Ok(result)
    }
    fn wait(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> MonitorState {
        MonitorState {
            enabled: true,
            mode: 7,
            main: Source::Hdmi1,
            secondary: Source::DisplayPort,
            input: Some(Source::Hdmi1),
            raw_e2: 0x35cf,
        }
    }
    fn captured_endpoint_replacement() -> (NativeDevice, Display) {
        let fixture: diagnostics::Fixture =
            serde_json::from_str(include_str!("../tests/fixtures/layout-pip-enabled.json"))
                .unwrap();
        let mut before = fixture.display;
        before.registry_id = 4295617660;
        before.cg_display_id = Some(2);
        before.manufacturer = Some("SAM".into());
        before.serial = Some(42);
        before.product_id = Some(0x745e);
        before.connection = None;
        let mut device = NativeDevice::new(before.clone()).unwrap();
        device.bind_source(Source::Hdmi1).unwrap();
        let mut after = before;
        after.registry_id = 4295618005;
        after.product_id = Some(0x749b);
        (device, after)
    }
    #[test]
    fn captured_no_metadata_endpoint_rebinds_after_fullscreen() {
        let (device, after) = captured_endpoint_replacement();
        assert_eq!(
            device.rebind_candidates(&[after]).len(),
            1,
            "the recorded unique CG2/serial endpoint must survive its proxy replacement"
        );
        assert!(
            device.anchor.connection.is_none(),
            "a selected port is not discovered metadata"
        );
    }
    #[test]
    fn changed_endpoint_needs_unique_topology_same_cg_and_consistent_identity() {
        let (mut device, after) = captured_endpoint_replacement();
        for candidate in [
            Display {
                cg_display_id: None,
                ..after.clone()
            },
            Display {
                cg_display_id: Some(3),
                ..after.clone()
            },
            Display {
                serial: Some(123),
                ..after.clone()
            },
            Display {
                connection: Some("DisplayPort".into()),
                ..after.clone()
            },
        ] {
            assert!(device.rebind_candidates(&[candidate]).is_empty());
        }
        let anonymous = Display {
            registry_id: after.registry_id + 1,
            name: "External display".into(),
            manufacturer: None,
            serial: None,
            product_id: None,
            ..after.clone()
        };
        assert!(device
            .rebind_candidates(&[after.clone(), anonymous])
            .is_empty());
        device.anchor.cg_display_id = None;
        assert!(device.rebind_candidates(&[after]).is_empty());
    }
    #[test]
    fn captured_no_device_read_is_retryable_but_other_transport_failures_are_not() {
        let exchange: Exchange = serde_json::from_str(r#"{"elapsed_ms":1021.02675,"read_status":null,"request_hex":"8201e25e","response_hex":"","write_status":-536870208}"#).unwrap();
        let mut record = diagnostics::Record {
            vcp: "0xE2".into(),
            reading: protocol::Reading::failed("transport_error"),
            attempts: vec![exchange],
        };
        assert!(no_device_record(&record));
        record.attempts[0].write_status = 0xe00002d6u32 as i32;
        assert!(!no_device_record(&record));
        record.attempts[0].write_status = -1;
        assert!(!no_device_record(&record));
        assert!(!reenumeration_error(
            "DDC read timed out after 1500ms; worker still stopping"
        ));
        assert!(reenumeration_error("Expected exactly one external AV proxy with registry ID 4295617660; found 0. Run list again."));
    }
    #[test]
    fn read_only_state_attempt_retains_no_device_evidence_before_fresh_success() {
        struct LeftService;
        impl Transport for LeftService {
            fn exchange(&mut self, request: &[u8], _: usize, _: u64) -> Result<Exchange, String> {
                Ok(Exchange {
                    request_hex: crate::transport::hex(request),
                    response_hex: String::new(),
                    write_status: 0xe00002c0u32 as i32,
                    read_status: None,
                    elapsed_ms: 1021.02675,
                })
            }
        }
        let mut evidence = Vec::new();
        assert!(matches!(
            read_state_once(&mut LeftService, &mut evidence, true),
            Err(StateReadFailure::Reenumerating(_))
        ));
        assert_eq!(evidence.len(), 1);
        let fixture: diagnostics::Fixture =
            serde_json::from_str(include_str!("../tests/fixtures/layout-pip-enabled.json"))
                .unwrap();
        let mut fresh = diagnostics::MockTransport::from_fixture(fixture);
        let result = read_state_once(&mut fresh, &mut evidence, true).unwrap();
        assert_eq!(result.mode, 7);
        assert_eq!(evidence.len(), 3);
        assert!(no_device_record(&evidence[0]));
        assert!(fresh.writes.is_empty());
    }
    #[test]
    fn split_right_transport_failure_after_enable_stops_before_swap() {
        struct Interrupted {
            inner: MockDevice,
            blocked: bool,
            state_calls: usize,
        }
        impl Device for Interrupted {
            fn state(&mut self) -> Result<MonitorState, String> {
                self.state_calls += 1;
                if self.blocked {
                    Err("DDC request timed out".into())
                } else {
                    self.inner.state()
                }
            }
            fn send(&mut self, step: Step) -> Result<Exchange, String> {
                let result = self.inner.send(step);
                if step == Step::Enable {
                    self.blocked = true;
                }
                result
            }
            fn fit_resolution(&mut self, role: ResolutionRole) -> Result<Value, String> {
                self.inner.fit_resolution(role)
            }
            fn fit_audio(&mut self, role: AudioRole) -> Result<Value, String> {
                self.inner.fit_audio(role)
            }
            fn wait(&mut self) {}
        }
        let before = MonitorState {
            enabled: false,
            mode: 0,
            raw_e2: 0x35c0,
            ..state()
        };
        let mut device = Interrupted {
            inner: MockDevice::new(before),
            blocked: false,
            state_calls: 0,
        };
        let plan = Plan::build(LayoutChoice::SplitRight, Source::Hdmi1, &before).unwrap();
        let mut result = apply(&mut device, &plan, false, None).unwrap();
        crate::ui::annotate_failure(&mut result);
        assert_eq!(result["ok"], false, "{result}");
        assert_eq!(result["partial"], true);
        assert_eq!(result["error"], "DDC request timed out");
        assert_eq!(result["reconnect_required"], true);
        assert_eq!(result["action_hint"], crate::ui::RECONNECT_HINT);
        assert_eq!(device.inner.state.main, Source::Hdmi1);
        assert_eq!(device.inner.writes, [Step::Enable]);
        assert_eq!(device.state_calls, 2);
        assert_eq!(result["recoveries"], json!([]));

        let mut device = Interrupted {
            inner: MockDevice::new(before),
            blocked: true,
            state_calls: 0,
        };
        assert_eq!(
            apply(&mut device, &plan, false, None).unwrap_err(),
            "DDC request timed out"
        );
        assert!(device.inner.writes.is_empty());
        assert_eq!(device.state_calls, 1);
    }
    #[test]
    fn explicit_port_cannot_override_known_cable_and_unknown_rebind_cannot_hop_interfaces() {
        let fixture: diagnostics::Fixture =
            serde_json::from_str(include_str!("../tests/fixtures/layout-pip-enabled.json"))
                .unwrap();
        let mut d = NativeDevice::new(fixture.display.clone()).unwrap();
        assert!(d.bind_source(Source::DisplayPort).is_err());
        assert!(d.bind_source(Source::Hdmi1).is_ok());
        let mut other = fixture.display.clone();
        other.registry_id += 1;
        other.connection = None;
        assert!(!d.connection_matches(&other));
        d.anchor.connection = None;
        assert!(!d.connection_matches(&other));
        d.bind_source(Source::Hdmi1).unwrap();
        other.connection = Some("DisplayPort".into());
        assert!(!d.connection_matches(&other));
        other.connection = Some("HDMI".into());
        assert_eq!(d.rebind_candidates(&[other]).len(), 1);
    }
    #[test]
    fn verified_plan_failure_never_retries_or_rolls_back() {
        let plan = Plan::build(LayoutChoice::SplitRight, Source::Hdmi1, &state()).unwrap();
        let mut d = MockDevice::new(state());
        assert!(check_plan(&plan, false).is_ok());
        d.fail_step = Some(2);
        let result = apply(&mut d, &plan, true, None).unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(d.writes.len(), 2);
    }
    #[test]
    fn all_five_plans_apply_on_the_mock_and_preserve_host_anchor() {
        for source in [Source::Hdmi1, Source::DisplayPort] {
            for choice in [
                LayoutChoice::Fullscreen,
                LayoutChoice::SplitLeft,
                LayoutChoice::SplitRight,
                LayoutChoice::PipBig,
                LayoutChoice::PipSmall,
            ] {
                let mut d = MockDevice::new(state());
                let plan = Plan::build(choice, source, &d.state).unwrap();
                let result = apply(&mut d, &plan, true, None).unwrap();
                assert_eq!(result["ok"], true, "{source:?} {choice:?}");
                assert_eq!(d.state.main, plan.target_main);
                assert_eq!(plan.this_source, source);
                if let Some(role) = audio::policy(choice) {
                    assert_eq!(d.audio_role, role);
                    assert_eq!(result["audio"]["logical_role"], json!(role));
                } else {
                    assert!(result["audio"].is_null());
                    assert!(d.audio_writes.is_empty());
                }
            }
        }
    }
    #[test]
    fn fresh_target_plan_never_replays_an_obsolete_source_swap() {
        let old_plan = Plan::build(LayoutChoice::SplitRight, Source::Hdmi1, &state()).unwrap();
        let already_right = MonitorState {
            enabled: true,
            mode: 0,
            raw_e2: 0x35c1,
            main: Source::DisplayPort,
            secondary: Source::Hdmi1,
            ..state()
        };
        let mut device = MockDevice::new(already_right);
        let result = apply(&mut device, &old_plan, false, None).unwrap();
        assert_eq!(result["ok"], true);
        assert!(device.writes.is_empty());
        assert_eq!(device.state.secondary, Source::Hdmi1);
    }
    #[test]
    fn audio_failure_preserves_the_applied_layout_and_resolution() {
        let mut d = MockDevice::new(state());
        d.fail_audio = true;
        let plan = Plan::build(LayoutChoice::PipBig, Source::Hdmi1, &d.state).unwrap();
        let result = apply(&mut d, &plan, false, None).unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(result["partial"], true);
        assert_eq!(result["resolution"]["verified"], true);
        assert_eq!(result["resolution"]["after"]["pixel_width"], 5120);
        assert_eq!(result["audio"]["status"], "write_failed");
        assert_eq!(d.audio_writes, [AudioRole::Secondary]);
        assert!(d.writes.is_empty());
    }
    #[test]
    fn repeated_picture_in_picture_dedupes_monitor_audio() {
        let mut d = MockDevice::new(state());
        let plan = Plan::build(LayoutChoice::PipBig, Source::Hdmi1, &d.state).unwrap();
        assert_eq!(
            apply(&mut d, &plan, false, None).unwrap()["audio"]["changed"],
            true
        );
        assert_eq!(
            apply(&mut d, &plan, false, None).unwrap()["audio"]["status"],
            "already_selected"
        );
        assert_eq!(d.audio_writes, [AudioRole::Secondary]);
    }
}
