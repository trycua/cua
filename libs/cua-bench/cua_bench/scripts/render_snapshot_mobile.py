"""Render mobile OS screenshots with different UI examples."""

import io
from pathlib import Path

from PIL import Image

from cua_bench.utils import render_windows


# Android-styled UIs
def create_settings_ui_android():
    """Create a settings screen UI with Android/Material Design styling."""
    return {
        "html": """
            <div style="font-family: 'Roboto', sans-serif; height: 100%; overflow-y: auto;">
                <div style="padding: 60px 20px 30px; color: black;">
                    <h1 style="margin: 0 0 5px; font-size: 28px; font-weight: 400;">Settings</h1>
                    <p style="margin: 0; opacity: 0.7; font-size: 14px; font-weight: 400;">Manage your preferences</p>
                </div>
                <div style="padding: 8px 0;">
                    <div style="padding: 16px 16px; display: flex; align-items: center; border-bottom: 1px solid #e0e0e0;">
                        <div style="width: 40px; height: 40px; background: #4CAF50; border-radius: 50%; display: flex; align-items: center; justify-content: center; margin-right: 16px; font-size: 20px;">👤</div>
                        <div style="flex: 1;"><div style="font-weight: 500; font-size: 16px; color: #212121;">Profile</div><div style="font-size: 14px; color: #757575;">View and edit profile</div></div>
                    </div>
                    <div style="padding: 16px 16px; display: flex; align-items: center; border-bottom: 1px solid #e0e0e0;">
                        <div style="width: 40px; height: 40px; background: #2196F3; border-radius: 50%; display: flex; align-items: center; justify-content: center; margin-right: 16px; font-size: 20px;">🔔</div>
                        <div style="flex: 1;"><div style="font-weight: 500; font-size: 16px; color: #212121;">Notifications</div><div style="font-size: 14px; color: #757575;">Manage alerts</div></div>
                    </div>
                    <div style="padding: 16px 16px; display: flex; align-items: center; border-bottom: 1px solid #e0e0e0;">
                        <div style="width: 40px; height: 40px; background: #FF9800; border-radius: 50%; display: flex; align-items: center; justify-content: center; margin-right: 16px; font-size: 20px;">🔒</div>
                        <div style="flex: 1;"><div style="font-weight: 500; font-size: 16px; color: #212121;">Privacy</div><div style="font-size: 14px; color: #757575;">Security settings</div></div>
                    </div>
                    <div style="padding: 16px 16px; display: flex; align-items: center; border-bottom: 1px solid #e0e0e0;">
                        <div style="width: 40px; height: 40px; background: #9C27B0; border-radius: 50%; display: flex; align-items: center; justify-content: center; margin-right: 16px; font-size: 20px;">🎨</div>
                        <div style="flex: 1;"><div style="font-weight: 500; font-size: 16px; color: #212121;">Appearance</div><div style="font-size: 14px; color: #757575;">Theme and display</div></div>
                    </div>
                    <div style="padding: 16px 16px; display: flex; align-items: center;">
                        <div style="width: 40px; height: 40px; background: #F44336; border-radius: 50%; display: flex; align-items: center; justify-content: center; margin-right: 16px; font-size: 20px;">ℹ️</div>
                        <div style="flex: 1;"><div style="font-weight: 500; font-size: 16px; color: #212121;">About</div><div style="font-size: 14px; color: #757575;">App information</div></div>
                    </div>
                </div>
            </div>
        """,
        "title": "Settings",
        "x": 0,
        "y": 0,
        "width": 360,
        "height": 640,
    }


def create_spreadsheet_ui_android():
    """Create a spreadsheet/table UI with Android/Material Design styling."""
    return {
        "html": """
            <div style="font-family: 'Roboto', sans-serif; height: 100%; display: flex; flex-direction: column;">
                <div style="padding: 60px 20px 20px; color: black;">
                    <h1 style="margin: 0; font-size: 24px; font-weight: 400;">Budget 2024</h1>
                </div>
                <div style="overflow-x: auto; flex: 1;">
                    <table style="width: 100%; border-collapse: collapse; font-size: 14px;">
                        <thead>
                            <tr style="background: #f5f5f5; border-bottom: 1px solid #e0e0e0;">
                                <th style="padding: 16px; text-align: left; font-weight: 500; color: #212121;">Category</th>
                                <th style="padding: 16px; text-align: right; font-weight: 500; color: #212121;">Amount</th>
                            </tr>
                        </thead>
                        <tbody>
                            <tr style="border-bottom: 1px solid #e0e0e0;">
                                <td style="padding: 16px; color: #212121;">🏠 Housing</td>
                                <td style="padding: 16px; text-align: right; font-weight: 400;">$1,200</td>
                            </tr>
                            <tr style="border-bottom: 1px solid #e0e0e0;">
                                <td style="padding: 16px; color: #212121;">🍔 Food</td>
                                <td style="padding: 16px; text-align: right; font-weight: 400;">$450</td>
                            </tr>
                            <tr style="border-bottom: 1px solid #e0e0e0;">
                                <td style="padding: 16px; color: #212121;">🚗 Transport</td>
                                <td style="padding: 16px; text-align: right; font-weight: 400;">$280</td>
                            </tr>
                            <tr style="border-bottom: 1px solid #e0e0e0;">
                                <td style="padding: 16px; color: #212121;">💡 Utilities</td>
                                <td style="padding: 16px; text-align: right; font-weight: 400;">$150</td>
                            </tr>
                            <tr style="border-bottom: 1px solid #e0e0e0;">
                                <td style="padding: 16px; color: #212121;">🎮 Entertainment</td>
                                <td style="padding: 16px; text-align: right; font-weight: 400;">$120</td>
                            </tr>
                            <tr style="border-bottom: 1px solid #e0e0e0;">
                                <td style="padding: 16px; color: #212121;">👕 Shopping</td>
                                <td style="padding: 16px; text-align: right; font-weight: 400;">$200</td>
                            </tr>
                            <tr style="background: #f5f5f5; border-top: 1px solid #e0e0e0;">
                                <td style="padding: 16px; font-weight: 500; color: #212121;">Total</td>
                                <td style="padding: 16px; text-align: right; font-weight: 500; color: #1976D2; font-size: 16px;">$2,400</td>
                            </tr>
                        </tbody>
                    </table>
                </div>
            </div>
        """,
        "title": "Budget",
        "x": 0,
        "y": 0,
        "width": 360,
        "height": 640,
    }


def create_minimal_ui_android():
    """Create a minimal example UI with Android/Material Design styling."""
    return {
        "html": """
            <div style="font-family: 'Roboto', sans-serif; height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; padding: 40px; color: black; text-align: center;">
                <div style="width: 80px; height: 80px; background: rgba(0,0,0,0.1); border-radius: 50%; display: flex; align-items: center; justify-content: center; margin-bottom: 30px; font-size: 40px;">✨</div>
                <h1 style="margin: 0 0 15px; font-size: 32px; font-weight: 400;">Welcome</h1>
                <p style="margin: 0 0 40px; font-size: 16px; line-height: 1.6; opacity: 0.7; max-width: 280px; font-weight: 400;">Get started with your journey to productivity and success.</p>
                <button style="background: #2196F3; color: white; border: none; padding: 14px 40px; border-radius: 4px; font-size: 14px; font-weight: 500; text-transform: uppercase; letter-spacing: 1.25px; box-shadow: 0 2px 4px rgba(0,0,0,0.2); cursor: pointer;">Get Started</button>
                <div style="margin-top: 50px; display: flex; gap: 30px;">
                    <div style="text-align: center;">
                        <div style="font-size: 28px; font-weight: 400; margin-bottom: 5px;">12K</div>
                        <div style="font-size: 13px; opacity: 0.7; font-weight: 400;">Users</div>
                    </div>
                    <div style="text-align: center;">
                        <div style="font-size: 28px; font-weight: 400; margin-bottom: 5px;">4.8</div>
                        <div style="font-size: 13px; opacity: 0.7; font-weight: 400;">Rating</div>
                    </div>
                    <div style="text-align: center;">
                        <div style="font-size: 28px; font-weight: 400; margin-bottom: 5px;">99%</div>
                        <div style="font-size: 13px; opacity: 0.7; font-weight: 400;">Uptime</div>
                    </div>
                </div>
            </div>
        """,
        "title": "Welcome",
        "x": 0,
        "y": 0,
        "width": 360,
        "height": 640,
    }


# iOS-styled UIs
def create_settings_ui_ios():
    """Create a settings screen UI with iOS styling."""
    return {
        "html": """
            <div style="font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Display', sans-serif; height: 100%; overflow-y: auto;">
                <div style="padding: 60px 20px 30px; color: black;">
                    <h1 style="margin: 0 0 5px; font-size: 34px; font-weight: 700; letter-spacing: -0.5px;">Settings</h1>
                    <p style="margin: 0; opacity: 0.6; font-size: 15px; font-weight: 400;">Manage your preferences</p>
                </div>
                <div style="padding: 15px;">
                    <div style="background: rgba(255,255,255,0.8); border-radius: 10px; padding: 0; margin-bottom: 15px; backdrop-filter: blur(10px);">
                        <div style="padding: 14px 16px; border-bottom: 0.5px solid rgba(0,0,0,0.1); display: flex; align-items: center;">
                            <div style="width: 32px; height: 32px; background: linear-gradient(135deg, #4CAF50, #45a049); border-radius: 8px; display: flex; align-items: center; justify-content: center; margin-right: 12px; font-size: 16px;">👤</div>
                            <div style="flex: 1;"><div style="font-weight: 400; font-size: 17px; color: black;">Profile</div><div style="font-size: 13px; color: #8e8e93;">View and edit profile</div></div>
                            <div style="color: #c7c7cc; font-size: 14px;">›</div>
                        </div>
                        <div style="padding: 14px 16px; border-bottom: 0.5px solid rgba(0,0,0,0.1); display: flex; align-items: center;">
                            <div style="width: 32px; height: 32px; background: linear-gradient(135deg, #2196F3, #1976d2); border-radius: 8px; display: flex; align-items: center; justify-content: center; margin-right: 12px; font-size: 16px;">🔔</div>
                            <div style="flex: 1;"><div style="font-weight: 400; font-size: 17px; color: black;">Notifications</div><div style="font-size: 13px; color: #8e8e93;">Manage alerts</div></div>
                            <div style="color: #c7c7cc; font-size: 14px;">›</div>
                        </div>
                        <div style="padding: 14px 16px; display: flex; align-items: center;">
                            <div style="width: 32px; height: 32px; background: linear-gradient(135deg, #FF9800, #f57c00); border-radius: 8px; display: flex; align-items: center; justify-content: center; margin-right: 12px; font-size: 16px;">🔒</div>
                            <div style="flex: 1;"><div style="font-weight: 400; font-size: 17px; color: black;">Privacy</div><div style="font-size: 13px; color: #8e8e93;">Security settings</div></div>
                            <div style="color: #c7c7cc; font-size: 14px;">›</div>
                        </div>
                    </div>
                    <div style="background: rgba(255,255,255,0.8); border-radius: 10px; padding: 0; backdrop-filter: blur(10px);">
                        <div style="padding: 14px 16px; border-bottom: 0.5px solid rgba(0,0,0,0.1); display: flex; align-items: center;">
                            <div style="width: 32px; height: 32px; background: linear-gradient(135deg, #9C27B0, #7b1fa2); border-radius: 8px; display: flex; align-items: center; justify-content: center; margin-right: 12px; font-size: 16px;">🎨</div>
                            <div style="flex: 1;"><div style="font-weight: 400; font-size: 17px; color: black;">Appearance</div><div style="font-size: 13px; color: #8e8e93;">Theme and display</div></div>
                            <div style="color: #c7c7cc; font-size: 14px;">›</div>
                        </div>
                        <div style="padding: 14px 16px; display: flex; align-items: center;">
                            <div style="width: 32px; height: 32px; background: linear-gradient(135deg, #F44336, #d32f2f); border-radius: 8px; display: flex; align-items: center; justify-content: center; margin-right: 12px; font-size: 16px;">ℹ️</div>
                            <div style="flex: 1;"><div style="font-weight: 400; font-size: 17px; color: black;">About</div><div style="font-size: 13px; color: #8e8e93;">App information</div></div>
                            <div style="color: #c7c7cc; font-size: 14px;">›</div>
                        </div>
                    </div>
                </div>
            </div>
        """,
        "title": "Settings",
        "x": 0,
        "y": 0,
        "width": 360,
        "height": 640,
    }


def create_spreadsheet_ui_ios():
    """Create a spreadsheet/table UI with iOS styling."""
    return {
        "html": """
            <div style="font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Display', sans-serif; height: 100%; display: flex; flex-direction: column;">
                <div style="padding: 60px 20px 20px; color: black;">
                    <h1 style="margin: 0; font-size: 28px; font-weight: 700; letter-spacing: -0.5px;">Budget 2024</h1>
                </div>
                <div style="overflow-x: auto; flex: 1;">
                    <table style="width: 100%; border-collapse: collapse; font-size: 15px;">
                        <thead>
                            <tr style="background: rgba(255,255,255,0.5); border-bottom: 0.5px solid #c6c6c8;">
                                <th style="padding: 12px 16px; text-align: left; font-weight: 600; color: black;">Category</th>
                                <th style="padding: 12px 16px; text-align: right; font-weight: 600; color: black;">Amount</th>
                            </tr>
                        </thead>
                        <tbody>
                            <tr style="border-bottom: 0.5px solid #c6c6c8;">
                                <td style="padding: 14px 16px; color: black; font-weight: 400;">🏠 Housing</td>
                                <td style="padding: 14px 16px; text-align: right; font-weight: 400;">$1,200</td>
                            </tr>
                            <tr style="border-bottom: 0.5px solid #c6c6c8;">
                                <td style="padding: 14px 16px; color: black; font-weight: 400;">🍔 Food</td>
                                <td style="padding: 14px 16px; text-align: right; font-weight: 400;">$450</td>
                            </tr>
                            <tr style="border-bottom: 0.5px solid #c6c6c8;">
                                <td style="padding: 14px 16px; color: black; font-weight: 400;">🚗 Transport</td>
                                <td style="padding: 14px 16px; text-align: right; font-weight: 400;">$280</td>
                            </tr>
                            <tr style="border-bottom: 0.5px solid #c6c6c8;">
                                <td style="padding: 14px 16px; color: black; font-weight: 400;">💡 Utilities</td>
                                <td style="padding: 14px 16px; text-align: right; font-weight: 400;">$150</td>
                            </tr>
                            <tr style="border-bottom: 0.5px solid #c6c6c8;">
                                <td style="padding: 14px 16px; color: black; font-weight: 400;">🎮 Entertainment</td>
                                <td style="padding: 14px 16px; text-align: right; font-weight: 400;">$120</td>
                            </tr>
                            <tr style="border-bottom: 0.5px solid #c6c6c8;">
                                <td style="padding: 14px 16px; color: black; font-weight: 400;">👕 Shopping</td>
                                <td style="padding: 14px 16px; text-align: right; font-weight: 400;">$200</td>
                            </tr>
                            <tr style="background: rgba(255,255,255,0.5); border-top: 0.5px solid #c6c6c8;">
                                <td style="padding: 16px; font-weight: 600; color: black;">Total</td>
                                <td style="padding: 16px; text-align: right; font-weight: 700; color: #007AFF; font-size: 17px;">$2,400</td>
                            </tr>
                        </tbody>
                    </table>
                </div>
            </div>
        """,
        "title": "Budget",
        "x": 0,
        "y": 0,
        "width": 360,
        "height": 640,
    }


def create_minimal_ui_ios():
    """Create a minimal example UI with iOS styling."""
    return {
        "html": """
            <div style="font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Display', sans-serif; height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; padding: 40px; color: black; text-align: center;">
                <div style="width: 80px; height: 80px; background: rgba(0,122,255,0.15); border-radius: 50%; display: flex; align-items: center; justify-content: center; margin-bottom: 30px; font-size: 40px;">✨</div>
                <h1 style="margin: 0 0 15px; font-size: 34px; font-weight: 700; letter-spacing: -0.5px;">Welcome</h1>
                <p style="margin: 0 0 40px; font-size: 17px; line-height: 1.5; opacity: 0.6; max-width: 280px; font-weight: 400;">Get started with your journey to productivity and success.</p>
                <button style="background: #007AFF; color: white; border: none; padding: 14px 40px; border-radius: 12px; font-size: 17px; font-weight: 600; cursor: pointer;">Get Started</button>
                <div style="margin-top: 50px; display: flex; gap: 30px;">
                    <div style="text-align: center;">
                        <div style="font-size: 28px; font-weight: 600; margin-bottom: 5px;">12K</div>
                        <div style="font-size: 13px; opacity: 0.6; font-weight: 400;">Users</div>
                    </div>
                    <div style="text-align: center;">
                        <div style="font-size: 28px; font-weight: 600; margin-bottom: 5px;">4.8</div>
                        <div style="font-size: 13px; opacity: 0.6; font-weight: 400;">Rating</div>
                    </div>
                    <div style="text-align: center;">
                        <div style="font-size: 28px; font-weight: 600; margin-bottom: 5px;">99%</div>
                        <div style="font-size: 13px; opacity: 0.6; font-weight: 400;">Uptime</div>
                    </div>
                </div>
            </div>
        """,
        "title": "Welcome",
        "x": 0,
        "y": 0,
        "width": 360,
        "height": 640,
    }


def main():
    """Generate mobile OS screenshots."""
    
    # Dimensions for each screenshot
    screenshot_width = 360
    screenshot_height = 640
    
    output_dir = Path(__file__).parent
    
    print("=" * 60)
    print("ANDROID SCREENSHOTS")
    print("=" * 60)
    
    # Generate Android screenshots with Android styling
    android_images = []
    android_uis = [
        ("Settings", create_settings_ui_android()),
        ("Spreadsheet", create_spreadsheet_ui_android()),
        ("Minimal", create_minimal_ui_android()),
    ]
    
    for i, (name, ui_func) in enumerate(android_uis):
        print(f"\nRendering Android {i+1}/3: {name}...")
        
        setup_config = {
            "os_type": "android",
            "width": screenshot_width,
            "height": screenshot_height,
            "background": "#1a1a1a",
        }
        
        screenshot_bytes = render_windows(
            provider="webtop",
            setup_config=setup_config,
            windows=[ui_func],
            screenshot_delay=0.5,
        )
        
        img = Image.open(io.BytesIO(screenshot_bytes))
        android_images.append(img)
    
    # Create Android composite
    print("\nCreating Android composite image...")
    android_width = screenshot_width * 3
    android_composite = Image.new('RGB', (android_width, screenshot_height))
    
    for i, img in enumerate(android_images):
        x = i * screenshot_width
        android_composite.paste(img, (x, 0))
    
    android_path = output_dir / "android.png"
    android_composite.save(android_path)
    print(f"✓ Android composite saved to: {android_path}")
    
    print("\n" + "=" * 60)
    print("iOS SCREENSHOTS")
    print("=" * 60)
    
    # Generate iOS screenshots with iOS styling (all light mode)
    ios_images = []
    ios_uis = [
        ("Settings", create_settings_ui_ios()),
        ("Spreadsheet", create_spreadsheet_ui_ios()),
        ("Minimal", create_minimal_ui_ios()),
    ]
    
    for i, (name, ui_func) in enumerate(ios_uis):
        print(f"\nRendering iOS {i+1}/3: {name} (Light)...")
        
        setup_config = {
            "os_type": "ios_light",
            "width": screenshot_width,
            "height": screenshot_height,
            "background": "#f2f2f7",
        }
        
        screenshot_bytes = render_windows(
            provider="webtop",
            setup_config=setup_config,
            windows=[ui_func],
            screenshot_delay=0.5,
        )
        
        img = Image.open(io.BytesIO(screenshot_bytes))
        ios_images.append(img)
    
    # Create iOS composite
    print("\nCreating iOS composite image...")
    ios_width = screenshot_width * 3
    ios_composite = Image.new('RGB', (ios_width, screenshot_height))
    
    for i, img in enumerate(ios_images):
        x = i * screenshot_width
        ios_composite.paste(img, (x, 0))
    
    ios_path = output_dir / "ios.png"
    ios_composite.save(ios_path)
    print(f"✓ iOS composite saved to: {ios_path}")
    
    print("\n" + "=" * 60)
    print("COMPLETE")
    print("=" * 60)
    print(f"\nGenerated files:")
    print(f"  • {android_path.name} ({android_width}x{screenshot_height})")
    print(f"  • {ios_path.name} ({ios_width}x{screenshot_height})")


if __name__ == "__main__":
    main()
