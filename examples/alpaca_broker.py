"""
Alpaca Broker Integration for Trading Agent

This module provides real broker integration with Alpaca Markets API.
Supports both paper trading and live trading.

Setup:
1. Create account at https://alpaca.markets
2. Get API keys from dashboard
3. Set environment variables:
   - ALPACA_API_KEY
   - ALPACA_SECRET_KEY
   - ALPACA_BASE_URL (paper: https://paper-api.alpaca.markets)

Usage:
    from alpaca_broker import AlpacaBroker

    broker = AlpacaBroker(
        api_key=os.getenv("ALPACA_API_KEY"),
        secret_key=os.getenv("ALPACA_SECRET_KEY"),
        paper=True  # Start with paper trading
    )

    # Place order
    order = await broker.submit_order(
        symbol="AAPL",
        qty=10,
        side="buy",
        type="market"
    )
"""

import asyncio
import logging
import os
from datetime import datetime, timedelta
from typing import Any, Dict, List, Optional

try:
    from alpaca.trading.client import TradingClient
    from alpaca.trading.requests import (
        MarketOrderRequest,
        LimitOrderRequest,
        StopOrderRequest,
        GetOrdersRequest
    )
    from alpaca.trading.enums import OrderSide, TimeInForce, OrderType
    from alpaca.data.historical import StockHistoricalDataClient
    from alpaca.data.requests import StockLatestQuoteRequest, StockBarsRequest
    from alpaca.data.timeframe import TimeFrame
    ALPACA_AVAILABLE = True
except ImportError:
    ALPACA_AVAILABLE = False
    logging.warning("alpaca-trade-api not installed. Install with: pip install alpaca-trade-api")

logger = logging.getLogger(__name__)


class AlpacaBroker:
    """
    Alpaca broker integration for executing trades.

    Features:
    - Market, limit, and stop orders
    - Position management
    - Account information
    - Real-time and historical data
    - Paper and live trading support
    """

    def __init__(
        self,
        api_key: Optional[str] = None,
        secret_key: Optional[str] = None,
        paper: bool = True
    ):
        """
        Initialize Alpaca broker.

        Args:
            api_key: Alpaca API key (or set ALPACA_API_KEY env var)
            secret_key: Alpaca secret key (or set ALPACA_SECRET_KEY env var)
            paper: Use paper trading (default True)
        """
        if not ALPACA_AVAILABLE:
            raise ImportError(
                "alpaca-trade-api not installed. "
                "Install with: pip install alpaca-trade-api"
            )

        self.api_key = api_key or os.getenv("ALPACA_API_KEY")
        self.secret_key = secret_key or os.getenv("ALPACA_SECRET_KEY")

        if not self.api_key or not self.secret_key:
            raise ValueError(
                "Alpaca API credentials required. "
                "Set ALPACA_API_KEY and ALPACA_SECRET_KEY environment variables."
            )

        self.paper = paper

        # Initialize trading client
        self.trading_client = TradingClient(
            api_key=self.api_key,
            secret_key=self.secret_key,
            paper=paper
        )

        # Initialize data client
        self.data_client = StockHistoricalDataClient(
            api_key=self.api_key,
            secret_key=self.secret_key
        )

        mode = "PAPER" if paper else "LIVE"
        logger.info(f"Alpaca broker initialized ({mode} mode)")

    async def get_account(self) -> Dict[str, Any]:
        """
        Get account information.

        Returns:
            Dict with: cash, portfolio_value, buying_power, etc.
        """
        account = self.trading_client.get_account()

        return {
            "cash": float(account.cash),
            "portfolio_value": float(account.portfolio_value),
            "buying_power": float(account.buying_power),
            "equity": float(account.equity),
            "initial_margin": float(account.initial_margin),
            "maintenance_margin": float(account.maintenance_margin),
            "daytrade_count": account.daytrade_count,
            "pattern_day_trader": account.pattern_day_trader,
            "trading_blocked": account.trading_blocked,
            "account_blocked": account.account_blocked,
        }

    async def submit_order(
        self,
        symbol: str,
        qty: int,
        side: str,
        type: str = "market",
        limit_price: Optional[float] = None,
        stop_price: Optional[float] = None,
        time_in_force: str = "day"
    ) -> Dict[str, Any]:
        """
        Submit an order.

        Args:
            symbol: Stock symbol (e.g., "AAPL")
            qty: Number of shares
            side: "buy" or "sell"
            type: "market", "limit", or "stop"
            limit_price: Limit price (for limit orders)
            stop_price: Stop price (for stop orders)
            time_in_force: "day", "gtc", "ioc", "fok"

        Returns:
            Dict with order details
        """
        # Convert strings to enums
        order_side = OrderSide.BUY if side.lower() == "buy" else OrderSide.SELL

        tif_map = {
            "day": TimeInForce.DAY,
            "gtc": TimeInForce.GTC,
            "ioc": TimeInForce.IOC,
            "fok": TimeInForce.FOK
        }
        tif = tif_map.get(time_in_force.lower(), TimeInForce.DAY)

        # Create order request based on type
        if type.lower() == "market":
            order_data = MarketOrderRequest(
                symbol=symbol,
                qty=qty,
                side=order_side,
                time_in_force=tif
            )
        elif type.lower() == "limit":
            if not limit_price:
                raise ValueError("limit_price required for limit orders")
            order_data = LimitOrderRequest(
                symbol=symbol,
                qty=qty,
                side=order_side,
                time_in_force=tif,
                limit_price=limit_price
            )
        elif type.lower() == "stop":
            if not stop_price:
                raise ValueError("stop_price required for stop orders")
            order_data = StopOrderRequest(
                symbol=symbol,
                qty=qty,
                side=order_side,
                time_in_force=tif,
                stop_price=stop_price
            )
        else:
            raise ValueError(f"Unsupported order type: {type}")

        # Submit order
        order = self.trading_client.submit_order(order_data)

        logger.info(f"Order submitted: {order.id} - {side} {qty} {symbol}")

        return {
            "order_id": str(order.id),
            "symbol": order.symbol,
            "qty": int(order.qty),
            "side": order.side.value,
            "type": order.type.value,
            "status": order.status.value,
            "filled_qty": int(order.filled_qty or 0),
            "filled_avg_price": float(order.filled_avg_price or 0),
            "submitted_at": order.submitted_at.isoformat() if order.submitted_at else None,
        }

    async def cancel_order(self, order_id: str) -> bool:
        """Cancel an order."""
        try:
            self.trading_client.cancel_order_by_id(order_id)
            logger.info(f"Order cancelled: {order_id}")
            return True
        except Exception as e:
            logger.error(f"Failed to cancel order {order_id}: {e}")
            return False

    async def get_order(self, order_id: str) -> Dict[str, Any]:
        """Get order status."""
        order = self.trading_client.get_order_by_id(order_id)

        return {
            "order_id": str(order.id),
            "symbol": order.symbol,
            "qty": int(order.qty),
            "side": order.side.value,
            "type": order.type.value,
            "status": order.status.value,
            "filled_qty": int(order.filled_qty or 0),
            "filled_avg_price": float(order.filled_avg_price or 0),
            "submitted_at": order.submitted_at.isoformat() if order.submitted_at else None,
        }

    async def get_positions(self) -> List[Dict[str, Any]]:
        """Get all open positions."""
        positions = self.trading_client.get_all_positions()

        return [
            {
                "symbol": pos.symbol,
                "qty": float(pos.qty),
                "side": "long" if float(pos.qty) > 0 else "short",
                "avg_entry_price": float(pos.avg_entry_price),
                "current_price": float(pos.current_price),
                "market_value": float(pos.market_value),
                "cost_basis": float(pos.cost_basis),
                "unrealized_pl": float(pos.unrealized_pl),
                "unrealized_plpc": float(pos.unrealized_plpc),
            }
            for pos in positions
        ]

    async def close_position(self, symbol: str) -> Dict[str, Any]:
        """Close a position."""
        try:
            order = self.trading_client.close_position(symbol)
            logger.info(f"Position closed: {symbol}")

            return {
                "order_id": str(order.id),
                "symbol": order.symbol,
                "status": order.status.value,
            }
        except Exception as e:
            logger.error(f"Failed to close position {symbol}: {e}")
            raise

    async def get_quote(self, symbol: str) -> Dict[str, Any]:
        """Get latest quote for a symbol."""
        request = StockLatestQuoteRequest(symbol_or_symbols=symbol)
        quotes = self.data_client.get_stock_latest_quote(request)

        quote = quotes[symbol]

        return {
            "symbol": symbol,
            "bid": float(quote.bid_price),
            "ask": float(quote.ask_price),
            "bid_size": int(quote.bid_size),
            "ask_size": int(quote.ask_size),
            "price": (float(quote.bid_price) + float(quote.ask_price)) / 2,
            "timestamp": quote.timestamp.isoformat(),
        }

    async def get_bars(
        self,
        symbol: str,
        timeframe: str = "1Day",
        start: Optional[str] = None,
        end: Optional[str] = None,
        limit: int = 100
    ) -> List[Dict[str, Any]]:
        """
        Get historical bars.

        Args:
            symbol: Stock symbol
            timeframe: "1Min", "5Min", "15Min", "1Hour", "1Day"
            start: Start date (ISO format)
            end: End date (ISO format)
            limit: Number of bars

        Returns:
            List of OHLCV bars
        """
        # Map timeframe string to TimeFrame enum
        timeframe_map = {
            "1Min": TimeFrame.Minute,
            "5Min": TimeFrame(5, "Min"),
            "15Min": TimeFrame(15, "Min"),
            "1Hour": TimeFrame.Hour,
            "1Day": TimeFrame.Day,
        }

        tf = timeframe_map.get(timeframe, TimeFrame.Day)

        # Default to last 100 days if no dates specified
        if not start:
            start = (datetime.now() - timedelta(days=100)).isoformat()
        if not end:
            end = datetime.now().isoformat()

        request = StockBarsRequest(
            symbol_or_symbols=symbol,
            timeframe=tf,
            start=start,
            end=end,
            limit=limit
        )

        bars = self.data_client.get_stock_bars(request)

        return [
            {
                "timestamp": bar.timestamp.isoformat(),
                "open": float(bar.open),
                "high": float(bar.high),
                "low": float(bar.low),
                "close": float(bar.close),
                "volume": int(bar.volume),
            }
            for bar in bars[symbol]
        ]

    async def is_market_open(self) -> bool:
        """Check if market is currently open."""
        clock = self.trading_client.get_clock()
        return clock.is_open


# Example usage
async def main():
    """Example demonstrating Alpaca broker integration."""
    print("\n=== Alpaca Broker Integration Example ===\n")

    # Check if credentials are available
    if not os.getenv("ALPACA_API_KEY"):
        print("⚠️  Alpaca credentials not found!")
        print("Set ALPACA_API_KEY and ALPACA_SECRET_KEY environment variables")
        print("Get free paper trading keys at: https://alpaca.markets\n")
        return

    try:
        # Initialize broker (paper trading)
        broker = AlpacaBroker(paper=True)

        # 1. Get account info
        print("1. Account Information:")
        account = await broker.get_account()
        print(f"   Portfolio Value: ${account['portfolio_value']:,.2f}")
        print(f"   Cash: ${account['cash']:,.2f}")
        print(f"   Buying Power: ${account['buying_power']:,.2f}")
        print(f"   Day Trade Count: {account['daytrade_count']}")

        # 2. Check market status
        print("\n2. Market Status:")
        is_open = await broker.is_market_open()
        print(f"   Market Open: {is_open}")

        # 3. Get quote
        print("\n3. Getting quote for AAPL:")
        quote = await broker.get_quote("AAPL")
        print(f"   Price: ${quote['price']:.2f}")
        print(f"   Bid/Ask: ${quote['bid']:.2f} / ${quote['ask']:.2f}")

        # 4. Get historical data
        print("\n4. Historical data (last 5 days):")
        bars = await broker.get_bars("AAPL", timeframe="1Day", limit=5)
        for bar in bars[-5:]:
            print(f"   {bar['timestamp'][:10]}: ${bar['close']:.2f}")

        # 5. Get current positions
        print("\n5. Current Positions:")
        positions = await broker.get_positions()
        if positions:
            for pos in positions:
                pnl_color = "+" if pos['unrealized_pl'] >= 0 else ""
                print(f"   {pos['symbol']}: {pos['qty']} shares @ ${pos['avg_entry_price']:.2f}")
                print(f"      P&L: {pnl_color}${pos['unrealized_pl']:.2f} ({pnl_color}{pos['unrealized_plpc']*100:.2f}%)")
        else:
            print("   No open positions")

        # 6. Place a test order (commented out for safety)
        # print("\n6. Placing test order (PAPER TRADING):")
        # order = await broker.submit_order(
        #     symbol="AAPL",
        #     qty=1,
        #     side="buy",
        #     type="market"
        # )
        # print(f"   Order ID: {order['order_id']}")
        # print(f"   Status: {order['status']}")

        print("\n=== Example Complete ===\n")
        print("✅ Alpaca broker integration working!")

    except Exception as e:
        print(f"\n❌ Error: {e}")
        print("\nMake sure you have:")
        print("1. Alpaca account (free at https://alpaca.markets)")
        print("2. API keys in environment variables")
        print("3. alpaca-trade-api installed: pip install alpaca-trade-api")


if __name__ == "__main__":
    # Load environment variables
    from dotenv import load_dotenv
    load_dotenv()

    asyncio.run(main())
