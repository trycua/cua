"""Apple Calendar app definition for macOS."""

from __future__ import annotations

from typing import TYPE_CHECKING, List, Optional

from .registry import App, launch

if TYPE_CHECKING:
    from .registry import BoundApp


class Calendar(App):
    """Apple Calendar - macOS first-party calendar application.

    Provides getters for retrieving calendar data via AppleScript/osascript.
    Calendar is pre-installed on macOS, so no install method is needed.

    Launch options:
        - None (just opens the app)

    Custom getters (macOS only):
        - get_calendars(): Get list of calendar names
        - get_events_today(): Get events for today
        - get_events_in_calendar(calendar): Get events in a specific calendar
        - get_event_by_title(title): Get a specific event by title
    """

    name = "calendar"
    description = "Apple Calendar - macOS calendar application"

    @launch("macos")
    async def launch_macos(self: "BoundApp") -> None:
        """Launch Calendar on macOS."""
        await self.session.run_command('open -a "Calendar"', check=False)

    async def get_calendars(self: "BoundApp") -> List[str]:
        """Get list of calendar names.

        Returns:
            List of calendar names.
        """
        script = """
tell application "Calendar"
    set calNames to {}
    repeat with c in calendars
        set end of calNames to name of c
    end repeat
    return calNames
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return _parse_list_output(result.get("stdout", ""))

    async def get_events_today(self: "BoundApp") -> List[dict]:
        """Get all events for today.

        Returns:
            List of dicts with 'summary', 'start_date', 'end_date', 'calendar' keys.
        """
        script = """
tell application "Calendar"
    set todayStart to current date
    set hours of todayStart to 0
    set minutes of todayStart to 0
    set seconds of todayStart to 0
    set todayEnd to todayStart + (1 * days)

    set eventList to {}
    repeat with c in calendars
        set calName to name of c
        set todayEvents to (events of c whose start date >= todayStart and start date < todayEnd)
        repeat with e in todayEvents
            set eventSummary to summary of e
            set eventStart to start date of e as string
            set eventEnd to end date of e as string
            set end of eventList to {summary:eventSummary, start_date:eventStart, end_date:eventEnd, calendar:calName}
        end repeat
    end repeat
    return eventList
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return _parse_event_output(result.get("stdout", ""))

    async def get_events_in_calendar(self: "BoundApp", *, calendar_name: str) -> List[dict]:
        """Get all events in a specific calendar.

        Args:
            calendar_name: Name of the calendar.

        Returns:
            List of dicts with 'summary', 'start_date', 'end_date' keys.
        """
        escaped_name = calendar_name.replace('"', '\\"')
        script = f"""
tell application "Calendar"
    set eventList to {{}}
    set targetCal to calendar "{escaped_name}"
    repeat with e in events of targetCal
        set eventSummary to summary of e
        set eventStart to start date of e as string
        set eventEnd to end date of e as string
        set end of eventList to {{summary:eventSummary, start_date:eventStart, end_date:eventEnd}}
    end repeat
    return eventList
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return _parse_event_output(result.get("stdout", ""))

    async def get_event_by_title(self: "BoundApp", *, title: str) -> Optional[dict]:
        """Get a specific event by its title/summary.

        Args:
            title: The title/summary of the event to find.

        Returns:
            Dict with 'summary', 'start_date', 'end_date', 'calendar' or None if not found.
        """
        escaped_title = title.replace('"', '\\"')
        script = f"""
tell application "Calendar"
    repeat with c in calendars
        set calName to name of c
        set matchingEvents to (events of c whose summary is "{escaped_title}")
        if (count of matchingEvents) > 0 then
            set e to item 1 of matchingEvents
            return {{summary:summary of e, start_date:(start date of e as string), end_date:(end date of e as string), calendar:calName}}
        end if
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
        records = _parse_event_output(stdout)
        return records[0] if records else None

    async def get_events_in_range(
        self: "BoundApp",
        *,
        start_date: str,
        end_date: str,
    ) -> List[dict]:
        """Get events within a date range.

        Args:
            start_date: Start date string (e.g., "January 1, 2025").
            end_date: End date string (e.g., "January 31, 2025").

        Returns:
            List of dicts with 'summary', 'start_date', 'end_date', 'calendar' keys.
        """
        escaped_start = start_date.replace('"', '\\"')
        escaped_end = end_date.replace('"', '\\"')
        script = f"""
tell application "Calendar"
    set rangeStart to date "{escaped_start}"
    set rangeEnd to date "{escaped_end}"

    set eventList to {{}}
    repeat with c in calendars
        set calName to name of c
        set rangeEvents to (events of c whose start date >= rangeStart and start date <= rangeEnd)
        repeat with e in rangeEvents
            set eventSummary to summary of e
            set eventStart to start date of e as string
            set eventEnd to end date of e as string
            set end of eventList to {{summary:eventSummary, start_date:eventStart, end_date:eventEnd, calendar:calName}}
        end repeat
    end repeat
    return eventList
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return _parse_event_output(result.get("stdout", ""))

    async def create_event(
        self: "BoundApp",
        *,
        summary: str,
        start_date: str,
        end_date: str,
        calendar_name: str = "Calendar",
    ) -> bool:
        """Create a new calendar event.

        Args:
            summary: Title of the event.
            start_date: Start date/time string (e.g., "January 15, 2025 at 2:00 PM").
            end_date: End date/time string (e.g., "January 15, 2025 at 3:00 PM").
            calendar_name: Name of the calendar to add to.

        Returns:
            True if created successfully.
        """
        escaped_summary = summary.replace('"', '\\"')
        escaped_start = start_date.replace('"', '\\"')
        escaped_end = end_date.replace('"', '\\"')
        escaped_cal = calendar_name.replace('"', '\\"')
        script = f"""
tell application "Calendar"
    tell calendar "{escaped_cal}"
        make new event with properties {{summary:"{escaped_summary}", start date:date "{escaped_start}", end date:date "{escaped_end}"}}
    end tell
end tell
"""
        result = await self.session.run_command(
            f"osascript -e '{script}'",
            check=False,
        )
        return result.get("return_code", 1) == 0

    async def delete_event(
        self: "BoundApp", *, title: str, calendar_name: Optional[str] = None
    ) -> bool:
        """Delete an event by title.

        Args:
            title: Title of the event to delete.
            calendar_name: Optional calendar name (searches all if not specified).

        Returns:
            True if deleted successfully.
        """
        escaped_title = title.replace('"', '\\"')
        if calendar_name:
            escaped_cal = calendar_name.replace('"', '\\"')
            script = f"""
tell application "Calendar"
    tell calendar "{escaped_cal}"
        delete (first event whose summary is "{escaped_title}")
    end tell
end tell
"""
        else:
            script = f"""
tell application "Calendar"
    repeat with c in calendars
        try
            delete (first event of c whose summary is "{escaped_title}")
            return true
        end try
    end repeat
    return false
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


def _parse_event_output(output: str) -> List[dict]:
    """Parse AppleScript event records output."""
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
            value = value.strip().strip('"')
            result[key] = value

    return result if result else None
