use crate::{
    display::Display,
    protocol,
    transport::{Exchange, Transport},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Write,
    path::Path,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Serialize)]
pub struct Record {
    pub vcp: String,
    #[serde(flatten)]
    pub reading: protocol::Reading,
    pub attempts: Vec<Exchange>,
}

pub fn read(bus: &mut dyn Transport, code: u8) -> Result<Record, String> {
    read_with_pause(bus, code, thread::sleep)
}

fn read_with_pause(
    bus: &mut dyn Transport,
    code: u8,
    mut pause: impl FnMut(Duration),
) -> Result<Record, String> {
    tracing::info!(vcp = format_args!("0x{code:02X}"), "starting Get VCP");
    let mut attempts = Vec::new();
    let mut reading = protocol::Reading::failed("not_attempted");
    for attempt in 0..5 {
        let exchange = bus.exchange(&protocol::request(code), 48, 100)?;
        reading = if exchange.write_status != 0 || exchange.read_status != Some(0) {
            protocol::Reading::failed("transport_error")
        } else {
            protocol::parse(code, &protocol::unhex(&exchange.response_hex)?)
        };
        attempts.push(exchange);
        if reading.status == "transport_error" {
            break;
        }
        if reading.status == "ok" || reading.status == "unsupported" {
            break;
        }
        // Only a valid reply for a different feature earns the longer drain/settle
        // window. Other protocol errors retain their original two-attempt limit.
        let stale = reading.status == "stale_reply";
        let limit = if stale { 5 } else { 2 };
        if attempt + 1 >= limit {
            break;
        }
        let delay = if stale {
            [150, 300, 600, 1200][attempt]
        } else {
            150
        };
        pause(Duration::from_millis(delay));
    }
    tracing::info!(vcp = format_args!("0x{code:02X}"), status = %reading.status, current = ?reading.current);
    Ok(Record {
        vcp: format!("0x{code:02X}"),
        reading,
        attempts,
    })
}

#[derive(Serialize)]
pub struct Scan {
    pub reads: Vec<Record>,
    pub complete: bool,
    pub expected_count: usize,
    pub stopped_reason: Option<String>,
}

pub fn scan(bus: &mut dyn Transport, all: bool, journal: Option<&Path>) -> Result<Scan, String> {
    let codes: Vec<u8> = if all {
        (0..=255).collect()
    } else {
        let mut c = vec![
            0x00, 0x02, 0x08, 0x09, 0x0a, 0x0d, 0x0f, 0x10, 0x12, 0x14, 0x16, 0x18, 0x1a, 0x23,
            0x25, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f, 0x52, 0x60, 0x62, 0x87, 0x8d,
            0xaa, 0xac, 0xae, 0xb6, 0xc0, 0xc6, 0xc8, 0xc9, 0xca, 0xcc, 0xd6, 0xdb, 0xdc, 0xdf,
        ];
        c.extend(0xe0..=0xff);
        c
    };
    let expected_count = codes.len();
    let mut journal = journal
        .map(|path| {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|e| e.to_string())
        })
        .transpose()?;
    let started = Instant::now();
    let mut failures = 0;
    let mut stopped_reason = None;
    let mut records = Vec::new();
    for code in codes {
        let record = read(bus, code)?;
        if let Some(file) = &mut journal {
            serde_json::to_writer(&mut *file, &record).map_err(|e| e.to_string())?;
            writeln!(file)
                .and_then(|_| file.flush())
                .map_err(|e| e.to_string())?;
        }
        failures = if record.reading.status == "transport_error" {
            failures + 1
        } else {
            0
        };
        records.push(record);
        if failures >= 3 {
            stopped_reason = Some("three consecutive transport failures".into());
            break;
        }
        if started.elapsed() > Duration::from_secs(120) {
            stopped_reason =
                Some("120-second scan budget exhausted; checked between synchronous calls".into());
            break;
        }
        thread::sleep(Duration::from_millis(150));
    }
    let complete = records.len() == expected_count;
    Ok(Scan {
        reads: records,
        complete,
        expected_count,
        stopped_reason,
    })
}

/// Offset-checked, checksummed, bounded capability transfer; stops on first anomaly.
pub fn capabilities(bus: &mut dyn Transport) -> Result<Value, String> {
    let mut attempts = Vec::new();
    let mut content = Vec::new();
    let mut complete = false;
    let mut status = "chunk_limit";
    for _ in 0..16 {
        let exchange = bus.exchange(&protocol::caps_request(content.len() as u16), 64, 150)?;
        let raw = protocol::unhex(&exchange.response_hex)?;
        let transport_ok = exchange.write_status == 0 && exchange.read_status == Some(0);
        attempts.push(exchange);
        if !transport_ok {
            status = "transport_error";
            break;
        }
        let mut chunk = None;
        for off in 0..raw.len().saturating_sub(5) {
            if raw[off] != 0x6e || raw[off + 1] & 0x80 == 0 || raw[off + 2] != 0xe3 {
                continue;
            }
            let len = (raw[off + 1] & 0x7f) as usize;
            if len < 3 || off + len + 3 > raw.len() {
                continue;
            }
            let frame = &raw[off..off + len + 3];
            if frame.iter().fold(0x50, |a, b| a ^ b) != 0 {
                continue;
            }
            if u16::from_be_bytes([frame[3], frame[4]]) != content.len() as u16 {
                continue;
            }
            chunk = Some(frame[5..frame.len() - 1].to_vec());
            break;
        }
        match chunk {
            Some(data) if data.is_empty() => {
                complete = true;
                status = "ok";
                break;
            }
            Some(data) => content.extend(data),
            None => {
                status = "unrecognized_response";
                break;
            }
        }
        thread::sleep(Duration::from_millis(150));
    }
    Ok(
        json!({"status":status,"complete":complete,"text":String::from_utf8_lossy(&content),"attempts":attempts}),
    )
}

pub fn capture(
    bus: &mut dyn Transport,
    display: &Display,
    label: &str,
    all: bool,
    journal: &Path,
) -> Result<Value, String> {
    // Capabilities are intentionally separate: some Samsung panels desynchronize on long caps reads.
    let result = scan(bus, all, Some(journal))?;
    Ok(
        json!({"schema_version":1,"label":label,"unix_seconds":SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e|e.to_string())?.as_secs(),"display":display,"read_only":true,"reads":result.reads,"complete":result.complete,"expected_count":result.expected_count,"stopped_reason":result.stopped_reason,"journal":journal,"capabilities":"Run capabilities explicitly; excluded from repeated captures"}),
    )
}

#[derive(Deserialize)]
pub struct Fixture {
    pub display: Display,
    pub replies: BTreeMap<String, String>,
    #[serde(default)]
    pub after_write: BTreeMap<String, String>,
    #[serde(default)]
    pub write_status: i32,
}
#[derive(Default)]
pub struct MockTransport {
    pub replies: BTreeMap<String, String>,
    pub after_write: BTreeMap<String, String>,
    pub write_status: i32,
    pub writes: Vec<String>,
    write_used: bool,
}
impl MockTransport {
    pub fn from_fixture(fixture: Fixture) -> Self {
        Self {
            replies: fixture.replies,
            after_write: fixture.after_write,
            write_status: fixture.write_status,
            ..Self::default()
        }
    }
}
impl Transport for MockTransport {
    fn write_once(&mut self, payload: &[u8]) -> Result<Exchange, String> {
        if std::mem::replace(&mut self.write_used, true) {
            return Err("Setting write attempt already consumed for this transport".into());
        }
        crate::transport::validate_set_request(payload)?;
        self.writes.push(crate::transport::hex(payload));
        if self.write_status == 0 {
            self.replies.extend(self.after_write.clone());
        }
        Ok(Exchange {
            request_hex: crate::transport::hex(payload),
            response_hex: String::new(),
            write_status: self.write_status,
            read_status: None,
            elapsed_ms: 0.0,
        })
    }
    fn exchange(&mut self, payload: &[u8], read_len: usize, _: u64) -> Result<Exchange, String> {
        crate::transport::validate_request(payload, 0x51, read_len)?;
        let key = if payload.get(1) == Some(&1) {
            format!("0x{:02X}", payload[2])
        } else {
            format!("caps:{}", u16::from_be_bytes([payload[2], payload[3]]))
        };
        let response = self
            .replies
            .get(&key)
            .cloned()
            .unwrap_or_else(|| "6e80be".into());
        Ok(Exchange {
            request_hex: payload.iter().map(|b| format!("{b:02x}")).collect(),
            response_hex: response,
            write_status: 0,
            read_status: Some(0),
            elapsed_ms: 0.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct ScriptedReads {
        responses: VecDeque<(String, i32, Option<i32>)>,
        gets: Vec<Vec<u8>>,
        sets: usize,
    }
    impl ScriptedReads {
        fn new(responses: Vec<(String, i32, Option<i32>)>) -> Self {
            Self {
                responses: responses.into(),
                gets: vec![],
                sets: 0,
            }
        }
    }
    impl Transport for ScriptedReads {
        fn exchange(&mut self, payload: &[u8], _: usize, _: u64) -> Result<Exchange, String> {
            self.gets.push(payload.to_vec());
            let (response_hex, write_status, read_status) =
                self.responses.pop_front().expect("unexpected extra Get");
            Ok(Exchange {
                request_hex: crate::transport::hex(payload),
                response_hex,
                write_status,
                read_status,
                elapsed_ms: 0.0,
            })
        }
        fn write_once(&mut self, _: &[u8]) -> Result<Exchange, String> {
            self.sets += 1;
            Err("A read must never issue a Set".into())
        }
    }
    fn get_reply(result: u8) -> String {
        let mut bytes = vec![0x6e, 0x88, 2, result, 0xe2, 0, 0, 0x7f, 0x35, 0x11];
        bytes.push(bytes.iter().fold(0x50, |a, b| a ^ b));
        crate::transport::hex(&bytes)
    }
    fn stale_e3() -> String {
        "6e880200e300515100104700".repeat(4)
    }

    #[test]
    fn two_captured_stale_replies_then_e2_need_three_gets_and_no_set() {
        let mut bus = ScriptedReads::new(vec![
            (stale_e3(), 0, Some(0)),
            (stale_e3(), 0, Some(0)),
            (get_reply(0), 0, Some(0)),
        ]);
        let mut waits = vec![];
        let record =
            read_with_pause(&mut bus, 0xe2, |delay| waits.push(delay.as_millis())).unwrap();
        assert_eq!(record.reading.status, "ok");
        assert_eq!(record.reading.current, Some(0x3511));
        assert_eq!(record.attempts.len(), 3);
        assert_eq!(bus.gets, vec![protocol::request(0xe2); 3]);
        assert_eq!(bus.sets, 0);
        assert_eq!(waits, [150, 300]);
    }
    #[test]
    fn stale_replies_have_five_get_cap_and_no_sleep_after_last() {
        let mut bus = ScriptedReads::new(vec![(stale_e3(), 0, Some(0)); 5]);
        let mut waits = vec![];
        let record =
            read_with_pause(&mut bus, 0xe2, |delay| waits.push(delay.as_millis())).unwrap();
        assert_eq!(record.reading.status, "stale_reply");
        assert_eq!((record.reading.current, record.reading.max), (None, None));
        assert_eq!(bus.gets.len(), 5);
        assert_eq!(bus.sets, 0);
        assert_eq!(waits, [150, 300, 600, 1200]);
    }
    #[test]
    fn unsupported_and_transport_failure_stop_and_other_errors_keep_two_attempts() {
        for response in [(get_reply(1), 0, Some(0)), (String::new(), -1, None)] {
            let mut bus = ScriptedReads::new(vec![response]);
            let record =
                read_with_pause(&mut bus, 0xe2, |_| panic!("No wait after terminal result"))
                    .unwrap();
            assert_eq!(record.attempts.len(), 1);
            assert_eq!(bus.sets, 0);
        }
        let mut bus = ScriptedReads::new(vec![("6e80be".into(), 0, Some(0)); 2]);
        let mut waits = vec![];
        let record =
            read_with_pause(&mut bus, 0xe2, |delay| waits.push(delay.as_millis())).unwrap();
        assert_eq!(record.reading.status, "null_response");
        assert_eq!(bus.gets.len(), 2);
        assert_eq!(waits, [150]);
        let mut bus = ScriptedReads::new(vec![
            (stale_e3(), 0, Some(0)),
            (stale_e3(), 0, Some(0)),
            ("6e80be".into(), 0, Some(0)),
        ]);
        let record = read_with_pause(&mut bus, 0xe2, |_| {}).unwrap();
        assert_eq!(record.reading.status, "null_response");
        assert_eq!(
            bus.gets.len(),
            3,
            "a non-stale error must not inherit the five-attempt policy"
        );
    }
    #[test]
    fn capability_chunks_require_matching_offset_and_checksum() {
        fn frame(offset: u16, data: &[u8]) -> String {
            let mut bytes = vec![
                0x6e,
                0x80 | (3 + data.len()) as u8,
                0xe3,
                (offset >> 8) as u8,
                offset as u8,
            ];
            bytes.extend(data);
            bytes.push(bytes.iter().fold(0x50, |a, b| a ^ b));
            crate::transport::hex(&bytes)
        }
        let mut bus = MockTransport {
            replies: BTreeMap::from([
                ("caps:0".into(), frame(0, b"abc")),
                ("caps:3".into(), frame(3, b"")),
            ]),
            ..Default::default()
        };
        let result = capabilities(&mut bus).unwrap();
        assert_eq!(result["complete"], true);
        assert_eq!(result["text"], "abc");
        bus.replies.insert("caps:3".into(), frame(4, b""));
        assert_eq!(capabilities(&mut bus).unwrap()["complete"], false);
        // Even a checksummed reply needs the mandatory length high bit.
        bus.replies.insert("caps:0".into(), "6e03e30000de".into());
        assert_eq!(capabilities(&mut bus).unwrap()["complete"], false);
    }
    #[test]
    fn scan_stops_after_three_transport_failures() {
        struct FailedBus;
        impl Transport for FailedBus {
            fn exchange(&mut self, p: &[u8], _: usize, _: u64) -> Result<Exchange, String> {
                Ok(Exchange {
                    request_hex: crate::transport::hex(p),
                    response_hex: String::new(),
                    write_status: -1,
                    read_status: None,
                    elapsed_ms: 0.0,
                })
            }
        }
        let result = scan(&mut FailedBus, true, None).unwrap();
        assert!(!result.complete);
        assert_eq!(result.reads.len(), 3);
        assert!(result.stopped_reason.unwrap().contains("three consecutive"));
    }
    #[test]
    fn mock_uses_same_decoder() {
        let mut bytes = vec![0x6e, 0x88, 2, 0, 0xe2, 0, 0, 0xff, 0x39, 1];
        bytes.push(bytes.iter().fold(0x50, |a, b| a ^ b));
        let hex = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut bus = MockTransport {
            replies: BTreeMap::from([("0xE2".into(), hex)]),
            ..Default::default()
        };
        let r = read(&mut bus, 0xe2).unwrap();
        assert_eq!(r.reading.current, Some(0x3901));
    }
}
