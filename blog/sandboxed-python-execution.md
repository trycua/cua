# Sandboxed Python Execution: Run Code Safely in Cua Containers

_Published on June 23, 2025 by Dillon DuPont_

Cua's computer-use capabilities that we touched on in [Building your own Operator on macOS - Part 2](build-your-own-operator-on-macos-2.md) – your AI agents can click, scroll, type, and interact with any desktop application. But what if your agent needs to do more than just UI automation? What if it needs to process data, make API calls, analyze images, or run complex logic alongside those UI interactions, within the same virtual environment?

That's where Cua's `@sandboxed` decorator comes in. While Cua handles the clicking and typing, sandboxed execution lets you run full Python code inside the same virtual environment. It's like giving your AI agents a programming brain to complement their clicking fingers.

Think of it as the perfect marriage: Cua handles the "what you see" (UI interactions), while sandboxed Python handles the "what you compute" (data processing, logic, API calls) – all happening in the same isolated environment.

## So, what exactly is sandboxed execution?

Cua excels at automating user interfaces – clicking buttons, filling forms, navigating applications. But modern AI agents need to do more than just UI automation. They need to process the data they collect, make intelligent decisions, call external APIs, and run sophisticated algorithms.

Sandboxed execution bridges this gap. You write a Python function, decorate it with `@sandboxed`, and it runs inside your Cua container alongside your UI automation. Your agent can now click a button, extract some data, process it with Python, and then use those results to decide what to click next.

Here's what makes this combination powerful for AI agent development:

- **Unified environment**: Your UI automation and code execution happen in the same container
- **Rich capabilities**: Combine Cua's clicking with Python's data processing, API calls, and libraries
- **Seamless integration**: Pass data between UI interactions and Python functions effortlessly
- **Cross-platform consistency**: Your Python code runs the same way across different Cua environments
- **Complete workflows**: Build agents that can both interact with apps AND process the data they collect

## The architecture behind @sandboxed

Let's jump right into an example that'll make this crystal clear:

```python
from computer.helpers import sandboxed

@sandboxed("demo_venv")
def greet_and_print(name):
    """This function runs inside the container"""
    import PyXA  # macOS-specific library
    safari = PyXA.Application("Safari")
    html = safari.current_document.source()
    print(f"Hello from inside the container, {name}!")
    return {"greeted": name, "safari_html": html}

# When called, this executes in the container
result = await greet_and_print("Cua")
```

What's happening here? When you call `greet_and_print()`, Cua extracts the function's source code, transmits it to the container, and executes it there. The result returns to you seamlessly, while the actual execution remains completely isolated.

## How does sandboxed execution work?

Cua's sandboxed execution system employs several key architectural components:

### 1. Source Code Extraction

Cua uses Python's `inspect.getsource()` to extract your function's source code and reconstruct the function definition in the remote environment.

### 2. Virtual Environment Isolation

Each sandboxed function runs in a named virtual environment within the container. This provides complete dependency isolation between different functions and their respective environments.

### 3. Data Serialization and Transport

Arguments and return values are serialized as JSON and transported between the host and container. This ensures compatibility across different Python versions and execution environments.

### 4. Comprehensive Error Handling

The system captures both successful results and exceptions, preserving stack traces and error information for debugging purposes.

## Getting your sandbox ready

Setting up sandboxed execution is simple:

```python
import asyncio
from computer.computer import Computer
from computer.helpers import sandboxed, set_default_computer

async def main():
    # Fire up the computer
    computer = Computer()
    await computer.run()

    # Make it the default for all sandboxed functions
    set_default_computer(computer)

    # Install some packages in a virtual environment
    await computer.venv_install("demo_venv", ["requests", "beautifulsoup4"])
```

If you want to get fancy, you can specify which computer instance to use:

```python
@sandboxed("my_venv", computer=my_specific_computer)
def my_function():
    # This runs on your specified computer instance
    pass
```

## Real-world examples that actually work

### Browser automation without the headaches

Ever tried to automate a browser and had it crash your entire system? Yeah, us too. Here's how to do it safely:

```python
@sandboxed("browser_env")
def automate_browser_with_playwright():
    """Automate browser interactions using Playwright"""
    from playwright.sync_api import sync_playwright
    import time
    import base64
    from datetime import datetime

    try:
        with sync_playwright() as p:
            # Launch browser (visible, because why not?)
            browser = p.chromium.launch(
                headless=False,
                args=['--no-sandbox', '--disable-dev-shm-usage']
            )

            page = browser.new_page()
            page.set_viewport_size({"width": 1280, "height": 720})

            actions = []
            screenshots = {}

            # Let's visit example.com and poke around
            page.goto("https://example.com")
            actions.append("Navigated to example.com")

            # Grab a screenshot because screenshots are cool
            screenshot_bytes = page.screenshot(full_page=True)
            screenshots["initial"] = base64.b64encode(screenshot_bytes).decode()

            # Get some basic info
            title = page.title()
            actions.append(f"Page title: {title}")

            # Find links and headings
            try:
                links = page.locator("a").all()
                link_texts = [link.text_content() for link in links[:5]]
                actions.append(f"Found {len(links)} links: {link_texts}")

                headings = page.locator("h1, h2, h3").all()
                heading_texts = [h.text_content() for h in headings[:3]]
                actions.append(f"Found headings: {heading_texts}")

            except Exception as e:
                actions.append(f"Element interaction error: {str(e)}")

            # Let's try a form for good measure
            try:
                page.goto("https://httpbin.org/forms/post")
                actions.append("Navigated to form page")

                # Fill out the form
                page.fill('input[name="custname"]', "Test User from Sandboxed Environment")
                page.fill('input[name="custtel"]', "555-0123")
                page.fill('input[name="custemail"]', "test@example.com")
                page.select_option('select[name="size"]', "large")

                actions.append("Filled out form fields")

                # Submit and see what happens
                page.click('input[type="submit"]')
                page.wait_for_load_state("networkidle")

                actions.append("Submitted form")

            except Exception as e:
                actions.append(f"Form interaction error: {str(e)}")

            browser.close()

            return {
                "actions_performed": actions,
                "screenshots": screenshots,
                "success": True
            }

    except Exception as e:
        return {"error": f"Browser automation failed: {str(e)}"}

# Install Playwright and its browsers
await computer.venv_install("browser_env", ["playwright"])
await computer.venv_cmd("browser_env", "playwright install chromium")

# Run the automation
result = await automate_browser_with_playwright()
print(f"Performed {len(result.get('actions_performed', []))} actions")
```

### Building code analysis agents

Want to build agents that can analyze code safely? Here's a security audit tool that won't accidentally `eval()` your system into oblivion:

```python
@sandboxed("analysis_env")
def security_audit_tool(code_snippet):
    """Analyze code for potential security issues"""
    import ast
    import re

    issues = []

    # Check for the usual suspects
    dangerous_patterns = [
        (r'eval\s*\(', "Use of eval() function"),
        (r'exec\s*\(', "Use of exec() function"),
        (r'__import__\s*\(', "Dynamic import usage"),
        (r'subprocess\.', "Subprocess usage"),
        (r'os\.system\s*\(', "OS system call"),
    ]

    for pattern, description in dangerous_patterns:
        if re.search(pattern, code_snippet):
            issues.append(description)

    # Get fancy with AST analysis
    try:
        tree = ast.parse(code_snippet)
        for node in ast.walk(tree):
            if isinstance(node, ast.Call):
                if hasattr(node.func, 'id'):
                    if node.func.id in ['eval', 'exec', 'compile']:
                        issues.append(f"Dangerous function call: {node.func.id}")
    except SyntaxError:
        issues.append("Syntax error in code")

    return {
        "security_issues": issues,
        "risk_level": "HIGH" if len(issues) > 2 else "MEDIUM" if issues else "LOW"
    }

# Test it on some sketchy code
audit_result = await security_audit_tool("eval(user_input)")
print(f"Security audit: {audit_result}")
```

### Desktop automation in the cloud

Here's where things get really interesting. Cua Cloud Sandbox comes with full desktop environments, so you can automate GUIs:

```python
@sandboxed("desktop_env")
def take_screenshot_and_analyze():
    """Take a screenshot and analyze the desktop"""
    import io
    import base64
    from PIL import ImageGrab
    from datetime import datetime

    try:
        # Grab the screen
        screenshot = ImageGrab.grab()

        # Convert to base64 for easy transport
        buffer = io.BytesIO()
        screenshot.save(buffer, format='PNG')
        screenshot_data = base64.b64encode(buffer.getvalue()).decode()

        # Get some basic info
        screen_info = {
            "size": screenshot.size,
            "mode": screenshot.mode,
            "timestamp": datetime.now().isoformat()
        }

        # Analyze the colors (because why not?)
        colors = screenshot.getcolors(maxcolors=256*256*256)
        dominant_color = max(colors, key=lambda x: x[0])[1] if colors else None

        return {
            "screenshot_base64": screenshot_data,
            "screen_info": screen_info,
            "dominant_color": dominant_color,
            "unique_colors": len(colors) if colors else 0
        }

    except Exception as e:
        return {"error": f"Screenshot failed: {str(e)}"}

# Install the dependencies
await computer.venv_install("desktop_env", ["Pillow"])

# Take and analyze a screenshot
result = await take_screenshot_and_analyze()
print("Desktop analysis complete!")
```

## Pro tips for sandboxed success

### Keep it self-contained

Always put your imports inside the function. Trust us on this one:

```python
@sandboxed("good_env")
def good_function():
    import os  # Import inside the function
    import json

    # Your code here
    return {"result": "success"}
```

### Install dependencies first

Don't forget to install packages before using them:

```python
# Install first
await computer.venv_install("my_env", ["pandas", "numpy", "matplotlib"])

@sandboxed("my_env")
def data_analysis():
    import pandas as pd
    import numpy as np
    # Now you can use them
```

### Use descriptive environment names

Future you will thank you:

```python
@sandboxed("data_processing_env")
def process_data(): pass

@sandboxed("web_scraping_env")
def scrape_site(): pass

@sandboxed("ml_training_env")
def train_model(): pass
```

### Always handle errors gracefully

Things break. Plan for it:

```python
@sandboxed("robust_env")
def robust_function(data):
    try:
        result = process_data(data)
        return {"success": True, "result": result}
    except Exception as e:
        return {"success": False, "error": str(e)}
```

## What about performance?

Let's be honest – there's some overhead here. Code needs to be serialized, sent over the network, and executed remotely. But for most use cases, the benefits far outweigh the costs.

If you're building something performance-critical, consider:

- Batching multiple operations into a single sandboxed function
- Minimizing data transfer between host and container
- Using persistent virtual environments

## The security angle

This is where sandboxed execution really shines:

1. **Complete process isolation** – code runs in a separate container
2. **File system protection** – limited access to your host files
3. **Network isolation** – controlled network access
4. **Clean environments** – no package conflicts or pollution
5. **Resource limits** – container-level constraints keep things in check

## Ready to get started?

The `@sandboxed` decorator is one of those features that sounds simple but opens up a world of possibilities. Whether you're testing sketchy code, building AI agents, or just want to keep your development environment pristine, it's got you covered.

Give it a try in your next Cua project and see how liberating it feels to run code without fear!

Happy coding (safely)!

---

_Want to dive deeper? Check out our [sandboxed functions examples](https://github.com/trycua/cua/blob/main/examples/sandboxed_functions_examples.py) and [virtual environment tests](https://github.com/trycua/cua/blob/main/tests/venv.py) on GitHub. Questions? Come chat with us on Discord!_
