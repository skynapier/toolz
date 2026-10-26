#!/usr/bin/env python3
"""Prefer Toolz for common shell reads. A convenience guard, not a sandbox."""
import argparse
import ast
import json
import os
import re
import shlex
import sys
from pathlib import Path

VERSION = "1.0.0"
READERS = set("cat head tail sed awk less more wc nl xxd od strings bat jq yq".split())
PREFIXES = {"env", "command", "builtin", "nohup", "time"}
SEPARATORS = {";", "&&", "||", "|", "&", "|&", "(", ")"}
ASSIGNMENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")


def shell_text(command):
    """1. Preserve quotes and remove comments. 2. Recognize real fallback lines."""
    # 1.
    result, quote, i, fallback = [], None, 0, False
    while i < len(command):
        char = command[i]
        if char == "\\" and quote != "'" and i + 1 < len(command):
            result.extend(command[i:i + 2])
            i += 2
            continue
        if quote:
            if char == quote:
                quote = None
        elif char in "'\"":
            quote = char
        elif char == "#" and (i == 0 or command[i - 1].isspace()):
            end = command.find("\n", i)
            end = len(command) if end == -1 else end
            prefix = command[command.rfind("\n", 0, i) + 1:i]
            # 2.
            if not prefix.strip() and re.fullmatch(r"# toolz-fallback:\s*\S.*", command[i:end]):
                fallback = True
            i = end
            continue
        elif char == "\n":
            char = ";"
        result.append(char)
        i += 1
    return "".join(result), fallback


def segments(command):
    lexer = shlex.shlex(command, posix=True, punctuation_chars=True)
    lexer.whitespace_split = True
    lexer.commenters = ""
    current = []
    for token in lexer:
        if token in SEPARATORS:
            if current:
                yield current
            current = []
        else:
            current.append(token)
    if current:
        yield current


def unprefix(words):
    while words and (words[0] in PREFIXES or ASSIGNMENT.match(words[0])):
        words = words[1:]
    return words


def protected(raw, cwd, roots):
    if not raw or raw == "-":
        return False
    expanded = os.path.expandvars(os.path.expanduser(raw))
    if "$" in expanded or "`" in expanded:
        return False
    path = (cwd / expanded).resolve()
    return any(path == root or root in path.parents for root in roots)


def python_paths(code):
    """Find literal open/io.open and Path(...).read_text/read_bytes/open calls."""
    try:
        tree = ast.parse(code)
    except SyntaxError:
        return
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        func, args, target = node.func, node.args, None
        if isinstance(func, ast.Name) and func.id == "open":
            target = args[0] if args else None
        elif isinstance(func, ast.Attribute):
            if isinstance(func.value, ast.Name) and func.value.id == "io" and func.attr == "open":
                target = args[0] if args else None
            elif func.attr in {"read_text", "read_bytes", "open"} and isinstance(func.value, ast.Call):
                receiver = func.value
                constructor = receiver.func
                if ((isinstance(constructor, ast.Name) and constructor.id == "Path") or
                        (isinstance(constructor, ast.Attribute) and constructor.attr == "Path" and
                         isinstance(constructor.value, ast.Name) and constructor.value.id == "pathlib")):
                    target = receiver.args[0] if receiver.args else None
        if isinstance(target, ast.Constant) and isinstance(target.value, str):
            yield target.value


def reader_paths(name, args):
    """Skip reader options and expressions, keeping explicit file operands."""
    needs_pattern = name in {"sed", "awk", "jq", "yq"}
    i, options = 0, True
    while i < len(args):
        item = args[i]
        i += 1
        if options and item == "--":
            options = False
            continue
        if options and item.startswith("-") and item != "-":
            if name in {"sed", "awk"} and item in {"-e", "--expression"}:
                needs_pattern = False
                i += 1
            elif name in {"sed", "awk"} and item in {"-f", "--file"}:
                needs_pattern = False
                if i < len(args):
                    yield args[i]
                i += 1
            elif name in {"head", "tail"} and item in {"-n", "-c", "--lines", "--bytes"}:
                i += 1
            elif name in {"sed", "awk"} and item.startswith("-e"):
                needs_pattern = False
            elif name in {"sed", "awk"} and item.startswith("-f"):
                needs_pattern = False
                yield item[2:]
            continue
        if needs_pattern:
            needs_pattern = False
        else:
            yield item


def blocked(command, cwd, roots):
    """1. Parse simple commands. 2. Follow cd and inspect explicit reads."""
    text, fallback = shell_text(command)
    if fallback:
        return False
    # 1.
    for tokens in segments(text):
        words, i = [], 0
        while i < len(tokens):
            token = tokens[i]
            if token == "<" and i + 1 < len(tokens) and protected(tokens[i + 1], cwd, roots):
                return True
            if token and set(token) <= set("<>&"):
                i += 2
            else:
                words.append(token)
                i += 1
        words = unprefix(words)
        if not words:
            continue
        name = Path(words[0]).name
        # 2.
        if name == "cd":
            operands = words[1:]
            if operands and operands[0] == "--":
                operands = operands[1:]
            if operands and "$" not in operands[0]:
                cwd = (cwd / os.path.expanduser(operands[0])).resolve()
        elif name in READERS:
            if any(protected(path, cwd, roots) for path in reader_paths(name, words[1:])):
                return True
        elif re.fullmatch(r"python(?:\d+(?:\.\d+)*)?", name) and "-c" in words[1:]:
            index = words.index("-c") + 1
            if index < len(words) and any(protected(path, cwd, roots) for path in python_paths(words[index])):
                return True
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", action="version", version=VERSION)
    parser.add_argument("--protected-root", action="append", required=True)
    args = parser.parse_args()
    try:
        roots = []
        for raw in args.protected_root:
            path = Path(raw)
            if not path.is_absolute() or not path.is_dir() or path.resolve() == Path("/"):
                raise ValueError("--protected-root requires an existing absolute directory other than /")
            roots.append(path.resolve())
        event = json.load(sys.stdin)
        if not isinstance(event, dict) or not isinstance(event.get("tool_name"), str) or not event["tool_name"]:
            raise ValueError("event requires tool_name")
        if not isinstance(event.get("tool_input"), dict):
            raise ValueError("event requires object tool_input")
        if event["tool_name"] not in {"Bash", "exec_command"}:
            return 0
        cwd = event.get("cwd")
        command = event["tool_input"].get("command", event["tool_input"].get("cmd"))
        if not isinstance(cwd, str) or not Path(cwd).is_absolute():
            raise ValueError("shell event requires absolute cwd")
        if not isinstance(command, str) or not command.strip():
            raise ValueError("shell event requires nonempty command or cmd")
        if blocked(command, Path(cwd), roots):
            reason = (
                "Use Toolz repo_read/repo_search/diff_inspect for protected repository reads. "
                "Discover deferred mcp__toolz__ tools and supply an absolute repo_root. "
                "If Toolz cannot answer, add a standalone '# toolz-fallback: reason' shell "
                "comment and explain the reason to the user."
            )
            print(json.dumps({"hookSpecificOutput": {"hookEventName": "PreToolUse",
                "permissionDecision": "deny", "permissionDecisionReason": reason}}))
        return 0
    except (ValueError, OSError, RuntimeError) as exc:
        print(f"prefer_toolz: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
