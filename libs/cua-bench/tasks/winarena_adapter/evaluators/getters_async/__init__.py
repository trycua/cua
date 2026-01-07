"""Async getters for WAA tasks.

These getters run in the worker container and communicate with the VM
via session.* methods (HTTP calls to cua-computer-server).

All getters are async functions with signature:
    async def get_xxx(session, config: Dict[str, Any]) -> Any

Usage:
    from .getters_async import get_vm_file, get_async_getter

    # Direct import
    result = await get_vm_file(session, {"path": "...", "dest": "..."})

    # Or via lookup
    getter = get_async_getter("vm_file")
    result = await getter(session, config)
"""

# File operations
from .file import (
    get_cloud_file,
    get_vm_file,
    get_cache_file,
    get_content_from_vm_file,
    get_vm_file_exists_in_vm_folder,
)

# General command execution
from .general import (
    get_vm_command_line,
    get_vm_command_error,
    get_vm_terminal_output,
)

# Registry operations
from .registry import (
    get_registry_key,
    get_registry_key_binary,
)

# Windows Settings
from .settings import (
    get_night_light_state,
    get_default_browser,
    get_storage_sense_run_frequency,
    get_active_hours_of_user_to_not_interrupt_for_windows_updates,
    get_system_timezone,
    get_desktop_background,
    get_system_notifications,
)

# Misc
from .misc import (
    get_rule,
    get_accessibility_tree,
    get_rule_relativeTime,
    get_time_diff_range,
)

# Chrome browser
from .chrome import (
    get_enable_do_not_track,
    get_bookmarks,
    get_history,
    get_cookie_data,
    get_open_tabs_info,
    get_default_search_engine,
)

# File Explorer
from .fileexplorer import (
    get_vm_folder_exists_in_documents,
    get_vm_file_exists_in_desktop,
    get_vm_active_window_title,
    get_is_file_hidden,
    get_are_files_sorted_by_modified_time,
    get_is_details_view,
    get_all_png_file_names,
    get_zipped_folder_in_desktop,
    get_is_all_docx_in_archive,
    get_is_directory_read_only_for_user,
    get_are_all_images_tagged,
    get_is_file_desktop,
    get_vm_library_folders,
    get_is_files_moved_downloads,
    get_is_file_saved_desktop,
)

# Info getters
from .info import (
    get_vm_screen_size,
    get_vm_window_size,
    get_vm_wallpaper,
    get_list_directory,
)


# Mapping from getter type names to async functions
# This allows the evaluator to look up getters by name
ASYNC_GETTERS = {
    # File
    "cloud_file": get_cloud_file,
    "vm_file": get_vm_file,
    "cache_file": get_cache_file,
    "content_from_vm_file": get_content_from_vm_file,
    "vm_file_exists_in_vm_folder": get_vm_file_exists_in_vm_folder,
    # General
    "vm_command_line": get_vm_command_line,
    "vm_command_error": get_vm_command_error,
    "vm_terminal_output": get_vm_terminal_output,
    # Registry
    "registry_key": get_registry_key,
    "registry_key_binary": get_registry_key_binary,
    # Settings
    "night_light_state": get_night_light_state,
    "default_browser": get_default_browser,
    "storage_sense_run_frequency": get_storage_sense_run_frequency,
    "active_hours_of_user_to_not_interrupt_for_windows_updates": get_active_hours_of_user_to_not_interrupt_for_windows_updates,
    "system_timezone": get_system_timezone,
    "desktop_background": get_desktop_background,
    "system_notifications": get_system_notifications,
    # Misc
    "rule": get_rule,
    "accessibility_tree": get_accessibility_tree,
    "rule_relativeTime": get_rule_relativeTime,
    "time_diff_range": get_time_diff_range,
    # Chrome
    "enable_do_not_track": get_enable_do_not_track,
    "bookmarks": get_bookmarks,
    "history": get_history,
    "cookie_data": get_cookie_data,
    "open_tabs_info": get_open_tabs_info,
    "default_search_engine": get_default_search_engine,
    # File Explorer
    "vm_folder_exists_in_documents": get_vm_folder_exists_in_documents,
    "vm_file_exists_in_desktop": get_vm_file_exists_in_desktop,
    "vm_active_window_title": get_vm_active_window_title,
    "is_file_hidden": get_is_file_hidden,
    "are_files_sorted_by_modified_time": get_are_files_sorted_by_modified_time,
    "is_details_view": get_is_details_view,
    "all_png_file_names": get_all_png_file_names,
    "zipped_folder_in_desktop": get_zipped_folder_in_desktop,
    "is_all_docx_in_archive": get_is_all_docx_in_archive,
    "is_directory_read_only_for_user": get_is_directory_read_only_for_user,
    "are_all_images_tagged": get_are_all_images_tagged,
    "is_file_desktop": get_is_file_desktop,
    "vm_library_folders": get_vm_library_folders,
    "is_files_moved_downloads": get_is_files_moved_downloads,
    "is_file_saved_desktop": get_is_file_saved_desktop,
    # Info
    "vm_screen_size": get_vm_screen_size,
    "vm_window_size": get_vm_window_size,
    "vm_wallpaper": get_vm_wallpaper,
    "list_directory": get_list_directory,
}


def get_async_getter(getter_type: str):
    """Get an async getter function by type name.

    Args:
        getter_type: The getter type (e.g., 'vm_file', 'enable_do_not_track')

    Returns:
        The async getter function, or None if not found
    """
    return ASYNC_GETTERS.get(getter_type)


__all__ = [
    # Lookup function
    "get_async_getter",
    "ASYNC_GETTERS",
    # File
    "get_cloud_file",
    "get_vm_file",
    "get_cache_file",
    "get_content_from_vm_file",
    "get_vm_file_exists_in_vm_folder",
    # General
    "get_vm_command_line",
    "get_vm_command_error",
    "get_vm_terminal_output",
    # Registry
    "get_registry_key",
    "get_registry_key_binary",
    # Settings
    "get_night_light_state",
    "get_default_browser",
    "get_storage_sense_run_frequency",
    "get_active_hours_of_user_to_not_interrupt_for_windows_updates",
    "get_system_timezone",
    "get_desktop_background",
    "get_system_notifications",
    # Misc
    "get_rule",
    "get_accessibility_tree",
    "get_rule_relativeTime",
    "get_time_diff_range",
    # Chrome
    "get_enable_do_not_track",
    "get_bookmarks",
    "get_history",
    "get_cookie_data",
    "get_open_tabs_info",
    "get_default_search_engine",
    # File Explorer
    "get_vm_folder_exists_in_documents",
    "get_vm_file_exists_in_desktop",
    "get_vm_active_window_title",
    "get_is_file_hidden",
    "get_are_files_sorted_by_modified_time",
    "get_is_details_view",
    "get_all_png_file_names",
    "get_zipped_folder_in_desktop",
    "get_is_all_docx_in_archive",
    "get_is_directory_read_only_for_user",
    "get_are_all_images_tagged",
    "get_is_file_desktop",
    "get_vm_library_folders",
    "get_is_files_moved_downloads",
    "get_is_file_saved_desktop",
    # Info
    "get_vm_screen_size",
    "get_vm_window_size",
    "get_vm_wallpaper",
    "get_list_directory",
]
