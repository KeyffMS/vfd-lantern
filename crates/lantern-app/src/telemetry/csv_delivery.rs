use std::collections::BTreeSet;

use lantern_domain::{CsvTelemetryItem, ParameterId, TelemetryGapCore, TelemetrySampleCore};
use tokio::sync::mpsc;

#[derive(Debug, Default)]
pub(super) struct CsvDeliveryState {
    enabled: bool,
    parameters: BTreeSet<ParameterId>,
    pending_gap: Option<TelemetryGapCore>,
}

impl CsvDeliveryState {
    pub(super) fn start(&mut self, parameters: impl IntoIterator<Item = ParameterId>) {
        self.enabled = true;
        self.parameters = parameters.into_iter().collect();
        self.pending_gap = None;
    }

    pub(super) fn stop(&mut self) -> Option<TelemetryGapCore> {
        self.enabled = false;
        self.parameters.clear();
        self.pending_gap.take()
    }

    #[must_use]
    pub(super) const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Attempts a non-blocking publish. The return value is the number of
    /// newly dropped samples (zero or one). A pending global stream gap is
    /// always emitted before the first recovered sample.
    pub(super) fn publish(
        &mut self,
        sample: &TelemetrySampleCore,
        sender: &mpsc::Sender<CsvTelemetryItem>,
    ) -> u64 {
        if !self.enabled || !self.parameters.contains(&sample.parameter_id) {
            return 0;
        }

        if let Some(mut gap) = self.pending_gap.take()
            && sender.try_send(CsvTelemetryItem::Gap(gap.clone())).is_err()
        {
            gap.extend_with_dropped_sample(sample);
            self.pending_gap = Some(gap);
            return 1;
        }

        if sender
            .try_send(CsvTelemetryItem::Sample(sample.clone()))
            .is_err()
        {
            self.pending_gap = Some(TelemetryGapCore::from_dropped_sample(sample));
            1
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use lantern_domain::{
        CsvTelemetryItem, EngineeringValue, MonotonicInstant, ParameterId, RawRegisters, RequestId,
        SessionId, TelemetryQuality, TelemetrySampleCore, UtcTimestamp,
    };
    use tokio::sync::mpsc;

    use super::CsvDeliveryState;

    fn sample(request: u64) -> TelemetrySampleCore {
        TelemetrySampleCore {
            session_id: SessionId::new(9),
            parameter_id: ParameterId::parse("status.frequency").expect("parameter"),
            raw: RawRegisters::new(vec![u16::try_from(request).expect("raw")]).expect("raw"),
            engineering: EngineeringValue::Fixed(lantern_domain::Decimal::from(request)),
            quality: TelemetryQuality::Good,
            monotonic_time: MonotonicInstant::from_nanos(u128::from(request) * 10),
            utc_time: UtcTimestamp::from_unix_nanos(i128::from(request) * 100),
            request_id: RequestId::new(request),
        }
    }

    #[test]
    fn disabled_delivery_never_consumes_queue_capacity() {
        let (tx, mut rx) = mpsc::channel(1);
        let mut state = CsvDeliveryState::default();
        assert_eq!(state.publish(&sample(1), &tx), 0);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn one_drop_wave_has_exact_range_and_gap_precedes_recovery() {
        let (tx, mut rx) = mpsc::channel(3);
        let parameter = ParameterId::parse("status.frequency").expect("parameter");
        let mut state = CsvDeliveryState::default();
        state.start([parameter]);
        assert_eq!(state.publish(&sample(1), &tx), 0);
        assert_eq!(state.publish(&sample(2), &tx), 0);
        assert_eq!(state.publish(&sample(3), &tx), 0);
        assert_eq!(state.publish(&sample(4), &tx), 1);
        assert_eq!(state.publish(&sample(5), &tx), 1);

        for expected in 1..=3 {
            let CsvTelemetryItem::Sample(item) = rx.try_recv().expect("queued sample") else {
                panic!("sample")
            };
            assert_eq!(item.request_id.get(), expected);
        }
        assert_eq!(state.publish(&sample(6), &tx), 0);
        let CsvTelemetryItem::Gap(gap) = rx.try_recv().expect("gap") else {
            panic!("gap")
        };
        assert_eq!(gap.start_monotonic.as_nanos(), 40);
        assert_eq!(gap.end_monotonic.as_nanos(), 50);
        assert_eq!(gap.start_utc.as_unix_nanos(), 400);
        assert_eq!(gap.end_utc.as_unix_nanos(), 500);
        assert_eq!(gap.dropped_count, 2);
        let CsvTelemetryItem::Sample(recovered) = rx.try_recv().expect("recovered") else {
            panic!("sample")
        };
        assert_eq!(recovered.request_id.get(), 6);
    }

    #[test]
    fn stop_returns_active_gap_for_finalization() {
        let (tx, _rx) = mpsc::channel(1);
        let parameter = ParameterId::parse("status.frequency").expect("parameter");
        let mut state = CsvDeliveryState::default();
        state.start([parameter]);
        assert_eq!(state.publish(&sample(1), &tx), 0);
        assert_eq!(state.publish(&sample(2), &tx), 1);
        let gap = state.stop().expect("pending gap");
        assert_eq!(gap.dropped_count, 1);
        assert_eq!(gap.start_monotonic.as_nanos(), 20);
        assert!(!state.is_enabled());
    }
}
