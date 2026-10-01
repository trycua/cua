import logging
import os
import re
import xml.etree.ElementTree as ET
import zipfile
from io import BytesIO
from typing import Any, Dict, List

from docx import Document
from docx.enum.text import WD_PARAGRAPH_ALIGNMENT, WD_TAB_ALIGNMENT
from odf.opendocument import load
from odf.text import P, Span
from PIL import Image
from rapidfuzz import fuzz

logger = logging.getLogger("desktopenv.metric.docs")


def find_default_font(config_file_path, rules):
    """Find the default font in LibreOffice Writer."""
    default_font = None
    expected_font = rules["font_name"]

    if not config_file_path:
        return 0

    try:
        tree = ET.parse(config_file_path)
        root = tree.getroot()

        # Define the XML namespace used in the file
        namespace = {"oor": "http://openoffice.org/2001/registry"}

        # Search for the node containing the default font setting for LibreOffice Writer
        for elem in root.findall(
            './/item[@oor:path="/org.openoffice.Office.Writer/DefaultFont"]', namespace
        ):
            for prop in elem.findall('.//prop[@oor:name="Standard"]', namespace):
                for value in prop.findall("value", namespace):
                    default_font = value.text
    except Exception as e:
        logger.error(f"Error: {e}")

    return 1 if default_font == expected_font else 0


def contains_page_break(docx_file):
    if not docx_file:
        return 0

    try:
        doc = Document(docx_file)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    namespaces = {"w": "http://schemas.openxmlformats.org/wordprocessingml/2006/main"}

    for paragraph in doc.paragraphs:
        for run in paragraph.runs:
            br_elems = run.element.findall(".//w:br", namespaces)
            for br in br_elems:
                if (
                    br is not None
                    and "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}type"
                    in br.attrib
                    and br.attrib[
                        "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}type"
                    ]
                    == "page"
                ):
                    # logger.info(f"Page break found in {br}, {print(br_elems)}, {print(run)}, {print(paragraph)}")
                    return 1
    return 0


def compare_docx_files(file1, file2, **options):
    ignore_blanks = options.get("ignore_blanks", True)
    ignore_case = options.get("ignore_case", False)
    ignore_order = options.get("ignore_order", False)
    content_only = options.get("content_only", False)

    if not file1 or not file2:
        return 0

    def get_paragraph_texts_odt(document):
        paragraphs = document.getElementsByType(P)
        paragraph_texts = []
        for paragraph in paragraphs:
            text_parts = []
            for node in paragraph.childNodes:
                if node.nodeType == node.TEXT_NODE:
                    text_parts.append(node.data)
                elif node.nodeType == node.ELEMENT_NODE and node.tagName == "text:span":
                    # Assuming direct text content in <text:span>, for simplicity
                    for child in node.childNodes:
                        if child.nodeType == child.TEXT_NODE:
                            text_parts.append(child.data)
            paragraph_texts.append("".join(text_parts))
        return paragraph_texts

    # Determine file types and load documents
    if file1.endswith(".docx") and file2.endswith(".docx"):
        try:
            doc1 = Document(file1)
            doc2 = Document(file2)
        except Exception as e:
            logger.error(f"Error: {e}")
            return 0
        doc1_paragraphs = [p.text for p in doc1.paragraphs]
        doc2_paragraphs = [p.text for p in doc2.paragraphs]
        if ignore_order:
            doc1_paragraphs = sorted(doc1_paragraphs)
            doc2_paragraphs = sorted(doc2_paragraphs)
    elif file1.endswith(".odt") and file2.endswith(".odt"):
        try:
            doc1 = load(file1)
            doc2 = load(file2)
        except Exception as e:
            logger.error(f"Error: {e}")
            return 0
        doc1_paragraphs = get_paragraph_texts_odt(doc1)
        doc2_paragraphs = get_paragraph_texts_odt(doc2)
        if ignore_order:
            doc1_paragraphs = sorted(doc1_paragraphs)
            doc2_paragraphs = sorted(doc2_paragraphs)
    else:
        # Unsupported file types or mismatch
        print("Unsupported file types or mismatch between file types.")
        return 0

    if content_only:
        # Compare the content of the documents
        text1 = re.sub(r"\s+", " ", "\n".join(doc1_paragraphs)).strip()
        text2 = re.sub(r"\s+", " ", "\n".join(doc2_paragraphs)).strip()
        if ignore_case:
            text1, text2 = text1.lower(), text2.lower()
        similarity = fuzz.ratio(text1, text2) / 100.0
        return similarity

    # Process and compare documents
    if ignore_blanks:
        text1 = re.sub(r"\s+", " ", "\n".join(doc1_paragraphs)).strip()
        text2 = re.sub(r"\s+", " ", "\n".join(doc2_paragraphs)).strip()
        if ignore_case:
            text1, text2 = text1.lower(), text2.lower()
        if text1 != text2:
            return 0
    else:
        print("ignore_blanks=false")
        if len(doc1_paragraphs) != len(doc2_paragraphs):
            print(doc1_paragraphs)
            print(doc2_paragraphs)
            print(len(doc1_paragraphs))
            print(len(doc2_paragraphs))
            return 0
        print("in compare")
        # Compare each paragraph
        for p1, p2 in zip(doc1_paragraphs, doc2_paragraphs):
            if ignore_case:
                p1, p2 = p1.lower(), p2.lower()
            if p1 != p2:
                print(p1)
                print(p2)
                return 0

    return 1


def compare_init_lines(file1, file2):
    if not file1 or not file2:
        return 0

    try:
        doc1 = Document(file1)
        doc2 = Document(file2)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    doc1_paragraphs = [p.text for p in doc1.paragraphs]
    doc2_paragraphs = [p.text for p in doc2.paragraphs]

    # Compare each paragraph
    for p1, p2 in zip(doc1_paragraphs, doc2_paragraphs):
        if p1 != p2:
            # print(p1)
            # print(p2)
            return 0

    return 1


def compare_docx_tables(docx_file1, docx_file2):
    if not docx_file1 or not docx_file2:
        return 0

    try:
        doc1 = Document(docx_file1)
        doc2 = Document(docx_file2)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    # get list of tables in docx
    tables1 = doc1.tables
    tables2 = doc2.tables

    if len(tables1) != len(tables2):
        return 0

    # Compare each table content
    for table1, table2 in zip(tables1, tables2):

        if len(table1.rows) != len(table2.rows) or len(table1.columns) != len(table2.columns):
            return 0

        # Compare each cell
        for i in range(len(table1.rows)):
            for j in range(len(table1.columns)):
                if table1.cell(i, j).text.strip() != table2.cell(i, j).text.strip():
                    return 0

    return 1


def compare_docx_images(docx_file1, docx_file2):
    if not docx_file1 or not docx_file2:
        return 0

    try:
        doc1 = Document(docx_file1)
        doc2 = Document(docx_file2)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    def extract_images(doc):
        images = []
        for rel in doc.part.rels.values():
            if "image" in rel.reltype:
                img_data = rel.target_part.blob
                images.append(BytesIO(img_data))
        return images

    images1 = extract_images(doc1)
    images2 = extract_images(doc2)
    if len(images1) != len(images2):
        return 0
    for img1, img2 in zip(images1, images2):
        if Image.open(img1).tobytes() != Image.open(img2).tobytes():
            return 0
    return 1


def compare_line_spacing(docx_file1, docx_file2):
    if not docx_file1 or not docx_file2:
        return 0

    if not compare_docx_files(docx_file1, docx_file2):
        return 0

    try:
        doc1 = Document(docx_file1)
        doc2 = Document(docx_file2)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    if len(doc1.paragraphs) != len(doc2.paragraphs):
        return 0

    # Compare each paragraph line spacing
    for para1, para2 in zip(doc1.paragraphs, doc2.paragraphs):

        spacing1 = para1.paragraph_format.line_spacing
        spacing2 = para2.paragraph_format.line_spacing

        if spacing1 != spacing2:
            return 0

    return 1


def compare_insert_equation(docx_file1, docx_file2):
    if not docx_file1 or not docx_file2:
        return 0

    if not compare_docx_files(docx_file1, docx_file2):
        return 0

    try:
        doc1 = Document(docx_file1)
        doc2 = Document(docx_file2)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    # Compare each paragraph if it contains equation
    for para1, para2 in zip(doc1.paragraphs, doc2.paragraphs):
        for run1, run2 in zip(para1.runs, para2.runs):
            if run1.element.xpath(".//w:object") and run2.element.xpath(".//w:object"):
                return 1
    return 0


def compare_font_names(docx_file, rules: List[Dict[str, Any]]):
    if not docx_file:
        return 0

    try:
        doc = Document(docx_file)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    expected_font = rules["font_name"]

    for paragraph in doc.paragraphs:
        for run in paragraph.runs:
            font_name = run.font.name
            if font_name != expected_font:
                return 0
    return 1


def compare_subscript_contains(docx_file1, docx_file2):
    if not docx_file1 or not docx_file2:
        return 0

    try:
        doc1 = Document(docx_file1)
        doc2 = Document(docx_file2)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    for para1, para2 in zip(doc1.paragraphs, doc2.paragraphs):
        for run1, run2 in zip(para1.runs, para2.runs):
            # check if two paras both contain subscript
            if run1.font.subscript and run2.font.subscript:
                return 1
    return 0


def has_page_numbers_in_footers(docx_file):
    if not docx_file:
        return 0

    try:
        doc = Document(docx_file)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    for section in doc.sections:
        footer = section.footer
        if footer is None:
            return 0
        footer_text = footer.paragraphs[0].text if footer.paragraphs else ""
        if not any(char.isdigit() for char in footer_text):
            # if no digit in footer, then no page number
            return 0
    return 1


def is_first_line_centered(docx_file):
    if not docx_file:
        return 0

    try:
        doc = Document(docx_file)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    first_paragraph = doc.paragraphs[0]

    # check if the first line is center justified
    return 1 if first_paragraph.paragraph_format.alignment == WD_PARAGRAPH_ALIGNMENT.CENTER else 0


def check_file_exists(directory, filename):
    if not directory or not filename:
        return 0
    file_path = os.path.join(directory, filename)
    return 1 if os.path.isfile(file_path) else 0


def check_tabstops(docx_file1, docx_file2, **kwargs) -> float:
    if not docx_file1 or not docx_file2:
        return 0.0

    try:
        doc1: Document = Document(docx_file1)
        doc2: Document = Document(docx_file2)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0.0

    para1 = [p for p in doc1.paragraphs if p.text.strip()]
    para2 = [p for p in doc2.paragraphs if p.text.strip()]
    if len(para1) != len(para2):
        return 0.0

    if kwargs.get("word_number_split_by_tabstop", None) is not None:
        number = kwargs["word_number_split_by_tabstop"]
        index = kwargs.get("index", 0)
        for p1 in para1:
            splits = p1.text.split("\t")
            if len(splits) == 0:
                logger.info("d1 splits")
                return 0.0
            words = list(filter(lambda x: x.strip(), re.split(r"\s", splits[index])))
            if len(words) != number:
                return 0.0

    section = doc2.sections[0]
    paragraph_width = section.page_width - section.left_margin - section.right_margin

    def ignore_tabs(x):
        return x.alignment == WD_TAB_ALIGNMENT.CLEAR or (
            x.alignment == WD_TAB_ALIGNMENT.LEFT and x.position == 0
        )

    minus = 0.0
    for p1, p2 in zip(para1, para2):
        # filter CLEAR tabstop and default left-0 tabstop
        tabs1 = [tst for tst in p1.paragraph_format.tab_stops if not ignore_tabs(tst)]
        tabs2 = [tst for tst in p2.paragraph_format.tab_stops if not ignore_tabs(tst)]
        if len(tabs1) != len(tabs2):
            return 0.0
        difference = 0.0
        for t1, t2 in zip(tabs1, tabs2):
            if t1.alignment != t2.alignment:
                return 0.0
            difference += abs(t1.position - t2.position)
        minus += difference / paragraph_width
    score = 1 - (minus / len(para1))
    return score


def compare_contains_image(docx_file1, docx_file2):
    if not docx_file1 or not docx_file2:
        return 0

    try:
        doc1 = Document(docx_file1)
        doc2 = Document(docx_file2)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    for para1, para2 in zip(doc1.paragraphs, doc2.paragraphs):
        for run1, run2 in zip(para1.runs, para2.runs):
            logger.info(f"run1xml: {run1._element.xml}, run2xml: {run2._element.xml}")
            if ("graphicData" in run1._element.xml and "graphicData" not in run2._element.xml) or (
                "graphicData" not in run1._element.xml and "graphicData" in run2._element.xml
            ):
                return 0
    return 1


def check_highlighted_words(file_path1, file_path2):
    if not file_path1 or not file_path2:
        return 0

    if not compare_docx_files(file_path1, file_path2):
        return 0

    doc = load(file_path1)
    highlighted = False

    for span in doc.getElementsByType(Span):
        style_name = span.getAttribute("stylename")
        if style_name:
            for automatic_style in doc.automaticstyles.childNodes:
                if automatic_style.getAttribute("name") == style_name:
                    for property in automatic_style.childNodes:
                        if property.getAttribute("backgroundcolor") == "#ffff00":
                            highlighted = True
                            break
            if highlighted:
                break

    return 0 if highlighted else 1


def evaluate_strike_through_last_paragraph(file_path1, file_path2):
    if not file_path1 or not file_path2:
        return 0

    if not compare_docx_files(file_path1, file_path2):
        return 0

    try:
        document = Document(file_path1)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    # Get the last paragraph
    last_paragraph = document.paragraphs[-1]

    # Check if any run in the last paragraph has strike-through formatting
    for run in last_paragraph.runs:
        if not run.font.strike:
            return 0  # At least one word does not have strike-through formatting

    return 1  # All words in the last paragraph have strike-through formatting


def evaluate_conversion(file_path):
    if not file_path:
        return 0

    try:
        document = Document(file_path)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    for table in document.tables:
        for row in table.rows:
            for cell in row.cells:
                for paragraph in cell.paragraphs:
                    for run in paragraph.runs:
                        if run.text.isupper():
                            return 0  # Uppercase text should be converted to lowercase

    for paragraph in document.paragraphs:
        for run in paragraph.runs:
            if run.text.isupper():
                return 0  # Uppercase text should be converted to lowercase

    return 1  # All uppercase text has been successfully converted


def evaluate_spacing(file_path):
    if not file_path:
        return 0

    try:
        document = Document(file_path)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    # Check line spacing for introduction, body, and conclusion
    introduction_spacing = document.paragraphs[0].paragraph_format.line_spacing
    body_spacing = document.paragraphs[1].paragraph_format.line_spacing
    conclusion_spacing = document.paragraphs[2].paragraph_format.line_spacing
    if introduction_spacing == 1.0 and body_spacing == 2.0 and conclusion_spacing == 1.5:
        return 1
    else:
        return 0


def check_italic_font_size_14(path1, path2):
    if not path1 or not path2:
        return 0

    if not compare_docx_files(path1, path2):
        return 0

    try:
        document = Document(path1)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    for paragraph in document.paragraphs:
        for run in paragraph.runs:
            if run.italic:
                # Check if font size is 14
                if run.font.size is None or run.font.size.pt != 14:
                    return 0
    return 1


def evaluate_alignment(docx_path):
    if not docx_path:
        return 0

    # Load the document
    try:
        doc = Document(docx_path)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    # Iterate through each paragraph in the document
    for para in doc.paragraphs:
        # Split the paragraph into individual sentences
        sentences = para.text.split(".")

        for sentence in sentences:
            # Split the sentence into words
            words = sentence.strip().split()

            # Check if the sentence has at least three words
            if len(words) < 3:
                continue  # Skip sentences with less than three words

            # The first three words should be separated from the rest
            first_part = " ".join(words[:3])
            second_part = " ".join(words[3:])

            # Check if the sentence structure matches the pattern: first part + large space/tab + second part
            if not (
                first_part in sentence
                and second_part in sentence
                and sentence.find(first_part) < sentence.find(second_part)
            ):
                return 0  # The sentence does not meet the alignment criteria

    return 1  # All sentences meet the alignment criteria


def get_unique_train_ids(initial_file):  # fixed standard
    if not initial_file:
        return set(), 0

    try:
        doc = Document(initial_file)
    except Exception as e:
        logger.error(f"Error: {e}")
        return set(), 0

    train_ids = set()
    processed_lines = 0

    for para in doc.paragraphs:
        line_parts = para.text.split(",")
        if len(line_parts) == 4:
            train_id = line_parts[1].strip()
            if train_id not in train_ids:
                train_ids.add(train_id)
                processed_lines += 1

    return train_ids, processed_lines


def check_no_duplicates(initial_file, processed_file):
    if not initial_file or not processed_file:
        return 0

    # Open the document
    train_ids_ini, ini_lines = get_unique_train_ids(initial_file)

    try:
        doc_processed = Document(processed_file)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    train_ids_pro = set()
    processed_lines = 0  # Counter for valid lines processed

    # processed
    for para in doc_processed.paragraphs:
        # Each line has the format: time_HH:MM:SS, train_id, station_id, platform_no
        line_parts = para.text.split(",")
        # Ensure the line has the correct format
        if len(line_parts) == 4:
            train_id = line_parts[1].strip()
            # If train_id is already in the set, it's a duplicate
            if train_id in train_ids_pro:
                return 0  # Duplicate found
            train_ids_pro.add(train_id)
            processed_lines += 1  # Increment valid lines counter

    if train_ids_pro != train_ids_ini or processed_lines != ini_lines:
        return 0

    # No duplicates found and at least one valid line was processed
    return 1


def compare_docx_lines(file1, file2):
    if not file1 or not file2:
        return 0

    # Read the text of the document, line by line
    try:
        doc1 = Document(file1)
        doc2 = Document(file2)
    except Exception as e:
        logger.error(f"Error: {e}")
        return 0

    doc1_lines = [p.text.strip() for p in doc1.paragraphs if p.text.strip()]
    doc2_lines = [p.text.strip() for p in doc2.paragraphs if p.text.strip()]
    # print(doc1_lines)
    # print(doc2_lines)

    # Convert the list of lines to sets and compare
    if set(doc1_lines) == set(doc2_lines):
        return 1
    else:
        return 0


def compare_docx_files_and_ignore_new_lines(file1, file2, **options):
    ignore_blanks = options.get("ignore_blanks", True)

    if not file1 or not file2:
        return 0

    # Determine file types and load documents
    if file1.endswith(".docx") and file2.endswith(".docx"):
        try:
            doc1 = Document(file1)
            doc2 = Document(file2)
        except Exception as e:
            logger.error(f"Error: {e}")
            return 0

        # First, delete all the blank in paragraphs
        doc1 = [p for p in doc1.paragraphs if p.text != ""]
        doc2 = [p for p in doc2.paragraphs if p.text != ""]
        doc1_paragraphs = [p.text for p in doc1]
        doc2_paragraphs = [p.text for p in doc2]
    else:
        # Unsupported file types or mismatch
        print("Unsupported file types or mismatch between file types.")
        return 0

    # Process and compare documents
    if ignore_blanks:
        text1 = re.sub(r"\s+", " ", "\n".join(doc1_paragraphs)).strip()
        text2 = re.sub(r"\s+", " ", "\n".join(doc2_paragraphs)).strip()
        if text1 != text2:
            return 0
    else:
        if len(doc1_paragraphs) != len(doc2_paragraphs):
            return 0
        # Compare each paragraph
        for p1, p2 in zip(doc1_paragraphs, doc2_paragraphs):
            if p1 != p2:
                return 0
    return 1


# Docx file saved in the ubuntu cannot use this function to compare highlight, don't know why, deprecated
def compare_highlighted_text(file1, file2):
    if not file1 or not file2:
        return 0

    def extract_highlighted_text(file_path):
        highlighted_texts = []

        # Open the .docx file as a zip file and read the document.xml
        with zipfile.ZipFile(file_path, "r") as docx:
            with docx.open("word/document.xml") as document_xml:
                tree = ET.parse(document_xml)
                root = tree.getroot()

        # Define the namespaces
        namespaces = {
            "w": "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
        }

        # Find all runs with highlight property
        for run in root.findall(".//w:r", namespaces):
            highlight = run.find(".//w:highlight", namespaces)
            if (
                highlight is not None
                and highlight.get(
                    "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}val"
                )
                != "none"
            ):
                text = run.find(".//w:t", namespaces)
                if text is not None:
                    highlighted_texts.append(text.text)

        return highlighted_texts

    # Read the highlighted text from both documents
    doc1_highlighted = extract_highlighted_text(file1)
    doc2_highlighted = extract_highlighted_text(file2)

    # Compare the sets of highlighted text to check if they are the same
    if set(doc1_highlighted) == set(doc2_highlighted):
        return 1
    else:
        return 0


def compare_references(file1, file2, **options):
    if not file1 or not file2:
        return 0

    reference_indicator = options.get("reference_indicator", "References")
    reference_base_result = options.get("reference_base_result", 0.5)

    # Determine file types and load documents
    if file1.endswith(".docx") and file2.endswith(".docx"):
        try:
            doc1 = Document(file1)
            doc2 = Document(file2)
        except Exception as e:
            logger.error(f"Error: {e}")
            return 0

        doc1_paragraphs = [p.text for p in doc1.paragraphs]
        doc2_paragraphs = [p.text for p in doc2.paragraphs]
    else:
        # Unsupported file types or mismatch
        print("Unsupported file types or mismatch between file types.")
        return 0

    # Find the references section in the paragraphs, find the idx of the last reference_indicator in the paragraph list
    ref1_idx = (
        doc1_paragraphs.index(reference_indicator) if reference_indicator in doc1_paragraphs else -1
    )
    ref2_idx = (
        doc2_paragraphs.index(reference_indicator) if reference_indicator in doc2_paragraphs else -1
    )

    if ref1_idx == -1 and ref2_idx == -1:
        return 1

    if ref1_idx == -1 or ref2_idx == -1:
        return 0

    # split the reference section into reference items, and remove the empty string items
    ref1 = [p for p in doc1_paragraphs[ref1_idx + 1 :] if p.strip()]
    ref2 = [p for p in doc2_paragraphs[ref2_idx + 1 :] if p.strip()]

    # Compare the references

    if len(ref1) != len(ref2):
        return 0

    total_similarity = 0
    for r1, r2 in zip(ref1, ref2):
        # fuzzy match the references
        similarity = fuzz.ratio(r1, r2) / 100.0
        total_similarity += similarity

    result = total_similarity / len(ref1)
    if result >= reference_base_result:
        return (result - reference_base_result) / (1 - reference_base_result)
    else:
        return 0
