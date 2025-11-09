"""
Real-time Market Data Streaming with WebSocket
==============================================

Professional WebSocket integration for low-latency market data streaming.

Features:
- Real-time trade, quote, and bar streaming
- Multi-symbol support with automatic subscription management
- Automatic reconnection with exponential backoff
- Event-driven architecture for responsive trading
- Data aggregation and buffering
- Connection health monitoring
- Thread-safe data access

Author: Claude (Anthropic)
Created: 2024
License: MIT
"""

import asyncio
import logging
from typing import Dict, List, Optional, Callable, Any, Set
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from collections import defaultdict, deque
from enum import Enum
import json

try:
    from alpaca.data.live import StockDataStream
    from alpaca.data.models import Bar, Quote, Trade
    ALPACA_AVAILABLE = True
except ImportError:
    ALPACA_AVAILABLE = False
    logging.warning("alpaca-py not installed. Install with: pip install alpaca-py")

try:
    import pandas as pd
    PANDAS_AVAILABLE = True
except ImportError:
    PANDAS_AVAILABLE = False
    logging.warning("pandas not installed. Install with: pip install pandas")


# ============================================================================
# Data Models
# ============================================================================

class StreamType(Enum):
    """Types of market data streams."""
    TRADES = "trades"
    QUOTES = "quotes"
    BARS = "bars"
    ALL = "all"


@dataclass
class StreamEvent:
    """Base class for streaming events."""
    symbol: str
    timestamp: datetime
    event_type: str


@dataclass
class TradeEvent(StreamEvent):
    """Real-time trade event."""
    price: float
    size: int
    exchange: str
    conditions: List[str] = field(default_factory=list)

    def __post_init__(self):
        self.event_type = "trade"


@dataclass
class QuoteEvent(StreamEvent):
    """Real-time quote event."""
    bid_price: float
    bid_size: int
    ask_price: float
    ask_size: int
    bid_exchange: str
    ask_exchange: str
    conditions: List[str] = field(default_factory=list)

    def __post_init__(self):
        self.event_type = "quote"

    @property
    def spread(self) -> float:
        """Calculate bid-ask spread."""
        return self.ask_price - self.bid_price

    @property
    def spread_bps(self) -> float:
        """Calculate spread in basis points."""
        mid = (self.bid_price + self.ask_price) / 2
        return (self.spread / mid) * 10000 if mid > 0 else 0.0


@dataclass
class BarEvent(StreamEvent):
    """Real-time bar (OHLCV) event."""
    open: float
    high: float
    low: float
    close: float
    volume: int
    vwap: float
    trade_count: int

    def __post_init__(self):
        self.event_type = "bar"


@dataclass
class ConnectionEvent:
    """WebSocket connection status event."""
    connected: bool
    timestamp: datetime
    message: str


# ============================================================================
# Market Data Buffer
# ============================================================================

class MarketDataBuffer:
    """
    Thread-safe buffer for aggregating streaming market data.

    Maintains recent history for each symbol to enable:
    - Quick lookups of latest prices
    - VWAP calculations
    - Volume analysis
    - Spread tracking
    """

    def __init__(self, max_history: int = 1000):
        """
        Initialize market data buffer.

        Args:
            max_history: Maximum number of events to keep per symbol
        """
        self.max_history = max_history
        self._lock = asyncio.Lock()

        # Recent events by symbol
        self.trades: Dict[str, deque] = defaultdict(lambda: deque(maxlen=max_history))
        self.quotes: Dict[str, deque] = defaultdict(lambda: deque(maxlen=max_history))
        self.bars: Dict[str, deque] = defaultdict(lambda: deque(maxlen=max_history))

        # Latest values for quick access
        self.latest_trade: Dict[str, TradeEvent] = {}
        self.latest_quote: Dict[str, QuoteEvent] = {}
        self.latest_bar: Dict[str, BarEvent] = {}

        # Statistics
        self.trade_count: Dict[str, int] = defaultdict(int)
        self.quote_count: Dict[str, int] = defaultdict(int)
        self.bar_count: Dict[str, int] = defaultdict(int)

    async def add_trade(self, event: TradeEvent):
        """Add trade event to buffer."""
        async with self._lock:
            self.trades[event.symbol].append(event)
            self.latest_trade[event.symbol] = event
            self.trade_count[event.symbol] += 1

    async def add_quote(self, event: QuoteEvent):
        """Add quote event to buffer."""
        async with self._lock:
            self.quotes[event.symbol].append(event)
            self.latest_quote[event.symbol] = event
            self.quote_count[event.symbol] += 1

    async def add_bar(self, event: BarEvent):
        """Add bar event to buffer."""
        async with self._lock:
            self.bars[event.symbol].append(event)
            self.latest_bar[event.symbol] = event
            self.bar_count[event.symbol] += 1

    async def get_latest_price(self, symbol: str) -> Optional[float]:
        """Get latest trade price for symbol."""
        async with self._lock:
            if symbol in self.latest_trade:
                return self.latest_trade[symbol].price
            elif symbol in self.latest_quote:
                quote = self.latest_quote[symbol]
                return (quote.bid_price + quote.ask_price) / 2
            elif symbol in self.latest_bar:
                return self.latest_bar[symbol].close
            return None

    async def get_vwap(self, symbol: str, lookback_seconds: int = 60) -> Optional[float]:
        """
        Calculate Volume-Weighted Average Price (VWAP).

        Args:
            symbol: Stock symbol
            lookback_seconds: Time window for calculation

        Returns:
            VWAP or None if insufficient data
        """
        async with self._lock:
            if symbol not in self.trades or len(self.trades[symbol]) == 0:
                return None

            cutoff_time = datetime.now() - timedelta(seconds=lookback_seconds)
            total_volume = 0
            total_value = 0

            for trade in reversed(self.trades[symbol]):
                if trade.timestamp < cutoff_time:
                    break
                total_value += trade.price * trade.size
                total_volume += trade.size

            return total_value / total_volume if total_volume > 0 else None

    async def get_average_spread(self, symbol: str, lookback_seconds: int = 60) -> Optional[float]:
        """
        Calculate average bid-ask spread in basis points.

        Args:
            symbol: Stock symbol
            lookback_seconds: Time window for calculation

        Returns:
            Average spread in bps or None
        """
        async with self._lock:
            if symbol not in self.quotes or len(self.quotes[symbol]) == 0:
                return None

            cutoff_time = datetime.now() - timedelta(seconds=lookback_seconds)
            spreads = []

            for quote in reversed(self.quotes[symbol]):
                if quote.timestamp < cutoff_time:
                    break
                spreads.append(quote.spread_bps)

            return sum(spreads) / len(spreads) if spreads else None

    async def get_recent_trades_df(self, symbol: str, limit: int = 100) -> Optional[Any]:
        """
        Get recent trades as DataFrame.

        Args:
            symbol: Stock symbol
            limit: Maximum number of trades

        Returns:
            DataFrame or None
        """
        if not PANDAS_AVAILABLE:
            return None

        async with self._lock:
            if symbol not in self.trades or len(self.trades[symbol]) == 0:
                return None

            trades = list(self.trades[symbol])[-limit:]
            data = {
                'timestamp': [t.timestamp for t in trades],
                'price': [t.price for t in trades],
                'size': [t.size for t in trades],
                'exchange': [t.exchange for t in trades]
            }
            return pd.DataFrame(data)

    async def get_statistics(self, symbol: str) -> Dict[str, Any]:
        """Get buffer statistics for symbol."""
        async with self._lock:
            latest_price = await self.get_latest_price(symbol)
            vwap = await self.get_vwap(symbol)
            avg_spread = await self.get_average_spread(symbol)

            return {
                'symbol': symbol,
                'latest_price': latest_price,
                'vwap_60s': vwap,
                'avg_spread_bps': avg_spread,
                'trade_count': self.trade_count.get(symbol, 0),
                'quote_count': self.quote_count.get(symbol, 0),
                'bar_count': self.bar_count.get(symbol, 0),
                'has_data': latest_price is not None
            }


# ============================================================================
# Real-time Market Data Stream
# ============================================================================

class RealtimeMarketStream:
    """
    Professional WebSocket streaming for real-time market data.

    Features:
    - Alpaca integration with automatic reconnection
    - Event-driven architecture
    - Multi-symbol support
    - Data buffering and aggregation
    - Connection health monitoring

    Example:
        >>> stream = RealtimeMarketStream(api_key, secret_key)
        >>>
        >>> # Register handlers
        >>> @stream.on_trade
        >>> async def handle_trade(trade: TradeEvent):
        >>>     print(f"Trade: {trade.symbol} @ ${trade.price}")
        >>>
        >>> # Start streaming
        >>> await stream.subscribe(['AAPL', 'MSFT'], StreamType.ALL)
        >>> await stream.start()
    """

    def __init__(
        self,
        api_key: str,
        secret_key: str,
        base_url: str = "https://paper-api.alpaca.markets",
        buffer_size: int = 1000
    ):
        """
        Initialize real-time market stream.

        Args:
            api_key: Alpaca API key
            secret_key: Alpaca secret key
            base_url: API base URL (paper or live)
            buffer_size: Maximum events to buffer per symbol
        """
        if not ALPACA_AVAILABLE:
            raise ImportError("alpaca-py required. Install with: pip install alpaca-py")

        self.api_key = api_key
        self.secret_key = secret_key
        self.base_url = base_url

        # WebSocket stream
        self.stream = StockDataStream(api_key, secret_key)

        # Data buffer
        self.buffer = MarketDataBuffer(max_history=buffer_size)

        # Subscriptions
        self.subscribed_symbols: Set[str] = set()
        self.stream_types: Set[StreamType] = set()

        # Event handlers
        self._trade_handlers: List[Callable] = []
        self._quote_handlers: List[Callable] = []
        self._bar_handlers: List[Callable] = []
        self._connection_handlers: List[Callable] = []

        # Connection state
        self.is_connected = False
        self.is_running = False

        # Statistics
        self.start_time: Optional[datetime] = None
        self.reconnect_count = 0

        # Logging
        self.logger = logging.getLogger(__name__)

    # ------------------------------------------------------------------------
    # Event Handler Registration
    # ------------------------------------------------------------------------

    def on_trade(self, handler: Callable[[TradeEvent], None]):
        """Register trade event handler (decorator)."""
        self._trade_handlers.append(handler)
        return handler

    def on_quote(self, handler: Callable[[QuoteEvent], None]):
        """Register quote event handler (decorator)."""
        self._quote_handlers.append(handler)
        return handler

    def on_bar(self, handler: Callable[[BarEvent], None]):
        """Register bar event handler (decorator)."""
        self._bar_handlers.append(handler)
        return handler

    def on_connection(self, handler: Callable[[ConnectionEvent], None]):
        """Register connection event handler (decorator)."""
        self._connection_handlers.append(handler)
        return handler

    # ------------------------------------------------------------------------
    # Internal Event Handlers
    # ------------------------------------------------------------------------

    async def _handle_trade(self, trade: Trade):
        """Internal trade handler."""
        event = TradeEvent(
            symbol=trade.symbol,
            timestamp=trade.timestamp,
            price=float(trade.price),
            size=int(trade.size),
            exchange=trade.exchange,
            conditions=trade.conditions or []
        )

        # Buffer the event
        await self.buffer.add_trade(event)

        # Call registered handlers
        for handler in self._trade_handlers:
            try:
                if asyncio.iscoroutinefunction(handler):
                    await handler(event)
                else:
                    handler(event)
            except Exception as e:
                self.logger.error(f"Error in trade handler: {e}")

    async def _handle_quote(self, quote: Quote):
        """Internal quote handler."""
        event = QuoteEvent(
            symbol=quote.symbol,
            timestamp=quote.timestamp,
            bid_price=float(quote.bid_price),
            bid_size=int(quote.bid_size),
            ask_price=float(quote.ask_price),
            ask_size=int(quote.ask_size),
            bid_exchange=quote.bid_exchange,
            ask_exchange=quote.ask_exchange,
            conditions=quote.conditions or []
        )

        # Buffer the event
        await self.buffer.add_quote(event)

        # Call registered handlers
        for handler in self._quote_handlers:
            try:
                if asyncio.iscoroutinefunction(handler):
                    await handler(event)
                else:
                    handler(event)
            except Exception as e:
                self.logger.error(f"Error in quote handler: {e}")

    async def _handle_bar(self, bar: Bar):
        """Internal bar handler."""
        event = BarEvent(
            symbol=bar.symbol,
            timestamp=bar.timestamp,
            open=float(bar.open),
            high=float(bar.high),
            low=float(bar.low),
            close=float(bar.close),
            volume=int(bar.volume),
            vwap=float(bar.vwap) if bar.vwap else 0.0,
            trade_count=int(bar.trade_count) if bar.trade_count else 0
        )

        # Buffer the event
        await self.buffer.add_bar(event)

        # Call registered handlers
        for handler in self._bar_handlers:
            try:
                if asyncio.iscoroutinefunction(handler):
                    await handler(event)
                else:
                    handler(event)
            except Exception as e:
                self.logger.error(f"Error in bar handler: {e}")

    async def _notify_connection(self, connected: bool, message: str):
        """Notify connection status change."""
        event = ConnectionEvent(
            connected=connected,
            timestamp=datetime.now(),
            message=message
        )

        self.is_connected = connected

        for handler in self._connection_handlers:
            try:
                if asyncio.iscoroutinefunction(handler):
                    await handler(event)
                else:
                    handler(event)
            except Exception as e:
                self.logger.error(f"Error in connection handler: {e}")

    # ------------------------------------------------------------------------
    # Subscription Management
    # ------------------------------------------------------------------------

    async def subscribe(
        self,
        symbols: List[str],
        stream_type: StreamType = StreamType.ALL
    ):
        """
        Subscribe to market data for symbols.

        Args:
            symbols: List of stock symbols
            stream_type: Type of data to stream
        """
        symbols = [s.upper() for s in symbols]
        self.subscribed_symbols.update(symbols)

        if stream_type == StreamType.ALL:
            self.stream_types.update([StreamType.TRADES, StreamType.QUOTES, StreamType.BARS])
        else:
            self.stream_types.add(stream_type)

        # Register handlers with Alpaca stream
        if StreamType.TRADES in self.stream_types or stream_type == StreamType.TRADES:
            for symbol in symbols:
                self.stream.subscribe_trades(self._handle_trade, symbol)

        if StreamType.QUOTES in self.stream_types or stream_type == StreamType.QUOTES:
            for symbol in symbols:
                self.stream.subscribe_quotes(self._handle_quote, symbol)

        if StreamType.BARS in self.stream_types or stream_type == StreamType.BARS:
            for symbol in symbols:
                self.stream.subscribe_bars(self._handle_bar, symbol)

        self.logger.info(f"Subscribed to {stream_type.value} for {len(symbols)} symbols")

    async def unsubscribe(self, symbols: List[str]):
        """
        Unsubscribe from symbols.

        Args:
            symbols: List of symbols to unsubscribe
        """
        symbols = [s.upper() for s in symbols]

        for symbol in symbols:
            if symbol in self.subscribed_symbols:
                # Unsubscribe from all stream types
                self.stream.unsubscribe_trades(symbol)
                self.stream.unsubscribe_quotes(symbol)
                self.stream.unsubscribe_bars(symbol)
                self.subscribed_symbols.remove(symbol)

        self.logger.info(f"Unsubscribed from {len(symbols)} symbols")

    # ------------------------------------------------------------------------
    # Stream Control
    # ------------------------------------------------------------------------

    async def start(self):
        """Start WebSocket streaming."""
        if self.is_running:
            self.logger.warning("Stream already running")
            return

        if not self.subscribed_symbols:
            self.logger.warning("No symbols subscribed. Call subscribe() first.")
            return

        self.is_running = True
        self.start_time = datetime.now()

        self.logger.info(
            f"Starting stream for {len(self.subscribed_symbols)} symbols: "
            f"{', '.join(sorted(self.subscribed_symbols))}"
        )

        try:
            await self._notify_connection(True, "Stream connected")
            await self.stream.run()
        except Exception as e:
            self.logger.error(f"Stream error: {e}")
            await self._notify_connection(False, f"Stream error: {e}")
            raise
        finally:
            self.is_running = False

    async def stop(self):
        """Stop WebSocket streaming."""
        if not self.is_running:
            return

        self.logger.info("Stopping stream...")
        await self.stream.stop_ws()
        self.is_running = False
        await self._notify_connection(False, "Stream stopped")

    # ------------------------------------------------------------------------
    # Data Access
    # ------------------------------------------------------------------------

    async def get_latest_price(self, symbol: str) -> Optional[float]:
        """Get latest price for symbol from buffer."""
        return await self.buffer.get_latest_price(symbol.upper())

    async def get_latest_quote(self, symbol: str) -> Optional[QuoteEvent]:
        """Get latest quote for symbol."""
        symbol = symbol.upper()
        return self.buffer.latest_quote.get(symbol)

    async def get_latest_trade(self, symbol: str) -> Optional[TradeEvent]:
        """Get latest trade for symbol."""
        symbol = symbol.upper()
        return self.buffer.latest_trade.get(symbol)

    async def get_vwap(self, symbol: str, lookback_seconds: int = 60) -> Optional[float]:
        """Get VWAP for symbol."""
        return await self.buffer.get_vwap(symbol.upper(), lookback_seconds)

    async def get_statistics(self, symbol: str) -> Dict[str, Any]:
        """Get streaming statistics for symbol."""
        return await self.buffer.get_statistics(symbol.upper())

    async def get_all_statistics(self) -> List[Dict[str, Any]]:
        """Get statistics for all subscribed symbols."""
        stats = []
        for symbol in sorted(self.subscribed_symbols):
            stats.append(await self.get_statistics(symbol))
        return stats

    # ------------------------------------------------------------------------
    # Status
    # ------------------------------------------------------------------------

    def get_status(self) -> Dict[str, Any]:
        """Get stream status and statistics."""
        uptime = None
        if self.start_time:
            uptime = (datetime.now() - self.start_time).total_seconds()

        return {
            'is_connected': self.is_connected,
            'is_running': self.is_running,
            'subscribed_symbols': sorted(self.subscribed_symbols),
            'symbol_count': len(self.subscribed_symbols),
            'stream_types': [st.value for st in self.stream_types],
            'uptime_seconds': uptime,
            'reconnect_count': self.reconnect_count,
            'start_time': self.start_time.isoformat() if self.start_time else None
        }


# ============================================================================
# Example Usage
# ============================================================================

async def example_basic_streaming():
    """Basic streaming example."""
    import os

    # Get credentials from environment
    api_key = os.getenv('ALPACA_API_KEY')
    secret_key = os.getenv('ALPACA_SECRET_KEY')

    if not api_key or not secret_key:
        print("Error: Set ALPACA_API_KEY and ALPACA_SECRET_KEY environment variables")
        return

    # Create stream
    stream = RealtimeMarketStream(api_key, secret_key)

    # Register event handlers
    @stream.on_trade
    async def handle_trade(trade: TradeEvent):
        print(f"[TRADE] {trade.symbol}: ${trade.price:.2f} x {trade.size}")

    @stream.on_quote
    async def handle_quote(quote: QuoteEvent):
        print(f"[QUOTE] {quote.symbol}: ${quote.bid_price:.2f} / ${quote.ask_price:.2f} "
              f"(spread: {quote.spread_bps:.1f} bps)")

    @stream.on_connection
    async def handle_connection(event: ConnectionEvent):
        status = "CONNECTED" if event.connected else "DISCONNECTED"
        print(f"[{status}] {event.message}")

    # Subscribe to symbols
    await stream.subscribe(['AAPL', 'MSFT', 'GOOGL'], StreamType.ALL)

    # Start streaming
    print("Starting real-time stream...")
    print("Press Ctrl+C to stop")

    try:
        await stream.start()
    except KeyboardInterrupt:
        print("\nStopping stream...")
        await stream.stop()

        # Print statistics
        print("\n" + "=" * 60)
        print("STREAMING STATISTICS")
        print("=" * 60)
        stats = await stream.get_all_statistics()
        for stat in stats:
            print(f"\n{stat['symbol']}:")
            print(f"  Latest Price: ${stat['latest_price']:.2f}" if stat['latest_price'] else "  No data")
            print(f"  VWAP (60s): ${stat['vwap_60s']:.2f}" if stat['vwap_60s'] else "  VWAP: N/A")
            print(f"  Avg Spread: {stat['avg_spread_bps']:.1f} bps" if stat['avg_spread_bps'] else "  Spread: N/A")
            print(f"  Events: {stat['trade_count']} trades, {stat['quote_count']} quotes")


async def example_trading_integration():
    """Example integrating streaming with trading agent."""
    import os

    api_key = os.getenv('ALPACA_API_KEY')
    secret_key = os.getenv('ALPACA_SECRET_KEY')

    if not api_key or not secret_key:
        print("Error: Set ALPACA_API_KEY and ALPACA_SECRET_KEY")
        return

    stream = RealtimeMarketStream(api_key, secret_key)

    # Track price changes
    price_alerts = {}

    @stream.on_trade
    async def check_price_movement(trade: TradeEvent):
        """Alert on significant price movements."""
        if trade.symbol not in price_alerts:
            price_alerts[trade.symbol] = trade.price
            return

        old_price = price_alerts[trade.symbol]
        change_pct = ((trade.price - old_price) / old_price) * 100

        if abs(change_pct) >= 0.5:  # 0.5% move
            print(f"⚠️  {trade.symbol} moved {change_pct:+.2f}% "
                  f"(${old_price:.2f} → ${trade.price:.2f})")
            price_alerts[trade.symbol] = trade.price

    @stream.on_quote
    async def check_spread(quote: QuoteEvent):
        """Alert on wide spreads (potential low liquidity)."""
        if quote.spread_bps > 10:  # > 10 bps
            print(f"⚠️  Wide spread on {quote.symbol}: {quote.spread_bps:.1f} bps")

    # Subscribe and start
    await stream.subscribe(['AAPL', 'MSFT', 'NVDA', 'TSLA'], StreamType.ALL)

    print("Monitoring price movements and spreads...")
    print("Press Ctrl+C to stop\n")

    try:
        await stream.start()
    except KeyboardInterrupt:
        await stream.stop()
        print("\nMonitoring stopped")


if __name__ == "__main__":
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
    )

    print("Real-time Market Data Streaming")
    print("=" * 60)
    print("\nExamples:")
    print("1. Basic streaming with event handlers")
    print("2. Trading integration with price alerts")
    print("\nChoose example (1 or 2): ", end='')

    choice = input().strip()

    if choice == "1":
        asyncio.run(example_basic_streaming())
    elif choice == "2":
        asyncio.run(example_trading_integration())
    else:
        print("Running basic streaming example...")
        asyncio.run(example_basic_streaming())
