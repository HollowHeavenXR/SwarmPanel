"""Run every test, each in its own process so their isolation can't bleed."""

import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
TESTS = ["test_api.py", "test_live.py", "test_session_gating.py",
         "test_endpoints.py", "test_ui_smoke.py"]

failed = []
for name in TESTS:
    print(f"\n=== {name} " + "=" * (60 - len(name)))
    command = [sys.executable, "-u", str(HERE / name)]
    if name == "test_ui_smoke.py":
        command = ["xvfb-run", "-a", *command]
    result = subprocess.run(command)
    if result.returncode != 0:
        failed.append(name)

print("\n" + "=" * 66)
if failed:
    print("FAILED: " + ", ".join(failed))
    sys.exit(1)
print(f"ALL {len(TESTS)} TEST SUITES PASSED")
