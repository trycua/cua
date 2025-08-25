"""
Agent loops for agent
"""

# Import the loops to register them
from . import anthropic
from . import openai
from . import uitars
from . import omniparser
from . import gta1
from . import composed_grounded
from . import glm45v
from . import opencua

__all__ = [
    "anthropic", 
    "openai", 
    "uitars", 
    "omniparser", 
    "gta1", 
    "composed_grounded", 
    "glm45v", 
    "opencua"
]