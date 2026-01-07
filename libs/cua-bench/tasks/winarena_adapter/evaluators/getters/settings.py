from typing import Dict
import json

def generate_time(hour, minute):
    if hour < 12:
        am_pm = "AM"
    else:
        am_pm = "PM"
        hour -= 12
    return f"{hour:02d}:{minute:02d} {am_pm}"

def get_night_light_state(env, config: Dict[str, str]):
    key_path = r"HKCU:\Software\Microsoft\Windows\CurrentVersion\CloudStore\Store\DefaultAccount\Current\default$windows.data.bluelightreduction.settings\windows.data.bluelightreduction.settings"
    setting_path = r"Data"
    result = env.controller.get_registry_key(key_path, setting_path)
    # Decoder for the registry key
    # if Byte 24 is 1, then the night light is enabled
    # If Byte 28 is not 202, then the schedule is set
    # if minutes are set in starting or ending hour we need to use index 30 and 32 respectively
    # Otherwise, we use index 29 and 31 as they will have a value of 0. If minute is non-zero there is a marker value of 46
    decoded_result = result['output'].split('\n')
    if decoded_result[24] == "1":
        starting_hour = int(decoded_result[28])
        if starting_hour != 202:
            starting_minute = int(decoded_result[30] if decoded_result[29] != '0' else '0')
            if starting_minute == 0:
                ending_index = 33
            else:
                ending_index = 35
            ending_hour = int(decoded_result[ending_index])
            ending_minute = int(decoded_result[ending_index+2] if decoded_result[ending_index+1] != '0' else '0')
            start = generate_time(starting_hour, starting_minute)
            end = generate_time(ending_hour, ending_minute)
            print(f"Starting: {start}, End: {end}")
            return [True, start, end]
        else:
            return [True]
    return False

def get_default_browser(env, config: Dict[str,str]):
    key_path = r"HKCU:Software\Microsoft\Windows\Shell\Associations\UrlAssociations\http\UserChoice"
    setting_path = r"ProgId"
    result = env.controller.get_registry_key(key_path, setting_path)
    try:
        decoded_result = result['output'].split('\n')[0]
    except Exception as e:
        return "Error"
    return decoded_result

def get_storage_sense_run_frequency(env, config: Dict[str, str]):
    '''
    Decoder for the registry key
    If the value is 1 in setting path '01' then storage sense is enabled
    The frequency of running storage sense is stored in setting path '2048'
    '''
    KEY_PATH = r"HKCU:\Software\Microsoft\Windows\CurrentVersion\StorageSense\Parameters\StoragePolicy"
    STORAGE_SENSE_ON_OFF_SETTING_PATH = "01"
    STORAGE_SENSE_FREQUENCY_SETTING_PATH = "2048"

    is_storage_sense_enabled = env.controller.get_registry_key(KEY_PATH, STORAGE_SENSE_ON_OFF_SETTING_PATH)
    if is_storage_sense_enabled['status'] == 'success' and is_storage_sense_enabled['output'] == "1\n":
        storage_sense_run_frequency = env.controller.get_registry_key(KEY_PATH, STORAGE_SENSE_FREQUENCY_SETTING_PATH)
        if storage_sense_run_frequency['status'] == 'success':
            return storage_sense_run_frequency['output'].split('\n')[0]

    return None

def get_active_hours_of_user_to_not_interrupt_for_windows_updates(env, config: Dict[str, str]):
    '''
    Decoder for the registry key
    The value in setting path 'ActiveHoursStart' gives the start time of active hours
    The value in setting path 'ActiveHoursEnd' gives the end time of active hours
    '''
    KEY_PATH = r"HKLM:\SOFTWARE\Microsoft\WindowsUpdate\UX\Settings"
    ACTIVE_HOURS_START_SETTING_PATH = "ActiveHoursStart"
    ACTIVE_HOURS_END_SETTING_PATH = "ActiveHoursEnd"

    active_start_hours = env.controller.get_registry_key(KEY_PATH, ACTIVE_HOURS_START_SETTING_PATH)
    active_end_hours = env.controller.get_registry_key(KEY_PATH, ACTIVE_HOURS_END_SETTING_PATH)

    active_start_hours_formatted = generate_time(int(active_start_hours['output'].split('\n')[0]), 0)
    active_end_hours_formatted = generate_time(int(active_end_hours['output'].split('\n')[0]), 0)

    if active_start_hours['status'] == 'success' and active_end_hours['status'] == 'success':
        return [active_start_hours_formatted, active_end_hours_formatted]

    return None

def get_system_timezone(env, config: Dict[str,str]):
    """
    Fetches the system timezone from the VM.
    """
    command = "powershell -Command \"Get-TimeZone | Select-Object -Property DisplayName, StandardName | ConvertTo-Json\""
    try:
        result = env.controller.execute_shell_command(command)
        data = json.loads(result['output'])
        """Context on display name and Standard name - 
        DisplayName: (UTC-08:00) Pacific Time (US & Canada)
        StandardName: Pacific Standard Time
        """
        display_name = data.get('DisplayName', None)
        standard_name = data.get('StandardName', None)
        
        if display_name and standard_name:
            return display_name
    except Exception as e:
        return "Error getting the timezone"
    return "Unable to get time zone"

def get_desktop_background(env, config: Dict[str,str]):
    """
    Fetches the dekstop background of the VM.
    """
    key_path = r"HKCU:Control Panel\Desktop"
    setting_path = r"WallPaper"
    result = env.controller.get_registry_key(key_path, setting_path)
    wallpaper_path = result['output'].replace("\n","")
    try:
        if wallpaper_path == "" :
            return 'True'
    except Exception as e:
        return "error"
    return 'False'

def get_system_notifications(env, config: Dict[str,str]):
    """
    Checks if system notifications are enabled or disabled.
    If the `ToastEnabled` key exists and its value is 1, notifications are enabled.
    If the value is 0, notifications are disabled.
    If the key does not exist, assume notifications are enabled.
    """
    key_path = r"HKCU:Software\Microsoft\Windows\CurrentVersion\PushNotifications"
    setting_path = "ToastEnabled"
    is_notification_enabled  = env.controller.get_registry_key(key_path, setting_path)
    try:
        if is_notification_enabled is None or is_notification_enabled['output'] == "" :
            # Key doesn't exist, assume notifications are enabled
            return 'False'
        elif is_notification_enabled['output']:
            toast_enabled =  is_notification_enabled['output'].replace('\n','')
            if toast_enabled == '0':
                return 'True'
        else:
            return 'False'
    except Exception as e:
        print("Error retrieving notification setting")
        return "Uanble to get the notifications settings state"    