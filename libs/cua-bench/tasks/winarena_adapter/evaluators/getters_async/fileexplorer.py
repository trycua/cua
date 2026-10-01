"""Async File Explorer getters."""

import logging
from typing import Any, Dict, Optional

from .base import run_powershell, run_python

logger = logging.getLogger("winarena.getters_async.fileexplorer")


async def get_vm_folder_exists_in_documents(session, config: Dict[str, Any]) -> bool:
    """Check if a folder exists in Documents.

    Config:
        folder_name (str): Name of folder to check

    Returns:
        True if folder exists
    """
    folder_name = config.get("folder_name")
    if not folder_name:
        return False

    result = await run_powershell(
        session, f'Test-Path -Path "$env:USERPROFILE\\Documents\\{folder_name}"'
    )

    return result["stdout"].strip().lower() == "true"


async def get_vm_file_exists_in_desktop(session, config: Dict[str, Any]) -> bool:
    """Check if a file exists on the Desktop.

    Config:
        file_name (str): Name of file to check

    Returns:
        True if file exists
    """
    file_name = config.get("file_name")
    if not file_name:
        return False

    result = await run_powershell(
        session, f'Test-Path -Path "$env:USERPROFILE\\Desktop\\{file_name}"'
    )

    return result["stdout"].strip().lower() == "true"


async def get_vm_active_window_title(session, config: Dict[str, Any]) -> Optional[str]:
    """Get the title of the currently active window.

    Returns:
        Window title or None
    """
    try:
        window_id = await session.get_current_window_id()
        if window_id:
            name = await session.get_window_name(window_id)
            return name
    except Exception as e:
        logger.error(f"Failed to get active window title: {e}")

    return None


async def get_is_file_hidden(session, config: Dict[str, Any]) -> int:
    """Check if a file is hidden.

    Config:
        path (str): Path to the file

    Returns:
        1 if hidden, 0 otherwise
    """
    path = config.get("path")
    if not path:
        return 0

    result = await run_powershell(
        session,
        f"(Get-Item -Path '{path}' -Force).Attributes -band [System.IO.FileAttributes]::Hidden",
    )

    # If result is non-empty and not "0", file is hidden
    stdout = result["stdout"].strip()
    if stdout and stdout != "0" and stdout.lower() != "false":
        return 1
    return 0


async def get_are_files_sorted_by_modified_time(session, config: Dict[str, Any]) -> bool:
    """Check if files in directory are sorted by modified time (descending).

    Config:
        directory (str): Directory path

    Returns:
        True if sorted correctly
    """
    directory = config.get("directory")
    if not directory:
        return False

    result = await run_python(
        session,
        f"""
import os
files = [(f, os.path.getmtime(os.path.join(r'{directory}', f)))
         for f in os.listdir(r'{directory}')
         if os.path.isfile(os.path.join(r'{directory}', f))]
sorted_files = sorted(files, key=lambda x: x[1], reverse=True)
print('true' if files == sorted_files else 'false')
""",
    )

    return result["stdout"].strip().lower() == "true"


async def get_is_details_view(session, config: Dict[str, Any]) -> bool:
    """Check if File Explorer is in details view.

    Note: This requires UI automation to properly detect.

    Returns:
        True if in details view (simplified check)
    """
    logger.warning("get_is_details_view not fully implemented for async")
    return False


async def get_all_png_file_names(session, config: Dict[str, Any]) -> list:
    """Get all PNG filenames in a directory.

    Config:
        directory (str): Directory path

    Returns:
        List of PNG filenames
    """
    directory = config.get("directory")
    if not directory:
        return []

    result = await run_powershell(
        session,
        f"Get-ChildItem -Path '{directory}' -Filter '*.png' | Select-Object -ExpandProperty Name",
    )

    if result["exit_code"] == 0 and result["stdout"]:
        return result["stdout"].strip().split("\n")
    return []


async def get_zipped_folder_in_desktop(session, config: Dict[str, Any]) -> Optional[str]:
    """Get the name of a zip file on the desktop.

    Returns:
        Name of first zip file found, or None
    """
    result = await run_powershell(
        session,
        "Get-ChildItem -Path \"$env:USERPROFILE\\Desktop\" -Filter '*.zip' | Select-Object -First 1 -ExpandProperty Name",
    )

    if result["exit_code"] == 0 and result["stdout"]:
        return result["stdout"].strip()
    return None


async def get_is_all_docx_in_archive(session, config: Dict[str, Any]) -> bool:
    """Check if all .docx files are in a zip archive.

    Config:
        directory (str): Directory to check
        archive_name (str): Name of zip file

    Returns:
        True if all .docx files are in the archive
    """
    directory = config.get("directory")
    archive_name = config.get("archive_name")

    if not directory or not archive_name:
        return False

    result = await run_powershell(
        session,
        f"""
$docxFiles = Get-ChildItem -Path '{directory}' -Filter '*.docx' | Select-Object -ExpandProperty Name
$archivePath = Join-Path '{directory}' '{archive_name}'
$archiveContents = (Get-ChildItem -Path $archivePath -ErrorAction SilentlyContinue | Expand-Archive -DestinationPath $env:TEMP -PassThru -Force | Select-Object -ExpandProperty Name)
$allInArchive = $true
foreach ($file in $docxFiles) {{
    if ($archiveContents -notcontains $file) {{
        $allInArchive = $false
        break
    }}
}}
Write-Output $allInArchive
""",
    )

    return result["stdout"].strip().lower() == "true"


async def get_is_directory_read_only_for_user(session, config: Dict[str, Any]) -> bool:
    """Check if directory is read-only for a user.

    Config:
        directory (str): Directory path
        user (str): Username

    Returns:
        True if read-only
    """
    directory = config.get("directory")
    user = config.get("user")

    if not directory or not user:
        return False

    result = await run_powershell(
        session,
        f"(Get-Acl '{directory}').Access | Where-Object {{ $_.IdentityReference -like '*{user}*' }} | Select-Object -ExpandProperty FileSystemRights",
    )

    if result["exit_code"] == 0:
        rights = result["stdout"].strip().lower()
        return "write" not in rights and "read" in rights

    return False


async def get_are_all_images_tagged(session, config: Dict[str, Any]) -> bool:
    """Check if all images in directory have tags.

    Config:
        directory (str): Directory path

    Returns:
        True if all images are tagged
    """
    logger.warning("get_are_all_images_tagged not fully implemented for async")
    return False


async def get_is_file_desktop(session, config: Dict[str, Any]) -> bool:
    """Check if a file is on the desktop.

    Config:
        file_name (str): Name of file

    Returns:
        True if on desktop
    """
    return await get_vm_file_exists_in_desktop(session, config)


async def get_vm_library_folders(session, config: Dict[str, Any]) -> Optional[list]:
    """Get Windows library folders.

    Config:
        library_name (str): Library GUID or name

    Returns:
        List of folder paths in library
    """
    library_name = config.get("library_name")
    if not library_name:
        return None

    result = await run_powershell(
        session,
        f"""
$shell = New-Object -ComObject Shell.Application
$library = $shell.NameSpace('shell:::{{{library_name}}}')
if ($library) {{
    $library.Items() | ForEach-Object {{ $_.Path }}
}}
""",
    )

    if result["exit_code"] == 0 and result["stdout"]:
        return result["stdout"].strip().split("\n")
    return None


async def get_is_files_moved_downloads(session, config: Dict[str, Any]) -> bool:
    """Check if files were moved to Downloads.

    Config:
        file_names (list): List of file names to check

    Returns:
        True if all files are in Downloads
    """
    file_names = config.get("file_names", [])
    if not file_names:
        return False

    for file_name in file_names:
        result = await run_powershell(
            session, f'Test-Path -Path "$env:USERPROFILE\\Downloads\\{file_name}"'
        )
        if result["stdout"].strip().lower() != "true":
            return False

    return True


async def get_is_file_saved_desktop(session, config: Dict[str, Any]) -> bool:
    """Check if file is saved to desktop.

    Config:
        file_name (str): Name of file

    Returns:
        True if file exists on desktop
    """
    return await get_vm_file_exists_in_desktop(session, config)
