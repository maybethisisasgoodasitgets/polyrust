#!/bin/bash
# Automated deployment script for crypto_arb_bot
# Usage: ./deploy.sh [--skip-tests]

set -e  # Exit on error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
PROJECT_DIR="/root/polyrust/rust"
BOT_NAME="crypto_arb_bot"
LOG_FILE="crypto_arb.log"
BUILD_LOG="build.log"
TEST_LOG="test.log"

# Parse arguments
SKIP_TESTS=false
if [[ "$1" == "--skip-tests" ]]; then
    SKIP_TESTS=true
fi

echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BLUE}   🚀 Crypto Arb Bot Deployment Script${NC}"
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

# Step 1: Stop existing bot
echo -e "${YELLOW}[1/6] Stopping existing bot...${NC}"
pkill -f "$BOT_NAME" && echo -e "${GREEN}✓ Bot stopped${NC}" || echo -e "${YELLOW}⚠ No running bot found${NC}"
sleep 2

# Step 2: Navigate to project directory
echo -e "${YELLOW}[2/6] Navigating to project directory...${NC}"
cd "$PROJECT_DIR" || {
    echo -e "${RED}✗ Failed to navigate to $PROJECT_DIR${NC}"
    exit 1
}
echo -e "${GREEN}✓ In directory: $(pwd)${NC}"

# Step 3: Pull latest code
echo -e "${YELLOW}[3/6] Pulling latest code from git...${NC}"
git fetch origin crypto-arbitrage
BEFORE_COMMIT=$(git rev-parse HEAD)
git pull origin crypto-arbitrage || {
    echo -e "${RED}✗ Git pull failed${NC}"
    exit 1
}
AFTER_COMMIT=$(git rev-parse HEAD)

if [ "$BEFORE_COMMIT" == "$AFTER_COMMIT" ]; then
    echo -e "${BLUE}ℹ No new commits (already up to date)${NC}"
else
    echo -e "${GREEN}✓ Updated from $BEFORE_COMMIT to $AFTER_COMMIT${NC}"
    echo -e "${BLUE}Recent changes:${NC}"
    git log --oneline "$BEFORE_COMMIT".."$AFTER_COMMIT" | head -5
fi

# Step 4: Build
echo -e "${YELLOW}[4/6] Building release binary...${NC}"
echo "Build started at: $(date)" > "$BUILD_LOG"
~/.cargo/bin/cargo build --release --bin "$BOT_NAME" 2>&1 | tee -a "$BUILD_LOG" | tail -20 || {
    echo -e "${RED}✗ Build failed. Check $BUILD_LOG for details${NC}"
    exit 1
}
echo -e "${GREEN}✓ Build successful${NC}"

# Step 5: Run tests (optional)
if [ "$SKIP_TESTS" = false ]; then
    echo -e "${YELLOW}[5/6] Running tests...${NC}"
    echo "Tests started at: $(date)" > "$TEST_LOG"
    ~/.cargo/bin/cargo test --release 2>&1 | tee -a "$TEST_LOG" | grep -E "(test result|running)" || {
        echo -e "${RED}✗ Tests failed. Check $TEST_LOG for details${NC}"
        exit 1
    }
    echo -e "${GREEN}✓ Tests passed${NC}"
else
    echo -e "${BLUE}[5/6] Skipping tests (--skip-tests flag)${NC}"
fi

# Step 6: Start bot
echo -e "${YELLOW}[6/6] Starting bot...${NC}"
# Archive old log
if [ -f "$LOG_FILE" ]; then
    TIMESTAMP=$(date +%Y%m%d_%H%M%S)
    mv "$LOG_FILE" "${LOG_FILE}.${TIMESTAMP}.old"
    echo -e "${BLUE}ℹ Old log archived to ${LOG_FILE}.${TIMESTAMP}.old${NC}"
fi

# Start bot in background
nohup target/release/"$BOT_NAME" > "$LOG_FILE" 2>&1 &
BOT_PID=$!
echo -e "${GREEN}✓ Bot started with PID: $BOT_PID${NC}"

# Wait and verify startup
sleep 5
if ps -p $BOT_PID > /dev/null; then
    echo -e "${GREEN}✓ Bot is running${NC}"
    echo ""
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${GREEN}   ✅ DEPLOYMENT SUCCESSFUL${NC}"
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
    echo -e "${BLUE}📊 Status:${NC}"
    echo "   PID: $BOT_PID"
    echo "   Log: $PROJECT_DIR/$LOG_FILE"
    echo ""
    echo -e "${BLUE}💡 Useful commands:${NC}"
    echo "   View live log:  tail -f $PROJECT_DIR/$LOG_FILE"
    echo "   Check process:  ps aux | grep $BOT_NAME | grep -v grep"
    echo "   Stop bot:       pkill -f $BOT_NAME"
    echo ""
    echo -e "${YELLOW}Showing last 30 lines of log:${NC}"
    echo "----------------------------------------"
    tail -30 "$LOG_FILE"
else
    echo -e "${RED}✗ Bot failed to start. Check $LOG_FILE for errors${NC}"
    tail -50 "$LOG_FILE"
    exit 1
fi
