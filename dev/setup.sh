#!/bin/bash
set -e

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

# Detect project root
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo -e "${GREEN}🦀 LiveClaw Dev Environment Setup${NC}"
echo ""

# 1. Check Rust toolchain
if ! command -v cargo &> /dev/null; then
    echo -e "${RED}❌ cargo not found. Install Rust: https://rustup.rs${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} Rust $(rustc --version | cut -d' ' -f2)"

# 2. Check ADK-Rust crates are available
ADK_PATH="$PROJECT_ROOT/../adk-rust"
if [ ! -d "$ADK_PATH/adk-core" ]; then
    echo -e "${RED}❌ ADK-Rust not found at $ADK_PATH${NC}"
    echo "  Expected: ../adk-rust/ relative to liveclaw/"
    echo "  Clone it: git clone <adk-rust-repo> ../adk-rust"
    exit 1
fi
echo -e "${GREEN}✓${NC} ADK-Rust found at $ADK_PATH"

# Count available crates
ADK_CRATES=(adk-core adk-realtime adk-session adk-tool adk-runner adk-auth adk-memory adk-graph adk-plugin adk-telemetry)
MISSING=()
for crate in "${ADK_CRATES[@]}"; do
    if [ ! -d "$ADK_PATH/$crate" ]; then
        MISSING+=("$crate")
    fi
done

if [ ${#MISSING[@]} -gt 0 ]; then
    echo -e "${YELLOW}⚠  Missing ADK crates: ${MISSING[*]}${NC}"
    echo "  Build may fail until these are available."
else
    echo -e "${GREEN}✓${NC} All ${#ADK_CRATES[@]} ADK-Rust crates present"
fi

# 3. Copy dev config if not present
CONFIG_FILE="$PROJECT_ROOT/liveclaw.toml"
if [ ! -f "$CONFIG_FILE" ]; then
    cp "$SCRIPT_DIR/liveclaw.dev.toml" "$CONFIG_FILE"
    echo -e "${GREEN}✓${NC} Created liveclaw.toml from dev template"
else
    echo -e "${GREEN}✓${NC} liveclaw.toml already exists"
fi

# 4. Install required components
echo ""
echo -e "${YELLOW}Installing Rust components...${NC}"
rustup component add clippy rustfmt 2>/dev/null || true
echo -e "${GREEN}✓${NC} clippy + rustfmt installed"

# 5. Verify workspace compiles
echo ""
echo -e "${YELLOW}Checking workspace...${NC}"
cd "$PROJECT_ROOT"
if cargo check 2>/dev/null; then
    echo -e "${GREEN}✓${NC} Workspace compiles"
else
    echo -e "${YELLOW}⚠  cargo check failed — expected if ADK crates have unmet deps${NC}"
    echo "  This is normal for initial setup. Fix as you go."
fi

echo ""
echo -e "${GREEN}🚀 Dev environment ready!${NC}"
echo ""
echo "  Run:    cargo check          # verify compilation"
echo "  Run:    cargo test            # run tests"
echo "  Run:    cargo clippy          # lint"
echo "  Run:    cargo run -p liveclaw-app -- liveclaw.toml"
echo ""
echo "  Config: liveclaw.toml (edit API key before running)"
