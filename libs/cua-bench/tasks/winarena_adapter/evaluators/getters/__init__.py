from .calc import get_conference_city_in_order as get_conference_city_in_order
from .chrome import get_active_tab_html_parse as get_active_tab_html_parse
from .chrome import get_active_tab_info as get_active_tab_info
from .chrome import get_active_tab_url_parse as get_active_tab_url_parse
from .chrome import get_active_url_from_accessTree as get_active_url_from_accessTree
from .chrome import get_bookmarks as get_bookmarks
from .chrome import get_chrome_font_size as get_chrome_font_size
from .chrome import get_chrome_language as get_chrome_language
from .chrome import get_cookie_data as get_cookie_data
from .chrome import get_data_delete_automacally as get_data_delete_automacally
from .chrome import get_default_search_engine as get_default_search_engine
from .chrome import get_enable_do_not_track as get_enable_do_not_track
from .chrome import (
    get_enable_enhanced_safety_browsing as get_enable_enhanced_safety_browsing,
)
from .chrome import get_enabled_experiments as get_enabled_experiments
from .chrome import (
    get_find_installed_extension_name as get_find_installed_extension_name,
)
from .chrome import get_find_unpacked_extension_path as get_find_unpacked_extension_path
from .chrome import get_googledrive_file as get_googledrive_file
from .chrome import (
    get_gotoRecreationPage_and_get_html_content as get_gotoRecreationPage_and_get_html_content,
)
from .chrome import get_history as get_history
from .chrome import get_info_from_website as get_info_from_website
from .chrome import get_new_startup_page as get_new_startup_page
from .chrome import get_number_of_search_results as get_number_of_search_results
from .chrome import get_open_tabs_info as get_open_tabs_info
from .chrome import get_page_info as get_page_info
from .chrome import get_pdf_from_url as get_pdf_from_url
from .chrome import get_profile_name as get_profile_name
from .chrome import get_shortcuts_on_desktop as get_shortcuts_on_desktop
from .chrome import get_url_dashPart as get_url_dashPart
from .edge import get_cookie_data_for_edge as get_cookie_data_for_edge
from .edge import (
    get_data_delete_automacally_from_edge as get_data_delete_automacally_from_edge,
)
from .edge import (
    get_default_search_engine_from_edge as get_default_search_engine_from_edge,
)
from .edge import get_edge_font_size as get_edge_font_size
from .edge import get_enable_do_not_track_from_edge as get_enable_do_not_track_from_edge
from .edge import (
    get_enable_enhanced_safety_browsing_from_edge as get_enable_enhanced_safety_browsing_from_edge,
)
from .edge import get_favorites as get_favorites
from .edge import get_history_for_edge as get_history_for_edge
from .edge import get_profile_name_from_edge as get_profile_name_from_edge
from .edge import get_url_shortcuts_on_desktop as get_url_shortcuts_on_desktop
from .file import get_cache_file as get_cache_file
from .file import get_cloud_file as get_cloud_file
from .file import get_content_from_vm_file as get_content_from_vm_file
from .file import get_vm_file as get_vm_file
from .file import get_vm_file_exists_in_vm_folder as get_vm_file_exists_in_vm_folder
from .fileexplorer import get_all_png_file_names as get_all_png_file_names
from .fileexplorer import get_are_all_images_tagged as get_are_all_images_tagged
from .fileexplorer import (
    get_are_files_sorted_by_modified_time as get_are_files_sorted_by_modified_time,
)
from .fileexplorer import get_is_all_docx_in_archive as get_is_all_docx_in_archive
from .fileexplorer import get_is_details_view as get_is_details_view
from .fileexplorer import (
    get_is_directory_read_only_for_user as get_is_directory_read_only_for_user,
)
from .fileexplorer import get_is_file_desktop as get_is_file_desktop
from .fileexplorer import get_is_file_hidden as get_is_file_hidden
from .fileexplorer import get_is_file_saved_desktop as get_is_file_saved_desktop
from .fileexplorer import get_is_files_moved_downloads as get_is_files_moved_downloads
from .fileexplorer import get_vm_active_window_title as get_vm_active_window_title
from .fileexplorer import get_vm_file_exists_in_desktop as get_vm_file_exists_in_desktop
from .fileexplorer import (
    get_vm_folder_exists_in_documents as get_vm_folder_exists_in_documents,
)
from .fileexplorer import get_vm_library_folders as get_vm_library_folders
from .fileexplorer import get_zipped_folder_in_desktop as get_zipped_folder_in_desktop
from .general import get_vm_command_error as get_vm_command_error
from .general import get_vm_command_line as get_vm_command_line
from .general import get_vm_terminal_output as get_vm_terminal_output
from .gimp import get_gimp_config_file as get_gimp_config_file
from .impress import get_audio_in_slide as get_audio_in_slide
from .impress import get_background_image_in_slide as get_background_image_in_slide
from .info import get_list_directory as get_list_directory
from .info import get_vm_screen_size as get_vm_screen_size
from .info import get_vm_wallpaper as get_vm_wallpaper
from .info import get_vm_window_size as get_vm_window_size
from .microsoftpaint import (
    get_image_dimension_matches_input as get_image_dimension_matches_input,
)
from .microsoftpaint import (
    get_is_red_circle_present_on_canvas as get_is_red_circle_present_on_canvas,
)
from .misc import get_accessibility_tree as get_accessibility_tree
from .misc import get_rule as get_rule
from .misc import get_rule_relativeTime as get_rule_relativeTime
from .misc import get_time_diff_range as get_time_diff_range
from .msedge import get_edge_default_download_folder as get_edge_default_download_folder
from .msedge import get_edge_home_page as get_edge_home_page
from .msedge import get_validate_pwa_installed as get_validate_pwa_installed
from .replay import get_replay as get_replay
from .settings import (
    get_active_hours_of_user_to_not_interrupt_for_windows_updates as get_active_hours_of_user_to_not_interrupt_for_windows_updates,
)
from .settings import get_default_browser as get_default_browser
from .settings import get_desktop_background as get_desktop_background
from .settings import get_night_light_state as get_night_light_state
from .settings import get_storage_sense_run_frequency as get_storage_sense_run_frequency
from .settings import get_system_notifications as get_system_notifications
from .settings import get_system_timezone as get_system_timezone
from .vlc import get_default_video_player as get_default_video_player
from .vlc import get_vlc_config as get_vlc_config
from .vlc import get_vlc_playing_info as get_vlc_playing_info
from .vscode import get_vscode_config as get_vscode_config
from .windows_clock import get_check_if_timer_started as get_check_if_timer_started
from .windows_clock import (
    get_check_if_world_clock_exists as get_check_if_world_clock_exists,
)
