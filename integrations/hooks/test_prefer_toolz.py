"""Exercise the hook's stdin/stdout contract without executing shell commands."""
import json
import shlex
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HOOK = Path(__file__).with_name("prefer_toolz.py").resolve()


class HookTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name).resolve()
        self.root = self.base / "project"
        self.root.mkdir()
        (self.root / "a file.txt").write_text("example")
        (self.root / "README").write_text("example")
        self.file = shlex.quote(str(self.root / "a file.txt"))
        self.outside = self.base / "project-other"
        self.outside.mkdir()
        (self.outside / "README").write_text("example")

    def invoke(self, event, roots=None, raw=None):
        args = [sys.executable, str(HOOK)]
        for root in roots if roots is not None else [self.root]:
            args.extend(["--protected-root", str(root)])
        return subprocess.run(args, input=json.dumps(event) if raw is None else raw,
                              text=True, capture_output=True)

    def event(self, command, tool="Bash", cwd=None):
        key = "cmd" if tool == "exec_command" else "command"
        return {"tool_name": tool, "tool_input": {key: command},
                "cwd": str(cwd or self.root)}

    def decision(self, command, deny, **kwargs):
        result = self.invoke(self.event(command, **kwargs))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, "")
        if deny:
            output = json.loads(result.stdout)["hookSpecificOutput"]
            self.assertEqual(output["hookEventName"], "PreToolUse")
            self.assertEqual(output["permissionDecision"], "deny")
            self.assertIn("repo_root", output["permissionDecisionReason"])
            self.assertIn("toolz-fallback", output["permissionDecisionReason"])
        else:
            self.assertEqual(result.stdout, "")

    def test_common_reads(self):
        for command in [f"cat {self.file}", "cat README", f"/bin/cat {self.file}",
                        f"head -n 5 {self.file}", f"tail -c10 {self.file}",
                        f"sed -n '1p' {self.file}", f"sed -e '1p' {self.file}",
                        f"sed -f {self.file}", f"awk '{{print}}' {self.file}",
                        f"jq . {self.file}", f"wc -l {self.file}",
                        f"env MODE=test cat {self.file}", f"command cat {self.file}",
                        f"cat < {self.file}", f"grep x < {self.file}",
                        f"echo hi\ncat {self.file}", f"echo hi && cat {self.file}",
                        f"echo hi || cat {self.file}", f"cat {self.file} | head -n 1"]:
            with self.subTest(command=command):
                self.decision(command, True)
        self.decision("cat README", True, tool="exec_command")
        self.decision(f"cd -- {shlex.quote(str(self.root))} && cat README", True, cwd=self.base)

    def test_allowed_commands(self):
        for command in [f"rg text {self.file}", f"grep text {self.file}",
                        f"cat {shlex.quote(str(self.outside / 'README'))}",
                        "echo 'cat README'", "python3 script.py",
                        "python3 -c \"print(\"\"\"open('README')\"\"\")\"",
                        "cat $TOOLZ_UNRESOLVED_TEST_VARIABLE", "printf hi | head -n 1",
                        "echo hi > README"]:
            with self.subTest(command=command):
                self.decision(command, False)
        result = self.invoke({"tool_name": "mcp__toolz__repo_read", "tool_input": {}})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")

    def test_literal_python(self):
        for code in ["open('a file.txt').read()", "io.open('a file.txt').read()",
                     "Path('a file.txt').read_text()", "Path('a file.txt').read_bytes()",
                     "pathlib.Path('a file.txt').open()"]:
            with self.subTest(code=code):
                self.decision("python3 -c " + shlex.quote(code), True)
        self.decision("python3 -c " + shlex.quote("print(\"open('a file.txt')\")"), False)
        self.decision("echo " + shlex.quote("python3 -c open('a file.txt')"), False)

    def test_fallback_comments(self):
        self.decision("# toolz-fallback: Toolz unavailable\ncat README", False)
        self.decision("  # toolz-fallback: Toolz unavailable\ncat README", False)
        for prefix in ["# unrelated toolz-fallback: reason", "# toolz-fallback:   ",
                       "echo '# toolz-fallback: reason'", 'echo "# toolz-fallback: reason"',
                       "echo 'start\n# toolz-fallback: reason\nend'",
                       'echo "start\n# toolz-fallback: reason\nend"']:
            with self.subTest(prefix=prefix):
                self.decision(prefix + "\ncat README", True)

    def test_symlinks(self):
        into = self.outside / "into"
        into.symlink_to(self.root / "README")
        out = self.root / "out"
        out.symlink_to(self.outside / "README")
        self.decision("cat " + shlex.quote(str(into)), True)
        self.decision("cat out", False)

    def test_bad_events(self):
        valid = self.event("true")
        events = [None, [], {}, {**valid, "tool_name": []}, {**valid, "tool_name": ""},
                  {**valid, "tool_input": None}, {**valid, "cwd": 123},
                  {**valid, "cwd": "relative"}, {**valid, "tool_input": {"command": []}},
                  {**valid, "tool_input": {"command": ""}}]
        for event in events:
            with self.subTest(event=event):
                result = self.invoke(event)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")
                self.assertNotIn("Traceback", result.stderr)
        for raw in ["not json", ""]:
            self.assertEqual(self.invoke(None, raw=raw).returncode, 2)
        self.assertEqual(self.invoke(self.event("cat 'unfinished")).returncode, 2)

    def test_roots_and_version(self):
        for roots in [[], ["relative"], ["/"], [self.base / "missing"], [self.root / "README"]]:
            with self.subTest(roots=roots):
                result = self.invoke(self.event("true"), roots=roots)
                self.assertEqual(result.returncode, 2)
                self.assertNotIn("Traceback", result.stderr)
        result = subprocess.run([sys.executable, str(HOOK), "--version"],
                                text=True, capture_output=True)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout.strip(), "1.0.0")
        self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
