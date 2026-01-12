import asyncio
import json
import logging
import os
import platform
import sqlite3
import time
from contextlib import redirect_stdout
from io import BytesIO, StringIO
from typing import Any, Dict, List
from urllib.parse import parse_qs, unquote, urlparse

import LnkParse3
import lxml.etree
from computer.interface.factory import InterfaceFactory
from lxml.cssselect import CSSSelector
from playwright.sync_api import sync_playwright
from pydrive.auth import GoogleAuth
from pydrive.drive import GoogleDrive, GoogleDriveFile, GoogleDriveFileList

_accessibility_ns_map = {
    "st": "uri:deskat:state.at-spi.gnome.org",
    "attr": "uri:deskat:attributes.at-spi.gnome.org",
    "cp": "uri:deskat:component.at-spi.gnome.org",
    "doc": "uri:deskat:document.at-spi.gnome.org",
    "docattr": "uri:deskat:attributes.document.at-spi.gnome.org",
    "txt": "uri:deskat:text.at-spi.gnome.org",
    "val": "uri:deskat:value.at-spi.gnome.org",
    "act": "uri:deskat:action.at-spi.gnome.org",
}

logger = logging.getLogger("desktopenv.getters.chrome")


def _launch_app_on_vm(host: str, command: List[str], port: int = 5000):
    """Launch an application on the VM using cua-computer SDK."""
    try:
        interface = InterfaceFactory.create_interface_for_os(
            os="windows",
            ip_address=host,
            api_port=port,
        )
        app = command[0]
        args = command[1:] if len(command) > 1 else None
        asyncio.run(interface.launch(app, args))
        interface.close()
    except Exception as e:
        logger.error("Failed to launch app %s: %s", command, e)


"""
WARNING:
1. Functions from this script assume that no account is registered on Chrome, otherwise the default file path needs to be changed.
2. The functions are not tested on Windows and Mac, but they should work.
"""


def get_info_from_website(env, config: Dict[Any, Any]) -> Any:
    """Get information from a website. Especially useful when the information may be updated through time.
    Args:
        env (Any): The environment object.
        config (Dict[Any, Any]): The configuration dictionary.
            - url (str): The URL of the website to visit
            - infos (List[Dict[str, str]]): The list of information to be extracted from the website. Each dictionary contains:
                - action (str): chosen from 'inner_text', 'attribute', 'click_and_inner_text', 'click_and_attribute', etc., concretely,
                    - inner_text: extract the inner text of the element specified by the selector
                    - attribute: extract the attribute of the element specified by the selector
                    - click_and_inner_text: click elements following the selector and then extract the inner text of the last element
                    - click_and_attribute: click elements following the selector and then extract the attribute of the last element
                - selector (Union[str, List[str]]): The CSS selector(s) of the element(s) to be extracted.
                - attribute (str): optional for 'attribute' and 'click_and_attribute', the attribute to be extracted.
            - backups (Any): The backup information to be returned if the extraction fails.
    """
    try:
        host = env.vm_ip
        port = 9222  # fixme: this port is hard-coded, need to be changed from config file
        remote_debugging_url = f"http://{host}:{port}"
        with sync_playwright() as p:
            # connect to remote Chrome instance
            try:
                browser = p.chromium.connect_over_cdp(remote_debugging_url)
            except Exception:
                # If the connection fails (e.g., the agent close the browser instance), start a new browser instance
                app = "chromium" if "arm" in platform.machine() else "google-chrome"
                _launch_app_on_vm(host, [app, "--remote-debugging-port=1337"])
                time.sleep(5)
                browser = p.chromium.connect_over_cdp(remote_debugging_url)

            page = browser.contexts[0].new_page()
            page.goto(config["url"])
            page.wait_for_load_state("load")
            infos = []
            for info_dict in config.get("infos", []):
                if page.url != config["url"]:
                    page.goto(config["url"])
                    page.wait_for_load_state("load")
                action = info_dict.get("action", "inner_text")
                if action == "inner_text":
                    ele = page.wait_for_selector(
                        info_dict["selector"], state="attached", timeout=10000
                    )
                    infos.append(ele.inner_text())
                elif action == "attribute":
                    ele = page.wait_for_selector(
                        info_dict["selector"], state="attached", timeout=10000
                    )
                    infos.append(ele.get_attribute(info_dict["attribute"]))
                elif action == "click_and_inner_text":
                    for idx, sel in enumerate(info_dict["selector"]):
                        if idx != len(info_dict["selector"]) - 1:
                            link = page.wait_for_selector(sel, state="attached", timeout=10000)
                            link.click()
                            page.wait_for_load_state("load")
                        else:
                            ele = page.wait_for_selector(sel, state="attached", timeout=10000)
                            infos.append(ele.inner_text())
                elif action == "click_and_attribute":
                    for idx, sel in enumerate(info_dict["selector"]):
                        if idx != len(info_dict["selector"]) - 1:
                            link = page.wait_for_selector(sel, state="attached", timeout=10000)
                            link.click()
                            page.wait_for_load_state("load")
                        else:
                            ele = page.wait_for_selector(sel, state="attached")
                            infos.append(ele.get_attribute(info_dict["attribute"]))
                else:
                    raise NotImplementedError(f"The action {action} is not supported yet.")
        return infos
    except Exception:
        logger.error(
            f'[ERROR]: failed to obtain information from the website: {config["url"]}. Use backup results instead.'
        )
        return config.get("backups", None)


# The following ones just need to load info from the files of software, no need to connect to the software
def get_default_search_engine(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == "Windows":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Darwin":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            raise NotImplementedError
        else:
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Default/Preferences'))"
            )["output"].strip()
    else:
        raise Exception("Unsupported operating system")

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)

        # Windows fix (default_search_provider_data is missing on Windows)
        # GUID reference:
        # https://chromium.googlesource.com/chromium/src/+/8aca664983590de953a8785d88151bddc91c170c/components/test/data/web_database/version_82.sql
        engine_guids = {
            "485bf7d3-0215-45af-87dc-538868000001": "Google",
            "485bf7d3-0215-45af-87dc-538868000003": "Bing",
            "485bf7d3-0215-45af-87dc-538868000002": "Yahoo!",
            "485bf7d3-0215-45af-87dc-538868000092": "DuckDuckGo",
        }
        search_engine = engine_guids.get(data.get("default_search_provider", {}).get("guid"))
        if not search_engine:  # fallback
            search_engine = (
                data.get("default_search_provider_data", {})
                .get("template_url_data", {})
                .get("short_name", "Google")
            )
        return search_engine
    except Exception as e:
        logger.error(f"Error: {e}")
        return "Google"


def get_cookie_data(env, config: Dict[str, str]):
    """
    Get the cookies from the Chrome browser.
    Assume the cookies are stored in the default location, not encrypted and not large in size.
    Args:
        env (Any): The environment object.
        config (Dict[Any, Any]): The configuration dictionary.
            - user_data_dir (str): optional, for using a specific user data directory for the browser
    """
    os_type = env.vm_platform
    if "user_data_dir" in config:
        assert (
            "\\" not in config["user_data_dir"]
        ), "user_data_dir cannot contain backslash"  # maybe sanitize user_data_dir
        assert "'" not in config["user_data_dir"], "user_data_dir cannot contain single backticks"
        chrome_cookie_file_path = env.controller.execute_python_command(
            f"import os; print(os.path.join(os.path.expandvars('{config['user_data_dir']}'), 'Default/Network/Cookies'))"
        )["output"].strip()
    else:
        if os_type == "Windows":
            chrome_cookie_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Default/Network/Cookies'))"
            )["output"].strip()
        elif os_type == "Darwin":
            chrome_cookie_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Default/Cookies'))"
            )["output"].strip()
        elif os_type == "Linux":
            if "arm" in platform.machine():
                raise NotImplementedError
            else:
                chrome_cookie_file_path = env.controller.execute_python_command(
                    "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Default/Cookies'))"
                )["output"].strip()
        else:
            raise Exception("Unsupported operating system")

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


def get_history(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == "Windows":
        chrome_history_path = env.controller.execute_python_command(
            """import os; print(os.path.join(os.getenv('USERPROFILE'), "AppData", "Local", "Google", "Chrome", "User Data", "Default", "History"))"""
        )["output"].strip()
    elif os_type == "Darwin":
        chrome_history_path = env.controller.execute_python_command(
            """import os; print(os.path.join(os.getenv('HOME'), "Library", "Application Support", "Google", "Chrome", "Default", "History"))"""
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            raise NotImplementedError
        else:
            chrome_history_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config', 'google-chrome', 'Default', 'History'))"
            )["output"].strip()
    else:
        raise Exception("Unsupported operating system")

    try:
        content = env.controller.get_file(chrome_history_path)
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


def get_enabled_experiments(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == "Windows":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Local State'))"
        )["output"].strip()
    elif os_type == "Darwin":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Local State'))"
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            raise NotImplementedError
        else:
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Local State'))"
            )["output"].strip()
    else:
        raise Exception("Unsupported operating system")

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)

        # The path within the JSON data to the default search engine might vary
        enabled_labs_experiments = data.get("browser", {}).get("enabled_labs_experiments", [])
        return enabled_labs_experiments
    except Exception as e:
        logger.error(f"Error: {e}")
        return []


def get_profile_name(env, config: Dict[str, str]):
    """
    Get the username from the Chrome browser.
    Assume the cookies are stored in the default location, not encrypted and not large in size.
    """
    os_type = env.vm_platform
    if os_type == "Windows":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Darwin":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            raise NotImplementedError
        else:
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Default/Preferences'))"
            )["output"].strip()
    else:
        raise Exception("Unsupported operating system")

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)

        # The path within the JSON data to the default search engine might vary
        profile_name = data.get("profile", {}).get("name", None)
        return profile_name
    except Exception as e:
        logger.error(f"Error: {e}")
        return None


def get_chrome_language(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == "Windows":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Local State'))"
        )["output"].strip()
    elif os_type == "Darwin":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Local State'))"
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            raise NotImplementedError
        else:
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Local State'))"
            )["output"].strip()
    else:
        raise Exception("Unsupported operating system")

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)

        # The path within the JSON data to the default search engine might vary
        enabled_labs_experiments = data.get("intl", {}).get("app_locale", "en-US")
        return enabled_labs_experiments
    except Exception as e:
        logger.error(f"Error: {e}")
        return "en-US"


def get_chrome_font_size(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == "Windows":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Darwin":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            raise NotImplementedError
        else:
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Default/Preferences'))"
            )["output"].strip()
    else:
        raise Exception("Unsupported operating system")

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)

        # The path within the JSON data to the default search engine might vary
        search_engine = data.get("webkit", {}).get(
            "webprefs",
            {"default_fixed_font_size": 13, "default_font_size": 16, "minimum_font_size": 13},
        )
        return search_engine
    except Exception as e:
        logger.error(f"Error: {e}")
        return {"default_fixed_font_size": 13, "default_font_size": 16}


def get_bookmarks(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == "Windows":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Default/Bookmarks'))"
            # "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google', 'Chrome', 'User Data', 'Default', 'Bookmarks'))"
        )["output"].strip()
    elif os_type == "Darwin":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Default/Bookmarks'))"
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            raise NotImplementedError
        else:
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Default/Bookmarks'))"
            )["output"].strip()
    else:
        raise Exception("Unsupported operating system")

    content = env.controller.get_file(preference_file_path)
    if not content:
        return []
    data = json.loads(content)
    bookmarks = data.get("roots", {})
    return bookmarks


# todo: move this to the main.py
def get_extensions_installed_from_shop(env, config: Dict[str, str]):
    """Find the Chrome extensions directory based on the operating system."""
    os_type = env.vm_platform
    if os_type == "Windows":
        chrome_extension_dir = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Default/Extensions/'))"
        )["output"].strip()
    elif os_type == "Darwin":  # macOS
        chrome_extension_dir = env.controller.execute_python_command(
            """os.path.expanduser('~') + '/Library/Application Support/Google/Chrome/Default/Extensions/'"""
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            raise NotImplementedError
        else:
            chrome_extension_dir = env.controller.execute_python_command(
                """os.path.expanduser('~') + '/.config/google-chrome/Default/Extensions/'"""
            )["output"].strip()
    else:
        raise Exception("Unsupported operating system")

    manifests = []
    for extension_id in os.listdir(chrome_extension_dir):
        extension_path = os.path.join(chrome_extension_dir, extension_id)
        if os.path.isdir(extension_path):
            # Iterate through version-named subdirectories
            for version_dir in os.listdir(extension_path):
                version_path = os.path.join(extension_path, version_dir)
                manifest_path = os.path.join(version_path, "manifest.json")
                if os.path.isfile(manifest_path):
                    with open(manifest_path, "r") as file:
                        try:
                            manifest = json.load(file)
                            manifests.append(manifest)
                        except json.JSONDecodeError:
                            logger.error(f"Error reading {manifest_path}")
    return manifests


# The following ones require Playwright to be installed on the target machine, and the chrome needs to be pre-config on
# port info to allow remote debugging, see README.md for details


def get_page_info(env, config: Dict[str, str]):
    """Get information from a website.
    Args:
        env (Any): The environment object.
        config (Dict[Any, Any]): The configuration dictionary.
            - url (str): The URL of the website to visit
            - load_state (str): The playwright load state to wait for. Can be 'load' or 'domcontentloaded'
    """

    host = env.vm_ip
    port = 9222  # fixme: this port is hard-coded, need to be changed from config file
    url = config["url"]
    load_state = config.get("load_state", "load")

    remote_debugging_url = f"http://{host}:{port}"
    with sync_playwright() as p:
        # connect to remote Chrome instance
        try:
            browser = p.chromium.connect_over_cdp(remote_debugging_url)
        except Exception:
            # If the connection fails, start a new browser instance
            app = "chromium" if "arm" in platform.machine() else "google-chrome"
            _launch_app_on_vm(host, [app, "--remote-debugging-port=1337"])
            time.sleep(5)
            browser = p.chromium.connect_over_cdp(remote_debugging_url)

        page = browser.contexts[0].new_page()
        page.goto(url)

        try:
            # Wait for the page to finish loading, this prevents the "execution context was destroyed" issue
            page.wait_for_load_state(load_state)  # Wait for the 'load' event to complete
            title = page.title()
            url = page.url
            page_info = {"title": title, "url": url, "content": page.content()}
        except TimeoutError:
            # If page loading times out, catch the exception and store the current information in the list
            page_info = {"title": "Load timeout", "url": page.url, "content": page.content()}
        except Exception as e:
            # Catch other potential exceptions that might occur while reading the page title
            print(f"Error: {e}")
            page_info = {"title": "Error encountered", "url": page.url, "content": page.content()}

        browser.close()
        return page_info


def get_open_tabs_info(env, config: Dict[str, str]):
    host = env.vm_ip
    port = 9222  # fixme: this port is hard-coded, need to be changed from config file

    remote_debugging_url = f"http://{host}:{port}"
    with sync_playwright() as p:
        # connect to remote Chrome instance
        try:
            browser = p.chromium.connect_over_cdp(remote_debugging_url)
        except Exception:
            # If the connection fails, start a new browser instance
            app = "chromium" if "arm" in platform.machine() else "google-chrome"
            _launch_app_on_vm(host, [app, "--remote-debugging-port=1337"])
            time.sleep(5)
            try:
                browser = p.chromium.connect_over_cdp(remote_debugging_url)
            except Exception:
                return []

        tabs_info = []
        for context in browser.contexts:
            for page in context.pages:
                try:
                    # Wait for the page to finish loading, this prevents the "execution context was destroyed" issue
                    page.wait_for_load_state("networkidle")  # Wait for the 'load' event to complete
                    title = page.title()
                    url = page.url
                    tabs_info.append({"title": title, "url": url})
                except TimeoutError:
                    # If page loading times out, catch the exception and store the current information in the list
                    tabs_info.append({"title": "Load timeout", "url": page.url})
                except Exception as e:
                    # Catch other potential exceptions that might occur while reading the page title
                    print(f"Error: {e}")
                    tabs_info.append({"title": "Error encountered", "url": page.url})

        browser.close()
        return tabs_info


def get_active_url_from_accessTree(env, config):
    """
    Playwright cannot get the url of active tab directly,
    so we need to use accessibility tree to get the active tab info.
    This function is used to get the active tab url from the accessibility tree.
    config:
        Dict[str, str]{
            # we no longer need to specify the xpath or selectors, since we will use defalut value
            # 'xpath':
            #     the same as in metrics.general.accessibility_tree.
            # 'selectors':
            #     the same as in metrics.general.accessibility_tree.
            'goto_prefix':
                the prefix you want to add to the beginning of the url to be opened, default is "https://",
                (the url we get from accTree does not have prefix)
            ...(other keys, not used in this function)
    }
    Return
        url: str
    """

    # Determine the correct a11y backend & selector based on OS and system architecture
    selector = None
    selector_string = None
    backend = None
    arch = platform.machine()
    os_name = env.vm_platform
    print(f"Your OS is: {os_name}")
    print(f"Your architecture is: {arch}")
    if os_name == "Windows":
        backend = "uia"
        selector_string = "chrome_widgetwin_1 [name=Address\\ and\\ search\\ bar]"
    else:
        if "arm" in arch:
            selector_string = "application[name=Chromium] entry[name=Address\\ and\\ search\\ bar]"
        else:
            selector_string = (
                "application[name=Google\\ Chrome] entry[name=Address\\ and\\ search\\ bar]"
            )

    # Ensure the controller and its method are accessible and return a valid result
    if hasattr(env, "controller") and callable(
        getattr(env.controller, "get_accessibility_tree", None)
    ):
        accessibility_tree = env.controller.get_accessibility_tree(backend=backend)
        if accessibility_tree is None:
            print("Failed to get the accessibility tree.")
            return None
    else:
        print("Controller or method 'get_accessibility_tree' not found.")
        return None

    logger.debug("AT@eval: %s", accessibility_tree)

    at = None
    try:
        at = lxml.etree.fromstring(accessibility_tree)
    except ValueError as e:
        logger.error(f"Error parsing accessibility tree: {e}")
        return None

    try:
        selector = CSSSelector(selector_string, namespaces=_accessibility_ns_map)
    except Exception as e:
        logger.error(f"Failed to parse the selector for active tab URL: {e}")
        return None

    elements = selector(at) if selector else []
    if not elements:
        print("No elements found.")
        return None

    text = None
    if os_name == "Windows":
        text = elements[-1].attrib["{uri:deskat:value.at-spi.gnome.org}value"]
    else:
        text = elements[-1].text

    if not text:
        print("No text found in the latest element.")
        return None

    # Use a default prefix if 'goto_prefix' is not specified in the config
    goto_prefix = config.get("goto_prefix", "https://")

    active_tab_url = f"{goto_prefix}{text}"
    print(f"Active tab url now: {active_tab_url}")
    return active_tab_url


def get_active_tab_info(env, config: Dict[str, str]):
    """
    This function is used to get all info about active tab.
    Warning! This function will reload the target-url page
    If the tartget url has cache or cookie, this function may reload to another page.
    If you have tested the url will not pop up to another page (check in incongnito mode yourself first),
    you can use this function.
    config: Dict[str, str]{
        # Keys used in get_active_url_from_accessTree: "xpath", "selectors"
    }
    """
    active_tab_url = get_active_url_from_accessTree(env, config)
    if active_tab_url is None:
        logger.error("Failed to get the url of active tab")
        return None
    host = env.vm_ip
    port = 9222  # fixme: this port is hard-coded, need to be changed from config file

    remote_debugging_url = f"http://{host}:{port}"
    with sync_playwright() as p:
        # connect to remote Chrome instance, since it is supposed to be the active one, we won't start a new one if failed
        try:
            browser = p.chromium.connect_over_cdp(remote_debugging_url)
        except Exception:
            return None

        active_tab_info = {}
        # go to the target URL page
        page = browser.new_page()
        try:
            page.goto(active_tab_url)
        except Exception as e:
            logger.error("Failed to go to the target URL page: %s", str(e))
            return None
        page.wait_for_load_state("load")  # Wait for the 'load' event to complete
        active_tab_info = {
            "title": page.title(),
            "url": page.url,
            "content": page.content(),  # get the HTML content of the page
        }

        browser.close()
        # print("active_tab_title: {}".format(active_tab_info.get('title', 'None')))
        # print("active_tab_url: {}".format(active_tab_info.get('url', 'None')))
        # print("active_tab_content: {}".format(active_tab_info.get('content', 'None')))
        return active_tab_info


def get_pdf_from_url(env, config: Dict[str, str]) -> str:
    """
    Download a PDF from a URL.
    """
    _url = config["path"]
    _path = os.path.join(env.cache_dir, config["dest"])

    host = env.vm_ip
    port = 9222  # fixme: this port is hard-coded, need to be changed from config file

    remote_debugging_url = f"http://{host}:{port}"

    with sync_playwright() as p:
        try:
            browser = p.chromium.connect_over_cdp(remote_debugging_url)
        except Exception:
            # If the connection fails, start a new browser instance
            app = "chromium" if "arm" in platform.machine() else "google-chrome"
            _launch_app_on_vm(host, [app, "--remote-debugging-port=1337"])
            time.sleep(5)
            browser = p.chromium.connect_over_cdp(remote_debugging_url)

        page = browser.new_page()
        page.goto(_url)
        page.pdf(path=_path)
        browser.close()

    return _path


# fixme: needs to be changed (maybe through post-processing) since it's not working
def get_chrome_saved_address(env, config: Dict[str, str]):
    host = env.vm_ip
    port = 9222  # fixme: this port is hard-coded, need to be changed from config file

    remote_debugging_url = f"http://{host}:{port}"
    with sync_playwright() as p:
        # connect to remote Chrome instance
        try:
            browser = p.chromium.connect_over_cdp(remote_debugging_url)
        except Exception:
            # If the connection fails, start a new browser instance
            app = "chromium" if "arm" in platform.machine() else "google-chrome"
            _launch_app_on_vm(host, [app, "--remote-debugging-port=1337"])
            time.sleep(5)
            browser = p.chromium.connect_over_cdp(remote_debugging_url)

        page = browser.new_page()

        # Navigate to Chrome's settings page for autofill
        page.goto("chrome://settings/addresses")

        # Get the HTML content of the page
        content = page.content()

        browser.close()

    return content


def get_shortcuts_on_desktop(env, config: Dict[str, str]):
    # Find out the operating system
    os_name = env.vm_platform

    # Depending on the OS, define the shortcut file extension
    if os_name == "Windows":
        # Windows shortcuts are typically .url or .lnk files
        shortcut_extension = ".lnk"
    elif os_name == "Darwin":
        # macOS's shortcuts are .webloc files
        shortcut_extension = ".webloc"
    elif os_name == "Linux":
        # Linux (Ubuntu, etc.) shortcuts are typically .desktop files
        shortcut_extension = ".desktop"
    else:
        logger.error(f"Unsupported operating system: {os_name}")
        return []

    # Get the path to the desktop folder
    desktop_path = env.controller.get_vm_desktop_path()
    desktop_directory_tree = env.controller.get_vm_directory_tree(desktop_path)

    shortcuts_paths = [
        file["name"]
        for file in desktop_directory_tree["children"]
        if file["name"].endswith(shortcut_extension)
    ]

    short_cuts = {}

    for shortcut_name in shortcuts_paths:
        shortcut_path = os.path.join(desktop_path, shortcut_name)
        shortcut_data = env.controller.get_file(shortcut_path)
        if (
            os_name == "Windows"
        ):  # on windows shortcuts use a binary encoded .lnk file, so we need to parse them
            with BytesIO(shortcut_data) as indata:
                lnk = LnkParse3.lnk_file(indata)
            mock_stdout = StringIO()
            with redirect_stdout(mock_stdout):
                lnk.print_json(print_all=True)
            short_cuts[shortcut_name] = mock_stdout.getvalue()
        else:
            short_cuts[shortcut_name] = shortcut_data.decode("utf-8")

    return short_cuts


def get_number_of_search_results(env, config: Dict[str, str]):
    # todo: move into the config file
    url, result_selector = "https://google.com/search?q=query", ".search-result"
    host = env.vm_ip
    port = 9222  # fixme: this port is hard-coded, need to be changed from config file

    remote_debugging_url = f"http://{host}:{port}"
    with sync_playwright() as p:
        try:
            browser = p.chromium.connect_over_cdp(remote_debugging_url)
        except Exception:
            # If the connection fails, start a new browser instance
            app = "chromium" if "arm" in platform.machine() else "google-chrome"
            _launch_app_on_vm(host, [app, "--remote-debugging-port=1337"])
            time.sleep(5)
            browser = p.chromium.connect_over_cdp(remote_debugging_url)
        page = browser.new_page()
        page.goto(url)
        search_results = page.query_selector_all(result_selector)
        actual_count = len(search_results)
        browser.close()

    return actual_count


def get_googledrive_file(env, config: Dict[str, Any]) -> str:
    """Get the desired file from Google Drive based on config, return the downloaded local filepath.
    @args: keys in config dict
        settings_file(str): target filepath to the settings file for Google Drive authentication, default is 'evaluation_examples/settings/googledrive/settings.yml'
        query/path[_list](Union[str, List[str]]): the query or path [list] to the file(s) on Google Drive. To retrieve the file, we provide multiple key options to specify the filepath on drive in config dict:
            1) query: a list of queries to search the file, each query is a string that follows the format of Google Drive search query. The documentation is available here: (support more complex search but too complicated to use)
                https://developers.google.com/drive/api/guides/search-files?hl=en
            2) path: a str list poingting to file path on googledrive, e.g., 'folder/subfolder/filename.txt' ->
                config contain one key-value pair "path": ['folder', 'subfolder', 'filename.txt']
            3) query_list: query extends to list to download multiple files
            4) path_list: path extends to list to download multiple files, e.g.,
                "path_list": [['folder', 'subfolder', 'filename1.txt'], ['folder', 'subfolder', 'filename2.txt']]
    @return:
        dest(Union[List[str], str]): target file name or list. If *_list is used in input config, dest should also be a list of the same length. Return the downloaded local filepath.
    """
    settings_file = config.get(
        "settings_file", "evaluation_examples/settings/googledrive/settings.yml"
    )
    auth = GoogleAuth(settings_file=settings_file)
    drive = GoogleDrive(auth)

    def get_single_file(_query, _path):
        parent_id = "root"
        try:
            for q in _query:
                search = f'( {q} ) and "{parent_id}" in parents'
                filelist: GoogleDriveFileList = drive.ListFile({"q": search}).GetList()
                if len(filelist) == 0:  # target file not found
                    return None
                file: GoogleDriveFile = filelist[
                    0
                ]  # HACK: if multiple candidates, just use the first one
                parent_id = file["id"]

            file.GetContentFile(_path, mimetype=file["mimeType"])
        except Exception as e:
            logger.info("[ERROR]: Failed to download the file from Google Drive", e)
            return None
        return _path

    if "query" in config:
        return get_single_file(config["query"], os.path.join(env.cache_dir, config["dest"]))
    elif "path" in config:
        query = [
            (
                f"title = '{fp}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false"
                if idx < len(config["path"]) - 1
                else f"title = '{fp}' and trashed = false"
            )
            for idx, fp in enumerate(config["path"])
        ]
        return get_single_file(query, os.path.join(env.cache_dir, config["dest"]))
    elif "query_list" in config:
        _path_list = []
        assert len(config["query_list"]) == len(config["dest"])
        for idx, query in enumerate(config["query_list"]):
            dest = config["dest"][idx]
            _path_list.append(get_single_file(query, os.path.join(env.cache_dir, dest)))
        return _path_list
    else:  # path_list in config
        _path_list = []
        assert len(config["path_list"]) == len(config["dest"])
        for idx, path in enumerate(config["path_list"]):
            query = [
                (
                    f"title = '{fp}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false"
                    if jdx < len(path) - 1
                    else f"title = '{fp}' and trashed = false"
                )
                for jdx, fp in enumerate(path)
            ]
            dest = config["dest"][idx]
            _path_list.append(get_single_file(query, os.path.join(env.cache_dir, dest)))
        return _path_list


def get_enable_do_not_track(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == "Windows":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Darwin":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), 'snap/chromium/common/chromium/Default/Preferences'))"
            )["output"].strip()
        else:
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Default/Preferences'))"
            )["output"].strip()

    else:
        raise Exception("Unsupported operating system")

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)

        if_enable_do_not_track = data.get("enable_do_not_track", {})  # bool
        return "true" if if_enable_do_not_track else "false"
    except Exception as e:
        logger.error(f"Error: {e}")
        return "false"


def get_enable_enhanced_safety_browsing(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == "Windows":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Darwin":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), 'snap/chromium/common/chromium/Default/Preferences'))"
            )["output"].strip()
        else:
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Default/Preferences'))"
            )["output"].strip()

    else:
        raise Exception("Unsupported operating system")

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)

        if_enable_do_not_track = data.get("safebrowsing", {}).get("enhanced", {})  # bool
        return "true" if if_enable_do_not_track else "false"
    except Exception as e:
        logger.error(f"Error: {e}")
        return "Google"


def get_new_startup_page(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == "Windows":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Darwin":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), 'snap/chromium/common/chromium/Default/Preferences'))"
            )["output"].strip()
        else:
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Default/Preferences'))"
            )["output"].strip()

    else:
        raise Exception("Unsupported operating system")

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)

        # if data has no key called 'session', it means the chrome is on a fresh-start mode, which is a true state;
        # otherwise, try to find the code number in 'restored_on_startup' in 'session'
        if "session" not in data.keys():
            return "true"
        else:
            if_enable_do_not_track = data.get("session", {}).get(
                "restore_on_startup", {}
            )  # int, need to be 5
            return "true" if if_enable_do_not_track == 5 else "false"
    except Exception as e:
        logger.error(f"Error: {e}")
        return "Google"


def get_find_unpacked_extension_path(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == "Windows":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Darwin":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), 'snap/chromium/common/chromium/Default/Preferences'))"
            )["output"].strip()
        else:
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Default/Preferences'))"
            )["output"].strip()

    else:
        raise Exception("Unsupported operating system")

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)
        # Preferences store all the path of installed extensions, return them all and let metrics try to find one matches the targeted extension path
        all_extensions_path = []
        all_extensions = data.get("extensions", {}).get("settings", {})
        for id in all_extensions.keys():
            path = all_extensions[id]["path"]
            all_extensions_path.append(path)
        return all_extensions_path
    except Exception as e:
        logger.error(f"Error: {e}")
        return "Google"


def get_find_installed_extension_name(env, config: Dict[str, str]):
    os_type = env.vm_platform
    if os_type == "Windows":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Darwin":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), 'snap/chromium/common/chromium/Default/Preferences'))"
            )["output"].strip()
        else:
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Default/Preferences'))"
            )["output"].strip()

    else:
        raise Exception("Unsupported operating system")

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)
        # Preferences store all the path of installed extensions, return them all and let metrics try to find one matches the targeted extension path
        all_extensions_name = []
        all_extensions = data.get("extensions", {}).get("settings", {})
        for id in all_extensions.keys():
            name = all_extensions[id]["manifest"]["name"]
            all_extensions_name.append(name)
        return all_extensions_name
    except Exception as e:
        logger.error(f"Error: {e}")
        return "Google"


def get_data_delete_automacally(env, config: Dict[str, str]):
    """
    This function is used to open th "auto-delete" mode of chromium
    """
    os_type = env.vm_platform
    if os_type == "Windows":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Google/Chrome/User Data/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Darwin":
        preference_file_path = env.controller.execute_python_command(
            "import os; print(os.path.join(os.getenv('HOME'), 'Library/Application Support/Google/Chrome/Default/Preferences'))"
        )["output"].strip()
    elif os_type == "Linux":
        if "arm" in platform.machine():
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), 'snap/chromium/common/chromium/Default/Preferences'))"
            )["output"].strip()
        else:
            preference_file_path = env.controller.execute_python_command(
                "import os; print(os.path.join(os.getenv('HOME'), '.config/google-chrome/Default/Preferences'))"
            )["output"].strip()
    else:
        raise Exception("Unsupported operating system")

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)
        data_delete_state = data["profile"].get("default_content_setting_values", None)
        return "true" if data_delete_state is not None else "false"
    except Exception as e:
        logger.error(f"Error: {e}")
        return "Google"


def get_active_tab_html_parse(env, config: Dict[str, Any]):
    """
    This function is used to get the specific element's text content from the active tab's html.
    config:
        Dict[str, str]{
            # Keys used in get_active_url_from_accessTree: "xpath", "selectors"
            'category':
                choose from ["class", "label", "xpath", "input"], used to indicate how to find the element
            'labelObject':
                only exists when category is "label",
                a dict like { "labelSelector": "the key you want to store the text content of this label's ee=lement"}
            'class_singleObject':
                only exists when category is "class", a dict with keys as the class name,
                like { "class name" : "the key you want to store the text content of this element" }
            'class_multiObject':
                only exists when category is "class", used for elements with same class name.
                Two layer of dict, like
                    ( {
                        "class name": {
                            "rank in this class" : "the key you want to store the text content of this element"
                            ...
                            }
                        } )
            'xpathObject':
                only exists when category is "xpath", a dict with keys as the xpath,
                like { "full xpath" : "the key you want to store the text content of this element" }
            'inputObject':
                only exists when category is "input",
                a dict with keys as the input element's xpath, like { "full xpath" : "the key you want to store the text content of this element" }
    }
    """
    active_tab_url = get_active_url_from_accessTree(env, config)
    if not isinstance(active_tab_url, str):
        logger.error("active_tab_url is not a string")
        return None
    host = env.vm_ip
    port = 9222  # fixme: this port is hard-coded, need to be changed from config file

    remote_debugging_url = f"http://{host}:{port}"
    with sync_playwright() as p:
        # connect to remote Chrome instance
        try:
            browser = p.chromium.connect_over_cdp(remote_debugging_url)
        except Exception:
            # If the connection fails, start a new browser instance
            app = "chromium" if "arm" in platform.machine() else "google-chrome"
            _launch_app_on_vm(host, [app, "--remote-debugging-port=1337"])
            time.sleep(5)
            browser = p.chromium.connect_over_cdp(remote_debugging_url)
        target_page = None
        for context in browser.contexts:
            for page in context.pages:
                page.wait_for_load_state("networkidle")
                # the accTree and playwright can get encoding(percent-encoding) characters, we need to convert them to normal characters
                if unquote(page.url) == unquote(active_tab_url):
                    target_page = page
                    print("\33[32mtartget page url: ", target_page.url, "\33[0m")
                    print("\33[32mtartget page title: ", target_page.title(), "\33[0m")
                    break
        if target_page is None:
            logger.error("Your tab is not the target tab.")
            return {}

        return_json = {}

        def safely_get_text_content(selector):
            elements = target_page.query_selector_all(selector)
            return [element.text_content().strip() for element in elements if element]

        if config["category"] == "class":
            class_multiObject = config.get("class_multiObject", {})
            for class_name, object_dict in class_multiObject.items():
                elements_texts = safely_get_text_content("." + class_name)
                for order_key, key in object_dict.items():
                    index = int(order_key)
                    if len(elements_texts) > index:
                        return_json[key] = elements_texts[index]

            class_singleObject = config.get("class_singleObject", {})
            for class_name, key in class_singleObject.items():
                element_text = safely_get_text_content("." + class_name)
                if element_text:
                    return_json[key] = element_text[0]

        elif config["category"] == "label":
            # Assuming get_by_label is a custom function or part of the framework being used
            labelObject = config.get("labelObject", {})
            for labelSelector, key in labelObject.items():
                text = target_page.locator(f"text={labelSelector}").first.text_content().strip()
                if text:
                    return_json[key] = text

        elif config["category"] == "xpath":
            xpathObject = config.get("xpathObject", {})
            for xpath, key in xpathObject.items():
                elements = target_page.locator(f"xpath={xpath}")
                if elements.count() > 0:
                    return_json[key] = elements.first.text_content().strip()

        elif config["category"] == "input":
            inputObjects = config.get("inputObject", {})
            for xpath, key in inputObjects.items():
                inputs = target_page.locator(f"xpath={xpath}")
                if inputs.count() > 0:
                    return_json[key] = inputs.first.input_value().strip()

        browser.close()
    return return_json


def get_gotoRecreationPage_and_get_html_content(env, config: Dict[str, Any]):
    """
    especially used for www.recreation.gov examples
    """
    host = env.vm_ip
    port = 9222  # fixme: this port is hard-coded, need to be changed from config file

    remote_debugging_url = f"http://{host}:{port}"
    with sync_playwright() as p:
        try:
            browser = p.chromium.connect_over_cdp(remote_debugging_url)
        except Exception:
            # If the connection fails, start a new browser instance
            app = "chromium" if "arm" in platform.machine() else "google-chrome"
            _launch_app_on_vm(host, [app, "--remote-debugging-port=1337"])
            time.sleep(5)
            browser = p.chromium.connect_over_cdp(remote_debugging_url)
        page = browser.new_page()
        page.goto("https://www.recreation.gov/")
        page.fill("input#hero-search-input", "Albion Basin")
        page.click("button.nav-search-button")
        print("after first click")
        time.sleep(2)
        # Assuming .search-result-highlight--success leads to a new page or requires page load
        with page.expect_popup() as popup_info:
            page.click(".search-result-highlight--success")
        print("after second click")
        newpage = popup_info.value
        newpage.wait_for_load_state()
        print("go to newpage: ")
        print(newpage.title())
        time.sleep(2)
        newpage.click("button.next-available")
        print("after third click")

        return_json = {}
        return_json["expected"] = {}
        # find the text of elements in html with specific class name
        if config["selector"] == "class":
            if "order" in config.keys():
                className = config["class"]
                return_json["expected"][className] = (
                    newpage.query_selector_all("." + className)[int(config["order"])]
                    .text_content()
                    .strip()
                )
            else:
                className = config["class"]
                return_json["expected"][className] = (
                    newpage.query_selector("." + className).text_content().strip()
                )
        browser.close()
    return return_json


def get_active_tab_url_parse(env, config: Dict[str, Any]):
    """
    This function is used to parse the url according to config["parse_keys"].
    config:
        'parse_keys': must exist,
            a list of keys to extract from the query parameters of the url.
        'replace': optional,
            a dict, used to replace the original key with the new key.
            ( { "original key": "new key" } )
        'or': optional,
            a dict, used to replace the original key's value with the fallback key's value if the original key has no value.
            ( { "original key": "fallback key" } )
    """
    active_tab_url = get_active_url_from_accessTree(env, config)
    if active_tab_url is None:
        return None

    # connect to remote Chrome instance
    # parse in a hard-coded way to find the specific info about task
    parsed_url = urlparse(active_tab_url)
    # Extract the query parameters
    query_params = parse_qs(parsed_url.query)
    # Define the keys of interest
    keys_of_interest = [key for key in config["parse_keys"]]
    # Extract the parameters of interest
    extracted_params = {key: query_params.get(key, [""])[0] for key in keys_of_interest}
    if "replace" in config:
        for key in config["replace"].keys():
            # change original key to new key, keep value unchange
            value = extracted_params.pop(key)
            extracted_params[config["replace"][key]] = value
    if "or" in config:
        for key in config["or"].keys():
            # change original key to fallback key if the original key has no value
            extracted_params[key] = extracted_params[key] or extracted_params[config["or"][key]]
    return extracted_params


def get_url_dashPart(env, config: Dict[str, str]):
    """
    This function is used to extract one of the dash-separated part of the URL.
    config
        'partIndex': must exist,
            the index of the dash-separated part to extract, starting from 0.
        'needDeleteId': optional,
            a boolean, used to indicate whether to delete the "id" part ( an example: "/part-you-want?id=xxx" )
        'returnType': must exist,
            a string, used to indicate the return type, "string" or "json".
    """
    active_tab_url = get_active_url_from_accessTree(env, config)
    if active_tab_url is None:
        return None

    # extract the last dash-separated part of the URL, and delete all the characters after "id"
    dash_part = active_tab_url.split("/")[config["partIndex"]]
    if config["needDeleteId"]:
        dash_part = dash_part.split("?")[0]
    # print("active_tab_title: {}".format(active_tab_info.get('title', 'None')))
    # print("active_tab_url: {}".format(active_tab_info.get('url', 'None')))
    # print("active_tab_content: {}".format(active_tab_info.get('content', 'None')))
    if config["returnType"] == "string":
        return dash_part
    elif config["returnType"] == "json":
        return {config["key"]: dash_part}
