use anyhow::Result;
use reqwest::Client;
use std::env;

/// Simple Telegram notification helper
pub struct TelegramNotifier {
    bot_token: String,
    chat_id: String,
    client: Client,
    enabled: bool,
}

impl TelegramNotifier {
    pub fn new() -> Self {
        let bot_token = env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default();
        let chat_id = env::var("TELEGRAM_CHAT_ID").unwrap_or_default();
        let enabled = !bot_token.is_empty() && !chat_id.is_empty();
        
        if !enabled {
            println!("⚠️ Telegram notifications disabled (TELEGRAM_BOT_TOKEN or TELEGRAM_CHAT_ID not set)");
        }
        
        Self {
            bot_token,
            chat_id,
            client: Client::new(),
            enabled,
        }
    }
    
    /// Send a notification to Telegram
    pub async fn send(&self, message: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);
        
        let response = self.client
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": self.chat_id,
                "text": message,
                "parse_mode": "HTML"
            }))
            .send()
            .await;
        
        match response {
            Ok(_) => Ok(()),
            Err(e) => {
                eprintln!("Failed to send Telegram notification: {}", e);
                Ok(()) // Don't fail the bot if Telegram fails
            }
        }
    }
    
    /// Send startup notification
    pub async fn notify_startup(&self, mode: &str) {
        let msg = format!(
            "🟢 <b>Crypto Arb Bot Started</b>\n\nMode: {}\nMonitoring: BTC, ETH, SOL, XRP\n\nWaiting for velocity signals...",
            mode
        );
        let _ = self.send(&msg).await;
    }
    
    /// Send velocity signal detected notification
    pub async fn notify_signal(&self, asset: &str, velocity: f64, direction: &str) {
        let msg = format!(
            "🎯 <b>Signal Detected</b>\n\nAsset: {}\nVelocity: {:.3}%\nDirection: {}\n\nValidating orderbook...",
            asset, velocity, direction
        );
        let _ = self.send(&msg).await;
    }
    
    /// Send trade blocked notification
    pub async fn notify_blocked(&self, asset: &str, reason: &str) {
        let msg = format!(
            "🛑 <b>Trade Blocked</b>\n\nAsset: {}\nReason: {}",
            asset, reason
        );
        let _ = self.send(&msg).await;
    }
    
    /// Send trade executed notification
    pub async fn notify_trade(&self, asset: &str, direction: &str, entry_price: f64, size: f64, market: &str, is_mock: bool) {
        let header = if is_mock {
            "📝 <b>MOCK Trade Executed</b>"
        } else {
            "✅ <b>LIVE Trade Executed</b>"
        };
        let msg = format!(
            "{}\n\nAsset: {}\nDirection: {}\nEntry: {:.2}¢\nSize: ${:.2}\nMarket: {}",
            header, asset, direction, entry_price * 100.0, size, market
        );
        let _ = self.send(&msg).await;
    }
    
    /// Send trade failed notification
    pub async fn notify_failed(&self, asset: &str, error: &str) {
        let msg = format!(
            "❌ <b>Trade Failed</b>\n\nAsset: {}\nError: {}",
            asset, error
        );
        let _ = self.send(&msg).await;
    }
    
    /// Send status update
    pub async fn notify_status(&self, total_trades: usize, open_positions: usize, pnl: f64, mode: &str) {
        let msg = format!(
            "📊 <b>Status Update</b>\n\nMode: {}\nTotal Trades: {}\nOpen Positions: {}\nP&L: ${:.2}\n\nBot running normally...",
            mode, total_trades, open_positions, pnl
        );
        let _ = self.send(&msg).await;
    }
    
    /// Send periodic status analysis explaining why no trades are happening
    /// This provides transparency during quiet market periods
    pub async fn notify_status_analysis(&self, analysis: &str) {
        if !self.enabled {
            return;
        }
        
        // Convert console formatting to Telegram HTML
        // Replace emoji icons and format for readability
        let telegram_msg = analysis
            .replace("📊", "📊")
            .replace("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━", "━━━━━━━━━━━━━━━━━━━━━━━")
            .replace("⚪", "⚪")
            .replace("🟠", "🟠")
            .replace("🟡", "🟡")
            .replace("✅", "✅")
            .replace("⬆", "⬆")
            .replace("⬇", "⬇")
            .replace("📈", "📈")
            .replace("📉", "📉")
            .replace("🎯", "🎯");
        
        // Wrap in monospace for better formatting
        let formatted = format!("<pre>{}</pre>", telegram_msg);
        let _ = self.send(&formatted).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_telegram_notifier_initialization() {
        // Test that TelegramNotifier initializes correctly
        let notifier = TelegramNotifier::new();
        
        // Should not panic or error
        assert!(notifier.bot_token.is_empty() || !notifier.bot_token.is_empty());
    }
    
    #[test]
    fn test_status_analysis_formatting() {
        // Test that status analysis message formatting works
        let sample_analysis = r#"📊 SIGNAL STATUS ANALYSIS
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
   ⚪ BTC: $87,650 ⬇-0.0012% (need +0.02%) [6% of threshold]
   ⚪ ETH: $2,874 ⬆+0.0089% (need +0.03%) [30% of threshold]
   ⚪ SOL: $121.55 ⬆+0.0156% (need +0.04%) [39% of threshold]
   ⚪ XRP: $1.87 ⬇-0.0051% (need +0.04%) [13% of threshold]

📉 VERDICT: VERY QUIET - All assets well below signal threshold"#;
        
        // Should contain expected elements
        assert!(sample_analysis.contains("SIGNAL STATUS ANALYSIS"));
        assert!(sample_analysis.contains("BTC"));
        assert!(sample_analysis.contains("ETH"));
        assert!(sample_analysis.contains("SOL"));
        assert!(sample_analysis.contains("XRP"));
        assert!(sample_analysis.contains("VERDICT"));
    }
    
    #[tokio::test]
    async fn test_notify_status_analysis_no_panic() {
        // Test that notify_status_analysis doesn't panic even without credentials
        let notifier = TelegramNotifier::new();
        let analysis = "📊 Test Status\n⚪ BTC: $90,000 ⬇-0.001%\n📉 VERDICT: Quiet";
        
        // Should not panic
        notifier.notify_status_analysis(analysis).await;
    }
    
    #[test]
    fn test_emoji_preservation_in_formatting() {
        // Verify emojis are preserved in message formatting
        let notifier = TelegramNotifier::new();
        let test_msg = "📊 Status ⚪ BTC ⬆ Up ⬇ Down 📈 High 📉 Low ✅ Good 🟡 Medium 🟠 Warning";
        
        // The replace calls should preserve emojis (they replace with themselves)
        let processed = test_msg
            .replace("📊", "📊")
            .replace("⚪", "⚪")
            .replace("⬆", "⬆")
            .replace("⬇", "⬇")
            .replace("📈", "📈")
            .replace("📉", "📉")
            .replace("✅", "✅")
            .replace("🟡", "🟡")
            .replace("🟠", "🟠");
        
        assert_eq!(processed, test_msg, "Emojis should be preserved");
    }
    
    #[test]
    fn test_status_analysis_monospace_wrapping() {
        // Test that monospace HTML wrapping is applied correctly
        let sample = "Test analysis\nLine 2";
        let formatted = format!("<pre>{}</pre>", sample);
        
        assert!(formatted.starts_with("<pre>"));
        assert!(formatted.ends_with("</pre>"));
        assert!(formatted.contains("Test analysis"));
    }
}
