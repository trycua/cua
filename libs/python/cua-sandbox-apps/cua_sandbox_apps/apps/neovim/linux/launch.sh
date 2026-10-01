#!/bin/bash

# Launch Neovim in a terminal with a sample file
# Neovim is a terminal-based editor, so we need to open it in a terminal

# Create a sample file to edit
mkdir -p /tmp/neovim_demo
cat > /tmp/neovim_demo/sample.txt << 'EOF'
Welcome to Neovim!
==================

This is a hyperextensible Vim-based text editor with:
- Modern enhancements
- Built-in LSP client
- Tree-sitter parsing
- Lua support

You can navigate with arrow keys or Vim keys (hjkl).
Press :q to quit without saving, or :wq to save and quit.
EOF

# Launch Neovim in the terminal with the sample file
exec nvim /tmp/neovim_demo/sample.txt
