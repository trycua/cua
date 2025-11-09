"""
Quick Start Demo - TradingView AI Trading Agent

A simplified demo that demonstrates the trading agent without requiring
full TradingView browser automation. Perfect for testing the setup.

This demo:
✓ Connects to Alpaca broker (paper trading)
✓ Analyzes stocks using technical indicators
✓ Generates trading signals
✓ Validates trades with risk management
✓ Simulates AI vision analysis
✓ Logs all decisions for audit

No browser automation required - perfect for initial testing!

Usage:
    # Set up .env file first with your Alpaca keys
    cp .env.example .env
    # Edit .env and add your keys

    # Run the demo
    python quick_start_demo.py

    # Or run with custom symbols
    python quick_start_demo.py --symbols AAPL,MSFT,NVDA
"""

import argparse
import asyncio
import logging
import os
import sys
from datetime import datetime
from pathlib import Path

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

try:
    from dotenv import load_dotenv
    load_dotenv()
except ImportError:
    print("⚠️  python-dotenv not installed. Install with: pip install python-dotenv")

from trading_tools import (
    MarketDataTool,
    RiskManagerTool,
    TradeExecutorTool,
    PortfolioAnalyzerTool,
    TradingMode,
    OrderSide,
    OrderType,
    RiskLimits,
    Position
)

from trading_callbacks import (
    KillSwitchCallback,
    AuditTrailCallback,
    PerformanceMonitorCallback
)

# Optional: Use real Alpaca broker if available
try:
    from alpaca_broker import AlpacaBroker
    ALPACA_AVAILABLE = True
except ImportError:
    ALPACA_AVAILABLE = False

# Set up logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)


class SimpleAIAnalyzer:
    """
    Simplified AI analysis that simulates vision analysis without requiring
    actual browser automation or Claude API.

    In production, this would be replaced with actual Claude Sonnet vision
    analyzing TradingView charts.
    """

    @staticmethod
    async def analyze_chart(symbol: str, market_data: dict, indicators: dict) -> dict:
        """
        Simulate AI vision analysis of a TradingView chart.

        In production, this would:
        1. Capture TradingView screenshot
        2. Send to Claude Sonnet 4.5 vision
        3. Get detailed pattern analysis

        For demo, we simulate the analysis based on indicators.
        """
        price = market_data["price"]
        rsi = indicators.get("RSI_14", 50)
        macd = indicators.get("MACD", {})
        ema_20 = indicators.get("EMA_20", price)
        ema_50 = indicators.get("EMA_50", price)

        # Simulate AI vision analysis
        trend = "bullish" if price > ema_20 and ema_20 > ema_50 else "bearish" if price < ema_20 else "neutral"
        momentum = "strong" if abs(rsi - 50) > 20 else "weak"

        patterns = []
        if rsi < 30:
            patterns.append("RSI oversold - potential bounce")
        elif rsi > 70:
            patterns.append("RSI overbought - potential pullback")

        if macd.get("histogram", 0) > 0 and price > ema_20:
            patterns.append("Bullish MACD crossover above EMA20")
        elif macd.get("histogram", 0) < 0 and price < ema_20:
            patterns.append("Bearish MACD crossover below EMA20")

        support = round(min(ema_20, ema_50) * 0.98, 2)
        resistance = round(max(ema_20, ema_50) * 1.02, 2)

        analysis = f"""
        AI VISION ANALYSIS FOR {symbol}:

        TREND: {trend.upper()} (Momentum: {momentum})
        - Price: ${price:.2f}
        - EMA20: ${ema_20:.2f}
        - EMA50: ${ema_50:.2f}
        - RSI(14): {rsi:.1f}

        PATTERNS DETECTED:
        {chr(10).join(f'  - {p}' for p in patterns) if patterns else '  - No clear patterns'}

        KEY LEVELS:
        - Support: ${support:.2f}
        - Resistance: ${resistance:.2f}

        MACD: {macd.get('value', 0):.3f} (Signal: {macd.get('signal', 0):.3f})
        """

        return {
            "analysis_text": analysis,
            "trend": trend,
            "patterns": patterns,
            "support": support,
            "resistance": resistance,
            "confidence": 0.75 if patterns else 0.50
        }


class QuickStartDemo:
    """
    Quick start demo of the trading agent.

    Demonstrates all core functionality without requiring browser automation.
    """

    def __init__(self, symbols: list, paper: bool = True):
        self.symbols = symbols
        self.paper = paper

        # Initialize tools
        self.market_data = MarketDataTool(mode=TradingMode.PAPER if paper else TradingMode.LIVE)
        self.risk_manager = RiskManagerTool(limits=RiskLimits(
            max_position_size=0.10,
            max_daily_loss=0.02,
            max_drawdown=0.10,
            require_stop_loss=True
        ))
        self.executor = TradeExecutorTool(mode=TradingMode.PAPER if paper else TradingMode.LIVE)
        self.analyzer = PortfolioAnalyzerTool()

        # Initialize callbacks
        self.kill_switch = KillSwitchCallback(
            max_daily_loss=0.02,
            max_drawdown=0.10,
            max_losing_streak=5
        )
        self.audit_trail = AuditTrailCallback(save_dir="./demo_audit")
        self.monitor = PerformanceMonitorCallback()

        # AI analyzer (simulated)
        self.ai_analyzer = SimpleAIAnalyzer()

        # Real broker (if available)
        self.alpaca_broker = None
        if ALPACA_AVAILABLE and os.getenv("ALPACA_API_KEY"):
            try:
                self.alpaca_broker = AlpacaBroker(paper=paper)
                logger.info("✅ Connected to Alpaca broker")
            except Exception as e:
                logger.warning(f"Alpaca not available: {e}")

        self.portfolio_value = 100000  # Starting value

    async def analyze_symbol(self, symbol: str) -> dict:
        """Analyze a symbol and generate trading signal."""
        logger.info(f"📊 Analyzing {symbol}...")

        # Get market data
        if self.alpaca_broker:
            # Use real Alpaca data
            quote = await self.alpaca_broker.get_quote(symbol)
            market_data = {
                "symbol": symbol,
                "price": quote["price"],
                "bid": quote["bid"],
                "ask": quote["ask"],
                "volume": 0,  # Not available in quote
                "timestamp": quote["timestamp"]
            }
        else:
            # Use mock data
            market_data = await self.market_data.get_quote(symbol)

        # Get technical indicators
        indicators = await self.market_data.get_technical_indicators(
            symbol,
            ["RSI_14", "MACD", "EMA_20", "EMA_50"]
        )

        # AI vision analysis (simulated)
        ai_analysis = await self.ai_analyzer.analyze_chart(
            symbol,
            market_data,
            indicators
        )

        print(ai_analysis["analysis_text"])

        # Generate trading signal
        signal = await self._generate_signal(symbol, market_data, indicators, ai_analysis)

        return {
            "symbol": symbol,
            "market_data": market_data,
            "indicators": indicators,
            "ai_analysis": ai_analysis,
            "signal": signal
        }

    async def _generate_signal(
        self,
        symbol: str,
        market_data: dict,
        indicators: dict,
        ai_analysis: dict
    ) -> dict:
        """Generate trading signal based on analysis."""

        price = market_data["price"]
        rsi = indicators.get("RSI_14", 50)
        ema_20 = indicators.get("EMA_20", price)
        macd = indicators.get("MACD", {})

        # Simple trend-following strategy
        signal = None

        # Buy signal: Bullish trend with RSI not overbought
        if (price > ema_20 and
            rsi > 40 and rsi < 70 and
            macd.get("histogram", 0) > 0):

            signal = {
                "side": OrderSide.BUY,
                "entry": price,
                "stop_loss": ema_20 * 0.98,  # 2% below EMA
                "target": price * 1.04,      # 4% target
                "confidence": ai_analysis.get("confidence", 0.70),
                "reasoning": f"Bullish: Price above EMA20 ({ema_20:.2f}), RSI={rsi:.1f}, MACD positive"
            }

        # Sell signal: Bearish trend with RSI not oversold
        elif (price < ema_20 and
              rsi < 60 and rsi > 30 and
              macd.get("histogram", 0) < 0):

            signal = {
                "side": OrderSide.SELL,
                "entry": price,
                "stop_loss": ema_20 * 1.02,  # 2% above EMA
                "target": price * 0.96,      # 4% target
                "confidence": ai_analysis.get("confidence", 0.65),
                "reasoning": f"Bearish: Price below EMA20 ({ema_20:.2f}), RSI={rsi:.1f}, MACD negative"
            }

        if signal:
            logger.info(f"📈 SIGNAL: {signal['side'].value.upper()} {symbol}")
            logger.info(f"   Entry: ${signal['entry']:.2f}")
            logger.info(f"   Stop: ${signal['stop_loss']:.2f}")
            logger.info(f"   Target: ${signal['target']:.2f}")
            logger.info(f"   Confidence: {signal['confidence']:.0%}")
            logger.info(f"   Reason: {signal['reasoning']}")
        else:
            logger.info(f"⏸️  No signal for {symbol}")

        return signal

    async def execute_signal(self, analysis: dict) -> dict:
        """Execute a trading signal with risk management."""

        signal = analysis["signal"]
        if not signal:
            return {"success": False, "reason": "No signal"}

        symbol = analysis["symbol"]
        logger.info(f"\n🎯 Executing signal for {symbol}...")

        # 1. Calculate position size
        position_calc = await self.risk_manager.calculate_position_size(
            symbol=symbol,
            entry_price=signal["entry"],
            stop_loss_price=signal["stop_loss"],
            portfolio_value=self.portfolio_value,
            risk_per_trade=0.01  # 1% risk
        )

        logger.info(f"   Position size: {position_calc['shares']} shares")
        logger.info(f"   Position value: ${position_calc['position_value']:,.2f}")
        logger.info(f"   Risk: ${position_calc['risk_amount']:.2f} ({position_calc['risk_percent']:.2f}%)")

        # 2. Validate trade
        positions = await self.executor.get_positions()
        validation = await self.risk_manager.validate_trade(
            symbol=symbol,
            side=signal["side"],
            quantity=position_calc["shares"],
            price=signal["entry"],
            stop_loss=signal["stop_loss"],
            portfolio_value=self.portfolio_value,
            current_positions=positions
        )

        if not validation["approved"]:
            logger.warning(f"   ❌ Trade rejected: {validation['reason']}")
            return {"success": False, "reason": validation["reason"]}

        logger.info(f"   ✅ Trade validated")

        # 3. Execute order
        order = await self.executor.submit_order(
            symbol=symbol,
            side=signal["side"],
            quantity=position_calc["shares"],
            order_type=OrderType.MARKET
        )

        logger.info(f"   📝 Order executed: {order['order_id']}")
        logger.info(f"   Status: {order['status']}")

        # 4. Log for audit
        await self.audit_trail.log_trade(
            symbol=symbol,
            side=signal["side"].value,
            quantity=position_calc["shares"],
            price=order["filled_avg_price"],
            order_id=order["order_id"],
            reasoning=signal["reasoning"]
        )

        return {
            "success": True,
            "order": order,
            "position_calc": position_calc
        }

    async def run_scan(self):
        """Run a complete scan of the watchlist."""
        print("\n" + "="*70)
        print("TradingView AI Trading Agent - Quick Start Demo")
        print("="*70)
        print(f"Mode: {'PAPER TRADING' if self.paper else 'LIVE TRADING'}")
        print(f"Symbols: {', '.join(self.symbols)}")
        print(f"Portfolio Value: ${self.portfolio_value:,.2f}")
        print("="*70 + "\n")

        # Show account info if Alpaca connected
        if self.alpaca_broker:
            account = await self.alpaca_broker.get_account()
            print("📊 ACCOUNT INFO:")
            print(f"   Portfolio Value: ${account['portfolio_value']:,.2f}")
            print(f"   Cash: ${account['cash']:,.2f}")
            print(f"   Buying Power: ${account['buying_power']:,.2f}")
            print()

        signals_found = []

        for symbol in self.symbols:
            try:
                # Analyze symbol
                analysis = await self.analyze_symbol(symbol)

                if analysis["signal"]:
                    signals_found.append(analysis)

                    # Execute signal (in demo, we'll just simulate)
                    result = await self.execute_signal(analysis)

                    if result["success"]:
                        print(f"\n✅ Trade executed successfully for {symbol}\n")
                    else:
                        print(f"\n⏸️  Trade not executed: {result['reason']}\n")

                print("-" * 70 + "\n")

                await asyncio.sleep(1)  # Rate limiting

            except Exception as e:
                logger.error(f"Error analyzing {symbol}: {e}")

        # Summary
        print("\n" + "="*70)
        print("SCAN SUMMARY")
        print("="*70)
        print(f"Symbols scanned: {len(self.symbols)}")
        print(f"Signals found: {len(signals_found)}")

        if signals_found:
            print("\nSignals:")
            for analysis in signals_found:
                signal = analysis["signal"]
                print(f"  - {analysis['symbol']}: {signal['side'].value.upper()} @ ${signal['entry']:.2f}")

        # Performance summary
        print()
        self.monitor.print_summary()

        # Generate audit report
        summary = self.audit_trail.generate_summary_report()
        print(f"\nAudit logs saved to: {self.audit_trail.session_dir}")
        print("="*70 + "\n")


async def main():
    """Main entry point."""
    parser = argparse.ArgumentParser(description="TradingView Trading Agent Quick Start Demo")
    parser.add_argument(
        "--symbols",
        type=str,
        default="AAPL,MSFT,NVDA",
        help="Comma-separated list of symbols to analyze"
    )
    parser.add_argument(
        "--paper",
        action="store_true",
        default=True,
        help="Use paper trading mode (default)"
    )

    args = parser.parse_args()
    symbols = [s.strip().upper() for s in args.symbols.split(",")]

    # Check for environment setup
    if not os.path.exists(".env"):
        print("\n⚠️  No .env file found!")
        print("Copy .env.example to .env and add your credentials:")
        print("  cp .env.example .env\n")
        print("You need:")
        print("  1. ANTHROPIC_API_KEY (from https://console.anthropic.com)")
        print("  2. ALPACA_API_KEY and ALPACA_SECRET_KEY (from https://alpaca.markets)")
        print("\nFor demo purposes, we'll run in simulation mode...\n")

    # Run demo
    demo = QuickStartDemo(symbols=symbols, paper=args.paper)

    try:
        await demo.run_scan()
    except KeyboardInterrupt:
        print("\n\nDemo interrupted by user")
    except Exception as e:
        logger.error(f"Error running demo: {e}", exc_info=True)


if __name__ == "__main__":
    asyncio.run(main())
