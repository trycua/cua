#!/usr/bin/env python3
"""
Interactive Click Prediction Tool

Takes screenshots and allows testing multiple models interactively.
Models are loaded/unloaded one at a time to avoid memory issues.
"""

import asyncio
import os
from datetime import datetime
from typing import Any, Dict, List

from utils import (
    ModelWrapper,
    get_available_models,
    save_prediction_visualization,
    take_screenshot,
)


async def predict_with_all_models(image, instruction: str, models) -> List[Dict[str, Any]]:
    """
    Predict click coordinates with all models sequentially.

    Args:
        image: PIL Image to analyze
        instruction: Instruction text
        models: List of model instances

    Returns:
        List of prediction results
    """
    predictions = []

    for model in models:
        model_wrapper = ModelWrapper(model)
        print(f"\n🔄 Loading {model_wrapper.model_name}...")

        try:
            # Load model
            await model_wrapper.load_model()

            # Predict
            coords = await model_wrapper.predict_click(image, instruction)

            predictions.append(
                {"model_name": model_wrapper.model_name, "coords": coords, "error": None}
            )

            if coords:
                print(f"✅ {model_wrapper.model_name}: ({coords[0]}, {coords[1]})")
            else:
                print(f"❌ {model_wrapper.model_name}: No prediction")

        except Exception as e:
            print(f"❌ {model_wrapper.model_name}: ERROR - {str(e)}")
            predictions.append(
                {"model_name": model_wrapper.model_name, "coords": None, "error": str(e)}
            )

        finally:
            # Always unload model to free memory
            try:
                await model_wrapper.unload_model()
                print(f"🗑️  Unloaded {model_wrapper.model_name}")
            except Exception as e:
                print(f"⚠️  Error unloading {model_wrapper.model_name}: {e}")

    return predictions


def print_header():
    """Print the interactive tool header."""
    print("=" * 60)
    print("🖱️  Interactive Click Prediction Tool")
    print("=" * 60)
    print("Commands:")
    print("  • Type an instruction to test models on last screenshot")
    print("  • 'screenshot' - Take a new screenshot")
    print("  • 'models' - List available models")
    print("  • 'quit' or 'exit' - Exit the tool")
    print("=" * 60)
    print("💡 Tip: Take a screenshot first, then send instructions to test models!")


def print_models(models):
    """Print available models."""
    print("\n📋 Available Models:")
    for i, model in enumerate(models, 1):
        if isinstance(model, str):
            print(f"  {i}. {model}")
        else:
            print(f"  {i}. models.{model.__class__.__name__}")


async def main():
    """
    Main interactive loop.
    """
    print_header()

    # Get available models
    models = get_available_models()
    print_models(models)

    # Create output directory for visualizations
    output_dir = "interactive_output"
    os.makedirs(output_dir, exist_ok=True)

    session_count = 0
    last_screenshot = None
    screenshot_timestamp = None

    while True:
        try:
            # Get user input
            print(f"\n{'='*40}")
            user_input = input("🎯 Enter instruction (or command): ").strip()

            if not user_input:
                continue

            # Handle commands
            if user_input.lower() in ["quit", "exit", "q"]:
                print("👋 Goodbye!")
                break

            elif user_input.lower() == "models":
                print_models(models)
                continue

            elif user_input.lower() == "screenshot":
                print("📸 Taking screenshot...")
                try:
                    last_screenshot = take_screenshot()
                    screenshot_timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
                    screenshot_path = os.path.join(
                        output_dir, f"screenshot_{screenshot_timestamp}.png"
                    )
                    last_screenshot.save(screenshot_path)
                    print(f"✅ Screenshot captured and saved to: {screenshot_path}")
                    print(f"📝 Ready for instructions! Screenshot size: {last_screenshot.size}")
                except Exception as e:
                    print(f"❌ Error taking screenshot: {e}")
                continue

            # Handle instruction input
            if last_screenshot is None:
                print(
                    "⚠️  No screenshot available! Please take a screenshot first using 'screenshot' command."
                )
                continue

            session_count += 1
            print(f"\n🎯 Session {session_count}: '{user_input}'")
            print(f"📷 Using screenshot from: {screenshot_timestamp}")

            # Predict with all models using last screenshot
            print(f"\n🤖 Testing {len(models)} models on screenshot...")
            predictions = await predict_with_all_models(last_screenshot, user_input, models)

            # Display results summary
            print("\n📊 Results Summary:")
            print("-" * 50)
            for pred in predictions:
                if pred["coords"]:
                    print(f"✅ {pred['model_name']}: ({pred['coords'][0]}, {pred['coords'][1]})")
                elif pred["error"]:
                    print(f"❌ {pred['model_name']}: ERROR - {pred['error']}")
                else:
                    print(f"❌ {pred['model_name']}: No prediction")

            # Save visualization
            timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
            vis_filename = f"session_{session_count:03d}_{timestamp}.png"
            vis_path = os.path.join(output_dir, vis_filename)

            try:
                save_prediction_visualization(last_screenshot, user_input, predictions, vis_path)
                print(f"\n💾 Visualization saved to: {vis_path}")
            except Exception as e:
                print(f"⚠️  Error saving visualization: {e}")

            print(f"\n✨ Session {session_count} completed!")

        except KeyboardInterrupt:
            print("\n\n👋 Interrupted by user. Goodbye!")
            break
        except Exception as e:
            print(f"\n❌ Unexpected error: {e}")
            print("Continuing...")


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\n👋 Goodbye!")
    except Exception as e:
        print(f"❌ Fatal error: {e}")
