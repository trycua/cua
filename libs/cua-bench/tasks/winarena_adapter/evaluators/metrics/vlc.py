import logging
import os
import subprocess
from typing import Dict
from xml.etree import ElementTree
# import requests

import acoustid
import cv2
import imagehash
import librosa
import numpy as np
from PIL import Image
from fastdtw import fastdtw
from scipy.spatial.distance import cosine
from skimage.metrics import structural_similarity as ssim

logger = logging.getLogger("desktopenv.metrics.vlc")


def is_vlc_playing(actual_status_path: str, rule: Dict[str, str]) -> float:
    """
    Checks if VLC is currently playing a file.
    """
    with open(actual_status_path, 'rb') as file:
        actual_status = file.read().decode('utf-8')

    tree = ElementTree.fromstring(actual_status)
    status = tree.find('state').text
    if status == 'playing':
        if rule['type'] == 'file_name':
            file_info = tree.find('information/category[@name="meta"]/info[@name="filename"]').text
            if file_info:
                return 1 if file_info.endswith(rule['file_name']) else 0
        elif rule['type'] == 'url':
            file_info = tree.find('information/category[@name="meta"]/info[@name="url"]').text
            if file_info:
                return 1 if file_info.endswith(rule['url']) else 0
        else:
            logger.error(f"Unknown type: {rule['type']}")
            return 0
    else:
        return 0


# fixme: part of this function can be moved to getters
def is_vlc_recordings_folder(actual_config_path: str, rule: Dict[str, str]) -> float:
    """
    Checks if VLC's recording folder is set to the expected value.
    """
    with open(actual_config_path, 'rb') as file:
        config_file = file.read().decode('utf-8')

    expected_recording_file_path = rule['recording_file_path']

    try:
        for line in config_file.split("\n"):
            # Skip comments and empty lines
            if line.startswith('#') or not line.strip():
                continue
            # Check if the line contains the recording path setting
            # if 'recorded_files_path' in line:
            if 'input-record-path' in line:
                # Extract the value of the recording path and remove surrounding whitespace
                current_path = line.split('=')[-1].strip()
                # Compare with the Desktop path
                if current_path == expected_recording_file_path:
                    return 1
                else:
                    return 0
            # The configuration key was not found in the file
            # return 0
        return 0
    except FileNotFoundError:
        logger.error("VLC configuration file not found.")
        return 0
    except Exception as e:
        logger.error(f"An error occurred: {e}")
        return 0


def is_vlc_fullscreen(actual_window_size, screen_size):
    if actual_window_size['width'] == screen_size['width'] and actual_window_size['height'] == screen_size['height']:
        return 1
    else:
        return 0


def compare_images(image1_path, image2_path):
    # You would call this function with the paths to the two images you want to compare:
    # score = compare_images('path_to_image1', 'path_to_image2')
    # print("Similarity score:", score)

    if not image1_path or not image2_path:
        return 0

    # Open the images and convert to grayscale
    image1 = Image.open(image1_path).convert('L')
    image2 = Image.open(image2_path).convert('L')

    # Resize images to the smaller one's size for comparison
    image1_size = image1.size
    image2_size = image2.size
    new_size = min(image1_size, image2_size)

    image1 = image1.resize(new_size, Image.Resampling.LANCZOS)
    image2 = image2.resize(new_size, Image.Resampling.LANCZOS)

    # Convert images to numpy arrays
    image1_array = np.array(image1)
    image2_array = np.array(image2)

    # Calculate SSIM between two images
    similarity_index = ssim(image1_array, image2_array)

    return similarity_index


def compare_audios(audio_path_1, audio_path_2):
    """
    Compare two audio files and return a similarity score in the range [0, 1].
    audio_path_1, audio_path_2: paths to the audio files to compare
    """
    # Example Usage:
    # similarity = compare_audios_simple('path_to_audio1.mp3', 'path_to_audio2.mp3')
    # print(f'Similarity Score: {similarity}')

    # Convert to common format if necessary and load audio
    if not audio_path_1 or not audio_path_2:
        return 0

    # Load the audio files and extract MFCC features
    y1, sr1 = librosa.load(audio_path_1)
    mfcc1 = librosa.feature.mfcc(y=y1, sr=sr1)

    y2, sr2 = librosa.load(audio_path_2)
    mfcc2 = librosa.feature.mfcc(y=y2, sr=sr2)

    # Normalize the MFCC features
    mfcc1 = librosa.util.normalize(mfcc1, axis=1)
    mfcc2 = librosa.util.normalize(mfcc2, axis=1)

    # Define a lambda function to compute cosine distance
    dist_func = lambda x, y: cosine(x, y)

    # Use the DTW algorithm to find the best alignment path
    distance, path = fastdtw(mfcc1.T, mfcc2.T, dist=dist_func)

    # Calculate the similarity score, here we use 1/(1+distance) to convert distance to a similarity score
    similarity = 1 / (1 + distance)

    return similarity


def compare_audios_by_dl_model(audio_path_1, audio_path_2):
    pass


def compare_videos(video_path1, video_path2, max_frames_to_check=100, threshold=5):
    # Open both video files
    cap1 = cv2.VideoCapture(video_path1)
    cap2 = cv2.VideoCapture(video_path2)

    frames_checked = 0
    mismatch_count = 0

    while frames_checked < max_frames_to_check:
        # Read frames from both videos
        ret1, frame1 = cap1.read()
        ret2, frame2 = cap2.read()

        # If a video ends, then check if both ended to confirm they are of the same length
        if not ret1 or not ret2:
            return ret1 == ret2

        # Convert frames to PIL Images
        frame1 = Image.fromarray(cv2.cvtColor(frame1, cv2.COLOR_BGR2RGB))
        frame2 = Image.fromarray(cv2.cvtColor(frame2, cv2.COLOR_BGR2RGB))

        # Compute the perceptual hash for each frame
        hash1 = imagehash.phash(frame1)
        hash2 = imagehash.phash(frame2)

        # Increment the frames checked
        frames_checked += 1

        # Compute the difference in the hashes
        if hash1 - hash2 > threshold:
            mismatch_count += 1
            # If there's a significant difference, the frames are not the same
            if mismatch_count > threshold:
                return False

    # If we reach here, the content appears to be the same
    return True


def are_audio_files_similar(mp3_file_path, mp4_file_path):
    # Extract audio fingerprint from MP3 file
    mp3_fingerprint, mp3_duration = acoustid.fingerprint_file(mp3_file_path)

    # Extract the audio stream from the MP4 file
    mp4_audio_path = os.path.splitext(mp4_file_path)[0] + '_extracted.mp3'
    try:
        subprocess.run(["ffmpeg", "-i", mp4_file_path, "-vn", "-ar", "44100", "-ac", "2", "-ab", "192k", "-f", "mp3",
                        mp4_audio_path], check=True)
    except subprocess.CalledProcessError as e:
        print(f"An error occurred during audio extraction from MP4: {e}")
        return False

    # Extract audio fingerprint from the extracted audio
    mp4_fingerprint, mp4_duration = acoustid.fingerprint_file(mp4_audio_path)

    # Clean up temporary extracted audio file
    os.remove(mp4_audio_path)

    # Compare fingerprints (rudimentary comparison)
    if mp3_duration >= mp4_duration and mp3_fingerprint == mp4_fingerprint:
        return True

    return False


def check_qt_bgcone(actual_config_path, rule):
    with open(actual_config_path, 'rb') as file:
        config_file = file.read().decode('utf-8')

    expected_qt_bgcone = rule['expected_qt_bgcone']
    if isinstance(expected_qt_bgcone, int):
        expected_qt_bgcone = str(expected_qt_bgcone)

    try:
        # The default value of qt_bgcone is 1, which means it is enabled
        qt_bgcone = "1"
        for line in config_file.split("\n"):
            # Check if the line contains the recording path setting
            if 'qt-bgcone=' in line:
                # Extract the value of the recording path and remove surrounding whitespace
                qt_bgcone = line.split('=')[-1].strip()
            # The configuration key was not found in the file

        if qt_bgcone == expected_qt_bgcone:
            return 1
        else:
            return 0
    except FileNotFoundError:
        logger.error("VLC configuration file not found.")
        return 0
    except Exception as e:
        logger.error(f"An error occurred: {e}")
        return 0


def check_qt_max_volume(actual_config_path, rule):
    logger.info(f"check_qt_max_volume CONFIG PATH: {actual_config_path}")
    logger.info(f"check_qt_max_volume RULE: {rule}")

    with open(actual_config_path, 'rb') as file:
        config_file = file.read().decode('utf-8')

    expected_qt_max_volume = rule['expected_qt_max_volume']
    if isinstance(expected_qt_max_volume, int):
        expected_qt_max_volume = str(expected_qt_max_volume)

    try:
        qt_max_volume = "125"
        for line in config_file.split("\n"):
            if 'qt-max-volume=' in line:
                qt_max_volume = line.split('=')[-1].strip()
            # The configuration key was not found in the file

        if qt_max_volume == expected_qt_max_volume:
            return 1
        else:
            return 0
    except FileNotFoundError:
        logger.error("VLC configuration file not found.")
        return 0
    except Exception as e:
        logger.error(f"An error occurred: {e}")
        return 0


def check_qt_minimal_view(actual_config_path, rule):

    logger.info(f"check_qt_minimal_view CONFIG PATH: {actual_config_path}")
    logger.info(f"check_qt_minimal_view RULE: {rule}")

    with open(actual_config_path, 'rb') as file:
        config_file = file.read().decode('utf-8')

    expected_qt_minimal_view = rule['expected_qt_minimal_view']
    if isinstance(expected_qt_minimal_view, int):
        expected_qt_minimal_view = str(expected_qt_minimal_view)

    try:
        qt_minimal_view = "0"
        for line in config_file.split("\n"):
            if 'qt-minimal-view=' in line:
                qt_minimal_view = line.split('=')[-1].strip()

        if qt_minimal_view == expected_qt_minimal_view:
            return 1
        else:
            return 0
    except FileNotFoundError:
        logger.error("VLC configuration file not found.")
        return 0
    except Exception as e:
        logger.error(f"An error occurred: {e}")
        return 0


def check_qt_video_resize(actual_config_path, rule):

    logger.info(f"check_qt_resize CONFIG PATH: {actual_config_path}")
    logger.info(f"check_qt_resize RULE: {rule}")

    with open(actual_config_path, 'rb') as file:
        config_file = file.read().decode('utf-8')

    expected_qt_resize = rule['qt-video-autoresize']
    if isinstance(expected_qt_resize, int):
        expected_qt_resize = str(expected_qt_resize)

    try:
        qt_resize = "0"
        for line in config_file.split("\n"):
            if 'qt-video-autoresize=' in line:
                qt_resize = line.split('=')[-1].strip()

        if qt_resize == expected_qt_resize:
            return 1
        else:
            return 0
    except FileNotFoundError:
        logger.error("VLC configuration file not found.")
        return 0
    except Exception as e:
        logger.error(f"An error occurred: {e}")
        return 0


def check_qt_slider_colours(actual_config_path, rule):
    with open(actual_config_path, 'rb') as file:
        config_file = file.read().decode('utf-8')

    try:
        qt_slider_colours = "153;210;153;20;210;20;255;199;15;245;39;29"
        for line in config_file.split("\n"):
            if 'qt-slider-colours' in line:
                qt_slider_colours = line.split('=')[-1].strip()
            # The configuration key was not found in the file

        if rule['type'] == 'match':
            expected_qt_slider_colours = rule['expected_qt_slider_colours']
            if qt_slider_colours == expected_qt_slider_colours:
                return 1
            else:
                return 0
        elif rule['type'] == 'blackish':
            def is_color_blackish(rgb_values, threshold=100):
                # decide if the color is blackish
                return all(value < threshold for value in rgb_values)

            def parse_qt_slider_colours(colours_string):
                # parse the string of colours into a list of RGB tuples
                values = [int(x) for x in colours_string.split(';')]
                colors = list(zip(values[0::3], values[1::3], values[2::3]))
                return colors

            colors = parse_qt_slider_colours(qt_slider_colours)

            # check if all colors are blackish
            for color in colors:
                if is_color_blackish(color):
                    pass
                else:
                    return 0
            return 1

    except FileNotFoundError:
        logger.error("VLC configuration file not found.")
        return 0
    except Exception as e:
        logger.error(f"An error occurred: {e}")
        return 0


def check_global_key_play_pause(actual_config_path, rule):
    """
    # Play/Pause (str)
    #global-key-play-pause=

    # Play/Pause (str)
    #key-play-pause=Space
    """
    with open(actual_config_path, 'rb') as file:
        config_file = file.read().decode('utf-8')

    expected_global_key_play_pause = rule['expected_global_key_play_pause']

    if isinstance(expected_global_key_play_pause, int):
        expected_global_key_play_pause = str(expected_global_key_play_pause)

    try:
        global_key_play_pause = "0"
        for line in config_file.split("\n"):
            # Check if the line contains the recording path setting
            if 'global-key-play-pause=' in line:
                global_key_play_pause = "0" if line.split('=')[-1].strip() == "" else "1"

        if global_key_play_pause == expected_global_key_play_pause:
            return 1
        else:
            return 0
    except FileNotFoundError:
        logger.error("VLC configuration file not found.")
        return 0
    except Exception as e:
        logger.error(f"An error occurred: {e}")
        return 0


def check_one_instance_when_started_from_file(actual_config_path, rule):
    with open(actual_config_path, 'rb') as file:
        config_file = file.read().decode('utf-8')

    expected_one_instance_when_started_from_file = rule['expected_one_instance_when_started_from_file']

    if isinstance(expected_one_instance_when_started_from_file, int):
        expected_one_instance_when_started_from_file = str(expected_one_instance_when_started_from_file)

    try:
        one_instance_when_started_from_file = "1"
        for line in config_file.split("\n"):
            # Check if the line contains the recording path setting
            if 'one-instance-when-started-from-file=' in line:
                one_instance_when_started_from_file = line.split('=')[-1].strip()

        if one_instance_when_started_from_file == expected_one_instance_when_started_from_file:
            return 1
        else:
            return 0
    except FileNotFoundError:
        logger.error("VLC configuration file not found.")
        return 0
    except Exception as e:
        logger.error(f"An error occurred: {e}")
        return 0
