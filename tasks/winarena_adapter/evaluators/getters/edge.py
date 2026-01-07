import json
import logging
import platform
from typing import Dict
import os
import platform
import sqlite3
import time
from urllib.parse import unquote
from typing import Dict, Any, List
from urllib.parse import urlparse, parse_qs

import lxml.etree
import requests
from lxml.cssselect import CSSSelector
from lxml.etree import _Element
from playwright.sync_api import sync_playwright, expect
from pydrive.auth import GoogleAuth
from pydrive.drive import GoogleDrive, GoogleDriveFileList, GoogleDriveFile

import LnkParse3
from io import BytesIO, StringIO
from contextlib import redirect_stdout

logger = logging.getLogger("desktopenv.getters.edge")

def get_url_shortcuts_on_desktop(env, config: Dict[str, str]):
    # Find out the operating system
    os_name = env.vm_platform

    # Depending on the OS, define the shortcut file extension
    if os_name == 'Windows':
        # Windows shortcuts are typically .url or .lnk files
        shortcut_extension = '.url'
    else:
        logger.error(f"Unsupported operating system: {os_name}")
        return []

    # Get the path to the desktop folder
    desktop_path = env.controller.get_vm_desktop_path()
    desktop_directory_tree = env.controller.get_vm_directory_tree(desktop_path)

    shortcuts_paths = [file['name'] for file in desktop_directory_tree['children'] if
                       file['name'].endswith(shortcut_extension)]

    short_cuts = {}

    for shortcut_name in shortcuts_paths:
        shortcut_path = os.path.join(desktop_path, shortcut_name)
        shortcut_data = env.controller.get_file(shortcut_path)
        short_cuts[shortcut_name] = shortcut_data.decode('utf-8')

    return short_cuts

def get_data_delete_automacally_from_edge(env, config: Dict[str, str]):
    """
    This function is used to open th "auto-delete" mode of chromium
    """
    os_type = env.vm_platform
    if os_type == 'Windows':
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Microsoft/Edge/User Data/Default/Preferences'))"
        )['output'].strip()
    else:
        raise Exception('Unsupported operating system')

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)
        data_delete_state = data["browser"].get("clear_data_on_exit", None)
        logger.info(f"data_delete_state: {data_delete_state}")
        target_state = {'browsing_history': True, 'cache': True, 'cookies': True, 'download_history': True, 'form_data': True, 'hosted_apps_data': False, 'passwords': True, 'site_settings': True}
        return 1.0 if data_delete_state == target_state else 0.0
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0.0

def get_default_search_engine_from_edge(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == 'Windows':
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Microsoft/Edge/User Data/Default/Preferences'))"
        )['output'].strip()
    else:
        raise Exception('Unsupported operating system')
    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)
        engine_guids = {
            '485bf7d3-0215-45af-87dc-538868000001': 'Bing',
            '485bf7d3-0215-45af-87dc-538868000002': 'Yahoo!',
            '485bf7d3-0215-45af-87dc-538868000003': 'Google',
            '485bf7d3-0215-45af-87dc-538868000092': 'DuckDuckGo',
            '485bf7d3-0215-45af-87dc-538868000015': 'Yandex'
        }
        cur_guid = data.get('default_search_provider', {}).get('synced_guid')
        logger.info(f"Current search engine guid: {cur_guid}")
        
        search_engine = engine_guids.get(cur_guid, {})
        if not search_engine: # fallback
            search_engine = "Unknown"
        logger.info(f"Current search engine: {search_engine}")
        return search_engine
    except Exception as e:
        logger.error(f"Error: {e}")
        return "Unknown"

def get_edge_font_size(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == 'Windows':
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Microsoft/Edge/User Data/Default/Preferences'))"
        )['output'].strip()
    else:
        raise Exception('Unsupported operating system')
    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)
        # The path within the JSON data to the default search engine might vary
        font_size = data.get('webkit', {}).get('webprefs', {
            "default_fixed_font_size": 13,
            "default_font_size": 16
        })
        logger.info(f"font_size: {font_size}")
        return font_size
    except Exception as e:
        logger.error(f"Error: {e}")
        return {
            "default_fixed_font_size": 13,
            "default_font_size": 16
        }

def get_enable_enhanced_safety_browsing_from_edge(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == 'Windows':
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Microsoft/Edge/User Data/Default/Preferences'))"
        )['output'].strip()
    else:
        raise Exception('Unsupported operating system')
    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)
        is_enabled = data['edge']['super_duper_secure_mode']['enabled'] # bool
        logger.info(f"enhanced safety browsing is enabled: {is_enabled}")
        return 1.0 if is_enabled else 0.0
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0.0
    
def get_enable_do_not_track_from_edge(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == 'Windows':
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Microsoft/Edge/User Data/Default/Preferences'))"
        )['output'].strip()
    else:
        raise Exception('Unsupported operating system')
    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)
        if_enable_do_not_track = data.get('enable_do_not_track', {})  # bool
        logger.info(f"enable_do_not_track: {if_enable_do_not_track}")
        return 1.0 if if_enable_do_not_track else 0.0
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0.0
    
def get_profile_name_from_edge(env, config: Dict[str, str]):
    """
    Get the username from the Chrome browser.
    Assume the cookies are stored in the default location, not encrypted and not large in size.
    """
    os_type = env.vm_platform
    if os_type == 'Windows':
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Microsoft/Edge/User Data/Default/Preferences'))"
        )['output'].strip()
    # elif os_type == 'Darwin':
    #     preference_file_path = env.controller.execute_python_command(
    #         "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Default/Preferences'))")[
    #         'output'].strip()
    # elif os_type == 'Linux':
    #     if "arm" in platform.machine():
    #         raise NotImplementedError
    #     else:
    #         preference_file_path = env.controller.execute_python_command(
    #             "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Default/Preferences'))")[
    #             'output'].strip()
    else:
        raise Exception('Unsupported operating system')

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)

        # The path within the JSON data to the default search engine might vary
        profile_name = data.get('profile', {}).get('name', None)
        return profile_name
    except Exception as e:
        logger.error(f"Error: {e}")
        return None


def get_cookie_data_from_edge(env, config: Dict[str, str]):
    """
    Get the cookies from the Chrome browser.
    Assume the cookies are stored in the default location, not encrypted and not large in size.
    Args:
        env (Any): The environment object.
        config (Dict[Any, Any]): The configuration dictionary.
            - user_data_dir (str): optional, for using a specific user data directory for the browser
    """
    os_type = env.vm_platform
    if 'user_data_dir' in config:
        assert not '\\' in config['user_data_dir'], 'user_data_dir cannot contain backslash' # maybe sanitize user_data_dir
        assert not '\'' in config['user_data_dir'], 'user_data_dir cannot contain single backticks'
        chrome_cookie_file_path = env.controller.execute_python_command(
            f"import os; print(os.path.join(os.path.expandvars('{config['user_data_dir']}'), 'Default/Network/Cookies'))"
        )['output'].strip()
    else:
        if os_type == 'Windows':
            chrome_cookie_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Default/Network/Cookies'))"
            )['output'].strip()
        elif os_type == 'Darwin':
            chrome_cookie_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Default/Cookies'))")[
                'output'].strip()
        elif os_type == 'Linux':
            if "arm" in platform.machine():
                raise NotImplementedError
            else:
                chrome_cookie_file_path = env.controller.execute_python_command(
                    "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Default/Cookies'))")[
                    'output'].strip()
        else:
            raise Exception('Unsupported operating system')

    try:
        content = env.controller.get_file(chrome_cookie_file_path)
        _path = os.path.join(env.cache_dir, config["dest"])

        with open(_path, "wb") as f:
            f.write(content)

        conn = sqlite3.connect(_path)
        cursor = conn.cursor()

        # Query to check for cookies
        cursor.execute("SELECT * FROM cookies")
        cookies = cursor.fetchall()
        return cookies
    except Exception as e:
        logger.error(f"Error: {e}")
        return None
    
def get_favorites(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == 'Windows':
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Microsoft/Edge/User Data/Default/Bookmarks'))"
        )['output'].strip()
    elif os_type == 'Darwin':
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Microsoft Edge/Default/Bookmarks'))"
        )['output'].strip()
    elif os_type == 'Linux':
        if "arm" in platform.machine():
            raise NotImplementedError
        else:
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config/microsoft-edge/Default/Bookmarks'))"
        )['output'].strip()
    else:
        raise Exception('Unsupported operating system')

    content = env.controller.get_file(preference_file_path)
    if not content:
        return []
    data = json.loads(content)
    favorites = data.get('roots', {})
    return favorites

def get_history_for_edge(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == 'Windows':
        edge_history_path = env.controller.execute_python_command(
            """import os; print(os.path.join(os.getenv('USERPROFILE'), "AppData", "Local", "Microsoft", "Edge", "User Data", "Default", "History"))""")[
            'output'].strip()
    elif os_type == 'Darwin':
        edge_history_path = env.controller.execute_python_command(
            """import os; print(os.path.join(os.getenv('HOME'), "Library", "Application Support", "Microsoft", "Edge", "Default", "History"))""")[
            'output'].strip()
    elif os_type == 'Linux':
        if "arm" in platform.machine():
            raise NotImplementedError
        else:
            edge_history_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config', 'microsoft-edge', 'Default', 'History'))")[
                'output'].strip()
    else:
        raise Exception('Unsupported operating system')

    try:
        content = env.controller.get_file(edge_history_path)
        _path = os.path.join(env.cache_dir, config["dest"])

        with open(_path, "wb") as f:
            f.write(content)

        conn = sqlite3.connect(_path)
        cursor = conn.cursor()

        # Query to check for OpenAI cookies
        cursor.execute("SELECT url, title, last_visit_time FROM urls")
        history_items = cursor.fetchall()
        return history_items
    except Exception as e:
        logger.error(f"Error: {e}")
        return None

def get_cookie_data_for_edge(env, config: Dict[str, str]):
    """
    Get the cookies from the Edge browser.
    Assume the cookies are stored in the default location, not encrypted and not large in size.
    Args:
        env (Any): The environment object.
        config (Dict[Any, Any]): The configuration dictionary.
            - user_data_dir (str): optional, for using a specific user data directory for the browser
    """
    os_type = env.vm_platform
    if 'user_data_dir' in config:
        assert not '\\' in config['user_data_dir'], 'user_data_dir cannot contain backslash' # maybe sanitize user_data_dir
        assert not '\'' in config['user_data_dir'], 'user_data_dir cannot contain single backticks'
        edge_cookie_file_path = env.controller.execute_python_command(
            f"import os; print(os.path.join(os.path.expandvars('{config['user_data_dir']}'), 'Default/Network/Cookies'))"
        )['output'].strip()
    else:
        if os_type == 'Windows':
            edge_cookie_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Microsoft/Edge/User Data/Default/Network/Cookies'))"
            )['output'].strip()
        elif os_type == 'Darwin':
            edge_cookie_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Microsoft/Edge/Default/Cookies'))")[
                'output'].strip()
        elif os_type == 'Linux':
            if "arm" in platform.machine():
                raise NotImplementedError
            else:
                edge_cookie_file_path = env.controller.execute_python_command(
                    "import os; print(os.path.join(os.getenv('HOME'), '.config/microsoft-edge/Default/Cookies'))")[
                    'output'].strip()
        else:
            raise Exception('Unsupported operating system')

    try:
        # kill msedge first, otherwise we can't access the cookies
        env.controller.execute_python_command("import os; os.system('taskkill /F /IM msedge.exe /T')")['output'].strip()
        
        content = env.controller.get_file(edge_cookie_file_path)
        _path = os.path.join(env.cache_dir, config["dest"])

        with open(_path, "wb") as f:
            f.write(content)

        conn = sqlite3.connect(_path)
        cursor = conn.cursor()

        # Query to check for cookies
        cursor.execute("SELECT * FROM cookies")
        cookies = cursor.fetchall()
        return cookies
    except Exception as e:
        logger.error(f"Error: {e}")
        return None