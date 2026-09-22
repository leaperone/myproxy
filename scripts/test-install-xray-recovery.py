#!/usr/bin/env python3
"""Mocked safety checks for install-xray-channel.py recovery decisions."""
import ast, json, plistlib, shutil, subprocess, tempfile
from pathlib import Path
from unittest import mock

source = Path(__file__).with_name("install-xray-channel.py").read_text()
tree = ast.parse(source)
functions = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name in {"recover_replaced_application", "verify_proxy_clients"}]
namespace = {"subprocess": subprocess, "plistlib": plistlib, "shutil": shutil, "Path": Path, "time": __import__("time"), "json": json}
exec(compile(ast.Module(body=functions, type_ignores=[]), "install-xray-channel.py", "exec"), namespace)
recover = namespace["recover_replaced_application"]

with tempfile.TemporaryDirectory(prefix="myproxy-upgrade-recovery-") as raw:
 root = Path(raw); backup = root / "backup"; backup.mkdir(); previous = backup / "MyProxy.app"; candidate = root / "MyProxy.app"; previous.mkdir(); candidate.mkdir()
 for bundle in (previous, candidate):
  contents = bundle / "Contents"; (contents / "MacOS").mkdir(parents=True)
  (contents / "MacOS" / "myproxy").write_text("fixture")
  (contents / "Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "local.harry.myproxy"}))
 namespace["wait_for_exact_process_exit"] = lambda executable: True
 cli_calls = []
 def fake_cli(cli_path, *args):
  cli_calls.append(args)
  if args == ("status",) and cli_calls.count(("status",)) == 1:
   return {"xray": {"running": False, "wanted": False}, "extension_runtime": {"observed": True, "captureEnabled": False, "phase": "disabled", "dnsPhase": "disabled"}}
  return {"xray": {"running": True, "wanted": True, "ready": True}, "mixed_port": 40808}
 namespace["cli_run"] = fake_cli
 verified_ports = []
 namespace["verify_proxy_clients"] = verified_ports.append
 calls = []
 def fake_run(args, **kwargs):
  calls.append(args)
  if args and args[0] == "/usr/bin/ditto": shutil.copytree(args[1], args[2], symlinks=True)
  return mock.Mock()
 with mock.patch.object(subprocess, "run", side_effect=fake_run):
  recover(previous, candidate, candidate, True, 40808)
 assert previous.exists() and candidate.exists()
 assert (root / "MyProxy.app" / "Contents" / "Info.plist").exists()
 assert any(args[:2] == ["/usr/bin/osascript", "-e"] for args in calls)
 assert any(args[:2] == ["codesign", "--verify"] for args in calls)
 assert ("connect",) in cli_calls
 assert verified_ports == [40808]
 print("mocked recovery success: PASS")

with tempfile.TemporaryDirectory(prefix="myproxy-upgrade-recovery-blocked-") as raw:
 root = Path(raw); backup = root / "backup"; backup.mkdir(); previous = backup / "MyProxy.app"; candidate = root / "MyProxy.app"; previous.mkdir(); candidate.mkdir()
 previous_contents = previous / "Contents"; (previous_contents / "MacOS").mkdir(parents=True); (previous_contents / "MacOS" / "myproxy").write_text("fixture")
 (previous_contents / "Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "local.harry.myproxy"}))
 contents = candidate / "Contents"; (contents / "MacOS").mkdir(parents=True); (contents / "MacOS" / "myproxy").write_text("fixture")
 (contents / "Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "local.harry.myproxy"}))
 namespace["wait_for_exact_process_exit"] = lambda executable: False
 namespace["cli_run"] = lambda cli_path, *args: {"xray": {"running": False, "wanted": False}, "extension_runtime": {"observed": True, "captureEnabled": False, "phase": "disabled", "dnsPhase": "disabled"}}
 with mock.patch.object(subprocess, "run", return_value=mock.Mock()):
  try:
   recover(previous, candidate, candidate, False, 40808)
  except RuntimeError as error:
   assert "did not quit" in str(error)
  else:
   raise AssertionError("expected candidate quit blocker")
 assert candidate.exists() and previous.exists()
print("mocked recovery blocker: PASS")

with tempfile.TemporaryDirectory(prefix="myproxy-upgrade-recovery-disconnect-") as raw:
 root = Path(raw); backup = root / "backup"; backup.mkdir(); previous = backup / "MyProxy.app"; candidate = root / "MyProxy.app"; previous.mkdir(); candidate.mkdir()
 for bundle in (previous, candidate):
  contents = bundle / "Contents"; (contents / "MacOS").mkdir(parents=True)
  (contents / "MacOS" / "myproxy").write_text("fixture")
  (contents / "Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "local.harry.myproxy"}))
 namespace["cli_run"] = lambda cli_path, *args: {"xray": {"running": True, "wanted": True}, "extension_runtime": {"observed": True, "captureEnabled": True, "phase": "running", "dnsPhase": "running"}}
 with mock.patch.object(subprocess, "run", return_value=mock.Mock()):
  try:
   recover(previous, candidate, candidate, False, 40808)
  except RuntimeError as error:
   assert "did not confirm" in str(error)
  else:
   raise AssertionError("expected disconnect confirmation blocker")
 assert candidate.exists() and previous.exists()
 print("mocked disconnect blocker: PASS")

with tempfile.TemporaryDirectory(prefix="myproxy-upgrade-recovery-backup-") as raw:
 root = Path(raw); backup = root / "backup"; backup.mkdir(); previous = backup / "MyProxy.app"; candidate = root / "MyProxy.app"; candidate.mkdir()
 contents = candidate / "Contents"; (contents / "MacOS").mkdir(parents=True); (contents / "MacOS" / "myproxy").write_text("fixture")
 (contents / "Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "local.harry.myproxy"}))
 namespace["cli_run"] = lambda cli_path, *args: {"xray": {"running": False, "wanted": False}, "extension_runtime": {"observed": True, "captureEnabled": False, "phase": "disabled", "dnsPhase": "disabled"}}
 with mock.patch.object(subprocess, "run", return_value=mock.Mock()):
  try:
   recover(previous, candidate, candidate, False, 40808)
  except FileNotFoundError:
   pass
  else:
   raise AssertionError("expected missing backup blocker")
 assert candidate.exists()
 print("mocked missing-backup blocker: PASS")
