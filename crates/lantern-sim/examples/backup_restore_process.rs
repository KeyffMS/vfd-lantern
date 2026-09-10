use super::*;

use lantern_sim::{
    ConformanceSimulatorRuntime, LoadedConformanceScenario, LoadedScenario, SimulatorLogRecord,
    load_conformance_scenario, load_scenario,
};

const RESTORE_SOURCE_ACCELERATION: &str = "10";
const RESTORE_SOURCE_DECELERATION: &str = "8";
const RESTORE_CURRENT_ACCELERATION: &str = "12";
const RESTORE_CURRENT_DECELERATION: &str = "11";

pub(super) fn run_backup_restore_process_matrix(
    simulator_binary: &Path,
    product_binary: &Path,
) -> Result<()> {
    let env = CaseEnvironment::new()?;
    let selected = env.root.path().join("restore-two-step-vfd.toml");
    let base = fs::read_to_string(reference_profile())?;
    fs::write(&selected, two_step_restore_profile(&base)?)?;
    let profile = lantern_sim::load_profile(&selected)?;
    approve_write_fixture(product_binary, &selected, &profile.profile_hash().to_hex(), &env)?;

    let source_backup = capture_source_backup(simulator_binary, product_binary, &selected, &profile, &env)?;
    ensure!(source_backup.is_file(), "source backup was not persisted");

    run_success_case(product_binary, &selected, &profile, &source_backup, &env)?;
    run_device_failure_case(product_binary, &selected, &profile, &source_backup, &env)?;
    Ok(())
}

fn capture_source_backup(
    simulator_binary: &Path,
    product_binary: &Path,
    selected: &Path,
    profile: &lantern_profile::ValidatedDeviceProfile,
    env: &CaseEnvironment,
) -> Result<PathBuf> {
    let scenario = env.root.path().join("restore-source.toml");
    fs::write(
        &scenario,
        scenario_with_values(
            selected,
            profile,
            RESTORE_SOURCE_ACCELERATION,
            RESTORE_SOURCE_DECELERATION,
        ),
    )?;
    let simulator = Simulator::spawn(
        simulator_binary,
        selected,
        &scenario,
        env.root.path().join("restore-source.jsonl"),
    )?;
    let mut args = product_args(selected, &simulator.pty);
    args.push("--enable-writes".to_owned());
    let mut product = TerminalChild::spawn(product_binary, &args, env)?;
    drive_to_summary(&mut product)?;
    product.send("\r")?;
    product.wait_for("Verified read-only session established")?;
    product.send("5")?;
    product.wait_for("Backup / Diff / Restore")?;
    product.send("b")?;
    product.wait_for("captured; complete=true")?;
    product.quit()?;
    let records = simulator.stop()?;
    assert_read_only_at_least("backup-source-capture", &records, 3)?;

    let backups = backup_files(env)?;
    ensure!(
        backups.len() == 1,
        "source capture must persist exactly one backup, found {backups:?}"
    );
    Ok(backups[0].clone())
}

fn run_success_case(
    product_binary: &Path,
    selected: &Path,
    profile: &lantern_profile::ValidatedDeviceProfile,
    source_backup: &Path,
    env: &CaseEnvironment,
) -> Result<()> {
    let core_path = env.root.path().join("restore-success-core.toml");
    fs::write(
        &core_path,
        scenario_with_values(
            selected,
            profile,
            RESTORE_CURRENT_ACCELERATION,
            RESTORE_CURRENT_DECELERATION,
        ),
    )?;
    let conformance_path = env.root.path().join("restore-success-conformance.toml");
    let mut simulator = ConformanceProcessSimulator::spawn(
        selected,
        &core_path,
        &conformance_path,
        "[[write_behaviors]]\nstart_write = 1\ncount = 2\nkind = \"accept\"\n\n[restore]\ntotal_steps = 2\n",
    )?;

    let mut args = product_args(selected, simulator.client_path());
    args.push("--enable-writes".to_owned());
    let mut product = TerminalChild::spawn(product_binary, &args, env)?;
    connect_and_arm(&mut product, profile)?;
    product.send("5")?;
    product.wait_for("Backup / Diff / Restore")?;
    load_source(&mut product, source_backup)?;
    product.send("p")?;
    product.wait_for("restore plan prepared; steps=2")?;
    product.wait_for("Exact confirmation required: restore:")?;
    let challenge = restore_challenge(&product.screen_text())?;

    // First prove phase-2 confirmation is exact and that rejection cannot touch the bus.
    product.send("r")?;
    product.wait_for("Restore confirmation:")?;
    product.send("wrong\r")?;
    product.wait_for("operator confirmation does not exactly match restore plan")?;
    ensure!(
        simulator.snapshot().write_count == 0,
        "wrong restore confirmation touched the physical write path"
    );

    product.send("r")?;
    product.wait_for("Restore confirmation:")?;
    product.send(&format!("{challenge}\r"))?;
    product.wait_for("restore completed; verified_steps=2")?;
    ensure!(
        simulator.snapshot().write_count == 2,
        "successful two-step restore must execute exactly two physical writes"
    );
    product.quit()?;
    let records = simulator.stop()?;
    assert_exact_restore_writes(&records, &[10, 11])?;
    assert_restore_audit(env, true)?;
    Ok(())
}

fn run_device_failure_case(
    product_binary: &Path,
    selected: &Path,
    profile: &lantern_profile::ValidatedDeviceProfile,
    source_backup: &Path,
    env: &CaseEnvironment,
) -> Result<()> {
    let core_path = env.root.path().join("restore-device-failure-core.toml");
    fs::write(
        &core_path,
        scenario_with_values(
            selected,
            profile,
            RESTORE_CURRENT_ACCELERATION,
            RESTORE_CURRENT_DECELERATION,
        ),
    )?;
    let conformance_path = env.root.path().join("restore-device-failure-conformance.toml");
    let mut simulator = ConformanceProcessSimulator::spawn(
        selected,
        &core_path,
        &conformance_path,
        "[[write_behaviors]]\nstart_write = 1\nkind = \"accept\"\n\n[[write_behaviors]]\nstart_write = 2\nkind = \"exception\"\ncode = 2\n\n[restore]\ntotal_steps = 2\n",
    )?;

    let mut args = product_args(selected, simulator.client_path());
    args.push("--enable-writes".to_owned());
    let mut product = TerminalChild::spawn(product_binary, &args, env)?;
    connect_and_arm(&mut product, profile)?;
    product.send("5")?;
    product.wait_for("Backup / Diff / Restore")?;
    load_source(&mut product, source_backup)?;
    product.send("p")?;
    product.wait_for("restore plan prepared; steps=2")?;
    let challenge = restore_challenge(&product.screen_text())?;
    product.send("r")?;
    product.wait_for("Restore confirmation:")?;
    product.send(&format!("{challenge}\r"))?;
    product.wait_for("restore stopped at step 1: DeviceRejected")?;
    ensure!(
        simulator.snapshot().write_count == 2,
        "device rejection must stop after the failing second write without retry"
    );
    thread::sleep(Duration::from_millis(150));
    ensure!(
        simulator.snapshot().write_count == 2,
        "terminal restore failure must not schedule a catch-up or retry write"
    );
    product.quit()?;
    let records = simulator.stop()?;
    assert_exact_restore_writes(&records, &[10, 11])?;
    let writes = records
        .iter()
        .filter(|record| record.function == 6)
        .collect::<Vec<_>>();
    ensure!(writes[0].outcome == "ok", "first restore step must be accepted");
    ensure!(
        writes[1].outcome == "exception:02",
        "second restore step must expose the injected device exception"
    );
    assert_restore_audit(env, false)?;
    Ok(())
}

fn connect_and_arm(
    product: &mut TerminalChild,
    profile: &lantern_profile::ValidatedDeviceProfile,
) -> Result<()> {
    drive_to_summary(product)?;
    product.send("\r")?;
    product.wait_for("Verified read-only session established")?;
    product.send("4")?;
    product.wait_for("WRITES DISARMED")?;
    product.send("A")?;
    product.wait_for("Arming confirmation:")?;
    let hash = profile.profile_hash().to_hex();
    product.send(&format!("ARM {}\r", &hash[..12]))?;
    product.wait_for("WRITES ARMED")?;
    Ok(())
}

fn load_source(product: &mut TerminalChild, source_backup: &Path) -> Result<()> {
    product.send("l")?;
    product.wait_for("Source backup path:")?;
    product.send(&format!("{}\r", source_backup.display()))?;
    product.wait_for("loaded; complete=true")?;
    Ok(())
}

fn restore_challenge(screen: &str) -> Result<String> {
    let marker = "Exact confirmation required: ";
    screen
        .split(marker)
        .nth(1)
        .and_then(|suffix| suffix.lines().next())
        .map(str::trim)
        .filter(|value| value.starts_with("restore:") && value.matches(':').count() == 2)
        .map(str::to_owned)
        .context("operator-visible exact restore challenge")
}

fn approve_write_fixture(
    product_binary: &Path,
    selected: &Path,
    hash: &str,
    env: &CaseEnvironment,
) -> Result<()> {
    let approval = Command::new(product_binary)
        .args(["profile", "approve-write"])
        .arg(selected)
        .args(["--expected-hash", hash, "--manual-source", "PTY restore fixture"])
        .args(["--summary", "Disposable two-step process restore acceptance"])
        .env("HOME", &env.home)
        .env("XDG_CONFIG_HOME", &env.config)
        .env("XDG_DATA_HOME", &env.data)
        .env("XDG_STATE_HOME", &env.state)
        .env("XDG_CACHE_HOME", &env.cache)
        .status()?;
    ensure!(approval.success(), "isolated fixture profile approval");
    Ok(())
}

fn scenario_with_values(
    profile_path: &Path,
    profile: &lantern_profile::ValidatedDeviceProfile,
    acceleration: &str,
    deceleration: &str,
) -> String {
    let mut source = scenario_source(profile_path, profile, Case::MatchDisarmed);
    source.push_str(&format!(
        "\n[initial_values]\n\"config.acceleration\" = \"{acceleration}\"\n\"config.deceleration\" = \"{deceleration}\"\n"
    ));
    source
}

fn two_step_restore_profile(base: &str) -> Result<String> {
    let restore_order = "restore_order = [\"config.acceleration\"]";
    ensure!(base.contains(restore_order), "reference profile restore_order changed");
    let source = base.replacen(
        restore_order,
        "restore_order = [\"config.acceleration\", \"config.deceleration\"]",
        1,
    );
    let marker = "[aliases]";
    ensure!(source.contains(marker), "reference profile aliases marker changed");
    let extra = r#"[[parameters]]
id = "config.deceleration"
code = "D0.02"
name = "Deceleration time"
description = "Fictional second writable parameter used by process restore acceptance"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 11 }
encoding = "unsigned16"
quantity = "time"
unit = "s"
minimum = "0.1"
maximum = "600"
step = "0.1"
forbidden_raw = [[0]]
access = "writable_when_stopped"
restore_policy = "normal"
required_drive_state = "stopped"
write_function = "write_single_register"
backup = true
read_back = { kind = "accepted_raw_set", values = [[80], [81]], documentation_source = "Process acceptance fixture", hil_report_id = "SIM-M6.5.2" }
scale = { multiplier = "1", divisor = "10", offset = "0", decimal_places = 1 }

[aliases]"#;
    Ok(source.replacen(marker, extra, 1))
}

fn backup_files(env: &CaseEnvironment) -> Result<Vec<PathBuf>> {
    let mut output = Vec::new();
    collect_backup_files(&env.data, &mut output)?;
    collect_backup_files(&env.state, &mut output)?;
    output.sort();
    Ok(output)
}

fn collect_backup_files(root: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_backup_files(&path, output)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(lantern_storage::BACKUP_SUFFIX))
        {
            output.push(path);
        }
    }
    Ok(())
}

fn assert_exact_restore_writes(records: &[SimulatorLogRecord], addresses: &[u16]) -> Result<()> {
    let writes = records
        .iter()
        .filter(|record| record.function == 6 || record.function == 16)
        .collect::<Vec<_>>();
    ensure!(
        writes.len() == addresses.len(),
        "restore emitted {} writes; expected {}",
        writes.len(),
        addresses.len()
    );
    for (record, expected_address) in writes.into_iter().zip(addresses) {
        ensure!(record.function == 6, "fixture restore must use FC06 only");
        ensure!(
            record.address == Some(*expected_address),
            "restore write order/address mismatch: {:?}",
            record.address
        );
    }
    Ok(())
}

fn assert_restore_audit(env: &CaseEnvironment, completed: bool) -> Result<()> {
    let audit_root = env.state.join("vfd-lantern/audit");
    let mut operation_started = 0;
    let mut operation_finished = 0;
    let mut prepared = 0;
    let mut finalized = 0;
    let mut verified = 0;
    let mut aborted = 0;
    for entry in fs::read_dir(&audit_root)? {
        let path = entry?.path();
        if !path.extension().is_some_and(|extension| extension == "jsonl") {
            continue;
        }
        for line in fs::read_to_string(path)?.lines() {
            let record: serde_json::Value = serde_json::from_str(line)?;
            match record["kind"].as_str() {
                Some("operation_started") => operation_started += 1,
                Some("operation_finished") => {
                    operation_finished += 1;
                    if record["body"]["outcome"] == "completed" {
                        verified += 1;
                    }
                    if record["body"]["outcome"] == "aborted" {
                        aborted += 1;
                    }
                }
                Some("device_write_prepared") => prepared += 1,
                Some("device_write_finalized") => finalized += 1,
                _ => {}
            }
        }
    }
    if completed {
        ensure!(operation_started >= 1 && operation_finished >= 1 && verified >= 1);
        ensure!(prepared >= 2 && finalized >= 2);
    } else {
        ensure!(operation_started >= 2 && operation_finished >= 2 && aborted >= 1);
        // Cumulative audit includes the prior successful case; the failure case adds two more steps.
        ensure!(prepared >= 4 && finalized >= 4);
    }
    Ok(())
}

struct ConformanceProcessSimulator {
    runtime: tokio::runtime::Runtime,
    simulator: Option<ConformanceSimulatorRuntime>,
}

impl ConformanceProcessSimulator {
    fn spawn(
        profile_path: &Path,
        core_path: &Path,
        conformance_path: &Path,
        behavior: &str,
    ) -> Result<Self> {
        let profile = Arc::new(lantern_sim::load_profile(profile_path)?);
        let core = Arc::new(load_scenario(core_path)?);
        fs::write(
            conformance_path,
            conformance_source(core_path, &core, behavior),
        )?;
        let conformance = Arc::new(load_conformance_scenario(conformance_path)?);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let simulator = {
            let _guard = runtime.enter();
            ConformanceSimulatorRuntime::spawn(profile, core, conformance)?
        };
        Ok(Self {
            runtime,
            simulator: Some(simulator),
        })
    }

    fn client_path(&self) -> &Path {
        self.simulator
            .as_ref()
            .expect("simulator is live")
            .client_path()
    }

    fn snapshot(&self) -> lantern_sim::ConformanceSimulatorSnapshot {
        self.simulator
            .as_ref()
            .expect("simulator is live")
            .control()
            .snapshot()
    }

    fn stop(&mut self) -> Result<Vec<SimulatorLogRecord>> {
        let simulator = self.simulator.as_mut().context("simulator already stopped")?;
        simulator.shutdown();
        self.runtime.block_on(simulator.wait())?;
        let records = simulator.control().structured_log();
        self.simulator = None;
        Ok(records)
    }
}

fn conformance_source(core_path: &Path, core: &LoadedScenario, behavior: &str) -> String {
    format!(
        "schema_version = 1\n\n[core]\nscenario_path = {:?}\nscenario_hash = {:?}\nprofile_hash = {:?}\nseed = {:?}\n\n{behavior}",
        core_path.to_string_lossy(),
        core.hash().to_hex(),
        core.document().profile_hash,
        core.document().seed,
    )
}

#[allow(dead_code)]
fn _assert_conformance_type(_: &LoadedConformanceScenario) {}
