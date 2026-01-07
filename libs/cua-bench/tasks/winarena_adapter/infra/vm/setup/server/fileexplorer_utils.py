import os
import ntsecuritycon
import win32security
from PIL import Image
import xml.etree.ElementTree as ET

def sid_equal(sid1, sid2):
    sid1_str = win32security.ConvertSidToStringSid(sid1)
    sid2_str = win32security.ConvertSidToStringSid(sid2)
    return sid1_str == sid2_str

def is_directory_read_only_for_user(directory, specific_user):
    if not os.path.exists(directory):
        raise FileNotFoundError(f"Directory '{directory}' does not exist.")
    if not specific_user:
        raise ValueError("Specific user must be provided.")

    try:
        # Get the security descriptor for the directory
        sd = win32security.GetFileSecurity(directory, win32security.DACL_SECURITY_INFORMATION)
        dacl = sd.GetSecurityDescriptorDacl()
        
        # Get the SID for the specific user
        user_sid, _, _ = win32security.LookupAccountName(None, specific_user)
    except Exception as e:
        raise ValueError(f"Could not find user '{specific_user}': {e}")

    has_read_only_permission = False
    for i in range(dacl.GetAceCount()):
        ace = dacl.GetAce(i)
        ace_sid = ace[2]

        # Check if the ACE is for the specific user
        if sid_equal(ace_sid, user_sid):
            mask = ace[1]
            print(f"Mask value: {mask:#010x}")

            # Checking for read-related permissions and ensuring no write-related permissions are present
            has_read_only_permission = (
                (mask & (ntsecuritycon.FILE_GENERIC_READ | ntsecuritycon.FILE_GENERIC_EXECUTE)) != 0 and
                (mask & (ntsecuritycon.FILE_APPEND_DATA | ntsecuritycon.FILE_WRITE_ATTRIBUTES | ntsecuritycon.FILE_WRITE_DATA | ntsecuritycon.FILE_WRITE_EA)) == 0
            )
            break

    return has_read_only_permission


def convert_tag_to_string(value):
    if isinstance(value, bytes):
        try:
            return value.decode('utf-16', errors='ignore')
        except UnicodeDecodeError:
            return str(value)
    elif isinstance(value, tuple):
        return ' '.join([convert_tag_to_string(v) for v in value])
    else:
        return str(value)

def check_tag_in_image(image_path, tag_to_search) -> bool:
    try:
        image = Image.open(image_path)
        exif_data = image._getexif()
        if exif_data is not None:
            for tag, value in exif_data.items():
                value_str = convert_tag_to_string(value)
                print(f"Tag: {tag}, Value: {value_str}")
                if tag_to_search in value_str:
                    print(f"Found '{tag_to_search}' in {image_path}")
                    return True
        else:
            print(f"No EXIF data found in {image_path}")
    except Exception as e:
        print(f"Could not open image {image_path}: {e}")
    return False


def check_library_exists(library_name):
    library_file = os.path.join(
        os.getenv('APPDATA'),
        'Microsoft\\Windows\\Libraries',
        f'{library_name}.library-ms'
    )
    return os.path.exists(library_file)

def get_library_folders(library_name):
    library_file = os.path.join(
        os.getenv('APPDATA'),
        'Microsoft\\Windows\\Libraries',
        f'{library_name}.library-ms'
    )

    if not os.path.exists(library_file):
        raise FileNotFoundError(f"Library '{library_name}' does not exist.")
    
    tree = ET.parse(library_file)
    root = tree.getroot()
    namespace = {'default': 'http://schemas.microsoft.com/windows/2009/library'}
    folder_paths = []

    for search in root.findall('default:searchConnectorDescriptionList/default:searchConnectorDescription/default:simpleLocation/default:url', namespace):
        folder_paths.append(search.text)

    return sorted(folder_paths)
