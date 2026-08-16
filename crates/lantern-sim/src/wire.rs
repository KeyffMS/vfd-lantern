use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use lantern_app::MonotonicClock;
use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    task::JoinHandle,
};
use tokio_serial::SerialStream;
use tokio_util::sync::CancellationToken;

use crate::{ScheduledWireFaultV1, SimulatorError, WireFaultKindV1, pty::connected_serial_pair};

/// One applied byte-level wire mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WireFaultRecord {
    pub response_index: u64,
    pub fault: String,
    pub original_hex: String,
    pub emitted_hex: String,
}

/// Byte proxy placed only between the normal RTU server and the client PTY.
///
/// It has no register map, profile parser, or Modbus service implementation.
/// Requests are forwarded unchanged; selected response frames are mutated after
/// the real `tokio-modbus` server has encoded them.
pub struct WireFaultHarness {
    cancellation: CancellationToken,
    task: JoinHandle<Result<(), SimulatorError>>,
    records: Arc<Mutex<Vec<WireFaultRecord>>>,
}

impl WireFaultHarness {
    pub fn spawn(
        faults: &[ScheduledWireFaultV1],
        clock: Arc<dyn MonotonicClock>,
    ) -> Result<WireTopology, SimulatorError> {
        let (server_stream, server_peer, _) = connected_serial_pair()?;
        let (client_peer, client_guard, client_path) = connected_serial_pair()?;

        let schedule = faults
            .iter()
            .map(|item| (item.response_index, item.fault.clone()))
            .collect::<BTreeMap<_, _>>();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let records = Arc::new(Mutex::new(Vec::new()));
        let task_records = Arc::clone(&records);
        let task = tokio::spawn(async move {
            run_proxy(
                server_peer,
                client_peer,
                schedule,
                clock,
                task_cancellation,
                task_records,
            )
            .await
        });

        Ok(WireTopology {
            server_stream,
            client_guard,
            client_path,
            harness: Self {
                cancellation,
                task,
                records,
            },
        })
    }

    /// Returns all applied mutations in response order.
    #[must_use]
    pub fn records(&self) -> Vec<WireFaultRecord> {
        self.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Requests byte-proxy shutdown without waiting.
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Stops the byte proxy and waits for deterministic descriptor closure.
    pub async fn shutdown(self) -> Result<(), SimulatorError> {
        self.cancel();
        self.task
            .await
            .map_err(|error| SimulatorError::Task(error.to_string()))?
    }
}

/// Server endpoint, client PTY path, and the isolated fault proxy.
pub struct WireTopology {
    server_stream: SerialStream,
    client_guard: SerialStream,
    client_path: PathBuf,
    harness: WireFaultHarness,
}

impl WireTopology {
    pub(crate) fn into_parts(self) -> (SerialStream, SerialStream, PathBuf, WireFaultHarness) {
        (
            self.server_stream,
            self.client_guard,
            self.client_path,
            self.harness,
        )
    }

    #[must_use]
    pub fn client_path(&self) -> &Path {
        &self.client_path
    }
}

async fn run_proxy(
    server_peer: SerialStream,
    client_peer: SerialStream,
    schedule: BTreeMap<u64, WireFaultKindV1>,
    clock: Arc<dyn MonotonicClock>,
    cancellation: CancellationToken,
    records: Arc<Mutex<Vec<WireFaultRecord>>>,
) -> Result<(), SimulatorError> {
    let (server_read, server_write) = tokio::io::split(server_peer);
    let (client_read, client_write) = tokio::io::split(client_peer);
    let requests = forward_requests(client_read, server_write);
    let responses = forward_responses(server_read, client_write, schedule, clock, records);
    tokio::pin!(requests);
    tokio::pin!(responses);

    tokio::select! {
        () = cancellation.cancelled() => Ok(()),
        result = &mut requests => result,
        result = &mut responses => result,
    }
}

async fn forward_requests<R, W>(mut reader: R, mut writer: W) -> Result<(), SimulatorError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    tokio::io::copy(&mut reader, &mut writer)
        .await
        .map_err(|error| SimulatorError::Runtime(format!("wire request proxy failed: {error}")))?;
    writer
        .shutdown()
        .await
        .map_err(|error| SimulatorError::Runtime(format!("wire request shutdown failed: {error}")))
}

async fn forward_responses<R, W>(
    mut reader: R,
    mut writer: W,
    schedule: BTreeMap<u64, WireFaultKindV1>,
    clock: Arc<dyn MonotonicClock>,
    records: Arc<Mutex<Vec<WireFaultRecord>>>,
) -> Result<(), SimulatorError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut response_index = 0_u64;
    while let Some(frame) = read_response_frame(&mut reader).await? {
        response_index = response_index.saturating_add(1);
        let original = frame.clone();
        let fault = schedule.get(&response_index).cloned();
        let mut emitted = frame;
        let mut delay = Duration::ZERO;
        let mut inter_byte_gap = None;
        if let Some(fault) = &fault {
            apply_fault(&mut emitted, fault, &mut delay, &mut inter_byte_gap)?;
        }
        if let Some(fault) = &fault {
            records
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(WireFaultRecord {
                    response_index,
                    fault: format!("{fault:?}"),
                    original_hex: hex(&original),
                    emitted_hex: hex(&emitted),
                });
        }
        if !delay.is_zero() {
            clock.sleep(delay).await;
        }
        if let Some(gap) = inter_byte_gap {
            for byte in &emitted {
                writer
                    .write_all(&[*byte])
                    .await
                    .map_err(proxy_write_error)?;
                writer.flush().await.map_err(proxy_write_error)?;
                clock.sleep(gap).await;
            }
        } else {
            writer
                .write_all(&emitted)
                .await
                .map_err(proxy_write_error)?;
            writer.flush().await.map_err(proxy_write_error)?;
        }
    }
    Ok(())
}

async fn read_response_frame<R>(reader: &mut R) -> Result<Option<Vec<u8>>, SimulatorError>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0_u8; 2];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => {
            return Err(SimulatorError::Runtime(format!(
                "wire response header read failed: {error}"
            )));
        }
    }
    let mut frame = header.to_vec();
    let function = header[1];
    if function & 0x80 != 0 {
        read_more(reader, &mut frame, 3).await?;
        return Ok(Some(frame));
    }
    match function {
        0x03 | 0x04 => {
            let mut count = [0_u8; 1];
            reader
                .read_exact(&mut count)
                .await
                .map_err(proxy_read_error)?;
            frame.push(count[0]);
            read_more(reader, &mut frame, usize::from(count[0]) + 2).await?;
        }
        0x06 | 0x10 => read_more(reader, &mut frame, 6).await?,
        _ => read_more(reader, &mut frame, 3).await?,
    }
    Ok(Some(frame))
}

async fn read_more<R>(
    reader: &mut R,
    frame: &mut Vec<u8>,
    length: usize,
) -> Result<(), SimulatorError>
where
    R: AsyncRead + Unpin,
{
    let start = frame.len();
    frame.resize(start + length, 0);
    reader
        .read_exact(&mut frame[start..])
        .await
        .map_err(proxy_read_error)?;
    Ok(())
}

fn apply_fault(
    frame: &mut Vec<u8>,
    fault: &WireFaultKindV1,
    delay: &mut Duration,
    inter_byte_gap: &mut Option<Duration>,
) -> Result<(), SimulatorError> {
    match fault {
        WireFaultKindV1::BadCrc => {
            let Some(last) = frame.last_mut() else {
                return Err(SimulatorError::Runtime(
                    "cannot corrupt an empty frame".to_owned(),
                ));
            };
            *last ^= 0x01;
        }
        WireFaultKindV1::Truncated { bytes } => {
            let keep = frame.len().saturating_sub(*bytes).max(1);
            frame.truncate(keep);
        }
        WireFaultKindV1::WrongLength => {
            if frame.len() < 5 || !matches!(frame[1], 0x03 | 0x04) {
                return Err(SimulatorError::Runtime(
                    "wrong_length requires an FC03/FC04 response".to_owned(),
                ));
            }
            frame[2] = frame[2].saturating_add(2);
            replace_crc(frame);
        }
        WireFaultKindV1::WrongFunction { function } => {
            if frame.len() < 4 {
                return Err(SimulatorError::Runtime("frame is too short".to_owned()));
            }
            frame[1] = *function;
            replace_crc(frame);
        }
        WireFaultKindV1::WrongSlave { slave } => {
            if frame.len() < 4 {
                return Err(SimulatorError::Runtime("frame is too short".to_owned()));
            }
            frame[0] = *slave;
            replace_crc(frame);
        }
        WireFaultKindV1::UnexpectedWords { words } => {
            if frame.len() < 5 || !matches!(frame[1], 0x03 | 0x04) {
                return Err(SimulatorError::Runtime(
                    "unexpected_words requires an FC03/FC04 response".to_owned(),
                ));
            }
            let slave = frame[0];
            let function = frame[1];
            frame.clear();
            frame.push(slave);
            frame.push(function);
            frame.push(
                u8::try_from(words.len().saturating_mul(2)).map_err(|error| {
                    SimulatorError::Runtime(format!("wire word count overflow: {error}"))
                })?,
            );
            for word in words {
                frame.extend_from_slice(&word.to_be_bytes());
            }
            append_crc(frame);
        }
        WireFaultKindV1::Delay { milliseconds } => {
            *delay = Duration::from_millis(*milliseconds);
        }
        WireFaultKindV1::InterByteGap { microseconds } => {
            *inter_byte_gap = Some(Duration::from_micros(*microseconds));
        }
    }
    Ok(())
}

fn replace_crc(frame: &mut Vec<u8>) {
    if frame.len() >= 2 {
        frame.truncate(frame.len() - 2);
        append_crc(frame);
    }
}

fn append_crc(frame: &mut Vec<u8>) {
    let crc = modbus_crc(frame);
    frame.extend_from_slice(&crc.to_le_bytes());
}

fn modbus_crc(bytes: &[u8]) -> u16 {
    let mut crc = 0xFFFF_u16;
    for byte in bytes {
        crc ^= u16::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

fn proxy_read_error(error: io::Error) -> SimulatorError {
    SimulatorError::Runtime(format!("wire response read failed: {error}"))
}

fn proxy_write_error(error: io::Error) -> SimulatorError {
    SimulatorError::Runtime(format!("wire response write failed: {error}"))
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::{WireFaultKindV1, wire::modbus_crc};

    #[test]
    fn crc_matches_modbus_reference_vector() {
        assert_eq!(modbus_crc(&[0x01, 0x03, 0x00, 0x00, 0x00, 0x0A]), 0xCDC5);
    }

    #[test]
    fn delay_fault_is_separate_from_frame_mutation() {
        let mut frame = vec![1, 3, 2, 0, 1, 0, 0];
        let mut delay = Duration::ZERO;
        let mut gap = None;
        super::apply_fault(
            &mut frame,
            &WireFaultKindV1::Delay { milliseconds: 7 },
            &mut delay,
            &mut gap,
        )
        .expect("delay");
        assert_eq!(delay, Duration::from_millis(7));
        assert!(gap.is_none());
    }
}
