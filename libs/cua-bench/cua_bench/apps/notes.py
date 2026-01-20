"""Apple Notes app definition for macOS."""

from __future__ import annotations

from typing import TYPE_CHECKING, List, Optional

from .registry import App, launch

if TYPE_CHECKING:
    from .registry import BoundApp


class Notes(App):
    """Apple Notes - macOS first-party notes application.

    Provides getters for retrieving notes data via AppleScript/osascript.
    Notes is pre-installed on macOS, so no install method is needed.

    Launch options:
        - None (just opens the app)

    Custom getters (macOS only):
        - get_all_notes(): Get all notes with titles and bodies
        - get_note_by_title(title): Get a specific note by title
        - get_folders(): Get list of note folders
        - get_notes_in_folder(folder): Get notes in a specific folder
    """

    name = "notes"
    description = "Apple Notes - macOS notes application"

    @launch("macos")
    async def launch_macos(self: "BoundApp") -> None:
        """Launch Notes on macOS."""
        await self.session.run_command('open -a "Notes"', check=False)

    async def get_all_notes(self: "BoundApp") -> List[dict]:
        """Get all notes with their titles and bodies.

        Returns:
            List of dicts with 'title', 'body', and 'folder' keys.
        """
        script = """
tell application "Notes"
    set noteList to {}
    repeat with f in folders
        set folderName to name of f
        repeat with n in notes of f
            set noteTitle to name of n
            set noteBody to plaintext of n
            set end of noteList to {title:noteTitle, body:noteBody, folder:folderName}
        end repeat
    end repeat
    return noteList
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return _parse_applescript_records(result.get("stdout", ""))

    async def get_note_by_title(self: "BoundApp", *, title: str) -> Optional[dict]:
        """Get a specific note by its title.

        Args:
            title: The title of the note to find.

        Returns:
            Dict with 'title', 'body', 'folder' or None if not found.
        """
        # Escape single quotes in title
        escaped_title = title.replace("'", "'\\''")
        script = f"""
tell application "Notes"
    repeat with f in folders
        set folderName to name of f
        repeat with n in notes of f
            if name of n is "{escaped_title}" then
                return {{title:name of n, body:plaintext of n, folder:folderName}}
            end if
        end repeat
    end repeat
    return missing value
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        stdout = result.get("stdout", "").strip()
        if not stdout or stdout == "missing value":
            return None
        records = _parse_applescript_records(stdout)
        return records[0] if records else None

    async def get_folders(self: "BoundApp") -> List[str]:
        """Get list of note folder names.

        Returns:
            List of folder names.
        """
        script = """
tell application "Notes"
    set folderNames to {}
    repeat with f in folders
        set end of folderNames to name of f
    end repeat
    return folderNames
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return _parse_applescript_list(result.get("stdout", ""))

    async def get_notes_in_folder(self: "BoundApp", *, folder: str) -> List[dict]:
        """Get all notes in a specific folder.

        Args:
            folder: Name of the folder.

        Returns:
            List of dicts with 'title' and 'body' keys.
        """
        escaped_folder = folder.replace("'", "'\\''")
        script = f"""
tell application "Notes"
    set noteList to {{}}
    set targetFolder to folder "{escaped_folder}"
    repeat with n in notes of targetFolder
        set end of noteList to {{title:name of n, body:plaintext of n}}
    end repeat
    return noteList
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return _parse_applescript_records(result.get("stdout", ""))

    async def create_note(
        self: "BoundApp",
        *,
        title: str,
        body: str,
        folder: str = "Notes",
    ) -> bool:
        """Create a new note.

        Args:
            title: Title of the note.
            body: Body content of the note.
            folder: Folder to create the note in (default "Notes").

        Returns:
            True if created successfully.
        """
        escaped_title = title.replace('"', '\\"')
        escaped_body = body.replace('"', '\\"')
        escaped_folder = folder.replace('"', '\\"')
        script = f"""
tell application "Notes"
    tell folder "{escaped_folder}"
        make new note with properties {{name:"{escaped_title}", body:"{escaped_body}"}}
    end tell
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return result.get("return_code", 1) == 0


def _parse_applescript_list(output: str) -> List[str]:
    """Parse AppleScript list output like 'item1, item2, item3'."""
    if not output.strip():
        return []
    # AppleScript returns comma-separated values
    items = [item.strip() for item in output.strip().split(",")]
    return [item for item in items if item]


def _parse_applescript_records(output: str) -> List[dict]:
    """Parse AppleScript record output.

    AppleScript records look like: {title:"foo", body:"bar"}
    """
    if not output.strip():
        return []

    records = []
    # Simple parsing - AppleScript returns records in a specific format
    # For complex parsing, we'd need a proper parser
    # This handles the basic case of {key:value, key:value}
    output = output.strip()

    # Handle list of records: {{...}, {...}}
    if output.startswith("{{"):
        # Remove outer braces
        output = output[1:-1]
        # Split on "}, {" to get individual records
        parts = output.split("}, {")
        for i, part in enumerate(parts):
            if i == 0:
                part = part[1:]  # Remove leading {
            if i == len(parts) - 1:
                part = part[:-1]  # Remove trailing }
            record = _parse_single_record(part)
            if record:
                records.append(record)
    elif output.startswith("{"):
        # Single record
        record = _parse_single_record(output[1:-1])
        if record:
            records.append(record)

    return records


def _parse_single_record(record_str: str) -> Optional[dict]:
    """Parse a single AppleScript record string."""
    if not record_str:
        return None

    result = {}
    # Split on comma, but be careful of commas in values
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
            value = value.strip().strip('"')
            result[key] = value

    return result if result else None
