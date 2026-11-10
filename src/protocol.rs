use serde::Serialize;

pub fn number(value: &str) -> Result<u16, String> {
    let result = if let Some(hex) = value.strip_prefix("0x").or(value.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16)
    } else {
        value.parse()
    };
    result.map_err(|_| format!("invalid 16-bit number: {value}"))
}

pub fn request(code: u8) -> Vec<u8> {
    vec![0x82, 0x01, code, 0x6e ^ 0x51 ^ 0x82 ^ 0x01 ^ code]
}

pub fn caps_request(offset: u16) -> Vec<u8> {
    let hi = (offset >> 8) as u8;
    let lo = offset as u8;
    vec![0x83, 0xf3, hi, lo, 0x6e ^ 0x51 ^ 0x83 ^ 0xf3 ^ hi ^ lo]
}

pub fn set_request(code: u8, value: u16) -> Vec<u8> {
    let [hi, lo] = value.to_be_bytes();
    vec![
        0x84,
        0x03,
        code,
        hi,
        lo,
        0x6e ^ 0x51 ^ 0x84 ^ 0x03 ^ code ^ hi ^ lo,
    ]
}

/// Only enable an already selected PiP layout; never select an untested layout.
pub fn pip_enabled(reading: &Reading) -> Result<bool, String> {
    if reading.status != "ok" || reading.max != Some(0x007f) {
        return Err("E2 does not match the verified G93SC profile (valid reply and max 0x007F required); no write sent".into());
    }
    let current = reading
        .current
        .ok_or("E2 current value missing; no write sent")?;
    if current >> 8 != 0x35 {
        return Err(
            "E2 does not match the verified G93SC profile (high byte 0x35 required); no write sent"
                .into(),
        );
    }
    if (current >> 1) & 7 != 7 {
        return Err("Saved layout is not PiP; selecting a different layout is unverified and disabled. No write sent".into());
    }
    Ok(current & 1 != 0)
}

pub fn unhex(s: &str) -> Result<Vec<u8>, String> {
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.len() % 2 != 0 || !compact.is_ascii() {
        return Err("invalid hex buffer".into());
    }
    (0..compact.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&compact[i..i + 2], 16).map_err(|_| "invalid hex buffer".into())
        })
        .collect()
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Reading {
    pub status: String,
    pub current: Option<u16>,
    pub max: Option<u16>,
    pub result_code: Option<u8>,
    pub frame_offset: Option<usize>,
}

impl Reading {
    pub fn failed(status: &str) -> Self {
        Self {
            status: status.into(),
            current: None,
            max: None,
            result_code: None,
            frame_offset: None,
        }
    }
}

/// Strict checksummed frames only: do not turn driver success into protocol success.
pub fn parse(code: u8, bytes: &[u8]) -> Reading {
    let mut malformed = false;
    let mut stale = false;
    for (offset, frame) in bytes.windows(11).enumerate() {
        if frame[0] != 0x6e || frame[1] != 0x88 || frame[2] != 2 {
            continue;
        }
        let checksummed = frame.iter().fold(0x50, |sum, b| sum ^ b) == 0;
        if frame[4] != code {
            stale |= checksummed;
            continue;
        }
        if !checksummed {
            malformed = true;
            continue;
        }
        let status = match frame[3] {
            0 => "ok",
            1 => "unsupported",
            _ => "monitor_error",
        };
        return Reading {
            status: status.into(),
            current: (frame[3] == 0).then(|| u16::from_be_bytes([frame[8], frame[9]])),
            max: (frame[3] == 0).then(|| u16::from_be_bytes([frame[6], frame[7]])),
            result_code: Some(frame[3]),
            frame_offset: Some(offset),
        };
    }
    if malformed {
        return Reading::failed("invalid_checksum");
    }
    if stale {
        return Reading::failed("stale_reply");
    }
    if bytes
        .windows(3)
        .any(|f| f[0] == 0x6e && f[1] == 0x80 && (0x50 ^ f[0] ^ f[1] ^ f[2]) == 0)
    {
        return Reading::failed("null_response");
    }
    Reading::failed("unrecognized_response")
}

/// Verified only for the matching G93SC profile and an already selected PiP layout.
pub const WRITE_ALLOWLIST: &[(u8, u16)] = &[(0xe2, 1)];

#[cfg(test)]
mod tests {
    use super::*;
    fn reply(code: u8, result: u8) -> Vec<u8> {
        let mut b = vec![0x6e, 0x88, 2, result, code, 0, 0, 3, 0, 5];
        b.push(b.iter().fold(0x50, |a, b| a ^ b));
        b
    }
    #[test]
    fn valid_and_unsupported_are_distinct() {
        let b = reply(0x60, 0);
        assert_eq!(parse(0x60, &b).current, Some(5));
        assert_eq!(parse(0x60, &reply(0x60, 1)).status, "unsupported");
        let mut bad = b.clone();
        bad[10] ^= 1;
        assert_eq!(parse(0x60, &bad).status, "invalid_checksum");
        assert_eq!(parse(0xe2, &b).status, "stale_reply");
        let mut prefix = vec![0xff, 0, 1];
        prefix.extend(b);
        assert_eq!(parse(0x60, &prefix).frame_offset, Some(3));
        assert_eq!(parse(0xe2, &[0x6e, 0x80, 0xbe]).status, "null_response");
    }
    #[test]
    fn captured_stale_e3_reply_never_supplies_e2_values() {
        let bytes = unhex(&"6e880200e300515100104700".repeat(4)).unwrap();
        let stale = parse(0xe2, &bytes);
        assert_eq!(stale.status, "stale_reply");
        assert_eq!(
            (
                stale.current,
                stale.max,
                stale.result_code,
                stale.frame_offset
            ),
            (None, None, None, None)
        );
        let mut mixed = bytes;
        mixed.extend(reply(0xe2, 0));
        assert_eq!(parse(0xe2, &mixed).status, "ok");
        assert_eq!(parse(0xe2, &mixed).current, Some(5));
        let mut bad = reply(0xe2, 0);
        bad[10] ^= 1;
        mixed.truncate(48);
        mixed.extend(bad);
        assert_eq!(parse(0xe2, &mixed).status, "invalid_checksum");
    }
    #[test]
    fn read_packets_and_bounds() {
        assert_eq!(request(0x60), vec![0x82, 1, 0x60, 0xdc]);
        assert!(number("65536").is_err());
        assert_eq!(number("0xFFFF"), Ok(65535));
        assert!(unhex("é").is_err());
        assert_eq!(WRITE_ALLOWLIST, &[(0xe2, 1)]);
        assert_eq!(set_request(0xe2, 1), vec![0x84, 3, 0xe2, 0, 1, 0x5b]);
    }
    #[test]
    fn pip_profile_requires_current_mode_and_exact_fingerprint() {
        let mut reading = Reading {
            status: "ok".into(),
            current: Some(0x35ce),
            max: Some(0x7f),
            result_code: Some(0),
            frame_offset: Some(0),
        };
        assert_eq!(pip_enabled(&reading), Ok(false));
        reading.current = Some(0x35cf);
        assert_eq!(pip_enabled(&reading), Ok(true));
        for current in [0x35cc, 0x34ce, 0x00ce] {
            reading.current = Some(current);
            assert!(pip_enabled(&reading).is_err());
        }
        reading.current = Some(0x35ce);
        reading.max = Some(0xff);
        assert!(pip_enabled(&reading).is_err());
        reading.max = Some(0x7f);
        reading.status = "unsupported".into();
        assert!(pip_enabled(&reading).is_err());
    }
}
