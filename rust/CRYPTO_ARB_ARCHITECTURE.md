# Crypto Arbitrage Bot - Architecture & How It Works

## Plain English Summary

This bot makes money by being **faster than the market**. It watches real-time crypto prices on Binance and bets on Polymarket's "Will BTC/ETH/SOL/XRP go up or down?" prediction markets before other traders react.

**Example:** If Bitcoin jumps +0.15% in 30 seconds with strong momentum, the bot quickly buys "YES" on the "Will BTC go up in the next 15 minutes?" market at 50¢. If BTC keeps rising, that bet becomes worth closer to $1.

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                           CRYPTO ARBITRAGE BOT                                   │
└─────────────────────────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────────────────────┐
│                              DATA SOURCES                                         │
├──────────────────────────────────────────────────────────────────────────────────┤
│                                                                                   │
│   ┌─────────────────────┐         ┌─────────────────────┐                        │
│   │   BINANCE           │         │   POLYMARKET        │                        │
│   │   WebSocket Feeds   │         │   Gamma API         │                        │
│   ├─────────────────────┤         ├─────────────────────┤                        │
│   │ • BTC/USDT trades   │         │ • Market discovery  │                        │
│   │ • ETH/USDT trades   │         │ • Find active 5m,   │                        │
│   │ • SOL/USDT trades   │         │   15m, 4h markets   │                        │
│   │ • XRP/USDT trades   │         │ • Get token IDs     │                        │
│   │                     │         │                     │                        │
│   │ ~100 updates/sec    │         │ Refresh every 3s    │                        │
│   └──────────┬──────────┘         └──────────┬──────────┘                        │
│              │                               │                                    │
│              ▼                               ▼                                    │
│   ┌─────────────────────────────────────────────────────────────────┐            │
│   │                      PRICE STATE                                 │            │
│   │  ┌─────────┬─────────┬─────────┬─────────┐                      │            │
│   │  │   BTC   │   ETH   │   SOL   │   XRP   │                      │            │
│   │  ├─────────┼─────────┼─────────┼─────────┤                      │            │
│   │  │ $95,464 │ $3,316  │ $187.45 │ $2.45   │  ← Current prices    │            │
│   │  │ $95,400 │ $3,310  │ $187.00 │ $2.44   │  ← Interval start    │            │
│   │  │ +0.07%  │ +0.18%  │ +0.24%  │ +0.41%  │  ← Change %          │            │
│   │  │ [hist]  │ [hist]  │ [hist]  │ [hist]  │  ← Last 10 prices    │            │
│   │  └─────────┴─────────┴─────────┴─────────┘                      │            │
│   └─────────────────────────────────────────────────────────────────┘            │
│                                                                                   │
└──────────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌──────────────────────────────────────────────────────────────────────────────────┐
│                           SIGNAL DETECTION                                        │
├──────────────────────────────────────────────────────────────────────────────────┤
│                                                                                   │
│   For each asset (BTC, ETH, SOL, XRP):                                           │
│                                                                                   │
│   ┌─────────────────────────────────────────────────────────────────┐            │
│   │  1. MINIMUM PRICE MOVE CHECK                                     │            │
│   │     ┌────────────────────────────────────────────┐              │            │
│   │     │  Market Type  │  Min Move Required         │              │            │
│   │     ├───────────────┼────────────────────────────┤              │            │
│   │     │  5-minute     │  0.05%                     │              │            │
│   │     │  15-minute    │  0.10%  ← Most common      │              │            │
│   │     │  1-hour       │  0.20%                     │              │            │
│   │     │  4-hour       │  0.30%                     │              │            │
│   │     └────────────────────────────────────────────┘              │            │
│   │     If price hasn't moved enough → SKIP (no signal)             │            │
│   └─────────────────────────────────────────────────────────────────┘            │
│                                      │                                            │
│                                      ▼                                            │
│   ┌─────────────────────────────────────────────────────────────────┐            │
│   │  2. MOMENTUM CHECK                                               │            │
│   │                                                                  │            │
│   │     Analyze last 10 price samples:                              │            │
│   │                                                                  │            │
│   │     Score: -1.0 ◄────────── 0 ──────────► +1.0                  │            │
│   │            Strong Down    Neutral    Strong Up                   │            │
│   │                                                                  │            │
│   │     ✓ Consistency: Are moves in same direction?                 │            │
│   │     ✓ Acceleration: Is it speeding up or slowing?               │            │
│   │     ✓ Direction Match: Does momentum support our bet?           │            │
│   │                                                                  │            │
│   │     If momentum is AGAINST price direction → SKIP               │            │
│   │     If momentum is WEAK and decelerating → SKIP                 │            │
│   └─────────────────────────────────────────────────────────────────┘            │
│                                      │                                            │
│                                      ▼                                            │
│   ┌─────────────────────────────────────────────────────────────────┐            │
│   │  3. EDGE CALCULATION                                             │            │
│   │                                                                  │            │
│   │     "Edge" = How much better our odds are vs the market         │            │
│   │                                                                  │            │
│   │     Example (15-min market, BTC up +0.20%):                     │            │
│   │     • Estimated true probability: 60% (BTC will stay up)        │            │
│   │     • Market price: 50¢ (implies 50% probability)               │            │
│   │     • Edge: 60% - 50% = 10% edge                                │            │
│   │                                                                  │            │
│   │     Minimum edge required:                                       │            │
│   │     • 5-min markets:  0.5%                                      │            │
│   │     • 15-min markets: 1.0%                                      │            │
│   │     • 1-hour markets: 2.0%                                      │            │
│   │     • 4-hour markets: 3.0%                                      │            │
│   │                                                                  │            │
│   │     If edge too small → SKIP                                    │            │
│   └─────────────────────────────────────────────────────────────────┘            │
│                                      │                                            │
│                                      ▼                                            │
│   ┌─────────────────────────────────────────────────────────────────┐            │
│   │  4. GENERATE SIGNAL                                              │            │
│   │                                                                  │            │
│   │     ┌─────────────────────────────────────────────────┐         │            │
│   │     │  🎰 SIGNAL: ⬆️ UP                                │         │            │
│   │     │  Asset: BTC                                      │         │            │
│   │     │  Price: $95,500 (+0.15%)                        │         │            │
│   │     │  Buy: YES token @ 50¢                           │         │            │
│   │     │  Edge: 2.5%                                      │         │            │
│   │     │  Confidence: 65%                                 │         │            │
│   │     │  Size: $5.00                                     │         │            │
│   │     └─────────────────────────────────────────────────┘         │            │
│   └─────────────────────────────────────────────────────────────────┘            │
│                                                                                   │
└──────────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌──────────────────────────────────────────────────────────────────────────────────┐
│                           TRADE EXECUTION                                         │
├──────────────────────────────────────────────────────────────────────────────────┤
│                                                                                   │
│   ┌─────────────────────────────────────────────────────────────────┐            │
│   │  POSITION CHECK                                                  │            │
│   │  • Already have open BTC position? → Skip BTC signals           │            │
│   │  • Already have open ETH position? → Skip ETH signals           │            │
│   │  • Can hold up to 4 positions (one per asset)                   │            │
│   └─────────────────────────────────────────────────────────────────┘            │
│                                      │                                            │
│                          ┌───────────┴───────────┐                               │
│                          ▼                       ▼                               │
│              ┌─────────────────────┐  ┌─────────────────────┐                    │
│              │    MOCK MODE        │  │    LIVE MODE        │                    │
│              │    (Paper Trade)    │  │    (Real Money)     │                    │
│              ├─────────────────────┤  ├─────────────────────┤                    │
│              │ • Log trade details │  │ • Build order       │                    │
│              │ • Track position    │  │ • Sign with wallet  │                    │
│              │ • Simulate P&L      │  │ • Submit to CLOB    │                    │
│              │ • No real money     │  │ • Track fill        │                    │
│              └─────────────────────┘  └─────────────────────┘                    │
│                                                                                   │
└──────────────────────────────────────────────────────────────────────────────────┘
                                      │
                                      ▼
┌──────────────────────────────────────────────────────────────────────────────────┐
│                           POSITION MANAGEMENT                                     │
├──────────────────────────────────────────────────────────────────────────────────┤
│                                                                                   │
│   ┌─────────────────────────────────────────────────────────────────┐            │
│   │  OPEN POSITIONS (Example)                                        │            │
│   │  ┌──────┬────────────┬───────────┬──────────┬─────────────────┐ │            │
│   │  │Asset │ Direction  │ Entry     │ Size     │ Entry Price     │ │            │
│   │  ├──────┼────────────┼───────────┼──────────┼─────────────────┤ │            │
│   │  │ BTC  │ YES (UP)   │ $95,400   │ $5.00    │ 50¢             │ │            │
│   │  │ ETH  │ NO (DOWN)  │ $3,320    │ $5.00    │ 48¢             │ │            │
│   │  │ SOL  │ -          │ -         │ -        │ -               │ │            │
│   │  │ XRP  │ YES (UP)   │ $2.44     │ $5.00    │ 52¢             │ │            │
│   │  └──────┴────────────┴───────────┴──────────┴─────────────────┘ │            │
│   └─────────────────────────────────────────────────────────────────┘            │
│                                      │                                            │
│                                      ▼                                            │
│   ┌─────────────────────────────────────────────────────────────────┐            │
│   │  EXIT CONDITIONS (checked every 100ms)                           │            │
│   │                                                                  │            │
│   │     ┌─────────────────────────────────────────────────┐         │            │
│   │     │  ✅ TAKE PROFIT: P&L >= +15%                     │         │            │
│   │     │     → Sell position, lock in gains              │         │            │
│   │     ├─────────────────────────────────────────────────┤         │            │
│   │     │  ❌ STOP LOSS: P&L <= -10%                       │         │            │
│   │     │     → Sell position, cut losses                 │         │            │
│   │     ├─────────────────────────────────────────────────┤         │            │
│   │     │  ⏰ TIME EXIT: Held for 80% of interval          │         │            │
│   │     │     → Exit before market resolves               │         │            │
│   │     │     → 15-min market = exit after 12 minutes     │         │            │
│   │     ├─────────────────────────────────────────────────┤         │            │
│   │     │  ⏳ MIN HOLD: Must hold at least 60 seconds      │         │            │
│   │     │     → Avoid exiting on noise                    │         │            │
│   │     └─────────────────────────────────────────────────┘         │            │
│   └─────────────────────────────────────────────────────────────────┘            │
│                                                                                   │
└──────────────────────────────────────────────────────────────────────────────────┘

```

---

## Data Flow Summary

```
    BINANCE                           POLYMARKET
       │                                   │
       │ Real-time prices                  │ Market discovery
       │ (WebSocket)                       │ (REST API)
       │                                   │
       ▼                                   ▼
┌─────────────────────────────────────────────────┐
│              CRYPTO ARB ENGINE                   │
│                                                  │
│  1. Update price state (BTC, ETH, SOL, XRP)     │
│  2. Calculate price change since interval start │
│  3. Analyze momentum (last 10 samples)          │
│  4. Check if move exceeds threshold             │
│  5. Calculate edge vs market odds               │
│  6. Generate signal if conditions met           │
│                                                  │
└─────────────────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────┐
│              TRADING STATE                       │
│                                                  │
│  • Track open positions per asset               │
│  • Record trades to CSV                         │
│  • Monitor P&L                                  │
│  • Check exit conditions                        │
│                                                  │
└─────────────────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────┐
│              POLYMARKET CLOB                     │
│                                                  │
│  • Submit buy orders                            │
│  • GTC (Good Till Cancelled) orders             │
│  • Signed with private key                      │
│                                                  │
└─────────────────────────────────────────────────┘
```

---

## Why This Works (The Edge)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                           THE LATENCY ADVANTAGE                                  │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                  │
│   Timeline of a typical trade:                                                  │
│                                                                                  │
│   T+0.0s    BTC price jumps from $95,400 → $95,500 (+0.10%)                    │
│             │                                                                    │
│   T+0.1s    Bot detects move via Binance WebSocket                             │
│             Bot calculates: momentum ✓, edge ✓, confidence ✓                   │
│             │                                                                    │
│   T+0.2s    Bot places order: BUY YES @ 50¢                                    │
│             │                                                                    │
│   T+1-5s    Other traders notice BTC moved                                     │
│             They start buying YES tokens too                                    │
│             YES price rises: 50¢ → 52¢ → 55¢                                   │
│             │                                                                    │
│   T+15min   Market resolves: BTC stayed up                                     │
│             YES token pays out $1.00                                            │
│             │                                                                    │
│   RESULT:   Bot bought at 50¢, worth $1.00 = +100% profit                      │
│             (Or sold early at 55¢ for +10% profit)                             │
│                                                                                  │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## Key Components

| Component | File | Purpose |
|-----------|------|---------|
| **Price State** | `crypto_arb.rs` | Stores current/historical prices for all 4 assets |
| **Momentum Detection** | `crypto_arb.rs` | Analyzes price acceleration and consistency |
| **Signal Generation** | `crypto_arb.rs` | Decides when to trade based on edge calculation |
| **Market Discovery** | `crypto_arb.rs` | Finds active Polymarket prediction markets |
| **Trading State** | `crypto_arb_bot.rs` | Manages open positions and exit logic |
| **Trade Execution** | `crypto_arb_bot.rs` | Submits orders to Polymarket CLOB |

---

## Configuration

| Setting | Default | Description |
|---------|---------|-------------|
| `MOCK_TRADING` | `true` | Paper trade (no real money) |
| `MAX_POSITION_USD` | `$10` | Maximum bet size |
| `MIN_POSITION_USD` | `$5` | Minimum bet size |
| `TAKE_PROFIT_PCT` | `+15%` | Exit when winning this much |
| `STOP_LOSS_PCT` | `-10%` | Exit when losing this much |
| `MIN_PRICE_MOVE_PCT` | `0.10%` | Minimum price change to trigger |

---

## Example Console Output

```
📡 Starting Binance BTC + ETH + SOL + XRP price feeds...
✅ Got initial BTC price: $95464.97
✅ Got initial ETH price: $3316.33
✅ Got initial SOL price: $187.45
✅ Got initial XRP price: $2.4521

📊 MULTI-MARKET MODE (4 assets):
   🟠 BTC: Bitcoin Up or Down 12:00PM-12:15PM ET - Yes: 50.00¢
   🔵 ETH: Ethereum Up or Down 12:00PM-12:15PM ET - Yes: 50.00¢
   🟣 SOL: Solana Up or Down 12:00PM-12:15PM ET - Yes: 50.00¢
   ⚪ XRP: XRP Up or Down 12:00PM-12:15PM ET - Yes: 50.00¢

🎯 Monitoring for arbitrage opportunities...

📈 BTC $95464⬆️+0.05% | ETH $3316⬆️+0.03% | SOL $187.4⬆️+0.08% | XRP $2.452⬇️-0.02% | T:0 O:0 | MOCK

🎰 SIGNAL: ⬆️ UP | BTC $95550.00 (+0.15%) | Buy @ 50.00¢ | Edge 2.1% | Conf 58%
   📝 [MOCK TRADE] 2026-01-18 15:32:00 UTC
      Market: Bitcoin Up or Down 12:30PM-12:45PM ET (15m)
      Asset: BTC
      Direction: BUY YES (UP)
      Position Size: $5.00

💰 [EXIT] TAKE PROFIT ✅ - 2026-01-18 15:38:00 UTC
      Market: Bitcoin Up or Down 12:30PM-12:45PM ET
      Asset: BTC
      P&L: +18.5% ($0.93)
```
