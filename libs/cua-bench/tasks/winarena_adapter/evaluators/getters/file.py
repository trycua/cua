import os
from typing import Dict, List, Set
from typing import Optional, Any, Union
from datetime import datetime
import requests
import pandas as pd
import logging

logger = logging.getLogger("desktopenv.getters.file")

def get_content_from_vm_file(env, config: Dict[str, Any]) -> Any:
    """
    Config:
        path (str): absolute path on the VM to fetch
    """

    path = config["path"]
    file_path = get_vm_file(env, {"path": path, "dest": os.path.basename(path)})
    file_type, file_content = config['file_type'], config['file_content']
    if file_type == 'xlsx':
        if file_content == 'last_row':
            df = pd.read_excel(file_path)
            last_row = df.iloc[-1]
            last_row_as_list = last_row.astype(str).tolist()
            return last_row_as_list
    else:
        raise NotImplementedError(f"File type {file_type} not supported")


def get_cloud_file(env, config: Dict[str, Any]) -> Union[str, List[str]]:
    """
    Config:
        path (str|List[str]): the url to download from
        dest (str|List[str])): file name of the downloaded file
        multi (bool) : optional. if path and dest are lists providing
          information of multiple files. defaults to False
        gives (List[int]): optional. defaults to [0]. which files are directly
          returned to the metric. if len==1, str is returned; else, list is
          returned.
    """

    if not config.get("multi", False):
        paths: List[str] = [config["path"]]
        dests: List[str] = [config["dest"]]
    else:
        paths: List[str] = config["path"]
        dests: List[str] = config["dest"]
    cache_paths: List[str] = []

    gives: Set[int] = set(config.get("gives", [0]))

    for i, (p, d) in enumerate(zip(paths, dests)):
        _path = os.path.join(env.cache_dir, d)
        if i in gives:
            cache_paths.append(_path)

        if os.path.exists(_path):
            #return _path
            continue

        url = p
        response = requests.get(url, stream=True)
        response.raise_for_status()

        with open(_path, 'wb') as f:
            for chunk in response.iter_content(chunk_size=8192):
                if chunk:
                    f.write(chunk)

    return cache_paths[0] if len(cache_paths)==1 else cache_paths


def get_vm_file(env, config: Dict[str, Any]) -> Union[Optional[str], List[Optional[str]]]:
    """
    Config:
        path (str): absolute path on the VM to fetch
        dest (str): file name of the downloaded file
        multi (bool) : optional. if path and dest are lists providing
          information of multiple files. defaults to False
        gives (List[int]): optional. defaults to [0]. which files are directly
          returned to the metric. if len==1, str is returned; else, list is
          returned.
        only support for single file now:
        time_suffix(bool): optional. defaults to False. if True, append the current time in required format.
        time_format(str): optional. defaults to "%Y_%m_%d". format of the time suffix.
    """
    time_format = "%Y_%m_%d"
    if not config.get("multi", False):
        paths: List[str] = [config["path"]]
        dests: List[str] = [config["dest"]]
        if "time_suffix" in config.keys() and config["time_suffix"]:
            if "time_format" in config.keys():
                time_format = config["time_format"]
            # Insert time before . in file type suffix
            paths = [p.split(".")[0] + datetime.now().strftime(time_format) + "." + p.split(".")[1] if "." in p else p for p in paths]
            dests = [d.split(".")[0] + datetime.now().strftime(time_format) + "." + d.split(".")[1] if "." in d else d for d in dests]
    else:
        paths: List[str] = config["path"]
        dests: List[str] = config["dest"]


    cache_paths: List[str] = []

    gives: Set[int] = set(config.get("gives", [0]))

    for i, (p, d) in enumerate(zip(paths, dests)):
        _path = os.path.join(env.cache_dir, d)
        file = env.controller.get_file(p)
        if file is None:
            #return None
            # raise FileNotFoundError("File not found on VM: {:}".format(config["path"]))
            if i in gives:
                cache_paths.append(None)
            continue

        if i in gives:
            cache_paths.append(_path)
        with open(_path, "wb") as f:
            f.write(file)
    return cache_paths[0] if len(cache_paths)==1 else cache_paths


def get_cache_file(env, config: Dict[str, str]) -> str:
    """
    Config:
        path (str): relative path in cache dir
    """

    _path = os.path.join(env.cache_dir, config["path"])
    assert os.path.exists(_path)
    return _path


def get_vm_file_exists_in_vm_folder(env, config) -> str:
    folder_name = config["folder_name"]
    logger.info(f"Folder name: {folder_name}")
    file_name = config["file_name"]
    logger.info(f"File name: {file_name}")
    if not folder_name or not file_name:
        return 0.0
    
    file_path = os.path.join(folder_name, file_name)
    if env.controller.get_file(file_path):
        return 1.0
    else:
        return 0.0