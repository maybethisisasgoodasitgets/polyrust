/// Orderbook Fetcher Module
/// 
/// Fetches and analyzes orderbook depth from Polymarket CLOB API

use anyhow::{Result, anyhow};
use serde::Deserialize;
use crate::strategy_filters::OrderbookDepth;

#[derive(Debug, Deserialize)]
struct OrderbookLevel {
    price: String,
    size: String,
}

#[derive(Debug, Deserialize)]
struct OrderbookResponse {
    bids: Vec<OrderbookLevel>,
    asks: Vec<OrderbookLevel>,
}

/// Fetch orderbook depth for a token from Polymarket CLOB
pub async fn fetch_orderbook_depth(token_id: &str) -> Result<OrderbookDepth> {
    let client = reqwest::Client::new();
    let url = format!("https://clob.polymarket.com/book?token_id={}", token_id);
    
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map_err(|e| anyhow!("Failed to fetch orderbook: {}", e))?;
    
    if !resp.status().is_success() {
        return Err(anyhow!("Orderbook API returned status: {}", resp.status()));
    }
    
    let book: OrderbookResponse = resp.json().await
        .map_err(|e| anyhow!("Failed to parse orderbook: {}", e))?;
    
    // Calculate depth: sum of (price * size) for top levels
    let mut bid_depth_usd = 0.0;
    let mut ask_depth_usd = 0.0;
    
    // Sum top 5 levels for each side
    for bid in book.bids.iter().take(5) {
        if let (Ok(price), Ok(size)) = (bid.price.parse::<f64>(), bid.size.parse::<f64>()) {
            bid_depth_usd += price * size;
        }
    }
    
    for ask in book.asks.iter().take(5) {
        if let (Ok(price), Ok(size)) = (ask.price.parse::<f64>(), ask.size.parse::<f64>()) {
            ask_depth_usd += price * size;
        }
    }
    
    // Calculate spread
    let best_bid = book.bids.first()
        .and_then(|b| b.price.parse::<f64>().ok())
        .unwrap_or(0.0);
    let best_ask = book.asks.first()
        .and_then(|a| a.price.parse::<f64>().ok())
        .unwrap_or(1.0);
    
    let spread_pct = if best_bid > 0.0 {
        ((best_ask - best_bid) / best_bid) * 100.0
    } else {
        100.0
    };
    
    Ok(OrderbookDepth {
        bid_depth_usd,
        ask_depth_usd,
        spread_pct,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_orderbook_depth_calculation() {
        let bids = vec![
            OrderbookLevel { price: "0.50".to_string(), size: "100.0".to_string() },
            OrderbookLevel { price: "0.49".to_string(), size: "200.0".to_string() },
        ];
        let asks = vec![
            OrderbookLevel { price: "0.51".to_string(), size: "150.0".to_string() },
            OrderbookLevel { price: "0.52".to_string(), size: "100.0".to_string() },
        ];
        
        let mut bid_depth = 0.0;
        let mut ask_depth = 0.0;
        
        for bid in &bids {
            let price: f64 = bid.price.parse().unwrap();
            let size: f64 = bid.size.parse().unwrap();
            bid_depth += price * size;
        }
        
        for ask in &asks {
            let price: f64 = ask.price.parse().unwrap();
            let size: f64 = ask.size.parse().unwrap();
            ask_depth += price * size;
        }
        
        // 0.50*100 + 0.49*200 = 50 + 98 = 148
        assert!((bid_depth - 148.0).abs() < 0.01);
        // 0.51*150 + 0.52*100 = 76.5 + 52 = 128.5
        assert!((ask_depth - 128.5).abs() < 0.01);
    }
    
    #[test]
    fn test_spread_calculation() {
        let best_bid = 0.50;
        let best_ask = 0.51;
        
        let spread_pct = ((best_ask - best_bid) / best_bid) * 100.0;
        assert!((spread_pct - 2.0).abs() < 0.01);
    }
}
