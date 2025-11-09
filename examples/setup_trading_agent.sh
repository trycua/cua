#!/bin/bash
# Trading Agent Setup Script
# This script helps you set up the TradingView AI Trading Agent

set -e  # Exit on error

echo "=========================================="
echo "TradingView AI Trading Agent Setup"
echo "=========================================="
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check Python version
echo "Checking Python version..."
python_version=$(python --version 2>&1 | awk '{print $2}')
echo "Found Python $python_version"

if ! python -c 'import sys; sys.exit(0 if sys.version_info >= (3, 10) else 1)' 2>/dev/null; then
    echo -e "${RED}❌ Python 3.10 or higher required${NC}"
    exit 1
fi
echo -e "${GREEN}✓ Python version OK${NC}"
echo ""

# Check if in cua directory
if [ ! -f "../pyproject.toml" ]; then
    echo -e "${YELLOW}⚠️  Please run this script from the cua/examples directory${NC}"
    exit 1
fi

# Install core Cua packages
echo "Installing Cua framework..."
cd ..
pip install -q -e ".[all]" || {
    echo -e "${RED}❌ Failed to install Cua${NC}"
    exit 1
}
cd examples
echo -e "${GREEN}✓ Cua installed${NC}"
echo ""

# Install trading-specific requirements
echo "Installing trading dependencies..."

# Check if requirements file exists
if [ ! -f "requirements-trading.txt" ]; then
    echo -e "${RED}❌ requirements-trading.txt not found${NC}"
    exit 1
fi

# Install most packages
pip install -q python-dotenv pandas numpy yfinance pandas-ta aiohttp websockets requests || {
    echo -e "${YELLOW}⚠️  Some packages failed to install (non-critical)${NC}"
}

# Install Alpaca API
pip install -q alpaca-trade-api || {
    echo -e "${YELLOW}⚠️  alpaca-trade-api installation failed${NC}"
    echo "You can install it manually: pip install alpaca-trade-api"
}

# TA-Lib requires system dependencies
echo ""
echo "Checking for TA-Lib..."
if ! python -c "import talib" 2>/dev/null; then
    echo -e "${YELLOW}⚠️  TA-Lib not installed${NC}"
    echo "TA-Lib requires system libraries. To install:"
    echo ""
    echo "macOS:"
    echo "  brew install ta-lib"
    echo "  pip install ta-lib"
    echo ""
    echo "Linux:"
    echo "  wget http://prdownloads.sourceforge.net/ta-lib/ta-lib-0.4.0-src.tar.gz"
    echo "  tar -xzf ta-lib-0.4.0-src.tar.gz"
    echo "  cd ta-lib/ && ./configure --prefix=/usr && make && sudo make install"
    echo "  pip install ta-lib"
    echo ""
    echo "For now, using pandas-ta as alternative..."
    pip install -q pandas-ta
else
    echo -e "${GREEN}✓ TA-Lib available${NC}"
fi

echo ""
echo -e "${GREEN}✓ Trading dependencies installed${NC}"
echo ""

# Set up environment file
echo "Setting up environment configuration..."
if [ -f ".env" ]; then
    echo -e "${YELLOW}⚠️  .env file already exists${NC}"
    read -p "Overwrite? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "Keeping existing .env file"
    else
        cp .env.example .env
        echo -e "${GREEN}✓ .env file created${NC}"
    fi
else
    cp .env.example .env
    echo -e "${GREEN}✓ .env file created from template${NC}"
fi

echo ""
echo "=========================================="
echo "Next Steps"
echo "=========================================="
echo ""
echo "1. Get your API keys:"
echo ""
echo "   Anthropic (Claude AI):"
echo "   → https://console.anthropic.com"
echo "   → Get API key and add to .env as ANTHROPIC_API_KEY"
echo ""
echo "   Alpaca (Broker - Free Paper Trading):"
echo "   → https://alpaca.markets"
echo "   → Create account and get paper trading keys"
echo "   → Add to .env as ALPACA_API_KEY and ALPACA_SECRET_KEY"
echo ""
echo "2. Edit your .env file:"
echo "   nano .env"
echo ""
echo "3. Test the setup:"
echo "   python alpaca_broker.py        # Test broker connection"
echo "   python trading_tools.py        # Test trading tools"
echo "   python quick_start_demo.py     # Run quick demo"
echo ""
echo "4. Read the documentation:"
echo "   cat ../TRADINGVIEW_SETUP.md    # Complete setup guide"
echo ""
echo "5. Run the full trading agent:"
echo "   python tradingview_agent_complete.py --mode paper"
echo ""
echo "=========================================="
echo -e "${GREEN}Setup complete!${NC}"
echo "=========================================="
echo ""
echo -e "${RED}⚠️  IMPORTANT: Start with paper trading!${NC}"
echo "Test for at least 2 weeks before considering live trading."
echo ""
