/**
 * macOS computer interface implementation.
 */

import { BaseComputerInterface } from "./base";
import type { MouseButton, CursorPosition, AccessibilityNode } from "./base";
import type { ScreenSize } from "../types";

export class MacOSComputerInterface extends BaseComputerInterface {
  // Mouse Actions
  async mouseDown(
    x?: number,
    y?: number,
    button: MouseButton = "left"
  ): Promise<void> {
    await this.sendCommand("mouse_down", { x, y, button });
  }

  async mouseUp(
    x?: number,
    y?: number,
    button: MouseButton = "left"
  ): Promise<void> {
    await this.sendCommand("mouse_up", { x, y, button });
  }

  async leftClick(x?: number, y?: number): Promise<void> {
    await this.sendCommand("left_click", { x, y });
  }

  async rightClick(x?: number, y?: number): Promise<void> {
    await this.sendCommand("right_click", { x, y });
  }

  async doubleClick(x?: number, y?: number): Promise<void> {
    await this.sendCommand("double_click", { x, y });
  }

  async moveCursor(x: number, y: number): Promise<void> {
    await this.sendCommand("move_cursor", { x, y });
  }

  async dragTo(
    x: number,
    y: number,
    button: MouseButton = "left",
    duration = 0.5
  ): Promise<void> {
    await this.sendCommand("drag_to", { x, y, button, duration });
  }

  async drag(
    path: Array<[number, number]>,
    button: MouseButton = "left",
    duration = 0.5
  ): Promise<void> {
    await this.sendCommand("drag", { path, button, duration });
  }

  // Keyboard Actions
  async keyDown(key: string): Promise<void> {
    await this.sendCommand("key_down", { key });
  }

  async keyUp(key: string): Promise<void> {
    await this.sendCommand("key_up", { key });
  }

  async typeText(text: string): Promise<void> {
    await this.sendCommand("type_text", { text });
  }

  async pressKey(key: string): Promise<void> {
    await this.sendCommand("press_key", { key });
  }

  async hotkey(...keys: string[]): Promise<void> {
    await this.sendCommand("hotkey", { keys });
  }

  // Scrolling Actions
  async scroll(x: number, y: number): Promise<void> {
    await this.sendCommand("scroll", { x, y });
  }

  async scrollDown(clicks = 1): Promise<void> {
    await this.sendCommand("scroll_down", { clicks });
  }

  async scrollUp(clicks = 1): Promise<void> {
    await this.sendCommand("scroll_up", { clicks });
  }

  // Screen Actions
  async screenshot(): Promise<Buffer> {
    const response = await this.sendCommand("screenshot");
    if (!response.image_data) {
      throw new Error("Failed to take screenshot");
    }
    return Buffer.from(response.image_data as string, "base64");
  }

  async getScreenSize(): Promise<ScreenSize> {
    const response = await this.sendCommand("get_screen_size");
    if (!response.success || !response.size) {
      throw new Error("Failed to get screen size");
    }
    return response.size as ScreenSize;
  }

  async getCursorPosition(): Promise<CursorPosition> {
    const response = await this.sendCommand("get_cursor_position");
    if (!response.success || !response.position) {
      throw new Error("Failed to get cursor position");
    }
    return response.position as CursorPosition;
  }

  // Clipboard Actions
  async copyToClipboard(): Promise<string> {
    const response = await this.sendCommand("copy_to_clipboard");
    if (!response.success || !response.content) {
      throw new Error("Failed to get clipboard content");
    }
    return response.content as string;
  }

  async setClipboard(text: string): Promise<void> {
    await this.sendCommand("set_clipboard", { text });
  }

  // File System Actions
  async fileExists(path: string): Promise<boolean> {
    const response = await this.sendCommand("file_exists", { path });
    return (response.exists as boolean) || false;
  }

  async directoryExists(path: string): Promise<boolean> {
    const response = await this.sendCommand("directory_exists", { path });
    return (response.exists as boolean) || false;
  }

  async listDir(path: string): Promise<string[]> {
    const response = await this.sendCommand("list_dir", { path });
    if (!response.success) {
      throw new Error(response.error as string || "Failed to list directory");
    }
    return (response.files as string[]) || [];
  }

  async readText(path: string): Promise<string> {
    const response = await this.sendCommand("read_text", { path });
    if (!response.success) {
      throw new Error(response.error as string || "Failed to read file");
    }
    return (response.content as string) || "";
  }

  async writeText(path: string, content: string): Promise<void> {
    const response = await this.sendCommand("write_text", { path, content });
    if (!response.success) {
      throw new Error(response.error as string || "Failed to write file");
    }
  }

  async readBytes(path: string): Promise<Buffer> {
    const response = await this.sendCommand("read_bytes", { path });
    if (!response.success) {
      throw new Error(response.error as string || "Failed to read file");
    }
    return Buffer.from(response.content_b64 as string, "base64");
  }

  async writeBytes(path: string, content: Buffer): Promise<void> {
    const response = await this.sendCommand("write_bytes", {
      path,
      content_b64: content.toString("base64"),
    });
    if (!response.success) {
      throw new Error(response.error as string || "Failed to write file");
    }
  }

  async deleteFile(path: string): Promise<void> {
    const response = await this.sendCommand("delete_file", { path });
    if (!response.success) {
      throw new Error(response.error as string || "Failed to delete file");
    }
  }

  async createDir(path: string): Promise<void> {
    const response = await this.sendCommand("create_dir", { path });
    if (!response.success) {
      throw new Error(response.error as string || "Failed to create directory");
    }
  }

  async deleteDir(path: string): Promise<void> {
    const response = await this.sendCommand("delete_dir", { path });
    if (!response.success) {
      throw new Error(response.error as string || "Failed to delete directory");
    }
  }

  async runCommand(command: string): Promise<[string, string]> {
    const response = await this.sendCommand("run_command", { command });
    if (!response.success) {
      throw new Error(response.error as string || "Failed to run command");
    }
    return [(response.stdout as string) || "", (response.stderr as string) || ""];
  }

  // Accessibility Actions
  async getAccessibilityTree(): Promise<AccessibilityNode> {
    const response = await this.sendCommand("get_accessibility_tree");
    if (!response.success) {
      throw new Error(response.error as string || "Failed to get accessibility tree");
    }
    return response as unknown as AccessibilityNode;
  }

  async toScreenCoordinates(x: number, y: number): Promise<[number, number]> {
    const response = await this.sendCommand("to_screen_coordinates", { x, y });
    if (!response.success || !response.coordinates) {
      throw new Error("Failed to convert to screen coordinates");
    }
    return response.coordinates as [number, number];
  }

  async toScreenshotCoordinates(
    x: number,
    y: number
  ): Promise<[number, number]> {
    const response = await this.sendCommand("to_screenshot_coordinates", {
      x,
      y,
    });
    if (!response.success || !response.coordinates) {
      throw new Error("Failed to convert to screenshot coordinates");
    }
    return response.coordinates as [number, number];
  }
}
