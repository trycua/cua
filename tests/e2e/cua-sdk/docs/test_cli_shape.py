"""Unit tests for the cli-shape lane (stdlib only)."""

from __future__ import annotations

import unittest

import cli_shape as cs

SPEC = {
    "cua": {
        "name": "cua",
        "global_options": [
            {"name": "json", "takes_value": False},
            {"name": "daemon", "type": "DAEMON", "takes_value": True},
        ],
        "commands": [
            {
                "name": "sandbox",
                "aliases": ["sb"],
                "usage": "cua sandbox [OPTIONS] <COMMAND>",
                "arguments": [],
                "options": [],
                "flags": [],
                "subcommands": [
                    {
                        "name": "create",
                        "usage": "cua sandbox create [OPTIONS] [IMAGE] [-- <COMMAND>...]",
                        "arguments": [
                            {"name": "IMAGE", "is_optional": True},
                            {"name": "COMMAND", "is_optional": True, "repeatable": True},
                        ],
                        "options": [
                            {"name": "on", "type": "ON", "takes_value": True},
                            {
                                "name": "name",
                                "short_name": "n",
                                "type": "NAME",
                                "takes_value": True,
                            },
                            {"name": "format", "type": "md | json", "takes_value": True},
                        ],
                        "flags": [
                            {"name": "vm", "short_name": "y"},
                            {"name": "x", "short_name": "x"},
                        ],
                        "subcommands": [],
                    },
                    {
                        "name": "exec",
                        "usage": "cua sandbox exec [OPTIONS] <NAME> <COMMAND>...",
                        "arguments": [
                            {"name": "NAME", "is_optional": False},
                            {"name": "COMMAND", "is_optional": False, "repeatable": True},
                        ],
                        "options": [],
                        "flags": [],
                        "subcommands": [],
                    },
                    {
                        "name": "mcp",
                        "usage": "cua sandbox mcp [OPTIONS] <NAME> <SERVICE> <COMMAND>",
                        "arguments": [
                            {"name": "NAME", "is_optional": False},
                            {"name": "SERVICE", "is_optional": False},
                        ],
                        "options": [],
                        "flags": [],
                        "subcommands": [
                            {
                                "name": "tools",
                                "arguments": [],
                                "options": [],
                                "flags": [],
                                "subcommands": [],
                            }
                        ],
                    },
                ],
            }
        ],
    },
    "cua-driver": {
        "name": "cua-driver",
        "commands": [
            {"name": "serve", "arguments": [], "options": [], "flags": [], "subcommands": []}
        ],
    },
}


def problems(code: str, lang: str = "bash") -> list[str]:
    return cs.check_block(code, lang, SPEC)[1]


class CliShapeTest(unittest.TestCase):
    def test_valid_commands(self):
        for code in [
            "cua sb create linux --name dev --on cloud",
            "cua --json sb create linux -n dev -- python -m http.server 8000",
            "cua sandbox create linux --format=json -yx",
            "cua sb exec dev uname -a",
            "cua sb exec <name> ls -la /tmp | head",
            "cua sb mcp web mcp tools",
            "X=1 sudo cua sb create \\\n  linux --vm",
            "echo hi && cua sb create linux; cua sb --help",
            'TOKEN="$(cua --json sb create linux)"',
            "cua-driver serve",
            "cua-driver list_apps '{}'",
            "cat <<'EOF' > x\ncua sb bogus\nEOF\ncua sb create linux",
            "# a comment\ncua --version",
        ]:
            with self.subTest(code=code):
                self.assertEqual(problems(code), [])

    def test_invalid_commands(self):
        for code, needle in [
            ("cua sb creat linux", "unknown command 'creat'"),
            ("cua sb create linux --nope", "unknown option --nope"),
            ("cua sb create linux --name", "needs a value"),
            ("cua sb create linux --format yaml", "takes md | json"),
            ("cua sb create linux --vm=1", "takes no value"),
            ("cua sb exec", "missing <NAME>"),
            ("cua sb", "missing subcommand"),
            ("cua", "missing command"),
            ("cua sb create a b", "unexpected argument"),
            ("cua-driver sreve", "unknown command"),
        ]:
            with self.subTest(code=code):
                found = problems(code)
                self.assertTrue(any(needle in p for p in found), found)

    def test_console_blocks_check_prompted_lines_only(self):
        code = "$ cua sb create linux\ncreated dev\n$ cua sb ls\nNAME STATUS\n"
        self.assertEqual(
            cs.check_block(code, "console", SPEC),
            (
                2,
                [
                    "'cua sb ls': cua sandbox: unknown command 'ls' (expected one of create, exec, mcp)"
                ],
            ),
        )

    def test_powershell_continuation(self):
        self.assertEqual(problems("cua sb create linux `\n  --name dev", "powershell"), [])

    def test_blocks_without_invocations(self):
        self.assertEqual(cs.check_block("docker ps\nls cua", "bash", SPEC), (0, []))


if __name__ == "__main__":
    unittest.main()
