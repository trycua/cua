/**
 * macOS computer interface implementation.
 */

import type { ScreenSize } from '../types';
import type { AccessibilityNode, CursorPosition, MouseButton } from './base';
import { BaseComputerInterface } from './base';

export class MacOSComputerInterface extends BaseComputerInterface {
  // Mouse Actions
  /**
   * Press and hold a mouse button at the specified coordinates.
   * @param {number} [x] - X coordinate for the mouse action
   * @param {number} [y] - Y coordinate for the mouse action
   * @param {MouseButton} [button='left'] - Mouse button to press down
   * @returns {Promise<void>}
   */
  async mouseDown(x?: number, y?: number, button: MouseButton = 'left'): Promise<void> {
    await this.sendCommand('mouse_down', { x, y, button });
  }

  /**
   * Release a mouse button at the specified coordinates.
   * @param {number} [x] - X coordinate for the mouse action
   * @param {number} [y] - Y coordinate for the mouse action
   * @param {MouseButton} [button='left'] - Mouse button to release
   * @returns {Promise<void>}
   */
  async mouseUp(x?: number, y?: number, button: MouseButton = 'left'): Promise<void> {
    await this.sendCommand('mouse_up', { x, y, button });
  }

  /**
   * Perform a left mouse click at the specified coordinates.
   * @param {number} [x] - X coordinate for the click
   * @param {number} [y] - Y coordinate for the click
   * @returns {Promise<void>}
   */
  async leftClick(x?: number, y?: number): Promise<void> {
    await this.sendCommand('left_click', { x, y });
  }

  /**
   * Perform a right mouse click at the specified coordinates.
   * @param {number} [x] - X coordinate for the click
   * @param {number} [y] - Y coordinate for the click
   * @returns {Promise<void>}
   */
  async rightClick(x?: number, y?: number): Promise<void> {
    await this.sendCommand('right_click', { x, y });
  }

  /**
   * Perform a double click at the specified coordinates.
   * @param {number} [x] - X coordinate for the double click
   * @param {number} [y] - Y coordinate for the double click
   * @returns {Promise<void>}
   */
  async doubleClick(x?: number, y?: number): Promise<void> {
    await this.sendCommand('double_click', { x, y });
  }

  /**
   * Move the cursor to the specified coordinates.
   * @param {number} x - X coordinate to move to
   * @param {number} y - Y coordinate to move to
   * @returns {Promise<void>}
   */
  async moveCursor(x: number, y: number): Promise<void> {
    await this.sendCommand('move_cursor', { x, y });
  }

  /**
   * Drag from current position to the specified coordinates.
   * @param {number} x - X coordinate to drag to
   * @param {number} y - Y coordinate to drag to
   * @param {MouseButton} [button='left'] - Mouse button to use for dragging
   * @param {number} [duration=0.5] - Duration of the drag operation in seconds
   * @returns {Promise<void>}
   */
  async dragTo(x: number, y: number, button: MouseButton = 'left', duration = 0.5): Promise<void> {
    await this.sendCommand('drag_to', { x, y, button, duration });
  }

  /**
   * Drag along a path of coordinates.
   * @param {Array<[number, number]>} path - Array of [x, y] coordinate pairs to drag through
   * @param {MouseButton} [button='left'] - Mouse button to use for dragging
   * @param {number} [duration=0.5] - Duration of the drag operation in seconds
   * @returns {Promise<void>}
   */
  async drag(
    path: Array<[number, number]>,
    button: MouseButton = 'left',
    duration = 0.5
  ): Promise<void> {
    await this.sendCommand('drag', { path, button, duration });
  }

  // Keyboard Actions
  /**
   * Press and hold a key.
   * @param {string} key - Key to press down
   * @returns {Promise<void>}
   */
  async keyDown(key: string): Promise<void> {
    await this.sendCommand('key_down', { key });
  }

  /**
   * Release a key.
   * @param {string} key - Key to release
   * @returns {Promise<void>}
   */
  async keyUp(key: string): Promise<void> {
    await this.sendCommand('key_up', { key });
  }

  /**
   * Type text as if entered from keyboard.
   * @param {string} text - Text to type
   * @returns {Promise<void>}
   */
  async typeText(text: string): Promise<void> {
    await this.sendCommand('type_text', { text });
  }

  /**
   * Press and release a key.
   * @param {string} key - Key to press
   * @returns {Promise<void>}
   */
  async pressKey(key: string): Promise<void> {
    await this.sendCommand('press_key', { key });
  }

  /**
   * Press multiple keys simultaneously as a hotkey combination.
   * @param {...string} keys - Keys to press together
   * @returns {Promise<void>}
   */
  async hotkey(...keys: string[]): Promise<void> {
    await this.sendCommand('hotkey', { keys });
  }

  // Scrolling Actions
  /**
   * Scroll by the specified amount in x and y directions.
   * @param {number} x - Horizontal scroll amount
   * @param {number} y - Vertical scroll amount
   * @returns {Promise<void>}
   */
  async scroll(x: number, y: number): Promise<void> {
    await this.sendCommand('scroll', { x, y });
  }

  /**
   * Scroll down by the specified number of clicks.
   * @param {number} [clicks=1] - Number of scroll clicks
   * @returns {Promise<void>}
   */
  async scrollDown(clicks = 1): Promise<void> {
    await this.sendCommand('scroll_down', { clicks });
  }

  /**
   * Scroll up by the specified number of clicks.
   * @param {number} [clicks=1] - Number of scroll clicks
   * @returns {Promise<void>}
   */
  async scrollUp(clicks = 1): Promise<void> {
    await this.sendCommand('scroll_up', { clicks });
  }

  // Screen Actions
  /**
   * Take a screenshot of the screen.
   * @returns {Promise<Buffer>} Screenshot image data as a Buffer
   * @throws {Error} If screenshot fails
   */
  async screenshot(): Promise<Buffer> {
    const response = await this.sendCommand('screenshot');
    if (!response.image_data) {
      throw new Error('Failed to take screenshot');
    }
    return Buffer.from(response.image_data as string, 'base64');
  }

  /**
   * Get the current screen size.
   * @returns {Promise<ScreenSize>} Screen dimensions
   * @throws {Error} If unable to get screen size
   */
  async getScreenSize(): Promise<ScreenSize> {
    const response = await this.sendCommand('get_screen_size');
    if (!response.success || !response.size) {
      throw new Error('Failed to get screen size');
    }
    return response.size as ScreenSize;
  }

  /**
   * Get the current cursor position.
   * @returns {Promise<CursorPosition>} Current cursor coordinates
   * @throws {Error} If unable to get cursor position
   */
  async getCursorPosition(): Promise<CursorPosition> {
    const response = await this.sendCommand('get_cursor_position');
    if (!response.success || !response.position) {
      throw new Error('Failed to get cursor position');
    }
    return response.position as CursorPosition;
  }

  // Desktop Actions
  /**
   * Get the current desktop environment string (e.g., 'xfce4', 'gnome', 'kde', 'mac', 'windows').
   */
  async getDesktopEnvironment(): Promise<string> {
    const response = await this.sendCommand('get_desktop_environment');
    if (!response.success) {
      throw new Error((response.error as string) || 'Failed to get desktop environment');
    }
    return (response.environment as string) || 'unknown';
  }

  /**
   * Set the desktop wallpaper image.
   * @param path Absolute path to the image file on the VM
   */
  async setWallpaper(path: string): Promise<void> {
    const response = await this.sendCommand('set_wallpaper', { path });
    if (!response.success) {
      throw new Error((response.error as string) || 'Failed to set wallpaper');
    }
  }

  // Clipboard Actions
  /**
   * Copy current selection to clipboard and return the content.
   * @returns {Promise<string>} Clipboard content
   * @throws {Error} If unable to get clipboard content
   */
  async copyToClipboard(): Promise<string> {
    const response = await this.sendCommand('copy_to_clipboard');
    if (!response.success || !response.content) {
      throw new Error('Failed to get clipboard content');
    }
    return response.content as string;
  }

  /**
   * Set the clipboard content to the specified text.
   * @param {string} text - Text to set in clipboard
   * @returns {Promise<void>}
   */
  async setClipboard(text: string): Promise<void> {
    await this.sendCommand('set_clipboard', { text });
  }

  // File System Actions
  /**
   * Check if a file exists at the specified path.
   * @param {string} path - Path to the file
   * @returns {Promise<boolean>} True if file exists, false otherwise
   */
  async fileExists(path: string): Promise<boolean> {
    const response = await this.sendCommand('file_exists', { path });
    return (response.exists as boolean) || false;
  }

  /**
   * Check if a directory exists at the specified path.
   * @param {string} path - Path to the directory
   * @returns {Promise<boolean>} True if directory exists, false otherwise
   */
  async directoryExists(path: string): Promise<boolean> {
    const response = await this.sendCommand('directory_exists', { path });
    return (response.exists as boolean) || false;
  }

  /**
   * List the contents of a directory.
   * @param {string} path - Path to the directory
   * @returns {Promise<string[]>} Array of file and directory names
   * @throws {Error} If unable to list directory
   */
  async listDir(path: string): Promise<string[]> {
    const response = await this.sendCommand('list_dir', { path });
    if (!response.success) {
      throw new Error((response.error as string) || 'Failed to list directory');
    }
    return (response.files as string[]) || [];
  }

  /**
   * Get the size of a file in bytes.
   * @param {string} path - Path to the file
   * @returns {Promise<number>} File size in bytes
   * @throws {Error} If unable to get file size
   */
  async getFileSize(path: string): Promise<number> {
    const response = await this.sendCommand('get_file_size', { path });
    if (!response.success) {
      throw new Error((response.error as string) || 'Failed to get file size');
    }
    return (response.size as number) || 0;
  }

  /**
   * Read file content in chunks for large files.
   * @private
   * @param {string} path - Path to the file
   * @param {number} offset - Starting byte offset
   * @param {number} totalLength - Total number of bytes to read
   * @param {number} [chunkSize=1048576] - Size of each chunk in bytes
   * @returns {Promise<Buffer>} File content as Buffer
   * @throws {Error} If unable to read file chunk
   */
  private async readBytesChunked(
    path: string,
    offset: number,
    totalLength: number,
    chunkSize: number = 1024 * 1024
  ): Promise<Buffer> {
    const chunks: Buffer[] = [];
    let currentOffset = offset;
    let remaining = totalLength;

    while (remaining > 0) {
      const readSize = Math.min(chunkSize, remaining);
      const response = await this.sendCommand('read_bytes', {
        path,
        offset: currentOffset,
        length: readSize,
      });

      if (!response.success) {
        throw new Error((response.error as string) || 'Failed to read file chunk');
      }

      const chunkData = Buffer.from(response.content_b64 as string, 'base64');
      chunks.push(chunkData);

      currentOffset += readSize;
      remaining -= readSize;
    }

    return Buffer.concat(chunks);
  }

  /**
   * Write file content in chunks for large files.
   * @private
   * @param {string} path - Path to the file
   * @param {Buffer} content - Content to write
   * @param {boolean} [append=false] - Whether to append to existing file
   * @param {number} [chunkSize=1048576] - Size of each chunk in bytes
   * @returns {Promise<void>}
   * @throws {Error} If unable to write file chunk
   */
  private async writeBytesChunked(
    path: string,
    content: Buffer,
    append: boolean = false,
    chunkSize: number = 1024 * 1024
  ): Promise<void> {
    const totalSize = content.length;
    let currentOffset = 0;

    while (currentOffset < totalSize) {
      const chunkEnd = Math.min(currentOffset + chunkSize, totalSize);
      const chunkData = content.subarray(currentOffset, chunkEnd);

      // First chunk uses the original append flag, subsequent chunks always append
      const chunkAppend = currentOffset === 0 ? append : true;

      const response = await this.sendCommand('write_bytes', {
        path,
        content_b64: chunkData.toString('base64'),
        append: chunkAppend,
      });

      if (!response.success) {
        throw new Error((response.error as string) || 'Failed to write file chunk');
      }

      currentOffset = chunkEnd;
    }
  }

  /**
   * Read text from a file with specified encoding.
   * @param {string} path - Path to the file to read
   * @param {BufferEncoding} [encoding='utf8'] - Text encoding to use
   * @returns {Promise<string>} The decoded text content of the file
   */
  async readText(path: string, encoding: BufferEncoding = 'utf8'): Promise<string> {
    const contentBytes = await this.readBytes(path);
    return contentBytes.toString(encoding);
  }

  /**
   * Write text to a file with specified encoding.
   * @param {string} path - Path to the file to write
   * @param {string} content - Text content to write
   * @param {BufferEncoding} [encoding='utf8'] - Text encoding to use
   * @param {boolean} [append=false] - Whether to append to the file instead of overwriting
   * @returns {Promise<void>}
   */
  async writeText(
    path: string,
    content: string,
    encoding: BufferEncoding = 'utf8',
    append: boolean = false
  ): Promise<void> {
    const contentBytes = Buffer.from(content, encoding);
    await this.writeBytes(path, contentBytes, append);
  }

  /**
   * Read bytes from a file, with optional offset and length.
   * @param {string} path - Path to the file
   * @param {number} [offset=0] - Starting byte offset
   * @param {number} [length] - Number of bytes to read (reads entire file if not specified)
   * @returns {Promise<Buffer>} File content as Buffer
   * @throws {Error} If unable to read file
   */
  async readBytes(path: string, offset: number = 0, length?: number): Promise<Buffer> {
    // For large files, use chunked reading
    if (length === undefined) {
      // Get file size first to determine if we need chunking
      const fileSize = await this.getFileSize(path);
      // If file is larger than 5MB, read in chunks
      if (fileSize > 5 * 1024 * 1024) {
        const readLength = offset > 0 ? fileSize - offset : fileSize;
        return await this.readBytesChunked(path, offset, readLength);
      }
    }

    const response = await this.sendCommand('read_bytes', {
      path,
      offset,
      length,
    });
    if (!response.success) {
      throw new Error((response.error as string) || 'Failed to read file');
    }
    return Buffer.from(response.content_b64 as string, 'base64');
  }

  /**
   * Write bytes to a file.
   * @param {string} path - Path to the file
   * @param {Buffer} content - Content to write as Buffer
   * @param {boolean} [append=false] - Whether to append to existing file
   * @returns {Promise<void>}
   * @throws {Error} If unable to write file
   */
  async writeBytes(path: string, content: Buffer, append: boolean = false): Promise<void> {
    // For large files, use chunked writing
    if (content.length > 5 * 1024 * 1024) {
      // 5MB threshold
      await this.writeBytesChunked(path, content, append);
      return;
    }

    const response = await this.sendCommand('write_bytes', {
      path,
      content_b64: content.toString('base64'),
      append,
    });
    if (!response.success) {
      throw new Error((response.error as string) || 'Failed to write file');
    }
  }

  /**
   * Delete a file at the specified path.
   * @param {string} path - Path to the file to delete
   * @returns {Promise<void>}
   * @throws {Error} If unable to delete file
   */
  async deleteFile(path: string): Promise<void> {
    const response = await this.sendCommand('delete_file', { path });
    if (!response.success) {
      throw new Error((response.error as string) || 'Failed to delete file');
    }
  }

  /**
   * Create a directory at the specified path.
   * @param {string} path - Path where to create the directory
   * @returns {Promise<void>}
   * @throws {Error} If unable to create directory
   */
  async createDir(path: string): Promise<void> {
    const response = await this.sendCommand('create_dir', { path });
    if (!response.success) {
      throw new Error((response.error as string) || 'Failed to create directory');
    }
  }

  /**
   * Delete a directory at the specified path.
   * @param {string} path - Path to the directory to delete
   * @returns {Promise<void>}
   * @throws {Error} If unable to delete directory
   */
  async deleteDir(path: string): Promise<void> {
    const response = await this.sendCommand('delete_dir', { path });
    if (!response.success) {
      throw new Error((response.error as string) || 'Failed to delete directory');
    }
  }

  /**
   * Execute a shell command and return stdout and stderr.
   * @param {string} command - Command to execute
   * @returns {Promise<[string, string]>} Tuple of [stdout, stderr]
   * @throws {Error} If command execution fails
   */
  async runCommand(command: string): Promise<[string, string]> {
    const response = await this.sendCommand('run_command', { command });
    if (!response.success) {
      throw new Error((response.error as string) || 'Failed to run command');
    }
    return [(response.stdout as string) || '', (response.stderr as string) || ''];
  }

  // Accessibility Actions
  /**
   * Get the accessibility tree of the current screen.
   * @returns {Promise<AccessibilityNode>} Root accessibility node
   * @throws {Error} If unable to get accessibility tree
   */
  async getAccessibilityTree(): Promise<AccessibilityNode> {
    const response = await this.sendCommand('get_accessibility_tree');
    if (!response.success) {
      throw new Error((response.error as string) || 'Failed to get accessibility tree');
    }
    return response as unknown as AccessibilityNode;
  }

  /**
   * Convert coordinates to screen coordinates.
   * @param {number} x - X coordinate to convert
   * @param {number} y - Y coordinate to convert
   * @returns {Promise<[number, number]>} Converted screen coordinates as [x, y]
   * @throws {Error} If coordinate conversion fails
   */
  async toScreenCoordinates(x: number, y: number): Promise<[number, number]> {
    const response = await this.sendCommand('to_screen_coordinates', { x, y });
    if (!response.success || !response.coordinates) {
      throw new Error('Failed to convert to screen coordinates');
    }
    return response.coordinates as [number, number];
  }

  /**
   * Convert coordinates to screenshot coordinates.
   * @param {number} x - X coordinate to convert
   * @param {number} y - Y coordinate to convert
   * @returns {Promise<[number, number]>} Converted screenshot coordinates as [x, y]
   * @throws {Error} If coordinate conversion fails
   */
  async toScreenshotCoordinates(x: number, y: number): Promise<[number, number]> {
    const response = await this.sendCommand('to_screenshot_coordinates', {
      x,
      y,
    });
    if (!response.success || !response.coordinates) {
      throw new Error('Failed to convert to screenshot coordinates');
    }
    return response.coordinates as [number, number];
  }
}
