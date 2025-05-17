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

conf_dir="${XDG_CONFIG_HOME:-$HOME/.config}/history-grep"
echo "Using configurationg directory ${conf_dir}"
mkdir -p ${conf_dir}
cp bash-integration.sh ${conf_dir}
echo "Add the following to your .bashrc:"
echo ""
echo 'source ${XDG_CONFIG_HOME:-$HOME/.config}/history-grep/bash-integration.sh'
echo ""

hash -r 
command -v hgr >/dev/null || echo "hgr not found in PATH. Ensure ${install_dir} is in PATH"

