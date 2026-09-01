use std::{sync::Arc, time::Instant};

use lantern_app::{
    ApplicationAction, BusError, BusRequestContext, FaultAction, FaultEventId, FreezeFrameValue,
    ParameterId, PollPlanner, RawRegisters, ReadBusPort, ReadBusRequest, RequestId, SessionId,
    SlaveId, SystemUtcClock, TelemetryQuality, UtcClock, ValidatedDeviceProfile,
};
use lantern_transport::BusActorHandle;
use tokio::sync::mpsc;

pub fn spawn_freeze_frame_capture(
    profile: Arc<ValidatedDeviceProfile>,
    bus: Option<BusActorHandle>,
    slave_id: Option<SlaveId>,
    event_id: FaultEventId,
    session_id: SessionId,
    parameters: Vec<ParameterId>,
    action_tx: mpsc::UnboundedSender<ApplicationAction>,
) {
    tokio::spawn(async move {
        let mut captured = Vec::new();
        let mut errors = Vec::new();
        let Some(bus) = bus else {
            errors.push("freeze-frame bus is unavailable".to_owned());
            complete(&action_tx, event_id, captured, errors);
            return;
        };
        let Some(slave_id) = slave_id else {
            errors.push("freeze-frame has no validated slave ID".to_owned());
            complete(&action_tx, event_id, captured, errors);
            return;
        };
        let plan = match PollPlanner::new().build_fault_freeze_frame(&profile, &parameters) {
            Ok(plan) => plan,
            Err(error) => {
                errors.push(error.to_string());
                complete(&action_tx, event_id, captured, errors);
                return;
            }
        };
        let timeout = profile.protocol().default_link().response_timeout;
        let utc = SystemUtcClock;

        for (block_index, block) in plan.blocks.iter().enumerate() {
            let request_id = RequestId::new(
                u64::try_from(event_id.get())
                    .unwrap_or(u64::MAX)
                    .saturating_add(u64::try_from(block_index).unwrap_or(u64::MAX)),
            );
            let deadline = Instant::now().checked_add(timeout).unwrap_or_else(Instant::now);
            let request = ReadBusRequest::one_shot(
                BusRequestContext::interactive(request_id, session_id, deadline, None),
                slave_id,
                block.function,
                block.block,
            );
            let raw_block = match request {
                Ok(request) => bus.read(request).await,
                Err(error) => Err(error),
            };
            match raw_block {
                Ok(raw_block) => {
                    for slice in &block.parameters {
                        let start = usize::from(slice.register_offset);
                        let count = usize::from(slice.register_count.get());
                        let words = start
                            .checked_add(count)
                            .and_then(|end| raw_block.as_slice().get(start..end));
                        let Some(words) = words else {
                            captured.push(failed_value(
                                slice.parameter_id.clone(),
                                TelemetryQuality::DecodeError,
                                "freeze-frame block slice is invalid".to_owned(),
                            ));
                            errors.push(format!(
                                "{}: invalid register slice",
                                slice.parameter_id
                            ));
                            continue;
                        };
                        let raw = match RawRegisters::new(words.to_vec()) {
                            Ok(raw) => raw,
                            Err(error) => {
                                captured.push(failed_value(
                                    slice.parameter_id.clone(),
                                    TelemetryQuality::DecodeError,
                                    error.to_string(),
                                ));
                                errors.push(format!("{}: {error}", slice.parameter_id));
                                continue;
                            }
                        };
                        let Some(parameter) = profile.parameter(&slice.parameter_id) else {
                            captured.push(failed_value(
                                slice.parameter_id.clone(),
                                TelemetryQuality::DecodeError,
                                "parameter disappeared from validated profile".to_owned(),
                            ));
                            errors.push(format!(
                                "{}: missing validated parameter",
                                slice.parameter_id
                            ));
                            continue;
                        };
                        match parameter.codec().decode(raw.as_slice()) {
                            Ok(engineering) => captured.push(FreezeFrameValue {
                                parameter_id: slice.parameter_id.clone(),
                                raw: Some(raw),
                                engineering: Some(engineering),
                                quality: TelemetryQuality::Good,
                                observed_at: Some(utc.now()),
                                age: Some(std::time::Duration::ZERO),
                                error: None,
                            }),
                            Err(error) => {
                                captured.push(FreezeFrameValue {
                                    parameter_id: slice.parameter_id.clone(),
                                    raw: Some(raw),
                                    engineering: None,
                                    quality: TelemetryQuality::DecodeError,
                                    observed_at: Some(utc.now()),
                                    age: Some(std::time::Duration::ZERO),
                                    error: Some(error.to_string()),
                                });
                                errors.push(format!("{}: {error}", slice.parameter_id));
                            }
                        }
                    }
                }
                Err(error) => {
                    let quality = quality_for_bus_error(&error);
                    errors.push(format!(
                        "freeze-frame {:?} read failed: {error}",
                        block.block.table()
                    ));
                    for slice in &block.parameters {
                        captured.push(failed_value(
                            slice.parameter_id.clone(),
                            quality,
                            error.to_string(),
                        ));
                    }
                }
            }
        }
        complete(&action_tx, event_id, captured, errors);
    });
}

fn complete(
    action_tx: &mpsc::UnboundedSender<ApplicationAction>,
    event_id: FaultEventId,
    captured: Vec<FreezeFrameValue>,
    errors: Vec<String>,
) {
    let _ = action_tx.send(ApplicationAction::Faults(FaultAction::FreezeFrameCompleted {
        event_id,
        captured,
        errors,
    }));
}

fn failed_value(
    parameter_id: ParameterId,
    quality: TelemetryQuality,
    error: String,
) -> FreezeFrameValue {
    FreezeFrameValue {
        parameter_id,
        raw: None,
        engineering: None,
        quality,
        observed_at: None,
        age: None,
        error: Some(error),
    }
}

fn quality_for_bus_error(error: &BusError) -> TelemetryQuality {
    match error {
        BusError::TimeoutBeforeSend | BusError::ResponseTimeout => TelemetryQuality::Timeout,
        BusError::ProtocolException { .. } => TelemetryQuality::ProtocolException,
        BusError::PortRemoved | BusError::Shutdown => TelemetryQuality::Disconnected,
        BusError::InvalidFrameOrTransport | BusError::InvalidResponse => TelemetryQuality::DecodeError,
        BusError::PermissionDenied
        | BusError::PortBusy
        | BusError::Io(_)
        | BusError::Cancelled
        | BusError::QueueFull
        | BusError::OutcomeUnknown
        | BusError::InvalidRequest(_) => TelemetryQuality::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use lantern_app::{BusError, TelemetryQuality};

    use super::quality_for_bus_error;

    #[test]
    fn queue_full_degrades_freeze_frame_without_turning_into_a_fault_reset_or_write() {
        assert_eq!(
            quality_for_bus_error(&BusError::QueueFull),
            TelemetryQuality::Unavailable
        );
    }
}
