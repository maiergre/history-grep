#!/usr/bin/env bash 

script_dir=$(dirname $BASH_SOURCE)
install_dir=~/.local/bin

set -e 
set -o pipefail
set -o nounset

cd $script_dir

echo "Building history-grep"
cargo build --release

echo "Ensuring install directory ${install_dir} exists"
mkdir -p ${install_dir}

echo "Installing hgr"
cp target/release/hgr ${install_dir}/

hash -r 
command -v hgr || echo "hgr not found in PATH. Ensure ${install_dir} is in PATH"

