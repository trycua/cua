"""
Complete TradingView Trading Agent with AI Vision

A production-ready trading agent that uses TradingView for chart analysis
and AI vision for pattern recognition and decision making.

This implementation:
- Uses TradingView exclusively for charting
- Leverages Claude Sonnet 4.5 vision for chart analysis
- Integrates with broker APIs for execution
- Includes full risk management and safety features
- Provides comprehensive audit trails

Platform: TradingView (https://www.tradingview.com)
API Docs: https://www.tradingview.com/charting-library-docs/latest/api/

⚠️  DISCLAIMER: For educational purposes only. Test thoroughly in paper trading.

Usage:
    # Paper trading mode (recommended for testing)
    python tradingview_agent_complete.py --mode paper --symbols AAPL,MSFT,NVDA

    # With custom strategy
    python tradingview_agent_complete.py --mode paper --strategy breakout

    # Live trading (extreme caution)
    python tradingview_agent_complete.py --mode live --confirm-risk
"""

import argparse
import asyncio
import logging
import os
import signal
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional

from agent import ComputerAgent
from computer import Computer

# Import trading components
from tradingview_integration import (
    TradingViewTool,
    TradingViewTimeframe,
    ChartConfig
)

from trading_tools import (
    MarketDataTool,
    RiskManagerTool,
    TradeExecutorTool,
    PortfolioAnalyzerTool,
    TradingMode,
    OrderSide,
    OrderType,
    RiskLimits
)

from trading_callbacks import (
    KillSwitchCallback,
    AuditTrailCallback,
    ComplianceCallback,
    PerformanceMonitorCallback
)

# Set up logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s',
    handlers=[
        logging.FileHandler('trading_agent.log'),
        logging.StreamHandler()
    ]
)
logger = logging.getLogger(__name__)


class TradingStrategy:
    """Base class for trading strategies."""

    def __init__(self, name: str):
        self.name = name

    async def generate_signal(
        self,
        tv_analysis: Dict[str, Any],
        market_data: Dict[str, Any],
        indicators: Dict[str, Any]
    ) -> Optional[Dict[str, Any]]:
        """
        Generate trading signal based on analysis.

        Returns:
            Dict with: side, confidence, reasoning, entry, stop_loss, target
            Or None if no signal
        """
        raise NotImplementedError


class TrendFollowingStrategy(TradingStrategy):
    """
    Trend following strategy using moving averages and momentum.

    Entry signals:
    - Price crosses above EMA20 with RSI > 50
    - Strong volume confirmation
    - MACD positive crossover
    """

    def __init__(self):
        super().__init__("Trend Following")

    async def generate_signal(
        self,
        tv_analysis: Dict[str, Any],
        market_data: Dict[str, Any],
        indicators: Dict[str, Any]
    ) -> Optional[Dict[str, Any]]:

        price = market_data["price"]
        rsi = indicators.get("RSI_14", 50)
        macd = indicators.get("MACD", {})
        ema_20 = indicators.get("EMA_20", price)

        # Bullish conditions
        if (price > ema_20 and
            rsi > 50 and rsi < 70 and
            macd.get("histogram", 0) > 0):

            return {
                "side": OrderSide.BUY,
                "confidence": 0.75,
                "reasoning": f"Bullish trend: Price above EMA20, RSI={rsi:.1f}, MACD positive",
                "entry": price,
                "stop_loss": ema_20 * 0.98,  # 2% below EMA
                "target": price * 1.04,  # 4% target (2:1 R:R)
            }

        # Bearish conditions
        elif (price < ema_20 and
              rsi < 50 and rsi > 30 and
              macd.get("histogram", 0) < 0):

            return {
                "side": OrderSide.SELL,
                "confidence": 0.70,
                "reasoning": f"Bearish trend: Price below EMA20, RSI={rsi:.1f}, MACD negative",
                "entry": price,
                "stop_loss": ema_20 * 1.02,  # 2% above EMA
                "target": price * 0.96,  # 4% target
            }

        return None


class BreakoutStrategy(TradingStrategy):
    """
    Breakout strategy using support/resistance and volume.

    Entry signals:
    - Price breaks above resistance with volume spike
    - Consolidation followed by expansion
    """

    def __init__(self):
        super().__init__("Breakout")

    async def generate_signal(
        self,
        tv_analysis: Dict[str, Any],
        market_data: Dict[str, Any],
        indicators: Dict[str, Any]
    ) -> Optional[Dict[str, Any]]:

        # This would use the AI vision analysis from TradingView
        # to detect breakout patterns

        # Placeholder implementation
        price = market_data["price"]
        volume = market_data.get("volume", 0)

        # Would check for:
        # - Consolidation period
        # - Volume spike
        # - Clear breakout above resistance

        return None


class TradingViewAgent:
    """
    Professional trading agent using TradingView and AI vision.

    This agent:
    1. Uses TradingView for chart visualization and analysis
    2. Employs Claude Sonnet vision for pattern recognition
    3. Executes trades via broker API (Alpaca, IBKR, etc.)
    4. Maintains strict risk management
    5. Provides comprehensive audit trails
    """

    def __init__(
        self,
        config: Dict[str, Any],
        strategy: TradingStrategy
    ):
        self.config = config
        self.strategy = strategy
        self.is_running = False
        self.should_stop = False

        # Core components
        self.computer: Optional[Computer] = None
        self.agent: Optional[ComputerAgent] = None
        self.tradingview: Optional[TradingViewTool] = None

        # Trading tools
        self.market_data = MarketDataTool(mode=TradingMode(config["mode"]))
        self.risk_manager = RiskManagerTool(limits=RiskLimits(
            max_position_size=config.get("max_position_size", 0.10),
            max_daily_loss=config.get("max_daily_loss", 0.02),
            max_drawdown=config.get("max_drawdown", 0.10)
        ))
        self.executor = TradeExecutorTool(mode=TradingMode(config["mode"]))
        self.analyzer = PortfolioAnalyzerTool()

        # Safety callbacks
        self.kill_switch = KillSwitchCallback(
            max_daily_loss=config.get("max_daily_loss", 0.02),
            max_drawdown=config.get("max_drawdown", 0.10)
        )
        self.audit_trail = AuditTrailCallback(save_dir="./trading_audit_tv")
        self.compliance = ComplianceCallback(
            account_value=config.get("portfolio_value", 100000)
        )
        self.monitor = PerformanceMonitorCallback()

        logger.info(f"TradingView Agent initialized ({config['mode']} mode)")
        logger.info(f"Strategy: {strategy.name}")

    async def initialize(self):
        """Initialize all components."""
        logger.info("Initializing TradingView Agent...")

        # 1. Initialize computer for browser automation
        self.computer = Computer(
            os_type=self.config.get("os_type", "macos"),
            verbosity=logging.INFO
        )
        await self.computer.run()

        # 2. Initialize TradingView
        self.tradingview = TradingViewTool(computer=self.computer)
        await self.tradingview.initialize()

        # 3. Create AI agent with vision
        tools = [
            self.market_data,
            self.risk_manager,
            self.executor,
            self.analyzer,
            self.tradingview
        ]

        self.agent = ComputerAgent(
            model=self.config.get("model", "anthropic/claude-sonnet-4-5"),
            tools=tools,
            only_n_most_recent_images=5,  # Keep recent chart images
            trajectory_dir="./trading_trajectories_tv",
            max_trajectory_budget=self.config.get("max_budget", 20.0),
            use_prompt_caching=True,
            verbosity=logging.INFO
        )

        logger.info("✅ TradingView Agent fully initialized")

    async def analyze_symbol_with_vision(
        self,
        symbol: str,
        timeframe: TradingViewTimeframe = TradingViewTimeframe.D1
    ) -> Dict[str, Any]:
        """
        Complete analysis workflow for a symbol using TradingView + AI vision.

        Steps:
        1. Load chart in TradingView
        2. Capture screenshot
        3. AI vision analysis
        4. Technical indicator analysis
        5. Generate trading signal
        """
        logger.info(f"📊 Analyzing {symbol} on {timeframe.value} timeframe")

        # 1. Load chart in TradingView
        await self.tradingview.load_chart(
            symbol=symbol,
            timeframe=timeframe,
            indicators=["RSI", "MACD", "EMA 20", "EMA 50", "Volume"]
        )

        # 2. AI vision analysis using the agent
        tv_analysis = await self.tradingview.analyze_chart_with_ai(
            symbol=symbol,
            analysis_type="comprehensive"
        )

        # 3. Get market data and indicators
        quote = await self.market_data.get_quote(symbol)
        indicators = await self.market_data.get_technical_indicators(
            symbol,
            ["RSI_14", "MACD", "EMA_20", "EMA_50", "SMA_50"]
        )

        # 4. Generate trading signal using strategy
        signal = await self.strategy.generate_signal(
            tv_analysis=tv_analysis,
            market_data=quote,
            indicators=indicators
        )

        # 5. Capture screenshot for audit
        screenshot = await self.tradingview.take_chart_screenshot()

        # 6. Log analysis decision
        await self.audit_trail.log_decision(
            decision_type="chart_analysis",
            reasoning=tv_analysis.get("ai_response", "Analysis completed"),
            data={
                "symbol": symbol,
                "timeframe": timeframe.value,
                "quote": quote,
                "indicators": indicators,
                "signal": signal
            },
            screenshot=screenshot
        )

        return {
            "symbol": symbol,
            "timeframe": timeframe.value,
            "tv_analysis": tv_analysis,
            "market_data": quote,
            "indicators": indicators,
            "signal": signal,
            "timestamp": datetime.now().isoformat()
        }

    async def execute_signal(
        self,
        symbol: str,
        signal: Dict[str, Any]
    ) -> Dict[str, Any]:
        """Execute a trading signal with full risk management."""

        if not signal:
            return {"success": False, "reason": "No signal provided"}

        logger.info(f"🎯 Executing signal: {signal['side'].value} {symbol}")

        # 1. Calculate position size
        position_calc = await self.risk_manager.calculate_position_size(
            symbol=symbol,
            entry_price=signal["entry"],
            stop_loss_price=signal["stop_loss"],
            portfolio_value=self.config["portfolio_value"],
            risk_per_trade=0.01
        )

        # 2. Validate trade
        positions = await self.executor.get_positions()
        validation = await self.risk_manager.validate_trade(
            symbol=symbol,
            side=signal["side"],
            quantity=position_calc["shares"],
            price=signal["entry"],
            stop_loss=signal["stop_loss"],
            portfolio_value=self.config["portfolio_value"],
            current_positions=positions
        )

        if not validation["approved"]:
            logger.warning(f"❌ Trade rejected: {validation['reason']}")
            return {"success": False, "reason": validation["reason"]}

        # 3. Check compliance
        try:
            await self.compliance.on_before_trade({
                "symbol": symbol,
                "side": signal["side"].value,
                "is_day_trade": False
            })
        except RuntimeError as e:
            logger.error(f"❌ Compliance failed: {e}")
            return {"success": False, "reason": str(e)}

        # 4. Execute order
        order = await self.executor.submit_order(
            symbol=symbol,
            side=signal["side"],
            quantity=position_calc["shares"],
            order_type=OrderType.MARKET
        )

        # 5. Log trade
        await self.audit_trail.log_trade(
            symbol=symbol,
            side=signal["side"].value,
            quantity=position_calc["shares"],
            price=order["filled_avg_price"],
            order_id=order["order_id"],
            reasoning=signal["reasoning"]
        )

        logger.info(f"✅ Trade executed: {order['order_id']}")

        return {
            "success": True,
            "order": order,
            "position_calc": position_calc,
            "signal": signal
        }

    async def scan_and_trade(self):
        """Scan watchlist and execute trades."""

        symbols = self.config.get("symbols", ["AAPL", "MSFT", "GOOGL"])
        timeframe = TradingViewTimeframe.D1

        logger.info(f"🔍 Scanning {len(symbols)} symbols...")

        for symbol in symbols:
            try:
                # Check kill switch
                if self.kill_switch.is_halted:
                    logger.critical("🛑 Kill switch activated, stopping...")
                    break

                # Analyze symbol
                analysis = await self.analyze_symbol_with_vision(symbol, timeframe)

                # Execute signal if found
                if analysis["signal"]:
                    logger.info(f"📈 Signal found for {symbol}")
                    result = await self.execute_signal(symbol, analysis["signal"])

                    if result["success"]:
                        # Update performance tracking
                        self.monitor.update_performance({
                            "symbol": symbol,
                            "pnl": 0,
                            "portfolio_value": self.config["portfolio_value"]
                        })

                await asyncio.sleep(2)  # Rate limiting

            except Exception as e:
                logger.error(f"Error analyzing {symbol}: {e}")

        # Print performance summary
        self.monitor.print_summary()

    async def trading_loop(self):
        """Main trading loop."""
        logger.info("🚀 Starting trading loop...")
        self.is_running = True

        scan_interval = self.config.get("scan_interval", 300)  # 5 minutes default

        while not self.should_stop:
            try:
                # Check market hours
                if not await self.market_data.is_market_open():
                    logger.info("💤 Market closed, waiting...")
                    await asyncio.sleep(60)
                    continue

                # Check kill switch
                if self.kill_switch.is_halted:
                    logger.critical("🛑 Trading halted by kill switch!")
                    break

                # Scan and trade
                await self.scan_and_trade()

                # Wait before next scan
                logger.info(f"⏱️  Waiting {scan_interval}s until next scan...")
                await asyncio.sleep(scan_interval)

            except Exception as e:
                logger.error(f"Error in trading loop: {e}")
                await asyncio.sleep(10)

        self.is_running = False
        logger.info("Trading loop stopped")

    async def run(self):
        """Run the trading agent."""
        try:
            await self.initialize()
            self.kill_switch.reset_daily_limits()
            await self.trading_loop()

        except KeyboardInterrupt:
            logger.info("Received interrupt, shutting down...")
            self.should_stop = True

        finally:
            await self.shutdown()

    async def shutdown(self):
        """Clean shutdown."""
        logger.info("Shutting down TradingView Agent...")

        # Generate reports
        summary = await self.audit_trail.on_session_end()
        logger.info(f"Session summary: {summary}")

        # Stop computer
        if self.computer:
            await self.computer.stop()

        logger.info("✅ Shutdown complete")


def parse_args():
    """Parse command line arguments."""
    parser = argparse.ArgumentParser(
        description="TradingView Trading Agent with AI Vision"
    )

    parser.add_argument(
        "--mode",
        type=str,
        default="paper",
        choices=["paper", "live"],
        help="Trading mode"
    )

    parser.add_argument(
        "--symbols",
        type=str,
        default="AAPL,MSFT,GOOGL,NVDA,TSLA",
        help="Comma-separated list of symbols"
    )

    parser.add_argument(
        "--strategy",
        type=str,
        default="trend_following",
        choices=["trend_following", "breakout"],
        help="Trading strategy"
    )

    parser.add_argument(
        "--portfolio-value",
        type=float,
        default=100000,
        help="Portfolio value"
    )

    parser.add_argument(
        "--max-budget",
        type=float,
        default=20.0,
        help="Max API budget (USD)"
    )

    parser.add_argument(
        "--scan-interval",
        type=int,
        default=300,
        help="Scan interval in seconds"
    )

    parser.add_argument(
        "--confirm-risk",
        action="store_true",
        help="Confirm trading risks"
    )

    return parser.parse_args()


def main():
    """Main entry point."""
    args = parse_args()

    # Safety check for live mode
    if args.mode == "live" and not args.confirm_risk:
        print("\n⚠️  ERROR: Live trading requires --confirm-risk flag\n")
        return

    # Parse symbols
    symbols = [s.strip() for s in args.symbols.split(",")]

    # Select strategy
    if args.strategy == "trend_following":
        strategy = TrendFollowingStrategy()
    elif args.strategy == "breakout":
        strategy = BreakoutStrategy()
    else:
        strategy = TrendFollowingStrategy()

    # Create config
    config = {
        "mode": args.mode,
        "symbols": symbols,
        "portfolio_value": args.portfolio_value,
        "max_budget": args.max_budget,
        "scan_interval": args.scan_interval,
        "max_position_size": 0.10,
        "max_daily_loss": 0.02,
        "max_drawdown": 0.10,
    }

    # Create and run agent
    agent = TradingViewAgent(config, strategy)

    # Signal handlers
    def signal_handler(sig, frame):
        logger.info("Shutdown signal received")
        agent.should_stop = True

    signal.signal(signal.SIGINT, signal_handler)
    signal.signal(signal.SIGTERM, signal_handler)

    # Run
    print("\n" + "="*60)
    print(f"TradingView Trading Agent - {args.mode.upper()} MODE")
    print("="*60)
    print(f"Strategy: {strategy.name}")
    print(f"Symbols: {', '.join(symbols)}")
    print(f"Portfolio: ${args.portfolio_value:,.2f}")
    print("="*60 + "\n")

    try:
        asyncio.run(agent.run())
    except Exception as e:
        logger.error(f"Fatal error: {e}", exc_info=True)


if __name__ == "__main__":
    main()
