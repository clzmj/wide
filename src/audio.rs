//! Monitor picture-audio policy only. This never changes a macOS output device or volume.
use crate::{diagnostics, layout::LayoutChoice, protocol, transport::Transport};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{thread, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Main,
    Secondary,
}

impl Role {
    pub fn value(self) -> u16 {
        match self {
            Self::Main => 0,
            Self::Secondary => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Main => "main picture",
            Self::Secondary => "secondary picture",
        }
    }
}

pub fn policy(choice: LayoutChoice) -> Option<Role> {
    match choice {
        LayoutChoice::Fullscreen => None,
        LayoutChoice::SplitLeft | LayoutChoice::SplitRight => Some(Role::Main),
        LayoutChoice::PipBig | LayoutChoice::PipSmall => Some(Role::Secondary),
    }
}

fn selected(reading: &protocol::Reading) -> Result<Role, String> {
    if reading.status != "ok" || reading.max != Some(2) {
        return Err("The monitor audio reply does not match the supported profile (valid reply and maximum 2 required). No audio setting was sent.".into());
    }
    match reading.current {
        Some(0) => Ok(Role::Main),
        Some(1) => Ok(Role::Secondary),
        _ => Err("The monitor audio role is outside the supported main/secondary pair. No audio setting was sent.".into()),
    }
}

fn pause(milliseconds: u64, mock: bool) {
    if !mock {
        thread::sleep(Duration::from_millis(milliseconds));
    }
}

/// Dedupe is essential: repeating the firmware's secondary-audio setter can affect
/// another stored layout bit. Each call reads first and submits at most one Set.
pub fn apply(bus: &mut dyn Transport, role: &Role, mock: bool) -> Result<Value, String> {
    let mut result = json!({"ok":false,"mock":mock,"logical_role":role,"role_label":role.label(),
        "changed":false,"outcome_unknown":false,"setting_writes":0,"preflight":null,"write":null,"readback":null,"readbacks":[],
        "audible_verification_required":true});
    pause(150, mock);
    let preflight = diagnostics::read(bus, 0xe8)?;
    let previous = selected(&preflight.reading);
    result["preflight"] = json!(preflight);
    let previous = match previous {
        Ok(value) => value,
        Err(error) => {
            result["status"] = json!("refused");
            result["error"] = json!(error);
            return Ok(result);
        }
    };
    if previous == *role {
        result["ok"] = json!(true);
        result["status"] = json!("already_selected");
        return Ok(result);
    }
    pause(150, mock);
    let write = bus.write_once(&protocol::set_request(0xe8, role.value()))?;
    let submitted = write.write_status == 0;
    result["setting_writes"] = json!(1);
    result["outcome_unknown"] = json!(true);
    result["write"] = json!(write);
    if !submitted {
        result["status"] = json!("write_failed");
        result["error"] =
            json!("The monitor audio write failed. It was not retried; no rollback was sent.");
        return Ok(result);
    }
    // The G93SC can audibly switch before Get E8 updates. Poll only reads;
    // never resend the Set because a cached reply still names the old role.
    for delay in [250, 750, 1000, 2000] {
        pause(delay, mock);
        let readback = match diagnostics::read(bus, 0xe8) {
            Ok(value) => value,
            Err(error) => {
                result["status"] = json!("unverified_readback");
                result["error"] = json!(format!(
                    "Monitor audio was submitted, but its readback failed: {error}"
                ));
                return Ok(result);
            }
        };
        let observed = selected(&readback.reading);
        result["readback"] = json!(readback);
        result["readbacks"]
            .as_array_mut()
            .unwrap()
            .push(json!(readback));
        if observed == Ok(*role) {
            result["ok"] = json!(true);
            result["changed"] = json!(true);
            result["outcome_unknown"] = json!(false);
            result["status"] = json!("verified_readback");
            return Ok(result);
        }
        if observed.is_err() {
            break;
        }
    }
    result["status"] = json!("unverified_readback");
    result["error"] = json!(
        "The monitor did not report the requested audio role within the settling window. No setting retry or rollback was sent."
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Exchange, Transport};

    struct Bus {
        current: u16,
        max: u16,
        writes: Vec<Vec<u8>>,
        status: i32,
        update: bool,
        stale_after_write: usize,
    }

    impl Default for Bus {
        fn default() -> Self {
            Self {
                current: 0,
                max: 2,
                writes: vec![],
                status: 0,
                update: true,
                stale_after_write: 0,
            }
        }
    }

    impl Transport for Bus {
        fn exchange(&mut self, request: &[u8], _: usize, _: u64) -> Result<Exchange, String> {
            assert_eq!(request, protocol::request(0xe8));
            let [mh, ml] = self.max.to_be_bytes();
            let current = if !self.writes.is_empty() && self.stale_after_write > 0 {
                self.stale_after_write -= 1;
                0
            } else {
                self.current
            };
            let [ch, cl] = current.to_be_bytes();
            let mut bytes = vec![0x6e, 0x88, 2, 0, 0xe8, 0, mh, ml, ch, cl];
            bytes.push(bytes.iter().fold(0x50, |a, b| a ^ b));
            Ok(Exchange {
                request_hex: crate::transport::hex(request),
                response_hex: crate::transport::hex(&bytes),
                write_status: 0,
                read_status: Some(0),
                elapsed_ms: 0.0,
            })
        }

        fn write_once(&mut self, payload: &[u8]) -> Result<Exchange, String> {
            assert!(self.writes.is_empty(), "Audio must never retry a Set");
            self.writes.push(payload.to_vec());
            if self.status == 0 && self.update {
                self.current = u16::from_be_bytes([payload[3], payload[4]]);
            }
            Ok(Exchange {
                request_hex: crate::transport::hex(payload),
                response_hex: String::new(),
                write_status: self.status,
                read_status: None,
                elapsed_ms: 0.0,
            })
        }
    }

    #[test]
    fn secondary_readback_is_verified_and_repeat_is_a_noop() {
        let mut bus = Bus::default();
        let result = apply(&mut bus, &Role::Secondary, true).unwrap();
        assert_eq!(result["status"], "verified_readback");
        assert_eq!(result["changed"], true);
        assert_eq!(result["audible_verification_required"], true);
        assert_eq!(result["write"]["request_hex"], "8403e8000151");
        assert_eq!(bus.writes.len(), 1);
        assert_eq!(
            apply(&mut bus, &Role::Secondary, true).unwrap()["status"],
            "already_selected"
        );
        assert_eq!(bus.writes.len(), 1);
    }

    #[test]
    fn unsupported_profiles_and_role_two_never_write() {
        for (current, max) in [(0, 3), (2, 2)] {
            let mut bus = Bus {
                current,
                max,
                ..Default::default()
            };
            assert_eq!(
                apply(&mut bus, &Role::Secondary, true).unwrap()["ok"],
                false
            );
            assert!(bus.writes.is_empty());
        }
    }

    #[test]
    fn stale_audio_readback_only_repeats_reads() {
        let mut bus = Bus {
            stale_after_write: 1,
            ..Default::default()
        };
        let result = apply(&mut bus, &Role::Secondary, true).unwrap();
        assert_eq!(result["status"], "verified_readback");
        assert_eq!(result["readbacks"].as_array().unwrap().len(), 2);
        assert_eq!(bus.writes.len(), 1);
    }

    #[test]
    fn write_failure_and_failed_readback_never_retry() {
        let mut bus = Bus {
            status: -1,
            ..Default::default()
        };
        let result = apply(&mut bus, &Role::Secondary, true).unwrap();
        assert_eq!(result["status"], "write_failed");
        assert_eq!(result["write"]["write_status"], -1);
        assert_eq!(bus.writes.len(), 1);
        let mut bus = Bus {
            update: false,
            ..Default::default()
        };
        let result = apply(&mut bus, &Role::Secondary, true).unwrap();
        assert_eq!(result["status"], "unverified_readback");
        assert_eq!(result["changed"], false);
        assert_eq!(result["outcome_unknown"], true);
        assert_eq!(bus.writes.len(), 1);
    }

    #[test]
    fn policy_follows_picture_roles_not_this_mac_position() {
        assert_eq!(policy(LayoutChoice::Fullscreen), None);
        for choice in [LayoutChoice::PipBig, LayoutChoice::PipSmall] {
            assert_eq!(policy(choice), Some(Role::Secondary));
        }
        for choice in [LayoutChoice::SplitLeft, LayoutChoice::SplitRight] {
            assert_eq!(policy(choice), Some(Role::Main));
        }
    }
}
