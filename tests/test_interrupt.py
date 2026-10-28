"""Terminal integration tests. Run after cargo build:

    python3 -m unittest discover -s tests -p 'test_*.py'
"""

import errno
import json
import os
from pathlib import Path
import pty
import select
import shlex
import signal
import tempfile
import time
import unittest


BINARY = Path(__file__).resolve().parents[1] / "target/debug/demonator"


class InterruptTests(unittest.TestCase):
    def check_command(self, options="", interrupt=True, program="sleep 30"):
        with tempfile.TemporaryDirectory() as directory:
            ready = Path(directory) / "ready"
            continued = Path(directory) / "continued"
            # The marker confirms the actual child is running even with capture
            # or hidden output. The shell and its child share the foreground
            # process group, just as they do in a live demo.
            command = f"echo ready > {shlex.quote(str(ready))}"
            if interrupt:
                command += "; " + program
            config = Path(directory) / "demo.yml"
            config.write_text(
                "speed: 1000\nauto_advance: 0\nsteps:\n"
                f"  - text: {json.dumps(command)}\n"
                + options
                + f"  - text: {json.dumps('touch ' + shlex.quote(str(continued)))}\n"
                "    hidden: true\n"
                '  - text: "echo should-not-run"\n'
                "    wait_before: true\n"
            )
            pid, terminal = pty.fork()
            if pid == 0:
                os.execv(str(BINARY), [str(BINARY), "-c", str(config)])

            output = bytearray()
            reaped = False

            def drain():
                if select.select([terminal], [], [], 0.02)[0]:
                    try:
                        output.extend(os.read(terminal, 65536))
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise

            def wait_for_file(path):
                deadline = time.monotonic() + 8
                while not path.exists() and time.monotonic() < deadline:
                    drain()
                self.assertTrue(path.exists(), output.decode(errors="replace"))

            try:
                wait_for_file(ready)
                if interrupt:
                    time.sleep(0.1)
                    os.write(terminal, b"\x03")
                wait_for_file(continued)
                # The next step is waiting for Enter; Ctrl+C must now exit.
                time.sleep(0.1)
                os.write(terminal, b"\x03")
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    drain()
                    child, status = os.waitpid(pid, os.WNOHANG)
                    if child:
                        reaped = True
                        self.assertTrue(os.WIFSIGNALED(status))
                        self.assertEqual(os.WTERMSIG(status), signal.SIGINT)
                        break
                self.assertTrue(reaped, "Demonator did not exit between commands")
            finally:
                if not reaped:
                    os.killpg(pid, signal.SIGKILL)
                    os.waitpid(pid, 0)
                os.close(terminal)

    def test_normal_command(self):
        self.check_command()

    def test_pipeline(self):
        self.check_command(program="sleep 30 | cat")

    def test_capture(self):
        self.check_command('    capture:\n      name: output\n      pattern: "(.*)"\n')

    def test_wait_for(self):
        self.check_command('    wait_for: "never-matches"\n')

    def test_interact(self):
        self.check_command('    interact:\n      - expect: "never-matches"\n        send: "yes"\n')

    def test_hidden_command(self):
        self.check_command("    hidden: true\n")

    def test_normal_completion_restores_interrupt(self):
        self.check_command(interrupt=False)


if __name__ == "__main__":
    unittest.main()
