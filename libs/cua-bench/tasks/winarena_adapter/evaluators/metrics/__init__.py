from .basic_os import check_gnome_favorite_apps as check_gnome_favorite_apps
from .basic_os import check_moved_jpgs as check_moved_jpgs
from .basic_os import check_text_enlarged as check_text_enlarged
from .basic_os import is_in_vm_clickboard as is_in_vm_clickboard
from .basic_os import is_utc_0 as is_utc_0
from .chrome import (
    check_enabled_experiments as check_enabled_experiments,  # NOTE: compare_htmls, compare_archive, compare_pdf_images removed - dead code; NOTE: is_expected_search_query, is_expected_url_pattern_match, is_expected_installed_extensions removed - dead code
)
from .chrome import check_font_size as check_font_size
from .chrome import check_history_deleted as check_history_deleted
from .chrome import compare_pdfs as compare_pdfs
from .chrome import is_added_to_steam_cart as is_added_to_steam_cart
from .chrome import is_cookie_deleted as is_cookie_deleted
from .chrome import is_expected_active_tab as is_expected_active_tab
from .chrome import is_expected_bookmarks as is_expected_bookmarks
from .chrome import is_expected_tabs as is_expected_tabs
from .chrome import is_shortcut_on_desktop as is_shortcut_on_desktop
from .docs import check_file_exists as check_file_exists
from .docs import check_highlighted_words as check_highlighted_words
from .docs import check_italic_font_size_14 as check_italic_font_size_14
from .docs import check_no_duplicates as check_no_duplicates
from .docs import check_tabstops as check_tabstops
from .docs import compare_contains_image as compare_contains_image
from .docs import compare_docx_files as compare_docx_files
from .docs import (
    compare_docx_files_and_ignore_new_lines as compare_docx_files_and_ignore_new_lines,
)
from .docs import compare_docx_images as compare_docx_images
from .docs import compare_docx_lines as compare_docx_lines
from .docs import compare_docx_tables as compare_docx_tables
from .docs import compare_font_names as compare_font_names
from .docs import compare_highlighted_text as compare_highlighted_text
from .docs import compare_init_lines as compare_init_lines
from .docs import compare_insert_equation as compare_insert_equation
from .docs import compare_line_spacing as compare_line_spacing
from .docs import compare_references as compare_references
from .docs import compare_subscript_contains as compare_subscript_contains
from .docs import contains_page_break as contains_page_break
from .docs import evaluate_alignment as evaluate_alignment
from .docs import evaluate_conversion as evaluate_conversion
from .docs import evaluate_spacing as evaluate_spacing
from .docs import (
    evaluate_strike_through_last_paragraph as evaluate_strike_through_last_paragraph,
)
from .docs import find_default_font as find_default_font
from .docs import get_unique_train_ids as get_unique_train_ids
from .docs import has_page_numbers_in_footers as has_page_numbers_in_footers
from .docs import is_first_line_centered as is_first_line_centered
from .edge import check_edge_font_size as check_edge_font_size
from .edge import is_url_shortcut_on_desktop as is_url_shortcut_on_desktop
from .general import check_accessibility_tree as check_accessibility_tree
from .general import check_csv as check_csv
from .general import check_direct_json_object as check_direct_json_object
from .general import check_include_exclude as check_include_exclude
from .general import check_json as check_json
from .general import check_line_number as check_line_number
from .general import check_list as check_list
from .general import compare_python_pure_text as compare_python_pure_text
from .general import compare_terminal_and_txt as compare_terminal_and_txt
from .general import (
    compare_time_in_speedtest_results as compare_time_in_speedtest_results,
)
from .general import diff_text_file as diff_text_file
from .general import exact_match as exact_match
from .general import file_contains as file_contains
from .general import fuzzy_match as fuzzy_match
from .general import fuzzy_place_math as fuzzy_place_math
from .general import is_gold_text_included_in_pdf as is_gold_text_included_in_pdf
from .general import is_in_list as is_in_list
from .general import is_included_all_json_objects as is_included_all_json_objects
from .general import literal_match as literal_match
from .general import run_sqlite3 as run_sqlite3

# NOTE: gimp.py imports removed - all 20 functions are dead code (no GIMP tasks exist)
# This avoids requiring cv2/skimage at import time
from .libreoffice import check_libre_locale as check_libre_locale
from .others import check_mp3_meta as check_mp3_meta
from .others import compare_epub as compare_epub
from .pdf import check_pdf_pages as check_pdf_pages
from .slides import check_auto_saving_time as check_auto_saving_time
from .slides import check_image_stretch_and_center as check_image_stretch_and_center
from .slides import check_left_panel as check_left_panel
from .slides import check_page_number_colors as check_page_number_colors
from .slides import check_presenter_console_disable as check_presenter_console_disable
from .slides import check_slide_numbers_color as check_slide_numbers_color
from .slides import check_slide_orientation_Portrait as check_slide_orientation_Portrait
from .slides import check_strikethrough as check_strikethrough
from .slides import check_transition as check_transition
from .slides import compare_pptx_files as compare_pptx_files
from .slides import (
    evaluate_presentation_fill_to_rgb_distance as evaluate_presentation_fill_to_rgb_distance,
)
from .table import compare_conference_city_in_order as compare_conference_city_in_order
from .table import compare_csv as compare_csv
from .table import compare_table as compare_table
from .thunderbird import check_thunderbird_filter as check_thunderbird_filter
from .thunderbird import check_thunderbird_folder as check_thunderbird_folder
from .thunderbird import check_thunderbird_prefs as check_thunderbird_prefs
from .vlc import (
    check_global_key_play_pause as check_global_key_play_pause,  # NOTE: is_vlc_playing and is_vlc_fullscreen removed - dead code
)
from .vlc import (
    check_one_instance_when_started_from_file as check_one_instance_when_started_from_file,
)
from .vlc import check_qt_bgcone as check_qt_bgcone
from .vlc import check_qt_max_volume as check_qt_max_volume
from .vlc import check_qt_minimal_view as check_qt_minimal_view
from .vlc import check_qt_slider_colours as check_qt_slider_colours
from .vlc import check_qt_video_resize as check_qt_video_resize
from .vlc import compare_audios as compare_audios
from .vlc import compare_images as compare_images
from .vlc import compare_videos as compare_videos
from .vlc import is_vlc_recordings_folder as is_vlc_recordings_folder
from .vscode import check_html_background_image as check_html_background_image
from .vscode import check_json_keybindings as check_json_keybindings
from .vscode import check_json_settings as check_json_settings
from .vscode import check_python_file_by_gold_file as check_python_file_by_gold_file
from .vscode import check_python_file_by_test_suite as check_python_file_by_test_suite
from .vscode import compare_answer as compare_answer
from .vscode import compare_config as compare_config
from .vscode import compare_result_files as compare_result_files
from .vscode import compare_text_file as compare_text_file
from .vscode import compare_zip_files as compare_zip_files
from .vscode import is_extension_installed as is_extension_installed


def infeasible():
    pass
