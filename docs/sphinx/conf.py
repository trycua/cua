import os
import sys

# Add the Python libs to the path
sys.path.insert(0, os.path.abspath("../../libs/python"))

project = "CUA Python SDK"
copyright = "2024, TryCua"
author = "TryCua"

extensions = [
    "sphinx.ext.autodoc",
    "sphinx.ext.napoleon",
    "sphinx.ext.viewcode",
    "sphinx_autodoc_typehints",
    "myst_parser",
    "sphinx_markdown_builder",
]

autodoc_default_options = {
    "members": True,
    "undoc-members": True,
    "show-inheritance": True,
}

# Napoleon settings for Google/NumPy style docstrings
napoleon_google_docstring = True
napoleon_numpy_docstring = True
napoleon_include_init_with_doc = False
napoleon_include_private_with_doc = False

# Type hints settings
typehints_fully_qualified = False
always_document_param_types = True

# Markdown builder settings
markdown_uri_doc_suffix = ".md"
