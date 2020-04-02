#!/bin/bash
#
# NOTE: Create an AWS instance with enough SSD storage in the primary 
# partitions (100GB is more than enough)

rm -fr *

sudo apt-get update
sudo apt-get -y upgrade
sudo apt-get -y autoremove

# The following dependencies prevent the error: [error: linker `cc` not found]
sudo apt -y install build-essential
sudo apt -y install cmake

# Get Twins and install rust
git clone https://github.com/sheharbano/libra.git
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
rustup default nightly

# Install rockDB
git clone https://github.com/tikv/rust-rocksdb.git
cd rust-rocksdb/
git submodule update --init --recursive
cargo build

cd
