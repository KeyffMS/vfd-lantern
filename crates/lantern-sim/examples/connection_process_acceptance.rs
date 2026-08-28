use std::{
    fs::{self, File},
    io::{BufRead as _, BufReader, Read as _, Write as _},
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, MutexGuard},
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

const ROWS: usize = 42;
const COLS: usize = 140;
const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const PROCESS_TIMEOUT: Duration = Duration::from_secs(8);
const BEFORE_CONNECT_SETTLE: Duration = Duration::from_millis(150);
const SCREEN_SETTLE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy)]
enum Case {
    MatchProcessOff,
    MatchDisarmed,
    Partial,
    MismatchExport,
    Ambiguous,
    Timeout,
    ProtocolException,
}

impl Case {
    const fn name(self) -> &'static str {
        match self {
            Self::MatchProcessOff => "match-process-off",
            Self::MatchDisarmed => "match-disarmed",
            Self::Partial => "partial",
            Self::MismatchExport => "mismatch-export",
            Self::Ambiguous => "ambiguous",
            Self::Timeout => "timeout",
            Self::ProtocolException => "protocol-exception",
        }
    }

    const fn enable_writes(self) -> bool {
        matches!(self, Self::MatchDisarmed)
    }

    const fn expected_requests(self) -> usize {
        if matches!(self, Self::Partial) { 2 } else { 1 }
    }
}

#[derive(Debug, Deserialize)]
struct Handshake {
    pty: PathBuf,
}

#[derive(Debug, Deserialize)]
struct StructuredLogLine {
    record: String,
    function: Option<u8>,
}

#[derive(Debug)]
struct LogRecord {
    function: u8,
}

struct ChildGuard(Child);

impl ChildGuard {
    fn id(&self) -> u32 {
        self.0.id()
    }

    fn wait_timeout(&mut self) -> Result<std::process::ExitStatus> {
        let deadline = Instant::now() + PROCESS_TIMEOUT;
        loop {
            if let Some(status) = self.0.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                bail!(
                    "child {} did not exit within {PROCESS_TIMEOUT:?}",
                    self.id()
                );
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

struct Screen {
    cells: Vec<u8>,
    row: usize,
    col: usize,
    saved: (usize, usize),
}

impl Screen {
    fn new() -> Self {
        Self {
            cells: vec![b' '; ROWS * COLS],
            row: 0,
            col: 0,
            saved: (0, 0),
        }
    }

    fn from_terminal_stream(bytes: &[u8]) -> Self {
        let mut screen = Self::new();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                0x1b => index = screen.escape(bytes, index + 1),
                b'\r' => {
                    screen.col = 0;
                    index += 1;
                }
                b'\n' => {
                    screen.row = (screen.row + 1).min(ROWS - 1);
                    index += 1;
                }
                0x08 => {
                    screen.col = screen.col.saturating_sub(1);
                    index += 1;
                }
                b'\t' => {
                    screen.col = ((screen.col / 8) + 1).saturating_mul(8).min(COLS - 1);
                    index += 1;
                }
                byte @ 0x20..=0x7e => {
                    screen.put(byte);
                    index += 1;
                }
                byte @ 0x80..=0xff => {
                    screen.put(b'?');
                    index += utf8_sequence_length(byte).min(bytes.len() - index);
                }
                _ => index += 1,
            }
        }
        screen
    }

    fn escape(&mut self, bytes: &[u8], mut index: usize) -> usize {
        if index >= bytes.len() {
            return index;
        }
        match bytes[index] {
            b'[' => {
                index += 1;
                let start = index;
                while index < bytes.len() && !(0x40..=0x7e).contains(&bytes[index]) {
                    index += 1;
                }
                if index < bytes.len() {
                    self.csi(&bytes[start..index], bytes[index]);
                    index + 1
                } else {
                    index
                }
            }
            b']' => {
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == 0x07 {
                        return index + 1;
                    }
                    if bytes[index] == 0x1b && bytes.get(index + 1).copied() == Some(b'\\') {
                        return index + 2;
                    }
                    index += 1;
                }
                index
            }
            b'7' => {
                self.saved = (self.row, self.col);
                index + 1
            }
            b'8' => {
                (self.row, self.col) = self.saved;
                index + 1
            }
            _ => index + 1,
        }
    }

    fn csi(&mut self, raw: &[u8], final_byte: u8) {
        let private = raw.first().copied() == Some(b'?');
        let params_raw = if private { &raw[1..] } else { raw };
        let params = params_raw
            .split(|byte| *byte == b';')
            .map(|part| {
                std::str::from_utf8(part)
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>();
        let p = |index: usize, default: usize| {
            params
                .get(index)
                .copied()
                .filter(|value| *value != 0)
                .unwrap_or(default)
        };

        match final_byte {
            b'H' | b'f' => {
                self.row = p(0, 1).saturating_sub(1).min(ROWS - 1);
                self.col = p(1, 1).saturating_sub(1).min(COLS - 1);
            }
            b'G' => self.col = p(0, 1).saturating_sub(1).min(COLS - 1),
            b'd' => self.row = p(0, 1).saturating_sub(1).min(ROWS - 1),
            b'A' => self.row = self.row.saturating_sub(p(0, 1)),
            b'B' => self.row = (self.row + p(0, 1)).min(ROWS - 1),
            b'C' => self.col = (self.col + p(0, 1)).min(COLS - 1),
            b'D' => self.col = self.col.saturating_sub(p(0, 1)),
            b'E' => {
                self.row = (self.row + p(0, 1)).min(ROWS - 1);
                self.col = 0;
            }
            b'F' => {
                self.row = self.row.saturating_sub(p(0, 1));
                self.col = 0;
            }
            b'J' => self.erase_display(p(0, 0)),
            b'K' => self.erase_line(p(0, 0)),
            b'X' => self.erase_chars(p(0, 1)),
            b's' => self.saved = (self.row, self.col),
            b'u' => (self.row, self.col) = self.saved,
            b'h' if private && params.contains(&1049) => self.clear(),
            b'l' | b'h' | b'm' | b'n' | b'r' | b'q' => {}
            _ => {}
        }
    }

    fn put(&mut self, byte: u8) {
        if self.col >= COLS {
            self.col = 0;
            self.row = (self.row + 1).min(ROWS - 1);
        }
        self.cells[self.row * COLS + self.col] = byte;
        self.col += 1;
    }

    fn clear(&mut self) {
        self.cells.fill(b' ');
        self.row = 0;
        self.col = 0;
    }

    fn erase_display(&mut self, mode: usize) {
        let cursor = self.row * COLS + self.col.min(COLS - 1);
        match mode {
            0 => self.cells[cursor..].fill(b' '),
            1 => self.cells[..=cursor].fill(b' '),
            2 | 3 => self.cells.fill(b' '),
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: usize) {
        let start = self.row * COLS;
        let col = self.col.min(COLS - 1);
        match mode {
            0 => self.cells[start + col..start + COLS].fill(b' '),
            1 => self.cells[start..=start + col].fill(b' '),
            2 => self.cells[start..start + COLS].fill(b' '),
            _ => {}
        }
    }

    fn erase_chars(&mut self, count: usize) {
        let start = self.row * COLS + self.col.min(COLS - 1);
        let end = (start + count).min((self.row + 1) * COLS);
        self.cells[start..end].fill(b' ');
    }

    fn text(&self) -> String {
        let mut lines = Vec::new();
        for row in 0..ROWS {
            let cells = &self.cells[row * COLS..(row + 1) * COLS];
            let end = cells
                .iter()
                .rposition(|byte| *byte != b' ')
                .map_or(0, |index| index + 1);
            lines.push(String::from_utf8_lossy(&cells[..end]).into_owned());
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }
}

struct TerminalChild {
    child: ChildGuard,
    writer: File,
    output: Arc<Mutex<Vec<u8>>>,
    reader: Option<thread::JoinHandle<()>>,
}

impl TerminalChild {
    fn spawn(binary: &Path, args: &[String], environment: &CaseEnvironment) -> Result<Self> {
        let size = Winsize {
            ws_row: ROWS as u16,
            ws_col: COLS as u16,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = openpty(Some(&size), None).context("open controlled TUI PTY")?;
        let master = File::from(pty.master);
        let slave = File::from(pty.slave);
        let child = Command::new(binary)
            .args(args)
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave))
            .env("TERM", "xterm-256color")
            .env("HOME", &environment.home)
            .env("XDG_CONFIG_HOME", &environment.config)
            .env("XDG_DATA_HOME", &environment.data)
            .env("XDG_STATE_HOME", &environment.state)
            .env("XDG_CACHE_HOME", &environment.cache)
            .spawn()
            .with_context(|| format!("spawn {}", binary.display()))?;

        let output = Arc::new(Mutex::new(Vec::new()));
        let reader_output = Arc::clone(&output);
        let mut reader = master.try_clone()?;
        let reader_thread = thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            let mut query_tail = Vec::new();
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let chunk = &buffer[..count];
                        lock_output(&reader_output).extend_from_slice(chunk);
                        query_tail.extend_from_slice(chunk);
                        let queries = query_tail
                            .windows(4)
                            .filter(|window| *window == b"\x1b[6n")
                            .count();
                        for _ in 0..queries {
                            if reader.write_all(b"\x1b[1;1R").is_err() || reader.flush().is_err() {
                                return;
                            }
                        }
                        if queries > 0 {
                            query_tail.clear();
                        } else if query_tail.len() > 3 {
                            let keep = query_tail.len() - 3;
                            query_tail.drain(..keep);
                        }
                    }
                    Err(error) if error.raw_os_error() == Some(nix::libc::EIO) => break,
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            child: ChildGuard(child),
            writer: master,
            output,
            reader: Some(reader_thread),
        })
    }

    fn send(&mut self, input: &str) -> Result<()> {
        self.writer.write_all(input.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    fn screen_text(&self) -> String {
        Screen::from_terminal_stream(&lock_output(&self.output)).text()
    }

    fn wait_for(&self, needle: &str) -> Result<()> {
        let deadline = Instant::now() + PROCESS_TIMEOUT;
        loop {
            let text = self.screen_text();
            if text.contains(needle) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!(
                    "TUI did not render {needle:?}; reconstructed screen:\n{}",
                    tail(&text, 6000)
                );
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn assert_not_contains(&self, needle: &str) -> Result<()> {
        let text = self.screen_text();
        ensure!(
            !text.contains(needle),
            "TUI unexpectedly rendered {needle:?}; reconstructed screen:\n{}",
            tail(&text, 4000)
        );
        Ok(())
    }

    fn quit(mut self) -> Result<()> {
        self.send("q")?;
        let status = self.child.wait_timeout()?;
        ensure!(status.success(), "vfd-lantern exited with {status}");
        drop(self.writer);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        Ok(())
    }
}

struct Simulator {
    child: ChildGuard,
    pty: PathBuf,
    log_path: PathBuf,
}

impl Simulator {
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
            .spawn()?;
        let stdout = child.stdout.take().context("simulator stdout")?;
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
            let _ = tx.send(result);
        });
        let line = rx
            .recv_timeout(PROCESS_TIMEOUT)
            .context("wait for simulator handshake")??;
        let handshake: Handshake = serde_json::from_str(line.trim())?;
        Ok(Self {
            child: ChildGuard(child),
            pty: handshake.pty,
            log_path,
        })
    }

    fn stop(mut self) -> Result<Vec<LogRecord>> {
        let pid = i32::try_from(self.child.id()).context("simulator pid")?;
        match kill(Pid::from_raw(pid), Signal::SIGINT) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => return Err(error).context("send SIGINT to lantern-sim"),
        }
        let status = self.child.wait_timeout()?;
        ensure!(status.success(), "lantern-sim exited with {status}");
        read_log_records(&self.log_path)
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
        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        let config = root.path().join("config");
        let data = root.path().join("data");
        let state = root.path().join("state");
        let cache = root.path().join("cache");
        for path in [&home, &config, &data, &state, &cache] {
            fs::create_dir_all(path)?;
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

    fn add_ambiguous_profile(&self, source: &str) -> Result<()> {
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

    fn reports(&self) -> Result<Vec<PathBuf>> {
        let mut reports = Vec::new();
        collect_named_files(&self.state, &mut reports)?;
        collect_named_files(&self.data, &mut reports)?;
        Ok(reports)
    }
}

fn main() -> Result<()> {
    let debug = debug_directory()?;
    let simulator = debug.join("lantern-sim");
    let product = debug.join("vfd-lantern");
    ensure!(simulator.is_file(), "missing {}", simulator.display());
    ensure!(product.is_file(), "missing {}", product.display());

    run_no_traffic_before_connect_case(&simulator, &product)?;
    println!("process-e2e no-traffic-before-connect ok");

    for case in [
        Case::MatchProcessOff,
        Case::MatchDisarmed,
        Case::Partial,
        Case::MismatchExport,
        Case::Ambiguous,
        Case::Timeout,
        Case::ProtocolException,
    ] {
        run_case(&simulator, &product, case)
            .with_context(|| format!("process E2E case {}", case.name()))?;
        println!("process-e2e {} ok", case.name());
    }
    run_reconnect_case(&simulator, &product)?;
    println!("process-e2e reconnect-identity-change ok");
    Ok(())
}

fn run_no_traffic_before_connect_case(
    simulator_binary: &Path,
    product_binary: &Path,
) -> Result<()> {
    let env = CaseEnvironment::new()?;
    let selected = env.root.path().join("selected-vfd.toml");
    fs::write(&selected, fs::read_to_string(reference_profile())?)?;
    let profile = lantern_sim::load_profile(&selected)?;
    let scenario = env.root.path().join("no-traffic-before-connect.toml");
    fs::write(
        &scenario,
        scenario_source(&selected, &profile, Case::MatchProcessOff),
    )?;
    let simulator = Simulator::spawn(
        simulator_binary,
        &selected,
        &scenario,
        env.root.path().join("no-traffic-before-connect.jsonl"),
    )?;
    let mut product = TerminalChild::spawn(
        product_binary,
        &product_args(&selected, &simulator.pty),
        &env,
    )?;

    drive_to_summary(&mut product)?;
    thread::sleep(BEFORE_CONNECT_SETTLE);
    product.quit()?;
    let requests = simulator.stop()?;
    ensure!(
        requests.is_empty(),
        "product transmitted {} Modbus request(s) before explicit Connect",
        requests.len()
    );
    Ok(())
}

fn run_case(simulator_binary: &Path, product_binary: &Path, case: Case) -> Result<()> {
    let env = CaseEnvironment::new()?;
    let base = fs::read_to_string(reference_profile())?;
    let selected_source = if matches!(case, Case::Partial) {
        profile_with_partial_probe(&base)?
    } else {
        base.clone()
    };
    let selected = env.root.path().join("selected-vfd.toml");
    fs::write(&selected, &selected_source)?;
    let profile = lantern_sim::load_profile(&selected)?;
    if matches!(case, Case::Ambiguous) {
        env.add_ambiguous_profile(&base.replacen(
            "profile_id = \"example.vfd1000\"",
            "profile_id = \"example.vfd2000\"",
            1,
        ))?;
    }
    let scenario = env.root.path().join(format!("{}.toml", case.name()));
    fs::write(&scenario, scenario_source(&selected, &profile, case))?;
    let simulator = Simulator::spawn(
        simulator_binary,
        &selected,
        &scenario,
        env.root.path().join("simulator.jsonl"),
    )?;

    let mut args = product_args(&selected, &simulator.pty);
    if case.enable_writes() {
        args.push("--enable-writes".to_owned());
    }
    let mut product = TerminalChild::spawn(product_binary, &args, &env)?;
    drive_to_summary(&mut product)?;
    product.send("\r")?;

    match case {
        Case::MatchProcessOff => {
            product.wait_for("Verified read-only session established")?;
            product.wait_for("PROCESS-OFF")?;
            product.assert_not_contains("authorization=ARMED")?;
        }
        Case::MatchDisarmed => {
            product.wait_for("Verified read-only session established")?;
            product.wait_for("DISARMED")?;
            product.assert_not_contains("authorization=ARMED")?;
        }
        Case::Partial => {
            product.wait_for("Outcome=Partial")?;
            product.wait_for("Verified session: NOT CREATED")?;
        }
        Case::MismatchExport => {
            product.wait_for("Outcome=Mismatch")?;
            product.wait_for("Verified session: NOT CREATED")?;
            product.send("e")?;
            product.wait_for("Last export:")?;
        }
        Case::Ambiguous => {
            product.wait_for("Outcome=Ambiguous")?;
            product.wait_for("Verified session: NOT CREATED")?;
        }
        Case::Timeout => {
            product.wait_for("Outcome=Error")?;
            product.wait_for("quality=Timeout")?;
            product.wait_for("Verified session: NOT CREATED")?;
        }
        Case::ProtocolException => {
            product.wait_for("Outcome=Error")?;
            product.wait_for("quality=ProtocolException")?;
            product.wait_for("Verified session: NOT CREATED")?;
        }
    }

    thread::sleep(SCREEN_SETTLE);
    product.quit()?;
    let records = simulator.stop()?;
    assert_read_only(case.name(), &records, case.expected_requests())?;

    if matches!(case, Case::MismatchExport) {
        let reports = env.reports()?;
        ensure!(reports.len() == 1, "expected one export, found {reports:?}");
        let report: serde_json::Value = serde_json::from_slice(&fs::read(&reports[0])?)?;
        ensure!(
            report["outcome"] == "mismatch",
            "unexpected report {report}"
        );
    }
    Ok(())
}

fn run_reconnect_case(simulator_binary: &Path, product_binary: &Path) -> Result<()> {
    let env = CaseEnvironment::new()?;
    let selected = env.root.path().join("selected-vfd.toml");
    fs::write(&selected, fs::read_to_string(reference_profile())?)?;
    let profile = lantern_sim::load_profile(&selected)?;
    let scenario = env.root.path().join("reconnect.toml");
    fs::write(
        &scenario,
        scenario_source(&selected, &profile, Case::MatchProcessOff),
    )?;
    let first = Simulator::spawn(
        simulator_binary,
        &selected,
        &scenario,
        env.root.path().join("first.jsonl"),
    )?;
    let second = Simulator::spawn(
        simulator_binary,
        &selected,
        &scenario,
        env.root.path().join("second.jsonl"),
    )?;
    let manual = env.root.path().join("manual-vfd");
    symlink(&first.pty, &manual)?;

    let mut product =
        TerminalChild::spawn(product_binary, &product_args(&selected, &manual), &env)?;
    drive_to_summary(&mut product)?;
    product.send("\r")?;
    product.wait_for("Verified read-only session established")?;
    product.wait_for("PROCESS-OFF")?;

    fs::remove_file(&manual)?;
    product.wait_for("RECONNECTING")?;
    symlink(&second.pty, &manual)?;
    let first_records = first.stop()?;
    product.wait_for("reconnect identity did not match the verified session")?;
    product.wait_for("FAULTED")?;
    product.assert_not_contains("authorization=ARMED")?;

    product.quit()?;
    let second_records = second.stop()?;
    assert_read_only("reconnect-initial", &first_records, 1)?;
    assert_read_only("reconnect-replacement", &second_records, 1)?;
    Ok(())
}

fn drive_to_summary(product: &mut TerminalChild) -> Result<()> {
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
    product.wait_for("profile_hash=")?;
    product.wait_for("source_hash=")?;
    product.wait_for("Identification probes")?;
    Ok(())
}

fn assert_read_only(case: &str, records: &[LogRecord], expected: usize) -> Result<()> {
    ensure!(
        records.len() == expected,
        "{case} sent {} requests; expected {expected}",
        records.len()
    );
    ensure!(
        records
            .iter()
            .all(|record| matches!(record.function, 3 | 4)),
        "{case} emitted non-read Modbus function"
    );
    Ok(())
}

fn product_args(profile: &Path, device: &Path) -> Vec<String> {
    vec![
        "--profile".to_owned(),
        profile.to_string_lossy().into_owned(),
        "--device".to_owned(),
        device.to_string_lossy().into_owned(),
        "--no-color".to_owned(),
    ]
}

fn scenario_source(
    profile_path: &Path,
    profile: &lantern_profile::ValidatedDeviceProfile,
    case: Case,
) -> String {
    let extra = match case {
        Case::Partial => "[probe_overrides]\naux = [9999]\n",
        Case::MismatchExport => "[probe_overrides]\nmodel = [4097]\n",
        Case::Timeout => "[[read_behaviors]]\nstart_request = 1\nkind = \"timeout\"\n",
        Case::ProtocolException => {
            "[[read_behaviors]]\nstart_request = 1\nkind = \"exception\"\ncode = 2\n"
        }
        Case::MatchProcessOff | Case::MatchDisarmed | Case::Ambiguous => "",
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
    ensure!(base.contains(marker), "reference profile has no parameters");
    Ok(base.replacen(marker, insertion, 1))
}

fn reference_profile() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/example-vfd.toml")
}

fn debug_directory() -> Result<PathBuf> {
    std::env::current_exe()?
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("derive target/debug directory")
}

fn read_log_records(path: &Path) -> Result<Vec<LogRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let source = fs::read_to_string(path)?;
    let mut requests = Vec::new();
    for line in source.lines().filter(|line| !line.trim().is_empty()) {
        let parsed: StructuredLogLine =
            serde_json::from_str(line).context("parse simulator JSONL record")?;
        if parsed.record == "request" {
            requests.push(LogRecord {
                function: parsed
                    .function
                    .context("simulator request record is missing function")?,
            });
        }
    }
    Ok(requests)
}

fn collect_named_files(root: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_named_files(&path, output)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("identification-") && name.ends_with(".json"))
        {
            output.push(path);
        }
    }
    Ok(())
}

fn utf8_sequence_length(first: u8) -> usize {
    match first {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

fn lock_output(output: &Arc<Mutex<Vec<u8>>>) -> MutexGuard<'_, Vec<u8>> {
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
