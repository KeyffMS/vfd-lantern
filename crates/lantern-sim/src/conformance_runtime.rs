use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use lantern_app::{MonotonicClock, TokioMonotonicClock};
use lantern_profile::ValidatedDeviceProfile;
use serde::Serialize;
use tokio::task::JoinHandle;
use tokio_modbus::server::{Terminated, rtu::Server};
use tokio_util::sync::CancellationToken;

use crate::{
    ConformanceSimulatorControl, ConformanceSimulatorService, LoadedConformanceScenario,
    LoadedScenario, SimulatorError, SimulatorLogRecord, SimulatorPty, WireFaultHarness,
    WireFaultRecord,
};

/// Machine-readable identity for a full-product conformance simulator instance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConformanceSimulatorHandshake {
    pub pty: PathBuf,
    pub profile_hash: String,
    pub core_scenario_hash: String,
    pub conformance_scenario_hash: String,
    pub seed: String,
    pub fingerprint: String,
}

/// #27 runtime using real PTY/RTU while preserving the read-only #20 runtime.
pub struct ConformanceSimulatorRuntime {
    handshake: ConformanceSimulatorHandshake,
    control: ConformanceSimulatorControl,
    stop: CancellationToken,
    terminated: CancellationToken,
    server_task: Option<JoinHandle<Result<Terminated, std::io::Error>>>,
    client_guard: Option<tokio_serial::SerialStream>,
    wire: Option<WireFaultHarness>,
    completed_wire_records: Vec<WireFaultRecord>,
}

impl ConformanceSimulatorRuntime {
    pub fn spawn(
        profile: Arc<ValidatedDeviceProfile>,
        core_scenario: Arc<LoadedScenario>,
        conformance: Arc<LoadedConformanceScenario>,
    ) -> Result<Self, SimulatorError> {
        Self::spawn_with_clock(
            profile,
            core_scenario,
            conformance,
            Arc::new(TokioMonotonicClock),
        )
    }

    pub fn spawn_with_clock(
        profile: Arc<ValidatedDeviceProfile>,
        core_scenario: Arc<LoadedScenario>,
        conformance: Arc<LoadedConformanceScenario>,
        clock: Arc<dyn MonotonicClock>,
    ) -> Result<Self, SimulatorError> {
        let stop = CancellationToken::new();
        let terminated = CancellationToken::new();
        let (server_stream, client_guard, client_path, wire) =
            if core_scenario.wire_faults().is_empty() {
                let pty = SimulatorPty::direct()?;
                let (server, guard, path) = pty.into_parts();
                (server, guard, path, None)
            } else {
                let topology =
                    WireFaultHarness::spawn(core_scenario.wire_faults(), Arc::clone(&clock))?;
                let (server, guard, path, harness) = topology.into_parts();
                (server, guard, path, Some(harness))
            };
        let (service, control) = ConformanceSimulatorService::new(
            Arc::clone(&profile),
            Arc::clone(&core_scenario),
            Arc::clone(&conformance),
            clock,
            stop.clone(),
        )?;
        let task_stop = stop.clone();
        let task_terminated = terminated.clone();
        let server_task = tokio::spawn(async move {
            let result = Server::new(server_stream)
                .serve_until(service, task_stop.cancelled_owned())
                .await;
            task_terminated.cancel();
            result
        });
        let handshake = ConformanceSimulatorHandshake {
            pty: client_path,
            profile_hash: profile.profile_hash().to_hex(),
            core_scenario_hash: core_scenario.hash().to_hex(),
            conformance_scenario_hash: conformance.hash().to_hex(),
            seed: hex(&core_scenario.seed()),
            fingerprint: core_scenario.fingerprint().to_string(),
        };
        Ok(Self {
            handshake,
            control,
            stop,
            terminated,
            server_task: Some(server_task),
            client_guard: Some(client_guard),
            wire,
            completed_wire_records: Vec::new(),
        })
    }

    #[must_use]
    pub const fn handshake(&self) -> &ConformanceSimulatorHandshake {
        &self.handshake
    }

    #[must_use]
    pub fn client_path(&self) -> &Path {
        &self.handshake.pty
    }

    #[must_use]
    pub const fn control(&self) -> &ConformanceSimulatorControl {
        &self.control
    }

    #[must_use]
    pub const fn uses_wire_fault_harness(&self) -> bool {
        self.wire.is_some()
    }

    #[must_use]
    pub fn wire_records(&self) -> Vec<WireFaultRecord> {
        self.wire
            .as_ref()
            .map(WireFaultHarness::records)
            .unwrap_or_else(|| self.completed_wire_records.clone())
    }

    pub async fn cancelled(&self) {
        self.terminated.cancelled().await;
    }

    pub fn shutdown(&self) {
        self.stop.cancel();
        if let Some(wire) = &self.wire {
            wire.cancel();
        }
    }

    pub async fn wait(&mut self) -> Result<Terminated, SimulatorError> {
        let task = self
            .server_task
            .take()
            .ok_or_else(|| SimulatorError::Task("server task already awaited".to_owned()))?;
        let terminated = task
            .await
            .map_err(|error| SimulatorError::Task(error.to_string()))?
            .map_err(|error| SimulatorError::Runtime(error.to_string()))?;
        if let Some(wire) = self.wire.take() {
            self.completed_wire_records = wire.records();
            wire.shutdown().await?;
        }
        drop(self.client_guard.take());
        Ok(terminated)
    }

    /// Writes deterministic conformance metadata, complete request trace and wire faults.
    pub async fn write_structured_log(&self, path: &Path) -> Result<(), SimulatorError> {
        #[derive(Serialize)]
        #[serde(tag = "record", rename_all = "snake_case")]
        enum Record<'a> {
            Metadata {
                handshake: &'a ConformanceSimulatorHandshake,
            },
            Request {
                #[serde(flatten)]
                value: &'a SimulatorLogRecord,
            },
            WireFault {
                #[serde(flatten)]
                value: &'a WireFaultRecord,
            },
        }

        let requests = self.control.structured_log();
        let wire_records = self.wire_records();
        let mut output = String::new();
        push_json_line(
            &mut output,
            &Record::Metadata {
                handshake: &self.handshake,
            },
        )?;
        for value in &requests {
            push_json_line(&mut output, &Record::Request { value })?;
        }
        for value in &wire_records {
            push_json_line(&mut output, &Record::WireFault { value })?;
        }
        tokio::fs::write(path, output)
            .await
            .map_err(|source| SimulatorError::WriteFile {
                path: path.to_path_buf(),
                source,
            })
    }
}

impl Drop for ConformanceSimulatorRuntime {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(wire) = &self.wire {
            wire.cancel();
        }
    }
}

fn push_json_line<T: Serialize>(output: &mut String, value: &T) -> Result<(), SimulatorError> {
    output.push_str(
        &serde_json::to_string(value)
            .map_err(|error| SimulatorError::Runtime(error.to_string()))?,
    );
    output.push('\n');
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}
