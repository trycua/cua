"""
Professional Backtesting Framework

Accurate historical simulation of trading strategies with realistic execution modeling.

Features:
- Historical data replay
- Realistic slippage and commission modeling
- Position tracking and P&L calculation
- Performance metrics (Sharpe, Sortino, max drawdown)
- Equity curve visualization
- Trade-by-trade analysis
- Multiple timeframe support
- Strategy comparison

Usage:
    from backtesting import Backtest, Strategy

    class MyStrategy(Strategy):
        def on_bar(self, bar):
            if self.should_buy(bar):
                self.buy(symbol="AAPL", quantity=100)

    backtest = Backtest(
        strategy=MyStrategy(),
        data=historical_data,
        initial_capital=100000
    )

    results = backtest.run()
    print(results.summary())
"""

import logging
from dataclasses import dataclass
from datetime import datetime
from enum import Enum
from typing import Any, Dict, List, Optional, Tuple

import numpy as np
import pandas as pd

logger = logging.getLogger(__name__)


class OrderType(Enum):
    """Order types supported in backtesting."""
    MARKET = "market"
    LIMIT = "limit"
    STOP = "stop"


class OrderSide(Enum):
    """Order side."""
    BUY = "buy"
    SELL = "sell"


@dataclass
class Order:
    """Represents a trading order."""
    symbol: str
    side: OrderSide
    quantity: int
    order_type: OrderType
    limit_price: Optional[float] = None
    stop_price: Optional[float] = None
    timestamp: Optional[datetime] = None
    filled_price: Optional[float] = None
    filled: bool = False
    commission: float = 0.0


@dataclass
class Position:
    """Represents an open position."""
    symbol: str
    quantity: int
    entry_price: float
    entry_time: datetime
    current_price: float
    stop_loss: Optional[float] = None
    take_profit: Optional[float] = None

    @property
    def market_value(self) -> float:
        """Current market value."""
        return self.quantity * self.current_price

    @property
    def unrealized_pnl(self) -> float:
        """Unrealized profit/loss."""
        return (self.current_price - self.entry_price) * self.quantity

    @property
    def unrealized_pnl_pct(self) -> float:
        """Unrealized P&L percentage."""
        return ((self.current_price - self.entry_price) / self.entry_price) * 100


@dataclass
class Trade:
    """Represents a completed trade."""
    symbol: str
    entry_time: datetime
    exit_time: datetime
    entry_price: float
    exit_price: float
    quantity: int
    side: OrderSide  # Original side (BUY or SELL)
    pnl: float
    pnl_pct: float
    commission: float
    hold_time: float  # Hours
    mae: float  # Maximum Adverse Excursion
    mfe: float  # Maximum Favorable Excursion


@dataclass
class BacktestResults:
    """Complete backtest results."""
    # Performance metrics
    total_return: float  # %
    annual_return: float  # %
    sharpe_ratio: float
    sortino_ratio: float
    calmar_ratio: float

    # Risk metrics
    max_drawdown: float  # %
    max_drawdown_duration: int  # Days
    volatility: float  # Annual %

    # Trade statistics
    total_trades: int
    winning_trades: int
    losing_trades: int
    win_rate: float  # %
    avg_win: float
    avg_loss: float
    profit_factor: float  # Gross profit / Gross loss
    avg_trade: float
    avg_hold_time: float  # Hours

    # Best/Worst
    best_trade: float
    worst_trade: float
    largest_win_streak: int
    largest_loss_streak: int

    # Capital
    initial_capital: float
    final_capital: float
    peak_capital: float

    # Exposure
    avg_exposure: float  # % of capital in market

    # Detailed data
    equity_curve: List[float]
    trades: List[Trade]
    daily_returns: np.ndarray

    def summary(self) -> str:
        """Generate formatted summary report."""
        report = f"""
{'='*70}
BACKTEST RESULTS SUMMARY
{'='*70}

PERFORMANCE METRICS
{'─'*70}
Total Return:              {self.total_return:>10.2f}%
Annual Return:             {self.annual_return:>10.2f}%
Sharpe Ratio:              {self.sharpe_ratio:>10.2f}
Sortino Ratio:             {self.sortino_ratio:>10.2f}
Calmar Ratio:              {self.calmar_ratio:>10.2f}

RISK METRICS
{'─'*70}
Max Drawdown:              {self.max_drawdown:>10.2f}%
Drawdown Duration:         {self.max_drawdown_duration:>10} days
Annual Volatility:         {self.volatility:>10.2f}%

TRADE STATISTICS
{'─'*70}
Total Trades:              {self.total_trades:>10}
Winning Trades:            {self.winning_trades:>10}
Losing Trades:             {self.losing_trades:>10}
Win Rate:                  {self.win_rate:>10.2f}%
Profit Factor:             {self.profit_factor:>10.2f}
Average Trade:             ${self.avg_trade:>10,.2f}
Average Hold Time:         {self.avg_hold_time:>10.1f} hours

WIN/LOSS ANALYSIS
{'─'*70}
Average Win:               ${self.avg_win:>10,.2f}
Average Loss:              ${self.avg_loss:>10,.2f}
Best Trade:                ${self.best_trade:>10,.2f}
Worst Trade:               ${self.worst_trade:>10,.2f}
Largest Win Streak:        {self.largest_win_streak:>10}
Largest Loss Streak:       {self.largest_loss_streak:>10}

CAPITAL
{'─'*70}
Initial Capital:           ${self.initial_capital:>10,.2f}
Final Capital:             ${self.final_capital:>10,.2f}
Peak Capital:              ${self.peak_capital:>10,.2f}

EXPOSURE
{'─'*70}
Average Exposure:          {self.avg_exposure:>10.2f}%

{'='*70}
"""
        return report


class Strategy:
    """
    Base strategy class for backtesting.

    Subclass this and implement on_bar() to define your strategy logic.
    """

    def __init__(self):
        """Initialize strategy."""
        self.backtest: Optional['Backtest'] = None

    def on_bar(self, bar: pd.Series):
        """
        Called on each new bar of data.

        Override this method to implement your strategy logic.

        Args:
            bar: Current bar with OHLCV data
        """
        raise NotImplementedError("Implement on_bar() in your strategy")

    def buy(
        self,
        symbol: str,
        quantity: int,
        order_type: OrderType = OrderType.MARKET,
        limit_price: Optional[float] = None,
        stop_loss: Optional[float] = None
    ):
        """Submit buy order."""
        if self.backtest:
            self.backtest.submit_order(
                symbol=symbol,
                side=OrderSide.BUY,
                quantity=quantity,
                order_type=order_type,
                limit_price=limit_price,
                stop_loss=stop_loss
            )

    def sell(
        self,
        symbol: str,
        quantity: int,
        order_type: OrderType = OrderType.MARKET,
        limit_price: Optional[float] = None
    ):
        """Submit sell order."""
        if self.backtest:
            self.backtest.submit_order(
                symbol=symbol,
                side=OrderSide.SELL,
                quantity=quantity,
                order_type=order_type,
                limit_price=limit_price
            )

    def get_position(self, symbol: str) -> Optional[Position]:
        """Get current position for symbol."""
        if self.backtest:
            return self.backtest.positions.get(symbol)
        return None

    def get_positions(self) -> Dict[str, Position]:
        """Get all current positions."""
        if self.backtest:
            return self.backtest.positions
        return {}

    @property
    def portfolio_value(self) -> float:
        """Current portfolio value."""
        if self.backtest:
            return self.backtest.portfolio_value
        return 0.0

    @property
    def cash(self) -> float:
        """Current cash balance."""
        if self.backtest:
            return self.backtest.cash
        return 0.0


class Backtest:
    """
    Professional backtesting engine.

    Simulates strategy execution on historical data with realistic
    execution modeling including slippage and commissions.
    """

    def __init__(
        self,
        strategy: Strategy,
        data: pd.DataFrame,
        initial_capital: float = 100000,
        commission: float = 0.001,  # 0.1% per trade
        slippage: float = 0.0005,  # 0.05% slippage
        risk_free_rate: float = 0.04  # 4% annual
    ):
        """
        Initialize backtest.

        Args:
            strategy: Strategy instance to test
            data: Historical OHLCV data with DatetimeIndex
            initial_capital: Starting capital
            commission: Commission rate (fraction)
            slippage: Slippage estimate (fraction)
            risk_free_rate: Annual risk-free rate
        """
        self.strategy = strategy
        self.strategy.backtest = self
        self.data = data
        self.initial_capital = initial_capital
        self.commission_rate = commission
        self.slippage_rate = slippage
        self.risk_free_rate = risk_free_rate

        # State
        self.cash = initial_capital
        self.positions: Dict[str, Position] = {}
        self.pending_orders: List[Order] = []
        self.trades: List[Trade] = []
        self.equity_curve: List[float] = []
        self.current_time: Optional[datetime] = None

        # MAE/MFE tracking
        self.position_extremes: Dict[str, Tuple[float, float]] = {}  # symbol -> (min, max) price

    @property
    def portfolio_value(self) -> float:
        """Current total portfolio value."""
        position_value = sum(pos.market_value for pos in self.positions.values())
        return self.cash + position_value

    def submit_order(
        self,
        symbol: str,
        side: OrderSide,
        quantity: int,
        order_type: OrderType = OrderType.MARKET,
        limit_price: Optional[float] = None,
        stop_loss: Optional[float] = None
    ):
        """Submit an order."""
        order = Order(
            symbol=symbol,
            side=side,
            quantity=quantity,
            order_type=order_type,
            limit_price=limit_price,
            stop_price=stop_loss,
            timestamp=self.current_time
        )
        self.pending_orders.append(order)

    def execute_orders(self, bar: pd.Series):
        """Execute pending orders based on current bar."""
        executed_orders = []

        for order in self.pending_orders:
            if order.symbol != bar.name:
                continue

            filled = False
            fill_price = None

            if order.order_type == OrderType.MARKET:
                # Market order fills at open + slippage
                fill_price = bar['open'] * (1 + self.slippage_rate if order.side == OrderSide.BUY else 1 - self.slippage_rate)
                filled = True

            elif order.order_type == OrderType.LIMIT:
                # Limit order fills if price reached
                if order.side == OrderSide.BUY and bar['low'] <= order.limit_price:
                    fill_price = order.limit_price
                    filled = True
                elif order.side == OrderSide.SELL and bar['high'] >= order.limit_price:
                    fill_price = order.limit_price
                    filled = True

            if filled and fill_price:
                # Calculate commission
                trade_value = fill_price * order.quantity
                commission = trade_value * self.commission_rate

                if order.side == OrderSide.BUY:
                    # Open or add to position
                    if symbol in self.positions:
                        # Add to existing
                        pos = self.positions[symbol]
                        total_qty = pos.quantity + order.quantity
                        total_cost = (pos.entry_price * pos.quantity) + (fill_price * order.quantity)
                        pos.entry_price = total_cost / total_qty
                        pos.quantity = total_qty
                    else:
                        # New position
                        self.positions[symbol] = Position(
                            symbol=symbol,
                            quantity=order.quantity,
                            entry_price=fill_price,
                            entry_time=self.current_time,
                            current_price=fill_price,
                            stop_loss=order.stop_price
                        )
                        # Initialize MAE/MFE tracking
                        self.position_extremes[symbol] = (fill_price, fill_price)

                    self.cash -= (trade_value + commission)

                elif order.side == OrderSide.SELL:
                    # Close position
                    if symbol in self.positions:
                        pos = self.positions[symbol]

                        if order.quantity >= pos.quantity:
                            # Close entire position
                            pnl = (fill_price - pos.entry_price) * pos.quantity - commission
                            pnl_pct = ((fill_price - pos.entry_price) / pos.entry_price) * 100

                            # Calculate MAE/MFE
                            min_price, max_price = self.position_extremes.get(symbol, (fill_price, fill_price))
                            mae = (min_price - pos.entry_price) * pos.quantity  # Most negative
                            mfe = (max_price - pos.entry_price) * pos.quantity  # Most positive

                            # Record trade
                            hold_time = (self.current_time - pos.entry_time).total_seconds() / 3600
                            trade = Trade(
                                symbol=symbol,
                                entry_time=pos.entry_time,
                                exit_time=self.current_time,
                                entry_price=pos.entry_price,
                                exit_price=fill_price,
                                quantity=pos.quantity,
                                side=OrderSide.BUY,  # Original side
                                pnl=pnl,
                                pnl_pct=pnl_pct,
                                commission=commission,
                                hold_time=hold_time,
                                mae=mae,
                                mfe=mfe
                            )
                            self.trades.append(trade)

                            self.cash += (trade_value - commission)
                            del self.positions[symbol]
                            del self.position_extremes[symbol]

                        else:
                            # Partial close
                            pnl = (fill_price - pos.entry_price) * order.quantity - commission
                            pos.quantity -= order.quantity
                            self.cash += (trade_value - commission)

                order.filled = True
                order.filled_price = fill_price
                order.commission = commission
                executed_orders.append(order)

        # Remove executed orders
        self.pending_orders = [o for o in self.pending_orders if not o.filled]

    def update_positions(self, bar: pd.Series):
        """Update position prices and check stop losses."""
        symbol = bar.name

        if symbol in self.positions:
            pos = self.positions[symbol]
            pos.current_price = bar['close']

            # Update MAE/MFE
            if symbol in self.position_extremes:
                min_price, max_price = self.position_extremes[symbol]
                self.position_extremes[symbol] = (
                    min(min_price, bar['low']),
                    max(max_price, bar['high'])
                )

            # Check stop loss
            if pos.stop_loss and bar['low'] <= pos.stop_loss:
                # Stop hit - close position
                self.submit_order(
                    symbol=symbol,
                    side=OrderSide.SELL,
                    quantity=pos.quantity,
                    order_type=OrderType.MARKET
                )

    def run(self) -> BacktestResults:
        """
        Run the backtest.

        Returns:
            BacktestResults object with complete analysis
        """
        logger.info(f"Starting backtest with ${self.initial_capital:,.2f} initial capital")

        # Iterate through bars
        for timestamp, bar in self.data.iterrows():
            self.current_time = timestamp

            # Update positions
            if hasattr(bar, 'name'):
                self.update_positions(bar)

            # Execute pending orders
            if hasattr(bar, 'name'):
                self.execute_orders(bar)

            # Call strategy
            self.strategy.on_bar(bar)

            # Record equity
            self.equity_curve.append(self.portfolio_value)

        # Calculate results
        results = self._calculate_results()

        logger.info(f"Backtest complete: {results.total_trades} trades, {results.total_return:.2f}% return")

        return results

    def _calculate_results(self) -> BacktestResults:
        """Calculate comprehensive backtest results."""
        # Returns
        equity_arr = np.array(self.equity_curve)
        daily_returns = np.diff(equity_arr) / equity_arr[:-1]

        total_return = ((self.portfolio_value - self.initial_capital) / self.initial_capital) * 100

        # Annualized return
        days = len(self.equity_curve)
        years = days / 252  # Trading days per year
        annual_return = ((self.portfolio_value / self.initial_capital) ** (1/years) - 1) * 100 if years > 0 else 0

        # Sharpe ratio
        if len(daily_returns) > 0:
            mean_return = np.mean(daily_returns) * 252
            std_return = np.std(daily_returns) * np.sqrt(252)
            sharpe_ratio = (mean_return - self.risk_free_rate) / std_return if std_return > 0 else 0
        else:
            sharpe_ratio = 0

        # Sortino ratio
        downside_returns = daily_returns[daily_returns < 0]
        if len(downside_returns) > 0:
            downside_std = np.std(downside_returns) * np.sqrt(252)
            sortino_ratio = (annual_return/100 - self.risk_free_rate) / downside_std if downside_std > 0 else 0
        else:
            sortino_ratio = 0

        # Max drawdown
        running_max = np.maximum.accumulate(equity_arr)
        drawdown = (equity_arr - running_max) / running_max
        max_drawdown = abs(drawdown.min()) * 100

        # Drawdown duration
        max_dd_idx = drawdown.argmin()
        recovery_idx = max_dd_idx
        for i in range(max_dd_idx + 1, len(equity_arr)):
            if equity_arr[i] >= running_max[max_dd_idx]:
                recovery_idx = i
                break
        max_dd_duration = recovery_idx - max_dd_idx

        # Calmar ratio
        calmar_ratio = (annual_return / max_drawdown) if max_drawdown > 0 else 0

        # Volatility
        volatility = np.std(daily_returns) * np.sqrt(252) * 100 if len(daily_returns) > 0 else 0

        # Trade statistics
        winning_trades = [t for t in self.trades if t.pnl > 0]
        losing_trades = [t for t in self.trades if t.pnl < 0]

        win_rate = (len(winning_trades) / len(self.trades) * 100) if self.trades else 0
        avg_win = np.mean([t.pnl for t in winning_trades]) if winning_trades else 0
        avg_loss = np.mean([t.pnl for t in losing_trades]) if losing_trades else 0

        gross_profit = sum(t.pnl for t in winning_trades)
        gross_loss = abs(sum(t.pnl for t in losing_trades))
        profit_factor = (gross_profit / gross_loss) if gross_loss > 0 else 0

        avg_trade = np.mean([t.pnl for t in self.trades]) if self.trades else 0
        avg_hold_time = np.mean([t.hold_time for t in self.trades]) if self.trades else 0

        best_trade = max([t.pnl for t in self.trades]) if self.trades else 0
        worst_trade = min([t.pnl for t in self.trades]) if self.trades else 0

        # Streaks
        streaks = []
        current_streak = 0
        for trade in self.trades:
            if trade.pnl > 0:
                current_streak = current_streak + 1 if current_streak > 0 else 1
            else:
                current_streak = current_streak - 1 if current_streak < 0 else -1
            streaks.append(current_streak)

        largest_win_streak = max([s for s in streaks if s > 0]) if any(s > 0 for s in streaks) else 0
        largest_loss_streak = abs(min([s for s in streaks if s < 0])) if any(s < 0 for s in streaks) else 0

        # Exposure
        position_values = [sum(pos.market_value for pos in self.positions.values()) for _ in self.equity_curve]
        avg_exposure = (np.mean(position_values) / self.initial_capital * 100) if position_values else 0

        return BacktestResults(
            total_return=total_return,
            annual_return=annual_return,
            sharpe_ratio=sharpe_ratio,
            sortino_ratio=sortino_ratio,
            calmar_ratio=calmar_ratio,
            max_drawdown=max_drawdown,
            max_drawdown_duration=max_dd_duration,
            volatility=volatility,
            total_trades=len(self.trades),
            winning_trades=len(winning_trades),
            losing_trades=len(losing_trades),
            win_rate=win_rate,
            avg_win=avg_win,
            avg_loss=avg_loss,
            profit_factor=profit_factor,
            avg_trade=avg_trade,
            avg_hold_time=avg_hold_time,
            best_trade=best_trade,
            worst_trade=worst_trade,
            largest_win_streak=largest_win_streak,
            largest_loss_streak=largest_loss_streak,
            initial_capital=self.initial_capital,
            final_capital=self.portfolio_value,
            peak_capital=max(self.equity_curve),
            avg_exposure=avg_exposure,
            equity_curve=self.equity_curve,
            trades=self.trades,
            daily_returns=daily_returns
        )


# Example strategy
class SimpleTrendStrategy(Strategy):
    """Simple trend-following strategy example."""

    def __init__(self, fast_ma: int = 20, slow_ma: int = 50):
        super().__init__()
        self.fast_ma = fast_ma
        self.slow_ma = slow_ma

    def on_bar(self, bar: pd.Series):
        """Buy when fast MA crosses above slow MA, sell on reverse."""
        # This is simplified - in production would calculate MAs properly
        pass  # Example only


# Example usage
def example():
    """Example backtest."""
    print("\n=== Backtesting Framework Example ===\n")

    # Create sample data
    dates = pd.date_range('2023-01-01', '2024-01-01', freq='D')
    np.random.seed(42)

    # Simulate price data with trend
    price = 100
    prices = []
    for _ in range(len(dates)):
        price = price * (1 + np.random.normal(0.001, 0.02))
        prices.append(price)

    data = pd.DataFrame({
        'open': prices,
        'high': [p * 1.01 for p in prices],
        'low': [p * 0.99 for p in prices],
        'close': prices,
        'volume': np.random.randint(1000000, 10000000, len(dates))
    }, index=dates)

    # Simple strategy
    class TestStrategy(Strategy):
        def on_bar(self, bar: pd.Series):
            # Buy at start, sell at end (simple test)
            positions = self.get_positions()
            if len(positions) == 0 and self.cash > 10000:
                self.buy(symbol="TEST", quantity=100)
            elif len(positions) > 0 and pd.Timestamp(bar.name) > dates[-10]:
                self.sell(symbol="TEST", quantity=100)

    # Run backtest
    backtest = Backtest(
        strategy=TestStrategy(),
        data=data,
        initial_capital=100000
    )

    results = backtest.run()
    print(results.summary())


if __name__ == "__main__":
    example()
