use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let mut text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    if text.contains(new) {
        return;
    }
    let index = text.find(old).unwrap_or_else(|| panic!("anchor missing in {}: {old}", path.display()));
    text.replace_range(index..index + old.len(), new);
    fs::write(path, text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        "mod cli;\n",
        "mod backup_commands;\nmod cli;\n",
    );
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        "use anyhow::{Result, bail};",
        "use anyhow::Result;",
    );
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        r#"        Some(Command::Backup(arguments)) => match arguments.command {
            BackupCommand::Inspect { file } => {
                bail!(
                    "backup inspection for {} is implemented by roadmap issue #17",
                    file.display()
                )
            }
            BackupCommand::Diff { left, right } => bail!(
                "backup diff for {} and {} is implemented by roadmap issue #17",
                left.display(),
                right.display()
            ),
        },"#,
        "        Some(Command::Backup(arguments)) => backup_commands::run(arguments.command),",
    );
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        "    cli::{BackupCommand, Cli, Command, DiagnosticsCommand},",
        "    cli::{Cli, Command, DiagnosticsCommand},",
    );

    replace_once(
        "crates/lantern-storage/src/backup.rs",
        r#"    fn into_engineering(self) -> Result<EngineeringValue, BackupStorageError> {
        match self {
            Self::Fixed { decimal } => Decimal::from_str(&decimal)
                .map(EngineeringValue::Fixed)
                .map_err(|error| invalid("values.engineering.decimal", error.to_string())),
            Self::Float32 { bits, .. } => Ok(EngineeringValue::Float32Bits(bits)),
            Self::Float64 { bits, .. } => Ok(EngineeringValue::Float64Bits(bits)),
            Self::Enum { raw } => Ok(EngineeringValue::EnumRaw(raw)),
            Self::Bitfield { raw } => Ok(EngineeringValue::BitfieldRaw(raw)),
        }
    }"#,
        r#"    fn into_engineering(self) -> Result<EngineeringValue, BackupStorageError> {
        match self {
            Self::Fixed { decimal } => {
                let value = Decimal::from_str(&decimal)
                    .map_err(|error| invalid("values.engineering.decimal", error.to_string()))?;
                if value.normalize().to_string() != decimal {
                    return Err(invalid(
                        "values.engineering.decimal",
                        "fixed decimal is not canonical",
                    ));
                }
                Ok(EngineeringValue::Fixed(value))
            }
            Self::Float32 { bits, text } => {
                if f32::from_bits(bits).to_string() != text {
                    return Err(invalid(
                        "values.engineering.text",
                        "float32 text does not match raw bits",
                    ));
                }
                Ok(EngineeringValue::Float32Bits(bits))
            }
            Self::Float64 { bits, text } => {
                if f64::from_bits(bits).to_string() != text {
                    return Err(invalid(
                        "values.engineering.text",
                        "float64 text does not match raw bits",
                    ));
                }
                Ok(EngineeringValue::Float64Bits(bits))
            }
            Self::Enum { raw } => Ok(EngineeringValue::EnumRaw(raw)),
            Self::Bitfield { raw } => Ok(EngineeringValue::BitfieldRaw(raw)),
        }
    }"#,
    );

    replace_once(
        "crates/lantern-storage/src/backup.rs",
        "        let mut values = BTreeMap::new();\n",
        r#"        let completeness = parse_completeness(&self.completeness)?;
        if matches!(completeness, BackupCompleteness::Complete) && !self.errors.is_empty() {
            return Err(invalid(
                "errors",
                "complete backup cannot contain read errors",
            ));
        }
        let started_at = parse_i128("started_at_unix_nanos", &self.started_at_unix_nanos)?;
        let finished_at = parse_i128("finished_at_unix_nanos", &self.finished_at_unix_nanos)?;
        if finished_at < started_at {
            return Err(invalid(
                "finished_at_unix_nanos",
                "backup finished before it started",
            ));
        }
        let mut values = BTreeMap::new();
"#,
    );
    replace_once(
        "crates/lantern-storage/src/backup.rs",
        r#"            started_at: UtcTimestamp::from_unix_nanos(parse_i128(
                "started_at_unix_nanos",
                &self.started_at_unix_nanos,
            )?),
            finished_at: UtcTimestamp::from_unix_nanos(parse_i128(
                "finished_at_unix_nanos",
                &self.finished_at_unix_nanos,
            )?),"#,
        r#"            started_at: UtcTimestamp::from_unix_nanos(started_at),
            finished_at: UtcTimestamp::from_unix_nanos(finished_at),"#,
    );
    replace_once(
        "crates/lantern-storage/src/backup.rs",
        "            completeness: parse_completeness(&self.completeness)?,",
        "            completeness,",
    );

    const EXTRA_TESTS: &str = r#"

    #[test]
    fn backup_rejects_symlink_irregular_and_oversized_files() {
        use std::{fs::File, os::unix::fs::symlink};

        let directory = tempdir().expect("tempdir");
        let regular = directory.path().join("regular.vfdlantern-backup.json");
        write_backup(&regular, &sample_backup()).expect("write");
        let link = directory.path().join("link.vfdlantern-backup.json");
        symlink(&regular, &link).expect("symlink");
        assert!(read_backup(&link).is_err());
        assert!(read_backup(directory.path()).is_err());

        let oversized = directory.path().join("oversized.vfdlantern-backup.json");
        let file = File::create(&oversized).expect("create oversized");
        file.set_len((super::MAX_BACKUP_FILE_BYTES as u64) + 1)
            .expect("set len");
        assert!(read_backup(&oversized).is_err());
    }

    #[test]
    fn backup_rejects_noncanonical_decimal_and_mismatched_float_text() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("backup.vfdlantern-backup.json");
        write_backup(&path, &sample_backup()).expect("write");
        let bytes = fs::read(&path).expect("read");
        let mut envelope: serde_json::Value = serde_json::from_slice(&bytes).expect("json");

        envelope["payload"]["values"][0]["engineering"]["decimal"] =
            serde_json::Value::String("1.230".to_owned());
        let payload = serde_jcs::to_vec(&envelope["payload"]).expect("jcs");
        envelope["payload_sha256"] = serde_json::Value::String(super::sha256_hex(&payload));
        fs::write(&path, serde_jcs::to_vec(&envelope).expect("jcs")).expect("tamper");
        assert!(read_backup(&path).is_err());

        let mut envelope: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let float_index = envelope["payload"]["values"]
            .as_array()
            .expect("values")
            .iter()
            .position(|value| value["parameter_id"] == "p.float")
            .expect("float entry");
        envelope["payload"]["values"][float_index]["engineering"]["text"] =
            serde_json::Value::String("0".to_owned());
        let payload = serde_jcs::to_vec(&envelope["payload"]).expect("jcs");
        envelope["payload_sha256"] = serde_json::Value::String(super::sha256_hex(&payload));
        fs::write(&path, serde_jcs::to_vec(&envelope).expect("jcs")).expect("tamper");
        assert!(read_backup(&path).is_err());
    }
"#;
    let path = Path::new("crates/lantern-storage/src/backup.rs");
    let mut text = fs::read_to_string(path).expect("read backup.rs");
    if !text.contains("backup_rejects_symlink_irregular_and_oversized_files") {
        let index = text.rfind("\n}").expect("test module end");
        text.insert_str(index, EXTRA_TESTS);
        fs::write(path, text).expect("write backup.rs");
    }
}
