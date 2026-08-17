use std::{
    fs,
    os::fd::{AsFd, AsRawFd, OwnedFd},
    path::{Path, PathBuf},
};

use nix::{
    sys::termios::{self, InputFlags, LocalFlags, OutputFlags, SetArg},
    unistd::{read, write},
};
use tokio_serial::SerialStream;

use crate::SimulatorError;

/// A Tokio serial endpoint for the server and the real slave PTY path opened by
/// the production serial transport.
#[derive(Debug)]
pub struct SimulatorPty {
    server: SerialStream,
    client_guard: SerialStream,
    client_path: PathBuf,
}

impl SimulatorPty {
    /// Creates a connected raw PTY pair suitable for a Modbus RTU server.
    pub fn direct() -> Result<Self, SimulatorError> {
        let (server, client_guard, client_path) = connected_serial_pair()?;
        Ok(Self {
            server,
            client_guard,
            client_path,
        })
    }

    /// Consumes the topology while keeping the slave node open for a later
    /// production `open_serial_bus` call.
    pub(crate) fn into_parts(self) -> (SerialStream, SerialStream, PathBuf) {
        (self.server, self.client_guard, self.client_path)
    }

    /// Returns the slave PTY path that must be opened by `open_serial_bus`.
    #[must_use]
    pub fn client_path(&self) -> &Path {
        &self.client_path
    }
}

pub(crate) fn connected_serial_pair()
-> Result<(SerialStream, SerialStream, PathBuf), SimulatorError> {
    let (first, second) =
        SerialStream::pair().map_err(|error| SimulatorError::Serial(error.to_string()))?;
    let second_path = path_for_open_fd(second.as_raw_fd())?;
    Ok((first, second, second_path))
}

fn path_for_open_fd(raw_fd: i32) -> Result<PathBuf, SimulatorError> {
    let link = PathBuf::from(format!("/proc/self/fd/{raw_fd}"));
    let path = fs::read_link(&link).map_err(|error| SimulatorError::ReadFile {
        path: link,
        source: error,
    })?;
    if !path.starts_with("/dev/pts") {
        return Err(SimulatorError::Pty(format!(
            "serial pair did not expose a /dev/pts slave: {}",
            path.display()
        )));
    }
    Ok(path)
}

/// Safe `nix::pty::openpty` wrapper used to verify Linux line discipline and
/// byte transparency independently of the serial crate.
#[derive(Debug)]
pub struct RawPtyPair {
    master: OwnedFd,
    slave: OwnedFd,
}

impl RawPtyPair {
    /// Opens a PTY and applies a strict raw, 8-bit-clean line discipline.
    pub fn open() -> Result<Self, SimulatorError> {
        let pair = nix::pty::openpty(None, None)
            .map_err(|error| SimulatorError::Pty(error.to_string()))?;
        let mut attributes = termios::tcgetattr(pair.slave.as_fd())
            .map_err(|error| SimulatorError::Pty(error.to_string()))?;
        termios::cfmakeraw(&mut attributes);
        termios::tcsetattr(pair.slave.as_fd(), SetArg::TCSANOW, &attributes)
            .map_err(|error| SimulatorError::Pty(error.to_string()))?;
        Ok(Self {
            master: pair.master,
            slave: pair.slave,
        })
    }

    /// Returns whether canonical input, echo, translations, and software flow
    /// control are all disabled.
    pub fn is_raw(&self) -> Result<bool, SimulatorError> {
        let attributes = termios::tcgetattr(self.slave.as_fd())
            .map_err(|error| SimulatorError::Pty(error.to_string()))?;
        let forbidden_local = LocalFlags::ICANON | LocalFlags::ECHO | LocalFlags::ECHONL;
        let forbidden_input =
            InputFlags::ICRNL | InputFlags::INLCR | InputFlags::IGNCR | InputFlags::IXON;
        Ok(!attributes.local_flags.intersects(forbidden_local)
            && !attributes.input_flags.intersects(forbidden_input)
            && !attributes.output_flags.contains(OutputFlags::OPOST))
    }

    /// Writes bytes at the master and reads the exact sequence at the slave.
    pub fn master_to_slave(&self, bytes: &[u8]) -> Result<Vec<u8>, SimulatorError> {
        write_all(&self.master, bytes)?;
        read_exact(&self.slave, bytes.len())
    }

    /// Writes bytes at the slave and reads the exact sequence at the master.
    pub fn slave_to_master(&self, bytes: &[u8]) -> Result<Vec<u8>, SimulatorError> {
        write_all(&self.slave, bytes)?;
        read_exact(&self.master, bytes.len())
    }
}

fn write_all(fd: &OwnedFd, mut bytes: &[u8]) -> Result<(), SimulatorError> {
    while !bytes.is_empty() {
        let written = write(fd, bytes).map_err(|error| SimulatorError::Pty(error.to_string()))?;
        if written == 0 {
            return Err(SimulatorError::Pty("zero-length PTY write".to_owned()));
        }
        bytes = &bytes[written..];
    }
    Ok(())
}

fn read_exact(fd: &OwnedFd, length: usize) -> Result<Vec<u8>, SimulatorError> {
    let mut bytes = vec![0_u8; length];
    let mut filled = 0;
    while filled < length {
        let read_count = read(fd, &mut bytes[filled..])
            .map_err(|error| SimulatorError::Pty(error.to_string()))?;
        if read_count == 0 {
            return Err(SimulatorError::Pty("unexpected PTY EOF".to_owned()));
        }
        filled += read_count;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::RawPtyPair;

    #[test]
    fn raw_pty_is_byte_transparent_in_both_directions() {
        let pair = RawPtyPair::open().expect("raw PTY");
        assert!(pair.is_raw().expect("line discipline"));
        let all_bytes = (0_u8..=u8::MAX).collect::<Vec<_>>();
        assert_eq!(
            pair.master_to_slave(&all_bytes).expect("master to slave"),
            all_bytes
        );
        assert_eq!(
            pair.slave_to_master(&all_bytes).expect("slave to master"),
            all_bytes
        );
    }
}
