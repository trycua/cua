from collections import defaultdict
from pathlib import Path
from typing import Any, Dict, Literal, get_args

from computer.computer import Computer

from ....core.tools.edit import BaseEditTool
from .base import BaseAnthropicTool, CLIResult, ToolError, ToolResult
from .run import maybe_truncate

Command = Literal[
    "view",
    "create",
    "str_replace",
    "insert",
    "undo_edit",
]
SNIPPET_LINES: int = 4


class EditTool(BaseEditTool, BaseAnthropicTool):
    """
    An filesystem editor tool that allows the agent to view, create, and edit files.
    The tool parameters are defined by Anthropic and are not editable.
    """

    api_type: Literal["text_editor_20250124"] = "text_editor_20250124"
    name: Literal["str_replace_editor"] = "str_replace_editor"
    _timeout: float = 30.0  # seconds

    def __init__(self, computer: Computer):
        """Initialize the edit tool.

        Args:
            computer: Computer instance for file operations
        """
        # Initialize the base edit tool first
        BaseEditTool.__init__(self, computer)
        # Then initialize the Anthropic tool
        BaseAnthropicTool.__init__(self)

        # Edit history for the current session
        self.edit_history = defaultdict(list)

    async def __call__(
        self,
        *,
        command: Command,
        path: str,
        file_text: str | None = None,
        view_range: list[int] | None = None,
        old_str: str | None = None,
        new_str: str | None = None,
        insert_line: int | None = None,
        **kwargs,
    ):
        _path = Path(path)
        await self.validate_path(command, _path)

        if command == "view":
            return await self.view(_path, view_range)
        elif command == "create":
            if file_text is None:
                raise ToolError("Parameter `file_text` is required for command: create")
            await self.write_file(_path, file_text)
            self.edit_history[_path].append(file_text)
            return ToolResult(output=f"File created successfully at: {_path}")
        elif command == "str_replace":
            if old_str is None:
                raise ToolError("Parameter `old_str` is required for command: str_replace")
            return await self.str_replace(_path, old_str, new_str)
        elif command == "insert":
            if insert_line is None:
                raise ToolError("Parameter `insert_line` is required for command: insert")
            if new_str is None:
                raise ToolError("Parameter `new_str` is required for command: insert")
            return await self.insert(_path, insert_line, new_str)
        elif command == "undo_edit":
            return await self.undo_edit(_path)

        raise ToolError(
            f'Unrecognized command {command}. The allowed commands for the {self.name} tool are: {", ".join(get_args(Command))}'
        )

    async def validate_path(self, command: str, path: Path):
        """Check that the path/command combination is valid."""
        # Check if its an absolute path
        if not path.is_absolute():
            suggested_path = Path("") / path
            raise ToolError(
                f"The path {path} is not an absolute path, it should start with `/`. Maybe you meant {suggested_path}?"
            )

        # Check if path exists using bash commands
        try:
            result = await self.computer.interface.run_command(
                f'[ -e "{str(path)}" ] && echo "exists" || echo "not exists"'
            )
            exists = result.stdout.strip() == "exists"

            if exists:
                result = await self.computer.interface.run_command(
                    f'[ -d "{str(path)}" ] && echo "dir" || echo "file"'
                )
                is_dir = result.stdout.strip() == "dir"
            else:
                is_dir = False

            # Check path validity
            if not exists and command != "create":
                raise ToolError(f"The path {path} does not exist. Please provide a valid path.")
            if exists and command == "create":
                raise ToolError(
                    f"File already exists at: {path}. Cannot overwrite files using command `create`."
                )
            if is_dir and command != "view":
                raise ToolError(
                    f"The path {path} is a directory and only the `view` command can be used on directories"
                )
        except Exception as e:
            raise ToolError(f"Failed to validate path: {str(e)}")

    async def view(self, path: Path, view_range: list[int] | None = None):
        """Implement the view command"""
        try:
            # Check if path is a directory
            result = await self.computer.interface.run_command(
                f'[ -d "{str(path)}" ] && echo "dir" || echo "file"'
            )
            is_dir = result.stdout.strip() == "dir"

            if is_dir:
                if view_range:
                    raise ToolError(
                        "The `view_range` parameter is not allowed when `path` points to a directory."
                    )

                # List directory contents using ls
                result = await self.computer.interface.run_command(f'ls -la "{str(path)}"')
                contents = result.stdout
                if contents:
                    stdout = f"Here's the files and directories in {path}:\n{contents}\n"
                else:
                    stdout = f"Directory {path} is empty\n"
                return CLIResult(output=stdout)

            # Read file content using cat
            file_content = await self.read_file(path)
            init_line = 1

            if view_range:
                if len(view_range) != 2 or not all(isinstance(i, int) for i in view_range):
                    raise ToolError("Invalid `view_range`. It should be a list of two integers.")

                file_lines = file_content.split("\n")
                n_lines_file = len(file_lines)
                init_line, final_line = view_range

                if init_line < 1 or init_line > n_lines_file:
                    raise ToolError(
                        f"Invalid `view_range`: {view_range}. Its first element `{init_line}` should be within the range of lines of the file: {[1, n_lines_file]}"
                    )
                if final_line > n_lines_file:
                    raise ToolError(
                        f"Invalid `view_range`: {view_range}. Its second element `{final_line}` should be smaller than the number of lines in the file: `{n_lines_file}`"
                    )
                if final_line != -1 and final_line < init_line:
                    raise ToolError(
                        f"Invalid `view_range`: {view_range}. Its second element `{final_line}` should be larger or equal than its first `{init_line}`"
                    )

                if final_line == -1:
                    file_content = "\n".join(file_lines[init_line - 1 :])
                else:
                    file_content = "\n".join(file_lines[init_line - 1 : final_line])

            return CLIResult(output=self._make_output(file_content, str(path), init_line=init_line))
        except Exception as e:
            raise ToolError(f"Failed to view path: {str(e)}")

    async def str_replace(self, path: Path, old_str: str, new_str: str | None):
        """Implement the str_replace command"""
        # Read the file content
        file_content = await self.read_file(path)
        file_content = file_content.expandtabs()
        old_str = old_str.expandtabs()
        new_str = new_str.expandtabs() if new_str is not None else ""

        # Check if old_str is unique in the file
        occurrences = file_content.count(old_str)
        if occurrences == 0:
            raise ToolError(
                f"No replacement was performed, old_str `{old_str}` did not appear verbatim in {path}."
            )
        elif occurrences > 1:
            file_content_lines = file_content.split("\n")
            lines = [idx + 1 for idx, line in enumerate(file_content_lines) if old_str in line]
            raise ToolError(
                f"No replacement was performed. Multiple occurrences of old_str `{old_str}` in lines {lines}. Please ensure it is unique"
            )

        # Replace old_str with new_str
        new_file_content = file_content.replace(old_str, new_str)

        # Write the new content to the file
        await self.write_file(path, new_file_content)

        # Save the content to history
        self.edit_history[path].append(file_content)

        # Create a snippet of the edited section
        replacement_line = file_content.split(old_str)[0].count("\n")
        start_line = max(0, replacement_line - SNIPPET_LINES)
        end_line = replacement_line + SNIPPET_LINES + new_str.count("\n")
        snippet = "\n".join(new_file_content.split("\n")[start_line : end_line + 1])

        # Prepare the success message
        success_msg = f"The file {path} has been edited. "
        success_msg += self._make_output(snippet, f"a snippet of {path}", start_line + 1)
        success_msg += "Review the changes and make sure they are as expected. Edit the file again if necessary."

        return CLIResult(output=success_msg)

    async def insert(self, path: Path, insert_line: int, new_str: str):
        """Implement the insert command"""
        file_text = await self.read_file(path)
        file_text = file_text.expandtabs()
        new_str = new_str.expandtabs()
        file_text_lines = file_text.split("\n")
        n_lines_file = len(file_text_lines)

        if insert_line < 0 or insert_line > n_lines_file:
            raise ToolError(
                f"Invalid `insert_line` parameter: {insert_line}. It should be within the range of lines of the file: {[0, n_lines_file]}"
            )

        new_str_lines = new_str.split("\n")
        new_file_text_lines = (
            file_text_lines[:insert_line] + new_str_lines + file_text_lines[insert_line:]
        )
        snippet_lines = (
            file_text_lines[max(0, insert_line - SNIPPET_LINES) : insert_line]
            + new_str_lines
            + file_text_lines[insert_line : insert_line + SNIPPET_LINES]
        )

        new_file_text = "\n".join(new_file_text_lines)
        snippet = "\n".join(snippet_lines)

        await self.write_file(path, new_file_text)
        self.edit_history[path].append(file_text)

        success_msg = f"The file {path} has been edited. "
        success_msg += self._make_output(
            snippet, "a snippet of the edited file", max(1, insert_line - SNIPPET_LINES + 1)
        )
        success_msg += "Review the changes and make sure they are as expected (correct indentation, no duplicate lines, etc). Edit the file again if necessary."
        return CLIResult(output=success_msg)

    async def undo_edit(self, path: Path):
        """Implement the undo_edit command"""
        if not self.edit_history[path]:
            raise ToolError(f"No edit history found for {path}.")

        old_text = self.edit_history[path].pop()
        await self.write_file(path, old_text)

        return CLIResult(
            output=f"Last edit to {path} undone successfully. {self._make_output(old_text, str(path))}"
        )

    async def read_file(self, path: Path) -> str:
        """Read the content of a file using cat command."""
        try:
            result = await self.computer.interface.run_command(f'cat "{str(path)}"')
            if result.stderr:  # If there's stderr output
                raise ToolError(f"Error reading file: {result.stderr}")
            return result.stdout
        except Exception as e:
            raise ToolError(f"Failed to read {path}: {str(e)}")

    async def write_file(self, path: Path, content: str):
        """Write content to a file using echo and redirection."""
        try:
            # Create parent directories if they don't exist
            parent = path.parent
            if parent != Path("/"):
                await self.computer.interface.run_command(f'mkdir -p "{str(parent)}"')

            # Write content to file using echo and heredoc to preserve formatting
            cmd = f"""cat > "{str(path)}" << 'EOFCUA'
{content}
EOFCUA"""
            result = await self.computer.interface.run_command(cmd)
            if result.stderr:  # If there's stderr output
                raise ToolError(f"Error writing file: {result.stderr}")
        except Exception as e:
            raise ToolError(f"Failed to write to {path}: {str(e)}")

    def _make_output(
        self,
        file_content: str,
        file_descriptor: str,
        init_line: int = 1,
        expand_tabs: bool = True,
    ) -> str:
        """Generate output for the CLI based on the content of a file."""
        file_content = maybe_truncate(file_content)
        if expand_tabs:
            file_content = file_content.expandtabs()
        file_content = "\n".join(
            [f"{i + init_line:6}\t{line}" for i, line in enumerate(file_content.split("\n"))]
        )
        return (
            f"Here's the result of running `cat -n` on {file_descriptor}:\n" + file_content + "\n"
        )

    def to_params(self) -> Dict[str, Any]:
        """Convert tool to API parameters.

        Returns:
            Dictionary with tool parameters
        """
        return {
            "name": self.name,
            "type": self.api_type,
        }
