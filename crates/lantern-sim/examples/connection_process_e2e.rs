use std::{
    fs::{self, File},
    io::{BufRead as _, BufReader, Read as _, Write as _},
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use nix::{
    pty::{Winsize, openpty},
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use serde::Deserialize;
use tempfile::TempDir;

const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const PROCESS_TIMEOUT: Duration = Duration::from_secs(8);
const SCREEN_SETTLE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy)]
enum ExpectedOutcome {
    MatchProcessOff,
    MatchDisarmed,
    Partial,
    MismatchWithExport,
    Ambiguous,
    Timeout,
    ProtocolException,
}

impl ExpectedOutcome {
    const fn name(self) -> &'static str {
        match self {
            Self::MatchProcessOff => "match-process-off",
            Self::MatchDisarmed => "match-disarmed",
            Self::Partial => "partial",
            Self::MismatchWithExport => "mismatch-export",
            Self::Ambiguous => "ambiguous",
            Self::Timeout => "timeout",
            Self::ProtocolException => "protocol-exception",
        }
    }

    const fn enable_writes(self) -> bool {
        matches!(self, Self::MatchDisarmed)
    }

    const fn expected_requests(self) -> usize {
        match self {
            Self::Partial => 2,
            _ => 1,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Handshake {
    pty: PathBuf,
}

#[derive(Debug, Deserialize)]
struct LogRecord {
    function: u8,
}

struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("child is present")
    }

    fn id(&self) -> u32 {
        self.child.as_ref().expect("child is present").id()
    }

    fn wait_timeout(&mut self, timeout: Duration) -> Result<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child_mut().try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                bail!("child {} did not exit within {timeout:?}", self.id());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child
            && child.try_wait().ok().flatten().is_none()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct TerminalChild {
    child: ChildGuard,
    writer: File,
    output: Arc<Mutex<Vec<u8>>>,
    reader: Option<thread::JoinHandle<()>>,
}

impl TerminalChild {
    fn spawn(binary: &Path, arguments: &[String], environment: &CaseEnvironment) -> Result<Self> {
        let size = Winsize {
            ws_row: 42,
            ws_col: 140,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = openpty(Some(&size), None).context("open controlled TUI PTY")?;
        let master = File::from(pty.master);
        let slave = File::from(pty.slave);
        let stdin = slave.try_clone().context("clone TUI stdin")?;
        let stdout = slave.try_clone().context("clone TUI stdout")?;

        let mut command = Command::new(binary);
        command
            .args(arguments)
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(slave))
            .env("TERM", "xterm-256color")
            .env("HOME", &environment.home)
            .env("XDG_CONFIG_HOME", &environment.config)
            .env("XDG_DATA_HOME", &environment.data)
            .env("XDG_STATE_HOME", &environment.state)
            .env("XDG_CACHE_HOME", &environment.cache);
        let child = command
            .spawn()
            .with_context(|| format!("spawn {}", binary.display()))?;

        let output = Arc::new(Mutex::new(Vec::new()));
        let reader_output = Arc::clone(&output);
        let mut reader = master.try_clone().context("clone TUI PTY master")?;
        let reader_thread = thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            let mut terminal_query_tail = Vec::new();
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let chunk = &buffer[..count];
                        lock_output(&reader_output).extend_from_slice(chunk);

                        terminal_query_tail.extend_from_slice(chunk);
                        let cursor_queries = terminal_query_tail
                            .windows(4)
                            .filter(|window| *window == b"\x1b[6n")
                            .count();
                        for _ in 0..cursor_queries {
                            if reader.write_all(b"\x1b[1;1R").is_err() || reader.flush().is_err() {
                                return;
                            }
                        }
                        if cursor_queries > 0 {
                            terminal_query_tail.clear();
                        } else if terminal_query_tail.len() > 3 {
                            let keep_from = terminal_query_tail.len() - 3;
                            terminal_query_tail.drain(..keep_from);
                        }
                    }
                    Err(error) if error.raw_os_error() == Some(nix::libc::EIO) => break,
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            child: ChildGuard::new(child),
            writer: master,
            output,
            reader: Some(reader_thread),
        })
    }

    fn send(&mut self, text: &str) -> Result<()> {
        self.writer
            .write_all(text.as_bytes())
            .and_then(|()| self.writer.flush())
            .context("write controlled TUI input")
    }

    fn wait_for(&self, needle: &str) -> Result<()> {
        let deadline = Instant::now() + PROCESS_TIMEOUT;
        loop {
            let text = self.text();
            if text.contains(needle) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!(
                    "TUI did not render {needle:?}; output tail:\n{}",
                    tail(&text, 6000)
                );
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn assert_not_contains(&self, needle: &str) -> Result<()> {
        let text = self.text();
        ensure!(
            !text.contains(needle),
            "TUI unexpectedly rendered {needle:?}; output tail:\n{}",
            tail(&text, 4000)
        );
        Ok(())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&lock_output(&self.output)).into_owned()
    }

    fn quit_and_wait(mut self) -> Result<()> {
        self.send("q")?;
        let status = self.child.wait_timeout(PROCESS_TIMEOUT)?;
        ensure!(status.success(), "vfd-lantern exited with {status}");
        drop(self.writer);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        Ok(())
    }
}

struct SimulatorProcess {
    child: ChildGuard,
    handshake: Handshake,
    log_path: PathBuf,
}

impl SimulatorProcess {
    fn spawn(binary: &Path, profile: &Path, scenario: &Path, log_path: PathBuf) -> Result<Self> {
        let mut child = Command::new(binary)
            .arg("--profile")
            .arg(profile)
            .arg("--scenario")
            .arg(scenario)
            .arg("--log")
            .arg(&log_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawn {}", binary.display()))?;
        let stdout = child.stdout.take().context("simulator stdout")?;
        let (sender, receiver) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
            let _ = sender.send(result);
        });
        let line = receiver
            .recv_timeout(PROCESS_TIMEOUT)
            .context("wait for simulator handshake")??;
        let handshake: Handshake = serde_json::from_str(line.trim())
            .with_context(|| format!("parse simulator handshake {line:?}"))?;
        Ok(Self {
            child: ChildGuard::new(child),
            handshake,
            log_path,
        })
    }

    fn stop_and_read_log(mut self) -> Result<Vec<LogRecord>> {
        let pid = i32::try_from(self.child.id()).context("simulator pid")?;
        match kill(Pid::from_raw(pid), Signal::SIGINT) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => return Err(error).context("send SIGINT to lantern-sim"),
        }
        let status = self.child.wait_timeout(PROCESS_TIMEOUT)?;
        ensure!(status.success(), "lantern-sim exited with {status}");
        let source = fs::read_to_string(&self.log_path)
            .with_context(|| format!("read simulator log {}", self.log_path.display()))?;
        source
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).context("parse simulator JSONL record"))
            .collect()
    }
}

struct CaseEnvironment {
    root: TempDir,
    home: PathBuf,
    config: PathBuf,
    data: PathBuf,
    state: PathBuf,
    cache: PathBuf,
}

impl CaseEnvironment {
    fn new() -> Result<Self> {
        let root = tempfile::tempdir().context("case tempdir")?;
        let home = root.path().join("home");
        let config = root.path().join("config");
        let data = root.path().join("data");
        let state = root.path().join("state");
        let cache = root.path().join("cache");
        for directory in [&home, &config, &data, &state, &cache] {
            fs::create_dir_all(directory)
                .with_context(|| format!("create {}", directory.display()))?;
        }
        Ok(Self {
            root,
            home,
            config,
            data,
            state,
            cache,
        })
    }

    fn add_ambiguous_user_profile(&self, source: &str) -> Result<()> {
        // `directories::ProjectDirs` uses the application component on Linux. The extra
        // candidates keep this harness robust to an organization-qualified layout without
        // relaxing the product's own profile-source rules.
        for relative in [
            "vfd-lantern/profiles",
            "aiteracja/vfd-lantern/profiles",
            "pl/aiteracja/vfd-lantern/profiles",
        ] {
            let directory = self.config.join(relative);
            fs::create_dir_all(&directory)?;
            fs::write(directory.join("ambiguous-vfd.toml"), source)?;
        }
        Ok(())
    }

    fn exported_reports(&self) -> Result<Vec<PathBuf>> {
        let mut reports = Vec::new();
        collect_named_files(&self.state, "identification-", ".json", &mut reports)?;
        collect_named_files(&self.data, "identification-", ".json", &mut reports)?;
        Ok(reports)
    }
}

fn main() -> Result<()> {
    let debug_directory = debug_directory()?;
    let simulator = debug_directory.join("lantern-sim");
    let product = debug_directory.join("vfd-lantern");
    ensure!(
        simulator.is_file(),
        "missing built binary {}",
        simulator.display()
    );
    ensure!(
        product.is_file(),
        "missing built binary {}",
        product.display()
    );

    for outcome in [
        ExpectedOutcome::MatchProcessOff,
        ExpectedOutcome::MatchDisarmed,
        ExpectedOutcome::Partial,
        ExpectedOutcome::MismatchWithExport,
        ExpectedOutcome::Ambiguous,
        ExpectedOutcome::Timeout,
        ExpectedOutcome::ProtocolException,
    ] {
        run_case(&simulator, &product, outcome)
            .with_context(|| format!("process E2E case {}", outcome.name()))?;
        println!("process-e2e {} ok", outcome.name());
    }

    run_reconnect_identity_change_case(&simulator, &product)
        .context("process E2E case reconnect-identity-change")?;
    println!("process-e2e reconnect-identity-change ok");
    Ok(())
}

fn run_case(
    simulator_binary: &Path,
    product_binary: &Path,
    outcome: ExpectedOutcome,
) -> Result<()> {
    let environment = CaseEnvironment::new()?;
    let base_profile = fs::read_to_string(reference_profile())?;
    let selected_source = if matches!(outcome, ExpectedOutcome::Partial) {
        profile_with_partial_probe(&base_profile)?
    } else {
        base_profile.clone()
    };
    let selected_profile = environment.root.path().join("selected-vfd.toml");
    fs::write(&selected_profile, &selected_source)?;
    let profile = lantern_sim::load_profile(&selected_profile)?;

    if matches!(outcome, ExpectedOutcome::Ambiguous) {
        let other = base_profile.replacen(
            "profile_id = \"example.vfd1000\"",
            "profile_id = \"example.vfd2000\"",
            1,
        );
        environment.add_ambiguous_user_profile(&other)?;
    }

    let scenario = environment
        .root
        .path()
        .join(format!("{}.toml", outcome.name()));
    fs::write(
        &scenario,
        scenario_source(&selected_profile, &profile, outcome),
    )?;
    let simulator_log = environment.root.path().join("simulator.jsonl");
    let simulator = SimulatorProcess::spawn(
        simulator_binary,
        &selected_profile,
        &scenario,
        simulator_log,
    )?;

    let mut arguments = vec![
        "--profile".to_owned(),
        selected_profile.to_string_lossy().into_owned(),
        "--device".to_owned(),
        simulator.handshake.pty.to_string_lossy().into_owned(),
        "--no-color".to_owned(),
    ];
    if outcome.enable_writes() {
        arguments.push("--enable-writes".to_owned());
    }
    let mut product = TerminalChild::spawn(product_binary, &arguments, &environment)?;
    drive_wizard_to_connect(&mut product)?;

    match outcome {
        ExpectedOutcome::MatchProcessOff => {
            product.wait_for("Verified read-only session established")?;
            product.wait_for("PROCESS-OFF")?;
            product.assert_not_contains("authorization=ARMED")?;
        }
        ExpectedOutcome::MatchDisarmed => {
            product.wait_for("Verified read-only session established")?;
            product.wait_for("DISARMED")?;
            product.assert_not_contains("authorization=ARMED")?;
        }
        ExpectedOutcome::Partial => {
            product.wait_for("Outcome=Partial")?;
            product.wait_for("Verified session: NOT CREATED")?;
        }
        ExpectedOutcome::MismatchWithExport => {
            product.wait_for("Outcome=Mismatch")?;
            product.wait_for("Verified session: NOT CREATED")?;
            product.send("e")?;
            product.wait_for("Last export:")?;
        }
        ExpectedOutcome::Ambiguous => {
            product.wait_for("Outcome=Ambiguous")?;
            product.wait_for("Verified session: NOT CREATED")?;
        }
        ExpectedOutcome::Timeout => {
            product.wait_for("Outcome=Error")?;
            product.wait_for("quality=Timeout")?;
            product.wait_for("Verified session: NOT CREATED")?;
        }
        ExpectedOutcome::ProtocolException => {
            product.wait_for("Outcome=Error")?;
            product.wait_for("quality=ProtocolException")?;
            product.wait_for("Verified session: NOT CREATED")?;
        }
    }

    thread::sleep(SCREEN_SETTLE);
    product.quit_and_wait()?;
    let records = simulator.stop_and_read_log()?;
    assert_read_only_request_count(outcome.name(), &records, outcome.expected_requests())?;

    if matches!(outcome, ExpectedOutcome::MismatchWithExport) {
        let reports = environment.exported_reports()?;
        ensure!(
            reports.len() == 1,
            "expected one offline identification report, found {reports:?}"
        );
        let report: serde_json::Value = serde_json::from_slice(&fs::read(&reports[0])?)?;
        ensure!(
            report["outcome"] == "mismatch",
            "unexpected exported report: {report}"
        );
    }
    Ok(())
}

fn run_reconnect_identity_change_case(
    simulator_binary: &Path,
    product_binary: &Path,
) -> Result<()> {
    let environment = CaseEnvironment::new()?;
    let selected_profile = environment.root.path().join("selected-vfd.toml");
    fs::write(&selected_profile, fs::read_to_string(reference_profile())?)?;
    let profile = lantern_sim::load_profile(&selected_profile)?;
    let scenario = environment.root.path().join("reconnect.toml");
    fs::write(
        &scenario,
        scenario_source(
            &selected_profile,
            &profile,
            ExpectedOutcome::MatchProcessOff,
        ),
    )?;

    let first = SimulatorProcess::spawn(
        simulator_binary,
        &selected_profile,
        &scenario,
        environment.root.path().join("simulator-first.jsonl"),
    )?;
    let second = SimulatorProcess::spawn(
        simulator_binary,
        &selected_profile,
        &scenario,
        environment.root.path().join("simulator-second.jsonl"),
    )?;
    let manual_link = environment.root.path().join("manual-vfd");
    symlink(&first.handshake.pty, &manual_link).context("link first simulated adapter")?;

    let arguments = vec![
        "--profile".to_owned(),
        selected_profile.to_string_lossy().into_owned(),
        "--device".to_owned(),
        manual_link.to_string_lossy().into_owned(),
        "--no-color".to_owned(),
    ];
    let mut product = TerminalChild::spawn(product_binary, &arguments, &environment)?;
    drive_wizard_to_connect(&mut product)?;
    product.wait_for("Verified read-only session established")?;
    product.wait_for("PROCESS-OFF")?;

    fs::remove_file(&manual_link).context("remove selected manual adapter path")?;
    product.wait_for("RECONNECTING")?;
    symlink(&second.handshake.pty, &manual_link).context("link replacement simulated adapter")?;

    let first_records = first.stop_and_read_log()?;
    product.wait_for("reconnect identity did not match the verified session")?;
    product.wait_for("FAULTED")?;
    product.assert_not_contains("authorization=ARMED")?;

    thread::sleep(SCREEN_SETTLE);
    product.quit_and_wait()?;
    let second_records = second.stop_and_read_log()?;
    assert_read_only_request_count("reconnect-initial", &first_records, 1)?;
    assert_read_only_request_count("reconnect-replacement", &second_records, 1)?;
    Ok(())
}

fn assert_read_only_request_count(
    case: &str,
    records: &[LogRecord],
    expected: usize,
) -> Result<()> {
    ensure!(
        records.len() == expected,
        "{case} sent {} Modbus requests; expected exactly {expected} bounded identification reads",
        records.len()
    );
    ensure!(
        records
            .iter()
            .all(|record| matches!(record.function, 3 | 4)),
        "{case} emitted a non-read Modbus function: {:?}",
        records
            .iter()
            .map(|record| record.function)
            .collect::<Vec<_>>()
    );
    Ok(())
}

fn drive_wizard_to_connect(product: &mut TerminalChild) -> Result<()> {
    product.wait_for("step Port")?;
    product.send("m")?;
    product.wait_for("Manual device path:")?;
    product.send("\r")?;

    product.wait_for("step Profile")?;
    product.send("/")?;
    product.wait_for("Profile search:")?;
    product.send("example.vfd1000")?;
    product.send("\r")?;
    product.wait_for("Profile filter:")?;
    product.send("\r")?;

    product.wait_for("step Link")?;
    product.send("\r")?;
    product.wait_for("step Summary")?;
    product.wait_for("[Manual]")?;
    product.wait_for("stable=-")?;
    product.send("\r")?;
    Ok(())
}

fn scenario_source(
    profile_path: &Path,
    profile: &lantern_profile::ValidatedDeviceProfile,
    outcome: ExpectedOutcome,
) -> String {
    let extra = match outcome {
        ExpectedOutcome::Partial => "[probe_overrides]\naux = [9999]\n",
        ExpectedOutcome::MismatchWithExport => "[probe_overrides]\nmodel = [4097]\n",
        ExpectedOutcome::Timeout => "[[read_behaviors]]\nstart_request = 1\nkind = \"timeout\"\n",
        ExpectedOutcome::ProtocolException => {
            "[[read_behaviors]]\nstart_request = 1\nkind = \"exception\"\ncode = 2\n"
        }
        ExpectedOutcome::MatchProcessOff
        | ExpectedOutcome::MatchDisarmed
        | ExpectedOutcome::Ambiguous => "",
    };
    format!(
        "schema_version = 1\nprofile_path = {:?}\nprofile_hash = {:?}\nslave_id = 1\nfingerprint = \"process.issue13\"\nseed = \"{SEED}\"\ntick_micros = 1000\n\n{extra}",
        profile_path.to_string_lossy(),
        profile.profile_hash().to_hex(),
    )
}

fn profile_with_partial_probe(base: &str) -> Result<String> {
    let marker = "[[parameters]]";
    let insertion = r#"[[identification.probes]]
id = "aux"
description = "Second process-level identity word"
table = "holding_registers"
count = 1
expected_raw = [[1234]]
address = { notation = "pdu_zero_based", value = 100 }

[[parameters]]"#;
    ensure!(
        base.contains(marker),
        "reference profile has no parameter marker"
    );
    Ok(base.replacen(marker, insertion, 1))
}

fn reference_profile() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/example-vfd.toml")
}

fn debug_directory() -> Result<PathBuf> {
    let executable = std::env::current_exe().context("current process E2E executable")?;
    executable
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("derive target/debug directory")
}

fn lock_output(output: &Arc<Mutex<Vec<u8>>>) -> std::sync::MutexGuard<'_, Vec<u8>> {
    output
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn tail(text: &str, maximum: usize) -> &str {
    if text.len() <= maximum {
        text
    } else {
        let mut start = text.len() - maximum;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        &text[start..]
    }
}

fn collect_named_files(
    root: &Path,
    prefix: &str,
    suffix: &str,
    output: &mut Vec<PathBuf>,
) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_named_files(&path, prefix, suffix, output)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(prefix) && name.ends_with(suffix))
        {
            output.push(path);
        }
    }
    Ok(())
}
