import sys
import os
import logging
import glob
from send2trash import send2trash
from logging import FileHandler
from logging.handlers import RotatingFileHandler
from contextlib import redirect_stdout
from io import StringIO

# Redirect stdout and stderr to log file
class Logger(object):
    def __init__(self,logger):
        self.logger = logger
        self.stdout = sys.stdout
        sys.stdout = self
        self.msg = ""

        # handle exceptions and errors
        sys.excepthook = self.excepthook

    def __del__(self):
        sys.stdout = self.stdout
    def write(self, data):
        if data != '\n':
            self.msg += data
        else:
            self.logger.info(self.msg)
            self.msg = ""
        self.stdout.write(data)
    def flush(self):
        self.stdout.flush()

    # catch exceptions and log to file
    def excepthook(self, exctype, value, traceback):
        self.logger.error("Uncaught exception", exc_info=(exctype, value, traceback))
        self.stdout.excepthook(exctype, value, traceback)
    
    # log errors
    def error(self, msg):
        self.logger.error(msg)


# Flask logging
import argparse
parser = argparse.ArgumentParser()
parser.add_argument("--log_file", help="log file path", type=str,
                    default=os.path.join(os.path.dirname(__file__), "server.log"))
#port
parser.add_argument("--port", help="port", type=int, default=5000)

args = parser.parse_args()
log_file = args.log_file

logging.basicConfig(filename='server.log',level=logging.DEBUG, filemode='w' )

# logging.basicConfig(filename=log_file,level=logging.INFO, filemode='w' )
logger = logging.getLogger('werkzeug')
custom_logger = Logger(logger)
# check if argument log_file is passed

import ctypes
import platform
import shlex
import subprocess, signal
from pathlib import Path
from typing import Any, Optional
from typing import List, Dict, Tuple

import Xlib
import lxml.etree
import pyautogui
import requests
from PIL import Image
from Xlib import display, X
from flask import Flask, request, jsonify, send_file, abort  # , send_from_directory
from lxml.etree import _Element
import traceback
import time

platform_name: str = platform.system()

if platform_name=="Linux":
    import pyatspi
    from pyatspi import Accessible, StateType, STATE_SHOWING
    from pyatspi import Action as ATAction
    from pyatspi import Component, Document
    from pyatspi import Text as ATText
    from pyatspi import Value as ATValue

    BaseWrapper = Any
elif platform_name=="Windows":
    from pywinauto import Desktop
    from pywinauto.base_wrapper import BaseWrapper

    Accessible = Any

from pyxcursor import Xcursor


from computer import Computer, WindowManager
# global computer

from human import Human
human = Human()

app = Flask(__name__)

pyautogui.PAUSE = 0
pyautogui.DARWIN_CATCH_UP_TIME = 0

logger = app.logger
computer = Computer(logger)
recording_process = None  # fixme: this is a temporary solution for recording, need to be changed to support multiple-process

recording_path = os.path.join(os.path.dirname(__file__), "recordings")
#create tmp directory if not exists
os.makedirs(recording_path, exist_ok=True)
recording_path = os.path.join(recording_path, "recording.mp4")

print("recording dir set to", recording_path)

# recording_path = "/tmp/recording.mp4"

@app.errorhandler(Exception)
def handle_exception(e):
    logger.error("\n" + traceback.format_exc() + "\n")

    return jsonify({"status": "error", "message": str(e)}), 500
  
@app.route('/update_computer', methods=['POST'])  
def update_computer(): 
    print("STARTED UPDATING COMPUTER")
    # print(request)
    data = request.json  # get data from POST request  
    
    # print(data)

    rects = data.get('rects')  
     
    window_rect = data.get('window_rect')  

    

    import base64  
    from PIL import Image  
    import io  

    def base64_str_to_image(img_str):  
        img_bytes = base64.b64decode(img_str)  
        img = Image.open(io.BytesIO(img_bytes))  
        return img  

    screenshot_str = data.get('screenshot')  
    screenshot = base64_str_to_image(screenshot_str)  
    # screenshot.save("//host.lan/Data/models/test.png")

    scale = data.get('scale')  
    clipboard_content = data.get('clipboard_content')  
    swap_ctrl_alt = data.get('swap_ctrl_alt', False)  
    computer.update(rects=rects, window_rect=window_rect, screenshot=screenshot, scale=scale, clipboard_content=clipboard_content, swap_ctrl_alt=swap_ctrl_alt)  
    print("mouse window rect", computer.mouse.window_rect)
    print("FINISHED UPDATING COMPUTER")
    return jsonify(success=True)  # Return a success message  


@app.route('/execute_windows', methods=['POST'])  
def execute_command_windows():  
    # data = request.json 
    data = request.get_json() 
    # shell = data.get('shell', False)  
    command = data.get('command')  
    print(command)

    try:  
        # exec(command_with_computer)  
        # exec(command_with_computer, globals())  # Use globals() to make 'computer' available in exec()
        
        f = StringIO()
        with redirect_stdout(f):
            exec(command, {'computer': computer, 'human': human})  
        s = f.getvalue()
        if "error" in s.lower():
            raise Exception(s)
        
        return jsonify({  
            'status': 'success',  
        })  
    except Exception as e:
        logger.error(f"Failed to execute command: {command}. Error: {e}")
        logger.error("\n" + traceback.format_exc() + "\n")

        return jsonify({  
            'status': 'error',  
            'message': str(e)  
        }), 500  
    
@app.route('/setup/execute', methods=['POST'])
@app.route('/execute', methods=['POST'])
def execute_command():
    
    data = request.json
    # The 'command' key in the JSON request should contain the command to be executed.
    shell = data.get('shell', False)
    command = data.get('command', "" if shell else [])

    if isinstance(command, str) and not shell:
        command = shlex.split(command)

    # Expand user directory
    for i, arg in enumerate(command):
        if arg.startswith("~/"):
            command[i] = os.path.expanduser(arg)

    # Execute the command without any safety checks.
    try:
        result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, shell=shell, text=True, timeout=120)
        return jsonify({
            'status': 'success',
            'output': result.stdout,
            'error': result.stderr,
            'returncode': result.returncode
        })
    except Exception as e:
        logger.error("\n" + traceback.format_exc() + "\n")
        return jsonify({
            'status': 'error',
            'message': str(e)
        }), 500
        # return jsonify({
        #     'status': 'error',
        #     'message': str(e),
        #     'shell': str(shell),
        #     'command': command,
        #     'data': data
        # }), 500    


def _get_machine_architecture() -> str:
    """ Get the machine architecture, e.g., x86_64, arm64, aarch64, i386, etc.
    """
    architecture = platform.machine().lower()
    if architecture in ['amd32', 'amd64', 'x86', 'x86_64', 'x86-64', 'x64', 'i386', 'i686']:
        return 'amd'
    elif architecture in ['arm64', 'aarch64', 'aarch32']:
        return 'arm'
    else:
        return 'unknown'

@app.route('/probe', methods=['GET'])
def probe_endpoint():
    # This endpoint simply returns a status 200 response with a custom message
    return jsonify({"status": "Probe successful", "message": "Service is operational"}), 200

# Used with the --prepare-image flag gracefully shutdown the VM at the first setup
@app.route('/shutdown', methods=['POST'])
def shutdown_endpoint():
    logger.info('/shutdown')

    os.system("shutdown /s /t 2")

    return jsonify({"status": "Shutdown started successfully", "message": "Shutdown started successfully"}), 200

@app.route('/setup/launch', methods=["POST"])
def launch_app():
    data = request.json
    # log the request data 
    logger.info('/setup/launch')
    logger.info(data)
    shell = data.get("shell", False)
    shell = True

    command: List[str] = data.get("command", "" if shell else [])

    if isinstance(command, str) and not shell:
        command = shlex.split(command)

    # Expand user directory
    for i, arg in enumerate(command):
        if arg.startswith("~/"):
            command[i] = os.path.expanduser(arg)

    try:
        user_platform = platform.system()
        if 'google-chrome' in command and user_platform == 'Windows':
            index = command.index('google-chrome')
            command[index] = 'chrome'


        if 'google-chrome' in command and _get_machine_architecture() == 'arm':
            index = command.index('google-chrome')
            command[index] = 'chromium-browser' # arm64 chrome is not available yet, can only use chromium
        subprocess.Popen(command, shell=shell)
        return "{:} launched successfully".format(command if shell else " ".join(command))
    except Exception as e:
        

        logger.error("\n" + traceback.format_exc() + "\n")
        # return jsonify({"status": "error", "message": str(e)}), 500
        return jsonify({"status": "error", 
                        "message": str(e),
                        'shell': str(shell),
                        'command': command,
                        'data': data
                        }), 500    


@app.route('/screenshot', methods=['GET'])
def capture_screen_with_cursor():
    # DEPRECATED: if you want to capture the cursor, use the vm_controller.py screen capture function instead

    file_path = os.path.join(os.path.dirname(__file__), "screenshots", "screenshot.png")
    user_platform = platform.system()

    # Ensure the screenshots directory exists
    os.makedirs(os.path.dirname(file_path), exist_ok=True)

    # fixme: This is a temporary fix for the cursor not being captured on Windows and Linux
    if user_platform == "Windows":
        cursor_path = os.path.join(os.path.dirname(__file__), "cursor.png")
        screenshot = pyautogui.screenshot()
        cursor_x, cursor_y = pyautogui.position()
        cursor = Image.open(cursor_path)
        # make the cursor smaller
        cursor = cursor.resize((int(cursor.width / 1.5), int(cursor.height / 1.5)))
        screenshot.paste(cursor, (cursor_x, cursor_y), cursor)
        screenshot.save(file_path)
    elif user_platform == "Linux":
        cursor_obj = Xcursor()
        imgarray = cursor_obj.getCursorImageArrayFast()
        cursor_img = Image.fromarray(imgarray)
        screenshot = pyautogui.screenshot()
        cursor_x, cursor_y = pyautogui.position()
        screenshot.paste(cursor_img, (cursor_x, cursor_y), cursor_img)
        screenshot.save(file_path)
    elif user_platform == "Darwin":  # (Mac OS)
        # Use the screencapture utility to capture the screen with the cursor
        subprocess.run(["screencapture", "-C", file_path])
    else:
        logger.warning(f"The platform you're using ({user_platform}) is not currently supported")

    return send_file(file_path, mimetype='image/png')


def _has_active_terminal(desktop: Accessible) -> bool:
    """ A quick check whether the terminal window is open and active.
    """
    for app in desktop:
        if app.getRoleName() == "application" and app.name == "gnome-terminal-server":
            for frame in app:
                if frame.getRoleName() == "frame" and frame.getState().contains(pyatspi.STATE_ACTIVE):
                    return True
    return False


@app.route('/terminal', methods=['GET'])
def get_terminal_output():
    user_platform = platform.system()
    output: Optional[str] = None
    try:
        if user_platform == "Linux":
            desktop: Accessible = pyatspi.Registry.getDesktop(0)
            if _has_active_terminal(desktop):
                desktop_xml: _Element = _create_atspi_node(desktop)
                # 1. the terminal window (frame of application is st:active) is open and active
                # 2. the terminal tab (terminal status is st:focused) is focused
                xpath = '//application[@name="gnome-terminal-server"]/frame[@st:active="true"]//terminal[@st:focused="true"]'
                terminals: List[_Element] = desktop_xml.xpath(xpath, namespaces=_accessibility_ns_map)
                output = terminals[0].text.rstrip() if len(terminals) == 1 else None
        else:  # windows and macos platform is not implemented currently
            # raise NotImplementedError
            return "Currently not implemented for platform {:}.".format(platform.platform()), 500
        return jsonify({"output": output, "status": "success"})
    except Exception as e:
        logger.error("Failed to get terminal output. Error: %s", e)
        logger.error("\n" + traceback.format_exc() + "\n")
        return jsonify({"status": "error", "message": str(e)}), 500

import win32gui
from screeninfo import get_monitors 
from PIL import ImageGrab
import io
import base64  
def obs_winagent():      
    window = win32gui.GetForegroundWindow()
    try:    
        window_title = pyautogui.getActiveWindowTitle()
    except Exception as e:
        window_title = '' # no window title active

    if window_title=='Program Manager' or window_title=='':    
        monitors = get_monitors()  
        for monitor in monitors:  
            if monitor.is_primary:  
                main_monitor = monitor  
                break  
        rect = (main_monitor.x, main_monitor.y, main_monitor.width, main_monitor.height)    
        window_title = "Desktop"    
    else:    
        rect = win32gui.GetWindowRect(window)  # x0, y0, x1, y1 from the screen in global coordinates  
    image = ImageGrab.grab(rect)    
    window_manager = WindowManager()
    window_names = window_manager.find_open_applications()
    window_names_str = "\n".join(window_names)
    if computer.clipboard is not None:
        computer_clipboard = computer.clipboard.clipboard_content
    else:
        computer_clipboard = None
    human_input = human.get_past_input()
    return image, window_title, rect, window_names_str, computer_clipboard, human_input
    
@app.route('/obs_winagent', methods=['GET'])
def get_obs_winagent():
    try:  
        image, window_title, rect, window_names_str, computer_clipboard, human_input = obs_winagent()  
        img_bytes = io.BytesIO()  
        image.save(img_bytes, format='PNG')  
        img_bytes.seek(0)  
        img_base64 = base64.b64encode(img_bytes.read()).decode('utf-8')  
        return jsonify({"image": img_base64, "window_title": window_title, "rect": rect, "window_names_str": window_names_str, "computer_clipboard": computer_clipboard, "human_input": human_input, "status": "success"})  
    except Exception as e:  
        logger.error("Failed to get OBS window agent. Error: %s", e)
        logger.error("\n" + traceback.format_exc() + "\n")
        return jsonify({"status": "error", "message": str(e)}), 500  



_accessibility_ns_map = { "st": "uri:deskat:state.at-spi.gnome.org"
                        , "attr": "uri:deskat:attributes.at-spi.gnome.org"
                        , "cp": "uri:deskat:component.at-spi.gnome.org"
                        , "doc": "uri:deskat:document.at-spi.gnome.org"
                        , "docattr": "uri:deskat:attributes.document.at-spi.gnome.org"
                        , "txt": "uri:deskat:text.at-spi.gnome.org"
                        , "val": "uri:deskat:value.at-spi.gnome.org"
                        , "act": "uri:deskat:action.at-spi.gnome.org"
                        , "win": "uri:deskat:uia.windows.microsoft.org"
                        }


def _create_atspi_node(node: Accessible, depth: int = 0, flag: Optional[str] = None) -> _Element:
    #  function _create_atspi_node {{{ # 
    if node.getRoleName() == "document spreadsheet":
        flag = "calc"
    if node.getRoleName() == "application" and node.name=="Thunderbird":
        flag = "thunderbird"

    attribute_dict: Dict[str, Any] = {"name": node.name}

    #  States {{{ # 
    states: List[StateType] = node.getState().get_states()
    for st in states:
        state_name: str = StateType._enum_lookup[st]
        if len(state_name.split("_", maxsplit=1)[1].lower()) == 0:
            continue
        attribute_dict[
            "{{{:}}}{:}".format(_accessibility_ns_map["st"], state_name.split("_", maxsplit=1)[1].lower())] = "true"
    #  }}} States # 

    #  Attributes {{{ # 
    attributes: List[str] = node.getAttributes()
    for attrbt in attributes:
        attribute_name: str
        attribute_value: str
        attribute_name, attribute_value = attrbt.split(":", maxsplit=1)
        if len(attribute_name) == 0:
            continue
        attribute_dict["{{{:}}}{:}".format(_accessibility_ns_map["attr"], attribute_name)] = attribute_value
    #  }}} Attributes # 

    #  Component {{{ # 
    try:
        component: Component = node.queryComponent()
    except NotImplementedError:
        pass
    else:
        attribute_dict["{{{:}}}screencoord".format(_accessibility_ns_map["cp"])] = str(
            component.getPosition(pyatspi.XY_SCREEN))
        attribute_dict["{{{:}}}windowcoord".format(_accessibility_ns_map["cp"])] = str(
            component.getPosition(pyatspi.XY_WINDOW))
        attribute_dict["{{{:}}}parentcoord".format(_accessibility_ns_map["cp"])] = str(
            component.getPosition(pyatspi.XY_PARENT))
        attribute_dict["{{{:}}}size".format(_accessibility_ns_map["cp"])] = str(component.getSize())
    #  }}} Component # 

    #  Document {{{ # 
    try:
        document: Document = node.queryDocument()
    except NotImplementedError:
        pass
    else:
        attribute_dict["{{{:}}}locale".format(_accessibility_ns_map["doc"])] = document.getLocale()
        attribute_dict["{{{:}}}pagecount".format(_accessibility_ns_map["doc"])] = str(document.getPageCount())
        attribute_dict["{{{:}}}currentpage".format(_accessibility_ns_map["doc"])] = str(document.getCurrentPageNumber())
        for attrbt in document.getAttributes():
            attribute_name: str
            attribute_value: str
            attribute_name, attribute_value = attrbt.split(":", maxsplit=1)
            if len(attribute_name) == 0:
                continue
            attribute_dict["{{{:}}}{:}".format(_accessibility_ns_map["docattr"], attribute_name)] = attribute_value
    #  }}} Document # 

    #  Text {{{ # 
    try:
        text_obj: ATText = node.queryText()
    except NotImplementedError:
        pass
    else:
        # only text shown on current screen is available
        # attribute_dict["txt:text"] = text_obj.getText(0, text_obj.characterCount)
        text: str = text_obj.getText(0, text_obj.characterCount)
        #if flag=="thunderbird":
        # appeard in thunderbird (uFFFC) (not only in thunderbird), "Object
        # Replacement Character" in Unicode, "used as placeholder in text for
        # an otherwise unspecified object; uFFFD is another "Replacement
        # Character", just in case
        text = text.replace("\ufffc", "").replace("\ufffd", "")
    #  }}} Text # 

    #  Image {{{ # 
    try:
        node.queryImage()
    except NotImplementedError:
        pass
    else:
        attribute_dict["image"] = "true"
    #  }}} Image # 

    #  Selection {{{ # 
    try:
        node.querySelection()
    except NotImplementedError:
        pass
    else:
        attribute_dict["selection"] = "true"
    #  }}} Selection # 

    #  Value {{{ # 
    try:
        value: ATValue = node.queryValue()
    except NotImplementedError:
        pass
    else:
        try:
            attribute_dict["{{{:}}}value".format(_accessibility_ns_map["val"])] = str(value.currentValue)
        except:
            pass
        try:
            attribute_dict["{{{:}}}min".format(_accessibility_ns_map["val"])] = str(value.minimumValue)
        except:
            pass
        try:
            attribute_dict["{{{:}}}max".format(_accessibility_ns_map["val"])] = str(value.maximumValue)
        except:
            pass
        try:
            attribute_dict["{{{:}}}step".format(_accessibility_ns_map["val"])] = str(value.minimumIncrement)
        except:
            pass
    #  }}} Value # 

    #  Action {{{ # 
    try:
        action: ATAction = node.queryAction()
    except NotImplementedError:
        pass
    else:
        for i in range(action.nActions):
            action_name: str = action.getName(i).replace(" ", "-")
            attribute_dict["{{{:}}}{:}_desc" \
                .format(_accessibility_ns_map["act"]
                        , action_name
                        )
            ] = action.getDescription(i)
            attribute_dict["{{{:}}}{:}_kb" \
                .format(_accessibility_ns_map["act"]
                        , action_name
                        )
            ] = action.getKeyBinding(i)
    #  }}} Action #

    if node.getRoleName().strip() == "":
        node_role_name = "unknown"
    else:
        node_role_name = node.getRoleName().replace(" ", "-")

    xml_node = lxml.etree.Element(
        node_role_name,
        attrib=attribute_dict,
        nsmap=_accessibility_ns_map
    )
    if "text" in locals() and len(text) > 0:
        xml_node.text = text

    # HYPERPARAMETER
    if depth==50:
        logger.warning("Max depth reached")
        return xml_node

    if flag=="calc" and node_role_name=="table":
        # Maximum column: 1024 if ver<=7.3 else 16384
        # Maximum row: 104 8576
        # Maximun sheet: 1 0000

        version_str: str = subprocess.run("libreoffice --version", shell=True, text=True, stdout=subprocess.PIPE).stdout
        version_str = version_str.split()[1]
        version_tuple: Tuple[int] = tuple(map(int, version_str.split(".")))
        MAXIMUN_COLUMN = 1024 if version_tuple<(7, 4) else 16384
        MAX_ROW = 104_8576

        index_base = 0
        first_showing = False
        column_base = None
        for r in range(MAX_ROW):
            #logger.warning(r)
            for clm in range(column_base or 0, MAXIMUN_COLUMN):
                child_node: Accessible = node[index_base+clm]
                showing: bool = child_node.getState().contains(STATE_SHOWING)
                if showing:
                    child_node: _Element = _create_atspi_node(child_node, depth+1, flag)
                    if not first_showing:
                        column_base = clm
                        first_showing = True
                    xml_node.append(child_node)
                elif first_showing and column_base is not None or clm>=500:
                    break
            if first_showing and clm==column_base or not first_showing and r>=500:
                break
            index_base += MAXIMUN_COLUMN
        return xml_node
    else:
        try:
            for i, ch in enumerate(node):
                # HYPERPARAMETER
                if i>=1025:
                    logger.warning("Max width reached")
                    break
                xml_node.append(_create_atspi_node(ch, depth+1, flag))
        except:
            logger.warning("Error occurred during children traversing. Has Ignored. Node: %s", lxml.etree.tostring(xml_node, encoding="unicode"))
        return xml_node
    #  }}} function _create_atspi_node # 

def _create_pywinauto_node(node: BaseWrapper, depth: int = 0, flag: Optional[str] = None) -> _Element:
    #  function _create_pywinauto_node {{{ # 
    #element_info: ElementInfo = node.element_info
    attribute_dict: Dict[str, Any] = {"name": node.element_info.name}

    #  States {{{ # 
    try:
        attribute_dict["{{{:}}}enabled".format(_accessibility_ns_map["st"])] = str(node.is_enabled()).lower()
    except:
        pass
    try:
        attribute_dict["{{{:}}}visible".format(_accessibility_ns_map["st"])] = str(node.is_visible()).lower()
    except:
        pass
    try:
        attribute_dict["{{{:}}}active".format(_accessibility_ns_map["st"])] = str(node.is_active()).lower()
    except:
        pass

    if hasattr(node, "is_minimized"):
        try:
            attribute_dict["{{{:}}}minimized".format(_accessibility_ns_map["st"])] = str(node.is_minimized()).lower()
        except:
            pass
    if hasattr(node, "is_maximized"):
        try:
            attribute_dict["{{{:}}}maximized".format(_accessibility_ns_map["st"])] = str(node.is_maximized()).lower()
        except:
            pass
    if hasattr(node, "is_normal"):
        try:
            attribute_dict["{{{:}}}normal".format(_accessibility_ns_map["st"])] = str(node.is_normal()).lower()
        except:
            pass

    if hasattr(node, "is_unicode"):
        try:
            attribute_dict["{{{:}}}unicode".format(_accessibility_ns_map["st"])] = str(node.is_unicode()).lower()
        except:
            pass

    if hasattr(node, "is_collapsed"):
        try:
            attribute_dict["{{{:}}}collapsed".format(_accessibility_ns_map["st"])] = str(node.is_collapsed()).lower()
        except:
            pass
    if hasattr(node, "is_checkable"):
        try:
            attribute_dict["{{{:}}}checkable".format(_accessibility_ns_map["st"])] = str(node.is_checkable()).lower()
        except:
            pass
    if hasattr(node, "is_checked"):
        try:
            attribute_dict["{{{:}}}checked".format(_accessibility_ns_map["st"])] = str(node.is_checked()).lower()
        except:
            pass
    if hasattr(node, "is_focused"):
        try:
            attribute_dict["{{{:}}}focused".format(_accessibility_ns_map["st"])] = str(node.is_focused()).lower()
        except:
            pass
    if hasattr(node, "is_keyboard_focused"):
        try:
            attribute_dict["{{{:}}}keyboard_focused".format(_accessibility_ns_map["st"])] = str(node.is_keyboard_focused()).lower()
        except:
            pass
    if hasattr(node, "is_selected"):
        try:
            attribute_dict["{{{:}}}selected".format(_accessibility_ns_map["st"])] = str(node.is_selected()).lower()
        except:
            pass
    if hasattr(node, "is_selection_required"):
        try:
            attribute_dict["{{{:}}}selection_required".format(_accessibility_ns_map["st"])] = str(node.is_selection_required()).lower()
        except:
            pass
    if hasattr(node, "is_pressable"):
        try:
            attribute_dict["{{{:}}}pressable".format(_accessibility_ns_map["st"])] = str(node.is_pressable()).lower()
        except:
            pass
    if hasattr(node, "is_pressed"):
        try:
            attribute_dict["{{{:}}}pressed".format(_accessibility_ns_map["st"])] = str(node.is_pressed()).lower()
        except:
            pass

    if hasattr(node, "is_expanded"):
        try:
            attribute_dict["{{{:}}}expanded".format(_accessibility_ns_map["st"])] = str(node.is_expanded()).lower()
        except:
            pass
    if hasattr(node, "is_editable"):
        try:
            attribute_dict["{{{:}}}editable".format(_accessibility_ns_map["st"])] = str(node.is_editable()).lower()
        except:
            pass
    #  }}} States # 

    #  Component {{{ # 
    rectangle = node.rectangle()
    attribute_dict["{{{:}}}screencoord".format(_accessibility_ns_map["cp"])] = "({:d}, {:d})".format(rectangle.left, rectangle.top)
    attribute_dict["{{{:}}}size".format(_accessibility_ns_map["cp"])] = "({:d}, {:d})".format(rectangle.width(), rectangle.height())
    #  }}} Component # 

    #  Text {{{ # 
    text: str = node.window_text()
    if text==attribute_dict["name"]:
        text = ""
    #if hasattr(node, "texts"):
        #texts: List[str] = node.texts()[1:]
        #texts: Iterable[str] = map(lambda itm: itm if isinstance(itm, str) else "".join(itm), texts)
    #text += "\n".join(texts)
    #text = text.strip()
    #  }}} Text # 

    #  Selection {{{ # 
    if hasattr(node, "select"):
        attribute_dict["selection"] = "true"
    #  }}} Selection # 

    #  Value {{{ # 
    if hasattr(node, "get_step"):
        try:
            attribute_dict["{{{:}}}step".format(_accessibility_ns_map["val"])] = str(node.get_step())
        except:
            pass
    if hasattr(node, "value"):
        try:
            attribute_dict["{{{:}}}value".format(_accessibility_ns_map["val"])] = str(node.value())
        except:
            pass
    if hasattr(node, "get_value"):
        try:
            attribute_dict["{{{:}}}value".format(_accessibility_ns_map["val"])] = str(node.get_value())
        except:
            pass
    elif hasattr(node, "get_position"):
        try:
            attribute_dict["{{{:}}}value".format(_accessibility_ns_map["val"])] = str(node.get_position())
        except:
            pass
    if hasattr(node, "min_value"):
        try:
            attribute_dict["{{{:}}}min".format(_accessibility_ns_map["val"])] = str(node.min_value())
        except:
            pass
    elif hasattr(node, "get_range_min"):
        try:
            attribute_dict["{{{:}}}min".format(_accessibility_ns_map["val"])] = str(node.get_range_min())
        except:
            pass
    if hasattr(node, "max_value"):
        try:
            attribute_dict["{{{:}}}max".format(_accessibility_ns_map["val"])] = str(node.max_value())
        except:
            pass
    elif hasattr(node, "get_range_max"):
        try:
            attribute_dict["{{{:}}}max".format(_accessibility_ns_map["val"])] = str(node.get_range_max())
        except:
            pass
    #  }}} Value # 

    attribute_dict["{{{:}}}class".format(_accessibility_ns_map["win"])] = str(type(node))

    node_role_name: str = node.class_name().lower().replace(" ", "-")
    node_role_name = "".join( map( lambda ch: ch if ch.isidentifier()\
                                                 or ch in {"-"}\
                                                 or ch.isalnum()
                                               else "-"
                                 , node_role_name
                                 )
                            )
    if node_role_name.strip() == "":
        node_role_name = "unknown"
    if not node_role_name[0].isalpha():
        node_role_name = "tag" + node_role_name

    xml_node = lxml.etree.Element(
        node_role_name,
        attrib=attribute_dict,
        nsmap=_accessibility_ns_map
    )
    if text is not None and len(text)>0 and text!=attribute_dict["name"]:
        xml_node.text = text

    # HYPERPARAMETER
    if depth==50:
        logger.warning("Max depth reached")
        #print("Max depth reached")
        return xml_node

    for i, ch in enumerate(node.children()):
        # HYPERPARAMETER
        if i>=2048:
            logger.warning("Max width reached")
            #print("Max width reached")
            break
        xml_node.append(_create_pywinauto_node(ch, depth+1, flag))
    return xml_node
    #  }}} function _create_pywinauto_node # 

@app.route("/accessibility", methods=["GET"])
def get_accessibility_tree():
    os_name: str = platform.system()

    # AT-SPI works for KDE as well
    if os_name == "Linux":
        desktop: Accessible = pyatspi.Registry.getDesktop(0)
        desktop_xml: _Element = _create_atspi_node(desktop, 0)
        return jsonify({"AT": lxml.etree.tostring(desktop_xml, encoding="unicode")})

    elif os_name == "Windows":
        # Windows AT may be read through `pywinauto` module, however, two different backends `win32` and `uia` are supported and different results may be returned
        # win32 is much faster, but we need to use uia for the chrome evals
        backend: str = request.args.get("backend", "win32")
        if backend not in {"win32", "uia"}:
            return "Invalid backend specified", 400
        desktop: Desktop = Desktop(backend=backend)

        xml_node = lxml.etree.Element("desktop", nsmap=_accessibility_ns_map)
        for wnd in desktop.windows():
            logger.debug("Win UIA AT parsing: %s(%d)", wnd.element_info.name, len(wnd.children()))
            node: _Element = _create_pywinauto_node(wnd, 1)
            xml_node.append(node)
        return jsonify({"AT": lxml.etree.tostring(xml_node, encoding="unicode")})
    else:
        return "Currently not implemented for platform {:}.".format(platform.platform()), 500


@app.route('/screen_size', methods=['POST'])
def get_screen_size():
    if platform_name=="Linux":
        d = display.Display()
        screen_width = d.screen().width_in_pixels
        screen_height = d.screen().height_in_pixels
    elif platform_name=="Windows":
        user32 = ctypes.windll.user32
        screen_width: int = user32.GetSystemMetrics(0)
        screen_height: int = user32.GetSystemMetrics(1)
    return jsonify(
        {
            "width": screen_width,
            "height": screen_height
        }
    )


@app.route('/window_size', methods=['POST'])
def get_window_size():
    if 'app_class_name' in request.form:
        app_class_name = request.form['app_class_name']
    else:
        return jsonify({"error": "app_class_name is required"}), 400

    d = display.Display()
    root = d.screen().root
    window_ids = root.get_full_property(d.intern_atom('_NET_CLIENT_LIST'), X.AnyPropertyType).value

    for window_id in window_ids:
        try:
            window = d.create_resource_object('window', window_id)
            wm_class = window.get_wm_class()

            if wm_class is None:
                continue

            if app_class_name.lower() in [name.lower() for name in wm_class]:
                geom = window.get_geometry()
                return jsonify(
                    {
                        "width": geom.width,
                        "height": geom.height
                    }
                )
        except Xlib.error.XError:  # Ignore windows that give an error
            continue
    return None

@app.route('/desktop_path', methods=['POST'])
def get_desktop_path():
    # Get the home directory in a platform-independent manner using pathlib
    home_directory = str(Path.home())

    # Determine the desktop path based on the operating system
    desktop_path = {
        "Windows": os.path.join(home_directory, "Desktop"),
        "Darwin": os.path.join(home_directory, "Desktop"),  # macOS
        "Linux": os.path.join(home_directory, "Desktop")
    }.get(platform.system(), None)

    # Check if the operating system is supported and the desktop path exists
    if desktop_path and os.path.exists(desktop_path):
        return jsonify(desktop_path=desktop_path)
    else:
        return jsonify(error="Unsupported operating system or desktop path not found"), 404

@app.route('/documents_path', methods=['POST'])
def get_documents_path():
    # Get the home directory in a platform-independent manner using pathlib
    home_directory = str(Path.home())

    # Determine the documents path based on the operating system
    documents_path = {
        "Windows": os.path.join(home_directory, "Documents"),
        "Darwin": os.path.join(home_directory, "Documents"),  # macOS
        "Linux": os.path.join(home_directory, "Documents")
    }.get(platform.system(), None)

    # Check if the operating system is supported and the documents path exists
    if documents_path and os.path.exists(documents_path):
        return jsonify(documents_path=documents_path)
    else:
        return jsonify(error="Unsupported operating system or documents path not found"), 400

@app.route('/setup/create_folder', methods=['POST'])
def create_folder():
    data = request.json
    print(data)
    if "path" not in data:
        return jsonify({"error": "path required"}), 400

    path: str = data["path"]
    try:
        os.makedirs(path)
        return jsonify({'status': 'success', 'message': f'{path} created.'}), 200
    except FileExistsError:
        return jsonify({'status': 'success', 'message': f'{path} already exists.'}), 200
    except Exception as e:
        return jsonify({'status': 'error', 'message': f'Failed to create {path}.'}), 400
    
@app.route('/setup/create_file', methods=['POST'])
def create_file():
    """
    Create a file at the specified path with the specified content.
    If 'path' is just the file name, it will be created in the user's desktop directory.
    """
    data = request.json
    print(data)
    if "path" not in data:
        return jsonify({"error": "path required"}), 400
    
    # Absolute path to the file
    path = Path(data["path"]).resolve()
    print("Resolved path:", path)

    if path.is_dir():
        return jsonify({"status": "error", "message": "path is not a file"}), 400
    
    content = data.get("content", "")
    try:
        root_path = path.parent
        if str(root_path) == "":
            root_path = get_desktop_path()
            path = os.path.join(root_path, path.name)
        
        os.makedirs(root_path, exist_ok=True)
        with open(path, "w") as f:
            f.write(content)
        return jsonify({'status': 'success', 'message': f'{path} created.'}), 200
    except Exception as e:
        print(e)
        return jsonify({'status': 'error', 'message': f'Failed to create {path}.'}), 400
    
@app.route("/setup/recycle", methods=["POST"])
def recycle_file():
    data = request.json
    if 'path' not in data:
        return jsonify(error="Missing 'path' parameter"), 400
    try:
        send2trash(data['path'])
        return jsonify({"status": "success", "message": f"{data['path']} recycled"}), 200
    except:
        return jsonify({"status": "error", "message": f"Failed to recycle {data['path']}"}), 400
    
@app.route('/folder_exists', methods=['POST'])
def get_folder_exists():
    data = request.json
    print(data)
    if "folder_name" not in data:
        return jsonify({"error": "folder_name required"}), 400
    if "path" not in data:
        return jsonify({"error": "path required"}), 400

    folder_name: str = data["folder_name"]
    path: str = data["path"]
    folder_path = os.path.join(path, folder_name)
    if os.path.exists(folder_path) and os.path.isdir(folder_path):
        return jsonify({'status': 'success', 'message': f'Folder {folder_name} exists in {path}.'}), 200
    else:
        return jsonify({'status': 'error', 'message': f'Folder {folder_name} does not exist in {path}.'}), 400

@app.route('/file_exists', methods=['POST'])
def get_file_exists():
    data = request.json
    print(data)
    if "file_name" not in data:
        return jsonify({"error": "file_name required"}), 400
    if "path" not in data:
        return jsonify({"error": "path required"}), 400
    file_name: str = data["file_name"]
    path: str = data["path"]
    file_path = os.path.join(path, file_name)
    if os.path.exists(file_path) and os.path.isfile(file_path):
        return jsonify({'status': 'success', 'message': f'File {file_name} exists in {path}.'}), 200
    else:
        return jsonify({'status': 'error', 'message': f'File {file_name} does not exist in {path}.'}), 400

@app.route('/is_details_view', methods=['POST'])
def get_file_explorer_is_details_view():
    data = request.json
    if "path" not in data:
        return jsonify({"error": "path is required"}), 400
    
    path = data["path"]
    
    import uiautomation_utils
    is_details = uiautomation_utils.is_file_explorer_details_view(path)
    
    if is_details:
        return jsonify({'status': 'success', 'message': 'File Explorer is in Details view', 'is_details_view': True}), 200
    else:
        return jsonify({'status': 'error', 'message': 'File Explorer is not in Details view', 'is_details_view': False}), 400

@app.route('/wallpaper', methods=['POST'])
def get_wallpaper():
    def get_wallpaper_windows():
        SPI_GETDESKWALLPAPER = 0x73
        MAX_PATH = 260
        buffer = ctypes.create_unicode_buffer(MAX_PATH)
        ctypes.windll.user32.SystemParametersInfoW(SPI_GETDESKWALLPAPER, MAX_PATH, buffer, 0)
        return buffer.value

    def get_wallpaper_macos():
        script = """
        tell application "System Events" to tell every desktop to get picture
        """
        process = subprocess.Popen(['osascript', '-e', script], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        output, error = process.communicate()
        if error:
            logger.error("Error: %s", error.decode('utf-8'))
            return None
        return output.strip().decode('utf-8')

    def get_wallpaper_linux():
        try:
            output = subprocess.check_output(
                ["gsettings", "get", "org.gnome.desktop.background", "picture-uri"],
                stderr=subprocess.PIPE
            )
            return output.decode('utf-8').strip().replace('file://', '').replace("'", "")
        except subprocess.CalledProcessError as e:
            logger.error("Error: %s", e)
            return None

    os_name = platform.system()
    wallpaper_path = None
    if os_name == 'Windows':
        wallpaper_path = get_wallpaper_windows()
    elif os_name == 'Darwin':
        wallpaper_path = get_wallpaper_macos()
    elif os_name == 'Linux':
        wallpaper_path = get_wallpaper_linux()
    else:
        logger.error(f"Unsupported OS: {os_name}")
        abort(400, description="Unsupported OS")

    if wallpaper_path:
        try:
            # Ensure the filename is secure
            return send_file(wallpaper_path, mimetype='image/png')
        except Exception as e:
            logger.error(f"An error occurred while serving the wallpaper file: {e}")
            logger.error("\n" + traceback.format_exc() + "\n")
            
            abort(500, description="Unable to serve the wallpaper file")
    else:
        abort(404, description="Wallpaper file not found")


@app.route('/list_directory', methods=['POST'])
def get_directory_tree():
    def _list_dir_contents(directory):
        """
        List the contents of a directory recursively, building a tree structure.

        :param directory: The path of the directory to inspect.
        :return: A nested dictionary with the contents of the directory.
        """
        tree = {'type': 'directory', 'name': os.path.basename(directory), 'children': []}
        try:
            # List all files and directories in the current directory
            for entry in os.listdir(directory):
                full_path = os.path.join(directory, entry)
                # If entry is a directory, recurse into it
                if os.path.isdir(full_path):
                    tree['children'].append(_list_dir_contents(full_path))
                else:
                    tree['children'].append({'type': 'file', 'name': entry})
        except OSError as e:
            # If the directory cannot be accessed, return the exception message
            tree = {'error': str(e)}
        return tree

    # Extract the 'path' parameter from the JSON request
    data = request.get_json()
    if 'path' not in data:
        return jsonify(error="Missing 'path' parameter"), 400

    start_path = data['path']
    # Ensure the provided path is a directory
    if not os.path.isdir(start_path):
        return jsonify(error="The provided path is not a directory"), 400

    # Generate the directory tree starting from the provided path
    directory_tree = _list_dir_contents(start_path)
    return jsonify(directory_tree=directory_tree)


@app.route('/file', methods=['POST'])
def get_file():
    # Retrieve filename from the POST request
    if 'file_path' in request.form:
        file_path = os.path.expandvars(os.path.expanduser(request.form['file_path']))
    else:
        return jsonify({"error": "file_path is required"}), 400

    try:
        # Check if the file exists and send it to the user
        return send_file(file_path, as_attachment=True)
    except FileNotFoundError:
        # If the file is not found, return a 404 error
        return jsonify({"error": "File not found"}), 404


@app.route("/setup/upload", methods=["POST"])
def upload_file():
    # Retrieve filename from the POST request
    if 'file_path' in request.form and 'file_data' in request.files:
        file_path = os.path.expandvars(os.path.expanduser(request.form['file_path']))
        file = request.files["file_data"]
        file.save(file_path)
        return "File Uploaded"
    else:
        return jsonify({"error": "file_path and file_data are required"}), 400


@app.route('/platform', methods=['GET'])
def get_platform():
    return platform.system()


@app.route('/cursor_position', methods=['GET'])
def get_cursor_position():
    return pyautogui.position().x, pyautogui.position().y


@app.route("/setup/change_wallpaper", methods=['POST'])
def change_wallpaper():
    data = request.json
    path = data.get('path', None)

    if not path:
        return "Path not supplied!", 400

    path = Path(os.path.expandvars(os.path.expanduser(path)))

    if not path.exists():
        return f"File not found: {path}", 404

    try:
        user_platform = platform.system()
        if user_platform == "Windows":
            import ctypes
            ctypes.windll.user32.SystemParametersInfoW(20, 0, str(path), 3)
        elif user_platform == "Linux":
            import subprocess
            subprocess.run(["gsettings", "set", "org.gnome.desktop.background", "picture-uri", f"file://{path}"])
        elif user_platform == "Darwin":  # (Mac OS)
            import subprocess
            subprocess.run(
                ["osascript", "-e", f'tell application "Finder" to set desktop picture to POSIX file "{path}"'])
        return "Wallpaper changed successfully"
    except Exception as e:
        logger.error(f"Failed to change wallpaper. Error: {e}")
        logger.error("\n" + traceback.format_exc() + "\n")
        return f"Failed to change wallpaper. Error: {e}", 500


@app.route("/setup/download_file", methods=['POST'])
def download_file():
    data = request.json
    url = data.get('url', None)
    path = data.get('path', None)
    print(url, path)
    print("*" * 100)

    if not url or not path:
        return "Path or URL not supplied!", 400

    path = Path(os.path.expandvars(os.path.expanduser(path)))
    path.parent.mkdir(parents=True, exist_ok=True)

    max_retries = 3
    error: Optional[Exception] = None
    for i in range(max_retries):
        try:
            response = requests.get(url, stream=True)
            response.raise_for_status()

            with open(path, 'wb') as f:
                for chunk in response.iter_content(chunk_size=8192):
                    if chunk:
                        f.write(chunk)
            time.sleep(2)
            return "File downloaded successfully"

        except requests.RequestException as e:
            error = e
            logger.error(f"Failed to download {url}. Retrying... ({max_retries - i - 1} attempts left)")

    return f"Failed to download {url}. No retries left. Error: {error}", 500


@app.route("/setup/open_file", methods=['POST'])
def open_file():
    data = request.json
    path = data.get('path', None)

    if not path:
        return "Path not supplied!", 400

    path = Path(os.path.expandvars(os.path.expanduser(path)))

    if not path.exists():
        return f"File not found: {path}", 404

    try:
        if platform.system() == "Windows":
            os.startfile(path)
            time.sleep(5)
        else:
            open_cmd: str = "open" if platform.system() == "Darwin" else "xdg-open"
            subprocess.Popen([open_cmd, str(path)])
            time.sleep(5)
        return "File opened successfully"
    except Exception as e:
        logger.error(f"Failed to open {path}. Error: {e}")
        logger.error("\n" + traceback.format_exc() + "\n")
        return f"Failed to open {path}. Error: {e}", 500


@app.route("/setup/activate_window", methods=['POST'])
def activate_window():
    data = request.json
    window_name = data.get('window_name', None)
    if not window_name:
        return "window_name required", 400
    strict: bool = data.get("strict", False)  # compare case-sensitively and match the whole string
    by_class_name: bool = data.get("by_class", False)

    os_name = platform.system()

    if os_name == 'Windows':
        import pygetwindow as gw

        if by_class_name:
            return "Get window by class name is currently not supported on Windows.", 500

        windows: List[gw.Window] = gw.getWindowsWithTitle(window_name)
        
        # window: Optional[gw.Window] = None       
        if len(windows) == 0:
            return "Window {:} not found (empty results)".format(window_name), 404

        if windows and strict:
            window = windows[0]
            window.minimize()
            time.sleep(1)
            window.restore()
            time.sleep(1)
            
            assert window.isActive
            # return f"ACTIVE? {window.title} is {window.isActive} active"
    
        # windows = []
        # for w in gw.getAllWindows():
        #     windows.append(w.title)
                
        # window: Optional[gw.Window] = None
        # if len(windows) == 0:
        #     return "Window {:} not found (empty results)".format(window_name), 404
        # elif strict:
        #     for wnd in windows:
        #         if wnd.title == window_name:
        #             window = wnd
        #         #     return f"WINDOW ACT FOUND!! {window.title}", 999
        #         # if wnd.title == wnd:
        #         #     window = wnd
        #     if window is None:
        #         return "Window {:} not found (strict mode).".format(window_name), 404

        # else:
        #     window = windows[0]
        #     # window.activate()
        #     # return "Unstrict and windows exist, defaulting to first window {:}.".format(window_name), 200

        # window.activate()

    elif os_name == 'Darwin':
        import pygetwindow as gw
        if by_class_name:
            return "Get window by class name is not supported on macOS currently.", 500
        # Find the VS Code window
        windows = gw.getWindowsWithTitle(window_name)

        window: Optional[gw.Window] = None
        if len(windows) == 0:
            return "Window {:} not found (empty results)".format(window_name), 404
        elif strict:
            for wnd in windows:
                if wnd.title == wnd:
                    window = wnd
            if window is None:
                return "Window {:} not found (strict mode).".format(window_name), 404
        else:
            window = windows[0]

        # Un-minimize the window and then bring it to the front
        window.unminimize()
        window.activate()

    elif os_name == 'Linux':
        # Attempt to activate VS Code window using wmctrl
        subprocess.run(["wmctrl"
                           , "-{:}{:}a".format("x" if by_class_name else ""
                                               , "F" if strict else ""
                                               )
                           , window_name
                        ]
                       )

    else:
        return f"Operating system {os_name} not supported.", 400

    return "Window activated successfully", 200


@app.route("/setup/clear_task_files", methods=["POST"])
def clear_task_files():
    os_name = platform.system()
    if os_name == "Windows":

        # path_to_rm = os.path.join("C:", "Users", "Docker", "Downloads")
        path_to_rm = "C:\\Users\\Docker\\Downloads"
        def remove_files_in_path(path):
            """
            Remove all files in the specified path to avoid clutter build-up or interference with later tasks
            """
            files = glob.glob(path + "\\*")    
            files_present = os.listdir(path)

            rm_files = []
            rm_fail = []
            for file in files:
                if file != "C:\\Users\\Docker\\Downloads\\desktop.ini":
                    try:  
                        os.remove(file)
                        rm_files.append(file)  
                    except OSError as e:  
                        # print("Error: %s : %s" % (file, e.strerror)) 
                        rm_fail.append(file)

            return rm_files, rm_fail

        rmed_files, failed_files = remove_files_in_path(path_to_rm)
           
    elif os_name == "Linux":
        return "Currently not supported on Linux.", 500
    elif os_name == "Darwin":
        return "Currently not supported on Darwin.", 500
    else:
        return "Not supported platform {:}".format(os_name), 500

    if failed_files and rmed_files:
        return f"Failed to remove ({failed_files}) but sucessfully cleared ({rmed_files}) from {repr(path_to_rm)}.", 200
    if failed_files and not rmed_files:
        return f"Failed to remove ({failed_files}) from {repr(path_to_rm)}.", 200
    if not failed_files and rmed_files:
        return f"All task files ({rmed_files}) cleared successfully from {repr(path_to_rm)}.", 200
    if not failed_files and not rmed_files:
        return f"No task files to clear from {repr(path_to_rm)}.", 200

@app.route("/setup/close_all", methods=["POST"])
def close_all_windows():
    os_name = platform.system()
    if os_name == "Windows":
        import pygetwindow as gw

        # w = []
        # for window in gw.getAllWindows():
        #     w.append(window.title)
        # return w, 999

        for window in gw.getAllWindows():
            if window.title != "Program Manager" and window.title != "" and \
                window.title != "Administrator: C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe":
                window.close()
            
        # for window in gw.getAllWindows():
        #     if window.title != "Program Manager" and window.title != "":
        #         window.close()
                
    elif os_name == "Linux":
        subprocess.run(["wmctrl", "-c", ":ALL:"], stdout=subprocess.DEVNULL)
    elif os_name == "Darwin":
        import pygetwindow as gw
        return "Currently not supported on macOS.", 500
    else:
        return "Not supported platform {:}".format(os_name), 500

    return "All windows closed successfully.", 200

@app.route("/setup/close_window", methods=["POST"])
def close_window():
    data = request.json
    if "window_name" not in data:
        return "window_name required", 400
    window_name: str = data["window_name"]
    strict: bool = data.get("strict", False)  # compare case-sensitively and match the whole string
    by_class_name: bool = data.get("by_class", False)

    os_name: str = platform.system()
    if os_name == "Windows":
        import pygetwindow as gw

        if by_class_name:
            return "Get window by class name is not supported on Windows currently.", 500
        windows: List[gw.Window] = gw.getWindowsWithTitle(window_name)

        window: Optional[gw.Window] = None
        if len(windows) == 0:
            return "Window {:} not found (empty results)".format(window_name), 404
        elif strict:
            for wnd in windows:
                if wnd.title == wnd:
                    window = wnd
            if window is None:
                return "Window {:} not found (strict mode).".format(window_name), 404
        else:
            window = windows[0]
        window.close()
    elif os_name == "Linux":
        subprocess.run(["wmctrl"
                           , "-{:}{:}c".format("x" if by_class_name else ""
                                               , "F" if strict else ""
                                               )
                           , window_name
                        ]
                       )
    elif os_name == "Darwin":
        import pygetwindow as gw
        return "Currently not supported on macOS.", 500
    else:
        return "Not supported platform {:}".format(os_name), 500

    return "Window closed successfully.", 200


@app.route('/start_recording', methods=['POST'])
def start_recording():
    global recording_process
    if recording_process:
        return jsonify({'status': 'error', 'message': 'Recording is already in progress.'}), 400

    if platform_name == 'Linux':
        d = display.Display()
        screen_width = d.screen().width_in_pixels
        screen_height = d.screen().height_in_pixels

        start_command = f"ffmpeg -y -f x11grab -draw_mouse 1 -s {screen_width}x{screen_height} -i :0.0 -pix_fmt yuv420p -c:v libx264 -r 30 '{recording_path}'"

        recording_process = subprocess.Popen(shlex.split(start_command), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    elif platform_name == 'Windows':
        screen_width, screen_height = pyautogui.size()
        start_command = f"ffmpeg -y -f gdigrab -draw_mouse 1 -video_size {screen_width}x{screen_height} -i desktop -pix_fmt yuv420p -c:v libx264 -r 30 '{recording_path}'"

        recording_process = subprocess.Popen(shlex.split(start_command), stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    else:
        return jsonify({'status': 'error', 'message': 'Recording is not supported on this platform.'}), 400
    
    return jsonify({'status': 'success', 'message': f'Started recording to: {recording_path}.\nCOMMAND: {start_command}'})

@app.route('/end_recording', methods=['POST'])
def end_recording():
    global recording_process

    if not recording_process:
        return jsonify({'status': 'error', 'message': 'No recording in progress to stop.'}), 400

    # recording_process.send_signal(signal.SIGINT)
    # ps_childrend = recording_process.children()
    # logger.info(f"Children: {ps_childrend}")

    # for c in ps_childrend:
    #     c.send_signal(signal.CTRL_C_EVENT)
    # os.killpg(os.getpgid(recording_process.pid), signal.SIGTERM)
    # os.kill(recording_process.pid, signal.CTRL_C_EVENT)
    recording_process.communicate(b'q') 
    # os.kill(recording_process.pid, signal.SIGINT)
    # os.kill(recording_process.pid, signal.SIGINT)
    recording_process.wait()
    recording_process.terminate()
    # if recording_process.poll() is None:  
    #     # Forcefully kill the process if it did not terminate  
    #     recording_process.kill()
    recording_process = None

    # return recording video file
    if os.path.exists(recording_path):
        return  jsonify({'status': 'success', 'message': f'record saved to: {recording_path}'}), 200
    else:
        return abort(404, description="Recording failed")


@app.route('/save_state', methods=['POST'])
def save_state():
    # creates a system snapshot with the name provided
    if 'snapshot_name' in request.form:
        snapshot_name = os.path.expandvars(os.path.expanduser(request.form['file_path']))
    else:
        return jsonify({"error": "snapshot_name is required"}), 400

    try:
        # Create a snapshot of the system

        command = f'Checkpoint-Computer -Description "{snapshot_name}" -RestorePointType "MODIFY_SETTINGS"'
        command = shlex.split(command)

        result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, shell=True, text=True, timeout=120)
        return jsonify({
            'status': 'success',
            'message': f'Created a snapshot named {snapshot_name}',
            'output': result.stdout,
            'error': result.stderr,
            'returncode': result.returncode
        })
    except FileNotFoundError:
        return jsonify({"error": "Could not create snapshot"}), 404
    
@app.route('/revert_to_snapshot', methods=['POST'])
def revert_to_snapshot():
    
    # restores a system snapshot with the name provided
    # if 'snapshot_name' in request.form:
    #     snapshot_name = os.path.expandvars(os.path.expanduser(request.form['file_path']))
    # else:
    #     return jsonify({"error": "snapshot_name is required"}), 400

    # try:
    #     # Restore a snapshot of the system

    #     command = f'powershell.exe -Command "& {{ $rp = Get-ComputerRestorePoint | Where-Object {{ $_.Description -eq \'{snapshot_name}\' }} | Sort-Object -Property SequenceNumber -Descending | Select-Object -First 1; Restore-Computer -RestorePoint $rp.SequenceNumber }}"'
    #     command = shlex.split(command)

    #     result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, shell=True, text=True, timeout=120)
    #     return jsonify({
    #         'status': 'success',
    #         'message': f'Restored a snapshot named {snapshot_name}',
    #         'output': result.stdout,
    #         'error': result.stderr,
    #         'returncode': result.returncode
    #     })
    # except FileNotFoundError:
    #     return jsonify({"error": "Could not restore snapshot"}), 404
    return jsonify({"error": "Restore snapshot no implemented"}), 501

@app.route('/are_files_sorted_by_modified_time', methods=['POST'])
def get_are_files_sorted_by_modified_time():
    data = request.json
    if "directory" not in data:
        return jsonify({"error": "directory is required"}), 400

    directory = data["directory"]
    print(directory)

    # Get the list of files in the order displayed in Windows Explorer.
    import uiautomation_utils
    explorer_sorted_files = uiautomation_utils.get_file_list_from_explorer(directory)
    print(f"explorer_sorted_files: {explorer_sorted_files}")

    # Retrieve the full path and modification time (rounded to the nearest second) for each item
    items_with_mtime = []
    for item in explorer_sorted_files:
        item_path = os.path.join(directory, item)
        # Get the modification time of the item (file or folder)
        mtime = int(os.path.getmtime(item_path))
        is_directory = os.path.isdir(item_path)  # Check if the item is a directory
        items_with_mtime.append((item, mtime, is_directory))

    # Sort by:
    # 1. Whether the item is a directory
    # 2. Modification time (descending)
    # 3. File/folder name (ascending) if times are identical
    sorted_items_desc = sorted(items_with_mtime, key=lambda x: (x[2], -x[1], x[0]))
    sorted_items_asc = sorted(items_with_mtime, key=lambda x: (-x[2], x[1], x[0]))

    sorted_items_names_desc = [item[0] for item in sorted_items_desc]
    sorted_items_names_asc = [item[0] for item in sorted_items_asc]

    print(f"sorted_items_names_desc: {sorted_items_names_desc}")
    print(f"sorted_items_names_asc: {sorted_items_names_asc}")

    # Check if explorer_sorted_files matches either ascending or descending order
    if explorer_sorted_files == sorted_items_names_desc or explorer_sorted_files == sorted_items_names_asc:
        return jsonify({'status': 'success', 'message': f'{directory}: files are sorted by modified time'}), 200
    else:
        return jsonify({'status': 'error', 'message': f'{directory}: files are not sorted by modified time'}), 400

@app.route('/is_directory_read_only_for_user', methods=['POST'])
def is_directory_read_only_for_user():
    data = request.json
    if "directory" not in data:
        return jsonify({"error": "directory is required"}), 400
    if "user" not in data:
        return jsonify({"error": "user is required"}), 400

    directory = data["directory"]
    user = data["user"]
    import fileexplorer_utils
    if fileexplorer_utils.is_directory_read_only_for_user(directory, user):
        return jsonify({'status': 'success', 'message': f'{directory} is read-only for {user}'}), 200
    else:
        return jsonify({'status': 'error', 'message': f'{directory} is not read-only for {user}'}), 400
    
@app.route('/are_all_images_tagged', methods=['POST'])
def are_all_images_tagged():
    data = request.json
    if "directory" not in data:
        return jsonify({"error": f"directory is required"}), 400
    if "tag" not in data:
        return jsonify({"error": "tag is required"}), 400

    directory = data["directory"]
    tag = data["tag"]
    files = os.listdir(directory)
    image_files = [file for file in files if file.lower().endswith(('jpg', 'jpeg', 'tiff'))]
    import fileexplorer_utils
    for image_file in image_files:
        image_path = os.path.join(directory, image_file)
        if not fileexplorer_utils.check_tag_in_image(image_path, tag):
            return jsonify({'status': 'error', 'message': f'{image_file} in {directory} is not tagged with {tag}'}), 400
    return jsonify({'status': 'success', 'message': f'All images in {directory} are tagged with {tag}'}), 200

@app.route('/library_folders', methods=['POST'])
def get_library_folders():
    data = request.json
    if "library_name" not in data:
        return jsonify({"error": "library_name is required"}), 400

    library_name = data["library_name"]
    import fileexplorer_utils;
    if not fileexplorer_utils.check_library_exists(library_name):
        return jsonify({"error": f"{library_name} does not exist"}), 400

    library_folders = fileexplorer_utils.get_library_folders(library_name)
    print(f"Library Folders: {library_folders}")
    return jsonify({'status': 'success', 'output': library_folders}), 200
@app.route('/check_if_timer_started', methods=['POST'])
def get_check_if_timer_started():
    data = request.json
    if 'hours' not in data:
        return jsonify({'error': 'hours is required'}), 400
    if 'minutes' not in data:
        return jsonify({'error': 'minutes is required'}), 400
    if 'seconds' not in data:
        return jsonify({'error': 'seconds is required'}), 400
    
    hours = data['hours']
    minutes = data['minutes']
    seconds = data['seconds']

    print("Checking if timer exists for {} hours, {} minutes, and {} seconds".format(hours, minutes, seconds))

    # Check if the timer exists
    import uiautomation_utils
    timer_exists = uiautomation_utils.clock_check_if_timer_started(hours, minutes, seconds)

    if timer_exists:
        return jsonify({'status': 'success', 'message': 'Timer exists'}), 200
    else:
        return jsonify({'status': 'error', 'message': 'Timer does not exist'}), 400
    
@app.route('/check_if_world_clock_exists', methods=['POST'])
def get_check_if_world_clock_exists():
    data = request.json
    if 'city' not in data or 'country' not in data:
        return jsonify({'error': 'city and country are required'}), 400
        
    city = data['city']
    country = data['country']

    print("Checking if world clock exists for {}, {}".format(city, country))
    
    # Check if the world clock exists
    import uiautomation_utils
    world_clock_exists = uiautomation_utils.clock_check_if_world_clock_exists(city, country)

    if world_clock_exists:
        return jsonify({'status': 'success', 'message': 'World clock exists'}), 200
    else:
        return jsonify({'status': 'error', 'message': 'World clock does not exist'}), 400

if __name__ == '__main__':
    app.run(debug=True, host="0.0.0.0", port=args.port)


# example command to test server. get platform
# curl -X GET http://127.0.0.1:5000/platform
# on windows:
# Invoke-WebRequest -Uri http://127.0.0.1:5000/platform