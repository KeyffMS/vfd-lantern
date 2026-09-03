use std::{fs, path::Path};

fn main() {
    let path = Path::new("crates/lantern-storage/src/csv_writer.rs");
    let text = fs::read_to_string(path).expect("read csv_writer");
    let old = r#"fn persist_failed_artifacts(logger: &mut RunningLogger) -> Result<(), CsvWriterError> {
    logger.checkpoint.rows_written = logger.samples_written.saturating_add(logger.gaps_written);
    logger.checkpoint.dropped_count = logger.dropped_count;
    logger.checkpoint.last_update_utc = now_utc_text()?;
    logger.checkpoint.status = CsvSessionStatusV1::Failed;
    update_csv_session_sidecar(&logger.sidecar_path, &logger.sidecar)?;
    write_csv_runtime_checkpoint(&logger.checkpoint_path, &logger.checkpoint)?;
    Ok(())
}
"#;
    let new = r#"fn persist_failed_artifacts(logger: &mut RunningLogger) -> Result<(), CsvWriterError> {
    logger.checkpoint.rows_written = logger.samples_written.saturating_add(logger.gaps_written);
    logger.checkpoint.dropped_count = logger.dropped_count;
    logger.checkpoint.last_update_utc = now_utc_text()?;
    logger.checkpoint.status = CsvSessionStatusV1::Failed;
    let sidecar_error = update_csv_session_sidecar(&logger.sidecar_path, &logger.sidecar).err();
    let checkpoint_error = write_csv_runtime_checkpoint(&logger.checkpoint_path, &logger.checkpoint).err();
    if let Some(error) = sidecar_error {
        return Err(error.into());
    }
    if let Some(error) = checkpoint_error {
        return Err(error.into());
    }
    Ok(())
}
"#;
    let Some(index) = text.find(old) else { panic!("persist_failed_artifacts anchor not found"); };
    let mut output = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    output.push_str(&text[..index]);
    output.push_str(new);
    output.push_str(&text[index + old.len()..]);
    fs::write(path, output).expect("write csv_writer");
}
