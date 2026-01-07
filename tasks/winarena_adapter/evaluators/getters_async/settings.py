"""Async Windows Settings getters."""

import json
import logging
from typing import Any, Dict, List, Optional, Union

from .base import get_registry_value, get_registry_binary, run_powershell

logger = logging.getLogger("winarena.getters_async.settings")


def _generate_time(hour: int, minute: int) -> str:
    """Format hour/minute to AM/PM string."""
    if hour < 12:
        am_pm = "AM"
    else:
        am_pm = "PM"
        hour -= 12
    return f"{hour:02d}:{minute:02d} {am_pm}"


async def get_night_light_state(session, config: Dict[str, Any]) -> Union[bool, List]:
    """Check if Windows Night Light is enabled and get schedule.

    Returns:
        False if disabled, True or [True, start_time, end_time] if enabled
    """
    key_path = r"HKCU:\Software\Microsoft\Windows\CurrentVersion\CloudStore\Store\DefaultAccount\Current\default$windows.data.bluelightreduction.settings\windows.data.bluelightreduction.settings"
    value_name = "Data"

    binary_data = await get_registry_binary(session, key_path, value_name)

    if binary_data is None or len(binary_data) < 25:
        return False

    # Decode the registry binary data
    # Byte 24 == 1 means night light is enabled
    if binary_data[24] != 1:
        return False

    # Byte 28 != 202 means schedule is set
    if len(binary_data) > 28 and binary_data[28] != 202:
        starting_hour = binary_data[28]

        # Check if minutes are set (marker value 46)
        if len(binary_data) > 30:
            starting_minute = binary_data[30] if binary_data[29] != 0 else 0
            ending_index = 33 if starting_minute == 0 else 35

            if len(binary_data) > ending_index + 2:
                ending_hour = binary_data[ending_index]
                ending_minute = binary_data[ending_index + 2] if binary_data[ending_index + 1] != 0 else 0

                start = _generate_time(starting_hour, starting_minute)
                end = _generate_time(ending_hour, ending_minute)
                return [True, start, end]

    return [True]


async def get_default_browser(session, config: Dict[str, Any]) -> str:
    """Get the default browser from registry.

    Returns:
        Browser ProgId (e.g., 'ChromeHTML', 'MSEdgeHTM') or 'Error'
    """
    key_path = r"HKCU:Software\Microsoft\Windows\Shell\Associations\UrlAssociations\http\UserChoice"
    value_name = "ProgId"

    value = await get_registry_value(session, key_path, value_name)

    if value:
        return value.split("\n")[0]
    return "Error"


async def get_storage_sense_run_frequency(session, config: Dict[str, Any]) -> Optional[str]:
    """Get Storage Sense run frequency.

    Returns:
        Frequency value (e.g., '1' for daily) or None if disabled
    """
    key_path = r"HKCU:\Software\Microsoft\Windows\CurrentVersion\StorageSense\Parameters\StoragePolicy"

    # Check if Storage Sense is enabled
    enabled = await get_registry_value(session, key_path, "01")
    if enabled != "1":
        return None

    # Get frequency
    frequency = await get_registry_value(session, key_path, "2048")
    if frequency:
        return frequency.split("\n")[0]

    return None


async def get_active_hours_of_user_to_not_interrupt_for_windows_updates(
    session, config: Dict[str, Any]
) -> Optional[List[str]]:
    """Get Windows Update active hours.

    Returns:
        [start_time, end_time] formatted as "HH:MM AM/PM" or None
    """
    key_path = r"HKLM:\SOFTWARE\Microsoft\WindowsUpdate\UX\Settings"

    start_hour = await get_registry_value(session, key_path, "ActiveHoursStart")
    end_hour = await get_registry_value(session, key_path, "ActiveHoursEnd")

    if start_hour and end_hour:
        try:
            start_formatted = _generate_time(int(start_hour.split("\n")[0]), 0)
            end_formatted = _generate_time(int(end_hour.split("\n")[0]), 0)
            return [start_formatted, end_formatted]
        except (ValueError, IndexError):
            pass

    return None


async def get_system_timezone(session, config: Dict[str, Any]) -> str:
    """Get the system timezone.

    Returns:
        Timezone display name (e.g., "(UTC-08:00) Pacific Time (US & Canada)")
    """
    result = await run_powershell(
        session,
        "Get-TimeZone | Select-Object -Property DisplayName, StandardName | ConvertTo-Json"
    )

    if result["exit_code"] == 0 and result["stdout"]:
        try:
            data = json.loads(result["stdout"])
            display_name = data.get("DisplayName")
            if display_name:
                return display_name
        except json.JSONDecodeError:
            pass

    return "Unable to get time zone"


async def get_desktop_background(session, config: Dict[str, Any]) -> str:
    """Check if desktop background is set.

    Returns:
        'True' if no wallpaper set (solid color), 'False' if wallpaper set
    """
    key_path = r"HKCU:Control Panel\Desktop"
    value_name = "WallPaper"

    value = await get_registry_value(session, key_path, value_name)

    if value is None or value.strip() == "":
        return "True"
    return "False"


async def get_system_notifications(session, config: Dict[str, Any]) -> str:
    """Check if system notifications are enabled.

    Returns:
        'True' if notifications are disabled, 'False' if enabled
    """
    key_path = r"HKCU:Software\Microsoft\Windows\CurrentVersion\PushNotifications"
    value_name = "ToastEnabled"

    value = await get_registry_value(session, key_path, value_name)

    if value is None or value.strip() == "":
        # Key doesn't exist, assume notifications are enabled
        return "False"

    if value.strip() == "0":
        # ToastEnabled = 0 means disabled
        return "True"

    return "False"
