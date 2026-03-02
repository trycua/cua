"""Tests for skills command module."""

import argparse
from unittest.mock import patch

from cua_cli.commands import skills


class TestRegisterParser:
    """Tests for register_parser function."""

    def test_registers_skills_command(self):
        """Test that skills command is registered."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()

        skills.register_parser(subparsers)

        args = parser.parse_args(["skills", "list"])
        assert args.skills_command == "list"

    def test_record_command_exists(self):
        """Test that record command is registered."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        skills.register_parser(subparsers)

        args = parser.parse_args(["skills", "record"])
        assert args.skills_command == "record"

    def test_record_command_has_sandbox_flag(self):
        """Test that record command has --sandbox flag."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        skills.register_parser(subparsers)

        args = parser.parse_args(["skills", "record", "--sandbox", "my-sandbox"])
        assert args.sandbox == "my-sandbox"

    def test_read_command_has_name_arg(self):
        """Test that read command has name argument."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        skills.register_parser(subparsers)

        args = parser.parse_args(["skills", "read", "my-skill"])
        assert args.name == "my-skill"


class TestExecute:
    """Tests for execute function."""

    def test_dispatch_to_list(self, args_namespace):
        """Test dispatch to list command."""
        args = args_namespace(command="skills", skills_command="list", json=False)

        with patch.object(skills, "cmd_list", return_value=0) as mock_cmd:
            result = skills.execute(args)

        mock_cmd.assert_called_once_with(args)
        assert result == 0

    def test_dispatch_to_read(self, args_namespace):
        """Test dispatch to read command."""
        args = args_namespace(command="skills", skills_command="read", name="my-skill")

        with patch.object(skills, "cmd_read", return_value=0) as mock_cmd:
            skills.execute(args)

        mock_cmd.assert_called_once_with(args)

    def test_dispatch_to_delete(self, args_namespace):
        """Test dispatch to delete command."""
        args = args_namespace(command="skills", skills_command="delete", name="my-skill")

        with patch.object(skills, "cmd_delete", return_value=0) as mock_cmd:
            skills.execute(args)

        mock_cmd.assert_called_once_with(args)

    def test_unknown_command_returns_error(self, args_namespace):
        """Test that unknown command returns error."""
        args = args_namespace(command="skills", skills_command=None)

        with patch.object(skills, "print_error"):
            result = skills.execute(args)

        assert result == 1


class TestCmdList:
    """Tests for cmd_list function."""

    def test_list_empty_directory(self, args_namespace, temp_skills_dir):
        """Test listing when no skills exist."""
        args = args_namespace(json=False)

        with patch.object(skills, "print_info") as mock_print:
            result = skills.cmd_list(args)

        assert result == 0
        mock_print.assert_called()

    def test_list_with_skills(self, args_namespace, sample_skill, temp_skills_dir):
        """Test listing existing skills."""
        args = args_namespace(json=False)

        with patch.object(skills, "print_table") as mock_print:
            result = skills.cmd_list(args)

        assert result == 0
        mock_print.assert_called_once()
        # Verify the skill is in the output
        call_args = mock_print.call_args[0]
        skill_data = call_args[0]
        assert len(skill_data) == 1
        assert skill_data[0]["name"] == "test-skill"

    def test_list_json_output(self, args_namespace, sample_skill, temp_skills_dir):
        """Test JSON output format."""
        args = args_namespace(json=True)

        with patch.object(skills, "print_json") as mock_print:
            result = skills.cmd_list(args)

        assert result == 0
        mock_print.assert_called_once()


class TestCmdRead:
    """Tests for cmd_read function."""

    def test_read_existing_skill(self, args_namespace, sample_skill, temp_skills_dir, capsys):
        """Test reading an existing skill."""
        args = args_namespace(name="test-skill", format="md")

        result = skills.cmd_read(args)

        assert result == 0
        # Verify content was printed to stdout
        captured = capsys.readouterr()
        assert "test-skill" in captured.out

    def test_read_nonexistent_skill(self, args_namespace, temp_skills_dir):
        """Test reading a nonexistent skill."""
        args = args_namespace(name="nonexistent", format="md")

        with patch.object(skills, "print_error") as mock_error:
            result = skills.cmd_read(args)

        assert result == 1
        mock_error.assert_called()


class TestCmdDelete:
    """Tests for cmd_delete function."""

    def test_delete_existing_skill(self, args_namespace, sample_skill, temp_skills_dir):
        """Test deleting an existing skill."""
        args = args_namespace(name="test-skill")

        assert sample_skill.exists()

        with patch.object(skills, "print_success"):
            result = skills.cmd_delete(args)

        assert result == 0
        assert not sample_skill.exists()

    def test_delete_nonexistent_skill(self, args_namespace, temp_skills_dir):
        """Test deleting a nonexistent skill."""
        args = args_namespace(name="nonexistent")

        with patch.object(skills, "print_error") as mock_error:
            result = skills.cmd_delete(args)

        assert result == 1
        mock_error.assert_called()


class TestCmdClean:
    """Tests for cmd_clean function."""

    def test_clean_removes_all_skills(self, args_namespace, sample_skill, temp_skills_dir):
        """Test that clean removes all skills."""
        args = args_namespace()

        assert sample_skill.exists()

        with patch("builtins.input", return_value="y"):
            with patch.object(skills, "print_success"):
                result = skills.cmd_clean(args)

        assert result == 0
        # Verify no skills remain
        remaining = list(temp_skills_dir.iterdir())
        assert len(remaining) == 0

    def test_clean_empty_directory(self, args_namespace, temp_skills_dir):
        """Test clean on empty directory."""
        args = args_namespace()

        with patch.object(skills, "print_info"):
            result = skills.cmd_clean(args)

        assert result == 0


class TestCmdReplay:
    """Tests for cmd_replay function."""

    def test_replay_existing_skill(
        self, args_namespace, sample_skill, temp_skills_dir, mock_webbrowser
    ):
        """Test replaying an existing skill opens the video."""
        args = args_namespace(name="test-skill")

        with patch.object(skills, "print_info"):
            result = skills.cmd_replay(args)

        assert result == 0
        # Verify webbrowser was called with the video path
        mock_webbrowser.assert_called_once()
        call_url = mock_webbrowser.call_args[0][0]
        assert "test-skill.mp4" in call_url

    def test_replay_nonexistent_skill(self, args_namespace, temp_skills_dir):
        """Test replaying a nonexistent skill."""
        args = args_namespace(name="nonexistent")

        with patch.object(skills, "print_error") as mock_error:
            result = skills.cmd_replay(args)

        assert result == 1
        mock_error.assert_called()
