import os
import xml.etree.ElementTree as ET
import zipfile
from typing import Dict

from .file import get_vm_file


def get_background_image_in_slide(env, config: Dict[str, str]):
    ppt_file_path, slide_index, dest = (
        config["ppt_file_path"],
        int(config["slide_index"]),
        config["dest"],
    )
    image_id, image_file_path = None, None

    ppt_file_localhost_path = get_vm_file(
        env, {"path": ppt_file_path, "dest": os.path.split(ppt_file_path)[-1]}
    )

    with zipfile.ZipFile(ppt_file_localhost_path, "r") as myzip:
        slide1_xml_file = "ppt/slides/slide{}.xml".format(slide_index + 1)
        # firstly, check whether the background image is used in the slide
        if slide1_xml_file not in myzip.namelist():
            return None
        with myzip.open(slide1_xml_file) as f:
            # Parse the XML tree from the relationships file
            tree = ET.parse(f)
            root = tree.getroot()
            bg_tag = "{http://schemas.openxmlformats.org/presentationml/2006/main}bgPr"
            image_tag = "{http://schemas.openxmlformats.org/drawingml/2006/main}blip"
            attr_tag = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}embed"
            for child in root.iter(bg_tag):
                try:
                    for element in child.iter(image_tag):
                        image_id = element.attrib[attr_tag]
                        break
                except Exception:
                    pass
                if image_id is not None:
                    break
            else:
                return None

        # next, extract the background image from the slide
        slide1_rels_file = "ppt/slides/_rels/slide{}.xml.rels".format(slide_index + 1)
        if slide1_rels_file in myzip.namelist():
            with myzip.open(slide1_rels_file) as f:
                # Parse the XML tree from the relationships file
                tree = ET.parse(f)
                root = tree.getroot()
                # Define the namespace used in the relationships file
                namespaces = {"r": "http://schemas.openxmlformats.org/package/2006/relationships"}
                # Look for all relationship elements that have a type attribute for image
                for rel in root.findall("r:Relationship", namespaces):
                    # Check if the relationship is for an image file
                    if "image" in rel.attrib["Type"] and rel.attrib["Id"] == image_id:
                        target = rel.attrib["Target"]
                        if target.startswith(".."):
                            # Resolve the relative path to get the correct path within the zip file
                            image_file_path = os.path.normpath(os.path.join("ppt/slides", target))
                            # Replace backslashes with forward slashes for ZIP compatibility
                            image_file_path = image_file_path.replace("\\", "/")
                            tmpdirname = os.path.dirname(ppt_file_localhost_path)
                            myzip.extract(image_file_path, tmpdirname)
                            image_file_path = os.path.join(tmpdirname, image_file_path)
                            return image_file_path
                        else:  # absolute path
                            assert target.startswith("file://"), target
                            image_file_path = target[7:]
                        break
    if image_file_path is None:
        return None

    else:
        # Get the audio file from vm and return the file path in the host
        return get_vm_file(env, {"path": image_file_path, "dest": dest})


def get_audio_in_slide(env, config: Dict[str, str]):
    ppt_file_path, slide_index, dest = (
        config["ppt_file_path"],
        int(config["slide_index"]),
        config["dest"],
    )

    # Open the .pptx file as a zip file, fixme: now we assume there is only one audio file in the slides
    audio_file_path = None

    ppt_file_localhost_path = get_vm_file(
        env, {"path": ppt_file_path, "dest": os.path.split(ppt_file_path)[-1]}
    )

    with zipfile.ZipFile(ppt_file_localhost_path, "r") as myzip:
        # Find the relationships XML file for the first slide
        slide1_rels_file = "ppt/slides/_rels/slide{}.xml.rels".format(slide_index + 1)
        if slide1_rels_file in myzip.namelist():
            with myzip.open(slide1_rels_file) as f:
                # Parse the XML tree from the relationships file
                tree = ET.parse(f)
                root = tree.getroot()
                # Define the namespace used in the relationships file
                namespaces = {"r": "http://schemas.openxmlformats.org/package/2006/relationships"}
                # Look for all relationship elements that have a type attribute for audio
                for rel in root.findall("r:Relationship", namespaces):
                    # Check if the relationship is for an audio file
                    if "audio" in rel.attrib["Type"]:
                        # The audio can be embedded inside the file or linked to an external file
                        # Get the target attribute which contains the audio file path
                        target = rel.attrib["Target"]

                        if target.startswith(".."):
                            # Resolve the relative path to get the correct path within the zip file
                            audio_file_path = os.path.normpath(os.path.join("ppt/slides", target))
                            # Replace backslashes with forward slashes for ZIP compatibility
                            audio_file_path = audio_file_path.replace("\\", "/")

                            # Create a temporary directory to extract the audio file
                            tmpdirname = os.path.dirname(ppt_file_localhost_path)
                            myzip.extract(audio_file_path, tmpdirname)
                            audio_file_path = os.path.join(tmpdirname, audio_file_path)
                            return audio_file_path
                            # with tempfile.TemporaryDirectory() as tmpdirname:
                            #     # Extract the audio file
                            #     myzip.extract(audio_file_path, tmpdirname)
                            #     # Get the full path of the extracted audio file
                            #     extracted_audio_path = os.path.join(tmpdirname, audio_file_path)
                            #     # Return the extracted audio file path
                            #     audio_file_path = extracted_audio_path
                        else:
                            # the audio file is external to the .pptx file
                            # Return the audio file path
                            assert target.startswith("file://"), target
                            audio_file_path = target[7:]
                        break
    if audio_file_path is None:
        return None

    else:
        # Get the audio file from vm and return the file path in the host
        return get_vm_file(env, {"path": audio_file_path, "dest": dest})
