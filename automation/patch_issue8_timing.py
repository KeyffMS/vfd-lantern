#!/usr/bin/env python3
from pathlib import Path

path = Path("crates/lantern-transport/src/bus_actor.rs")
text = path.read_text(encoding="utf-8")
text = text.replace(
    "let result = execute_read(&mut backend, &request, &statistics).await;",
    "let result = execute_read(&mut backend, &request, config.t35(), &statistics).await;",
)
text = text.replace(
    '''async fn execute_read<B: RtuBackend>(
    backend: &mut B,
    request: &ReadBusRequest,
    statistics: &Arc<Mutex<BusStatistics>>,
) -> Result<RawRegisters, BusError> {''',
    '''async fn execute_read<B: RtuBackend>(
    backend: &mut B,
    request: &ReadBusRequest,
    retry_delay: Duration,
    statistics: &Arc<Mutex<BusStatistics>>,
) -> Result<RawRegisters, BusError> {''',
)
text = text.replace(
    '''                retries += 1;
                lock_stats(statistics).read_retries += 1;''',
    '''                retries += 1;
                {
                    let mut stats = lock_stats(statistics);
                    stats.read_retries += 1;
                    stats.t35_delay += retry_delay;
                }
                sleep(retry_delay).await;''',
)
path.write_text(text, encoding="utf-8")
