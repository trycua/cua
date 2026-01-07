from typing import Dict
import json



def get_edge_home_page(env, config: Dict[str, str]):

    preference_file_path = env.controller.execute_python_command(\
        "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Microsoft/Edge/User Data/Default/Secure Preferences'))"
    )['output'].strip()

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)

        home_page = data.get('homepage', {})
        if home_page:
            home_page = home_page.replace("https://", "").replace("http://", "").replace("/","")
            return home_page
    except Exception as e:
        return "error"
    return "False"


def get_validate_pwa_installed(env, config: Dict[str, str]):
    try:
        content = env.controller.get_all_installed_apps()
        if content['output']:
            data = json.loads(content['output'])
            pwa_packages = [pkg for pkg in data if 'pwabuilder' in pkg['Name'].lower()]
            if pwa_packages:
                return "app_installed"
    except Exception as e:
        return "error"
    return "app_not_installed"


def get_edge_default_download_folder(env, config: Dict[str, str]):

    preference_file_path = env.controller.execute_python_command(\
        "import os; print(os.path.join(os.getenv('LOCALAPPDATA'), 'Microsoft/Edge/User Data/Default/Preferences'))"
    )['output'].strip()

    try:
        content = env.controller.get_file(preference_file_path)
        data = json.loads(content)

        folder = data.get('download',{}).get('default_directory', {})
        if folder:
            return folder
    except Exception as e:
        return "error"
    return "False"

