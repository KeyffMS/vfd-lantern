use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(160)]);
    };
    let mut output = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    output.push_str(&text[..index]);
    output.push_str(new);
    output.push_str(&text[index + old.len()..]);
    fs::write(path, output).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn main() {
    let writer = "crates/lantern-storage/src/csv_writer.rs";

    replace_once(
        writer,
        "    flushes: u64,\n    syncs: u64,\n}\n",
        "    flushes: u64,\n    syncs: u64,\n    failed: bool,\n}\n",
    );
    replace_once(
        writer,
        "        flushes: 1,\n        syncs: 1,\n    })\n",
        "        flushes: 1,\n        syncs: 1,\n        failed: false,\n    })\n",
    );

    replace_once(
        writer,
        r#"                    CsvWriterCommand::Stop(request, reply) => {
                        let result = if let Some(mut logger) = active.take() {
                            let mut failure = None;
                            while failure.is_none() {
                                match data.try_recv() {
                                    Ok(item) => {
                                        if let Err(error) = write_item(&mut logger, item) {
                                            failure = Some(error);
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                            if failure.is_none()
                                && let Some(gap) = request.pending_gap.as_ref()
                                && let Err(error) = write_gap(&mut logger, gap)
                            {
                                failure = Some(error);
                            }
                            if let Some(error) = failure {
                                let message = fail_logger(&mut logger, error);
                                publish_status(
                                    &status_tx,
                                    &data,
                                    CsvWriterState::Failed,
                                    &logger,
                                    Some(message.clone()),
                                );
                                Err(message)
                            } else {
                                match finalize_logger(&mut logger, *request) {
                                    Ok(()) => {
                                        publish_status(
                                            &status_tx,
                                            &data,
                                            CsvWriterState::Completed,
                                            &logger,
                                            None,
                                        );
                                        Ok(())
                                    }
                                    Err(error) => {
                                        let message = fail_logger(&mut logger, error);
                                        publish_status(
                                            &status_tx,
                                            &data,
                                            CsvWriterState::Failed,
                                            &logger,
                                            Some(message.clone()),
                                        );
                                        Err(message)
                                    }
                                }
                            }
                        } else {
                            Ok(())
                        };
                        let _ = reply.send(result);
                    }
"#,
        r#"                    CsvWriterCommand::Stop(request, reply) => {
                        let request = *request;
                        let result = if let Some(mut logger) = active.take() {
                            if logger.failed {
                                let original = logger.sidecar.last_error.clone().unwrap_or_else(|| {
                                    "CSV logging failed before finalization".to_owned()
                                });
                                match finalize_failed_logger(&mut logger, &request, true) {
                                    Ok(()) => {
                                        publish_status(
                                            &status_tx,
                                            &data,
                                            CsvWriterState::Failed,
                                            &logger,
                                            Some(original.clone()),
                                        );
                                        Err(original)
                                    }
                                    Err(error) => {
                                        let message = fail_logger(&mut logger, error);
                                        publish_status(
                                            &status_tx,
                                            &data,
                                            CsvWriterState::Failed,
                                            &logger,
                                            Some(message.clone()),
                                        );
                                        Err(message)
                                    }
                                }
                            } else {
                                let mut failure = None;
                                while failure.is_none() {
                                    match data.try_recv() {
                                        Ok(item) => {
                                            if let Err(error) = write_item(&mut logger, item) {
                                                failure = Some(error);
                                            }
                                        }
                                        Err(_) => break,
                                    }
                                }
                                if failure.is_none()
                                    && let Some(gap) = request.pending_gap.as_ref()
                                    && let Err(error) = write_gap(&mut logger, gap)
                                {
                                    failure = Some(error);
                                }
                                if let Some(error) = failure {
                                    logger.failed = true;
                                    let mut message = fail_logger(&mut logger, error);
                                    if let Err(error) = finalize_failed_logger(&mut logger, &request, true) {
                                        message = format!(
                                            "{message}; failed to persist final CSV failure metadata: {error}"
                                        );
                                    }
                                    publish_status(
                                        &status_tx,
                                        &data,
                                        CsvWriterState::Failed,
                                        &logger,
                                        Some(message.clone()),
                                    );
                                    Err(message)
                                } else {
                                    match finalize_logger(&mut logger, &request) {
                                        Ok(()) => {
                                            publish_status(
                                                &status_tx,
                                                &data,
                                                CsvWriterState::Completed,
                                                &logger,
                                                None,
                                            );
                                            Ok(())
                                        }
                                        Err(error) => {
                                            logger.failed = true;
                                            let mut message = fail_logger(&mut logger, error);
                                            if let Err(error) =
                                                finalize_failed_logger(&mut logger, &request, false)
                                            {
                                                message = format!(
                                                    "{message}; failed to persist final CSV failure metadata: {error}"
                                                );
                                            }
                                            publish_status(
                                                &status_tx,
                                                &data,
                                                CsvWriterState::Failed,
                                                &logger,
                                                Some(message.clone()),
                                            );
                                            Err(message)
                                        }
                                    }
                                }
                            }
                        } else {
                            Ok(())
                        };
                        let _ = reply.send(result);
                    }
"#,
    );

    replace_once(
        writer,
        r#"                    CsvWriterCommand::Shutdown => {
                        if let Some(mut logger) = active.take() {
                            let _ = logger.writer.flush();
                            let _ = logger.writer.get_ref().sync_data();
                            let message = "process shutdown interrupted active CSV logging".to_owned();
                            logger.sidecar.status = CsvSessionStatusV1::Failed;
                            logger.sidecar.last_error = Some(message.clone());
                            logger.checkpoint.status = CsvSessionStatusV1::Failed;
                            logger.checkpoint.last_error = Some(message.clone());
                            let _ = persist_running_artifacts(&mut logger);
                            publish_status(&status_tx, &data, CsvWriterState::Failed, &logger, Some(message));
                        }
                        return;
                    }
"#,
        r#"                    CsvWriterCommand::Shutdown => {
                        if let Some(mut logger) = active.take() {
                            let message = if logger.failed {
                                logger.sidecar.last_error.clone().unwrap_or_else(|| {
                                    "CSV logging failed before process shutdown".to_owned()
                                })
                            } else {
                                let _ = logger.writer.flush();
                                let _ = logger.writer.get_ref().sync_data();
                                let message =
                                    "process shutdown interrupted active CSV logging".to_owned();
                                logger.failed = true;
                                logger.sidecar.status = CsvSessionStatusV1::Failed;
                                logger.sidecar.last_error = Some(message.clone());
                                logger.checkpoint.status = CsvSessionStatusV1::Failed;
                                logger.checkpoint.last_error = Some(message.clone());
                                message
                            };
                            let _ = persist_failed_artifacts(&mut logger);
                            publish_status(
                                &status_tx,
                                &data,
                                CsvWriterState::Failed,
                                &logger,
                                Some(message),
                            );
                        }
                        return;
                    }
"#,
    );

    replace_once(
        writer,
        r#"            item = data.recv() => {
                let Some(item) = item else { return; };
                if let Some(logger) = active.as_mut()
                    && let Err(error) = write_item(logger, item)
                {
                    let message = fail_logger(logger, error);
                    publish_status(&status_tx, &data, CsvWriterState::Failed, logger, Some(message));
                    active = None;
                }
            }
            _ = interval.tick() => {
                if let Some(logger) = active.as_mut() {
                    if let Err(error) = maintain_logger(logger) {
                        let message = fail_logger(logger, error);
                        publish_status(&status_tx, &data, CsvWriterState::Failed, logger, Some(message));
                        active = None;
                    } else {
                        publish_status(&status_tx, &data, CsvWriterState::Running, logger, None);
                    }
                }
            }
"#,
        r#"            item = data.recv() => {
                let Some(item) = item else { return; };
                if let Some(logger) = active.as_mut()
                    && !logger.failed
                    && let Err(error) = write_item(logger, item)
                {
                    logger.failed = true;
                    let message = fail_logger(logger, error);
                    publish_status(&status_tx, &data, CsvWriterState::Failed, logger, Some(message));
                }
            }
            _ = interval.tick() => {
                if let Some(logger) = active.as_mut()
                    && !logger.failed
                {
                    if let Err(error) = maintain_logger(logger) {
                        logger.failed = true;
                        let message = fail_logger(logger, error);
                        publish_status(&status_tx, &data, CsvWriterState::Failed, logger, Some(message));
                    } else {
                        publish_status(&status_tx, &data, CsvWriterState::Running, logger, None);
                    }
                }
            }
"#,
    );

    replace_once(
        writer,
        r#"fn finalize_logger(
    logger: &mut RunningLogger,
    request: CsvWriterStop,
) -> Result<(), CsvWriterError> {
    logger.writer.flush()?;
    logger.flushes = logger.flushes.saturating_add(1);
    logger.writer.get_ref().sync_all()?;
    logger.syncs = logger.syncs.saturating_add(1);
    logger.sidecar.status = CsvSessionStatusV1::Completed;
    logger.sidecar.stopped_utc = Some(utc_text(request.stopped_utc)?);
    logger.sidecar.bus_stop = Some(CsvBusStatisticsV1::from(&request.bus_stop));
    logger.sidecar.faults = request.faults;
    logger.sidecar.counts.samples = logger.samples_written;
    logger.sidecar.counts.gaps = logger.gaps_written;
    logger.sidecar.counts.dropped = logger.dropped_count;
    update_csv_session_sidecar(&logger.sidecar_path, &logger.sidecar)?;
    remove_csv_runtime_checkpoint(&logger.checkpoint_path)?;
    Ok(())
}

fn fail_logger(logger: &mut RunningLogger, error: CsvWriterError) -> String {
    let message = error.to_string();
    logger.sidecar.status = CsvSessionStatusV1::Failed;
    logger.sidecar.last_error = Some(message.clone());
    logger.checkpoint.status = CsvSessionStatusV1::Failed;
    logger.checkpoint.last_error = Some(message.clone());
    logger.checkpoint.rows_written = logger.samples_written.saturating_add(logger.gaps_written);
    logger.checkpoint.dropped_count = logger.dropped_count;
    logger.checkpoint.last_update_utc =
        now_utc_text().unwrap_or_else(|_| logger.checkpoint.started_utc.clone());
    let _ = logger.writer.flush();
    let _ = update_csv_session_sidecar(&logger.sidecar_path, &logger.sidecar);
    let _ = write_csv_runtime_checkpoint(&logger.checkpoint_path, &logger.checkpoint);
    message
}
"#,
        r#"fn finalize_logger(
    logger: &mut RunningLogger,
    request: &CsvWriterStop,
) -> Result<(), CsvWriterError> {
    logger.writer.flush()?;
    logger.flushes = logger.flushes.saturating_add(1);
    logger.writer.get_ref().sync_all()?;
    logger.syncs = logger.syncs.saturating_add(1);
    logger.sidecar.status = CsvSessionStatusV1::Completed;
    logger.sidecar.stopped_utc = Some(utc_text(request.stopped_utc)?);
    logger.sidecar.bus_stop = Some(CsvBusStatisticsV1::from(&request.bus_stop));
    logger.sidecar.faults = request.faults.clone();
    logger.sidecar.counts.samples = logger.samples_written;
    logger.sidecar.counts.gaps = logger.gaps_written;
    logger.sidecar.counts.dropped = logger.dropped_count;
    update_csv_session_sidecar(&logger.sidecar_path, &logger.sidecar)?;
    remove_csv_runtime_checkpoint(&logger.checkpoint_path)?;
    Ok(())
}

fn record_failed_gap_summary(
    logger: &mut RunningLogger,
    gap: &TelemetryGapCore,
) -> Result<(), CsvWriterError> {
    logger.dropped_count = logger.dropped_count.saturating_add(gap.dropped_count);
    logger.sidecar.counts.gaps = logger.sidecar.counts.gaps.saturating_add(1);
    logger.sidecar.counts.dropped = logger.dropped_count;
    logger.sidecar.gaps.records = logger.sidecar.gaps.records.saturating_add(1);
    logger.sidecar.gaps.dropped_count = logger.dropped_count;
    let start = utc_text(gap.start_utc)?;
    let end = utc_text(gap.end_utc)?;
    if logger.sidecar.gaps.first_gap_start_utc.is_none() {
        logger.sidecar.gaps.first_gap_start_utc = Some(start);
    }
    logger.sidecar.gaps.last_gap_end_utc = Some(end);
    Ok(())
}

fn finalize_failed_logger(
    logger: &mut RunningLogger,
    request: &CsvWriterStop,
    include_pending_gap: bool,
) -> Result<(), CsvWriterError> {
    if include_pending_gap
        && let Some(gap) = request.pending_gap.as_ref()
    {
        record_failed_gap_summary(logger, gap)?;
    }
    logger.failed = true;
    logger.sidecar.status = CsvSessionStatusV1::Failed;
    logger.sidecar.stopped_utc = Some(utc_text(request.stopped_utc)?);
    logger.sidecar.bus_stop = Some(CsvBusStatisticsV1::from(&request.bus_stop));
    logger.sidecar.faults = request.faults.clone();
    logger.sidecar.counts.samples = logger.samples_written;
    logger.sidecar.counts.dropped = logger.dropped_count;
    logger.checkpoint.status = CsvSessionStatusV1::Failed;
    if logger.checkpoint.last_error.is_none() {
        logger.checkpoint.last_error = logger.sidecar.last_error.clone();
    }
    persist_failed_artifacts(logger)
}

fn persist_failed_artifacts(logger: &mut RunningLogger) -> Result<(), CsvWriterError> {
    logger.checkpoint.rows_written = logger.samples_written.saturating_add(logger.gaps_written);
    logger.checkpoint.dropped_count = logger.dropped_count;
    logger.checkpoint.last_update_utc = now_utc_text()?;
    logger.checkpoint.status = CsvSessionStatusV1::Failed;
    update_csv_session_sidecar(&logger.sidecar_path, &logger.sidecar)?;
    write_csv_runtime_checkpoint(&logger.checkpoint_path, &logger.checkpoint)?;
    Ok(())
}

fn fail_logger(logger: &mut RunningLogger, error: CsvWriterError) -> String {
    let message = error.to_string();
    logger.failed = true;
    logger.sidecar.status = CsvSessionStatusV1::Failed;
    logger.sidecar.last_error = Some(message.clone());
    logger.checkpoint.status = CsvSessionStatusV1::Failed;
    logger.checkpoint.last_error = Some(message.clone());
    let _ = logger.writer.flush();
    let _ = persist_failed_artifacts(logger);
    message
}
"#,
    );

    let vet = "supply-chain/config.toml";
    replace_once(
        vet,
        r#"[[exemptions.crossterm_winapi]]
version = "0.9.1"
criteria = "safe-to-deploy"

[[exemptions.crypto-common]]
"#,
        r#"[[exemptions.crossterm_winapi]]
version = "0.9.1"
criteria = "safe-to-deploy"

[[exemptions.csv]]
version = "1.4.0"
criteria = "safe-to-deploy"

[[exemptions.csv-core]]
version = "0.1.13"
criteria = "safe-to-deploy"

[[exemptions.crypto-common]]
"#,
    );
    replace_once(
        vet,
        r#"[[exemptions.time-core]]
version = "0.1.9"
criteria = "safe-to-deploy"

[[exemptions.tokio]]
"#,
        r#"[[exemptions.time-core]]
version = "0.1.9"
criteria = "safe-to-deploy"

[[exemptions.time-macros]]
version = "0.2.32"
criteria = "safe-to-deploy"

[[exemptions.tokio]]
"#,
    );
}
