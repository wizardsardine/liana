import csv
import re
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path

from .x11 import terminate_process


class GuiDriverError(RuntimeError):
    pass


@dataclass
class TextBox:
    text: str
    left: int
    top: int
    width: int
    height: int
    confidence: float

    @property
    def center(self):
        return (self.left + self.width // 2, self.top + self.height // 2)


class GuiApp:
    def __init__(self, session, datadir, network="regtest", width=1280, height=960):
        self.session = session
        self.datadir = Path(datadir)
        self.network = network
        self.width = width
        self.height = height
        self.window_id = None
        self.proc = None
        self.log = None
        self.screenshot_count = 0
        self.launcher = Path(__file__).parent / "bin" / "launch-liana-gui"

    def start(self, *extra_args):
        if self.proc is not None:
            raise GuiDriverError("GUI process already started")

        args = [
            str(self.launcher),
            "--datadir",
            str(self.datadir),
            f"--{self.network}",
            *extra_args,
        ]
        log_path = self.session.artifacts_dir / "liana-gui.log"
        self.log = log_path.open("wb")
        self.proc = subprocess.Popen(
            args,
            env=self.session.env,
            stdout=self.log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        self.wait_for_window()
        self.resize(self.width, self.height)
        self.activate()
        return self

    def stop(self):
        if self.proc is not None:
            terminate_process(self.proc)
            self.proc = None
        if self.log is not None:
            self.log.close()
            self.log = None

    def wait_for_window(self, title_pattern="Liana", timeout=30):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self.proc and self.proc.poll() is not None:
                raise GuiDriverError("liana-gui exited before creating a window")
            res = self.session.run(
                ["xdotool", "search", "--name", title_pattern],
                check=False,
                timeout=2,
            )
            ids = [line.strip() for line in res.stdout.splitlines() if line.strip()]
            if ids:
                self.window_id = ids[-1]
                return self.window_id
            time.sleep(0.25)
        raise GuiDriverError(f"Timed out waiting for GUI window matching {title_pattern!r}")

    def activate(self):
        self._require_window()
        for command in ("windowactivate", "windowfocus"):
            for _ in range(3):
                res = self.session.run(
                    ["xdotool", command, self.window_id],
                    check=False,
                    timeout=5,
                )
                if res.returncode == 0:
                    time.sleep(0.1)
                    return
                time.sleep(0.25)
        time.sleep(0.1)

    def resize(self, width, height):
        self._require_window()
        self.session.run(["xdotool", "windowsize", self.window_id, str(width), str(height)])
        time.sleep(0.25)

    def screenshot(self, label="screen"):
        self.screenshot_count += 1
        safe = re.sub(r"[^A-Za-z0-9_.-]+", "-", label).strip("-") or "screen"
        path = self.session.artifacts_dir / f"{self.screenshot_count:04d}-{safe}.png"
        try:
            self.session.run(["import", "-window", "root", str(path)], timeout=10)
        except (FileNotFoundError, subprocess.CalledProcessError):
            self.session.run(["magick", "import", "-window", "root", str(path)], timeout=10)
        return path

    def save_debug_artifacts(self, label="failure"):
        self.screenshot(label)
        if self.window_id:
            res = self.session.run(
                ["xwininfo", "-id", self.window_id],
                check=False,
                timeout=5,
            )
            (self.session.artifacts_dir / f"{label}-xwininfo.txt").write_text(res.stdout)

    def click_text(self, needle, timeout=15, button=1):
        box = self.wait_for_text(needle, timeout=timeout)
        self.click_at(*box.center, button=button)
        return box

    def wait_for_text(self, needle, timeout=15):
        deadline = time.time() + timeout
        last_text = ""
        while time.time() < deadline:
            image = self.screenshot(f"ocr-{needle}")
            boxes = self.ocr(image)
            last_text = "\n".join(box.text for box in boxes)
            box = self._find_text_box(boxes, needle)
            if box:
                return box
            time.sleep(0.5)
        raise GuiDriverError(f"Timed out waiting for text {needle!r}. Last OCR text:\n{last_text}")

    def assert_text(self, needle, timeout=10):
        self.wait_for_text(needle, timeout=timeout)

    def click_at(self, x, y, button=1):
        self.session.run(
            [
                "xdotool",
                "mousemove",
                str(int(x)),
                str(int(y)),
                "click",
                str(button),
            ],
            timeout=5,
        )
        time.sleep(0.2)

    def type_text(self, value, delay=1):
        self.session.run(
            ["xdotool", "type", "--clearmodifiers", "--delay", str(delay), str(value)],
            timeout=max(10, len(str(value)) // 10),
        )

    def key(self, *keys):
        self.session.run(["xdotool", "key", "--clearmodifiers", *keys], timeout=5)
        time.sleep(0.1)

    def ocr(self, image_path):
        res = self.session.run(
            ["tesseract", str(image_path), "stdout", "--psm", "11", "tsv"],
            check=False,
            timeout=20,
        )
        if res.returncode not in (0, 1):
            raise GuiDriverError(f"OCR failed: {res.stdout}")
        return _parse_tesseract_tsv(res.stdout)

    def _find_text_box(self, boxes, needle):
        normalized_needle = _normalize(needle)
        if not normalized_needle:
            return None
        for box in boxes:
            if normalized_needle == _normalize(box.text):
                return box
        for box in boxes:
            if normalized_needle in _normalize(box.text):
                return box
        return None

    def _require_window(self):
        if not self.window_id:
            raise GuiDriverError("GUI window was not discovered yet")


def _parse_tesseract_tsv(output):
    reader = csv.DictReader(output.splitlines(), delimiter="\t")
    words_by_line = {}
    for row in reader:
        text = (row.get("text") or "").strip()
        if not text:
            continue
        try:
            confidence = float(row.get("conf", "-1"))
        except ValueError:
            confidence = -1
        if confidence < 0:
            continue
        key = (
            row.get("page_num"),
            row.get("block_num"),
            row.get("par_num"),
            row.get("line_num"),
        )
        words_by_line.setdefault(key, []).append(
            (
                text,
                int(row["left"]),
                int(row["top"]),
                int(row["width"]),
                int(row["height"]),
                confidence,
            )
        )

    boxes = []
    for words in words_by_line.values():
        left = min(w[1] for w in words)
        top = min(w[2] for w in words)
        right = max(w[1] + w[3] for w in words)
        bottom = max(w[2] + w[4] for w in words)
        text = " ".join(w[0] for w in words)
        confidence = sum(w[5] for w in words) / len(words)
        boxes.append(TextBox(text, left, top, right - left, bottom - top, confidence))
    return boxes


def _normalize(value):
    return re.sub(r"\s+", " ", value).strip().lower()
