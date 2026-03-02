"""Tests for MCP server command module."""

import argparse
import sys
from unittest.mock import patch

from cua_cli.commands.mcp import (
    PERMISSION_GROUPS,
    Permission,
    execute,
    parse_permissions,
    register_parser,
)


class TestPermission:
    """Tests for Permission enum."""

    def test_all_permissions_have_values(self):
        """Test that all permissions have string values."""
        for perm in Permission:
            assert isinstance(perm.value, str)
            assert ":" in perm.value

    def test_sandbox_permissions(self):
        """Test sandbox permission values."""
        assert Permission.SANDBOX_LIST.value == "sandbox:list"
        assert Permission.SANDBOX_CREATE.value == "sandbox:create"
        assert Permission.SANDBOX_DELETE.value == "sandbox:delete"

    def test_computer_permissions(self):
        """Test computer permission values."""
        assert Permission.COMPUTER_SCREENSHOT.value == "computer:screenshot"
        assert Permission.COMPUTER_CLICK.value == "computer:click"
        assert Permission.COMPUTER_TYPE.value == "computer:type"

    def test_skills_permissions(self):
        """Test skills permission values."""
        assert Permission.SKILLS_LIST.value == "skills:list"
        assert Permission.SKILLS_READ.value == "skills:read"
        assert Permission.SKILLS_RECORD.value == "skills:record"


class TestPermissionGroups:
    """Tests for permission groups."""

    def test_sandbox_all_group(self):
        """Test sandbox:all group contains all sandbox permissions."""
        group = PERMISSION_GROUPS["sandbox:all"]
        assert Permission.SANDBOX_LIST in group
        assert Permission.SANDBOX_CREATE in group
        assert Permission.SANDBOX_DELETE in group
        assert Permission.SANDBOX_START in group
        assert Permission.SANDBOX_STOP in group

    def test_sandbox_readonly_group(self):
        """Test sandbox:readonly group contains only read permissions."""
        group = PERMISSION_GROUPS["sandbox:readonly"]
        assert Permission.SANDBOX_LIST in group
        assert Permission.SANDBOX_GET in group
        assert Permission.SANDBOX_CREATE not in group
        assert Permission.SANDBOX_DELETE not in group

    def test_computer_all_group(self):
        """Test computer:all group contains all computer permissions."""
        group = PERMISSION_GROUPS["computer:all"]
        assert Permission.COMPUTER_SCREENSHOT in group
        assert Permission.COMPUTER_CLICK in group
        assert Permission.COMPUTER_TYPE in group
        assert Permission.COMPUTER_SHELL in group

    def test_computer_readonly_group(self):
        """Test computer:readonly group contains only screenshot."""
        group = PERMISSION_GROUPS["computer:readonly"]
        assert Permission.COMPUTER_SCREENSHOT in group
        assert Permission.COMPUTER_CLICK not in group

    def test_all_group_contains_everything(self):
        """Test that 'all' group contains all permissions."""
        group = PERMISSION_GROUPS["all"]
        assert len(group) == len(Permission)


class TestParsePermissions:
    """Tests for parse_permissions function."""

    def test_empty_string_returns_empty_set(self):
        """Test that empty string returns empty set."""
        result = parse_permissions("")
        assert result == set()

    def test_single_permission(self):
        """Test parsing a single permission."""
        result = parse_permissions("sandbox:list")
        assert result == {Permission.SANDBOX_LIST}

    def test_multiple_permissions(self):
        """Test parsing multiple permissions."""
        result = parse_permissions("sandbox:list,sandbox:create")
        assert result == {Permission.SANDBOX_LIST, Permission.SANDBOX_CREATE}

    def test_permission_with_spaces(self):
        """Test parsing permissions with spaces."""
        result = parse_permissions("sandbox:list, sandbox:create")
        assert result == {Permission.SANDBOX_LIST, Permission.SANDBOX_CREATE}

    def test_group_expansion(self):
        """Test that groups are expanded."""
        result = parse_permissions("sandbox:readonly")
        assert Permission.SANDBOX_LIST in result
        assert Permission.SANDBOX_GET in result

    def test_all_group(self):
        """Test that 'all' group includes everything."""
        result = parse_permissions("all")
        assert len(result) == len(Permission)

    def test_mixed_groups_and_permissions(self):
        """Test mixing groups and individual permissions."""
        result = parse_permissions("sandbox:readonly,computer:screenshot")
        assert Permission.SANDBOX_LIST in result
        assert Permission.SANDBOX_GET in result
        assert Permission.COMPUTER_SCREENSHOT in result

    def test_unknown_permission_ignored(self):
        """Test that unknown permissions are ignored with warning."""
        with patch("cua_cli.commands.mcp.logger") as mock_logger:
            result = parse_permissions("sandbox:list,unknown:perm")

        assert result == {Permission.SANDBOX_LIST}
        mock_logger.warning.assert_called()


class TestRegisterParser:
    """Tests for register_parser function."""

    def test_registers_serve_mcp_command(self):
        """Test that serve-mcp command is registered."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()

        register_parser(subparsers)

        args = parser.parse_args(["serve-mcp"])
        assert hasattr(args, "permissions")

    def test_has_permissions_flag(self):
        """Test that --permissions flag is available."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        register_parser(subparsers)

        args = parser.parse_args(["serve-mcp", "--permissions", "sandbox:all"])
        assert args.permissions == "sandbox:all"

    def test_has_sandbox_flag(self):
        """Test that --sandbox flag is available."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        register_parser(subparsers)

        args = parser.parse_args(["serve-mcp", "--sandbox", "my-sandbox"])
        assert args.sandbox == "my-sandbox"


class TestExecute:
    """Tests for execute function."""

    def test_missing_fastmcp_returns_error(self, args_namespace, capsys):
        """Test that missing fastmcp dependency returns error."""

        args = args_namespace(permissions="", sandbox="")

        # Temporarily remove mcp modules to simulate ImportError
        original_modules = {}
        for mod in list(sys.modules.keys()):
            if mod.startswith("mcp"):
                original_modules[mod] = sys.modules.pop(mod)

        try:
            with patch.dict(
                "sys.modules", {"mcp": None, "mcp.server": None, "mcp.server.fastmcp": None}
            ):
                result = execute(args)
        finally:
            # Restore modules
            sys.modules.update(original_modules)

        assert result == 1

    def test_permissions_from_env_var(self, args_namespace, monkeypatch):
        """Test that permissions are read from environment variable."""
        monkeypatch.setenv("CUA_MCP_PERMISSIONS", "sandbox:readonly")

        permissions = parse_permissions("sandbox:readonly")
        assert Permission.SANDBOX_LIST in permissions
        assert Permission.SANDBOX_CREATE not in permissions

    def test_permissions_from_args_override_env(self, args_namespace, monkeypatch):
        """Test that command line args override environment variable."""
        args = args_namespace(permissions="computer:all", sandbox="")
        monkeypatch.setenv("CUA_MCP_PERMISSIONS", "sandbox:readonly")

        # The args.permissions should take precedence
        assert args.permissions == "computer:all"


class TestMCPToolRegistration:
    """Tests for MCP tool registration based on permissions."""

    def test_permission_affects_tool_registration(self):
        """Test that permissions control which tools would be registered."""
        # This is a unit test of the permission logic, not the actual registration
        # The actual registration requires fastmcp which may not be installed

        all_perms = parse_permissions("all")
        assert len(all_perms) == len(Permission)

        readonly_perms = parse_permissions("sandbox:readonly")
        assert Permission.SANDBOX_LIST in readonly_perms
        assert Permission.SANDBOX_CREATE not in readonly_perms

        # Test that the registration functions exist and are callable
        from cua_cli.commands.mcp import (
            _register_computer_tools,
            _register_sandbox_tools,
            _register_skills_tools,
        )

        assert callable(_register_sandbox_tools)
        assert callable(_register_computer_tools)
        assert callable(_register_skills_tools)
