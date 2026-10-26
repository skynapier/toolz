# Optional Toolz read hook

Toolz works without hooks. `integrations/hooks/prefer_toolz.py` is an optional,
standalone Python 3.9+ `PreToolUse` hook that directs common shell file reads to
Toolz. It contains only the Toolz read preference; personal codez implementation
and cleanup policies are outside its scope.

## Configure

Point your host's hook command at the checked-out script. Pass one or more
`--protected-root` arguments naming existing absolute directories, excluding `/`.
No installation manager, generated policy file, or extra Python package is needed.

Merge this entry into your existing `hooks.PreToolUse` array; preserve other entries
and settings. Replace both example paths with your actual absolute paths:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash|exec_command",
        "hooks": [
          {
            "type": "command",
            "command": "python3 '/absolute/path/to/toolz/integrations/hooks/prefer_toolz.py' --protected-root '/absolute/path/to/projects' || exit 2"
          }
        ]
      }
    ]
  }
}
```

The command requires `python3` on the host's PATH. Quote shell paths appropriately,
especially paths containing spaces or apostrophes. Host configuration locations:

| Host | Configuration |
| --- | --- |
| Claude | `~/.claude/settings.json` |
| Codex | `~/.codex/hooks.json` |
| Codey | `~/.codey/hooks.json` |

Reload the host as needed. Codex/Codey may require reviewing and trusting the hook
through `/hooks`. These are configuration examples; actual host loading and trust
have not been tested end to end. Nothing changes your live host configuration.

## Behavior and limits

- Inspect `Bash` (`command`) and `exec_command` (`cmd`) events using their absolute
  `cwd`. Other tool names, including Toolz MCP calls, pass through.
- Deny common literal file reads such as `cat`, `head`, `tail`, `sed`, `awk`, `jq`,
  input redirection, and literal Python `-c` calls using `open`, `io.open`, or
  `Path(...).read_text/read_bytes/open`. Follow simple `cd` chains and resolve
  symlinks before checking whether a path lies under a protected root.
- Allow `rg`/`grep` searches and reads outside protected roots. The hook does not
  impose search output bounds; callers should still bound their searches.
- A denied call receives a JSON decision directing it to deferred Toolz tools
  (`repo_read`, `repo_search`, `diff_inspect`) with an absolute `repo_root`.
- If Toolz is unavailable or cannot answer, put a real standalone shell comment
  `# toolz-fallback: <reason>` in the command and explain the reason to the user.
  A marker inside a quoted string does not grant an exception.
- Allowed calls exit 0 with no stdout. Denials exit 0 with a `PreToolUse` JSON
  decision. Invalid arguments/events exit 2 with a diagnostic on stderr.

This is a convenience guard, not a security sandbox or a complete shell parser.
Dynamic variables, interpreter aliases, nested shells, substitutions, heredocs,
and complex reader options are not comprehensively analyzed. It never executes
commands while inspecting them.

## Update, remove, and migrate

The host references the checked-out script directly, so reviewed changes pulled
into Toolz are used by subsequent invocations. Run the tests after updating and
reload/review trust if your host requires it. `--version` reports the script version.
To remove the integration, remove only this hook command from your host config.

If a personal `enforce_codez.py` already contains the same Toolz read rule, test this
standalone hook first, then manually remove only that overlapping read rule. Keep
your codez and cleanup policies. No migration or trust changes run automatically.

## Verify

```bash
python3 integrations/hooks/prefer_toolz.py --version
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s integrations/hooks -p 'test_*.py'
```

Tests feed simulated events to the script and use temporary repositories. They do
not execute the inspected shell commands or edit any host configuration.
