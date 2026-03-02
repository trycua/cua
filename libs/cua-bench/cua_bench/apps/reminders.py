"""Apple Reminders app definition for macOS."""

from __future__ import annotations

from typing import TYPE_CHECKING, List, Optional

from .registry import App, launch

if TYPE_CHECKING:
    from .registry import BoundApp


class Reminders(App):
    """Apple Reminders - macOS first-party reminders application.

    Provides getters for retrieving reminders data via AppleScript/osascript.
    Reminders is pre-installed on macOS, so no install method is needed.

    Launch options:
        - None (just opens the app)

    Custom getters (macOS only):
        - get_all_reminders(): Get all reminders
        - get_lists(): Get list of reminder lists
        - get_reminders_in_list(list_name): Get reminders in a specific list
        - get_completed_reminders(): Get completed reminders
        - get_incomplete_reminders(): Get incomplete reminders
    """

    name = "reminders"
    description = "Apple Reminders - macOS reminders application"

    @launch("macos")
    async def launch_macos(self: "BoundApp") -> None:
        """Launch Reminders on macOS."""
        await self.session.run_command('open -a "Reminders"', check=False)

    async def get_all_reminders(self: "BoundApp") -> List[dict]:
        """Get all reminders with their details.

        Returns:
            List of dicts with 'name', 'completed', 'list', 'due_date' keys.
        """
        script = """
tell application "Reminders"
    set reminderList to {}
    repeat with l in lists
        set listName to name of l
        repeat with r in reminders of l
            set reminderName to name of r
            set isCompleted to completed of r
            try
                set dueDate to due date of r as string
            on error
                set dueDate to ""
            end try
            set end of reminderList to {name:reminderName, completed:isCompleted, list:listName, due_date:dueDate}
        end repeat
    end repeat
    return reminderList
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return _parse_reminder_output(result.get("stdout", ""))

    async def get_lists(self: "BoundApp") -> List[str]:
        """Get list of reminder list names.

        Returns:
            List of list names.
        """
        script = """
tell application "Reminders"
    set listNames to {}
    repeat with l in lists
        set end of listNames to name of l
    end repeat
    return listNames
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return _parse_list_output(result.get("stdout", ""))

    async def get_reminders_in_list(self: "BoundApp", *, list_name: str) -> List[dict]:
        """Get all reminders in a specific list.

        Args:
            list_name: Name of the reminder list.

        Returns:
            List of dicts with 'name', 'completed', 'due_date' keys.
        """
        escaped_name = list_name.replace('"', '\\"')
        script = f"""
tell application "Reminders"
    set reminderList to {{}}
    set targetList to list "{escaped_name}"
    repeat with r in reminders of targetList
        set reminderName to name of r
        set isCompleted to completed of r
        try
            set dueDate to due date of r as string
        on error
            set dueDate to ""
        end try
        set end of reminderList to {{name:reminderName, completed:isCompleted, due_date:dueDate}}
    end repeat
    return reminderList
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return _parse_reminder_output(result.get("stdout", ""))

    async def get_completed_reminders(self: "BoundApp") -> List[dict]:
        """Get all completed reminders.

        Returns:
            List of dicts with 'name', 'list' keys.
        """
        script = """
tell application "Reminders"
    set reminderList to {}
    repeat with l in lists
        set listName to name of l
        repeat with r in (reminders of l whose completed is true)
            set end of reminderList to {name:name of r, list:listName}
        end repeat
    end repeat
    return reminderList
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return _parse_reminder_output(result.get("stdout", ""))

    async def get_incomplete_reminders(self: "BoundApp") -> List[dict]:
        """Get all incomplete reminders.

        Returns:
            List of dicts with 'name', 'list', 'due_date' keys.
        """
        script = """
tell application "Reminders"
    set reminderList to {}
    repeat with l in lists
        set listName to name of l
        repeat with r in (reminders of l whose completed is false)
            set reminderName to name of r
            try
                set dueDate to due date of r as string
            on error
                set dueDate to ""
            end try
            set end of reminderList to {name:reminderName, list:listName, due_date:dueDate}
        end repeat
    end repeat
    return reminderList
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return _parse_reminder_output(result.get("stdout", ""))

    async def create_reminder(
        self: "BoundApp",
        *,
        name: str,
        list_name: str = "Reminders",
        due_date: Optional[str] = None,
    ) -> bool:
        """Create a new reminder.

        Args:
            name: Name of the reminder.
            list_name: Name of the list to add to (default "Reminders").
            due_date: Optional due date string (e.g., "January 1, 2025 at 9:00 AM").

        Returns:
            True if created successfully.
        """
        escaped_name = name.replace('"', '\\"')
        escaped_list = list_name.replace('"', '\\"')

        if due_date:
            escaped_due = due_date.replace('"', '\\"')
            script = f"""
tell application "Reminders"
    tell list "{escaped_list}"
        make new reminder with properties {{name:"{escaped_name}", due date:date "{escaped_due}"}}
    end tell
end tell
"""
        else:
            script = f"""
tell application "Reminders"
    tell list "{escaped_list}"
        make new reminder with properties {{name:"{escaped_name}"}}
    end tell
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return result.get("return_code", 1) == 0

    async def complete_reminder(
        self: "BoundApp", *, name: str, list_name: str = "Reminders"
    ) -> bool:
        """Mark a reminder as completed.

        Args:
            name: Name of the reminder.
            list_name: Name of the list containing the reminder.

        Returns:
            True if completed successfully.
        """
        escaped_name = name.replace('"', '\\"')
        escaped_list = list_name.replace('"', '\\"')
        script = f"""
tell application "Reminders"
    tell list "{escaped_list}"
        set completed of (first reminder whose name is "{escaped_name}") to true
    end tell
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return result.get("return_code", 1) == 0


def _parse_list_output(output: str) -> List[str]:
    """Parse AppleScript list output."""
    if not output.strip():
        return []
    items = [item.strip() for item in output.strip().split(",")]
    return [item for item in items if item]


def _parse_reminder_output(output: str) -> List[dict]:
    """Parse AppleScript reminder records output."""
    if not output.strip():
        return []

    records = []
    output = output.strip()

    # Handle list of records: {{...}, {...}}
    if output.startswith("{{"):
        output = output[1:-1]
        parts = output.split("}, {")
        for i, part in enumerate(parts):
            if i == 0:
                part = part[1:]
            if i == len(parts) - 1:
                part = part[:-1]
            record = _parse_single_record(part)
            if record:
                records.append(record)
    elif output.startswith("{"):
        record = _parse_single_record(output[1:-1])
        if record:
            records.append(record)

    return records


def _parse_single_record(record_str: str) -> Optional[dict]:
    """Parse a single AppleScript record string."""
    if not record_str:
        return None

    result = {}
    parts = []
    current = ""
    in_quotes = False

    for char in record_str:
        if char == '"' and (not current or current[-1] != "\\"):
            in_quotes = not in_quotes
        elif char == "," and not in_quotes:
            parts.append(current.strip())
            current = ""
            continue
        current += char

    if current.strip():
        parts.append(current.strip())

    for part in parts:
        if ":" in part:
            key, value = part.split(":", 1)
            key = key.strip()
            value = value.strip()
            # Handle boolean values
            if value == "true":
                result[key] = True
            elif value == "false":
                result[key] = False
            else:
                result[key] = value.strip('"')

    return result if result else None
