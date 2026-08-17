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
    LoadedScenario, SimulatorControl, SimulatorError, SimulatorLogRecord, SimulatorPty,
    WireFaultHarness, WireFaultRecord,
};

/// Machine-readable first line printed by `lantern-sim`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SimulatorHandshake {
    pub pty: PathBuf,
    pub profile_hash: String,
    pub scenario_hash: String,
    pub seed: String,
    pub fingerprint: String,
}

/// Running deterministic Modbus RTU simulator.
pub struct SimulatorRuntime {
    handshake: SimulatorHandshake,
    control: SimulatorControl,
    stop: CancellationToken,
    terminated: CancellationToken,
    server_task: Option<JoinHandle<Result<Terminated, std::io::Error>>>,
    client_guard: Option<tokio_serial::SerialStream>,
    wire: Option<WireFaultHarness>,
    completed_wire_records: Vec<WireFaultRecord>,
}

impl SimulatorRuntime {
    /// Starts a simulator with the production Tokio monotonic clock.
    pub fn spawn(
        profile: Arc<ValidatedDeviceProfile>,
        scenario: Arc<LoadedScenario>,
    ) -> Result<Self, SimulatorError> {
        Self::spawn_with_clock(profile, scenario, Arc::new(TokioMonotonicClock))
    }

    /// Starts a simulator using the application-owned clock boundary.
    pub fn spawn_with_clock(
        profile: Arc<ValidatedDeviceProfile>,
        scenario: Arc<LoadedScenario>,
        clock: Arc<dyn MonotonicClock>,
    ) -> Result<Self, SimulatorError> {
        let stop = CancellationToken::new();
        let terminated = CancellationToken::new();
        let (server_stream, client_guard, client_path, wire) = if scenario.wire_faults().is_empty()
        {
            let pty = SimulatorPty::direct()?;
            let (server, guard, path) = pty.into_parts();
            (server, guard, path, None)
        } else {
            let topology = WireFaultHarness::spawn(scenario.wire_faults(), Arc::clone(&clock))?;
            let (server, guard, path, harness) = topology.into_parts();
            (server, guard, path, Some(harness))
        };
        let (service, control) = crate::SimulatorService::new(
            Arc::clone(&profile),
            Arc::clone(&scenario),
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
        let handshake = SimulatorHandshake {
            pty: client_path,
            profile_hash: profile.profile_hash().to_hex(),
            scenario_hash: scenario.hash().to_hex(),
            seed: hex(&scenario.seed()),
            fingerprint: scenario.fingerprint().to_string(),
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
    pub const fn handshake(&self) -> &SimulatorHandshake {
        &self.handshake
    }

    #[must_use]
    pub fn client_path(&self) -> &Path {
        &self.handshake.pty
    }

    /// Reports whether the isolated byte-level fault proxy is active.
    #[must_use]
    pub const fn uses_wire_fault_harness(&self) -> bool {
        self.wire.is_some()
    }

    #[must_use]
    pub const fn control(&self) -> &SimulatorControl {
        &self.control
    }

    /// Returns byte-level mutations applied by the isolated wire proxy.
    #[must_use]
    pub fn wire_records(&self) -> Vec<WireFaultRecord> {
        self.wire
            .as_ref()
            .map(WireFaultHarness::records)
            .unwrap_or_else(|| self.completed_wire_records.clone())
    }

    /// Resolves when the server stopped because of shutdown, hangup, or a
    /// scheduled disconnect event.
    pub async fn cancelled(&self) {
        self.terminated.cancelled().await;
    }

    pub fn shutdown(&self) {
        self.stop.cancel();
        if let Some(wire) = &self.wire {
            wire.cancel();
        }
    }

    /// Waits for server and proxy shutdown.
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

    /// Writes deterministic metadata, service records, and wire mutations as
    /// newline-delimited JSON.
    pub async fn write_structured_log(&self, path: &Path) -> Result<(), SimulatorError> {
        #[derive(Serialize)]
        #[serde(tag = "record", rename_all = "snake_case")]
        enum Record<'a> {
            Metadata {
                handshake: &'a SimulatorHandshake,
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

impl Drop for SimulatorRuntime {
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
