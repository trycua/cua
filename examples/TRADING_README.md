# TradingView AI Trading Agent - Quick Start Guide

Welcome! This guide will get you up and running with your AI-powered trading agent in under 10 minutes.

## 🎯 What You're Building

A professional trading agent that:
- ✅ Analyzes TradingView charts using Claude Sonnet 4.5 vision
- ✅ Identifies patterns, trends, and trading opportunities
- ✅ Executes trades automatically via broker API
- ✅ Manages risk with stop-losses and position sizing
- ✅ Logs all decisions for compliance and analysis
- ✅ Starts in **paper trading** mode (no real money risk)

## 🚀 5-Minute Quick Start

### Step 1: Run the Setup Script

```bash
cd examples
./setup_trading_agent.sh
```

This installs all dependencies automatically.

### Step 2: Get Your API Keys

You need two free accounts:

**1. Anthropic (AI Vision) - ~5 minutes**
- Go to: https://console.anthropic.com
- Sign up and get API key
- Free $5 credit to start

**2. Alpaca (Broker) - ~5 minutes**
- Go to: https://alpaca.markets
- Create account (instant approval)
- Get **paper trading** keys (no money needed)

### Step 3: Configure Environment

```bash
# Copy template
cp .env.example .env

# Edit and add your keys
nano .env

# Add these lines:
ANTHROPIC_API_KEY=sk-ant-api03-YOUR_KEY_HERE
ALPACA_API_KEY=YOUR_ALPACA_PAPER_KEY
ALPACA_SECRET_KEY=YOUR_ALPACA_PAPER_SECRET
ALPACA_BASE_URL=https://paper-api.alpaca.markets
```

### Step 4: Test the Setup

```bash
# Test broker connection
python alpaca_broker.py

# Run quick demo
python quick_start_demo.py --symbols AAPL,MSFT,NVDA
```

### Step 5: Run the Trading Agent

```bash
# Paper trading mode (safe testing)
python tradingview_agent_complete.py \
    --mode paper \
    --symbols "AAPL,MSFT,GOOGL,NVDA,TSLA" \
    --strategy trend_following
```

**That's it!** Your agent is now running in paper trading mode.

## 📚 Files Overview

| File | Purpose |
|------|---------|
| `quick_start_demo.py` | **Start here** - Simple demo without browser |
| `tradingview_agent_complete.py` | Full agent with TradingView integration |
| `alpaca_broker.py` | Broker API integration |
| `trading_tools.py` | Core trading tools (risk, execution, data) |
| `trading_callbacks.py` | Safety features (kill switch, audit) |
| `tradingview_integration.py` | TradingView browser automation |
| `.env.example` | Configuration template |
| `requirements-trading.txt` | Python dependencies |

## 🎓 Learning Path

### Day 1: Understanding the Basics
```bash
# 1. Read the quick demo code
cat quick_start_demo.py

# 2. Run it and watch the output
python quick_start_demo.py

# 3. Review the audit logs
ls demo_audit/
cat demo_audit/*/summary.json
```

### Day 2-7: Paper Trading
```bash
# Run daily scans
python tradingview_agent_complete.py --mode paper

# Review decisions
tail -f trading_agent.log

# Check performance
cat trading_audit_tv/*/summary.json
```

### Week 2-3: Optimization
- Adjust risk parameters in `.env`
- Try different strategies
- Analyze audit logs for patterns
- Fine-tune entry/exit rules

### Week 4+: Consider Live Trading
⚠️ **Only after:**
- 2+ weeks successful paper trading
- Reviewed all decisions manually
- Starting with small capital ($1,000-$5,000)
- Monitoring closely daily

## 🔧 Configuration

### Risk Settings (in `.env`)

```bash
# Conservative (Recommended to start)
MAX_POSITION_SIZE=0.05      # 5% max per trade
MAX_DAILY_LOSS=0.01         # 1% daily loss limit
RISK_PER_TRADE=0.005        # 0.5% risk per trade

# Moderate
MAX_POSITION_SIZE=0.10      # 10% max per trade
MAX_DAILY_LOSS=0.02         # 2% daily loss limit
RISK_PER_TRADE=0.01         # 1% risk per trade

# Aggressive (Not recommended for beginners)
MAX_POSITION_SIZE=0.20      # 20% max per trade
MAX_DAILY_LOSS=0.03         # 3% daily loss limit
RISK_PER_TRADE=0.02         # 2% risk per trade
```

### Trading Strategies

**Trend Following** (Default)
```bash
--strategy trend_following
```
- Follows EMA crossovers
- Best for: Strong trending stocks (AAPL, MSFT)
- Win rate: 45-55%

**Breakout**
```bash
--strategy breakout
```
- Trades consolidation breakouts
- Best for: Volatile stocks (TSLA, NVDA)
- Win rate: 40-50%

### Watchlists

Edit in `.env`:
```bash
# Large caps (stable)
WATCHLIST=AAPL,MSFT,GOOGL,AMZN,META

# Tech growth (volatile)
WATCHLIST=NVDA,AMD,TSLA,PLTR,SNOW

# Index ETFs (diversified)
WATCHLIST=SPY,QQQ,IWM,DIA

# Custom mix
WATCHLIST=AAPL,NVDA,SPY,TSLA,MSFT,AMD,GOOGL,META
```

## 🛡️ Safety Features

### Kill Switch (Automatic Stop)

The agent automatically stops trading when:
- Daily loss exceeds limit (default: 2%)
- Maximum drawdown reached (default: 10%)
- 5+ consecutive losing trades
- Any API connection failures

### Audit Trail

Every decision is logged with:
- Screenshot of chart (if using TradingView)
- AI reasoning
- Trade execution details
- Risk calculations
- Timestamp

Logs saved to: `trading_audit_tv/YYYYMMDD_HHMMSS/`

### Compliance Checks

- Pattern Day Trader (PDT) rule monitoring
- Wash sale detection (30-day rule)
- Position limit enforcement
- Good faith violation prevention

## 📊 Monitoring

### Real-time Monitoring

```bash
# Watch live logs
tail -f trading_agent.log

# Monitor specific symbol
tail -f trading_agent.log | grep AAPL

# Watch for errors
tail -f trading_agent.log | grep ERROR
```

### Performance Dashboard

View audit logs:
```bash
cd trading_audit_tv/LATEST_SESSION/
cat summary.json          # Session summary
cat trades.json           # All trades
cat decisions.json        # AI decisions
ls screenshots/           # Chart images
```

## 🐛 Troubleshooting

### "Alpaca credentials not found"

```bash
# Check .env file exists
ls -la .env

# Verify keys are set
grep ALPACA .env

# Test manually
export ALPACA_API_KEY="your_key"
export ALPACA_SECRET_KEY="your_secret"
python alpaca_broker.py
```

### "Anthropic API key invalid"

```bash
# Verify key format (should start with sk-ant-api03-)
grep ANTHROPIC_API_KEY .env

# Test directly
python -c "import anthropic; print(anthropic.Anthropic(api_key='YOUR_KEY').messages.count_tokens(messages=[]))"
```

### "Trading halted by kill switch"

This is a **safety feature**! Check logs for reason:
```bash
grep "TRADING HALTED" trading_agent.log
```

Common causes:
- Daily loss limit reached → Reduce position sizes
- Losing streak → Review strategy
- Maximum drawdown → Check risk settings

Reset:
- Wait for next trading day
- Or adjust risk limits in `.env`

### "Browser automation failed"

For full TradingView integration:
```bash
# macOS: Install Chrome
brew install --cask google-chrome

# Linux: Install Chromium
sudo apt install chromium-browser

# Test computer SDK
python -c "from computer import Computer; print('OK')"
```

## 💡 Tips for Success

### 1. Start Small
```bash
# Begin with 1-3 symbols
--symbols AAPL,MSFT

# Low scan frequency
--scan-interval 600  # 10 minutes
```

### 2. Test Thoroughly
- Run paper trading for 2+ weeks minimum
- Review every decision manually
- Understand why agent makes each trade
- Keep a trading journal

### 3. Monitor Closely
- Check logs daily
- Review performance weekly
- Adjust risk settings based on results
- Don't set and forget!

### 4. Gradual Scaling
```bash
# Week 1-2: Paper trading, $100k virtual
# Week 3-4: Paper trading, review all trades
# Week 5+: Consider live with $1,000-$2,000
# Month 2+: Gradually increase if profitable
```

### 5. Know When to Stop
- Consistent losses → Pause and analyze
- Strategy not working → Try different approach
- High stress → Take a break
- Market conditions changed → Adapt strategy

## 📖 Additional Resources

### Documentation
- [Complete Setup Guide](../TRADINGVIEW_SETUP.md) - Detailed installation
- [Architecture](../TRADING_AGENT_ARCHITECTURE.md) - System design
- [Cua Docs](https://cua.ai/docs) - Framework documentation

### Learning
- [Alpaca Docs](https://alpaca.markets/docs) - Broker API reference
- [TradingView](https://www.tradingview.com/education/) - Technical analysis
- [Claude API](https://docs.anthropic.com) - AI integration

### Community
- [GitHub Issues](https://github.com/trycua/cua/issues) - Bug reports
- [Discord](https://discord.com/invite/mVnXXpdE85) - Community support

## ⚠️ Important Disclaimers

**This is educational software.** Trading carries significant financial risk.

- ✅ Start with **paper trading** only
- ✅ Test for minimum 2 weeks
- ✅ Only trade money you can afford to lose
- ✅ Monitor the agent closely
- ✅ Understand all code before using

**Not financial advice.** You are responsible for all trading decisions and losses.

## 🎯 Next Steps

**Complete Beginner?**
1. Run `quick_start_demo.py` daily for a week
2. Read the logs and understand decisions
3. Study `trading_tools.py` code
4. Customize strategy in `tradingview_agent_complete.py`
5. Paper trade for 2+ weeks

**Experienced Trader?**
1. Review strategy code
2. Customize entry/exit rules
3. Add custom technical indicators
4. Optimize risk parameters
5. Backtest before live trading

**Ready for Live Trading?**
1. Verify 2+ weeks successful paper trading
2. Start with minimal capital ($1,000-$2,000)
3. Monitor closely 24/7 during market hours
4. Scale gradually based on results
5. Always have manual override ready

---

## 🚀 Quick Commands Reference

```bash
# Setup
./setup_trading_agent.sh

# Quick Demo (No browser needed)
python quick_start_demo.py

# Test Broker
python alpaca_broker.py

# Paper Trading
python tradingview_agent_complete.py --mode paper

# Custom Symbols
python tradingview_agent_complete.py --mode paper --symbols "AAPL,NVDA,TSLA"

# Different Strategy
python tradingview_agent_complete.py --mode paper --strategy breakout

# View Logs
tail -f trading_agent.log

# Check Audit
cat trading_audit_tv/*/summary.json
```

---

**Happy Trading!** 📈

Remember: The best trade is sometimes no trade. Let the AI do the analysis, but always understand its decisions.

Questions? Check the [full documentation](../TRADINGVIEW_SETUP.md) or ask in [Discord](https://discord.com/invite/mVnXXpdE85).
