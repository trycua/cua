"""
Market Regime Detection
======================

Professional market regime classification for adaptive trading strategies.

Market regimes include:
- Trending (bullish/bearish)
- Range-bound (sideways)
- High/low volatility
- Normal/crisis conditions

Author: Claude (Anthropic)
Created: 2024
License: MIT
"""

import logging
from typing import Dict, List, Optional, Tuple, Any
from dataclasses import dataclass
from datetime import datetime, timedelta
from enum import Enum
import warnings

warnings.filterwarnings('ignore')

try:
    import pandas as pd
    import numpy as np
    PANDAS_AVAILABLE = True
except ImportError:
    PANDAS_AVAILABLE = False
    logging.warning("pandas/numpy not installed")

try:
    import pandas_ta as ta
    PANDAS_TA_AVAILABLE = True
except ImportError:
    PANDAS_TA_AVAILABLE = False
    logging.warning("pandas-ta not installed")


# ============================================================================
# Regime Types
# ============================================================================

class TrendRegime(Enum):
    """Trend classification."""
    STRONG_BULLISH = "strong_bullish"
    BULLISH = "bullish"
    NEUTRAL = "neutral"
    BEARISH = "bearish"
    STRONG_BEARISH = "strong_bearish"


class VolatilityRegime(Enum):
    """Volatility classification."""
    VERY_LOW = "very_low"
    LOW = "low"
    NORMAL = "normal"
    HIGH = "high"
    EXTREME = "extreme"


class MarketRegime(Enum):
    """Overall market regime."""
    BULL_TRENDING = "bull_trending"
    BEAR_TRENDING = "bear_trending"
    RANGE_BOUND = "range_bound"
    HIGH_VOLATILITY = "high_volatility"
    CRISIS = "crisis"
    RECOVERY = "recovery"


@dataclass
class RegimeAnalysis:
    """Complete regime analysis result."""
    symbol: str
    timestamp: datetime

    # Trend analysis
    trend_regime: TrendRegime
    trend_strength: float  # 0 to 1
    trend_direction: float  # -1 to 1

    # Volatility analysis
    volatility_regime: VolatilityRegime
    volatility_percentile: float  # 0 to 100
    current_volatility: float

    # Market regime
    market_regime: MarketRegime
    regime_confidence: float  # 0 to 1

    # Regime metrics
    adx: float  # Trend strength
    atr_pct: float  # Volatility %
    rsi: float  # Momentum
    regime_duration_days: int

    # Trading recommendations
    recommended_strategy: str
    position_size_multiplier: float  # 0.5 to 2.0
    stop_loss_multiplier: float  # 1.0 to 3.0

    def __repr__(self) -> str:
        return (
            f"RegimeAnalysis({self.symbol}: {self.market_regime.value}, "
            f"trend={self.trend_regime.value}, vol={self.volatility_regime.value})"
        )


# ============================================================================
# Market Regime Detector
# ============================================================================

class MarketRegimeDetector:
    """
    Professional market regime detection system.

    Classifies market conditions across multiple dimensions:
    1. Trend (ADX, EMA slopes, price action)
    2. Volatility (ATR, Bollinger Bands, historical comparison)
    3. Overall regime (combining trend + volatility + momentum)

    Uses:
    - Adaptive strategy selection
    - Dynamic position sizing
    - Risk adjustment based on regime
    - Stop-loss widening in volatile regimes

    Example:
        >>> detector = MarketRegimeDetector()
        >>> regime = detector.detect_regime(ohlcv_df, "AAPL")
        >>> print(f"Regime: {regime.market_regime}")
        >>> print(f"Recommended: {regime.recommended_strategy}")
        >>> print(f"Position sizing: {regime.position_size_multiplier}x")
    """

    def __init__(
        self,
        lookback_period: int = 100,
        volatility_window: int = 20,
        regime_history_days: int = 252
    ):
        """
        Initialize regime detector.

        Args:
            lookback_period: Bars for trend calculation
            volatility_window: Window for volatility metrics
            regime_history_days: Historical data for percentile calculation
        """
        if not PANDAS_AVAILABLE or not PANDAS_TA_AVAILABLE:
            raise ImportError("pandas and pandas-ta required")

        self.lookback_period = lookback_period
        self.volatility_window = volatility_window
        self.regime_history_days = regime_history_days

        # Regime history tracking
        self.regime_history: Dict[str, List[RegimeAnalysis]] = {}

        self.logger = logging.getLogger(__name__)

    # ------------------------------------------------------------------------
    # Main Detection
    # ------------------------------------------------------------------------

    def detect_regime(
        self,
        df: pd.DataFrame,
        symbol: str,
        timestamp: Optional[datetime] = None
    ) -> RegimeAnalysis:
        """
        Detect current market regime.

        Args:
            df: OHLCV DataFrame with columns: open, high, low, close, volume
            symbol: Stock symbol
            timestamp: Analysis timestamp (default: now)

        Returns:
            RegimeAnalysis with complete classification
        """
        if timestamp is None:
            timestamp = datetime.now()

        # Ensure sufficient data
        if len(df) < self.lookback_period:
            raise ValueError(f"Insufficient data: need {self.lookback_period} bars, got {len(df)}")

        # Calculate all indicators
        df = self._calculate_indicators(df.copy())

        # Analyze each dimension
        trend_regime, trend_strength, trend_direction = self._analyze_trend(df)
        volatility_regime, vol_percentile, current_vol = self._analyze_volatility(df)
        market_regime, confidence = self._classify_market_regime(
            df, trend_regime, volatility_regime
        )

        # Get latest metrics
        latest = df.iloc[-1]
        adx = latest.get('ADX_14', 0)
        atr_pct = (latest.get('ATRr_14', 0) * 100) if 'ATRr_14' in df.columns else 0
        rsi = latest.get('RSI_14', 50)

        # Calculate regime duration
        regime_duration = self._calculate_regime_duration(symbol, market_regime)

        # Get trading recommendations
        strategy, pos_multiplier, sl_multiplier = self._get_trading_recommendations(
            market_regime, trend_strength, volatility_regime
        )

        analysis = RegimeAnalysis(
            symbol=symbol,
            timestamp=timestamp,
            trend_regime=trend_regime,
            trend_strength=trend_strength,
            trend_direction=trend_direction,
            volatility_regime=volatility_regime,
            volatility_percentile=vol_percentile,
            current_volatility=current_vol,
            market_regime=market_regime,
            regime_confidence=confidence,
            adx=adx,
            atr_pct=atr_pct,
            rsi=rsi,
            regime_duration_days=regime_duration,
            recommended_strategy=strategy,
            position_size_multiplier=pos_multiplier,
            stop_loss_multiplier=sl_multiplier
        )

        # Track regime history
        if symbol not in self.regime_history:
            self.regime_history[symbol] = []
        self.regime_history[symbol].append(analysis)

        return analysis

    # ------------------------------------------------------------------------
    # Indicator Calculation
    # ------------------------------------------------------------------------

    def _calculate_indicators(self, df: pd.DataFrame) -> pd.DataFrame:
        """Calculate all required technical indicators."""
        # Trend indicators
        df.ta.ema(length=20, append=True)
        df.ta.ema(length=50, append=True)
        df.ta.ema(length=200, append=True)
        df.ta.adx(length=14, append=True)

        # Volatility indicators
        df.ta.atr(length=14, append=True)
        df.ta.bbands(length=20, std=2, append=True)

        # Momentum indicators
        df.ta.rsi(length=14, append=True)
        df.ta.macd(fast=12, slow=26, signal=9, append=True)

        # Volume
        df.ta.obv(append=True)

        return df

    # ------------------------------------------------------------------------
    # Trend Analysis
    # ------------------------------------------------------------------------

    def _analyze_trend(self, df: pd.DataFrame) -> Tuple[TrendRegime, float, float]:
        """
        Analyze trend regime.

        Returns:
            (trend_regime, strength, direction)
        """
        latest = df.iloc[-1]

        # Get EMAs
        price = latest['close']
        ema20 = latest.get('EMA_20', price)
        ema50 = latest.get('EMA_50', price)
        ema200 = latest.get('EMA_200', price)

        # ADX for trend strength
        adx = latest.get('ADX_14', 0)

        # Calculate EMA alignment
        ema_alignment = 0
        if price > ema20 > ema50 > ema200:
            ema_alignment = 1.0  # Perfect bullish alignment
        elif price < ema20 < ema50 < ema200:
            ema_alignment = -1.0  # Perfect bearish alignment
        else:
            # Partial alignment
            bullish_count = sum([price > ema20, price > ema50, price > ema200])
            ema_alignment = (bullish_count - 1.5) / 1.5

        # Trend strength (0 to 1)
        trend_strength = min(adx / 50, 1.0)

        # Trend direction (-1 to 1)
        trend_direction = ema_alignment

        # Classify regime
        if adx > 40 and ema_alignment > 0.5:
            regime = TrendRegime.STRONG_BULLISH
        elif adx > 25 and ema_alignment > 0:
            regime = TrendRegime.BULLISH
        elif adx > 40 and ema_alignment < -0.5:
            regime = TrendRegime.STRONG_BEARISH
        elif adx > 25 and ema_alignment < 0:
            regime = TrendRegime.BEARISH
        else:
            regime = TrendRegime.NEUTRAL

        return regime, trend_strength, trend_direction

    # ------------------------------------------------------------------------
    # Volatility Analysis
    # ------------------------------------------------------------------------

    def _analyze_volatility(self, df: pd.DataFrame) -> Tuple[VolatilityRegime, float, float]:
        """
        Analyze volatility regime.

        Returns:
            (volatility_regime, percentile, current_volatility)
        """
        # Calculate ATR as percentage of price
        df['ATR_pct'] = (df.get('ATRr_14', 0) * 100)

        # Get current volatility
        current_vol = df['ATR_pct'].iloc[-1]

        # Calculate percentile in recent history
        lookback = min(len(df), self.regime_history_days)
        vol_percentile = (df['ATR_pct'].iloc[-lookback:] < current_vol).sum() / lookback * 100

        # Classify regime based on percentile
        if vol_percentile < 20:
            regime = VolatilityRegime.VERY_LOW
        elif vol_percentile < 40:
            regime = VolatilityRegime.LOW
        elif vol_percentile < 60:
            regime = VolatilityRegime.NORMAL
        elif vol_percentile < 80:
            regime = VolatilityRegime.HIGH
        else:
            regime = VolatilityRegime.EXTREME

        return regime, vol_percentile, current_vol

    # ------------------------------------------------------------------------
    # Market Regime Classification
    # ------------------------------------------------------------------------

    def _classify_market_regime(
        self,
        df: pd.DataFrame,
        trend: TrendRegime,
        volatility: VolatilityRegime
    ) -> Tuple[MarketRegime, float]:
        """
        Classify overall market regime.

        Returns:
            (market_regime, confidence)
        """
        latest = df.iloc[-1]
        adx = latest.get('ADX_14', 0)
        rsi = latest.get('RSI_14', 50)

        # Default regime and confidence
        regime = MarketRegime.RANGE_BOUND
        confidence = 0.5

        # Crisis detection (extreme volatility + bearish)
        if volatility in [VolatilityRegime.EXTREME, VolatilityRegime.HIGH]:
            if trend in [TrendRegime.STRONG_BEARISH, TrendRegime.BEARISH]:
                regime = MarketRegime.CRISIS
                confidence = 0.9
            else:
                regime = MarketRegime.HIGH_VOLATILITY
                confidence = 0.8

        # Recovery detection (volatility decreasing, trend turning bullish)
        elif volatility == VolatilityRegime.NORMAL:
            if trend == TrendRegime.BULLISH and rsi > 45:
                regime = MarketRegime.RECOVERY
                confidence = 0.75

        # Strong trending markets
        elif trend == TrendRegime.STRONG_BULLISH and adx > 35:
            regime = MarketRegime.BULL_TRENDING
            confidence = 0.85
        elif trend == TrendRegime.STRONG_BEARISH and adx > 35:
            regime = MarketRegime.BEAR_TRENDING
            confidence = 0.85

        # Moderate trending
        elif trend in [TrendRegime.BULLISH] and adx > 25:
            regime = MarketRegime.BULL_TRENDING
            confidence = 0.7
        elif trend in [TrendRegime.BEARISH] and adx > 25:
            regime = MarketRegime.BEAR_TRENDING
            confidence = 0.7

        # Range-bound (low ADX)
        elif adx < 20:
            regime = MarketRegime.RANGE_BOUND
            confidence = 0.8

        return regime, confidence

    # ------------------------------------------------------------------------
    # Trading Recommendations
    # ------------------------------------------------------------------------

    def _get_trading_recommendations(
        self,
        regime: MarketRegime,
        trend_strength: float,
        volatility: VolatilityRegime
    ) -> Tuple[str, float, float]:
        """
        Get trading recommendations based on regime.

        Returns:
            (strategy, position_size_multiplier, stop_loss_multiplier)
        """
        # Default recommendations
        strategy = "neutral"
        pos_multiplier = 1.0
        sl_multiplier = 1.0

        if regime == MarketRegime.BULL_TRENDING:
            strategy = "trend_following_long"
            pos_multiplier = 1.0 + (trend_strength * 0.5)  # 1.0 to 1.5
            sl_multiplier = 1.0

        elif regime == MarketRegime.BEAR_TRENDING:
            strategy = "trend_following_short"
            pos_multiplier = 0.8  # Reduce size for shorts
            sl_multiplier = 1.2  # Wider stops (bear markets more volatile)

        elif regime == MarketRegime.RANGE_BOUND:
            strategy = "mean_reversion"
            pos_multiplier = 0.8  # Smaller positions in chop
            sl_multiplier = 0.8  # Tighter stops

        elif regime == MarketRegime.HIGH_VOLATILITY:
            strategy = "volatility_breakout"
            pos_multiplier = 0.6  # Reduce size significantly
            sl_multiplier = 2.0  # Much wider stops

        elif regime == MarketRegime.CRISIS:
            strategy = "risk_off"
            pos_multiplier = 0.3  # Minimal positions
            sl_multiplier = 3.0  # Very wide stops

        elif regime == MarketRegime.RECOVERY:
            strategy = "early_trend_long"
            pos_multiplier = 1.2  # Slightly larger
            sl_multiplier = 1.5  # Moderate stops

        # Adjust for volatility
        if volatility == VolatilityRegime.EXTREME:
            pos_multiplier *= 0.5
            sl_multiplier *= 2.0
        elif volatility == VolatilityRegime.HIGH:
            pos_multiplier *= 0.75
            sl_multiplier *= 1.5
        elif volatility == VolatilityRegime.VERY_LOW:
            pos_multiplier *= 1.2
            sl_multiplier *= 0.8

        # Clamp values
        pos_multiplier = max(0.2, min(2.0, pos_multiplier))
        sl_multiplier = max(0.5, min(3.0, sl_multiplier))

        return strategy, pos_multiplier, sl_multiplier

    # ------------------------------------------------------------------------
    # Regime Duration
    # ------------------------------------------------------------------------

    def _calculate_regime_duration(self, symbol: str, current_regime: MarketRegime) -> int:
        """Calculate how many days current regime has persisted."""
        if symbol not in self.regime_history or not self.regime_history[symbol]:
            return 0

        # Count consecutive days in current regime
        duration = 0
        for analysis in reversed(self.regime_history[symbol]):
            if analysis.market_regime == current_regime:
                duration += 1
            else:
                break

        return duration

    # ------------------------------------------------------------------------
    # Regime Transitions
    # ------------------------------------------------------------------------

    def detect_regime_transition(
        self,
        symbol: str,
        current_regime: MarketRegime
    ) -> Optional[Dict[str, Any]]:
        """
        Detect if regime has changed.

        Returns:
            Transition info or None
        """
        if symbol not in self.regime_history or len(self.regime_history[symbol]) < 2:
            return None

        previous = self.regime_history[symbol][-2]

        if previous.market_regime != current_regime:
            return {
                'symbol': symbol,
                'from_regime': previous.market_regime.value,
                'to_regime': current_regime.value,
                'previous_duration': self._calculate_regime_duration(symbol, previous.market_regime),
                'timestamp': datetime.now(),
                'significance': self._assess_transition_significance(
                    previous.market_regime, current_regime
                )
            }

        return None

    def _assess_transition_significance(
        self,
        from_regime: MarketRegime,
        to_regime: MarketRegime
    ) -> str:
        """Assess significance of regime transition."""
        # Critical transitions
        if (from_regime != MarketRegime.CRISIS and to_regime == MarketRegime.CRISIS):
            return "CRITICAL"
        if (from_regime == MarketRegime.CRISIS and to_regime == MarketRegime.RECOVERY):
            return "CRITICAL"

        # Major transitions
        if (from_regime in [MarketRegime.BULL_TRENDING, MarketRegime.BEAR_TRENDING] and
            to_regime in [MarketRegime.BEAR_TRENDING, MarketRegime.BULL_TRENDING]):
            return "MAJOR"

        # Minor transitions
        return "MINOR"

    # ------------------------------------------------------------------------
    # Reporting
    # ------------------------------------------------------------------------

    def get_regime_summary(self, symbol: str) -> str:
        """Get human-readable regime summary."""
        if symbol not in self.regime_history or not self.regime_history[symbol]:
            return f"No regime data for {symbol}"

        latest = self.regime_history[symbol][-1]

        summary = f"""
Market Regime Analysis: {symbol}
{'=' * 60}
Timestamp: {latest.timestamp.strftime('%Y-%m-%d %H:%M:%S')}

OVERALL REGIME: {latest.market_regime.value.upper()}
Confidence: {latest.regime_confidence * 100:.1f}%
Duration: {latest.regime_duration_days} periods

TREND
  Regime: {latest.trend_regime.value}
  Strength: {latest.trend_strength * 100:.1f}%
  Direction: {latest.trend_direction:+.2f}
  ADX: {latest.adx:.1f}

VOLATILITY
  Regime: {latest.volatility_regime.value}
  Percentile: {latest.volatility_percentile:.1f}%
  Current ATR: {latest.atr_pct:.2f}%

MOMENTUM
  RSI: {latest.rsi:.1f}

TRADING RECOMMENDATIONS
  Strategy: {latest.recommended_strategy}
  Position Size: {latest.position_size_multiplier:.2f}x normal
  Stop Loss: {latest.stop_loss_multiplier:.2f}x normal
{'=' * 60}
"""
        return summary


# ============================================================================
# Example Usage
# ============================================================================

def example_regime_detection():
    """Example: Detect market regime."""
    import yfinance as yf

    print("Market Regime Detection Example")
    print("=" * 60)

    # Download data
    symbol = "AAPL"
    print(f"\nDownloading {symbol} data...")
    data = yf.download(symbol, period="1y", interval="1d", progress=False)

    if data.empty:
        print("Error: No data downloaded")
        return

    # Prepare DataFrame
    df = pd.DataFrame({
        'open': data['Open'],
        'high': data['High'],
        'low': data['Low'],
        'close': data['Close'],
        'volume': data['Volume']
    })

    # Detect regime
    detector = MarketRegimeDetector()
    regime = detector.detect_regime(df, symbol)

    # Print summary
    print(detector.get_regime_summary(symbol))

    # Trading advice
    print("\nTRADING ADVICE:")
    if regime.market_regime == MarketRegime.CRISIS:
        print("⚠️  CRISIS MODE - Reduce exposure, preserve capital")
    elif regime.market_regime == MarketRegime.HIGH_VOLATILITY:
        print("⚠️  HIGH VOLATILITY - Use smaller positions, wider stops")
    elif regime.market_regime == MarketRegime.BULL_TRENDING:
        print("✅ BULL TREND - Follow trend, use trailing stops")
    elif regime.market_regime == MarketRegime.BEAR_TRENDING:
        print("⚠️  BEAR TREND - Consider shorts or stay defensive")
    elif regime.market_regime == MarketRegime.RANGE_BOUND:
        print("📊 RANGE-BOUND - Mean reversion strategies, quick profits")
    elif regime.market_regime == MarketRegime.RECOVERY:
        print("🟢 RECOVERY - Early trend entry opportunity")


def example_multi_symbol():
    """Example: Compare regimes across multiple symbols."""
    import yfinance as yf

    symbols = ['AAPL', 'MSFT', 'TSLA', 'SPY']

    print("\nMulti-Symbol Regime Analysis")
    print("=" * 80)

    detector = MarketRegimeDetector()

    for symbol in symbols:
        print(f"\nAnalyzing {symbol}...")
        data = yf.download(symbol, period="1y", interval="1d", progress=False)

        if data.empty:
            continue

        df = pd.DataFrame({
            'open': data['Open'],
            'high': data['High'],
            'low': data['Low'],
            'close': data['Close'],
            'volume': data['Volume']
        })

        regime = detector.detect_regime(df, symbol)

        print(f"{symbol:6} | {regime.market_regime.value:20} | "
              f"Trend: {regime.trend_regime.value:15} | "
              f"Vol: {regime.volatility_regime.value:10} | "
              f"Strategy: {regime.recommended_strategy}")


if __name__ == "__main__":
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s - %(levelname)s - %(message)s'
    )

    print("\nChoose example:")
    print("1. Single symbol regime detection")
    print("2. Multi-symbol comparison")
    print("\nChoice (1 or 2): ", end='')

    choice = input().strip()

    if choice == "2":
        example_multi_symbol()
    else:
        example_regime_detection()
