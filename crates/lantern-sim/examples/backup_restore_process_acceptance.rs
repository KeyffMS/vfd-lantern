use std::{
    fs::{self, File},
    io::{BufRead as _, BufReader, Read as _, Write as _},
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

const ROWS: usize = 64;
const COLS: usize = 140;
const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, Deserialize)]
struct Handshake {
    pty: PathBuf,
}

#[derive(Debug, Deserialize)]
struct StructuredLogLine {
    record: String,
    function: Option<u8>,
    outcome: Option<String>,
}

#[derive(Debug)]
struct LogRecord {
    function: u8,
    outcome: String,
}

struct ChildGuard(Child);

impl ChildGuard {
    fn id(&self) -> u32 {
        self.0.id()
    }

    fn wait_timeout(&mut self) -> Result<std::process::ExitStatus> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.0.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                bail!("child {} did not exit within {TIMEOUT:?}", self.id());
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
    fn spawn(binary: &Path, args: &[String], environment: &Environment) -> Result<Self> {
        let size = Winsize {
            ws_row: ROWS as u16,
            ws_col: COLS as u16,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = openpty(Some(&size), None).context("open controlled product PTY")?;
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

    fn terminal_text(&self) -> String {
        Screen::from_terminal_stream(&lock_output(&self.output)).text()
    }

    fn wait_for(&self, needle: &str) -> Result<()> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let text = self.terminal_text();
            if text.contains(needle) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!(
                    "TUI did not render {needle:?}; reconstructed screen:\n{}",
                    tail(&text, 12_000)
                );
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn assert_not_emitted(&self, needle: &str) -> Result<()> {
        let text = self.terminal_text();
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
        let line = rx.recv_timeout(TIMEOUT).context("simulator handshake")??;
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

struct Environment {
    root: TempDir,
    home: PathBuf,
    config: PathBuf,
    data: PathBuf,
    state: PathBuf,
    cache: PathBuf,
}

impl Environment {
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
}

fn main() -> Result<()> {
    let debug = debug_directory()?;
    let simulator = debug.join("lantern-sim");
    let product = debug.join("vfd-lantern");
    ensure!(simulator.is_file(), "missing {}", simulator.display());
    ensure!(product.is_file(), "missing {}", product.display());

    run_case(&simulator, &product, false)?;
    println!("process-e2e backup-restore-rejected-confirmation-zero-write ok");
    run_case(&simulator, &product, true)?;
    println!("process-e2e backup-restore-device-rejection-no-retry ok");
    Ok(())
}

fn run_case(simulator_binary: &Path, product_binary: &Path, confirm: bool) -> Result<()> {
    let env = Environment::new()?;
    let selected = env.root.path().join("selected-vfd.toml");
    fs::write(&selected, fs::read_to_string(reference_profile())?)?;
    let profile = lantern_sim::load_profile(&selected)?;
    let hash = profile.profile_hash().to_hex();

    let approval = Command::new(product_binary)
        .args(["profile", "approve-write"])
        .arg(&selected)
        .args([
            "--expected-hash",
            &hash,
            "--manual-source",
            "PTY restore fixture",
        ])
        .args([
            "--summary",
            "Disposable simulator-only backup/restore acceptance",
        ])
        .env("HOME", &env.home)
        .env("XDG_CONFIG_HOME", &env.config)
        .env("XDG_DATA_HOME", &env.data)
        .env("XDG_STATE_HOME", &env.state)
        .env("XDG_CACHE_HOME", &env.cache)
        .status()?;
    ensure!(approval.success(), "isolated fixture profile approval");

    let scenario = env.root.path().join("backup-restore.toml");
    fs::write(&scenario, scenario_source(&selected, &profile))?;
    let simulator = Simulator::spawn(
        simulator_binary,
        &selected,
        &scenario,
        env.root.path().join("backup-restore.jsonl"),
    )?;

    let args = vec![
        "--profile".to_owned(),
        selected.to_string_lossy().into_owned(),
        "--device".to_owned(),
        simulator.pty.to_string_lossy().into_owned(),
        "--no-color".to_owned(),
        "--enable-writes".to_owned(),
    ];
    let mut product = TerminalChild::spawn(product_binary, &args, &env)?;
    drive_to_summary(&mut product)?;
    product.send("\r")?;
    product.wait_for("Verified read-only session established")?;

    // Arm writes through the real product TUI. Backup/restore must consume the
    // exact same guarded session and WriteCoordinator as a manual write.
    product.send("4")?;
    product.wait_for("WRITES DISARMED")?;
    product.send("/acceleration\r")?;
    product.wait_for("matches=1")?;
    product.send("R")?;
    product.wait_for("quality=Good")?;
    product.send("A")?;
    product.wait_for("Arming confirmation:")?;
    product.send(&format!("ARM {}\r", &hash[..12]))?;
    product.wait_for("WRITES ARMED")?;

    // Real Backup screen, not a layer-level reducer test.
    product.send("5")?;
    product.wait_for("b capture complete backup")?;
    product.assert_not_emitted("remain owned by #17")?;
    product.send("b")?;
    product.wait_for("captured; complete=true")?;

    let backup_directory = env.data.join("vfd-lantern/backups");
    let captured = only_backup_file(&backup_directory)?;
    let mut source = lantern_storage::read_backup(&captured)?;
    let parameter_id = lantern_domain::ParameterId::parse("config.acceleration")?;
    let parameter = profile
        .parameter(&parameter_id)
        .context("acceleration parameter in fixture profile")?;
    let target_raw = lantern_domain::RawRegisters::new(vec![100])?;
    let target_engineering = parameter.codec().decode(target_raw.as_slice())?;
    let value = source
        .values
        .get_mut(&parameter_id)
        .context("captured acceleration value")?;
    ensure!(
        value.raw != target_raw,
        "fixture target must differ from device"
    );
    value.raw = target_raw;
    value.engineering = target_engineering;
    let source_path = env
        .root
        .path()
        .join("restore-source.vfdlantern-backup.json");
    lantern_storage::write_backup(&source_path, &source)?;

    product.send("l")?;
    product.wait_for("Source backup path:")?;
    product.send(&format!("{}\r", source_path.display()))?;
    product.wait_for("loaded; complete=true")?;
    product.send("p")?;
    product.wait_for("restore plan prepared; steps=1")?;
    product.wait_for("Exact confirmation required:")?;
    let text = product.terminal_text();
    let confirmation = text
        .rsplit("Exact confirmation required:")
        .next()
        .and_then(|suffix| suffix.split_whitespace().next())
        .context("operator-visible exact restore challenge")?
        .to_owned();
    ensure!(confirmation.starts_with("restore:"));

    // PrepareRestore must have created a second, fresh pre-restore backup.
    ensure!(
        backup_files(&backup_directory)?.len() == 2,
        "restore preparation must persist one fresh pre-restore backup"
    );

    product.send("r")?;
    product.wait_for("Restore confirmation:")?;
    if !confirm {
        product.send("wrong\r")?;
        product.wait_for("operator confirmation does not exactly match restore plan")?;
        product.quit()?;
        let records = simulator.stop()?;
        ensure!(
            records
                .iter()
                .all(|record| record.function != 6 && record.function != 16),
            "wrong exact confirmation must execute zero physical writes"
        );
        return Ok(());
    }

    product.send(&format!("{confirmation}\r"))?;
    // The base process simulator is deliberately read-only. A valid restore
    // therefore reaches the physical PTY/RTU boundary once, receives
    // IllegalFunction, and must stop without retry. Successful device-side
    // write/apply semantics are already covered by the write-capable #27
    // conformance PTY runtime; this proves the actual product process wiring.
    product.wait_for("restore stopped at step 0")?;
    product.quit()?;
    let records = simulator.stop()?;
    ensure!(records.iter().filter(|record| record.function == 6).count() == 1);
    ensure!(
        records
            .iter()
            .filter(|record| record.function == 16)
            .count()
            == 0
    );
    let write = records
        .iter()
        .find(|record| record.function == 6)
        .context("single restore write request")?;
    ensure!(write.outcome == "exception:01");
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
    product.wait_for("Identification probes")?;
    Ok(())
}

fn scenario_source(
    profile_path: &Path,
    profile: &lantern_profile::ValidatedDeviceProfile,
) -> String {
    format!(
        "schema_version = 1\nprofile_path = {:?}\nprofile_hash = {:?}\nslave_id = 1\nfingerprint = \"process.m65.restore\"\nseed = \"{SEED}\"\ntick_micros = 1000\n\n[initial_values]\n\"config.acceleration\" = \"9\"\n",
        profile_path.to_string_lossy(),
        profile.profile_hash().to_hex(),
    )
}

fn backup_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut files = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(lantern_storage::BACKUP_SUFFIX))
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn only_backup_file(directory: &Path) -> Result<PathBuf> {
    let files = backup_files(directory)?;
    ensure!(
        files.len() == 1,
        "expected one captured backup, found {files:?}"
    );
    Ok(files[0].clone())
}

fn reference_profile() -> PathBuf {
    std::env::var_os("VFD_LANTERN_TEST_PROFILE").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/example-vfd.toml"),
        PathBuf::from,
    )
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
        let parsed: StructuredLogLine = serde_json::from_str(line)?;
        if parsed.record == "request" {
            requests.push(LogRecord {
                function: parsed.function.context("request function")?,
                outcome: parsed.outcome.context("request outcome")?,
            });
        }
    }
    Ok(requests)
}

fn utf8_sequence_length(first: u8) -> usize {
    match first {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
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
