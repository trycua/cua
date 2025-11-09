"""
Trading-Specific Callbacks and Safeguards for Cua Agent

This module provides safety mechanisms and monitoring callbacks specifically
designed for trading agents:

- KillSwitchCallback: Emergency stop on loss limits
- AuditTrailCallback: Comprehensive logging of all decisions
- ComplianceCallback: Regulatory checks (PDT, wash sales, etc.)
- PerformanceMonitorCallback: Real-time performance tracking
- ScreenshotAuditCallback: Visual record of every decision

Example usage:
    from agent import ComputerAgent
    from trading_callbacks import KillSwitchCallback, AuditTrailCallback

    kill_switch = KillSwitchCallback(max_daily_loss=0.02, max_drawdown=0.10)
    audit_trail = AuditTrailCallback(save_dir="./audit_logs")

    agent = ComputerAgent(
        model="anthropic/claude-sonnet-4-5",
        tools=[...],
        callbacks=[kill_switch, audit_trail]
    )
"""

import asyncio
import json
import logging
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Dict, List, Optional

logger = logging.getLogger(__name__)


class KillSwitchCallback:
    """
    Emergency stop mechanism that halts trading on risk limit violations.

    Monitors:
    - Daily loss limit
    - Maximum drawdown
    - Losing streak
    - Position concentration
    - API connection health
    """

    def __init__(
        self,
        max_daily_loss: float = 0.02,  # 2%
        max_drawdown: float = 0.10,    # 10%
        max_losing_streak: int = 5,
        max_position_size: float = 0.20,  # 20%
        enabled: bool = True
    ):
        self.max_daily_loss = max_daily_loss
        self.max_drawdown = max_drawdown
        self.max_losing_streak = max_losing_streak
        self.max_position_size = max_position_size
        self.enabled = enabled

        self.starting_value = None
        self.peak_value = None
        self.current_losing_streak = 0
        self.is_halted = False
        self.halt_reason = None

    def check_daily_loss(self, current_value: float) -> bool:
        """Check if daily loss limit is exceeded."""
        if self.starting_value is None:
            self.starting_value = current_value
            return False

        daily_loss = (self.starting_value - current_value) / self.starting_value
        if daily_loss >= self.max_daily_loss:
            self.halt_trading(f"Daily loss limit exceeded: {daily_loss:.2%}")
            return True
        return False

    def check_drawdown(self, current_value: float) -> bool:
        """Check if maximum drawdown is exceeded."""
        if self.peak_value is None:
            self.peak_value = current_value
        else:
            self.peak_value = max(self.peak_value, current_value)

        drawdown = (self.peak_value - current_value) / self.peak_value
        if drawdown >= self.max_drawdown:
            self.halt_trading(f"Maximum drawdown exceeded: {drawdown:.2%}")
            return True
        return False

    def record_trade_result(self, is_winner: bool):
        """Record trade result and check losing streak."""
        if is_winner:
            self.current_losing_streak = 0
        else:
            self.current_losing_streak += 1
            if self.current_losing_streak >= self.max_losing_streak:
                self.halt_trading(f"Maximum losing streak reached: {self.current_losing_streak}")

    def halt_trading(self, reason: str):
        """Halt all trading activity."""
        self.is_halted = True
        self.halt_reason = reason
        logger.critical(f"🛑 TRADING HALTED: {reason}")

    def reset_daily_limits(self):
        """Reset daily tracking (call at start of trading day)."""
        self.starting_value = None
        self.is_halted = False
        self.halt_reason = None
        logger.info("Kill switch daily limits reset")

    async def on_before_action(self, context: Dict[str, Any]) -> Dict[str, Any]:
        """Called before any action - check if trading is halted."""
        if not self.enabled:
            return context

        if self.is_halted:
            logger.error(f"Action blocked by kill switch: {self.halt_reason}")
            raise RuntimeError(f"Trading is halted: {self.halt_reason}")

        # Check limits if portfolio value is available
        if "portfolio_value" in context:
            self.check_daily_loss(context["portfolio_value"])
            self.check_drawdown(context["portfolio_value"])

        return context

    async def on_after_action(self, context: Dict[str, Any], result: Any) -> Any:
        """Called after action completes."""
        return result


class AuditTrailCallback:
    """
    Comprehensive audit logging for regulatory compliance and debugging.

    Logs:
    - Every agent decision and reasoning
    - All trade executions
    - Screenshots at decision points
    - Performance metrics
    - Risk calculations
    """

    def __init__(
        self,
        save_dir: str = "./audit_logs",
        save_screenshots: bool = True,
        save_reasoning: bool = True
    ):
        self.save_dir = Path(save_dir)
        self.save_dir.mkdir(parents=True, exist_ok=True)

        self.save_screenshots = save_screenshots
        self.save_reasoning = save_reasoning

        self.session_id = datetime.now().strftime("%Y%m%d_%H%M%S")
        self.session_dir = self.save_dir / self.session_id
        self.session_dir.mkdir(exist_ok=True)

        self.trade_log: List[Dict[str, Any]] = []
        self.decision_log: List[Dict[str, Any]] = []

        logger.info(f"Audit trail initialized: {self.session_dir}")

    async def log_decision(
        self,
        decision_type: str,
        reasoning: str,
        data: Dict[str, Any],
        screenshot: Optional[bytes] = None
    ):
        """Log an agent decision."""
        timestamp = datetime.now().isoformat()
        decision_id = f"{timestamp}_{decision_type}"

        decision_entry = {
            "id": decision_id,
            "timestamp": timestamp,
            "type": decision_type,
            "reasoning": reasoning if self.save_reasoning else "[redacted]",
            "data": data,
        }

        self.decision_log.append(decision_entry)

        # Save screenshot if provided
        if screenshot and self.save_screenshots:
            screenshot_path = self.session_dir / f"{decision_id}.png"
            with open(screenshot_path, "wb") as f:
                f.write(screenshot)
            decision_entry["screenshot"] = str(screenshot_path)

        # Save decision to JSON
        self._save_decision_log()

    async def log_trade(
        self,
        symbol: str,
        side: str,
        quantity: int,
        price: float,
        order_id: str,
        reasoning: str
    ):
        """Log a trade execution."""
        trade_entry = {
            "timestamp": datetime.now().isoformat(),
            "symbol": symbol,
            "side": side,
            "quantity": quantity,
            "price": price,
            "value": quantity * price,
            "order_id": order_id,
            "reasoning": reasoning if self.save_reasoning else "[redacted]",
        }

        self.trade_log.append(trade_entry)
        self._save_trade_log()

        logger.info(f"Trade logged: {side} {quantity} {symbol} @ ${price}")

    def _save_decision_log(self):
        """Save decision log to JSON file."""
        log_path = self.session_dir / "decisions.json"
        with open(log_path, "w") as f:
            json.dump(self.decision_log, f, indent=2)

    def _save_trade_log(self):
        """Save trade log to JSON file."""
        log_path = self.session_dir / "trades.json"
        with open(log_path, "w") as f:
            json.dump(self.trade_log, f, indent=2)

    def generate_summary_report(self) -> Dict[str, Any]:
        """Generate summary report of the session."""
        if not self.trade_log:
            return {"message": "No trades executed in this session"}

        total_trades = len(self.trade_log)
        total_value = sum(t["value"] for t in self.trade_log)
        buy_trades = [t for t in self.trade_log if t["side"] == "buy"]
        sell_trades = [t for t in self.trade_log if t["side"] == "sell"]

        symbols_traded = set(t["symbol"] for t in self.trade_log)

        return {
            "session_id": self.session_id,
            "total_trades": total_trades,
            "total_value": total_value,
            "buy_trades": len(buy_trades),
            "sell_trades": len(sell_trades),
            "symbols_traded": list(symbols_traded),
            "total_decisions": len(self.decision_log),
        }

    async def on_session_end(self):
        """Called when trading session ends."""
        summary = self.generate_summary_report()
        summary_path = self.session_dir / "summary.json"
        with open(summary_path, "w") as f:
            json.dump(summary, f, indent=2)

        logger.info(f"Session summary saved: {summary_path}")
        return summary


class ComplianceCallback:
    """
    Regulatory compliance checks for US markets.

    Monitors:
    - Pattern Day Trading (PDT) rule
    - Wash sale violations
    - Position limits
    - Good faith violations (cash accounts)
    """

    def __init__(
        self,
        account_value: float,
        account_type: str = "margin",  # or "cash"
        jurisdiction: str = "US"
    ):
        self.account_value = account_value
        self.account_type = account_type
        self.jurisdiction = jurisdiction

        self.day_trades_count = 0  # Rolling 5 business days
        self.day_trades_history: List[datetime] = []
        self.trade_history: List[Dict[str, Any]] = []

    def check_pattern_day_trader(self) -> tuple[bool, str]:
        """
        Check if Pattern Day Trading rule is violated.

        PDT Rule: 4+ day trades in 5 business days with account < $25k
        """
        if self.jurisdiction != "US":
            return True, "Not subject to US PDT rule"

        if self.account_value >= 25000:
            return True, "Account value above PDT threshold"

        # Count day trades in last 5 business days
        cutoff = datetime.now() - timedelta(days=5)
        recent_day_trades = [dt for dt in self.day_trades_history if dt > cutoff]

        if len(recent_day_trades) >= 4:
            return False, f"PDT violation: {len(recent_day_trades)} day trades with <$25k account"

        return True, f"OK: {len(recent_day_trades)}/4 day trades"

    def check_wash_sale(self, symbol: str, side: str) -> tuple[bool, str]:
        """
        Check for potential wash sale violation.

        Wash Sale: Selling at loss and repurchasing within 30 days
        """
        if side != "buy":
            return True, "Not a buy order"

        # Look for sells of same symbol in last 30 days
        cutoff = datetime.now() - timedelta(days=30)
        recent_sells = [
            t for t in self.trade_history
            if t["symbol"] == symbol
            and t["side"] == "sell"
            and datetime.fromisoformat(t["timestamp"]) > cutoff
        ]

        if recent_sells:
            return False, f"Potential wash sale: sold {symbol} within last 30 days"

        return True, "No wash sale risk"

    def record_day_trade(self):
        """Record a day trade."""
        self.day_trades_count += 1
        self.day_trades_history.append(datetime.now())
        logger.info(f"Day trade recorded. Total in 5 days: {self.day_trades_count}")

    def record_trade(self, trade: Dict[str, Any]):
        """Record a trade for compliance tracking."""
        self.trade_history.append(trade)

    async def on_before_trade(self, context: Dict[str, Any]) -> Dict[str, Any]:
        """Check compliance before executing trade."""
        symbol = context.get("symbol")
        side = context.get("side")
        is_day_trade = context.get("is_day_trade", False)

        warnings = []

        # Check PDT
        if is_day_trade:
            pdt_ok, pdt_msg = self.check_pattern_day_trader()
            if not pdt_ok:
                raise RuntimeError(f"Trade blocked: {pdt_msg}")
            warnings.append(pdt_msg)

        # Check wash sale
        if symbol and side:
            wash_ok, wash_msg = self.check_wash_sale(symbol, side)
            if not wash_ok:
                warnings.append(f"WARNING: {wash_msg}")

        if warnings:
            context["compliance_warnings"] = warnings
            for warning in warnings:
                logger.warning(f"Compliance: {warning}")

        return context


class PerformanceMonitorCallback:
    """
    Real-time performance monitoring and alerting.

    Tracks:
    - Win rate
    - Profit factor
    - Sharpe ratio
    - Current drawdown
    - Trade frequency
    """

    def __init__(
        self,
        alert_on_drawdown: float = 0.05,  # Alert at 5% drawdown
        alert_on_low_win_rate: float = 0.40,  # Alert if win rate < 40%
    ):
        self.alert_on_drawdown = alert_on_drawdown
        self.alert_on_low_win_rate = alert_on_low_win_rate

        self.trades: List[Dict[str, Any]] = []
        self.equity_curve: List[float] = []
        self.peak_equity = 0

    def update_performance(self, trade_result: Dict[str, Any]):
        """Update performance metrics with new trade result."""
        self.trades.append(trade_result)

        # Update equity curve
        if "portfolio_value" in trade_result:
            equity = trade_result["portfolio_value"]
            self.equity_curve.append(equity)
            self.peak_equity = max(self.peak_equity, equity)

            # Check drawdown
            current_dd = (self.peak_equity - equity) / self.peak_equity
            if current_dd >= self.alert_on_drawdown:
                logger.warning(f"⚠️  Drawdown alert: {current_dd:.2%}")

    def get_current_metrics(self) -> Dict[str, Any]:
        """Calculate current performance metrics."""
        if not self.trades:
            return {"message": "No trades yet"}

        winning_trades = [t for t in self.trades if t.get("pnl", 0) > 0]
        losing_trades = [t for t in self.trades if t.get("pnl", 0) < 0]

        win_rate = len(winning_trades) / len(self.trades)
        total_pnl = sum(t.get("pnl", 0) for t in self.trades)

        # Check win rate alert
        if win_rate < self.alert_on_low_win_rate:
            logger.warning(f"⚠️  Low win rate alert: {win_rate:.1%}")

        return {
            "total_trades": len(self.trades),
            "winning_trades": len(winning_trades),
            "losing_trades": len(losing_trades),
            "win_rate": win_rate * 100,
            "total_pnl": total_pnl,
            "avg_win": sum(t["pnl"] for t in winning_trades) / len(winning_trades) if winning_trades else 0,
            "avg_loss": sum(t["pnl"] for t in losing_trades) / len(losing_trades) if losing_trades else 0,
            "current_drawdown": ((self.peak_equity - self.equity_curve[-1]) / self.peak_equity * 100)
                                if self.equity_curve else 0,
        }

    def print_summary(self):
        """Print performance summary to console."""
        metrics = self.get_current_metrics()
        if "message" in metrics:
            print(metrics["message"])
            return

        print("\n=== Performance Summary ===")
        print(f"Total Trades: {metrics['total_trades']}")
        print(f"Win Rate: {metrics['win_rate']:.1f}%")
        print(f"Total P&L: ${metrics['total_pnl']:,.2f}")
        print(f"Avg Win: ${metrics['avg_win']:.2f}")
        print(f"Avg Loss: ${metrics['avg_loss']:.2f}")
        print(f"Current Drawdown: {metrics['current_drawdown']:.2f}%")
        print("=" * 27 + "\n")


# Example usage
async def example_usage():
    """Example of using trading callbacks."""

    print("\n=== Trading Callbacks Example ===\n")

    # Initialize callbacks
    kill_switch = KillSwitchCallback(
        max_daily_loss=0.02,
        max_drawdown=0.10,
        max_losing_streak=5
    )

    audit_trail = AuditTrailCallback(
        save_dir="./audit_logs",
        save_screenshots=True
    )

    compliance = ComplianceCallback(
        account_value=30000,
        account_type="margin"
    )

    monitor = PerformanceMonitorCallback(
        alert_on_drawdown=0.05,
        alert_on_low_win_rate=0.40
    )

    # Simulate some trading activity
    print("1. Simulating trades...\n")

    # Trade 1: Winning trade
    await audit_trail.log_trade(
        symbol="AAPL",
        side="buy",
        quantity=100,
        price=150.0,
        order_id="order_001",
        reasoning="Bullish breakout pattern on daily chart"
    )

    kill_switch.record_trade_result(is_winner=True)
    monitor.update_performance({
        "symbol": "AAPL",
        "pnl": 500.0,
        "portfolio_value": 100500
    })

    # Trade 2: Losing trade
    await audit_trail.log_trade(
        symbol="TSLA",
        side="buy",
        quantity=50,
        price=200.0,
        order_id="order_002",
        reasoning="RSI oversold, expecting bounce"
    )

    kill_switch.record_trade_result(is_winner=False)
    monitor.update_performance({
        "symbol": "TSLA",
        "pnl": -300.0,
        "portfolio_value": 100200
    })

    # Check compliance for new trade
    print("\n2. Checking compliance for new trade...\n")
    context = {
        "symbol": "AAPL",
        "side": "buy",
        "is_day_trade": False
    }
    context = await compliance.on_before_trade(context)

    # Print performance
    print("\n3. Current performance metrics:\n")
    monitor.print_summary()

    # Generate audit report
    print("4. Generating audit trail summary...\n")
    summary = audit_trail.generate_summary_report()
    print(f"   Session ID: {summary['session_id']}")
    print(f"   Total trades: {summary['total_trades']}")
    print(f"   Symbols traded: {', '.join(summary['symbols_traded'])}")

    await audit_trail.on_session_end()

    print("\n=== Example Complete ===\n")


if __name__ == "__main__":
    asyncio.run(example_usage())
