#!/bin/bash

# Setup script for local CI environment

set -e

echo "Setting up local CI environment..."

# Check if running as root
if [[ $EUID -eq 0 ]]; then
   echo "This script should not be run as root" 
   exit 1
fi

# Install act if not present
if ! command -v act &> /dev/null; then
    echo "Installing act..."
    curl -s https://raw.githubusercontent.com/nektos/act/master/install.sh | sudo bash
    sudo mv ./bin/act /usr/local/bin/
    echo "✓ act installed"
else
    echo "✓ act already installed"
fi

# Check Docker
if ! command -v docker &> /dev/null; then
    echo "Installing Docker..."
    curl -fsSL https://get.docker.com -o get-docker.sh
    sudo sh get-docker.sh
    sudo usermod -aG docker $USER
    sudo systemctl start docker
    sudo systemctl enable docker
    echo "✓ Docker installed (you may need to log out and back in)"
else
    echo "✓ Docker already installed"
fi

# Check Rust
if ! command -v cargo &> /dev/null; then
    echo "Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source ~/.cargo/env
    echo "✓ Rust installed"
else
    echo "✓ Rust already installed"
fi

# Install Rust components
echo "Installing Rust components..."
rustup component add rustfmt clippy
echo "✓ Rust components installed"

# Make scripts executable
chmod +x run-ci.sh
echo "✓ Scripts made executable"

echo ""
echo "Setup complete! You can now use:"
echo "  ./run-ci.sh --simple fmt      # Quick format check"
echo "  ./run-ci.sh --simple clippy   # Quick lint check"
echo "  ./run-ci.sh --list            # List all available jobs"
echo ""
echo "See CI_LOCAL.md for full documentation."
