"""
Professional Trading Agent with AI Vision

Complete implementation of a trading agent that uses:
- AI vision for chart pattern recognition
- Custom trading tools for execution
- Risk management and position sizing
- Safety callbacks and compliance checks
- Multi-strategy support

This example demonstrates how to build a production-ready trading agent
using the Cua framework with advanced safety features.

⚠️  DISCLAIMER: This is for educational purposes only. Trading carries
significant financial risk. Test thoroughly in paper trading mode before
considering live trading. Not financial advice.

Usage:
    # Paper trading (safe testing)
    python trading_agent_example.py --mode paper

    # Live trading (requires setup and extreme caution)
    python trading_agent_example.py --mode live --confirm-risk
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

# Import our custom trading components
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
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)


class TradingAgentConfig:
    """Configuration for the trading agent."""

    def __init__(
        self,
        mode: TradingMode = TradingMode.PAPER,
        model: str = "anthropic/claude-sonnet-4-5",
        portfolio_value: float = 100000,
        risk_limits: Optional[RiskLimits] = None,
        symbols_watchlist: Optional[List[str]] = None,
        trading_strategy: str = "trend_following",
        max_trajectory_budget: float = 10.0,
        enable_vision: bool = True,
    ):
        self.mode = mode
        self.model = model
        self.portfolio_value = portfolio_value
        self.risk_limits = risk_limits or RiskLimits()
        self.symbols_watchlist = symbols_watchlist or ["AAPL", "MSFT", "GOOGL", "TSLA", "NVDA"]
        self.trading_strategy = trading_strategy
        self.max_trajectory_budget = max_trajectory_budget
        self.enable_vision = enable_vision


class TradingAgent:
    """
    Professional trading agent with AI vision and safety features.

    This agent can:
    - Analyze charts using AI vision
    - Make informed trading decisions
    - Execute trades with proper risk management
    - Monitor performance and compliance
    - React to market conditions in real-time
    """

    def __init__(self, config: TradingAgentConfig):
        self.config = config
        self.is_running = False
        self.should_stop = False

        # Initialize tools
        self.market_data = MarketDataTool(mode=config.mode)
        self.risk_manager = RiskManagerTool(limits=config.risk_limits)
        self.executor = TradeExecutorTool(mode=config.mode)
        self.analyzer = PortfolioAnalyzerTool()

        # Initialize callbacks
        self.kill_switch = KillSwitchCallback(
            max_daily_loss=config.risk_limits.max_daily_loss,
            max_drawdown=config.risk_limits.max_drawdown,
            max_losing_streak=5
        )

        self.audit_trail = AuditTrailCallback(
            save_dir="./trading_audit",
            save_screenshots=True
        )

        self.compliance = ComplianceCallback(
            account_value=config.portfolio_value,
            account_type="margin"
        )

        self.monitor = PerformanceMonitorCallback()

        # Initialize computer if vision is enabled
        self.computer: Optional[Computer] = None

        # Initialize agent
        self.agent: Optional[ComputerAgent] = None

        logger.info(f"Trading agent initialized in {config.mode.value} mode")

    async def initialize(self):
        """Initialize the agent and computer environment."""

        # Set up computer for vision-based trading
        if self.config.enable_vision:
            logger.info("Initializing computer environment for vision...")
            self.computer = Computer(
                os_type="macos",  # or "linux", "windows"
                verbosity=logging.INFO,
            )
            await self.computer.run()

        # Create the AI agent
        tools = [self.market_data, self.risk_manager, self.executor, self.analyzer]
        if self.computer:
            tools.append(self.computer)

        self.agent = ComputerAgent(
            model=self.config.model,
            tools=tools,
            only_n_most_recent_images=3,
            trajectory_dir="./trading_trajectories",
            max_trajectory_budget=self.config.max_trajectory_budget,
            use_prompt_caching=True,
            verbosity=logging.INFO,
        )

        logger.info("Trading agent initialized successfully")

    async def analyze_chart_with_vision(self, symbol: str) -> Dict[str, Any]:
        """
        Use AI vision to analyze a chart.

        This method:
        1. Opens trading platform (TradingView, broker platform, etc.)
        2. Loads the chart for the symbol
        3. Uses vision to analyze patterns, support/resistance, indicators
        4. Returns structured analysis

        Returns:
            Dict with: pattern, trend, support_levels, resistance_levels, signals
        """
        if not self.computer or not self.agent:
            raise RuntimeError("Vision not enabled or agent not initialized")

        logger.info(f"Analyzing chart for {symbol} with AI vision...")

        # System prompt for chart analysis
        analysis_prompt = f"""
        You are a professional technical analyst. Analyze the chart for {symbol} and provide:

        1. **Overall Trend**: Identify if the trend is bullish, bearish, or neutral
        2. **Chart Patterns**: Identify any patterns (head and shoulders, triangles, flags, double tops/bottoms, etc.)
        3. **Support/Resistance**: Key support and resistance levels
        4. **Technical Indicators**: Analyze RSI, MACD, moving averages, volume
        5. **Trade Setup**: Is there a good entry opportunity? If so, specify entry, stop-loss, and target

        Be specific and provide price levels where applicable.

        Current task: Take a screenshot of the {symbol} chart and analyze it.
        """

        history = [{"role": "user", "content": analysis_prompt}]

        analysis_result = {}

        async for result in self.agent.run(history, stream=False):
            for item in result.get("output", []):
                if item.get("type") == "message":
                    content = item.get("content", [])
                    for content_part in content:
                        if content_part.get("text"):
                            analysis_text = content_part.get("text")
                            analysis_result = {
                                "symbol": symbol,
                                "analysis": analysis_text,
                                "timestamp": datetime.now().isoformat()
                            }

        await self.audit_trail.log_decision(
            decision_type="chart_analysis",
            reasoning=analysis_result.get("analysis", ""),
            data={"symbol": symbol},
            screenshot=None  # Would capture screenshot here
        )

        return analysis_result

    async def evaluate_trade_opportunity(
        self,
        symbol: str,
        analysis: Optional[Dict[str, Any]] = None
    ) -> Dict[str, Any]:
        """
        Evaluate if there's a valid trade opportunity.

        Combines:
        - AI vision analysis (if available)
        - Technical indicators
        - Market data
        - Risk assessment

        Returns:
            Dict with: should_trade, side, entry_price, stop_loss, target, reasoning
        """
        logger.info(f"Evaluating trade opportunity for {symbol}...")

        # Get market data
        quote = await self.market_data.get_quote(symbol)
        indicators = await self.market_data.get_technical_indicators(
            symbol,
            ["RSI_14", "MACD", "EMA_20", "SMA_50"]
        )

        # Simple trend-following logic (replace with AI decision)
        current_price = quote["price"]
        rsi = indicators["RSI_14"]
        ema_20 = indicators["EMA_20"]

        should_trade = False
        side = None
        reasoning = ""

        # Simple strategy: buy on RSI oversold + price above EMA
        if rsi < 35 and current_price > ema_20:
            should_trade = True
            side = OrderSide.BUY
            reasoning = f"RSI oversold ({rsi:.1f}) with price above EMA20, potential bounce"

        # Simple strategy: sell on RSI overbought
        elif rsi > 70:
            should_trade = True
            side = OrderSide.SELL
            reasoning = f"RSI overbought ({rsi:.1f}), potential pullback"

        else:
            reasoning = f"No clear setup: RSI={rsi:.1f}, Price vs EMA20={current_price:.2f} vs {ema_20:.2f}"

        if should_trade:
            # Calculate entry and stop loss
            entry_price = current_price
            stop_loss_pct = 0.02  # 2% stop loss

            if side == OrderSide.BUY:
                stop_loss = entry_price * (1 - stop_loss_pct)
                target = entry_price * 1.04  # 2:1 risk/reward
            else:
                stop_loss = entry_price * (1 + stop_loss_pct)
                target = entry_price * 0.96

            return {
                "should_trade": True,
                "symbol": symbol,
                "side": side,
                "entry_price": entry_price,
                "stop_loss": stop_loss,
                "target": target,
                "reasoning": reasoning,
                "indicators": indicators,
            }

        return {
            "should_trade": False,
            "symbol": symbol,
            "reasoning": reasoning,
        }

    async def execute_trade(self, opportunity: Dict[str, Any]) -> Dict[str, Any]:
        """
        Execute a validated trade with full risk management.

        Steps:
        1. Calculate position size
        2. Validate against risk limits
        3. Check compliance
        4. Execute order
        5. Log for audit
        """
        symbol = opportunity["symbol"]
        side = opportunity["side"]
        entry_price = opportunity["entry_price"]
        stop_loss = opportunity["stop_loss"]

        logger.info(f"Executing trade: {side.value} {symbol} @ ${entry_price:.2f}")

        # 1. Calculate position size
        position_calc = await self.risk_manager.calculate_position_size(
            symbol=symbol,
            entry_price=entry_price,
            stop_loss_price=stop_loss,
            portfolio_value=self.config.portfolio_value,
            risk_per_trade=0.01  # 1% risk per trade
        )

        quantity = position_calc["shares"]

        # 2. Validate trade
        positions = await self.executor.get_positions()
        validation = await self.risk_manager.validate_trade(
            symbol=symbol,
            side=side,
            quantity=quantity,
            price=entry_price,
            stop_loss=stop_loss,
            portfolio_value=self.config.portfolio_value,
            current_positions=positions
        )

        if not validation["approved"]:
            logger.warning(f"Trade rejected: {validation['reason']}")
            return {
                "success": False,
                "reason": validation["reason"]
            }

        # 3. Check compliance
        compliance_context = {
            "symbol": symbol,
            "side": side.value,
            "is_day_trade": False  # Would need logic to detect day trades
        }
        try:
            compliance_context = await self.compliance.on_before_trade(compliance_context)
        except RuntimeError as e:
            logger.error(f"Compliance check failed: {e}")
            return {"success": False, "reason": str(e)}

        # 4. Execute order
        order = await self.executor.submit_order(
            symbol=symbol,
            side=side,
            quantity=quantity,
            order_type=OrderType.MARKET
        )

        # 5. Log trade
        await self.audit_trail.log_trade(
            symbol=symbol,
            side=side.value,
            quantity=quantity,
            price=order["filled_avg_price"],
            order_id=order["order_id"],
            reasoning=opportunity["reasoning"]
        )

        logger.info(f"✅ Trade executed: {order['order_id']}")

        return {
            "success": True,
            "order": order,
            "position_size": position_calc,
            "validation": validation
        }

    async def scan_watchlist(self) -> List[Dict[str, Any]]:
        """
        Scan watchlist for trade opportunities.

        For each symbol:
        1. Check market data
        2. Optionally analyze chart with vision
        3. Evaluate trade opportunity
        """
        logger.info(f"Scanning watchlist: {self.config.symbols_watchlist}")

        opportunities = []

        for symbol in self.config.symbols_watchlist:
            try:
                # Get market data
                quote = await self.market_data.get_quote(symbol)

                # Optionally use vision for chart analysis
                # (Disabled by default to save API costs)
                # analysis = await self.analyze_chart_with_vision(symbol)

                # Evaluate opportunity
                opportunity = await self.evaluate_trade_opportunity(symbol)

                if opportunity["should_trade"]:
                    opportunities.append(opportunity)
                    logger.info(f"📈 Opportunity found: {symbol} - {opportunity['reasoning']}")

                await asyncio.sleep(0.5)  # Rate limiting

            except Exception as e:
                logger.error(f"Error scanning {symbol}: {e}")

        return opportunities

    async def trading_loop(self):
        """
        Main trading loop.

        Continuously:
        1. Check market hours
        2. Scan for opportunities
        3. Execute trades
        4. Monitor positions
        5. Update performance
        """
        logger.info("Starting trading loop...")
        self.is_running = True

        while not self.should_stop:
            try:
                # Check if market is open
                if not await self.market_data.is_market_open():
                    logger.info("Market closed, waiting...")
                    await asyncio.sleep(60)
                    continue

                # Check kill switch
                if self.kill_switch.is_halted:
                    logger.critical("Trading halted by kill switch!")
                    break

                # Scan for opportunities
                opportunities = await self.scan_watchlist()

                # Execute trades
                for opp in opportunities:
                    result = await self.execute_trade(opp)

                    if result["success"]:
                        # Update performance tracking
                        self.monitor.update_performance({
                            "symbol": opp["symbol"],
                            "pnl": 0,  # Would be updated on position close
                            "portfolio_value": self.config.portfolio_value
                        })

                # Monitor existing positions
                positions = await self.executor.get_positions()
                logger.info(f"Current positions: {len(positions)}")

                # Print performance summary
                self.monitor.print_summary()

                # Wait before next scan
                await asyncio.sleep(30)  # Scan every 30 seconds

            except Exception as e:
                logger.error(f"Error in trading loop: {e}")
                await asyncio.sleep(10)

        self.is_running = False
        logger.info("Trading loop stopped")

    async def run(self):
        """Run the trading agent."""
        try:
            await self.initialize()

            # Reset daily limits
            self.kill_switch.reset_daily_pnl()

            # Start trading loop
            await self.trading_loop()

        except KeyboardInterrupt:
            logger.info("Received interrupt signal, shutting down...")
            self.should_stop = True

        finally:
            await self.shutdown()

    async def shutdown(self):
        """Clean shutdown of the trading agent."""
        logger.info("Shutting down trading agent...")

        # Generate final reports
        await self.audit_trail.on_session_end()

        # Close all positions (optional, configurable)
        # positions = await self.executor.get_positions()
        # for position in positions:
        #     await self.executor.close_position(position.symbol)

        # Stop computer
        if self.computer:
            await self.computer.stop()

        logger.info("Trading agent shutdown complete")


def parse_args():
    """Parse command line arguments."""
    parser = argparse.ArgumentParser(description="Professional Trading Agent with AI Vision")

    parser.add_argument(
        "--mode",
        type=str,
        default="paper",
        choices=["paper", "live", "backtest", "monitor"],
        help="Trading mode (default: paper)"
    )

    parser.add_argument(
        "--model",
        type=str,
        default="anthropic/claude-sonnet-4-5",
        help="AI model to use (default: claude-sonnet-4-5)"
    )

    parser.add_argument(
        "--portfolio-value",
        type=float,
        default=100000,
        help="Initial portfolio value (default: 100000)"
    )

    parser.add_argument(
        "--max-budget",
        type=float,
        default=10.0,
        help="Maximum API budget in USD (default: 10.0)"
    )

    parser.add_argument(
        "--no-vision",
        action="store_true",
        help="Disable AI vision (use indicators only)"
    )

    parser.add_argument(
        "--confirm-risk",
        action="store_true",
        help="Confirm understanding of trading risks (required for live mode)"
    )

    return parser.parse_args()


def main():
    """Main entry point."""
    args = parse_args()

    # Safety check for live trading
    if args.mode == "live" and not args.confirm_risk:
        print("\n⚠️  ERROR: Live trading requires explicit risk confirmation")
        print("    Use --confirm-risk flag to acknowledge trading risks")
        print("    Remember: Only trade with money you can afford to lose\n")
        return

    if args.mode == "live":
        print("\n" + "="*60)
        print("⚠️  WARNING: LIVE TRADING MODE")
        print("="*60)
        print("You are about to run the agent in LIVE trading mode.")
        print("Real money will be at risk. Make sure you have:")
        print("  1. Thoroughly tested in paper trading")
        print("  2. Configured broker API credentials")
        print("  3. Set appropriate risk limits")
        print("  4. Monitored the agent in real-time")
        print("="*60)
        confirm = input("Type 'I UNDERSTAND THE RISKS' to continue: ")
        if confirm != "I UNDERSTAND THE RISKS":
            print("Aborting.")
            return

    # Create configuration
    config = TradingAgentConfig(
        mode=TradingMode(args.mode),
        model=args.model,
        portfolio_value=args.portfolio_value,
        max_trajectory_budget=args.max_budget,
        enable_vision=not args.no_vision
    )

    # Create and run agent
    agent = TradingAgent(config)

    # Set up signal handlers
    def signal_handler(sig, frame):
        logger.info("Received shutdown signal")
        agent.should_stop = True

    signal.signal(signal.SIGINT, signal_handler)
    signal.signal(signal.SIGTERM, signal_handler)

    # Run agent
    try:
        asyncio.run(agent.run())
    except Exception as e:
        logger.error(f"Fatal error: {e}")
        raise


if __name__ == "__main__":
    main()
