# TradingView AI Trading Agent - Complete Setup Guide

A professional AI-powered trading agent that uses TradingView for chart analysis and Claude Sonnet 4.5 vision for pattern recognition.

## Overview

This trading agent system provides:

- **AI Vision Analysis**: Claude Sonnet 4.5 analyzes TradingView charts
- **Automated Chart Reading**: Pattern recognition, support/resistance identification
- **Multi-Strategy Support**: Trend following, breakout, mean reversion strategies
- **Risk Management**: Position sizing, stop-loss enforcement, portfolio limits
- **Safety Features**: Kill switches, compliance checks, audit trails
- **Paper Trading**: Safe testing environment before live trading

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  Claude Sonnet 4.5 (Vision Model)                       │
│  - Chart pattern recognition                            │
│  - Support/resistance identification                    │
│  - Trend analysis                                       │
└────────────────┬────────────────────────────────────────┘
                 │
┌────────────────▼────────────────────────────────────────┐
│  Cua Computer SDK                                       │
│  - Browser automation (TradingView)                     │
│  - Screenshot capture                                   │
│  - Chart interaction                                    │
└────────────────┬────────────────────────────────────────┘
                 │
┌────────────────▼────────────────────────────────────────┐
│  TradingView Platform                                   │
│  - Advanced charting                                    │
│  - Technical indicators                                 │
│  - Multi-timeframe analysis                             │
└─────────────────────────────────────────────────────────┘
                 │
┌────────────────▼────────────────────────────────────────┐
│  Broker API (Execution)                                 │
│  - Alpaca / Interactive Brokers / TD Ameritrade         │
│  - Paper trading mode                                   │
│  - Real-time order execution                            │
└─────────────────────────────────────────────────────────┘
```

## Prerequisites

### 1. System Requirements

- **OS**: macOS, Linux, or Windows
- **Python**: 3.10 or higher
- **RAM**: 8GB minimum (16GB recommended)
- **Storage**: 2GB free space

### 2. Required Accounts

#### TradingView Account
- Free or paid TradingView account
- URL: https://www.tradingview.com
- Recommended: Pro+ for advanced features

#### Anthropic API Key
- Claude Sonnet 4.5 access
- Get key at: https://console.anthropic.com
- Estimated cost: $10-50/month depending on usage

#### Broker Account (choose one)

**Option A: Alpaca (Recommended for Beginners)**
- Free paper trading account
- Easy API integration
- Commission-free trading
- URL: https://alpaca.markets
- Free tier available

**Option B: Interactive Brokers (Professional)**
- $10,000 minimum for margin
- Lower fees for high volume
- More complex API
- URL: https://www.interactivebrokers.com

**Option C: TD Ameritrade**
- Good for options trading
- ThinkorSwim platform
- URL: https://www.tdameritrade.com

## Installation

### Step 1: Install Cua

```bash
# Clone the repository
git clone https://github.com/trycua/cua.git
cd cua

# Install dependencies
pip install -e ".[all]"

# Install trading-specific requirements
pip install pandas ta-lib alpaca-trade-api
```

### Step 2: Set Up Environment Variables

Create a `.env` file in the `cua/examples/` directory:

```bash
# .env file
# Anthropic API
ANTHROPIC_API_KEY=your_anthropic_key_here

# Alpaca API (for execution)
ALPACA_API_KEY=your_alpaca_key_here
ALPACA_SECRET_KEY=your_alpaca_secret_here
ALPACA_BASE_URL=https://paper-api.alpaca.markets  # Paper trading

# Optional: Other brokers
# IBKR_API_KEY=...
# TD_API_KEY=...

# Trading Configuration
PORTFOLIO_VALUE=100000
MAX_DAILY_LOSS=0.02  # 2%
MAX_POSITION_SIZE=0.10  # 10%
```

### Step 3: Verify Installation

```bash
cd examples

# Test the trading tools
python trading_tools.py

# Test TradingView integration (without browser)
python tradingview_integration.py

# Test safety callbacks
python trading_callbacks.py
```

## Configuration

### Risk Management Settings

Edit the risk parameters in your script or `.env`:

```python
RISK_LIMITS = {
    "max_position_size": 0.10,      # 10% max per position
    "max_daily_loss": 0.02,          # 2% daily loss limit
    "max_drawdown": 0.10,            # 10% max drawdown
    "max_positions": 10,             # Max concurrent positions
    "require_stop_loss": True,       # Force stop-loss on all trades
    "max_portfolio_leverage": 1.0,   # No leverage
}
```

### Trading Strategy Selection

Choose from built-in strategies or create custom ones:

**Trend Following** (Default)
- Follows EMA crossovers and momentum
- Best for: Strong trending markets
- Win rate: ~45-55%
- Risk/Reward: 1:2

**Breakout**
- Trades consolidation breakouts
- Best for: Volatile markets
- Win rate: ~40-50%
- Risk/Reward: 1:3

**Custom Strategy**
- Create your own by extending `TradingStrategy` class
- See `tradingview_agent_complete.py` for examples

## Usage

### Paper Trading (Recommended First)

Start with paper trading to test the system:

```bash
# Basic paper trading
python tradingview_agent_complete.py --mode paper

# With specific symbols
python tradingview_agent_complete.py \
    --mode paper \
    --symbols "AAPL,MSFT,NVDA,GOOGL,TSLA"

# With custom strategy
python tradingview_agent_complete.py \
    --mode paper \
    --strategy breakout \
    --scan-interval 180  # Scan every 3 minutes
```

### Running the Agent

The agent will:

1. **Initialize**
   - Open TradingView in browser
   - Connect to broker API
   - Load AI vision model

2. **Scan Watchlist**
   - Load each symbol in TradingView
   - Capture chart screenshot
   - AI analyzes patterns and indicators

3. **Generate Signals**
   - Strategy evaluates opportunities
   - Risk management validates trades
   - Compliance checks pass

4. **Execute Trades**
   - Submit orders via broker API
   - Monitor position
   - Log all decisions

5. **Monitor Performance**
   - Track P&L
   - Check risk limits
   - Generate reports

### Live Trading (Use with Extreme Caution)

**⚠️  WARNING**: Only proceed if you:
- Tested extensively in paper trading
- Understand all risks
- Can afford to lose the capital
- Have reviewed all code

```bash
python tradingview_agent_complete.py \
    --mode live \
    --confirm-risk \
    --portfolio-value 10000 \
    --max-budget 50.0
```

## TradingView Setup

### Chart Configuration

For optimal AI analysis, configure TradingView charts:

1. **Indicators to Add**:
   - RSI (14)
   - MACD (12, 26, 9)
   - EMA 20 (orange)
   - EMA 50 (blue)
   - Volume

2. **Chart Settings**:
   - Style: Candlestick
   - Theme: Dark (better for vision)
   - Remove unnecessary clutter
   - Full screen for analysis

3. **Timeframes to Monitor**:
   - Daily (1D) - primary
   - 4-hour (4H) - confirmation
   - 1-hour (1H) - entry timing

### Browser Automation Setup

The agent controls TradingView via browser automation:

**macOS**:
```bash
# Safari (default)
# No additional setup needed

# Chrome (alternative)
brew install --cask google-chrome
```

**Linux**:
```bash
# Chrome/Chromium
sudo apt install chromium-browser
```

**Windows**:
```bash
# Chrome
# Download from https://www.google.com/chrome/
```

## Monitoring & Alerts

### Real-time Monitoring

The agent logs all activity to:

```
./trading_audit_tv/           # Audit logs
    ├── YYYYMMDD_HHMMSS/      # Session folder
    │   ├── decisions.json    # All decisions
    │   ├── trades.json       # Trade executions
    │   ├── screenshots/      # Chart screenshots
    │   └── summary.json      # Session summary
├── trading_agent.log          # Application log
└── trading_trajectories_tv/   # Agent trajectories
```

### Dashboard (Optional)

Set up a monitoring dashboard:

```bash
# Install monitoring tools
pip install streamlit plotly

# Run dashboard
streamlit run trading_dashboard.py  # (create this)
```

### Alerts

Configure alerts for:
- Daily loss limit approaching
- New trades executed
- Errors or API failures
- Kill switch activation

## Strategies Guide

### 1. Trend Following

**Logic**:
- Enter when price crosses above EMA20 with RSI > 50
- Exit when price crosses below EMA20 or target hit
- Stop loss: 2% below EMA20

**Best For**:
- AAPL, MSFT, GOOGL (large caps)
- Bull markets
- Clear trends

**Example**:
```python
# In tradingview_agent_complete.py
strategy = TrendFollowingStrategy()
```

### 2. Breakout

**Logic**:
- Detect consolidation patterns
- Enter on volume spike above resistance
- Exit at pattern target or 5% move

**Best For**:
- TSLA, NVDA (volatile stocks)
- Range-bound markets
- Earnings plays

**Example**:
```python
strategy = BreakoutStrategy()
```

### 3. Custom Strategy

Create your own:

```python
class MyCustomStrategy(TradingStrategy):
    def __init__(self):
        super().__init__("My Custom Strategy")

    async def generate_signal(self, tv_analysis, market_data, indicators):
        # Your logic here
        price = market_data["price"]
        rsi = indicators["RSI_14"]

        # Example: Buy on RSI oversold
        if rsi < 30:
            return {
                "side": OrderSide.BUY,
                "confidence": 0.80,
                "reasoning": f"RSI oversold at {rsi}",
                "entry": price,
                "stop_loss": price * 0.97,
                "target": price * 1.06
            }

        return None
```

## Performance Optimization

### Reduce API Costs

1. **Use Prompt Caching**:
   ```python
   agent = ComputerAgent(
       use_prompt_caching=True  # Already enabled
   )
   ```

2. **Limit Image History**:
   ```python
   only_n_most_recent_images=3  # Keep fewer screenshots
   ```

3. **Increase Scan Interval**:
   ```bash
   --scan-interval 600  # Scan every 10 minutes instead of 5
   ```

### Improve Execution Speed

1. **Parallel Symbol Analysis**:
   ```python
   # Analyze multiple symbols simultaneously
   tasks = [analyze_symbol(s) for s in symbols]
   results = await asyncio.gather(*tasks)
   ```

2. **Use Cached Indicators**:
   - Pre-calculate indicators
   - Update only on new data

3. **WebSocket Data Feeds**:
   - Replace polling with streaming
   - Reduces latency

## Troubleshooting

### Common Issues

**1. "Browser automation failed"**
```bash
# Solution: Check browser installation
# macOS: Safari should be available by default
# Linux: Install Chrome/Chromium

# Verify Computer SDK
python -c "from computer import Computer; print('OK')"
```

**2. "Anthropic API rate limit"**
```bash
# Solution: Reduce scan frequency
--scan-interval 900  # 15 minutes

# Or use prompt caching (enabled by default)
```

**3. "TradingView charts not loading"**
```bash
# Solution: Check internet connection
# Increase wait times in tradingview_integration.py
await asyncio.sleep(5)  # Increase from 2 to 5 seconds
```

**4. "Kill switch activated unexpectedly"**
```bash
# Check logs for reason
grep "TRADING HALTED" trading_agent.log

# Common causes:
# - Daily loss limit reached (check config)
# - Losing streak (5+ losses)
# - Drawdown exceeded
```

## Safety Checklist

Before running in live mode:

- [ ] Tested in paper trading for at least 2 weeks
- [ ] Reviewed all generated trades manually
- [ ] Verified risk limits are appropriate
- [ ] Set up monitoring and alerts
- [ ] Started with small capital ($1,000-$5,000)
- [ ] Understand the code and can modify it
- [ ] Have emergency stop procedure
- [ ] Checked broker API limits and fees
- [ ] Backed up all audit logs
- [ ] Read broker terms of service

## Cost Analysis

### Monthly Operating Costs

| Component | Cost | Notes |
|-----------|------|-------|
| Anthropic API (Claude Sonnet 4.5) | $20-100 | Depends on scan frequency |
| TradingView Pro+ | $0-60 | Optional but recommended |
| Alpaca Pro (data) | $0-99 | Free tier may suffice |
| Cloud hosting (optional) | $0-50 | If running 24/7 |
| **Total** | **$20-309/month** | |

### Break-Even Analysis

With $10,000 capital:
- Monthly costs: ~$50
- Need 0.5% monthly return to break even
- Target: 2-5% monthly for profitability

## Advanced Features

### Multi-Timeframe Analysis

Analyze multiple timeframes before trading:

```python
timeframes = [
    TradingViewTimeframe.D1,
    TradingViewTimeframe.H4,
    TradingViewTimeframe.H1
]

analyses = await tv.multi_timeframe_analysis("AAPL", timeframes)
```

### Options Trading Support

Extend for options trading:

```python
# Add to trading_tools.py
class OptionsAnalyzerTool:
    async def analyze_option_chain(self, symbol):
        # Analyze Greeks, IV, etc.
        pass
```

### Portfolio Optimization

Use modern portfolio theory:

```python
# Add to portfolio_analyzer.py
class PortfolioOptimizer:
    def optimize_weights(self, positions):
        # Sharpe ratio maximization
        pass
```

## Resources

- **TradingView Docs**: https://www.tradingview.com/charting-library-docs/
- **Claude API Docs**: https://docs.anthropic.com
- **Alpaca API Docs**: https://alpaca.markets/docs/
- **Cua Documentation**: https://cua.ai/docs

## Support

- **GitHub Issues**: https://github.com/trycua/cua/issues
- **Discord**: https://discord.com/invite/mVnXXpdE85

## Disclaimer

⚠️ **IMPORTANT DISCLAIMER**:

This trading agent is provided for **EDUCATIONAL PURPOSES ONLY**.

- Trading involves substantial risk of loss
- Past performance does not guarantee future results
- Only trade with capital you can afford to lose
- This is NOT financial advice
- Test thoroughly in paper trading before live use
- The authors are not responsible for any losses
- Review all code before executing trades
- Understand the risks of algorithmic trading

**By using this software, you acknowledge and accept all risks.**

---

**Last Updated**: 2025-11-09
**Version**: 1.0.0
**Status**: Educational/Alpha
