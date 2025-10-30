"""Set the desktop wallpaper."""

import os
import subprocess
import sys
from pathlib import Path


def get_desktop_environment() -> str:
    """
    Returns the name of the current desktop environment.
    """
    # From https://stackoverflow.com/a/21213358/2624876
    # which takes from:
    # http://stackoverflow.com/questions/2035657/what-is-my-current-desktop-environment
    # and http://ubuntuforums.org/showthread.php?t=652320
    # and http://ubuntuforums.org/showthread.php?t=1139057
    if sys.platform in ["win32", "cygwin"]:
        return "windows"
    elif sys.platform == "darwin":
        return "mac"
    else:  # Most likely either a POSIX system or something not much common
        desktop_session = os.environ.get("DESKTOP_SESSION")
        if (
            desktop_session is not None
        ):  # easier to match if we doesn't have to deal with character cases
            desktop_session = desktop_session.lower()
            if desktop_session in [
                "gnome",
                "unity",
                "cinnamon",
                "mate",
                "xfce4",
                "lxde",
                "fluxbox",
                "blackbox",
                "openbox",
                "icewm",
                "jwm",
                "afterstep",
                "trinity",
                "kde",
            ]:
                return desktop_session
            ## Special cases ##
            # Canonical sets $DESKTOP_SESSION to Lubuntu rather than LXDE if using LXDE.
            # There is no guarantee that they will not do the same with the other desktop environments.
            elif "xfce" in desktop_session or desktop_session.startswith("xubuntu"):
                return "xfce4"
            elif desktop_session.startswith("ubuntustudio"):
                return "kde"
            elif desktop_session.startswith("ubuntu"):
                return "gnome"
            elif desktop_session.startswith("lubuntu"):
                return "lxde"
            elif desktop_session.startswith("kubuntu"):
                return "kde"
            elif desktop_session.startswith("razor"):  # e.g. razorkwin
                return "razor-qt"
            elif desktop_session.startswith("wmaker"):  # e.g. wmaker-common
                return "windowmaker"
        gnome_desktop_session_id = os.environ.get("GNOME_DESKTOP_SESSION_ID")
        if os.environ.get("KDE_FULL_SESSION") == "true":
            return "kde"
        elif gnome_desktop_session_id:
            if "deprecated" not in gnome_desktop_session_id:
                return "gnome2"
        # From http://ubuntuforums.org/showthread.php?t=652320
        elif is_running("xfce-mcs-manage"):
            return "xfce4"
        elif is_running("ksmserver"):
            return "kde"
    return "unknown"


def is_running(process: str) -> bool:
    """Returns whether a process with the given name is (likely) currently running.

    Uses a basic text search, and so may have false positives.
    """
    # From http://www.bloggerpolis.com/2011/05/how-to-check-if-a-process-is-running-using-python/
    # and http://richarddingwall.name/2009/06/18/windows-equivalents-of-ps-and-kill-commands/
    try:  # Linux/Unix
        s = subprocess.Popen(["ps", "axw"], stdout=subprocess.PIPE)
    except:  # Windows
        s = subprocess.Popen(["tasklist", "/v"], stdout=subprocess.PIPE)
    assert s.stdout is not None
    for x in s.stdout:
        # if re.search(process, x):
        if process in str(x):
            return True
    return False


def set_wallpaper(file_loc: str, first_run: bool = True):
    """Sets the wallpaper to the given file location."""
    # From https://stackoverflow.com/a/21213504/2624876
    # I have not personally tested most of this. -- @1j01
    # -----------------------------------------

    # Note: There are two common Linux desktop environments where
    # I have not been able to set the desktop background from
    # command line: KDE, Enlightenment
    desktop_env = get_desktop_environment()
    if desktop_env in ["gnome", "unity", "cinnamon"]:
        # Tested on Ubuntu 22 -- @1j01
        uri = Path(file_loc).as_uri()
        SCHEMA = "org.gnome.desktop.background"
        KEY = "picture-uri"
        # Needed for Ubuntu 22 in dark mode
        # Might be better to set only one or the other, depending on the current theme
        # In the settings it will say "This background selection only applies to the dark style"
        # even if it's set for both, arguably referring to the selection that you can make on that page.
        # -- @1j01
        KEY_DARK = "picture-uri-dark"
        try:
            from gi.repository import Gio  # type: ignore

            gsettings = Gio.Settings.new(SCHEMA)  # type: ignore
            gsettings.set_string(KEY, uri)
            gsettings.set_string(KEY_DARK, uri)
        except Exception:
            # Fallback tested on Ubuntu 22 -- @1j01
            args = ["gsettings", "set", SCHEMA, KEY, uri]
            subprocess.Popen(args)
            args = ["gsettings", "set", SCHEMA, KEY_DARK, uri]
            subprocess.Popen(args)
    elif desktop_env == "mate":
        try:  # MATE >= 1.6
            # info from http://wiki.mate-desktop.org/docs:gsettings
            args = ["gsettings", "set", "org.mate.background", "picture-filename", file_loc]
            subprocess.Popen(args)
        except Exception:  # MATE < 1.6
            # From https://bugs.launchpad.net/variety/+bug/1033918
            args = [
                "mateconftool-2",
                "-t",
                "string",
                "--set",
                "/desktop/mate/background/picture_filename",
                file_loc,
            ]
            subprocess.Popen(args)
    elif desktop_env == "gnome2":  # Not tested
        # From https://bugs.launchpad.net/variety/+bug/1033918
        args = [
            "gconftool-2",
            "-t",
            "string",
            "--set",
            "/desktop/gnome/background/picture_filename",
            file_loc,
        ]
        subprocess.Popen(args)
    ## KDE4 is difficult
    ## see http://blog.zx2c4.com/699 for a solution that might work
    elif desktop_env in ["kde3", "trinity"]:
        # From http://ubuntuforums.org/archive/index.php/t-803417.html
        args = ["dcop", "kdesktop", "KBackgroundIface", "setWallpaper", "0", file_loc, "6"]
        subprocess.Popen(args)
    elif desktop_env == "xfce4":
        # Iterate over all wallpaper-related keys and set to file_loc
        try:
            list_proc = subprocess.run(
                ["xfconf-query", "-c", "xfce4-desktop", "-l"],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                check=False,
            )
            keys = []
            if list_proc.stdout:
                for line in list_proc.stdout.splitlines():
                    line = line.strip()
                    if not line:
                        continue
                    # Common keys: .../last-image and .../image-path
                    if "/last-image" in line or "/image-path" in line:
                        keys.append(line)
            # Fallback: known defaults if none were listed
            if not keys:
                keys = [
                    "/backdrop/screen0/monitorVNC-0/workspace0/last-image",
                    "/backdrop/screen0/monitor0/image-path",
                ]
            for key in keys:
                subprocess.run(
                    [
                        "xfconf-query",
                        "-c",
                        "xfce4-desktop",
                        "-p",
                        key,
                        "-s",
                        file_loc,
                    ],
                    check=False,
                )
        except Exception:
            pass
        # Reload xfdesktop to apply changes
        subprocess.Popen(["xfdesktop", "--reload"])
    elif desktop_env == "razor-qt":  # TODO: implement reload of desktop when possible
        if first_run:
            import configparser

            desktop_conf = configparser.ConfigParser()
            # Development version
            desktop_conf_file = os.path.join(get_config_dir("razor"), "desktop.conf")
            if os.path.isfile(desktop_conf_file):
                config_option = R"screens\1\desktops\1\wallpaper"
            else:
                desktop_conf_file = os.path.join(get_home_dir(), ".razor/desktop.conf")
                config_option = R"desktops\1\wallpaper"
            desktop_conf.read(os.path.join(desktop_conf_file))
            try:
                if desktop_conf.has_option("razor", config_option):  # only replacing a value
                    desktop_conf.set("razor", config_option, file_loc)
                    with open(desktop_conf_file, "w", encoding="utf-8", errors="replace") as f:
                        desktop_conf.write(f)
            except Exception:
                pass
        else:
            # TODO: reload desktop when possible
            pass
    elif desktop_env in ["fluxbox", "jwm", "openbox", "afterstep"]:
        # http://fluxbox-wiki.org/index.php/Howto_set_the_background
        # used fbsetbg on jwm too since I am too lazy to edit the XML configuration
        # now where fbsetbg does the job excellent anyway.
        # and I have not figured out how else it can be set on Openbox and AfterSTep
        # but fbsetbg works excellent here too.
        try:
            args = ["fbsetbg", file_loc]
            subprocess.Popen(args)
        except Exception:
            sys.stderr.write("ERROR: Failed to set wallpaper with fbsetbg!\n")
            sys.stderr.write("Please make sre that You have fbsetbg installed.\n")
    elif desktop_env == "icewm":
        # command found at http://urukrama.wordpress.com/2007/12/05/desktop-backgrounds-in-window-managers/
        args = ["icewmbg", file_loc]
        subprocess.Popen(args)
    elif desktop_env == "blackbox":
        # command found at http://blackboxwm.sourceforge.net/BlackboxDocumentation/BlackboxBackground
        args = ["bsetbg", "-full", file_loc]
        subprocess.Popen(args)
    elif desktop_env == "lxde":
        args = ["pcmanfm", "--set-wallpaper", file_loc, "--wallpaper-mode=scaled"]
        subprocess.Popen(args)
    elif desktop_env == "windowmaker":
        # From http://www.commandlinefu.com/commands/view/3857/set-wallpaper-on-windowmaker-in-one-line
        args = ["wmsetbg", "-s", "-u", file_loc]
        subprocess.Popen(args)
    # elif desktop_env == "enlightenment": # I have not been able to make it work on e17. On e16 it would have been something in this direction
    #     args = ["enlightenment_remote", "-desktop-bg-add", "0", "0", "0", "0", file_loc]
    #     subprocess.Popen(args)
    elif desktop_env == "windows":
        # From https://stackoverflow.com/questions/1977694/change-desktop-background
        # Tested on Windows 10. -- @1j01
        import ctypes

        SPI_SETDESKWALLPAPER = 20
        ctypes.windll.user32.SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, file_loc, 0)  # type: ignore
    elif desktop_env == "mac":
        # From https://stackoverflow.com/questions/431205/how-can-i-programatically-change-the-background-in-mac-os-x
        try:
            # Tested on macOS 10.14.6 (Mojave) -- @1j01
            assert (
                sys.platform == "darwin"
            )  # ignore `Import "appscript" could not be resolved` for other platforms
            from appscript import app, mactypes

            app("Finder").desktop_picture.set(mactypes.File(file_loc))
        except ImportError:
            # Tested on macOS 10.14.6 (Mojave) -- @1j01
            # import subprocess
            # SCRIPT = f"""/usr/bin/osascript<<END
            # tell application "Finder" to set desktop picture to POSIX file "{file_loc}"
            # END"""
            # subprocess.Popen(SCRIPT, shell=True)

            # Safer version, avoiding string interpolation,
            # to protect against command injection (both in the shell and in AppleScript):
            OSASCRIPT = """
            on run (clp)
                if clp's length is not 1 then error "Incorrect Parameters"
                local file_loc
                set file_loc to clp's item 1
                tell application "Finder" to set desktop picture to POSIX file file_loc
            end run
            """
            subprocess.Popen(["osascript", "-e", OSASCRIPT, "--", file_loc])
    else:
        if first_run:  # don't spam the user with the same message over and over again
            sys.stderr.write(
                "Warning: Failed to set wallpaper. Your desktop environment is not supported."
            )
            sys.stderr.write(f"You can try manually to set your wallpaper to {file_loc}")
        return False
    return True


def get_config_dir(app_name: str) -> str:
    """Returns the configuration directory for the given application name."""
    if "XDG_CONFIG_HOME" in os.environ:
        config_home = os.environ["XDG_CONFIG_HOME"]
    elif "APPDATA" in os.environ:  # On Windows
        config_home = os.environ["APPDATA"]
    else:
        try:
            from xdg import BaseDirectory

            config_home = BaseDirectory.xdg_config_home
        except ImportError:  # Most likely a Linux/Unix system anyway
            config_home = os.path.join(get_home_dir(), ".config")
    config_dir = os.path.join(config_home, app_name)
    return config_dir


def get_home_dir() -> str:
    """Returns the home directory of the current user."""
    return os.path.expanduser("~")
