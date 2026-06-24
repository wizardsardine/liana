import os
import select
import shutil
import signal
import subprocess
import time
from pathlib import Path

from ephemeral_port_reserve import reserve


class X11SessionError(RuntimeError):
    pass


class X11Session:
    """Own a small X11 desktop suitable for native GUI automation."""

    def __init__(self, directory, width=1280, height=960, depth=24):
        self.directory = Path(directory)
        self.artifacts_dir = self.directory / "gui-artifacts"
        self.runtime_dir = self.directory / "xdg-runtime"
        self.width = width
        self.height = height
        self.depth = depth
        self.display = None
        self.processes = []
        self.env = os.environ.copy()

    def start(self):
        self.artifacts_dir.mkdir(parents=True, exist_ok=True)
        self.runtime_dir.mkdir(parents=True, exist_ok=True)
        self.runtime_dir.chmod(0o700)

        self._start_xvfb()
        self.env.update(
            {
                "DISPLAY": self.display,
                "XDG_RUNTIME_DIR": str(self.runtime_dir),
                "WINIT_UNIX_BACKEND": "x11",
                "WINIT_X11_SCALE_FACTOR": "1",
                "NO_AT_BRIDGE": "1",
                "GTK_USE_PORTAL": os.getenv("GTK_USE_PORTAL", "0"),
                "LC_ALL": "C",
                "TZ": "UTC",
            }
        )
        self.env.pop("WAYLAND_DISPLAY", None)

        self._wait_for_x()
        self._start_process("openbox", ["openbox"])
        self._wait_for_window_manager()

        if os.getenv("GUI_TEST_VNC") == "1":
            port = str(reserve())
            self.env["GUI_TEST_VNC_PORT"] = port
            self._start_process(
                "x11vnc",
                [
                    "x11vnc",
                    "-display",
                    self.display,
                    "-forever",
                    "-shared",
                    "-nopw",
                    "-rfbport",
                    port,
                ],
            )

        return self

    def close(self):
        for _, proc, _ in reversed(self.processes):
            if proc.poll() is None:
                proc.terminate()

        deadline = time.time() + 5
        for _, proc, _ in reversed(self.processes):
            if proc.poll() is not None:
                continue
            timeout = max(0.1, deadline - time.time())
            try:
                proc.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=5)

    def run(self, args, check=True, capture_output=True, timeout=10, **kwargs):
        stdout = subprocess.PIPE if capture_output else None
        stderr = subprocess.STDOUT if capture_output else None
        return subprocess.run(
            args,
            check=check,
            env=self.env,
            text=True,
            stdout=stdout,
            stderr=stderr,
            timeout=timeout,
            **kwargs,
        )

    def _start_xvfb(self):
        if shutil.which("Xvfb") is None:
            raise X11SessionError("Xvfb is not available in PATH")

        read_fd, write_fd = os.pipe()
        log_path = self.artifacts_dir / "Xvfb.log"
        log = log_path.open("wb")
        proc = subprocess.Popen(
            [
                "Xvfb",
                "-displayfd",
                str(write_fd),
                "-screen",
                "0",
                f"{self.width}x{self.height}x{self.depth}",
                "-nolisten",
                "tcp",
            ],
            stdout=log,
            stderr=subprocess.STDOUT,
            pass_fds=(write_fd,),
            close_fds=True,
        )
        os.close(write_fd)

        ready, _, _ = select.select([read_fd], [], [], 10)
        if not ready:
            proc.terminate()
            raise X11SessionError("Timed out waiting for Xvfb display allocation")

        display_num = os.read(read_fd, 32).decode().strip()
        os.close(read_fd)
        if proc.poll() is not None or not display_num:
            raise X11SessionError(f"Xvfb exited before reporting a display; see {log_path}")

        self.display = f":{display_num}"
        self.processes.append(("Xvfb", proc, log))

    def _start_process(self, name, args):
        log = (self.artifacts_dir / f"{name}.log").open("wb")
        proc = subprocess.Popen(
            args,
            stdout=log,
            stderr=subprocess.STDOUT,
            env=self.env,
            start_new_session=True,
        )
        self.processes.append((name, proc, log))
        return proc

    def _wait_for_x(self):
        deadline = time.time() + 10
        while time.time() < deadline:
            try:
                self.run(["xdpyinfo"], timeout=2)
                return
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                time.sleep(0.1)
        raise X11SessionError("Timed out waiting for X11 to accept clients")

    def _wait_for_window_manager(self):
        deadline = time.time() + 10
        while time.time() < deadline:
            try:
                self.run(["xdotool", "getdisplaygeometry"], timeout=2)
                return
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                time.sleep(0.1)
        raise X11SessionError("Timed out waiting for xdotool to access display")


def terminate_process(proc, timeout=10):
    if proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            proc.kill()
        proc.wait(timeout=5)

