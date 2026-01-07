import json  
import os  
  
def get_default_search_provider():  
    # Path to the Preferences file  
    path_to_preferences = os.path.expanduser("~") + r'\AppData\Local\Google\Chrome\User Data\Default\Preferences'  
  
    # Load json from Preferences  
    with open(path_to_preferences, 'r', encoding='utf8') as f:  
        preferences_json = json.load(f)  
  
    # Get default search engine  
    default_search_engine = preferences_json.get('default_search_provider', {}).get('guid', 'Not found')  
      
    return default_search_engine  
  
print(get_default_search_provider())