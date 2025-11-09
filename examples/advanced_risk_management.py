"""
Advanced Risk Management Module

Professional-grade risk management for trading algorithms.

Features:
- Kelly Criterion position sizing
- Portfolio correlation analysis
- Value at Risk (VaR) calculation
- Sharpe ratio optimization
- Maximum Adverse Excursion (MAE) tracking
- Position correlation limits
- Dynamic position sizing based on volatility
- Risk-adjusted returns

Usage:
    from advanced_risk_management import AdvancedRiskManager

    risk_mgr = AdvancedRiskManager(
        portfolio_value=100000,
        max_portfolio_risk=0.02
    )

    # Kelly Criterion position size
    position_size = risk_mgr.kelly_position_size(
        win_rate=0.55,
        avg_win=1000,
        avg_loss=500
    )

    # Check portfolio correlation
    can_trade = risk_mgr.check_correlation_limits(
        symbol="AAPL",
        existing_positions=positions
    )
"""

import logging
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Tuple

import numpy as np
import pandas as pd

logger = logging.getLogger(__name__)


@dataclass
class RiskMetrics:
    """Container for risk metrics."""
    portfolio_var: float  # Value at Risk (95% confidence)
    portfolio_volatility: float  # Annualized volatility
    sharpe_ratio: float
    sortino_ratio: float
    max_drawdown: float
    max_drawdown_duration: int  # Days
    calmar_ratio: float  # Return / Max Drawdown
    correlation_risk: float  # Portfolio correlation score
    concentration_risk: float  # Position concentration score
    leverage: float  # Current leverage ratio


@dataclass
class PositionRisk:
    """Risk analysis for a specific position."""
    symbol: str
    quantity: int
    entry_price: float
    current_price: float
    position_value: float

    # Risk metrics
    var_95: float  # 95% Value at Risk
    stop_loss: float
    max_loss: float  # Max loss if stop hit
    max_loss_pct: float
    volatility: float  # Historical volatility
    beta: float  # Beta vs market (SPY)

    # Portfolio impact
    portfolio_pct: float  # % of portfolio
    portfolio_risk_contribution: float  # Contribution to portfolio risk


class AdvancedRiskManager:
    """
    Advanced risk management system.

    Implements professional risk management techniques including:
    - Kelly Criterion
    - Portfolio optimization
    - Correlation analysis
    - Value at Risk (VaR)
    - Dynamic position sizing
    """

    def __init__(
        self,
        portfolio_value: float,
        max_portfolio_risk: float = 0.02,  # 2% max daily risk
        max_position_size: float = 0.10,  # 10% max per position
        max_correlated_exposure: float = 0.30,  # 30% max in correlated positions
        max_leverage: float = 1.0,  # No leverage by default
        risk_free_rate: float = 0.04,  # 4% annual risk-free rate
    ):
        """
        Initialize advanced risk manager.

        Args:
            portfolio_value: Current portfolio value
            max_portfolio_risk: Maximum daily portfolio risk (fraction)
            max_position_size: Maximum position size (fraction of portfolio)
            max_correlated_exposure: Max exposure to correlated positions
            max_leverage: Maximum leverage allowed
            risk_free_rate: Annual risk-free rate for Sharpe calculation
        """
        self.portfolio_value = portfolio_value
        self.max_portfolio_risk = max_portfolio_risk
        self.max_position_size = max_position_size
        self.max_correlated_exposure = max_correlated_exposure
        self.max_leverage = max_leverage
        self.risk_free_rate = risk_free_rate

        # Historical data for calculations
        self.equity_curve: List[float] = [portfolio_value]
        self.trade_history: List[Dict[str, Any]] = []

    def kelly_position_size(
        self,
        win_rate: float,
        avg_win: float,
        avg_loss: float,
        kelly_fraction: float = 0.25  # Use quarter Kelly for safety
    ) -> float:
        """
        Calculate optimal position size using Kelly Criterion.

        Kelly % = (Win Rate * Avg Win - Loss Rate * Avg Loss) / Avg Win

        Args:
            win_rate: Historical win rate (0-1)
            avg_win: Average winning trade amount
            avg_loss: Average losing trade amount (positive number)
            kelly_fraction: Fraction of Kelly to use (0.25 = quarter Kelly)

        Returns:
            Optimal position size as fraction of portfolio
        """
        if avg_win <= 0:
            logger.warning("Average win must be positive")
            return 0.0

        loss_rate = 1 - win_rate

        # Kelly formula
        kelly_pct = (win_rate * avg_win - loss_rate * avg_loss) / avg_win

        # Apply fraction (quarter Kelly is safer)
        kelly_pct = kelly_pct * kelly_fraction

        # Cap at max position size
        kelly_pct = max(0, min(kelly_pct, self.max_position_size))

        logger.info(f"Kelly position size: {kelly_pct:.2%} (win rate: {win_rate:.1%})")

        return kelly_pct

    def volatility_adjusted_position_size(
        self,
        base_position_size: float,
        current_volatility: float,
        avg_volatility: float
    ) -> float:
        """
        Adjust position size based on current volatility vs average.

        In high volatility, reduce position size. In low volatility, can increase.

        Args:
            base_position_size: Base position size (fraction)
            current_volatility: Current volatility estimate
            avg_volatility: Average historical volatility

        Returns:
            Adjusted position size
        """
        if avg_volatility == 0:
            return base_position_size

        # Inverse relationship: higher volatility = smaller position
        vol_ratio = avg_volatility / current_volatility
        adjusted_size = base_position_size * vol_ratio

        # Cap adjustments
        adjusted_size = max(base_position_size * 0.5, min(base_position_size * 1.5, adjusted_size))
        adjusted_size = min(adjusted_size, self.max_position_size)

        return adjusted_size

    def calculate_var(
        self,
        returns: np.ndarray,
        confidence: float = 0.95,
        time_horizon: int = 1
    ) -> float:
        """
        Calculate Value at Risk (VaR) using historical simulation.

        Args:
            returns: Array of historical returns
            confidence: Confidence level (0.95 = 95%)
            time_horizon: Time horizon in days

        Returns:
            VaR as a positive number (max expected loss)
        """
        if len(returns) == 0:
            return 0.0

        # Sort returns
        sorted_returns = np.sort(returns)

        # Find percentile
        index = int((1 - confidence) * len(sorted_returns))
        var = abs(sorted_returns[index])

        # Scale by time horizon (assuming i.i.d. returns)
        var = var * np.sqrt(time_horizon)

        return var

    def calculate_portfolio_correlation(
        self,
        positions: List[Dict[str, Any]],
        price_data: Dict[str, pd.DataFrame]
    ) -> float:
        """
        Calculate average correlation between portfolio positions.

        Args:
            positions: List of position dicts with 'symbol' and 'value'
            price_data: Dict of symbol -> DataFrame with 'close' column

        Returns:
            Average pairwise correlation (0-1)
        """
        if len(positions) < 2:
            return 0.0

        symbols = [p['symbol'] for p in positions]

        # Get returns for each symbol
        returns_data = {}
        for symbol in symbols:
            if symbol in price_data:
                df = price_data[symbol]
                returns_data[symbol] = df['close'].pct_change().dropna()

        if len(returns_data) < 2:
            return 0.0

        # Align dates
        returns_df = pd.DataFrame(returns_data)
        returns_df = returns_df.dropna()

        # Calculate correlation matrix
        corr_matrix = returns_df.corr()

        # Get average correlation (excluding diagonal)
        n = len(corr_matrix)
        total_corr = corr_matrix.values.sum() - n  # Subtract diagonal
        avg_corr = total_corr / (n * (n - 1))

        return avg_corr

    def check_correlation_limits(
        self,
        symbol: str,
        existing_positions: List[Dict[str, Any]],
        price_data: Dict[str, pd.DataFrame],
        new_position_value: float
    ) -> Tuple[bool, str]:
        """
        Check if adding new position would violate correlation limits.

        Args:
            symbol: New symbol to add
            existing_positions: Current positions
            price_data: Historical price data
            new_position_value: Value of new position

        Returns:
            (allowed, reason) tuple
        """
        if not existing_positions:
            return True, "No existing positions"

        # Calculate correlation with existing positions
        high_corr_exposure = 0.0

        for pos in existing_positions:
            pos_symbol = pos['symbol']

            if pos_symbol not in price_data or symbol not in price_data:
                continue

            # Get returns
            returns1 = price_data[pos_symbol]['close'].pct_change().dropna()
            returns2 = price_data[symbol]['close'].pct_change().dropna()

            # Align
            df = pd.DataFrame({'a': returns1, 'b': returns2}).dropna()

            if len(df) < 20:  # Need minimum data
                continue

            correlation = df.corr().iloc[0, 1]

            # If highly correlated (>0.7), count exposure
            if abs(correlation) > 0.7:
                high_corr_exposure += pos['value']

        # Add new position
        high_corr_exposure += new_position_value

        # Check limit
        exposure_pct = high_corr_exposure / self.portfolio_value

        if exposure_pct > self.max_correlated_exposure:
            return False, f"Correlated exposure {exposure_pct:.1%} exceeds limit {self.max_correlated_exposure:.1%}"

        return True, "Correlation check passed"

    def calculate_sharpe_ratio(
        self,
        returns: np.ndarray,
        periods_per_year: int = 252
    ) -> float:
        """
        Calculate annualized Sharpe ratio.

        Args:
            returns: Array of period returns
            periods_per_year: Trading periods per year (252 for daily)

        Returns:
            Sharpe ratio
        """
        if len(returns) == 0:
            return 0.0

        # Annualize returns and volatility
        mean_return = np.mean(returns) * periods_per_year
        std_return = np.std(returns) * np.sqrt(periods_per_year)

        if std_return == 0:
            return 0.0

        # Sharpe ratio
        sharpe = (mean_return - self.risk_free_rate) / std_return

        return sharpe

    def calculate_sortino_ratio(
        self,
        returns: np.ndarray,
        periods_per_year: int = 252
    ) -> float:
        """
        Calculate Sortino ratio (uses downside deviation instead of total volatility).

        Args:
            returns: Array of period returns
            periods_per_year: Trading periods per year

        Returns:
            Sortino ratio
        """
        if len(returns) == 0:
            return 0.0

        mean_return = np.mean(returns) * periods_per_year

        # Downside deviation (only negative returns)
        downside_returns = returns[returns < 0]
        if len(downside_returns) == 0:
            return float('inf')

        downside_std = np.std(downside_returns) * np.sqrt(periods_per_year)

        if downside_std == 0:
            return 0.0

        sortino = (mean_return - self.risk_free_rate) / downside_std

        return sortino

    def calculate_max_drawdown(self, equity_curve: List[float]) -> Tuple[float, int]:
        """
        Calculate maximum drawdown and its duration.

        Args:
            equity_curve: List of portfolio values over time

        Returns:
            (max_drawdown, duration_in_periods)
        """
        if len(equity_curve) < 2:
            return 0.0, 0

        equity = np.array(equity_curve)

        # Running maximum
        running_max = np.maximum.accumulate(equity)

        # Drawdown
        drawdown = (equity - running_max) / running_max

        max_dd = abs(drawdown.min())

        # Duration
        max_dd_idx = drawdown.argmin()
        # Find when it recovered
        recovery_idx = max_dd_idx
        for i in range(max_dd_idx + 1, len(equity)):
            if equity[i] >= running_max[max_dd_idx]:
                recovery_idx = i
                break

        duration = recovery_idx - max_dd_idx

        return max_dd, duration

    def calculate_risk_metrics(
        self,
        equity_curve: List[float],
        positions: List[Dict[str, Any]],
        price_data: Optional[Dict[str, pd.DataFrame]] = None
    ) -> RiskMetrics:
        """
        Calculate comprehensive risk metrics for portfolio.

        Args:
            equity_curve: Historical portfolio values
            positions: Current positions
            price_data: Historical price data for correlation

        Returns:
            RiskMetrics object
        """
        # Calculate returns
        if len(equity_curve) < 2:
            returns = np.array([0.0])
        else:
            equity_arr = np.array(equity_curve)
            returns = np.diff(equity_arr) / equity_arr[:-1]

        # VaR
        var_95 = self.calculate_var(returns, confidence=0.95) * self.portfolio_value

        # Volatility (annualized)
        volatility = np.std(returns) * np.sqrt(252) if len(returns) > 0 else 0.0

        # Sharpe ratio
        sharpe = self.calculate_sharpe_ratio(returns)

        # Sortino ratio
        sortino = self.calculate_sortino_ratio(returns)

        # Max drawdown
        max_dd, dd_duration = self.calculate_max_drawdown(equity_curve)

        # Calmar ratio
        annual_return = ((equity_curve[-1] / equity_curve[0]) ** (252/len(equity_curve)) - 1) if len(equity_curve) > 1 else 0.0
        calmar = annual_return / max_dd if max_dd > 0 else 0.0

        # Correlation risk
        if price_data and positions:
            corr_risk = self.calculate_portfolio_correlation(positions, price_data)
        else:
            corr_risk = 0.0

        # Concentration risk (Herfindahl index)
        if positions:
            total_value = sum(p['value'] for p in positions)
            weights = [p['value'] / total_value for p in positions]
            concentration = sum(w**2 for w in weights)
        else:
            concentration = 0.0

        # Leverage
        position_value = sum(p.get('value', 0) for p in positions)
        leverage = position_value / self.portfolio_value if self.portfolio_value > 0 else 0.0

        return RiskMetrics(
            portfolio_var=var_95,
            portfolio_volatility=volatility,
            sharpe_ratio=sharpe,
            sortino_ratio=sortino,
            max_drawdown=max_dd,
            max_drawdown_duration=dd_duration,
            calmar_ratio=calmar,
            correlation_risk=corr_risk,
            concentration_risk=concentration,
            leverage=leverage
        )

    def analyze_position_risk(
        self,
        symbol: str,
        quantity: int,
        entry_price: float,
        current_price: float,
        stop_loss: float,
        historical_returns: np.ndarray,
        beta: float = 1.0
    ) -> PositionRisk:
        """
        Analyze risk for a specific position.

        Args:
            symbol: Symbol name
            quantity: Position size
            entry_price: Entry price
            current_price: Current price
            stop_loss: Stop loss price
            historical_returns: Historical daily returns
            beta: Beta vs market

        Returns:
            PositionRisk object
        """
        position_value = quantity * current_price

        # VaR
        var_95 = self.calculate_var(historical_returns) * position_value

        # Max loss if stop hit
        max_loss = abs(quantity * (current_price - stop_loss))
        max_loss_pct = abs((current_price - stop_loss) / current_price)

        # Volatility
        volatility = np.std(historical_returns) * np.sqrt(252) if len(historical_returns) > 0 else 0.0

        # Portfolio impact
        portfolio_pct = position_value / self.portfolio_value
        portfolio_risk_contribution = var_95 / self.portfolio_value

        return PositionRisk(
            symbol=symbol,
            quantity=quantity,
            entry_price=entry_price,
            current_price=current_price,
            position_value=position_value,
            var_95=var_95,
            stop_loss=stop_loss,
            max_loss=max_loss,
            max_loss_pct=max_loss_pct,
            volatility=volatility,
            beta=beta,
            portfolio_pct=portfolio_pct,
            portfolio_risk_contribution=portfolio_risk_contribution
        )

    def optimal_portfolio_allocation(
        self,
        expected_returns: Dict[str, float],
        volatilities: Dict[str, float],
        correlations: pd.DataFrame,
        risk_tolerance: float = 1.0
    ) -> Dict[str, float]:
        """
        Calculate optimal portfolio weights using mean-variance optimization.

        Args:
            expected_returns: Dict of symbol -> expected annual return
            volatilities: Dict of symbol -> annual volatility
            correlations: Correlation matrix of returns
            risk_tolerance: Risk tolerance factor (1.0 = moderate)

        Returns:
            Dict of symbol -> portfolio weight
        """
        # This is a simplified version
        # In production, would use scipy.optimize or cvxpy

        symbols = list(expected_returns.keys())
        n = len(symbols)

        if n == 0:
            return {}

        # Equal weight as baseline
        weights = {symbol: 1/n for symbol in symbols}

        # Adjust by Sharpe ratio
        sharpe_ratios = {
            symbol: (expected_returns[symbol] - self.risk_free_rate) / volatilities[symbol]
            if volatilities[symbol] > 0 else 0
            for symbol in symbols
        }

        total_sharpe = sum(max(0, sr) for sr in sharpe_ratios.values())

        if total_sharpe > 0:
            weights = {
                symbol: max(0, sharpe_ratios[symbol]) / total_sharpe
                for symbol in symbols
            }

        # Normalize
        total = sum(weights.values())
        if total > 0:
            weights = {symbol: w/total for symbol, w in weights.items()}

        return weights

    def update_equity(self, new_value: float):
        """Update equity curve with new portfolio value."""
        self.equity_curve.append(new_value)
        self.portfolio_value = new_value

    def record_trade(self, trade: Dict[str, Any]):
        """Record trade for performance tracking."""
        self.trade_history.append(trade)


# Example usage
def example():
    """Example usage of AdvancedRiskManager."""
    print("\n=== Advanced Risk Management Example ===\n")

    # Initialize
    risk_mgr = AdvancedRiskManager(
        portfolio_value=100000,
        max_portfolio_risk=0.02,
        max_position_size=0.10
    )

    # 1. Kelly Criterion position sizing
    print("1. Kelly Criterion Position Sizing:")
    kelly_size = risk_mgr.kelly_position_size(
        win_rate=0.55,
        avg_win=1000,
        avg_loss=500
    )
    print(f"   Optimal position size: {kelly_size:.2%} of portfolio")
    print(f"   Dollar amount: ${kelly_size * risk_mgr.portfolio_value:,.2f}\n")

    # 2. Volatility-adjusted sizing
    print("2. Volatility-Adjusted Position Sizing:")
    base_size = 0.10
    adj_size = risk_mgr.volatility_adjusted_position_size(
        base_position_size=base_size,
        current_volatility=0.25,  # 25% vol
        avg_volatility=0.20  # 20% avg vol
    )
    print(f"   Base size: {base_size:.2%}")
    print(f"   Adjusted for high volatility: {adj_size:.2%}\n")

    # 3. Value at Risk
    print("3. Value at Risk Calculation:")
    # Simulate returns
    np.random.seed(42)
    returns = np.random.normal(0.001, 0.02, 100)  # 0.1% mean, 2% std daily returns
    var_95 = risk_mgr.calculate_var(returns, confidence=0.95)
    print(f"   95% VaR: {var_95:.2%} of portfolio")
    print(f"   Dollar amount: ${var_95 * risk_mgr.portfolio_value:,.2f}\n")

    # 4. Performance metrics
    print("4. Performance Metrics:")
    # Simulate equity curve
    equity = [100000]
    for r in returns:
        equity.append(equity[-1] * (1 + r))

    sharpe = risk_mgr.calculate_sharpe_ratio(returns)
    sortino = risk_mgr.calculate_sortino_ratio(returns)
    max_dd, dd_duration = risk_mgr.calculate_max_drawdown(equity)

    print(f"   Sharpe Ratio: {sharpe:.2f}")
    print(f"   Sortino Ratio: {sortino:.2f}")
    print(f"   Max Drawdown: {max_dd:.2%}")
    print(f"   Drawdown Duration: {dd_duration} periods\n")

    # 5. Risk metrics
    print("5. Comprehensive Risk Metrics:")
    positions = [
        {'symbol': 'AAPL', 'value': 25000},
        {'symbol': 'MSFT', 'value': 20000},
    ]

    metrics = risk_mgr.calculate_risk_metrics(equity, positions)
    print(f"   Portfolio VaR: ${metrics.portfolio_var:,.2f}")
    print(f"   Portfolio Volatility: {metrics.portfolio_volatility:.2%}")
    print(f"   Leverage: {metrics.leverage:.2f}x")
    print(f"   Concentration Risk: {metrics.concentration_risk:.3f}")

    print("\n=== Example Complete ===\n")


if __name__ == "__main__":
    example()
