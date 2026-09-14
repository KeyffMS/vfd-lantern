use std::sync::Arc;

use crate::{FaultAction, FaultEffect, FaultIdentityContext, SessionState};

use super::{ApplicationEffect, ApplicationState};

impl ApplicationState {
    pub(super) fn reduce_faults(&mut self, action: FaultAction) -> Vec<ApplicationEffect> {
        match action {
            FaultAction::ObserveTelemetry { event, bus } => {
                let Some(profile) = self.selected_profile() else {
                    return Vec::new();
                };
                let identity = match self.session.state() {
                    SessionState::Active(active) => FaultIdentityContext {
                        session_id: active.session_id,
                        fingerprint: active.identity.device.fingerprint.clone(),
                        profile_hash: active.identity.profile_hash.to_hex(),
                    },
                    _ => return Vec::new(),
                };
                let latest = self
                    .monitoring
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.latest.as_ref());
                match self
                    .faults
                    .observe(&profile, &event, latest, identity, *bus)
                {
                    Ok(Some(detection)) => {
                        vec![ApplicationEffect::Faults(FaultEffect::CaptureFreezeFrame {
                            event_id: detection.event_id,
                            session_id: detection.session_id,
                            profile: Arc::clone(&profile),
                            parameters: detection.freeze_frame_parameters,
                        })]
                    }
                    Ok(None) => Vec::new(),
                    Err(error) => {
                        self.faults.set_error(error.to_string());
                        Vec::new()
                    }
                }
            }
            FaultAction::FreezeFrameCompleted {
                event_id,
                captured,
                errors,
            } => {
                self.faults
                    .complete_freeze_frame(event_id, captured, errors);
                Vec::new()
            }
            FaultAction::Acknowledge(event_id) => {
                self.faults.acknowledge(event_id);
                Vec::new()
            }
            FaultAction::Export(event_id) => {
                let active = match self.session.state() {
                    SessionState::Active(active) => active,
                    _ => {
                        self.faults.set_error(
                            "fault export requires an active Verified session".to_owned(),
                        );
                        return Vec::new();
                    }
                };
                let Some(event) = self.faults.export_event(event_id) else {
                    self.faults
                        .set_error("fault event is no longer in the bounded timeline".to_owned());
                    return Vec::new();
                };
                if event.event.session_id != active.session_id
                    || event.event.fingerprint != active.identity.device.fingerprint
                    || event.event.profile_hash != active.identity.profile_hash.to_hex()
                {
                    self.faults.set_error(
                        "fault export identity does not match the active Verified session"
                            .to_owned(),
                    );
                    return Vec::new();
                }
                let suggested_name =
                    format!("fault-{}-{}", active.session_id.get(), event_id.get());
                vec![ApplicationEffect::Faults(FaultEffect::Export {
                    suggested_name,
                    event: Box::new(event),
                })]
            }
            FaultAction::ExportFinished(result) => {
                self.faults.export_finished(result);
                Vec::new()
            }
        }
    }
}
