"""
Custom Trading Tools for Cua Agent Framework

This module provides specialized tools for building professional trading agents:
- MarketDataTool: Real-time and historical market data
- RiskManagerTool: Position sizing and risk calculations
- TradeExecutorTool: Order execution and management
- TechnicalAnalysisTool: Pre-computed technical indicators
- PortfolioAnalyzerTool: Position tracking and P&L analysis

Example usage:
    from trading_tools import MarketDataTool, TradeExecutorTool, RiskManagerTool
    from agent import ComputerAgent

    market_data = MarketDataTool(api_key="your_key", mode="paper")
    risk_manager = RiskManagerTool(max_position_size=0.1, max_daily_loss=0.02)
    executor = TradeExecutorTool(broker="alpaca", mode="paper", api_key="your_key")

    agent = ComputerAgent(
        model="anthropic/claude-sonnet-4-5",
        tools=[market_data, risk_manager, executor],
        max_trajectory_budget=10.0
    )
"""

import asyncio
import logging
from dataclasses import dataclass
from datetime import datetime, timedelta
from enum import Enum
from typing import Any, Dict, List, Optional

logger = logging.getLogger(__name__)


class TradingMode(Enum):
    """Operating modes for the trading agent."""
    PAPER = "paper"  # Paper trading (simulated)
    LIVE = "live"    # Live trading with real money
    BACKTEST = "backtest"  # Historical simulation
    MONITOR = "monitor"  # Monitor only, no execution


class OrderSide(Enum):
    """Order side (buy or sell)."""
    BUY = "buy"
    SELL = "sell"


class OrderType(Enum):
    """Order types supported."""
    MARKET = "market"
    LIMIT = "limit"
    STOP = "stop"
    STOP_LIMIT = "stop_limit"


@dataclass
class Position:
    """Represents a trading position."""
    symbol: str
    quantity: float
    entry_price: float
    current_price: float
    side: OrderSide
    stop_loss: Optional[float] = None
    take_profit: Optional[float] = None

    @property
    def market_value(self) -> float:
        """Current market value of position."""
        return self.quantity * self.current_price

    @property
    def unrealized_pnl(self) -> float:
        """Unrealized profit/loss."""
        if self.side == OrderSide.BUY:
            return (self.current_price - self.entry_price) * self.quantity
        else:
            return (self.entry_price - self.current_price) * self.quantity

    @property
    def unrealized_pnl_percent(self) -> float:
        """Unrealized P&L as percentage."""
        if self.side == OrderSide.BUY:
            return ((self.current_price - self.entry_price) / self.entry_price) * 100
        else:
            return ((self.entry_price - self.current_price) / self.entry_price) * 100


@dataclass
class RiskLimits:
    """Risk management parameters."""
    max_position_size: float = 0.10  # Max 10% per position
    max_daily_loss: float = 0.02     # Max 2% daily loss
    max_drawdown: float = 0.10       # Max 10% drawdown
    max_positions: int = 10           # Max concurrent positions
    require_stop_loss: bool = True    # Require stop-loss on all trades
    max_portfolio_leverage: float = 1.0  # No leverage by default


class MarketDataTool:
    """
    Tool for fetching real-time and historical market data.

    This tool provides the agent with market data capabilities including:
    - Real-time price quotes
    - Historical OHLCV data
    - Pre-computed technical indicators
    - Market status checks

    Integrates with: Alpaca, Polygon.io, Alpha Vantage, Yahoo Finance
    """

    def __init__(
        self,
        provider: str = "alpaca",
        api_key: Optional[str] = None,
        mode: TradingMode = TradingMode.PAPER
    ):
        self.provider = provider
        self.api_key = api_key
        self.mode = mode
        self.name = "market_data"
        self.description = "Get real-time market data, historical prices, and technical indicators"

    async def get_quote(self, symbol: str) -> Dict[str, Any]:
        """
        Get real-time quote for a symbol.

        Args:
            symbol: Stock symbol (e.g., "AAPL", "TSLA")

        Returns:
            Dict with: price, bid, ask, volume, timestamp
        """
        # TODO: Implement actual API integration
        # For now, return mock data structure
        return {
            "symbol": symbol,
            "price": 150.50,
            "bid": 150.48,
            "ask": 150.52,
            "bid_size": 100,
            "ask_size": 200,
            "volume": 1500000,
            "timestamp": datetime.now().isoformat(),
            "change": 2.5,
            "change_percent": 1.69,
        }

    async def get_historical_data(
        self,
        symbol: str,
        timeframe: str = "1D",
        start: Optional[str] = None,
        end: Optional[str] = None,
        limit: int = 100
    ) -> List[Dict[str, Any]]:
        """
        Get historical OHLCV data.

        Args:
            symbol: Stock symbol
            timeframe: "1m", "5m", "15m", "1h", "1D", etc.
            start: Start date (ISO format)
            end: End date (ISO format)
            limit: Number of bars to return

        Returns:
            List of OHLCV bars
        """
        # TODO: Implement actual API integration
        return []

    async def get_technical_indicators(
        self,
        symbol: str,
        indicators: List[str]
    ) -> Dict[str, Any]:
        """
        Get pre-computed technical indicators.

        Args:
            symbol: Stock symbol
            indicators: List of indicators ["RSI", "MACD", "BB", "EMA_20", etc.]

        Returns:
            Dict of indicator values
        """
        # TODO: Implement technical analysis calculations
        return {
            "symbol": symbol,
            "RSI_14": 65.5,
            "MACD": {"value": 1.2, "signal": 0.8, "histogram": 0.4},
            "BB_upper": 155.0,
            "BB_middle": 150.0,
            "BB_lower": 145.0,
            "EMA_20": 149.5,
            "SMA_50": 148.0,
            "volume_SMA_20": 1200000,
        }

    async def is_market_open(self) -> bool:
        """Check if market is currently open."""
        # TODO: Implement actual market hours check
        now = datetime.now()
        return 9 <= now.hour < 16  # Simple 9am-4pm check

    def get_tool_schema(self) -> Dict[str, Any]:
        """Return tool schema for agent integration."""
        return {
            "name": self.name,
            "description": self.description,
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["get_quote", "get_historical", "get_indicators", "check_market_open"],
                        "description": "Action to perform"
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Stock symbol (e.g., AAPL)"
                    },
                    "timeframe": {
                        "type": "string",
                        "description": "Timeframe for historical data"
                    },
                    "indicators": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "List of technical indicators"
                    }
                },
                "required": ["action"]
            }
        }


class RiskManagerTool:
    """
    Tool for risk management and position sizing calculations.

    This tool helps the agent make safe trading decisions by:
    - Calculating appropriate position sizes
    - Validating trades against risk limits
    - Computing stop-loss and take-profit levels
    - Checking portfolio exposure
    """

    def __init__(self, limits: Optional[RiskLimits] = None):
        self.limits = limits or RiskLimits()
        self.name = "risk_manager"
        self.description = "Calculate position sizes and validate trades against risk limits"
        self.daily_pnl = 0.0
        self.starting_portfolio_value = 100000.0  # Default

    async def calculate_position_size(
        self,
        symbol: str,
        entry_price: float,
        stop_loss_price: float,
        portfolio_value: float,
        risk_per_trade: float = 0.01  # 1% risk per trade
    ) -> Dict[str, Any]:
        """
        Calculate appropriate position size based on risk parameters.

        Args:
            symbol: Stock symbol
            entry_price: Planned entry price
            stop_loss_price: Stop loss price
            portfolio_value: Current portfolio value
            risk_per_trade: Maximum risk as fraction of portfolio (default 1%)

        Returns:
            Dict with: shares, position_value, risk_amount, risk_per_share
        """
        risk_per_share = abs(entry_price - stop_loss_price)
        max_risk_amount = portfolio_value * risk_per_trade

        shares = int(max_risk_amount / risk_per_share)
        position_value = shares * entry_price

        # Check against max position size limit
        max_position_value = portfolio_value * self.limits.max_position_size
        if position_value > max_position_value:
            shares = int(max_position_value / entry_price)
            position_value = shares * entry_price

        return {
            "symbol": symbol,
            "shares": shares,
            "position_value": position_value,
            "position_size_percent": (position_value / portfolio_value) * 100,
            "risk_amount": shares * risk_per_share,
            "risk_percent": (shares * risk_per_share / portfolio_value) * 100,
            "entry_price": entry_price,
            "stop_loss_price": stop_loss_price,
        }

    async def validate_trade(
        self,
        symbol: str,
        side: OrderSide,
        quantity: int,
        price: float,
        stop_loss: Optional[float],
        portfolio_value: float,
        current_positions: List[Position]
    ) -> Dict[str, Any]:
        """
        Validate a proposed trade against risk limits.

        Returns:
            Dict with: approved (bool), reason (str), warnings (List[str])
        """
        warnings = []

        # Check if stop-loss is required
        if self.limits.require_stop_loss and stop_loss is None:
            return {
                "approved": False,
                "reason": "Stop-loss is required but not provided",
                "warnings": warnings
            }

        # Check position size limit
        position_value = quantity * price
        position_size_pct = position_value / portfolio_value
        if position_size_pct > self.limits.max_position_size:
            return {
                "approved": False,
                "reason": f"Position size {position_size_pct:.1%} exceeds limit {self.limits.max_position_size:.1%}",
                "warnings": warnings
            }

        # Check max positions limit
        if len(current_positions) >= self.limits.max_positions:
            return {
                "approved": False,
                "reason": f"Already at maximum position limit ({self.limits.max_positions})",
                "warnings": warnings
            }

        # Check daily loss limit
        daily_loss_pct = abs(self.daily_pnl) / self.starting_portfolio_value
        if self.daily_pnl < 0 and daily_loss_pct >= self.limits.max_daily_loss:
            return {
                "approved": False,
                "reason": f"Daily loss limit reached ({daily_loss_pct:.1%})",
                "warnings": warnings
            }

        # Add warnings for risky but acceptable trades
        if position_size_pct > 0.05:  # > 5%
            warnings.append(f"Large position size: {position_size_pct:.1%}")

        if stop_loss:
            risk_pct = abs(price - stop_loss) / price
            if risk_pct > 0.05:  # > 5% risk per trade
                warnings.append(f"High risk per trade: {risk_pct:.1%}")

        return {
            "approved": True,
            "reason": "Trade validated successfully",
            "warnings": warnings,
            "position_size_percent": position_size_pct * 100,
            "risk_details": {
                "daily_pnl": self.daily_pnl,
                "daily_loss_pct": daily_loss_pct * 100,
                "current_positions": len(current_positions),
            }
        }

    def update_daily_pnl(self, pnl: float):
        """Update daily P&L tracking."""
        self.daily_pnl += pnl

    def reset_daily_pnl(self):
        """Reset daily P&L (call at start of trading day)."""
        self.daily_pnl = 0.0

    def get_tool_schema(self) -> Dict[str, Any]:
        """Return tool schema for agent integration."""
        return {
            "name": self.name,
            "description": self.description,
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["calculate_position_size", "validate_trade"],
                        "description": "Risk management action"
                    },
                    "symbol": {"type": "string"},
                    "entry_price": {"type": "number"},
                    "stop_loss_price": {"type": "number"},
                    "quantity": {"type": "integer"},
                    "side": {"type": "string", "enum": ["buy", "sell"]},
                },
                "required": ["action"]
            }
        }


class TradeExecutorTool:
    """
    Tool for executing and managing trades.

    This tool handles:
    - Order submission (market, limit, stop)
    - Order status monitoring
    - Position management
    - Paper trading simulation
    """

    def __init__(
        self,
        broker: str = "alpaca",
        mode: TradingMode = TradingMode.PAPER,
        api_key: Optional[str] = None,
        api_secret: Optional[str] = None,
    ):
        self.broker = broker
        self.mode = mode
        self.api_key = api_key
        self.api_secret = api_secret
        self.name = "trade_executor"
        self.description = "Submit orders and manage positions"

        # Paper trading simulation
        self.paper_positions: Dict[str, Position] = {}
        self.paper_orders: List[Dict[str, Any]] = []
        self.order_id_counter = 1000

    async def submit_order(
        self,
        symbol: str,
        side: OrderSide,
        quantity: int,
        order_type: OrderType = OrderType.MARKET,
        limit_price: Optional[float] = None,
        stop_price: Optional[float] = None,
        time_in_force: str = "day"
    ) -> Dict[str, Any]:
        """
        Submit a trading order.

        Args:
            symbol: Stock symbol
            side: BUY or SELL
            quantity: Number of shares
            order_type: MARKET, LIMIT, STOP, or STOP_LIMIT
            limit_price: Limit price (for limit orders)
            stop_price: Stop price (for stop orders)
            time_in_force: "day", "gtc", "ioc", "fok"

        Returns:
            Dict with order details: order_id, status, filled_qty, filled_avg_price
        """
        if self.mode == TradingMode.PAPER:
            # Simulate order in paper trading mode
            order_id = f"paper_{self.order_id_counter}"
            self.order_id_counter += 1

            order = {
                "order_id": order_id,
                "symbol": symbol,
                "side": side.value,
                "quantity": quantity,
                "order_type": order_type.value,
                "status": "filled",  # Instant fill for paper trading
                "filled_qty": quantity,
                "filled_avg_price": limit_price or 150.0,  # Mock price
                "submitted_at": datetime.now().isoformat(),
                "filled_at": datetime.now().isoformat(),
            }

            self.paper_orders.append(order)

            logger.info(f"Paper trade executed: {side.value} {quantity} {symbol} @ {order['filled_avg_price']}")

            return order

        elif self.mode == TradingMode.LIVE:
            # TODO: Implement actual broker API integration
            logger.warning("Live trading not yet implemented!")
            raise NotImplementedError("Live trading requires broker API integration")

        else:
            raise ValueError(f"Unsupported mode: {self.mode}")

    async def cancel_order(self, order_id: str) -> Dict[str, Any]:
        """Cancel an existing order."""
        # TODO: Implement order cancellation
        return {"order_id": order_id, "status": "cancelled"}

    async def get_order_status(self, order_id: str) -> Dict[str, Any]:
        """Get status of an order."""
        if self.mode == TradingMode.PAPER:
            for order in self.paper_orders:
                if order["order_id"] == order_id:
                    return order
            return {"error": "Order not found"}

        # TODO: Implement for live mode
        return {}

    async def get_positions(self) -> List[Position]:
        """Get all current positions."""
        if self.mode == TradingMode.PAPER:
            return list(self.paper_positions.values())

        # TODO: Implement for live mode
        return []

    async def close_position(self, symbol: str) -> Dict[str, Any]:
        """Close an existing position."""
        if self.mode == TradingMode.PAPER:
            if symbol in self.paper_positions:
                position = self.paper_positions[symbol]
                opposite_side = OrderSide.SELL if position.side == OrderSide.BUY else OrderSide.BUY

                result = await self.submit_order(
                    symbol=symbol,
                    side=opposite_side,
                    quantity=int(position.quantity),
                    order_type=OrderType.MARKET
                )

                del self.paper_positions[symbol]
                return result
            else:
                return {"error": f"No position found for {symbol}"}

        # TODO: Implement for live mode
        return {}

    def get_tool_schema(self) -> Dict[str, Any]:
        """Return tool schema for agent integration."""
        return {
            "name": self.name,
            "description": self.description,
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["submit_order", "cancel_order", "get_order_status", "get_positions", "close_position"],
                        "description": "Trading action to perform"
                    },
                    "symbol": {"type": "string"},
                    "side": {"type": "string", "enum": ["buy", "sell"]},
                    "quantity": {"type": "integer"},
                    "order_type": {"type": "string", "enum": ["market", "limit", "stop", "stop_limit"]},
                    "limit_price": {"type": "number"},
                    "stop_price": {"type": "number"},
                    "order_id": {"type": "string"},
                },
                "required": ["action"]
            }
        }


class PortfolioAnalyzerTool:
    """
    Tool for portfolio analysis and performance tracking.

    Provides:
    - Real-time P&L calculation
    - Performance metrics (Sharpe, win rate, etc.)
    - Position concentration analysis
    - Trade history analytics
    """

    def __init__(self):
        self.name = "portfolio_analyzer"
        self.description = "Analyze portfolio performance and risk metrics"
        self.trade_history: List[Dict[str, Any]] = []

    async def get_portfolio_summary(
        self,
        positions: List[Position],
        cash_balance: float
    ) -> Dict[str, Any]:
        """
        Get comprehensive portfolio summary.

        Returns:
            Dict with: total_value, positions_value, cash, unrealized_pnl, etc.
        """
        positions_value = sum(p.market_value for p in positions)
        total_value = positions_value + cash_balance
        unrealized_pnl = sum(p.unrealized_pnl for p in positions)

        return {
            "total_value": total_value,
            "positions_value": positions_value,
            "cash_balance": cash_balance,
            "unrealized_pnl": unrealized_pnl,
            "unrealized_pnl_percent": (unrealized_pnl / total_value * 100) if total_value > 0 else 0,
            "num_positions": len(positions),
            "largest_position": max((p.market_value for p in positions), default=0),
            "positions": [
                {
                    "symbol": p.symbol,
                    "quantity": p.quantity,
                    "market_value": p.market_value,
                    "unrealized_pnl": p.unrealized_pnl,
                    "unrealized_pnl_percent": p.unrealized_pnl_percent,
                }
                for p in positions
            ]
        }

    async def calculate_performance_metrics(
        self,
        equity_curve: List[float],
        trades: List[Dict[str, Any]]
    ) -> Dict[str, Any]:
        """
        Calculate performance metrics from equity curve and trade history.

        Returns:
            Dict with: total_return, sharpe_ratio, max_drawdown, win_rate, etc.
        """
        if not equity_curve or len(equity_curve) < 2:
            return {"error": "Insufficient data for performance calculation"}

        # Calculate returns
        returns = [(equity_curve[i] - equity_curve[i-1]) / equity_curve[i-1]
                   for i in range(1, len(equity_curve))]

        total_return = (equity_curve[-1] - equity_curve[0]) / equity_curve[0]

        # Calculate Sharpe ratio (simplified, assuming daily returns)
        if returns:
            import statistics
            avg_return = statistics.mean(returns)
            std_return = statistics.stdev(returns) if len(returns) > 1 else 0
            sharpe_ratio = (avg_return / std_return * (252 ** 0.5)) if std_return > 0 else 0
        else:
            sharpe_ratio = 0

        # Calculate max drawdown
        peak = equity_curve[0]
        max_dd = 0
        for value in equity_curve:
            if value > peak:
                peak = value
            dd = (peak - value) / peak
            max_dd = max(max_dd, dd)

        # Calculate trade statistics
        winning_trades = [t for t in trades if t.get("pnl", 0) > 0]
        losing_trades = [t for t in trades if t.get("pnl", 0) < 0]

        win_rate = len(winning_trades) / len(trades) if trades else 0
        avg_win = sum(t["pnl"] for t in winning_trades) / len(winning_trades) if winning_trades else 0
        avg_loss = sum(t["pnl"] for t in losing_trades) / len(losing_trades) if losing_trades else 0

        return {
            "total_return": total_return * 100,
            "sharpe_ratio": sharpe_ratio,
            "max_drawdown": max_dd * 100,
            "num_trades": len(trades),
            "winning_trades": len(winning_trades),
            "losing_trades": len(losing_trades),
            "win_rate": win_rate * 100,
            "avg_win": avg_win,
            "avg_loss": avg_loss,
            "profit_factor": abs(avg_win / avg_loss) if avg_loss != 0 else 0,
        }

    def get_tool_schema(self) -> Dict[str, Any]:
        """Return tool schema for agent integration."""
        return {
            "name": self.name,
            "description": self.description,
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["get_portfolio_summary", "calculate_performance"],
                        "description": "Analysis action to perform"
                    }
                },
                "required": ["action"]
            }
        }


# Example usage and testing
async def example_usage():
    """Example showing how to use the trading tools."""

    print("\n=== Trading Tools Example ===\n")

    # Initialize tools
    market_data = MarketDataTool(mode=TradingMode.PAPER)
    risk_manager = RiskManagerTool()
    executor = TradeExecutorTool(mode=TradingMode.PAPER)
    analyzer = PortfolioAnalyzerTool()

    # 1. Get market data
    print("1. Fetching market data...")
    quote = await market_data.get_quote("AAPL")
    print(f"   AAPL: ${quote['price']} (${quote['change']:+.2f}, {quote['change_percent']:+.2f}%)")

    indicators = await market_data.get_technical_indicators("AAPL", ["RSI_14", "MACD"])
    print(f"   RSI: {indicators['RSI_14']:.1f}")
    print(f"   MACD: {indicators['MACD']['value']:.2f}")

    # 2. Calculate position size
    print("\n2. Calculating position size...")
    portfolio_value = 100000
    entry_price = quote['price']
    stop_loss_price = entry_price * 0.98  # 2% stop loss

    position_calc = await risk_manager.calculate_position_size(
        symbol="AAPL",
        entry_price=entry_price,
        stop_loss_price=stop_loss_price,
        portfolio_value=portfolio_value,
        risk_per_trade=0.01  # 1% risk
    )
    print(f"   Recommended shares: {position_calc['shares']}")
    print(f"   Position value: ${position_calc['position_value']:,.2f}")
    print(f"   Risk amount: ${position_calc['risk_amount']:.2f}")

    # 3. Validate trade
    print("\n3. Validating trade...")
    validation = await risk_manager.validate_trade(
        symbol="AAPL",
        side=OrderSide.BUY,
        quantity=position_calc['shares'],
        price=entry_price,
        stop_loss=stop_loss_price,
        portfolio_value=portfolio_value,
        current_positions=[]
    )
    print(f"   Approved: {validation['approved']}")
    print(f"   Reason: {validation['reason']}")
    if validation['warnings']:
        print(f"   Warnings: {', '.join(validation['warnings'])}")

    # 4. Execute trade
    if validation['approved']:
        print("\n4. Executing trade...")
        order = await executor.submit_order(
            symbol="AAPL",
            side=OrderSide.BUY,
            quantity=position_calc['shares'],
            order_type=OrderType.MARKET
        )
        print(f"   Order ID: {order['order_id']}")
        print(f"   Status: {order['status']}")
        print(f"   Filled: {order['filled_qty']} @ ${order['filled_avg_price']}")

    # 5. Get portfolio summary
    print("\n5. Portfolio analysis...")
    positions = await executor.get_positions()
    summary = await analyzer.get_portfolio_summary(
        positions=positions,
        cash_balance=portfolio_value - position_calc['position_value']
    )
    print(f"   Total value: ${summary['total_value']:,.2f}")
    print(f"   Positions: {summary['num_positions']}")
    print(f"   Cash: ${summary['cash_balance']:,.2f}")

    print("\n=== Example Complete ===\n")


if __name__ == "__main__":
    # Run the example
    asyncio.run(example_usage())
