#!/usr/bin/env python3
"""Literal #13 process acceptance: real vfd-lantern + real lantern-sim + PTY."""

from __future__ import annotations

import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import queue
import select
import shutil
import signal
import struct
import subprocess
import tempfile
import termios
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
PRODUCT = ROOT / "target" / "debug" / "vfd-lantern"
SIMULATOR = ROOT / "target" / "debug" / "lantern-sim"
REFERENCE_PROFILE = ROOT / "profiles" / "example-vfd.toml"
TIMEOUT = 8.0
SETTLE = 0.30
CURSOR_QUERY = b"\x1b[6n"
CURSOR_RESPONSE = b"\x1b[1;1R"
SEED = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"

CASES = (
    "match-process-off",
    "match-disarmed",
    "partial",
    "mismatch-export",
    "ambiguous",
    "timeout",
    "protocol-exception",
)


def semantic_text(data: bytes | str) -> str:
    """Remove ANSI/control/whitespace so sparse terminal redraws compare semantically."""
    if isinstance(data, str):
        data = data.encode()
    out = bytearray()
    index = 0
    while index < len(data):
        byte = data[index]
        if byte == 0x1B:
            index += 1
            if index < len(data) and data[index] == ord("["):
                index += 1
                while index < len(data):
                    final_byte = data[index]
                    index += 1
                    if 0x40 <= final_byte <= 0x7E:
                        break
            elif index < len(data):
                index += 1
            continue
        if 0x21 <= byte <= 0x7E:
            out.append(byte)
        index += 1
    return out.decode("ascii", errors="ignore")


def tail(text: str, limit: int = 6000) -> str:
    return text[-limit:]


class ControlledTerminal:
    def __init__(self, command: list[str], env: dict[str, str]) -> None:
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 42, 140, 0, 0))
        self.master = master
        self.capture = bytearray()
        self.lock = threading.Lock()
        self.stop_reader = threading.Event()
        self.process = subprocess.Popen(
            command,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=env,
            close_fds=True,
        )
        os.close(slave)
        self.reader = threading.Thread(target=self._read_loop, daemon=True)
        self.reader.start()

    def _read_loop(self) -> None:
        tail_bytes = bytearray()
        while not self.stop_reader.is_set():
            try:
                readable, _, _ = select.select([self.master], [], [], 0.1)
                if not readable:
                    continue
                chunk = os.read(self.master, 4096)
                if not chunk:
                    return
            except OSError as error:
                if error.errno in (errno.EIO, errno.EBADF):
                    return
                raise
            with self.lock:
                self.capture.extend(chunk)
            tail_bytes.extend(chunk)
            queries = sum(
                1
                for index in range(max(0, len(tail_bytes) - len(chunk) - 3), len(tail_bytes))
                if tail_bytes[index : index + len(CURSOR_QUERY)] == CURSOR_QUERY
            )
            for _ in range(queries):
                try:
                    os.write(self.master, CURSOR_RESPONSE)
                except OSError:
                    return
            if len(tail_bytes) > 16:
                del tail_bytes[:-16]

    def send(self, text: str) -> None:
        os.write(self.master, text.encode())

    def screen_history(self) -> str:
        with self.lock:
            captured = bytes(self.capture)
        return semantic_text(captured)

    def wait_for(self, text: str) -> None:
        needle = semantic_text(text)
        deadline = time.monotonic() + TIMEOUT
        while time.monotonic() < deadline:
            history = self.screen_history()
            if needle in history:
                return
            if self.process.poll() is not None:
                raise RuntimeError(
                    f"vfd-lantern exited {self.process.returncode} before {text!r}; "
                    f"tail={tail(history)!r}"
                )
            time.sleep(0.025)
        raise RuntimeError(f"TUI did not render {text!r}; tail={tail(self.screen_history())!r}")

    def assert_not_contains(self, text: str) -> None:
        needle = semantic_text(text)
        history = self.screen_history()
        if needle in history:
            raise AssertionError(f"TUI unexpectedly rendered {text!r}; tail={tail(history)!r}")

    def quit(self) -> None:
        self.send("q")
        try:
            status = self.process.wait(timeout=TIMEOUT)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
            raise AssertionError("vfd-lantern did not stop after q")
        finally:
            self.stop_reader.set()
            try:
                os.close(self.master)
            except OSError:
                pass
            self.reader.join(timeout=1.0)
        if status != 0:
            raise AssertionError(f"vfd-lantern exited with status {status}")

    def kill(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()
        self.stop_reader.set()
        try:
            os.close(self.master)
        except OSError:
            pass
        self.reader.join(timeout=1.0)


class Simulator:
    def __init__(self, profile: Path, scenario: Path, log_path: Path) -> None:
        self.log_path = log_path
        self.process = subprocess.Popen(
            [
                str(SIMULATOR),
                "--profile",
                str(profile),
                "--scenario",
                str(scenario),
                "--log",
                str(log_path),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        assert self.process.stdout is not None
        readable, _, _ = select.select([self.process.stdout], [], [], TIMEOUT)
        if not readable:
            self.kill()
            raise RuntimeError("lantern-sim did not emit its PTY handshake")
        line = self.process.stdout.readline()
        try:
            self.handshake = json.loads(line)
        except json.JSONDecodeError as error:
            self.kill()
            raise RuntimeError(f"invalid lantern-sim handshake {line!r}") from error

    @property
    def pty_path(self) -> Path:
        return Path(self.handshake["pty"])

    def live_records(self) -> list[dict]:
        if not self.log_path.exists():
            return []
        return [json.loads(line) for line in self.log_path.read_text().splitlines() if line.strip()]

    def stop(self) -> list[dict]:
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGINT)
        try:
            status = self.process.wait(timeout=TIMEOUT)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
            raise AssertionError("lantern-sim did not stop after SIGINT")
        if status != 0:
            stderr = self.process.stderr.read() if self.process.stderr else ""
            raise AssertionError(f"lantern-sim exited {status}: {stderr}")
        return self.live_records()

    def kill(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()


class CaseEnvironment:
    def __init__(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="vfd-lantern-issue13-")
        self.root = Path(self.temp.name)
        self.home = self.root / "home"
        self.config = self.root / "config"
        self.data = self.root / "data"
        self.state = self.root / "state"
        self.cache = self.root / "cache"
        for directory in (self.home, self.config, self.data, self.state, self.cache):
            directory.mkdir(parents=True, exist_ok=True)

    def env(self) -> dict[str, str]:
        env = os.environ.copy()
        env.update(
            HOME=str(self.home),
            XDG_CONFIG_HOME=str(self.config),
            XDG_DATA_HOME=str(self.data),
            XDG_STATE_HOME=str(self.state),
            XDG_CACHE_HOME=str(self.cache),
            TERM="xterm-256color",
        )
        return env

    def add_ambiguous_profile(self, source: str) -> None:
        # Linux ProjectDirs uses the application component; extra candidates make the
        # test robust if the implementation later includes the organization component.
        for relative in (
            "vfd-lantern/profiles",
            "aiteracja/vfd-lantern/profiles",
            "pl/aiteracja/vfd-lantern/profiles",
        ):
            directory = self.config / relative
            directory.mkdir(parents=True, exist_ok=True)
            (directory / "ambiguous-vfd.toml").write_text(source)

    def exported_reports(self) -> list[Path]:
        reports: list[Path] = []
        for root in (self.state, self.data):
            if root.exists():
                reports.extend(root.rglob("identification-*.json"))
        return reports

    def close(self) -> None:
        self.temp.cleanup()


def profile_hash(profile: Path, env: dict[str, str]) -> str:
    completed = subprocess.run(
        [str(PRODUCT), "profile", "hashes", str(profile)],
        env=env,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    for line in completed.stdout.splitlines():
        if line.startswith("profile_hash="):
            return line.split("=", 1)[1].strip()
    raise RuntimeError(f"profile hash missing from {completed.stdout!r}")


def partial_profile(base: str) -> str:
    marker = "[[parameters]]"
    if marker not in base:
        raise AssertionError("reference profile has no parameter marker")
    insertion = '''[[identification.probes]]
id = "aux"
description = "Second process-level identity word"
table = "holding_registers"
count = 1
expected_raw = [[1234]]
address = { notation = "pdu_zero_based", value = 100 }

[[parameters]]'''
    return base.replace(marker, insertion, 1)


def scenario_source(profile: Path, digest: str, case: str) -> str:
    extra = {
        "partial": "[probe_overrides]\naux = [9999]\n",
        "mismatch-export": "[probe_overrides]\nmodel = [4097]\n",
        "timeout": '[[read_behaviors]]\nstart_request = 1\nkind = "timeout"\n',
        "protocol-exception": (
            '[[read_behaviors]]\nstart_request = 1\nkind = "exception"\ncode = 2\n'
        ),
    }.get(case, "")
    return (
        "schema_version = 1\n"
        f"profile_path = {json.dumps(str(profile))}\n"
        f"profile_hash = {json.dumps(digest)}\n"
        "slave_id = 1\n"
        'fingerprint = "process.issue13"\n'
        f'seed = "{SEED}"\n'
        "tick_micros = 1000\n\n"
        f"{extra}"
    )


def drive_to_summary(product: ControlledTerminal) -> None:
    product.wait_for("Verified read-only connection wizard step Port")
    product.send("m")
    product.wait_for("Manual device path:")
    product.send("\r")

    product.wait_for("Verified read-only connection wizard step Profile")
    product.send("/")
    product.wait_for("Profile search:")
    product.send("example.vfd1000")
    product.send("\r")
    product.wait_for("Profile filter:")
    product.send("\r")

    product.wait_for("Verified read-only connection wizard step Link")
    product.send("\r")
    product.wait_for("Verified read-only connection wizard step Summary")
    product.wait_for("[Manual]")
    product.wait_for("stable=-")
    product.wait_for("vid:pid=-")
    product.wait_for("serial=-")
    product.wait_for("schema=v1")
    product.wait_for("profile_hash=")
    product.wait_for("source_hash=")
    product.wait_for("Identification probes")


def assert_read_only_records(case: str, records: list[dict]) -> None:
    minimum, maximum = ((2, 2) if case == "partial" else (1, 2) if case == "timeout" else (1, 1))
    if not minimum <= len(records) <= maximum:
        raise AssertionError(
            f"{case}: {len(records)} Modbus requests, expected bounded {minimum}..={maximum}"
        )
    functions = [record["function"] for record in records]
    if any(function not in (3, 4) for function in functions):
        raise AssertionError(f"{case}: non-read Modbus function(s) {functions}")


def run_case(case: str) -> None:
    environment = CaseEnvironment()
    simulator: Simulator | None = None
    product: ControlledTerminal | None = None
    try:
        base = REFERENCE_PROFILE.read_text()
        selected_source = partial_profile(base) if case == "partial" else base
        selected = environment.root / "selected-vfd.toml"
        selected.write_text(selected_source)
        digest = profile_hash(selected, environment.env())

        if case == "ambiguous":
            environment.add_ambiguous_profile(
                base.replace(
                    'profile_id = "example.vfd1000"',
                    'profile_id = "example.vfd2000"',
                    1,
                )
            )

        scenario = environment.root / f"{case}.toml"
        scenario.write_text(scenario_source(selected, digest, case))
        simulator = Simulator(selected, scenario, environment.root / "simulator.jsonl")

        arguments = [
            str(PRODUCT),
            "--profile",
            str(selected),
            "--device",
            str(simulator.pty_path),
            "--no-color",
        ]
        if case == "match-disarmed":
            arguments.append("--enable-writes")
        product = ControlledTerminal(arguments, environment.env())
        drive_to_summary(product)

        # The issue's critical invariant: selection/profile/link/summary is passive.
        time.sleep(0.10)
        if simulator.live_records():
            raise AssertionError(f"{case}: Modbus traffic occurred before explicit Connect")
        product.send("\r")

        if case == "match-process-off":
            product.wait_for("Verified read-only session established")
            product.wait_for("PROCESS-OFF")
            product.assert_not_contains("authorization=ARMED")
        elif case == "match-disarmed":
            product.wait_for("Verified read-only session established")
            product.wait_for("DISARMED")
            product.assert_not_contains("authorization=ARMED")
        elif case == "partial":
            product.wait_for("Outcome=Partial")
            product.wait_for("Verified session: NOT CREATED")
        elif case == "mismatch-export":
            product.wait_for("Outcome=Mismatch")
            product.wait_for("Verified session: NOT CREATED")
            product.send("e")
            product.wait_for("Last export:")
        elif case == "ambiguous":
            product.wait_for("Outcome=Ambiguous")
            product.wait_for("Verified session: NOT CREATED")
        elif case == "timeout":
            product.wait_for("Outcome=Error")
            product.wait_for("quality=Timeout")
            product.wait_for("Verified session: NOT CREATED")
        elif case == "protocol-exception":
            product.wait_for("Outcome=Error")
            product.wait_for("quality=ProtocolException")
            product.wait_for("Verified session: NOT CREATED")

        time.sleep(SETTLE)
        product.quit()
        product = None
        records = simulator.stop()
        simulator = None
        assert_read_only_records(case, records)

        if case == "mismatch-export":
            reports = environment.exported_reports()
            if len(reports) != 1:
                raise AssertionError(f"expected one offline report, found {reports}")
            report = json.loads(reports[0].read_text())
            if report.get("outcome") != "mismatch":
                raise AssertionError(f"unexpected exported report {report}")
    finally:
        if product is not None:
            product.kill()
        if simulator is not None:
            simulator.kill()
        environment.close()


def main() -> None:
    if not PRODUCT.is_file() or not SIMULATOR.is_file():
        raise SystemExit("build vfd-lantern and lantern-sim before process E2E")
    for case in CASES:
        run_case(case)
        print(f"process-e2e {case} ok", flush=True)


if __name__ == "__main__":
    main()
