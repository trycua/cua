#!/bin/bash

# Quick setup script for Claude Computer Use Evaluation System
# This script automates the installation and setup process

set -e  # Exit on any error

echo "🚀 Setting up Claude Computer Use Evaluation System..."

# Check Python version
python_version=$(python3 --version 2>&1 | grep -o 'Python [0-9]\+\.[0-9]\+' | grep -o '[0-9]\+\.[0-9]\+')
echo "📋 Detected Python version: $python_version"

if ! python3 -c "import sys; exit(0 if sys.version_info >= (3, 8) else 1)" 2>/dev/null; then
    echo "❌ Python 3.8+ is required. Current version: $python_version"
    echo "Please install Python 3.8 or later and try again."
    exit 1
fi

# Create virtual environment
echo "🏗️  Creating virtual environment..."
python3 -m venv venv

# Activate virtual environment
echo "⚡ Activating virtual environment..."
source venv/bin/activate

# Upgrade pip
echo "⬆️  Upgrading pip..."
pip install --upgrade pip

# Install dependencies
echo "📦 Installing dependencies..."
pip install -r requirements.txt

# Create required directories
echo "📁 Creating required directories..."
mkdir -p logs screenshots evaluations

# Check for .env file
if [ ! -f .env ]; then
    echo "⚠️  Creating .env file template..."
    cat > .env << EOF
# Anthropic API Key - Replace with your actual key
ANTHROPIC_API_KEY=your_api_key_here

# Optional: Set log level
LOG_LEVEL=INFO
EOF
    echo "📝 Please edit .env file and add your Anthropic API key"
else
    echo "✅ .env file already exists"
fi

# Check for API key
if grep -q "your_api_key_here" .env 2>/dev/null; then
    echo "⚠️  Warning: Please update your ANTHROPIC_API_KEY in the .env file"
fi

echo ""
echo "✅ Setup complete!"
echo ""
echo "📋 Quick Start Guide:"
echo "1. Edit .env file and add your Anthropic API key"
echo "2. Activate the virtual environment: source venv/bin/activate"
echo "3. Run the main app: streamlit run streamlit_app.py"
echo "4. Run the evaluation system: streamlit run run_eval.py"
echo ""
echo "📚 See README.md for detailed usage instructions"
echo "📊 See EVALUATION_SYSTEM_DOCUMENTATION.md for evaluation system details"
echo ""
echo "🔐 Remember to grant accessibility permissions when prompted!"
